//! Output speed times a call from when generation started, and only a call that
//! succeeded with every output token inside the timed span.

use super::*;
use crate::config::{Config, KeyEntry};
use crate::sse::SseEvent;
use axum::body::{Body, Bytes};
use axum::http::header;
use axum::routing::get;
use std::convert::Infallible;
use std::time::Duration;

fn new_tracker() -> (Arc<App>, Tracker) {
    let cfg = Config {
        auth_dir: "/nonexistent".into(),
        codex_api_key: vec![KeyEntry { api_key: "mock-only".into(), ..Default::default() }],
        ..Default::default()
    };
    let app = App::new(cfg, "/nonexistent/config.yaml".into());
    let mut tracker = Tracker::new(&app, Format::Responses, true, "http", "gpt-6.1-sol");
    tracker.attempt(&app.pool.all()[0]);
    (app, tracker)
}

fn events(format: Format, messages: &[Value]) -> Vec<Event> {
    let mut parser = formats::parser(format);
    let mut out = Vec::new();
    for v in messages {
        parser.feed_value(&SseEvent { event: None, data: v.to_string() }, Some(v), &mut out);
    }
    out
}

/// Pretends the call started `ms` ago, so the next event lands at that time.
fn at(tracker: &mut Tracker, ms: u64) {
    tracker.started = Instant::now() - Duration::from_millis(ms);
}

fn timing(tap: &crate::usage::capture::Capture) -> (Option<u64>, Option<u64>) {
    let o = tap.snapshot(None, None, true);
    (o.numeric_metadata.get("request_ttft_ms").copied(), o.numeric_metadata.get("request_latency_ms").copied())
}

fn near(got: Option<u64>, want: u64) -> bool {
    got.is_some_and(|g| g.abs_diff(want) < 500)
}

#[test]
fn reasoning_items_and_thinking_blocks_mark_where_generation_starts() {
    let codex = events(
        Format::Responses,
        &[
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[]}}),
        ],
    );
    assert!(matches!(codex[..], [Event::ReasoningStart]), "{codex:?}");
    // Claude opens a thinking block empty; omitted thinking then sends only its signature.
    let claude = events(
        Format::Claude,
        &[json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}})],
    );
    assert!(matches!(claude[..], [Event::ReasoningStart]), "{claude:?}");
    // The marker renders nothing in any format.
    let req = Request::default();
    for format in [Format::Chat, Format::Claude, Format::Responses, Format::Gemini] {
        let mut frames = Vec::new();
        formats::renderer(format, "m", &req).push(&Event::ReasoningStart, &mut frames);
        assert!(frames.is_empty(), "{format:?}: {frames:?}");
    }
}

#[tokio::test]
async fn hidden_reasoning_is_timed_from_its_start_not_the_first_text() {
    let (app, mut tracker) = new_tracker();
    let tap = tracker.usage_tap().unwrap();
    // Reasoning starts after 1 s, the first text arrives after 21 s, the call ends at 23 s.
    at(&mut tracker, 1_000);
    tracker.observe_stream_event(&Event::ReasoningStart);
    at(&mut tracker, 21_000);
    tracker.observe_stream_event(&Event::Text("Done.".into()));
    at(&mut tracker, 23_000);
    tracker.finish(200, &Usage { output: 2_200, reasoning: 2_000, ..Default::default() }, None);
    let (start, total) = timing(&tap);
    assert!(near(start, 1_000) && near(total, 23_000), "{start:?} {total:?}");
    let log = app.stats.recent.lock()[0].clone();
    // 2,200 tokens over 22 s, not over the 2 s after the text: 100 tok/s, not 1,100.
    assert!(near(log.stream_ms, 22_000), "{:?}", log.stream_ms);
    // The request log keeps the first content a client saw.
    assert!(near(log.ttft_ms, 21_000), "{:?}", log.ttft_ms);
}

#[tokio::test]
async fn reasoning_never_streamed_nor_announced_is_not_timed() {
    // Gemini and Chat report hidden reasoning in their usage only.
    let (app, mut tracker) = new_tracker();
    let tap = tracker.usage_tap().unwrap();
    at(&mut tracker, 20_000);
    tracker.observe_stream_event(&Event::Text("Done.".into()));
    at(&mut tracker, 22_000);
    tracker.finish(200, &Usage { output: 2_200, reasoning: 2_000, ..Default::default() }, None);
    let (start, total) = timing(&tap);
    assert_eq!(start, None);
    assert!(near(total, 22_000), "the call still counts, untimed");
    let log = app.stats.recent.lock()[0].clone();
    assert_eq!(log.stream_ms, None);
    assert!(near(log.ttft_ms, 20_000));

    // Reasoning text without a start marker may be a summary sent after the thinking
    // (Gemini's thoughts, a Chat `reasoning` field): not timed either.
    let (app, mut tracker) = new_tracker();
    let tap = tracker.usage_tap().unwrap();
    at(&mut tracker, 20_000);
    tracker.observe_stream_event(&Event::Reasoning("I considered the options.".into()));
    tracker.observe_stream_event(&Event::Text("Done.".into()));
    at(&mut tracker, 22_000);
    tracker.finish(200, &Usage { output: 2_200, reasoning: 2_000, ..Default::default() }, None);
    assert_eq!(timing(&tap).0, None);
    assert_eq!(app.stats.recent.lock()[0].stream_ms, None);

    // Reasoning streamed from its marked start is inside the span.
    let (app, mut tracker) = new_tracker();
    let tap = tracker.usage_tap().unwrap();
    at(&mut tracker, 1_000);
    tracker.observe_stream_event(&Event::ReasoningStart);
    tracker.observe_stream_event(&Event::Reasoning("Let me think".into()));
    at(&mut tracker, 21_000);
    tracker.observe_stream_event(&Event::Text("Done.".into()));
    at(&mut tracker, 22_000);
    tracker.finish(200, &Usage { output: 2_200, reasoning: 2_000, ..Default::default() }, None);
    assert!(near(timing(&tap).0, 1_000));
    assert!(near(app.stats.recent.lock()[0].stream_ms, 21_000));
}

#[tokio::test]
async fn redacted_thinking_before_anything_else_is_not_timed() {
    let (app, mut tracker) = new_tracker();
    let tap = tracker.usage_tap().unwrap();
    at(&mut tracker, 20_000);
    tracker.observe_stream_event(&Event::RedactedReasoning("opaque".into()));
    tracker.observe_stream_event(&Event::Text("Done.".into()));
    at(&mut tracker, 22_000);
    tracker.finish(200, &Usage { output: 2_200, ..Default::default() }, None);
    assert_eq!(timing(&tap).0, None);
    assert_eq!(app.stats.recent.lock()[0].stream_ms, None);
}

#[tokio::test]
async fn reasoning_held_by_bootstrap_counts_from_when_it_arrived_in_order() {
    // Bootstrap saw the reasoning start 4 s before releasing the stream.
    let (app, mut tracker) = new_tracker();
    let tap = tracker.usage_tap().unwrap();
    at(&mut tracker, 5_000);
    tracker.held_reasoning_at = Some(Instant::now() - Duration::from_millis(4_000));
    tracker.observe_stream_event(&Event::ReasoningStart);
    tracker.observe_stream_event(&Event::Text("Done.".into()));
    at(&mut tracker, 7_000);
    tracker.finish(200, &Usage { output: 600, reasoning: 400, ..Default::default() }, None);
    assert!(near(timing(&tap).0, 1_000), "{:?}", timing(&tap));
    assert!(near(app.stats.recent.lock()[0].stream_ms, 6_000));

    // Redacted thinking held before it still leaves the start unknown.
    let (app, mut tracker) = new_tracker();
    let tap = tracker.usage_tap().unwrap();
    at(&mut tracker, 5_000);
    tracker.held_reasoning_at = Some(Instant::now() - Duration::from_millis(1_000));
    tracker.observe_stream_event(&Event::RedactedReasoning("opaque".into()));
    tracker.observe_stream_event(&Event::ReasoningStart);
    tracker.observe_stream_event(&Event::Text("Done.".into()));
    at(&mut tracker, 7_000);
    tracker.finish(200, &Usage { output: 600, ..Default::default() }, None);
    assert_eq!(timing(&tap).0, None);
    assert_eq!(app.stats.recent.lock()[0].stream_ms, None);
}

#[tokio::test]
async fn a_new_attempt_starts_its_timing_afresh() {
    let (app, mut tracker) = new_tracker();
    let tap = tracker.usage_tap().unwrap();
    at(&mut tracker, 1_000);
    tracker.observe_stream_event(&Event::RedactedReasoning("opaque".into()));
    tracker.attempt(&app.pool.all()[0]);
    let tap = tracker.usage_tap().unwrap_or(tap);
    at(&mut tracker, 3_000);
    tracker.observe_stream_event(&Event::Text("Done.".into()));
    at(&mut tracker, 5_000);
    tracker.finish(200, &Usage { output: 200, ..Default::default() }, None);
    assert!(near(timing(&tap).0, 3_000), "{:?}", timing(&tap));
    assert!(near(app.stats.recent.lock()[0].stream_ms, 2_000));
}

#[tokio::test]
async fn failed_and_cancelled_calls_are_not_timed() {
    // Claude reports an output count at its start, so a call cut off later still has one.
    for (status, error) in [(499, "client disconnected"), (502, "upstream stream error")] {
        let (app, mut tracker) = new_tracker();
        let tap = tracker.usage_tap().unwrap();
        at(&mut tracker, 1_000);
        tracker.observe_stream_event(&Event::Text("Partial".into()));
        at(&mut tracker, 30_000);
        tracker.finish(status, &Usage { output: 1, ..Default::default() }, Some(error.into()));
        assert_eq!(timing(&tap).0, None, "{status}");
        let log = app.stats.recent.lock()[0].clone();
        assert_eq!(log.stream_ms, None, "{status}");
        assert!(near(log.ttft_ms, 1_000), "{status}: the request log keeps its first content");
    }
}

async fn upstream(messages: Vec<Value>, linger: bool) -> (ByteStream, tokio::task::JoinHandle<()>) {
    let data: String = messages.into_iter().map(|v| format!("data: {v}\n\n")).collect();
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
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (Box::pin(reqwest::get(url).await.unwrap().bytes_stream()), server)
}

#[tokio::test]
async fn bootstrap_reports_when_held_reasoning_began() {
    // Reasoning arrives at once, then nothing: the stream is held for BOOT_HOLD, and
    // the tracker sees the held events only after it.
    let (body, server) = upstream(
        vec![
            json!({"type":"response.created","response":{"id":"resp_1","status":"in_progress"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1"}}),
        ],
        true,
    )
    .await;
    let limits = crate::stream_limits::Limits::default();
    let mut first_error = None;
    let boot = bootstrap(body, Format::Responses, None, &limits, "gpt-6.1-sol", &mut first_error).await;
    let released = tokio::time::Instant::now();
    let Boot::Ready(_, Some(at)) = boot else { panic!("held reasoning without its start") };
    assert!(released - at >= BOOT_HOLD - Duration::from_millis(50), "{:?}", released - at);
    server.abort();
}

#[tokio::test]
async fn a_non_streaming_client_on_a_streaming_upstream_gets_a_first_token() {
    let (app, tracker) = new_tracker();
    let (body, server) = upstream(
        vec![
            json!({"type":"response.output_text.delta","output_index":0,"delta":"OK"}),
            json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[],
                "usage":{"input_tokens":10,"output_tokens":4}}}),
        ],
        false,
    )
    .await;
    collect_passthrough(body, Format::Responses, tracker, Format::Responses, 16 << 20).await;
    let log = app.stats.recent.lock()[0].clone();
    assert_eq!(log.status, 200);
    assert!(log.ttft_ms.is_some(), "timed like a translated non-streaming call");
    server.abort();
}

/// Sends each batch of messages after its delay.
async fn paced_upstream(batches: Vec<(u64, Vec<Value>)>) -> (ByteStream, tokio::task::JoinHandle<()>) {
    let router = axum::Router::new().route(
        "/",
        get(move || {
            let batches = batches.clone();
            async move {
                let body = Body::from_stream(async_stream::stream! {
                    for (delay, messages) in batches {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        let data: String = messages.into_iter().map(|v| format!("data: {v}\n\n")).collect();
                        yield Ok::<_, Infallible>(Bytes::from(data));
                    }
                });
                ([(header::CONTENT_TYPE, "text/event-stream")], body)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (Box::pin(reqwest::get(url).await.unwrap().bytes_stream()), server)
}

#[tokio::test]
async fn a_held_codex_stream_is_timed_from_its_reasoning_item() {
    // Reasoning starts at once; bootstrap holds it for BOOT_HOLD (300 ms in tests)
    // and the text comes 600 ms later, then the end.
    let (app, mut tracker) = new_tracker();
    let tap = tracker.usage_tap().unwrap();
    let (body, server) = paced_upstream(vec![
        (
            0,
            vec![
                json!({"type":"response.created","response":{"id":"resp_1","status":"in_progress"}}),
                json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1"}}),
            ],
        ),
        (600, vec![json!({"type":"response.output_text.delta","output_index":1,"delta":"Done."})]),
        (
            100,
            vec![json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[],
            "usage":{"input_tokens":10,"output_tokens":400,"output_tokens_details":{"reasoning_tokens":300}}}})],
        ),
    ])
    .await;
    let limits = crate::stream_limits::Limits::default();
    let mut first_error = None;
    let Boot::Ready(stream, reasoning_at) =
        bootstrap(body, Format::Responses, None, &limits, "gpt-6.1-sol", &mut first_error).await
    else {
        panic!("the stream should be released after the hold")
    };
    // As the request path does with a released stream.
    tracker.held_reasoning_at = reasoning_at.map(|at| at.into_std());
    let mut frames = passthrough_stream(stream, Format::Responses, tracker, false, 16 << 20, false);
    while frames.next().await.is_some() {}
    drop(frames);
    let log = app.stats.recent.lock()[0].clone();
    assert_eq!(log.status, 200);
    let (start, total) = timing(&tap);
    let (start, total) = (start.expect("timed"), total.unwrap());
    // From the reasoning item, not from the release at 300 ms or the text at 600 ms.
    assert!(start < 200, "start {start} ms");
    assert!(log.ttft_ms.unwrap() >= 550, "the request log keeps the first text: {:?}", log.ttft_ms);
    assert_eq!(log.stream_ms, Some(total - start));
    server.abort();
}
