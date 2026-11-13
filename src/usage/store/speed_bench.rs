//! Manual benchmark: a production-sized journal written with and without timings,
//! the speed read over 1 to 90 days, and a backfill of the whole journal.
//! Run with `cargo test speed_bench -- --ignored --nocapture`.
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

fn journal(count: usize, days: i64, timed: bool) -> Vec<Observation> {
    let now = Utc::now().timestamp_millis();
    let start = now - days * 86_400_000;
    let mut rng = Lcg(7);
    let models = ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "gpt-6.1-sol", "gpt-6-astra"];
    (0..count)
        .map(|i| {
            let source = match rng.next(10) {
                0 => "claude_code",
                1 => "codex",
                _ => "proxy",
            };
            let anthropic = rng.next(3) != 0;
            let mut o =
                Observation::new(source, format!("bench-{i}"), if anthropic { "anthropic" } else { "openai" }, 0);
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
            if !anthropic {
                o.set_requested_speed(Some(if rng.next(3) == 0 { "priority" } else { "default" }));
            }
            if timed && source == "proxy" {
                let ttft = 300 + rng.next(4000);
                o.numeric_metadata.insert("request_ttft_ms".into(), ttft);
                o.numeric_metadata.insert("request_latency_ms".into(), ttft + 1000 + rng.next(60_000));
            }
            o
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual benchmark; run with --ignored --nocapture"]
async fn speed_bench() {
    let days: i64 = std::env::var("BENCH_DAYS").ok().and_then(|v| v.parse().ok()).unwrap_or(90);
    let count: usize = std::env::var("BENCH_CALLS").ok().and_then(|v| v.parse().ok()).unwrap_or(600_000);
    let dir = std::env::temp_dir().join(format!("fusebox-speed-bench-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut out = serde_json::Map::new();
    out.insert("calls".into(), json!(count));
    out.insert("days".into(), json!(days));
    for timed in [false, true] {
        let path = dir.join(format!("bench-{timed}.sqlite"));
        let store = Store::open(&path, 4096, 3650, None).unwrap();
        let records = journal(count, days, timed);
        let started = Instant::now();
        let mut samples = Vec::new();
        for chunk in records.chunks(256) {
            let chunk = chunk.to_vec();
            let at = Instant::now();
            store.call(move |c| insert_batch(c, &chunk)).await.unwrap();
            samples.push(at.elapsed());
        }
        let name = if timed { "with_timings" } else { "without_timings" };
        out.insert(format!("{name}_write_total_s"), json!(started.elapsed().as_secs_f64()));
        out.insert(format!("{name}_write_batch_median_ms"), json!(median(samples)));
        store.call(|c| Ok(c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?)).await.unwrap();
        out.insert(format!("{name}_bytes"), json!(std::fs::metadata(&path).unwrap().len()));
        if !timed {
            store.shutdown().await.unwrap();
            continue;
        }
        let rows: i64 = store
            .call(|c| Ok(c.query_row("SELECT COUNT(*) FROM usage_speed_rollup", [], |r| r.get(0))?))
            .await
            .unwrap();
        out.insert("speed_rollup_rows".into(), json!(rows));
        let today = Utc::now().date_naive();
        for span in [1_u64, 7, 30, 90] {
            let q = Query {
                start: Some((today - chrono::Days::new(span - 1)).to_string()),
                end: Some((today + chrono::Days::new(1)).to_string()),
                timezone: Some("Europe/Amsterdam".into()),
                ..Default::default()
            };
            store.speed(q.clone()).await.unwrap();
            let mut samples = Vec::new();
            for _ in 0..15 {
                let at = Instant::now();
                store.speed(q.clone()).await.unwrap();
                samples.push(at.elapsed());
            }
            out.insert(format!("speed_{span}d_ms"), json!(median(samples)));
        }
        // The backfill of a journal written by a release without timings.
        let backfill = store
            .call(|c| {
                c.execute_batch("DELETE FROM usage_timings; DELETE FROM usage_speed_rollup;")?;
                let below: i64 = c.query_row("SELECT MAX(id)+1 FROM usage_observations", [], |r| r.get(0))?;
                c.execute("INSERT INTO usage_meta(key,value) VALUES('speed_backfill_below',?1)", [below.to_string()])?;
                let started = Instant::now();
                let mut steps = Vec::new();
                loop {
                    let at = Instant::now();
                    if speed::backfill_step(c, 2_000)? == 0 {
                        break;
                    }
                    steps.push(at.elapsed());
                }
                Ok((started.elapsed(), median(steps)))
            })
            .await
            .unwrap();
        out.insert("backfill_total_s".into(), json!(backfill.0.as_secs_f64()));
        out.insert("backfill_step_median_ms".into(), json!(backfill.1));
        store.shutdown().await.unwrap();
    }
    println!("{}", Value::Object(out));
    if std::env::var_os("BENCH_KEEP").is_some() {
        println!("kept {}", dir.display());
    } else {
        let _ = std::fs::remove_dir_all(&dir);
    }
}
