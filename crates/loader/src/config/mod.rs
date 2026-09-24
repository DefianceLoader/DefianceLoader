//! Configuration: where the loader's files live, what the settings are, and
//! the one immutable snapshot every reader shares.
//!
//! `defiance-loader.ini` beside the executable is bootstrap only (`root`, and
//! the legacy `plugins` override). Everything else lives under `root`:
//! `config/<group>.ini`, `logs/`, `plugins/`. Core parses and validates; a
//! plugin only declares settings. The plugin-facing contract is in
//! `docs/plugin-api.md`.

pub mod builtin;
pub mod defaults;
pub mod migration;
pub mod parse;
pub mod paths;
pub mod schema;
pub mod snapshot;

pub use paths::Paths;
pub use snapshot::{Declared, GroupInput, Snapshot};

use super::manifest;
use crate::config::schema::{Restart, SettingDecl, ValueType};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const BOOTSTRAP_DEFAULTS: &str = "\
; Bootstrap for the DefianceLoader. This file is read beside trm.exe before
; anything else. Everything else lives under `root`.

; The loader's data directory, relative to this executable's directory.
; Default: ../DefianceLoader
root = ../DefianceLoader

; Optional legacy override for the plugin directory. Relative paths are
; resolved against this executable's directory, not `root`. Leave it out to use
; root/plugins. Uncomment to keep an existing bin/plugins installation:
; Default: <root>/plugins (no override)
; plugins = plugins

";

static SNAPSHOT: OnceLock<Snapshot> = OnceLock::new();

/// The executable directory (bin), independent of the working directory.
pub fn game_dir() -> PathBuf {
    let mut buffer = vec![0u16; 1024];
    let length = unsafe {
        crate::win::GetModuleFileNameW(
            core::ptr::null_mut(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    let length = length.min(buffer.len());
    let exe = PathBuf::from(crate::win::from_wide(&buffer[..length]));
    exe.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Read one group file. `None` means it does not exist; `Some(Err(..))` means
/// it exists but could not be read or decoded (which is not the same thing).
fn read_group(path: &Path) -> Option<Result<String, String>> {
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(Err(e.to_string())),
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Some(Ok(text)),
            Err(_) => Some(Err("file is not valid UTF-8".into())),
        },
    }
}

/// Everything reading the files produced, before validation.
struct Discovered {
    paths: Paths,
    bootstrap: parse::Document,
    inputs: Vec<GroupInput>,
    extras: Vec<Declared>,
    failures: Vec<String>,
    catalog: manifest::Catalog,
}

/// Load the whole configuration, once. Writes a missing bootstrap and missing
/// group files with commented defaults; never rewrites an existing file.
pub fn load() -> &'static Snapshot {
    SNAPSHOT.get_or_init(load_once)
}

/// Read the bootstrap and every group file. With `generate`, a missing file is
/// written with its commented defaults; without it (the read-only inspection a
/// report uses), a missing file simply becomes an in-memory default and nothing
/// on disk is touched.
fn read(exe_dir: &Path, generate: bool) -> Discovered {
    let bootstrap_path = exe_dir.join(paths::BOOTSTRAP_FILE);
    let mut failures = Vec::new();
    let mut missing_bootstrap = false;
    let bootstrap_text = match std::fs::read_to_string(&bootstrap_path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            missing_bootstrap = true;
            BOOTSTRAP_DEFAULTS.to_string()
        }
        Err(e) => {
            failures.push(format!("could not read {}: {e}", bootstrap_path.display()));
            String::new()
        }
    };
    let bootstrap = parse::parse(&bootstrap_text);
    for issue in &bootstrap.issues {
        crate::log::warn(&format!(
            "{}:{}: {}",
            bootstrap_path.display(),
            issue.line,
            issue.message
        ));
    }
    let (paths, warnings) = Paths::resolve(exe_dir, &bootstrap);
    for warning in &warnings {
        crate::log::warn(warning);
    }
    if !bootstrap.issues.is_empty() {
        failures.push("bootstrap configuration is not parseable".into());
    }
    match migration::read_state(&paths.config_dir) {
        Ok(state) => {
            if let Err(reason) = migration::check_supported(&state) {
                failures.push(reason);
            }
        }
        Err(e) => failures.push(format!("could not read config metadata: {e}")),
    }
    // Do not create or interpret configuration owned by an unsupported schema.
    if !failures.is_empty() {
        return Discovered {
            catalog: manifest::catalog(&paths.plugin_dir),
            paths,
            bootstrap,
            inputs: Vec::new(),
            extras: Vec::new(),
            failures,
        };
    }
    if generate && missing_bootstrap {
        if let Err(e) = std::fs::write(&bootstrap_path, &bootstrap_text) {
            crate::log::warn(&format!(
                "could not write bootstrap defaults: {e}; using in-memory defaults"
            ));
        }
    }
    if generate {
        if let Err(e) = std::fs::create_dir_all(&paths.config_dir) {
            crate::log::warn(&format!(
                "could not create {}: {e}; defaults are in memory only",
                paths.config_dir.display()
            ));
        }
    }

    // Third-party managed plugins declare their settings and config group in
    // their manifests; core resolves them from the same group files as the
    // built-ins, so a shared group's sections all live in one file.
    let catalog = manifest::catalog(&paths.plugin_dir);
    let (extras, extra_groups) = manifest_declarations(&catalog);
    let mut groups: Vec<String> = builtin::GROUPS
        .iter()
        .map(|group| (*group).to_string())
        .collect();
    for group in extra_groups {
        if !groups.contains(&group) {
            groups.push(group);
        }
    }

    let mut inputs = Vec::new();
    for group in &groups {
        let group = group.as_str();
        let path = paths.config_dir.join(format!("{group}.ini"));
        let input = match read_group(&path) {
            None if !generate => GroupInput::missing(group, path.clone()),
            None => {
                let defaults = complete_group("", group, &bootstrap, &extras);
                match save_defaults(&path, None, &defaults) {
                    Ok(()) => {
                        crate::log::info(&format!("wrote {} with the defaults", path.display()));
                        GroupInput::present(group, path.clone(), &defaults)
                    }
                    Err(e) => {
                        crate::log::warn(&format!(
                            "could not write {}: {e}; using in-memory defaults (unsaved)",
                            path.display()
                        ));
                        GroupInput::missing(group, path.clone())
                    }
                }
            }
            Some(Ok(text)) => {
                let after = if generate {
                    complete_group(&text, group, &bootstrap, &extras)
                } else {
                    text.clone()
                };
                if after != text {
                    match save_defaults(&path, Some(&text), &after) {
                        Ok(()) => {
                            crate::log::info(&format!(
                                "added missing settings to {}",
                                path.display()
                            ));
                            GroupInput::present(group, path.clone(), &after)
                        }
                        Err(e) => {
                            crate::log::warn(&format!(
                                "could not extend {}: {e}; missing defaults remain in memory",
                                path.display()
                            ));
                            GroupInput::present(group, path.clone(), &text)
                        }
                    }
                } else {
                    GroupInput::present(group, path.clone(), &text)
                }
            }
            Some(Err(reason)) => {
                crate::log::error(&format!("{reason}: {}", path.display()));
                GroupInput::unreadable(group, path.clone(), &reason)
            }
        };
        inputs.push(input);
    }
    Discovered {
        paths,
        bootstrap,
        inputs,
        extras,
        failures,
        catalog,
    }
}

/// Turn every third-party manifest found by the shared catalog into core
/// declarations and group names. An invalid or orphan manifest contributes
/// nothing; the catalog reports those to the planner. Strings are leaked once,
/// for the process's life.
fn manifest_declarations(catalog: &manifest::Catalog) -> (Vec<Declared>, Vec<String>) {
    let mut extras = Vec::new();
    let mut groups = BTreeSet::new();
    for entry in &catalog.entries {
        let Some(manifest) = &entry.manifest else {
            continue;
        };
        // Built-ins come from the authoritative table, not the packaged file.
        if builtin::find(&manifest.id).is_some() || !builtin::safe_group(&manifest.group) {
            continue;
        }
        groups.insert(manifest.group.clone());
        for setting in &manifest.settings {
            extras.push(leak_setting(&manifest.id, &manifest.group, setting));
        }
    }
    (extras, groups.into_iter().collect())
}

fn leak_setting(owner: &str, group: &str, setting: &manifest::ManifestSetting) -> Declared {
    let ty = match setting.ty.as_str() {
        "bool" => ValueType::Bool,
        "int" => ValueType::Integer {
            min: i64::MIN,
            max: i64::MAX,
        },
        "number" => ValueType::Number {
            min: f64::MIN,
            max: f64::MAX,
        },
        // A manifest choice has no static spelling list in core; treat it as
        // text. The initial schema only ships booleans.
        _ => ValueType::Text,
    };
    let decl = SettingDecl {
        key: Box::leak(setting.key.clone().into_boxed_str()),
        ty,
        default: Box::leak(setting.default.clone().into_boxed_str()),
        description: Box::leak(setting.description.clone().into_boxed_str()),
        restart: Restart::Startup,
        sensitive: setting.sensitive,
    };
    Declared {
        owner: Box::leak(owner.to_string().into_boxed_str()),
        group: Box::leak(group.to_string().into_boxed_str()),
        decl: Box::leak(Box::new(decl)),
    }
}

/// One declaration path for new files and upgrades, including shared groups.
fn complete_group(
    existing: &str,
    group: &str,
    bootstrap: &parse::Document,
    extras: &[Declared],
) -> String {
    let mut declared: Vec<(&str, &SettingDecl)> = defaults::blocks(group)
        .into_iter()
        .flat_map(|(owner, settings)| settings.iter().map(move |decl| (owner, decl)))
        .collect();
    declared.extend(
        extras
            .iter()
            .filter(|e| e.group == group)
            .map(|e| (e.owner, e.decl)),
    );
    let initial = if existing.is_empty() {
        format!("; DefianceLoader configuration: {group}.\n; Edit, save, and restart the game to apply changes.\n")
    } else {
        existing.to_string()
    };
    defaults::extend_missing(&initial, &declared, bootstrap)
}

fn save_defaults(
    path: &Path,
    before: Option<&str>,
    after: &str,
) -> Result<(), migration::ApplyError> {
    migration::apply(&migration::Plan {
        changes: vec![migration::Change {
            path: path.to_path_buf(),
            before: before.map(str::to_string),
            after: after.to_string(),
        }],
        refuse: None,
    })
}

fn build_snapshot(
    paths: Paths,
    inputs: &[GroupInput],
    bootstrap: &parse::Document,
    extras: &[Declared],
    failures: &[String],
    catalog: manifest::Catalog,
) -> Snapshot {
    let mut snapshot = Snapshot::build_with_extras(paths, inputs, bootstrap, extras);
    snapshot.catalog = catalog;
    for message in failures {
        snapshot.blocked.insert(builtin::LOADER_SECTION.into());
        snapshot.problems.push(snapshot::Problem {
            owner: builtin::LOADER_SECTION.into(),
            file: snapshot.paths.config_dir.display().to_string(),
            line: None,
            message: message.clone(),
        });
    }
    snapshot
}

fn discover(exe_dir: &Path) -> Discovered {
    read(exe_dir, true)
}

/// Build the configuration snapshot from the files as they are, touching
/// nothing. Used by the diagnostic report.
pub fn inspect(exe_dir: &Path) -> Snapshot {
    let Discovered {
        paths,
        bootstrap,
        inputs,
        extras,
        failures,
        catalog,
    } = read(exe_dir, false);
    build_snapshot(paths, &inputs, &bootstrap, &extras, &failures, catalog)
}

fn load_once() -> Snapshot {
    let Discovered {
        paths,
        bootstrap,
        inputs,
        extras,
        failures,
        catalog,
    } = discover(&game_dir());

    // From here the log directory is known; switch the sink to it.
    let log_path = crate::log::relocate(&paths);
    crate::log::info(&format!(
        "DefianceLoader {} (ABI {}); exe {}; root {}; config {}; plugins {}; log {}",
        env!("CARGO_PKG_VERSION"),
        defiance_api::ABI_VERSION,
        paths.exe_dir.display(),
        paths.root.display(),
        paths.config_dir.display(),
        paths.plugin_dir.display(),
        log_path.display()
    ));
    if paths.plugin_override {
        crate::log::warn(&format!(
            "plugin discovery uses the legacy `plugins` override: {}",
            paths.plugin_dir.display()
        ));
    }

    let snapshot = build_snapshot(
        paths.clone(),
        &inputs,
        &bootstrap,
        &extras,
        &failures,
        catalog,
    );
    for warning in &snapshot.warnings {
        crate::log::warn(warning);
    }
    for problem in &snapshot.problems {
        crate::log::error(&format!(
            "{}: {}{}",
            problem.owner,
            problem.message,
            problem
                .line
                .map(|line| format!(" ({}:{line})", problem.file))
                .unwrap_or_else(|| format!(" ({})", problem.file))
        ));
    }
    for owner in &snapshot.blocked {
        crate::log::error(&format!("{owner} blocked by invalid configuration"));
    }

    if snapshot.startup_error().is_some() {
        return snapshot;
    }

    // Record the configuration schema, but not while an explicit legacy
    // migration is still pending: the version is a claim about the files.
    match migration::read_state(&paths.config_dir) {
        Ok(state) => {
            if let Err(reason) = migration::check_supported(&state) {
                crate::log::error(&reason);
            } else if state.schema_version == 0 {
                let pending = legacy_pending(&bootstrap, &inputs);
                if pending {
                    crate::log::warn(
                        "legacy loader settings are still in defiance-loader.ini; \
                         an explicit migration can move them into core.ini",
                    );
                } else if let Err(e) =
                    migration::write_state(&paths.config_dir, &migration::State::default())
                {
                    crate::log::warn(&format!("could not write config metadata: {e}"));
                }
            }
        }
        Err(e) => crate::log::warn(&format!("could not read config metadata: {e}")),
    }

    snapshot
}

/// Whether the bootstrap still holds a loader key that has no explicit value
/// in its group file.
fn legacy_pending(bootstrap: &parse::Document, inputs: &[GroupInput]) -> bool {
    let Some(core) = inputs.iter().find(|input| input.group == "core") else {
        return false;
    };
    let document = core
        .text
        .as_ref()
        .and_then(|text| text.as_ref().ok())
        .map(|text| parse::parse(text))
        .unwrap_or_default();
    builtin::LOADER_SETTINGS.iter().any(|decl| {
        bootstrap.top(decl.key).is_some()
            && document.lookup(builtin::LOADER_SECTION, decl.key).is_none()
    })
}

/// The loaded snapshot, if `load` has run.
pub fn current() -> Option<&'static Snapshot> {
    SNAPSHOT.get()
}

/// The compatibility adapter: a value from the configuration by `(section,
/// key)`, both case-insensitive. Declared settings return their validated
/// canonical string; a section that is not declared falls back to the legacy
/// bootstrap file's own sections, keeping ABI 5 third-party plugins working.
pub fn get(section: &str, key: &str) -> Option<String> {
    let snapshot = SNAPSHOT.get()?;
    // The old loader keys were unsectioned; accept both spellings.
    if section.is_empty() {
        if let Some(value) = snapshot.text(builtin::LOADER_SECTION, key) {
            return Some(value);
        }
    }
    if let Some(resolved) = snapshot.get(section, key) {
        return Some(resolved.canonical());
    }
    snapshot.legacy(section, key).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "defiance-config-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// A hand-written manifest with the real ABI, so a version bump cannot
    /// change what a config test exercises.
    fn host_abi(json: &str) -> String {
        json.replace(
            "\"abi\":5",
            &format!("\"abi\":{}", defiance_api::ABI_VERSION),
        )
    }

    #[test]
    fn first_run_writes_the_bootstrap_and_every_group_file() {
        let base = unique_dir("first-run");
        let exe = base.join("bin");
        std::fs::create_dir_all(&exe).unwrap();
        let discovered = discover(&exe);
        assert!(exe.join(paths::BOOTSTRAP_FILE).is_file());
        // The default root is a sibling of bin.
        let config = base.join("DefianceLoader").join("config");
        for group in builtin::GROUPS {
            assert!(
                config.join(format!("{group}.ini")).is_file(),
                "{group} missing"
            );
        }
        // And the generated files validate against the schema.
        let snapshot = Snapshot::build(
            discovered.paths.clone(),
            &discovered.inputs,
            &discovered.bootstrap,
        );
        assert!(snapshot.problems.is_empty(), "{:?}", snapshot.problems);
        assert_eq!(snapshot.integer(builtin::LOADER_SECTION, "wait"), Some(60));
        assert_eq!(snapshot.enabled("defiance.selection"), Some(true));
    }

    #[test]
    fn a_second_run_adds_missing_settings_and_preserves_edits() {
        let base = unique_dir("preserve");
        let exe = base.join("bin");
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::write(exe.join(paths::BOOTSTRAP_FILE), "root = data\n").unwrap();
        let first = discover(&exe);
        let core = exe.join("data").join("config").join("core.ini");
        let edited = "; my own comment\n[loader]\nwait = 7\n";
        std::fs::write(&core, edited).unwrap();
        let second = discover(&exe);
        let extended = std::fs::read_to_string(&core).unwrap();
        assert!(extended.starts_with(edited));
        assert_eq!(
            std::fs::read_to_string(core.with_extension("defiance-backup")).unwrap(),
            edited
        );
        discover(&exe);
        assert_eq!(std::fs::read_to_string(&core).unwrap(), extended);
        let snapshot = Snapshot::build(second.paths.clone(), &second.inputs, &second.bootstrap);
        assert_eq!(snapshot.integer(builtin::LOADER_SECTION, "wait"), Some(7));
        // The first snapshot still saw the generated default.
        let before = Snapshot::build(first.paths, &first.inputs, &first.bootstrap);
        assert_eq!(before.integer(builtin::LOADER_SECTION, "wait"), Some(60));
    }

    #[test]
    fn the_bootstrap_plugins_override_beats_root_for_discovery() {
        let base = unique_dir("override");
        let exe = base.join("bin");
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::write(
            exe.join(paths::BOOTSTRAP_FILE),
            "root = data\nplugins = ../old/plugins\n",
        )
        .unwrap();
        let discovered = discover(&exe);
        assert_eq!(
            discovered.paths.plugin_dir,
            base.join("old").join("plugins")
        );
        assert!(discovered.paths.plugin_override);
    }

    #[test]
    fn resolving_does_not_depend_on_the_working_directory() {
        // Paths are derived from the executable directory alone, so a relative
        // root is the same however the process was started.
        let base = unique_dir("cwd");
        let exe = base.join("bin");
        std::fs::create_dir_all(&exe).unwrap();
        let document = parse::parse("root = ../DefianceLoader\n");
        let (paths, _) = Paths::resolve(&exe, &document);
        assert_eq!(paths.config_dir, base.join("DefianceLoader").join("config"));
    }

    #[test]
    fn a_third_party_manifest_declares_its_group_and_settings() {
        let base = unique_dir("third-party");
        let exe = base.join("bin");
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::write(exe.join(paths::BOOTSTRAP_FILE), "root = data\n").unwrap();
        let plugins = exe.join("data").join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        // The catalog pairs a sidecar with its DLL, so both must be present.
        std::fs::write(plugins.join("demo.dll"), b"fixture; never loaded here").unwrap();
        std::fs::write(
            plugins.join("demo.plugin.json"),
            host_abi(r#"{"schema":1,"id":"author.demo","dll":"demo.dll","version":"1.0.0","abi":5,"group":"author.demo","settings":[{"key":"enabled","type":"bool","default":"true","description":"Toggle."},{"key":"token","type":"text","default":"","description":"Secret.","sensitive":true}],"depends":[],"conflicts":[]}"#),
        )
        .unwrap();
        // The first load generates the third-party group with its defaults.
        let _ = discover(&exe);
        let group = exe.join("data").join("config").join("author.demo.ini");
        assert!(
            group.is_file(),
            "the third-party group file was not generated"
        );
        assert!(std::fs::read_to_string(&group)
            .unwrap()
            .contains("enabled = true"));
        // A user edit is honored, and an invalid one blocks the owner.
        std::fs::write(&group, "[author.demo]\nenabled = false\ntoken = s3cret\n").unwrap();
        let snapshot = inspect(&exe);
        assert_eq!(snapshot.enabled("author.demo"), Some(false));
        assert!(snapshot.problems.is_empty(), "{:?}", snapshot.problems);
        let report = snapshot.report();
        assert!(
            report.contains("author.demo.token = <redacted>"),
            "{report}"
        );
        assert!(!report.contains("s3cret"), "{report}");
        std::fs::write(&group, "[author.demo]\nenabled = maybe\n").unwrap();
        let snapshot = inspect(&exe);
        assert!(snapshot.is_blocked("author.demo"));
    }

    #[test]
    fn mixed_case_sidecar_extensions_still_declare_their_group() {
        let base = unique_dir("mixed-case");
        let exe = base.join("bin");
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::write(exe.join(paths::BOOTSTRAP_FILE), "root = data\n").unwrap();
        let plugins = exe.join("data").join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        // The DLL and sidecar are both uppercase; discovery must pair them and
        // strip the extension without regard to case.
        std::fs::write(plugins.join("DEMO.DLL"), b"fixture").unwrap();
        std::fs::write(
            plugins.join("DEMO.PLUGIN.JSON"),
            host_abi(r#"{"schema":1,"id":"author.demo","dll":"demo.dll","version":"1.0.0","abi":5,"group":"author.demo","settings":[{"key":"enabled","type":"bool","default":"true","description":"Toggle."}],"depends":[],"conflicts":[]}"#),
        )
        .unwrap();
        let _ = discover(&exe);
        let group = exe.join("data").join("config").join("author.demo.ini");
        assert!(
            group.is_file(),
            "the mixed-case sidecar did not declare its group"
        );
        let (nodes, warnings) = crate::plan::discover(&plugins);
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].manifest.is_some(), "{warnings:?}");
        assert!(nodes[0].error.is_none(), "{:?}", nodes[0].error);
    }

    #[test]
    fn repository_fixtures_parse_and_resolve() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("fixtures");
        let exe = PathBuf::from("C:/Game/bin");

        let text = std::fs::read_to_string(root.join("paths").join("bootstrap_root.ini")).unwrap();
        let (paths, warnings) = Paths::resolve(&exe, &parse::parse(&text));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(paths.root, PathBuf::from("C:/Game/Mods/Defiance"));

        let text =
            std::fs::read_to_string(root.join("paths").join("bootstrap_legacy.ini")).unwrap();
        let (paths, _) = Paths::resolve(&exe, &parse::parse(&text));
        assert_eq!(paths.plugin_dir, PathBuf::from("C:/Game/Legacy/plugins"));
        assert!(paths.plugin_override);

        let text = std::fs::read_to_string(root.join("config").join("quirks.ini")).unwrap();
        let document = parse::parse(&text);
        assert!(document.issues.is_empty(), "{:?}", document.issues);
        assert_eq!(
            document
                .lookup("defiance.example", "greeting")
                .unwrap()
                .0
                .value,
            "hello"
        );
        assert_eq!(
            document
                .lookup("defiance.example", "quoted")
                .unwrap()
                .0
                .value,
            "a; b # c = d"
        );
        assert_eq!(
            document
                .lookup("defiance.example", "duplicate")
                .unwrap()
                .0
                .value,
            "second"
        );

        let text = std::fs::read_to_string(root.join("manifest").join("demo.plugin.json")).unwrap();
        let manifest = manifest::parse(&text, "demo.dll").unwrap();
        assert_eq!(manifest.id, "author.demo");
        assert_eq!(manifest.version.to_string(), "1.2.3");
        assert_eq!(manifest.depends[0].id, "defiance.core");
    }
}

#[cfg(test)]
mod startup_regressions {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "defiance-config-startup-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(path.join("bin")).unwrap();
            Self(path)
        }
        fn load(&self) -> (Snapshot, bool) {
            let Discovered {
                paths,
                bootstrap,
                inputs,
                extras,
                failures,
                catalog,
            } = read(&self.0.join("bin"), true);
            let pending = legacy_pending(&bootstrap, &inputs);
            (
                build_snapshot(paths, &inputs, &bootstrap, &extras, &failures, catalog),
                pending,
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn shared_manifest_defaults_upgrade_without_overwriting_or_repeating() {
        let f = Fixture::new();
        let plugins = f.0.join("DefianceLoader/plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        std::fs::write(
            plugins.join("defiance_plugin_regroup.dll"),
            b"fixture; never loaded here",
        )
        .unwrap();
        std::fs::write(
            plugins.join("defiance_plugin_regroup.plugin.json"),
            include_str!("../../../../plugins/regroup/defiance_plugin_regroup.plugin.json"),
        )
        .unwrap();
        let (snapshot, _) = f.load();
        assert_eq!(
            snapshot
                .text("defiance.regroup", "regroup_hotkey")
                .as_deref(),
            Some("Ctrl+Alt+R")
        );
        let ini = f.0.join("DefianceLoader/config/infantry.ini");
        let first = std::fs::read_to_string(&ini).unwrap();
        assert!(first.contains("regroup_hotkey = Ctrl+Alt+R"));
        let manifest = crate::manifest::parse(
            include_str!("../../../../plugins/regroup/defiance_plugin_regroup.plugin.json"),
            "defiance_plugin_regroup.dll",
        )
        .unwrap();
        let help = &manifest
            .settings
            .iter()
            .find(|s| s.key == "regroup_hotkey")
            .unwrap()
            .description;
        for line in help.lines() {
            assert!(
                first.contains(&format!("; {line}\n")),
                "manifest help was not materialized"
            );
        }
        assert!(first.contains("; Default: Ctrl+Alt+R\n"));
        assert!(first.contains("[defiance.selection]"));
        let edited = "\u{feff}; my settings\r\n[defiance.regroup]\r\nregroup_hotkey = F9 ; keep\r\ncustom = untouched\r\n[defiance.selection]\r\nenabled = false";
        std::fs::write(&ini, edited).unwrap();
        inspect(&f.0.join("bin"));
        assert_eq!(std::fs::read_to_string(&ini).unwrap(), edited);
        let (snapshot, _) = f.load();
        let after = std::fs::read_to_string(&ini).unwrap();
        assert!(after.starts_with("\u{feff}; my settings\r\n[defiance.regroup]\r\nregroup_hotkey = F9 ; keep\r\ncustom = untouched\r\n"));
        let document = parse::parse(&after);
        assert!(document.issues.is_empty());
        assert_eq!(
            document
                .lookup("defiance.regroup", "restore_hotkey")
                .unwrap()
                .0
                .value,
            "Ctrl+Alt+U"
        );
        assert!(after.contains("; Default: Ctrl+Alt+U\r\n"));
        assert!(document
            .lookup("defiance.selection", "restore_hotkey")
            .is_none());
        assert_eq!(
            snapshot
                .text("defiance.regroup", "regroup_hotkey")
                .as_deref(),
            Some("F9")
        );
        assert_eq!(snapshot.enabled("defiance.selection"), Some(false));
        assert_eq!(
            std::fs::read_to_string(ini.with_extension("defiance-backup")).unwrap(),
            edited
        );
        f.load();
        assert_eq!(std::fs::read_to_string(&ini).unwrap(), after);
        // Malformed files are never materialized.
        let broken = "[defiance.regroup]\nnot an entry\n";
        std::fs::write(&ini, broken).unwrap();
        assert!(f.load().0.is_blocked("defiance.regroup"));
        assert_eq!(std::fs::read_to_string(&ini).unwrap(), broken);
    }

    #[test]
    fn failed_replacement_preserves_file_and_uses_missing_defaults_in_memory() {
        let f = Fixture::new();
        let dir = f.0.join("DefianceLoader/config");
        std::fs::create_dir_all(&dir).unwrap();
        let ini = dir.join("infantry.ini");
        let original = "[defiance.selection]\nenabled=false\n";
        std::fs::write(&ini, original).unwrap();
        // Occupy the temporary replacement path to force a write failure.
        std::fs::create_dir(ini.with_extension("tmp")).unwrap();
        let (snapshot, _) = f.load();
        assert_eq!(std::fs::read_to_string(&ini).unwrap(), original);
        assert_eq!(snapshot.enabled("defiance.selection"), Some(false));
        assert_eq!(snapshot.enabled("defiance.movement"), Some(true));
    }

    #[test]
    fn generation_preserves_legacy_policy_across_restarts_until_migration() {
        let f = Fixture::new();
        std::fs::write(
            f.0.join("bin/defiance-loader.ini"),
            "wait=7\nallow_unknown_build=true\n",
        )
        .unwrap();
        for _ in 0..2 {
            let (snapshot, pending) = f.load();
            assert!(pending);
            assert_eq!(snapshot.integer("loader", "wait"), Some(7));
            assert_eq!(
                snapshot.text("loader", "allow_unknown_build").as_deref(),
                Some("true")
            );
            assert!(matches!(
                snapshot.get("loader", "wait").unwrap().provenance,
                snapshot::Provenance::Bootstrap { .. }
            ));
        }
        let core = f.0.join("DefianceLoader/config/core.ini");
        let original = std::fs::read_to_string(&core).unwrap();
        let bootstrap = parse::parse("wait=7\nallow_unknown_build=true\n");
        migration::apply(&migration::plan_loader_keys(
            &bootstrap,
            &core,
            Some(&original),
        ))
        .unwrap();
        let (snapshot, pending) = f.load();
        assert!(!pending);
        assert_eq!(snapshot.integer("loader", "wait"), Some(7));
        assert!(matches!(
            snapshot.get("loader", "wait").unwrap().provenance,
            snapshot::Provenance::Group { .. }
        ));
    }

    #[test]
    fn invalid_legacy_policy_is_not_hidden_by_generated_defaults() {
        let f = Fixture::new();
        std::fs::write(
            f.0.join("bin/defiance-loader.ini"),
            "allow_unknown_build=maybe\n",
        )
        .unwrap();
        for _ in 0..2 {
            assert!(f.load().0.startup_error().is_some());
        }
    }

    #[test]
    fn unsupported_or_broken_metadata_blocks_before_any_default_writes() {
        for bytes in [
            b"[config]\nschema_version=99\n".as_slice(),
            b"[config]\nschema_version=invalid\n",
            b"\xff\xfe",
        ] {
            let f = Fixture::new();
            let dir = f.0.join("DefianceLoader/config");
            std::fs::create_dir_all(&dir).unwrap();
            let meta = dir.join(migration::METADATA_FILE);
            std::fs::write(&meta, bytes).unwrap();
            assert!(f.load().0.startup_error().is_some());
            assert!(!f.0.join("bin/defiance-loader.ini").exists());
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
            assert_eq!(std::fs::read(&meta).unwrap(), bytes);
            assert!(inspect(&f.0.join("bin")).startup_error().is_some());
        }
    }
}
