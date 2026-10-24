//! Deliberate, bounded requests that start idle subscription windows. Provider
//! usage is authoritative; a local clock never creates a subscription allowance.
mod journal;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, ensure};
use axum::http::HeaderMap;
use chrono::{DateTime, Datelike, NaiveTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::{Account, Credential, Provider};
use crate::state::App;
use journal::{Attempt, Journal, Outcome};

const CHECK_SECONDS: i64 = 300;
const UNCERTAIN_SECONDS: i64 = 5 * 3600 + 600;
const MAX_BODY: usize = 1 << 20;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    #[default]
    Off,
    Always,
    WorkingHours,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    pub mode: Mode,
    /// Explicit account IDs, as shown by /api/accounts. Empty selects nobody.
    pub accounts: Vec<String>,
    pub model: String,
    pub timezone: String,
    /// ISO weekdays: Monday=1, Sunday=7. Overnight hours belong to their start day.
    pub weekdays: Vec<u32>,
    pub start: String,
    pub end: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Off,
            accounts: vec![],
            model: "claude-haiku-4-5-20251001".into(),
            timezone: "UTC".into(),
            weekdays: vec![1, 2, 3, 4, 5],
            start: "08:00".into(),
            end: "18:00".into(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.accounts.len() <= 1000, "Select at most 1000 window starter accounts");
        let mut ids = std::collections::HashSet::new();
        for id in &self.accounts {
            ensure!(
                !id.is_empty() && id.len() <= 200 && !id.chars().any(char::is_control) && ids.insert(id),
                "Window starter account IDs must be nonempty, unique and at most 200 characters"
            );
        }
        ensure!(
            self.model.starts_with("claude-")
                && self.model.len() <= 120
                && self.model.bytes().all(|c| c.is_ascii_alphanumeric() || b"-._".contains(&c)),
            "Window starter model must be a native Claude model name, without a group, prefix or options"
        );
        self.timezone.parse::<chrono_tz::Tz>().context("Choose a valid IANA window starter timezone")?;
        ensure!(
            !self.weekdays.is_empty()
                && self.weekdays.len() <= 7
                && self.weekdays.iter().all(|d| (1..=7).contains(d))
                && self.weekdays.iter().collect::<std::collections::HashSet<_>>().len() == self.weekdays.len(),
            "Choose unique window starter weekdays from 1 (Monday) to 7 (Sunday)"
        );
        let start = clock(&self.start).context("Window starter start must be HH:MM")?;
        let end = clock(&self.end).context("Window starter end must be HH:MM")?;
        ensure!(start != end, "Window starter working hours must have different start and end times");
        Ok(())
    }

    pub fn selected(&self, acct: &Account) -> bool {
        self.mode != Mode::Off && self.accounts.contains(&acct.id) && native_id(acct).is_ok()
    }

    pub fn in_hours(&self, now: DateTime<Utc>) -> bool {
        match self.mode {
            Mode::Off => false,
            Mode::Always => true,
            Mode::WorkingHours => {
                let (Ok(tz), Some(start), Some(end)) =
                    (self.timezone.parse::<chrono_tz::Tz>(), clock(&self.start), clock(&self.end))
                else {
                    return false;
                };
                let local = now.with_timezone(&tz);
                let time = local.hour() * 60 + local.minute();
                let day = local.weekday().number_from_monday();
                if start < end {
                    self.weekdays.contains(&day) && (start..end).contains(&time)
                } else if time >= start {
                    self.weekdays.contains(&day)
                } else {
                    time < end && self.weekdays.contains(&(if day == 1 { 7 } else { day - 1 }))
                }
            }
        }
    }
}

fn clock(s: &str) -> Option<u32> {
    if s.len() != 5 || s.as_bytes()[2] != b':' {
        return None;
    }
    let time = NaiveTime::parse_from_str(s, "%H:%M").ok()?;
    Some(time.hour() * 60 + time.minute())
}

#[derive(Clone, Debug, Serialize)]
pub struct View {
    pub status: String,
    pub message: String,
    pub checked_at: DateTime<Utc>,
    pub attempted_at: Option<DateTime<Utc>>,
    pub resets_at: Option<DateTime<Utc>>,
    pub next_check_at: DateTime<Utc>,
    pub retry_after: Option<DateTime<Utc>>,
    pub automatic: bool,
}

impl View {
    fn new(status: &str, message: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self {
            status: status.into(),
            message: message.into(),
            checked_at: now,
            attempted_at: None,
            resets_at: None,
            next_check_at: now + chrono::Duration::seconds(CHECK_SECONDS),
            retry_after: None,
            automatic: false,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub request_id: String,
    #[serde(default)]
    pub check_only: bool,
}

pub fn supported(acct: &Account) -> bool {
    native_id(acct).is_ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Window {
    Unknown,
    Inactive,
    Active(DateTime<Utc>),
}

/// Only explicit provider data proves inactivity. Missing, malformed, and a
/// zero-percent window with a future reset are never treated as unused windows.
fn window(value: &Value, now: DateTime<Utc>) -> Window {
    let Some(w) = value.get("five_hour") else { return Window::Unknown };
    if w.is_null() {
        return Window::Inactive;
    }
    let Some(used) = w["utilization"].as_f64().filter(|u| u.is_finite() && (0.0..=100.0).contains(u)) else {
        return Window::Unknown;
    };
    match w.get("resets_at") {
        Some(Value::Null) if used == 0.0 => Window::Inactive,
        Some(Value::String(s)) => match DateTime::parse_from_rfc3339(s) {
            Ok(at) if at > now => Window::Active(at.with_timezone(&Utc)),
            // An old reset alone cannot prove a fresh window has not begun.
            _ => Window::Unknown,
        },
        _ => Window::Unknown,
    }
}

fn weekly_available(value: &Value, now: DateTime<Utc>) -> bool {
    let Some(week) = value.get("seven_day") else { return false };
    // A never-used subscription can also have an explicitly inactive week.
    if !week.is_null()
        && !(week["utilization"].as_f64().is_some_and(|u| u.is_finite() && (0.0..100.0).contains(&u))
            && (week["resets_at"].as_str().is_some_and(|s| DateTime::parse_from_rfc3339(s).is_ok_and(|at| at > now))
                || week.get("resets_at") == Some(&Value::Null) && week["utilization"].as_f64() == Some(0.0)))
    {
        return false;
    }
    true
}

fn native_id(acct: &Account) -> Result<String> {
    ensure!(acct.provider == Provider::Claude, "Window starters currently support Claude subscriptions only");
    match &*acct.cred.read() {
        Credential::OAuth(o) if o.base_url.is_none() => o
            .account_id
            .as_deref()
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
            .map(|id| id.to_string())
            .ok_or_else(|| anyhow!("A verified Claude account ID is required; sign in again")),
        _ => Err(anyhow!("Window starters require a native Claude subscription OAuth account")),
    }
}

fn peers(app: &App, id: &str) -> Vec<Arc<Account>> {
    app.pool.all().into_iter().filter(|a| native_id(a).ok().as_deref() == Some(id)).collect()
}

fn store(app: &App, id: &str, view: &View) {
    for acct in peers(app, id) {
        acct.state.lock().window_starter = Some(view.clone());
    }
    app.broadcast("accounts", Value::Null);
}

fn current(app: &App, acct: &Arc<Account>, id: &str, root: &std::path::Path, automatic: bool) -> Result<()> {
    ensure!(!app.shutdown.is_cancelled(), "Fusebox is shutting down");
    ensure!(
        app.pool.get(&acct.id).is_some_and(|a| Arc::ptr_eq(&a, acct)) && native_id(acct)? == id,
        "Account changed; refresh before starting a window"
    );
    let cfg = app.cfg();
    ensure!(cfg.auth_dir() == root, "Credentials directory changed; refresh before starting a window");
    ensure!(
        !automatic || cfg.window_starter.selected(acct) && cfg.window_starter.in_hours(Utc::now()),
        "Automatic window starting is no longer enabled for this account and time"
    );
    ensure!(!acct.state.lock().disabled, "Enable this account before starting its window");
    Ok(())
}

fn busy(app: &App, id: &str, model: &str, now: DateTime<Utc>) -> bool {
    let sessions = app.sessions.session_counts(app.cfg().session_affinity_idle_seconds);
    peers(app, id).iter().any(|a| {
        let st = a.state.lock();
        st.active_requests.load(Ordering::Acquire) > 0
            || st.last_used.is_some_and(|at| now - at < chrono::Duration::minutes(5))
            || sessions.get(&a.id).is_some_and(|(recent, _)| *recent > 0)
            || st.quota_refreshing
            || st.cooldowns.iter().chain(st.quota_cooldowns.iter()).any(|(m, at)| *at > now && (m == "*" || m == model))
    })
}

struct ForegroundGuard {
    app: Arc<App>,
    account: String,
    accounts: Vec<Arc<Account>>,
    cancel: Arc<tokio_util::sync::CancellationToken>,
}

impl ForegroundGuard {
    fn new(app: &Arc<App>, id: &str) -> Self {
        let accounts = peers(app, id);
        let cancel = Arc::new(tokio_util::sync::CancellationToken::new());
        for acct in &accounts {
            let mut st = acct.state.lock();
            if st.active_requests.load(Ordering::Acquire) > 0 {
                cancel.cancel();
            }
            st.window_starter_cancel = Some(cancel.clone());
        }
        Self { app: app.clone(), account: id.into(), accounts, cancel }
    }
}

impl Drop for ForegroundGuard {
    fn drop(&mut self) {
        // A config reload may replace an Account while preserving its token.
        let mut accounts = self.accounts.clone();
        accounts.extend(peers(&self.app, &self.account));
        for acct in &accounts {
            let mut st = acct.state.lock();
            if st.window_starter_cancel.as_ref().is_some_and(|c| Arc::ptr_eq(c, &self.cancel)) {
                st.window_starter_cancel = None;
            }
        }
    }
}

struct Api {
    http: reqwest::Client,
    token: String,
    origin: String,
}

impl Api {
    async fn new(app: &Arc<App>, acct: &Arc<Account>) -> Result<Self> {
        crate::oauth::ensure_fresh(app, acct, chrono::Duration::minutes(5), false)
            .await
            .map_err(|_| anyhow!("Could not refresh Claude subscription credentials"))?;
        let token = match &*acct.cred.read() {
            Credential::OAuth(o) if o.base_url.is_none() => o.access_token.clone(),
            _ => return Err(anyhow!("Subscription credentials changed")),
        };
        let origin = crate::upstream::CLAUDE_API.to_string();
        #[cfg(test)]
        let origin = app.window_starter_test_origin.lock().clone().unwrap_or(origin);
        Ok(Self { http: app.http.reset_for_account(acct), token, origin })
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let response = self
            .http
            .get(format!("{}{path}", self.origin))
            .bearer_auth(&self.token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("user-agent", crate::clients::claude_user_agent())
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| anyhow!("Could not read Claude window status"))?;
        ensure!(response.status().is_success(), "Claude window status unavailable (HTTP {})", response.status());
        json_body(response).await
    }

    async fn identity(&self, account: &str) -> Result<String> {
        let profile = self.get("/api/oauth/profile").await?;
        let id = profile["account"]["uuid"].as_str().and_then(|s| uuid::Uuid::parse_str(s).ok());
        ensure!(id.map(|id| id.to_string()).as_deref() == Some(account), "Claude account identity changed");
        profile["organization"]["uuid"]
            .as_str()
            .and_then(|s| uuid::Uuid::parse_str(s).ok())
            .map(|id| id.to_string())
            .ok_or_else(|| anyhow!("Claude organization identity unavailable"))
    }

    async fn usage(&self, app: &App, acct: &Arc<Account>, id: &str) -> Result<Value> {
        let epoch = acct.state.lock().identity_epoch;
        let usage = self.get("/api/oauth/usage").await?;
        ensure!(native_id(acct)? == id && acct.state.lock().identity_epoch == epoch, "Claude account changed");
        // The general quota merger deliberately retains omitted windows. An
        // explicit null from this endpoint instead means an inactive window.
        let mut normalized = usage.clone();
        for key in ["five_hour", "seven_day"] {
            if usage.get(key) == Some(&Value::Null) {
                normalized[key] = json!({"utilization":0.0,"resets_at":null});
            }
        }
        for a in peers(app, id) {
            let mut st = a.state.lock();
            // A foreground response that overtook this preflight keeps its quota.
            if st.window_starter_cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
                continue;
            }
            st.quota_epoch += 1;
            crate::quota::usage(&mut st, Provider::Claude, &normalized);
        }
        Ok(usage)
    }
}

async fn json_body(mut response: reqwest::Response) -> Result<Value> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| anyhow!("Claude response interrupted"))? {
        ensure!(bytes.len() + chunk.len() <= MAX_BODY, "Claude response exceeded the window starter size limit");
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| anyhow!("Invalid Claude response"))
}

pub async fn start(app: Arc<App>, acct: Arc<Account>, action: Action, automatic: bool) -> Result<View> {
    ensure!(uuid::Uuid::parse_str(&action.request_id).is_ok(), "Window starter request_id must be a UUID");
    let id = native_id(&acct)?;
    let result = run(&app, &acct, &id, action, automatic).await;
    match result {
        Ok(view) => {
            store(&app, &id, &view);
            Ok(view)
        }
        Err(e) => {
            let view = View::new("unavailable", e.to_string(), Utc::now());
            store(&app, &id, &view);
            Err(e)
        }
    }
}

async fn run(app: &Arc<App>, acct: &Arc<Account>, id: &str, action: Action, automatic: bool) -> Result<View> {
    let cfg = app.cfg();
    let root = cfg.auth_dir();
    let policy = cfg.window_starter.clone();
    current(app, acct, id, &root, automatic)?;
    let now = Utc::now();
    if !action.check_only && busy(app, id, &policy.model, now) {
        return Ok(View::new("skipped", "Real work, a cooldown or a quota operation is already active", now));
    }
    let api = Api::new(app, acct).await?;
    let organization = api.identity(id).await?;
    current(app, acct, id, &root, automatic)?;
    // The same organization lock as reset redemption: neither operation can
    // race the other, even across processes or duplicate credential files.
    let reset_lock = crate::banked_resets::ledger::Ledger::open(&root, &format!("claude:{organization}"))?;
    ensure!(reset_lock.journal.unresolved().is_none(), "Resolve the pending banked reset before starting a window");
    let mut journal = Journal::open(&root, id, &organization)?;
    let foreground = ForegroundGuard::new(app, id);
    let usage = api.usage(app, acct, id).await?;
    let now = Utc::now();
    let state = window(&usage, now);
    if let Window::Active(reset) = state {
        if let Some(attempt) = journal.attempt.as_mut()
            && matches!(attempt.outcome, Outcome::Pending | Outcome::Unknown)
        {
            attempt.outcome = Outcome::Started;
            attempt.resets_at = Some(reset);
            attempt.retry_after = reset;
            journal.save()?;
        }
        let mut view = View::new("active", "The provider already reports an active usage window", now);
        view.resets_at = Some(reset);
        if let Some(a) = &journal.attempt {
            view.attempted_at = Some(a.at);
            view.automatic = a.automatic;
        }
        view.next_check_at = reset.min(now + chrono::Duration::seconds(CHECK_SECONDS));
        return Ok(view);
    }
    if let Some(a) = &journal.attempt
        && (a.request_id == action.request_id
            || a.retry_after > now
                && !reset_lock.journal.operations.values().any(|op| {
                    op.updated_at > a.at
                        && matches!(op.status.as_str(), "applied" | "already_used" | "reconciled_used")
                        && op.clears.iter().any(|w| w == "5h" || w == "*")
                }))
    {
        return Ok(a.view(now));
    }
    ensure!(state == Window::Inactive, "Provider usage does not confirm an inactive five-hour window");
    if action.check_only {
        return Ok(View::new("inactive", "The provider reports an inactive five-hour window", now));
    }
    let model = acct
        .resolve(&policy.model)
        .filter(|m| m == &policy.model && acct.lists(m))
        .ok_or_else(|| anyhow!("The configured window starter model is unavailable on this account"))?;
    ensure!(weekly_available(&usage, now), "Weekly subscription quota is exhausted or unknown");
    ensure!(!acct.state.lock().quota.exhausted(&model), "An applicable subscription usage limit is exhausted");
    current(app, acct, id, &root, automatic)?;
    ensure!(app.cfg().window_starter == policy, "Window starter settings changed; try again");
    if foreground.cancel.is_cancelled() || busy(app, id, &model, Utc::now()) {
        return Ok(View::new("skipped", "Real work arrived while checking provider usage", Utc::now()));
    }

    let headers = HeaderMap::new();
    let prepared = crate::upstream::prepare(
        &crate::upstream::Target {
            acct,
            cfg: &cfg,
            client_headers: &headers,
            model: &model,
            wire: crate::ir::Format::Claude,
            passthrough: false,
            stream: false,
            count_tokens: false,
            cache_key: None,
            provider_session: None,
            cache_defaults: false,
        },
        json!({"model":model,"messages":[{"role":"user","content":"Reply OK."}],"max_tokens":8,"stream":false,"thinking":{"type":"disabled"}}),
    );
    #[cfg(test)]
    let prepared = {
        let mut p = prepared;
        if app.window_starter_test_origin.lock().is_some() {
            p.url = format!("{}/v1/messages?beta=true", api.origin);
        }
        p
    };
    ensure!(
        prepared.headers.iter().any(|(key, value)| {
            key.eq_ignore_ascii_case("authorization") && value == &format!("Bearer {}", api.token)
        }) && !prepared.headers.iter().any(|(key, _)| key.eq_ignore_ascii_case("x-api-key")),
        "Subscription authorization changed during preflight; check credentials before starting a window"
    );
    // Bounded, direct subscription request: no tools, groups, failover, redirects
    // or HTTP retries. The native preparation supplies the account's auth dialect.
    let mut rb = api.http.post(&prepared.url).timeout(Duration::from_secs(20));
    for (key, value) in &prepared.headers {
        rb = rb.header(key, value);
    }
    let at = Utc::now();
    journal.attempt = Some(Attempt {
        request_id: action.request_id,
        at,
        automatic,
        outcome: Outcome::Pending,
        retry_after: at + chrono::Duration::seconds(UNCERTAIN_SECONDS),
        resets_at: None,
    });
    journal.save()?; // A crash after this point must reconcile before resending.
    let mut capture = crate::usage::capture::RequestUsage::new(app.usage.clone(), &model);
    let mut labels = HeaderMap::new();
    labels.insert(crate::usage::capture::CLIENT_HEADER, "system:window-starter".parse().unwrap());
    capture.client(&labels);
    capture.program(Some("Window starter"));
    capture.attempt(acct);
    let epoch = acct.quota_epoch();
    let sent = tokio::select! {
        biased;
        _ = foreground.cancel.cancelled() => None,
        _ = app.shutdown.cancelled() => None,
        result = rb.json(&prepared.body).send() => Some(result),
    };
    let mut outcome = Outcome::Unknown;
    let mut status = None;
    if let Some(Ok(response)) = sent {
        let code = response.status().as_u16();
        status = Some(code);
        let retry_after = retry_after(response.headers(), at);
        if let Some(tap) = capture.tap() {
            tap.headers(response.headers(), code);
        }
        // A late response from a replaced credential must not mutate its new owner.
        if native_id(acct).ok().as_deref() == Some(id) {
            crate::quota::observe(acct, response.headers(), epoch);
        }
        if code < 300 {
            if let Ok(value) = json_body(response).await
                && let Some(tap) = capture.tap()
            {
                tap.wire(&value);
            }
        } else if (400..500).contains(&code) && !matches!(code, 408 | 409) {
            outcome = Outcome::Refused;
            journal.attempt.as_mut().unwrap().retry_after = retry_after;
        }
    }
    capture.finish(status, None, true);
    let attempt = journal.attempt.as_mut().unwrap();
    attempt.outcome = outcome;
    journal.save()?;
    // A successful generation is not proof the provider started the intended
    // window. Re-read usage, including after an uncertain network completion.
    if current(app, acct, id, &root, false).is_ok()
        && let Ok(usage) = api.usage(app, acct, id).await
        && let Window::Active(reset) = window(&usage, Utc::now())
    {
        let attempt = journal.attempt.as_mut().unwrap();
        attempt.outcome = Outcome::Started;
        attempt.resets_at = Some(reset);
        attempt.retry_after = reset;
        journal.save()?;
    }
    Ok(journal.attempt.as_ref().unwrap().view(Utc::now()))
}

fn retry_after(headers: &HeaderMap, now: DateTime<Utc>) -> DateTime<Utc> {
    let seconds = headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.parse::<i64>()
                .ok()
                .or_else(|| DateTime::parse_from_rfc2822(v).ok().map(|at| (at.with_timezone(&Utc) - now).num_seconds()))
        })
        .unwrap_or(30 * 60)
        .clamp(60, 24 * 3600);
    now + chrono::Duration::seconds(seconds)
}

pub async fn poller(app: Arc<App>) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = app.shutdown.cancelled() => return,
            _ = interval.tick() => {},
        }
        let policy = app.cfg().window_starter.clone();
        if !policy.in_hours(Utc::now()) {
            continue;
        }
        // Serial and capped by per-request deadlines; foreground work always wins.
        // The identity journal also coalesces duplicate credential files.
        for acct in app.pool.all() {
            if app.shutdown.is_cancelled() {
                return;
            }
            if !policy.selected(&acct)
                || acct.state.lock().window_starter.as_ref().is_some_and(|v| v.next_check_at > Utc::now())
            {
                continue;
            }
            let action = Action { request_id: uuid::Uuid::new_v4().to_string(), check_only: false };
            let _ = start(app.clone(), acct, action, true).await;
        }
    }
}
