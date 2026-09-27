fn main() {
    defiance_build_support::windows_resources("dll", "Defiance movement plugin");
    defiance_build_support::embed_units(&["movement"]);
}
