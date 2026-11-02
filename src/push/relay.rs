//! The push relay, so the iPhone app gets notifications from any Fusebox, not only one
//! holding the app's APNs key.
//!
//! The app registers its device token with the relay and gets back a random handle.
//! It gives its own Fusebox that handle and a 32-byte key it made. Fusebox encrypts
//! each notification with that key and gives the relay only the handle and the
//! ciphertext. The relay sends them on through Apple's push service, and the app's
//! notification service extension decrypts them on the phone. So neither the relay
//! nor Apple reads what a Fusebox sends, and a Fusebox never learns the device token.
//!
//! The relay keeps device tokens and a hash of each handle in SQLite; a handle unused
//! for 180 days is forgotten. `fusebox push-relay` runs it.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, ensure};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::Utc;
use parking_lot::Mutex;
use ring::rand::{SecureRandom, SystemRandom};
use ring::{aead, digest, hmac};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::apns;
use super::faults::Fault;
use crate::config::Apns;

/// The relay the Fusebox iPhone app uses, unless `push-relay` names another.
pub const DEFAULT_URL: &str = "https://push.whombat.com";
/// Bound into every sealed notification, so a ciphertext can't be replayed elsewhere.
const AAD: &[u8] = b"fusebox-push-v1";
/// APNs takes 4 KB of payload; this leaves room for the rest of it.
const SEALED_MAX: usize = 3000;
const HANDLE_BYTES: usize = 32;
const KEY_BYTES: usize = 32;
/// Handles per device token: one for each Fusebox the phone gets notifications from.
const HANDLES_PER_TOKEN: usize = 20;
const UNUSED_DAYS: i64 = 180;
const TTL_MAX: u32 = 7 * 24 * 3600;
const HOUR: Duration = Duration::from_secs(3600);
const REGISTRATIONS_PER_HOUR: u32 = 30;
const SENDS_PER_HANDLE_HOUR: u32 = 120;
const SENDS_PER_ADDRESS_HOUR: u32 = 1200;

// ------------------------------------------------------------- Fusebox's side

/// A device reached through the relay: its handle there, and the key only the device
/// and this Fusebox hold.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relayed {
    pub handle: String,
    /// AES-256-GCM key, base64.
    pub key: String,
}

impl Relayed {
    pub fn new(handle: &str, key: &str) -> Result<Self> {
        ensure!(
            URL_SAFE_NO_PAD.decode(handle).is_ok_and(|h| h.len() == HANDLE_BYTES),
            "The relay handle isn't one the relay gives out"
        );
        ensure!(STANDARD.decode(key).is_ok_and(|k| k.len() == KEY_BYTES), "The device key must be 32 bytes, base64");
        Ok(Self { handle: handle.into(), key: key.into() })
    }

    /// The subscription's endpoint, which tells devices apart.
    pub fn endpoint(&self) -> String {
        format!("relay:{}", self.handle)
    }
}

/// `nonce ‖ ciphertext ‖ tag`, base64: AES-256-GCM with a random nonce.
pub fn seal(key: &[u8], plaintext: &[u8]) -> Result<String> {
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, key).map_err(|_| anyhow!("bad key"))?);
    let mut nonce = [0u8; aead::NONCE_LEN];
    SystemRandom::new().fill(&mut nonce).map_err(|_| anyhow!("no randomness for the nonce"))?;
    let mut sealed = plaintext.to_vec();
    key.seal_in_place_append_tag(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::from(AAD), &mut sealed)
        .map_err(|_| anyhow!("sealing failed"))?;
    Ok(STANDARD.encode([nonce.as_slice(), &sealed].concat()))
}

/// What the app's extension does with a sealed notification.
#[cfg_attr(not(test), allow(dead_code))]
pub fn open(key: &[u8], sealed: &str) -> Result<Vec<u8>> {
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, key).map_err(|_| anyhow!("bad key"))?);
    let data = STANDARD.decode(sealed)?;
    ensure!(data.len() > aead::NONCE_LEN, "too short");
    let (nonce, rest) = data.split_at(aead::NONCE_LEN);
    let mut rest = rest.to_vec();
    let plain = key
        .open_in_place(
            aead::Nonce::try_assume_unique_for_key(nonce).map_err(|_| anyhow!("bad nonce"))?,
            aead::Aad::from(AAD),
            &mut rest,
        )
        .map_err(|_| anyhow!("not sealed with this key"))?;
    Ok(plain.to_vec())
}

/// What the app shows, as a direct push carries it.
fn contents(fault: &Fault) -> Value {
    json!({
        "title": fault.title,
        "body": fault.body,
        "path": fault.path,
        "kind": fault.kind,
        "level": fault.level,
        "provider": fault.provider,
        "account": fault.account,
        "thread": if fault.provider.is_empty() { "fusebox" } else { fault.provider.as_str() },
    })
}

/// Ties notifications about the same fault together without telling the relay or
/// Apple which fault it is.
fn collapse_id(key: &[u8], fault_key: &str) -> String {
    let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), fault_key.as_bytes());
    tag.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Sends one notification through the relay. A device the relay or Apple no longer
/// knows comes back as `GONE`, so the caller removes it.
pub async fn send(
    client: &reqwest::Client,
    relay_url: &str,
    device: &Relayed,
    fault: &Fault,
    badge: usize,
    high: bool,
    ttl_seconds: u32,
) -> Result<StatusCode> {
    ensure!(!relay_url.is_empty(), "The push relay is turned off on this server (push-relay in config.yaml)");
    let key = STANDARD.decode(&device.key)?;
    let body = json!({
        "handle": device.handle,
        "sealed": seal(&key, contents(fault).to_string().as_bytes())?,
        "badge": badge,
        "collapse_id": collapse_id(&key, &fault.key),
        "priority": if high { "high" } else { "normal" },
        "ttl": ttl_seconds,
    });
    let resp = client
        .post(format!("{}/v1/send", relay_url.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .context("The push relay is unreachable")?;
    match resp.status().as_u16() {
        200..=299 => Ok(StatusCode::OK),
        410 => Ok(StatusCode::GONE),
        status => {
            let error = resp.json::<Value>().await.ok().and_then(|v| v["error"].as_str().map(String::from));
            Err(anyhow!("The push relay refused it: {}", error.unwrap_or_else(|| format!("status {status}"))))
        }
    }
}

// ---------------------------------------------------------------- the relay

/// How `fusebox push-relay` runs.
pub struct Settings {
    pub listen: SocketAddr,
    pub state_dir: PathBuf,
    pub apns: Apns,
}

struct Relay {
    db: Mutex<rusqlite::Connection>,
    apns: Apns,
    tokens: apns::Tokens,
    http: reqwest::Client,
    /// Apple's production and development servers; tests point both at a fake.
    bases: (String, String),
    limits: Mutex<Limits>,
}

/// Counts per key in fixed windows.
#[derive(Default)]
struct Limits {
    windows: HashMap<String, (Instant, u32)>,
}

impl Limits {
    /// Counts one more for `key`; false once that is over `max` in the window.
    fn allow(&mut self, key: String, max: u32, window: Duration) -> bool {
        let now = Instant::now();
        if self.windows.len() > 100_000 {
            self.windows.retain(|_, (start, _)| now.duration_since(*start) < window);
        }
        let (start, count) = self.windows.entry(key).or_insert((now, 0));
        if now.duration_since(*start) >= window {
            (*start, *count) = (now, 0);
        }
        *count += 1;
        *count <= max
    }
}

fn hash(handle: &str) -> Vec<u8> {
    digest::digest(&digest::SHA256, handle.as_bytes()).as_ref().to_vec()
}

fn open_db(path: &std::path::Path) -> Result<rusqlite::Connection> {
    let db = rusqlite::Connection::open(path).with_context(|| format!("could not open {}", path.display()))?;
    db.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS devices (
             handle_hash BLOB PRIMARY KEY,
             token TEXT NOT NULL,
             environment TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             used_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS devices_token ON devices(token);",
    )?;
    Ok(db)
}

impl Relay {
    fn new(db: rusqlite::Connection, apns: Apns, bases: (String, String)) -> Arc<Self> {
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(crate::tls::http(false))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("an HTTP client");
        Arc::new(Self {
            db: Mutex::new(db),
            apns,
            tokens: apns::Tokens::default(),
            http,
            bases,
            limits: Mutex::default(),
        })
    }

    /// Forgets handles nobody has used for a long time.
    fn purge(&self) -> usize {
        let before = Utc::now().timestamp() - UNUSED_DAYS * 86400;
        self.db.lock().execute("DELETE FROM devices WHERE used_at < ?1", [before]).unwrap_or(0)
    }

    fn store_device(&self, handle: &str, device: &apns::Device, now: i64) -> rusqlite::Result<()> {
        let environment = match device.environment {
            apns::Environment::Production => "production",
            apns::Environment::Development => "development",
        };
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO devices (handle_hash, token, environment, created_at, used_at) VALUES (?1, ?2, ?3, ?4, ?4)",
            rusqlite::params![hash(handle), device.token, environment, now],
        )?;
        // Timestamps have second resolution: newest insertion wins ties so the
        // handle returned by registration cannot already have been evicted.
        tx.execute(
            "DELETE FROM devices WHERE token = ?1 AND handle_hash NOT IN
                 (SELECT handle_hash FROM devices WHERE token = ?1 ORDER BY used_at DESC, created_at DESC, rowid DESC LIMIT ?2)",
            rusqlite::params![device.token, HANDLES_PER_TOKEN as i64],
        )?;
        tx.commit()
    }
}

fn router(relay: Arc<Relay>) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/v1/devices", post(register))
        .route("/v1/devices/{handle}", delete(unregister))
        .route("/v1/devices/{handle}/touch", post(touch))
        .route("/v1/send", post(send_one))
        .layer(DefaultBodyLimit::max(16 << 10))
        .with_state(relay)
}

pub async fn serve(settings: Settings) -> Result<()> {
    std::fs::create_dir_all(&settings.state_dir)
        .with_context(|| format!("could not create {}", settings.state_dir.display()))?;
    let db = open_db(&settings.state_dir.join("relay.sqlite3"))?;
    let bases = (apns::Environment::Production.base().into(), apns::Environment::Development.base().into());
    let relay = Relay::new(db, settings.apns, bases);
    relay.tokens_ready()?;
    let purged = relay.purge();
    let worker = relay.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(86400));
        loop {
            tick.tick().await;
            let n = worker.purge();
            if n > 0 {
                tracing::info!(devices = n, "forgot unused relay handles");
            }
        }
    });
    let listener = tokio::net::TcpListener::bind(settings.listen)
        .await
        .with_context(|| format!("could not listen on {}", settings.listen))?;
    tracing::info!(listen = %settings.listen, topic = %relay.apns.topic, purged, "push relay listening");
    axum::serve(listener, router(relay).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

impl Relay {
    /// Signs a provider token once at start, so a missing or wrong key stops the relay
    /// before it accepts devices.
    fn tokens_ready(&self) -> Result<()> {
        self.tokens.token(&self.apns).map(|_| ())
    }
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

/// The caller's address; behind a local tunnel or proxy, the one it names.
fn client_ip(addr: SocketAddr, headers: &HeaderMap) -> IpAddr {
    let peer = addr.ip().to_canonical();
    if peer.is_loopback()
        && let Some(ip) = headers
            .get("cf-connecting-ip")
            .or_else(|| headers.get("x-real-ip"))
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse().ok())
    {
        return ip;
    }
    peer
}

async fn health(State(relay): State<Arc<Relay>>) -> Json<Value> {
    let devices: i64 = relay.db.lock().query_row("SELECT COUNT(*) FROM devices", [], |r| r.get(0)).unwrap_or(0);
    Json(json!({ "ok": true, "devices": devices }))
}

#[derive(Deserialize)]
struct Registration {
    token: String,
    environment: apns::Environment,
}

async fn register(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<Registration>,
) -> Response {
    let ip = client_ip(addr, &headers);
    if !relay.limits.lock().allow(format!("register:{ip}"), REGISTRATIONS_PER_HOUR, HOUR) {
        return error(StatusCode::TOO_MANY_REQUESTS, "Too many registrations from this address; try again later");
    }
    let device = match apns::Device::new(&req.token, req.environment) {
        Ok(d) => d,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.to_string()),
    };
    let mut bytes = [0u8; HANDLE_BYTES];
    if SystemRandom::new().fill(&mut bytes).is_err() {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "no randomness");
    }
    let handle = URL_SAFE_NO_PAD.encode(bytes);
    match relay.store_device(&handle, &device, Utc::now().timestamp()) {
        Ok(_) => (StatusCode::CREATED, Json(json!({ "handle": handle }))).into_response(),
        Err(e) => {
            tracing::warn!("could not save a relay device: {e}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the device")
        }
    }
}

async fn unregister(State(relay): State<Arc<Relay>>, UrlPath(handle): UrlPath<String>) -> Response {
    match relay.db.lock().execute("DELETE FROM devices WHERE handle_hash = ?1", [hash(&handle)]) {
        Ok(0) => error(StatusCode::NOT_FOUND, "No such device"),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "Could not remove the device"),
    }
}

/// The app checks in when it opens, so a handle in use isn't forgotten while its
/// Fusebox has nothing to say; `GONE` tells it to register again.
async fn touch(State(relay): State<Arc<Relay>>, UrlPath(handle): UrlPath<String>) -> Response {
    let now = Utc::now().timestamp();
    match relay
        .db
        .lock()
        .execute("UPDATE devices SET used_at = ?1 WHERE handle_hash = ?2", rusqlite::params![now, hash(&handle)])
    {
        Ok(0) => error(StatusCode::GONE, "Unknown device; register again"),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "Could not update the device"),
    }
}

#[derive(Deserialize)]
struct Outgoing {
    handle: String,
    sealed: String,
    #[serde(default)]
    badge: u32,
    collapse_id: String,
    #[serde(default)]
    priority: String,
    ttl: u32,
}

fn check(out: &Outgoing) -> Result<(), &'static str> {
    if out.sealed.len() > SEALED_MAX || STANDARD.decode(&out.sealed).map_or(true, |d| d.len() <= aead::NONCE_LEN) {
        return Err("sealed must be base64, at most 3000 characters");
    }
    if out.collapse_id.is_empty()
        || out.collapse_id.len() > 64
        || !out.collapse_id.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err("collapse_id must be 1 to 64 visible ASCII characters");
    }
    if out.ttl > TTL_MAX || out.badge > 99_999 || !matches!(out.priority.as_str(), "" | "high" | "normal") {
        return Err("ttl, badge or priority out of range");
    }
    Ok(())
}

async fn send_one(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(out): Json<Outgoing>,
) -> Response {
    if let Err(problem) = check(&out) {
        return error(StatusCode::BAD_REQUEST, problem);
    }
    let key = hash(&out.handle);
    let ip = client_ip(addr, &headers);
    {
        let mut limits = relay.limits.lock();
        let per_handle = limits.allow(format!("send:{}", URL_SAFE_NO_PAD.encode(&key)), SENDS_PER_HANDLE_HOUR, HOUR);
        if !per_handle || !limits.allow(format!("sendip:{ip}"), SENDS_PER_ADDRESS_HOUR, HOUR) {
            return error(StatusCode::TOO_MANY_REQUESTS, "Too many notifications; try again later");
        }
    }
    let found: Option<(String, String)> = relay
        .db
        .lock()
        .query_row("SELECT token, environment FROM devices WHERE handle_hash = ?1", [&key], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .ok();
    let Some((token, environment)) = found else {
        return error(StatusCode::GONE, "Unknown device; it has to register again");
    };
    let base = if environment == "development" { &relay.bases.1 } else { &relay.bases.0 };
    let payload = json!({
        "aps": {
            "alert": { "title": "Fusebox", "body": "Open Fusebox to see what happened." },
            "mutable-content": 1,
            "badge": out.badge,
            "sound": "default",
            "thread-id": "fusebox",
        },
        "sealed": out.sealed,
        "v": 1,
    });
    let message = apns::Message {
        payload: &payload,
        collapse_id: &out.collapse_id,
        high: out.priority == "high",
        ttl_seconds: out.ttl,
    };
    match apns::post(&relay.http, base, &relay.tokens, &relay.apns, &token, &message).await {
        Ok(StatusCode::GONE) => {
            let n = relay.db.lock().execute("DELETE FROM devices WHERE token = ?1", [&token]).unwrap_or(0);
            tracing::info!(handles = n, "Apple no longer knows a relay device; forgot it");
            error(StatusCode::GONE, "Apple no longer knows this device")
        }
        Ok(_) => {
            let now = Utc::now().timestamp();
            let _ = relay
                .db
                .lock()
                .execute("UPDATE devices SET used_at = ?1 WHERE handle_hash = ?2", rusqlite::params![now, key]);
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => {
            tracing::warn!("relay push failed: {e:#}");
            error(StatusCode::BAD_GATEWAY, format!("{e:#}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use tokio::sync::mpsc;

    #[test]
    fn sealed_notifications_open_only_with_their_key() {
        let key = [7u8; KEY_BYTES];
        let sealed = seal(&key, b"{\"title\":\"x\"}").unwrap();
        assert_eq!(open(&key, &sealed).unwrap(), b"{\"title\":\"x\"}");
        assert_ne!(seal(&key, b"same").unwrap(), seal(&key, b"same").unwrap(), "a fresh nonce each time");
        assert!(open(&[8u8; KEY_BYTES], &sealed).is_err());
        let mut tampered = STANDARD.decode(&sealed).unwrap();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(open(&key, &STANDARD.encode(tampered)).is_err());
        assert_eq!(collapse_id(&key, "provider:claude").len(), 64);
        assert_ne!(collapse_id(&key, "provider:claude"), collapse_id(&[8u8; KEY_BYTES], "provider:claude"));

        assert!(Relayed::new("short", &STANDARD.encode(key)).is_err());
        let handle = URL_SAFE_NO_PAD.encode([1u8; HANDLE_BYTES]);
        assert!(Relayed::new(&handle, "AAAA").is_err());
        assert_eq!(Relayed::new(&handle, &STANDARD.encode(key)).unwrap().endpoint(), format!("relay:{handle}"));
    }

    /// The same vector is in the iPhone app's tests (ios/FuseboxTests), so both sides
    /// agree on the sealed format: base64 of nonce, ciphertext and tag, with the AAD.
    #[test]
    fn the_sealed_format_matches_the_apps_test_vector() {
        let key = STANDARD.decode("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=").unwrap();
        let sealed = "ZGVmZ2hpamtsbW5vMzmqDw2FM7wEQByEuxAOmGKxb23lQZoch7TUOJLRwCy2xWKiI3biJsfRDUyYMPuPfiP5ilAmJAaZnsH2geTA7nusqGexJVJr7E+ELTHrVocNFjEWi+f9p6uZFP5ZMGEw7RL9CwU5y1HQlr+wxmiVnVFO8i7vE8pb3Fu/k7HdZqMQpbrbAFUsQqdreifl5Srn+uXHZDU59LAdsw4/j2fMnvVDBWayv42BHjzE1cBWwq31nrjHQ5Fo7RttcD2m5TK7uUp024Z/B24sXUdBMVKAbSEstDilO2Dch3xn9pGSp6gJOzXHramsjmeW4IC5";
        let opened: Value = serde_json::from_slice(&open(&key, sealed).unwrap()).unwrap();
        assert_eq!(opened["title"], "Claude sign-in expired");
        assert_eq!(opened["path"], "#/accounts/claude-user1.json");
        assert_eq!(opened["thread"], "claude");
    }

    #[test]
    fn limits_count_per_key_and_window() {
        let mut limits = Limits::default();
        assert!((0..3).all(|_| limits.allow("a".into(), 3, HOUR)));
        assert!(!limits.allow("a".into(), 3, HOUR));
        assert!(limits.allow("b".into(), 3, HOUR));
        assert!(limits.allow("a".into(), 3, Duration::ZERO), "a new window starts over");
    }

    #[tokio::test]
    async fn registering_at_the_handle_cap_keeps_the_new_handle_when_timestamps_tie() {
        let relay = Relay::new(
            open_db(std::path::Path::new(":memory:")).unwrap(),
            Apns { team_id: "test".into(), key_id: "test".into(), key_file: "unused".into(), topic: "test".into() },
            (String::new(), String::new()),
        );
        let device = apns::Device::new(&"c3".repeat(32), apns::Environment::Production).unwrap();
        let handles: Vec<_> =
            (0..=HANDLES_PER_TOKEN).map(|i| URL_SAFE_NO_PAD.encode([i as u8; HANDLE_BYTES])).collect();
        // Registration timestamps have second resolution. Rapid registrations can tie.
        for handle in &handles {
            relay.store_device(handle, &device, 1_700_000_000).unwrap();
        }
        let count: i64 = relay.db.lock().query_row("SELECT COUNT(*) FROM devices", [], |r| r.get(0)).unwrap();
        assert_eq!(count, HANDLES_PER_TOKEN as i64);
        let newest = handles.last().unwrap().clone();
        assert_eq!(touch(State(relay.clone()), UrlPath(newest)).await.status(), StatusCode::NO_CONTENT);
        assert_eq!(touch(State(relay), UrlPath(handles[0].clone())).await.status(), StatusCode::GONE);
    }

    struct Seen {
        path: String,
        headers: HeaderMap,
        body: Bytes,
    }

    #[tokio::test]
    async fn a_fusebox_reaches_a_device_through_the_relay_without_its_token() {
        let dir = std::env::temp_dir().join(format!("fusebox-relay-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let key_file = dir.join("AuthKey_KEY1234567.p8");
        let der = super::super::crypto::Vapid::generate().unwrap();
        std::fs::write(
            &key_file,
            format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", STANDARD.encode(der)),
        )
        .unwrap();
        let cfg = Apns {
            team_id: "ABCDE12345".into(),
            key_id: "KEY1234567".into(),
            key_file: key_file.to_string_lossy().into(),
            topic: "com.digitalpals.fusebox".into(),
        };

        // Apple: accepts the first push, then says the token is gone.
        let (tx, mut rx) = mpsc::unbounded_channel::<Seen>();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let apple = Router::new().route(
            "/3/device/{token}",
            post(move |UrlPath(token): UrlPath<String>, headers: HeaderMap, body: Bytes| {
                let tx = tx.clone();
                let calls = calls.clone();
                async move {
                    let _ = tx.send(Seen { path: token, headers, body });
                    match calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                        0 => (StatusCode::OK, String::new()),
                        _ => (StatusCode::GONE, r#"{"reason":"Unregistered"}"#.into()),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let apple_url = format!("http://{}", listener.local_addr().unwrap());
        let apple = tokio::spawn(async move { axum::serve(listener, apple).await.unwrap() });

        let relay = Relay::new(open_db(&dir.join("relay.sqlite3")).unwrap(), cfg, (apple_url.clone(), apple_url));
        relay.tokens_ready().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay_url = format!("http://{}", listener.local_addr().unwrap());
        let app = router(relay.clone()).into_make_service_with_connect_info::<SocketAddr>();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();

        // The phone registers its token and keeps the handle and a key of its own.
        let token = "c3".repeat(32);
        let bad = client
            .post(format!("{relay_url}/v1/devices"))
            .json(&json!({ "token": "nope", "environment": "production" }));
        assert_eq!(bad.send().await.unwrap().status(), reqwest::StatusCode::BAD_REQUEST);
        let registered: Value = client
            .post(format!("{relay_url}/v1/devices"))
            .json(&json!({ "token": token, "environment": "production" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let handle = registered["handle"].as_str().unwrap().to_string();
        let key = [42u8; KEY_BYTES];
        let device = Relayed::new(&handle, &STANDARD.encode(key)).unwrap();
        let touched = client.post(format!("{relay_url}/v1/devices/{handle}/touch")).send().await.unwrap();
        assert_eq!(touched.status(), reqwest::StatusCode::NO_CONTENT);

        let fault = Fault {
            key: "signin:claude-work.json".into(),
            provider: "claude".into(),
            title: "Claude sign-in expired".into(),
            body: "work@example.com needs signing in again.".into(),
            path: "#/accounts/claude-work.json".into(),
            kind: "signin",
            level: "err",
            account: Some("claude-work.json".into()),
            ..Default::default()
        };
        assert_eq!(send(&client, &relay_url, &device, &fault, 2, true, 3600).await.unwrap(), StatusCode::OK);
        let seen = rx.recv().await.unwrap();
        assert_eq!(seen.path, token, "the relay knows the token; Fusebox never did");
        assert_eq!(seen.headers["apns-priority"], "10");
        assert_eq!(seen.headers["apns-collapse-id"].to_str().unwrap(), collapse_id(&key, &fault.key));
        let payload: Value = serde_json::from_slice(&seen.body).unwrap();
        assert_eq!(payload["aps"]["mutable-content"], 1);
        assert_eq!(payload["aps"]["badge"], 2);
        assert!(!seen.body.windows(6).any(|w| w == b"Claude"), "the relay and Apple see no contents");
        let opened: Value = serde_json::from_slice(&open(&key, payload["sealed"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(opened["title"], "Claude sign-in expired");
        assert_eq!(opened["path"], "#/accounts/claude-work.json");
        assert_eq!(opened["thread"], "claude");

        // Apple forgot the token: the relay forgets it, and Fusebox hears the device is gone.
        assert_eq!(send(&client, &relay_url, &device, &fault, 0, false, 60).await.unwrap(), StatusCode::GONE);
        assert_eq!(send(&client, &relay_url, &device, &fault, 0, false, 60).await.unwrap(), StatusCode::GONE);
        let touched = client.post(format!("{relay_url}/v1/devices/{handle}/touch")).send().await.unwrap();
        assert_eq!(touched.status(), reqwest::StatusCode::GONE);
        assert!(send(&client, "", &device, &fault, 0, false, 60).await.unwrap_err().to_string().contains("turned off"));

        // A Fusebox can't push unbounded volumes, or arbitrary payloads.
        let again: Value = client
            .post(format!("{relay_url}/v1/devices"))
            .json(&json!({ "token": token, "environment": "development" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let oversized = json!({ "handle": again["handle"], "sealed": "A".repeat(4000), "collapse_id": "x", "ttl": 60 });
        let resp = client.post(format!("{relay_url}/v1/send")).json(&oversized).send().await.unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
        let removed = client
            .delete(format!("{relay_url}/v1/devices/{}", again["handle"].as_str().unwrap()))
            .send()
            .await
            .unwrap();
        assert_eq!(removed.status(), reqwest::StatusCode::NO_CONTENT);
        let health: Value = client.get(format!("{relay_url}/healthz")).send().await.unwrap().json().await.unwrap();
        assert_eq!(health["devices"], 0);

        server.abort();
        apple.abort();
        drop(relay);
        // Windows keeps the database open until the aborted server lets go of it.
        let _ = std::fs::remove_dir_all(&dir);
    }
}
