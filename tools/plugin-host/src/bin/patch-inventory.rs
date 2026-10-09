//! Start the plugins in `../DefianceLoader/plugins` as the loader does
//! (discovery, configuration, plan, `init`), against the stock game modules
//! copied beside this executable ([`MODULES`]), and print what each plugin
//! wrote as one JSON object. Driven by `tools/patch_inventory.py`.
//!
//! The modules are mapped as images without running their code
//! (`DONT_RESOLVE_DLL_REFERENCES`), so the loader finds them by name as it
//! finds the game's. `reverse` breaks initialization-order ties by descending
//! plugin ID.
use core::ffi::c_void;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
}

const DONT_RESOLVE_DLL_REFERENCES: u32 = 1;

/// The game modules plugins write to. `logic` and `game` are required; the
/// renderer (`world2`) and engine (`galileo`) modules, which only Core
/// patches, are mapped when they are beside this executable.
const MODULES: [&str; 4] = ["logic", "game", "world2", "galileo"];

/// `text` as a JSON string literal.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn main() {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap();
    let args: Vec<_> = std::env::args().collect();
    let reload_vehicle = args.iter().any(|arg| arg == "reload-vehicle");
    if args.iter().any(|arg| arg == "reverse") {
        defiance_loader::test_host::reverse_ties(true);
    }
    let mut modules = Vec::new();
    for name in MODULES {
        let file = dir.join(format!("{name}.dll"));
        if !file.is_file() && !["logic", "game"].contains(&name) {
            continue;
        }
        let path: Vec<u16> = file
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let base = unsafe {
            LoadLibraryExW(
                path.as_ptr(),
                core::ptr::null_mut(),
                DONT_RESOLVE_DLL_REFERENCES,
            )
        };
        assert!(!base.is_null(), "{name}.dll did not map");
        let size = unsafe { *(base.cast::<u8>().add(0x3c) as *const u32) } as usize;
        let size_of_image = unsafe { *(base.cast::<u8>().add(size + 0x50) as *const u32) };
        modules.push((name, base as usize, size_of_image as usize));
    }
    let states = defiance_loader::test_host::load_plugins(dir);
    if reload_vehicle {
        const PLUGIN: &str = "defiance.vehicle-special-fire";
        assert!(
            states.iter().any(|(id, state)| {
                id.eq_ignore_ascii_case(PLUGIN) && state.starts_with("Active")
            }),
            "{PLUGIN} must be active before the reload: {states:?}"
        );
        let before_loaded = defiance_loader::test_host::loaded_plugins();
        let before_owner = before_loaded
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(PLUGIN))
            .map(|(_, owner)| *owner)
            .expect("the passenger plugin has a live owner");
        let mut before_spans = defiance_loader::test_host::installed();
        before_spans.sort();
        let before_target_spans: Vec<_> = before_spans
            .iter()
            .filter(|(owner, _, _, _)| owner.eq_ignore_ascii_case(PLUGIN))
            .cloned()
            .collect();
        assert!(
            !before_target_spans.is_empty(),
            "the passenger plugin owns hooks"
        );

        defiance_loader::test_host::reload_plugin(PLUGIN)
            .unwrap_or_else(|error| panic!("{PLUGIN} reload failed: {error}"));

        let after_loaded = defiance_loader::test_host::loaded_plugins();
        let after_owner = after_loaded
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(PLUGIN))
            .map(|(_, owner)| *owner)
            .expect("the passenger plugin remains loaded after reload");
        assert_ne!(
            before_owner, after_owner,
            "reload creates a new plugin owner"
        );
        let mut after_spans = defiance_loader::test_host::installed();
        after_spans.sort();
        assert_eq!(
            after_spans, before_spans,
            "reload removes the old hook spans and installs the same new spans"
        );
        let after_target_spans: Vec<_> = after_spans
            .iter()
            .filter(|(owner, _, _, _)| owner.eq_ignore_ascii_case(PLUGIN))
            .cloned()
            .collect();
        assert_eq!(
            after_target_spans, before_target_spans,
            "reload restores each old hook before reinstalling it without leaking spans"
        );
        assert_eq!(
            after_target_spans
                .iter()
                .map(|(_, target, _, _)| *target)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            after_target_spans.len(),
            "reload leaves one owned span per passenger hook site"
        );
        println!(
            "{PLUGIN}: reload restored and reinstalled {} spans",
            after_target_spans.len()
        );
    }
    let order = defiance_loader::test_host::planned_order();
    let mut spans = defiance_loader::test_host::installed();
    spans.sort_by_key(|span| span.1);
    let states: Vec<String> = states
        .iter()
        .map(|(id, state)| format!("{}: {}", quoted(id), quoted(state)))
        .collect();
    let order: Vec<String> = order.iter().map(|id| quoted(id)).collect();
    let spans: Vec<String> = spans
        .iter()
        .map(|(owner, target, length, kind)| {
            let (module, rva) = modules
                .iter()
                .find(|(_, base, size)| (*base..base + size).contains(target))
                .map_or(("other", *target), |(name, base, _)| (*name, target - base));
            format!(
                "{{\"plugin\": {}, \"module\": \"{module}\", \"rva\": {rva}, \"length\": {length}, \"kind\": \"{kind}\"}}",
                quoted(owner)
            )
        })
        .collect();
    println!(
        "{{\"states\": {{{}}}, \"order\": [{}], \"spans\": [{}]}}",
        states.join(", "),
        order.join(", "),
        spans.join(", ")
    );
}
