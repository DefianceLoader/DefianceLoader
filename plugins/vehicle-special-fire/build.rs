fn main() {
    defiance_build_support::windows_resources("dll", "Defiance vehicle special-fire plugin");
    defiance_build_support::embed_units(&["vehicle-special-fire"]);
}
