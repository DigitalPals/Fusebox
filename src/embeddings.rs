//! `/v1/embeddings` in OpenAI's shape. OpenAI and OpenAI-compatible keys answer it
//! natively; Gemini keys through `batchEmbedContents` and Vertex AI through `predict`.
//! Subscriptions don't offer embeddings, so they never take these requests.

use futures::{StreamExt, stream::FuturesUnordered};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use axum::http::HeaderMap;
use base64::Engine;
use chrono::Utc;
use serde_json::{Value, json};

use crate::accounts::{Account, Credential, Only, Provider};
use crate::ir::{Format, Usage};
use crate::media::{Outcome, fail, openai_base, read_json, send};
use crate::proxy::Tracker;
use crate::state::App;
use crate::upstream::{self, Target};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    pub concurrency: usize,
    pub cache_seconds: u64,
    pub cache_bytes: usize,
}
impl Default for Config {
    fn default() -> Self {
        Self { concurrency: 1, cache_seconds: 0, cache_bytes: 16 << 20 }
    }
}
impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=16).contains(&self.concurrency) || self.cache_seconds > 86400 || self.cache_bytes > 256 << 20 {
            return Err(
                "embeddings: concurrency must be 1–16, cache-seconds 0–86400, cache-bytes at most 256 MiB".into()
            );
        }
        Ok(())
    }
}
struct Cached {
    value: Arc<Value>,
    bytes: usize,
    created: std::time::Instant,
    expires: std::time::Instant,
    used: std::time::Instant,
}
#[derive(Default)]
pub struct Cache {
    entries: std::collections::HashMap<String, Cached>,
    bytes: usize,
    hits: u64,
    misses: u64,
    next_expiry: Option<std::time::Instant>,
    limits: Option<(u64, usize)>,
}
impl Cache {
    fn key(headers: &HeaderMap, acct: &Account, model: &str, body: &Value) -> String {
        // Read each lock on its own: a guard inside the loop's array would be held
        // for the whole loop, including while the credential lock is taken.
        let identity = acct.state.lock().credential_identity.clone();
        let backend = upstream::backend_stamp(acct);
        let mut hash = Sha256::new();
        for part in
            [crate::affinity::client_scope(headers), acct.id.clone(), identity, backend, model.into(), body.to_string()]
        {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part.as_bytes());
        }
        hex::encode(hash.finalize())
    }
    fn get(&mut self, key: &str, cfg: &Config) -> Option<Arc<Value>> {
        let now = std::time::Instant::now();
        self.trim(cfg, now);
        let Some(cached) = self.entries.get_mut(key) else {
            self.misses = self.misses.saturating_add(1);
            return None;
        };
        self.hits = self.hits.saturating_add(1);
        cached.used = now;
        Some(cached.value.clone())
    }
    fn trim(&mut self, cfg: &Config, now: std::time::Instant) {
        let limits = (cfg.cache_seconds, cfg.cache_bytes);
        if cfg.cache_seconds == 0 {
            self.clear();
        } else if self.limits != Some(limits) || self.next_expiry.is_some_and(|expires| expires <= now) {
            // Hits only need a sweep once the earliest deadline arrives or the
            // operator changes a limit. Keep each original deadline when the
            // TTL grows; a shorter TTL still applies to already cached results.
            self.next_expiry = None;
            self.entries.retain(|_, e| {
                let expires = e.expires.min(e.created + std::time::Duration::from_secs(cfg.cache_seconds));
                if expires <= now {
                    debug_assert!(self.bytes >= e.bytes);
                    self.bytes -= e.bytes;
                    false
                } else {
                    self.next_expiry = Some(self.next_expiry.map_or(expires, |next| next.min(expires)));
                    true
                }
            });
        }
        self.limits = Some(limits);
        while self.bytes > cfg.cache_bytes || self.entries.len() > 1000 {
            let Some(old) = self.entries.iter().min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone()) else {
                break;
            };
            let removed = self.entries.remove(&old).unwrap();
            debug_assert!(self.bytes >= removed.bytes);
            self.bytes -= removed.bytes;
        }
        if self.entries.is_empty() {
            self.bytes = 0;
            self.next_expiry = None;
        }
    }
    fn insert(&mut self, key: String, value: Arc<Value>, bytes: usize, cfg: &Config) {
        if cfg.cache_seconds == 0 || bytes > cfg.cache_bytes {
            return;
        }
        let now = std::time::Instant::now();
        let expires = now + std::time::Duration::from_secs(cfg.cache_seconds);
        if let Some(old) = self.entries.insert(key, Cached { value, bytes, created: now, expires, used: now }) {
            debug_assert!(self.bytes >= old.bytes);
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        self.next_expiry = Some(self.next_expiry.map_or(expires, |next| next.min(expires)));
        self.trim(cfg, now);
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
        self.next_expiry = None;
    }
    pub fn stats(&mut self, cfg: &Config) -> (u64, u64, usize, usize) {
        self.trim(cfg, std::time::Instant::now());
        (self.hits, self.misses, self.entries.len(), self.bytes)
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn shorter_ttl_and_capacity_apply_to_existing_results() {
        let mut cache = Cache::default();
        let mut cfg = Config { cache_seconds: 60, cache_bytes: 1024, ..Default::default() };
        cache.insert("old".into(), Arc::new(json!({"data":[]})), 11, &cfg);
        cache.entries.get_mut("old").unwrap().created -= std::time::Duration::from_secs(10);
        cfg.cache_seconds = 5;
        assert!(cache.get("old", &cfg).is_none());
        cache.insert("new".into(), Arc::new(json!({"data":[]})), 11, &cfg);
        assert!(cache.get("new", &cfg).is_some());
        cfg.cache_bytes = 0;
        assert!(cache.get("new", &cfg).is_none());
        assert_eq!(cache.stats(&cfg), (1, 2, 0, 0));
    }

    #[test]
    fn expiry_boundary_and_longer_ttl_preserve_original_deadline() {
        let mut cache = Cache::default();
        let mut cfg = Config { cache_seconds: 2, cache_bytes: 1024, ..Default::default() };
        cache.insert("short".into(), Arc::new(json!([])), 2, &cfg);
        let deadline = cache.entries["short"].expires;
        cfg.cache_seconds = 60;
        cache.insert("long".into(), Arc::new(json!([1])), 3, &cfg);
        assert_eq!(cache.next_expiry, Some(deadline));
        cache.trim(&cfg, deadline - std::time::Duration::from_nanos(1));
        assert_eq!(cache.bytes, 5);
        cache.trim(&cfg, deadline);
        assert!(!cache.entries.contains_key("short"));
        assert_eq!(cache.bytes, 3);
        assert_eq!(cache.next_expiry, Some(cache.entries["long"].expires));
    }

    #[test]
    fn replacement_eviction_disable_and_clear_keep_byte_accounting_exact() {
        let mut cache = Cache::default();
        let mut cfg = Config { cache_seconds: 60, cache_bytes: 10, ..Default::default() };
        cache.insert("a".into(), Arc::new(json!([1])), 3, &cfg);
        cache.insert("b".into(), Arc::new(json!([2])), 3, &cfg);
        cache.entries.get_mut("a").unwrap().used -= std::time::Duration::from_secs(1);
        cache.insert("b".into(), Arc::new(json!([2, 3, 4])), 7, &cfg);
        assert_eq!(cache.bytes, 10);
        cache.insert("c".into(), Arc::new(json!([5])), 3, &cfg);
        assert!(!cache.entries.contains_key("a"));
        assert_eq!(cache.bytes, 10);
        cfg.cache_seconds = 0;
        assert_eq!(cache.stats(&cfg), (0, 0, 0, 0));
        assert!(cache.next_expiry.is_none());
        cfg.cache_seconds = 60;
        cache.insert("fresh".into(), Arc::new(json!([])), 2, &cfg);
        assert!(cache.get("fresh", &cfg).is_some());
        cache.clear();
        assert_eq!(cache.stats(&cfg), (1, 0, 0, 0));
        assert!(cache.next_expiry.is_none());
    }

    #[test]
    fn deferred_maintenance_matches_eager_reference_across_limits_and_deadlines() {
        fn eager_trim(cache: &mut Cache, cfg: &Config, now: std::time::Instant) {
            cache.entries.retain(|_, e| {
                cfg.cache_seconds > 0 && e.expires > now && now.duration_since(e.created).as_secs() < cfg.cache_seconds
            });
            cache.bytes = cache.entries.values().map(|e| e.bytes).sum();
            while cache.bytes > cfg.cache_bytes || cache.entries.len() > 1000 {
                let oldest = cache.entries.iter().min_by_key(|(_, e)| e.used).unwrap().0.clone();
                cache.bytes -= cache.entries.remove(&oldest).unwrap().bytes;
            }
        }
        let start = std::time::Instant::now();
        let fixture = || {
            let mut cache = Cache::default();
            for index in 0..1003 {
                let created = start - std::time::Duration::from_millis(index as u64);
                let expires = start + std::time::Duration::from_millis(50 + index as u64 * 3);
                let bytes = index % 11;
                cache.entries.insert(
                    index.to_string(),
                    Cached { value: Arc::new(Value::Null), bytes, created, expires, used: created },
                );
                cache.bytes += bytes;
                cache.next_expiry = Some(cache.next_expiry.map_or(expires, |next| next.min(expires)));
            }
            cache
        };
        for scenario in 0..8 {
            let (mut actual, mut expected) = (fixture(), fixture());
            for step in 0..40 {
                let cfg = Config {
                    cache_seconds: [60, 1, 60, 2, 0][(scenario + step / 9) % 5],
                    cache_bytes: [20_000, 2_000, 0, 20_000][(scenario + step / 7) % 4],
                    ..Default::default()
                };
                let now = start + std::time::Duration::from_millis(step as u64 * 100);
                actual.trim(&cfg, now);
                eager_trim(&mut expected, &cfg, now);
                assert_eq!(actual.bytes, expected.bytes, "scenario={scenario} step={step}");
                assert_eq!(actual.entries.len(), expected.entries.len(), "scenario={scenario} step={step}");
                assert!(actual.entries.keys().all(|key| expected.entries.contains_key(key)));
            }
        }
    }
}

/// Google's embedding models (`gemini-embedding-001`, `text-embedding-005`, ...).
fn google_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.starts_with("gemini-embedding") || m.starts_with("text-embedding-0") || m.contains("multilingual-embedding")
}

/// Whether an account can embed with `model`.
fn serves(a: &Account, model: &str) -> bool {
    let api_key = matches!(*a.cred.read(), Credential::ApiKey { .. });
    match a.provider {
        Provider::Codex => api_key && (model.starts_with("text-embedding-") || a.resolve(model).is_some()),
        // A compatible provider that lists its models must list this one.
        Provider::Compat => a.models.is_empty() || a.resolve(model).is_some(),
        Provider::Gemini | Provider::Vertex => google_model(model),
        _ => false,
    }
}

/// The texts to embed. OpenAI also takes token ids, which only OpenAI-shaped upstreams read.
fn texts(input: &Value) -> Option<Vec<String>> {
    match input {
        Value::String(s) => Some(vec![s.clone()]),
        Value::Array(items) if items.iter().all(Value::is_string) => {
            Some(items.iter().filter_map(Value::as_str).map(String::from).collect())
        }
        _ => None,
    }
}

fn validate(body: &Value) -> Result<(), (u16, Value)> {
    let tokens = |v: &Value| v.as_array().is_some_and(|a| !a.is_empty() && a.iter().all(|n| n.as_u64().is_some()));
    let text = |v: &Value| v.as_str().is_some_and(|s| !s.is_empty());
    let input = &body["input"];
    if !(text(input)
        || tokens(input)
        || input.as_array().is_some_and(|a| !a.is_empty() && (a.iter().all(text) || a.iter().all(tokens))))
    {
        return Err(fail(400, "`input` must contain nonempty text or token arrays"));
    }
    if body.get("encoding_format").is_some_and(|v| !matches!(v.as_str(), Some("float" | "base64"))) {
        return Err(fail(400, "`encoding_format` must be `float` or `base64`"));
    }
    if body.get("dimensions").is_some_and(|v| !v.as_u64().is_some_and(|n| (1..=u32::MAX as u64).contains(&n))) {
        return Err(fail(400, "`dimensions` must be a positive integer"));
    }
    Ok(())
}

struct Failure {
    error: (u16, Value),
    /// Repeating a batch that already did work would duplicate its earlier chunks.
    completed: bool,
}

fn failure(error: (u16, Value), completed: bool) -> Failure {
    Failure { error, completed }
}

pub async fn embeddings(app: Arc<App>, headers: HeaderMap, body: Value) -> Outcome {
    if crate::upstream::residency_requirement(&headers).map_err(|message| fail(400, message))?.is_some() {
        return Err(fail(400, "Codex subscription residency is unavailable on the embeddings endpoint"));
    }
    let Some(requested) = body["model"].as_str().map(str::trim).filter(|m| !m.is_empty()) else {
        return Err(fail(400, "`model` is required"));
    };
    if body["input"].is_null() {
        return Err(fail(400, "`input` is required"));
    }
    validate(&body)?;
    if let Err(d) = crate::policy::check_model(&app, &headers, requested) {
        return Err(fail(d.status, d.message));
    }
    let (only, model) = app.pool.route(requested);
    let mut tracker = Tracker::new(&app, Format::Chat, false, "http", &model);
    tracker.client_app(&headers);
    let mut candidates: Vec<Arc<Account>> = app
        .pool
        .all()
        .into_iter()
        .filter(|a| match &only {
            Some(Only::Provider(p)) => *p == a.provider,
            Some(Only::Prefix(x)) => a.prefix.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(x)),
            None => true,
        })
        .filter(|a| serves(a, &model) && !a.state.lock().disabled && a.cooling_until(&model).is_none())
        .collect();
    // Higher priorities first; a stable order otherwise.
    candidates.sort_by_key(|a| std::cmp::Reverse(a.priority));
    if candidates.is_empty() {
        let message = format!("no account can embed with `{model}` (add an OpenAI, compatible, Gemini or Vertex key)");
        tracker.finish(404, &Usage::default(), Some(message.clone()));
        return Err(fail(404, message));
    }
    let attempts = (app.cfg().request_retry as usize).clamp(1, 10);
    let mut last = None;
    for acct in candidates.into_iter().take(attempts) {
        let upstream = acct.resolve(&model).unwrap_or_else(|| model.clone());
        let cache_cfg = app.cfg().embeddings.clone();
        let cache_key = (cache_cfg.cache_seconds > 0).then(|| Cache::key(&headers, &acct, &upstream, &body));
        let cached = cache_key.as_deref().and_then(|key| app.embedding_cache.lock().get(key, &cache_cfg));
        if let Some(value) = cached {
            // Copy the vectors after releasing the shared cache lock.
            let mut value = (*value).clone();
            value["usage"] = json!({"prompt_tokens":0,"total_tokens":0});
            value["fusebox_cache_hit"] = true.into();
            tracker.cached(&acct);
            tracker.finish(200, &Usage::default(), None);
            return Ok(value);
        }
        let result = match acct.provider {
            Provider::Gemini => google(&app, &acct, &headers, &upstream, &body, &mut tracker, false).await,
            Provider::Vertex => google(&app, &acct, &headers, &upstream, &body, &mut tracker, true).await,
            _ => openai(&app, &acct, &headers, &upstream, &body, &mut tracker).await,
        };
        match result {
            Ok((mut v, usage)) => {
                v["model"] = requested.into();
                if let Some(key) = cache_key {
                    let bytes = v.to_string().len();
                    if bytes <= cache_cfg.cache_bytes {
                        let value = Arc::new(v.clone());
                        let mut cache = app.embedding_cache.lock();
                        // A completed request must not repopulate a cache disabled
                        // or resized while that request was running.
                        if app.cfg().embeddings == cache_cfg {
                            cache.insert(key, value, bytes, &cache_cfg);
                        }
                    }
                }
                tracker.finish(200, &usage, None);
                return Ok(v);
            }
            Err(Failure { error: (status, err), completed: false })
                if status != 504 && (status == 429 || status >= 500) =>
            {
                let (until, reason) = (Utc::now() + chrono::Duration::seconds(30), format!("{status}: embeddings"));
                if status == 429 {
                    acct.rate_limit(&model, until, &reason);
                } else {
                    acct.cool(Some(&model), until, &reason);
                }
                last = Some((status, err));
            }
            Err(Failure { error: (status, err), .. }) => {
                tracker.finish(status, &Usage::default(), Some(crate::proxy::error_message(&err.to_string())));
                return Err((status, err));
            }
        }
    }
    let (status, err) = last.unwrap_or_else(|| fail(502, "no account could embed"));
    tracker.finish(status, &Usage::default(), Some(crate::proxy::error_message(&err.to_string())));
    Err((status, err))
}

async fn openai(
    app: &App,
    acct: &Arc<Account>,
    headers: &HeaderMap,
    model: &str,
    body: &Value,
    tracker: &mut Tracker,
) -> Result<(Value, Usage), Failure> {
    let (base, auth) = openai_base(app, acct, headers, model, body);
    let mut req = body.clone();
    req["model"] = model.into();
    let v = request(app, acct, &format!("{base}/embeddings"), &auth, &req, model, tracker)
        .await
        .map_err(|e| failure(e, false))?;
    let tokens = v["usage"]["prompt_tokens"].as_u64();
    if let Some(c) = tracker.usage_tap() {
        c.wire(&v);
        c.embedding(tokens, false);
    }
    let usage = Usage { input: tokens.unwrap_or(0), ..Default::default() };
    Ok((v, usage))
}

/// The provider's URL for a Google model, as chat requests would use it.
fn google_url(app: &App, acct: &Account, headers: &HeaderMap, model: &str) -> (String, Vec<(String, String)>) {
    let cfg = app.cfg();
    let t = Target {
        acct,
        cfg: &cfg,
        client_headers: headers,
        model,
        wire: Format::Gemini,
        passthrough: true,
        stream: false,
        count_tokens: false,
        cache_key: None,
        provider_session: None,
        cache_defaults: true,
    };
    let p = upstream::prepare(&t, json!({}));
    (p.url, p.headers)
}

fn response(vectors: Vec<Value>, tokens: u64, encoding: &str) -> Result<(Value, Usage), (u16, Value)> {
    let mut data = Vec::with_capacity(vectors.len());
    for (i, e) in vectors.into_iter().enumerate() {
        let values = e
            .as_array()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| fail(502, "upstream returned an invalid embedding"))?;
        let mut bytes = Vec::with_capacity(values.len() * 4);
        for value in values {
            let n = value.as_f64().ok_or_else(|| fail(502, "upstream returned a nonnumeric embedding"))? as f32;
            if !n.is_finite() {
                return Err(fail(502, "upstream returned an out-of-range embedding"));
            }
            bytes.extend_from_slice(&n.to_le_bytes());
        }
        let embedding = if encoding == "base64" {
            Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
        } else {
            e
        };
        data.push(json!({ "object": "embedding", "index": i, "embedding": embedding }));
    }
    let v = json!({ "object": "list", "data": data, "usage": { "prompt_tokens": tokens, "total_tokens": tokens } });
    Ok((v, Usage { input: tokens, ..Default::default() }))
}

async fn request(
    app: &App,
    acct: &Arc<Account>,
    url: &str,
    auth: &[(String, String)],
    body: &Value,
    model: &str,
    tracker: &mut Tracker,
) -> Result<Value, (u16, Value)> {
    tracker.attempt(acct);
    if let Some(c) = tracker.usage_tap() {
        c.model(model);
    }
    let resp = match send(app, acct, url, auth, body).await {
        Ok(resp) => resp,
        Err(error) => {
            if let Some(c) = tracker.usage_tap() {
                c.headers(&HeaderMap::new(), error.0);
            }
            return Err(error);
        }
    };
    if let Some(c) = tracker.usage_tap() {
        c.headers(resp.headers(), resp.status().as_u16());
    }
    read_json(resp).await
}

struct ChunkUsage {
    analytics: crate::usage::capture::RequestUsage,
    app: Arc<App>,
    _load: crate::accounts::RequestLoad,
    finished: bool,
}
impl ChunkUsage {
    fn finish(&mut self, status: u16) {
        if let Some(o) = self.analytics.finish(Some(status), None, false) {
            self.app.stats.performance.lock().attempt(&o, "http");
        }
        self.finished = true;
    }
}
impl Drop for ChunkUsage {
    fn drop(&mut self) {
        if !self.finished {
            self.finish(499);
        }
    }
}

async fn google(
    app: &Arc<App>,
    acct: &Arc<Account>,
    headers: &HeaderMap,
    model: &str,
    body: &Value,
    tracker: &mut Tracker,
    vertex: bool,
) -> Result<(Value, Usage), Failure> {
    let texts = texts(&body["input"]).ok_or_else(|| failure(fail(400, "Google embeddings take text input"), false))?;
    tracker.embedding_attempt(acct);
    let (url, auth) = google_url(app, acct, headers, model);
    let method = if vertex { ":predict" } else { ":batchEmbedContents" };
    let url = url.replace(":generateContent", method).replace(":streamGenerateContent", method);
    let limit = if !vertex {
        100
    } else if model.starts_with("gemini-embedding") {
        1
    } else {
        5
    };
    let mut jobs = texts.chunks(limit).enumerate().map(|(index, chunk)| {
        let mut req = if vertex {
            json!({"instances":chunk.iter().map(|t|json!({"content":t})).collect::<Vec<_>>(),"parameters":{"autoTruncate":false}})
        } else {
            json!({"requests":chunk.iter().map(|t|json!({"model":format!("models/{model}"),"content":{"parts":[{"text":t}]}})).collect::<Vec<_>>()})
        };
        if let Some(d) = body["dimensions"].as_u64() {
            if vertex { req["parameters"]["outputDimensionality"] = d.into(); }
            else { for r in req["requests"].as_array_mut().unwrap() { r["outputDimensionality"] = d.into(); } }
        }
        (index, req, chunk.len(), chunk.iter().map(|t|(t.len() as u64).div_ceil(4)).sum::<u64>())
    });
    let mut pending = FuturesUnordered::new();
    let concurrency = app.cfg().embeddings.concurrency.clamp(1, 16);
    let mut launched = 0usize;
    let mut results = Vec::new();
    let mut first_error = None;
    // A chunk that failed validation after a successful response was still executed.
    let mut executed_failure = false;
    let mut exhausted = false;
    let mut final_chunk = None;
    loop {
        while first_error.is_none() && !exhausted && pending.len() < concurrency {
            let Some((index, req, count, estimate)) = jobs.next() else {
                exhausted = true;
                break;
            };
            launched += 1;
            let child = ChunkUsage {
                analytics: tracker.embedding_child(acct, index, model),
                app: app.clone(),
                _load: crate::accounts::RequestLoad::new(acct),
                finished: false,
            };
            pending.push(google_chunk(app, acct, &url, &auth, req, index, count, estimate, vertex, child));
        }
        let Some((index, result, mut child)) = pending.next().await else {
            break;
        };
        let chunk_status = result.as_ref().err().map_or(200, |(e, _)| e.0);
        match result {
            Ok((vectors, tokens)) => {
                tracker.embedding_progress(tokens, !vertex);
                results.push((index, vectors, tokens));
            }
            Err((error, executed)) => {
                tracker.embedding_progress(None, false);
                executed_failure |= executed;
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        if pending.is_empty() && (launched == texts.len().div_ceil(limit) || first_error.is_some()) {
            // Final response validation can still fail after successful chunks.
            final_chunk = Some((child, chunk_status));
        } else {
            child.finish(chunk_status);
        }
        // Drain in-flight work after failure so its usage survives, but schedule no new chunks.
    }
    let outcome = (|| {
        if let Some(error) = first_error {
            return Err(failure(error, launched > 1 || !results.is_empty() || executed_failure));
        }
        results.sort_by_key(|(index, _, _)| *index);
        let mut vectors = Vec::with_capacity(texts.len());
        let mut tokens = Some(0u64);
        for (_, values, count) in results {
            vectors.extend(values);
            tokens = tokens.zip(count).map(|(a, b)| a + b);
        }
        let (mut output, usage) =
            response(vectors, tokens.unwrap_or(0), body["encoding_format"].as_str().unwrap_or("float"))
                .map_err(|e| failure(e, true))?;
        if tokens.is_none() {
            output.as_object_mut().unwrap().remove("usage");
        }
        Ok((output, usage))
    })();
    if let Some((mut child, status)) = final_chunk {
        if let Some(c) = child.analytics.tap() {
            c.logical_result(outcome.is_ok());
        }
        child.finish(status);
    }
    outcome
}

#[allow(clippy::too_many_arguments)]
async fn google_chunk(
    app: &App,
    acct: &Arc<Account>,
    url: &str,
    auth: &[(String, String)],
    req: Value,
    index: usize,
    expected: usize,
    estimate: u64,
    vertex: bool,
    child: ChunkUsage,
) -> (usize, Result<(Vec<Value>, Option<u64>), ((u16, Value), bool)>, ChunkUsage) {
    let result = async {
        let resp = send(app, acct, url, auth, &req).await.map_err(|e| (e, false))?;
        if let Some(c) = child.analytics.tap() {
            c.headers(resp.headers(), resp.status().as_u16());
        }
        let v = read_json(resp).await.map_err(|e| (e, false))?;
        // The provider has done this chunk's work from here on; retrying elsewhere would repeat it.
        let executed = |e: (u16, Value)| (e, true);
        let (vectors, tokens) = if vertex {
            let predictions = v["predictions"]
                .as_array()
                .ok_or_else(|| executed(fail(502, "Vertex returned invalid predictions")))?;
            let counts: Option<Vec<u64>> = predictions
                .iter()
                .map(|p| {
                    p["embeddings"]["statistics"]["token_count"]
                        .as_f64()
                        .filter(|n| {
                            n.is_finite() && *n >= 0. && n.fract() == 0. && *n <= crate::usage::types::MAX_TOKENS as f64
                        })
                        .map(|n| n as u64)
                })
                .collect();
            let tokens = counts.map(|c| c.into_iter().sum());
            if let Some(c) = child.analytics.tap() {
                c.embedding(tokens, false);
            }
            if predictions.iter().any(|p| p["embeddings"]["statistics"]["truncated"] == true) {
                return Err(executed(fail(400, "Vertex truncated embedding input despite autoTruncate=false")));
            }
            (predictions.iter().map(|p| p["embeddings"]["values"].clone()).collect::<Vec<_>>(), tokens)
        } else {
            if let Some(c) = child.analytics.tap() {
                c.embedding(Some(estimate), true);
            }
            (v["embeddings"].as_array().into_iter().flatten().map(|p| p["values"].clone()).collect(), Some(estimate))
        };
        if vectors.len() != expected {
            return Err(executed(fail(502, "Google returned a different number of embeddings")));
        }
        Ok((vectors, tokens))
    }
    .await;
    (index, result, child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_and_models() {
        assert_eq!(texts(&json!("hi")), Some(vec!["hi".to_string()]));
        assert_eq!(texts(&json!(["a", "b"])), Some(vec!["a".to_string(), "b".to_string()]));
        assert_eq!(texts(&json!([[1, 2]])), None);
        assert!(google_model("gemini-embedding-001"));
        assert!(google_model("text-embedding-005"));
        assert!(!google_model("text-embedding-3-small"));
        let (v, usage) = response(vec![json!([0.1]), json!([0.2])], 7, "float").unwrap();
        assert_eq!(v["data"][1]["index"], 1);
        assert_eq!(v["usage"]["total_tokens"], 7);
        assert_eq!(usage.input, 7);
    }

    #[test]
    fn base64_contains_little_endian_float32_values() {
        let (v, _) = response(vec![json!([0.5, -0.25, 1.0])], 3, "base64").unwrap();
        let bytes =
            base64::engine::general_purpose::STANDARD.decode(v["data"][0]["embedding"].as_str().unwrap()).unwrap();
        assert_eq!(bytes, [0.5_f32, -0.25, 1.0].into_iter().flat_map(f32::to_le_bytes).collect::<Vec<_>>());
        assert!(response(vec![json!(["bad"])], 0, "float").is_err());
        assert!(response(vec![json!([1e300])], 0, "base64").is_err());
    }

    #[test]
    fn invalid_options_and_empty_inputs_are_rejected() {
        for input in [json!(""), json!([]), json!([""]), json!([[]]), json!([-1]), json!(["x", 1])] {
            assert_eq!(validate(&json!({"input":input})).unwrap_err().0, 400);
        }
        for options in [
            json!({"encoding_format":"hex"}),
            json!({"encoding_format":null}),
            json!({"dimensions":0}),
            json!({"dimensions":1.5}),
            json!({"dimensions":"3"}),
        ] {
            let mut body = options;
            body["input"] = "hi".into();
            assert_eq!(validate(&body).unwrap_err().0, 400);
        }
        for input in [json!("hi"), json!(["a", "b"]), json!([1, 2]), json!([[1], [2, 3]])] {
            assert!(validate(&json!({"input":input,"dimensions":3,"encoding_format":"base64"})).is_ok());
        }
    }
}
