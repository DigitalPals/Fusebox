//! Request pipeline: pick an account, translate, send, retry, stream back.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use axum::http::HeaderMap;
use chrono::{Duration, Utc};
use futures::{Stream, StreamExt};
use serde_json::{Value, json};

use crate::accounts::{Account, Pick, Provider};
use crate::formats::{self, Frame};
use crate::ir::{self, Aggregate, Event, Format, Reasoning, Request, Usage};
use crate::sse::SseDecoder;
use crate::state::{App, RequestLog, RoutingAttempt};
use crate::upstream::{self, Target};

pub type FrameStream = Pin<Box<dyn Stream<Item = Frame> + Send>>;

#[cfg(test)]
#[path = "diagnostic_tests.rs"]
mod diagnostic_tests;
#[cfg(test)]
#[path = "stream_accounting_tests.rs"]
mod stream_accounting_tests;

pub struct Call {
    pub format: Format,
    pub body: Value,
    pub headers: HeaderMap,
    pub stream: bool,
    pub transport: &'static str,
    /// Model from the URL (Gemini routes).
    pub path_model: Option<String>,
    /// Shared coding-session identity (inferred from the original request for HTTP).
    pub session: Option<String>,
    pub session_source: Option<&'static str>,
    /// A native WebSocket selection made before falling back to this pipeline.
    pub routing_selection: Option<crate::affinity::Selected>,
}

pub enum Reply {
    Stream { frames: FrameStream, account: String },
    Json(Value),
    Error(u16, Value),
}

// --------------------------------------------------------------------- tracker

/// Records the request in stats when finished (or dropped mid-stream).
pub struct Tracker {
    app: Arc<App>,
    log: RequestLog,
    started: Instant,
    acct: Option<Arc<Account>>,
    /// Stream progress must outlive the suspended generator when its body is dropped.
    stream_usage: Usage,
    stream_error: Option<(u16, String)>,
    stream_finished: bool,
    load: Option<crate::accounts::RequestLoad>,
    done: bool,
    analytics: Option<crate::usage::capture::RequestUsage>,
    downstream: crate::diagnostics::Downstream,
    logical_final: bool,
}

impl Tracker {
    pub fn new(app: &Arc<App>, client: Format, stream: bool, transport: &'static str, model: &str) -> Self {
        app.stats.active.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            app: app.clone(),
            analytics: Some(crate::usage::capture::RequestUsage::new(app.usage.clone(), model)),
            downstream: crate::diagnostics::Downstream::current(),
            logical_final: true,
            started: Instant::now(),
            acct: None,
            stream_usage: Usage::default(),
            stream_error: None,
            stream_finished: false,
            load: None,
            done: false,
            log: RequestLog {
                id: app.stats.next_id(),
                ts: Utc::now(),
                client: client.as_str(),
                client_app: None,
                provider: String::new(),
                model: model.to_string(),
                account: String::new(),
                account_id: String::new(),
                session_id: None,
                session_source: None,
                routing_strategy: app.cfg().routing,
                routing_reason: None,
                routing_warning: None,
                routing_attempts: Vec::new(),
                cache_policy: None,
                status: 0,
                latency_ms: 0,
                ttft_ms: None,
                input_tokens: 0,
                output_tokens: 0,
                cache_tokens: 0,
                stream,
                transport,
                attempts: 0,
                error: None,
                usage_completeness: "missing",
                failure_kind: None,
            },
        }
    }

    pub fn attempt(&mut self, acct: &Arc<Account>) {
        if let Some(a) = &mut self.analytics {
            a.attempt(acct);
        }
        self.load = Some(crate::accounts::RequestLoad::new(acct));
        self.log.attempts += 1;
        self.log.provider = acct.provider.as_str().to_string();
        self.log.account = acct.label.clone();
        self.log.account_id = acct.id.clone();
        self.acct = Some(acct.clone());
    }

    /// Names the client program from its User-Agent, for the dashboard.
    pub fn client_app(&mut self, headers: &HeaderMap) {
        self.log.client_app = client_app(headers);
        if let Some(a) = &mut self.analytics {
            a.client(headers);
            a.program(self.log.client_app);
            if crate::policy::named_client(&self.app.cfg(), headers).is_some() {
                a.track_limits(self.app.clients.clone());
            }
        }
    }

    pub fn session(&mut self, key: Option<&str>, source: Option<&'static str>, cfg: &crate::config::Config) {
        self.log.session_id = key.map(String::from);
        if let Some(a) = &mut self.analytics {
            a.session(key);
        }
        self.log.session_source = source;
        self.log.routing_strategy = cfg.routing;
        self.log.routing_warning = if !cfg.session_affinity {
            Some("affinity_disabled")
        } else {
            match source {
                None | Some("generated_response") => Some("missing_session_id"),
                Some(crate::affinity::INFERRED_SOURCE) => Some("inferred_session"),
                Some("websocket_connection") => Some("connection_only"),
                Some("previous_response_id") => Some("response_id_only"),
                _ => None,
            }
        };
        if self.log.routing_warning == Some("missing_session_id") {
            // Once per program and transport every ten minutes; the request log marks each one.
            let key = format!("session:{}:{}", self.log.client_app.unwrap_or("unknown"), self.log.transport);
            if crate::diagnostics::warn_once(&key, std::time::Duration::from_secs(600)) {
                tracing::warn!(
                    request_id = self.log.id,
                    transport = self.log.transport,
                    "request has no stable session identifier; send x-fusebox-session-id to preserve its subscription across requests (repeats are logged at debug for ten minutes)"
                );
            } else {
                tracing::debug!(
                    request_id = self.log.id,
                    transport = self.log.transport,
                    "request has no stable session identifier"
                );
            }
        }
    }

    pub fn selected(&mut self, selected: &crate::affinity::Selected) {
        // Preserve the initial allocation and any migration even if later retries reuse it.
        if self.log.routing_reason.is_none()
            || !matches!(selected.reason, "new_session" | "session_reused" | "retry_same")
        {
            self.log.routing_reason = Some(selected.reason);
        }
        self.log.routing_strategy = selected.strategy;
        self.log.routing_attempts.push(RoutingAttempt {
            account_id: selected.account.id.clone(),
            account: selected.account.label.clone(),
            reason: selected.reason,
            previous_account: selected.previous_account.clone(),
        });
        self.attempt(&selected.account);
        if let Some(c) = self.usage_tap() {
            c.model(&selected.model);
        }
    }

    pub fn cache_policy(&mut self, policy: crate::cache::Policy) {
        self.log.cache_policy = Some(policy);
    }

    pub(crate) fn usage_tap(&self) -> Option<crate::usage::capture::Capture> {
        self.analytics.as_ref().and_then(|a| a.tap())
    }
    pub(crate) fn observe_wire(&self, value: &Value) {
        if let Some(c) = self.usage_tap() {
            c.wire(value);
        }
    }
    pub(crate) fn diagnostic_failure(&self, failure: crate::diagnostics::Failure) {
        if let Some(c) = self.usage_tap() {
            c.failure(failure);
        }
    }
    fn observe_usage_text(&self, text: &str) {
        if let Some(c) = self.usage_tap() {
            c.text(text);
        }
    }

    /// Drops the tracker without recording a request.
    pub fn cancel(&mut self) {
        if !self.done {
            if let Some(a) = &mut self.analytics {
                let _ = a.finish(None, None, false);
            }
            self.done = true;
            self.load = None;
            self.app.stats.active.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub fn first_token(&mut self) {
        if self.log.ttft_ms.is_none() {
            self.log.ttft_ms = Some(self.started.elapsed().as_millis() as u64);
        }
    }

    pub(crate) fn observe_stream_event(&mut self, event: &Event) {
        match event {
            Event::Usage(usage) => self.stream_usage.merge(usage),
            Event::Error { status, message } => self.stream_error = Some((*status, message.clone())),
            // A client may stop reading immediately after the terminal event. Keep
            // consuming while it reads, since Chat can send usage after its finish reason.
            Event::Finish(_) => self.stream_finished = true,
            event if is_content(event) => self.first_token(),
            _ => {}
        }
    }

    fn finish_stream(&mut self) {
        if !self.stream_finished && self.stream_error.is_none() {
            self.observe_stream_event(&premature_end());
        }
        let usage = self.stream_usage.clone();
        match self.stream_error.take() {
            Some((status, message)) => self.finish(status, &usage, Some(message)),
            None => self.finish(200, &usage, None),
        }
    }

    fn observe_quota_value(&self, value: &Value) {
        if let Some(acct) = &self.acct {
            observe_quota_value(acct, &self.log.model, value);
        }
    }

    /// A native transport attempt failed before HTTP fallback; the logical request continues.
    pub(crate) fn finish_fallback(&mut self, status: u16, usage: &Usage, error: Option<String>) {
        self.logical_final = false;
        self.finish(status, usage, error);
    }

    pub fn finish(&mut self, status: u16, usage: &Usage, error: Option<String>) {
        if self.done {
            return;
        }
        self.done = true;
        let failure = self.usage_tap().and_then(|c| c.failure_detail());
        let captured = self.analytics.as_mut().and_then(|a| a.finish(Some(status), Some(usage), self.logical_final));
        self.log.usage_completeness = match captured.as_ref().map(|o| o.completeness.as_str()) {
            Some("complete") => "complete",
            Some("partial") => "partial",
            _ => "missing",
        };
        self.log.failure_kind = if status == 499 {
            Some(self.downstream.kind())
        } else if status < 400 {
            None
        } else {
            failure.as_ref().map(|f| f.kind).or_else(|| crate::diagnostics::failure_kind(status, error.as_deref()))
        };
        self.load = None;
        self.app.stats.active.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        let (input, output, cache) = crate::state::usage_tokens(usage);
        self.log.status = status;
        self.log.latency_ms = self.started.elapsed().as_millis() as u64;
        self.log.input_tokens = input;
        self.log.output_tokens = output;
        self.log.cache_tokens = cache;
        self.log.error = error.map(|e| e.chars().take(400).collect());
        if let Some(a) = &self.acct {
            if let Some(message) = &self.log.error
                && quota_exhausted(a, &self.log.model, status, message)
            {
                mark_quota_exhausted(a, &self.log.model, &reqwest::header::HeaderMap::new(), message);
            }
            let mut st = a.state.lock();
            st.counters.record(&self.log);
            st.last_used = Some(Utc::now());
        }
        self.app.stats.record(&self.log);
        self.app.broadcast("request", &self.log);
        crate::diagnostics::record(&self.log, captured.as_ref(), failure.as_ref());
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        if !self.done {
            if self.stream_finished || self.stream_error.is_some() {
                self.finish_stream();
            } else {
                let usage = self.stream_usage.clone();
                self.finish(499, &usage, Some("client disconnected".into()));
            }
        }
    }
}

/// The client program behind a request, when its headers say so. Unknown agents stay unnamed.
pub fn client_app(headers: &HeaderMap) -> Option<&'static str> {
    let get = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).unwrap_or_default().to_ascii_lowercase();
    let (agent, originator) = (get("user-agent"), get("originator"));
    let has = |s: &str| agent.contains(s);
    Some(if has("claude-cli") || has("claude-code") {
        "Claude Code"
    } else if originator.starts_with("codex") || agent.starts_with("codex") {
        "Codex"
    } else if has("geminicli") || has("gemini-cli") {
        "Gemini CLI"
    } else if agent.starts_with("openai/") {
        "OpenAI SDK"
    } else if agent.starts_with("anthropic/") {
        "Anthropic SDK"
    } else if agent.starts_with("curl/") {
        "curl"
    } else {
        return None;
    })
}

// -------------------------------------------------------------------- pipeline

fn error_reply(format: Format, status: u16, msg: &str) -> Reply {
    Reply::Error(status, formats::error_body(format, status, msg))
}

/// Pulls a human readable message out of an upstream error body.
pub fn error_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        let e = if v["error"].is_object() { &v["error"] } else { &v };
        for k in ["message", "detail", "error_description"] {
            if let Some(m) = e[k].as_str() {
                return m.to_string();
            }
        }
        if let Some(m) = v["error"].as_str() {
            return m.to_string();
        }
    }
    let t = body.trim();
    if t.is_empty() { "upstream error".into() } else { t.chars().take(500).collect() }
}

/// When the upstream told us its quota resets.
fn reset_after(headers: &reqwest::header::HeaderMap, body: &str) -> Option<chrono::DateTime<Utc>> {
    let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string());
    if let Some(s) = h("retry-after").and_then(|s| s.parse::<i64>().ok()) {
        return Some(Utc::now() + Duration::seconds(s.clamp(1, 6 * 3600)));
    }
    if let Some(ts) = h("anthropic-ratelimit-unified-reset").and_then(|s| s.parse::<i64>().ok()) {
        return chrono::DateTime::from_timestamp(ts, 0);
    }
    let v: Value = serde_json::from_str(body).ok()?;
    let e = &v["error"];
    if let Some(s) = e["resets_in_seconds"].as_i64() {
        return Some(Utc::now() + Duration::seconds(s.max(1)));
    }
    if let Some(t) = e["resets_at"].as_i64() {
        return chrono::DateTime::from_timestamp(t, 0);
    }
    // Google: {"details": [{"@type": "...RetryInfo", "retryDelay": "3.5s"}]}
    for d in e["details"].as_array().into_iter().flatten() {
        if let Some(delay) = d["retryDelay"].as_str().and_then(|s| s.trim_end_matches('s').parse::<f64>().ok()) {
            return Some(Utc::now() + Duration::milliseconds((delay * 1000.0).max(1000.0) as i64));
        }
        if let Some(ts) = d["metadata"]["quotaResetTimeStamp"].as_str()
            && let Ok(t) = chrono::DateTime::parse_from_rfc3339(ts)
        {
            return Some(t.with_timezone(&Utc));
        }
    }
    None
}

/// A failure worth retrying on the same account, and how long to wait first:
/// capacity errors (Google's "No capacity available") and rate limits that
/// reset within seconds. Real quota exhaustion is not one of them.
fn soft_failure(
    provider: Provider,
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body: &str,
) -> Option<std::time::Duration> {
    let short = |until: chrono::DateTime<Utc>| {
        let secs = (until - Utc::now()).num_milliseconds().max(0) as u64;
        (secs <= 10_000).then(|| std::time::Duration::from_millis(secs))
    };
    match status {
        // Not 504: the provider may still be generating, so it is answered, not repeated.
        500 | 502 | 503 | 529 => Some(std::time::Duration::ZERO),
        429 => {
            if body.contains("QUOTA_EXHAUSTED") {
                return None;
            }
            match reset_after(headers, body) {
                Some(until) => short(until),
                // Google answers bursts with a bare RESOURCE_EXHAUSTED while quota is left.
                None if matches!(provider, Provider::Antigravity | Provider::Gemini | Provider::Vertex) => {
                    Some(std::time::Duration::ZERO)
                }
                None => None,
            }
        }
        _ => None,
    }
}

/// A 429 is also used for burst limits. Move a pinned task only when quota
/// headers or an explicit provider error confirm subscription exhaustion.
pub fn quota_exhausted(acct: &Account, model: &str, status: u16, body: &str) -> bool {
    if acct.exhausted_until(model).is_some() {
        return true;
    }
    if status != 429 && status != 402 {
        return false;
    }
    let b = body.to_ascii_lowercase();
    [
        "quota_exhausted",
        "quota_exceeded",
        "insufficient_quota",
        "usage_limit_reached",
        "usage limit reached",
        "hit your usage limit",
        "hit your chatgpt usage limit",
        "reached your usage limit",
        "weekly limit",
        "weekly usage",
        "credit balance is too low",
        "credits exhausted",
        "quota has been exhausted",
    ]
    .iter()
    .any(|s| b.contains(s))
}

pub fn mark_quota_exhausted(acct: &Account, model: &str, headers: &reqwest::header::HeaderMap, body: &str) {
    let until = acct
        .exhausted_until(model)
        .or_else(|| reset_after(headers, body))
        .unwrap_or_else(|| Utc::now() + Duration::minutes(5));
    acct.exhaust(model, until, &format!("subscription quota exhausted: {}", error_message(body)));
}

fn observe_quota_value(acct: &Account, model: &str, v: &Value) {
    let error = if v["response"]["error"].is_object() { &v["response"]["error"] } else { &v["error"] };
    if error.is_object() {
        let status = v["status"].as_u64().unwrap_or(429) as u16;
        if quota_exhausted(acct, model, status, &error.to_string()) {
            mark_quota_exhausted(acct, model, &reqwest::header::HeaderMap::new(), &error.to_string());
        }
    }
}

/// Responses backends that understand freeform `custom` tools.
fn native_custom_tools(p: Provider) -> bool {
    matches!(p, Provider::Codex | Provider::Compat)
}

/// 403s that a token refresh won't fix (region / plan / ToS blocks).
fn forbidden_for_good(body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    ["not available in your", "unsupported_country", "permission_denied", "terms of service"]
        .iter()
        .any(|k| b.contains(k))
}

fn backoff(acct: &Account) -> chrono::DateTime<Utc> {
    let mut st = acct.state.lock();
    st.strikes = st.strikes.saturating_add(1);
    let secs = (30u64 << (st.strikes - 1).min(6)).min(30 * 60);
    Utc::now() + Duration::seconds(secs as i64)
}

/// Applies a `model(effort)` suffix to a body that is passed through untouched.
fn apply_native_reasoning(format: Format, body: &mut Value, r: &Reasoning, model: &str) {
    match format {
        Format::Chat => {
            if let Some(e) = r.effort_level() {
                body["reasoning_effort"] = e.into();
            }
        }
        Format::Responses => {
            if let Some(e) = r.effort_level() {
                body["reasoning"]["effort"] = e.into();
            }
        }
        Format::Claude => {
            if r.disabled {
                body["thinking"] = json!({ "type": "disabled" });
            } else if formats::claude::uses_budget_thinking(model) {
                if let Some(b) = r.budget_tokens() {
                    let max = body["max_tokens"].as_u64().unwrap_or(formats::claude::default_max_tokens(model));
                    body["thinking"] = json!({ "type": "enabled", "budget_tokens": formats::claude::thinking_budget(max, b).unwrap_or(1024) });
                }
            } else if let Some(e) = r.effort_level() {
                body["thinking"] = json!({ "type": "adaptive" });
                body["output_config"]["effort"] = e.into();
            }
        }
        Format::Gemini => formats::gemini::apply_native_reasoning(body, r, model),
    }
}

/// OpenAI and Gemini limit tool names to 64 characters.
fn has_long_tool_names(req: &Request) -> bool {
    req.tools.iter().any(|tool| tool.name.len() > 64)
        || req.custom_tools.iter().any(|name| name.len() > 64)
        || matches!(&req.tool_choice, ir::ToolChoice::Tool(name) if name.len() > 64)
        || req
            .messages
            .iter()
            .flat_map(|message| &message.parts)
            .any(|part| matches!(part, ir::Part::ToolCall { name, .. } if name.len() > 64))
}

fn shorten_tool_names(req: &mut Request) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut short = |name: &mut String| {
        if name.len() <= 64 {
            return;
        }
        use sha2::{Digest, Sha256};
        let h = hex::encode(Sha256::digest(name.as_bytes()));
        let mut cut = 55;
        while !name.is_char_boundary(cut) {
            cut -= 1;
        }
        let s = format!("{}_{}", &name[..cut], &h[..8]);
        map.insert(s.clone(), name.clone());
        *name = s;
    };
    for t in &mut req.tools {
        short(&mut t.name);
    }
    for n in &mut req.custom_tools {
        short(n);
    }
    if let ir::ToolChoice::Tool(n) = &mut req.tool_choice {
        short(n);
    }
    for m in &mut req.messages {
        for p in &mut m.parts {
            if let ir::Part::ToolCall { name, .. } = p {
                short(name);
            }
        }
    }
    map
}

pub async fn execute(app: Arc<App>, mut call: Call) -> Reply {
    if crate::clients::observe(&call.headers) {
        app.models.save_clients();
    }
    let cfg = app.cfg();
    let raw = call.path_model.clone().or_else(|| call.body["model"].as_str().map(String::from)).unwrap_or_default();
    let (base, _) = ir::split_model_suffix(&raw);
    if let Err(d) = crate::policy::check_model(&app, &call.headers, &base) {
        return error_reply(call.format, d.status, &d.message);
    }
    // Claude Code asks its Haiku model for quick background work (titles, summaries,
    // checks); `background-model` sends those calls elsewhere.
    if let Some(background) = cfg.background_model.as_deref()
        && base.to_ascii_lowercase().contains("haiku")
        && crate::upstream::is_claude_code(&call.headers)
    {
        set_model(&mut call, background);
    }
    if let Some(identity) = crate::affinity::session_identity(&call.headers, &call.body) {
        call.session = Some(identity.key);
        call.session_source = Some(identity.source);
    }
    if call.format == Format::Responses
        && let Some(id) = call.body["previous_response_id"].as_str()
        && let Some((session, mut previous)) = app.sessions.previous(&call.headers, id, call.session.as_deref())
    {
        previous.extend(crate::affinity::input_items(&call.body));
        call.body["input"] = Value::Array(previous);
        call.body.as_object_mut().unwrap().remove("previous_response_id");
        if call.session.is_none() {
            call.session_source = Some("previous_response_id");
        }
        call.session = Some(session);
    }
    // Without a session id, a conversation is recognised by its first user message.
    // A continuation by response id already has its session, or names one we don't know.
    if call.session.is_none()
        && !call.body["previous_response_id"].is_string()
        && let Some(identity) = crate::affinity::inferred_identity(&call.headers, call.format, &call.body)
    {
        call.session = Some(identity.key);
        call.session_source = Some(identity.source);
    }
    if call.format == Format::Responses && call.session.is_none() {
        // A response id can identify subsequent turns even without client session metadata.
        call.session = Some(crate::affinity::connection_key(&call.headers, &uuid::Uuid::new_v4().to_string()));
        call.session_source = Some("generated_response");
    }
    // Public API backends may resolve ids outside our local history. Do not
    // record an unknown continuation as though its input were complete.
    let capture = (call.format == Format::Responses && !call.body["previous_response_id"].is_string())
        .then(|| call.session.clone())
        .flatten();
    let headers = call.headers.clone();
    let session = call.session.clone();
    let lease = session.as_deref().map(|s| app.sessions.hold(s, app.cfg().session_affinity_idle_seconds));
    let end_session = crate::affinity::ends_session(&headers);
    let mut full = capture.as_ref().map(|_| crate::affinity::input_items(&call.body)).unwrap_or_default();
    match execute_models(app.clone(), call).await {
        Reply::Stream { mut frames, account } if lease.is_some() => {
            let frames = Box::pin(async_stream::stream! {
                let mut history = History::default();
                let _lease = lease;
                while let Some(frame) = frames.next().await {
                    if let Some(session) = &capture
                        && let Some(response) = history.observe(&frame)
                    {
                        app.sessions.remember(&headers, session, &response, std::mem::take(&mut full));
                    }
                    yield frame;
                }
                if end_session && let Some(session) = &session {
                    app.sessions.end(session);
                    app.sessions.save_async().await;
                }
            });
            Reply::Stream { frames, account }
        }
        Reply::Json(v) => {
            if let Some(session) = capture {
                app.sessions.remember(&headers, &session, &v, full);
            }
            if end_session && let Some(session) = &session {
                app.sessions.end(session);
                app.sessions.save_async().await;
            }
            Reply::Json(v)
        }
        other => other,
    }
}

/// Builds a Responses turn's continuation history from the frames sent to the client.
///
/// The terminal response normally carries the output. Some Codex backends leave it
/// empty: most then sent each finished item, which is kept; for a backend that sent
/// only deltas, the turn is rebuilt from them as before. Named deltas are buffered
/// unparsed (until the first finished item makes them unnecessary) and parsed only
/// when that rebuild is needed, so ordinary turns parse just a few frames.
#[derive(Default)]
struct History {
    items: Vec<(u64, Value)>,
    deltas: Vec<Frame>,
    buffered: usize,
    overflow: bool,
}

impl History {
    /// The rebuild is bounded like a stored history entry; past it, deltas are dropped.
    const LIMIT: usize = 8 << 20;

    fn buffer_delta(&mut self, frame: &Frame) {
        if self.items.is_empty() && !self.overflow {
            self.buffered = self.buffered.saturating_add(frame.data.len());
            self.overflow = self.buffered > Self::LIMIT;
            if self.overflow {
                self.deltas = Vec::new();
            } else {
                self.deltas.push(Frame { event: frame.event.clone(), data: frame.data.clone() });
            }
        }
    }

    /// The finished response to remember, once the terminal frame arrives.
    fn observe(&mut self, frame: &Frame) -> Option<Value> {
        let named = frame.event.as_deref();
        let wanted = named.is_none_or(|e| {
            matches!(e, "response.output_item.done" | "response.completed" | "response.incomplete" | "response.done")
        });
        if !wanted {
            self.buffer_delta(frame);
            return None;
        }
        let mut value = serde_json::from_str::<Value>(&frame.data).ok()?;
        match value["type"].as_str() {
            Some("response.output_item.done") => {
                let index = value["output_index"].as_u64().unwrap_or(self.items.len() as u64);
                self.items.push((index, value["item"].take()));
                self.deltas = Vec::new();
                None
            }
            Some("response.completed" | "response.incomplete" | "response.done") => {
                let mut response = value["response"].take();
                if response["output"].as_array().is_none_or(|o| o.is_empty()) {
                    if !self.items.is_empty() {
                        self.items.sort_by_key(|(index, _)| *index);
                        response["output"] = Value::Array(self.items.drain(..).map(|(_, item)| item).collect());
                    } else if !self.deltas.is_empty() {
                        use crate::formats::StreamParser;
                        let mut parser = formats::responses::Parser::default();
                        let mut aggregate = Aggregate::default();
                        let mut events = Vec::new();
                        for delta in self.deltas.drain(..) {
                            let parsed = serde_json::from_str::<Value>(&delta.data).ok();
                            let sse =
                                crate::sse::SseEvent { event: delta.event.map(|e| e.into_owned()), data: delta.data };
                            parser.feed_value(&sse, parsed.as_ref(), &mut events);
                            for event in events.drain(..) {
                                aggregate.push(&event);
                            }
                        }
                        crate::affinity::complete_output(&mut response, &aggregate);
                    }
                }
                Some(response)
            }
            _ => {
                self.buffer_delta(frame);
                None
            }
        }
    }
}

/// Points a call at another model, keeping a reasoning suffix the client added.
fn set_model(call: &mut Call, model: &str) {
    let raw = call.path_model.clone().or_else(|| call.body["model"].as_str().map(String::from)).unwrap_or_default();
    let (base, _) = ir::split_model_suffix(&raw);
    let suffix = raw.trim().strip_prefix(base.as_str()).unwrap_or_default();
    let (_, own) = ir::split_model_suffix(model);
    let model = if own.is_some() { model.to_string() } else { format!("{model}{suffix}") };
    if call.path_model.is_some() {
        call.path_model = Some(model.clone());
    }
    if call.body.is_object() && (call.path_model.is_none() || call.body.get("model").is_some()) {
        call.body["model"] = model.into();
    }
}

/// Whether the next model of a group should take over from an answer like this one:
/// no account, rate limits, used-up quota or an upstream failure, not a bad request.
fn group_moves_on(reply: &Reply) -> bool {
    // An upstream gateway timeout can represent work still running. The inner
    // pipeline's terminal decision also applies to the outer model group.
    matches!(reply, Reply::Error(status, _) if *status != 504 && (matches!(status, 401 | 403 | 404 | 408 | 429) || *status >= 500))
}

struct RetryBudget {
    started: Instant,
    remaining_accounts: usize,
    remaining_attempts: usize,
    selected: std::collections::HashSet<(String, String)>,
}

impl RetryBudget {
    fn new(attempts: u32) -> Self {
        Self {
            started: Instant::now(),
            remaining_accounts: (attempts as usize).clamp(1, MAX_ACCOUNTS),
            remaining_attempts: MAX_ACCOUNTS,
            selected: Default::default(),
        }
    }

    fn available(&self) -> bool {
        self.remaining_attempts > 0 && self.started.elapsed() < RETRY_BUDGET
    }

    fn next_model(&self) -> bool {
        self.available() && self.remaining_accounts > 0
    }

    fn take(&mut self, model: &str, account: &str) -> bool {
        if !self.available() {
            return false;
        }
        let selection = (model.to_string(), account.to_string());
        if !self.selected.contains(&selection) {
            if self.remaining_accounts == 0 {
                return false;
            }
            self.remaining_accounts -= 1;
            self.selected.insert(selection);
        }
        self.remaining_attempts -= 1;
        true
    }

    async fn wait(&self, delay: std::time::Duration) -> bool {
        if !self.available() || delay >= RETRY_BUDGET.saturating_sub(self.started.elapsed()) {
            return false;
        }
        tokio::time::sleep(delay).await;
        self.available()
    }
}

static GROUP_TURN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A model group's models in the order this call tries them. In order by default; with
/// `balance`, new sessions start at different models (a session always the same one,
/// so its prompt cache keeps working) and the rest follow.
fn group_order<'a>(group: &'a crate::config::ModelGroup, session: Option<&str>) -> Vec<&'a str> {
    let n = group.models.len();
    let start = if !group.balance || n == 0 {
        0
    } else if let Some(s) = session {
        use sha2::{Digest, Sha256};
        let h = Sha256::digest(s.as_bytes());
        u64::from_le_bytes(h[..8].try_into().unwrap()) as usize % n
    } else {
        GROUP_TURN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % n
    };
    (0..n).map(|i| group.models[(start + i) % n].as_str()).collect()
}

#[cfg(test)]
pub fn group_order_for_tests(group: &crate::config::ModelGroup, session: Option<&str>) -> Vec<String> {
    group_order(group, session).into_iter().map(String::from).collect()
}

/// Runs a call, or for a model group, each of its models until one answers.
async fn execute_models(app: Arc<App>, mut call: Call) -> Reply {
    let cfg = app.cfg();
    let mut budget = RetryBudget::new(cfg.request_retry);
    let raw = call.path_model.clone().or_else(|| call.body["model"].as_str().map(String::from)).unwrap_or_default();
    let (base, _) = ir::split_model_suffix(&raw);
    let Some(group) = cfg.model_groups.get(&base) else { return execute_inner(app, call, &mut budget).await };
    let order = group_order(group, call.session.as_deref());
    let mut last = None;
    for (i, model) in order.iter().enumerate() {
        let mut attempt = Call {
            format: call.format,
            body: if i + 1 == order.len() { call.body.take() } else { call.body.clone() },
            headers: call.headers.clone(),
            stream: call.stream,
            transport: call.transport,
            path_model: call.path_model.clone(),
            session: call.session.clone(),
            session_source: call.session_source,
            routing_selection: if i == 0 { call.routing_selection.clone() } else { None },
        };
        set_model(&mut attempt, model);
        let reply = execute_inner(app.clone(), attempt, &mut budget).await;
        if i + 1 < order.len() && group_moves_on(&reply) && budget.next_model() {
            tracing::info!(group = %base, model = %model, "model group: trying the next model");
            last = Some(reply);
            continue;
        }
        return reply;
    }
    last.unwrap_or_else(|| error_reply(call.format, 404, &format!("model group {base} lists no models")))
}

async fn execute_inner(app: Arc<App>, call: Call, budget: &mut RetryBudget) -> Reply {
    let cfg = app.cfg();
    let raw_model =
        call.path_model.clone().or_else(|| call.body["model"].as_str().map(String::from)).unwrap_or_default();
    if raw_model.is_empty() {
        return error_reply(call.format, 400, "`model` is required");
    }
    let (model, suffix) = ir::split_model_suffix(&raw_model);
    let (only, model) = app.pool.route(&model);
    let model = app.pool.canonical(&model, only.as_ref());
    let mut tracker = Tracker::new(&app, call.format, call.stream, call.transport, &model);
    tracker.client_app(&call.headers);
    tracker.session(call.session.as_deref(), call.session_source, &cfg);
    let requested_cache = crate::cache::Requested::of(&call.body);

    let mut parsed: Option<Request> = None;
    let mut shortened: Option<(Request, HashMap<String, String>)> = None;
    let mut tried: Vec<String> = Vec::new();
    let mut refreshed: Vec<String> = Vec::new();
    let mut last_error: Option<(u16, Value)> = None;
    let mut retry_same: Option<String> = None;
    let mut pending_selection = call.routing_selection;
    // Same-account retries for blips (capacity errors, soft rate limits).
    let mut soft_tries: HashMap<String, u32> = HashMap::new();
    let attempts = (cfg.request_retry as usize).clamp(1, MAX_ACCOUNTS);
    let first_token =
        (cfg.first_token_timeout_seconds > 0).then(|| std::time::Duration::from_secs(cfg.first_token_timeout_seconds));
    let inferred = call.session_source == Some(crate::affinity::INFERRED_SOURCE);
    // A per-request random id would split the provider's cache rather than route to it.
    let (cache_key, cache_scope) =
        crate::cache::key(&call.headers, &call.body, call.session.as_deref(), call.session_source, &cfg.prompt_cache);

    while tried.len() < attempts {
        // Past the budget, the error so far is the answer rather than another wait.
        if !budget.available() {
            break;
        }
        let pin = retry_same.take();
        // A retry on a temporary detour must stay on that account without
        // replacing the session's long-lived assignment to its original account.
        let retry = pin.as_deref().and_then(|id| app.pool.get(id)).and_then(|account| {
            if tried.contains(&account.id) || account.state.lock().disabled || account.cooling_until(&model).is_some() {
                return None;
            }
            app.pool.resolve_account(&account, &model, only.as_ref()).map(|model| crate::affinity::Selected {
                account,
                model,
                strategy: cfg.routing,
                reason: "retry_same",
                previous_account: None,
            })
        });
        // A generated id only threads previous_response_id continuations; it must not
        // create an assignment for every request that arrives without a session.
        let picked = if let Some(retry) = retry {
            Ok(retry)
        } else if cfg.session_affinity && call.session.is_some() && call.session_source != Some("generated_response") {
            app.sessions.pick_session(&app.pool, &cfg, &model, call.session.as_deref(), inferred, &tried, only.as_ref())
        } else {
            match app.sessions.pick_unbound(&app.pool, &cfg, &model, &tried, pin.as_deref(), only.as_ref()) {
                Pick::Ok(a, m) => Ok(crate::affinity::Selected {
                    reason: if pin.as_deref() == Some(a.id.as_str()) {
                        "retry_same"
                    } else if !cfg.session_affinity {
                        "affinity_disabled"
                    } else {
                        "missing_session"
                    },
                    account: a,
                    model: m,
                    strategy: cfg.routing,
                    previous_account: None,
                }),
                Pick::Cooling(until) => Err((
                    429,
                    format!(
                        "all accounts for {model} are rate limited; next available in {}s",
                        (until - Utc::now()).num_seconds().max(1)
                    ),
                )),
                Pick::None => Err((404, format!("no available account serves model `{model}`"))),
            }
        };
        let mut selected = match picked {
            Ok(pair) => pair,
            Err((status, msg)) => {
                if let Some((s, b)) = last_error {
                    tracker.finish(s, &Usage::default(), Some(error_message(&b.to_string())));
                    return Reply::Error(s, b);
                }
                tracker.finish(status, &Usage::default(), Some(msg.clone()));
                return error_reply(call.format, status, &msg);
            }
        };
        // Native eligibility checks can assign a session before HTTP sends anything.
        // Keep that decision when this selection merely reuses the same assignment.
        if let Some(pending) = pending_selection.take()
            && selected.reason == "session_reused"
            && selected.account.id == pending.account.id
            && matches!(pending.reason, "new_session" | "quota_exhausted")
        {
            selected.reason = pending.reason;
            selected.strategy = pending.strategy;
            selected.previous_account = pending.previous_account;
        }
        if !budget.take(&model, &selected.account.id) {
            break;
        }
        tracker.selected(&selected);
        // An inferred assignment is a best guess: it is written in the background, so
        // requests without a session id never wait for the disk. Other new assignments
        // are written while the provider is contacted, and are durable before the
        // client sees any of the answer.
        let mut durable = if inferred { None } else { app.sessions.begin_persist(&selected) };
        let (acct, upstream_model) = (selected.account, selected.model);

        if let Err(e) = crate::oauth::ensure_ready(&app, &acct).await {
            let failure = crate::diagnostics::Failure {
                kind: "credential_refresh",
                causes: crate::diagnostics::cause_chain(e.as_ref()),
            };
            tracker.diagnostic_failure(failure.clone());
            tracing::warn!(request_id = tracker.log.id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
                failure_kind = failure.kind, error_causes = ?failure.causes, "credential refresh failed");
            acct.cool(None, Utc::now() + Duration::minutes(5), &format!("token refresh failed: {e}"));
            tried.push(acct.id.clone());
            last_error = Some((401, formats::error_body(call.format, 401, &format!("token refresh failed: {e}"))));
            continue;
        }

        let quota_epoch = acct.quota_epoch();
        let provider = acct.provider;
        if call.body["previous_response_id"].is_string()
            && ((provider == Provider::Codex && acct.is_oauth()) || !provider.wires().contains(&Format::Responses))
        {
            let msg =
                "Previous response is unavailable; resend the full conversation input without previous_response_id";
            tracker.finish(400, &Usage::default(), Some(msg.into()));
            return Reply::Error(
                400,
                json!({"error":{"message":msg,"type":"invalid_request_error","code":"previous_response_not_found","param":"previous_response_id"}}),
            );
        }
        let devin = provider == Provider::Devin;
        // Freeform (custom) tools only exist on OpenAI's own Responses backends.
        let custom_tools = call.format == Format::Responses
            && call.body["tools"].as_array().is_some_and(|t| t.iter().any(|t| t["type"] == "custom"));
        let passthrough = provider.wires().contains(&call.format) && (!custom_tools || native_custom_tools(provider));
        let native = if passthrough { call.format } else { provider.wires().first().copied().unwrap_or(Format::Chat) };
        let mut names = HashMap::new();
        let mut body = if passthrough {
            let mut b = call.body.clone();
            if let Some(r) = &suffix {
                apply_native_reasoning(native, &mut b, r, &upstream_model);
            }
            b
        } else {
            if parsed.is_none() {
                let body = if call.format == Format::Gemini {
                    let mut body = call.body.clone();
                    body["model"] = model.clone().into();
                    std::borrow::Cow::Owned(body)
                } else {
                    std::borrow::Cow::Borrowed(&call.body)
                };
                match formats::parse_request(call.format, &body) {
                    Ok(mut r) => {
                        if suffix.is_some() {
                            r.reasoning = suffix.clone();
                        }
                        parsed = Some(r);
                    }
                    Err(e) => {
                        tracker.finish(400, &Usage::default(), Some(e.clone()));
                        return error_reply(call.format, 400, &e);
                    }
                }
            }
            let original = parsed.as_ref().unwrap();
            let req = if native != Format::Claude && has_long_tool_names(original) {
                let (req, mapping) = shortened.get_or_insert_with(|| {
                    let mut req = original.clone();
                    let mapping = shorten_tool_names(&mut req);
                    (req, mapping)
                });
                names = mapping.clone();
                &*req
            } else {
                original
            };
            if native == Format::Claude
                && !devin
                && let Err(message) = formats::claude::validate_thinking_budget(req, &upstream_model)
            {
                tracker.finish(400, &Usage::default(), Some(message.clone()));
                return error_reply(call.format, 400, &message);
            }
            match native {
                _ if devin => crate::devin::build_request(req, &upstream_model),
                Format::Claude => formats::claude::build_request(req, &upstream_model),
                Format::Responses => formats::responses::build_request(
                    req,
                    &upstream_model,
                    &formats::responses::BuildOpts {
                        chatgpt_backend: provider == Provider::Codex && acct.is_oauth(),
                        custom_tools: native_custom_tools(provider),
                        default_reasoning: provider == Provider::Codex,
                    },
                ),
                Format::Gemini => formats::gemini::build_request(req, &upstream_model),
                Format::Chat => formats::chat::build_request(req, &upstream_model),
            }
        };
        let cache_downgrade = if passthrough {
            None
        } else {
            match formats::preserve_cache_hints(call.format, native, &call.body, &mut body) {
                Ok(downgrade) => downgrade,
                Err(message) => {
                    let mut policy = crate::cache::Policy::for_request(
                        &requested_cache,
                        &body,
                        native,
                        cache_scope,
                        Some(message.clone()),
                    );
                    policy.applied = "rejected";
                    policy.key_scope = "not_sent";
                    tracker.cache_policy(policy);
                    tracker.finish(400, &Usage::default(), Some(message.clone()));
                    return error_reply(call.format, 400, &message);
                }
            }
        };
        if let Some(reason) = &cache_downgrade {
            if cfg.prompt_cache.on_incompatible == crate::cache::Incompatible::Reject {
                let mut policy = crate::cache::Policy::for_request(
                    &requested_cache,
                    &body,
                    native,
                    cache_scope,
                    cache_downgrade.clone(),
                );
                policy.applied = "rejected";
                policy.key_scope = "not_sent";
                tracker.cache_policy(policy);
                tracker.finish(400, &Usage::default(), Some(reason.clone()));
                return error_reply(call.format, 400, reason);
            }
            if cfg.prompt_cache.on_incompatible == crate::cache::Incompatible::Automatic
                && provider == Provider::Claude
                && requested_cache.markers
                && !crate::cache::has_markers(&body)
            {
                // The caller explicitly opted into provider defaults after a lost
                // boundary. Explicit mode without any markers retains its intent.
                body["cache_control"] = json!({"type":"ephemeral"});
            }
        }
        if native == Format::Claude
            && !devin
            && let Err(message) = formats::claude::validate_native_thinking_budget(&body, &upstream_model)
        {
            tracker.finish(400, &Usage::default(), Some(message.clone()));
            return error_reply(call.format, 400, &message);
        }
        if !passthrough && provider == Provider::Claude {
            formats::claude::apply_translated_cache_policy_with(
                requested_cache.explicit_mode,
                requested_cache.markers,
                &mut body,
            );
        }

        // Upstream streaming: always when translating (we re-render), and for Codex OAuth.
        let upstream_stream = !passthrough || call.stream || (provider == Provider::Codex && acct.is_oauth());
        let prepared = upstream::prepare(
            &Target {
                acct: &acct,
                cfg: &cfg,
                client_headers: &call.headers,
                model: &upstream_model,
                wire: native,
                passthrough,
                stream: upstream_stream,
                count_tokens: false,
                cache_key: cache_key.as_deref(),
            },
            body,
        );
        let mut cache_policy =
            crate::cache::Policy::for_request(&requested_cache, &prepared.body, native, cache_scope, cache_downgrade);
        if prepared.headers.iter().any(|(name, _)| name == "x-grok-conv-id") {
            cache_policy.key_scope = cache_scope;
        }
        tracker.cache_policy(cache_policy);
        if cfg.debug {
            tracing::debug!(provider = provider.as_str(), "upstream request prepared");
        }

        let client = app.http.for_account(&acct);
        let mut rb = client.post(&prepared.url);
        for (k, v) in &prepared.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        let payload = prepared.raw.unwrap_or_else(|| serde_json::to_vec(&prepared.body).unwrap_or_default());
        let send = rb.body(payload).send();
        // A stream's headers come before its first token; an upstream that hasn't even
        // sent those by the first-token deadline has stalled.
        let sent = match first_token.filter(|_| upstream_stream) {
            Some(limit) => tokio::time::timeout(limit, send).await.ok(),
            None => Some(send.await),
        };
        let Some(sent) = sent else {
            let msg = stalled_message(first_token);
            tracker.diagnostic_failure(crate::diagnostics::Failure::classified("upstream_stalled"));
            tracing::warn!(request_id = tracker.log.id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
                "upstream sent nothing before the first-token deadline; trying another account");
            acct.cool(Some(&model), Utc::now() + Duration::minutes(2), &msg);
            app.broadcast("accounts", Value::Null);
            tried.push(acct.id.clone());
            last_error = Some((504, formats::error_body(call.format, 504, &msg)));
            continue;
        };
        let resp = match sent {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("upstream connection failed: {e}");
                let failure = crate::diagnostics::Failure::http("upstream_connect", &e);
                tracker.diagnostic_failure(failure.clone());
                tracing::warn!(request_id = tracker.log.id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
                    failure_kind = failure.kind, error_causes = ?failure.causes, "upstream connection failed");
                acct.state.lock().last_error = Some(msg.clone());
                // A connection that never opened is a blip worth one more try on the same
                // account. Once the request went out, the provider may have run it, so it
                // moves on rather than running it twice on one account.
                let n = soft_tries.entry(acct.id.clone()).or_default();
                if *n < 1 && e.is_connect() {
                    *n += 1;
                    last_error = Some((502, formats::error_body(call.format, 502, &msg)));
                    if !budget.wait(std::time::Duration::from_secs(1)).await {
                        break;
                    }
                    retry_same = Some(acct.id.clone());
                    continue;
                }
                tried.push(acct.id.clone());
                last_error = Some((502, formats::error_body(call.format, 502, &msg)));
                continue;
            }
        };
        if let Some(durable) = durable.take() {
            let _ = durable.await;
        }

        let status = resp.status().as_u16();
        if let Some(c) = tracker.usage_tap() {
            c.headers(resp.headers(), status);
        }
        crate::quota::observe(&acct, resp.headers(), quota_epoch);
        if !resp.status().is_success() {
            let headers = resp.headers().clone();
            let text = match resp.text().await {
                Ok(text) => text,
                Err(error) => {
                    tracker.diagnostic_failure(crate::diagnostics::Failure::http("upstream_body_read", &error));
                    String::new()
                }
            };
            tracker.observe_usage_text(&text);
            let msg = error_message(&text);
            tracing::warn!(request_id = tracker.log.id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
                status, failure_kind = crate::diagnostics::failure_kind(status, Some(&msg)), "upstream rejected request");
            let client_body = if passthrough {
                serde_json::from_str(&text).unwrap_or_else(|_| formats::error_body(call.format, status, &msg))
            } else {
                formats::error_body(call.format, status, &msg)
            };
            if quota_exhausted(&acct, &model, status, &text) {
                mark_quota_exhausted(&acct, &model, &headers, &text);
                app.broadcast("accounts", Value::Null);
                tried.push(acct.id.clone());
                last_error = Some((status, client_body));
                continue;
            }
            if let Some(delay) = soft_failure(provider, status, &headers, &text) {
                let n = soft_tries.entry(acct.id.clone()).or_default();
                if *n < 2 {
                    *n += 1;
                    let wait = delay.max(std::time::Duration::from_secs(if *n == 1 { 1 } else { 3 }));
                    tracing::info!(request_id = tracker.log.id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
                        status, retry_delay_seconds = wait.as_secs(), "upstream busy; retrying");
                    last_error = Some((status, client_body));
                    if !budget.wait(wait).await {
                        break;
                    }
                    retry_same = Some(acct.id.clone());
                    continue;
                }
                // Still busy: step aside briefly, without the escalating backoff.
                if status == 429 {
                    acct.cool(Some(&model), Utc::now() + Duration::seconds(15), &format!("429: {msg}"));
                } else {
                    acct.state.lock().last_error = Some(format!("{status}: {msg}"));
                }
                app.broadcast("accounts", Value::Null);
                tried.push(acct.id.clone());
                last_error = Some((status, client_body));
                continue;
            }
            match status {
                429 => {
                    // Quota exhaustion was handled above; this is a rate limit.
                    let until = reset_after(&headers, &text).unwrap_or_else(|| backoff(&acct));
                    acct.cool(Some(&model), until, &format!("429: {msg}"));
                    app.broadcast("accounts", Value::Null);
                }
                401 | 403 if acct.is_oauth() && !refreshed.contains(&acct.id) && !forbidden_for_good(&text) => {
                    refreshed.push(acct.id.clone());
                    if crate::oauth::ensure_fresh(&app, &acct, Duration::minutes(5), true).await.is_ok() {
                        // Retry the same account with the new token.
                        retry_same = Some(acct.id.clone());
                        last_error = Some((status, client_body));
                        continue;
                    }
                    acct.cool(None, Utc::now() + Duration::minutes(10), &format!("{status}: {msg}"));
                }
                401 | 403 => acct.cool(None, Utc::now() + Duration::minutes(10), &format!("{status}: {msg}")),
                // A gateway timeout may still be generating upstream: answer, don't repeat.
                400 | 404 | 413 | 422 | 504 => {
                    acct.state.lock().last_error = Some(format!("{status}: {msg}"));
                    tracker.finish(status, &Usage::default(), Some(msg));
                    return Reply::Error(status, client_body);
                }
                _ => {
                    acct.state.lock().last_error = Some(format!("{status}: {msg}"));
                }
            }
            tried.push(acct.id.clone());
            last_error = Some((status, client_body));
            continue;
        }

        // The ChatGPT backend streams without any Content-Type, so a stream we
        // asked for counts as one unless it says it's JSON.
        let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default();
        let is_sse = ctype.contains("event-stream") || (upstream_stream && !ctype.contains("json"));
        let mut resp = Some(resp);
        // A stream is held back until it shows content, so an overload or rate limit
        // that arrives inside a 200 still goes to another account (see `bootstrap`).
        let mut body: Option<ByteStream> = None;
        if is_sse && !devin {
            let stream: ByteStream = Box::pin(resp.take().expect("response").bytes_stream());
            match bootstrap(stream, native, first_token).await {
                Boot::Ready(stream) => body = Some(stream),
                Boot::Failed { status, message, held } => {
                    tracing::warn!(request_id = tracker.log.id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
                        status, "upstream failed before its first token; trying another account");
                    // What it reported before failing still counts for this attempt.
                    let tap = tracker.usage_tap();
                    let mut dec = SseDecoder::default();
                    let mut events: Vec<_> = held.iter().flat_map(|chunk| dec.push(chunk)).collect();
                    events.extend(dec.finish());
                    for sse in &events {
                        observe_sse(&acct, &model, tap.as_ref(), sse);
                    }
                    if let Some(c) = &tap {
                        c.headers(&HeaderMap::new(), status);
                    }
                    if status == 429 && quota_exhausted(&acct, &model, status, &message) {
                        mark_quota_exhausted(&acct, &model, &reqwest::header::HeaderMap::new(), &message);
                    } else if status == 429 {
                        acct.cool(Some(&model), Utc::now() + Duration::seconds(30), &format!("429: {message}"));
                    } else {
                        acct.state.lock().last_error = Some(format!("{status}: {message}"));
                    }
                    app.broadcast("accounts", Value::Null);
                    tried.push(acct.id.clone());
                    last_error = Some((status, formats::error_body(call.format, status, &message)));
                    continue;
                }
                Boot::Stalled => {
                    let msg = stalled_message(first_token);
                    tracker.diagnostic_failure(crate::diagnostics::Failure::classified("upstream_stalled"));
                    tracing::warn!(request_id = tracker.log.id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
                        "upstream stream stayed silent past the first-token deadline; trying another account");
                    if let Some(c) = tracker.usage_tap() {
                        c.headers(&HeaderMap::new(), 504);
                    }
                    acct.cool(Some(&model), Utc::now() + Duration::minutes(2), &msg);
                    app.broadcast("accounts", Value::Null);
                    tried.push(acct.id.clone());
                    last_error = Some((504, formats::error_body(call.format, 504, &msg)));
                    continue;
                }
            }
        }
        acct.record_ok();
        let account = acct.label.clone();
        let req = Arc::new(parsed.take().unwrap_or_default());

        let unwrap = provider == Provider::Antigravity;
        if passthrough {
            if let Some(body) = body {
                if call.stream {
                    return Reply::Stream { frames: passthrough_stream(body, native, tracker, unwrap), account };
                }
                // Codex only streams: rebuild the final response object.
                return collect_passthrough(body, native, tracker, call.format).await;
            }
            let resp = resp.take().expect("response");
            let text = match resp.text().await {
                Ok(text) => text,
                Err(e) => {
                    tracker.diagnostic_failure(crate::diagnostics::Failure::http("upstream_body_read", &e));
                    let message = format!("upstream response read failed: {e}");
                    tracker.finish(502, &Usage::default(), Some(message.clone()));
                    return error_reply(call.format, 502, &message);
                }
            };
            let mut v: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                // An unlabelled stream after all: rebuild the final object from it.
                Err(_) if text.contains("data:") => {
                    let body = futures::stream::once(async move { Ok(bytes::Bytes::from(text)) });
                    return collect_passthrough(Box::pin(body), native, tracker, call.format).await;
                }
                Err(_) => {
                    let message = "upstream returned an invalid JSON response";
                    tracker.finish(502, &Usage::default(), Some(message.into()));
                    return error_reply(call.format, 502, message);
                }
            };
            if unwrap {
                v = crate::antigravity::unwrap(v);
            }
            tracker.observe_wire(&v);
            let mut agg = Aggregate::default();
            checked_full_events(native, &v).iter().for_each(|e| agg.push(e));
            if let Some((status, message)) = &agg.error {
                tracker.finish(*status, &agg.usage, Some(message.clone()));
                return error_reply(call.format, *status, message);
            }
            tracker.finish(status, &agg.usage, None);
            return Reply::Json(v);
        }

        let events = if devin {
            crate::devin::event_stream(resp.take().expect("response"), names)
        } else if let Some(body) = body {
            sse_events(body, native, names, acct.clone(), model.clone(), tracker.usage_tap())
        } else {
            full_events(resp.take().expect("response"), native, names, acct.clone(), model.clone(), tracker.usage_tap())
        };
        let client_model = model.clone();
        if call.stream {
            let frames = render_stream(events, call.format, client_model, req, tracker);
            return Reply::Stream { frames, account };
        }
        return collect(events, call.format, &client_model, &req, tracker).await;
    }

    let (status, body) = last_error.unwrap_or_else(|| {
        (404, formats::error_body(call.format, 404, &format!("no available account serves model `{model}`")))
    });
    tracker.finish(status, &Usage::default(), Some(error_message(&body.to_string())));
    Reply::Error(status, body)
}

// -------------------------------------------------------------------- streams

type EventStream = Pin<Box<dyn Stream<Item = Event> + Send>>;

const PREMATURE_END: &str = "upstream ended before completing the response";

fn premature_end() -> Event {
    Event::Error { status: 502, message: PREMATURE_END.into() }
}

/// A clean transport EOF is not evidence that the model completed its turn.
fn require_completion(mut events: EventStream) -> EventStream {
    Box::pin(async_stream::stream! {
        let mut finished = false;
        while let Some(event) = events.next().await {
            let failed = matches!(event, Event::Error { .. });
            finished |= matches!(event, Event::Finish(_));
            yield event;
            if failed { return; }
        }
        if !finished { yield premature_end(); }
    })
}

fn stream_error_frame(format: Format, status: u16, message: &str) -> Frame {
    let mut body = formats::error_body(format, status, message);
    if format == Format::Responses {
        body["type"] = "error".into();
        body["status"] = status.into();
    }
    Frame::named("error", body.to_string())
}

/// Full-body parsers assume a valid completed object; validate that assumption
/// before allowing their default finish reasons to turn partial JSON into success.
fn checked_full_events(format: Format, body: &Value) -> Vec<Event> {
    let mut events = formats::full_to_events(format, body);
    let response = if format == Format::Responses && body["response"].is_object() { &body["response"] } else { body };
    if response["error"].is_object() || (format == Format::Responses && response["status"] == "failed") {
        events.retain(|e| !matches!(e, Event::Finish(_) | Event::Error { .. }));
        events.push(Event::Error {
            status: response["error"]["code"].as_u64().filter(|s| (400..=599).contains(s)).unwrap_or(502) as u16,
            message: if response["error"].is_object() {
                error_message(&response.to_string())
            } else {
                "upstream response failed".into()
            },
        });
        return events;
    }
    let valid = match format {
        Format::Chat => body["choices"][0]["finish_reason"].is_string(),
        Format::Claude => body["stop_reason"].is_string(),
        Format::Responses => {
            matches!(response["status"].as_str(), Some("completed" | "incomplete"))
        }
        // Gemini's parser already requires an explicit finishReason.
        Format::Gemini => true,
    };
    if !valid || !events.iter().any(|e| matches!(e, Event::Finish(_) | Event::Error { .. })) {
        events.retain(|e| !matches!(e, Event::Finish(_)));
        events.push(premature_end());
    }
    events
}

/// At most this many accounts answer one request, whatever `request-retry` says.
const MAX_ACCOUNTS: usize = 10;
/// After this long, a failed request stops starting new attempts and answers instead.
const RETRY_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);
/// How long a stream that already sent events is held back waiting for its content.
/// Rate limits and overloads inside a 200 arrive at once; past this, it streams as is.
const BOOT_HOLD: std::time::Duration =
    if cfg!(test) { std::time::Duration::from_millis(300) } else { std::time::Duration::from_secs(5) };

fn stalled_message(limit: Option<std::time::Duration>) -> String {
    format!("upstream sent nothing within {}s", limit.map_or(0, |d| d.as_secs()))
}

/// What the first moments of a streamed answer showed.
enum Boot {
    /// The stream, with everything read so far put back in front.
    Ready(ByteStream),
    /// It failed before the client saw anything, so another account can answer.
    /// `held` is what it sent first, for the usage it reported.
    Failed { status: u16, message: String, held: Vec<bytes::Bytes> },
    /// Not a single event before the first-token deadline.
    Stalled,
}

/// Holds a stream back until its first content (text, reasoning or a tool call), its
/// end or an error, so a rate limit or overload that arrives inside a 200 response
/// can go to another account before the client sees anything. The first event must
/// come within `first_event`; after it, content gets `BOOT_HOLD` before the stream is
/// let through as it is.
async fn bootstrap(mut body: ByteStream, native: Format, first_event: Option<std::time::Duration>) -> Boot {
    let start = tokio::time::Instant::now();
    let stall_at = first_event.map(|d| start + d);
    let mut release_at = stall_at.is_none().then(|| start + BOOT_HOLD);
    let mut held: Vec<bytes::Bytes> = Vec::new();
    let mut dec = SseDecoder::default();
    let mut parser = formats::parser(native);
    let mut events = Vec::new();
    let ready = |held: Vec<bytes::Bytes>, rest: ByteStream| -> Boot {
        Boot::Ready(Box::pin(futures::stream::iter(held.into_iter().map(Ok)).chain(rest)))
    };
    loop {
        let Some(deadline) = release_at.or(stall_at) else { return ready(held, body) };
        let next = match tokio::time::timeout_at(deadline, body.next()).await {
            Ok(next) => next,
            Err(_) if release_at.is_none() => return Boot::Stalled,
            Err(_) => return ready(held, body),
        };
        let chunk = match next {
            Some(Ok(chunk)) => chunk,
            // The connection failed or ended before any content: nothing reached the client.
            Some(Err(e)) => return Boot::Failed { status: 502, message: format!("upstream stream error: {e}"), held },
            None => return Boot::Failed { status: 502, message: PREMATURE_END.into(), held },
        };
        held.push(chunk.clone());
        for sse in dec.push(&chunk) {
            release_at.get_or_insert_with(|| tokio::time::Instant::now() + BOOT_HOLD);
            let parsed = serde_json::from_str::<Value>(&sse.data).ok();
            parser.feed_value(&sse, parsed.as_ref(), &mut events);
        }
        for ev in events.drain(..) {
            match ev {
                Event::Error { status, message } if matches!(status, 429 | 500 | 502 | 503 | 504 | 529) => {
                    return Boot::Failed { status, message, held };
                }
                Event::Error { .. } | Event::Finish(_) => return ready(held, body),
                ev if is_content(&ev) => return ready(held, body),
                _ => {}
            }
        }
    }
}

/// Decodes a complete (non-streamed) upstream body into IR events.
fn full_events(
    resp: reqwest::Response,
    native: Format,
    names: HashMap<String, String>,
    acct: Arc<Account>,
    model: String,
    usage_tap: Option<crate::usage::capture::Capture>,
) -> EventStream {
    let rename = renamer(names);
    Box::pin(async_stream::stream! {
        let text = match resp.text().await {
            Ok(text) => text,
            Err(e) => {
                if let Some(c) = &usage_tap { c.failure(crate::diagnostics::Failure::http("upstream_body_read", &e)); }
                yield Event::Error { status: 502, message: format!("upstream response read failed: {e}") };
                return;
            }
        };
        let evs = match serde_json::from_str::<Value>(&text) {
            Ok(v) => {
                observe_quota_value(&acct, &model, &v);
                if let Some(c) = &usage_tap { c.wire(&v); }
                checked_full_events(native, &v)
            }
            // Mislabelled stream: decode it as SSE after all.
            Err(_) => {
                let mut dec = SseDecoder::default();
                let mut parser = formats::parser(native);
                let mut out = Vec::new();
                for sse in dec.push(text.as_bytes()).into_iter().chain(dec.finish()) {
                    let parsed = observe_sse(&acct, &model, usage_tap.as_ref(), &sse);
                    parser.feed_value(&sse, parsed.as_ref(), &mut out);
                }
                out
            }
        };
        for ev in evs {
            yield rename(ev);
        }
    })
}

fn renamer(names: HashMap<String, String>) -> impl Fn(Event) -> Event + Send {
    move |ev: Event| match ev {
        Event::ToolStart { key, id, name } => {
            let name = names.get(&name).cloned().unwrap_or(name);
            Event::ToolStart { key, id, name }
        }
        other => other,
    }
}

/// Decodes an upstream event stream into IR events.
fn sse_events(
    mut body: ByteStream,
    native: Format,
    names: HashMap<String, String>,
    acct: Arc<Account>,
    model: String,
    usage_tap: Option<crate::usage::capture::Capture>,
) -> EventStream {
    let rename = renamer(names);
    Box::pin(async_stream::stream! {
        let mut dec = SseDecoder::default();
        let mut parser = formats::parser(native);
        let mut out = Vec::new();
        let mut finished = false;
        loop {
            match body.next().await {
                Some(Ok(chunk)) => {
                    for sse in dec.push(&chunk) {
                        let parsed = observe_sse(&acct, &model, usage_tap.as_ref(), &sse);
                        parser.feed_value(&sse, parsed.as_ref(), &mut out);
                    }
                }
                Some(Err(e)) => {
                    if finished { return; }
                    if let Some(c) = &usage_tap { c.failure(crate::diagnostics::Failure::http("upstream_body_read", &e)); }
                    out.push(Event::Error { status: 502, message: format!("upstream stream error: {e}") });
                    for ev in out.drain(..) { yield rename(ev); }
                    return;
                }
                None => {
                    for sse in dec.finish() {
                        let parsed = observe_sse(&acct, &model, usage_tap.as_ref(), &sse);
                        parser.feed_value(&sse, parsed.as_ref(), &mut out);
                    }
                    for ev in out.drain(..) { yield rename(ev); }
                    return;
                }
            }
            for ev in out.drain(..) {
                finished |= matches!(ev, Event::Finish(_));
                yield rename(ev);
            }
        }
    })
}

/// Quota, usage and the stream parser all read the event's JSON: parse it once, here.
fn observe_sse(
    acct: &Account,
    model: &str,
    usage_tap: Option<&crate::usage::capture::Capture>,
    sse: &crate::sse::SseEvent,
) -> Option<Value> {
    let v = serde_json::from_str::<Value>(&sse.data).ok()?;
    observe_quota_value(acct, model, &v);
    if let Some(c) = usage_tap {
        c.wire(&v);
    }
    Some(v)
}

fn is_content(ev: &Event) -> bool {
    matches!(ev, Event::Text(_) | Event::Reasoning(_) | Event::ToolStart { .. })
}

fn render_stream(
    events: EventStream,
    format: Format,
    model: String,
    req: Arc<Request>,
    mut tracker: Tracker,
) -> FrameStream {
    let mut renderer = formats::renderer(format, &model, &req);
    Box::pin(async_stream::stream! {
        let mut events = require_completion(events);
        let mut frames = Vec::new();
        while let Some(ev) = events.next().await {
            tracker.observe_stream_event(&ev);
            renderer.push(&ev, &mut frames);
            for f in frames.drain(..) { yield f; }
        }
        // Commit before yielding final frames: the client need not poll for EOF.
        tracker.finish_stream();
        renderer.finish(&mut frames);
        for f in frames.drain(..) { yield f; }
    })
}

async fn collect(events: EventStream, format: Format, model: &str, req: &Request, mut tracker: Tracker) -> Reply {
    let mut events = require_completion(events);
    let mut agg = Aggregate::default();
    while let Some(ev) = events.next().await {
        tracker.observe_stream_event(&ev);
        if is_content(&ev) {
            tracker.first_token();
        }
        agg.push(&ev);
    }
    if let Some((status, msg)) = agg.error.clone() {
        tracker.finish(status, &agg.usage, Some(msg.clone()));
        return error_reply(format, status, &msg);
    }
    tracker.finish(200, &agg.usage, None);
    Reply::Json(formats::render_full(format, &agg, model, req))
}

/// Forwards upstream SSE events untouched while tapping usage.
fn passthrough_stream(mut body: ByteStream, native: Format, mut tracker: Tracker, unwrap: bool) -> FrameStream {
    Box::pin(async_stream::stream! {
        let mut dec = SseDecoder::default();
        let mut parser = formats::parser(native);
        let mut evs = Vec::new();
        let mut transport_error = None;
        loop {
            let (batch, end) = match body.next().await {
                Some(Ok(chunk)) => (dec.push(&chunk), false),
                Some(Err(e)) => {
                    if !tracker.stream_finished { tracker.diagnostic_failure(crate::diagnostics::Failure::http("upstream_body_read", &e)); }
                    transport_error = Some(format!("upstream stream error: {e}"));
                    (dec.finish(), true)
                }
                None => (dec.finish(), true),
            };
            for sse in batch {
                if native == Format::Chat && sse.data.trim() == "[DONE]" {
                    if !tracker.stream_finished && tracker.stream_error.is_none() {
                        tracker.observe_stream_event(&premature_end());
                        tracker.finish_stream();
                        yield stream_error_frame(native, 502, PREMATURE_END);
                    }
                    tracker.finish_stream();
                    yield Frame::data("[DONE]");
                    return;
                }
                // Quota, usage, the stop check and Antigravity's unwrap share one parse.
                let parsed = serde_json::from_str::<Value>(&sse.data).ok();
                if let Some(v) = &parsed {
                    tracker.observe_quota_value(v);
                    tracker.observe_wire(v);
                }
                parser.feed_value(&sse, parsed.as_ref(), &mut evs);
                for ev in evs.drain(..) {
                    tracker.observe_stream_event(&ev);
                }
                let claude_stop = native == Format::Claude
                    && (sse.event.as_deref() == Some("message_stop")
                        || parsed.as_ref().is_some_and(|v| v["type"] == "message_stop"));
                if claude_stop && !tracker.stream_finished && tracker.stream_error.is_none() {
                    tracker.observe_stream_event(&premature_end());
                    tracker.finish_stream();
                    yield stream_error_frame(native, 502, PREMATURE_END);
                    return;
                }
                let data = match parsed {
                    Some(v) if unwrap => crate::antigravity::unwrap(v).to_string(),
                    _ => sse.data,
                };
                yield Frame { event: sse.event.map(std::borrow::Cow::Owned), data };
            }
            if end {
                break;
            }
        }
        if !tracker.stream_finished && tracker.stream_error.is_none() {
            let message = transport_error.unwrap_or_else(|| PREMATURE_END.into());
            tracker.observe_stream_event(&Event::Error { status: 502, message: message.clone() });
            tracker.finish_stream();
            yield stream_error_frame(native, 502, &message);
        }
        tracker.finish_stream();
    })
}

/// Non-streaming client on a stream-only upstream (Codex): return the final response object.
type ByteStream = Pin<Box<dyn Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>;

async fn collect_passthrough(mut body: ByteStream, native: Format, mut tracker: Tracker, format: Format) -> Reply {
    let mut dec = SseDecoder::default();
    let mut parser = formats::parser(native);
    let mut agg = Aggregate::default();
    let mut final_obj: Option<Value> = None;
    let mut evs = Vec::new();
    let mut handle = |sse: crate::sse::SseEvent, agg: &mut Aggregate, final_obj: &mut Option<Value>| {
        let parsed = serde_json::from_str::<Value>(&sse.data).ok();
        if let Some(v) = &parsed {
            tracker.observe_quota_value(v);
            tracker.observe_wire(v);
        }
        parser.feed_value(&sse, parsed.as_ref(), &mut evs);
        evs.drain(..).for_each(|e| agg.push(&e));
        if let Some(mut v) = parsed
            && matches!(v["type"].as_str(), Some("response.completed" | "response.incomplete" | "response.done"))
        {
            *final_obj = Some(v["response"].take());
        }
    };
    while let Some(chunk) = body.next().await {
        match chunk {
            Ok(c) => dec.push(&c).into_iter().for_each(|s| handle(s, &mut agg, &mut final_obj)),
            Err(e) => {
                if agg.finish.is_none() {
                    tracker.diagnostic_failure(crate::diagnostics::Failure::http("upstream_body_read", &e));
                    agg.error = Some((502, format!("upstream stream error: {e}")));
                }
                break;
            }
        }
    }
    dec.finish().into_iter().for_each(|s| handle(s, &mut agg, &mut final_obj));
    if let Some((status, msg)) = agg.error.clone() {
        tracker.finish(status, &agg.usage, Some(msg.clone()));
        return error_reply(format, status, &msg);
    }
    if agg.finish.is_none() {
        tracker.finish(502, &agg.usage, Some(PREMATURE_END.into()));
        return error_reply(format, 502, PREMATURE_END);
    }
    tracker.finish(200, &agg.usage, None);
    match final_obj {
        Some(mut obj) => {
            // Codex omits streamed output from the final event; fill it in.
            if obj["output"].as_array().is_none_or(|a| a.is_empty()) {
                let req = Request::default();
                let rebuilt = formats::responses::render_full(&agg, obj["model"].as_str().unwrap_or_default(), &req);
                obj["output"] = rebuilt["output"].clone();
            }
            Reply::Json(obj)
        }
        None => Reply::Json(formats::render_full(
            format,
            &agg,
            agg.model.as_deref().unwrap_or_default(),
            &Request::default(),
        )),
    }
}

// ------------------------------------------------------------------ utilities

/// `/v1/messages/count_tokens`: ask Claude when possible, otherwise estimate.
pub async fn count_tokens(app: Arc<App>, headers: HeaderMap, body: Value) -> (Value, bool) {
    let cfg = app.cfg();
    let (model, _) = ir::split_model_suffix(body["model"].as_str().unwrap_or_default());
    let (only, model) = app.pool.route(&model);
    let model = app.pool.canonical(&model, only.as_ref());
    let session = crate::affinity::session_key(&headers, &body);
    if let Ok(selected) = app.sessions.pick_with_reason(&app.pool, &cfg, &model, session.as_deref(), &[], only.as_ref())
        && selected.account.provider == Provider::Claude
        && crate::oauth::ensure_fresh(&app, &selected.account, Duration::minutes(5), false).await.is_ok()
    {
        app.sessions.persist_selection(&selected).await;
        let acct = selected.account;
        let upstream_model = selected.model;
        let p = upstream::prepare(
            &Target {
                acct: &acct,
                cfg: &cfg,
                client_headers: &headers,
                model: &upstream_model,
                wire: Format::Claude,
                passthrough: true,
                stream: false,
                count_tokens: true,
                cache_key: None,
            },
            body.clone(),
        );
        let mut rb = app.http.control(acct.proxy_url.as_deref()).post(&p.url);
        for (k, v) in &p.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        if let Ok(resp) = rb.json(&p.body).send().await
            && resp.status().is_success()
            && let Ok(v) = resp.json::<Value>().await
            && v["input_tokens"].is_u64()
        {
            return (v, false);
        }
    }
    (json!({ "input_tokens": estimate_tokens(&body) }), true)
}

pub fn estimate_tokens(body: &Value) -> u64 {
    crate::token_count::estimate_tokens(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuation_deltas_are_bounded_with_or_without_named_sse_events() {
        for named in [false, true] {
            let mut history = History::default();
            let mut frame =
                Frame::data(json!({"type":"response.output_text.delta","delta":"x".repeat(1 << 20)}).to_string());
            if named {
                frame.event = Some("response.output_text.delta".into());
            }
            for _ in 0..7 {
                assert!(history.observe(&frame).is_none());
            }
            assert!(!history.deltas.is_empty());
            assert_eq!(history.buffered, 7 * frame.data.len());
            assert!(!history.overflow);
            for _ in 0..3 {
                history.observe(&frame);
            }
            assert!(history.overflow, "named: {named}");
            assert!(history.deltas.is_empty(), "oversized history must be discarded");
            // Once discarded, later small deltas must not produce a partial history.
            history.observe(&Frame::data(json!({"type":"response.output_text.delta","delta":"tail"}).to_string()));
            assert!(history.deltas.is_empty());
        }
    }

    #[test]
    fn bounded_continuation_deltas_still_rebuild_the_completed_output() {
        for named in [false, true] {
            let mut history = History::default();
            for delta in ["hello", " world"] {
                let mut frame = Frame::data(
                    json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":delta})
                        .to_string(),
                );
                if named {
                    frame.event = Some("response.output_text.delta".into());
                }
                history.observe(&frame);
            }
            let response = history
                .observe(&Frame::data(json!({"type":"response.completed","response":{"output":[]}}).to_string()))
                .unwrap();
            assert_eq!(response["output"][0]["content"][0]["text"], "hello world", "named: {named}");
        }
    }

    #[tokio::test]
    async fn retry_budget_bounds_soft_retries_models_and_waits() {
        let mut budget = RetryBudget::new(2);
        assert!(budget.take("one", "a"));
        assert!(budget.take("one", "a")); // A soft retry preserves its account choice.
        assert!(budget.take("two", "a"));
        assert!(!budget.take("three", "a"));
        assert!(!budget.next_model());
        for _ in 3..MAX_ACCOUNTS {
            assert!(budget.take("two", "a"));
        }
        assert!(!budget.take("two", "a"));
        let mut budget = RetryBudget::new(10);
        budget.started = Instant::now() - (RETRY_BUDGET - std::time::Duration::from_secs(1));
        assert!(!budget.wait(std::time::Duration::from_secs(2)).await);
        budget.started = Instant::now() - RETRY_BUDGET;
        assert!(!budget.take("one", "a"));
    }

    #[test]
    fn soft_failures_retry_and_quota_does_not() {
        let h = reqwest::header::HeaderMap::new();
        let busy = r#"{"error":{"code":429,"message":"Resource has been exhausted (e.g. check quota).","status":"RESOURCE_EXHAUSTED"}}"#;
        assert!(soft_failure(Provider::Antigravity, 429, &h, busy).is_some());
        assert!(soft_failure(Provider::Claude, 429, &h, busy).is_none());
        let quota = r#"{"error":{"status":"RESOURCE_EXHAUSTED","details":[{"reason":"QUOTA_EXHAUSTED"}]}}"#;
        assert!(soft_failure(Provider::Antigravity, 429, &h, quota).is_none());
        assert!(soft_failure(Provider::Antigravity, 503, &h, "No capacity available").is_some());
        let mut later = reqwest::header::HeaderMap::new();
        later.insert("retry-after", "3600".parse().unwrap());
        assert!(soft_failure(Provider::Codex, 429, &later, "{}").is_none());
        let mut soon = reqwest::header::HeaderMap::new();
        soon.insert("retry-after", "2".parse().unwrap());
        assert!(soft_failure(Provider::Codex, 429, &soon, "{}").is_some());
        assert!(soft_failure(Provider::Codex, 400, &h, "{}").is_none());
    }

    #[test]
    fn only_confirmed_subscription_exhaustion_triggers_migration() {
        let cfg = crate::config::Config {
            auth_dir: "/nonexistent".into(),
            codex_api_key: vec![crate::config::KeyEntry { api_key: "test".into(), ..Default::default() }],
            ..Default::default()
        };
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        let account = pool.all().remove(0);
        for code in ["rate_limit_exceeded", "RESOURCE_EXHAUSTED", "context_length_exceeded", "invalid_api_key"] {
            assert!(!quota_exhausted(&account, "gpt-6.1-sol", 429, &json!({"error":{"code":code}}).to_string()));
        }
        for code in ["usage_limit_reached", "insufficient_quota", "QUOTA_EXHAUSTED"] {
            assert!(quota_exhausted(&account, "gpt-6.1-sol", 429, &json!({"error":{"code":code}}).to_string()));
        }
        assert!(!quota_exhausted(&account, "gpt-6.1-sol", 400, "context length exceeded"));
    }
}
