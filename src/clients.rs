//! The client releases Fusebox speaks as.
//!
//! Requests Fusebox builds for other clients (translated requests, cloaking) and its
//! own small requests (usage, model lists, limit resets) claim the newest release it
//! knows of each client: the built-in one (the constants listed in
//! `client-versions.json`, whose separate `reviewed` baselines drive a weekly
//! compatibility review), a newer one set in `client-identities`, or, for Claude Code and Codex,
//! the newest one the user's real clients sent through Fusebox. A version-only
//! release therefore needs no Fusebox release for anyone who runs the real client
//! through it. Claude Code's runtime headers (package and Node versions, OS and
//! architecture) are learned the same way, so cloaked requests look like the
//! user's own installation. Real clients passing through keep their own identity.
//! A release that changes the request shape still needs a code change; the
//! identity tests in `upstream` catch that.

use std::collections::BTreeMap;

use axum::http::HeaderMap;
use parking_lot::RwLock;

use crate::upstream::{CC_USER_AGENT, CC_VERSION, CODEX_USER_AGENT, CODEX_VERSION};

pub const CLAUDE_CODE: &str = "claude-code";
pub const CODEX: &str = "codex";
pub const GROK: &str = "grok-build";
pub const DEVIN: &str = "devin-cli";
pub const ANTIGRAVITY: &str = "antigravity";
/// The clients `client-identities` can name. Muse Code's identity includes the
/// build's commit hash, so a version number alone can't stand in for it.
pub const CONFIGURABLE: [&str; 5] = [CLAUDE_CODE, CODEX, GROK, DEVIN, ANTIGRAVITY];

/// Claude Code's runtime headers, and what the cloak sends before any is learned.
const PROFILE: [(&str, &str); 4] = [
    ("x-stainless-package-version", "0.128.0"),
    ("x-stainless-os", "MacOS"),
    ("x-stainless-arch", "arm64"),
    ("x-stainless-runtime-version", "v26.3.0"),
];

#[derive(Default)]
struct State {
    /// Newest version seen per client, never older than the built-in one.
    seen: BTreeMap<&'static str, String>,
    /// From `client-identities`.
    configured: BTreeMap<&'static str, String>,
    /// Claude Code's runtime headers from its newest release seen, with that version.
    profile: Option<(String, Vec<String>)>,
}

#[cfg(not(test))]
static STATE: std::sync::LazyLock<RwLock<State>> = std::sync::LazyLock::new(Default::default);

// Tests run in parallel and each checks exact identities, so each test thread
// learns on its own.
#[cfg(test)]
thread_local! {
    static STATE: &'static RwLock<State> = Box::leak(Box::default());
}

fn state() -> &'static RwLock<State> {
    #[cfg(not(test))]
    return &STATE;
    #[cfg(test)]
    return STATE.with(|s| *s);
}

fn parse(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.trim().splitn(3, '.');
    let n = |s: Option<&str>| s?.parse::<u32>().ok();
    let (a, b) = (n(it.next())?, n(it.next())?);
    // `0.161.0`, but not `0.161.0-alpha.2`: pre-releases are nobody's default.
    let c = n(it.next())?;
    Some((a, b, c))
}

/// Newer than the built-in release and close enough to it to be real: same major
/// version, at most 20 minor versions ahead.
fn plausible(builtin: &str, v: &str) -> bool {
    match (parse(builtin), parse(v)) {
        (Some(b), Some(v)) => v > b && v.0 == b.0 && v.1 <= b.1 + 20,
        _ => false,
    }
}

fn builtin(client: &str) -> &'static str {
    match client {
        CODEX => CODEX_VERSION,
        GROK => crate::device::xai::CLIENT_VERSION,
        DEVIN => crate::devin::CLIENT_VERSION,
        ANTIGRAVITY => crate::antigravity::FALLBACK_VERSION,
        _ => CC_VERSION,
    }
}

fn id(client: &str) -> Option<&'static str> {
    CONFIGURABLE.into_iter().find(|c| c.eq_ignore_ascii_case(client.trim()))
}

/// Checks a `client-identities` entry: a known client and a plain `x.y.z` version.
pub fn validate(client: &str, version: &str) -> Result<(), String> {
    let Some(id) = id(client) else {
        return Err(format!("client-identities: unknown client {client}; use one of {}", CONFIGURABLE.join(", ")));
    };
    if parse(version).is_none() {
        return Err(format!("client-identities: {id} needs a version like {}", builtin(id)));
    }
    Ok(())
}

/// Applies `client-identities`. Entries older than the built-in release are kept
/// but never win, so a forgotten entry can't hold Fusebox back.
pub fn configure(identities: &BTreeMap<String, String>) {
    let configured = identities
        .iter()
        .filter_map(|(client, version)| Some((id(client)?, version.trim().to_string())))
        .filter(|(_, version)| parse(version).is_some())
        .collect();
    state().write().configured = configured;
}

/// The release Fusebox claims for a client: the newest of the built-in one, the
/// configured one and the newest seen.
pub fn version(client: &'static str) -> String {
    newest(client, builtin(client).to_string())
}

/// `current`, or a newer configured or seen release of the client.
pub fn newest(client: &'static str, current: String) -> String {
    let state = state().read();
    let mut best = current;
    for candidate in [state.configured.get(client), state.seen.get(client)].into_iter().flatten() {
        if parse(candidate) > parse(&best) {
            best = candidate.clone();
        }
    }
    best
}

/// The version of a request's client, when it is Claude Code or Codex.
fn version_of(headers: &HeaderMap) -> Option<(&'static str, String)> {
    let get = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).unwrap_or_default();
    let agent = get("user-agent");
    let (name, rest) = agent.split_once('/')?;
    let version = rest.split([' ', '(', ';']).next().unwrap_or_default().to_string();
    match name.to_ascii_lowercase().as_str() {
        "claude-cli" => Some((CLAUDE_CODE, version)),
        n if n.starts_with("codex") && get("originator").starts_with("codex") => Some((CODEX, version)),
        _ => None,
    }
}

/// A header value worth repeating: short, plain and printable.
fn sane(v: &str) -> bool {
    !v.is_empty() && v.len() <= 32 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
}

/// Remembers a newer Claude Code or Codex release from a client's request, and
/// Claude Code's runtime headers from its newest release. True when that changed
/// what Fusebox claims (worth saving).
pub fn observe(headers: &HeaderMap) -> bool {
    let Some((client, version)) = version_of(headers) else { return false };
    let mut changed = false;
    if plausible(builtin(client), &version)
        && state().read().seen.get(client).is_none_or(|seen| parse(seen) < parse(&version))
    {
        tracing::info!("{client} {version} seen; Fusebox's own and cloaked requests now say so");
        state().write().seen.insert(client, version.clone());
        changed = true;
    }
    if client == CLAUDE_CODE {
        changed |= learn_profile(headers, &version);
    }
    changed
}

fn learn_profile(headers: &HeaderMap, version: &str) -> bool {
    // Only the release the cloak claims (or a newer one) may describe it.
    if parse(version) < parse(&self::version(CLAUDE_CODE)) {
        return false;
    }
    let values: Option<Vec<String>> = PROFILE
        .iter()
        .map(|(name, _)| headers.get(*name).and_then(|v| v.to_str().ok()).filter(|v| sane(v)).map(String::from))
        .collect();
    let Some(values) = values else { return false };
    let mut state = state().write();
    if state.profile.as_ref().is_some_and(|(v, old)| parse(v) > parse(version) || (v == version && *old == values)) {
        return false;
    }
    state.profile = Some((version.to_string(), values));
    true
}

/// Claude Code's runtime headers for cloaked requests.
pub fn claude_profile() -> Vec<(&'static str, String)> {
    let state = state().read();
    match &state.profile {
        Some((_, values)) => PROFILE.iter().zip(values).map(|((name, _), v)| (*name, v.clone())).collect(),
        None => PROFILE.iter().map(|(name, v)| (*name, v.to_string())).collect(),
    }
}

/// What to save, so a restart keeps claiming the newest release.
pub fn seen() -> BTreeMap<String, String> {
    let state = state().read();
    let mut out: BTreeMap<String, String> = state.seen.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    if let Some((version, values)) = &state.profile {
        out.insert(format!("{CLAUDE_CODE}/profile"), version.clone());
        for ((name, _), value) in PROFILE.iter().zip(values) {
            out.insert(format!("{CLAUDE_CODE}/{name}"), value.clone());
        }
    }
    out
}

pub fn restore(saved: &BTreeMap<String, String>) {
    let mut state = state().write();
    for client in [CLAUDE_CODE, CODEX] {
        if let Some(v) = saved.get(client).filter(|v| plausible(builtin(client), v))
            && state.seen.get(client).is_none_or(|old| parse(old) < parse(v))
        {
            state.seen.insert(client, v.clone());
        }
    }
    if state.profile.is_none()
        && let Some(version) = saved.get(&format!("{CLAUDE_CODE}/profile"))
    {
        let values: Option<Vec<String>> = PROFILE
            .iter()
            .map(|(name, _)| saved.get(&format!("{CLAUDE_CODE}/{name}")).filter(|v| sane(v)).cloned())
            .collect();
        state.profile = values.map(|values| (version.clone(), values));
    }
}

/// Claude Code's user agent, for Fusebox's own requests to Anthropic and for cloaking.
pub fn claude_user_agent() -> String {
    CC_USER_AGENT.replace(CC_VERSION, &version(CLAUDE_CODE))
}

/// For Fusebox's own and cloaked requests to the Codex backend: `client_version` and `version`.
pub fn codex_version() -> String {
    version(CODEX)
}

pub fn codex_user_agent() -> String {
    CODEX_USER_AGENT.replace(CODEX_VERSION, &version(CODEX))
}

/// A real Codex client: its own user agent and originator go upstream unchanged.
pub fn is_codex(headers: &HeaderMap) -> bool {
    matches!(version_of(headers), Some((CODEX, _)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `.github/client-versions.json` is what the daily check compares; the code must agree.
    #[test]
    fn the_version_list_matches_the_code() {
        let list: serde_json::Value = serde_json::from_str(include_str!("../.github/client-versions.json")).unwrap();
        let mut ids = Vec::new();
        for client in list["clients"].as_array().unwrap() {
            let (id, v) = (client["id"].as_str().unwrap(), client["version"].as_str().unwrap());
            ids.push(id);
            let in_code = match id {
                "claude-code" => CC_VERSION == v && CC_USER_AGENT.contains(&format!("/{v} ")),
                "codex" => CODEX_VERSION == v && CODEX_USER_AGENT.matches(v).count() == 2,
                "grok-build" => crate::device::xai::CLIENT_VERSION == v,
                "muse-code" => crate::device::meta::API_UA.starts_with(&format!("muse-build/{v} ")),
                "devin-cli" => crate::devin::CLIENT_VERSION == v,
                "antigravity" => crate::antigravity::FALLBACK_VERSION == v,
                other => panic!("{other} has no check here"),
            };
            assert!(in_code, "{id} {v} in client-versions.json differs from the code");
        }
        assert_eq!(ids.len(), 6);
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn clients_are_recognised_by_their_own_headers() {
        let cc = headers(&[("user-agent", "claude-cli/2.9.1 (external, cli)")]);
        assert_eq!(version_of(&cc), Some((CLAUDE_CODE, "2.9.1".into())));
        let codex = headers(&[
            ("user-agent", "codex_cli_rs/0.170.2 (Mac OS 26.5.2; arm64) iTerm.app/3.6.11"),
            ("originator", "codex_cli_rs"),
        ]);
        assert_eq!(version_of(&codex), Some((CODEX, "0.170.2".into())));
        assert!(is_codex(&codex));
        // A Codex-looking agent without Codex's originator is some other program.
        assert!(!is_codex(&headers(&[("user-agent", "codex_cli_rs/0.170.2")])));
        assert_eq!(version_of(&headers(&[("user-agent", "curl/8.9.0")])), None);
    }

    #[test]
    fn only_plausible_newer_releases_count() {
        let (major, minor, _) = parse(CODEX_VERSION).unwrap();
        assert!(plausible(CODEX_VERSION, &format!("{major}.{}.0", minor + 1)));
        assert!(plausible(CODEX_VERSION, &format!("{major}.{}.3", minor + 20)));
        for no in [
            CODEX_VERSION.to_string(),
            format!("{major}.{}.0", minor - 1),
            format!("{major}.{}.0", minor + 21),
            format!("{}.0.0", major + 1),
            format!("{major}.{}.0-alpha.1", minor + 1),
            "garbage".into(),
        ] {
            assert!(!plausible(CODEX_VERSION, &no), "{no}");
        }
    }

    #[test]
    fn the_newest_client_release_is_claimed_for_fusebox_requests() {
        let (major, minor, _) = parse(CC_VERSION).unwrap();
        let newer = format!("{major}.{minor}.9999");
        assert!(!observe(&headers(&[("user-agent", &format!("claude-cli/{major}.{}.0 (x)", minor + 21))])));
        assert!(observe(&headers(&[("user-agent", &format!("claude-cli/{newer} (external, cli)"))])));
        assert!(!observe(&headers(&[("user-agent", &format!("claude-cli/{newer} (external, cli)"))])), "already known");
        assert_eq!(claude_user_agent(), CC_USER_AGENT.replace(CC_VERSION, &newer));
        assert_eq!(seen().get(CLAUDE_CODE), Some(&newer));
        restore(&BTreeMap::from([(CLAUDE_CODE.to_string(), CC_VERSION.to_string())]));
        assert_eq!(seen().get(CLAUDE_CODE), Some(&newer), "an older saved release changes nothing");
    }

    #[test]
    fn configured_identities_win_only_when_newer() {
        let (major, minor, patch) = parse(crate::devin::CLIENT_VERSION).unwrap();
        let ahead = format!("{major}.{minor}.{}", patch + 1);
        configure(&BTreeMap::from([
            ("devin-cli".to_string(), ahead.clone()),
            ("Codex".to_string(), "0.1.0".to_string()),
            ("unknown".to_string(), "9.9.9".to_string()),
        ]));
        assert_eq!(version(DEVIN), ahead);
        assert_eq!(version(CODEX), CODEX_VERSION, "an older configured release never wins");
        assert_eq!(version(GROK), crate::device::xai::CLIENT_VERSION);
        assert!(validate("unknown", "1.0.0").is_err());
        assert!(validate("claude-code", "2.1").is_err());
        assert!(validate("antigravity", "2.30.0").is_ok());
        // A configured Claude Code release reaches the cloak's user agent and billing version.
        let (major, minor, _) = parse(CC_VERSION).unwrap();
        let configured = format!("{major}.{minor}.9000");
        configure(&BTreeMap::from([("claude-code".to_string(), configured.clone())]));
        assert_eq!(claude_user_agent(), CC_USER_AGENT.replace(CC_VERSION, &configured));
        configure(&BTreeMap::new());
        assert_eq!(version(DEVIN), crate::devin::CLIENT_VERSION);
    }

    #[test]
    fn claude_codes_runtime_headers_are_learned_from_its_newest_release() {
        assert_eq!(claude_profile()[1], ("x-stainless-os", "MacOS".to_string()));
        let linux = |version: &str, runtime: &str| {
            headers(&[
                ("user-agent", &format!("claude-cli/{version} (external, cli)")),
                ("x-stainless-package-version", "0.131.0"),
                ("x-stainless-os", "Linux"),
                ("x-stainless-arch", "x64"),
                ("x-stainless-runtime-version", runtime),
            ])
        };
        // An older release than the one claimed describes nothing.
        assert!(!observe(&linux("2.0.1", "v24.1.0")));
        assert!(observe(&linux(CC_VERSION, "v24.11.1")));
        let profile = claude_profile();
        assert_eq!(profile[1], ("x-stainless-os", "Linux".to_string()));
        assert_eq!(profile[3], ("x-stainless-runtime-version", "v24.11.1".to_string()));
        assert!(!observe(&linux(CC_VERSION, "v24.11.1")), "nothing new");
        // Odd values are not repeated upstream.
        assert!(!observe(&headers(&[
            ("user-agent", &format!("claude-cli/{CC_VERSION} (external, cli)")),
            ("x-stainless-package-version", "0.131.0"),
            ("x-stainless-os", "Linux; rm -rf"),
            ("x-stainless-arch", "x64"),
            ("x-stainless-runtime-version", "v24.11.1"),
        ])));
        let saved = seen();
        assert_eq!(saved.get("claude-code/x-stainless-arch"), Some(&"x64".to_string()));
        state().write().profile = None;
        restore(&saved);
        assert_eq!(claude_profile(), profile, "a restart keeps the learned headers");
    }
}
