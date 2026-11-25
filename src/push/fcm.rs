//! Native push for the Fusebox Android app through Firebase Cloud Messaging (HTTP v1):
//! an OAuth2 access token minted from a Firebase service account JSON key, and one
//! HTTPS request per device.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, ensure};
use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::Utc;
use parking_lot::Mutex;
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::faults::Fault;
use crate::config::Fcm;

const BASE: &str = "https://fcm.googleapis.com";
const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
/// Access tokens last an hour; renew with a 10-minute margin.
const TOKEN_LIFETIME: Duration = Duration::from_secs(50 * 60);
/// FCM's limit on `android.collapse_key`.
const COLLAPSE_KEY_MAX: usize = 64;

/// A registered Android app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    /// The Firebase Cloud Messaging registration token.
    pub token: String,
}

impl Device {
    pub fn new(token: &str) -> Result<Self> {
        let token = token.trim();
        ensure!(
            (32..=512).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_graphic()),
            "The FCM device token is not valid"
        );
        Ok(Self { token: token.to_string() })
    }

    /// The subscription's endpoint, which tells devices apart.
    pub fn endpoint(&self) -> String {
        format!("fcm:{}", self.token)
    }
}

/// The default FCM HTTP v1 API base URL.
pub fn base_url() -> &'static str {
    BASE
}

struct ServiceAccount {
    project_id: String,
    client_email: String,
    private_key: String,
    private_key_id: Option<String>,
    token_uri: String,
}

fn load_service_account(path: &str) -> Result<ServiceAccount> {
    let text = std::fs::read_to_string(path).with_context(|| format!("could not read the FCM key {path}"))?;
    parse_service_account(&text)
}

fn parse_service_account(text: &str) -> Result<ServiceAccount> {
    let v: Value = serde_json::from_str(text.trim()).context("the FCM key is not valid JSON")?;
    ensure!(v["type"] == "service_account", "expected a Firebase service account key (\"type\": \"service_account\")");
    let project_id = v["project_id"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow!("FCM service account has no project_id"))?
        .trim()
        .to_string();
    let client_email = v["client_email"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow!("FCM service account has no client_email"))?
        .trim()
        .to_string();
    let private_key = v["private_key"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow!("FCM service account has no private_key"))?
        .to_string();
    let private_key_id = v["private_key_id"].as_str().filter(|s| !s.is_empty()).map(String::from);
    let token_uri = v["token_uri"].as_str().filter(|s| !s.is_empty()).unwrap_or(DEFAULT_TOKEN_URI).to_string();
    Ok(ServiceAccount { project_id, client_email, private_key, private_key_id, token_uri })
}

fn pem_der(pem: &str) -> Result<(Vec<u8>, bool)> {
    let pkcs8 = pem.contains("BEGIN PRIVATE KEY");
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).map(str::trim).collect();
    let der = STANDARD.decode(body).context("FCM private_key is not valid PEM")?;
    Ok((der, pkcs8))
}

fn parse_rsa_key(pem: &str) -> Result<RsaKeyPair> {
    let (der, pkcs8) = pem_der(pem)?;
    if pkcs8 { RsaKeyPair::from_pkcs8(&der) } else { RsaKeyPair::from_der(&der) }
        .map_err(|e| anyhow!("unusable FCM private_key: {e}"))
}

fn sign_jwt(sa: &ServiceAccount, now: i64) -> Result<String> {
    let mut header = json!({ "alg": "RS256", "typ": "JWT" });
    if let Some(kid) = &sa.private_key_id {
        header["kid"] = kid.clone().into();
    }
    let claims = json!({
        "iss": sa.client_email,
        "scope": SCOPE,
        "aud": sa.token_uri,
        "iat": now,
        "exp": now + 3600,
    });
    let signing_input =
        format!("{}.{}", URL_SAFE_NO_PAD.encode(header.to_string()), URL_SAFE_NO_PAD.encode(claims.to_string()));
    let pair = parse_rsa_key(&sa.private_key)?;
    let mut sig = vec![0u8; pair.public().modulus_len()];
    pair.sign(&RSA_PKCS1_SHA256, &SystemRandom::new(), signing_input.as_bytes(), &mut sig)
        .map_err(|_| anyhow!("FCM JWT signing failed"))?;
    Ok(format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(sig)))
}

struct CachedToken {
    cfg: Fcm,
    project_id: String,
    access_token: String,
    expires_at: Instant,
}

/// Cached Google OAuth2 access token for FCM HTTP v1, renewed before expiry or when
/// the configured service account changes.
#[derive(Default)]
pub struct Tokens {
    cached: Mutex<Option<CachedToken>>,
}

impl Tokens {
    /// Checks at startup that the service account file and RSA private key can be read.
    pub fn check_config(&self, cfg: &Fcm) -> Result<String> {
        let sa = load_service_account(&cfg.service_account_file)?;
        parse_rsa_key(&sa.private_key)?;
        Ok(sa.project_id)
    }

    /// Returns `(project_id, access_token)`, minting a fresh token if needed.
    pub(super) async fn token(&self, client: &reqwest::Client, cfg: &Fcm) -> Result<(String, String)> {
        {
            let cached = self.cached.lock();
            if let Some(c) = cached.as_ref()
                && c.cfg == *cfg
                && Instant::now() < c.expires_at
            {
                return Ok((c.project_id.clone(), c.access_token.clone()));
            }
        }
        let sa = load_service_account(&cfg.service_account_file)?;
        let jwt = sign_jwt(&sa, Utc::now().timestamp())?;
        let resp = client
            .post(&sa.token_uri)
            .form(&[("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"), ("assertion", jwt.as_str())])
            .send()
            .await
            .context("Google OAuth token endpoint is unreachable")?;
        let status = resp.status();
        let body: Value = resp.json().await.context("unreadable response from Google OAuth token endpoint")?;
        if !status.is_success() {
            let reason = body["error_description"]
                .as_str()
                .or_else(|| body["error"].as_str())
                .unwrap_or("token exchange failed");
            return Err(anyhow!("FCM token exchange refused ({status}): {reason}"));
        }
        let access_token =
            body["access_token"].as_str().ok_or_else(|| anyhow!("missing access_token from Google OAuth"))?.to_string();
        let expires_in = body["expires_in"].as_u64().unwrap_or(3600).saturating_sub(300);
        let ttl = Duration::from_secs(expires_in).min(TOKEN_LIFETIME).max(Duration::from_secs(60));
        let project_id = sa.project_id;
        *self.cached.lock() = Some(CachedToken {
            cfg: cfg.clone(),
            project_id: project_id.clone(),
            access_token: access_token.clone(),
            expires_at: Instant::now() + ttl,
        });
        Ok((project_id, access_token))
    }

    fn forget(&self) {
        *self.cached.lock() = None;
    }

    #[cfg(test)]
    pub(super) fn seed_test_token(&self, cfg: &Fcm, project_id: &str, access_token: &str) {
        *self.cached.lock() = Some(CachedToken {
            cfg: cfg.clone(),
            project_id: project_id.to_string(),
            access_token: access_token.to_string(),
            expires_at: Instant::now() + TOKEN_LIFETIME,
        });
    }
}

/// The data-only FCM payload for direct delivery: every value is a string as FCM v1 requires.
/// Data-only delivery wakes `FirebaseMessagingService.onMessageReceived` in foreground and
/// background so the Android app can style, group and badge notifications consistently.
pub fn payload(fault: &Fault, badge: usize) -> Value {
    let thread = if fault.provider.is_empty() { "fusebox" } else { fault.provider.as_str() };
    let mut data = json!({
        "title": fault.title,
        "body": fault.body,
        "path": fault.path,
        "kind": fault.kind,
        "level": fault.level,
        "provider": fault.provider,
        "thread": thread,
        "badge": badge.to_string(),
    });
    if let Some(account) = &fault.account {
        data["account"] = json!(account);
    }
    data
}

/// Faults keep their notification's collapse key so a newer one replaces the older on the device.
fn collapse_key(key: &str) -> String {
    if key.len() <= COLLAPSE_KEY_MAX {
        return key.to_string();
    }
    let digest = ring::digest::digest(&ring::digest::SHA256, key.as_bytes());
    digest.as_ref().iter().take(COLLAPSE_KEY_MAX / 2).map(|b| format!("{b:02x}")).collect()
}

/// One data message as Firebase Cloud Messaging HTTP v1 takes it.
pub struct Message<'a> {
    pub data: &'a Value,
    pub collapse_key: &'a str,
    pub high: bool,
    pub ttl_seconds: u32,
}

/// Sends one notification directly through FCM. A token Firebase no longer knows comes
/// back as `GONE`, so the caller removes the device.
#[allow(clippy::too_many_arguments)]
pub async fn send(
    client: &reqwest::Client,
    base: &str,
    tokens: &Tokens,
    cfg: Option<&Fcm>,
    device: &Device,
    fault: &Fault,
    badge: usize,
    high: bool,
    ttl_seconds: u32,
) -> Result<StatusCode> {
    let cfg = cfg.ok_or_else(|| anyhow!("Firebase push isn't set up on this server (fcm in config.yaml)"))?;
    let data = payload(fault, badge);
    let key = collapse_key(&fault.key);
    let message = Message { data: &data, collapse_key: &key, high, ttl_seconds };
    post(client, base, tokens, cfg, &device.token, &message).await
}

/// Posts one data message to an FCM registration token; shared with the push relay.
pub async fn post(
    client: &reqwest::Client,
    base: &str,
    tokens: &Tokens,
    cfg: &Fcm,
    device_token: &str,
    message: &Message<'_>,
) -> Result<StatusCode> {
    let (project_id, access_token) = tokens.token(client, cfg).await?;
    let body = json!({
        "message": {
            "token": device_token,
            "data": message.data,
            "android": {
                "priority": if message.high { "high" } else { "normal" },
                "ttl": format!("{}s", message.ttl_seconds),
                "collapse_key": message.collapse_key,
            },
        }
    });
    let url = format!("{}/v1/projects/{project_id}/messages:send", base.trim_end_matches('/'));
    let resp = client
        .post(url)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
        .context("Firebase Cloud Messaging is unreachable")?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if status.is_success() {
        return Ok(status);
    }
    let err_json = resp.json::<Value>().await.ok();
    if is_unregistered(status, err_json.as_ref()) {
        return Ok(StatusCode::GONE);
    }
    if status == StatusCode::UNAUTHORIZED {
        tokens.forget();
    }
    let reason = err_json
        .as_ref()
        .and_then(|v| {
            v["error"]["details"]
                .as_array()
                .and_then(|arr| arr.iter().find_map(|d| d["errorCode"].as_str()))
                .or_else(|| v["error"]["status"].as_str())
                .or_else(|| v["error"]["message"].as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| status.to_string());
    Err(anyhow!("Firebase Cloud Messaging refused it: {reason}"))
}

fn is_unregistered(status: StatusCode, body: Option<&Value>) -> bool {
    if status == StatusCode::NOT_FOUND {
        return true;
    }
    let Some(body) = body else { return false };
    let code = body["error"]["details"].as_array().and_then(|arr| arr.iter().find_map(|d| d["errorCode"].as_str()));
    if matches!(code, Some("UNREGISTERED" | "SENDER_ID_MISMATCH")) {
        return true;
    }
    if status == StatusCode::BAD_REQUEST
        && body["error"]["status"].as_str() == Some("INVALID_ARGUMENT")
        && body["error"]["message"].as_str().is_some_and(|m| m.to_ascii_lowercase().contains("registration token"))
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_tokens_and_payloads() {
        assert!(Device::new("short").is_err());
        assert!(Device::new(&format!("{} {}", "a".repeat(20), "b".repeat(20))).is_err());
        let tok = format!("dToken123:APA91b{}", "x".repeat(40));
        let device = Device::new(&tok).unwrap();
        assert_eq!(device.token, tok);
        assert_eq!(device.endpoint(), format!("fcm:{tok}"));

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
        assert_eq!(p["title"], "Claude sign-in expired");
        assert_eq!(p["body"], "work@example.com needs signing in again.");
        assert_eq!(p["badge"], "3");
        assert_eq!(p["thread"], "claude");
        assert_eq!(p["path"], "#/accounts/claude-work.json");
        assert_eq!(p["account"], "claude-work.json");
        assert_eq!(p["kind"], "signin");
        let test = payload(&Fault { key: "test".into(), ..Default::default() }, 0);
        assert_eq!(test["thread"], "fusebox");
        assert!(test.get("account").is_none());

        assert_eq!(collapse_key("quota:a"), "quota:a");
        assert_eq!(collapse_key(&"x".repeat(80)).len(), COLLAPSE_KEY_MAX);
    }

    #[test]
    fn service_account_validation_rejects_bad_keys() {
        assert!(parse_service_account("not json").is_err());
        assert!(parse_service_account(r#"{"type":"user"}"#).is_err());
        assert!(parse_service_account(r#"{"type":"service_account","project_id":"p"}"#).is_err());
        let sa = parse_service_account(
            r#"{"type":"service_account","project_id":"fusebox-dev","client_email":"fcm@fusebox-dev.iam.gserviceaccount.com","private_key":"unused"}"#,
        )
        .unwrap();
        assert_eq!(sa.project_id, "fusebox-dev");
        assert_eq!(sa.token_uri, DEFAULT_TOKEN_URI);
    }

    #[test]
    fn unregistered_detection_handles_fcm_v1_errors() {
        assert!(is_unregistered(StatusCode::NOT_FOUND, None));
        let unreg = json!({ "error": { "status": "NOT_FOUND", "details": [{ "errorCode": "UNREGISTERED" }] } });
        assert!(is_unregistered(StatusCode::BAD_REQUEST, Some(&unreg)));
        let bad_tok = json!({ "error": { "status": "INVALID_ARGUMENT", "message": "The registration token is not a valid FCM registration token" } });
        assert!(is_unregistered(StatusCode::BAD_REQUEST, Some(&bad_tok)));
        let other = json!({ "error": { "status": "INTERNAL", "message": "try again" } });
        assert!(!is_unregistered(StatusCode::INTERNAL_SERVER_ERROR, Some(&other)));
    }
}
