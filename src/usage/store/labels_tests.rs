use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::time::Instant;

fn create_collectors(conn: &Connection, collectors: usize) {
    conn.execute_batch(
        "CREATE TABLE usage_collectors(id TEXT PRIMARY KEY,label TEXT NOT NULL,revoked INTEGER NOT NULL DEFAULT 0);",
    )
    .unwrap();
    for id in 0..collectors {
        conn.execute(
            "INSERT INTO usage_collectors(id,label,revoked) VALUES(?1,?2,?3)",
            params![id.to_string(), format!("Work laptop {id}"), id % 2],
        )
        .unwrap();
    }
}

fn fixture(collectors: usize) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    create_collectors(&conn, collectors);
    conn
}

fn reference_page(conn: &Connection, origins: &[String]) -> Vec<Option<String>> {
    // Exact prechange implementation retained behind cfg(test) in store.rs.
    origins.iter().map(|origin| collector_label(conn, origin).unwrap()).collect()
}

fn current_page(conn: &Connection, origins: &[String]) -> Vec<Option<String>> {
    let mut labels = CollectorLabels::default();
    origins.iter().map(|origin| labels.get(conn, origin).unwrap().map(String::from)).collect()
}

#[derive(Default)]
struct Reads {
    schema: AtomicU64,
    labels: AtomicU64,
}

// Both implementations prepare uncached SELECTs and immediately execute them.
// SQLite's authorizer therefore observes one matching column authorization per
// executed schema/label query. Instrument separately from timed runs.
fn count_reads(conn: &Connection, action: impl FnOnce()) -> Value {
    let reads = Arc::new(Reads::default());
    let measured = reads.clone();
    conn.authorizer(Some(move |context: AuthContext<'_>| {
        match context.action {
            AuthAction::Read { table_name: "sqlite_master" | "sqlite_schema", column_name: "name" } => {
                measured.schema.fetch_add(1, Ordering::Relaxed);
            }
            AuthAction::Read { table_name: "usage_collectors", column_name: "label" } => {
                measured.labels.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        Authorization::Allow
    }));
    action();
    conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    json!({"schema_queries":reads.schema.load(Ordering::Relaxed),"label_queries":reads.labels.load(Ordering::Relaxed)})
}

#[test]
fn collector_labels_current_output_matches_reference() {
    let conn = fixture(3);
    let origins =
        ["collector:0", "collector:1", "collector:2", "collector:missing", "local", "keeper:example"].map(String::from);
    assert_eq!(current_page(&conn, &origins), reference_page(&conn, &origins));
    let absent = Connection::open_in_memory().unwrap();
    assert_eq!(current_page(&absent, &origins), reference_page(&absent, &origins));
}

#[test]
fn collector_labels_cache_positive_negative_and_absent_table_reads() {
    let conn = fixture(2);
    let origins =
        ["collector:0", "collector:1", "collector:missing", "collector:missing", "collector:0"].map(String::from);
    let expected = reference_page(&conn, &origins);
    let reads = count_reads(&conn, || assert_eq!(current_page(&conn, &origins), expected));
    assert_eq!(reads, json!({"schema_queries":1,"label_queries":3}));

    // Cached hits and misses require no further SQL, including schema checks.
    let mut labels = CollectorLabels::default();
    assert_eq!(labels.get(&conn, "collector:0").unwrap(), Some("Work laptop 0"));
    assert_eq!(labels.get(&conn, "collector:missing").unwrap(), None);
    conn.authorizer(Some(|_: AuthContext<'_>| Authorization::Deny));
    assert_eq!(labels.get(&conn, "collector:0").unwrap(), Some("Work laptop 0"));
    assert_eq!(labels.get(&conn, "collector:missing").unwrap(), None);
    assert_eq!(labels.get(&conn, "local").unwrap(), None);
    assert!(labels.get(&conn, "collector:1").is_err(), "uncached SQL errors must propagate");
    conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(labels.get(&conn, "collector:1").unwrap(), Some("Work laptop 1"), "errors must not cache misses");

    let absent = Connection::open_in_memory().unwrap();
    assert_eq!(
        count_reads(&absent, || {
            current_page(&absent, &origins);
        }),
        json!({"schema_queries":1,"label_queries":0})
    );
    let ordinary = ["local".to_owned(), "keeper:example".to_owned()];
    assert_eq!(
        count_reads(&absent, || {
            current_page(&absent, &ordinary);
        }),
        json!({"schema_queries":0,"label_queries":0})
    );
}

fn journal_fixture(with_collectors: bool) -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate(&mut conn).unwrap();
    conn.execute(
        "INSERT INTO usage_meta(key,value) VALUES('catalogue',?1)",
        [serde_json::to_string(&Catalogue::provider_fixture()).unwrap()],
    )
    .unwrap();
    if with_collectors {
        create_collectors(&conn, 2);
    }
    let origins = ["collector:0", "collector:0", "collector:1", "collector:missing", "local", "keeper:example"];
    let observations: Vec<_> = origins
        .into_iter()
        .enumerate()
        .map(|(i, origin)| {
            let mut o = Observation::new("codex", format!("event-{i}"), "openai", 1_790_827_200_000 + i as i64);
            o.origin_id = origin.into();
            o
        })
        .collect();
    insert_batch(&mut conn, &observations).unwrap();
    conn
}

fn journal_query() -> Query {
    Query { start: Some("2026-10-01".into()), end: Some("2026-11-01".into()), ..Default::default() }
}

fn assert_page_labels(conn: &mut Connection, query: &Query, known: Option<&str>, revoked: Option<&str>) {
    let page = details(conn, query).unwrap();
    assert_eq!(page["total"], 6);
    for value in page["items"].as_array().unwrap() {
        let origin = value["origin_id"].as_str().unwrap();
        let expected = match origin {
            "collector:0" => known,
            "collector:1" => revoked,
            _ => None,
        };
        assert_eq!(value.get("collector_label"), expected.map(|label| json!(label)).as_ref(), "{origin}");
        if query.view.as_deref() == Some("combined") {
            let expected_origin = if origin == "local" { Some("This server") } else { expected };
            assert_eq!(value["origin_label"], json!(expected_origin), "{origin}");
        }
    }
    let dashboard = dashboard::summary(conn, query).unwrap();
    let clients = dashboard["facets"]["clients"].as_array().unwrap();
    assert_eq!(clients.len(), 3);
    for value in clients {
        let id = value["id"].as_str().unwrap();
        let expected = match id {
            "collector:0" => known.unwrap_or(id),
            "collector:1" => revoked.unwrap_or(id),
            _ => id,
        };
        assert_eq!(value["label"], expected, "{id}");
    }
}

#[test]
fn collector_labels_details_and_facets_refresh_between_reads() {
    let mut conn = journal_fixture(true);
    for view in ["raw", "combined"] {
        let query = Query { view: Some(view.into()), ..journal_query() };
        assert_page_labels(&mut conn, &query, Some("Work laptop 0"), Some("Work laptop 1"));
    }
    conn.execute("UPDATE usage_collectors SET label='Renamed laptop' WHERE id='0'", []).unwrap();
    conn.execute("INSERT INTO usage_collectors(id,label) VALUES('missing','Newly enrolled')", []).unwrap();
    for view in ["raw", "combined"] {
        let query = Query { view: Some(view.into()), ..journal_query() };
        let page = details(&mut conn, &query).unwrap();
        for value in page["items"].as_array().unwrap() {
            let expected = collector_label(&conn, value["origin_id"].as_str().unwrap()).unwrap();
            assert_eq!(value.get("collector_label"), expected.map(Value::String).as_ref());
        }
        let dashboard = dashboard::summary(&conn, &query).unwrap();
        assert_eq!(
            dashboard["facets"]["clients"],
            json!([
                {"id":"collector:0","label":"Renamed laptop"},
                {"id":"collector:1","label":"Work laptop 1"},
                {"id":"collector:missing","label":"Newly enrolled"}
            ])
        );
    }
}

#[test]
fn collector_labels_enrollment_after_absent_table_is_visible_on_next_read() {
    let mut conn = journal_fixture(false);
    for view in ["raw", "combined"] {
        let query = Query { view: Some(view.into()), ..journal_query() };
        assert_page_labels(&mut conn, &query, None, None);
    }
    create_collectors(&conn, 2);
    for view in ["raw", "combined"] {
        let query = Query { view: Some(view.into()), ..journal_query() };
        assert_page_labels(&mut conn, &query, Some("Work laptop 0"), Some("Work laptop 1"));
    }
}

#[test]
#[ignore = "manual CPU benchmark; run with --ignored --nocapture"]
fn usage_collector_labels_benchmark() {
    const ROWS: usize = 500;
    const PAGES: usize = 20;
    const WARMUPS: usize = 3;
    const ROUNDS: usize = 9;
    for distinct in [1, 10, 500] {
        let conn = fixture(distinct);
        let origins: Vec<_> = (0..ROWS).map(|i| format!("collector:{}", i % distinct)).collect();
        assert_eq!(current_page(&conn, &origins), reference_page(&conn, &origins));
        let measure = |lookup: fn(&Connection, &[String]) -> Vec<Option<String>>| {
            let started = Instant::now();
            for _ in 0..PAGES {
                std::hint::black_box(lookup(&conn, &origins));
            }
            started.elapsed().as_secs_f64() * 1000.0 / PAGES as f64
        };
        for _ in 0..WARMUPS {
            measure(reference_page);
            measure(current_page);
        }
        let (mut before, mut after) = (Vec::new(), Vec::new());
        for round in 0..ROUNDS {
            if round % 2 == 0 {
                before.push(measure(reference_page));
                after.push(measure(current_page));
            } else {
                after.push(measure(current_page));
                before.push(measure(reference_page));
            }
        }
        let before_reads = count_reads(&conn, || {
            std::hint::black_box(reference_page(&conn, &origins));
        });
        let after_reads = count_reads(&conn, || {
            std::hint::black_box(current_page(&conn, &origins));
        });
        println!(
            "{}",
            json!({"rows_per_page":ROWS,"distinct_origins":distinct,"pages_per_sample":PAGES,
            "warmups":WARMUPS,"alternating_runs":ROUNDS,"reference_ms_per_page":before,
            "current_ms_per_page":after,"reference_reads_per_page":before_reads,"current_reads_per_page":after_reads})
        );
    }
}
