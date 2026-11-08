//! Totals kept after retention deletes records: a past day or month reads the same
//! before and after its records go, in every timezone and filter, nothing is
//! counted twice, and records never come back into a period kept as totals.
use super::super::types::Tokens;
use super::*;

struct TempDb {
    dir: PathBuf,
    path: PathBuf,
}
impl std::ops::Deref for TempDb {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}
impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn temp(name: &str) -> TempDb {
    let dir = std::env::temp_dir().join(format!("fusebox-history-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    TempDb { path: dir.join(format!("{name}.sqlite")), dir }
}

fn at(value: &str) -> i64 {
    boundary(value, &crate::zone::Zone::utc()).unwrap()
}

fn call(source: &str, id: &str, provider: &str, when: i64) -> Observation {
    let mut o = Observation::new(source, id.into(), provider, when);
    o.actual_model = Some(if provider == "anthropic" { "claude-sonnet-4-6" } else { "gpt-6.1-sol" }.into());
    o.completeness = "complete".into();
    o.tokens = Tokens {
        input: Some(1000),
        cache_read: Some(100),
        cache_write: Some(10),
        output: Some(100),
        reasoning: Some(0),
        ..Default::default()
    };
    o
}

/// Six days of mixed traffic at hours that fall on different local days in
/// Kathmandu (+5:45) and St. John's (-2:30): priced and unpriced proxy calls with
/// and without entrypoints, imports a proxy call matched, imports nothing matched
/// (one without a stable ID) and a collector's.
fn traffic() -> Vec<Observation> {
    let mut out = Vec::new();
    for day in 1..=6 {
        for (n, time) in ["00:05", "02:20", "05:59", "07:03", "12:00", "18:10", "18:20", "23:50"].iter().enumerate() {
            let when = at(&format!("2026-10-{day:02}T{time}:00Z"));
            let id = |kind: &str| format!("{kind}-{day}-{n}");
            let mut claude = call("proxy", &id("claude"), "anthropic", when);
            claude.account_id = Some("acct-a".into());
            claude.client_id = Some("client-1".into());
            claude.response_id = Some(id("msg"));
            claude.entrypoint = Some(if n % 2 == 0 { "cli" } else { "sdk-ts" }.into());
            let mut openai = call("proxy", &id("openai"), "openai", when + 60_000);
            openai.account_id = Some("acct-b".into());
            openai.response_id = Some(id("resp"));
            let mut unknown = call("proxy", &id("unknown"), "openai", when + 120_000);
            unknown.actual_model = Some("unknown-model-x".into());
            unknown.account_id = Some("acct-b".into());
            let mut missing = call("proxy", &id("missing"), "anthropic", when + 180_000);
            missing.tokens = Tokens::default();
            missing.completeness = "missing".into();
            missing.status = Some(499);
            let mut incomplete = call("proxy", &id("incomplete"), "anthropic", when + 240_000);
            incomplete.tokens.cache_read = None;
            incomplete.completeness = "partial".into();
            incomplete.entrypoint = Some("cli".into());
            let mut matched = call("claude_code", &id("matched"), "anthropic", when + 30_000);
            matched.response_id = claude.response_id.clone();
            let mut imported = call("claude_code", &id("imported"), "anthropic", when + 90_000);
            imported.response_id = Some(id("history"));
            let weak = call("codex", &id("weak"), "openai", when + 150_000);
            let mut collected = call("codex", &id("collected"), "openai", when + 210_000);
            collected.origin_id = "collector:laptop".into();
            collected.response_id = Some(id("laptop"));
            out.extend([claude, openai, unknown, missing, incomplete, matched, imported, weak, collected]);
        }
    }
    out
}

fn queries() -> Vec<Query> {
    let mut out = Vec::new();
    for tz in ["UTC", "Asia/Kathmandu", "America/St_Johns"] {
        for day in 1..=7 {
            out.push(Query {
                start: Some(format!("2026-10-{day:02}")),
                end: Some(format!("2026-10-{:02}", day + 1)),
                timezone: Some(tz.into()),
                ..Default::default()
            });
        }
    }
    let all = Query {
        start: Some("2026-09-01".into()),
        end: Some("2026-11-01".into()),
        timezone: Some("UTC".into()),
        ..Default::default()
    };
    out.push(Query { stack: Some("model".into()), ..all.clone() });
    out.push(Query { provider: Some("anthropic".into()), ..all.clone() });
    out.push(Query { model: Some("claude-sonnet-4-6".into()), ..all.clone() });
    out.push(Query { account: Some("acct-a".into()), ..all.clone() });
    out.push(Query { client: Some("client-1".into()), ..all.clone() });
    out.push(Query { client: Some("collector:laptop".into()), ..all.clone() });
    for source in ["proxy", "claude_code", "codex"] {
        out.push(Query { source: Some(source.into()), ..all.clone() });
    }
    for entrypoint in ["cli", "sdk-ts"] {
        out.push(Query { entrypoint: Some(entrypoint.into()), ..all.clone() });
        out.push(Query { entrypoint: Some(entrypoint.into()), timezone: Some("Asia/Kathmandu".into()), ..all.clone() });
    }
    out.push(all);
    out
}

/// What a range reads, without the two fields that say how the data is stored.
async fn read(store: &Store, q: &Query) -> (Value, Value) {
    let mut summary = store.dashboard(q.clone()).await.unwrap();
    let combined = summary["combined"].as_object_mut().unwrap();
    combined.remove("proxy_first_event_at_ms");
    combined.remove("summarized_before_ms");
    (summary, store.entrypoints(q.clone()).await.unwrap())
}

async fn retain(store: &Store, before: i64) {
    store.call(move |c| purge_conn(c, before, true)).await.unwrap();
}

#[tokio::test]
async fn past_days_and_months_read_the_same_after_their_records_are_deleted() {
    let p = temp("parity");
    let store = Store::open(&p, 64, 3650, None).unwrap();
    let records = traffic();
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    let mut before = Vec::new();
    for q in queries() {
        before.push(read(&store, &q).await);
    }
    let first = store.dashboard(queries().pop().unwrap()).await.unwrap()["combined"]["proxy_first_event_at_ms"].clone();
    // Not on a bucket boundary: one bucket is kept partly as totals, partly as records.
    let cutoff = at("2026-10-04T07:07:00Z");
    retain(&store, cutoff).await;
    let left: i64 = store
        .call(move |c| {
            Ok(c.query_row("SELECT COUNT(*) FROM usage_observations WHERE event_at_ms<?1", [cutoff], |r| r.get(0))?)
        })
        .await
        .unwrap();
    assert_eq!(left, 0, "retention deleted the records");
    for (q, expected) in queries().iter().zip(&before) {
        let after = read(&store, q).await;
        assert_eq!(after.0, expected.0, "the summary changed for {q:?}");
        assert_eq!(after.1, expected.1, "the entrypoints changed for {q:?}");
        // The full scan reads the kept totals too.
        let mut scan = store.dashboard_scan(q.clone()).await.unwrap();
        scan["combined"].as_object_mut().unwrap().remove("proxy_first_event_at_ms");
        scan["combined"].as_object_mut().unwrap().remove("summarized_before_ms");
        assert_eq!(scan, after.0, "the full scan disagrees for {q:?}");
        assert_eq!(store.entrypoints_scan(q.clone()).await.unwrap(), after.1);
    }
    let all = store.dashboard(queries().pop().unwrap()).await.unwrap();
    let totals = &all["combined"]["totals"];
    assert_eq!(totals["observations"], 6 * 8 * 8, "every call but the matched imports counts once");
    assert_eq!(totals["matched"], 6 * 8);
    assert_eq!(totals["weak_identity"], 6 * 8);
    assert_eq!(totals["unpriced_reasons"], json!({"incomplete_usage":48,"missing_usage":48,"unknown_model":48}));
    // Kept totals know the first call's bucket: 00:05 becomes 00:00. Every local
    // midnight is a bucket boundary, so it falls on the same local day everywhere.
    assert_eq!(first, at("2026-10-01T00:05:00Z"));
    assert_eq!(all["combined"]["proxy_first_event_at_ms"], at("2026-10-01T00:00:00Z"));
    assert_eq!(all["combined"]["summarized_before_ms"], cutoff);
    // Status measures the kept totals apart from the records it projects.
    let path = p.to_path_buf();
    let stats = store.call(move |c| compact::stats(c, &path, false, 45)).await.unwrap();
    assert!(stats.kept_bytes > 0);
    assert_eq!(stats.retention_days, 45);
    assert_eq!(stats.settles_near_bytes, stats.growth_bytes_per_day * 45 + stats.kept_bytes);
    // Records themselves are gone: the records list starts at the cutoff.
    let old = Query { end: Some("2026-10-04".into()), ..queries().pop().unwrap() };
    assert_eq!(store.details(old).await.unwrap()["total"], 0);
    // Later purges add to the kept totals; nothing drifts.
    retain(&store, at("2026-10-06T00:00:00Z")).await;
    for (q, expected) in queries().iter().zip(&before) {
        assert_eq!(read(&store, q).await.0, expected.0, "a second purge changed {q:?}");
    }
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_entry_a_newer_record_replaces_is_not_kept_twice() {
    let p = temp("moved");
    let store = Store::open(&p, 16, 3650, None).unwrap();
    let mut old = call("proxy", "attempt-1", "anthropic", at("2026-10-02T10:00:00Z"));
    old.account_id = Some("acct-a".into());
    old.response_id = Some("msg-shared".into());
    // The same response recorded again later, less completely: it loses while both exist.
    let mut newer = old.clone();
    newer.source_event_id = "attempt-2".into();
    newer.event_at_ms = at("2026-10-05T10:00:00Z");
    newer.completeness = "partial".into();
    newer.tokens.cache_read = None;
    store.call(move |c| insert_batch(c, &[old, newer])).await.unwrap();
    let month = queries().pop().unwrap();
    let day = |d: &str, e: &str| Query { start: Some(d.into()), end: Some(e.into()), ..month.clone() };
    let count = |v: Value| v["combined"]["totals"]["observations"].as_i64().unwrap();
    assert_eq!(count(store.dashboard(month.clone()).await.unwrap()), 1);
    assert_eq!(count(store.dashboard(day("2026-10-02", "2026-10-03")).await.unwrap()), 1);
    retain(&store, at("2026-10-04T00:00:00Z")).await;
    // The response is now counted where its remaining record is, once.
    assert_eq!(count(store.dashboard(month.clone()).await.unwrap()), 1);
    assert_eq!(count(store.dashboard(day("2026-10-02", "2026-10-03")).await.unwrap()), 0);
    assert_eq!(count(store.dashboard(day("2026-10-05", "2026-10-06")).await.unwrap()), 1);
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn records_never_come_back_into_a_period_kept_as_totals() {
    let p = temp("floor");
    let store = Store::open(&p, 16, 3650, None).unwrap();
    let first = call("codex", "history-1", "openai", at("2026-10-02T10:00:00Z"));
    store.call(move |c| insert_batch(c, &[first])).await.unwrap();
    retain(&store, at("2026-10-04T00:00:00Z")).await;
    // `usage backfill` lowers the purge watermark to re-read history files; the same
    // record must not return beside its kept total, nor any other from that period.
    store.call(|c| Ok(c.execute("UPDATE usage_meta SET value='0' WHERE key='purge_before_ms'", [])?)).await.unwrap();
    let again = call("codex", "history-1", "openai", at("2026-10-02T10:00:00Z"));
    let other = call("codex", "history-2", "openai", at("2026-10-03T23:00:00Z"));
    let later = call("codex", "history-3", "openai", at("2026-10-04T01:00:00Z"));
    let result = store.call(move |c| insert_batch(c, &[again, other, later])).await.unwrap();
    assert_eq!(result["purged"], 2);
    assert_eq!(result["inserted"], 1);
    let totals = store.dashboard(queries().pop().unwrap()).await.unwrap()["combined"]["totals"].clone();
    assert_eq!(totals["observations"], 2);
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_explicit_purge_deletes_kept_totals_too() {
    let p = temp("explicit");
    let store = Store::open(&p, 64, 3650, None).unwrap();
    let records = traffic();
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    retain(&store, at("2026-10-04T00:00:00Z")).await;
    let day = |d: u32| Query {
        start: Some(format!("2026-10-{d:02}")),
        end: Some(format!("2026-10-{:02}", d + 1)),
        timezone: Some("UTC".into()),
        ..Default::default()
    };
    let calls = |v: Value| v["combined"]["totals"]["observations"].as_i64().unwrap();
    assert_eq!(calls(store.dashboard(day(2)).await.unwrap()), 64);
    store.purge(at("2026-10-03T00:00:00Z")).await.unwrap();
    for d in 1..=2 {
        let value = store.dashboard(day(d)).await.unwrap();
        assert_eq!(calls(value.clone()), 0, "day {d} kept a total after an explicit purge");
        assert_eq!(value["facets"]["providers"], json!([]));
        assert_eq!(store.entrypoints(day(d)).await.unwrap()["breakdown"], json!([]));
    }
    assert_eq!(calls(store.dashboard(day(3)).await.unwrap()), 64, "later kept totals stay");
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn totals_retention_drops_only_old_totals_and_survives_reopening() {
    let p = temp("totals-retention");
    let now = Utc::now().timestamp_millis();
    let store = Store::open(&p, 16, 3650, None).unwrap();
    let records: Vec<_> =
        [40, 10, 1].iter().map(|d| call("proxy", &format!("call-{d}"), "openai", now - d * 86_400_000)).collect();
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    store.shutdown().await.unwrap();
    // Records for 5 days, totals for 20: the 10-day-old call is kept as a total,
    // the 40-day-old one is gone entirely.
    let store = Store::open_keeping(&p, 16, 5, 20, None).unwrap();
    let recent = Query {
        start: Some((Utc::now() - chrono::Duration::days(60)).date_naive().to_string()),
        end: Some((Utc::now() + chrono::Duration::days(1)).date_naive().to_string()),
        timezone: Some("UTC".into()),
        ..Default::default()
    };
    let calls = |v: Value| v["combined"]["totals"]["observations"].as_i64().unwrap();
    assert_eq!(calls(store.dashboard(recent.clone()).await.unwrap()), 2);
    assert_eq!(store.details(recent.clone()).await.unwrap()["total"], 1, "one record is left");
    store.shutdown().await.unwrap();
    // Offline commands reopen with the saved settings, not the defaults.
    let store = Store::open_existing(&p, 16).unwrap();
    let saved: String = store
        .call(|c| Ok(c.query_row("SELECT value FROM usage_meta WHERE key='totals_retention_days'", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(saved, "20");
    assert_eq!(calls(store.dashboard(recent).await.unwrap()), 2);
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_sum_past_64_bits_stays_out_of_range_once_kept() {
    let p = temp("overflow");
    let store = Store::open(&p, 8, 3650, None).unwrap();
    let records: Vec<_> = (0..200)
        .map(|i| {
            let mut o = call("proxy", &i.to_string(), "openai", at("2026-10-02T10:00:00Z") + i);
            o.actual_model = Some("gpt-6-astra".into());
            o.tokens.output = Some(1_000_000_000_000);
            o
        })
        .collect();
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    let month = queries().pop().unwrap();
    let before = store.dashboard(month.clone()).await.unwrap()["combined"]["totals"].clone();
    assert_eq!(before["aggregation_overflow"], true);
    retain(&store, at("2026-10-04T00:00:00Z")).await;
    let after = store.dashboard(month).await.unwrap()["combined"]["totals"].clone();
    assert_eq!(after, before);
    assert!(after["estimated_cost_nanos"].is_null());
    store.shutdown().await.unwrap();
}

/// What keeping totals costs: time added to a large purge (such as the first after
/// lowering `retention-days`) and bytes a day, for traffic shaped like production's
/// (about 12,300 calls a day; a few accounts with long ids, models and clients active
/// at once). `cargo test --release history_tests::purge_cost -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn purge_cost() {
    const PER_DAY: i64 = 12_300;
    const DAYS: i64 = 14;
    let models = [
        ("anthropic", "claude-opus-5-5"),
        ("anthropic", "claude-sonnet-5-5"),
        ("openai", "gpt-6.1-sol"),
        ("anthropic", "claude-haiku-5-5"),
    ];
    let mut results = Vec::new();
    for keep in [false, true] {
        let p = temp("purge-cost");
        let store = Store::open(&p, 4096, 3650, None).unwrap();
        let start = at("2026-08-01T00:00:00Z");
        let total = PER_DAY * DAYS;
        let mut n = 0;
        while n < total {
            let records: Vec<_> = (n..(n + 10_000).min(total))
                .map(|n| {
                    let (provider, model) = models[(n % 7 % 4) as usize];
                    let mut o = call("proxy", &format!("call-{n}"), provider, start + n * 86_400_000 / PER_DAY);
                    o.actual_model = Some(model.into());
                    o.account_id = Some(format!("{provider}-{:x}-{}", (n / 900) % 4, "0123456789abcdef".repeat(4)));
                    o.client_id = Some(format!("client-{}", (n / 13) % 3));
                    o.response_id = Some(format!("response-{n}"));
                    o.entrypoint = (provider == "anthropic").then(|| if n % 5 == 0 { "sdk-ts" } else { "cli" }.into());
                    o
                })
                .collect();
            n += records.len() as i64;
            store.call(move |c| insert_batch(c, &records)).await.unwrap();
        }
        let cutoff = start + DAYS * 86_400_000;
        let began = std::time::Instant::now();
        store.call(move |c| purge_conn(c, cutoff, keep)).await.unwrap();
        let elapsed = began.elapsed();
        let sizes: Vec<(String, i64, i64)> = store
            .call(|c| {
                let mut out = Vec::new();
                for table in ["usage_history", "usage_history_entrypoints", "usage_history_facets", "usage_history_dims"] {
                    let rows: i64 = c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
                    let bytes: i64 = c.query_row(
                        "SELECT COALESCE(SUM(pgsize),0) FROM dbstat WHERE name=?1 OR name LIKE 'sqlite_autoindex_'||?1||'%'",
                        [table],
                        |r| r.get(0),
                    )?;
                    out.push((table.to_string(), rows, bytes));
                }
                Ok(out)
            })
            .await
            .unwrap();
        let bytes: i64 = sizes.iter().map(|s| s.2).sum();
        results.push(json!({"keep_totals":keep,"purged":total,"seconds":elapsed.as_secs_f64(),
            "kept_bytes_per_day":bytes / DAYS,"kept_bytes_per_year":bytes * 365 / DAYS,
            "tables":sizes.iter().map(|(t, rows, bytes)| json!({"table":t,"rows":rows,"bytes":bytes})).collect::<Vec<_>>()}));
        store.shutdown().await.unwrap();
    }
    println!("{}", serde_json::to_string_pretty(&results).unwrap());
}

/// Where a stored record's bytes go, by table and index, after compression and a
/// vacuum, for records shaped like production proxy calls.
/// `cargo test history_tests::record_bytes -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn record_bytes() {
    const N: i64 = 50_000;
    let p = temp("record-bytes");
    let store = Store::open(&p, 4096, 3650, None).unwrap();
    let start = Utc::now().timestamp_millis() - 5 * 86_400_000;
    let mut n = 0;
    while n < N {
        let records: Vec<_> = (n..(n + 5_000).min(N))
            .map(|n| {
                let anthropic = n % 3 != 0;
                let id = uuid::Uuid::new_v4().to_string();
                let mut o = call("proxy", &id, if anthropic { "anthropic" } else { "openai" }, start + n * 8_000);
                o.parser_version = "proxy-native-v1".into();
                o.actual_model = Some(if anthropic { "claude-opus-5-5" } else { "gpt-6.1-sol" }.into());
                o.requested_model = o.actual_model.clone();
                o.logical_request_id = Some(uuid::Uuid::new_v4().to_string());
                o.attempt_id = Some(id);
                o.session_id = Some(uuid::Uuid::new_v4().to_string());
                o.account_id = Some(format!(
                    "{}-{}@example.com-{:x}.json",
                    if anthropic { "claude" } else { "codex" },
                    "account-name",
                    n % 5
                ));
                o.auth_type = Some("subscription".into());
                o.client_id = Some(format!("client-{}", n % 4));
                o.response_id =
                    Some(format!("{}{}", if anthropic { "msg_01" } else { "resp_" }, uuid::Uuid::new_v4().simple()));
                o.entrypoint = anthropic.then(|| "cli".into());
                o.tokens.cache_read = Some(150_000 + (n % 1000) as u64);
                o
            })
            .collect();
        n += records.len() as i64;
        store.call(move |c| insert_batch(c, &records)).await.unwrap();
    }
    store.shutdown().await.unwrap();
    compact::compact(&p).unwrap();
    let c = Connection::open(&*p).unwrap();
    let mut rows: Vec<(String, i64)> = c
        .prepare("SELECT name,SUM(pgsize) FROM dbstat GROUP BY name ORDER BY 2 DESC")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let total: i64 = rows.iter().map(|r| r.1).sum();
    rows.truncate(20);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"records":N,"bytes_per_record":total / N,
            "by_object":rows.iter().map(|(name, bytes)| json!({"name":name,"bytes_per_record":bytes / N,"share":format!("{:.1}%", *bytes as f64 * 100.0 / total as f64)})).collect::<Vec<_>>()}))
        .unwrap()
    );
}

/// A proxy call and its imported copy a few seconds apart, the purge between them:
/// the response counts once and its copy stays matched, whichever side goes first,
/// until both are gone.
#[tokio::test]
async fn a_call_and_its_import_split_by_the_cutoff_count_once() {
    let p = temp("straddle");
    let store = Store::open(&p, 16, 3650, None).unwrap();
    let pair = |id: &str, proxy_at: &str, import_at: &str| {
        let mut proxy = call("proxy", &format!("{id}-proxy"), "anthropic", at(proxy_at));
        proxy.account_id = Some("acct-a".into());
        proxy.response_id = Some(format!("msg-{id}"));
        let mut import = call("claude_code", &format!("{id}-import"), "anthropic", at(import_at));
        import.response_id = proxy.response_id.clone();
        [proxy, import]
    };
    // Proxy before the cutoff, import after; then the reverse.
    let mut records = pair("a", "2026-10-02T09:59:55Z", "2026-10-02T10:00:05Z").to_vec();
    records.extend(pair("b", "2026-10-02T12:00:05Z", "2026-10-02T11:59:55Z"));
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    let month = queries().pop().unwrap();
    let totals = |store: &Store| {
        let (store, month) = (store.clone(), month.clone());
        async move {
            let totals = store.dashboard(month.clone()).await.unwrap()["combined"]["totals"].clone();
            let scan = store.dashboard_scan(month.clone()).await.unwrap()["combined"]["totals"].clone();
            assert_eq!(scan, totals, "the full scan disagrees");
            let listed =
                store.details(Query { view: Some("combined".into()), ..month }).await.unwrap()["total"].clone();
            (totals["observations"].as_i64().unwrap(), totals["matched"].as_i64().unwrap(), listed)
        }
    };
    assert_eq!(totals(&store).await, (2, 2, json!(2)));
    let unchanged = store.dashboard(month.clone()).await.unwrap()["combined"]["totals"].clone();
    for cutoff in ["2026-10-02T10:00:00Z", "2026-10-02T12:00:00Z"] {
        retain(&store, at(cutoff)).await;
        let (calls, matched, _) = totals(&store).await;
        assert_eq!((calls, matched), (2, 2), "after a purge at {cutoff}");
    }
    assert_eq!(store.dashboard(month.clone()).await.unwrap()["combined"]["totals"], unchanged);
    // Once both sides are gone, nothing is left to match.
    retain(&store, at("2026-10-03T00:00:00Z")).await;
    assert_eq!(totals(&store).await, (2, 2, json!(0)));
    assert_eq!(store.dashboard(month.clone()).await.unwrap()["combined"]["totals"], unchanged);
    let left: i64 =
        store.call(|c| Ok(c.query_row("SELECT COUNT(*) FROM usage_history_matches", [], |r| r.get(0))?)).await.unwrap();
    assert_eq!(left, 0);
    store.shutdown().await.unwrap();
}

/// An explicit purge deletes the proxy call for good: its import then counts, once.
#[tokio::test]
async fn an_explicit_purge_lets_the_surviving_import_count() {
    let p = temp("straddle-explicit");
    let store = Store::open(&p, 16, 3650, None).unwrap();
    let mut proxy = call("proxy", "a-proxy", "anthropic", at("2026-10-02T09:59:55Z"));
    proxy.account_id = Some("acct-a".into());
    proxy.response_id = Some("msg-a".into());
    let mut import = call("claude_code", "a-import", "anthropic", at("2026-10-02T10:00:05Z"));
    import.response_id = proxy.response_id.clone();
    store.call(move |c| insert_batch(c, &[proxy, import])).await.unwrap();
    retain(&store, at("2026-10-02T10:00:00Z")).await;
    store.purge(at("2026-10-02T10:00:00Z")).await.unwrap();
    let totals = store.dashboard(queries().pop().unwrap()).await.unwrap()["combined"]["totals"].clone();
    assert_eq!((totals["observations"].clone(), totals["matched"].clone()), (json!(1), json!(0)));
    store.shutdown().await.unwrap();
}

/// Records that arrive after their response was kept as totals: the proxy call's
/// imported copy stays matched, and another record of the same response is purged.
/// The lists that remember those responses empty once a copy is out of reach.
#[tokio::test]
async fn records_arriving_after_their_response_is_kept_count_once() {
    let p = temp("late");
    let store = Store::open(&p, 16, 3650, None).unwrap();
    let mut proxy = call("proxy", "a-proxy", "anthropic", at("2026-10-02T09:59:55Z"));
    proxy.account_id = Some("acct-a".into());
    proxy.response_id = Some("msg-a".into());
    let mut import = call("claude_code", "a-import", "anthropic", at("2026-10-02T10:00:05Z"));
    import.response_id = proxy.response_id.clone();
    let mut again = proxy.clone();
    again.source_event_id = "a-proxy-again".into();
    again.event_at_ms = at("2026-10-02T10:00:10Z");
    let first = proxy.clone();
    store.call(move |c| insert_batch(c, &[first])).await.unwrap();
    let month = queries().pop().unwrap();
    let totals = |store: &Store| {
        let (store, month) = (store.clone(), month.clone());
        async move {
            let totals = store.dashboard(month.clone()).await.unwrap()["combined"]["totals"].clone();
            assert_eq!(store.dashboard_scan(month).await.unwrap()["combined"]["totals"], totals);
            totals
        }
    };
    let mut expected = totals(&store).await;
    expected["matched"] = json!(1);
    retain(&store, at("2026-10-02T10:00:00Z")).await;
    let result = store.call(move |c| insert_batch(c, &[import, again])).await.unwrap();
    assert_eq!((result["inserted"].clone(), result["purged"].clone()), (json!(1), json!(1)));
    assert_eq!(totals(&store).await, expected);
    retain(&store, at("2026-10-03T00:00:00Z")).await;
    assert_eq!(totals(&store).await, expected);
    let left: (i64, i64) = store
        .call(|c| {
            Ok(c.query_row(
                "SELECT (SELECT COUNT(*) FROM usage_history_matches),(SELECT COUNT(*) FROM usage_history_responses)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(left, (0, 0));
    store.shutdown().await.unwrap();
}

/// Totals written by a build without the lists that keep later records from
/// counting again: reads and inserts work, as they did then.
#[tokio::test]
async fn totals_without_the_later_lists_still_read() {
    let p = temp("no-lists");
    let store = Store::open(&p, 16, 3650, None).unwrap();
    store.call(|c| insert_batch(c, &traffic())).await.unwrap();
    retain(&store, at("2026-10-04T00:00:00Z")).await;
    let month = queries().pop().unwrap();
    let before = store.dashboard(month.clone()).await.unwrap()["combined"]["totals"].clone();
    store
        .call(|c| Ok(c.execute_batch("DROP TABLE usage_history_matches; DROP TABLE usage_history_responses;")?))
        .await
        .unwrap();
    assert_eq!(store.dashboard(month.clone()).await.unwrap()["combined"]["totals"], before);
    store.details(Query { view: Some("combined".into()), ..month }).await.unwrap();
    let late = call("proxy", "late", "anthropic", at("2026-10-04T00:00:01Z"));
    store.call(move |c| insert_batch(c, &[late])).await.unwrap();
    retain(&store, at("2026-10-05T00:00:00Z")).await;
    store.shutdown().await.unwrap();
}
