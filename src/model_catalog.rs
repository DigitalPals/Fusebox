//! Capabilities follow the resolved upstream route, including aliases/prefixes.
//! Shared names and fallback groups advertise only what every route guarantees.
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::accounts::Provider;
use crate::state::App;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Capabilities {
    #[serde(skip_serializing_if = "Option::is_none", alias = "context-window")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", alias = "max-output-tokens")]
    pub max_output_tokens: Option<u64>,
    /// None means unknown, rather than text-only or no image support.
    #[serde(skip_serializing_if = "Option::is_none", alias = "input-modalities")]
    pub input_modalities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", alias = "output-modalities")]
    pub output_modalities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", alias = "tool-calling")]
    pub tool_calling: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", alias = "reasoning-efforts")]
    pub reasoning_efforts: Option<Vec<String>>,
}

impl Capabilities {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.context_window != Some(0) && self.max_output_tokens != Some(0),
            "Model token limits must be positive"
        );
        for list in [&self.input_modalities, &self.output_modalities, &self.reasoning_efforts].into_iter().flatten() {
            anyhow::ensure!(
                list.len() <= 32 && list.iter().all(|s| !s.trim().is_empty() && s.len() <= 64),
                "Invalid model capability list"
            );
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    fn overlay(&mut self, other: &Self) {
        if other.context_window.is_some() {
            self.context_window = other.context_window;
        }
        if other.max_output_tokens.is_some() {
            self.max_output_tokens = other.max_output_tokens;
        }
        if other.input_modalities.is_some() {
            self.input_modalities.clone_from(&other.input_modalities);
        }
        if other.output_modalities.is_some() {
            self.output_modalities.clone_from(&other.output_modalities);
        }
        if other.tool_calling.is_some() {
            self.tool_calling = other.tool_calling;
        }
        if other.reasoning_efforts.is_some() {
            self.reasoning_efforts.clone_from(&other.reasoning_efforts);
        }
    }
}

fn positive(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.parse().ok()).filter(|n| *n > 0)
}

fn strings(v: &Value) -> Option<Vec<String>> {
    let a = v.as_array()?;
    a.iter().map(|s| s.as_str().map(|s| s.to_ascii_lowercase())).collect()
}

pub fn claude(v: &Value) -> Capabilities {
    let caps = &v["capabilities"];
    let mut modalities = strings(&v["input_modalities"]);
    if modalities.is_none() {
        // A definite no is useful too; absent support stays unknown.
        if let Some(image) =
            caps["image_input"]["supported"].as_bool().or_else(|| caps["vision"]["supported"].as_bool())
        {
            modalities = Some(if image { vec!["text".into(), "image".into()] } else { vec!["text".into()] });
        }
    }
    let efforts = caps["effort"]
        .as_object()
        .filter(|_| caps["effort"]["supported"] == true)
        .map(|e| e.iter().filter(|(_, v)| v["supported"] == true).map(|(k, _)| k.clone()).collect());
    Capabilities {
        context_window: positive(&v["max_input_tokens"]).or_else(|| positive(&v["context_window"])),
        max_output_tokens: positive(&v["max_tokens"]).or_else(|| positive(&v["max_output_tokens"])),
        input_modalities: modalities,
        output_modalities: strings(&v["output_modalities"]),
        tool_calling: caps["tool_use"]["supported"].as_bool(),
        reasoning_efforts: efforts,
    }
}

pub fn codex(v: &Value) -> Capabilities {
    Capabilities {
        context_window: positive(&v["context_window"]),
        max_output_tokens: positive(&v["max_output_tokens"]),
        input_modalities: strings(&v["input_modalities"]),
        output_modalities: strings(&v["output_modalities"]),
        tool_calling: v["supports_tool_calls"].as_bool(),
        reasoning_efforts: v["supported_reasoning_levels"]
            .as_array()
            .map(|levels| levels.iter().filter_map(|l| l["effort"].as_str().map(String::from)).collect()),
    }
}

fn intersect(lists: impl Iterator<Item = Option<Vec<String>>>) -> Option<Vec<String>> {
    let mut lists = lists;
    let mut result = lists.next()??;
    for list in lists {
        let list = list?;
        result.retain(|s| list.contains(s));
    }
    Some(result)
}

fn minimum(values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    let mut values = values;
    let mut min = values.next()??;
    for value in values {
        min = min.min(value?);
    }
    Some(min)
}

fn combine(caps: &[Capabilities]) -> Capabilities {
    Capabilities {
        context_window: minimum(caps.iter().map(|c| c.context_window)),
        max_output_tokens: minimum(caps.iter().map(|c| c.max_output_tokens)),
        input_modalities: intersect(caps.iter().map(|c| c.input_modalities.clone())),
        output_modalities: intersect(caps.iter().map(|c| c.output_modalities.clone())),
        tool_calling: if caps.iter().any(|c| c.tool_calling == Some(false)) {
            Some(false)
        } else if !caps.is_empty() && caps.iter().all(|c| c.tool_calling == Some(true)) {
            Some(true)
        } else {
            None
        },
        reasoning_efforts: intersect(caps.iter().map(|c| c.reasoning_efforts.clone())),
    }
}

fn routes(app: &App, public: &str) -> Vec<(Provider, Option<crate::discovery::Listed>)> {
    let (only, model) = app.pool.route(public);
    app.pool
        .all()
        .into_iter()
        .filter(|a| !a.state.lock().disabled)
        .filter_map(|a| {
            let canonical = app.pool.resolve_account(&a, &model, only.as_ref())?;
            Some((a.provider, app.models.listed(&a, &canonical)))
        })
        .collect()
}

fn direct(app: &App, public: &str) -> Capabilities {
    let caps: Vec<_> =
        routes(app, public).into_iter().map(|(_, m)| m.map(|m| m.capabilities).unwrap_or_default()).collect();
    let mut caps = combine(&caps);
    if let Some(overrides) = app.cfg().model_capabilities.get(public) {
        caps.overlay(overrides);
    }
    caps
}

pub fn capabilities(app: &App, public: &str) -> Capabilities {
    let cfg = app.cfg();
    let mut caps = match cfg.model_groups.get(public) {
        Some(group) => combine(&group.models.iter().map(|id| direct(app, id)).collect::<Vec<_>>()),
        None => direct(app, public),
    };
    if let Some(overrides) = cfg.model_capabilities.get(public) {
        caps.overlay(overrides);
    }
    caps
}

/// Preserve the provider's complete Codex template. Inventing a sparse template
/// can silently drop tools/instructions. Mixed routes cannot share one template.
pub fn codex_template(app: &App, public: &str) -> Option<Value> {
    if app.cfg().model_groups.contains_key(public) {
        return None;
    }
    let mut template = None;
    for (provider, listed) in routes(app, public) {
        if provider != Provider::Codex {
            return None;
        }
        let raw = listed?.codex?;
        if template.as_ref().is_some_and(|t| t != &raw) {
            return None;
        }
        template = Some(raw);
    }
    let mut template = template?;
    template["slug"] = public.into();
    template["upgrade"] = Value::Null;
    let caps = capabilities(app, public);
    if let Some(n) = caps.context_window {
        template["context_window"] = n.into();
    }
    if let Some(modalities) = caps.input_modalities {
        template["input_modalities"] = serde_json::json!(modalities);
    }
    Some(template)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn discovery_keeps_reported_limits_without_guessing_missing_capabilities() {
        let caps = claude(
            &json!({"max_input_tokens":200000,"max_tokens":64000,"capabilities":{"image_input":{"supported":true},"tool_use":{"supported":false}}}),
        );
        assert_eq!(caps.context_window, Some(200000));
        assert_eq!(caps.max_output_tokens, Some(64000));
        assert!(caps.input_modalities.unwrap().contains(&"image".into()));
        assert_eq!(caps.tool_calling, Some(false));
        assert!(codex(&json!({})).is_empty());
    }

    #[test]
    fn shared_routes_use_smallest_limits_and_common_capabilities() {
        let a = codex(&json!({"context_window":500000,"input_modalities":["text","image"]}));
        let b = codex(&json!({"context_window":128000,"input_modalities":["text"]}));
        let caps = combine(&[a.clone(), b]);
        assert_eq!(caps.context_window, Some(128000));
        assert_eq!(caps.input_modalities, Some(vec!["text".into()]));
        assert!(combine(&[a, Capabilities::default()]).is_empty(), "an unreported route is unknown");
    }
}
