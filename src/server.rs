use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{DefaultBodyLimit, Extension, FromRequest, FromRequestParts, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures::StreamExt;
use serde_json::{Value, json};
use tower_http::cors::CorsLayer;

use crate::formats::{self, Frame};
use crate::ir::Format;
use crate::proxy::{self, Call, FrameStream, Reply};
use crate::state::App;

mod assets;
mod request_body;

pub fn router(app: Arc<App>) -> Router {
    let api = Router::new()
        .route("/v1/chat/completions", post(chat))
        .route("/v1/messages", post(messages))
        .route("/v1/messages/count_tokens", post(count_tokens))
        .route("/v1/responses", post(responses).get(responses_ws))
        .route("/backend-api/codex/responses", post(responses).get(responses_ws))
        .route("/v1/completions", post(completions))
        .route("/v1/responses/compact", post(compact))
        .route("/v1/images/generations", post(image_generations))
        .route("/v1/images/edits", post(image_edits))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/videos/{kind}", post(video_create).get(video_status))
        .route("/v1/models", get(models))
        .route("/v1/codex/models.json", get(codex_models))
        .route("/v1beta/models", get(gemini_models))
        .route("/v1beta/models/{*rest}", post(gemini))
        .layer(middleware::from_fn_with_state(app.clone(), request_body::decoded_body))
        .layer(middleware::from_fn_with_state(app.clone(), request_body::encoded_body))
        .layer(middleware::from_fn_with_state(app.clone(), client_auth))
        .layer(DefaultBodyLimit::max(request_body::MAX_BODY_SIZE))
        .layer(CorsLayer::permissive());

    Router::new()
        .merge(api)
        .route("/api/usage-ingest", post(crate::usage::api::ingest).layer(DefaultBodyLimit::max(512 << 10)))
        // Dashboard JSON compresses well (the request log ~20x); streams and
        // WebSocket upgrades are left alone by the layer's default predicate.
        .nest("/api", crate::mgmt::router(app.clone()).layer(tower_http::compression::CompressionLayer::new()))
        .route("/", get(ui_index))
        .route("/ui/{file}", get(ui_asset))
        .route("/ui/fonts/{file}", get(ui_font))
        .route("/favicon.ico", get(favicon))
        .route("/manifest.webmanifest", get(manifest))
        .route("/sw.js", get(service_worker))
        .route("/ui/icons/{file}", get(ui_icon))
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(crate::metrics::readyz))
        .merge(
            Router::new()
                .route("/metrics", get(crate::metrics::metrics))
                .layer(middleware::from_fn_with_state(app.clone(), crate::mgmt::auth)),
        )
        // Before anything else, so every check and log line sees the real client.
        .layer(middleware::from_fn_with_state(app.clone(), crate::source::layer))
        .with_state(app)
}

// ------------------------------------------------------------------------ auth

/// The access granted at the handshake. A socket must reconnect if its key or
/// client identity changes, so stale headers cannot bypass a client's limits.
#[derive(Clone)]
pub(crate) enum ClientAuth {
    Open,
    Shared(String),
    Named { id: String, key: String },
}

impl ClientAuth {
    /// The named client this access belongs to, while it still exists.
    pub(crate) fn named<'a>(&self, cfg: &'a crate::config::Config) -> Option<&'a crate::config::NamedClient> {
        match self {
            Self::Named { id, .. } => cfg.named_clients.iter().find(|c| &c.id == id),
            _ => None,
        }
    }

    pub(crate) fn valid(&self, cfg: &crate::config::Config) -> bool {
        match self {
            Self::Open => cfg.api_keys.is_empty() && cfg.named_clients.is_empty(),
            Self::Shared(key) => cfg.api_keys.iter().any(|k| crate::mgmt::constant_eq(k, key)),
            Self::Named { id, key } => {
                cfg.named_clients.iter().any(|c| &c.id == id && crate::mgmt::constant_eq(&c.key, key))
            }
        }
    }
}

async fn client_auth(State(app): State<Arc<App>>, mut req: Request, next: Next) -> Response {
    let cfg = app.cfg();
    let h = req.headers();
    let get = |n: &str| h.get(n).and_then(|v| v.to_str().ok()).map(str::trim).map(String::from);
    let provided = get("authorization")
        .and_then(|v| v.strip_prefix("Bearer ").map(|s| s.trim().to_string()))
        .or_else(|| get("x-api-key"))
        .or_else(|| get("x-goog-api-key"))
        .or_else(|| {
            req.uri().query().and_then(|q| {
                url::form_urlencoded::parse(q.as_bytes()).find(|(k, _)| k == "key").map(|(_, v)| v.into_owned())
            })
        });
    // Collector credentials are ingestion-only even on an otherwise open proxy.
    let collector_key = provided.as_deref().is_some_and(|k| k.starts_with("fbxc_"));
    let named = provided.as_ref().and_then(|k| cfg.named_clients.iter().find(|c| crate::mgmt::constant_eq(&c.key, k)));
    let open = cfg.api_keys.is_empty() && cfg.named_clients.is_empty();
    if !collector_key
        && (open
            || named.is_some()
            || provided.as_ref().is_some_and(|k| cfg.api_keys.iter().any(|a| crate::mgmt::constant_eq(a, k))))
    {
        // A proxy without client keys trusts whoever can reach it; on this machine
        // alone, that must not include web pages.
        if open
            && cfg.is_loopback()
            && let Some(problem) = crate::mgmt::foreign_page(&req)
        {
            let format = format_for_path(req.uri().path());
            return (StatusCode::FORBIDDEN, axum::Json(formats::error_body(format, 403, problem))).into_response();
        }
        let websocket = if req.method() == axum::http::Method::GET
            && matches!(req.uri().path(), "/v1/responses" | "/backend-api/codex/responses")
        {
            let (mut parts, body) = req.into_parts();
            // The extractor consumes OnUpgrade; preserve it for the route handler.
            let extensions = parts.extensions.clone();
            let valid = WebSocketUpgrade::from_request_parts(&mut parts, &app).await.is_ok();
            parts.extensions = extensions;
            req = Request::from_parts(parts, body);
            valid
        } else {
            false
        };
        let source = req.extensions().get::<crate::source::Source>().cloned();
        if let Some(client) = named
            && let Err(denied) = app.clients.admit(client, req.headers(), !websocket)
        {
            if let Some(source) = &source
                && !crate::policy::address_allowed(client, req.headers())
            {
                address_refused(&app, client, source);
            }
            let format = format_for_path(req.uri().path());
            let status = StatusCode::from_u16(denied.status).unwrap_or(StatusCode::FORBIDDEN);
            let mut resp =
                (status, axum::Json(formats::error_body(format, denied.status, &denied.message))).into_response();
            if let Some(wait) = denied.retry_after {
                resp.headers_mut().insert(header::RETRY_AFTER, wait.into());
            }
            return resp;
        }
        // Never trust an incoming internal scope header. Query-string credentials
        // and header credentials get the same namespace without forwarding keys.
        let scope = crate::affinity::scope_for_key(provided.as_deref());
        req.headers_mut().remove(crate::usage::capture::REQUEST_HEADER);
        req.headers_mut().remove(crate::usage::capture::CLIENT_HEADER);
        let identity = named
            .map(|c| c.id.clone())
            .or_else(|| provided.as_ref().filter(|_| !cfg.api_keys.is_empty()).map(|_| scope.clone()));
        // A WebSocket's handshake isn't a request; each turn records its address instead.
        if let (Some(client), Some(source), false) = (named, &source, websocket) {
            address_used(&app, client, source);
        }
        if let Some(source) = &source {
            app.addresses.seen(identity.as_deref().unwrap_or(""), source);
        }
        if let Some(identity) = identity {
            app.clients.touch(&identity);
            if let Ok(value) = HeaderValue::from_str(&identity) {
                req.headers_mut().insert(crate::usage::capture::CLIENT_HEADER, value);
            }
        }
        req.headers_mut().remove(crate::affinity::LEGACY_SCOPE_HEADER);
        req.headers_mut().insert(crate::affinity::SCOPE_HEADER, HeaderValue::from_str(&scope).unwrap());
        let auth = match named {
            Some(client) => ClientAuth::Named { id: client.id.clone(), key: client.key.clone() },
            None if open => ClientAuth::Open,
            None => ClientAuth::Shared(provided.unwrap()),
        };
        req.extensions_mut().insert(auth);
        return next.run(req).await;
    }
    let format = format_for_path(req.uri().path());
    (StatusCode::UNAUTHORIZED, axum::Json(formats::error_body(format, 401, "invalid or missing API key")))
        .into_response()
}

/// A named client's admitted request: its address is recorded, and is news when new.
pub(crate) fn address_used(app: &Arc<App>, client: &crate::config::NamedClient, source: &crate::source::Source) {
    if app.addresses.note(&client.id, source) == crate::addresses::Noted::New {
        tracing::info!(client = %client.id, address = %source.ip, "client key used from a new address");
        crate::push::client_address(app, client, source, false);
    }
}

/// A named client's key refused by its address list.
pub(crate) fn address_refused(app: &Arc<App>, client: &crate::config::NamedClient, source: &crate::source::Source) {
    tracing::warn!(client = %client.id, address = %source.ip, "refused a client key from outside its address list");
    crate::push::client_address(app, client, source, true);
}

fn format_for_path(path: &str) -> Format {
    if path.starts_with("/v1/messages") {
        Format::Claude
    } else if path.starts_with("/v1beta") {
        Format::Gemini
    } else if path.contains("/responses") {
        Format::Responses
    } else {
        Format::Chat
    }
}

// -------------------------------------------------------------------- handlers

fn parse_body(app: &Arc<App>, format: Format, headers: &HeaderMap, body: &Bytes) -> Result<Value, Box<Response>> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| Box::new(body_error(app, format, headers, 400, "request body must be a JSON object")))
}

/// Record rejected input without assigning a provider or recording a provider attempt.
fn body_error(app: &Arc<App>, format: Format, headers: &HeaderMap, status: u16, message: &str) -> Response {
    let mut tracker = proxy::Tracker::new(app, format, false, "http", "");
    tracker.client_app(headers);
    tracker.finish(status, &crate::ir::Usage::default(), Some(message.into()));
    reply(format, Reply::Error(status, formats::error_body(format, status, message)), false)
}

async fn run(app: Arc<App>, format: Format, headers: HeaderMap, body: Bytes) -> Response {
    let body = match parse_body(&app, format, &headers, &body) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let stream = body["stream"].as_bool().unwrap_or(false);
    let call = Call {
        format,
        body,
        headers,
        stream,
        transport: "http",
        path_model: None,
        session: None,
        session_source: None,
        routing_selection: None,
    };
    reply(format, proxy::execute(app, call).await, false)
}

async fn chat(State(app): State<Arc<App>>, headers: HeaderMap, body: Bytes) -> Response {
    run(app, Format::Chat, headers, body).await
}

async fn messages(State(app): State<Arc<App>>, headers: HeaderMap, body: Bytes) -> Response {
    run(app, Format::Claude, headers, body).await
}

async fn responses(State(app): State<Arc<App>>, headers: HeaderMap, body: Bytes) -> Response {
    run(app, Format::Responses, headers, body).await
}

async fn responses_ws(
    State(app): State<Arc<App>>,
    Extension(auth): Extension<ClientAuth>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    ws.max_message_size(256 << 20)
        .max_frame_size(256 << 20)
        .on_upgrade(move |socket| crate::ws::handle(app, headers, auth, socket))
}

fn outcome(o: crate::media::Outcome) -> Response {
    match o {
        Ok(v) => axum::Json(v).into_response(),
        Err((status, body)) => {
            (StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), axum::Json(body)).into_response()
        }
    }
}

/// A named client may only use the models its limits allow, media models included.
fn model_denied(app: &App, headers: &HeaderMap, body: &Value, fallback: &str) -> Option<Response> {
    let model = body["model"].as_str().filter(|m| !m.is_empty()).unwrap_or(fallback);
    let denied = crate::policy::check_model(app, headers, model).err()?;
    Some(outcome(Err((denied.status, formats::error_body(Format::Chat, denied.status, &denied.message)))))
}

async fn image_generations(State(app): State<Arc<App>>, headers: HeaderMap, body: Bytes) -> Response {
    match parse_body(&app, Format::Chat, &headers, &body) {
        Ok(v) => match model_denied(&app, &headers, &v, crate::media::DEFAULT_IMAGE_MODEL) {
            Some(r) => r,
            None => outcome(crate::media::images(app, headers, v, false).await),
        },
        Err(r) => *r,
    }
}

async fn image_edits(State(app): State<Arc<App>>, req: Request) -> Response {
    let headers = req.headers().clone();
    let multipart =
        headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|c| c.starts_with("multipart/"));
    let body = if multipart {
        let form = match axum::extract::Multipart::from_request(req, &app).await {
            Ok(f) => f,
            Err(e) => return outcome(Err((400, formats::error_body(Format::Chat, 400, &e.body_text())))),
        };
        match crate::media::multipart_to_json(form).await {
            Ok(v) => v,
            Err(e) => return outcome(Err((400, formats::error_body(Format::Chat, 400, &e)))),
        }
    } else {
        let bytes = match axum::body::to_bytes(req.into_body(), request_body::MAX_BODY_SIZE).await {
            Ok(b) => b,
            Err(e) => return outcome(Err((400, formats::error_body(Format::Chat, 400, &e.to_string())))),
        };
        match parse_body(&app, Format::Chat, &headers, &bytes) {
            Ok(v) => v,
            Err(r) => return *r,
        }
    };
    if let Some(r) = model_denied(&app, &headers, &body, crate::media::DEFAULT_IMAGE_MODEL) {
        return r;
    }
    outcome(crate::media::images(app, headers, body, true).await)
}

async fn video_create(
    State(app): State<Arc<App>>,
    Path(kind): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !matches!(kind.as_str(), "generations" | "edits" | "extensions") {
        return outcome(Err((404, formats::error_body(Format::Chat, 404, "unknown video endpoint"))));
    }
    match parse_body(&app, Format::Chat, &headers, &body) {
        Ok(v) => match model_denied(&app, &headers, &v, crate::media::DEFAULT_VIDEO_MODEL) {
            Some(r) => r,
            None => outcome(crate::media::video_create(app, headers, v, &kind).await),
        },
        Err(r) => *r,
    }
}

async fn embeddings(State(app): State<Arc<App>>, headers: HeaderMap, body: Bytes) -> Response {
    match parse_body(&app, Format::Chat, &headers, &body) {
        Ok(v) => outcome(crate::embeddings::embeddings(app, headers, v).await),
        Err(r) => *r,
    }
}

#[derive(serde::Deserialize)]
pub struct PlaygroundBody {
    model: String,
    prompt: String,
}

/// The dashboard's playground: one prompt to one model, streamed back as Chat
/// Completions. It goes through routing like any request and shows on the Usage page
/// as the client `dashboard`.
pub async fn playground(
    State(app): State<Arc<App>>,
    incoming: HeaderMap,
    axum::Json(b): axum::Json<PlaygroundBody>,
) -> Response {
    let mut headers = HeaderMap::new();
    // Where the dashboard user is, as `source::layer` resolved it.
    if let Some(source) = incoming.get(crate::source::HEADER) {
        headers.insert(crate::source::HEADER, source.clone());
    }
    let scope = crate::affinity::scope_for_key(Some("fusebox-dashboard-playground"));
    if let (Ok(scope), Ok(client)) = (HeaderValue::from_str(&scope), HeaderValue::from_str("dashboard")) {
        headers.insert(crate::affinity::SCOPE_HEADER, scope);
        headers.insert(crate::usage::capture::CLIENT_HEADER, client);
    }
    headers.insert(header::USER_AGENT, HeaderValue::from_static("fusebox-playground"));
    let body = json!({
        "model": b.model.trim(),
        "stream": true,
        "stream_options": { "include_usage": true },
        "messages": [{ "role": "user", "content": b.prompt }],
    });
    let call = Call {
        format: Format::Chat,
        body,
        headers,
        stream: true,
        transport: "http",
        path_model: None,
        session: None,
        session_source: None,
        routing_selection: None,
    };
    reply(Format::Chat, proxy::execute(app, call).await, false)
}

async fn video_status(State(app): State<Arc<App>>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    outcome(crate::media::video_status(app, headers, id).await)
}

async fn compact(State(app): State<Arc<App>>, headers: HeaderMap, body: Bytes) -> Response {
    match parse_body(&app, Format::Responses, &headers, &body) {
        Ok(v) => match model_denied(&app, &headers, &v, "") {
            Some(r) => r,
            None => outcome(crate::media::compact(app, headers, v).await),
        },
        Err(r) => *r,
    }
}

/// Legacy `/v1/completions`, served through the chat pipeline.
async fn completions(State(app): State<Arc<App>>, headers: HeaderMap, body: Bytes) -> Response {
    let body = match parse_body(&app, Format::Chat, &headers, &body) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let prompt = match &body["prompt"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    };
    let mut chat = json!({ "model": body["model"], "messages": [{ "role": "user", "content": prompt }] });
    for k in [
        "max_tokens",
        "temperature",
        "top_p",
        "stop",
        "stream",
        "stream_options",
        "user",
        "metadata",
        "prompt_cache_key",
    ] {
        if !body[k].is_null() {
            chat[k] = body[k].clone();
        }
    }
    let stream = body["stream"].as_bool().unwrap_or(false);
    let call = Call {
        format: Format::Chat,
        body: chat,
        headers,
        stream,
        transport: "http",
        path_model: None,
        session: None,
        session_source: None,
        routing_selection: None,
    };
    match proxy::execute(app, call).await {
        Reply::Json(v) => {
            let choice = &v["choices"][0];
            axum::Json(json!({
                "id": v["id"].as_str().map(|s| s.replace("chatcmpl-", "cmpl-")),
                "object": "text_completion", "created": v["created"], "model": v["model"],
                "choices": [{
                    "text": choice["message"]["content"].as_str().unwrap_or_default(), "index": 0,
                    "logprobs": null, "finish_reason": choice["finish_reason"],
                }],
                "usage": v["usage"],
            }))
            .into_response()
        }
        Reply::Stream { frames, account } => {
            let frames: FrameStream = Box::pin(frames.filter_map(|f| async move {
                let Ok(v) = serde_json::from_str::<Value>(&f.data) else { return Some(f) };
                if v["error"].is_object() {
                    return Some(f);
                }
                let choice = &v["choices"][0];
                let text = choice["delta"]["content"].as_str().unwrap_or_default();
                if text.is_empty() && choice["finish_reason"].is_null() && v["usage"].is_null() {
                    return None;
                }
                let mut out = json!({
                    "id": v["id"].as_str().map(|s| s.replace("chatcmpl-", "cmpl-")), "object": "text_completion", "created": v["created"], "model": v["model"],
                    "choices": if choice.is_null() { json!([]) } else { json!([{
                        "text": text, "index": 0, "logprobs": null, "finish_reason": choice["finish_reason"],
                    }]) },
                });
                if !v["usage"].is_null() {
                    out["usage"] = v["usage"].clone();
                }
                Some(Frame::data(out.to_string()))
            }));
            reply(Format::Chat, Reply::Stream { frames, account }, false)
        }
        other => reply(Format::Chat, other, false),
    }
}

async fn count_tokens(State(app): State<Arc<App>>, headers: HeaderMap, body: Bytes) -> Response {
    match parse_body(&app, Format::Claude, &headers, &body) {
        Ok(v) => {
            let (count, estimated) = proxy::count_tokens(app, headers, v).await;
            token_count_response(count, estimated)
        }
        Err(r) => *r,
    }
}

fn token_count_response(body: Value, estimated: bool) -> Response {
    ([("x-fusebox-token-count-estimated", if estimated { "true" } else { "false" })], axum::Json(body)).into_response()
}

async fn gemini(
    State(app): State<Arc<App>>,
    Path(rest): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some((model, action)) = rest.rsplit_once(':') else {
        return reply(
            Format::Gemini,
            Reply::Error(404, formats::error_body(Format::Gemini, 404, "unknown method")),
            false,
        );
    };
    let body = match parse_body(&app, Format::Gemini, &headers, &body) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let stream = match action {
        "generateContent" => false,
        "streamGenerateContent" => true,
        "countTokens" => return token_count_response(json!({ "totalTokens": proxy::estimate_tokens(&body) }), true),
        _ => {
            return reply(
                Format::Gemini,
                Reply::Error(404, formats::error_body(Format::Gemini, 404, "unknown method")),
                false,
            );
        }
    };
    let json_array = stream && q.get("alt").map(String::as_str) != Some("sse");
    let call = Call {
        format: Format::Gemini,
        body,
        headers,
        stream,
        transport: "http",
        path_model: Some(model.trim_start_matches("models/").to_string()),
        session: None,
        session_source: None,
        routing_selection: None,
    };
    reply(Format::Gemini, proxy::execute(app, call).await, json_array)
}

/// Model ids clients can ask for: every account's, plus model groups (as `fusebox`'s).
fn listed_models(app: &App) -> Vec<(String, &'static str)> {
    let mut list: Vec<(String, &'static str)> =
        app.pool.models().into_iter().map(|(id, provider)| (id, provider.as_str())).collect();
    for name in app.cfg().model_groups.keys() {
        if !list.iter().any(|(id, _)| id == name) {
            list.push((name.clone(), "fusebox"));
        }
    }
    list
}

async fn models(State(app): State<Arc<App>>, Extension(auth): Extension<ClientAuth>) -> Response {
    let created = app.started.timestamp();
    let cfg = app.cfg();
    let data: Vec<Value> = listed_models(&app)
        .into_iter()
        .filter(|(id, _)| auth.named(&cfg).is_none_or(|client| crate::policy::model_allowed(client, id)))
        .map(|(id, provider)| {
            let caps = crate::model_catalog::capabilities(&app, &id);
            let mut model = json!({
                "id": id, "object": "model", "created": created, "owned_by": provider,
                "type": "model", "display_name": id, "created_at": app.started.to_rfc3339(),
                "capabilities": caps,
            });
            if let Some(n) = caps.context_window {
                model["context_window"] = n.into();
                model["max_input_tokens"] = n.into();
            }
            if let Some(n) = caps.max_output_tokens {
                model["max_output_tokens"] = n.into();
                model["max_tokens"] = n.into();
            }
            model
        })
        .collect();
    let first = data.first().and_then(|m| m["id"].as_str()).map(String::from);
    let last = data.last().and_then(|m| m["id"].as_str()).map(String::from);
    axum::Json(json!({ "object": "list", "data": data, "has_more": false, "first_id": first, "last_id": last }))
        .into_response()
}

async fn gemini_models(State(app): State<Arc<App>>, Extension(auth): Extension<ClientAuth>) -> Response {
    let cfg = app.cfg();
    let models: Vec<Value> = listed_models(&app)
        .into_iter()
        .filter(|(id, _)| auth.named(&cfg).is_none_or(|client| crate::policy::model_allowed(client, id)))
        .map(|(id, _)| {
            let caps = crate::model_catalog::capabilities(&app, &id);
            let mut model = json!({
                "name": format!("models/{id}"), "displayName": id, "version": "001",
                "supportedGenerationMethods": ["generateContent", "streamGenerateContent", "countTokens"],
                "capabilities": caps,
            });
            if let Some(n) = caps.context_window {
                model["inputTokenLimit"] = n.into();
            }
            if let Some(n) = caps.max_output_tokens {
                model["outputTokenLimit"] = n.into();
            }
            model
        })
        .collect();
    axum::Json(json!({ "models": models })).into_response()
}

/// Codex needs its full provider template for custom aliases, rather than an
/// OpenAI model-name list. Cached discovery supplies the versioned template.
async fn codex_models(
    State(app): State<Arc<App>>,
    Extension(auth): Extension<ClientAuth>,
    Query(query): Query<std::collections::BTreeMap<String, String>>,
) -> Response {
    let cfg = app.cfg();
    let version = |s: &str| -> Option<Vec<u64>> { s.split('.').map(str::parse).collect::<Result<Vec<_>, _>>().ok() };
    let requested_version = query.get("client_version").and_then(|s| version(s));
    let models: Vec<Value> = listed_models(&app)
        .into_iter()
        .filter(|(id, _)| query.get("model").is_none_or(|selected| selected == id))
        .filter(|(id, _)| auth.named(&cfg).is_none_or(|client| crate::policy::model_allowed(client, id)))
        .filter_map(|(id, _)| crate::model_catalog::codex_template(&app, &id))
        .filter(|m| {
            requested_version.as_ref().is_none_or(|requested| {
                m["minimal_client_version"].as_str().and_then(version).is_none_or(|min| requested >= &min)
            })
        })
        .collect();
    if models.is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, axum::Json(json!({"error":"No complete Codex model metadata is available for the selected models/client version. Check for new models in Fusebox or use the client's built-in catalog."}))).into_response();
    }
    let body = serde_json::to_vec(&json!({ "models": models })).unwrap();
    if body.len() > 1_048_576 {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(
                json!({"error":"Codex catalogs must fit in 1 MiB. Select one model with the model query parameter."}),
            ),
        )
            .into_response();
    }
    ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "private, no-cache")], body).into_response()
}

// -------------------------------------------------------------------- replies

fn reply(_format: Format, r: Reply, json_array: bool) -> Response {
    match r {
        Reply::Json(v) => axum::Json(v).into_response(),
        Reply::Error(status, v) => {
            let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
            (code, axum::Json(v)).into_response()
        }
        Reply::Stream { frames, account } => {
            let (body, ctype) = if json_array {
                (json_array_body(frames), "application/json")
            } else {
                (sse_body(frames), "text/event-stream")
            };
            let mut resp = Response::new(body);
            let h = resp.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static(ctype));
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            h.insert("x-accel-buffering", HeaderValue::from_static("no"));
            if let Ok(v) = HeaderValue::from_str(&account) {
                h.insert("x-fusebox-account", v);
            }
            resp
        }
    }
}

const KEEPALIVE: Duration = Duration::from_secs(15);

/// One write carries at most this much of what is already waiting.
const BATCH_BYTES: usize = 64 << 10;

/// The next frame, or None when the keepalive interval passed first. One timer is
/// reused for the whole stream instead of a new one per frame.
async fn next_or_keepalive(
    frames: &mut FrameStream,
    keepalive: &mut std::pin::Pin<&mut tokio::time::Sleep>,
) -> Option<Option<Frame>> {
    tokio::select! {
        biased;
        next = frames.next() => Some(next),
        _ = keepalive.as_mut() => None,
    }
}

/// Frames that are ready right now (one upstream chunk often carries several),
/// so they leave in one write. Never waits.
fn drain_ready(frames: &mut FrameStream, mut write: impl FnMut(Frame) -> usize) -> bool {
    use futures::FutureExt;
    let mut written = 0;
    while written < BATCH_BYTES {
        match frames.next().now_or_never() {
            Some(Some(f)) => written += write(f),
            Some(None) => return true,
            None => break,
        }
    }
    false
}

fn sse_body(mut frames: FrameStream) -> Body {
    Body::from_stream(async_stream::stream! {
        let keepalive = tokio::time::sleep(KEEPALIVE);
        tokio::pin!(keepalive);
        loop {
            match next_or_keepalive(&mut frames, &mut keepalive).await {
                Some(Some(f)) => {
                    let mut buf = Vec::with_capacity(f.data.len() + 48);
                    f.write_sse(&mut buf);
                    let ended = drain_ready(&mut frames, |f| {
                        let before = buf.len();
                        f.write_sse(&mut buf);
                        buf.len() - before
                    });
                    yield Ok::<_, Infallible>(Bytes::from(buf));
                    if ended {
                        break;
                    }
                }
                Some(None) => break,
                None => yield Ok(Bytes::from_static(b": keepalive\n\n")),
            }
            keepalive.as_mut().reset(tokio::time::Instant::now() + KEEPALIVE);
        }
    })
}

/// Gemini without `alt=sse` streams a JSON array.
fn json_array_body(mut frames: FrameStream) -> Body {
    Body::from_stream(async_stream::stream! {
        let mut first = true;
        yield Ok::<_, Infallible>(Bytes::from_static(b"["));
        let keepalive = tokio::time::sleep(KEEPALIVE);
        tokio::pin!(keepalive);
        loop {
            match next_or_keepalive(&mut frames, &mut keepalive).await {
                Some(Some(f)) => {
                    let mut buf = Vec::with_capacity(f.data.len() + 4);
                    let mut append = |Frame { data, .. }: Frame| {
                        if !first {
                            buf.extend_from_slice(b",\r\n");
                        }
                        first = false;
                        buf.extend_from_slice(data.as_bytes());
                        data.len()
                    };
                    append(f);
                    let ended = drain_ready(&mut frames, &mut append);
                    yield Ok::<_, Infallible>(Bytes::from(buf));
                    if ended {
                        break;
                    }
                }
                Some(None) => break,
                None => yield Ok(Bytes::from_static(b"\n")),
            }
            keepalive.as_mut().reset(tokio::time::Instant::now() + KEEPALIVE);
        }
        yield Ok(Bytes::from_static(b"]"));
    })
}

// ------------------------------------------------------------------------- ui

const FAVICON: &[u8] = include_bytes!("../ui/favicon.ico");

async fn ui_index(headers: HeaderMap) -> Response {
    assets::serve("/", &headers, None)
}

// `?v=` names the version the index linked to; see assets.rs.
async fn ui_asset(
    Path(file): Path<String>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    headers: HeaderMap,
) -> Response {
    assets::serve(&file, &headers, assets::version(query.as_deref()).as_deref())
}

// Self-hosted fonts (latin subset, SIL OFL 1.1; licences in ui/fonts). Their names
// change with their contents, so they can be cached for a long time.
const FONTS: &[(&str, &[u8])] = &[
    ("ibm-plex-sans.woff2", include_bytes!("../ui/fonts/ibm-plex-sans.woff2")),
    ("ibm-plex-sans-condensed-500.woff2", include_bytes!("../ui/fonts/ibm-plex-sans-condensed-500.woff2")),
    ("ibm-plex-sans-condensed-600.woff2", include_bytes!("../ui/fonts/ibm-plex-sans-condensed-600.woff2")),
    ("dm-mono-400.woff2", include_bytes!("../ui/fonts/dm-mono-400.woff2")),
    ("dm-mono-500.woff2", include_bytes!("../ui/fonts/dm-mono-500.woff2")),
];

async fn ui_font(Path(file): Path<String>) -> Response {
    match FONTS.iter().find(|(name, _)| *name == file) {
        Some((_, body)) => (
            [(header::CONTENT_TYPE, "font/woff2"), (header::CACHE_CONTROL, "public, max-age=31536000, immutable")],
            *body,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

// Installing the dashboard as an app: the manifest, its icons, and the service worker,
// served from the root so its scope covers the dashboard at `/`.
const ICONS: &[(&str, &[u8])] = &[
    ("icon-192.png", include_bytes!("../ui/icons/icon-192.png")),
    ("icon-512.png", include_bytes!("../ui/icons/icon-512.png")),
    ("maskable-512.png", include_bytes!("../ui/icons/maskable-512.png")),
    ("apple-touch-icon.png", include_bytes!("../ui/icons/apple-touch-icon.png")),
];

async fn manifest(headers: HeaderMap) -> Response {
    assets::serve("manifest.webmanifest", &headers, None)
}

async fn service_worker(headers: HeaderMap) -> Response {
    assets::serve("sw.js", &headers, None)
}

async fn ui_icon(Path(file): Path<String>) -> Response {
    match ICONS.iter().find(|(name, _)| *name == file) {
        Some((_, body)) => {
            ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=86400")], *body)
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn favicon() -> Response {
    ([(header::CONTENT_TYPE, "image/x-icon"), (header::CACHE_CONTROL, "public, max-age=86400")], FAVICON)
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn serves_the_installable_app() {
        let cfg = crate::config::Config { auth_dir: "/nonexistent".into(), ..Default::default() };
        let app = App::new(cfg, "/nonexistent/config.yaml".into());
        for (path, ctype) in [
            ("/manifest.webmanifest", "application/manifest+json"),
            ("/sw.js", "text/javascript; charset=utf-8"),
            ("/ui/icons/icon-192.png", "image/png"),
            ("/ui/icons/maskable-512.png", "image/png"),
            ("/ui/icons/apple-touch-icon.png", "image/png"),
        ] {
            let resp = router(app.clone()).oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{path}");
            assert_eq!(resp.headers()[header::CONTENT_TYPE], ctype, "{path}");
        }
        let missing = router(app.clone()).oneshot(Request::get("/ui/icons/nope.png").body(Body::empty()).unwrap());
        assert_eq!(missing.await.unwrap().status(), StatusCode::NOT_FOUND);

        // Every icon the manifest names is served.
        let resp = router(app.clone()).oneshot(Request::get("/manifest.webmanifest").body(Body::empty()).unwrap());
        let manifest: Value =
            serde_json::from_slice(&to_bytes(resp.await.unwrap().into_body(), 1 << 16).await.unwrap()).unwrap();
        assert_eq!(manifest["scope"], "/");
        for icon in manifest["icons"].as_array().unwrap() {
            let src = icon["src"].as_str().unwrap();
            let resp = router(app.clone()).oneshot(Request::get(src).body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{src}");
        }
    }
}
