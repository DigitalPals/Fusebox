//! YAML properties: what Fusebox writes reads back unchanged, and `crate::yaml`
//! reads documents the way `serde_yaml` 0.9 did. A differential run against
//! `serde_yaml` (issue #42) produced `yaml_corpus.json`: what the old parser
//! made of each document there, which the new one must keep doing. Generated
//! documents are config-shaped trees heavy on scalars YAML parsers disagree about.

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
        (-1.0e6f64..1.0e6).prop_map(|f| serde_json::Number::from_f64(f).map_or(serde_json::Value::Null, Into::into)),
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
    crate::yaml::from_str(text).unwrap_or_else(|e| panic!("emitted YAML parses: {e}\n{text}"))
}

/// Renders a tree as YAML text in styles drawn from `picks`: block or flow,
/// strings single- or double-quoted, two or four columns of indentation,
/// stray comments. Read back, it must be the same tree.
fn render(value: &serde_json::Value, picks: &mut Picks, indent: usize, out: &mut String) {
    let pad = |out: &mut String, n: usize| out.extend(std::iter::repeat_n(' ', n));
    match value {
        serde_json::Value::Object(map) if !map.is_empty() && picks.next(3) != 0 => {
            let step = if picks.next(2) == 0 { 2 } else { 4 };
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    pad(out, indent);
                }
                out.push_str(&scalar_text(&serde_json::Value::String(k.clone()), 1 + picks.next(3)));
                out.push(':');
                if v.as_object().is_some_and(|m| !m.is_empty()) || v.as_array().is_some_and(|a| !a.is_empty()) {
                    if picks.next(5) == 0 {
                        out.push_str(" # note");
                    }
                    out.push('\n');
                    pad(out, indent + step);
                } else {
                    out.push(' ');
                }
                render(v, picks, indent + step, out);
            }
        }
        serde_json::Value::Array(items) if !items.is_empty() && picks.next(3) != 0 => {
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    pad(out, indent);
                }
                out.push_str("- ");
                render(v, picks, indent + 2, out);
            }
        }
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            flow(value, out);
            out.push('\n');
        }
        scalar => {
            out.push_str(&scalar_text(scalar, 1 + picks.next(3)));
            out.push('\n');
        }
    }
}

/// A deterministic stream of style choices.
struct Picks(u64);

impl Picks {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

/// Flow style, with strings double-quoted so a flow indicator inside a string
/// doesn't change the structure.
fn flow(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&serde_json::to_string(k).unwrap());
                out.push_str(": ");
                flow(v, out);
            }
            out.push('}');
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                flow(v, out);
            }
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other).unwrap()),
    }
}

fn scalar_text(value: &serde_json::Value, style: u64) -> String {
    match value {
        // A raw line break in a quoted scalar folds to a space, so multi-line
        // strings are always escaped.
        serde_json::Value::String(s) if s.contains('\n') => serde_json::to_string(s).unwrap(),
        serde_json::Value::String(s) => match style {
            1 => format!("'{}'", s.replace('\'', "''")),
            _ => serde_json::to_string(s).unwrap(),
        },
        serde_json::Value::Null if style == 1 => "~".into(),
        other => serde_json::to_string(other).unwrap(),
    }
}

/// Raw scalar spellings the core schema resolves in different ways.
fn number_like() -> impl Strategy<Value = String> {
    prop_oneof![
        "[-+]?(0|[1-9][0-9]{0,19})",
        "[-+]?0[0-9]{1,4}",
        "[-+]?0[xob][0-9a-fA-F_]{0,6}",
        "[-+]?[0-9]{0,3}\\.[0-9]{0,3}([eE][-+]?[0-9]{1,3})?",
        "[-+]?\\.(inf|Inf|INF|nan|NaN|NAN)",
        "(true|True|TRUE|false|False|FALSE|yes|no|on|off|y|n|~|null|Null|NULL|nul)",
        "[0-9]{1,2}:[0-9]{2}(:[0-9]{2})?",
        "[0-9]{4}-[0-9]{2}-[0-9]{2}",
    ]
}

proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(256))]

    #[test]
    fn generated_documents_round_trip_through_emit_and_parse(value in tree()) {
        let emitted = crate::yaml::to_string(&value).expect("value emits as YAML");
        prop_assert_eq!(parse_as_json(&emitted), value);
    }

    #[test]
    fn emitted_documents_are_stable(value in tree()) {
        // Writing the reparsed tree must give the same document: the
        // dashboard's config saves depend on rewrite stability.
        let once = crate::yaml::to_string(&value).expect("value emits as YAML");
        let twice = crate::yaml::to_string(&parse_as_json(&once)).expect("reparsed value emits");
        prop_assert_eq!(twice, once);
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
        let emitted = crate::yaml::to_string(&doc).expect("config emits");
        prop_assert_eq!(parse_as_json(&emitted), doc);
    }

    #[test]
    fn rendered_documents_read_back_as_their_tree(value in tree(), seed in any::<u64>()) {
        let mut text = String::new();
        render(&value, &mut Picks(seed), 0, &mut text);
        prop_assert_eq!(crate::yaml::from_str(&text).map_err(|e| format!("{e}\n{text}")), Ok(value));
    }

    #[test]
    fn quoted_number_like_scalars_stay_strings(raw in number_like()) {
        let text = format!("v: '{raw}'\nw: \"{raw}\"\n");
        prop_assert_eq!(parse_as_json(&text), serde_json::json!({"v": raw.clone(), "w": raw}));
    }

    #[test]
    fn arbitrary_text_never_panics(text in "[ -~\n\t]{0,64}", raw in number_like()) {
        let _ = crate::yaml::from_str(&text);
        let _ = crate::yaml::from_str(&format!("{raw}: {raw}\n- {raw}\n"));
    }
}

/// Where `crate::yaml` deliberately reads differently from `serde_yaml`, and what
/// it does instead: both follow YAML 1.2 where libyaml was lenient.
const KNOWN_DIFFERENCES: &[(&str, Option<&str>)] = &[
    // A quoted scalar's continuation line must be indented under its parent node.
    ("a:\n  b: 'multi\nline'\n", None),
    ("a:\n  b: \"multi\nline\"\n", None),
    // Anchor names may contain `:` (libyaml stopped at it).
    ("&a: b\n", Some(r#""b""#)),
    // A bare tag in a flow list (libyaml refused it; a block value took it).
    ("a: [!t]\n", Some(r#"{"a": [{"t": null}]}"#)),
];

#[test]
fn known_differences_from_serde_yaml_hold() {
    for (text, want) in KNOWN_DIFFERENCES {
        let ours = crate::yaml::from_str(text);
        match want {
            Some(json) => assert_eq!(ours.as_ref().ok(), Some(&serde_json::from_str(json).unwrap()), "{text:?}"),
            None => assert!(ours.is_err(), "{text:?} should be refused: {ours:?}"),
        }
    }
}

/// What `serde_yaml` 0.9 made of config-shaped documents and of the corners
/// where YAML parsers differ, recorded from a run against it: `refused` where
/// it failed, the tree where it gave the same one, and, marked `kept`, the tree
/// this parser makes where serde_yaml accepted a document whose tree has no
/// JSON form (tags, non-string keys, infinities). Every document that loaded
/// before still loads.
#[test]
fn documents_read_as_serde_yaml_read_them() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!("yaml_corpus.json")).unwrap();
    assert!(cases.len() >= 90);
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|case| {
            let text = case["yaml"].as_str().unwrap();
            let got = crate::yaml::from_str(text);
            let ok = if case["refused"] == true { got.is_err() } else { got.as_ref().ok() == Some(&case["value"]) };
            (!ok).then(|| format!("{text:?}: want {case}, got {got:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{} document(s) read differently:\n{}", wrong.len(), wrong.join("\n"));
}

#[test]
fn collections_nest_at_most_128_deep() {
    for (depth, ok) in [(100, true), (128, true), (129, false), (200, false)] {
        let seq = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        assert_eq!(crate::yaml::from_str(&seq).is_ok(), ok, "sequences {depth} deep");
        let map = format!("{}1{}", "{a: ".repeat(depth), "}".repeat(depth));
        assert_eq!(crate::yaml::from_str(&map).is_ok(), ok, "mappings {depth} deep");
    }
}
