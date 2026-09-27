fn main() {
    defiance_build_support::windows_resources("dll", "Defiance pickup plugin");
    defiance_build_support::embed_units(&["pickup"]);
}
