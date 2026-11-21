//! Antigravity: Google's agent IDE, served by the private Cloud Code
//! (`v1internal`) API. Requests are Gemini `generateContent` bodies wrapped in
//! an envelope with the account's Cloud project; responses wrap each Gemini
//! chunk in `{"response": ...}`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::accounts::{Account, Credential};
use crate::schema;
use crate::state::App;

mod release;

pub const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
pub const CLIENT_SECRET: &str = "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";
pub const PORT: u16 = 51121;
pub const REDIRECT: &str = "http://localhost:51121/oauth-callback";
pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo?alt=json";
pub const SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs";

/// Inference goes to the daily channel, account setup to prod (as the IDE does).
pub const BASE_DAILY: &str = "https://daily-cloudcode-pa.googleapis.com";
pub const BASE_PROD: &str = "https://cloudcode-pa.googleapis.com";

pub(crate) const FALLBACK_VERSION: &str = "2.33.0";
/// The build changelist (`cl=`) each reviewed release stamps into its language
/// server (`changelist N with baseline M`). Fusebox reads a newer release's number
/// from the release itself (see `release`); this table is what it knows without that.
const CHANGELISTS: &[(&str, &str)] = &[("2.33.0", "1023160026")];
/// How long a release whose build number could not be read waits for another try.
const RETRY_UNREAD: Duration = Duration::from_secs(24 * 3600);
const MANIFEST_URL: &str =
    "https://antigravity-hub-auto-updater-974169037036.us-central1.run.app/manifest/latest-arm64-mac.yml";
const NODE_CLIENT: &str = "google-api-nodejs-client/10.3.0";
const GOOG_API_CLIENT: &str = "gl-node/22.21.1";

static VERSION: RwLock<String> = RwLock::new(String::new());
/// A release missing from `CHANGELISTS` and the build number read from it.
static LEARNED: RwLock<Option<(String, String)>> = RwLock::new(None);
/// Deprecated model id -> its replacement, from `fetchAvailableModels`
/// (`gemini-3.1-pro-high` -> `gemini-pro-agent`). Old ids still list but fail.
static RENAMED: RwLock<Vec<(String, String)>> = RwLock::new(Vec::new());

/// The id to send upstream for a requested Antigravity model.
pub fn current_id(model: &str) -> String {
    RENAMED
        .read()
        .iter()
        .find(|(old, _)| old.eq_ignore_ascii_case(model))
        .map(|(_, new)| new.clone())
        .unwrap_or_else(|| model.to_string())
}

/// The IDE release Fusebox claims (see `identity`).
pub fn version() -> String {
    identity().0
}

/// The language server's form: `antigravity/hub/<release> (aidev_client; os_type=darwin;
/// arch=arm64; cl=<changelist>)`.
pub fn user_agent() -> String {
    let (version, changelist) = identity();
    let changelist = changelist.map(|cl| format!("; cl={cl}")).unwrap_or_default();
    format!("antigravity/hub/{version} (aidev_client; os_type=darwin; arch=arm64{changelist})")
}

/// The release from Google's update manifest (or the built-in fallback), unless
/// `client-identities` names a newer one, with its build number.
fn identity() -> (String, Option<String>) {
    let fetched = {
        let v = VERSION.read();
        if v.is_empty() { FALLBACK_VERSION.to_string() } else { v.clone() }
    };
    let wanted = crate::clients::newest(crate::clients::ANTIGRAVITY, fetched.clone());
    choose(&fetched, &wanted, LEARNED.read().as_ref())
}

/// A release goes out with its own build number, never another's and never none: until
/// Fusebox knows a new release's number, it stays on the newest release whose number it
/// knows, as an IDE that hasn't updated yet would. Only a release set in
/// `client-identities` that Fusebox has no number for goes without: the operator's choice.
fn choose(fetched: &str, wanted: &str, learned: Option<&(String, String)>) -> (String, Option<String>) {
    let known = || CHANGELISTS.iter().map(|(v, cl)| (*v, *cl)).chain(learned.map(|(v, cl)| (v.as_str(), cl.as_str())));
    if let Some((_, cl)) = known().find(|(v, _)| *v == wanted) {
        return (wanted.to_string(), Some(cl.to_string()));
    }
    if wanted != fetched {
        return (wanted.to_string(), None);
    }
    known()
        .max_by_key(|(v, _)| crate::clients::parse(v))
        .map(|(v, cl)| (v.to_string(), Some(cl.to_string())))
        .unwrap_or_else(|| (wanted.to_string(), None))
}

fn knows_build(version: &str) -> bool {
    CHANGELISTS.iter().any(|(v, _)| *v == version) || LEARNED.read().as_ref().is_some_and(|(v, _)| v == version)
}

/// Keeps the advertised IDE release current (Cloud Code refuses newer models to older
/// clients), reading a new release's build number from the release itself.
pub async fn version_updater(app: Arc<App>) {
    // A release whose build number could not be read, and when.
    let mut unread: Option<(String, Instant)> = None;
    loop {
        if app.pool.all().iter().any(|a| a.provider == crate::accounts::Provider::Antigravity) {
            match fetch_manifest(&app).await {
                Ok((version, url)) => {
                    *VERSION.write() = version.clone();
                    let due = unread.as_ref().is_none_or(|(v, at)| *v != version || at.elapsed() >= RETRY_UNREAD);
                    if !knows_build(&version) && due {
                        let read = match &url {
                            Some(url) => release::read_build(&app.http.client(None), url).await,
                            None => Err(anyhow!("the update manifest names no release in Google's storage")),
                        };
                        match read {
                            Ok(cl) => {
                                tracing::info!("antigravity {version}: build {cl}, read from Google's release");
                                *LEARNED.write() = Some((version, cl));
                                unread = None;
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "antigravity {version}: build number not readable ({e:#}); claiming {} until it is",
                                    self::version()
                                );
                                unread = Some((version, Instant::now()));
                            }
                        }
                    }
                }
                Err(e) => tracing::debug!("antigravity version check failed: {e:#}"),
            }
        }
        tokio::time::sleep(Duration::from_secs(3 * 3600)).await;
    }
}

/// The manifest's release and the URL of its macOS ZIP.
async fn fetch_manifest(app: &App) -> Result<(String, Option<String>)> {
    let text = app.http.client(None).get(MANIFEST_URL).timeout(Duration::from_secs(10)).send().await?.text().await?;
    let version = text
        .lines()
        .find_map(|l| l.strip_prefix("version:"))
        .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
        .filter(|v| crate::clients::parse(v).is_some())
        .ok_or_else(|| anyhow!("no version in manifest"))?;
    let url = release::release_url(&text, &version);
    Ok((version, url))
}

fn base(acct: &Account, default: &str) -> String {
    match &*acct.cred.read() {
        Credential::OAuth(o) => o.base_url.clone().unwrap_or_else(|| default.to_string()),
        _ => default.to_string(),
    }
    .trim_end_matches('/')
    .to_string()
}

pub fn request_base(acct: &Account) -> String {
    base(acct, BASE_DAILY)
}

fn token(acct: &Account) -> String {
    match &*acct.cred.read() {
        Credential::OAuth(o) => o.access_token.clone(),
        Credential::ApiKey { key, .. } => key.clone(),
    }
}

pub fn project(acct: &Account) -> Option<String> {
    match &*acct.cred.read() {
        Credential::OAuth(o) => o.project_id.clone(),
        _ => None,
    }
}

fn project_of(v: &Value) -> Option<String> {
    for k in ["cloudaicompanionProject", "projectId", "project"] {
        match &v[k] {
            Value::String(s) if !s.trim().is_empty() => return Some(s.trim().to_string()),
            Value::Object(o) => {
                if let Some(id) = o.get("id").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
                    return Some(id.trim().to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn default_tier(v: &Value) -> String {
    v["allowedTiers"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["isDefault"] == true)
        .and_then(|t| t["id"].as_str())
        .or_else(|| v["currentTier"]["id"].as_str())
        .unwrap_or("free-tier")
        .to_string()
}

/// Why Google keeps an Antigravity account from serving, until its owner acts: a
/// suspension for a terms-of-service violation (`TOS_VIOLATION`), or a check that the
/// account is a real person's (`VALIDATION_REQUIRED`). Google answers both with a 403
/// that names a page to appeal or verify at; the account's token and model list keep
/// working, so only this reason tells the account apart from a working one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hold {
    pub kind: HoldKind,
    /// Google's own words.
    pub message: String,
    /// Where the owner appeals or verifies: an `https` page on a Google domain, shown and
    /// never opened by Fusebox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_text: Option<String>,
    pub since: DateTime<Utc>,
    /// The account error that tells of it; dismissing that error ends the hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldKind {
    /// Suspended for a terms-of-service violation: out until the owner says otherwise.
    Suspended,
    /// To be verified: back once `loadCodeAssist` no longer asks for it.
    Verify,
}

/// How long a verification hold waits between free `loadCodeAssist` checks.
pub const VERIFY_RECHECK: chrono::Duration = chrono::Duration::minutes(10);

impl Hold {
    /// The account error: what happened, and where to go.
    pub fn error_text(&self) -> String {
        let what = match self.kind {
            HoldKind::Suspended => "Google suspended this account for a terms-of-service violation",
            HoldKind::Verify => "Google asks you to verify this account",
        };
        let link = self
            .link
            .as_deref()
            .map(|l| format!(" {}: {l}", self.link_text.as_deref().unwrap_or("Open")))
            .unwrap_or_default();
        format!("{what}: {}.{link}", self.message.trim_end_matches('.'))
    }
}

/// A link Google gave, if it is safe to show: `https` on google.com or one of its
/// subdomains, without a user name, password or port (the dashboard checks the same).
fn safe_link(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url.trim()).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    (parsed.scheme() == "https"
        && (host == "google.com" || host.ends_with(".google.com"))
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.port().is_none())
    .then(|| parsed.to_string())
}

/// The hold a 403 from Cloud Code reports, if it is one.
pub fn hold_of(body: &str) -> Option<Hold> {
    let v: Value = serde_json::from_str(body).ok()?;
    let error = &v["error"];
    let details = error["details"].as_array()?;
    let info = details.iter().find(|d| d["@type"].as_str().is_some_and(|t| t.ends_with("google.rpc.ErrorInfo")))?;
    let meta = &info["metadata"];
    let text = |v: &Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let kind = match info["reason"].as_str()? {
        "TOS_VIOLATION" => HoldKind::Suspended,
        "VALIDATION_REQUIRED" => HoldKind::Verify,
        _ => return None,
    };
    // A Help detail's first link comes first, as in Google's Gemini CLI.
    let help = details
        .iter()
        .find(|d| d["@type"].as_str().is_some_and(|t| t.ends_with("google.rpc.Help")))
        .and_then(|h| h["links"].get(0));
    // The first one safe to show; a Help link elsewhere leaves the metadata's.
    let safe = |link: Option<String>, link_text: Option<String>| Some((safe_link(&link?)?, link_text));
    let (link, link_text) = match kind {
        HoldKind::Suspended => safe(text(&meta["appeal_url"]), text(&meta["appeal_url_link_text"])),
        HoldKind::Verify => safe(help.and_then(|l| text(&l["url"])), help.and_then(|l| text(&l["description"])))
            .or_else(|| safe(text(&meta["validation_url"]), text(&meta["validation_url_link_text"])))
            .or_else(|| safe(text(&meta["validation_link"]), text(&meta["validation_url_link_text"])))
            .or_else(|| safe(text(&meta["validation_learn_more_url"]), None)),
    }
    .map_or((None, None), |(link, text)| (Some(link), text));
    Some(Hold {
        kind,
        message: text(&error["message"]).unwrap_or_else(|| "Google refused this account".into()),
        link_text: link.as_ref().and(link_text),
        link,
        since: Utc::now(),
        error_id: None,
    })
}

/// What `loadCodeAssist` says beyond the plan: the Google AI credit balance and whether
/// Google still wants the account verified.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Standing {
    pub plan: Option<String>,
    pub plan_name: Option<String>,
    /// Google AI credits (`GOOGLE_ONE_AI`): the balance, and the least a request needs.
    pub credits: Option<(f64, Option<f64>)>,
    /// Still to be verified, with Google's link.
    pub verify: Option<Hold>,
}

/// The `loadCodeAssist` answer's credits and verification request (see `Standing`).
pub fn standing(v: &Value) -> Standing {
    let (plan, plan_name) = tier(v);
    let number = |v: &Value| v.as_f64().or_else(|| v.as_str()?.trim().parse().ok()).filter(|n: &f64| n.is_finite());
    let credits = v["paidTier"]["availableCredits"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["creditType"].as_str().is_some_and(|t| t.eq_ignore_ascii_case("GOOGLE_ONE_AI")))
        // A zero balance is left out of Google's JSON, as protobuf leaves out every zero.
        .map(|c| (number(&c["creditAmount"]).unwrap_or(0.0), number(&c["minimumCreditAmountForUsage"])));
    let text = |v: &Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let verify =
        v["ineligibleTiers"].as_array().into_iter().flatten().find(|t| t["reasonCode"] == "VALIDATION_REQUIRED").map(
            |t| {
                let link = text(&t["validationUrl"]).as_deref().and_then(safe_link);
                Hold {
                    kind: HoldKind::Verify,
                    message: text(&t["validationErrorMessage"])
                        .or_else(|| text(&t["reasonMessage"]))
                        .unwrap_or_else(|| "Verify your account to continue".into()),
                    link_text: link.as_ref().and(text(&t["validationUrlLinkText"])),
                    link,
                    since: Utc::now(),
                    error_id: None,
                }
            },
        );
    Standing { plan, plan_name, credits, verify }
}

/// The account's Google AI plan from a `loadCodeAssist` answer: the identifier and
/// Google's name for it. A subscription is `paidTier` (`g1-ultra-tier`, "Google AI
/// Ultra"); without one `currentTier` names only the free tier, and its name is the
/// product's ("Antigravity"), so it gives no plan name.
pub fn tier(v: &Value) -> (Option<String>, Option<String>) {
    let text = |t: &Value| t.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let paid = &v["paidTier"];
    match text(&paid["id"]) {
        Some(id) => (Some(id), text(&paid["name"])),
        None => (text(&v["currentTier"]["id"]), None),
    }
}

/// Reads the account's plan, credits and verification request (see `Standing`) without
/// using model tokens.
pub async fn load_standing(http: &reqwest::Client, acct: &Account, token: &str) -> Result<Standing> {
    let resp = http
        .post(format!("{}/v1internal:loadCodeAssist", base(acct, BASE_PROD)))
        .bearer_auth(token)
        .header("user-agent", user_agent())
        .header("accept", "*/*")
        .json(&json!({ "metadata": { "ideType": "ANTIGRAVITY" } }))
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?;
    let v: Value = resp.json().await?;
    let found = standing(&v);
    if found.plan.is_none() {
        bail!("loadCodeAssist names no tier");
    }
    Ok(found)
}

/// Finds (or provisions) the Cloud project behind an Antigravity account.
pub async fn fetch_project(app: &App, http: &reqwest::Client, base: &str, token: &str) -> Result<String> {
    let resp = http
        .post(format!("{base}/v1internal:loadCodeAssist"))
        .bearer_auth(token)
        .header("user-agent", user_agent())
        .header("accept", "*/*")
        .json(&json!({ "metadata": { "ideType": "ANTIGRAVITY" } }))
        .send()
        .await
        .map_err(crate::oauth::transport_error)?;
    let load = crate::oauth::read_json(resp).await?;
    if let Some(p) = project_of(&load) {
        return Ok(p);
    }
    let tier = default_tier(&load);
    tracing::info!("antigravity: onboarding account on tier {tier}");
    let body = json!({
        "tier_id": tier,
        "metadata": { "ide_type": "ANTIGRAVITY", "ide_version": version(), "ide_name": "antigravity" },
    });
    for _ in 0..5 {
        let resp = http
            .post(format!("{BASE_DAILY}/v1internal:onboardUser"))
            .bearer_auth(token)
            .header("user-agent", format!("{} {NODE_CLIENT}", user_agent()))
            .header("x-goog-api-client", GOOG_API_CLIENT)
            .json(&body)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(crate::oauth::transport_error)?;
        let v = crate::oauth::read_json(resp).await?;
        if v["done"] == true {
            return project_of(&v["response"]).ok_or_else(|| anyhow!("onboarding finished without a project id"));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let _ = app;
    bail!("onboarding did not complete")
}

/// Makes sure the account knows its project (and models) before first use.
pub async fn ensure_ready(app: &App, acct: &Arc<Account>) -> Result<()> {
    if project(acct).is_none() {
        let _guard = acct.refresh_lock.lock().await;
        if project(acct).is_none() {
            let http = app.http.control_for_account(acct);
            let id = fetch_project(app, &http, &base(acct, BASE_PROD), &token(acct)).await?;
            let snapshot = {
                let mut cred = acct.cred.write();
                let Credential::OAuth(o) = &mut *cred else { return Ok(()) };
                o.project_id = Some(id);
                o.clone()
            };
            app.suppress_reload();
            if !crate::oauth::publish_refreshed(acct, snapshot, vec![]) {
                bail!("account identity changed; project update discarded");
            }
        }
    }
    Ok(())
}

/// The models Google offers this account (`fetchAvailableModels`, asked for its project
/// as the IDE and CLIProxyAPI do), with their display names. `discovery` asks every six
/// hours; an empty answer is an error, so a failed check never leaves an account serving
/// nothing.
pub async fn fetch_models(acct: &Account, http: &reqwest::Client) -> Result<Vec<crate::discovery::Listed>> {
    let body = match project(acct) {
        Some(p) => json!({ "project": p }),
        None => json!({}),
    };
    let resp = http
        .post(format!("{}/v1internal:fetchAvailableModels", request_base(acct)))
        .bearer_auth(token(acct))
        .header("user-agent", user_agent())
        .json(&body)
        .timeout(Duration::from_secs(15))
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("fetchAvailableModels returned {}", resp.status());
    }
    let v: Value = resp.json().await?;
    let out = models_of(&v);
    if out.is_empty() {
        bail!("fetchAvailableModels listed no models");
    }
    if let Some(Value::Object(dep)) = v.get("deprecatedModelIds") {
        let renamed: Vec<(String, String)> = dep
            .iter()
            .filter_map(|(old, info)| Some((old.clone(), info["newModelId"].as_str()?.to_string())))
            .collect();
        *RENAMED.write() = renamed;
    }
    Ok(out)
}

/// The chat models in a `fetchAvailableModels` answer, sorted by ID.
fn models_of(v: &Value) -> Vec<crate::discovery::Listed> {
    let listed = |id: &str, name: Option<&str>| crate::discovery::Listed {
        id: id.to_string(),
        name: name.map(String::from),
        ..Default::default()
    };
    let mut out: Vec<crate::discovery::Listed> = match &v["models"] {
        // Internal helpers (tab completion, tiered routers) have no display name.
        Value::Object(m) => m
            .iter()
            .filter_map(|(id, info)| Some(listed(id, Some(info.get("displayName")?.as_str().unwrap_or(id)))))
            .collect(),
        Value::Array(a) => a
            .iter()
            .filter_map(|x| Some(listed(x["id"].as_str().or(x["name"].as_str())?, x["displayName"].as_str())))
            .collect(),
        _ => vec![],
    };
    out.retain(|m| {
        crate::discovery::valid_model_id(&m.id)
            && !m.id.starts_with("chat_")
            && !m.id.starts_with("tab_")
            && !m.id.starts_with("gemini-2.5")
    });
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Stable per-conversation session id derived from the first user message.
fn session_id(req: &Value) -> String {
    let first = req["contents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["role"] == "user")
        .find_map(|c| c["parts"][0]["text"].as_str().filter(|t| !t.is_empty()));
    let n = match first {
        Some(t) => {
            let h = Sha256::digest(t.as_bytes());
            i64::from_be_bytes(h[..8].try_into().unwrap()) & i64::MAX
        }
        None => rand::random::<i64>() & i64::MAX,
    };
    format!("-{n}")
}

/// Cloud Code turns a response schema into a forced tool, which Opus and Sonnet 5.5
/// reject. They get the schema as an instruction instead, so the JSON is not guaranteed.
fn schema_as_instruction(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    ["opus-5-5", "sonnet-5-5"].iter().any(|name| {
        m.match_indices(name).any(|(at, _)| !m[at + name.len()..].starts_with(|c: char| c.is_ascii_digit()))
    })
}

/// Moves the response schema into the system instruction and removes the native
/// structured-output fields. Returns whether there was a schema to move.
fn move_schema_to_instruction(req: &mut Value) -> bool {
    const CONFIGS: [&str; 2] = ["generationConfig", "generation_config"];
    const SCHEMAS: [&str; 4] = ["responseJsonSchema", "responseSchema", "response_json_schema", "response_schema"];
    let Some(schema) =
        CONFIGS.iter().find_map(|c| SCHEMAS.iter().find_map(|k| req.get(c)?.get(k).filter(|s| s.is_object()).cloned()))
    else {
        return false;
    };
    for c in CONFIGS {
        if let Some(gc) = req.get_mut(c).and_then(Value::as_object_mut) {
            for k in SCHEMAS.iter().chain(&["responseMimeType", "response_mime_type"]) {
                gc.remove(*k);
            }
        }
    }
    let text = format!(
        "You must format your entire response as valid JSON that conforms strictly to the following JSON schema:\n{schema}"
    );
    let key = if req.get("systemInstruction").is_none() && req.get("system_instruction").is_some() {
        "system_instruction"
    } else {
        "systemInstruction"
    };
    let Some(instruction) = req.as_object_mut().map(|o| o.entry(key).or_insert_with(|| json!({}))) else {
        return false;
    };
    if let Value::String(earlier) = instruction {
        *instruction = json!({ "parts": [{ "text": std::mem::take(earlier) }] });
    } else if !instruction.is_object() {
        *instruction = json!({});
    }
    let parts = instruction.as_object_mut().unwrap().entry("parts").or_insert_with(|| json!([]));
    if !parts.is_array() {
        *parts = json!([]);
    }
    parts.as_array_mut().unwrap().push(json!({ "text": text }));
    true
}

/// Wraps a Gemini request for the Cloud Code API.
pub fn envelope(mut req: Value, model: &str, project: Option<&str>) -> Value {
    let claude = model.contains("claude");
    let image = model.contains("image");
    if let Some(o) = req.as_object_mut() {
        o.remove("model");
        o.remove("safetySettings");
        o.remove("safety_settings");
    }
    let schema_in_instruction = schema_as_instruction(model) && move_schema_to_instruction(&mut req);
    // Function schemas: the backend only accepts its OpenAPI subset under `parameters`.
    let validated = claude || model.contains("gemini-3-pro") || model.contains("gemini-3.1-pro");
    for t in req.get_mut("tools").and_then(Value::as_array_mut).into_iter().flatten() {
        for key in ["functionDeclarations", "function_declarations"] {
            for f in t.get_mut(key).and_then(Value::as_array_mut).into_iter().flatten() {
                let Some(fo) = f.as_object_mut() else { continue };
                let schema = ["parametersJsonSchema", "parameters_json_schema", "parameters"]
                    .iter()
                    .find_map(|k| fo.remove(*k))
                    .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
                fo.insert("parameters".into(), schema::clean_tool(&schema, validated));
            }
        }
    }
    for key in ["generationConfig", "generation_config"] {
        let Some(gc) = req.get_mut(key).and_then(Value::as_object_mut) else { continue };
        for sk in ["responseSchema", "responseJsonSchema", "response_schema", "response_json_schema"] {
            if let Some(schema) = gc.get_mut(sk).filter(|v| v.is_object()) {
                *schema = schema::clean_response(schema);
            }
        }
        if !claude {
            gc.remove("maxOutputTokens");
        }
        // gpt-oss on Antigravity has no thinking controls.
        if model.starts_with("gpt-oss") {
            gc.remove("thinkingConfig");
        }
    }
    if claude && req["tools"].as_array().is_some_and(|t| !t.is_empty()) {
        let calling = &mut req["toolConfig"]["functionCallingConfig"];
        if schema_in_instruction {
            // Forced modes are rejected too; AUTO takes no allowed-function list.
            calling["mode"] = "AUTO".into();
            if let Some(c) = calling.as_object_mut() {
                c.remove("allowedFunctionNames");
                c.remove("allowed_function_names");
            }
        } else {
            calling["mode"] = "VALIDATED".into();
        }
    }
    let mut env = json!({
        "model": model,
        "userAgent": "antigravity",
        "requestType": if image { "image_gen" } else { "agent" },
    });
    if let Some(p) = project {
        env["project"] = p.into();
    }
    if image {
        env["requestId"] =
            format!("image_gen/{}/{}/12", chrono::Utc::now().timestamp_millis(), uuid::Uuid::new_v4()).into();
    } else {
        env["requestId"] = format!("agent-{}", uuid::Uuid::new_v4()).into();
        if req["sessionId"].as_str().is_none_or(str::is_empty) {
            req["sessionId"] = session_id(&req).into();
        }
    }
    env["request"] = req;
    env
}

/// Cloud Code wraps each Gemini chunk as `{"response": {...}}`.
pub fn unwrap(v: Value) -> Value {
    match v {
        Value::Object(mut o) if o.get("response").is_some_and(Value::is_object) && !o.contains_key("candidates") => {
            o.remove("response").unwrap_or_default()
        }
        Value::Array(a) => Value::Array(a.into_iter().map(unwrap).collect()),
        other => other,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn fallback_user_agent_matches_the_current_release() {
        assert_eq!(user_agent(), "antigravity/hub/2.33.0 (aidev_client; os_type=darwin; arch=arm64; cl=1023160026)");
        assert!(
            CHANGELISTS.iter().any(|(release, _)| *release == FALLBACK_VERSION),
            "the fallback needs its changelist"
        );
    }

    #[test]
    fn the_plan_is_the_paid_tier_or_else_the_free_one_without_a_name() {
        let pair = |id: Option<&str>, name: Option<&str>| (id.map(String::from), name.map(String::from));
        // As Google answered for an AI Ultra account on 2026-11-21.
        let ultra = json!({
            "currentTier": {"id": "free-tier", "name": "Antigravity"},
            "allowedTiers": [{"id": "free-tier", "name": "Antigravity", "isDefault": true}],
            "paidTier": {"id": "g1-ultra-tier", "name": "Google AI Ultra"}
        });
        assert_eq!(tier(&ultra), pair(Some("g1-ultra-tier"), Some("Google AI Ultra")));
        assert_eq!(
            tier(&json!({"currentTier": {"id": "free-tier", "name": "Antigravity"}})),
            pair(Some("free-tier"), None)
        );
        assert_eq!(tier(&json!({"paidTier": {"id": " "}, "currentTier": {}})), pair(None, None));
    }

    /// Google's 403 for a suspended account, as CLIProxyAPI #7296 quotes it.
    pub(crate) fn tos_violation() -> Value {
        json!({"error": {"code": 403, "status": "PERMISSION_DENIED",
            "message": "This service has been disabled in this account for violation of Terms of Service.",
            "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "TOS_VIOLATION",
                "domain": "cloudcode-pa.googleapis.com",
                "metadata": {"appeal_url": "https://support.google.com/accounts/contact/appeal?case=1",
                    "appeal_url_link_text": "Submit an appeal"}}]}})
    }

    #[test]
    fn google_says_when_it_holds_an_account_and_where_to_go() {
        let hold = hold_of(&tos_violation().to_string()).unwrap();
        assert_eq!(hold.kind, HoldKind::Suspended);
        assert_eq!(hold.link.as_deref(), Some("https://support.google.com/accounts/contact/appeal?case=1"));
        assert_eq!(hold.link_text.as_deref(), Some("Submit an appeal"));
        assert_eq!(
            hold.error_text(),
            "Google suspended this account for a terms-of-service violation: This service has been disabled in this \
             account for violation of Terms of Service. Submit an appeal: https://support.google.com/accounts/contact/appeal?case=1"
        );
        // As Google answered an unverified account's first request on 2026-11-21.
        let verify = json!({"error": {"code": 403, "message": "Verify your account to continue.", "status": "PERMISSION_DENIED",
            "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "VALIDATION_REQUIRED",
                "domain": "cloudcode-pa.googleapis.com",
                "metadata": {"validation_url_link_text": "Verify your account",
                    "validation_url": "https://accounts.google.com/v3/signin/continue?plt=x&flowName=GlifWebSignIn"}}]}});
        let hold = hold_of(&verify.to_string()).unwrap();
        assert_eq!(hold.kind, HoldKind::Verify);
        assert_eq!(hold.message, "Verify your account to continue.");
        assert_eq!(
            hold.link.as_deref(),
            Some("https://accounts.google.com/v3/signin/continue?plt=x&flowName=GlifWebSignIn")
        );
        assert_eq!(hold.link_text.as_deref(), Some("Verify your account"));
        // A link anywhere but Google is not shown; the hold stands.
        let mut elsewhere = tos_violation();
        elsewhere["error"]["details"][0]["metadata"]["appeal_url"] = "https://google.com.example.net/appeal".into();
        let hold = hold_of(&elsewhere.to_string()).unwrap();
        assert_eq!((hold.link, hold.link_text), (None, None));
        for odd in ["https://user@support.google.com/appeal", "https://support.google.com:8443/appeal"] {
            elsewhere["error"]["details"][0]["metadata"]["appeal_url"] = odd.into();
            assert_eq!(hold_of(&elsewhere.to_string()).unwrap().link, None, "{odd}");
        }
        // A Help link Fusebox can't show leaves the one in Google's metadata.
        let mut help = verify.clone();
        help["error"]["details"].as_array_mut().unwrap().push(json!({"@type": "type.googleapis.com/google.rpc.Help",
            "links": [{"description": "Short link", "url": "https://goo.gl/abc"}]}));
        let hold = hold_of(&help.to_string()).unwrap();
        assert_eq!(
            (hold.link.as_deref(), hold.link_text.as_deref()),
            (
                Some("https://accounts.google.com/v3/signin/continue?plt=x&flowName=GlifWebSignIn"),
                Some("Verify your account")
            )
        );
        for other in [
            json!({"error": {"code": 403, "status": "PERMISSION_DENIED", "message": "no",
                "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "SOMETHING_ELSE"}]}}),
            json!({"error": {"code": 403, "message": "Permission denied"}}),
        ] {
            assert_eq!(hold_of(&other.to_string()), None);
        }
        assert_eq!(hold_of("not json"), None);
    }

    #[test]
    fn load_code_assist_gives_the_plan_credits_and_any_verification_request() {
        // As Google answered before the owner verified (2026-11-21), with the link shortened.
        let before = json!({
            "currentTier": {"id": "standard-tier", "name": "Antigravity"},
            "ineligibleTiers": [{"reasonCode": "VALIDATION_REQUIRED", "tierId": "free-tier",
                "validationErrorMessage": "Verify your account to continue.",
                "validationUrlLinkText": "Verify your account",
                "validationUrl": "https://accounts.google.com/v3/signin/continue?plt=x"}],
            "paidTier": {"id": "g1-ultra-tier", "name": "Google AI Ultra",
                "availableCredits": [{"creditType": "GOOGLE_ONE_AI", "minimumCreditAmountForUsage": "50"}]}
        });
        let s = standing(&before);
        assert_eq!((s.plan.as_deref(), s.plan_name.as_deref()), (Some("g1-ultra-tier"), Some("Google AI Ultra")));
        assert_eq!(s.credits, Some((0.0, Some(50.0))), "Google leaves a zero balance out");
        let verify = s.verify.unwrap();
        assert_eq!(verify.link.as_deref(), Some("https://accounts.google.com/v3/signin/continue?plt=x"));
        let mut after = before.clone();
        after.as_object_mut().unwrap().remove("ineligibleTiers");
        after["paidTier"]["availableCredits"][0]["creditAmount"] = "1250".into();
        let s = standing(&after);
        assert_eq!(s.verify, None);
        assert_eq!(s.credits, Some((1250.0, Some(50.0))));
        assert_eq!(standing(&json!({"currentTier": {"id": "free-tier"}})).credits, None);
    }

    #[test]
    fn the_model_list_keeps_named_chat_models() {
        // Part of Google's answer on 2026-11-21.
        let v = json!({"models": {
            "gemini-4-neon-high": {"displayName": "Gemini 4 Neon (High)"},
            "claude-opus-5-5-medium": {"displayName": "Claude Opus 5.5 (Medium)"},
            "gemini-3.8-flash-tiered": {},
            "tab_flash_lite_preview": {},
            "gemini-2.5-pro": {"displayName": "Gemini 2.5 Pro"},
            "gemini-pro-orchestrator": {"displayName": "Orchestrator (Carbon)"}
        }});
        let listed: Vec<(String, Option<String>)> = models_of(&v).into_iter().map(|m| (m.id, m.name)).collect();
        assert_eq!(
            listed,
            [
                ("claude-opus-5-5-medium".to_string(), Some("Claude Opus 5.5 (Medium)".to_string())),
                ("gemini-4-neon-high".into(), Some("Gemini 4 Neon (High)".into())),
                ("gemini-pro-orchestrator".into(), Some("Orchestrator (Carbon)".into())),
            ]
        );
    }

    #[test]
    fn a_release_goes_out_only_with_its_own_build() {
        let pair = |v: &str, cl: Option<&str>| (v.to_string(), cl.map(String::from));
        assert_eq!(choose("2.33.0", "2.33.0", None), pair("2.33.0", Some("1023160026")));
        // A new release whose number isn't known yet: stay on the newest one that is.
        assert_eq!(choose("2.34.0", "2.34.0", None), pair("2.33.0", Some("1023160026")));
        let learned = ("2.34.0".to_string(), "1024000001".to_string());
        assert_eq!(choose("2.34.0", "2.34.0", Some(&learned)), pair("2.34.0", Some("1024000001")));
        // Learned for an older manifest, then the manifest moves on again.
        assert_eq!(choose("2.35.0", "2.35.0", Some(&learned)), pair("2.34.0", Some("1024000001")));
        // `client-identities` named it: the operator's choice, with its number if known.
        assert_eq!(choose("2.33.0", "2.34.0", Some(&learned)), pair("2.34.0", Some("1024000001")));
        assert_eq!(choose("2.33.0", "2.36.0", Some(&learned)), pair("2.36.0", None));
    }

    #[test]
    fn envelope_wraps_and_cleans() {
        let req = json!({
            "model": "x",
            "contents": [{ "role": "user", "parts": [{ "text": "hello" }] }],
            "safetySettings": [],
            "tools": [{ "functionDeclarations": [{ "name": "f", "parametersJsonSchema": { "type": "object", "properties": {}, "$schema": "x" } }] }],
            "generationConfig": { "maxOutputTokens": 100 }
        });
        let env = envelope(req.clone(), "claude-sonnet-4-6", Some("proj-1"));
        assert_eq!(env["project"], "proj-1");
        assert_eq!(env["requestType"], "agent");
        assert!(env["requestId"].as_str().unwrap().starts_with("agent-"));
        let r = &env["request"];
        assert!(r.get("model").is_none() && r.get("safetySettings").is_none());
        let decl = &r["tools"][0]["functionDeclarations"][0];
        assert!(decl.get("parametersJsonSchema").is_none());
        assert_eq!(decl["parameters"]["required"], json!(["reason"]));
        assert_eq!(r["toolConfig"]["functionCallingConfig"]["mode"], "VALIDATED");
        assert_eq!(r["generationConfig"]["maxOutputTokens"], 100);
        // Same first message, same session.
        assert_eq!(r["sessionId"], envelope(req.clone(), "claude-sonnet-4-6", None)["request"]["sessionId"]);
        let gem = envelope(req, "gemini-3.8-flash-high", None);
        assert!(gem["request"]["generationConfig"].get("maxOutputTokens").is_none());
        assert!(gem["request"].get("generation_config").is_none());
        assert!(gem["request"]["tools"][0].get("function_declarations").is_none());
    }

    #[test]
    fn opus_and_sonnet_5_5_get_the_response_schema_as_an_instruction() {
        let schema = json!({ "type": "object", "properties": { "title": { "type": "string", "minLength": 1 } },
            "required": ["title"], "additionalProperties": false });
        let req = json!({
            "contents": [{ "role": "user", "parts": [{ "text": "Name a book." }] }],
            "systemInstruction": { "parts": [{ "text": "Be brief." }] },
            "tools": [{ "functionDeclarations": [{ "name": "f", "parametersJsonSchema": { "type": "object", "properties": {} } }] }],
            "toolConfig": { "functionCallingConfig": { "mode": "ANY", "allowedFunctionNames": ["f"] } },
            "generationConfig": { "temperature": 0.2, "responseMimeType": "application/json", "responseJsonSchema": schema }
        });
        let instructed = |r: &Value, at: usize| {
            let text = r["systemInstruction"]["parts"][at]["text"].as_str().unwrap().to_string();
            serde_json::from_str::<Value>(&text[text.find('{').unwrap()..]).unwrap()
        };
        for model in ["claude-opus-5-5", "claude-sonnet-5-5", "claude-opus-5-5-max", "claude-sonnet-5-5-low"] {
            let r = &envelope(req.clone(), model, None)["request"];
            let gc = &r["generationConfig"];
            assert!(gc.get("responseJsonSchema").is_none() && gc.get("responseMimeType").is_none(), "{model}: {gc}");
            assert_eq!(gc["temperature"], 0.2);
            assert_eq!(r["systemInstruction"]["parts"][0]["text"], "Be brief.");
            assert_eq!(instructed(r, 1), schema, "{model}: the schema is sent as written");
            assert_eq!(r["toolConfig"]["functionCallingConfig"], json!({ "mode": "AUTO" }), "{model}");
        }
        for model in ["claude-sonnet-4-6", "claude-haiku-5-5", "claude-opus-5-50"] {
            let r = &envelope(req.clone(), model, None)["request"];
            assert!(r["generationConfig"]["responseJsonSchema"].is_object(), "{model}");
            assert_eq!(r["generationConfig"]["responseMimeType"], "application/json");
            assert_eq!(r["systemInstruction"]["parts"].as_array().unwrap().len(), 1);
            assert_eq!(r["toolConfig"]["functionCallingConfig"]["mode"], "VALIDATED");
        }
        // Without a schema, a 5.5 request keeps its instruction and VALIDATED tools.
        let mut plain = req.clone();
        plain["generationConfig"] = json!({ "temperature": 0.2 });
        let r = &envelope(plain, "claude-opus-5-5", None)["request"];
        assert_eq!(r["systemInstruction"]["parts"].as_array().unwrap().len(), 1);
        assert_eq!(r["toolConfig"]["functionCallingConfig"]["mode"], "VALIDATED");
        // snake_case fields, and no instruction yet.
        let snake = json!({ "contents": req["contents"],
            "generation_config": { "response_mime_type": "application/json", "response_schema": { "type": "OBJECT" } } });
        let r = &envelope(snake, "claude-sonnet-5-5", None)["request"];
        assert_eq!(r["generation_config"], json!({}));
        assert_eq!(instructed(r, 0), json!({ "type": "OBJECT" }));
    }

    #[test]
    fn unwrap_strips_response_envelope() {
        let v = json!({ "response": { "candidates": [] }, "traceId": "t" });
        assert_eq!(unwrap(v), json!({ "candidates": [] }));
        let plain = json!({ "candidates": [], "response": { "x": 1 } });
        assert_eq!(unwrap(plain.clone()), plain);
    }
}
