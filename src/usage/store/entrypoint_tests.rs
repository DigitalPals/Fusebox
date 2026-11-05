//! Claude Code entrypoints: what the Usage page shows per label, that filtering by one
//! agrees with a full scan, and that releases without entrypoints can still share the
//! database and leave its rollup exact.
use super::super::types::Tokens;
use super::*;
use axum::http::{HeaderMap, HeaderValue, header::USER_AGENT};

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
    let dir = std::env::temp_dir().join(format!("fusebox-entrypoint-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    TempDb { path: dir.join(format!("{name}.sqlite")), dir }
}

const DAY: &str = "2026-10-20";

fn call(source: &str, id: &str, provider: &str, entrypoint: Option<&str>, at: &str) -> Observation {
    let mut o = Observation::new(source, id.into(), provider, boundary(at, &crate::zone::Zone::utc()).unwrap());
    o.actual_model = Some(if provider == "anthropic" { "claude-sonnet-4-6" } else { "gpt-6.1-sol" }.into());
    o.account_id = Some("account-1".into());
    o.response_id = Some(format!("response-{id}"));
    o.completeness = "complete".into();
    o.tokens = Tokens {
        input: Some(1000),
        cache_read: Some(100),
        cache_write: Some(0),
        output: Some(100),
        reasoning: Some(0),
        ..Default::default()
    };
    o.entrypoint = entrypoint.map(Into::into);
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

fn row<'a>(rows: &'a Value, id: Option<&str>) -> &'a Value {
    rows.as_array().unwrap().iter().find(|r| r["id"].as_str() == id).unwrap_or_else(|| panic!("no row {id:?}"))
}

fn dump(c: &Connection) -> Result<Vec<String>> {
    let sums = (0..rollup::AMOUNTS.len()).map(|i| format!("k{i},s{i}")).collect::<Vec<_>>().join(",");
    Ok(c.prepare(&format!(
        "SELECT json_array(bucket,dims,provider,model,account_id,client,entrypoint,observations,partial,missing,pricing_partial,{sums})
         FROM usage_entrypoint_rollup ORDER BY bucket,dims"
    ))?
    .query_map([], |r| r.get::<_, String>(0))?
    .collect::<rusqlite::Result<_>>()?)
}

/// The triggers must leave exactly what a rebuild from the entries produces.
fn assert_exact(c: &mut Connection) -> Result<()> {
    let live = dump(c)?;
    let tx = c.transaction()?;
    tx.execute_batch("DELETE FROM usage_entrypoint_rollup;")?;
    rollup::backfill_entrypoints(&tx)?;
    let rebuilt = dump(&tx)?;
    tx.rollback()?;
    assert_eq!(live, rebuilt, "the entrypoint rollup drifted from the journal");
    Ok(())
}

#[test]
fn the_user_agent_names_the_entrypoint() {
    let entrypoint = |agent: &str| {
        let mut h = HeaderMap::new();
        h.insert(USER_AGENT, HeaderValue::from_str(agent).unwrap());
        crate::usage::capture::claude_code_entrypoint(&h)
    };
    assert_eq!(entrypoint("claude-cli/2.1.304 (external, sdk-ts, agent-sdk/0.3.276)").as_deref(), Some("sdk-ts"));
    assert_eq!(entrypoint("claude-cli/2.1.309 (external, cli)").as_deref(), Some("cli"));
    assert_eq!(entrypoint("claude-cli/2.1.316 (external, t3-code)").as_deref(), Some("t3-code"));
    assert_eq!(entrypoint("claude-cli/2.1.316 (external,  Claude-VSCode )").as_deref(), Some("claude-vscode"));
    for other in [
        "claude-cli/2.1.316",
        "claude-cli/2.1.316 (external)",
        "claude-cli/2.1.316 (external, )",
        "claude-cli/2.1.316 (external, two words)",
        "claude-cli/2.1.316 (external, sdk-ts\u{7f})",
        "codex_cli_rs/0.163.0 (Mac OS 26.0; arm64)",
        "Fusebox/0.12.1 (external, cli)",
    ] {
        if let Ok(value) = HeaderValue::from_str(other) {
            let mut h = HeaderMap::new();
            h.insert(USER_AGENT, value);
            assert_eq!(crate::usage::capture::claude_code_entrypoint(&h), None, "{other}");
        }
    }
    let long = format!("claude-cli/2.1.316 (external, {})", "a".repeat(65));
    assert_eq!(entrypoint(&long), None);
    assert_eq!(crate::usage::capture::claude_code_entrypoint(&HeaderMap::new()), None);
}

#[tokio::test]
async fn the_usage_page_counts_calls_per_entrypoint_and_filters_by_one() {
    let p = temp("page");
    let store = Store::open(&p, 64, 3650, None).unwrap();
    let records = vec![
        call("proxy", "a", "anthropic", Some("sdk-ts"), "2026-10-20T08:00:00Z"),
        call("proxy", "b", "anthropic", Some("sdk-ts"), "2026-10-20T08:07:30Z"),
        call("proxy", "c", "anthropic", Some("cli"), "2026-10-20T09:00:00Z"),
        call("proxy", "d", "anthropic", None, "2026-10-20T09:30:00Z"),
        call("proxy", "e", "openai", None, "2026-10-20T10:00:00Z"),
        call("claude_code", "f", "anthropic", None, "2026-10-20T11:00:00Z"),
        // The day before: outside the range.
        call("proxy", "g", "anthropic", Some("sdk-ts"), "2026-10-19T12:00:00Z"),
    ];
    store.call(move |c| insert_batch(c, &records)).await.unwrap();

    let value = store.dashboard(day()).await.unwrap();
    assert_eq!(value, store.dashboard_scan(day()).await.unwrap());
    assert_eq!(value["combined"]["totals"]["observations"], 6);
    assert!(value["combined"]["breakdowns"].get("entrypoint").is_none(), "the summary itself is unchanged");
    let entrypoints = store.entrypoints(day()).await.unwrap();
    assert_eq!(entrypoints, store.entrypoints_scan(day()).await.unwrap());
    let rows = &entrypoints["breakdown"];
    assert_eq!(rows.as_array().unwrap().len(), 2, "only calls that named an entrypoint");
    assert_eq!(row(rows, Some("sdk-ts"))["observations"], 2);
    assert_eq!(row(rows, Some("sdk-ts"))["tokens"]["input"], 2000);
    assert_eq!(row(rows, Some("cli"))["observations"], 1);
    assert_eq!(rows[0]["id"], "sdk-ts", "largest cost first");
    let trend = entrypoints["trend"].as_array().unwrap();
    assert_eq!(
        trend
            .iter()
            .map(|t| (t["date"].as_str().unwrap(), t["group"].as_str().unwrap(), t["observations"].as_i64().unwrap()))
            .collect::<Vec<_>>(),
        [(DAY, "cli", 1), (DAY, "sdk-ts", 2)]
    );
    // A range that starts and ends inside buckets reads those ends from entries.
    let partial = Query {
        start: Some("2026-10-20T08:05:00Z".into()),
        end: Some("2026-10-20T09:00:00.001Z".into()),
        timezone: Some("UTC".into()),
        ..Default::default()
    };
    let ends = store.entrypoints(partial.clone()).await.unwrap();
    assert_eq!(ends, store.entrypoints_scan(partial).await.unwrap());
    assert_eq!(row(&ends["breakdown"], Some("sdk-ts"))["observations"], 1);
    assert_eq!(row(&ends["breakdown"], Some("cli"))["observations"], 1);
    let imported = Query { source: Some("claude_code".into()), ..day() };
    assert_eq!(store.entrypoints(imported).await.unwrap()["breakdown"], json!([]));

    let only = Query { entrypoint: Some("sdk-ts".into()), ..day() };
    let filtered = store.dashboard(only.clone()).await.unwrap();
    assert_eq!(filtered, store.dashboard_scan(only.clone()).await.unwrap());
    assert_eq!(filtered["combined"]["totals"]["observations"], 2);
    assert_eq!(store.entrypoints(only.clone()).await.unwrap()["breakdown"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["combined"]["breakdowns"]["model"][0]["id"], "claude-sonnet-4-6");
    assert_eq!(filtered["facets"], value["facets"], "the menus still offer every value");
    assert_eq!(store.details(only.clone()).await.unwrap()["total"], 2);
    let imports = Query { source: Some("claude_code".into()), ..only };
    assert_eq!(store.dashboard(imports.clone()).await.unwrap()["combined"]["totals"]["observations"], 0);
    assert_eq!(store.dashboard(imports.clone()).await.unwrap(), store.dashboard_scan(imports).await.unwrap());
    store.call(assert_exact).await.unwrap();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_release_without_entrypoints_keeps_the_rollup_exact() {
    let p = temp("older-release");
    let store = Store::open(&p, 64, 3650, None).unwrap();
    let records: Vec<_> = (0..40)
        .map(|i| {
            let at = format!("2026-10-20T{:02}:{:02}:00Z", 6 + i / 4, (i % 4) * 15 + 3);
            let entrypoint = [Some("sdk-ts"), Some("cli"), None][i % 3];
            call("proxy", &format!("call-{i}"), "anthropic", entrypoint, &at)
        })
        .collect();
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    store
        .call(|c| {
            // An older release writes no labels, deletes observations before entries
            // when it purges, and never touches usage_entrypoints.
            let unlabelled = call("proxy", "older", "anthropic", None, "2026-10-20T16:00:00Z");
            insert_batch(c, &[unlabelled])?;
            let cut = boundary("2026-10-20T09:00:00Z", &crate::zone::Zone::utc())?;
            c.execute("DELETE FROM usage_observations WHERE event_at_ms<?1", [cut])?;
            c.execute("DELETE FROM usage_entries WHERE event_at_ms<?1", [cut])?;
            assert_exact(c)?;
            // Its repricing rebuilds entries through delete and insert.
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
    assert_eq!(store.dashboard(day()).await.unwrap(), store.dashboard_scan(day()).await.unwrap());
    // This release's purge removes the labels of what it purges.
    let cut = boundary("2026-10-20T12:00:00Z", &crate::zone::Zone::utc()).unwrap();
    store.purge(cut).await.unwrap();
    store
        .call(move |c| {
            assert_exact(c)?;
            let stale: i64 = c.query_row(
                "SELECT COUNT(*) FROM usage_entrypoints e JOIN usage_observations o ON o.id=e.id WHERE o.event_at_ms<?1",
                [cut],
                |r| r.get(0),
            )?;
            assert_eq!(stale, 0);
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(store.dashboard(day()).await.unwrap(), store.dashboard_scan(day()).await.unwrap());
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn every_database_has_the_tables_and_a_downgrade_removes_them() {
    let p = temp("downgrade");
    let store = Store::open(&p, 64, 3650, None).unwrap();
    let records = vec![call("proxy", "a", "anthropic", Some("sdk-ts"), "2026-10-20T08:00:00Z")];
    store.call(move |c| insert_batch(c, &records)).await.unwrap();
    store.shutdown().await.unwrap();
    drop(store);
    let objects = |c: &Connection| -> Vec<String> {
        c.prepare("SELECT name FROM sqlite_master WHERE name LIKE 'usage_entrypoint%' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    {
        let mut c = Connection::open(&*p).unwrap();
        assert_eq!(objects(&c).len(), 2 + rollup::ENTRYPOINT_TRIGGERS.len(), "two tables and their triggers");
        // Opening again changes nothing.
        let before = c.query_row("PRAGMA schema_version", [], |r| r.get::<_, i64>(0)).unwrap();
        migrate(&mut c).unwrap();
        assert_eq!(c.query_row("PRAGMA schema_version", [], |r| r.get::<_, i64>(0)).unwrap(), before);
        // A missing trigger is put back and the rollup refilled from the entries.
        c.execute_batch("DROP TRIGGER usage_entrypoint_rollup_delete; DELETE FROM usage_entrypoint_rollup;").unwrap();
        migrate(&mut c).unwrap();
        assert_eq!(objects(&c).len(), 2 + rollup::ENTRYPOINT_TRIGGERS.len());
        assert_eq!(dump(&c).unwrap().len(), 1);
        assert_exact(&mut c).unwrap();
    }
    compact::downgrade(&p).unwrap();
    assert!(objects(&Connection::open(&*p).unwrap()).is_empty());
}
