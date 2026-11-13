//! Google Gemini generateContent / streamGenerateContent.

use std::collections::{HashMap, VecDeque};

use serde_json::{Value, json};

use super::{Frame, StreamParser, StreamRenderer};
use crate::ir::*;
use crate::sse::SseEvent;

/// Accepted by Gemini in place of a real thought signature on replayed calls.
const SKIP_SIG: &str = "skip_thought_signature_validator";

fn g<'a>(v: &'a Value, camel: &str, snake: &str) -> &'a Value {
    match v.get(camel) {
        Some(x) if !x.is_null() => x,
        _ => v.get(snake).unwrap_or(&Value::Null),
    }
}

// ------------------------------------------------------------------ request in

pub fn parse_request(v: &Value) -> Result<Request, String> {
    let mut req = Request::default();
    for p in g(v, "systemInstruction", "system_instruction")["parts"].as_array().into_iter().flatten() {
        if let Some(t) = p["text"].as_str().filter(|t| !t.is_empty()) {
            req.system.push(t.to_string());
        }
    }

    let gc = g(v, "generationConfig", "generation_config");
    req.max_tokens = g(gc, "maxOutputTokens", "max_output_tokens").as_u64();
    req.temperature = gc["temperature"].as_f64();
    req.top_p = g(gc, "topP", "top_p").as_f64();
    req.stop = g(gc, "stopSequences", "stop_sequences")
        .as_array()
        .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let tc = g(gc, "thinkingConfig", "thinking_config");
    if tc.is_object() {
        let budget = g(tc, "thinkingBudget", "thinking_budget").as_i64();
        let level = g(tc, "thinkingLevel", "thinking_level").as_str().map(|s| s.to_ascii_lowercase());
        req.reasoning = Some(Reasoning {
            effort: level,
            budget: budget.filter(|b| *b > 0).map(|b| b as u64),
            disabled: budget == Some(0),
        });
    }
    if g(gc, "responseMimeType", "response_mime_type").as_str() == Some("application/json") {
        let json_schema = g(gc, "responseJsonSchema", "response_json_schema");
        // `responseSchema` is the OpenAPI-style Schema; `responseJsonSchema` is JSON Schema.
        let schema = if json_schema.is_null() {
            lower_schema_types(g(gc, "responseSchema", "response_schema"))
        } else {
            json_schema.clone()
        };
        req.response_format = Some(if schema.is_object() {
            ResponseFormat::JsonSchema { name: "response".into(), schema, strict: false }
        } else {
            ResponseFormat::JsonObject
        });
    }

    let mut pending: HashMap<String, VecDeque<String>> = HashMap::new();
    let mut counter = 0usize;
    for c in v["contents"].as_array().into_iter().flatten() {
        // Gemini lets a turn omit its role; one that calls a function is the model's.
        let calls =
            || c["parts"].as_array().into_iter().flatten().any(|p| g(p, "functionCall", "function_call").is_object());
        let role = match c["role"].as_str() {
            Some("model") => Role::Assistant,
            None | Some("") if calls() => Role::Assistant,
            _ => Role::User,
        };
        let mut parts = Vec::new();
        for p in c["parts"].as_array().into_iter().flatten() {
            let sig =
                g(p, "thoughtSignature", "thought_signature").as_str().filter(|s| !s.is_empty() && *s != SKIP_SIG);
            let sig = sig.map(|s| Sig::Gemini(s.to_string()));
            if let Some(t) = p["text"].as_str() {
                if p["thought"].as_bool() == Some(true) {
                    parts.push(Part::Reasoning { text: t.to_string(), sig });
                } else {
                    parts.push(Part::Text(t.to_string()));
                }
                continue;
            }
            let inline = g(p, "inlineData", "inline_data");
            if inline.is_object() {
                parts.push(Part::Image(Image::Base64 {
                    mime: g(inline, "mimeType", "mime_type").as_str().unwrap_or("image/png").to_string(),
                    data: inline["data"].as_str().unwrap_or_default().to_string(),
                }));
                continue;
            }
            let file = g(p, "fileData", "file_data");
            if let Some(uri) = g(file, "fileUri", "file_uri").as_str() {
                parts.push(Part::Image(Image::Url(uri.to_string())));
                continue;
            }
            let fc = g(p, "functionCall", "function_call");
            if fc.is_object() {
                let name = fc["name"].as_str().unwrap_or_default().to_string();
                let id = fc["id"].as_str().map(String::from).unwrap_or_else(|| {
                    counter += 1;
                    format!("call_{name}_{counter}")
                });
                pending.entry(name.clone()).or_default().push_back(id.clone());
                parts.push(Part::ToolCall { id, name, args: fc["args"].to_string(), sig });
                continue;
            }
            let fr = g(p, "functionResponse", "function_response");
            if fr.is_object() {
                let name = fr["name"].as_str().unwrap_or_default().to_string();
                let id = fr["id"]
                    .as_str()
                    .map(String::from)
                    .or_else(|| pending.get_mut(&name).and_then(|q| q.pop_front()))
                    .unwrap_or_else(|| format!("call_{name}"));
                let resp = &fr["response"];
                let text = match resp.get("result").or_else(|| resp.get("output")).or_else(|| resp.get("content")) {
                    Some(Value::String(s)) => s.clone(),
                    _ => resp.to_string(),
                };
                parts.push(Part::ToolResult {
                    id,
                    name: Some(name),
                    content: vec![Part::Text(text)],
                    is_error: resp.get("error").is_some(),
                });
            }
        }
        req.messages.push(Message { role, parts });
    }
    for t in v["tools"].as_array().into_iter().flatten() {
        for f in g(t, "functionDeclarations", "function_declarations").as_array().into_iter().flatten() {
            let params = g(f, "parametersJsonSchema", "parameters_json_schema");
            let params = if params.is_object() { params.clone() } else { lower_schema_types(&f["parameters"]) };
            req.tools.push(Tool {
                name: f["name"].as_str().unwrap_or_default().to_string(),
                description: f["description"].as_str().unwrap_or_default().to_string(),
                parameters: super::chat::params_or_empty(&params),
            });
        }
    }
    let fcc = &g(v, "toolConfig", "tool_config");
    let fcc = g(fcc, "functionCallingConfig", "function_calling_config");
    req.tool_choice = match fcc["mode"].as_str() {
        Some("NONE") => ToolChoice::None,
        Some("ANY") => {
            match g(fcc, "allowedFunctionNames", "allowed_function_names").as_array().map(|a| a.as_slice()) {
                Some([one]) => ToolChoice::Tool(one.as_str().unwrap_or_default().to_string()),
                _ => ToolChoice::Required,
            }
        }
        _ => ToolChoice::Auto,
    };
    Ok(req)
}

/// Schema limits that Gemini types as int64, which its JSON sends as strings, and
/// `minimum`/`maximum`, which clients may quote too.
const NUMERIC_LIMITS: &[&str] =
    &["minItems", "maxItems", "minLength", "maxLength", "minProperties", "maxProperties", "minimum", "maximum"];

/// Gemini's OpenAPI-style schemas use upper-case type names ("OBJECT") and send
/// integer limits as strings (`"maxLength": "10"`). Rewrites both at schema keyword
/// positions only, so property names and literal values (`enum`, `default`) stay.
fn lower_schema_types(v: &Value) -> Value {
    let Value::Object(m) = v else { return v.clone() };
    let each = |a: &Vec<Value>| Value::Array(a.iter().map(lower_schema_types).collect());
    let named = |o: &serde_json::Map<String, Value>| {
        Value::Object(o.iter().map(|(name, s)| (name.clone(), lower_schema_types(s))).collect())
    };
    Value::Object(
        m.iter()
            .map(|(k, val)| {
                let val = match (k.as_str(), val) {
                    ("type", Value::String(s)) => Value::String(s.to_ascii_lowercase()),
                    (k, Value::String(s)) if NUMERIC_LIMITS.contains(&k) => numeric(s).unwrap_or_else(|| val.clone()),
                    ("properties" | "patternProperties" | "$defs" | "definitions", Value::Object(o)) => named(o),
                    ("items" | "additionalProperties" | "not", Value::Object(_)) => lower_schema_types(val),
                    ("items" | "prefixItems" | "anyOf" | "oneOf" | "allOf", Value::Array(a)) => each(a),
                    _ => val.clone(),
                };
                (k.clone(), val)
            })
            .collect(),
    )
}

fn numeric(s: &str) -> Option<Value> {
    let s = s.trim();
    s.parse::<i64>()
        .map(Value::from)
        .ok()
        .or_else(|| s.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number))
}

// ----------------------------------------------------------------- request out

fn strip_schema(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter().filter(|(k, _)| k.as_str() != "$schema").map(|(k, val)| (k.clone(), strip_schema(val))).collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(strip_schema).collect()),
        other => other.clone(),
    }
}

/// Share effort and budget normalization between translated requests and native
/// model suffixes, so `none` and `medium` cannot turn into high effort in one path.
pub fn thinking_config(reasoning: Option<&Reasoning>, model: &str) -> Value {
    let uses_levels = model.starts_with("gemini-3") || model.starts_with("gemini-4");
    match reasoning {
        Some(r) if r.disabled => {
            if uses_levels {
                json!({ "thinkingLevel": "low" })
            } else {
                json!({ "thinkingBudget": 0 })
            }
        }
        Some(r) if uses_levels => {
            let level = match r.effort_level().as_deref() {
                Some("minimal") | Some("low") => "low",
                Some("medium") => "medium",
                _ => "high",
            };
            json!({ "includeThoughts": true, "thinkingLevel": level })
        }
        Some(r) => match r.budget_tokens() {
            Some(b) => json!({ "includeThoughts": true, "thinkingBudget": b.min(32_768) }),
            None => json!({ "includeThoughts": true }),
        },
        None => json!({ "includeThoughts": true }),
    }
}

/// Google rejects an empty system part (`{}`, "required oneof field 'data'"). Removes
/// such parts, and the instruction when none remain; other bodies stay as they are.
pub fn drop_empty_system_parts(body: &mut Value) {
    let Some(object) = body.as_object_mut() else { return };
    for key in ["systemInstruction", "system_instruction"] {
        let Some(parts) = object.get_mut(key).and_then(|s| s.get_mut("parts")).and_then(Value::as_array_mut) else {
            continue;
        };
        let before = parts.len();
        parts.retain(|p| p.as_object().is_none_or(|p| !p.is_empty()));
        if parts.is_empty() && before > 0 {
            object.remove(key);
        }
    }
}

fn image_model(model: &str) -> bool {
    model.contains("-image") || model.starts_with("gemini-nano-banana-")
}

pub fn apply_native_reasoning(body: &mut Value, reasoning: &Reasoning, model: &str) {
    let Some(object) = body.as_object_mut() else { return };
    let legacy = object.remove("generation_config").unwrap_or_else(|| json!({}));
    let config = object.entry("generationConfig").or_insert(legacy);
    if !config.is_object() {
        *config = json!({});
    }
    let old = g(config, "thinkingConfig", "thinking_config");
    let include_thoughts = g(old, "includeThoughts", "include_thoughts").as_bool();
    let mut thinking = thinking_config(Some(reasoning), model);
    // A suffix controls effort, not whether the caller wants thought summaries.
    if let Some(include) = include_thoughts {
        thinking["includeThoughts"] = include.into();
    } else if let Some(object) = thinking.as_object_mut() {
        object.remove("includeThoughts");
    }
    let config = config.as_object_mut().unwrap();
    config.remove("thinking_config");
    if image_model(model) {
        config.remove("thinkingConfig");
    } else {
        config.insert("thinkingConfig".into(), thinking);
    }
}

pub fn build_request(req: &Request, model: &str) -> Value {
    let mut names: HashMap<String, String> = HashMap::new();
    let mut contents = Vec::new();
    for m in super::alternating_messages(&req.messages).iter() {
        let mut parts = Vec::new();
        for p in &m.parts {
            match p {
                Part::Text(t) if !t.is_empty() => parts.push(json!({ "text": t })),
                Part::Image(Image::Base64 { mime, data }) => {
                    parts.push(json!({ "inlineData": { "mimeType": mime, "data": data } }))
                }
                Part::Image(Image::Url(u)) => {
                    parts.push(json!({ "fileData": { "mimeType": "image/*", "fileUri": u } }))
                }
                Part::Reasoning { text, sig: Some(Sig::Gemini(s)) } if m.role == Role::Assistant => {
                    parts.push(json!({ "text": text, "thought": true, "thoughtSignature": s }))
                }
                Part::ToolCall { id, name, args, sig } => {
                    names.insert(id.clone(), name.clone());
                    let s = match sig {
                        Some(Sig::Gemini(s)) => s.as_str(),
                        _ => SKIP_SIG,
                    };
                    parts.push(
                        json!({ "functionCall": { "name": name, "args": parse_args(args) }, "thoughtSignature": s }),
                    );
                }
                Part::ToolResult { id, name, content, is_error } => {
                    let name = name.clone().or_else(|| names.get(id).cloned()).unwrap_or_else(|| id.clone());
                    let text = parts_text(content);
                    let response = if *is_error { json!({ "error": text }) } else { json!({ "result": text }) };
                    parts.push(json!({ "functionResponse": { "name": name, "response": response } }));
                }
                _ => {}
            }
        }
        if parts.is_empty() {
            continue;
        }
        let role = if m.role == Role::Assistant { "model" } else { "user" };
        contents.push(json!({ "role": role, "parts": parts }));
    }

    let mut out = json!({ "contents": contents });
    let o = out.as_object_mut().unwrap();
    if !req.system.is_empty() {
        o.insert("systemInstruction".into(), json!({ "parts": [{ "text": req.system.join("\n\n") }] }));
    }
    if !req.tools.is_empty() {
        let decls: Vec<Value> = req
            .tools
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "parametersJsonSchema": strip_schema(&t.parameters) }))
            .collect();
        o.insert("tools".into(), json!([{ "functionDeclarations": decls }]));
        let cfg = match &req.tool_choice {
            ToolChoice::Auto => json!({ "mode": "AUTO" }),
            ToolChoice::None => json!({ "mode": "NONE" }),
            ToolChoice::Required => json!({ "mode": "ANY" }),
            ToolChoice::Tool(n) => json!({ "mode": "ANY", "allowedFunctionNames": [n] }),
        };
        o.insert("toolConfig".into(), json!({ "functionCallingConfig": cfg }));
    }

    let mut gc = serde_json::Map::new();
    if let Some(m) = req.max_tokens {
        gc.insert("maxOutputTokens".into(), m.into());
    }
    if let Some(t) = req.temperature {
        gc.insert("temperature".into(), t.into());
    }
    if let Some(t) = req.top_p {
        gc.insert("topP".into(), t.into());
    }
    if !req.stop.is_empty() {
        gc.insert("stopSequences".into(), req.stop.clone().into());
    }
    if image_model(model) {
        // Image models answer with pictures and don't take thinking settings.
        gc.insert("responseModalities".into(), json!(["TEXT", "IMAGE"]));
    } else {
        gc.insert("thinkingConfig".into(), thinking_config(req.reasoning.as_ref(), model));
    }
    match &req.response_format {
        Some(ResponseFormat::JsonObject) => {
            gc.insert("responseMimeType".into(), "application/json".into());
        }
        Some(ResponseFormat::JsonSchema { schema, .. }) => {
            gc.insert("responseMimeType".into(), "application/json".into());
            gc.insert("responseJsonSchema".into(), strip_schema(schema));
        }
        None => {}
    }
    o.insert("generationConfig".into(), Value::Object(gc));
    out
}

// --------------------------------------------------------------- stream parser

#[derive(Default)]
pub struct Parser {
    started: bool,
    tools: usize,
}

fn finish_of(s: &str) -> Finish {
    match s {
        "MAX_TOKENS" => Finish::Length,
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY" => Finish::Filter,
        _ => Finish::Stop,
    }
}

fn usage_of(u: &Value) -> Option<Usage> {
    if !u.is_object() {
        return None;
    }
    let prompt = u["promptTokenCount"].as_u64().unwrap_or(0);
    let cached = u["cachedContentTokenCount"].as_u64().unwrap_or(0);
    let thoughts = u["thoughtsTokenCount"].as_u64().unwrap_or(0);
    Some(Usage {
        input: prompt.saturating_sub(cached),
        cache_read: cached,
        output: u["candidatesTokenCount"].as_u64().unwrap_or(0) + thoughts,
        reasoning: thoughts,
        cache_write: 0,
        ..Default::default()
    })
}

impl Parser {
    fn chunk(&mut self, v: &Value, out: &mut Vec<Event>) {
        if let Some(e) = v.get("error") {
            out.push(Event::Error {
                status: e["code"].as_u64().unwrap_or(500) as u16,
                message: e["message"].as_str().unwrap_or("upstream error").to_string(),
            });
            return;
        }
        if !self.started {
            self.started = true;
            out.push(Event::Start {
                id: v["responseId"].as_str().map(String::from),
                model: v["modelVersion"].as_str().map(String::from),
            });
        }
        let cand = &v["candidates"][0];
        for p in cand["content"]["parts"].as_array().into_iter().flatten() {
            let sig = p["thoughtSignature"].as_str().filter(|s| !s.is_empty()).map(|s| Sig::Gemini(s.to_string()));
            if let Some(fc) = p.get("functionCall") {
                let key = self.tools;
                self.tools += 1;
                out.push(Event::ToolStart {
                    key,
                    id: fc["id"].as_str().map(String::from).unwrap_or_else(|| new_id("call_")),
                    name: fc["name"].as_str().unwrap_or_default().to_string(),
                });
                out.push(Event::ToolArgs { key, delta: fc["args"].to_string() });
                if let Some(s) = sig {
                    out.push(Event::ToolSig { key, sig: s });
                }
            } else if let Some(img) = p.get("inlineData").or_else(|| p.get("inline_data")) {
                if let Some(data) = img["data"].as_str() {
                    let mime = g(img, "mimeType", "mime_type").as_str().unwrap_or("image/png").to_string();
                    out.push(Event::Image { mime, data: data.to_string() });
                }
            } else if let Some(t) = p["text"].as_str() {
                if p["thought"].as_bool() == Some(true) {
                    if !t.is_empty() {
                        out.push(Event::Reasoning(t.to_string()));
                    }
                    if let Some(s) = sig {
                        out.push(Event::ReasoningSig(s));
                    }
                } else if !t.is_empty() {
                    out.push(Event::Text(t.to_string()));
                }
            }
        }
        if let Some(u) = usage_of(&v["usageMetadata"]) {
            out.push(Event::Usage(u));
        }
        if let Some(f) = cand["finishReason"].as_str() {
            out.push(Event::Finish(finish_of(f)));
        } else if g(g(v, "promptFeedback", "prompt_feedback"), "blockReason", "block_reason")
            .as_str()
            .is_some_and(|reason| !reason.is_empty() && reason != "BLOCK_REASON_UNSPECIFIED")
        {
            // A blocked prompt legitimately has no candidate or finishReason.
            out.push(Event::Finish(Finish::Filter));
        }
    }
}

impl StreamParser for Parser {
    fn feed_value(&mut self, _ev: &SseEvent, v: Option<&Value>, out: &mut Vec<Event>) {
        let Some(v) = v else { return };
        // Cloud Code (Antigravity) wraps every chunk in {"response": ...}.
        match v {
            Value::Object(o) if o.get("response").is_some_and(Value::is_object) && !o.contains_key("candidates") => {
                self.chunk(&o["response"], out)
            }
            Value::Array(_) => self.chunk(&crate::antigravity::unwrap(v.clone()), out),
            _ => self.chunk(v, out),
        }
    }
}

pub fn full_to_events(v: &Value) -> Vec<Event> {
    let mut p = Parser::default();
    let mut out = Vec::new();
    let v = &crate::antigravity::unwrap(v.clone());
    match v {
        Value::Array(chunks) => chunks.iter().for_each(|c| p.chunk(c, &mut out)),
        _ => p.chunk(v, &mut out),
    }
    out
}

// ------------------------------------------------------------- stream renderer

pub struct Renderer {
    id: String,
    model: String,
    tool: Option<(usize, String, String, String, Option<String>)>,
    usage: Usage,
    finish: Option<Finish>,
    errored: bool,
}

impl Renderer {
    pub fn new(model: &str) -> Self {
        Self {
            id: new_id(""),
            model: model.to_string(),
            tool: None,
            usage: Usage::default(),
            finish: None,
            errored: false,
        }
    }

    fn chunk(&self, parts: Vec<Value>) -> Frame {
        Frame::data(
            json!({
                "candidates": [{ "content": { "role": "model", "parts": parts }, "index": 0 }],
                "modelVersion": self.model, "responseId": self.id
            })
            .to_string(),
        )
    }

    fn flush_tool(&mut self, out: &mut Vec<Frame>) {
        if let Some((_, id, name, args, sig)) = self.tool.take() {
            let mut part = json!({ "functionCall": { "id": id, "name": name, "args": parse_args(&args) } });
            if let Some(s) = sig {
                part["thoughtSignature"] = s.into();
            }
            out.push(self.chunk(vec![part]));
        }
    }
}

fn finish_str(f: Finish) -> &'static str {
    match f {
        Finish::Length => "MAX_TOKENS",
        Finish::Filter => "SAFETY",
        _ => "STOP",
    }
}

fn usage_json(u: &Usage) -> Value {
    json!({
        "promptTokenCount": u.prompt_total(),
        "candidatesTokenCount": u.output.saturating_sub(u.reasoning),
        "thoughtsTokenCount": u.reasoning,
        "cachedContentTokenCount": u.cache_read,
        "totalTokenCount": u.prompt_total() + u.output,
    })
}

impl StreamRenderer for Renderer {
    fn push(&mut self, ev: &Event, out: &mut Vec<Frame>) {
        match ev {
            Event::ToolArgs { key, delta } => {
                if let Some((k, _, _, args, _)) = &mut self.tool
                    && k == key
                {
                    args.push_str(delta);
                }
                return;
            }
            Event::ToolSig { key, sig: Sig::Gemini(s) } => {
                if let Some((k, _, _, _, sig)) = &mut self.tool
                    && k == key
                {
                    *sig = Some(s.clone());
                }
                return;
            }
            Event::Usage(u) => return self.usage.merge(u),
            Event::Finish(f) => return self.finish = Some(*f),
            Event::ReasoningStart => return,
            Event::Error { status, message } => {
                self.tool = None;
                self.errored = true;
                out.push(Frame::data(super::error_body(Format::Gemini, *status, message).to_string()));
                return;
            }
            _ => {}
        }
        self.flush_tool(out);
        match ev {
            Event::Text(t) => out.push(self.chunk(vec![json!({ "text": t })])),
            Event::Reasoning(t) => out.push(self.chunk(vec![json!({ "text": t, "thought": true })])),
            Event::Image { mime, data } => {
                out.push(self.chunk(vec![json!({ "inlineData": { "mimeType": mime, "data": data } })]))
            }
            Event::ReasoningSig(Sig::Gemini(s)) => {
                out.push(self.chunk(vec![json!({ "text": "", "thought": true, "thoughtSignature": s })]))
            }
            Event::ToolStart { key, id, name } => {
                self.tool = Some((*key, id.clone(), name.clone(), String::new(), None))
            }
            _ => {}
        }
    }

    fn finish(&mut self, out: &mut Vec<Frame>) {
        if self.errored {
            return;
        }
        self.flush_tool(out);
        out.push(Frame::data(
            json!({
                "candidates": [{
                    "content": { "role": "model", "parts": [{ "text": "" }] },
                    "finishReason": finish_str(self.finish.unwrap_or_default()), "index": 0
                }],
                "usageMetadata": usage_json(&self.usage),
                "modelVersion": self.model, "responseId": self.id
            })
            .to_string(),
        ));
    }
}

pub fn render_full(agg: &Aggregate, model: &str) -> Value {
    let mut parts = Vec::new();
    for p in &agg.parts {
        match p {
            Part::Text(t) => parts.push(json!({ "text": t })),
            Part::Image(Image::Base64 { mime, data }) => {
                parts.push(json!({ "inlineData": { "mimeType": mime, "data": data } }))
            }
            Part::Reasoning { text, sig } => {
                let mut part = json!({ "text": text, "thought": true });
                if let Some(Sig::Gemini(s)) = sig {
                    part["thoughtSignature"] = s.as_str().into();
                }
                parts.push(part);
            }
            Part::ToolCall { id, name, args, sig } => {
                let mut part = json!({ "functionCall": { "id": id, "name": name, "args": parse_args(args) } });
                if let Some(Sig::Gemini(s)) = sig {
                    part["thoughtSignature"] = s.as_str().into();
                }
                parts.push(part);
            }
            _ => {}
        }
    }
    json!({
        "candidates": [{
            "content": { "role": "model", "parts": parts },
            "finishReason": finish_str(agg.finish.unwrap_or_default()), "index": 0
        }],
        "usageMetadata": usage_json(&agg.usage),
        "modelVersion": model,
        "responseId": new_id(""),
    })
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderMap;

    use super::*;
    use crate::accounts::Provider;
    use crate::config::{Config, KeyEntry};

    #[test]
    fn empty_system_parts_are_removed_before_every_google_upstream() {
        let dir = std::env::temp_dir().join(format!("fusebox-gemini-parts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, cred) in [
            (
                "antigravity-a.json",
                json!({"type":"antigravity","access_token":"t","email":"a@example.invalid","project_id":"p"}),
            ),
            ("vertex-a.json", json!({"type":"vertex","access_token":"t","email":"v@example.invalid","project_id":"p"})),
        ] {
            std::fs::write(dir.join(name), cred.to_string()).unwrap();
        }
        let key = |k: &str| vec![KeyEntry { api_key: k.into(), ..Default::default() }];
        let cfg = Config {
            auth_dir: dir.to_string_lossy().into(),
            gemini_api_key: key("g"),
            vertex_api_key: key("v"),
            ..Default::default()
        };
        let pool = crate::accounts::Pool::default();
        pool.reload(&cfg);
        let accounts = pool.all();
        let _ = std::fs::remove_dir_all(&dir);
        let mut providers: Vec<_> = accounts.iter().map(|a| a.provider.as_str()).collect();
        providers.sort();
        assert_eq!(providers, ["antigravity", "gemini", "vertex", "vertex"]);
        let contents = json!([{ "role": "user", "parts": [{ "text": "hi" }] }]);
        let empty =
            json!({ "model": "gemini-3.8-flash", "contents": contents, "systemInstruction": { "parts": [{}] } });
        let mixed =
            json!({ "contents": contents, "system_instruction": { "parts": [{}, { "text": "Be brief." }, {}] } });
        let normal = json!({ "contents": contents, "systemInstruction": { "role": "user", "parts": [{ "text": "Be brief." }] } });
        let headers = HeaderMap::new();
        for acct in &accounts {
            let prepare = |body: &Value| {
                let target = crate::upstream::Target {
                    acct,
                    cfg: &cfg,
                    client_headers: &headers,
                    model: "gemini-3.8-flash",
                    wire: Format::Gemini,
                    passthrough: true,
                    stream: false,
                    count_tokens: false,
                    cache_key: None,
                    provider_session: None,
                    cache_defaults: false,
                };
                let out = crate::upstream::prepare(&target, body.clone()).body;
                if acct.provider == Provider::Antigravity { out["request"].clone() } else { out }
            };
            let out = prepare(&empty);
            assert!(out.get("systemInstruction").is_none() && out.get("system_instruction").is_none(), "{out}");
            assert_eq!(out["contents"], contents);
            assert_eq!(prepare(&mixed)["system_instruction"], json!({ "parts": [{ "text": "Be brief." }] }));
            let out = prepare(&normal);
            assert_eq!(out["systemInstruction"], normal["systemInstruction"], "{}", acct.provider.as_str());
        }
        let mut unchanged =
            json!({ "contents": contents, "systemInstruction": { "parts": [] }, "system_instruction": "x" });
        let before = unchanged.clone();
        drop_empty_system_parts(&mut unchanged);
        assert_eq!(unchanged, before);
    }

    #[test]
    fn a_turn_without_a_role_that_calls_a_function_is_the_model_turn() {
        let body = json!({ "contents": [
            { "parts": [{ "text": "Weather in Paris?" }] },
            { "parts": [{ "functionCall": { "name": "weather", "args": { "city": "Paris" } } }] },
            { "role": "user", "parts": [{ "functionResponse": { "name": "weather", "response": { "result": "sunny" } } }] },
            { "role": "", "parts": [{ "text": "Checking again." }, { "function_call": { "name": "weather", "args": {} } }] },
            { "role": "user", "parts": [{ "functionResponse": { "name": "weather", "response": { "result": "sunny" } } }] },
            { "parts": [{ "text": "Thanks" }] }
        ] });
        let req = parse_request(&body).unwrap();
        let roles: Vec<_> = req.messages.iter().map(|m| m.role).collect();
        use Role::{Assistant, User};
        assert_eq!(roles, [User, Assistant, User, Assistant, User, User]);
    }

    #[test]
    fn openapi_schemas_are_lowered_at_keyword_positions_only() {
        let body = json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            "tools": [{ "functionDeclarations": [{ "name": "f", "parameters": {
                "type": "OBJECT",
                "properties": {
                    "type": { "type": "STRING", "enum": ["A", "B"] },
                    "maxLength": { "type": "INTEGER", "minimum": "1", "maximum": "2.5" },
                    "tags": { "type": "ARRAY", "items": { "type": "STRING", "maxLength": "10", "minLength": " 2 " },
                              "minItems": "1", "maxItems": "many" },
                    "choice": { "anyOf": [{ "type": "STRING" }, { "type": "NUMBER" }],
                                "default": { "type": "KEEP", "maxLength": "3" } }
                },
                "required": ["type"]
            } }] }],
            "generationConfig": { "responseMimeType": "application/json",
                "responseSchema": { "type": "OBJECT", "properties": { "n": { "type": "STRING", "maxLength": "4" } } } }
        });
        let req = parse_request(&body).unwrap();
        let p = &req.tools[0].parameters;
        assert_eq!(p["type"], "object");
        assert_eq!(p["properties"]["type"], json!({ "type": "string", "enum": ["A", "B"] }));
        assert_eq!(p["properties"]["maxLength"], json!({ "type": "integer", "minimum": 1, "maximum": 2.5 }));
        let tags = &p["properties"]["tags"];
        assert_eq!(tags["items"], json!({ "type": "string", "maxLength": 10, "minLength": 2 }));
        assert_eq!((&tags["minItems"], &tags["maxItems"]), (&json!(1), &json!("many")));
        assert_eq!(p["properties"]["choice"]["anyOf"], json!([{ "type": "string" }, { "type": "number" }]));
        assert_eq!(p["properties"]["choice"]["default"], json!({ "type": "KEEP", "maxLength": "3" }));
        assert_eq!(p["required"], json!(["type"]));
        let Some(ResponseFormat::JsonSchema { schema, .. }) = &req.response_format else { panic!("no schema") };
        assert_eq!(schema, &json!({ "type": "object", "properties": { "n": { "type": "string", "maxLength": 4 } } }));
        // JSON Schema needs no lowering and is passed on as sent.
        let mut json_schema = body.clone();
        json_schema["generationConfig"] = json!({ "responseMimeType": "application/json",
            "responseJsonSchema": { "type": "object", "properties": { "n": { "type": "string", "maxLength": "4" } } } });
        let Some(ResponseFormat::JsonSchema { schema, .. }) = parse_request(&json_schema).unwrap().response_format
        else {
            panic!("no schema")
        };
        assert_eq!(schema["properties"]["n"]["maxLength"], "4");
    }
}
