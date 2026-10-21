//! One streaming pass for the metrics displayed by the Usage page. Request/attempt
//! identities and source evidence remain available in the ledger and raw records.
use super::*;
use std::collections::BTreeMap;

const METRICS: [&str; 10] = [
    "estimated_cost_nanos",
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
const DIMS: [&str; 4] = ["provider", "model", "account", "client"];

#[derive(Default)]
struct Metrics {
    observations: i64,
    sums: [Option<i128>; 10],
    overflow: [bool; 10],
    known: [i64; 10],
    partial: i64,
    missing: i64,
    pricing_partial: i64,
}
impl Metrics {
    fn add(&mut self, amounts: &[Option<i64>; 10], completeness: &str, pricing_partial: bool) {
        self.observations += 1;
        self.partial += i64::from(completeness == "partial");
        self.missing += i64::from(completeness == "missing");
        self.pricing_partial += i64::from(pricing_partial);
        for (i, amount) in amounts.iter().enumerate() {
            if let Some(amount) = amount {
                self.known[i] += 1;
                if !self.overflow[i] {
                    self.sums[i] = self.sums[i].unwrap_or(0).checked_add(i128::from(*amount));
                    self.overflow[i] = self.sums[i].is_none();
                }
            }
        }
    }
    /// Adds a rollup bucket: the same result as adding each of its entries.
    fn merge(&mut self, counts: [i64; 4], known: &[i64; 10], sums: &[i64; 10]) {
        self.observations += counts[0];
        self.partial += counts[1];
        self.missing += counts[2];
        self.pricing_partial += counts[3];
        for i in 0..METRICS.len() {
            if known[i] > 0 {
                self.known[i] += known[i];
                if !self.overflow[i] {
                    self.sums[i] = self.sums[i].unwrap_or(0).checked_add(i128::from(sums[i]));
                    self.overflow[i] = self.sums[i].is_none();
                }
            }
        }
    }
    fn value(&self) -> Value {
        let sums = self.sums.map(|n| n.and_then(|n| i64::try_from(n).ok()));
        let overflow: Vec<_> = METRICS
            .iter()
            .enumerate()
            .filter_map(|(i, name)| (self.known[i] > 0 && sums[i].is_none()).then_some(*name))
            .collect();
        let tokens: serde_json::Map<_, _> =
            METRICS.iter().enumerate().skip(1).map(|(i, name)| ((*name).into(), json!(sums[i]))).collect();
        let missing: serde_json::Map<_, _> = [1, 2, 3, 6, 7]
            .into_iter()
            .map(|i| (METRICS[i].into(), json!(self.observations - self.known[i])))
            .collect();
        json!({"observations":self.observations,"estimated_cost_nanos":sums[0],
            "known_cost_nanos":if self.known[0]>0 && sums[0].is_none(){None}else{Some(sums[0].unwrap_or(0))},
            "aggregation_overflow":!overflow.is_empty(),"aggregation_overflow_fields":overflow,
            "unpriced":self.observations-self.known[0],"partial":self.partial,"missing_usage":self.missing,
            "pricing_partial":self.pricing_partial,"tokens":tokens,"missing_token_counts":missing})
    }
}
#[derive(Default)]
struct Group {
    metrics: Metrics,
    accounts: BTreeSet<String>,
    provider: Option<String>,
}

// Repeated dimensions dominate large reads. Borrow their keys for lookups and
// allocate an owned key only when a group is first encountered.
fn update_group<T: Default>(groups: &mut BTreeMap<Option<String>, T>, key: &Option<String>, add: impl FnOnce(&mut T)) {
    if let Some(group) = groups.get_mut(key) {
        add(group);
    } else {
        let mut group = T::default();
        add(&mut group);
        groups.insert(key.clone(), group);
    }
}

/// Everything one summary accumulates, fed by rollup buckets and by entry rows alike.
struct Summary {
    tz: Tz,
    totals: Metrics,
    matched: i64,
    history_only: i64,
    weak_identity: i64,
    trends: [BTreeMap<NaiveDate, BTreeMap<Option<String>, Metrics>>; 2],
    breakdowns: [BTreeMap<Option<String>, Group>; 4],
    /// Per client, what each of its models used: the Usage page's top clients.
    client_models: BTreeMap<Option<String>, BTreeMap<Option<String>, Metrics>>,
}

impl Summary {
    fn new(tz: Tz) -> Self {
        Self {
            tz,
            totals: Metrics::default(),
            matched: 0,
            history_only: 0,
            weak_identity: 0,
            trends: Default::default(),
            breakdowns: Default::default(),
            client_models: BTreeMap::new(),
        }
    }

    fn add(&mut self, date: NaiveDate, dims: &[Option<String>; 4], add: impl Fn(&mut Metrics)) {
        add(&mut self.totals);
        for (i, trend) in self.trends.iter_mut().enumerate() {
            update_group(trend.entry(date).or_default(), &dims[i], &add);
        }
        for (i, breakdown) in self.breakdowns.iter_mut().enumerate() {
            update_group(breakdown, &dims[i], |group| {
                add(&mut group.metrics);
                if let Some(account) = &dims[2]
                    && !group.accounts.contains(account)
                {
                    group.accounts.insert(account.clone());
                }
                if group.provider.is_none() || dims[0] < group.provider {
                    group.provider.clone_from(&dims[0]);
                }
            });
        }
        update_group(&mut self.client_models, &dims[3], |models| update_group(models, &dims[1], &add));
    }

    /// One pass over entry rows, as the summary always worked.
    fn scan(&mut self, conn: &Connection, table: &str, where_sql: &str, values: &[SqlValue]) -> Result<()> {
        let client = dimension("client_id", "o");
        // Match against the entire journal, including proxy evidence outside this range/filter.
        // Only scalar accounting columns are read; no payload or pricing JSON is parsed.
        let mut statement = conn.prepare_cached(&format!(
            "SELECT CASE WHEN o.source='proxy' THEN 0 ELSE EXISTS(SELECT 1 FROM usage_entries p INDEXED BY usage_entries_association WHERE p.association_key=o.association_key AND p.source='proxy') END,
             o.source<>'proxy',COALESCE(o.response_id,'')='',o.event_at_ms,o.provider,o.model,o.account_id,{client},
             o.cost_nanos,o.input,o.cache_read,o.cache_write,o.write_5m,o.write_1h,o.output,o.reasoning,o.write_30m,o.write_12h,o.completeness,o.pricing_partial
             FROM {table} WHERE {where_sql}"
        ))?;
        let mut rows = statement.query(rusqlite::params_from_iter(values))?;
        while let Some(row) = rows.next()? {
            read::check()?;
            if row.get::<_, bool>(0)? {
                self.matched += 1;
                continue;
            }
            let history = row.get::<_, bool>(1)?;
            self.history_only += i64::from(history);
            self.weak_identity += i64::from(history && row.get::<_, bool>(2)?);
            let date = DateTime::from_timestamp_millis(row.get(3)?)
                .context("usage timestamp out of bounds")?
                .with_timezone(&self.tz)
                .date_naive();
            let dims: [Option<String>; 4] = [row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?];
            let mut amounts = [None; 10];
            for (i, amount) in amounts.iter_mut().enumerate() {
                *amount = row.get(8 + i)?;
            }
            let completeness = row.get_ref(18)?.as_str()?;
            let pricing_partial = row.get::<_, Option<i64>>(19)? == Some(1);
            self.add(date, &dims, |m| m.add(&amounts, completeness, pricing_partial));
        }
        Ok(())
    }

    /// Adds whole rollup buckets. False when a bucket can't be used exactly (a sum
    /// left the 64-bit range, or the bucket crosses a local midnight in this
    /// timezone); the caller then recomputes everything from entries.
    fn rollups(&mut self, conn: &Connection, where_sql: &str, values: &[SqlValue]) -> Result<bool> {
        let mut statement = conn.prepare_cached(&format!(
            "SELECT bucket,provider,model,account_id,client,observations,partial,missing,pricing_partial,
               k0,k1,k2,k3,k4,k5,k6,k7,k8,k9,s0,s1,s2,s3,s4,s5,s6,s7,s8,s9 FROM usage_rollup WHERE {where_sql}"
        ))?;
        let mut rows = statement.query(rusqlite::params_from_iter(values))?;
        while let Some(row) = rows.next()? {
            read::check()?;
            let bucket: i64 = row.get(0)?;
            let first = bucket * rollup::BUCKET_MS;
            let tz = self.tz;
            let local = |ms: i64| -> Result<NaiveDate> {
                Ok(DateTime::from_timestamp_millis(ms)
                    .context("usage timestamp out of bounds")?
                    .with_timezone(&tz)
                    .date_naive())
            };
            let date = local(first)?;
            if local(first + rollup::BUCKET_MS - 1)? != date {
                return Ok(false);
            }
            let dims: [Option<String>; 4] = [row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?];
            let counts: [i64; 4] = [row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?];
            let mut known = [0_i64; 10];
            let mut sums = [0_i64; 10];
            for i in 0..METRICS.len() {
                known[i] = row.get(9 + i)?;
                match row.get_ref(19 + i)? {
                    rusqlite::types::ValueRef::Integer(n) => sums[i] = n,
                    _ => return Ok(false),
                }
            }
            self.add(date, &dims, |m| m.merge(counts, &known, &sums));
        }
        Ok(true)
    }
}

pub(super) fn summary(conn: &Connection, q: &Query) -> Result<Value> {
    summary_with(conn, q, true)
}

/// The same summary computed only from entries, as before rollups existed.
#[cfg(test)]
pub(super) fn summary_scan(conn: &Connection, q: &Query) -> Result<Value> {
    summary_with(conn, q, false)
}

fn summary_with(conn: &Connection, q: &Query, use_rollups: bool) -> Result<Value> {
    let r = range(q)?;
    let (where_sql, values) = filter(q, &r, "o");
    let everything = "usage_entries o INDEXED BY usage_entries_event_time";
    let mut summary = Summary::new(r.tz);
    // Rollups hold proxy entries only; a filter for another source reads entries.
    let buckets = rollup::whole_buckets(r.start, r.end)
        .filter(|_| use_rollups && q.source.as_deref().is_none_or(|s| s == "proxy"));
    let complete = match buckets {
        Some(buckets) => {
            let (rollup_sql, rollup_values) = rollup::filter(q, buckets, false);
            summary.rollups(conn, &rollup_sql, &rollup_values)?
        }
        None => false,
    };
    if let (true, Some((first, last))) = (complete, buckets) {
        // Proxy entries in the partial buckets at either end of the range.
        for (low, high) in [(r.start, first * rollup::BUCKET_MS), (last * rollup::BUCKET_MS, r.end)] {
            if low < high {
                let edge = Range { start: low, end: high, tz: r.tz };
                let (edge_sql, edge_values) = filter(q, &edge, "o");
                summary.scan(conn, everything, &format!("{edge_sql} AND o.source='proxy'"), &edge_values)?;
            }
        }
        // Imported entries are never rolled up: whether one counts depends on proxy
        // evidence anywhere in the journal.
        if q.source.as_deref().is_none_or(|s| s != "proxy") {
            summary.scan(
                conn,
                "usage_entries o INDEXED BY usage_entries_imported",
                &format!("{where_sql} AND o.source<>'proxy'"),
                &values,
            )?;
        }
    } else {
        summary = Summary::new(r.tz);
        if q.source.as_deref().is_some_and(|s| s != "proxy") {
            let sql = format!("{where_sql} AND o.source<>'proxy'");
            summary.scan(conn, "usage_entries o INDEXED BY usage_entries_imported", &sql, &values)?;
        } else {
            summary.scan(conn, everything, &where_sql, &values)?;
        }
    }
    let Summary { totals, matched, history_only, weak_identity, trends, breakdowns, mut client_models, .. } = summary;
    let mut totals = totals.value();
    totals["matched"] = json!(matched);
    totals["history_only"] = json!(history_only);
    totals["weak_identity"] = json!(weak_identity);
    let mut trend_values: [Vec<Value>; 2] = Default::default();
    for (output, groups) in trend_values.iter_mut().zip(trends) {
        for (date, groups) in groups {
            for (group, metrics) in groups {
                read::check()?;
                let mut value = metrics.value();
                value["date"] = json!(date.to_string());
                value["group"] = json!(group);
                output.push(value);
            }
        }
    }
    let trends = trend_values;
    let mut out = serde_json::Map::new();
    for (name, groups) in DIMS.into_iter().zip(breakdowns) {
        let mut group_values = Vec::with_capacity(groups.len());
        for (id, group) in groups {
            read::check()?;
            let mut value = group.metrics.value();
            if name == "client" {
                value["models"] = client_model_values(client_models.remove(&id).unwrap_or_default());
            }
            value["id"] = json!(id);
            value["accounts"] = json!(group.accounts.len());
            value["provider"] = json!(group.provider);
            group_values.push(value);
        }
        let mut groups = group_values;
        read::check()?;
        groups.sort_by(|a, b| {
            b["estimated_cost_nanos"]
                .as_i64()
                .cmp(&a["estimated_cost_nanos"].as_i64())
                .then(b["observations"].as_i64().cmp(&a["observations"].as_i64()))
                .then(a["id"].as_str().cmp(&b["id"].as_str()))
        });
        read::check()?;
        groups.truncate(500);
        out.insert(name.into(), json!(groups));
    }
    let proxy_first: Option<i64> = conn
        .query_row("SELECT event_at_ms FROM usage_observations INDEXED BY usage_event_time WHERE source='proxy' ORDER BY event_at_ms LIMIT 1",[],|r|r.get(0))
        .optional()?;
    let raw: String = conn.query_row("SELECT value FROM usage_meta WHERE key='catalogue'", [], |row| row.get(0))?;
    let catalogue: Catalogue = serde_json::from_str(&raw)?;
    let stack = q.stack.as_deref().unwrap_or("provider");
    let facets = facets(conn, q, &r, use_rollups)?;
    read::check()?;
    Ok(json!({"range":{"start":r.start,"end":r.end,"timezone":r.tz.to_string()},
        "combined":{"basis":"One entry per provider response. Imported entries that share a response ID with a proxy entry are excluded.",
            "totals":totals,"proxy_first_event_at_ms":proxy_first,"stack":stack,
            "trend":trends[usize::from(stack=="model")],"trends":{"provider":trends[0],"model":trends[1]},"breakdowns":out},
        "facets":facets,
        "pricing":{"version":catalogue.version,"verified_at":catalogue.verified_at,
            "basis":"API list-price equivalent estimate; usage older than the catalogue is priced at today's rates as a backdated current-rate equivalent, not what was paid at the time"}}))
}

/// A client's models, largest list price first: the model, its calls and its cost.
fn client_model_values(models: BTreeMap<Option<String>, Metrics>) -> Value {
    // None when nothing was priced or the sum left the supported range.
    let cost = |m: &Metrics| m.sums[0];
    let mut models: Vec<(Option<String>, Metrics)> = models.into_iter().collect();
    models
        .sort_by(|a, b| cost(&b.1).cmp(&cost(&a.1)).then(b.1.observations.cmp(&a.1.observations)).then(a.0.cmp(&b.0)));
    models.truncate(50);
    models
        .into_iter()
        .map(|(id, m)| {
            json!({"id":id,"observations":m.observations,"unpriced":m.observations-m.known[0],
                "estimated_cost_nanos":cost(&m).and_then(|n| i64::try_from(n).ok())})
        })
        .collect()
}

fn collect_facets(conn: &Connection, sql: &str, values: &[SqlValue], facets: &mut [BTreeSet<String>; 5]) -> Result<()> {
    let mut statement = conn.prepare_cached(sql)?;
    let mut rows = statement.query(rusqlite::params_from_iter(values))?;
    while let Some(row) = rows.next()? {
        read::check()?;
        for (i, facet) in facets.iter_mut().enumerate() {
            if let Some(value) = row.get::<_, Option<String>>(i)? {
                facet.insert(value);
            }
        }
    }
    Ok(())
}

fn facets(conn: &Connection, q: &Query, r: &Range, use_rollups: bool) -> Result<Value> {
    let mut facets: [BTreeSet<String>; 5] = Default::default();
    // Distinct dimension tuples keep memory tied to facets, not record count, and
    // retain raw-only origins whose canonical evidence is selected elsewhere.
    let client = dimension("client_id", "o");
    let raw = |range: &Range| {
        let (sql, values) = filter(q, range, "o");
        (
            format!(
                "SELECT DISTINCT o.provider,o.model,o.account_id,{client},o.source FROM usage_observations o INDEXED BY usage_event_time WHERE {sql}"
            ),
            values,
        )
    };
    match rollup::whole_buckets(r.start, r.end).filter(|_| use_rollups) {
        Some((first, last)) => {
            let (sql, values) = rollup::filter(q, (first, last), true);
            collect_facets(
                conn,
                &format!("SELECT DISTINCT provider,model,account_id,client,source FROM usage_facets WHERE {sql}"),
                &values,
                &mut facets,
            )?;
            for (low, high) in [(r.start, first * rollup::BUCKET_MS), (last * rollup::BUCKET_MS, r.end)] {
                if low < high {
                    let (sql, values) = raw(&Range { start: low, end: high, tz: r.tz });
                    collect_facets(conn, &sql, &values, &mut facets)?;
                }
            }
        }
        None => {
            let (sql, values) = raw(r);
            collect_facets(conn, &sql, &values, &mut facets)?;
        }
    }
    let mut out = serde_json::Map::new();
    for (i, (name, values)) in
        ["providers", "models", "accounts", "clients", "sources"].into_iter().zip(facets).enumerate()
    {
        let mut items = Vec::new();
        for value in values.into_iter().take(500) {
            read::check()?;
            items.push(if matches!(i, 2 | 3) {
                let label = if i == 3 {
                    collector_label(conn, &value)?.unwrap_or_else(|| value.clone())
                } else {
                    value.clone()
                };
                json!({"id":value,"label":label})
            } else {
                json!(value)
            });
        }
        out.insert(name.into(), json!(items));
    }
    read::check()?;
    Ok(json!(out))
}
