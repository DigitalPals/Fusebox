//! For whoever runs Fusebox: `/readyz` says whether it can take requests, and
//! `/metrics` exposes its counters to Prometheus (with the dashboard's authentication).

use std::fmt::Write;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde_json::json;

use crate::state::App;
pub mod performance;

/// `0.4.0 (1a2b3c4)`, or `0.4.0` when the commit isn't known (see build.rs).
pub const VERSION: &str = env!("FUSEBOX_VERSION");

/// Ready means it can answer: the usage database (when on) works and at least one
/// enabled account isn't sitting out. Liveness stays `/healthz`.
pub async fn readyz(State(app): State<Arc<App>>) -> Response {
    let now = Utc::now();
    let accounts = app.pool.all();
    let ready = accounts
        .iter()
        .filter(|a| {
            let st = a.state.lock();
            !st.disabled && !st.pauses(now).contains_key("*")
        })
        .count();
    let usage = match (&app.usage, &app.usage_error) {
        (Some(store), _) => {
            let state = store.health()["state"].as_str().unwrap_or("unknown").to_string();
            if state == "stopped" { "error".to_string() } else { state }
        }
        (None, Some(_)) => "error".into(),
        (None, None) => "off".into(),
    };
    let ok = ready > 0 && usage != "error";
    let status = if ok { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    let body = json!({
        "status": if ok { "ready" } else { "unavailable" },
        "accounts": { "ready": ready, "total": accounts.len() },
        "usage": usage,
    });
    (status, axum::Json(body)).into_response()
}

/// Prometheus label values escape backslashes, quotes and newlines.
fn label(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

pub async fn metrics(State(app): State<Arc<App>>) -> Response {
    let mut out = String::new();
    let now = Utc::now();
    let mut metric = |name: &str, kind: &str, help: &str, rows: Vec<(String, f64)>| {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
        for (labels, value) in rows {
            let _ = if labels.is_empty() {
                writeln!(out, "{name} {value}")
            } else {
                writeln!(out, "{name}{{{labels}}} {value}")
            };
        }
    };
    let commit = env!("FUSEBOX_COMMIT");
    metric(
        "fusebox_build_info",
        "gauge",
        "The running version.",
        vec![(format!("version=\"{}\",commit=\"{commit}\"", env!("CARGO_PKG_VERSION")), 1.0)],
    );
    metric(
        "fusebox_uptime_seconds",
        "gauge",
        "Seconds since start.",
        vec![(String::new(), (now - app.started).num_seconds() as f64)],
    );
    let totals = app.stats.totals.lock().clone();
    metric(
        "fusebox_requests_total",
        "counter",
        "Requests since start, by outcome.",
        vec![
            ("outcome=\"ok\"".into(), totals.ok as f64),
            ("outcome=\"failed\"".into(), totals.failed as f64),
            ("outcome=\"cancelled\"".into(), totals.cancelled as f64),
        ],
    );
    metric(
        "fusebox_requests_in_flight",
        "gauge",
        "Requests being answered now.",
        vec![(String::new(), app.stats.active.load(Ordering::Relaxed) as f64)],
    );
    metric(
        "fusebox_tokens_total",
        "counter",
        "Tokens since start: input (with cache writes), output and cache reads.",
        vec![
            ("kind=\"input\"".into(), totals.input_tokens as f64),
            ("kind=\"output\"".into(), totals.output_tokens as f64),
            ("kind=\"cached\"".into(), totals.cache_tokens as f64),
        ],
    );
    let mut ready = Vec::new();
    let mut in_flight = Vec::new();
    let mut requests = Vec::new();
    let mut failures = Vec::new();
    let mut used = Vec::new();
    let mut resets = Vec::new();
    for a in app.pool.all() {
        let id =
            format!("account=\"{}\",provider=\"{}\",label=\"{}\"", label(&a.id), a.provider.as_str(), label(&a.label));
        let st = a.state.lock();
        let up = !st.disabled && !st.pauses(now).contains_key("*");
        ready.push((id.clone(), f64::from(u8::from(up))));
        in_flight.push((id.clone(), st.active_requests.load(Ordering::Relaxed) as f64));
        requests.push((id.clone(), st.counters.requests as f64));
        failures.push((id.clone(), st.counters.failures as f64));
        for w in st.quota.windows.iter().filter(|w| w.resets_at.is_none_or(|t| t > now)) {
            let window = match &w.model {
                Some(m) => format!("{}:{m}", w.name),
                None => w.name.clone(),
            };
            let wl = format!("{id},window=\"{}\"", label(&window));
            used.push((wl.clone(), w.used));
            if let Some(t) = w.resets_at {
                resets.push((wl, t.timestamp() as f64));
            }
        }
    }
    metric("fusebox_account_ready", "gauge", "1 when the account can take requests.", ready);
    metric("fusebox_account_in_flight", "gauge", "Requests on the account now.", in_flight);
    metric("fusebox_account_requests_total", "counter", "Requests the account answered since start.", requests);
    metric("fusebox_account_failures_total", "counter", "Failed requests on the account since start.", failures);
    metric("fusebox_account_quota_used_percent", "gauge", "Subscription usage window, percent used.", used);
    metric("fusebox_account_quota_resets_timestamp_seconds", "gauge", "When a usage window resets.", resets);
    let faults = crate::push::current_faults(&app);
    let mut kinds = std::collections::BTreeMap::<String, usize>::new();
    for f in faults.as_array().into_iter().flatten() {
        *kinds.entry(f["kind"].as_str().unwrap_or("other").to_string()).or_default() += 1;
    }
    metric(
        "fusebox_faults",
        "gauge",
        "What has tripped, as the dashboard's faults button lists it.",
        kinds.into_iter().map(|(k, n)| (format!("kind=\"{}\"", label(&k)), n as f64)).collect(),
    );
    let cfg = app.cfg();
    let (hits, misses, entries, bytes) = app.embedding_cache.lock().stats(&cfg.embeddings);
    metric(
        "fusebox_embedding_cache_lookups_total",
        "counter",
        "Exact embedding result cache lookups since start.",
        vec![("result=\"hit\"".into(), hits as f64), ("result=\"miss\"".into(), misses as f64)],
    );
    metric(
        "fusebox_embedding_cache_entries",
        "gauge",
        "Exact embedding results retained.",
        vec![(String::new(), entries as f64)],
    );
    metric(
        "fusebox_embedding_cache_bytes",
        "gauge",
        "Serialized bytes of retained embedding results.",
        vec![(String::new(), bytes as f64)],
    );
    let clients: Vec<_> = cfg.named_clients.iter().map(|c| (c.id.clone(), app.clients.spend(&c.id))).collect();
    metric(
        "fusebox_client_tokens_today",
        "gauge",
        "A named client's counted tokens today (cache reads excluded).",
        clients.iter().map(|(id, s)| (format!("client=\"{}\"", label(id)), s.tokens_today as f64)).collect(),
    );
    metric(
        "fusebox_client_spend_month_usd",
        "gauge",
        "A named client's estimated spend this month at API list prices.",
        clients.iter().map(|(id, s)| (format!("client=\"{}\"", label(id)), s.usd_month)).collect(),
    );
    if let Some(store) = &app.usage {
        let health = store.health();
        metric(
            "fusebox_usage_store_healthy",
            "gauge",
            "1 when the usage database writes cleanly.",
            vec![(String::new(), f64::from(u8::from(health["state"] == "healthy")))],
        );
        metric(
            "fusebox_usage_dropped_total",
            "counter",
            "Usage observations dropped by this writer.",
            vec![(String::new(), health["dropped"].as_f64().unwrap_or(0.0))],
        );
    }
    app.stats.performance.lock().render(&mut out);
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], out).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_values_are_escaped() {
        assert_eq!(label("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }
}
