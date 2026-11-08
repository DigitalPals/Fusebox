//! Totals that outlive their records. Retention deletes usage records after
//! `usage.retention-days`; in the same transaction, their 15-minute totals move
//! here, so the Usage page keeps every past day and month, at the rollups'
//! resolution, after the records themselves are gone. `usage.totals-retention-days`
//! bounds these totals; 0 keeps them for good.
//!
//! `usage_history` holds what the summary counts: proxy entries, and imported
//! entries no proxy entry matched, by provider, model, account, client and source,
//! with their unpriced reasons and a count of the matched imports it left out.
//! `usage_history_entrypoints` does the same for proxy entries that named a Claude
//! Code entrypoint, and `usage_history_facets` keeps what the filter menus offer.
//! Each row names its provider, model, account, client, source and entrypoint by a
//! number from `usage_history_dims`, which holds every combination once: the names
//! are most of a row's size, and a few hundred combinations cover years of buckets.
//!
//! A purge can also move an entry forward: when a newer record shares its key, the
//! rebuilt entry is that one, still counted where it is, so the old one is not kept
//! here. Records never come back into a period kept as totals (see `floor`), so
//! nothing is counted twice. The tables sit beside the schema-5 layout, without
//! triggers: releases that don't know them leave them alone.
use super::rollup::{AMOUNTS, BUCKET_MS};
use super::*;
use std::collections::BTreeMap;

/// Unpriced reasons in column order (`u0`..`u4`), as the summary names them.
pub(super) const REASONS: [&str; 5] =
    ["missing_usage", "incomplete_usage", "invalid_usage", "unknown_model", "unsupported_pricing"];
const FACTS: [&str; 3] = ["usage_history", "usage_history_entrypoints", "usage_history_facets"];
/// Every table and index the kept totals use, for measuring them.
pub(super) const OBJECTS: [&str; 5] = [
    "usage_history",
    "usage_history_entrypoints",
    "usage_history_facets",
    "usage_history_dims",
    "sqlite_autoindex_usage_history_dims_1",
];
/// Indexes of input, cache read, cache write and output in `AMOUNTS`: the tokens
/// whose absence makes usage missing or incomplete.
const REQUIRED: [usize; 4] = [1, 2, 3, 6];

/// Why a call has no price: the same answer for live entries and kept totals.
pub(super) fn unpriced_reason(tokens: [Option<i64>; 4], basis: &str) -> &'static str {
    let basis = basis.trim_start_matches("local_override:");
    if tokens.iter().all(Option::is_none) {
        "missing_usage"
    } else if tokens.iter().any(Option::is_none) {
        "incomplete_usage"
    } else if basis == "invalid_tokens" {
        "invalid_usage"
    } else if basis == "unknown_model" {
        "unknown_model"
    } else {
        "unsupported_pricing"
    }
}

fn numbers() -> String {
    (0..AMOUNTS.len())
        .flat_map(|i| [format!("k{i} INTEGER NOT NULL"), format!("s{i} INTEGER NOT NULL")])
        .chain((0..REASONS.len()).map(|i| format!("u{i} INTEGER NOT NULL")))
        .collect::<Vec<_>>()
        .join(",")
}

/// Creates the tables when a database lacks them; an existing one is left as is.
pub(super) fn ensure(conn: &Connection) -> Result<()> {
    let numbers = numbers();
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS usage_history_dims(id INTEGER PRIMARY KEY,dims TEXT NOT NULL UNIQUE,provider TEXT,
           model TEXT,account_id TEXT,client TEXT,source TEXT NOT NULL,entrypoint TEXT,last_bucket INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS usage_history(bucket INTEGER NOT NULL,dim INTEGER NOT NULL,observations INTEGER NOT NULL,
           partial INTEGER NOT NULL,missing INTEGER NOT NULL,pricing_partial INTEGER NOT NULL,matched INTEGER NOT NULL,
           weak INTEGER NOT NULL,{numbers},PRIMARY KEY(bucket,dim)) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS usage_history_entrypoints(bucket INTEGER NOT NULL,dim INTEGER NOT NULL,
           observations INTEGER NOT NULL,partial INTEGER NOT NULL,missing INTEGER NOT NULL,pricing_partial INTEGER NOT NULL,
           {numbers},PRIMARY KEY(bucket,dim)) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS usage_history_facets(bucket INTEGER NOT NULL,dim INTEGER NOT NULL,
           observations INTEGER NOT NULL,PRIMARY KEY(bucket,dim)) WITHOUT ROWID;"
    ))?;
    Ok(())
}

/// Collector outboxes keep no totals: they only forward records.
pub(super) fn exists(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='usage_history')",
        [],
        |r| r.get(0),
    )?)
}

/// The earliest time a record may be stored at: before it, records were deleted,
/// either for good or with their totals kept. `usage backfill` lowers only the
/// purge watermark, so it can never add records to a period kept as totals.
pub(crate) fn floor(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT MAX(COALESCE(MAX(CASE WHEN key='purge_before_ms' THEN CAST(value AS INTEGER) END),0),
                    COALESCE(MAX(CASE WHEN key='totals_before_ms' THEN CAST(value AS INTEGER) END),0))
         FROM usage_meta WHERE key IN ('purge_before_ms','totals_before_ms')",
        [],
        |r| r.get(0),
    )?)
}

/// The key a combination of names is kept under. `source` is `proxy` for entrypoints.
fn dims_sql(alias: &str, client: &str, source: &str, entrypoint: &str) -> String {
    format!("json_array({alias}.provider,{alias}.model,{alias}.account_id,{client},{source},{entrypoint})")
}

/// The purge's first half, before it deletes anything: note the entries it will
/// delete, and keep the filter values of every record it will delete. False when
/// this database keeps no totals.
pub(super) fn stage(tx: &Connection, before: i64) -> Result<bool> {
    if !exists(tx)? {
        return Ok(false);
    }
    tx.execute_batch(&format!(
        "DROP TABLE IF EXISTS temp.usage_purged;
         CREATE TEMP TABLE usage_purged(bucket INTEGER NOT NULL,canonical_key TEXT NOT NULL,association_key TEXT NOT NULL,
           source TEXT NOT NULL,weak INTEGER NOT NULL,provider TEXT,model TEXT,account_id TEXT,client TEXT,
           completeness TEXT NOT NULL,pricing_partial INTEGER NOT NULL,pricing_basis TEXT NOT NULL,entrypoint TEXT,{});",
        AMOUNTS.join(",")
    ))?;
    tx.execute(
        &format!(
            "INSERT INTO temp.usage_purged SELECT e.event_at_ms/{BUCKET_MS},e.canonical_key,e.association_key,e.source,
               COALESCE(e.response_id,'')='',e.provider,e.model,e.account_id,{},e.completeness,
               COALESCE(e.pricing_partial,0)=1,e.pricing_basis,(SELECT entrypoint FROM usage_entrypoints x WHERE x.id=e.id),{}
             FROM usage_entries e INDEXED BY usage_entries_event_time WHERE e.event_at_ms<?1",
            rollup::client("e"),
            AMOUNTS.map(|c| format!("e.{c}")).join(",")
        ),
        [before],
    )?;
    tx.execute_batch("CREATE INDEX temp.usage_purged_association ON usage_purged(association_key,source);")?;
    // The filter values of every record, winners or not, as `usage_facets` has them.
    let client = rollup::client("o");
    let dims = dims_sql("o", &client, "o.source", "NULL");
    tx.execute(
        &format!(
            "INSERT INTO usage_history_dims(dims,provider,model,account_id,client,source,last_bucket)
             SELECT {dims},o.provider,o.model,o.account_id,{client},o.source,MAX(o.event_at_ms/{BUCKET_MS})
             FROM usage_observations o INDEXED BY usage_event_time WHERE o.event_at_ms<?1 GROUP BY 1
             ON CONFLICT(dims) DO UPDATE SET last_bucket=MAX(last_bucket,excluded.last_bucket)"
        ),
        [before],
    )?;
    tx.execute(
        &format!(
            "INSERT INTO usage_history_facets(bucket,dim,observations)
             SELECT o.event_at_ms/{BUCKET_MS},d.id,COUNT(*) FROM usage_observations o INDEXED BY usage_event_time
             JOIN usage_history_dims d ON d.dims={dims} WHERE o.event_at_ms<?1 GROUP BY 1,2
             ON CONFLICT(bucket,dim) DO UPDATE SET observations=observations+excluded.observations"
        ),
        [before],
    )?;
    Ok(true)
}

/// An explicit purge's part: delete the kept totals of every bucket that starts
/// before `before`. Returns false: there is nothing to seal afterwards.
pub(super) fn forget(tx: &Connection, before: i64) -> Result<bool> {
    if exists(tx)? {
        let (end, _) = buckets(before, before);
        drop_before(tx, end)?;
    }
    Ok(false)
}

/// Deletes the kept totals of buckets before `end`, and the names only they used.
fn drop_before(tx: &Connection, end: i64) -> Result<u64> {
    let mut deleted = 0;
    for table in FACTS {
        deleted += tx.execute(&format!("DELETE FROM {table} WHERE bucket<?1"), [end])? as u64;
    }
    tx.execute("DELETE FROM usage_history_dims WHERE last_bucket<?1", [end])?;
    Ok(deleted)
}

#[derive(Default)]
struct Totals {
    provider: Option<String>,
    model: Option<String>,
    account: Option<String>,
    client: Option<String>,
    source: String,
    entrypoint: Option<String>,
    counts: [i64; 4],
    matched: i64,
    weak: i64,
    known: [i64; 10],
    sums: [i128; 10],
    reasons: [i64; 5],
}

impl Totals {
    fn add(&mut self, amounts: &[Option<i64>; 10], completeness: &str, pricing_partial: bool, basis: &str) {
        self.counts[0] += 1;
        self.counts[1] += i64::from(completeness == "partial");
        self.counts[2] += i64::from(completeness == "missing");
        self.counts[3] += i64::from(pricing_partial);
        for (i, amount) in amounts.iter().enumerate() {
            if let Some(amount) = amount {
                self.known[i] += 1;
                self.sums[i] += i128::from(*amount);
            }
        }
        if amounts[0].is_none() {
            let reason = unpriced_reason(REQUIRED.map(|i| amounts[i]), basis);
            self.reasons[REASONS.iter().position(|r| *r == reason).unwrap_or(4)] += 1;
        }
    }

    /// The numeric columns in table order, `extra` (matched and weak) after the four
    /// counts. A sum past 64 bits is stored REAL, as the rollup triggers would;
    /// readers report it as out of range.
    fn numbers(&self, extra: bool) -> Vec<SqlValue> {
        let mut values: Vec<SqlValue> = self.counts.iter().map(|n| SqlValue::from(*n)).collect();
        if extra {
            values.extend([SqlValue::from(self.matched), SqlValue::from(self.weak)]);
        }
        for i in 0..AMOUNTS.len() {
            values.push(self.known[i].into());
            values.push(match i64::try_from(self.sums[i]) {
                Ok(n) => SqlValue::Integer(n),
                Err(_) => SqlValue::Real(self.sums[i] as f64),
            });
        }
        values.extend(self.reasons.iter().map(|n| SqlValue::from(*n)));
        values
    }
}

/// Adds `rows` to `table`, keyed by bucket and the number of their names.
fn upsert(tx: &Connection, table: &str, extra: bool, rows: BTreeMap<(i64, String), Totals>) -> Result<()> {
    let numbers: Vec<String> = ["observations", "partial", "missing", "pricing_partial"]
        .into_iter()
        .chain(if extra { ["matched", "weak"].as_slice() } else { [].as_slice() }.iter().copied())
        .map(String::from)
        .chain((0..AMOUNTS.len()).flat_map(|i| [format!("k{i}"), format!("s{i}")]))
        .chain((0..REASONS.len()).map(|i| format!("u{i}")))
        .collect();
    let mut name = tx.prepare_cached(
        "INSERT INTO usage_history_dims(dims,provider,model,account_id,client,source,entrypoint,last_bucket)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(dims) DO UPDATE SET last_bucket=MAX(last_bucket,excluded.last_bucket)
         RETURNING id",
    )?;
    let mut insert = tx.prepare(&format!(
        "INSERT INTO {table}(bucket,dim,{}) VALUES({}) ON CONFLICT(bucket,dim) DO UPDATE SET {}",
        numbers.join(","),
        (1..=numbers.len() + 2).map(|i| format!("?{i}")).collect::<Vec<_>>().join(","),
        numbers.iter().map(|c| format!("{c}={c}+excluded.{c}")).collect::<Vec<_>>().join(",")
    ))?;
    for ((bucket, dims), t) in rows {
        let dim: i64 = name.query_row(
            params![dims, t.provider, t.model, t.account, t.client, t.source, t.entrypoint, bucket],
            |r| r.get(0),
        )?;
        let mut values: Vec<SqlValue> = vec![bucket.into(), dim.into()];
        values.extend(t.numbers(extra));
        insert.execute(rusqlite::params_from_iter(values))?;
    }
    Ok(())
}

/// The purge's second half, after it deleted the records and rebuilt the entries
/// it touched: keep the totals of every staged entry that no longer counts anywhere.
pub(super) fn seal(tx: &Connection, before: i64) -> Result<()> {
    let mut kept: BTreeMap<(i64, String), Totals> = BTreeMap::new();
    let mut entrypoints: BTreeMap<(i64, String), Totals> = BTreeMap::new();
    {
        // An import counts unless a proxy entry has its response, whether that entry
        // stays or was purged with it: the summary's rule, applied once, here.
        let mut statement = tx.prepare(&format!(
            "SELECT p.bucket,{},{},p.provider,p.model,p.account_id,p.client,p.source,p.entrypoint,p.weak,p.completeness,
               p.pricing_partial,p.pricing_basis,
               p.source<>'proxy' AND (
                 EXISTS(SELECT 1 FROM usage_entries x WHERE x.association_key=p.association_key AND x.source='proxy')
                 OR EXISTS(SELECT 1 FROM temp.usage_purged x WHERE x.association_key=p.association_key AND x.source='proxy')),
               {}
             FROM temp.usage_purged p
             WHERE NOT EXISTS(SELECT 1 FROM usage_entries e WHERE e.canonical_key=p.canonical_key)",
            dims_sql("p", "p.client", "p.source", "NULL"),
            dims_sql("p", "p.client", "'proxy'", "p.entrypoint"),
            AMOUNTS.map(|c| format!("p.{c}")).join(",")
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let bucket: i64 = row.get(0)?;
            let source: String = row.get(7)?;
            let entrypoint: Option<String> = row.get(8)?;
            let start = |source: &str, entrypoint: Option<String>| Totals {
                provider: row.get(3).ok().flatten(),
                model: row.get(4).ok().flatten(),
                account: row.get(5).ok().flatten(),
                client: row.get(6).ok().flatten(),
                source: source.into(),
                entrypoint,
                ..Default::default()
            };
            let totals = kept.entry((bucket, row.get(1)?)).or_insert_with(|| start(&source, None));
            if row.get::<_, bool>(13)? {
                totals.matched += 1;
                continue;
            }
            let mut amounts = [None; 10];
            for (i, amount) in amounts.iter_mut().enumerate() {
                *amount = row.get(14 + i)?;
            }
            let completeness: String = row.get(10)?;
            let pricing_partial: bool = row.get(11)?;
            let basis: String = row.get(12)?;
            totals.weak += i64::from(source != "proxy" && row.get::<_, bool>(9)?);
            totals.add(&amounts, &completeness, pricing_partial, &basis);
            if source == "proxy" && entrypoint.is_some() {
                entrypoints.entry((bucket, row.get(2)?)).or_insert_with(|| start("proxy", entrypoint)).add(
                    &amounts,
                    &completeness,
                    pricing_partial,
                    &basis,
                );
            }
        }
    }
    upsert(tx, "usage_history", true, kept)?;
    upsert(tx, "usage_history_entrypoints", false, entrypoints)?;
    tx.execute_batch("DROP TABLE temp.usage_purged;")?;
    tx.execute(
        "INSERT INTO usage_meta(key,value) VALUES('totals_before_ms',?1) ON CONFLICT(key)
         DO UPDATE SET value=CAST(MAX(CAST(value AS INTEGER),CAST(excluded.value AS INTEGER)) AS TEXT)",
        [before.to_string()],
    )?;
    Ok(())
}

/// Drops totals older than `days`; 0 keeps them.
pub(super) fn trim(conn: &mut Connection, days: u32) -> Result<u64> {
    if days == 0 || !exists(conn)? {
        return Ok(0);
    }
    let cutoff = (Utc::now().timestamp_millis() - i64::from(days) * 86_400_000).max(0).div_euclid(BUCKET_MS);
    let tx = conn.transaction()?;
    let deleted = drop_before(&tx, cutoff)?;
    tx.commit()?;
    Ok(deleted)
}

/// The buckets a range holds as kept totals: those that start inside it. Totals
/// have no finer resolution, and every day in every current timezone starts on a
/// bucket boundary, so whole days and months are exact.
pub(super) fn buckets(start: i64, end: i64) -> (i64, i64) {
    let ceil = |ms: i64| ms.div_euclid(BUCKET_MS) + i64::from(ms.rem_euclid(BUCKET_MS) != 0);
    (ceil(start), ceil(end))
}

/// A kept-totals table joined to its names, for `rollup::filter`'s conditions.
pub(super) fn named(table: &str) -> String {
    format!("{table} h JOIN usage_history_dims d ON d.id=h.dim")
}

/// When the first proxy call kept only as totals happened (its bucket's start).
pub(super) fn first_proxy(conn: &Connection) -> Result<Option<i64>> {
    if !exists(conn)? {
        return Ok(None);
    }
    Ok(conn
        .query_row(
            &format!(
                "SELECT h.bucket FROM {} WHERE d.source='proxy' ORDER BY h.bucket LIMIT 1",
                named("usage_history")
            ),
            [],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .map(|bucket| bucket * BUCKET_MS))
}

/// Before this time the database holds only totals; None while it keeps none.
pub(super) fn summarized_before(conn: &Connection) -> Result<Option<i64>> {
    if !exists(conn)? || !conn.query_row("SELECT EXISTS(SELECT 1 FROM usage_history)", [], |r| r.get::<_, bool>(0))? {
        return Ok(None);
    }
    Ok(conn
        .query_row("SELECT CAST(value AS INTEGER) FROM usage_meta WHERE key='totals_before_ms'", [], |r| r.get(0))
        .optional()?)
}
