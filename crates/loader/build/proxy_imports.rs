use std::{env, fs, path::PathBuf, process::Command};

fn librarian() -> PathBuf {
    println!("cargo:rerun-if-env-changed=DEFIANCE_LIB");
    if let Some(path) = env::var_os("DEFIANCE_LIB") {
        return path.into();
    }
    if let Some(path) = env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|p| p.join("lib.exe"))
            .find(|p| p.is_file())
    }) {
        return path;
    }
    let vswhere = PathBuf::from(env::var_os("ProgramFiles(x86)").expect("ProgramFiles(x86)"))
        .join("Microsoft Visual Studio/Installer/vswhere.exe");
    let output = Command::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-find",
            "VC/Tools/MSVC/*/bin/Hostx64/x64/lib.exe",
        ])
        .output()
        .expect("locate MSVC lib.exe using vswhere");
    assert!(
        output.status.success(),
        "vswhere failed; set DEFIANCE_LIB to MSVC lib.exe"
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .expect("install MSVC build tools or set DEFIANCE_LIB")
        .into()
}

pub fn generate() {
    println!("cargo:rerun-if-changed=build/proxy_imports.rs");
    println!("cargo:rerun-if-changed=src/proxy_generated.rs");
    let generated = fs::read_to_string("src/proxy_generated.rs").unwrap();
    let real = generated
        .lines()
        .find_map(|line| {
            line.strip_prefix("pub const REAL: &str = \"")
                .and_then(|s| s.strip_suffix("\";"))
        })
        .expect("generated proxy DLL name");
    assert!(
        real.ends_with(".dll")
            && real
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.'),
        "invalid proxy DLL name"
    );
    let path = format!(r"\\?\GLOBALROOT\SystemRoot\System32\{real}");
    // LIB strips directories from LIBRARY statements. Build a same-length
    // placeholder, then replace its stem in the COFF archive, including the
    // import descriptor symbol. Equal lengths preserve all archive offsets.
    let placeholder = format!("defiance_{}.dll", "x".repeat(path.len() - 13));
    assert_eq!(placeholder.len(), path.len());
    // Only the anchor is a load-time import (see src/proxy.rs): it loads the
    // real DLL before ours, and DllMain resolves every other export at runtime,
    // so an export an older Windows lacks cannot stop the proxy from loading.
    let anchor = generated
        .lines()
        .find_map(|line| {
            line.strip_prefix("pub const ANCHOR: &str = \"")
                .and_then(|s| s.strip_suffix("\";"))
        })
        .expect("generated proxy anchor");
    assert!(
        !anchor.is_empty()
            && anchor
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "invalid proxy anchor"
    );
    let names = [anchor];
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let def = out.join("system.def");
    let lib = out.join("defiance_system_proxy.lib");
    // DATA requests IAT symbols without public callable thunks, whose names
    // would collide with our own exports. The slots still hold code addresses.
    fs::write(
        &def,
        format!(
            "LIBRARY {placeholder}\nEXPORTS\n{}\n",
            names
                .iter()
                .map(|name| format!("{name} DATA"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )
    .unwrap();
    let output = Command::new(librarian())
        .arg("/nologo")
        .arg("/machine:x64")
        .arg(format!("/def:{}", def.display()))
        .arg(format!("/out:{}", lib.display()))
        .output()
        .expect("run MSVC librarian");
    assert!(
        output.status.success(),
        "import library failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut bytes = fs::read(&lib).unwrap();
    let needle = &placeholder.as_bytes()[..placeholder.len() - 4];
    let replacement = &path.as_bytes()[..path.len() - 4];
    let mut count = 0;
    let mut at = 0;
    while at + needle.len() <= bytes.len() {
        if &bytes[at..at + needle.len()] == needle {
            bytes[at..at + needle.len()].copy_from_slice(replacement);
            count += 1;
            at += needle.len();
        } else {
            at += 1;
        }
    }
    assert!(count > names.len(), "unexpected MSVC import-library format");
    fs::write(lib, bytes).unwrap();
    println!("cargo:rustc-link-search=native={}", out.display());
}
