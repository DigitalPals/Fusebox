//! YAML for config files, in safe Rust (issue #42 retired `serde_yaml` and its
//! unmaintained `unsafe-libyaml`).
//!
//! Documents parse into `serde_json::Value` trees under the rules config files
//! always had, which were `serde_yaml` 0.9's:
//! - plain scalars follow the YAML 1.2 core schema: `yes`, `on` and `017` stay
//!   strings, `0x10` and `0o17` are integers, `.inf` and `.nan` are refused
//!   because a JSON tree can't hold them;
//! - quoted and block scalars are always strings;
//! - `!!str`, `!!int`, `!!float`, `!!bool` and `!!null` are honoured; another
//!   global tag (`!!binary`, `!!timestamp`, a `%TAG` one) makes a scalar a string
//!   and leaves a collection as it is; a local tag (`!thing`, `!`) is refused;
//! - a duplicate key, a key that isn't a string, or a second document is an error;
//! - an alias resolves to a copy of its anchor, with a bound on the total
//!   copied so an alias bomb fails instead of eating memory;
//! - `<<` is an ordinary key, never a merge;
//! - an empty document is null, and a leading byte-order mark is skipped;
//! - collections nest at most 128 deep.
//!
//! One deliberate difference: anchor names follow YAML 1.2 and may contain `:`,
//! so `&a: b` anchors `b` as `a:`, where `serde_yaml` (libyaml) refused it.
//!
//! Writing is block style. A string is written plain only when it would read
//! back as the same string; otherwise it is double-quoted.

use std::collections::HashMap;
use std::fmt;

use serde_json::{Map, Number, Value};
use yaml_rust2::parser::{Event, Parser, Tag};
use yaml_rust2::scanner::{Marker, TScalarStyle};

/// Collections nested deeper than this are refused, as `serde_yaml` refused them.
const MAX_DEPTH: usize = 128;
/// Aliases may copy at most this many nodes per parsed event, plus a fixed
/// allowance, before the document counts as an alias bomb.
const ALIAS_NODES_PER_EVENT: usize = 100;
const ALIAS_NODES_ALLOWANCE: usize = 10_000;
/// YAML limits implicit keys to 1024 characters.
const MAX_KEY_CHARS: usize = 1024;
const CORE_TAG: &str = "tag:yaml.org,2002:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    fn at(mark: Marker, message: impl fmt::Display) -> Self {
        Self(format!("{message} at line {} column {}", mark.line(), mark.col() + 1))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Parses one YAML document. Empty text (or only comments) is `Value::Null`.
pub fn from_str(text: &str) -> Result<Value, Error> {
    let mut loader = Loader {
        parser: Parser::new_from_str(text.strip_prefix('\u{feff}').unwrap_or(text)),
        anchors: HashMap::new(),
        events: 0,
        copied: 0,
    };
    let (event, mark) = loader.next()?;
    if event != Event::StreamStart {
        return Err(Error::at(mark, "expected the start of the YAML stream"));
    }
    let (event, mark) = loader.next()?;
    let value = match event {
        Event::StreamEnd => return Ok(Value::Null),
        Event::DocumentStart => {
            let (event, mark) = loader.next()?;
            let value = loader.node(event, mark, 0)?;
            let (event, mark) = loader.next()?;
            if event != Event::DocumentEnd {
                return Err(Error::at(mark, "expected the end of the document"));
            }
            value
        }
        _ => return Err(Error::at(mark, "expected a YAML document")),
    };
    let (event, mark) = loader.next()?;
    if event != Event::StreamEnd {
        return Err(Error::at(mark, "a config file holds one YAML document, but this one has more"));
    }
    Ok(value)
}

struct Loader<'a> {
    parser: Parser<std::str::Chars<'a>>,
    /// Finished anchored nodes with their node counts and collection depth, by
    /// the parser's anchor id.
    anchors: HashMap<usize, (Value, usize, usize)>,
    events: usize,
    copied: usize,
}

impl Loader<'_> {
    fn next(&mut self) -> Result<(Event, Marker), Error> {
        self.events += 1;
        self.parser.next_token().map_err(|e| Error(e.to_string()))
    }

    fn node(&mut self, event: Event, mark: Marker, depth: usize) -> Result<Value, Error> {
        if depth >= MAX_DEPTH && matches!(event, Event::SequenceStart(..) | Event::MappingStart(..)) {
            return Err(Error::at(mark, "the document nests too deeply"));
        }
        match event {
            Event::Scalar(text, style, anchor, tag) => {
                let value = scalar(text, style, tag.as_ref(), mark)?;
                self.anchor(anchor, &value);
                Ok(value)
            }
            Event::SequenceStart(anchor, tag) => {
                collection_tag(tag.as_ref(), mark)?;
                let mut items = Vec::new();
                loop {
                    let (event, mark) = self.next()?;
                    if event == Event::SequenceEnd {
                        break;
                    }
                    items.push(self.node(event, mark, depth + 1)?);
                }
                let value = Value::Array(items);
                self.anchor(anchor, &value);
                Ok(value)
            }
            Event::MappingStart(anchor, tag) => {
                collection_tag(tag.as_ref(), mark)?;
                let mut map = Map::new();
                loop {
                    let (event, key_mark) = self.next()?;
                    if event == Event::MappingEnd {
                        break;
                    }
                    let key = match self.node(event, key_mark, depth + 1)? {
                        Value::String(key) => key,
                        other => {
                            return Err(Error::at(
                                key_mark,
                                format!("mapping keys must be strings, not {}", kind(&other)),
                            ));
                        }
                    };
                    if map.contains_key(&key) {
                        return Err(Error::at(key_mark, format!("duplicate key `{key}`")));
                    }
                    let (event, mark) = self.next()?;
                    let value = self.node(event, mark, depth + 1)?;
                    map.insert(key, value);
                }
                let value = Value::Object(map);
                self.anchor(anchor, &value);
                Ok(value)
            }
            Event::Alias(id) => {
                let Some((value, size, height)) = self.anchors.get(&id) else {
                    return Err(Error::at(mark, "an alias refers to its own anchor or to one not defined yet"));
                };
                // A copied collection nests as deep as its anchor did: the
                // limit holds for what the alias expands to.
                if depth + height > MAX_DEPTH {
                    return Err(Error::at(mark, "the document nests too deeply"));
                }
                self.copied += size;
                if self.copied > ALIAS_NODES_ALLOWANCE + self.events * ALIAS_NODES_PER_EVENT {
                    return Err(Error::at(mark, "aliases expand this document too far"));
                }
                Ok(value.clone())
            }
            _ => Err(Error::at(mark, "unexpected YAML structure")),
        }
    }

    fn anchor(&mut self, id: usize, value: &Value) {
        if id != 0 {
            self.anchors.insert(id, (value.clone(), nodes(value), height(value)));
        }
    }
}

/// How many collections deep a value nests (0 for a scalar).
fn height(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(height).max().unwrap_or(0),
        Value::Object(map) => 1 + map.values().map(height).max().unwrap_or(0),
        _ => 0,
    }
}

fn nodes(value: &Value) -> usize {
    1 + match value {
        Value::Array(items) => items.iter().map(nodes).sum(),
        Value::Object(map) => map.values().map(nodes).sum(),
        _ => 0,
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a sequence",
        Value::Object(_) => "a mapping",
    }
}

/// A local tag (`!thing`, or a bare `!`) names an application type; Fusebox has none.
fn local(tag: &Tag) -> bool {
    tag.handle.starts_with('!') || (tag.handle.is_empty() && tag.suffix.starts_with('!'))
}

fn collection_tag(tag: Option<&Tag>, mark: Marker) -> Result<(), Error> {
    match tag {
        Some(tag) if local(tag) => Err(unsupported(tag, mark)),
        _ => Ok(()),
    }
}

fn unsupported(tag: &Tag, mark: Marker) -> Error {
    let name =
        if tag.handle == CORE_TAG { format!("!!{}", tag.suffix) } else { format!("{}{}", tag.handle, tag.suffix) };
    Error::at(mark, format!("the YAML tag {name} is not supported"))
}

fn scalar(text: String, style: TScalarStyle, tag: Option<&Tag>, mark: Marker) -> Result<Value, Error> {
    let Some(tag) = tag else {
        return if style == TScalarStyle::Plain {
            plain(text).map_err(|e| Error::at(mark, e))
        } else {
            Ok(Value::String(text))
        };
    };
    if local(tag) {
        return Err(unsupported(tag, mark));
    }
    // Global tags other than the core ones leave a string.
    if tag.handle != CORE_TAG {
        return Ok(Value::String(text));
    }
    let invalid = |what: &str| Error::at(mark, format!("`{text}` is not a valid {what}"));
    match tag.suffix.as_str() {
        "str" => Ok(Value::String(text)),
        "null" if null(&text) => Ok(Value::Null),
        "null" => Err(invalid("!!null")),
        "bool" => boolean(&text).map(Value::Bool).ok_or_else(|| invalid("!!bool")),
        "int" => integer(&text).ok_or_else(|| invalid("!!int"))?.map_err(|e| Error::at(mark, e)),
        // `!!float 1` is the float 1.0, and leading zeros are allowed.
        "float" => float(&text).ok_or_else(|| invalid("!!float"))?.map_err(|e| Error::at(mark, e)),
        _ => Ok(Value::String(text)),
    }
}

/// An untagged plain scalar under the core schema, resolved in `serde_yaml`'s order.
fn plain(text: String) -> Result<Value, String> {
    if text.is_empty() || null(&text) {
        return Ok(Value::Null);
    }
    if let Some(b) = boolean(&text) {
        return Ok(Value::Bool(b));
    }
    if let Some(n) = integer(&text) {
        return n;
    }
    if !digits_but_not_number(&text)
        && let Some(f) = float(&text)
    {
        return f;
    }
    Ok(Value::String(text))
}

fn null(text: &str) -> bool {
    matches!(text, "~" | "null" | "Null" | "NULL")
}

fn boolean(text: &str) -> Option<bool> {
    match text {
        "true" | "True" | "TRUE" => Some(true),
        "false" | "False" | "FALSE" => Some(false),
        _ => None,
    }
}

/// `Some` when the text is an integer; `Err` inside when it is one too large to keep.
fn integer(text: &str) -> Option<Result<Value, String>> {
    if let Some(n) = unsigned::<u64>(text, u64::from_str_radix) {
        return Some(Ok(Value::Number(n.into())));
    }
    if let Some(n) = negative::<i64>(text, i64::from_str_radix) {
        return Some(Ok(Value::Number(n.into())));
    }
    if unsigned::<u128>(text, u128::from_str_radix).is_some() || negative::<i128>(text, i128::from_str_radix).is_some()
    {
        return Some(Err(format!("the integer {text} is too large")));
    }
    None
}

fn unsigned<T>(text: &str, radix: fn(&str, u32) -> Result<T, std::num::ParseIntError>) -> Option<T> {
    let unsigned = text.strip_prefix('+').unwrap_or(text);
    for (prefix, base) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        if let Some(rest) = unsigned.strip_prefix(prefix) {
            if rest.starts_with(['+', '-']) {
                return None;
            }
            if let Ok(n) = radix(rest, base) {
                return Some(n);
            }
        }
    }
    if unsigned.starts_with(['+', '-']) || digits_but_not_number(text) {
        return None;
    }
    radix(unsigned, 10).ok()
}

fn negative<T>(text: &str, radix: fn(&str, u32) -> Result<T, std::num::ParseIntError>) -> Option<T> {
    for (prefix, base) in [("-0x", 16), ("-0o", 8), ("-0b", 2)] {
        if let Some(rest) = text.strip_prefix(prefix)
            && let Ok(n) = radix(&format!("-{rest}"), base)
        {
            return Some(n);
        }
    }
    if digits_but_not_number(text) {
        return None;
    }
    radix(text, 10).ok()
}

/// Leading zeros followed by digits are a string under YAML 1.2 (`017`, `-007`).
fn digits_but_not_number(text: &str) -> bool {
    let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
    digits.len() > 1 && digits.starts_with('0') && digits[1..].bytes().all(|b| b.is_ascii_digit())
}

/// `Some` when the text is a float; `Err` inside for infinities and NaN.
fn float(text: &str) -> Option<Result<Value, String>> {
    let unsigned = match text.strip_prefix('+') {
        Some(rest) if rest.starts_with(['+', '-']) => return None,
        Some(rest) => rest,
        None => text,
    };
    if matches!(unsigned, ".inf" | ".Inf" | ".INF")
        || matches!(text, "-.inf" | "-.Inf" | "-.INF" | ".nan" | ".NaN" | ".NAN")
    {
        return Some(Err(format!("{text} can't be stored in a config value")));
    }
    let f = unsigned.parse::<f64>().ok().filter(|f| f.is_finite())?;
    Number::from_f64(f).map(|n| Ok(Value::Number(n)))
}

/// Writes anything serializable as a block-style YAML document, ending in a newline.
pub fn to_string<T: serde::Serialize + ?Sized>(value: &T) -> Result<String, Error> {
    match serde_json::to_value(value) {
        Ok(value) => write(&value),
        Err(e) => Err(Error(e.to_string())),
    }
}

fn write(value: &Value) -> Result<String, Error> {
    let mut out = String::new();
    match value {
        Value::Object(map) if !map.is_empty() => write_map(&mut out, map, 0)?,
        Value::Array(items) if !items.is_empty() => write_seq(&mut out, items, 0)?,
        other => {
            out.push_str(&inline(other).expect("scalars and empty collections are inline"));
            out.push('\n');
        }
    }
    Ok(out)
}

/// A scalar or empty collection on one line; `None` for anything that needs a block.
fn inline(value: &Value) -> Option<String> {
    Some(match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => string(s),
        Value::Array(items) if items.is_empty() => "[]".into(),
        Value::Object(map) if map.is_empty() => "{}".into(),
        _ => return None,
    })
}

fn write_map(out: &mut String, map: &Map<String, Value>, indent: usize) -> Result<(), Error> {
    for (key, value) in map {
        if key.chars().count() > MAX_KEY_CHARS {
            return Err(Error(format!("a key of {} characters is too long to write", key.chars().count())));
        }
        push_indent(out, indent);
        out.push_str(&string(key));
        out.push(':');
        match inline(value) {
            Some(scalar) => {
                out.push(' ');
                out.push_str(&scalar);
                out.push('\n');
            }
            None => {
                out.push('\n');
                match value {
                    Value::Object(inner) => write_map(out, inner, indent + 2)?,
                    // A sequence under a key starts at the key's own column.
                    Value::Array(items) => write_seq(out, items, indent)?,
                    _ => unreachable!("scalars are inline"),
                }
            }
        }
    }
    Ok(())
}

fn write_seq(out: &mut String, items: &[Value], indent: usize) -> Result<(), Error> {
    for item in items {
        push_indent(out, indent);
        out.push('-');
        if let Some(scalar) = inline(item) {
            out.push(' ');
            out.push_str(&scalar);
            out.push('\n');
            continue;
        }
        // The item's block goes two columns in, its first line after the dash.
        let mut block = String::new();
        match item {
            Value::Object(inner) => write_map(&mut block, inner, indent + 2)?,
            Value::Array(inner) => write_seq(&mut block, inner, indent + 2)?,
            _ => unreachable!("scalars are inline"),
        }
        out.push(' ');
        out.push_str(&block[indent + 2..]);
    }
    Ok(())
}

fn push_indent(out: &mut String, indent: usize) {
    out.extend(std::iter::repeat_n(' ', indent));
}

fn string(s: &str) -> String {
    if plain_safe(s) { s.to_string() } else { quoted(s) }
}

/// Words YAML 1.1 readers take for booleans, plus its merge key. Fusebox reads
/// them as strings, but CLIProxyAPI and other tools may read the same file.
const YAML11_WORDS: &[&str] =
    &["y", "Y", "yes", "Yes", "YES", "n", "N", "no", "No", "NO", "on", "On", "ON", "off", "Off", "OFF", "<<"];

/// Whether `s` written plain reads back as the same string in any position,
/// for this parser and for YAML 1.1 ones. Anything starting with a digit, a
/// sign or a dot is quoted, so no reader takes it for a number, a date or `...`.
fn plain_safe(s: &str) -> bool {
    let Some(first) = s.chars().next() else { return false };
    if "-?:,[]{}#&*!|>'\"%@` .+".contains(first) || first.is_ascii_digit() || YAML11_WORDS.contains(&s) {
        return false;
    }
    if s.ends_with([' ', ':']) || s.contains(": ") || s.contains(" #") {
        return false;
    }
    if !s.chars().all(|c| c == ' ' || (!c.is_whitespace() && printable(c) && !matches!(c, ',' | '[' | ']' | '{' | '}')))
    {
        return false;
    }
    plain(s.to_string()).is_ok_and(|v| v.as_str() == Some(s))
}

/// Characters YAML allows unescaped, minus line separators and the byte-order mark.
fn printable(c: char) -> bool {
    matches!(c, '\u{20}'..='\u{7E}' | '\u{A0}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
        && !matches!(c, '\u{2028}' | '\u{2029}' | '\u{FEFF}')
}

fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if printable(c) => out.push(c),
            c if (c as u32) <= 0xFFFF => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push_str(&format!("\\U{:08X}", c as u32)),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(text: &str) -> Value {
        from_str(text).unwrap_or_else(|e| panic!("{text:?}: {e}"))
    }

    #[test]
    fn plain_scalars_follow_the_core_schema() {
        let doc = parse(
            "a: yes\nb: on\nc: 017\nd: 0x10\ne: 0o17\nf: 1_000\ng: ~\nh: True\ni: -5\nj: 1.5\nk: 1e3\nl: 2026-11-04\nm:\nn: .5\no: +7\n",
        );
        assert_eq!(
            doc,
            json!({"a": "yes", "b": "on", "c": "017", "d": 16, "e": 15, "f": "1_000", "g": null, "h": true,
                "i": -5, "j": 1.5, "k": 1000.0, "l": "2026-11-04", "m": null, "n": 0.5, "o": 7})
        );
    }

    #[test]
    fn quoted_and_tagged_scalars() {
        assert_eq!(
            parse("a: '5'\nb: \"true\"\nc: !!str 7\nd: !!int '8'\ne: |\n  x\n"),
            json!({"a": "5", "b": "true", "c": "7", "d": 8, "e": "x\n"})
        );
        assert!(from_str("a: !custom x").is_err());
        assert!(from_str("a: !!int nope").is_err());
    }

    #[test]
    fn structure_rules() {
        assert_eq!(parse(""), Value::Null);
        assert_eq!(parse("# only a comment\n"), Value::Null);
        assert_eq!(
            parse("base: &b {x: 1}\ncopy: *b\n<<: *b\n"),
            json!({"base": {"x": 1}, "copy": {"x": 1}, "<<": {"x": 1}})
        );
        assert!(from_str("a: 1\na: 2\n").unwrap_err().to_string().contains("duplicate key `a`"));
        assert!(from_str("1: x\n").is_err());
        assert!(from_str("a: 1\n---\nb: 2\n").is_err());
        assert!(from_str("a: .inf\n").is_err());
        assert!(from_str("a: 18446744073709551616\n").is_err());
        assert!(from_str(&"[".repeat(200)).is_err());
        // Aliases can't nest deeper than the limit either.
        let chain = |links: usize| {
            let mut doc = String::from("a0: &a0 [x]\n");
            for i in 1..links {
                doc.push_str(&format!("a{i}: &a{i} [*a{}]\n", i - 1));
            }
            doc
        };
        assert!(from_str(&chain(120)).is_ok(), "120 deep, through aliases");
        assert!(from_str(&chain(150)).unwrap_err().to_string().contains("nests too deeply"));
    }

    #[test]
    fn alias_bombs_are_refused() {
        let mut doc = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
        for i in 1..9 {
            let refs = vec![format!("*a{}", i - 1); 10].join(", ");
            doc.push_str(&format!("a{i}: &a{i} [{refs}]\n"));
        }
        assert!(from_str(&doc).unwrap_err().to_string().contains("aliases expand"));
    }

    #[test]
    fn writes_block_yaml_that_reads_back() {
        let value = json!({
            "port": 8317, "ratio": 0.25, "debug": false, "none": null, "empty": {}, "list": [],
            "models": ["gpt-6.1-sol", "yes", "017", "", " pad", "a: b", "multi\nline", "\u{7f}", "café", "<<", "4o"],
            "groups": [{"name": "pair", "models": ["a", "b"]}, [1, [2]]],
            "url": "https://api.example.com:8443/v1",
        });
        let text = to_string(&value).unwrap();
        assert!(text.contains("models:\n- gpt-6.1-sol\n- \"yes\"\n- \"017\"\n"), "{text}");
        assert!(text.contains("groups:\n- name: pair\n  models:\n  - a\n  - b\n- - 1\n  - - 2\n"), "{text}");
        assert_eq!(parse(&text), value);
        assert_eq!(to_string(&json!({})).unwrap(), "{}\n");
        assert_eq!(to_string(&json!("...")).unwrap(), "\"...\"\n");
    }
}
