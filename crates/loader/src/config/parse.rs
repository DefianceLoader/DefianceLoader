//! The INI reader. Pure text in, entries out: no filesystem, no schema, no
//! logging, so the quirks below are testable on their own.
//!
//! The grammar is the one the old `defiance-loader.ini` already used, kept so
//! existing files keep working:
//!
//! - `[section]` opens a section; names and keys are matched case-insensitively.
//! - `key = value` is one setting. The value ends at an inline `;` or `#`
//!   comment, unless the value is double-quoted.
//! - A line that is neither blank, a comment, a section, nor `key = value` is a
//!   structural error. A file with one is not applied; its bytes are preserved.
//!
//! Quoting is the documented escape hatch for a value that must contain `#`,
//! `;` or `=` literally: wrap it in double quotes. Inside the quotes a backslash
//! makes the next character literal (`\"` is a quote, `\\` is a backslash), and
//! nothing else is special. The decoded value never includes the quotes or the
//! backslashes. A UTF-8 BOM and both LF and CRLF line endings are accepted.

/// One `key = value` under its section. `section` is empty at the top level,
/// which is where the bootstrap file keeps its own keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub section: String,
    pub key: String,
    pub value: String,
    pub line: usize,
    pub quoted: bool,
}

/// A line the reader could not make sense of. `line` is one-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub line: usize,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    pub entries: Vec<Entry>,
    pub issues: Vec<Issue>,
}

impl Document {
    /// The last value for `(section, key)`, lower-cased on both, matching the
    /// loader's old last-value-wins rule, plus the line numbers of any earlier
    /// declarations so a duplicate can be reported without changing the result.
    pub fn lookup(&self, section: &str, key: &str) -> Option<(&Entry, Vec<usize>)> {
        let section = section.to_ascii_lowercase();
        let key = key.to_ascii_lowercase();
        let matching: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| entry.section == section && entry.key == key)
            .collect();
        let last = *matching.last()?;
        let earlier = matching[..matching.len() - 1]
            .iter()
            .map(|entry| entry.line)
            .collect();
        Some((last, earlier))
    }

    /// The top-level value of `key` (the bootstrap's own unsectioned keys).
    pub fn top(&self, key: &str) -> Option<(&Entry, Vec<usize>)> {
        self.lookup("", key)
    }
}

/// Remove a UTF-8 BOM, if present.
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Parse a whole file. Never fails: a structural problem is recorded in
/// `issues` and parsing continues, so every entry is still available for
/// provenance reporting.
pub fn parse(text: &str) -> Document {
    let mut document = Document::default();
    let mut section = String::new();
    let text = strip_bom(text);

    for (index, raw) in text.split('\n').enumerate() {
        let line = index + 1;
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = raw.trim_start();
        if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('[') {
            match rest.find(']') {
                Some(end) => {
                    let name = rest[..end].trim().to_ascii_lowercase();
                    if name.is_empty() {
                        document.issues.push(Issue {
                            line,
                            message: "empty section name".into(),
                        });
                    }
                    section = name;
                    let trailing = rest[end + 1..].trim();
                    if !trailing.is_empty()
                        && !trailing.starts_with(';')
                        && !trailing.starts_with('#')
                    {
                        document.issues.push(Issue {
                            line,
                            message: format!("unexpected text after section header: `{trailing}`"),
                        });
                    }
                }
                None => document.issues.push(Issue {
                    line,
                    message: "section header has no closing `]`".into(),
                }),
            }
            continue;
        }
        let Some((left, right)) = trimmed.split_once('=') else {
            document.issues.push(Issue {
                line,
                message: "line is not `key = value`".into(),
            });
            continue;
        };
        let key = left.trim().to_ascii_lowercase();
        if key.is_empty() {
            document.issues.push(Issue {
                line,
                message: "setting has an empty key".into(),
            });
            continue;
        }
        match decode_value(right) {
            Ok((value, quoted)) => document.entries.push(Entry {
                section: section.clone(),
                key,
                value,
                line,
                quoted,
            }),
            Err(message) => document.issues.push(Issue { line, message }),
        }
    }
    document
}

/// Decode the right-hand side of `key = value`: a quoted string, or the text up
/// to an inline comment. Returns the decoded value and whether it was quoted.
pub fn decode_value(raw: &str) -> Result<(String, bool), String> {
    let trimmed = raw.trim_start();
    if let Some(body) = trimmed.strip_prefix('"') {
        let mut value = String::new();
        let mut chars = body.chars();
        loop {
            match chars.next() {
                None => return Err("unterminated quoted value".into()),
                Some('"') => break,
                Some('\\') => match chars.next() {
                    None => return Err("unterminated quoted value".into()),
                    Some(next) => value.push(next),
                },
                Some(other) => value.push(other),
            }
        }
        let trailing = chars.as_str().trim();
        if !trailing.is_empty() && !trailing.starts_with(';') && !trailing.starts_with('#') {
            return Err(format!("text after a quoted value: `{trailing}`"));
        }
        Ok((value, true))
    } else {
        let end = trimmed.find([';', '#']).unwrap_or(trimmed.len());
        Ok((trimmed[..end].trim().to_string(), false))
    }
}

/// Skip a leading comment line's marker; used when deciding whether a stray
/// line is content or a comment.
pub fn is_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with(';') || trimmed.starts_with('#')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sections_keys_and_inline_comments() {
        let document = parse("[One]\nKey = value ; a comment\nother=# another\n");
        assert!(document.issues.is_empty());
        let (entry, earlier) = document.lookup("one", "KEY").unwrap();
        assert_eq!(entry.value, "value");
        assert!(!entry.quoted);
        assert!(earlier.is_empty());
        assert_eq!(document.lookup("one", "other").unwrap().0.value, "");
    }

    #[test]
    fn quoted_values_carry_comment_characters_literally() {
        let document = parse("a = \"x; y # z = w\"\nb = \"a\\\"b\\\\c\"\nc = plain\n");
        assert!(document.issues.is_empty());
        assert_eq!(document.top("a").unwrap().0.value, "x; y # z = w");
        assert_eq!(document.top("b").unwrap().0.value, "a\"b\\c");
        assert_eq!(document.top("c").unwrap().0.value, "plain");
        assert!(document.top("a").unwrap().0.quoted);
    }

    #[test]
    fn top_level_section_is_empty() {
        let document = parse("root = ../DefianceLoader\n[defiance.x]\nenabled = true\n");
        assert_eq!(document.top("root").unwrap().0.value, "../DefianceLoader");
        assert_eq!(
            document.lookup("defiance.x", "enabled").unwrap().0.value,
            "true"
        );
    }

    #[test]
    fn bom_crlf_and_unicode_survive() {
        let document = parse("\u{feff}[s\u{e9}ction]\r\nna\u{ef}ve = caf\u{e9} \u{1f600}\r\n");
        assert!(document.issues.is_empty(), "{:?}", document.issues);
        let (entry, _) = document.lookup("s\u{e9}ction", "na\u{ef}ve").unwrap();
        assert_eq!(entry.value, "caf\u{e9} \u{1f600}");
    }

    #[test]
    fn duplicate_keys_keep_the_last_and_report_the_earlier_lines() {
        let document = parse("plugins = one\nplugins = two ; comment\nplugins = three\n");
        let (entry, earlier) = document.top("plugins").unwrap();
        assert_eq!(entry.value, "three");
        assert_eq!(entry.line, 3);
        assert_eq!(earlier, vec![1, 2]);
    }

    #[test]
    fn structural_errors_are_collected_not_fatal() {
        let document = parse("this line has no equals\n[unclosed\nkey = value\n");
        let lines: Vec<usize> = document.issues.iter().map(|issue| issue.line).collect();
        assert_eq!(lines, vec![1, 2]);
        assert_eq!(document.top("key").unwrap().0.value, "value");
    }

    #[test]
    fn unterminated_quotes_are_an_issue() {
        let document = parse("a = \"never closed\nb = ok\n");
        assert_eq!(document.issues.len(), 1);
        assert_eq!(document.issues[0].line, 1);
        assert_eq!(document.top("b").unwrap().0.value, "ok");
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let document = parse("; header\n\n# other\n   \n[ a ]\n k = v \n");
        assert!(document.issues.is_empty());
        assert_eq!(document.lookup("a", "k").unwrap().0.value, "v");
    }
}
