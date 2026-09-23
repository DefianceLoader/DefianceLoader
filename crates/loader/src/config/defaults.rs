//! Generated commented defaults and insertion of missing settings.
//!
//! Startup inserts only missing declarations; existing text stays byte-for-byte
//! intact. The runtime supplies both built-in and manifest declarations.

use super::builtin::{
    self, BUILTINS, LOADER_SECTION, LOADER_SETTINGS, LOGGING_SECTION, LOGGING_SETTINGS,
};
use super::schema::SettingDecl;

/// The declared `(section, settings)` blocks of a group, in file order.
pub fn blocks(group: &str) -> Vec<(&'static str, &'static [SettingDecl])> {
    let mut blocks = Vec::new();
    match group {
        "core" => {
            blocks.push((LOADER_SECTION, LOADER_SETTINGS));
            blocks.push((LOGGING_SECTION, LOGGING_SETTINGS));
        }
        _ => {
            for builtin in BUILTINS {
                if builtin.group != group {
                    continue;
                }
                let settings = builtin::settings(builtin.id);
                if settings.is_empty() {
                    continue;
                }
                blocks.push((builtin.id, settings));
            }
        }
    }
    blocks
}

/// The one-line summary for a section, if it has one.
fn summary(section: &str) -> Option<&'static str> {
    builtin::find(section).map(|builtin| builtin.summary)
}

/// The complete commented file for a group that does not exist yet.
pub fn render_group(group: &str) -> String {
    let mut text = String::new();
    text.push_str(&format!("; DefianceLoader configuration: {group}.\n"));
    text.push_str("; Startup-only for this version: edit, save, and start the game again.\n");
    text.push_str(
        "; Disabling a required feature also prevents its dependent features from loading.\n",
    );
    if group == "core" {
        text.push_str(
            "; Core support has no enabled toggle; use the individual gameplay settings instead.\n",
        );
    }
    text.push_str("; Values may contain `;`, `#` or `=` if the whole value is double-quoted.\n");
    for (section, settings) in blocks(group) {
        text.push('\n');
        text.push_str(&format!("[{section}]\n"));
        if let Some(summary) =
            summary(section).filter(|s| !settings.iter().any(|d| d.description == *s))
        {
            text.push_str(&format!("; {summary}\n"));
        }
        for decl in settings {
            text.push_str(&setting_help(decl.description, decl.default, "\n"));
            text.push_str(&format!("{} = {}\n", decl.key, decl.default));
        }
    }
    text
}

/// Insert any declared setting missing from `existing`, preserving comments,
/// ordering and every existing value. Returns the new text and whether it
/// differs from what was passed in.
pub fn materialize(existing: &str, group: &str) -> (String, bool) {
    let declared = blocks(group);
    // Which declared keys each present section already has.
    let mut present: Vec<(String, Vec<String>)> = Vec::new();
    let mut current = String::new();
    for raw in existing.lines() {
        let line = raw.trim_start();
        if line.starts_with('[') {
            if let Some(end) = line.find(']') {
                current = line[1..end].trim().to_ascii_lowercase();
                if !present.iter().any(|(name, _)| *name == current) {
                    present.push((current.clone(), Vec::new()));
                }
            }
            continue;
        }
        let content = line.split([';', '#']).next().unwrap_or("").trim();
        if let Some((key, _)) = content.split_once('=') {
            if let Some((_, keys)) = present.iter_mut().find(|(name, _)| *name == current) {
                keys.push(key.trim().to_ascii_lowercase());
            }
        }
    }

    let mut text = existing.to_string();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    let mut changed = false;
    for (section, settings) in declared {
        let has = present
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(section))
            .map(|(_, keys)| keys.as_slice())
            .unwrap_or(&[]);
        let missing: Vec<&SettingDecl> = settings
            .iter()
            .filter(|decl| !has.iter().any(|key| key.eq_ignore_ascii_case(decl.key)))
            .collect();
        if missing.is_empty() {
            continue;
        }
        changed = true;
        let section_present = present
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(section));
        if !section_present {
            if !text.ends_with("\n\n") && !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("[{section}]\n"));
            if let Some(summary) =
                summary(section).filter(|s| !settings.iter().any(|d| d.description == *s))
            {
                text.push_str(&format!("; {summary}\n"));
            }
        }
        for decl in missing {
            text.push_str(&setting_help(decl.description, decl.default, "\n"));
            text.push_str(&format!("{} = {}\n", decl.key, decl.default));
        }
    }
    (text, changed)
}

/// Materialize combined declarations without rewriting existing text. Invalid
/// files and invalid defaults are left alone; snapshot validation reports them.
pub fn extend_missing(
    existing: &str,
    declarations: &[(&str, &SettingDecl)],
    bootstrap: &super::parse::Document,
) -> String {
    if !super::parse::parse(existing).issues.is_empty() {
        return existing.to_string();
    }
    let newline = if existing.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut text = existing.to_string();
    for &(section, decl) in declarations {
        let document = super::parse::parse(&text);
        if document.lookup(section, decl.key).is_some() {
            continue;
        }
        // Leave legacy overrides authoritative until explicitly migrated.
        if section == LOADER_SECTION && bootstrap.top(decl.key).is_some() {
            continue;
        }
        if super::schema::validate(decl, decl.default).is_err() {
            continue;
        }
        let value = quote_value(decl.default);
        let mut block = setting_help(decl.description, decl.default, newline);
        block.push_str(&format!("{} = {}{}", decl.key, value, newline));
        let mut found = false;
        let mut offset = 0;
        let mut insert_at = text.len();
        for line in text.split_inclusive('\n') {
            let trimmed = line.trim_start_matches('\u{feff}').trim_start();
            if let Some(rest) = trimmed.strip_prefix('[') {
                if let Some(end) = rest.find(']') {
                    if rest[..end].trim().eq_ignore_ascii_case(section) {
                        found = true;
                    } else if found {
                        insert_at = offset;
                        break;
                    }
                }
            }
            offset += line.len();
        }
        if insert_at > 0 && !text[..insert_at].ends_with('\n') {
            block.insert_str(0, newline);
        }
        if !found {
            block.insert_str(
                0,
                &format!(
                    "{}[{}]{}",
                    if text.is_empty() { "" } else { newline },
                    section,
                    newline
                ),
            );
        }
        // A malformed third-party declaration must never damage a valid file.
        let mut candidate = text.clone();
        candidate.insert_str(insert_at, &block);
        if super::parse::parse(&candidate).issues.is_empty() {
            text = candidate;
        }
    }
    text
}

fn setting_help(description: &str, default: &str, newline: &str) -> String {
    let mut text = String::new();
    for line in description.lines() {
        text.push_str(&format!("; {line}{newline}"));
    }
    let default = quote_value(default)
        .replace('\r', "\\r")
        .replace('\n', "\\n");
    text.push_str(&format!("; Default: {default}{newline}"));
    text
}

fn quote_value(value: &str) -> String {
    if value.is_empty() || value.trim() != value || value.contains([';', '#', '=', '"', '\n', '\r'])
    {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

/// Insert `key = value` at the end of `section`, or append the section if it is
/// absent. Used by migration to write a legacy value into a group file without
/// disturbing comments or any other line. Always ends with a newline.
pub fn insert_setting(
    existing: &str,
    section: &str,
    key: &str,
    value: &str,
    description: &str,
    default: &str,
) -> String {
    let section_lower = section.to_ascii_lowercase();
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    let mut found = false;
    let mut insert_at = lines.len();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix('[') else {
            continue;
        };
        let Some(end) = rest.find(']') else { continue };
        let name = rest[..end].trim().to_ascii_lowercase();
        if name == section_lower {
            found = true;
        } else if found {
            insert_at = index;
            break;
        }
    }
    let mut block: Vec<String> = setting_help(description, default, "\n")
        .lines()
        .map(str::to_string)
        .collect();
    block.push(format!("{key} = {value}"));
    if found {
        for (offset, line) in block.into_iter().enumerate() {
            lines.insert(insert_at + offset, line);
        }
    } else {
        if !lines.is_empty() && !lines.last().is_some_and(String::is_empty) {
            lines.push(String::new());
        }
        lines.push(format!("[{section}]"));
        lines.extend(block);
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_text_values_round_trip_and_multiline_help_is_commented() {
        let decl = SettingDecl {
            key: "text",
            ty: super::super::schema::ValueType::Text,
            default: " a;#=\"\\z ",
            description: "First line\nSecond line",
            restart: super::super::schema::Restart::Startup,
            sensitive: false,
        };
        let after = extend_missing("; keep\r\n", &[("demo", &decl)], &Default::default());
        assert!(after.contains("; First line\r\n; Second line\r\n"));
        assert!(after.contains(&format!("; Default: {}\r\n", quote_value(decl.default))));
        let doc = super::super::parse::parse(&after);
        assert!(doc.issues.is_empty());
        assert_eq!(doc.lookup("demo", "text").unwrap().0.value, decl.default);
        assert_eq!(
            extend_missing(&after, &[("demo", &decl)], &Default::default()),
            after
        );
    }

    #[test]
    fn rendered_defaults_parse_back_to_the_declared_values() {
        for group in builtin::GROUPS {
            let text = render_group(group);
            let document = super::super::parse::parse(&text);
            assert!(document.issues.is_empty(), "{group}: {:?}", document.issues);
            for (section, settings) in blocks(group) {
                for decl in settings {
                    let (entry, _) = document
                        .lookup(section, decl.key)
                        .unwrap_or_else(|| panic!("{group} is missing {section}.{}", decl.key));
                    assert_eq!(entry.value, decl.default);
                    assert!(text.contains(&format!("; Default: {}\n", quote_value(decl.default))));
                }
            }
        }
    }

    #[test]
    fn materialize_is_idempotent_and_preserves_content() {
        let original = "; a comment\n[defiance.selection]\n; keep me\nenabled = false\n";
        let (once, changed) = materialize(original, "infantry");
        assert!(changed);
        assert!(once.contains("; a comment"));
        assert!(once.contains("; keep me"));
        // The existing explicit value is untouched.
        assert!(once.contains("enabled = false"));
        // The other infantry sections were added.
        assert!(once.contains("[defiance.movement]"));
        assert!(once.contains("[defiance.posture]"));
        let (twice, changed) = materialize(&once, "infantry");
        assert!(!changed);
        assert_eq!(twice, once);
    }

    #[test]
    fn materialize_adds_missing_keys_to_an_existing_section() {
        let (text, changed) = materialize("[logging]\n; mine\nlevel = warn\n", "core");
        assert!(changed);
        assert!(text.contains("level = warn"));
        assert!(text.contains("[loader]"));
        // The missing loader keys were appended under a fresh header.
        assert!(text.contains("wait = 60"));
    }

    #[test]
    fn empty_file_materializes_all_blocks() {
        let (text, changed) = materialize("", "core");
        assert!(changed);
        assert!(text.contains("[loader]"));
        assert!(text.contains("[logging]"));
        assert!(text.contains("allow_unknown_build = false"));
    }
}
