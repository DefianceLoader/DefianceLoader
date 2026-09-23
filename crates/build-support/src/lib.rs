//! Build-time Windows resources: a VERSIONINFO block and a manifest, compiled
//! with the SDK's `rc.exe`, so shipped binaries carry identifiable metadata.
//! These resources do not guarantee antivirus acceptance.
//!
//! Called from a crate's `build.rs`. Windows release builds require resources;
//! development builds may continue with a warning when the SDK is unavailable.
//!
//! The version comes from Cargo's own environment (`CARGO_PKG_VERSION`); the
//! description is passed in. `windows_resources` gives every output of a crate
//! the same metadata, named after the crate; a crate that links several
//! outputs (a DLL plus tools) uses `windows_resources_for` so each file names
//! itself.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One linked output of a crate.
pub enum Artifact<'a> {
    /// The crate's `cdylib`, named after the crate (`my-crate` -> `my_crate.dll`).
    Library,
    /// A binary target, by its Cargo name (`my-tool` -> `my-tool.exe`).
    Bin(&'a str),
}

/// Emit the resources for this crate. `file_type` is "dll" or "exe", and
/// `description` becomes the FileDescription/ProductName. Every output the
/// crate links receives them, so a crate with more than one output should use
/// `windows_resources_for` instead.
pub fn windows_resources(file_type: &str, description: &str) {
    let name = env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "defiance".to_string());
    let is_dll = file_type.eq_ignore_ascii_case("dll");
    let original = format!(
        "{}.{}",
        name.replace('-', "_"),
        if is_dll { "dll" } else { "exe" }
    );
    emit(&Output {
        name: &name,
        original,
        is_dll,
        description,
        link: "rustc-link-arg".into(),
    });
}

/// Emit separate resources for each listed output of this crate, so a DLL and
/// the tools built beside it each carry their own description, `InternalName`
/// and `OriginalFilename`. An output left out gets no resources.
pub fn windows_resources_for(outputs: &[(Artifact<'_>, &str)]) {
    let crate_name = env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "defiance".to_string());
    for (artifact, description) in outputs {
        let output = match artifact {
            Artifact::Library => Output {
                name: &crate_name,
                original: format!("{}.dll", crate_name.replace('-', "_")),
                is_dll: true,
                description,
                link: "rustc-link-arg-cdylib".into(),
            },
            Artifact::Bin(bin) => Output {
                name: bin,
                original: format!("{bin}.exe"),
                is_dll: false,
                description,
                link: format!("rustc-link-arg-bin={bin}"),
            },
        };
        emit(&output);
    }
}

/// What one output's resources say, and the Cargo instruction that links them.
struct Output<'a> {
    name: &'a str,
    original: String,
    is_dll: bool,
    description: &'a str,
    link: String,
}

fn emit(output: &Output) {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    match generate_resources(output) {
        Ok(res_path) => println!("cargo:{}={}", output.link, res_path.display()),
        Err(error) => {
            if env::var("PROFILE").as_deref() == Ok("release") {
                panic!("Windows release metadata is required: {error}");
            }
            println!("cargo:warning={error}; development build has no resources");
        }
    }
}

fn generate_resources(output: &Output) -> Result<PathBuf, String> {
    let Output {
        name,
        original,
        is_dll,
        description,
        ..
    } = output;
    let is_dll = *is_dll;
    // Each output compiles in its own directory, so their files never collide.
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"))
        .join(format!("resources-{name}"));
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    let company = env::var("DEFIANCE_COMPANY").unwrap_or_else(|_| "Defiance RE".to_string());

    println!("cargo:rerun-if-env-changed=DEFIANCE_COMPANY");
    println!("cargo:rerun-if-env-changed=DEFIANCE_RC");

    let rc = find_rc().ok_or_else(|| {
        format!("rc.exe not found for {name}; install the Windows SDK or set DEFIANCE_RC")
    })?;

    let numeric = version_tuple(&version);
    let filetype = if is_dll { "0x2" } else { "0x1" }; // VFT_DLL / VFT_APP
    let manifest_id = if is_dll { 2 } else { 1 }; // isolation-aware / create-process

    let manifest = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <assembly xmlns=\"urn:schemas-microsoft-com:asm.v1\" manifestVersion=\"1.0\">\n\
         \x20 <assemblyIdentity type=\"win32\" name=\"{name}\" version=\"{identity}\" \
         processorArchitecture=\"amd64\"/>\n\
         \x20 <trustInfo xmlns=\"urn:schemas-microsoft-com:asm.v3\">\n\
         \x20   <security><requestedPrivileges>\
         <requestedExecutionLevel level=\"asInvoker\" uiAccess=\"false\"/>\
         </requestedPrivileges></security>\n\
         \x20 </trustInfo>\n\
         </assembly>\n",
        identity = dotted(&numeric),
    );
    let resource = format!(
        "1 VERSIONINFO\n\
         \x20 FILEVERSION {numeric}\n\
         \x20 PRODUCTVERSION {numeric}\n\
         \x20 FILEFLAGSMASK 0x3fL\n\
         \x20 FILEFLAGS 0x0L\n\
         \x20 FILEOS 0x40004L\n\
         \x20 FILETYPE {filetype}L\n\
         \x20 FILESUBTYPE 0x0L\n\
         BEGIN\n\
         \x20 BLOCK \"StringFileInfo\"\n\
         \x20 BEGIN\n\
         \x20   BLOCK \"040904b0\"\n\
         \x20   BEGIN\n\
         \x20     VALUE \"CompanyName\", \"{company}\"\n\
         \x20     VALUE \"FileDescription\", \"{description}\"\n\
         \x20     VALUE \"FileVersion\", \"{version}\"\n\
         \x20     VALUE \"InternalName\", \"{name}\"\n\
         \x20     VALUE \"OriginalFilename\", \"{original}\"\n\
         \x20     VALUE \"ProductName\", \"{description}\"\n\
         \x20     VALUE \"ProductVersion\", \"{version}\"\n\
         \x20     VALUE \"LegalCopyright\", \"Copyright (C) {company}\"\n\
         \x20   END\n\
         \x20 END\n\
         \x20 BLOCK \"VarFileInfo\"\n\
         \x20 BEGIN\n\
         \x20   VALUE \"Translation\", 0x409, 1200\n\
         \x20 END\n\
         END\n\
         \n\
         {manifest_id} 24 \"{manifest_name}\"\n",
        manifest_name = "defiance.manifest",
    );

    let rc_path = out.join("defiance.rc");
    let manifest_path = out.join("defiance.manifest");
    let res_path = out.join("defiance.res");
    std::fs::write(&rc_path, resource).map_err(|e| format!("{}: {e}", rc_path.display()))?;
    std::fs::write(&manifest_path, manifest)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    // A successful tool must produce this build's resource, not reuse an old one.
    match std::fs::remove_file(&res_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", res_path.display())),
    }

    let status = Command::new(&rc)
        .args(["/nologo", "/fo"])
        .arg(&res_path)
        .arg(&rc_path)
        .current_dir(&out)
        .status();
    match status {
        Ok(status) if status.success() && res_path.metadata().is_ok_and(|m| m.len() > 0) => {
            Ok(res_path)
        }
        Ok(status) => Err(format!(
            "{} failed to generate resources for {name} (status {status})",
            rc.display()
        )),
        Err(e) => Err(format!("could not run {} for {name}: {e}", rc.display())),
    }
}

/// `0.1.0` -> `0, 1, 0, 0`, for the fixed version fields.
fn version_tuple(version: &str) -> String {
    let mut parts: Vec<u32> = version
        .split(['.', '-', '+'])
        .take(4)
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    while parts.len() < 4 {
        parts.push(0);
    }
    parts
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `0, 1, 0, 0` -> `0.1.0.0`, for the manifest identity.
fn dotted(numeric: &str) -> String {
    numeric.replace(", ", ".")
}

/// Explicit `DEFIANCE_RC`, otherwise the newest SDK's x64 rc.exe, then PATH.
fn find_rc() -> Option<PathBuf> {
    if let Ok(explicit) = env::var("DEFIANCE_RC") {
        return Some(PathBuf::from(explicit));
    }
    for root in ["ProgramFiles(x86)", "ProgramFiles"] {
        let Ok(base) = env::var(root) else {
            continue;
        };
        let bin = Path::new(&base).join("Windows Kits").join("10").join("bin");
        let Ok(entries) = std::fs::read_dir(&bin) else {
            continue;
        };
        let mut versions: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        versions.sort();
        versions.reverse();
        for version in versions {
            let candidate = version.join("x64").join("rc.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    if let Ok(path) = env::var("PATH") {
        for directory in env::split_paths(&path) {
            let candidate = directory.join("rc.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
