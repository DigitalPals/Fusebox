//! Accounts: OAuth credential files + API keys from config, with selection,
//! cooldowns and usage counters.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::config::{Config, ModelAlias, OAuthAlias, Routing};
use crate::ir::Format;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
    Gemini,
    Vertex,
    Antigravity,
    Kimi,
    Xai,
    Meta,
    Devin,
    #[serde(rename = "openai-compat")]
    Compat,
}

pub const PROVIDERS: [Provider; 10] = [
    Provider::Claude,
    Provider::Codex,
    Provider::Gemini,
    Provider::Vertex,
    Provider::Antigravity,
    Provider::Kimi,
    Provider::Xai,
    Provider::Meta,
    Provider::Devin,
    Provider::Compat,
];

const ANTIGRAVITY_MODELS: &[&str] = &[
    "claude-opus-5-5-max",
    "claude-opus-5-5-high",
    "claude-opus-5-5-medium",
    "claude-opus-5-5-low",
    "claude-sonnet-5-5-max",
    "claude-sonnet-5-5-high",
    "claude-sonnet-5-5-medium",
    "claude-sonnet-5-5-low",
    "claude-opus-4-6-thinking",
    "claude-sonnet-4-6",
    "gemini-4-argon-high",
    "gemini-4-argon-medium",
    "gemini-4-argon-low",
    "gemini-3.8-flash-high",
    "gemini-3.7-flash-high",
    "gemini-3.6-flash-high",
    "gemini-pro-agent",
    "gemini-3.1-pro-low",
    "gemini-3-flash",
    "gemini-3.1-flash-lite",
    "gemini-3.5-flash-lite",
    "gemini-3.1-flash-image",
    "gpt-oss-120b-medium",
];

const DEVIN_MODELS: &[&str] = &[
    "claude-opus-5-5",
    "claude-fable-5-1",
    "claude-sonnet-5-5",
    "claude-haiku-5-5",
    "claude-sonnet-5",
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-sonnet-4-6",
    "gpt-6-astra",
    "gpt-6-1-sol",
    "gpt-6-sol",
    "gpt-6-luna",
    "gpt-5-6-sol",
    "gpt-5-6-terra",
    "gpt-5-6-luna",
    "gemini-3-8-flash",
    "gemini-3-7-flash",
    "gemini-3-1-pro",
    "grok-4-7",
    "grok-4-6",
    "kimi-k3",
    "glm-5-3",
    "glm-5-3-flash",
    "deepseek-v4-pro",
    "deepseek-v4-1-flash",
    "swe-2",
    "swe-1-7",
    "swe-1-7-lightning",
];

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
            Provider::Gemini => "gemini",
            Provider::Vertex => "vertex",
            Provider::Antigravity => "antigravity",
            Provider::Kimi => "kimi",
            Provider::Xai => "xai",
            Provider::Meta => "meta",
            Provider::Devin => "devin",
            Provider::Compat => "openai-compat",
        }
    }

    /// Accepts the canonical name and the aliases people actually type.
    pub fn parse(s: &str) -> Option<Provider> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "claude" | "anthropic" => Provider::Claude,
            "codex" | "openai" | "chatgpt" => Provider::Codex,
            "gemini" | "aistudio" | "google" => Provider::Gemini,
            "vertex" | "vertex-ai" => Provider::Vertex,
            "antigravity" => Provider::Antigravity,
            "kimi" | "moonshot" => Provider::Kimi,
            "xai" | "grok" => Provider::Xai,
            "meta" | "muse" => Provider::Meta,
            "devin" | "windsurf" | "codeium" => Provider::Devin,
            "compat" | "openai-compat" | "openai-compatibility" => Provider::Compat,
            _ => return None,
        })
    }

    /// Wire formats the upstream accepts, preferred one first. A client whose
    /// format is in this list is passed through; everything else is translated
    /// to the first entry.
    pub fn wires(self) -> &'static [Format] {
        match self {
            Provider::Claude => &[Format::Claude],
            Provider::Codex | Provider::Xai | Provider::Meta => &[Format::Responses],
            Provider::Gemini | Provider::Vertex | Provider::Antigravity => &[Format::Gemini],
            Provider::Kimi => &[Format::Chat, Format::Claude, Format::Responses],
            // Devin speaks its own protobuf dialect, built from the IR.
            Provider::Devin => &[],
            Provider::Compat => &[Format::Chat],
        }
    }

    /// Models this provider serves when routing automatically. Names only an
    /// aggregator knows (`gemini-3.8-flash-high`, `gpt-5-6-sol`) stay with it.
    pub fn serves(self, model: &str) -> bool {
        let aggregator_only = || {
            let known = |list: &[&str]| list.iter().any(|m| m.eq_ignore_ascii_case(model));
            (known(ANTIGRAVITY_MODELS) || known(DEVIN_MODELS)) && !known(self.builtin_models())
        };
        self.family(model) && (matches!(self, Provider::Antigravity | Provider::Devin) || !aggregator_only())
    }

    /// Model-name families this provider can serve (used for explicit `provider/model`).
    pub fn family(self, model: &str) -> bool {
        let m = model.to_ascii_lowercase();
        match self {
            Provider::Claude => m.starts_with("claude-"),
            Provider::Codex => {
                m.starts_with("gpt-")
                    || m.starts_with("codex-")
                    || (m.len() >= 2 && m.starts_with('o') && m.as_bytes()[1].is_ascii_digit())
            }
            Provider::Gemini => m.starts_with("gemini-") || m.starts_with("gemma-"),
            Provider::Vertex => m.starts_with("gemini-") || m.starts_with("imagen-"),
            Provider::Kimi => m.starts_with("kimi-") || m.starts_with("moonshot-"),
            Provider::Xai => m.starts_with("grok-"),
            Provider::Meta => m.starts_with("muse-"),
            Provider::Antigravity | Provider::Devin => {
                self.builtin_models().iter().any(|b| b.eq_ignore_ascii_case(model))
            }
            Provider::Compat => false,
        }
    }

    /// Aggregators (Antigravity, Devin) also serve other vendors' models;
    /// requests prefer the vendor's own provider and overflow to them.
    pub fn first_party(self, model: &str) -> bool {
        !matches!(self, Provider::Antigravity | Provider::Devin)
            || !PROVIDERS
                .iter()
                .any(|p| !matches!(p, Provider::Antigravity | Provider::Devin | Provider::Compat) && p.serves(model))
    }

    pub fn builtin_models(self) -> &'static [&'static str] {
        match self {
            Provider::Claude => &[
                "claude-fable-5-1",
                "claude-opus-5-5",
                "claude-sonnet-5-5",
                "claude-haiku-5-5",
                "claude-opus-5",
                "claude-sonnet-5",
                "claude-opus-4-8",
                "claude-opus-4-7",
                "claude-opus-4-6",
                "claude-sonnet-4-6",
                "claude-haiku-4-5-20251001",
            ],
            Provider::Codex => &[
                "gpt-6-astra",
                "gpt-6.1-sol",
                "gpt-6.1-terra",
                "gpt-6-sol",
                "gpt-6-luna",
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gpt-5.5",
                "gpt-image-2",
                "gpt-image-2.5-flare",
                "gpt-image-2.5-sunburst",
            ],
            Provider::Gemini => &[
                "gemini-4-argon",
                "gemini-3.8-flash",
                "gemini-3.7-flash",
                "gemini-3.6-flash",
                "gemini-3.1-pro-preview",
                "gemini-3.5-flash-lite",
                "gemini-3.1-flash-image-preview",
                "gemini-3.1-flash-image",
                "gemini-3.1-flash-lite-image",
                "gemini-nano-banana-2.1",
                "gemini-2.5-pro",
                "gemini-2.5-flash",
            ],
            Provider::Vertex => &[
                "gemini-4-argon",
                "gemini-3.8-flash",
                "gemini-3.7-flash",
                "gemini-3.1-pro",
                "gemini-3.5-flash-lite",
                "gemini-3.1-flash-image",
                "gemini-2.5-pro",
                "gemini-2.5-flash",
                "imagen-4.0-generate-001",
                "imagen-4.0-ultra-generate-001",
                "imagen-4.0-fast-generate-001",
            ],
            Provider::Antigravity => ANTIGRAVITY_MODELS,
            Provider::Kimi => &[
                "kimi-k3.1-pro",
                "kimi-k3.1-pro-highspeed",
                "kimi-k3",
                "kimi-k3-256k",
                "kimi-k2.8",
                "kimi-k2.8-code",
                "kimi-k2.7-code",
                "kimi-k2.7-code-highspeed",
                "kimi-k2.6",
                "kimi-k2.5",
                "kimi-k2-thinking",
            ],
            Provider::Xai => &[
                "grok-4.7",
                "grok-4.7-build-fast",
                "grok-4.6",
                "grok-build-0.1",
                "grok-4.5",
                "grok-4.3",
                "grok-4.20-0309-reasoning",
                "grok-4.20-0309-non-reasoning",
                "grok-3-mini",
                "grok-imagine-image-2.0",
                "grok-imagine-image-quality",
                "grok-imagine-video-1.5",
                "grok-imagine-video-1.5-lite",
            ],
            Provider::Meta => {
                &["muse-wonder-1.0", "muse-wonder-full-1.0", "muse-spark-1.3", "muse-spark-1.2", "muse-spark-1.1"]
            }
            Provider::Devin => DEVIN_MODELS,
            Provider::Compat => &[],
        }
    }
}

#[derive(Debug, Clone)]
pub enum Credential {
    OAuth(OAuth),
    ApiKey { key: String, base_url: Option<String> },
}

impl Credential {
    /// A stable fingerprint for subscription state; access and refresh tokens
    /// change routinely and must not reset attribution.
    pub(crate) fn identity(&self, provider: Provider) -> String {
        use sha2::{Digest, Sha256};
        let (kind, owner, base_url) = match self {
            Self::OAuth(o) => ("oauth", o.account_id.as_deref().or(o.email.as_deref()), o.base_url.as_deref()),
            Self::ApiKey { key, base_url } => ("api-key", Some(key.as_str()), base_url.as_deref()),
        };
        let bytes = serde_json::to_vec(&(provider, kind, owner, base_url)).unwrap();
        hex::encode(Sha256::digest(bytes))
    }
}

#[derive(Debug, Clone, Default)]
pub struct OAuth {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub email: Option<String>,
    /// ChatGPT account id (Codex) or Anthropic account uuid (Claude).
    pub account_id: Option<String>,
    /// Optional API base override (e.g. a gateway), from `base_url` in the file.
    pub base_url: Option<String>,
    /// Google Cloud project (Antigravity, Vertex).
    pub project_id: Option<String>,
    /// Every other field of the credential file (provider specific extras).
    pub raw: Map<String, Value>,
}

impl OAuth {
    pub fn field(&self, key: &str) -> Option<&str> {
        self.raw.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
    }
}

#[derive(Clone)]
pub struct PendingOAuthSave {
    pub credential: OAuth,
    /// Owner on disk before this rotation; a replacement must never be overwritten.
    pub expected_identity: String,
    pub extra: Vec<(String, Value)>,
    pub retry_at: std::time::Instant,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Counters {
    pub requests: u64,
    pub failures: u64,
    pub cancelled: u64,
    pub usage_missing: u64,
    pub usage_partial: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
}

impl Counters {
    pub fn record(&mut self, log: &crate::state::RequestLog) {
        self.requests += 1;
        if log.status == 499 {
            self.cancelled += 1;
        } else if log.status >= 400 {
            self.failures += 1;
        }
        self.usage_missing += u64::from(log.usage_completeness == "missing");
        self.usage_partial += u64::from(log.usage_completeness == "partial");
        self.input_tokens += log.input_tokens;
        self.output_tokens += log.output_tokens;
        self.cache_tokens += log.cache_tokens;
    }
}

/// An account's latest problem, as the dashboard and notifications show it.
#[derive(Debug, Clone)]
struct AccountError {
    id: String,
    message: String,
    /// Temporary upstream failures and pauses stop being current at this deadline.
    until: Option<DateTime<Utc>>,
    /// Set when the problem is only a pause (spent quota or a rate limit): it ends
    /// with that pause instead of waiting for a request to succeed.
    pause: Option<ErrorPause>,
}

#[derive(Debug, Clone)]
struct ErrorPause {
    quota: bool,
    /// The paused model, as the cooldown is keyed ("*" = every model).
    model: String,
}

#[derive(Debug, Default)]
pub struct AccountState {
    pub latency: HashMap<(String, crate::latency::Context), crate::latency::Sample>,
    /// Shared across account reloads so in-flight requests remain counted.
    pub active_requests: Arc<AtomicUsize>,
    /// Codex's `x-models-etag` from the latest response: a changed value means its model list changed.
    pub models_etag: Option<String>,
    pub disabled: bool,
    /// Cooldowns keyed by model ("*" = whole account).
    pub cooldowns: HashMap<String, DateTime<Utc>>,
    /// Confirmed subscription exhaustion, distinct from temporary rate limits. A
    /// banked reset or a fresh usage poll clears it; sessions move off it.
    pub quota_cooldowns: HashMap<String, DateTime<Utc>>,
    pub quota_epoch: u64,
    /// Fingerprint bound to the quota and attribution under this same lock.
    pub credential_identity: String,
    /// Changes only when the credential's identity changes, so old attempts and
    /// upstream sockets cannot be reused for another subscription.
    pub identity_epoch: u64,
    pub quota_refreshing: bool,
    pub banked_resets: Option<crate::banked_resets::View>,
    /// Unexpired banked resets at the last good inventory read (ID, expiry, count), so
    /// one that expires unused is noticed once.
    pub reset_watch: Vec<(String, DateTime<Utc>, u64)>,
    pub window_starter: Option<crate::window_starter::View>,
    /// Foreground attempts cancel an idle-window preflight or pending send.
    pub window_starter_cancel: Option<Arc<tokio_util::sync::CancellationToken>>,
    pub strikes: u32,
    last_error: Option<AccountError>,
    pub last_used: Option<DateTime<Utc>>,
    pub counters: Counters,
    /// Subscription usage windows (Claude, ChatGPT).
    pub quota: crate::quota::Quota,
    /// Which clients and sessions used those windows.
    pub attribution: crate::attribution::Attribution,
}

impl AccountState {
    /// Invalidate subscription-specific observations while keeping lifetime
    /// counters and in-flight accounting attached to this configured slot.
    pub(crate) fn update_identity(&mut self, identity: String) -> bool {
        if self.credential_identity == identity {
            return false;
        }
        self.quota_epoch += 1;
        self.identity_epoch += 1;
        self.credential_identity = identity;
        self.quota_refreshing = false;
        self.quota = Default::default();
        self.latency.clear();
        self.cooldowns.clear();
        self.quota_cooldowns.clear();
        self.models_etag = None;
        self.banked_resets = None;
        self.reset_watch.clear();
        self.window_starter = None;
        if let Some(cancel) = self.window_starter_cancel.take() {
            cancel.cancel();
        }
        self.attribution = Default::default();
        self.attribution.dirty = true;
        self.strikes = 0;
        self.clear_error();
        true
    }

    /// A quota reading: the windows, and the attribution of whatever rose since the last one.
    pub fn set_quota(&mut self, windows: Vec<crate::quota::Window>, plan: Option<String>) {
        if plan.as_ref().zip(self.quota.plan.as_ref()).is_some_and(|(new, old)| !crate::quota::same_plan(new, old)) {
            self.attribution.history.clear();
            self.attribution.dirty = true;
        }
        self.attribution.reading(&windows, Utc::now());
        self.quota.set(windows, plan);
    }

    /// The latest problem, unless its temporary-error or pause deadline has passed.
    pub fn last_error(&self, now: DateTime<Utc>) -> Option<&str> {
        let error = self.last_error.as_ref()?;
        error.until.is_none_or(|until| until > now).then_some(error.message.as_str())
    }

    pub fn last_error_id(&self, now: DateTime<Utc>) -> Option<&str> {
        self.last_error(now)?;
        self.last_error.as_ref().map(|error| error.id.as_str())
    }

    /// Records a problem that stays until a request succeeds or the account is reset.
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.last_error = Some(AccountError {
            id: uuid::Uuid::new_v4().to_string(),
            message: message.into(),
            until: None,
            pause: None,
        });
    }

    /// A recent upstream failure, without requiring another inference to clear it.
    pub fn set_temporary_error(&mut self, message: impl Into<String>, until: DateTime<Utc>) {
        self.last_error = Some(AccountError {
            id: uuid::Uuid::new_v4().to_string(),
            message: message.into(),
            until: Some(until),
            pause: None,
        });
    }

    pub fn clear_error(&mut self) {
        self.last_error = None;
    }

    /// Dismiss the displayed occurrence without losing a newer failure, including
    /// a repeated failure with the same message. An already-cleared error is OK.
    pub fn clear_error_if(&mut self, id: &str) -> bool {
        if self.last_error.as_ref().is_some_and(|error| error.id != id) {
            return false;
        }
        self.clear_error();
        true
    }

    fn pause_error(&mut self, message: &str, quota: bool, model: &str, until: DateTime<Utc>) {
        let pause = Some(ErrorPause { quota, model: model.to_string() });
        self.last_error = Some(AccountError {
            id: uuid::Uuid::new_v4().to_string(),
            message: message.to_string(),
            until: Some(until),
            pause,
        });
    }

    /// Ends a spent-quota error early once nothing keeps that model paused for quota,
    /// as when a usage reading or a banked reset shows the allowance is back.
    pub fn settle_quota_error(&mut self, now: DateTime<Utc>) {
        let Some(p) = self.last_error.as_ref().and_then(|e| e.pause.as_ref()).filter(|p| p.quota) else { return };
        let paused = [p.model.as_str(), "*"].iter().any(|m| self.quota_cooldowns.get(*m).is_some_and(|t| *t > now));
        let spent = self.quota.exhausted(if p.model == "*" { "" } else { &p.model });
        if !paused && !spent {
            self.last_error = None;
        }
    }

    /// What pauses the account, per model ("*" = every model): until when, and why
    /// (`rate_limit`, spent subscription `quota`, or a quota check, `checking`).
    pub fn pauses(&self, now: DateTime<Utc>) -> BTreeMap<String, (DateTime<Utc>, &'static str)> {
        let mut pauses: BTreeMap<String, (DateTime<Utc>, &'static str)> =
            self.cooldowns.iter().filter(|(_, t)| **t > now).map(|(k, t)| (k.clone(), (*t, "rate_limit"))).collect();
        for (model, deadline) in &self.quota_cooldowns {
            if *deadline > now && pauses.get(model).is_none_or(|(t, _)| deadline > t) {
                pauses.insert(model.clone(), (*deadline, "quota"));
            }
        }
        if self.quota_refreshing {
            pauses.insert("*".into(), (now + chrono::Duration::seconds(30), "checking"));
        }
        // A used-up usage window blocks every model until it resets.
        if let Some(t) = self
            .quota
            .exhausted_until("")
            .or_else(|| self.quota.exhausted("").then(|| now + chrono::Duration::minutes(5)))
            && !pauses.contains_key("*")
        {
            pauses.insert("*".into(), (t, "quota"));
        }
        pauses
    }
}

/// Counts an actual request attempt, including streaming, until completion or cancellation.
pub struct RequestLoad(Arc<AtomicUsize>);

impl RequestLoad {
    pub fn new(account: &Account) -> Self {
        let state = account.state.lock();
        if let Some(cancel) = &state.window_starter_cancel {
            cancel.cancel();
        }
        let active = state.active_requests.clone();
        active.fetch_add(1, Ordering::Relaxed);
        Self(active)
    }
}

impl Drop for RequestLoad {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

pub struct Account {
    pub id: String,
    pub provider: Provider,
    pub label: String,
    pub path: Option<PathBuf>,
    /// Display name of the openai-compatibility group.
    pub group: Option<String>,
    /// Public model name -> upstream model name. Empty = provider default families.
    pub models: Vec<ModelAlias>,
    pub headers: BTreeMap<String, String>,
    pub proxy_url: Option<String>,
    pub cred: Arc<RwLock<Credential>>,
    /// Shared by config-only replacements so an in-flight refresh publishes its
    /// identity epoch to the currently selected Account allocation as well.
    pub state: Arc<Mutex<AccountState>>,
    pub refresh_lock: Arc<tokio::sync::Mutex<()>>,
    /// Rotated credentials that still need saving. Shared with in-flight users
    /// across account/config reloads, as are the credentials and refresh lock.
    pub pending_oauth_save: Arc<Mutex<Option<PendingOAuthSave>>>,
    /// Stable per-account device id for Claude cloaking.
    pub device_id: String,
    pub session_id: String,
    /// Models the upstream lists for this account (Antigravity, Claude and Codex sign-ins).
    pub discovered: RwLock<Vec<String>>,
    /// Models added from the dashboard (`extra-models`) until Fusebox knows them.
    pub extra: Vec<String>,
    /// Clients reach this account as `prefix/model`.
    pub prefix: Option<String>,
    /// Model patterns this account must not serve.
    pub excluded: Vec<String>,
    /// OAuth model renames (config `oauth-model-alias` + the file's `model_aliases`).
    pub aliases: Vec<OAuthAlias>,
    /// Higher goes first; lower priorities only take over (the file's or key's `priority`).
    pub priority: i32,
}

/// `gemini-3-8-flash` -> `gemini-3.8-flash`, `gpt-6-1-sol` -> `gpt-6.1-sol`
/// (single digits joined by a hyphen are a version number).
fn dot_versions(m: &str) -> String {
    let b = m.as_bytes();
    let mut out = String::with_capacity(m.len());
    for (i, c) in m.char_indices() {
        let digit = |j: usize| b.get(j).is_some_and(|c| c.is_ascii_digit());
        let lone = |j: usize| digit(j) && !digit(j.wrapping_sub(1)) && !digit(j + 1);
        // Only between two single digits, so dates (20251001) are left alone.
        if c == '-' && i > 0 && lone(i - 1) && lone(i + 1) && (i < 2 || b[i - 2] == b'-') {
            out.push('.');
        } else {
            out.push(c);
        }
    }
    out
}

/// `*` wildcard match, case-insensitive (`gemini-2.5-*`, `*-preview`, `*flash*`).
pub fn wildcard(pattern: &str, s: &str) -> bool {
    let pattern = pattern.trim();
    if !pattern.contains('*') {
        return pattern.eq_ignore_ascii_case(s);
    }
    let mut rest = s.as_bytes();
    let mut parts = pattern.split('*').enumerate().peekable();
    while let Some((i, part)) = parts.next() {
        let part = part.as_bytes();
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !rest.get(..part.len()).is_some_and(|prefix| prefix.eq_ignore_ascii_case(part)) {
                return false;
            }
            rest = &rest[part.len()..];
        } else if parts.peek().is_none() {
            return rest
                .get(rest.len().saturating_sub(part.len())..)
                .is_some_and(|suffix| suffix.len() == part.len() && suffix.eq_ignore_ascii_case(part));
        } else {
            let Some(at) = rest.windows(part.len()).position(|window| window.eq_ignore_ascii_case(part)) else {
                return false;
            };
            rest = &rest[at + part.len()..];
        }
    }
    true
}

impl Account {
    pub fn first_party(&self, model: &str) -> bool {
        !self.models.is_empty() || self.provider.first_party(model)
    }

    pub fn is_oauth(&self) -> bool {
        matches!(*self.cred.read(), Credential::OAuth(_))
    }

    /// Wonder Full is a Muse Code subscription offering, not a pay-as-you-go
    /// Model API model. Explicit model mappings can describe other entitlements.
    fn default_model_allowed(&self, model: &str) -> bool {
        self.provider != Provider::Meta
            || !model.to_ascii_lowercase().starts_with("muse-wonder-full")
            || self.is_oauth()
    }

    /// Upstream model name if this account can serve `model`.
    pub fn resolve(&self, model: &str) -> Option<String> {
        self.resolve_with(model, false)
    }

    fn excludes(&self, upstream: &str) -> bool {
        self.excluded.iter().any(|p| wildcard(p, upstream))
    }

    /// `forced`: the client named this provider explicitly, so any model of its family goes.
    pub fn resolve_with(&self, model: &str, forced: bool) -> Option<String> {
        self.resolve_inner(model, forced).filter(|m| !self.excludes(m))
    }

    fn resolve_inner(&self, model: &str, forced: bool) -> Option<String> {
        if !self.models.is_empty() {
            return self.models.iter().find(|m| m.public().eq_ignore_ascii_case(model)).map(|m| m.name.clone());
        }
        if let Some(a) = self.aliases.iter().find(|a| a.alias.eq_ignore_ascii_case(model)) {
            return Some(a.name.clone());
        }
        // Renamed without `fork`: the original name is gone.
        if self.aliases.iter().any(|a| a.name.eq_ignore_ascii_case(model) && !a.fork) {
            return None;
        }
        if !self.default_model_allowed(model) {
            return None;
        }
        let aggregator = matches!(self.provider, Provider::Antigravity | Provider::Devin);
        let known = if forced { aggregator || self.provider.family(model) } else { self.provider.serves(model) }
            || self.discovered.read().iter().any(|m| m.eq_ignore_ascii_case(model))
            || self.extra.iter().any(|m| m.eq_ignore_ascii_case(model));
        known.then(|| self.upstream_name(model))
    }

    /// Kimi Code exposes `k3`, `kimi-for-coding`, ... while clients use `kimi-*` names.
    fn upstream_name(&self, model: &str) -> String {
        if self.provider != Provider::Kimi || self.custom_base() {
            return model.to_string();
        }
        let base = model.trim().to_ascii_lowercase();
        let base = base.trim_end_matches("[1m]");
        match base {
            "kimi-k2.8" | "kimi-k2.8-code" | "kimi-k2.7-code" | "kimi-for-coding" => "kimi-for-coding".into(),
            "kimi-k2.7-code-highspeed" | "kimi-for-coding-highspeed" => "kimi-for-coding-highspeed".into(),
            other => other.strip_prefix("kimi-").unwrap_or(other).to_string(),
        }
    }

    /// True when an API key points at a non-default endpoint.
    fn custom_base(&self) -> bool {
        match &*self.cred.read() {
            Credential::ApiKey { base_url, .. } => base_url.as_ref().is_some_and(|b| !b.trim().is_empty()),
            Credential::OAuth(_) => false,
        }
    }

    /// Whether `model` is one of `public_models()`, without building the list. The
    /// list keeps the first spelling of each name, so only that entry decides.
    pub fn lists(&self, model: &str) -> bool {
        let same = |m: &str| m.eq_ignore_ascii_case(model);
        if !self.models.is_empty() {
            return self.models.iter().any(|m| same(m.public()) && !self.excludes(&m.name));
        }
        let shown = |m: &str| {
            self.default_model_allowed(m)
                && !self.excludes(m)
                && !self.aliases.iter().any(|a| a.name.eq_ignore_ascii_case(m) && !a.fork)
        };
        let first = match self.provider.builtin_models().iter().find(|m| same(m)) {
            Some(m) => Some(shown(m)),
            None => match self.discovered.read().iter().find(|m| same(m)) {
                Some(m) => Some(shown(m)),
                None => self.extra.iter().find(|m| same(m)).map(|m| shown(m)),
            },
        };
        first == Some(true) || self.aliases.iter().any(|a| same(&a.alias) && !self.excludes(&a.name))
    }

    pub fn public_models(&self) -> Vec<String> {
        if !self.models.is_empty() {
            return self.models.iter().filter(|m| !self.excludes(&m.name)).map(|m| m.public().to_string()).collect();
        }
        let mut out: Vec<String> = self.provider.builtin_models().iter().map(|s| s.to_string()).collect();
        for m in self.discovered.read().iter().chain(&self.extra) {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(m)) {
                out.push(m.clone());
            }
        }
        out.retain(|m| {
            self.default_model_allowed(m)
                && !self.excludes(m)
                && !self.aliases.iter().any(|a| a.name.eq_ignore_ascii_case(m) && !a.fork)
        });
        for a in &self.aliases {
            if !self.excludes(&a.name) && !out.iter().any(|o| o.eq_ignore_ascii_case(&a.alias)) {
                out.push(a.alias.clone());
            }
        }
        out
    }

    pub fn cooling_until(&self, model: &str) -> Option<DateTime<Utc>> {
        let quota_model = self.resolve(model).unwrap_or_else(|| model.to_string());
        let st = self.state.lock();
        let now = Utc::now();
        if st.quota_refreshing {
            return Some(now + chrono::Duration::seconds(30));
        }
        // A used-up quota window counts as a cooldown, so we don't wait for the 429.
        let spent = st
            .quota
            .exhausted_until(&quota_model)
            .or_else(|| st.quota.exhausted(&quota_model).then(|| Utc::now() + chrono::Duration::minutes(5)));
        [
            st.cooldowns.get("*"),
            st.cooldowns.get(model),
            st.quota_cooldowns.get("*"),
            st.quota_cooldowns.get(model),
            spent.as_ref(),
        ]
        .into_iter()
        .flatten()
        .filter(|t| **t > now)
        .max()
        .copied()
    }

    pub fn exhausted_until(&self, model: &str) -> Option<DateTime<Utc>> {
        let quota_model = self.resolve(model).unwrap_or_else(|| model.to_string());
        let st = self.state.lock();
        let spent = st
            .quota
            .exhausted_until(&quota_model)
            .or_else(|| st.quota.exhausted(&quota_model).then(|| Utc::now() + chrono::Duration::minutes(5)));
        [st.quota_cooldowns.get("*"), st.quota_cooldowns.get(model), spent.as_ref()]
            .into_iter()
            .flatten()
            .filter(|t| **t > Utc::now())
            .max()
            .copied()
    }

    /// Records confirmed quota exhaustion, unless a banked reset is being applied.
    pub fn exhaust(&self, model: &str, until: DateTime<Utc>, reason: &str) {
        let epoch = self.quota_epoch();
        self.cool_quota(model, until, reason, epoch);
    }

    /// Pauses the account (or one model) for a problem that stays shown until a
    /// request succeeds: sign-in, permission, a stalled upstream.
    pub fn cool(&self, model: Option<&str>, until: DateTime<Utc>, reason: &str) {
        let mut st = self.state.lock();
        st.cooldowns.insert(model.unwrap_or("*").to_string(), until);
        st.set_error(reason);
    }

    /// Pauses a model for a rate limit; the error ends with the pause.
    pub fn rate_limit(&self, model: &str, until: DateTime<Utc>, reason: &str) {
        let mut st = self.state.lock();
        st.cooldowns.insert(model.to_string(), until);
        st.pause_error(reason, false, model, until);
    }

    pub fn quota_epoch(&self) -> u64 {
        self.state.lock().quota_epoch
    }

    pub fn cool_quota(&self, model: &str, until: DateTime<Utc>, reason: &str, epoch: u64) {
        let mut st = self.state.lock();
        if st.quota_epoch == epoch && !st.quota_refreshing {
            st.quota_cooldowns.insert(model.to_string(), until);
            st.pause_error(reason, true, model, until);
        }
    }

    pub fn record_ok(&self) {
        let mut st = self.state.lock();
        st.strikes = 0;
        st.clear_error();
    }

    pub fn snapshot(&self) -> Value {
        let st = self.state.lock();
        let now = Utc::now();
        let pauses = st.pauses(now);
        let cooldowns: BTreeMap<&str, String> = pauses.iter().map(|(k, (t, _))| (k.as_str(), t.to_rfc3339())).collect();
        let kinds: BTreeMap<&str, &str> = pauses.iter().map(|(k, (_, kind))| (k.as_str(), *kind)).collect();
        let (kind, expires, email) = match &*self.cred.read() {
            Credential::OAuth(o) if o.raw.contains_key("service_account") => ("service-account", None, o.email.clone()),
            Credential::OAuth(o) => ("oauth", o.expires_at.map(|t| t.to_rfc3339()), o.email.clone()),
            Credential::ApiKey { .. } => ("api-key", None, None),
        };
        serde_json::json!({
            "id": self.id,
            "provider": self.provider,
            "label": self.label,
            "email": email,
            "kind": kind,
            "group": self.group,
            "file": self.path.as_ref().and_then(|p| p.file_name()).map(|f| f.to_string_lossy().to_string()),
            "disabled": st.disabled,
            "cooldowns": cooldowns,
            "cooldown_kinds": kinds,
            "quota_reset_unknown": st.quota.reset_unknown(""),
            "last_error": st.last_error(now),
            "last_error_id": st.last_error_id(now),
            "last_error_until": st.last_error.as_ref().and_then(|e| e.until).filter(|t| *t > now),
            "last_used": st.last_used.map(|t| t.to_rfc3339()),
            "expires_at": expires,
            "counters": st.counters,
            "priority": self.priority,
            "quota": st.quota,
            "banked_resets": st.banked_resets,
            "window_starter": st.window_starter,
            "models": self.public_models(),
        })
    }
}

// --------------------------------------------------------------------- loading

fn parse_time(v: &Value) -> Option<DateTime<Utc>> {
    match v {
        Value::String(s) => DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&Utc)),
        Value::Number(n) => {
            let n = n.as_i64()?;
            let secs = if n > 10_000_000_000 { n / 1000 } else { n };
            DateTime::from_timestamp(secs, 0)
        }
        _ => None,
    }
}

pub fn file_provider(kind: &str) -> Option<Provider> {
    let k = kind.trim().to_ascii_lowercase();
    if k.starts_with("kimi") {
        return Some(Provider::Kimi);
    }
    match k.as_str() {
        "gemini" | "gemini-cli" | "aistudio" | "compat" | "openai-compat" => None,
        other => Provider::parse(other),
    }
}

pub fn read_oauth_file(path: &Path) -> Option<(Provider, OAuth, bool, Map<String, Value>)> {
    let text = std::fs::read_to_string(path).ok()?;
    let map: Map<String, Value> = serde_json::from_str(&text).ok()?;
    let provider = file_provider(map.get("type").and_then(Value::as_str)?)?;
    let s = |k: &str| map.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let mut access = s("access_token");
    if provider == Provider::Meta {
        // Meta requests use the API key minted from the device token.
        access = s("api_key").or(access).filter(|t| !t.starts_with("dca:"));
    }
    if provider == Provider::Devin {
        access = s("session_token").or_else(|| s("api_key")).or(access);
    }
    let oauth = OAuth {
        access_token: access.unwrap_or_default(),
        refresh_token: s("refresh_token").unwrap_or_default(),
        expires_at: map.get("expired").or_else(|| map.get("expires_at")).and_then(parse_time),
        email: s("email").or_else(|| s("client_email")),
        account_id: s("account_id").or_else(|| s("account_uuid")),
        base_url: s("base_url"),
        project_id: s("project_id")
            .or_else(|| map.get("service_account").and_then(|v| v["project_id"].as_str()).map(String::from)),
        raw: map.clone(),
    };
    let disabled = map.get("disabled").and_then(Value::as_bool).unwrap_or(false);
    Some((provider, oauth, disabled, map))
}

/// Writes refreshed tokens back into the credential file, keeping unknown fields.
pub fn write_oauth_file(path: &Path, provider: Provider, o: &OAuth, extra: &[(&str, Value)]) -> std::io::Result<()> {
    let mut map = o.raw.clone();
    if let Some(disk) =
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Map<String, Value>>(&t).ok())
    {
        map.extend(disk);
    }
    map.insert("type".into(), provider.as_str().into());
    map.insert("access_token".into(), o.access_token.clone().into());
    map.insert("refresh_token".into(), o.refresh_token.clone().into());
    if let Some(e) = &o.email {
        map.insert("email".into(), e.clone().into());
    }
    if let Some(t) = o.expires_at {
        map.insert("expired".into(), t.to_rfc3339().into());
    }
    if let Some(a) = &o.account_id {
        let key = if provider == Provider::Claude { "account_uuid" } else { "account_id" };
        map.insert(key.into(), a.clone().into());
    }
    if let Some(p) = &o.project_id {
        map.insert("project_id".into(), p.clone().into());
    }
    if let Some(b) = &o.base_url {
        map.insert("base_url".into(), b.clone().into());
    }
    if provider == Provider::Meta {
        map.insert("api_key".into(), o.access_token.clone().into());
    }
    if provider == Provider::Devin {
        map.insert("api_key".into(), o.access_token.clone().into());
        map.insert("session_token".into(), o.access_token.clone().into());
        map.remove("access_token");
    }
    if o.refresh_token.is_empty() {
        map.remove("refresh_token");
    }
    map.insert("last_refresh".into(), Utc::now().to_rfc3339().into());
    for (k, v) in extra {
        map.insert((*k).into(), v.clone());
    }
    write_credential(path, &map)
}

/// Credential files are replaced atomically and are never readable by others, not
/// even while they are being written.
fn write_credential(path: &Path, map: &Map<String, Value>) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(map)?;
    crate::files::write_private_via(path, &path.with_extension("json.tmp"), &bytes).map_err(std::io::Error::other)
}

/// Sets or removes one top-level field of a credential file, keeping everything else.
pub fn set_file_field(path: &Path, key: &str, value: Option<Value>) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let mut map: Map<String, Value> = serde_json::from_str(&text).map_err(std::io::Error::other)?;
    match value {
        Some(v) => map.insert(key.into(), v),
        None => map.remove(key),
    };
    write_credential(path, &map)
}

pub fn set_file_disabled(path: &Path, disabled: bool) -> std::io::Result<()> {
    set_file_field(path, "disabled", disabled.then_some(Value::Bool(true)))
}

/// `sk-ant…f3e2`: whole characters, so keys that aren't ASCII can't split one.
fn mask(key: &str) -> String {
    let k: Vec<char> = key.trim().chars().collect();
    let take = |r: std::ops::Range<usize>| k[r].iter().collect::<String>();
    if k.len() <= 10 {
        return format!("{}…", take(0..k.len().min(3)));
    }
    format!("{}…{}", take(0..6), take(k.len() - 4..k.len()))
}

/// The account id an API key gets (`claude:1a2b3c4d5e`), for finding it again.
pub fn key_account_id(prefix: &str, key: &str) -> String {
    key_id(prefix, key)
}

fn key_id(prefix: &str, key: &str) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(key.as_bytes());
    format!("{prefix}:{}", &hex::encode(h)[..10])
}

fn random_hex(n: usize) -> String {
    let bytes: Vec<u8> = (0..n).map(|_| rand::random::<u8>()).collect();
    hex::encode(bytes)
}

struct Spec {
    id: String,
    provider: Provider,
    label: String,
    path: Option<PathBuf>,
    group: Option<String>,
    models: Vec<ModelAlias>,
    headers: BTreeMap<String, String>,
    proxy_url: Option<String>,
    cred: Credential,
    disabled: bool,
    device_id: Option<String>,
    prefix: Option<String>,
    excluded: Vec<String>,
    aliases: Vec<OAuthAlias>,
    extra: Vec<String>,
    priority: i32,
    /// Why this account can't be used as it is (a credential file's invalid proxy).
    problem: Option<String>,
}

/// Config sections keyed by provider name (`claude`, `codex`, `aistudio`, ...).
fn for_provider<T: Clone>(map: &BTreeMap<String, Vec<T>>, p: Provider) -> Vec<T> {
    map.iter()
        .filter(|(k, _)| Provider::parse(k) == Some(p) || (k.as_str() == "aistudio" && p == Provider::Gemini))
        .flat_map(|(_, v)| v.clone())
        .collect()
}

fn nonempty(s: &Option<String>) -> Option<String> {
    s.as_ref().map(|s| s.trim().trim_matches('/').to_string()).filter(|s| !s.is_empty())
}

fn collect(cfg: &Config) -> Vec<Spec> {
    let mut specs = Vec::new();
    let dir = cfg.auth_dir();
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(&dir).map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default();
    files.sort();
    for path in files {
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some((provider, oauth, disabled, map)) = read_oauth_file(&path) else { continue };
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let device_id = map
            .get("claude_device_ids")
            .and_then(|v| v.get(0))
            .and_then(Value::as_str)
            .filter(|s| s.len() == 64)
            .map(String::from);
        let mut aliases = for_provider(&cfg.oauth_model_alias, provider);
        let file_aliases = map.get("model_aliases").or_else(|| map.get("model-aliases"));
        if let Some(list) = file_aliases.and_then(|v| serde_json::from_value::<Vec<OAuthAlias>>(v.clone()).ok()) {
            // Per-account aliases take precedence over global ones.
            aliases.retain(|a| !list.iter().any(|l| l.alias.eq_ignore_ascii_case(&a.alias)));
            aliases.splice(0..0, list);
        }
        aliases.retain(|a| !a.name.is_empty() && !a.alias.is_empty());
        let mut excluded = for_provider(&cfg.oauth_excluded_models, provider);
        if let Some(list) =
            map.get("excluded_models").and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
        {
            excluded.extend(list);
        }
        let prefix = nonempty(&map.get("prefix").and_then(Value::as_str).map(String::from));
        let proxy_url = map.get("proxy_url").and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
        let problem = proxy_url.as_deref().and_then(|p| crate::config::check_proxy_url(p).err()).map(|e| {
            tracing::error!("{name}: invalid proxy_url, so the account is off until it is fixed: {e}");
            format!("proxy_url in {name} is invalid: {e}")
        });
        let priority =
            map.get("priority").and_then(Value::as_i64).map(|p| p.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
        specs.push(Spec {
            prefix,
            excluded,
            aliases,
            priority: priority.unwrap_or(0),
            problem,
            extra: for_provider(&cfg.extra_models, provider),
            id: format!("file:{name}"),
            provider,
            label: oauth
                .email
                .clone()
                .or_else(|| oauth.field("user_name").map(String::from))
                .unwrap_or_else(|| name.trim_end_matches(".json").to_string()),
            path: Some(path),
            group: None,
            models: vec![],
            headers: BTreeMap::new(),
            proxy_url,
            cred: Credential::OAuth(oauth),
            disabled,
            device_id,
        });
    }
    let keys = [
        (Provider::Claude, &cfg.claude_api_key),
        (Provider::Codex, &cfg.codex_api_key),
        (Provider::Gemini, &cfg.gemini_api_key),
        (Provider::Vertex, &cfg.vertex_api_key),
        (Provider::Kimi, &cfg.kimi_api_key),
        (Provider::Xai, &cfg.xai_api_key),
        (Provider::Meta, &cfg.meta_api_key),
    ];
    for (provider, entries) in keys {
        for e in entries.iter().filter(|e| !e.api_key.trim().is_empty()) {
            specs.push(Spec {
                id: key_id(provider.as_str(), &e.api_key),
                provider,
                label: e.label.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| mask(&e.api_key)),
                path: None,
                group: None,
                models: e.models.clone(),
                headers: e.headers.clone(),
                proxy_url: e.proxy_url.clone(),
                cred: Credential::ApiKey { key: e.api_key.trim().to_string(), base_url: e.base_url.clone() },
                disabled: false,
                device_id: None,
                prefix: nonempty(&e.prefix),
                excluded: e.excluded_models.clone(),
                aliases: vec![],
                extra: for_provider(&cfg.extra_models, provider),
                priority: e.priority,
                problem: None,
            });
        }
    }
    for c in cfg.openai_compatibility.iter().filter(|c| !c.disabled) {
        for k in c.api_keys.iter().filter(|k| !k.trim().is_empty()) {
            specs.push(Spec {
                id: key_id(&format!("compat-{}", c.name), k),
                provider: Provider::Compat,
                label: format!("{} {}", c.name, mask(k)),
                path: None,
                group: Some(c.name.clone()),
                models: c.models.clone(),
                headers: c.headers.clone(),
                proxy_url: c.proxy_url.clone(),
                cred: Credential::ApiKey { key: k.trim().to_string(), base_url: Some(c.base_url.clone()) },
                disabled: false,
                device_id: None,
                prefix: nonempty(&c.prefix),
                excluded: c.excluded_models.clone(),
                aliases: vec![],
                extra: vec![],
                priority: c.priority,
                problem: None,
            });
        }
        // Keyless local endpoints (Ollama, LM Studio, ...)
        if c.api_keys.iter().all(|k| k.trim().is_empty()) && !c.base_url.is_empty() {
            specs.push(Spec {
                id: format!("compat-{}:nokey", c.name),
                provider: Provider::Compat,
                label: c.name.clone(),
                path: None,
                group: Some(c.name.clone()),
                models: c.models.clone(),
                headers: c.headers.clone(),
                proxy_url: c.proxy_url.clone(),
                cred: Credential::ApiKey { key: String::new(), base_url: Some(c.base_url.clone()) },
                disabled: false,
                device_id: None,
                prefix: nonempty(&c.prefix),
                excluded: c.excluded_models.clone(),
                aliases: vec![],
                extra: vec![],
                priority: c.priority,
                problem: None,
            });
        }
    }
    specs
}

// ------------------------------------------------------------------------ pool

#[derive(Default)]
pub struct Pool {
    accounts: RwLock<Vec<Arc<Account>>>,
    cursor: Mutex<HashMap<String, usize>>,
    /// `force-model-prefix`: unprefixed names skip accounts that have a prefix.
    force_prefix: std::sync::atomic::AtomicBool,
}

/// Restricts a request to some accounts (`provider/model` or `prefix/model`).
#[derive(Debug, Clone, PartialEq)]
pub enum Only {
    Provider(Provider),
    Prefix(String),
}

pub enum Pick {
    Ok(Arc<Account>, String),
    /// Every candidate is cooling down; earliest availability.
    Cooling(DateTime<Utc>),
    None,
}

struct SmartQuotaScore {
    five_hour_remaining: Option<f64>,
    weekly_remaining: Option<f64>,
    weekly_reset: Option<DateTime<Utc>>,
    banked_reset_expiry: Option<DateTime<Utc>>,
    load: usize,
    weight: f64,
}

impl SmartQuotaScore {
    /// Reserves protect sessions already using an account, not idle allowance.
    /// An idle account may admit a session below either reserve; that assignment
    /// counts immediately, so subsequent new sessions protect its remaining quota.
    fn needs_reserve(&self, five_hour_reserve: f64) -> bool {
        self.load > 0
            && (self.five_hour_remaining.is_some_and(|left| left < five_hour_reserve)
                || self.weekly_remaining.is_some_and(|left| left < SMART_WEEKLY_RESERVE))
    }
}

impl Pool {
    /// Resolve a pinned account without replacing it with a different provider.
    pub fn resolve_account(&self, a: &Account, model: &str, only: Option<&Only>) -> Option<String> {
        let allowed = match only {
            Some(Only::Provider(p)) => *p == a.provider,
            Some(Only::Prefix(x)) => a.prefix.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(x)),
            None => !(self.force_prefix.load(std::sync::atomic::Ordering::Relaxed) && a.prefix.is_some()),
        };
        allowed.then(|| a.resolve_with(model, matches!(only, Some(Only::Provider(_))))).flatten()
    }
    pub fn reload(&self, cfg: &Config) {
        self.force_prefix.store(cfg.force_model_prefix, std::sync::atomic::Ordering::Relaxed);
        let specs = collect(cfg);
        for s in &specs {
            // `claude/<model>` would otherwise pick any Claude account.
            if let Some(prefix) = s.prefix.as_deref().filter(|p| Provider::parse(p).is_some()) {
                tracing::warn!(
                    "{}: model prefix \"{prefix}\" is a provider name, so {prefix}/<model> only reaches this account; choose another prefix",
                    s.label
                );
            }
        }
        let old: HashMap<String, Arc<Account>> =
            self.accounts.read().iter().map(|a| (a.id.clone(), a.clone())).collect();
        let mut next = Vec::with_capacity(specs.len());
        for mut s in specs {
            let mut identity_changed = false;
            if let Some(prev) = old.get(&s.id) {
                // Never replace a successfully rotated token with its older disk
                // copy. A shared refresh lock also prevents a changed config from
                // starting a second refresh while the old Account is in flight.
                let guard = prev.refresh_lock.try_lock();
                let pending = prev.pending_oauth_save.lock();
                if let Some(path) = &s.path {
                    // Collection happened before taking the refresh lock; reread
                    // here in case a refresh completed while files were scanned.
                    if let Some((_, oauth, _, _)) = read_oauth_file(path) {
                        s.cred = Credential::OAuth(oauth);
                    }
                }
                let current = prev.cred.read().clone();
                let current_identity = current.identity(prev.provider);
                let disk_identity = s.cred.identity(s.provider);
                let older_copy = disk_identity == current_identity
                    || pending.as_ref().is_some_and(|save| save.expected_identity == disk_identity);
                if (guard.is_err() || pending.is_some()) && older_copy {
                    s.cred = current;
                }
                let identity = s.cred.identity(s.provider);
                identity_changed = current_identity != identity || prev.state.lock().credential_identity != identity;
                if !identity_changed && guard.is_ok() && pending.is_none() {
                    *prev.cred.write() = s.cred.clone();
                }
                if identity_changed {
                    prev.state.lock().update_identity(identity);
                }
                // Keep counters and cooldowns; refresh credentials from disk/config.
                let same_shape = !identity_changed
                    && prev.provider == s.provider
                    && prev.label == s.label
                    && prev.path == s.path
                    && prev.group == s.group
                    && prev.models.iter().map(|m| (&m.name, &m.alias)).eq(s.models.iter().map(|m| (&m.name, &m.alias)))
                    && prev.headers == s.headers
                    && prev.proxy_url == s.proxy_url
                    && prev.prefix == s.prefix
                    && prev.excluded == s.excluded
                    && prev.aliases == s.aliases
                    && prev.extra == s.extra
                    && prev.priority == s.priority;
                if same_shape {
                    let mut st = prev.state.lock();
                    st.disabled = s.disabled || s.problem.is_some();
                    if let Some(problem) = &s.problem {
                        st.set_error(problem.clone());
                    }
                    drop(st);
                    next.push(prev.clone());
                    continue;
                }
            }
            let previous_identity = old.get(&s.id).filter(|_| !identity_changed);
            let discovered = previous_identity.map(|p| p.discovered.read().clone()).unwrap_or_default();
            let mut state = old
                .get(&s.id)
                .map(|p| {
                    let st = p.state.lock();
                    AccountState {
                        active_requests: st.active_requests.clone(),
                        cooldowns: st.cooldowns.clone(),
                        quota_cooldowns: st.quota_cooldowns.clone(),
                        quota_epoch: st.quota_epoch,
                        identity_epoch: st.identity_epoch,
                        models_etag: st.models_etag.clone(),
                        quota_refreshing: st.quota_refreshing,
                        banked_resets: st.banked_resets.clone(),
                        reset_watch: st.reset_watch.clone(),
                        strikes: st.strikes,
                        last_error: st.last_error.clone(),
                        last_used: st.last_used,
                        counters: st.counters.clone(),
                        latency: st.latency.clone(),
                        quota: st.quota.clone(),
                        window_starter: st.window_starter.clone(),
                        window_starter_cancel: st.window_starter_cancel.clone(),
                        attribution: st.attribution.clone(),
                        ..Default::default()
                    }
                })
                .unwrap_or_default();
            state.disabled = s.disabled || s.problem.is_some();
            state.credential_identity = s.cred.identity(s.provider);
            if let Some(problem) = &s.problem {
                state.set_error(problem.clone());
            }
            // Config changes keep one coherent state for both current and
            // in-flight users. A real replacement gets a separate state below.
            if let Some(prev) = previous_identity {
                let mut shared = prev.state.lock();
                shared.disabled = state.disabled;
                if let Some(problem) = &s.problem {
                    shared.set_error(problem.clone());
                }
            }
            let cred = previous_identity.map(|prev| prev.cred.clone()).unwrap_or_else(|| Arc::new(RwLock::new(s.cred)));
            let refresh_lock = previous_identity.map(|prev| prev.refresh_lock.clone()).unwrap_or_default();
            let pending_oauth_save = previous_identity.map(|prev| prev.pending_oauth_save.clone()).unwrap_or_default();
            let device_id = previous_identity.map(|prev| prev.device_id.clone()).unwrap_or_else(|| {
                // A copied credential file can carry the previous owner's device
                // id. Accept an explicitly different device, otherwise rotate it.
                s.device_id
                    .filter(|device| old.get(&s.id).is_none_or(|prev| *device != prev.device_id))
                    .unwrap_or_else(|| random_hex(32))
            });
            if identity_changed && s.provider == Provider::Claude {
                let devices = serde_json::json!([device_id]);
                if let Credential::OAuth(oauth) = &mut *cred.write() {
                    oauth.raw.insert("claude_device_ids".into(), devices.clone());
                }
                // Keep a regenerated device after restart too: a copied auth
                // file must not restore the previous identity's imported id.
                if let Some(path) = &s.path
                    && set_file_field(path, "claude_device_ids", Some(devices)).is_err()
                {
                    state.set_error("saving replacement account device failed");
                }
            }
            let session_id = previous_identity
                .map(|prev| prev.session_id.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            next.push(Arc::new(Account {
                id: s.id,
                provider: s.provider,
                label: s.label,
                path: s.path,
                group: s.group,
                models: s.models,
                headers: s.headers,
                proxy_url: s.proxy_url,
                cred,
                state: previous_identity.map(|prev| prev.state.clone()).unwrap_or_else(|| Arc::new(Mutex::new(state))),
                refresh_lock,
                pending_oauth_save,
                device_id,
                session_id,
                discovered: RwLock::new(discovered),
                extra: s.extra,
                prefix: s.prefix,
                excluded: s.excluded,
                aliases: s.aliases,
                priority: s.priority,
            }));
        }
        // Another program (CLIProxyAPI itself) may be rewriting a credential file
        // right now; keep the account rather than dropping it for a moment.
        for (id, prev) in &old {
            if let Some(p) = &prev.path
                && p.exists()
                && !next.iter().any(|a| a.id == *id)
                && read_oauth_file(p).is_none()
            {
                next.push(prev.clone());
            }
        }
        *self.accounts.write() = next;
    }

    /// The model name some account actually serves, for names that are close:
    /// `gemini-3-8-flash` -> `gemini-3.8-flash`, `google-gemini-...`, `-preview`,
    /// and tiered providers (Antigravity) -> `...-high`.
    pub fn canonical(&self, model: &str, only: Option<&Only>) -> String {
        let force = self.force_prefix.load(std::sync::atomic::Ordering::Relaxed);
        let accounts = self.accounts.read();
        let usable: Vec<&Arc<Account>> = accounts
            .iter()
            .filter(|a| match only {
                Some(Only::Provider(p)) => *p == a.provider,
                Some(Only::Prefix(x)) => a.prefix.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(x)),
                None => !(force && a.prefix.is_some()),
            })
            .filter(|a| !a.state.lock().disabled)
            .collect();
        // A listed model beats one that only matches a family (`gpt-*` takes anything).
        let known = |m: &str| usable.iter().any(|a| a.lists(m));
        let forced = matches!(only, Some(Only::Provider(_)));
        let served = |m: &str| usable.iter().any(|a| a.resolve_with(m, forced).is_some());
        if known(model) {
            return model.to_string();
        }
        let mut bases = vec![model.to_ascii_lowercase()];
        for strip in ["google-", "google/", "models/", "anthropic/", "openai/"] {
            if let Some(rest) = bases[0].strip_prefix(strip) {
                bases.push(rest.to_string());
            }
        }
        for b in bases.clone() {
            bases.push(dot_versions(&b));
        }
        for b in bases.clone() {
            if let Some(rest) = b.strip_suffix("-preview") {
                bases.push(rest.to_string());
            }
        }
        let candidates: Vec<String> =
            bases.iter().flat_map(|b| ["", "-high", "-medium", "-low"].map(|t| format!("{b}{t}"))).collect();
        if let Some(c) = candidates.iter().find(|c| known(c)) {
            return c.clone();
        }
        if served(model) {
            return model.to_string();
        }
        candidates.into_iter().find(|c| served(c)).unwrap_or_else(|| model.to_string())
    }

    pub fn all(&self) -> Vec<Arc<Account>> {
        self.accounts.read().clone()
    }

    /// True when this release or a provider's own model list already has `model`,
    /// so an `extra-models` entry for it is no longer needed.
    pub fn knows(&self, provider: Provider, model: &str) -> bool {
        let same =
            |m: &str| m.eq_ignore_ascii_case(model) || dot_versions(m).eq_ignore_ascii_case(&dot_versions(model));
        provider.builtin_models().iter().any(|m| same(m))
            || self
                .accounts
                .read()
                .iter()
                .filter(|a| a.provider == provider)
                .any(|a| a.discovered.read().iter().any(|m| same(m)))
    }

    pub fn get(&self, id: &str) -> Option<Arc<Account>> {
        self.accounts.read().iter().find(|a| a.id == id).cloned()
    }

    /// Public models served by at least one enabled account (prefixed ones as `prefix/model`).
    pub fn models(&self) -> Vec<(String, Provider)> {
        let force = self.force_prefix.load(std::sync::atomic::Ordering::Relaxed);
        let mut seen = std::collections::BTreeMap::new();
        for a in self.accounts.read().iter() {
            if a.state.lock().disabled {
                continue;
            }
            for m in a.public_models() {
                if let Some(p) = &a.prefix {
                    seen.entry(format!("{p}/{m}")).or_insert(a.provider);
                    if force {
                        continue;
                    }
                }
                seen.entry(m).or_insert(a.provider);
            }
        }
        seen.into_iter().collect()
    }

    /// Splits an optional `provider/` prefix off a model name (`antigravity/claude-sonnet-4-6`).
    /// Names that some account serves verbatim (`openai/gpt-oss` on OpenRouter) are left alone.
    pub fn route(&self, model: &str) -> (Option<Only>, String) {
        if let Some((prefix, rest)) = model.split_once('/')
            && !rest.is_empty()
        {
            let accounts = self.accounts.read();
            if accounts.iter().any(|a| a.prefix.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(prefix))) {
                return (Some(Only::Prefix(prefix.to_string())), rest.to_string());
            }
            if let Some(p) = Provider::parse(prefix)
                && !accounts.iter().any(|a| a.resolve(model).is_some())
            {
                return (Some(Only::Provider(p)), rest.to_string());
            }
        }
        (None, model.to_string())
    }

    pub fn pick(
        &self,
        model: &str,
        exclude: &[String],
        cfg: &Config,
        pinned: Option<&str>,
        only: Option<&Only>,
        session_load: &HashMap<String, usize>,
    ) -> Pick {
        self.pick_with_context(model, exclude, cfg, pinned, only, session_load, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn pick_with_context(
        &self,
        model: &str,
        exclude: &[String],
        cfg: &Config,
        pinned: Option<&str>,
        only: Option<&Only>,
        session_load: &HashMap<String, usize>,
        context: Option<&crate::latency::Context>,
    ) -> Pick {
        let force = self.force_prefix.load(std::sync::atomic::Ordering::Relaxed);
        let accounts = self.accounts.read();
        let mut candidates: Vec<(&Arc<Account>, String)> = Vec::new();
        let mut earliest: Option<DateTime<Utc>> = None;
        for a in accounts.iter() {
            let allowed = match only {
                Some(Only::Provider(p)) => *p == a.provider,
                Some(Only::Prefix(x)) => a.prefix.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(x)),
                None => !(force && a.prefix.is_some()),
            };
            if !allowed || exclude.contains(&a.id) || a.state.lock().disabled {
                continue;
            }
            let forced = matches!(only, Some(Only::Provider(_)));
            let Some(upstream) = a.resolve_with(model, forced) else { continue };
            if let Some(until) = a.cooling_until(model) {
                earliest = Some(earliest.map_or(until, |e| e.min(until)));
                continue;
            }
            candidates.push((a, upstream));
        }
        if candidates.is_empty() {
            return earliest.map(Pick::Cooling).unwrap_or(Pick::None);
        }
        if let Some(pin) = pinned
            && let Some((a, m)) = candidates.iter().find(|(a, _)| a.id == pin)
        {
            return Pick::Ok((*a).clone(), m.clone());
        }
        // Higher priorities go first; lower ones only take over when none of them is ready.
        let top = candidates.iter().map(|(a, _)| a.priority).max().unwrap_or(0);
        candidates.retain(|(a, _)| a.priority == top);
        // Prefer the model vendor for new assignments, while preserving existing overflow pins.
        if candidates.iter().any(|(a, _)| a.first_party(model)) {
            candidates.retain(|(a, _)| a.first_party(model));
        }
        let idx = self.choose(&candidates, model, cfg, session_load, true, context);
        let (a, m) = &candidates[idx];
        Pick::Ok((*a).clone(), m.clone())
    }

    /// Which candidate the routing strategy takes. `advance` moves the turn-taking
    /// cursor; without it this only reports who is next.
    fn choose(
        &self,
        candidates: &[(&Arc<Account>, String)],
        model: &str,
        cfg: &Config,
        session_load: &HashMap<String, usize>,
        advance: bool,
        context: Option<&crate::latency::Context>,
    ) -> usize {
        let take_turn = |count: usize| {
            let mut cur = self.cursor.lock();
            let c = cur.entry(model.to_ascii_lowercase()).or_insert(0);
            let turn = *c % count;
            if advance {
                *c = c.wrapping_add(1);
            }
            turn
        };
        match cfg.routing {
            Routing::FillFirst => 0,
            Routing::SmartQuota => {
                let scores: Vec<_> = candidates.iter().map(|(a, up)| smart_score(a, up, cfg, session_load)).collect();
                let reserve = f64::from(cfg.five_hour_reserve_percent);
                let needs_reserve = |s: &SmartQuotaScore| s.needs_reserve(reserve);
                // This is an admission preference, not a cooldown: never refuse usable
                // accounts merely because all of them need their reserve.
                let protect = scores.iter().any(|s| !needs_reserve(s));
                let eligible = |s: &SmartQuotaScore| !protect || !needs_reserve(s);
                let banked = scores.iter().filter(|s| eligible(s)).filter_map(|s| s.banked_reset_expiry).min();
                // Drain the earliest eligible week. If every account needs its reserve,
                // fall back to quota/load rather than refusing available allowance.
                // Missing five-hour data must not suppress known weekly reset priority.
                let earliest_reset = if protect {
                    scores.iter().filter(|s| eligible(s)).filter_map(|s| s.weekly_reset).min()
                } else {
                    None
                };
                // Renewals within one hour compete on quota and load. A session count alone
                // cannot send new work to an account that renews days later.
                let preferred = |s: &SmartQuotaScore| {
                    eligible(s)
                        && if let Some(first) = banked {
                            s.banked_reset_expiry.is_some_and(|expiry| expiry <= first + chrono::Duration::hours(1))
                        } else {
                            earliest_reset.is_none_or(|first| {
                                s.weekly_reset.is_some_and(|reset| reset <= first + chrono::Duration::hours(1))
                            })
                        }
                };
                let best = scores.iter().filter(|s| preferred(s)).map(|s| s.weight).fold(0.0, f64::max);
                let tied: Vec<_> =
                    (0..scores.len()).filter(|&i| preferred(&scores[i]) && scores[i].weight >= best - 1e-9).collect();
                if cfg.latency_routing
                    && let Some(context) = context
                {
                    let estimates: Option<Vec<_>> = tied
                        .iter()
                        .map(|&i| {
                            let (account, model) = &candidates[i];
                            account
                                .state
                                .lock()
                                .latency
                                .get(&(model.clone(), context.clone()))
                                .and_then(|s| s.estimate())
                                .map(|ms| (i, ms))
                        })
                        .collect();
                    if let Some(estimates) = estimates
                        && let Some((i, _)) = estimates.into_iter().min_by(|a, b| a.1.total_cmp(&b.1))
                    {
                        return i;
                    }
                }
                tied[take_turn(tied.len())]
            }
            Routing::LeastUsed => {
                // Most headroom in its tightest usage window; near-ties take turns.
                let scores: Vec<f64> = candidates.iter().map(|(a, _)| least_used_score(a, model)).collect();
                let best = scores.iter().copied().fold(f64::INFINITY, f64::min);
                let tied: Vec<usize> = (0..candidates.len()).filter(|i| scores[*i] <= best + 2.0).collect();
                tied[take_turn(tied.len())]
            }
            Routing::RoundRobin => take_turn(candidates.len()),
        }
    }

    /// Every account that can serve `model`, in the order new sessions would try them:
    /// the strategy's pick first, then the other ready accounts, aggregator fallbacks,
    /// accounts cooling down, and disabled ones. Read-only: no turn is taken.
    pub fn route_order(
        &self,
        model: &str,
        cfg: &Config,
        only: Option<&Only>,
        session_load: &HashMap<String, usize>,
    ) -> Vec<RouteStep> {
        let accounts = self.accounts.read();
        let mut ready: Vec<(&Arc<Account>, String)> = Vec::new();
        let mut rest: Vec<RouteStep> = Vec::new();
        for a in accounts.iter() {
            let Some(upstream) = self.resolve_account(a, model, only) else { continue };
            if a.state.lock().disabled {
                rest.push(RouteStep::new(a, upstream, "disabled", None));
            } else if let Some(until) = a.cooling_until(model) {
                rest.push(RouteStep::new(a, upstream, "cooling", Some(until)));
            } else {
                ready.push((a, upstream));
            }
        }
        // Accounts below the highest ready priority only take over, like aggregators.
        let top = ready.iter().map(|(a, _)| a.priority).max().unwrap_or(0);
        let (ready, mut lower): (Vec<_>, Vec<_>) = ready.into_iter().partition(|(a, _)| a.priority == top);
        lower.sort_by_key(|(a, _)| std::cmp::Reverse(a.priority));
        let vendor = ready.iter().any(|(a, _)| a.first_party(model));
        let (first, overflow): (Vec<_>, Vec<_>) = ready.into_iter().partition(|(a, _)| !vendor || a.first_party(model));
        let mut steps = Vec::new();
        if !first.is_empty() {
            let next = self.choose(&first, model, cfg, session_load, false, None);
            let mut order: Vec<usize> = (0..first.len()).filter(|&i| i != next).collect();
            if cfg.routing == Routing::SmartQuota {
                // Accounts keeping a reserve come last, then the earliest weekly renewal;
                // quota and load order the rest.
                let reserve = f64::from(cfg.five_hour_reserve_percent);
                let scores: Vec<_> = first.iter().map(|(a, up)| smart_score(a, up, cfg, session_load)).collect();
                let renewal = |s: &SmartQuotaScore| s.weekly_reset.unwrap_or(DateTime::<Utc>::MAX_UTC);
                let banked = |s: &SmartQuotaScore| s.banked_reset_expiry.unwrap_or(DateTime::<Utc>::MAX_UTC);
                order.sort_by(|&x, &y| {
                    let (a, b) = (&scores[x], &scores[y]);
                    a.needs_reserve(reserve)
                        .cmp(&b.needs_reserve(reserve))
                        .then(banked(a).cmp(&banked(b)))
                        .then(renewal(a).cmp(&renewal(b)))
                        .then(b.weight.total_cmp(&a.weight))
                });
            } else {
                let rank = |i: usize| -> f64 {
                    let (a, _) = &first[i];
                    match cfg.routing {
                        Routing::LeastUsed => -least_used_score(a, model),
                        // Turns continue from the next account, wrapping around.
                        Routing::RoundRobin => -(((i + first.len() - next) % first.len()) as f64),
                        Routing::FillFirst | Routing::SmartQuota => -(i as f64),
                    }
                };
                order.sort_by(|&x, &y| rank(y).total_cmp(&rank(x)));
            }
            for i in std::iter::once(next).chain(order) {
                let (a, up) = &first[i];
                let mut step = RouteStep::new(a, up.clone(), "ready", None);
                step.next = i == next;
                step.banked_reset_expires_at = crate::banked_resets::priority_expiry(a, up, cfg);
                steps.push(step);
            }
        }
        for (a, up) in overflow.into_iter().chain(lower) {
            let mut step = RouteStep::new(a, up, "ready", None);
            step.fallback = true;
            steps.push(step);
        }
        rest.sort_by_key(|s| (s.state == "disabled", s.until));
        steps.extend(rest);
        steps
    }
}

/// Where cooldowns are kept between runs, in the auth directory.
pub const COOLDOWNS_FILE: &str = ".cooldowns.state";

/// Rate-limit cooldowns and confirmed quota exhaustion per account and model, so a
/// restart doesn't send requests straight back to an account that is sitting out.
#[derive(Debug, Default, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct Cooldowns(BTreeMap<String, AccountCooldowns>);

#[derive(Debug, Default, Clone, PartialEq, Serialize, serde::Deserialize)]
struct AccountCooldowns {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    rate: BTreeMap<String, DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    quota: BTreeMap<String, DateTime<Utc>>,
}

impl Cooldowns {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        crate::files::write_private(path, &serde_json::to_vec(self)?)
    }
}

impl Pool {
    /// Every cooldown that hasn't run out yet.
    pub fn cooldowns(&self) -> Cooldowns {
        let now = Utc::now();
        let live = |m: &HashMap<String, DateTime<Utc>>| -> BTreeMap<String, DateTime<Utc>> {
            m.iter().filter(|(_, t)| **t > now).map(|(k, t)| (k.clone(), *t)).collect()
        };
        let mut out = BTreeMap::new();
        for a in self.accounts.read().iter() {
            let st = a.state.lock();
            let entry = AccountCooldowns { rate: live(&st.cooldowns), quota: live(&st.quota_cooldowns) };
            if !entry.rate.is_empty() || !entry.quota.is_empty() {
                out.insert(a.id.clone(), entry);
            }
        }
        Cooldowns(out)
    }

    /// Every account's attribution worth saving, and whether any changed since the last call.
    pub fn attributions(&self) -> (crate::attribution::Saved, bool) {
        let mut changed = false;
        let mut out = BTreeMap::new();
        for a in self.accounts.read().iter() {
            let mut st = a.state.lock();
            changed |= std::mem::take(&mut st.attribution.dirty);
            if !st.attribution.current.is_empty() || !st.attribution.previous.is_empty() {
                out.insert(
                    a.id.clone(),
                    crate::attribution::SavedAccount {
                        identity: Some(st.credential_identity.clone()),
                        attribution: st.attribution.for_disk(),
                    },
                );
            }
        }
        (crate::attribution::Saved(out), changed)
    }

    pub fn restore_attributions(&self, saved: crate::attribution::Saved) {
        let mut saved = saved.0;
        for a in self.accounts.read().iter() {
            if let Some(saved) = saved.remove(&a.id) {
                let mut st = a.state.lock();
                if saved.identity.as_ref().is_none_or(|old| old == &st.credential_identity) {
                    st.attribution = saved.attribution;
                    st.attribution.dirty |= saved.identity.is_none();
                } else {
                    // Also remove the stale entry from the next state-file save.
                    st.attribution.dirty = true;
                }
            }
        }
    }

    /// Puts saved cooldowns back, keeping whichever of two deadlines is later.
    pub fn restore_cooldowns(&self, saved: &Cooldowns) {
        let now = Utc::now();
        let merge = |into: &mut HashMap<String, DateTime<Utc>>, from: &BTreeMap<String, DateTime<Utc>>| {
            for (model, until) in from.iter().filter(|(_, t)| **t > now) {
                let slot = into.entry(model.clone()).or_insert(*until);
                *slot = (*slot).max(*until);
            }
        };
        for a in self.accounts.read().iter() {
            if let Some(c) = saved.0.get(&a.id) {
                let mut st = a.state.lock();
                merge(&mut st.cooldowns, &c.rate);
                merge(&mut st.quota_cooldowns, &c.quota);
            }
        }
    }
}

/// One account in a model's route order, for the dashboard.
#[derive(Debug, Clone, Serialize)]
pub struct RouteStep {
    pub account: String,
    pub upstream: String,
    /// "ready", "cooling" or "disabled".
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<DateTime<Utc>>,
    /// An aggregator that only takes over when the vendor's own accounts can't.
    pub fallback: bool,
    /// The account the next new session would use.
    pub next: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banked_reset_expires_at: Option<DateTime<Utc>>,
}

impl RouteStep {
    fn new(a: &Account, upstream: String, state: &'static str, until: Option<DateTime<Utc>>) -> Self {
        Self {
            account: a.id.clone(),
            upstream,
            state,
            until,
            fallback: false,
            next: false,
            banked_reset_expires_at: None,
        }
    }
}

/// Weekly quota (percent left) that smart routing keeps for sessions already on an account.
const SMART_WEEKLY_RESERVE: f64 = 5.0;

/// Smart quota balancing: what reserves and reset priority need, plus the weight that
/// breaks ties between accounts renewing within an hour of each other.
fn smart_score(a: &Account, upstream: &str, cfg: &Config, session_load: &HashMap<String, usize>) -> SmartQuotaScore {
    let banked_reset_expiry = crate::banked_resets::priority_expiry(a, upstream, cfg);
    let st = a.state.lock();
    let remaining = st.quota.five_hour_remaining(upstream);
    // A busy session is already represented by its in-flight request;
    // take the larger count instead of charging that work twice.
    let load = session_load.get(&a.id).copied().unwrap_or(0).max(st.active_requests.load(Ordering::Relaxed));
    // Use known weekly/model-specific headroom when no 5-hour window is
    // reported. The reserve still applies only to real 5-hour data.
    let headroom = remaining.or_else(|| st.quota.remaining(upstream)).unwrap_or(crate::quota::UNKNOWN);
    let headroom = st.quota.limit_remaining(upstream).map_or(headroom, |limit| headroom.min(limit));
    let weekly_remaining = st.quota.weekly_remaining(upstream);
    let weekly = weekly_remaining.map_or(1.0, |left| (left / SMART_WEEKLY_RESERVE).min(1.0));
    SmartQuotaScore {
        five_hour_remaining: remaining,
        weekly_remaining,
        weekly_reset: st.quota.weekly_reset(upstream),
        banked_reset_expiry,
        load,
        weight: headroom * weekly / (1.0 + load as f64),
    }
}

/// Least used: use of the tightest window (lower is better).
fn least_used_score(a: &Account, model: &str) -> f64 {
    a.state.lock().quota.pressure(model).unwrap_or(crate::quota::UNKNOWN)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AuthDirectory(PathBuf);

    impl AuthDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("fusebox-account-identity-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, owner: &str, token: &str, device: Option<&str>) {
            let mut value = serde_json::json!({
                "type": "claude", "account_uuid": owner, "email": "fake@example.test",
                "access_token": token, "refresh_token": "FAKE_REFRESH",
            });
            if let Some(device) = device {
                value["claude_device_ids"] = serde_json::json!([device]);
            }
            std::fs::write(self.0.join("same-filename.json"), value.to_string()).unwrap();
        }
    }

    impl Drop for AuthDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn same_filename_identity_replacement_rotates_devices_and_sessions_but_keeps_accounting() {
        let directory = AuthDirectory::new();
        let imported_device = "a".repeat(64);
        directory.write("fake-owner-a", "FAKE_ACCESS_A", Some(&imported_device));
        let cfg = Config { auth_dir: directory.0.to_string_lossy().into(), ..Default::default() };
        let pool = Pool::default();
        pool.reload(&cfg);
        let original = pool.all().pop().unwrap();
        let identity = original.state.lock().credential_identity.clone();
        original.state.lock().counters.requests = 17;
        original.discovered.write().push("claude-fake-model".into());
        let load = RequestLoad::new(&original);
        let epoch = original.state.lock().identity_epoch;
        directory.write("fake-owner-b", "FAKE_ACCESS_B", Some(&imported_device));
        pool.reload(&cfg);
        let replacement = pool.all().pop().unwrap();
        assert_eq!(replacement.id, original.id);
        assert!(!Arc::ptr_eq(&original, &replacement));
        assert!(!Arc::ptr_eq(&original.cred, &replacement.cred));
        assert!(!Arc::ptr_eq(&original.refresh_lock, &replacement.refresh_lock));
        assert_ne!(replacement.device_id, original.device_id);
        assert_ne!(replacement.session_id, original.session_id);
        assert_eq!(replacement.device_id.len(), 64);
        assert_eq!(replacement.state.lock().identity_epoch, epoch + 1);
        assert_eq!(original.state.lock().identity_epoch, epoch + 1, "old sessions are invalidated");
        assert_ne!(replacement.state.lock().credential_identity, identity);
        assert_eq!(replacement.state.lock().counters.requests, 17);
        assert_eq!(replacement.state.lock().active_requests.load(Ordering::Relaxed), 1);
        assert!(replacement.discovered.read().is_empty());
        let Credential::OAuth(new) = replacement.cred.read().clone() else { unreachable!() };
        assert_eq!(new.access_token, "FAKE_ACCESS_B");
        let Credential::OAuth(old) = original.cred.read().clone() else { unreachable!() };
        assert_eq!(old.access_token, "FAKE_ACCESS_A", "old work cannot refresh the replacement's credentials");
        drop(load);
        assert_eq!(replacement.state.lock().active_requests.load(Ordering::Relaxed), 0);
        pool.reload(&cfg);
        assert!(Arc::ptr_eq(&replacement, &pool.all()[0]));
        let fresh_pool = Pool::default();
        fresh_pool.reload(&cfg);
        assert_eq!(fresh_pool.all()[0].device_id, replacement.device_id, "restart retains the replacement's device");
    }

    #[test]
    fn token_and_configuration_reload_preserve_account_device_session_and_epoch() {
        let directory = AuthDirectory::new();
        directory.write("fake-owner", "FAKE_ACCESS_OLD", None);
        let mut cfg = Config { auth_dir: directory.0.to_string_lossy().into(), ..Default::default() };
        let pool = Pool::default();
        pool.reload(&cfg);
        let original = pool.all().pop().unwrap();
        let epoch = original.state.lock().identity_epoch;
        directory.write("fake-owner", "FAKE_ACCESS_NEW", None);
        pool.reload(&cfg);
        assert!(Arc::ptr_eq(&original, &pool.all()[0]));
        cfg.oauth_excluded_models.insert("claude".into(), vec!["claude-excluded".into()]);
        pool.reload(&cfg);
        let updated = pool.all().pop().unwrap();
        assert!(!Arc::ptr_eq(&original, &updated));
        assert!(Arc::ptr_eq(&original.cred, &updated.cred));
        assert_eq!(updated.device_id, original.device_id);
        assert_eq!(updated.session_id, original.session_id);
        assert_eq!(updated.state.lock().identity_epoch, epoch);
        let Credential::OAuth(credential) = updated.cred.read().clone() else { unreachable!() };
        assert_eq!(credential.access_token, "FAKE_ACCESS_NEW");
    }

    #[test]
    fn errors_that_explain_a_pause_end_with_it() {
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            claude_api_key: vec![crate::config::KeyEntry { api_key: "one".into(), ..Default::default() }],
            ..Default::default()
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        let acct = &pool.all()[0];
        let (model, now) = ("claude-sonnet-5-5", Utc::now());
        let at = |t: DateTime<Utc>| acct.state.lock().last_error(t).map(String::from);
        let later = now + chrono::Duration::hours(1);
        acct.exhaust(model, later, "subscription quota exhausted: try again later");
        assert_eq!(acct.snapshot()["last_error"], "subscription quota exhausted: try again later");
        assert_eq!(at(later), None, "the error ends when the limit renews");
        acct.rate_limit(model, now + chrono::Duration::seconds(15), "429: slow down");
        assert_eq!(at(now).as_deref(), Some("429: slow down"));
        assert_eq!(at(now + chrono::Duration::seconds(15)), None);
        let until = now + chrono::Duration::seconds(30);
        let cooldowns = acct.snapshot()["cooldowns"].clone();
        acct.state.lock().set_temporary_error("529: Overloaded", until);
        assert_eq!(at(now).as_deref(), Some("529: Overloaded"));
        assert_eq!(acct.snapshot()["last_error_until"], serde_json::json!(until));
        assert_eq!(at(until), None, "an idle account does not need another inference to clear an overload");
        assert_eq!(acct.snapshot()["cooldowns"], cooldowns, "the error deadline does not change routing pauses");
        acct.state.lock().set_temporary_error("529: Overloaded", now - chrono::Duration::seconds(1));
        assert_eq!(acct.snapshot()["last_error"], Value::Null);
        assert_eq!(acct.snapshot()["last_error_until"], Value::Null);
        // Problems that may need attention stay until a request succeeds.
        acct.cool(None, now + chrono::Duration::minutes(10), "403: forbidden");
        assert_eq!(at(later).as_deref(), Some("403: forbidden"));
        acct.state.lock().set_error("500: upstream exploded");
        assert_eq!(at(later).as_deref(), Some("500: upstream exploded"));
        acct.record_ok();
        assert_eq!(acct.snapshot()["last_error"], Value::Null);
    }

    #[test]
    fn route_order_lists_every_account_without_taking_a_turn() {
        let model = "claude-sonnet-4-6";
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            claude_api_key: ["one", "two", "three"]
                .map(|key| crate::config::KeyEntry { api_key: key.into(), ..Default::default() })
                .to_vec(),
            routing: Routing::RoundRobin,
            ..Default::default()
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        let ids: Vec<String> = pool.all().iter().map(|a| a.id.clone()).collect();
        let order = |pool: &Pool| -> Vec<(String, &'static str, bool)> {
            pool.route_order(model, &cfg, None, &HashMap::new())
                .into_iter()
                .map(|s| (s.account, s.state, s.next))
                .collect()
        };
        let ready = |i: usize, next| (ids[i].clone(), "ready", next);
        assert_eq!(order(&pool), [ready(0, true), ready(1, false), ready(2, false)]);
        // Looking doesn't take the turn; picking does, and the order follows it.
        assert_eq!(order(&pool), [ready(0, true), ready(1, false), ready(2, false)]);
        assert!(matches!(pool.pick(model, &[], &cfg, None, None, &HashMap::new()), Pick::Ok(a, _) if a.id == ids[0]));
        assert_eq!(order(&pool), [ready(1, true), ready(2, false), ready(0, false)]);
        // Cooling accounts follow the ready ones; disabled accounts come last.
        pool.get(&ids[2]).unwrap().state.lock().disabled = true;
        pool.get(&ids[0]).unwrap().cool(Some(model), Utc::now() + chrono::Duration::minutes(5), "429");
        assert_eq!(
            order(&pool),
            [ready(1, true), (ids[0].clone(), "cooling", false), (ids[2].clone(), "disabled", false)]
        );
    }

    #[test]
    fn smart_quota_prefers_earlier_renewals_and_respects_availability() {
        use crate::quota::{Quota, Window};
        use chrono::Duration;
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            claude_api_key: ["early", "middle", "late"]
                .map(|key| crate::config::KeyEntry { api_key: key.into(), ..Default::default() })
                .to_vec(),
            ..Default::default()
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        let accounts = pool.accounts.read().clone();
        let now = Utc::now();
        for (account, (hours, used)) in accounts.iter().zip([(82, 33.0), (104, 71.0), (129, 16.0)]) {
            account.state.lock().quota = Quota {
                windows: vec![
                    Window { name: "week".into(), used, resets_at: Some(now + Duration::hours(hours)), model: None },
                    Window { name: "5h".into(), used: 0.0, resets_at: Some(now + Duration::hours(1)), model: None },
                ],
                updated_at: Some(now),
                ..Default::default()
            };
        }
        let pick = |routing, exclude: &[String], pinned| match pool.pick(
            "claude-sonnet-4-6",
            exclude,
            &Config { routing, ..cfg.clone() },
            pinned,
            None,
            &HashMap::new(),
        ) {
            Pick::Ok(a, _) => a.id.clone(),
            _ => panic!("expected available account"),
        };
        let strategy = Routing::SmartQuota;
        assert_eq!(pick(Routing::LeastUsed, &[], None), accounts[2].id);
        for _ in 0..4 {
            assert_eq!(pick(strategy, &[], None), accounts[0].id);
        }
        assert_eq!(pick(strategy, &[], Some(accounts[2].id.as_str())), accounts[2].id);
        assert_eq!(pick(strategy, &[accounts[0].id.clone()], None), accounts[1].id);
        accounts[0].state.lock().disabled = true;
        assert_eq!(pick(strategy, &[], None), accounts[1].id);
        accounts[0].state.lock().disabled = false;
        // A 5-hour limit blocks selection even when the weekly reset is earliest.
        accounts[0].state.lock().quota.windows[1].used = 100.0;
        assert_eq!(pick(strategy, &[], None), accounts[1].id);
        accounts[0].state.lock().quota.windows[1].used = 0.0;
        accounts[0].state.lock().quota.windows[0].used = 100.0;
        assert_eq!(pick(strategy, &[], None), accounts[1].id);
        accounts[0].state.lock().quota.windows[0].used = 33.0;
        // Past and unknown weekly resets must not beat a known future reset.
        accounts[0].state.lock().quota.windows[0].resets_at = Some(now - Duration::hours(1));
        accounts[2].state.lock().quota.windows[0].resets_at = None;
        assert_eq!(pick(strategy, &[], None), accounts[1].id);
        // Equal dates break ties by 5-hour headroom.
        accounts[1].state.lock().quota.windows[1].used = 10.0;
        accounts[0].state.lock().quota.windows[0].resets_at = Some(now + Duration::hours(104));
        assert_eq!(pick(strategy, &[], None), accounts[0].id);
        // Without weekly dates, use 5-hour headroom instead.
        accounts[0].state.lock().quota.windows[1].used = 20.0;
        for account in &accounts {
            account.state.lock().quota.windows[0].resets_at = None;
        }
        assert_eq!(pick(strategy, &[], None), accounts[2].id);
    }

    #[test]
    fn smart_quota_protects_busy_accounts_nearly_out_of_their_week() {
        use crate::quota::{Quota, Window};
        use chrono::Duration;
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            claude_api_key: ["a", "b"]
                .map(|key| crate::config::KeyEntry { api_key: key.into(), ..Default::default() })
                .to_vec(),
            routing: Routing::SmartQuota,
            ..Default::default()
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        let accounts = pool.accounts.read().clone();
        let now = Utc::now();
        // An earlier renewal must not spend the reserve of an existing session.
        for (account, (days, used)) in accounts.iter().zip([(2, 98.0), (6, 0.0)]) {
            account.state.lock().quota = Quota {
                windows: vec![
                    Window { name: "week".into(), used, resets_at: Some(now + Duration::days(days)), model: None },
                    Window { name: "5h".into(), used: 0.0, resets_at: Some(now + Duration::hours(1)), model: None },
                ],
                updated_at: Some(now),
                ..Default::default()
            };
        }
        let load = HashMap::from([(accounts[0].id.clone(), 1)]);
        for _ in 0..4 {
            match pool.pick("claude-sonnet-4-6", &[], &cfg, None, None, &load) {
                Pick::Ok(a, _) => assert_eq!(a.id, accounts[1].id),
                _ => panic!("expected available account"),
            }
        }
    }

    #[test]
    fn smart_quota_packs_earlier_renewals_until_quota_is_low() {
        use crate::quota::{Quota, Window};
        use chrono::Duration;
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            claude_api_key: ["early", "middle", "late"]
                .map(|key| crate::config::KeyEntry { api_key: key.into(), ..Default::default() })
                .to_vec(),
            routing: Routing::SmartQuota,
            ..Default::default()
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        let accounts = pool.all();
        let now = Utc::now();
        for (account, (hours, left)) in accounts.iter().zip([(57, 60.0), (79, 16.0), (104, 84.0)]) {
            account.state.lock().quota = Quota {
                windows: vec![
                    Window { name: "5h".into(), used: 0.0, resets_at: Some(now + Duration::hours(4)), model: None },
                    Window {
                        name: "week".into(),
                        used: 100.0 - left,
                        resets_at: Some(now + Duration::hours(hours)),
                        model: None,
                    },
                ],
                updated_at: Some(now),
                ..Default::default()
            };
        }
        let load =
            HashMap::from([(accounts[0].id.clone(), 30), (accounts[1].id.clone(), 1), (accounts[2].id.clone(), 1)]);
        let pick = || match pool.pick("claude-opus-5-5", &[], &cfg, None, None, &load) {
            Pick::Ok(a, _) => a.id.clone(),
            _ => panic!("expected available account"),
        };
        assert_eq!(pick(), accounts[0].id); // even many sessions do not outweigh an earlier reset
        let five_hour = accounts[0].state.lock().quota.windows[0].clone();
        for a in &accounts {
            a.state.lock().quota.windows.remove(0);
        }
        assert_eq!(pick(), accounts[0].id); // weekly-only quota keeps the same reset priority
        for a in &accounts {
            a.state.lock().quota.windows.insert(0, five_hour.clone());
        }
        accounts[0].state.lock().quota.windows[0].used = 70.0;
        accounts[0].state.lock().quota.windows[1].used = 95.0;
        assert_eq!(pick(), accounts[0].id); // exactly the reserves is still healthy
        accounts[0].state.lock().quota.windows[0].used = 71.0;
        assert_eq!(pick(), accounts[1].id); // the middle renewal is next despite only 16% of its week left
        let order: Vec<_> =
            pool.route_order("claude-opus-5-5", &cfg, None, &load).into_iter().map(|s| s.account).collect();
        assert_eq!(order, [accounts[1].id.clone(), accounts[2].id.clone(), accounts[0].id.clone()]); // accounts keeping a reserve are listed last
        accounts[0].state.lock().quota.windows[0].used = 0.0;
        accounts[0].state.lock().quota.windows[1].used = 96.0;
        assert_eq!(pick(), accounts[1].id); // protect the early account's remaining weekly quota
        accounts[1].state.lock().quota.windows[1].used = 96.0;
        assert_eq!(pick(), accounts[2].id); // the latest renewal is last
        for a in &accounts {
            a.state.lock().quota.windows[1].used = 96.0;
        }
        assert_ne!(pick(), accounts[0].id); // all low: quota and load decide, without refusing requests
    }

    #[test]
    fn smart_quota_uses_load_for_close_or_unknown_renewals() {
        use crate::quota::{Quota, Window};
        use chrono::Duration;
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            claude_api_key: ["a", "b"]
                .map(|key| crate::config::KeyEntry { api_key: key.into(), ..Default::default() })
                .to_vec(),
            routing: Routing::SmartQuota,
            ..Default::default()
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        let accounts = pool.all();
        let now = Utc::now();
        for (a, minutes) in accounts.iter().zip([0, 30]) {
            a.state.lock().quota = Quota {
                windows: vec![
                    Window { name: "5h".into(), used: 0.0, resets_at: Some(now + Duration::hours(4)), model: None },
                    Window {
                        name: "week".into(),
                        used: 20.0,
                        resets_at: Some(now + Duration::days(2) + Duration::minutes(minutes)),
                        model: None,
                    },
                ],
                updated_at: Some(now),
                ..Default::default()
            };
        }
        let load = HashMap::from([(accounts[0].id.clone(), 1)]);
        let pick = || match pool.pick("claude-opus-5-5", &[], &cfg, None, None, &load) {
            Pick::Ok(a, _) => a.id.clone(),
            _ => panic!("expected available account"),
        };
        assert_eq!(pick(), accounts[1].id); // half an hour is a close tie
        accounts[1].state.lock().quota.windows[1].resets_at = Some(now + Duration::days(2) + Duration::hours(1));
        assert_eq!(pick(), accounts[1].id); // the one-hour boundary is included
        accounts[1].state.lock().quota.windows[1].resets_at =
            Some(now + Duration::days(2) + Duration::hours(1) + Duration::seconds(1));
        assert_eq!(pick(), accounts[0].id); // beyond the tie, reset priority wins
        accounts[1].state.lock().quota.windows[1].resets_at = None;
        assert_eq!(pick(), accounts[0].id); // unknown renewal cannot outrank a known healthy reset
        accounts[0].state.lock().quota.windows[1].resets_at = Some(now - Duration::seconds(1));
        assert_eq!(pick(), accounts[1].id); // without a future renewal, load still spreads work
    }

    #[test]
    fn request_load_survives_account_reload_and_is_released() {
        let mut cfg = Config {
            auth_dir: "/nonexistent".into(),
            codex_api_key: vec![crate::config::KeyEntry { api_key: "a".into(), ..Default::default() }],
            ..Default::default()
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        let old = pool.all()[0].clone();
        let request = RequestLoad::new(&old);
        cfg.codex_api_key[0].headers.insert("test-header".into(), "value".into());
        pool.reload(&cfg); // changed shape replaces the Arc<Account>
        let new = pool.all()[0].clone();
        assert!(!Arc::ptr_eq(&old, &new));
        assert_eq!(new.state.lock().active_requests.load(Ordering::Relaxed), 1);
        drop(request);
        assert_eq!(new.state.lock().active_requests.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn aggregator_names_stay_with_aggregators() {
        assert!(Provider::Gemini.serves("gemini-3.8-flash"));
        assert!(!Provider::Gemini.serves("gemini-3.8-flash-high"));
        assert!(!Provider::Vertex.serves("gemini-pro-agent"));
        assert!(Provider::Antigravity.serves("gemini-3.8-flash-high"));
        assert!(!Provider::Codex.serves("gpt-5-6-sol"));
        assert!(Provider::Devin.serves("gpt-5-6-sol"));
        // Shared names belong to the vendor first.
        assert!(Provider::Claude.serves("claude-sonnet-4-6"));
        assert!(!Provider::Antigravity.first_party("claude-sonnet-4-6"));
        assert!(Provider::Antigravity.first_party("gemini-3.8-flash-high"));
        assert!(Provider::Kimi.serves("kimi-k3") && !Provider::Devin.first_party("kimi-k3"));
    }

    #[test]
    fn provider_prefix_routing() {
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            claude_api_key: vec![crate::config::KeyEntry { api_key: "k".into(), ..Default::default() }],
            ..Default::default()
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        let claude = Only::Provider(Provider::Claude);
        assert_eq!(pool.route("claude/claude-opus-5-5"), (Some(claude.clone()), "claude-opus-5-5".into()));
        assert_eq!(pool.route("moonshotai/kimi-k3"), (None, "moonshotai/kimi-k3".into()));
        assert!(matches!(pool.pick("claude-opus-5-5", &[], &cfg, None, Some(&claude), &HashMap::new()), Pick::Ok(..)));
        let codex = Only::Provider(Provider::Codex);
        assert!(matches!(pool.pick("claude-opus-5-5", &[], &cfg, None, Some(&codex), &HashMap::new()), Pick::None));
    }

    #[test]
    fn added_models_are_served_and_listed_until_fusebox_knows_them() {
        let text = r#"
auth-dir: /nonexistent
claude-api-key:
  - api-key: k
extra-models:
  claude: [mythos-1, claude-opus-5-5]
  codex: [gpt-7]
"#;
        let cfg = Config::parse(text).unwrap();
        let pool = Pool::default();
        pool.reload(&cfg);
        // Even a name outside the claude-* family reaches the provider it was added for.
        assert!(matches!(pool.pick("mythos-1", &[], &cfg, None, None, &HashMap::new()), Pick::Ok(..)));
        assert!(matches!(pool.pick("gpt-7", &[], &cfg, None, None, &HashMap::new()), Pick::None));
        let models: Vec<String> = pool.models().into_iter().map(|(m, _)| m).collect();
        assert!(models.contains(&"mythos-1".to_string()) && !models.contains(&"gpt-7".to_string()));
        assert!(pool.knows(Provider::Claude, "claude-opus-5-5") && pool.knows(Provider::Claude, "CLAUDE-OPUS-5.5"));
        assert!(!pool.knows(Provider::Claude, "mythos-1") && !pool.knows(Provider::Codex, "mythos-1"));
        pool.all()[0].discovered.write().push("mythos-1".into());
        assert!(pool.knows(Provider::Claude, "mythos-1"));
    }

    #[test]
    fn listing_checks_agree_with_the_public_model_lists() {
        let text = r#"
auth-dir: /nonexistent
claude-api-key:
  - api-key: k
codex-api-key:
  - api-key: k2
    excluded-models: [gpt-6-*]
openai-compatibility:
  - name: local
    base-url: http://127.0.0.1:1/v1
    models:
      - name: qwen3-coder:30b
        alias: qwen
      - name: hidden-model
    excluded-models: [hidden-*]
extra-models:
  claude: [mythos-1, CLAUDE-OPUS-5-5]
oauth-model-alias:
  claude:
    - name: claude-sonnet-5-5
      alias: sonnet
"#;
        let cfg = Config::parse(text).unwrap();
        let pool = Pool::default();
        pool.reload(&cfg);
        pool.all()[0].discovered.write().extend(["Claude-Opus-5-5".to_string(), "discovered-1".to_string()]);
        for acct in pool.all() {
            let listed = acct.public_models();
            let mut names: Vec<String> = listed.clone();
            names.extend(listed.iter().map(|m| m.to_ascii_uppercase()));
            names.extend(
                ["sonnet", "claude-sonnet-5-5", "gpt-6-astra", "qwen", "hidden-model", "mythos-1", "nothing", ""]
                    .map(String::from),
            );
            for name in names {
                assert_eq!(
                    acct.lists(&name),
                    listed.iter().any(|m| m.eq_ignore_ascii_case(&name)),
                    "{} lists {name}",
                    acct.label
                );
            }
        }
    }

    #[test]
    fn close_model_names_resolve() {
        assert_eq!(dot_versions("gemini-3-8-flash"), "gemini-3.8-flash");
        assert_eq!(dot_versions("gpt-6-1-sol"), "gpt-6.1-sol");
        assert_eq!(dot_versions("claude-haiku-4-5-20251001"), "claude-haiku-4.5-20251001");
        assert_eq!(dot_versions("gpt-5.5"), "gpt-5.5");
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            codex_api_key: vec![crate::config::KeyEntry { api_key: "k".into(), ..Default::default() }],
            ..Default::default()
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        assert_eq!(pool.canonical("gpt-6-1-sol", None), "gpt-6.1-sol");
        assert_eq!(pool.canonical("gpt-6-1-terra", None), "gpt-6.1-terra");
        assert_eq!(pool.canonical("openai/gpt-6-astra", None), "gpt-6-astra");
        assert_eq!(pool.canonical("nothing-like-it", None), "nothing-like-it");
    }

    #[test]
    fn wonder_full_defaults_to_subscription_accounts_and_respects_explicit_models() {
        let mut cfg = Config::parse("auth-dir: /nonexistent\nmeta-api-key:\n  - api-key: synthetic-meta\n").unwrap();
        let pool = Pool::default();
        pool.reload(&cfg);
        let acct = pool.all().remove(0);
        assert!(acct.lists("muse-wonder-1.0"));
        assert!(acct.resolve("muse-wonder-1.0").is_some());
        assert!(!acct.lists("muse-wonder-full-1.0"));
        assert!(!acct.public_models().iter().any(|m| m == "muse-wonder-full-1.0"));
        assert!(acct.resolve_with("muse-wonder-full-1.0", true).is_none());
        // Canonicalization probes effort suffixes for aggregator models. Those
        // probes must not turn a denied Full request into a synthetic model ID.
        assert_eq!(pool.canonical("muse-wonder-full-1.0", None), "muse-wonder-full-1.0");
        assert!(acct.resolve("muse-wonder-full-1.0-high").is_none());
        assert!(matches!(pool.pick("muse-wonder-full-1.0", &[], &cfg, None, None, &HashMap::new()), Pick::None));

        *acct.cred.write() =
            Credential::OAuth(OAuth { access_token: "synthetic-subscription".into(), ..Default::default() });
        assert!(acct.lists("MUSE-WONDER-FULL-1.0"));
        assert!(acct.public_models().iter().any(|m| m == "muse-wonder-full-1.0"));
        assert!(matches!(pool.pick("muse-wonder-full-1.0", &[], &cfg, None, None, &HashMap::new()), Pick::Ok(..)));

        cfg.meta_api_key[0].models.push(ModelAlias { name: "muse-wonder-full-1.0".into(), alias: None });
        pool.reload(&cfg);
        let acct = pool.all().remove(0);
        assert!(!acct.is_oauth());
        assert!(acct.lists("muse-wonder-full-1.0"));
        assert!(acct.resolve("muse-wonder-full-1.0").is_some());
    }

    #[test]
    fn new_provider_models_keep_aggregator_variants_on_their_provider() {
        for (provider, model) in [
            (Provider::Codex, "gpt-6.1-terra"),
            (Provider::Codex, "gpt-image-2.5-sunburst"),
            (Provider::Codex, "gpt-image-2.5-flare"),
            (Provider::Claude, "claude-haiku-5-5"),
            (Provider::Gemini, "gemini-4-argon"),
            (Provider::Vertex, "gemini-4-argon"),
            (Provider::Kimi, "kimi-k3.1-pro"),
            (Provider::Kimi, "kimi-k3.1-pro-highspeed"),
            (Provider::Xai, "grok-imagine-video-1.5-lite"),
            (Provider::Devin, "gpt-6-1-sol"),
            (Provider::Devin, "claude-haiku-5-5"),
            (Provider::Antigravity, "gemini-4-argon-high"),
            (Provider::Antigravity, "claude-sonnet-5-5-max"),
        ] {
            assert!(provider.serves(model), "{} serves {model}", provider.as_str());
        }
        assert!(!Provider::Codex.serves("gpt-6-1-sol"));
        assert!(!Provider::Codex.serves("gemini-4-argon-high"));
        assert!(!Provider::Gemini.serves("gemini-4-argon-high"));
        assert!(!Provider::Claude.serves("claude-sonnet-5-5-max"));
        assert!(!Provider::Antigravity.family("gpt-6.1-terra"));
        assert!(!Provider::Devin.family("gpt-6.1-terra"));
        assert!(!Provider::Devin.first_party("claude-haiku-5-5"));
    }

    #[test]
    fn wildcard_boundaries_preserve_ascii_case_and_unicode() {
        for (pattern, value, expected) in [
            ("", "", true),
            ("  ", "", true),
            ("*", "", true),
            ("***", "anything", true),
            ("abc*", "ab", false),
            ("*abc", "bc", false),
            ("a*b", "ab", true),
            ("a*a", "a", false),
            ("a**B*", "aB", true),
            ("*ab*ab", "ab", false),
            ("*ab*ab", "abab", true),
            ("  GPT-*SOL  ", "gpt-6.1-sol", true),
            ("gpt-*", " gpt-sol", false),
            ("*é*B", "AéB", true),
            ("*É*", "aéb", false),
            ("*🦀b", "a🦀B", true),
        ] {
            assert_eq!(wildcard(pattern, value), expected, "{pattern:?} vs {value:?}");
        }
    }

    #[test]
    fn wildcard_matches_reference_for_small_patterns() {
        fn reference(pattern: &str, value: &str) -> bool {
            let (pattern, value) = (pattern.trim().to_ascii_lowercase(), value.to_ascii_lowercase());
            let parts: Vec<_> = pattern.split('*').collect();
            if parts.len() == 1 {
                return pattern == value;
            }
            let mut rest = value.as_str();
            for (i, part) in parts.iter().enumerate() {
                if part.is_empty() {
                    continue;
                }
                if i == 0 {
                    let Some(next) = rest.strip_prefix(part) else { return false };
                    rest = next;
                } else if i + 1 == parts.len() {
                    return rest.ends_with(part);
                } else {
                    let Some(at) = rest.find(part) else { return false };
                    rest = &rest[at + part.len()..];
                }
            }
            true
        }
        fn words(depth: usize) -> Vec<String> {
            let mut result = vec![String::new()];
            let mut level = vec![String::new()];
            for _ in 0..depth {
                level =
                    level.iter().flat_map(|prefix| ["a", "B", "é", "*", " "].map(|c| format!("{prefix}{c}"))).collect();
                result.extend(level.iter().cloned());
            }
            result
        }
        let values = words(3);
        for pattern in words(4) {
            for value in &values {
                assert_eq!(wildcard(&pattern, value), reference(&pattern, value), "{pattern:?} vs {value:?}");
            }
        }
    }

    #[test]
    fn prefixes_exclusions_and_aliases() {
        assert!(wildcard("gemini-2.5-*", "gemini-2.5-pro"));
        assert!(wildcard("*-preview", "gemini-3-pro-preview"));
        assert!(wildcard("*flash*", "gemini-2.5-flash-lite"));
        assert!(!wildcard("*flash*", "gemini-2.5-pro"));
        assert!(wildcard("claude-opus-5-5", "CLAUDE-OPUS-5-5"));

        let text = r#"
auth-dir: /nonexistent
force-model-prefix: true
claude-api-key:
  - api-key: team-key
    prefix: team
  - api-key: plain-key
    excluded-models: ["claude-opus-*"]
"#;
        let cfg = Config::parse(text).unwrap();
        let pool = Pool::default();
        pool.reload(&cfg);
        let team = Only::Prefix("team".into());
        assert_eq!(pool.route("team/claude-opus-5-5"), (Some(team.clone()), "claude-opus-5-5".into()));
        let Pick::Ok(a, _) = pool.pick("claude-opus-5-5", &[], &cfg, None, Some(&team), &HashMap::new()) else {
            panic!()
        };
        assert_eq!(a.prefix.as_deref(), Some("team"));
        // Unprefixed: the team key is reserved and the plain key excludes opus.
        assert!(matches!(pool.pick("claude-opus-5-5", &[], &cfg, None, None, &HashMap::new()), Pick::None));
        assert!(matches!(pool.pick("claude-sonnet-5-5", &[], &cfg, None, None, &HashMap::new()), Pick::Ok(..)));
        let models: Vec<String> = pool.models().into_iter().map(|(m, _)| m).collect();
        assert!(models.contains(&"team/claude-opus-5-5".to_string()));
        assert!(!models.contains(&"claude-opus-5-5".to_string()));
    }

    #[test]
    fn oauth_aliases_rename_models() {
        let a = |fork| crate::config::OAuthAlias { name: "claude-opus-5-5".into(), alias: "opus".into(), fork };
        let acct = |aliases| Account {
            id: "x".into(),
            provider: Provider::Claude,
            label: "x".into(),
            path: None,
            group: None,
            models: vec![],
            headers: BTreeMap::new(),
            proxy_url: None,
            cred: Arc::new(RwLock::new(Credential::ApiKey { key: "k".into(), base_url: None })),
            state: Arc::new(Mutex::new(AccountState::default())),
            refresh_lock: Arc::default(),
            pending_oauth_save: Arc::default(),
            device_id: String::new(),
            session_id: String::new(),
            discovered: RwLock::new(vec![]),
            extra: vec![],
            prefix: None,
            excluded: vec![],
            aliases,
            priority: 0,
        };
        let renamed = acct(vec![a(false)]);
        assert_eq!(renamed.resolve("opus").as_deref(), Some("claude-opus-5-5"));
        assert!(renamed.resolve("claude-opus-5-5").is_none());
        assert!(renamed.public_models().contains(&"opus".to_string()));
        let forked = acct(vec![a(true)]);
        assert_eq!(forked.resolve("claude-opus-5-5").as_deref(), Some("claude-opus-5-5"));
    }
}
