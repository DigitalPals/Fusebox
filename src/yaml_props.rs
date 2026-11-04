//! YAML round-trip properties: the contract the config parser must keep,
//! whichever YAML crate backs it (the differential gate for replacing
//! `serde_yaml`, issue #42). Generated documents are config-shaped trees
//! heavy on scalars YAML parsers disagree about.

use proptest::prelude::*;

/// Strings that historically split YAML parsers, plus model-id shapes from
/// real configs.
fn tricky_string() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("yes".into()),
        Just("no".into()),
        Just("on".into()),
        Just("off".into()),
        Just("true".into()),
        Just("null".into()),
        Just("~".into()),
        Just("0o17".into()),
        Just("0x10".into()),
        Just("1_000".into()),
        Just("2026-11-04".into()),
        Just("12:34:56".into()),
        Just("# comment".into()),
        Just("key: value".into()),
        Just("- item".into()),
        Just("*alias".into()),
        Just("&anchor".into()),
        Just("<<".into()),
        Just(" leading".into()),
        Just("trailing ".into()),
        Just("multi\nline".into()),
        Just("café 🦀".into()),
        Just("claude-opus-*".into()),
        Just("gpt-6.1-sol(high)".into()),
        Just("".into()),
        "[a-zA-Z0-9 _./:-]{0,24}",
    ]
}

fn scalar() -> impl Strategy<Value = serde_json::Value> {
    prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::Bool),
        (-10_000i64..10_000).prop_map(|n| serde_json::Value::Number(n.into())),
        tricky_string().prop_map(serde_json::Value::String),
    ]
}

fn tree() -> impl Strategy<Value = serde_json::Value> {
    scalar().prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(serde_json::Value::Array),
            proptest::collection::btree_map(tricky_string(), inner, 0..4)
                .prop_map(|map| serde_json::Value::Object(map.into_iter().collect())),
        ]
    })
}

fn parse_as_json(text: &str) -> serde_json::Value {
    let yaml: serde_yaml::Value = serde_yaml::from_str(text).expect("generated YAML parses");
    serde_json::to_value(yaml).expect("YAML tree transcodes to JSON")
}

proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(128))]

    #[test]
    fn generated_documents_round_trip_through_emit_and_parse(value in tree()) {
        let emitted = serde_yaml::to_string(&value).expect("value emits as YAML");
        let back = parse_as_json(&emitted);
        prop_assert_eq!(back, value);
    }

    #[test]
    fn emitted_documents_are_stable(value in tree()) {
        // Emitting the reparsed tree must not keep changing the document:
        // the dashboard's config saves depend on rewrite stability.
        let once = serde_yaml::to_string(&value).expect("value emits as YAML");
        let twice = serde_yaml::to_string(&parse_as_json(&once)).expect("reparsed value emits");
        prop_assert_eq!(parse_as_json(&twice), parse_as_json(&once));
    }

    #[test]
    fn config_shaped_documents_round_trip(
        port in 1u16..=65535,
        host in tricky_string(),
        models in proptest::collection::vec(tricky_string(), 0..4),
        enabled in any::<bool>(),
    ) {
        let doc = serde_json::json!({
            "host": host, "port": port, "debug": enabled,
            "extra-models": {"claude": models},
        });
        let emitted = serde_yaml::to_string(&doc).expect("config emits");
        prop_assert_eq!(parse_as_json(&emitted), doc);
    }
}
