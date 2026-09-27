fn main() {
    defiance_build_support::windows_resources("dll", "Defiance diagnostics plugin");
    defiance_build_support::embed_units(&["diagnostics"]);
}
