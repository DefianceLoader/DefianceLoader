//! Configuration schema version and the explicit migration operation.
//!
//! The config schema version is not the plugin ABI and not the product version.
//! It lives in a Core-owned metadata file under the config directory, never in
//! a plugin's own namespace.
//!
//! Migration is explicit, idempotent, previewable and backed up. A plan is
//! computed from the current bytes; applying it writes every file through a
//! temporary file plus replace, keeping a `.defiance-backup` copy first, and
//! only records the new schema version once every write has succeeded. If a
//! write fails, the files already written are put back, and reruns leave
//! explicit new values alone because the plan is recomputed from disk.

use super::parse::{self, Document};
use std::io;
use std::path::{Path, PathBuf};

/// The configuration schema this loader understands.
pub const SCHEMA_VERSION: u32 = 1;
pub const METADATA_FILE: &str = "defiance-config.ini";

/// The core-owned metadata, as read from disk or about to be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub schema_version: u32,
}

impl Default for State {
    fn default() -> Self {
        State {
            schema_version: SCHEMA_VERSION,
        }
    }
}

/// Read the metadata file. A missing file is an initial state of version 0,
/// distinct from "could not read", which the caller reports.
pub fn read_state(config_dir: &Path) -> io::Result<State> {
    let path = config_dir.join(METADATA_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let document = parse::parse(&text);
            if !document.issues.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "config metadata is not parseable",
                ));
            }
            let schema_version = document
                .lookup("config", "schema_version")
                .and_then(|(entry, _)| entry.value.trim().parse().ok())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "config metadata has no valid schema_version",
                    )
                })?;
            Ok(State { schema_version })
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(State { schema_version: 0 }),
        Err(e) => Err(e),
    }
}

/// Write the metadata atomically. Never leaves a half-written file.
pub fn write_state(config_dir: &Path, state: &State) -> io::Result<()> {
    let path = config_dir.join(METADATA_FILE);
    let text = format!(
        "; Written by DefianceLoader. Do not edit.\n[config]\nschema_version = {}\n",
        state.schema_version
    );
    replace_file(&path, text.as_bytes())
}

/// One file to change, with the exact bytes before and after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: PathBuf,
    pub before: Option<String>,
    pub after: String,
}

impl Change {
    pub fn changed(&self) -> bool {
        self.before.as_deref() != Some(self.after.as_str())
    }
}

/// A preview of a migration: the changes to write, or a refusal.
#[derive(Debug, Clone)]
pub struct Plan {
    pub changes: Vec<Change>,
    pub refuse: Option<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.refuse.is_none() && self.changes.iter().all(|change| !change.changed())
    }
}

/// Whether the loader can migrate from `state`. A newer schema is never
/// downgraded or rewritten.
pub fn check_supported(state: &State) -> Result<(), String> {
    if state.schema_version > SCHEMA_VERSION {
        return Err(format!(
            "configuration schema {} is newer than this loader's {SCHEMA_VERSION}; \
             not touching it",
            state.schema_version
        ));
    }
    Ok(())
}

/// Plan the move of legacy bootstrap loader keys into `[loader]` of the core
/// group file. It is idempotent: a key already written explicitly is left
/// alone, and a key with no legacy value is left to the declared default.
pub fn plan_loader_keys(bootstrap: &Document, core_path: &Path, core_text: Option<&str>) -> Plan {
    let existing = core_text.unwrap_or("");
    let document = parse::parse(existing);
    let mut after = existing.to_string();
    if !after.is_empty() && !after.ends_with('\n') {
        after.push('\n');
    }
    let mut changed = false;
    for decl in super::builtin::LOADER_SETTINGS {
        if document
            .lookup(super::builtin::LOADER_SECTION, decl.key)
            .is_some()
        {
            continue;
        }
        let Some((entry, _)) = bootstrap.top(decl.key) else {
            continue;
        };
        after = super::defaults::insert_setting(
            &after,
            super::builtin::LOADER_SECTION,
            decl.key,
            &entry.value,
            decl.description,
            decl.default,
        );
        changed = true;
    }
    Plan {
        changes: vec![Change {
            path: core_path.to_path_buf(),
            before: core_text.map(str::to_string),
            after,
        }],
        refuse: None,
    }
    .normalize(changed)
}

/// Plan removing a legacy `plugins` override that resolves to the default
/// `root/plugins`: it changes nothing about discovery, but the loader warns
/// about every override at startup. Conservative on purpose: `None` unless the
/// bootstrap declares `plugins` exactly once and its resolved path equals the
/// default exactly. A duplicate, or an override pointing anywhere else, is the
/// user's to change. Only that one line goes; comments, the BOM, line endings
/// and every other key are kept.
pub fn plan_default_plugins_override(
    paths: &super::paths::Paths,
    bootstrap: &Document,
    bootstrap_text: &str,
) -> Option<Change> {
    let (entry, earlier) = bootstrap.top("plugins")?;
    if !earlier.is_empty()
        || !paths.plugin_override
        || paths.plugin_dir != paths.root.join(super::paths::DEFAULT_PLUGINS)
    {
        return None;
    }
    let body = parse::strip_bom(bootstrap_text);
    let bom = &bootstrap_text[..bootstrap_text.len() - body.len()];
    let mut after = bom.to_string();
    for (index, line) in body.split_inclusive('\n').enumerate() {
        if index + 1 != entry.line {
            after.push_str(line);
        }
    }
    Some(Change {
        path: paths.bootstrap.clone(),
        before: Some(bootstrap_text.to_string()),
        after,
    })
}

impl Plan {
    fn normalize(mut self, changed: bool) -> Plan {
        if !changed {
            self.changes.clear();
        }
        self
    }
}

/// Why a migration stopped, and what rollback could not undo.
///
/// A caller must not report a clean rollback when `restoration_failures` is
/// non-empty: `Display` names the file that failed and every file that could
/// not be put back, with the restore error.
#[derive(Debug)]
pub struct ApplyError {
    /// The file whose write failed, or `None` when the plan was refused.
    pub failed: Option<PathBuf>,
    /// The write failure itself.
    pub source: io::Error,
    /// Earlier files whose previous state could not be restored, with why.
    pub restoration_failures: Vec<(PathBuf, io::Error)>,
}

impl ApplyError {
    fn refused(reason: String) -> ApplyError {
        ApplyError {
            failed: None,
            source: io::Error::new(io::ErrorKind::InvalidInput, reason),
            restoration_failures: Vec::new(),
        }
    }

    /// Whether every earlier file was put back, so the transaction is clean.
    pub fn rolled_back(&self) -> bool {
        self.restoration_failures.is_empty()
    }
}

impl core::fmt::Display for ApplyError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.failed {
            Some(path) => write!(formatter, "{}: {}", path.display(), self.source)?,
            None => write!(formatter, "{}", self.source)?,
        }
        if !self.restoration_failures.is_empty() {
            write!(formatter, "; rollback incomplete:")?;
            for (path, error) in &self.restoration_failures {
                write!(formatter, " {} ({error})", path.display())?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for ApplyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Apply a plan, backing up and replacing each file. On failure the files
/// already written are put back; the error carries the original failure and
/// every restoration that did not succeed, so a caller never implies a fully
/// restored transaction.
pub fn apply(plan: &Plan) -> Result<(), ApplyError> {
    apply_with(plan, write_change, |change| {
        write_change_raw(&change.path, change.before.as_deref())
    })
}

/// The apply loop, with the write and restore operations injectable so tests
/// can force a failure at either boundary.
fn apply_with(
    plan: &Plan,
    mut write: impl FnMut(&Change) -> io::Result<()>,
    mut restore: impl FnMut(&Change) -> io::Result<()>,
) -> Result<(), ApplyError> {
    if let Some(reason) = &plan.refuse {
        return Err(ApplyError::refused(reason.clone()));
    }
    let mut written: Vec<&Change> = Vec::new();
    for change in &plan.changes {
        if !change.changed() {
            continue;
        }
        if let Err(source) = write(change) {
            let mut restoration_failures = Vec::new();
            for done in written.iter().rev() {
                if let Err(error) = restore(done) {
                    restoration_failures.push((done.path.clone(), error));
                }
            }
            return Err(ApplyError {
                failed: Some(change.path.clone()),
                source,
                restoration_failures,
            });
        }
        written.push(change);
    }
    Ok(())
}

fn write_change(change: &Change) -> io::Result<()> {
    backup(&change.path)?;
    write_change_raw(&change.path, Some(&change.after))
}

/// A best-effort `.defiance-backup` copy before the first change to a file.
fn backup(path: &Path) -> io::Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let backup = path.with_extension("defiance-backup");
    if !backup.exists() {
        std::fs::copy(path, &backup)?;
    }
    Ok(())
}

/// Write `text` (or remove the file when `None`), through a temporary file and
/// a replace.
fn write_change_raw(path: &Path, text: Option<&str>) -> io::Result<()> {
    match text {
        Some(text) => replace_file(path, text.as_bytes()),
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
    }
}

fn replace_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse::parse;

    #[test]
    fn missing_state_is_version_zero_and_writes_back() {
        let dir = unique_dir("state");
        assert_eq!(read_state(&dir).unwrap().schema_version, 0);
        write_state(
            &dir,
            &State {
                schema_version: SCHEMA_VERSION,
            },
        )
        .unwrap();
        assert_eq!(read_state(&dir).unwrap().schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn a_newer_schema_is_refused() {
        assert!(check_supported(&State {
            schema_version: SCHEMA_VERSION + 1
        })
        .is_err());
        assert!(check_supported(&State::default()).is_ok());
    }

    #[test]
    fn planning_is_idempotent_and_preserves_explicit_values() {
        let bootstrap = parse("wait = 15\nallow_unknown_build = yes\n");
        let path = PathBuf::from("C:/cfg/core.ini");
        let plan = plan_loader_keys(&bootstrap, &path, Some("[loader]\nwait = 5\n"));
        assert!(!plan.is_empty());
        let after = &plan.changes[0].after;
        assert!(after.contains("wait = 5"), "{after}");
        assert!(after.contains("allow_unknown_build = yes"), "{after}");
        // Replanning against the new text changes nothing.
        let again = plan_loader_keys(&bootstrap, &path, Some(after));
        assert!(again.is_empty(), "not idempotent: {again:?}");
    }

    #[test]
    fn no_legacy_keys_means_no_plan() {
        let plan = plan_loader_keys(
            &parse("root = ../X\n"),
            &PathBuf::from("C:/cfg/core.ini"),
            None,
        );
        assert!(plan.is_empty());
    }

    fn override_plan(text: &str) -> Option<Change> {
        let bootstrap = parse(text);
        let (paths, _) =
            crate::config::paths::Paths::resolve(&PathBuf::from("C:/Game/bin"), &bootstrap);
        plan_default_plugins_override(&paths, &bootstrap, text)
    }

    #[test]
    fn a_plugins_override_equal_to_the_default_is_removed_alone() {
        let text = "\u{feff}; mine\r\nwait = 15\r\nplugins = ../DefianceLoader/plugins/\r\nroot = ../DefianceLoader\r\n";
        let change = override_plan(text).expect("the default override is removed");
        assert_eq!(
            change.path,
            PathBuf::from("C:/Game/bin/defiance-loader.ini")
        );
        assert_eq!(
            change.after,
            "\u{feff}; mine\r\nwait = 15\r\nroot = ../DefianceLoader\r\n"
        );
        // On the first line, without a trailing newline, the BOM stays.
        let first = override_plan("\u{feff}plugins = ../DefianceLoader/plugins").unwrap();
        assert_eq!(first.after, "\u{feff}");
        // Replanning against the result changes nothing.
        assert!(override_plan(&change.after).is_none());
    }

    #[test]
    fn other_plugins_overrides_are_left_alone() {
        for text in [
            "wait = 15\n",
            "plugins = ../old/plugins\n",
            // Default for the default root, but `root` moved elsewhere.
            "root = D:/Mods/Defiance\nplugins = ../DefianceLoader/plugins\n",
            // Declared twice: which one the user means is theirs to settle.
            "plugins = ../DefianceLoader/plugins\nplugins = ../DefianceLoader/plugins\n",
        ] {
            assert!(override_plan(text).is_none(), "{text:?}");
        }
    }

    #[test]
    fn apply_writes_then_rolls_back_on_failure() {
        let dir = unique_dir("apply");
        let good = dir.join("core.ini");
        std::fs::write(&good, "[loader]\nwait = 60\n").unwrap();
        let bad = dir.join("blocked");
        std::fs::create_dir(&bad).unwrap();
        let plan = Plan {
            changes: vec![
                Change {
                    path: good.clone(),
                    before: Some("[loader]\nwait = 60\n".into()),
                    after: "[loader]\nwait = 15\n".into(),
                },
                Change {
                    path: bad.clone(),
                    before: None,
                    after: "x = 1\n".into(),
                },
            ],
            refuse: None,
        };
        let error = apply(&plan).unwrap_err();
        assert_eq!(error.failed.as_deref(), Some(bad.as_path()));
        assert!(
            error.rolled_back(),
            "the earlier file was restored: {error}"
        );
        // The first file was restored to its original bytes.
        assert_eq!(
            std::fs::read_to_string(&good).unwrap(),
            "[loader]\nwait = 60\n"
        );
    }

    #[test]
    fn rollback_failures_are_reported_with_their_paths() {
        let dir = unique_dir("rollback");
        let first = dir.join("core.ini");
        let second = dir.join("infantry.ini");
        let plan = Plan {
            changes: vec![
                Change {
                    path: first.clone(),
                    before: Some("old\n".into()),
                    after: "new\n".into(),
                },
                Change {
                    path: second.clone(),
                    before: None,
                    after: "x\n".into(),
                },
            ],
            refuse: None,
        };
        // The second write fails; putting the first back then fails too. The
        // error must carry both facts, not imply a clean rollback.
        let error = apply_with(
            &plan,
            |change| {
                if change.path == second {
                    Err(io::Error::new(
                        io::ErrorKind::Other,
                        "injected write failure",
                    ))
                } else {
                    Ok(())
                }
            },
            |change| {
                if change.path == first {
                    Err(io::Error::new(
                        io::ErrorKind::Other,
                        "injected restore failure",
                    ))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(error.failed.as_deref(), Some(second.as_path()));
        assert!(!error.rolled_back());
        assert_eq!(error.restoration_failures.len(), 1);
        assert_eq!(error.restoration_failures[0].0, first);
        let text = error.to_string();
        assert!(text.contains("injected write failure"), "{text}");
        assert!(text.contains("rollback incomplete"), "{text}");
        assert!(text.contains(first.to_string_lossy().as_ref()), "{text}");
    }

    fn unique_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "defiance-config-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }
}
