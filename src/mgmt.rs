//! Management API used by the dashboard, plus OAuth login orchestration.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use axum::Json;
use axum::Router;
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::{Credential, Only, Provider, set_file_disabled, write_oauth_file};
use crate::config::{Config, ModelAlias};
use crate::oauth;
use crate::state::App;

#[derive(Clone, Serialize)]
pub struct Login {
    pub provider: Provider,
    pub status: &'static str,
    pub message: Option<String>,
    pub url: String,
    pub callback: bool,
    /// "redirect" (browser OAuth) or "device" (enter `user_code` at `url`).
    pub kind: &'static str,
    pub user_code: Option<String>,
    #[serde(skip)]
    pub verifier: String,
    #[serde(skip)]
    pub created: Instant,
}

static CLAUDE_CB: AtomicBool = AtomicBool::new(false);
static CODEX_CB: AtomicBool = AtomicBool::new(false);
static ANTIGRAVITY_CB: AtomicBool = AtomicBool::new(false);

fn remember(app: &Arc<App>, state: &str, login: &Login) {
    let mut logins = app.logins.lock();
    logins.retain(|_, l| l.created.elapsed() < Duration::from_secs(1800));
    logins.insert(state.to_string(), login.clone());
}

fn settle(app: &Arc<App>, state: &str, result: &Result<String>) {
    if let Some(l) = app.logins.lock().get_mut(state) {
        match result {
            Ok(label) => {
                l.status = "done";
                l.message = Some(label.clone());
            }
            Err(e) => {
                l.status = "error";
                l.message = Some(format!("{e:#}"));
            }
        }
    }
    app.broadcast("login", json!({ "state": state }));
}

pub async fn start_login(app: &Arc<App>, provider: Provider) -> Result<(String, Login)> {
    let state = oauth::random_state();
    match provider {
        Provider::Claude | Provider::Codex | Provider::Antigravity => {
            let pkce = oauth::pkce();
            let url = oauth::auth_url(provider, &state, &pkce);
            let callback = ensure_callback_server(app, provider).await;
            let login = Login {
                provider,
                status: "pending",
                message: None,
                url,
                callback,
                kind: "redirect",
                user_code: None,
                verifier: pkce.verifier,
                created: Instant::now(),
            };
            remember(app, &state, &login);
            Ok((state, login))
        }
        Provider::Kimi | Provider::Xai | Provider::Meta => {
            let dev = crate::device::start(app, provider).await?;
            let login = Login {
                provider,
                status: "pending",
                message: None,
                url: dev.verification_uri.clone(),
                callback: true,
                kind: "device",
                user_code: Some(dev.user_code.clone()),
                verifier: String::new(),
                created: Instant::now(),
            };
            remember(app, &state, &login);
            let (app2, state2) = (app.clone(), state.clone());
            tokio::spawn(async move {
                let result = async {
                    let signed = crate::device::wait(&app2, provider, &dev).await?;
                    save_signed(&app2, provider, signed)
                }
                .await;
                settle(&app2, &state2, &result);
            });
            Ok((state, login))
        }
        Provider::Devin => {
            // Devin accepts any localhost redirect, so use a fresh port per login.
            let pkce = oauth::pkce();
            let (callback, redirect) = match tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
                Ok(listener) => {
                    let port = listener.local_addr()?.port();
                    serve_callback(app, listener, "/callback", provider, None);
                    (true, format!("http://127.0.0.1:{port}/callback"))
                }
                Err(_) => (false, String::new()),
            };
            let login = Login {
                provider,
                status: "pending",
                message: None,
                url: crate::devin::auth_url(&redirect, &state, &pkce.challenge),
                callback,
                kind: "redirect",
                user_code: None,
                verifier: pkce.verifier,
                created: Instant::now(),
            };
            remember(app, &state, &login);
            Ok((state, login))
        }
        Provider::Vertex => Err(anyhow!("Vertex uses a service account key: import the JSON instead")),
        Provider::Gemini | Provider::Compat => Err(anyhow!("{} uses API keys", provider.as_str())),
    }
}

fn save_signed(app: &Arc<App>, provider: Provider, s: crate::device::Signed) -> Result<String> {
    let path = app.cfg().auth_dir().join(&s.file);
    write_oauth_file(&path, provider, &s.oauth, &s.extra)?;
    app.reload_accounts();
    Ok(s.oauth.email.unwrap_or(s.file))
}

/// Listens on the fixed OAuth redirect port while logins are pending.
async fn ensure_callback_server(app: &Arc<App>, provider: Provider) -> bool {
    let (flag, port, path) = match provider {
        Provider::Claude => (&CLAUDE_CB, oauth::claude::PORT, "/callback"),
        Provider::Antigravity => (&ANTIGRAVITY_CB, crate::antigravity::PORT, "/oauth-callback"),
        _ => (&CODEX_CB, oauth::codex::PORT, "/auth/callback"),
    };
    if flag.load(Ordering::SeqCst) {
        return true;
    }
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(
                "cannot listen on localhost:{port} for the OAuth callback ({e}); paste the redirect URL instead"
            );
            return false;
        }
    };
    flag.store(true, Ordering::SeqCst);
    serve_callback(app, listener, path, provider, Some(flag));
    true
}

/// Serves the OAuth redirect on `listener` until no login for `provider` is pending.
fn serve_callback(
    app: &Arc<App>,
    listener: tokio::net::TcpListener,
    path: &'static str,
    provider: Provider,
    flag: Option<&'static AtomicBool>,
) {
    let router = Router::new().route(path, get(callback)).with_state(app.clone());
    let app2 = app.clone();
    tokio::spawn(async move {
        let shutdown = async move {
            // Stay up while a login for this provider is pending (max 15 minutes).
            let started = Instant::now();
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let pending = app2.logins.lock().values().any(|l| l.provider == provider && l.status == "pending");
                if !pending || started.elapsed() > Duration::from_secs(900) {
                    break;
                }
            }
        };
        let _ = axum::serve(listener, router).with_graceful_shutdown(shutdown).await;
        if let Some(f) = flag {
            f.store(false, Ordering::SeqCst);
        }
    });
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn callback(State(app): State<Arc<App>>, Query(q): Query<CallbackQuery>) -> Html<String> {
    let result = match (q.code, q.state, q.error) {
        (_, _, Some(e)) => Err(q.error_description.unwrap_or(e)),
        (Some(code), Some(state), _) => complete_login(&app, &state, &code).await.map_err(|e| format!("{e:#}")),
        _ => Err("missing code or state".to_string()),
    };
    let (title, body, dot) = match result {
        Ok(label) => (
            "Signed in",
            format!(
                "Connected <b style=\"color:#f2efe8;font-weight:500\">{}</b>. You can close this tab.",
                html_escape(&label)
            ),
            "#4ade80",
        ),
        Err(e) => ("Sign-in failed", html_escape(&e), "#fb7185"),
    };
    // Served from the OAuth callback port, so the page carries its own styles and no assets.
    Html(format!(
        r##"<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="color-scheme" content="dark"><meta name="theme-color" content="#0b0b0a"><title>{title} · Fusebox</title>
<body style="margin:0;min-height:100vh;display:grid;place-items:center;background:#0b0b0a;color:#f2efe8;font:14px/1.45 'IBM Plex Sans',system-ui,-apple-system,sans-serif">
<main style="box-sizing:border-box;width:min(420px,calc(100% - 32px));padding:22px 24px 24px;background:#121210;border:1px solid #24231f;border-radius:8px">
<div style="font:600 11px/1 'IBM Plex Sans Condensed','IBM Plex Sans',system-ui,sans-serif;letter-spacing:.12em;text-transform:uppercase;color:#857f74">Fusebox</div>
<h1 style="display:flex;align-items:center;gap:10px;font-size:17px;font-weight:600;margin:14px 0 6px"><span style="width:8px;height:8px;border-radius:50%;background:{dot}"></span>{title}</h1>
<p style="color:#b4afa4;margin:0;font-size:13px">{body}</p></main></body>"##
    ))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub async fn complete_login(app: &Arc<App>, state: &str, code: &str) -> Result<String> {
    let (provider, verifier) = {
        let logins = app.logins.lock();
        let l = logins.get(state).ok_or_else(|| anyhow!("unknown or expired login, start again"))?;
        if l.status != "pending" {
            return Err(anyhow!("this login was already completed"));
        }
        if l.kind == "device" {
            return Err(anyhow!("approve the code in your browser; there is nothing to paste"));
        }
        (l.provider, l.verifier.clone())
    };
    let result = async {
        if provider == Provider::Devin {
            let signed = crate::devin::complete_login(app, code.trim(), &verifier).await?;
            return save_signed(app, provider, signed);
        }
        let (cred, name, extra) = oauth::exchange(app, provider, code.trim(), state, &verifier).await?;
        let path = app.cfg().auth_dir().join(&name);
        write_oauth_file(&path, provider, &cred, &extra)?;
        app.reload_accounts();
        Ok::<_, anyhow::Error>(cred.email.unwrap_or(name))
    }
    .await;
    settle(app, state, &result);
    result
}

/// Accepts a pasted redirect URL, a `code#state` string or a bare code.
pub fn parse_pasted(input: &str) -> (String, Option<String>) {
    let input = input.trim();
    if let Some(q) = input.split_once('?').map(|(_, q)| q).filter(|q| q.contains("code=")) {
        let mut code = String::new();
        let mut state = None;
        for (k, v) in url::form_urlencoded::parse(q.split('#').next().unwrap_or(q).as_bytes()) {
            match k.as_ref() {
                "code" => code = v.into_owned(),
                "state" => state = Some(v.into_owned()),
                _ => {}
            }
        }
        return (code, state);
    }
    (input.to_string(), None)
}

// ---------------------------------------------------------------------- router

pub fn router(app: Arc<App>) -> Router<Arc<App>> {
    Router::new()
        .merge(crate::usage::api::router())
        .merge(crate::push::router())
        .route("/overview", get(overview))
        .route("/accounts", get(accounts))
        .route("/accounts/{id}", delete(delete_account))
        .route("/accounts/{id}/toggle", post(toggle_account))
        .route("/accounts/{id}/refresh", post(refresh_account))
        .route("/accounts/{id}/reset", post(reset_account))
        .route("/accounts/{id}/error/clear", post(clear_account_error))
        .route("/accounts/{id}/banked-resets", get(banked_resets).post(apply_banked_reset))
        .route("/accounts/{id}/quota/refresh", post(refresh_quota))
        .route("/accounts/{id}/window-starter", post(start_window))
        .route("/accounts/{id}/activity", get(account_activity))
        .route("/accounts/{id}/priority", post(set_priority))
        .route("/clients/usage", get(client_usage))
        .route("/client-identities", get(client_identities))
        .route("/client-identities/reset", post(reset_client_identities))
        .route("/playground", post(crate::server::playground))
        .route("/routes", get(routes))
        .route("/keys", post(add_key))
        .route("/vertex", post(import_vertex))
        .route("/requests", get(requests))
        .route("/faults", get(faults))
        .route("/models", get(models))
        .route("/models/added", post(add_model).delete(remove_model))
        .route("/models/check", post(check_models))
        .route("/config", get(get_config).put(put_config))
        .route("/config/settings", get(get_settings).patch(patch_settings))
        .route("/login/{target}", post(login_start).get(login_status))
        .route("/login/{target}/code", post(login_code))
        .route("/live", get(live))
        .layer(middleware::from_fn_with_state(app, auth))
}

async fn client_identities() -> Json<Value> {
    Json(crate::clients::snapshot())
}

async fn reset_client_identities(State(app): State<Arc<App>>) -> Json<Value> {
    crate::clients::reset();
    app.models.save_clients();
    app.broadcast("client-identities", Value::Null);
    Json(crate::clients::snapshot())
}

pub(crate) async fn auth(
    State(app): State<Arc<App>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let cfg = app.cfg();
    let collector_key =
        req.headers().get("authorization").and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("Bearer fbxc_"))
            || req.uri().query().is_some_and(|q| {
                url::form_urlencoded::parse(q.as_bytes()).any(|(k, v)| k == "key" && v.starts_with("fbxc_"))
            });
    if collector_key {
        return err(StatusCode::UNAUTHORIZED, "collector credentials are ingestion-only");
    }
    let key = cfg.management_key.clone();
    let loopback = addr.ip().to_canonical().is_loopback();
    let local = loopback && !proxied(req.headers());
    if key.is_empty() {
        if local {
            if let Some(problem) = foreign_page(&req) {
                return err(StatusCode::FORBIDDEN, problem);
            }
            return next.run(req).await;
        }
        return err(
            StatusCode::FORBIDDEN,
            if loopback {
                "requests through a proxy (tailscale serve, nginx, Caddy) need management-key"
            } else {
                "the dashboard is only reachable from localhost until you set management-key"
            },
        );
    }
    if cfg.management_allow_remote == Some(false) && !local {
        return err(StatusCode::FORBIDDEN, "remote management is off (allow-remote: false)");
    }
    let bearer = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(String::from);
    let query = req
        .uri()
        .query()
        .and_then(|q| url::form_urlencoded::parse(q.as_bytes()).find(|(k, _)| k == "key").map(|(_, v)| v.into_owned()));
    // Each client has its own count, also behind a proxy that names it: one machine's wrong
    // keys must not lock everyone else out.
    let ip = req.extensions().get::<crate::source::Source>().map_or_else(|| addr.ip().to_canonical(), |s| s.ip);
    if let Some(wait) = app.key_guesses.blocked(ip) {
        let mut resp = err(StatusCode::TOO_MANY_REQUESTS, format!("too many wrong keys; try again in {wait}s"));
        resp.headers_mut().insert("retry-after", wait.into());
        return resp;
    }
    let provided = bearer.or(query);
    if provided.as_deref().is_some_and(|k| management_key_matches(k, &key)) {
        app.key_guesses.forget(ip);
        return next.run(req).await;
    }
    // A missing key is a dashboard that hasn't asked for one yet, not a guess.
    if provided.as_deref().is_some_and(|k| !k.is_empty()) {
        app.key_guesses.wrong(ip);
    }
    err(StatusCode::UNAUTHORIZED, "management key required")
}

/// Wrong management keys per address. After five, the address is locked out, for twice
/// as long with each further wrong key: from 30 seconds up to 15 minutes.
#[derive(Default)]
pub struct Guesses(parking_lot::Mutex<Option<std::collections::HashMap<std::net::IpAddr, (u32, Instant)>>>);

impl Guesses {
    const FREE: u32 = 5;

    fn blocked(&self, ip: std::net::IpAddr) -> Option<u64> {
        let map = self.0.lock();
        let (count, at) = *map.as_ref()?.get(&ip)?;
        let lock = Self::lockout(count)?;
        let left = lock.saturating_sub(at.elapsed());
        (!left.is_zero()).then(|| left.as_secs().max(1))
    }

    fn lockout(count: u32) -> Option<Duration> {
        let over = count.checked_sub(Self::FREE)?;
        Some(Duration::from_secs(30u64.saturating_mul(1 << over.min(5)).min(900)))
    }

    fn wrong(&self, ip: std::net::IpAddr) {
        let mut map = self.0.lock();
        let map = map.get_or_insert_with(Default::default);
        // Forget addresses that have been quiet for an hour, so the map stays small.
        map.retain(|_, (_, at)| at.elapsed() < Duration::from_secs(3600));
        let entry = map.entry(ip).or_insert((0, Instant::now()));
        *entry = (entry.0 + 1, Instant::now());
        if entry.0 > Self::FREE {
            tracing::warn!(address = %ip, attempts = entry.0, "wrong management keys; address locked out for a while");
        }
    }

    fn forget(&self, ip: std::net::IpAddr) {
        if let Some(map) = self.0.lock().as_mut() {
            map.remove(&ip);
        }
    }
}

/// Fusebox trusts this machine's own address where no key is set (the dashboard, a proxy
/// without client keys). A web page must not borrow that trust: one on another site can
/// send requests here, and one whose name now points at 127.0.0.1 (DNS rebinding) is
/// treated by the browser as same-origin. Browsers name both: the page's host is in
/// `Host` or `Origin`. Local tools and pages on this machine are unaffected.
pub(crate) fn foreign_page(req: &Request) -> Option<&'static str> {
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| req.uri().authority().map(|a| a.as_str()));
    // A page rebound to 127.0.0.1 connects directly. Through a reverse proxy (tailscale
    // serve, say), the host is the proxy's own name, so only the page's origin counts.
    if !proxied(req.headers()) && host.is_some_and(|h| !loopback_host(h)) {
        return Some("open Fusebox at http://127.0.0.1 or http://localhost, or set a key to reach it by another name");
    }
    let origin = req.headers().get(axum::http::header::ORIGIN).and_then(|v| v.to_str().ok());
    if let Some(origin) = origin
        && url::Url::parse(origin).ok().and_then(|u| u.host_str().map(loopback_host)) != Some(true)
    {
        return Some("web pages on other sites can't use Fusebox without a key");
    }
    None
}

/// `localhost`, `*.localhost`, `127.x.x.x` or `[::1]`, with or without a port.
pub(crate) fn loopback_host(host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default().to_string()
    } else {
        host.rsplit_once(':')
            .filter(|(_, port)| port.parse::<u16>().is_ok())
            .map_or(host.as_str(), |(h, _)| h)
            .to_string()
    };
    name == "localhost"
        || name.ends_with(".localhost")
        || name.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.to_canonical().is_loopback())
}

/// A reverse proxy on the same host (tailscale serve, nginx, Caddy) connects from
/// loopback, but its forwarding headers show the request came from somewhere else.
/// Proxies that forward raw TCP add nothing, so behind those only a key protects.
fn proxied(headers: &axum::http::HeaderMap) -> bool {
    ["forwarded", "x-forwarded-for", "x-real-ip", "tailscale-user-login"].iter().any(|h| headers.contains_key(*h))
}

/// Plain keys compare in constant time; bcrypt hashes (CLIProxyAPI hashes
/// `secret-key` on first start) are verified once per key and remembered.
fn management_key_matches(provided: &str, configured: &str) -> bool {
    use sha2::{Digest, Sha256};
    static VERIFIED: parking_lot::Mutex<Vec<[u8; 32]>> = parking_lot::Mutex::new(Vec::new());
    if !["$2a$", "$2b$", "$2y$"].iter().any(|p| configured.starts_with(p)) {
        return constant_eq(provided, configured);
    }
    let id: [u8; 32] = Sha256::digest(format!("{configured}\0{provided}").as_bytes()).into();
    if VERIFIED.lock().contains(&id) {
        return true;
    }
    let ok = bcrypt::verify(provided, configured).unwrap_or(false);
    if ok {
        VERIFIED.lock().push(id);
    }
    ok
}

pub fn constant_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn ok() -> Response {
    Json(json!({ "ok": true })).into_response()
}

async fn overview(State(app): State<Arc<App>>) -> Json<Value> {
    let cfg = app.cfg();
    let accounts = app.pool.all();
    let (mut active, mut cooling, mut disabled) = (0, 0, 0);
    let mut providers = std::collections::BTreeMap::<&str, usize>::new();
    for a in &accounts {
        *providers.entry(a.provider.as_str()).or_default() += 1;
        let st = a.state.lock();
        if st.disabled {
            disabled += 1;
        } else if st.quota_refreshing
            || st.cooldowns.get("*").is_some_and(|t| *t > chrono::Utc::now())
            || st.quota_cooldowns.get("*").is_some_and(|t| *t > chrono::Utc::now())
            || st.quota.exhausted_until("").is_some()
        {
            cooling += 1;
        } else {
            active += 1;
        }
    }
    let host = if cfg.host == "0.0.0.0" || cfg.host.is_empty() { "127.0.0.1".to_string() } else { cfg.host.clone() };
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "started_at": app.started.to_rfc3339(),
        "uptime_secs": (chrono::Utc::now() - app.started).num_seconds(),
        "base_url": format!("http://{host}:{}", cfg.port),
        "client_keys": cfg.api_keys,
        // With named clients and no shared key, clients still need a key: their own.
        "named_clients": cfg.named_clients.len(),
        "routing": cfg.routing,
        "banked_resets": cfg.banked_resets,
        "banked_reset_policy": cfg.banked_reset_policy,
        "window_starter": cfg.window_starter,
        "session_affinity": cfg.session_affinity,
        "request_retry": cfg.request_retry.max(1),
        "management_key": !cfg.management_key.is_empty(),
        "totals": *app.stats.totals.lock(),
        "active": app.stats.active.load(Ordering::Relaxed),
        "series": app.stats.series(),
        "accounts": { "total": accounts.len(), "active": active, "cooling": cooling, "disabled": disabled, "providers": providers },
        // Each provider's subscriptions together, per window: the dashboard's combined limits.
        "pools": crate::push::current_pools(&app),
        "models": app.pool.models().len(),
        "config_path": app.cfg_path.display().to_string(),
        "auth_dir": cfg.auth_dir().display().to_string(),
    }))
}

async fn accounts(State(app): State<Arc<App>>) -> Json<Value> {
    let cfg = app.cfg();
    Json(Value::Array(
        app.pool
            .all()
            .iter()
            .map(|a| {
                let mut snapshot = a.snapshot();
                snapshot["banked_reset_plan"] =
                    serde_json::to_value(crate::banked_resets::plan(a, &cfg)).unwrap_or_default();
                snapshot["quota_attribution"] = attribution_json(&a.state.lock().attribution, &cfg);
                snapshot["window_starter_supported"] = json!(crate::window_starter::supported(a));
                snapshot["window_starter_selected"] = json!(cfg.window_starter.selected(a));
                snapshot
            })
            .collect(),
    ))
}

/// A client key as people know it: the named client's label, the program, or neither.
pub fn client_label(cfg: &Config, key: &str) -> String {
    if key == "system:window-starter" {
        return "Window starter".into();
    }
    let name = |c: &crate::config::NamedClient| if c.label.trim().is_empty() { c.id.clone() } else { c.label.clone() };
    // A shared key's usage id is a hash of the key. One that has since been named keeps
    // its value, so its earlier use goes by the client's name too.
    let named_since = |hash: &str| {
        cfg.named_clients
            .iter()
            .find(|c| crate::affinity::scope_for_key(Some(&c.key)).starts_with(hash))
            .map(|c| format!("{} (before naming)", name(c)))
    };
    if let Some(program) = key.strip_prefix("app:") {
        return program.to_string();
    }
    if let Some(prefix) = key.strip_prefix("key:") {
        return named_since(prefix).filter(|_| prefix.len() >= 8).unwrap_or_else(|| format!("Shared key {prefix}…"));
    }
    match cfg.named_clients.iter().find(|c| c.id == key) {
        Some(c) => name(c),
        None if key.is_empty() => "Other requests".into(),
        None if key == "dashboard" => "Dashboard playground".into(),
        None if key.len() >= 16 && key.bytes().all(|b| b.is_ascii_hexdigit()) => {
            named_since(key).unwrap_or_else(|| format!("Shared key {}…", &key[..8]))
        }
        None => key.to_string(),
    }
}

fn ranked(map: &std::collections::BTreeMap<String, f64>, limit: usize, label: impl Fn(&str) -> String) -> Vec<Value> {
    let mut items: Vec<(&String, &f64)> = map.iter().filter(|(_, p)| **p > 0.0).collect();
    items.sort_by(|a, b| b.1.total_cmp(a.1).then(a.0.cmp(b.0)));
    items
        .into_iter()
        .take(limit)
        .map(|(id, points)| json!({ "id": id, "label": label(id), "points": points }))
        .collect()
}

fn share_json(s: &crate::attribution::Share, cfg: &Config) -> Value {
    json!({
        "key": crate::attribution::window_key(s),
        "name": s.name,
        "model": s.model,
        "resets_at": s.resets_at,
        "since": s.since,
        "used": s.used,
        "before": s.before,
        "attributed": s.attributed(),
        "outside": s.outside,
        "by_tokens": s.by_tokens,
        "clients": ranked(&s.clients, 50, |id| client_label(cfg, id)),
        "models": ranked(&s.models, 20, |m| m.to_string()),
        "sessions": s.sessions.len(),
    })
}

/// Which clients used an account's subscription windows: the current windows and
/// the ones that renewed last, in percentage points of each window.
pub fn attribution_json(a: &crate::attribution::Attribution, cfg: &Config) -> Value {
    json!({
        "basis": "estimate",
        "current": a.current.iter().map(|s| share_json(s, cfg)).collect::<Vec<_>>(),
        "previous": a.previous.iter().take(4).map(|s| share_json(s, cfg)).collect::<Vec<_>>(),
    })
}

#[derive(Deserialize)]
struct RequestsQuery {
    limit: Option<usize>,
}

/// The request log, newest first; `limit` keeps a first paint small.
async fn requests(State(app): State<Arc<App>>, Query(q): Query<RequestsQuery>) -> Json<Value> {
    let recent = app.stats.recent.lock();
    let limit = q.limit.unwrap_or(usize::MAX);
    Json(serde_json::to_value(recent.iter().rev().take(limit).collect::<Vec<_>>()).unwrap_or_default())
}

/// What has tripped: expired sign-ins, used-up limits, rate limits, account errors,
/// runs of failed requests and providers with no account left. The same rules as the
/// faults menu and push notifications.
async fn faults(State(app): State<Arc<App>>) -> Json<Value> {
    Json(crate::push::current_faults(&app))
}

/// One account's last hour, and the coding sessions pinned to it. Session details
/// (client, model, requests) come from the recent request log, so older ones show none.
async fn account_activity(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    let Some(account) = app.pool.get(&id) else {
        return err(StatusCode::NOT_FOUND, "unknown account");
    };
    #[derive(Default)]
    struct Seen {
        session: String,
        since: Option<chrono::DateTime<chrono::Utc>>,
        client: &'static str,
        client_app: Option<&'static str>,
        source: Option<crate::source::Source>,
        key_label: Option<String>,
        model: String,
        requests: u64,
        cache_tokens: u64,
        usage_missing: u64,
        usage_partial: u64,
    }
    let mut seen: std::collections::HashMap<String, Seen> = Default::default();
    for log in app.stats.recent.lock().iter().filter(|l| l.account_id == id) {
        let Some(session) = &log.session_id else { continue };
        let s = seen.entry(crate::affinity::owner_of(session)).or_default();
        s.session.clone_from(session);
        s.since = Some(s.since.map_or(log.ts, |t| t.min(log.ts)));
        (s.client, s.client_app) = (log.client, log.client_app);
        if log.source.is_some() {
            s.source.clone_from(&log.source);
            s.key_label.clone_from(&log.key_label);
        }
        s.model.clone_from(&log.model);
        s.requests += 1;
        s.cache_tokens += log.cache_tokens;
        s.usage_missing += u64::from(log.usage_completeness == "missing");
        s.usage_partial += u64::from(log.usage_completeness == "partial");
    }
    let sessions: Vec<Value> = app
        .sessions
        .pinned(&id, app.cfg().session_affinity_idle_seconds)
        .into_iter()
        .map(|p| {
            let s = seen.remove(&p.owner).unwrap_or_default();
            // What this session used of each current window, in points.
            let quota = account.state.lock().attribution.session(&s.session);
            json!({
                "quota": quota,
                "session": Some(s.session).filter(|s| !s.is_empty()),
                "last_seen": chrono::DateTime::from_timestamp(p.last_seen, 0).map(|t| t.to_rfc3339()),
                "active": p.active,
                "since": s.since.map(|t| t.to_rfc3339()),
                "client": Some(s.client).filter(|c| !c.is_empty()),
                "client_app": s.client_app,
                "source": s.source,
                "key_label": s.key_label,
                "model": Some(s.model).filter(|m| !m.is_empty()),
                "requests": s.requests,
                "cache_tokens": s.cache_tokens,
                "usage_missing": s.usage_missing,
                "usage_partial": s.usage_partial,
            })
        })
        .collect();
    Json(json!({ "series": app.stats.account_series(&id), "sessions": sessions })).into_response()
}

/// For every public model, the accounts new sessions would try, in order.
async fn routes(State(app): State<Arc<App>>) -> Json<Value> {
    let cfg = app.cfg();
    let load = app.sessions.account_load(&cfg);
    let models: serde_json::Map<String, Value> = app
        .pool
        .models()
        .into_iter()
        .map(|(id, _)| {
            let (only, model) = app.pool.route(&id);
            let model = app.pool.canonical(&model, only.as_ref());
            let steps = app.pool.route_order(&model, &cfg, only.as_ref(), &load);
            (id, serde_json::to_value(steps).unwrap_or_default())
        })
        .collect();
    let groups: serde_json::Map<String, Value> = cfg
        .model_groups
        .iter()
        .map(|(name, g)| (name.clone(), json!({ "models": g.models, "balance": g.balance })))
        .collect();
    Json(json!({
        "routing": cfg.routing,
        "session_affinity": cfg.session_affinity,
        "request_retry": cfg.request_retry.max(1),
        "models": models,
        "groups": groups,
        "background_model": cfg.background_model,
        "removed_models": app.models.removed(),
    }))
}

/// Public models; `prefix` marks ids that only reach the accounts carrying that prefix,
/// `added` the ones added from the dashboard that Fusebox doesn't know yet.
async fn models(State(app): State<Arc<App>>) -> Json<Value> {
    Json(model_list(&app))
}

/// What `/api/models` returns; its fingerprint decides when dashboards refetch it.
pub fn model_list(app: &App) -> Value {
    let models = app.pool.models().into_iter().map(|(m, p)| {
        let mut v = json!({ "id": m, "provider": p, "capabilities": crate::model_catalog::capabilities(app, &m),
            "codex_catalog": crate::model_catalog::codex_template(app, &m).is_some() });
        match app.pool.route(&m) {
            (Some(Only::Prefix(prefix)), _) => v["prefix"] = prefix.into(),
            (_, base) if crate::discovery::added(app, p, &base) => v["added"] = true.into(),
            _ => {}
        }
        v
    });
    let mut models: Vec<Value> = models.collect();
    for id in app.cfg().model_groups.keys() {
        if !models.iter().any(|m| m["id"] == *id) {
            models.push(json!({ "id": id, "provider": "fusebox", "capabilities": crate::model_catalog::capabilities(app, id), "codex_catalog": false }));
        }
    }
    Value::Array(models)
}

#[derive(Deserialize)]
struct AddedModel {
    provider: String,
    model: String,
}

/// Adds a model Fusebox doesn't know yet to `extra-models`.
async fn add_model(State(app): State<Arc<App>>, Json(b): Json<AddedModel>) -> Response {
    let model = b.model.trim();
    let provider = match Provider::parse(&b.provider) {
        Some(Provider::Compat) => {
            return err(StatusCode::BAD_REQUEST, "Compatible providers list their models in Config");
        }
        Some(p) => p,
        None => return err(StatusCode::BAD_REQUEST, "Choose a provider"),
    };
    if !crate::discovery::valid_model_id(model) {
        return err(StatusCode::BAD_REQUEST, "Use the model id as the provider writes it, like claude-opus-6");
    }
    if app.pool.knows(provider, model) {
        return err(StatusCode::CONFLICT, format!("Fusebox already knows {model}"));
    }
    let mut models = app.cfg().extra_models.clone();
    let list = models.entry(provider.as_str().to_string()).or_default();
    if !list.iter().any(|m| m.eq_ignore_ascii_case(model)) {
        list.push(model.to_string());
    }
    match crate::discovery::save_extra_models(&app, models) {
        Ok(()) => ok(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not save config: {e:#}")),
    }
}

async fn remove_model(State(app): State<Arc<App>>, Json(b): Json<AddedModel>) -> Response {
    let Some(provider) = Provider::parse(&b.provider) else { return err(StatusCode::BAD_REQUEST, "Choose a provider") };
    let mut models = app.cfg().extra_models.clone();
    for (key, list) in models.iter_mut() {
        if Provider::parse(key) == Some(provider) {
            list.retain(|m| !m.eq_ignore_ascii_case(b.model.trim()));
        }
    }
    models.retain(|_, list| !list.is_empty());
    match crate::discovery::save_extra_models(&app, models) {
        Ok(()) => ok(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not save config: {e:#}")),
    }
}

/// Schedule bounded model checks and report any overlapping checks still in progress.
async fn check_models(State(app): State<Arc<App>>) -> Json<crate::discovery::CheckResult> {
    let result = crate::discovery::check_now(&app).await;
    app.broadcast("accounts", Value::Null);
    Json(result)
}

#[derive(Deserialize)]
struct ToggleBody {
    disabled: bool,
}

async fn toggle_account(State(app): State<Arc<App>>, Path(id): Path<String>, Json(b): Json<ToggleBody>) -> Response {
    let Some(acct) = app.pool.get(&id) else { return err(StatusCode::NOT_FOUND, "unknown account") };
    if let Some(path) = &acct.path
        && let Err(e) = set_file_disabled(path, b.disabled)
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    acct.state.lock().disabled = b.disabled;
    app.broadcast("accounts", Value::Null);
    ok()
}

async fn refresh_account(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    let Some(acct) = app.pool.get(&id) else { return err(StatusCode::NOT_FOUND, "unknown account") };
    if !acct.is_oauth() {
        return err(StatusCode::BAD_REQUEST, "API keys don't need refreshing");
    }
    match oauth::ensure_fresh(&app, &acct, chrono::Duration::minutes(5), true).await {
        Ok(()) => {
            let _ = crate::quota::poll(&app, &acct).await;
            acct.state.lock().cooldowns.remove("*");
            app.broadcast("accounts", Value::Null);
            ok()
        }
        Err(e) => err(StatusCode::BAD_GATEWAY, format!("{e:#}")),
    }
}

async fn reset_account(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    let Some(acct) = app.pool.get(&id) else { return err(StatusCode::NOT_FOUND, "unknown account") };
    let mut st = acct.state.lock();
    st.cooldowns.clear();
    st.quota_cooldowns.clear();
    st.strikes = 0;
    st.clear_error();
    drop(st);
    app.broadcast("accounts", Value::Null);
    ok()
}

const RESETS_OFF: &str = "Banked resets are turned off. Turn them on under Config, Connections.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClearErrorBody {
    error_id: String,
}

async fn clear_account_error(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(b): Json<ClearErrorBody>,
) -> Response {
    let Some(acct) = app.pool.get(&id) else { return err(StatusCode::NOT_FOUND, "unknown account") };
    let Ok(error_id) = uuid::Uuid::parse_str(&b.error_id) else {
        return err(StatusCode::BAD_REQUEST, "A valid error ID is required");
    };
    if !acct.state.lock().clear_error_if(&error_id.to_string()) {
        return err(StatusCode::CONFLICT, "A newer error arrived. Review it before clearing.");
    }
    app.broadcast("accounts", Value::Null);
    Json(acct.snapshot()).into_response()
}

async fn start_window(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(action): Json<crate::window_starter::Action>,
) -> Response {
    let Some(acct) = app.pool.get(&id) else { return err(StatusCode::NOT_FOUND, "unknown account") };
    // The journal remains authoritative if the browser disconnects mid-request.
    match tokio::spawn(crate::window_starter::start(app, acct, action, false)).await {
        Ok(Ok(view)) => Json(view).into_response(),
        Ok(Err(e)) => err(StatusCode::CONFLICT, e.to_string()),
        Err(_) => {
            err(StatusCode::INTERNAL_SERVER_ERROR, "Window starter interrupted; check provider status before retrying")
        }
    }
}

async fn banked_resets(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    if !app.cfg().banked_resets {
        return err(StatusCode::NOT_FOUND, RESETS_OFF);
    }
    let Some(acct) = app.pool.get(&id) else { return err(StatusCode::NOT_FOUND, "unknown account") };
    match crate::banked_resets::refresh(&app, &acct).await {
        Ok(view) => Json(view).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}
async fn apply_banked_reset(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<crate::banked_resets::Action>,
) -> Response {
    if !app.cfg().banked_resets {
        return err(StatusCode::NOT_FOUND, RESETS_OFF);
    }
    let Some(acct) = app.pool.get(&id) else { return err(StatusCode::NOT_FOUND, "unknown account") };
    match tokio::spawn(crate::banked_resets::apply(app, acct, body)).await {
        Ok(Ok(view)) => Json(view).into_response(),
        Ok(Err(e)) => err(StatusCode::CONFLICT, e.to_string()),
        Err(_) => {
            err(StatusCode::INTERNAL_SERVER_ERROR, "Reset operation interrupted; refresh its status before continuing")
        }
    }
}
async fn refresh_quota(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    let Some(acct) = app.pool.get(&id) else { return err(StatusCode::NOT_FOUND, "unknown account") };
    if !matches!(acct.provider, Provider::Codex | Provider::Claude) || !acct.is_oauth() {
        return err(
            StatusCode::BAD_REQUEST,
            "Subscription quota is only available for Codex and Claude OAuth accounts",
        );
    }
    if oauth::ensure_fresh(&app, &acct, chrono::Duration::minutes(5), false).await.is_err() {
        return err(StatusCode::BAD_GATEWAY, "Could not refresh subscription credentials");
    }
    // Quota can still refresh if this subscription does not offer banked resets.
    acct.state.lock().quota_epoch += 1;
    if crate::quota::poll(&app, &acct).await.is_err() {
        return err(StatusCode::BAD_GATEWAY, "Could not refresh provider usage");
    }
    let _ = crate::quota::poll_plan(&app, &acct).await;
    app.broadcast("accounts", Value::Null);
    if !app.cfg().banked_resets {
        return ok();
    }
    match crate::banked_resets::refresh(&app, &acct).await {
        Ok(view) => Json(view).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

async fn delete_account(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    let Some(acct) = app.pool.get(&id) else { return err(StatusCode::NOT_FOUND, "unknown account") };
    if let Some(path) = &acct.path {
        if let Err(e) = std::fs::remove_file(path) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
        app.reload_accounts();
        return ok();
    }
    let key = match &*acct.cred.read() {
        Credential::ApiKey { key, .. } => key.clone(),
        _ => String::new(),
    };
    let group = if acct.provider == Provider::Compat { acct.group.clone() } else { None };
    edit_config(&app, |doc| crate::compat::remove_key(doc, &key, group.as_deref()))
}

#[derive(Deserialize)]
struct KeyBody {
    provider: String,
    api_key: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    models: String,
}

async fn add_key(State(app): State<Arc<App>>, Json(b): Json<KeyBody>) -> Response {
    let key = b.api_key.trim().to_string();
    let base = Some(b.base_url.trim().to_string()).filter(|s| !s.is_empty());
    let models: Vec<ModelAlias> = b
        .models
        .split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|m| match m.split_once('=') {
            Some((alias, name)) => ModelAlias { name: name.trim().into(), alias: Some(alias.trim().into()) },
            None => ModelAlias { name: m.into(), alias: None },
        })
        .collect();
    let models: Vec<(String, Option<String>)> = models.into_iter().map(|m| (m.name, m.alias)).collect();
    let (group, name) = match Provider::parse(&b.provider) {
        Some(Provider::Compat) => {
            let Some(base) = &base else { return err(StatusCode::BAD_REQUEST, "base URL is required") };
            if models.is_empty() {
                return err(StatusCode::BAD_REQUEST, "list at least one model");
            }
            let name = if b.name.trim().is_empty() {
                url::Url::parse(base)
                    .ok()
                    .and_then(|u| u.host_str().map(String::from))
                    .unwrap_or_else(|| "provider".into())
            } else {
                b.name.trim().to_string()
            };
            ("openai-compatibility", Some(name))
        }
        Some(
            p @ (Provider::Claude
            | Provider::Codex
            | Provider::Gemini
            | Provider::Vertex
            | Provider::Kimi
            | Provider::Xai
            | Provider::Meta),
        ) => {
            if key.is_empty() {
                return err(StatusCode::BAD_REQUEST, "API key is required");
            }
            (p.as_str(), None)
        }
        Some(p) => return err(StatusCode::BAD_REQUEST, format!("{} does not take API keys", p.as_str())),
        None => return err(StatusCode::BAD_REQUEST, "unknown provider"),
    };
    let new = crate::compat::NewKey { group, api_key: &key, base_url: base.as_deref(), models, name: name.as_deref() };
    edit_config(&app, |doc| crate::compat::add_key(doc, &new))
}

#[derive(Deserialize)]
struct VertexBody {
    json: String,
    #[serde(default)]
    location: String,
}

async fn import_vertex(State(app): State<Arc<App>>, Json(b): Json<VertexBody>) -> Response {
    match crate::vertex::import(&app, &b.json, &b.location).await {
        Ok(label) => Json(json!({ "ok": true, "label": label })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

/// Applies an edit to the config file's YAML tree, keeping every setting this
/// binary doesn't know about (so the file still works with CLIProxyAPI).
/// A rewrite drops comments, so the commented original is kept once as config.yaml.bak.
pub(crate) fn keep_original(app: &App, text: &str) {
    let backup = app.cfg_path.with_extension("yaml.bak");
    if text.contains('#') && !backup.exists() {
        let _ = std::fs::write(&backup, text);
    }
}

fn edit_config(app: &Arc<App>, edit: impl FnOnce(&mut serde_yaml::Value)) -> Response {
    let _guard = app.config_write.lock();
    let text = match std::fs::read_to_string(&app.cfg_path) {
        Ok(text) => text,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not read config: {e}")),
    };
    let mut doc: serde_yaml::Value = match serde_yaml::from_str(&text) {
        Ok(serde_yaml::Value::Null) | Err(_) if text.trim().is_empty() => {
            serde_yaml::Value::Mapping(Default::default())
        }
        Ok(v) => v,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("config.yaml doesn't parse: {e}")),
    };
    let original = doc.clone();
    edit(&mut doc);
    let out = match crate::config_editor::render(&text, &original, &doc) {
        Ok((t, rewritten)) => {
            if rewritten {
                keep_original(app, &text);
            }
            t
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    };
    let cfg = match Config::parse(&out) {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    };
    if let Err(e) = std::fs::write(&app.cfg_path, out) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    app.set_config(cfg);
    ok()
}

async fn get_config(State(app): State<Arc<App>>) -> Json<Value> {
    let text = std::fs::read_to_string(&app.cfg_path).unwrap_or_default();
    let revision = crate::config_editor::revision(&text);
    Json(json!({ "text": text, "revision": revision, "path": app.cfg_path.display().to_string() }))
}

#[derive(Deserialize)]
struct ConfigBody {
    text: String,
    /// The revision the edit started from; the save is refused if the file moved on.
    revision: String,
}

async fn put_config(State(app): State<Arc<App>>, Json(b): Json<ConfigBody>) -> Response {
    let _guard = app.config_write.lock();
    let current = std::fs::read_to_string(&app.cfg_path).unwrap_or_default();
    if b.revision != crate::config_editor::revision(&current) {
        return err(StatusCode::CONFLICT, "The config changed since you opened it. Reload the file before saving.");
    }
    let cfg = match Config::parse(&b.text) {
        Ok(c) => c,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    };
    if let Err(e) = std::fs::write(&app.cfg_path, &b.text) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let restart = !crate::config_editor::restart_fields(&app.startup_config, &cfg).is_empty();
    app.set_config(cfg);
    let revision = crate::config_editor::revision(&b.text);
    Json(json!({ "ok": true, "restart_required": restart, "revision": revision })).into_response()
}

fn settings_response(app: &Arc<App>, text: &str) -> anyhow::Result<Value> {
    let cfg = Config::parse(text)?;
    let restart = crate::config_editor::restart_fields(&app.startup_config, &cfg);
    Ok(json!({
        "values": crate::config_editor::values(text)?,
        "defaults": crate::config_editor::values("")?,
        "revision": crate::config_editor::revision(text),
        "path": app.cfg_path.display().to_string(),
        "ignored": cfg.ignored,
        "restart_fields": restart,
        "restart_required": !restart.is_empty(),
    }))
}

async fn get_settings(State(app): State<Arc<App>>) -> Response {
    let _guard = app.config_write.lock();
    let text = match std::fs::read_to_string(&app.cfg_path) {
        Ok(text) => text,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not read config: {e}")),
    };
    match settings_response(&app, &text) {
        Ok(body) => Json(body).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

#[derive(Deserialize)]
struct SettingsBody {
    revision: String,
    changes: serde_json::Map<String, Value>,
    /// Work out the new file and return it next to the current one, without saving.
    #[serde(default)]
    dry_run: bool,
}

async fn patch_settings(State(app): State<Arc<App>>, Json(body): Json<SettingsBody>) -> Response {
    let _guard = app.config_write.lock();
    let text = match std::fs::read_to_string(&app.cfg_path) {
        Ok(text) => text,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not read config: {e}")),
    };
    if body.revision != crate::config_editor::revision(&text) {
        return err(
            StatusCode::CONFLICT,
            "The config changed since you opened it. Reload the latest settings before saving.",
        );
    }
    let (out, cfg, rewritten) = match crate::config_editor::apply(&text, &body.changes) {
        Ok(result) => result,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    };
    if body.dry_run {
        return Json(json!({ "before": text, "after": out, "rewritten": rewritten })).into_response();
    }
    let mut response = match settings_response(&app, &out) {
        Ok(body) => body,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    };
    response["rewritten"] = rewritten.into();
    // Also catch external file changes made while validation was running.
    if std::fs::read_to_string(&app.cfg_path).ok().as_deref() != Some(&text) {
        return err(StatusCode::CONFLICT, "The config changed while saving. Reload the latest settings before saving.");
    }
    if out != text {
        if rewritten {
            keep_original(&app, &text);
        }
        if let Err(e) = std::fs::write(&app.cfg_path, &out) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not save config: {e}"));
        }
        app.set_config(cfg);
    }
    Json(response).into_response()
}

async fn login_start(State(app): State<Arc<App>>, Path(target): Path<String>) -> Response {
    let Some(provider) = Provider::parse(&target) else { return err(StatusCode::BAD_REQUEST, "unknown provider") };
    match start_login(&app, provider).await {
        Ok((state, l)) => Json(json!({
            "state": state, "url": l.url, "callback": l.callback, "kind": l.kind, "user_code": l.user_code,
        }))
        .into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

async fn login_status(State(app): State<Arc<App>>, Path(target): Path<String>) -> Response {
    match app.logins.lock().get(&target) {
        Some(l) => Json(serde_json::to_value(l).unwrap_or_default()).into_response(),
        None => err(StatusCode::NOT_FOUND, "unknown login"),
    }
}

#[derive(Deserialize)]
struct CodeBody {
    input: String,
}

async fn login_code(State(app): State<Arc<App>>, Path(target): Path<String>, Json(b): Json<CodeBody>) -> Response {
    let (code, state) = parse_pasted(&b.input);
    if code.is_empty() {
        return err(StatusCode::BAD_REQUEST, "no authorization code found");
    }
    if state.as_deref().is_some_and(|s| s != target) {
        return err(StatusCode::BAD_REQUEST, "that URL belongs to a different login attempt");
    }
    match complete_login(&app, &target, &code).await {
        Ok(label) => Json(json!({ "ok": true, "label": label })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

/// What each busy account is doing right now: requests in flight (including
/// streams that have not finished), coding sessions seen in the last few minutes,
/// and those seen in the last half hour, which are still going on between turns.
fn account_load(app: &App) -> Value {
    let counts = app.sessions.session_counts(app.cfg().session_affinity_idle_seconds);
    let mut load = serde_json::Map::new();
    for a in app.pool.all() {
        let in_flight = a.state.lock().active_requests.load(Ordering::Relaxed);
        let (sessions, ongoing) = counts.get(&a.id).copied().unwrap_or_default();
        if in_flight > 0 || ongoing > 0 {
            load.insert(
                a.id.clone(),
                json!({ "in_flight": in_flight, "sessions": sessions, "ongoing_sessions": ongoing }),
            );
        }
    }
    Value::Object(load)
}

fn live_tick(app: &App) -> String {
    json!({ "type": "tick", "data": {
        "active": app.stats.active.load(Ordering::Relaxed),
        "totals": *app.stats.totals.lock(),
    }})
    .to_string()
}

fn live_load(app: &App) -> String {
    format!(r#"{{"type":"load","data":{}}}"#, account_load(app))
}

fn live_faults(app: &App) -> String {
    format!(r#"{{"type":"faults","data":{}}}"#, crate::push::current_faults(app))
}

/// One ticker for every dashboard: account load and faults are checked each second
/// and totals every five, computed once and broadcast only when they change. It
/// starts with the first dashboard connection and idles while none is connected.
async fn live_ticker(app: Arc<App>) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let (mut load, mut faults, mut tick) = (String::new(), String::new(), String::new());
    let mut seconds = 0_u64;
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = app.shutdown.cancelled() => return,
        }
        if app.live.receiver_count() == 0 {
            continue;
        }
        seconds += 1;
        let send = |last: &mut String, next: String| {
            if *last != next {
                let _ = app.live.send(next.clone());
                *last = next;
            }
        };
        send(&mut load, live_load(&app));
        send(&mut faults, live_faults(&app));
        if seconds.is_multiple_of(5) {
            send(&mut tick, live_tick(&app));
        }
    }
}

async fn live(State(app): State<Arc<App>>, ws: WebSocketUpgrade) -> Response {
    if !app.live_ticker.swap(true, Ordering::AcqRel) {
        tokio::spawn(live_ticker(app.clone()));
    }
    ws.on_upgrade(move |mut socket| async move {
        let mut rx = app.live.subscribe();
        // The current state first; the ticker only broadcasts changes.
        for msg in [live_load(&app), live_faults(&app), live_tick(&app)] {
            if socket.send(Message::Text(msg.into())).await.is_err() {
                return;
            }
        }
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Ok(m) => if socket.send(Message::Text(m.into())).await.is_err() { break },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                },
                incoming = socket.recv() => match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                },
                // Going away: the dashboard reconnects to the next run by itself.
                _ = app.shutdown.cancelled() => {
                    let _ = socket.send(Message::Close(Some(axum::extract::ws::CloseFrame {
                        code: axum::extract::ws::close_code::AWAY,
                        reason: "Fusebox is restarting".into(),
                    }))).await;
                    break;
                }
            }
        }
    })
}

#[derive(Deserialize)]
struct PriorityBody {
    priority: i32,
}

/// Higher priorities take new sessions first. A sign-in keeps it in its credential file;
/// an API key in its config entry (a compatible provider's applies to all its keys).
async fn set_priority(State(app): State<Arc<App>>, Path(id): Path<String>, Json(b): Json<PriorityBody>) -> Response {
    let Some(acct) = app.pool.get(&id) else { return err(StatusCode::NOT_FOUND, "unknown account") };
    if let Some(path) = &acct.path {
        app.suppress_reload();
        let value = (b.priority != 0).then(|| json!(b.priority));
        if let Err(e) = crate::accounts::set_file_field(path, "priority", value) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not save the priority: {e}"));
        }
        app.reload_accounts();
        return ok();
    }
    let _guard = app.config_write.lock();
    let text = match std::fs::read_to_string(&app.cfg_path) {
        Ok(text) => text,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not read config: {e}")),
    };
    let values = match crate::config_editor::values(&text) {
        Ok(v) => v,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    };
    let fields = [
        ("claude-api-key", "claude"),
        ("codex-api-key", "codex"),
        ("gemini-api-key", "gemini"),
        ("vertex-api-key", "vertex"),
        ("kimi-api-key", "kimi"),
        ("xai-api-key", "xai"),
        ("meta-api-key", "meta"),
    ];
    let mut change = None;
    for (field, provider) in fields {
        let mut list = values[field].as_array().cloned().unwrap_or_default();
        if let Some(entry) = list
            .iter_mut()
            .find(|e| e["api-key"].as_str().is_some_and(|k| crate::accounts::key_account_id(provider, k) == id))
        {
            set_entry_priority(entry, b.priority);
            change = Some((field, list));
            break;
        }
    }
    if change.is_none() {
        let mut list = values["openai-compatibility"].as_array().cloned().unwrap_or_default();
        if let Some(entry) = list.iter_mut().find(|e| {
            let name = e["name"].as_str().unwrap_or_default();
            id == format!("compat-{name}:nokey")
                || e["api-keys"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .any(|k| crate::accounts::key_account_id(&format!("compat-{name}"), k) == id)
        }) {
            set_entry_priority(entry, b.priority);
            change = Some(("openai-compatibility", list));
        }
    }
    let Some((field, list)) = change else { return err(StatusCode::NOT_FOUND, "that key isn't in config.yaml") };
    let changes = serde_json::Map::from_iter([(field.to_string(), Value::Array(list))]);
    let (out, cfg, rewritten) = match crate::config_editor::apply(&text, &changes) {
        Ok(result) => result,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    };
    if rewritten {
        keep_original(&app, &text);
    }
    if let Err(e) = std::fs::write(&app.cfg_path, &out) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not save config: {e}"));
    }
    app.set_config(cfg);
    ok()
}

/// Normal priority (0) is no `priority` line at all, as in a file that never set one.
fn set_entry_priority(entry: &mut Value, priority: i32) {
    if let Some(o) = entry.as_object_mut() {
        if priority == 0 {
            o.remove("priority");
        } else {
            o.insert("priority".into(), json!(priority));
        }
    }
}

/// Addresses listed per named client.
const ADDRESSES_SHOWN: usize = 20;

/// What each named client used today and this month, beside its limits, and when each
/// client key was last used since Fusebox started (shared keys in `api-keys` order),
/// where named clients were used from, and which keys and addresses are connected now.
async fn client_usage(State(app): State<Arc<App>>) -> Json<Value> {
    let cfg = app.cfg();
    let clients: serde_json::Map<String, Value> =
        cfg.named_clients.iter().map(|c| (c.id.clone(), json!(app.clients.spend(&c.id)))).collect();
    let key_used = |key: &str| app.clients.last_used(&crate::affinity::scope_for_key(Some(key)));
    // A client named from a shared key was last used under that key's hash until then.
    let last_used: serde_json::Map<String, Value> = cfg
        .named_clients
        .iter()
        .filter_map(|c| Some((c.id.clone(), json!(app.clients.last_used(&c.id).max(key_used(&c.key))?))))
        .collect();
    let shared_last_used: Vec<_> = cfg.api_keys.iter().map(|k| key_used(k)).collect();
    // Where each named client's key was used from, most recent first.
    let addresses: serde_json::Map<String, Value> = cfg
        .named_clients
        .iter()
        .map(|c| {
            let seen: Vec<Value> = app
                .addresses
                .history(&c.id)
                .into_iter()
                .take(ADDRESSES_SHOWN)
                .map(|(ip, s)| {
                    json!({ "ip": ip, "via": s.via, "login": s.login, "first": s.first, "last": s.last, "requests": s.requests })
                })
                .collect();
            (c.id.clone(), json!(seen))
        })
        .collect();
    let connected: Vec<Value> = app
        .addresses
        .connected()
        .into_iter()
        .map(|c| {
            let mut v = json!(c);
            v["label"] = json!(client_label(&cfg, &c.key));
            v
        })
        .collect();
    Json(json!({
        "clients": clients,
        "estimates": app.usage.is_some(),
        "last_used": last_used,
        "shared_last_used": shared_last_used,
        "addresses": addresses,
        "connected": connected,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn clearing_an_account_error_is_authenticated_scoped_and_race_safe() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let app = App::new(
            Config {
                auth_dir: "/nonexistent".into(),
                management_key: "management-only".into(),
                api_keys: vec!["inference-only".into()],
                claude_api_key: vec![crate::config::KeyEntry { api_key: "mock-only".into(), ..Default::default() }],
                ..Default::default()
            },
            "/nonexistent/config.yaml".into(),
        );
        let account = app.pool.all()[0].clone();
        let now = chrono::Utc::now();
        {
            let mut state = account.state.lock();
            state.cooldowns.insert("claude-opus-5-5".into(), now + chrono::Duration::minutes(1));
            state.quota_cooldowns.insert("*".into(), now + chrono::Duration::hours(1));
            state.quota.set(
                vec![crate::quota::Window {
                    name: "week".into(),
                    used: 100.0,
                    resets_at: Some(now + chrono::Duration::hours(1)),
                    model: None,
                }],
                None,
            );
            state.counters.requests = 19;
            state.strikes = 3;
            state.disabled = true;
            state.set_error("400: billing restriction");
        }
        let before = account.snapshot();
        let old_id = before["last_error_id"].as_str().unwrap().to_string();
        let service = router(app.clone()).with_state(app.clone());
        let post = |key: Option<&str>, body: Value, id: &str| {
            let mut request = Request::builder()
                .method("POST")
                .uri(format!("/accounts/{id}/error/clear"))
                .header("content-type", "application/json");
            if let Some(key) = key {
                request = request.header("authorization", format!("Bearer {key}"));
            }
            let mut request = request.body(Body::from(body.to_string())).unwrap();
            request.extensions_mut().insert(ConnectInfo("192.0.2.1:1234".parse::<SocketAddr>().unwrap()));
            service.clone().oneshot(request)
        };
        let body = json!({"error_id":old_id});
        assert_eq!(post(None, body.clone(), &account.id).await.unwrap().status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            post(Some("inference-only"), body.clone(), &account.id).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(account.snapshot()["last_error_id"], before["last_error_id"]);
        assert_eq!(
            post(Some("management-only"), json!({}), &account.id).await.unwrap().status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            post(Some("management-only"), json!({"error_id":"invalid"}), &account.id).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post(Some("management-only"), body.clone(), "missing").await.unwrap().status(),
            StatusCode::NOT_FOUND
        );

        // Repeated identical messages are distinct occurrences.
        account.state.lock().set_error("400: billing restriction");
        let newer_id = account.snapshot()["last_error_id"].as_str().unwrap().to_string();
        assert_ne!(old_id, newer_id);
        assert_eq!(post(Some("management-only"), body, &account.id).await.unwrap().status(), StatusCode::CONFLICT);
        assert_eq!(account.snapshot()["last_error_id"], newer_id);
        let mut events = app.live.subscribe();
        let body = json!({"error_id":newer_id});
        let response = post(Some("management-only"), body.clone(), &account.id).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let after: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(after["last_error"].is_null());
        assert!(after["last_error_id"].is_null());
        for field in ["cooldowns", "cooldown_kinds", "quota", "counters", "disabled"] {
            assert_eq!(after[field], before[field], "clearing an error preserves {field}");
        }
        assert_eq!(account.state.lock().strikes, 3);
        assert_eq!(app.stats.recent.lock().len(), 0, "no inference or request-history entry is generated");
        let event: Value = serde_json::from_str(&events.try_recv().unwrap()).unwrap();
        assert_eq!(event["type"], "accounts");
        assert_eq!(
            post(Some("management-only"), body, &account.id).await.unwrap().status(),
            StatusCode::OK,
            "duplicate clears are harmless"
        );

        // An ordinary error with no cooldown must be dismissible too.
        {
            let mut state = account.state.lock();
            state.cooldowns.clear();
            state.quota_cooldowns.clear();
            state.quota = Default::default();
            state.set_error("400: third-party billing");
        }
        let body = json!({"error_id":account.snapshot()["last_error_id"]});
        assert_eq!(post(Some("management-only"), body, &account.id).await.unwrap().status(), StatusCode::OK);
        assert!(account.snapshot()["last_error"].is_null());
    }

    #[tokio::test]
    async fn requests_through_a_local_proxy_need_the_management_key() {
        let app = App::new(
            Config { auth_dir: "/nonexistent".into(), ..Default::default() },
            "/nonexistent/config.yaml".into(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let service = router(app.clone()).with_state(app.clone()).into_make_service_with_connect_info::<SocketAddr>();
        let server = tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });
        let client = reqwest::Client::new();
        let status = |header: Option<(&'static str, &'static str)>, key: Option<&'static str>| {
            let mut req = client.get(format!("{origin}/requests"));
            if let Some((name, value)) = header {
                req = req.header(name, value);
            }
            if let Some(key) = key {
                req = req.bearer_auth(key);
            }
            async move { req.send().await.unwrap().status().as_u16() }
        };
        let tailscale = Some(("tailscale-user-login", "someone@example.com"));
        let nginx = Some(("x-forwarded-for", "100.64.0.7"));

        assert_eq!(status(None, None).await, 200, "localhost needs no key");
        assert_eq!(status(tailscale, None).await, 403);
        assert_eq!(status(nginx, None).await, 403);
        assert_eq!(status(Some(("forwarded", "for=192.0.2.60")), None).await, 403);

        let mut cfg = (*app.cfg()).clone();
        cfg.management_key = "secret".into();
        app.set_config(cfg.clone());
        assert_eq!(status(nginx, None).await, 401);
        assert_eq!(status(nginx, Some("secret")).await, 200);
        cfg.management_allow_remote = Some(false);
        app.set_config(cfg);
        assert_eq!(status(nginx, Some("secret")).await, 403, "a proxied request is remote");
        assert_eq!(status(None, Some("secret")).await, 200);
        server.abort();
    }

    #[tokio::test]
    async fn faults_are_listed_and_streamed_when_they_change() {
        use futures::StreamExt;

        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            management_key: "secret".into(),
            codex_api_key: vec![crate::config::KeyEntry { api_key: "mock-only".into(), ..Default::default() }],
            ..Default::default()
        };
        let app = App::new(cfg, "/nonexistent/config.yaml".into());
        let account = app.pool.all()[0].clone();
        account.state.lock().set_error("upstream exploded");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let service = router(app.clone()).with_state(app.clone()).into_make_service_with_connect_info::<SocketAddr>();
        let server = tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });

        let client = reqwest::Client::new();
        let proxied = client.get(format!("http://{addr}/faults")).header("x-forwarded-for", "100.64.0.7");
        assert_eq!(proxied.send().await.unwrap().status(), 401, "faults need the management key");
        let listed: Value = client
            .get(format!("http://{addr}/faults"))
            .header("x-forwarded-for", "100.64.0.7")
            .bearer_auth("secret")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let fault = &listed.as_array().unwrap()[0];
        assert_eq!((fault["kind"].as_str(), fault["level"].as_str()), (Some("error"), Some("err")));
        assert_eq!(
            (fault["account_id"].as_str(), fault["detail"].as_str()),
            (Some(account.id.as_str()), Some("upstream exploded"))
        );

        let mut request =
            tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(format!("ws://{addr}/live"))
                .unwrap();
        request.headers_mut().insert("authorization", "Bearer secret".parse().unwrap());
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let mut next_faults = async || loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
            let msg: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
            if msg["type"] == "faults" {
                return msg["data"].clone();
            }
        };
        assert_eq!(next_faults().await, listed, "a new socket starts with every fault");
        account.state.lock().clear_error();
        assert_eq!(next_faults().await, json!([]), "a cleared fault is sent at once");
        server.abort();
    }

    #[tokio::test]
    async fn the_overview_pools_subscriptions_as_the_pace_faults_do() {
        // Keys reporting usage limits count as subscriptions, as on the dashboard.
        let key = |k: &str| crate::config::KeyEntry { api_key: k.into(), ..Default::default() };
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            codex_api_key: vec![key("one"), key("two")],
            ..Default::default()
        };
        let app = App::new(cfg, "/nonexistent/config.yaml".into());
        let resets = chrono::Utc::now() + chrono::Duration::days(3);
        for (a, used) in app.pool.all().iter().zip([90.0, 60.0]) {
            let mut state = a.state.lock();
            state.quota.windows =
                vec![crate::quota::Window { name: "week".into(), used, resets_at: Some(resets), model: None }];
            state.attribution.history =
                vec![crate::outlook::test_history(&state.quota.windows[0], chrono::Utc::now(), 20.0)];
        }
        let Json(view) = overview(State(app.clone())).await;
        let [pool] = view["pools"].as_array().unwrap().as_slice() else { panic!("one pool: {}", view["pools"]) };
        assert_eq!((pool["provider"].as_str(), pool["window"].as_str()), (Some("codex"), Some("week")));
        assert_eq!(
            (pool["accounts"].as_u64(), pool["reported"].as_u64(), pool["used"].as_f64()),
            (Some(2), Some(2), Some(75.0))
        );
        assert_eq!((pool["priced"].as_bool(), pool["runs_out_early"].as_bool()), (Some(false), Some(true)));
        assert_eq!(pool["until"], resets.to_rfc3339());
        assert_eq!(pool["outlook"]["state"], "run_out");
        assert_eq!(pool["outlook"]["min_remaining"], 0.0);
        assert_eq!(pool["outlook"]["recovers_at"], json!(resets));
        let faults = crate::push::current_faults(&app);
        let pace: Vec<&Value> = faults.as_array().unwrap().iter().filter(|f| f["kind"] == "pace").collect();
        assert_eq!(pace.len(), 1);
        assert_eq!(
            (pace[0]["key"].as_str(), pace[0]["until"].as_str()),
            (Some("pace:pool:codex:week"), pool["until"].as_str())
        );
    }

    #[test]
    fn load_reports_live_and_ongoing_sessions_per_account() {
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            codex_api_key: vec![crate::config::KeyEntry { api_key: "mock-only".into(), ..Default::default() }],
            ..Default::default()
        };
        let mut app = App::new(cfg.clone(), "/nonexistent/config.yaml".into());
        Arc::get_mut(&mut app).unwrap().sessions = Arc::new(crate::affinity::Sessions::memory());
        assert_eq!(account_load(&app), json!({}), "idle accounts are left out");
        let (account, _) = app.sessions.pick(&app.pool, &cfg, "gpt-6.1-sol", Some("task"), &[], None).unwrap();
        assert_eq!(account_load(&app)[&account.id], json!({ "in_flight": 0, "sessions": 1, "ongoing_sessions": 1 }));
    }

    #[tokio::test]
    async fn pinned_session_activity_preserves_missing_and_partial_usage() {
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            codex_api_key: vec![crate::config::KeyEntry { api_key: "mock-only".into(), ..Default::default() }],
            ..Default::default()
        };
        let mut app = App::new(cfg.clone(), "/nonexistent/config.yaml".into());
        Arc::get_mut(&mut app).unwrap().sessions = Arc::new(crate::affinity::Sessions::memory());
        let model = "gpt-6.1-sol";
        let mut account_id = String::new();
        for (session, cached) in [("mixed", 0), ("mixed", 40), ("missing", 0)] {
            let (account, _) = app.sessions.pick(&app.pool, &cfg, model, Some(session), &[], None).unwrap();
            account_id.clone_from(&account.id);
            let mut tracker = crate::proxy::Tracker::new(&app, crate::ir::Format::Responses, true, "http", model);
            tracker.session(Some(session), Some("thread-id"), &cfg);
            tracker.attempt(&account);
            tracker.finish(499, &crate::ir::Usage { cache_read: cached, ..Default::default() }, None);
        }
        let response = account_activity(State(app), Path(account_id)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();
        let sessions = payload["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 2);
        let mixed = sessions.iter().find(|s| s["session"] == "mixed").unwrap();
        assert_eq!((mixed["requests"].as_u64(), mixed["cache_tokens"].as_u64()), (Some(2), Some(40)));
        assert_eq!((mixed["usage_missing"].as_u64(), mixed["usage_partial"].as_u64()), (Some(1), Some(1)));
        let missing = sessions.iter().find(|s| s["session"] == "missing").unwrap();
        assert_eq!((missing["requests"].as_u64(), missing["cache_tokens"].as_u64()), (Some(1), Some(0)));
        assert_eq!((missing["usage_missing"].as_u64(), missing["usage_partial"].as_u64()), (Some(1), Some(0)));
    }

    #[tokio::test]
    async fn key_edits_fall_back_to_a_rewrite_when_formatting_cannot_be_kept() {
        let dir = std::env::temp_dir().join(format!("fusebox-edit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        let original = format!(
            "# indentless list\nauth-dir: {}\nclaude-api-key:\n- api-key: first\n  headers: {{X-Team: core}}\n",
            dir.display()
        );
        std::fs::write(&path, &original).unwrap();
        let app = App::new(Config::parse(&original).unwrap(), path.clone());
        let response = edit_config(&app, |doc| {
            crate::compat::add_key(
                doc,
                &crate::compat::NewKey {
                    group: "claude",
                    api_key: "second",
                    base_url: None,
                    models: vec![],
                    name: None,
                },
            )
        });
        assert_eq!(response.status(), StatusCode::OK);
        let cfg = Config::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.claude_api_key.len(), 2);
        assert_eq!(cfg.claude_api_key[0].headers["X-Team"], "core");
        assert_eq!(std::fs::read_to_string(dir.join("config.yaml.bak")).unwrap(), original);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn structured_saves_validate_and_reject_stale_edits() {
        let dir = std::env::temp_dir().join(format!("fusebox-settings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        let original = format!("# Keep this comment\nport: 8317 # port note\nauth-dir: {}\n", dir.display());
        std::fs::write(&path, &original).unwrap();
        let app = App::new(Config::parse(&original).unwrap(), path.clone());
        let version = crate::config_editor::revision(&original);
        let response = patch_settings(
            State(app.clone()),
            Json(SettingsBody {
                revision: version.clone(),
                changes: json!({"port": 70000}).as_object().unwrap().clone(),
                dry_run: false,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let response = patch_settings(
            State(app.clone()),
            Json(SettingsBody {
                revision: version.clone(),
                changes: json!({"port": 9000}).as_object().unwrap().clone(),
                dry_run: false,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("# Keep this comment"));
        assert!(saved.contains("# port note"));
        let response = patch_settings(
            State(app.clone()),
            Json(SettingsBody {
                revision: version,
                changes: json!({"request-retry": 4}).as_object().unwrap().clone(),
                dry_run: false,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        let response = patch_settings(
            State(app.clone()),
            Json(SettingsBody {
                revision: crate::config_editor::revision(&saved),
                changes: json!({"request-retry": 4}).as_object().unwrap().clone(),
                dry_run: false,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response: Value =
            serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap();
        assert_eq!(response["restart_fields"], json!(["port"]));
        assert_eq!(response["values"]["request-retry"], json!(4));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bcrypt_management_keys() {
        let hash = bcrypt::hash("open sesame", 4).unwrap();
        assert!(management_key_matches("open sesame", &hash));
        assert!(management_key_matches("open sesame", &hash));
        assert!(!management_key_matches("wrong", &hash));
        assert!(management_key_matches("plain", "plain"));
        assert!(!management_key_matches("plain", "other"));
    }

    #[test]
    fn a_shared_key_named_later_keeps_its_history_under_the_name() {
        let hash = crate::affinity::scope_for_key(Some("fbx_was_shared"));
        let other = crate::affinity::scope_for_key(Some("fbx_still_shared"));
        let mut cfg = Config { api_keys: vec!["fbx_still_shared".into()], ..Default::default() };
        assert_eq!(client_label(&cfg, &hash), format!("Shared key {}…", &hash[..8]));
        cfg.named_clients = vec![crate::config::NamedClient {
            id: "macbook".into(),
            label: "MacBook Pro".into(),
            key: "fbx_was_shared".into(),
            ..Default::default()
        }];
        assert_eq!(client_label(&cfg, "macbook"), "MacBook Pro");
        assert_eq!(client_label(&cfg, &hash), "MacBook Pro (before naming)");
        assert_eq!(client_label(&cfg, &format!("key:{}", &hash[..12])), "MacBook Pro (before naming)");
        assert_eq!(client_label(&cfg, &other), format!("Shared key {}…", &other[..8]));
        assert_eq!(client_label(&cfg, "key:"), "Shared key …", "an empty prefix names nobody");
    }
}
