use super::*;
use axum::{
    Json, Router,
    routing::{get, post},
};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const ACCOUNT: &str = "bc6d7b9b-441d-4b9b-a16b-38c1a38f9d7a";
const ORG: &str = "a3bc830b-c72c-4340-ae2e-632bd008dce8";

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("fusebox-window-starter-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Mock {
    usage: Mutex<Value>,
    requests: Mutex<Vec<Value>>,
    posts: AtomicUsize,
    status: Mutex<axum::http::StatusCode>,
    activate: Mutex<bool>,
    redirect: Mutex<bool>,
    profile_account: Mutex<String>,
    before_usage: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl Default for Mock {
    fn default() -> Self {
        Self {
            usage: Mutex::new(idle()),
            requests: Default::default(),
            posts: Default::default(),
            status: Mutex::new(axum::http::StatusCode::OK),
            activate: Mutex::new(true),
            redirect: Mutex::new(false),
            profile_account: Mutex::new(ACCOUNT.into()),
            before_usage: Mutex::new(None),
        }
    }
}

fn idle() -> Value {
    json!({"five_hour":{"utilization":0.0,"resets_at":null},"seven_day":{"utilization":10.0,"resets_at":Utc::now()+chrono::Duration::days(2)}})
}

async fn fixture(copies: usize, analytics: bool) -> (Temp, Arc<App>, Arc<Mock>, tokio::task::JoinHandle<()>) {
    let temp = Temp::new();
    for i in 0..copies {
        std::fs::write(
            temp.0.join(format!("account-{i}.json")),
            json!({"type":"claude","account_id":ACCOUNT,"access_token":"test-only","expired":"2099-01-01T00:00:00Z"})
                .to_string(),
        )
        .unwrap();
    }
    let mut cfg = crate::config::Config { auth_dir: temp.0.to_string_lossy().into(), ..Default::default() };
    cfg.window_starter.mode = Mode::Always;
    if analytics {
        cfg.usage.database = Some(temp.0.join("usage.db").to_string_lossy().into());
    }
    let app = App::new(cfg, temp.0.join("config.yaml"));
    let mut cfg = (*app.cfg()).clone();
    cfg.window_starter.accounts = app.pool.all().iter().map(|a| a.id.clone()).collect();
    app.set_config(cfg);
    let mock = Arc::new(Mock::default());
    let profile = mock.clone();
    let usage = mock.clone();
    let message = mock.clone();
    let routes = Router::new()
        .route("/api/oauth/profile", get(move || { let m = profile.clone(); async move {
            Json(json!({"account":{"uuid":*m.profile_account.lock()},"organization":{"uuid":ORG}}))
        }}))
        .route("/api/oauth/usage", get(move || { let m = usage.clone(); async move {
            if let Some(f) = m.before_usage.lock().take() { f(); }
            Json(m.usage.lock().clone())
        }}))
        .route("/v1/messages", post(move |Json(body): Json<Value>| { let m = message.clone(); async move {
            m.posts.fetch_add(1, Ordering::SeqCst);
            m.requests.lock().push(body);
            if *m.redirect.lock() {
                return (axum::http::StatusCode::TEMPORARY_REDIRECT, [("location", "/redirected"), ("retry-after", "60")], Json(json!({})));
            }
            let status = *m.status.lock();
            if status.is_success() && *m.activate.lock() {
                m.usage.lock()["five_hour"] = json!({"utilization":0.001,"resets_at":Utc::now()+chrono::Duration::hours(5)});
            }
            (status, [("location", "/redirected"), ("retry-after", "60")], Json(json!({"type":"message","role":"assistant","model":"claude-haiku-4-5-20251001","usage":{"input_tokens":11,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"content":[{"type":"text","text":"OK"}]})))
        }}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    *app.window_starter_test_origin.lock() = Some(format!("http://{}", listener.local_addr().unwrap()));
    let task = tokio::spawn(async move {
        axum::serve(listener, routes).await.unwrap();
    });
    (temp, app, mock, task)
}

fn action() -> Action {
    Action { request_id: uuid::Uuid::new_v4().to_string(), check_only: false }
}
async fn run(app: &Arc<App>, automatic: bool) -> Result<View> {
    start(app.clone(), app.pool.all()[0].clone(), action(), automatic).await
}

#[test]
fn provider_inactivity_is_explicit_and_zero_percent_can_be_active() {
    let now = Utc::now();
    assert_eq!(window(&idle(), now), Window::Inactive);
    assert_eq!(window(&json!({"five_hour":null}), now), Window::Inactive);
    for w in [
        json!({}),
        json!({"five_hour":{}}),
        json!({"five_hour":{"utilization":0}}),
        json!({"five_hour":{"utilization":1,"resets_at":null}}),
        json!({"five_hour":{"utilization":0,"resets_at":"bad"}}),
        json!({"five_hour":{"utilization":0,"resets_at":now-chrono::Duration::hours(1)}}),
    ] {
        assert_eq!(window(&w, now), Window::Unknown, "{w}");
    }
    let reset = now + chrono::Duration::hours(2);
    assert_eq!(window(&json!({"five_hour":{"utilization":0,"resets_at":reset}}), now), Window::Active(reset));
    assert!(weekly_available(&idle(), now));
    assert!(weekly_available(&json!({"seven_day":{"utilization":0.0,"resets_at":null}}), now));
    assert!(!weekly_available(&json!({}), now));
    assert!(!weekly_available(&json!({"seven_day":{"utilization":100,"resets_at":reset}}), now));
}

#[test]
fn schedules_follow_local_weekdays_overnight_hours_and_dst() {
    let mut p = Config {
        mode: Mode::WorkingHours,
        timezone: "Europe/Amsterdam".into(),
        weekdays: vec![1],
        start: "22:00".into(),
        end: "02:00".into(),
        ..Default::default()
    };
    let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
    assert!(p.in_hours(at("2026-10-19T20:00:00Z"))); // Mon 22:00 CEST
    assert!(p.in_hours(at("2026-10-19T23:00:00Z"))); // Tue 01:00
    assert!(!p.in_hours(at("2026-10-20T00:00:00Z"))); // exclusive end
    assert!(!p.in_hours(at("2026-10-20T21:00:00Z"))); // Tue not selected
    p.weekdays = vec![7];
    p.start = "01:00".into();
    p.end = "04:00".into();
    assert!(p.in_hours(at("2026-10-25T00:30:00Z"))); // first 02:30
    assert!(p.in_hours(at("2026-10-25T01:30:00Z"))); // second 02:30
    assert!(!p.in_hours(at("2026-10-25T03:00:00Z")));
    p.timezone = "America/New_York".into();
    p.weekdays = vec![1];
    p.start = "08:00".into();
    p.end = "18:00".into();
    assert!(p.in_hours(at("2026-10-19T12:00:00Z")));
    assert!(!p.in_hours(at("2026-10-19T11:59:00Z")));
}

#[test]
fn config_round_trips_both_layouts_and_rejects_unsafe_values() {
    for source in ["", "server:\n  port: 8317\nrouting:\n  strategy: smart-quota\n"] {
        let p = Config { mode: Mode::WorkingHours, accounts: vec!["one".into()], ..Default::default() };
        let changes = json!({"window-starter":p});
        let (yaml, cfg, _) = crate::config_editor::apply(source, changes.as_object().unwrap()).unwrap();
        assert_eq!(cfg.window_starter, p);
        assert_eq!(crate::config::Config::parse(&yaml).unwrap().window_starter, p);
        assert!(yaml.contains("window-starter:"));
    }
    for value in [
        json!({"mode":"bogus"}),
        json!({"timezone":"local"}),
        json!({"weekdays":[]}),
        json!({"weekdays":[0]}),
        json!({"weekdays":[1,1]}),
        json!({"start":"8:00"}),
        json!({"start":"25:00"}),
        json!({"start":"18:00","end":"18:00"}),
        json!({"model":"group/claude-haiku-4-5-20251001"}),
        json!({"model":"claude-haiku-4-5-20251001(fast)"}),
        json!({"accounts":["a","a"]}),
        json!({"unknown":true}),
    ] {
        let yaml = format!("window-starter: {}\n", value);
        assert!(crate::config::Config::parse(&yaml).is_err(), "{value}");
        let changes = json!({"window-starter":value});
        assert!(crate::config_editor::apply("", changes.as_object().unwrap()).is_err());
    }
    assert_eq!(Config::default().mode, Mode::Off);
}

#[tokio::test]
async fn starts_once_confirms_provider_timestamp_and_records_system_usage() {
    let (_temp, app, mock, server) = fixture(2, true).await;
    let first = run(&app, true).await.unwrap();
    assert_eq!(first.status, "started");
    assert!(first.resets_at.unwrap() > Utc::now());
    assert_eq!(
        first.resets_at.unwrap().to_rfc3339(),
        DateTime::parse_from_rfc3339(mock.usage.lock()["five_hour"]["resets_at"].as_str().unwrap())
            .unwrap()
            .to_rfc3339()
    );
    assert_eq!(mock.posts.load(Ordering::SeqCst), 1);
    assert_eq!(run(&app, true).await.unwrap().status, "active");
    assert_eq!(mock.posts.load(Ordering::SeqCst), 1);
    for a in app.pool.all() {
        assert!(a.state.lock().window_starter.as_ref().unwrap().resets_at.is_some());
        assert_eq!(a.state.lock().active_requests.load(Ordering::Relaxed), 0);
    }
    let body = mock.requests.lock()[0].clone();
    assert_eq!(body["max_tokens"], 8);
    assert_eq!(body["thinking"]["type"], "disabled");
    assert!(body["tools"].is_null());
    assert!(body["service_tier"].is_null());
    assert!(body["speed"].is_null());
    assert_eq!(app.sessions.session_counts(86400).len(), 0);
    let a = app.pool.all()[0].clone();
    assert!(a.state.lock().latency.is_empty());
    assert_eq!(a.state.lock().counters.requests, 0);
    let store = app.usage.as_ref().unwrap();
    store.flush().await.unwrap();
    let q = crate::usage::store::Query { client: Some("system:window-starter".into()), ..Default::default() };
    let data = store.reference_summary(q).await.unwrap();
    assert_eq!(data["proxy"]["tokens"]["input"], 11);
    assert_eq!(data["proxy"]["tokens"]["output"], 1);
    assert_eq!(crate::mgmt::client_label(&app.cfg(), "system:window-starter"), "Window starter");
    store.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn no_requests_for_unknown_active_exhausted_or_unverified_accounts() {
    let (_temp, app, mock, server) = fixture(1, false).await;
    for usage in [
        json!({}),
        json!({"five_hour":{"utilization":0},"seven_day":null}),
        json!({"five_hour":null}),
        json!({"five_hour":null,"seven_day":{"utilization":100,"resets_at":Utc::now()+chrono::Duration::days(1)}}),
    ] {
        *mock.usage.lock() = usage;
        assert!(run(&app, true).await.is_err());
    }
    *mock.usage.lock() =
        json!({"five_hour":{"utilization":0,"resets_at":Utc::now()+chrono::Duration::hours(1)},"seven_day":null});
    assert_eq!(run(&app, true).await.unwrap().status, "active");
    *mock.usage.lock() = idle();
    *mock.profile_account.lock() = uuid::Uuid::new_v4().to_string();
    assert!(run(&app, true).await.is_err());
    assert_eq!(mock.posts.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn pending_or_unknown_attempt_survives_restart_and_coalesces_duplicates() {
    let (temp, app, mock, server) = fixture(2, false).await;
    *mock.activate.lock() = false;
    assert_eq!(run(&app, true).await.unwrap().status, "unknown");
    let cfg = (*app.cfg()).clone();
    let restarted = App::new(cfg, temp.0.join("config.yaml"));
    *restarted.window_starter_test_origin.lock() = app.window_starter_test_origin.lock().clone();
    for a in restarted.pool.all() {
        assert_eq!(start(restarted.clone(), a, action(), true).await.unwrap().status, "unknown");
    }
    assert_eq!(mock.posts.load(Ordering::SeqCst), 1);
    let mut journal = Journal::open(&temp.0, ACCOUNT, ORG).unwrap();
    journal.attempt.as_mut().unwrap().outcome = Outcome::Pending;
    journal.save().unwrap();
    assert_eq!(run(&restarted, false).await.unwrap().status, "unknown");
    assert_eq!(mock.posts.load(Ordering::SeqCst), 1);
    mock.usage.lock()["five_hour"] = json!({"utilization":0,"resets_at":Utc::now()+chrono::Duration::hours(4)});
    assert_eq!(run(&restarted, true).await.unwrap().status, "active");
    assert_eq!(Journal::open(&temp.0, ACCOUNT, ORG).unwrap().attempt.unwrap().outcome, Outcome::Started);
    server.abort();
}

#[tokio::test]
async fn real_work_or_settings_changes_during_preflight_cancel_the_starter() {
    let (_temp, app, mock, server) = fixture(1, false).await;
    let a = app.pool.all()[0].clone();
    let acct = a.clone();
    *mock.before_usage.lock() = Some(Box::new(move || {
        acct.state.lock().active_requests.fetch_add(1, Ordering::SeqCst);
    }));
    assert_eq!(run(&app, true).await.unwrap().status, "skipped");
    a.state.lock().active_requests.store(0, Ordering::SeqCst);
    let changed = app.clone();
    *mock.before_usage.lock() = Some(Box::new(move || {
        let mut cfg = (*changed.cfg()).clone();
        cfg.window_starter.mode = Mode::Off;
        changed.set_config(cfg);
    }));
    assert!(run(&app, true).await.is_err());
    assert_eq!(mock.posts.load(Ordering::SeqCst), 0);
    // Manual starting is deliberate even while the automatic mode is off.
    assert_eq!(run(&app, false).await.unwrap().status, "started");
    server.abort();
}

#[tokio::test]
async fn reset_lock_and_unresolved_reset_prevent_dispatch() {
    let (temp, app, mock, server) = fixture(1, false).await;
    let lock = crate::banked_resets::ledger::Ledger::open(&temp.0, &format!("claude:{ORG}")).unwrap();
    assert!(run(&app, true).await.is_err());
    drop(lock);
    let mut lock = crate::banked_resets::ledger::Ledger::open(&temp.0, &format!("claude:{ORG}")).unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    lock.journal.operations.insert(
        request.clone(),
        crate::banked_resets::ledger::Operation {
            request_id: request,
            account: ACCOUNT.into(),
            provider: "claude".into(),
            grant_id: "test".into(),
            clears: vec!["5h".into()],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            status: "unknown".into(),
            message: String::new(),
            automatic: false,
            auto_credit: None,
        },
    );
    lock.save().unwrap();
    drop(lock);
    assert!(run(&app, true).await.is_err());
    assert_eq!(mock.posts.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn check_only_never_generates_and_refusals_respect_retry_after() {
    let (_temp, app, mock, server) = fixture(1, false).await;
    let mut check = action();
    check.check_only = true;
    assert_eq!(start(app.clone(), app.pool.all()[0].clone(), check, false).await.unwrap().status, "inactive");
    assert_eq!(mock.posts.load(Ordering::SeqCst), 0);
    *mock.status.lock() = axum::http::StatusCode::TOO_MANY_REQUESTS;
    let before = Utc::now();
    let refused = run(&app, false).await.unwrap();
    assert_eq!(refused.status, "refused");
    assert!(refused.retry_after.unwrap() >= before + chrono::Duration::seconds(60));
    assert!(refused.retry_after.unwrap() < before + chrono::Duration::seconds(65));
    assert_eq!(run(&app, false).await.unwrap().status, "refused");
    assert_eq!(mock.posts.load(Ordering::SeqCst), 1);
    assert!(app.pool.all()[0].state.lock().last_error(Utc::now()).is_none());
    server.abort();
}

#[tokio::test]
async fn redirects_and_server_errors_do_not_retry_generation() {
    for redirect in [true, false] {
        let (_temp, app, mock, server) = fixture(1, false).await;
        *mock.redirect.lock() = redirect;
        *mock.status.lock() = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
        assert_eq!(run(&app, true).await.unwrap().status, "unknown");
        assert_eq!(run(&app, true).await.unwrap().status, "unknown");
        assert_eq!(mock.posts.load(Ordering::SeqCst), 1);
        server.abort();
    }
}

#[tokio::test]
async fn new_window_can_start_after_confirmed_expiry_and_unknown_hold_expires() {
    let (temp, app, mock, server) = fixture(1, false).await;
    assert_eq!(run(&app, true).await.unwrap().status, "started");
    let mut journal = Journal::open(&temp.0, ACCOUNT, ORG).unwrap();
    let a = journal.attempt.as_mut().unwrap();
    a.at = Utc::now() - chrono::Duration::hours(6);
    a.retry_after = Utc::now() - chrono::Duration::minutes(1);
    a.resets_at = Some(a.retry_after);
    journal.save().unwrap();
    *mock.usage.lock() = idle();
    assert_eq!(run(&app, true).await.unwrap().status, "started");
    assert_eq!(mock.posts.load(Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn disabled_unselected_off_and_cooling_accounts_never_generate() {
    let (_temp, app, mock, server) = fixture(1, false).await;
    let a = app.pool.all()[0].clone();
    a.state.lock().disabled = true;
    assert!(run(&app, false).await.is_err());
    a.state.lock().disabled = false;
    a.state.lock().cooldowns.insert("*".into(), Utc::now() + chrono::Duration::minutes(5));
    assert_eq!(run(&app, true).await.unwrap().status, "skipped");
    a.state.lock().cooldowns.clear();
    let mut cfg = (*app.cfg()).clone();
    cfg.window_starter.accounts.clear();
    app.set_config(cfg);
    assert!(run(&app, true).await.is_err());
    let mut cfg = (*app.cfg()).clone();
    cfg.window_starter.mode = Mode::Off;
    app.set_config(cfg);
    assert!(run(&app, true).await.is_err());
    assert_eq!(mock.posts.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn foreground_attempt_that_finishes_during_preflight_still_cancels_generation() {
    let (_temp, app, mock, server) = fixture(1, false).await;
    let a = app.pool.all()[0].clone();
    let foreground = a.clone();
    *mock.before_usage.lock() = Some(Box::new(move || {
        let load = crate::accounts::RequestLoad::new(&foreground);
        drop(load);
    }));
    assert_eq!(run(&app, true).await.unwrap().status, "skipped");
    assert_eq!(mock.posts.load(Ordering::SeqCst), 0);
    assert!(a.state.lock().window_starter_cancel.is_none());
    server.abort();
}

#[tokio::test]
async fn corrupt_journal_blocks_spending_and_reads_do_not_require_a_supported_model() {
    let (temp, app, mock, server) = fixture(1, false).await;
    let mut cfg = (*app.cfg()).clone();
    cfg.window_starter.model = "claude-missing-model".into();
    app.set_config(cfg);
    let mut check = action();
    check.check_only = true;
    assert_eq!(start(app.clone(), app.pool.all()[0].clone(), check, false).await.unwrap().status, "inactive");
    assert!(run(&app, true).await.is_err());
    let journal = Journal::open(&temp.0, ACCOUNT, ORG).unwrap();
    journal.save().unwrap();
    let file = std::fs::read_dir(temp.0.join(".window-starters")).unwrap().next().unwrap().unwrap().path();
    std::fs::write(file, b"broken journal").unwrap();
    assert!(run(&app, false).await.unwrap_err().to_string().contains("journal is unreadable"));
    assert_eq!(mock.posts.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn confirmed_banked_reset_releases_the_old_window_hold() {
    let (temp, app, mock, server) = fixture(1, false).await;
    assert_eq!(run(&app, true).await.unwrap().status, "started");
    let mut reset = crate::banked_resets::ledger::Ledger::open(&temp.0, &format!("claude:{ORG}")).unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    reset.journal.operations.insert(
        request.clone(),
        crate::banked_resets::ledger::Operation {
            request_id: request,
            account: ACCOUNT.into(),
            provider: "claude".into(),
            grant_id: "test".into(),
            clears: vec!["5h".into()],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            status: "applied".into(),
            message: String::new(),
            automatic: false,
            auto_credit: None,
        },
    );
    reset.save().unwrap();
    drop(reset);
    *mock.usage.lock() = idle();
    assert_eq!(run(&app, true).await.unwrap().status, "started");
    assert_eq!(mock.posts.load(Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn management_action_requires_auth_and_reconciles_an_idempotent_retry() {
    let (_temp, app, mock, server) = fixture(1, false).await;
    let mut cfg = (*app.cfg()).clone();
    cfg.management_key = "test-management".into();
    app.set_config(cfg);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/accounts/{}/window-starter", listener.local_addr().unwrap(), app.pool.all()[0].id);
    let router = crate::server::router(app.clone());
    let proxy = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
    });
    let client = reqwest::Client::new();
    let body = json!({"request_id":uuid::Uuid::new_v4().to_string(),"check_only":false});
    assert_eq!(client.post(&url).json(&body).send().await.unwrap().status(), axum::http::StatusCode::UNAUTHORIZED);
    let result: Value = client
        .post(&url)
        .bearer_auth("test-management")
        .json(&body)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["status"], "started");
    let replay: Value = client
        .post(&url)
        .bearer_auth("test-management")
        .json(&body)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(replay["status"], "active");
    assert_eq!(mock.posts.load(Ordering::SeqCst), 1);
    server.abort();
    proxy.abort();
}

#[tokio::test]
async fn explicit_inactive_windows_replace_old_quota_but_missing_windows_never_spend() {
    let (_temp, app, mock, server) = fixture(1, false).await;
    let stale = json!({"five_hour":{"utilization":100,"resets_at":Utc::now()+chrono::Duration::hours(2)},"seven_day":{"utilization":100,"resets_at":Utc::now()+chrono::Duration::days(2)}});
    crate::quota::usage(&mut app.pool.all()[0].state.lock(), Provider::Claude, &stale);
    *mock.usage.lock() = json!({});
    assert!(run(&app, true).await.is_err());
    assert!(app.pool.all()[0].state.lock().quota.exhausted("claude-haiku-4-5-20251001"));
    *mock.usage.lock() = json!({"five_hour":null,"seven_day":null});
    assert_eq!(run(&app, true).await.unwrap().status, "started");
    assert_eq!(mock.posts.load(Ordering::SeqCst), 1);
    server.abort();
}
