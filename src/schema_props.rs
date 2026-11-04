//! Schema-cleaner properties: cleaning is total (never panics) and a fixed
//! point — re-cleaning an already-clean schema changes nothing, so a schema
//! can safely pass through more than one layer that cleans it.

use proptest::prelude::*;
use serde_json::Value;

fn scalar() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-100i64..100).prop_map(|n| Value::Number(n.into())),
        prop_oneof![
            Just("string".into()),
            Just("integer".into()),
            Just("object".into()),
            Just("array".into()),
            Just("boolean".into()),
            Just("#/$defs/Thing".into()),
            "[a-z ]{0,16}",
        ]
        .prop_map(Value::String),
    ]
}

/// Schema-shaped objects: the keywords the cleaner rewrites, mixed with
/// arbitrary JSON so unexpected shapes are covered too.
const KEYS: &[&str] = &[
    "type",
    "properties",
    "items",
    "required",
    "enum",
    "const",
    "$ref",
    "$defs",
    "allOf",
    "anyOf",
    "oneOf",
    "additionalProperties",
    "description",
    "format",
    "minLength",
    "pattern",
    "if",
    "title",
];

fn schema() -> impl Strategy<Value = Value> {
    let key = prop_oneof![
        3 => proptest::sample::select(KEYS).prop_map(|k: &str| k.to_string()).boxed(),
        1 => "[a-z]{1,8}".boxed(),
    ];
    scalar().prop_recursive(4, 32, 5, move |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            proptest::collection::btree_map(key.clone(), inner, 0..5)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}

/// A schema whose root defines `Thing` and refers to it (directly, or from a
/// property), where the definition may refer to itself.
fn schema_with_defs() -> impl Strategy<Value = Value> {
    (schema(), schema(), any::<bool>()).prop_map(|(def, body, direct)| {
        let mut root = match body {
            Value::Object(map) => map,
            other => serde_json::Map::from_iter([("items".to_string(), other)]),
        };
        if direct {
            root.insert("$ref".into(), "#/$defs/Thing".into());
        } else {
            root.insert("properties".into(), serde_json::json!({"thing": {"$ref": "#/$defs/Thing"}}));
        }
        root.insert("$defs".into(), serde_json::json!({"Thing": def}));
        Value::Object(root)
    })
}

/// Whether any object in `value` still has `"$ref": target`.
fn refers_to(value: &Value, target: &str) -> bool {
    match value {
        Value::Object(map) => {
            map.get("$ref").and_then(Value::as_str) == Some(target) || map.values().any(|v| refers_to(v, target))
        }
        Value::Array(items) => items.iter().any(|v| refers_to(v, target)),
        _ => false,
    }
}

proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(128))]

    #[test]
    fn inline_only_resolves_every_defined_reference(value in schema_with_defs()) {
        let once = crate::schema::inline_only(&value);
        prop_assert!(once.get("$defs").is_none(), "{}", once);
        // Resolved, or (recursion) replaced by a placeholder: never left dangling.
        prop_assert!(!refers_to(&once, "#/$defs/Thing"), "{}", once);
        prop_assert_eq!(crate::schema::inline_only(&once), once);
    }

    #[test]
    fn clean_tool_is_idempotent(value in schema(), validated in any::<bool>()) {
        let once = crate::schema::clean_tool(&value, validated);
        let twice = crate::schema::clean_tool(&once, validated);
        prop_assert_eq!(twice, once);
    }

    #[test]
    fn clean_response_is_idempotent(value in schema()) {
        let once = crate::schema::clean_response(&value);
        let twice = crate::schema::clean_response(&once);
        prop_assert_eq!(twice, once);
    }

    #[test]
    fn inline_only_is_idempotent(value in schema()) {
        let once = crate::schema::inline_only(&value);
        let twice = crate::schema::inline_only(&once);
        prop_assert_eq!(twice, once);
    }
}
