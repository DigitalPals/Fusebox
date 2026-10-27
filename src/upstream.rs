//! Builds the HTTP request for each upstream provider.

use axum::http::HeaderMap;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

use crate::accounts::{Account, Credential, Provider};
use crate::config::Config;
use crate::device;
use crate::ir::Format;

pub const CLAUDE_API: &str = "https://api.anthropic.com";
pub const CODEX_BACKEND: &str = "https://chatgpt.com/backend-api/codex";
pub const OPENAI_API: &str = "https://api.openai.com/v1";
pub const GEMINI_API: &str = "https://generativelanguage.googleapis.com";

pub const CC_VERSION: &str = "2.1.309";
pub const CC_USER_AGENT: &str = "claude-cli/2.1.309 (external, cli)";
const CC_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const CC_FINGERPRINT_SALT: &str = "59cf53e54c78";
pub const CODEX_USER_AGENT: &str = "codex-tui/0.164.0 (Mac OS 26.5.2; arm64) iTerm.app/3.6.11 (codex-tui; 0.164.0)";
pub const CODEX_ORIGINATOR: &str = "codex-tui";
/// The Codex release `CODEX_USER_AGENT` claims; its model list depends on it.
pub const CODEX_VERSION: &str = "0.164.0";
pub const CODEX_WS_BETA: &str = "responses_websockets=2026-02-06";

pub struct Prepared {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
    /// Pre-encoded body (Devin's protobuf); `body` is then only for logging.
    pub raw: Option<Vec<u8>>,
}

pub struct Target<'a> {
    pub acct: &'a Account,
    pub cfg: &'a Config,
    pub client_headers: &'a HeaderMap,
    pub model: &'a str,
    /// Dialect of `body` (one of the provider's wires).
    pub wire: Format,
    pub passthrough: bool,
    pub stream: bool,
    pub count_tokens: bool,
    /// A stable per-session key for providers that route their prompt cache by one.
    pub cache_key: Option<&'a str>,
    /// Provider-visible identity for this conversation's stint on the selected account.
    pub provider_session: Option<&'a str>,
    /// Whether final Claude preparation may add default cache boundaries.
    pub cache_defaults: bool,
}

/// Headers never copied from the client to an upstream.
const HOP: &[&str] = &[
    "host",
    "authorization",
    "x-api-key",
    "x-goog-api-key",
    "content-length",
    "connection",
    "accept-encoding",
    "transfer-encoding",
    "cookie",
    "upgrade",
    "te",
    "keep-alive",
    "proxy-authorization",
    "proxy-authenticate",
    "proxy-connection",
    "trailer",
    "via",
    "set-cookie",
    "www-authenticate",
    "x-fusebox-usage-request",
    "x-fusebox-usage-client",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-real-ip",
    "forwarded",
    "origin",
    "referer",
    "sec-websocket-key",
    "sec-websocket-version",
    "sec-websocket-extensions",
    "sec-websocket-protocol",
];

/// Whether a client header may be copied upstream. Fusebox's own headers (session ids,
/// the client scope, usage tags) are for Fusebox alone, whatever their spelling.
/// Where a client is (forwarding headers, the Tailscale user) is not the provider's business.
fn forwardable(name: &str) -> bool {
    !HOP.contains(&name)
        && !name.starts_with("x-fusebox-")
        && !name.starts_with("x-cliproxy-")
        && !name.starts_with("tailscale-")
        && !name.starts_with("x-forwarded-")
        && !name.starts_with("x-real-")
        && !name.starts_with("x-auth-")
        && !name.starts_with("x-authenticated-")
        && !name.starts_with("x-original-")
        && !name.starts_with("x-envoy-")
        && !name.starts_with("x-amzn-")
        && !name.starts_with("cf-")
        && !name.starts_with("sec-")
        && !name.split(['-', '_']).any(|part| {
            matches!(
                part,
                "authorization"
                    | "authentication"
                    | "auth"
                    | "apikey"
                    | "key"
                    | "token"
                    | "secret"
                    | "password"
                    | "credential"
                    | "credentials"
                    | "cookie"
                    | "internal"
                    | "private"
                    | "forwarded"
                    | "gateway"
                    | "management"
                    | "proxy"
            )
        })
}

const CODEX_RESIDENCY: &str = "x-openai-internal-codex-residency";
const CODEX_HEADERS: &[&str] = &[
    "user-agent",
    "originator",
    "version",
    "session_id",
    "session-id",
    "thread-id",
    "x-codex-turn-metadata",
    "x-codex-turn-state",
    "x-codex-beta-features",
    "x-client-request-id",
    "x-codex-window-id",
    "x-openai-internal-codex-responses-lite",
    CODEX_RESIDENCY,
];

fn safe_header(provider: Provider, name: &str) -> bool {
    axum::http::HeaderName::from_bytes(name.as_bytes()).is_ok()
        && (forwardable(name)
            || (provider == Provider::Codex
                && matches!(name, CODEX_RESIDENCY | "x-openai-internal-codex-responses-lite")))
}

fn reserved_header(name: &str) -> bool {
    matches!(
        name,
        "chatgpt-account-id"
            | "session_id"
            | "session-id"
            | "x-claude-code-session-id"
            | "user-agent"
            | "originator"
            | "version"
            | "anthropic-version"
            | "anthropic-beta"
            | "openai-beta"
            | "content-type"
            | "accept"
            | "x-client-request-id"
            | "x-app"
    )
}

/// Extensions cannot override authentication, provider identity or transport controls.
pub fn header_extension_allowed(provider: Provider, name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    safe_header(provider, &name) && !reserved_header(&name)
}

/// Operator-supplied gateway credentials are intentional on API/custom routes.
/// Subscription identities retain the stricter provider privacy policy.
pub fn custom_header_allowed(provider: Provider, oauth: bool, name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    axum::http::HeaderName::from_bytes(name.as_bytes()).is_ok()
        && !HOP.contains(&name.as_str())
        && !name.starts_with("x-fusebox-")
        && !name.starts_with("x-cliproxy-")
        && !reserved_header(&name)
        && (!(oauth && matches!(provider, Provider::Claude | Provider::Codex)) || safe_header(provider, &name))
}

fn connection_headers(client: &HeaderMap) -> HashSet<String> {
    client
        .get_all("connection")
        .iter()
        .flat_map(|value| value.as_bytes().split(|byte| *byte == b','))
        .filter_map(|value| std::str::from_utf8(value).ok())
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect()
}

/// Validate a managed-residency constraint before routing, without reflecting caller values.
pub fn residency_requirement(headers: &HeaderMap) -> Result<Option<String>, String> {
    if headers.contains_key(CODEX_RESIDENCY) && connection_headers(headers).contains(CODEX_RESIDENCY) {
        return Err("Codex residency cannot be nominated by Connection".into());
    }
    let mut requirement: Option<String> = None;
    for value in headers.get_all(CODEX_RESIDENCY) {
        let value = value.to_str().map_err(|_| "Invalid Codex residency requirement")?;
        if value.is_empty()
            || value.len() > 64
            || value.trim() != value
            || !value.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || !value.as_bytes()[0].is_ascii_lowercase()
            || value.ends_with('-')
        {
            return Err("Invalid Codex residency requirement".into());
        }
        if requirement.as_deref().is_some_and(|existing| existing != value) {
            return Err("Conflicting Codex residency requirements".into());
        }
        requirement = Some(value.into());
    }
    Ok(requirement)
}

fn inbound_headers(provider: Provider, cfg: Option<&Config>, client: &HeaderMap) -> Vec<(String, String)> {
    let nominated = connection_headers(client);
    let extensions = cfg.and_then(|cfg| cfg.upstream_header_allowlist.get(provider.as_str()));
    client
        .iter()
        .filter_map(|(name, value)| {
            let name = name.as_str();
            let documented = match provider {
                Provider::Claude => {
                    name.starts_with("anthropic-")
                        || name.starts_with("x-claude-code-")
                        || name.starts_with("x-stainless-")
                        || matches!(name, "user-agent" | "x-app" | "x-client-request-id" | "x-request-id")
                }
                Provider::Codex => CODEX_HEADERS.contains(&name),
                _ => false,
            };
            let extended = header_extension_allowed(provider, name)
                && extensions.is_some_and(|names| names.iter().any(|n| n.eq_ignore_ascii_case(name)));
            if !nominated.contains(name)
                && safe_header(provider, name)
                && (documented || extended)
                && let Ok(value) = value.to_str()
            {
                return Some((name.to_string(), value.to_string()));
            }
            tracing::debug!(provider = provider.as_str(), header = name, "dropping upstream header");
            None
        })
        .collect()
}

fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: String) {
    headers.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
    headers.push((name.to_string(), value));
}

fn header(h: &HeaderMap, name: &str) -> Option<String> {
    h.get(name).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn is_claude_code(h: &HeaderMap) -> bool {
    crate::clients::is_claude_code(h)
}

/// A request-local memoization stamp. Credentials themselves are never retained
/// in a cache key or diagnostics, and token rotation invalidates prepared headers.
pub fn credential_stamp(acct: &Account) -> String {
    let (token, _, _, _) = creds(acct);
    stamp([token, backend_stamp(acct), acct.state.lock().identity_epoch.to_string()])
}

/// The account's base URL, provider account id and configured custom headers.
/// A header-only reload keeps the credential identity, so caches of prepared
/// requests or upstream results must include this as well.
pub fn backend_stamp(acct: &Account) -> String {
    let (_, base, _, account) = creds(acct);
    let headers = acct.headers.iter().flat_map(|(k, v)| [k.clone(), v.clone()]);
    stamp([base.unwrap_or_default(), account.unwrap_or_default()].into_iter().chain(headers))
}

fn stamp(values: impl IntoIterator<Item = String>) -> String {
    let mut hash = Sha256::new();
    for value in values {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
    }
    hex::encode(hash.finalize())
}

fn creds(acct: &Account) -> (String, Option<String>, bool, Option<String>) {
    match &*acct.cred.read() {
        Credential::OAuth(o) => (
            o.access_token.clone(),
            o.base_url.as_ref().map(|b| b.trim_end_matches('/').to_string()),
            true,
            o.account_id.clone(),
        ),
        Credential::ApiKey { key, base_url } => (
            key.clone(),
            base_url.clone().filter(|b| !b.trim().is_empty()).map(|b| b.trim_end_matches('/').to_string()),
            false,
            None,
        ),
    }
}

fn push_custom(acct: &Account, client: &HeaderMap, headers: &mut Vec<(String, String)>) {
    let nominated = connection_headers(client);
    for (k, v) in &acct.headers {
        let name = k.to_ascii_lowercase();
        if !custom_header_allowed(acct.provider, acct.is_oauth(), &name)
            || nominated.contains(&name)
            || axum::http::HeaderValue::from_str(v).is_err()
        {
            tracing::debug!(provider = acct.provider.as_str(), header = name, "dropping configured upstream header");
            continue;
        }
        // A caller's residency requirement must survive account configuration.
        if name == CODEX_RESIDENCY && headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            continue;
        }
        set_header(headers, &name, v.clone());
    }
}

pub fn prepare(t: &Target, body: Value) -> Prepared {
    use crate::accounts::Provider::*;
    let mut p = match t.acct.provider {
        Claude => claude(t, body),
        Codex => codex(t, body),
        Gemini => gemini(t, body),
        Vertex => vertex(t, body),
        Antigravity => antigravity(t, body),
        Kimi => kimi(t, body),
        Xai | Meta => responses_api(t, body),
        Devin => crate::devin::prepare(t, body),
        Compat => compat(t, body),
    };
    push_custom(t.acct, t.client_headers, &mut p.headers);
    p
}

// ---------------------------------------------------------------------- claude

fn claude(t: &Target, mut body: Value) -> Prepared {
    let (token, base, oauth, account_uuid) = creds(t.acct);
    let base = base.unwrap_or_else(|| CLAUDE_API.into());
    let path = if t.count_tokens { "/v1/messages/count_tokens" } else { "/v1/messages" };
    let url = if oauth { format!("{base}{path}?beta=true") } else { format!("{base}{path}") };
    body["model"] = t.model.into();
    strip_foreign_thinking(&mut body);
    crate::formats::claude::normalize_disabled_thinking(&mut body, t.model);

    let uncloaked = !oauth || !t.cfg.claude_cloak;
    if !t.passthrough && t.cache_defaults && uncloaked && !t.count_tokens && !crate::cache::has_markers(&body) {
        body["cache_control"] = json!({"type":"ephemeral"});
    }
    let native_cc = t.passthrough && is_claude_code(t.client_headers);
    let mut headers = inbound_headers(Provider::Claude, Some(t.cfg), t.client_headers);

    if !native_cc && oauth && t.cfg.claude_cloak {
        // A generated Node/Claude Code profile must not retain another SDK's
        // async/runtime/helper headers. Operator extensions are explicit.
        let extensions = t.cfg.upstream_header_allowlist.get(Provider::Claude.as_str());
        headers.retain(|(name, _)| {
            name == "anthropic-beta"
                || (header_extension_allowed(Provider::Claude, name)
                    && extensions.is_some_and(|names| names.iter().any(|n| n.eq_ignore_ascii_case(name))))
        });
        let mut cc = vec![
            "claude-code-20250219",
            "oauth-2025-04-20",
            "interleaved-thinking-2025-05-14",
            "context-management-2025-06-27",
            "prompt-caching-scope-2026-01-05",
        ];
        if body["output_config"]["effort"].is_string() && !has_claude_beta_family(&headers, "effort-") {
            cc.push("effort-2025-11-24");
        }
        if body["output_config"]["format"].is_object() {
            cc.push("structured-outputs-2025-12-15");
        }
        for beta in cc {
            ensure_claude_beta(&mut headers, beta);
        }
        cloak_body(&mut body, t.count_tokens, t.cache_defaults);
        // The newest Claude Code release known, with the runtime headers the user's
        // own installation sends (see `clients`).
        let (user_agent, profile) = crate::clients::claude_identity();
        let learned = |name: &str| profile.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone()).unwrap_or_default();
        for (name, value) in [
            ("anthropic-version", "2023-06-01".to_string()),
            ("anthropic-dangerous-direct-browser-access", "true".to_string()),
            ("x-app", "cli".to_string()),
            ("user-agent", user_agent),
            ("x-stainless-lang", "js".to_string()),
            ("x-stainless-package-version", learned("x-stainless-package-version")),
            ("x-stainless-os", learned("x-stainless-os")),
            ("x-stainless-arch", learned("x-stainless-arch")),
            ("x-stainless-runtime", "node".to_string()),
            ("x-stainless-runtime-version", learned("x-stainless-runtime-version")),
            ("x-stainless-retry-count", "0".to_string()),
            ("x-stainless-timeout", "600".to_string()),
        ] {
            set_header(&mut headers, name, value);
        }
    } else if !native_cc && body["output_config"]["effort"].is_string() && !has_claude_beta_family(&headers, "effort-")
    {
        ensure_claude_beta(&mut headers, "effort-2025-11-24");
    }
    if !headers.iter().any(|(name, _)| name == "anthropic-version") {
        headers.push(("anthropic-version".into(), "2023-06-01".into()));
    }
    if !headers.iter().any(|(name, _)| name == "user-agent") {
        headers.push(("user-agent".into(), format!("Fusebox/{}", env!("CARGO_PKG_VERSION"))));
    }
    if oauth {
        ensure_claude_beta(&mut headers, "oauth-2025-04-20");
        let session = t.provider_session.map(str::to_string).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        set_header(&mut headers, "x-claude-code-session-id", session.clone());
        if !t.count_tokens {
            // Caller identity belongs to a different credential/account until rebound.
            // Preserve other metadata, but never retain unvalidated compound identity.
            if !body["metadata"].is_object() {
                body["metadata"] = json!({});
            }
            body["metadata"]["user_id"] = json!({
                "device_id": t.acct.device_id,
                "account_uuid": account_uuid.unwrap_or_default(),
                "session_id": session,
            })
            .to_string()
            .into();
        }
        headers.push(("authorization".into(), format!("Bearer {token}")));
    } else {
        headers.push(("x-api-key".into(), token));
    }
    if t.count_tokens
        && let Some(object) = body.as_object_mut()
    {
        object.remove("metadata");
    }
    if !headers.iter().any(|(name, value)| name == "x-client-request-id" && !value.trim().is_empty()) {
        set_header(&mut headers, "x-client-request-id", uuid::Uuid::new_v4().to_string());
    }
    headers.retain(|(k, _)| k != "content-type" && k != "accept");
    headers.push(("content-type".into(), "application/json".into()));
    headers.push(("accept".into(), if t.stream { "text/event-stream" } else { "application/json" }.into()));
    Prepared { url, headers, body, raw: None }
}

/// Keep native beta formatting and unknown provider flags; append only missing requirements.
fn has_claude_beta_family(headers: &[(String, String)], prefix: &str) -> bool {
    headers
        .iter()
        .filter(|(name, _)| name == "anthropic-beta")
        .any(|(_, value)| value.split(',').any(|flag| flag.trim().starts_with(prefix)))
}

fn ensure_claude_beta(headers: &mut Vec<(String, String)>, beta: &str) {
    if headers
        .iter()
        .filter(|(name, _)| name == "anthropic-beta")
        .any(|(_, value)| value.split(',').any(|flag| flag.trim() == beta))
    {
        return;
    }
    if let Some((_, value)) = headers.iter_mut().find(|(name, _)| name == "anthropic-beta") {
        if !value.is_empty() {
            value.push(',');
        }
        value.push_str(beta);
    } else {
        headers.push(("anthropic-beta".into(), beta.into()));
    }
}

/// Thinking blocks that carried another provider's reasoning through a Claude
/// client would fail Anthropic's signature check.
fn strip_foreign_thinking(body: &mut Value) {
    for m in body["messages"].as_array_mut().into_iter().flatten() {
        if let Some(blocks) = m["content"].as_array_mut() {
            blocks.retain(|b| {
                !(b["type"] == "thinking" && b["signature"].as_str().is_some_and(|s| s.starts_with("cpx-")))
            });
        }
    }
}

/// JavaScript-compatible 3-char build fingerprint Claude Code puts in its billing header.
fn cc_fingerprint(message: &str) -> String {
    let units: Vec<u16> = message.encode_utf16().collect();
    let sampled: Vec<u16> = [4usize, 7, 20].iter().map(|&i| units.get(i).copied().unwrap_or(b'0' as u16)).collect();
    let version = crate::clients::version(crate::clients::CLAUDE_CODE);
    let input = format!("{CC_FINGERPRINT_SALT}{}{version}", String::from_utf16_lossy(&sampled));
    hex::encode(Sha256::digest(input.as_bytes()))[..3].to_string()
}

fn first_user_text(body: &Value) -> (Option<usize>, String) {
    let Some(msgs) = body["messages"].as_array() else { return (None, String::new()) };
    let Some(idx) = msgs.iter().position(|m| m["role"] == "user") else { return (None, String::new()) };
    let text = match &msgs[idx]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .find(|t| !t.starts_with("<system-reminder>"))
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    };
    (Some(idx), text)
}

/// Makes a third-party request look like Claude Code: Claude Code system
/// prompt on top and caller instructions moved into the first user turn.
/// Account/session identity is bound separately for native and cloaked requests alike.
fn cloak_body(body: &mut Value, count_tokens: bool, cache_defaults: bool) {
    let configured_cache = !cache_defaults || crate::cache::has_claude_controls(body);
    let original: Vec<Value> = match &body["system"] {
        Value::String(s) => vec![json!({"type": "text", "text": s})],
        Value::Array(blocks) => blocks.clone(),
        _ => vec![],
    };
    let caller: Vec<Value> = original
        .iter()
        .filter_map(|block| {
            let text = block["text"].as_str()?;
            if text.trim().is_empty() || text == CC_IDENTITY || text.starts_with("x-anthropic-billing-header") {
                return None;
            }
            let mut reminder =
                json!({"type": "text", "text": format!("<system-reminder>\n{}\n</system-reminder>", text.trim_end())});
            if !block["cache_control"].is_null() {
                reminder["cache_control"] = block["cache_control"].clone();
            }
            Some(reminder)
        })
        .collect();

    let (first_user, text) = first_user_text(body);
    let billing = format!(
        "x-anthropic-billing-header: cc_version={}.{}; cc_entrypoint=cli;",
        crate::clients::version(crate::clients::CLAUDE_CODE),
        cc_fingerprint(&text)
    );
    body["system"] = json!([
        { "type": "text", "text": billing },
        { "type": "text", "text": CC_IDENTITY }
    ]);
    for block in &original {
        let text = block["text"].as_str().unwrap_or_default();
        let index = if text == CC_IDENTITY {
            Some(1)
        } else if text.starts_with("x-anthropic-billing-header") {
            Some(0)
        } else {
            None
        };
        if let Some(index) = index
            && !block["cache_control"].is_null()
        {
            body["system"][index]["cache_control"] = block["cache_control"].clone();
        }
    }
    // An extra 5m breakpoint could exceed Anthropic's four-breakpoint limit or
    // precede the caller's 1h breakpoint. Respect explicit and automatic caching.
    // Without any, cache the shared tools and system prompt, and let automatic caching
    // follow the growing conversation (which now holds the caller's instructions too).
    if !configured_cache {
        body["system"][1]["cache_control"] = json!({"type": "ephemeral"});
        if !count_tokens {
            body["cache_control"] = json!({"type": "ephemeral"});
        }
    }

    if let Some(idx) = first_user.filter(|_| !caller.is_empty()) {
        let has_long_cache = caller.iter().any(|b| b["cache_control"]["ttl"] == "1h");
        let short_cache = |block: &Value| !block["cache_control"].is_null() && block["cache_control"]["ttl"] != "1h";
        let earlier_short_cache = body["system"].as_array().unwrap().iter().any(short_cache)
            || body["messages"].as_array().unwrap()[..idx]
                .iter()
                .any(|message| message["content"].as_array().into_iter().flatten().any(short_cache));
        let content = &mut body["messages"][idx]["content"];
        let mut blocks = match content.take() {
            Value::String(s) => vec![json!({ "type": "text", "text": s })],
            Value::Array(a) => a,
            _ => vec![],
        };
        let at = blocks.iter().take_while(|b| b["type"] == "tool_result").count();
        let ttl_conflict = has_long_cache && (earlier_short_cache || blocks[..at].iter().any(short_cache));
        if ttl_conflict {
            // Keep the original system-cache order before any earlier assistant
            // or leading tool-result block with a shorter cache lifetime.
            body["system"] = cloaked_system_in_original_order(&original, &body["system"], caller);
        } else {
            blocks.splice(at..at, caller);
        }
        body["messages"][idx]["content"] = Value::Array(blocks);
    } else if first_user.is_none() {
        body["system"] = cloaked_system_in_original_order(&original, &body["system"], caller);
    }
}

fn cloaked_system_in_original_order(original: &[Value], identity: &Value, caller: Vec<Value>) -> Value {
    let mut system = Vec::new();
    for index in 0..2 {
        let present = original.iter().any(|block| {
            let text = block["text"].as_str().unwrap_or_default();
            if index == 0 { text.starts_with("x-anthropic-billing-header") } else { text == CC_IDENTITY }
        });
        if !present {
            system.push(identity[index].clone());
        }
    }
    let mut reminders = caller.into_iter();
    for block in original {
        let text = block["text"].as_str().unwrap_or_default();
        if text == CC_IDENTITY {
            system.push(identity[1].clone());
        } else if text.starts_with("x-anthropic-billing-header") {
            system.push(identity[0].clone());
        } else if !text.trim().is_empty()
            && let Some(reminder) = reminders.next()
        {
            system.push(reminder);
        }
    }
    Value::Array(system)
}

// ----------------------------------------------------------------------- codex

/// Fields the ChatGPT Codex backend rejects (`generate` is fine on websockets).
const CODEX_STRIP: &[&str] = &[
    "max_output_tokens",
    "max_completion_tokens",
    "temperature",
    "top_p",
    "safety_identifier",
    "prompt_cache_retention",
    "generate",
    "user",
    "truncation",
    "background",
];

/// `websocket`: the request is a native websocket turn, which keeps `previous_response_id`
/// and `generate` (Codex warms a connection up with `generate: false`).
pub fn sanitize_codex_body(body: &mut Value, model: &str, websocket: bool) {
    body["model"] = model.into();
    body["store"] = false.into();
    if let Some(o) = body.as_object_mut() {
        for k in CODEX_STRIP.iter().filter(|k| !(websocket && **k == "generate")) {
            o.remove(*k);
        }
        // Native Codex requests use this summary-delivery control on both transports.
        // Chat Completions stream options such as include_usage are unsupported here.
        if let Some(options) = o.get_mut("stream_options") {
            if let Some(options) = options.as_object_mut() {
                options.retain(|name, _| name == "reasoning_summary_delivery");
            }
            if !options.is_object() || options.as_object().is_some_and(|options| options.is_empty()) {
                o.remove("stream_options");
            }
        }
        // The ChatGPT backend only takes a list ("Input must be a list").
        if let Some(Value::String(text)) = o.get("input").cloned() {
            o.insert(
                "input".into(),
                json!([{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": text }] }]),
            );
        }
        // Codex 0.162 sends base instructions as developer input, on HTTP and websockets.
        if let Some(Value::String(text)) = o.remove("instructions")
            && !text.is_empty()
        {
            let input = o.entry("input").or_insert_with(|| json!([]));
            if input.is_null() {
                *input = json!([]);
            }
            if let Some(items) = input.as_array_mut() {
                items.insert(
                    0,
                    json!({ "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": text }] }),
                );
            }
        }
        if !websocket {
            o.remove("previous_response_id");
        }
        if let Some(Value::Array(items)) = o.get_mut("input") {
            // Reasoning tunnelled from another provider can't be decrypted by OpenAI.
            items.retain(|it| {
                !(it["type"] == "reasoning" && it["encrypted_content"].as_str().is_some_and(|e| e.starts_with("cpx-")))
            });
            // Item ids reference server-side state that store=false never kept.
            for it in items.iter_mut() {
                // ChatGPT plan usage accepts developer instructions, not system
                // message items. Normalize here for HTTP and WebSocket callers,
                // retaining message endings and per-block cache metadata.
                if it["role"] == "system" && matches!(it["type"].as_str(), None | Some("message")) {
                    it["role"] = "developer".into();
                }
                if let Some(io) = it.as_object_mut()
                    && io.get("type").and_then(Value::as_str) != Some("item_reference")
                {
                    io.remove("id");
                }
            }
        }
    }
}

/// Codex 0.162 sends routing fields first; other keys keep their order after these.
const CODEX_FIELD_ORDER: &[&str] = &[
    "type",
    "model",
    "stream",
    "service_tier",
    "previous_response_id",
    "input",
    "tools",
    "tool_choice",
    "parallel_tool_calls",
    "reasoning",
    "store",
    "stream_options",
    "include",
    "prompt_cache_key",
    "text",
    "generate",
    "client_metadata",
    "access_programs",
];

/// Puts a Codex request body's fields in the order Codex itself writes them.
pub fn order_codex_body(body: &mut Value) {
    let Some(o) = body.as_object_mut() else { return };
    let mut rest = std::mem::take(o);
    for k in CODEX_FIELD_ORDER {
        if let Some(v) = rest.remove(*k) {
            o.insert((*k).to_string(), v);
        }
    }
    o.extend(rest);
}

#[cfg(test)]
pub fn codex_headers(client: &HeaderMap, token: &str, account_id: Option<&str>, oauth: bool) -> Vec<(String, String)> {
    codex_headers_with_config(client, token, account_id, oauth, None)
}

fn codex_headers_with_config(
    client: &HeaderMap,
    token: &str,
    account_id: Option<&str>,
    oauth: bool,
    cfg: Option<&Config>,
) -> Vec<(String, String)> {
    let mut h = inbound_headers(Provider::Codex, cfg, client);
    set_header(&mut h, "authorization", format!("Bearer {token}"));
    if oauth {
        let generated_version = crate::clients::codex_version();
        let generated_agent = CODEX_USER_AGENT.replace(CODEX_VERSION, &generated_version);
        // Recognition chooses the client profile, never the forwarding policy.
        let nominated = connection_headers(client);
        let native = crate::clients::is_codex(client)
            && !["user-agent", "originator", "version"].iter().any(|name| nominated.contains(*name));
        if !native {
            set_header(&mut h, "version", generated_version);
            set_header(&mut h, "user-agent", generated_agent);
            set_header(&mut h, "originator", CODEX_ORIGINATOR.into());
        } else {
            // Connection-nominated profile fields must not be copied back in.
            for (name, value) in [
                ("version", crate::clients::native_codex_version(client).unwrap_or(generated_version)),
                ("user-agent", generated_agent),
                ("originator", CODEX_ORIGINATOR.into()),
            ] {
                if !h.iter().any(|(key, _)| key == name) {
                    h.push((name.into(), value));
                }
            }
        }
        if let Some(a) = account_id {
            h.push(("chatgpt-account-id".into(), a.into()));
        }
    } else {
        set_header(&mut h, "user-agent", format!("Fusebox/{}", env!("CARGO_PKG_VERSION")));
        h.retain(|(name, _)| name != "originator");
    }
    h
}

/// Replaces caller/cache-derived session identity while retaining thread/window/turn metadata.
pub fn bind_codex_session(headers: &mut Vec<(String, String)>, session: &str) {
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case("session_id") && !name.eq_ignore_ascii_case("session-id"));
    headers.push(("session-id".into(), session.into()));
}

fn codex(t: &Target, mut body: Value) -> Prepared {
    let (token, base, oauth, account_id) = creds(t.acct);
    let base = base.unwrap_or_else(|| if oauth { CODEX_BACKEND.into() } else { OPENAI_API.into() });
    if oauth {
        sanitize_codex_body(&mut body, t.model, false);
        body["stream"] = true.into();
    } else {
        body["model"] = t.model.into();
        body["stream"] = t.stream.into();
    }
    // OpenAI routes cache lookups by prefix and this key. Codex always sends one; other
    // clients (and translated requests) get the session's. Custom endpoints may reject it.
    let official = oauth || base.trim_end_matches('/') == OPENAI_API;
    if official
        && body["prompt_cache_key"].is_null()
        && let Some(key) = t.cache_key
    {
        body["prompt_cache_key"] = key.into();
    }
    if oauth {
        order_codex_body(&mut body);
    }
    let mut headers = codex_headers_with_config(t.client_headers, &token, account_id.as_deref(), oauth, Some(t.cfg));
    if let Some(session) = t.provider_session {
        bind_codex_session(&mut headers, session);
    } else if oauth && !headers.iter().any(|(k, _)| k == "session_id" || k == "session-id") {
        // Background calls without a conversation binding must not share account identity.
        bind_codex_session(&mut headers, &uuid::Uuid::new_v4().to_string());
    }
    headers.push(("content-type".into(), "application/json".into()));
    headers.push(("accept".into(), if oauth || t.stream { "text/event-stream" } else { "application/json" }.into()));
    Prepared { url: format!("{base}/responses"), headers, body, raw: None }
}

pub fn codex_ws_url(acct: &Account, cfg: &Config, client_headers: &HeaderMap) -> (String, Vec<(String, String)>) {
    let (token, base, oauth, account_id) = creds(acct);
    let base = base.unwrap_or_else(|| if oauth { CODEX_BACKEND.into() } else { OPENAI_API.into() });
    let ws = base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
    let mut h = codex_headers_with_config(client_headers, &token, account_id.as_deref(), oauth, Some(cfg));
    h.push(("openai-beta".into(), CODEX_WS_BETA.into()));
    push_custom(acct, client_headers, &mut h);
    (format!("{ws}/responses"), h)
}

// ---------------------------------------------------------------------- gemini

fn gemini(t: &Target, mut body: Value) -> Prepared {
    let (token, base, _, _) = creds(t.acct);
    let base = base.unwrap_or_else(|| GEMINI_API.into());
    let action = gemini_action(t);
    if let Some(o) = body.as_object_mut() {
        o.remove("model");
    }
    let headers = vec![
        ("x-goog-api-key".into(), token),
        ("content-type".into(), "application/json".into()),
        ("user-agent".into(), format!("Fusebox/{}", env!("CARGO_PKG_VERSION"))),
    ];
    Prepared { url: format!("{base}/v1beta/models/{}:{action}", t.model), headers, body, raw: None }
}

// ---------------------------------------------------------------------- vertex

fn gemini_action(t: &Target) -> &'static str {
    if t.count_tokens {
        "countTokens"
    } else if t.stream {
        "streamGenerateContent?alt=sse"
    } else {
        "generateContent"
    }
}

fn vertex(t: &Target, mut body: Value) -> Prepared {
    let (token, base, oauth, _) = creds(t.acct);
    if let Some(o) = body.as_object_mut() {
        o.remove("model");
    }
    let action = gemini_action(t);
    let mut headers = vec![
        ("content-type".into(), "application/json".into()),
        ("user-agent".into(), format!("Fusebox/{}", env!("CARGO_PKG_VERSION"))),
    ];
    let url = if oauth {
        let (project, location) = match &*t.acct.cred.read() {
            Credential::OAuth(o) => {
                (o.project_id.clone().unwrap_or_default(), o.field("location").unwrap_or("us-central1").to_string())
            }
            _ => Default::default(),
        };
        let base = base.unwrap_or_else(|| crate::vertex::base_url(&location));
        headers.push(("authorization".into(), format!("Bearer {token}")));
        format!("{base}/v1/projects/{project}/locations/{location}/publishers/google/models/{}:{action}", t.model)
    } else {
        // Express mode: API key, no project.
        let base = base.unwrap_or_else(|| "https://aiplatform.googleapis.com".into());
        headers.push(("x-goog-api-key".into(), token));
        format!("{base}/v1/publishers/google/models/{}:{action}", t.model)
    };
    Prepared { url, headers, body, raw: None }
}

// ----------------------------------------------------------------- antigravity

fn antigravity(t: &Target, body: Value) -> Prepared {
    let (token, _, _, _) = creds(t.acct);
    let project = crate::antigravity::project(t.acct);
    let base = crate::antigravity::request_base(t.acct);
    let path = if t.stream { "streamGenerateContent?alt=sse" } else { "generateContent" };
    Prepared {
        url: format!("{base}/v1internal:{path}"),
        headers: vec![
            ("authorization".into(), format!("Bearer {token}")),
            ("content-type".into(), "application/json".into()),
            ("user-agent".into(), crate::antigravity::user_agent()),
            ("accept".into(), if t.stream { "text/event-stream" } else { "application/json" }.into()),
        ],
        body: crate::antigravity::envelope(body, &crate::antigravity::current_id(t.model), project.as_deref()),
        raw: None,
    }
}

// ------------------------------------------------------------------------ kimi

/// Inlines local `$ref`s and makes sure every tool schema is an object (Moonshot is strict).
fn normalize_chat_tools(body: &mut Value) {
    for tool in body["tools"].as_array_mut().into_iter().flatten() {
        let params = &mut tool["function"]["parameters"];
        if params.is_object() {
            let mut p = crate::schema::inline_only(params);
            if p.get("type").is_none() {
                p["type"] = "object".into();
            }
            *params = p;
        }
    }
}

fn kimi(t: &Target, mut body: Value) -> Prepared {
    let (token, base, oauth, _) = creds(t.acct);
    let base = base.unwrap_or_else(|| device::kimi::API_BASE.into());
    let root = base.trim_end_matches("/v1").to_string();
    let v1 = format!("{root}/v1");
    body["model"] = t.model.into();
    let mut headers = vec![
        ("authorization".into(), format!("Bearer {token}")),
        ("content-type".into(), "application/json".into()),
        ("user-agent".into(), format!("Fusebox/{}", env!("CARGO_PKG_VERSION"))),
        ("accept".into(), if t.stream { "text/event-stream" } else { "application/json" }.into()),
    ];
    if oauth {
        let device_id = match &*t.acct.cred.read() {
            Credential::OAuth(o) => o.field("device_id").map(String::from),
            _ => None,
        };
        headers.extend(device::kimi_headers(&device_id.unwrap_or_else(|| t.acct.device_id.clone())));
    }
    let url = match t.wire {
        Format::Claude => {
            headers.push(("x-api-key".into(), token));
            let version = header(t.client_headers, "anthropic-version").unwrap_or_else(|| "2023-06-01".into());
            headers.push(("anthropic-version".into(), version));
            if let Some(b) = header(t.client_headers, "anthropic-beta") {
                headers.push(("anthropic-beta".into(), b));
            }
            strip_foreign_thinking(&mut body);
            format!("{root}/v1/messages")
        }
        Format::Responses => {
            body["stream"] = t.stream.into();
            strip_responses_extras(&mut body);
            format!("{v1}/responses")
        }
        _ => {
            body["stream"] = t.stream.into();
            if t.stream {
                body["stream_options"] = json!({ "include_usage": true });
            }
            normalize_chat_tools(&mut body);
            if matches!(t.model, "kimi-k3.1-pro" | "kimi-k3.1-pro-highspeed") {
                normalize_kimi_k31(&mut body);
            }
            // Kimi only accepts its fixed temperatures; let it choose.
            let thinking_off = body["thinking"]["type"] == "disabled";
            let temp = body["temperature"].as_f64();
            if temp.is_some_and(|x| (thinking_off && x != 0.6) || (!thinking_off && x != 1.0))
                && let Some(o) = body.as_object_mut()
            {
                o.remove("temperature");
            }
            format!("{v1}/chat/completions")
        }
    };
    Prepared { url, headers, body, raw: None }
}

/// K3.1 always thinks and accepts only fixed sampling values. The Chat API
/// names its output limit `max_completion_tokens`, including translated limits.
fn normalize_kimi_k31(body: &mut Value) {
    let Some(o) = body.as_object_mut() else { return };
    for key in ["temperature", "top_p", "n", "presence_penalty", "frequency_penalty"] {
        o.remove(key);
    }
    if let Some(limit) = o.remove("max_tokens") {
        o.entry("max_completion_tokens").or_insert(limit);
    }
    if o.get("thinking").is_some_and(|v| v["type"] == "disabled") {
        o.remove("thinking");
        o.insert("reasoning_effort".into(), "low".into());
    } else if o.get("reasoning_effort").is_some_and(|v| v == "none") {
        o.insert("reasoning_effort".into(), "low".into());
    }
}

// ------------------------------------------------------------------ xai / meta

/// Fields the stateless Responses backends (xAI, Meta) reject.
fn strip_responses_extras(body: &mut Value) {
    if let Some(o) = body.as_object_mut() {
        for k in ["previous_response_id", "prompt_cache_retention", "safety_identifier", "stream_options", "stop"] {
            o.remove(k);
        }
        if let Some(Value::Array(items)) = o.get_mut("input") {
            items.retain(|it| {
                !(it["type"] == "reasoning" && it["encrypted_content"].as_str().is_some_and(|e| e.starts_with("cpx-")))
            });
        }
    }
}

fn responses_api(t: &Target, mut body: Value) -> Prepared {
    let (token, base, oauth, _) = creds(t.acct);
    body["model"] = t.model.into();
    body["stream"] = t.stream.into();
    strip_responses_extras(&mut body);
    let mut headers = vec![
        ("authorization".into(), format!("Bearer {token}")),
        ("content-type".into(), "application/json".into()),
        ("accept".into(), if t.stream { "text/event-stream" } else { "application/json" }.into()),
    ];
    let base = if t.acct.provider == Provider::Meta {
        if let Some(effort) = body["reasoning"]["effort"].as_str() {
            body["reasoning"]["effort"] = crate::formats::responses::normalize_reasoning_effort(effort, t.model).into();
        }
        headers.push(("user-agent".into(), device::meta::API_UA.into()));
        headers.push(("x-client-id".into(), "tbh:tui".into()));
        headers.push(("x-tbh-agent-role".into(), "main".into()));
        base.unwrap_or_else(|| device::meta::API_BASE.into())
    } else {
        // xAI routes its prompt cache by conversation id.
        if let Some(key) = body["prompt_cache_key"].as_str().or(t.cache_key) {
            headers.push(("x-grok-conv-id".into(), key.to_string()));
        }
        let official = base.as_deref().is_none_or(|b| b.trim_end_matches('/') == device::xai::API_BASE);
        if oauth && official {
            // Grok subscriptions (SuperGrok / X Premium) go through the CLI chat proxy.
            headers.extend(
                [
                    ("x-xai-token-auth", "xai-grok-cli".to_string()),
                    ("x-grok-client-version", crate::clients::version(crate::clients::GROK)),
                    // The interactive CLI: its pager names itself and the shell it runs.
                    (
                        "user-agent",
                        format!(
                            "grok-pager/{v} grok-shell/{v} (macos; aarch64)",
                            v = crate::clients::version(crate::clients::GROK)
                        ),
                    ),
                    ("x-grok-client-identifier", "grok-shell".to_string()),
                    ("x-grok-client-mode", "interactive".to_string()),
                    ("x-authenticateresponse", "authenticate-response".to_string()),
                ]
                .map(|(k, v)| (k.to_string(), v)),
            );
            device::xai::CLI_BASE.into()
        } else {
            headers.push(("user-agent".into(), format!("Fusebox/{}", env!("CARGO_PKG_VERSION"))));
            base.unwrap_or_else(|| device::xai::API_BASE.into())
        }
    };
    Prepared { url: format!("{}/responses", base.trim_end_matches('/')), headers, body, raw: None }
}

// ---------------------------------------------------------------------- compat

fn compat(t: &Target, mut body: Value) -> Prepared {
    let (token, base, _, _) = creds(t.acct);
    let base = base.unwrap_or_else(|| OPENAI_API.into());
    body["model"] = t.model.into();
    if !t.passthrough {
        body["stream"] = true.into();
        if t.cfg
            .openai_compatibility
            .iter()
            .any(|entry| t.acct.group.as_deref() == Some(entry.name.as_str()) && entry.strict_chat_template)
        {
            crate::formats::chat::normalize_strict_template(&mut body);
        }
    }
    let mut headers = vec![
        ("content-type".into(), "application/json".into()),
        ("user-agent".into(), format!("Fusebox/{}", env!("CARGO_PKG_VERSION"))),
    ];
    if !token.is_empty() {
        headers.push(("authorization".into(), format!("Bearer {token}")));
    }
    Prepared { url: format!("{base}/chat/completions"), headers, body, raw: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kimi_k31_chat_omits_fixed_sampling_and_preserves_explicit_completion_limit() {
        let cfg = Config::parse("auth-dir: /nonexistent\nkimi-api-key:\n  - api-key: synthetic-kimi\n").unwrap();
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        let acct = pool.all().remove(0);
        let headers = HeaderMap::new();
        for model in ["kimi-k3.1-pro", "kimi-k3.1-pro-highspeed"] {
            let mut target = test_target(&acct, &cfg, &headers);
            target.model = model;
            target.wire = Format::Chat;
            let prepared = prepare(
                &target,
                json!({"messages":[{"role":"user","content":"hello"}],
                "temperature":0.2,"top_p":0.8,"n":2,"presence_penalty":1,"frequency_penalty":1,
                "max_tokens":2048,"reasoning_effort":"none"}),
            );
            assert!(prepared.url.ends_with("/v1/chat/completions"));
            assert_eq!(prepared.body["model"], model);
            assert_eq!(prepared.body["max_completion_tokens"], 2048);
            assert_eq!(prepared.body["reasoning_effort"], "low");
            for key in ["temperature", "top_p", "n", "presence_penalty", "frequency_penalty", "max_tokens"] {
                assert!(prepared.body.get(key).is_none(), "{key}");
            }
            let prepared = prepare(
                &target,
                json!({"max_tokens":100,"max_completion_tokens":200,
                "reasoning_effort":"max"}),
            );
            assert_eq!(prepared.body["max_completion_tokens"], 200);
            assert_eq!(prepared.body["reasoning_effort"], "max");
        }
    }

    fn request_headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(), value.parse().unwrap());
        }
        headers
    }

    fn get_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }

    fn oauth_account(provider: Provider) -> std::sync::Arc<Account> {
        let mut cfg = Config { auth_dir: "/nonexistent".into(), ..Default::default() };
        let key = crate::config::KeyEntry { api_key: "synthetic-key-B".into(), ..Default::default() };
        match provider {
            Provider::Claude => cfg.claude_api_key.push(key),
            Provider::Codex => cfg.codex_api_key.push(key),
            _ => panic!("unsupported test provider"),
        }
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        let acct = pool.all().remove(0);
        *acct.cred.write() = Credential::OAuth(crate::accounts::OAuth {
            access_token: "synthetic-selected-token-B".into(),
            account_id: Some("selected-account-B".into()),
            ..Default::default()
        });
        acct
    }

    fn test_target<'a>(acct: &'a Account, cfg: &'a Config, headers: &'a HeaderMap) -> Target<'a> {
        Target {
            acct,
            cfg,
            client_headers: headers,
            model: if acct.provider == Provider::Claude { "claude-sonnet-4-6" } else { "gpt-6.1-sol" },
            wire: if acct.provider == Provider::Claude { Format::Claude } else { Format::Responses },
            passthrough: true,
            stream: true,
            count_tokens: false,
            cache_key: None,
            provider_session: Some("11111111-2222-4333-8444-555555555555"),
            cache_defaults: true,
        }
    }

    #[test]
    fn claude_header_policy_applies_to_native_cloaked_and_api_key_requests() {
        let dangerous = [
            "authorization",
            "x-api-key",
            "x-goog-api-key",
            "proxy-authorization",
            "cookie",
            "set-cookie",
            "via",
            "cf-connecting-ip",
            "x-forwarded-user",
            "x-auth-request-email",
            "x-gateway-key",
            "x-management-key",
            "x-fusebox-source",
            "x-cliproxy-source",
            "tailscale-user-login",
            "x-private-feature",
            "anthropic-api-key",
            "x-claude-code-access-token",
            "x-stainless-internal",
            "x-unknown-feature",
            "x-codex-only-feature",
            "x-provider-nominated",
            "anthropic-nominated",
        ];
        for oauth in [false, true] {
            for native in [false, true] {
                for cloak in [false, true] {
                    let acct = if oauth { oauth_account(Provider::Claude) } else { claude_account() };
                    let mut cfg = Config { claude_cloak: cloak, ..Default::default() };
                    // Even unvalidated extension config cannot bypass private-header filtering.
                    cfg.upstream_header_allowlist.insert(
                        "claude".into(),
                        [
                            "x-provider-feature",
                            "x-provider-nominated",
                            "x-gateway-key",
                            "authorization",
                            "chatgpt-account-id",
                        ]
                        .map(String::from)
                        .into(),
                    );
                    cfg.upstream_header_allowlist.insert("codex".into(), vec!["x-codex-only-feature".into()]);
                    let mut headers = request_headers(&[
                        ("user-agent", if native { CC_USER_AGENT } else { "third-party/1.0" }),
                        ("anthropic-version", "2023-06-01"),
                        ("anthropic-feature", "approved-provider-value"),
                        ("x-claude-code-feature", "approved-client-value"),
                        ("x-stainless-os", "Linux"),
                        ("x-app", "cli"),
                        ("x-client-request-id", "approved-request-id"),
                        ("x-provider-feature", "approved-extension"),
                        ("connection", "keep-alive, X-Provider-Nominated"),
                        ("connection", "Anthropic-Nominated"),
                        ("chatgpt-account-id", "synthetic-caller-account-A"),
                    ]);
                    for name in dangerous {
                        headers.insert(
                            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                            "SYNTHETIC_GATEWAY_CREDENTIAL".parse().unwrap(),
                        );
                    }
                    let prepared = prepare(
                        &test_target(&acct, &cfg, &headers),
                        json!({"messages":[{"role":"user","content":"hello"}]}),
                    );
                    for name in dangerous.into_iter().filter(|name| *name != "authorization" && *name != "x-api-key") {
                        assert_eq!(
                            get_header(&prepared.headers, name),
                            None,
                            "{name}: oauth={oauth}, native={native}, cloak={cloak}"
                        );
                    }
                    assert_eq!(get_header(&prepared.headers, "chatgpt-account-id"), None);
                    assert_eq!(get_header(&prepared.headers, "connection"), None);
                    let generated = oauth && !native && cloak;
                    assert_eq!(
                        get_header(&prepared.headers, "anthropic-feature"),
                        (!generated).then_some("approved-provider-value")
                    );
                    assert_eq!(
                        get_header(&prepared.headers, "x-claude-code-feature"),
                        (!generated).then_some("approved-client-value")
                    );
                    assert_eq!(get_header(&prepared.headers, "x-provider-feature"), Some("approved-extension"));
                    if generated {
                        assert!(
                            uuid::Uuid::parse_str(get_header(&prepared.headers, "x-client-request-id").unwrap())
                                .is_ok()
                        );
                    } else {
                        assert_eq!(get_header(&prepared.headers, "x-client-request-id"), Some("approved-request-id"));
                    }
                    if native {
                        assert_eq!(get_header(&prepared.headers, "user-agent"), Some(CC_USER_AGENT));
                        assert_eq!(get_header(&prepared.headers, "x-stainless-os"), Some("Linux"));
                    }
                    let selected = if oauth { "Bearer synthetic-selected-token-B" } else { "test" };
                    assert_eq!(
                        get_header(&prepared.headers, if oauth { "authorization" } else { "x-api-key" }),
                        Some(selected)
                    );
                    assert!(!prepared.headers.iter().any(|(_, value)| value.contains("SYNTHETIC_GATEWAY_CREDENTIAL")));
                }
            }
        }
    }

    #[test]
    fn claude_native_betas_keep_formatting_unknown_flags_and_required_oauth() {
        let acct = oauth_account(Provider::Claude);
        let cfg = Config::default();
        for beta in ["future-beta , oauth-2025-04-20, future-beta", "future-beta , another-beta"] {
            let headers = request_headers(&[("user-agent", CC_USER_AGENT), ("anthropic-beta", beta)]);
            let prepared = prepare(&test_target(&acct, &cfg, &headers), json!({"messages":[]}));
            let expected =
                if beta.contains("oauth-2025-04-20") { beta.into() } else { format!("{beta},oauth-2025-04-20") };
            assert_eq!(get_header(&prepared.headers, "anthropic-beta"), Some(expected.as_str()));
        }
        let headers = request_headers(&[
            ("user-agent", CC_USER_AGENT),
            ("anthropic-beta", "native-first"),
            ("anthropic-beta", "oauth-2025-04-20 , native-second"),
        ]);
        let prepared = prepare(&test_target(&acct, &cfg, &headers), json!({"messages":[]}));
        assert_eq!(
            prepared
                .headers
                .iter()
                .filter(|(name, _)| name == "anthropic-beta")
                .map(|(_, value)| value.as_str())
                .collect::<Vec<_>>(),
            ["native-first", "oauth-2025-04-20 , native-second"]
        );
    }

    #[test]
    fn claude_nominated_betas_are_not_reintroduced_but_oauth_is_synthesized() {
        let acct = oauth_account(Provider::Claude);
        let cfg = Config::default();
        let headers = request_headers(&[
            ("user-agent", CC_USER_AGENT),
            ("anthropic-beta", "private-caller-beta"),
            ("connection", "ANTHROPIC-BETA, x-client-request-id"),
            ("x-client-request-id", "caller-nominated"),
        ]);
        let prepared = prepare(&test_target(&acct, &cfg, &headers), json!({"messages":[]}));
        assert_eq!(get_header(&prepared.headers, "anthropic-beta"), Some("oauth-2025-04-20"));
        assert!(uuid::Uuid::parse_str(get_header(&prepared.headers, "x-client-request-id").unwrap()).is_ok());
        let mut headers = headers;
        headers.insert("connection", axum::http::HeaderValue::from_bytes(b"\xff, anthropic-beta").unwrap());
        let prepared = prepare(&test_target(&acct, &cfg, &headers), json!({"messages":[]}));
        assert_eq!(get_header(&prepared.headers, "anthropic-beta"), Some("oauth-2025-04-20"));
    }

    #[test]
    fn claude_rebinds_account_a_metadata_to_selected_account_b_in_every_oauth_path() {
        let acct = oauth_account(Provider::Claude);
        let caller_identity = json!({"device_id":"a".repeat(64), "account_uuid":"caller-account-A", "session_id":"caller-session-A", "untrusted":"discard"}).to_string();
        for native in [false, true] {
            for cloak in [false, true] {
                let cfg = Config { claude_cloak: cloak, ..Default::default() };
                let headers = request_headers(&[
                    ("user-agent", if native { CC_USER_AGENT } else { "third-party/1.0" }),
                    ("x-claude-code-session-id", "caller-session-A"),
                ]);
                let body = json!({"messages":[{"role":"user","content":"preserve this"}],
                    "metadata":{"user_id":caller_identity,"unrelated":{"keep":true}},"tools":[{"name":"lookup","input_schema":{"type":"object"}}]});
                let prepared = prepare(&test_target(&acct, &cfg, &headers), body.clone());
                let identity: Value =
                    serde_json::from_str(prepared.body["metadata"]["user_id"].as_str().unwrap()).unwrap();
                assert_eq!(
                    identity,
                    json!({"device_id":acct.device_id,"account_uuid":"selected-account-B","session_id":"11111111-2222-4333-8444-555555555555"})
                );
                assert_eq!(get_header(&prepared.headers, "x-claude-code-session-id"), identity["session_id"].as_str());
                assert_eq!(get_header(&prepared.headers, "authorization"), Some("Bearer synthetic-selected-token-B"));
                assert_eq!(prepared.body["metadata"]["unrelated"], body["metadata"]["unrelated"]);
                assert_eq!(prepared.body["tools"], body["tools"]);
                if native || !cloak {
                    assert_eq!(prepared.body["messages"], body["messages"]);
                }
            }
        }
    }

    #[test]
    fn claude_malformed_identity_is_replaced_and_background_sessions_are_request_local() {
        let acct = oauth_account(Provider::Claude);
        let cfg = Config::default();
        let headers = request_headers(&[("user-agent", CC_USER_AGENT), ("x-claude-code-session-id", "caller-session")]);
        let mut target = test_target(&acct, &cfg, &headers);
        target.provider_session = None;
        let mut sessions = HashSet::new();
        for metadata in [
            Value::Null,
            json!("malformed"),
            json!({"user_id":"not JSON", "unrelated":42}),
            json!({"user_id":{"device_id":"bad"}}),
        ] {
            let prepared = prepare(&target, json!({"messages":[],"metadata":metadata}));
            let identity: Value = serde_json::from_str(prepared.body["metadata"]["user_id"].as_str().unwrap()).unwrap();
            let session = identity["session_id"].as_str().unwrap();
            assert_eq!(get_header(&prepared.headers, "x-claude-code-session-id"), Some(session));
            assert_ne!(session, acct.session_id);
            assert!(uuid::Uuid::parse_str(session).is_ok());
            assert!(sessions.insert(session.to_string()));
            if metadata.is_object() && metadata.get("unrelated").is_some() {
                assert_eq!(prepared.body["metadata"]["unrelated"], metadata["unrelated"]);
            }
            assert!(uuid::Uuid::parse_str(get_header(&prepared.headers, "x-client-request-id").unwrap()).is_ok());
        }
    }

    #[test]
    fn claude_count_tokens_never_has_message_metadata_in_any_path() {
        for oauth in [false, true] {
            for native in [false, true] {
                for cloak in [false, true] {
                    let acct = if oauth { oauth_account(Provider::Claude) } else { claude_account() };
                    let cfg = Config { claude_cloak: cloak, ..Default::default() };
                    let headers =
                        request_headers(&[("user-agent", if native { CC_USER_AGENT } else { "third-party/1.0" })]);
                    let mut target = test_target(&acct, &cfg, &headers);
                    target.count_tokens = true;
                    let prepared =
                        prepare(&target, json!({"messages":[],"metadata":{"user_id":"malformed","unrelated":1}}));
                    assert!(prepared.body.get("metadata").is_none());
                    assert!(prepared.url.contains("/messages/count_tokens"));
                }
            }
        }
    }

    #[test]
    fn configured_claude_headers_cannot_replace_selected_identity_or_authentication() {
        let mut acct = oauth_account(Provider::Claude);
        std::sync::Arc::get_mut(&mut acct).unwrap().headers = [
            ("Authorization", "synthetic-operator-token-A"),
            ("x-api-key", "synthetic-operator-key-A"),
            ("x-claude-code-session-id", "operator-session-A"),
            ("anthropic-beta", "operator-beta"),
            ("user-agent", "operator-agent"),
            ("x-client-request-id", "operator-request"),
            ("x-gateway-key", "synthetic-operator-gateway-key"),
            ("x-provider-feature", "operator-feature"),
        ]
        .into_iter()
        .map(|(name, value)| (name.into(), value.into()))
        .collect();
        let cfg = Config::default();
        let headers = request_headers(&[("user-agent", CC_USER_AGENT), ("x-client-request-id", "native-request")]);
        let prepared = prepare(&test_target(&acct, &cfg, &headers), json!({"messages":[]}));
        assert_eq!(get_header(&prepared.headers, "authorization"), Some("Bearer synthetic-selected-token-B"));
        assert_eq!(get_header(&prepared.headers, "x-api-key"), None);
        assert_eq!(
            get_header(&prepared.headers, "x-claude-code-session-id"),
            Some("11111111-2222-4333-8444-555555555555")
        );
        assert_eq!(get_header(&prepared.headers, "anthropic-beta"), Some("oauth-2025-04-20"));
        assert_eq!(get_header(&prepared.headers, "user-agent"), Some(CC_USER_AGENT));
        assert_eq!(get_header(&prepared.headers, "x-client-request-id"), Some("native-request"));
        assert_eq!(get_header(&prepared.headers, "x-gateway-key"), None);
        assert_eq!(get_header(&prepared.headers, "x-provider-feature"), Some("operator-feature"));
    }

    #[test]
    fn generated_claude_profile_drops_foreign_sdk_headers_but_keeps_betas_and_explicit_extensions() {
        let acct = oauth_account(Provider::Claude);
        let mut cfg = Config::default();
        cfg.upstream_header_allowlist.insert("claude".into(), vec!["x-provider-feature".into()]);
        let headers = request_headers(&[
            ("user-agent", "anthropic-python/1.0.0"),
            ("x-stainless-async", "async:asyncio"),
            ("x-stainless-read-timeout", "30"),
            ("x-claude-code-helper", "foreign-helper"),
            ("anthropic-custom-feature", "foreign-feature"),
            ("anthropic-beta", " effort-2026-10-01 ,custom-beta"),
            ("x-provider-feature", "explicit-feature"),
        ]);
        let prepared =
            prepare(&test_target(&acct, &cfg, &headers), json!({"messages":[],"output_config":{"effort":"high"}}));
        for name in
            ["x-stainless-async", "x-stainless-read-timeout", "x-claude-code-helper", "anthropic-custom-feature"]
        {
            assert_eq!(get_header(&prepared.headers, name), None);
        }
        assert_eq!(get_header(&prepared.headers, "x-stainless-lang"), Some("js"));
        assert_eq!(get_header(&prepared.headers, "x-stainless-runtime"), Some("node"));
        assert_eq!(get_header(&prepared.headers, "x-provider-feature"), Some("explicit-feature"));
        let beta = get_header(&prepared.headers, "anthropic-beta").unwrap();
        assert!(beta.starts_with(" effort-2026-10-01 ,custom-beta"));
        assert!(!beta.contains("effort-2025-11-24"));
    }

    #[test]
    fn intentional_gateway_credentials_survive_custom_routes_and_never_come_from_the_caller() {
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            openai_compatibility: vec![crate::config::CompatEntry {
                name: "gateway".into(),
                base_url: "https://gateway.example.invalid/v1".into(),
                api_keys: vec!["FAKE_OPERATOR_KEY".into()],
                ..Default::default()
            }],
            ..Default::default()
        };
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        let mut acct = pool.all().remove(0);
        drop(pool);
        std::sync::Arc::get_mut(&mut acct).unwrap().headers = [
            ("CF-Access-Client-Id".into(), "FAKE_OPERATOR_ID".into()),
            ("CF-Access-Client-Secret".into(), "FAKE_OPERATOR_SECRET".into()),
            ("x-portkey-api-key".into(), "FAKE_OPERATOR_GATEWAY".into()),
        ]
        .into_iter()
        .collect();
        let headers = request_headers(&[
            ("cf-access-client-secret", "FAKE_CALLER_SECRET"),
            ("x-portkey-api-key", "FAKE_CALLER_GATEWAY"),
        ]);
        let cfg = Config::default();
        let prepared = prepare(&test_target(&acct, &cfg, &headers), json!({"messages":[]}));
        assert_eq!(get_header(&prepared.headers, "cf-access-client-secret"), Some("FAKE_OPERATOR_SECRET"));
        assert_eq!(get_header(&prepared.headers, "x-portkey-api-key"), Some("FAKE_OPERATOR_GATEWAY"));
        assert!(!prepared.headers.iter().any(|(_, value)| value.contains("FAKE_CALLER")));
        for provider in [Provider::Claude, Provider::Codex] {
            assert!(!custom_header_allowed(provider, true, "cf-access-client-secret"));
            assert!(custom_header_allowed(provider, false, "cf-access-client-secret"));
        }
    }

    #[test]
    fn codex_stream_options_preserve_summary_delivery_on_http_and_websocket() {
        let original = json!({"input":"hello","stream_options":{"reasoning_summary_delivery":"sequential_cutoff","include_usage":true,"unsupported":1},
            "client_metadata":{"turn_id":"turn", "workspace":"permitted-workspace"}, "generate":false});
        let acct = oauth_account(Provider::Codex);
        let cfg = Config::default();
        let headers = HeaderMap::new();
        let prepared = prepare(&test_target(&acct, &cfg, &headers), original.clone());
        let mut websocket = original.clone();
        sanitize_codex_body(&mut websocket, "gpt-6.1-sol", true);
        order_codex_body(&mut websocket);
        for body in [&prepared.body, &websocket] {
            assert_eq!(body["stream_options"], json!({"reasoning_summary_delivery":"sequential_cutoff"}));
            assert_eq!(body["client_metadata"], original["client_metadata"]);
        }
        assert_eq!(websocket["generate"], false);
        assert!(prepared.body.get("generate").is_none());
        for websocket in [false, true] {
            for options in [json!({"include_usage":true}), Value::Null, json!(true)] {
                let mut body = json!({"input":[],"stream_options":options});
                sanitize_codex_body(&mut body, "gpt-6.1-sol", websocket);
                assert!(body.get("stream_options").is_none());
            }
        }
    }

    #[test]
    fn codex_http_and_websocket_share_safe_custom_headers_residency_and_selected_identity() {
        let mut acct = oauth_account(Provider::Codex);
        std::sync::Arc::get_mut(&mut acct).unwrap().headers = [
            ("Authorization", "synthetic-operator-token-A"),
            ("chatgpt-account-id", "operator-account-A"),
            ("session-id", "operator-session-A"),
            ("session_id", "operator-session-A"),
            ("user-agent", "operator-agent"),
            ("originator", "operator-originator"),
            ("version", "999.0.0"),
            ("x-api-key", "synthetic-operator-key"),
            ("x-gateway-key", "synthetic-operator-gateway-key"),
            ("x-forwarded-user", "operator-private-user"),
            ("x-provider-feature", "operator-feature"),
            ("x-provider-nominated", "operator-nominated"),
            ("openai-beta", "operator-beta"),
            (CODEX_RESIDENCY, "eu"),
            ("x-bad-value", "not\na header"),
        ]
        .into_iter()
        .map(|(name, value)| (name.into(), value.into()))
        .collect();
        let mut cfg = Config::default();
        cfg.upstream_header_allowlist
            .insert("codex".into(), vec!["x-client-feature".into(), "chatgpt-account-id".into()]);
        let headers = request_headers(&[
            ("user-agent", CODEX_USER_AGENT),
            ("originator", CODEX_ORIGINATOR),
            ("version", CODEX_VERSION),
            ("authorization", "Bearer SYNTHETIC_GATEWAY_CREDENTIAL"),
            ("chatgpt-account-id", "caller-account-A"),
            ("session_id", "caller-session-A"),
            ("session-id", "caller-session-A"),
            (CODEX_RESIDENCY, "us"),
            ("x-client-feature", "approved-extension"),
            ("thread-id", "native-thread"),
            ("x-codex-window-id", "native-window"),
            ("x-codex-turn-metadata", "native-turn"),
            ("x-provider-nominated", "caller-nominated"),
            ("connection", "X-Provider-Nominated"),
            ("x-gateway-key", "SYNTHETIC_GATEWAY_CREDENTIAL"),
        ]);
        let target = test_target(&acct, &cfg, &headers);
        let prepared = prepare(&target, json!({"input":[],"prompt_cache_key":"unrelated-cache-key"}));
        let (url, mut websocket) = codex_ws_url(&acct, &cfg, &headers);
        bind_codex_session(&mut websocket, target.provider_session.unwrap());
        assert_eq!(url, "wss://chatgpt.com/backend-api/codex/responses");
        for outbound in [&prepared.headers, &websocket] {
            for (name, value) in [
                ("authorization", "Bearer synthetic-selected-token-B"),
                ("chatgpt-account-id", "selected-account-B"),
                ("session-id", "11111111-2222-4333-8444-555555555555"),
                (CODEX_RESIDENCY, "us"),
                ("user-agent", CODEX_USER_AGENT),
                ("originator", CODEX_ORIGINATOR),
                ("version", CODEX_VERSION),
                ("x-provider-feature", "operator-feature"),
                ("x-client-feature", "approved-extension"),
                ("thread-id", "native-thread"),
                ("x-codex-window-id", "native-window"),
                ("x-codex-turn-metadata", "native-turn"),
            ] {
                assert_eq!(get_header(outbound, name), Some(value), "{name}");
                assert_eq!(outbound.iter().filter(|(key, _)| key.eq_ignore_ascii_case(name)).count(), 1, "{name}");
            }
            for name in [
                "session_id",
                "x-api-key",
                "x-gateway-key",
                "x-forwarded-user",
                "x-provider-nominated",
                "connection",
                "x-bad-value",
            ] {
                assert_eq!(get_header(outbound, name), None, "{name}");
            }
        }
        assert_eq!(get_header(&websocket, "openai-beta"), Some(CODEX_WS_BETA));
        let mut http = prepared.headers;
        http.retain(|(name, _)| name != "content-type" && name != "accept");
        websocket.retain(|(name, _)| name != "openai-beta");
        http.sort();
        websocket.sort();
        assert_eq!(http, websocket);
        assert_eq!(prepared.body["prompt_cache_key"], "unrelated-cache-key");
    }

    #[test]
    fn codex_api_key_transports_preserve_safe_headers_and_replace_caller_credentials() {
        let acct = oauth_account(Provider::Codex);
        *acct.cred.write() = Credential::ApiKey { key: "synthetic-selected-api-key-B".into(), base_url: None };
        let cfg = Config::default();
        let headers = request_headers(&[
            ("authorization", "Bearer caller-key-A"),
            ("x-api-key", "caller-key-A"),
            ("chatgpt-account-id", "caller-account-A"),
            ("x-codex-turn-state", "preserved"),
            (CODEX_RESIDENCY, "us"),
        ]);
        let prepared = prepare(&test_target(&acct, &cfg, &headers), json!({"input":[]}));
        let (_, websocket) = codex_ws_url(&acct, &cfg, &headers);
        for outbound in [&prepared.headers, &websocket] {
            assert_eq!(get_header(outbound, "authorization"), Some("Bearer synthetic-selected-api-key-B"));
            assert_eq!(get_header(outbound, "x-api-key"), None);
            assert_eq!(get_header(outbound, "chatgpt-account-id"), None);
            assert_eq!(get_header(outbound, "x-codex-turn-state"), Some("preserved"));
            assert_eq!(get_header(outbound, CODEX_RESIDENCY), Some("us"));
        }
    }

    #[test]
    fn codex_recognition_preserves_desktop_and_legacy_native_identity_on_both_builders() {
        let acct = oauth_account(Provider::Codex);
        let cfg = Config::default();
        for (agent, originator, version) in [
            (
                "Codex Desktop/0.164.0 (Mac OS 26.5.2; arm64) unknown (Codex Desktop; 2026.1024.1)",
                "Codex Desktop",
                Some("0.164.0"),
            ),
            ("codex_cli_rs/0.163.0 (Linux 6.12; x86_64) xterm", "codex_cli_rs", None),
        ] {
            let mut headers = request_headers(&[("user-agent", agent), ("originator", originator)]);
            if let Some(version) = version {
                headers.insert("version", version.parse().unwrap());
            }
            let prepared = prepare(&test_target(&acct, &cfg, &headers), json!({"input":[]}));
            let (_, websocket) = codex_ws_url(&acct, &cfg, &headers);
            for outbound in [&prepared.headers, &websocket] {
                assert_eq!(get_header(outbound, "user-agent"), Some(agent));
                assert_eq!(get_header(outbound, "originator"), Some(originator));
                assert_eq!(get_header(outbound, "version"), Some(version.unwrap_or("0.163.0")));
            }
        }
        let headers = request_headers(&[
            ("user-agent", "codex-tui/0.170.0 (Linux 6.12; x86_64) xterm"),
            ("originator", "codex-tui"),
            ("version", "0.170.0"),
            ("connection", "USER-AGENT"),
        ]);
        let prepared = prepare(&test_target(&acct, &cfg, &headers), json!({"input":[]}));
        let (_, websocket) = codex_ws_url(&acct, &cfg, &headers);
        for outbound in [&prepared.headers, &websocket] {
            assert_eq!(get_header(outbound, "user-agent"), Some(CODEX_USER_AGENT));
            assert_eq!(get_header(outbound, "originator"), Some(CODEX_ORIGINATOR));
            assert_eq!(get_header(outbound, "version"), Some(CODEX_VERSION));
        }
    }

    #[test]
    fn codex_configured_residency_fills_absence_and_connection_nominations_stay_filtered() {
        let mut acct = oauth_account(Provider::Codex);
        std::sync::Arc::get_mut(&mut acct).unwrap().headers.insert(CODEX_RESIDENCY.into(), "us".into());
        let cfg = Config::default();
        for nominated in [false, true] {
            let headers = if nominated {
                request_headers(&[
                    ("connection", "x-codex-turn-state, X-OpenAI-Internal-Codex-Residency"),
                    ("x-codex-turn-state", "private-nominated-state"),
                    (CODEX_RESIDENCY, "us"),
                ])
            } else {
                HeaderMap::new()
            };
            let prepared = prepare(&test_target(&acct, &cfg, &headers), json!({"input":[]}));
            let (_, websocket) = codex_ws_url(&acct, &cfg, &headers);
            for outbound in [&prepared.headers, &websocket] {
                assert_eq!(get_header(outbound, CODEX_RESIDENCY), if nominated { None } else { Some("us") });
                assert_eq!(get_header(outbound, "x-codex-turn-state"), None);
            }
        }
    }

    #[test]
    fn codex_explicit_sessions_replace_aliases_without_changing_cache_and_background_gets_fresh_uuid() {
        let acct = oauth_account(Provider::Codex);
        let cfg = Config::default();
        let headers = request_headers(&[("session_id", "caller-session"), ("session-id", "other-caller-session")]);
        let mut target = test_target(&acct, &cfg, &headers);
        target.cache_key = Some("shared-cache");
        let prepared = prepare(&target, json!({"input":[]}));
        assert_eq!(get_header(&prepared.headers, "session-id"), target.provider_session);
        assert_eq!(get_header(&prepared.headers, "session_id"), None);
        assert_eq!(prepared.body["prompt_cache_key"], "shared-cache");
        let headers = HeaderMap::new();
        let mut target = test_target(&acct, &cfg, &headers);
        target.provider_session = None;
        target.cache_key = Some("shared-cache");
        let mut sessions = HashSet::new();
        for _ in 0..2 {
            let prepared = prepare(&target, json!({"input":[]}));
            let session = get_header(&prepared.headers, "session-id").unwrap();
            assert!(uuid::Uuid::parse_str(session).is_ok());
            assert_ne!(session, acct.session_id);
            assert_ne!(session, "shared-cache");
            assert!(sessions.insert(session.to_string()));
        }
    }

    #[test]
    fn residency_validation_rejects_malformed_conflicting_and_hop_by_hop_requirements() {
        assert_eq!(residency_requirement(&HeaderMap::new()).unwrap(), None);
        assert_eq!(
            residency_requirement(&request_headers(&[(CODEX_RESIDENCY, "us"), (CODEX_RESIDENCY, "us")])).unwrap(),
            Some("us".into())
        );
        assert!(residency_requirement(&request_headers(&[(CODEX_RESIDENCY, "us"), (CODEX_RESIDENCY, "eu")])).is_err());
        assert!(
            residency_requirement(&request_headers(&[
                (CODEX_RESIDENCY, "us"),
                ("connection", "keep-alive, X-OpenAI-Internal-Codex-Residency")
            ]))
            .is_err()
        );
        for value in ["", " ", " us", "us ", "us,eu", "us,us", "US", "../us", "us eu", "-us", "us-", "region=us"] {
            let error = residency_requirement(&request_headers(&[(CODEX_RESIDENCY, value)])).unwrap_err();
            assert_eq!(error, "Invalid Codex residency requirement");
        }
        let mut headers = HeaderMap::new();
        headers.insert(CODEX_RESIDENCY, axum::http::HeaderValue::from_bytes(b"\xff").unwrap());
        assert!(residency_requirement(&headers).is_err());
    }

    #[test]
    fn extension_validation_never_authorizes_private_or_reserved_names() {
        for provider in [Provider::Claude, Provider::Codex] {
            for name in [
                "Authorization",
                "x-private-feature",
                "anthropic-access-token",
                "x-forwarded-user",
                "cf-connecting-ip",
                "x-gateway-key",
                "connection",
                "session-id",
                "session_id",
                "chatgpt-account-id",
                "x-claude-code-session-id",
                "user-agent",
                "not a header",
            ] {
                assert!(!header_extension_allowed(provider, name), "{name}");
            }
            assert!(header_extension_allowed(provider, "x-provider-feature"));
        }
        assert!(!header_extension_allowed(Provider::Claude, CODEX_RESIDENCY));
        assert!(header_extension_allowed(Provider::Codex, CODEX_RESIDENCY));
    }

    #[test]
    fn where_a_client_is_never_reaches_a_provider() {
        for name in [
            "x-forwarded-for",
            "forwarded",
            "x-real-ip",
            "tailscale-user-login",
            "tailscale-user-name",
            "x-fusebox-source",
        ] {
            assert!(!forwardable(name), "{name}");
        }
        assert!(forwardable("anthropic-version"));
    }

    #[test]
    fn muse_wonder_native_and_translated_reasoning_agree_without_selecting_full() {
        let cfg = Config::parse("auth-dir: /nonexistent\nmeta-api-key:\n  - api-key: synthetic-meta\n").unwrap();
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        let acct = pool.all().remove(0);
        let headers = HeaderMap::new();
        let opts = crate::formats::responses::BuildOpts {
            chatgpt_backend: false,
            custom_tools: false,
            default_reasoning: false,
        };
        for (model, effort, expected) in [
            ("muse-wonder-1.0", "max", "max"),
            ("muse-wonder-full-1.0", "max", "max"),
            ("muse-wonder-1.0", "none", "minimal"),
            ("muse-wonder-full-1.0", "none", "minimal"),
            ("muse-wonder-1.0", "high", "high"),
            ("muse-spark-1.3", "max", "max"),
            ("muse-spark-1.3-contributor", "max", "xhigh"),
            ("muse-spark-1.2", "max", "xhigh"),
        ] {
            let mut target = test_target(&acct, &cfg, &headers);
            target.model = model;
            let req = crate::ir::Request {
                reasoning: Some(crate::ir::Reasoning {
                    effort: Some(effort.into()),
                    disabled: effort == "none",
                    ..Default::default()
                }),
                ..Default::default()
            };
            let translated = crate::formats::responses::build_request(&req, model, &opts);
            for body in [json!({"input":"hello","reasoning":{"effort":effort}}), translated] {
                let prepared = prepare(&target, body);
                assert_eq!(prepared.url, "https://api.meta.ai/v1/responses");
                assert_eq!(prepared.body["model"], model);
                assert_eq!(prepared.body["reasoning"]["effort"], expected);
            }
        }
    }

    #[test]
    fn muse_requests_match_1_4_12_identity() {
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            meta_api_key: vec![crate::config::KeyEntry { api_key: "test".into(), ..Default::default() }],
            ..Default::default()
        };
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        let acct = pool.all().remove(0);
        let headers = HeaderMap::new();
        let prepared = prepare(
            &Target {
                acct: &acct,
                cfg: &cfg,
                client_headers: &headers,
                model: "muse-spark-1.3",
                wire: Format::Responses,
                passthrough: false,
                stream: true,
                count_tokens: false,
                cache_key: None,
                provider_session: None,
                cache_defaults: true,
            },
            json!({"input":"Reply OK"}),
        );
        assert_eq!(prepared.url, "https://api.meta.ai/v1/responses");
        for (name, expected) in [
            (
                "user-agent",
                "muse-build/1.4.12 (interactive; macos-aarch64; build 80d5266d8a4cb84d6d4ec595a4ef39cc596fa128)",
            ),
            ("x-client-id", "tbh:tui"),
            ("x-tbh-agent-role", "main"),
        ] {
            assert_eq!(
                prepared.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str()),
                Some(expected),
                "{name}"
            );
        }
        assert_eq!(prepared.body, json!({"input":"Reply OK","model":"muse-spark-1.3","stream":true}));
    }

    #[test]
    fn grok_subscription_requests_match_1_0_67_identity() {
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            xai_api_key: vec![crate::config::KeyEntry { api_key: "test".into(), ..Default::default() }],
            ..Default::default()
        };
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        let acct = pool.all().remove(0);
        *acct.cred.write() =
            Credential::OAuth(crate::accounts::OAuth { access_token: "test".into(), ..Default::default() });
        let headers = HeaderMap::new();
        let prepared = prepare(
            &Target {
                acct: &acct,
                cfg: &cfg,
                client_headers: &headers,
                model: "grok-4",
                wire: Format::Responses,
                passthrough: false,
                stream: true,
                count_tokens: false,
                cache_key: Some("conversation"),
                provider_session: None,
                cache_defaults: true,
            },
            json!({"input":"Reply OK"}),
        );
        assert_eq!(prepared.url, "https://cli-chat-proxy.grok.com/v1/responses");
        for (name, expected) in [
            ("x-grok-client-version", "1.0.67"),
            ("user-agent", "grok-pager/1.0.67 grok-shell/1.0.67 (macos; aarch64)"),
            ("x-grok-client-identifier", "grok-shell"),
            ("x-grok-client-mode", "interactive"),
            ("x-xai-token-auth", "xai-grok-cli"),
            ("x-authenticateresponse", "authenticate-response"),
            ("x-grok-conv-id", "conversation"),
        ] {
            assert_eq!(prepared.headers.iter().find(|(key, _)| key == name).unwrap().1, expected, "{name}");
        }
        assert_eq!(prepared.body, json!({"input":"Reply OK","model":"grok-4","stream":true}));
    }

    fn claude_account() -> std::sync::Arc<Account> {
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            claude_api_key: vec![crate::config::KeyEntry { api_key: "test".into(), ..Default::default() }],
            ..Default::default()
        };
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        pool.all().remove(0)
    }

    #[test]
    fn claude_generated_requests_match_2_1_309_identity() {
        let acct = claude_account();
        *acct.cred.write() = Credential::OAuth(crate::accounts::OAuth {
            access_token: "test".into(),
            refresh_token: String::new(),
            expires_at: None,
            email: None,
            account_id: None,
            base_url: None,
            project_id: None,
            raw: Default::default(),
        });
        let cfg = Config::default();
        let headers = HeaderMap::new();
        let prepared = prepare(
            &Target {
                acct: &acct,
                cfg: &cfg,
                client_headers: &headers,
                model: "claude-sonnet-4-6",
                wire: Format::Claude,
                passthrough: false,
                stream: true,
                count_tokens: false,
                cache_key: None,
                provider_session: None,
                cache_defaults: true,
            },
            json!({"messages":[{"role":"user","content":"Reply OK"}]}),
        );
        assert_eq!(
            prepared.headers.iter().find(|(name, _)| name == "user-agent").unwrap().1,
            "claude-cli/2.1.309 (external, cli)"
        );
        // The published 2.1.309 client's algorithm uses the same salt and samples.
        assert_eq!(
            prepared.body["system"][0]["text"],
            "x-anthropic-billing-header: cc_version=2.1.309.c22; cc_entrypoint=cli;"
        );
    }

    #[test]
    fn claude_cloak_preserves_four_caller_breakpoints_and_ttl_order() {
        let mut body = json!({
            "tools":[{"name":"lookup","input_schema":{"type":"object"},"cache_control":{"type":"ephemeral","ttl":"1h"}}],
            "system":[
                {"type":"text","text":"stable instructions","cache_control":{"type":"ephemeral","ttl":"1h"}},
                {"type":"text","text":"brief instructions","cache_control":{"type":"ephemeral","ttl":"5m"}}
            ],
            "messages":[{"role":"user","content":[{"type":"text","text":"question","cache_control":{"type":"ephemeral"}}]}]
        });
        cloak_body(&mut body, false, true);
        assert!(body["system"][1]["cache_control"].is_null());
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["text"], "<system-reminder>\nstable instructions\n</system-reminder>");
        assert_eq!(blocks[0]["cache_control"], json!({"type":"ephemeral","ttl":"1h"}));
        assert_eq!(blocks[1]["cache_control"], json!({"type":"ephemeral","ttl":"5m"}));
        assert_eq!(blocks[2]["cache_control"], json!({"type":"ephemeral"}));
        assert_eq!(body["tools"][0]["cache_control"], json!({"type":"ephemeral","ttl":"1h"}));
    }

    #[test]
    fn claude_cloak_respects_automatic_caching_and_retains_default_when_unconfigured() {
        let request = json!({"system":"instructions","messages":[{"role":"user","content":"question"}]});
        let mut automatic = request.clone();
        automatic["cache_control"] = json!({"type":"ephemeral","ttl":"1h"});
        cloak_body(&mut automatic, false, true);
        assert!(automatic["system"][1]["cache_control"].is_null());
        assert_eq!(automatic["cache_control"], json!({"type":"ephemeral","ttl":"1h"}));
        let mut default = request.clone();
        cloak_body(&mut default, false, true);
        assert_eq!(default["system"][1]["cache_control"], json!({"type":"ephemeral"}));
        // The conversation, which now holds the caller's instructions, is cached too.
        assert_eq!(default["cache_control"], json!({"type":"ephemeral"}));
        assert_eq!(default["messages"][0]["content"][0]["text"], "<system-reminder>\ninstructions\n</system-reminder>");
        assert!(default["messages"][0]["content"][0]["cache_control"].is_null());
        // Token counting has nothing to cache.
        let mut counted = request;
        cloak_body(&mut counted, true, true);
        assert!(counted["cache_control"].is_null());
    }

    #[test]
    fn translated_automatic_cache_covers_instructions_after_cloaking() {
        let original = json!({"instructions":"stable instructions","input":"question"});
        let req = crate::formats::responses::parse_request(&original).unwrap();
        let mut body = crate::formats::claude::build_request(&req, "claude-sonnet-5-5");
        crate::formats::claude::apply_translated_cache_policy(&original, &mut body);
        cloak_body(&mut body, false, true);
        assert_eq!(body["cache_control"], json!({"type":"ephemeral"}));
        assert!(body["system"][1]["cache_control"].is_null());
        assert_eq!(
            body["messages"][0]["content"][0]["text"],
            "<system-reminder>\nstable instructions\n</system-reminder>"
        );
        assert_eq!(body["messages"][0]["content"][1]["text"], "question");
    }

    #[test]
    fn claude_cloak_does_not_move_long_ttl_after_short_cached_tool_result() {
        let mut body = json!({
            "system":[{"type":"text","text":"instructions","cache_control":{"type":"ephemeral","ttl":"1h"}}],
            "messages":[
                {"role":"assistant","content":[{"type":"tool_use","id":"tool_1","name":"lookup","input":{}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"tool_1","content":"result","cache_control":{"type":"ephemeral"}}]}
            ]
        });
        let messages = body["messages"].clone();
        cloak_body(&mut body, false, true);
        assert_eq!(body["messages"], messages);
        assert_eq!(body["system"][2]["cache_control"], json!({"type":"ephemeral","ttl":"1h"}));
        assert_eq!(body["system"][2]["text"], "<system-reminder>\ninstructions\n</system-reminder>");
    }

    #[test]
    fn claude_cloak_preserves_ttl_order_before_assistant_and_identity_markers() {
        let mut body = json!({
            "system":[{"type":"text","text":"instructions","cache_control":{"type":"ephemeral","ttl":"1h"}}],
            "messages":[
                {"role":"assistant","content":[{"type":"tool_use","id":"tool_1","name":"lookup","input":{},"cache_control":{"type":"ephemeral"}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"tool_1","content":"result"}]}
            ]
        });
        let messages = body["messages"].clone();
        cloak_body(&mut body, false, true);
        assert_eq!(body["messages"], messages);
        assert_eq!(body["system"][2]["cache_control"]["ttl"], "1h");

        let mut body = json!({
            "system":[
                {"type":"text","text":"instructions","cache_control":{"type":"ephemeral","ttl":"1h"}},
                {"type":"text","text":CC_IDENTITY,"cache_control":{"type":"ephemeral"}}
            ],
            "messages":[{"role":"user","content":"question"}]
        });
        cloak_body(&mut body, false, true);
        assert_eq!(body["system"][1]["cache_control"]["ttl"], "1h");
        assert_eq!(body["system"][2]["text"], CC_IDENTITY);
        assert_eq!(body["system"][2]["cache_control"], json!({"type":"ephemeral"}));
        assert_eq!(body["messages"][0]["content"], json!([{"type":"text","text":"question"}]));
    }

    #[test]
    fn claude_native_code_preserves_cache_controls_without_cloaking() {
        let acct = claude_account();
        *acct.cred.write() = Credential::OAuth(crate::accounts::OAuth {
            access_token: "test".into(),
            refresh_token: String::new(),
            expires_at: None,
            email: None,
            account_id: None,
            base_url: None,
            project_id: None,
            raw: Default::default(),
        });
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", CC_USER_AGENT.parse().unwrap());
        let cfg = Config::default();
        let body = json!({
            "model":"claude-sonnet-4-6",
            "cache_control":{"type":"ephemeral","ttl":"1h"},
            "system":[{"type":"text","text":"instructions","cache_control":{"type":"ephemeral","ttl":"1h"}}],
            "messages":[{"role":"user","content":"question"}]
        });
        let prepared = prepare(
            &Target {
                acct: &acct,
                cfg: &cfg,
                client_headers: &headers,
                model: "claude-sonnet-4-6",
                wire: Format::Claude,
                passthrough: true,
                stream: false,
                count_tokens: false,
                cache_key: None,
                provider_session: None,
                cache_defaults: true,
            },
            body.clone(),
        );
        let mut actual = prepared.body.clone();
        actual.as_object_mut().unwrap().remove("metadata");
        assert_eq!(actual, body);
    }

    #[test]
    fn claude_cloak_adds_no_breakpoints_when_the_caller_marked_a_nested_block() {
        let mut body = json!({"system":"instructions","messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"t","name":"lookup","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":[
                {"type":"text","text":"result","cache_control":{"type":"ephemeral"}}
            ]}]}
        ]});
        cloak_body(&mut body, false, true);
        assert!(body["cache_control"].is_null());
        assert!(body["system"][1]["cache_control"].is_null());
    }

    #[test]
    fn claude_cloak_keeps_instructions_without_a_user_turn() {
        let mut body = json!({"system":[{"type":"text","text":"instructions","cache_control":{"type":"ephemeral","ttl":"1h"}}],"messages":[]});
        cloak_body(&mut body, false, true);
        assert_eq!(body["system"][2]["text"], "<system-reminder>\ninstructions\n</system-reminder>");
        assert_eq!(body["system"][2]["cache_control"], json!({"type":"ephemeral","ttl":"1h"}));
    }

    #[test]
    fn strict_chat_templates_are_opted_in_per_provider_for_translations() {
        let source = json!({"instructions":"base", "input":[
            {"role":"developer","content":"permissions"},
            {"role":"user","content":"agents"},
            {"role":"user","content":"prompt"}
        ]});
        let translated = crate::formats::chat::build_request(
            &crate::formats::responses::parse_request(&source).unwrap(),
            "local-model",
        );
        let headers = HeaderMap::new();
        for strict in [false, true] {
            let cfg = Config {
                auth_dir: "/nonexistent".into(),
                openai_compatibility: vec![crate::config::CompatEntry {
                    name: "local".into(),
                    base_url: "http://localhost:11434/v1".into(),
                    strict_chat_template: strict,
                    ..Default::default()
                }],
                ..Default::default()
            };
            let pool = crate::accounts::Pool::default();
            pool.reload(&cfg);
            let acct = pool.all().remove(0);
            for passthrough in [false, true] {
                let prepared = prepare(
                    &Target {
                        acct: &acct,
                        cfg: &cfg,
                        client_headers: &headers,
                        model: "local-model",
                        wire: Format::Chat,
                        passthrough,
                        stream: true,
                        count_tokens: false,
                        cache_key: None,
                        provider_session: None,
                        cache_defaults: true,
                    },
                    translated.clone(),
                );
                if strict && !passthrough {
                    assert_eq!(
                        prepared.body["messages"],
                        json!([
                            {"role":"system","content":"base\n\npermissions"},
                            {"role":"user","content":"agents\n\nprompt"}
                        ])
                    );
                } else {
                    assert_eq!(prepared.body["messages"], translated["messages"]);
                }
            }
        }
    }

    #[test]
    fn codex_system_roles_become_developer_without_moving_cache_boundaries() {
        let original = json!({"messages":[
            {"role":"system","content":"base"},
            {"role":"developer","content":"permissions"},
            {"role":"user","content":"question"},
            {"role":"system","content":[{"type":"text","text":"late", "prompt_cache_breakpoint":{"mode":"explicit","ttl":"12h"}}]},
            {"role":"user","content":"next"}
        ]});
        let translated = crate::formats::responses::build_request(
            &crate::formats::chat::parse_request(&original).unwrap(),
            "gpt-6.1-sol",
            &crate::formats::responses::BuildOpts {
                chatgpt_backend: true,
                custom_tools: true,
                default_reasoning: true,
            },
        );
        assert_eq!(translated["input"][0]["role"], "system", "public Responses keeps original roles");
        for websocket in [false, true] {
            let mut body = translated.clone();
            sanitize_codex_body(&mut body, "gpt-6.1-sol", websocket);
            let mut expected = translated["input"].clone();
            expected[0]["role"] = "developer".into();
            expected[3]["role"] = "developer".into();
            assert_eq!(body["input"], expected);
            let mut native = json!({"input":[{"role":"system","content":"native"}]});
            sanitize_codex_body(&mut native, "gpt-6.1-sol", websocket);
            assert_eq!(native["input"][0]["role"], "developer");
        }
    }

    #[test]
    fn codex_bodies_follow_codex_field_order_and_keep_websocket_fields() {
        let mut body = json!({"input": [], "instructions": "", "prompt_cache_key": "k", "x-extra": 1, "stream": true,
            "model": "m", "type": "response.create", "store": false, "generate": false, "previous_response_id": "r"});
        sanitize_codex_body(&mut body, "gpt-6.1-sol", true);
        order_codex_body(&mut body);
        let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "type",
                "model",
                "stream",
                "previous_response_id",
                "input",
                "store",
                "prompt_cache_key",
                "generate",
                "x-extra"
            ]
        );
        let mut http = json!({"generate": false, "previous_response_id": "r", "input": []});
        sanitize_codex_body(&mut http, "gpt-6.1-sol", false);
        assert!(http.get("generate").is_none() && http.get("previous_response_id").is_none());
    }

    #[test]
    fn codex_instructions_are_developer_input_on_both_transports() {
        for websocket in [false, true] {
            let mut body = json!({"instructions": "Follow these rules.", "input": "question"});
            sanitize_codex_body(&mut body, "gpt-6.1-sol", websocket);
            assert!(body.get("instructions").is_none());
            assert_eq!(
                body["input"],
                json!([
                    {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "Follow these rules."}]},
                    {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "question"}]}
                ])
            );
            let once = body.clone();
            sanitize_codex_body(&mut body, "gpt-6.1-sol", websocket);
            assert_eq!(body, once, "sanitizing again must not duplicate instructions");
        }
    }

    #[test]
    fn codex_native_developer_input_does_not_gain_an_instructions_field() {
        let input = json!([
            {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "native instructions"}]},
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "question"}]}
        ]);
        for websocket in [false, true] {
            for instructions in [None, Some(Value::Null), Some(json!(""))] {
                let mut body = json!({"input": input});
                if let Some(instructions) = instructions {
                    body["instructions"] = instructions;
                }
                sanitize_codex_body(&mut body, "gpt-6.1-sol", websocket);
                assert!(body.get("instructions").is_none());
                assert_eq!(body["input"], input);
            }
        }
    }

    #[test]
    fn codex_identity_is_the_clients_own_or_the_built_in_release() {
        let get = |h: &[(String, String)], k: &str| h.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let mut real = HeaderMap::new();
        real.insert("user-agent", "codex-tui/0.170.0 (Linux 6.12; x86_64) xterm (codex-tui; 0.170.0)".parse().unwrap());
        real.insert("originator", "codex-tui".parse().unwrap());
        real.insert("version", "0.170.0".parse().unwrap());
        let h = codex_headers(&real, "t", Some("acct"), true);
        assert_eq!(get(&h, "user-agent").unwrap(), "codex-tui/0.170.0 (Linux 6.12; x86_64) xterm (codex-tui; 0.170.0)");
        assert_eq!(get(&h, "version").unwrap(), "0.170.0");
        assert_eq!(get(&h, "chatgpt-account-id").unwrap(), "acct");
        let other = HeaderMap::new();
        let h = codex_headers(&other, "t", None, true);
        assert_eq!(get(&h, "user-agent").unwrap(), CODEX_USER_AGENT);
        assert_eq!(get(&h, "version").unwrap(), CODEX_VERSION);
        assert_eq!(get(&h, "originator").unwrap(), CODEX_ORIGINATOR);
        assert_eq!(h.iter().filter(|(k, _)| k == "user-agent").count(), 1);
    }

    #[test]
    fn codex_input_strings_become_lists() {
        let mut body = json!({ "input": "hi", "max_output_tokens": 5 });
        sanitize_codex_body(&mut body, "gpt-6-astra", false);
        assert_eq!(body["input"][0]["content"][0]["text"], "hi");
        assert!(body.get("max_output_tokens").is_none());
        assert_eq!(body["store"], false);
    }

    #[test]
    fn fingerprint_is_three_hex_chars() {
        let f = cc_fingerprint("hello world, this is a test");
        assert_eq!(f.len(), 3);
        assert!(f.chars().all(|c| c.is_ascii_hexdigit()));
        // Short messages fall back to '0' for missing positions.
        assert_eq!(cc_fingerprint(""), cc_fingerprint("abc"));
    }
}
