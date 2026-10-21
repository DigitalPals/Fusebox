//! Fifteen-minute rollups that let the Usage page read whole buckets instead of
//! every entry. Every current UTC offset is a multiple of fifteen minutes, so a
//! bucket never straddles a local midnight and daily trends stay exact in any
//! timezone; `summary` still checks each bucket and falls back to a full scan
//! when one would.
//!
//! `usage_rollup` sums proxy entries (the accounting winners) by provider, model,
//! account and client. Imported entries are left out: whether one counts depends on
//! proxy evidence anywhere in the journal, so the summary scans them as before.
//! `usage_facets` lists the dimension values of every stored observation, which is
//! what the filter menus offer. Triggers keep both exact on every insert, delete
//! and update. A sum past the 64-bit range turns REAL in SQLite; readers treat
//! that as "recompute from entries" rather than trusting it.
use super::*;
use std::collections::BTreeMap;

pub(super) const BUCKET_MS: i64 = 900_000;
/// Entry columns in the order of the dashboard's metrics.
pub(super) const AMOUNTS: [&str; 10] = [
    "cost_nanos",
    "input",
    "cache_read",
    "cache_write",
    "write_5m",
    "write_1h",
    "output",
    "reasoning",
    "write_30m",
    "write_12h",
];

fn client(alias: &str) -> String {
    format!("COALESCE({alias}.client_id,CASE WHEN {alias}.origin_id LIKE 'collector:%' THEN {alias}.origin_id END)")
}

fn entry_dims(alias: &str) -> String {
    format!("json_array({alias}.provider,{alias}.model,{alias}.account_id,{})", client(alias))
}

fn facet_dims(alias: &str) -> String {
    format!("json_array({alias}.provider,{alias}.model,{alias}.account_id,{},{alias}.source)", client(alias))
}

fn known_columns() -> String {
    (0..AMOUNTS.len()).map(|i| format!("k{i}")).collect::<Vec<_>>().join(",")
}

fn sum_columns() -> String {
    (0..AMOUNTS.len()).map(|i| format!("s{i}")).collect::<Vec<_>>().join(",")
}

fn add_entry(alias: &str) -> String {
    let known: Vec<_> = AMOUNTS.iter().map(|c| format!("{alias}.{c} IS NOT NULL")).collect();
    let sums: Vec<_> = AMOUNTS.iter().map(|c| format!("COALESCE({alias}.{c},0)")).collect();
    let updates: Vec<_> = ["observations", "partial", "missing", "pricing_partial"]
        .into_iter()
        .map(|c| format!("{c}={c}+excluded.{c}"))
        .chain(
            (0..AMOUNTS.len()).flat_map(|i| [format!("k{i}=k{i}+excluded.k{i}"), format!("s{i}=s{i}+excluded.s{i}")]),
        )
        .collect();
    format!(
        "INSERT INTO usage_rollup(bucket,dims,provider,model,account_id,client,observations,partial,missing,pricing_partial,{},{})
         VALUES({alias}.event_at_ms/{BUCKET_MS},{},{alias}.provider,{alias}.model,{alias}.account_id,{},1,
           {alias}.completeness='partial',{alias}.completeness='missing',COALESCE({alias}.pricing_partial,0)=1,{},{})
         ON CONFLICT(bucket,dims) DO UPDATE SET {};",
        known_columns(),
        sum_columns(),
        entry_dims(alias),
        client(alias),
        known.join(","),
        sums.join(","),
        updates.join(",")
    )
}

fn remove_entry(alias: &str) -> String {
    let changes: Vec<_> = [
        "observations=observations-1".to_string(),
        format!("partial=partial-({alias}.completeness='partial')"),
        format!("missing=missing-({alias}.completeness='missing')"),
        format!("pricing_partial=pricing_partial-(COALESCE({alias}.pricing_partial,0)=1)"),
    ]
    .into_iter()
    .chain(AMOUNTS.iter().enumerate().flat_map(|(i, c)| {
        [format!("k{i}=k{i}-({alias}.{c} IS NOT NULL)"), format!("s{i}=s{i}-COALESCE({alias}.{c},0)")]
    }))
    .collect();
    let key = format!("bucket={alias}.event_at_ms/{BUCKET_MS} AND dims={}", entry_dims(alias));
    format!(
        "UPDATE usage_rollup SET {} WHERE {key};
         DELETE FROM usage_rollup WHERE {key} AND observations<=0;",
        changes.join(",")
    )
}

fn add_facet(alias: &str) -> String {
    format!(
        "INSERT INTO usage_facets(bucket,dims,provider,model,account_id,client,source,observations)
         VALUES({alias}.event_at_ms/{BUCKET_MS},{},{alias}.provider,{alias}.model,{alias}.account_id,{},{alias}.source,1)
         ON CONFLICT(bucket,dims) DO UPDATE SET observations=observations+1;",
        facet_dims(alias),
        client(alias)
    )
}

fn remove_facet(alias: &str) -> String {
    let key = format!("bucket={alias}.event_at_ms/{BUCKET_MS} AND dims={}", facet_dims(alias));
    format!(
        "UPDATE usage_facets SET observations=observations-1 WHERE {key};
         DELETE FROM usage_facets WHERE {key} AND observations<=0;"
    )
}

pub(super) const TRIGGERS: [&str; 7] = [
    "usage_rollup_insert",
    "usage_rollup_delete",
    "usage_rollup_update_old",
    "usage_rollup_update_new",
    "usage_facets_insert",
    "usage_facets_delete",
    "usage_facets_update",
];

/// Creates both rollups and their triggers, then fills them from the existing journal.
pub(super) fn create(tx: &Connection) -> Result<()> {
    let numbers: Vec<_> =
        (0..AMOUNTS.len()).flat_map(|i| [format!("k{i} INTEGER NOT NULL"), format!("s{i} INTEGER NOT NULL")]).collect();
    tx.execute_batch(&format!(
        "CREATE TABLE usage_rollup(bucket INTEGER NOT NULL,dims TEXT NOT NULL,provider TEXT,model TEXT,account_id TEXT,client TEXT,
           observations INTEGER NOT NULL,partial INTEGER NOT NULL,missing INTEGER NOT NULL,pricing_partial INTEGER NOT NULL,{},
           PRIMARY KEY(bucket,dims)) WITHOUT ROWID;
         CREATE TABLE usage_facets(bucket INTEGER NOT NULL,dims TEXT NOT NULL,provider TEXT,model TEXT,account_id TEXT,client TEXT,
           source TEXT,observations INTEGER NOT NULL,PRIMARY KEY(bucket,dims)) WITHOUT ROWID;
         CREATE TRIGGER usage_rollup_insert AFTER INSERT ON usage_entries WHEN NEW.source='proxy' BEGIN {} END;
         CREATE TRIGGER usage_rollup_delete AFTER DELETE ON usage_entries WHEN OLD.source='proxy' BEGIN {} END;
         CREATE TRIGGER usage_rollup_update_old AFTER UPDATE ON usage_entries WHEN OLD.source='proxy' BEGIN {} END;
         CREATE TRIGGER usage_rollup_update_new AFTER UPDATE ON usage_entries WHEN NEW.source='proxy' BEGIN {} END;
         CREATE TRIGGER usage_facets_insert AFTER INSERT ON usage_observations BEGIN {} END;
         CREATE TRIGGER usage_facets_delete AFTER DELETE ON usage_observations BEGIN {} END;
         CREATE TRIGGER usage_facets_update AFTER UPDATE OF event_at_ms,provider,model,account_id,client_id,origin_id,source
           ON usage_observations BEGIN {} {} END;",
        numbers.join(","),
        add_entry("NEW"),
        remove_entry("OLD"),
        remove_entry("OLD"),
        add_entry("NEW"),
        add_facet("NEW"),
        remove_facet("OLD"),
        remove_facet("OLD"),
        add_facet("NEW"),
    ))?;
    backfill(tx)
}

#[derive(Default)]
struct Sums {
    provider: Option<String>,
    model: Option<String>,
    account: Option<String>,
    client: Option<String>,
    counts: [i64; 4],
    known: [i64; 10],
    sums: [i128; 10],
}

pub(super) fn backfill(tx: &Connection) -> Result<()> {
    // Rust sums in i128 so a backfill never fails on overflow; an out-of-range
    // sum is stored REAL, exactly what the triggers would have produced.
    let mut groups: BTreeMap<(i64, String), Sums> = BTreeMap::new();
    {
        let mut statement = tx.prepare(&format!(
            "SELECT o.event_at_ms/{BUCKET_MS},{},o.provider,o.model,o.account_id,{},o.completeness='partial',o.completeness='missing',
               COALESCE(o.pricing_partial,0)=1,{} FROM usage_entries o WHERE o.source='proxy'",
            entry_dims("o"),
            client("o"),
            AMOUNTS.map(|c| format!("o.{c}")).join(",")
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let group = groups.entry((row.get(0)?, row.get(1)?)).or_insert_with(|| Sums {
                provider: row.get::<_, Option<String>>(2).ok().flatten(),
                model: row.get::<_, Option<String>>(3).ok().flatten(),
                account: row.get::<_, Option<String>>(4).ok().flatten(),
                client: row.get::<_, Option<String>>(5).ok().flatten(),
                ..Default::default()
            });
            group.counts[0] += 1;
            for i in 0..3 {
                group.counts[i + 1] += i64::from(row.get::<_, bool>(6 + i)?);
            }
            for i in 0..AMOUNTS.len() {
                if let Some(amount) = row.get::<_, Option<i64>>(9 + i)? {
                    group.known[i] += 1;
                    group.sums[i] += i128::from(amount);
                }
            }
        }
    }
    let mut insert = tx.prepare(&format!(
        "INSERT INTO usage_rollup(bucket,dims,provider,model,account_id,client,observations,partial,missing,pricing_partial,{},{})
         VALUES({})",
        known_columns(),
        sum_columns(),
        (1..=10 + 2 * AMOUNTS.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",")
    ))?;
    for ((bucket, dims), g) in groups {
        let mut values: Vec<SqlValue> =
            vec![bucket.into(), dims.into(), g.provider.into(), g.model.into(), g.account.into(), g.client.into()];
        values.extend(g.counts.iter().map(|n| SqlValue::from(*n)));
        values.extend(g.known.iter().map(|n| SqlValue::from(*n)));
        values.extend(g.sums.iter().map(|n| match i64::try_from(*n) {
            Ok(n) => SqlValue::Integer(n),
            Err(_) => SqlValue::Real(*n as f64),
        }));
        insert.execute(rusqlite::params_from_iter(values))?;
    }
    tx.execute_batch(&format!(
        "INSERT INTO usage_facets(bucket,dims,provider,model,account_id,client,source,observations)
         SELECT o.event_at_ms/{BUCKET_MS},{},o.provider,o.model,o.account_id,{},o.source,COUNT(*) FROM usage_observations o GROUP BY 1,2",
        facet_dims("o"),
        client("o")
    ))?;
    Ok(())
}

/// The part of a query range made of whole buckets, as bucket numbers [first, end).
pub(super) fn whole_buckets(start: i64, end: i64) -> Option<(i64, i64)> {
    let first = start.div_euclid(BUCKET_MS) + i64::from(start.rem_euclid(BUCKET_MS) != 0);
    let last = end.div_euclid(BUCKET_MS);
    (first < last).then_some((first, last))
}

/// Filters for a rollup table, matching `filter` on entries and observations.
pub(super) fn filter(q: &Query, buckets: (i64, i64), with_source: bool) -> (String, Vec<SqlValue>) {
    let mut sql = "bucket>=? AND bucket<?".to_string();
    let mut values: Vec<SqlValue> = vec![buckets.0.into(), buckets.1.into()];
    let mut columns =
        vec![("provider", &q.provider), ("model", &q.model), ("account_id", &q.account), ("client", &q.client)];
    if with_source {
        columns.push(("source", &q.source));
    }
    for (column, value) in columns {
        if let Some(value) = value {
            sql.push_str(&format!(" AND {column}=?"));
            values.push(value.clone().into());
        }
    }
    (sql, values)
}
