//! Request diagnostics remain useful when durable analytics is disabled and the
//! dashboard ring has evicted the original request.
use super::*;
use crate::config::{Config, KeyEntry};
use parking_lot::Mutex;
use std::io::Write;

// Tracing's callsite interest cache is process-wide. Exercise emitted records
// in a fresh test process with a global subscriber, as the service initializes
// logging, so unrelated parallel tests cannot disable a request callsite.
fn run_in_isolated_process(test: &str) -> bool {
    let name = format!("proxy::diagnostic_tests::{test}");
    const CHILD: &str = "FUSEBOX_DIAGNOSTIC_TEST_CHILD";
    if std::env::var(CHILD).as_deref() == Ok(name.as_str()) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--nocapture"])
        .env(CHILD, &name)
        .output()
        .expect("start isolated diagnostics test");
    assert!(
        output.status.success(),
        "isolated diagnostics failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);
impl Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl LogBuffer {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().clone()).unwrap()
    }
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        let writer = self.clone();
        tracing_subscriber::fmt().with_ansi(false).without_time().with_writer(move || writer.clone()).finish()
    }
}

fn app() -> Arc<App> {
    let app = App::new(
        Config {
            auth_dir: "/nonexistent".into(),
            codex_api_key: vec![KeyEntry { api_key: "mock-only".into(), ..Default::default() }],
            ..Default::default()
        },
        "/nonexistent/config.yaml".into(),
    );
    assert!(app.usage.is_none());
    app
}
fn tracker(app: &Arc<App>) -> Tracker {
    let mut tracker = Tracker::new(app, Format::Responses, true, "http", "gpt-6.1-sol");
    tracker.attempt(&app.pool.all()[0]);
    tracker
}

#[tokio::test]
async fn absent_and_explicit_zero_usage_remain_distinct_without_analytics() {
    let app = app();
    let mut missing = tracker(&app);
    missing.observe_wire(&json!({"usage":{}}));
    missing.finish(200, &Usage::default(), None);
    let mut zero = tracker(&app);
    zero.observe_wire(&json!({"usage":{"input_tokens":0,"output_tokens":0,
        "input_tokens_details":{"cached_tokens":0,"cache_creation_tokens":0}}}));
    zero.finish(200, &Usage::default(), None);
    let mut cancelled = tracker(&app);
    cancelled.observe_wire(&json!({"usage":{"output_tokens":4}}));
    cancelled.stream_usage.output = 4;
    drop(cancelled);
    drop(tracker(&app));
    let logs = app.stats.recent.lock();
    assert_eq!(logs[0].usage_completeness, "missing");
    assert_eq!(logs[1].usage_completeness, "complete");
    assert_eq!(logs[2].usage_completeness, "partial");
    assert_eq!(logs[2].output_tokens, 4);
    assert_eq!(logs[3].usage_completeness, "missing");
    for log in [&logs[2], &logs[3]] {
        assert_eq!(log.status, 499);
        assert_eq!(log.failure_kind, Some("downstream_disconnect"));
    }
}

#[tokio::test]
async fn fallback_keeps_diagnostic_snapshot_without_becoming_logical_final() {
    let app = app();
    let mut tracker = tracker(&app);
    let tap = tracker.usage_tap().unwrap();
    tracker.observe_wire(&json!({"response":{"id":"provider-response", "usage":{"output_tokens":3}}}));
    tracker.diagnostic_failure(crate::diagnostics::Failure::classified("rate_limit_or_quota"));
    tracker.finish_fallback(429, &Usage { output: 3, ..Default::default() }, Some("quota exhausted".into()));
    let log = app.stats.recent.lock()[0].clone();
    assert_eq!(log.usage_completeness, "partial");
    assert_eq!(log.failure_kind, Some("rate_limit_or_quota"));
    assert!(!tracker.logical_final);
    assert_eq!(tap.snapshot(Some(429), None, tracker.logical_final).logical_success, None);
    assert!(tracker.usage_tap().is_none());
}

#[tokio::test]
async fn journal_keeps_sanitized_request_details_after_ring_eviction() {
    if run_in_isolated_process("journal_keeps_sanitized_request_details_after_ring_eviction") {
        return;
    }
    let app = app();
    let buffer = LogBuffer::default();
    tracing::subscriber::set_global_default(buffer.subscriber()).unwrap();
    let secret = "private-prompt Bearer sk-token https://private.test/?key=secret";
    let mut first = tracker(&app);
    first.log.account = secret.into();
    first.log.account_id = secret.into();
    first.log.session_id = Some(secret.into());
    first.log.model = secret.into();
    first.first_token();
    first.observe_wire(&json!({"response":{"id":secret,"usage":{"output_tokens":7},
        "prompt_cache_diagnostics":{"type":"cache_miss","reason":secret,
            "comparison_reusable_tokens":120,"cache_missed_tokens":16,"comparison_response_id":secret}}}));
    first.diagnostic_failure(crate::diagnostics::Failure {
        kind: "upstream_body_read",
        causes: vec!["body_decode", "incomplete_message"],
    });
    first.finish(502, &Usage { output: 7, ..Default::default() }, Some(secret.into()));
    for _ in 0..300 {
        tracker(&app).finish(200, &Usage::default(), None);
    }
    assert_eq!(app.stats.recent.lock().len(), 300);
    assert_eq!(app.stats.recent.lock()[0].id, 2);
    let journal = buffer.text();
    assert_eq!(journal.matches("request completed").count(), 301);
    let recorded_ids: Vec<u64> = journal
        .lines()
        .filter(|line| line.contains("request completed"))
        .map(|line| line.split("request_id=").nth(1).unwrap().split_whitespace().next().unwrap().parse().unwrap())
        .collect();
    assert_eq!(recorded_ids, (1..=301).collect::<Vec<_>>());
    let failed = journal.lines().find(|line| line.contains("upstream_body_read")).unwrap();
    for field in [
        "request_id=1",
        "status=502",
        "transport=\"http\"",
        "attempts=1",
        "ttft_ms=Some(",
        "usage_completeness=\"partial\"",
        "body_decode > incomplete_message",
        "output_tokens=7",
        "reported_input_tokens=None",
        "reported_output_tokens=Some(7)",
        "provider_cache_result=\"cache_miss\"",
        "provider_cache_miss_reason=Some(\"unknown\")",
        "provider_cache_comparison_reusable_tokens=Some(120)",
        "provider_cache_missed_tokens=Some(16)",
    ] {
        assert!(failed.contains(field), "missing {field}: {failed}");
    }
    for fragment in ["private-prompt", "Bearer", "sk-token", "https://", "key=secret", "\u{1b}["] {
        assert!(!journal.contains(fragment), "journal exposed {fragment}");
    }
}

#[tokio::test]
async fn broken_http_body_records_sanitized_source_chain() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    if run_in_isolated_process("broken_http_body_records_sanitized_source_chain") {
        return;
    }
    let app = app();
    let buffer = LogBuffer::default();
    tracing::subscriber::set_global_default(buffer.subscriber()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 1000\r\n\r\ndata: {}\n\n",
            )
            .await
            .unwrap();
        socket.shutdown().await.unwrap();
    });
    let response = reqwest::get(format!("http://{address}/private?token=secret")).await.unwrap();
    let mut stream =
        passthrough_stream(Box::pin(response.bytes_stream()), Format::Responses, tracker(&app), false, 16 << 20, false);
    while stream.next().await.is_some() {}
    peer.await.unwrap();
    let logs = app.stats.recent.lock();
    assert_eq!(logs[0].status, 502);
    assert_eq!(logs[0].failure_kind, Some("upstream_body_read"));
    assert_eq!(logs[0].usage_completeness, "missing");
    let journal = buffer.text();
    assert!(journal.contains("body_decode"), "{journal}");
    assert!(journal.contains("unexpected_eof"), "{journal}");
    assert!(!journal.contains("token=secret"));
}

/// Whether Codex access tokens die before `expires_at` (action list B2): each Codex 401
/// logs the token's age from its `iat` and the time it had left, in whole hours, and
/// nothing else from the token.
#[test]
fn codex_401s_log_the_token_age_and_time_left_only() {
    if run_in_isolated_process("codex_401s_log_the_token_age_and_time_left_only") {
        return;
    }
    let buffer = LogBuffer::default();
    tracing::subscriber::set_global_default(buffer.subscriber()).unwrap();
    let directory = std::env::temp_dir().join(format!("fusebox-codex-401-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    use base64::Engine;
    let claims = json!({"iat": Utc::now().timestamp() - 50 * 3600 - 60, "email": "FAKE_CLAIM_MARKER"});
    let encode = |v: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
    let jwt = format!("{}.{}.FAKE_SIGNATURE_MARKER", encode(&json!({"alg":"none"})), encode(&claims));
    let expires = (Utc::now() + Duration::hours(46) + Duration::minutes(30)).to_rfc3339();
    std::fs::write(
        directory.join("a.json"),
        json!({"type":"codex", "access_token":jwt, "expired":expires, "account_id":"a"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        directory.join("b.json"),
        json!({"type":"codex", "access_token":"FAKE_OPAQUE_MARKER", "account_id":"b"}).to_string(),
    )
    .unwrap();
    let app = App::new(
        Config { auth_dir: directory.to_string_lossy().into(), ..Default::default() },
        directory.join("config.yaml"),
    );
    log_codex_401(&app.pool.get("file:a.json").unwrap(), None, 7);
    log_codex_401(&app.pool.get("file:b.json").unwrap(), None, 8);
    // A refresh replaced the token after the request went out: the age and expiry are
    // those of the token the request carried.
    let sent_claims =
        json!({"iat": Utc::now().timestamp() - 170 * 3600 - 60, "exp": Utc::now().timestamp() + 70 * 3600 + 60});
    let sent = format!("{}.{}.FAKE_SIGNATURE_MARKER", encode(&json!({"alg":"none"})), encode(&sent_claims));
    log_codex_401(&app.pool.get("file:a.json").unwrap(), Some(&sent), 9);
    let _ = std::fs::remove_dir_all(&directory);
    let journal = buffer.text();
    assert!(
        journal.contains("request_id=7") && journal.contains("token_age_hours=50 expires_in_hours=46"),
        "{journal}"
    );
    assert!(journal.contains("token_age_hours=unknown expires_in_hours=unknown"), "{journal}");
    assert!(
        journal.contains("request_id=9") && journal.contains("token_age_hours=170 expires_in_hours=70"),
        "{journal}"
    );
    for secret in ["FAKE_CLAIM_MARKER", "FAKE_SIGNATURE_MARKER", "FAKE_OPAQUE_MARKER", &jwt, &sent] {
        assert!(!journal.contains(secret), "{journal}");
    }
}

#[tokio::test]
async fn codex_stream_errors_and_capacity_waits_are_logged_without_messages() {
    use axum::response::IntoResponse;
    use std::sync::atomic::{AtomicUsize, Ordering};
    if run_in_isolated_process("codex_stream_errors_and_capacity_waits_are_logged_without_messages") {
        return;
    }
    let buffer = LogBuffer::default();
    tracing::subscriber::set_global_default(buffer.subscriber()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let provider = axum::Router::new().route(
        "/v1/responses",
        axum::routing::post(move || {
            let n = counter.fetch_add(1, Ordering::Relaxed);
            async move {
                let too_many = axum::http::StatusCode::TOO_MANY_REQUESTS;
                match n {
                    0 => {
                        let event = json!({"type":"response.failed","response":{"error":{"code":"auth_unavailable",
                            "type":"invalid_request_error","message":"private-detail"}}});
                        ([("content-type", "text/event-stream")], format!("data: {event}\n\n")).into_response()
                    }
                    1 => {
                        let error = json!({"error":{"code":"turn_rate_limit_exceeded","message":"private-detail"}});
                        (too_many, [("retry-after", "300")], axum::Json(error)).into_response()
                    }
                    2 => {
                        let error = json!({"error":{"message":"Codex is at capacity: private-detail"}});
                        (too_many, [("retry-after", "1")], axum::Json(error)).into_response()
                    }
                    _ => axum::Json(json!({"id":"resp","object":"response","status":"completed","model":"gpt-6.1-sol",
                        "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}],
                        "usage":{"input_tokens":1,"output_tokens":1}}))
                    .into_response(),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
    let app = App::new(
        Config {
            auth_dir: "/nonexistent".into(),
            codex_api_key: vec![KeyEntry {
                api_key: "mock-only".into(),
                base_url: Some(base_url),
                ..Default::default()
            }],
            ..Default::default()
        },
        "/nonexistent/config.yaml".into(),
    );
    let call = |stream: bool| Call {
        format: Format::Responses,
        body: json!({"model":"gpt-6.1-sol","input":"question","stream":stream}),
        headers: HeaderMap::new(),
        stream,
        transport: "http",
        path_model: None,
        session: None,
        session_source: None,
        routing_selection: None,
        pinned_account: None,
    };
    assert!(matches!(execute(app.clone(), call(true)).await, Reply::Error(500, _)));
    assert!(matches!(execute(app.clone(), call(false)).await, Reply::Error(429, _)));
    assert!(matches!(execute(app.clone(), call(false)).await, Reply::Json(_)));
    assert_eq!(calls.load(Ordering::Relaxed), 4);
    let journal = buffer.text();
    let line =
        |text: &str| journal.lines().find(|l| l.contains(text)).unwrap_or_else(|| panic!("no {text}: {journal}"));
    let opened = line("codex stream opened with an error");
    assert!(opened.contains("error_code=auth_unavailable error_type=invalid_request_error"), "{opened}");
    let long = line("upstream at capacity for longer than a retry");
    assert!(long.contains("upstream_retry_after_seconds=300 client_retry_after_seconds=300"), "{long}");
    let retry = line("upstream busy; retrying");
    assert!(retry.contains("retry_delay_seconds=1 upstream_retry_after_seconds=1"), "{retry}");
    assert!(!journal.contains("private-detail"), "messages are not logged: {journal}");
}
