//! The dashboard's faults, worked out on the server so they can be pushed. The rules
//! mirror `alertsList()` and `acctState()` in ui/app.js, so every notification is
//! something the faults menu shows too, plus whole providers that have run out.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::pools::{Member, Pool};
use crate::quota::Window;

/// Which `notifications` setting sends a fault.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Event {
    SignIn,
    ProviderExhausted,
    AccountUsedUp,
    AccountErrors,
    /// A named client's key used from a new address, or refused at one. Sent as it
    /// happens rather than found by the check.
    ClientAddress,
    /// A banked reset used automatically, needing review, or expired unused. Sent as it
    /// happens.
    BankedReset,
    /// Counted for the app badge, never sent (rate limits come and go too often).
    #[default]
    Quiet,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fault {
    /// Stable identity: `signin:<id>`, `quota:<id>`, `error:<id>`, `failures:<id>`,
    /// `rate:<id>`, `pace:<id>:<window>`, `pace:pool:<provider>:<window>` or
    /// `provider:<provider>`. Also the notification's tag.
    pub key: String,
    pub event: Event,
    /// The provider group, so one provider running out can stand in for its accounts.
    pub provider: String,
    pub title: String,
    pub body: String,
    /// Where a click lands in the dashboard.
    pub path: String,
    /// The rule that tripped: `signin`, `quota`, `rate_limit`, `error`, `failures`, `pace` or `provider`.
    pub kind: &'static str,
    /// `err` while requests fail, `warn` while an account sits out for a while.
    pub level: &'static str,
    /// The account it is about; none for a whole provider or all its subscriptions.
    pub account: Option<String>,
    pub label: Option<String>,
    pub provider_name: String,
    /// What tripped, without the account or a countdown ("Weekly limit used up").
    pub summary: String,
    /// More about it, also without a countdown.
    pub detail: Option<String>,
    /// When it should clear by itself.
    pub until: Option<DateTime<Utc>>,
}

/// Space left under this on the database's filesystem is an error: SQLite stops
/// writing when the disk fills, and usage recording with it.
const LOW_DISK: u64 = 1 << 30;

/// The usage database growing past `warn_mb`, or the disk under it filling up.
pub fn storage(stats: Option<&crate::usage::store::DatabaseStats>, warn_mb: u64) -> Option<Fault> {
    let stats = stats?;
    let mb = |b: u64| format!("{} MB", b / 1_000_000);
    let fault = |level, summary: &str, detail: String| Fault {
        key: "storage:usage".into(),
        event: Event::Quiet,
        provider: String::new(),
        provider_name: "Usage".into(),
        title: summary.to_string(),
        body: detail.clone(),
        path: "#/usage".into(),
        kind: "storage",
        level,
        summary: summary.to_string(),
        detail: Some(detail),
        ..Default::default()
    };
    if let Some(free) = stats.disk_free_bytes.filter(|f| *f < LOW_DISK) {
        return Some(fault(
            "err",
            "Disk almost full",
            format!("{} left where the usage database is; usage recording stops when the disk is full.", mb(free)),
        ));
    }
    (warn_mb > 0 && stats.bytes > warn_mb << 20).then(|| {
        fault(
            "warn",
            "Usage database is large",
            format!(
                "It is {} and grows about {} a day. Lower usage.retention-days, or raise usage.warn-database-mb.",
                mb(stats.bytes),
                mb(stats.growth_bytes_per_day)
            ),
        )
    })
}

/// The app badge's count. A pace warning is not counted: it can hold for days and
/// asks for no action, only attention.
pub fn badge(faults: &[Fault]) -> usize {
    faults.iter().filter(|f| f.kind != "pace").count()
}

impl Fault {
    /// The management API's form. It has no countdown text, so it only changes when the
    /// fault does: clients count down to `until` themselves.
    pub fn json(&self) -> Value {
        json!({
            "key": self.key,
            "kind": self.kind,
            "level": self.level,
            "provider": self.provider,
            "provider_name": self.provider_name,
            "account_id": self.account,
            "label": self.label,
            "title": self.summary,
            "detail": self.detail,
            "until": self.until.map(|t| t.to_rfc3339()),
            "path": self.path,
            "badge": self.kind != "pace",
        })
    }
}

/// What the rules need to know about one account.
#[derive(Debug, Clone, Default)]
pub struct AccountView {
    pub id: String,
    /// Provider group: the provider id, or the compatibility group's name.
    pub provider: String,
    /// The account's provider as the dashboard names it ("Claude", "OpenAI" for a key).
    pub provider_name: String,
    /// The provider group's name ("Claude", "Codex").
    pub group_name: String,
    pub label: String,
    pub api_key: bool,
    /// A Claude or Codex sign-in: a subscription that reports limits, even before it has.
    pub subscription: bool,
    /// The subscription plan, as the quota reports it ("max_20x").
    pub plan: Option<String>,
    /// The provider confirmed the account has no 5-hour limit.
    pub no_five_hour: bool,
    pub disabled: bool,
    /// From `AccountState::pauses`: model -> (until, kind).
    pub pauses: BTreeMap<String, (DateTime<Utc>, &'static str)>,
    pub last_error: Option<String>,
    pub windows: Vec<Window>,
    pub history: Vec<crate::outlook::History>,
    /// Failed requests (not cancellations) in the last hour.
    pub failures: u64,
    /// Per current window (name, model scope): the points of it each client used, by the
    /// client's name (from quota attribution).
    pub clients: Vec<(String, Option<String>, BTreeMap<String, f64>)>,
}

impl AccountView {
    fn member(&self) -> Member<'_> {
        Member {
            id: &self.id,
            provider: &self.provider,
            plan: self.plan.as_deref(),
            subscription: self.subscription,
            disabled: self.disabled,
            windows: &self.windows,
            history: &self.history,
            forecast_available: !self.signin_error(),
            no_five_hour: self.no_five_hour,
            paused: matches!(self.pauses.get("*"), Some((_, "quota" | "rate_limit"))),
        }
    }

    /// Each client's points of a current window.
    fn clients_of(&self, w: &Window) -> Option<&BTreeMap<String, f64>> {
        self.clients.iter().find(|(name, model, _)| *name == w.name && *model == w.model).map(|(.., c)| c)
    }

    fn signin_error(&self) -> bool {
        !self.api_key && self.last_error.as_deref().is_some_and(signin_error)
    }

    /// Can't take any request now: paused whole for a limit, or its sign-in expired.
    fn unavailable(&self) -> bool {
        matches!(self.pauses.get("*"), Some((_, "quota" | "rate_limit"))) || self.signin_error()
    }

    fn path(&self) -> String {
        format!("#/accounts/{}", url::form_urlencoded::byte_serialize(self.id.as_bytes()).collect::<String>())
    }
}

/// The text the dashboard reads as an expired sign-in (`SIGNIN_ERR` in ui/app.js).
/// When a window runs out if use continues at its pace so far, if that is before
/// it renews. As the dashboard's pace mark: at least a quarter of the window gone
/// and 20% used.
pub fn early_run_out(w: &Window, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let resets = w.resets_at.filter(|r| *r > now)?;
    let length = crate::attribution::length(&w.name)?;
    let elapsed = (now - (resets - length)).num_seconds() as f64;
    if w.used < 20.0 || w.used >= 100.0 || elapsed < 0.25 * length.num_seconds() as f64 {
        return None;
    }
    let per_second = w.used / elapsed;
    let out = now + chrono::Duration::seconds(((100.0 - w.used) / per_second) as i64);
    (out < resets).then_some(out)
}

pub fn signin_error(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "invalid_grant",
        "refresh token",
        "sign in again",
        "reauthenticat",
        "re-authenticat",
        "unauthorized",
        "unauthorised",
        "token expired",
        "token has expired",
        "expired token",
        "revoked",
    ]
    .iter()
    .any(|p| t.contains(p))
        || t.match_indices("401").any(|(i, _)| {
            let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            !word(t[..i].chars().next_back()) && !word(t[i + 3..].chars().next())
        })
}

/// Each provider's subscriptions taken together, as the overview's combined limits draw them.
pub fn pools(accounts: &[AccountView], now: DateTime<Utc>) -> Vec<Pool> {
    crate::pools::pools(&accounts.iter().map(AccountView::member).collect::<Vec<_>>(), now)
}

pub fn faults(accounts: &[AccountView], now: DateTime<Utc>) -> Vec<Fault> {
    let mut out = Vec::new();
    let members: Vec<Member> = accounts.iter().map(AccountView::member).collect();
    // Smart-quota routing runs down the account that renews first on purpose, so a provider
    // with several subscriptions has one pace warning per window, for all of them together.
    let pooled = crate::pools::pooled(&members);
    for a in accounts.iter().filter(|a| !a.disabled) {
        let fault = |key: &str, event, title: String, body: String| Fault {
            key: format!("{key}:{}", a.id),
            event,
            provider: a.provider.clone(),
            title,
            body,
            path: a.path(),
            account: Some(a.id.clone()),
            label: Some(a.label.clone()),
            provider_name: a.provider_name.clone(),
            ..Default::default()
        };
        // The latest pause decides the state, as in the dashboard.
        let pause = a.pauses.iter().max_by_key(|(_, (until, _))| *until);
        let error = pause.is_none() && a.last_error.is_some();
        match pause {
            Some((_, (until, "quota"))) => {
                let spent = spent_window(&a.windows, now);
                let back = spent.and_then(|w| w.resets_at).unwrap_or(*until);
                let limit = spent.map_or("usage", |w| window_title(&w.name));
                out.push(Fault {
                    kind: "quota",
                    level: "warn",
                    summary: format!("{} limit used up", capitalized(limit)),
                    until: Some(back),
                    ..fault(
                        "quota",
                        Event::AccountUsedUp,
                        format!("{}: {limit} limit used up", a.label),
                        format!("{} · back in {}.", a.provider_name, span(back - now)),
                    )
                });
            }
            Some((model, (until, "rate_limit"))) => {
                let what = if model == "*" { "Every model".to_string() } else { model.clone() };
                out.push(Fault {
                    kind: "rate_limit",
                    level: "warn",
                    summary: "Rate limited".into(),
                    detail: Some(format!("{what} is paused.")),
                    until: Some(*until),
                    ..fault(
                        "rate",
                        Event::Quiet,
                        format!("{}: rate limited", a.label),
                        format!("{what} is paused for {}.", span(*until - now)),
                    )
                });
            }
            _ => {}
        }
        // A window that this pace would use up before it renews, when the account is its
        // provider's only subscription. Not pushed: the meters say it too; this says who.
        if !matches!(pause, Some((_, (_, "quota")))) && !pooled.contains_key(a.provider.as_str()) {
            for w in &a.windows {
                let Some(pace) = early_run_out(w, now) else { continue };
                let clients = a.clients_of(w).into_iter().flatten().map(|(label, points)| (label.as_str(), *points));
                let who = top_client(clients, w.used);
                let limit = window_title(&w.name);
                out.push(Fault {
                    key: format!(
                        "pace:{}:{}{}",
                        a.id,
                        w.name,
                        w.model.as_deref().map(|m| format!(":{m}")).unwrap_or_default()
                    ),
                    kind: "pace",
                    level: "warn",
                    summary: format!("{} limit runs out early at this pace", capitalized(limit)),
                    detail: Some(format!("It would run out before it renews.{}", who.clone().unwrap_or_default())),
                    until: w.resets_at,
                    ..fault(
                        "pace",
                        Event::Quiet,
                        format!("{}: {limit} limit runs out early", a.label),
                        format!(
                            "At this pace in {}, before it renews in {}.{}",
                            span(pace - now),
                            span(w.resets_at.unwrap_or(now) - now),
                            who.unwrap_or_default()
                        ),
                    )
                });
            }
        }
        if error {
            if a.signin_error() {
                out.push(Fault {
                    kind: "signin",
                    level: "err",
                    summary: "Sign-in expired".into(),
                    detail: Some("Sign in again to put it back in rotation.".into()),
                    ..fault(
                        "signin",
                        Event::SignIn,
                        format!("{} sign-in expired", a.provider_name),
                        format!("{}: sign in again to put it back in rotation.", a.label),
                    )
                });
            } else {
                let text = a.last_error.as_deref().unwrap_or_default();
                let message: String = text.chars().take(120).collect();
                out.push(Fault {
                    kind: "error",
                    level: "err",
                    summary: "Account error".into(),
                    // The dashboard's faults menu shows this much of it.
                    detail: Some(text.chars().take(160).collect()),
                    ..fault(
                        "error",
                        Event::AccountErrors,
                        format!("{} account error", a.provider_name),
                        format!("{}: {message}", a.label),
                    )
                });
            }
        }
        if a.failures >= 3 && !error {
            let mut f = Fault {
                kind: "failures",
                level: "err",
                summary: format!("{} failed requests", a.failures),
                detail: Some("In the last hour.".into()),
                ..fault(
                    "failures",
                    Event::AccountErrors,
                    format!("{} failed requests", a.failures),
                    format!("{} · {} in the last hour.", a.provider_name, a.label),
                )
            };
            f.path =
                format!("#/requests?acc={}", url::form_urlencoded::byte_serialize(a.id.as_bytes()).collect::<String>());
            out.push(f);
        }
    }

    // Providers with several subscriptions: one pace warning per window, for them all.
    out.extend(crate::pools::pools(&members, now).iter().filter_map(|p| pool_pace(p, accounts, now)));

    // Providers whose every enabled account is out.
    let mut groups: BTreeMap<&str, Vec<&AccountView>> = BTreeMap::new();
    for a in accounts.iter().filter(|a| !a.disabled) {
        groups.entry(a.provider.as_str()).or_default().push(a);
    }
    for (provider, members) in groups {
        if !members.iter().all(|a| a.unavailable()) {
            continue;
        }
        let name = &members[0].group_name;
        let back = members
            .iter()
            .filter_map(|a| a.pauses.get("*").filter(|(_, kind)| matches!(*kind, "quota" | "rate_limit")))
            .map(|(until, _)| *until)
            .min();
        let used_up = members.iter().all(|a| matches!(a.pauses.get("*"), Some((_, "quota"))));
        let title = if used_up {
            format!("Every {name} account is used up")
        } else {
            format!("No {name} account can take requests")
        };
        let body = match back {
            Some(t) => format!("Requests for {name} models will fail. The first is back in {}.", span(t - now)),
            None => format!("Requests for {name} models will fail until an account is signed in again."),
        };
        out.push(Fault {
            key: format!("provider:{provider}"),
            event: Event::ProviderExhausted,
            provider: provider.to_string(),
            summary: title.clone(),
            title,
            body,
            path: "#/accounts".into(),
            kind: "provider",
            level: "err",
            provider_name: name.clone(),
            detail: Some(format!("Requests for {name} models will fail.")),
            until: back,
            ..Default::default()
        });
    }
    out
}

/// A provider's subscriptions together running out at this pace before renewals bring
/// capacity back. Not pushed, as one account's warning; it clears by the renewal it waits for.
fn pool_pace(pool: &Pool, accounts: &[AccountView], now: DateTime<Utc>) -> Option<Fault> {
    let back = pool.runs_out?;
    let reporting: Vec<(&AccountView, f64)> =
        pool.weights.iter().filter_map(|(id, k)| Some((accounts.iter().find(|a| a.id == *id)?, *k))).collect();
    let name = reporting.first()?.0.group_name.clone();
    // The client that used most of the windows, each counting as it does in the pool. A
    // window that has renewed counts as unused, and its clients are the last window's.
    let mut clients = BTreeMap::<&str, f64>::new();
    for (a, k) in &reporting {
        let live = a
            .windows
            .iter()
            .find(|w| w.name == pool.window && w.model == pool.model && w.resets_at.is_none_or(|r| r > now));
        for (label, points) in live.and_then(|w| a.clients_of(w)).into_iter().flatten() {
            *clients.entry(label).or_default() += k * points;
        }
    }
    let weight: f64 = pool.weights.iter().map(|(_, k)| k).sum();
    let who = top_client(clients, pool.used * weight).unwrap_or_default();
    let scope = pool.model.as_deref().map(|m| format!("{} ", capitalized(m))).unwrap_or_default();
    let title = format!("{name} {scope}{} limits run out early at this pace", window_title(&pool.window));
    let together = match reporting.len() {
        1 => "The one account reporting it runs out".to_string(),
        n => format!("Together the {n} accounts run out"),
    };
    let detail = format!("{together} before renewals bring capacity back.{who}");
    Some(Fault {
        key: format!(
            "pace:pool:{}:{}{}",
            pool.provider,
            pool.window,
            pool.model.as_deref().map(|m| format!(":{m}")).unwrap_or_default()
        ),
        event: Event::Quiet,
        provider: pool.provider.clone(),
        title: title.clone(),
        body: detail.clone(),
        path: "#/overview".into(),
        kind: "pace",
        level: "warn",
        provider_name: name,
        summary: title,
        detail: Some(detail),
        until: Some(back),
        ..Default::default()
    })
}

/// " The laptop used 62% of it.": the client that used most of a window, of `used` points.
fn top_client<'a>(clients: impl IntoIterator<Item = (&'a str, f64)>, used: f64) -> Option<String> {
    let (label, points) = clients.into_iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
    (points > 0.0 && used > 0.0).then(|| format!(" {label} used {:.0}% of it.", 100.0 * points / used))
}

/// A provider that was out has an account that can take requests again.
pub fn recovered(accounts: &[AccountView], provider: &str) -> Option<Fault> {
    let back = accounts.iter().find(|a| !a.disabled && a.provider == provider && !a.unavailable())?;
    Some(Fault {
        key: format!("provider:{provider}"),
        event: Event::ProviderExhausted,
        provider: provider.to_string(),
        title: format!("{} is back", back.group_name),
        body: format!("{} can take requests again.", back.label),
        path: back.path(),
        kind: "recovered",
        level: "ok",
        ..Default::default()
    })
}

/// The 5-hour or weekly window that is used up, as the dashboard picks it.
fn spent_window(windows: &[Window], now: DateTime<Utc>) -> Option<&Window> {
    let live = |short: bool| {
        windows
            .iter()
            .filter(|w| w.model.is_none() && w.resets_at.is_none_or(|r| r > now))
            .filter(move |w| is_hours(&w.name) == short)
            .max_by(|x, y| x.used.total_cmp(&y.used))
    };
    [live(true), live(false)].into_iter().flatten().find(|w| w.used >= 100.0)
}

fn is_hours(name: &str) -> bool {
    name.strip_suffix('h').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

fn window_title(name: &str) -> &'static str {
    match name {
        "5h" => "5-hour",
        "day" => "daily",
        n if is_hours(n) => "short",
        _ => "weekly",
    }
}

/// "2h 14m", "3d 7h", "45m", "<1m": the dashboard's countdowns, at most two units.
pub fn span(d: chrono::Duration) -> String {
    let s = d.num_seconds().max(0);
    match s {
        0..60 => "<1m".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d {}h", s / 86400, s % 86400 / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn acct(id: &str, provider: &str) -> AccountView {
        AccountView {
            id: id.into(),
            provider: provider.into(),
            provider_name: "Claude".into(),
            group_name: "Claude".into(),
            label: format!("{id}@example.com"),
            ..Default::default()
        }
    }

    fn keys(f: &[Fault]) -> Vec<&str> {
        f.iter().map(|f| f.key.as_str()).collect()
    }

    #[test]
    fn reads_sign_in_errors_like_the_dashboard() {
        for text in [
            "token refresh failed: invalid_grant",
            "401 Unauthorized",
            "upstream said 401",
            "Your refresh token was revoked",
            "OAuth token has expired",
            "please sign in again",
        ] {
            assert!(signin_error(text), "{text}");
        }
        for text in ["429 rate limited", "token refresh failed: connection reset", "status 4010", "5401 bytes"] {
            assert!(!signin_error(text), "{text}");
        }
    }

    #[test]
    fn mirrors_the_faults_menu() {
        let now = Utc::now();
        let mut signin = acct("a", "claude");
        signin.last_error = Some("token refresh failed: invalid_grant".into());
        let mut key = acct("k", "claude");
        key.api_key = true;
        key.last_error = Some("401 Unauthorized".into());
        let mut spent = acct("q", "claude");
        spent.pauses.insert("*".into(), (now + Duration::hours(3), "quota"));
        spent.windows =
            vec![Window { name: "week".into(), used: 100.0, resets_at: Some(now + Duration::hours(50)), model: None }];
        // Cooling wins over an old error, as in the dashboard.
        let mut limited = acct("r", "claude");
        limited.pauses.insert("claude-opus".into(), (now + Duration::minutes(5), "rate_limit"));
        limited.last_error = Some("429".into());
        limited.failures = 4;
        let mut checking = acct("c", "claude");
        checking.pauses.insert("*".into(), (now + Duration::seconds(30), "checking"));
        let mut off = acct("d", "claude");
        off.disabled = true;
        off.last_error = Some("invalid_grant".into());

        let f = faults(&[signin, key, spent, limited, checking, off], now);
        assert_eq!(keys(&f), ["signin:a", "error:k", "quota:q", "rate:r", "failures:r"]);
        assert_eq!(f[0].title, "Claude sign-in expired");
        assert_eq!(f[0].path, "#/accounts/a");
        assert_eq!(f[2].title, "q@example.com: weekly limit used up");
        assert_eq!(f[2].body, "Claude · back in 2d 2h.");
        assert_eq!(f[3].event, Event::Quiet);
        assert_eq!(f[4].path, "#/requests?acc=r");
    }

    #[test]
    fn the_api_form_names_the_account_and_counts_down_to_a_time() {
        let now = Utc::now();
        let mut signin = acct("a", "claude");
        signin.last_error = Some("token refresh failed: invalid_grant".into());
        let mut key = acct("k", "claude");
        key.api_key = true;
        key.last_error = Some(format!("upstream said {}", "x".repeat(300)));
        let mut spent = acct("q", "claude");
        let back = now + Duration::hours(50);
        spent.pauses.insert("*".into(), (now + Duration::hours(3), "quota"));
        spent.windows = vec![Window { name: "5h".into(), used: 100.0, resets_at: Some(back), model: None }];
        let mut limited = acct("r", "claude");
        let until = now + Duration::minutes(5);
        limited.pauses.insert("claude-opus".into(), (until, "rate_limit"));
        limited.failures = 4;

        let accounts = [signin, key, spent, limited];
        let api: Vec<Value> = faults(&accounts, now).iter().map(Fault::json).collect();
        let pick = |key: &str| api.iter().find(|v| v["key"] == key).unwrap().clone();

        let s = pick("signin:a");
        assert_eq!((s["kind"].as_str(), s["level"].as_str()), (Some("signin"), Some("err")));
        assert_eq!((s["account_id"].as_str(), s["label"].as_str()), (Some("a"), Some("a@example.com")));
        assert_eq!(s["title"], "Sign-in expired");
        assert_eq!(s["detail"], "Sign in again to put it back in rotation.");
        assert!(s["until"].is_null());
        assert_eq!(s["path"], "#/accounts/a");

        let e = pick("error:k");
        assert_eq!((e["title"].as_str(), e["level"].as_str()), (Some("Account error"), Some("err")));
        assert_eq!(e["detail"].as_str().unwrap().chars().count(), 160);

        let q = pick("quota:q");
        assert_eq!((q["kind"].as_str(), q["level"].as_str()), (Some("quota"), Some("warn")));
        assert_eq!(q["title"], "5-hour limit used up");
        assert!(q["detail"].is_null());
        assert_eq!(q["until"], back.to_rfc3339(), "back when the spent window resets");

        let r = pick("rate:r");
        assert_eq!((r["kind"].as_str(), r["title"].as_str()), (Some("rate_limit"), Some("Rate limited")));
        assert_eq!(r["detail"], "claude-opus is paused.");
        assert_eq!(r["until"], until.to_rfc3339());

        let n = pick("failures:r");
        assert_eq!((n["title"].as_str(), n["detail"].as_str()), (Some("4 failed requests"), Some("In the last hour.")));
        assert_eq!(n["path"], "#/requests?acc=r");

        // Nothing in it moves with the clock, so a live stream only sends real changes.
        let later: Vec<Value> = faults(&accounts, now + Duration::minutes(2)).iter().map(Fault::json).collect();
        assert_eq!(later, api);
    }

    #[test]
    fn a_provider_runs_out_when_every_enabled_account_is_out() {
        let now = Utc::now();
        let mut a = acct("a", "claude");
        a.pauses.insert("*".into(), (now + Duration::hours(2), "quota"));
        let mut b = acct("b", "claude");
        b.pauses.insert("*".into(), (now + Duration::minutes(30), "quota"));
        let mut off = acct("c", "claude");
        off.disabled = true;
        let mut other = acct("x", "codex");
        other.group_name = "Codex".into();
        other.pauses.insert("gpt-6".into(), (now + Duration::hours(1), "rate_limit"));

        let f = faults(&[a.clone(), b.clone(), off, other.clone()], now);
        let p = f.iter().find(|f| f.key == "provider:claude").unwrap();
        assert_eq!(p.title, "Every Claude account is used up");
        assert_eq!(p.body, "Requests for Claude models will fail. The first is back in 30m.");
        let api = p.json();
        assert_eq!((api["kind"].as_str(), api["level"].as_str()), (Some("provider"), Some("err")));
        assert_eq!(
            (api["title"].as_str(), api["provider_name"].as_str()),
            (Some("Every Claude account is used up"), Some("Claude"))
        );
        assert_eq!(api["detail"], "Requests for Claude models will fail.");
        assert_eq!(api["until"], (now + Duration::minutes(30)).to_rfc3339());
        assert!(api["account_id"].is_null() && api["label"].is_null());
        assert!(!keys(&f).contains(&"provider:codex"));
        assert!(recovered(&[a.clone(), b.clone()], "claude").is_none());

        // A sign-in problem counts as out, but changes the wording.
        let mut expired = acct("e", "claude");
        expired.last_error = Some("invalid_grant".into());
        let f = faults(&[a.clone(), expired.clone()], now);
        assert_eq!(f.iter().find(|f| f.key == "provider:claude").unwrap().title, "No Claude account can take requests");

        b.pauses.clear();
        let back = recovered(&[a, b], "claude").unwrap();
        assert_eq!(back.title, "Claude is back");
        assert_eq!(back.body, "b@example.com can take requests again.");
    }

    /// A Claude subscription with these windows.
    fn sub(id: &str, windows: Vec<Window>) -> AccountView {
        AccountView { subscription: true, plan: Some("max_20x".into()), windows, ..acct(id, "claude") }
    }

    fn window(name: &str, model: Option<&str>, used: f64, left: Duration, now: DateTime<Utc>) -> Window {
        Window { name: name.into(), used, resets_at: Some(now + left), model: model.map(String::from) }
    }

    fn clients(
        name: &str,
        model: Option<&str>,
        points: &[(&str, f64)],
    ) -> (String, Option<String>, BTreeMap<String, f64>) {
        (name.into(), model.map(String::from), points.iter().map(|(c, p)| (c.to_string(), *p)).collect())
    }

    #[test]
    fn several_subscriptions_get_one_pace_warning_for_them_all() {
        let now = Utc::now();
        let week = |used, days: f64| window("week", None, used, Duration::minutes((days * 1440.0) as i64), now);
        // The account renewing first is run down on purpose: over its pace alone, but the
        // other has room until it renews, so nothing is said about either.
        let drained = [sub("a", vec![week(90.0, 2.8)]), sub("b", vec![week(0.0, 4.9)])];
        assert!(faults(&drained, now).iter().all(|f| f.kind != "pace"));
        assert_eq!(faults(&drained[..1], now)[0].key, "pace:a:week", "on its own it would be");

        // Both busy: together they run out before the first renewal brings capacity back.
        let mut a = sub("a", vec![week(90.0, 3.5)]);
        a.clients = vec![clients("week", None, &[("The laptop", 50.0), ("CI", 40.0)])];
        let mut b = sub("b", vec![week(60.0, 4.2)]);
        b.clients = vec![clients("week", None, &[("The laptop", 20.0), ("CI", 40.0)])];
        a.history = vec![crate::outlook::test_history(&a.windows[0], now, 20.0)];
        b.history = vec![crate::outlook::test_history(&b.windows[0], now, 20.0)];
        let busy = [a.clone(), b];
        let f = faults(&busy, now);
        assert_eq!(keys(&f), ["pace:pool:claude:week"], "no warning for either account");
        assert_eq!((f[0].event, f[0].account.as_deref()), (Event::Quiet, None));
        let api = f[0].json();
        assert_eq!(
            (api["kind"].as_str(), api["level"].as_str(), api["badge"].as_bool()),
            (Some("pace"), Some("warn"), Some(false))
        );
        assert!(api["account_id"].is_null() && api["label"].is_null());
        assert_eq!((api["provider"].as_str(), api["provider_name"].as_str()), (Some("claude"), Some("Claude")));
        assert_eq!(api["title"], "Claude weekly limits run out early at this pace");
        // CI used 40 + 40 of the 150 points used, the laptop 50 + 20.
        assert_eq!(
            api["detail"],
            "Together the 2 accounts run out before renewals bring capacity back. CI used 53% of it."
        );
        assert_eq!(api["until"], a.windows[0].resets_at.unwrap().to_rfc3339(), "a real renewal, not when it runs out");
        assert_eq!(api["path"], "#/overview");
        let later: Vec<Value> = faults(&busy, now + Duration::minutes(2)).iter().map(Fault::json).collect();
        assert_eq!(later, [api], "nothing in it moves with the clock");

        // A disabled account, or an API key, makes no pool: one subscription keeps its own rule.
        let mut off = sub("b", vec![week(60.0, 4.2)]);
        off.disabled = true;
        let mut key = acct("k", "claude");
        key.api_key = true;
        let f = faults(&[a, off, key], now);
        assert_eq!(keys(&f), ["pace:a:week"]);
        assert_eq!(f[0].detail.as_deref(), Some("It would run out before it renews. The laptop used 56% of it."));
        assert_eq!(f[0].account.as_deref(), Some("a"));
    }

    #[test]
    fn an_ultra_account_that_cannot_serve_leaves_the_five_hour_warning() {
        let now = Utc::now();
        let five = |used, reset| Window { name: "5h".into(), used, resets_at: Some(reset), model: None };
        let week = |used| Window { name: "week".into(), used, resets_at: Some(now + Duration::days(3)), model: None };
        let limited = AccountView {
            subscription: true,
            plan: Some("max_5x".into()),
            windows: vec![five(90.0, now + Duration::hours(1)), week(10.0)],
            ..acct("limited", "claude")
        };
        let ultra = |last_error: Option<&str>, used| AccountView {
            subscription: true,
            plan: Some("max_20x".into()),
            no_five_hour: true,
            last_error: last_error.map(String::from),
            windows: vec![week(used)],
            ..acct("ultra", "claude")
        };
        let warned = |accounts: &[AccountView]| faults(accounts, now).iter().any(|f| f.key == "pace:pool:claude:5h");
        assert!(!warned(&[limited.clone(), ultra(None, 10.0)]), "a working Ultra account takes the overflow");
        assert!(warned(&[limited.clone(), ultra(Some("401: sign in again"), 100.0)]));
        assert!(warned(&[limited.clone(), ultra(Some("refresh failed: invalid_grant"), 10.0)]), "signed out");
        assert!(warned(&[limited.clone(), ultra(None, 100.0)]), "its week used up");
        let paused = AccountView {
            pauses: BTreeMap::from([("*".to_string(), (now + Duration::hours(2), "rate_limit"))]),
            ..ultra(None, 10.0)
        };
        assert!(paused.unavailable());
        assert!(warned(&[limited, paused]), "paused as a whole");
    }

    #[test]
    fn pools_warn_for_five_hour_and_model_windows_too() {
        let now = Utc::now();
        let five = |used| window("5h", None, used, Duration::hours(1), now);
        let opus = |used, days| window("week", Some("opus"), used, Duration::days(days), now);
        let a = sub("a", vec![five(90.0), opus(80.0, 3)]);
        let b = sub("b", vec![opus(75.0, 4)]);
        let f = faults(&[a.clone(), b.clone()], now);
        assert_eq!(keys(&f), ["pace:pool:claude:5h", "pace:pool:claude:week:opus"]);
        assert_eq!(f[0].summary, "Claude 5-hour limits run out early at this pace");
        assert_eq!(
            f[0].detail.as_deref(),
            Some("The one account reporting it runs out before renewals bring capacity back.")
        );
        assert_eq!(f[0].until, a.windows[0].resets_at);
        assert_eq!(f[1].summary, "Claude Opus weekly limits run out early at this pace");
        // An account without an Opus limit of its own takes Opus until its weekly limit
        // stops it, which the weekly pool covers.
        let c = sub("c", vec![]);
        assert_eq!(keys(&faults(&[a, b, c], now)), ["pace:pool:claude:5h"]);
    }

    #[test]
    fn an_incomplete_weekly_pool_does_not_predict_from_one_accounts_pace() {
        let now = Utc::now();
        for used in [15.0, 25.0, 40.0, 55.0, 70.0, 85.0, 99.0] {
            for hours in [12, 36, 60, 84, 108, 132, 156] {
                let w = window("week", None, used, Duration::hours(hours), now);
                let alone = faults(&[sub("a", vec![w.clone()])], now);
                let pooled = faults(&[sub("a", vec![w.clone()]), sub("b", vec![])], now);
                assert_eq!(early_run_out(&w, now).is_some(), !alone.is_empty(), "{used}% with {hours}h left");
                assert!(pooled.is_empty(), "the other account has no forecast data");
            }
        }
    }

    #[test]
    fn counts_down_like_the_dashboard() {
        assert_eq!(span(Duration::seconds(20)), "<1m");
        assert_eq!(span(Duration::minutes(45)), "45m");
        assert_eq!(span(Duration::minutes(134)), "2h 14m");
        assert_eq!(span(Duration::hours(79)), "3d 7h");
        assert_eq!(span(Duration::seconds(-5)), "<1m");
    }
}
