//! Load the actual plugin DLLs against mapped stock modules. Every module
//! byte must be stock or one of an installed feature's unit writes, and every
//! unit's code its blob linked where Core put it.
use core::ffi::{c_char, c_void, CStr};
use defiance_api::{Api, Entry};
use defiance_core::unit::{Fixup, Kind, Unit};
use defiance_core::Target;
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
static RTTI: OnceLock<Vec<defiance_core::pe::Mapped>> = OnceLock::new();
type ConfigGet = unsafe extern "C" fn(*const c_char, *const c_char) -> *const c_char;
static CONFIG_GET: OnceLock<ConfigGet> = OnceLock::new();
static PROBE_FILE: OnceLock<std::ffi::CString> = OnceLock::new();

unsafe extern "C" fn config_get(section: *const c_char, key: *const c_char) -> *const c_char {
    if !section.is_null()
        && !key.is_null()
        && unsafe { CStr::from_ptr(section) }.to_bytes() == b"defiance.diagnostics"
        && unsafe { CStr::from_ptr(key) }.to_bytes() == b"probe_file"
    {
        return PROBE_FILE
            .get_or_init(|| {
                let path = std::env::temp_dir().join(format!(
                    "defiance-native-host-{}-empty-probes.json",
                    std::process::id()
                ));
                std::ffi::CString::new(path.to_string_lossy().as_bytes()).unwrap()
            })
            .as_ptr();
    }
    CONFIG_GET
        .get()
        .map_or(std::ptr::null(), |get| unsafe { get(section, key) })
}

// Share the generated passenger contract with the byte inventory assertion.
mod passenger {
    #[derive(Clone, Copy)]
    pub(super) struct Site {
        pub rva: usize,
        pub before: &'static [u8],
    }
    #[allow(dead_code)]
    pub(super) struct Build {
        pub name: &'static str,
        pub tick: Site,
        pub deployment: Site,
        pub query: Site,
        pub choose: Site,
        pub command: Site,
        pub shared_refresh: Site,
        pub setter: Site,
        pub range: Site,
        pub move_acquire: Site,
        pub candidate_query: Site,
        pub capable: Site,
        pub ui: Site,
        pub gunner_count: usize,
        pub gunner_get: usize,
    }
    pub(super) const BUILDS: &[Build] = sites::BUILDS;
    mod sites {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/vehicle-special-fire/src/sites.rs"
        ));
    }
}
static PATCH: OnceLock<unsafe extern "C" fn(*mut c_void, *const u8, *const u8, usize) -> i32> =
    OnceLock::new();
static HOOK_EXACT: OnceLock<
    unsafe extern "C" fn(*mut c_void, *mut c_void, usize, *mut *mut c_void) -> i32,
> = OnceLock::new();
static FAIL_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
static CALLS: AtomicUsize = AtomicUsize::new(0);
const INSPECTION_MODULE_SIZE: usize = 0x1000;
static INSPECTION_MODULE: AtomicUsize = AtomicUsize::new(0);
static INSPECTION_PATTERN_CALLS: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" fn fail_pickup_hook(_: *mut c_void, _: *mut c_void, _: *mut *mut c_void) -> i32 {
    -1
}
unsafe extern "C" fn fail_unit_inspection_hook(
    _: *mut c_void,
    _: *mut c_void,
    _: *mut *mut c_void,
) -> i32 {
    -1
}
unsafe extern "C" fn inspection_module(name: *const c_char) -> *mut c_void {
    if unsafe { CStr::from_ptr(name) }.to_bytes() == b"game.dll" {
        INSPECTION_MODULE.load(Ordering::SeqCst) as *mut c_void
    } else {
        std::ptr::null_mut()
    }
}
unsafe extern "C" fn inspection_size(base: *mut c_void) -> usize {
    if base as usize == INSPECTION_MODULE.load(Ordering::SeqCst) {
        INSPECTION_MODULE_SIZE
    } else {
        0
    }
}
unsafe extern "C" fn inspection_find_pattern(
    base: *mut c_void,
    size: usize,
    _: *const c_char,
) -> *mut c_void {
    if base.is_null() || size != INSPECTION_MODULE_SIZE {
        return std::ptr::null_mut();
    }
    let index = INSPECTION_PATTERN_CALLS.fetch_add(1, Ordering::SeqCst);
    if unsafe { *(base as *const u8) } == 0 {
        return std::ptr::null_mut();
    }
    let Some(offset) = [0x100, 0x200, 0x300, 0x400].get(index) else {
        return std::ptr::null_mut();
    };
    unsafe { (base as *mut u8).add(*offset) as *mut c_void }
}
unsafe extern "C" fn inspection_config_get(
    section: *const c_char,
    key: *const c_char,
) -> *const c_char {
    if unsafe { CStr::from_ptr(section) }.to_bytes() != b"defiance.unit-inspection" {
        return std::ptr::null();
    }
    match unsafe { CStr::from_ptr(key) }.to_bytes() {
        b"show_allied" | b"show_neutral" | b"show_enemy" => c"true".as_ptr(),
        b"ally_weapon_toggles" => c"false".as_ptr(),
        b"own_colour" => c"teal".as_ptr(),
        b"allied_colour" => c"yellow".as_ptr(),
        b"neutral_colour" => c"grey-blue".as_ptr(),
        b"enemy_colour" => c"red".as_ptr(),
        _ => std::ptr::null(),
    }
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
// VirtualAlloc copies retain the file's preferred-base vtable pointers. Resolve
// their stock RTTI at that base, then return the live copy's method address.
unsafe extern "C" fn vtable_slot(class: *const c_char, slot: usize) -> *mut c_void {
    let class = unsafe { CStr::from_ptr(class) }.to_str().unwrap();
    for (i, image) in RTTI.get().unwrap().iter().enumerate() {
        if let Some(rva) =
            defiance_core::rtti::method(&image.image, image.base, class, slot, &|rva| {
                image.is_code(rva)
            })
        {
            return (MODULES.get().unwrap()[i].0 + rva) as *mut c_void;
        }
    }
    core::ptr::null_mut()
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
fn prepare_inspection_module(base: *mut u8) {
    unsafe {
        core::ptr::write_bytes(base, 0x90, INSPECTION_MODULE_SIZE);
        *base.add(0x386) = 0xe8;
        core::ptr::write_bytes(base.add(0x387), 0, 4);
        for (offset, bytes) in [
            (0x400 + 0x18a, &[0x4c, 0x8b, 0x82, 0x28, 0x06, 0, 0][..]),
            (0x400 + 0x1be, &[0xff, 0x90, 0x40, 0x06, 0, 0][..]),
            (0x400 + 0x1de, &[0xff, 0x90, 0x30, 0x06, 0, 0][..]),
            (0x400 + 0x1fe, &[0xff, 0x90, 0x38, 0x06, 0, 0][..]),
        ] {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(offset), bytes.len());
        }
    }
}

fn unit_inspection_partial_install_test(plugins: &PathBuf) {
    let path = plugins.join("defiance_plugin_unit_inspection.dll");
    let wide: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let library = unsafe { LoadLibraryW(wide.as_ptr()) };
    assert!(!library.is_null(), "load unit-inspection");
    let symbol = unsafe { GetProcAddress(library, b"defiance_plugin\0".as_ptr()) };
    assert!(!symbol.is_null(), "unit-inspection has a plugin entry");
    let entry: Entry = unsafe { core::mem::transmute(symbol) };
    let plugin = unsafe { &*entry() };
    assert_eq!(
        unsafe { CStr::from_ptr(plugin.name) }.to_bytes(),
        b"defiance.unit-inspection"
    );

    let module = unsafe { VirtualAlloc(std::ptr::null_mut(), INSPECTION_MODULE_SIZE, 0x3000, 0x40) }
        as *mut u8;
    assert!(!module.is_null(), "allocate the synthetic game module");
    INSPECTION_MODULE.store(module as usize, Ordering::SeqCst);

    for (case, expected_hooks) in [
        ("second ownership hook", 1),
        ("ammo click redirect", 2),
        ("relation label hook", 3),
    ] {
        prepare_inspection_module(module);
        let stock = snapshot(module as usize, INSPECTION_MODULE_SIZE);
        INSPECTION_PATTERN_CALLS.store(0, Ordering::SeqCst);
        CALLS.store(0, Ordering::SeqCst);
        FAIL_AT.store(usize::MAX, Ordering::SeqCst);

        let mut api = defiance_loader::test_host::build_api();
        api.module_base = inspection_module;
        api.module_size = inspection_size;
        api.find_pattern = inspection_find_pattern;
        api.config_get = inspection_config_get;
        api.hook_exact = hook_exact;
        api.log = log;
        match case {
            "second ownership hook" => FAIL_AT.store(1, Ordering::SeqCst),
            "ammo click redirect" => api.hook_call = fail_unit_inspection_hook,
            _ => api.hook = fail_unit_inspection_hook,
        }

        let owner = 0x7000 + expected_hooks;
        defiance_loader::test_host::begin_plugin(owner, "defiance.unit-inspection");
        let status = unsafe { (plugin.init)(&api) };
        defiance_loader::test_host::end_plugin();
        assert_ne!(status, 0, "{case} must fail plugin initialization");
        assert_eq!(
            INSPECTION_PATTERN_CALLS.load(Ordering::SeqCst),
            4,
            "{case} reaches the installation hooks on the synthetic build"
        );
        assert_eq!(
            defiance_loader::test_host::installed().len(),
            expected_hooks,
            "{case} leaves each earlier hook owned before rollback"
        );
        if let Some(stop) = plugin.stop {
            unsafe { stop() };
        }
        let (removed, failed) = defiance_loader::test_host::remove_owned_report(owner);
        assert_eq!(removed, expected_hooks, "{case} restores every owned hook");
        assert_eq!(failed, 0, "{case} rollback is complete");
        assert!(defiance_loader::test_host::installed().is_empty());
        assert_eq!(
            snapshot(module as usize, INSPECTION_MODULE_SIZE),
            stock,
            "{case} restores the synthetic game bytes"
        );
    }

    unsafe { core::ptr::write_bytes(module, 0, INSPECTION_MODULE_SIZE) };
    INSPECTION_PATTERN_CALLS.store(0, Ordering::SeqCst);
    CALLS.store(0, Ordering::SeqCst);
    let stock = snapshot(module as usize, INSPECTION_MODULE_SIZE);
    let mut api = defiance_loader::test_host::build_api();
    api.module_base = inspection_module;
    api.module_size = inspection_size;
    api.find_pattern = inspection_find_pattern;
    api.config_get = inspection_config_get;
    api.hook_exact = hook_exact;
    api.log = log;
    let owner = 0x7fff;
    defiance_loader::test_host::begin_plugin(owner, "defiance.unit-inspection");
    let status = unsafe { (plugin.init)(&api) };
    defiance_loader::test_host::end_plugin();
    assert_eq!(status, 0, "an unsupported build remains an optional no-op");
    assert_eq!(INSPECTION_PATTERN_CALLS.load(Ordering::SeqCst), 4);
    let contract_symbol =
        unsafe { GetProcAddress(library, b"defiance_patch_contract_v1\0".as_ptr()) };
    assert!(
        !contract_symbol.is_null(),
        "unit-inspection exports its contract"
    );
    let contract: unsafe extern "C" fn(*const Api) -> *const defiance_api::PatchContractV1 =
        unsafe { core::mem::transmute(contract_symbol) };
    let contract = unsafe { contract(&api) };
    assert!(
        !contract.is_null(),
        "unsupported build has an empty contract"
    );
    assert_eq!(unsafe { (*contract).count }, 0);
    assert_eq!(INSPECTION_PATTERN_CALLS.load(Ordering::SeqCst), 8);
    assert!(
        defiance_loader::test_host::installed().is_empty(),
        "an unsupported build stages no hooks"
    );
    let (removed, failed) = defiance_loader::test_host::remove_owned_report(owner);
    assert_eq!((removed, failed), (0, 0));
    assert_eq!(
        snapshot(module as usize, INSPECTION_MODULE_SIZE),
        stock,
        "an unsupported build leaves the module unchanged"
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
        "without-vehicle-special-fire" => vec![11],
        "without-core" => vec![0],
        "diagnostics-only" => vec![2, 4, 3, 5, 8, 9, 6, 1, 10, 11],
        "core-only" => vec![2, 4, 3, 5, 8, 9, 6, 1, 7, 10, 11],
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
        "rust-fail-pickup" | "rust-changed-pickup" => vec![1],
        "fail-firing" => vec![5],
        "without-posture" => vec![4, 3], // movement is refused without posture
        "without-selection" => vec![2, 4, 3, 5, 8, 9, 6],
        "diagnostics-only" => vec![2, 4, 3, 5, 8, 9, 6, 1, 7, 10, 11],
        "without-ammo" | "fail-ammo" | "disabled-ammo-corrupt" | "enabled-ammo-corrupt" => vec![6],
        "without-attack" | "fail-attack" => vec![8],
        "without-garrison" | "fail-garrison" => vec![9],
        "without-movement" => vec![3],
        "without-firing" => vec![5],
        "without-pickup" => vec![1],
        "without-diagnostics" => vec![7],
        "without-preview-weapon" => vec![10],
        "without-vehicle-special-fire" => vec![11],
        "core-only" => vec![2, 4, 3, 5, 8, 9, 6, 1, 7, 10, 11],
        "without-posture-and-ammo" => vec![4, 3, 6],
        "without-movement-and-ammo" => vec![3, 6],
        "shared-helper-corrupt" => vec![1, 7],
        _ => Vec::new(),
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
    let mut rtti = Vec::new();
    for name in ["logic.dll", "game.dll"] {
        let path = dir.join(name);
        let mapped = defiance_core::pe::map(&path).unwrap();
        let image = mapped.image.clone();
        rtti.push(mapped);
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
    assert!(RTTI.set(rtti).is_ok());
    // The reference build's units, as Core embeds them, relocated with the
    // same code Core uses when this copy is a verified build.
    let module_index = |unit: &Unit| usize::from(unit.module == "game.dll");
    // A unit's owner as this harness numbers the plugins (`order` below).
    let owner = |unit: &Unit| -> u32 {
        match unit.plugin.as_str() {
            "pickup" => 1,
            "selection" => 2,
            "movement" => 3,
            "posture" => 4,
            "firing" => 5,
            "ammunition" => 6,
            "diagnostics" => 7,
            "attack" => 8,
            "garrison" => 9,
            "preview-weapon" => 10,
            "vehicle-special-fire" => 11,
            _ => 0,
        }
    };
    let mut units: Vec<(Unit, Vec<u8>)> = Vec::new();
    let folder = repo.join("tools/variants/reference/units");
    let mut names: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    names.sort();
    for path in names {
        let unit = Unit::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let blob = std::fs::read(path.with_extension("bin")).unwrap();
        units.push((unit, blob));
    }
    let shas: Vec<String> = targets
        .iter()
        .map(|t| defiance_core::sha256::file(&t.path).unwrap())
        .collect();
    let relocated = {
        let core = &units
            .iter()
            .find(|(u, _)| u.name == "core-logic")
            .unwrap()
            .0;
        defiance_core::install::needs_relocation(
            &shas[0],
            &core.source_sha256,
            &core.verified,
            defiance_core::Scan::Default,
        )
    };
    let raw_units = units.clone();
    if relocated {
        for (unit, _) in &mut units {
            let i = module_index(unit);
            *unit = unit.relocate(&originals[i], &shas[i]).unwrap();
        }
    }
    let unit = |name: &str| &units.iter().find(|(u, _)| u.name == name).unwrap().0;
    let feature_units = |feature: u32| units.iter().filter(move |(u, _)| owner(u) == feature);
    // The Rust replacements, by name: each the jmp write over its function.
    let native_functions: Vec<(&str, &str, usize, usize)> = units
        .iter()
        .flat_map(|(u, _)| {
            u.natives.iter().map(move |n| {
                let write = u.writes.iter().find(|w| w.rva == n.rva).unwrap();
                (n.name.as_str(), u.name.as_str(), n.rva, write.before.len())
            })
        })
        .collect();
    let pickup_call = unit("pickup-logic").writes[0].rva;
    // An ammunition site, corrupted in memory. `disabled-ammo-corrupt` does not
    // load ammunition, so its site is never read and the rest install.
    // `enabled-ammo-corrupt` loads it: on the source build its install
    // refuses; on a relocated build its signature is not found and only it is
    // refused. On a relocated build the corruption is one byte of an
    // ammunition signature window that no other unit's site shares. Restored
    // before the final rollback check.
    let mut corrupted: Option<(usize, u8)> = None;
    let ammo = &raw_units
        .iter()
        .find(|(u, _)| u.name == "ammunition-logic")
        .unwrap()
        .0;
    let ammo_rva = ammo.writes[0].rva;
    let all_sites: Vec<(&str, &defiance_core::Site)> = raw_units
        .iter()
        .filter(|(u, _)| u.module == "logic.dll")
        .flat_map(|(u, _)| u.sites.iter().map(move |s| (u.name.as_str(), s)))
        .collect();
    let scenarios = scenario == "disabled-ammo-corrupt" || scenario == "enabled-ammo-corrupt";
    if scenarios && !relocated {
        let byte = unsafe { *targets[0].base.add(ammo_rva) };
        corrupted = Some((ammo_rva, byte));
        unsafe {
            *targets[0].base.add(ammo_rva) = byte ^ 0xff;
        }
    } else if scenarios {
        let moves = defiance_core::Moves::locate(
            &all_sites
                .iter()
                .map(|(_, s)| (*s).clone())
                .collect::<Vec<_>>(),
            &originals[0],
        )
        .unwrap();
        'choose: for site in &ammo.sites {
            if !(site.start <= ammo_rva && ammo_rva < site.start + site.pattern.len()) {
                continue;
            }
            let base = moves
                .at(site.start)
                .expect("the ammunition site was located");
            for (i, byte) in site.pattern.iter().enumerate() {
                let Some(byte) = byte else { continue };
                let at = base + i;
                let shared = all_sites.iter().any(|(owner, other)| {
                    *owner != "ammunition-logic" && {
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
    // The build anchor is required by every unit. Damaging it must refuse
    // Core, on the source build (the anchor check) and on a relocated build
    // (the anchor's signature is not found).
    if scenario == "shared-helper-corrupt" {
        let at = unit("core-logic").anchors[0].rva;
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
    CONFIG_GET.set(api.config_get).unwrap();
    api.config_get = config_get;
    api.module_base = module;
    api.module_size = size;
    api.vtable_slot = vtable_slot;
    api.log = log;
    PATCH.set(api.patch_bytes).unwrap();
    api.patch_bytes = patch;
    HOOK_EXACT.set(api.hook_exact).unwrap();
    api.hook_exact = hook_exact;
    if scenario == "rust-fail-pickup" {
        api.hook_call = fail_pickup_hook;
    }
    let api = Box::leak(Box::new(api));
    if scenario == "unit-inspection-partial-install" {
        unit_inspection_partial_install_test(&plugins);
        return;
    }
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
        (11, "vehicle_special_fire"),
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
            // Garrison has several spans: fail the second to exercise partial rollback.
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
            let byte = unsafe { *targets[0].base.add(pickup_call) };
            corrupted = Some((pickup_call, byte));
            unsafe {
                *targets[0].base.add(pickup_call) = byte ^ 0xff;
            }
        }
        let result = unsafe { (plugin.init)(api) };
        defiance_loader::test_host::finish_services(id as usize, result == 0);
        defiance_loader::test_host::end_plugin();
        if result != 0 {
            failed.push(id);
            defiance_loader::test_host::remove_owned(id as usize);
            if scenario == "fail-selection" && id == 2 {
                for (u, _) in feature_units(2) {
                    let i = module_index(u);
                    for w in &u.writes {
                        assert_eq!(
                            snapshot(targets[i].base as usize + w.rva, w.before.len()),
                            originals[i][w.rva..w.rva + w.before.len()],
                            "failed selection restored immediately"
                        );
                    }
                }
            }
        }
    }
    if scenario == "unknown-build" {
        assert_eq!(failed, [0, 2, 4, 3, 5, 8, 9, 6, 1, 10, 11]);
    } else if scenario == "without-core" {
        assert_eq!(failed, [2, 4, 3, 5, 8, 9, 6, 1, 10, 11]);
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
    } else if ["rust-fail-pickup", "rust-changed-pickup"].contains(&scenario) {
        assert_eq!(failed, [1]);
    } else if scenario == "shared-helper-corrupt" {
        // The build anchor is required by every unit, so damaging it refuses
        // Core, and with it every feature, on both the source and a relocated
        // build.
        assert_eq!(failed, [0, 2, 4, 3, 5, 8, 9, 6, 10, 11]);
    } else if scenario == "enabled-ammo-corrupt" {
        // Only ammunition is refused: its install on the source build, its
        // missing signature on a relocated build. Other features install.
        assert_eq!(failed, [6], "only ammunition should be refused");
    } else {
        assert!(failed.is_empty(), "failed plugins: {failed:?}");
    }
    // Every installed feature's writes must be exactly its units' writes, and
    // each unit's code exactly its blob linked where Core put it; everything
    // else in the modules is stock.
    let installed = |feature: u32| {
        feature != 7
            && !omitted(scenario).contains(&feature)
            && !failed.contains(&(feature as i32))
            && !skipped(scenario).contains(&feature)
    };
    let compare = !failed.contains(&0) && !skipped(scenario).contains(&0);
    if compare {
        let actual: Vec<_> = targets
            .iter()
            .map(|t| snapshot(t.base as usize, t.size))
            .collect();
        let mut expected = originals.clone();
        if installed(11) {
            let tick = unsafe { vtable_slot(c".?AVGunner@Leonardo@@".as_ptr(), 5) } as usize;
            let build = passenger::BUILDS
                .iter()
                .find(|build| targets[0].base as usize + build.tick.rva == tick)
                .expect("passenger native contract matches stock RTTI");
            let entry_sites = [
                (0, build.tick),
                (0, build.deployment),
                (0, build.choose),
                (0, build.command),
                (0, build.setter),
                (0, build.shared_refresh),
                (0, build.move_acquire),
                (0, build.candidate_query),
                (0, build.capable),
                (1, build.ui),
            ];
            for (i, site) in entry_sites {
                let at = site.rva;
                let length = site.before.len();
                assert_eq!(&originals[i][at..at + length], site.before);
                assert_eq!(actual[i][at], 0xe9, "passenger native entry holds a jump");
                let destination = reached(&actual[i], at, targets[i].base as usize);
                assert_ne!(destination, targets[i].base as usize + at);
                let mut branch = actual[i][at..at + 5].to_vec();
                branch.resize(length, 0x90);
                expected[i][at..at + length].copy_from_slice(&branch);
            }
        }
        // The trace ring every importing unit reaches must be one and the same.
        let mut ring: Option<usize> = None;
        for (u, blob) in &units {
            if u.plugin == "core" || !installed(owner(u)) {
                continue;
            }
            let i = module_index(u);
            let base = targets[i].base as usize;
            let native = |rva: usize| native_functions.iter().find(|n| n.2 == rva);
            // Where Core put the unit, from a branch into it that nothing
            // replaced; every such branch must agree.
            let mut at: Option<usize> = None;
            for w in u.writes.iter().filter(|w| w.kind != Kind::Edit) {
                if native(w.rva).is_some() || (owner(u) == 1 && rust_pickup) {
                    continue;
                }
                let here = reached(&actual[i], w.rva, base) - w.entry;
                assert_eq!(
                    *at.get_or_insert(here),
                    here,
                    "{}: branches disagree",
                    u.name
                );
            }
            if owner(u) == 1 && rust_pickup {
                // The Rust chooser replaces the assembly one at its call.
                let target = reached(&actual[0], pickup_call, base);
                unsafe { exercise_rust_pickup(target) };
                let mut after = vec![0xe8];
                after.extend_from_slice(
                    &i32::try_from(target as isize - (base + pickup_call + 5) as isize)
                        .unwrap()
                        .to_le_bytes(),
                );
                expected[0][pickup_call..pickup_call + 5].copy_from_slice(&after);
                continue;
            }
            let at = at.unwrap_or_else(|| panic!("{}: no branch reaches it", u.name));
            for (address, _, after) in u.placed(base, at).unwrap() {
                let rva = address - base;
                if native(rva).is_some() {
                    continue;
                }
                expected[i][rva..rva + after.len()].copy_from_slice(&after);
            }
            for &(name, _, rva, length) in native_functions.iter().filter(|n| n.1 == u.name) {
                let target = reached(&actual[0], rva, base);
                assert_ne!(
                    target,
                    at + u.writes.iter().find(|w| w.rva == rva).unwrap().entry,
                    "{name} still targets assembly"
                );
                let branch = if length >= 14 {
                    let mut branch = vec![0xff, 0x25, 0, 0, 0, 0];
                    branch.extend_from_slice(&(target as u64).to_le_bytes());
                    branch
                } else {
                    let mut branch = vec![0xe9];
                    branch.extend_from_slice(
                        &i32::try_from(target as isize - (base + rva + 5) as isize)
                            .unwrap()
                            .to_le_bytes(),
                    );
                    branch
                };
                let mut span = branch;
                span.resize(length, 0x90);
                expected[0][rva..rva + length].copy_from_slice(&span);
                if name == "firing_ui" || name == "is_selected" {
                    let query: unsafe extern "C" fn(*mut c_void) -> u8 =
                        unsafe { core::mem::transmute(target) };
                    assert_eq!(unsafe { query(core::ptr::null_mut()) }, 0);
                }
            }
            // The unit's code, linked where it is. Its cells are data: Core
            // fills some, and the code writes others.
            let code = snapshot(at, u.unit_bytes);
            for fixup in &u.fixups {
                if let Fixup::Cell { offset, end, .. } = fixup {
                    let disp = i32::from_le_bytes(code[*offset..offset + 4].try_into().unwrap());
                    let here = (at + end) as isize + disp as isize;
                    assert_eq!(
                        *ring.get_or_insert(here as usize),
                        here as usize,
                        "{}: a second trace ring",
                        u.name
                    );
                }
            }
            let mut linked = u
                .link(blob, base, at, &|_| ring, &|dll, name| {
                    defiance_core::apply::resolve_export(dll, name).map(|a| a as usize)
                })
                .unwrap();
            let mut code = code;
            for cell in &u.cells {
                let span = cell.offset..cell.offset + cell.bytes;
                linked[span.clone()].fill(0);
                code[span].fill(0);
            }
            assert!(
                code == linked,
                "{}: its code differs from its linked blob at +{:#x}",
                u.name,
                code.iter()
                    .zip(&linked)
                    .position(|(a, b)| a != b)
                    .unwrap_or(0)
            );
            if u.name == "selection-logic" {
                // Core fills the preview's material callback and the marquee's
                // key test before the hooks that read them.
                let cell = |name: &str| u.cell(name).unwrap().offset;
                assert_ne!(
                    snapshot(at + cell("preview_dim"), 8),
                    [0; 8],
                    "the preview callback is written"
                );
                assert_ne!(
                    snapshot(at + cell("marquee"), 8),
                    [0; 8],
                    "the marquee's key test is written"
                );
            }
        }
        // Execute the production setter/getter pairs through the installed
        // entry branches, not only the test-export wrappers used for parity.
        for (feature, setter, getter, size, enabled, changed) in [
            (2, "setter", "is_selected", 0x40, Some(0x18), 0x30),
            (5, "firing_set", "firing_ui", 0x300, None, 0x228),
        ] {
            if !installed(feature) {
                continue;
            }
            let address = |name| {
                let n = native_functions.iter().find(|n| n.0 == name).unwrap();
                reached(&actual[0], n.2, targets[0].base as usize)
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
        // Core hooks game functions in Rust whenever it initializes: the lobby
        // connection (the multiplayer guard) and the tactical state's
        // constructor and destructor (mission reports for hot reload). These are
        // the game.dll writes no feature owns. Each is found in the original
        // image, since the hook has changed the loaded one.
        let core_game = &raw_units
            .iter()
            .find(|(u, _)| u.name == "core-game")
            .unwrap()
            .0;
        let core_site = |name: &str| {
            let site = core_game
                .sites
                .iter()
                .find(|site| site.name == name)
                .unwrap();
            defiance_core::Moves::locate(core::slice::from_ref(site), &originals[1])
                .and_then(|moves| moves.at(site.start))
                .unwrap()
        };
        let mut hooked = |at: usize| {
            if actual[1][at] == expected[1][at] {
                return false;
            }
            assert!(
                matches!(actual[1][at], 0xe9 | 0xff),
                "{at:#x} holds Core's hook"
            );
            let span = (0..16)
                .rev()
                .find(|&i| actual[1][at + i] != expected[1][at + i])
                .unwrap()
                + 1;
            expected[1][at..at + span].copy_from_slice(&actual[1][at..at + span]);
            true
        };
        for name in ["tactical_state_ctor", "tactical_state_dtor"] {
            hooked(core_site(name));
        }
        let lobby = core_site("lobby_connect");
        if hooked(lobby) {
            // Before startup has recorded the blocking plugins the guard fails
            // closed, so calling the connection returns a failure without
            // touching the network.
            let connect: unsafe extern "system" fn(*mut c_void, *mut u8) -> *mut u8 =
                unsafe { core::mem::transmute(targets[1].base as usize + lobby) };
            let mut out = [0xaau8; 0x48];
            assert_eq!(
                unsafe { connect(core::ptr::null_mut(), out.as_mut_ptr()) },
                out.as_mut_ptr()
            );
            assert_eq!(out[0], 0, "the guarded connection fails");
            let size = usize::from_le_bytes(out[0x20..0x28].try_into().unwrap());
            assert!(size > 15, "with the message, on the game's heap");
        }
        // A corrupted byte stays corrupted in memory; expected carries it too.
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
    }
    if omitted(scenario).contains(&1) {
        let mut original = originals[0][pickup_call..pickup_call + 5].to_vec();
        if scenario == "rust-changed-pickup" {
            original[0] ^= 0xff;
        }
        assert_eq!(
            snapshot(targets[0].base as usize + pickup_call, 5),
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
    println!(
        "PASS {scenario}: actual plugin DLLs, their units' writes and code, and complete rollback"
    );
}
