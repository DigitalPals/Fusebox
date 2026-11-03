//! Manual benchmark: the same production-sized journal written and read by this build.
//! Run with `cargo test entrypoint_bench -- --ignored --nocapture`; the line marked
//! ENTRYPOINT is the only one an older build lacks.
use super::super::types::Tokens;
use super::*;
use std::time::{Duration, Instant};

struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

fn median(mut samples: Vec<Duration>) -> f64 {
    samples.sort();
    samples[samples.len() / 2].as_secs_f64() * 1000.0
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual benchmark; run with --ignored --nocapture"]
async fn entrypoint_bench() {
    let dir = std::env::temp_dir().join(format!("fusebox-entrypoint-bench-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bench.sqlite");
    let store = Store::open(&path, 4096, 3650, None).unwrap();
    let days: i64 = std::env::var("BENCH_DAYS").ok().and_then(|v| v.parse().ok()).unwrap_or(90);
    let count: usize = std::env::var("BENCH_CALLS").ok().and_then(|v| v.parse().ok()).unwrap_or(600_000);
    let now = Utc::now().timestamp_millis();
    let start = now - days * 86_400_000;
    let mut rng = Lcg(7);
    let models = ["claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5", "gpt-6.1-sol", "gpt-6.1-sol-mini"];
    let mut records = Vec::with_capacity(count);
    for i in 0..count {
        let source = match rng.next(10) {
            0 => "claude_code",
            1 => "codex",
            _ => "proxy",
        };
        let anthropic = rng.next(3) != 0;
        let provider = if anthropic { "anthropic" } else { "openai" };
        let mut o = Observation::new(source, format!("bench-{i}"), provider, 0);
        // Calls in time order, as a live proxy writes them.
        o.event_at_ms = start + (i as i64 * days * 86_400_000) / count as i64;
        o.actual_model = Some(models[if anthropic { rng.next(3) } else { 3 + rng.next(2) } as usize].into());
        o.account_id = Some(format!("account-{}", rng.next(7)));
        o.client_id = (rng.next(5) != 0).then(|| format!("client-{}", rng.next(12)));
        o.response_id = Some(format!("response-{i}"));
        o.completeness = "complete".into();
        o.tokens = Tokens {
            input: Some(rng.next(5000)),
            cache_read: Some(rng.next(200_000)),
            cache_write: Some(rng.next(4000)),
            output: Some(200 + rng.next(4000)),
            reasoning: Some(rng.next(200)),
            ..Default::default()
        };
        let label =
            (source == "proxy" && anthropic).then(|| ["sdk-ts", "cli", "t3-code", "sdk-cli"][rng.next(4) as usize]);
        let _ = &label;
        o.entrypoint = label.map(Into::into); // ENTRYPOINT
        records.push(o);
    }
    // Writes, in the writer's batch size.
    let started = Instant::now();
    let mut write_samples = Vec::new();
    for chunk in records.chunks(256) {
        let chunk = chunk.to_vec();
        let at = Instant::now();
        store.call(move |c| insert_batch(c, &chunk)).await.unwrap();
        write_samples.push(at.elapsed());
    }
    let write_total = started.elapsed();
    let bytes = std::fs::metadata(&path).unwrap().len()
        + std::fs::metadata(dir.join("bench.sqlite-wal")).map(|m| m.len()).unwrap_or(0);
    store.call(|c| Ok(c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?)).await.unwrap();
    let size = std::fs::metadata(&path).unwrap().len();
    let rollup_rows: i64 =
        store.call(|c| Ok(c.query_row("SELECT COUNT(*) FROM usage_rollup", [], |r| r.get(0))?)).await.unwrap();
    let today = Utc::now().date_naive();
    let mut reads = serde_json::Map::new();
    for span in [1_i64, 7, 30, 90] {
        let q = Query {
            start: Some((today - chrono::Days::new(span as u64 - 1)).to_string()),
            end: Some((today + chrono::Days::new(1)).to_string()),
            timezone: Some("Europe/Amsterdam".into()),
            ..Default::default()
        };
        store.dashboard(q.clone()).await.unwrap();
        let mut samples = Vec::new();
        for _ in 0..15 {
            let at = Instant::now();
            store.dashboard(q.clone()).await.unwrap();
            samples.push(at.elapsed());
        }
        reads.insert(format!("dashboard_{span}d_ms"), json!(median(samples)));
        let mut samples = Vec::new();
        let only = Query { entrypoint: Some("sdk-ts".into()), ..q.clone() }; // ENTRYPOINT
        for _ in 0..15 {
            let at = Instant::now();
            store.entrypoints(q.clone()).await.unwrap(); // ENTRYPOINT
            samples.push(at.elapsed());
        }
        reads.insert(format!("entrypoints_{span}d_ms"), json!(median(samples)));
        let mut samples = Vec::new();
        for _ in 0..15 {
            let at = Instant::now();
            store.dashboard(only.clone()).await.unwrap(); // ENTRYPOINT
            samples.push(at.elapsed());
        }
        reads.insert(format!("dashboard_sdk_ts_{span}d_ms"), json!(median(samples)));
    }
    println!(
        "{}",
        json!({"calls":count,"days":days,"write_total_s":write_total.as_secs_f64(),
            "write_batch_median_ms":median(write_samples),"database_bytes":size,"before_checkpoint_bytes":bytes,
            "usage_rollup_rows":rollup_rows,"reads":reads})
    );
    store.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Cross-version check, in three steps on one database file (`ENTRYPOINT_COMPAT_DB`):
/// `seed` with this build, `older` with a build from before entrypoints (this file
/// without the ENTRYPOINT lines), then `verify` with this build again.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual cross-version check; see the comment"]
async fn entrypoint_compat() {
    let path = PathBuf::from(std::env::var("ENTRYPOINT_COMPAT_DB").unwrap());
    let step = std::env::var("ENTRYPOINT_COMPAT_STEP").unwrap();
    let now = Utc::now().timestamp_millis();
    let calls = |prefix: &str, labelled: bool| -> Vec<Observation> {
        (0..3000)
            .map(|i| {
                let mut o = Observation::new(
                    "proxy",
                    format!("{prefix}-{i}"),
                    "anthropic",
                    now - 86_400_000 * 3 + i as i64 * 60_000,
                );
                o.actual_model = Some("claude-sonnet-4-6".into());
                o.account_id = Some(format!("account-{}", i % 3));
                o.client_id = Some(format!("client-{}", i % 4));
                o.response_id = Some(format!("{prefix}-response-{i}"));
                o.completeness = "complete".into();
                o.tokens = Tokens {
                    input: Some(1000),
                    cache_read: Some(100),
                    cache_write: Some(0),
                    output: Some(100 + i as u64),
                    reasoning: Some(0),
                    ..Default::default()
                };
                let _ = labelled;
                o.entrypoint = labelled.then(|| ["sdk-ts", "cli", "t3-code"][i % 3].to_string()); // ENTRYPOINT
                o
            })
            .collect()
    };
    let store = Store::open(&path, 4096, 3650, None).unwrap();
    let everything = Query {
        start: Some("2026-01-01".into()),
        end: Some("2027-01-01".into()),
        timezone: Some("UTC".into()),
        ..Default::default()
    };
    match step.as_str() {
        "seed" => {
            let records = calls("seed", true);
            store.call(move |c| insert_batch(c, &records)).await.unwrap();
        }
        "older" => {
            // Write, purge through the labelled calls, rebuild entries, and read.
            let records = calls("older", false);
            store.call(move |c| insert_batch(c, &records)).await.unwrap();
            store.purge(now - 86_400_000 * 3 + 1000 * 60_000).await.unwrap();
            store
                .call(|c| {
                    let keys: Vec<String> = c
                        .prepare("SELECT canonical_key FROM usage_observations WHERE id%5=0")?
                        .query_map([], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?;
                    for key in keys {
                        rebuild_entries(c, &key)?;
                    }
                    Ok(())
                })
                .await
                .unwrap();
            let value = store.dashboard(everything.clone()).await.unwrap();
            println!("older build read {} calls", value["combined"]["totals"]["observations"]);
        }
        "verify" => {
            let value = store.dashboard(everything.clone()).await.unwrap();
            assert_eq!(value, store.dashboard_scan(everything.clone()).await.unwrap()); // ENTRYPOINT
            let rows = store.entrypoints(everything.clone()).await.unwrap()["breakdown"].clone(); // ENTRYPOINT
            println!("{}", json!({"totals":value["combined"]["totals"]["observations"],"entrypoint":rows})); // ENTRYPOINT
            store
                .call(|c| {
                    let sums = (0..rollup::AMOUNTS.len()).map(|i| format!("k{i},s{i}")).collect::<Vec<_>>().join(","); // ENTRYPOINT
                    let sql = format!("SELECT json_array(bucket,dims,entrypoint,observations,{sums}) FROM usage_entrypoint_rollup ORDER BY bucket,dims"); // ENTRYPOINT
                    let dump = |c: &Connection| -> Result<Vec<String>> { Ok(c.prepare(&sql)?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?) }; // ENTRYPOINT
                    let live = dump(c)?; // ENTRYPOINT
                    let tx = c.transaction()?; // ENTRYPOINT
                    tx.execute_batch("DELETE FROM usage_entrypoint_rollup;")?; // ENTRYPOINT
                    rollup::backfill_entrypoints(&tx)?; // ENTRYPOINT
                    assert_eq!(live, dump(&tx)?, "the older build left the entrypoint rollup inexact"); // ENTRYPOINT
                    tx.rollback()?; // ENTRYPOINT
                    Ok(())
                })
                .await
                .unwrap();
        }
        other => panic!("unknown step {other}"),
    }
    store.shutdown().await.unwrap();
}

/// The only work on the request path: reading the entrypoint from the User-Agent.
#[test]
#[ignore = "manual benchmark; run with --ignored --nocapture"]
fn entrypoint_parse_bench() {
    use axum::http::{HeaderMap, HeaderValue, header::USER_AGENT};
    let mut claude = HeaderMap::new();
    claude.insert(USER_AGENT, HeaderValue::from_static("claude-cli/2.1.304 (external, sdk-ts, agent-sdk/0.3.276)"));
    let mut other = HeaderMap::new();
    other.insert(USER_AGENT, HeaderValue::from_static("codex_cli_rs/0.163.0 (Mac OS 26.0; arm64) iTerm.app/3.6.6"));
    for (name, headers) in [("claude", &claude), ("other", &other)] {
        let started = Instant::now();
        for _ in 0..1_000_000 {
            std::hint::black_box(crate::usage::capture::claude_code_entrypoint(std::hint::black_box(headers)));
        }
        println!("{name}: {:.0} ns per request", started.elapsed().as_nanos() as f64 / 1e6);
    }
}
