use super::*;
use crate::usage::types::Tokens;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("fusebox-rollback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn database(path: &Path) -> Connection {
    let mut c = Connection::open(path).unwrap();
    migrate(&mut c).unwrap();
    let catalogue = Catalogue::provider_fixture();
    c.execute("INSERT INTO usage_meta VALUES('catalogue',?1)", [serde_json::to_string(&catalogue).unwrap()]).unwrap();
    reprice(&mut c).unwrap();
    let mut known = Observation::new("proxy", "known".into(), "openai", Utc::now().timestamp_millis());
    known.actual_model = Some("gpt-6.1-sol".into());
    known.inference_geo = Some("global".into());
    known.completeness = "complete".into();
    known.tokens = Tokens {
        input: Some(700),
        cache_read: Some(100),
        cache_write: Some(200),
        write_30m: Some(80),
        write_12h: Some(120),
        output: Some(100),
        ..Default::default()
    };
    let mut zero = known.clone();
    zero.source_event_id = "zero".into();
    zero.tokens.cache_write = Some(0);
    zero.tokens.write_30m = Some(0);
    zero.tokens.write_12h = Some(0);
    let mut unknown = known.clone();
    unknown.source_event_id = "unknown".into();
    unknown.tokens.write_30m = None;
    unknown.tokens.write_12h = None;
    insert_batch(&mut c, &[known, zero, unknown]).unwrap();
    c
}

type Document = (i64, String, String, String, Option<i64>);
fn documents(c: &Connection) -> Vec<Document> {
    c.prepare("SELECT id,payload,snapshot_json,fingerprint,cost_nanos FROM usage_observations ORDER BY id")
        .unwrap()
        .query_map([], |r| {
            let payload: SqlValue = r.get(1)?;
            let snapshot: SqlValue = r.get(2)?;
            Ok((
                r.get(0)?,
                codec::decode(c, (&payload).into()).unwrap(),
                codec::decode(c, (&snapshot).into()).unwrap(),
                r.get(3)?,
                r.get(4)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn rollback_copy_preserves_the_source_costs_and_audit_and_converts_compressed_documents() {
    let fixture = Fixture::new();
    let source = fixture.path("source.sqlite3");
    let to = fixture.path("rollback.sqlite3");
    let c = database(&source);
    c.execute("INSERT INTO usage_price_revisions(observation_id,changed_at_ms,reason,previous_cost_nanos,previous_catalogue_version,previous_snapshot,catalogue_version,cost_nanos)
        SELECT id,0,'test',cost_nanos,catalogue_version,snapshot_json,catalogue_version,cost_nanos FROM usage_observations WHERE source_event_id='known'", []).unwrap();
    // A valid raw-content zstd dictionary exercises the same stored BLOB format
    // without requiring a thousand unrelated observations to train a fixture.
    c.execute(
        "INSERT INTO usage_dictionaries VALUES(1,0,3,3,?1)",
        [b"usage source provider tokens input cache_write output actual_model pricing catalogue cost_nanos".as_slice()],
    )
    .unwrap();
    c.execute("INSERT INTO usage_meta VALUES('active_dictionary','1')", []).unwrap();
    let before = documents(&c);
    let mut encoded = Documents::new(&c).unwrap();
    for (id, payload, snapshot, _, _) in &before {
        c.execute(
            "UPDATE usage_observations SET payload=?2,snapshot_json=?3 WHERE id=?1",
            params![id, encoded.store(payload).unwrap(), encoded.store(snapshot).unwrap()],
        )
        .unwrap();
    }
    assert_eq!(documents(&c), before);
    drop(c);

    let result = rollback_copy(&source, &to).unwrap();
    assert_eq!(result["schema"], LEGACY_VERSION);
    assert_eq!(result["omitted_lifetime_rows"], 2, "explicit zeroes are evidence too");
    let original = Connection::open(&source).unwrap();
    assert_eq!(original.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0)).unwrap(), VERSION);
    assert_eq!(documents(&original), before);
    let copy = Connection::open(&to).unwrap();
    assert_eq!(copy.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0)).unwrap(), LEGACY_VERSION);
    let rows = documents(&copy);
    for (old, new) in before.iter().zip(&rows) {
        assert_eq!(
            (new.0, &new.3, new.4),
            (old.0, &old.3, old.4),
            "ids, fingerprints and established costs stay fixed"
        );
        let payload: Value = serde_json::from_str(&new.1).unwrap();
        assert!(payload["tokens"].get("write_30m").is_none());
        assert!(payload["tokens"].get("write_12h").is_none());
        let snapshot: Value = serde_json::from_str(&new.2).unwrap();
        for field in LIFETIME_RATE_FIELDS {
            assert!(snapshot["rate"].get(field).is_none());
        }
    }
    let revision: String =
        copy.query_row("SELECT previous_snapshot FROM usage_price_revisions", [], |r| r.get(0)).unwrap();
    assert!(!revision.contains("cache_write_30m_nanos_per_token"));
    let original_revision: String =
        original.query_row("SELECT previous_snapshot FROM usage_price_revisions", [], |r| r.get(0)).unwrap();
    assert!(original_revision.contains("cache_write_30m_nanos_per_token"));
    let catalogue: String =
        copy.query_row("SELECT value FROM usage_meta WHERE key='catalogue'", [], |r| r.get(0)).unwrap();
    assert!(!catalogue.contains("cache_write_30m_nanos_per_token"));
    assert_eq!(copy.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)).unwrap(), "ok");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&to).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn rollback_copy_removes_the_new_calculation_from_only_the_active_catalogue() {
    let fixture = Fixture::new();
    let source = fixture.path("source.sqlite3");
    let to = fixture.path("rollback.sqlite3");
    let c = database(&source);
    let catalogue = Catalogue::load(None).unwrap();
    c.execute("UPDATE usage_meta SET value=?1 WHERE key='catalogue'", [serde_json::to_string(&catalogue).unwrap()])
        .unwrap();
    drop(c);
    rollback_copy(&source, &to).unwrap();
    for (path, expected) in [(&source, true), (&to, false)] {
        let c = Connection::open(path).unwrap();
        let raw: String = c.query_row("SELECT value FROM usage_meta WHERE key='catalogue'", [], |r| r.get(0)).unwrap();
        let catalogue: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(catalogue.get("calculation").is_some(), expected);
    }
}

#[test]
fn rollback_copy_refuses_active_sources_existing_targets_and_orphaned_companions() {
    let fixture = Fixture::new();
    let source = fixture.path("source.sqlite3");
    let to = fixture.path("rollback.sqlite3");
    drop(database(&source));
    let running = compact::lock(&source, false).unwrap();
    assert!(rollback_copy(&source, &to).unwrap_err().to_string().contains("stop Fusebox first"));
    assert!(!to.exists());
    drop(running);
    std::fs::write(&to, b"keep this file").unwrap();
    assert!(rollback_copy(&source, &to).is_err());
    assert_eq!(std::fs::read(&to).unwrap(), b"keep this file");
    let another = fixture.path("another.sqlite3");
    let wal = fixture.path("another.sqlite3-wal");
    std::fs::write(&wal, b"keep this companion").unwrap();
    assert!(rollback_copy(&source, &another).is_err());
    assert_eq!(std::fs::read(&wal).unwrap(), b"keep this companion");
    assert!(!another.exists());
}

#[test]
fn a_failed_conversion_removes_only_the_new_copy() {
    let fixture = Fixture::new();
    let source = fixture.path("source.sqlite3");
    let to = fixture.path("rollback.sqlite3");
    let c = database(&source);
    c.execute("UPDATE usage_observations SET payload='invalid json' WHERE source_event_id='known'", []).unwrap();
    drop(c);
    assert!(rollback_copy(&source, &to).is_err());
    assert!(!to.exists());
    assert!(!fixture.path("rollback.sqlite3.lock").exists());
    let c = Connection::open(&source).unwrap();
    assert_eq!(
        c.query_row("SELECT payload FROM usage_observations WHERE source_event_id='known'", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "invalid json"
    );
    assert_eq!(c.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0)).unwrap(), VERSION);
}

#[test]
fn rollback_copy_also_handles_a_database_without_price_revisions() {
    let fixture = Fixture::new();
    let source = fixture.path("source.sqlite3");
    let to = fixture.path("rollback.sqlite3");
    let c = database(&source);
    c.execute_batch("DROP TABLE usage_price_revisions;").unwrap();
    drop(c);
    rollback_copy(&source, &to).unwrap();
}
