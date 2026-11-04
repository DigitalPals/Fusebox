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
    let proxy = crate::routing_tests::serve(crate::server::router(app)).await;
    Golden { _recorder_server: upstream, proxy, recorder, directory, recorder_url: url }
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

