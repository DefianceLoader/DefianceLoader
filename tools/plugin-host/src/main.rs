//! Load the actual plugin DLLs against mapped stock modules. Compare every
//! module byte and relocated payload byte with the legacy injector's output.
use core::ffi::{c_char, c_void, CStr};
use defiance_api::{Api, Entry};
use defiance_core::{GamePatch, Patch, Target};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{path::PathBuf, sync::OnceLock};

#[link(name = "kernel32")]
extern "system" {
    fn VirtualAlloc(at: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn GetCurrentProcessId() -> u32;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}
static MODULES: OnceLock<[(usize, usize); 2]> = OnceLock::new();
static PATCH: OnceLock<unsafe extern "C" fn(*mut c_void, *const u8, *const u8, usize) -> i32> =
    OnceLock::new();
static HOOK_EXACT: OnceLock<
    unsafe extern "C" fn(*mut c_void, *mut c_void, usize, *mut *mut c_void) -> i32,
> = OnceLock::new();
static FAIL_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
static CALLS: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" fn fail_pickup_hook(_: *mut c_void, _: *mut c_void, _: *mut *mut c_void) -> i32 {
    -1
}
unsafe extern "C" fn patch(
    at: *mut c_void,
    before: *const u8,
    after: *const u8,
    size: usize,
) -> i32 {
    if CALLS.fetch_add(1, Ordering::SeqCst) == FAIL_AT.load(Ordering::SeqCst) {
        return -1;
    }
    unsafe { PATCH.get().unwrap()(at, before, after, size) }
}
unsafe extern "C" fn module(name: *const c_char) -> *mut c_void {
    let name = unsafe { CStr::from_ptr(name) }.to_str().unwrap();
    let i = match name {
        "logic.dll" => 0,
        "game.dll" => 1,
        _ => return std::ptr::null_mut(),
    };
    MODULES.get().unwrap()[i].0 as *mut c_void
}
unsafe extern "C" fn size(base: *mut c_void) -> usize {
    MODULES
        .get()
        .unwrap()
        .iter()
        .find(|m| m.0 == base as usize)
        .unwrap()
        .1
}
unsafe extern "C" fn log(_: u32, text: *const c_char) {
    println!("{}", unsafe { CStr::from_ptr(text) }.to_string_lossy());
}
fn snapshot(base: usize, size: usize) -> Vec<u8> {
    unsafe { core::slice::from_raw_parts(base as *const u8, size) }.to_vec()
}
fn reached(image: &[u8], at: usize, base: usize) -> usize {
    if image[at..at + 6] == [0xff, 0x25, 0, 0, 0, 0] {
        return u64::from_le_bytes(image[at + 6..at + 14].try_into().unwrap()) as usize;
    }
    (base as isize
        + at as isize
        + 5
        + i32::from_le_bytes(image[at + 1..at + 5].try_into().unwrap()) as isize) as usize
}
fn shift(image: &mut [u8], at: usize, delta: isize) {
    let old = i32::from_le_bytes(image[at + 1..at + 5].try_into().unwrap());
    let new = i32::try_from(old as isize + delta).unwrap();
    image[at + 1..at + 5].copy_from_slice(&new.to_le_bytes());
}
unsafe extern "C" fn hook_exact(
    at: *mut c_void,
    detour: *mut c_void,
    size: usize,
    original: *mut *mut c_void,
) -> i32 {
    if CALLS.fetch_add(1, Ordering::SeqCst) == FAIL_AT.load(Ordering::SeqCst) {
        return -1;
    }
    unsafe { HOOK_EXACT.get().unwrap()(at, detour, size, original) }
}

/// Exercise the production callback through the installed call/relay, with
/// actual Microsoft x64 virtual calls and its process-wide rotation cursor.
unsafe fn exercise_rust_pickup(target: usize) {
    unsafe extern "system" fn held(_: *mut c_void) -> i32 {
        7
    }
    unsafe extern "system" fn man(object: *mut c_void) -> *mut c_void {
        object
    }
    unsafe extern "system" fn facets(_: *mut c_void) -> *mut c_void {
        core::ptr::null_mut()
    }
    let mut vtable = vec![0usize; 64];
    vtable[0x180 / 8] = held as *const () as usize;
    vtable[0xc8 / 8] = man as *const () as usize;
    vtable[0xb0 / 8] = facets as *const () as usize;
    let mut first = [vtable.as_ptr() as usize];
    let mut second = [vtable.as_ptr() as usize];
    let members = [first.as_mut_ptr() as usize, second.as_mut_ptr() as usize];
    let script = vec![0u64; 0x200 / 8];
    let mut squad = vec![0u64; 0x300 / 8];
    squad[0x1e8 / 8] = members.as_ptr() as u64;
    squad[0x1f0 / 8] = members.as_ptr() as u64 + 16;
    squad[0x240 / 8] = script.as_ptr() as u64;
    squad[(0x260 + 7 * 8) / 8] = 2 | (2 << 32);
    let original = squad.clone();
    let choose: unsafe extern "system" fn(*mut c_void, i32, u8) -> *mut c_void =
        unsafe { core::mem::transmute(target) };
    assert!(unsafe { choose(core::ptr::null_mut(), 7, 1) }.is_null());
    for expected in [members[0], members[1], members[0]] {
        assert_eq!(
            unsafe { choose(squad.as_mut_ptr().cast(), 7, 1) } as usize,
            expected
        );
    }
    assert_eq!(
        squad, original,
        "production callback must not write the squad"
    );
}
/// The feature DLLs a scenario does not load at all, by legacy feature ID
/// (0 is core).
fn skipped(scenario: &str) -> Vec<u32> {
    match scenario {
        "fail-selection" => vec![4, 3, 5, 8, 9, 6],
        "without-ammo" | "disabled-ammo-corrupt" => vec![6],
        "without-attack" => vec![8],
        "without-garrison" => vec![9],
        "without-selection" => vec![2, 4, 3, 5, 8, 9, 6],
        "without-posture" => vec![4, 3],
        "without-movement" => vec![3],
        "without-firing" => vec![5],
        "without-pickup" => vec![1],
        "without-diagnostics" => vec![7],
        "without-preview-weapon" => vec![10],
        "without-core" => vec![0],
        "diagnostics-only" => vec![2, 4, 3, 5, 8, 9, 6, 1, 10],
        "core-only" => vec![2, 4, 3, 5, 8, 9, 6, 1, 7, 10],
        "without-posture-and-ammo" => vec![4, 3, 6],
        "without-movement-and-ammo" => vec![3, 6],
        "shared-helper-corrupt" => vec![1, 7],
        _ => Vec::new(),
    }
}

/// The features whose writes a scenario does not install, for the byte parity.
fn omitted(scenario: &str) -> Vec<u32> {
    match scenario {
        "fail-selection" => vec![2, 4, 3, 5, 8, 9, 6],
        "rust-fail-pickup" | "rust-changed-pickup" | "rust-disabled-pickup" => vec![1],
        "fail-firing" => vec![5],
        "without-posture" => vec![4, 3], // movement is refused without posture
        "without-selection" => vec![2, 4, 3, 5, 8, 9, 6],
        "diagnostics-only" => vec![2, 4, 3, 5, 8, 9, 6, 1, 10],
        "without-ammo" | "fail-ammo" | "disabled-ammo-corrupt" | "enabled-ammo-corrupt" => vec![6],
        "without-attack" | "fail-attack" => vec![8],
        "without-garrison" | "fail-garrison" => vec![9],
        "without-movement" => vec![3],
        "without-firing" => vec![5],
        "without-pickup" => vec![1],
        "without-diagnostics" => vec![7],
        "without-preview-weapon" => vec![10],
        "core-only" => vec![2, 4, 3, 5, 8, 9, 6, 1, 7, 10],
        "without-posture-and-ammo" => vec![4, 3, 6],
        "without-movement-and-ammo" => vec![3, 6],
        "shared-helper-corrupt" => vec![1, 7],
        _ => Vec::new(),
    }
}

/// The plan the host would hand to core before init, as a feature bit mask.
fn configure_mask(scenario: &str) -> u64 {
    match scenario {
        "rust-disabled-pickup" => !(1u64 << 1),
        "disabled-ammo-corrupt" => !(1u64 << 6),
        "shared-helper-corrupt" => !((1u64 << 1) | (1u64 << 7)),
        "core-only" => 0,
        _ => u64::MAX,
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let repo = PathBuf::from(&args[1]);
    let plugins = PathBuf::from(&args[2]);
    let scenario = args.get(3).map(String::as_str).unwrap_or("all");
    let rust_pickup = std::env::var_os("DEFIANCE_TEST_RUST_PICKUP").is_some();
    let dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let mut originals = Vec::new();
    let mut targets = Vec::new();
    for name in ["logic.dll", "game.dll"] {
        let path = dir.join(name);
        let image = defiance_core::pe::map_file(&path).unwrap();
        let base =
            unsafe { VirtualAlloc(std::ptr::null_mut(), image.len(), 0x3000, 0x40) } as *mut u8;
        assert!(!base.is_null());
        unsafe { core::ptr::copy_nonoverlapping(image.as_ptr(), base, image.len()) };
        targets.push(Target {
            process_id: unsafe { GetCurrentProcessId() },
            base,
            size: image.len(),
            path,
        });
        originals.push(image);
    }
    MODULES
        .set([
            (targets[0].base as usize, targets[0].size),
            (targets[1].base as usize, targets[1].size),
        ])
        .unwrap();
    let payload = std::fs::read(repo.join("out/payload.bin")).unwrap();
    let game_payload = std::fs::read(repo.join("out/payload-game.bin")).unwrap();
    let parsed = Patch::parse(&std::fs::read_to_string(repo.join("out/payload.json")).unwrap());
    let parsed_game =
        GamePatch::parse(&std::fs::read_to_string(repo.join("out/payload-game.json")).unwrap());
    let (p, relocated, logic_moves) =
        defiance_core::logic_for_build(&parsed, &targets[0], defiance_core::Scan::Default).unwrap();
    let (g, _, _) =
        defiance_core::game_for_build(&parsed_game, &targets[1], defiance_core::Scan::Default)
            .unwrap();
    let native_functions: Vec<_> = ["firing_set", "firing_ui", "setter", "is_selected"]
        .into_iter()
        .map(|name| {
            let site = parsed.sites.iter().find(|s| s.name == name).unwrap();
            let original = parsed.detours.iter().find(|h| h.rva == site.start).unwrap();
            (
                name,
                p.detours
                    .iter()
                    .find(|h| h.entry == original.entry)
                    .unwrap(),
            )
        })
        .collect();
    defiance_core::apply(&p, &targets[0], &payload, true).unwrap();
    defiance_core::apply_game(&g, &targets[1], &game_payload).unwrap();
    let mut expected: Vec<_> = targets
        .iter()
        .map(|t| snapshot(t.base as usize, t.size))
        .collect();
    let old_logic = reached(&expected[0], p.call_site, targets[0].base as usize);
    let old_game =
        reached(&expected[1], g.hooks[0].rva, targets[1].base as usize) - g.hooks[0].entry;
    let mut old_code = snapshot(old_logic, payload.len());
    let old_game_code = snapshot(old_game, game_payload.len());
    for (t, image) in targets.iter().zip(&originals) {
        unsafe { core::ptr::copy_nonoverlapping(image.as_ptr(), t.base, image.len()) };
    }
    // An ammunition site, corrupted in memory. `disabled-ammo-corrupt` has the
    // plan exclude ammunition, so preparation must not read it and unrelated
    // features install. `enabled-ammo-corrupt` keeps it enabled: on the source
    // build its install refuses; on a relocated build its missing signature is
    // dropped and only it is refused. On a relocated build the corruption is
    // one byte of the ammunition signature window that no other site shares.
    // Restored before the final rollback check.
    let mut corrupted: Option<(usize, u8)> = None;
    let ammo_rva = parsed
        .detours
        .iter()
        .find(|h| h.feature == 6)
        .map(|h| h.rva);
    if let Some(ammo_rva) = ammo_rva {
        let scenarios = scenario == "disabled-ammo-corrupt" || scenario == "enabled-ammo-corrupt";
        if scenarios && !relocated {
            let byte = unsafe { *targets[0].base.add(ammo_rva) };
            corrupted = Some((ammo_rva, byte));
            unsafe {
                *targets[0].base.add(ammo_rva) = byte ^ 0xff;
            }
        } else if scenarios {
            if let Some(moves) = &logic_moves {
                'choose: for site in &parsed.sites {
                    if !(site.start <= ammo_rva && ammo_rva < site.start + site.pattern.len()) {
                        continue;
                    }
                    let base = moves
                        .at(site.start)
                        .expect("the ammunition site was located");
                    for (i, byte) in site.pattern.iter().enumerate() {
                        let Some(byte) = byte else { continue };
                        let at = base + i;
                        let shared = parsed.sites.iter().any(|other| {
                            other.start != site.start && {
                                let start = moves.at(other.start).unwrap();
                                start <= at && at < start + other.pattern.len()
                            }
                        });
                        if !shared {
                            corrupted = Some((at, *byte));
                            unsafe {
                                *targets[0].base.add(at) = *byte ^ 0xff;
                            }
                            break 'choose;
                        }
                    }
                }
            }
        }
    }
    // A shared anchor is required by every consumer. Damaging it must refuse
    // the whole preparation, on the source build (the anchor check) and on a
    // relocated build (the anchor site is always kept).
    if scenario == "shared-helper-corrupt" {
        let at = if relocated {
            logic_moves
                .as_ref()
                .expect("relocated")
                .at(parsed.anchor_rva)
                .expect("the anchor")
        } else {
            parsed.anchor_rva
        };
        let byte = unsafe { *targets[0].base.add(at) };
        corrupted = Some((at, byte));
        unsafe {
            *targets[0].base.add(at) = byte ^ 0xff;
        }
    }
    // A relocated build must actually carry the damaged signature, or the
    // scenario would not prove what it claims.
    let damaged = scenario == "disabled-ammo-corrupt"
        || scenario == "enabled-ammo-corrupt"
        || scenario == "shared-helper-corrupt";
    if damaged && relocated {
        assert!(
            corrupted.is_some(),
            "no signature byte to corrupt for {scenario}"
        );
    }
    let mut api: Api = defiance_loader::test_host::build_api();
    api.module_base = module;
    api.module_size = size;
    api.log = log;
    PATCH.set(api.patch_bytes).unwrap();
    api.patch_bytes = patch;
    HOOK_EXACT.set(api.hook_exact).unwrap();
    api.hook_exact = hook_exact;
    if scenario == "rust-fail-pickup" {
        api.hook_call = fail_pickup_hook;
    }
    let api = Box::leak(Box::new(api));
    let order = [
        (0, "core"),
        (2, "selection"),
        (4, "posture"),
        (3, "movement"),
        (5, "firing"),
        (8, "attack"),
        (9, "garrison"),
        (6, "ammunition"),
        (1, "pickup"),
        (7, "diagnostics"),
        (10, "preview_weapon"),
    ];
    let mut failed = Vec::new();
    if scenario == "unknown-build" {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join("logic.dll"))
            .unwrap()
            .write_all(b"unknown build")
            .unwrap();
    }
    for (id, name) in order {
        if skipped(scenario).contains(&(id as u32)) {
            continue;
        }
        let dll = if id == 0 {
            "defiance_plugin_core.dll".to_string()
        } else {
            format!("defiance_plugin_feature_{name}.dll")
        };
        let wide: Vec<u16> = plugins
            .join(dll)
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let library = unsafe { LoadLibraryW(wide.as_ptr()) };
        assert!(!library.is_null(), "load {name}");
        // The host hands its accepted plan to core before init.
        let address =
            unsafe { GetProcAddress(library, b"defiance_configure_enabled_v1\0".as_ptr()) };
        if !address.is_null() {
            let configure: unsafe extern "C" fn(u64) = unsafe { core::mem::transmute(address) };
            unsafe { configure(configure_mask(scenario)) };
        }
        let address = unsafe { GetProcAddress(library, b"defiance_plugin\0".as_ptr()) };
        assert!(!address.is_null());
        let entry: Entry = unsafe { core::mem::transmute(address) };
        let plugin = unsafe { &*entry() };
        assert_eq!(plugin.abi_version, api.abi_version);
        if scenario == "fail-ammo" && id == 6 {
            FAIL_AT.store(CALLS.load(Ordering::SeqCst) + 2, Ordering::SeqCst);
        }
        if scenario == "fail-firing" && id == 5 {
            FAIL_AT.store(CALLS.load(Ordering::SeqCst) + 1, Ordering::SeqCst);
        }
        if scenario == "fail-selection" && id == 2 {
            FAIL_AT.store(CALLS.load(Ordering::SeqCst) + 1, Ordering::SeqCst);
        }
        if (scenario == "fail-attack" && id == 8) || (scenario == "fail-garrison" && id == 9) {
            // Garrison has three spans: fail the second to exercise partial rollback.
            FAIL_AT.store(
                CALLS.load(Ordering::SeqCst) + usize::from(id == 9),
                Ordering::SeqCst,
            );
        }
        defiance_loader::test_host::begin_plugin(id as usize, name);
        let plugin_id = unsafe { CStr::from_ptr(plugin.name) }.to_str().unwrap();
        let dependencies = defiance_loader::config::builtin::find(plugin_id)
            .map(|b| b.depends.iter().map(|id| id.to_string()).collect())
            .unwrap_or_default();
        defiance_loader::test_host::begin_services(id as usize, plugin_id, dependencies);
        let services = unsafe { GetProcAddress(library, defiance_api::SERVICES_ENTRY.as_ptr()) };
        if !services.is_null() {
            let handshake: unsafe extern "C" fn(*const defiance_api::ServiceApiV1) -> i32 =
                unsafe { core::mem::transmute(services) };
            assert_eq!(
                unsafe { handshake(defiance_loader::test_host::service_api()) },
                0
            );
        }
        // Damage the approved call after Core preparation, before pickup init.
        if id == 1 && scenario == "rust-changed-pickup" {
            let byte = unsafe { *targets[0].base.add(p.call_site) };
            corrupted = Some((p.call_site, byte));
            unsafe {
                *targets[0].base.add(p.call_site) = byte ^ 0xff;
            }
        }
        let result = unsafe { (plugin.init)(api) };
        defiance_loader::test_host::finish_services(id as usize, result == 0);
        defiance_loader::test_host::end_plugin();
        if result != 0 {
            failed.push(id);
            defiance_loader::test_host::remove_owned(id as usize);
            if scenario == "fail-selection" && id == 2 {
                for hook in p.detours.iter().filter(|h| h.feature == 2) {
                    assert_eq!(
                        snapshot(targets[0].base as usize + hook.rva, hook.displaced.len()),
                        originals[0][hook.rva..hook.rva + hook.displaced.len()],
                        "failed selection restored immediately"
                    );
                }
            }
        }
    }
    if scenario == "unknown-build" {
        assert_eq!(failed, [0, 2, 4, 3, 5, 8, 9, 6, 1, 7, 10]);
    } else if scenario == "without-core" {
        assert_eq!(failed, [2, 4, 3, 5, 8, 9, 6, 1, 7, 10]);
    } else if scenario == "fail-ammo" {
        assert_eq!(failed, [6]);
    } else if scenario == "fail-attack" {
        assert_eq!(failed, [8]);
    } else if scenario == "fail-garrison" {
        assert_eq!(failed, [9]);
    } else if scenario == "fail-firing" {
        assert_eq!(failed, [5]);
    } else if scenario == "fail-selection" {
        assert_eq!(failed, [2]);
    } else if [
        "rust-fail-pickup",
        "rust-changed-pickup",
        "rust-disabled-pickup",
    ]
    .contains(&scenario)
    {
        assert_eq!(failed, [1]);
    } else if scenario == "shared-helper-corrupt" {
        // The shared anchor is required by every consumer, so damaging it
        // refuses the whole preparation on both the source and a relocated
        // build.
        assert_eq!(failed, [0, 2, 4, 3, 5, 8, 9, 6, 10]);
    } else if scenario == "enabled-ammo-corrupt" {
        // Only ammunition is refused: its install on the source build, its
        // dropped signature on a relocated build. Other features install.
        assert_eq!(failed, [6], "only ammunition should be refused");
    } else {
        assert!(failed.is_empty(), "failed plugins: {failed:?}");
    }
    // Dependency-failure mode must restore every installed plugin to stock.
    // The byte parity needs the pickup call (the block anchor in logic.dll) and
    // a game.dll hook (selection or ammunition) to be installed; a scenario
    // that omits those is checked for rollback instead.
    let compare = ![
        "without-selection",
        "fail-selection",
        "without-core",
        "unknown-build",
        "shared-helper-corrupt",
    ]
    .contains(&scenario)
        && !omitted(scenario).contains(&1);
    if compare {
        let actual: Vec<_> = targets
            .iter()
            .map(|t| snapshot(t.base as usize, t.size))
            .collect();
        let pickup_target = reached(&actual[0], p.call_site, targets[0].base as usize);
        let new_logic = if rust_pickup {
            let anchor = p
                .detours
                .iter()
                .find(|h| {
                    h.feature == 2 && !native_functions.iter().any(|(_, n)| n.entry == h.entry)
                })
                .expect("selection adapter anchor");
            let base = reached(&actual[0], anchor.rva, targets[0].base as usize) - anchor.entry;
            assert_ne!(
                pickup_target, base,
                "Rust pickup must replace the assembly chooser"
            );
            unsafe { exercise_rust_pickup(pickup_target) };
            base
        } else {
            pickup_target
        };
        let new_game =
            reached(&actual[1], g.hooks[0].rva, targets[1].base as usize) - g.hooks[0].entry;
        let delta = new_logic as isize - old_logic as isize;
        shift(
            &mut expected[0],
            p.call_site,
            pickup_target as isize - old_logic as isize,
        );
        shift(&mut expected[0], p.move_call_site, delta);
        for h in &p.detours {
            shift(&mut expected[0], h.rva, delta);
        }
        for &(name, hook) in &native_functions {
            if !omitted(scenario).contains(&hook.feature) {
                let target = reached(&actual[0], hook.rva, targets[0].base as usize);
                assert_ne!(
                    target,
                    new_logic + hook.entry,
                    "{name} still targets assembly"
                );
                if hook.displaced.len() >= 14 {
                    let mut branch = vec![0xff, 0x25, 0, 0, 0, 0];
                    branch.extend_from_slice(&(target as u64).to_le_bytes());
                    branch.resize(hook.displaced.len(), 0x90);
                    expected[0][hook.rva..hook.rva + branch.len()].copy_from_slice(&branch);
                } else {
                    shift(
                        &mut expected[0],
                        hook.rva,
                        target as isize - (new_logic + hook.entry) as isize,
                    );
                }
                if name == "firing_ui" || name == "is_selected" {
                    let query: unsafe extern "C" fn(*mut c_void) -> u8 =
                        unsafe { core::mem::transmute(target) };
                    assert_eq!(unsafe { query(core::ptr::null_mut()) }, 0);
                }
            }
        }
        // Execute the production setter/getter pairs through the installed
        // entry branches, not only the test-export wrappers used for parity.
        for (feature, setter, getter, size, enabled, changed) in [
            (2, "setter", "is_selected", 0x40, Some(0x18), 0x30),
            (5, "firing_set", "firing_ui", 0x300, None, 0x228),
        ] {
            if omitted(scenario).contains(&feature) {
                continue;
            }
            let address = |name| {
                let (_, hook) = native_functions.iter().find(|(n, _)| *n == name).unwrap();
                reached(&actual[0], hook.rva, targets[0].base as usize)
            };
            let set: unsafe extern "C" fn(*mut c_void, u8) =
                unsafe { core::mem::transmute(address(setter)) };
            let get: unsafe extern "C" fn(*mut c_void) -> u8 =
                unsafe { core::mem::transmute(address(getter)) };
            let mut object = vec![0u64; size / 8];
            let bytes =
                unsafe { core::slice::from_raw_parts_mut(object.as_mut_ptr().cast::<u8>(), size) };
            if let Some(offset) = enabled {
                bytes[offset] = 1;
            }
            let mut expected_object = bytes.to_vec();
            let ptr = bytes.as_mut_ptr().cast();
            for value in [127, 0] {
                unsafe { set(ptr, value) };
                expected_object[changed] = value;
                assert_eq!(
                    &*bytes,
                    expected_object.as_slice(),
                    "{setter}: only the intended field changes"
                );
                assert_eq!(
                    unsafe { get(ptr) },
                    if feature == 2 {
                        u8::from(value != 0)
                    } else {
                        value
                    }
                );
            }
        }
        for c in &p.pose_calls {
            shift(&mut expected[0], c.rva, delta);
        }
        for h in &g.hooks {
            shift(
                &mut expected[1],
                h.rva,
                new_game as isize - old_game as isize,
            );
        }
        // Restore every write a not-installed feature would have made, so the
        // rest of the module can be compared byte for byte.
        for feature in omitted(scenario) {
            for (rva, len) in p
                .detours
                .iter()
                .filter(|h| h.feature == feature)
                .map(|h| (h.rva, h.displaced.len()))
                .chain(
                    p.pose_calls
                        .iter()
                        .filter(|c| c.feature == feature)
                        .map(|c| (c.rva, c.before.len())),
                )
            {
                expected[0][rva..rva + len].copy_from_slice(&originals[0][rva..rva + len]);
            }
            for h in g.hooks.iter().filter(|h| h.feature == feature) {
                expected[1][h.rva..h.rva + h.displaced.len()]
                    .copy_from_slice(&originals[1][h.rva..h.rva + h.displaced.len()]);
            }
            if feature == 1 && !p.call_before.is_empty() {
                let (rva, len) = (p.call_site, p.call_before.len());
                expected[0][rva..rva + len].copy_from_slice(&originals[0][rva..rva + len]);
            }
            if feature == 3 && !p.move_displaced.is_empty() {
                let (rva, len) = (p.move_call_site, p.move_displaced.len());
                expected[0][rva..rva + len].copy_from_slice(&originals[0][rva..rva + len]);
            }
            if feature == 2 {
                for (rva, before) in [
                    (p.select_is_rva, &p.select_is_before),
                    (p.select_squad_rva, &p.select_squad_before),
                    (p.select_toggle_rva, &p.select_toggle_before),
                    (p.select_type_rva, &p.select_type_before),
                ] {
                    if !before.is_empty() {
                        expected[0][rva..rva + before.len()]
                            .copy_from_slice(&originals[0][rva..rva + before.len()]);
                    }
                }
            }
        }
        // The disabled ammunition site stays corrupted in memory; expected
        // carries the same byte so the rest of the comparison is meaningful.
        if let Some((at, byte)) = corrupted {
            expected[0][at] = byte ^ 0xff;
        }
        for i in 0..2 {
            assert!(
                actual[i] == expected[i],
                "module {i} differs at {:?}",
                actual[i].iter().zip(&expected[i]).position(|(a, b)| a != b)
            );
        }
        // A partial configuration leaves the disabled feature's block slots
        // unresolved on purpose, so only the module bytes above are compared;
        // reachable code and selected writes already matched.
        let partial_block = (scenario == "disabled-ammo-corrupt"
            || scenario == "enabled-ammo-corrupt")
            && relocated;
        for fix in &p.rel_fixups {
            let old = i32::from_le_bytes(old_code[fix.offset..fix.offset + 4].try_into().unwrap());
            old_code[fix.offset..fix.offset + 4]
                .copy_from_slice(&i32::try_from(old as isize - delta).unwrap().to_le_bytes());
        }
        if partial_block {
            // Every differing block byte must lie in a disabled feature's fixup
            // slot: the enabled reachable code and its fixups are unchanged.
            let actual_block = snapshot(new_logic, payload.len());
            let disabled_slots: Vec<usize> = p
                .rel_fixups
                .iter()
                .filter(|fix| fix.feature == 6)
                .flat_map(|fix| fix.offset..fix.offset + 4)
                .collect();
            for (off, (a, b)) in actual_block.iter().zip(&old_code).enumerate() {
                if a != b {
                    assert!(
                        disabled_slots.contains(&off),
                        "enabled logic block byte at +{off:#x} differs"
                    );
                }
            }
            let actual_game = snapshot(new_game, game_payload.len());
            let disabled_game: Vec<usize> = g
                .fixups
                .iter()
                .filter(|fix| fix.feature == 6)
                .flat_map(|fix| fix.offset..fix.offset + 8)
                .collect();
            for (off, (a, b)) in actual_game.iter().zip(&old_game_code).enumerate() {
                if a != b {
                    assert!(
                        disabled_game.contains(&off),
                        "enabled game block byte at +{off:#x} differs"
                    );
                }
            }
            println!("  partial configuration: only inactive slots differ; enabled code and fixups match");
        } else {
            assert_eq!(
                snapshot(new_logic, payload.len()),
                old_code,
                "logic payload"
            );
            assert_eq!(
                snapshot(new_game, game_payload.len()),
                old_game_code,
                "game payload"
            );
        }
    }
    if omitted(scenario).contains(&1) {
        let mut original = originals[0][p.call_site..p.call_site + 5].to_vec();
        if scenario == "rust-changed-pickup" {
            original[0] ^= 0xff;
        }
        assert_eq!(
            snapshot(targets[0].base as usize + p.call_site, 5),
            original,
            "disabled/refused pickup must not alter its call site"
        );
    }
    if !failed.contains(&2) && !skipped(scenario).contains(&2) {
        defiance_loader::test_host::begin_services(
            100,
            "test.consumer",
            vec!["defiance.selection".into()],
        );
        let table = unsafe {
            (defiance_loader::test_host::service_api().query)(
                c"defiance.selection".as_ptr(),
                c"selection".as_ptr(),
                1,
                core::mem::size_of::<defiance_api::SelectionV1>(),
            )
        };
        assert!(!table.is_null());
        let selection = unsafe { &*table.cast::<defiance_api::SelectionV1>() };
        assert_eq!(unsafe { (selection.is_selected)(core::ptr::null_mut()) }, 0);
        unsafe extern "C" fn selected(_: *mut c_void) -> u8 {
            7
        }
        let mut vt = [0usize; 12];
        vt[0x58 / 8] = selected as *const () as usize;
        let mut facet = [vt.as_ptr() as usize];
        assert_eq!(
            unsafe { (selection.is_selected)(facet.as_mut_ptr().cast()) },
            1
        );
        defiance_loader::test_host::finish_services(100, false);
    }
    for (id, _) in order.into_iter().rev() {
        defiance_loader::test_host::remove_owned(id as usize);
        defiance_loader::test_host::remove_services(id as usize);
    }
    if let Some((at, byte)) = corrupted {
        unsafe {
            *targets[0].base.add(at) = byte;
        }
    }
    for (i, t) in targets.iter().enumerate() {
        assert_eq!(
            snapshot(t.base as usize, t.size),
            originals[i],
            "rollback restores stock module"
        );
    }
    println!("PASS {scenario}: actual plugin DLLs and complete rollback; byte parity checked where features initialize");
}
