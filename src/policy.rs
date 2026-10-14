//! Limits on named clients: which models and programs may use a key, how many requests
//! a minute, how many tokens a day, how much estimated spend a month, and until when.
//!
//! Request rates are counted here. Tokens and spend come from the usage database every
//! 30 seconds, plus what finished since, so a burst can't run far past a limit.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderMap;
use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Utc};
use parking_lot::Mutex;
use serde::Serialize;

use crate::accounts::wildcard;
use crate::config::NamedClient;
use crate::state::App;

const REFRESH_EVERY: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct Clients {
    /// Requests in the current minute (UTC minute number, count).
    rate: Mutex<HashMap<String, (i64, u32)>>,
    spend: Mutex<Spends>,
}

#[derive(Default)]
struct Spends {
    /// The day and month the totals are for; a new one starts from zero.
    day: Option<NaiveDate>,
    by_client: HashMap<String, Spend>,
}

impl Spends {
    /// On a new day, today's tokens start from zero (and the month's spend on the 1st)
    /// until the next refresh reads the database.
    fn roll(&mut self) {
        let today = Local::now().date_naive();
        match self.day {
            Some(day) if day == today => {}
            Some(_) => {
                let new_month = today.day() == 1;
                for v in self.by_client.values_mut() {
                    v.tokens_today = 0;
                    if new_month {
                        v.usd_month = 0.0;
                    }
                }
                self.day = Some(today);
            }
            None => self.day = Some(today),
        }
    }
}

/// What a client used today (tokens) and this month (estimated USD), server time.
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize)]
pub struct Spend {
    pub tokens_today: u64,
    pub usd_month: f64,
}

/// Why a request was refused, as the client should hear it.
#[derive(Debug, PartialEq)]
pub struct Denied {
    pub status: u16,
    pub message: String,
    pub retry_after: Option<u64>,
}

fn denied(status: u16, message: String) -> Denied {
    Denied { status, message, retry_after: None }
}

/// Starts of today and of this month in the server's time zone, as UTC milliseconds.
fn period_starts(now: DateTime<Local>) -> (i64, i64) {
    let start = |date: NaiveDate| {
        Local.from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap()).earliest().map_or(0, |t| t.timestamp_millis())
    };
    let today = now.date_naive();
    (start(today), start(today.with_day(1).unwrap_or(today)))
}

/// Tokens a limit counts: everything but cache reads, which cost a tenth or less.
pub fn counted_tokens(input_and_writes: u64, output: u64) -> u64 {
    input_and_writes.saturating_add(output)
}

impl Clients {
    /// Checks made before the request body is read: expiry, programs, rate and budgets.
    /// `count` is false for a WebSocket's opening request, whose turns count instead.
    pub fn admit(&self, client: &NamedClient, headers: &HeaderMap, count: bool) -> Result<(), Denied> {
        let now = Local::now();
        if let Some(last) = client.expires
            && now.date_naive() > last
        {
            return Err(denied(401, format!("this key expired on {last}")));
        }
        if !client.user_agents.is_empty() {
            let agent = headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or_default();
            if !client.user_agents.iter().any(|p| wildcard(p, agent)) {
                return Err(denied(403, format!("this key only works from {}", client.user_agents.join(", "))));
            }
        }
        let spend = self.spend(&client.id);
        if let Some(limit) = client.tokens_per_day
            && spend.tokens_today >= limit
        {
            return Err(denied(429, format!("this key used its {limit} tokens for today; it resets at midnight")));
        }
        if let Some(limit) = client.usd_per_month
            && spend.usd_month >= limit
        {
            return Err(denied(429, format!("this key reached its ${limit} estimated budget for this month")));
        }
        if let Some(limit) = client.requests_per_minute.filter(|_| count) {
            let minute = Utc::now().timestamp().div_euclid(60);
            let mut rate = self.rate.lock();
            rate.retain(|_, (m, _)| *m == minute);
            let (_, count) = rate.entry(client.id.clone()).or_insert((minute, 0));
            if *count >= limit {
                let wait = 60 - Utc::now().timestamp().rem_euclid(60) as u64;
                return Err(Denied {
                    status: 429,
                    message: format!("this key is limited to {limit} requests a minute"),
                    retry_after: Some(wait.max(1)),
                });
            }
            *count += 1;
        }
        Ok(())
    }

    pub fn spend(&self, id: &str) -> Spend {
        let mut s = self.spend.lock();
        s.roll();
        s.by_client.get(id).copied().unwrap_or_default()
    }

    /// Counts a finished request until the next refresh includes it.
    pub fn add(&self, id: &str, tokens: u64) {
        if tokens > 0 {
            let mut s = self.spend.lock();
            s.roll();
            let entry = s.by_client.entry(id.to_string()).or_default();
            entry.tokens_today = entry.tokens_today.saturating_add(tokens);
        }
    }

    fn replace(&self, day: NaiveDate, totals: HashMap<String, Spend>) {
        let mut s = self.spend.lock();
        s.day = Some(day);
        s.by_client = totals;
    }
}

/// Whether a client may ask for `model` (`*` wildcards; no patterns allows every model).
pub fn model_allowed(client: &NamedClient, model: &str) -> bool {
    client.models.is_empty() || client.models.iter().any(|p| wildcard(p, model))
}

/// The named client a request came from, as `client_auth` marked it.
pub fn named_client<'a>(cfg: &'a crate::config::Config, headers: &HeaderMap) -> Option<&'a NamedClient> {
    let id = headers.get(crate::usage::capture::CLIENT_HEADER)?.to_str().ok()?;
    cfg.named_clients.iter().find(|c| c.id == id)
}

/// Refuses a model the client's key may not use.
pub fn check_model(app: &App, headers: &HeaderMap, model: &str) -> Result<(), Denied> {
    let cfg = app.cfg();
    match named_client(&cfg, headers) {
        Some(c) if !model_allowed(c, model) => {
            Err(denied(403, format!("this key can't use {model}; it may use {}", c.models.join(", "))))
        }
        _ => Ok(()),
    }
}

/// Keeps token and spend totals current for clients that have such limits.
pub async fn refresher(app: Arc<App>) {
    let mut tick = tokio::time::interval(REFRESH_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let limited = app.cfg().named_clients.iter().any(|c| c.tokens_per_day.is_some() || c.usd_per_month.is_some());
        let Some(store) = app.usage.clone().filter(|_| limited) else { continue };
        let now = Local::now();
        let (day_ms, month_ms) = period_starts(now);
        let result = store
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT client_id,
                       SUM(CASE WHEN event_at_ms >= ?1 THEN COALESCE(input,0)+COALESCE(cache_write,0)+COALESCE(output,0) ELSE 0 END),
                       SUM(COALESCE(cost_nanos,0))
                     FROM usage_observations
                     WHERE source = 'proxy' AND client_id IS NOT NULL AND event_at_ms >= ?2
                     GROUP BY client_id",
                )?;
                let rows = stmt.query_map([day_ms, month_ms], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
                })?;
                let mut out = HashMap::new();
                for row in rows {
                    let (id, tokens, nanos) = row?;
                    out.insert(
                        id,
                        Spend { tokens_today: tokens.max(0) as u64, usd_month: nanos.max(0) as f64 / 1e9 },
                    );
                }
                Ok(out)
            })
            .await;
        match result {
            Ok(totals) => app.clients.replace(now.date_naive(), totals),
            Err(e) => tracing::warn!("could not read client usage for limits: {e:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> NamedClient {
        NamedClient { id: "laptop".into(), label: "Laptop".into(), key: "fbx_k".into(), ..Default::default() }
    }

    fn agent(ua: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("user-agent", ua.parse().unwrap());
        h
    }

    #[test]
    fn a_client_without_limits_is_always_admitted() {
        let clients = Clients::default();
        for _ in 0..100 {
            assert_eq!(clients.admit(&client(), &HeaderMap::new(), true), Ok(()));
        }
        assert!(model_allowed(&client(), "anything"));
    }

    #[test]
    fn requests_per_minute_refuse_the_one_over_the_limit() {
        let clients = Clients::default();
        let c = NamedClient { requests_per_minute: Some(2), ..client() };
        assert!(clients.admit(&c, &HeaderMap::new(), true).is_ok());
        assert!(clients.admit(&c, &HeaderMap::new(), true).is_ok());
        let refused = clients.admit(&c, &HeaderMap::new(), true).unwrap_err();
        assert_eq!(refused.status, 429);
        assert!(refused.retry_after.is_some_and(|s| (1..=60).contains(&s)));
        // Another client has its own count.
        assert!(clients.admit(&NamedClient { id: "other".into(), ..c.clone() }, &HeaderMap::new(), true).is_ok());
    }

    #[test]
    fn expired_keys_and_other_programs_are_refused() {
        let clients = Clients::default();
        let yesterday = Local::now().date_naive().pred_opt().unwrap();
        let expired = NamedClient { expires: Some(yesterday), ..client() };
        assert_eq!(clients.admit(&expired, &HeaderMap::new(), true).unwrap_err().status, 401);
        let today = NamedClient { expires: Some(Local::now().date_naive()), ..client() };
        assert!(clients.admit(&today, &HeaderMap::new(), true).is_ok());

        let cli = NamedClient { user_agents: vec!["claude-cli/*".into()], ..client() };
        assert!(clients.admit(&cli, &agent("claude-cli/2.1.296 (external, cli)"), true).is_ok());
        assert_eq!(clients.admit(&cli, &agent("curl/8.9"), true).unwrap_err().status, 403);
        assert_eq!(clients.admit(&cli, &HeaderMap::new(), true).unwrap_err().status, 403);
    }

    #[test]
    fn token_and_spend_limits_use_the_counted_totals() {
        let clients = Clients::default();
        let c = NamedClient { tokens_per_day: Some(1000), ..client() };
        clients.add("laptop", 999);
        assert!(clients.admit(&c, &HeaderMap::new(), true).is_ok());
        clients.add("laptop", 1);
        let refused = clients.admit(&c, &HeaderMap::new(), true).unwrap_err();
        assert_eq!(refused.status, 429);
        assert!(refused.message.contains("1000 tokens"));

        let budget = NamedClient { usd_per_month: Some(5.0), ..client() };
        let today = Local::now().date_naive();
        clients.replace(today, HashMap::from([("laptop".into(), Spend { tokens_today: 0, usd_month: 4.99 })]));
        assert!(clients.admit(&budget, &HeaderMap::new(), true).is_ok());
        clients.replace(today, HashMap::from([("laptop".into(), Spend { tokens_today: 0, usd_month: 5.0 })]));
        assert_eq!(clients.admit(&budget, &HeaderMap::new(), true).unwrap_err().status, 429);
    }

    #[test]
    fn model_patterns_allow_matching_models_only() {
        let c = NamedClient { models: vec!["claude-*".into(), "gpt-6-astra".into()], ..client() };
        assert!(model_allowed(&c, "claude-sonnet-5-5"));
        assert!(model_allowed(&c, "GPT-6-ASTRA"));
        assert!(!model_allowed(&c, "gpt-6.1-sol"));
    }

    #[test]
    fn periods_start_at_local_midnight() {
        let now = Local.with_ymd_and_hms(2026, 10, 13, 15, 30, 0).unwrap();
        let (day, month) = period_starts(now);
        assert_eq!(Local.timestamp_millis_opt(day).unwrap().naive_local().to_string(), "2026-10-13 00:00:00");
        assert_eq!(Local.timestamp_millis_opt(month).unwrap().naive_local().to_string(), "2026-10-01 00:00:00");
    }
}
