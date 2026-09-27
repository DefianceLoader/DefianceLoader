fn main() {
    defiance_build_support::windows_resources("dll", "Defiance garrison plugin");
    defiance_build_support::embed_units(&["garrison"]);
}
