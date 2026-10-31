//! Provider payloads are partial observations. Never turn a missing number into
//! zero, a balance into dollars, or a feature allowance into an account limit.
use super::*;

fn field<'a>(v: &'a Value, snake: &str, camel: &str) -> &'a Value {
    v.get(snake).or_else(|| v.get(camel)).unwrap_or(&Value::Null)
}

fn number(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.trim().parse().ok()).filter(|n| n.is_finite())
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

fn reset(v: &Value) -> Option<DateTime<Utc>> {
    for key in ["resets_at", "resetsAt", "reset_at", "resetAt", "reset_time", "resetTime"] {
        let raw = &v[key];
        if let Some(t) = raw.as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()) {
            return Some(t.with_timezone(&Utc));
        }
        if let Some(n) = number(raw).filter(|n| *n > 0.0)
            && let Some(t) = ts(n as i64)
        {
            return Some(t);
        }
    }
    number(field(v, "reset_after_seconds", "resetAfterSeconds"))
        .filter(|n| *n >= 0.0 && *n < 366.0 * 86_400.0)
        .and_then(|n| Utc::now().checked_add_signed(chrono::Duration::seconds(n as i64)))
}

fn limit(id: &str, name: &str, scope: Scope, raw: &Value) -> Limit {
    let cap = number(&raw["limit"]);
    let used = number(&raw["used"]);
    let remaining = number(&raw["remaining"]);
    let percent = number(field(raw, "remaining_percent", "remainingPercent"))
        .map(|n| 100.0 - n)
        .or_else(|| number(field(raw, "used_percent", "usedPercent")))
        .or_else(|| cap.filter(|n| *n > 0.0).and_then(|c| remaining.map(|r| (1.0 - r / c) * 100.0)))
        .or_else(|| cap.filter(|n| *n > 0.0).and_then(|c| used.map(|u| u / c * 100.0)))
        .or_else(|| (cap == Some(0.0)).then_some(100.0));
    Limit {
        id: id.into(),
        name: name.into(),
        scope,
        used_percent: percent.map(|n| n.clamp(0.0, 100.0)),
        used,
        limit: cap,
        remaining,
        unit: text(&raw["unit"]),
        unlimited: raw["unlimited"].as_bool(),
        enabled: None,
        resets_at: reset(raw),
    }
}

fn rate(v: &Value, id: &str, name: &str, scope: Scope, out: &mut Reading) {
    if !v.is_object() {
        return;
    }
    let reached = field(v, "limit_reached", "limitReached") == true || v["allowed"] == false;
    let account = scope == Scope::Account;
    let mut exhausted_resets = Vec::new();
    let mut exhausted = false;
    for (snake, camel) in [("primary_window", "primaryWindow"), ("secondary_window", "secondaryWindow")] {
        let w = field(v, snake, camel);
        if !w.is_object() {
            continue;
        }
        let used = number(field(w, "used_percent", "usedPercent")).or_else(|| reached.then_some(100.0));
        let Some(used) = used else { continue };
        let secs = number(field(w, "limit_window_seconds", "limitWindowSeconds"))
            .filter(|s| *s >= 1.0 && s.fract() == 0.0 && *s < 366.0 * 86_400.0)
            .map(|s| s as i64);
        let used = if reached && used >= 99.0 { 100.0 } else { used.clamp(0.0, 100.0) };
        if used >= 100.0 {
            exhausted = true;
            exhausted_resets.extend(reset(w));
        }
        if account && let Some(s) = secs {
            out.windows.push(Window { name: label(s), used, resets_at: reset(w), model: None });
        } else {
            let mut l = limit(
                &format!("{id}:{snake}"),
                &format!("{name} · {}", secs.map(label).unwrap_or_else(|| snake.trim_end_matches("_window").into())),
                scope.clone(),
                w,
            );
            l.used_percent = Some(used);
            out.limits.push(l);
        }
    }
    // An explicit provider gate matters even without percentages/window lengths.
    // Keep it separate so a later `allowed: true` can clear precisely this gate.
    if reached || v["allowed"] == true || field(v, "limit_reached", "limitReached") == false {
        let mut l = limit(&format!("{id}:gate"), name, scope, v);
        l.used_percent = Some(if reached && !exhausted { 100.0 } else { 0.0 });
        if reached {
            // A known exhausted window supplies the gate's renewal. Do not
            // guess a renewal from a window which still has headroom.
            l.resets_at = exhausted_resets.into_iter().max().or(l.resets_at);
        }
        out.limits.push(l);
    }
}

pub(super) fn codex(v: &Value) -> Reading {
    let mut out = Reading { plan: text(field(v, "plan_type", "planType")), ..Default::default() };
    rate(field(v, "rate_limit", "rateLimit"), "codex", "Account allowance", Scope::Account, &mut out);
    rate(
        field(v, "code_review_rate_limit", "codeReviewRateLimit"),
        "code_review",
        "Code review",
        Scope::Feature("code_review".into()),
        &mut out,
    );
    for (i, item) in
        field(v, "additional_rate_limits", "additionalRateLimits").as_array().into_iter().flatten().enumerate()
    {
        let feature = text(field(item, "metered_feature", "meteredFeature"));
        let name = text(field(item, "limit_name", "limitName"))
            .or_else(|| feature.clone())
            .unwrap_or_else(|| format!("Additional allowance {}", i + 1));
        let scope = text(field(item, "model_id", "modelId"))
            .or_else(|| {
                feature
                    .as_ref()
                    .filter(|f| {
                        (f.starts_with("gpt-") || f.starts_with("codex-")) && crate::discovery::valid_model_id(f)
                    })
                    .cloned()
            })
            .map(Scope::Model)
            .unwrap_or_else(|| Scope::Feature(feature.clone().unwrap_or_else(|| name.clone())));
        rate(
            field(item, "rate_limit", "rateLimit"),
            &format!("additional:{}", feature.as_deref().unwrap_or(&name)),
            &name,
            scope,
            &mut out,
        );
    }
    let spend = field(field(v, "spend_control", "spendControl"), "individual_limit", "individualLimit");
    if spend.is_object() {
        out.limits.push(limit("individual_spending", "Individual spending limit", Scope::Account, spend));
    }
    if v["credits"].is_object() {
        let mut l = limit("credits", "Account credits", Scope::Credits, &v["credits"]);
        l.remaining = number(&v["credits"]["balance"]);
        out.limits.push(l);
    }
    out
}

/// Verified Ultra accounts have no 5-hour limit: usage says `five_hour: null`, and its
/// typed list, which names every account-wide limit, has a weekly entry but no session.
/// A null alone is not enough, since plans with the limit report an idle window that way.
pub fn claude_no_five_hour(v: &Value) -> bool {
    let typed = v["limits"].as_array().map(Vec::as_slice).unwrap_or_default();
    v.get("five_hour").is_some_and(Value::is_null)
        && typed.iter().any(|w| w["kind"] == "weekly_all")
        && !typed.iter().any(|w| w["kind"] == "session")
}

pub(super) fn claude(v: &Value) -> Reading {
    let mut out = Reading { no_five_hour: claude_no_five_hour(v), ..Default::default() };
    for (key, name, model) in [
        ("five_hour", "5h", None),
        ("seven_day", "week", None),
        ("seven_day_opus", "week opus", Some("opus")),
        ("seven_day_sonnet", "week sonnet", Some("sonnet")),
        ("seven_day_oauth_apps", "week oauth apps", None),
        ("seven_day_overage_included", "week overage", None),
        ("iguana_necktie", "week fable", Some("fable")),
    ] {
        let w = &v[key];
        if let Some(used) = number(&w["utilization"]) {
            out.windows.push(Window {
                name: name.into(),
                used: used.clamp(0.0, 100.0),
                resets_at: reset(w),
                model: model.map(String::from),
            });
        }
    }
    let cowork = &v["seven_day_cowork"];
    if let Some(used) = number(&cowork["utilization"]) {
        let mut l = limit("cowork", "Weekly Cowork", Scope::Feature("cowork".into()), cowork);
        l.used_percent = Some(used.clamp(0.0, 100.0));
        out.limits.push(l);
    }
    let typed: Vec<_> = v["limits"].as_array().into_iter().flatten().collect();
    let is_fable = |w: &&Value| {
        w["kind"] == "weekly_scoped"
            && field(&w["scope"]["model"], "display_name", "displayName")
                .as_str()
                .is_some_and(|name| matches!(name.to_ascii_lowercase().as_str(), "fable" | "fable 5"))
            && number(&w["percent"]).is_some()
    };
    let fable = typed
        .iter()
        .copied()
        .filter(is_fable)
        .find(|w| w["is_active"] == true)
        .or_else(|| typed.iter().copied().find(is_fable));
    if let Some(w) = fable {
        out.windows.retain(|old| old.model.as_deref() != Some("fable"));
        out.windows.push(Window {
            name: "week fable".into(),
            used: number(&w["percent"]).unwrap().clamp(0.0, 100.0),
            // Explicitly inactive data retires a previously observed cap.
            resets_at: if w["is_active"] == false { Some(Utc::now()) } else { reset(w) },
            model: Some("fable".into()),
        });
    }
    for w in typed {
        let Some(used) = number(&w["percent"]) else { continue };
        // The account-wide entries repeat `five_hour` and `seven_day`; they stand in only
        // when those are missing.
        let whole = match w["kind"].as_str() {
            Some("session") => Some("5h"),
            Some("weekly_all") => Some("week"),
            _ => None,
        };
        if let Some(name) = whole {
            if !out.windows.iter().any(|old| old.name == name && old.model.is_none()) {
                out.windows.push(Window {
                    name: name.into(),
                    used: used.clamp(0.0, 100.0),
                    resets_at: reset(w),
                    model: None,
                });
            }
            continue;
        }
        let model = &w["scope"]["model"];
        let display = text(field(model, "display_name", "displayName")).unwrap_or_default();
        if w["kind"] == "weekly_scoped" && matches!(display.to_ascii_lowercase().as_str(), "fable" | "fable 5") {
            continue;
        } else {
            let id = text(&model["id"]).or_else(|| text(&model["slug"]));
            let scope = id.clone().map(Scope::Model).unwrap_or_else(|| Scope::Feature(display.clone()));
            let mut l = limit(
                &format!("scoped:{}:{}", w["kind"].as_str().unwrap_or("unknown"), id.as_deref().unwrap_or(&display)),
                &display,
                scope,
                w,
            );
            l.used_percent = Some(used.clamp(0.0, 100.0));
            l.enabled = w["is_active"].as_bool();
            out.limits.push(l);
        }
    }
    let extra = field(v, "extra_usage", "extraUsage");
    if extra.is_object() {
        let raw = serde_json::json!({"limit": field(extra, "monthly_limit", "monthlyLimit"), "used": field(extra, "used_credits", "usedCredits"), "used_percent": extra["utilization"], "unit": "USD cents"});
        let mut l = limit("extra_usage", "Monthly extra usage", Scope::Overage, &raw);
        l.enabled = field(extra, "is_enabled", "isEnabled").as_bool();
        l.resets_at = reset(extra);
        out.limits.push(l);
    }
    out
}

pub(super) fn antigravity(v: &Value) -> Reading {
    let mut out = Reading::default();
    for (i, group) in v["groups"].as_array().into_iter().flatten().enumerate() {
        let name =
            text(field(group, "display_name", "displayName")).unwrap_or_else(|| format!("Quota group {}", i + 1));
        let models: Vec<String> =
            field(group, "model_ids", "modelIds").as_array().into_iter().flatten().filter_map(text).collect();
        // These are the provider's two documented group labels. Unknown groups
        // remain informative until the provider supplies explicit model IDs.
        let scope = if !models.is_empty() {
            Scope::Models(models)
        } else {
            match name.to_ascii_lowercase().as_str() {
                "gemini models" => Scope::ModelFamilies(vec!["gemini-".into()]),
                "claude and gpt models" => Scope::ModelFamilies(vec!["claude-".into(), "gpt-".into()]),
                _ => Scope::Feature(name.clone()),
            }
        };
        let group_id = text(&group["id"]).unwrap_or_else(|| name.clone());
        for (j, bucket) in group["buckets"].as_array().into_iter().flatten().enumerate() {
            let raw = field(bucket, "remaining_fraction", "remainingFraction");
            let remaining = number(raw).or_else(|| {
                raw.as_str()?
                    .trim()
                    .strip_suffix('%')?
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .map(|n| n / 100.0)
            });
            let Some(remaining) = remaining else { continue };
            let window = text(&bucket["window"])
                .or_else(|| text(field(bucket, "display_name", "displayName")))
                .unwrap_or_else(|| format!("Window {}", j + 1));
            let id = text(&bucket["id"]).unwrap_or_else(|| window.clone());
            let mut l =
                limit(&format!("antigravity:{group_id}:{id}"), &format!("{name} · {window}"), scope.clone(), bucket);
            l.used_percent = Some((100.0 - remaining * 100.0).clamp(0.0, 100.0));
            out.limits.push(l);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn business_spending_alone_routes_and_partial_headers_cannot_erase_it() {
        let mut st = crate::accounts::AccountState::default();
        usage(
            &mut st,
            Provider::Codex,
            &json!({"spend_control":{"individual_limit":{
                "limit":"10", "used":"10", "remaining":"0", "remaining_percent":"0", "unit":"credits", "reset_at":2_000_000_000
            }}}),
        );
        assert!(st.quota.exhausted("gpt-test"));
        assert!(st.quota.refreshed_at.is_some());
        assert_eq!(st.quota.limits[0].unit.as_deref(), Some("credits"));
        st.quota_cooldowns.insert("gpt-test".into(), Utc::now() + chrono::Duration::hours(1));
        authoritative(&mut st, vec![Window { name: "5h".into(), used: 0.0, resets_at: None, model: None }], None);
        assert!(st.quota.exhausted("gpt-test"));
        assert!(!st.quota_cooldowns.is_empty());
        usage(
            &mut st,
            Provider::Codex,
            &json!({"rate_limit":{"primary_window":{"limit_window_seconds":18000,"used_percent":0}},"spend_control":{"individual_limit":{"limit":10,"remaining_percent":70}}}),
        );
        assert!(!st.quota.exhausted("gpt-test"));
        assert!(st.quota_cooldowns.is_empty());
    }

    #[test]
    fn financial_numbers_preserve_zero_unknown_and_provider_units() {
        let read = codex(
            &json!({"spend_control":{"individual_limit":{"limit":"0","used":"0"}},"credits":{"balance":null,"unlimited":false}}),
        );
        assert_eq!(read.limits[0].used, Some(0.0));
        assert_eq!(read.limits[0].used_percent, Some(100.0));
        assert_eq!(read.limits[0].unit, None);
        assert_eq!(read.limits[1].remaining, None);
        assert_eq!(read.limits[1].unlimited, Some(false));
        let mut q = crate::accounts::AccountState::default();
        usage(&mut q, Provider::Codex, &json!({"credits":{"balance":0}}));
        assert!(!q.quota.exhausted("any"), "empty extra credits do not spend the base subscription");
        assert_eq!(q.quota.pressure("any"), None);
        let read = codex(
            &json!({"spend_control":{"individual_limit":{"limit":20,"remaining":5,"used_percent":"30","remaining_percent":"0"}}}),
        );
        assert_eq!(read.limits[0].used_percent, Some(100.0), "explicit zero takes precedence");
    }

    #[test]
    fn feature_and_model_limits_remain_independent_and_months_are_not_weeks() {
        let mut st = crate::accounts::AccountState::default();
        usage(
            &mut st,
            Provider::Codex,
            &json!({"rateLimit":{"primaryWindow":{"limitWindowSeconds":2_592_000,"usedPercent":"20","resetAfterSeconds":3600}},
            "codeReviewRateLimit":{"allowed":false},"additionalRateLimits":[{"meteredFeature":"fast","modelId":"gpt-fast","rateLimit":{"primaryWindow":{"usedPercent":100,"limitWindowSeconds":18000}}}]}),
        );
        assert_eq!(st.quota.windows[0].name, "30d");
        assert!(st.quota.windows[0].resets_at.is_some());
        assert!(!st.quota.exhausted("gpt-normal"));
        assert!(st.quota.exhausted("gpt-fast"));
        usage(&mut st, Provider::Codex, &json!({"rate_limit":{"allowed":false}}));
        assert!(st.quota.exhausted("gpt-normal"));
        usage(&mut st, Provider::Codex, &json!({"rate_limit":{"allowed":true}}));
        assert!(!st.quota.exhausted("gpt-normal"));
    }

    #[test]
    fn claude_current_model_limit_wins_and_paid_overages_do_not_block_chat() {
        let mut st = crate::accounts::AccountState::default();
        usage(
            &mut st,
            Provider::Claude,
            &json!({"iguana_necktie":{"utilization":100},
            "limits":[{"kind":"weekly_scoped","scope":{"model":{"display_name":"Fable 5"}},"percent":20,"is_active":true}],
            "seven_day_cowork":{"utilization":100},"extra_usage":{"is_enabled":true,"monthly_limit":1000,"used_credits":1000}}),
        );
        assert_eq!(st.quota.pressure("claude-fable-5"), Some(20.0));
        assert!(!st.quota.exhausted("claude-sonnet"));
        assert!(!st.quota.exhausted("claude-fable-5"));
        assert_eq!(st.quota.limits[1].unit.as_deref(), Some("USD cents"));
    }

    #[test]
    fn independent_group_windows_do_not_hide_weekly_exhaustion() {
        let mut st = crate::accounts::AccountState::default();
        usage(
            &mut st,
            Provider::Antigravity,
            &json!({"groups":[{"id":"g","displayName":"Coding","modelIds":["model-a","model-b"],"buckets":[
                {"window":"5h","remainingFraction":1,"resetTime":"2099-01-01T00:00:00Z"},
                {"window":"weekly","remainingFraction":0,"resetTime":"2099-01-03T00:00:00Z"}
            ]}]}),
        );
        assert!(st.quota.exhausted("model-a"));
        assert!(st.quota.exhausted("model-b"));
        assert!(!st.quota.exhausted("model-c"));
        assert_eq!(st.quota.exhausted_until("model-a").unwrap().to_rfc3339(), "2099-01-03T00:00:00+00:00");
        usage(
            &mut st,
            Provider::Antigravity,
            &json!({"groups":[{"displayName":"Gemini models","buckets":[{"window":"weekly","remainingFraction":"0%"}]}]}),
        );
        assert!(st.quota.exhausted("gemini-pro"));
        assert!(!st.quota.exhausted("claude-sonnet"));
    }

    #[test]
    fn active_typed_scopes_win_and_explicitly_inactive_scopes_stop_blocking() {
        let mut st = crate::accounts::AccountState::default();
        usage(
            &mut st,
            Provider::Claude,
            &json!({"limits":[
                {"kind":"weekly_scoped","scope":{"model":{"display_name":"Fable"}},"percent":100,"is_active":true},
                {"kind":"weekly_scoped","scope":{"model":{"display_name":"Fable"}},"percent":0},
                {"kind":"weekly_scoped","scope":{"model":{"id":"claude-custom"}},"percent":100,"is_active":true}
            ]}),
        );
        assert!(st.quota.exhausted("claude-fable-5"));
        assert!(st.quota.exhausted("claude-custom"));
        usage(
            &mut st,
            Provider::Claude,
            &json!({"limits":[
                {"kind":"weekly_scoped","scope":{"model":{"display_name":"Fable"}},"percent":100,"is_active":false},
                {"kind":"weekly_scoped","scope":{"model":{"id":"claude-custom"}},"percent":100,"is_active":false}
            ]}),
        );
        assert!(!st.quota.exhausted("claude-fable-5"));
        assert!(!st.quota.exhausted("claude-custom"));
        usage(
            &mut st,
            Provider::Codex,
            &json!({"additional_rate_limits":[{"metered_feature":"gpt-fast","rate_limit":{"primary_window":{"used_percent":100}}}]}),
        );
        assert!(st.quota.exhausted("gpt-fast"));
        assert!(!st.quota.exhausted("gpt-normal"));
    }
}
