//! Each provider's subscriptions taken together. Smart-quota routing runs down the account
//! that renews first on purpose, so one account running out early says little: what
//! matters is whether all of them last until renewals bring capacity back. The overview's
//! Combined weekly limits draw these pools and the pace faults come from them, so the
//! meter and the warning work from the same numbers.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use crate::quota::Window;

/// What the pools need to know about one account.
#[derive(Debug, Clone, Copy)]
pub struct Member<'a> {
    pub id: &'a str,
    /// Provider group: the provider id, or the compatibility group's name.
    pub provider: &'a str,
    pub plan: Option<&'a str>,
    /// A Claude or Codex sign-in: a subscription that reports limits, even before it has.
    pub subscription: bool,
    pub disabled: bool,
    pub windows: &'a [Window],
    pub history: &'a [crate::outlook::History],
    pub forecast_available: bool,
    /// The provider confirmed the account has no 5-hour limit.
    pub no_five_hour: bool,
}

impl Member<'_> {
    /// As `metered()` in ui/app.js: a subscription, or anything reporting a whole-account window.
    fn metered(&self) -> bool {
        self.subscription || self.windows.iter().any(|w| w.model.is_none())
    }
}

/// Each plan's weekly allowance, relative to the provider's smallest paid plan. Claude
/// publishes Pro 1x, Max 5x and Ultra 10x a week (since 30 October 2026; the plans were
/// Max 5x and Max 20x, and their tier identifiers still are). The same weights are the
/// monthly prices. Every window is weighed the same way, so the meter and the warnings agree.
pub fn plan_weight(provider: &str, plan: Option<&str>) -> Option<f64> {
    match (provider, plan?) {
        ("claude", "pro") => Some(1.0),
        ("claude", "max_5x") => Some(5.0),
        ("claude", "max_20x" | "ultra") => Some(10.0),
        _ => None,
    }
}

/// The enabled subscriptions of each provider that has two or more.
pub fn pooled<'a>(members: &[Member<'a>]) -> BTreeMap<&'a str, Vec<Member<'a>>> {
    let mut groups: BTreeMap<&str, Vec<Member>> = BTreeMap::new();
    for m in members.iter().filter(|m| !m.disabled && m.metered()) {
        groups.entry(m.provider).or_default().push(*m);
    }
    groups.retain(|_, list| list.len() >= 2);
    groups
}

/// One window of one provider, all its subscriptions together.
#[derive(Debug, Clone, PartialEq)]
pub struct Pool {
    pub provider: String,
    /// "5h", "week", ... as in the quota windows.
    pub window: String,
    /// The models a window is limited to ("opus").
    pub model: Option<String>,
    /// Every member, reporting this window or not.
    pub accounts: Vec<String>,
    /// The members reporting the window, each with what it counts for.
    pub weights: Vec<(String, f64)>,
    /// Weighted use, 0 to 100.
    pub used: f64,
    /// Where use would be at even pace, 0 to 1: the weighted share of each member's own
    /// window gone. None while a window in use doesn't say when it resets.
    pub pace: Option<f64>,
    /// Each member counts by its plan's price; with any plan unknown, they count equally.
    pub priced: bool,
    /// The renewal that brings capacity back, when the pool runs out before it at this pace.
    pub runs_out: Option<DateTime<Utc>>,
    pub outlook: Option<crate::outlook::Outlook>,
}

impl Pool {
    /// The overview's form.
    pub fn json(&self) -> Value {
        json!({
            "provider": self.provider,
            "window": self.window,
            "model": self.model,
            "accounts": self.accounts.len(),
            "account_ids": self.accounts,
            "reported": self.weights.len(),
            "used": self.used,
            "pace": self.pace,
            "priced": self.priced,
            "runs_out_early": self.runs_out.is_some(),
            "until": self.runs_out.map(|t| t.to_rfc3339()),
            "outlook": self.outlook,
        })
    }
}

/// Every pool: for each provider with two or more subscriptions, one per window they report.
pub fn pools(members: &[Member], now: DateTime<Utc>) -> Vec<Pool> {
    let mut out = Vec::new();
    for (provider, list) in pooled(members) {
        let windows: BTreeSet<(&str, Option<&str>)> =
            list.iter().flat_map(|m| m.windows.iter().map(|w| (w.name.as_str(), w.model.as_deref()))).collect();
        out.extend(windows.into_iter().filter_map(|(name, model)| pool(provider, name, model, &list, now)));
    }
    out
}

/// One member in the run-out: what it counts for, what it has left and when it renews.
struct Stock {
    weight: f64,
    left: f64,
    renews: Option<DateTime<Utc>>,
}

/// The members' windows, as the dashboard's `weeklyPools()` once worked them out: each
/// member's pace comes from its own reset, and a window that has renewed, or not started,
/// counts as unused and at its start. Members that don't report the window are left out.
fn pool(provider: &str, name: &str, model: Option<&str>, members: &[Member], now: DateTime<Utc>) -> Option<Pool> {
    let reports: Vec<(&Member, &Window)> = members
        .iter()
        .filter_map(|m| {
            m.windows
                .iter()
                .find(|w| w.name == name && w.model.as_deref() == model && w.used.is_finite())
                .map(|w| (m, w))
        })
        .collect();
    // A member without a model's own limit takes that model's requests until its
    // whole-account limits stop it, and those have pools of their own. One without a
    // 5-hour limit (Claude Ultra) takes what the others can't, so together they have none.
    if reports.is_empty()
        || (model.is_some() && reports.len() < members.len())
        || (name == "5h" && model.is_none() && members.iter().any(|m| m.no_five_hour))
    {
        return None;
    }
    let priced = reports.iter().all(|(m, _)| plan_weight(m.provider, m.plan).is_some());
    let length = crate::attribution::length(name);
    let (mut total, mut used, mut elapsed, mut paced) = (0.0, 0.0, 0.0, true);
    let mut weights = Vec::new();
    let mut stock = Vec::new();
    for (m, w) in &reports {
        let k = if priced { plan_weight(m.provider, m.plan).unwrap_or(1.0) } else { 1.0 };
        weights.push((m.id.to_string(), k));
        total += k;
        if w.resets_at.is_some_and(|r| r <= now) {
            stock.push(Stock { weight: k, left: 100.0 * k, renews: None });
            continue;
        }
        let u = w.used.clamp(0.0, 100.0);
        used += k * u;
        match length.and_then(|l| gone(w, l, now)).or((u == 0.0).then_some(0.0)) {
            Some(t) => elapsed += k * t,
            None => paced = false,
        }
        stock.push(Stock { weight: k, left: k * (100.0 - u), renews: w.resets_at });
    }
    let used = used / total;
    let pace = paced.then(|| elapsed / total);
    // As one account's pace warning: at least a quarter of the window gone and 20% used.
    // With one member reporting, this is that rule.
    let outlook = (name == "week" && model.is_none()).then(|| crate::outlook::Outlook::calculate(members, priced, now));
    let runs_out = if let Some(outlook) = &outlook {
        outlook.exhausts_at.and(outlook.recovers_at)
    } else {
        match (pace, length) {
            (Some(p), Some(l)) if p >= 0.25 && (20.0..100.0).contains(&used) => {
                run_out(stock, total * used / (p * l.num_milliseconds() as f64), now)
            }
            _ => None,
        }
    };
    Some(Pool {
        provider: provider.to_string(),
        window: name.to_string(),
        model: model.map(String::from),
        accounts: members.iter().map(|m| m.id.to_string()).collect(),
        weights,
        used,
        pace,
        priced,
        runs_out,
        outlook,
    })
}

/// How far through its window a member is, 0 to 1, when the window says when it resets.
fn gone(w: &Window, length: Duration, now: DateTime<Utc>) -> Option<f64> {
    let left = (w.resets_at? - now).num_milliseconds() as f64;
    let length = length.num_milliseconds() as f64;
    (0.0..=length).contains(&left).then(|| 1.0 - left / length)
}

/// The renewal before which use at `rate` (weighted points a millisecond) empties the
/// pool, if one does. Use goes to the member that renews first, as smart-quota routing
/// drains it, and whatever a member has left when it renews is lost. Each renews once:
/// after the last renewal every window is new, and the pace so far says nothing of it.
fn run_out(mut stock: Vec<Stock>, rate: f64, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    stock.sort_by_key(|s| (s.renews.is_none(), s.renews));
    let mut at = now;
    for i in 0..stock.len() {
        let Some(renews) = stock[i].renews else { break };
        let mut need = rate * (renews - at).num_milliseconds() as f64;
        // Those still to renew, in order, then those that have.
        for j in (i..stock.len()).chain(0..i) {
            let take = need.min(stock[j].left);
            stock[j].left -= take;
            need -= take;
        }
        if need > 1e-9 {
            return Some(renews);
        }
        stock[i].left = 100.0 * stock[i].weight;
        at = renews;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    fn now() -> DateTime<Utc> {
        "2026-10-17T20:00:00Z".parse().unwrap()
    }

    fn week(used: f64, left: Option<f64>) -> Window {
        Window {
            name: "week".into(),
            used,
            resets_at: left.map(|d| now() + Duration::seconds((d * DAY as f64) as i64)),
            model: None,
        }
    }

    struct Account {
        id: &'static str,
        provider: &'static str,
        plan: Option<&'static str>,
        subscription: bool,
        disabled: bool,
        windows: Vec<Window>,
        no_five_hour: bool,
    }

    /// A Claude subscription reporting these windows.
    fn sub(id: &'static str, windows: Vec<Window>) -> Account {
        Account {
            id,
            provider: "claude",
            plan: None,
            subscription: true,
            disabled: false,
            windows,
            no_five_hour: false,
        }
    }

    fn on(plan: &'static str, id: &'static str, windows: Vec<Window>) -> Account {
        Account { plan: Some(plan), ..sub(id, windows) }
    }

    fn members(list: &[Account]) -> Vec<Member<'_>> {
        list.iter()
            .map(|a| Member {
                id: a.id,
                provider: a.provider,
                plan: a.plan,
                subscription: a.subscription,
                disabled: a.disabled,
                windows: &a.windows,
                history: &[],
                forecast_available: true,
                no_five_hour: a.no_five_hour,
            })
            .collect()
    }

    fn weekly(list: &[Account]) -> Vec<Pool> {
        pools(&members(list), now()).into_iter().filter(|p| p.window == "week" && p.model.is_none()).collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn each_accounts_pace_comes_from_its_own_reset_and_use_and_pace_are_averaged() {
        // Weeks half, a quarter and three quarters gone: even pace 50% between them.
        let list = [
            sub("a", vec![week(42.0, Some(3.5))]),
            sub("b", vec![week(4.0, Some(5.25))]),
            sub("c", vec![week(20.0, Some(1.75))]),
        ];
        let [p] = &weekly(&list)[..] else { panic!("one pool") };
        assert_eq!((p.accounts.len(), p.weights.len()), (3, 3));
        assert!(close(p.used, 22.0) && close(p.pace.unwrap(), 0.5));
        assert_eq!(p.runs_out, None);
        let api = p.json();
        assert_eq!(
            (api["provider"].as_str(), api["window"].as_str(), api["accounts"].as_u64(), api["reported"].as_u64()),
            (Some("claude"), Some("week"), Some(3), Some(3))
        );
        assert_eq!(api["account_ids"], json!(["a", "b", "c"]));
        assert!(api["model"].is_null() && api["until"].is_null());
        assert_eq!(api["runs_out_early"], false);
    }

    #[test]
    fn renewed_idle_and_unreported_windows_disabled_accounts_and_single_providers() {
        let list = [
            sub("a", vec![week(80.0, Some(-1.0 / 24.0))]), // reset an hour ago, not refreshed yet: a fresh week
            sub("b", vec![week(0.0, None)]),               // not started: nothing used, at its start
            sub("c", vec![]),                              // unreported
            Account { disabled: true, ..sub("d", vec![week(100.0, Some(1.0))]) },
            sub("e", vec![week(60.0, Some(3.5))]),
            Account { provider: "codex", ..sub("x", vec![week(36.0, Some(1.0))]) },
        ];
        let all = pools(&members(&list), now());
        assert_eq!(all.len(), 1, "one Codex account has no pool");
        let p = &all[0];
        assert_eq!(p.accounts, ["a", "b", "c", "e"], "the disabled account is left out");
        assert_eq!(p.weights.len(), 3);
        assert!(close(p.used, 20.0) && close(p.pace.unwrap(), 0.5 / 3.0));

        let one = [sub("a", vec![week(10.0, Some(1.0))]), Account { disabled: true, ..sub("d", vec![]) }];
        assert!(pools(&members(&one), now()).is_empty(), "one enabled account left");
        assert!(pools(&members(&[sub("a", vec![]), sub("b", vec![])]), now()).is_empty(), "nothing reported yet");
        // An API key without limits is no subscription; a key reporting a window is.
        let key = Account { subscription: false, ..sub("k", vec![]) };
        assert!(pooled(&members(&[sub("a", vec![]), key])).is_empty());
        let metered = Account { subscription: false, ..sub("k", vec![week(5.0, Some(2.0))]) };
        assert_eq!(pooled(&members(&[sub("a", vec![]), metered]))["claude"].len(), 2);

        // A window in use with no reset time has no pace, so neither has the pool.
        let unpaced = [sub("a", vec![week(30.0, None)]), sub("b", vec![week(10.0, Some(1.0))])];
        let [p] = &weekly(&unpaced)[..] else { panic!() };
        assert!(close(p.used, 20.0));
        assert_eq!((p.pace, p.runs_out), (None, None));
        assert!(p.json()["pace"].is_null());
    }

    #[test]
    fn each_account_counts_by_its_plans_weekly_allowance_when_every_plan_is_known() {
        // Ultra half through its week at 30%, Max seven eighths through at 60%: by weekly
        // allowance (10 and 5) the pool is at 40% with even pace at 62.5%.
        let list = [on("max_20x", "a", vec![week(30.0, Some(3.5))]), on("max_5x", "b", vec![week(60.0, Some(0.875))])];
        let [p] = &weekly(&list)[..] else { panic!() };
        assert!(p.priced);
        assert!(close(p.used, 40.0) && close(p.pace.unwrap(), 0.625));
        assert_eq!(p.weights, [("a".to_string(), 10.0), ("b".to_string(), 5.0)]);
        // Pro against Ultra is 1 to 10, under its old identifier or a new one.
        for ultra in ["max_20x", "ultra"] {
            let pro = [on(ultra, "a", vec![week(0.0, Some(7.0))]), on("pro", "b", vec![week(88.0, Some(3.5))])];
            assert!(close(weekly(&pro)[0].used, 8.0));
        }

        // An unknown plan, Max without its multiple, or any ChatGPT plan: they count equally.
        let unknown = [
            on("max_20x", "a", vec![week(30.0, Some(3.5))]),
            on("max_20x", "b", vec![week(50.0, Some(3.5))]),
            sub("c", vec![week(10.0, Some(3.5))]),
        ];
        let p = &weekly(&unknown)[0];
        assert!(!p.priced && close(p.used, 30.0));
        let max = [on("max", "a", vec![week(30.0, Some(3.5))]), on("max_20x", "b", vec![week(50.0, Some(3.5))])];
        assert!(close(weekly(&max)[0].used, 40.0));
        let codex = |id, plan, used| Account { provider: "codex", ..on(plan, id, vec![week(used, Some(1.0))]) };
        let p = &weekly(&[codex("x", "pro", 10.0), codex("y", "plus", 30.0)])[0];
        assert!(!p.priced && close(p.used, 20.0));
        // Only members reporting the window are weighed: an unreporting one's plan doesn't matter.
        let reporting = [
            on("max_20x", "a", vec![week(30.0, Some(3.5))]),
            on("pro", "b", vec![week(90.0, Some(3.5))]),
            sub("c", vec![]),
        ];
        assert!(weekly(&reporting)[0].priced);
    }

    #[test]
    fn weekly_forecast_and_warning_share_recent_consumption_and_missing_data_is_unknown() {
        let list = [sub("a", vec![week(90.0, Some(3.5))]), sub("b", vec![week(60.0, Some(4.2))])];
        let mut members = members(&list);
        assert_eq!(pools(&members, now())[0].outlook.as_ref().unwrap().state, "learning");
        assert!(pools(&members, now())[0].runs_out.is_none());
        let histories: Vec<_> =
            list.iter().map(|a| vec![crate::outlook::test_history(&a.windows[0], now(), 20.0)]).collect();
        for (member, history) in members.iter_mut().zip(&histories) {
            member.history = history;
        }
        let p = &pools(&members, now())[0];
        let outlook = p.outlook.as_ref().unwrap();
        assert_eq!(outlook.state, "run_out");
        assert_eq!(outlook.exhausts_at, Some(now() + Duration::hours(30)));
        assert_eq!(p.runs_out, outlook.recovers_at);
        assert_eq!(p.json()["outlook"]["state"], "run_out");
        members[1].windows = &[];
        let p = &pools(&members, now())[0];
        assert_eq!(p.outlook.as_ref().unwrap().reason, Some("missing_quota"));
        assert!(p.runs_out.is_none());
    }

    #[test]
    fn model_windows_pool_only_when_every_member_reports_them() {
        let opus = |used, left: f64| Window { model: Some("opus".into()), ..week(used, Some(left)) };
        let five = |used, hours: f64| Window {
            name: "5h".into(),
            used,
            resets_at: Some(now() + Duration::minutes((hours * 60.0) as i64)),
            model: None,
        };
        let all = [
            sub("a", vec![week(10.0, Some(5.0)), opus(80.0, 3.5), five(70.0, 1.0)]),
            sub("b", vec![week(10.0, Some(5.0)), opus(70.0, 3.0)]),
        ];
        let found: Vec<(String, Option<String>)> =
            pools(&members(&all), now()).into_iter().map(|p| (p.window, p.model)).collect();
        assert_eq!(
            found,
            [("5h".to_string(), None), ("week".to_string(), None), ("week".to_string(), Some("opus".to_string()))]
        );
        let some = [sub("a", vec![week(10.0, Some(5.0)), opus(80.0, 3.5)]), sub("b", vec![week(10.0, Some(5.0))])];
        assert!(
            pools(&members(&some), now()).iter().all(|p| p.model.is_none()),
            "b takes Opus without a limit of its own"
        );
        // A verified Ultra account has no 5-hour limit and takes what a Max account can't.
        let mixed = [
            on("max_5x", "a", vec![week(10.0, Some(5.0)), five(90.0, 1.0)]),
            Account { no_five_hour: true, ..on("max_20x", "b", vec![week(10.0, Some(5.0))]) },
        ];
        let found: Vec<String> = pools(&members(&mixed), now()).into_iter().map(|p| p.window).collect();
        assert_eq!(found, ["week"]);
    }
}
