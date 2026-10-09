//! Stage the sidecar manifests of the plugins shipped with the loader.
//!
//! Core's manifest is generated from the loader's authoritative table
//! (`config::builtin`), so the packaged manifest and the runtime registry
//! cannot drift apart. Every feature plugin is an ordinary plugin whose
//! manifest is committed beside its crate as
//! `plugins/<crate>/defiance_plugin_feature_<stem>.plugin.json`; those are
//! checked with the loader's own parser and copied unchanged. All land in
//! `--out` beside the DLLs as `<dll-stem>.plugin.json`.
//!
//!     cargo run -p defiance-loader --bin manifest-gen -- --out target/release

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The committed feature manifests under the workspace's `plugins/`, as
/// `(dll basename, manifest JSON)`, sorted by DLL.
fn feature_manifests(plugins: &Path) -> Result<Vec<(String, String)>, String> {
    let mut found = Vec::new();
    let dirs = std::fs::read_dir(plugins)
        .map_err(|e| format!("could not read {}: {e}", plugins.display()))?;
    for dir in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".plugin.json") else {
                continue;
            };
            if !stem.starts_with("defiance_plugin_feature_") {
                continue;
            }
            let dll = format!("{stem}.dll");
            let path = file.path();
            let json = std::fs::read_to_string(&path)
                .map_err(|e| format!("could not read {}: {e}", path.display()))?;
            defiance_loader::check_manifest(&json, &dll)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            found.push((dll, json));
        }
    }
    found.sort();
    Ok(found)
}

fn main() -> ExitCode {
    let mut out = PathBuf::from("target").join("release");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => match args.next() {
                Some(dir) => out = PathBuf::from(dir),
                None => {
                    eprintln!("manifest-gen: --out needs a directory");
                    return ExitCode::FAILURE;
                }
            },
            other => {
                eprintln!("manifest-gen: unknown argument `{other}`");
                return ExitCode::FAILURE;
            }
        }
    }
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("manifest-gen: could not create {}: {e}", out.display());
        return ExitCode::FAILURE;
    }
    let plugins = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
    let features = match feature_manifests(&plugins) {
        Ok(features) => features,
        Err(e) => {
            eprintln!("manifest-gen: {e}");
            return ExitCode::FAILURE;
        }
    };
    for (dll, json) in defiance_loader::builtin_manifests()
        .into_iter()
        .chain(features)
    {
        let path = out.join(defiance_loader::sidecar_name(&dll));
        if let Err(e) = std::fs::write(&path, json) {
            eprintln!("manifest-gen: could not write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        println!("manifest {}", path.display());
    }
    ExitCode::SUCCESS
}
