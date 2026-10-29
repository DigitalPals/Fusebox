use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

#[test]
fn unchanged_health_checkpoints_do_not_write_and_failed_updates_are_retried() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(WRITER_SESSIONS_SCHEMA).unwrap();
    conn.execute("INSERT INTO usage_writer_sessions(id,started_at_ms) VALUES('test',0)", []).unwrap();
    let health = Health::default();
    let mut checkpoint = HealthCheckpoint::default();
    checkpoint.persist(&conn, "test", &health).unwrap();
    let changes = conn.total_changes();
    for _ in 0..120 {
        checkpoint.persist(&conn, "test", &health).unwrap();
    }
    assert_eq!(conn.total_changes(), changes, "idle checkpoints must not write");

    // Producer-side drops and rejections must still reach disk without a batch.
    health.dropped.store(2, Ordering::Relaxed);
    health.rejected.store(3, Ordering::Relaxed);
    health.errors.store(4, Ordering::Relaxed);
    health.written.store(5, Ordering::Relaxed);
    health.last_commit.store(6, Ordering::Relaxed);
    conn.execute_batch("PRAGMA query_only=ON;").unwrap();
    assert!(checkpoint.persist(&conn, "test", &health).is_err());
    conn.execute_batch("PRAGMA query_only=OFF;").unwrap();
    checkpoint.persist(&conn, "test", &health).unwrap();
    let saved: (u64, u64, u64, u64, i64) = conn
        .query_row(
            "SELECT dropped,rejected,writer_errors,written,last_commit_at_ms FROM usage_writer_sessions WHERE id='test'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(saved, (2, 3, 4, 5, 6));
    assert_eq!(conn.total_changes(), changes + 1, "one dirty checkpoint writes once");
    checkpoint.persist(&conn, "test", &health).unwrap();
    assert_eq!(conn.total_changes(), changes + 1);
}

fn document_fixture() -> (Connection, Vec<SqlValue>, String) {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate(&mut conn).unwrap();
    let text = json!({"tokens":{"input":1000,"cache_read":100,"output":200},
        "provider":"openai","model":"gpt-6.1-sol","description":"usage metadata ".repeat(100)})
    .to_string();
    let mut values = Vec::new();
    for (id, bytes) in [(1, b"first usage metadata dictionary".as_slice()), (2, b"second token dictionary")] {
        conn.execute(
            "INSERT INTO usage_dictionaries(id,created_at_ms,trained_through_id,samples,dictionary) VALUES(?1,0,0,0,?2)",
            params![id, bytes],
        )
        .unwrap();
        conn.execute("INSERT OR REPLACE INTO usage_meta(key,value) VALUES('active_dictionary',?1)", [id.to_string()])
            .unwrap();
        values.push(Documents::new(&conn).unwrap().store(&text).unwrap());
    }
    (conn, values, text)
}

#[test]
fn batch_decoder_reuses_identity_and_handles_mixed_dictionaries_and_corruption() {
    let (conn, values, text) = document_fixture();
    let mut decoder = codec::Decoder::new(&conn);
    assert_eq!(decoder.decode((&values[0]).into()).unwrap(), text);
    // Once bound, even switching dictionaries must not query database identity.
    conn.authorizer(Some(|ctx: AuthContext<'_>| match ctx.action {
        AuthAction::Read { table_name: "usage_meta", .. } => Authorization::Deny,
        _ => Authorization::Allow,
    }));
    for value in [&values[0], &values[1], &values[0], &SqlValue::Text(text.clone())] {
        assert_eq!(decoder.decode(value.into()).unwrap(), text);
    }
    let SqlValue::Blob(blob) = &values[0] else { panic!("fixture must be compressed") };
    assert!(decoder.decode(rusqlite::types::ValueRef::Blob(&blob[..5])).is_err());
    assert!(decoder.decode(rusqlite::types::ValueRef::Blob(&blob[..blob.len() - 1])).is_err());
    assert!(decoder.decode(rusqlite::types::ValueRef::Text(&[0xff])).is_err());
    assert!(decoder.decode(rusqlite::types::ValueRef::Null).is_err());
    assert_eq!(decoder.decode((&values[0]).into()).unwrap(), text, "errors must not poison the context");
}

#[test]
#[ignore = "manual CPU benchmark; run with --ignored --nocapture"]
fn usage_document_decode_benchmark() {
    let (conn, values, text) = document_fixture();
    let document = &values[0];
    assert_eq!(codec::decode(&conn, document.into()).unwrap(), text);
    let count = 10_000;
    let started = std::time::Instant::now();
    for _ in 0..count {
        std::hint::black_box(codec::decode(&conn, document.into()).unwrap());
    }
    let separate = started.elapsed();
    let mut decoder = codec::Decoder::new(&conn);
    let started = std::time::Instant::now();
    for _ in 0..count {
        std::hint::black_box(decoder.decode(document.into()).unwrap());
    }
    let reused = started.elapsed();
    println!(
        "{}",
        json!({"documents":count,"document_bytes":text.len(),
            "separate_context_ms":separate.as_secs_f64()*1000.0,
            "reused_context_ms":reused.as_secs_f64()*1000.0})
    );
}
