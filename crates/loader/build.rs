#[path = "build/proxy_imports.rs"]
mod proxy_imports;

use defiance_build_support::Artifact;

fn main() {
    // Each output names itself; the crash helper in particular ships beside the
    // proxy DLL and must not claim to be it.
    defiance_build_support::windows_resources_for(&[
        (
            Artifact::Library,
            "Mod loader for Terminator: Dark Fate - Defiance",
        ),
        (
            Artifact::Bin("defiance-crash-helper"),
            "Crash report helper for the Defiance mod loader",
        ),
        (
            Artifact::Bin("defiance-config"),
            "Configuration tool for the Defiance mod loader",
        ),
        (
            Artifact::Bin("manifest-gen"),
            "Plugin manifest generator for the Defiance mod loader",
        ),
        (
            Artifact::Bin("crash-fixture"),
            "Crash report test fixture for the Defiance mod loader",
        ),
    ]);
    proxy_imports::generate();
}
