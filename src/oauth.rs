//! Browser OAuth login (Claude, Codex, Antigravity) and token refresh for
//! every provider with expiring credentials.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::accounts::{Account, Credential, OAuth, PendingOAuthSave, Provider, write_oauth_file};
use crate::antigravity;
use crate::device;
use crate::state::App;

/// Refreshes must outlive cancelled client requests, but must finish before the
/// runtime exits. Registration and shutdown use one lock so none can slip past
/// the final drain.
#[derive(Clone, Default)]
pub struct RefreshTasks {
    inner: Arc<RefreshTaskState>,
}

#[derive(Default)]
struct RefreshTaskState {
    lifecycle: parking_lot::Mutex<RefreshLifecycle>,
    idle: tokio::sync::Notify,
}

#[derive(Default)]
struct RefreshLifecycle {
    closing: bool,
    active: usize,
}

struct RefreshRegistration(Arc<RefreshTaskState>);

impl RefreshTasks {
    fn register(&self) -> Result<RefreshRegistration> {
        let mut lifecycle = self.inner.lifecycle.lock();
        if lifecycle.closing {
            bail!("token refresh is unavailable while the server is shutting down");
        }
        lifecycle.active += 1;
        Ok(RefreshRegistration(self.inner.clone()))
    }

    pub async fn shutdown(&self) {
        loop {
            let idle = self.inner.idle.notified();
            tokio::pin!(idle);
            // Register the waiter before checking the count, avoiding a missed
            // notification when the last worker exits concurrently.
            idle.as_mut().enable();
            {
                let mut lifecycle = self.inner.lifecycle.lock();
                lifecycle.closing = true;
                if lifecycle.active == 0 {
                    return;
                }
            }
            idle.await;
        }
    }
}

impl Drop for RefreshRegistration {
    fn drop(&mut self) {
        let mut lifecycle = self.0.lifecycle.lock();
        lifecycle.active -= 1;
        if lifecycle.active == 0 {
            self.0.idle.notify_waiters();
        }
    }
}

pub mod claude {
    pub const AUTH_URL: &str = "https://claude.ai/oauth/authorize";
    pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
    pub const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
    pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
    pub const REDIRECT: &str = "http://localhost:54545/callback";
    pub const PORT: u16 = 54545;
    pub const SCOPE: &str = "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
}

pub mod codex {
    pub const AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
    pub const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
    pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
    pub const REDIRECT: &str = "http://localhost:1455/auth/callback";
    pub const PORT: u16 = 1455;
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn pkce() -> Pkce {
    let bytes: Vec<u8> = (0..96).map(|_| rand::random::<u8>()).collect();
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    Pkce { verifier, challenge }
}

pub fn random_state() -> String {
    let bytes: Vec<u8> = (0..24).map(|_| rand::random::<u8>()).collect();
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn auth_url(provider: Provider, state: &str, p: &Pkce) -> String {
    let (base, params): (&str, Vec<(&str, &str)>) = match provider {
        Provider::Claude => (
            claude::AUTH_URL,
            vec![
                ("code", "true"),
                ("client_id", claude::CLIENT_ID),
                ("response_type", "code"),
                ("redirect_uri", claude::REDIRECT),
                ("scope", claude::SCOPE),
                ("code_challenge", &p.challenge),
                ("code_challenge_method", "S256"),
                ("state", state),
            ],
        ),
        Provider::Antigravity => (
            antigravity::AUTH_URL,
            vec![
                ("access_type", "offline"),
                ("client_id", antigravity::CLIENT_ID),
                ("prompt", "consent"),
                ("redirect_uri", antigravity::REDIRECT),
                ("response_type", "code"),
                ("scope", antigravity::SCOPES),
                ("state", state),
            ],
        ),
        _ => (
            codex::AUTH_URL,
            vec![
                ("client_id", codex::CLIENT_ID),
                ("response_type", "code"),
                ("redirect_uri", codex::REDIRECT),
                ("scope", "openid email profile offline_access"),
                ("state", state),
                ("code_challenge", &p.challenge),
                ("code_challenge_method", "S256"),
                ("prompt", "login"),
                ("id_token_add_organizations", "true"),
                ("codex_cli_simplified_flow", "true"),
            ],
        ),
    };
    let q = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(params).finish();
    format!("{base}?{q}")
}

fn claude_headers(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    rb.header("Accept", "application/json, text/plain, */*")
        .header("Content-Type", "application/json")
        .header("User-Agent", "axios/1.15.2")
}

/// Decodes the payload of a JWT without verifying it.
pub fn jwt_claims(token: &str) -> Value {
    token
        .split('.')
        .nth(1)
        .and_then(|p| URL_SAFE_NO_PAD.decode(p.trim_end_matches('=')).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null)
}

/// Only known OAuth protocol codes may enter logs, account status or errors.
/// Provider descriptions and arbitrary "error" strings can contain credentials.
pub(crate) fn error_classification(value: &Value) -> Option<&'static str> {
    let code = value["error"].as_str().or_else(|| value["error"]["code"].as_str())?;
    Some(match code {
        "invalid_request" => "invalid_request",
        "invalid_client" => "invalid_client",
        "invalid_grant" => "invalid_grant",
        "unauthorized_client" => "unauthorized_client",
        "unsupported_grant_type" => "unsupported_grant_type",
        "invalid_scope" => "invalid_scope",
        "access_denied" => "access_denied",
        "expired_token" => "expired_token",
        "authorization_pending" => "authorization_pending",
        "slow_down" => "slow_down",
        "server_error" => "server_error",
        "temporarily_unavailable" => "temporarily_unavailable",
        "refresh_token_expired" => "refresh_token_expired",
        "refresh_token_reused" => "refresh_token_reused",
        "refresh_token_invalidated" => "refresh_token_invalidated",
        _ => return None,
    })
}

/// Drop reqwest's URL/source chain: configurable OAuth URLs and failed response
/// parsing can contain secrets, even when a response body is never printed.
pub(crate) fn transport_error(error: reqwest::Error) -> anyhow::Error {
    safe_transport_error(&error)
}

fn safe_transport_error(error: &reqwest::Error) -> anyhow::Error {
    if let Some(status) = error.status() {
        anyhow!("OAuth request failed (HTTP {})", status.as_u16())
    } else if error.is_timeout() {
        anyhow!("OAuth request timed out")
    } else if error.is_connect() {
        anyhow!("OAuth connection failed")
    } else {
        anyhow!("OAuth request failed")
    }
}

#[derive(Debug, Clone, Copy)]
struct OAuthResponseFailure {
    status: reqwest::StatusCode,
    classification: Option<&'static str>,
}

impl std::fmt::Display for OAuthResponseFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OAuth endpoint returned HTTP {}", self.status.as_u16())?;
        if let Some(code) = self.classification {
            write!(f, " ({code})")?;
        }
        Ok(())
    }
}

impl std::error::Error for OAuthResponseFailure {}

/// Custom provider helpers may add unsafe context. Preserve only a typed safe
/// response failure or a reqwest classification, never their context chain.
fn sanitize_custom_error(error: anyhow::Error) -> anyhow::Error {
    if let Some(safe) = error.downcast_ref::<OAuthResponseFailure>() {
        anyhow::Error::new(*safe)
    } else if let Some(transport) = error.downcast_ref::<reqwest::Error>() {
        safe_transport_error(transport)
    } else {
        anyhow!("OAuth operation failed")
    }
}

pub(crate) async fn read_json(mut resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let mut bytes = Vec::new();
    // OAuth responses are small. Bound both error parsing and successful token
    // parsing without including serde errors or response excerpts in the chain.
    while let Some(chunk) = resp.chunk().await.map_err(transport_error)? {
        if bytes.len() + chunk.len() > 64 * 1024 {
            bail!("OAuth response too large (HTTP {})", status.as_u16());
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        return Err(anyhow::Error::new(OAuthResponseFailure { status, classification: error_classification(&value) }));
    }
    serde_json::from_slice(&bytes).map_err(|_| anyhow!("invalid OAuth response"))
}

fn expiry(v: &Value) -> Option<chrono::DateTime<Utc>> {
    v["expires_in"].as_i64().map(|s| Utc::now() + chrono::Duration::seconds(s))
}

/// Exchanges an authorization code. Returns the stored credential and file name.
pub async fn exchange(
    app: &App,
    provider: Provider,
    code: &str,
    state: &str,
    verifier: &str,
) -> Result<(OAuth, String, Vec<(&'static str, Value)>)> {
    let http = app.http.control(None);
    match provider {
        Provider::Claude => {
            // The hosted callback page shows `code#state`.
            let (code, code_state) = code.split_once('#').unwrap_or((code, state));
            let body = json!({
                "grant_type": "authorization_code",
                "code": code,
                "redirect_uri": claude::REDIRECT,
                "client_id": claude::CLIENT_ID,
                "code_verifier": verifier,
                "state": code_state,
            });
            let v = read_json(
                claude_headers(http.post(claude::TOKEN_URL)).json(&body).send().await.map_err(transport_error)?,
            )
            .await?;
            let mut o = OAuth {
                access_token: v["access_token"].as_str().unwrap_or_default().to_string(),
                refresh_token: v["refresh_token"].as_str().unwrap_or_default().to_string(),
                expires_at: expiry(&v),
                email: v["account"]["email_address"].as_str().map(String::from),
                account_id: v["account"]["uuid"].as_str().map(String::from),
                ..Default::default()
            };
            if o.email.is_none()
                && let Ok(p) = claude_profile(&http, &o.access_token).await
            {
                o.email = p["account"]["email"].as_str().map(String::from);
                o.account_id = o.account_id.or_else(|| p["account"]["uuid"].as_str().map(String::from));
            }
            let name = format!("claude-{}.json", file_safe(o.email.as_deref().unwrap_or("account")));
            let device: String = (0..32).map(|_| format!("{:02x}", rand::random::<u8>())).collect();
            Ok((o, name, vec![("claude_device_ids", json!([device]))]))
        }
        Provider::Antigravity => {
            let form = [
                ("code", code),
                ("client_id", antigravity::CLIENT_ID),
                ("client_secret", antigravity::CLIENT_SECRET),
                ("redirect_uri", antigravity::REDIRECT),
                ("grant_type", "authorization_code"),
            ];
            let v =
                read_json(http.post(antigravity::TOKEN_URL).form(&form).send().await.map_err(transport_error)?).await?;
            let access = v["access_token"].as_str().unwrap_or_default().to_string();
            let info = read_json(
                http.get(antigravity::USERINFO_URL)
                    .bearer_auth(&access)
                    .header("user-agent", antigravity::user_agent())
                    .send()
                    .await
                    .map_err(transport_error)?,
            )
            .await
            .unwrap_or_default();
            let email = info["email"].as_str().map(String::from);
            let project = match antigravity::fetch_project(app, &http, antigravity::BASE_PROD, &access).await {
                Ok(p) => Some(p),
                Err(e) => {
                    let _ = e;
                    tracing::warn!("antigravity: could not resolve the Cloud project yet");
                    None
                }
            };
            let o = OAuth {
                access_token: access,
                refresh_token: v["refresh_token"].as_str().unwrap_or_default().to_string(),
                expires_at: expiry(&v),
                email,
                project_id: project,
                ..Default::default()
            };
            let name = match &o.email {
                Some(e) => format!("antigravity-{}.json", file_safe(e)),
                None => "antigravity.json".into(),
            };
            let extra =
                vec![("expires_in", v["expires_in"].clone()), ("timestamp", Utc::now().timestamp_millis().into())];
            Ok((o, name, extra))
        }
        _ => {
            let form = [
                ("grant_type", "authorization_code"),
                ("client_id", codex::CLIENT_ID),
                ("code", code),
                ("redirect_uri", codex::REDIRECT),
                ("code_verifier", verifier),
            ];
            let v = read_json(
                http.post(codex::TOKEN_URL)
                    .header("Accept", "application/json")
                    .form(&form)
                    .send()
                    .await
                    .map_err(transport_error)?,
            )
            .await?;
            let id_token = v["id_token"].as_str().unwrap_or_default().to_string();
            let claims = jwt_claims(&id_token);
            let auth = &claims["https://api.openai.com/auth"];
            let o = OAuth {
                access_token: v["access_token"].as_str().unwrap_or_default().to_string(),
                refresh_token: v["refresh_token"].as_str().unwrap_or_default().to_string(),
                expires_at: expiry(&v),
                email: claims["email"].as_str().map(String::from),
                account_id: auth["chatgpt_account_id"].as_str().map(String::from),
                ..Default::default()
            };
            let plan = auth["chatgpt_plan_type"].as_str().unwrap_or("free").to_string();
            let name =
                format!("codex-{}-{}.json", file_safe(o.email.as_deref().unwrap_or("account")), file_safe(&plan));
            Ok((o, name, vec![("id_token", id_token.into()), ("plan_type", plan.into())]))
        }
    }
}

/// The signed-in account and its organization, which says the subscription plan.
pub(crate) async fn claude_profile(http: &reqwest::Client, token: &str) -> Result<Value> {
    let resp =
        claude_headers(http.get(claude::PROFILE_URL)).bearer_auth(token).send().await.map_err(transport_error)?;
    read_json(resp).await
}

/// The plan a Claude profile names, written the way ChatGPT names its plans: `max_20x`,
/// `max_5x`, `pro`, `team`, `enterprise`. The multiple comes from the organization's rate
/// limit tier (`default_claude_max_20x`); without one, Max is just `max`.
pub fn claude_plan(profile: &Value) -> Option<String> {
    let org = &profile["organization"];
    let account = &profile["account"];
    let kind = match org["organization_type"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
        Some(kind) => kind.to_ascii_lowercase(),
        None if account["has_claude_max"] == true => "claude_max".into(),
        None if account["has_claude_pro"] == true => "claude_pro".into(),
        None => return None,
    };
    let base = kind.strip_prefix("claude_").unwrap_or(&kind);
    let multiple = org["rate_limit_tier"]
        .as_str()
        .and_then(|tier| tier.rsplit('_').next())
        .and_then(|last| last.strip_suffix('x'))
        .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let plan = match multiple {
        Some(n) => format!("{base}_{n}x"),
        None => base.to_string(),
    };
    Some(plan.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect())
}

/// The plan's name as Claude shows it ("Ultra", "Max"). Names change while the tier
/// identifiers stay (Max 20x became Ultra as `default_claude_max_20x`), so this is only
/// for display.
pub fn claude_plan_name(profile: &Value) -> Option<String> {
    let name = profile["organization"]["plan_display_name"].as_str()?.trim();
    (!name.is_empty() && name.len() <= 40 && !name.chars().any(char::is_control)).then(|| name.to_string())
}

pub fn file_safe(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || "@._-".contains(c) { c } else { '_' }).collect()
}

/// Refreshes an account's access token if it expires within `margin`.
pub async fn ensure_fresh(app: &App, acct: &Arc<Account>, margin: chrono::Duration, force: bool) -> Result<()> {
    // A same-file replacement invalidates the old allocation. It must not
    // refresh and write old credentials over the newly selected account.
    let identity = acct.cred.read().identity(acct.provider);
    if identity != acct.state.lock().credential_identity {
        app.reload_accounts();
        bail!("account identity changed; reload accounts");
    }
    retry_pending_save(acct, false);
    let identity = acct.cred.read().identity(acct.provider);
    if identity != acct.state.lock().credential_identity {
        app.reload_accounts();
        bail!("account identity changed; reload accounts");
    }
    let needs = |acct: &Account| match &*acct.cred.read() {
        Credential::OAuth(o) => {
            force || o.access_token.is_empty() || o.expires_at.is_some_and(|t| t - Utc::now() < margin)
        }
        _ => false,
    };
    if !needs(acct) {
        return Ok(());
    }
    let token_before = match &*acct.cred.read() {
        Credential::OAuth(o) => o.access_token.clone(),
        _ => return Ok(()),
    };
    let guard = acct.refresh_lock.clone().lock_owned().await;
    let identity = acct.cred.read().identity(acct.provider);
    if identity != acct.state.lock().credential_identity {
        bail!("account identity changed; reload accounts");
    }
    let old = match &*acct.cred.read() {
        Credential::OAuth(o) => o.clone(),
        _ => return Ok(()),
    };
    // Another task refreshed while we waited for the lock.
    if old.access_token != token_before || !needs(acct) {
        return Ok(());
    }
    let self_minted = matches!(acct.provider, Provider::Vertex | Provider::Meta);
    if old.refresh_token.is_empty() && !self_minted {
        bail!("no refresh token");
    }
    let http = app.http.control_for_account(acct);
    let refreshing = acct.clone();
    let registration = app.refresh_tasks.register()?;
    // Once submitted, a refresh may rotate the provider's token even if the
    // waiting request is cancelled. Keep the bounded refresh + publication alive
    // independently; the account lock still coalesces concurrent callers.
    let result = tokio::spawn(async move {
        let _registration = registration;
        let _guard = guard;
        let result = refresh_locked(&refreshing, http, old).await;
        if let Err(error) = &result {
            tracing::warn!(account = %refreshing.label, "token refresh failed: {error:#}");
            refreshing.state.lock().set_error(format!("refresh failed: {error}"));
        }
        result
    })
    .await
    .context("token refresh task failed")?;
    let identity = acct.cred.read().identity(acct.provider);
    if identity != acct.state.lock().credential_identity {
        app.reload_accounts();
        bail!("account identity changed; reload accounts");
    }
    result
}

async fn refresh_locked(acct: &Arc<Account>, http: reqwest::Client, old: OAuth) -> Result<()> {
    let mut extra: Vec<(&str, Value)> = vec![];
    let keep = |access: String, v: &Value| OAuth {
        access_token: access,
        refresh_token: v["refresh_token"].as_str().map(String::from).unwrap_or(old.refresh_token.clone()),
        expires_at: expiry(v),
        ..old.clone()
    };
    let new = match acct.provider {
        Provider::Claude => {
            let body = json!({
                "client_id": claude::CLIENT_ID,
                "grant_type": "refresh_token",
                "refresh_token": old.refresh_token,
                "scope": claude::SCOPE,
            });
            let v = read_json(
                claude_headers(http.post(claude::TOKEN_URL)).json(&body).send().await.map_err(transport_error)?,
            )
            .await?;
            OAuth {
                email: v["account"]["email_address"].as_str().map(String::from).or(old.email.clone()),
                account_id: v["account"]["uuid"].as_str().map(String::from).or(old.account_id.clone()),
                ..keep(access_of(&v)?, &v)
            }
        }
        Provider::Codex => {
            let form = [
                ("client_id", codex::CLIENT_ID),
                ("grant_type", "refresh_token"),
                ("refresh_token", old.refresh_token.as_str()),
                ("scope", "openid profile email"),
            ];
            let v = read_json(
                http.post(codex::TOKEN_URL)
                    .header("Accept", "application/json")
                    .form(&form)
                    .send()
                    .await
                    .map_err(transport_error)?,
            )
            .await?;
            let id_token = v["id_token"].as_str().map(String::from);
            let claims = id_token.as_deref().map(jwt_claims).unwrap_or(Value::Null);
            if let Some(t) = id_token {
                extra.push(("id_token", t.into()));
            }
            OAuth {
                email: claims["email"].as_str().map(String::from).or(old.email.clone()),
                account_id: codex_refresh_account_id(&old, &claims),
                ..keep(access_of(&v)?, &v)
            }
        }
        Provider::Antigravity => {
            let form = [
                ("client_id", antigravity::CLIENT_ID),
                ("client_secret", antigravity::CLIENT_SECRET),
                ("grant_type", "refresh_token"),
                ("refresh_token", old.refresh_token.as_str()),
            ];
            let v =
                read_json(http.post(antigravity::TOKEN_URL).form(&form).send().await.map_err(transport_error)?).await?;
            extra.push(("expires_in", v["expires_in"].clone()));
            extra.push(("timestamp", Utc::now().timestamp_millis().into()));
            keep(access_of(&v)?, &v)
        }
        Provider::Kimi => {
            let device_id = old.field("device_id").map(String::from).unwrap_or_else(|| acct.device_id.clone());
            let form = [
                ("client_id", device::kimi::CLIENT_ID),
                ("grant_type", "refresh_token"),
                ("refresh_token", old.refresh_token.as_str()),
            ];
            let mut rb = http.post(device::kimi::TOKEN_URL).header("accept", "application/json").form(&form);
            for (k, v) in device::kimi_headers(&device_id) {
                rb = rb.header(k, v);
            }
            let v = read_json(rb.send().await.map_err(transport_error)?).await?;
            keep(access_of(&v)?, &v)
        }
        Provider::Xai => {
            let endpoint = match old.field("token_endpoint") {
                Some(e) => e.to_string(),
                None => device::xai_token_endpoint(&http).await.map_err(sanitize_custom_error)?,
            };
            let form = [
                ("grant_type", "refresh_token"),
                ("client_id", device::xai::CLIENT_ID),
                ("refresh_token", old.refresh_token.as_str()),
            ];
            let v = read_json(
                http.post(&endpoint)
                    .header("accept", "application/json")
                    .form(&form)
                    .send()
                    .await
                    .map_err(transport_error)?,
            )
            .await?;
            if let Some(t) = v["id_token"].as_str() {
                extra.push(("id_token", t.into()));
            }
            keep(access_of(&v)?, &v)
        }
        Provider::Meta => {
            let dca =
                old.field("dca_token").ok_or_else(|| anyhow!("no device token to mint a new key; sign in again"))?;
            let minted = device::meta_mint(&http, dca).await.map_err(sanitize_custom_error)?;
            OAuth {
                access_token: minted["api_key"].as_str().unwrap_or_default().to_string(),
                base_url: minted["base_url"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .or(old.base_url.clone()),
                expires_at: None,
                ..old.clone()
            }
        }
        Provider::Vertex => {
            let sa = old.raw.get("service_account").cloned().ok_or_else(|| anyhow!("no service account"))?;
            let (token, expires) = crate::vertex::mint(&http, &sa).await.map_err(sanitize_custom_error)?;
            OAuth { access_token: token, expires_at: Some(expires), ..old.clone() }
        }
        _ => return Ok(()),
    };
    if !publish_refreshed(acct, new, extra) {
        bail!("account identity changed; refresh result discarded");
    }
    tracing::info!(account = %acct.label, "refreshed {} token", acct.provider.as_str());
    Ok(())
}

/// A refresh updates tokens, never the user's explicitly selected workspace.
fn codex_refresh_account_id(old: &OAuth, claims: &Value) -> Option<String> {
    old.account_id
        .clone()
        .or_else(|| claims["https://api.openai.com/auth"]["chatgpt_account_id"].as_str().map(String::from))
}

/// Token rotation is already committed by the provider. Keep its result usable
/// even if the local filesystem is temporarily unable to save it.
pub(crate) fn publish_refreshed(acct: &Account, mut credential: OAuth, extra: Vec<(&str, Value)>) -> bool {
    for (key, value) in &extra {
        credential.raw.insert((*key).into(), value.clone());
    }
    {
        let mut pending = acct.pending_oauth_save.lock();
        let mut fields = pending.as_ref().map(|saved| saved.extra.clone()).unwrap_or_default();
        for (key, value) in extra {
            fields.retain(|(existing, _)| existing != key);
            fields.push((key.into(), value));
        }
        let refreshed = Credential::OAuth(credential.clone());
        let identity = refreshed.identity(acct.provider);
        let mut state = acct.state.lock();
        let mut cred = acct.cred.write();
        if cred.identity(acct.provider) != state.credential_identity {
            *pending = None;
            return false;
        }
        let expected_identity = pending
            .as_ref()
            .map(|save| save.expected_identity.clone())
            .unwrap_or_else(|| state.credential_identity.clone());
        *cred = refreshed;
        state.update_identity(identity);
        if acct.path.is_some() {
            *pending = Some(PendingOAuthSave {
                credential,
                expected_identity,
                extra: fields,
                retry_at: std::time::Instant::now(),
            });
        }
    }
    retry_pending_save(acct, true);
    let identity = acct.cred.read().identity(acct.provider);
    identity == acct.state.lock().credential_identity
}

fn retry_pending_save(acct: &Account, force: bool) {
    let mut pending = acct.pending_oauth_save.lock();
    let (Some(saved), Some(path)) = (pending.as_ref(), acct.path.as_ref()) else { return };
    if !force && saved.retry_at > std::time::Instant::now() {
        return;
    }
    if let Some((provider, disk, _, _)) = crate::accounts::read_oauth_file(path) {
        let disk_identity = Credential::OAuth(disk).identity(provider);
        if disk_identity != saved.expected_identity
            && disk_identity != Credential::OAuth(saved.credential.clone()).identity(acct.provider)
        {
            // An external replacement can happen before the directory watcher
            // runs. Discard the old save and invalidate its in-memory allocation.
            *pending = None;
            let mut state = acct.state.lock();
            state.update_identity(disk_identity);
            state.set_error("account identity changed; refresh result discarded");
            return;
        }
    }
    let extra: Vec<_> = saved.extra.iter().map(|(key, value)| (key.as_str(), value.clone())).collect();
    match write_oauth_file(path, acct.provider, &saved.credential, &extra) {
        Ok(()) => {
            *pending = None;
            let mut state = acct.state.lock();
            if state
                .last_error(Utc::now())
                .is_some_and(|message| message.starts_with("saving refreshed credentials failed:"))
            {
                state.clear_error();
            }
        }
        Err(error) => {
            tracing::warn!(account = %acct.label, "refreshed credentials remain in memory; saving will be retried: {error}");
            acct.state.lock().set_error(format!("saving refreshed credentials failed: {error}"));
            pending.as_mut().unwrap().retry_at = std::time::Instant::now() + Duration::from_secs(30);
        }
    }
}

fn access_of(v: &Value) -> Result<String> {
    Ok(v["access_token"].as_str().ok_or_else(|| anyhow!("missing access_token"))?.to_string())
}

/// Fresh credentials plus any per-provider setup (Antigravity's Cloud project).
pub async fn ensure_ready(app: &App, acct: &Arc<Account>) -> Result<()> {
    ensure_fresh(app, acct, chrono::Duration::minutes(5), false).await?;
    if acct.provider == Provider::Antigravity && acct.is_oauth() {
        antigravity::ensure_ready(app, acct).await.map_err(sanitize_custom_error)?;
    }
    Ok(())
}

/// Periodically refreshes tokens that are about to expire.
pub async fn refresher(app: Arc<App>) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        stream::iter(app.pool.all())
            .for_each_concurrent(4, |acct| {
                let app = &app;
                async move {
                    retry_pending_save(&acct, true);
                    if !acct.is_oauth() || acct.state.lock().disabled {
                        return;
                    }
                    if let Err(e) = ensure_fresh(app, &acct, chrono::Duration::minutes(10), false).await {
                        tracing::warn!(account = %acct.label, "token refresh failed: {e:#}");
                        acct.state.lock().set_error(format!("refresh failed: {e}"));
                    }
                }
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    struct Directory(std::path::PathBuf);

    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("fusebox-oauth-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn codex_refresh_preserves_selected_workspace_before_claims_fallback() {
        let claims = json!({"https://api.openai.com/auth": {"chatgpt_account_id": "claims-default-workspace"}});
        let selected = OAuth { account_id: Some("selected-workspace".into()), ..Default::default() };
        assert_eq!(codex_refresh_account_id(&selected, &claims).as_deref(), Some("selected-workspace"));
        assert_eq!(codex_refresh_account_id(&OAuth::default(), &claims).as_deref(), Some("claims-default-workspace"));
        assert_eq!(codex_refresh_account_id(&OAuth::default(), &Value::Null), None);
    }

    #[test]
    fn publishing_rotation_updates_identity_only_when_the_owner_changes() {
        let directory = Directory::new();
        std::fs::write(
            directory.0.join("account.json"),
            json!({
                "type": "codex", "access_token": "FAKE_OLD_ACCESS", "account_id": "selected-workspace",
            })
            .to_string(),
        )
        .unwrap();
        let cfg = Config { auth_dir: directory.0.to_string_lossy().into(), ..Default::default() };
        let app = App::new(cfg, directory.0.join("config.yaml"));
        let account = app.pool.all().pop().unwrap();
        let epoch = account.state.lock().identity_epoch;
        let identity = account.state.lock().credential_identity.clone();
        let Credential::OAuth(mut credential) = account.cred.read().clone() else { unreachable!() };
        credential.access_token = "FAKE_ROTATED_ACCESS".into();
        credential.refresh_token = "FAKE_ROTATED_REFRESH".into();
        credential.email = Some("updated@example.test".into());
        publish_refreshed(&account, credential.clone(), vec![]);
        assert_eq!(account.state.lock().identity_epoch, epoch);
        assert_eq!(account.state.lock().credential_identity, identity);
        credential.account_id = Some("replacement-workspace".into());
        publish_refreshed(&account, credential.clone(), vec![]);
        assert_eq!(account.state.lock().identity_epoch, epoch + 1);
        assert_eq!(account.state.lock().credential_identity, Credential::OAuth(credential).identity(Provider::Codex));
        assert_ne!(account.state.lock().credential_identity, identity);
        app.reload_accounts();
        assert_eq!(
            app.pool.all()[0].state.lock().identity_epoch,
            epoch + 1,
            "reload must not count the same change again"
        );
    }

    #[tokio::test]
    async fn refresh_errors_and_account_status_exclude_response_and_redirect_secrets() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let cases = [
            (400, json!({"error":"invalid_grant", "error_description":"FAKE_SECRET_MARKER", "refresh_token":"FAKE_SECRET_MARKER"}).to_string(), "HTTP 400 (invalid_grant)"),
            (401, json!({"error":"FAKE_SECRET_MARKER", "error_description":"FAKE_SECRET_MARKER"}).to_string(), "HTTP 401"),
            (503, "FAKE_SECRET_MARKER".into(), "HTTP 503"),
            (307, "FAKE_SECRET_MARKER".into(), "HTTP 307"),
            (200, "{\"FAKE_SECRET_MARKER\": broken FAKE_SECRET_MARKER}".into(), "invalid OAuth response"),
        ];
        for (status, body, expected) in cases {
            let directory = Directory::new();
            let redirected = Arc::new(AtomicUsize::new(0));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
            let router = axum::Router::new()
                .route(
                    "/token",
                    axum::routing::post(move |form: String| {
                        let body = body.clone();
                        async move {
                            assert!(form.contains("refresh_token=FAKE_REFRESH_REQUEST_MARKER"));
                            (
                                axum::http::StatusCode::from_u16(status).unwrap(),
                                [("location", "/target?secret=FAKE_SECRET_MARKER")],
                                body,
                            )
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
            let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            std::fs::write(directory.0.join("account.json"), json!({
                "type":"xai", "access_token":"FAKE_ACCESS_REQUEST_MARKER", "refresh_token":"FAKE_REFRESH_REQUEST_MARKER",
                "expired": (Utc::now() - chrono::Duration::minutes(1)).to_rfc3339(), "token_endpoint":endpoint,
            }).to_string()).unwrap();
            let cfg = Config {
                auth_dir: directory.0.to_string_lossy().into(),
                proxy_url: "direct".into(),
                ..Default::default()
            };
            let app = App::new(cfg, directory.0.join("config.yaml"));
            let account = app.pool.all().pop().unwrap();
            let error = ensure_fresh(&app, &account, chrono::Duration::minutes(5), false).await.unwrap_err();
            assert!(error.to_string().contains(expected), "{error:#}");
            for message in [format!("{error:#}"), format!("{error:?}"), account.snapshot().to_string()] {
                assert!(!message.contains("FAKE_SECRET_MARKER"), "unsafe OAuth failure: {message}");
                assert!(!message.contains("FAKE_REFRESH_REQUEST_MARKER"));
            }
            assert_eq!(redirected.load(Ordering::Relaxed), 0);
            server.abort();
        }
    }

    #[test]
    fn oauth_classifications_are_allowlisted_and_transport_urls_are_discarded() {
        assert_eq!(
            error_classification(&json!({"error":"invalid_grant", "error_description":"FAKE_SECRET_MARKER"})),
            Some("invalid_grant")
        );
        assert_eq!(
            error_classification(&json!({"error":{"code":"access_denied", "message":"FAKE_SECRET_MARKER"}})),
            Some("access_denied")
        );
        for value in [
            json!({"error":"FAKE_SECRET_MARKER"}),
            json!({"error":"invalid_grant\nFAKE_SECRET_MARKER"}),
            json!({"error":"x".repeat(10000)}),
        ] {
            assert_eq!(error_classification(&value), None);
        }
        let error = reqwest::Client::new().get("http://[FAKE_SECRET_MARKER").build().unwrap_err();
        let error = transport_error(error);
        assert!(!format!("{error:#}").contains("FAKE_SECRET_MARKER"));
        assert!(!format!("{error:?}").contains("FAKE_SECRET_MARKER"));
    }

    #[test]
    fn custom_refresh_failures_strip_context_but_preserve_safe_status_and_classification() {
        let unsafe_error = anyhow!("Meta provider response: FAKE_SECRET_MARKER").context("FAKE_CONTEXT_MARKER");
        let sanitized = sanitize_custom_error(unsafe_error);
        assert_eq!(sanitized.to_string(), "OAuth operation failed");
        let safe = anyhow::Error::new(OAuthResponseFailure {
            status: reqwest::StatusCode::BAD_REQUEST,
            classification: Some("invalid_grant"),
        })
        .context("FAKE_SECRET_MARKER");
        let sanitized = sanitize_custom_error(safe);
        assert_eq!(sanitized.to_string(), "OAuth endpoint returned HTTP 400 (invalid_grant)");
        assert!(!format!("{sanitized:#}").contains("FAKE_SECRET_MARKER"));
        assert_eq!(sanitized.chain().count(), 1);
    }

    #[tokio::test]
    async fn rotated_credentials_survive_failed_save_reload_and_retry_without_refresh() {
        let directory = Directory::new();
        let path = directory.0.join("account.json");
        std::fs::write(
            &path,
            json!({
                "type":"codex", "access_token":"old-access", "refresh_token":"old-refresh",
                "expired": (Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
                "custom_field":"keep-me", "id_token":"old-id"
            })
            .to_string(),
        )
        .unwrap();
        let mut cfg = Config {
            auth_dir: directory.0.to_string_lossy().into(),
            proxy_url: "http://127.0.0.1:1".into(),
            ..Default::default()
        };
        let app = App::new(cfg.clone(), directory.0.join("config.yaml"));
        let original = app.pool.all().pop().unwrap();
        let Credential::OAuth(old) = original.cred.read().clone() else { panic!("OAuth account") };
        // Deterministic save failure, even when tests run as root.
        let obstacle = path.with_extension("json.tmp");
        std::fs::create_dir(&obstacle).unwrap();
        publish_refreshed(
            &original,
            OAuth { access_token: "new-access".into(), refresh_token: "new-refresh".into(), ..old },
            vec![("id_token", json!("new-id"))],
        );
        assert!(original.pending_oauth_save.lock().is_some());
        // Provider setup can update metadata while a rotated token still needs
        // saving; neither update may discard the other's pending fields.
        let Credential::OAuth(mut updated) = original.cred.read().clone() else { panic!("OAuth account") };
        updated.project_id = Some("discovered-project".into());
        publish_refreshed(&original, updated, vec![]);
        app.pool.reload(&cfg);
        assert!(Arc::ptr_eq(&original, &app.pool.all()[0]));
        // A metadata change replaces the Account allocation, but must preserve
        // both rotated credentials and the lock shared with in-flight work.
        cfg.oauth_excluded_models.insert("codex".into(), vec!["unused-model".into()]);
        app.pool.reload(&cfg);
        let reloaded = app.pool.all().pop().unwrap();
        assert!(!Arc::ptr_eq(&original, &reloaded));
        assert!(Arc::ptr_eq(&original.cred, &reloaded.cred));
        assert!(Arc::ptr_eq(&original.refresh_lock, &reloaded.refresh_lock));
        let Credential::OAuth(current) = reloaded.cred.read().clone() else { panic!("OAuth account") };
        assert_eq!(current.refresh_token, "new-refresh");
        std::fs::remove_dir(&obstacle).unwrap();
        // Retry the save as the periodic refresher does after a disk failure.
        retry_pending_save(&reloaded, true);
        // The access token remains fresh: this only retries persistence. A
        // refresh attempt would contact a real provider and fail this test.
        ensure_fresh(&app, &reloaded, chrono::Duration::minutes(5), false).await.unwrap();
        assert!(reloaded.pending_oauth_save.lock().is_none());
        let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["access_token"], "new-access");
        assert_eq!(saved["refresh_token"], "new-refresh");
        assert_eq!(saved["id_token"], "new-id");
        assert_eq!(saved["project_id"], "discovered-project");
        assert_eq!(saved["custom_field"], "keep-me");
    }

    #[tokio::test]
    async fn reload_during_refresh_preserves_the_shared_token_and_refresh_lock() {
        let directory = Directory::new();
        let path = directory.0.join("account.json");
        std::fs::write(&path, r#"{"type":"codex","access_token":"disk-old"}"#).unwrap();
        let mut cfg = Config { auth_dir: directory.0.to_string_lossy().into(), ..Default::default() };
        let app = App::new(cfg.clone(), directory.0.join("config.yaml"));
        let original = app.pool.all().pop().unwrap();
        let guard = original.refresh_lock.lock().await;
        *original.cred.write() = Credential::OAuth(OAuth { access_token: "memory-new".into(), ..Default::default() });
        cfg.oauth_excluded_models.insert("codex".into(), vec!["unused-model".into()]);
        app.pool.reload(&cfg);
        let reloaded = app.pool.all().pop().unwrap();
        assert!(reloaded.refresh_lock.try_lock().is_err());
        assert!(Arc::ptr_eq(&original.state, &reloaded.state));
        let Credential::OAuth(current) = reloaded.cred.read().clone() else { panic!("OAuth account") };
        assert_eq!(current.access_token, "memory-new");
        drop(guard);
        assert!(reloaded.refresh_lock.try_lock().is_ok());
        let epoch = reloaded.state.lock().identity_epoch;
        let Credential::OAuth(mut changed) = original.cred.read().clone() else { unreachable!() };
        changed.account_id = Some("newly-discovered-workspace".into());
        publish_refreshed(&original, changed, vec![]);
        assert_eq!(reloaded.state.lock().identity_epoch, epoch + 1);
        assert_eq!(reloaded.state.lock().credential_identity, reloaded.cred.read().identity(Provider::Codex));
    }

    #[tokio::test]
    async fn account_replacement_wins_over_inflight_refresh_and_pending_save() {
        for provider in ["claude", "codex"] {
            for pending_save in [false, true] {
                let directory = Directory::new();
                let path = directory.0.join("account.json");
                let initial = json!({"type":provider,"account_id":"workspace-A","access_token":"FAKE_A","claude_device_ids":["a".repeat(64)]});
                std::fs::write(&path, initial.to_string()).unwrap();
                let cfg = Config { auth_dir: directory.0.to_string_lossy().into(), ..Default::default() };
                let app = App::new(cfg, directory.0.join("config.yaml"));
                let original = app.pool.all().pop().unwrap();
                let Credential::OAuth(mut rotated) = original.cred.read().clone() else { unreachable!() };
                rotated.access_token = "FAKE_ROTATED_A".into();
                let obstacle = path.with_extension("json.tmp");
                if pending_save {
                    std::fs::create_dir(&obstacle).unwrap();
                    assert!(publish_refreshed(&original, rotated.clone(), vec![]));
                    assert!(original.pending_oauth_save.lock().is_some());
                    std::fs::remove_dir(&obstacle).unwrap();
                }
                let guard = original.refresh_lock.lock().await;
                std::fs::write(&path, json!({"type":provider,"account_id":"workspace-B","access_token":"FAKE_B","claude_device_ids":["a".repeat(64)]}).to_string()).unwrap();
                app.reload_accounts();
                let replacement = app.pool.all().pop().unwrap();
                assert!(!Arc::ptr_eq(&original, &replacement));
                assert_ne!(original.device_id, replacement.device_id);
                assert!(replacement.refresh_lock.try_lock().is_ok());
                assert!(!publish_refreshed(&original, rotated, vec![]));
                retry_pending_save(&original, true);
                let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                assert_eq!(saved["account_id"], "workspace-B");
                assert_eq!(saved["access_token"], "FAKE_B");
                drop(guard);
                assert!(ensure_fresh(&app, &original, chrono::Duration::minutes(5), false).await.is_err());
                assert!(Arc::ptr_eq(&replacement, &app.pool.all()[0]));
            }
        }
    }

    #[tokio::test]
    async fn replacement_before_watcher_discards_refresh_publication_or_pending_retry() {
        for pending_save in [false, true] {
            let directory = Directory::new();
            let path = directory.0.join("account.json");
            std::fs::write(
                &path,
                json!({"type":"codex","account_id":"workspace-A","access_token":"FAKE_A"}).to_string(),
            )
            .unwrap();
            let cfg = Config { auth_dir: directory.0.to_string_lossy().into(), ..Default::default() };
            let app = App::new(cfg, directory.0.join("config.yaml"));
            let original = app.pool.all().pop().unwrap();
            let Credential::OAuth(mut rotated) = original.cred.read().clone() else { unreachable!() };
            rotated.access_token = "FAKE_ROTATED_A".into();
            let obstacle = path.with_extension("json.tmp");
            if pending_save {
                std::fs::create_dir(&obstacle).unwrap();
                assert!(publish_refreshed(&original, rotated.clone(), vec![]));
                std::fs::remove_dir(&obstacle).unwrap();
            }
            std::fs::write(
                &path,
                json!({"type":"codex","account_id":"workspace-B","access_token":"FAKE_B"}).to_string(),
            )
            .unwrap();
            if pending_save {
                retry_pending_save(&original, true);
            } else {
                assert!(!publish_refreshed(&original, rotated, vec![]));
            }
            assert!(original.pending_oauth_save.lock().is_none());
            assert!(ensure_fresh(&app, &original, chrono::Duration::minutes(5), false).await.is_err());
            let replacement = app.pool.all().pop().unwrap();
            assert!(!Arc::ptr_eq(&original, &replacement));
            let Credential::OAuth(current) = replacement.cred.read().clone() else { unreachable!() };
            assert_eq!(current.account_id.as_deref(), Some("workspace-B"));
            let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(saved["access_token"], "FAKE_B");
        }
    }

    #[tokio::test]
    async fn cancelling_the_request_does_not_discard_a_provider_token_rotation() {
        let directory = Directory::new();
        let accepted = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
        let router = axum::Router::new().route("/token", axum::routing::post({
            let accepted = accepted.clone();
            let release = release.clone();
            move || {
                let (accepted, release) = (accepted.clone(), release.clone());
                async move {
                    accepted.notify_one();
                    release.notified().await;
                    axum::Json(json!({"access_token":"rotated-access","refresh_token":"rotated-refresh","expires_in":3600}))
                }
            }
        }));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let path = directory.0.join("account.json");
        std::fs::write(
            &path,
            json!({
                "type":"xai", "access_token":"old-access", "refresh_token":"old-refresh",
                "expired": (Utc::now() - chrono::Duration::minutes(1)).to_rfc3339(), "token_endpoint":endpoint,
            })
            .to_string(),
        )
        .unwrap();
        let cfg =
            Config { auth_dir: directory.0.to_string_lossy().into(), proxy_url: "direct".into(), ..Default::default() };
        let app = App::new(cfg, directory.0.join("config.yaml"));
        let account = app.pool.all().pop().unwrap();
        let request = tokio::spawn({
            let (app, account) = (app.clone(), account.clone());
            async move { ensure_fresh(&app, &account, chrono::Duration::minutes(5), false).await }
        });
        tokio::time::timeout(Duration::from_secs(2), accepted.notified()).await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        let mut shutdown = tokio::spawn({
            let refresh_tasks = app.refresh_tasks.clone();
            async move { refresh_tasks.shutdown().await }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !app.refresh_tasks.inner.lifecycle.lock().closing {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(app.refresh_tasks.register().is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut shutdown).await.is_err(),
            "shutdown must wait for the accepted token rotation"
        );
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(2), shutdown).await.unwrap().unwrap();
        let Credential::OAuth(current) = account.cred.read().clone() else { panic!("OAuth account") };
        assert_eq!(current.refresh_token, "rotated-refresh");
        let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["access_token"], "rotated-access");
        assert_eq!(saved["refresh_token"], "rotated-refresh");
        let error = ensure_fresh(&app, &account, chrono::Duration::minutes(5), true).await.unwrap_err();
        assert!(error.to_string().contains("shutting down"));
        server.abort();
    }

    #[tokio::test]
    async fn shutdown_waits_for_every_registered_refresh_and_is_idempotent() {
        let tasks = RefreshTasks::default();
        let first = tasks.register().unwrap();
        let last = tasks.register().unwrap();
        let mut shutdown = tokio::spawn({
            let tasks = tasks.clone();
            async move { tasks.shutdown().await }
        });
        assert!(tokio::time::timeout(Duration::from_millis(30), &mut shutdown).await.is_err());
        assert!(tasks.register().is_err());
        drop(first);
        assert!(tokio::time::timeout(Duration::from_millis(30), &mut shutdown).await.is_err());
        drop(last);
        tokio::time::timeout(Duration::from_secs(1), shutdown).await.unwrap().unwrap();
        tasks.shutdown().await;
        assert!(tasks.register().is_err());
    }

    #[test]
    fn claude_profiles_name_their_plan() {
        let profile = |org: Value, account: Value| json!({ "organization": org, "account": account });
        let max =
            |tier: &str| profile(json!({ "organization_type": "claude_max", "rate_limit_tier": tier }), json!({}));
        assert_eq!(claude_plan(&max("default_claude_max_20x")).as_deref(), Some("max_20x"));
        assert_eq!(claude_plan(&max("default_claude_max_5x")).as_deref(), Some("max_5x"));
        assert_eq!(claude_plan(&max("default_claude_ai")).as_deref(), Some("max"), "a tier without a multiple");
        assert_eq!(claude_plan(&max("")).as_deref(), Some("max"));
        let pro = profile(json!({ "organization_type": "claude_pro", "rate_limit_tier": null }), json!({}));
        assert_eq!(claude_plan(&pro).as_deref(), Some("pro"));
        let team = profile(json!({ "organization_type": "claude_team", "seat_tier": "premium" }), json!({}));
        assert_eq!(claude_plan(&team).as_deref(), Some("team"));
        let odd = profile(json!({ "organization_type": "Claude Enterprise" }), json!({}));
        assert_eq!(claude_plan(&odd).as_deref(), Some("claude_enterprise"), "only plain names lose the prefix");
        // Older profiles only flag the account.
        let flagged = profile(json!({}), json!({ "has_claude_max": true, "has_claude_pro": false }));
        assert_eq!(claude_plan(&flagged).as_deref(), Some("max"));
        let flagged = profile(json!(null), json!({ "has_claude_max": false, "has_claude_pro": true }));
        assert_eq!(claude_plan(&flagged).as_deref(), Some("pro"));
        assert_eq!(claude_plan(&profile(json!({}), json!({ "has_claude_max": false }))), None);
        assert_eq!(claude_plan(&json!({})), None);
        // Production on 2026-10-31: Max 20x renamed Ultra, its tier unchanged.
        let ultra = profile(
            json!({ "organization_type": "claude_max", "rate_limit_tier": "default_claude_max_20x",
                "plan_display_name": "Ultra", "plan_display_label": "Ultra plan" }),
            json!({ "has_claude_max": true }),
        );
        assert_eq!(claude_plan(&ultra).as_deref(), Some("max_20x"), "the identifier stays");
        assert_eq!(claude_plan_name(&ultra).as_deref(), Some("Ultra"));
        assert_eq!(claude_plan_name(&max("default_claude_max_5x")), None, "older profiles have no name");
        for odd in [json!(""), json!("  "), json!(5), json!("a\nb"), json!("x".repeat(41))] {
            assert_eq!(claude_plan_name(&profile(json!({ "plan_display_name": odd }), json!({}))), None);
        }
    }
}
