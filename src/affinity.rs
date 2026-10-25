//! Shared coding-session assignments. Only identifiers and account ids are persisted;
//! conversation input stays in a bounded, process-local continuation cache.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use axum::http::HeaderMap;
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::accounts::{Account, Only, PROVIDERS, Pick, Pool, Provider};
use crate::config::{Config, Routing};
use crate::ir::Format;

mod persistence;
use persistence::Persistence;

const MAX_SESSIONS: usize = 10_000;
const MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_RESPONSES: usize = 1_000;
/// Recent assignments count immediately, including the gaps between coding turns.
const LOAD_IDLE_SECONDS: i64 = 5 * 60;
/// For display only: a session quiet for longer than the load window, waiting on its
/// user or a long tool run, is still going on. Routing never reads this.
const ONGOING_IDLE_SECONDS: i64 = 30 * 60;
/// Session source for a client that sent no session id: the conversation's first user message.
pub const INFERRED_SOURCE: &str = "conversation_start";

fn digest(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    hex::encode(h.finalize())
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok().map(str::trim).filter(|s| !s.is_empty())
}

fn identifier(v: &Value) -> Option<&str> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty() && s.len() <= 1024)
}

/// The internal scope header. Authentication middleware overwrites it (and strips
/// [`LEGACY_SCOPE_HEADER`]), including for query-string keys.
pub const SCOPE_HEADER: &str = "x-fusebox-client-scope";
/// The scope header's name before the rename to Fusebox.
pub const LEGACY_SCOPE_HEADER: &str = "x-cliproxy-client-scope";

/// A header sent under its Fusebox name or its pre-rename `x-cliproxy-*` name; the new name wins.
pub fn renamed_header<'a>(headers: &'a HeaderMap, name: &str, legacy: &str) -> Option<&'a str> {
    header(headers, name).or_else(|| header(headers, legacy))
}

/// True when the client asks to end its session (`x-fusebox-session-end: true`).
pub fn ends_session(headers: &HeaderMap) -> bool {
    renamed_header(headers, "x-fusebox-session-end", "x-cliproxy-session-end") == Some("true")
}

pub fn client_scope(headers: &HeaderMap) -> String {
    if let Some(scope) = renamed_header(headers, SCOPE_HEADER, LEGACY_SCOPE_HEADER) {
        return scope.to_string();
    }
    let key = header(headers, "authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .or_else(|| header(headers, "x-api-key"))
        .or_else(|| header(headers, "x-goog-api-key"))
        .unwrap_or("anonymous");
    digest(&["client", key])
}

pub fn scope_for_key(key: Option<&str>) -> String {
    digest(&["client", key.unwrap_or("anonymous")])
}

pub struct SessionIdentity {
    pub key: String,
    pub source: &'static str,
}

/// Read the original client request before provider translation or Claude cloaking.
/// The source identifies a field; the raw client identifier is never returned.
pub fn session_identity(headers: &HeaderMap, body: &Value) -> Option<SessionIdentity> {
    let claude_user = body["metadata"]["user_id"].as_str().unwrap_or_default();
    let claude_json: Value = serde_json::from_str(claude_user).unwrap_or(Value::Null);
    let (id, source) = [
        "x-fusebox-session-id",
        // The same header before the rename to Fusebox.
        "x-cliproxy-session-id",
        "thread-id",
        "x-codex-thread-id",
        "session_id",
        "session-id",
        "x-claude-code-session-id",
    ]
    .into_iter()
    .find_map(|name| header(headers, name).filter(|s| s.len() <= 1024).map(|id| (id, name)))
    .or_else(|| identifier(&claude_json["session_id"]).map(|id| (id, "metadata.user_id.session_id")))
    .or_else(|| {
        claude_user
            .strip_prefix("user_")
            .filter(|s| s.contains("_account_"))?
            .rsplit_once("_session_")
            .map(|(_, s)| s)
            .filter(|s| !s.is_empty() && s.len() <= 1024)
            .map(|id| (id, "metadata.user_id"))
    })
    .or_else(|| identifier(&body["metadata"]["session_id"]).map(|id| (id, "metadata.session_id")))
    .or_else(|| identifier(&body["metadata"]["thread_id"]).map(|id| (id, "metadata.thread_id")))
    .or_else(|| identifier(&body["conversation"]["id"]).map(|id| (id, "conversation.id")))
    .or_else(|| identifier(&body["conversation"]).map(|id| (id, "conversation")))
    .or_else(|| identifier(&body["prompt_cache_key"]).map(|id| (id, "prompt_cache_key")))?;
    Some(SessionIdentity { key: digest(&[&client_scope(headers), "session", id]), source })
}

/// Identify a request without a session id by the start of its conversation. Every turn
/// repeats the first user message, so a conversation's turns share an account and with
/// it the provider's prompt cache. Cache markers are ignored because clients move them
/// from turn to turn. Conversations that start identically simply share an account.
pub fn inferred_identity(headers: &HeaderMap, format: Format, body: &Value) -> Option<SessionIdentity> {
    let content = first_user_content(format, body)?;
    let mut hasher = Sha256::new();
    if !hash_content(content, &mut hasher) {
        return None;
    }
    let start = hex::encode(hasher.finalize());
    Some(SessionIdentity { key: digest(&[&client_scope(headers), "conversation", &start]), source: INFERRED_SOURCE })
}

fn first_user_content(format: Format, body: &Value) -> Option<&Value> {
    let is_user = |m: &&Value| m["role"] == "user";
    match format {
        Format::Claude | Format::Chat => body["messages"].as_array()?.iter().find(is_user).map(|m| &m["content"]),
        Format::Responses => match &body["input"] {
            text @ Value::String(_) => Some(text),
            Value::Array(items) => items.iter().find(is_user).map(|m| &m["content"]),
            _ => None,
        },
        Format::Gemini => body["contents"]
            .as_array()?
            .iter()
            .find(|c| c["role"] == "user" || c["role"].is_null())
            .map(|c| &c["parts"]),
    }
}

/// Feed a message's text (and any other parts, without cache markers) to the hasher.
/// A plain string and a single text block hash the same. Returns whether anything was fed.
fn hash_content(content: &Value, hasher: &mut Sha256) -> bool {
    let mut feed = |bytes: &[u8]| {
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    match content {
        Value::String(text) if !text.is_empty() => feed(text.as_bytes()),
        Value::Array(parts) => {
            let mut fed = false;
            for part in parts {
                match &part["text"] {
                    Value::String(text) => feed(text.as_bytes()),
                    _ if part.is_object() => {
                        let mut part = part.clone();
                        strip_cache_markers(&mut part);
                        feed(&serde_json::to_vec(&part).unwrap_or_default());
                    }
                    _ => continue,
                }
                fed = true;
            }
            return fed;
        }
        _ => return false,
    }
    true
}

fn strip_cache_markers(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("cache_control");
            object.remove("prompt_cache_breakpoint");
            object.values_mut().for_each(strip_cache_markers);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_cache_markers),
        _ => {}
    }
}

/// A provider cache key for a session: stable per session, never the session key itself.
/// Shaped like the UUIDs Codex sends as its own `prompt_cache_key`.
pub fn prompt_cache_key(session: &str) -> String {
    let d = digest(&["prompt-cache-key", session]);
    format!("{}-{}-{}-{}-{}", &d[..8], &d[8..12], &d[12..16], &d[16..20], &d[20..32])
}

pub fn session_key(headers: &HeaderMap, body: &Value) -> Option<String> {
    session_identity(headers, body).map(|identity| identity.key)
}

pub fn connection_key(headers: &HeaderMap, id: &str) -> String {
    digest(&[&client_scope(headers), "connection", id])
}

// Keep a subscription across model changes within a provider, while respecting
// explicit account prefixes and intentional switches to another provider.
fn route_scope(pool: &Pool, model: &str, only: Option<&Only>) -> String {
    match only {
        Some(Only::Prefix(p)) => format!("prefix:{}", p.to_ascii_lowercase()),
        Some(Only::Provider(p)) => format!("provider:{}", p.as_str()),
        None => {
            let vendor = |name: &str| {
                PROVIDERS
                    .iter()
                    .find(|p| {
                        !matches!(p, Provider::Antigravity | Provider::Devin | Provider::Compat) && p.family(name)
                    })
                    .copied()
            };
            vendor(model)
                .or_else(|| {
                    pool.all().iter().find_map(|a| pool.resolve_account(a, model, None).and_then(|m| vendor(&m)))
                })
                .map(|p| format!("provider:{}", p.as_str()))
                .unwrap_or_else(|| format!("model:{}", model.to_ascii_lowercase()))
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Binding {
    #[serde(default)]
    session: String,
    account: String,
    last_seen: i64,
    /// Made for a client without a session id; expires sooner and is evicted first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    inferred: bool,
}

/// Immutable shared history segments. Incremental native WebSocket turns append
/// only their delta; deep copies and full serialization happen only for fallback.
#[derive(Clone)]
pub struct History {
    tail: Option<Arc<HistorySegment>>,
    len: usize,
    bytes: usize,
    depth: usize,
}

impl Default for History {
    fn default() -> Self {
        Self { tail: None, len: 0, bytes: 2, depth: 0 }
    }
}

struct HistorySegment {
    previous: Option<Arc<HistorySegment>>,
    items: Vec<Value>,
}

impl From<Vec<Value>> for History {
    fn from(items: Vec<Value>) -> Self {
        let mut history = Self::default();
        history.extend(items);
        history
    }
}

impl History {
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn extend(&mut self, items: Vec<Value>) {
        if items.is_empty() {
            return;
        }
        let size = history_size(&items, &[], MAX_RESPONSE_BYTES).unwrap_or(MAX_RESPONSE_BYTES + 1);
        // Array brackets are shared; the joining comma is needed only when nonempty.
        let bytes = if self.len == 0 { size } else { self.bytes.saturating_add(size.saturating_sub(1)) };
        if self.depth >= 1024 {
            let values = self.to_values();
            self.tail = Some(Arc::new(HistorySegment { previous: None, items: values }));
            self.depth = 1;
        }
        self.len += items.len();
        self.bytes = bytes;
        self.depth += 1;
        self.tail = Some(Arc::new(HistorySegment { previous: self.tail.take(), items }));
    }

    pub fn to_values(&self) -> Vec<Value> {
        let mut segments = Vec::with_capacity(self.depth);
        let mut next = self.tail.as_deref();
        while let Some(segment) = next {
            segments.push(segment);
            next = segment.previous.as_deref();
        }
        let mut values = Vec::with_capacity(self.len);
        for segment in segments.into_iter().rev() {
            values.extend(segment.items.iter().cloned());
        }
        values
    }
}

struct Conversation {
    session: String,
    input: History,
    bytes: usize,
    last_seen: i64,
}

#[derive(Default)]
struct Registry {
    bindings: HashMap<String, Binding>,
    responses: HashMap<String, Conversation>,
    history_bytes: usize,
    dirty: bool,
    last_flush: i64,
    active: HashMap<String, usize>,
    /// When the request path last pruned; it prunes at most every few seconds.
    last_prune: i64,
    /// Binding keys per owner, for releasing a lease without scanning every binding.
    /// Rebuilt when invalidated or when the number of bindings changed.
    by_session: HashMap<String, Vec<String>>,
    indexed: Option<usize>,
}

/// How often the request path sweeps expired assignments. The binding a request
/// looks up is always checked exactly; the sweep only frees memory.
const PRUNE_EVERY_SECONDS: i64 = 5;

impl Registry {
    fn session_keys(&mut self, owner: &str) -> Vec<String> {
        if self.indexed != Some(self.bindings.len()) {
            self.by_session.clear();
            for (key, b) in &self.bindings {
                self.by_session.entry(b.session.clone()).or_default().push(key.clone());
            }
            self.indexed = Some(self.bindings.len());
        }
        self.by_session.get(owner).cloned().unwrap_or_default()
    }

    fn invalidate_index(&mut self) {
        self.indexed = None;
    }

    fn account_load(&self, cfg: &Config) -> HashMap<String, usize> {
        if !weighs_session_load(cfg) {
            return HashMap::new();
        }
        self.recent_sessions(cfg.session_affinity_idle_seconds)
    }

    /// Sessions per account seen in the last few minutes or with a call in flight.
    fn recent_sessions(&self, idle: u64) -> HashMap<String, usize> {
        let mut load = HashMap::new();
        let cutoff = Utc::now().timestamp() - LOAD_IDLE_SECONDS.min(idle.min(i64::MAX as u64) as i64);
        let mut seen = HashSet::new();
        for (key, b) in &self.bindings {
            let session = if b.session.is_empty() { key.as_str() } else { b.session.as_str() };
            if (b.last_seen > cutoff || self.active.contains_key(session)) && seen.insert((&b.account, session)) {
                *load.entry(b.account.clone()).or_insert(0) += 1;
            }
        }
        load
    }
}

impl Registry {
    /// Per account, in one pass: sessions in the load window (as `recent_sessions`) and
    /// sessions still going on (seen within `ONGOING_IDLE_SECONDS`). Calls in flight
    /// count in both.
    fn session_counts(&self, idle: u64) -> HashMap<String, (usize, usize)> {
        let idle = idle.min(i64::MAX as u64) as i64;
        let now = Utc::now().timestamp();
        let (recent, ongoing) = (now - LOAD_IDLE_SECONDS.min(idle), now - ONGOING_IDLE_SECONDS.min(idle));
        let mut latest: HashMap<(&str, &str), i64> = HashMap::new();
        for (key, b) in &self.bindings {
            let session = if b.session.is_empty() { key.as_str() } else { b.session.as_str() };
            let seen = if self.active.contains_key(session) { i64::MAX } else { b.last_seen };
            let entry = latest.entry((b.account.as_str(), session)).or_insert(i64::MIN);
            *entry = (*entry).max(seen);
        }
        let mut counts: HashMap<String, (usize, usize)> = HashMap::new();
        for ((account, _), seen) in latest.into_iter().filter(|(_, seen)| *seen > ongoing) {
            let count = counts.entry(account.to_string()).or_default();
            count.0 += usize::from(seen > recent);
            count.1 += 1;
        }
        counts
    }
}

fn weighs_session_load(cfg: &Config) -> bool {
    cfg.routing == Routing::SmartQuota && cfg.session_affinity
}

pub struct Sessions {
    registry: Arc<Mutex<Registry>>,
    persistence: Option<Persistence>,
    provider_sessions: crate::provider_sessions::Sessions,
}

/// Keep an assignment alive until a call (including its response stream) ends.
pub struct Lease {
    sessions: Arc<Sessions>,
    session: String,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut registry = self.sessions.registry.lock();
        if let Some(count) = registry.active.get_mut(&self.session) {
            *count -= 1;
            if *count == 0 {
                registry.active.remove(&self.session);
            }
        }
        let now = Utc::now().timestamp();
        for key in registry.session_keys(&self.session) {
            if let Some(binding) = registry.bindings.get_mut(&key)
                && binding.session == self.session
            {
                binding.last_seen = now;
            }
        }
        registry.dirty = true;
    }
}

/// A session assignment, as the dashboard lists it.
pub struct Pinned {
    pub owner: String,
    pub last_seen: i64,
    pub active: bool,
}

/// How an assignment names its session: request logs carry the session key, assignments this.
pub fn owner_of(session: &str) -> String {
    digest(&["owner", session])
}

pub type Selection = Result<(Arc<Account>, String), (u16, String)>;

#[derive(Clone)]
pub struct Selected {
    pub account: Arc<Account>,
    pub model: String,
    pub strategy: Routing,
    pub reason: &'static str,
    pub previous_account: Option<String>,
}

fn selection(pick: Pick, model: &str) -> Selection {
    match pick {
        Pick::Ok(a, m) => Ok((a, m)),
        Pick::Cooling(t) => Err((
            429,
            format!(
                "all accounts for {model} are rate limited; next available in {}s",
                (t - Utc::now()).num_seconds().max(1)
            ),
        )),
        Pick::None => Err((404, format!("no available account serves model `{model}`"))),
    }
}

/// When a session's own account is back, if only a pause keeps the session away from it.
/// Accounts the caller ruled out (already tried, wrong residency) don't count.
fn paused_until(acct: &Account, model: &str, exclude: &[String]) -> Option<chrono::DateTime<Utc>> {
    (!exclude.contains(&acct.id)).then(|| acct.cooling_until(model)).flatten()
}

/// A session leaving its paused account with no other account free waits for that
/// account (a 429 with the wait), rather than hearing that nothing serves the model.
fn or_paused(pick: Pick, paused: Option<chrono::DateTime<Utc>>) -> Pick {
    match (pick, paused) {
        (Pick::None, Some(until)) => Pick::Cooling(until),
        (Pick::Cooling(other), Some(until)) => Pick::Cooling(other.min(until)),
        (pick, _) => pick,
    }
}

impl Sessions {
    pub fn hold(self: &Arc<Self>, session: &str, idle: u64) -> Lease {
        let session = digest(&["owner", session]);
        let mut registry = self.registry.lock();
        Self::prune_locked(&mut registry, idle);
        *registry.active.entry(session.clone()).or_default() += 1;
        Lease { sessions: self.clone(), session }
    }

    pub fn end(&self, session: &str) {
        self.provider_sessions.end(session);
        let owner = digest(&["owner", session]);
        let mut registry = self.registry.lock();
        registry.bindings.retain(|_, b| b.session != owner);
        registry.invalidate_index();
        registry.responses.retain(|_, c| c.session != session);
        registry.history_bytes = registry.responses.values().map(|c| c.bytes).sum();
        registry.dirty = true;
        self.schedule_save();
    }
    pub fn load(auth_dir: &Path, idle: u64) -> Self {
        let path = auth_dir.join(".routing-sessions.state");
        let mut registry = Registry::default();
        match std::fs::read(&path) {
            Ok(data) => match serde_json::from_slice(&data) {
                Ok(bindings) => {
                    registry.bindings = bindings;
                    registry.invalidate_index();
                }
                Err(e) => tracing::warn!(path = %path.display(), "session assignments could not be read: {e}"),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(path = %path.display(), "session assignments could not be read: {e}"),
        }
        let registry = Arc::new(Mutex::new(registry));
        let persistence = Persistence::new(registry.clone(), path);
        let sessions = Self { registry, persistence, provider_sessions: Default::default() };
        sessions.prune(idle);
        sessions
    }

    #[cfg(test)]
    pub fn memory() -> Self {
        Self {
            registry: Arc::new(Mutex::new(Registry::default())),
            persistence: None,
            provider_sessions: Default::default(),
        }
    }

    /// Provider-visible identity follows only the current account stint. It is
    /// never restored from a previous process or conflated with cache routing.
    pub fn provider_session(&self, conversation: Option<&str>, account: &Account, idle: u64) -> String {
        self.provider_sessions.get(conversation, account, idle)
    }

    #[cfg(test)]
    pub fn pick(
        &self,
        pool: &Pool,
        cfg: &Config,
        model: &str,
        session: Option<&str>,
        exclude: &[String],
        only: Option<&Only>,
    ) -> Selection {
        self.pick_with_reason(pool, cfg, model, session, exclude, only)
            .map(|selected| (selected.account, selected.model))
    }

    /// Capture the routing reason while selecting under one lock. Concurrent calls
    /// cannot create competing assignments or report a stale migration decision.
    pub fn pick_with_reason(
        &self,
        pool: &Pool,
        cfg: &Config,
        model: &str,
        session: Option<&str>,
        exclude: &[String],
        only: Option<&Only>,
    ) -> Result<Selected, (u16, String)> {
        self.pick_session(pool, cfg, model, session, false, exclude, only)
    }

    /// [`Self::pick_with_reason`], recording whether a new assignment's session was inferred.
    #[allow(clippy::too_many_arguments)]
    pub fn pick_session(
        &self,
        pool: &Pool,
        cfg: &Config,
        model: &str,
        session: Option<&str>,
        inferred: bool,
        exclude: &[String],
        only: Option<&Only>,
    ) -> Result<Selected, (u16, String)> {
        self.pick_session_with_context(pool, cfg, model, session, inferred, exclude, only, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn pick_session_with_context(
        &self,
        pool: &Pool,
        cfg: &Config,
        model: &str,
        session: Option<&str>,
        inferred: bool,
        exclude: &[String],
        only: Option<&Only>,
        context: Option<&crate::latency::Context>,
    ) -> Result<Selected, (u16, String)> {
        let Some(session) = session.filter(|_| cfg.session_affinity) else {
            let (account, model) = selection(self.pick_unbound(pool, cfg, model, exclude, None, only), model)?;
            return Ok(Selected {
                account,
                model,
                strategy: cfg.routing,
                reason: if cfg.session_affinity { "missing_session" } else { "affinity_disabled" },
                previous_account: None,
            });
        };
        let key = digest(&[session, &route_scope(pool, model, only)]);
        let mut registry = self.registry.lock();
        Self::prune_due(&mut registry, cfg.session_affinity_idle_seconds);
        // The sweep runs every few seconds; this request's own assignment is checked now.
        if registry.bindings.get(&key).is_some_and(|b| Self::expired(&registry, b, cfg.session_affinity_idle_seconds)) {
            registry.bindings.remove(&key);
            registry.dirty = true;
            registry.invalidate_index();
        }
        let mut excluded = exclude.to_vec();
        // Why an existing assignment has to move, if it does.
        let mut moving: Option<&'static str> = None;
        let mut paused = None;
        if let Some(binding) = registry.bindings.get_mut(&key) {
            binding.last_seen = Utc::now().timestamp();
            let assigned = pool.get(&binding.account);
            let upstream = assigned.as_ref().and_then(|acct| pool.resolve_account(acct, model, only));
            match (&assigned, upstream) {
                (None, _) => moving = Some("account_removed"),
                (Some(acct), _) if acct.state.lock().disabled => moving = Some("account_disabled"),
                (Some(_), None) => moving = Some("model_unavailable"),
                (Some(acct), Some(_)) if acct.exhausted_until(model).is_some() => {
                    moving = Some("quota_exhausted");
                    paused = paused_until(acct, model, exclude);
                }
                (Some(acct), Some(upstream)) => {
                    if acct.cooling_until(model).is_none() && !excluded.contains(&acct.id) {
                        registry.dirty = true;
                        return Ok(Selected {
                            account: acct.clone(),
                            model: upstream,
                            strategy: cfg.routing,
                            reason: "session_reused",
                            previous_account: None,
                        });
                    }
                    // A rate limit or failed attempt is temporary: serve this request from
                    // another account but keep the assignment, so the session comes back
                    // (and finds its prompt cache) once its subscription recovers.
                    let assigned_id = acct.id.clone();
                    let paused = paused_until(acct, model, exclude);
                    if !excluded.contains(&assigned_id) {
                        excluded.push(assigned_id.clone());
                    }
                    let pick = pool.pick(model, &excluded, cfg, None, only, &registry.account_load(cfg));
                    let (account, model) = selection(or_paused(pick, paused), model)?;
                    return Ok(Selected {
                        account,
                        model,
                        strategy: cfg.routing,
                        reason: "temporary_detour",
                        previous_account: Some(assigned_id),
                    });
                }
            }
            if let Some(acct) = &assigned
                && !excluded.contains(&acct.id)
            {
                excluded.push(acct.id.clone());
            }
        } else if registry.bindings.len() >= MAX_SESSIONS {
            // Make room by forgetting the longest-idle assignment with nothing in flight,
            // inferred ones first: an explicit session id is the stronger claim.
            let oldest = registry
                .bindings
                .iter()
                .filter(|(_, b)| !registry.active.contains_key(&b.session))
                .min_by_key(|(_, b)| (!b.inferred, b.last_seen))
                .map(|(k, _)| k.clone());
            match oldest {
                Some(oldest) => {
                    registry.bindings.remove(&oldest);
                    registry.invalidate_index();
                }
                // Every assignment is in use: route this request on its own rather than fail it.
                None => {
                    let (account, model) =
                        selection(pool.pick(model, exclude, cfg, None, only, &registry.account_load(cfg)), model)?;
                    return Ok(Selected {
                        account,
                        model,
                        strategy: cfg.routing,
                        reason: "missing_session",
                        previous_account: None,
                    });
                }
            }
        }
        let pick = pool.pick_with_context(
            model,
            &excluded,
            cfg,
            None,
            only,
            &registry.account_load(cfg),
            context.filter(|_| moving.is_none() && exclude.is_empty()),
        );
        let (acct, upstream) = selection(or_paused(pick, paused), model)?;
        let previous = registry.bindings.insert(
            key,
            Binding {
                session: digest(&["owner", session]),
                account: acct.id.clone(),
                last_seen: Utc::now().timestamp(),
                inferred,
            },
        );
        if let Some(old) = &previous {
            tracing::info!(from = %old.account, to = %acct.id, reason = moving.unwrap_or("quota_exhausted"), "session moved to another account");
        }
        registry.dirty = true;
        self.schedule_save();
        let new_reason = if crate::banked_resets::priority_expiry(&acct, &upstream, cfg).is_some() {
            "expiring_reset"
        } else {
            "new_session"
        };
        Ok(Selected {
            account: acct,
            model: upstream,
            strategy: cfg.routing,
            reason: if previous.is_some() { moving.unwrap_or("quota_exhausted") } else { new_reason },
            previous_account: previous.map(|binding| binding.account),
        })
    }

    pub fn prune(&self, idle: u64) {
        let mut registry = self.registry.lock();
        Self::prune_locked(&mut registry, idle);
    }

    /// Sessions per account that smart balancing counts right now (empty for other strategies).
    pub fn account_load(&self, cfg: &Config) -> HashMap<String, usize> {
        self.registry.lock().account_load(cfg)
    }

    /// Per account, whatever the routing strategy: sessions seen in the last few minutes
    /// (the dashboard's live sessions) and sessions seen in the last half hour, both
    /// counting calls in flight.
    pub fn session_counts(&self, idle: u64) -> HashMap<String, (usize, usize)> {
        self.registry.lock().session_counts(idle)
    }

    /// Sessions currently assigned to `account`: their owner digest (see [`owner_of`]),
    /// when one was last seen, and whether a call is in flight. Most recent first.
    pub fn pinned(&self, account: &str, idle: u64) -> Vec<Pinned> {
        let mut registry = self.registry.lock();
        Self::prune_locked(&mut registry, idle);
        let mut out: HashMap<&str, Pinned> = HashMap::new();
        for (key, b) in registry.bindings.iter().filter(|(_, b)| b.account == account) {
            let owner = if b.session.is_empty() { key.as_str() } else { b.session.as_str() };
            let entry = out.entry(owner).or_insert_with(|| Pinned {
                owner: owner.to_string(),
                last_seen: b.last_seen,
                active: registry.active.contains_key(owner),
            });
            entry.last_seen = entry.last_seen.max(b.last_seen);
        }
        let mut out: Vec<Pinned> = out.into_values().collect();
        out.sort_by_key(|p| std::cmp::Reverse(p.last_seen));
        out
    }

    /// Retries and requests without affinity still respect the reserve and current load.
    pub fn pick_unbound(
        &self,
        pool: &Pool,
        cfg: &Config,
        model: &str,
        exclude: &[String],
        pinned: Option<&str>,
        only: Option<&Only>,
    ) -> Pick {
        // Only smart quota balancing weighs session load; other strategies skip the registry.
        let load = if weighs_session_load(cfg) { self.registry.lock().account_load(cfg) } else { HashMap::new() };
        pool.pick(model, exclude, cfg, pinned, only, &load)
    }

    fn cutoff(idle: u64) -> i64 {
        // Account affinity has its own configured lifetime. Provider cache entries
        // may survive for 12 or 24 hours, so an inferred identity must not impose
        // an unrelated one-hour expiry on an otherwise reusable assignment.
        Utc::now().timestamp().saturating_sub(idle.clamp(1, i64::MAX as u64) as i64)
    }

    fn expired(registry: &Registry, b: &Binding, idle: u64) -> bool {
        b.last_seen <= Self::cutoff(idle) && !registry.active.contains_key(&b.session)
    }

    /// The request path's sweep: at most every few seconds.
    fn prune_due(registry: &mut Registry, idle: u64) {
        let now = Utc::now().timestamp();
        if now - registry.last_prune >= PRUNE_EVERY_SECONDS {
            registry.last_prune = now;
            Self::prune_locked(registry, idle);
        }
    }

    fn prune_locked(registry: &mut Registry, idle: u64) {
        let cutoff = Self::cutoff(idle);
        let count = registry.bindings.len();
        registry.bindings.retain(|_, b| b.last_seen > cutoff || registry.active.contains_key(&b.session));
        if count != registry.bindings.len() {
            registry.dirty = true;
            registry.invalidate_index();
        }
        let responses = registry.responses.len();
        registry.responses.retain(|_, c| c.last_seen > cutoff);
        if responses != registry.responses.len() {
            registry.history_bytes = registry.responses.values().map(|c| c.bytes).sum();
        }
    }

    pub fn flush(&self, idle: u64) {
        let mut registry = self.registry.lock();
        Self::prune_locked(&mut registry, idle);
        if Utc::now().timestamp() - registry.last_flush >= 30 {
            drop(registry);
            self.schedule_save();
        }
    }

    #[cfg(test)]
    pub fn save(&self) {
        if let Some(writer) = &self.persistence {
            let _ = writer.flush().blocking_recv();
        }
    }

    /// Wait for the serial writer without blocking an async request worker.
    pub async fn save_async(&self) {
        if let Some(writer) = &self.persistence {
            let _ = writer.flush().await;
        }
    }

    /// New assignments and migrations reach disk before the client sees output.
    /// Activity timestamps on an existing assignment use the periodic flush.
    pub async fn persist_selection(&self, selected: &Selected) {
        if let Some(durable) = self.begin_persist(selected) {
            let _ = durable.await;
        }
    }

    /// Starts writing a selection that must be durable and returns its completion,
    /// so the caller can contact the provider meanwhile and wait before replying.
    pub fn begin_persist(&self, selected: &Selected) -> Option<tokio::sync::oneshot::Receiver<()>> {
        let writer = self.persistence.as_ref()?;
        if matches!(
            selected.reason,
            "new_session" | "quota_exhausted" | "account_removed" | "account_disabled" | "model_unavailable"
        ) {
            Some(writer.flush())
        } else if matches!(selected.reason, "session_reused" | "temporary_detour") {
            writer.pending_write()
        } else {
            None
        }
    }

    fn schedule_save(&self) {
        if let Some(writer) = &self.persistence {
            writer.schedule();
        }
    }

    pub fn previous(&self, headers: &HeaderMap, id: &str, session: Option<&str>) -> Option<(String, History)> {
        let key = digest(&[&client_scope(headers), "response", id]);
        let (session, input) = {
            let mut registry = self.registry.lock();
            let previous = registry.responses.get_mut(&key)?;
            if session.is_some_and(|s| s != previous.session) {
                return None;
            }
            previous.last_seen = Utc::now().timestamp();
            (previous.session.clone(), previous.input.clone())
        };
        Some((session, input))
    }

    /// Takes the input by value: the caller's copy becomes the stored history.
    pub fn remember(&self, headers: &HeaderMap, session: &str, response: &Value, full_input: impl Into<History>) {
        let Some(id) = response["id"].as_str() else {
            return;
        };
        let Some(output) = response["output"].as_array() else {
            return;
        };
        let mut input = full_input.into();
        input.extend(output.clone());
        let bytes = input.bytes;
        if bytes > MAX_RESPONSE_BYTES {
            return;
        }
        let key = digest(&[&client_scope(headers), "response", id]);
        let mut registry = self.registry.lock();
        if let Some(old) = registry.responses.remove(&key) {
            registry.history_bytes -= old.bytes;
        }
        while registry.responses.len() >= MAX_RESPONSES || registry.history_bytes + bytes > MAX_HISTORY_BYTES {
            let Some(oldest) = registry.responses.iter().min_by_key(|(_, c)| c.last_seen).map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some(old) = registry.responses.remove(&oldest) {
                registry.history_bytes -= old.bytes;
            }
        }
        registry.history_bytes += bytes;
        registry
            .responses
            .insert(key, Conversation { session: session.into(), input, bytes, last_seen: Utc::now().timestamp() });
    }
}

/// Count the exact serialized array size, stopping at the entry limit before
/// copying the history or allocating a second, serialized conversation.
fn history_size(input: &[Value], output: &[Value], limit: usize) -> Option<usize> {
    struct History<'a>(&'a [Value], &'a [Value]);
    impl Serialize for History<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeSeq;
            let mut seq = serializer.serialize_seq(Some(self.0.len() + self.1.len()))?;
            for value in self.0.iter().chain(self.1) {
                seq.serialize_element(value)?;
            }
            seq.end()
        }
    }
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if buf.len() > self.limit.saturating_sub(self.bytes) {
                return Err(std::io::Error::other("history exceeds entry limit"));
            }
            self.bytes += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, &History(input, output)).ok()?;
    Some(counter.bytes)
}

/// Some Codex backends omit output from the final event after streaming it.
pub fn complete_output(response: &mut Value, aggregate: &crate::ir::Aggregate) {
    if response["output"].as_array().is_none_or(|items| items.is_empty()) {
        let rebuilt = crate::formats::responses::render_full(
            aggregate,
            response["model"].as_str().unwrap_or_default(),
            &crate::ir::Request::default(),
        );
        response["output"] = rebuilt["output"].clone();
    }
}

pub fn input_items(body: &Value) -> Vec<Value> {
    match &body["input"] {
        Value::String(s) => vec![
            serde_json::json!({ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": s }] }),
        ],
        Value::Array(items) => items.clone(),
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{KeyEntry, Routing};
    use chrono::Duration;
    use serde_json::json;

    fn config(routing: Routing) -> Config {
        Config {
            auth_dir: "/nonexistent".into(),
            routing,
            codex_api_key: ["a", "b"].map(|key| KeyEntry { api_key: key.into(), ..Default::default() }).to_vec(),
            ..Default::default()
        }
    }

    fn smart_pool() -> (Config, Pool) {
        let cfg = config(Routing::SmartQuota);
        let pool = Pool::default();
        pool.reload(&cfg);
        let now = Utc::now();
        for (a, days) in pool.all().iter().zip([3, 5]) {
            a.state.lock().quota = crate::quota::Quota {
                windows: vec![
                    crate::quota::Window {
                        name: "5h".into(),
                        used: 0.0,
                        resets_at: Some(now + Duration::hours(4)),
                        model: None,
                    },
                    crate::quota::Window {
                        name: "week".into(),
                        used: 30.0,
                        resets_at: Some(now + Duration::days(days)),
                        model: None,
                    },
                ],
                updated_at: Some(now),
                ..Default::default()
            };
        }
        (cfg, pool)
    }

    #[test]
    fn smart_reserve_protects_existing_sessions_and_never_blocks_available_quota() {
        let (cfg, pool) = smart_pool();
        let sessions = Sessions::memory();
        let pick = |task| sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some(task), &[], None).unwrap().0;
        let accounts = pool.all();
        let a = pick("first");
        assert_eq!(a.id, accounts[0].id);
        a.state.lock().quota.windows[0].used = 71.0; // 29% left
        let b = pick("second");
        assert_eq!(b.id, accounts[1].id);
        assert_eq!(pick("first").id, a.id); // reserve never evicts a pinned session
        b.state.lock().quota.windows[0].used = 90.0;
        assert_eq!(pick("third").id, a.id); // all below reserve: use the best available account
        a.state.lock().quota.windows[0].used = 100.0;
        assert_eq!(pick("first").id, b.id); // genuine exhaustion migrates
        a.state.lock().quota.windows[0].used = 0.0;
        assert_eq!(pick("first").id, b.id); // recovery does not disrupt the replacement
    }

    #[test]
    fn smart_reserve_admits_idle_accounts_and_releases_after_completion_or_inactivity() {
        for window in [0, 1] {
            let (cfg, pool) = smart_pool();
            let accounts = pool.all();
            accounts[0].state.lock().quota.windows[window].used = if window == 0 { 80.0 } else { 96.0 };
            let sessions = Arc::new(Sessions::memory());
            let pick = |task| sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some(task), &[], None).unwrap().0;
            assert_eq!(pick("first").id, accounts[0].id); // idle account can consume its reserve
            assert_eq!(pick("second").id, accounts[1].id); // protect the first session from new work
            assert_eq!(pick("first").id, accounts[0].id); // the existing session keeps its cache

            let lease = sessions.hold("first", cfg.session_affinity_idle_seconds);
            sessions
                .registry
                .lock()
                .bindings
                .values_mut()
                .filter(|b| b.account == accounts[0].id)
                .for_each(|b| b.last_seen -= LOAD_IDLE_SECONDS + 1);
            assert_eq!(pick("while-streaming").id, accounts[1].id); // long streams remain active
            drop(lease);
            sessions.end("first");
            assert_eq!(pick("after-completion").id, accounts[0].id); // explicit completion releases the reserve
            assert_eq!(pick("after-admission").id, accounts[1].id);

            sessions
                .registry
                .lock()
                .bindings
                .values_mut()
                .filter(|b| b.account == accounts[0].id)
                .for_each(|b| b.last_seen -= LOAD_IDLE_SECONDS + 1);
            assert_eq!(pick("after-inactivity").id, accounts[0].id); // old pins do not reserve quota
            assert_eq!(pick("after-completion").id, accounts[0].id); // the older affinity survives inactivity

            let unbound = Sessions::memory();
            let request = crate::accounts::RequestLoad::new(&accounts[0]);
            assert_eq!(unbound.pick(&pool, &cfg, "gpt-6.1-sol", None, &[], None).unwrap().0.id, accounts[1].id);
            drop(request);
            assert_eq!(unbound.pick(&pool, &cfg, "gpt-6.1-sol", None, &[], None).unwrap().0.id, accounts[0].id);
        }
    }

    #[test]
    fn smart_reserve_boundaries_unknown_quota_and_expired_windows() {
        let (mut cfg, pool) = smart_pool();
        let accounts = pool.all();
        let _request = crate::accounts::RequestLoad::new(&accounts[0]);
        let pick = |cfg: &Config| {
            Sessions::memory().pick(&pool, cfg, "gpt-6.1-sol", Some("new"), &[], None).unwrap().0.id.clone()
        };
        accounts[0].state.lock().quota.windows[0].used = 61.0;
        accounts[1].state.lock().quota.windows[0].used = 60.0;
        cfg.five_hour_reserve_percent = 40;
        assert_eq!(pick(&cfg), accounts[1].id);
        cfg.five_hour_reserve_percent = 0;
        assert_eq!(pick(&cfg), accounts[0].id);
        cfg.five_hour_reserve_percent = 30;
        accounts[0].state.lock().quota.windows[0].used = 70.0;
        accounts[1].state.lock().quota.windows[0].used = 70.0;
        assert_eq!(pick(&cfg), accounts[0].id); // exactly the reserve remains eligible
        accounts[0].state.lock().quota.windows.remove(0);
        assert_eq!(pick(&cfg), accounts[0].id); // no five-hour reserve is invented for missing data
        accounts[1].state.lock().quota.windows.remove(0);
        assert_eq!(pick(&cfg), accounts[0].id); // both unknown: renewal priority still works
        accounts[0].state.lock().quota.windows.push(crate::quota::Window {
            name: "5h".into(),
            used: 100.0,
            resets_at: Some(Utc::now() - Duration::seconds(1)),
            model: None,
        });
        assert_eq!(pick(&cfg), accounts[0].id); // elapsed window is available again
        cfg.five_hour_reserve_percent = 100;
        assert_eq!(pick(&cfg), accounts[0].id);
    }

    #[test]
    fn simultaneous_smart_assignments_pack_the_earliest_healthy_week() {
        let (cfg, pool) = smart_pool();
        let sessions = Sessions::memory();
        let assign = |sessions: &Sessions| {
            let barrier = std::sync::Barrier::new(24);
            std::thread::scope(|scope| {
                let handles: Vec<_> = (0..24)
                    .map(|i| {
                        let (cfg, pool, barrier) = (&cfg, &pool, &barrier);
                        scope.spawn(move || {
                            let task = format!("parallel-{i}");
                            barrier.wait();
                            let account = sessions.pick(pool, cfg, "gpt-6.1-sol", Some(&task), &[], None).unwrap().0;
                            (task, account.id.clone())
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect::<Vec<_>>()
            })
        };
        let assignments = assign(&sessions);
        let accounts = pool.all();
        let early = assignments.iter().filter(|(_, id)| *id == accounts[0].id).count();
        assert_eq!(early, assignments.len(), "session count must not outweigh the earlier reset");
        accounts[0].state.lock().quota.windows[1].used = 96.0;
        assert_eq!(
            sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some("after-draining"), &[], None).unwrap().0.id,
            accounts[1].id
        );
        for (task, id) in assignments {
            assert_eq!(sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some(&task), &[], None).unwrap().0.id, id);
        }
        let idle = Sessions::memory();
        let assignments = assign(&idle);
        assert_eq!(
            assignments.iter().filter(|(_, id)| *id == accounts[0].id).count(),
            1,
            "one new session may drain an idle reserve; later assignments must protect it"
        );
    }

    #[test]
    fn smart_routing_respects_model_specific_quota_through_an_alias() {
        let cfg = Config {
            routing: Routing::SmartQuota,
            auth_dir: "/nonexistent".into(),
            claude_api_key: ["a", "b"]
                .map(|key| KeyEntry {
                    api_key: key.into(),
                    models: vec![crate::config::ModelAlias {
                        name: "claude-opus-5-5".into(),
                        alias: Some("coding".into()),
                    }],
                    ..Default::default()
                })
                .to_vec(),
            ..Default::default()
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        let sessions = Sessions::memory();
        let pick = |task| sessions.pick(&pool, &cfg, "coding", Some(task), &[], None).unwrap().0;
        let a = pick("existing");
        a.state.lock().quota.windows.push(crate::quota::Window {
            name: "week opus".into(),
            used: 100.0,
            resets_at: Some(Utc::now() + Duration::days(1)),
            model: Some("opus".into()),
        });
        assert_ne!(pick("new").id, a.id);
        assert_ne!(pick("existing").id, a.id);
    }

    #[test]
    fn smart_session_load_deduplicates_scopes_and_releases_idle_or_ended_work() {
        let (mut cfg, pool) = smart_pool();
        let sessions = Arc::new(Sessions::memory());
        let a = sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some("task"), &[], None).unwrap().0;
        {
            let mut registry = sessions.registry.lock();
            let binding = registry.bindings.values().next().unwrap().clone();
            registry.bindings.insert("another-route-scope".into(), binding);
            assert_eq!(registry.account_load(&cfg)[&a.id], 1);
            registry.bindings.values_mut().for_each(|b| b.last_seen -= LOAD_IDLE_SECONDS + 1);
            assert!(registry.account_load(&cfg).is_empty());
        }
        let lease = sessions.hold("task", cfg.session_affinity_idle_seconds);
        assert_eq!(sessions.registry.lock().account_load(&cfg)[&a.id], 1); // long-running stream
        cfg.session_affinity = false;
        assert!(sessions.registry.lock().account_load(&cfg).is_empty());
        cfg.session_affinity = true;
        drop(lease);
        assert_eq!(sessions.registry.lock().account_load(&cfg)[&a.id], 1); // recent after completion
        sessions.end("task");
        assert!(sessions.registry.lock().account_load(&cfg).is_empty());
    }

    #[test]
    fn live_and_ongoing_sessions_count_for_every_strategy_until_idle() {
        let cfg = config(Routing::RoundRobin);
        let pool = Pool::default();
        pool.reload(&cfg);
        let sessions = Arc::new(Sessions::memory());
        let a = sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some("task"), &[], None).unwrap().0;
        let idle = cfg.session_affinity_idle_seconds;
        assert!(sessions.account_load(&cfg).is_empty()); // routing does not weigh it
        assert_eq!(sessions.session_counts(idle)[&a.id], (1, 1));
        let age = |seconds| sessions.registry.lock().bindings.values_mut().for_each(|b| b.last_seen -= seconds);
        age(LOAD_IDLE_SECONDS + 1);
        // Quiet past the load window: no longer live, still going on.
        assert_eq!(sessions.session_counts(idle)[&a.id], (0, 1));
        let lease = sessions.hold("task", idle);
        assert_eq!(sessions.session_counts(idle)[&a.id], (1, 1)); // long-running stream
        drop(lease);
        age(ONGOING_IDLE_SECONDS);
        assert!(sessions.session_counts(idle).is_empty());
        // A shorter affinity expiry bounds both windows.
        let b = sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some("other"), &[], None).unwrap().0;
        sessions.registry.lock().bindings.values_mut().for_each(|b| b.last_seen -= 120);
        assert_eq!(sessions.session_counts(60).get(&b.id), None);
        assert_eq!(sessions.session_counts(idle)[&b.id], (1, 1));
        sessions.end("task");
    }

    #[test]
    fn all_strategies_pin_and_keep_the_replacement_after_recovery() {
        for routing in [Routing::LeastUsed, Routing::SmartQuota, Routing::RoundRobin, Routing::FillFirst] {
            let cfg = config(routing);
            let pool = Pool::default();
            pool.reload(&cfg);
            let sessions = Sessions::memory();
            let pick = || sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some("task"), &[], None).unwrap().0;
            let a = pick();
            for _ in 0..5 {
                assert_eq!(pick().id, a.id);
            }
            // A model change inside the same provider also preserves the subscription.
            assert_eq!(sessions.pick(&pool, &cfg, "gpt-6-astra", Some("task"), &[], None).unwrap().0.id, a.id);
            a.exhaust("gpt-6.1-sol", Utc::now() + Duration::minutes(5), "quota exhausted");
            let b = pick();
            assert_ne!(a.id, b.id);
            a.state.lock().quota_cooldowns.clear();
            a.state.lock().cooldowns.clear();
            assert_eq!(pick().id, b.id);
        }
    }

    #[test]
    fn routing_metadata_distinguishes_assignment_reuse_and_quota_migration() {
        for routing in [Routing::LeastUsed, Routing::SmartQuota, Routing::RoundRobin, Routing::FillFirst] {
            let cfg = config(routing);
            let pool = Pool::default();
            pool.reload(&cfg);
            let sessions = Sessions::memory();
            let pick = || sessions.pick_with_reason(&pool, &cfg, "gpt-6.1-sol", Some("task"), &[], None).unwrap();
            let first = pick();
            assert_eq!(first.reason, "new_session");
            assert_eq!(first.strategy, routing);
            assert_eq!(first.model, "gpt-6.1-sol");
            assert!(first.previous_account.is_none());

            let reused = pick();
            assert_eq!(reused.reason, "session_reused");
            assert_eq!(reused.account.id, first.account.id);
            assert!(reused.previous_account.is_none());

            first.account.exhaust("gpt-6.1-sol", Utc::now() + Duration::minutes(5), "quota exhausted");
            let migrated = pick();
            assert_eq!(migrated.reason, "quota_exhausted");
            assert_eq!(migrated.strategy, routing);
            assert_eq!(migrated.previous_account.as_deref(), Some(first.account.id.as_str()));
            assert_ne!(migrated.account.id, first.account.id);

            let next = pick();
            assert_eq!(next.reason, "session_reused");
            assert_eq!(next.account.id, migrated.account.id);
            assert!(next.previous_account.is_none());
        }
    }

    #[test]
    fn routing_metadata_explains_why_per_request_selection_is_used() {
        for (enabled, session, reason) in [
            (true, None, "missing_session"),
            (false, None, "affinity_disabled"),
            (false, Some("task"), "affinity_disabled"),
        ] {
            let cfg = Config { session_affinity: enabled, ..config(Routing::RoundRobin) };
            let pool = Pool::default();
            pool.reload(&cfg);
            let sessions = Sessions::memory();
            let pick = || sessions.pick_with_reason(&pool, &cfg, "gpt-6.1-sol", session, &[], None).unwrap();
            let first = pick();
            let second = pick();
            assert_eq!(first.reason, reason);
            assert_eq!(second.reason, reason);
            assert_eq!(first.strategy, Routing::RoundRobin);
            assert_ne!(first.account.id, second.account.id);
            assert!(first.previous_account.is_none());
            assert!(second.previous_account.is_none());
            assert!(sessions.registry.lock().bindings.is_empty());
        }
    }

    #[test]
    fn a_session_whose_only_account_is_paused_waits_for_it() {
        let cfg = Config {
            codex_api_key: vec![KeyEntry { api_key: "only".into(), ..Default::default() }],
            ..config(Routing::RoundRobin)
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        let sessions = Sessions::memory();
        let pick = |session: &str, exclude: &[String]| {
            sessions.pick_with_reason(&pool, &cfg, "gpt-6.1-sol", Some(session), exclude, None)
        };
        let wait = |result: Result<Selected, (u16, String)>| match result {
            Err((429, message)) => message.rsplit(' ').next().unwrap().trim_end_matches('s').parse::<i64>().unwrap(),
            Err(other) => panic!("expected a wait, got {other:?}"),
            Ok(selected) => panic!("expected a wait, got {}", selected.account.id),
        };
        let a = pick("task", &[]).unwrap().account;
        // A rate limit: the session hears when its account is back, as a new session does,
        // not that no account serves the model.
        a.rate_limit("gpt-6.1-sol", Utc::now() + Duration::seconds(120), "429: slow down");
        assert!((100..=120).contains(&wait(pick("task", &[]))));
        assert!((100..=120).contains(&wait(pick("new-task", &[]))));
        // An account this request already tried answers with its own upstream error instead.
        assert!(matches!(pick("task", std::slice::from_ref(&a.id)), Err((404, _))));
        a.state.lock().cooldowns.clear();
        let back = pick("task", &[]).unwrap();
        assert_eq!((back.account.id.as_str(), back.reason), (a.id.as_str(), "session_reused"));
        // A spent quota waits for its reset and keeps the assignment too.
        a.exhaust("gpt-6.1-sol", Utc::now() + Duration::hours(3), "spent");
        assert!((10_700..=10_800).contains(&wait(pick("task", &[]))));
        a.state.lock().quota_cooldowns.clear();
        assert_eq!(pick("task", &[]).unwrap().reason, "session_reused");

        // With other accounts paused too, the earliest return counts.
        let cfg = config(Routing::RoundRobin);
        let pool = Pool::default();
        pool.reload(&cfg);
        let sessions = Sessions::memory();
        let assigned = sessions.pick_with_reason(&pool, &cfg, "gpt-6.1-sol", Some("task"), &[], None).unwrap().account;
        let other = pool.all().into_iter().find(|a| a.id != assigned.id).unwrap();
        assigned.rate_limit("gpt-6.1-sol", Utc::now() + Duration::seconds(60), "429: slow down");
        other.rate_limit("gpt-6.1-sol", Utc::now() + Duration::seconds(300), "429: slow down");
        assert!((40..=60).contains(&wait(sessions.pick_with_reason(
            &pool,
            &cfg,
            "gpt-6.1-sol",
            Some("task"),
            &[],
            None
        ))));
    }

    #[test]
    fn temporary_cooldowns_detour_and_disabled_accounts_move_the_session() {
        let cfg = config(Routing::RoundRobin);
        let pool = Pool::default();
        pool.reload(&cfg);
        let sessions = Sessions::memory();
        let pick = || sessions.pick_with_reason(&pool, &cfg, "gpt-6.1-sol", Some("task"), &[], None).unwrap();
        let a = pick().account;
        // A rate limit serves the request elsewhere but keeps the assignment.
        a.cool(None, Utc::now() + Duration::seconds(60), "temporary");
        let detour = pick();
        assert_eq!(detour.reason, "temporary_detour");
        assert_ne!(detour.account.id, a.id);
        assert_eq!(detour.previous_account.as_deref(), Some(a.id.as_str()));
        a.state.lock().cooldowns.clear();
        assert_eq!(pick().account.id, a.id);
        // So does a failed attempt within one request.
        let retry = sessions
            .pick_with_reason(&pool, &cfg, "gpt-6.1-sol", Some("task"), std::slice::from_ref(&a.id), None)
            .unwrap();
        assert_eq!(retry.reason, "temporary_detour");
        assert_eq!(pick().account.id, a.id);
        // Disabling the account moves the session for good.
        a.state.lock().disabled = true;
        let moved = pick();
        assert_eq!(moved.reason, "account_disabled");
        assert_ne!(moved.account.id, a.id);
        a.state.lock().disabled = false;
        assert_eq!(pick().account.id, moved.account.id);
    }

    #[test]
    fn inferred_sessions_follow_the_first_user_message_across_turns_and_formats() {
        let client = |key: &str| {
            let mut headers = HeaderMap::new();
            headers.insert("authorization", format!("Bearer {key}").parse().unwrap());
            headers
        };
        let headers = client("first");
        let key = |format, body: Value| inferred_identity(&headers, format, &body).map(|i| i.key);
        // Responses: a string, a text block and a later turn all start the same way.
        let first = key(Format::Responses, json!({"input":"question"})).unwrap();
        assert_eq!(
            key(
                Format::Responses,
                json!({"input":[{"role":"user","content":[{"type":"input_text","text":"question"}]}]})
            )
            .unwrap(),
            first
        );
        let later = json!({"instructions":"changed","input":[
            {"role":"developer","content":"context"},
            {"role":"user","content":"question"},
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]},
            {"type":"function_call_output","call_id":"c","output":"ok"},
            {"role":"user","content":"next"}
        ]});
        assert_eq!(key(Format::Responses, later).unwrap(), first);
        assert_ne!(key(Format::Responses, json!({"input":"another question"})).unwrap(), first);
        // Claude: the moving cache marker on the latest turn does not change the key.
        let marked = json!({"messages":[{"role":"user","content":[
            {"type":"text","text":"task","cache_control":{"type":"ephemeral"}},
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAA"},"cache_control":{"type":"ephemeral"}}
        ]}]});
        let claude = key(Format::Claude, marked).unwrap();
        let next = json!({"system":"s","messages":[
            {"role":"user","content":[{"type":"text","text":"task"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAA"}}]},
            {"role":"assistant","content":"done"},
            {"role":"user","content":[{"type":"text","text":"more","cache_control":{"type":"ephemeral"}}]}
        ]});
        assert_eq!(key(Format::Claude, next).unwrap(), claude);
        let other_image = json!({"messages":[{"role":"user","content":[
            {"type":"text","text":"task"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"BBB"}}
        ]}]});
        assert_ne!(key(Format::Claude, other_image).unwrap(), claude);
        // Chat and Gemini.
        let chat =
            key(Format::Chat, json!({"messages":[{"role":"system","content":"s"},{"role":"user","content":"hello"}]}));
        assert_eq!(
            chat,
            key(
                Format::Chat,
                json!({"messages":[{"role":"user","content":[{"type":"text","text":"hello"}]},{"role":"assistant","content":"hi"}]})
            )
        );
        let gemini = key(Format::Gemini, json!({"contents":[{"parts":[{"text":"hello"}]}]}));
        assert_eq!(
            gemini,
            key(
                Format::Gemini,
                json!({"contents":[{"role":"user","parts":[{"text":"hello"}]},{"role":"model","parts":[{"text":"hi"}]}]})
            )
        );
        // Every client key has its own sessions, and nothing to go on means no session.
        let second = inferred_identity(&client("second"), Format::Responses, &json!({"input":"question"}));
        assert_ne!(second.unwrap().key, first);
        for (format, body) in [
            (Format::Responses, json!({"input":""})),
            (Format::Responses, json!({"input":[{"type":"function_call_output","call_id":"c","output":"ok"}]})),
            (Format::Claude, json!({"messages":[{"role":"user","content":[]}]})),
            (Format::Chat, json!({"messages":[{"role":"system","content":"only instructions"}]})),
            (Format::Gemini, json!({})),
        ] {
            assert!(key(format, body).is_none());
        }
        let identity = inferred_identity(&headers, Format::Responses, &json!({"input":"question"})).unwrap();
        assert_eq!(identity.source, INFERRED_SOURCE);
        // Never the same as an explicit session with the same text.
        assert_ne!(Some(identity.key), session_key(&headers, &json!({"prompt_cache_key":"question"})));
    }

    #[test]
    fn prompt_cache_keys_are_stable_uuid_shaped_and_not_the_session_key() {
        let key = prompt_cache_key("session");
        assert_eq!(key, prompt_cache_key("session"));
        assert_ne!(key, prompt_cache_key("other"));
        assert_eq!(key.split('-').map(str::len).collect::<Vec<_>>(), [8, 4, 4, 4, 12]);
        assert!(key.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
        assert!(!digest(&["owner", "session"]).contains(&key.replace('-', "")));
    }

    #[test]
    fn inferred_assignments_follow_configured_idle_lifetime_and_are_evicted_first() {
        let cfg = config(Routing::RoundRobin);
        let pool = Pool::default();
        pool.reload(&cfg);
        let sessions = Sessions::memory();
        let pick = |session, inferred| {
            sessions.pick_session(&pool, &cfg, "gpt-6.1-sol", Some(session), inferred, &[], None).unwrap()
        };
        let guessed = pick("guessed", true);
        pick("explicit", false);
        assert_eq!(pick("guessed", true).account.id, guessed.account.id);
        assert_eq!(pick("guessed", true).reason, "session_reused");
        {
            let registry = sessions.registry.lock();
            assert_eq!(registry.bindings.values().filter(|b| b.inferred).count(), 1);
            let stored = serde_json::to_string(&registry.bindings).unwrap();
            assert_eq!(stored.matches("\"inferred\":true").count(), 1);
            assert!(!stored.contains("\"inferred\":false"), "explicit assignments keep their old format");
        }
        // A long gap may still reuse an extended cache entry. Inferred account
        // assignments follow the same configured one-day lifetime as explicit ids.
        sessions.registry.lock().bindings.values_mut().for_each(|b| b.last_seen -= 2 * 3600);
        sessions.prune(cfg.session_affinity_idle_seconds);
        let registry = sessions.registry.lock();
        assert_eq!(registry.bindings.len(), 2);
        drop(registry);
        assert_eq!(pick("guessed", true).account.id, guessed.account.id);
        assert_eq!(pick("guessed", true).reason, "session_reused");

        // A shorter operator-selected timeout still applies to both identity types.
        sessions.registry.lock().bindings.values_mut().for_each(|b| b.last_seen -= 2 * 3600);
        sessions.prune(3600);
        assert!(sessions.registry.lock().bindings.is_empty());
        assert_eq!(pick("guessed", true).reason, "new_session");

        // A full table gives up an inferred assignment before an older explicit one.
        let sessions = Sessions::memory();
        {
            let mut registry = sessions.registry.lock();
            let now = Utc::now().timestamp();
            for i in 0..MAX_SESSIONS {
                registry.bindings.insert(
                    format!("old-{i}"),
                    Binding {
                        session: format!("owner-{i}"),
                        account: "gone".into(),
                        last_seen: now - 600 + i as i64,
                        inferred: i == MAX_SESSIONS - 1,
                    },
                );
            }
        }
        sessions.pick_session(&pool, &cfg, "gpt-6.1-sol", Some("new"), false, &[], None).unwrap();
        let registry = sessions.registry.lock();
        assert!(registry.bindings.contains_key("old-0"));
        assert!(!registry.bindings.contains_key(&format!("old-{}", MAX_SESSIONS - 1)));
        assert!(registry.bindings.values().all(|b| !b.inferred));
        // Assignments written before this change still load.
        let old: HashMap<String, Binding> =
            serde_json::from_str(r#"{"k":{"session":"s","account":"a","last_seen":1}}"#).unwrap();
        assert!(!old["k"].inferred);
    }

    #[test]
    fn a_full_table_forgets_the_longest_idle_session_instead_of_failing() {
        let cfg = config(Routing::RoundRobin);
        let pool = Pool::default();
        pool.reload(&cfg);
        let sessions = Sessions::memory();
        {
            let mut registry = sessions.registry.lock();
            let now = Utc::now().timestamp();
            for i in 0..MAX_SESSIONS {
                registry.bindings.insert(
                    format!("old-{i}"),
                    Binding {
                        session: format!("owner-{i}"),
                        account: "gone".into(),
                        last_seen: now - 20_000 + i as i64,
                        inferred: false,
                    },
                );
            }
        }
        let selected = sessions.pick_with_reason(&pool, &cfg, "gpt-6.1-sol", Some("new-task"), &[], None).unwrap();
        assert_eq!(selected.reason, "new_session");
        let registry = sessions.registry.lock();
        assert_eq!(registry.bindings.len(), MAX_SESSIONS);
        assert!(!registry.bindings.contains_key("old-0"));
    }

    #[test]
    fn identifiers_are_client_scoped_and_claude_metadata_is_read() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer first".parse().unwrap());
        let body = json!({"metadata":{"user_id":json!({"session_id":"coding-task"}).to_string()}});
        let a = session_key(&headers, &body).unwrap();
        headers.insert("thread-id", "coding-task".parse().unwrap());
        assert_eq!(session_key(&headers, &json!({})).unwrap(), a);
        headers.insert("authorization", "Bearer second".parse().unwrap());
        assert_ne!(session_key(&headers, &body).unwrap(), a);
        assert!(session_key(&HeaderMap::new(), &json!({"metadata":{"user_id":"ordinary-user"}})).is_none());
        assert_eq!(
            session_key(&HeaderMap::new(), &json!({"metadata":{"user_id":"user_x_account_y_session_coding-task"}})),
            session_key(&HeaderMap::new(), &json!({"prompt_cache_key":"coding-task"}))
        );
    }

    #[test]
    fn identity_sources_preserve_existing_hashes_and_precedence() {
        let headers = HeaderMap::new();
        let expected = digest(&[&client_scope(&headers), "session", "task"]);
        for (source, body) in [
            ("metadata.user_id.session_id", json!({"metadata":{"user_id":r#"{"session_id":"task"}"#}})),
            ("metadata.user_id", json!({"metadata":{"user_id":"user_u_account_a_session_task"}})),
            ("metadata.session_id", json!({"metadata":{"session_id":"task"}})),
            ("metadata.thread_id", json!({"metadata":{"thread_id":"task"}})),
            ("conversation.id", json!({"conversation":{"id":"task"}})),
            ("conversation", json!({"conversation":"task"})),
            ("prompt_cache_key", json!({"prompt_cache_key":"task"})),
        ] {
            let identity = session_identity(&headers, &body).unwrap();
            assert_eq!(identity.source, source);
            assert_eq!(identity.key, expected);
            assert_eq!(session_key(&headers, &body).as_deref(), Some(expected.as_str()));
        }
        let mut headers = HeaderMap::new();
        for name in [
            "x-claude-code-session-id",
            "session-id",
            "session_id",
            "x-codex-thread-id",
            "thread-id",
            "x-cliproxy-session-id",
            "x-fusebox-session-id",
        ] {
            headers.insert(name, "task".parse().unwrap());
            let identity = session_identity(&headers, &json!({"prompt_cache_key":"lower-priority"})).unwrap();
            assert_eq!(identity.source, name);
            assert_eq!(identity.key, expected);
        }
        let invalid = "x".repeat(1025);
        let mut headers = HeaderMap::new();
        headers.insert("thread-id", invalid.parse().unwrap());
        let body = json!({"metadata":{"session_id":invalid},"prompt_cache_key":"task"});
        let identity = session_identity(&headers, &body).unwrap();
        assert_eq!(identity.source, "prompt_cache_key");
        assert_eq!(identity.key, expected);
        assert!(session_identity(&HeaderMap::new(), &json!({})).is_none());
    }

    #[test]
    fn round_robin_distributes_sessions_and_concurrent_calls_share_assignment() {
        let cfg = config(Routing::RoundRobin);
        let pool = Pool::default();
        pool.reload(&cfg);
        let sessions = Sessions::memory();
        let a = sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some("one"), &[], None).unwrap().0;
        let b = sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some("two"), &[], None).unwrap().0;
        assert_ne!(a.id, b.id);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..12)
                .map(|_| {
                    scope.spawn(|| {
                        sessions.pick_with_reason(&pool, &cfg, "gpt-6.1-sol", Some("parallel"), &[], None).unwrap()
                    })
                })
                .collect();
            let selected: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            assert!(selected.iter().all(|s| s.account.id == selected[0].account.id));
            assert_eq!(selected.iter().filter(|s| s.reason == "new_session").count(), 1);
            assert_eq!(selected.iter().filter(|s| s.reason == "session_reused").count(), selected.len() - 1);
        });
    }

    #[test]
    fn history_cannot_cross_client_or_session_boundaries() {
        let sessions = Sessions::memory();
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer first".parse().unwrap());
        sessions.remember(
            &headers,
            "task",
            &json!({"id":"r1", "output":[{"text":"answer"}]}),
            vec![json!({"text":"question"})],
        );
        assert_eq!(sessions.previous(&headers, "r1", Some("task")).unwrap().1.len(), 2);
        assert!(sessions.previous(&headers, "r1", Some("another-task")).is_none());
        headers.insert("authorization", "Bearer second".parse().unwrap());
        assert!(sessions.previous(&headers, "r1", None).is_none());
    }

    #[test]
    fn history_limits_count_escaped_json_exactly_without_retaining_oversized_entries() {
        let input = [json!({"text":"quotes: \" and control: \n and unicode: 🦀"})];
        let output = [json!({"type":"message", "content":[]})];
        let combined: Vec<_> = input.iter().chain(&output).cloned().collect();
        let size = serde_json::to_vec(&combined).unwrap().len();
        assert_eq!(history_size(&input, &output, size), Some(size));
        assert_eq!(history_size(&input, &output, size - 1), None);
        assert_eq!(history_size(&[], &[], 2), Some(2));
        let sessions = Sessions::memory();
        let large = vec![json!("x".repeat(MAX_RESPONSE_BYTES))];
        sessions.remember(&HeaderMap::new(), "session", &json!({"id":"large", "output":[]}), large);
        assert!(sessions.previous(&HeaderMap::new(), "large", None).is_none());
        assert_eq!(sessions.registry.lock().history_bytes, 0);
    }

    #[test]
    fn assignments_survive_restart_and_idle_sessions_expire() {
        let dir = std::env::temp_dir().join(format!("fusebox-affinity-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = config(Routing::RoundRobin);
        let pool = Pool::default();
        pool.reload(&cfg);
        let sessions = Sessions::load(&dir, 86_400);
        let a = sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some("sensitive-task-id"), &[], None).unwrap().0;
        sessions.save();
        let loaded = Sessions::load(&dir, 86_400);
        assert_eq!(loaded.pick(&pool, &cfg, "gpt-6.1-sol", Some("sensitive-task-id"), &[], None).unwrap().0.id, a.id);
        let data = std::fs::read_to_string(dir.join(".routing-sessions.state")).unwrap();
        assert!(!data.contains("sensitive-task-id"));
        loaded.registry.lock().bindings.values_mut().for_each(|b| b.last_seen -= 90_000);
        loaded.prune(86_400);
        assert!(loaded.registry.lock().bindings.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn active_calls_prevent_idle_expiry_and_explicit_completion_releases_assignments() {
        let cfg = config(Routing::RoundRobin);
        let pool = Pool::default();
        pool.reload(&cfg);
        let sessions = Arc::new(Sessions::memory());
        let lease = sessions.hold("task", 86_400);
        let first = sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some("task"), &[], None).unwrap().0;
        sessions.registry.lock().bindings.values_mut().for_each(|b| b.last_seen -= 90_000);
        sessions.prune(86_400);
        assert_eq!(sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some("task"), &[], None).unwrap().0.id, first.id);
        drop(lease);
        sessions.end("task");
        assert!(sessions.registry.lock().bindings.is_empty());
        assert_ne!(sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some("task"), &[], None).unwrap().0.id, first.id);
    }

    #[test]
    fn disabling_affinity_restores_per_request_routing_and_nested_config_works() {
        let cfg = Config::parse("routing:\n  strategy: round-robin\n  session-affinity: false\n").unwrap();
        assert!(!cfg.session_affinity);
        assert!(!cfg.ignored.iter().any(|s| s.contains("session affinity")));
        let cfg = Config { session_affinity: false, ..config(Routing::RoundRobin) };
        let pool = Pool::default();
        pool.reload(&cfg);
        let sessions = Sessions::memory();
        let first = sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some("task"), &[], None).unwrap().0;
        let second = sessions.pick(&pool, &cfg, "gpt-6.1-sol", Some("task"), &[], None).unwrap().0;
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn exhaustion_from_usage_windows_and_provider_aliases_preserves_the_new_account() {
        let mut cfg = config(Routing::LeastUsed);
        for entry in &mut cfg.codex_api_key {
            entry.models = vec![
                crate::config::ModelAlias { name: "gpt-6.1-sol".into(), alias: Some("coding".into()) },
                crate::config::ModelAlias { name: "gpt-6-astra".into(), alias: None },
            ];
        }
        let pool = Pool::default();
        pool.reload(&cfg);
        let sessions = Sessions::memory();
        let a = sessions.pick(&pool, &cfg, "coding", Some("task"), &[], None).unwrap().0;
        assert_eq!(sessions.pick(&pool, &cfg, "gpt-6-astra", Some("task"), &[], None).unwrap().0.id, a.id);
        a.state.lock().quota = crate::quota::Quota {
            updated_at: Some(Utc::now()),
            windows: vec![crate::quota::Window { name: "5h".into(), used: 100.0, resets_at: None, model: None }],
            ..Default::default()
        };
        let b = sessions.pick(&pool, &cfg, "coding", Some("task"), &[], None).unwrap().0;
        assert_ne!(a.id, b.id);
        a.state.lock().quota = Default::default();
        assert_eq!(sessions.pick(&pool, &cfg, "coding", Some("task"), &[], None).unwrap().0.id, b.id);
    }
}

#[cfg(test)]
mod optimization_measurements {
    use super::*;
    /// Synthetic data-structure measurement, not provider latency or RSS.
    #[test]
    #[ignore]
    fn shared_history_measurement() {
        use std::{hint::black_box, time::Instant};
        for size in [10 << 10, 200 << 10, 1 << 20] {
            let seed = vec![serde_json::json!({"role":"user","content":"x".repeat(size)})];
            let delta = serde_json::json!({"role":"assistant","content":"y".repeat(128)});
            let mut original = seed.clone();
            let start = Instant::now();
            for _ in 0..64 {
                let mut next = black_box(original.clone());
                next.push(delta.clone());
                black_box(history_size(&next, &[], MAX_RESPONSE_BYTES));
                original = black_box(next.clone());
            }
            let old_us = start.elapsed().as_micros();
            let mut shared = History::from(seed);
            let start = Instant::now();
            for _ in 0..64 {
                let mut next = black_box(shared.clone());
                next.extend(vec![delta.clone()]);
                shared = black_box(next.clone());
            }
            let shared_us = start.elapsed().as_micros();
            assert_eq!(original, shared.to_values());
            println!(
                "{{\"seed_bytes\":{size},\"turns\":64,\"deep_copy_and_full_measure_us\":{old_us},\"shared_append_us\":{shared_us}}}"
            );
        }
    }
}
