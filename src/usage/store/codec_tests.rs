use super::*;

fn fixture() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    super::super::migrate(&mut conn).unwrap();
    conn.execute(
        "INSERT INTO usage_dictionaries(id,created_at_ms,trained_through_id,samples,dictionary) VALUES(1,0,0,0,?1)",
        [b"usage tokens provider model input output cache_read metadata".as_slice()],
    )
    .unwrap();
    conn.execute("INSERT INTO usage_meta(key,value) VALUES('active_dictionary','1')", []).unwrap();
    conn
}

fn reference(context: &mut zstd::zstd_safe::CCtx<'_>, dictionary: &Dictionary, text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + zstd::zstd_safe::compress_bound(text.len()));
    out.extend_from_slice(&[MAGIC, WITH_DICTIONARY]);
    out.extend_from_slice(&(dictionary.id as u32).to_le_bytes());
    let mut frame = Vec::with_capacity(zstd::zstd_safe::compress_bound(text.len()));
    context.compress_using_cdict(&mut frame, text.as_bytes(), dictionary.encoder.as_cdict()).unwrap();
    out.extend_from_slice(&frame);
    out
}

#[test]
fn reused_compression_scratch_preserves_frames_for_growing_and_shrinking_documents() {
    let conn = fixture();
    let mut documents = Documents::new(&conn).unwrap();
    let mut decoder = Decoder::new(&conn);
    let mut context = zstd::zstd_safe::CCtx::create();
    let dictionary = active(&conn).unwrap().unwrap();
    for size in [0, 1, 4096, 17, 100_000, 3, 65_536, 0] {
        let text = "usage tokens 日本語 ".repeat(size);
        let stored = documents.store(&text).unwrap();
        let rusqlite::types::Value::Blob(blob) = &stored else { panic!("expected compressed document") };
        assert_eq!(*blob, reference(&mut context, &dictionary, &text));
        assert_eq!(decoder.decode((&stored).into()).unwrap(), text);
        assert_eq!(blob.capacity(), blob.len(), "stored blob must not retain worst-case scratch capacity");
    }
    conn.execute("DELETE FROM usage_meta WHERE key='active_dictionary'", []).unwrap();
    assert_eq!(Documents::new(&conn).unwrap().store("plain").unwrap(), rusqlite::types::Value::Text("plain".into()));
}

#[test]
#[ignore = "manual CPU benchmark; run with --ignored --nocapture"]
fn usage_document_encode_benchmark() {
    let conn = fixture();
    let dictionary = active(&conn).unwrap().unwrap();
    let text = serde_json::json!({"provider":"openai","tokens":{"input":1000,"output":200},
        "metadata":"usage metadata ".repeat(100)})
    .to_string();
    let count = 10_000;
    let mut context = zstd::zstd_safe::CCtx::create();
    let started = std::time::Instant::now();
    let mut old_capacity = 0;
    for _ in 0..count {
        let blob = reference(&mut context, &dictionary, &text);
        old_capacity += blob.capacity();
        std::hint::black_box(blob);
    }
    let before = started.elapsed();
    let mut documents = Documents::new(&conn).unwrap();
    let started = std::time::Instant::now();
    let mut new_capacity = 0;
    for _ in 0..count {
        let blob = documents.store(&text).unwrap();
        let rusqlite::types::Value::Blob(bytes) = &blob else { panic!("expected compressed document") };
        new_capacity += bytes.capacity();
        std::hint::black_box(blob);
    }
    let after = started.elapsed();
    println!(
        "{}",
        serde_json::json!({"documents":count,"document_bytes":text.len(),
            "separate_scratch_ms":before.as_secs_f64()*1000.0,
            "reused_scratch_ms":after.as_secs_f64()*1000.0,
            "stored_blob_capacity_before":old_capacity,"stored_blob_capacity_after":new_capacity,
            "reused_scratch_capacity":documents.frame.capacity()})
    );
}
