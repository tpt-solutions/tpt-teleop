//! Minimal dependency-free JSON value, serializer, and parser.
//!
//! The fleet dashboard and the MCP JSON-RPC endpoint both speak JSON. Rather
//! than pull `serde_json` (dual MIT/Apache and a wider transitive tree), this
//! crate carries a tiny self-contained JSON codec that covers exactly the
//! shapes we emit and parse (objects, arrays, numbers, strings, bool, null).
//!
//! Object order is preserved via a `Vec<(String, Json)>` so dashboard payloads
//! are deterministic and diff-friendly.

/// A JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    /// `null`.
    Null,
    /// Boolean.
    Bool(bool),
    /// Number (stored as `f64`; integer-valued floats round-trip cleanly for
    /// the magnitudes this crate emits).
    Num(f64),
    /// String.
    Str(String),
    /// Array.
    Arr(Vec<Json>),
    /// Object (insertion-ordered key/value pairs).
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// Builds an object from borrowed key/value pairs.
    pub fn obj(fields: &[(&str, Json)]) -> Json {
        Json::Obj(
            fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    /// Builds an object whose values are all strings.
    pub fn obj_str(fields: &[(&str, &str)]) -> Json {
        Json::Obj(
            fields
                .iter()
                .map(|(k, v)| (k.to_string(), Json::Str(v.to_string())))
                .collect(),
        )
    }

    /// Builds an object whose values are all numbers.
    pub fn obj_num(fields: &[(&str, f64)]) -> Json {
        Json::Obj(
            fields
                .iter()
                .map(|(k, v)| (k.to_string(), Json::Num(*v)))
                .collect(),
        )
    }

    /// A string value.
    pub fn str(s: &str) -> Json {
        Json::Str(s.to_string())
    }

    /// A number value.
    pub fn num(n: f64) -> Json {
        Json::Num(n)
    }

    /// An unsigned integer value (widened to `f64`; see [`Json::Num`]).
    pub fn uint(n: u64) -> Json {
        Json::Num(n as f64)
    }

    /// An array value.
    pub fn arr(items: Vec<Json>) -> Json {
        Json::Arr(items)
    }

    /// Looks up a top-level object field by key.
    pub fn get(&self, key: &str) -> Option<&Json> {
        if let Json::Obj(f) = self {
            f.iter().find(|(k, _)| k == key).map(|(_, v)| v)
        } else {
            None
        }
    }

    /// Borrow the inner string, if this is one.
    pub fn as_str(&self) -> Option<&str> {
        if let Json::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }

    /// Borrow the inner number as `u64`, if this is a number.
    pub fn as_u64(&self) -> Option<u64> {
        if let Json::Num(n) = self {
            Some(*n as u64)
        } else {
            None
        }
    }

    /// Borrow the inner bool, if this is one.
    pub fn as_bool(&self) -> Option<bool> {
        if let Json::Bool(b) = self {
            Some(*b)
        } else {
            None
        }
    }

    /// Borrow the inner array, if this is one.
    pub fn as_array(&self) -> Option<&[Json]> {
        if let Json::Arr(items) = self {
            Some(items)
        } else {
            None
        }
    }

    /// Serializes to a compact JSON string.
    pub fn write_to_string(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Num(n) => {
                if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
                    out.push_str(&format!("{}", *n as i64));
                } else {
                    out.push_str(&format!("{n}"));
                }
            }
            Json::Str(s) => write_json_string(s, out),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(fields) => {
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_json_string(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

    /// Parses a single JSON value from `bytes` (trailing whitespace allowed).
    pub fn parse(bytes: &[u8]) -> Result<Json, String> {
        let mut i = 0usize;
        let v = parse_value(bytes, &mut i).map_err(|e| e.to_string())?;
        skip_ws(bytes, &mut i);
        if i != bytes.len() {
            return Err("trailing data after JSON value".into());
        }
        Ok(v)
    }
}

impl core::fmt::Display for Json {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut s = String::new();
        self.write(&mut s);
        f.write_str(&s)
    }
}

fn write_json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn skip_ws(b: &[u8], i: &mut usize) {
    while *i < b.len() {
        let c = b[*i] as char;
        if c == ' ' || c == '\t' || c == '\n' || c == '\r' {
            *i += 1;
        } else {
            break;
        }
    }
}

fn parse_value(b: &[u8], i: &mut usize) -> Result<Json, String> {
    skip_ws(b, i);
    if *i >= b.len() {
        return Err("unexpected end of input".into());
    }
    match b[*i] {
        b'"' => parse_string(b, i).map(Json::Str),
        b'{' => parse_object(b, i),
        b'[' => parse_array(b, i),
        b't' => {
            expect(b, i, b"true")?;
            Ok(Json::Bool(true))
        }
        b'f' => {
            expect(b, i, b"false")?;
            Ok(Json::Bool(false))
        }
        b'n' => {
            expect(b, i, b"null")?;
            Ok(Json::Null)
        }
        b'-' | b'0'..=b'9' => parse_number(b, i).map(Json::Num),
        other => Err(format!("unexpected character '{}' in JSON", other as char)),
    }
}

fn expect(b: &[u8], i: &mut usize, lit: &[u8]) -> Result<(), String> {
    if b.len() >= *i + lit.len() && &b[*i..*i + lit.len()] == lit {
        *i += lit.len();
        Ok(())
    } else {
        Err(format!("expected '{}'", String::from_utf8_lossy(lit)))
    }
}

fn parse_string(b: &[u8], i: &mut usize) -> Result<String, String> {
    // Assumes b[*i] == '"'.
    *i += 1;
    let mut s = String::new();
    while *i < b.len() {
        let c = b[*i];
        if c == b'"' {
            *i += 1;
            return Ok(s);
        }
        if c == b'\\' {
            *i += 1;
            if *i >= b.len() {
                return Err("unterminated escape".into());
            }
            let e = b[*i];
            *i += 1;
            match e {
                b'"' => s.push('"'),
                b'\\' => s.push('\\'),
                b'/' => s.push('/'),
                b'n' => s.push('\n'),
                b'r' => s.push('\r'),
                b't' => s.push('\t'),
                b'b' => s.push('\u{08}'),
                b'f' => s.push('\u{0c}'),
                b'u' => {
                    if *i + 4 > b.len() {
                        return Err("bad \\u escape".into());
                    }
                    let hex = &b[*i..*i + 4];
                    let code = u32::from_str_radix(&String::from_utf8_lossy(hex), 16)
                        .map_err(|_| "bad \\u escape".to_string())?;
                    *i += 4;
                    s.push(char::from_u32(code).ok_or_else(|| "bad \\u codepoint".to_string())?);
                }
                _ => return Err(format!("bad escape '\\{}'", e as char)),
            }
            continue;
        }
        // Raw byte — assume UTF-8 continuation; push as char from first byte.
        let ch = b[*i] as char;
        s.push(ch);
        *i += 1;
    }
    Err("unterminated string".into())
}

fn parse_number(b: &[u8], i: &mut usize) -> Result<f64, String> {
    let start = *i;
    if *i < b.len() && b[*i] == b'-' {
        *i += 1;
    }
    while *i < b.len() && b[*i].is_ascii_digit() {
        *i += 1;
    }
    if *i < b.len() && b[*i] == b'.' {
        *i += 1;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
    }
    if *i < b.len() && (b[*i] == b'e' || b[*i] == b'E') {
        *i += 1;
        if *i < b.len() && (b[*i] == b'+' || b[*i] == b'-') {
            *i += 1;
        }
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
    }
    let slice = &b[start..*i];
    let s = std::str::from_utf8(slice).map_err(|_| "invalid number".to_string())?;
    s.parse::<f64>().map_err(|_| "invalid number".to_string())
}

fn parse_array(b: &[u8], i: &mut usize) -> Result<Json, String> {
    *i += 1; // '['
    let mut items = Vec::new();
    skip_ws(b, i);
    if *i < b.len() && b[*i] == b']' {
        *i += 1;
        return Ok(Json::Arr(items));
    }
    loop {
        let v = parse_value(b, i)?;
        items.push(v);
        skip_ws(b, i);
        if *i >= b.len() {
            return Err("unterminated array".into());
        }
        match b[*i] {
            b',' => {
                *i += 1;
            }
            b']' => {
                *i += 1;
                return Ok(Json::Arr(items));
            }
            _ => return Err("expected ',' or ']' in array".into()),
        }
    }
}

fn parse_object(b: &[u8], i: &mut usize) -> Result<Json, String> {
    *i += 1; // '{'
    let mut fields = Vec::new();
    skip_ws(b, i);
    if *i < b.len() && b[*i] == b'}' {
        *i += 1;
        return Ok(Json::Obj(fields));
    }
    loop {
        skip_ws(b, i);
        if *i >= b.len() || b[*i] != b'"' {
            return Err("expected object key string".into());
        }
        let key = parse_string(b, i)?;
        skip_ws(b, i);
        if *i >= b.len() || b[*i] != b':' {
            return Err("expected ':' in object".into());
        }
        *i += 1; // ':'
        let v = parse_value(b, i)?;
        fields.push((key, v));
        skip_ws(b, i);
        if *i >= b.len() {
            return Err("unterminated object".into());
        }
        match b[*i] {
            b',' => {
                *i += 1;
            }
            b'}' => {
                *i += 1;
                return Ok(Json::Obj(fields));
            }
            _ => return Err("expected ',' or '}' in object".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_primitives() {
        assert_eq!(Json::Null.to_string(), "null");
        assert_eq!(Json::Bool(true).to_string(), "true");
        assert_eq!(Json::num(3.0).to_string(), "3");
        assert_eq!(Json::num(2.5).to_string(), "2.5");
        assert_eq!(Json::str("hi").to_string(), "\"hi\"");
        assert_eq!(Json::uint(42).to_string(), "42");
    }

    #[test]
    fn serialize_nested() {
        let v = Json::obj(&[
            ("name", Json::str("drone-1")),
            ("count", Json::uint(3)),
            ("tags", Json::arr(vec![Json::str("a"), Json::str("b")])),
        ]);
        assert_eq!(
            v.to_string(),
            "{\"name\":\"drone-1\",\"count\":3,\"tags\":[\"a\",\"b\"]}"
        );
    }

    #[test]
    fn escapes_strings() {
        assert_eq!(Json::str("a\"b\\c").to_string(), "\"a\\\"b\\\\c\"");
        assert_eq!(Json::str("line\nbreak").to_string(), "\"line\\nbreak\"");
    }

    #[test]
    fn roundtrip_parse() {
        let src = b"{\"name\":\"drone-1\",\"ok\":true,\"n\":3,\"vals\":[1,2.5,3],\"nested\":{\"x\":null}}";
        let v = Json::parse(src).unwrap();
        assert_eq!(v.get("name").and_then(|s| s.as_str()), Some("drone-1"));
        assert_eq!(v.get("ok").and_then(|b| b.as_bool()), Some(true));
        assert_eq!(v.get("n").and_then(|n| n.as_u64()), Some(3));
        assert_eq!(
            v.to_string(),
            Json::parse(v.to_string().as_bytes()).unwrap().to_string()
        );
    }

    #[test]
    fn parse_rejects_trailing_garbage() {
        assert!(Json::parse(b"1 2").is_err());
        assert!(Json::parse(b"{").is_err());
        assert!(Json::parse(b"\"unterminated").is_err());
    }
}
