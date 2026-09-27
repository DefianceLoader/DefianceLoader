fn main() {
    defiance_build_support::windows_resources("dll", "Defiance ammunition plugin");
    defiance_build_support::embed_units(&["ammunition"]);
}
