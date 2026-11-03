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
#[path = "cache_diagnostic_tests.rs"]
mod cache_diagnostic_tests;
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

/// Reuse encoded preparation for same-account retries. Bytes clone cheaply;
/// keeping only the latest preparation bounds memory to one payload per request.
type PreparationKey = (String, String, Format, String);
type PreparationCache = Option<(PreparationKey, Arc<WireRequest>)>;

struct WireRequest {
    url: String,
    headers: Vec<(String, String)>,
    payload: bytes::Bytes,
    policy: crate::cache::Policy,
    names: HashMap<String, String>,
}

/// The same upstream value feeds accounting and sanitized cache diagnostics.
#[derive(Clone)]
struct WireTap {
    usage: Option<crate::usage::capture::Capture>,
    diagnostics: crate::cache::DiagnosticObserver,
}

impl WireTap {
    fn wire(&self, value: &Value) {
        self.diagnostics.observe(value);
        if let Some(capture) = &self.usage {
            capture.wire(value);
        }
    }
}

// --------------------------------------------------------------------- tracker

/// Records the request in stats when finished (or dropped mid-stream).
pub struct Tracker {
    app: Arc<App>,
    log: RequestLog,
    started: Instant,
    attempt_started: Instant,
    prepared_ms: Option<u64>,
    headers_ms: Option<u64>,
    acct: Option<Arc<Account>>,
    /// Stream progress must outlive the suspended generator when its body is dropped.
    stream_usage: Usage,
    cache_diagnostics: crate::cache::DiagnosticObserver,
    stream_error: Option<(u16, String)>,
    stream_finished: bool,
    load: Option<crate::accounts::RequestLoad>,
    done: bool,
    analytics: Option<crate::usage::capture::RequestUsage>,
    downstream: crate::diagnostics::Downstream,
    logical_final: bool,
    latency_context: Option<crate::latency::Context>,
    latency_model: Option<String>,
    batch_completeness: Option<&'static str>,
    /// Keeps the request on the dashboard's connected list until it ends.
    connected: Option<crate::addresses::Guard>,
}

impl Tracker {
    /// This request's id in the request log and journal.
    pub fn request_id(&self) -> u64 {
        self.log.id
    }

    pub fn new(app: &Arc<App>, client: Format, stream: bool, transport: &'static str, model: &str) -> Self {
        app.stats.active.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            app: app.clone(),
            analytics: Some(crate::usage::capture::RequestUsage::new(app.usage.clone(), model)),
            downstream: crate::diagnostics::Downstream::current(),
            logical_final: true,
            batch_completeness: None,
            latency_context: None,
            latency_model: None,
            connected: None,
            started: Instant::now(),
            attempt_started: Instant::now(),
            prepared_ms: None,
            headers_ms: None,
            acct: None,
            stream_usage: Usage::default(),
            cache_diagnostics: crate::cache::DiagnosticObserver::default(),
            stream_error: None,
            stream_finished: false,
            load: None,
            done: false,
            log: RequestLog {
                id: app.stats.next_id(),
                ts: Utc::now(),
                client: client.as_str(),
                client_app: None,
                source: None,
                key: None,
                key_label: None,
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
                cache_diagnostics: None,
                control_downgrades: Vec::new(),
                status: 0,
                latency_ms: 0,
                ttft_ms: None,
                input_tokens: 0,
                output_tokens: 0,
                cache_tokens: 0,
                stream,
                transport,
                attempts: 0,
                chunks: 0,
                error: None,
                usage_completeness: "missing",
                failure_kind: None,
            },
        }
    }

    pub fn attempt(&mut self, acct: &Arc<Account>) {
        if let Some(c) = self.usage_tap() {
            self.app.stats.performance.lock().attempt(&c.snapshot(None, None, false), self.log.transport);
        }
        self.attempt_started = Instant::now();
        self.prepared_ms = None;
        self.headers_ms = None;
        if let Some(a) = &mut self.analytics {
            a.attempt(acct);
        }
        // A whole-request attempt replaces an earlier chunked attempt's accounting;
        // those chunks already recorded their own usage.
        if self.log.chunks > 0 {
            self.log.chunks = 0;
            self.batch_completeness = None;
            self.stream_usage = Usage::default();
        }
        self.account_attempt(acct);
    }
    fn account_attempt(&mut self, acct: &Arc<Account>) {
        self.cache_diagnostics.reset();
        self.load = Some(crate::accounts::RequestLoad::new(acct));
        self.log.attempts += 1;
        self.log.provider = acct.provider.as_str().to_string();
        self.log.account = acct.label.clone();
        self.log.account_id = acct.id.clone();
        self.acct = Some(acct.clone());
    }

    pub fn embedding_attempt(&mut self, acct: &Arc<Account>) {
        // End an earlier whole-request attempt the way a normal retry would. Left
        // open, it would be finalized with this attempt's status and usage.
        if let Some(c) = self.usage_tap() {
            self.app.stats.performance.lock().attempt(&c.snapshot(None, None, false), self.log.transport);
        }
        if let Some(a) = &mut self.analytics {
            let _ = a.finish(None, None, false);
        }
        self.attempt_started = Instant::now();
        self.prepared_ms = None;
        self.headers_ms = None;
        self.account_attempt(acct);
        // Concurrent chunks own their individual provider load reservations.
        self.load = None;
    }

    /// A cached result names the account it came from without counting an
    /// attempt or a request against that account.
    pub fn cached(&mut self, acct: &Account) {
        self.log.provider = acct.provider.as_str().to_string();
        self.log.account = acct.label.clone();
        self.log.account_id = acct.id.clone();
    }
    pub fn embedding_child(
        &self,
        acct: &Arc<Account>,
        index: usize,
        model: &str,
    ) -> crate::usage::capture::RequestUsage {
        let mut child = self.analytics.as_ref().expect("unfinished tracker").fork();
        child.attempt(acct);
        if let Some(c) = child.tap() {
            c.metadata("embedding_chunk", index as u64);
            c.model(model);
        }
        child
    }
    pub fn requested_speed(&mut self, body: &Value) {
        if let Some(usage) = &mut self.analytics {
            usage.requested_speed(body);
        }
    }
    pub fn embedding_progress(&mut self, tokens: Option<u64>, estimated: bool) {
        self.log.chunks += 1;
        self.stream_usage.input += tokens.unwrap_or(0);
        let complete = tokens.is_some() && !estimated && self.batch_completeness != Some("partial");
        self.batch_completeness = Some(if complete { "complete" } else { "partial" });
    }

    /// Names the client program from its User-Agent, and the key and address it came
    /// with, for the dashboard.
    pub fn client_app(&mut self, headers: &HeaderMap) {
        self.log.client_app = client_app(headers);
        self.log.source = crate::source::Source::from_headers(headers);
        self.log.key = crate::usage::capture::client_id(headers).filter(|k| !k.is_empty()).map(String::from);
        self.log.key_label = self.log.key.as_deref().map(|k| crate::mgmt::client_label(&self.app.cfg(), k));
        if let Some(source) = &self.log.source {
            self.connected = Some(self.app.addresses.begin(self.log.key.as_deref().unwrap_or(""), source, false));
        }
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
        self.latency_model = Some(selected.model.clone());
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

    pub(crate) fn latency_context(&mut self, context: Option<crate::latency::Context>) {
        self.latency_context = context;
    }

    pub(crate) fn usage_tap(&self) -> Option<crate::usage::capture::Capture> {
        self.analytics.as_ref().and_then(|a| a.tap())
    }
    fn wire_tap(&self) -> WireTap {
        WireTap { usage: self.usage_tap(), diagnostics: self.cache_diagnostics.clone() }
    }
    pub(crate) fn observe_wire(&self, value: &Value) {
        self.wire_tap().wire(value);
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
        self.log.cache_diagnostics = self.cache_diagnostics.snapshot();
        if let Some(c) = self.usage_tap() {
            c.metadata("request_latency_ms", self.started.elapsed().as_millis() as u64);
            if let Some(ttft) = self.log.ttft_ms {
                c.metadata("request_ttft_ms", ttft);
            }
            if let Some(ms) = self.prepared_ms {
                c.metadata("prepare_ms", ms);
            }
            if let Some(ms) = self.headers_ms {
                c.metadata("headers_ms", ms);
            }
            c.metadata("transport_ws", u64::from(self.log.transport == "ws"));
            c.metadata("attempt_number", u64::from(self.log.attempts));
            c.metadata("control_downgrade_count", self.log.control_downgrades.len() as u64);
            if let Some(p) = &self.log.cache_policy {
                c.metadata("cache_requested", crate::cache::intent_code(p.requested));
                c.metadata("cache_applied", crate::cache::intent_code(p.applied));
                c.metadata("cache_downgraded", u64::from(p.downgrade_reason.is_some()));
            }
        }
        let failure = self.usage_tap().and_then(|c| c.failure_detail());
        let captured = self.analytics.as_mut().and_then(|a| a.finish(Some(status), Some(usage), self.logical_final));
        self.log.usage_completeness =
            self.batch_completeness.unwrap_or(match captured.as_ref().map(|o| o.completeness.as_str()) {
                Some("complete") => "complete",
                Some("partial") => "partial",
                _ => "missing",
            });
        self.log.failure_kind = if status == 499 {
            Some(self.downstream.kind())
        } else if status < 400 {
            None
        } else {
            failure.as_ref().map(|f| f.kind).or_else(|| crate::diagnostics::failure_kind(status, error.as_deref()))
        };
        self.load = None;
        self.connected = None;
        self.app.stats.active.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        let usage = if self.log.chunks > 0 { &self.stream_usage } else { usage };
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
            if status < 400
                && self.log.attempts == 1
                && self.log.routing_reason == Some("new_session")
                && captured.as_ref().is_some_and(|o| o.tokens.cache_read == Some(0))
                && let (Some(context), Some(ttft)) = (&self.latency_context, self.log.ttft_ms)
            {
                // Routing compares the resolved upstream name, which may differ
                // from the provider's reported snapshot name.
                let model = self.latency_model.as_ref().unwrap_or(&self.log.model).clone();
                crate::latency::record(&mut st.latency, (model, context.clone()), ttft);
            }
            st.counters.record(&self.log);
            st.last_used = Some(Utc::now());
        }
        {
            let mut performance = self.app.stats.performance.lock();
            if let Some(o) = &captured {
                performance.attempt(o, self.log.transport);
            }
            performance.request(&self.log, captured.as_ref(), self.prepared_ms, self.headers_ms);
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
/// capacity errors (Google's "No capacity available", Codex "at capacity") and
/// rate limits that reset within seconds. Real quota exhaustion is not one of them.
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
            if at_capacity(status, body) {
                // A longer wait is not sat out on the same account (`long_capacity_wait`).
                return match reset_after(headers, body) {
                    Some(until) => short(until),
                    None => Some(std::time::Duration::ZERO),
                };
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

/// Codex answers "Codex is at capacity, please try again later" with a 429 when OpenAI
/// is short of servers, or limits turns with `turn_rate_limit_exceeded`. Like a 529 it
/// says nothing about the subscription, so it is retried and shown but pauses no model:
/// a pause would turn away working sessions. The code counts even when the message
/// doesn't say "at capacity".
pub fn at_capacity(status: u16, body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    status == 429 && ["at capacity", "overloaded", "turn_rate_limit_exceeded"].iter().any(|s| b.contains(s))
}

/// A capacity shortage whose own wait is too long to sit out on the same account (over
/// the ten seconds `soft_failure` waits): the request moves on to another account at
/// once. Codex's turn limit names waits of several minutes, and retries within them fail.
fn long_capacity_wait(status: u16, headers: &reqwest::header::HeaderMap, body: &str) -> bool {
    at_capacity(status, body)
        && reset_after(headers, body).is_some_and(|until| until - Utc::now() > Duration::seconds(10))
}

/// The longest wait a capacity shortage passes on to a client left without an account,
/// so it sits out only this turn.
const CAPACITY_WAIT_LIMIT: u64 = 600;
/// The longest pause a dry pool advertises as Retry-After. Clients sleep through whatever
/// they are told, and Claude Code may give up past a minute.
const ADVERTISED_WAIT_LIMIT: u64 = 60;

/// The Retry-After a capacity shortage passes on to the client, if the provider named one.
fn capacity_retry_after(status: u16, headers: &reqwest::header::HeaderMap, body: &str) -> Option<u64> {
    let until = reset_after(headers, body).filter(|_| at_capacity(status, body))?;
    Some(seconds_until(until).min(CAPACITY_WAIT_LIMIT))
}

/// Whole seconds until `until`, rounded up, at least one.
fn seconds_until(until: chrono::DateTime<Utc>) -> u64 {
    ((until - Utc::now()).num_milliseconds().max(1) as u64).div_ceil(1000)
}

/// The provider's own Retry-After in seconds, for logs.
fn upstream_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers.get("retry-after")?.to_str().ok()?.trim().parse().ok()
}

/// `Reply::Error` has no headers, so a Retry-After rides in the error body under this key
/// until `server::reply` turns it into the header. No client sees the key.
const RETRY_AFTER_KEY: &str = "__fusebox_retry_after";

fn with_retry_after(mut body: Value, seconds: u64) -> Value {
    if let Some(object) = body.as_object_mut() {
        object.insert(RETRY_AFTER_KEY.into(), seconds.into());
    }
    body
}

/// Takes the Retry-After an error reply carries out of its body.
pub fn take_retry_after(body: &mut Value) -> Option<u64> {
    body.as_object_mut()?.remove(RETRY_AFTER_KEY)?.as_u64()
}

/// The Retry-After an error body carries for its client, without removing it.
pub fn retry_after_of(body: &Value) -> Option<u64> {
    body.get(RETRY_AFTER_KEY)?.as_u64()
}

/// Anthropic answers an API key whose prepaid credit ran out with a 400. Only adding
/// credit fixes it, so the key sits out (`refuse`). Subscriptions are left alone: the same
/// text was seen on them with an unclear cause, and a usage reading would undo the pause.
fn credit_exhausted(acct: &Account, status: u16, message: &str) -> bool {
    acct.provider == Provider::Claude
        && !acct.is_oauth()
        && status == 400
        && message.trim_start().to_ascii_lowercase().starts_with("your credit balance is too low")
}

/// ChatGPT answers 502 "response protection is unavailable" to a request it won't run
/// as sent (one replaying web search results without the web search tool). Another try
/// or another account gets the same answer, and the account is fine. Inside a stream the
/// same refusal can come with another server-error status.
pub(crate) fn protection_refused(provider: Provider, status: u16, message: &str) -> bool {
    provider == Provider::Codex
        && (500..600).contains(&status)
        && message.to_ascii_lowercase().contains("response protection is unavailable")
}

/// What keeps every account for a model away, judged by the one back first.
enum Dry {
    /// A refusal only the owner can fix, to give again: status and message.
    Refused(u16, String),
    /// A rate limit, spent quota or a quota check, until then.
    Limit(chrono::DateTime<Utc>),
    /// Anything else, such as a sign-in or permission pause: not a wait to advertise.
    Other,
}

/// None when an account for `model` is free: the request used up its own choices.
fn dry_pool(app: &App, model: &str, only: Option<&crate::accounts::Only>, exclude: &[String]) -> Option<Dry> {
    let mut next: Option<(chrono::DateTime<Utc>, Arc<Account>)> = None;
    for a in app.pool.all() {
        let disabled = a.state.lock().disabled;
        // As at selection: a held account is out until a sign-in, not back at some time.
        if exclude.contains(&a.id) || disabled || a.held_out() || app.pool.resolve_account(&a, model, only).is_none() {
            continue;
        }
        let until = a.cooling_until(model)?;
        if next.as_ref().is_none_or(|(t, _)| until < *t) {
            next = Some((until, a));
        }
    }
    let (until, a) = next?;
    Some(match a.refusal() {
        Some((status, message)) => Dry::Refused(status, message),
        None if a.paused_by_limit(model) => Dry::Limit(until),
        None => Dry::Other,
    })
}

/// The answer for a request left without an account. A 429 tells when the next account
/// is back if a rate limit, spent quota or a quota check is all that keeps it away: within
/// a minute as Retry-After; later with no header, which clients would sleep through for
/// hours, and to OpenAI clients as `usage_limit_reached` with `resets_at`. A sign-in or
/// permission pause is no wait. With `refusals`, a refusal only the owner can fix (a
/// used-up API credit balance) is given instead while it keeps the next account away.
#[allow(clippy::too_many_arguments)]
fn no_account_left(
    app: &App,
    format: Format,
    model: &str,
    only: Option<&crate::accounts::Only>,
    exclude: &[String],
    status: u16,
    body: Value,
    refusals: bool,
) -> (u16, Value) {
    if status != 429 || body.get(RETRY_AFTER_KEY).is_some() {
        return (status, body);
    }
    match dry_pool(app, model, only, exclude) {
        Some(Dry::Refused(status, message)) if refusals => (status, formats::error_body(format, status, &message)),
        Some(Dry::Limit(until)) => {
            let wait = seconds_until(until);
            if wait <= ADVERTISED_WAIT_LIMIT {
                (status, with_retry_after(body, wait))
            } else if matches!(format, Format::Chat | Format::Responses)
                && body["error"]["type"] != "usage_limit_reached"
            {
                let message = error_message(&body.to_string());
                let error = json!({"type": "usage_limit_reached", "code": "usage_limit_reached", "message": message,
                    "resets_at": until.timestamp(), "resets_in_seconds": wait});
                (status, json!({ "error": error }))
            } else {
                (status, body)
            }
        }
        _ => (status, body),
    }
}

/// The failure a stream opened with, before any content: its error object, to match
/// codes, and its `code` and `type` as logs may show them.
struct FirstError {
    raw: String,
    code: String,
    kind: String,
}

impl FirstError {
    /// The failure a stream event reports, if it reports one.
    fn of(event: &Value) -> Option<Self> {
        let (error, typed) = if event["type"] == "response.failed" {
            (&event["response"]["error"], true)
        } else if event["error"].is_object() {
            (&event["error"], true)
        } else if event["type"] == "error" {
            // A bare `error` event carries the code itself; its type names only the event.
            (event, false)
        } else {
            return None;
        };
        let kind = if typed { log_label(&error["type"]) } else { "none" };
        Some(Self { raw: error.to_string(), code: log_label(&error["code"]).into(), kind: kind.into() })
    }
}

/// A provider's error code or type as logs may show it: a plain identifier, or "other"
/// for anything else, so no message or account detail is ever logged.
fn log_label(value: &Value) -> &str {
    match value {
        Value::Null => "none",
        Value::String(s)
            if (1..=64).contains(&s.len())
                && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b)) =>
        {
            s
        }
        _ => "other",
    }
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

/// Meta's 403 for a Muse Code subscription used from another client (it looks at the
/// request's tools). It refuses this request, not the account.
fn attestation_denied(body: &str) -> bool {
    serde_json::from_str::<Value>(body).is_ok_and(|v| v["error"]["code"] == "client_attestation_denied")
}

/// Whether Codex access tokens die before `expires_at`: the refused token's age from its
/// `iat` and the time it had left, in whole hours. Never the token or another claim.
pub(crate) fn log_codex_401(acct: &Account, sent: Option<&str>, request_id: u64) {
    let crate::accounts::Credential::OAuth(o) = &*acct.cred.read() else { return };
    let now = Utc::now();
    let hours = |h: Option<i64>| h.map_or_else(|| "unknown".to_string(), |h| h.to_string());
    // The token the request carried, which a refresh meanwhile may have replaced on file.
    let token = sent.unwrap_or(&o.access_token);
    let claims = crate::oauth::jwt_claims(token);
    let age = claims["iat"].as_i64().map(|iat| (now.timestamp() - iat) / 3600);
    let left = claims["exp"]
        .as_i64()
        .map(|exp| (exp - now.timestamp()) / 3600)
        .or_else(|| o.expires_at.filter(|_| token == o.access_token).map(|t| (t - now).num_hours()));
    tracing::info!(request_id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
        token_age_hours = %hours(age), expires_in_hours = %hours(left), "Codex refused its access token");
}

/// The bearer token a prepared request carries.
fn bearer(headers: &[(String, String)]) -> Option<&str> {
    let (_, value) = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("authorization"))?;
    value.strip_prefix("Bearer ")
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
    let cfg = app.cfg();
    if let Err(message) = upstream::residency_requirement(&call.headers) {
        return error_reply(call.format, 400, &message);
    }
    if crate::clients::observe_trusted(&call.headers, crate::policy::trusted_identity_source(&cfg, &call.headers)) {
        app.models.save_clients();
    }
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
    let mut continuation_history = None;
    if call.format == Format::Responses
        && let Some(id) = call.body["previous_response_id"].as_str()
        && let Some((session, mut previous)) = app.sessions.previous(&call.headers, id, call.session.as_deref())
    {
        previous.extend(crate::affinity::input_items(&call.body));
        call.body["input"] = Value::Array(previous.to_values());
        continuation_history = Some(previous);
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
    let mut full = continuation_history.unwrap_or_else(|| {
        crate::affinity::History::from(
            capture.as_ref().map(|_| crate::affinity::input_items(&call.body)).unwrap_or_default(),
        )
    });
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
    /// Accounts no model of this request may reach, apart from those it tried: Meta's,
    /// once Meta refused this client's tools. Kept across a model group's models.
    skipped: Vec<String>,
    /// The answer refuses the request itself, so a model group stops there too.
    terminal: bool,
}

impl RetryBudget {
    fn new(attempts: u32) -> Self {
        Self {
            started: Instant::now(),
            remaining_accounts: (attempts as usize).clamp(1, MAX_ACCOUNTS),
            remaining_attempts: MAX_ACCOUNTS,
            selected: Default::default(),
            skipped: Vec::new(),
            terminal: false,
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
        let mut reply = execute_inner(app.clone(), attempt, &mut budget).await;
        // A model left without accounts by this request's own refusal (Meta's) adds nothing:
        // the refusal stays the answer.
        if matches!(reply, Reply::Error(404, _))
            && !budget.skipped.is_empty()
            && let Some(earlier) = last.take()
        {
            reply = earlier;
        }
        if i + 1 < order.len() && !budget.terminal && group_moves_on(&reply) && budget.next_model() {
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
    let residency = upstream::residency_requirement(&call.headers).ok().flatten();
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
    tracker.requested_speed(&call.body);
    tracker.session(call.session.as_deref(), call.session_source, &cfg);
    let requested_cache = crate::cache::Requested::of(&call.body);
    tracker.latency_context = cfg.latency_routing.then(|| crate::latency::Context::new(&call.body, call.transport));

    let mut preparation_cache: PreparationCache = None;
    let mut parsed: Option<Request> = None;
    let mut shortened: Option<(Request, HashMap<String, String>)> = None;
    let mut tried: Vec<String> = Vec::new();
    let mut residency_excluded: Vec<_> = if residency.is_some() {
        app.pool
            .all()
            .into_iter()
            .filter(|a| a.provider != Provider::Codex || !a.is_oauth())
            .map(|a| a.id.clone())
            .collect()
    } else {
        Vec::new()
    };
    // Codex asks for remote compaction by provider, not model, and Kimi and Meta can't
    // compact: their accounts sit out such a request rather than fail it.
    if call.format == Format::Responses
        && call.body["input"].as_array().is_some_and(|items| items.iter().any(|i| i["type"] == "compaction_trigger"))
    {
        let serving: Vec<_> = app
            .pool
            .all()
            .into_iter()
            .filter(|a| {
                let disabled = a.state.lock().disabled;
                !residency_excluded.contains(&a.id)
                    && !disabled
                    && !a.held_out()
                    && app.pool.resolve_account(a, &model, only.as_ref()).is_some()
            })
            .collect();
        let cannot = |a: &Account| matches!(a.provider, Provider::Kimi | Provider::Meta);
        if !serving.is_empty() && serving.iter().all(|a| cannot(a)) {
            let message = format!(
                "{model} can't compact a Codex conversation remotely: Kimi and Meta don't support it. Point Codex at this model through a model provider entry of its own with `capabilities.remote_compaction = \"unsupported\"`, so Codex compacts locally"
            );
            tracker.finish(400, &Usage::default(), Some(message.clone()));
            return error_reply(call.format, 400, &message);
        }
        residency_excluded.extend(serving.iter().filter(|a| cannot(a)).map(|a| a.id.clone()));
    }
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
        let excluded: Vec<_> = tried.iter().chain(&residency_excluded).chain(&budget.skipped).cloned().collect();
        // Past the budget, the error so far is the answer rather than another wait.
        if !budget.available() {
            break;
        }
        let pin = retry_same.take();
        // A retry on a temporary detour must stay on that account without
        // replacing the session's long-lived assignment to its original account.
        let retry = pin.as_deref().and_then(|id| app.pool.get(id)).and_then(|account| {
            if tried.contains(&account.id)
                || account.state.lock().disabled
                || account.held_out()
                || account.cooling_until(&model).is_some()
            {
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
            app.sessions.pick_session_with_context(
                &app.pool,
                &cfg,
                &model,
                call.session.as_deref(),
                inferred,
                &excluded,
                only.as_ref(),
                tracker.latency_context.as_ref(),
            )
        } else {
            match app.sessions.pick_unbound(&app.pool, &cfg, &model, &excluded, pin.as_deref(), only.as_ref()) {
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
            Err((mut status, mut msg)) => {
                if residency.is_some() && status == 404 {
                    status = 400;
                    msg = "Requested Codex residency requires an available Codex subscription route".into();
                }
                // Judged by the accounts this request could reach, including those it tried.
                let unreachable: Vec<_> = residency_excluded.iter().chain(&budget.skipped).cloned().collect();
                let left = |status, body, refusals| {
                    no_account_left(&app, call.format, &model, only.as_ref(), &unreachable, status, body, refusals)
                };
                if let Some((s, b)) = last_error {
                    let (s, b) = left(s, b, false);
                    tracker.finish(s, &Usage::default(), Some(error_message(&b.to_string())));
                    return Reply::Error(s, b);
                }
                let (status, body) = left(status, formats::error_body(call.format, status, &msg), true);
                tracker.finish(status, &Usage::default(), Some(error_message(&body.to_string())));
                return Reply::Error(status, body);
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
            // Waiting for a sign-in already keeps it out; a cooldown would hide why.
            if !acct.held_out() {
                acct.cool(None, Utc::now() + Duration::minutes(5), &format!("token refresh failed: {e}"));
            }
            tried.push(acct.id.clone());
            last_error = Some((401, formats::error_body(call.format, 401, &format!("token refresh failed: {e}"))));
            continue;
        }

        let quota_epoch = acct.quota_epoch();
        let provider = acct.provider;
        if residency.is_some() && (provider != Provider::Codex || !acct.is_oauth()) {
            let message = "Requested Codex residency requires a Codex subscription route; this route cannot honor it";
            tracker.finish(400, &Usage::default(), Some(message.into()));
            return error_reply(call.format, 400, message);
        }
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
        // Upstream streaming: always when translating (we re-render), and for Codex OAuth.
        let upstream_stream = !passthrough || call.stream || (provider == Provider::Codex && acct.is_oauth());
        let provider_session =
            app.sessions.provider_session(call.session.as_deref(), &acct, cfg.provider_session_idle_seconds);
        let preparation_key = (
            acct.id.clone(),
            upstream_model.clone(),
            native,
            format!("{}:{provider_session}", upstream::credential_stamp(&acct)),
        );
        // Antigravity and Devin stamp each prepared request with fresh request and
        // session ids, so a retry must prepare again rather than resend them.
        let memoizable = !matches!(provider, Provider::Antigravity | Provider::Devin);
        let prepared = if let Some((_, cached)) =
            preparation_cache.as_ref().filter(|(key, _)| memoizable && key == &preparation_key)
        {
            cached.clone()
        } else {
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
                let hoisted = (devin || !matches!(native, Format::Chat | Format::Responses))
                    .then(|| formats::hoist_instructions(req))
                    .flatten();
                let req = hoisted.as_ref().unwrap_or(req);
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
            if !passthrough {
                let openai = provider == Provider::Codex
                    && (upstream_model.starts_with("gpt-5") || upstream_model.starts_with("gpt-6"));
                let mut dropped =
                    formats::generation_controls(&mut body, native, openai, cfg.translation.allow_service_tier);
                let verbosity_requested = call.format == Format::Chat && call.body["verbosity"].is_string()
                    || call.format == Format::Responses && call.body["text"]["verbosity"].is_string();
                if verbosity_requested && !matches!(native, Format::Chat | Format::Responses) {
                    dropped.push("verbosity_unsupported");
                }
                if matches!(call.format, Format::Chat | Format::Responses)
                    && call.body["service_tier"].is_string()
                    && !matches!(native, Format::Chat | Format::Responses)
                {
                    dropped.push("service_tier_not_enabled");
                }
                tracker.log.control_downgrades = dropped.clone();
                for reason in dropped {
                    if crate::diagnostics::warn_once(reason, std::time::Duration::from_secs(600)) {
                        tracing::warn!(request_id = tracker.log.id, reason, "translated generation control dropped");
                    }
                }
            }
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
            if let Some(reason) = &cache_downgrade
                && cfg.prompt_cache.on_incompatible == crate::cache::Incompatible::Reject
            {
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
            if native == Format::Claude
                && !devin
                && let Err(message) = formats::claude::validate_native_thinking_budget(&body, &upstream_model)
            {
                tracker.finish(400, &Usage::default(), Some(message.clone()));
                return error_reply(call.format, 400, &message);
            }
            let cache_defaults = passthrough
                || (!requested_cache.explicit_mode && !requested_cache.markers)
                || (cache_downgrade.is_some()
                    && cfg.prompt_cache.on_incompatible == crate::cache::Incompatible::Automatic
                    && requested_cache.markers);

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
                    cache_defaults,
                    provider_session: Some(&provider_session),
                },
                body,
            );
            let mut cache_policy = crate::cache::Policy::for_request(
                &requested_cache,
                &prepared.body,
                native,
                cache_scope,
                cache_downgrade,
            );
            if prepared.headers.iter().any(|(name, _)| name == "x-grok-conv-id") {
                cache_policy.key_scope = cache_scope;
            }
            if cfg.debug {
                tracing::debug!(provider = provider.as_str(), "upstream request prepared");
            }

            let payload = prepared.raw.unwrap_or_else(|| serde_json::to_vec(&prepared.body).unwrap_or_default());
            let wire = Arc::new(WireRequest {
                url: prepared.url,
                headers: prepared.headers,
                payload: payload.into(),
                policy: cache_policy,
                names,
            });
            preparation_cache = Some((preparation_key, wire.clone()));
            wire
        };
        let names = prepared.names.clone();
        tracker.cache_policy(prepared.policy.clone());
        let client = app.http.for_account(&acct);
        let mut rb = client.post(&prepared.url);
        for (k, v) in &prepared.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        let payload = prepared.payload.clone();
        let absolute_deadline = (cfg.stream_limits.pre_output_timeout_seconds > 0).then(|| {
            tokio::time::Instant::now() + std::time::Duration::from_secs(cfg.stream_limits.pre_output_timeout_seconds)
        });
        tracker.prepared_ms = Some(tracker.attempt_started.elapsed().as_millis() as u64);
        let send = rb.body(payload).send();
        // A stream's headers come before its first token; an upstream that hasn't even
        // sent those by the first-token deadline has stalled.
        let header_limit = first_token
            .filter(|_| upstream_stream)
            .into_iter()
            .chain(absolute_deadline.map(|d| d.saturating_duration_since(tokio::time::Instant::now())))
            .min();
        let sent = match header_limit {
            Some(limit) => tokio::time::timeout(limit, send).await.ok(),
            None => Some(send.await),
        };
        let Some(sent) = sent else {
            if absolute_deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
                let msg = "upstream pre-output deadline exceeded; inference may still be running";
                tracker.finish(504, &Usage::default(), Some(msg.into()));
                return error_reply(call.format, 504, msg);
            }
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
        tracker.headers_ms = Some(tracker.attempt_started.elapsed().as_millis() as u64);
        let resp = match sent {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("upstream connection failed: {e}");
                let failure = crate::diagnostics::Failure::http("upstream_connect", &e);
                tracker.diagnostic_failure(failure.clone());
                tracing::warn!(request_id = tracker.log.id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
                    failure_kind = failure.kind, error_causes = ?failure.causes, "upstream connection failed");
                acct.state.lock().set_temporary_error(msg.clone(), Utc::now() + Duration::seconds(30));
                app.broadcast("accounts", Value::Null);
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
                formats::translated_error_body(call.format, native, status, &msg)
            };
            // The request itself is refused: answered at once, and the account is fine.
            if protection_refused(provider, status, &msg) {
                let failure = crate::diagnostics::Failure::classified("response_protection_unavailable");
                tracker.diagnostic_failure(failure.clone());
                tracing::warn!(request_id = tracker.log.id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
                    status, failure_kind = failure.kind, "upstream refused the request as sent; answering without a retry");
                budget.terminal = true;
                tracker.finish(status, &Usage::default(), Some(msg));
                return Reply::Error(status, client_body);
            }
            if quota_exhausted(&acct, &model, status, &text) {
                mark_quota_exhausted(&acct, &model, &headers, &text);
                app.broadcast("accounts", Value::Null);
                tried.push(acct.id.clone());
                last_error = Some((status, client_body));
                continue;
            }
            if credit_exhausted(&acct, status, &msg) {
                let reason = format!("API credit balance too low: {msg}");
                acct.refuse(Utc::now() + Duration::minutes(30), &reason, status, &msg);
                app.broadcast("accounts", Value::Null);
                tried.push(acct.id.clone());
                last_error = Some((status, client_body));
                continue;
            }
            // A capacity shortage names the client's wait, should no other account answer.
            let client_wait = capacity_retry_after(status, &headers, &text);
            let client_body = match client_wait {
                Some(wait) => with_retry_after(client_body, wait),
                None => client_body,
            };
            if long_capacity_wait(status, &headers, &text) {
                // Retrying within the wait fails, and a pause would turn away the account's
                // other sessions: only this request moves on.
                tracing::info!(request_id = tracker.log.id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
                    status, upstream_retry_after_seconds = upstream_retry_after(&headers),
                    client_retry_after_seconds = client_wait, "upstream at capacity for longer than a retry; trying another account");
                acct.state.lock().set_temporary_error(format!("{status}: {msg}"), Utc::now() + Duration::seconds(30));
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
                    let upstream_wait = upstream_retry_after(&headers).filter(|_| at_capacity(status, &text));
                    tracing::info!(request_id = tracker.log.id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
                        status, retry_delay_seconds = wait.as_secs(), upstream_retry_after_seconds = upstream_wait,
                        "upstream busy; retrying");
                    last_error = Some((status, client_body));
                    if !budget.wait(wait).await {
                        break;
                    }
                    retry_same = Some(acct.id.clone());
                    continue;
                }
                // Still busy: step aside briefly, without the escalating backoff. A capacity
                // shortage only fails this request; the account's other sessions carry on.
                if status == 429 && !at_capacity(status, &text) {
                    acct.rate_limit(&model, Utc::now() + Duration::seconds(15), &format!("429: {msg}"));
                } else {
                    acct.state
                        .lock()
                        .set_temporary_error(format!("{status}: {msg}"), Utc::now() + Duration::seconds(30));
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
                    acct.rate_limit(&model, until, &format!("429: {msg}"));
                    app.broadcast("accounts", Value::Null);
                }
                // Meta refuses this client's tools, not the account: no new key, pause or
                // account error. Its other accounts would refuse the same, so only another
                // provider's route for this model may serve it; otherwise Meta's 403 answers.
                403 if provider == Provider::Meta && attestation_denied(&text) => {
                    tracing::info!(request_id = tracker.log.id, "Meta refused this client's tools; skipping Meta");
                    let meta = app.pool.all().into_iter().filter(|a| a.provider == Provider::Meta);
                    budget.skipped.extend(meta.map(|a| a.id.clone()));
                    // Not the account's failure, so not in its failed-requests count.
                    tracker.log.account_id.clear();
                }
                401 | 403 if acct.is_oauth() && !refreshed.contains(&acct.id) && !forbidden_for_good(&text) => {
                    if status == 401 && provider == Provider::Codex {
                        log_codex_401(&acct, bearer(&prepared.headers), tracker.log.id);
                    }
                    refreshed.push(acct.id.clone());
                    if crate::oauth::ensure_fresh(&app, &acct, Duration::minutes(5), true).await.is_ok() {
                        // Retry the same account with the new token.
                        retry_same = Some(acct.id.clone());
                        last_error = Some((status, client_body));
                        continue;
                    }
                    // A 401 refused the access token too, and its refresh token was refused
                    // for good: it is out until a sign-in, which a cooldown would only hide.
                    // A 403 can be a permission for this model alone, so the token may still
                    // serve others; it cools as before.
                    if status != 401 || !acct.refuse_access() {
                        acct.cool(None, Utc::now() + Duration::minutes(10), &format!("{status}: {msg}"));
                    }
                }
                401 | 403 => {
                    if status == 401 && provider == Provider::Codex && acct.is_oauth() {
                        log_codex_401(&acct, bearer(&prepared.headers), tracker.log.id);
                    }
                    acct.cool(None, Utc::now() + Duration::minutes(10), &format!("{status}: {msg}"))
                }
                // A gateway timeout may still be generating upstream: answer, don't repeat.
                400 | 404 | 413 | 422 | 504 => {
                    acct.state.lock().set_error(format!("{status}: {msg}"));
                    tracker.finish(status, &Usage::default(), Some(msg));
                    return Reply::Error(status, client_body);
                }
                _ => {
                    acct.state.lock().set_error(format!("{status}: {msg}"));
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
            // A wait the provider names for a failure inside the stream is in these headers.
            let stream_headers = resp.as_ref().map(|r| r.headers().clone()).unwrap_or_default();
            let stream: ByteStream = Box::pin(resp.take().expect("response").bytes_stream());
            let first_event = first_token
                .into_iter()
                .chain(absolute_deadline.map(|d| d.saturating_duration_since(tokio::time::Instant::now())))
                .min();
            let mut first_error = None;
            let boot =
                bootstrap(stream, native, first_event, &cfg.stream_limits, &upstream_model, &mut first_error).await;
            if provider == Provider::Codex
                && let Some(error) = &first_error
            {
                tracing::info!(request_id = tracker.log.id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
                    error_code = %error.code, error_type = %error.kind, "codex stream opened with an error");
            }
            match boot {
                Boot::Ready(stream) => body = Some(stream),
                Boot::Failed { status, message, held } => {
                    tracing::warn!(request_id = tracker.log.id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
                        status, "upstream failed before its first token; trying another account");
                    // What it reported before failing still counts for this attempt.
                    let tap = tracker.wire_tap();
                    let mut dec = SseDecoder::with_limit(cfg.stream_limits.for_model(&upstream_model));
                    let mut events: Vec<_> = held.iter().flat_map(|chunk| dec.push(chunk)).collect();
                    events.extend(dec.finish());
                    for sse in &events {
                        observe_sse(&acct, &model, &tap, sse);
                    }
                    if let Some(c) = &tap.usage {
                        c.headers(&HeaderMap::new(), status);
                    }
                    // As over plain HTTP: the request itself is refused, the account is fine.
                    if protection_refused(provider, status, &message) {
                        let failure = crate::diagnostics::Failure::classified("response_protection_unavailable");
                        tracker.diagnostic_failure(failure.clone());
                        tracing::warn!(request_id = tracker.log.id, account = ?crate::diagnostics::fingerprint(Some(&acct.id)),
                            status, failure_kind = failure.kind, "upstream refused the request as sent; answering without a retry");
                        budget.terminal = true;
                        tracker.finish(status, &Usage::default(), Some(message.clone()));
                        return error_reply(call.format, status, &message);
                    }
                    // The error's code tells a capacity shortage too, whatever its message says.
                    let capacity = at_capacity(status, &message)
                        || first_error.as_ref().is_some_and(|e| at_capacity(status, &e.raw));
                    if status == 429 && quota_exhausted(&acct, &model, status, &message) {
                        mark_quota_exhausted(&acct, &model, &reqwest::header::HeaderMap::new(), &message);
                    } else if status == 429 && !capacity {
                        acct.rate_limit(&model, Utc::now() + Duration::seconds(30), &format!("429: {message}"));
                    } else if matches!(status, 429 | 500 | 502 | 503 | 529) {
                        acct.state
                            .lock()
                            .set_temporary_error(format!("{status}: {message}"), Utc::now() + Duration::seconds(30));
                    } else {
                        acct.state.lock().set_error(format!("{status}: {message}"));
                    }
                    // Like an HTTP 504, a gateway timeout event is ambiguous: the provider
                    // may still be generating, so it is answered rather than repeated.
                    if status == 504 {
                        tracker.finish(status, &Usage::default(), Some(message.clone()));
                        return error_reply(call.format, status, &message);
                    }
                    app.broadcast("accounts", Value::Null);
                    tried.push(acct.id.clone());
                    let body = formats::error_body(call.format, status, &message);
                    // A capacity shortage passes its wait on, as over plain HTTP, should no
                    // other account answer.
                    let error =
                        first_error.as_ref().map_or_else(|| message.clone(), |e| format!("{{\"error\":{}}}", e.raw));
                    let body = match capacity_retry_after(status, &stream_headers, &error) {
                        Some(wait) => with_retry_after(body, wait),
                        None => body,
                    };
                    last_error = Some((status, body));
                    continue;
                }
                Boot::Stalled => {
                    if absolute_deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
                        let msg = "upstream pre-output deadline exceeded; inference may still be running";
                        tracker.finish(504, &Usage::default(), Some(msg.into()));
                        return error_reply(call.format, 504, msg);
                    }
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
        // Limits follow the resolved upstream model, as in `bootstrap`; a client alias
        // need not name the media type.
        let event_limit = cfg.stream_limits.for_model(&upstream_model);
        if passthrough {
            if let Some(body) = body {
                if call.stream {
                    return Reply::Stream {
                        frames: passthrough_stream(
                            body,
                            native,
                            tracker,
                            unwrap,
                            event_limit,
                            !reads_private_events(&call.headers),
                        ),
                        account,
                    };
                }
                // Codex only streams: rebuild the final response object.
                return collect_passthrough(body, native, tracker, call.format, event_limit).await;
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
                    return collect_passthrough(Box::pin(body), native, tracker, call.format, event_limit).await;
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
            sse_events(
                body,
                native,
                names,
                acct.clone(),
                model.clone(),
                tracker.wire_tap(),
                cfg.stream_limits.for_model(&upstream_model),
            )
        } else {
            full_events(
                resp.take().expect("response"),
                native,
                names,
                acct.clone(),
                model.clone(),
                tracker.wire_tap(),
                cfg.stream_limits.for_model(&upstream_model),
            )
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
    let unreachable: Vec<_> = residency_excluded.iter().chain(&budget.skipped).cloned().collect();
    let (status, body) = no_account_left(&app, call.format, &model, only.as_ref(), &unreachable, status, body, false);
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
/// let through as it is. An error that comes before any content or end is noted in
/// `first_error`, whether or not it fails the stream here.
async fn bootstrap(
    mut body: ByteStream,
    native: Format,
    first_event: Option<std::time::Duration>,
    limits: &crate::stream_limits::Limits,
    model: &str,
    first_error: &mut Option<FirstError>,
) -> Boot {
    let start = tokio::time::Instant::now();
    let stall_at = first_event.map(|d| start + d);
    let mut release_at = stall_at.is_none().then(|| start + BOOT_HOLD);
    let mut held: Vec<bytes::Bytes> = Vec::new();
    let mut held_bytes = 0usize;
    let mut dec = SseDecoder::with_limit(limits.for_model(model));
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
        held_bytes = held_bytes.saturating_add(chunk.len());
        held.push(chunk.clone());
        for sse in dec.push(&chunk) {
            release_at.get_or_insert_with(|| tokio::time::Instant::now() + BOOT_HOLD);
            let parsed = serde_json::from_str::<Value>(&sse.data).ok();
            let before = events.len();
            parser.feed_value(&sse, parsed.as_ref(), &mut events);
            let opens = |e: &Event| is_content(e) || matches!(e, Event::Error { .. } | Event::Finish(_));
            if first_error.is_none()
                && !events[..before].iter().any(opens)
                && events[before..].iter().any(|e| matches!(e, Event::Error { .. }))
            {
                *first_error = parsed.as_ref().and_then(FirstError::of);
            }
        }
        if dec.failed() {
            return ready(held, body);
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
        // Once an event has arrived, release the bounded bootstrap buffer to the
        // normal decoder, which still enforces event limits. Before the first
        // event the first-event deadline must keep applying, so buffering
        // continues there, bounded by the decoder's event limit instead.
        if release_at.is_some() && held_bytes >= limits.bootstrap_bytes {
            return ready(held, body);
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
    usage_tap: WireTap,
    event_limit: usize,
) -> EventStream {
    let rename = renamer(names);
    Box::pin(async_stream::stream! {
        let text = match resp.text().await {
            Ok(text) => text,
            Err(e) => {
                if let Some(c) = &usage_tap.usage { c.failure(crate::diagnostics::Failure::http("upstream_body_read", &e)); }
                yield Event::Error { status: 502, message: format!("upstream response read failed: {e}") };
                return;
            }
        };
        let evs = match serde_json::from_str::<Value>(&text) {
            Ok(v) => {
                observe_quota_value(&acct, &model, &v);
                usage_tap.wire(&v);
                checked_full_events(native, &v)
            }
            // Mislabelled stream: decode it as SSE after all.
            Err(_) => {
                let mut dec = SseDecoder::with_limit(event_limit);
                let mut parser = formats::parser(native);
                let mut out = Vec::new();
                for sse in dec.push(text.as_bytes()).into_iter().chain(dec.finish()) {
                    let parsed = observe_sse(&acct, &model, &usage_tap, &sse);
                    parser.feed_value(&sse, parsed.as_ref(), &mut out);
                }
                if dec.failed() { out.push(Event::Error { status:502, message: crate::sse::LIMIT_ERROR.into() }); }
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
    usage_tap: WireTap,
    event_limit: usize,
) -> EventStream {
    let rename = renamer(names);
    Box::pin(async_stream::stream! {
        let mut dec = SseDecoder::with_limit(event_limit);
        let mut parser = formats::parser(native);
        let mut out = Vec::new();
        let mut finished = false;
        loop {
            match body.next().await {
                Some(Ok(chunk)) => {
                    for sse in dec.push(&chunk) {
                        let parsed = observe_sse(&acct, &model, &usage_tap, &sse);
                        parser.feed_value(&sse, parsed.as_ref(), &mut out);
                    }
                }
                Some(Err(e)) => {
                    if finished { return; }
                    if let Some(c) = &usage_tap.usage { c.failure(crate::diagnostics::Failure::http("upstream_body_read", &e)); }
                    out.push(Event::Error { status: 502, message: format!("upstream stream error: {e}") });
                    for ev in out.drain(..) { yield rename(ev); }
                    return;
                }
                None => {
                    for sse in dec.finish() {
                        let parsed = observe_sse(&acct, &model, &usage_tap, &sse);
                        parser.feed_value(&sse, parsed.as_ref(), &mut out);
                    }
                    if dec.failed() {
                        out.push(Event::Error { status:502, message: crate::sse::LIMIT_ERROR.into() });
                    }
                    for ev in out.drain(..) { yield rename(ev); }
                    return;
                }
            }
            if dec.failed() {
                out.push(Event::Error { status:502, message: crate::sse::LIMIT_ERROR.into() });
            }
            for ev in out.drain(..) {
                finished |= matches!(ev, Event::Finish(_));
                yield rename(ev);
            }
            if dec.failed() { return; }
        }
    })
}

/// Quota, usage and the stream parser all read the event's JSON: parse it once, here.
fn observe_sse(acct: &Account, model: &str, usage_tap: &WireTap, sse: &crate::sse::SseEvent) -> Option<Value> {
    let v = serde_json::from_str::<Value>(&sse.data).ok()?;
    observe_quota_value(acct, model, &v);
    usage_tap.wire(&v);
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

/// Private Responses events left out, by client program, for `/metrics`: a client
/// wrongly taken for one that isn't Codex shows up here.
static PRIVATE_DROPPED: parking_lot::Mutex<std::collections::BTreeMap<&'static str, u64>> =
    parking_lot::Mutex::new(std::collections::BTreeMap::new());

pub fn count_private_dropped(client_app: Option<&'static str>) {
    *PRIVATE_DROPPED.lock().entry(client_app.unwrap_or("other")).or_default() += 1;
}

pub fn private_dropped() -> Vec<(&'static str, u64)> {
    PRIVATE_DROPPED.lock().iter().map(|(k, v)| (*k, *v)).collect()
}

/// Codex reads its private events (`response.metadata` drives its routing and safety
/// buffering), so every Codex client keeps them, including app-server hosts such as
/// T3 Code that name themselves only in `originator`. `client_app` doesn't know those
/// hosts, so it must not decide this.
pub(crate) fn reads_private_events(headers: &HeaderMap) -> bool {
    crate::clients::is_codex(headers) || headers.contains_key("originator")
}

/// Codex's own events (`response.metadata`, `codex.*`), Meta's `response.subscription_usage`
/// and the Responses websocket's timing telemetry are not public Responses events, and
/// strict clients reject an event they don't know.
pub(crate) fn private_event(name: &str) -> bool {
    matches!(name, "response.metadata" | "response.subscription_usage")
        || name.starts_with("codex.")
        || name.starts_with("responsesapi.")
}

/// Whether a frame is one of those, by its SSE event name or its `type`. A frame that
/// produced an IR event (usage, an error, the end) is never one: it is always forwarded.
pub(crate) fn private_frame(event: Option<&str>, data: Option<&Value>, events: &[Event]) -> bool {
    events.is_empty()
        && (event.is_some_and(private_event) || data.and_then(|v| v["type"].as_str()).is_some_and(private_event))
}

/// Forwards upstream SSE events untouched while tapping usage. With `drop_private`, a
/// native Responses stream leaves out the private events of `private_event`.
fn passthrough_stream(
    mut body: ByteStream,
    native: Format,
    mut tracker: Tracker,
    unwrap: bool,
    event_limit: usize,
    drop_private: bool,
) -> FrameStream {
    let drop_private = drop_private && native == Format::Responses;
    Box::pin(async_stream::stream! {
        let mut dec = SseDecoder::with_limit(event_limit);
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
            // Capture every event already received before yielding anything to
            // the client. Dropping the body during an early frame must not
            // discard usage later in the same upstream chunk. Keep terminal
            // state tied to forwarded events so an early disconnect stays 499.
            let mut received = Vec::with_capacity(batch.len());
            for sse in batch {
                let done = native == Format::Chat && sse.data.trim() == "[DONE]";
                let parsed = serde_json::from_str::<Value>(&sse.data).ok();
                if let Some(v) = &parsed {
                    tracker.observe_quota_value(v);
                    tracker.observe_wire(v);
                }
                parser.feed_value(&sse, parsed.as_ref(), &mut evs);
                for ev in &evs {
                    if let Event::Usage(usage) = ev {
                        tracker.stream_usage.merge(usage);
                    }
                }
                received.push((sse, parsed, std::mem::take(&mut evs)));
                if done { break; }
            }
            for (sse, parsed, events) in received {
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
                // Fusebox's own observers above have already read it.
                if drop_private && private_frame(sse.event.as_deref(), parsed.as_ref(), &events) {
                    count_private_dropped(tracker.log.client_app);
                    tracing::debug!(request_id = tracker.log.id, client_app = tracker.log.client_app,
                        event = sse.event.as_deref().or(parsed.as_ref().and_then(|v| v["type"].as_str())),
                        "left out a private Responses event this client does not read");
                    continue;
                }
                for ev in events {
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
            if dec.failed() {
                tracker.diagnostic_failure(crate::diagnostics::Failure::classified("upstream_event_limit"));
                tracker.observe_stream_event(&Event::Error { status:502, message: crate::sse::LIMIT_ERROR.into() });
                tracker.finish_stream();
                yield stream_error_frame(native, 502, crate::sse::LIMIT_ERROR);
                return;
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

async fn collect_passthrough(
    mut body: ByteStream,
    native: Format,
    mut tracker: Tracker,
    format: Format,
    event_limit: usize,
) -> Reply {
    let mut dec = SseDecoder::with_limit(event_limit);
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
        if dec.failed() {
            agg.error = Some((502, crate::sse::LIMIT_ERROR.into()));
            break;
        }
    }
    dec.finish().into_iter().for_each(|s| handle(s, &mut agg, &mut final_obj));
    // A trailing multiline event can first cross the limit in the final flush.
    if dec.failed() {
        tracker.diagnostic_failure(crate::diagnostics::Failure::classified("upstream_event_limit"));
        agg.error.get_or_insert_with(|| (502, crate::sse::LIMIT_ERROR.into()));
    }
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
                provider_session: None,
                cache_defaults: true,
            },
            body.clone(),
        );
        let mut rb = app.http.control_for_account(&acct).post(&p.url);
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
        // OpenAI's capacity shortage is retried on the same account only when its wait is
        // brief. A longer one moves the request on at once, and its wait goes to the client.
        let capacity = r#"{"error":{"message":"Codex is at capacity, please try again later"}}"#;
        assert_eq!(soft_failure(Provider::Codex, 429, &h, capacity), Some(std::time::Duration::ZERO));
        assert_eq!(soft_failure(Provider::Codex, 429, &later, capacity), None);
        assert_eq!(soft_failure(Provider::Codex, 429, &soon, capacity).map(|d| d.as_secs() <= 2), Some(true));
        assert!(long_capacity_wait(429, &later, capacity));
        assert!(!long_capacity_wait(429, &soon, capacity));
        assert!(!long_capacity_wait(429, &h, capacity), "no named wait: retried as before");
        assert!(!long_capacity_wait(429, &later, "{}"), "a plain rate limit is not a capacity shortage");
        assert_eq!(capacity_retry_after(429, &later, capacity), Some(CAPACITY_WAIT_LIMIT));
        assert_eq!(capacity_retry_after(429, &soon, capacity), Some(2));
        assert_eq!(capacity_retry_after(429, &h, capacity), None);
        assert_eq!(capacity_retry_after(429, &later, "{}"), None);
        let mut minutes = reqwest::header::HeaderMap::new();
        minutes.insert("retry-after", "256".parse().unwrap());
        // Codex's turn limit counts by its code, even when the message doesn't say "at capacity".
        let turn = r#"{"error":{"code":"turn_rate_limit_exceeded","message":"Too many turns, try again later"}}"#;
        assert!(at_capacity(429, turn));
        assert_eq!(soft_failure(Provider::Codex, 429, &minutes, turn), None);
        assert!(long_capacity_wait(429, &minutes, turn));
        assert_eq!(capacity_retry_after(429, &minutes, turn), Some(256));
        assert!(at_capacity(429, r#"{"error":{"code":"server_is_overloaded"}}"#));
        assert!(!at_capacity(503, capacity), "only 429s need telling apart from rate limits");
        assert!(!at_capacity(429, r#"{"error":{"code":"rate_limit_exceeded"}}"#));
    }

    #[test]
    fn codex_turn_limits_inside_a_stream_are_429s() {
        use crate::formats::StreamParser;
        for event in [
            json!({"type":"response.failed","response":{"error":{"code":"turn_rate_limit_exceeded","message":"Too many turns"}}}),
            json!({"type":"error","code":"turn_rate_limit_exceeded","message":"Too many turns"}),
        ] {
            let mut parser = formats::responses::Parser::default();
            let mut out = Vec::new();
            parser.feed_value(&crate::sse::SseEvent { event: None, data: event.to_string() }, Some(&event), &mut out);
            assert!(matches!(out.last(), Some(Event::Error { status: 429, .. })), "{event}");
        }
    }

    #[test]
    fn stream_errors_are_logged_by_plain_codes_only() {
        let labels = |event: Value| FirstError::of(&event).map(|e| (e.code, e.kind));
        let failed = json!({"type":"response.failed","response":{"error":{"code":"auth_unavailable","type":"invalid_request_error","message":"private detail"}}});
        assert_eq!(labels(failed.clone()), Some(("auth_unavailable".into(), "invalid_request_error".into())));
        assert!(FirstError::of(&failed).unwrap().raw.contains("auth_unavailable"));
        // A bare error event's type names the event, not the error.
        let bare = json!({"type":"error","code":"rate_limit_exceeded","message":"private detail"});
        assert_eq!(labels(bare), Some(("rate_limit_exceeded".into(), "none".into())));
        let odd = json!({"type":"error","error":{"code":"Has Spaces","type":"x".repeat(65)}});
        assert_eq!(labels(odd), Some(("other".into(), "other".into())));
        let numeric = json!({"type":"error","error":{"code":429,"type":"rate_limit_error"}});
        assert_eq!(labels(numeric), Some(("other".into(), "rate_limit_error".into())));
        assert_eq!(labels(json!({"type":"response.created","response":{"id":"r"}})), None);
        assert_eq!(labels(json!({"type":"response.output_text.delta","delta":"error"})), None);
    }

    #[test]
    fn only_a_claude_api_keys_empty_credit_balance_sits_it_out() {
        let directory = std::env::temp_dir().join(format!("fusebox-credit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("signed-in.json"),
            json!({"type":"claude","access_token":"token","email":"owner@example.com"}).to_string(),
        )
        .unwrap();
        let cfg = crate::config::Config {
            auth_dir: directory.to_string_lossy().into(),
            claude_api_key: vec![crate::config::KeyEntry { api_key: "key".into(), ..Default::default() }],
            codex_api_key: vec![crate::config::KeyEntry { api_key: "codex".into(), ..Default::default() }],
            ..Default::default()
        };
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        let find = |provider: Provider, oauth: bool| {
            pool.all().into_iter().find(|a| a.provider == provider && a.is_oauth() == oauth).unwrap()
        };
        let (key, subscription, codex) =
            (find(Provider::Claude, false), find(Provider::Claude, true), find(Provider::Codex, false));
        let empty = "Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing.";
        assert!(credit_exhausted(&key, 400, empty));
        assert!(credit_exhausted(&key, 400, &format!("  {}", empty.to_uppercase())));
        assert!(!credit_exhausted(&key, 429, empty));
        assert!(!credit_exhausted(&key, 400, "prompt is too long: your credit balance is too low"));
        assert!(!credit_exhausted(&subscription, 400, empty), "subscriptions are left alone");
        assert!(!credit_exhausted(&codex, 400, empty));
        // The pause covers the whole account and keeps the refusal, but is no wait to advertise.
        let until = Utc::now() + Duration::minutes(30);
        key.refuse(until, "API credit balance too low: empty", 400, empty);
        assert_eq!(key.cooling_until("claude-sonnet-4-6"), Some(until));
        assert_eq!(key.refusal(), Some((400, empty.to_string())));
        assert!(!key.paused_by_limit("claude-sonnet-4-6"));
        assert_eq!(key.state.lock().last_error(Utc::now()), Some("API credit balance too low: empty"));
        assert!(!quota_exhausted(&key, "claude-sonnet-4-6", 400, empty), "no subscription migration");
        key.state.lock().cooldowns.clear();
        assert_eq!(key.refusal(), None, "the refusal ends with its pause");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn only_limits_are_waits_to_advertise() {
        let cfg = crate::config::Config {
            auth_dir: "/nonexistent".into(),
            codex_api_key: vec![crate::config::KeyEntry { api_key: "test".into(), ..Default::default() }],
            ..Default::default()
        };
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        let account = pool.all().remove(0);
        let model = "gpt-6.1-sol";
        let soon = Utc::now() + Duration::seconds(30);
        let clear = || {
            let mut st = account.state.lock();
            st.cooldowns.clear();
            st.quota_cooldowns.clear();
            st.quota_refreshing = false;
            st.clear_error();
        };
        account.rate_limit(model, soon, "429: slow down");
        assert!(account.paused_by_limit(model));
        clear();
        account.exhaust(model, soon, "subscription quota exhausted: spent");
        assert!(account.paused_by_limit(model));
        clear();
        account.state.lock().quota_refreshing = true;
        assert!(account.paused_by_limit(model));
        clear();
        // A sign-in or permission pause, and a stalled model, are not waits however short.
        account.cool(None, soon, "401: unauthorized");
        assert!(!account.paused_by_limit(model));
        clear();
        account.cool(Some(model), soon, "upstream sent nothing within 120s");
        assert!(!account.paused_by_limit(model));
        // Nor is a rate limit stacked on one.
        account.rate_limit(model, soon, "429: slow down");
        account.cool(None, Utc::now() + Duration::minutes(10), "403: forbidden");
        assert!(!account.paused_by_limit(model));
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
