//! Cache intent and effective policy, without prompt content or raw cache keys.

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

/// The caller's explicit key always wins. Only derived keys use shared scopes;
/// account assignments remain tied to sessions and authenticated client scopes.
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
    has_fields(value, &["cache_control", "prompt_cache_breakpoint"])
}

pub fn has_claude_controls(value: &Value) -> bool {
    has_fields(value, &["cache_control"])
}

fn has_fields(value: &Value, fields: &[&str]) -> bool {
    match value {
        Value::Object(object) => {
            fields.iter().any(|field| object.get(*field).is_some_and(|v| !v.is_null()))
                || object.values().any(|value| has_fields(value, fields))
        }
        Value::Array(values) => values.iter().any(|value| has_fields(value, fields)),
        _ => false,
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
