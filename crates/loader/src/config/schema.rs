//! Setting declarations and value validation. Core owns both: a plugin says
//! *what* a setting is (through its manifest, or the built-in table here) and
//! core parses and validates it. No plugin ships its own INI parser.
//!
//! A declaration carries the key, its type and constraints, its default, a
//! description and a restart policy. For this version every setting is
//! startup-only: the snapshot is read once, before any plugin initializes, and
//! never mutated underneath a plugin.

/// The declared type of a setting. Each has one canonical string form, which is
/// what `config_get` returns and what the typed accessors parse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueType {
    /// `true`/`yes`/`on`/`1`, or `false`/`no`/`off`/`0`.
    Bool,
    Integer {
        min: i64,
        max: i64,
    },
    /// A finite number within an inclusive range.
    Number {
        min: f64,
        max: f64,
    },
    /// One of a fixed set of strings, matched case-insensitively. The canonical
    /// value is the declared spelling.
    Choice(&'static [&'static str]),
    /// An ordinary string, as decoded from the file.
    Text,
}

/// When a changed setting takes effect. Only startup is implemented; the enum
/// exists so a future live setting can be declared without changing the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restart {
    Startup,
}

/// One declared setting.
#[derive(Debug, Clone, Copy)]
pub struct SettingDecl {
    pub key: &'static str,
    pub ty: ValueType,
    pub default: &'static str,
    pub description: &'static str,
    pub restart: Restart,
    /// Whether the value must be redacted in a diagnostic report.
    pub sensitive: bool,
}

/// A validated setting value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Integer(i64),
    Number(f64),
    Text(String),
}

impl Value {
    /// The canonical string form: what `config_get` returns and what a plugin's
    /// typed accessor parses back. Booleans are always `true`/`false`, numbers
    /// have no exponent or trailing zeros, choices keep their declared case.
    pub fn canonical(&self) -> String {
        match self {
            Value::Bool(on) => if *on { "true" } else { "false" }.to_string(),
            Value::Integer(number) => number.to_string(),
            Value::Number(number) => {
                if number.fract() == 0.0 && number.abs() < 1e15 {
                    format!("{}", *number as i64)
                } else {
                    let mut text = format!("{number}");
                    if text.contains('e') || text.contains('E') {
                        text = format!("{number:.17}");
                        while text.ends_with('0') {
                            text.pop();
                        }
                        if text.ends_with('.') {
                            text.pop();
                        }
                    }
                    text
                }
            }
            Value::Text(text) => text.clone(),
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(on) => Some(*on),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Value::Integer(number) => Some(*number),
            _ => None,
        }
    }
}

/// Parse the boolean forms the loader has always accepted.
pub fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// Validate `raw` against a declaration. The error names the setting and why
/// the value was refused; an explicit invalid value blocks its owner rather
/// than being replaced by the default.
pub fn validate(decl: &SettingDecl, raw: &str) -> Result<Value, String> {
    match decl.ty {
        ValueType::Bool => parse_bool(raw)
            .map(Value::Bool)
            .ok_or_else(|| format!("`{}` is not true/false", raw.trim())),
        ValueType::Integer { min, max } => {
            let text = raw.trim();
            let number: i64 = text
                .parse()
                .map_err(|_| format!("`{text}` is not an integer"))?;
            if number < min || number > max {
                return Err(format!("`{number}` is outside {min}..={max}"));
            }
            Ok(Value::Integer(number))
        }
        ValueType::Number { min, max } => {
            let text = raw.trim();
            let number: f64 = text
                .parse()
                .map_err(|_| format!("`{text}` is not a number"))?;
            if !number.is_finite() {
                return Err(format!("`{text}` is not finite"));
            }
            if number < min || number > max {
                return Err(format!("`{number}` is outside {min}..={max}"));
            }
            Ok(Value::Number(number))
        }
        ValueType::Choice(allowed) => {
            let text = raw.trim();
            allowed
                .iter()
                .find(|candidate| candidate.eq_ignore_ascii_case(text))
                .map(|candidate| Value::Text((*candidate).to_string()))
                .ok_or_else(|| format!("`{text}` is not one of {}", allowed.join(", ")))
        }
        ValueType::Text => Ok(Value::Text(raw.trim().to_string())),
    }
}

impl Value {
    /// The declared type that produced this value, for equality checks in
    /// tests and diagnostics.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "bool",
            Value::Integer(_) => "integer",
            Value::Number(_) => "number",
            Value::Text(_) => "text",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Restart::Startup;

    fn decl(ty: ValueType) -> SettingDecl {
        SettingDecl {
            key: "k",
            ty,
            default: "",
            description: "",
            restart: Startup,
            sensitive: false,
        }
    }

    #[test]
    fn booleans_accept_the_legacy_forms() {
        let decl = decl(ValueType::Bool);
        for on in ["true", "TRUE", "Yes", "on", "1"] {
            assert_eq!(validate(&decl, on).unwrap(), Value::Bool(true), "{on}");
        }
        for off in ["false", "no", "OFF", "0"] {
            assert_eq!(validate(&decl, off).unwrap(), Value::Bool(false), "{off}");
        }
        assert!(validate(&decl, "maybe").unwrap_err().contains("true/false"));
    }

    #[test]
    fn integers_are_bounded_and_invariant() {
        let decl = decl(ValueType::Integer { min: 0, max: 60 });
        assert_eq!(validate(&decl, " 60 ").unwrap(), Value::Integer(60));
        assert!(validate(&decl, "61").unwrap_err().contains("outside"));
        assert!(validate(&decl, "-1").unwrap_err().contains("outside"));
        assert!(validate(&decl, "1.5")
            .unwrap_err()
            .contains("not an integer"));
        assert!(validate(&decl, "0x10")
            .unwrap_err()
            .contains("not an integer"));
    }

    #[test]
    fn numbers_must_be_finite_and_in_range() {
        let decl = decl(ValueType::Number { min: 0.0, max: 1.0 });
        assert_eq!(validate(&decl, "0.5").unwrap(), Value::Number(0.5));
        assert!(validate(&decl, "nan").unwrap_err().contains("not finite"));
        assert!(validate(&decl, "inf").unwrap_err().contains("not finite"));
        assert!(validate(&decl, "1.01").unwrap_err().contains("outside"));
    }

    #[test]
    fn choices_canonicalize_to_the_declared_spelling() {
        let decl = decl(ValueType::Choice(&["error", "warn", "info"]));
        assert_eq!(validate(&decl, "INFO").unwrap(), Value::Text("info".into()));
        assert!(validate(&decl, "trace")
            .unwrap_err()
            .contains("error, warn, info"));
    }

    #[test]
    fn canonical_text_is_stable() {
        assert_eq!(Value::Bool(true).canonical(), "true");
        assert_eq!(Value::Bool(false).canonical(), "false");
        assert_eq!(Value::Integer(-4).canonical(), "-4");
        assert_eq!(Value::Number(2.5).canonical(), "2.5");
        assert_eq!(Value::Number(3.0).canonical(), "3");
        assert_eq!(Value::Text("a b".into()).canonical(), "a b");
    }
}
