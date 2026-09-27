fn main() {
    defiance_build_support::windows_resources(
        "dll",
        "Core patches for Terminator: Dark Fate - Defiance",
    );
    // Core's own units only: the build anchors and the sites Core hooks.
    defiance_build_support::embed_units(&["core"]);
}
