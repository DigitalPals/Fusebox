//! OpenAI's 30-minute and 12-hour writes are disjoint subsets, never extra input.
use super::*;
use crate::usage::{capture::observe, types::Tokens};

fn event(id: &str) -> Observation {
    let mut o = Observation::new("proxy", id.into(), "openai", Utc::now().timestamp_millis());
    o.actual_model = Some("gpt-6.1-sol".into());
    o.inference_geo = Some("global".into());
    o.service_tier = Some("standard".into());
    o.completeness = "complete".into();
    o.tokens = Tokens {
        input: Some(700),
        cache_read: Some(100),
        cache_write: Some(200),
        write_30m: Some(80),
        write_12h: Some(120),
        output: Some(100),
        ..Default::default()
    };
    o
}

#[test]
fn captures_disjoint_openai_lifetimes_in_both_wire_formats() {
    for (input, details, output) in [
        ("input_tokens", "input_tokens_details", "output_tokens"),
        ("prompt_tokens", "prompt_tokens_details", "completion_tokens"),
    ] {
        let mut o = event("capture");
        o.tokens = Tokens::default();
        let usage = json!({
            input: 1000, output: 100,
            details: {"cached_tokens":100,"cache_write_tokens":200,
                "cache_write_tokens_details":{"ttl_30m_tokens":80,"ttl_12h_tokens":120}}
        });
        observe(&mut o, &json!({"usage":usage}));
        assert_eq!(o.tokens, event("expected").tokens);
        assert_eq!(o.tokens.total(), Some(1100));
        assert_eq!(o.completeness, "complete");
        assert_eq!(Catalogue::provider_fixture().price(&o).cost_nanos, Some(3_210_000));
        observe(&mut o, &json!({"usage":usage}));
        observe(&mut o, &json!({"usage":{output:110}}));
        assert_eq!((o.tokens.write_30m, o.tokens.write_12h), (Some(80), Some(120)));
        assert_eq!(o.tokens.total(), Some(1110));
    }
}

#[test]
fn lifetime_rates_cover_threshold_tier_and_region_without_double_counting() {
    let c = Catalogue::provider_fixture();
    let mut o = event("rates");
    // 30m = 1.25x input; 12h = 2.5x input, followed by the tier/region multiplier.
    assert_eq!(c.price(&o).cost_nanos, Some(700 * 2000 + 100 * 100 + 80 * 2500 + 120 * 5000 + 100 * 10000));
    o.tokens.input = Some(272_001);
    let long_context = 272_001 * 4000 + 100 * 200 + 80 * 5000 + 120 * 10000 + 100 * 15000;
    assert_eq!(c.price(&o).cost_nanos, Some(long_context));
    o.service_tier = Some("priority".into());
    o.inference_geo = Some("us".into());
    assert_eq!(c.price(&o).cost_nanos, Some(long_context * 22 / 10));
    o.tokens.write_5m = Some(1);
    assert!(o.tokens.validate().is_err(), "provider TTL subsets cannot overlap beyond total writes");
}

#[test]
fn absent_or_incomplete_lifetimes_are_unpriced_and_legacy_catalogues_still_load() {
    let c = Catalogue::provider_fixture();
    let mut o = event("unknown");
    o.tokens.write_30m = None;
    o.tokens.write_12h = None;
    assert_eq!(c.price(&o).basis, "unknown_cache_write_ttl");
    o.tokens.write_30m = Some(80);
    assert_eq!(c.price(&o).basis, "unknown_cache_write_ttl");
    // A subset equal to the total proves there are no writes in another lifetime.
    o.tokens.write_30m = Some(200);
    assert_eq!(c.price(&o).cost_nanos, Some(2_910_000));
    o.tokens.cache_write = Some(0);
    o.tokens.write_30m = None;
    assert_eq!(c.price(&o).cost_nanos, Some(2_410_000));

    let mut legacy: Value = serde_json::to_value(&c).unwrap();
    for rate in legacy["rates"].as_array_mut().unwrap() {
        let r = rate.as_object_mut().unwrap();
        if let Some(short) = r.remove("cache_write_30m_nanos_per_token") {
            r.insert("cache_write_nanos_per_token".into(), short);
        }
        if let Some(short) = r.remove("above_threshold_write_30m_nanos_per_token") {
            r.insert("above_threshold_write_nanos_per_token".into(), short);
        }
        r.remove("cache_write_12h_nanos_per_token");
        r.remove("above_threshold_write_12h_nanos_per_token");
    }
    let legacy: Catalogue = serde_json::from_value(legacy).unwrap();
    let mut o = event("legacy");
    o.tokens.write_30m = None;
    o.tokens.write_12h = None;
    assert_eq!(legacy.price(&o).cost_nanos, Some(2_910_000));
}

#[test]
fn malformed_lifetime_counts_never_keep_stale_known_lifetimes() {
    let c = Catalogue::provider_fixture();
    for details in [
        json!({"ttl_30m_tokens":null,"ttl_12h_tokens":120}),
        json!({"ttl_30m_tokens":-1,"ttl_12h_tokens":120}),
        json!({"ttl_30m_tokens":80,"ttl_12h_tokens":121}),
        json!({"ttl_30m_tokens":80,"ttl_12h_tokens":100}),
        Value::Null,
    ] {
        let mut o = event("invalid");
        observe(
            &mut o,
            &json!({"usage":{"input_tokens":1000,"output_tokens":100,
            "input_tokens_details":{"cached_tokens":100,"cache_write_tokens":200,"cache_write_tokens_details":details}}}),
        );
        assert_eq!(o.completeness, "partial", "{details}");
        assert_eq!(c.price(&o).cost_nanos, None, "{details}");
    }
    let mut o = event("replace");
    observe(
        &mut o,
        &json!({"usage":{"input_tokens":1100,"output_tokens":100,
        "input_tokens_details":{"cached_tokens":100,"cache_write_tokens":300}}}),
    );
    assert_eq!((o.tokens.write_30m, o.tokens.write_12h), (None, None));
    assert_eq!(c.price(&o).basis, "unknown_cache_write_ttl");
}

#[test]
fn canonical_write_total_wins_conflicting_alias_and_marks_partial() {
    let mut o = event("aliases");
    observe(
        &mut o,
        &json!({"usage":{"input_tokens":1000,"output_tokens":100,
        "input_tokens_details":{"cached_tokens":100,"cache_write_tokens":200,"cache_creation_tokens":300,
            "cache_write_tokens_details":{"ttl_30m_tokens":80,"ttl_12h_tokens":120}}}}),
    );
    assert_eq!((o.tokens.input, o.tokens.cache_write), (Some(700), Some(200)));
    assert_eq!(o.completeness, "partial");
}

#[test]
fn old_observation_serialization_and_fingerprints_have_no_new_null_fields() {
    let raw =
        r#"{"input":1,"cache_read":2,"cache_write":3,"write_5m":null,"write_1h":null,"output":4,"reasoning":null}"#;
    let t: Tokens = serde_json::from_str(raw).unwrap();
    assert_eq!(serde_json::to_string(&t).unwrap(), raw);
}

#[tokio::test]
async fn lifetime_columns_survive_reopen_rollups_and_combined_deduplication() {
    let dir = std::env::temp_dir().join(format!("fusebox-lifetime-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("usage.sqlite3");
    let store = Store::open(&path, 32, 3650, None).unwrap();
    let mut proxy = event("proxy");
    proxy.response_id = Some("same-response".into());
    let mut imported = proxy.clone();
    imported.source = "codex".into();
    imported.source_event_id = "native".into();
    store.call(move |c| insert_batch(c, &[proxy, imported])).await.unwrap();
    let rolled = store.dashboard(Query::default()).await.unwrap();
    let scanned = store.dashboard_scan(Query::default()).await.unwrap();
    assert_eq!(rolled, scanned);
    assert_eq!(rolled["combined"]["totals"]["observations"], 1);
    assert_eq!(rolled["combined"]["totals"]["tokens"]["cache_write"], 200);
    assert_eq!(rolled["combined"]["totals"]["tokens"]["write_30m"], 80);
    assert_eq!(rolled["combined"]["totals"]["tokens"]["write_12h"], 120);
    assert_eq!(rolled["combined"]["totals"]["estimated_cost_nanos"], 2_910_000);
    store.shutdown().await.unwrap();
    drop(store);
    let store = Store::open(&path, 32, 3650, None).unwrap();
    let after = store.dashboard(Query::default()).await.unwrap();
    assert_eq!(after["combined"], rolled["combined"]);
    store.shutdown().await.unwrap();
    drop(store);
    let error = downgrade(&path).unwrap_err().to_string();
    assert!(error.contains("cannot downgrade"), "{error}");
    let c = Connection::open(&path).unwrap();
    assert_eq!(c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), VERSION);
    drop(c);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn schema_four_migration_preserves_original_documents_without_guessing() {
    let mut c = Connection::open_in_memory().unwrap();
    migrate(&mut c).unwrap();
    let cat = Catalogue::provider_fixture();
    c.execute("INSERT INTO usage_meta VALUES('catalogue',?1)", [serde_json::to_string(&cat).unwrap()]).unwrap();
    let mut o = event("old");
    o.tokens.write_30m = None;
    o.tokens.write_12h = None;
    insert_batch(&mut c, &[o]).unwrap();
    let original: (String, String, String) = c
        .query_row("SELECT payload,fingerprint,snapshot_json FROM usage_observations", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    // Restore the exact column shape before lifetime accounting existed.
    for trigger in rollup::TRIGGERS {
        c.execute_batch(&format!("DROP TRIGGER {trigger}")).unwrap();
    }
    c.execute_batch(
        "DROP TABLE usage_rollup; DROP TABLE usage_facets;
        CREATE TABLE usage_rollup(old_layout INTEGER); CREATE TABLE usage_facets(old_layout INTEGER);
        ALTER TABLE usage_observations DROP COLUMN write_30m; ALTER TABLE usage_observations DROP COLUMN write_12h;
        ALTER TABLE usage_entries DROP COLUMN write_30m; ALTER TABLE usage_entries DROP COLUMN write_12h;
        PRAGMA user_version=4;",
    )
    .unwrap();
    migrate(&mut c).unwrap();
    let migrated: (String, String, String) = c
        .query_row("SELECT payload,fingerprint,snapshot_json FROM usage_observations", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(migrated, original);
    assert_eq!(reprice(&mut c).unwrap(), 0);
    assert_eq!(
        c.query_row("SELECT write_30m FROM usage_observations", [], |r| r.get::<_, Option<i64>>(0)).unwrap(),
        None
    );
}

#[test]
fn lifetime_details_without_write_total_preserve_evidence_and_remain_unpriced() {
    for details in [json!({"ttl_30m_tokens":80,"ttl_12h_tokens":120}), json!({"ttl_12h_tokens":120}), Value::Null] {
        let mut o = event("missing-total");
        observe(
            &mut o,
            &json!({"usage":{"input_tokens":1000,"output_tokens":100,
            "input_tokens_details":{"cached_tokens":100,"cache_write_tokens_details":details}}}),
        );
        assert_eq!(o.tokens.cache_write, None, "{details}");
        assert_eq!(o.tokens.input, None, "{details}");
        assert_eq!(o.completeness, "partial", "{details}");
        assert_eq!(Catalogue::provider_fixture().price(&o).cost_nanos, None, "{details}");
        if details["ttl_12h_tokens"].is_u64() {
            assert_eq!(o.tokens.write_12h, Some(120));
        }
    }
}

#[test]
fn richer_lifetime_evidence_wins_replayed_response_in_either_order() {
    for reverse in [false, true] {
        let mut c = Connection::open_in_memory().unwrap();
        migrate(&mut c).unwrap();
        let cat = Catalogue::provider_fixture();
        c.execute("INSERT INTO usage_meta VALUES('catalogue',?1)", [serde_json::to_string(&cat).unwrap()]).unwrap();
        let mut known = event("z-known");
        known.response_id = Some("replayed".into());
        let mut unknown = known.clone();
        unknown.source_event_id = "a-unknown".into();
        unknown.tokens.write_30m = None;
        unknown.tokens.write_12h = None;
        let records = if reverse { [known, unknown] } else { [unknown, known] };
        for record in records {
            insert_batch(&mut c, &[record]).unwrap();
        }
        let winner: (String, Option<i64>, Option<i64>, Option<i64>) = c
            .query_row("SELECT source_event_id,cost_nanos,write_30m,write_12h FROM usage_entries", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap();
        assert_eq!(winner, ("z-known".into(), Some(3_210_000), Some(80), Some(120)));
    }
}

#[test]
fn repricing_corrects_only_trustworthy_stored_lifetimes_with_full_audit() {
    let mut c = Connection::open_in_memory().unwrap();
    migrate(&mut c).unwrap();
    let cat = Catalogue::provider_fixture();
    let mut legacy = cat.clone();
    legacy.version = "2026-10-18.1".into();
    for rate in &mut legacy.rates {
        if rate.provider == "openai" {
            rate.cache_write_nanos_per_token = rate.cache_write_30m_nanos_per_token.take();
            rate.cache_write_12h_nanos_per_token = None;
            rate.above_threshold_write_nanos_per_token = rate.above_threshold_write_30m_nanos_per_token.take();
            rate.above_threshold_write_12h_nanos_per_token = None;
        }
    }
    c.execute("INSERT INTO usage_meta VALUES('catalogue',?1)", [serde_json::to_string(&legacy).unwrap()]).unwrap();
    let known = event("known");
    let mut unknown = event("unknown");
    unknown.tokens.write_30m = None;
    unknown.tokens.write_12h = None;
    insert_batch(&mut c, &[known, unknown]).unwrap();
    let original: Vec<(String, String, String, String)> = c
        .prepare(
            "SELECT source_event_id,payload,fingerprint,snapshot_json FROM usage_observations ORDER BY source_event_id",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    c.execute("UPDATE usage_meta SET value=?1 WHERE key='catalogue'", [serde_json::to_string(&cat).unwrap()]).unwrap();
    assert_eq!(reprice(&mut c).unwrap(), 1);
    assert_eq!(reprice(&mut c).unwrap(), 0);
    let costs: Vec<(String, i64)> = c
        .prepare("SELECT source_event_id,cost_nanos FROM usage_observations ORDER BY source_event_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(costs, vec![("known".into(), 3_210_000), ("unknown".into(), 2_910_000)]);
    for (id, payload, fingerprint, old_snapshot) in original {
        let current: (String, String, String) = c
            .query_row(
                "SELECT payload,fingerprint,snapshot_json FROM usage_observations WHERE source_event_id=?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((&current.0, &current.1), (&payload, &fingerprint));
        if id == "unknown" {
            assert_eq!(current.2, old_snapshot);
        } else {
            let revision: (String, String) = c
                .query_row("SELECT reason,previous_snapshot FROM usage_price_revisions", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .unwrap();
            assert_eq!(revision, ("openai_cache_write_lifetime_correction".into(), old_snapshot));
        }
    }
}

#[test]
fn same_total_partial_breakdown_replaces_old_lifetimes_atomically() {
    let mut o = event("same-total");
    observe(
        &mut o,
        &json!({"usage":{"input_tokens":100,"output_tokens":10,
        "input_tokens_details":{"cached_tokens":20,"cache_write_tokens":10,
            "cache_write_tokens_details":{"ttl_30m_tokens":8,"ttl_12h_tokens":2}}}}),
    );
    assert_eq!((o.tokens.write_30m, o.tokens.write_12h), (Some(8), Some(2)));
    observe(&mut o, &json!({"usage":{"output_tokens":11}}));
    assert_eq!((o.tokens.write_30m, o.tokens.write_12h), (Some(8), Some(2)));
    observe(
        &mut o,
        &json!({"usage":{"input_tokens_details":{"cache_write_tokens":10,
        "cache_write_tokens_details":{"ttl_30m_tokens":0}}}}),
    );
    assert_eq!(o.tokens.cache_write, Some(10));
    assert_eq!((o.tokens.write_30m, o.tokens.write_12h), (Some(0), None));
    assert_eq!(Catalogue::provider_fixture().price(&o).basis, "unknown_cache_write_ttl");
}
