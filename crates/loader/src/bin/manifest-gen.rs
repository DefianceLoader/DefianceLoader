//! Write the built-in sidecar manifests from the loader's authoritative table.
//!
//! The loader owns the single definition of every built-in (stable ID, DLL,
//! version, group, dependencies, settings). This small tool turns that into the
//! `<dll-stem>.plugin.json` files that are staged beside the DLLs, so the
//! packaged manifests and the runtime registry cannot drift apart.
//!
//!     cargo run -p defiance-loader --bin manifest-gen -- --out target/release

use std::path::PathBuf;
use std::process::ExitCode;

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
    for (dll, json) in defiance_loader::builtin_manifests() {
        let path = out.join(defiance_loader::sidecar_name(&dll));
        if let Err(e) = std::fs::write(&path, json) {
            eprintln!("manifest-gen: could not write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        println!("manifest {}", path.display());
    }
    ExitCode::SUCCESS
}
