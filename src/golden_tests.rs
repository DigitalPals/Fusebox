//! Golden upstream requests: for each client format × provider wire × path
//! (native passthrough, cloaked, translated), the exact request Fusebox sends
//! upstream — method, URL, header names and values, and the body bytes. Any
//! change to what reaches a provider shows up here as a diff to review.
//!
//! The goldens live in `tests/golden/upstream/`. After a deliberate change,
//! rewrite them with `FUSEBOX_UPDATE_GOLDENS=1 cargo test golden_` and review
//! the diff. Values that differ on every run (generated UUIDs, the recorder's
//! address) are replaced by numbered placeholders; a placeholder that appears
//! twice is the same value twice.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, Uri};
use axum::response::{IntoResponse, Response};
use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::config::{CompatEntry, Config, KeyEntry, ModelAlias, Routing};
use crate::state::App;

/// One request as the provider received it.
struct Capture {
    method: String,
    uri: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<Capture>>,
}

const SSE: [(&str, &str); 1] = [("content-type", "text/event-stream")];

/// Records inference calls and answers each wire's minimal success; anything
/// else (quota or profile reads) is refused and not recorded.
async fn record(
    State(rec): State<Arc<Recorder>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_string();
    let inference = ["/responses", "/messages", "/chat/completions", ":generateContent", ":streamGenerateContent"]
        .iter()
        .any(|end| path.ends_with(end));
    if !inference {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    let mut names: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
        .collect();
    names.sort();
    rec.seen.lock().push(Capture {
        method: method.to_string(),
        uri: uri.path_and_query().map_or(path.clone(), |pq| pq.as_str().to_string()),
        headers: names,
        body: body.to_vec(),
    });
    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let stream = request["stream"] == true || path.ends_with(":streamGenerateContent");
    let model = request["model"].clone();
    let sse = |events: Vec<Value>| {
        let text: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
        (SSE, text).into_response()
    };
    if path.ends_with("/responses") {
        let done = json!({"id":"resp_golden","object":"response","status":"completed","model":model,
            "output":[{"type":"message","id":"msg_golden","role":"assistant","content":[{"type":"output_text","text":"ok"}]}],
            "usage":{"input_tokens":10,"output_tokens":1,"total_tokens":11}});
        if !stream {
            return axum::Json(done).into_response();
        }
        return sse(vec![
            json!({"type":"response.created","response":{"id":"resp_golden","model":model}}),
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"ok"}),
            json!({"type":"response.completed","response":done}),
        ]);
    }
    if path.ends_with("/messages") {
        if !stream {
            return axum::Json(json!({"id":"msg_golden","type":"message","role":"assistant","model":model,
                "content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn",
                "usage":{"input_tokens":10,"output_tokens":1}}))
            .into_response();
        }
        let events = [
            json!({"type":"message_start","message":{"id":"msg_golden","type":"message","role":"assistant","model":model,
                "content":[],"usage":{"input_tokens":10,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
            json!({"type":"message_stop"}),
        ];
        let text: String =
            events.iter().map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap())).collect();
        return (SSE, text).into_response();
    }
    if path.ends_with("/chat/completions") {
        if !stream {
            return axum::Json(json!({"id":"chatcmpl-golden","object":"chat.completion","model":model,
                "choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":10,"completion_tokens":1,"total_tokens":11}}))
            .into_response();
        }
        let chunk = |delta: Value, finish: Value| {
            json!({"id":"chatcmpl-golden","object":"chat.completion.chunk","model":model,
                "choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
        };
        let mut text: String =
            [chunk(json!({"role":"assistant","content":"ok"}), Value::Null), chunk(json!({}), json!("stop"))]
                .iter()
                .map(|e| format!("data: {e}\n\n"))
                .collect();
        text.push_str("data: [DONE]\n\n");
        return (SSE, text).into_response();
    }
    let answer = json!({"candidates":[{"content":{"role":"model","parts":[{"text":"ok"}]},"finishReason":"STOP"}],
        "usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":1,"totalTokenCount":11}});
    if stream { sse(vec![answer]) } else { axum::Json(answer).into_response() }
}

/// The provider account a case's Fusebox has, and the model it serves.
#[derive(Clone, Copy)]
enum Account {
    /// ChatGPT subscription (Codex wire, cloaked identity).
    CodexOAuth,
    /// OpenAI API key (Responses wire).
    CodexKey,
    /// Claude subscription (Messages wire, cloaked unless the client is Claude Code).
    ClaudeOAuth,
    /// Anthropic API key (Messages wire).
    ClaudeKey,
    /// Gemini API key (generateContent wire).
    GeminiKey,
    /// An OpenAI-compatible endpoint (Chat Completions wire).
    Compat,
}

impl Account {
    fn model(self) -> &'static str {
        match self {
            Account::CodexOAuth | Account::CodexKey => "gpt-6-astra",
            Account::ClaudeOAuth | Account::ClaudeKey => "claude-sonnet-4-6",
            Account::GeminiKey => "gemini-3.8-flash",
            Account::Compat => "golden-compat-model",
        }
    }
}

struct Golden {
    app: Arc<App>,
    _recorder_server: crate::routing_tests::Server,
    proxy: crate::routing_tests::Server,
    recorder: Arc<Recorder>,
    directory: PathBuf,
    recorder_url: String,
}

impl Drop for Golden {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

async fn golden(account: Account) -> Golden {
    let recorder = Arc::new(Recorder::default());
    let upstream = crate::routing_tests::serve(
        axum::Router::new()
            .fallback(record)
            .layer(axum::extract::DefaultBodyLimit::disable())
            .with_state(recorder.clone()),
    )
    .await;
    let url = upstream.url.clone();
    let directory = std::env::temp_dir().join(format!("fusebox-golden-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let mut cfg = Config {
        auth_dir: directory.to_string_lossy().into(),
        api_keys: vec!["client-one".into()],
        routing: Routing::FillFirst,
        ..Default::default()
    };
    let key = |base: String| KeyEntry { api_key: "golden-key".into(), base_url: Some(base), ..Default::default() };
    match account {
        Account::CodexOAuth => std::fs::write(
            directory.join("codex.json"),
            json!({"type":"codex","access_token":"golden-token","email":"golden@example.test","account_id":"golden-account","base_url":format!("{url}/v1")}).to_string(),
        )
        .unwrap(),
        Account::ClaudeOAuth => std::fs::write(
            directory.join("claude.json"),
            json!({"type":"claude","access_token":"golden-token","email":"golden@example.test","account_id":"golden-account","base_url":url}).to_string(),
        )
        .unwrap(),
        Account::CodexKey => cfg.codex_api_key = vec![key(format!("{url}/v1"))],
        Account::ClaudeKey => cfg.claude_api_key = vec![key(url.clone())],
        Account::GeminiKey => cfg.gemini_api_key = vec![key(url.clone())],
        Account::Compat => {
            cfg.openai_compatibility = vec![CompatEntry {
                name: "golden-compat".into(),
                base_url: format!("{url}/v1"),
                api_keys: vec!["golden-key".into()],
                models: vec![ModelAlias { name: "golden-compat-model".into(), alias: None }],
                ..Default::default()
            }]
        }
    }
    let app = App::new(cfg, directory.join("config.yaml"));
    let proxy = crate::routing_tests::serve(crate::server::router(app.clone())).await;
    Golden { app, _recorder_server: upstream, proxy, recorder, directory, recorder_url: url }
}

/// The client side of a case: the format it speaks and how it identifies.
#[derive(Clone, Copy)]
enum Client {
    Responses,
    Chat,
    Messages,
    Gemini,
    /// Claude Code itself, on the Messages route.
    ClaudeCode,
    /// The Codex CLI itself, on the Responses route.
    CodexCli,
}

fn tool_schema() -> Value {
    json!({"type":"object","properties":{"q":{"type":"string"}},"required":["q"]})
}

/// The request a client sends: a system instruction, a question, one tool and a token cap.
fn client_request(client: Client, model: &str, stream: bool) -> (String, Vec<(&'static str, String)>, Value) {
    let generic = vec![("user-agent", "golden-client/1.0".to_string())];
    match client {
        Client::Responses | Client::CodexCli => {
            let mut body = json!({"model":model,"instructions":"be brief",
                "input":[{"role":"user","content":"golden question"}],
                "tools":[{"type":"function","name":"lookup","parameters":tool_schema()}],"max_output_tokens":64});
            if stream {
                body["stream"] = true.into();
            }
            let headers = if matches!(client, Client::CodexCli) {
                vec![
                    ("user-agent", crate::upstream::CODEX_USER_AGENT.to_string()),
                    ("originator", "codex-tui".to_string()),
                    ("version", crate::upstream::CODEX_VERSION.to_string()),
                    ("session_id", "0195a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".to_string()),
                ]
            } else {
                generic
            };
            ("/v1/responses".into(), headers, body)
        }
        Client::Chat => {
            let mut body = json!({"model":model,"messages":[{"role":"system","content":"be brief"},{"role":"user","content":"golden question"}],
                "tools":[{"type":"function","function":{"name":"lookup","parameters":tool_schema()}}],"max_tokens":64,"temperature":0.5});
            if stream {
                body["stream"] = true.into();
            }
            ("/v1/chat/completions".into(), generic, body)
        }
        Client::Messages | Client::ClaudeCode => {
            let mut body = json!({"model":model,"max_tokens":64,"system":"be brief",
                "messages":[{"role":"user","content":"golden question"}],
                "tools":[{"name":"lookup","input_schema":tool_schema()}]});
            if stream {
                body["stream"] = true.into();
            }
            let headers = if matches!(client, Client::ClaudeCode) {
                vec![
                    ("user-agent", crate::upstream::CC_USER_AGENT.to_string()),
                    ("x-app", "cli".to_string()),
                    ("x-claude-code-session-id", "0195a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".to_string()),
                    ("anthropic-version", "2023-06-01".to_string()),
                    ("anthropic-beta", "claude-code-20250219,interleaved-thinking-2025-05-14".to_string()),
                    ("x-stainless-package-version", "0.128.0".to_string()),
                    ("x-stainless-runtime-version", "v26.12.1".to_string()),
                ]
            } else {
                generic
            };
            ("/v1/messages".into(), headers, body)
        }
        Client::Gemini => {
            let action = if stream { "streamGenerateContent?alt=sse" } else { "generateContent" };
            let body = json!({"systemInstruction":{"parts":[{"text":"be brief"}]},
                "contents":[{"role":"user","parts":[{"text":"golden question"}]}],
                "tools":[{"functionDeclarations":[{"name":"lookup","parameters":tool_schema()}]}],
                "generationConfig":{"maxOutputTokens":64}});
            (format!("/v1beta/models/{model}:{action}"), generic, body)
        }
    }
}

/// Replaces generated values with numbered placeholders, in order of first
/// appearance: UUIDs become `<uuid-N>`, and 64-digit hex ids (Claude's
/// per-install device id) become `<hex64-N>`.
fn placeholders(text: &str, ids: &mut Vec<String>) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let number = |id: &str, ids: &mut Vec<String>| {
        ids.iter().position(|seen| seen == id).unwrap_or_else(|| {
            ids.push(id.to_string());
            ids.len() - 1
        }) + 1
    };
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes[i..].iter().take_while(|b| b.is_ascii_hexdigit()).count();
        let starts = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if starts && i + 36 <= bytes.len() && uuid_at(&bytes[i..i + 36]) {
            out.push_str(&format!("<uuid-{}>", number(&text[i..i + 36], ids)));
            i += 36;
        } else if starts && hex == 64 && !bytes.get(i + 64).is_some_and(u8::is_ascii_alphanumeric) {
            out.push_str(&format!("<hex64-{}>", number(&text[i..i + 64], ids)));
            i += 64;
        } else {
            let c = text[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

fn uuid_at(b: &[u8]) -> bool {
    b.iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

/// The golden text for what a case sent upstream (or why nothing was sent).
fn render(golden: &Golden, status: u16, reply: &str) -> String {
    let seen = golden.recorder.seen.lock();
    let mut ids = Vec::new();
    let mut out = String::new();
    if seen.is_empty() {
        return format!("no upstream request; the client got {status}\n{reply}\n");
    }
    for capture in seen.iter() {
        out.push_str(&format!("{} {}\n", capture.method, placeholders(&capture.uri, &mut ids)));
        for (name, value) in &capture.headers {
            let value = if name == "host" {
                "<upstream>".to_string()
            } else {
                value.replace(&golden.recorder_url, "<upstream>")
            };
            out.push_str(&format!("{name}: {}\n", placeholders(&value, &mut ids)));
        }
        out.push('\n');
        out.push_str(&placeholders(&String::from_utf8_lossy(&capture.body), &mut ids));
        out.push_str("\n\n");
    }
    out.push_str(&format!("client status: {status}\n"));
    out
}

async fn run_case(name: &str, account: Account, client: Client, stream: bool) -> Result<(), String> {
    let golden = golden(account).await;
    let (path, headers, body) = client_request(client, account.model(), stream);
    let mut request = reqwest::Client::new().post(format!("{}{}", golden.proxy.url, path)).bearer_auth("client-one");
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let response = request.json(&body).send().await.unwrap();
    let status = response.status().as_u16();
    let reply = response.text().await.unwrap();
    let got = render(&golden, status, &reply);
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/upstream").join(format!("{name}.txt"));
    if std::env::var_os("FUSEBOX_UPDATE_GOLDENS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, &got).unwrap();
        return Ok(());
    }
    let want = std::fs::read_to_string(&file).map_err(|e| format!("{name}: no golden at {} ({e})", file.display()))?;
    if want == got {
        return Ok(());
    }
    let line = want
        .lines()
        .zip(got.lines())
        .position(|(w, g)| w != g)
        .unwrap_or(want.lines().count().min(got.lines().count()));
    Err(format!(
        "{name}: upstream request changed at line {}\n  golden: {:?}\n  now:    {:?}",
        line + 1,
        want.lines().nth(line).unwrap_or("<end>"),
        got.lines().nth(line).unwrap_or("<end>")
    ))
}

/// Every case: client × account, plus the streaming and native-client paths.
fn cases() -> Vec<(String, Account, Client, bool)> {
    let accounts = [
        ("codex-oauth", Account::CodexOAuth),
        ("codex-key", Account::CodexKey),
        ("claude-oauth", Account::ClaudeOAuth),
        ("claude-key", Account::ClaudeKey),
        ("gemini-key", Account::GeminiKey),
        ("compat", Account::Compat),
    ];
    let clients = [
        ("responses", Client::Responses),
        ("chat", Client::Chat),
        ("messages", Client::Messages),
        ("gemini", Client::Gemini),
    ];
    let mut out = Vec::new();
    for (client_name, client) in clients {
        for (account_name, account) in accounts {
            out.push((format!("{client_name}-to-{account_name}"), account, client, false));
        }
    }
    for (name, account, client) in [
        ("responses-stream-to-codex-key", Account::CodexKey, Client::Responses),
        ("chat-stream-to-compat", Account::Compat, Client::Chat),
        ("messages-stream-to-claude-key", Account::ClaudeKey, Client::Messages),
        ("gemini-stream-to-gemini-key", Account::GeminiKey, Client::Gemini),
        ("messages-stream-to-claude-oauth", Account::ClaudeOAuth, Client::Messages),
        ("claude-code-to-claude-oauth", Account::ClaudeOAuth, Client::ClaudeCode),
        ("codex-cli-to-codex-oauth", Account::CodexOAuth, Client::CodexCli),
    ] {
        out.push((name.to_string(), account, client, true));
    }
    out
}

#[tokio::test]
async fn golden_upstream_requests_match() {
    let mut failures = Vec::new();
    for (name, account, client, stream) in cases() {
        if let Err(e) = run_case(&name, account, client, stream).await {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "{} golden(s) differ:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn placeholders_number_each_uuid_once() {
    let mut ids = Vec::new();
    let a = "0195a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
    let b = "11111111-2222-3333-4444-555555555555";
    assert_eq!(placeholders(&format!("{a} {b} {a}"), &mut ids), "<uuid-1> <uuid-2> <uuid-1>");
    let device = "8d6d382debe17ac8d0f6f7076f5fd8de4c952e2290b1db8fb227b1e323104db9";
    assert_eq!(placeholders(&format!("\"{device}\" {a}"), &mut ids), "\"<hex64-3>\" <uuid-1>");
    assert_eq!(placeholders(&format!("x{device}"), &mut Vec::new()), format!("x{device}"), "only whole tokens");
}

/// Generated values differ between an inspection and the real request, so
/// both sides drop them before comparing.
fn generic(text: &str) -> String {
    let mut ids = Vec::new();
    let numbered = placeholders(text, &mut ids);
    let mut out = String::new();
    let mut rest = numbered.as_str();
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        match ["<uuid-", "<hex64-"].iter().find(|p| tail.starts_with(**p)).and_then(|_| tail.find('>')) {
            Some(end) => {
                out.push_str(&tail[..tail[1..].find('-').unwrap() + 1]);
                out.push('>');
                rest = &tail[end + 1..];
            }
            None => {
                out.push('<');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn client_format(client: Client) -> crate::ir::Format {
    match client {
        Client::Responses | Client::CodexCli => crate::ir::Format::Responses,
        Client::Chat => crate::ir::Format::Chat,
        Client::Messages | Client::ClaudeCode => crate::ir::Format::Claude,
        Client::Gemini => crate::ir::Format::Gemini,
    }
}

/// `/api/inspect` shows exactly what the real request then sends (credentials
/// and account ids aside), and inspecting reaches no provider and leaves no
/// log or session behind.
#[tokio::test]
async fn inspection_shows_what_is_sent_and_changes_nothing() {
    let mut failures = Vec::new();
    for (name, account, client, stream) in cases() {
        let golden = golden(account).await;
        let (path, headers, body) = client_request(client, account.model(), stream);
        let mut map = HeaderMap::new();
        for (k, v) in &headers {
            map.insert(*k, v.parse().unwrap());
        }
        let inspected = crate::inspect::inspect_call(
            &golden.app,
            crate::proxy::Call {
                format: client_format(client),
                body: body.clone(),
                headers: map,
                stream: stream || body["stream"] == true,
                transport: "inspect",
                path_model: matches!(client, Client::Gemini).then(|| account.model().to_string()),
                session: None,
                session_source: None,
                routing_selection: None,
                pinned_account: None,
            },
        );
        assert!(golden.recorder.seen.lock().is_empty(), "{name}: inspecting sent a request");
        assert!(golden.app.stats.recent.lock().is_empty(), "{name}: inspecting logged a request");
        assert!(golden.app.sessions.session_counts(3600).is_empty(), "{name}: inspecting assigned a session");
        assert!(inspected.get("refusal").is_none(), "{name}: {inspected}");
        let mut request =
            reqwest::Client::new().post(format!("{}{}", golden.proxy.url, path)).bearer_auth("client-one");
        for (k, v) in headers {
            request = request.header(k, v);
        }
        assert_eq!(request.json(&body).send().await.unwrap().status(), 200, "{name}");
        let seen = golden.recorder.seen.lock();
        let real = &seen[0];
        let shown = &inspected["request"];
        let url = shown["url"].as_str().unwrap().replace(&golden.recorder_url, "");
        if generic(&url) != generic(&real.uri) {
            failures.push(format!("{name}: url {url} vs {}", real.uri));
        }
        let real_body: Value = serde_json::from_slice(&real.body).unwrap();
        let mut redacted = real_body.clone();
        crate::inspect::redact_for_tests(&mut redacted);
        if generic(&shown["body"].to_string()) != generic(&redacted.to_string()) {
            failures.push(format!("{name}: body\n  shown {}\n  sent  {redacted}", shown["body"]));
        }
        for pair in shown["headers"].as_array().unwrap() {
            let (k, v) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
            let sent = real.headers.iter().find(|(name, _)| name.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str());
            if v != "<redacted>" && sent.map(generic) != Some(generic(v)) {
                failures.push(format!("{name}: header {k}: shown {v:?}, sent {sent:?}"));
            }
        }
        let path_shown = inspected["shape"]["path"].as_str().unwrap();
        let native = matches!(
            (client, account),
            (Client::Responses | Client::CodexCli, Account::CodexOAuth | Account::CodexKey)
                | (Client::Chat, Account::Compat)
                | (Client::Messages | Client::ClaudeCode, Account::ClaudeOAuth | Account::ClaudeKey)
                | (Client::Gemini, Account::GeminiKey)
        );
        if path_shown != if native { "passthrough" } else { "translated" } {
            failures.push(format!("{name}: path {path_shown}"));
        }
    }
    assert!(failures.is_empty(), "{} difference(s):\n{}", failures.len(), failures.join("\n"));
}

#[tokio::test]
async fn the_inspect_route_needs_management_auth_and_explains_refusals() {
    let golden = golden(Account::CodexKey).await;
    let mut cfg = (*golden.app.cfg()).clone();
    cfg.management_key = "fake-management-key".into();
    golden.app.set_config(cfg);
    let url = format!("{}/api/inspect", golden.proxy.url);
    let client = reqwest::Client::new();
    let body = json!({"body": {"model": "gpt-6-astra", "input": "golden question"}});
    assert_eq!(client.post(&url).json(&body).send().await.unwrap().status(), 401);
    let shown: Value =
        client.post(&url).bearer_auth("fake-management-key").json(&body).send().await.unwrap().json().await.unwrap();
    assert_eq!((shown["format"].as_str(), shown["sent"].as_bool()), (Some("responses"), Some(false)), "{shown}");
    assert_eq!(shown["shape"]["path"], "passthrough");
    assert_eq!(shown["account"]["provider"], "codex");
    assert!(shown["request"]["url"].as_str().unwrap().ends_with("/v1/responses"), "{shown}");
    let headers = shown["request"]["headers"].as_array().unwrap();
    assert!(headers.iter().any(|h| h[0] == "authorization" && h[1] == "<redacted>"), "{shown}");
    for (body, want) in [
        (json!({"format": "telnet", "body": {"model": "gpt-6-astra"}}), 400),
        (json!({"client": "nobody", "body": {"model": "gpt-6-astra"}}), 400),
        (json!({"body": "not an object"}), 400),
    ] {
        let status = client.post(&url).bearer_auth("fake-management-key").json(&body).send().await.unwrap().status();
        assert_eq!(status, want, "{body}");
    }
    let missing: Value = client
        .post(&url)
        .bearer_auth("fake-management-key")
        .json(&json!({"body": {"model": "no-such-model", "messages": [{"role": "user", "content": "hi"}]}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(missing["refusal"]["status"], 404, "{missing}");
    // A pasted request may be as large as a client's (beyond axum's 2 MB default).
    let long = json!({"body": {"model": "gpt-6-astra", "input": "x".repeat(3 << 20)}});
    let status = client.post(&url).bearer_auth("fake-management-key").json(&long).send().await.unwrap().status();
    assert_eq!(status, 200);
    assert!(golden.recorder.seen.lock().is_empty(), "nothing was sent");
}

fn inspect_chat(app: &App, model: &str, headers: &[(&'static str, &str)]) -> Value {
    let mut map = HeaderMap::new();
    for (k, v) in headers {
        map.insert(*k, v.parse().unwrap());
    }
    crate::inspect::inspect_call(
        app,
        crate::proxy::Call {
            format: crate::ir::Format::Responses,
            // No user content, so no session is inferred: a balanced group would take a turn.
            body: json!({"model": model, "input": []}),
            headers: map,
            stream: false,
            transport: "inspect",
            path_model: None,
            session: None,
            session_source: None,
            routing_selection: None,
            pinned_account: None,
        },
    )
}

#[tokio::test]
async fn inspecting_a_balanced_group_takes_no_turn() {
    let golden = golden(Account::CodexKey).await;
    let mut cfg = (*golden.app.cfg()).clone();
    let group = crate::config::ModelGroup { models: vec!["gpt-6-astra".into(), "gpt-6.1-sol".into()], balance: true };
    cfg.model_groups.insert("golden-duo".into(), group.clone());
    golden.app.set_config(cfg);
    let preview = |g: &crate::config::ModelGroup| -> Vec<String> {
        crate::proxy::group_order_preview(g, None).into_iter().map(String::from).collect()
    };
    let before = preview(&group);
    for _ in 0..3 {
        let shown = inspect_chat(&golden.app, "golden-duo", &[]);
        assert_eq!(shown["group"]["order"], json!(before), "{shown}");
    }
    assert_eq!(preview(&group), before, "the next real request starts where it would have");
}

#[tokio::test]
async fn inspection_skips_accounts_execution_skips() {
    // A higher-priority OpenAI key and a ChatGPT subscription: a Codex residency
    // request can only go to the subscription, in inspection as in execution.
    let golden = golden(Account::CodexOAuth).await;
    let mut cfg = (*golden.app.cfg()).clone();
    cfg.codex_api_key = vec![KeyEntry {
        api_key: "golden-key".into(),
        base_url: Some(format!("{}/v1", golden.recorder_url)),
        priority: 10,
        ..Default::default()
    }];
    golden.app.set_config(cfg);
    golden.app.reload_accounts();
    let residency = [("x-openai-internal-codex-residency", "us")];
    let plain = inspect_chat(&golden.app, "gpt-6-astra", &[]);
    assert_eq!(plain["account"]["subscription"], false, "the key goes first otherwise: {plain}");
    let shown = inspect_chat(&golden.app, "gpt-6-astra", &residency);
    assert!(shown.get("refusal").is_none(), "{shown}");
    assert_eq!(shown["account"]["subscription"], true, "{shown}");
    assert_eq!(shown["ineligible"].as_array().map(Vec::len), Some(1), "{shown}");
    assert!(golden.recorder.seen.lock().is_empty());
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", golden.proxy.url))
        .bearer_auth("client-one")
        .header(residency[0].0, residency[0].1)
        .json(&json!({"model": "gpt-6-astra", "input": "golden question"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let seen = golden.recorder.seen.lock();
    let auth = seen[0].headers.iter().find(|(k, _)| k == "authorization").map(|(_, v)| v.as_str());
    assert_eq!(auth, Some("Bearer golden-token"), "execution used the subscription too");
}

#[tokio::test]
async fn inspection_follows_group_failover_and_expands_continuations() {
    let golden = golden(Account::CodexKey).await;
    let mut cfg = (*golden.app.cfg()).clone();
    let group =
        crate::config::ModelGroup { models: vec!["golden-unserved".into(), "gpt-6-astra".into()], balance: false };
    cfg.model_groups.insert("golden-fallback".into(), group);
    golden.app.set_config(cfg);
    // The group's first model has no account; execution moves on, and so does the preview.
    let shown = inspect_chat(&golden.app, "golden-fallback", &[]);
    assert!(shown.get("refusal").is_none(), "{shown}");
    assert_eq!(shown["model"], "gpt-6-astra", "{shown}");
    assert_eq!(shown["group"]["tried"], json!(["golden-unserved", "gpt-6-astra"]), "{shown}");

    // A stored response is expanded into the continuation's input, as when sent.
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", golden.proxy.url))
        .bearer_auth("client-one")
        .json(&json!({"model": "gpt-6-astra", "input": "golden first turn"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let scope = crate::affinity::scope_for_key(Some("client-one"));
    let mut headers = HeaderMap::new();
    headers.insert(crate::affinity::SCOPE_HEADER, scope.parse().unwrap());
    let call = |body: Value| crate::proxy::Call {
        format: crate::ir::Format::Responses,
        body,
        headers: headers.clone(),
        stream: false,
        transport: "inspect",
        path_model: None,
        session: None,
        session_source: None,
        routing_selection: None,
        pinned_account: None,
    };
    let shown = crate::inspect::inspect_call(
        &golden.app,
        call(json!({"model": "gpt-6-astra", "previous_response_id": "resp_golden", "input": "golden second turn"})),
    );
    assert!(shown["continuation"].as_str().unwrap().starts_with("expanded"), "{shown}");
    let body = shown["request"]["body"].to_string();
    assert!(body.contains("golden first turn") && body.contains("golden second turn"), "{body}");
    assert!(!body.contains("previous_response_id"), "{body}");
    let unknown = crate::inspect::inspect_call(
        &golden.app,
        call(json!({"model": "gpt-6-astra", "previous_response_id": "resp_unknown", "input": "x"})),
    );
    assert!(unknown["continuation"].as_str().unwrap().starts_with("not in"), "{unknown}");
    assert_eq!(golden.recorder.seen.lock().len(), 1, "only the real first turn was sent");
}
