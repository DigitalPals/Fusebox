//! Quota attribution end to end: real router, a mocked ChatGPT backend whose
//! quota headers rise with each response, two named clients.
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{Json, Router, extract::State, response::IntoResponse, routing::post};
use serde_json::{Value, json};

use crate::config::{Config, NamedClient};
use crate::state::App;

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(router: Router) -> Server {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(l, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
    });
    Server { url, task }
}

struct Directory(std::path::PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Each response reports 8 more points of the 5-hour window and 1 more of the week.
async fn backend(State(calls): State<Arc<AtomicUsize>>, Json(_): Json<Value>) -> axum::response::Response {
    let n = calls.fetch_add(1, Ordering::SeqCst) as u64 + 1;
    let now = chrono::Utc::now().timestamp();
    let completed = json!({"type":"response.completed","response":{"id":format!("resp_{n}"),"object":"response",
        "status":"completed","model":"gpt-6.1-sol","output":[{"type":"message","role":"assistant",
        "content":[{"type":"output_text","text":"ok"}]}],"usage":{"input_tokens":1000,"input_tokens_details":{"cached_tokens":200},"output_tokens":100}}});
    (
        [
            ("content-type", "text/event-stream".to_string()),
            ("x-codex-primary-used-percent", (8 * n).to_string()),
            ("x-codex-primary-window-minutes", "300".into()),
            // Two hours into the 5-hour window, so a fast pace shows.
            ("x-codex-primary-reset-at", (now + 3 * 3600).to_string()),
            ("x-codex-secondary-used-percent", n.to_string()),
            ("x-codex-secondary-window-minutes", "10080".into()),
            ("x-codex-secondary-reset-at", (now + 5 * 86_400).to_string()),
        ],
        format!("data: {completed}\n\n"),
    )
        .into_response()
}

fn config(dir: &Directory, upstream: &str) -> Config {
    let client = |id: &str, key: &str| NamedClient {
        id: id.into(),
        label: format!("The {id}"),
        key: key.into(),
        ..Default::default()
    };
    let mut cfg = Config {
        auth_dir: dir.0.join("auth").to_string_lossy().into(),
        management_key: "test-management".into(),
        named_clients: vec![client("laptop", "laptop-key"), client("agent", "agent-key")],
        codex_websockets: false,
        ..Default::default()
    };
    std::fs::create_dir_all(cfg.auth_dir()).unwrap();
    cfg.usage.database = Some(dir.0.join("usage.sqlite3").to_string_lossy().into());
    std::fs::write(
        cfg.auth_dir().join("codex-test.json"),
        json!({"type":"codex","access_token":"test-upstream","email":"test@example.invalid","base_url":format!("{upstream}/v1")})
            .to_string(),
    )
    .unwrap();
    cfg
}

async fn ask(client: &reqwest::Client, url: &str, key: &str) {
    let status = client
        .post(format!("{url}/v1/responses"))
        .bearer_auth(key)
        .json(&json!({"model":"gpt-6.1-sol","input":"hello","stream":false}))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 200);
}

#[tokio::test]
async fn subscription_windows_say_which_client_used_them() {
    let dir = Directory(std::env::temp_dir().join(format!("fusebox-attribution-{}", uuid::Uuid::new_v4())));
    let calls = Arc::new(AtomicUsize::new(0));
    let upstream = serve(Router::new().route("/v1/responses", post(backend)).with_state(calls.clone())).await;
    let app = App::new(config(&dir, &upstream.url), dir.0.join("config.yaml"));
    let proxy = serve(crate::server::router(app.clone())).await;
    let client = reqwest::Client::new();
    // The laptop sends three requests for every one the agent sends.
    for key in
        ["laptop-key", "laptop-key", "agent-key", "laptop-key", "laptop-key", "laptop-key", "agent-key", "laptop-key"]
    {
        ask(&client, &proxy.url, key).await;
    }
    let accounts: Value = client
        .get(format!("{}/api/accounts", proxy.url))
        .bearer_auth("test-management")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let attribution = &accounts[0]["quota_attribution"];
    assert_eq!(attribution["basis"], "estimate");
    let week = attribution["current"].as_array().unwrap().iter().find(|w| w["name"] == "week").unwrap();
    // The first reading is the starting point; each later one explains the attempt before it.
    assert_eq!(week["before"], 1.0);
    assert_eq!(week["used"], 8.0);
    assert!((week["attributed"].as_f64().unwrap() - 7.0).abs() < 1e-6, "{week}");
    let clients = week["clients"].as_array().unwrap();
    assert_eq!(clients[0]["id"], "laptop");
    assert_eq!(clients[0]["label"], "The laptop");
    assert!((clients[0]["points"].as_f64().unwrap() - 5.0).abs() < 1e-6, "{clients:?}");
    assert!((clients[1]["points"].as_f64().unwrap() - 2.0).abs() < 1e-6);
    assert_eq!(week["models"][0]["id"], "gpt-6.1-sol");

    // The 5-hour window is on course to run out early, and the fault names who.
    let faults: Value = client
        .get(format!("{}/api/faults", proxy.url))
        .bearer_auth("test-management")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pace = faults.as_array().unwrap().iter().find(|f| f["kind"] == "pace").expect("a pace warning");
    assert_eq!(pace["badge"], false);
    assert!(pace["detail"].as_str().unwrap().contains("The laptop used"), "{pace}");

    // The Usage page's top clients: each by name, with what it sent and its models.
    let store = app.usage.clone().unwrap();
    store.flush().await.unwrap();
    let usage: Value = client
        .get(format!("{}/api/usage/dashboard", proxy.url))
        .bearer_auth("test-management")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let clients = usage["combined"]["breakdowns"]["client"].as_array().unwrap();
    let laptop = &clients[0];
    let tokens = ["input", "cache_read", "output"].map(|k| laptop["tokens"][k].as_i64().unwrap_or(-1));
    assert_eq!(
        (laptop["id"].as_str(), laptop["observations"].as_i64(), tokens),
        (Some("laptop"), Some(6), [4800, 1200, 600])
    );
    assert!(laptop["estimated_cost_nanos"].as_i64().unwrap() > clients[1]["estimated_cost_nanos"].as_i64().unwrap());
    assert_eq!(laptop["models"][0]["id"], "gpt-6.1-sol");
    assert_eq!(laptop["models"][0]["observations"], 6);
    let label = |id: &str| {
        usage["facets"]["clients"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap()["label"].clone()
    };
    assert_eq!((label("laptop"), label("agent")), (json!("The laptop"), json!("The agent")));

    // A restart keeps what each window was used by.
    let (saved, changed) = app.pool.attributions();
    assert!(changed);
    let path = app.cfg().auth_dir().join(crate::attribution::FILE);
    saved.save(&path).unwrap();
    drop(proxy);
    store.shutdown().await.unwrap();
    let again = App::new(config(&dir, &upstream.url), dir.0.join("config.yaml"));
    let restored = again.pool.all()[0].state.lock().attribution.current.clone();
    let week = restored.iter().find(|w| w.name == "week").unwrap();
    assert!((week.clients["laptop"] - 5.0).abs() < 1e-6);
    if let Some(store) = &again.usage {
        store.shutdown().await.unwrap();
    }
}

fn identity_fixture() -> (Directory, Config, std::path::PathBuf) {
    let dir = Directory(std::env::temp_dir().join(format!("fusebox-attribution-identity-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let mut cfg = Config { auth_dir: dir.0.to_string_lossy().into(), ..Default::default() };
    cfg.usage.enabled = false;
    let file = dir.0.join("codex.json");
    write_identity(&file, "first-account", "first-token", false);
    (dir, cfg, file)
}

fn write_identity(file: &std::path::Path, id: &str, token: &str, metadata_change: bool) {
    std::fs::write(
        file,
        json!({"type":"codex","access_token":token,"account_id":id,"email":"test@example.invalid",
            "priority":if metadata_change { 10 } else { 0 }})
        .to_string(),
    )
    .unwrap();
}

fn seed_attribution(pool: &crate::accounts::Pool) {
    use chrono::{Duration, Utc};
    let account = pool.all().pop().unwrap();
    let mut state = account.state.lock();
    let window = |used, days| crate::quota::Window {
        name: "week".into(),
        model: None,
        used,
        resets_at: Some(Utc::now() + Duration::days(days)),
    };
    state.set_quota(vec![window(70.0, 3)], None);
    state.set_quota(vec![window(0.0, 10)], None);
    state.attribution.record(crate::attribution::Use {
        at: Utc::now() + Duration::seconds(1),
        cost: Some(1.0),
        tokens: 10.0,
        client: "old-client".into(),
        session: Some("old-session".into()),
        model: Some("gpt-6.1-sol".into()),
    });
    assert_eq!(state.attribution.current.len(), 1);
    assert_eq!(state.attribution.previous.len(), 1);
}

#[test]
fn replacing_an_oauth_identity_clears_attribution_on_reload_and_restart() {
    for restart in [false, true] {
        for metadata_change in [false, true] {
            let (_dir, cfg, file) = identity_fixture();
            let mut pool = crate::accounts::Pool::default();
            pool.reload(&cfg);
            seed_attribution(&pool);
            let path = cfg.auth_dir().join(crate::attribution::FILE);
            pool.attributions().0.save(&path).unwrap();
            let mut inflight = crate::usage::capture::RequestUsage::new(None, "gpt-6.1-sol");
            inflight.attempt(&pool.all()[0]);
            write_identity(&file, "different-account", "different-token", metadata_change);
            if restart {
                pool = crate::accounts::Pool::default();
            }
            pool.reload(&cfg);
            if restart {
                pool.restore_attributions(crate::attribution::Saved::load(&path));
            }
            // A late completion from the old subscription must not refill the new state.
            inflight.finish(Some(200), Some(&crate::ir::Usage { input: 100, ..Default::default() }), true);
            let account = pool.all().pop().unwrap();
            let state = account.state.lock();
            assert!(state.quota.windows.is_empty());
            assert!(state.attribution.current.is_empty(), "restart: {restart}, metadata: {metadata_change}");
            assert!(state.attribution.previous.is_empty());
            assert_eq!(serde_json::to_value(&state.attribution).unwrap()["uses"], json!([]));
            drop(state);
            // Clearing must be persisted even if the new account has no readings yet.
            let (saved, changed) = pool.attributions();
            assert!(changed);
            assert!(saved.0.is_empty());
            saved.save(&path).unwrap();
            assert!(crate::attribution::Saved::load(&path).0.is_empty());
        }
    }
}

#[test]
fn refreshing_tokens_preserves_attribution_and_its_saved_identity() {
    for metadata_change in [false, true] {
        let (_dir, cfg, file) = identity_fixture();
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        seed_attribution(&pool);
        let (before, _) = pool.attributions();
        write_identity(&file, "first-account", "refreshed-token", metadata_change);
        pool.reload(&cfg);
        let (after, changed) = pool.attributions();
        assert!(!changed);
        assert_eq!(after, before);
        let text = serde_json::to_string(&after).unwrap();
        assert!(!text.contains("first-token") && !text.contains("refreshed-token") && !text.contains("first-account"));
        let restored = crate::accounts::Pool::default();
        restored.reload(&cfg);
        restored.restore_attributions(serde_json::from_str(&text).unwrap());
        assert_eq!(restored.attributions().0, after);
    }
}

#[test]
fn legacy_attribution_is_preserved_and_bound_to_an_identity_on_save() {
    let (_dir, cfg, _file) = identity_fixture();
    let pool = crate::accounts::Pool::default();
    pool.reload(&cfg);
    seed_attribution(&pool);
    let (saved, _) = pool.attributions();
    let mut legacy = serde_json::to_value(&saved).unwrap();
    legacy["file:codex.json"].as_object_mut().unwrap().remove("identity");
    let restored = crate::accounts::Pool::default();
    restored.reload(&cfg);
    restored.restore_attributions(serde_json::from_value(legacy).unwrap());
    let (migrated, changed) = restored.attributions();
    assert!(changed, "legacy state must be rewritten with its identity");
    assert_eq!(migrated, saved);
}
