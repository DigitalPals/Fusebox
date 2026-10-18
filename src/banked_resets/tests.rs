use super::*;
use parking_lot::Mutex;
use serde_json::json;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("banked-reset-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture(provider: Provider, copies: usize) -> (Temp, Arc<App>, Arc<Account>) {
    let temp = Temp::new();
    for i in 0..copies {
        std::fs::write(temp.0.join(format!("account-{i}.json")), json!({"type":provider.as_str(),"account_id":"account-1", "access_token":"test-only", "expired":"2099-01-01T00:00:00Z"}).to_string()).unwrap();
    }
    let cfg =
        crate::config::Config { auth_dir: temp.0.to_string_lossy().into(), banked_resets: true, ..Default::default() };
    let app = App::new(cfg, temp.0.join("config.yaml"));
    let acct = app.pool.all()[0].clone();
    (temp, app, acct)
}
fn claude_usage() -> Value {
    json!({"five_hour":{"utilization":100,"resets_at":"2099-01-01T00:00:00Z"}, "seven_day":{"utilization":10,"resets_at":"2099-01-01T00:00:00Z"}, "cedar_ember":{"eligible":true,"at_limit":true,"grants":[{"id":"grant-1","label":"Demo grant","resets_total":2,"resets_left":2,"usable_now":true,"clears":["five_hour"],"ends_at":"2099-01-01T00:00:00Z"}]}})
}
struct Mock {
    inventory: Inventory,
    usage: Value,
    outcomes: Mutex<VecDeque<Outcome>>,
    calls: Mutex<Vec<String>>,
    reads: AtomicUsize,
    fail_after: usize,
}
impl Mock {
    fn new(outcomes: &[Outcome]) -> Self {
        Self {
            inventory: provider::claude(&claude_usage(), Utc::now()).unwrap(),
            usage: json!({"five_hour":{"utilization":0,"resets_at":"2099-01-01T00:00:00Z"},"seven_day":{"utilization":10,"resets_at":"2099-01-01T00:00:00Z"}}),
            outcomes: Mutex::new(outcomes.iter().copied().collect()),
            calls: Mutex::new(vec![]),
            reads: AtomicUsize::new(0),
            fail_after: usize::MAX,
        }
    }
}
impl Api for Mock {
    async fn read(&self) -> Result<(Inventory, Value)> {
        let n = self.reads.fetch_add(1, Ordering::Relaxed);
        ensure!(n < self.fail_after, "Mock read unavailable");
        Ok((self.inventory.clone(), self.usage.clone()))
    }
    async fn identity(&self) -> Result<String> {
        Ok("organization-1".into())
    }
    async fn redeem(&self, _: &str, _: &str, request: &str) -> Result<Outcome> {
        self.calls.lock().push(request.into());
        Ok(self.outcomes.lock().pop_front().unwrap())
    }
}
fn quote(app: &App, mock: &Mock, ledger: &Ledger) -> String {
    let request = uuid::Uuid::new_v4().to_string();
    app.reset_quotes.lock().insert(
        request.clone(),
        Quote {
            account: "account-1".into(),
            identity: "claude:organization-1".into(),
            version: ledger.journal.operations.len(),
            fingerprint: fingerprint(&mock.inventory),
            expires: Utc::now() + chrono::Duration::minutes(2),
        },
    );
    request
}
fn action(request: &str, kind: &str) -> Action {
    Action { action: kind.into(), request_id: request.into(), grant_id: "grant-1".into(), confirmed: true }
}
async fn run(app: &Arc<App>, acct: &Arc<Account>, mock: &Mock, ledger: &mut Ledger, action: Action) -> Result<View> {
    execute(app, acct, mock, "organization-1", "account-1", ledger, Request::Manual(action)).await
}

fn expiring_usage(weekly: f64, short: f64, hours: i64) -> Value {
    let now = Utc::now();
    json!({
        "five_hour":{"utilization":short,"resets_at":now + chrono::Duration::hours(4)},
        "seven_day":{"utilization":weekly,"resets_at":now + chrono::Duration::days(7)},
        "cedar_ember":{"eligible":true,"at_limit":true,"grants":[{
            "id":"grant-1","label":"Expiring grant","resets_total":2,"resets_left":2,"usable_now":true,
            "clears":["five_hour","seven_day"],"ends_at":now + chrono::Duration::hours(hours)
        }]}
    })
}

fn reset_view(inventory: Inventory) -> View {
    View {
        checked_at: Utc::now(),
        inventory: Some(inventory),
        error: None,
        quote: None,
        operation: None,
        retryable: false,
    }
}

fn automatic_fixture(weekly: f64, short: f64, hours: i64) -> (Temp, Arc<App>, Arc<Account>, Mock) {
    let (temp, app, acct) = fixture(Provider::Claude, 1);
    let mut cfg = (*app.cfg()).clone();
    cfg.routing = crate::config::Routing::SmartQuota;
    cfg.banked_reset_policy.auto_redeem = true;
    app.set_config(cfg);
    let mut mock = Mock::new(&[Outcome::Applied]);
    mock.usage = expiring_usage(weekly, short, hours);
    mock.inventory = provider::claude(&mock.usage, Utc::now()).unwrap();
    {
        let mut st = acct.state.lock();
        crate::quota::usage(&mut st, Provider::Claude, &mock.usage);
        st.banked_resets = Some(reset_view(mock.inventory.clone()));
    }
    (temp, app, acct, mock)
}

#[test]
fn automatic_policy_uses_weekly_consumption_until_the_expiry_deadline() {
    let (_temp, app, acct, _) = automatic_fixture(3.0, 0.0, 120);
    let cfg = app.cfg();
    let due = |usage: Value| {
        let inv = provider::claude(&usage, Utc::now()).unwrap();
        let mut fresh = crate::accounts::AccountState::default();
        crate::quota::usage(&mut fresh, Provider::Claude, &usage);
        automation::decision(&cfg, &fresh.quota, &inv, Utc::now()).is_some()
    };
    assert!(!due(expiring_usage(94.0, 100.0, 120)), "a short limit must not waste a weekly reset days early");
    assert!(due(expiring_usage(95.0, 100.0, 120)));
    assert!(!due(expiring_usage(20.0, 0.0, 5)));
    assert!(!due(expiring_usage(19.0, 0.0, 5)));
    assert!(due(expiring_usage(0.0, 0.0, 1)), "redeem at the one-hour deadline even with no usage");
    assert!(!due(expiring_usage(3.0, 100.0, 5)), "short allowance renews before the credit expires");
    assert!(!due(expiring_usage(3.0, 100.0, 2)), "the expiry fallback starts one hour before expiry");
    assert!(due(expiring_usage(3.0, 0.0, 1)), "low usage still redeems at the expiry deadline");
    assert!(!due(expiring_usage(100.0, 100.0, 200)), "outside the configured expiry window");
    assert!(!due(expiring_usage(100.0, 100.0, -1)));
    let mut usage = expiring_usage(100.0, 100.0, 120);
    usage["seven_day"]["resets_at"] = json!(Utc::now() + chrono::Duration::minutes(5));
    assert!(!due(usage), "wait for a weekly allowance that is about to renew naturally");
    let mut usage = expiring_usage(3.0, 95.0, 120);
    usage["cedar_ember"]["grants"][0]["clears"] = json!(["five_hour"]);
    assert!(due(usage), "a short-only reset can replenish its own depleted window");
    let mut usage = expiring_usage(100.0, 100.0, 120);
    usage["cedar_ember"]["grants"][0]["paused"] = json!(true);
    assert!(!due(usage));
    let mut usage = expiring_usage(100.0, 100.0, 120);
    usage["cedar_ember"]["grants"][0]["ends_at"] = Value::Null;
    assert!(!due(usage), "an unknown expiry cannot authorize expiry-based spending");
    assert!(plan(&acct, &cfg).unwrap().auto_redeem);
}

#[test]
fn expiry_deadline_is_exact_and_ignores_missing_usage_and_natural_renewal() {
    let (_temp, app, _, _) = automatic_fixture(0.0, 0.0, 1);
    let cfg = app.cfg();
    let now = Utc::now();
    let mut usage = expiring_usage(0.0, 0.0, 1);
    usage["seven_day"]["resets_at"] = json!(now + chrono::Duration::minutes(5));
    let mut fresh = crate::accounts::AccountState::default();
    crate::quota::usage(&mut fresh, Provider::Claude, &usage);
    let claude = provider::claude(&usage, now).unwrap();
    let codex = provider::codex(
        &json!({}),
        &json!({"available_count":1,"credits":[{
            "id":"credit-1","status":"available","reset_type":"codex_rate_limits",
            "expires_at":now + chrono::Duration::hours(1)
        }]}),
        now,
    )
    .unwrap();
    for mut inv in [claude, codex] {
        for (seconds, expected) in
            [(3601, false), (3600, true), (3599, true), (30, true), (1, true), (0, false), (-1, false)]
        {
            inv.grants[0].expires_at = Some(now + chrono::Duration::seconds(seconds));
            for quota in [&fresh.quota, &crate::quota::Quota::default()] {
                let decision = automation::decision(&cfg, quota, &inv, now);
                assert_eq!(decision.is_some(), expected, "{seconds} seconds before expiry");
                if let Some(d) = decision {
                    assert!(d.expiry_due);
                }
            }
        }
    }
}

#[tokio::test]
async fn expiry_redemption_uses_fresh_inventory_without_requiring_usage() {
    let (temp, app, acct, mut mock) = automatic_fixture(0.0, 0.0, 1);
    mock.usage.as_object_mut().unwrap().remove("five_hour");
    mock.usage.as_object_mut().unwrap().remove("seven_day");
    let plan = plan(&acct, &app.cfg()).unwrap();
    assert!(plan.due);
    assert_eq!(plan.expiry_redeem_at, plan.expires_at - chrono::Duration::hours(1));
    let mut ledger = Ledger::open(&temp.0, "claude:organization-1").unwrap();
    let result =
        execute(&app, &acct, &mock, "organization-1", "account-1", &mut ledger, Request::Automatic).await.unwrap();
    assert_eq!(result.operation.unwrap().status, "applied");
    assert_eq!(mock.calls.lock().len(), 1);
    assert!(execute(&app, &acct, &mock, "organization-1", "account-1", &mut ledger, Request::Automatic).await.is_err());
    assert_eq!(mock.calls.lock().len(), 1, "the deadline cannot override duplicate-credit protection");
    // Older/manual operations have no automatic credit marker, so retain their
    // recent-success pause until a distinct next credit can safely be evaluated.
    for op in ledger.journal.operations.values_mut() {
        op.automatic = false;
        op.auto_credit = None;
    }
    mock.inventory.grants[0].remaining = 1;
    mock.inventory.available = Some(1);
    assert!(execute(&app, &acct, &mock, "organization-1", "account-1", &mut ledger, Request::Automatic).await.is_err());
    assert_eq!(mock.calls.lock().len(), 1, "recent manual redemption still pauses automatic spending");
}

#[tokio::test]
async fn expiry_deadline_rechecks_refusals_after_five_minutes_and_can_use_distinct_credits() {
    let (temp, app, acct, mut mock) = automatic_fixture(0.0, 0.0, 1);
    *mock.outcomes.lock() = VecDeque::from([Outcome::Refused("Rate limited"), Outcome::Applied, Outcome::Applied]);
    let mut ledger = Ledger::open(&temp.0, "claude:organization-1").unwrap();
    let result =
        execute(&app, &acct, &mock, "organization-1", "account-1", &mut ledger, Request::Automatic).await.unwrap();
    assert_eq!(result.operation.unwrap().status, "refused");
    assert!(execute(&app, &acct, &mock, "organization-1", "account-1", &mut ledger, Request::Automatic).await.is_err());
    assert_eq!(mock.calls.lock().len(), 1);
    for op in ledger.journal.operations.values_mut() {
        op.updated_at -= chrono::Duration::minutes(6);
    }
    ledger.save().unwrap();
    let result =
        execute(&app, &acct, &mock, "organization-1", "account-1", &mut ledger, Request::Automatic).await.unwrap();
    assert_eq!(result.operation.unwrap().status, "applied");
    mock.inventory.grants[0].remaining = 1;
    mock.inventory.available = Some(1);
    let result =
        execute(&app, &acct, &mock, "organization-1", "account-1", &mut ledger, Request::Automatic).await.unwrap();
    assert_eq!(result.operation.unwrap().status, "applied");
    assert_eq!(mock.calls.lock().len(), 3, "fresh inventory confirms the next distinct credit");
}

#[tokio::test]
async fn automatic_redemption_rechecks_usage_and_persists_duplicate_protection() {
    let (temp, app, acct, mut mock) = automatic_fixture(95.0, 0.0, 120);
    let mut ledger = Ledger::open(&temp.0, "claude:organization-1").unwrap();
    // Cached usage says 95%, but authoritative preflight says 3%.
    mock.usage["seven_day"]["utilization"] = json!(3);
    assert!(execute(&app, &acct, &mock, "organization-1", "account-1", &mut ledger, Request::Automatic).await.is_err());
    assert!(mock.calls.lock().is_empty());
    assert!(ledger.journal.operations.is_empty());
    mock.usage["seven_day"]["utilization"] = json!(95);
    let result =
        execute(&app, &acct, &mock, "organization-1", "account-1", &mut ledger, Request::Automatic).await.unwrap();
    assert_eq!(result.operation.as_ref().unwrap().status, "applied");
    assert!(result.operation.as_ref().unwrap().automatic);
    assert_eq!(mock.calls.lock().len(), 1);
    // Even after the recent-spend delay, stale provider inventory must not spend the same credit again.
    for op in ledger.journal.operations.values_mut() {
        op.updated_at -= chrono::Duration::hours(2);
    }
    ledger.save().unwrap();
    drop(ledger);
    let mut ledger = Ledger::open(&temp.0, "claude:organization-1").unwrap();
    assert!(execute(&app, &acct, &mock, "organization-1", "account-1", &mut ledger, Request::Automatic).await.is_err());
    assert_eq!(mock.calls.lock().len(), 1);
    let mut additional = mock.inventory.grants[0].clone();
    additional.id = "unrelated-grant".into();
    additional.expires_at = additional.expires_at.map(|e| e + chrono::Duration::days(1));
    mock.inventory.grants.push(additional);
    mock.inventory.available = Some(4);
    assert!(execute(&app, &acct, &mock, "organization-1", "account-1", &mut ledger, Request::Automatic).await.is_err());
    assert_eq!(mock.calls.lock().len(), 1, "unrelated inventory changes cannot authorize a duplicate credit");
}

#[tokio::test]
async fn automatic_unknown_outcome_blocks_after_restart_and_opt_out_prevents_spending() {
    let (temp, app, acct, mock) = automatic_fixture(0.0, 0.0, 1);
    let mut cfg = (*app.cfg()).clone();
    cfg.banked_reset_policy.auto_redeem = false;
    app.set_config(cfg.clone());
    let mut ledger = Ledger::open(&temp.0, "claude:organization-1").unwrap();
    assert!(execute(&app, &acct, &mock, "organization-1", "account-1", &mut ledger, Request::Automatic).await.is_err());
    assert!(mock.calls.lock().is_empty());
    cfg.banked_reset_policy.auto_redeem = true;
    app.set_config(cfg);
    *mock.outcomes.lock() = VecDeque::from([Outcome::Unknown]);
    let result =
        execute(&app, &acct, &mock, "organization-1", "account-1", &mut ledger, Request::Automatic).await.unwrap();
    assert_eq!(result.operation.unwrap().status, "unknown");
    drop(ledger);
    let mut ledger = Ledger::open(&temp.0, "claude:organization-1").unwrap();
    assert!(execute(&app, &acct, &mock, "organization-1", "account-1", &mut ledger, Request::Automatic).await.is_err());
    assert_eq!(mock.calls.lock().len(), 1);
}

#[tokio::test]
async fn automatic_codex_reset_requires_shared_quota_and_keeps_model_limits() {
    let (temp, app, acct) = fixture(Provider::Codex, 1);
    let mut cfg = (*app.cfg()).clone();
    cfg.routing = crate::config::Routing::SmartQuota;
    cfg.banked_reset_policy.auto_redeem = true;
    app.set_config(cfg);
    let now = Utc::now();
    let mut mock = Mock::new(&[Outcome::Applied]);
    mock.usage = json!({"rate_limit":{
        "primary_window":{"used_percent":100,"limit_window_seconds":18000,"reset_at":(now + chrono::Duration::hours(4)).timestamp()},
        "secondary_window":{"used_percent":3,"limit_window_seconds":604800,"reset_at":(now + chrono::Duration::days(7)).timestamp()}
    }});
    let details = json!({"available_count":1,"credits":[{
        "id":"codex-credit","status":"available","reset_type":"codex_rate_limits",
        "expires_at":now + chrono::Duration::days(5)
    }]});
    mock.inventory = provider::codex(&mock.usage, &details, now).unwrap();
    let model_limit = crate::quota::Window {
        name: "week special".into(),
        used: 100.0,
        resets_at: Some(now + chrono::Duration::days(7)),
        model: Some("special".into()),
    };
    {
        let mut st = acct.state.lock();
        crate::quota::usage(&mut st, Provider::Codex, &mock.usage);
        st.quota.windows.push(model_limit.clone());
        assert!(automation::decision(&app.cfg(), &st.quota, &mock.inventory, now).is_none());
    }
    let mut ledger = Ledger::open(&temp.0, "codex:account-1").unwrap();
    assert!(execute(&app, &acct, &mock, "account-1", "account-1", &mut ledger, Request::Automatic).await.is_err());
    assert!(mock.calls.lock().is_empty(), "a short or model limit cannot spend a shared weekly credit");
    mock.usage["rate_limit"]["secondary_window"]["used_percent"] = json!(96);
    let result = execute(&app, &acct, &mock, "account-1", "account-1", &mut ledger, Request::Automatic).await.unwrap();
    assert_eq!(result.operation.unwrap().status, "applied");
    assert_eq!(mock.calls.lock().len(), 1);
    assert!(acct.state.lock().quota.windows.contains(&model_limit));
    assert!(ledger.journal.latest().unwrap().grant_id.is_empty(), "Codex chooses the credit on the provider side");
}

#[test]
fn expiry_routing_matches_production_order_and_preserves_session_reserves() {
    use crate::accounts::Pick;
    use crate::quota::Window;
    use std::collections::HashMap;
    let temp = Temp::new();
    for name in ["cybex", "itpals", "digitalbrain"] {
        std::fs::write(temp.0.join(format!("{name}.json")), json!({
            "type":"claude","account_id":name,"email":format!("{name}@test.invalid"),"access_token":"test-only","expired":"2099-01-01T00:00:00Z"
        }).to_string()).unwrap();
    }
    let mut cfg = crate::config::Config {
        auth_dir: temp.0.to_string_lossy().into(),
        banked_resets: true,
        routing: crate::config::Routing::SmartQuota,
        ..Default::default()
    };
    let app = App::new(cfg.clone(), temp.0.join("config.yaml"));
    let accounts = app.pool.all();
    let find = |name: &str| accounts.iter().find(|a| a.label.contains(name)).unwrap().clone();
    let (cybex, itpals, digitalbrain) = (find("cybex"), find("itpals"), find("digitalbrain"));
    let now = Utc::now();
    for (a, days, short, weekly) in [(&cybex, 4, 86.0, 35.0), (&itpals, 5, 0.0, 4.0), (&digitalbrain, 6, 0.0, 3.0)] {
        crate::quota::authoritative(
            &mut a.state.lock(),
            vec![
                Window {
                    name: "5h".into(),
                    used: short,
                    resets_at: Some(now + chrono::Duration::hours(4)),
                    model: None,
                },
                Window {
                    name: "week".into(),
                    used: weekly,
                    resets_at: Some(now + chrono::Duration::days(days)),
                    model: None,
                },
            ],
            None,
        );
    }
    digitalbrain.state.lock().banked_resets =
        Some(reset_view(provider::claude(&expiring_usage(3.0, 0.0, 122), now).unwrap()));
    let load = HashMap::from([(cybex.id.clone(), 1)]);
    let pick = |cfg: &crate::config::Config| match app.pool.pick("claude-opus-5-5", &[], cfg, None, None, &load) {
        Pick::Ok(a, _) => a.id.clone(),
        _ => panic!("expected an available account"),
    };
    assert_eq!(pick(&cfg), digitalbrain.id);
    let order = app.pool.route_order("claude-opus-5-5", &cfg, None, &load);
    assert_eq!(order.iter().map(|s| &s.account).collect::<Vec<_>>(), [&digitalbrain.id, &itpals.id, &cybex.id]);
    assert!(order[0].banked_reset_expires_at.is_some());
    cfg.banked_reset_policy.expiry_priority_hours = 24;
    assert_eq!(pick(&cfg), itpals.id);
    cfg.banked_reset_policy.expiry_priority_hours = 168;
    let first = app.sessions.pick_with_reason(&app.pool, &cfg, "claude-opus-5-5", Some("ongoing"), &[], None).unwrap();
    assert_eq!(first.account.id, digitalbrain.id);
    assert_eq!(first.reason, "expiring_reset");
    digitalbrain.state.lock().quota.windows[0].used = 80.0;
    assert_eq!(
        app.sessions
            .pick_with_reason(&app.pool, &cfg, "claude-opus-5-5", Some("ongoing"), &[], None)
            .unwrap()
            .account
            .id,
        digitalbrain.id
    );
    assert_ne!(
        app.sessions.pick_with_reason(&app.pool, &cfg, "claude-opus-5-5", Some("new"), &[], None).unwrap().account.id,
        digitalbrain.id
    );
    digitalbrain.state.lock().quota.windows[0].used = 0.0;
    digitalbrain.state.lock().banked_resets.as_mut().unwrap().checked_at -= chrono::Duration::hours(1);
    assert_eq!(pick(&cfg), itpals.id, "stale inventory must not keep an account prioritized");
}

#[tokio::test]
async fn automatic_worker_serializes_duplicate_credentials_and_refreshes_restored_quota() {
    use axum::{
        Router,
        body::Body,
        extract::State,
        http::{Request as HttpRequest, StatusCode},
        response::IntoResponse,
        routing::any,
    };
    #[derive(Clone)]
    struct Backend {
        usage: Value,
        posts: Arc<AtomicUsize>,
        root: PathBuf,
    }
    async fn backend(State(b): State<Backend>, req: HttpRequest<Body>) -> axum::response::Response {
        let path = req.uri().path_and_query().unwrap().as_str();
        if path == "/api/oauth/profile" {
            return axum::Json(
                json!({"account":{"uuid":"account-1"},"organization":{"uuid":"11111111-1111-4111-8111-111111111111"}}),
            )
            .into_response();
        }
        if path == "/api/oauth/usage?cedar_ember=1&skip_spend=1" {
            let mut usage = b.usage.clone();
            if b.posts.load(Ordering::SeqCst) > 0 {
                usage["five_hour"]["utilization"] = json!(0);
                usage["seven_day"]["utilization"] = json!(0);
                usage["cedar_ember"]["grants"][0]["resets_left"] = json!(0);
            }
            return axum::Json(usage).into_response();
        }
        if path == "/api/organizations/11111111-1111-4111-8111-111111111111/reset_rate_limits" {
            assert_eq!(req.method(), axum::http::Method::POST);
            let saved = ledger::last_operation(&b.root, Provider::Claude, "account-1").unwrap().unwrap();
            assert_eq!(saved.status, "pending", "the durable intent must precede HTTP dispatch");
            assert!(saved.automatic);
            assert!(saved.auto_credit.is_some());
            assert_eq!(b.posts.fetch_add(1, Ordering::SeqCst), 0, "one automatic spend across duplicate files");
            return axum::Json(json!({"result":"reset"})).into_response();
        }
        StatusCode::NOT_FOUND.into_response()
    }
    let (temp, app, acct) = fixture(Provider::Claude, 2);
    let mut usage = expiring_usage(0.0, 0.0, 1);
    usage["cedar_ember"]["grants"][0]["resets_total"] = json!(1);
    usage["cedar_ember"]["grants"][0]["resets_left"] = json!(1);
    let b = Backend { usage, posts: Arc::new(AtomicUsize::new(0)), root: temp.0.clone() };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    *app.reset_test_origin.lock() = Some(format!("http://{}", listener.local_addr().unwrap()));
    let server =
        tokio::spawn(axum::serve(listener, Router::new().fallback(any(backend)).with_state(b.clone())).into_future());
    refresh(&app, &acct).await.unwrap();
    assert!(!automatic_check(&app, &acct).await, "automatic use is off by default");
    let mut cfg = (*app.cfg()).clone();
    cfg.routing = crate::config::Routing::SmartQuota;
    cfg.banked_reset_policy.auto_redeem = true;
    app.set_config(cfg);
    let duplicate = app.pool.all().into_iter().find(|a| a.id != acct.id).unwrap();
    let (one, two) = tokio::join!(automatic_check(&app, &acct), automatic_check(&app, &duplicate));
    assert!(one || two);
    assert_eq!(b.posts.load(Ordering::SeqCst), 1);
    for account in app.pool.all() {
        let st = account.state.lock();
        assert_eq!(st.quota.weekly_remaining("claude-opus-5-5"), Some(100.0));
        assert_eq!(st.quota.five_hour_remaining("claude-opus-5-5"), Some(100.0));
        assert_eq!(st.banked_resets.as_ref().unwrap().inventory.as_ref().unwrap().available, Some(0));
        assert!(!st.quota_refreshing);
    }
    assert!(!automatic_check(&app, &acct).await);
    assert_eq!(b.posts.load(Ordering::SeqCst), 1);
    server.abort();
}

#[test]
fn codex_counts_and_expiries() {
    let usage = json!({"rate_limit_reset_credits":{"available_count":2,"applicable_available_count":0},"credits":{"balance":123}});
    let details = json!({"credits":[{"reset_type":"codex_rate_limits","status":"available","expires_at":"2099-01-01T00:00:00Z"}]});
    let i = provider::codex(&usage, &details, Utc::now()).unwrap();
    assert_eq!(i.available, Some(2));
    assert_eq!(i.applicable, Some(0));
    assert!(i.eligible);
    assert!(i.grants[0].usable);
    assert!(i.reason.is_none());
    assert_eq!(provider::codex(&json!({}), &json!({"credits":[]}), Utc::now()).unwrap().available, Some(0));
    assert!(
        provider::codex(
            &json!({}),
            &json!({"credits":[{"id":"x","reset_type":"codex_rate_limits","status":"available","expires_at":"bad"}]}),
            Utc::now()
        )
        .is_err()
    );
    assert!(provider::codex(&json!({}), &json!({"available_count":-1}), Utc::now()).is_err());
    let i = provider::codex(&json!({}), &json!({"credits":[{"id":"x","status":"available","reset_type":"codex_rate_limits","expires_at":"2000-01-01T00:00:00Z"}]}), Utc::now()).unwrap();
    assert_eq!(i.available, Some(0));
    assert!(!i.eligible);
}
#[test]
fn codex_manual_reset_uses_available_credits_and_checks_expiry() {
    let usage = json!({"rate_limit_reset_credits":{"available_count":1,"applicable_available_count":0},"rate_limit":{"allowed":true,"limit_reached":false}});
    let mut details = json!({"available_count":1,"credits":[{"id":"saved-credit","reset_type":"codex_rate_limits","status":"available","expires_at":"2099-01-01T00:00:00Z"}]});
    let inventory = provider::codex(&usage, &details, Utc::now()).unwrap();
    assert_eq!(inventory.applicable, Some(0));
    assert!(inventory.eligible);
    assert!(inventory.grants[0].usable);
    details["credits"][0]["expires_at"] = json!("2000-01-01T00:00:00Z");
    let expired = provider::codex(&usage, &details, Utc::now()).unwrap();
    assert!(!expired.eligible);
    assert!(!expired.grants[0].usable);
    assert_eq!(expired.reason.as_deref(), Some("Available resets have expired"));
    details["available_count"] = json!(0);
    assert!(!provider::codex(&usage, &details, Utc::now()).unwrap().eligible);
}
#[tokio::test]
async fn turned_off_resets_never_reach_the_provider() {
    use axum::{Router, body::Body, http::Request, routing::any};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static HITS: AtomicUsize = AtomicUsize::new(0);
    async fn backend(_: Request<Body>) -> axum::Json<Value> {
        HITS.fetch_add(1, Ordering::SeqCst);
        axum::Json(json!({}))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider_origin = format!("http://{}", listener.local_addr().unwrap());
    let provider_server = tokio::spawn(axum::serve(listener, Router::new().fallback(any(backend))).into_future());
    let (temp, app, acct) = fixture(Provider::Claude, 1);
    *app.reset_test_origin.lock() = Some(provider_origin);
    let mut config = (*app.cfg()).clone();
    config.management_key = "test-management-key".into();
    config.banked_resets = false;
    app.set_config(config);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::mgmt::router(app.clone()).with_state(app.clone());
    let server = tokio::spawn(
        axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).into_future(),
    );
    let client = reqwest::Client::new();
    let path = format!("{origin}/accounts/{}/banked-resets", acct.id);
    let read = client.get(&path).bearer_auth("test-management-key").send().await.unwrap();
    assert_eq!(read.status(), reqwest::StatusCode::NOT_FOUND);
    let spend = client
        .post(&path)
        .bearer_auth("test-management-key")
        .json(&json!({"action":"redeem","request_id":uuid::Uuid::new_v4().to_string(),"confirmed":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(spend.status(), reqwest::StatusCode::NOT_FOUND);
    assert_eq!(HITS.load(Ordering::SeqCst), 0);
    assert!(!temp.0.join(".banked-resets").exists());
    server.abort();
    provider_server.abort();
}

#[tokio::test]
async fn codex_available_credit_returns_confirmation_quote_without_spending() {
    use axum::{Router, body::Body, http::Request, routing::any};
    async fn backend(req: Request<Body>) -> axum::Json<Value> {
        assert_eq!(req.method(), axum::http::Method::GET, "Inventory must never spend a credit");
        axum::Json(match req.uri().path() {
            "/backend-api/wham/usage" => json!({
                "rate_limit_reset_credits":{"available_count":1,"applicable_available_count":0},
                "rate_limit":{"allowed":true,"limit_reached":false}
            }),
            "/backend-api/wham/rate-limit-reset-credits" => json!({
                "available_count":1,
                "credits":[{"id":"saved-credit","reset_type":"codex_rate_limits","status":"available","expires_at":"2099-01-01T00:00:00Z"}]
            }),
            _ => panic!("Unexpected provider request"),
        })
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider_origin = format!("http://{}", listener.local_addr().unwrap());
    let provider_server = tokio::spawn(axum::serve(listener, Router::new().fallback(any(backend))).into_future());
    let (_temp, app, acct) = fixture(Provider::Codex, 1);
    *app.reset_test_origin.lock() = Some(provider_origin);
    let mut config = (*app.cfg()).clone();
    config.management_key = "test-management-key".into();
    app.set_config(config);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::mgmt::router(app.clone()).with_state(app.clone());
    let server = tokio::spawn(
        axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).into_future(),
    );
    let client = reqwest::Client::new();
    let path = format!("{origin}/accounts/{}/banked-resets", acct.id);
    let response = client.get(&path).bearer_auth("test-management-key").send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let view: Value = response.json().await.unwrap();
    assert_eq!(view["inventory"]["applicable"], 0);
    assert_eq!(view["inventory"]["eligible"], true);
    let quote = view["quote"].as_str().unwrap();
    let denied = client
        .post(&path)
        .bearer_auth("test-management-key")
        .json(&json!({"action":"redeem","request_id":quote,"confirmed":false}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::CONFLICT);
    assert!(ledger::last_operation(&app.startup_config.auth_dir(), Provider::Codex, "account-1").unwrap().is_none());
    server.abort();
    provider_server.abort();
}
#[test]
fn claude_eligibility_and_selection() {
    let mut usage = claude_usage();
    let now = Utc::now();
    assert_eq!(provider::claude(&usage, now).unwrap().selected_grant.as_deref(), Some("grant-1"));
    for (key, value) in [
        ("paused", json!(true)),
        ("usable_now", json!(false)),
        ("ends_at", json!("2000-01-01T00:00:00Z")),
        ("starts_at", json!("2099-01-01T00:00:00Z")),
        ("resets_left", json!(0)),
        ("clears", json!(["unknown_window"])),
    ] {
        let mut v = usage.clone();
        v["cedar_ember"]["grants"][0][key] = value;
        assert!(!provider::claude(&v, now).unwrap().eligible, "{key}");
    }
    usage["cedar_ember"]["at_limit"] = json!(false);
    assert!(!provider::claude(&usage, now).unwrap().eligible);
    usage["cedar_ember"]["grants"][0]["use_requires_limit"] = json!(false);
    assert!(provider::claude(&usage, now).unwrap().eligible);
    usage["cedar_ember"]["cooldown_until"] = json!("2099-01-01T00:00:00Z");
    assert!(!provider::claude(&usage, now).unwrap().eligible);
    assert!(provider::claude(&json!({}), now).is_err());
    assert!(provider::claude(&json!({"cedar_ember":{"eligible":false}}), now).is_ok());
    let mut v = claude_usage();
    v["cedar_ember"]["grants"][0]["ends_at"] = Value::Null;
    let mut dated = v["cedar_ember"]["grants"][0].clone();
    dated["id"] = json!("dated");
    dated["ends_at"] = json!("2098-01-01T00:00:00Z");
    v["cedar_ember"]["grants"].as_array_mut().unwrap().push(dated);
    assert_eq!(provider::claude(&v, now).unwrap().selected_grant.as_deref(), Some("dated"));
    v["cedar_ember"]["next_grant_id"] = json!("grant-1");
    assert_eq!(provider::claude(&v, now).unwrap().selected_grant.as_deref(), Some("grant-1"));
    v["cedar_ember"]["grants"][1]["id"] = json!("grant-1");
    assert!(provider::claude(&v, now).is_err());
}
#[test]
fn journal_is_durable_exclusive_and_fails_closed() {
    let temp = Temp::new();
    let mut ledger = Ledger::open(&temp.0, "claude:org").unwrap();
    assert!(Ledger::open(&temp.0, "claude:org").is_err());
    let id = uuid::Uuid::new_v4().to_string();
    let o = Operation {
        request_id: id.clone(),
        account: "account".into(),
        provider: "claude".into(),
        grant_id: "g".into(),
        clears: vec!["5h".into()],
        created_at: Utc::now(),
        updated_at: Utc::now(),
        status: "pending".into(),
        message: "Pending".into(),
        automatic: false,
        auto_credit: None,
    };
    ledger.journal.operations.insert(id.clone(), o);
    ledger.save().unwrap();
    drop(ledger);
    let ledger = Ledger::open(&temp.0, "claude:org").unwrap();
    assert!(ledger.journal.unresolved().is_some());
    assert_eq!(ledger::last_operation(&temp.0, Provider::Claude, "account").unwrap().unwrap().request_id, id);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let name = hex::encode(Sha256::digest(b"claude:org"));
        let file = temp.0.join(".banked-resets").join(format!("{name}.json"));
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::write(file, b"corrupt").unwrap();
    }
    drop(ledger);
    #[cfg(unix)]
    assert!(Ledger::open(&temp.0, "claude:org").is_err());
}
#[tokio::test]
async fn double_click_and_stale_second_quote_do_not_spend_twice() {
    let (_temp, app, acct) = fixture(Provider::Claude, 2);
    let mock = Mock::new(&[Outcome::Applied]);
    let mut ledger = Ledger::open(&app.startup_config.auth_dir(), "claude:organization-1").unwrap();
    let id = quote(&app, &mock, &ledger);
    let second = quote(&app, &mock, &ledger);
    let v = run(&app, &acct, &mock, &mut ledger, action(&id, "redeem")).await.unwrap();
    assert_eq!(v.operation.unwrap().status, "applied");
    run(&app, &acct, &mock, &mut ledger, action(&id, "redeem")).await.unwrap();
    let duplicate = app.pool.all().into_iter().find(|a| a.id != acct.id).unwrap();
    assert!(run(&app, &duplicate, &mock, &mut ledger, action(&second, "redeem")).await.is_err());
    assert_eq!(mock.calls.lock().len(), 1);
}
#[tokio::test]
async fn unknown_outcome_survives_restart_and_reuses_id() {
    let (_temp, app, acct) = fixture(Provider::Claude, 1);
    let mock = Mock::new(&[Outcome::Unknown, Outcome::Refused("Unauthorized"), Outcome::AlreadyUsed]);
    let mut ledger = Ledger::open(&app.startup_config.auth_dir(), "claude:organization-1").unwrap();
    let id = quote(&app, &mock, &ledger);
    assert_eq!(
        run(&app, &acct, &mock, &mut ledger, action(&id, "redeem")).await.unwrap().operation.unwrap().status,
        "unknown"
    );
    drop(ledger);
    let mut ledger = Ledger::open(&app.startup_config.auth_dir(), "claude:organization-1").unwrap();
    let second = quote(&app, &mock, &ledger);
    assert!(run(&app, &acct, &mock, &mut ledger, action(&second, "redeem")).await.is_err());
    assert_eq!(
        run(&app, &acct, &mock, &mut ledger, action(&id, "retry")).await.unwrap().operation.unwrap().status,
        "unknown"
    );
    assert_eq!(
        run(&app, &acct, &mock, &mut ledger, action(&id, "retry")).await.unwrap().operation.unwrap().status,
        "already_used"
    );
    assert_eq!(*mock.calls.lock(), vec![id.clone(), id.clone(), id]);
}
#[tokio::test]
async fn expired_unknown_needs_explicit_resolution() {
    let (_temp, app, acct) = fixture(Provider::Claude, 1);
    let mock = Mock::new(&[Outcome::Unknown]);
    let mut ledger = Ledger::open(&app.startup_config.auth_dir(), "claude:organization-1").unwrap();
    let id = quote(&app, &mock, &ledger);
    run(&app, &acct, &mock, &mut ledger, action(&id, "redeem")).await.unwrap();
    ledger.journal.operations.get_mut(&id).unwrap().created_at -= chrono::Duration::minutes(11);
    ledger.save().unwrap();
    assert!(run(&app, &acct, &mock, &mut ledger, action(&id, "retry")).await.is_err());
    let mut a = action(&id, "resolve-unused");
    a.confirmed = false;
    assert!(run(&app, &acct, &mock, &mut ledger, a).await.is_err());
    let v = run(&app, &acct, &mock, &mut ledger, action(&id, "resolve-unused")).await.unwrap();
    assert_eq!(v.operation.unwrap().status, "reconciled_unused");
    assert!(ledger.journal.unresolved().is_none());
    assert_eq!(mock.calls.lock().len(), 1);
}
#[tokio::test]
async fn disabled_changed_identity_or_inventory_cannot_spend() {
    let (_temp, app, acct) = fixture(Provider::Claude, 1);
    let mut mock = Mock::new(&[Outcome::Applied]);
    let mut ledger = Ledger::open(&app.startup_config.auth_dir(), "claude:organization-1").unwrap();
    let id = quote(&app, &mock, &ledger);
    acct.state.lock().disabled = true;
    assert!(run(&app, &acct, &mock, &mut ledger, action(&id, "redeem")).await.is_err());
    acct.state.lock().disabled = false;
    if let Credential::OAuth(o) = &mut *acct.cred.write() {
        o.account_id = Some("different-account".into());
    }
    assert!(run(&app, &acct, &mock, &mut ledger, action(&id, "redeem")).await.is_err());
    if let Credential::OAuth(o) = &mut *acct.cred.write() {
        o.account_id = Some("account-1".into());
    }
    mock.inventory.applicable = Some(0);
    assert!(run(&app, &acct, &mock, &mut ledger, action(&id, "redeem")).await.is_err());
    assert!(mock.calls.lock().is_empty());
    assert!(ledger.journal.operations.is_empty());
}
#[tokio::test]
async fn applied_reset_survives_refresh_failure_and_preserves_other_cooldowns() {
    let (_temp, app, acct) = fixture(Provider::Claude, 2);
    let mut mock = Mock::new(&[Outcome::Applied]);
    mock.fail_after = 1;
    for a in app.pool.all() {
        let mut st = a.state.lock();
        crate::quota::usage(&mut st, Provider::Claude, &claude_usage());
        st.quota_cooldowns.insert("claude-sonnet-5-5".into(), Utc::now() + chrono::Duration::hours(1));
        st.cooldowns.insert("claude-opus-5-5".into(), Utc::now() + chrono::Duration::minutes(1));
    }
    let epoch = acct.quota_epoch();
    let mut ledger = Ledger::open(&app.startup_config.auth_dir(), "claude:organization-1").unwrap();
    let id = quote(&app, &mock, &ledger);
    let v = run(&app, &acct, &mock, &mut ledger, action(&id, "redeem")).await.unwrap();
    assert_eq!(v.operation.unwrap().status, "applied");
    assert!(v.error.unwrap().contains("refresh failed"));
    for a in app.pool.all() {
        let st = a.state.lock();
        assert!(!st.quota.windows.iter().any(|w| w.name == "5h"));
        assert!(st.quota.refreshed_at.is_none()); // failed reconciliation must not delay the next poll
        assert!(!st.quota_refreshing);
        assert!(st.quota_cooldowns.is_empty());
        assert!(!st.cooldowns.is_empty());
    }
    let event = json!({"type":"codex.rate_limits","primary":{"window_minutes":300,"used_percent":100,"reset_at":4102444800i64}});
    crate::quota::observe_codex_event(&acct, &event, epoch);
    assert!(!acct.state.lock().quota.windows.iter().any(|w| w.name == "5h"));
    drop(ledger);
    assert_eq!(
        Ledger::open(&app.startup_config.auth_dir(), "claude:organization-1").unwrap().journal.latest().unwrap().status,
        "applied"
    );
}
#[test]
fn a_reset_clears_exhaustion_the_proxy_detected() {
    let (_temp, app, acct) = fixture(Provider::Claude, 1);
    let body =
        r#"{"error":{"type":"rate_limit_error","code":"usage_limit_reached","message":"You've hit your usage limit"}}"#;
    assert!(crate::proxy::quota_exhausted(&acct, "claude-sonnet-5-5", 429, body));
    crate::proxy::mark_quota_exhausted(&acct, "claude-sonnet-5-5", &reqwest::header::HeaderMap::new(), body);
    assert!(acct.exhausted_until("claude-sonnet-5-5").is_some());
    assert!(acct.cooling_until("claude-sonnet-5-5").is_some());
    assert!(acct.state.lock().last_error(Utc::now()).is_some());
    // Applying a reset and reading fresh usage makes the account routable again.
    let guard = QuotaGuard::new(&app, &acct, "account-1");
    guard.invalidate(&["5h".into()]);
    drop(guard);
    let usage = json!({"five_hour":{"utilization":0,"resets_at":"2099-01-01T00:00:00Z"},"seven_day":{"utilization":10,"resets_at":"2099-01-01T00:00:00Z"}});
    reconcile(&app, &acct, "account-1", &usage);
    assert!(acct.exhausted_until("claude-sonnet-5-5").is_none());
    assert!(acct.cooling_until("claude-sonnet-5-5").is_none());
    assert_eq!(acct.state.lock().last_error(Utc::now()), None, "the reset ends the quota error");
}

#[test]
fn authoritative_refresh_recovers_routing_and_retains_overload() {
    let (_temp, app, acct) = fixture(Provider::Claude, 1);
    crate::quota::usage(&mut acct.state.lock(), Provider::Claude, &claude_usage());
    acct.cool_quota("claude-sonnet-5-5", Utc::now() + chrono::Duration::hours(2), "quota", acct.quota_epoch());
    acct.cool(Some("claude-opus-5-5"), Utc::now() + chrono::Duration::minutes(1), "overloaded");
    let usage = json!({"five_hour":{"utilization":0,"resets_at":"2099-01-01T00:00:00Z"},"seven_day":{"utilization":10,"resets_at":"2099-01-01T00:00:00Z"}});
    reconcile(&app, &acct, "account-1", &usage);
    assert!(acct.cooling_until("claude-sonnet-5-5").is_none());
    assert!(acct.cooling_until("claude-opus-5-5").is_some());
    let quota_error = |body: &str| crate::proxy::quota_exhausted(&acct, "claude-opus-5-5", 429, body);
    assert!(!quota_error(r#"{"error":{"type":"overloaded_error"}}"#));
    assert!(quota_error(r#"{"type":"response.failed","response":{"error":{"code":"usage_limit_reached"}}}"#));
}

#[tokio::test]
async fn provider_http_payloads_headers_and_refusals() {
    use axum::{
        Router,
        body::Body,
        extract::State,
        http::{Request, StatusCode},
        response::IntoResponse,
        routing::any,
    };
    #[derive(Clone)]
    struct Capture {
        received: Arc<Mutex<Vec<(String, Value, axum::http::HeaderMap)>>>,
        result: Arc<Mutex<(u16, Value)>>,
    }
    async fn handle(State(c): State<Capture>, req: Request<Body>) -> axum::response::Response {
        let path = req.uri().to_string();
        let headers = req.headers().clone();
        let body = axum::body::to_bytes(req.into_body(), 1 << 20).await.unwrap();
        let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
        c.received.lock().push((path, body, headers));
        let (status, body) = c.result.lock().clone();
        let mut response = (StatusCode::from_u16(status).unwrap(), axum::Json(body)).into_response();
        if status == 307 {
            response.headers_mut().insert("location", "/redirected".parse().unwrap());
        }
        response
    }
    let captured = Capture {
        received: Arc::new(Mutex::new(vec![])),
        result: Arc::new(Mutex::new((200, json!({"result":"reset"})))),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(listener, Router::new().fallback(any(handle)).with_state(captured.clone())).into_future(),
    );
    let (_temp, app, _) = fixture(Provider::Claude, 1);
    let mut provider = HttpProvider {
        provider: Provider::Claude,
        client: app.http.for_reset(Some("direct")),
        token: "mock-token".into(),
        account_id: "account-1".into(),
        origin: Some(origin),
    };
    let org = "11111111-1111-4111-8111-111111111111";
    let request = uuid::Uuid::new_v4().to_string();
    assert_eq!(provider.redeem(org, "grant-1", &request).await.unwrap(), Outcome::Applied);
    let (path, body, headers) = captured.received.lock()[0].clone();
    assert_eq!(path, format!("/api/organizations/{org}/reset_rate_limits"));
    assert_eq!(body, json!({"program":"cedar_ember","grant_id":"grant-1","request_id":request}));
    assert_eq!(headers["authorization"], "Bearer mock-token");
    assert_eq!(headers["anthropic-beta"], "oauth-2025-04-20");
    for (status, body, outcome) in [
        (403, json!({}), Outcome::Refused("Provider refused authorization")),
        (429, json!({}), Outcome::Refused("Provider rate limited the reset")),
        (503, json!({}), Outcome::Unknown),
        (200, json!({"result":"new_result"}), Outcome::Unknown),
        (200, json!({"result":"already_used"}), Outcome::AlreadyUsed),
    ] {
        *captured.result.lock() = (status, body);
        assert_eq!(provider.redeem(org, "grant-1", &request).await.unwrap(), outcome);
    }
    *captured.result.lock() = (200, json!({"account":{"uuid":"another-account"},"organization":{"uuid":org}}));
    assert!(provider.identity().await.is_err());
    *captured.result.lock() = (200, json!({"account":{"uuid":"account-1"},"organization":{"uuid":org}}));
    assert_eq!(provider.identity().await.unwrap(), org);
    provider.provider = Provider::Codex;
    *captured.result.lock() = (204, Value::Null);
    assert_eq!(provider.redeem("", "", &request).await.unwrap(), Outcome::Applied);
    let (path, body, headers) = captured.received.lock().last().unwrap().clone();
    assert_eq!(path, "/backend-api/wham/rate-limit-reset-credits/consume");
    assert_eq!(body, json!({"redeem_request_id":request}));
    assert_eq!(headers["chatgpt-account-id"], "account-1");
    assert_eq!(headers["openai-beta"], "codex-1");
    *captured.result.lock() = (307, json!({}));
    assert_eq!(provider.redeem("", "", &request).await.unwrap(), Outcome::Unknown);
    // Redirects and retries must not generate additional spending requests.
    assert_eq!(captured.received.lock().len(), 10);
    server.abort();
}
#[tokio::test]
async fn pending_entry_precedes_dispatch_and_failed_save_never_dispatches() {
    struct Durable {
        root: PathBuf,
        calls: AtomicUsize,
    }
    impl Api for Durable {
        async fn read(&self) -> Result<(Inventory, Value)> {
            Ok((provider::claude(&claude_usage(), Utc::now()).unwrap(), claude_usage()))
        }
        async fn identity(&self) -> Result<String> {
            Ok("organization-1".into())
        }
        async fn redeem(&self, _: &str, _: &str, id: &str) -> Result<Outcome> {
            let saved = ledger::last_operation(&self.root, Provider::Claude, "account-1")?.unwrap();
            ensure!(saved.request_id == id && saved.status == "pending", "Journal not persisted before dispatch");
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(Outcome::Applied)
        }
    }
    let (_temp, app, acct) = fixture(Provider::Claude, 1);
    let mock = Mock::new(&[]);
    let durable = Durable { root: app.startup_config.auth_dir(), calls: AtomicUsize::new(0) };
    let mut ledger = Ledger::open(&durable.root, "claude:organization-1").unwrap();
    let id = quote(&app, &mock, &ledger);
    execute(&app, &acct, &durable, "organization-1", "account-1", &mut ledger, Request::Manual(action(&id, "redeem")))
        .await
        .unwrap();
    assert_eq!(durable.calls.load(Ordering::Relaxed), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let id = quote(&app, &mock, &ledger);
        let dir = durable.root.join(".banked-resets");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = execute(
            &app,
            &acct,
            &durable,
            "organization-1",
            "account-1",
            &mut ledger,
            Request::Manual(action(&id, "redeem")),
        )
        .await;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.is_err());
        assert_eq!(durable.calls.load(Ordering::Relaxed), 1);
    }
}
#[tokio::test]
async fn refusal_is_saved_and_can_be_replayed_without_spending() {
    let (_temp, app, acct) = fixture(Provider::Claude, 1);
    let mock = Mock::new(&[Outcome::Refused("Ineligible")]);
    let mut ledger = Ledger::open(&app.startup_config.auth_dir(), "claude:organization-1").unwrap();
    let id = quote(&app, &mock, &ledger);
    let v = run(&app, &acct, &mock, &mut ledger, action(&id, "redeem")).await.unwrap();
    assert_eq!(v.operation.unwrap().status, "refused");
    assert!(ledger.journal.unresolved().is_none());
    run(&app, &acct, &mock, &mut ledger, action(&id, "retry")).await.unwrap();
    assert_eq!(mock.calls.lock().len(), 1);
}
#[test]
fn views_do_not_disclose_tokens_or_provider_account_identity() {
    let temp = Temp::new();
    let mut ledger = Ledger::open(&temp.0, "claude:org").unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    ledger.journal.operations.insert(
        id.clone(),
        Operation {
            request_id: id,
            account: "private-account-id".into(),
            provider: "claude".into(),
            grant_id: "grant".into(),
            clears: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            status: "unknown".into(),
            message: "Unknown".into(),
            automatic: false,
            auto_credit: None,
        },
    );
    let serialized = serde_json::to_string(&view(&ledger, None, None, None)).unwrap();
    assert!(!serialized.contains("private-account-id"));
    assert!(!serialized.contains("token"));
}

#[tokio::test]
async fn management_request_survives_disconnect_and_serializes_tabs() {
    use axum::{
        Router,
        body::Body,
        extract::State,
        http::{Request, StatusCode},
        response::IntoResponse,
        routing::any,
    };
    use tokio::sync::Notify;
    #[derive(Clone)]
    struct Backend {
        entered: Arc<Notify>,
        release: Arc<Notify>,
        posts: Arc<AtomicUsize>,
    }
    async fn backend(State(b): State<Backend>, req: Request<Body>) -> axum::response::Response {
        let path = req.uri().to_string();
        if req.method() == axum::http::Method::POST {
            let body = axum::body::to_bytes(req.into_body(), 1024).await.unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["program"], "cedar_ember");
            assert_eq!(body["grant_id"], "grant-1");
            assert!(uuid::Uuid::parse_str(body["request_id"].as_str().unwrap()).is_ok());
            b.posts.fetch_add(1, Ordering::Relaxed);
            b.entered.notify_one();
            b.release.notified().await;
            return axum::Json(json!({"result":"reset"})).into_response();
        }
        if path == "/api/oauth/profile" {
            return axum::Json(
                json!({"account":{"uuid":"account-1"},"organization":{"uuid":"11111111-1111-4111-8111-111111111111"}}),
            )
            .into_response();
        }
        if path == "/api/oauth/usage?cedar_ember=1&skip_spend=1" {
            let mut usage = claude_usage();
            if b.posts.load(Ordering::Relaxed) > 0 {
                usage["five_hour"]["utilization"] = json!(0);
                usage["cedar_ember"]["grants"][0]["resets_left"] = json!(1);
            }
            return axum::Json(usage).into_response();
        }
        StatusCode::NOT_FOUND.into_response()
    }
    let b = Backend {
        entered: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
        posts: Arc::new(AtomicUsize::new(0)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider_origin = format!("http://{}", listener.local_addr().unwrap());
    let provider_server =
        tokio::spawn(axum::serve(listener, Router::new().fallback(any(backend)).with_state(b.clone())).into_future());
    let (_temp, app, acct) = fixture(Provider::Claude, 2);
    *app.reset_test_origin.lock() = Some(provider_origin);
    let mut config = (*app.cfg()).clone();
    config.management_key = "test-management-key".into();
    app.set_config(config);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::mgmt::router(app.clone()).with_state(app.clone());
    let server = tokio::spawn(
        axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).into_future(),
    );
    let client = reqwest::Client::new();
    let path = format!("{origin}/accounts/{}/banked-resets", acct.id);
    let unauthorized = client.get(&path).send().await.unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("authorization", "Bearer test-management-key".parse().unwrap());
    let client = reqwest::Client::builder().default_headers(headers).build().unwrap();
    let quote: Value = client.get(&path).send().await.unwrap().json().await.unwrap();
    let id = quote["quote"].as_str().unwrap().to_string();
    assert_eq!(quote["inventory"]["available"], 2);
    let denied = client
        .post(&path)
        .json(&json!({"action":"redeem","request_id":id,"grant_id":"grant-1","confirmed":false}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::CONFLICT);
    assert_eq!(b.posts.load(Ordering::Relaxed), 0);
    let body = json!({"action":"redeem","request_id":id,"grant_id":"grant-1","confirmed":true});
    let (c, p, body2) = (client.clone(), path.clone(), body.clone());
    let claim = tokio::spawn(async move { c.post(p).json(&body2).send().await });
    tokio::time::timeout(std::time::Duration::from_secs(2), b.entered.notified()).await.unwrap();
    let saved = ledger::last_operation(&app.startup_config.auth_dir(), Provider::Claude, "account-1").unwrap().unwrap();
    assert_eq!(saved.status, "pending");
    let duplicate = app.pool.all().into_iter().find(|a| a.id != acct.id).unwrap();
    let conflict =
        client.post(format!("{origin}/accounts/{}/banked-resets", duplicate.id)).json(&body).send().await.unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(b.posts.load(Ordering::Relaxed), 1);
    // Cancel the waiting client while the provider is processing. The detached server task finishes.
    claim.abort();
    b.release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if acct
                .state
                .lock()
                .banked_resets
                .as_ref()
                .is_some_and(|v| v.operation.as_ref().is_some_and(|o| o.status == "applied"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let replay: Value = client.post(path).json(&body).send().await.unwrap().json().await.unwrap();
    assert_eq!(replay["operation"]["status"], "applied");
    assert_eq!(b.posts.load(Ordering::Relaxed), 1);
    assert!(acct.cooling_until("claude-sonnet-5-5").is_none());
    server.abort();
    provider_server.abort();
}

#[test]
fn partial_quota_refresh_preserves_missing_windows_and_scoped_limits() {
    let (_temp, _app, acct) = fixture(Provider::Claude, 1);
    let mut usage = claude_usage();
    usage["seven_day"]["utilization"] = json!(100);
    usage["seven_day_opus"] = json!({"utilization":100,"resets_at":"2099-01-01T00:00:00Z"});
    crate::quota::usage(&mut acct.state.lock(), Provider::Claude, &usage);
    acct.cool_quota("claude-sonnet-5-5", Utc::now() + chrono::Duration::hours(1), "quota", acct.quota_epoch());
    crate::quota::usage(
        &mut acct.state.lock(),
        Provider::Claude,
        &json!({"five_hour":{"utilization":0,"resets_at":"2099-01-01T00:00:00Z"}}),
    );
    assert!(acct.cooling_until("claude-sonnet-5-5").is_some());
    crate::quota::usage(
        &mut acct.state.lock(),
        Provider::Claude,
        &json!({"five_hour":{"utilization":0,"resets_at":"2099-01-01T00:00:00Z"},"seven_day":{"utilization":0,"resets_at":"2099-01-01T00:00:00Z"}}),
    );
    assert!(acct.cooling_until("claude-sonnet-5-5").is_none());
    assert!(acct.cooling_until("claude-opus-5-5").is_some());
}
#[tokio::test]
async fn codex_ambiguous_result_requires_reconciliation() {
    let (_temp, app, acct) = fixture(Provider::Codex, 1);
    let mock = Mock::new(&[Outcome::Unknown]);
    let mut ledger = Ledger::open(&app.startup_config.auth_dir(), "codex:organization-1").unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    app.reset_quotes.lock().insert(
        id.clone(),
        Quote {
            account: "account-1".into(),
            identity: "codex:organization-1".into(),
            version: 0,
            fingerprint: fingerprint(&mock.inventory),
            expires: Utc::now() + chrono::Duration::minutes(2),
        },
    );
    let mut a = action(&id, "redeem");
    a.grant_id.clear();
    let v = run(&app, &acct, &mock, &mut ledger, a).await.unwrap();
    assert_eq!(v.operation.unwrap().status, "unknown");
    assert!(!v.retryable);
    assert!(run(&app, &acct, &mock, &mut ledger, action(&id, "retry")).await.is_err());
    run(&app, &acct, &mock, &mut ledger, action(&id, "resolve-used")).await.unwrap();
    assert!(ledger.journal.unresolved().is_none());
    assert_eq!(mock.calls.lock().len(), 1);
}
#[test]
fn replacing_credential_identity_discards_old_quota_and_reset_state() {
    let (temp, app, acct) = fixture(Provider::Claude, 1);
    crate::quota::usage(&mut acct.state.lock(), Provider::Claude, &claude_usage());
    acct.state.lock().quota_refreshing = true;
    let path = temp.0.join("account-0.json");
    let mut raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    raw["account_id"] = json!("new-account");
    std::fs::write(path, raw.to_string()).unwrap();
    app.reload_accounts();
    assert!(Arc::ptr_eq(&app.pool.get(&acct.id).unwrap(), &acct));
    let st = acct.state.lock();
    assert!(st.quota.windows.is_empty());
    assert!(st.quota.refreshed_at.is_none());
    assert!(!st.quota_refreshing);
    assert!(st.banked_resets.is_none());
}

#[tokio::test]
async fn same_organization_cannot_retry_another_account_operation() {
    let (_temp, app, acct) = fixture(Provider::Claude, 2);
    let mock = Mock::new(&[Outcome::Unknown]);
    let mut ledger = Ledger::open(&app.startup_config.auth_dir(), "claude:organization-1").unwrap();
    let id = quote(&app, &mock, &ledger);
    run(&app, &acct, &mock, &mut ledger, action(&id, "redeem")).await.unwrap();
    let other = app.pool.all().into_iter().find(|a| a.id != acct.id).unwrap();
    if let Credential::OAuth(o) = &mut *other.cred.write() {
        o.account_id = Some("account-2".into());
    }
    for kind in ["retry", "resolve-used", "resolve-unused"] {
        assert!(
            execute(
                &app,
                &other,
                &mock,
                "organization-1",
                "account-2",
                &mut ledger,
                Request::Manual(action(&id, kind))
            )
            .await
            .is_err()
        );
    }
    assert_eq!(mock.calls.lock().len(), 1);
    assert!(ledger.journal.unresolved().is_some());
}
