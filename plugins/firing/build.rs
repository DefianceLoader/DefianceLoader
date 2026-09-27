fn main() {
    defiance_build_support::windows_resources("dll", "Defiance firing plugin");
    defiance_build_support::embed_units(&["firing"]);
}
