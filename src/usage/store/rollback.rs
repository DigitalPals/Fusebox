//! An explicitly requested compatibility copy retains the original schema-5
//! journal, including evidence older releases cannot represent.
use super::*;

const LIFETIME_RATE_FIELDS: [&str; 4] = [
    "cache_write_30m_nanos_per_token",
    "cache_write_12h_nanos_per_token",
    "above_threshold_write_30m_nanos_per_token",
    "above_threshold_write_12h_nanos_per_token",
];

/// Create a new, private legacy database. The original is never downgraded and
/// retains the complete lifetime evidence, fingerprints and pricing audit.
pub fn rollback_copy(database: &Path, to: &Path) -> Result<Value> {
    ensure!(database.is_file(), "there is no usage database at {}", database.display());
    let _lock = compact::lock(database, true)?;
    let source = Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let version: i64 = source.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let application: i64 = source.pragma_query_value(None, "application_id", |r| r.get(0))?;
    ensure!(
        version == VERSION && application == APPLICATION_ID,
        "rollback copy requires a Fusebox schema-{VERSION} database"
    );
    drop(source);
    for suffix in ["-wal", "-shm", "-journal", ".lock"] {
        let companion = format!("{}{suffix}", to.display());
        ensure!(
            std::fs::symlink_metadata(&companion).is_err(),
            "{companion} already exists; choose a new rollback file"
        );
    }
    // create_new prevents overwriting the source, another database, or a symlink.
    backup_file(database, to)?;
    let result = (|| -> Result<Value> {
        let mut conn = Connection::open(to)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA foreign_keys=ON;")?;
        let lifetime_rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM usage_observations WHERE write_30m IS NOT NULL OR write_12h IS NOT NULL",
            [],
            |r| r.get(0),
        )?;
        let tx = conn.transaction()?;
        let mut cursor = 0_i64;
        loop {
            let rows = tx
                .prepare("SELECT id,payload,snapshot_json FROM usage_observations WHERE id>?1 ORDER BY id LIMIT 1000")?
                .query_map([cursor], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, SqlValue>(1)?, r.get::<_, SqlValue>(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let Some(last) = rows.last() else { break };
            cursor = last.0;
            for (id, payload, snapshot) in rows {
                let mut observation: Observation = serde_json::from_str(&codec::decode(&tx, (&payload).into())?)?;
                observation.tokens.write_30m = None;
                observation.tokens.write_12h = None;
                let snapshot = compatible_snapshot(&codec::decode(&tx, (&snapshot).into())?)?;
                // Preserve identities and historical costs. Canonical/source keys
                // continue to deduplicate replays under an older importer.
                tx.execute("UPDATE usage_observations SET payload=?2,snapshot_json=?3,write_30m=NULL,write_12h=NULL WHERE id=?1",
                    params![id,serde_json::to_string(&observation)?,snapshot])?;
            }
        }
        // Previous snapshots are documents too; removing columns alone would
        // leave unknown rate fields rejected by older readers.
        let mut cursor = 0_i64;
        let revisions: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='usage_price_revisions')",
            [],
            |r| r.get(0),
        )?;
        if revisions {
            loop {
                let rows = tx
                    .prepare(
                        "SELECT id,previous_snapshot FROM usage_price_revisions WHERE id>?1 ORDER BY id LIMIT 1000",
                    )?
                    .query_map([cursor], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, SqlValue>(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let Some(last) = rows.last() else { break };
                cursor = last.0;
                for (id, snapshot) in rows {
                    let snapshot = compatible_snapshot(&codec::decode(&tx, (&snapshot).into())?)?;
                    tx.execute(
                        "UPDATE usage_price_revisions SET previous_snapshot=?2 WHERE id=?1",
                        params![id, snapshot],
                    )?;
                }
            }
        }
        tx.execute_batch("UPDATE usage_entries SET write_30m=NULL,write_12h=NULL;")?;
        tx.execute(
            "INSERT OR REPLACE INTO usage_meta(key,value) VALUES('rollback_omitted_lifetime_rows',?1)",
            [lifetime_rows.to_string()],
        )?;
        tx.commit()?;
        drop(conn);
        // Schema 4 releases already know how to migrate schema 3. Reuse the
        // existing converter instead of maintaining another rollup dialect.
        let mut result = downgrade(to)?;
        result["source_database"] = json!(database);
        result["omitted_lifetime_rows"] = lifetime_rows.into();
        Ok(result)
    })();
    if result.is_err() {
        // Only files belonging to this newly created copy are removed.
        let _ = std::fs::remove_file(to);
        for suffix in ["-wal", "-shm", "-journal", ".lock"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", to.display()));
        }
    }
    result
}

fn compatible_snapshot(raw: &str) -> Result<String> {
    let mut snapshot: Value = serde_json::from_str(raw)?;
    if let Some(rate) = snapshot.get_mut("rate").and_then(Value::as_object_mut) {
        for field in LIFETIME_RATE_FIELDS {
            rate.remove(field);
        }
    }
    Ok(serde_json::to_string(&snapshot)?)
}

#[cfg(test)]
#[path = "rollback_tests.rs"]
mod tests;
