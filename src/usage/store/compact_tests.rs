//! Schema 4: rollups must agree with a full scan, compression must be invisible to
//! readers, and a downgrade must give older releases back exactly what they wrote.
use super::super::types::Tokens;
use super::compact::lock;
use super::*;
use std::time::Duration;

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
    let dir = std::env::temp_dir().join(format!("fusebox-compact-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    TempDb { path: dir.join(format!("{name}.sqlite")), dir }
}

/// Deterministic pseudo-random numbers, so a failure reproduces.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

const START: &str = "2026-03-27T00:00:00Z";

/// A varied journal: several providers, models, accounts and clients; unpriced,
/// partial and missing usage; duplicate proxy evidence; imported copies of proxy
/// responses (matched) and imports of their own; collector origins; times on and
/// off bucket boundaries across the Amsterdam DST change on 29 March.
fn journal(seed: u64, count: usize) -> Vec<Observation> {
    let mut rng = Lcg(seed);
    let base = boundary(START, chrono_tz::UTC).unwrap();
    let mut out = Vec::new();
    for i in 0..count {
        let source = match rng.next(10) {
            0 => "codex",
            1 => "claude_code",
            _ => "proxy",
        };
        let provider = if rng.next(2) == 0 { "anthropic" } else { "openai" };
        let mut o = Observation::new(source, format!("event-{seed}-{i}"), provider, 0);
        // Mostly arbitrary milliseconds, sometimes exactly on a bucket edge.
        o.event_at_ms = if rng.next(8) == 0 {
            base + rng.next(4 * 96) as i64 * rollup::BUCKET_MS
        } else {
            base + rng.next(4 * 86_400_000) as i64
        };
        o.actual_model = match rng.next(5) {
            0 => None,
            1 => Some("unpriced-model".into()),
            _ if provider == "anthropic" => Some("claude-sonnet-4-6".into()),
            _ => Some("gpt-6.1-sol".into()),
        };
        o.account_id = (rng.next(4) != 0).then(|| format!("account-{}", rng.next(3)));
        o.client_id = (rng.next(3) == 0).then(|| format!("client-{}", rng.next(2)));
        if source != "proxy" && rng.next(2) == 0 {
            o.origin_id = "collector:laptop".into();
        }
        o.response_id = (rng.next(3) != 0).then(|| format!("response-{}", rng.next(count as u64 / 2 + 1)));
        o.completeness = "complete".into();
        o.tokens = Tokens {
            input: Some(rng.next(5000)),
            cache_read: Some(rng.next(100_000)),
            cache_write: Some(rng.next(2000)),
            output: Some(500 + rng.next(3000)),
            reasoning: Some(rng.next(500)),
            ..Default::default()
        };
        match rng.next(12) {
            0 => {
                o.tokens = Tokens::default();
                o.completeness = "missing".into();
            }
            1 => {
                o.tokens.cache_write = None;
                o.completeness = "partial".into();
            }
            _ => {}
        }
        out.push(o);
    }
    out
}

fn queries() -> Vec<Query> {
    let zones =
        ["UTC", "Europe/Amsterdam", "Asia/Kathmandu", "America/St_Johns", "Australia/Lord_Howe", "Pacific/Chatham"];
    let ranges = [
        (Some("2026-03-26"), Some("2026-04-01")),
        (Some("2026-03-28"), Some("2026-03-30")),
        (Some("2026-03-27T03:07:11Z"), Some("2026-03-29T19:52:00.5Z")),
        (Some("2026-03-28T10:00:00Z"), Some("2026-03-28T10:15:00Z")),
        (Some("2026-03-28T10:01:00Z"), Some("2026-03-28T10:14:00Z")),
    ];
    let mut out = Vec::new();
    for zone in zones {
        for (start, end) in ranges {
            out.push(Query {
                start: start.map(Into::into),
                end: end.map(Into::into),
                timezone: Some(zone.into()),
                ..Default::default()
            });
        }
    }
    let base = Query {
        start: Some("2026-03-26".into()),
        end: Some("2026-04-01".into()),
        timezone: Some("Europe/Amsterdam".into()),
        ..Default::default()
    };
    for f in [
        |q: &mut Query| q.provider = Some("anthropic".into()),
        |q: &mut Query| q.model = Some("claude-sonnet-4-6".into()),
        |q: &mut Query| q.account = Some("account-1".into()),
        |q: &mut Query| q.client = Some("client-0".into()),
        |q: &mut Query| q.client = Some("collector:laptop".into()),
        |q: &mut Query| q.source = Some("proxy".into()),
        |q: &mut Query| q.source = Some("codex".into()),
        |q: &mut Query| {
            q.source = Some("proxy".into());
            q.stack = Some("model".into());
        },
    ] {
        let mut q = base.clone();
        f(&mut q);
        out.push(q);
    }
    out
}

async fn assert_rollups_match(store: &Store) {
    for q in queries() {
        let fast = store.dashboard(q.clone()).await.unwrap();
        let scan = store.dashboard_scan(q.clone()).await.unwrap();
        assert_eq!(fast, scan, "rollup summary differs for {q:?}");
    }
    // The triggers must leave exactly what a rebuild from scratch produces.
    store
        .call(|c| {
            let dump = |c: &Connection| -> Result<(Vec<String>, Vec<String>)> {
                let rows = |sql: &str| -> Result<Vec<String>> {
                    Ok(c.prepare(sql)?.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?)
                };
                let sums = (0..8).map(|i| format!("k{i},s{i}")).collect::<Vec<_>>().join(",");
                Ok((
                    rows(&format!(
                        "SELECT json_array(bucket,dims,provider,model,account_id,client,observations,partial,missing,pricing_partial,{sums}) FROM usage_rollup ORDER BY bucket,dims"
                    ))?,
                    rows("SELECT json_array(bucket,dims,source,observations) FROM usage_facets ORDER BY bucket,dims")?,
                ))
            };
            let live = dump(c)?;
            let tx = c.transaction()?;
            tx.execute_batch("DELETE FROM usage_rollup; DELETE FROM usage_facets;")?;
            rollup::backfill(&tx)?;
            let rebuilt = dump(&tx)?;
            tx.rollback()?;
            assert_eq!(live, rebuilt, "rollups drifted from the journal");
            assert!(!live.0.is_empty());
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn rollups_match_a_full_scan_through_inserts_purges_suppression_and_reprices() {
    let p = temp("rollups");
    let store = Store::open(&p, 64, 3650, None).unwrap();
    let records = journal(7, 1500);
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    assert_rollups_match(&store).await;

    // Suppressing native fallback evidence moves accounting winners around.
    store
        .call(|c| {
            for i in (0..1500).step_by(37) {
                suppress_origin_event(c, "codex", &format!("event-7-{i}"), "", "test")?;
            }
            Ok(())
        })
        .await
        .unwrap();
    // More evidence for the same responses, then a purge through the middle of a bucket.
    let records = journal(8, 600);
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    let cut = boundary("2026-03-27T09:07:30Z", chrono_tz::UTC).unwrap();
    store.purge(cut).await.unwrap();
    // Repricing rewrites entries in place through delete and insert.
    store
        .call(|c| {
            c.execute(
                "UPDATE usage_observations SET cost_nanos=NULL,pricing_basis='outside_effective_period' WHERE id%9=0 AND model='gpt-6.1-sol'",
                [],
            )?;
            let keys: Vec<String> = c
                .prepare("SELECT canonical_key FROM usage_observations WHERE id%9=0 AND model='gpt-6.1-sol'")?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for key in keys {
                rebuild_entries(c, &key)?;
            }
            reprice(c)
        })
        .await
        .unwrap();
    assert_rollups_match(&store).await;
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn sums_beyond_64_bits_fall_back_to_the_exact_scan() {
    let p = temp("overflow");
    let store = Store::open(&p, 64, 3650, None).unwrap();
    let mut records = journal(3, 50);
    // 200 maximal responses in one bucket: their cost passes the 64-bit range.
    let at = boundary("2026-03-28T12:01:00Z", chrono_tz::UTC).unwrap();
    for i in 0..200 {
        let mut o = Observation::new("proxy", format!("huge-{i}"), "openai", at + i);
        o.actual_model = Some("gpt-6-astra".into());
        o.account_id = Some("account-0".into());
        o.response_id = Some(format!("huge-{i}"));
        o.completeness = "complete".into();
        o.tokens = Tokens {
            input: Some(0),
            cache_read: Some(0),
            cache_write: Some(0),
            output: Some(1_000_000_000_000),
            reasoning: Some(0),
            ..Default::default()
        };
        records.push(o);
    }
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    let real: i64 = store
        .call(|c| Ok(c.query_row("SELECT COUNT(*) FROM usage_rollup WHERE typeof(s0)='real'", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(real, 1, "the overflowing bucket is marked REAL");
    for q in queries() {
        assert_eq!(store.dashboard(q.clone()).await.unwrap(), store.dashboard_scan(q).await.unwrap());
    }
    store.shutdown().await.unwrap();
}

/// id, payload, snapshot, and the stored type of each.
type Document = (i64, String, String, String, String);

fn documents(c: &Connection) -> Result<Vec<Document>> {
    let rows = c
        .prepare(
            "SELECT id,payload,snapshot_json,typeof(payload),typeof(snapshot_json) FROM usage_observations ORDER BY id",
        )?
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, rusqlite::types::Value>(1)?,
                r.get::<_, rusqlite::types::Value>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(id, p, s, tp, ts)| Ok((id, codec::decode(c, (&p).into())?, codec::decode(c, (&s).into())?, tp, ts)))
        .collect()
}

#[tokio::test]
async fn compression_is_invisible_to_readers_and_downgrade_restores_the_original_text() {
    let p = temp("compress");
    let store = Store::open(&p, 64, 3650, None).unwrap();
    let records = journal(11, 1200);
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    let everything = Query { start: Some("2026-03-01".into()), end: Some("2026-05-01".into()), ..Default::default() };
    let page = Query { limit: Some(500), view: Some("combined".into()), ..everything.clone() };
    let before = (
        store.call(|c| documents(c)).await.unwrap(),
        store.details(page.clone()).await.unwrap(),
        store.dashboard(everything.clone()).await.unwrap(),
    );
    // The writer trains a dictionary and compresses by itself once there are enough rows.
    let mut compressed = false;
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let text: i64 = store
            .call(|c| {
                Ok(c.query_row("SELECT COUNT(*) FROM usage_observations WHERE typeof(payload)='text'", [], |r| {
                    r.get(0)
                })?)
            })
            .await
            .unwrap();
        if text == 0 {
            compressed = true;
            break;
        }
    }
    assert!(compressed, "the writer did not compress the journal");
    let after = store.call(|c| documents(c)).await.unwrap();
    assert!(after.iter().all(|d| d.3 == "blob" && d.4 == "blob"));
    let strip = |d: &[Document]| d.iter().map(|d| (d.0, d.1.clone(), d.2.clone())).collect::<Vec<_>>();
    assert_eq!(strip(&before.0), strip(&after));
    assert_eq!(store.details(page.clone()).await.unwrap(), before.1);
    assert_eq!(store.dashboard(everything.clone()).await.unwrap(), before.2);
    let (stored, original): (i64, i64) = store
        .call(|c| {
            Ok(c.query_row(
                "SELECT SUM(length(payload)+length(snapshot_json)),(SELECT 0) FROM usage_observations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .await
        .unwrap();
    let text: usize = after.iter().map(|d| d.1.len() + d.2.len()).sum();
    assert!((stored as usize) * 3 < text, "compressed {stored} bytes from {text}; expected at least 3x");
    let _ = original;

    // New rows go in compressed; an incompatible stored value fails loudly.
    let records = journal(12, 30);
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    let latest = store.call(|c| documents(c)).await.unwrap();
    assert!(latest.iter().all(|d| d.3 == "blob"));
    store
        .call(|c| {
            let bad = c.query_row("SELECT x'F501FFFFFFFF00'", [], |r| r.get::<_, rusqlite::types::Value>(0))?;
            assert!(codec::decode(c, (&bad).into()).is_err());
            Ok(())
        })
        .await
        .unwrap();
    store.shutdown().await.unwrap();
    drop(store);

    let result = downgrade(&p).unwrap();
    assert_eq!(result["schema"], LEGACY_VERSION);
    let c = Connection::open(&*p).unwrap();
    assert_eq!(c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), LEGACY_VERSION);
    let rows: Vec<(String, String, String, String)> = c
        .prepare(
            "SELECT payload,snapshot_json,typeof(payload),typeof(snapshot_json) FROM usage_observations ORDER BY id",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(rows.iter().all(|r| r.2 == "text" && r.3 == "text"));
    assert_eq!(
        rows.iter().map(|r| (r.0.clone(), r.1.clone())).collect::<Vec<_>>(),
        latest.iter().map(|d| (d.1.clone(), d.2.clone())).collect::<Vec<_>>()
    );
    let raw: String = c.query_row("SELECT value FROM usage_meta WHERE key='catalogue'", [], |r| r.get(0)).unwrap();
    let legacy: Value = serde_json::from_str(&raw).unwrap();
    for rate in legacy["rates"].as_array().unwrap() {
        assert!(rate.get("above_threshold_write_5m_nanos_per_token").is_none());
        assert!(rate.get("above_threshold_write_1h_nanos_per_token").is_none());
        assert!(rate.get("long_context_excluded_tiers").is_none());
        if rate["models"].as_array().unwrap().iter().any(|m| m == "gpt-5.5" || m == "gpt-5.4") {
            assert!(rate["tier_multipliers"].get("fast").is_none());
        }
    }
    assert_eq!(result["omitted_unsupported_rates"], 1);
    let original: String =
        c.query_row("SELECT value FROM usage_meta WHERE key='catalogue_before_downgrade'", [], |r| r.get(0)).unwrap();
    assert!(original.contains("above_threshold_write_5m_nanos_per_token"));
    // Schema-3 objects plus an additive audit table older releases can ignore.
    let names: BTreeSet<String> = c
        .prepare("SELECT name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let expected: BTreeSet<String> = [
        "usage_meta",
        "usage_price_revisions",
        "usage_price_revisions_observation",
        "usage_observations",
        "usage_suppressed",
        "usage_event_time",
        "usage_canonical",
        "usage_association",
        "usage_source_event",
        "usage_source_canonical",
        "usage_filters",
        "usage_entries",
        "usage_entries_key",
        "usage_entries_event_time",
        "usage_entries_filters",
        "usage_source_entries",
        "usage_source_entries_key",
        "usage_source_entries_event_time",
        "usage_source_entries_filters",
        "usage_writer_sessions",
        "usage_entries_association",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(names, expected);
    let columns: Vec<String> = c
        .prepare("SELECT name FROM pragma_table_info('usage_observations')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(!columns.contains(&"pricing_partial".to_string()));
    assert_eq!(columns.len(), 31);
    // Per-source winners rebuilt exactly as schema 3 kept them.
    let winners: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM (SELECT DISTINCT o.source,o.canonical_key FROM usage_observations o WHERE NOT EXISTS(SELECT 1 FROM usage_suppressed s WHERE s.source=o.source AND s.source_event_id=o.source_event_id AND (s.origin_id='' OR s.origin_id=o.origin_id)))",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let source_entries: i64 = c.query_row("SELECT COUNT(*) FROM usage_source_entries", [], |r| r.get(0)).unwrap();
    assert_eq!(source_entries, winners);
    drop(c);

    // And forward again: the same records, the same totals.
    let store = Store::open(&p, 64, 3650, None).unwrap();
    assert_eq!(store.dashboard_scan(everything.clone()).await.unwrap(), store.dashboard(everything).await.unwrap());
    let back = store.call(|c| documents(c)).await.unwrap();
    assert_eq!(strip(&back), strip(&latest));
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn offline_commands_wait_for_the_server_and_compact_shrinks_an_old_layout() {
    let p = temp("offline");
    let store = Store::open(&p, 64, 3650, None).unwrap();
    let records = journal(21, 2000);
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    let error = compact(&p).unwrap_err().to_string();
    assert!(error.contains("stop Fusebox first"), "{error}");
    assert!(downgrade(&p).unwrap_err().to_string().contains("stop Fusebox first"));
    store.shutdown().await.unwrap();
    drop(store);
    // An uncompressed schema-3 database without incremental vacuum, as 0.5 wrote it.
    downgrade(&p).unwrap();
    {
        let c = Connection::open(&*p).unwrap();
        c.execute_batch("PRAGMA auto_vacuum=NONE; VACUUM;").unwrap();
        assert_eq!(c.query_row("PRAGMA auto_vacuum", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    }
    let held = lock(&p, false).unwrap();
    assert!(compact(&p).is_err(), "compact must wait for other writers");
    drop(held);
    let result = compact(&p).unwrap();
    assert!(result["compressed_rows"].as_u64().unwrap() >= 2000);
    assert!(result["bytes_after"].as_u64().unwrap() < result["bytes_before"].as_u64().unwrap());
    let c = Connection::open(&*p).unwrap();
    assert_eq!(c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), VERSION);
    assert_eq!(c.query_row("PRAGMA auto_vacuum", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
    let text: i64 =
        c.query_row("SELECT COUNT(*) FROM usage_observations WHERE typeof(payload)='text'", [], |r| r.get(0)).unwrap();
    assert_eq!(text, 0);
}

#[test]
fn whole_buckets_cover_only_complete_fifteen_minutes() {
    let b = rollup::BUCKET_MS;
    assert_eq!(rollup::whole_buckets(0, b), Some((0, 1)));
    assert_eq!(rollup::whole_buckets(1, b), None);
    assert_eq!(rollup::whole_buckets(1, 3 * b - 1), Some((1, 2)));
    assert_eq!(rollup::whole_buckets(b, 2 * b), Some((1, 2)));
    assert_eq!(rollup::whole_buckets(b + 1, 2 * b + 1), None);
}

#[tokio::test]
async fn each_client_lists_its_models_adding_up_to_its_totals() {
    let p = temp("clients");
    let store = Store::open(&p, 64, 3650, None).unwrap();
    let records = journal(5, 3000);
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    for model in [None, Some("gpt-6.1-sol")] {
        let q = Query {
            start: Some("2026-03-20".into()),
            end: Some("2026-04-12".into()),
            model: model.map(String::from),
            ..Default::default()
        };
        let dashboard = store.dashboard(q.clone()).await.unwrap();
        assert_eq!(dashboard, store.dashboard_scan(q).await.unwrap());
        let clients = dashboard["combined"]["breakdowns"]["client"].as_array().unwrap();
        assert!(clients.len() >= 3, "{clients:?}");
        for c in clients {
            let models = c["models"].as_array().unwrap();
            let sum = |key: &str| models.iter().map(|m| m[key].as_i64().unwrap_or(0)).sum::<i64>();
            assert_eq!(sum("observations"), c["observations"].as_i64().unwrap(), "{c}");
            assert_eq!(sum("unpriced"), c["unpriced"].as_i64().unwrap(), "{c}");
            assert_eq!(sum("estimated_cost_nanos"), c["estimated_cost_nanos"].as_i64().unwrap_or(0), "{c}");
            let costs: Vec<i64> = models.iter().map(|m| m["estimated_cost_nanos"].as_i64().unwrap_or(-1)).collect();
            assert!(costs.windows(2).all(|p| p[0] >= p[1]), "largest list price first: {costs:?}");
            if let Some(model) = model {
                assert!(models.iter().all(|m| m["id"] == model), "{c}");
            }
        }
    }
    store.shutdown().await.unwrap();
}
