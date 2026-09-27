fn main() {
    defiance_build_support::windows_resources("dll", "Defiance posture plugin");
    defiance_build_support::embed_units(&["posture"]);
}
