use std::net::SocketAddr;

use axum::body::Bytes;
use axum::http::HeaderMap;
use chrono::Duration as Span;
use ring::agreement;
use tokio::sync::mpsc;

use super::*;
use crate::config::Config;

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("push-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn account(id: &str) -> AccountView {
    AccountView {
        id: id.into(),
        provider: "claude".into(),
        provider_name: "Claude".into(),
        group_name: "Claude".into(),
        label: format!("{id}@example.com"),
        ..Default::default()
    }
}

fn signed_out(id: &str) -> AccountView {
    AccountView { last_error: Some("token refresh failed: invalid_grant".into()), ..account(id) }
}

fn used_up(id: &str) -> AccountView {
    let mut a = account(id);
    a.pauses.insert("*".into(), (Utc::now() + Span::hours(2), "quota"));
    a
}

fn run(push: &Push, accounts: &[AccountView], settings: &Notifications) -> Vec<String> {
    let current = faults::faults(accounts, Utc::now());
    push.track(&current, accounts, settings).into_iter().map(|f| f.title).collect()
}

#[test]
fn a_fault_is_sent_once_after_two_checks_and_survives_a_restart() {
    let temp = Temp::new();
    let push = Push::load(&temp.0);
    let on = Notifications::default();
    let broken = [signed_out("a"), account("b")];
    assert!(run(&push, &broken, &on).is_empty(), "one sighting is not enough");
    assert_eq!(run(&push, &broken, &on), ["Claude sign-in expired"]);
    assert!(run(&push, &broken, &on).is_empty(), "already sent");

    let push = Push::load(&temp.0);
    assert!(run(&push, &broken, &on).is_empty());
    assert!(run(&push, &broken, &on).is_empty(), "a restart doesn't send it again");

    let fixed = [account("a"), account("b")];
    assert!(run(&push, &fixed, &on).is_empty());
    assert!(run(&push, &fixed, &on).is_empty());
    assert!(push.stored.lock().notified.is_empty(), "cleared after two checks without it");
    assert!(run(&push, &broken, &on).is_empty());
    assert_eq!(run(&push, &broken, &on), ["Claude sign-in expired"], "it can trip again");
}

#[test]
fn settings_choose_what_is_sent() {
    let temp = Temp::new();
    let push = Push::load(&temp.0);
    let off = Notifications { sign_in_expired: false, ..Default::default() };
    let broken = [signed_out("a"), account("b")];
    run(&push, &broken, &off);
    assert!(run(&push, &broken, &off).is_empty());
    // Turning it on later doesn't send a fault from before.
    assert!(run(&push, &broken, &Notifications::default()).is_empty());
}

#[test]
fn a_provider_running_out_stands_in_for_its_accounts_and_says_when_it_is_back() {
    let temp = Temp::new();
    let push = Push::load(&temp.0);
    let all = Notifications { account_used_up: true, ..Default::default() };
    let out = [used_up("a"), used_up("b")];
    run(&push, &out, &all);
    assert_eq!(run(&push, &out, &all), ["Every Claude account is used up"]);

    let one_back = [used_up("a"), account("b")];
    assert!(run(&push, &one_back, &all).is_empty());
    assert_eq!(run(&push, &one_back, &all), ["Claude is back"]);

    // One account running out is its own notification when others still work.
    let temp = Temp::new();
    let push = Push::load(&temp.0);
    run(&push, &one_back, &all);
    assert_eq!(run(&push, &one_back, &all), ["a@example.com: usage limit used up"]);
}

#[test]
fn faults_present_when_notifications_are_turned_on_are_not_sent() {
    let temp = Temp::new();
    let push = Push::load(&temp.0);
    let broken = [signed_out("a"), account("b")];
    push.seed(&faults::faults(&broken, Utc::now()));
    run(&push, &broken, &Notifications::default());
    assert!(run(&push, &broken, &Notifications::default()).is_empty());
}

struct Received {
    headers: HeaderMap,
    body: Bytes,
}

#[tokio::test]
async fn devices_subscribe_and_receive_encrypted_notifications() {
    let temp = Temp::new();
    let cfg = Config { auth_dir: temp.0.to_string_lossy().into(), management_key: "k".into(), ..Default::default() };
    let app = App::new(cfg, temp.0.join("config.yaml"));

    // A push service that records what it gets: the first push is accepted, the next one
    // finds the subscription gone.
    let (tx, mut rx) = mpsc::unbounded_channel::<Received>();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let service = Router::new().route(
        "/push/{token}",
        post(move |headers: HeaderMap, body: Bytes| {
            let tx = tx.clone();
            let calls = calls.clone();
            async move {
                let _ = tx.send(Received { headers, body });
                if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    StatusCode::CREATED
                } else {
                    StatusCode::GONE
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let service_url = format!("http://{}", listener.local_addr().unwrap());
    let service = tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::mgmt::router(app.clone()).with_state(app.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap()
    });
    let client = reqwest::Client::new();

    let status: Value =
        client.get(format!("{origin}/push")).bearer_auth("k").send().await.unwrap().json().await.unwrap();
    let public_key = status["public_key"].as_str().unwrap().to_string();
    assert_eq!(crypto::decode(&public_key).unwrap().len(), 65);
    assert_eq!(status["subscriptions"], json!([]));
    assert!(client.get(format!("{origin}/push")).send().await.unwrap().status().is_client_error(), "needs the key");

    // The browser's side of the subscription.
    let rng = ring::rand::SystemRandom::new();
    let ua = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
    let p256dh = URL_SAFE_NO_PAD.encode(ua.compute_public_key().unwrap().as_ref());
    let auth = [9u8; 16];
    let subscription = |endpoint: &str, p256dh: &str| {
        json!({
            "endpoint": endpoint,
            "keys": { "p256dh": p256dh, "auth": URL_SAFE_NO_PAD.encode(auth) },
            "origin": "https://box.example.ts.net/#/config",
            "label": "Test phone",
        })
    };
    for bad in [subscription("ftp://push.example/x", &p256dh), subscription(&format!("{service_url}/push/a"), "AAAA")] {
        let resp =
            client.post(format!("{origin}/push/subscriptions")).bearer_auth("k").json(&bad).send().await.unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    }
    let endpoint = format!("{service_url}/push/device-token");
    let created: Value = client
        .post(format!("{origin}/push/subscriptions"))
        .bearer_auth("k")
        .json(&subscription(&endpoint, &p256dh))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    let again: Value = client
        .post(format!("{origin}/push/subscriptions"))
        .bearer_auth("k")
        .json(&subscription(&endpoint, &p256dh))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(again["id"], id.as_str(), "the same endpoint is the same device");
    let status: Value =
        client.get(format!("{origin}/push")).bearer_auth("k").send().await.unwrap().json().await.unwrap();
    assert_eq!(status["subscriptions"][0]["label"], "Test phone");
    assert_eq!(status["subscriptions"][0]["origin"], "https://box.example.ts.net");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(temp.0.join(STATE_FILE)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let sent =
        client.post(format!("{origin}/push/test")).bearer_auth("k").json(&json!({ "id": id })).send().await.unwrap();
    assert_eq!(sent.status(), reqwest::StatusCode::OK);
    let got = rx.recv().await.unwrap();
    assert_eq!(got.headers["ttl"], "43200");
    assert_eq!(got.headers["content-encoding"], "aes128gcm");
    let authorization = got.headers["authorization"].to_str().unwrap();
    assert!(authorization.starts_with("vapid t="));
    assert!(authorization.ends_with(&format!(", k={public_key}")));
    let message: Value = serde_json::from_slice(&crypto::decrypt(ua, &auth, &got.body).unwrap()).unwrap();
    assert_eq!(message["web_push"], 8030);
    assert_eq!(message["notification"]["title"], "Notifications are on");
    assert_eq!(message["notification"]["navigate"], "https://box.example.ts.net/#/config/notifications");
    assert_eq!(message["notification"]["app_badge"], "0");

    // The push service forgot the device: it is removed.
    let gone =
        client.post(format!("{origin}/push/test")).bearer_auth("k").json(&json!({ "id": id })).send().await.unwrap();
    assert_eq!(gone.status(), reqwest::StatusCode::GONE);
    assert!(app.push.subscriptions().is_empty());
    let missing = client.delete(format!("{origin}/push/subscriptions/{id}")).bearer_auth("k").send().await.unwrap();
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);

    server.abort();
    service.abort();
}

#[test]
fn a_large_usage_database_or_a_full_disk_is_a_fault() {
    use crate::usage::store::DatabaseStats;
    let stats = |bytes: u64, free: u64| DatabaseStats {
        bytes,
        growth_bytes_per_day: 14_000_000,
        disk_free_bytes: Some(free),
        ..Default::default()
    };
    assert!(faults::storage(None, 2048).is_none());
    assert!(faults::storage(Some(&stats(1 << 30, 100 << 30)), 2048).is_none());
    let large = faults::storage(Some(&stats(3 << 30, 100 << 30)), 2048).unwrap();
    assert_eq!((large.kind, large.level, large.path.as_str()), ("storage", "warn", "#/usage"));
    assert!(large.detail.unwrap().contains("14 MB a day"));
    assert!(faults::storage(Some(&stats(3 << 30, 100 << 30)), 0).is_none(), "0 turns the size warning off");
    let full = faults::storage(Some(&stats(1 << 20, 500 << 20)), 0).unwrap();
    assert_eq!(full.level, "err");
    assert_eq!(faults::badge(std::slice::from_ref(&full)), 1);
    assert_eq!(full.json()["badge"], true);
}

#[test]
fn a_used_up_account_is_not_an_error_once_its_limit_renews() {
    let cfg = Config {
        auth_dir: "/nonexistent".into(),
        claude_api_key: vec![crate::config::KeyEntry { api_key: "mock-only".into(), ..Default::default() }],
        ..Default::default()
    };
    let app = App::new(cfg, "/nonexistent/config.yaml".into());
    let account = app.pool.all()[0].clone();
    let kinds = || -> Vec<String> {
        let faults = current_faults(&app);
        faults.as_array().unwrap().iter().map(|f| f["kind"].as_str().unwrap().to_string()).collect()
    };
    let spent = "subscription quota exhausted: This request would exceed your account's rate limit.";
    account.exhaust("claude-sonnet-5-5", Utc::now() + Span::hours(1), spent);
    assert_eq!(kinds(), ["quota"]);
    // The limit renewed and no request has reached the account since.
    account.exhaust("claude-sonnet-5-5", Utc::now() - Span::seconds(1), spent);
    assert!(kinds().is_empty(), "a renewed limit is not an account error");
    assert_eq!(account.snapshot()["last_error"], Value::Null);
    account.rate_limit("claude-sonnet-5-5", Utc::now() - Span::seconds(1), "429: slow down");
    assert!(kinds().is_empty(), "nor is an ended rate limit");
    account.cool(None, Utc::now() - Span::seconds(1), "403: forbidden");
    assert_eq!(kinds(), ["error"], "other errors stay after their cooldown");
}

#[tokio::test]
async fn the_iphone_app_registers_and_apple_answers_are_handled() {
    let temp = Temp::new();
    let cfg = Config { auth_dir: temp.0.to_string_lossy().into(), management_key: "k".into(), ..Default::default() };
    let app = App::new(cfg.clone(), temp.0.join("config.yaml"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::mgmt::router(app.clone()).with_state(app.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap()
    });
    let client = reqwest::Client::new();
    let token = "a1".repeat(32);
    let register = |token: &str, environment: &str| {
        client
            .post(format!("{origin}/push/apns"))
            .bearer_auth("k")
            .json(&json!({ "token": token, "environment": environment, "label": "John's iPhone" }))
            .send()
    };

    // Without a key the app hears why, and nothing is saved.
    let resp = register(&token, "production").await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert!(resp.json::<Value>().await.unwrap()["error"].as_str().unwrap().contains("apns"));
    assert!(app.push.subscriptions().is_empty());

    let key = temp.0.join("AuthKey_KEY1234567.p8");
    let der = crypto::Vapid::generate().unwrap();
    std::fs::write(
        &key,
        format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            base64::engine::general_purpose::STANDARD.encode(der)
        ),
    )
    .unwrap();
    let apns = crate::config::Apns {
        team_id: "ABCDE12345".into(),
        key_id: "KEY1234567".into(),
        key_file: key.to_string_lossy().into(),
        topic: "com.digitalpals.fusebox".into(),
    };
    app.set_config(Config { apns: Some(apns.clone()), ..cfg });

    for (bad, environment) in [("zz".repeat(32), "production"), (token.clone(), "staging")] {
        assert!(register(&bad, environment).await.unwrap().status().is_client_error());
    }
    let created: Value = register(&token.to_uppercase(), "development").await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    let again: Value = register(&token, "production").await.unwrap().json().await.unwrap();
    assert_eq!(again["id"], id.as_str(), "the same token is the same device");
    let status: Value =
        client.get(format!("{origin}/push")).bearer_auth("k").send().await.unwrap().json().await.unwrap();
    assert_eq!(status["apns"], true);
    assert_eq!(status["subscriptions"][0]["kind"], "apns");
    assert_eq!(status["subscriptions"][0]["label"], "John's iPhone");
    let sub = app.push.subscriptions().remove(0);
    let device = sub.apns.clone().unwrap();
    assert_eq!(device.environment, apns::Environment::Production);
    assert_eq!(device.token, token);

    // Apple's side: the first push is accepted, then the token is refused as unknown,
    // then the provider token is refused.
    let (tx, mut rx) = mpsc::unbounded_channel::<Received>();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let service = Router::new().route(
        "/3/device/{token}",
        post(move |headers: HeaderMap, body: Bytes| {
            let tx = tx.clone();
            let calls = calls.clone();
            async move {
                let _ = tx.send(Received { headers, body });
                match calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                    0 => (StatusCode::OK, String::new()),
                    1 => (StatusCode::BAD_REQUEST, r#"{"reason":"BadDeviceToken"}"#.into()),
                    _ => (StatusCode::FORBIDDEN, r#"{"reason":"InvalidProviderToken"}"#.into()),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let service = tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });
    let fault = Fault {
        key: "provider:claude".into(),
        provider: "claude".into(),
        title: "Every Claude account is used up".into(),
        body: "The first is back in 2h 10m.".into(),
        path: "#/accounts".into(),
        kind: "provider",
        level: "err",
        ..Default::default()
    };
    let send = || apns::send(&client, &base, &app.push.apns, Some(&apns), &device, &fault, 2, true, TTL_SECONDS);
    assert_eq!(send().await.unwrap(), StatusCode::OK);
    let got = rx.recv().await.unwrap();
    assert_eq!(got.headers["apns-topic"], "com.digitalpals.fusebox");
    assert_eq!(got.headers["apns-push-type"], "alert");
    assert_eq!(got.headers["apns-priority"], "10");
    assert_eq!(got.headers["apns-collapse-id"], "provider:claude");
    assert!(got.headers["authorization"].to_str().unwrap().starts_with("bearer ey"));
    let body: Value = serde_json::from_slice(&got.body).unwrap();
    assert_eq!(body["aps"]["alert"]["title"], "Every Claude account is used up");
    assert_eq!(body["aps"]["badge"], 2);
    assert_eq!(body["path"], "#/accounts");
    assert_eq!(send().await.unwrap(), StatusCode::GONE, "an unknown token means the device is gone");
    let refused = send().await.unwrap_err().to_string();
    assert!(refused.contains("InvalidProviderToken"), "{refused}");
    assert!(
        apns::send(&client, &base, &app.push.apns, None, &device, &fault, 0, false, TTL_SECONDS)
            .await
            .unwrap_err()
            .to_string()
            .contains("isn't set up")
    );

    let removed = client.delete(format!("{origin}/push/subscriptions/{id}")).bearer_auth("k").send().await.unwrap();
    assert_eq!(removed.status(), reqwest::StatusCode::OK);
    assert!(app.push.subscriptions().is_empty());
    server.abort();
    service.abort();
}

#[tokio::test]
async fn the_iphone_app_registers_through_the_relay_without_an_apns_key() {
    let temp = Temp::new();
    let cfg = Config { auth_dir: temp.0.to_string_lossy().into(), management_key: "k".into(), ..Default::default() };
    assert_eq!(cfg.push_relay, relay::DEFAULT_URL, "the relay is on by default");
    let app = App::new(cfg.clone(), temp.0.join("config.yaml"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::mgmt::router(app.clone()).with_state(app.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap()
    });
    let client = reqwest::Client::new();
    let handle = URL_SAFE_NO_PAD.encode([5u8; 32]);
    let key = base64::engine::general_purpose::STANDARD.encode([6u8; 32]);
    let register = |body: Value| client.post(format!("{origin}/push/apns")).bearer_auth("k").json(&body).send();

    let status: Value =
        client.get(format!("{origin}/push")).bearer_auth("k").send().await.unwrap().json().await.unwrap();
    assert_eq!((status["apns"].clone(), status["relay"].clone()), (json!(false), json!(relay::DEFAULT_URL)));
    // Without apns, a device token can't be used directly.
    let direct = register(json!({ "token": "ab".repeat(32), "environment": "production" })).await.unwrap();
    assert_eq!(direct.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert!(direct.json::<Value>().await.unwrap()["error"].as_str().unwrap().contains("relay"));
    for bad in [json!({ "label": "x" }), json!({ "relay": { "handle": "short", "key": key } })] {
        assert_eq!(register(bad).await.unwrap().status(), reqwest::StatusCode::BAD_REQUEST);
    }
    let body = json!({ "relay": { "handle": handle, "key": key }, "label": "John's iPhone" });
    let created: Value = register(body.clone()).await.unwrap().json().await.unwrap();
    let again: Value = register(body.clone()).await.unwrap().json().await.unwrap();
    assert_eq!(created["id"], again["id"], "the same handle is the same device");
    let status: Value =
        client.get(format!("{origin}/push")).bearer_auth("k").send().await.unwrap().json().await.unwrap();
    assert_eq!(status["subscriptions"][0]["kind"], "apns");
    assert_eq!(status["subscriptions"][0]["via"], "relay");
    assert_eq!(app.push.subscriptions()[0].relay.as_ref().unwrap().handle, handle);

    // Turned off, the relay takes no new devices.
    app.set_config(Config { push_relay: String::new(), ..cfg });
    let status: Value =
        client.get(format!("{origin}/push")).bearer_auth("k").send().await.unwrap().json().await.unwrap();
    assert_eq!(status["relay"], Value::Null);
    let off = register(json!({ "relay": { "handle": URL_SAFE_NO_PAD.encode([9u8; 32]), "key": key } })).await.unwrap();
    assert_eq!(off.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert!(Config::parse("push-relay: http://push.example.com\n").is_err(), "the relay takes https only");
    assert_eq!(Config::parse("push-relay: ''\n").unwrap().push_relay, "");
    server.abort();
}
