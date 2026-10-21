use super::*;
use crate::ir::{Finish, Usage};
use serde_json::json;

fn build(target: Format, req: &Request) -> Value {
    match target {
        Format::Chat => chat::build_request(req, "model"),
        Format::Responses => responses::build_request(
            req,
            "model",
            &responses::BuildOpts { chatgpt_backend: false, custom_tools: true, default_reasoning: false },
        ),
        Format::Claude => claude::build_request(req, "claude-sonnet-4-6"),
        Format::Gemini => gemini::build_request(req, "gemini-3-pro"),
    }
}

#[test]
fn mixed_openai_breakpoint_lifetimes_survive_round_trip() {
    let body = json!({
        "prompt_cache_options":{"mode":"explicit","ttl":"30m"},
        "messages":[
            {"role":"system","content":[
                {"type":"text","text":"stable instructions","prompt_cache_breakpoint":{"mode":"explicit","ttl":"12h"}},
                {"type":"text","text":"changing instructions"}
            ]},
            {"role":"developer","content":[
                {"type":"text","text":"workspace","prompt_cache_breakpoint":{"mode":"explicit","ttl":"30m"}}
            ]},
            {"role":"user","content":[
                {"type":"image_url","image_url":{"url":"https://example.com/image.png"},"prompt_cache_breakpoint":{"mode":"explicit","ttl":"12h"}},
                {"type":"text","text":"question","prompt_cache_breakpoint":{"mode":"explicit"}}
            ]},
            {"role":"assistant","content":[
                {"type":"text","text":"answer","prompt_cache_breakpoint":{"mode":"explicit","ttl":"30m"}}
            ],"tool_calls":[{"id":"a","type":"function","function":{"name":"lookup","arguments":"{}"}}]},
            {"role":"tool","tool_call_id":"a","content":[
                {"type":"text","text":"stable result","prompt_cache_breakpoint":{"mode":"explicit","ttl":"12h"}},
                {"type":"text","text":"changing result","prompt_cache_breakpoint":{"mode":"explicit","ttl":"30m"}}
            ]}
        ]
    });
    let mut responses = build(Format::Responses, &chat::parse_request(&body).unwrap());
    assert!(preserve_cache_hints(Format::Chat, Format::Responses, &body, &mut responses).unwrap().is_none());
    assert_eq!(responses["input"][0]["role"], "system");
    assert_eq!(responses["input"][1]["role"], "developer");
    assert_eq!(responses["input"][0]["content"][0]["prompt_cache_breakpoint"]["ttl"], "12h");
    assert_eq!(responses["input"][1]["content"][0]["prompt_cache_breakpoint"]["ttl"], "30m");
    let mut chat = build(Format::Chat, &responses::parse_request(&responses).unwrap());
    assert!(preserve_cache_hints(Format::Responses, Format::Chat, &responses, &mut chat).unwrap().is_none());
    assert_eq!(chat["prompt_cache_options"], body["prompt_cache_options"]);
    assert_eq!(chat["messages"], body["messages"]);
}

#[test]
fn extending_same_role_history_preserves_prior_openai_message_endings() {
    // A cached ending after A must still exist when the next request appends B.
    // Parsers previously joined both into one message before the target was known.
    for source in [Format::Chat, Format::Responses, Format::Claude, Format::Gemini] {
        for explicit in [false, true] {
            let blocks = |text: &str| {
                let mut block = json!({"type":"text","text":text});
                if explicit {
                    block["prompt_cache_breakpoint"] = json!({"mode":"explicit","ttl":"12h"});
                }
                json!([block])
            };
            let message = |text: &str| match source {
                Format::Gemini => json!({"role":"user","parts":[{"text":text}]}),
                _ => json!({"role":"user","content":blocks(text)}),
            };
            let field = match source {
                Format::Responses => "input",
                Format::Gemini => "contents",
                _ => "messages",
            };
            let initial = json!({field:[message("A")]});
            let extended = json!({field:[message("A"), message("B")]});
            let initial = parse_request(source, &initial).unwrap();
            let extended = parse_request(source, &extended).unwrap();
            for target in [Format::Chat, Format::Responses] {
                let initial = build(target, &initial);
                let extended = build(target, &extended);
                let field = if target == Format::Chat { "messages" } else { "input" };
                assert_eq!(extended[field].as_array().unwrap().len(), 2, "{source:?} -> {target:?}, {explicit}");
                assert_eq!(initial[field][0], extended[field][0], "{source:?} -> {target:?}, {explicit}");
            }
        }
    }
}

#[test]
fn leading_openai_instruction_roles_and_boundaries_survive_translation() {
    for source in [Format::Chat, Format::Responses] {
        let field = if source == Format::Chat { "messages" } else { "input" };
        let initial = json!({field:[{"role":"system","content":"A"}]});
        let extended = json!({field:[
            {"role":"system","content":"A"},
            {"role":"developer","content":"B"},
            {"role":"user","content":"question"}
        ]});
        for target in [Format::Chat, Format::Responses] {
            let initial = build(target, &parse_request(source, &initial).unwrap());
            let extended = build(target, &parse_request(source, &extended).unwrap());
            let field = if target == Format::Chat { "messages" } else { "input" };
            assert_eq!(initial[field][0], extended[field][0]);
            assert_eq!(extended[field][0]["role"], "system");
            assert_eq!(extended[field][1]["role"], "developer");
            assert_eq!(extended[field][2]["role"], "user");
        }
    }
}

#[test]
fn provider_required_turn_merging_still_orders_parallel_tool_results_first() {
    let body = json!({"input":[
        {"role":"user","content":"A"},
        {"role":"user","content":"B"},
        {"type":"function_call","call_id":"a","name":"first","arguments":"{}"},
        {"type":"function_call","call_id":"b","name":"second","arguments":"{}"},
        {"type":"function_call_output","call_id":"a","output":"first result"},
        {"type":"function_call_output","call_id":"b","output":"second result"},
        {"role":"user","content":"next question"}
    ]});
    let req = responses::parse_request(&body).unwrap();
    let chat = build(Format::Chat, &req);
    assert_eq!(chat["messages"].as_array().unwrap().len(), 6);
    assert_eq!(chat["messages"][2]["tool_calls"].as_array().unwrap().len(), 2);
    assert_eq!(chat["messages"][3]["tool_call_id"], "a");
    assert_eq!(chat["messages"][4]["tool_call_id"], "b");
    let claude = build(Format::Claude, &req);
    assert_eq!(claude["messages"].as_array().unwrap().len(), 3);
    assert_eq!(claude["messages"][2]["content"][0]["type"], "tool_result");
    assert_eq!(claude["messages"][2]["content"][1]["type"], "tool_result");
    assert_eq!(claude["messages"][2]["content"][2]["text"], "next question");
    let gemini = build(Format::Gemini, &req);
    assert_eq!(gemini["contents"].as_array().unwrap().len(), 3);
    assert!(gemini["contents"][2]["parts"][0]["functionResponse"].is_object());
    assert!(gemini["contents"][2]["parts"][1]["functionResponse"].is_object());
    assert_eq!(gemini["contents"][2]["parts"][2]["text"], "next question");
}

#[test]
fn claude_manual_thinking_recognizes_signed_reasoning_before_separate_call_items() {
    let body = json!({"reasoning":{"effort":"high"},"input":[
        {"role":"user","content":"question"},
        {"type":"reasoning","summary":[{"text":"thinking"}],"encrypted_content":"cpx-claude:signed"},
        {"role":"assistant","content":[{"type":"output_text","text":"checking"}]},
        {"type":"function_call","call_id":"a","name":"lookup","arguments":"{}"},
        {"type":"function_call_output","call_id":"a","output":"result"}
    ]});
    let req = responses::parse_request(&body).unwrap();
    let claude = claude::build_request(&req, "claude-sonnet-4-5");
    assert_eq!(claude["thinking"]["type"], "enabled");
    assert_eq!(claude["messages"][1]["content"][0]["type"], "thinking");
    assert_eq!(claude["messages"][1]["content"][1]["type"], "text");
    assert_eq!(claude["messages"][1]["content"][2]["type"], "tool_use");
    let chat = build(Format::Chat, &req);
    assert_eq!(chat["messages"].as_array().unwrap().len(), 3);
    assert_eq!(chat["messages"][1]["content"], "checking");
    assert_eq!(chat["messages"][1]["tool_calls"][0]["id"], "a");
}

#[test]
fn comparison_diagnostics_preserve_supported_cache_controls_and_report_downgrades() {
    let body = json!({
        "prompt_cache_options":{"mode":"explicit","ttl":"30m","comparison_response_id":"resp_previous"},
        "messages":[{"role":"user","content":[
            {"type":"text","text":"prefix","prompt_cache_breakpoint":{"mode":"explicit","ttl":"12h"}}
        ]}]
    });
    let req = chat::parse_request(&body).unwrap();
    let mut responses = build(Format::Responses, &req);
    assert!(preserve_cache_hints(Format::Chat, Format::Responses, &body, &mut responses).unwrap().is_none());
    assert_eq!(responses["prompt_cache_options"], body["prompt_cache_options"]);
    let req = responses::parse_request(&responses).unwrap();
    let mut chat = build(Format::Chat, &req);
    let downgrade = preserve_cache_hints(Format::Responses, Format::Chat, &responses, &mut chat).unwrap().unwrap();
    assert!(downgrade.contains("comparison_response_id"));
    assert_eq!(chat["prompt_cache_options"], json!({"mode":"explicit","ttl":"30m"}));
    assert_eq!(chat["messages"][0]["content"][0]["prompt_cache_breakpoint"]["ttl"], "12h");
}

#[test]
fn openai_cache_lifetime_usage_survives_full_and_stream_translation() {
    for (write_30m, write_12h) in [(Some(8), Some(2)), (Some(0), Some(10)), (Some(10), None), (None, None)] {
        let expected =
            Usage { input: 70, cache_read: 20, cache_write: 10, write_30m, write_12h, output: 4, ..Default::default() };
        let req = Request { include_usage: true, ..Default::default() };
        for target in [Format::Chat, Format::Responses] {
            let mut aggregate = Aggregate::default();
            aggregate.push(&Event::Usage(expected.clone()));
            aggregate.push(&Event::Finish(Finish::Stop));
            let full = render_full(target, &aggregate, "model", &req);
            let mut parsed = Aggregate::default();
            for event in full_to_events(target, &full) {
                parsed.push(&event);
            }
            assert_eq!(parsed.usage, expected, "full {target:?}");
            let mut frames = Vec::new();
            let mut renderer = renderer(target, "model", &req);
            renderer.push(&Event::Usage(expected.clone()), &mut frames);
            renderer.push(&Event::Finish(Finish::Stop), &mut frames);
            renderer.finish(&mut frames);
            let mut events = Vec::new();
            let mut parser = parser(target);
            for frame in frames {
                parser.feed(&SseEvent { event: frame.event.map(Cow::into_owned), data: frame.data }, &mut events);
            }
            let mut parsed = Aggregate::default();
            for event in events {
                parsed.push(&event);
            }
            assert_eq!(parsed.usage, expected, "stream {target:?}");
        }
    }
    // Anthropic's cache writes have different lifetime definitions.
    let anthropic = claude::usage_of(&json!({
        "cache_creation_input_tokens":10,
        "cache_creation":{"ephemeral_5m_input_tokens":8,"ephemeral_1h_input_tokens":2}
    }));
    assert_eq!((anthropic.write_30m, anthropic.write_12h), (None, None));
}

#[test]
fn raw_openai_cache_write_usage_prefers_canonical_counts_and_keeps_lifetimes() {
    let details = json!({
        "cached_tokens":20,"cache_write_tokens":10,"cache_creation_tokens":99,
        "cache_write_tokens_details":{"ttl_30m_tokens":0,"ttl_12h_tokens":10}
    });
    for source in [Format::Chat, Format::Responses] {
        let usage = if source == Format::Chat {
            json!({"prompt_tokens":100,"prompt_tokens_details":details,"completion_tokens":4})
        } else {
            json!({"input_tokens":100,"input_tokens_details":details,"output_tokens":4})
        };
        let body = if source == Format::Chat {
            json!({"choices":[{"message":{"content":"answer"},"finish_reason":"stop"}],"usage":usage})
        } else {
            json!({"status":"completed","output":[],"usage":usage})
        };
        let mut aggregate = Aggregate::default();
        for event in full_to_events(source, &body) {
            aggregate.push(&event);
        }
        assert_eq!(aggregate.usage.input, 70);
        assert_eq!(aggregate.usage.cache_write, 10);
        assert_eq!(aggregate.usage.write_30m, Some(0));
        assert_eq!(aggregate.usage.write_12h, Some(10));
    }
}

#[test]
fn newer_write_totals_do_not_inherit_stale_cache_lifetime_counts() {
    let mut usage = Usage { cache_write: 10, write_30m: Some(8), write_12h: Some(2), ..Default::default() };
    usage.merge(&Usage { cache_write: 20, ..Default::default() });
    assert_eq!((usage.write_30m, usage.write_12h), (None, None));
    usage.merge(&Usage { cache_write: 20, write_30m: Some(0), write_12h: Some(20), ..Default::default() });
    assert_eq!((usage.write_30m, usage.write_12h), (Some(0), Some(20)));
    // An older snapshot cannot overwrite the newer lifetime breakdown either.
    usage.merge(&Usage { cache_write: 10, write_30m: Some(10), write_12h: Some(0), ..Default::default() });
    assert_eq!((usage.write_30m, usage.write_12h), (Some(0), Some(20)));
}

#[test]
fn cache_lifetime_breakdowns_replace_equal_total_snapshots_atomically() {
    let mut usage = Usage { cache_write: 10, write_30m: Some(8), write_12h: Some(2), ..Default::default() };
    usage.merge(&Usage { cache_write: 10, write_30m: Some(0), write_12h: Some(10), ..Default::default() });
    assert_eq!((usage.write_30m, usage.write_12h), (Some(0), Some(10)));
    assert_eq!(usage.cache_write, 10);
    // A new partial breakdown must not inherit the other old lifetime bucket.
    usage.merge(&Usage { cache_write: 10, write_30m: Some(3), ..Default::default() });
    assert_eq!((usage.write_30m, usage.write_12h), (Some(3), None));
    // Output-only updates with an unchanged total do not erase known details.
    usage.merge(&Usage { cache_write: 10, output: 4, ..Default::default() });
    assert_eq!((usage.write_30m, usage.write_12h), (Some(3), None));
}
