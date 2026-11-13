//! Output speed per model. `usage_timings` keeps, by observation id, how long a
//! proxy call took, when its first content arrived and the speed tier it ran at;
//! `usage_speed_rollup` sums the proxy entries that have one, like `usage_rollup`
//! with the tier as a fifth dimension. Triggers on `usage_entries` keep it exact: a
//! new call's timing lands before its entry is chosen, and a purge deletes entries
//! before their timings. Only the history backfill adds timings to entries that
//! already exist, and it adds them to the rollup itself, a batch at a time.
//! (`usage_entries` has no index on `id`, so a trigger on the timings could only
//! find an entry by reading them all.)
//!
//! Both tables sit beside the schema-5 layout, like the entrypoint tables: releases
//! that don't know them keep reading and writing the database, and the entry
//! triggers keep the rollup exact meanwhile.
//!
//! A call is *timed* when content arrived, it reported output tokens, and it kept
//! streaming for at least [`MIN_STREAM_MS`] at no more than [`MAX_RATE`] tokens a
//! second; anything faster arrived in one piece and says nothing about speed. Speed
//! is output tokens over streaming time (total minus time to first content), summed
//! over timed calls, so long answers weigh more than short ones.
use super::*;
use std::{collections::BTreeMap, time::Instant};

/// The rule of [`super::super::types::stream_ms`], which the request log uses.
pub(super) const MIN_STREAM_MS: i64 = super::super::types::MIN_STREAM_MS as i64;
pub(super) const MAX_RATE: i64 = super::super::types::MAX_STREAM_RATE as i64;
const STEP_ROWS: i64 = 2_000;

/// Trigger names, for a downgrade to drop.
pub(super) const TRIGGERS: [&str; 4] = [
    "usage_speed_entry_insert",
    "usage_speed_entry_delete",
    "usage_speed_entry_update_old",
    "usage_speed_entry_update_new",
];

/// What one entry `e` with timing `t` adds to its rollup row. An entry without an
/// output count (NULL) is not timed: `timed` is always 0 or 1, never NULL.
fn row(e: &str, t: &str) -> String {
    let output = format!("COALESCE({e}.output,0)");
    let timed = format!(
        "({t}.ttft_ms IS NOT NULL AND {output}>0 AND {t}.total_ms-{t}.ttft_ms>={MIN_STREAM_MS}
          AND {output}*1000<={MAX_RATE}*({t}.total_ms-{t}.ttft_ms))"
    );
    format!(
        "{e}.event_at_ms/{bucket} AS bucket,
         json_array({e}.provider,{e}.model,{e}.account_id,{client},{t}.speed) AS dims,
         {e}.provider AS provider,{e}.model AS model,{e}.account_id AS account_id,{client} AS client,{t}.speed AS speed,
         {timed} AS timed,CASE WHEN {timed} THEN {output} ELSE 0 END AS output,
         CASE WHEN {timed} THEN {t}.total_ms-{t}.ttft_ms ELSE 0 END AS stream_ms,
         CASE WHEN {timed} THEN {t}.ttft_ms ELSE 0 END AS ttft_ms",
        bucket = rollup::BUCKET_MS,
        client = rollup::client(e)
    )
}

const SUMS: [&str; 4] = ["timed", "output", "stream_ms", "ttft_ms"];
const COLUMNS: &str = "bucket,dims,provider,model,account_id,client,speed,calls,timed,output,stream_ms,ttft_ms";

/// Adds the rows `source` selects; `source` names `e` and `t` and ends in a WHERE
/// clause, which also keeps SQLite from reading ON CONFLICT as a join constraint.
fn add(e: &str, t: &str, source: &str) -> String {
    format!(
        "INSERT INTO usage_speed_rollup({COLUMNS})
         SELECT bucket,dims,provider,model,account_id,client,speed,1,timed,output,stream_ms,ttft_ms
         FROM (SELECT {} {source})
         WHERE 1 ON CONFLICT(bucket,dims) DO UPDATE SET {}
           calls=usage_speed_rollup.calls+1;",
        row(e, t),
        SUMS.map(|c| format!("{c}=usage_speed_rollup.{c}+excluded.{c},")).join("")
    )
}

fn remove(e: &str, t: &str, source: &str) -> String {
    let x = format!("(SELECT {} {source})", row(e, t));
    format!(
        "UPDATE usage_speed_rollup SET {} calls=usage_speed_rollup.calls-1
         FROM {x} AS x WHERE usage_speed_rollup.bucket=x.bucket AND usage_speed_rollup.dims=x.dims;
         DELETE FROM usage_speed_rollup WHERE calls<=0 AND (bucket,dims) IN (SELECT bucket,dims FROM {x});",
        SUMS.map(|c| format!("{c}=usage_speed_rollup.{c}-x.{c},")).join("")
    )
}

/// The four triggers, as `CREATE TRIGGER` statements (also how SQLite stores them).
pub(super) fn trigger_sql() -> [String; 4] {
    // Trigger bodies name the entry NEW or OLD; its timing is selected under `t`.
    let entry_timing = |id: &str| format!("FROM usage_timings t WHERE t.id={id}.id");
    let added = add("NEW", "t", &entry_timing("NEW"));
    let removed = remove("OLD", "t", &entry_timing("OLD"));
    [
        format!(
            "CREATE TRIGGER {} AFTER INSERT ON usage_entries WHEN NEW.source='proxy' BEGIN {added} END",
            TRIGGERS[0]
        ),
        format!(
            "CREATE TRIGGER {} AFTER DELETE ON usage_entries WHEN OLD.source='proxy' BEGIN {removed} END",
            TRIGGERS[1]
        ),
        format!(
            "CREATE TRIGGER {} AFTER UPDATE ON usage_entries WHEN OLD.source='proxy' BEGIN {removed} END",
            TRIGGERS[2]
        ),
        format!(
            "CREATE TRIGGER {} AFTER UPDATE ON usage_entries WHEN NEW.source='proxy' BEGIN {added} END",
            TRIGGERS[3]
        ),
    ]
}

/// Creates the timing tables and triggers when a database lacks them, and marks
/// the observations already stored for the backfill. Triggers an earlier build defined
/// differently are replaced and the rollup rebuilt; the timings and the backfill mark stay.
pub(super) fn ensure(conn: &Connection) -> Result<()> {
    let triggers = trigger_sql();
    let tables: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('usage_timings','usage_speed_rollup')",
        [],
        |r| r.get(0),
    )?;
    let mut current = tables == 2;
    for (name, sql) in TRIGGERS.iter().zip(&triggers) {
        let stored: Option<String> = conn
            .query_row("SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?1", [name], |r| r.get(0))
            .optional()?;
        current &= stored.as_deref() == Some(sql.as_str());
    }
    if current {
        return Ok(());
    }
    conn.execute_batch(&format!(
        "SAVEPOINT usage_speed;
         CREATE TABLE IF NOT EXISTS usage_timings(id INTEGER PRIMARY KEY,speed INTEGER NOT NULL,ttft_ms INTEGER,
           total_ms INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS usage_speed_rollup(bucket INTEGER NOT NULL,dims TEXT NOT NULL,provider TEXT,model TEXT,
           account_id TEXT,client TEXT,speed INTEGER NOT NULL,calls INTEGER NOT NULL,timed INTEGER NOT NULL,
           output INTEGER NOT NULL,stream_ms INTEGER NOT NULL,ttft_ms INTEGER NOT NULL,PRIMARY KEY(bucket,dims)) WITHOUT ROWID;
         {drops}
         {creates}
         DELETE FROM usage_speed_rollup;",
        drops = TRIGGERS.map(|t| format!("DROP TRIGGER IF EXISTS {t};")).join("\n"),
        creates = triggers.map(|t| format!("{t};")).join("\n"),
    ))?;
    let result = (|| -> Result<()> {
        rebuild(conn)?;
        // Everything stored so far gets its timing from the backfill, newest first.
        let below: i64 = conn.query_row("SELECT COALESCE(MAX(id),0)+1 FROM usage_observations", [], |r| r.get(0))?;
        let known: bool =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM usage_meta WHERE key='speed_backfill_below')", [], |r| {
                r.get(0)
            })?;
        if !known && below > 1 {
            conn.execute("INSERT INTO usage_meta(key,value) VALUES('speed_backfill_below',?1)", [below.to_string()])?;
        }
        Ok(())
    })();
    conn.execute_batch(if result.is_ok() {
        "RELEASE usage_speed;"
    } else {
        "ROLLBACK TO usage_speed; RELEASE usage_speed;"
    })?;
    result
}

/// Fills `usage_speed_rollup` from the entries and timings, as the triggers would have.
pub(super) fn rebuild(conn: &Connection) -> Result<()> {
    conn.execute_batch("DELETE FROM usage_speed_rollup;")?;
    conn.execute(&sum_into_rollup("1"), [])?;
    Ok(())
}

/// Adds to the rollup the proxy entries with a timing that `filter` (on `e` and `t`) selects.
fn sum_into_rollup(filter: &str) -> String {
    format!(
        "INSERT INTO usage_speed_rollup({COLUMNS})
         SELECT bucket,dims,MAX(provider),MAX(model),MAX(account_id),MAX(client),MAX(speed),COUNT(*),SUM(timed),SUM(output),
           SUM(stream_ms),SUM(ttft_ms)
         FROM (SELECT {} FROM usage_entries e INDEXED BY usage_entries_event_time JOIN usage_timings t ON t.id=e.id
               WHERE e.source='proxy' AND {filter})
         WHERE 1 GROUP BY bucket,dims
         ON CONFLICT(bucket,dims) DO UPDATE SET {} calls=usage_speed_rollup.calls+excluded.calls;",
        row("e", "t"),
        SUMS.map(|c| format!("{c}=usage_speed_rollup.{c}+excluded.{c},")).join("")
    )
}

/// The timing a proxy observation carries, as (speed, ttft_ms, total_ms).
pub(super) fn timing(o: &Observation) -> Option<(i64, Option<i64>, i64)> {
    if o.source != "proxy" {
        return None;
    }
    let total = i64::try_from(*o.numeric_metadata.get("request_latency_ms")?).ok()?;
    let ttft = o.numeric_metadata.get("request_ttft_ms").and_then(|n| i64::try_from(*n).ok()).filter(|n| *n <= total);
    Some((o.speed() as i64, ttft, total))
}

/// Records an inserted observation's timing, before its entry is chosen. It replaces
/// a stale row an older release left when it purged, should SQLite reuse the id.
pub(super) fn record(tx: &Connection, id: i64, o: &Observation) -> Result<()> {
    if let Some((speed, ttft, total)) = timing(o) {
        tx.prepare_cached("INSERT OR REPLACE INTO usage_timings(id,speed,ttft_ms,total_ms) VALUES(?1,?2,?3,?4)")?
            .execute(params![id, speed, ttft, total])?;
    }
    Ok(())
}

/// Adds the timings of up to `limit` stored proxy observations below the saved
/// mark, newest first, so recent days fill in before older ones. Returns how many
/// it read; 0 once there is nothing left, and the mark is gone.
pub(super) fn backfill_step(conn: &mut Connection, limit: i64) -> Result<usize> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let Some(below): Option<i64> = tx
        .query_row("SELECT CAST(value AS INTEGER) FROM usage_meta WHERE key='speed_backfill_below'", [], |r| r.get(0))
        .optional()?
    else {
        return Ok(0);
    };
    let rows = tx
        .prepare_cached(
            // `+source`: walk ids downward instead of sorting every proxy record by the source index.
            "SELECT id,event_at_ms,payload FROM usage_observations WHERE id<?1 AND +source='proxy' ORDER BY id DESC LIMIT ?2",
        )?
        .query_map(params![below, limit], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, rusqlite::types::Value>(2)?))
        })?
        .collect::<rusqlite::Result<Vec<(i64, i64, rusqlite::types::Value)>>>()?;
    let mut decoder = codec::Decoder::new(&tx);
    for (id, _, payload) in &rows {
        // A record that no longer parses has no timing to add.
        let Ok(text) = decoder.decode(payload.into()) else { continue };
        let Ok(o) = serde_json::from_str::<Observation>(&text) else { continue };
        record(&tx, *id, &o)?;
    }
    drop(decoder);
    // Ids below the mark had no timing, so this adds each of their entries once.
    // Entries are found by time: they keep their observation's event time.
    if let (Some(newest), Some(oldest)) = (rows.first(), rows.last()) {
        let (from, to) = rows.iter().fold((i64::MAX, i64::MIN), |(lo, hi), (_, at, _)| (lo.min(*at), hi.max(*at)));
        tx.prepare_cached(&sum_into_rollup("e.event_at_ms BETWEEN ?1 AND ?2 AND t.id BETWEEN ?3 AND ?4"))?
            .execute(params![from, to, oldest.0, newest.0])?;
    }
    match rows.last() {
        Some((id, _, _)) if rows.len() as i64 == limit => {
            tx.execute("UPDATE usage_meta SET value=?1 WHERE key='speed_backfill_below'", [id.to_string()])?;
        }
        _ => {
            tx.execute("DELETE FROM usage_meta WHERE key='speed_backfill_below'", [])?;
        }
    }
    tx.commit()?;
    Ok(rows.len())
}

pub(super) fn backfilling(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM usage_meta WHERE key='speed_backfill_below')", [], |r| r.get(0))?)
}

/// Runs the backfill from the writer loop in steps, like the compactor.
pub(super) struct Backfill {
    pending: bool,
    last_step: Instant,
}
impl Backfill {
    pub fn new(collector: bool) -> Self {
        Self { pending: !collector, last_step: Instant::now() }
    }
    pub fn busy(&self) -> bool {
        self.pending
    }
    pub fn tick(&mut self, conn: &mut Connection) -> Result<()> {
        if !self.pending || self.last_step.elapsed() < compact::STEP_INTERVAL {
            return Ok(());
        }
        self.last_step = Instant::now();
        // A failure stops it until the next start, which resumes at the saved mark.
        let read = backfill_step(conn, STEP_ROWS).inspect_err(|_| self.pending = false)?;
        self.pending = read > 0;
        Ok(())
    }
}

/// Speed sums for one group of calls.
#[derive(Default, Clone, Copy)]
struct Sums {
    calls: i64,
    timed: i64,
    output: i64,
    stream_ms: i64,
    ttft_ms: i64,
}
impl Sums {
    fn add(&mut self, o: &Sums) {
        self.calls += o.calls;
        self.timed += o.timed;
        self.output += o.output;
        self.stream_ms += o.stream_ms;
        self.ttft_ms += o.ttft_ms;
    }
}

type Key = (Option<String>, Option<String>, i64);

fn speed_name(code: i64) -> &'static str {
    match code {
        2 => "fast",
        3 => "ultrafast",
        _ => "standard",
    }
}

/// Per provider, model and speed tier, and per local day, for the range and the
/// equally long period before it. SQLite sums the rollup by bucket; only the
/// partial buckets at either end, or a read filtered by entrypoint, read entries.
pub(super) fn read(conn: &Connection, q: &Query) -> Result<Value> {
    read_with(conn, q, true)
}

#[cfg(test)]
pub(super) fn read_scan(conn: &Connection, q: &Query) -> Result<Value> {
    read_with(conn, q, false)
}

fn read_with(conn: &Connection, q: &Query, use_rollups: bool) -> Result<Value> {
    let r = range(q)?;
    let mut groups: BTreeMap<Key, Sums> = BTreeMap::new();
    let mut days: BTreeMap<(NaiveDate, Key), Sums> = BTreeMap::new();
    let mut previous: BTreeMap<Key, Sums> = BTreeMap::new();
    // Only proxy calls are timed.
    if q.source.as_deref().is_none_or(|s| s == "proxy") {
        let dates = std::cell::RefCell::new(LocalDates::new(r.tz.clone()));
        let date =
            |ms: i64| -> Result<NaiveDate> { dates.borrow_mut().date(ms).context("usage timestamp out of bounds") };
        collect(conn, q, &r, use_rollups, &mut |ms, key, sums| {
            days.entry((date(ms)?, key.clone())).or_default().add(sums);
            groups.entry(key).or_default().add(sums);
            Ok(())
        })?;
        let length = r.end - r.start;
        let before = Range { start: r.start - length, end: r.start, tz: r.tz.clone() };
        collect(conn, q, &before, use_rollups, &mut |_, key, sums| {
            previous.entry(key).or_default().add(sums);
            Ok(())
        })?;
    }
    let row = |(provider, model, speed): &Key, s: &Sums| {
        json!({"provider":provider,"model":model,"speed":speed_name(*speed),"calls":s.calls,"timed":s.timed,
            "output":s.output,"stream_ms":s.stream_ms,"ttft_ms":s.ttft_ms})
    };
    let mut trend: Vec<Value> = Vec::with_capacity(days.len());
    for ((day, key), sums) in &days {
        let mut value = row(key, sums);
        value["date"] = json!(day.to_string());
        trend.push(value);
    }
    Ok(json!({
        "models":groups.iter().map(|(k, s)| row(k, s)).collect::<Vec<_>>(),
        "trend":trend,
        "previous":previous.iter().map(|(k, s)| row(k, s)).collect::<Vec<_>>(),
        "backfilling":backfilling(conn)?,
        "min_stream_ms":MIN_STREAM_MS,
        "max_rate":MAX_RATE,
    }))
}

/// Calls `add` with each bucket's (or entry's) start, group and sums in `r`.
fn collect(
    conn: &Connection,
    q: &Query,
    r: &Range,
    use_rollups: bool,
    add: &mut dyn FnMut(i64, Key, &Sums) -> Result<()>,
) -> Result<()> {
    let entries = |r: &Range, add: &mut dyn FnMut(i64, Key, &Sums) -> Result<()>| -> Result<()> {
        let (sql, values) = filter(q, r, "e");
        // Columns after the time: bucket, dims, provider, model, account, client, speed, then the sums.
        let mut statement = conn.prepare_cached(&format!(
            "SELECT e.event_at_ms,{} FROM usage_entries e INDEXED BY usage_entries_event_time
             JOIN usage_timings t ON t.id=e.id WHERE {sql} AND e.source='proxy'",
            row("e", "t")
        ))?;
        let mut rows = statement.query(rusqlite::params_from_iter(values))?;
        while let Some(row) = rows.next()? {
            read::check()?;
            let timed: bool = row.get(8)?;
            let sums = Sums {
                calls: 1,
                timed: i64::from(timed),
                output: row.get(9)?,
                stream_ms: row.get(10)?,
                ttft_ms: row.get(11)?,
            };
            add(row.get(0)?, (row.get(3)?, row.get(4)?, row.get(7)?), &sums)?;
        }
        Ok(())
    };
    // The rollup has no entrypoint; a read filtered by one reads entries.
    let buckets = rollup::whole_buckets(r.start, r.end).filter(|_| use_rollups && q.entrypoint.is_none());
    let Some((first, last)) = buckets else {
        return entries(r, add);
    };
    // Whole buckets first, checked to stay inside one local day, then the edges.
    let mut bucketed: Vec<(i64, Key, Sums)> = Vec::new();
    {
        let (sql, values) = rollup::filter(q, (first, last), false, false);
        let mut statement = conn.prepare_cached(&format!(
            "SELECT bucket,provider,model,speed,SUM(calls),SUM(timed),SUM(output),SUM(stream_ms),SUM(ttft_ms)
             FROM usage_speed_rollup WHERE {sql} GROUP BY bucket,provider,model,speed"
        ))?;
        let mut rows = statement.query(rusqlite::params_from_iter(values))?;
        while let Some(row) = rows.next()? {
            read::check()?;
            let sums = Sums {
                calls: row.get(4)?,
                timed: row.get(5)?,
                output: row.get(6)?,
                stream_ms: row.get(7)?,
                ttft_ms: row.get(8)?,
            };
            bucketed.push((row.get::<_, i64>(0)? * rollup::BUCKET_MS, (row.get(1)?, row.get(2)?, row.get(3)?), sums));
        }
    }
    let mut dates = LocalDates::new(r.tz.clone());
    for (start, _, _) in &bucketed {
        let day = dates.date(*start).context("usage timestamp out of bounds")?;
        if dates.date(start + rollup::BUCKET_MS - 1).context("usage timestamp out of bounds")? != day {
            // A bucket that straddles a local midnight: read the entries instead.
            return entries(r, add);
        }
    }
    for (start, key, sums) in bucketed {
        add(start, key, &sums)?;
    }
    for (low, high) in [(r.start, first * rollup::BUCKET_MS), (last * rollup::BUCKET_MS, r.end)] {
        if low < high {
            entries(&Range { start: low, end: high, tz: r.tz.clone() }, add)?;
        }
    }
    Ok(())
}
