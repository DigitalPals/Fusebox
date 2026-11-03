//! Account for terminal events even when the client stops before transport EOF.

use super::*;
use crate::config::{Config, KeyEntry};
use crate::ir::Finish;
use axum::body::{Body, Bytes};
use axum::http::header;
use axum::routing::get;
use std::convert::Infallible;
use std::sync::atomic::Ordering;

struct Upstream(tokio::task::JoinHandle<()>);

impl Drop for Upstream {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn response(messages: Vec<Value>, done: bool, linger: bool) -> (reqwest::Response, Upstream) {
    let mut data: String = messages.into_iter().map(|v| format!("data: {v}\n\n")).collect();
    if done {
        data.push_str("data: [DONE]\n\n");
    }
    let router = axum::Router::new().route(
        "/",
        get(move || {
            let data = data.clone();
            async move {
                let body = Body::from_stream(async_stream::stream! {
                    yield Ok::<_, Infallible>(Bytes::from(data));
                    if linger { std::future::pending::<()>().await; }
                });
                ([(header::CONTENT_TYPE, "text/event-stream")], body)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = Upstream(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    (reqwest::get(url).await.unwrap(), server)
}

fn tracker(format: Format) -> (Arc<App>, Tracker) {
    let cfg = Config {
        auth_dir: "/nonexistent".into(),
        codex_api_key: vec![KeyEntry { api_key: "mock-only".into(), ..Default::default() }],
        ..Default::default()
    };
    let app = App::new(cfg, "/nonexistent/config.yaml".into());
    let mut tracker = Tracker::new(&app, format, true, "http", "gpt-6.1-sol");
    tracker.attempt(&app.pool.all()[0]);
    (app, tracker)
}

fn recorded(app: &App, status: u16, input: u64, output: u64, cache: u64) -> RequestLog {
    let recent = app.stats.recent.lock();
    assert_eq!(recent.len(), 1);
    let log = recent[0].clone();
    assert_eq!(log.status, status);
    assert_eq!((log.input_tokens, log.output_tokens, log.cache_tokens), (input, output, cache));
    assert_eq!(app.stats.active.load(Ordering::Relaxed), 0);
    let totals = app.stats.totals.lock();
    assert_eq!(totals.requests, 1);
    assert_eq!(totals.failed, u64::from(status >= 400 && status != 499));
    assert_eq!((totals.input_tokens, totals.output_tokens, totals.cache_tokens), (input, output, cache));
    let account = app.pool.all()[0].clone();
    let state = account.state.lock();
    assert_eq!(state.counters.requests, 1);
    assert_eq!(state.counters.failures, u64::from(status >= 400 && status != 499));
    assert_eq!(
        (state.counters.input_tokens, state.counters.output_tokens, state.counters.cache_tokens),
        (input, output, cache)
    );
    log
}

fn completed_messages(format: Format) -> Vec<Value> {
    match format {
        Format::Responses => vec![json!({"type":"response.completed", "response":{
            "id":"resp_test", "status":"completed", "output":[],
            "usage":{"input_tokens":100,"input_tokens_details":{"cached_tokens":80},"output_tokens":4}
        }})],
        Format::Claude => vec![
            json!({"type":"message_start", "message":{"id":"msg_test","usage":{"input_tokens":20,"cache_read_input_tokens":80}}}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}),
            json!({"type":"message_stop"}),
        ],
        Format::Chat => vec![
            json!({"id":"chat_test","choices":[{"index":0,"delta":{"content":"OK"},"finish_reason":"stop"}]}),
            // Usage arrives after the finish reason, as in stream_options.include_usage.
            json!({"id":"chat_test","choices":[],"usage":{"prompt_tokens":100,"prompt_tokens_details":{"cached_tokens":80},"completion_tokens":4}}),
        ],
        Format::Gemini => vec![json!({"candidates":[{"content":{"parts":[{"text":"OK"}]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":100,"cachedContentTokenCount":80,"candidatesTokenCount":4}})],
    }
}

fn terminal(format: Format, frame: &Frame) -> bool {
    if format == Format::Chat {
        return frame.data == "[DONE]";
    }
    let v: Value = serde_json::from_str(&frame.data).unwrap();
    match format {
        Format::Responses => v["type"] == "response.completed",
        Format::Claude => v["type"] == "message_stop",
        Format::Gemini => v["candidates"][0]["finishReason"] == "STOP",
        Format::Chat => unreachable!(),
    }
}

#[tokio::test]
async fn passthrough_completion_keeps_status_and_usage_before_eof() {
    for format in [Format::Responses, Format::Claude, Format::Chat, Format::Gemini] {
        let (app, tracker) = tracker(format);
        let (response, _server) = response(completed_messages(format), format == Format::Chat, true).await;
        let mut stream = passthrough_stream(Box::pin(response.bytes_stream()), format, tracker, false, 16 << 20, false);
        loop {
            let frame = stream.next().await.unwrap();
            if terminal(format, &frame) {
                break;
            }
            // A Chat finish reason must not discard the following usage-only chunk.
            assert!(app.stats.recent.lock().is_empty());
        }
        // The server deliberately keeps the body open after its terminal event.
        drop(stream);
        assert!(recorded(&app, 200, 20, 4, 80).error.is_none());
    }
}

#[tokio::test]
async fn rendered_completion_keeps_status_and_usage_when_final_frame_is_last_poll() {
    for format in [Format::Responses, Format::Claude, Format::Chat, Format::Gemini] {
        let (app, tracker) = tracker(format);
        let events = futures::stream::iter([
            Event::Text("OK".into()),
            Event::Usage(Usage { input: 20, output: 4, cache_read: 80, ..Default::default() }),
            Event::Finish(Finish::Stop),
        ]);
        let req = Arc::new(Request { include_usage: true, ..Default::default() });
        let mut stream = render_stream(Box::pin(events), format, "gpt-6.1-sol".into(), req, tracker);
        while !terminal(format, &stream.next().await.unwrap()) {}
        drop(stream);
        assert!(recorded(&app, 200, 20, 4, 80).error.is_none());
    }
}

#[tokio::test]
async fn interrupted_passthrough_keeps_received_usage_and_stays_cancelled() {
    let (app, tracker) = tracker(Format::Claude);
    let messages = vec![json!({"type":"message_start","message":{"id":"msg_test","usage":{
        "input_tokens":20,"output_tokens":1,"cache_read_input_tokens":80,"cache_creation_input_tokens":5
    }}})];
    let (response, _server) = response(messages, false, true).await;
    let mut stream =
        passthrough_stream(Box::pin(response.bytes_stream()), Format::Claude, tracker, false, 16 << 20, false);
    stream.next().await.unwrap();
    drop(stream);
    assert_eq!(recorded(&app, 499, 25, 1, 80).error.as_deref(), Some("client disconnected"));
}

#[tokio::test]
async fn disconnect_keeps_usage_later_in_an_already_received_sse_batch() {
    for format in [Format::Responses, Format::Claude, Format::Chat, Format::Gemini] {
        let (app, tracker) = tracker(format);
        let capture = tracker.usage_tap().unwrap();
        let first = match format {
            Format::Responses => {
                json!({"type":"response.created","response":{"id":"resp_test","status":"in_progress"}})
            }
            Format::Claude => json!({"type":"ping"}),
            Format::Chat => {
                json!({"id":"chat_test","choices":[{"index":0,"delta":{"content":"first"},"finish_reason":null}]})
            }
            Format::Gemini => json!({"candidates":[{"content":{"parts":[{"text":"first"}]}}]}),
        };
        let messages = std::iter::once(first).chain(completed_messages(format)).collect();
        let (response, _server) = response(messages, format == Format::Chat, true).await;
        let mut stream = passthrough_stream(Box::pin(response.bytes_stream()), format, tracker, false, 16 << 20, false);
        // All events arrived in one upstream chunk, but the client reads only
        // the first event. Do not drop usage already sitting in that chunk.
        assert!(!terminal(format, &stream.next().await.unwrap()));
        drop(stream);
        let log = recorded(&app, 499, 20, 4, 80);
        assert_eq!(log.error.as_deref(), Some("client disconnected"));
        let observation = capture.snapshot(Some(499), None, true);
        assert_eq!(observation.tokens.input, Some(20), "{format:?}");
        assert_eq!(observation.tokens.cache_read, Some(80), "{format:?}");
        assert_eq!(observation.tokens.cache_write, Some(0), "{format:?}");
        assert_eq!(observation.tokens.output, Some(4), "{format:?}");
        assert_eq!(observation.logical_success, Some(false));
    }
}

#[tokio::test]
async fn interrupted_rendered_stream_keeps_received_usage_and_stays_cancelled() {
    let (app, tracker) = tracker(Format::Responses);
    let events = futures::stream::iter([
        Event::Usage(Usage { input: 20, output: 1, cache_read: 80, cache_write: 5, ..Default::default() }),
        Event::Text("partial".into()),
    ])
    .chain(futures::stream::pending());
    let mut stream = render_stream(Box::pin(events), Format::Responses, "gpt-6.1-sol".into(), Arc::default(), tracker);
    stream.next().await.unwrap();
    drop(stream);
    assert_eq!(recorded(&app, 499, 25, 1, 80).error.as_deref(), Some("client disconnected"));
}

#[tokio::test]
async fn upstream_errors_keep_their_status_and_usage_when_dropped() {
    for passthrough in [false, true] {
        let (app, tracker) = tracker(Format::Claude);
        let (response, _server) = response(
            vec![
                json!({"type":"message_start","message":{"usage":{"input_tokens":20,"cache_read_input_tokens":80}}}),
                json!({"type":"error","error":{"type":"overloaded_error","message":"mock overloaded"}}),
            ],
            false,
            true,
        )
        .await;
        let mut stream = if passthrough {
            passthrough_stream(Box::pin(response.bytes_stream()), Format::Claude, tracker, false, 16 << 20, false)
        } else {
            let events = futures::stream::iter([
                Event::Usage(Usage { input: 20, cache_read: 80, ..Default::default() }),
                Event::Error { status: 529, message: "mock overloaded".into() },
            ])
            .chain(futures::stream::pending());
            render_stream(Box::pin(events), Format::Claude, "claude-sonnet-4-6".into(), Arc::default(), tracker)
        };
        loop {
            let frame = stream.next().await.unwrap();
            if frame.event.as_deref() == Some("error")
                || serde_json::from_str::<Value>(&frame.data).unwrap()["type"] == "error"
            {
                break;
            }
        }
        drop(stream);
        assert_eq!(recorded(&app, 529, 20, 0, 80).error.as_deref(), Some("mock overloaded"));
    }
}

#[tokio::test]
async fn draining_a_completed_stream_records_usage_once() {
    let (app, tracker) = tracker(Format::Responses);
    let (response, _server) = response(completed_messages(Format::Responses), false, false).await;
    let mut stream =
        passthrough_stream(Box::pin(response.bytes_stream()), Format::Responses, tracker, false, 16 << 20, false);
    while stream.next().await.is_some() {}
    drop(stream);
    assert!(recorded(&app, 200, 20, 4, 80).error.is_none());
}

fn partial_messages(format: Format) -> Vec<Value> {
    match format {
        Format::Chat => vec![json!({"choices":[{"index":0,"delta":{"content":"PARTIAL"},"finish_reason":null}]})],
        Format::Responses => vec![json!({"type":"response.output_text.delta","delta":"PARTIAL"})],
        Format::Claude => {
            vec![json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"PARTIAL"}})]
        }
        Format::Gemini => vec![json!({"candidates":[{"content":{"parts":[{"text":"PARTIAL"}]}}]})],
    }
}

fn failed_frame(frame: &Frame) -> bool {
    serde_json::from_str::<Value>(&frame.data)
        .is_ok_and(|body| body["error"].is_object() || body["type"] == "response.failed")
}

#[tokio::test]
async fn premature_eof_and_done_are_errors_in_every_passthrough_format() {
    for format in [Format::Chat, Format::Responses, Format::Claude, Format::Gemini] {
        for done in [false, true] {
            if done && format != Format::Chat {
                continue;
            }
            let (app, tracker) = tracker(format);
            let (response, _server) = response(partial_messages(format), done, false).await;
            let frames: Vec<_> =
                passthrough_stream(Box::pin(response.bytes_stream()), format, tracker, false, 16 << 20, false)
                    .collect()
                    .await;
            let failure = frames.iter().position(failed_frame).expect("missing downstream error");
            if done {
                let end = frames.iter().position(|f| f.data == "[DONE]").unwrap();
                assert!(failure < end, "clients must see the error before [DONE]");
            }
            assert_eq!(recorded(&app, 502, 0, 0, 0).error.as_deref(), Some(PREMATURE_END));
        }
    }
}

#[tokio::test]
async fn claude_message_stop_without_stop_reason_cannot_hide_a_truncated_turn() {
    let (app, tracker) = tracker(Format::Claude);
    let mut messages = partial_messages(Format::Claude);
    messages.push(json!({"type":"message_stop"}));
    let (response, _server) = response(messages, false, true).await;
    let mut stream =
        passthrough_stream(Box::pin(response.bytes_stream()), Format::Claude, tracker, false, 16 << 20, false);
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next()).await.unwrap().unwrap();
        assert!(!terminal(Format::Claude, &frame), "a client would stop before seeing the error");
        if failed_frame(&frame) {
            break;
        }
    }
    drop(stream);
    recorded(&app, 502, 0, 0, 0);
}

#[tokio::test]
async fn incomplete_translated_streams_keep_usage_and_never_report_success() {
    for format in [Format::Chat, Format::Responses, Format::Claude, Format::Gemini] {
        let (app, tracker) = tracker(format);
        let events = futures::stream::iter([
            Event::Usage(Usage { input: 20, output: 4, cache_read: 80, ..Default::default() }),
            Event::Text("PARTIAL".into()),
            Event::ToolStart { key: 0, id: "call_partial".into(), name: "lookup".into() },
            Event::ToolArgs { key: 0, delta: "{\"incomplete\":".into() },
        ]);
        let frames: Vec<_> =
            render_stream(Box::pin(events), format, "test-model".into(), Arc::default(), tracker).collect().await;
        assert!(frames.iter().any(failed_frame));
        assert!(!frames.iter().any(|f| f.data != "[DONE]" && terminal(format, f)));
        assert_eq!(recorded(&app, 502, 20, 4, 80).error.as_deref(), Some(PREMATURE_END));
    }
}

#[tokio::test]
async fn incomplete_nonstreaming_collection_is_an_error() {
    for format in [Format::Chat, Format::Responses, Format::Claude, Format::Gemini] {
        let (app, tracker) = tracker(format);
        let events = futures::stream::iter([Event::Text("PARTIAL".into())]);
        let reply = collect(Box::pin(events), format, "test-model", &Request::default(), tracker).await;
        assert!(matches!(reply, Reply::Error(502, _)));
        recorded(&app, 502, 0, 0, 0);
    }
    let (app, tracker) = tracker(Format::Responses);
    let bytes = bytes::Bytes::from(format!("data: {}\n\n", partial_messages(Format::Responses)[0]));
    let body = futures::stream::once(async move { Ok(bytes) });
    let reply = collect_passthrough(Box::pin(body), Format::Responses, tracker, Format::Responses, 16 << 20).await;
    assert!(matches!(reply, Reply::Error(502, _)));
    recorded(&app, 502, 0, 0, 0);
}

#[test]
fn incomplete_json_cannot_gain_a_default_successful_finish_reason() {
    for (format, body) in [
        (Format::Chat, json!({"choices":[{"message":{"content":"PARTIAL"}}]})),
        (Format::Claude, json!({"content":[{"type":"text","text":"PARTIAL"}]})),
        (Format::Responses, json!({"status":"in_progress","output":[]})),
        (Format::Gemini, json!({"candidates":[{"content":{"parts":[{"text":"PARTIAL"}]}}]})),
    ] {
        let events = checked_full_events(format, &body);
        assert!(events.iter().any(|e| matches!(e, Event::Error { status: 502, .. })));
        assert!(!events.iter().any(|e| matches!(e, Event::Finish(_))));
    }
}

#[tokio::test]
async fn failed_complete_json_keeps_billable_usage() {
    let (app, tracker) = tracker(Format::Responses);
    let body = json!({"id":"failed","status":"failed","output":[],
        "error":{"code":"server_error","message":"generation failed"},
        "usage":{"input_tokens":100,"input_tokens_details":{"cached_tokens":80},"output_tokens":4}});
    let events = futures::stream::iter(checked_full_events(Format::Responses, &body));
    assert!(matches!(
        collect(Box::pin(events), Format::Responses, "test", &Request::default(), tracker).await,
        Reply::Error(502, _)
    ));
    recorded(&app, 502, 20, 4, 80);
}

#[tokio::test]
async fn passthrough_body_read_failure_emits_a_downstream_error() {
    let router = axum::Router::new().route("/", get(|| async {
        let body = Body::from_stream(async_stream::stream! {
            yield Ok::<_, std::io::Error>(Bytes::from(format!("data: {}\n\n", partial_messages(Format::Responses)[0])));
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            yield Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "mock disconnect"));
        });
        ([(header::CONTENT_TYPE, "text/event-stream")], body)
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let _server = Upstream(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    let response = reqwest::get(url).await.unwrap();
    let (app, tracker) = tracker(Format::Responses);
    let frames: Vec<_> =
        passthrough_stream(Box::pin(response.bytes_stream()), Format::Responses, tracker, false, 16 << 20, false)
            .collect()
            .await;
    assert!(frames.iter().any(failed_frame));
    assert!(recorded(&app, 502, 0, 0, 0).error.unwrap().contains("upstream stream error"));
}

#[tokio::test]
async fn a_trailing_event_over_the_limit_fails_collection_even_after_completion() {
    let (app, tracker) = tracker(Format::Responses);
    let completed = completed_messages(Format::Responses)[0].to_string();
    // The trailing event's lines each fit, but together they exceed the limit,
    // which the decoder can only notice in its final flush.
    let text = format!("data: {completed}\n\ndata: {}\ndata: {}", "x".repeat(150), "y".repeat(120));
    let body = futures::stream::once(async move { Ok(bytes::Bytes::from(text)) });
    let reply = collect_passthrough(Box::pin(body), Format::Responses, tracker, Format::Responses, 256).await;
    let Reply::Error(502, body) = reply else { panic!("collection succeeded past the event limit") };
    assert!(body.to_string().contains(crate::sse::LIMIT_ERROR), "{body}");
    assert_eq!(recorded(&app, 502, 20, 4, 80).error.as_deref(), Some(crate::sse::LIMIT_ERROR));
}

const T3_AGENT: &str = "T3 Code/0.164.0 (Mac OS 26.5.2; arm64) xterm-256color (T3 Code; 0.5.1)";

fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.insert(*k, v.parse().unwrap());
    }
    h
}

#[test]
fn codex_clients_and_app_server_hosts_read_private_events_and_others_do_not() {
    for codex in [
        headers(&[("user-agent", T3_AGENT), ("originator", "T3 Code")]),
        headers(&[("user-agent", crate::upstream::CODEX_USER_AGENT), ("originator", "codex-tui")]),
        headers(&[
            ("user-agent", "codex_cli_rs/0.164.0 (Mac OS 26.5.2; arm64) iTerm.app/3.6.11"),
            ("originator", "codex_cli_rs"),
        ]),
        // An app-server host with its own name, which `client_app` doesn't know.
        headers(&[("user-agent", "Some Host/1.0"), ("originator", "Some Host")]),
    ] {
        assert!(reads_private_events(&codex), "{codex:?}");
    }
    assert_eq!(client_app(&headers(&[("user-agent", T3_AGENT), ("originator", "T3 Code")])), None);
    for other in [
        headers(&[("user-agent", "OpenAI/Python 2.3.0")]),
        headers(&[("user-agent", "OpenAI/JS 6.1.0")]),
        headers(&[("user-agent", "curl/8.9.0")]),
        HeaderMap::new(),
    ] {
        assert!(!reads_private_events(&other), "{other:?}");
    }
    for name in [
        "response.metadata",
        "codex.rate_limits",
        "codex.response.metadata",
        "response.subscription_usage",
        "responsesapi.websocket_timing",
    ] {
        assert!(private_event(name), "{name}");
    }
    for name in [
        "response.created",
        "response.completed",
        "response.failed",
        "error",
        "response.output_text.delta",
        "codex",
        "response.metadata.extra",
    ] {
        assert!(!private_event(name), "{name}");
    }
}

/// A Responses stream with every private event shape: by `type`, by SSE event name
/// only, and an event name with empty data.
fn private_wire() -> (String, usize) {
    let frames = [
        "event: response.metadata\ndata: \n\n".to_string(),
        format!(
            "data: {}\n\n",
            json!({"type":"response.created","response":{"id":"resp_test","status":"in_progress"}})
        ),
        format!(
            "event: response.metadata\ndata: {}\n\n",
            json!({"type":"response.metadata","metadata":{"buffer":true}})
        ),
        format!("data: {}\n\n", json!({"type":"codex.rate_limits","rate_limits":{"primary":{"used_percent":10.0}}})),
        format!("event: codex.response.metadata\ndata: {}\n\n", json!({"headers":{"x-models-etag":"etag"}})),
        format!(
            "data: {}\n\n",
            json!({"type":"response.subscription_usage","subscription":{"tier":"t1","window":{"used_percent":3,"window_duration_mins":300,"resets_at":4102444800i64},"weekly":{"used_percent":1,"resets_at":4102444800i64}}})
        ),
        format!("data: {}\n\n", json!({"type":"responsesapi.websocket_timing","elapsed_ms":3})),
        format!(
            "event: response.output_text.delta\ndata: {}\n\n",
            json!({"type":"response.output_text.delta","delta":"OK"})
        ),
        format!("event: response.completed\ndata: {}\n\n", completed_messages(Format::Responses)[0]),
    ];
    (frames.concat(), frames.len())
}

#[tokio::test]
async fn private_responses_events_reach_codex_clients_only_and_usage_is_kept() {
    let dropped = || crate::proxy::private_dropped().into_iter().map(|(_, n)| n).sum::<u64>();
    for drop_private in [false, true] {
        let (app, tracker) = tracker(Format::Responses);
        let (wire, count) = private_wire();
        let before = dropped();
        let body = futures::stream::once(async move { Ok(bytes::Bytes::from(wire)) });
        let frames: Vec<_> =
            passthrough_stream(Box::pin(body), Format::Responses, tracker, false, 16 << 20, drop_private)
                .collect()
                .await;
        let kinds: Vec<_> = frames
            .iter()
            .map(|f| {
                let data = serde_json::from_str::<Value>(&f.data).ok();
                (f.event.as_deref().map(String::from), data.and_then(|v| v["type"].as_str().map(String::from)))
            })
            .collect();
        if drop_private {
            let public = |event: Option<&str>, kind: &str| (event.map(String::from), Some(kind.to_string()));
            assert_eq!(
                kinds,
                [
                    public(None, "response.created"),
                    public(Some("response.output_text.delta"), "response.output_text.delta"),
                    public(Some("response.completed"), "response.completed"),
                ]
            );
            // Counted for /metrics (other tests may count at the same time).
            assert!(dropped() >= before + (count - 3) as u64);
        } else {
            assert_eq!(frames.len(), count, "{kinds:?}");
            assert_eq!(frames[0].event.as_deref(), Some("response.metadata"));
            assert_eq!(frames[0].data, "");
        }
        assert!(recorded(&app, 200, 20, 4, 80).error.is_none());
    }
    // Only native Responses streams are filtered.
    let (app, tracker) = tracker(Format::Chat);
    let mut messages = vec![json!({"type":"response.metadata"})];
    messages.extend(completed_messages(Format::Chat));
    let (response, _server) = response(messages, true, false).await;
    let frames: Vec<_> =
        passthrough_stream(Box::pin(response.bytes_stream()), Format::Chat, tracker, false, 16 << 20, true)
            .collect()
            .await;
    assert!(frames[0].data.contains("response.metadata"));
    assert_eq!(frames.len(), 4);
    recorded(&app, 200, 20, 4, 80);
}

/// Two Codex API keys behind one mock. The first call overloads after a metadata frame.
async fn private_event_upstream() -> (Arc<App>, Upstream, Arc<parking_lot::Mutex<Vec<String>>>) {
    let calls = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let seen = calls.clone();
    let router = axum::Router::new().route(
        "/responses",
        axum::routing::post(move |headers: HeaderMap| {
            let calls = seen.clone();
            async move {
                let account = headers["authorization"].to_str().unwrap().trim_start_matches("Bearer ").to_string();
                let first = {
                    let mut calls = calls.lock();
                    calls.push(account.clone());
                    calls.len() == 1
                };
                let metadata = format!("event: response.metadata\ndata: {}\n\n", json!({"type":"response.metadata","account":account}));
                let rest = if first {
                    let failed = json!({"type":"response.failed","response":{"error":{"code":"server_is_overloaded","message":"mock overloaded"}}});
                    format!("data: {failed}\n\n")
                } else {
                    private_wire().0
                };
                ([(header::CONTENT_TYPE, "text/event-stream")], format!("{metadata}{rest}"))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = Upstream(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    let key = |name: &str| KeyEntry {
        api_key: name.into(),
        base_url: Some(url.clone()),
        label: Some(name.into()),
        ..Default::default()
    };
    let cfg = Config { auth_dir: "/nonexistent".into(), codex_api_key: vec![key("a"), key("b")], ..Default::default() };
    (App::new(cfg, "/nonexistent/config.yaml".into()), server, calls)
}

#[tokio::test]
async fn an_overload_after_a_metadata_frame_still_fails_over_for_every_client() {
    for (agent, originator, codex) in [
        ("OpenAI/Python 2.3.0", None, false),
        (T3_AGENT, Some("T3 Code"), true),
        (crate::upstream::CODEX_USER_AGENT, Some("codex-tui"), true),
    ] {
        let (app, _upstream, calls) = private_event_upstream().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let router = crate::server::router(app.clone());
        let _proxy = Upstream(tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
        }));
        let mut request = reqwest::Client::new()
            .post(format!("{proxy}/v1/responses"))
            .header("user-agent", agent)
            .json(&json!({"model":"gpt-6.1-sol","input":"question","stream":true}));
        if let Some(originator) = originator {
            request = request.header("originator", originator);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 200, "{agent}");
        let text = response.text().await.unwrap();
        let mut dec = SseDecoder::default();
        let events: Vec<_> = dec.push(text.as_bytes()).into_iter().chain(dec.finish()).collect();
        let named = |name: &str| {
            events
                .iter()
                .filter(|e| {
                    e.event.as_deref() == Some(name)
                        || serde_json::from_str::<Value>(&e.data).is_ok_and(|v| v["type"] == name)
                })
                .count()
        };
        // The failed attempt's held frames never reach the client, whoever it is.
        let calls = calls.lock().clone();
        assert_eq!(calls.len(), 2, "{agent}");
        assert_ne!(calls[0], calls[1], "{agent}: the overload must move to the other account");
        assert!(!text.contains(&format!("\"account\":\"{}\"", calls[0])), "{agent}: {text}");
        assert!(!text.contains("mock overloaded"), "{agent}: {text}");
        assert_eq!(named("response.completed"), 1, "{agent}: {text}");
        assert_eq!(named("response.output_text.delta"), 1, "{agent}: {text}");
        if codex {
            assert_eq!(events.len(), private_wire().1 + 1, "{agent}: {text}");
            assert_eq!(named("response.metadata"), 3, "{agent}: {text}");
            assert_eq!(named("codex.rate_limits"), 1, "{agent}: {text}");
        } else {
            assert_eq!(events.len(), 3, "{agent}: {text}");
            for private in [
                "response.metadata",
                "codex.rate_limits",
                "codex.response.metadata",
                "response.subscription_usage",
                "responsesapi.websocket_timing",
            ] {
                assert_eq!(named(private), 0, "{agent}: {private} in {text}");
            }
        }
        // Usage and completion are recorded on the account that answered.
        let log = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if let Some(log) = app.stats.recent.lock().iter().find(|l| l.status == 200).cloned() {
                    return log;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the request log was not recorded");
        assert_eq!((log.input_tokens, log.output_tokens, log.cache_tokens), (20, 4, 80), "{agent}");
        assert_eq!(log.error, None);
        assert_eq!(log.account, calls[1], "{agent}");
    }
}
