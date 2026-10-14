//! `/v1/embeddings` in OpenAI's shape. OpenAI and OpenAI-compatible keys answer it
//! natively; Gemini keys through `batchEmbedContents` and Vertex AI through `predict`.
//! Subscriptions don't offer embeddings, so they never take these requests.

use std::sync::Arc;

use axum::http::HeaderMap;
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

pub async fn embeddings(app: Arc<App>, headers: HeaderMap, body: Value) -> Outcome {
    let Some(requested) = body["model"].as_str().map(str::trim).filter(|m| !m.is_empty()) else {
        return Err(fail(400, "`model` is required"));
    };
    if body["input"].is_null() {
        return Err(fail(400, "`input` is required"));
    }
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
        tracker.attempt(&acct);
        let upstream = acct.resolve(&model).unwrap_or_else(|| model.clone());
        let result = match acct.provider {
            Provider::Gemini => gemini(&app, &acct, &headers, &upstream, &body).await,
            Provider::Vertex => vertex(&app, &acct, &headers, &upstream, &body).await,
            _ => openai(&app, &acct, &headers, &upstream, &body).await,
        };
        match result {
            Ok((mut v, usage)) => {
                v["model"] = requested.into();
                tracker.finish(200, &usage, None);
                return Ok(v);
            }
            Err((status, err)) if status == 429 || status >= 500 => {
                acct.cool(Some(&model), Utc::now() + chrono::Duration::seconds(30), &format!("{status}: embeddings"));
                last = Some((status, err));
            }
            Err((status, err)) => {
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
    acct: &Account,
    headers: &HeaderMap,
    model: &str,
    body: &Value,
) -> Result<(Value, Usage), (u16, Value)> {
    let (base, auth) = openai_base(app, acct, headers, model);
    let mut req = body.clone();
    req["model"] = model.into();
    let v = read_json(send(app, acct, &format!("{base}/embeddings"), &auth, &req).await?).await?;
    let usage = Usage { input: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0), ..Default::default() };
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

fn response(vectors: Vec<Value>, tokens: u64) -> (Value, Usage) {
    let data: Vec<Value> = vectors
        .into_iter()
        .enumerate()
        .map(|(i, e)| json!({ "object": "embedding", "index": i, "embedding": e }))
        .collect();
    let v = json!({ "object": "list", "data": data, "usage": { "prompt_tokens": tokens, "total_tokens": tokens } });
    (v, Usage { input: tokens, ..Default::default() })
}

async fn gemini(
    app: &App,
    acct: &Account,
    headers: &HeaderMap,
    model: &str,
    body: &Value,
) -> Result<(Value, Usage), (u16, Value)> {
    let Some(texts) = texts(&body["input"]) else { return Err(fail(400, "Gemini embeddings take text input")) };
    let (url, auth) = google_url(app, acct, headers, model);
    let url =
        url.replace(":generateContent", ":batchEmbedContents").replace(":streamGenerateContent", ":batchEmbedContents");
    let requests: Vec<Value> = texts
        .iter()
        .map(|t| {
            let mut r = json!({ "model": format!("models/{model}"), "content": { "parts": [{ "text": t }] } });
            if let Some(d) = body["dimensions"].as_u64() {
                r["outputDimensionality"] = d.into();
            }
            r
        })
        .collect();
    let v = read_json(send(app, acct, &url, &auth, &json!({ "requests": requests })).await?).await?;
    let vectors: Vec<Value> = v["embeddings"].as_array().into_iter().flatten().map(|e| e["values"].clone()).collect();
    if vectors.len() != texts.len() {
        return Err(fail(502, "Gemini returned a different number of embeddings"));
    }
    // Gemini doesn't report tokens for embeddings; a rough count keeps usage honest.
    let tokens = texts.iter().map(|t| (t.len() as u64).div_ceil(4)).sum();
    Ok(response(vectors, tokens))
}

async fn vertex(
    app: &App,
    acct: &Account,
    headers: &HeaderMap,
    model: &str,
    body: &Value,
) -> Result<(Value, Usage), (u16, Value)> {
    let Some(texts) = texts(&body["input"]) else { return Err(fail(400, "Vertex embeddings take text input")) };
    let (url, auth) = google_url(app, acct, headers, model);
    let url = url.replace(":generateContent", ":predict").replace(":streamGenerateContent", ":predict");
    let mut req = json!({ "instances": texts.iter().map(|t| json!({ "content": t })).collect::<Vec<_>>() });
    if let Some(d) = body["dimensions"].as_u64() {
        req["parameters"] = json!({ "outputDimensionality": d });
    }
    let v = read_json(send(app, acct, &url, &auth, &req).await?).await?;
    let predictions = v["predictions"].as_array().cloned().unwrap_or_default();
    if predictions.len() != texts.len() {
        return Err(fail(502, "Vertex returned a different number of embeddings"));
    }
    let tokens =
        predictions.iter().filter_map(|p| p["embeddings"]["statistics"]["token_count"].as_f64()).sum::<f64>() as u64;
    let vectors = predictions.into_iter().map(|p| p["embeddings"]["values"].clone()).collect();
    Ok(response(vectors, tokens))
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
        let (v, usage) = response(vec![json!([0.1]), json!([0.2])], 7);
        assert_eq!(v["data"][1]["index"], 1);
        assert_eq!(v["usage"]["total_tokens"], 7);
        assert_eq!(usage.input, 7);
    }
}
