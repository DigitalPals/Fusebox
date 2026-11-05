use super::*;
use crate::usage::store::{insert_batch, reprice};

type Row = (String, i64, i64, i64, i64);

fn rows(conn: &rusqlite::Connection, sql: &str, day: i64, month: i64) -> anyhow::Result<Vec<Row>> {
    Ok(conn
        .prepare(sql)?
        .query_map([day, month], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

fn at(s: &str) -> i64 {
    DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis()
}

fn observation(id: impl Into<String>, event_at_ms: i64) -> Observation {
    let mut o = Observation::new("proxy", id.into(), "openai", event_at_ms);
    o.client_id = Some("laptop".into());
    o.actual_model = Some("gpt-6-astra".into());
    o.status = Some(200);
    o.completeness = "complete".into();
    o.tokens = crate::usage::types::Tokens {
        input: Some(100),
        cache_read: Some(0),
        cache_write: Some(0),
        output: Some(0),
        ..Default::default()
    };
    o
}

struct Db {
    dir: std::path::PathBuf,
    store: Store,
}

impl Db {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("fusebox-reconcile-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("test.sqlite3"), 4096, 365, None).unwrap();
        Self { dir, store }
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn check_periods(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    for (day, month) in [
        ("2026-03-29T00:00:00+01:00", "2026-03-01T00:00:00+01:00"),
        ("2026-03-30T00:00:00+02:00", "2026-03-01T00:00:00+01:00"),
        ("2026-10-25T00:00:00+02:00", "2026-10-01T00:00:00+02:00"),
        ("2026-10-26T00:00:00+01:00", "2026-10-01T00:00:00+02:00"),
        ("2026-10-25T00:00:00+05:45", "2026-10-01T00:00:00+05:45"),
        ("2026-11-01T00:00:00+05:30", "2026-11-01T00:00:00+05:30"),
        ("2026-11-02T00:00:00.123+05:45", "2026-11-01T00:00:00.456+05:45"),
    ] {
        assert_eq!(
            rows(conn, RECONCILE_TOTALS, at(day), at(month))?,
            rows(conn, RAW_RECONCILE_TOTALS, at(day), at(month))?,
            "day={day}, month={month}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn hourly_totals_match_raw_attempts_across_mutations_retention_and_local_periods() {
    let db = Db::new();
    let mut records = Vec::new();
    for (span, base) in ["2026-03-27T00:00:00Z", "2026-10-23T00:00:00Z", "2026-10-30T00:00:00Z"].into_iter().enumerate()
    {
        for i in 0..400 {
            let mut o = observation(format!("{span}-{i}"), at(base) + i * 16 * 60_000 + i % 3);
            if i % 7 == 0 {
                o.source = "codex".into();
            }
            if i % 11 == 0 {
                o.client_id = None;
            }
            if i % 13 == 0 {
                o.actual_model = Some("unknown-model".into());
            }
            if i % 17 == 0 {
                o.origin_id = "collector:import".into();
            }
            if i % 3 == 0 {
                o.tokens.input = None;
            }
            if i % 5 == 0 {
                o.tokens.cache_write = None;
            }
            if i % 6 == 0 {
                o.tokens.output = None;
            }
            if i % 9 == 0 {
                o.tokens.cache_read = Some(1000);
            }
            o.status = if i % 4 == 0 { None } else { Some(if i % 2 == 0 { 500 } else { 200 }) };
            records.push(o);
        }
    }
    // Event times are deliberately out of insertion order; replays are idempotent.
    records.reverse();
    db.store.call(move |c| {
        insert_batch(c, &records)?;
        let changes = c.total_changes();
        insert_batch(c, &records)?;
        assert_eq!(c.total_changes(), changes, "duplicate raw evidence must not change a total");
        check_periods(c)?;
        c.execute("UPDATE usage_observations SET cost_nanos=NULL,pricing_basis='unknown_model' WHERE model='gpt-6-astra'", [])?;
        check_periods(c)?;
        assert!(reprice(c)? > 0, "repair existing rows, rather than only inserting later IDs");
        check_periods(c)?;
        c.execute("UPDATE usage_observations SET cost_nanos=COALESCE(cost_nanos,0)+123 WHERE id%19=0", [])?;
        check_periods(c)?;
        c.execute("UPDATE usage_observations SET client_id='renamed',input=42,event_at_ms=?1 WHERE id%23=0", [at("2026-11-01T00:15:00Z")])?;
        c.execute("UPDATE usage_observations SET source=CASE source WHEN 'proxy' THEN 'codex' ELSE 'proxy' END WHERE id%29=0", [])?;
        // Payload status changes, and compression-only rewrites, both remain exact.
        c.execute("UPDATE usage_observations SET payload=json_set(usage_document(payload),'$.status',200) WHERE id%31=0", [])?;
        check_periods(c)?;
        c.execute_batch("SAVEPOINT rollback_totals; DELETE FROM usage_observations WHERE id%3=0; ROLLBACK TO rollback_totals; RELEASE rollback_totals;")?;
        check_periods(c)?;
        c.execute("DELETE FROM usage_observations WHERE id%37=0", [])?;
        check_periods(c)
    }).await.unwrap();
    db.store.purge(at("2026-11-01T00:00:00Z")).await.unwrap();
    db.store.call(|c| check_periods(c)).await.unwrap();
    db.store.shutdown().await.unwrap();
}

#[tokio::test]
async fn startup_backfills_existing_rows_and_reopening_does_not_rebuild() {
    let db = Db::new();
    let record = observation("before-upgrade", at("2026-11-01T00:01:00Z"));
    db.store
        .call(move |c| {
            insert_batch(c, &[record])?;
            for suffix in ["insert", "delete", "update"] {
                c.execute_batch(&format!("DROP TRIGGER usage_client_totals_{suffix};"))?;
            }
            c.execute_batch("DROP TABLE usage_client_totals;")?;
            Ok(())
        })
        .await
        .unwrap();
    db.store.shutdown().await.unwrap();
    // A marker bucket shows whether opening rebuilt the table: kept while the
    // definition is current, gone once the recorded definition is an older one.
    for (step, marker_expected) in [(0, false), (1, true), (2, false)] {
        let store = Store::open(&db.dir.join("test.sqlite3"), 4096, 365, None).unwrap();
        store
            .call(move |c| {
                check_periods(c)?;
                let marker: bool = c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM usage_client_totals WHERE client_id='marker')",
                    [],
                    |r| r.get(0),
                )?;
                assert_eq!(marker, marker_expected, "step {step}");
                let count: i64 = c.query_row(
                    "SELECT SUM(observations) FROM usage_client_totals WHERE client_id<>'marker'",
                    [],
                    |r| r.get(0),
                )?;
                assert_eq!(count, 1);
                match step {
                    0 => {
                        c.execute("INSERT INTO usage_client_totals VALUES(0,'marker',1,0,0,0,0)", [])?;
                    }
                    1 => {
                        c.execute("UPDATE usage_meta SET value='0' WHERE key='client_totals_definition'", [])?;
                    }
                    _ => {}
                }
                Ok(())
            })
            .await
            .unwrap();
        store.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn snapshot_does_not_lock_admission_or_record_and_preserves_racing_pending() {
    let db = Db::new();
    let now = Utc::now().timestamp_millis();
    let records: Vec<_> = (0..1500).map(|i| observation(format!("seed-{i}"), now - i * 60_000)).collect();
    db.store.call(move |c| insert_batch(c, &records).map(|_| ())).await.unwrap();
    let clients = Clients::new(Some(db.store.clone()));
    let refresh = || {
        let clients = clients.clone();
        db.store.call(move |c| clients.reconcile(c))
    };
    refresh().await.unwrap();
    let before = clients.spend("laptop");
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (store, writer) = (db.store.clone(), clients.clone());
    let run = tokio::spawn(async move {
        store
            .call(move |c| {
                let mut entered = Some(entered);
                c.progress_handler(
                    100,
                    Some(move || {
                        if let Some(entered) = entered.take() {
                            let _ = entered.send(());
                            wait.recv_timeout(Duration::from_secs(5)).unwrap();
                        }
                        false
                    }),
                );
                let result = writer.reconcile(c);
                c.progress_handler(0, None::<fn() -> bool>);
                result
            })
            .await
    });
    ready.await.unwrap();
    assert!(clients.spend.try_lock().is_some(), "SQL must not hold a Tokio request's mutex");
    let limited = NamedClient { id: "laptop".into(), tokens_per_day: Some(u64::MAX), ..Default::default() };
    assert!(clients.admit(&limited, &HeaderMap::new(), true).is_ok());
    let a = observation("during-query-enqueued", now);
    let b = observation("during-query-not-enqueued", now);
    clients.record(&a, Some(&db.store));
    assert!(db.store.enqueue(a));
    clients.record(&b, Some(&db.store));
    let expected = clients.spend("laptop");
    assert_eq!(expected.tokens_today, before.tokens_today + 200);
    release.send(()).unwrap();
    run.await.unwrap().unwrap();
    assert_eq!(clients.spend("laptop"), expected);
    refresh().await.unwrap();
    assert_eq!(clients.spend("laptop"), expected);
    assert!(db.store.enqueue(b));
    refresh().await.unwrap();
    assert_eq!(clients.spend("laptop"), expected);
    assert!(clients.spend.lock().pending.is_empty());
    db.store.shutdown().await.unwrap();
}

#[tokio::test]
async fn acknowledgment_sql_holds_no_request_lock_and_keeps_later_completions() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    let db = Db::new();
    let clients = Clients::new(Some(db.store.clone()));
    let now = Utc::now().timestamp_millis();
    let a = observation("acknowledged", now);
    let b = observation("pending-before-copy", now);
    let c = observation("pending-after-copy", now);
    clients.record(&a, Some(&db.store));
    assert!(db.store.enqueue(a));
    clients.record(&b, Some(&db.store));
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (store, writer) = (db.store.clone(), clients.clone());
    let run = tokio::spawn(async move {
        store
            .call(move |conn| {
                let mut entered = Some(entered);
                conn.authorizer(Some(move |context: AuthContext<'_>| {
                    if matches!(
                        context.action,
                        AuthAction::Read { table_name: "usage_observations", column_name: "source_event_id" }
                    ) && let Some(entered) = entered.take()
                    {
                        let _ = entered.send(());
                        wait.recv_timeout(Duration::from_secs(5)).unwrap();
                    }
                    Authorization::Allow
                }));
                let result = writer.reconcile(conn);
                conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
                result
            })
            .await
    });
    ready.await.unwrap();
    assert!(clients.spend.try_lock().is_some(), "acknowledgment SQL must also run outside the mutex");
    clients.record(&c, Some(&db.store));
    assert_eq!(clients.spend("laptop").tokens_today, 300);
    release.send(()).unwrap();
    run.await.unwrap().unwrap();
    assert_eq!(clients.spend("laptop").tokens_today, 300);
    assert_eq!(clients.spend.lock().pending.len(), 2);
    assert!(db.store.enqueue(b));
    assert!(db.store.enqueue(c));
    let writer = clients.clone();
    db.store.call(move |conn| writer.reconcile(conn)).await.unwrap();
    assert_eq!(clients.spend("laptop").tokens_today, 300);
    assert!(clients.spend.lock().pending.is_empty());
    db.store.shutdown().await.unwrap();
}

#[test]
fn day_and_month_rolls_reset_the_matching_unknown_flags_and_fail_closed() {
    let mut s =
        Spends { day: Some(NaiveDate::from_ymd_opt(2026, 10, 30).unwrap()), available: true, ..Default::default() };
    s.by_client.insert("laptop".into(), Totals { tokens_today: 10, cost_nanos: 20, unknown_tokens: 1, unpriced: 2 });
    s.roll_at(NaiveDate::from_ymd_opt(2026, 10, 31).unwrap());
    let t = s.by_client["laptop"];
    assert_eq!((t.tokens_today, t.cost_nanos, t.unknown_tokens, t.unpriced), (0, 20, 0, 2));
    assert!(!s.available);
    s.roll_at(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
    let t = s.by_client["laptop"];
    assert_eq!((t.tokens_today, t.cost_nanos, t.unknown_tokens, t.unpriced), (0, 0, 0, 0));
    assert!(!s.available);
}

#[tokio::test]
async fn crossing_midnight_does_not_publish_an_old_snapshot() {
    let db = Db::new();
    let clients = Clients::new(Some(db.store.clone()));
    let writer = clients.clone();
    let before = Local.with_ymd_and_hms(2026, 10, 31, 23, 59, 59).single().unwrap();
    let after = before + chrono::Duration::seconds(2);
    let mut times = [before, after].into_iter();
    let error = db
        .store
        .call(move |c| {
            writer.reconcile_with_clock(c, || times.next().unwrap(), Instant::now() + Duration::from_secs(10))
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("period changed"));
    assert!(!clients.spend.lock().available);
    assert!(clients.spend.lock().refreshed.is_none());
    db.store.shutdown().await.unwrap();
}

#[tokio::test]
async fn overflowing_persistent_totals_fail_closed_without_losing_observations() {
    let db = Db::new();
    let a = observation("overflow-a", Utc::now().timestamp_millis());
    let b = observation("overflow-b", a.event_at_ms);
    db.store
        .call(move |c| {
            insert_batch(c, &[a, b])?;
            c.execute("UPDATE usage_observations SET cost_nanos=?1", [i64::MAX])?;
            let count: i64 = c.query_row("SELECT COUNT(*) FROM usage_observations", [], |r| r.get(0))?;
            assert_eq!(count, 2);
            Ok(())
        })
        .await
        .unwrap();
    let clients = Clients::new(Some(db.store.clone()));
    let writer = clients.clone();
    assert!(db.store.call(move |c| writer.reconcile(c)).await.is_err());
    let limited = NamedClient { id: "laptop".into(), usd_per_month: Some(1.0), ..Default::default() };
    assert_eq!(clients.admit(&limited, &HeaderMap::new(), true).unwrap_err().status, 503);
    db.store.shutdown().await.unwrap();
}

#[tokio::test]
async fn pending_rollover_keeps_monthly_cost_but_only_todays_unknown_tokens() {
    let db = Db::new();
    let clients = Clients::new(Some(db.store.clone()));
    let now = Local.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).single().unwrap();
    {
        let mut s = clients.spend.lock();
        for (event, date, tokens, cost) in [
            ("last-month", (2026, 10, 31), 11, 100),
            ("yesterday", (2026, 11, 1), 22, 200),
            ("today", (2026, 11, 2), 33, 300),
        ] {
            s.pending.insert(
                event.into(),
                Pending {
                    origin: "local".into(),
                    client: "laptop".into(),
                    event_at_ms: Local
                        .with_ymd_and_hms(date.0, date.1, date.2, 1, 0, 0)
                        .single()
                        .unwrap()
                        .timestamp_millis(),
                    totals: Totals { tokens_today: tokens, cost_nanos: cost, unknown_tokens: 1, unpriced: 1 },
                },
            );
        }
    }
    for _ in 0..2 {
        let writer = clients.clone();
        db.store
            .call(move |c| writer.reconcile_with_clock(c, || now, Instant::now() + Duration::from_secs(10)))
            .await
            .unwrap();
        let s = clients.spend.lock();
        let t = s.by_client["laptop"];
        assert_eq!((t.tokens_today, t.cost_nanos, t.unknown_tokens, t.unpriced), (33, 500, 1, 2));
        assert_eq!(s.pending.len(), 2);
        assert!(!s.pending.contains_key("last-month"));
    }
    db.store.shutdown().await.unwrap();
}

#[tokio::test]
async fn incomplete_accounting_only_gates_the_affected_budget() {
    let db = Db::new();
    let now = Utc::now().timestamp_millis();
    let mut missing = observation("missing-tokens", now);
    missing.client_id = Some("missing-tokens".into());
    missing.tokens.input = None;
    let mut unpriced = observation("unpriced", now);
    unpriced.client_id = Some("unpriced".into());
    unpriced.actual_model = Some("unpriced-model".into());
    let mut failed = observation("failed-empty", now);
    failed.client_id = Some("failed-empty".into());
    failed.tokens = Default::default();
    failed.status = Some(500);
    let mut cache_only = failed.clone();
    cache_only.source_event_id = "cache-only".into();
    cache_only.client_id = Some("cache-only".into());
    cache_only.tokens.cache_read = Some(1000);
    db.store
        .call(move |c| {
            insert_batch(c, &[missing, unpriced, failed, cache_only])?;
            // Proves an independently established price need not make token counts complete.
            c.execute("UPDATE usage_observations SET cost_nanos=123 WHERE source_event_id='missing-tokens'", [])?;
            Ok(())
        })
        .await
        .unwrap();
    let clients = Clients::new(Some(db.store.clone()));
    let writer = clients.clone();
    db.store.call(move |c| writer.reconcile(c)).await.unwrap();
    for (id, tokens_blocked, cost_blocked) in [
        ("missing-tokens", true, false),
        ("unpriced", false, true),
        ("failed-empty", false, false),
        ("cache-only", true, true),
    ] {
        for (tokens, blocked) in [(true, tokens_blocked), (false, cost_blocked)] {
            let limited = NamedClient {
                id: id.into(),
                tokens_per_day: tokens.then_some(1000),
                usd_per_month: (!tokens).then_some(1.0),
                ..Default::default()
            };
            let admitted = clients.admit(&limited, &HeaderMap::new(), true);
            assert_eq!(admitted.as_ref().err().map(|e| e.status), blocked.then_some(503), "id={id},tokens={tokens}");
        }
        assert!(clients.admit(&NamedClient { id: id.into(), ..Default::default() }, &HeaderMap::new(), true).is_ok());
    }
    // A separate maintenance connection repairs an existing row; no writer-local
    // row-ID cursor or update hook may be required to notice the change.
    let path = db.dir.join("test.sqlite3");
    tokio::task::spawn_blocking(move || {
        let c = rusqlite::Connection::open(path).unwrap();
        crate::usage::store::register_functions(&c).unwrap();
        c.execute("UPDATE usage_observations SET cost_nanos=456 WHERE client_id='unpriced'", []).unwrap();
    })
    .await
    .unwrap();
    let writer = clients.clone();
    db.store.call(move |c| writer.reconcile(c)).await.unwrap();
    let limited = NamedClient { id: "unpriced".into(), usd_per_month: Some(1.0), ..Default::default() };
    assert!(clients.admit(&limited, &HeaderMap::new(), true).is_ok());
    db.store.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_expired_refresh_cannot_reenable_budget_admission() {
    let db = Db::new();
    let clients = Clients::new(Some(db.store.clone()));
    let writer = clients.clone();
    let result = db
        .store
        .call(move |c| {
            let deadline = Instant::now() + Duration::from_millis(2);
            writer.reconcile_with_clock(
                c,
                || {
                    std::thread::sleep(Duration::from_millis(5));
                    Local::now()
                },
                deadline,
            )
        })
        .await;
    assert!(result.unwrap_err().to_string().contains("expired"));
    assert!(!clients.spend.lock().available);
    db.store.shutdown().await.unwrap();
}

fn buckets(c: &rusqlite::Connection) -> anyhow::Result<Vec<(i64, i64)>> {
    Ok(c.prepare("SELECT bucket_ms,observations FROM usage_client_totals ORDER BY bucket_ms")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

#[tokio::test]
async fn a_table_started_this_month_leaves_older_rows_out_and_refuses_older_months() {
    let db = Db::new();
    let october = observation("october", at("2026-10-15T12:00:00Z"));
    let november = observation("november", at("2026-11-03T12:00:00Z"));
    db.store
        .call(move |c| {
            insert_batch(c, &[october, november])?;
            crate::usage::store::build_client_totals(c, at("2026-10-31T00:00:00Z"))?;
            assert_eq!(buckets(c)?, [(at("2026-11-03T12:00:00Z"), 1)]);
            // Rows from before the start stay out as they change, arrive late or go.
            c.execute("UPDATE usage_observations SET cost_nanos=999,input=7 WHERE source_event_id='october'", [])?;
            insert_batch(c, &[observation("late-october", at("2026-10-20T00:00:00Z"))])?;
            c.execute("DELETE FROM usage_observations WHERE source_event_id='october'", [])?;
            assert_eq!(buckets(c)?, [(at("2026-11-03T12:00:00Z"), 1)]);
            for (day, month) in [
                ("2026-11-05T00:00:00+01:00", "2026-11-01T00:00:00+01:00"),
                ("2026-11-03T00:00:00+05:30", "2026-11-01T00:00:00+05:30"),
                ("2026-11-01T00:00:00+14:00", "2026-11-01T00:00:00+14:00"),
            ] {
                assert_eq!(
                    rows(c, RECONCILE_TOTALS, at(day), at(month))?,
                    rows(c, RAW_RECONCILE_TOTALS, at(day), at(month))?,
                    "day={day}, month={month}"
                );
            }
            Ok(())
        })
        .await
        .unwrap();
    let clients = Clients::new(Some(db.store.clone()));
    let reconcile_at = |now: DateTime<Local>| {
        let writer = clients.clone();
        db.store.call(move |c| writer.reconcile_with_clock(c, || now, Instant::now() + Duration::from_secs(10)))
    };
    let error = reconcile_at(Local.with_ymd_and_hms(2026, 10, 20, 12, 0, 0).single().unwrap()).await.unwrap_err();
    assert!(error.to_string().contains("start after"), "{error}");
    assert!(!clients.spend.lock().available);
    reconcile_at(Local.with_ymd_and_hms(2026, 11, 5, 12, 0, 0).single().unwrap()).await.unwrap();
    assert!(clients.spend.lock().available);
    db.store.shutdown().await.unwrap();
}
