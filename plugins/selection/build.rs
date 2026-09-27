fn main() {
    defiance_build_support::windows_resources("dll", "Defiance selection plugin");
    defiance_build_support::embed_units(&["selection"]);
}
