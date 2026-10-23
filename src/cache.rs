//! Cache intent, effective policy and provider diagnostics without prompt content or raw identifiers.

use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ir::Format;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    pub scope: Scope,
    /// Change when a shared prompt/tool schema version should have its own key.
    pub namespace: String,
    pub on_incompatible: Incompatible,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    #[default]
    Session,
    Client,
    Workspace,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Incompatible {
    /// Preserve the existing permissive request behavior and explicit intent.
    #[default]
    Drop,
    Automatic,
    Reject,
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.namespace.len() > 256 {
            return Err("prompt-cache namespace must be at most 256 bytes".into());
        }
        Ok(())
    }
}

/// The caller's explicit key, or a native Codex client's omitted key, always wins.
/// Only derived keys use shared scopes; account assignments remain tied to sessions
/// and authenticated client scopes.
pub fn key(
    headers: &HeaderMap,
    body: &Value,
    session: Option<&str>,
    source: Option<&str>,
    config: &Config,
) -> (Option<String>, &'static str) {
    if body["prompt_cache_key"].is_string() {
        return (None, "caller");
    }
    // Codex 0.164 can deliberately omit this field independently of session-id.
    // HTTP and native WebSocket requests share this decision: do not synthesize
    // a key that undoes the client's policy, even under a shared cache scope.
    if body["prompt_cache_key"].is_null() && crate::clients::is_codex(headers) {
        return (None, "caller_omitted");
    }
    let session_key = || {
        session.filter(|_| source != Some("generated_response")).map(|session| {
            if config.namespace.is_empty() {
                crate::affinity::prompt_cache_key(session)
            } else {
                scoped_key(&["session", session, &config.namespace])
            }
        })
    };
    match config.scope {
        Scope::Session => (session_key(), "session"),
        Scope::Client => {
            (Some(scoped_key(&["client", &crate::affinity::client_scope(headers), &config.namespace])), "client")
        }
        Scope::Workspace => {
            let workspace = headers
                .get("x-fusebox-cache-workspace")
                .and_then(|h| h.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty() && s.len() <= 1024);
            match workspace {
                Some(workspace) => (
                    Some(scoped_key(&[
                        "workspace",
                        &crate::affinity::client_scope(headers),
                        workspace,
                        &config.namespace,
                    ])),
                    "workspace",
                ),
                None => (session_key(), "session_fallback"),
            }
        }
    }
}

fn scoped_key(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    crate::affinity::prompt_cache_key(&hex::encode(hash.finalize()))
}

#[derive(Debug, Clone, Serialize)]
pub struct Policy {
    pub requested: &'static str,
    pub applied: &'static str,
    pub key_scope: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downgrade_reason: Option<String>,
}

/// The provider's explanation of cache reuse, independent of requested controls.
/// Keep only known classifications and bounded counts: diagnostic responses can
/// also contain identifiers or future free-text fields that must not reach logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostics {
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comparison_reusable_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_missed_tokens: Option<u64>,
}

impl Diagnostics {
    fn from_wire(value: &Value) -> Option<Self> {
        // Both full Responses objects and response.* SSE/WebSocket events.
        // Never search recursively through arbitrary model or tool output.
        let diagnostic =
            value["response"].get("prompt_cache_diagnostics").or_else(|| value.get("prompt_cache_diagnostics"))?;
        let diagnostic = diagnostic.as_object()?;
        let kind = match diagnostic.get("type").and_then(Value::as_str) {
            Some("cache_hit") => "cache_hit",
            Some("cache_miss") => "cache_miss",
            Some("comparison_response_not_found") => "comparison_response_not_found",
            _ => "unavailable",
        };
        let reason = (kind == "cache_miss").then(|| match diagnostic.get("reason").and_then(Value::as_str) {
            Some("model_changed") => "model_changed",
            Some("prompt_cache_key_changed") => "prompt_cache_key_changed",
            Some("service_tier_changed") => "service_tier_changed",
            Some("tools_changed") => "tools_changed",
            Some("text_format_changed") => "text_format_changed",
            Some("reasoning_effort_changed") => "reasoning_effort_changed",
            Some("verbosity_changed") => "verbosity_changed",
            Some("context_compacted") => "context_compacted",
            Some("input_changed") => "input_changed",
            _ => "unknown",
        });
        // JSON dashboard numbers must be exact; reject negatives, strings,
        // fractional values and numbers outside JavaScript's safe integer range.
        let tokens = |name: &str| {
            (kind == "cache_miss")
                .then(|| diagnostic.get(name).and_then(Value::as_u64).filter(|n| *n <= 9_007_199_254_740_991))
                .flatten()
        };
        Some(Self {
            kind,
            reason,
            comparison_reusable_tokens: tokens("comparison_reusable_tokens"),
            cache_missed_tokens: tokens("cache_missed_tokens"),
        })
    }
}

/// Shared with translated stream decoders, which outlive request preparation.
/// Only a sanitized diagnostic survives beyond the original wire event.
#[derive(Clone, Default)]
pub(crate) struct DiagnosticObserver(std::sync::Arc<parking_lot::Mutex<Option<Diagnostics>>>);

impl DiagnosticObserver {
    pub(crate) fn observe(&self, value: &Value) {
        if let Some(diagnostic) = Diagnostics::from_wire(value) {
            *self.0.lock() = Some(diagnostic);
        }
    }
    pub(crate) fn snapshot(&self) -> Option<Diagnostics> {
        self.0.lock().clone()
    }
    pub(crate) fn reset(&self) {
        *self.0.lock() = None;
    }
}

/// What the caller's own body asks of the cache, read once per request: each
/// question walks the whole body, and retries ask again for every attempt.
pub struct Requested {
    pub intent: &'static str,
    /// Any cache marker anywhere in the body (Claude's or OpenAI's).
    pub markers: bool,
    pub explicit_mode: bool,
}

impl Requested {
    pub fn of(original: &Value) -> Self {
        let markers = has_markers(original);
        Self {
            intent: intent_with(original, markers),
            markers,
            explicit_mode: original["prompt_cache_options"]["mode"] == "explicit",
        }
    }
}

impl Policy {
    pub fn for_request(
        requested: &Requested,
        prepared: &Value,
        wire: Format,
        scope: &'static str,
        downgrade: Option<String>,
    ) -> Self {
        let applied = intent(prepared);
        Self {
            requested: requested.intent,
            applied: if applied == "provider_default" && wire == Format::Claude { "none" } else { applied },
            key_scope: if prepared["prompt_cache_key"].is_string() { scope } else { "not_sent" },
            downgrade_reason: downgrade,
        }
    }

    pub fn new(
        original: &Value,
        prepared: &Value,
        wire: Format,
        scope: &'static str,
        downgrade: Option<String>,
    ) -> Self {
        let applied = intent(prepared);
        Self {
            requested: intent(original),
            applied: if applied == "provider_default" && wire == Format::Claude { "none" } else { applied },
            key_scope: if prepared["prompt_cache_key"].is_string() { scope } else { "not_sent" },
            downgrade_reason: downgrade,
        }
    }
}

pub fn has_markers(value: &Value) -> bool {
    has_fields(value, &["cache_control", "prompt_cache_breakpoint"], &mut Vec::new())
}

pub fn has_claude_controls(value: &Value) -> bool {
    has_fields(value, &["cache_control"], &mut Vec::new())
}

/// Schemas and tool arguments are caller data. Everywhere else the scan stays
/// conservative, including unknown block types and nested tool-result content:
/// missing a real marker could add a fifth Anthropic breakpoint.
fn has_fields<'a>(value: &'a Value, fields: &[&str], path: &mut Vec<&'a str>) -> bool {
    match value {
        Value::Object(object) => {
            fields.iter().any(|field| object.get(*field).is_some_and(|v| !v.is_null()))
                || object.iter().any(|(key, value)| {
                    let tool_schema = matches!(key.as_str(), "input_schema" | "parameters")
                        && (path.as_slice() == ["tools"] || path.as_slice() == ["tools", "function"])
                        || matches!(key.as_str(), "parameters" | "parametersJsonSchema" | "parameters_json_schema")
                            && matches!(path.as_slice(), ["tools", "functionDeclarations" | "function_declarations"]);
                    let arguments = (key == "input"
                        && matches!(object.get("type").and_then(Value::as_str), Some("tool_use" | "server_tool_use")))
                        || (key == "arguments"
                            && (matches!(object.get("type").and_then(Value::as_str), Some("function_call"))
                                || path.ends_with(&["tool_calls", "function"])))
                        || key == "args" && matches!(path.last(), Some(&"functionCall" | &"function_call"));
                    let output_schema = key == "schema"
                        && matches!(
                            path.as_slice(),
                            ["output_config", "format"] | ["response_format", "json_schema"] | ["text", "format"]
                        )
                        || matches!(
                            key.as_str(),
                            "responseSchema" | "responseJsonSchema" | "response_schema" | "response_json_schema"
                        ) && matches!(path.as_slice(), ["generationConfig" | "generation_config"]);
                    if tool_schema || arguments || output_schema {
                        return false;
                    }
                    path.push(key);
                    let found = has_fields(value, fields, path);
                    path.pop();
                    found
                })
        }
        Value::Array(values) => values.iter().any(|value| has_fields(value, fields, path)),
        _ => false,
    }
}

/// Stable compact codes for the allowlisted persistent numeric metadata.
pub fn intent_code(intent: &str) -> u64 {
    match intent {
        "provider_default" => 1,
        "none" => 2,
        "automatic" => 3,
        "explicit" => 4,
        "explicit_no_breakpoints" => 5,
        "prewarm" => 6,
        "rejected" => 7,
        _ => 0,
    }
}

fn intent(body: &Value) -> &'static str {
    let explicit = body["prompt_cache_options"]["mode"] == "explicit";
    let automatic = !body["cache_control"].is_null() || body["prompt_cache_options"]["mode"] == "implicit";
    // Walk the body for markers only when the answer depends on them.
    let markers = (body["prompt_cache_options"]["prewarm"] != true && (explicit || !automatic)) && has_markers(body);
    intent_with(body, markers)
}

fn intent_with(body: &Value, markers: bool) -> &'static str {
    if body["prompt_cache_options"]["prewarm"] == true {
        "prewarm"
    } else if body["prompt_cache_options"]["mode"] == "explicit" {
        if markers { "explicit" } else { "explicit_no_breakpoints" }
    } else if !body["cache_control"].is_null() || body["prompt_cache_options"]["mode"] == "implicit" {
        "automatic"
    } else if markers {
        "explicit"
    } else {
        "provider_default"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_keys_remain_compatible_and_never_randomize_generated_sessions() {
        let config = Config::default();
        let headers = HeaderMap::new();
        let derive = |session, source| key(&headers, &json!({}), session, source, &config).0;
        assert_eq!(derive(Some("task"), None), Some(crate::affinity::prompt_cache_key("task")));
        assert_eq!(derive(Some("task"), Some("generated_response")), None);
        assert_eq!(derive(None, None), None);
        assert_eq!(key(&headers, &json!({"prompt_cache_key":"caller"}), Some("task"), None, &config), (None, "caller"));
    }

    #[test]
    fn native_codex_can_omit_cache_keys_independently_of_session_and_shared_scopes() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "codex_cli_rs/0.164.0".parse().unwrap());
        headers.insert("originator", "codex_cli_rs".parse().unwrap());
        headers.insert("session-id", "native-session".parse().unwrap());
        headers.insert("x-fusebox-cache-workspace", "repo".parse().unwrap());
        for scope in [Scope::Session, Scope::Client, Scope::Workspace] {
            let config = Config { scope, ..Default::default() };
            for body in [json!({}), json!({"prompt_cache_key":null}), json!({"type":"response.create"})] {
                assert_eq!(key(&headers, &body, Some("task"), None, &config), (None, "caller_omitted"));
            }
            assert_eq!(
                key(&headers, &json!({"prompt_cache_key":"caller"}), Some("task"), None, &config),
                (None, "caller")
            );
        }
        headers.remove("originator");
        assert!(key(&headers, &json!({}), Some("task"), None, &Config::default()).0.is_some());
    }

    #[test]
    fn shared_keys_keep_clients_workspaces_and_prompt_versions_separate() {
        let mut headers = HeaderMap::new();
        headers.insert(crate::affinity::SCOPE_HEADER, "client-a".parse().unwrap());
        let mut config = Config { scope: Scope::Client, ..Default::default() };
        let derive = |headers: &HeaderMap, config: &Config, session| {
            key(headers, &json!({}), Some(session), None, config).0.unwrap()
        };
        let shared = derive(&headers, &config, "task-a");
        assert_eq!(shared, derive(&headers, &config, "task-b"));
        headers.insert(crate::affinity::SCOPE_HEADER, "client-b".parse().unwrap());
        assert_ne!(shared, derive(&headers, &config, "task-a"));
        config.scope = Scope::Workspace;
        assert_eq!(key(&headers, &json!({}), Some("task-a"), None, &config).1, "session_fallback");
        headers.insert("x-fusebox-cache-workspace", "repo-one".parse().unwrap());
        let workspace = derive(&headers, &config, "task-a");
        assert_eq!(workspace, derive(&headers, &config, "task-b"));
        headers.insert("x-fusebox-cache-workspace", "repo-two".parse().unwrap());
        assert_ne!(workspace, derive(&headers, &config, "task-a"));
        let before = derive(&headers, &config, "task-a");
        config.namespace = "tools-v2".into();
        assert_ne!(before, derive(&headers, &config, "task-a"));
    }

    #[test]
    fn policy_records_downgrades_without_prompt_content_or_keys() {
        let original = json!({"prompt_cache_key":"private-key", "input":"private-prompt", "prompt_cache_options":{"mode":"explicit"}});
        let policy = Policy::new(&original, &json!({}), Format::Claude, "caller", Some("incompatible format".into()));
        assert_eq!(policy.requested, "explicit_no_breakpoints");
        assert_eq!(policy.applied, "none");
        let encoded = serde_json::to_string(&policy).unwrap();
        assert!(!encoded.contains("private"));
        assert_eq!(policy.key_scope, "not_sent");
    }
}
