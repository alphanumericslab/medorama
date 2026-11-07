//! Dynamic values: records as the Python implementations hold them (dicts in insertion order,
//! lists, numbers, strings, None), with JSON and `str()` text identical to Python's, so that
//! every implementation writes the same bytes. Readers convert their records to a [`Value`];
//! the engine's index metadata and the viewers' answers are [`Value`]s.

use crate::pyfmt::repr;
use indexmap::IndexMap;

pub type Dict = IndexMap<String, Value>;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    #[default]
    None,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    Dict(Dict),
}

impl Value {
    pub fn is_none(&self) -> bool {
        matches!(self, Value::None)
    }

    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_dict_mut(&mut self) -> Option<&mut Dict> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&Vec<Value>> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i128> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// `d.get(key)` for a dict (None for anything else).
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_dict().and_then(|d| d.get(key))
    }

    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Value::None => false,
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::Dict(d) => !d.is_empty(),
        }
    }

    /// `json.dumps(v, separators=(',', ':'))` (ASCII output, NaN/Infinity as Python writes
    /// them).
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out);
        out
    }

    pub fn write_json(&self, out: &mut String) {
        match self {
            Value::None => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(i) => out.push_str(&i.to_string()),
            Value::Float(f) => out.push_str(&json_float(*f)),
            Value::Str(s) => json_string(s, out),
            Value::List(l) => {
                out.push('[');
                for (i, v) in l.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write_json(out);
                }
                out.push(']');
            }
            Value::Dict(d) => {
                out.push('{');
                for (i, (k, v)) in d.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    json_string(k, out);
                    out.push(':');
                    v.write_json(out);
                }
                out.push('}');
            }
        }
    }

    /// Python's `str(v)`.
    pub fn py_str(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            _ => self.py_repr(),
        }
    }

    /// Python's `repr(v)`.
    pub fn py_repr(&self) -> String {
        match self {
            Value::None => "None".into(),
            Value::Bool(b) => (if *b { "True" } else { "False" }).into(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => repr(*f),
            Value::Str(s) => str_repr(s),
            Value::List(l) => format!("[{}]", l.iter().map(Value::py_repr).collect::<Vec<_>>().join(", ")),
            Value::Dict(d) => format!(
                "{{{}}}",
                d.iter().map(|(k, v)| format!("{}: {}", str_repr(k), v.py_repr())).collect::<Vec<_>>().join(", ")
            ),
        }
    }
}

impl Value {
    /// Parse JSON text (as written by [`Value::to_json`] or Python's `json`: `NaN`,
    /// `Infinity` and `-Infinity` accepted).  Integers stay integers.
    pub fn from_json(text: &str) -> Result<Value, String> {
        let mut p = JsonParser { b: text.as_bytes(), i: 0, depth: 0 };
        let v = p.value()?;
        p.ws();
        if p.i != p.b.len() {
            return Err(format!("trailing characters at byte {}", p.i));
        }
        Ok(v)
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
}

/// Arrays and objects nested deeper than this are refused (the parser recurses).
const MAX_DEPTH: usize = 128;

struct JsonParser<'a> {
    b: &'a [u8],
    i: usize,
    /// arrays and objects open around the current position
    depth: usize,
}

impl JsonParser<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("JSON: {what} at byte {}", self.i))
    }

    fn lit(&mut self, word: &str, v: Value) -> Result<Value, String> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            self.err("unexpected character")
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        self.ws();
        if !matches!(self.b.get(self.i), Some(b'{' | b'[')) {
            return self.scalar_or_container();
        }
        if self.depth >= MAX_DEPTH {
            return self.err("arrays and objects nested too deeply");
        }
        self.depth += 1;
        let v = self.scalar_or_container();
        self.depth -= 1;
        v
    }

    fn scalar_or_container(&mut self) -> Result<Value, String> {
        let Some(&c) = self.b.get(self.i) else { return self.err("unexpected end") };
        match c {
            b'{' => {
                self.i += 1;
                let mut d = Dict::new();
                self.ws();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Value::Dict(d));
                }
                loop {
                    self.ws();
                    let Value::Str(k) = self.string()? else { unreachable!() };
                    self.ws();
                    if self.b.get(self.i) != Some(&b':') {
                        return self.err("expected ':'");
                    }
                    self.i += 1;
                    let v = self.value()?;
                    d.insert(k, v);
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Value::Dict(d));
                        }
                        _ => return self.err("expected ',' or '}'"),
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut l = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Value::List(l));
                }
                loop {
                    l.push(self.value()?);
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Value::List(l));
                        }
                        _ => return self.err("expected ',' or ']'"),
                    }
                }
            }
            b'"' => self.string(),
            b't' => self.lit("true", Value::Bool(true)),
            b'f' => self.lit("false", Value::Bool(false)),
            b'n' => self.lit("null", Value::None),
            b'N' => self.lit("NaN", Value::Float(f64::NAN)),
            b'I' => self.lit("Infinity", Value::Float(f64::INFINITY)),
            b'-' if self.b[self.i..].starts_with(b"-Infinity") => {
                self.lit("-Infinity", Value::Float(f64::NEG_INFINITY))
            }
            _ => self.number(),
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let s = self.i;
        while self.i < self.b.len() && matches!(self.b[self.i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
            self.i += 1;
        }
        let t = std::str::from_utf8(&self.b[s..self.i]).unwrap_or("");
        if t.is_empty() {
            return self.err("unexpected character");
        }
        if !t.contains(['.', 'e', 'E']) {
            if let Ok(i) = t.parse::<i128>() {
                return Ok(Value::Int(i));
            }
        }
        t.parse::<f64>().map(Value::Float).map_err(|_| format!("JSON: bad number {t:?}"))
    }

    fn string(&mut self) -> Result<Value, String> {
        if self.b.get(self.i) != Some(&b'"') {
            return self.err("expected a string");
        }
        self.i += 1;
        let mut out = String::new();
        let mut units: Vec<u16> = Vec::new();
        loop {
            let Some(&c) = self.b.get(self.i) else { return self.err("unterminated string") };
            if c == b'\\' && self.b.get(self.i + 1) == Some(&b'u') {
                let h = std::str::from_utf8(self.b.get(self.i + 2..self.i + 6).unwrap_or(b"")).unwrap_or("");
                let u = u16::from_str_radix(h, 16).map_err(|_| "JSON: bad \\u escape".to_string())?;
                units.push(u);
                self.i += 6;
                continue;
            }
            if !units.is_empty() {
                out.extend(char::decode_utf16(units.drain(..)).map(|r| r.unwrap_or('\u{fffd}')));
            }
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(Value::Str(out));
                }
                b'\\' => {
                    let e = *self.b.get(self.i + 1).unwrap_or(&b'?');
                    out.push(match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'/' => '/',
                        b'"' => '"',
                        b'\\' => '\\',
                        _ => return self.err("bad escape"),
                    });
                    self.i += 2;
                }
                _ => {
                    let start = self.i;
                    while self.i < self.b.len() && self.b[self.i] != b'"' && self.b[self.i] != b'\\' {
                        self.i += 1;
                    }
                    out.push_str(
                        std::str::from_utf8(&self.b[start..self.i]).map_err(|_| "JSON: invalid UTF-8".to_string())?,
                    );
                }
            }
        }
    }
}

pub fn json_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else {
        repr(f)
    }
}

/// JSON string with Python's default escaping (`ensure_ascii=True`).
pub fn json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Python's `repr(str)`.
pub fn str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (0x80..0xa0).contains(&(c as u32)) => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::Str(s.to_string())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Value {
        Value::Str(s)
    }
}
impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Bool(b)
    }
}
impl From<f64> for Value {
    fn from(f: f64) -> Value {
        Value::Float(f)
    }
}
impl From<f32> for Value {
    fn from(f: f32) -> Value {
        Value::Float(f as f64)
    }
}
macro_rules! from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(i: $t) -> Value { Value::Int(i as i128) }
        }
    )*};
}
from_int!(i8, i16, i32, i64, u8, u16, u32, u64, usize, i128);
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(o: Option<T>) -> Value {
        o.map_or(Value::None, Into::into)
    }
}
impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Value {
        Value::List(v.into_iter().map(Into::into).collect())
    }
}
impl From<Dict> for Value {
    fn from(d: Dict) -> Value {
        Value::Dict(d)
    }
}

/// Build a [`Dict`]: `dict! { "a" => 1, "b" => "x" }`.
#[macro_export]
macro_rules! dict {
    ($($k:expr => $v:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut d = $crate::value::Dict::new();
        $( d.insert(($k).to_string(), $crate::value::Value::from($v)); )*
        d
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_like_python() {
        let d = dict! { "a" => 1, "f" => 240.0, "n" => f64::NAN, "s" => "é\"x\n", "l" => vec![1.5, 1e-5], "z" => Value::None };
        assert_eq!(
            Value::Dict(d).to_json(),
            "{\"a\":1,\"f\":240.0,\"n\":NaN,\"s\":\"\\u00e9\\\"x\\n\",\"l\":[1.5,1e-05],\"z\":null}"
        );
        assert_eq!(Value::Str("\u{1F600}".into()).to_json(), "\"\\ud83d\\ude00\"");
    }

    #[test]
    fn json_nesting_is_bounded() {
        let deep = |n: usize| format!("{}1{}", "[".repeat(n), "]".repeat(n));
        assert!(Value::from_json(&deep(MAX_DEPTH)).is_ok());
        assert!(Value::from_json(&deep(MAX_DEPTH + 1)).unwrap_err().contains("nested too deeply"));
        // deep enough to overflow the stack without the limit
        assert!(Value::from_json(&"[".repeat(1_000_000)).is_err());
        assert!(Value::from_json(&"{\"a\":".repeat(200_000)).is_err());
    }

    #[test]
    fn json_roundtrip() {
        let text = r#"{"a":[1,2.5,NaN,-Infinity,null,true],"b":{"c":"x\"y\u00e9\ud83d\ude00","d":-3e-05},"e":[]}"#;
        let v = Value::from_json(text).unwrap();
        assert_eq!(v.get("b").and_then(|b| b.get("c")).and_then(Value::as_str), Some("x\"y\u{e9}\u{1F600}"));
        assert_eq!(v.to_json(), text);
    }

    #[test]
    fn str_like_python() {
        let d = dict! { "__class__" => "X", "value__" => 3, "t" => true, "q" => "it's" };
        assert_eq!(Value::Dict(d).py_str(), r#"{'__class__': 'X', 'value__': 3, 't': True, 'q': "it's"}"#);
        assert_eq!(Value::Float(2.0).py_str(), "2.0");
        assert_eq!(Value::None.py_str(), "None");
    }
}
