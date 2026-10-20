//! Behavioral regressions for the independently reviewed optimization findings.
use crate::{
    accounts::{Credential, Pool},
    config::{Config, KeyEntry},
    formats,
    ir::*,
    upstream::{self, Target},
};
use axum::http::HeaderMap;
use serde_json::json;

#[test]
fn cache_detection_treats_schemas_and_arguments_as_opaque_but_keeps_nested_controls() {
    for body in [
        json!({"tools":[{"type":"function","function":{"name":"configure","parameters":{"properties":{"cache_control":{"type":"string"},"prompt_cache_breakpoint":{"type":"boolean"}}}}}]}),
        json!({"tools":[{"name":"configure","input_schema":{"properties":{"cache_control":{"type":"string"}}}}]}),
        json!({"tools":[{"functionDeclarations":[{"name":"configure","parametersJsonSchema":{"properties":{"cache_control":{"type":"string"}}}}]}]}),
        json!({"tools":[{"function_declarations":[{"name":"configure","parameters":{"properties":{"cache_control":{"type":"string"}}}}]}]}),
        json!({"messages":[{"tool_calls":[{"type":"function","function":{"name":"configure","arguments":{"cache_control":true}}}]}]}),
        json!({"contents":[{"parts":[{"functionCall":{"name":"configure","args":{"cache_control":true}}}]}]}),
        json!({"generationConfig":{"responseJsonSchema":{"properties":{"cache_control":{}}}}}),
        json!({"messages":[{"role":"assistant","content":[{"type":"tool_use","input":{"cache_control":true,"prompt_cache_breakpoint":{}}}]}]}),
        json!({"messages":[{"role":"assistant","content":[{"type":"server_tool_use","input":{"cache_control":true}}]}]}),
        json!({"text":{"format":{"schema":{"properties":{"cache_control":{}}}}}}),
        json!({"response_format":{"json_schema":{"schema":{"properties":{"cache_control":{}}}}}}),
        json!({"output_config":{"format":{"schema":{"properties":{"cache_control":{}}}}}}),
    ] {
        assert!(!crate::cache::has_markers(&body), "{body}");
        let mut translated = json!({});
        assert!(
            formats::preserve_cache_hints(Format::Claude, Format::Responses, &body, &mut translated).unwrap().is_none()
        );
    }
    for kind in ["tool_result", "unknown_future_block"] {
        let body = json!({"messages":[{"content":[{"type":kind,"content":[{"cache_control":{"type":"ephemeral"}}]}]}]});
        assert!(crate::cache::has_claude_controls(&body));
    }
}

#[test]
fn final_claude_preparation_honors_cache_intent_for_api_keys_and_oauth_cloak() {
    let cfg = Config {
        auth_dir: "/nonexistent".into(),
        claude_api_key: vec![KeyEntry { api_key: "test".into(), ..Default::default() }],
        ..Default::default()
    };
    let pool = Pool::default();
    pool.reload(&cfg);
    let acct = pool.all().remove(0);
    for oauth in [false, true] {
        if oauth {
            *acct.cred.write() = Credential::OAuth(crate::accounts::OAuth {
                access_token: "test".into(),
                refresh_token: String::new(),
                expires_at: None,
                email: None,
                account_id: None,
                base_url: None,
                project_id: None,
                raw: Default::default(),
            });
        }
        for defaults in [false, true] {
            for colliding in [false, true] {
                let body = json!({"model":"claude-sonnet-5-5","messages":[{"role":"user","content":"q"}],
                    "tools":if colliding {json!([{"name":"configure","input_schema":{"properties":{"cache_control":{"type":"string"}}}}])}else{json!([])}});
                let prepared = upstream::prepare(
                    &Target {
                        acct: &acct,
                        cfg: &cfg,
                        client_headers: &HeaderMap::new(),
                        model: "claude-sonnet-5-5",
                        wire: Format::Claude,
                        passthrough: false,
                        stream: true,
                        count_tokens: false,
                        cache_key: None,
                        cache_session: None,
                        cache_defaults: defaults,
                    },
                    body,
                );
                assert_eq!(
                    crate::cache::has_claude_controls(&prepared.body),
                    defaults,
                    "oauth={oauth} collision={colliding}"
                );
            }
        }
    }
}

#[test]
fn codex_shared_cache_keys_keep_sessions_separate_and_preserve_caller_overrides() {
    let cfg = Config {
        auth_dir: "/nonexistent".into(),
        codex_api_key: vec![KeyEntry { api_key: "test".into(), ..Default::default() }],
        ..Default::default()
    };
    let pool = Pool::default();
    pool.reload(&cfg);
    let acct = pool.all().remove(0);
    *acct.cred.write() = Credential::OAuth(crate::accounts::OAuth {
        access_token: "test".into(),
        refresh_token: String::new(),
        expires_at: None,
        email: None,
        account_id: None,
        base_url: None,
        project_id: None,
        raw: Default::default(),
    });
    let mut headers = HeaderMap::new();
    headers.insert("x-fusebox-cache-workspace", "project".parse().unwrap());
    for scope in [crate::cache::Scope::Client, crate::cache::Scope::Workspace] {
        let cache_cfg = crate::cache::Config { scope, ..Default::default() };
        let prepare = |session: &str, headers: &HeaderMap, body: serde_json::Value| {
            let (key, scope) = crate::cache::key(headers, &body, Some(session), Some("header"), &cache_cfg);
            upstream::prepare(
                &Target {
                    acct: &acct,
                    cfg: &cfg,
                    client_headers: headers,
                    model: "gpt-6.1-sol",
                    wire: Format::Responses,
                    passthrough: true,
                    stream: true,
                    count_tokens: false,
                    cache_key: key.as_deref(),
                    cache_session: matches!(scope, "client" | "workspace").then_some(session),
                    cache_defaults: true,
                },
                body,
            )
        };
        let first = prepare("one", &headers, json!({"input":"q"}));
        let second = prepare("two", &headers, json!({"input":"q"}));
        assert_eq!(first.body["prompt_cache_key"], second.body["prompt_cache_key"]);
        let session_header = |p: &upstream::Prepared| {
            p.headers.iter().find(|(k, _)| k == "session_id" || k == "session-id").unwrap().1.clone()
        };
        assert_ne!(session_header(&first), session_header(&second));
        for name in ["session_id", "session-id"] {
            let mut custom = headers.clone();
            custom.insert(name, "caller-session".parse().unwrap());
            assert_eq!(session_header(&prepare("one", &custom, json!({"input":"q"}))), "caller-session");
        }
        let caller = prepare("one", &headers, json!({"input":"q","prompt_cache_key":"caller-key"}));
        assert_eq!(caller.body["prompt_cache_key"], "caller-key");
        assert_eq!(session_header(&caller), "caller-key");
    }
}

#[test]
fn late_privileged_instructions_and_cache_boundaries_keep_their_positions() {
    let original = json!({"messages":[{"role":"system","content":"stable"},{"role":"user","content":"question"},
        {"role":"assistant","content":"answer"},{"role":"developer","content":[{"type":"text","text":"late","prompt_cache_breakpoint":{"mode":"explicit"}}]},
        {"role":"user","content":"next"}],"prompt_cache_options":{"mode":"explicit"},"verbosity":"low","service_tier":"flex"});
    let req = formats::chat::parse_request(&original).unwrap();
    let mut output = formats::responses::build_request(
        &req,
        "gpt-6.1-sol",
        &formats::responses::BuildOpts { chatgpt_backend: true, custom_tools: true, default_reasoning: true },
    );
    assert!(formats::preserve_cache_hints(Format::Chat, Format::Responses, &original, &mut output).unwrap().is_none());
    let input = output["input"].as_array().unwrap();
    assert_eq!(
        input.iter().map(|m| m["role"].as_str().unwrap()).collect::<Vec<_>>(),
        vec!["developer", "user", "assistant", "developer", "user"]
    );
    assert_eq!(input[3]["content"][0]["prompt_cache_breakpoint"]["mode"], "explicit");
    assert_eq!(output["text"]["verbosity"], "low");
    assert_eq!(output["service_tier"], "flex");
    assert!(formats::generation_controls(&mut output, Format::Responses, true, true).is_empty());
    assert_eq!(
        formats::generation_controls(&mut output, Format::Responses, true, false),
        vec!["service_tier_not_enabled"]
    );
    assert_eq!(
        formats::generation_controls(&mut output, Format::Responses, false, false),
        vec!["verbosity_unsupported"]
    );
    let back = formats::chat::build_request(&formats::responses::parse_request(&output).unwrap(), "gpt-6.1-sol");
    assert_eq!(back["messages"][3]["role"], "developer");
}

#[test]
fn responses_usage_uses_canonical_cache_write_field_without_double_counting() {
    let mut agg = Aggregate::default();
    agg.usage = Usage { input: 70, cache_read: 20, cache_write: 10, output: 40, reasoning: 30 };
    let v = formats::responses::render_full(&agg, "model", &Request::default());
    assert_eq!(v["usage"]["input_tokens_details"]["cache_write_tokens"], 10);
    // The legacy alias repeats the same count rather than adding to it.
    assert_eq!(v["usage"]["input_tokens_details"]["cache_creation_tokens"], 10);
    assert_eq!(v["usage"]["total_tokens"], 140);
    // Fusebox's own parser reads either name once.
    assert_eq!(formats::responses::usage_of(&v["usage"]).unwrap().cache_write, 10);
}

#[test]
fn sse_limits_bound_partial_lines_and_multiline_events_and_keep_prior_usage() {
    let mut d = crate::sse::SseDecoder::with_limit(32);
    let prior = d.push(b"data: usage\n\ndata: abcdefghijkl\ndata: mnopqrstuvwxyz0123456789\n");
    assert_eq!(prior.len(), 1);
    assert_eq!(prior[0].data, "usage");
    assert!(d.failed());
    assert!(d.finish().is_empty());
    assert!(d.push(b"data: after\n\n").is_empty());
    let mut partial = crate::sse::SseDecoder::with_limit(32);
    for _ in 0..10 {
        partial.push(b"abcdefgh");
    }
    assert!(partial.failed());
    let mut many = crate::sse::SseDecoder::with_limit(32);
    assert_eq!(many.push(&b"data: x\n\n".repeat(100)).len(), 100);
    assert!(!many.failed());
}

#[test]
fn history_branches_share_prefixes_without_mutating_their_parent() {
    let base = crate::affinity::History::from(vec![json!({"text":"large".repeat(1000)})]);
    let mut left = base.clone();
    left.extend(vec![json!("left")]);
    let mut right = base.clone();
    right.extend(vec![json!("right")]);
    assert_eq!(base.len(), 1);
    assert_eq!(left.to_values()[1], "left");
    assert_eq!(right.to_values()[1], "right");
}

#[test]
fn latency_ties_require_comparable_samples_and_do_not_change_pinned_accounts() {
    let cfg = Config {
        auth_dir: "/nonexistent".into(),
        latency_routing: true,
        routing: crate::config::Routing::SmartQuota,
        codex_api_key: ["a", "b"].map(|key| KeyEntry { api_key: key.into(), ..Default::default() }).to_vec(),
        ..Default::default()
    };
    let pool = Pool::default();
    pool.reload(&cfg);
    let accounts = pool.all();
    let model = "gpt-6.1-sol";
    let context = crate::latency::Context::new(&json!({"input":"q"}), "http");
    for (a, ms) in accounts.iter().zip([100, 10]) {
        let mut sample = crate::latency::Sample::new(ms);
        sample.add(ms);
        sample.add(ms);
        a.state.lock().latency.insert((model.into(), context.clone()), sample);
    }
    let empty = std::collections::HashMap::new();
    for _ in 0..3 {
        let crate::accounts::Pick::Ok(a, _) =
            pool.pick_with_context(model, &[], &cfg, None, None, &empty, Some(&context))
        else {
            panic!()
        };
        assert_eq!(a.id, accounts[1].id);
    }
    let crate::accounts::Pick::Ok(a, _) =
        pool.pick_with_context(model, &[], &cfg, Some(&accounts[0].id), None, &empty, Some(&context))
    else {
        panic!()
    };
    assert_eq!(a.id, accounts[0].id);
    let different = crate::latency::Context::new(&json!({"input":"q","reasoning":{"effort":"high"}}), "http");
    let picks: Vec<_> = (0..2)
        .map(|_| match pool.pick_with_context(model, &[], &cfg, None, None, &empty, Some(&different)) {
            crate::accounts::Pick::Ok(a, _) => a.id.clone(),
            _ => panic!(),
        })
        .collect();
    assert_ne!(picks[0], picks[1]);
}

#[test]
fn late_instructions_join_the_system_prompt_on_targets_without_positions() {
    let plain = formats::chat::parse_request(&json!({"messages":[{"role":"user","content":"q"}]})).unwrap();
    assert!(formats::hoist_instructions(&plain).is_none());
    let original = json!({"messages":[{"role":"system","content":"stable"},{"role":"user","content":"question"},
        {"role":"assistant","content":"answer"},{"role":"developer","content":[{"type":"text","text":"late"}]},
        {"role":"user","content":"next"}]});
    let req = formats::chat::parse_request(&original).unwrap();
    let hoisted = formats::hoist_instructions(&req).unwrap();
    // Leading instructions first, then late ones in order, as before positions were kept.
    assert_eq!(hoisted.system, vec!["stable", "late"]);
    assert!(hoisted.messages.iter().all(|m| !matches!(m.role, crate::ir::Role::System | crate::ir::Role::Developer)));
    assert!(req.messages.iter().any(|m| m.role == crate::ir::Role::Developer), "the original keeps its positions");
    let claude = formats::claude::build_request(&hoisted, "claude-sonnet-5-5");
    assert!(claude["system"].to_string().contains("late"));
    assert!(!claude["messages"].to_string().contains("late"), "late instructions are not demoted to a user turn");
    let gemini = formats::gemini::build_request(&hoisted, "gemini-3.8-flash");
    assert!(!gemini["contents"].to_string().contains("late"));
}

#[test]
fn prepared_request_stamps_change_with_custom_account_headers() {
    let stamp_with = |headers: &[(&str, &str)]| {
        let cfg = Config {
            auth_dir: "/nonexistent".into(),
            codex_api_key: vec![crate::config::KeyEntry {
                api_key: "key".into(),
                headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let pool = Pool::default();
        pool.reload(&cfg);
        let acct = pool.all().remove(0);
        (upstream::credential_stamp(&acct), upstream::backend_stamp(&acct))
    };
    let plain = stamp_with(&[]);
    assert_eq!(plain, stamp_with(&[]));
    let routed = stamp_with(&[("x-backend", "eu")]);
    assert_ne!(plain.0, routed.0);
    assert_ne!(plain.1, routed.1);
    assert_ne!(routed, stamp_with(&[("x-backend", "us")]));
}
