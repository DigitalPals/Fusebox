//! YAML for config files, in safe Rust (issue #42 retired `serde_yaml` and its
//! unmaintained `unsafe-libyaml`).
//!
//! Documents parse into `serde_json::Value` trees under the rules config files
//! always had, which were `serde_yaml` 0.9's. Whatever it accepted still loads:
//! where its tree has no JSON form, the value is kept in one that refuses the
//! same way when a setting reads it, and passes where nothing does.
//! - plain scalars follow the YAML 1.2 core schema: `yes`, `on` and `017` stay
//!   strings, `0x10` and `0o17` are integers; `.inf` and `.nan` keep their
//!   spelling as a string; an integer beyond 64 bits but within 128 is refused,
//!   as `serde_yaml` refused it (longer ones read as floats, as there);
//! - quoted and block scalars are always strings;
//! - `!!str`, `!!int`, `!!float`, `!!bool` and `!!null` are honoured; another
//!   global tag (`!!binary`, `!!timestamp`, a `%TAG` one) makes a scalar a string
//!   and leaves a collection as it is; a local tag wraps its value, so
//!   `!env HOME` is `{"env": "HOME"}` (as `serde_yaml` read a tag, as an enum);
//! - a key that isn't a string is kept as text (`429`, `true`, `1.5`);
//! - a duplicate key or a second document is an error;
//! - an alias resolves to a copy of its anchor, with a bound on the total
//!   copied (nodes and string bytes) so an alias bomb fails instead of eating
//!   memory;
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
/// Aliases may copy at most this weight (nodes plus string bytes) per parsed
/// event, plus a fixed allowance, before the document counts as an alias bomb.
const ALIAS_WEIGHT_PER_EVENT: usize = 100;
const ALIAS_WEIGHT_ALLOWANCE: usize = 4 << 20;
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
                let mut items = Vec::new();
                loop {
                    let (event, mark) = self.next()?;
                    if event == Event::SequenceEnd {
                        break;
                    }
                    items.push(self.node(event, mark, depth + 1)?);
                }
                let value = tagged(tag.as_ref(), Value::Array(items));
                self.anchor(anchor, &value);
                Ok(value)
            }
            Event::MappingStart(anchor, tag) => {
                let mut map = Map::new();
                // Which keys were written as text, so `'429'` and `429` (two keys
                // to serde_yaml) don't collide: the text one, which a setting
                // reads, is kept.
                let (mut textual, mut other) = (std::collections::HashSet::new(), std::collections::HashSet::new());
                loop {
                    let (event, key_mark) = self.next()?;
                    if event == Event::MappingEnd {
                        break;
                    }
                    // `: value` with no key at all was refused, unlike `~:`.
                    if matches!(&event, Event::Scalar(text, TScalarStyle::Plain, _, None) if text.is_empty()) {
                        return Err(Error::at(key_mark, "a mapping entry has no key"));
                    }
                    // Written as text: not a plain scalar that resolved to something
                    // else (`.inf` is kept as a string but was a number to serde_yaml).
                    let spelled = match &event {
                        Event::Scalar(text, TScalarStyle::Plain, _, None) => Some(text.clone()),
                        _ => None,
                    };
                    let key = self.node(event, key_mark, depth + 1)?;
                    let is_text = key.is_string()
                        && spelled.is_none_or(|text| key.as_str() == Some(&text) && float(&text).is_none());
                    let key = key_text(key);
                    let (event, mark) = self.next()?;
                    let value = self.node(event, mark, depth + 1)?;
                    let fresh = if is_text { textual.insert(key.clone()) } else { other.insert(key.clone()) };
                    if !fresh {
                        return Err(Error::at(key_mark, format!("duplicate key `{key}`")));
                    }
                    if is_text || !textual.contains(&key) {
                        map.insert(key, value);
                    }
                }
                let value = tagged(tag.as_ref(), Value::Object(map));
                self.anchor(anchor, &value);
                Ok(value)
            }
            Event::Alias(id) => {
                let Some((value, weight, height)) = self.anchors.get(&id) else {
                    return Err(Error::at(mark, "an alias refers to its own anchor or to one not defined yet"));
                };
                // A copied collection nests as deep as its anchor did: the
                // limit holds for what the alias expands to.
                if depth + height > MAX_DEPTH {
                    return Err(Error::at(mark, "the document nests too deeply"));
                }
                self.copied += weight;
                if self.copied > ALIAS_WEIGHT_ALLOWANCE + self.events * ALIAS_WEIGHT_PER_EVENT {
                    return Err(Error::at(mark, "aliases expand this document too far"));
                }
                Ok(value.clone())
            }
            _ => Err(Error::at(mark, "unexpected YAML structure")),
        }
    }

    fn anchor(&mut self, id: usize, value: &Value) {
        if id != 0 {
            self.anchors.insert(id, (value.clone(), weight(value), height(value)));
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

/// What copying a value costs: one per node plus the bytes of its strings.
fn weight(value: &Value) -> usize {
    1 + match value {
        Value::String(s) => s.len(),
        Value::Array(items) => items.iter().map(weight).sum(),
        Value::Object(map) => map.iter().map(|(k, v)| k.len() + weight(v)).sum(),
        _ => 0,
    }
}

/// A mapping key as text: a string as it is, a number or boolean as written
/// back (`429`, `1.5`, `true`), null as `null`, a collection as JSON.
fn key_text(key: Value) -> String {
    match key {
        Value::String(s) => s,
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// The name of a local tag (`!env` is `env`; a bare `!` is `!`), which names
/// an application type; Fusebox has none.
fn local(tag: &Tag) -> Option<String> {
    let full = format!("{}{}", tag.handle, tag.suffix);
    let name = full.strip_prefix('!')?;
    Some(if name.is_empty() { "!".into() } else { name.to_string() })
}

/// A value under a local tag is `{name: value}`: a setting that wants a string
/// or a list refuses it, as it refused `serde_yaml`'s tagged value.
fn tagged(tag: Option<&Tag>, value: Value) -> Value {
    match tag.and_then(local) {
        Some(name) => Value::Object(Map::from_iter([(name, value)])),
        None => value,
    }
}

fn scalar(text: String, style: TScalarStyle, tag: Option<&Tag>, mark: Marker) -> Result<Value, Error> {
    let Some(tag) = tag else {
        return if style == TScalarStyle::Plain {
            plain(text).map_err(|e| Error::at(mark, e))
        } else {
            Ok(Value::String(text))
        };
    };
    if local(tag).is_some() {
        let inner = if style == TScalarStyle::Plain {
            plain(text).map_err(|e| Error::at(mark, e))?
        } else {
            Value::String(text)
        };
        return Ok(tagged(Some(tag), inner));
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
        "float" => float(&text).ok_or_else(|| invalid("!!float")),
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
        return Ok(f);
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

/// `Some` when the text is an integer; `Err` inside for one beyond 64 bits but
/// within 128, which `serde_yaml` refused.
fn integer(text: &str) -> Option<Result<Value, String>> {
    if let Some(n) = unsigned::<u64>(text, u64::from_str_radix) {
        return Some(Ok(Value::Number(n.into())));
    }
    if let Some(n) = negative::<i64>(text, i64::from_str_radix) {
        return Some(Ok(Value::Number(n.into())));
    }
    (unsigned::<u128>(text, u128::from_str_radix).is_some() || negative::<i128>(text, i128::from_str_radix).is_some())
        .then(|| Err(format!("the integer {text} is too large")))
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

/// `Some` when the text is a float: a number, or for infinities and NaN (which
/// a JSON tree can't hold) its spelling as a string.
fn float(text: &str) -> Option<Value> {
    let unsigned = match text.strip_prefix('+') {
        Some(rest) if rest.starts_with(['+', '-']) => return None,
        Some(rest) => rest,
        None => text,
    };
    if matches!(unsigned, ".inf" | ".Inf" | ".INF")
        || matches!(text, "-.inf" | "-.Inf" | "-.INF" | ".nan" | ".NaN" | ".NAN")
    {
        return Some(Value::String(text.to_string()));
    }
    let f = unsigned.parse::<f64>().ok().filter(|f| f.is_finite())?;
    Number::from_f64(f).map(Value::Number)
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
        out.push_str(&key_spelling(key));
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

/// A key spelled as an integer (`429`) is written plain: it was most likely an
/// integer key, which other readers (CLIProxyAPI) still see as one, and this
/// parser reads keys as text either way.
fn key_spelling(key: &str) -> String {
    let digits = key.strip_prefix('-').unwrap_or(key);
    let canonical = !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
        && matches!(plain(key.to_string()), Ok(Value::Number(_)));
    if canonical { key.to_string() } else { string(key) }
}

fn string(s: &str) -> String {
    if plain_safe(s) { s.to_string() } else { quoted(s) }
}

/// Words YAML 1.1 readers take for booleans, plus its merge key. Fusebox reads
/// them as strings, but CLIProxyAPI and other tools may read the same file.
const YAML11_WORDS: &[&str] =
    &["y", "Y", "yes", "Yes", "YES", "n", "N", "no", "No", "NO", "on", "On", "ON", "off", "Off", "OFF", "<<", "="];

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
        // A local tag wraps its value, so a setting expecting a string refuses it.
        assert_eq!(
            parse("a: !env HOME\nb: !t [1]\nc: !t\n"),
            json!({"a": {"env": "HOME"}, "b": {"t": [1]}, "c": {"t": null}})
        );
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
        // Keys that aren't strings are kept as text; a text key wins over the
        // same spelling as a number, and a real duplicate is still refused.
        assert_eq!(parse("429: x\ntrue: y\n1.5: z\n~: n\n"), json!({"429": "x", "true": "y", "1.5": "z", "null": "n"}));
        assert_eq!(parse("429: number\n'429': text\n"), json!({"429": "text"}));
        assert!(from_str("429: a\n429: b\n").is_err());
        assert!(from_str(": no key\n").is_err());
        assert!(from_str("a: 1\n---\nb: 2\n").is_err());
        assert_eq!(parse("a: .inf\nb: -.Inf\n"), json!({"a": ".inf", "b": "-.Inf"}), "kept as written");
        assert!(from_str("a: 18446744073709551616\n").is_err(), "65 to 128 bits, as serde_yaml refused");
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
    fn integer_keys_are_written_plain_and_other_readers_keep_them_integers() {
        let text =
            to_string(&json!({"statuses": {"429": "retry", "007": "x", "-1": "y", "1.5": "z", "=": "eq"}})).unwrap();
        assert!(text.contains("\n  429: retry\n"), "{text}");
        assert!(text.contains("\n  -1: \"y\"\n"), "{text}");
        assert!(
            text.contains("\n  \"007\": x\n") && text.contains("\n  \"1.5\": z\n") && text.contains("\n  \"=\": eq\n"),
            "{text}"
        );
        assert_eq!(parse(&text), json!({"statuses": {"429": "retry", "007": "x", "-1": "y", "1.5": "z", "=": "eq"}}));
    }

    #[test]
    fn big_aliased_strings_count_against_the_alias_budget() {
        let big = "x".repeat(1 << 20);
        let mut doc = format!("a: &a \"{big}\"\nb: [");
        doc.push_str(&vec!["*a"; 64].join(", "));
        doc.push_str("]\n");
        assert!(from_str(&doc).unwrap_err().to_string().contains("aliases expand"));
        assert!(from_str(&format!("a: &a \"{}\"\nb: [*a, *a, *a]\n", "x".repeat(1000))).is_ok());
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
