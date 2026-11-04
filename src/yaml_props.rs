//! YAML properties: what Fusebox writes reads back unchanged, and `crate::yaml`
//! reads documents the way `serde_yaml` 0.9 did (the differential gate for
//! replacing it, issue #42). Generated documents are config-shaped trees
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

/// What `serde_yaml` 0.9 (the parser config files had until issue #42) made of
/// a document, as the tree `crate::yaml` must produce: `None` where it failed,
/// or where its tree has no JSON form (tags, non-string keys, infinities),
/// which `crate::yaml` refuses.
fn reference(text: &str) -> Option<serde_json::Value> {
    fn convert(v: serde_yaml::Value) -> Option<serde_json::Value> {
        Some(match v {
            serde_yaml::Value::Null => serde_json::Value::Null,
            serde_yaml::Value::Bool(b) => serde_json::Value::Bool(b),
            serde_yaml::Value::Number(n) => {
                if let Some(u) = n.as_u64() {
                    u.into()
                } else if let Some(i) = n.as_i64() {
                    i.into()
                } else {
                    serde_json::Number::from_f64(n.as_f64()?)?.into()
                }
            }
            serde_yaml::Value::String(s) => serde_json::Value::String(s),
            serde_yaml::Value::Sequence(items) => {
                serde_json::Value::Array(items.into_iter().map(convert).collect::<Option<_>>()?)
            }
            serde_yaml::Value::Mapping(map) => serde_json::Value::Object(
                map.into_iter().map(|(k, v)| Some((k.as_str()?.to_string(), convert(v)?))).collect::<Option<_>>()?,
            ),
            serde_yaml::Value::Tagged(_) => return None,
        })
    }
    convert(serde_yaml::from_str(text).ok()?)
}

/// Both parsers agree: the same tree, or both refuse the document.
fn agree(text: &str) -> Result<(), String> {
    match (reference(text), crate::yaml::from_str(text)) {
        (Some(want), Ok(got)) if want == got => Ok(()),
        (None, Err(_)) => Ok(()),
        (want, got) => Err(format!("{text:?}\n  serde_yaml:  {want:?}\n  crate::yaml: {got:?}")),
    }
}

/// Renders a tree as YAML text in styles drawn from `picks`: block or flow,
/// strings plain (unchecked, so often not the same string), single- or
/// double-quoted, two or four columns of indentation, stray comments.
fn render(value: &serde_json::Value, picks: &mut Picks, indent: usize, out: &mut String) {
    let pad = |out: &mut String, n: usize| out.extend(std::iter::repeat_n(' ', n));
    match value {
        serde_json::Value::Object(map) if !map.is_empty() && picks.next(3) != 0 => {
            let step = if picks.next(2) == 0 { 2 } else { 4 };
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    pad(out, indent);
                }
                // A plain key starting with `&` or `*` is anchor or alias syntax, where
                // YAML 1.2 lets the name run on into the colon (libyaml stopped there).
                let style = if k.starts_with(['&', '*']) { 2 } else { picks.next(4) };
                out.push_str(&scalar_text(&serde_json::Value::String(k.clone()), style));
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
            out.push_str(&scalar_text(scalar, picks.next(4)));
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
        // A raw line break would need the continuation indented under its parent
        // (`KNOWN_DIFFERENCES`), so multi-line strings are always escaped.
        serde_json::Value::String(s) if s.contains('\n') => serde_json::to_string(s).unwrap(),
        serde_json::Value::String(s) => match style {
            0 => s.clone(),
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
        prop_assert_eq!(&parse_as_json(&emitted), &value);
        // Other YAML readers must see the same tree in what Fusebox writes.
        prop_assert_eq!(reference(&emitted), Some(value), "{}", emitted);
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
    fn parsers_agree_on_generated_documents(value in tree(), seed in any::<u64>()) {
        let mut text = String::new();
        render(&value, &mut Picks(seed), 0, &mut text);
        let verdict = agree(&text);
        prop_assert!(verdict.is_ok(), "{}", verdict.unwrap_err());
    }

    #[test]
    fn parsers_agree_on_number_like_scalars(raw in number_like(), quoted in any::<bool>()) {
        let text = if quoted { format!("v: '{raw}'\nw: \"{raw}\"\n") } else { format!("v: {raw}\nw: [{raw}]\n{raw}: k\n") };
        let verdict = agree(&text);
        prop_assert!(verdict.is_ok(), "{}", verdict.unwrap_err());
        let alone = format!("{raw}\n");
        let verdict = agree(&alone);
        prop_assert!(verdict.is_ok(), "{}", verdict.unwrap_err());
    }
}

/// Documents both parsers must read the same way: config-shaped YAML and the
/// corners where YAML parsers differ.
const CORPUS: &[&str] = &[
    r#""#,
    r#"# just a comment
"#,
    "---\n",
    "---\n...\n",
    "--- a\n",
    "%YAML 1.2\n---\na: 1\n",
    "\u{feff}a: 1\n",
    "a: 1\r\nb: [2, 3]\r\n",
    r#"port: 8317
host: ""
auth-dir: ~/.fusebox
debug: false
"#,
    r#"api-keys:
  - one
  - "two"
claude-api-key:
- api-key: sk-1
  base-url: https://api.example.com:8443/v1
  models:
    - name: claude-opus-*
      alias: big
"#,
    r#"a:
  b:
    c: [1, {d: e}, [f]]
  g: {h: i, j: [k, l]}
"#,
    r#"list:
- a
-
- - nested
  - more
- key: v
  other: w
"#,
    "a: yes\nb: no\nc: on\nd: off\ne: y\nf: n\ng: Yes\nh: NO\n",
    "a: true\nb: True\nc: TRUE\nd: tRUE\ne: false\nf: FALSE\n",
    "a: ~\nb: null\nc: Null\nd: NULL\ne: nUll\nf:\n",
    "a: 0\nb: -0\nc: +0\nd: 007\ne: -007\nf: 0x1F\ng: 0X1F\nh: 0o17\ni: 0b101\nj: -0x10\nk: +0x10\nl: 0x-1\nm: 1_000\nn: 0x\n",
    "a: 9223372036854775807\nb: 9223372036854775808\nc: 18446744073709551615\nd: -9223372036854775808\n",
    "a: 18446744073709551616\n",
    "a: -9223372036854775809\n",
    "a: 1.5\nb: -1.5\nc: .5\nd: 5.\ne: 1e3\nf: 1E-3\ng: +1.0e+3\nh: 1.0e\ni: 0.1e1\nj: 00.5\nk: 1.2.3\n",
    "a: .inf\n",
    "a: -.Inf\n",
    "a: .NaN\n",
    "a: inf\nb: nan\nc: infinity\nd: NaN\n",
    "a: 2026-11-04\nb: 2026-11-04T10:00:00Z\nc: 12:30\nd: 1:20:30\n",
    r#"a: 'single ''quoted'''
b: "double \"q\" \n \t \u00e9 \x41 \U0001F600 \/"
c: 'it''s'
"#,
    "a: |\n  line one\n  line two\nb: |-\n  kept\nc: |+\n  trailing\n\nd: >\n  folded\n  text\n\n  para\ne: >-\n  x\n",
    "a: plain\n  continued\n  lines\nb: \"quoted\n  continued\"\n",
    "url: http://x/a#frag\nhash: a#b\ncomment: a # real comment\ncolon: a:b\nkv: key:value\n",
    r#""quoted key": 1
'single key': 2
key with spaces: 3
"": 4
"#,
    "1: one\n",
    "true: yes\n",
    "~: null\n",
    ": empty key\n",
    "? [complex]\n: key\n",
    "? simple\n: explicit\n",
    "a: 1\na: 2\n",
    "a: 1\n\"a\": 2\n",
    "x: &k a\n*k : b\na: c\n",
    "base: &b\n  x: 1\n  y: [1, 2]\ncopy: *b\nmerged:\n  <<: *b\n  z: 3\n",
    "s: &s scalar\nt: *s\nlist: &l [1, 2]\nm: [*l, *l]\n",
    "a: &a [*a]\n",
    "a: *missing\n",
    "a: &x 1\nb: &x 2\nc: *x\n",
    "a: !!str 123\n",
    "a: !!int \"7\"\n",
    "a: !!int 007\n",
    "a: !!float 1\n",
    "a: !!float 007\n",
    "a: !!float .inf\n",
    "a: !!bool true\n",
    "a: !!null ''\n",
    "a: !!null ~\n",
    "a: !!seq [1]\n",
    "a: !!map {x: 1}\n",
    "a: !!set {x: 1}\n",
    "a: !!binary 5\n",
    "a: !<tag:example.com,2026:x> 5\n",
    "a: !custom [1]\n",
    "a: !!int 1.5\n",
    "a: !!float abc\n",
    "a: !!bool yes\n",
    "a: !!binary aGVsbG8=\n",
    "a: !!timestamp 2026-11-04\n",
    "a: !custom value\n",
    "a: !custom {x: 1}\n",
    "a: ! 12\n",
    "%TAG !e! tag:example.com,2026:\n---\na: !e!thing x\n",
    "a: 1\n---\nb: 2\n",
    "--- 1\n--- 2\n",
    "a: b: c\n",
    "- a\nb: c\n",
    "a:\n\t- tab\n",
    "a: \"unclosed\n",
    "a: [1, 2\n",
    "a: {b: 1\n",
    "a:\n  - x\n - y\n",
    "key: [a, b, ]\n",
    "key: {a: 1, b: 2, }\n",
    "{a: 1, b: [x, y]}\n",
    "[1, two, 'three', {four: 4}]\n",
    "- &a x\n- *a\n",
    "café: 🦀\n\"\\u2028\": sep\n",
    r#"a: "\0\a\b\e\f\v\N\_\L\P"
"#,
    "a: '\n  multi\n  line\n  '\n",
    "a: - b\n",
    "a: -b\nc: ?d\ne: :f\n",
    "a: @x\n",
    "a: `x\n",
    "a: %x\n",
    "a: x\n...\n",
    "a: x\n...\nb: y\n",
];

/// Where `crate::yaml` deliberately reads differently from `serde_yaml`, and what
/// it does instead. Both follow YAML 1.2 where libyaml was lenient or YAML 1.1.
const KNOWN_DIFFERENCES: &[(&str, Option<&str>)] = &[
    // A quoted scalar's continuation line must be indented under its parent node.
    ("a:\n  b: 'multi\nline'\n", None),
    ("a:\n  b: \"multi\nline\"\n", None),
    // Anchor names may contain `:` (libyaml stopped at it).
    ("&a: b\n", Some(r#""b""#)),
];

#[test]
fn known_differences_from_serde_yaml_hold() {
    for (text, want) in KNOWN_DIFFERENCES {
        let ours = crate::yaml::from_str(text);
        match want {
            Some(json) => assert_eq!(ours.as_ref().ok(), Some(&serde_json::from_str(json).unwrap()), "{text:?}"),
            None => assert!(ours.is_err(), "{text:?} should be refused: {ours:?}"),
        }
        assert!(agree(text).is_err(), "{text:?} no longer differs from serde_yaml; drop it from the list");
    }
}

#[test]
fn parsers_agree_on_the_corpus() {
    let disagreements: Vec<String> = CORPUS.iter().filter_map(|text| agree(text).err()).collect();
    assert!(disagreements.is_empty(), "{} disagreement(s):\n{}", disagreements.len(), disagreements.join("\n"));
}

#[test]
fn parsers_agree_on_nesting_limits() {
    for depth in [100, 126, 127, 128, 129, 130, 200] {
        let seq = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        assert!(agree(&seq).is_ok(), "depth {depth}: {}", agree(&seq).unwrap_err());
        let map = format!("{}1{}", "{a: ".repeat(depth), "}".repeat(depth));
        assert!(agree(&map).is_ok(), "depth {depth}: {}", agree(&map).unwrap_err());
    }
}
