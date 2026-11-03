use super::*;
use crate::accounts::{Credential, OAuth};
use crate::config::{Config, KeyEntry};
use axum::body::{Body, Bytes};
use axum::routing::post;
use std::convert::Infallible;
use std::sync::atomic::Ordering;
use tokio::sync::{Notify, mpsc};

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

fn app(base: &str, native: bool) -> Arc<App> {
    let key = KeyEntry { api_key: "loopback-only".into(), base_url: Some(base.into()), ..Default::default() };
    let cfg = Config {
        auth_dir: "/nonexistent".into(),
        codex_api_key: if native { vec![key.clone()] } else { vec![] },
        claude_api_key: if native { vec![] } else { vec![key] },
        ..Default::default()
    };
    let app = App::new(cfg, "/nonexistent/config.yaml".into());
    if native {
        *app.pool.all()[0].cred.write() = Credential::OAuth(OAuth {
            access_token: "loopback-only".into(),
            base_url: Some(base.into()),
            ..Default::default()
        });
        let account = &app.pool.all()[0];
        account.state.lock().credential_identity = account.cred.read().identity(account.provider);
    }
    app
}

async fn client(server: &Server) -> Upstream {
    tokio_tungstenite::connect_async(format!("{}/v1/responses", server.url.replace("http://", "ws://")))
        .await
        .unwrap()
        .0
}

async fn create(client: &mut Upstream, native: bool, input: &str) {
    client
        .send(tungstenite::Message::Text(
            json!({
                "type":"response.create", "model":if native {"gpt-6.1-sol"} else {"claude-sonnet-4-6"}, "input":input
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
}

async fn until_event(client: &mut Upstream, kind: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let message = client.next().await.unwrap().unwrap();
            if let tungstenite::Message::Text(text) = message {
                let value: Value = serde_json::from_str(&text).unwrap();
                if value["type"] == kind {
                    return value;
                }
            }
        }
    })
    .await
    .expect("expected downstream event")
}

async fn cancelled(app: &App, input: u64, cache: u64) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(log) = app.stats.recent.lock().front().cloned() {
                assert_eq!(log.status, 499);
                assert!(matches!(log.failure_kind, Some("downstream_disconnect" | "downstream_read_error")));
                assert_eq!(log.usage_completeness, if input > 0 { "partial" } else { "missing" });
                assert_eq!((log.input_tokens, log.cache_tokens), (input, cache));
                assert_eq!(app.stats.active.load(Ordering::Relaxed), 0);
                assert_eq!(app.pool.all()[0].state.lock().active_requests.load(Ordering::Relaxed), 0);
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("disconnected client must cancel a silent turn promptly");
}

#[tokio::test]
async fn downstream_disconnect_cancels_silent_native_generation() {
    for graceful in [true, false] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let disconnected = Arc::new(Notify::new());
        let observed = disconnected.clone();
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket
                .send(tungstenite::Message::Text(
                    json!({
                        "type":"response.created", "response":{"id":"native-pending","status":"in_progress"}
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            loop {
                match socket.next().await {
                    Some(Ok(tungstenite::Message::Close(_))) | Some(Err(_)) | None => break,
                    _ => {}
                }
            }
            observed.notify_one();
        });
        let _upstream = Server { url: base.clone(), task };
        let app = app(&base, true);
        let proxy = serve(crate::server::router(app.clone())).await;
        let mut socket = client(&proxy).await;
        create(&mut socket, true, "first").await;
        until_event(&mut socket, "response.created").await;
        if graceful {
            socket.close(None).await.unwrap();
        }
        drop(socket);
        cancelled(&app, 0, 0).await;
        tokio::time::timeout(Duration::from_secs(2), disconnected.notified())
            .await
            .expect("the provider socket must be released on client cancellation");
    }
}

#[tokio::test]
async fn downstream_disconnect_cancels_http_generation_and_keeps_usage() {
    let upstream = serve(axum::Router::new().route("/v1/messages", post(|| async {
        let body = Body::from_stream(async_stream::stream! {
            for value in [
                json!({"type":"message_start","message":{"id":"msg_pending","usage":{"input_tokens":20,"cache_read_input_tokens":80}}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}),
            ] {
                yield Ok::<_, Infallible>(Bytes::from(format!("data: {value}\n\n")));
            }
            std::future::pending::<()>().await;
        });
        ([("content-type", "text/event-stream")], body)
    }))).await;
    let app = app(&upstream.url, false);
    let proxy = serve(crate::server::router(app.clone())).await;
    let mut socket = client(&proxy).await;
    create(&mut socket, false, "first").await;
    until_event(&mut socket, "response.output_text.delta").await;
    socket.close(None).await.unwrap();
    drop(socket);
    cancelled(&app, 20, 80).await;
}

#[tokio::test]
async fn pipelined_creates_wait_for_prior_completion_and_keep_their_order() {
    let released = Arc::new(Notify::new());
    let (calls, mut received) = mpsc::unbounded_channel();
    let upstream = serve(axum::Router::new().route("/v1/messages", post({
        let released = released.clone();
        move |axum::Json(body): axum::Json<Value>| {
            let released = released.clone();
            let calls = calls.clone();
            async move {
                let prompt = body["messages"][0]["content"][0]["text"].as_str().unwrap().to_string();
                calls.send(prompt.clone()).unwrap();
                let body = Body::from_stream(async_stream::stream! {
                    let start = json!({"type":"message_start","message":{"id":format!("msg_{prompt}"),"usage":{"input_tokens":1}}});
                    yield Ok::<_, Infallible>(Bytes::from(format!("data: {start}\n\n")));
                    if prompt == "first" { released.notified().await; }
                    let text = json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":prompt}});
                    yield Ok(Bytes::from(format!("data: {text}\n\n")));
                    let stop = json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}});
                    yield Ok(Bytes::from(format!("data: {stop}\n\ndata: {{\"type\":\"message_stop\"}}\n\n")));
                });
                ([("content-type", "text/event-stream")], body)
            }
        }
    }))).await;
    let app = app(&upstream.url, false);
    let proxy = serve(crate::server::router(app.clone())).await;
    let mut socket = client(&proxy).await;
    create(&mut socket, false, "first").await;
    assert_eq!(received.recv().await.unwrap(), "first");
    for prompt in ["second", "third"] {
        create(&mut socket, false, prompt).await;
    }
    assert!(tokio::time::timeout(Duration::from_millis(50), received.recv()).await.is_err());
    released.notify_one();
    for prompt in ["first", "second", "third"] {
        let completed = until_event(&mut socket, "response.completed").await;
        assert_eq!(completed["response"]["output"][0]["content"][0]["text"], prompt);
    }
    assert_eq!(received.recv().await.unwrap(), "second");
    assert_eq!(received.recv().await.unwrap(), "third");
}

#[tokio::test]
async fn downstream_write_deadline_releases_a_blocked_sink() {
    let (socket, _peer) = tokio::io::duplex(1);
    let mut sender =
        tokio_tungstenite::WebSocketStream::from_raw_socket(socket, tungstenite::protocol::Role::Client, None).await;
    let started = tokio::time::Instant::now();
    let downstream = crate::diagnostics::Downstream::default();
    assert!(
        crate::diagnostics::downstream_scope(
            downstream.clone(),
            client_write(sender.send(tungstenite::Message::Text("blocked response".into())))
        )
        .await
        .is_err()
    );
    assert_eq!(downstream.kind(), "downstream_write_timeout");
    assert!(started.elapsed() >= CLIENT_WRITE_TIMEOUT);
    assert!(started.elapsed() < CLIENT_WRITE_TIMEOUT + Duration::from_secs(2));
}

#[tokio::test]
async fn downstream_socket_error_is_distinct_from_write_timeout() {
    let downstream = crate::diagnostics::Downstream::default();
    assert!(
        crate::diagnostics::downstream_scope(
            downstream.clone(),
            client_write(async { Err::<(), _>("private socket error") })
        )
        .await
        .is_err()
    );
    assert_eq!(downstream.kind(), "downstream_write_error");
}

#[tokio::test]
async fn native_completion_reports_full_usage_without_durable_analytics() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket.send(tungstenite::Message::Text(json!({
            "type":"response.completed", "response":{"id":"native-complete","status":"completed","output":[],
                "usage":{"input_tokens":12,"output_tokens":2,"input_tokens_details":{"cached_tokens":4,"cache_creation_tokens":0}}}
        }).to_string().into())).await.unwrap();
        while socket.next().await.is_some() {}
    });
    let _upstream = Server { url: base.clone(), task };
    let app = app(&base, true);
    assert!(app.usage.is_none());
    let proxy = serve(crate::server::router(app.clone())).await;
    let mut socket = client(&proxy).await;
    create(&mut socket, true, "first").await;
    until_event(&mut socket, "response.completed").await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(log) = app.stats.recent.lock().front().cloned() {
                assert_eq!(log.status, 200);
                assert_eq!(log.transport, "ws");
                assert_eq!(log.usage_completeness, "complete");
                assert_eq!(log.failure_kind, None);
                assert_eq!((log.input_tokens, log.output_tokens, log.cache_tokens), (8, 2, 4));
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn codex_interrupts_reach_the_running_native_response() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (seen_tx, mut seen) = mpsc::unbounded_channel::<Value>();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(tungstenite::Message::Text(
                json!({"type":"response.created","response":{"id":"resp-int","status":"in_progress"}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        // The response only ends once the interrupt arrives.
        let tungstenite::Message::Text(text) = socket.next().await.unwrap().unwrap() else { panic!() };
        seen_tx.send(serde_json::from_str(&text).unwrap()).unwrap();
        socket
            .send(tungstenite::Message::Text(
                json!({
                    "type":"response.completed", "response":{"id":"resp-int","status":"completed","output":[],
                        "usage":{"input_tokens":3,"output_tokens":0}}
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        while socket.next().await.is_some() {}
    });
    let _upstream = Server { url: base.clone(), task };
    let app = app(&base, true);
    let proxy = serve(crate::server::router(app.clone())).await;
    let mut socket = client(&proxy).await;
    create(&mut socket, true, "long answer").await;
    until_event(&mut socket, "response.created").await;
    let interrupt = json!({"type":"response.interrupt","response_id":"resp-int","mode":"discard_partial_items"});
    socket.send(tungstenite::Message::Text(interrupt.to_string().into())).await.unwrap();
    until_event(&mut socket, "response.completed").await;
    assert_eq!(seen.recv().await.unwrap(), interrupt);
    // A late interrupt, after the response ended, is dropped rather than answered with an error.
    socket.send(tungstenite::Message::Text(interrupt.to_string().into())).await.unwrap();
    create(&mut socket, true, "next").await;
    let next = tokio::time::timeout(Duration::from_millis(500), socket.next()).await;
    if let Ok(Some(Ok(tungstenite::Message::Text(text)))) = next {
        assert!(!text.contains("unsupported message type"), "{text}");
    }
}

async fn client_with(server: &Server, headers: &[(&'static str, &str)]) -> Upstream {
    let url = format!("{}/v1/responses", server.url.replace("http://", "ws://"));
    let mut request = tungstenite::client::IntoClientRequest::into_client_request(url).unwrap();
    for (name, value) in headers {
        request.headers_mut().insert(*name, value.parse().unwrap());
    }
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

/// Every event of one turn, through its terminal event.
async fn turn_events(client: &mut Upstream) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        loop {
            let tungstenite::Message::Text(text) = client.next().await.unwrap().unwrap() else { continue };
            let value: Value = serde_json::from_str(&text).unwrap();
            let terminal = matches!(value["type"].as_str(), Some("response.completed" | "response.failed" | "error"));
            events.push(value);
            if terminal {
                return events;
            }
        }
    })
    .await
    .expect("the turn did not end")
}

/// Codex parses an `error` event with `code: Option<String>` and drops the event otherwise.
fn codex_reads(event: &Value) -> &str {
    assert_eq!(event["type"], "error", "{event}");
    assert!(event["status"].is_u64(), "{event}");
    event["error"]["code"].as_str().unwrap_or_else(|| panic!("Codex ignores this error event: {event}"))
}

#[derive(Default)]
struct Turns {
    /// Accounts whose socket received a turn, in order.
    websocket: parking_lot::Mutex<Vec<String>>,
    /// Accounts that served a turn over HTTP.
    http: parking_lot::Mutex<Vec<String>>,
}

fn bearer(headers: &axum::http::HeaderMap) -> String {
    headers["authorization"].to_str().unwrap().trim_start_matches("Bearer ").to_string()
}

/// A Codex upstream whose socket answers each turn with `frames`, and whose HTTP
/// endpoint completes it.
async fn codex_upstream(frames: Vec<Value>) -> (Server, Arc<Turns>) {
    let turns = Arc::new(Turns::default());
    let (ws_turns, http_turns) = (turns.clone(), turns.clone());
    let router = axum::Router::new().route(
        "/responses",
        axum::routing::get(move |headers: axum::http::HeaderMap, upgrade: axum::extract::ws::WebSocketUpgrade| {
            let (turns, frames) = (ws_turns.clone(), frames.clone());
            async move {
                upgrade.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.next().await {
                        if !matches!(message, axum::extract::ws::Message::Text(_)) {
                            continue;
                        }
                        turns.websocket.lock().push(bearer(&headers));
                        for frame in &frames {
                            let text = axum::extract::ws::Message::Text(frame.to_string().into());
                            if socket.send(text).await.is_err() {
                                return;
                            }
                        }
                    }
                })
            }
        })
        .post(move |headers: axum::http::HeaderMap| {
            let turns = http_turns.clone();
            async move {
                let account = bearer(&headers);
                turns.http.lock().push(account.clone());
                let events = [
                    json!({"type":"response.created","response":{"id":"resp-http","status":"in_progress"}}),
                    json!({"type":"response.output_text.delta","delta":account}),
                    json!({"type":"response.completed","response":{"id":"resp-http","status":"completed","output":[],
                        "usage":{"input_tokens":5,"output_tokens":1}}}),
                ];
                let body: String = events.iter().map(|v| format!("data: {v}\n\n")).collect();
                ([("content-type", "text/event-stream")], body)
            }
        }),
    );
    (serve(router).await, turns)
}

/// Codex subscriptions `a` and `b` on one upstream, both able to open native sockets.
fn codex_app(base: &str) -> Arc<App> {
    let key = |name: &str| KeyEntry {
        api_key: name.into(),
        base_url: Some(base.into()),
        label: Some(name.into()),
        ..Default::default()
    };
    let cfg = Config { auth_dir: "/nonexistent".into(), codex_api_key: vec![key("a"), key("b")], ..Default::default() };
    let app = App::new(cfg, "/nonexistent/config.yaml".into());
    for account in app.pool.all() {
        *account.cred.write() = Credential::OAuth(OAuth {
            access_token: account.label.clone(),
            base_url: Some(base.into()),
            ..Default::default()
        });
        account.state.lock().credential_identity = account.cred.read().identity(account.provider);
    }
    app
}

#[tokio::test]
async fn a_busy_first_frame_falls_back_to_http_but_never_after_output_or_on_a_5xx() {
    let capacity = json!({"type":"error","status":429,"error":{"type":"server_error","code":"turn_rate_limit_exceeded",
        "message":"Codex is at capacity, please try again later"}});
    let rate_limit = json!({"type":"error","status":429,"error":{"code":"rate_limit_exceeded","message":"slow down"}});
    let created = json!({"type":"response.created","response":{"id":"resp-ws","status":"in_progress"}});
    let delta = json!({"type":"response.output_text.delta","delta":"partial"});
    let overloaded = json!({"type":"response.failed","response":{"id":"resp-ws","status":"failed",
        "error":{"code":"server_is_overloaded","message":"Codex is overloaded"}}});
    let server_error =
        json!({"type":"error","status":503,"error":{"code":"server_is_overloaded","message":"overloaded"}});
    let rate_limits =
        json!({"type":"codex.rate_limits","rate_limits":{"primary":{"used_percent":10.0,"window_minutes":300}}});
    for (name, frames, falls_back) in [
        ("capacity first", vec![capacity.clone()], true),
        ("rate limit first", vec![rate_limit.clone()], true),
        // A frame only Fusebox read reached no client: still nothing forwarded.
        ("capacity after a private frame", vec![rate_limits, capacity.clone()], true),
        ("capacity after output", vec![created.clone(), delta.clone(), capacity.clone()], false),
        ("rate limit after created", vec![created.clone(), rate_limit], false),
        ("5xx response.failed first", vec![overloaded], false),
        ("5xx error first", vec![server_error], false),
    ] {
        let (upstream, turns) = codex_upstream(frames.clone()).await;
        let app = codex_app(&upstream.url);
        let proxy = serve(crate::server::router(app.clone())).await;
        let mut socket = client(&proxy).await;
        create(&mut socket, true, "question").await;
        let events = turn_events(&mut socket).await;
        let websocket = turns.websocket.lock().clone();
        let http = turns.http.lock().clone();
        assert_eq!(websocket.len(), 1, "{name}");
        if falls_back {
            // The client sees one clean HTTP turn, never the upstream's error.
            let kinds: Vec<_> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
            assert_eq!(kinds, ["response.created", "response.output_text.delta", "response.completed"], "{name}");
            assert_eq!(http.len(), 1, "{name}");
            if name.starts_with("rate limit") {
                assert_ne!(http[0], websocket[0], "{name}: a rate limit moves the turn to another account");
            }
        } else {
            assert!(http.is_empty(), "{name}: a turn that may have run, or was seen, is never replayed");
            let shown: Vec<_> = frames.iter().filter(|f| f["type"] != "codex.rate_limits").cloned().collect();
            assert_eq!(events, shown, "{name}");
        }
        let account = app.pool.all().into_iter().find(|a| a.label == websocket[0]).unwrap();
        let rate_limited = account.cooling_until("gpt-6.1-sol").is_some();
        assert_eq!(rate_limited, name.starts_with("rate limit"), "{name}: only a plain rate limit pauses the model");
    }
}

#[tokio::test]
async fn native_sockets_send_private_events_to_codex_clients_only() {
    let frames = vec![
        json!({"type":"codex.rate_limits","rate_limits":{"primary":{"used_percent":40.0,"window_minutes":300,"reset_after_seconds":3600}}}),
        json!({"type":"response.metadata","metadata":{"buffer":true}}),
        json!({"type":"response.created","response":{"id":"resp-private","status":"in_progress"}}),
        json!({"type":"codex.response.metadata","headers":{"x-models-etag":"etag"}}),
        json!({"type":"response.output_text.delta","delta":"OK"}),
        json!({"type":"response.subscription_usage","usage":{"credits":1}}),
        json!({"type":"responsesapi.websocket_timing","elapsed_ms":3}),
        json!({"type":"response.completed","response":{"id":"resp-private","status":"completed","output":[],
            "usage":{"input_tokens":12,"output_tokens":2,"input_tokens_details":{"cached_tokens":4}}}}),
    ];
    let t3 = "T3 Code/0.164.0 (Mac OS 26.5.2; arm64) xterm-256color (T3 Code; 0.5.1)";
    for (headers, codex) in [
        (vec![("user-agent", "OpenAI/Python 2.3.0")], false),
        (vec![], false),
        (vec![("user-agent", t3), ("originator", "T3 Code")], true),
        (vec![("user-agent", crate::upstream::CODEX_USER_AGENT), ("originator", "codex-tui")], true),
    ] {
        let (upstream, _turns) = codex_upstream(frames.clone()).await;
        let app = codex_app(&upstream.url);
        let proxy = serve(crate::server::router(app.clone())).await;
        let mut socket = client_with(&proxy, &headers).await;
        create(&mut socket, true, "question").await;
        let events = turn_events(&mut socket).await;
        if codex {
            assert_eq!(events, frames, "{headers:?}");
        } else {
            let kinds: Vec<_> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
            assert_eq!(kinds, ["response.created", "response.output_text.delta", "response.completed"], "{headers:?}");
        }
        // Fusebox read the quota event either way, and recorded the usage.
        let account = app.pool.all().into_iter().find(|a| a.state.lock().quota.windows.len() == 1);
        assert!(account.is_some(), "{headers:?}: the quota event was not observed");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(log) = app.stats.recent.lock().front().cloned() {
                    assert_eq!(log.status, 200);
                    assert_eq!((log.input_tokens, log.output_tokens, log.cache_tokens), (8, 2, 4));
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn translated_and_refused_turns_send_error_events_codex_can_read() {
    let upstream = serve(axum::Router::new().route(
        "/v1/messages",
        post(|axum::Json(body): axum::Json<Value>| async move {
            let prompt = body["messages"][0]["content"][0]["text"].as_str().unwrap_or_default().to_string();
            let message = if prompt == "long" {
                "prompt is too long: 210000 tokens > 200000 maximum"
            } else {
                "messages: text content blocks must be non-empty"
            };
            let error = json!({"type":"error","error":{"type":"invalid_request_error","message":message}});
            (axum::http::StatusCode::BAD_REQUEST, axum::Json(error))
        }),
    ))
    .await;
    let app = app(&upstream.url, false);
    let mut cfg = (*app.cfg()).clone();
    cfg.named_clients = vec![crate::config::NamedClient {
        id: "socket".into(),
        label: "socket".into(),
        key: "fbx_socket".into(),
        requests_per_minute: Some(2),
        ..Default::default()
    }];
    app.set_config(cfg);
    let proxy = serve(crate::server::router(app.clone())).await;
    let mut socket = client_with(&proxy, &[("authorization", "Bearer fbx_socket")]).await;

    create(&mut socket, false, "long").await;
    let events = turn_events(&mut socket).await;
    assert_eq!(events.len(), 1);
    assert_eq!(codex_reads(&events[0]), "context_length_exceeded");
    assert_eq!(events[0]["status"], 400);
    assert!(events[0]["error"]["message"].as_str().unwrap().starts_with("prompt is too long"));

    create(&mut socket, false, "other").await;
    let events = turn_events(&mut socket).await;
    assert_eq!(codex_reads(&events[0]), "invalid_request_error");
    assert_eq!(events[0]["status"], 400);

    // The key's third turn this minute is refused before any upstream.
    create(&mut socket, false, "long").await;
    let events = turn_events(&mut socket).await;
    assert_eq!(codex_reads(&events[0]), "rate_limit_exceeded");
    assert_eq!(events[0]["status"], 429);
    assert!(events[0]["error"]["message"].as_str().unwrap().contains("requests a minute"));

    // Codes the body already names as strings are kept as they are.
    let body = json!({"error":{"message":"gone","code":"previous_response_not_found"}});
    let event: Value = serde_json::from_str(&error_event(400, &body)).unwrap();
    assert_eq!(codex_reads(&event), "previous_response_not_found");
    let event: Value =
        serde_json::from_str(&error_event(502, &json!({"error":{"message":"x","type":"upstream_error"}}))).unwrap();
    assert!(event["error"].get("code").is_none());
    let event: Value =
        serde_json::from_str(&error_event(401, &crate::formats::error_body(Format::Responses, 401, "no key"))).unwrap();
    assert_eq!(codex_reads(&event), "invalid_api_key");
}

/// With no other account to take it, a plain rate limit as the first frame reaches the
/// client as the provider sent it: falling back would only meet the pause it just set.
#[tokio::test]
async fn a_plain_rate_limit_on_the_only_account_reaches_the_client_as_sent() {
    let rate_limit = json!({"type":"error","status":429,"error":{"code":"rate_limit_exceeded","message":"slow down"}});
    let (upstream, turns) = codex_upstream(vec![rate_limit.clone()]).await;
    let app = codex_app(&upstream.url);
    let only = app.pool.all().into_iter().find(|a| a.label == "a").unwrap();
    for account in app.pool.all().into_iter().filter(|a| a.id != only.id) {
        account.state.lock().disabled = true;
    }
    let proxy = serve(crate::server::router(app.clone())).await;
    let mut socket = client(&proxy).await;
    create(&mut socket, true, "question").await;
    let events = turn_events(&mut socket).await;
    assert_eq!(events, [rate_limit]);
    assert!(turns.http.lock().is_empty(), "no HTTP turn");
    assert!(only.cooling_until("gpt-6.1-sol").is_some(), "the rate limit still pauses the model");
}

/// A socket has no response headers, so a wait for the client goes in the error event's
/// own `headers`, where Codex reads it; the private marker never reaches the client.
#[test]
fn error_events_carry_a_wait_in_their_headers() {
    let body = json!({"error":{"message":"all accounts are rate limited","code":429},"__fusebox_retry_after":30});
    let event: Value = serde_json::from_str(&error_event(429, &body)).unwrap();
    assert_eq!(event["headers"]["retry-after"], "30");
    assert_eq!(event["error"]["code"], "rate_limit_exceeded");
    assert!(!event.to_string().contains("__fusebox"), "{event}");
    let plain: Value = serde_json::from_str(&error_event(400, &json!({"error":{"message":"bad"}}))).unwrap();
    assert!(plain.get("headers").is_none(), "{plain}");
}

/// A 401 after output, on an account whose refresh token was refused for good, puts it
/// out until a sign-in, as over HTTP; a cooldown would only hide that it needs one.
#[tokio::test]
async fn a_refused_token_after_output_puts_a_held_account_out_rather_than_cooling_it() {
    let created = json!({"type":"response.created","response":{"id":"resp-ws","status":"in_progress"}});
    let refused = json!({"type":"error","status":401,"error":{"code":"invalid_api_key","message":"token revoked"}});
    let (upstream, _turns) = codex_upstream(vec![created, refused]).await;
    let app = codex_app(&upstream.url);
    for account in app.pool.all() {
        account.state.lock().refresh_hold = Some(crate::accounts::RefreshHold::new("", "invalid_grant".into()));
        assert!(!account.held_out(), "it serves while its access token lasts");
    }
    let proxy = serve(crate::server::router(app.clone())).await;
    let mut socket = client(&proxy).await;
    create(&mut socket, true, "question").await;
    let events = turn_events(&mut socket).await;
    assert_eq!(events.last().unwrap()["status"], 401, "{events:?}");
    let refused = app.pool.all().into_iter().find(|a| a.held_out()).expect("the refusing account is out");
    assert!(refused.state.lock().cooldowns.is_empty(), "no cooldown hides the sign-in");
}
