use super::notices::Observation;
use super::tests::{claude_usage, fixture, reset_view};
use super::*;
use axum::{Json, Router, body::Body, extract::State, http::Request, response::IntoResponse, routing::any};
use parking_lot::Mutex;
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

fn credit(id: &str, now: DateTime<Utc>) -> Value {
    json!({"id":id,"reset_type":"codex_rate_limits","status":"available",
        "granted_at":now - chrono::Duration::days(1),"expires_at":now + chrono::Duration::days(7)})
}

fn codex(count: u64, credits: Value, now: DateTime<Utc>) -> Inventory {
    provider::codex(&json!({}), &json!({"available_count":count,"credits":credits}), now).unwrap()
}

#[test]
fn claude_receipts_follow_allocations_instead_of_balances_or_usability() {
    let now = Utc::now();
    let mut usage = claude_usage();
    let mut seen = Observation::default();
    let mut read = |usage: &Value| seen.update(&provider::claude(usage, now).unwrap(), now).count;
    assert_eq!(read(&usage), 0, "initial inventory is quiet");
    usage["cedar_ember"]["grants"][0]["resets_left"] = json!(1);
    assert_eq!(read(&usage), 0);
    usage["cedar_ember"]["grants"][0]["resets_left"] = json!(2);
    assert_eq!(read(&usage), 0, "a stale balance rebound is not a new allocation");
    usage["cedar_ember"]["grants"][0]["usable_now"] = json!(false);
    usage["cedar_ember"]["at_limit"] = json!(false);
    usage["cedar_ember"]["cooldown_until"] = json!(now + chrono::Duration::hours(1));
    assert_eq!(read(&usage), 0, "usability and quota changes are not receipts");
    usage["cedar_ember"]["grants"][0]["resets_total"] = json!(3);
    assert_eq!(read(&usage), 1, "an allocation increase is detected even at an unchanged balance");
    assert_eq!(read(&usage), 0);
    usage["cedar_ember"]["grants"] = json!([]);
    assert_eq!(read(&usage), 0);
    usage = claude_usage();
    assert_eq!(read(&usage), 0, "disappearance does not forget an old grant");
    usage["cedar_ember"]["grants"][0]["id"] = json!("new-grant");
    usage["cedar_ember"]["grants"][0]["starts_at"] = json!(now + chrono::Duration::days(1));
    usage["cedar_ember"]["grants"][0]["ends_at"] = Value::Null;
    assert_eq!(read(&usage), 2, "future-dated grants and unknown expiry still count as received");
}

#[test]
fn codex_receipts_find_replacements_and_keep_seen_ids_across_partial_reads() {
    let now = Utc::now();
    let mut seen = Observation::default();
    assert_eq!(seen.update(&codex(2, json!([credit("a", now), credit("b", now)]), now), now).count, 0);
    let next = codex(2, json!([credit("a", now), credit("c", now)]), now);
    assert_eq!(seen.update(&next, now).count, 1, "one credit arrived while another was spent");
    assert_eq!(seen.update(&codex(2, Value::Null, now), now).count, 0);
    assert_eq!(seen.update(&next, now).count, 0, "returning details must not repeat the receipt");
    assert_eq!(seen.update(&codex(1, json!([credit("b", now)]), now), now).count, 0);
    assert_eq!(seen.update(&codex(2, json!([credit("a", now), credit("b", now)]), now), now).count, 0);
    let mut expired = credit("expired", now);
    expired["expires_at"] = json!(now);
    assert_eq!(seen.update(&codex(0, json!([expired]), now), now).count, 0);
    let mut unknown_expiry = credit("no-expiry", now);
    unknown_expiry["expires_at"] = Value::Null;
    let received = seen.update(&codex(1, json!([unknown_expiry]), now), now);
    assert_eq!(received.count, 1);
    assert!(received.expires_at.is_none());
    let mut invalid_metadata = credit("bad-grant-time", now);
    invalid_metadata["granted_at"] = json!("invalid");
    let inventory = codex(1, json!([invalid_metadata]), now);
    assert!(inventory.eligible && inventory.grants[0].granted_at.is_none());
    assert_eq!(seen.update(&inventory, now).count, 1, "optional receipt metadata must not block redemption");
}

#[test]
fn count_only_receipts_are_not_repeated_when_credit_details_arrive() {
    let now = Utc::now();
    let mut seen = Observation::default();
    assert_eq!(seen.update(&codex(2, Value::Null, now), now).count, 0);
    assert_eq!(seen.update(&codex(3, Value::Null, now), now).count, 1);
    assert_eq!(seen.update(&codex(3, Value::Null, now), now).count, 0);
    let details = json!([credit("a", now), credit("b", now), credit("c", now)]);
    assert_eq!(seen.update(&codex(3, details.clone(), now), now).count, 0);
    let mut next = details.as_array().unwrap().clone();
    next.push(credit("d", now));
    assert_eq!(seen.update(&codex(4, json!(next), now), now).count, 1);
    assert_eq!(seen.update(&codex(4, Value::Null, now), now).count, 0);
    assert_eq!(seen.update(&codex(4, json!(next), now), now).count, 0);
}

#[test]
fn capped_details_do_not_announce_old_credits_but_grant_times_identify_new_ones() {
    let now = Utc::now();
    let later = now + chrono::Duration::minutes(30);
    let mut seen = Observation::default();
    assert_eq!(seen.update(&codex(3, json!([credit("a", now)]), now), now).count, 0);
    assert_eq!(seen.update(&codex(3, json!([credit("b", now)]), now), now).count, 0);
    let mut new_credit = credit("new", now);
    new_credit["granted_at"] = json!(now + chrono::Duration::minutes(1));
    assert_eq!(seen.update(&codex(3, json!([new_credit]), later), later).count, 1);
    assert_eq!(seen.update(&codex(3, json!([new_credit]), later), later).count, 0);
    assert_eq!(seen.update(&codex(4, Value::Null, later), later).count, 1);
    let mut delayed = credit("delayed", now);
    delayed["granted_at"] = json!(now + chrono::Duration::minutes(2));
    assert_eq!(seen.update(&codex(4, json!([delayed]), later), later).count, 0);
}

struct Destination {
    url: String,
    messages: mpsc::UnboundedReceiver<Value>,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl Destination {
    async fn new() -> Self {
        async fn hook(State(tx): State<mpsc::UnboundedSender<Value>>, Json(body): Json<Value>) {
            tx.send(body).unwrap();
        }
        let (tx, messages) = mpsc::unbounded_channel();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let server = tokio::spawn(
            axum::serve(listener, Router::new().route("/hook", axum::routing::post(hook)).with_state(tx)).into_future(),
        );
        Self { url, messages, server }
    }
    fn enable(&self, app: &App) {
        let mut cfg = (*app.cfg()).clone();
        cfg.notifications.webhooks = vec![self.url.clone()];
        app.set_config(cfg);
    }
    async fn receive(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(3), self.messages.recv()).await.unwrap().unwrap()
    }
    async fn quiet(&mut self) {
        assert!(tokio::time::timeout(Duration::from_millis(100), self.messages.recv()).await.is_err());
    }
}
impl Drop for Destination {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn both_providers_notify_from_real_refreshes_once_across_duplicates_and_restart() {
    async fn backend(State(body): State<Arc<Mutex<Value>>>, request: Request<Body>) -> axum::response::Response {
        assert_eq!(request.method(), axum::http::Method::GET, "observing receipts must never spend a reset");
        let body = body.lock().clone();
        if body.is_null() {
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Json(match request.uri().path() {
            "/api/oauth/profile" => json!({"account":{"uuid":"account-1"},
                "organization":{"uuid":"11111111-1111-4111-8111-111111111111"}}),
            "/api/oauth/usage" | "/backend-api/wham/rate-limit-reset-credits" => body,
            "/backend-api/wham/usage" => json!({}),
            path => panic!("unexpected provider path: {path}"),
        })
        .into_response()
    }
    for provider in [Provider::Claude, Provider::Codex] {
        let (temp, app, acct) = fixture(provider, 2);
        let mut destination = Destination::new().await;
        destination.enable(&app);
        let now = Utc::now();
        let initial = if provider == Provider::Claude {
            claude_usage()
        } else {
            json!({"available_count":1,"credits":[credit("old", now)]})
        };
        let body = Arc::new(Mutex::new(initial));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        *app.reset_test_origin.lock() = Some(origin.clone());
        let server = tokio::spawn(
            axum::serve(listener, Router::new().fallback(any(backend)).with_state(body.clone())).into_future(),
        );
        assert!(refresh(&app, &acct).await.unwrap().error.is_none());
        destination.quiet().await;
        let next = if provider == Provider::Claude {
            let mut next = claude_usage();
            let mut grant = next["cedar_ember"]["grants"][0].clone();
            grant["id"] = json!("new-grant");
            grant["resets_total"] = json!(1);
            grant["resets_left"] = json!(1);
            grant["usable_now"] = json!(false);
            next["cedar_ember"]["grants"][0]["resets_left"] = json!(1);
            next["cedar_ember"]["grants"].as_array_mut().unwrap().push(grant);
            next
        } else {
            json!({"available_count":1,"credits":[credit("new", now)]})
        };
        // Errors preserve receipt history instead of establishing another baseline.
        *body.lock() = Value::Null;
        assert!(refresh(&app, &acct).await.unwrap().error.is_some());
        *body.lock() = next;
        let duplicate = app.pool.all().into_iter().find(|a| a.id != acct.id).unwrap();
        let (one, two) = tokio::join!(refresh(&app, &acct), refresh(&app, &duplicate));
        assert!(one.unwrap().error.is_none() || two.unwrap().error.is_none());
        let message = destination.receive().await;
        assert_eq!(message["title"], format!("New {} banked reset", provider_name(provider)));
        assert_eq!(message["event"], "banked_reset");
        assert_eq!(message["level"], "ok");
        assert_eq!(message["provider"], provider.as_str());
        let text = message["body"].as_str().unwrap();
        let notifying_account = app.pool.get(message["account"].as_str().unwrap()).unwrap();
        assert!(text.contains(&notifying_account.label) && text.contains("1 new banked reset"));
        assert!(text.contains(if provider == Provider::Claude { "2 banked in total" } else { "1 banked in total" }));
        assert!(text.contains("UTC"));
        assert!(refresh(&app, &duplicate).await.unwrap().error.is_none());
        let restarted = App::new((*app.cfg()).clone(), temp.0.join("config.yaml"));
        *restarted.reset_test_origin.lock() = Some(origin);
        let restored_acct = restarted.pool.all()[0].clone();
        assert!(refresh(&restarted, &restored_acct).await.unwrap().error.is_none());
        destination.quiet().await;
        server.abort();
    }
}

#[tokio::test]
async fn disabled_notifications_and_absent_destinations_still_update_the_baseline() {
    let (temp, app, acct) = fixture(Provider::Codex, 1);
    let mut destination = Destination::new().await;
    let now = Utc::now();
    let mut ledger = Ledger::open(&temp.0, "codex:account-1").unwrap();
    store(&app, &acct, "account-1", &mut ledger, &reset_view(codex(0, json!([]), now)));
    let first = codex(1, json!([credit("first", now)]), now);
    store(&app, &acct, "account-1", &mut ledger, &reset_view(first.clone()));
    destination.enable(&app);
    store(&app, &acct, "account-1", &mut ledger, &reset_view(first));
    let mut cfg = (*app.cfg()).clone();
    cfg.notifications.banked_resets = false;
    app.set_config(cfg.clone());
    let second = codex(1, json!([credit("second", now)]), now);
    store(&app, &acct, "account-1", &mut ledger, &reset_view(second.clone()));
    cfg.notifications.banked_resets = true;
    app.set_config(cfg);
    store(&app, &acct, "account-1", &mut ledger, &reset_view(second));
    destination.quiet().await;
    let third = codex(3, json!([credit("third", now), credit("fourth", now), credit("fifth", now)]), now);
    store(&app, &acct, "account-1", &mut ledger, &reset_view(third));
    let message = destination.receive().await;
    assert_eq!(message["title"], "3 new Codex banked resets");
    assert!(message["body"].as_str().unwrap().contains("3 banked in total"));
    destination.quiet().await;
}

#[tokio::test]
async fn receipt_save_failure_preserves_the_previous_observation_and_sends_nothing() {
    let (temp, app, acct) = fixture(Provider::Codex, 1);
    let mut destination = Destination::new().await;
    destination.enable(&app);
    let now = Utc::now();
    let identity = "codex:account-1";
    let mut ledger = Ledger::open(&temp.0, identity).unwrap();
    store(&app, &acct, "account-1", &mut ledger, &reset_view(codex(0, json!([]), now)));
    let saved = serde_json::to_value(&ledger.journal).unwrap();
    let path = temp.0.join(".banked-resets").join(format!("{}.json", hex::encode(Sha256::digest(identity))));
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let next = reset_view(codex(1, json!([credit("new", now)]), now));
    store(&app, &acct, "account-1", &mut ledger, &next);
    assert_eq!(serde_json::to_value(&ledger.journal).unwrap(), saved);
    destination.quiet().await;
    std::fs::remove_dir(&path).unwrap();
    store(&app, &acct, "account-1", &mut ledger, &next);
    assert_eq!(destination.receive().await["title"], "New Codex banked reset");
    destination.quiet().await;
}

#[test]
fn old_journals_and_distinct_subscription_identities_get_independent_quiet_baselines() {
    let (temp, app, acct) = fixture(Provider::Claude, 1);
    let identity = "claude:organization";
    let mut ledger = Ledger::open(&temp.0, identity).unwrap();
    ledger.journal = serde_json::from_value(json!({"operations":{}})).unwrap();
    let inventory = provider::claude(&claude_usage(), Utc::now()).unwrap();
    store(&app, &acct, "account-1", &mut ledger, &reset_view(inventory.clone()));
    if let Credential::OAuth(oauth) = &mut *acct.cred.write() {
        oauth.account_id = Some("account-2".into());
    }
    store(&app, &acct, "account-2", &mut ledger, &reset_view(inventory));
    assert_eq!(ledger.journal.observations.len(), 2);
    assert!(ledger.journal.operations.is_empty());
    drop(ledger);
    let restored = Ledger::open(&temp.0, identity).unwrap();
    assert_eq!(restored.journal.observations.len(), 2);
}
