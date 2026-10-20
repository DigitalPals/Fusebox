//! Limits on named clients: which models and programs may use a key, how many requests
//! a minute, how many tokens a day, how much estimated spend a month, and until when.
//!
//! Request rates are counted here. Tokens and spend come from the usage database every
//! 30 seconds, plus what finished since, so a burst can't run far past a limit.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;
use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Utc};
use parking_lot::Mutex;
use serde::Serialize;

use crate::accounts::wildcard;
use crate::config::NamedClient;
use crate::state::App;
use crate::usage::{store::Store, types::Observation};

const REFRESH_EVERY: Duration = Duration::from_secs(30);
const MAX_AGE: Duration = Duration::from_secs(90);

#[derive(Clone, Default)]
pub struct Clients {
    /// Requests in the current minute (UTC minute number, count).
    rate: Arc<Mutex<HashMap<String, (i64, u32)>>>,
    spend: Arc<Mutex<Spends>>,
    /// When each client key was last used since Fusebox started, by its usage id: a
    /// named client's id or a shared key's hash. Only accepted keys get here.
    seen: Arc<Mutex<HashMap<String, DateTime<Utc>>>>,
    store: Option<Store>,
}

#[derive(Default)]
struct Spends {
    /// The day and month the totals are for; a new one starts from zero.
    day: Option<NaiveDate>,
    by_client: HashMap<String, Totals>,
    /// Completions not yet acknowledged by a database snapshot.
    pending: HashMap<String, Pending>,
    refreshed: Option<Instant>,
    available: bool,
}

#[derive(Default, Clone, Copy)]
struct Totals {
    tokens_today: u64,
    cost_nanos: u64,
    unknown_tokens: u64,
    unpriced: u64,
}

impl Totals {
    fn add(&mut self, other: Self) {
        self.tokens_today = self.tokens_today.saturating_add(other.tokens_today);
        self.cost_nanos = self.cost_nanos.saturating_add(other.cost_nanos);
        self.unknown_tokens = self.unknown_tokens.saturating_add(other.unknown_tokens);
        self.unpriced = self.unpriced.saturating_add(other.unpriced);
    }

    fn spend(self) -> Spend {
        Spend { tokens_today: self.tokens_today, usd_month: self.cost_nanos as f64 / 1e9 }
    }
}

struct Pending {
    origin: String,
    client: String,
    event_at_ms: i64,
    totals: Totals,
}

impl Spends {
    /// A period change requires a fresh database snapshot before budget admission.
    fn roll(&mut self) {
        let today = Local::now().date_naive();
        match self.day {
            Some(day) if day == today => {}
            Some(day) => {
                let new_month = (day.year(), day.month()) != (today.year(), today.month());
                for v in self.by_client.values_mut() {
                    v.tokens_today = 0;
                    v.unknown_tokens = 0;
                    if new_month {
                        v.cost_nanos = 0;
                        v.unpriced = 0;
                    }
                }
                self.day = Some(today);
                self.available = false;
            }
            None => self.day = Some(today),
        }
    }

    fn ready(&self) -> bool {
        self.available && self.refreshed.is_some_and(|at| at.elapsed() < MAX_AGE)
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

/// Whether a named client's address list lets this request in; the source comes from
/// `source::layer`. A list with no source to check refuses.
pub fn address_allowed(client: &NamedClient, headers: &HeaderMap) -> bool {
    crate::source::allowed(&client.addresses, crate::source::Source::from_headers(headers).map(|s| s.ip))
}

impl Clients {
    pub fn new(store: Option<Store>) -> Self {
        Self { store, ..Default::default() }
    }
    /// Checks made before the request body is read: expiry, programs, rate and budgets.
    /// `count` is false for a WebSocket's opening request, whose turns count instead.
    pub fn admit(&self, client: &NamedClient, headers: &HeaderMap, count: bool) -> Result<(), Denied> {
        let now = Local::now();
        if let Some(last) = client.expires
            && now.date_naive() > last
        {
            return Err(denied(401, format!("this key expired on {last}")));
        }
        if !address_allowed(client, headers) {
            let from = crate::source::Source::from_headers(headers).map(|s| s.ip.to_string());
            return Err(denied(403, format!("this key doesn't work from {}", from.as_deref().unwrap_or("here"))));
        }
        if !client.user_agents.is_empty() {
            let agent = headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or_default();
            if !client.user_agents.iter().any(|p| wildcard(p, agent)) {
                return Err(denied(403, format!("this key only works from {}", client.user_agents.join(", "))));
            }
        }
        let spend = {
            let mut s = self.spend.lock();
            s.roll();
            let totals = s.by_client.get(&client.id).copied().unwrap_or_default();
            if (client.tokens_per_day.is_some() || client.usd_per_month.is_some())
                && (!s.ready() || self.store.as_ref().is_some_and(|store| !store.accounting_available()))
                || client.tokens_per_day.is_some() && totals.unknown_tokens > 0
                || client.usd_per_month.is_some() && totals.unpriced > 0
            {
                return Err(Denied {
                    status: 503,
                    message: "usage accounting is unavailable or incomplete; this key's budget cannot be checked"
                        .into(),
                    retry_after: Some(REFRESH_EVERY.as_secs()),
                });
            }
            totals.spend()
        };
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
        s.by_client.get(id).copied().unwrap_or_default().spend()
    }

    /// Notes that a client key was used just now.
    pub fn touch(&self, id: &str) {
        let now = Utc::now();
        let mut seen = self.seen.lock();
        match seen.get_mut(id) {
            Some(at) => *at = now,
            None => {
                seen.insert(id.to_string(), now);
            }
        }
    }

    pub fn last_used(&self, id: &str) -> Option<DateTime<Utc>> {
        self.seen.lock().get(id).copied()
    }

    /// Counts each provider attempt, including partial work before a retry.
    /// Called before its observation enters the writer queue.
    pub fn record(&self, o: &Observation, store: Option<&Store>) {
        let Some(id) = &o.client_id else { return };
        // A WebSocket is admitted once; each turn that finishes shows it is still in use.
        self.touch(id);
        let now = Local::now();
        let (day_ms, month_ms) = period_starts(now);
        if o.event_at_ms < month_ms {
            return;
        }
        let t = &o.tokens;
        let tokens =
            counted_tokens(t.input.unwrap_or(0).saturating_add(t.cache_write.unwrap_or(0)), t.output.unwrap_or(0));
        let chargeable = o.status.is_some_and(|s| s < 400) || tokens > 0 || t.cache_read.unwrap_or(0) > 0;
        let price = store.and_then(|s| s.price(o).cost_nanos).map(|n| n.max(0) as u64);
        let totals = Totals {
            tokens_today: if o.event_at_ms >= day_ms { tokens } else { 0 },
            cost_nanos: price.unwrap_or(0),
            unknown_tokens: u64::from(
                o.event_at_ms >= day_ms
                    && chargeable
                    && (t.input.is_none() || t.cache_write.is_none() || t.output.is_none()),
            ),
            unpriced: u64::from(chargeable && price.is_none()),
        };
        let mut s = self.spend.lock();
        s.roll();
        s.by_client.entry(id.clone()).or_default().add(totals);
        if store.is_some_and(Store::accounting_available) {
            s.pending.insert(
                o.source_event_id.clone(),
                Pending { origin: o.origin_id.clone(), client: id.clone(), event_at_ms: o.event_at_ms, totals },
            );
        }
    }

    fn unavailable(&self) {
        self.spend.lock().available = false;
    }

    /// Runs on the database worker. The lock covers both queries and replacement;
    /// record-before-enqueue prevents concurrent completions from being lost or doubled.
    fn reconcile(&self, conn: &mut rusqlite::Connection) -> anyhow::Result<()> {
        let mut s = self.spend.lock();
        let now = Local::now();
        let (day_ms, month_ms) = period_starts(now);
        let tx = conn.transaction()?;
        let mut totals = {
            let mut stmt = tx.prepare(
                "SELECT client_id,
                   SUM(CASE WHEN event_at_ms >= ?1 THEN COALESCE(input,0)+COALESCE(cache_write,0)+COALESCE(output,0) ELSE 0 END),
                   SUM(COALESCE(cost_nanos,0)),
                   SUM(CASE WHEN event_at_ms >= ?1 AND (input IS NULL OR cache_write IS NULL OR output IS NULL)
                       AND (COALESCE(input,0)+COALESCE(cache_read,0)+COALESCE(cache_write,0)+COALESCE(output,0)>0 OR json_extract(usage_document(payload),'$.status') < 400) THEN 1 ELSE 0 END),
                   SUM(CASE WHEN cost_nanos IS NULL AND (COALESCE(input,0)+COALESCE(cache_read,0)+COALESCE(cache_write,0)+COALESCE(output,0)>0 OR json_extract(usage_document(payload),'$.status') < 400) THEN 1 ELSE 0 END)
                 FROM usage_observations
                 WHERE source = 'proxy' AND client_id IS NOT NULL AND event_at_ms >= ?2
                 GROUP BY client_id",
            )?;
            let rows = stmt.query_map([day_ms, month_ms], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    Totals {
                        tokens_today: r.get::<_, i64>(1)?.max(0) as u64,
                        cost_nanos: r.get::<_, i64>(2)?.max(0) as u64,
                        unknown_tokens: r.get::<_, i64>(3)?.max(0) as u64,
                        unpriced: r.get::<_, i64>(4)?.max(0) as u64,
                    },
                ))
            })?;
            rows.collect::<Result<HashMap<_, _>, _>>()?
        };
        let mut acknowledged = Vec::new();
        {
            let mut stmt = tx.prepare("SELECT EXISTS(SELECT 1 FROM usage_observations WHERE source='proxy' AND origin_id=?1 AND source_event_id=?2)")?;
            for (event, p) in &s.pending {
                if p.event_at_ms < month_ms || stmt.query_row([&p.origin, event], |r| r.get::<_, bool>(0))? {
                    acknowledged.push(event.clone());
                } else {
                    let mut delta = p.totals;
                    if p.event_at_ms < day_ms {
                        delta.tokens_today = 0;
                        delta.unknown_tokens = 0;
                    }
                    totals.entry(p.client.clone()).or_default().add(delta);
                }
            }
        }
        tx.commit()?;
        for event in acknowledged {
            s.pending.remove(&event);
        }
        s.day = Some(now.date_naive());
        s.by_client = totals;
        s.refreshed = Some(Instant::now());
        s.available = true;
        Ok(())
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
pub async fn refresh(app: &App) {
    if app.cfg().named_clients.is_empty() {
        return;
    }
    let Some(store) = app.usage.as_ref().filter(|s| s.accounting_available()) else {
        app.clients.unavailable();
        return;
    };
    let clients = app.clients.clone();
    let result = tokio::time::timeout(Duration::from_secs(10), store.call(move |conn| clients.reconcile(conn))).await;
    if !matches!(result, Ok(Ok(()))) || !store.accounting_available() {
        app.clients.unavailable();
        tracing::warn!("could not establish client usage totals; budget-limited keys are unavailable");
    }
}

pub async fn refresher(app: Arc<App>) {
    let mut tick = tokio::time::interval(REFRESH_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        refresh(&app).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(clients: &Clients, tokens: u64, nanos: u64) {
        let mut s = clients.spend.lock();
        s.day = Some(Local::now().date_naive());
        s.refreshed = Some(Instant::now());
        s.available = true;
        s.by_client.insert("laptop".into(), Totals { tokens_today: tokens, cost_nanos: nanos, ..Default::default() });
    }

    fn observation(input: u64) -> Observation {
        let mut o =
            Observation::new("proxy", uuid::Uuid::new_v4().to_string(), "openai", Utc::now().timestamp_millis());
        o.client_id = Some("laptop".into());
        o.actual_model = Some("gpt-6-astra".into());
        o.status = Some(200);
        o.completeness = "complete".into();
        o.tokens = crate::usage::types::Tokens {
            input: Some(input),
            cache_read: Some(0),
            cache_write: Some(0),
            output: Some(0),
            ..Default::default()
        };
        o
    }

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
        seed(&clients, 0, 0);
        clients.record(&observation(999), None);
        assert!(clients.admit(&c, &HeaderMap::new(), true).is_ok());
        clients.record(&observation(1), None);
        let refused = clients.admit(&c, &HeaderMap::new(), true).unwrap_err();
        assert_eq!(refused.status, 429);
        assert!(refused.message.contains("1000 tokens"));

        let budget = NamedClient { usd_per_month: Some(5.0), ..client() };
        seed(&clients, 0, 4_990_000_000);
        assert!(clients.admit(&budget, &HeaderMap::new(), true).is_ok());
        seed(&clients, 0, 5_000_000_000);
        assert_eq!(clients.admit(&budget, &HeaderMap::new(), true).unwrap_err().status, 429);
    }

    #[test]
    fn budgets_require_initialized_fresh_accounting() {
        let clients = Clients::default();
        for c in [
            NamedClient { tokens_per_day: Some(1000), ..client() },
            NamedClient { usd_per_month: Some(5.0), ..client() },
        ] {
            assert_eq!(clients.admit(&c, &HeaderMap::new(), true).unwrap_err().status, 503);
        }
        seed(&clients, 0, 0);
        let c = NamedClient { usd_per_month: Some(5.0), ..client() };
        assert!(clients.admit(&c, &HeaderMap::new(), true).is_ok());
        clients.spend.lock().refreshed = Some(Instant::now() - MAX_AGE);
        assert_eq!(clients.admit(&c, &HeaderMap::new(), true).unwrap_err().status, 503);
        seed(&clients, 0, 0);
        clients.unavailable();
        assert_eq!(clients.admit(&c, &HeaderMap::new(), true).unwrap_err().status, 503);
        assert!(clients.admit(&client(), &HeaderMap::new(), true).is_ok());
    }

    #[test]
    fn a_new_period_requires_a_snapshot_and_resets_skipped_months() {
        let clients = Clients::default();
        seed(&clients, 999, 5_000_000_000);
        clients.spend.lock().day = Some(Local::now().date_naive().with_day(1).unwrap().pred_opt().unwrap());
        assert_eq!(clients.spend("laptop"), Spend::default());
        let c = NamedClient { usd_per_month: Some(5.0), ..client() };
        assert_eq!(clients.admit(&c, &HeaderMap::new(), true).unwrap_err().status, 503);
    }

    #[tokio::test]
    async fn reconciliation_preserves_completions_before_and_after_enqueue() {
        let dir = std::env::temp_dir().join(format!("fusebox-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("usage.sqlite3"), 64, 90, None).unwrap();
        let clients = Clients::new(Some(store.clone()));
        let a = observation(100);
        let b = observation(200);
        let refresh = || {
            let clients = clients.clone();
            store.call(move |conn| clients.reconcile(conn))
        };
        refresh().await.unwrap();
        clients.record(&a, Some(&store));
        let first = clients.spend("laptop");
        assert_eq!(first.tokens_today, 100);
        assert!(first.usd_month > 0.0);
        refresh().await.unwrap(); // The completion has not entered SQLite yet.
        assert_eq!(clients.spend("laptop"), first);
        assert!(store.enqueue(a));
        clients.record(&b, Some(&store));
        let both = clients.spend("laptop");
        refresh().await.unwrap(); // One committed row and one pending completion.
        assert_eq!(clients.spend("laptop"), both);
        assert!(store.enqueue(b));
        for _ in 0..3 {
            refresh().await.unwrap();
            assert_eq!(clients.spend("laptop"), both);
        }
        assert!(clients.spend.lock().pending.is_empty());
        store.shutdown().await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn budget_totals_read_compressed_records() {
        let dir = std::env::temp_dir().join(format!("fusebox-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("usage.sqlite3"), 64, 90, None).unwrap();
        // Enough records to train a dictionary, with failed and incomplete ones whose
        // status only the stored document holds.
        let records: Vec<Observation> = (0..1200)
            .map(|i| {
                let mut o = observation(if i % 3 == 0 { 0 } else { 100 });
                if i % 3 == 0 {
                    o.tokens.output = None;
                    o.status = Some(if i % 2 == 0 { 500 } else { 200 });
                }
                o
            })
            .collect();
        store.call(move |c| crate::usage::store::insert_batch(c, &records).map(|_| ())).await.unwrap();
        let clients = Clients::new(Some(store.clone()));
        let totals = || {
            let (clients, store) = (clients.clone(), store.clone());
            async move {
                let writer = clients.clone();
                store.call(move |conn| writer.reconcile(conn)).await.unwrap();
                let t = clients.spend.lock().by_client["laptop"];
                (t.tokens_today, t.cost_nanos, t.unknown_tokens, t.unpriced)
            }
        };
        let before = totals().await;
        assert_eq!(before.0, 800 * 100);
        assert_eq!(before.2, 200, "only the incomplete records that succeeded");
        let mut compressed = false;
        for _ in 0..200 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let text: i64 = store
                .call(|c| {
                    Ok(c.query_row("SELECT COUNT(*) FROM usage_observations WHERE typeof(payload)='text'", [], |r| {
                        r.get(0)
                    })?)
                })
                .await
                .unwrap();
            if text == 0 {
                compressed = true;
                break;
            }
        }
        assert!(compressed, "the writer did not compress the records");
        assert_eq!(totals().await, before);
        store.shutdown().await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
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
