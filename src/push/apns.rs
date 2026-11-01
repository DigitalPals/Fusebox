//! Native push for the Fusebox iPhone app through the Apple Push Notification service:
//! an ES256 provider token signed with the developer account's `.p8` key, and one
//! HTTP/2 request per device.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, ensure};
use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::Utc;
use parking_lot::Mutex;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::faults::Fault;
use crate::config::Apns;

/// Apple refuses provider tokens older than an hour, and new ones more often than
/// every 20 minutes.
const TOKEN_LIFETIME: Duration = Duration::from_secs(50 * 60);
const PRODUCTION: &str = "https://api.push.apple.com";
const DEVELOPMENT: &str = "https://api.sandbox.push.apple.com";
/// Apple's limit on `apns-collapse-id`.
const COLLAPSE_ID_MAX: usize = 64;

/// Which APNs servers know a device token: development for builds run from Xcode,
/// production for TestFlight and the App Store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Production,
    Development,
}

/// A registered iPhone app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    /// The device token, lowercase hex.
    pub token: String,
    pub environment: Environment,
}

impl Environment {
    /// Where its notifications are sent.
    pub fn base(self) -> &'static str {
        match self {
            Self::Production => PRODUCTION,
            Self::Development => DEVELOPMENT,
        }
    }
}

impl Device {
    pub fn new(token: &str, environment: Environment) -> Result<Self> {
        let token = token.trim().to_ascii_lowercase();
        ensure!(
            (16..=200).contains(&token.len())
                && token.len().is_multiple_of(2)
                && token.bytes().all(|b| b.is_ascii_hexdigit()),
            "The device token must be hex"
        );
        Ok(Self { token, environment })
    }

    /// The subscription's endpoint, which tells devices apart.
    pub fn endpoint(&self) -> String {
        format!("apns:{}", self.token)
    }
}

/// The provider token, made again before Apple stops accepting it or when the
/// configured key changes.
#[derive(Default)]
pub struct Tokens {
    cached: Mutex<Option<(Apns, Instant, String)>>,
}

impl Tokens {
    pub(super) fn token(&self, cfg: &Apns) -> Result<String> {
        let mut cached = self.cached.lock();
        if let Some((made_for, made, token)) = cached.as_ref()
            && made_for == cfg
            && made.elapsed() < TOKEN_LIFETIME
        {
            return Ok(token.clone());
        }
        let text = std::fs::read_to_string(&cfg.key_file)
            .with_context(|| format!("could not read the APNs key {}", cfg.key_file))?;
        let token = provider_token(&pkcs8(&text)?, cfg, Utc::now().timestamp())?;
        *cached = Some((cfg.clone(), Instant::now(), token.clone()));
        Ok(token)
    }

    /// Apple refused the token: the next push signs a new one.
    fn forget(&self) {
        *self.cached.lock() = None;
    }
}

/// The DER key inside a `.p8` file. Also takes the PEM with its line breaks lost.
fn pkcs8(text: &str) -> Result<Vec<u8>> {
    let body: String = text
        .replace("-----BEGIN PRIVATE KEY-----", "")
        .replace("-----END PRIVATE KEY-----", "")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    STANDARD.decode(body).map_err(|_| anyhow!("the APNs key is not a .p8 private key"))
}

fn provider_token(pkcs8: &[u8], cfg: &Apns, now: i64) -> Result<String> {
    let rng = SystemRandom::new();
    let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8, &rng)
        .map_err(|e| anyhow!("unusable APNs key: {e}"))?;
    let header = URL_SAFE_NO_PAD.encode(json!({ "alg": "ES256", "kid": cfg.key_id }).to_string());
    let claims = URL_SAFE_NO_PAD.encode(json!({ "iss": cfg.team_id, "iat": now }).to_string());
    let signing_input = format!("{header}.{claims}");
    let sig = key.sign(&rng, signing_input.as_bytes()).map_err(|_| anyhow!("APNs token signing failed"))?;
    Ok(format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref())))
}

/// The notification as the app gets it: the alert, the fault count for the icon, and
/// where a tap should land.
pub fn payload(fault: &Fault, badge: usize) -> Value {
    let thread = if fault.provider.is_empty() { "fusebox" } else { fault.provider.as_str() };
    let mut payload = json!({
        "aps": {
            "alert": { "title": fault.title, "body": fault.body },
            "badge": badge,
            "sound": "default",
            "thread-id": thread,
        },
        "path": fault.path,
        "kind": fault.kind,
        "level": fault.level,
        "provider": fault.provider,
    });
    if let Some(account) = &fault.account {
        payload["account"] = json!(account);
    }
    payload
}

/// Faults keep their notification's tag: a newer one replaces the older on the device.
fn collapse_id(key: &str) -> String {
    if key.len() <= COLLAPSE_ID_MAX {
        return key.to_string();
    }
    let digest = ring::digest::digest(&ring::digest::SHA256, key.as_bytes());
    digest.as_ref().iter().take(COLLAPSE_ID_MAX / 2).map(|b| format!("{b:02x}")).collect()
}

/// Sends one notification. A token Apple no longer knows comes back as `GONE`, so the
/// caller removes the device; any other refusal is an error with Apple's reason.
#[allow(clippy::too_many_arguments)]
pub async fn send(
    client: &reqwest::Client,
    base: &str,
    tokens: &Tokens,
    cfg: Option<&Apns>,
    device: &Device,
    fault: &Fault,
    badge: usize,
    high: bool,
    ttl_seconds: u32,
) -> Result<StatusCode> {
    let cfg = cfg.ok_or_else(|| anyhow!("Apple push isn't set up on this server (apns in config.yaml)"))?;
    let message = Message { payload: &payload(fault, badge), collapse_id: &collapse_id(&fault.key), high, ttl_seconds };
    post(client, base, tokens, cfg, &device.token, &message).await
}

/// One alert as Apple's push service takes it.
pub struct Message<'a> {
    pub payload: &'a Value,
    pub collapse_id: &'a str,
    pub high: bool,
    pub ttl_seconds: u32,
}

/// Posts one alert to a device token; shared with the push relay.
pub async fn post(
    client: &reqwest::Client,
    base: &str,
    tokens: &Tokens,
    cfg: &Apns,
    device_token: &str,
    message: &Message<'_>,
) -> Result<StatusCode> {
    let resp = client
        .post(format!("{base}/3/device/{device_token}"))
        .header("authorization", format!("bearer {}", tokens.token(cfg)?))
        .header("apns-topic", &cfg.topic)
        .header("apns-push-type", "alert")
        .header("apns-priority", if message.high { "10" } else { "5" })
        .header("apns-expiration", (Utc::now().timestamp() + i64::from(message.ttl_seconds)).to_string())
        .header("apns-collapse-id", message.collapse_id)
        .json(message.payload)
        .send()
        .await
        .context("Apple's push service is unreachable")?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if status.is_success() {
        return Ok(status);
    }
    let reason = resp
        .json::<Value>()
        .await
        .ok()
        .and_then(|v| v["reason"].as_str().map(String::from))
        .unwrap_or_else(|| status.to_string());
    match reason.as_str() {
        "Unregistered" | "BadDeviceToken" | "DeviceTokenNotForTopic" => Ok(StatusCode::GONE),
        _ => {
            if matches!(
                reason.as_str(),
                "InvalidProviderToken" | "ExpiredProviderToken" | "TooManyProviderTokenUpdates"
            ) {
                tokens.forget();
            }
            Err(anyhow!("Apple's push service refused it: {reason}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, KeyPair, UnparsedPublicKey};

    fn cfg(key_file: &str) -> Apns {
        Apns {
            team_id: "ABCDE12345".into(),
            key_id: "KEY1234567".into(),
            key_file: key_file.into(),
            topic: "com.digitalpals.fusebox".into(),
        }
    }

    fn p8(der: &[u8]) -> String {
        let b64 = STANDARD.encode(der);
        let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
        format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", lines.join("\n"))
    }

    #[test]
    fn provider_token_is_an_es256_jwt_apple_can_verify() {
        let der = super::super::crypto::Vapid::generate().unwrap();
        let file = p8(&der);
        assert_eq!(pkcs8(&file).unwrap(), der);
        // Pasted into a one-line field, the PEM loses its line breaks.
        assert_eq!(pkcs8(&file.replace('\n', " ")).unwrap(), der);

        let token = provider_token(&der, &cfg("unused"), 1_793_520_000).unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(header, json!({ "alg": "ES256", "kid": "KEY1234567" }));
        assert_eq!(claims, json!({ "iss": "ABCDE12345", "iat": 1_793_520_000 }));

        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &SystemRandom::new()).unwrap();
        let public = UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, key.public_key().as_ref().to_vec());
        let signed = format!("{}.{}", parts[0], parts[1]);
        public.verify(signed.as_bytes(), &URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
    }

    #[test]
    fn tokens_are_reused_until_the_key_changes() {
        let dir = std::env::temp_dir().join(format!("fusebox-apns-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("AuthKey_KEY1234567.p8");
        std::fs::write(&path, p8(&super::super::crypto::Vapid::generate().unwrap())).unwrap();
        let tokens = Tokens::default();
        let mut config = cfg(path.to_str().unwrap());
        let first = tokens.token(&config).unwrap();
        assert_eq!(tokens.token(&config).unwrap(), first);
        config.key_id = "KEY7654321".into();
        assert_ne!(tokens.token(&config).unwrap(), first);
        tokens.forget();
        config.key_file = dir.join("missing.p8").to_string_lossy().into_owned();
        assert!(tokens.token(&config).unwrap_err().to_string().contains("could not read the APNs key"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn device_tokens_and_payloads() {
        assert!(Device::new("not hex", Environment::Production).is_err());
        assert!(Device::new("abc", Environment::Production).is_err());
        let device = Device::new(&"AB".repeat(32), Environment::Development).unwrap();
        assert_eq!(device.token, "ab".repeat(32));
        assert_eq!(device.endpoint(), format!("apns:{}", "ab".repeat(32)));

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
        let p = payload(&fault, 3);
        assert_eq!(p["aps"]["alert"]["title"], "Claude sign-in expired");
        assert_eq!(p["aps"]["badge"], 3);
        assert_eq!(p["aps"]["thread-id"], "claude");
        assert_eq!(p["path"], "#/accounts/claude-work.json");
        assert_eq!(p["account"], "claude-work.json");
        assert_eq!(p["kind"], "signin");
        let test = payload(&Fault { key: "test".into(), ..Default::default() }, 0);
        assert_eq!(test["aps"]["thread-id"], "fusebox");
        assert!(test.get("account").is_none());

        assert_eq!(collapse_id("quota:a"), "quota:a");
        let long = collapse_id(&format!("address:{}:100.64.0.1", "x".repeat(80)));
        assert_eq!(long.len(), COLLAPSE_ID_MAX);
    }
}
