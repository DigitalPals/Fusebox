use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::accounts::Pool;
use crate::config::Config;
use crate::ir::Usage;

pub struct App {
    cfg: ArcSwap<Config>,
    pub cfg_path: PathBuf,
    pub config_write: Mutex<()>,
    /// Startup settings remain separate from hot-reloaded settings.
    pub startup_config: Config,
    pub pool: Pool,
    pub sessions: Arc<crate::affinity::Sessions>,
    pub refresh_tasks: crate::oauth::RefreshTasks,
    pub http: Http,
    pub stats: Stats,
    pub embedding_cache: Mutex<crate::embeddings::Cache>,
    pub usage: Option<crate::usage::store::Store>,
    pub usage_error: Option<String>,
    pub logins: Mutex<HashMap<String, crate::mgmt::Login>>,
    pub reset_quotes: Mutex<HashMap<String, crate::banked_resets::Quote>>,
    pub push: crate::push::Push,
    /// Model lists fetched from Claude and Codex, and removed dashboard additions.
    pub models: crate::discovery::Store,
    #[cfg(test)]
    pub reset_test_origin: Mutex<Option<String>>,
    #[cfg(test)]
    pub window_starter_test_origin: Mutex<Option<String>>,
    pub started: DateTime<Utc>,
    pub live: broadcast::Sender<String>,
    /// Hash of the last model list dashboards were told about.
    pub models_fingerprint: std::sync::atomic::AtomicU64,
    /// The dashboard ticker starts with the first dashboard connection.
    pub live_ticker: std::sync::atomic::AtomicBool,
    /// Ignore our own writes to the auth dir in the file watcher.
    quiet_until: AtomicI64,
    /// Fired when the server starts shutting down, so long-lived sockets can close cleanly.
    pub shutdown: tokio_util::sync::CancellationToken,
    /// Named clients' request rates and spend, for their limits.
    pub clients: crate::policy::Clients,
    pub admission: Arc<crate::admission::Admission>,
    /// Wrong management keys per address, for the lockout.
    pub key_guesses: crate::mgmt::Guesses,
    /// Where client keys are used from, and what is connected now.
    pub addresses: Arc<crate::addresses::Addresses>,
}

impl App {
    pub fn new(cfg: Config, cfg_path: PathBuf) -> Arc<Self> {
        let pool = Pool::default();
        pool.reload(&cfg);
        pool.restore_cooldowns(&crate::accounts::Cooldowns::load(
            &cfg.auth_dir().join(crate::accounts::COOLDOWNS_FILE),
        ));
        pool.restore_attributions(crate::attribution::Saved::load(&cfg.auth_dir().join(crate::attribution::FILE)));
        let (live, _) = broadcast::channel(512);
        let sessions = Arc::new(crate::affinity::Sessions::load(&cfg.auth_dir(), cfg.session_affinity_idle_seconds));
        let push = crate::push::Push::load(&cfg.auth_dir());
        let addresses = Arc::new(crate::addresses::Addresses::load(&cfg.auth_dir()));
        let models = crate::discovery::Store::load(&cfg.auth_dir());
        models.apply(&pool);
        // Tests opt in with an explicit isolated database; never touch a user's default DB.
        let usage_enabled = cfg.usage.enabled && (!cfg!(test) || cfg.usage.database.is_some());
        let (usage, usage_error) = if usage_enabled {
            match crate::usage::store::Store::open(
                &crate::usage::database_path(&cfg, &cfg_path),
                cfg.usage.queue_capacity,
                cfg.usage.retention_days,
                cfg.usage.pricing_overrides.as_deref().map(std::path::Path::new),
            ) {
                Ok(store) => (Some(store), None),
                Err(_) => {
                    tracing::error!("usage database unavailable; proxy remains available with an analytics gap");
                    (
                        None,
                        Some("Usage database unavailable; check file access, disk space and migration version".into()),
                    )
                }
            }
        } else {
            (None, None)
        };
        // Read the system's TLS roots now, not on the first request's worker thread.
        crate::tls::warm();
        let clients = crate::policy::Clients::new(usage.clone());
        crate::clients::configure(&cfg.client_identities);
        Arc::new(Self {
            usage,
            usage_error,
            http: Http::new(&cfg.proxy_url),
            startup_config: cfg.clone(),
            cfg: ArcSwap::from_pointee(cfg),
            cfg_path,
            config_write: Mutex::new(()),
            pool,
            sessions,
            refresh_tasks: Default::default(),
            stats: Stats::default(),
            embedding_cache: Default::default(),
            logins: Mutex::new(HashMap::new()),
            reset_quotes: Mutex::new(HashMap::new()),
            push,
            models,
            #[cfg(test)]
            reset_test_origin: Mutex::new(None),
            #[cfg(test)]
            window_starter_test_origin: Mutex::new(None),
            started: Utc::now(),
            live,
            models_fingerprint: Default::default(),
            live_ticker: Default::default(),
            quiet_until: AtomicI64::new(0),
            shutdown: Default::default(),
            clients,
            admission: Default::default(),
            key_guesses: Default::default(),
            addresses,
        })
    }

    pub fn cfg(&self) -> Arc<Config> {
        self.cfg.load_full()
    }

    pub fn set_config(&self, cfg: Config) {
        let reset_embedding_cache = self.cfg().embeddings != cfg.embeddings;
        crate::clients::configure(&cfg.client_identities);
        self.http.set_default_proxy(&cfg.proxy_url);
        self.pool.reload(&cfg);
        self.models.apply(&self.pool);
        self.retain_account_pools();
        self.cfg.store(Arc::new(cfg));
        if reset_embedding_cache {
            self.embedding_cache.lock().clear();
        }
        self.broadcast("accounts", serde_json::Value::Null);
    }

    pub fn reload_accounts(&self) {
        self.pool.reload(&self.cfg());
        self.models.apply(&self.pool);
        self.retain_account_pools();
        self.broadcast("accounts", serde_json::Value::Null);
    }

    fn retain_account_pools(&self) {
        self.http.retain_accounts(&self.pool.all());
    }

    pub fn suppress_reload(&self) {
        self.quiet_until.store(Utc::now().timestamp() + 3, Ordering::Relaxed);
    }

    pub fn reload_suppressed(&self) -> bool {
        Utc::now().timestamp() < self.quiet_until.load(Ordering::Relaxed)
    }

    pub fn broadcast(&self, kind: &str, data: impl Serialize) {
        if self.live.receiver_count() > 0 {
            let msg = serde_json::json!({ "type": kind, "data": data }).to_string();
            let _ = self.live.send(msg);
            // Every path that changes the model list (config, sign-ins, discovery)
            // already announces account changes; tell dashboards about models only
            // when the list they read actually changed.
            if kind == "accounts" {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                crate::mgmt::model_list(self).to_string().hash(&mut hasher);
                let fingerprint = hasher.finish();
                if self.models_fingerprint.swap(fingerprint, Ordering::Relaxed) != fingerprint {
                    let _ = self.live.send(r#"{"type":"models","data":null}"#.to_string());
                }
            }
        }
    }
}

// ------------------------------------------------------------------------ http

pub struct Http {
    default_proxy: Mutex<String>,
    clients: Mutex<HashMap<ClientPool, reqwest::Client>>,
    account_tls: Mutex<HashMap<AccountPool, Arc<crate::tls::AccountTls>>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct AccountPool {
    id: String,
    identity: String,
    epoch: u64,
}

impl AccountPool {
    fn new(acct: &crate::accounts::Account) -> Self {
        // An invalidated old allocation still holds its original credentials.
        // Read separately: snapshot/publication take state before credentials.
        let identity = acct.cred.read().identity(acct.provider);
        let epoch = acct.state.lock().identity_epoch;
        Self { id: acct.id.clone(), identity, epoch }
    }
}

#[derive(PartialEq, Eq, Hash)]
struct ClientPool {
    proxy: String,
    account: Option<AccountPool>,
    http1: bool,
    reset: bool,
    control: bool,
}

/// How long a TCP connection may be silent before the OS checks it is still alive,
/// then how often it asks and how many unanswered checks close it. A connection
/// dropped by a NAT or load balancer is found in about a minute instead of waiting
/// for the next request to hang until the read timeout.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const KEEPALIVE_RETRIES: u32 = 3;

impl Http {
    fn new(proxy: &str) -> Self {
        Self {
            default_proxy: Mutex::new(proxy.to_string()),
            clients: Mutex::new(HashMap::new()),
            account_tls: Mutex::new(HashMap::new()),
        }
    }

    fn set_default_proxy(&self, proxy: &str) {
        *self.default_proxy.lock() = proxy.to_string();
    }

    /// Client for the given proxy (falls back to the configured default).
    pub fn client(&self, proxy: Option<&str>) -> reqwest::Client {
        self.build(proxy, Sharing::Shared, false, false)
    }

    /// Short, bounded requests for credentials, discovery and usage data.
    pub fn control(&self, proxy: Option<&str>) -> reqwest::Client {
        self.build(proxy, Sharing::Shared, false, true)
    }

    pub fn control_for_account(&self, acct: &crate::accounts::Account) -> reqwest::Client {
        self.build(acct.proxy_url.as_deref(), Sharing::for_account(acct), false, true)
    }

    /// Spending requests must never follow redirects or retry in the HTTP layer.
    #[cfg(test)]
    pub fn for_reset(&self, proxy: Option<&str>) -> reqwest::Client {
        self.build(proxy, Sharing::Shared, true, false)
    }

    pub fn reset_for_account(&self, acct: &crate::accounts::Account) -> reqwest::Client {
        self.build(acct.proxy_url.as_deref(), Sharing::for_account(acct), true, false)
    }

    /// Each signed-in subscription gets its own connections, as its own CLI would:
    /// requests for different accounts never share one HTTP/2 connection. Antigravity
    /// accounts use HTTP/1.1, as the IDE does; Google's backend treats shared HTTP/2
    /// connections less kindly. API keys share a pool per proxy.
    pub fn for_account(&self, acct: &crate::accounts::Account) -> reqwest::Client {
        self.build(acct.proxy_url.as_deref(), Sharing::for_account(acct), false, false)
    }

    /// WebSocket TLS uses the same account-owned tickets as its HTTP clients.
    pub fn websocket_tls_for_account(&self, acct: &crate::accounts::Account) -> Arc<rustls::ClientConfig> {
        self.tls_for_account(&AccountPool::new(acct)).websocket()
    }

    fn tls_for_account(&self, account: &AccountPool) -> Arc<crate::tls::AccountTls> {
        let mut stores = self.account_tls.lock();
        stores.retain(|key, _| key.id != account.id || key == account);
        stores.entry(account.clone()).or_insert_with(|| Arc::new(crate::tls::AccountTls::new())).clone()
    }

    /// Drop removed accounts and obsolete identities, including TLS tickets.
    pub fn retain_accounts(&self, accounts: &[Arc<crate::accounts::Account>]) {
        let default_proxy = self.default_proxy.lock().clone();
        let current: HashMap<_, _> = accounts
            .iter()
            .map(|a| {
                let proxy = a.proxy_url.as_deref().filter(|p| !p.is_empty()).unwrap_or(&default_proxy).to_string();
                (AccountPool::new(a), proxy)
            })
            .collect();
        self.clients.lock().retain(|key, _| {
            key.account.as_ref().is_none_or(|a| current.get(a).is_some_and(|proxy| proxy == &key.proxy))
        });
        self.account_tls.lock().retain(|key, _| current.contains_key(key));
    }

    fn build(&self, proxy: Option<&str>, pool: Sharing, reset: bool, control: bool) -> reqwest::Client {
        let proxy =
            proxy.filter(|p| !p.is_empty()).map(String::from).unwrap_or_else(|| self.default_proxy.lock().clone());
        let account = match pool {
            Sharing::Shared => None,
            Sharing::Account(acct) | Sharing::Http1(acct) => Some(AccountPool::new(acct)),
        };
        let http1 = matches!(pool, Sharing::Http1(_));
        let key = ClientPool { proxy: proxy.clone(), account, http1, reset, control };
        let mut clients = self.clients.lock();
        if let Some(c) = clients.get(&key) {
            return c.clone();
        }
        if let Some(account) = &key.account {
            clients.retain(|key, _| key.account.as_ref().is_none_or(|old| old.id != account.id || old == account));
        }
        let tls = match &key.account {
            Some(account) => self.tls_for_account(account).http(http1),
            None => crate::tls::http(http1),
        };
        let mut b = reqwest::Client::builder()
            .use_preconfigured_tls(tls)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(600))
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(KEEPALIVE_IDLE)
            .tcp_keepalive_interval(KEEPALIVE_INTERVAL)
            .tcp_keepalive_retries(KEEPALIVE_RETRIES);
        if control {
            b = b
                .connect_timeout(Duration::from_secs(10))
                .read_timeout(Duration::from_secs(30))
                .timeout(Duration::from_secs(30));
        }
        if reset {
            b = b.retry(reqwest::retry::never());
        }
        if proxy == "direct" || proxy == "none" {
            // CLIProxyAPI's spelling for "no proxy, not even the default one".
            b = b.no_proxy();
        } else if !proxy.is_empty() {
            match crate::config::check_proxy_url(&proxy)
                .and_then(|_| reqwest::Proxy::all(&proxy).map_err(|e| e.to_string()))
            {
                Ok(p) => b = b.proxy(p),
                Err(e) => {
                    // Never fall back to a direct connection: send everything to an
                    // address that can't resolve, so each request fails and says why.
                    tracing::error!("unusable proxy, so requests through it fail: {e}");
                    b = b.proxy(reqwest::Proxy::all("http://invalid-proxy-url.invalid:9").expect("static proxy"));
                }
            }
        }
        if http1 {
            b = b.http1_only().pool_idle_timeout(Duration::from_secs(200));
        }
        let c = b.build().expect("http client");
        clients.insert(key, c.clone());
        c
    }
}

/// Which connection pool a client draws from.
#[derive(Clone, Copy)]
enum Sharing<'a> {
    Shared,
    /// One account's own HTTP/2-capable pool.
    Account(&'a crate::accounts::Account),
    /// One account's own HTTP/1.1 pool.
    Http1(&'a crate::accounts::Account),
}

impl<'a> Sharing<'a> {
    fn for_account(acct: &'a crate::accounts::Account) -> Self {
        match acct.provider {
            crate::accounts::Provider::Antigravity => Self::Http1(acct),
            _ if acct.is_oauth() => Self::Account(acct),
            _ => Self::Shared,
        }
    }
}

// ----------------------------------------------------------------------- stats

#[derive(Debug, Clone, Serialize)]
pub struct RoutingAttempt {
    pub account_id: String,
    pub account: String,
    pub reason: &'static str,
    pub previous_account: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestLog {
    pub id: u64,
    pub ts: DateTime<Utc>,
    pub client: &'static str,
    /// The client program, when its User-Agent names one (Claude Code, Codex, an SDK).
    pub client_app: Option<&'static str>,
    /// Where the request came from; see `source`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::source::Source>,
    /// The client key's usage id (a named client's id, or a shared key's hash) and the
    /// name it goes by.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_label: Option<String>,
    pub provider: String,
    pub model: String,
    pub account: String,
    /// Id of the account that answered (empty when none was tried).
    pub account_id: String,
    /// Client-scoped fingerprint; never the client's raw session ID or API key.
    pub session_id: Option<String>,
    pub session_source: Option<&'static str>,
    pub routing_strategy: crate::config::Routing,
    pub routing_reason: Option<&'static str>,
    pub routing_warning: Option<&'static str>,
    pub routing_attempts: Vec<RoutingAttempt>,
    /// Requested and effective cache controls; contains no prompt text or keys.
    pub cache_policy: Option<crate::cache::Policy>,
    /// Sanitized provider-reported cache outcome; absence is not a cache hit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_diagnostics: Option<crate::cache::Diagnostics>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub control_downgrades: Vec<&'static str>,
    pub status: u16,
    pub latency_ms: u64,
    pub ttft_ms: Option<u64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
    /// Whether provider usage is final, an observed lower bound, or absent.
    pub usage_completeness: &'static str,
    pub failure_kind: Option<&'static str>,
    pub stream: bool,
    pub transport: &'static str,
    pub attempts: u32,
    /// Planned provider chunks are distinct from account retries.
    pub chunks: u32,
    pub error: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Totals {
    pub requests: u64,
    pub ok: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub usage_missing: u64,
    pub usage_partial: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Bucket {
    pub minute: i64,
    pub requests: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub usage_missing: u64,
    pub usage_partial: u64,
    pub tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
}

impl Bucket {
    fn add(&mut self, log: &RequestLog) {
        self.requests += 1;
        if log.status == 499 {
            self.cancelled += 1;
        } else if log.status >= 400 {
            self.failed += 1;
        }
        self.usage_missing += u64::from(log.usage_completeness == "missing");
        self.usage_partial += u64::from(log.usage_completeness == "partial");
        self.tokens += log.input_tokens + log.output_tokens + log.cache_tokens;
        self.input_tokens += log.input_tokens;
        self.output_tokens += log.output_tokens;
        self.cache_tokens += log.cache_tokens;
    }
}

/// Adds a request to the minute it belongs to, keeping the last hour.
fn add_to_minute(series: &mut VecDeque<Bucket>, log: &RequestLog) {
    let minute = log.ts.timestamp() / 60;
    let cutoff = Utc::now().timestamp() / 60 - MINUTES as i64 + 1;
    series.retain(|b| b.minute >= cutoff);
    if minute < cutoff {
        return;
    }
    let index = series.partition_point(|b| b.minute < minute);
    if series.get(index).is_none_or(|b| b.minute != minute) {
        series.insert(index, Bucket { minute, ..Default::default() });
    }
    series[index].add(log);
}

/// The last 60 minutes, oldest first, with empty minutes filled in. The series is
/// kept sorted by minute, so one forward pass fills the hour.
fn last_hour(series: &VecDeque<Bucket>) -> Vec<Bucket> {
    let now = Utc::now().timestamp() / 60;
    let mut stored = series.iter().peekable();
    (now - MINUTES as i64 + 1..=now)
        .map(|m| {
            while stored.next_if(|b| b.minute < m).is_some() {}
            match stored.next_if(|b| b.minute == m) {
                Some(b) => b.clone(),
                None => Bucket { minute: m, ..Default::default() },
            }
        })
        .collect()
}

#[derive(Default)]
pub struct Stats {
    pub performance: Mutex<crate::metrics::performance::Performance>,
    pub totals: Mutex<Totals>,
    pub recent: Mutex<VecDeque<RequestLog>>,
    pub series: Mutex<VecDeque<Bucket>>,
    /// The same minutes per account id.
    accounts: Mutex<HashMap<String, VecDeque<Bucket>>>,
    pub active: std::sync::atomic::AtomicU64,
    next_id: std::sync::atomic::AtomicU64,
}

const RECENT: usize = 300;
const MINUTES: usize = 60;

impl Stats {
    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn record(&self, log: &RequestLog) {
        {
            let mut t = self.totals.lock();
            t.requests += 1;
            if log.status == 499 {
                t.cancelled += 1
            } else if log.status < 400 {
                t.ok += 1
            } else {
                t.failed += 1
            }
            t.usage_missing += u64::from(log.usage_completeness == "missing");
            t.usage_partial += u64::from(log.usage_completeness == "partial");
            t.input_tokens += log.input_tokens;
            t.output_tokens += log.output_tokens;
            t.cache_tokens += log.cache_tokens;
        }
        add_to_minute(&mut self.series.lock(), log);
        if !log.account_id.is_empty() {
            let mut accounts = self.accounts.lock();
            // Forget accounts that have been quiet for an hour (or were removed).
            let cutoff = Utc::now().timestamp() / 60 - MINUTES as i64;
            accounts.retain(|_, s| s.back().is_some_and(|b| b.minute > cutoff));
            add_to_minute(accounts.entry(log.account_id.clone()).or_default(), log);
        }
        let mut r = self.recent.lock();
        r.push_back(log.clone());
        while r.len() > RECENT {
            r.pop_front();
        }
    }

    pub fn series(&self) -> Vec<Bucket> {
        last_hour(&self.series.lock())
    }

    /// One account's last 60 minutes.
    pub fn account_series(&self, id: &str) -> Vec<Bucket> {
        last_hour(self.accounts.lock().get(id).unwrap_or(&VecDeque::new()))
    }

    /// Failed requests on one account in the last hour, without building the series.
    pub fn account_failures(&self, id: &str) -> u64 {
        let cutoff = Utc::now().timestamp() / 60 - MINUTES as i64 + 1;
        self.accounts.lock().get(id).map_or(0, |s| s.iter().filter(|b| b.minute >= cutoff).map(|b| b.failed).sum())
    }
}

pub fn usage_tokens(u: &Usage) -> (u64, u64, u64) {
    (u.input + u.cache_write, u.output, u.cache_read)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(minute: i64, tokens: u64) -> RequestLog {
        RequestLog {
            id: 0,
            ts: DateTime::from_timestamp(minute * 60, 0).unwrap(),
            client: "responses",
            client_app: None,
            source: None,
            key: None,
            key_label: None,
            provider: "codex".into(),
            model: "test".into(),
            account: "test".into(),
            account_id: "account".into(),
            session_id: None,
            session_source: None,
            routing_strategy: crate::config::Routing::RoundRobin,
            routing_reason: None,
            routing_warning: None,
            routing_attempts: vec![],
            cache_policy: None,
            cache_diagnostics: None,
            control_downgrades: Vec::new(),
            status: 200,
            latency_ms: 0,
            ttft_ms: None,
            input_tokens: tokens,
            output_tokens: 2,
            cache_tokens: 3,
            usage_completeness: "complete",
            failure_kind: None,
            stream: true,
            transport: "http",
            attempts: 1,
            chunks: 0,
            error: None,
        }
    }

    #[test]
    fn subscriptions_get_their_own_pools_and_removed_accounts_close_them() {
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            claude_api_key: ["one", "two", "three"]
                .map(|key| crate::config::KeyEntry { api_key: key.into(), ..Default::default() })
                .to_vec(),
            ..Default::default()
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        let accounts = pool.all();
        for a in &accounts[..2] {
            *a.cred.write() = crate::accounts::Credential::OAuth(crate::accounts::OAuth {
                access_token: "token".into(),
                ..Default::default()
            });
        }
        let http = Http::new("");
        let count = || http.clients.lock().len();
        http.for_account(&accounts[0]);
        http.for_account(&accounts[1]);
        assert_eq!(count(), 2, "each subscription has its own pool");
        http.for_account(&accounts[2]);
        http.client(None);
        assert_eq!(count(), 3, "API keys share the default pool");
        http.for_account(&accounts[0]);
        http.control_for_account(&accounts[0]);
        assert_eq!(count(), 4, "pools are reused; control requests belong to the account");
        assert_eq!(http.account_tls.lock().len(), 2);
        http.retain_accounts(&accounts[1..]);
        assert_eq!(count(), 2);
        assert!(http.clients.lock().keys().all(|k| k.account.as_ref().is_none_or(|a| a.id != accounts[0].id)));
        assert_eq!(http.account_tls.lock().len(), 1);
        http.retain_accounts(&[]);
        assert_eq!(count(), 1, "shared pools are never pruned");
        assert!(http.account_tls.lock().is_empty());
    }

    #[tokio::test]
    async fn provider_clients_never_follow_307_with_credentials_or_request_bodies() {
        use std::sync::atomic::AtomicUsize;
        let redirected = Arc::new(AtomicUsize::new(0));
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = axum::Router::new()
            .route(
                "/start",
                axum::routing::post({
                    let attempts = attempts.clone();
                    move |headers: axum::http::HeaderMap, body: String| {
                        let attempts = attempts.clone();
                        async move {
                            assert_eq!(headers["authorization"], "Bearer FAKE_ACCESS_MARKER");
                            assert_eq!(headers["x-api-key"], "FAKE_KEY_MARKER");
                            assert!(body.contains("FAKE_PROMPT_OR_REFRESH_MARKER"));
                            attempts.fetch_add(1, Ordering::Relaxed);
                            (
                                axum::http::StatusCode::TEMPORARY_REDIRECT,
                                [("location", "/target?secret=FAKE_REDIRECT_TARGET_MARKER")],
                            )
                        }
                    }
                }),
            )
            .route(
                "/target",
                axum::routing::any({
                    let redirected = redirected.clone();
                    move || {
                        let redirected = redirected.clone();
                        async move {
                            redirected.fetch_add(1, Ordering::Relaxed);
                            "unexpected"
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/start", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            claude_api_key: vec![crate::config::KeyEntry { api_key: "fake".into(), ..Default::default() }],
            ..Default::default()
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        let account = pool.all().pop().unwrap();
        *account.cred.write() = crate::accounts::Credential::OAuth(crate::accounts::OAuth {
            access_token: "fake".into(),
            account_id: Some("fake-account".into()),
            ..Default::default()
        });
        let http = Http::new("direct");
        let clients = [
            http.client(None),
            http.control(None),
            http.for_reset(None),
            http.for_account(&account),
            http.control_for_account(&account),
            http.reset_for_account(&account),
        ];
        for client in &clients {
            let response = client
                .post(&endpoint)
                .bearer_auth("FAKE_ACCESS_MARKER")
                .header("x-api-key", "FAKE_KEY_MARKER")
                .body("FAKE_PROMPT_OR_REFRESH_MARKER")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
        }
        assert_eq!(attempts.load(Ordering::Relaxed), clients.len());
        assert_eq!(redirected.load(Ordering::Relaxed), 0);
        server.abort();
    }

    #[tokio::test]
    async fn account_connections_and_tls_survive_rotation_but_expire_on_identity_change() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let router = axum::Router::new().route(
            "/",
            axum::routing::get(
                |axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>| async move {
                    peer.to_string()
                },
            ),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap()
        });
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            codex_api_key: ["one", "two"]
                .map(|key| crate::config::KeyEntry { api_key: key.into(), ..Default::default() })
                .to_vec(),
            ..Default::default()
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        let accounts = pool.all();
        for (i, account) in accounts.iter().enumerate() {
            crate::oauth::publish_refreshed(
                account,
                crate::accounts::OAuth {
                    access_token: "FAKE_ACCESS".into(),
                    account_id: Some(format!("fake-workspace-{i}")),
                    ..Default::default()
                },
                vec![],
            );
        }
        let http = Http::new("direct");
        let peer = |client: reqwest::Client| {
            let endpoint = endpoint.clone();
            async move { client.get(endpoint).send().await.unwrap().text().await.unwrap() }
        };
        let inference = peer(http.for_account(&accounts[0])).await;
        let control = peer(http.control_for_account(&accounts[0])).await;
        let second = peer(http.for_account(&accounts[1])).await;
        let second_control = peer(http.control_for_account(&accounts[1])).await;
        assert_ne!(inference, second);
        assert_ne!(control, second_control);
        let tls = http.websocket_tls_for_account(&accounts[0]);
        assert!(!Arc::ptr_eq(&tls, &http.websocket_tls_for_account(&accounts[1])));
        let crate::accounts::Credential::OAuth(mut credential) = accounts[0].cred.read().clone() else {
            unreachable!()
        };
        credential.access_token = "FAKE_ROTATED_ACCESS".into();
        crate::oauth::publish_refreshed(&accounts[0], credential.clone(), vec![]);
        assert_eq!(inference, peer(http.for_account(&accounts[0])).await);
        assert_eq!(control, peer(http.control_for_account(&accounts[0])).await);
        assert!(Arc::ptr_eq(&tls, &http.websocket_tls_for_account(&accounts[0])));
        credential.account_id = Some("different-workspace".into());
        crate::oauth::publish_refreshed(&accounts[0], credential, vec![]);
        assert_ne!(inference, peer(http.for_account(&accounts[0])).await);
        assert_ne!(control, peer(http.control_for_account(&accounts[0])).await);
        assert!(!Arc::ptr_eq(&tls, &http.websocket_tls_for_account(&accounts[0])));
        assert_eq!(http.clients.lock().len(), 4, "obsolete identity pools were removed");
        assert_eq!(http.account_tls.lock().len(), 2);
        server.abort();
    }

    #[test]
    fn account_reload_prunes_replaced_and_removed_pools_without_pruning_token_rotations() {
        struct Directory(PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory =
            Directory(std::env::temp_dir().join(format!("fusebox-transport-reload-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(&directory.0).unwrap();
        let path = directory.0.join("same-account.json");
        let write = |workspace: &str, access: &str| {
            std::fs::write(
                &path,
                serde_json::json!({"type":"codex", "account_id":workspace, "access_token":access}).to_string(),
            )
            .unwrap();
        };
        write("fake-workspace-a", "FAKE_ACCESS_A");
        let mut cfg =
            Config { auth_dir: directory.0.to_string_lossy().into(), proxy_url: "direct".into(), ..Default::default() };
        let app = App::new(cfg.clone(), directory.0.join("config.yaml"));
        let account = app.pool.all().pop().unwrap();
        app.http.for_account(&account);
        app.http.control_for_account(&account);
        app.http.reset_for_account(&account);
        let tls = app.http.websocket_tls_for_account(&account);
        write("fake-workspace-a", "FAKE_ROTATED_ACCESS_A");
        app.reload_accounts();
        assert_eq!(app.http.clients.lock().len(), 3);
        assert!(Arc::ptr_eq(&tls, &app.http.websocket_tls_for_account(&app.pool.all()[0])));
        cfg.oauth_excluded_models.insert("codex".into(), vec!["unused-model".into()]);
        app.set_config(cfg);
        assert_eq!(app.http.clients.lock().len(), 3);
        assert!(Arc::ptr_eq(&tls, &app.http.websocket_tls_for_account(&app.pool.all()[0])));
        write("fake-workspace-b", "FAKE_ACCESS_B");
        app.reload_accounts();
        assert!(app.http.clients.lock().is_empty());
        assert!(app.http.account_tls.lock().is_empty());
        let replacement = app.pool.all().pop().unwrap();
        assert_ne!(AccountPool::new(&account).identity, AccountPool::new(&replacement).identity);
        assert_ne!(
            app.sessions.provider_session(Some("replacement-race"), &account, 1800),
            app.sessions.provider_session(Some("replacement-race"), &replacement, 1800)
        );
        let invalidated_tls = app.http.websocket_tls_for_account(&account);
        assert!(!Arc::ptr_eq(&invalidated_tls, &app.http.websocket_tls_for_account(&replacement)));
        assert!(!Arc::ptr_eq(&tls, &app.http.websocket_tls_for_account(&replacement)));
        app.http.control_for_account(&replacement);
        std::fs::remove_file(&path).unwrap();
        app.reload_accounts();
        assert!(app.http.clients.lock().is_empty());
        assert!(app.http.account_tls.lock().is_empty());
    }

    #[test]
    fn concurrent_streams_finishing_out_of_order_share_their_start_minute() {
        let now = Utc::now().timestamp() / 60;
        let stats = Stats::default();
        for (minute, tokens) in [(now - 2, 100), (now, 200), (now - 2, 300), (now - 1, 400), (now - 2, 500)] {
            stats.record(&log(minute, tokens));
        }
        let series = stats.series();
        assert_eq!(series.iter().map(|b| b.requests).sum::<u64>(), 5);
        assert_eq!(series.iter().map(|b| b.input_tokens).sum::<u64>(), 1500);
        let first = series.iter().find(|b| b.minute == now - 2).unwrap();
        assert_eq!((first.requests, first.input_tokens, first.tokens), (3, 900, 915));
        assert_eq!(stats.series.lock().len(), 3);
        assert!(series.windows(2).all(|b| b[0].minute < b[1].minute));
        assert_eq!(stats.account_series("account").iter().map(|b| b.input_tokens).sum::<u64>(), 1500);
    }

    #[test]
    fn a_late_completion_does_not_evict_the_current_hour() {
        let now = Utc::now().timestamp() / 60;
        let stats = Stats::default();
        for ago in (0..MINUTES as i64).rev() {
            stats.record(&log(now - ago, 10));
        }
        stats.record(&log(now - 65, 1000));
        let series = stats.series();
        assert_eq!(series.iter().map(|b| b.requests).sum::<u64>(), 60);
        assert_eq!(series.iter().map(|b| b.input_tokens).sum::<u64>(), 600);
        assert_eq!(stats.account_series("account").iter().map(|b| b.requests).sum::<u64>(), 60);
        assert_eq!(stats.totals.lock().input_tokens, 1600);
    }

    #[test]
    fn cancellations_and_usage_coverage_are_independent_of_failures() {
        let now = Utc::now().timestamp() / 60;
        let stats = Stats::default();
        let mut account = crate::accounts::Counters::default();
        // Missing usage is unknown even on a successful request; a reported zero
        // is complete and must not become unknown just because the client left.
        for (status, completeness, input, output, cache) in [
            (200, "complete", 100, 20, 30),
            (200, "missing", 0, 0, 0),
            (499, "missing", 0, 0, 0),
            (499, "partial", 10, 2, 3),
            (499, "complete", 0, 0, 0),
            (502, "missing", 0, 0, 0),
            (502, "partial", 5, 1, 2),
        ] {
            let mut request = log(now, input);
            request.status = status;
            request.usage_completeness = completeness;
            request.output_tokens = output;
            request.cache_tokens = cache;
            stats.record(&request);
            account.record(&request);
        }
        let totals = stats.totals.lock();
        assert_eq!((totals.requests, totals.ok, totals.failed, totals.cancelled), (7, 2, 2, 3));
        assert_eq!((totals.usage_missing, totals.usage_partial), (3, 2));
        assert_eq!((totals.input_tokens, totals.output_tokens, totals.cache_tokens), (115, 23, 35));
        assert_eq!((account.requests, account.failures, account.cancelled), (7, 2, 3));
        assert_eq!((account.usage_missing, account.usage_partial), (3, 2));
        assert_eq!((account.input_tokens, account.output_tokens, account.cache_tokens), (115, 23, 35));
        for series in [stats.series(), stats.account_series("account")] {
            let bucket = series.iter().find(|b| b.minute == now).unwrap();
            assert_eq!((bucket.requests, bucket.failed, bucket.cancelled), (7, 2, 3));
            assert_eq!((bucket.usage_missing, bucket.usage_partial, bucket.tokens), (3, 2, 173));
        }
    }
}
