//! The validated, immutable configuration snapshot.
//!
//! Loading is separated from parsing and from the filesystem: `build` takes one
//! [`GroupInput`] per group (its text, or why it could not be read) and the
//! parsed bootstrap, and returns everything the runtime needs. That keeps the
//! failure policy — missing, invalid, duplicate, unknown, unreadable, blocked —
//! testable without touching disk or loading a DLL.
//!
//! The setting identity is `(plugin_id, key)`. Each identity has one
//! declaration and one authoritative destination file. A group file is an
//! organizational choice, not a dependency.

use super::builtin::{self, LOADER_SECTION, LOGGING_SECTION};
use super::parse::{self, Document, Entry};
use super::paths::Paths;
use super::schema::{validate, SettingDecl, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

/// Where a resolved value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    /// An explicit setting in its group file.
    Group { file: String, line: usize },
    /// A supported legacy equivalent in the bootstrap file.
    Bootstrap { file: String, line: usize },
    /// The declaration's default, because nothing explicitly set it.
    Default,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub value: Value,
    pub provenance: Provenance,
}

impl Resolved {
    pub fn canonical(&self) -> String {
        self.value.canonical()
    }
}

/// Something wrong with one owner's configuration. Every problem blocks its
/// owner; the loader refuses to enable a plugin whose explicit setting is
/// invalid rather than silently falling back to a default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub owner: String,
    pub file: String,
    pub line: Option<usize>,
    pub message: String,
}

/// One group file as handed to `build`. `text` is `None` when the file does not
/// exist; `Some(Err(_))` when it exists but cannot be read or decoded (an
/// unreadable file is *not* a missing one).
#[derive(Debug, Clone)]
pub struct GroupInput {
    pub group: String,
    pub path: PathBuf,
    pub text: Option<Result<String, String>>,
}

impl GroupInput {
    pub fn present(group: &str, path: PathBuf, text: &str) -> GroupInput {
        GroupInput {
            group: group.to_string(),
            path,
            text: Some(Ok(text.to_string())),
        }
    }

    pub fn missing(group: &str, path: PathBuf) -> GroupInput {
        GroupInput {
            group: group.to_string(),
            path,
            text: None,
        }
    }

    pub fn unreadable(group: &str, path: PathBuf, reason: &str) -> GroupInput {
        GroupInput {
            group: group.to_string(),
            path,
            text: Some(Err(reason.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupStatus {
    pub group: String,
    pub path: PathBuf,
    pub existed: bool,
    pub usable: bool,
}

/// The frozen configuration. Values are canonical strings by the time anything
/// can read them; strings live in the map for the process's life.
pub struct Snapshot {
    pub paths: Paths,
    resolved: HashMap<(String, String), Resolved>,
    legacy: HashMap<(String, String), String>,
    pub problems: Vec<Problem>,
    pub blocked: BTreeSet<String>,
    pub unknown: Vec<Entry>,
    pub warnings: Vec<String>,
    pub files: Vec<GroupStatus>,
    /// `(plugin_id, key)` pairs whose value must be redacted in a report.
    sensitive: BTreeSet<(String, String)>,
}

/// One declaration to resolve, with the group it lives in and a display owner.
struct DeclRef {
    owner: String,
    group: &'static str,
    decl: &'static SettingDecl,
}

/// A declaration contributed by a third-party manifest. Core owns parsing and
/// validation; the strings are leaked once at load so the uniform declaration
/// model can carry them for the process's life.
#[derive(Debug, Clone, Copy)]
pub struct Declared {
    pub owner: &'static str,
    pub group: &'static str,
    pub decl: &'static SettingDecl,
}

fn declarations(extras: &[Declared]) -> Vec<DeclRef> {
    let mut decls = Vec::new();
    for decl in builtin::LOADER_SETTINGS {
        decls.push(DeclRef {
            owner: LOADER_SECTION.to_string(),
            group: "core",
            decl,
        });
    }
    for decl in builtin::LOGGING_SETTINGS {
        decls.push(DeclRef {
            owner: LOGGING_SECTION.to_string(),
            group: "core",
            decl,
        });
    }
    for feature in builtin::features() {
        for decl in builtin::settings(feature.id) {
            decls.push(DeclRef {
                owner: feature.id.to_string(),
                group: feature.group,
                decl,
            });
        }
    }
    for extra in extras {
        decls.push(DeclRef {
            owner: extra.owner.to_string(),
            group: extra.group,
            decl: extra.decl,
        });
    }
    decls
}

impl Snapshot {
    /// Resolve every declaration against the group inputs. `bootstrap` is the
    /// parsed `defiance-loader.ini`, used both as the legacy fallback for loader
    /// policy and as the compatibility source for third-party plugin sections.
    pub fn build(paths: Paths, inputs: &[GroupInput], bootstrap: &Document) -> Snapshot {
        Snapshot::build_with_extras(paths, inputs, bootstrap, &[])
    }

    /// As `build`, plus declarations contributed by third-party manifests.
    pub fn build_with_extras(
        paths: Paths,
        inputs: &[GroupInput],
        bootstrap: &Document,
        extras: &[Declared],
    ) -> Snapshot {
        let mut resolved: HashMap<(String, String), Resolved> = HashMap::new();
        let mut problems = Vec::new();
        let mut blocked: BTreeSet<String> = BTreeSet::new();
        let mut unknown = Vec::new();
        let mut warnings = Vec::new();
        let mut files = Vec::new();

        let declarations = declarations(extras);
        let sensitive: BTreeSet<(String, String)> = declarations
            .iter()
            .filter(|decl| decl.decl.sensitive)
            .map(|decl| (decl.owner.clone(), decl.decl.key.to_string()))
            .collect();
        let legacy = legacy_values(bootstrap);

        // Parse each group once.
        let mut parsed: HashMap<String, (Document, bool, bool)> = HashMap::new();
        for input in inputs {
            let (document, existed, usable) = match &input.text {
                None => (Document::default(), false, true),
                Some(Ok(text)) => {
                    let document = parse::parse(text);
                    let usable = document.issues.is_empty();
                    (document, true, usable)
                }
                Some(Err(_)) => (Document::default(), true, false),
            };
            files.push(GroupStatus {
                group: input.group.clone(),
                path: input.path.clone(),
                existed,
                usable,
            });
            parsed.insert(input.group.clone(), (document, existed, usable));
        }

        // The declared `(section, key)` pairs per group, for unknown detection.
        let mut known: HashMap<&str, HashSet<(String, String)>> = HashMap::new();
        for decl in &declarations {
            known
                .entry(decl.group)
                .or_default()
                .insert((decl.owner.clone(), decl.decl.key.to_string()));
        }
        for input in inputs {
            let Some((document, _, _)) = parsed.get(&input.group) else {
                continue;
            };
            let declared = known.get(input.group.as_str());
            for entry in &document.entries {
                let key = (entry.section.clone(), entry.key.clone());
                if declared.is_some_and(|set| set.contains(&key)) {
                    continue;
                }
                warnings.push(format!(
                    "{}:{}: unknown setting `{}` in section `{}` is kept",
                    input.path.display(),
                    entry.line,
                    entry.key,
                    if entry.section.is_empty() {
                        "(top level)"
                    } else {
                        &entry.section
                    }
                ));
                unknown.push(entry.clone());
            }
        }

        // Owners blocked because their whole file is unreadable or structurally
        // broken. Every declaration in that group is skipped.
        let mut blocked_groups: HashSet<String> = HashSet::new();
        for input in inputs {
            let Some((document, existed, usable)) = parsed.get(&input.group) else {
                continue;
            };
            let owners: BTreeSet<String> = declarations
                .iter()
                .filter(|decl| decl.group == input.group)
                .map(|decl| decl.owner.clone())
                .collect();
            if !*existed {
                continue;
            }
            if matches!(input.text, Some(Err(_))) {
                for owner in &owners {
                    problems.push(Problem {
                        owner: owner.clone(),
                        file: input.path.display().to_string(),
                        line: None,
                        message: "configuration file could not be read".to_string(),
                    });
                    blocked.insert(owner.clone());
                }
                blocked_groups.insert(input.group.clone());
            } else if !*usable {
                let reasons = document
                    .issues
                    .iter()
                    .map(|issue| format!("line {}: {}", issue.line, issue.message))
                    .collect::<Vec<_>>()
                    .join("; ");
                for owner in &owners {
                    problems.push(Problem {
                        owner: owner.clone(),
                        file: input.path.display().to_string(),
                        line: None,
                        message: format!("configuration is not parseable: {reasons}"),
                    });
                    blocked.insert(owner.clone());
                }
                blocked_groups.insert(input.group.clone());
            }
        }

        for decl in &declarations {
            if blocked_groups.contains(decl.group) {
                continue;
            }
            let Some(input) = inputs.iter().find(|input| input.group == decl.group) else {
                continue;
            };
            let Some((document, _, _)) = parsed.get(decl.group) else {
                continue;
            };
            let identity = (decl.owner.clone(), decl.decl.key.to_string());

            // Explicit group value wins over everything else.
            if let Some((entry, earlier)) = document.lookup(&decl.owner, decl.decl.key) {
                if !earlier.is_empty() {
                    let lines = earlier
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    warnings.push(format!(
                        "{}:{}: `{}` is declared again at line {} (earlier at {lines}); using line {}",
                        input.path.display(),
                        entry.line,
                        decl.decl.key,
                        entry.line,
                        entry.line
                    ));
                }
                // Precedence warning: say who won when both forms exist.
                if let Some(legacy_key) = builtin::legacy_bootstrap_key(&decl.owner, decl.decl.key)
                {
                    if let Some((bootstrap_entry, _)) = bootstrap.top(legacy_key) {
                        warnings.push(format!(
                            "{}:{}: `{}` is set in both the group file (line {}) and {} (line {}); the group file wins",
                            input.path.display(),
                            entry.line,
                            decl.decl.key,
                            entry.line,
                            paths.bootstrap.display(),
                            bootstrap_entry.line
                        ));
                    }
                }
                match validate(decl.decl, &entry.value) {
                    Ok(value) => {
                        resolved.insert(
                            identity,
                            Resolved {
                                value,
                                provenance: Provenance::Group {
                                    file: input.path.display().to_string(),
                                    line: entry.line,
                                },
                            },
                        );
                    }
                    Err(reason) => {
                        problems.push(Problem {
                            owner: decl.owner.clone(),
                            file: input.path.display().to_string(),
                            line: Some(entry.line),
                            message: format!("invalid value for `{}`: {reason}", decl.decl.key),
                        });
                        blocked.insert(decl.owner.clone());
                    }
                }
                continue;
            }

            // The supported legacy equivalent, for loader policy.
            if let Some(legacy_key) = builtin::legacy_bootstrap_key(&decl.owner, decl.decl.key) {
                if let Some((entry, _)) = bootstrap.top(legacy_key) {
                    match validate(decl.decl, &entry.value) {
                        Ok(value) => {
                            resolved.insert(
                                identity.clone(),
                                Resolved {
                                    value,
                                    provenance: Provenance::Bootstrap {
                                        file: paths.bootstrap.display().to_string(),
                                        line: entry.line,
                                    },
                                },
                            );
                        }
                        Err(reason) => {
                            problems.push(Problem {
                                owner: decl.owner.clone(),
                                file: paths.bootstrap.display().to_string(),
                                line: Some(entry.line),
                                message: format!(
                                    "invalid legacy value for `{}`: {reason}",
                                    decl.decl.key
                                ),
                            });
                            blocked.insert(decl.owner.clone());
                        }
                    }
                    continue;
                }
            }

            // Nothing set it: the declared default.
            if let Ok(value) = validate(decl.decl, decl.decl.default) {
                resolved.insert(
                    identity,
                    Resolved {
                        value,
                        provenance: Provenance::Default,
                    },
                );
            }
        }

        Snapshot {
            paths,
            resolved,
            legacy,
            problems,
            blocked,
            unknown,
            warnings,
            files,
            sensitive,
        }
    }

    /// The declared setting `(plugin_id, key)`, with provenance.
    pub fn get(&self, plugin_id: &str, key: &str) -> Option<&Resolved> {
        self.resolved
            .get(&(plugin_id.to_ascii_lowercase(), key.to_ascii_lowercase()))
    }

    /// The provenance of `(plugin_id, key)` as text, for a startup log that
    /// names where an effective value came from.
    pub fn provenance(&self, plugin_id: &str, key: &str) -> Option<String> {
        self.get(plugin_id, key)
            .map(|resolved| provenance(&resolved.provenance))
    }

    /// Whether a gameplay feature is explicitly or by default enabled. `None`
    /// when the plugin is not a declared feature.
    pub fn enabled(&self, plugin_id: &str) -> Option<bool> {
        self.get(plugin_id, "enabled")
            .and_then(|resolved| resolved.value.as_bool())
    }

    pub fn integer(&self, plugin_id: &str, key: &str) -> Option<i64> {
        self.get(plugin_id, key)
            .and_then(|resolved| resolved.value.as_integer())
    }

    /// A textual loader setting, by `[loader]`/`[logging]` key.
    pub fn text(&self, section: &str, key: &str) -> Option<String> {
        self.get(section, key).map(Resolved::canonical)
    }

    /// A legacy `[section] key` from the bootstrap file, last value wins. This
    /// keeps third-party ABI 5 plugins working before they ship a manifest.
    pub fn legacy(&self, section: &str, key: &str) -> Option<&str> {
        self.legacy
            .get(&(section.to_ascii_lowercase(), key.to_ascii_lowercase()))
            .map(String::as_str)
    }

    /// Whether an owner is blocked by an invalid or unreadable setting.
    pub fn is_blocked(&self, owner: &str) -> bool {
        self.blocked.contains(owner)
    }

    /// Shared configuration failures gate the entire host, including legacy DLLs.
    pub fn startup_error(&self) -> Option<String> {
        [LOADER_SECTION, LOGGING_SECTION]
            .into_iter()
            .find(|owner| self.is_blocked(owner))
            .map(|owner| {
                let reasons = self
                    .problems
                    .iter()
                    .filter(|problem| problem.owner == owner)
                    .map(|problem| problem.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                format!("startup blocked by invalid {owner} configuration: {reasons}")
            })
    }

    /// A human-readable effective configuration: the resolved paths, every
    /// resolved setting with its value and provenance, then every blocked
    /// owner, validation failure and preserved unknown setting. This is an
    /// explicit diagnostic; the startup log reports provenance and failures
    /// without dumping values.
    pub fn report(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "root     {}\nconfig   {}\nplugins  {}\nlogs     {}\n",
            self.paths.root.display(),
            self.paths.config_dir.display(),
            self.paths.plugin_dir.display(),
            self.paths.log_dir.display()
        ));
        let mut rows: Vec<((String, String), &Resolved)> = self
            .resolved
            .iter()
            .map(|(key, value)| (key.clone(), value))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        for ((owner, key), resolved) in rows {
            let redacted = self.sensitive.contains(&(owner.clone(), key.clone()));
            let value = if redacted {
                "<redacted>".to_string()
            } else {
                resolved.canonical()
            };
            out.push_str(&format!(
                "{owner}.{key} = {value} [{}]\n",
                provenance(&resolved.provenance)
            ));
        }
        for problem in &self.problems {
            out.push_str(&format!(
                "problem {owner}: {message} ({file}{line})\n",
                owner = problem.owner,
                message = problem.message,
                file = problem.file,
                line = problem
                    .line
                    .map(|line| format!(":{line}"))
                    .unwrap_or_default()
            ));
        }
        for owner in &self.blocked {
            out.push_str(&format!("blocked {owner}\n"));
        }
        for entry in &self.unknown {
            let section = if entry.section.is_empty() {
                "(top level)"
            } else {
                &entry.section
            };
            out.push_str(&format!("unknown {section}.{} (kept)\n", entry.key));
        }
        out
    }
}

fn provenance(provenance: &Provenance) -> String {
    match provenance {
        Provenance::Group { file, line } => format!("{file}:{line}"),
        Provenance::Bootstrap { file, line } => format!("bootstrap {file}:{line}"),
        Provenance::Default => "default".to_string(),
    }
}

/// The bootstrap's sectioned entries, last value wins, lower-cased on both.
/// The top-level keys are loader policy and are not part of the plugin-facing
/// compatibility surface.
fn legacy_values(bootstrap: &Document) -> HashMap<(String, String), String> {
    let mut values = HashMap::new();
    for entry in &bootstrap.entries {
        if entry.section.is_empty() {
            continue;
        }
        values.insert(
            (entry.section.clone(), entry.key.clone()),
            entry.value.clone(),
        );
    }
    values
}

#[cfg(test)]
mod tests {
    use super::super::builtin::{CORE_ID, LOADER_SECTION};
    use super::super::parse::parse;
    use super::super::paths::Paths;
    use super::*;
    use std::path::PathBuf;

    fn paths() -> Paths {
        Paths::resolve(&PathBuf::from("C:/Game/bin"), &parse("")).0
    }

    fn build(groups: &[(&str, Option<&str>)], bootstrap: &str) -> Snapshot {
        let inputs: Vec<GroupInput> = groups
            .iter()
            .map(|(group, text)| {
                let path = PathBuf::from(format!("C:/Game/DefianceLoader/config/{group}.ini"));
                match text {
                    Some(text) => GroupInput::present(group, path, text),
                    None => GroupInput::missing(group, path),
                }
            })
            .collect();
        Snapshot::build(paths(), &inputs, &parse(bootstrap))
    }

    fn all_groups() -> Vec<(&'static str, Option<&'static str>)> {
        vec![
            (
                "core",
                Some("[loader]\nwait = 60\nallow_unknown_build = false\n[logging]\nlevel = info\n"),
            ),
            (
                "infantry",
                Some("[defiance.selection]\nenabled = true\n[defiance.movement]\nenabled = true\n"),
            ),
            ("weapons", Some("[defiance.firing]\nenabled = true\n")),
            (
                "diagnostics",
                Some("[defiance.diagnostics]\nenabled = true\n"),
            ),
        ]
    }

    #[test]
    fn missing_files_use_the_declared_defaults_with_provenance() {
        let snapshot = build(
            &[
                ("core", None),
                ("infantry", None),
                ("weapons", None),
                ("diagnostics", None),
            ],
            "",
        );
        assert_eq!(snapshot.enabled("defiance.selection"), Some(true));
        assert_eq!(
            snapshot
                .get("defiance.selection", "enabled")
                .unwrap()
                .provenance,
            Provenance::Default
        );
        assert_eq!(snapshot.integer(LOADER_SECTION, "wait"), Some(60));
        assert!(snapshot.problems.is_empty());
    }

    #[test]
    fn explicit_values_win_and_report_their_line() {
        let snapshot = build(&all_groups(), "");
        let resolved = snapshot.get("defiance.movement", "enabled").unwrap();
        assert_eq!(
            resolved.provenance,
            Provenance::Group {
                file: "C:/Game/DefianceLoader/config/infantry.ini".into(),
                line: 4,
            }
        );
    }

    #[test]
    fn an_invalid_explicit_value_blocks_its_owner() {
        let mut groups = all_groups();
        groups[1].1 =
            Some("[defiance.selection]\nenabled = maybe\n[defiance.movement]\nenabled = true\n");
        let snapshot = build(&groups, "");
        assert!(snapshot.is_blocked("defiance.selection"));
        assert_eq!(snapshot.enabled("defiance.selection"), None);
        assert_eq!(snapshot.problems.len(), 1);
        assert!(snapshot.problems[0].message.contains("invalid value"));
        // The unrelated feature is untouched.
        assert_eq!(snapshot.enabled("defiance.movement"), Some(true));
    }

    #[test]
    fn duplicate_settings_keep_the_last_and_warn() {
        let snapshot = build(
            &[(
                "infantry",
                Some("[defiance.selection]\nenabled = false\nenabled = true\n"),
            )],
            "",
        );
        assert_eq!(snapshot.enabled("defiance.selection"), Some(true));
        assert!(snapshot
            .warnings
            .iter()
            .any(|w| w.contains("declared again")));
    }

    #[test]
    fn legacy_bootstrap_values_fill_loader_policy_and_plugin_sections() {
        let snapshot = build(
            &[
                ("core", None),
                ("infantry", None),
                ("weapons", None),
                ("diagnostics", None),
            ],
            "wait = 15\nallow_unknown_build = yes\n[defiance.example]\ngreeting = hello\n",
        );
        assert_eq!(snapshot.integer(LOADER_SECTION, "wait"), Some(15));
        assert_eq!(
            snapshot.get(LOADER_SECTION, "wait").unwrap().provenance,
            Provenance::Bootstrap {
                file: PathBuf::from("C:/Game/bin")
                    .join("defiance-loader.ini")
                    .display()
                    .to_string(),
                line: 1
            }
        );
        assert_eq!(
            snapshot
                .text(LOADER_SECTION, "allow_unknown_build")
                .unwrap(),
            "true"
        );
        assert_eq!(
            snapshot.legacy("defiance.example", "greeting"),
            Some("hello")
        );
        // The bootstrap has no plugin enablement, so the default still applies.
        assert_eq!(snapshot.enabled("defiance.selection"), Some(true));
    }

    #[test]
    fn both_forms_warn_and_the_group_file_wins() {
        let snapshot = build(&[("core", Some("[loader]\nwait = 5\n"))], "wait = 15\n");
        assert_eq!(snapshot.integer(LOADER_SECTION, "wait"), Some(5));
        assert!(snapshot
            .warnings
            .iter()
            .any(|w| w.contains("group file wins")));
    }

    #[test]
    fn unknown_settings_are_kept_and_warned_about() {
        let snapshot = build(
            &[(
                "infantry",
                Some("[defiance.selection]\nenabled = true\nfuture = 1\n[vendor.extra]\nkey = v\n"),
            )],
            "",
        );
        assert_eq!(snapshot.unknown.len(), 2);
        assert_eq!(snapshot.unknown[0].key, "future");
        assert_eq!(snapshot.unknown[1].section, "vendor.extra");
        assert!(
            snapshot
                .warnings
                .iter()
                .filter(|w| w.contains("unknown setting"))
                .count()
                == 2
        );
    }

    #[test]
    fn an_unparseable_file_blocks_every_owner_in_its_group_but_not_others() {
        let mut groups = all_groups();
        groups[1].1 = Some("[defiance.selection\nenabled = true\n");
        let snapshot = build(&groups, "");
        assert!(snapshot.is_blocked("defiance.selection"));
        assert!(snapshot.is_blocked("defiance.movement"));
        assert!(!snapshot.is_blocked("defiance.firing"));
        assert_eq!(snapshot.enabled("defiance.firing"), Some(true));
    }

    #[test]
    fn an_unreadable_file_blocks_its_group_and_is_not_missing() {
        let inputs = vec![
            GroupInput::unreadable("infantry", PathBuf::from("C:/x/infantry.ini"), "denied"),
            GroupInput::present(
                "core",
                PathBuf::from("C:/x/core.ini"),
                "[loader]\nwait = 3\n",
            ),
        ];
        let snapshot = Snapshot::build(paths(), &inputs, &parse(""));
        assert!(snapshot.is_blocked("defiance.selection"));
        assert!(snapshot
            .problems
            .iter()
            .any(|p| p.message.contains("could not be read")));
        assert!(
            !snapshot
                .files
                .iter()
                .find(|f| f.group == "infantry")
                .unwrap()
                .usable
        );
    }

    #[test]
    fn logging_level_is_validated_as_a_choice() {
        let snapshot = build(&[("core", Some("[logging]\nlevel = DEBUG\n"))], "");
        assert_eq!(snapshot.text("logging", "level").unwrap(), "debug");
        let snapshot = build(&[("core", Some("[logging]\nlevel = chatty\n"))], "");
        assert!(snapshot.is_blocked("logging"));
        assert_eq!(snapshot.text("logging", "level"), None);
    }

    #[test]
    fn core_is_never_a_toggle() {
        let snapshot = build(&all_groups(), "");
        assert_eq!(snapshot.enabled(CORE_ID), None);
        assert!(!snapshot.is_blocked(CORE_ID));
    }
}
