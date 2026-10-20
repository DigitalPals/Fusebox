//! End-to-end checks for routing priority, model groups, the background model, stream
//! failover, client limits, the browser guards, readiness, metrics, embeddings,
//! webhooks and the playground, over real HTTP against a local mock provider.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::config::{CompatEntry, Config, KeyEntry, ModelGroup, NamedClient};
use crate::state::App;

/// What the mock provider does for one account key and model.
#[derive(Clone, Copy, PartialEq)]
enum Act {
    Answer,
    /// 200 and an event stream that never sends anything.
    Stall,
    /// 200, then an overload error before any content.
    Overload,
    /// 404: the model doesn't exist.
    Missing,
    /// An upstream gateway may still be running the generation.
    GatewayTimeout,
}

#[derive(Default)]
struct Mock {
    acts: Mutex<HashMap<(String, String), Act>>,
    calls: Mutex<Vec<(String, String)>>,
    hooks: Mutex<Vec<Value>>,
    embedding_calls: Mutex<Vec<(String, Value)>>,
    embedding_delay: std::sync::atomic::AtomicBool,
    embedding_active: std::sync::atomic::AtomicUsize,
    embedding_peak: std::sync::atomic::AtomicUsize,
}

fn key(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .trim_start_matches("Bearer ")
        .to_string()
}

async fn responses(State(mock): State<Arc<Mock>>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let account = key(&headers);
    let model = body["model"].as_str().unwrap_or_default().to_string();
    mock.calls.lock().push((account.clone(), model.clone()));
    let act = mock.acts.lock().get(&(account.clone(), model.clone())).copied().unwrap_or(Act::Answer);
    let created = json!({"type":"response.created","response":{"id":"resp_1","model":model}});
    let sse = |events: Vec<Value>| {
        let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
        ([("content-type", "text/event-stream")], body).into_response()
    };
    match act {
        Act::GatewayTimeout => {
            (StatusCode::GATEWAY_TIMEOUT, Json(json!({"error":{"message":"generation still running"}}))).into_response()
        }
        Act::Missing => {
            (StatusCode::NOT_FOUND, Json(json!({"error":{"message":"no such model","code":"model_not_found"}})))
                .into_response()
        }
        Act::Overload => sse(vec![
            created,
            json!({"type":"response.failed","response":{"error":{"code":"server_is_overloaded","message":"busy"}}}),
        ]),
        Act::Stall => {
            let stream = async_stream::stream! {
                tokio::time::sleep(Duration::from_secs(30)).await;
                yield Ok::<_, std::convert::Infallible>(String::new());
            };
            ([("content-type", "text/event-stream")], axum::body::Body::from_stream(stream)).into_response()
        }
        Act::Answer => {
            let text = format!("{account}:{model}");
            let done = json!({"type":"response.completed","response":{"id":"resp_1","object":"response","status":"completed","model":model,
                "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}],
                "usage":{"input_tokens":10,"input_tokens_details":{"cached_tokens":0},"output_tokens":2}}});
            if body["stream"] == true {
                sse(vec![
                    created,
                    json!({"type":"response.output_text.delta","delta":text,"output_index":0,"content_index":0}),
                    done,
                ])
            } else {
                Json(done["response"].clone()).into_response()
            }
        }
    }
}

async fn embeddings(State(mock): State<Arc<Mock>>, Json(body): Json<Value>) -> Response {
    mock.calls.lock().push(("embed".into(), body["model"].as_str().unwrap_or_default().into()));
    Json(json!({"object":"list","model":body["model"],"data":[{"object":"embedding","index":0,"embedding":[0.5,0.25]}],
        "usage":{"prompt_tokens":3,"total_tokens":3}}))
    .into_response()
}

async fn hook(State(mock): State<Arc<Mock>>, Json(body): Json<Value>) -> Response {
    mock.hooks.lock().push(body);
    StatusCode::NO_CONTENT.into_response()
}

async fn google_embeddings(
    State(mock): State<Arc<Mock>>,
    Path(path): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    mock.embedding_calls.lock().push((path.clone(), body.clone()));
    if mock.embedding_delay.load(std::sync::atomic::Ordering::Relaxed) {
        use std::sync::atomic::Ordering::Relaxed;
        let active = mock.embedding_active.fetch_add(1, Relaxed) + 1;
        mock.embedding_peak.fetch_max(active, Relaxed);
        let fail = body["instances"].as_array().is_some_and(|a| a.iter().any(|v| v["content"] == "fail"));
        tokio::time::sleep(Duration::from_millis(if fail { 5 } else { 50 })).await;
        mock.embedding_active.fetch_sub(1, Relaxed);
    }
    if path.ends_with(":batchEmbedContents") {
        let vectors: Vec<Value> = body["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let n: f32 = r["content"]["parts"][0]["text"].as_str().unwrap().parse().unwrap();
                json!({"values":[n, 0.5]})
            })
            .collect();
        Json(json!({"embeddings":vectors})).into_response()
    } else {
        let inputs = body["instances"].as_array().unwrap();
        let limit = if path.contains("gemini-embedding") { 1 } else { 5 };
        if inputs.len() > limit {
            return (StatusCode::BAD_REQUEST, Json(json!({"error":{"message":"too many inputs"}}))).into_response();
        }
        if inputs.iter().any(|v| v["content"] == "fail") {
            return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":{"message":"capacity exceeded"}})))
                .into_response();
        }
        let predictions: Vec<Value> = inputs
            .iter()
            .map(|v| {
                let n: f32 = v["content"].as_str().unwrap().parse().unwrap();
                json!({"embeddings":{"values":[n,0.5],"statistics":{"token_count":3,"truncated":false}}})
            })
            .collect();
        Json(json!({"predictions":predictions})).into_response()
    }
}

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(router: axum::Router) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
    });
    Server { url, task }
}

struct Setup {
    app: Arc<App>,
    proxy: Server,
    mock: Arc<Mock>,
    _provider: Server,
    dir: std::path::PathBuf,
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Two OpenAI keys, `a` and `b`, both answering from the mock; `edit` adjusts the config.
async fn setup(edit: impl FnOnce(&mut Config, &str)) -> Setup {
    let mock = Arc::new(Mock::default());
    let provider = serve(
        axum::Router::new()
            .route("/v1/responses", post(responses))
            .route("/v1/embeddings", post(embeddings))
            .route("/v1beta/models/{*path}", post(google_embeddings))
            .route("/v1/publishers/google/models/{*path}", post(google_embeddings))
            .route("/hook", post(hook))
            .with_state(mock.clone()),
    )
    .await;
    let dir = std::env::temp_dir().join(format!("fusebox-features-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = Config {
        auth_dir: dir.to_string_lossy().into(),
        api_keys: vec!["client".into()],
        codex_api_key: ["a", "b"]
            .map(|k| KeyEntry {
                api_key: k.into(),
                base_url: Some(format!("{}/v1", provider.url)),
                label: Some(k.into()),
                ..Default::default()
            })
            .to_vec(),
        ..Default::default()
    };
    edit(&mut cfg, &provider.url);
    let app = App::new(cfg, dir.join("config.yaml"));
    let proxy = serve(crate::server::router(app.clone())).await;
    Setup { app, proxy, mock, _provider: provider, dir }
}

/// A streamed Responses request; the answer's text, or the error status.
async fn ask(s: &Setup, key: &str, model: &str, headers: &[(&str, &str)]) -> (u16, String) {
    let mut rb = reqwest::Client::new().post(format!("{}/v1/responses", s.proxy.url)).bearer_auth(key);
    for (k, v) in headers {
        rb = rb.header(*k, *v);
    }
    let body = json!({"model": model, "stream": true, "input": [{"role":"user","content":format!("hello {}", uuid::Uuid::new_v4())}]});
    let resp = rb.json(&body).send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    let answer: String = text
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter_map(|d| serde_json::from_str::<Value>(d).ok())
        .filter(|v| v["type"] == "response.output_text.delta")
        .filter_map(|v| v["delta"].as_str().map(String::from))
        .collect();
    (status, if answer.is_empty() { text } else { answer })
}

#[tokio::test]
async fn higher_priority_accounts_answer_first() {
    let s = setup(|cfg, _| cfg.codex_api_key[1].priority = 5).await;
    for _ in 0..3 {
        assert_eq!(ask(&s, "client", "gpt-up", &[]).await, (200, "b:gpt-up".into()));
    }
    // While it sits out, the lower priority takes over.
    let b = s.app.pool.all().into_iter().find(|a| a.label == "b").unwrap();
    b.cool(None, chrono::Utc::now() + chrono::Duration::minutes(5), "429");
    assert_eq!(ask(&s, "client", "gpt-up", &[]).await, (200, "a:gpt-up".into()));
}

#[tokio::test]
async fn model_groups_move_on_and_background_calls_go_elsewhere() {
    let s = setup(|cfg, _| {
        cfg.model_groups = BTreeMap::from([(
            "smart".to_string(),
            ModelGroup { models: vec!["gpt-gone".into(), "gpt-up".into()], balance: false },
        )]);
        cfg.background_model = Some("gpt-cheap".into());
    })
    .await;
    for account in ["a", "b"] {
        s.mock.acts.lock().insert((account.into(), "gpt-gone".into()), Act::Missing);
    }
    let (status, answer) = ask(&s, "client", "smart", &[]).await;
    assert_eq!(status, 200);
    assert!(answer.ends_with(":gpt-up"), "{answer}");
    // The group is listed for clients.
    let models: Value = reqwest::Client::new()
        .get(format!("{}/v1/models", s.proxy.url))
        .bearer_auth("client")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(models["data"].as_array().unwrap().iter().any(|m| m["id"] == "smart" && m["owned_by"] == "fusebox"));
    // Claude Code's Haiku calls go to the background model; other clients' don't.
    let cli = [("user-agent", "claude-cli/2.1.296 (external, cli)")];
    let (_, answer) = ask(&s, "client", "claude-haiku-4-5", &cli).await;
    assert!(answer.ends_with(":gpt-cheap"), "{answer}");
    let (status, _) = ask(&s, "client", "claude-haiku-4-5", &[]).await;
    assert_eq!(status, 404, "no account serves Haiku itself");
}

#[test]
fn balanced_groups_keep_a_session_on_one_model() {
    let group = ModelGroup { models: vec!["x".into(), "y".into(), "z".into()], balance: true };
    let first = crate::proxy::group_order_for_tests(&group, Some("session-1"));
    assert_eq!(first.len(), 3);
    for _ in 0..5 {
        assert_eq!(crate::proxy::group_order_for_tests(&group, Some("session-1")), first);
    }
    let starts: std::collections::HashSet<String> =
        (0..30).map(|i| crate::proxy::group_order_for_tests(&group, Some(&format!("s{i}")))[0].clone()).collect();
    assert!(starts.len() > 1, "sessions spread across the models");
    let ordered = ModelGroup { balance: false, ..group };
    assert_eq!(crate::proxy::group_order_for_tests(&ordered, Some("session-1")), ["x", "y", "z"]);
}

#[tokio::test]
async fn streams_that_fail_or_stall_before_content_move_to_the_next_account() {
    let s = setup(|cfg, _| {
        cfg.codex_api_key[0].priority = 10;
        cfg.first_token_timeout_seconds = 1;
    })
    .await;
    s.mock.acts.lock().insert(("a".into(), "gpt-busy".into()), Act::Overload);
    assert_eq!(ask(&s, "client", "gpt-busy", &[]).await, (200, "b:gpt-busy".into()));

    s.mock.acts.lock().insert(("a".into(), "gpt-slow".into()), Act::Stall);
    let started = Instant::now();
    assert_eq!(ask(&s, "client", "gpt-slow", &[]).await, (200, "b:gpt-slow".into()));
    assert!(started.elapsed() < Duration::from_secs(10), "took {:?}", started.elapsed());
    // The stalled account sits out that model for a while.
    let a = s.app.pool.all().into_iter().find(|x| x.label == "a").unwrap();
    assert!(a.cooling_until("gpt-slow").is_some());
    assert!(a.cooling_until("gpt-up").is_none());
}

#[tokio::test]
async fn named_client_limits_are_enforced() {
    let s = setup(|cfg, _| {
        cfg.api_keys.clear();
        let client =
            |id: &str| NamedClient { id: id.into(), label: id.into(), key: format!("fbx_{id}"), ..Default::default() };
        cfg.named_clients = vec![
            NamedClient { requests_per_minute: Some(1), ..client("rate") },
            NamedClient { models: vec!["gpt-up".into()], ..client("models") },
            NamedClient { expires: chrono::Local::now().date_naive().pred_opt(), ..client("old") },
            NamedClient { user_agents: vec!["codex*".into()], ..client("codex") },
        ];
    })
    .await;
    assert_eq!(ask(&s, "fbx_rate", "gpt-up", &[]).await.0, 200);
    let resp = reqwest::Client::new()
        .post(format!("{}/v1/responses", s.proxy.url))
        .bearer_auth("fbx_rate")
        .json(&json!({"model":"gpt-up","input":"hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 429);
    assert!(resp.headers().contains_key("retry-after"));

    assert_eq!(ask(&s, "fbx_models", "gpt-up", &[]).await.0, 200);
    let (status, body) = ask(&s, "fbx_models", "gpt-other", &[]).await;
    assert_eq!(status, 403);
    assert!(body.contains("can't use gpt-other"), "{body}");

    assert_eq!(ask(&s, "fbx_old", "gpt-up", &[]).await.0, 401);
    assert_eq!(ask(&s, "fbx_codex", "gpt-up", &[("user-agent", "curl/8")]).await.0, 403);
    assert_eq!(ask(&s, "fbx_codex", "gpt-up", &[("user-agent", "codex_cli_rs/0.162.1")]).await.0, 200);
    // Their usage today is on the management API.
    let usage: Value = reqwest::get(format!("{}/api/clients/usage", s.proxy.url)).await.unwrap().json().await.unwrap();
    assert!(usage["clients"]["rate"]["tokens_today"].as_u64().unwrap() >= 12);
}

#[tokio::test]
async fn client_keys_report_when_they_were_last_used() {
    let s = setup(|cfg, _| {
        cfg.api_keys = vec!["fbx_shared_one".into(), "fbx_shared_two".into()];
        let client =
            |id: &str| NamedClient { id: id.into(), label: id.into(), key: format!("fbx_{id}"), ..Default::default() };
        cfg.named_clients = vec![client("laptop"), client("idle")];
    })
    .await;
    let usage = || async {
        reqwest::get(format!("{}/api/clients/usage", s.proxy.url)).await.unwrap().json::<Value>().await.unwrap()
    };
    let before = usage().await;
    assert_eq!(before["last_used"], json!({}));
    assert_eq!(before["shared_last_used"], json!([null, null]));

    assert_eq!(ask(&s, "fbx_laptop", "gpt-up", &[]).await.0, 200);
    assert_eq!(ask(&s, "fbx_shared_two", "gpt-up", &[]).await.0, 200);
    assert_eq!(ask(&s, "fbx_wrong", "gpt-up", &[]).await.0, 401);
    let after = usage().await;
    assert!(after["last_used"]["laptop"].is_string(), "{after}");
    assert!(after["last_used"].get("idle").is_none(), "{after}");
    assert!(after["shared_last_used"][0].is_null(), "{after}");
    assert!(after["shared_last_used"][1].is_string(), "{after}");

    // Named from the shared key, the client was last used when the key was.
    let mut cfg = (*s.app.cfg()).clone();
    let key = cfg.api_keys.pop().unwrap();
    cfg.named_clients.push(NamedClient { id: "macbook".into(), label: "MacBook".into(), key, ..Default::default() });
    s.app.set_config(cfg);
    let named = usage().await;
    assert_eq!(named["last_used"]["macbook"], after["shared_last_used"][1], "{named}");
    assert_eq!(named["shared_last_used"], json!([null]));
}

#[tokio::test]
async fn upgrade_headers_cannot_bypass_http_rate_limits() {
    let s = setup(|cfg, _| {
        cfg.api_keys.clear();
        cfg.named_clients = vec![NamedClient {
            id: "rate".into(),
            label: "rate".into(),
            key: "fbx_rate".into(),
            requests_per_minute: Some(1),
            ..Default::default()
        }];
    })
    .await;
    let headers = [
        ("upgrade", "websocket"),
        ("connection", "upgrade"),
        ("sec-websocket-version", "13"),
        ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
    ];
    assert_eq!(ask(&s, "fbx_rate", "gpt-up", &headers).await.0, 200);
    assert_eq!(ask(&s, "fbx_rate", "gpt-up", &headers).await.0, 429);
    assert_eq!(s.mock.calls.lock().len(), 1);
}

#[tokio::test]
async fn websocket_handshakes_are_free_and_turns_are_counted() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    let s = setup(|cfg, _| {
        cfg.api_keys.clear();
        cfg.named_clients = vec![NamedClient {
            id: "socket".into(),
            label: "socket".into(),
            key: "fbx_socket".into(),
            requests_per_minute: Some(1),
            ..Default::default()
        }];
    })
    .await;
    for path in ["/v1/responses", "/backend-api/codex/responses"] {
        let mut request = format!("{}{path}", s.proxy.url.replace("http://", "ws://")).into_client_request().unwrap();
        request.headers_mut().insert("authorization", "Bearer fbx_socket".parse().unwrap());
        let (mut socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
        assert_eq!(response.status(), 101);
        socket
            .send(Message::Text(json!({"type":"response.create","model":"gpt-up","input":"hi"}).to_string().into()))
            .await
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event: Value =
                    serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
                if event["type"] == "response.completed" || event["type"] == "error" {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        if path == "/v1/responses" {
            assert_eq!(event["type"], "response.completed");
        } else {
            assert_eq!(event["type"], "error");
            assert_eq!(event["status"], 429);
        }
        socket.close(None).await.unwrap();
    }
    assert_eq!(s.mock.calls.lock().len(), 1);
}

type TestSocket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn websocket_turn(socket: &mut TestSocket) -> Value {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    socket
        .send(Message::Text(json!({"type":"response.create","model":"gpt-up","input":"hi"}).to_string().into()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            if let Message::Text(text) = message {
                let event: Value = serde_json::from_str(&text).unwrap();
                if event["type"] == "response.completed" || event["type"] == "error" {
                    break event;
                }
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn websocket_turns_revalidate_named_keys_after_rotation_or_removal() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    for (path, query) in [("/v1/responses", false), ("/backend-api/codex/responses", true)] {
        for remove in [false, true] {
            let s = setup(|cfg, _| {
                cfg.codex_websockets = false;
                cfg.named_clients = vec![NamedClient {
                    id: "laptop".into(),
                    label: "Laptop".into(),
                    key: "fbx_old".into(),
                    ..Default::default()
                }];
            })
            .await;
            let url =
                format!("{}{path}{}", s.proxy.url.replace("http://", "ws://"), if query { "?key=fbx_old" } else { "" });
            let mut request = url.into_client_request().unwrap();
            if !query {
                request.headers_mut().insert("authorization", "Bearer fbx_old".parse().unwrap());
            }
            let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
            assert_eq!(websocket_turn(&mut socket).await["type"], "response.completed");
            // A rename keeps the same identity and credential and must not revoke access.
            let mut cfg = (*s.app.cfg()).clone();
            cfg.named_clients[0].label = "Renamed laptop".into();
            s.app.set_config(cfg);
            assert_eq!(websocket_turn(&mut socket).await["type"], "response.completed");
            let mut cfg = (*s.app.cfg()).clone();
            if remove {
                cfg.named_clients.clear();
            } else {
                cfg.named_clients[0].key = "fbx_replacement".into();
            }
            s.app.set_config(cfg);
            assert_eq!(ask(&s, "fbx_old", "gpt-up", &[]).await.0, 401);
            let event = websocket_turn(&mut socket).await;
            assert_eq!(event["type"], "error", "{event}");
            assert_eq!(event["status"], 401, "{event}");
            assert_eq!(s.mock.calls.lock().len(), 2, "revoked turns must not reach a provider");
            if !remove {
                let mut request =
                    format!("{}{path}", s.proxy.url.replace("http://", "ws://")).into_client_request().unwrap();
                request.headers_mut().insert("x-api-key", "fbx_replacement".parse().unwrap());
                let (mut replacement, _) = tokio_tungstenite::connect_async(request).await.unwrap();
                assert_eq!(websocket_turn(&mut replacement).await["type"], "response.completed");
            }
        }
    }
}

#[tokio::test]
async fn shared_and_anonymous_websockets_revalidate_their_access() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    for anonymous in [false, true] {
        let s = setup(|cfg, _| {
            cfg.codex_websockets = false;
            if anonymous {
                cfg.api_keys.clear();
            }
        })
        .await;
        let mut request =
            format!("{}/v1/responses", s.proxy.url.replace("http://", "ws://")).into_client_request().unwrap();
        if !anonymous {
            request.headers_mut().insert("x-api-key", "client".parse().unwrap());
        }
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        assert_eq!(websocket_turn(&mut socket).await["type"], "response.completed");
        let mut cfg = (*s.app.cfg()).clone();
        cfg.api_keys = vec!["replacement".into()];
        s.app.set_config(cfg);
        let event = websocket_turn(&mut socket).await;
        assert_eq!(event["type"], "error", "{event}");
        assert_eq!(event["status"], 401, "{event}");
        assert_eq!(s.mock.calls.lock().len(), 1);
    }
}

#[tokio::test]
async fn unavailable_accounting_blocks_budget_keys() {
    for enabled in [false, true] {
        let s = setup(|cfg, _| {
            cfg.usage.enabled = enabled;
            std::fs::write(format!("{}/not-a-directory", cfg.auth_dir), "test").unwrap();
            cfg.usage.database = Some(format!("{}/not-a-directory/usage.sqlite3", cfg.auth_dir));
            cfg.named_clients = vec![NamedClient {
                id: "budget".into(),
                label: "budget".into(),
                key: "fbx_budget".into(),
                usd_per_month: Some(1.0),
                ..Default::default()
            }];
        })
        .await;
        crate::policy::refresh(&s.app).await;
        assert_eq!(ask(&s, "fbx_budget", "gpt-6-astra", &[]).await.0, 503);
        assert!(s.mock.calls.lock().is_empty());
        assert_eq!(ask(&s, "client", "gpt-6-astra", &[]).await.0, 200);
    }
}

#[tokio::test]
async fn completed_spend_is_enforced_before_refresh_and_survives_reconciliation() {
    let s = setup(|cfg, _| {
        cfg.usage.database = Some(format!("{}/usage.sqlite3", cfg.auth_dir));
        cfg.named_clients = vec![NamedClient {
            id: "budget".into(),
            label: "budget".into(),
            key: "fbx_budget".into(),
            usd_per_month: Some(0.000001),
            ..Default::default()
        }];
    })
    .await;
    assert_eq!(ask(&s, "fbx_budget", "gpt-6-astra", &[]).await.0, 503); // Startup cannot assume zero.
    crate::policy::refresh(&s.app).await;
    assert_eq!(ask(&s, "fbx_budget", "gpt-6-astra", &[]).await.0, 200);
    let spend = s.app.clients.spend("budget");
    assert_eq!(spend.tokens_today, 12);
    assert!(spend.usd_month > 0.000001);
    assert_eq!(ask(&s, "fbx_budget", "gpt-6-astra", &[]).await.0, 429);
    let store = s.app.usage.as_ref().unwrap();
    store.flush().await.unwrap();
    for _ in 0..3 {
        crate::policy::refresh(&s.app).await;
        assert_eq!(s.app.clients.spend("budget"), spend);
    }
    let nanos: i64 = store
        .call(|conn| {
            Ok(conn.query_row("SELECT SUM(cost_nanos) FROM usage_observations WHERE client_id='budget'", [], |r| {
                r.get(0)
            })?)
        })
        .await
        .unwrap();
    assert_eq!(spend.usd_month, nanos as f64 / 1e9);
    store.shutdown().await.unwrap();
    assert_eq!(ask(&s, "fbx_budget", "gpt-6-astra", &[]).await.0, 503);
}

#[tokio::test]
async fn failed_refresh_and_unpriced_usage_do_not_reset_a_budget_to_zero() {
    let s = setup(|cfg, _| {
        cfg.usage.database = Some(format!("{}/usage.sqlite3", cfg.auth_dir));
        cfg.named_clients = vec![NamedClient {
            id: "budget".into(),
            label: "budget".into(),
            key: "fbx_budget".into(),
            usd_per_month: Some(1.0),
            ..Default::default()
        }];
    })
    .await;
    crate::policy::refresh(&s.app).await;
    assert_eq!(ask(&s, "fbx_budget", "gpt-unknown-rate", &[]).await.0, 200);
    assert_eq!(ask(&s, "fbx_budget", "gpt-unknown-rate", &[]).await.0, 503);
    crate::policy::refresh(&s.app).await;
    assert_eq!(ask(&s, "fbx_budget", "gpt-unknown-rate", &[]).await.0, 503);
    let store = s.app.usage.as_ref().unwrap();
    store
        .call(|conn| {
            conn.execute_batch("ALTER TABLE usage_observations RENAME TO missing_observations")?;
            Ok(())
        })
        .await
        .unwrap();
    crate::policy::refresh(&s.app).await;
    assert_eq!(ask(&s, "fbx_budget", "gpt-6-astra", &[]).await.0, 503);
    store
        .call(|conn| {
            conn.execute_batch("ALTER TABLE missing_observations RENAME TO usage_observations")?;
            Ok(())
        })
        .await
        .unwrap();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn model_groups_preserve_gateway_timeouts_and_share_attempts() {
    let s = setup(|cfg, _| {
        cfg.request_retry = 2;
        cfg.model_groups = BTreeMap::from([(
            "group".into(),
            ModelGroup { models: vec!["gpt-one".into(), "gpt-two".into(), "gpt-three".into()], balance: false },
        )]);
    })
    .await;
    for account in ["a", "b"] {
        s.mock.acts.lock().insert((account.into(), "gpt-one".into()), Act::GatewayTimeout);
    }
    assert_eq!(ask(&s, "client", "group", &[]).await.0, 504);
    assert_eq!(s.mock.calls.lock().len(), 1);
    s.mock.calls.lock().clear();
    for account in ["a", "b"] {
        for model in ["gpt-one", "gpt-two", "gpt-three"] {
            s.mock.acts.lock().insert((account.into(), model.into()), Act::Missing);
        }
    }
    assert_eq!(ask(&s, "client", "group", &[]).await.0, 404);
    let calls = s.mock.calls.lock();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|(_, model)| model != "gpt-three"));
}

#[tokio::test]
async fn google_embedding_contracts_preserve_encoding_batch_limits_and_order() {
    use base64::Engine;
    for (vertex, model, count, chunks) in [
        (false, "gemini-embedding-001", 3, vec![3]),
        (true, "gemini-embedding-001", 3, vec![1, 1, 1]),
        (true, "text-embedding-005", 7, vec![5, 2]),
    ] {
        let s = setup(|cfg, url| {
            cfg.codex_api_key.clear();
            cfg.usage.database = Some(format!("{}/usage.sqlite3", cfg.auth_dir));
            let entry = KeyEntry { api_key: "google".into(), base_url: Some(url.into()), ..Default::default() };
            if vertex {
                cfg.vertex_api_key = vec![entry];
            } else {
                cfg.gemini_api_key = vec![entry];
            }
        })
        .await;
        let inputs: Vec<String> = (0..count).map(|n| n.to_string()).collect();
        let resp = reqwest::Client::new()
            .post(format!("{}/v1/embeddings", s.proxy.url))
            .bearer_auth("client")
            .json(&json!({"model":model,"input":inputs,"encoding_format":"base64","dimensions":2}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["data"].as_array().unwrap().len(), count);
        for i in 0..count {
            assert_eq!(body["data"][i]["index"], i);
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(body["data"][i]["embedding"].as_str().unwrap())
                .unwrap();
            assert_eq!(bytes, [i as f32, 0.5].into_iter().flat_map(f32::to_le_bytes).collect::<Vec<_>>());
        }
        {
            let calls = s.mock.embedding_calls.lock();
            let sizes: Vec<usize> = calls
                .iter()
                .map(|(_, v)| v[if vertex { "instances" } else { "requests" }].as_array().unwrap().len())
                .collect();
            assert_eq!(sizes, chunks);
            if vertex {
                assert_eq!(body["usage"]["total_tokens"], count * 3);
                assert!(
                    calls.iter().all(|(_, v)| v["parameters"]["autoTruncate"] == false
                        && v["parameters"]["outputDimensionality"] == 2)
                );
            } else {
                assert!(calls[0].1["requests"].as_array().unwrap().iter().all(|v| v["outputDimensionality"] == 2));
            }
        }
        let store = s.app.usage.as_ref().unwrap();
        store.flush().await.unwrap();
        assert_eq!(store.health()["state"], "healthy");
        let (rows, input, models, estimates): (i64, i64, i64, i64) = store.call(move |conn| {
            Ok(conn.query_row("SELECT COUNT(*),SUM(input),SUM(model=?1),SUM(json_extract(payload,'$.numeric_metadata.estimated_input_tokens') IS NOT NULL) FROM usage_observations", [model], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?)
        }).await.unwrap();
        assert_eq!(rows as usize, chunks.len());
        assert_eq!(models, rows);
        assert_eq!(input as usize, count * if vertex { 3 } else { 1 });
        assert_eq!(estimates, if vertex { 0 } else { rows });
        store.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn failed_vertex_chunks_preserve_completed_usage_without_replay() {
    let s = setup(|cfg, url| {
        cfg.codex_api_key.clear();
        cfg.usage.database = Some(format!("{}/usage.sqlite3", cfg.auth_dir));
        cfg.vertex_api_key = ["a", "b"]
            .map(|k| KeyEntry { api_key: k.into(), base_url: Some(url.into()), ..Default::default() })
            .to_vec();
    })
    .await;
    let resp = reqwest::Client::new()
        .post(format!("{}/v1/embeddings", s.proxy.url))
        .bearer_auth("client")
        .json(&json!({"model":"gemini-embedding-001","input":["1","fail"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 503);
    assert_eq!(s.mock.embedding_calls.lock().len(), 2);
    let store = s.app.usage.as_ref().unwrap();
    store.flush().await.unwrap();
    let (rows, tokens, models): (i64, i64, i64) = store
        .call(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*),SUM(input),SUM(model='gemini-embedding-001') FROM usage_observations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!((rows, tokens, models), (2, 3, 2));
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn web_pages_cannot_use_what_trusts_this_machine() {
    let s = setup(|cfg, _| cfg.api_keys.clear()).await;
    let client = reqwest::Client::new();
    let post = |origin: Option<&str>, host: Option<&str>| {
        let mut rb = client.post(format!("{}/v1/responses", s.proxy.url)).json(&json!({"model":"gpt-up","input":"hi"}));
        if let Some(o) = origin {
            rb = rb.header("origin", o);
        }
        if let Some(h) = host {
            rb = rb.header("host", h);
        }
        rb.send()
    };
    assert_eq!(post(Some("https://evil.example"), None).await.unwrap().status(), 403);
    assert_eq!(post(None, Some("evil.example:8317")).await.unwrap().status(), 403);
    assert_eq!(post(Some("http://localhost:3000"), None).await.unwrap().status(), 200);
    assert_eq!(post(None, None).await.unwrap().status(), 200);
    // The dashboard API without a key is guarded the same way.
    let get = |origin: Option<&str>, host: Option<&str>| {
        let mut rb = client.get(format!("{}/api/overview", s.proxy.url));
        if let Some(o) = origin {
            rb = rb.header("origin", o);
        }
        if let Some(h) = host {
            rb = rb.header("host", h);
        }
        rb.send()
    };
    assert_eq!(get(Some("https://evil.example"), None).await.unwrap().status(), 403);
    assert_eq!(get(None, Some("rebound.example")).await.unwrap().status(), 403);
    assert_eq!(get(None, None).await.unwrap().status(), 200);
}

#[tokio::test]
async fn wrong_management_keys_lock_the_address_out() {
    let s = setup(|cfg, _| cfg.management_key = "right-key".into()).await;
    let client = reqwest::Client::new();
    let with = |k: &str| client.get(format!("{}/api/overview", s.proxy.url)).bearer_auth(k).send();
    assert_eq!(with("right-key").await.unwrap().status(), 200);
    for _ in 0..5 {
        assert_eq!(with("wrong").await.unwrap().status(), 401);
    }
    let locked = with("right-key").await.unwrap();
    assert_eq!(locked.status(), 429);
    assert!(locked.headers().contains_key("retry-after"));
}

#[tokio::test]
async fn readiness_metrics_embeddings_webhooks_and_the_playground() {
    let s = setup(|cfg, url| {
        cfg.openai_compatibility =
            vec![CompatEntry { name: "local".into(), base_url: format!("{url}/v1"), ..Default::default() }];
    })
    .await;
    let client = reqwest::Client::new();
    let ready = client.get(format!("{}/readyz", s.proxy.url)).send().await.unwrap();
    assert_eq!(ready.status(), 200);
    assert_eq!(ready.json::<Value>().await.unwrap()["accounts"]["total"], 3);

    let metrics = client.get(format!("{}/metrics", s.proxy.url)).send().await.unwrap();
    assert_eq!(metrics.status(), 200);
    let text = metrics.text().await.unwrap();
    assert!(text.contains("fusebox_account_ready{account="), "{text}");
    assert!(text.contains("fusebox_build_info{version="));

    let emb: Value = client
        .post(format!("{}/v1/embeddings", s.proxy.url))
        .bearer_auth("client")
        .json(&json!({"model":"nomic-embed","input":"hello"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(emb["data"][0]["embedding"], json!([0.5, 0.25]));
    assert_eq!(emb["model"], "nomic-embed");

    let hook_url = format!("{}/hook", s._provider.url);
    let tested = client
        .post(format!("{}/api/push/webhooks/test", s.proxy.url))
        .json(&json!({"url": hook_url}))
        .send()
        .await
        .unwrap();
    assert_eq!(tested.status(), 200);
    let hooks = s.mock.hooks.lock().clone();
    assert_eq!(hooks.len(), 1);
    assert!(hooks[0]["text"].as_str().unwrap().contains("notifications are on"));

    let played = client
        .post(format!("{}/api/playground", s.proxy.url))
        .json(&json!({"model":"gpt-up","prompt":"hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(played.status(), 200);
    assert!(played.headers().contains_key("x-fusebox-account"));
    assert!(played.text().await.unwrap().contains(":gpt-up"));
}

#[tokio::test]
async fn readiness_needs_an_account() {
    let s = setup(|cfg, _| cfg.codex_api_key.clear()).await;
    let ready = reqwest::get(format!("{}/readyz", s.proxy.url)).await.unwrap();
    assert_eq!(ready.status(), 503);
    assert_eq!(ready.json::<Value>().await.unwrap()["status"], "unavailable");
}

#[test]
fn the_config_refuses_settings_that_would_misbehave() {
    let bad = |text: &str, problem: &str| {
        let e = format!("{:#}", Config::parse(text).unwrap_err());
        assert!(e.contains(problem), "{text}: {e}");
    };
    // A proxy that can't be used must never become a direct connection.
    bad("proxy-url: 127.0.0.1:1080\n", "not a URL");
    bad("proxy-url: localhost:1080\n", "socks5");
    bad("proxy-url: ftp://proxy:21\n", "socks5");
    bad("claude-api-key:\n  - api-key: k\n    proxy-url: \"socks5//x\"\n", "proxy-url");
    for good in ["", "direct", "none", "socks5://127.0.0.1:1080", "socks5h://proxy:1080", "http://u:p@proxy:3128"] {
        Config::parse(&format!("proxy-url: \"{good}\"\n")).unwrap();
    }
    bad("notifications:\n  webhooks: [\"discord.com/api/webhooks/1/x\"]\n", "not a URL");
    bad("model-groups:\n  smart: []\n", "lists no models");
    bad("model-groups:\n  smart: [\"\"]\n", "empty model");
    bad("named-clients:\n  - id: t\n    label: T\n    key: k\n    requests-per-minute: 0\n", "limit of 0");
    bad("named-clients:\n  - id: t\n    label: T\n    key: k\n    usd-per-month: -1\n", "more than 0");
    bad("named-clients:\n  - id: t\n    label: T\n    key: k\n    expires: someday\n", "invalid");

    let cfg = Config::parse(
        "model-groups:\n  smart: [claude-opus-5-5, gpt-6-astra]\n  fast: { models: [a, b], balance: true }\n  \
         claude-opus-5-5: [claude-opus-5-5, gpt-6-astra]\nbackground-model: gemini-3.8-flash\n\
         first-token-timeout-seconds: 30\nlog-format: json\n\
         named-clients:\n  - id: t\n    label: T\n    key: k\n    models: [claude-*]\n    tokens-per-day: 5000\n    expires: 2030-01-31\n",
    )
    .unwrap();
    assert_eq!(
        cfg.model_groups["smart"],
        ModelGroup { models: vec!["claude-opus-5-5".into(), "gpt-6-astra".into()], balance: false }
    );
    assert!(cfg.model_groups["fast"].balance);
    assert_eq!(cfg.background_model.as_deref(), Some("gemini-3.8-flash"));
    assert_eq!(cfg.first_token_timeout_seconds, 30);
    assert_eq!(cfg.log_format, crate::config::LogFormat::Json);
    assert_eq!(cfg.named_clients[0].tokens_per_day, Some(5000));
    // Groups serialize back to the short form unless they balance.
    let yaml = serde_yaml::to_string(&cfg.model_groups).unwrap();
    assert!(yaml.contains("smart:\n- claude-opus-5-5"), "{yaml}");
    assert!(yaml.contains("balance: true"), "{yaml}");
    assert_eq!(Config::default().first_token_timeout_seconds, 120);
}

#[test]
fn cooldowns_survive_a_restart() {
    let dir = std::env::temp_dir().join(format!("fusebox-cooldowns-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = Config {
        auth_dir: dir.to_string_lossy().into(),
        claude_api_key: vec![KeyEntry { api_key: "one".into(), ..Default::default() }],
        ..Default::default()
    };
    let pool = crate::accounts::Pool::default();
    pool.reload(&cfg);
    let acct = pool.all().pop().unwrap();
    let until = chrono::Utc::now() + chrono::Duration::minutes(30);
    acct.cool(Some("claude-sonnet-5-5"), until, "429");
    acct.cool(Some("expired"), chrono::Utc::now() - chrono::Duration::minutes(1), "old");
    let path = dir.join(crate::accounts::COOLDOWNS_FILE);
    pool.cooldowns().save(&path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let restarted = crate::accounts::Pool::default();
    restarted.reload(&cfg);
    restarted.restore_cooldowns(&crate::accounts::Cooldowns::load(&path));
    let acct = restarted.all().pop().unwrap();
    assert_eq!(acct.cooling_until("claude-sonnet-5-5").map(|t| t.timestamp()), Some(until.timestamp()));
    assert!(acct.cooling_until("expired").is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn keys_that_are_not_ascii_are_masked_without_panicking() {
    let cfg = Config {
        auth_dir: "/nonexistent".into(),
        claude_api_key: vec![
            KeyEntry { api_key: "ключ-доступа-секрет".into(), ..Default::default() },
            KeyEntry { api_key: "kéy".into(), ..Default::default() },
        ],
        ..Default::default()
    };
    let pool = crate::accounts::Pool::default();
    pool.reload(&cfg);
    let labels: Vec<String> = pool.all().iter().map(|a| a.label.clone()).collect();
    assert_eq!(labels, ["ключ-д…крет", "kéy…"]);
}

#[test]
fn fusebox_headers_never_reach_a_provider() {
    let cfg = Config {
        auth_dir: "/nonexistent".into(),
        claude_api_key: vec![KeyEntry { api_key: "k".into(), ..Default::default() }],
        ..Default::default()
    };
    let pool = crate::accounts::Pool::default();
    pool.reload(&cfg);
    let acct = pool.all().pop().unwrap();
    let mut headers = HeaderMap::new();
    for (k, v) in [
        ("user-agent", "claude-cli/2.1.296 (external, cli)"),
        ("x-fusebox-client-scope", "digest"),
        ("x-cliproxy-client-scope", "digest"),
        ("x-fusebox-session-id", "task"),
        ("x-cliproxy-session-end", "true"),
        ("x-claude-code-session-id", "kept"),
    ] {
        headers.insert(k, v.parse().unwrap());
    }
    let t = crate::upstream::Target {
        acct: &acct,
        cfg: &cfg,
        client_headers: &headers,
        model: "claude-sonnet-5-5",
        wire: crate::ir::Format::Claude,
        passthrough: true,
        stream: true,
        count_tokens: false,
        cache_key: None,
        cache_session: None,
        cache_defaults: true,
    };
    let p = crate::upstream::prepare(&t, json!({"model":"claude-sonnet-5-5","messages":[]}));
    let names: Vec<&str> = p.headers.iter().map(|(k, _)| k.as_str()).collect();
    assert!(names.contains(&"x-claude-code-session-id"), "{names:?}");
    assert!(!names.iter().any(|n| n.starts_with("x-fusebox-") || n.starts_with("x-cliproxy-")), "{names:?}");
}

#[test]
fn private_files_stay_private_even_over_an_old_temporary_file() {
    let dir = std::env::temp_dir().join(format!("fusebox-private-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("claude.json");
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, "left over").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    crate::files::write_private_via(&path, &temp, b"{\"type\":\"claude\"}").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"type\":\"claude\"}");
    assert!(!temp.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn only_this_machines_names_are_local() {
    for host in ["localhost", "localhost:8317", "app.localhost", "127.0.0.1:8317", "127.1.2.3", "[::1]:8317", "[::1]"] {
        assert!(crate::mgmt::loopback_host(host), "{host}");
    }
    for host in ["evil.example", "evil.example:8317", "10.0.0.2:8317", "localhost.evil.example", "[2001:db8::1]:80"] {
        assert!(!crate::mgmt::loopback_host(host), "{host}");
    }
}

#[tokio::test]
async fn the_usage_database_can_be_backed_up_while_open() {
    let dir = std::env::temp_dir().join(format!("fusebox-backup-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("usage.sqlite3");
    let store = crate::usage::store::Store::open(&db, 64, 90, None).unwrap();
    store.flush().await.unwrap();
    let copy = dir.join("copy.sqlite3");
    let (from, to) = (db.clone(), copy.clone());
    tokio::task::spawn_blocking(move || crate::usage::store::backup(&from, &to)).await.unwrap().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let conn = rusqlite::Connection::open(&copy).unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM usage_observations", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    // Windows can't delete a file that is still open.
    drop(conn);
    // An existing file is never overwritten.
    let (from, to) = (db.clone(), copy.clone());
    assert!(tokio::task::spawn_blocking(move || crate::usage::store::backup(&from, &to)).await.unwrap().is_err());
    store.shutdown().await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_settings_editor_keeps_direct_proxies() {
    // CLIProxyAPI writes `proxy-url: direct` for "no proxy at all"; editing the entry
    // (here, its priority) must not trip over it.
    let text = "codex-api-key:\n  - api-key: \"k\"\n    proxy-url: direct\n";
    let values = crate::config_editor::values(text).unwrap();
    let mut list = values["codex-api-key"].as_array().unwrap().clone();
    list[0]["priority"] = json!(5);
    let changes = serde_json::Map::from_iter([("codex-api-key".to_string(), Value::Array(list))]);
    let (out, cfg, _) = crate::config_editor::apply(text, &changes).unwrap();
    assert!(out.contains("proxy-url: direct"), "{out}");
    assert_eq!(cfg.codex_api_key[0].priority, 5);
    // Back to normal: the field leaves the file instead of staying as `priority: 0`.
    let values = crate::config_editor::values(&out).unwrap();
    let mut list = values["codex-api-key"].as_array().unwrap().clone();
    list[0].as_object_mut().unwrap().remove("priority");
    let changes = serde_json::Map::from_iter([("codex-api-key".to_string(), Value::Array(list))]);
    let (out, cfg, _) = crate::config_editor::apply(&out, &changes).unwrap();
    assert!(!out.contains("priority"), "{out}");
    assert!(out.contains("proxy-url: direct"), "{out}");
    assert_eq!(cfg.codex_api_key[0].priority, 0);
    let bad = serde_json::Map::from_iter([("proxy-url".to_string(), json!("localhost:1080"))]);
    assert!(crate::config_editor::apply("", &bad).is_err());
}

#[test]
fn readings_past_the_limit_count_as_a_full_window() {
    let cfg = Config {
        auth_dir: "/nonexistent".into(),
        codex_api_key: vec![KeyEntry { api_key: "x".into(), ..Default::default() }],
        ..Default::default()
    };
    let pool = crate::accounts::Pool::default();
    pool.reload(&cfg);
    let acct = pool.all().pop().unwrap();
    let event = json!({"type":"codex.rate_limits","rate_limits":{"primary":{"used_percent":104.0,"window_minutes":300,"resets_in_seconds":600}}});
    crate::quota::observe_codex_event(&acct, &event, acct.quota_epoch());
    let windows = acct.state.lock().quota.windows.clone();
    assert_eq!(windows.len(), 1, "{windows:?}");
    assert_eq!(windows[0].used, 100.0);
}

#[tokio::test]
async fn an_open_proxy_behind_a_reverse_proxy_still_serves_its_own_name() {
    let s = setup(|cfg, _| cfg.api_keys.clear()).await;
    let post = |headers: &[(&str, &str)]| {
        let mut rb = reqwest::Client::new()
            .post(format!("{}/v1/responses", s.proxy.url))
            .json(&json!({"model":"gpt-up","input":"hi"}));
        for (k, v) in headers {
            rb = rb.header(*k, *v);
        }
        rb.send()
    };
    // tailscale serve: the tailnet name, forwarded. A tool there is served...
    let tailnet = [("host", "box.tail1234.ts.net"), ("x-forwarded-for", "100.64.0.7")];
    assert_eq!(post(&tailnet).await.unwrap().status(), 200);
    // ...but a page on another site, even through it, is not.
    let page = [("host", "box.tail1234.ts.net"), ("x-forwarded-for", "100.64.0.7"), ("origin", "https://evil.example")];
    assert_eq!(post(&page).await.unwrap().status(), 403);
}

// ------------------------------------------------------------ source addresses

/// `proxy-protocol-port` for a test app, as `serve` starts it.
async fn forwarded_port(app: &Arc<App>) -> (std::net::SocketAddr, Server) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let service = crate::server::router(app.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>();
    let task = tokio::spawn({
        let app = app.clone();
        async move {
            crate::serve_forwarded(app, listener, None, service).await.unwrap();
        }
    });
    (addr, Server { url: format!("http://{addr}"), task })
}

/// One HTTP/1.1 request on a fresh connection that starts with `preamble` (a PROXY
/// header, or nothing); the status (0 when the connection was closed) and the response.
async fn raw(
    addr: std::net::SocketAddr,
    preamble: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&Value>,
) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = body.map(Value::to_string).unwrap_or_default();
    let mut req = format!(
        "{preamble}{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if !body.is_empty() {
        req.push_str("Content-Type: application/json\r\n");
    }
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(&body);
    let _ = stream.write_all(req.as_bytes()).await;
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).to_string();
    (text.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0), text)
}

fn proxy_v1(client: &str) -> String {
    format!("PROXY TCP4 {client} 10.10.0.235 50000 8317\r\n")
}

fn last_request(s: &Setup) -> crate::state::RequestLog {
    s.app.stats.recent.lock().iter().last().cloned().unwrap()
}

async fn hook_containing(s: &Setup, text: &str) -> Option<Value> {
    for _ in 0..50 {
        if let Some(h) = s.mock.hooks.lock().iter().find(|h| h["text"].as_str().is_some_and(|t| t.contains(text))) {
            return Some(h.clone());
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    None
}

#[tokio::test]
async fn forwarded_connections_carry_the_client_address() {
    let s = setup(|cfg, url| {
        cfg.api_keys.clear();
        let client =
            |id: &str| NamedClient { id: id.into(), label: id.into(), key: format!("fbx_{id}"), ..Default::default() };
        cfg.named_clients =
            vec![client("beast"), NamedClient { addresses: vec!["10.10.0.0/24".into()], ..client("locked") }];
        cfg.notifications.webhooks = vec![format!("{url}/hook")];
    })
    .await;
    let (gate, _gate) = forwarded_port(&s.app).await;
    let body = json!({"model": "gpt-up", "stream": true, "input": [{"role":"user","content":"hi"}]});
    let beast = [("authorization", "Bearer fbx_beast")];
    let source = |log: &crate::state::RequestLog| log.source.clone().map(|x| (x.ip.to_string(), x.via, x.login));
    use crate::source::Via;

    // A LAN client through the gate is seen as itself, with its key.
    let (status, text) = raw(gate, &proxy_v1("10.10.0.7"), "POST", "/v1/responses", &beast, Some(&body)).await;
    assert_eq!(status, 200, "{text}");
    let log = last_request(&s);
    assert_eq!(source(&log), Some(("10.10.0.7".into(), Via::ProxyProtocol, None)));
    assert_eq!((log.key.as_deref(), log.key_label.as_deref()), (Some("beast"), Some("beast")));
    // Its own forwarding header is not believed.
    let spoof = [("authorization", "Bearer fbx_beast"), ("x-forwarded-for", "127.0.0.1")];
    raw(gate, &proxy_v1("10.10.0.7"), "POST", "/v1/responses", &spoof, Some(&body)).await;
    assert_eq!(source(&last_request(&s)).unwrap().0, "10.10.0.7");
    // tailscale serve through the gate: the PROXY header says loopback, tailscale says who.
    let tailnet = [
        ("authorization", "Bearer fbx_beast"),
        ("x-forwarded-for", "100.101.7.12"),
        ("tailscale-user-login", "someone@github"),
    ];
    let (status, _) = raw(gate, &proxy_v1("127.0.0.1"), "POST", "/v1/responses", &tailnet, Some(&body)).await;
    assert_eq!(status, 200);
    assert_eq!(source(&last_request(&s)), Some(("100.101.7.12".into(), Via::Forwarded, Some("someone@github".into()))));
    // The first address was quiet; the next one is news.
    let alert = hook_containing(&s, "beast used from a new address").await.expect("a new-address notification");
    assert!(alert["text"].as_str().unwrap().contains("100.101.7.12 (Tailscale user someone@github)"), "{alert}");
    assert_eq!(alert["event"], "client_new_address");
    assert!(hook_containing(&s, "10.10.0.7 for the first time").await.is_none());
    // A direct connection is itself too.
    let (status, _) = ask(&s, "fbx_beast", "gpt-up", &[]).await;
    assert_eq!(status, 200);
    assert_eq!(source(&last_request(&s)), Some(("127.0.0.1".into(), Via::Direct, None)));

    // A key with an address list only works from there, on every path.
    let locked = [("authorization", "Bearer fbx_locked")];
    let (status, _) = raw(gate, &proxy_v1("10.10.0.9"), "POST", "/v1/responses", &locked, Some(&body)).await;
    assert_eq!(status, 200);
    let (status, text) = raw(gate, &proxy_v1("192.168.1.9"), "POST", "/v1/responses", &locked, Some(&body)).await;
    assert_eq!(status, 403);
    assert!(text.contains("this key doesn't work from 192.168.1.9"), "{text}");
    assert!(hook_containing(&s, "locked's key refused").await.is_some());
    assert_eq!(ask(&s, "fbx_locked", "gpt-up", &[]).await.0, 403, "loopback isn't on its list");
    let forged = [("authorization", "Bearer fbx_locked"), ("x-forwarded-for", "10.10.0.9")];
    let (status, _) = raw(gate, &proxy_v1("192.168.1.9"), "POST", "/v1/responses", &forged, Some(&body)).await;
    assert_eq!(status, 403, "a LAN client can't claim a listed address");

    // Without a management key the dashboard is this machine's alone: a LAN client
    // through the gate no longer counts as this machine.
    assert_eq!(raw(gate, &proxy_v1("10.10.0.7"), "GET", "/api/overview", &[], None).await.0, 403);
    assert_eq!(raw(gate, &proxy_v1("127.0.0.1"), "GET", "/api/overview", &[], None).await.0, 200);

    // The Clients page: each named key's addresses, and what is connected now.
    let usage: Value = reqwest::get(format!("{}/api/clients/usage", s.proxy.url)).await.unwrap().json().await.unwrap();
    let ips: Vec<&str> =
        usage["addresses"]["beast"].as_array().unwrap().iter().map(|a| a["ip"].as_str().unwrap()).collect();
    assert_eq!(ips.len(), 3, "{usage}");
    assert!(ips.contains(&"10.10.0.7") && ips.contains(&"100.101.7.12") && ips.contains(&"127.0.0.1"));
    let lan = usage["addresses"]["beast"].as_array().unwrap().iter().find(|a| a["ip"] == "10.10.0.7").unwrap();
    assert_eq!((lan["requests"].as_u64(), lan["via"].as_str()), (Some(2), Some("proxy_protocol")));
    assert_eq!(usage["addresses"]["locked"].as_array().unwrap().len(), 1, "refused requests aren't addresses it uses");
    let connected = usage["connected"].as_array().unwrap();
    assert!(
        connected.iter().any(|c| c["label"] == "beast" && c["ip"] == "10.10.0.7" && c["in_flight"] == 0),
        "{usage}"
    );

    // Pinned sessions say where they come from.
    let account = s.app.pool.all().into_iter().next().unwrap();
    let activity: Value = reqwest::get(format!("{}/api/accounts/{}/activity", s.proxy.url, account.id))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        activity["sessions"].as_array().unwrap().iter().all(|x| x["source"]["ip"].is_string() || x["requests"] == 0),
        "{activity}"
    );
}

#[tokio::test]
async fn with_a_forwarder_port_only_that_port_believes_forwarding_headers() {
    // A forwarder that falls back to the ordinary port connects from this machine; its
    // clients' own X-Forwarded-For must not pass for a trusted proxy's there.
    let s = setup(|cfg, _| {
        cfg.proxy_protocol_port = 1;
        cfg.api_keys.clear();
        cfg.named_clients = vec![NamedClient {
            id: "locked".into(),
            label: "locked".into(),
            key: "fbx_locked".into(),
            addresses: vec!["10.10.0.0/24".into()],
            ..Default::default()
        }];
    })
    .await;
    let (gate, _gate) = forwarded_port(&s.app).await;
    let forged = [("x-forwarded-for", "10.10.0.9")];
    assert_eq!(ask(&s, "fbx_locked", "gpt-up", &forged).await.0, 403, "seen as this machine, not 10.10.0.9");
    assert!(s.app.addresses.history("locked").is_empty());
    // Through the forwarder's port, tailscale serve's header still counts.
    let body = json!({"model": "gpt-up", "stream": true, "input": [{"role":"user","content":"hi"}]});
    let tailnet = [("authorization", "Bearer fbx_locked"), ("x-forwarded-for", "10.10.0.9")];
    let (status, _) = raw(gate, &proxy_v1("127.0.0.1"), "POST", "/v1/responses", &tailnet, Some(&body)).await;
    assert_eq!(status, 200);
    assert_eq!(last_request(&s).source.unwrap().ip.to_string(), "10.10.0.9");
}

#[tokio::test]
async fn websocket_turns_keep_a_keys_addresses_and_its_address_list() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let s = setup(|cfg, url| {
        cfg.codex_websockets = false;
        cfg.api_keys.clear();
        // Ids may be any text; the per-turn checks must still find the client.
        cfg.named_clients = vec![NamedClient {
            id: "café".into(),
            label: "Café".into(),
            key: "fbx_cafe".into(),
            addresses: vec!["127.0.0.1".into()],
            ..Default::default()
        }];
        cfg.notifications.webhooks = vec![format!("{url}/hook")];
    })
    .await;
    let mut request =
        format!("{}/v1/responses", s.proxy.url.replace("http://", "ws://")).into_client_request().unwrap();
    request.headers_mut().insert("authorization", "Bearer fbx_cafe".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert!(s.app.addresses.history("café").is_empty(), "a handshake isn't a request");
    for _ in 0..3 {
        assert_eq!(websocket_turn(&mut socket).await["type"], "response.completed");
    }
    let history = s.app.addresses.history("café");
    assert_eq!((history.len(), history[0].1.requests), (1, 3), "each turn counts");
    let connected = s.app.addresses.connected();
    assert!(connected.iter().any(|c| c.key == "café" && c.sockets == 1), "{connected:?}");
    // Tightening the list refuses the next turn and says so.
    let mut cfg = (*s.app.cfg()).clone();
    cfg.named_clients[0].addresses = vec!["10.10.0.0/24".into()];
    s.app.set_config(cfg);
    let event = websocket_turn(&mut socket).await;
    assert_eq!((event["type"].as_str(), event["status"].as_u64()), (Some("error"), Some(403)), "{event}");
    assert!(hook_containing(&s, "Café's key refused").await.is_some());
    drop(socket);
}

#[tokio::test]
async fn untracked_requests_and_the_playground_still_say_who_was_there() {
    let s = setup(|_, _| {}).await;
    let (gate, _gate) = forwarded_port(&s.app).await;
    let key = [("authorization", "Bearer client")];
    assert_eq!(raw(gate, &proxy_v1("10.10.0.7"), "GET", "/v1/models", &key, None).await.0, 200);
    let connected = s.app.addresses.connected();
    assert!(connected.iter().any(|c| c.ip.to_string() == "10.10.0.7" && c.in_flight == 0), "{connected:?}");
    // The dashboard's playground keeps the dashboard user's address.
    let body = json!({"model": "gpt-up", "prompt": "hi"});
    let (status, _) = raw(gate, &proxy_v1("127.0.0.1"), "POST", "/api/playground", &[], Some(&body)).await;
    assert_eq!(status, 200);
    let log = last_request(&s);
    assert_eq!(log.key_label.as_deref(), Some("Dashboard playground"));
    assert_eq!(
        log.source.map(|x| (x.ip.to_string(), x.via)),
        Some(("127.0.0.1".into(), crate::source::Via::ProxyProtocol))
    );
}

#[tokio::test]
async fn an_unreadable_forwarding_header_is_not_the_proxy_itself() {
    let s = setup(|cfg, _| {
        cfg.api_keys.clear();
        cfg.named_clients = vec![NamedClient {
            id: "local".into(),
            label: "local".into(),
            key: "fbx_local".into(),
            addresses: vec!["127.0.0.1".into()],
            ..Default::default()
        }];
    })
    .await;
    assert_eq!(ask(&s, "fbx_local", "gpt-up", &[]).await.0, 200, "a tool on this machine");
    let (status, text) = ask(&s, "fbx_local", "gpt-up", &[("x-forwarded-for", "unknown")]).await;
    assert_eq!(status, 403, "a client behind the proxy, spoiling its header: {text}");
    assert!(text.contains("an address the proxy in front didn't give"), "{text}");
    for (name, value) in [
        ("x-forwarded-for", "10.0.0.1,"),
        ("x-forwarded-for", "unknown, 127.0.0.1"),
        ("forwarded", "for=unknown, for=127.0.0.1"),
    ] {
        assert_eq!(ask(&s, "fbx_local", "gpt-up", &[(name, value)]).await.0, 403, "{name}: {value}");
    }
    // A fully readable local client and a direct local tool still work.
    assert_eq!(ask(&s, "fbx_local", "gpt-up", &[("x-forwarded-for", "127.0.0.1")]).await.0, 200);
    assert_eq!(ask(&s, "fbx_local", "gpt-up", &[]).await.0, 200);
}

#[tokio::test]
async fn beast_cli_requests_keep_their_address_on_the_forwarder_port() {
    let s = setup(|cfg, _| {
        cfg.proxy_protocol_port = 1;
        cfg.api_keys.clear();
        cfg.named_clients = vec![NamedClient {
            id: "the-beast".into(),
            label: "The Beast".into(),
            key: "fbx_beast".into(),
            addresses: vec!["10.10.0.7".into()],
            ..Default::default()
        }];
    })
    .await;
    let (gate, _gate) = forwarded_port(&s.app).await;
    let codex = json!({"model":"gpt-up","stream":true,"input":[{"role":"user","content":"hi"}]});
    let claude = json!({"model":"gpt-up","stream":true,"max_tokens":16,
        "messages":[{"role":"user","content":"hi"}]});
    for (path, body, agent, completed) in [
        ("/v1/responses", &codex, "codex_cli_rs/0.163.0", "response.completed"),
        ("/v1/messages", &claude, "claude-cli/2.1.304 (external, cli)", "message_stop"),
    ] {
        let headers = [
            ("authorization", "Bearer fbx_beast"),
            ("user-agent", agent),
            // A LAN client's own forwarding headers never override its PROXY address.
            ("x-forwarded-for", "unknown, 127.0.0.1"),
        ];
        let (status, text) = raw(gate, &proxy_v1("10.10.0.7"), "POST", path, &headers, Some(body)).await;
        assert_eq!(status, 200, "{path}: {text}");
        assert!(text.contains(completed), "{path}: {text}");
        let source = last_request(&s).source.unwrap();
        assert_eq!((source.ip.to_string(), source.via), ("10.10.0.7".into(), crate::source::Via::ProxyProtocol));
    }
}

#[tokio::test]
async fn proxy_protocol_port_takes_only_trusted_forwarders_with_a_header() {
    let s = setup(|_, _| {}).await;
    let (gate, _gate) = forwarded_port(&s.app).await;
    let key = [("authorization", "Bearer client")];
    assert_eq!(raw(gate, &proxy_v1("10.10.0.7"), "GET", "/v1/models", &key, None).await.0, 200);
    assert_eq!(raw(gate, "", "GET", "/v1/models", &key, None).await.0, 0, "no header, no connection");
    assert_eq!(raw(gate, "PROXY TCP4 nonsense\r\n", "GET", "/v1/models", &key, None).await.0, 0);
    // LOCAL (a health check) is the forwarder's own connection.
    assert_eq!(raw(gate, "PROXY UNKNOWN\r\n", "GET", "/v1/models", &key, None).await.0, 200);
    // Only trusted proxies may connect at all.
    let mut cfg = (*s.app.cfg()).clone();
    cfg.trusted_proxies = Some(vec!["10.99.0.1".into()]);
    s.app.set_config(cfg);
    assert_eq!(raw(gate, &proxy_v1("10.10.0.7"), "GET", "/v1/models", &key, None).await.0, 0);
}

#[tokio::test]
async fn wrong_management_keys_lock_out_only_their_own_address() {
    let s = setup(|cfg, _| cfg.management_key = "right-key".into()).await;
    let (gate, _gate) = forwarded_port(&s.app).await;
    let get = |client: &'static str, key: &'static str| {
        let bearer = format!("Bearer {key}");
        async move { raw(gate, &proxy_v1(client), "GET", "/api/overview", &[("authorization", &bearer)], None).await.0 }
    };
    for _ in 0..5 {
        assert_eq!(get("10.10.0.50", "wrong").await, 401);
    }
    assert_eq!(get("10.10.0.50", "right-key").await, 429);
    assert_eq!(get("10.10.0.51", "right-key").await, 200, "another machine through the same gate");
    let local = reqwest::Client::new().get(format!("{}/api/overview", s.proxy.url)).bearer_auth("right-key");
    assert_eq!(local.send().await.unwrap().status(), 200);
}

#[tokio::test]
async fn embedding_concurrency_keeps_output_order_and_records_each_chunk_once() {
    let s = setup(|cfg, url| {
        cfg.codex_api_key.clear();
        cfg.embeddings.concurrency = 3;
        cfg.usage.database = Some(format!("{}/usage.sqlite3", cfg.auth_dir));
        cfg.vertex_api_key =
            vec![KeyEntry { api_key: "google".into(), base_url: Some(url.into()), ..Default::default() }];
    })
    .await;
    s.mock.embedding_delay.store(true, std::sync::atomic::Ordering::Relaxed);
    let response = reqwest::Client::new()
        .post(format!("{}/v1/embeddings", s.proxy.url))
        .bearer_auth("client")
        .json(&json!({"model":"gemini-embedding-001","input":["0","1","2","3","4","5"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    for i in 0..6 {
        assert_eq!(body["data"][i]["embedding"][0].as_f64(), Some(i as f64));
    }
    assert_eq!(s.mock.embedding_peak.load(std::sync::atomic::Ordering::Relaxed), 3);
    let store = s.app.usage.as_ref().unwrap();
    store.flush().await.unwrap();
    let counts: (u64, u64, u64) = store
        .call(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*),SUM(input),SUM(json_extract(payload,'$.logical_success')=1) FROM usage_observations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(counts, (6, 18, 1));
    let log = s.app.stats.recent.lock().back().unwrap().clone();
    assert_eq!(log.attempts, 1);
    assert_eq!(log.chunks, 6);
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn concurrent_embedding_failure_drains_paid_chunks_without_replay_or_new_work() {
    let s = setup(|cfg, url| {
        cfg.codex_api_key.clear();
        cfg.embeddings.concurrency = 3;
        cfg.usage.database = Some(format!("{}/usage.sqlite3", cfg.auth_dir));
        cfg.vertex_api_key = ["a", "b"]
            .map(|k| KeyEntry { api_key: k.into(), base_url: Some(url.into()), ..Default::default() })
            .to_vec();
    })
    .await;
    s.mock.embedding_delay.store(true, std::sync::atomic::Ordering::Relaxed);
    let response = reqwest::Client::new()
        .post(format!("{}/v1/embeddings", s.proxy.url))
        .bearer_auth("client")
        .json(&json!({"model":"gemini-embedding-001","input":["0","fail","2","3","4"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(s.mock.embedding_calls.lock().len(), 3);
    let store = s.app.usage.as_ref().unwrap();
    store.flush().await.unwrap();
    let counts: (u64, u64, u64) = store
        .call(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*),SUM(input),SUM(json_extract(payload,'$.logical_success')=0) FROM usage_observations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(counts, (3, 6, 1));
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn invalid_embedding_vectors_record_usage_and_logical_failure_without_replay() {
    let s = setup(|cfg, url| {
        cfg.codex_api_key.clear();
        cfg.usage.database = Some(format!("{}/usage.sqlite3", cfg.auth_dir));
        cfg.vertex_api_key = ["a", "b"]
            .map(|key| KeyEntry { api_key: key.into(), base_url: Some(url.into()), ..Default::default() })
            .to_vec();
    })
    .await;
    // The mock's float parser emits a null vector component for NaN.
    let response = reqwest::Client::new()
        .post(format!("{}/v1/embeddings", s.proxy.url))
        .bearer_auth("client")
        .json(&json!({"model":"gemini-embedding-001","input":["NaN"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    assert_eq!(s.mock.embedding_calls.lock().len(), 1);
    let store = s.app.usage.as_ref().unwrap();
    store.flush().await.unwrap();
    let counts: (u64, u64, u64) = store
        .call(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*),SUM(input),SUM(json_extract(payload,'$.logical_success')=0) FROM usage_observations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(counts, (1, 3, 1));
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn embedding_result_cache_is_opt_in_scoped_and_has_zero_provider_usage_on_hit() {
    let s = setup(|cfg, url| {
        cfg.codex_api_key.clear();
        cfg.api_keys.push("second-client".into());
        cfg.embeddings.cache_seconds = 60;
        cfg.vertex_api_key =
            vec![KeyEntry { api_key: "google".into(), base_url: Some(url.into()), ..Default::default() }];
    })
    .await;
    let body = json!({"model":"gemini-embedding-001","input":["1"],"dimensions":2});
    for (i, (key, dimensions)) in
        [("client", 2), ("client", 2), ("second-client", 2), ("client", 3)].into_iter().enumerate()
    {
        let mut request = body.clone();
        request["dimensions"] = dimensions.into();
        let response = reqwest::Client::new()
            .post(format!("{}/v1/embeddings", s.proxy.url))
            .bearer_auth(key)
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let value: Value = response.json().await.unwrap();
        if i == 1 {
            assert_eq!(value["fusebox_cache_hit"], true);
            assert_eq!(value["usage"]["total_tokens"], 0);
        } else {
            assert!(value["fusebox_cache_hit"].is_null());
        }
    }
    assert_eq!(s.mock.embedding_calls.lock().len(), 3);
    let mut cfg = (*s.app.cfg()).clone();
    cfg.embeddings.cache_seconds = 0;
    s.app.set_config(cfg.clone());
    cfg.embeddings.cache_seconds = 60;
    s.app.set_config(cfg);
    let value: Value = reqwest::Client::new()
        .post(format!("{}/v1/embeddings", s.proxy.url))
        .bearer_auth("client")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(value["fusebox_cache_hit"].is_null(), "disabled cache retained an old result");
    assert_eq!(s.mock.embedding_calls.lock().len(), 4);
}

#[tokio::test]
async fn token_and_latency_metrics_share_the_actual_provider_model_cohort() {
    let s = setup(|cfg, _| {
        for entry in &mut cfg.codex_api_key {
            entry.models = vec![crate::config::ModelAlias { name: "gpt-up".into(), alias: Some("friendly".into()) }];
        }
    })
    .await;
    assert_eq!(ask(&s, "client", "friendly", &[]).await.0, 200);
    let metrics =
        reqwest::Client::new().get(format!("{}/metrics", s.proxy.url)).send().await.unwrap().text().await.unwrap();
    let cohort = "provider=\"openai\",model=\"gpt-up\",transport=\"http\"";
    assert!(metrics.contains(&format!("fusebox_usage_tokens_total{{{cohort},kind=\"input\"}} 10")), "{metrics}");
    assert!(metrics.contains(&format!("fusebox_total_seconds_count{{{cohort}}} 1")), "{metrics}");
    assert!(!metrics.contains("provider=\"codex\",model=\"friendly\""));
}

#[tokio::test]
async fn absolute_pre_output_timeout_is_terminal_and_does_not_duplicate_inference() {
    let s = setup(|cfg, _| {
        cfg.stream_limits.pre_output_timeout_seconds = 1;
        cfg.first_token_timeout_seconds = 5;
    })
    .await;
    s.mock.acts.lock().insert(("a".into(), "gpt-up".into()), Act::Stall);
    let start = Instant::now();
    let response = ask(&s, "client", "gpt-up", &[]).await;
    assert_eq!(response.0, 504);
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(s.mock.calls.lock().len(), 1);
}
