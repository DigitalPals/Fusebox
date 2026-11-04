//! Format-translation properties: parsing never panics on arbitrary JSON,
//! and parse→build reaches a fixed point in each wire format (building the
//! reparsed request yields the same body again), which is what keeps a
//! conversation replayed through Fusebox stable turn after turn.

use proptest::prelude::*;
use serde_json::{Value, json};

use crate::ir::Format;

fn scalar() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-100i64..100).prop_map(|n| Value::Number(n.into())),
        "[a-zA-Z0-9 ]{0,16}".prop_map(Value::String),
    ]
}

fn any_json() -> impl Strategy<Value = Value> {
    scalar().prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            proptest::collection::btree_map("[a-z_]{1,10}", inner, 0..4)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}

fn format() -> impl Strategy<Value = Format> {
    prop_oneof![Just(Format::Chat), Just(Format::Responses), Just(Format::Claude), Just(Format::Gemini)]
}

fn build(format: Format, req: &crate::ir::Request) -> Value {
    match format {
        Format::Chat => super::chat::build_request(req, "prop-model"),
        Format::Claude => super::claude::build_request(req, "prop-model"),
        Format::Gemini => super::gemini::build_request(req, "prop-model"),
        Format::Responses => super::responses::build_request(
            req,
            "prop-model",
            &super::responses::BuildOpts { chatgpt_backend: false, custom_tools: false, default_reasoning: false },
        ),
    }
}

/// A simple valid client body in `format`'s wire shape: an optional system
/// text and 1–3 alternating text turns starting with the user.
fn client_body(format: Format, system: &str, turns: &[String], max_tokens: Option<u64>) -> Value {
    let roles = ["user", "assistant"];
    match format {
        Format::Claude => {
            let mut body = json!({"messages": turns.iter().enumerate()
                .map(|(i, t)| json!({"role": roles[i % 2], "content": t})).collect::<Vec<_>>()});
            if !system.is_empty() {
                body["system"] = json!(system);
            }
            if let Some(m) = max_tokens {
                body["max_tokens"] = json!(m);
            }
            body
        }
        Format::Chat => {
            let mut messages: Vec<Value> = Vec::new();
            if !system.is_empty() {
                messages.push(json!({"role": "system", "content": system}));
            }
            messages.extend(turns.iter().enumerate().map(|(i, t)| json!({"role": roles[i % 2], "content": t})));
            let mut body = json!({"messages": messages});
            if let Some(m) = max_tokens {
                body["max_tokens"] = json!(m);
            }
            body
        }
        Format::Responses => {
            let mut body = json!({"input": turns.iter().enumerate()
                .map(|(i, t)| json!({"role": roles[i % 2], "content": t})).collect::<Vec<_>>()});
            if !system.is_empty() {
                body["instructions"] = json!(system);
            }
            if let Some(m) = max_tokens {
                body["max_output_tokens"] = json!(m);
            }
            body
        }
        Format::Gemini => {
            let gemini_roles = ["user", "model"];
            let mut body = json!({"contents": turns.iter().enumerate()
                .map(|(i, t)| json!({"role": gemini_roles[i % 2], "parts": [{"text": t}]}))
                .collect::<Vec<_>>()});
            if !system.is_empty() {
                body["systemInstruction"] = json!({"parts": [{"text": system}]});
            }
            if let Some(m) = max_tokens {
                body["generationConfig"] = json!({"maxOutputTokens": m});
            }
            body
        }
    }
}

proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(128))]

    #[test]
    fn parsing_arbitrary_json_never_panics(format in format(), body in any_json()) {
        // The result may be a structured error; it must not panic.
        let _ = super::parse_request(format, &body);
    }

    #[test]
    fn parse_build_reaches_a_fixed_point(
        format in format(),
        system in "[a-zA-Z0-9 ]{0,24}",
        turns in proptest::collection::vec("[a-zA-Z0-9 ]{1,24}", 1..4),
        max_tokens in proptest::option::of(1u64..4096),
    ) {
        let body = client_body(format, &system, &turns, max_tokens);
        let Ok(req) = super::parse_request(format, &body) else { return Ok(()) };
        // Parsing normalizes: blank turns drop and consecutive same-role
        // turns merge, so the first reparsing may still change the shape.
        // After that the built body must be a fixed point — anything else
        // means the request mutates a little more on every replayed turn.
        let mut built = build(format, &req);
        let mut stable = false;
        for _ in 0..3 {
            let reparsed = super::parse_request(format, &built)
                .map_err(|e| TestCaseError::fail(format!("built body no longer parses: {e}")))?;
            let rebuilt = build(format, &reparsed);
            stable = rebuilt == built;
            if stable {
                break;
            }
            built = rebuilt;
        }
        prop_assert!(stable, "parse/build did not stabilize within three rounds");
    }

    #[test]
    fn translated_builds_keep_the_turn_texts(
        from in format(),
        to in format(),
        turns in proptest::collection::vec("[a-z]{4,12}", 1..3),
    ) {
        let body = client_body(from, "", &turns, None);
        let Ok(req) = super::parse_request(from, &body) else { return Ok(()) };
        let built = build(to, &req);
        let text = serde_json::to_string(&built).unwrap();
        for turn in &turns {
            prop_assert!(text.contains(turn.as_str()), "turn {turn} lost translating {from:?} -> {to:?}");
        }
    }
}
