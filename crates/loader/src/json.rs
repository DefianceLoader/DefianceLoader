//! A small JSON reader, sufficient for the plugin manifests. The project takes
//! no external dependencies, and the manifest is a small, known document, so a
//! hand-written recursive-descent parser is less code than justifying a crate.
//!
//! Object order is preserved, which keeps generated manifests stable. Duplicate
//! object keys are kept as separate entries so the manifest reader can report
//! them rather than silently last-wins.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

#[allow(dead_code)] // some accessors are used only by tests today
impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(on) => Some(*on),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(entries) => entries
                .iter()
                .rev()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }
}

pub fn parse(text: &str) -> Result<Value, String> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        at: 0,
    };
    parser.skip_space();
    let value = parser.value()?;
    parser.skip_space();
    if parser.at != parser.bytes.len() {
        return Err(format!("unexpected trailing text at byte {}", parser.at));
    }
    Ok(value)
}

/// Parse and require an object, for a document whose top level is one.
pub fn parse_object(text: &str) -> Result<Vec<(String, Value)>, String> {
    match parse(text)? {
        Value::Object(entries) => Ok(entries),
        other => Err(format!("expected a JSON object, found {}", kind(&other))),
    }
}

pub fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Render an object with stable two-space indentation, for generated manifests.
pub fn render(value: &Value) -> String {
    let mut text = String::new();
    write_value(&mut text, value, 0);
    text.push('\n');
    text
}

fn write_value(out: &mut String, value: &Value, depth: usize) {
    let pad = "  ".repeat(depth);
    let inner = "  ".repeat(depth + 1);
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(on) => out.push_str(if *on { "true" } else { "false" }),
        Value::Number(number) => out.push_str(&number_string(*number)),
        Value::String(text) => out.push_str(&quote(text)),
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items) => {
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                out.push_str(&inner);
                write_value(out, item, depth + 1);
                if index + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad);
            out.push(']');
        }
        Value::Object(entries) if entries.is_empty() => out.push_str("{}"),
        Value::Object(entries) => {
            out.push_str("{\n");
            for (index, (key, item)) in entries.iter().enumerate() {
                out.push_str(&inner);
                out.push_str(&quote(key));
                out.push_str(": ");
                write_value(out, item, depth + 1);
                if index + 1 < entries.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad);
            out.push('}');
        }
    }
}

fn number_string(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{}", number as i64)
    } else {
        format!("{number}")
    }
}

fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn skip_space(&mut self) {
        while self.at < self.bytes.len()
            && matches!(self.bytes[self.at], b' ' | b'\t' | b'\r' | b'\n')
        {
            self.at += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(format!("expected `{}` at byte {}", byte as char, self.at))
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(format!("invalid literal at byte {}", self.at))
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        match self.peek() {
            None => Err("unexpected end of input".into()),
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(format!("unexpected `{}` at byte {}", c as char, self.at)),
        }
    }

    fn object(&mut self) -> Result<Value, String> {
        self.expect(b'{')?;
        let mut entries = Vec::new();
        self.skip_space();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Value::Object(entries));
        }
        loop {
            self.skip_space();
            let key = self.string()?;
            self.skip_space();
            self.expect(b':')?;
            self.skip_space();
            let value = self.value()?;
            entries.push((key, value));
            self.skip_space();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Value::Object(entries));
                }
                _ => return Err(format!("expected `,` or `}}` at byte {}", self.at)),
            }
        }
    }

    fn array(&mut self) -> Result<Value, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_space();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_space();
            items.push(self.value()?);
            self.skip_space();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(format!("expected `,` or `]` at byte {}", self.at)),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err("unterminated string".into());
            };
            match byte {
                b'"' => {
                    self.at += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.at += 1;
                    let escape = self.peek().ok_or("unterminated escape")?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let code = self.hex4()?;
                            if (0xd800..0xdc00).contains(&code) {
                                // Surrogate pair.
                                self.expect(b'\\')?;
                                self.expect(b'u')?;
                                let low = self.hex4()?;
                                let combined = 0x10000
                                    + (((code - 0xd800) as u32) << 10)
                                    + (low.saturating_sub(0xdc00) as u32);
                                out.push(char::from_u32(combined).unwrap_or('\u{fffd}'));
                            } else {
                                out.push(char::from_u32(code as u32).unwrap_or('\u{fffd}'));
                            }
                        }
                        other => return Err(format!("bad escape `\\{}`", other as char)),
                    }
                }
                c if c < 0x20 => return Err("control character in string".into()),
                _ => {
                    let start = self.at;
                    let lead = self.bytes[self.at];
                    let extra = if lead < 0x80 {
                        0
                    } else if lead >> 5 == 0b110 {
                        1
                    } else if lead >> 4 == 0b1110 {
                        2
                    } else if lead >> 3 == 0b11110 {
                        3
                    } else {
                        return Err(format!("invalid UTF-8 lead byte at {}", self.at));
                    };
                    let end = self.at + extra + 1;
                    if end > self.bytes.len() {
                        return Err("truncated UTF-8 in string".into());
                    }
                    let text = std::str::from_utf8(&self.bytes[start..end])
                        .map_err(|_| "invalid UTF-8 in string".to_string())?;
                    out.push_str(text);
                    self.at = end;
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u16, String> {
        if self.at + 4 > self.bytes.len() {
            return Err("truncated \\u escape".into());
        }
        let text = std::str::from_utf8(&self.bytes[self.at..self.at + 4])
            .map_err(|_| "bad \\u escape".to_string())?;
        let code = u16::from_str_radix(text, 16).map_err(|_| "bad \\u escape".to_string())?;
        self.at += 4;
        Ok(code)
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            self.at += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at]).unwrap_or("");
        let number: f64 = text
            .parse()
            .map_err(|_| format!("invalid number `{text}`"))?;
        if !number.is_finite() {
            return Err(format!("number `{text}` is not finite"));
        }
        Ok(Value::Number(number))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_nested_documents() {
        let value = parse(r#"{ "a": [1, 2, {"b": "c"}], "d": true, "e": null }"#).unwrap();
        assert_eq!(value.get("d").unwrap().as_bool(), Some(true));
        assert_eq!(value.get("a").unwrap().as_array().unwrap().len(), 3);
        assert_eq!(
            value.get("a").unwrap().as_array().unwrap()[2]
                .get("b")
                .unwrap()
                .as_str(),
            Some("c")
        );
    }

    #[test]
    fn decodes_escapes_and_unicode() {
        let value = parse(r#""a\"b\\c\n\u0041\u00e9""#).unwrap();
        assert_eq!(value.as_str(), Some("a\"b\\c\nA\u{e9}"));
    }

    #[test]
    fn reports_errors_without_panicking() {
        assert!(parse("{").is_err());
        assert!(parse("{\"a\": }").is_err());
        assert!(parse("\"unterminated").is_err());
        assert!(parse("[1,]").is_err());
        assert!(parse("{} trailing").is_err());
    }

    #[test]
    fn rendering_round_trips() {
        let value = parse(r#"{"b":2,"a":[true,"x"]}"#).unwrap();
        let rendered = render(&value);
        assert_eq!(parse(&rendered).unwrap(), value);
    }
}
