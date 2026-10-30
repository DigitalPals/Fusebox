//! Keeps the usage database small and says how big it is.
//!
//! The writer thread runs the compactor in short steps between batches: it trains
//! a dictionary once the journal has enough rows, compresses rows still stored as
//! text, and, once nothing is left to compress, vacuums an older database once so
//! it switches to incremental vacuum. After that, every retention purge hands its
//! freed pages back to the filesystem. Offline, `usage compact` does the same in
//! one go and `usage downgrade` restores the schema-3 layout for older releases.
use super::*;
use std::time::{Duration, Instant};

const STEP_ROWS: i64 = 2_000;
pub(super) const STEP_INTERVAL: Duration = Duration::from_millis(250);
const RECHECK: Duration = Duration::from_secs(3600);

/// The compactor's progress and the database's size, shared with status and faults.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DatabaseStats {
    /// The database and its WAL on disk.
    pub bytes: u64,
    /// Pages holding data; the rest are free pages awaiting reuse or vacuum.
    pub live_bytes: u64,
    pub free_bytes: u64,
    pub observations: i64,
    /// Average stored bytes per observation, including indexes and rollups.
    pub bytes_per_observation: u64,
    /// Observations a day over the last week, times bytes per observation.
    pub growth_bytes_per_day: u64,
    /// Space left on the filesystem that holds the database.
    pub disk_free_bytes: Option<u64>,
    /// Rows still stored as uncompressed text; zero once compaction has finished.
    pub uncompressed_rows: i64,
    pub compressing: bool,
    pub incremental_vacuum: bool,
    pub updated_at_ms: i64,
}

pub(super) struct Compactor {
    path: PathBuf,
    collector: bool,
    cursor: i64,
    pending: bool,
    /// Set when a cycle starts: dictionaries are checked once per cycle, not per step.
    fresh_cycle: bool,
    last_step: Instant,
    last_check: Instant,
    warned_disk: bool,
}

impl Compactor {
    pub fn new(path: &Path, collector: bool) -> Self {
        Self {
            path: path.to_path_buf(),
            collector,
            cursor: 0,
            pending: true,
            fresh_cycle: true,
            last_step: Instant::now(),
            last_check: Instant::now(),
            warned_disk: false,
        }
    }

    /// One bounded unit of work, at most every 250 ms while there is work, and a
    /// fresh look once an hour after that.
    pub fn tick(&mut self, conn: &mut Connection) -> Result<()> {
        if !self.pending {
            if self.last_check.elapsed() < RECHECK {
                return Ok(());
            }
            self.pending = true;
            self.fresh_cycle = true;
        }
        if self.last_step.elapsed() < STEP_INTERVAL {
            return Ok(());
        }
        self.last_step = Instant::now();
        self.last_check = Instant::now();
        let fresh = std::mem::take(&mut self.fresh_cycle);
        if fresh {
            // Rows written while a dictionary exists are compressed as they arrive, so
            // a cycle resumes where the last one stopped instead of rescanning.
            self.cursor = saved_cursor(conn)?;
        }
        if fresh && codec::needs_training(conn)? {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let trained = codec::train(&tx)?;
            tx.commit()?;
            if let Some(id) = trained {
                tracing::info!(dictionary = id, "trained a usage compression dictionary");
            }
        }
        if codec::active(conn)?.is_none() {
            // Too few rows to learn from yet.
            self.pending = false;
            return Ok(());
        }
        let compressed = compress_step(conn, &mut self.cursor, STEP_ROWS)?;
        if compressed == 0 {
            self.pending = false;
            if !self.collector {
                self.reclaim(conn)?;
            }
        }
        Ok(())
    }

    pub fn busy(&self) -> bool {
        self.pending
    }

    pub fn pause(&mut self) {
        self.pending = false;
        self.last_check = Instant::now();
    }

    /// Once compression is done: switch an older database to incremental vacuum with
    /// one full vacuum (needs free disk for a copy), or hand free pages back.
    fn reclaim(&mut self, conn: &mut Connection) -> Result<()> {
        let incremental: i64 = conn.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
        if incremental == 2 {
            release_free_pages(conn)?;
            return Ok(());
        }
        let (page_size, pages, free): (u64, u64, u64) = conn.query_row(
            "SELECT page_size,page_count,freelist_count FROM pragma_page_size(),pragma_page_count(),pragma_freelist_count()",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let live = (pages - free) * page_size;
        // VACUUM writes a temporary copy and then the new pages through the WAL.
        let needed = live.saturating_mul(2) + (256 << 20);
        let available = disk_free(&self.path);
        if available.is_some_and(|a| a < needed) {
            if !self.warned_disk {
                tracing::warn!(
                    needed_mb = needed >> 20,
                    available_mb = available.unwrap_or(0) >> 20,
                    "not enough free disk to vacuum the usage database; its free pages are reused instead (run `fusebox usage compact` with more space to shrink the file)"
                );
                self.warned_disk = true;
            }
            return Ok(());
        }
        let before = file_bytes(&self.path);
        let started = Instant::now();
        conn.execute_batch("PRAGMA auto_vacuum=INCREMENTAL; VACUUM;")?;
        // The vacuum wrote the whole database through the WAL. This one time, wait
        // (up to the busy timeout) for readers so the WAL is copied back and trimmed.
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        tracing::info!(
            before_mb = before >> 20,
            after_mb = file_bytes(&self.path) >> 20,
            seconds = started.elapsed().as_secs_f32(),
            "compacted the usage database"
        );
        Ok(())
    }
}

fn saved_cursor(conn: &Connection) -> Result<i64> {
    Ok(conn
        .query_row("SELECT CAST(value AS INTEGER) FROM usage_meta WHERE key='compact_cursor'", [], |r| r.get(0))
        .optional()?
        .unwrap_or(0))
}

/// Compresses up to `limit` text rows after `cursor`; returns how many it changed.
pub(super) fn compress_step(conn: &mut Connection, cursor: &mut i64, limit: i64) -> Result<usize> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut documents = Documents::new(&tx)?;
    if !documents.compressing() {
        return Ok(0);
    }
    let rows = tx
        .prepare_cached(
            "SELECT id,payload,snapshot_json FROM usage_observations
             WHERE id>?1 AND (typeof(payload)='text' OR typeof(snapshot_json)='text') ORDER BY id LIMIT ?2",
        )?
        .query_map(params![*cursor, limit], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, rusqlite::types::Value>(1)?, r.get::<_, rusqlite::types::Value>(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut update = tx.prepare_cached(
        "UPDATE usage_observations SET payload=?2,snapshot_json=?3,
           pricing_partial=COALESCE(pricing_partial,json_extract(?4,'$.partial')) WHERE id=?1",
    )?;
    let mut decoder = codec::Decoder::new(&tx);
    for (id, payload, snapshot) in &rows {
        let payload = decoder.decode(payload.into())?;
        let snapshot = decoder.decode(snapshot.into())?;
        update.execute(params![id, documents.store(&payload)?, documents.store(&snapshot)?, snapshot])?;
        *cursor = *id;
    }
    drop(decoder);
    drop(update);
    if rows.is_empty() {
        // Caught up: everything up to the newest row is compressed.
        *cursor = tx.query_row("SELECT COALESCE(MAX(id),0) FROM usage_observations", [], |r| r.get(0))?;
    }
    tx.execute(
        "INSERT INTO usage_meta(key,value) VALUES('compact_cursor',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [cursor.to_string()],
    )?;
    tx.commit()?;
    Ok(rows.len())
}

/// After a purge: return free pages to the filesystem when the database allows it.
pub(super) fn release_free_pages(conn: &Connection) -> Result<()> {
    let incremental: i64 = conn.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
    let free: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
    if incremental == 2 && free > 0 {
        conn.execute_batch("PRAGMA incremental_vacuum;")?;
        // The file shrinks at the next checkpoint. PASSIVE never waits for (or blocks)
        // other connections; the writer's journal_size_limit trims the WAL afterwards.
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);");
    }
    Ok(())
}

fn file_bytes(path: &Path) -> u64 {
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    [path, wal.as_path()].iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum()
}

fn disk_free(path: &Path) -> Option<u64> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs2::available_space(dir).ok()
}

pub(super) fn stats(conn: &Connection, path: &Path, compressing: bool) -> Result<DatabaseStats> {
    let (page_size, pages, free, incremental): (u64, u64, u64, i64) = conn.query_row(
        "SELECT page_size,page_count,freelist_count,auto_vacuum FROM pragma_page_size(),pragma_page_count(),pragma_freelist_count(),pragma_auto_vacuum()",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let now = Utc::now().timestamp_millis();
    let observations: i64 = conn.query_row("SELECT COUNT(*) FROM usage_observations", [], |r| r.get(0))?;
    let week: i64 = conn.query_row(
        "SELECT COUNT(*) FROM usage_observations INDEXED BY usage_event_time WHERE event_at_ms>=?1",
        [now - 7 * 86_400_000],
        |r| r.get(0),
    )?;
    // Only rows past the compactor's cursor can still be text: a range, not a scan.
    let uncompressed: i64 = if compressing {
        conn.query_row(
            "SELECT COUNT(*) FROM usage_observations WHERE id>?1 AND (typeof(payload)='text' OR typeof(snapshot_json)='text')",
            [saved_cursor(conn)?],
            |r| r.get(0),
        )?
    } else {
        0
    };
    let live = (pages - free) * page_size;
    let per = if observations > 0 { live / observations as u64 } else { 0 };
    Ok(DatabaseStats {
        bytes: file_bytes(path),
        live_bytes: live,
        free_bytes: free * page_size,
        observations,
        bytes_per_observation: per,
        growth_bytes_per_day: (week as u64 * per) / 7,
        disk_free_bytes: disk_free(path),
        uncompressed_rows: uncompressed,
        compressing,
        incremental_vacuum: incremental == 2,
        updated_at_ms: now,
    })
}

/// Holds `<database>.lock`: shared while a store writes, exclusive for the offline
/// commands that rewrite the file, so those never run beside a live server.
pub(crate) struct FileLock(#[allow(dead_code)] std::fs::File);

pub(crate) fn lock(path: &Path, exclusive: bool) -> Result<FileLock> {
    let lock_path = PathBuf::from(format!("{}.lock", path.display()));
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&lock_path).with_context(|| format!("open {}", lock_path.display()))?;
    // fs2 by name: newer standard libraries have an inherent File::try_lock_shared.
    let locked = if exclusive { fs2::FileExt::try_lock_exclusive(&file) } else { fs2::FileExt::try_lock_shared(&file) };
    if locked.is_err() {
        if exclusive {
            bail!("the usage database at {} is in use; stop Fusebox first", path.display());
        }
        bail!(
            "the usage database at {} is being compacted or downgraded; try again when that finishes",
            path.display()
        );
    }
    Ok(FileLock(file))
}

/// `fusebox usage compact`: compress every row and vacuum, with Fusebox stopped.
pub fn compact(path: &Path) -> Result<Value> {
    ensure!(path.is_file(), "there is no usage database at {}", path.display());
    let _lock = lock(path, true)?;
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    migrate(&mut conn)?;
    let before = file_bytes(path);
    if codec::needs_training(&conn)? {
        let tx = conn.transaction()?;
        codec::train(&tx)?;
        tx.commit()?;
    }
    let mut cursor = 0;
    let mut compressed = 0;
    loop {
        let n = compress_step(&mut conn, &mut cursor, 5_000)?;
        if n == 0 {
            break;
        }
        compressed += n;
    }
    conn.execute_batch("PRAGMA auto_vacuum=INCREMENTAL; VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
    let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    ensure!(check == "ok", "the compacted database failed its check: {check}");
    drop(conn);
    Ok(json!({"database":path,"compressed_rows":compressed,"bytes_before":before,"bytes_after":file_bytes(path)}))
}

/// `fusebox usage downgrade`: restore the schema-3 layout so a release before 0.6
/// can open the database again. Fusebox must be stopped.
pub fn downgrade(path: &Path) -> Result<Value> {
    ensure!(path.is_file(), "there is no usage database at {}", path.display());
    let _lock = lock(path, true)?;
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    ensure!(version == VERSION, "the usage database is at schema {version}; downgrade starts from {VERSION}");
    let lifetime_rows: i64 = conn.query_row(
        "SELECT count(*) FROM usage_observations WHERE write_30m IS NOT NULL OR write_12h IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    ensure!(
        lifetime_rows == 0,
        "cannot downgrade: {lifetime_rows} observations contain OpenAI cache lifetimes that older releases cannot preserve; back up the database and use a release supporting usage schema {VERSION}"
    );
    // Decompress in batches before the single restructuring transaction.
    let mut decompressed = 0_usize;
    loop {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let rows = tx
            .prepare(
                "SELECT id,payload,snapshot_json FROM usage_observations
                 WHERE typeof(payload)='blob' OR typeof(snapshot_json)='blob' ORDER BY id LIMIT 5000",
            )?
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, rusqlite::types::Value>(1)?,
                    r.get::<_, rusqlite::types::Value>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.is_empty() {
            break;
        }
        let mut decoder = codec::Decoder::new(&tx);
        for (id, payload, snapshot) in &rows {
            let payload = decoder.decode(payload.into())?;
            let snapshot = decoder.decode(snapshot.into())?;
            tx.execute(
                "UPDATE usage_observations SET payload=?2,snapshot_json=?3 WHERE id=?1",
                params![id, payload, snapshot],
            )?;
        }
        decompressed += rows.len();
        drop(decoder);
        tx.commit()?;
    }
    let tx = conn.transaction()?;
    // Old releases reject unknown rate fields. Retain the full catalogue for
    // audit, and omit rate combinations their calculator cannot represent.
    let catalogue: String = tx.query_row("SELECT value FROM usage_meta WHERE key='catalogue'", [], |r| r.get(0))?;
    let mut legacy: Value = serde_json::from_str(&catalogue)?;
    legacy.as_object_mut().unwrap().remove("calculation");
    let rates = legacy["rates"].as_array_mut().ok_or_else(|| anyhow!("invalid saved catalogue"))?;
    let before_rates = rates.len();
    rates.retain(|r| {
        r.get("above_threshold_write_5m_nanos_per_token").is_none()
            && r.get("above_threshold_write_1h_nanos_per_token").is_none()
            && r.get("cache_write_30m_nanos_per_token").is_none()
            && r.get("cache_write_12h_nanos_per_token").is_none()
            && r.get("above_threshold_write_30m_nanos_per_token").is_none()
            && r.get("above_threshold_write_12h_nanos_per_token").is_none()
    });
    let omitted_rates = before_rates - rates.len();
    for rate in rates {
        if let Some(excluded) = rate.as_object_mut().and_then(|r| r.remove("long_context_excluded_tiers")) {
            for tier in excluded.as_array().into_iter().flatten().filter_map(Value::as_str) {
                if let Some(tiers) = rate["tier_multipliers"].as_object_mut() {
                    tiers.remove(tier);
                }
            }
        }
    }
    tx.execute("INSERT OR REPLACE INTO usage_meta(key,value) VALUES('catalogue_before_downgrade',?1)", [&catalogue])?;
    tx.execute("UPDATE usage_meta SET value=?1 WHERE key='catalogue'", [serde_json::to_string(&legacy)?])?;
    for trigger in rollup::TRIGGERS {
        tx.execute_batch(&format!("DROP TRIGGER IF EXISTS {trigger};"))?;
    }
    tx.execute_batch(&format!(
        "DROP TABLE usage_rollup;
         DROP TABLE usage_facets;
         DROP TABLE usage_dictionaries;
         DROP INDEX usage_entries_imported;
         DROP INDEX IF EXISTS usage_entries_unpriced_time;
         DELETE FROM usage_meta WHERE key IN ('database_id','active_dictionary','compact_cursor');
         ALTER TABLE usage_observations DROP COLUMN pricing_partial;
         ALTER TABLE usage_observations DROP COLUMN write_30m;
         ALTER TABLE usage_observations DROP COLUMN write_12h;
         ALTER TABLE usage_entries DROP COLUMN write_30m;
         ALTER TABLE usage_entries DROP COLUMN write_12h;
         CREATE TABLE usage_source_entries AS SELECT {LEGACY_ENTRY_COLUMNS} FROM usage_observations WHERE 0;
         CREATE UNIQUE INDEX usage_source_entries_key ON usage_source_entries(source,canonical_key);
         CREATE INDEX usage_source_entries_event_time ON usage_source_entries(event_at_ms);
         CREATE INDEX usage_source_entries_filters ON usage_source_entries(provider,model,account_id,client_id,event_at_ms);
         CREATE INDEX usage_source_canonical ON usage_observations(source,canonical_key,trust_rank);
         CREATE INDEX usage_filters ON usage_observations(provider,model,account_id,client_id,event_at_ms);
         CREATE INDEX usage_entries_filters ON usage_entries(provider,model,account_id,client_id,event_at_ms);
         INSERT INTO usage_source_entries SELECT {LEGACY_ENTRY_COLUMNS} FROM (
           SELECT o.*,row_number() OVER (PARTITION BY o.source,o.canonical_key ORDER BY {LEGACY_WINNER_ORDER}) AS winner
           FROM usage_observations o WHERE NOT EXISTS(SELECT 1 FROM usage_suppressed s WHERE s.source=o.source
             AND s.source_event_id=o.source_event_id AND (s.origin_id='' OR s.origin_id=o.origin_id))
         ) WHERE winner=1;"
    ))?;
    tx.pragma_update(None, "user_version", LEGACY_VERSION)?;
    tx.commit()?;
    let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    ensure!(check == "ok", "the downgraded database failed its check: {check}");
    Ok(
        json!({"database":path,"schema":LEGACY_VERSION,"decompressed_rows":decompressed,"omitted_unsupported_rates":omitted_rates}),
    )
}
