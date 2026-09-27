fn main() {
    defiance_build_support::windows_resources("dll", "Defiance attack plugin");
    defiance_build_support::embed_units(&["attack"]);
}
