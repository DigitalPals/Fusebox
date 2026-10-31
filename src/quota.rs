//! Subscription quota: how much of each usage window (Claude's 5-hour and
//! weekly limits, ChatGPT's Codex windows) an account has used. Read from
//! response headers on every request and from the providers' usage endpoints
//! in the background, so routing can prefer the account with most headroom.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;

use crate::accounts::{Account, Credential, Provider};
use crate::state::App;

mod readings;

pub use readings::{FiveHour, claude_five_hour};

const CLAUDE_USAGE: &str = "https://api.anthropic.com/api/oauth/usage";
const CODEX_USAGE: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_SUBSCRIPTION: &str = "https://chatgpt.com/backend-api/subscriptions";
/// Accounts without quota data count as half used.
pub const UNKNOWN: f64 = 50.0;
const POLL_EVERY: i64 = 5 * 60;
/// Response headers keep a busy account's shared windows current, so the usage endpoint
/// is only needed now and then, for the model-specific windows headers don't carry.
const BUSY_POLL_EVERY: i64 = 30 * 60;
/// Banked resets change rarely; opening the dashboard panel checks on demand.
const RESET_POLL_EVERY: i64 = 30 * 60;
/// Subscription plans change only with an upgrade or a downgrade.
const PLAN_EVERY: i64 = 6 * 3600;
const POLL_CONCURRENCY: usize = 4;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Window {
    /// "5h", "week", ...
    pub name: String,
    /// 0-100.
    pub used: f64,
    pub resets_at: Option<DateTime<Utc>>,
    /// Only counts for models whose name contains this ("opus", "sonnet").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// A limit that is not an ordinary subscription window. Feature allowances and
/// optional paid overages are informative; only account/model limits route chat.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Scope {
    Account,
    Model(String),
    Models(Vec<String>),
    ModelFamilies(Vec<String>),
    Feature(String),
    Overage,
    Credits,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Limit {
    pub id: String,
    pub name: String,
    pub scope: Scope,
    pub used_percent: Option<f64>,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub remaining: Option<f64>,
    /// Provider-reported units. Credits are never implicitly dollars.
    pub unit: Option<String>,
    pub unlimited: Option<bool>,
    pub enabled: Option<bool>,
    pub resets_at: Option<DateTime<Utc>>,
}

impl Limit {
    fn applies(&self, model: &str) -> bool {
        match &self.scope {
            Scope::Account => true,
            Scope::Model(m) => m.eq_ignore_ascii_case(model),
            Scope::Models(models) => models.iter().any(|m| m.eq_ignore_ascii_case(model)),
            Scope::ModelFamilies(prefixes) => prefixes.iter().any(|p| model.to_ascii_lowercase().starts_with(p)),
            _ => false,
        }
    }

    fn live(&self) -> bool {
        self.enabled != Some(false) && self.unlimited != Some(true) && self.resets_at.is_none_or(|r| r > Utc::now())
    }
}

#[derive(Default)]
struct Reading {
    windows: Vec<Window>,
    limits: Vec<Limit>,
    plan: Option<String>,
    /// The provider confirms the account has no 5-hour limit.
    no_five_hour: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Quota {
    pub windows: Vec<Window>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<Limit>,
    pub updated_at: Option<DateTime<Utc>>,
    /// Last accepted usage-endpoint refresh; partial observations do not advance this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// The plan's name as the provider shows it ("Ultra"); `plan` keeps the stable identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_name: Option<String>,
    /// The provider confirmed this account has no 5-hour limit (verified Claude Ultra).
    /// Any later reading that carries a 5-hour window clears it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub no_five_hour_limit: bool,
    /// The active ChatGPT subscription SKU; `plan` alone does not distinguish Pro tiers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_plan: Option<String>,
    /// Usage-endpoint checks in a row that brought no data, and when the last one ran.
    #[serde(skip)]
    pub(crate) check_failures: u32,
    #[serde(skip)]
    pub(crate) checked_at: Option<DateTime<Utc>>,
    /// Plan detail reads in a row that failed, and when the last one ran.
    #[serde(skip)]
    pub(crate) plan_failures: u32,
    #[serde(skip)]
    pub(crate) plan_checked_at: Option<DateTime<Utc>>,
}

/// Whether two plan identifiers name the same plan: Claude's Ultra may one day be
/// `ultra` rather than its `max_20x` tier, and that is no change of plan.
pub fn same_plan(a: &str, b: &str) -> bool {
    fn canonical(p: &str) -> &str {
        if p == "ultra" { "max_20x" } else { p }
    }
    canonical(a) == canonical(b)
}

#[derive(Debug, Clone, PartialEq)]
enum PlanDetails {
    /// The plan identifier and the provider's display name.
    Claude(Option<String>, Option<String>),
    Codex(Option<String>),
}

impl Quota {
    pub fn limit_remaining(&self, model: &str) -> Option<f64> {
        self.limit_pressure(model).map(|u| 100.0 - u.clamp(0.0, 100.0)).reduce(f64::min)
    }
    fn limit_pressure<'a>(&'a self, model: &'a str) -> impl Iterator<Item = f64> + 'a {
        self.limits.iter().filter(move |l| l.applies(model) && l.live()).filter_map(|l| l.used_percent)
    }

    fn live(&self, model: &str) -> impl Iterator<Item = &Window> {
        let now = Utc::now();
        let model = model.to_ascii_lowercase();
        self.windows
            .iter()
            .filter(move |w| w.model.as_ref().is_none_or(|m| model.contains(m.as_str())))
            .filter(move |w| w.resets_at.is_none_or(|r| r > now))
    }

    /// On Claude Ultra (formerly Max 20x) by its plan, or already confirmed to have no
    /// 5-hour limit: either way a bare null may mean there is no window, not an idle one.
    pub fn may_lack_five_hour(&self) -> bool {
        self.no_five_hour_limit
            || matches!(self.plan.as_deref(), Some("max_20x" | "ultra"))
            || self.plan_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("ultra"))
    }

    /// Use of the tightest window that applies to `model` (None = no data).
    pub fn pressure(&self, model: &str) -> Option<f64> {
        self.updated_at?;
        self.remaining(model).map(|remaining| 100.0 - remaining)
    }

    pub fn exhausted(&self, model: &str) -> bool {
        self.live(model).any(|w| w.used >= 100.0) || self.limit_pressure(model).any(|u| u >= 100.0)
    }

    /// A routing retry delay is not a provider-reported renewal time.
    pub fn reset_unknown(&self, model: &str) -> bool {
        self.live(model).any(|w| w.used >= 100.0 && w.resets_at.is_none())
            || self.limits.iter().any(|l| {
                l.applies(model) && l.live() && l.used_percent.is_some_and(|u| u >= 100.0) && l.resets_at.is_none()
            })
    }

    /// The general weekly renewal, ignoring expired, short and model-specific windows.
    pub fn weekly_reset(&self, model: &str) -> Option<DateTime<Utc>> {
        self.live(model).filter(|w| w.name == "week" && w.model.is_none()).filter_map(|w| w.resets_at).min()
    }

    /// Missing data is unknown; an elapsed window has its allowance available again.
    pub fn five_hour_remaining(&self, model: &str) -> Option<f64> {
        let lower = model.to_ascii_lowercase();
        if !self.windows.iter().any(|w| w.name == "5h" && w.model.as_ref().is_none_or(|m| lower.contains(m))) {
            return None;
        }
        let used = self.live(model).filter(|w| w.name == "5h").map(|w| w.used).fold(0.0, f64::max);
        Some(100.0 - used.clamp(0.0, 100.0))
    }

    /// Allowance left in the tightest weekly window that applies to `model` (None = no data).
    pub fn weekly_remaining(&self, model: &str) -> Option<f64> {
        self.live(model)
            .filter(|w| w.name.starts_with("week"))
            .map(|w| 100.0 - w.used.clamp(0.0, 100.0))
            .reduce(f64::min)
    }

    /// Headroom in the tightest known applicable window, for accounts without 5-hour data.
    pub fn remaining(&self, model: &str) -> Option<f64> {
        let lower = model.to_ascii_lowercase();
        if !self.windows.iter().any(|w| w.model.as_ref().is_none_or(|m| lower.contains(m)))
            && !self.limits.iter().any(|l| l.applies(model) && l.used_percent.is_some())
        {
            return None;
        }
        let used = self.live(model).map(|w| w.used).chain(self.limit_pressure(model)).fold(0.0, f64::max);
        Some(100.0 - used.clamp(0.0, 100.0))
    }

    /// `pauses`: when the account's quota pauses end.
    fn needs_refresh(&self, now: DateTime<Utc>, pauses: impl IntoIterator<Item = DateTime<Utc>>) -> bool {
        // A check that brought no data waits 2, 4, 8 ... up to 30 minutes before the next.
        if self.check_failures > 0
            && let Some(at) = self.checked_at
            && (now - at).num_seconds() < (60i64 << self.check_failures.min(5)).min(BUSY_POLL_EVERY)
        {
            return false;
        }
        let Some(refreshed) = self.refreshed_at else { return true };
        // A window renewed or a quota pause ended since the last reading: confirm it at
        // the next round rather than up to five minutes later.
        let ended = |t: DateTime<Utc>| t > refreshed && t <= now;
        if self
            .windows
            .iter()
            .filter_map(|w| w.resets_at)
            .chain(self.limits.iter().filter_map(|l| l.resets_at))
            .chain(pauses)
            .any(ended)
        {
            return true;
        }
        let busy = self.updated_at.is_some_and(|u| u > refreshed && (now - u).num_seconds() < POLL_EVERY);
        (now - refreshed).num_seconds() >= if busy { BUSY_POLL_EVERY } else { POLL_EVERY }
    }

    fn checked(&mut self, now: DateTime<Utc>, refreshed: bool) {
        self.checked_at = Some(now);
        self.check_failures = if refreshed { 0 } else { self.check_failures.saturating_add(1) };
    }

    /// The plan is read every 6 hours; after a failed read, again after 5, 10, 20 ...
    /// minutes, up to the same 6 hours.
    fn plan_due(&self, now: DateTime<Utc>) -> bool {
        let Some(at) = self.plan_checked_at else { return true };
        let wait = match self.plan_failures {
            0 => PLAN_EVERY,
            n => (300i64 << (n - 1).min(7)).min(PLAN_EVERY),
        };
        (now - at).num_seconds() >= wait
    }

    fn plan_read(&mut self, now: DateTime<Utc>, details: Option<PlanDetails>) {
        self.plan_checked_at = Some(now);
        match details {
            Some(details) => {
                match details {
                    PlanDetails::Claude(plan, name) => {
                        self.plan = plan;
                        self.plan_name = name;
                    }
                    // Usage remains the source of the broad Codex plan. The web
                    // subscription API can use different names for the same tier.
                    PlanDetails::Codex(plan) => self.subscription_plan = plan,
                }
                self.plan_failures = 0;
            }
            None => self.plan_failures = self.plan_failures.saturating_add(1),
        }
    }

    /// A window that is used up, and when it resets.
    pub fn exhausted_until(&self, model: &str) -> Option<DateTime<Utc>> {
        self.live(model)
            .filter(|w| w.used >= 100.0)
            .filter_map(|w| w.resets_at)
            .chain(
                self.limits
                    .iter()
                    .filter(|l| l.applies(model) && l.live() && l.used_percent.is_some_and(|u| u >= 100.0))
                    .filter_map(|l| l.resets_at),
            )
            .max()
    }

    pub(crate) fn set(&mut self, windows: Vec<Window>, plan: Option<String>) {
        if plan.is_some() {
            if self.plan.as_deref().zip(plan.as_deref()).is_some_and(|(old, new)| !same_plan(old, new)) {
                // A usage response can reveal a downgrade before the next plan check.
                self.subscription_plan = None;
                self.plan_name = None;
                self.plan_checked_at = None;
                self.plan_failures = 0;
            }
            self.plan = plan;
        }
        if windows.is_empty() {
            return;
        }
        if windows.iter().any(|w| w.name == "5h" && w.model.is_none()) {
            self.no_five_hour_limit = false;
        }
        // Observations can omit any window. Only replace the same window and model scope;
        // missing data must not erase exhaustion or renewal dates learned earlier.
        let retained: Vec<Window> = self
            .windows
            .iter()
            .filter(|w| !windows.iter().any(|n| n.name == w.name && n.model == w.model))
            .cloned()
            .collect();
        self.windows = windows;
        self.windows.extend(retained);
        self.updated_at = Some(Utc::now());
    }
}

fn label(secs: i64) -> String {
    match secs {
        604_800 => "week".into(),
        86_400 => "day".into(),
        s if s % 86_400 == 0 => format!("{}d", s / 86_400),
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

fn ts(secs: i64) -> Option<DateTime<Utc>> {
    (secs > 0).then(|| DateTime::from_timestamp(secs, 0)).flatten()
}

/// Reads the quota headers a Claude or ChatGPT response carries.
pub fn observe(acct: &Account, headers: &reqwest::header::HeaderMap, epoch: u64) {
    let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).map(str::trim).filter(|s| !s.is_empty());
    let num = |n: &str| h(n).and_then(|v| v.parse::<f64>().ok());
    let mut windows = Vec::new();
    let mut plan = None;
    match acct.provider {
        Provider::Claude => {
            for (key, name) in [("5h", "5h"), ("7d", "week")] {
                let Some(u) = num(&format!("anthropic-ratelimit-unified-{key}-utilization")) else { continue };
                let rejected = h(&format!("anthropic-ratelimit-unified-{key}-status")) == Some("rejected");
                let reset = num(&format!("anthropic-ratelimit-unified-{key}-reset")).and_then(|t| ts(t as i64));
                let used = if rejected { 100.0 } else { (u * 100.0).clamp(0.0, 100.0) };
                windows.push(Window { name: name.into(), used, resets_at: reset, model: None });
            }
        }
        Provider::Codex => {
            for which in ["primary", "secondary"] {
                let minutes = num(&format!("x-codex-{which}-window-minutes")).unwrap_or(0.0) as i64;
                let Some(used) = num(&format!("x-codex-{which}-used-percent")).filter(|_| minutes > 0) else {
                    continue;
                };
                let reset = num(&format!("x-codex-{which}-reset-at")).and_then(|t| ts(t as i64)).or_else(|| {
                    num(&format!("x-codex-{which}-reset-after-seconds"))
                        .map(|s| Utc::now() + chrono::Duration::seconds(s as i64))
                });
                windows.push(Window {
                    name: label(minutes * 60),
                    used: used.clamp(0.0, 100.0),
                    resets_at: reset,
                    model: None,
                });
            }
            plan = h("x-codex-plan-type").map(String::from);
            if let Some(etag) = h("x-models-etag") {
                acct.state.lock().models_etag = Some(etag.to_string());
            }
        }
        _ => return,
    }
    let mut st = acct.state.lock();
    if st.quota_epoch == epoch && !st.quota_refreshing {
        st.set_quota(windows, plan);
    }
}

/// Codex websocket sessions report quota as a `codex.rate_limits` event.
pub fn observe_codex_event(acct: &Account, v: &Value, epoch: u64) {
    if v["type"] != "codex.rate_limits" {
        return;
    }
    let rl = if v["rate_limits"].is_object() { &v["rate_limits"] } else { v };
    let windows = ["primary", "secondary"]
        .iter()
        .filter_map(|which| {
            let w = &rl[*which];
            let minutes = w["window_minutes"].as_i64().filter(|m| *m > 0)?;
            let reset = w["reset_at"]
                .as_i64()
                .and_then(ts)
                .or_else(|| w["reset_after_seconds"].as_i64().map(|s| Utc::now() + chrono::Duration::seconds(s)));
            Some(Window {
                name: label(minutes * 60),
                used: w["used_percent"].as_f64()?.clamp(0.0, 100.0),
                resets_at: reset,
                model: None,
            })
        })
        .collect();
    let mut st = acct.state.lock();
    if st.quota_epoch == epoch && !st.quota_refreshing {
        st.set_quota(windows, v["plan_type"].as_str().map(String::from));
    }
}

#[cfg(test)]
fn claude_windows(v: &Value) -> Vec<Window> {
    readings::claude(v).windows
}

#[cfg(test)]
fn codex_windows(v: &Value) -> Vec<Window> {
    readings::codex(v).windows
}

/// Asks the provider's usage endpoint (free, no tokens) for current quota.
pub async fn poll(app: &App, acct: &Arc<Account>) -> anyhow::Result<()> {
    let epoch = acct.quota_epoch();
    let (token, account_id) = match &*acct.cred.read() {
        Credential::OAuth(o) if o.base_url.is_none() => (o.access_token.clone(), o.account_id.clone()),
        _ => return Ok(()),
    };
    let http = app.http.control_for_account(acct);
    let reading = match acct.provider {
        Provider::Claude => {
            let v: Value = http
                .get(CLAUDE_USAGE)
                .bearer_auth(&token)
                .header("anthropic-beta", "oauth-2025-04-20")
                .header("user-agent", crate::clients::claude_user_agent())
                .timeout(Duration::from_secs(15))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            readings::claude(&v)
        }
        Provider::Codex => {
            let mut rb = http
                .get(CODEX_USAGE)
                .bearer_auth(&token)
                .header("user-agent", crate::clients::codex_user_agent())
                .header("originator", crate::upstream::CODEX_ORIGINATOR)
                .timeout(Duration::from_secs(15));
            if let Some(id) = &account_id {
                rb = rb.header("chatgpt-account-id", id);
            }
            let v: Value = rb.send().await?.error_for_status()?.json().await?;
            readings::codex(&v)
        }
        Provider::Antigravity => {
            let Some(project) = crate::antigravity::project(acct) else { return Ok(()) };
            let v: Value = http
                .post(format!("{}/v1internal:retrieveUserQuotaSummary", crate::antigravity::request_base(acct)))
                .bearer_auth(&token)
                .json(&serde_json::json!({ "project": project }))
                .header("user-agent", crate::antigravity::user_agent())
                .timeout(Duration::from_secs(15))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            readings::antigravity(&v)
        }
        _ => return Ok(()),
    };
    let mut st = acct.state.lock();
    if st.quota_epoch == epoch && !st.quota_refreshing {
        st.quota_epoch += 1;
        apply_reading(&mut st, reading);
    }
    Ok(())
}

/// The subscription endpoint is scoped to the signed-in ChatGPT account, not its
/// default workspace. Only the active entitlement identifies the current paid tier;
/// past subscriptions and scheduled changes must not label the current account.
fn codex_plan(value: &Value) -> anyhow::Result<PlanDetails> {
    let plan = value["plan_type"].as_str().map(str::trim).filter(|s| !s.is_empty());
    if plan.is_none() {
        anyhow::bail!("ChatGPT subscription response is missing plan_type")
    }
    let entitlement = &value["entitlement"];
    let subscription_plan = (entitlement["has_active_subscription"] == true)
        .then(|| entitlement["subscription_plan"].as_str())
        .flatten()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    Ok(PlanDetails::Codex(subscription_plan))
}

/// Reads Claude plan names and ChatGPT subscription SKUs without using model tokens.
pub async fn poll_plan(app: &App, acct: &Arc<Account>) -> anyhow::Result<()> {
    if !matches!(acct.provider, Provider::Claude | Provider::Codex) {
        return Ok(());
    }
    let epoch = acct.quota_epoch();
    let (token, account_id) = match &*acct.cred.read() {
        Credential::OAuth(o) if o.base_url.is_none() => (o.access_token.clone(), o.account_id.clone()),
        _ => return Ok(()),
    };
    let http = app.http.control_for_account(acct);
    let details = match acct.provider {
        Provider::Claude => crate::oauth::claude_profile(&http, &token).await.map(|profile| {
            PlanDetails::Claude(crate::oauth::claude_plan(&profile), crate::oauth::claude_plan_name(&profile))
        }),
        Provider::Codex => {
            async {
                let Some(id) = account_id.as_deref().filter(|id| !id.is_empty()) else {
                    anyhow::bail!("ChatGPT subscription check requires an account ID")
                };
                let value: Value = http
                    .get(CODEX_SUBSCRIPTION)
                    .query(&[("account_id", id)])
                    .bearer_auth(&token)
                    .header("chatgpt-account-id", id)
                    .header("user-agent", crate::clients::codex_user_agent())
                    .header("originator", crate::upstream::CODEX_ORIGINATOR)
                    .timeout(Duration::from_secs(15))
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                codex_plan(&value)
            }
            .await
        }
        _ => unreachable!(),
    };
    let mut st = acct.state.lock();
    // A different account signed in to this file while the plan was read.
    if st.quota_epoch == epoch {
        let before = (st.quota.plan.clone(), st.quota.subscription_plan.clone());
        st.quota.plan_read(Utc::now(), details.as_ref().ok().cloned());
        if before.0.as_ref().zip(st.quota.plan.as_ref()).is_some_and(|(old, new)| !same_plan(old, new))
            || before.1.as_ref().zip(st.quota.subscription_plan.as_ref()).is_some_and(|(old, new)| old != new)
        {
            st.attribution.history.clear();
            st.attribution.dirty = true;
        }
    }
    details.map(drop)
}

/// Provider usage is authoritative, but absent windows cannot prove a cooldown has ended.
#[cfg(test)]
pub fn authoritative(st: &mut crate::accounts::AccountState, windows: Vec<Window>, plan: Option<String>) {
    apply_reading(st, Reading { windows, plan, ..Default::default() });
}

fn apply_reading(st: &mut crate::accounts::AccountState, reading: Reading) {
    let Reading { windows, limits, plan, no_five_hour } = reading;
    // Only a provider-confirmed absence retires a retained 5-hour window; an omitted one stays.
    let retired = |w: &Window| no_five_hour && w.name == "5h" && w.model.is_none();
    if windows.is_empty() && limits.is_empty() {
        st.quota.set(windows, plan);
        return;
    }
    let covered: Vec<String> = st
        .quota_cooldowns
        .keys()
        .filter(|model| {
            let model = if model.as_str() == "*" { "" } else { model.as_str() };
            st.quota
                .windows
                .iter()
                .filter(|old| old.model.as_ref().is_none_or(|m| model.contains(m)))
                .all(|old| retired(old) || windows.iter().any(|new| new.name == old.name && new.model == old.model))
                && st
                    .quota
                    .limits
                    .iter()
                    .filter(|old| old.applies(model))
                    .all(|old| limits.iter().any(|new| new.id == old.id && new.scope == old.scope))
        })
        .cloned()
        .collect();
    st.set_quota(windows, plan);
    if no_five_hour {
        st.quota.windows.retain(|w| !retired(w));
        st.quota.no_five_hour_limit = true;
        st.attribution.retire("5h");
    }
    // An omitted scope cannot erase a spending cap learned on an earlier read.
    for limit in limits {
        st.quota.limits.retain(|old| old.id != limit.id || old.scope != limit.scope);
        st.quota.limits.push(limit);
    }
    st.quota.updated_at = Some(Utc::now());
    st.quota.refreshed_at = st.quota.updated_at;
    let quota = &st.quota;
    st.quota_cooldowns.retain(|model, _| {
        !covered.contains(model) || quota.pressure(if model == "*" { "" } else { model }).is_none_or(|u| u >= 100.0)
    });
    st.settle_quota_error(Utc::now());
}

pub fn usage(st: &mut crate::accounts::AccountState, provider: Provider, value: &Value) {
    let reading = match provider {
        Provider::Claude => readings::claude(value),
        Provider::Codex => readings::codex(value),
        Provider::Antigravity => readings::antigravity(value),
        _ => return,
    };
    apply_reading(st, reading);
}

/// Keeps quota and model lists fresh for signed-in Claude and ChatGPT accounts.
pub async fn poller(app: Arc<App>) {
    tokio::time::sleep(Duration::from_secs(2)).await;
    loop {
        let checks = app.pool.all().into_iter().map(|acct| poll_account(&app, acct));
        let quota = poll_batch(checks).await;
        let lists = crate::discovery::refresh_due(&app).await;
        crate::discovery::prune(&app);
        if quota || lists {
            app.broadcast("accounts", Value::Null);
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

async fn poll_batch<F: std::future::Future<Output = bool>>(checks: impl IntoIterator<Item = F>) -> bool {
    futures::stream::iter(checks)
        .buffer_unordered(POLL_CONCURRENCY)
        .fold(false, |changed, refreshed| async move { changed || refreshed })
        .await
}

async fn poll_account(app: &Arc<App>, acct: Arc<Account>) -> bool {
    let mut changed = false;
    // Learn an Antigravity account's real model list without waiting for a request.
    if acct.provider == Provider::Antigravity
        && acct.is_oauth()
        && acct.discovered.read().is_empty()
        && !acct.state.lock().disabled
    {
        match crate::oauth::ensure_ready(app, &acct).await {
            Ok(()) => changed |= !acct.discovered.read().is_empty(),
            Err(e) => tracing::debug!(account = %acct.label, "antigravity setup failed: {e:#}"),
        }
    }
    if !matches!(acct.provider, Provider::Claude | Provider::Codex | Provider::Antigravity) || !acct.is_oauth() {
        return changed;
    }
    let reset_stale = acct.provider != Provider::Antigravity && app.cfg().banked_resets && {
        let st = acct.state.lock();
        !st.disabled
            && st.banked_resets.as_ref().is_none_or(|v| {
                (Utc::now() - v.checked_at).num_seconds() >= RESET_POLL_EVERY
                    || crate::banked_resets::expiry_passed(v, Utc::now())
            })
    };
    if reset_stale {
        let _ = crate::banked_resets::refresh(app, &acct).await;
        changed = true;
    }
    changed |= crate::banked_resets::automatic_check(app, &acct).await;
    let plan_due = acct.provider != Provider::Antigravity
        && matches!(&*acct.cred.read(), Credential::OAuth(o) if o.base_url.is_none()
        && (acct.provider != Provider::Codex || o.account_id.as_ref().is_some_and(|id| !id.is_empty())))
        && {
            let st = acct.state.lock();
            !st.disabled && st.quota.plan_due(Utc::now())
        };
    if plan_due && crate::oauth::ensure_fresh(app, &acct, chrono::Duration::minutes(5), false).await.is_ok() {
        let before = {
            let st = acct.state.lock();
            (st.quota.plan.clone(), st.quota.subscription_plan.clone())
        };
        if let Err(e) = poll_plan(app, &acct).await {
            tracing::debug!(account = %acct.label, "plan check failed: {e:#}");
        }
        let st = acct.state.lock();
        changed |= (st.quota.plan.clone(), st.quota.subscription_plan.clone()) != before;
    }
    let stale = {
        let st = acct.state.lock();
        !st.disabled && st.quota.needs_refresh(Utc::now(), st.quota_cooldowns.values().copied())
    };
    if !stale || crate::oauth::ensure_fresh(app, &acct, chrono::Duration::minutes(5), false).await.is_err() {
        return changed;
    }
    let before = acct.state.lock().quota.refreshed_at;
    let result = poll(app, &acct).await;
    {
        let mut st = acct.state.lock();
        let refreshed = st.quota.refreshed_at.is_some() && st.quota.refreshed_at != before;
        st.quota.checked(Utc::now(), refreshed);
    }
    match result {
        Ok(()) => changed = true,
        Err(e) => tracing::debug!(account = %acct.label, "quota check failed: {e:#}"),
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn quota_refreshes_are_bounded_and_a_stalled_account_does_not_stop_others() {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let release_first = Arc::new(tokio::sync::Notify::new());
        let (entered, mut started) = tokio::sync::mpsc::unbounded_channel();
        let (completed, mut finished) = tokio::sync::mpsc::unbounded_channel();
        let count = POLL_CONCURRENCY + 5;
        let checks = (0..count)
            .map(|index| {
                let gate = gate.clone();
                let release_first = release_first.clone();
                let entered = entered.clone();
                let completed = completed.clone();
                async move {
                    entered.send(index).unwrap();
                    let _permit = gate.acquire().await.unwrap();
                    // Keep the first account stalled while other accounts reuse slots.
                    if index == 0 {
                        release_first.notified().await;
                    }
                    completed.send(index).unwrap();
                    index == count - 1
                }
            })
            .collect::<Vec<_>>();
        let batch = tokio::spawn(poll_batch(checks));
        for _ in 0..POLL_CONCURRENCY {
            tokio::time::timeout(Duration::from_secs(2), started.recv()).await.unwrap().unwrap();
        }
        assert!(started.try_recv().is_err(), "polling should admit at most four concurrent accounts");
        // Account 0 holds one permit; another lets all other accounts complete.
        gate.add_permits(2);
        for _ in 1..count {
            let index = tokio::time::timeout(Duration::from_secs(2), finished.recv()).await.unwrap().unwrap();
            assert_ne!(index, 0);
        }
        assert!(!batch.is_finished());
        release_first.notify_one();
        assert!(tokio::time::timeout(Duration::from_secs(2), batch).await.unwrap().unwrap());
    }

    #[test]
    fn partial_headers_preserve_known_five_hour_exhaustion() {
        let reset = Utc::now() + chrono::Duration::hours(4);
        let mut q = Quota::default();
        q.set(vec![Window { name: "5h".into(), used: 100.0, resets_at: Some(reset), model: None }], None);
        q.set(
            vec![Window {
                name: "week".into(),
                used: 20.0,
                resets_at: Some(reset + chrono::Duration::days(3)),
                model: None,
            }],
            None,
        );
        assert!(q.exhausted("gpt-6.1-sol"));
        assert_eq!(q.five_hour_remaining("gpt-6.1-sol"), Some(0.0));
    }

    #[test]
    fn partial_headers_preserve_weekly_priority() {
        let reset = Utc::now() + chrono::Duration::days(3);
        let mut q = Quota::default();
        q.set(vec![Window { name: "week".into(), used: 33.0, resets_at: Some(reset), model: None }], None);
        q.set(
            vec![Window {
                name: "5h".into(),
                used: 0.0,
                resets_at: Some(reset - chrono::Duration::days(2)),
                model: None,
            }],
            None,
        );
        assert_eq!(q.weekly_reset("claude-sonnet-5-5"), Some(reset));
    }

    #[test]
    fn partial_observations_only_slow_the_usage_refresh() {
        let cfg = crate::config::Config {
            auth_dir: "/nonexistent".into(),
            codex_api_key: vec![crate::config::KeyEntry { api_key: "a".into(), ..Default::default() }],
            ..Default::default()
        };
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        let acct = &pool.all()[0];
        let headers = reqwest::header::HeaderMap::from_iter([
            (reqwest::header::HeaderName::from_static("x-codex-primary-window-minutes"), "10080".parse().unwrap()),
            (reqwest::header::HeaderName::from_static("x-codex-primary-used-percent"), "20".parse().unwrap()),
        ]);
        observe(acct, &headers, acct.quota_epoch());
        assert!(acct.state.lock().quota.needs_refresh(Utc::now(), [])); // headers cannot replace the first poll
        let usage_value = json!({"rate_limit":{"primary_window":{"limit_window_seconds":604800,"used_percent":20}}});
        let refreshed;
        {
            let mut st = acct.state.lock();
            usage(&mut st, Provider::Codex, &usage_value);
            refreshed = st.quota.refreshed_at.unwrap();
            assert!(!st.quota.needs_refresh(refreshed + chrono::Duration::seconds(POLL_EVERY - 1), []));
            assert!(st.quota.needs_refresh(refreshed + chrono::Duration::seconds(POLL_EVERY), []));
        }
        observe(acct, &headers, acct.quota_epoch());
        let event = json!({"type":"codex.rate_limits","primary":{"window_minutes":300,"used_percent":95,"reset_after_seconds":3600}});
        observe_codex_event(acct, &event, acct.quota_epoch());
        {
            let st = acct.state.lock();
            assert_eq!(st.quota.refreshed_at, Some(refreshed));
            // A busy account still refreshes its model-specific windows, just less often.
            assert!(!st.quota.needs_refresh(refreshed + chrono::Duration::seconds(POLL_EVERY), []));
            assert!(st.quota.needs_refresh(refreshed + chrono::Duration::seconds(BUSY_POLL_EVERY), []));
            assert_eq!(st.quota.windows.len(), 2); // WebSocket update also retains the weekly window
            assert_eq!(st.quota.five_hour_remaining("gpt-6.1-sol"), Some(5.0));
        }
        let mut st = acct.state.lock();
        usage(&mut st, Provider::Codex, &json!({}));
        assert_eq!(st.quota.refreshed_at, Some(refreshed)); // empty data does not claim a refresh
        st.quota.refreshed_at = Some(refreshed - chrono::Duration::seconds(POLL_EVERY));
        usage(&mut st, Provider::Codex, &usage_value);
        assert!(!st.quota.needs_refresh(Utc::now(), []));
        assert_eq!(st.quota.five_hour_remaining("gpt-6.1-sol"), Some(5.0));
    }

    #[test]
    fn a_usage_reading_with_headroom_ends_a_quota_error() {
        let cfg = crate::config::Config {
            auth_dir: "/nonexistent".into(),
            claude_api_key: vec![crate::config::KeyEntry { api_key: "a".into(), ..Default::default() }],
            ..Default::default()
        };
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        let acct = &pool.all()[0];
        let model = "claude-sonnet-5-5";
        acct.exhaust(model, Utc::now() + chrono::Duration::hours(2), "subscription quota exhausted: x");
        let reading = |used: f64| json!({ "five_hour": { "utilization": used, "resets_at": "2099-01-01T00:00:00Z" } });
        let mut st = acct.state.lock();
        usage(&mut st, Provider::Claude, &reading(100.0));
        assert_eq!(st.last_error(Utc::now()), Some("subscription quota exhausted: x"), "still spent");
        usage(&mut st, Provider::Claude, &reading(0.0));
        assert!(st.quota_cooldowns.is_empty());
        assert_eq!(st.last_error(Utc::now()), None, "the allowance is back before the planned reset");
        // Other errors are not the usage reading's to clear.
        st.set_error("500: upstream exploded");
        usage(&mut st, Provider::Claude, &reading(0.0));
        assert_eq!(st.last_error(Utc::now()), Some("500: upstream exploded"));
    }

    /// Usage as a verified Ultra account reported it on 2026-10-31 (trimmed).
    fn ultra_usage() -> Value {
        json!({
            "five_hour": null,
            "seven_day": { "utilization": 58.0, "resets_at": "2099-11-02T13:00:00.176119+00:00" },
            "iguana_necktie": null,
            "limits": [
                { "kind": "weekly_all", "group": "weekly", "percent": 58, "resets_at": "2099-11-02T13:00:00.176119+00:00",
                  "scope": null, "is_active": true },
                { "kind": "weekly_scoped", "group": "weekly", "percent": 0, "resets_at": "2099-11-02T13:00:00+00:00",
                  "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null }, "is_active": false },
            ],
        })
    }

    #[test]
    fn only_a_typed_list_without_a_session_proves_there_is_no_five_hour_limit() {
        assert!(readings::claude_no_five_hour(&ultra_usage()));
        let reading = readings::claude(&ultra_usage());
        assert!(reading.no_five_hour);
        assert!(reading.windows.iter().all(|w| w.name != "5h"));
        assert!(reading.limits.iter().all(|l| !l.name.is_empty()), "account-wide entries make no nameless limits");
        // An idle window on a plan that has one, as Max reported it the same night.
        let mut idle = ultra_usage();
        idle["limits"].as_array_mut().unwrap().insert(
            0,
            json!({ "kind": "session", "group": "session", "percent": 0, "resets_at": null, "is_active": true }),
        );
        assert!(!readings::claude_no_five_hour(&idle));
        let reading = readings::claude(&idle);
        assert!(!reading.no_five_hour);
        let five = reading.windows.iter().find(|w| w.name == "5h").expect("the session entry stands in");
        assert_eq!((five.used, five.resets_at), (0.0, None));
        assert_eq!(reading.windows.iter().filter(|w| w.name == "week").count(), 1, "seven_day wins over weekly_all");
        // Without a typed list, a null is only an idle or unknown window.
        for v in [
            json!({ "five_hour": null, "seven_day": { "utilization": 1.0 } }),
            json!({ "five_hour": null, "limits": [] }),
            json!({ "limits": [{ "kind": "weekly_all", "percent": 1 }] }),
            json!({ "five_hour": { "utilization": 3.0 }, "limits": [{ "kind": "weekly_all", "percent": 1 }] }),
            // A malformed weekly entry proves nothing either, nor does a list with an entry
            // it can't classify, or one that names a session in any way.
            json!({ "five_hour": null, "limits": [{ "kind": "weekly_all" }] }),
            json!({ "five_hour": null, "limits": [{ "kind": "weekly_all", "percent": 140 }] }),
            json!({ "five_hour": null, "limits": [{ "kind": "weekly_all", "percent": 1 }, { "kind": null, "group": "session" }] }),
            json!({ "five_hour": null, "limits": [{ "kind": "weekly_all", "percent": 1 }, { "kind": " " }] }),
            json!({ "five_hour": null, "limits": [{ "kind": "weekly_all", "percent": 1 }, 5] }),
            json!({ "five_hour": null, "limits": [{ "kind": "weekly_all", "percent": 1 }, { "kind": "x", "group": "session" }] }),
        ] {
            assert!(!readings::claude_no_five_hour(&v), "{v}");
        }
    }

    #[test]
    fn a_malformed_session_entry_is_no_proof_of_absence() {
        let mut st = crate::accounts::AccountState::default();
        st.quota.plan = Some("max_20x".into());
        let reset = Utc::now() + chrono::Duration::hours(2);
        let week = json!({ "utilization": 20, "resets_at": Utc::now() + chrono::Duration::days(2) });
        usage(
            &mut st,
            Provider::Claude,
            &json!({ "five_hour": { "utilization": 100, "resets_at": reset }, "seven_day": week }),
        );
        usage(
            &mut st,
            Provider::Claude,
            &json!({ "five_hour": null, "seven_day": week, "limits": [
                { "kind": "weekly_all", "group": "weekly", "percent": 20, "resets_at": Utc::now() + chrono::Duration::days(2), "is_active": true },
                { "kind": null, "group": "session", "percent": 100, "resets_at": reset, "is_active": true },
            ] }),
        );
        assert!(!st.quota.no_five_hour_limit);
        assert!(st.quota.exhausted("claude-sonnet-5-5"));
    }

    #[test]
    fn ultra_and_max_20x_are_the_same_plan() {
        assert!(same_plan("ultra", "max_20x") && same_plan("max_20x", "ultra") && same_plan("pro", "pro"));
        assert!(!same_plan("max_5x", "max_20x") && !same_plan("max", "ultra"));
        let mut st = crate::accounts::AccountState::default();
        let week = |used| Window {
            name: "week".into(),
            used,
            resets_at: Some(Utc::now() + chrono::Duration::days(2)),
            model: None,
        };
        st.set_quota(vec![week(10.0)], Some("max_20x".into()));
        st.set_quota(vec![week(11.0)], Some("max_20x".into()));
        st.quota.plan_name = Some("Ultra".into());
        assert!(!st.attribution.history.is_empty());
        st.set_quota(vec![week(12.0)], Some("ultra".into()));
        assert!(!st.attribution.history.is_empty(), "a new identifier for the same plan keeps the history");
        assert_eq!(st.quota.plan_name.as_deref(), Some("Ultra"));
        st.set_quota(vec![week(13.0)], Some("max_5x".into()));
        assert!(st.attribution.history.len() <= 1, "a real change of plan starts over");
        assert_eq!(st.quota.plan_name, None);
    }

    #[test]
    fn malformed_typed_limits_never_retire_a_spent_five_hour_window() {
        let mut st = crate::accounts::AccountState::default();
        let reset = Utc::now() + chrono::Duration::hours(2);
        let week = json!({ "utilization": 20, "resets_at": Utc::now() + chrono::Duration::days(2) });
        usage(
            &mut st,
            Provider::Claude,
            &json!({ "five_hour": { "utilization": 100, "resets_at": reset }, "seven_day": week }),
        );
        usage(
            &mut st,
            Provider::Claude,
            &json!({ "five_hour": null, "seven_day": week, "limits": [{ "kind": "weekly_all" }] }),
        );
        assert!(!st.quota.no_five_hour_limit);
        assert!(st.quota.exhausted("claude-sonnet-5-5"));
    }

    #[test]
    fn a_confirmed_missing_five_hour_limit_retires_the_window_and_its_cooldown() {
        let cfg = crate::config::Config {
            auth_dir: "/nonexistent".into(),
            claude_api_key: vec![crate::config::KeyEntry { api_key: "a".into(), ..Default::default() }],
            ..Default::default()
        };
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        let acct = &pool.all()[0];
        let model = "claude-sonnet-5-5";
        let renews = Utc::now() + chrono::Duration::hours(2);
        acct.exhaust(model, renews, "subscription quota exhausted: x");
        let mut st = acct.state.lock();
        usage(
            &mut st,
            Provider::Claude,
            &json!({
                "five_hour": { "utilization": 100.0, "resets_at": renews },
                "seven_day": { "utilization": 50.0, "resets_at": "2099-11-02T13:00:00Z" },
            }),
        );
        assert!(st.quota.exhausted(model) && !st.quota_cooldowns.is_empty());
        // The 5-hour limit goes away: a weekly reading alone keeps it, the Ultra one retires it.
        usage(&mut st, Provider::Claude, &json!({ "seven_day": { "utilization": 51.0 } }));
        assert!(st.quota.exhausted(model), "an omitted window stays");
        usage(&mut st, Provider::Claude, &ultra_usage());
        assert!(st.quota.no_five_hour_limit);
        assert!(st.quota.windows.iter().all(|w| w.name != "5h"));
        assert_eq!(st.quota.five_hour_remaining(model), None);
        assert!(!st.quota.exhausted(model));
        assert!(st.quota_cooldowns.is_empty());
        assert_eq!(st.last_error(Utc::now()), None);
        assert!(st.attribution.current.iter().all(|s| s.name != "5h"));
        assert!(st.attribution.previous.iter().any(|s| s.name == "5h"), "its last instance is kept as finished");
        let shown = serde_json::to_value(&st.quota).unwrap();
        assert_eq!(shown["no_five_hour_limit"], true);
        // A spent week still blocks.
        let mut spent = ultra_usage();
        spent["seven_day"]["utilization"] = json!(100.0);
        usage(&mut st, Provider::Claude, &spent);
        assert!(st.quota.exhausted(model));
        // A limit that comes back, from usage or from headers, is followed again.
        drop(st);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("anthropic-ratelimit-unified-5h-utilization", "0.25".parse().unwrap());
        observe(acct, &headers, acct.quota_epoch());
        let st = acct.state.lock();
        assert!(!st.quota.no_five_hour_limit);
        assert_eq!(st.quota.five_hour_remaining(model), Some(75.0));
    }

    #[test]
    fn a_renewed_window_or_an_ended_pause_is_read_at_the_next_round() {
        let now = Utc::now();
        let secs = chrono::Duration::seconds;
        let renews = now + secs(120);
        let mut q = Quota::default();
        q.set(vec![Window { name: "5h".into(), used: 100.0, resets_at: Some(renews), model: None }], None);
        q.refreshed_at = Some(now);
        assert!(!q.needs_refresh(renews - secs(1), []));
        assert!(q.needs_refresh(renews, []), "the window renewed after the last reading");
        q.refreshed_at = Some(renews + secs(1));
        assert!(!q.needs_refresh(renews + secs(60), []), "read once after the renewal");
        // A quota pause that ended since the last reading counts the same.
        let ended = renews + secs(30);
        assert!(!q.needs_refresh(ended - secs(1), [ended]));
        assert!(q.needs_refresh(ended, [ended]));
        // A check that brought no data still backs off.
        q.checked(ended, false);
        assert!(!q.needs_refresh(ended + secs(60), [ended]));
    }

    #[test]
    fn usage_checks_that_bring_no_data_back_off() {
        let now = Utc::now();
        let mut q = Quota::default();
        assert!(q.needs_refresh(now, []));
        q.checked(now, false);
        assert!(!q.needs_refresh(now + chrono::Duration::seconds(119), []));
        assert!(q.needs_refresh(now + chrono::Duration::seconds(120), []));
        for _ in 0..10 {
            q.checked(now, false);
        }
        assert!(!q.needs_refresh(now + chrono::Duration::seconds(BUSY_POLL_EVERY - 1), []));
        assert!(q.needs_refresh(now + chrono::Duration::seconds(BUSY_POLL_EVERY), []));
        // A successful check ends the backoff; a banked reset can still force the next one.
        q.refreshed_at = Some(now);
        q.checked(now, true);
        assert!(q.needs_refresh(now + chrono::Duration::seconds(POLL_EVERY), []));
        q.refreshed_at = None;
        assert!(q.needs_refresh(now, []));
    }

    #[test]
    fn the_plan_is_read_every_six_hours_and_failed_reads_back_off() {
        let now = Utc::now();
        let secs = chrono::Duration::seconds;
        let mut q = Quota::default();
        assert!(q.plan_due(now));
        q.plan_read(now, None);
        assert!(!q.plan_due(now + secs(299)));
        assert!(q.plan_due(now + secs(300)));
        q.plan_read(now, None);
        assert!(!q.plan_due(now + secs(599)));
        assert!(q.plan_due(now + secs(600)));
        for _ in 0..20 {
            q.plan_read(now, None);
        }
        assert!(!q.plan_due(now + secs(PLAN_EVERY - 1)));
        assert!(q.plan_due(now + secs(PLAN_EVERY)));
        // A good read ends the backoff, and its plan replaces the last one, even with none.
        q.plan_read(now, Some(PlanDetails::Claude(Some("max_20x".into()), Some("Ultra".into()))));
        assert_eq!((q.plan.as_deref(), q.plan_name.as_deref(), q.plan_failures), (Some("max_20x"), Some("Ultra"), 0));
        assert!(!q.plan_due(now + secs(PLAN_EVERY - 1)));
        q.plan_read(now, None);
        assert_eq!(q.plan.as_deref(), Some("max_20x"), "a failed read keeps the plan");
        q.plan_read(now, Some(PlanDetails::Claude(None, None)));
        assert_eq!((q.plan.as_deref(), q.plan_name.as_deref()), (None, None));
        // Usage readings without a plan (Claude's) leave it alone.
        q.plan = Some("max_5x".into());
        q.set(vec![Window { name: "week".into(), used: 10.0, resets_at: None, model: None }], None);
        assert_eq!(q.plan.as_deref(), Some("max_5x"));
        let shown = serde_json::to_value(&q).unwrap();
        assert_eq!(shown["plan"], "max_5x");
        assert!(shown.get("plan_checked_at").is_none() && shown.get("plan_failures").is_none());
        assert!(shown.get("plan_name").is_none() && shown.get("no_five_hour_limit").is_none());
        q.plan_name = Some("Max".into());
        q.set(vec![], Some("pro".into()));
        assert_eq!((q.plan.as_deref(), q.plan_name), (Some("pro"), None), "a changed plan drops the old name");
    }

    #[test]
    fn codex_plan_uses_the_active_subscription_not_past_or_scheduled_tiers() {
        for sku in ["chatgptprolite", "chatgptpro", "chatgptpromax", "future-plan"] {
            let mut value = json!({
                "plan_type": "pro",
                "entitlement": {
                    "has_active_subscription": true,
                    "subscription_plan": sku,
                    "scheduled_plan_change": { "subscription_plan": "chatgptpromax" }
                },
                "last_active_subscription": { "subscription_plan": "chatgptprolite" }
            });
            assert_eq!(codex_plan(&value).unwrap(), PlanDetails::Codex(Some(sku.into())));
            value["entitlement"]["has_active_subscription"] = false.into();
            assert_eq!(codex_plan(&value).unwrap(), PlanDetails::Codex(None));
        }
        for value in [
            json!({ "plan_type": "free" }),
            json!({ "plan_type": "pro", "entitlement": { "subscription_plan": "chatgptpro" } }),
            json!({ "plan_type": "pro", "entitlement": { "has_active_subscription": true, "subscription_plan": " " } }),
        ] {
            assert_eq!(codex_plan(&value).unwrap(), PlanDetails::Codex(None));
        }
        assert!(codex_plan(&json!({})).is_err());
        assert!(codex_plan(&json!({ "plan_type": " " })).is_err());
    }

    #[test]
    fn codex_subscription_details_survive_usage_and_failed_reads_but_clear_on_plan_changes() {
        let now = Utc::now();
        let mut q = Quota::default();
        let details = codex_plan(&json!({
            "plan_type": "pro",
            "entitlement": { "has_active_subscription": true, "subscription_plan": "chatgptpro" }
        }))
        .unwrap();
        q.plan_read(now, Some(details));
        let window = Window { name: "week".into(), used: 10.0, resets_at: None, model: None };
        q.set(vec![window.clone()], Some("pro".into()));
        q.set(vec![window.clone()], None);
        assert_eq!(q.subscription_plan.as_deref(), Some("chatgptpro"));
        assert!(!q.plan_due(now));
        q.plan_read(now, None);
        assert_eq!(q.subscription_plan.as_deref(), Some("chatgptpro"));
        assert_eq!(q.plan.as_deref(), Some("pro"));
        assert_eq!(serde_json::to_value(&q).unwrap()["subscription_plan"], "chatgptpro");
        // A downgrade observed through usage must stop displaying the paid tier at once.
        q.set(vec![], Some("plus".into()));
        assert_eq!(q.subscription_plan, None);
        assert!(q.plan_due(now));
        // A successful subscription read without an active entitlement also clears it.
        q.subscription_plan = Some("chatgptpro".into());
        q.plan_read(now, Some(codex_plan(&json!({ "plan_type": "free" })).unwrap()));
        assert_eq!(q.subscription_plan, None);
        assert_eq!(q.windows, vec![window], "plan reads do not change quota windows");
        assert!(serde_json::to_value(&q).unwrap().get("subscription_plan").is_none());
    }

    #[test]
    fn partial_updates_keep_model_scopes_and_expired_limits_stop_applying() {
        let now = Utc::now();
        let week =
            Window { name: "week".into(), used: 40.0, resets_at: Some(now + chrono::Duration::days(3)), model: None };
        let mut st = crate::accounts::AccountState::default();
        authoritative(
            &mut st,
            vec![
                week.clone(),
                Window { model: Some("opus".into()), used: 100.0, ..week.clone() },
                Window {
                    name: "5h".into(),
                    used: 100.0,
                    resets_at: Some(now - chrono::Duration::seconds(1)),
                    model: None,
                },
            ],
            None,
        );
        st.quota_cooldowns.insert("claude-opus-5-5".into(), now + chrono::Duration::hours(1));
        authoritative(&mut st, vec![Window { used: 20.0, ..week }], None);
        assert_eq!(st.quota.windows.len(), 3);
        assert!(st.quota_cooldowns.contains_key("claude-opus-5-5"));
        assert_eq!(st.quota.remaining("claude-opus-5-5"), Some(0.0));
        assert_eq!(st.quota.remaining("claude-sonnet-5-5"), Some(80.0));
        assert_eq!(st.quota.five_hour_remaining("claude-sonnet-5-5"), Some(100.0));
        assert!(!st.quota.exhausted("claude-sonnet-5-5"));
    }

    #[test]
    fn remaining_requires_applicable_data_and_recognizes_elapsed_windows() {
        let mut q = Quota::default();
        assert_eq!(q.remaining("claude-sonnet-5-5"), None);
        q.set(vec![Window { name: "week opus".into(), used: 80.0, resets_at: None, model: Some("opus".into()) }], None);
        assert_eq!(q.remaining("claude-sonnet-5-5"), None);
        assert_eq!(q.remaining("CLAUDE-OPUS-5-5"), Some(20.0));
        q.windows[0].resets_at = Some(Utc::now() - chrono::Duration::seconds(1));
        assert_eq!(q.remaining("claude-opus-5-5"), Some(100.0));
        assert_eq!(q.five_hour_remaining("claude-opus-5-5"), None);
    }

    #[test]
    fn weekly_reset_ignores_other_windows_and_expired_dates() {
        let now = Utc::now();
        let weekly = now + chrono::Duration::days(3);
        let mut q = Quota::default();
        assert_eq!(q.weekly_reset("claude-opus-5-5"), None);
        q.set(
            vec![
                Window { name: "5h".into(), used: 5.0, resets_at: Some(now + chrono::Duration::hours(1)), model: None },
                Window {
                    name: "week opus".into(),
                    used: 5.0,
                    resets_at: Some(now + chrono::Duration::days(1)),
                    model: Some("opus".into()),
                },
                Window { name: "week".into(), used: 33.0, resets_at: Some(weekly), model: None },
            ],
            None,
        );
        assert_eq!(q.weekly_reset("claude-opus-5-5"), Some(weekly));
        q.windows[2].resets_at = Some(now - chrono::Duration::seconds(1));
        assert_eq!(q.weekly_reset("claude-opus-5-5"), None);
        q.windows[2].resets_at = None;
        assert_eq!(q.weekly_reset("claude-opus-5-5"), None);
    }

    #[test]
    fn usage_endpoints_parse() {
        let claude = json!({
            "five_hour": { "utilization": 81.0, "resets_at": "2099-10-02T12:09:59.520850+00:00" },
            "seven_day": { "utilization": 56.0, "resets_at": "2099-10-05T18:59:59+00:00" },
            "seven_day_opus": null,
        });
        let w = claude_windows(&claude);
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].name.as_str(), w[0].used), ("5h", 81.0));
        assert_eq!(w[1].name, "week");

        let codex = json!({ "rate_limit": { "limit_reached": false,
            "primary_window": { "used_percent": 31, "limit_window_seconds": 604800, "reset_at": 4102444800i64 },
            "secondary_window": null } });
        let w = codex_windows(&codex);
        assert_eq!((w[0].name.as_str(), w[0].used), ("week", 31.0));
    }

    #[test]
    fn pressure_uses_the_tightest_live_window() {
        let future = Utc::now() + chrono::Duration::hours(1);
        let past = Utc::now() - chrono::Duration::hours(1);
        let mut q = Quota::default();
        assert_eq!(q.pressure("claude-opus-5-5"), None);
        q.set(
            vec![
                Window { name: "5h".into(), used: 81.0, resets_at: Some(future), model: None },
                Window { name: "week".into(), used: 56.0, resets_at: Some(future), model: None },
                Window { name: "old".into(), used: 99.0, resets_at: Some(past), model: None },
                Window { name: "week opus".into(), used: 100.0, resets_at: Some(future), model: Some("opus".into()) },
            ],
            None,
        );
        assert_eq!(q.pressure("claude-sonnet-5-5"), Some(81.0));
        assert_eq!(q.pressure("claude-opus-5-5"), Some(100.0));
        assert_eq!(q.exhausted_until("claude-opus-5-5"), Some(future));
        assert_eq!(q.exhausted_until("claude-sonnet-5-5"), None);
    }
}
