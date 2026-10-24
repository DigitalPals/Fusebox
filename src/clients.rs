//! The client releases Fusebox speaks as.
//!
//! Requests Fusebox builds for other clients (translated requests, cloaking) and its
//! own small requests (usage, model lists, limit resets) claim the newest release it
//! knows of each client: the built-in one (the constants listed in
//! `client-versions.json`, whose separate `reviewed` baselines drive a weekly
//! compatibility review), a newer one set in `client-identities`, or, for Claude Code and Codex,
//! the newest plausible one an explicitly trusted named client sent through Fusebox. A version-only
//! release therefore needs no Fusebox release for anyone who runs the real client
//! through it. Claude Code's runtime headers (package and Node versions, OS and
//! architecture) are learned the same way, so cloaked requests look like the
//! user's own installation. Native recognition only classifies a request; it
//! never grants permission to teach Fusebox an identity.
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

const PROVENANCE: &str = "trusted-named-client-v1";
const CLAUDE_PATCH_ADVANCE: u32 = 100;
const CODEX_MINOR_ADVANCE: u32 = 20;
const CODEX_PATCH_ADVANCE: u32 = 100;

#[derive(Clone, PartialEq, Eq)]
struct Observation {
    version: String,
    source: String,
}

struct Profile {
    observation: Observation,
    values: Vec<String>,
}

#[derive(Default)]
struct State {
    /// Newest plausible version from an explicitly trusted named client.
    seen: BTreeMap<&'static str, Observation>,
    /// From `client-identities`.
    configured: BTreeMap<&'static str, String>,
    /// Claude Code's runtime headers from its newest release seen, with that version.
    profile: Option<Profile>,
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
    if v.is_empty() || v.len() > 32 {
        return None;
    }
    let mut it = v.split('.');
    let n = |s: Option<&str>| {
        let s = s?;
        if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse::<u32>().ok()
    };
    let (a, b) = (n(it.next())?, n(it.next())?);
    // `0.161.0`, but not `0.161.0-alpha.2`: pre-releases are nobody's default.
    let c = n(it.next())?;
    it.next().is_none().then_some((a, b, c))
}

/// Bounds are anchored to the bundled release, never to a previously learned
/// release or an operator override. Learning cannot walk an identity forward
/// indefinitely. Claude advances patches; Codex advances minor versions.
fn plausible(client: &str, builtin: &str, version: &str) -> bool {
    let (Some(b), Some(v)) = (parse(builtin), parse(version)) else { return false };
    if v < b || v.0 != b.0 {
        return false;
    }
    match client {
        CLAUDE_CODE => v.1 == b.1 && v.2.checked_sub(b.2).is_some_and(|n| n <= CLAUDE_PATCH_ADVANCE),
        CODEX => {
            v.1.checked_sub(b.1).is_some_and(|n| n <= CODEX_MINOR_ADVANCE)
                && v.2.saturating_sub(b.2) <= CODEX_PATCH_ADVANCE
        }
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
    if parse(version.trim()).is_none() {
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
    let mut state = state().write();
    state.configured = configured;
    discard_stale_profile(&mut state);
}

/// The release Fusebox claims for a client: the newest of the built-in one, the
/// configured one and the newest seen.
pub fn version(client: &'static str) -> String {
    newest(client, builtin(client).to_string())
}

/// `current`, or a newer configured or seen release of the client.
pub fn newest(client: &'static str, current: String) -> String {
    let state = state().read();
    newest_in(&state, client, current)
}

fn newest_in(state: &State, client: &'static str, current: String) -> String {
    let mut best = current;
    for candidate in
        [state.configured.get(client), state.seen.get(client).map(|seen| &seen.version)].into_iter().flatten()
    {
        if parse(candidate) > parse(&best) {
            best = candidate.clone();
        }
    }
    best
}

/// Identity headers must be single-valued. Ambiguous duplicate fields must not
/// classify a request differently from the values forwarded upstream.
fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

fn attribution(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.trim() == value
        && value.bytes().all(|b| (b' '..=b'~').contains(&b) && !b"/();,".contains(&b))
}

fn codex_product(name: &str) -> bool {
    matches!(name, "codex_cli_rs" | "codex-tui" | "codex_vscode" | "codex_atlas" | "codex_chatgpt_desktop")
        || name.starts_with("Codex ")
}

/// App-server attribution is `(client name; client version)`. A desktop/editor
/// version is independent of the Codex engine release in the UA and `version`.
fn app_attribution(tail: &str) -> Result<Option<(&str, &str)>, ()> {
    let Some((_, suffix)) = tail.rsplit_once(" (") else { return Ok(None) };
    let Some(suffix) = suffix.strip_suffix(')') else { return Ok(None) };
    let Some((name, version)) = suffix.split_once(';') else { return Ok(None) };
    let Some(version) = version.strip_prefix(' ') else { return Err(()) };
    // App versions need not use the engine's release scheme (for example a
    // desktop date/build number or an editor prerelease).
    if !attribution(name)
        || version.is_empty()
        || version.len() > 64
        || !version.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_+".contains(&b))
    {
        return Err(());
    }
    Ok(Some((name, version)))
}

fn valid_tail(tail: &str) -> bool {
    if tail.is_empty() {
        return true;
    }
    if !tail.starts_with('(') || tail.trim() != tail {
        return false;
    }
    let mut depth = 0u32;
    for b in tail.bytes() {
        match b {
            b'(' => depth += 1,
            b')' if depth == 0 => return false,
            b')' => depth -= 1,
            _ => (),
        }
    }
    depth == 0
}

/// The version of a request's client, when it is Claude Code or Codex.
fn version_of(headers: &HeaderMap) -> Option<(&'static str, String)> {
    let agent = header(headers, "user-agent")?;
    if agent.len() > 1024 || agent.trim() != agent || !agent.bytes().all(|b| (b' '..=b'~').contains(&b)) {
        return None;
    }
    let (name, rest) = agent.split_once('/')?;
    let (version, tail) = rest.split_once(' ').unwrap_or((rest, ""));
    parse(version)?;
    if !valid_tail(tail) {
        return None;
    }
    if name.eq_ignore_ascii_case("claude-cli") {
        return Some((CLAUDE_CODE, version.to_string()));
    }
    let originator = header(headers, "originator")?;
    if !attribution(name) || !attribution(originator) {
        return None;
    }
    if headers.contains_key("version") && header(headers, "version")? != version {
        return None;
    }
    let app = app_attribution(tail).ok()?;
    // TUI's app version is its engine version. Other app-server clients have
    // their own release scheme and may legitimately report a different version.
    if let Some((app_name, app_version)) = app
        && matches!(app_name, "codex-tui" | "codex_cli_rs")
        && app_version != version
    {
        return None;
    }
    // Native clients can override originator per thread; do not require it to
    // equal the UA's process originator. A custom process identity needs either
    // the engine's version header or its matching app-server attribution.
    if !(codex_product(name)
        || app.is_some_and(|(app, _)| app == name)
        || (name == originator && headers.contains_key("version")))
    {
        return None;
    }
    Some((CODEX, version.to_string()))
}

fn valid_source(source: &str) -> bool {
    !source.is_empty() && source.len() <= 256 && source.trim() == source && !source.chars().any(char::is_control)
}

/// Compatibility for callers that have not supplied an explicit trust decision.
/// Recognition, shared API keys and caller-supplied attribution never grant trust.
#[cfg(test)]
pub fn observe(_headers: &HeaderMap) -> bool {
    false
}

/// Learn only from an authenticated named client explicitly marked as trusted.
/// `source` MUST be its configured id, selected by the authentication layer,
/// never a request header. `None` never learns, including open/shared-key mode.
/// Returns true when persisted observations or profiles changed.
pub fn observe_trusted(headers: &HeaderMap, source: Option<&str>) -> bool {
    let Some(source) = source.filter(|source| valid_source(source)) else { return false };
    let Some((client, version)) = version_of(headers) else { return false };
    if !plausible(client, builtin(client), &version) {
        return false;
    }
    let mut state = state().write();
    let mut changed = false;
    if parse(&version) > parse(builtin(client))
        && state.seen.get(client).is_none_or(|seen| parse(&seen.version) < parse(&version))
    {
        tracing::info!("{client} {version} learned from a trusted named client");
        state.seen.insert(client, Observation { version: version.clone(), source: source.to_string() });
        changed = true;
    }
    changed |= discard_stale_profile(&mut state);
    if client == CLAUDE_CODE {
        changed |= learn_profile(&mut state, headers, &version, source);
    }
    changed
}

fn valid_profile_value(name: &str, value: &str) -> bool {
    if value.len() > 32 {
        return false;
    }
    match name {
        "x-stainless-package-version" => parse(value).is_some(),
        "x-stainless-runtime-version" => value.strip_prefix('v').and_then(parse).is_some(),
        "x-stainless-os" => matches!(value, "MacOS" | "Linux" | "Windows" | "FreeBSD" | "OpenBSD" | "Android" | "iOS"),
        "x-stainless-arch" => matches!(value, "x32" | "x64" | "arm" | "arm64"),
        _ => false,
    }
}

fn discard_stale_profile(state: &mut State) -> bool {
    let effective = newest_in(state, CLAUDE_CODE, CC_VERSION.to_string());
    if state.profile.as_ref().is_some_and(|profile| profile.observation.version != effective) {
        state.profile = None;
        return true;
    }
    false
}

fn learn_profile(state: &mut State, headers: &HeaderMap, version: &str, source: &str) -> bool {
    // Profiles describe exactly the release claimed. A newer version without a
    // valid complete profile discards the old profile and uses bundled defaults.
    if version != newest_in(state, CLAUDE_CODE, CC_VERSION.to_string())
        || !plausible(CLAUDE_CODE, CC_VERSION, version)
        || (headers.contains_key("x-stainless-runtime") && header(headers, "x-stainless-runtime") != Some("node"))
    {
        return false;
    }
    let values: Option<Vec<String>> = PROFILE
        .iter()
        .map(|(name, _)| header(headers, name).filter(|v| valid_profile_value(name, v)).map(String::from))
        .collect();
    let Some(values) = values else { return false };
    if state.profile.as_ref().is_some_and(|profile| profile.observation.version == version && profile.values == values)
    {
        return false;
    }
    state.profile =
        Some(Profile { observation: Observation { version: version.to_string(), source: source.to_string() }, values });
    true
}

/// Claude Code's runtime headers for cloaked requests.
#[cfg(test)]
pub fn claude_profile() -> Vec<(&'static str, String)> {
    let state = state().read();
    profile_in(&state)
}

fn profile_in(state: &State) -> Vec<(&'static str, String)> {
    if let Some(profile) = &state.profile
        && profile.observation.version == newest_in(state, CLAUDE_CODE, CC_VERSION.to_string())
    {
        PROFILE.iter().zip(&profile.values).map(|((name, _), v)| (*name, v.clone())).collect()
    } else {
        PROFILE.iter().map(|(name, v)| (*name, v.to_string())).collect()
    }
}

/// What to save, so a restart keeps claiming the newest release.
pub fn seen() -> BTreeMap<String, String> {
    let state = state().read();
    let mut out = BTreeMap::new();
    for (client, observation) in &state.seen {
        save_observation(&mut out, client, observation);
    }
    if let Some(profile) = &state.profile {
        save_observation(&mut out, &format!("{CLAUDE_CODE}/profile"), &profile.observation);
        // The cloak always sends the Node runtime; persist and validate that
        // invariant alongside the variable runtime values.
        out.insert(format!("{CLAUDE_CODE}/x-stainless-runtime"), "node".to_string());
        for ((name, _), value) in PROFILE.iter().zip(&profile.values) {
            out.insert(format!("{CLAUDE_CODE}/{name}"), value.clone());
        }
    }
    out
}

fn save_observation(saved: &mut BTreeMap<String, String>, key: &str, observation: &Observation) {
    saved.insert(key.to_string(), observation.version.clone());
    saved.insert(format!("{key}/provenance"), PROVENANCE.to_string());
    saved.insert(format!("{key}/source"), observation.source.clone());
}

fn restore_observation(saved: &BTreeMap<String, String>, key: &str, client: &str) -> Option<Observation> {
    if saved.get(&format!("{key}/provenance"))?.as_str() != PROVENANCE {
        return None;
    }
    let source = saved.get(&format!("{key}/source"))?;
    let version = saved.get(key)?;
    (valid_source(source) && plausible(client, builtin(client), version))
        .then(|| Observation { version: version.clone(), source: source.clone() })
}

/// Restore only fully validated observations carrying trusted-source provenance.
/// Legacy records have no proof of an explicit trust decision and are dropped.
pub fn restore(saved: &BTreeMap<String, String>) {
    let mut state = state().write();
    for client in [CLAUDE_CODE, CODEX] {
        if let Some(observation) = restore_observation(saved, client, client)
            && parse(&observation.version) > parse(builtin(client))
            && state.seen.get(client).is_none_or(|old| parse(&old.version) < parse(&observation.version))
        {
            state.seen.insert(client, observation);
        }
    }
    discard_stale_profile(&mut state);
    if state.profile.is_none()
        && let Some(observation) = restore_observation(saved, &format!("{CLAUDE_CODE}/profile"), CLAUDE_CODE)
        && observation.version == newest_in(&state, CLAUDE_CODE, CC_VERSION.to_string())
        && saved.get(&format!("{CLAUDE_CODE}/x-stainless-runtime")).map(String::as_str) == Some("node")
    {
        let values: Option<Vec<String>> = PROFILE
            .iter()
            .map(|(name, _)| {
                saved.get(&format!("{CLAUDE_CODE}/{name}")).filter(|v| valid_profile_value(name, v)).cloned()
            })
            .collect();
        state.profile = values.map(|values| Profile { observation, values });
    }
}

fn provenance(observation: &Observation) -> serde_json::Value {
    serde_json::json!({"kind": "trusted-named-client", "source": observation.source})
}

/// Management inspection of identity decisions. Contains configured client ids,
/// releases and runtime values, never authentication keys or caller headers.
pub fn snapshot() -> serde_json::Value {
    let state = state().read();
    let clients: Vec<_> = CONFIGURABLE.into_iter().map(|client| {
        let effective = newest_in(&state, client, builtin(client).to_string());
        let seen = state.seen.get(client);
        let configured = state.configured.get(client);
        let effective_provenance = if effective == builtin(client) {
            serde_json::json!({"kind": "builtin"})
        } else if configured == Some(&effective) {
            serde_json::json!({"kind": "configured"})
        } else {
            seen.map(provenance).unwrap_or(serde_json::Value::Null)
        };
        let profile = if client == CLAUDE_CODE {
            let learned = state.profile.as_ref().filter(|p| p.observation.version == effective);
            let values: BTreeMap<_, _> = profile_in(&state).into_iter().chain([("x-stainless-runtime", "node".to_string())]).collect();
            serde_json::json!({
                "version": learned.map(|p| p.observation.version.as_str()).unwrap_or(CC_VERSION),
                "values": values,
                "provenance": learned.map(|p| provenance(&p.observation)).unwrap_or(serde_json::json!({"kind": "builtin"})),
            })
        } else {
            serde_json::Value::Null
        };
        serde_json::json!({
            "id": client,
            "builtin": builtin(client),
            "configured": configured,
            "seen": seen.map(|s| &s.version),
            "seen_provenance": seen.map(provenance),
            "effective": effective,
            "provenance": effective_provenance,
            "profile": profile,
        })
    }).collect();
    serde_json::json!({"clients": clients, "profile_policy": "matching-release-or-builtin"})
}

/// Clear learned releases and runtime profiles, retaining operator configuration.
/// The caller should persist `seen()` after reset.
pub fn reset() {
    let mut state = state().write();
    state.seen.clear();
    state.profile = None;
}

/// Claude Code's user agent, for Fusebox's own requests to Anthropic and for cloaking.
pub fn claude_user_agent() -> String {
    CC_USER_AGENT.replace(CC_VERSION, &version(CLAUDE_CODE))
}

/// Read the generated UA and matching runtime profile together, so an observer
/// updating the learned release cannot split one outgoing request's identity.
pub fn claude_identity() -> (String, Vec<(&'static str, String)>) {
    let state = state().read();
    let version = newest_in(&state, CLAUDE_CODE, CC_VERSION.to_string());
    (CC_USER_AGENT.replace(CC_VERSION, &version), profile_in(&state))
}

/// For Fusebox's own and cloaked requests to the Codex backend: `client_version` and `version`.
pub fn codex_version() -> String {
    version(CODEX)
}

pub fn codex_user_agent() -> String {
    CODEX_USER_AGENT.replace(CODEX_VERSION, &version(CODEX))
}

/// The validated native engine release. Upstream can use it when a legacy
/// client's version header is absent, keeping that field consistent with its UA.
pub fn native_codex_version(headers: &HeaderMap) -> Option<String> {
    version_of(headers).and_then(|(client, version)| (client == CODEX).then_some(version))
}

/// A native-shaped Codex client. Classification grants no trust for learning.
pub fn is_codex(headers: &HeaderMap) -> bool {
    native_codex_version(headers).is_some()
}

pub fn is_claude_code(headers: &HeaderMap) -> bool {
    version_of(headers).is_some_and(|(client, _)| client == CLAUDE_CODE)
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
        assert_eq!(native_codex_version(&codex).as_deref(), Some("0.170.2"));
        // A Codex-looking agent without Codex's originator is some other program.
        assert!(!is_codex(&headers(&[("user-agent", "codex_cli_rs/0.170.2")])));
        assert_eq!(version_of(&headers(&[("user-agent", "curl/8.9.0")])), None);
    }

    fn trusted(headers: &HeaderMap) -> bool {
        observe_trusted(headers, Some("workstation"))
    }

    fn claude_version(patches: u32) -> String {
        let (major, minor, patch) = parse(CC_VERSION).unwrap();
        format!("{major}.{minor}.{}", patch + patches)
    }

    fn codex_version_ahead(minors: u32) -> String {
        let (major, minor, patch) = parse(CODEX_VERSION).unwrap();
        format!("{major}.{}.{patch}", minor + minors)
    }

    fn claude_headers(version: &str) -> HeaderMap {
        headers(&[
            ("user-agent", &format!("claude-cli/{version} (external, cli)")),
            ("x-stainless-package-version", "0.131.0"),
            ("x-stainless-os", "Linux"),
            ("x-stainless-arch", "x64"),
            ("x-stainless-runtime", "node"),
            ("x-stainless-runtime-version", "v24.11.1"),
        ])
    }

    fn codex_headers(version: &str) -> HeaderMap {
        headers(&[("user-agent", &format!("codex-tui/{version}")), ("originator", "codex-tui"), ("version", version)])
    }

    #[test]
    fn versions_are_plain_numeric_semver() {
        assert_eq!(parse("0.164.0"), Some((0, 164, 0)));
        assert_eq!(parse("4294967295.0.0"), Some((u32::MAX, 0, 0)));
        for value in [
            "",
            " 0.164.0",
            "0.164.0 ",
            "0.164",
            "0.164.0.1",
            "00.164.0",
            "0.0164.0",
            "0.164.+0",
            "0.164.0-alpha.1",
            "0.164.0+build",
            "4294967296.0.0",
        ] {
            assert_eq!(parse(value), None, "{value}");
        }
    }

    #[test]
    fn native_desktop_and_custom_app_server_attribution_is_recognised() {
        for (agent, originator) in [
            ("Codex Desktop/0.164.0 (Mac OS 26.5.2; arm64) unknown (Codex Desktop; 2026.1024.1)", "Codex Desktop"),
            ("codex_cli_rs/0.164.0 (Mac OS 26.5.2; arm64) unknown", "Codex Desktop"),
            ("My Editor/0.164.0 (Linux 6.12; x64) xterm (My Editor; 1.2.3-beta.1)", "My Editor"),
            ("My Editor/0.164.0 (Linux 6.12; x64) xterm (My Editor; 1.2.3)", "Project Agent"),
            ("codex-tui/0.164.0 (Linux 6.12; x64) xterm (codex-tui; 0.164.0)", "Project Agent"),
            ("codex_vscode/0.164.0", "codex_vscode"),
        ] {
            let h = headers(&[("user-agent", agent), ("originator", originator), ("version", "0.164.0")]);
            assert!(is_codex(&h), "{agent} / {originator}");
            assert_eq!(h["user-agent"], agent, "classification preserves attribution");
        }
        // Custom app-server suffixes establish the same shape without a version header.
        assert!(is_codex(&headers(&[
            ("user-agent", "My Editor/0.164.0 (Linux 6.12; x64) xterm (My Editor; 1.2.3)"),
            ("originator", "My Editor"),
        ])));
    }

    #[test]
    fn malformed_or_inconsistent_native_identities_are_rejected() {
        for agent in [
            "codex-fake/not-a-version",
            "codex-tui/0.164",
            "codex-tui/0.164.0-alpha.1",
            "codex-tui/0.164.00",
            "codex-tui/0.164.0(no-space)",
            "codex-tui/0.164.0 (unclosed",
            "codex-tui/0.164.0 (Linux; x64) xterm (codex-tui; 0.165.0)",
            "codex-tui/0.164.0 (Linux; x64) xterm (codex-tui; not-a-version)",
            "codex-tui/0.164.0 (Linux; x64) xterm (codex-tui;0.164.0)",
        ] {
            assert!(!is_codex(&headers(&[("user-agent", agent), ("originator", "codex-tui")])), "{agent}");
        }
        let mut h = codex_headers(CODEX_VERSION);
        h.insert("version", "0.1.0".parse().unwrap());
        assert!(!is_codex(&h));
        h.insert("version", "not-a-version".parse().unwrap());
        assert!(!is_codex(&h));
        h.insert("version", CODEX_VERSION.parse().unwrap());
        h.append("version", CODEX_VERSION.parse().unwrap());
        assert!(!is_codex(&h), "duplicate version is ambiguous");
        let mut h = codex_headers(CODEX_VERSION);
        h.append("originator", "other".parse().unwrap());
        assert!(!is_codex(&h));
        let mut h = codex_headers(CODEX_VERSION);
        h.append("user-agent", "codex-tui/0.1.0".parse().unwrap());
        assert!(!is_codex(&h));
        assert_eq!(version_of(&headers(&[("user-agent", "claude-cli/not-a-version (external, cli)")])), None);
    }

    #[test]
    fn plausibility_matches_each_clients_release_scheme_and_cannot_overflow() {
        assert!(plausible(CLAUDE_CODE, CC_VERSION, CC_VERSION));
        assert!(plausible(CLAUDE_CODE, CC_VERSION, &claude_version(CLAUDE_PATCH_ADVANCE)));
        assert!(!plausible(CLAUDE_CODE, CC_VERSION, &claude_version(CLAUDE_PATCH_ADVANCE + 1)));
        assert!(!plausible(CLAUDE_CODE, "2.1.309", "2.2.0"));
        assert!(!plausible(CLAUDE_CODE, "2.1.309", "2.1.308"));
        assert!(plausible(CODEX, CODEX_VERSION, &codex_version_ahead(CODEX_MINOR_ADVANCE)));
        assert!(!plausible(CODEX, CODEX_VERSION, &codex_version_ahead(CODEX_MINOR_ADVANCE + 1)));
        assert!(!plausible(CODEX, "0.164.0", "0.165.4294967295"));
        assert!(!plausible(CODEX, "0.164.0", "1.0.0"));
        assert!(plausible(CODEX, "0.4294967290.0", "0.4294967295.0"));
        assert!(plausible(CLAUDE_CODE, "2.1.4294967290", "2.1.4294967295"));
        assert!(!plausible(CODEX, "0.4294967295.0", "0.0.0"));
    }

    #[test]
    fn recognition_and_authenticated_looking_headers_never_grant_learning() {
        let mut h = claude_headers(&claude_version(1));
        h.insert("authorization", "Bearer shared-key".parse().unwrap());
        h.insert("x-fusebox-client", "workstation".parse().unwrap());
        h.insert("x-fusebox-trust-client-identity", "true".parse().unwrap());
        assert!(!observe_trusted(&h, None));
        assert!(!observe(&h), "legacy API cannot grant trust");
        for source in ["", " ", "bad\nsource", &"x".repeat(257)] {
            assert!(!observe_trusted(&h, Some(source)));
        }
        let codex = codex_headers(&codex_version_ahead(1));
        assert!(is_codex(&codex));
        assert!(!observe_trusted(&codex, None));
        assert!(seen().is_empty());
        assert_eq!(version(CLAUDE_CODE), CC_VERSION);
        assert_eq!(version(CODEX), CODEX_VERSION);
        assert_eq!(claude_profile()[1].1, "MacOS");
    }

    #[test]
    fn trusted_newest_releases_are_claimed_with_provenance() {
        let newer = claude_version(1);
        let h = headers(&[("user-agent", &format!("claude-cli/{newer} (external, cli)"))]);
        assert!(trusted(&h));
        assert!(!trusted(&h), "already known");
        assert_eq!(claude_user_agent(), CC_USER_AGENT.replace(CC_VERSION, &newer));
        let codex = codex_version_ahead(1);
        assert!(trusted(&codex_headers(&codex)));
        assert_eq!(codex_user_agent(), CODEX_USER_AGENT.replace(CODEX_VERSION, &codex));
        let saved = seen();
        assert_eq!(saved.get(CLAUDE_CODE), Some(&newer));
        assert_eq!(saved["claude-code/source"], "workstation");
        assert_eq!(saved["claude-code/provenance"], PROVENANCE);
        reset();
        restore(&saved);
        assert_eq!(version(CLAUDE_CODE), newer);
        assert_eq!(version(CODEX), codex);
        assert_eq!(seen(), saved);
    }

    #[test]
    fn huge_versions_cannot_pin_a_release_or_runtime_profile() {
        for version in ["999.999.999", "2.1.9999", "2.1.4294967295", "2.2.0"] {
            assert!(!trusted(&claude_headers(version)), "{version}");
        }
        for version in ["999.999.999", "0.4294967295.0", "0.165.4294967295"] {
            let h = codex_headers(version);
            assert!(is_codex(&h), "recognition is independent of learning bounds");
            assert!(!trusted(&h), "{version}");
        }
        assert!(seen().is_empty());
        assert_eq!(claude_profile()[1].1, "MacOS");
        assert!(trusted(&claude_headers(CC_VERSION)), "normal profiles can still be learned");
        assert_eq!(claude_profile()[1].1, "Linux");
    }

    #[test]
    fn learning_cannot_walk_past_the_builtin_bounds() {
        assert!(trusted(&codex_headers(&codex_version_ahead(CODEX_MINOR_ADVANCE))));
        assert!(!trusted(&codex_headers(&codex_version_ahead(CODEX_MINOR_ADVANCE + 1))));
        assert!(trusted(&claude_headers(&claude_version(CLAUDE_PATCH_ADVANCE))));
        assert!(!trusted(&claude_headers(&claude_version(CLAUDE_PATCH_ADVANCE + 1))));
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
        let (major, minor, _) = parse(CC_VERSION).unwrap();
        let configured = format!("{major}.{minor}.9000");
        configure(&BTreeMap::from([("claude-code".to_string(), configured.clone())]));
        assert_eq!(claude_user_agent(), CC_USER_AGENT.replace(CC_VERSION, &configured));
        assert!(!trusted(&claude_headers(&configured)), "operator overrides do not expand learning bounds");
        configure(&BTreeMap::new());
        assert_eq!(version(DEVIN), crate::devin::CLIENT_VERSION);
    }

    #[test]
    fn runtime_profiles_roundtrip_only_with_their_trusted_provenance() {
        assert!(trusted(&claude_headers(CC_VERSION)));
        assert!(!trusted(&claude_headers(CC_VERSION)), "nothing new");
        let profile = claude_profile();
        assert_eq!(profile[1].1, "Linux");
        assert_eq!(profile[3].1, "v24.11.1");
        assert_eq!(claude_identity(), (CC_USER_AGENT.to_string(), profile.clone()));
        let saved = seen();
        assert_eq!(saved["claude-code/profile/source"], "workstation");
        assert_eq!(saved["claude-code/profile/provenance"], PROVENANCE);
        reset();
        restore(&saved);
        assert_eq!(claude_profile(), profile);
        assert_eq!(seen(), saved);
    }

    #[test]
    fn runtime_fields_have_semantic_validation_and_bounded_lengths() {
        for (name, value) in [
            ("x-stainless-package-version", "audit-marker"),
            ("x-stainless-package-version", "0.131.0-alpha.1"),
            ("x-stainless-os", "audit-marker"),
            ("x-stainless-os", "Linux; rm -rf"),
            ("x-stainless-arch", "audit-marker"),
            ("x-stainless-runtime", "audit-marker"),
            ("x-stainless-runtime", "deno"),
            ("x-stainless-runtime-version", "audit-marker"),
            ("x-stainless-runtime-version", "v24.1"),
            ("x-stainless-runtime-version", "v24.01.0"),
            ("x-stainless-runtime-version", "v24.11.1-alpha.1"),
            ("x-stainless-package-version", &"1".repeat(33)),
        ] {
            let mut h = claude_headers(CC_VERSION);
            h.insert(name, value.parse().unwrap());
            assert!(!trusted(&h), "{name}: {value}");
            assert!(seen().is_empty());
        }
        let mut duplicate = claude_headers(CC_VERSION);
        duplicate.append("x-stainless-os", "Linux".parse().unwrap());
        assert!(!trusted(&duplicate));
        let mut incomplete = claude_headers(CC_VERSION);
        incomplete.remove("x-stainless-arch");
        assert!(!trusted(&incomplete));
        let mut implicit_node = claude_headers(CC_VERSION);
        implicit_node.remove("x-stainless-runtime");
        assert!(trusted(&implicit_node), "legacy Node clients may omit the runtime name");
    }

    #[test]
    fn profiles_cannot_outlive_the_release_they_describe() {
        assert!(trusted(&claude_headers(CC_VERSION)));
        let next = claude_version(1);
        assert!(trusted(&headers(&[("user-agent", &format!("claude-cli/{next} (external, cli)"))])));
        assert_eq!(version(CLAUDE_CODE), next);
        let (agent, profile) = claude_identity();
        assert_eq!(agent, CC_USER_AGENT.replace(CC_VERSION, &next));
        assert_eq!(profile[1].1, "MacOS");
        assert_eq!(claude_profile()[1].1, "MacOS", "a newer release without a profile falls back to bundled defaults");
        assert!(!seen().contains_key("claude-code/profile"));
        assert!(!trusted(&claude_headers(CC_VERSION)), "an older profile cannot be relabelled as the new release");
        assert!(trusted(&claude_headers(&next)));
        assert_eq!(claude_profile()[1].1, "Linux");
        configure(&BTreeMap::from([(CLAUDE_CODE.to_string(), claude_version(2))]));
        assert_eq!(claude_profile()[1].1, "MacOS", "configuration changes also invalidate older profiles");
        assert!(!seen().contains_key("claude-code/profile"));
    }

    #[test]
    fn legacy_or_unproven_saved_observations_are_dropped() {
        assert!(trusted(&claude_headers(&claude_version(1))));
        assert!(trusted(&codex_headers(&codex_version_ahead(1))));
        let saved = seen();
        for suffix in ["/provenance", "/source"] {
            reset();
            let unproven =
                saved.iter().filter(|(key, _)| !key.ends_with(suffix)).map(|(k, v)| (k.clone(), v.clone())).collect();
            restore(&unproven);
            assert!(seen().is_empty(), "missing {suffix}");
        }
        reset();
        let legacy = saved
            .iter()
            .filter(|(key, _)| !key.ends_with("/provenance") && !key.ends_with("/source"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        restore(&legacy);
        assert!(seen().is_empty());
    }

    #[test]
    fn poisoned_saved_versions_and_profiles_are_revalidated() {
        assert!(trusted(&claude_headers(&claude_version(1))));
        let saved = seen();
        for (key, value) in [
            (CLAUDE_CODE, "999.999.999"),
            (CLAUDE_CODE, "2.1.9999"),
            (CLAUDE_CODE, "2.1.310-alpha.1"),
            ("claude-code/provenance", "caller-header"),
            ("claude-code/source", ""),
            ("claude-code/source", "bad\nsource"),
            ("claude-code/source", &"x".repeat(257)),
        ] {
            reset();
            let mut poisoned = saved.clone();
            poisoned.insert(key.to_string(), value.to_string());
            restore(&poisoned);
            assert_eq!(version(CLAUDE_CODE), CC_VERSION, "{key} = {value}");
            assert!(seen().is_empty(), "an unmatched profile cannot restore independently");
        }
        for (key, value) in [
            ("claude-code/profile", "999.999.999"),
            ("claude-code/profile", CC_VERSION),
            ("claude-code/profile/provenance", "caller-header"),
            ("claude-code/profile/source", ""),
            ("claude-code/x-stainless-os", "audit-marker"),
            ("claude-code/x-stainless-arch", "audit-marker"),
            ("claude-code/x-stainless-package-version", "audit-marker"),
            ("claude-code/x-stainless-runtime", "audit-marker"),
            ("claude-code/x-stainless-runtime-version", "audit-marker"),
        ] {
            reset();
            let mut poisoned = saved.clone();
            poisoned.insert(key.to_string(), value.to_string());
            restore(&poisoned);
            assert_eq!(version(CLAUDE_CODE), claude_version(1));
            assert_eq!(claude_profile()[1].1, "MacOS", "{key} = {value}");
            assert!(!seen().contains_key("claude-code/profile"));
        }
        reset();
        assert!(trusted(&codex_headers(&codex_version_ahead(1))));
        let mut poisoned = seen();
        poisoned.insert(CODEX.to_string(), "0.165.4294967295".to_string());
        reset();
        restore(&poisoned);
        assert!(seen().is_empty());
    }

    #[test]
    fn restore_cannot_replace_a_newer_observation_or_install_an_older_profile() {
        assert!(trusted(&claude_headers(&claude_version(1))));
        let saved = seen();
        let newer = claude_version(2);
        assert!(trusted(&headers(&[("user-agent", &format!("claude-cli/{newer} (external, cli)"))])));
        restore(&saved);
        assert_eq!(version(CLAUDE_CODE), newer);
        assert_eq!(claude_profile()[1].1, "MacOS");
        assert!(!seen().contains_key("claude-code/profile"));
    }

    #[test]
    fn snapshot_explains_effective_identities_profiles_and_sources() {
        let next = claude_version(1);
        assert!(observe_trusted(&claude_headers(&next), Some("trusted laptop")));
        let snapshot = snapshot();
        let cc = &snapshot["clients"][0];
        assert_eq!(cc["id"], CLAUDE_CODE);
        assert_eq!(cc["builtin"], CC_VERSION);
        assert_eq!(cc["configured"], serde_json::Value::Null);
        assert_eq!(cc["effective"], next);
        assert_eq!(cc["seen"], next);
        assert_eq!(cc["seen_provenance"]["source"], "trusted laptop");
        assert_eq!(cc["provenance"]["kind"], "trusted-named-client");
        assert_eq!(cc["profile"]["version"], next);
        assert_eq!(cc["profile"]["provenance"]["source"], "trusted laptop");
        assert_eq!(cc["profile"]["values"]["x-stainless-runtime"], "node");
        assert_eq!(snapshot["clients"][1]["provenance"]["kind"], "builtin");
        assert_eq!(snapshot["clients"].as_array().unwrap().len(), CONFIGURABLE.len());
    }

    #[test]
    fn reset_clears_learned_identity_but_retains_operator_configuration() {
        let configured = claude_version(1);
        configure(&BTreeMap::from([(CLAUDE_CODE.to_string(), configured.clone())]));
        assert!(trusted(&claude_headers(&claude_version(2))));
        assert!(trusted(&codex_headers(&codex_version_ahead(1))));
        reset();
        assert!(seen().is_empty(), "persisting after reset removes all saved observations");
        assert_eq!(version(CLAUDE_CODE), configured);
        assert_eq!(version(CODEX), CODEX_VERSION);
        assert_eq!(claude_profile()[1].1, "MacOS");
        let snapshot = snapshot();
        assert_eq!(snapshot["clients"][0]["configured"], configured);
        assert_eq!(snapshot["clients"][0]["provenance"]["kind"], "configured");
        assert!(snapshot["clients"][0]["seen"].is_null());
        assert_eq!(snapshot["clients"][0]["profile"]["provenance"]["kind"], "builtin");
        assert!(trusted(&claude_headers(&configured)), "reset permits learning a valid profile again");
    }
}
