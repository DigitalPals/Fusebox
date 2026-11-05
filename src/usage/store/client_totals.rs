//! Budget totals over raw proxy observations, including every provider attempt.
//! Unlike the dashboard's entry rollup, these must not choose canonical winners.
//! UTC hour buckets allow exact local periods with at most two short raw edges.
//!
//! Policy only ever asks about the current month, so the table starts a day before
//! the month it was built in (recorded in `usage_meta`) rather than at the oldest
//! retained row. Building it then reads one month through the time index instead
//! of the whole table, which took 6 s on production. The triggers leave out rows
//! before that start too.
use super::*;

pub(crate) const BUCKET_MS: i64 = 3_600_000;
const TABLE: &str = "usage_client_totals";
const TRIGGERS: [&str; 3] = ["usage_client_totals_insert", "usage_client_totals_delete", "usage_client_totals_update"];
const COLUMNS: [&str; 4] = ["tokens", "cost_nanos", "unknown_tokens", "unpriced"];
const CHANGED: &str = "source,client_id,event_at_ms,input,cache_read,cache_write,output,cost_nanos,payload";
/// Bump when the table or trigger definitions change, so a database built with
/// earlier definitions is rebuilt rather than kept.
const DEFINITION: &str = "1";
const DEFINITION_KEY: &str = "client_totals_definition";
const FROM_KEY: &str = "client_totals_from";

/// The first bucket a new table holds: midnight UTC a day before the month starts,
/// which is before that month's local start in every time zone.
fn first_bucket(now: DateTime<Utc>) -> i64 {
    use chrono::Datelike;
    let first = now.date_naive().with_day(1).unwrap_or(now.date_naive());
    (first.and_time(chrono::NaiveTime::MIN).and_utc() - chrono::Duration::days(1)).timestamp_millis()
}

/// Where the table starts. Budget reads refuse a month that begins before it.
pub(crate) fn start(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT CAST(value AS INTEGER) FROM usage_meta WHERE key=?1", [FROM_KEY], |r| r.get(0))
        .optional()?
        .context("client totals are not installed")
}

fn included(alias: &str) -> String {
    format!(
        "{alias}.source='proxy' AND {alias}.client_id IS NOT NULL
         AND {alias}.event_at_ms>=(SELECT CAST(value AS INTEGER) FROM usage_meta WHERE key='{FROM_KEY}')"
    )
}

fn amounts(alias: &str) -> [String; 4] {
    let tokens = format!("COALESCE({alias}.input,0)+COALESCE({alias}.cache_write,0)+COALESCE({alias}.output,0)");
    let chargeable = format!(
        "({tokens}+COALESCE({alias}.cache_read,0)>0 OR json_extract(usage_document({alias}.payload),'$.status')<400)"
    );
    [
        tokens,
        format!("COALESCE({alias}.cost_nanos,0)"),
        format!(
            "CASE WHEN ({alias}.input IS NULL OR {alias}.cache_write IS NULL OR {alias}.output IS NULL) AND {chargeable} THEN 1 ELSE 0 END"
        ),
        format!("CASE WHEN {alias}.cost_nanos IS NULL AND {chargeable} THEN 1 ELSE 0 END"),
    ]
}

fn add(alias: &str) -> String {
    let updates = COLUMNS.map(|c| format!("{c}={c}+excluded.{c}")).join(",");
    format!(
        "INSERT INTO {TABLE}(bucket_ms,client_id,observations,{})
         SELECT {alias}.event_at_ms/{BUCKET_MS}*{BUCKET_MS},{alias}.client_id,1,{} WHERE {}
         ON CONFLICT(bucket_ms,client_id) DO UPDATE SET observations=observations+1,{updates};",
        COLUMNS.join(","),
        amounts(alias).join(","),
        included(alias)
    )
}

fn subtract(alias: &str) -> String {
    let updates =
        COLUMNS.into_iter().zip(amounts(alias)).map(|(c, v)| format!("{c}={c}-({v})")).collect::<Vec<_>>().join(",");
    let key = format!(
        "bucket_ms={alias}.event_at_ms/{BUCKET_MS}*{BUCKET_MS} AND client_id={alias}.client_id AND {}",
        included(alias)
    );
    format!(
        "UPDATE {TABLE} SET observations=observations-1,{updates} WHERE {key};
             DELETE FROM {TABLE} WHERE {key} AND observations=0;"
    )
}

/// Auxiliary schema, installed atomically with its backfill. Store connections
/// register usage_document; direct maintenance writers must register it too.
/// The supported downgrade removes this table and its triggers.
pub(super) fn ensure(conn: &mut Connection) -> Result<()> {
    let present: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM sqlite_master WHERE (type='table' AND name='{TABLE}')
            OR (type='trigger' AND name IN ({}))",
            TRIGGERS.map(|t| format!("'{t}'")).join(",")
        ),
        [],
        |r| r.get(0),
    )?;
    let definition: Option<String> =
        conn.query_row("SELECT value FROM usage_meta WHERE key=?1", [DEFINITION_KEY], |r| r.get(0)).optional()?;
    if present == 1 + TRIGGERS.len() as i64 && definition.as_deref() == Some(DEFINITION) {
        return Ok(());
    }
    // Tests read periods from any year, so their tables hold every row.
    let from = if cfg!(test) { i64::MIN / BUCKET_MS * BUCKET_MS } else { first_bucket(Utc::now()) };
    build(conn, from)
}

/// Replaces the table and its triggers with ones holding every row from `from_ms`.
pub(crate) fn build(conn: &mut Connection, from_ms: i64) -> Result<()> {
    ensure!(from_ms % BUCKET_MS == 0, "client totals must start on a whole hour");
    let tx = conn.transaction()?;
    remove(&tx)?;
    tx.execute("INSERT INTO usage_meta(key,value) VALUES(?1,?2)", params![FROM_KEY, from_ms.to_string()])?;
    tx.execute_batch(&format!(
        "CREATE TABLE {TABLE}(bucket_ms INTEGER NOT NULL,client_id TEXT NOT NULL,observations INTEGER NOT NULL,
           tokens INTEGER NOT NULL,cost_nanos INTEGER NOT NULL,unknown_tokens INTEGER NOT NULL,unpriced INTEGER NOT NULL,
           PRIMARY KEY(bucket_ms,client_id)) WITHOUT ROWID;
         CREATE TRIGGER {} AFTER INSERT ON usage_observations WHEN {} BEGIN {} END;
         CREATE TRIGGER {} AFTER DELETE ON usage_observations WHEN {} BEGIN {} END;
         CREATE TRIGGER {} AFTER UPDATE OF {CHANGED} ON usage_observations BEGIN {} {} END;",
        TRIGGERS[0],included("NEW"),add("NEW"),
        TRIGGERS[1],included("OLD"),subtract("OLD"),
        TRIGGERS[2],subtract("OLD"),add("NEW"),
    ))?;
    // Wide backfill sums preserve the journal even when a budget total overflows.
    // SQLite trigger arithmetic also promotes overflow to REAL; the i64 policy
    // reader then fails closed instead of trusting a rounded total.
    let mut totals: HashMap<(i64, String), [i128; 5]> = HashMap::new();
    {
        let mut stmt = tx.prepare(&format!(
            "SELECT o.event_at_ms/{BUCKET_MS}*{BUCKET_MS},o.client_id,{}
            FROM usage_observations o INDEXED BY usage_event_time WHERE o.event_at_ms>=?1 AND {}",
            amounts("o").join(","),
            included("o")
        ))?;
        let mut rows = stmt.query([from_ms])?;
        while let Some(r) = rows.next()? {
            let entry = totals.entry((r.get(0)?, r.get(1)?)).or_default();
            entry[0] += 1;
            for (i, value) in entry[1..].iter_mut().enumerate() {
                *value += i128::from(r.get::<_, i64>(i + 2)?);
            }
        }
    }
    let mut insert = tx.prepare(&format!("INSERT INTO {TABLE} VALUES(?1,?2,?3,?4,?5,?6,?7)"))?;
    for ((bucket, client), values) in totals {
        let mut params: Vec<SqlValue> = vec![bucket.into(), client.into()];
        params.extend(
            values.into_iter().map(|n| i64::try_from(n).map_or_else(|_| SqlValue::Real(n as f64), SqlValue::Integer)),
        );
        insert.execute(rusqlite::params_from_iter(params))?;
    }
    drop(insert);
    tx.execute("INSERT INTO usage_meta(key,value) VALUES(?1,?2)", [DEFINITION_KEY, DEFINITION])?;
    tx.commit()?;
    Ok(())
}

pub(super) fn remove(conn: &Connection) -> Result<()> {
    for trigger in TRIGGERS {
        conn.execute_batch(&format!("DROP TRIGGER IF EXISTS {trigger};"))?;
    }
    conn.execute_batch(&format!("DROP TABLE IF EXISTS {TABLE};"))?;
    conn.execute("DELETE FROM usage_meta WHERE key IN (?1,?2)", [DEFINITION_KEY, FROM_KEY])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_table_starts_a_day_before_the_utc_month() {
        for (now, from) in [
            ("2026-11-05T17:20:00Z", "2026-10-31T00:00:00Z"),
            ("2026-11-01T00:00:00Z", "2026-10-31T00:00:00Z"),
            ("2026-03-01T13:00:00Z", "2026-02-28T00:00:00Z"),
            ("2027-01-31T23:59:59Z", "2026-12-31T00:00:00Z"),
        ] {
            let now = DateTime::parse_from_rfc3339(now).unwrap().with_timezone(&Utc);
            let from = DateTime::parse_from_rfc3339(from).unwrap().timestamp_millis();
            assert_eq!(first_bucket(now), from);
            assert_eq!(from % BUCKET_MS, 0);
        }
        // Local months begin at most 14 hours before the UTC month, in Kiribati.
        let kiribati = DateTime::parse_from_rfc3339("2026-11-01T00:00:00+14:00").unwrap().timestamp_millis();
        assert!(first_bucket(DateTime::parse_from_rfc3339("2026-11-01T00:00:00Z").unwrap().to_utc()) <= kiribati);
    }
}
