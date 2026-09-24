//! Preview or apply the loader's explicit configuration migration.
//!
//! The loader never migrates on its own: it reads the current files and keeps
//! working. This tool is the explicit step. It previews by default; `--apply`
//! writes through a temporary file plus replace, keeps a `.defiance-backup`
//! copy, and records the schema version only after every write succeeds.
//!
//! ```text
//! defiance-config --game "C:\Games\...\bin"    # preview
//! defiance-config --game "..." --apply         # write the changes
//! defiance-config --game "..." --report        # effective config
//! defiance-config --defaults DIR               # explicit developer export
//! ```
//!
//! It also reports a legacy `plugins` override that points at a missing
//! directory while plugins exist at the new default, with the exact value to
//! set; it never searches for another path itself.

use std::path::PathBuf;
use std::process::ExitCode;

use defiance_loader::config::{builtin, defaults, migration, parse, paths::Paths};

fn main() -> ExitCode {
    let mut game: Option<PathBuf> = std::env::var_os("DEFIANCE_GAME_DIR").map(PathBuf::from);
    let mut apply = false;
    let mut report = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--game" => match args.next() {
                Some(dir) => game = Some(PathBuf::from(dir)),
                None => return fail("--game needs a directory"),
            },
            "--apply" => apply = true,
            "--report" => report = true,
            "--defaults" => match args.next() {
                Some(dir) => return write_defaults(&PathBuf::from(dir)),
                None => return fail("--defaults needs a directory"),
            },
            other => return fail(&format!("unknown argument `{other}`")),
        }
    }
    if report {
        let Some(dir) = &game else {
            return fail("--report needs --game DIR or DEFIANCE_GAME_DIR");
        };
        print!("{}", defiance_loader::config::inspect(dir).report());
        return ExitCode::SUCCESS;
    }
    let Some(exe_dir) = game else {
        return fail("give --game DIR or set DEFIANCE_GAME_DIR");
    };

    let bootstrap_text = match read_optional(&exe_dir.join("defiance-loader.ini")) {
        Ok(text) => text.unwrap_or_default(),
        Err(reason) => return fail(&reason),
    };
    let bootstrap = parse::parse(&bootstrap_text);
    if !bootstrap.issues.is_empty() {
        return fail("bootstrap is not parseable; no files changed");
    }
    let (paths, _) = Paths::resolve(&exe_dir, &bootstrap);

    report_plugin_mismatch(&paths);

    match migration::read_state(&paths.config_dir) {
        Ok(state) => {
            if let Err(reason) = migration::check_supported(&state) {
                return fail(&reason);
            }
        }
        Err(e) => return fail(&format!("could not read config metadata: {e}")),
    }

    let core_path = paths.config_dir.join("core.ini");
    let core_text = match read_optional(&core_path) {
        Ok(text) => text,
        Err(reason) => return fail(&reason),
    };
    if core_text
        .as_ref()
        .is_some_and(|text| !parse::parse(text).issues.is_empty())
    {
        return fail("core.ini is not parseable; no files changed");
    }
    let plan = migration::plan_loader_keys(&bootstrap, &core_path, core_text.as_deref());
    if let Some(reason) = &plan.refuse {
        return fail(reason);
    }
    if plan.is_empty() {
        println!("configuration is up to date; nothing to migrate");
        if apply {
            if let Err(e) = migration::write_state(&paths.config_dir, &migration::State::default())
            {
                return fail(&format!("could not write config metadata: {e}"));
            }
        }
        return ExitCode::SUCCESS;
    }

    for change in &plan.changes {
        if !change.changed() {
            continue;
        }
        println!("{}", change.path.display());
        for line in change.before.as_deref().unwrap_or("").lines() {
            println!("  - {line}");
        }
        for line in change.after.lines() {
            println!("  + {line}");
        }
    }

    if !apply {
        println!("preview only; pass --apply to write these changes");
        return ExitCode::SUCCESS;
    }
    if let Err(e) = migration::apply(&plan) {
        return fail(&format!("migration failed: {e}"));
    }
    if let Err(e) = migration::write_state(&paths.config_dir, &migration::State::default()) {
        return fail(&format!(
            "migration wrote the files but not the metadata: {e}"
        ));
    }
    println!("migration applied");
    ExitCode::SUCCESS
}

/// Write commented defaults for a packaged installation. An existing file is
/// never overwritten, so an upgrade keeps the user's configuration.
fn write_defaults(dir: &std::path::Path) -> ExitCode {
    if let Err(e) = std::fs::create_dir_all(dir) {
        return fail(&format!("could not create {}: {e}", dir.display()));
    }
    for group in builtin::GROUPS {
        let path = dir.join(format!("{group}.ini"));
        if path.exists() {
            println!("kept {}", path.display());
            continue;
        }
        if let Err(e) = std::fs::write(&path, defaults::render_group(group)) {
            return fail(&format!("could not write {}: {e}", path.display()));
        }
        println!("wrote {}", path.display());
    }
    ExitCode::SUCCESS
}

/// Report a legacy `plugins` override that points nowhere while the new default
/// holds plugins. This is a diagnostic, not an automatic move.
fn report_plugin_mismatch(paths: &Paths) {
    if !paths.plugin_override {
        return;
    }
    let has_dlls = |dir: &std::path::Path| {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries.flatten().any(|entry| {
                    entry.path().extension().is_some_and(|extension| {
                        extension.to_string_lossy().eq_ignore_ascii_case("dll")
                    })
                })
            })
            .unwrap_or(false)
    };
    if !has_dlls(&paths.plugin_dir) && has_dlls(&paths.root.join("plugins")) {
        println!(
            "note: the legacy `plugins` override points at {} but it has no DLLs; \
             plugins exist at {}. To migrate, set `plugins = {}` in {}.",
            paths.plugin_dir.display(),
            paths.root.join("plugins").display(),
            paths.root.join("plugins").display(),
            paths.bootstrap.display(),
        );
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("defiance-config: {message}");
    ExitCode::FAILURE
}

fn read_optional(path: &std::path::Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "could not read {}: {e}; no files changed",
            path.display()
        )),
    }
}
