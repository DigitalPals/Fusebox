//! Provider diagnostics stay separate from requested cache controls and survive
//! each upstream transport without retaining prompts or comparison identifiers.
use super::*;
use crate::config::{Config, KeyEntry};

fn tracker(transport: &'static str) -> (Arc<App>, Tracker) {
    let app = App::new(
        Config {
            auth_dir: "/nonexistent".into(),
            codex_api_key: vec![KeyEntry { api_key: "local-mock-only".into(), ..Default::default() }],
            ..Default::default()
        },
        "/nonexistent/config.yaml".into(),
    );
    let mut tracker = Tracker::new(&app, Format::Responses, true, transport, "gpt-5.6");
    tracker.attempt(&app.pool.all()[0]);
    (app, tracker)
}

fn miss() -> Value {
    json!({"type":"cache_miss", "reason":"input_changed", "comparison_reusable_tokens":1024,
        "cache_missed_tokens":512, "comparison_response_id":"resp-private", "input":"private-prompt"})
}

fn response(diagnostic: Value) -> Value {
    json!({"id":"resp-result", "status":"completed", "output":[],
        "usage":{"input_tokens":2048,"output_tokens":0,"input_tokens_details":{"cached_tokens":1024}},
        "prompt_cache_diagnostics":diagnostic})
}

fn bytes(value: &Value) -> ByteStream {
    let bytes = bytes::Bytes::from(format!("data: {value}\n\n"));
    Box::pin(futures::stream::iter([Ok(bytes)]))
}

#[tokio::test]
async fn provider_cache_diagnostics_capture_full_http_and_native_websocket_wire_values() {
    for transport in ["http", "ws"] {
        let (app, mut tracker) = tracker(transport);
        let wire = if transport == "http" {
            response(miss())
        } else {
            json!({"type":"response.completed","response":response(miss())})
        };
        tracker.observe_wire(&wire);
        tracker.finish(200, &Usage::default(), None);
        let logs = app.stats.recent.lock();
        let log = &logs[0];
        let d = log.cache_diagnostics.as_ref().unwrap();
        assert_eq!(d.kind, "cache_miss");
        assert_eq!(d.reason, Some("input_changed"));
        assert_eq!(d.comparison_reusable_tokens, Some(1024));
        assert_eq!(d.cache_missed_tokens, Some(512));
        let saved = serde_json::to_string(log).unwrap();
        assert!(!saved.contains("resp-private"));
        assert!(!saved.contains("private-prompt"));
    }
}

#[tokio::test]
async fn provider_cache_diagnostics_survive_passthrough_and_collected_sse_without_rewriting_wire_data() {
    let wire = json!({"type":"response.completed", "response":response(miss())});
    let (app, tracker) = tracker("http");
    let mut stream = passthrough_stream(bytes(&wire), Format::Responses, tracker, false, 16 << 20);
    let frame = stream.next().await.unwrap();
    assert_eq!(serde_json::from_str::<Value>(&frame.data).unwrap(), wire);
    // Dropping after the terminal event must still retain diagnostics.
    drop(stream);
    assert_eq!(app.stats.recent.lock()[0].cache_diagnostics.as_ref().unwrap().reason, Some("input_changed"));

    let (app, tracker) = self::tracker("http");
    let reply = collect_passthrough(bytes(&wire), Format::Responses, tracker, Format::Responses, 16 << 20).await;
    let Reply::Json(body) = reply else { panic!("expected an unchanged response object") };
    assert_eq!(body, wire["response"]);
    assert_eq!(app.stats.recent.lock()[0].cache_diagnostics.as_ref().unwrap().kind, "cache_miss");
}

#[tokio::test]
async fn provider_cache_diagnostics_survive_translated_streams_and_collection() {
    for stream in [false, true] {
        let (app, tracker) = tracker("http");
        let wire = json!({"type":"response.completed", "response":response(miss())});
        let events = sse_events(
            bytes(&wire),
            Format::Responses,
            HashMap::new(),
            app.pool.all()[0].clone(),
            "gpt-5.6".into(),
            tracker.wire_tap(),
            16 << 20,
        );
        if stream {
            let mut frames = render_stream(events, Format::Chat, "gpt-5.6".into(), Arc::default(), tracker);
            while frames.next().await.is_some() {}
        } else {
            assert!(matches!(
                collect(events, Format::Chat, "gpt-5.6", &Request::default(), tracker).await,
                Reply::Json(_)
            ));
        }
        assert_eq!(app.stats.recent.lock()[0].cache_diagnostics.as_ref().unwrap().cache_missed_tokens, Some(512));
    }
}

#[tokio::test]
async fn provider_cache_diagnostics_survive_translated_full_http_responses() {
    let expected = response(miss());
    let data = expected.clone();
    let router = axum::Router::new().route("/", axum::routing::get(move || async move { axum::Json(data) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let response = reqwest::get(url).await.unwrap();
    let (app, tracker) = tracker("http");
    let events = full_events(
        response,
        Format::Responses,
        HashMap::new(),
        app.pool.all()[0].clone(),
        "gpt-5.6".into(),
        tracker.wire_tap(),
        16 << 20,
    );
    assert!(matches!(collect(events, Format::Chat, "gpt-5.6", &Request::default(), tracker).await, Reply::Json(_)));
    server.abort();
    assert_eq!(app.stats.recent.lock()[0].cache_diagnostics.as_ref().unwrap().kind, "cache_miss");
}

#[tokio::test]
async fn provider_cache_diagnostics_are_not_invented_from_policy_or_reused_after_retry() {
    let (app, mut tracker) = tracker("http");
    tracker.cache_policy(crate::cache::Policy::new(&json!({}), &json!({}), Format::Responses, "session", None));
    tracker.observe_wire(&json!({"prompt_cache_diagnostics":{"type":"cache_hit"}}));
    tracker.attempt(&app.pool.all()[0]);
    tracker.observe_wire(&json!({"output":[{"prompt_cache_diagnostics":{"type":"cache_hit"}}]}));
    tracker.finish(200, &Usage::default(), None);
    let logs = app.stats.recent.lock();
    assert!(logs[0].cache_policy.is_some());
    assert!(logs[0].cache_diagnostics.is_none());
}

#[test]
fn provider_cache_diagnostics_allow_only_known_outcomes_reasons_and_exact_nonnegative_counts() {
    let observer = crate::cache::DiagnosticObserver::default();
    for kind in ["cache_hit", "cache_miss", "comparison_response_not_found", "unavailable"] {
        observer.observe(&json!({"prompt_cache_diagnostics":{"type":kind}}));
        assert_eq!(observer.snapshot().unwrap().kind, kind);
    }
    for reason in [
        "model_changed",
        "prompt_cache_key_changed",
        "service_tier_changed",
        "tools_changed",
        "text_format_changed",
        "reasoning_effort_changed",
        "verbosity_changed",
        "context_compacted",
        "input_changed",
    ] {
        observer.observe(&json!({"prompt_cache_diagnostics":{"type":"cache_miss","reason":reason}}));
        assert_eq!(observer.snapshot().unwrap().reason, Some(reason));
    }
    for value in [json!(-1), json!(1.5), json!("1024"), json!(null), json!({}), json!(9_007_199_254_740_992_u64)] {
        observer.observe(&json!({"prompt_cache_diagnostics":{"type":"cache_miss","reason":"private-prompt",
            "comparison_reusable_tokens":value,"cache_missed_tokens":value}}));
        let d = observer.snapshot().unwrap();
        assert_eq!(d.reason, Some("unknown"));
        assert_eq!(d.comparison_reusable_tokens, None);
        assert_eq!(d.cache_missed_tokens, None);
        assert!(!serde_json::to_string(&d).unwrap().contains("private-prompt"));
    }
    observer.observe(&json!({"prompt_cache_diagnostics":{"type":"cache_miss","comparison_reusable_tokens":0,"cache_missed_tokens":0}}));
    assert_eq!(observer.snapshot().unwrap().cache_missed_tokens, Some(0));
    observer.observe(&json!({"prompt_cache_diagnostics":{"type":"future-private-outcome"}}));
    assert_eq!(observer.snapshot().unwrap().kind, "unavailable");
    for value in [json!(null), json!([]), json!("cache_hit"), json!(17)] {
        observer.reset();
        observer.observe(&json!({"prompt_cache_diagnostics":value}));
        assert!(observer.snapshot().is_none());
    }
}
