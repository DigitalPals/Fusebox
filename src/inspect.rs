//! `POST /api/inspect`: what Fusebox would send upstream for a pasted request,
//! without sending it. It shows the effective model, the route, the account the
//! next new session would use, whether the body passes through or is
//! translated, the request as it would leave (credentials and account
//! identifiers redacted), the controls a translation drops, and any refusal.
//!
//! Inspecting changes nothing: it sends nothing, refreshes no token, records no
//! usage, log or metric, takes no routing turn, touches no session assignment
//! or cooldown, learns no client identity and creates no provider session (a
//! fixed placeholder stands in for one).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::Account;
use crate::ir::{self, Format};
use crate::proxy::{AttemptShape, Call, PlanInput, PlanMemo};
use crate::state::App;

/// Stands in for the provider session a real request would get, so planning
/// never creates one.
pub(crate) const PLACEHOLDER_SESSION: &str = "00000000-0000-4000-8000-000000000000";
const REDACTED: &str = "<redacted>";

#[derive(Deserialize)]
pub struct InspectBody {
    /// `responses`, `chat`, `messages` or `gemini`; guessed from the body when absent.
    #[serde(default)]
    format: Option<String>,
    /// The body exactly as a client would send it.
    body: Value,
    /// Headers a client would send, such as `user-agent`; credentials are ignored.
    #[serde(default)]
    headers: BTreeMap<String, String>,
    /// A named client whose `model-map` and model allowlist apply.
    #[serde(default)]
    client: Option<String>,
    /// For Gemini, the model named in the URL.
    #[serde(default)]
    model: Option<String>,
    /// Whether the client streams; defaults to the body's `stream` (Gemini says so in its URL).
    #[serde(default)]
    stream: Option<bool>,
}

fn format_named(name: &str) -> Option<Format> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "responses" => Format::Responses,
        "chat" | "chat-completions" | "openai" => Format::Chat,
        "messages" | "claude" | "anthropic" => Format::Claude,
        "gemini" | "generatecontent" => Format::Gemini,
        _ => return None,
    })
}

fn format_name(format: Format) -> &'static str {
    match format {
        Format::Responses => "responses",
        Format::Chat => "chat",
        Format::Claude => "messages",
        Format::Gemini => "gemini",
    }
}

/// The client format a body is written in, judged by its shape.
fn guess_format(body: &Value, headers: &HeaderMap) -> Format {
    if body.get("contents").is_some() {
        return Format::Gemini;
    }
    if body.get("input").is_some() || body.get("instructions").is_some() {
        return Format::Responses;
    }
    let claude_blocks = body["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|m| {
            m["content"].as_array().is_some_and(|blocks| {
                blocks.iter().any(|b| matches!(b["type"].as_str(), Some("tool_use" | "tool_result" | "thinking")))
            })
        })
    });
    if body.get("system").is_some() || claude_blocks || headers.contains_key("anthropic-version") {
        Format::Claude
    } else {
        Format::Chat
    }
}

/// Credentials never ride along: an inspection uses the account's own.
fn client_headers(given: &BTreeMap<String, String>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in given {
        let lower = name.trim().to_ascii_lowercase();
        if matches!(lower.as_str(), "authorization" | "x-api-key" | "x-goog-api-key" | "cookie")
            || lower == crate::usage::capture::CLIENT_HEADER
        {
            continue;
        }
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(lower.as_bytes()), HeaderValue::from_str(value.trim())) {
            headers.insert(name, value);
        }
    }
    headers
}

fn bad_request(message: impl Into<String>) -> Response {
    (axum::http::StatusCode::BAD_REQUEST, Json(json!({ "error": message.into() }))).into_response()
}

fn refusal(status: u16, message: impl Into<String>) -> Value {
    json!({ "status": status, "message": message.into() })
}

pub async fn inspect(State(app): State<Arc<App>>, Json(b): Json<InspectBody>) -> Response {
    let mut headers = client_headers(&b.headers);
    let format = match b.format.as_deref().filter(|f| !f.trim().is_empty()) {
        Some(name) => match format_named(name) {
            Some(format) => format,
            None => return bad_request(format!("unknown request format `{name}`")),
        },
        None => guess_format(&b.body, &headers),
    };
    if !b.body.is_object() {
        return bad_request("paste a JSON request body");
    }
    if let Some(id) = b.client.as_deref().filter(|id| !id.is_empty()) {
        if !app.cfg().named_clients.iter().any(|c| c.id == id) {
            return bad_request(format!("there is no named client `{id}`"));
        }
        if let Ok(value) = HeaderValue::from_str(id) {
            headers.insert(crate::usage::capture::CLIENT_HEADER, value);
        }
    }
    let stream = b.stream.unwrap_or(b.body["stream"] == true);
    Json(inspect_call(
        &app,
        Call {
            format,
            body: b.body,
            headers,
            stream,
            transport: "inspect",
            path_model: b.model.filter(|m| format == Format::Gemini && !m.is_empty()),
            session: None,
            session_source: None,
            routing_selection: None,
            pinned_account: None,
        },
    ))
    .into_response()
}

/// The inspection itself: the steps `proxy::execute` takes before sending, with
/// every one that writes state left out.
pub(crate) fn inspect_call(app: &App, mut call: Call) -> Value {
    let cfg = app.cfg();
    let requested =
        call.path_model.clone().or_else(|| call.body["model"].as_str().map(String::from)).unwrap_or_default();
    let mut out = json!({
        "format": format_name(call.format),
        "requested_model": requested,
        "sent": false,
        "notes": [
            "Nothing was sent. The account shown is the one a new session would use; an existing session stays on its own account.",
            "The provider session id is a placeholder; a real request gets the conversation's own.",
        ],
    });
    let refuse = |mut out: Value, status: u16, message: String| {
        out["refusal"] = refusal(status, message);
        out
    };
    if let Err(message) = crate::upstream::residency_requirement(&call.headers) {
        return refuse(out, 400, message);
    }
    if requested.is_empty() {
        return refuse(out, 400, "`model` is required".into());
    }
    let admitted = match crate::policy::admit_model(app, &call.headers, &requested) {
        Ok(admitted) => admitted,
        Err(d) => return refuse(out, d.status, d.message),
    };
    if let Some((client, from)) = &admitted.mapped {
        out["mapped"] = json!({ "client": client, "from": from, "to": admitted.model });
        crate::proxy::set_model(&mut call, &admitted.model);
    }
    let (base, _) = ir::split_model_suffix(&admitted.model);
    if let Some(background) = cfg.background_model.as_deref()
        && base.to_ascii_lowercase().contains("haiku")
        && crate::upstream::is_claude_code(&call.headers)
    {
        out["background_model"] = background.into();
        crate::proxy::set_model(&mut call, background);
    }
    if let Some(identity) = crate::affinity::session_identity(&call.headers, &call.body)
        .or_else(|| crate::affinity::inferred_identity(&call.headers, call.format, &call.body))
    {
        call.session = Some(identity.key);
        call.session_source = Some(identity.source);
    }
    let raw = call.path_model.clone().or_else(|| call.body["model"].as_str().map(String::from)).unwrap_or_default();
    let (group_base, _) = ir::split_model_suffix(&raw);
    if let Some(group) = cfg.model_groups.get(&group_base) {
        // Without a session a balanced group takes turns; previewing must not take one.
        let order = crate::proxy::group_order_preview(group, call.session.as_deref());
        out["group"] = json!({ "name": group_base, "order": order });
        match order.first() {
            Some(first) => crate::proxy::set_model(&mut call, first),
            None => return refuse(out, 404, format!("model group {group_base} lists no models")),
        }
    }
    let raw = call.path_model.clone().or_else(|| call.body["model"].as_str().map(String::from)).unwrap_or_default();
    let (model, suffix) = ir::split_model_suffix(&raw);
    let suffix_text = raw.trim().strip_prefix(model.as_str()).unwrap_or_default().to_string();
    let (only, model) = app.pool.route(&model);
    let model = app.pool.canonical(&model, only.as_ref());
    out["model"] = model.clone().into();
    out["reasoning_suffix"] = Some(suffix_text).filter(|s| !s.is_empty() && suffix.is_some()).into();
    let residency = crate::upstream::residency_requirement(&call.headers).ok().flatten().is_some();
    let excluded =
        match crate::proxy::ineligible_accounts(app, call.format, &call.body, &model, only.as_ref(), residency) {
            Ok(ids) => ids,
            Err(message) => return refuse(out, 400, message),
        };
    let load = app.sessions.account_load(&cfg);
    let route = app.pool.route_order(&model, &cfg, only.as_ref(), &load);
    out["route"] = serde_json::to_value(&route).unwrap_or_default();
    if !excluded.is_empty() {
        let skipped: Vec<&str> =
            route.iter().filter(|s| excluded.contains(&s.account)).map(|s| s.account.as_str()).collect();
        out["ineligible"] = json!(skipped);
    }
    let eligible = |step: &&crate::accounts::RouteStep| !excluded.contains(&step.account);
    let chosen = route
        .iter()
        .filter(eligible)
        .find(|step| step.next)
        .or_else(|| route.iter().filter(eligible).find(|step| step.state == "ready"));
    let Some((step, acct)) = chosen.and_then(|step| Some((step, app.pool.get(&step.account)?))) else {
        if residency {
            return refuse(out, 400, "Requested Codex residency requires an available Codex subscription route".into());
        }
        if let Some(until) = route.iter().filter(eligible).filter_map(|step| step.until).min() {
            let wait = (until - chrono::Utc::now()).num_seconds().max(1);
            return refuse(out, 429, format!("all accounts for {model} are rate limited; next available in {wait}s"));
        }
        return refuse(out, 404, format!("no available account serves model `{model}`"));
    };
    out["account"] = account_view(&acct, &step.upstream);
    let requested_cache = crate::cache::Requested::of(&call.body);
    let (cache_key, cache_scope) =
        crate::cache::key(&call.headers, &call.body, call.session.as_deref(), call.session_source, &cfg.prompt_cache);
    let input = PlanInput {
        format: call.format,
        body: &call.body,
        headers: &call.headers,
        stream: call.stream,
        model: &model,
        suffix: suffix.as_ref(),
        cfg: &cfg,
        requested_cache: &requested_cache,
        cache_key: cache_key.as_deref(),
        cache_scope,
        residency,
    };
    let planned = crate::upstream::inspecting(|| {
        crate::proxy::plan_attempt(&input, &acct, &step.upstream, PLACEHOLDER_SESSION, &mut PlanMemo::default())
    });
    match planned {
        Ok(plan) => {
            out["shape"] = shape_view(plan.shape);
            out["dropped_controls"] = plan.control_downgrades.unwrap_or_default().into();
            out["cache"] = serde_json::to_value(&plan.wire.policy).unwrap_or_default();
            out["request"] = json!({
                "method": "POST",
                "url": redact_url(&plan.wire.url),
                "headers": plan.wire.headers.iter().map(|(k, v)| json!([k, redact_header(k, v)])).collect::<Vec<_>>(),
                "body": redact_body(&plan.wire.payload),
                "bytes": plan.wire.payload.len(),
            });
            if !plan.wire.names.is_empty() {
                out["shortened_tool_names"] = json!(plan.wire.names);
            }
            out
        }
        Err(refused) => {
            if let Ok(shape) = crate::proxy::attempt_shape(&input, &acct) {
                out["shape"] = shape_view(shape);
            }
            out["dropped_controls"] = refused.control_downgrades.unwrap_or_default().into();
            refuse(out, refused.status, refused.message)
        }
    }
}

fn account_view(acct: &Account, upstream_model: &str) -> Value {
    json!({
        "id": acct.id,
        "label": acct.label,
        "provider": acct.provider.as_str(),
        "subscription": acct.is_oauth(),
        "upstream_model": upstream_model,
    })
}

fn shape_view(shape: AttemptShape) -> Value {
    json!({
        "path": if shape.passthrough { "passthrough" } else { "translated" },
        "wire": format_name(shape.native),
        "upstream_stream": shape.upstream_stream,
    })
}

/// Header values that carry a credential or tie the request to an account or
/// machine (Kimi's `X-Msh-Device-*` carry the host name and a device id).
fn secret_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "authorization" | "x-api-key" | "x-goog-api-key" | "cookie" | "chatgpt-account-id" | "x-goog-user-project"
    ) || ["token", "secret", "auth", "key", "password", "account", "device"].iter().any(|word| name.contains(word))
}

fn redact_header(name: &str, value: &str) -> String {
    if secret_header(name) { REDACTED.into() } else { value.into() }
}

/// Removes userinfo, credential query values and the project in a Vertex path
/// (`/projects/{id}/`) from a URL.
fn redact_url(raw: &str) -> String {
    let Ok(mut url) = url::Url::parse(raw) else { return REDACTED.into() };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    if let Some(segments) = url.path_segments() {
        let mut previous = "";
        let path: Vec<String> = segments
            .map(|segment| {
                let shown = if previous.eq_ignore_ascii_case("projects") { REDACTED } else { segment };
                previous = segment;
                shown.to_string()
            })
            .collect();
        url.set_path(&path.join("/"));
    }
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| {
            let value = if secret_header(&k) || k == "key" { REDACTED.to_string() } else { v.into_owned() };
            (k.into_owned(), value)
        })
        .collect();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(pairs);
    }
    url.to_string()
}

/// Body fields that identify the account or device, at any depth.
const SECRET_FIELDS: &[&str] = &[
    "user_id",
    "account_uuid",
    "account_id",
    "device_id",
    "project",
    "api_key",
    "access_token",
    "refresh_token",
    "id_token",
];

fn redact_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, field) in map.iter_mut() {
                if SECRET_FIELDS.contains(&key.as_str()) && !field.is_null() {
                    *field = REDACTED.into();
                } else {
                    redact_value(field);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_value),
        _ => {}
    }
}

#[cfg(test)]
pub(crate) fn redact_for_tests(value: &mut Value) {
    redact_value(value);
}

fn redact_body(payload: &[u8]) -> Value {
    match serde_json::from_slice::<Value>(payload) {
        Ok(mut body) => {
            redact_value(&mut body);
            body
        }
        Err(_) => format!("<{} bytes, not JSON>", payload.len()).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_guessed_from_the_body_shape() {
        let none = HeaderMap::new();
        assert_eq!(guess_format(&json!({"contents": []}), &none), Format::Gemini);
        assert_eq!(guess_format(&json!({"input": "hi"}), &none), Format::Responses);
        assert_eq!(guess_format(&json!({"messages": [], "system": "x"}), &none), Format::Claude);
        assert_eq!(guess_format(&json!({"messages": [{"role":"user","content":"hi"}]}), &none), Format::Chat);
        let mut claude = HeaderMap::new();
        claude.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        assert_eq!(guess_format(&json!({"messages": []}), &claude), Format::Claude);
    }

    #[test]
    fn credentials_and_account_ids_are_redacted() {
        assert_eq!(redact_header("authorization", "Bearer sk-live"), REDACTED);
        assert_eq!(redact_header("chatgpt-account-id", "acct"), REDACTED);
        assert_eq!(redact_header("x-xai-token-auth", "t"), REDACTED);
        assert_eq!(redact_header("user-agent", "claude-cli/2.1.318"), "claude-cli/2.1.318");
        assert_eq!(redact_header("X-Msh-Device-Id", "a1b2"), REDACTED);
        assert_eq!(redact_header("X-Msh-Device-Name", "my-laptop"), REDACTED);
        assert_eq!(redact_header("X-Msh-Platform", "CLIProxyAPI"), "CLIProxyAPI");
        assert_eq!(
            redact_url(
                "https://aiplatform.googleapis.com/v1/projects/my-project-123/locations/us/publishers/google/models/m:generateContent"
            ),
            "https://aiplatform.googleapis.com/v1/projects/%3Credacted%3E/locations/us/publishers/google/models/m:generateContent"
        );
        assert_eq!(redact_header("anthropic-beta", "oauth-2025-04-20"), "oauth-2025-04-20");
        assert_eq!(
            redact_url("https://user:pass@example.test/v1beta/models/m:streamGenerateContent?alt=sse&key=SECRET"),
            "https://example.test/v1beta/models/m:streamGenerateContent?alt=sse&key=%3Credacted%3E"
        );
        let body = redact_body(
            br#"{"metadata":{"user_id":"{\"device_id\":\"d\"}"},"messages":[{"project":"p","text":"keep"}]}"#,
        );
        assert_eq!(body, json!({"metadata":{"user_id":REDACTED},"messages":[{"project":REDACTED,"text":"keep"}]}));
        let headers = client_headers(&BTreeMap::from([
            ("Authorization".to_string(), "Bearer client".to_string()),
            ("x-fusebox-usage-client".to_string(), "someone".to_string()),
            ("User-Agent".to_string(), "golden".to_string()),
        ]));
        assert_eq!(headers.len(), 1, "only the user agent survives");
    }
}
