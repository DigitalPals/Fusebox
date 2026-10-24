//! Exercise actual HTTP and WebSocket routes against token-free local providers.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use axum::Json;
use axum::extract::{
    State,
    ws::{Message, WebSocketUpgrade},
};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

use crate::config::{Config, KeyEntry, Routing};
use crate::state::{App, RequestLog};

#[derive(Default)]
struct Mock {
    mode: AtomicU8,
    sequence: AtomicU64,
    calls: Mutex<Vec<(String, Value, &'static str)>>,
    headers: Mutex<Vec<(String, HeaderMap, &'static str)>>,
    ws_connections: AtomicU64,
    ws_mode: Mutex<WsMode>,
    ws_rich_output: AtomicBool,
    ws_control: Mutex<Option<WsControl>>,
}

#[derive(Clone, Copy, Default)]
enum WsMode {
    #[default]
    Normal,
    Partial,
    DropOnHandshake,
}

enum WsCommand {
    Ping(Vec<u8>),
    Data(Value),
    Flood,
    Close,
    Drop,
}

#[derive(Debug, PartialEq)]
enum WsObservation {
    Pong(Vec<u8>),
    Closed,
    Dropped,
    Flooding,
}

struct WsControl {
    commands: tokio::sync::mpsc::UnboundedReceiver<WsCommand>,
    observations: tokio::sync::mpsc::UnboundedSender<WsObservation>,
}

struct WsPeer {
    commands: tokio::sync::mpsc::UnboundedSender<WsCommand>,
    observations: tokio::sync::mpsc::UnboundedReceiver<WsObservation>,
}

impl WsPeer {
    async fn observe(&mut self) -> WsObservation {
        tokio::time::timeout(std::time::Duration::from_secs(3), self.observations.recv())
            .await
            .expect("proxy did not service the idle upstream websocket")
            .expect("mock upstream disconnected before the expected observation")
    }
}

fn account(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .or_else(|| headers.get("x-api-key"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .trim_start_matches("Bearer ")
        .into()
}

fn completed(mock: &Mock, account: &str, body: &Value) -> Value {
    json!({"id":format!("resp_{}", mock.sequence.fetch_add(1, Ordering::Relaxed)), "object":"response", "status":"completed", "model":body["model"], "output":[{"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":account}]}], "usage":{"input_tokens":100, "input_tokens_details":{"cached_tokens":80}, "output_tokens":1}})
}

fn events(response: &Value, omit_output: bool) -> Vec<Value> {
    let mut terminal = response.clone();
    if omit_output {
        terminal["output"] = json!([]);
    }
    vec![
        json!({"type":"response.created", "response":{"id":response["id"], "model":response["model"]}}),
        json!({"type":"response.output_text.delta", "delta":response["output"][0]["content"][0]["text"], "output_index":0, "content_index":0}),
        json!({"type":"response.completed", "response":terminal}),
    ]
}

fn rich_events(response: &mut Value) -> Vec<Value> {
    let message = response["output"][0].clone();
    response["output"] = json!([
        {"id":"reasoning_local", "type":"reasoning", "summary":[{"type":"summary_text", "text":"tool planning"}], "encrypted_content":"local-encrypted-reasoning"},
        {"id":"tool_local", "type":"function_call", "call_id":"call_local", "name":"lookup", "arguments":"{\"query\":\"local\"}"},
        message
    ]);
    let mut out = vec![json!({"type":"response.created", "response":{"id":response["id"], "model":response["model"]}})];
    for (index, item) in response["output"].as_array().unwrap().iter().enumerate() {
        out.push(json!({"type":"response.output_item.added", "output_index":index, "item":item}));
        if item["type"] == "reasoning" {
            out.push(
                json!({"type":"response.reasoning_summary_text.delta", "output_index":index, "delta":"tool planning"}),
            );
        }
        out.push(json!({"type":"response.output_item.done", "output_index":index, "item":item}));
    }
    let mut terminal = response.clone();
    // The proxy must rebuild the history from the streamed tool and reasoning items.
    terminal["output"] = json!([]);
    out.push(json!({"type":"response.completed", "response":terminal}));
    out
}

async fn mock_http(State(mock): State<Arc<Mock>>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let account = account(&headers);
    mock.headers.lock().push((account.clone(), headers.clone(), "http"));
    mock.calls.lock().push((account.clone(), body.clone(), "http"));
    let mode = mock.mode.load(Ordering::Relaxed);
    if mode == 7 && account != "a" && mock.calls.lock().iter().filter(|(a, _, _)| a == &account).count() == 1 {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":{"message":"temporary detour overload"}})))
            .into_response();
    }
    if account == "a" && ((1..=3).contains(&mode) || mode == 6 || mode == 8) {
        let (status, code) = match mode {
            1 => (429, "usage_limit_reached"),
            2 => (429, "rate_limit_exceeded"),
            6 => (401, "invalid_api_key"),
            8 => (529, "overloaded_error"),
            _ => (503, "server_is_overloaded"),
        };
        return (
            StatusCode::from_u16(status).unwrap(),
            [("retry-after", "120")],
            Json(json!({"error":{"code":code, "message":"mock error", "resets_in_seconds":3600}})),
        )
            .into_response();
    }
    if account == "a" && mode == 4 {
        let event = json!({"type":"response.failed", "response":{"error":{"code":"usage_limit_reached", "message":"mock error"}}});
        return ([("content-type", "text/event-stream")], format!("data: {event}\n\n")).into_response();
    }
    if account == "a" && mode == 9 {
        let event = json!({"type":"response.failed", "response":{"error":{"code":"server_is_overloaded", "message":"mock overloaded"}}});
        return ([("content-type", "text/event-stream")], format!("data: {event}\n\n")).into_response();
    }
    if body["messages"].is_array() {
        return Json(json!({"id":"message", "type":"message", "role":"assistant", "model":body["model"], "content":[{"type":"text", "text":account}], "stop_reason":"end_turn", "usage":{"input_tokens":100,"output_tokens":1}})).into_response();
    }
    let response = completed(&mock, &account, &body);
    let mut out = if body["stream"] == true {
        let sse: String = events(&response, true).into_iter().map(|v| format!("data: {v}\n\n")).collect();
        ([("content-type", "text/event-stream")], sse).into_response()
    } else {
        Json(response).into_response()
    };
    if account == "a" && mode == 5 {
        for (k, v) in [
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-primary-used-percent", "100"),
            ("x-codex-primary-reset-after-seconds", "3600"),
        ] {
            out.headers_mut().insert(k, v.parse().unwrap());
        }
    }
    out
}

async fn mock_chat(State(mock): State<Arc<Mock>>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let account = account(&headers);
    mock.headers.lock().push((account.clone(), headers.clone(), "chat"));
    mock.calls.lock().push((account.clone(), body.clone(), "chat"));
    Json(json!({
        "id":"chat-response", "object":"chat.completion", "model":body["model"],
        "choices":[{"index":0,"message":{"role":"assistant","content":account},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":100,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":80}}
    }))
    .into_response()
}

async fn mock_ws(State(mock): State<Arc<Mock>>, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Response {
    let account = account(&headers);
    mock.headers.lock().push((account.clone(), headers.clone(), "ws"));
    upgrade.on_upgrade(move |socket| async move {
        mock.ws_connections.fetch_add(1, Ordering::Relaxed);
        if matches!(*mock.ws_mode.lock(), WsMode::DropOnHandshake) {
            return;
        }
        let (mut tx, mut rx) = socket.split();
        let mut control = mock.ws_control.lock().take();
        let mut known = std::collections::HashSet::<String>::new();
        let mut flooding = false;
        let mut flood_announced = false;
        loop {
            let message = tokio::select! {
                // The provider must still read and answer requests while sending idle frames.
                biased;
                message = rx.next() => message,
                command = async {
                    match &mut control {
                        Some(control) => control.commands.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match command {
                        Some(WsCommand::Ping(data)) => {
                            if tx.send(Message::Ping(data.into())).await.is_err() { return; }
                        }
                        Some(WsCommand::Data(value)) => {
                            if tx.send(Message::Text(value.to_string().into())).await.is_err() { return; }
                        }
                        Some(WsCommand::Flood) => flooding = true,
                        Some(WsCommand::Close) => {
                            if tx.send(Message::Close(None)).await.is_err() { return; }
                        }
                        Some(WsCommand::Drop) => {
                            drop(tx);
                            drop(rx);
                            let _ = control.as_ref().unwrap().observations.send(WsObservation::Dropped);
                            return;
                        }
                        None => control = None,
                    }
                    continue;
                }
                result = async {
                    for _ in 0..64 {
                        tx.feed(Message::Pong(vec![0; 16].into())).await?;
                    }
                    tx.flush().await
                }, if flooding => {
                    if result.is_err() { return; }
                    if !flood_announced {
                        let _ = control.as_ref().unwrap().observations.send(WsObservation::Flooding);
                        flood_announced = true;
                    }
                    continue;
                }
            };
            let text = match message {
                Some(Ok(Message::Text(text))) => text,
                Some(Ok(Message::Pong(data))) => {
                    if let Some(control) = &control {
                        let _ = control.observations.send(WsObservation::Pong(data.to_vec()));
                    }
                    continue;
                }
                Some(Ok(Message::Close(_))) => {
                    let _ = tx.flush().await;
                    if let Some(control) = &control {
                        let _ = control.observations.send(WsObservation::Closed);
                    }
                    return;
                }
                Some(Ok(_)) => continue,
                _ => {
                    if let Some(control) = &control {
                        let _ = control.observations.send(WsObservation::Dropped);
                    }
                    return;
                }
            };
            let body: Value = serde_json::from_str(&text).unwrap();
            mock.calls.lock().push((account.clone(), body.clone(), "ws"));
            if account == "a" && mock.mode.load(Ordering::Relaxed) == 1 {
                let error = json!({"type":"error", "status":429, "error":{"code":"usage_limit_reached", "message":"mock error", "resets_in_seconds":3600}});
                if tx.send(Message::Text(error.to_string().into())).await.is_err() { break; }
                continue;
            }
            if let Some(prev) = body["previous_response_id"].as_str() && !known.contains(prev) {
                let error = json!({"type":"error", "status":400, "error":{"code":"previous_response_not_found", "message":"unknown on this socket"}});
                if tx.send(Message::Text(error.to_string().into())).await.is_err() { break; }
                continue;
            }
            let mut response = completed(&mock, &account, &body);
            known.insert(response["id"].as_str().unwrap().to_string());
            let mut output = if mock.ws_rich_output.load(Ordering::Relaxed) {
                rich_events(&mut response)
            } else {
                events(&response, true)
            };
            let partial = matches!(*mock.ws_mode.lock(), WsMode::Partial);
            if partial {
                output.truncate(2);
            }
            for event in output {
                if tx.send(Message::Text(event.to_string().into())).await.is_err() { return; }
            }
            if partial {
                return;
            }
        }
    }).into_response()
}

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

async fn serve(router: axum::Router) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
    });
    Server { url, task }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Fixture {
    directory: PathBuf,
    cfg: Config,
    app: Arc<App>,
    proxy: Server,
    provider: Server,
    mock: Arc<Mock>,
}

impl Fixture {
    fn control_ws(&self) -> WsPeer {
        let (commands, command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (observation_tx, observations) = tokio::sync::mpsc::unbounded_channel();
        *self.mock.ws_control.lock() = Some(WsControl { commands: command_rx, observations: observation_tx });
        WsPeer { commands, observations }
    }

    async fn new(routing: Routing, native: bool) -> Self {
        let mock = Arc::new(Mock::default());
        let provider = serve(
            axum::Router::new()
                .route("/v1/responses", post(mock_http).get(mock_ws))
                .route("/v1/responses/compact", post(mock_http))
                .route("/v1/messages", post(mock_http))
                .route("/v1/chat/completions", post(mock_chat))
                .layer(axum::extract::DefaultBodyLimit::disable())
                .with_state(mock.clone()),
        )
        .await;
        let directory = std::env::temp_dir().join(format!("fusebox-routing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut cfg = Config {
            auth_dir: directory.to_string_lossy().into(),
            api_keys: vec!["client-one".into(), "client-two".into()],
            routing,
            codex_websockets: native,
            ..Default::default()
        };
        if native {
            for account in ["a", "b"] {
                std::fs::write(directory.join(format!("{account}.json")), json!({"type":"codex", "access_token":account, "email":account, "base_url":format!("{}/v1", provider.url)}).to_string()).unwrap();
            }
        } else {
            cfg.codex_api_key = ["a", "b"]
                .map(|account| KeyEntry {
                    api_key: account.into(),
                    base_url: Some(format!("{}/v1", provider.url)),
                    label: Some(account.into()),
                    ..Default::default()
                })
                .to_vec();
        }
        let app = App::new(cfg.clone(), directory.join("config.yaml"));
        let proxy = serve(crate::server::router(app.clone())).await;
        Self { directory, cfg, app, proxy, provider, mock }
    }

    async fn request(&self, task: Option<&str>, body: Value) -> (u16, Value) {
        let mut request = reqwest::Client::new()
            .post(format!("{}/v1/responses", self.proxy.url))
            .bearer_auth("client-one")
            .json(&body);
        if let Some(task) = task {
            request = request.header("thread-id", task);
        }
        let response = request.send().await.unwrap();
        (response.status().as_u16(), response.json().await.unwrap())
    }

    /// A Responses request from client-one with exactly these extra headers.
    async fn request_with(&self, headers: &[(&str, &str)], body: Value) -> Value {
        let mut request = reqwest::Client::new()
            .post(format!("{}/v1/responses", self.proxy.url))
            .bearer_auth("client-one")
            .json(&body);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.send().await.unwrap().json().await.unwrap()
    }

    async fn socket(
        &self,
        task: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
        self.socket_with_task(Some(task)).await
    }

    async fn socket_with_task(
        &self,
        task: Option<&str>,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
        let mut request =
            format!("{}/v1/responses", self.proxy.url.replacen("http://", "ws://", 1)).into_client_request().unwrap();
        request.headers_mut().insert("authorization", "Bearer client-one".parse().unwrap());
        if let Some(task) = task {
            request.headers_mut().insert("thread-id", task.parse().unwrap());
        }
        tokio_tungstenite::connect_async(request).await.unwrap().0
    }

    async fn logs(&self, count: usize) -> Vec<RequestLog> {
        // A WebSocket's terminal event can reach the client just before its tracker finishes.
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let mut logs: Vec<_> = self.app.stats.recent.lock().iter().cloned().collect();
                if logs.len() >= count {
                    logs.sort_by_key(|log| log.id);
                    return logs;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("request diagnostics were not recorded")
    }

    fn recover(&self) {
        self.mock.mode.store(0, Ordering::Relaxed);
        for account in self.app.pool.all() {
            let mut state = account.state.lock();
            state.cooldowns.clear();
            state.quota_cooldowns.clear();
            state.quota = Default::default();
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.proxy.task.abort();
        self.provider.task.abort();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn prompt() -> Value {
    json!({"model":"gpt-6.1-sol","input":"question"})
}
fn answer(response: &Value) -> &str {
    response["output"].as_array().unwrap().iter().find_map(|item| item["content"][0]["text"].as_str()).unwrap()
}

async fn turn(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    mut body: Value,
) -> Value {
    body["type"] = "response.create".into();
    socket.send(tungstenite::Message::Text(body.to_string().into())).await.unwrap();
    loop {
        let event = ws_event(socket).await;
        assert_ne!(event["type"], "error", "{event}");
        if event["type"] == "response.completed" {
            return event["response"].clone();
        }
    }
}

async fn ws_event(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
) -> Value {
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(10), socket.next())
            .await
            .expect("proxy did not finish the websocket turn")
            .unwrap()
            .unwrap();
        match frame {
            tungstenite::Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            tungstenite::Message::Close(_) => panic!("proxy closed the client websocket before completing the turn"),
            _ => {}
        }
    }
}

async fn client_ping(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
) {
    let data = b"idle-client-barrier".to_vec();
    socket.send(tungstenite::Message::Ping(data.clone().into())).await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(3), socket.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(reply, tungstenite::Message::Pong(data.into()));
}

#[tokio::test]
async fn smart_balancing_packs_earlier_resets_and_respects_reserve_over_http_and_websockets() {
    for native in [false, true] {
        let fixture = Fixture::new(Routing::SmartQuota, native).await;
        let accounts = fixture.app.pool.all();
        let now = chrono::Utc::now();
        for (account, days) in accounts.iter().zip([3, 5]) {
            account.state.lock().quota = crate::quota::Quota {
                windows: vec![
                    crate::quota::Window {
                        name: "5h".into(),
                        used: 0.0,
                        resets_at: Some(now + chrono::Duration::hours(4)),
                        model: None,
                    },
                    crate::quota::Window {
                        name: "week".into(),
                        used: 20.0,
                        resets_at: Some(now + chrono::Duration::days(days)),
                        model: None,
                    },
                ],
                updated_at: Some(now),
                ..Default::default()
            };
        }
        let first = fixture.request(Some("http-first"), prompt()).await;
        assert_eq!(first.0, 200);
        assert_eq!(answer(&first.1), "a");
        let mut socket = fixture.socket("ws-second").await;
        turn(&mut socket, prompt()).await;
        assert_eq!(fixture.mock.calls.lock().last().unwrap().0, "a"); // load does not outweigh the earlier reset
        assert_eq!(answer(&fixture.request(Some("http-first"), prompt()).await.1), "a");

        // Change the reserve at runtime without disturbing either existing assignment.
        let mut cfg = fixture.cfg.clone();
        cfg.five_hour_reserve_percent = 60;
        fixture.app.set_config(cfg);
        accounts[0].state.lock().quota.windows[0].used = 41.0;
        assert_eq!(answer(&fixture.request(Some("new-after-reserve"), prompt()).await.1), "b");
        assert_eq!(answer(&fixture.request(Some("http-first"), prompt()).await.1), "a");
        turn(&mut socket, prompt()).await;
        assert_eq!(fixture.mock.calls.lock().last().unwrap().0, "a"); // the existing WebSocket remains pinned

        let logs = fixture.logs(6).await;
        assert!(logs.iter().all(|log| log.routing_strategy == Routing::SmartQuota));
        assert!(accounts.iter().all(|a| a.state.lock().active_requests.load(Ordering::Relaxed) == 0));
        socket.close(None).await.unwrap();
    }
}

#[tokio::test]
async fn smart_reserve_admits_idle_accounts_and_releases_on_session_end_over_http_and_websockets() {
    for native in [false, true] {
        for window in [0, 1] {
            let fixture = Fixture::new(Routing::SmartQuota, native).await;
            let accounts = fixture.app.pool.all();
            let now = chrono::Utc::now();
            for (account, days) in accounts.iter().zip([3, 5]) {
                account.state.lock().quota = crate::quota::Quota {
                    windows: vec![
                        crate::quota::Window {
                            name: "5h".into(),
                            used: 0.0,
                            resets_at: Some(now + chrono::Duration::hours(4)),
                            model: None,
                        },
                        crate::quota::Window {
                            name: "week".into(),
                            used: 20.0,
                            resets_at: Some(now + chrono::Duration::days(days)),
                            model: None,
                        },
                    ],
                    updated_at: Some(now),
                    ..Default::default()
                };
            }
            accounts[0].state.lock().quota.windows[window].used = if window == 0 { 80.0 } else { 96.0 };
            assert_eq!(answer(&fixture.request(Some("idle-admission"), prompt()).await.1), "a");
            let mut socket = fixture.socket("additional-session").await;
            turn(&mut socket, prompt()).await;
            assert_eq!(fixture.mock.calls.lock().last().unwrap().0, "b");
            assert_eq!(answer(&fixture.request(Some("idle-admission"), prompt()).await.1), "a");

            let response = reqwest::Client::new()
                .post(format!("{}/v1/responses", fixture.proxy.url))
                .bearer_auth("client-one")
                .header("thread-id", "idle-admission")
                .header("x-cliproxy-session-end", "true")
                .json(&prompt())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(answer(&response.json::<Value>().await.unwrap()), "a");
            assert_eq!(answer(&fixture.request(Some("after-session-end"), prompt()).await.1), "a");
            turn(&mut socket, prompt()).await;
            assert_eq!(fixture.mock.calls.lock().last().unwrap().0, "b");
            let logs = fixture.logs(6).await;
            assert_eq!(logs[0].routing_reason, Some("new_session"));
            assert_eq!(logs[2].routing_reason, Some("session_reused"));
            assert_eq!(logs[4].routing_reason, Some("new_session"));
            socket.close(None).await.unwrap();
        }
    }
}

#[tokio::test]
async fn smart_balancing_uses_weekly_headroom_without_inventing_a_five_hour_reserve() {
    for native in [false, true] {
        let fixture = Fixture::new(Routing::SmartQuota, native).await;
        let accounts = fixture.app.pool.all();
        let reset = chrono::Utc::now() + chrono::Duration::days(3);
        for (account, used) in accounts.iter().zip([80.0, 20.0]) {
            crate::quota::authoritative(
                &mut account.state.lock(),
                vec![crate::quota::Window { name: "week".into(), used, resets_at: Some(reset), model: None }],
                None,
            );
        }
        // Identical renewals: known weekly headroom beats the old equal 50% fallback.
        assert_eq!(answer(&fixture.request(Some("weekly-http"), prompt()).await.1), "b");
        let mut socket = fixture.socket("weekly-ws").await;
        turn(&mut socket, prompt()).await;
        assert_eq!(fixture.mock.calls.lock().last().unwrap().0, "b");

        // A real 5-hour window at the reserve takes precedence over weekly-only headroom.
        accounts[0].state.lock().quota.windows.push(crate::quota::Window {
            name: "5h".into(),
            used: 70.0,
            resets_at: Some(reset),
            model: None,
        });
        assert_eq!(answer(&fixture.request(Some("real-reserve"), prompt()).await.1), "a");
        assert_eq!(answer(&fixture.request(Some("weekly-http"), prompt()).await.1), "b");
        turn(&mut socket, prompt()).await;
        assert_eq!(fixture.mock.calls.lock().last().unwrap().0, "b"); // existing assignment stays pinned
        socket.close(None).await.unwrap();
    }
}

#[tokio::test]
async fn request_load_releases_on_retry_finish_cancel_and_drop() {
    let fixture = Fixture::new(Routing::SmartQuota, false).await;
    let accounts = fixture.app.pool.all();
    let count = |i: usize| accounts[i].state.lock().active_requests.load(Ordering::Relaxed);
    let new_tracker =
        || crate::proxy::Tracker::new(&fixture.app, crate::ir::Format::Responses, true, "http", "gpt-6.1-sol");
    let mut tracker = new_tracker();
    tracker.attempt(&accounts[0]);
    assert_eq!(count(0), 1);
    tracker.attempt(&accounts[0]); // retry on same account must not accumulate
    assert_eq!(count(0), 1);
    assert_eq!(answer(&fixture.request(None, prompt()).await.1), "b"); // unbound requests see in-flight work
    tracker.attempt(&accounts[1]);
    assert_eq!((count(0), count(1)), (0, 1));
    tracker.finish(200, &crate::ir::Usage::default(), None);
    assert_eq!((count(0), count(1)), (0, 0));
    drop(tracker);
    let mut tracker = new_tracker();
    tracker.attempt(&accounts[0]);
    tracker.cancel();
    tracker.cancel();
    assert_eq!(count(0), 0);
    drop(tracker);
    let mut tracker = new_tracker();
    tracker.attempt(&accounts[0]);
    drop(tracker); // client disconnects mid-stream
    assert_eq!(count(0), 0);
}

#[tokio::test]
async fn http_tasks_stay_pinned_and_migrate_only_when_quota_runs_out() {
    for routing in [Routing::LeastUsed, Routing::SmartQuota, Routing::RoundRobin, Routing::FillFirst] {
        let fixture = Fixture::new(routing, false).await;
        let task = "private-coding-thread-identifier";
        for _ in 0..3 {
            let (status, response) = fixture.request(Some(task), prompt()).await;
            assert_eq!(status, 200);
            assert_eq!(answer(&response), "a");
        }
        let initial = fixture.logs(3).await;
        let session = initial[0].session_id.as_deref().unwrap();
        assert_eq!(session.len(), 64);
        assert!(session.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(initial[0].routing_reason, Some("new_session"));
        for log in &initial[1..] {
            assert_eq!(log.routing_reason, Some("session_reused"));
        }
        fixture.mock.mode.store(1, Ordering::Relaxed);
        let (status, response) = fixture.request(Some(task), prompt()).await;
        assert_eq!(status, 200);
        assert_eq!(answer(&response), "b");
        let migrated = fixture.logs(4).await.pop().unwrap();
        assert_eq!(migrated.routing_reason, Some("quota_exhausted"));
        assert_eq!(migrated.routing_attempts.len(), 2);
        assert_eq!(migrated.routing_attempts[0].reason, "session_reused");
        assert_eq!(migrated.routing_attempts[1].reason, "quota_exhausted");
        assert_eq!(
            migrated.routing_attempts[1].previous_account.as_deref(),
            Some(initial[0].routing_attempts[0].account_id.as_str())
        );
        assert_ne!(migrated.routing_attempts[0].account_id, migrated.routing_attempts[1].account_id);
        fixture.recover();
        for _ in 0..3 {
            assert_eq!(answer(&fixture.request(Some(task), prompt()).await.1), "b");
        }
        let logs = fixture.logs(7).await;
        for log in &logs {
            assert_eq!(log.session_id.as_deref(), Some(session));
            assert_eq!(log.session_source, Some("thread-id"));
            assert_eq!(log.routing_strategy, routing);
            assert_eq!(log.routing_warning, None);
        }
        for log in &logs[4..] {
            assert_eq!(log.routing_reason, Some("session_reused"));
        }
        let serialized = serde_json::to_string(&logs).unwrap();
        assert!(!serialized.contains(task));
        assert!(!serialized.contains("client-one"));
        let restarted = App::new(fixture.cfg.clone(), fixture.directory.join("config.yaml"));
        let proxy = serve(crate::server::router(restarted)).await;
        let response: Value = reqwest::Client::new()
            .post(format!("{}/v1/responses", proxy.url))
            .bearer_auth("client-one")
            .header("thread-id", task)
            .json(&prompt())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(answer(&response), "b");
    }
}

#[tokio::test]
async fn temporary_failures_detour_and_the_session_returns_to_its_subscription() {
    for mode in [2, 3, 6] {
        let fixture = Fixture::new(Routing::RoundRobin, false).await;
        assert_eq!(answer(&fixture.request(Some("task"), prompt()).await.1), "a");
        fixture.mock.mode.store(mode, Ordering::Relaxed);
        // The client still gets an answer, from the other subscription...
        let (status, body) = fixture.request(Some("task"), prompt()).await;
        assert_eq!(status, 200, "mode {mode}");
        assert_eq!(answer(&body), "b");
        assert_eq!(fixture.logs(2).await.pop().unwrap().routing_reason, Some("temporary_detour"));
        // ...and the session goes back to its own once that recovers.
        fixture.recover();
        assert_eq!(answer(&fixture.request(Some("task"), prompt()).await.1), "a");
    }
}

#[tokio::test]
async fn overload_errors_expire_without_another_inference_over_http_and_streams() {
    for (mode, expected_status) in [(8, 529), (9, 503)] {
        let fixture = Fixture::new(Routing::RoundRobin, false).await;
        assert_eq!(answer(&fixture.request(Some("task"), prompt()).await.1), "a");
        let account = fixture.app.pool.all().into_iter().find(|a| a.label == "a").unwrap();
        fixture.mock.mode.store(mode, Ordering::Relaxed);
        let (status, body) = fixture.request(Some("task"), prompt()).await;
        assert_eq!(status, 200);
        assert_eq!(answer(&body), "b", "another account answers the failed request");
        let snapshot = account.snapshot();
        assert!(snapshot["last_error"].as_str().unwrap().starts_with(&format!("{expected_status}:")));
        let until = chrono::DateTime::parse_from_rfc3339(snapshot["last_error_until"].as_str().unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc);
        let now = chrono::Utc::now();
        assert!(until > now && until <= now + chrono::Duration::seconds(30));
        let calls = fixture.mock.calls.lock().len();
        assert_eq!(account.state.lock().last_error(until), None, "no successful inference is required");
        assert_eq!(fixture.mock.calls.lock().len(), calls);
        fixture.mock.mode.store(0, Ordering::Relaxed);
        assert_eq!(answer(&fixture.request(Some("task"), prompt()).await.1), "a", "the session keeps its account");
        assert_eq!(account.snapshot()["last_error"], Value::Null);
        assert_eq!(account.snapshot()["last_error_until"], Value::Null);
    }
}

#[tokio::test]
async fn quota_headers_and_stream_errors_trigger_migration() {
    // A used-up limit reported inside a 200 stream, before any content, moves this very
    // request to the next account; one reported in headers moves the next request.
    for mode in [4, 5] {
        let fixture = Fixture::new(Routing::RoundRobin, false).await;
        fixture.mock.mode.store(mode, Ordering::Relaxed);
        let (status, body) = fixture.request(Some("task"), prompt()).await;
        assert_eq!(status, 200, "mode {mode}");
        assert_eq!(answer(&body), if mode == 4 { "b" } else { "a" }, "mode {mode}");
        fixture.mock.mode.store(0, Ordering::Relaxed);
        assert_eq!(answer(&fixture.request(Some("task"), prompt()).await.1), "b");
    }
}

#[tokio::test]
async fn response_ids_continue_the_task_without_session_headers_and_replay_full_input() {
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    let first = fixture.request(None, prompt()).await.1;
    assert_eq!(answer(&first), "a");
    let first_log = fixture.logs(1).await.pop().unwrap();
    // No session id: the conversation is recognised by its first message and assigned.
    assert_eq!(first_log.session_source, Some(crate::affinity::INFERRED_SOURCE));
    assert_eq!(first_log.routing_warning, Some("inferred_session"));
    assert_eq!(first_log.routing_reason, Some("new_session"));
    fixture.mock.mode.store(1, Ordering::Relaxed);
    let body = json!({"model":"gpt-6.1-sol","previous_response_id":first["id"],"input":"next"});
    let (status, next) = fixture.request(None, body).await;
    assert_eq!(status, 200);
    assert_eq!(answer(&next), "b");
    let next_log = fixture.logs(2).await.pop().unwrap();
    assert_eq!(next_log.session_id, first_log.session_id);
    assert_eq!(next_log.session_source, Some("previous_response_id"));
    assert_eq!(next_log.routing_warning, Some("response_id_only"));
    // The continuation found the first turn's assignment used up and moved it.
    assert_eq!(next_log.routing_reason, Some("quota_exhausted"));
    {
        let calls = fixture.mock.calls.lock();
        let (_, sent, _) = calls.last().unwrap();
        assert!(sent["previous_response_id"].is_null());
        assert_eq!(sent["input"].as_array().unwrap().len(), 3);
    }
    assert_eq!(
        fixture.request(None, json!({"model":"gpt-6.1-sol","previous_response_id":"external", "input":"next"})).await.0,
        200
    );
    // With no user message to go on, a generated id still threads later continuations.
    let tool_output =
        json!({"model":"gpt-6.1-sol","input":[{"type":"function_call_output","call_id":"call","output":"ok"}]});
    assert_eq!(fixture.request(None, tool_output).await.0, 200);
    let logs = fixture.logs(4).await;
    for log in &logs[2..] {
        assert_eq!(log.session_source, Some("generated_response"));
        assert_eq!(log.routing_warning, Some("missing_session_id"));
    }
}

#[tokio::test]
async fn websocket_fallback_and_reconnect_share_http_assignments_and_history() {
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    let first = fixture.request(Some("task"), prompt()).await.1;
    let mut socket = fixture.socket("task").await;
    let next =
        turn(&mut socket, json!({"model":"gpt-6.1-sol","previous_response_id":first["id"],"input":"next"})).await;
    socket.close(None).await.unwrap();
    let mut socket = fixture.socket("task").await;
    turn(&mut socket, json!({"model":"gpt-6.1-sol","previous_response_id":next["id"],"input":"last"})).await;
    assert!(fixture.mock.calls.lock().iter().all(|(account, _, _)| account == "a"));
    assert_eq!(fixture.mock.calls.lock().last().unwrap().1["input"].as_array().unwrap().len(), 5);
    let logs = fixture.logs(3).await;
    for log in &logs {
        assert_eq!(log.session_id, logs[0].session_id);
        assert!(log.session_id.is_some());
        assert_eq!(log.session_source, Some("thread-id"));
        assert_eq!(log.routing_warning, None);
    }
    for log in &logs[1..] {
        assert_eq!(log.transport, "ws");
        assert_eq!(log.routing_reason, Some("session_reused"));
    }
}

#[tokio::test]
async fn native_websocket_idle_ping_is_answered_before_next_turn_and_reuses_connection() {
    let fixture = Fixture::new(Routing::RoundRobin, true).await;
    let mut peer = fixture.control_ws();
    let mut socket = fixture.socket("idle-ping").await;
    let first = turn(&mut socket, prompt()).await;
    client_ping(&mut socket).await;
    let ping = b"idle-upstream-ping".to_vec();
    peer.commands.send(WsCommand::Ping(ping.clone())).unwrap();
    assert_eq!(peer.observe().await, WsObservation::Pong(ping));
    // No new request has been sent while the provider waits for its Pong.
    assert_eq!(fixture.mock.calls.lock().len(), 1);
    turn(&mut socket, json!({"model":"gpt-6.1-sol", "previous_response_id":first["id"], "input":"next"})).await;
    assert_eq!(fixture.mock.ws_connections.load(Ordering::Relaxed), 1);
    let calls = fixture.mock.calls.lock();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|(account, _, transport)| account == "a" && *transport == "ws"));
    assert_eq!(calls[1].1["previous_response_id"], first["id"]);
    assert_eq!(calls[1].1["input"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn native_websocket_reconnects_for_a_new_account_but_keeps_token_refreshes() {
    let fixture = Fixture::new(Routing::RoundRobin, true).await;
    let mut socket = fixture.socket("account-identity").await;
    let first = turn(&mut socket, prompt()).await;
    assert_eq!(fixture.mock.calls.lock()[0].0, "a");
    client_ping(&mut socket).await;
    let file = fixture.directory.join("a.json");
    let credentials = |token: &str, id: Option<&str>| {
        json!({"type":"codex","access_token":token,"account_id":id,"email":"a",
            "base_url":format!("{}/v1", fixture.provider.url)})
        .to_string()
    };
    std::fs::write(&file, credentials("refreshed", None)).unwrap();
    fixture.app.reload_accounts();
    let second =
        turn(&mut socket, json!({"model":"gpt-6.1-sol","previous_response_id":first["id"],"input":"next"})).await;
    assert_eq!(fixture.mock.calls.lock()[1].0, "a");
    assert_eq!(fixture.mock.ws_connections.load(Ordering::Relaxed), 1);
    client_ping(&mut socket).await;
    std::fs::write(&file, credentials("replacement", Some("different-account"))).unwrap();
    fixture.app.reload_accounts();
    turn(&mut socket, json!({"model":"gpt-6.1-sol","previous_response_id":second["id"],"input":"next"})).await;
    assert_eq!(fixture.mock.ws_connections.load(Ordering::Relaxed), 2);
    let calls = fixture.mock.calls.lock();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[2].0, "replacement");
    assert!(calls.iter().all(|(_, _, transport)| *transport == "ws"));
    assert!(calls[2].1.get("previous_response_id").is_none(), "the new account must receive local history");
    assert!(calls[2].1["input"].as_array().unwrap().len() > 1);
}

#[tokio::test]
async fn native_websocket_idle_disconnects_reconnect_and_replay_tool_and_reasoning_history() {
    for command in [WsCommand::Close, WsCommand::Drop] {
        let fixture = Fixture::new(Routing::RoundRobin, true).await;
        fixture.mock.ws_rich_output.store(true, Ordering::Relaxed);
        let mut peer = fixture.control_ws();
        let mut socket = fixture.socket("idle-disconnect").await;
        let first = turn(&mut socket, prompt()).await;
        client_ping(&mut socket).await;
        let expected = if matches!(command, WsCommand::Close) { WsObservation::Closed } else { WsObservation::Dropped };
        peer.commands.send(command).unwrap();
        assert_eq!(peer.observe().await, expected);
        // Exercise the client side while the proxy services the dropped upstream.
        client_ping(&mut socket).await;
        let input = json!([
            {"type":"function_call_output", "call_id":"call_local", "output":"tool result"},
            {"type":"message", "role":"user", "content":[{"type":"input_text", "text":"continue"}]}
        ]);
        turn(&mut socket, json!({"model":"gpt-6.1-sol", "previous_response_id":first["id"], "input":input})).await;
        assert_eq!(fixture.mock.ws_connections.load(Ordering::Relaxed), 2);
        let calls = fixture.mock.calls.lock();
        assert_eq!(calls.len(), 2, "the turn must not be retried through HTTP");
        assert!(calls.iter().all(|(account, _, transport)| account == "a" && *transport == "ws"));
        let replay = &calls[1].1;
        assert!(replay["previous_response_id"].is_null());
        let items = replay["input"].as_array().unwrap();
        assert_eq!(items.len(), 6, "{items:?}");
        assert_eq!(items[0]["content"][0]["text"], "question");
        let reasoning = items.iter().find(|item| item["type"] == "reasoning").unwrap();
        assert_eq!(reasoning["encrypted_content"], "local-encrypted-reasoning");
        assert_eq!(reasoning["summary"][0]["text"], "tool planning");
        let tool = items.iter().find(|item| item["type"] == "function_call").unwrap();
        assert_eq!(tool["call_id"], "call_local");
        assert_eq!(tool["name"], "lookup");
        assert_eq!(tool["arguments"], "{\"query\":\"local\"}");
        assert!(items.iter().any(|item| item["role"] == "assistant" && item["content"][0]["text"] == "a"));
        assert_eq!(&items[4..], input.as_array().unwrap());
        assert!(items.iter().all(|item| item["id"].is_null()));
    }
}

#[tokio::test]
async fn native_websocket_partial_stream_failure_is_reported_without_replay() {
    let fixture = Fixture::new(Routing::RoundRobin, true).await;
    let mut socket = fixture.socket("partial-stream").await;
    let first = turn(&mut socket, prompt()).await;
    *fixture.mock.ws_mode.lock() = WsMode::Partial;
    let body = json!({"type":"response.create", "model":"gpt-6.1-sol", "previous_response_id":first["id"], "input":"failed turn"});
    socket.send(tungstenite::Message::Text(body.to_string().into())).await.unwrap();
    assert_eq!(ws_event(&mut socket).await["type"], "response.created");
    let delta = ws_event(&mut socket).await;
    assert_eq!(delta["type"], "response.output_text.delta");
    assert_eq!(delta["delta"], "a");
    let failure = ws_event(&mut socket).await;
    assert_eq!(failure["type"], "error");
    assert_eq!(failure["status"], 502);
    assert_eq!(failure["error"]["type"], "upstream_error");
    assert_eq!(fixture.mock.calls.lock().len(), 2);
    assert_eq!(fixture.mock.ws_connections.load(Ordering::Relaxed), 1);
    let logs = fixture.logs(2).await;
    assert_eq!(logs[1].status, 502);
    assert!(logs[1].error.as_ref().unwrap().contains("websocket"));
    *fixture.mock.ws_mode.lock() = WsMode::Normal;
    turn(&mut socket, json!({"model":"gpt-6.1-sol", "previous_response_id":first["id"], "input":"retry explicitly"}))
        .await;
    assert_eq!(fixture.mock.ws_connections.load(Ordering::Relaxed), 2);
    let calls = fixture.mock.calls.lock();
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|(_, _, transport)| *transport == "ws"));
    assert!(calls[2].1["previous_response_id"].is_null());
    assert_eq!(calls[2].1["input"].as_array().unwrap().len(), 3);
    assert!(!calls[2].1["input"].to_string().contains("failed turn"));
}

#[tokio::test]
async fn native_websocket_idle_quota_events_update_routing_without_overwriting_newer_quota() {
    for newer_epoch in [false, true] {
        let fixture = Fixture::new(Routing::RoundRobin, true).await;
        let mut peer = fixture.control_ws();
        let mut socket = fixture.socket("idle-quota").await;
        let first = turn(&mut socket, prompt()).await;
        let log = fixture.logs(1).await.pop().unwrap();
        let account = fixture.app.pool.all().into_iter().find(|a| a.id == log.routing_attempts[0].account_id).unwrap();
        if newer_epoch {
            account.state.lock().quota_epoch += 1;
        }
        peer.commands.send(WsCommand::Data(json!({
            "type":"codex.rate_limits", "rate_limits":{"primary":{"window_minutes":300, "used_percent":100.0, "reset_after_seconds":3600}}
        }))).unwrap();
        // A Pong proves the preceding rate-limit frame was serviced during idle.
        peer.commands.send(WsCommand::Ping(b"quota-barrier".to_vec())).unwrap();
        assert_eq!(peer.observe().await, WsObservation::Pong(b"quota-barrier".to_vec()));
        {
            let state = account.state.lock();
            if newer_epoch {
                assert!(state.quota.windows.is_empty());
            } else {
                assert_eq!(state.quota.windows[0].used, 100.0);
            }
        }
        turn(&mut socket, json!({"model":"gpt-6.1-sol", "previous_response_id":first["id"], "input":"next"})).await;
        let calls = fixture.mock.calls.lock();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].2, "ws");
        if newer_epoch {
            assert_eq!(calls[1].0, "a");
            assert_eq!(calls[1].1["previous_response_id"], first["id"]);
        } else {
            assert_eq!(calls[1].0, "b");
            assert!(calls[1].1["previous_response_id"].is_null());
            assert_eq!(calls[1].1["input"].as_array().unwrap().len(), 3);
        }
    }
}

#[tokio::test]
async fn native_websocket_unexpected_idle_frames_retire_connection_without_rewriting_history() {
    for kind in ["error", "response.completed"] {
        let fixture = Fixture::new(Routing::RoundRobin, true).await;
        let mut peer = fixture.control_ws();
        let mut socket = fixture.socket("idle-unexpected").await;
        let first = turn(&mut socket, prompt()).await;
        peer.commands.send(WsCommand::Data(json!({"type":kind, "response":{"id":"unexpected"}}))).unwrap();
        assert_eq!(peer.observe().await, WsObservation::Dropped);
        turn(&mut socket, json!({"model":"gpt-6.1-sol", "previous_response_id":first["id"], "input":"next"})).await;
        assert_eq!(fixture.mock.ws_connections.load(Ordering::Relaxed), 2);
        let calls = fixture.mock.calls.lock();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|(_, _, transport)| *transport == "ws"));
        assert!(calls[1].1["previous_response_id"].is_null());
        assert_eq!(calls[1].1["input"].as_array().unwrap().len(), 3);
        assert_eq!(calls[1].1["input"][0]["content"][0]["text"], "question");
    }
}

#[tokio::test]
async fn native_websocket_busy_idle_upstream_does_not_starve_next_turn() {
    let fixture = Fixture::new(Routing::RoundRobin, true).await;
    let mut peer = fixture.control_ws();
    let mut socket = fixture.socket("idle-flood").await;
    let first = turn(&mut socket, prompt()).await;
    peer.commands.send(WsCommand::Flood).unwrap();
    assert_eq!(peer.observe().await, WsObservation::Flooding);
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        turn(&mut socket, json!({"model":"gpt-6.1-sol", "previous_response_id":first["id"], "input":"next"})),
    )
    .await
    .expect("continuous upstream frames starved a ready client turn");
    let connections = fixture.mock.ws_connections.load(Ordering::Relaxed);
    assert!(matches!(connections, 1 | 2));
    let calls = fixture.mock.calls.lock();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|(account, _, transport)| account == "a" && *transport == "ws"));
    // TCP may leave fewer than the drain limit ready. Both healthy reuse and retirement are valid.
    if connections == 1 {
        assert_eq!(calls[1].1["previous_response_id"], first["id"]);
        assert_eq!(calls[1].1["input"].as_array().unwrap().len(), 1);
    } else {
        assert!(calls[1].1["previous_response_id"].is_null());
        assert_eq!(calls[1].1["input"].as_array().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn native_websocket_dropped_connection_never_runs_a_turn_twice() {
    let fixture = Fixture::new(Routing::RoundRobin, true).await;
    *fixture.mock.ws_mode.lock() = WsMode::DropOnHandshake;
    let mut socket = fixture.socket("dropped-upstream").await;
    // TCP buffering decides whether the drop fails the send (macOS and Linux, usually) or is
    // only seen on the next read (Windows can buffer the whole message). A failed send never
    // reached upstream, so HTTP serves the turn once; after a complete send the turn may have
    // run, so it is reported and never replayed.
    let body = json!({"type":"response.create", "model":"gpt-6.1-sol", "input":"x".repeat(16 * 1024 * 1024)});
    socket.send(tungstenite::Message::Text(body.to_string().into())).await.unwrap();
    let served = loop {
        let event = ws_event(&mut socket).await;
        if event["type"] == "error" {
            assert_eq!(event["status"], 502, "{event}");
            assert_eq!(event["error"]["type"], "upstream_error");
            break false;
        }
        if event["type"] == "response.completed" {
            break true;
        }
    };
    assert_eq!(fixture.mock.ws_connections.load(Ordering::Relaxed), 1);
    {
        let calls = fixture.mock.calls.lock();
        if served {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].2, "http");
        } else {
            assert!(calls.is_empty(), "a completed send must not be replayed over HTTP");
        }
    }
    let logs = fixture.logs(1).await;
    assert_eq!(logs[0].status, if served { 200 } else { 502 });
    assert_eq!(logs[0].transport, "ws");
}

#[tokio::test]
async fn native_websocket_reconnect_replays_history_and_quota_failover_stays_on_replacement() {
    for routing in [Routing::LeastUsed, Routing::SmartQuota, Routing::RoundRobin, Routing::FillFirst] {
        let fixture = Fixture::new(routing, true).await;
        let mut socket = fixture.socket("task").await;
        let first = turn(&mut socket, prompt()).await;
        let second =
            turn(&mut socket, json!({"model":"gpt-6.1-sol","previous_response_id":first["id"],"input":"second"})).await;
        assert_eq!(fixture.mock.calls.lock().last().unwrap().1["previous_response_id"], first["id"]);
        socket.close(None).await.unwrap();
        let mut socket = fixture.socket("task").await;
        let third =
            turn(&mut socket, json!({"model":"gpt-6.1-sol","previous_response_id":second["id"],"input":"third"})).await;
        assert!(fixture.mock.calls.lock().last().unwrap().1["previous_response_id"].is_null());
        assert_eq!(fixture.mock.calls.lock().last().unwrap().1["input"].as_array().unwrap().len(), 5);
        fixture.mock.mode.store(1, Ordering::Relaxed);
        let fourth =
            turn(&mut socket, json!({"model":"gpt-6.1-sol","previous_response_id":third["id"],"input":"fourth"})).await;
        assert_eq!(fixture.mock.calls.lock().last().unwrap().0, "b");
        fixture.recover();
        turn(&mut socket, json!({"model":"gpt-6.1-sol","previous_response_id":fourth["id"],"input":"last"})).await;
        {
            let calls = fixture.mock.calls.lock();
            assert_eq!(calls.last().unwrap().0, "b");
            assert_eq!(calls.last().unwrap().2, "ws");
            assert_eq!(calls.last().unwrap().1["input"].as_array().unwrap().len(), 9);
        }
        // Quota exhaustion records the failed native turn and its successful HTTP fallback.
        let logs = fixture.logs(6).await;
        for log in &logs {
            assert_eq!(log.session_id, logs[0].session_id);
            assert!(log.session_id.is_some());
            assert_eq!(log.session_source, Some("thread-id"));
            assert_eq!(log.routing_strategy, routing);
            assert_eq!(log.routing_warning, None);
            assert_eq!(log.transport, "ws");
        }
        assert_eq!(logs[0].routing_reason, Some("new_session"));
        let migration = logs.iter().find(|log| log.routing_reason == Some("quota_exhausted")).unwrap();
        assert_eq!(migration.status, 200);
        assert_eq!(
            migration.routing_attempts[0].previous_account,
            Some(logs[0].routing_attempts[0].account_id.clone())
        );
        assert_eq!(logs.last().unwrap().routing_reason, Some("session_reused"));
    }
}

#[tokio::test]
async fn authentication_scopes_prevent_cross_client_pins_and_accept_query_keys() {
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    assert_eq!(answer(&fixture.request(Some("task"), prompt()).await.1), "a");
    let response: Value = reqwest::Client::new()
        .post(format!("{}/v1/responses?key=client-one", fixture.proxy.url))
        .header("thread-id", "task")
        .json(&prompt())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(answer(&response), "a");
    // Neither the Fusebox scope header nor its pre-rename name can borrow another client's pins.
    for name in ["x-fusebox-client-scope", "x-cliproxy-client-scope"] {
        let response: Value = reqwest::Client::new()
            .post(format!("{}/v1/responses", fixture.proxy.url))
            .bearer_auth("client-two")
            .header("thread-id", "task")
            .header(name, crate::affinity::scope_for_key(Some("client-one")))
            .json(&prompt())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(answer(&response), "b", "{name}");
    }
}

#[tokio::test]
async fn fusebox_session_headers_and_their_legacy_names_pin_the_same_session() {
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    let first = fixture.request_with(&[("x-fusebox-session-id", "task")], prompt()).await;
    assert_eq!(answer(&first), "a");
    let legacy = fixture.request_with(&[("x-cliproxy-session-id", "task")], prompt()).await;
    assert_eq!(answer(&legacy), "a");
    // With both present the Fusebox name wins: "other" is a new session, so it takes the next account.
    let both =
        fixture.request_with(&[("x-fusebox-session-id", "other"), ("x-cliproxy-session-id", "task")], prompt()).await;
    assert_eq!(answer(&both), "b");
    let logs = fixture.logs(3).await;
    assert_eq!(logs[0].session_source, Some("x-fusebox-session-id"));
    assert_eq!(logs[1].session_source, Some("x-cliproxy-session-id"));
    assert_eq!(logs[1].session_id, logs[0].session_id);
    assert_eq!(logs[1].routing_reason, Some("session_reused"));
    assert_eq!(logs[2].session_source, Some("x-fusebox-session-id"));
    assert_ne!(logs[2].session_id, logs[0].session_id);
    assert_eq!(logs[2].routing_reason, Some("new_session"));
}

#[tokio::test]
async fn compaction_uses_the_tasks_account_and_configuration_changes_do_not_move_it() {
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    let first = fixture.request(Some("task"), prompt()).await.1;
    let mut cfg = fixture.cfg.clone();
    cfg.routing = Routing::LeastUsed;
    fixture.app.set_config(cfg);
    let response: Value = reqwest::Client::new()
        .post(format!("{}/v1/responses/compact", fixture.proxy.url))
        .bearer_auth("client-one")
        .header("thread-id", "task")
        .json(&prompt())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(answer(&response), answer(&first));
    let logs = fixture.logs(2).await;
    assert_eq!(logs[1].session_id, logs[0].session_id);
    assert!(logs[1].session_id.is_some());
    assert_eq!(logs[1].session_source, Some("thread-id"));
    assert_eq!(logs[1].routing_strategy, Routing::LeastUsed);
    assert_eq!(logs[1].routing_reason, Some("session_reused"));
    assert_eq!(logs[1].routing_warning, None);
}

#[tokio::test]
async fn claude_metadata_pins_the_session_before_request_translation() {
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    let mut cfg = fixture.cfg.clone();
    cfg.claude_api_key = cfg.codex_api_key.clone();
    cfg.codex_api_key.clear();
    for key in &mut cfg.claude_api_key {
        key.base_url = Some(fixture.provider.url.clone());
    }
    fixture.app.set_config(cfg);
    for _ in 0..4 {
        let body = json!({"model":"claude-sonnet-4-6","messages":[{"role":"user","content":"question"}],"max_tokens":32,"metadata":{"user_id":json!({"device_id":"device","session_id":"claude-task"}).to_string()}});
        let response: Value = reqwest::Client::new()
            .post(format!("{}/v1/messages", fixture.proxy.url))
            .header("x-api-key", "client-one")
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["content"][0]["text"], "a");
    }
    assert!(fixture.mock.calls.lock().iter().all(|(account, _, _)| account == "a"));
}

#[tokio::test]
async fn ending_a_task_releases_its_assignment_and_missing_codex_history_is_explicit() {
    // The Fusebox header and its pre-rename name both end the session.
    for name in ["x-fusebox-session-end", "x-cliproxy-session-end"] {
        let fixture = Fixture::new(Routing::RoundRobin, false).await;
        assert_eq!(answer(&fixture.request(Some("task"), prompt()).await.1), "a");
        let response = fixture.request_with(&[("thread-id", "task"), (name, "true")], prompt()).await;
        assert_eq!(answer(&response), "a", "{name}");
        assert_eq!(answer(&fixture.request(Some("task"), prompt()).await.1), "b", "{name}");
    }
    let fixture = Fixture::new(Routing::RoundRobin, true).await;
    let (status, response) = fixture
        .request(
            Some("task"),
            json!({"model":"gpt-6.1-sol","input":"next","previous_response_id":"from-before-restart"}),
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(response["error"]["code"], "previous_response_not_found");
    assert!(fixture.mock.calls.lock().is_empty());
}

#[tokio::test]
async fn diagnostics_distinguish_agent_threads_and_missing_or_disabled_affinity() {
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    let threads = ["private-parent-thread", "private-agent-one-thread", "private-agent-two-thread"];
    for thread in threads.into_iter().chain([threads[0]]) {
        assert_eq!(fixture.request(Some(thread), prompt()).await.0, 200);
    }
    let logs = fixture.logs(4).await;
    let sessions: std::collections::HashSet<_> = logs[..3].iter().map(|log| log.session_id.clone()).collect();
    assert_eq!(sessions.len(), 3);
    assert!(!sessions.contains(&None));
    assert_eq!(logs[0].session_id, logs[3].session_id);
    assert_eq!(logs[0].account, logs[3].account);
    assert_eq!(logs[3].routing_reason, Some("session_reused"));
    let serialized = serde_json::to_string(&logs).unwrap();
    assert!(threads.iter().all(|thread| !serialized.contains(*thread)));
    assert!(!serialized.contains("client-one"));

    // Without a session id, a conversation keeps its account by its first message;
    // a conversation that starts differently is assigned on its own.
    let second_turn = json!({"model":"gpt-6.1-sol","input":[
        {"role":"user","content":"question"},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"a"}]},
        {"role":"user","content":"follow-up"}
    ]});
    for body in [prompt(), second_turn, json!({"model":"gpt-6.1-sol","input":"another question"})] {
        assert_eq!(fixture.request(None, body).await.0, 200);
    }
    let logs = fixture.logs(7).await;
    assert_eq!(logs[4].session_id, logs[5].session_id);
    assert_eq!(logs[4].account, logs[5].account);
    assert_eq!((logs[4].routing_reason, logs[5].routing_reason), (Some("new_session"), Some("session_reused")));
    assert_ne!(logs[6].session_id, logs[4].session_id);
    assert_eq!(logs[6].routing_reason, Some("new_session"));
    for log in &logs[4..] {
        assert_eq!(log.session_source, Some(crate::affinity::INFERRED_SOURCE));
        assert_eq!(log.routing_warning, Some("inferred_session"));
    }
    assert!(!serde_json::to_string(&logs).unwrap().contains("another question"));

    let mut cfg = fixture.cfg.clone();
    cfg.session_affinity = false;
    fixture.app.set_config(cfg);
    for _ in 0..2 {
        assert_eq!(fixture.request(Some(threads[0]), prompt()).await.0, 200);
    }
    let logs = fixture.logs(9).await;
    assert_ne!(logs[7].account, logs[8].account);
    for log in &logs[7..] {
        assert_eq!(log.session_id, logs[0].session_id);
        assert_eq!(log.session_source, Some("thread-id"));
        assert_eq!(log.routing_reason, Some("affinity_disabled"));
        assert_eq!(log.routing_warning, Some("affinity_disabled"));
        assert_eq!(log.routing_strategy, Routing::RoundRobin);
    }
}

#[tokio::test]
async fn websocket_without_client_identity_warns_that_affinity_is_connection_only() {
    for native in [false, true] {
        let fixture = Fixture::new(Routing::RoundRobin, native).await;
        let mut socket = fixture.socket_with_task(None).await;
        turn(&mut socket, prompt()).await;
        turn(&mut socket, prompt()).await;
        socket.close(None).await.unwrap();
        let mut socket = fixture.socket_with_task(None).await;
        turn(&mut socket, prompt()).await;
        let logs = fixture.logs(3).await;
        assert_eq!(logs[0].session_id, logs[1].session_id);
        assert_eq!(logs[0].account, logs[1].account);
        assert_ne!(logs[0].session_id, logs[2].session_id);
        assert_ne!(logs[0].account, logs[2].account);
        for log in &logs {
            assert!(log.session_id.is_some());
            assert_eq!(log.session_source, Some("websocket_connection"));
            assert_eq!(log.routing_warning, Some("connection_only"));
        }
        assert_eq!(logs[0].routing_reason, Some("new_session"));
        assert_eq!(logs[1].routing_reason, Some("session_reused"));
        assert_eq!(logs[2].routing_reason, Some("new_session"));
    }
}

#[tokio::test]
async fn websocket_native_selection_preserves_decision_when_api_keys_require_http_fallback() {
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    let mut cfg = fixture.cfg.clone();
    cfg.codex_websockets = true;
    fixture.app.set_config(cfg);
    let mut socket = fixture.socket("fallback-task").await;
    turn(&mut socket, prompt()).await;
    turn(&mut socket, prompt()).await;
    let logs = fixture.logs(2).await;
    assert_eq!(logs[0].routing_reason, Some("new_session"));
    assert_eq!(logs[1].routing_reason, Some("session_reused"));
    let previous = logs[0].routing_attempts[0].account_id.clone();
    fixture.app.pool.get(&previous).unwrap().exhaust(
        "gpt-6.1-sol",
        chrono::Utc::now() + chrono::Duration::minutes(5),
        "mock observed quota exhaustion",
    );
    turn(&mut socket, prompt()).await;
    turn(&mut socket, prompt()).await;
    let logs = fixture.logs(4).await;
    assert_eq!(logs[2].routing_reason, Some("quota_exhausted"));
    assert_eq!(logs[2].routing_attempts[0].previous_account.as_deref(), Some(previous.as_str()));
    assert_ne!(logs[2].routing_attempts[0].account_id, previous);
    assert_eq!(logs[3].routing_reason, Some("session_reused"));
    assert!(logs[3].routing_attempts[0].previous_account.is_none());
    for log in &logs {
        assert_eq!(log.session_id, logs[0].session_id);
        assert_eq!(log.attempts, 1);
        assert_eq!(log.routing_attempts.len(), 1);
    }
    let calls = fixture.mock.calls.lock();
    assert_eq!(calls.len(), 4);
    assert!(calls.iter().all(|(_, _, transport)| *transport == "http"));
}

#[tokio::test]
async fn websocket_turn_without_its_original_body_identifier_reports_connection_only() {
    for native in [false, true] {
        let fixture = Fixture::new(Routing::RoundRobin, native).await;
        let mut socket = fixture.socket_with_task(None).await;
        let mut first = prompt();
        first["prompt_cache_key"] = "private-first-frame-session-key".into();
        turn(&mut socket, first).await;
        turn(&mut socket, prompt()).await;
        let logs = fixture.logs(2).await;
        assert_eq!(logs[0].session_source, Some("prompt_cache_key"));
        assert_eq!(logs[0].routing_warning, None);
        assert_eq!(logs[1].session_source, Some("websocket_connection"));
        assert_eq!(logs[1].routing_warning, Some("connection_only"));
        assert_eq!(logs[1].routing_reason, Some("session_reused"));
        assert_eq!(logs[0].session_id, logs[1].session_id);
        assert_eq!(logs[0].account, logs[1].account);
        assert!(!serde_json::to_string(&logs).unwrap().contains("private-first-frame-session-key"));
    }
}

#[tokio::test]
async fn translated_responses_preserve_cache_controls_and_reject_unsupported_prewarming() {
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    let mut cfg = fixture.cfg.clone();
    cfg.codex_api_key.clear();
    cfg.openai_compatibility = vec![crate::config::CompatEntry {
        name: "local-chat".into(),
        base_url: format!("{}/v1", fixture.provider.url),
        api_keys: vec!["a".into()],
        models: vec![crate::config::ModelAlias { name: "cache-compat-model".into(), alias: None }],
        ..Default::default()
    }];
    fixture.app.set_config(cfg);
    let mut body = json!({
        "model":"cache-compat-model",
        "prompt_cache_key":"private-cache-key",
        "prompt_cache_retention":"24h",
        "prompt_cache_options":{"mode":"explicit","ttl":"30m"},
        "input":[
            {"role":"system","content":[
                {"type":"input_text","text":"stable instructions","prompt_cache_breakpoint":{"mode":"explicit"}},
                {"type":"input_text","text":"variable instructions"}
            ]},
            {"role":"user","content":[
                {"type":"input_text","text":"stable context","prompt_cache_breakpoint":{"mode":"explicit"}},
                {"type":"input_text","text":"changing question"}
            ]}
        ]
    });
    let (status, response) = fixture.request(Some("cache-task"), body.clone()).await;
    assert_eq!(status, 200, "{response}");
    assert_eq!(answer(&response), "a");
    {
        let calls = fixture.mock.calls.lock();
        assert_eq!(calls.len(), 1);
        let (_, sent, transport) = &calls[0];
        assert_eq!(*transport, "chat");
        for field in ["prompt_cache_key", "prompt_cache_options", "prompt_cache_retention"] {
            assert_eq!(sent[field], body[field]);
        }
        let messages = sent["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        for (message, original) in messages.iter().zip(body["input"].as_array().unwrap()) {
            assert_eq!(message["role"], original["role"]);
            assert_eq!(message["content"][0]["text"], original["content"][0]["text"]);
            assert_eq!(message["content"][0]["prompt_cache_breakpoint"], json!({"mode":"explicit"}));
            assert_eq!(message["content"][1]["text"], original["content"][1]["text"]);
            assert!(message["content"][1]["prompt_cache_breakpoint"].is_null());
        }
    }
    body["prompt_cache_options"]["prewarm"] = true.into();
    let (status, response) = fixture.request(Some("cache-task"), body).await;
    assert_eq!(status, 400, "{response}");
    assert!(response["error"]["message"].as_str().unwrap().contains("prewarming"));
    assert_eq!(fixture.mock.calls.lock().len(), 1, "unsupported cache controls reached the upstream");
}

#[tokio::test]
async fn openai_backends_get_a_session_cache_key_and_custom_endpoints_do_not() {
    // ChatGPT subscriptions: OpenAI's own backend, here at a local address.
    let fixture = Fixture::new(Routing::RoundRobin, true).await;
    let claude = |messages: Value| json!({"model":"gpt-6.1-sol","max_tokens":32,"messages":messages});
    let turns = [
        claude(json!([{"role":"user","content":"translate this task"}])),
        claude(json!([
            {"role":"user","content":"translate this task"},
            {"role":"assistant","content":"done"},
            {"role":"user","content":"and the next one"}
        ])),
    ];
    for body in turns {
        let response = reqwest::Client::new()
            .post(format!("{}/v1/messages", fixture.proxy.url))
            .header("x-api-key", "client-one")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }
    let sent_key = |i: usize| fixture.mock.calls.lock()[i].1["prompt_cache_key"].clone();
    // Both turns of the translated conversation carry the same key, shaped like Codex's own.
    let key = sent_key(0);
    assert_eq!(key, sent_key(1));
    assert_eq!(key.as_str().unwrap().split('-').count(), 5);
    let accounts: Vec<_> = fixture.mock.calls.lock().iter().map(|(a, _, _)| a.clone()).collect();
    assert_eq!(accounts[0], accounts[1]);
    // A key the client chose is passed on unchanged; another session gets its own.
    fixture.request(Some("task"), json!({"model":"gpt-6.1-sol","input":"q","prompt_cache_key":"client-key"})).await;
    assert_eq!(sent_key(2), "client-key");
    fixture.request(Some("task"), prompt()).await;
    assert!(sent_key(3).is_string() && sent_key(3) != key && sent_key(3) != "client-key");

    // API keys on a custom endpoint may not accept the field: it is not added there.
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    assert_eq!(fixture.request(Some("task"), prompt()).await.0, 200);
    assert!(fixture.mock.calls.lock()[0].1.get("prompt_cache_key").is_none());
}

#[tokio::test]
async fn soft_retries_preserve_the_temporary_detour_account() {
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    let mut cfg = fixture.cfg.clone();
    cfg.codex_api_key.push(KeyEntry {
        api_key: "c".into(),
        label: Some("c".into()),
        base_url: Some(format!("{}/v1", fixture.provider.url)),
        ..Default::default()
    });
    fixture.app.set_config(cfg);
    assert_eq!(answer(&fixture.request(Some("detour-task"), prompt()).await.1), "a");
    let assigned = fixture.app.pool.all().into_iter().find(|a| a.label == "a").unwrap();
    assigned.cool(Some("gpt-6.1-sol"), chrono::Utc::now() + chrono::Duration::minutes(5), "busy");
    fixture.mock.mode.store(7, Ordering::Relaxed);
    assert_eq!(answer(&fixture.request(Some("detour-task"), prompt()).await.1), "c");
    assert_eq!(fixture.mock.calls.lock().iter().map(|(a, _, _)| a.as_str()).collect::<Vec<_>>(), vec!["a", "c", "c"]);
    let logs = fixture.logs(2).await;
    assert_eq!(logs[1].routing_attempts[0].reason, "temporary_detour");
    assert_eq!(logs[1].routing_attempts[1].reason, "retry_same");
    fixture.recover();
    assert_eq!(answer(&fixture.request(Some("detour-task"), prompt()).await.1), "a");
}

#[tokio::test]
async fn codex_cache_key_omission_and_explicit_keys_survive_http_and_native_websockets() {
    let fixture = Fixture::new(Routing::RoundRobin, true).await;
    let mut cfg = fixture.cfg.clone();
    // A shared policy would otherwise supply a key even when the client omitted it.
    cfg.prompt_cache.scope = crate::cache::Scope::Client;
    fixture.app.set_config(cfg);
    let headers =
        [("user-agent", "codex_cli_rs/0.164.0"), ("originator", "codex_cli_rs"), ("session-id", "native-session")];
    let mut request =
        format!("{}/v1/responses", fixture.proxy.url.replace("http://", "ws://")).into_client_request().unwrap();
    request.headers_mut().insert("authorization", "Bearer client-one".parse().unwrap());
    for (name, value) in headers {
        request.headers_mut().insert(name, value.parse().unwrap());
    }
    let mut socket = tokio_tungstenite::connect_async(request).await.unwrap().0;
    let mut omitted = prompt();
    let mut null = prompt();
    null["prompt_cache_key"] = Value::Null;
    let mut explicit = prompt();
    explicit["prompt_cache_key"] = "caller-choice".into();
    // Client metadata must also survive passthrough when the cache key is absent.
    omitted["client_metadata"] = json!({"turn":"native"});
    for body in [omitted, null, explicit] {
        fixture.request_with(&headers, body.clone()).await;
        turn(&mut socket, body.clone()).await;
        let calls = fixture.mock.calls.lock();
        for (_, upstream, transport) in &calls[calls.len() - 2..] {
            assert_eq!(upstream.get("prompt_cache_key"), body.get("prompt_cache_key"), "{transport}");
            assert_eq!(upstream.get("client_metadata"), body.get("client_metadata"), "{transport}");
        }
    }
    let calls = fixture.mock.calls.lock().clone();
    assert_eq!(calls.iter().filter(|(_, _, transport)| *transport == "ws").count(), 3);
    socket.close(None).await.unwrap();
}

#[tokio::test]
async fn shared_cache_scope_matches_http_and_native_websockets_without_changing_affinity() {
    let fixture = Fixture::new(Routing::RoundRobin, true).await;
    let mut cfg = fixture.cfg.clone();
    cfg.prompt_cache.scope = crate::cache::Scope::Client;
    fixture.app.set_config(cfg.clone());
    assert_eq!(fixture.request(Some("http-task"), prompt()).await.0, 200);
    let mut socket = fixture.socket("ws-task").await;
    turn(&mut socket, prompt()).await;
    let calls = fixture.mock.calls.lock().clone();
    assert_eq!(calls[0].1["prompt_cache_key"], calls[1].1["prompt_cache_key"]);
    assert_ne!(calls[0].0, calls[1].0, "separate sessions must still select accounts independently");
    let logs = fixture.logs(2).await;
    assert!(logs.iter().all(|log| log.cache_policy.as_ref().unwrap().key_scope == "client"));
    let mut explicit = prompt();
    explicit["prompt_cache_key"] = "caller-choice".into();
    turn(&mut socket, explicit).await;
    assert_eq!(fixture.mock.calls.lock().last().unwrap().1["prompt_cache_key"], "caller-choice");
    assert_eq!(fixture.logs(3).await[2].cache_policy.as_ref().unwrap().key_scope, "caller");
    socket.close(None).await.unwrap();

    cfg.prompt_cache.scope = crate::cache::Scope::Workspace;
    fixture.app.set_config(cfg);
    for session in ["workspace-one", "workspace-two"] {
        fixture.request_with(&[("thread-id", session), ("x-fusebox-cache-workspace", "repo")], prompt()).await;
    }
    let calls = fixture.mock.calls.lock();
    assert_eq!(calls[3].1["prompt_cache_key"], calls[4].1["prompt_cache_key"]);
    assert_ne!(calls[0].1["prompt_cache_key"], calls[3].1["prompt_cache_key"]);
}

#[tokio::test]
async fn incompatible_cache_policy_is_observable_and_automatic_is_opt_in() {
    use crate::cache::Incompatible;
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    let mut cfg = fixture.cfg.clone();
    cfg.codex_api_key.clear();
    cfg.claude_api_key =
        vec![KeyEntry { api_key: "a".into(), base_url: Some(fixture.provider.url.clone()), ..Default::default() }];
    let explicit = json!({"model":"claude-sonnet-4-6", "input":[{"role":"user", "content":[{"type":"input_text", "text":"stable", "prompt_cache_breakpoint":{"mode":"explicit"}}]}], "prompt_cache_options":{"mode":"explicit"}});
    for (i, handling) in [Incompatible::Drop, Incompatible::Automatic, Incompatible::Reject].into_iter().enumerate() {
        cfg.prompt_cache.on_incompatible = handling;
        fixture.app.set_config(cfg.clone());
        let (status, response) = fixture.request(Some("cache-task"), explicit.clone()).await;
        assert_eq!(status, if handling == Incompatible::Reject { 400 } else { 200 }, "{response}");
        let logs = fixture.logs(i + 1).await;
        let policy = logs[i].cache_policy.as_ref().unwrap();
        assert_eq!(policy.requested, "explicit");
        assert_eq!(
            policy.applied,
            match handling {
                Incompatible::Drop => "none",
                Incompatible::Automatic => "automatic",
                Incompatible::Reject => "rejected",
            }
        );
        assert!(policy.downgrade_reason.as_ref().unwrap().contains("cannot be translated"));
    }
    assert_eq!(fixture.mock.calls.lock().len(), 2);
    assert!(fixture.mock.calls.lock()[0].1["cache_control"].is_null());
    assert_eq!(fixture.mock.calls.lock()[1].1["cache_control"], json!({"type":"ephemeral"}));
    cfg.prompt_cache.on_incompatible = Incompatible::Automatic;
    fixture.app.set_config(cfg);
    let mut no_markers = explicit;
    no_markers["input"][0]["content"][0].as_object_mut().unwrap().remove("prompt_cache_breakpoint");
    assert_eq!(fixture.request(Some("cache-task"), no_markers).await.0, 200);
    assert!(fixture.mock.calls.lock().last().unwrap().1["cache_control"].is_null());
}

#[tokio::test]
async fn claude_budget_errors_are_400_and_small_valid_caps_reach_upstream_unchanged() {
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    let mut cfg = fixture.cfg.clone();
    cfg.claude_api_key = vec![KeyEntry {
        api_key: "claude-test".into(),
        base_url: Some(fixture.provider.url.clone()),
        ..Default::default()
    }];
    fixture.app.set_config(cfg);
    for native in [false, true] {
        for cap in [64, 1024, 1025, 1500, 2047, 2048] {
            let response = if native {
                reqwest::Client::new()
                    .post(format!("{}/v1/messages", fixture.proxy.url))
                    .bearer_auth("client-one")
                    .json(&json!({"model":"claude-sonnet-4-5(low)","max_tokens":cap,
                        "messages":[{"role":"user","content":"hello"}]}))
                    .send()
                    .await
                    .unwrap()
            } else {
                reqwest::Client::new()
                    .post(format!("{}/v1/responses", fixture.proxy.url))
                    .bearer_auth("client-one")
                    .json(&json!({"model":"claude-sonnet-4-5","reasoning":{"effort":"low"},
                        "max_output_tokens":cap,"input":"hello"}))
                    .send()
                    .await
                    .unwrap()
            };
            let status = response.status().as_u16();
            let body: Value = response.json().await.unwrap();
            assert_eq!(status, if cap <= 1024 { 400 } else { 200 }, "native={native} cap={cap}: {body}");
            if cap > 1024 {
                let calls = fixture.mock.calls.lock();
                let sent = &calls.last().unwrap().1;
                assert_eq!(sent["max_tokens"], cap);
                assert!(sent["thinking"]["budget_tokens"].as_u64().unwrap() < cap);
                if !native {
                    assert_eq!(sent["cache_control"], json!({"type":"ephemeral"}));
                }
            }
        }
    }
    assert_eq!(fixture.mock.calls.lock().len(), 8, "invalid budgets must not reach the provider");
}

#[tokio::test]
async fn token_count_fallback_marks_estimates_and_ignores_base64_size() {
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/messages/count_tokens", fixture.proxy.url))
        .bearer_auth("client-one")
        .json(&json!({"model":"gpt-6.1-sol","messages":[{"role":"user","content":[
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":"A".repeat(100_000)}}
        ]}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-fusebox-token-count-estimated"], "true");
    let count: Value = response.json().await.unwrap();
    assert!(count["input_tokens"].as_u64().unwrap() < 10_000);
    assert!(fixture.mock.calls.lock().is_empty());
    let response = reqwest::Client::new()
        .post(format!("{}/v1beta/models/gemini-3.8-flash:countTokens", fixture.proxy.url))
        .bearer_auth("client-one")
        .json(&json!({"contents":[{"parts":[{"text":"hello"}]}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-fusebox-token-count-estimated"], "true");
    assert!(response.json::<Value>().await.unwrap()["totalTokens"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn translated_cache_intent_survives_oauth_cloak_for_each_incompatible_policy() {
    use crate::cache::Incompatible;
    let fixture = Fixture::new(Routing::RoundRobin, false).await;
    let mut cfg = fixture.cfg.clone();
    cfg.codex_api_key.clear();
    cfg.claude_api_key =
        vec![KeyEntry { api_key: "a".into(), base_url: Some(fixture.provider.url.clone()), ..Default::default() }];
    let mut logs_count = 0;
    for handling in [Incompatible::Drop, Incompatible::Automatic, Incompatible::Reject] {
        for markers in [false, true] {
            cfg.prompt_cache.on_incompatible = handling;
            fixture.app.set_config(cfg.clone());
            let acct =
                fixture.app.pool.all().into_iter().find(|a| a.provider == crate::accounts::Provider::Claude).unwrap();
            *acct.cred.write() = crate::accounts::Credential::OAuth(crate::accounts::OAuth {
                access_token: "a".into(),
                refresh_token: "unused".into(),
                expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                base_url: Some(fixture.provider.url.clone()),
                email: None,
                account_id: None,
                project_id: None,
                raw: Default::default(),
            });
            acct.state.lock().credential_identity = acct.cred.read().identity(acct.provider);
            let mut body = json!({"model":"claude-sonnet-5-5","prompt_cache_options":{"mode":"explicit"},
                "input":[{"role":"user","content":[{"type":"input_text","text":"q"}]}]});
            if markers {
                body["input"][0]["content"][0]["prompt_cache_breakpoint"] = json!({"mode":"explicit"});
            }
            let (status, response) = fixture.request(Some("oauth-cache-test"), body).await;
            assert_eq!(status, if handling == Incompatible::Reject { 400 } else { 200 }, "{response}");
            logs_count += 1;
            let logs = fixture.logs(logs_count).await;
            let expected = if handling == Incompatible::Reject {
                "rejected"
            } else if handling == Incompatible::Automatic && markers {
                "automatic"
            } else {
                "none"
            };
            assert_eq!(logs.last().unwrap().cache_policy.as_ref().unwrap().applied, expected);
            if handling != Incompatible::Reject {
                let sent = fixture.mock.calls.lock().last().unwrap().1.clone();
                assert_eq!(crate::cache::has_claude_controls(&sent), handling == Incompatible::Automatic && markers);
            }
        }
    }
}

#[tokio::test]
async fn provider_sessions_rotate_on_subscription_switch_and_return_without_losing_http_history() {
    let fixture = Fixture::new(Routing::FillFirst, true).await;
    let first = fixture.request(Some("chat-42"), prompt()).await.1;
    assert_eq!(answer(&first), "a");
    fixture.mock.mode.store(2, Ordering::Relaxed); // temporary rate limit, preserving the original assignment
    let second = fixture
        .request(
            Some("chat-42"),
            json!({"model":"gpt-6.1-sol","previous_response_id":first["id"],"input":[
                {"type":"function_call","call_id":"call_42","name":"lookup","arguments":"{}"},
                {"type":"function_call_output","call_id":"call_42","output":"retained tool result"}
            ]}),
        )
        .await
        .1;
    assert_eq!(answer(&second), "b");
    fixture.recover();
    let third = fixture
        .request(Some("chat-42"), json!({"model":"gpt-6.1-sol","previous_response_id":second["id"],"input":"continue"}))
        .await
        .1;
    assert_eq!(answer(&third), "a");
    let headers = fixture.mock.headers.lock();
    let sessions: Vec<_> = headers
        .iter()
        .map(|(account, h, _)| (account.as_str(), h.get("session-id").unwrap().to_str().unwrap()))
        .collect();
    assert_eq!(sessions.iter().map(|(a, _)| *a).collect::<Vec<_>>(), ["a", "a", "b", "a"]);
    assert_eq!(sessions[0].1, sessions[1].1);
    assert_ne!(sessions[0].1, sessions[2].1);
    assert_ne!(sessions[0].1, sessions[3].1);
    assert_ne!(sessions[2].1, sessions[3].1);
    let calls = fixture.mock.calls.lock();
    let last = &calls.last().unwrap().1;
    assert!(last.get("previous_response_id").is_none());
    assert!(
        last["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["type"] == "function_call_output" && v["output"] == "retained tool result")
    );
    assert!(
        last["input"].as_array().unwrap().iter().any(|v| v["role"] == "user" && v["content"][0]["text"] == "question")
    );
}

#[tokio::test]
async fn provider_session_rotation_reconnects_native_ws_and_replays_tools_on_account_return() {
    let fixture = Fixture::new(Routing::FillFirst, true).await;
    fixture.mock.ws_rich_output.store(true, Ordering::Relaxed);
    let mut socket = fixture.socket("chat-42").await;
    let first = turn(&mut socket, prompt()).await;
    fixture.mock.mode.store(1, Ordering::Relaxed);
    let second = turn(&mut socket,json!({"model":"gpt-6.1-sol","previous_response_id":first["id"],"input":[{"type":"function_call_output","call_id":"call_local","output":"result before failover"}]})).await;
    assert!(second["id"].is_string());
    assert_eq!(fixture.mock.calls.lock().last().unwrap().0, "b");
    fixture.recover();
    let replacement = fixture.app.pool.all().into_iter().find(|a| a.label == "b").unwrap();
    replacement.state.lock().disabled = true;
    let third = turn(
        &mut socket,
        json!({"model":"gpt-6.1-sol","previous_response_id":second["id"],"input":"continue after returning"}),
    )
    .await;
    assert!(third["id"].is_string());
    assert_eq!(fixture.mock.calls.lock().last().unwrap().0, "a");
    let captures = fixture.mock.headers.lock();
    let original = captures.iter().find(|(account, _, kind)| account == "a" && *kind == "ws").unwrap();
    let returned = captures.iter().rev().find(|(account, _, kind)| account == "a" && *kind == "ws").unwrap();
    assert_ne!(original.1.get("session-id"), returned.1.get("session-id"));
    let calls = fixture.mock.calls.lock();
    let body = &calls.last().unwrap().1;
    assert!(body.get("previous_response_id").is_none());
    assert!(
        body["input"].as_array().unwrap().iter().any(|v| v["type"] == "function_call" && v["call_id"] == "call_local")
    );
    assert!(
        body["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["type"] == "function_call_output" && v["output"] == "result before failover")
    );
}

#[tokio::test]
async fn residency_is_preserved_on_http_and_ws_and_invalid_requirements_never_reach_upstream() {
    let fixture = Fixture::new(Routing::FillFirst, true).await;
    let mut cfg = fixture.cfg.clone();
    cfg.codex_api_key.push(KeyEntry {
        api_key: "ineligible-api-key".into(),
        base_url: Some(format!("{}/v1", fixture.provider.url)),
        priority: 100,
        ..Default::default()
    });
    fixture.app.set_config(cfg);
    let body = json!({"model":"gpt-6.1-sol","input":"question","stream_options":{"reasoning_summary_delivery":"sequential_cutoff","include_usage":true}});
    let response = fixture
        .request_with(&[("thread-id", "residency-http"), ("x-openai-internal-codex-residency", "us")], body.clone())
        .await;
    assert_eq!(answer(&response), "a");
    // Exercise the native socket too, independently of the API-key fallback above.
    fixture.app.set_config(fixture.cfg.clone());
    let mut request =
        format!("{}/v1/responses", fixture.proxy.url.replacen("http://", "ws://", 1)).into_client_request().unwrap();
    request.headers_mut().insert("authorization", "Bearer client-one".parse().unwrap());
    request.headers_mut().insert("x-openai-internal-codex-residency", "us".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    turn(&mut socket, body).await;
    for (_, headers, _) in fixture.mock.headers.lock().iter() {
        assert_eq!(headers.get("x-openai-internal-codex-residency").unwrap(), "us");
    }
    assert!(fixture.mock.headers.lock().iter().any(|(_, _, kind)| *kind == "ws"));
    for (_, body, _) in fixture.mock.calls.lock().iter() {
        assert_eq!(body["stream_options"], json!({"reasoning_summary_delivery":"sequential_cutoff"}));
    }
    let before = fixture.mock.calls.lock().len();
    let rejected = fixture
        .request_with(
            &[("x-openai-internal-codex-residency", "us"), ("connection", "x-openai-internal-codex-residency")],
            prompt(),
        )
        .await;
    assert!(rejected["error"].is_object(), "{rejected}");
    assert_eq!(fixture.mock.calls.lock().len(), before);
    let api_key = Fixture::new(Routing::FillFirst, false).await;
    let rejected = api_key.request_with(&[("x-openai-internal-codex-residency", "us")], prompt()).await;
    assert!(rejected["error"].is_object());
    assert!(api_key.mock.calls.lock().is_empty());
}

#[tokio::test]
async fn claude_native_and_emulated_requests_bind_selected_identity_and_filter_gateway_headers() {
    let fixture = Fixture::new(Routing::FillFirst, true).await;
    for name in ["a", "b"] {
        std::fs::write(fixture.directory.join(format!("{name}.json")),json!({"type":"claude","access_token":name,"account_id":format!("account-{name}"),"email":name,"base_url":format!("{}/v1",fixture.provider.url)}).to_string()).unwrap();
    }
    // Claude endpoints use /v1/messages, so use the mock origin without /v1 here.
    for name in ["a", "b"] {
        let path = fixture.directory.join(format!("{name}.json"));
        let mut cred: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        cred["base_url"] = fixture.provider.url.clone().into();
        std::fs::write(path, cred.to_string()).unwrap();
    }
    fixture.app.reload_accounts();
    for (conversation, ua) in [("native", "claude-cli/2.1.309 (external, cli)"), ("translated", "example-client/1.0.0")]
    {
        let response = reqwest::Client::new().post(format!("{}/v1/messages",fixture.proxy.url)).bearer_auth("client-one")
            .header("user-agent",ua).header("x-claude-code-session-id",conversation)
            .header("via","1.1 private-gateway").header("x-gateway-key","FAKE_GATEWAY_KEY")
            .header("x-auth-request-email","private@example.invalid").header("cf-connecting-ip","192.0.2.42")
            .header("connection","x-private-hop").header("x-private-hop","PRIVATE_HOP")
            .header("anthropic-beta","context-management-2025-06-27")
            .json(&json!({"model":"claude-sonnet-4-6","max_tokens":8,"messages":[{"role":"user","content":"question"}],"metadata":{"user_id":json!({"device_id":"a".repeat(64),"account_uuid":"wrong-account","session_id":"wrong-session","private":"marker"}).to_string()}}))
            .send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{}", response.text().await.unwrap());
    }
    let captures = fixture.mock.headers.lock();
    let calls = fixture.mock.calls.lock();
    for ((account, headers, _), (_, body, _)) in captures.iter().zip(calls.iter()) {
        for name in ["via", "x-gateway-key", "x-auth-request-email", "cf-connecting-ip", "x-private-hop"] {
            assert!(headers.get(name).is_none(), "{name}");
        }
        assert!(headers.get("anthropic-beta").unwrap().to_str().unwrap().contains("context-management-2025-06-27"));
        let user: Value = serde_json::from_str(body["metadata"]["user_id"].as_str().unwrap()).unwrap();
        assert_eq!(user["account_uuid"], format!("account-{account}"));
        assert_eq!(
            user["session_id"].as_str().unwrap(),
            headers.get("x-claude-code-session-id").unwrap().to_str().unwrap()
        );
        assert_ne!(user["device_id"], "a".repeat(64));
        assert!(user.get("private").is_none());
    }
    assert_ne!(captures[0].1.get("x-claude-code-session-id"), captures[1].1.get("x-claude-code-session-id"));
}

#[tokio::test]
async fn identity_learning_requires_authenticated_opt_in_and_management_reset_is_protected() {
    let fixture = Fixture::new(Routing::FillFirst, true).await;
    let mut cfg = fixture.cfg.clone();
    cfg.named_clients = vec![crate::config::NamedClient {
        id: "trusted-laptop".into(),
        label: "Laptop".into(),
        key: "trusted-key".into(),
        trust_client_identity: true,
        ..Default::default()
    }];
    cfg.management_key = "fake-management-key".into();
    fixture.app.set_config(cfg);
    let request = |key: &str| {
        reqwest::Client::new()
            .post(format!("{}/v1/responses", fixture.proxy.url))
            .bearer_auth(key)
            .header("x-fusebox-usage-client", "trusted-laptop")
            .header("user-agent", crate::upstream::CC_USER_AGENT)
            .header("x-stainless-package-version", "0.128.0")
            .header("x-stainless-os", "Linux")
            .header("x-stainless-arch", "x64")
            .header("x-stainless-runtime", "node")
            .header("x-stainless-runtime-version", "v26.3.0")
            .json(&prompt())
    };
    crate::clients::reset();
    assert_eq!(request("client-one").send().await.unwrap().status(), StatusCode::OK);
    assert!(crate::clients::seen().is_empty(), "shared key cannot spoof trusted client identity");
    assert_eq!(request("trusted-key").send().await.unwrap().status(), StatusCode::OK);
    assert!(!crate::clients::seen().is_empty());
    let client = reqwest::Client::new();
    let url = format!("{}/api/client-identities/reset", fixture.proxy.url);
    assert_eq!(client.post(&url).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
    assert!(!crate::clients::seen().is_empty());
    assert_eq!(client.post(&url).bearer_auth("fake-management-key").send().await.unwrap().status(), StatusCode::OK);
    assert!(crate::clients::seen().is_empty());
}

#[tokio::test]
async fn oauth_error_markers_do_not_escape_to_downstream_status_or_request_logs() {
    let fixture = Fixture::new(Routing::FillFirst, true).await;
    let provider = serve(axum::Router::new().route("/token",post(||async {
        (StatusCode::BAD_REQUEST,Json(json!({"error":"invalid_grant","error_description":"FAKE_CREDENTIAL_MARKER","refresh_token":"FAKE_CREDENTIAL_MARKER"})))
    }))).await;
    std::fs::write(fixture.directory.join("xai.json"),json!({"type":"xai","access_token":"FAKE_ACCESS","refresh_token":"FAKE_REFRESH","expired":"2000-01-01T00:00:00Z","token_endpoint":format!("{}/token",provider.url),"base_url":provider.url}).to_string()).unwrap();
    fixture.app.reload_accounts();
    let response = fixture.request(Some("refresh-error"), json!({"model":"grok-4.20","input":"hello"})).await;
    assert_eq!(response.0, 401, "{}", response.1);
    let accounts = reqwest::get(format!("{}/api/accounts", fixture.proxy.url)).await.unwrap().text().await.unwrap();
    let logs = serde_json::to_string(&fixture.logs(1).await).unwrap();
    for text in [response.1.to_string(), accounts, logs] {
        for marker in ["FAKE_CREDENTIAL_MARKER", "FAKE_REFRESH", "FAKE_ACCESS"] {
            assert!(!text.contains(marker), "{text}");
        }
    }
}
