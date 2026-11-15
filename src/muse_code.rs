//! Muse Code's own endpoints, which it expects beside `/v1` on the host it is pointed at.
//!
//! The CLI fetches its model catalog from `GET /muse-code/models` when it starts, and
//! exits on a 404 once its cached copy is stale or of an older format. It also reads
//! `GET /muse-code/config` and sends its web searches to `POST /muse-code/search`. Each is
//! passed to Meta with a Meta account's key and answered with Meta's own reply, so the
//! catalog is always the one this version of the CLI understands.
//!
//! Only these three are passed on: `/muse-code/key`, which mints keys, never is. They are
//! not model requests: nothing is logged as one, and no reply pauses an account or is
//! held against it.

use std::sync::Arc;

use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderName, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::accounts::{Account, Credential, Provider};
use crate::config::NamedClient;
use crate::formats;
use crate::ir::Format;
use crate::state::App;

/// The endpoints passed on, each with its one method.
const ENDPOINTS: [(&str, Method); 3] = [("models", Method::GET), ("config", Method::GET), ("search", Method::POST)];
/// What the client may tell Meta about its request, besides the body.
const REQUEST_HEADERS: [HeaderName; 3] = [header::ACCEPT, header::CONTENT_TYPE, header::IF_NONE_MATCH];
/// What of Meta's reply reaches the client, besides the body.
const REPLY_HEADERS: [HeaderName; 5] =
    [header::CONTENT_TYPE, header::CACHE_CONTROL, header::ETAG, header::LAST_MODIFIED, header::RETRY_AFTER];
/// No catalog or search reply comes near this.
const MAX_REPLY: usize = 16 << 20;
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// How long a client waits at most, however many accounts are asked.
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(90);

fn error(status: u16, message: &str) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    (code, axum::Json(formats::error_body(Format::Responses, status, message))).into_response()
}

/// The key to call Meta with, and where: the account's base URL without its `/v1`.
fn target(acct: &Account) -> (String, String) {
    let (key, base) = match &*acct.cred.read() {
        Credential::OAuth(o) => (o.access_token.clone(), o.base_url.clone()),
        Credential::ApiKey { key, base_url } => (key.clone(), base_url.clone()),
    };
    let base = base.filter(|b| !b.trim().is_empty()).unwrap_or_else(|| crate::device::meta::API_BASE.into());
    let base = base.trim_end_matches('/');
    (key, base.strip_suffix("/v1").unwrap_or(base).to_string())
}

/// The client's query without its Fusebox key, which may travel as `key=`.
fn forwarded_query(query: Option<&str>) -> String {
    let pairs = url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()).filter(|(name, _)| name != "key");
    url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish()
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl IntoResponse for Reply {
    fn into_response(self) -> Response {
        let mut response = (self.status, self.body).into_response();
        response.headers_mut().remove(header::CONTENT_TYPE);
        for name in REPLY_HEADERS {
            if let Some(value) = self.headers.get(&name) {
                response.headers_mut().insert(name, value.clone());
            }
        }
        response
    }
}

async fn send(
    app: &App,
    acct: &Account,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<Reply, String> {
    let (key, origin) = target(acct);
    let mut request = app
        .http
        .for_account(acct)
        .request(method.clone(), format!("{origin}{path}"))
        .timeout(TIMEOUT)
        .bearer_auth(key)
        .header(header::USER_AGENT, crate::device::meta::API_UA)
        .header("x-client-id", "tbh:tui");
    for name in REQUEST_HEADERS {
        if let Some(value) = headers.get(&name) {
            request = request.header(name, value.clone());
        }
    }
    if *method == Method::POST {
        request = request.body(body.clone());
    }
    let mut response = request.send().await.map_err(|e| format!("could not reach Meta: {}", e.without_url()))?;
    let (status, reply_headers) = (response.status(), response.headers().clone());
    let mut reply = Vec::new();
    while let Some(chunk) =
        response.chunk().await.map_err(|e| format!("Meta's reply broke off: {}", e.without_url()))?
    {
        if reply.len() + chunk.len() > MAX_REPLY {
            return Err("Meta's reply is too large".into());
        }
        reply.extend_from_slice(&chunk);
    }
    Ok(Reply { status, headers: reply_headers, body: reply.into() })
}

/// Passes one Muse Code endpoint on to Meta. `client` is the named client asking, if one.
pub(crate) async fn relay(
    app: &Arc<App>,
    client: Option<&NamedClient>,
    method: Method,
    endpoint: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    if !ENDPOINTS.iter().any(|(name, allowed)| *name == endpoint && *allowed == method) {
        return error(404, "unknown Muse Code endpoint");
    }
    // A key limited to other models has no business with a Meta account. The models are
    // the ones the Meta accounts list, under the names clients use for them.
    let allowed = client.is_none_or(|c| {
        let listed = app.pool.models();
        c.models.is_empty()
            || listed.iter().any(|(model, by)| *by == Provider::Meta && crate::policy::model_allowed(c, model))
    });
    if !allowed {
        return error(403, "this key can't use Muse models");
    }
    let query = forwarded_query(query);
    let path =
        if query.is_empty() { format!("/muse-code/{endpoint}") } else { format!("/muse-code/{endpoint}?{query}") };
    // One limit for all of it: the CLI waits on its catalog before it starts.
    match tokio::time::timeout(DEADLINE, ask(app, &method, &path, headers, &body)).await {
        Ok(response) => response,
        Err(_) => error(504, "Meta did not answer in time"),
    }
}

/// Asks the Meta accounts in turn, until one answers.
async fn ask(app: &Arc<App>, method: &Method, path: &str, headers: &HeaderMap, body: &Bytes) -> Response {
    let mut last = None;
    for acct in app.pool.all().into_iter().filter(|a| a.provider == Provider::Meta) {
        if acct.state.lock().disabled || acct.held_out() || crate::oauth::ensure_ready(app, &acct).await.is_err() {
            continue;
        }
        let mut renewed = false;
        loop {
            match send(app, &acct, method, path, headers, body).await {
                // A refused key is minted again once; another account may still answer.
                Ok(reply) if reply.status == StatusCode::UNAUTHORIZED => {
                    let renew = !renewed && acct.is_oauth();
                    renewed = true;
                    if renew && crate::oauth::ensure_fresh(app, &acct, chrono::Duration::minutes(5), true).await.is_ok()
                    {
                        continue;
                    }
                    last = Some(reply.into_response());
                }
                Ok(reply) => return reply.into_response(),
                Err(problem) => last = Some(error(502, &problem)),
            }
            break;
        }
    }
    last.unwrap_or_else(|| error(404, "no Meta account is available for Muse Code"))
}

#[cfg(test)]
mod tests {
    use axum::extract::State;
    use parking_lot::Mutex;
    use serde_json::{Value, json};

    use super::*;
    use crate::config::{Config, KeyEntry, NamedClient};
    use crate::routing_tests::{Server, serve};

    const CATALOG: &str = r#"{"schema_version":3, "rows":[{"model_id":"muse-ember-1.0"}]}"#;

    /// What a stand-in for Meta was asked: the method, path and query, headers and body.
    type Asked = Arc<Mutex<Vec<(String, String, HeaderMap, String)>>>;

    async fn meta(State(asked): State<Asked>, request: axum::extract::Request) -> Response {
        let (parts, body) = request.into_parts();
        let body = String::from_utf8(axum::body::to_bytes(body, usize::MAX).await.unwrap().to_vec()).unwrap();
        let target = parts.uri.path_and_query().unwrap().to_string();
        asked.lock().push((parts.method.to_string(), target.clone(), parts.headers.clone(), body.clone()));
        if parts.headers.get(header::AUTHORIZATION).unwrap() == "Bearer stale" {
            let refused = json!({"error":{"code":"invalid_api_key","message":"Invalid API key"}});
            return (StatusCode::UNAUTHORIZED, axum::Json(refused)).into_response();
        }
        match parts.uri.path() {
            "/muse-code/models" => (
                [
                    (header::CONTENT_TYPE, "application/json; charset=utf-8"),
                    (header::ETAG, "\"catalog-3\""),
                    (header::SET_COOKIE, "session=meta"),
                ],
                CATALOG,
            )
                .into_response(),
            "/muse-code/config" => axum::Json(json!({"asked": target})).into_response(),
            "/muse-code/search" => (StatusCode::CREATED, body).into_response(),
            _ => StatusCode::IM_A_TEAPOT.into_response(),
        }
    }

    struct Fixture {
        app: Arc<App>,
        proxy: Server,
        _meta: Server,
        asked: Asked,
    }

    /// A proxy with one Meta key per name in `keys`, all at a stand-in for Meta.
    async fn fixture(keys: &[&str], clients: Vec<NamedClient>) -> Fixture {
        let asked = Asked::default();
        let meta = serve(axum::Router::new().fallback(meta).with_state(asked.clone())).await;
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            api_keys: vec!["client-one".into()],
            named_clients: clients,
            meta_api_key: keys
                .iter()
                .map(|key| KeyEntry {
                    api_key: key.to_string(),
                    base_url: Some(format!("{}/v1", meta.url)),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let app = App::new(cfg, std::env::temp_dir().join(format!("fusebox-muse-{}.yaml", uuid::Uuid::new_v4())));
        let proxy = serve(crate::server::router(app.clone())).await;
        Fixture { app, proxy, _meta: meta, asked }
    }

    impl Fixture {
        async fn get(&self, path: &str, key: &str) -> reqwest::Response {
            reqwest::Client::new().get(format!("{}{path}", self.proxy.url)).bearer_auth(key).send().await.unwrap()
        }
    }

    #[test]
    fn the_fusebox_key_never_travels_on_in_the_query() {
        assert_eq!(forwarded_query(None), "");
        assert_eq!(forwarded_query(Some("channel=public")), "channel=public");
        assert_eq!(forwarded_query(Some("key=client-secret&channel=public&a=b+c")), "channel=public&a=b+c");
        assert_eq!(forwarded_query(Some("key=client-secret")), "");
    }

    /// Muse Code exits at startup when `/muse-code/models` answers 404. It gets Meta's own
    /// catalog, byte for byte, fetched with the Meta account's key and never the client's.
    #[tokio::test]
    async fn muse_code_gets_metas_catalog_with_the_accounts_key() {
        let fixture = fixture(&["meta-key"], Vec::new()).await;
        let response = fixture.get("/muse-code/models", "client-one").await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json; charset=utf-8");
        assert_eq!(response.headers()[header::ETAG], "\"catalog-3\"");
        assert!(response.headers().get(header::SET_COOKIE).is_none(), "only named reply headers pass");
        assert_eq!(response.text().await.unwrap(), CATALOG);

        let asked = fixture.asked.lock().clone();
        let (method, target, headers, _) = &asked[0];
        assert_eq!((method.as_str(), target.as_str(), asked.len()), ("GET", "/muse-code/models", 1));
        assert_eq!(headers[header::AUTHORIZATION], "Bearer meta-key");
        assert_eq!(headers[header::USER_AGENT], crate::device::meta::API_UA);
        assert_eq!(headers["x-client-id"], "tbh:tui");
        assert!(!format!("{headers:?}").contains("client-one"), "{headers:?}");

        // Nothing about it is held against the account.
        let account = fixture.app.pool.all().remove(0);
        let st = account.state.lock();
        assert!(st.cooldowns.is_empty() && st.last_error(chrono::Utc::now()).is_none());
    }

    #[tokio::test]
    async fn settings_and_searches_pass_too_without_the_clients_key() {
        let fixture = fixture(&["meta-key"], Vec::new()).await;
        // A key in the query is the client's Fusebox key: it stays here.
        let url = format!("{}/muse-code/config?channel=public&key=client-one", fixture.proxy.url);
        let response = reqwest::get(url).await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.json::<Value>().await.unwrap()["asked"], "/muse-code/config?channel=public");

        let search = r#"{"query":"rust axum fallback"}"#;
        let response = reqwest::Client::new()
            .post(format!("{}/muse-code/search", fixture.proxy.url))
            .bearer_auth("client-one")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, "private=1")
            .body(search)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201, "Meta's own status");
        assert_eq!(response.text().await.unwrap(), search);
        let asked = fixture.asked.lock().clone();
        let (method, target, headers, body) = &asked[1];
        assert_eq!((method.as_str(), target.as_str(), body.as_str()), ("POST", "/muse-code/search", search));
        assert_eq!(headers[header::CONTENT_TYPE], "application/json");
        assert!(headers.get(header::COOKIE).is_none(), "only named request headers pass");
        assert!(asked.iter().all(|(_, target, headers, _)| !format!("{target} {headers:?}").contains("client-one")));
    }

    /// Only the three endpoints, each with its method, and only for a client with a key.
    #[tokio::test]
    async fn nothing_else_under_muse_code_reaches_meta() {
        let fixture = fixture(&["meta-key"], Vec::new()).await;
        // The endpoint that mints keys, above all.
        for path in ["/muse-code/key", "/muse-code/browser_open", "/muse-code/search"] {
            assert_eq!(fixture.get(path, "client-one").await.status(), 404, "{path}");
        }
        let client = reqwest::Client::new();
        for path in ["/muse-code/key", "/muse-code/models", "/muse-code/config"] {
            let response = client.post(format!("{}{path}", fixture.proxy.url)).bearer_auth("client-one").body("{}");
            assert_eq!(response.send().await.unwrap().status(), 404, "{path}");
        }
        assert_eq!(fixture.get("/muse-code/models", "not-a-key").await.status(), 401);
        assert!(fixture.asked.lock().is_empty());

        // With no Meta account there is nothing to ask.
        let empty = self::fixture(&[], Vec::new()).await;
        let response = empty.get("/muse-code/models", "client-one").await;
        assert_eq!(response.status(), 404);
        let body: Value = response.json().await.unwrap();
        assert!(body["error"]["message"].as_str().unwrap().contains("no Meta account"), "{body}");
    }

    /// A key Meta refuses doesn't end it while another account's key is good, and pauses
    /// nothing; with no other, Meta's own refusal is the answer. A disabled account is
    /// not asked.
    #[tokio::test]
    async fn a_refused_key_moves_on_to_the_next_account() {
        let fixture = fixture(&["stale", "meta-key"], Vec::new()).await;
        let response = fixture.get("/muse-code/models", "client-one").await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), CATALOG);
        let keys = |asked: &Asked| -> Vec<String> {
            asked.lock().iter().map(|(_, _, h, _)| h[header::AUTHORIZATION].to_str().unwrap().to_string()).collect()
        };
        assert_eq!(keys(&fixture.asked), ["Bearer stale", "Bearer meta-key"]);
        for account in fixture.app.pool.all() {
            let st = account.state.lock();
            assert!(st.cooldowns.is_empty() && st.last_error(chrono::Utc::now()).is_none(), "{}", account.id);
        }

        // The pool keeps the keys in their order: the second is the good one.
        fixture.app.pool.all()[1].state.lock().disabled = true;
        fixture.asked.lock().clear();
        let response = fixture.get("/muse-code/models", "client-one").await;
        assert_eq!(response.status(), 401);
        assert_eq!(response.json::<Value>().await.unwrap()["error"]["code"], "invalid_api_key");
        assert_eq!(keys(&fixture.asked), ["Bearer stale"]);
    }

    /// A key limited to other models has no use of a Meta account.
    #[tokio::test]
    async fn a_client_limited_to_other_models_is_refused() {
        let client = |id: &str, models: &str| NamedClient {
            id: id.into(),
            label: id.into(),
            key: format!("fbx_{id}"),
            models: vec![models.into()],
            ..Default::default()
        };
        let fixture = fixture(&["meta-key"], vec![client("claude", "claude-*"), client("muse", "muse-*")]).await;
        let response = fixture.get("/muse-code/models", "fbx_claude").await;
        assert_eq!(response.status(), 403);
        assert!(fixture.asked.lock().is_empty());
        assert_eq!(fixture.get("/muse-code/models", "fbx_muse").await.status(), 200);

        // A Meta model under another name counts by that name, as it does for requests.
        let mut cfg = (*fixture.app.cfg()).clone();
        cfg.meta_api_key[0].models =
            vec![crate::config::ModelAlias { name: "muse-spark-1.3".into(), alias: Some("coding".into()) }];
        cfg.named_clients.push(client("alias", "coding"));
        fixture.app.set_config(cfg);
        assert_eq!(fixture.get("/muse-code/models", "fbx_alias").await.status(), 200);
        assert_eq!(
            fixture.get("/muse-code/models", "fbx_muse").await.status(),
            403,
            "no Muse model is left by that name"
        );
    }
}
