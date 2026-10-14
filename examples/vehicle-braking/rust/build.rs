//! Put the manifest beside the DLL so both can be copied to the plugins folder.
use std::{env, fs, path::PathBuf};

fn main() {
    let manifest = "defiance_example_vehicle_braking_rust.plugin.json";
    println!("cargo:rerun-if-changed={manifest}");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    // OUT_DIR is <target>/<profile>/build/<package>/out, including cross targets.
    let profile = out.ancestors().nth(3).expect("Cargo output directory");
    let destination = profile.join(manifest);
    println!("cargo:rerun-if-changed={}", destination.display());
    let source = fs::read(manifest).expect("read plugin manifest");
    if fs::read(&destination).ok().as_deref() != Some(source.as_slice()) {
        fs::write(destination, source).expect("copy plugin manifest");
    }
}
