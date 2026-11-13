//! Output speed per model: what the Usage page reads, that the rollup agrees with a
//! scan of the entries, that the triggers stay exact whichever table changes, and
//! that the backfill gives stored calls the same timings live recording would have.
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
    let dir = std::env::temp_dir().join(format!("fusebox-speed-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    TempDb { path: dir.join(format!("{name}.sqlite")), dir }
}

const DAY: &str = "2026-10-20";

/// A proxy call of `output` tokens that took `total` ms, the first content after `ttft`.
fn call(id: &str, model: &str, at: &str, output: u64, ttft: Option<u64>, total: u64) -> Observation {
    let provider = if model.starts_with("claude") { "anthropic" } else { "openai" };
    let mut o = Observation::new("proxy", id.into(), provider, boundary(at, &crate::zone::Zone::utc()).unwrap());
    o.actual_model = Some(model.into());
    o.account_id = Some("account-1".into());
    o.response_id = Some(format!("response-{id}"));
    o.completeness = "complete".into();
    o.tokens = Tokens {
        input: Some(1000),
        cache_read: Some(0),
        cache_write: Some(0),
        output: Some(output),
        reasoning: Some(0),
        ..Default::default()
    };
    o.numeric_metadata.insert("request_latency_ms".into(), total);
    if let Some(ttft) = ttft {
        o.numeric_metadata.insert("request_ttft_ms".into(), ttft);
    }
    if provider == "openai" {
        o.set_requested_speed(Some("default"));
    }
    o
}
fn fast(mut o: Observation) -> Observation {
    o.set_requested_speed(Some("priority"));
    // Codex Fast calls can come back reporting the default tier; the request decides.
    o.service_tier = Some("default".into());
    o
}

fn day() -> Query {
    Query {
        start: Some(DAY.into()),
        end: Some("2026-10-21".into()),
        timezone: Some("Europe/Amsterdam".into()),
        ..Default::default()
    }
}

fn model<'a>(v: &'a Value, model: &str, speed: &str) -> &'a Value {
    v["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["model"] == model && r["speed"] == speed)
        .unwrap_or_else(|| panic!("no row {model} {speed}"))
}

fn dump(c: &Connection) -> Result<Vec<String>> {
    Ok(c.prepare(
        "SELECT json_array(bucket,dims,provider,model,account_id,client,speed,calls,timed,output,stream_ms,ttft_ms)
         FROM usage_speed_rollup ORDER BY bucket,dims",
    )?
    .query_map([], |r| r.get::<_, String>(0))?
    .collect::<rusqlite::Result<_>>()?)
}

/// The triggers must leave exactly what a rebuild from entries and timings produces.
fn assert_exact(c: &mut Connection) -> Result<()> {
    let live = dump(c)?;
    let tx = c.transaction()?;
    speed::rebuild(&tx)?;
    let rebuilt = dump(&tx)?;
    tx.rollback()?;
    assert_eq!(live, rebuilt, "the speed rollup drifted from the journal");
    Ok(())
}

fn records() -> Vec<Observation> {
    vec![
        // Opus: 2 timed calls, 3000 tokens in 60 s of streaming.
        call("a", "claude-opus-5-5", "2026-10-20T08:00:00Z", 1000, Some(2000), 22_000),
        call("b", "claude-opus-5-5", "2026-10-20T08:07:30Z", 2000, Some(4000), 44_000),
        // No content arrived: counted, not timed.
        call("c", "claude-opus-5-5", "2026-10-20T08:30:00Z", 0, None, 5000),
        // Everything arrived at once: too short a stream to time.
        call("d", "claude-opus-5-5", "2026-10-20T09:00:00Z", 800, Some(3000), 3010),
        // Codex standard and Fast are separate rows of the same model.
        call("e", "gpt-6.1-sol", "2026-10-20T10:00:00Z", 720, Some(1000), 11_000),
        fast(call("f", "gpt-6.1-sol", "2026-10-20T10:20:00Z", 1240, Some(500), 10_500)),
        // The day before: the previous period of a one-day range.
        call("g", "claude-opus-5-5", "2026-10-19T12:00:00Z", 500, Some(1000), 11_000),
    ]
}

#[tokio::test]
async fn the_usage_page_reads_speed_per_model_and_tier() {
    let p = temp("page");
    let store = Store::open(&p, 64, 3650, None).unwrap();
    let mut batch = records();
    // Imported history carries no timings.
    let mut imported = call("h", "claude-opus-5-5", "2026-10-20T11:00:00Z", 500, Some(10), 20);
    imported.source = "claude_code".into();
    imported.numeric_metadata.clear();
    batch.push(imported);
    store.call(move |c| insert_batch(c, &batch)).await.unwrap();

    let value = store.speed(day()).await.unwrap();
    assert_eq!(value, store.speed_scan(day()).await.unwrap());
    let opus = model(&value, "claude-opus-5-5", "standard");
    assert_eq!((opus["calls"].as_i64(), opus["timed"].as_i64()), (Some(4), Some(2)));
    assert_eq!(
        (opus["output"].as_i64(), opus["stream_ms"].as_i64(), opus["ttft_ms"].as_i64()),
        (Some(3000), Some(60_000), Some(6000))
    );
    assert_eq!(opus["provider"], "anthropic");
    let standard = model(&value, "gpt-6.1-sol", "standard");
    assert_eq!((standard["output"].as_i64(), standard["stream_ms"].as_i64()), (Some(720), Some(10_000)));
    let fast = model(&value, "gpt-6.1-sol", "fast");
    assert_eq!((fast["output"].as_i64(), fast["stream_ms"].as_i64()), (Some(1240), Some(10_000)), "the requested tier");
    assert_eq!(value["models"].as_array().unwrap().len(), 3);
    assert_eq!(value["previous"].as_array().unwrap().len(), 1);
    assert_eq!(value["previous"][0]["output"], 500);
    let trend = value["trend"].as_array().unwrap();
    assert!(trend.iter().all(|t| t["date"] == DAY));
    assert_eq!(trend.len(), 3);
    assert_eq!(value["backfilling"], false);

    // A range that starts and ends inside buckets reads those ends from entries.
    let partial = Query {
        start: Some("2026-10-20T08:05:00Z".into()),
        end: Some("2026-10-20T10:10:00Z".into()),
        timezone: Some("UTC".into()),
        ..Default::default()
    };
    let ends = store.speed(partial.clone()).await.unwrap();
    assert_eq!(ends, store.speed_scan(partial).await.unwrap());
    assert_eq!(model(&ends, "claude-opus-5-5", "standard")["calls"], 3);
    assert_eq!(model(&ends, "gpt-6.1-sol", "standard")["calls"], 1);

    for q in [
        Query { model: Some("gpt-6.1-sol".into()), ..day() },
        Query { provider: Some("anthropic".into()), ..day() },
        Query { account: Some("account-2".into()), ..day() },
        Query { source: Some("proxy".into()), ..day() },
        Query { entrypoint: Some("sdk-ts".into()), ..day() },
    ] {
        assert_eq!(store.speed(q.clone()).await.unwrap(), store.speed_scan(q).await.unwrap());
    }
    let one = store.speed(Query { model: Some("gpt-6.1-sol".into()), ..day() }).await.unwrap();
    assert_eq!(one["models"].as_array().unwrap().len(), 2);
    let imports = store.speed(Query { source: Some("claude_code".into()), ..day() }).await.unwrap();
    assert_eq!(imports["models"], json!([]));
    store.call(assert_exact).await.unwrap();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_rollup_stays_exact_through_rebuilds_purges_and_older_releases() {
    let p = temp("exact");
    let store = Store::open(&p, 64, 3650, None).unwrap();
    let records: Vec<_> = (0..40)
        .map(|i| {
            let at = format!("2026-10-20T{:02}:{:02}:00Z", 6 + i / 4, (i % 4) * 15 + 3);
            let o = call(
                &format!("call-{i}"),
                ["claude-opus-5-5", "gpt-6.1-sol"][i % 2],
                &at,
                100 + i as u64,
                Some(500),
                5000,
            );
            if i % 3 == 0 { fast(o) } else { o }
        })
        .collect();
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    store
        .call(|c| {
            assert_exact(c)?;
            // An older release deletes observations before entries when it purges.
            let cut = boundary("2026-10-20T09:00:00Z", &crate::zone::Zone::utc())?;
            c.execute("DELETE FROM usage_observations WHERE event_at_ms<?1", [cut])?;
            c.execute("DELETE FROM usage_entries WHERE event_at_ms<?1", [cut])?;
            assert_exact(c)?;
            // Repricing rebuilds entries through delete and insert.
            let keys: Vec<String> = c
                .prepare("SELECT canonical_key FROM usage_observations WHERE id%3=0")?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for key in keys {
                rebuild_entries(c, &key)?;
            }
            c.execute("UPDATE usage_entries SET output=output+1 WHERE id%2=0", [])?;
            assert_exact(c)
        })
        .await
        .unwrap();
    assert_eq!(store.speed(day()).await.unwrap(), store.speed_scan(day()).await.unwrap());
    let cut = boundary("2026-10-20T12:00:00Z", &crate::zone::Zone::utc()).unwrap();
    store.purge(cut).await.unwrap();
    store
        .call(move |c| {
            assert_exact(c)?;
            let stale: i64 = c.query_row(
                "SELECT COUNT(*) FROM usage_timings t JOIN usage_observations o ON o.id=t.id WHERE o.event_at_ms<?1",
                [cut],
                |r| r.get(0),
            )?;
            assert_eq!(stale, 0, "a purge removes the timings of what it purges");
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(store.speed(day()).await.unwrap(), store.speed_scan(day()).await.unwrap());
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_backfill_gives_stored_calls_their_timings_newest_first() {
    let live = temp("live");
    let older = temp("older");
    let batch: Vec<_> = (0..25)
        .map(|i| {
            let at = format!("2026-10-20T{:02}:{:02}:00Z", 6 + i / 4, (i % 4) * 15 + 1);
            let o = call(
                &format!("call-{i}"),
                ["claude-opus-5-5", "gpt-6.1-sol"][i % 2],
                &at,
                300 + i as u64,
                Some(800),
                9000,
            );
            if i % 4 == 0 { fast(o) } else { o }
        })
        .collect();
    let recorded = Store::open(&live, 64, 3650, None).unwrap();
    let copy = batch.clone();
    recorded.call(move |c| insert_batch(c, &copy)).await.unwrap();
    let expected = recorded.speed(day()).await.unwrap();
    recorded.shutdown().await.unwrap();

    // A database written by a release without timings.
    {
        let store = Store::open(&older, 64, 3650, None).unwrap();
        store.call(move |c| insert_batch(c, &batch)).await.unwrap();
        store.shutdown().await.unwrap();
        let c = Connection::open(&*older).unwrap();
        for trigger in speed::TRIGGERS {
            c.execute_batch(&format!("DROP TRIGGER {trigger};")).unwrap();
        }
        c.execute_batch("DROP TABLE usage_timings; DROP TABLE usage_speed_rollup;").unwrap();
    }
    let mut c = Connection::open(&*older).unwrap();
    codec::register(&c).unwrap();
    migrate(&mut c).unwrap();
    assert!(speed::backfilling(&c).unwrap());
    assert_eq!(dump(&c).unwrap().len(), 0, "nothing until the backfill runs");
    // Newest first: the first step covers the latest calls.
    assert_eq!(speed::backfill_step(&mut c, 10).unwrap(), 10);
    let newest: i64 = c.query_row("SELECT MIN(id) FROM usage_timings", [], |r| r.get(0)).unwrap();
    let max: i64 = c.query_row("SELECT MAX(id) FROM usage_observations", [], |r| r.get(0)).unwrap();
    assert_eq!(newest, max - 9);
    assert_exact(&mut c).unwrap();
    // A new call while the backfill runs is recorded at once and not read twice.
    let late = call("late", "claude-opus-5-5", "2026-10-20T20:00:00Z", 900, Some(600), 6600);
    insert_batch(&mut c, &[late]).unwrap();
    while speed::backfill_step(&mut c, 10).unwrap() > 0 {}
    assert!(!speed::backfilling(&c).unwrap());
    assert_exact(&mut c).unwrap();
    drop(c);
    let store = Store::open(&older, 64, 3650, None).unwrap();
    let mut value = store.speed(day()).await.unwrap();
    let late = model(&value, "claude-opus-5-5", "standard")["calls"].as_i64().unwrap();
    assert_eq!(late, model(&expected, "claude-opus-5-5", "standard")["calls"].as_i64().unwrap() + 1);
    // Without the late call, the backfilled database reads as if it had recorded live.
    let without = Query { end: Some("2026-10-20T19:00:00Z".into()), ..day() };
    value = store.speed(without.clone()).await.unwrap();
    let recorded = Store::open(&live, 64, 3650, None).unwrap();
    assert_eq!(value["models"], recorded.speed(without).await.unwrap()["models"]);
    assert_eq!(value["models"], expected["models"]);
    recorded.shutdown().await.unwrap();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_store_backfills_on_its_own_and_a_downgrade_removes_the_tables() {
    let p = temp("downgrade");
    {
        let store = Store::open(&p, 64, 3650, None).unwrap();
        store.call(move |c| insert_batch(c, &records())).await.unwrap();
        store.shutdown().await.unwrap();
        let c = Connection::open(&*p).unwrap();
        c.execute_batch("DELETE FROM usage_timings; DELETE FROM usage_speed_rollup;").unwrap();
        c.execute("INSERT INTO usage_meta(key,value) VALUES('speed_backfill_below','1000000')", []).unwrap();
    }
    let store = Store::open(&p, 64, 3650, None).unwrap();
    let mut value = store.speed(day()).await.unwrap();
    for _ in 0..100 {
        if value["backfilling"] == false {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        value = store.speed(day()).await.unwrap();
    }
    assert_eq!(value["backfilling"], false, "the writer finishes the backfill");
    assert_eq!(model(&value, "claude-opus-5-5", "standard")["timed"], 2);
    store.shutdown().await.unwrap();
    drop(store);
    let objects = |c: &Connection| -> Vec<String> {
        c.prepare("SELECT name FROM sqlite_master WHERE name LIKE 'usage_speed%' OR name='usage_timings' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    {
        let mut c = Connection::open(&*p).unwrap();
        assert_eq!(objects(&c).len(), 2 + speed::TRIGGERS.len(), "two tables and their triggers");
        let before = c.query_row("PRAGMA schema_version", [], |r| r.get::<_, i64>(0)).unwrap();
        migrate(&mut c).unwrap();
        assert_eq!(
            c.query_row("PRAGMA schema_version", [], |r| r.get::<_, i64>(0)).unwrap(),
            before,
            "opening again changes nothing"
        );
        assert!(!speed::backfilling(&c).unwrap());
    }
    compact::downgrade(&p).unwrap();
    assert!(objects(&Connection::open(&*p).unwrap()).is_empty());
}

#[test]
fn speed_follows_the_request_for_codex_and_the_report_for_claude() {
    let mut codex = call("x", "gpt-6.1-sol", "2026-10-20T08:00:00Z", 10, Some(1), 100);
    assert_eq!(speed::timing(&codex), Some((1, Some(1), 100)));
    codex = fast(codex);
    assert_eq!(speed::timing(&codex).unwrap().0, 2);
    let mut claude = call("y", "claude-opus-5-5", "2026-10-20T08:00:00Z", 10, Some(1), 100);
    claude.service_tier = Some("fast".into());
    assert_eq!(speed::timing(&claude).unwrap().0, 2);
    // A first-content time past the total is not a time.
    let odd = call("z", "claude-opus-5-5", "2026-10-20T08:00:00Z", 10, Some(500), 100);
    assert_eq!(speed::timing(&odd), Some((1, None, 100)));
    let mut imported = call("w", "claude-opus-5-5", "2026-10-20T08:00:00Z", 10, Some(1), 100);
    imported.source = "codex".into();
    assert_eq!(speed::timing(&imported), None);
}

#[test]
fn only_a_live_stream_measures_speed() {
    use super::super::types::stream_ms;
    assert_eq!(stream_ms(1000, Some(2000), 22_000), Some(20_000));
    assert_eq!(stream_ms(1000, None, 22_000), None, "no content arrived");
    assert_eq!(stream_ms(0, Some(2000), 22_000), None, "no output reported");
    assert_eq!(stream_ms(800, Some(3000), 3049), None, "one read, not a stream");
    assert_eq!(stream_ms(800, Some(3000), 3050), None, "16,000 tokens a second arrived buffered");
    assert_eq!(stream_ms(500, Some(3000), 3050), Some(50), "10,000 a second is the limit");
    assert_eq!(stream_ms(10, Some(5000), 100), None, "a first-content time past the total");
}
