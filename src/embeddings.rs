//! `/v1/embeddings` in OpenAI's shape. OpenAI and OpenAI-compatible keys answer it
//! natively; Gemini keys through `batchEmbedContents` and Vertex AI through `predict`.
//! Subscriptions don't offer embeddings, so they never take these requests.

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
        let result = match acct.provider {
            Provider::Gemini => gemini(&app, &acct, &headers, &upstream, &body, &mut tracker).await,
            Provider::Vertex => vertex(&app, &acct, &headers, &upstream, &body, &mut tracker).await,
            _ => openai(&app, &acct, &headers, &upstream, &body, &mut tracker).await,
        };
        match result {
            Ok((mut v, usage)) => {
                v["model"] = requested.into();
                tracker.finish(200, &usage, None);
                return Ok(v);
            }
            Err(Failure { error: (status, err), completed: false })
                if status != 504 && (status == 429 || status >= 500) =>
            {
                acct.cool(Some(&model), Utc::now() + chrono::Duration::seconds(30), &format!("{status}: embeddings"));
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
    let (base, auth) = openai_base(app, acct, headers, model);
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

async fn gemini(
    app: &App,
    acct: &Arc<Account>,
    headers: &HeaderMap,
    model: &str,
    body: &Value,
    tracker: &mut Tracker,
) -> Result<(Value, Usage), Failure> {
    let Some(texts) = texts(&body["input"]) else {
        return Err(failure(fail(400, "Gemini embeddings take text input"), false));
    };
    let (url, auth) = google_url(app, acct, headers, model);
    let url =
        url.replace(":generateContent", ":batchEmbedContents").replace(":streamGenerateContent", ":batchEmbedContents");
    let mut vectors = Vec::with_capacity(texts.len());
    let mut tokens = 0;
    for chunk in texts.chunks(100) {
        let requests: Vec<Value> = chunk
            .iter()
            .map(|t| {
                let mut r = json!({ "model": format!("models/{model}"), "content": { "parts": [{ "text": t }] } });
                if let Some(d) = body["dimensions"].as_u64() {
                    r["outputDimensionality"] = d.into();
                }
                r
            })
            .collect();
        let v = request(app, acct, &url, &auth, &json!({ "requests": requests }), model, tracker)
            .await
            .map_err(|e| failure(e, !vectors.is_empty()))?;
        // Gemini doesn't report embedding tokens. Persist the estimate as partial.
        let count = chunk.iter().map(|t| (t.len() as u64).div_ceil(4)).sum::<u64>();
        if let Some(c) = tracker.usage_tap() {
            c.embedding(Some(count), true);
        }
        let embeddings: Vec<Value> =
            v["embeddings"].as_array().into_iter().flatten().map(|e| e["values"].clone()).collect();
        if embeddings.len() != chunk.len() {
            return Err(failure(fail(502, "Gemini returned a different number of embeddings"), true));
        }
        vectors.extend(embeddings);
        tokens += count;
    }
    response(vectors, tokens, body["encoding_format"].as_str().unwrap_or("float")).map_err(|e| failure(e, true))
}

async fn vertex(
    app: &App,
    acct: &Arc<Account>,
    headers: &HeaderMap,
    model: &str,
    body: &Value,
    tracker: &mut Tracker,
) -> Result<(Value, Usage), Failure> {
    let Some(texts) = texts(&body["input"]) else {
        return Err(failure(fail(400, "Vertex embeddings take text input"), false));
    };
    let (url, auth) = google_url(app, acct, headers, model);
    let url = url.replace(":generateContent", ":predict").replace(":streamGenerateContent", ":predict");
    let mut vectors = Vec::with_capacity(texts.len());
    let mut tokens = 0;
    let limit = if model.starts_with("gemini-embedding") { 1 } else { 5 };
    for chunk in texts.chunks(limit) {
        let mut req = json!({ "instances": chunk.iter().map(|t| json!({ "content": t })).collect::<Vec<_>>(), "parameters": {"autoTruncate": false} });
        if let Some(d) = body["dimensions"].as_u64() {
            req["parameters"]["outputDimensionality"] = d.into();
        }
        let v =
            request(app, acct, &url, &auth, &req, model, tracker).await.map_err(|e| failure(e, !vectors.is_empty()))?;
        let predictions = v["predictions"].as_array().cloned().unwrap_or_default();
        let counts: Option<Vec<u64>> = predictions
            .iter()
            .map(|p| {
                p["embeddings"]["statistics"]["token_count"]
                    .as_f64()
                    .filter(|n| {
                        n.is_finite() && *n >= 0.0 && n.fract() == 0.0 && *n <= crate::usage::types::MAX_TOKENS as f64
                    })
                    .map(|n| n as u64)
            })
            .collect();
        let count = counts.map(|c| c.into_iter().sum::<u64>());
        if let Some(c) = tracker.usage_tap() {
            c.embedding(count, false);
        }
        if predictions.len() != chunk.len() {
            return Err(failure(fail(502, "Vertex returned a different number of embeddings"), true));
        }
        if predictions.iter().any(|p| p["embeddings"]["statistics"]["truncated"] == true) {
            return Err(failure(fail(400, "Vertex truncated embedding input despite autoTruncate=false"), true));
        }
        tokens += count.unwrap_or(0);
        vectors.extend(predictions.into_iter().map(|p| p["embeddings"]["values"].clone()));
    }
    response(vectors, tokens, body["encoding_format"].as_str().unwrap_or("float")).map_err(|e| failure(e, true))
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
