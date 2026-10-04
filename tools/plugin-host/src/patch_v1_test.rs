use core::ffi::{c_void, CStr};
use defiance_api::{Api, PatchUnitV1, PatchV1, PATCH_KIND_BYTES};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
}

const SOURCE_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TARGET_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

pub(super) fn run() {
    const MODULE_BYTES: usize = 0x1000;
    let logic_image = vec![0; MODULE_BYTES];
    let mut game_image = vec![0; MODULE_BYTES];
    game_image[0x180..0x184].copy_from_slice(&[0xa1, 0xa2, 0xa3, 0xa4]);
    game_image[0x280..0x288].copy_from_slice(&[0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17]);
    game_image[0x380..0x384].copy_from_slice(&[0x20, 0x21, 0x22, 0x23]);
    game_image[0x480..0x488].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37]);

    let logic_base = synthetic_module(&logic_image);
    let game_base = synthetic_module(&game_image);
    unsafe {
        defiance_plugin_core::test_host::initialize_runtime(
            logic_base,
            TARGET_SHA,
            logic_image,
            game_base,
            TARGET_SHA,
            game_image,
        )
        .expect("Core synthetic runtime");
    }

    let api = defiance_loader::test_host::build_api();
    let patch = defiance_plugin_core::test_host::patch_api();
    let relocated = synthetic_unit(
        "relocated",
        &[("moved-site", 0x100, 1, "a1a2a3a4")],
        &[(0x100, "a1a2a3a4", "ffa2a3a4")],
    );
    defiance_loader::test_host::begin_plugin(501, "patch-v1-relocated");
    let (status, prepared) = install_patch_units(&api, patch, &[relocated]);
    defiance_loader::test_host::end_plugin();
    assert_eq!(status, 0, "the relocated unit installs");
    let contract = unsafe { (patch.contract)(&api, prepared) };
    assert!(!contract.is_null(), "the installed unit has a contract");
    let contract = unsafe { &*contract };
    assert_eq!(contract.count, 1);
    let entries = unsafe { std::slice::from_raw_parts(contract.entries, contract.count) };
    assert_eq!(
        unsafe { CStr::from_ptr(entries[0].module) }.to_bytes(),
        b"game.dll"
    );
    assert_eq!(entries[0].rva, 0x180, "the contract uses the relocated RVA");
    assert_eq!(entries[0].kind, PATCH_KIND_BYTES);
    assert_eq!(entries[0].before_len, 4);
    assert_eq!(
        unsafe { std::slice::from_raw_parts(entries[0].before, entries[0].before_len) },
        [0xa1, 0xa2, 0xa3, 0xa4],
        "the contract uses the relocated stock bytes"
    );
    assert_eq!(bytes(game_base, 0x100, 4), vec![0; 4]);
    assert_eq!(bytes(game_base, 0x180, 4), vec![0xff, 0xa2, 0xa3, 0xa4]);
    assert!(owns("patch-v1-relocated", game_base, 0x180, 4));
    assert_eq!(defiance_loader::test_host::remove_owned_report(501), (1, 0));
    assert_eq!(bytes(game_base, 0x180, 4), vec![0xa1, 0xa2, 0xa3, 0xa4]);
    println!("relocated unit: installed at synthetic game.dll+0x180");

    let owner = synthetic_unit(
        "overlap-owner",
        &[("owner-site", 0x200, 2, "101112131415")],
        &[(0x200, "101112131415", "cc1112131415")],
    );
    let overlap = synthetic_unit(
        "overlap-claim",
        &[("claim-site", 0x203, 3, "13141516")],
        &[(0x203, "1314151617", "13141516ff")],
    );
    defiance_loader::test_host::begin_plugin(502, "patch-v1-owner");
    assert_eq!(install_patch_units(&api, patch, &[owner]).0, 0);
    defiance_loader::test_host::end_plugin();
    defiance_loader::test_host::begin_plugin(503, "patch-v1-overlap");
    let (status, _) = install_patch_units(&api, patch, &[overlap]);
    defiance_loader::test_host::end_plugin();
    assert_ne!(status, 0, "an overlapping span is refused");
    assert_eq!(
        bytes(game_base, 0x280, 6),
        vec![0xcc, 0x11, 0x12, 0x13, 0x14, 0x15]
    );
    assert!(owns("patch-v1-owner", game_base, 0x280, 6));
    assert!(!owns("patch-v1-overlap", game_base, 0x283, 5));
    assert_eq!(defiance_loader::test_host::remove_owned_report(503), (0, 0));
    assert_eq!(defiance_loader::test_host::remove_owned_report(502), (1, 0));
    assert_eq!(
        bytes(game_base, 0x280, 6),
        vec![0x10, 0x11, 0x12, 0x13, 0x14, 0x15]
    );
    println!("overlap: refused without changing the existing owner's span");

    let guard = synthetic_unit(
        "rollback-guard",
        &[("guard-site", 0x400, 4, "3031323334353637")],
        &[(0x400, "3031323334353637", "cc31323334353637")],
    );
    defiance_loader::test_host::begin_plugin(504, "patch-v1-guard");
    assert_eq!(install_patch_units(&api, patch, &[guard]).0, 0);
    defiance_loader::test_host::end_plugin();
    let failed = synthetic_unit(
        "failed-unit",
        &[
            ("first-site", 0x300, 5, "20212223"),
            ("blocked-site", 0x404, 6, "34353637"),
        ],
        &[
            (0x300, "20212223", "ff212223"),
            (0x404, "34353637", "ff353637"),
        ],
    );
    defiance_loader::test_host::begin_plugin(505, "patch-v1-failed");
    let (status, _) = install_patch_units(&api, patch, &[failed]);
    defiance_loader::test_host::end_plugin();
    assert_ne!(status, 0, "the later write is refused by the guard");
    assert_eq!(bytes(game_base, 0x380, 4), vec![0xff, 0x21, 0x22, 0x23]);
    assert_eq!(
        bytes(game_base, 0x480, 8),
        vec![0xcc, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37]
    );
    assert_eq!(defiance_loader::test_host::remove_owned_report(505), (1, 0));
    assert_eq!(bytes(game_base, 0x380, 4), vec![0x20, 0x21, 0x22, 0x23]);
    assert!(owns("patch-v1-guard", game_base, 0x480, 8));
    assert_eq!(defiance_loader::test_host::remove_owned_report(504), (1, 0));
    assert_eq!(
        bytes(game_base, 0x480, 8),
        vec![0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37]
    );
    assert!(defiance_loader::test_host::installed().is_empty());
    println!("failed unit: later refusal rolled back its earlier write");
    println!("ownership: spans restore under their installing plugin");
}

fn synthetic_module(image: &[u8]) -> *mut u8 {
    unsafe {
        let base = VirtualAlloc(std::ptr::null_mut(), image.len(), 0x3000, 0x40).cast::<u8>();
        assert!(!base.is_null(), "could not allocate a synthetic module");
        core::ptr::copy_nonoverlapping(image.as_ptr(), base, image.len());
        base
    }
}

fn synthetic_unit(
    name: &str,
    sites: &[(&str, usize, usize, &str)],
    writes: &[(usize, &str, &str)],
) -> String {
    let sites = sites
        .iter()
        .map(|(site_name, start, group, pattern)| {
            format!(
                r#"{{"site_name":"{site_name}","site_start":{start},"site_group":{group},"site_pattern":"{pattern}"}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let writes = writes
        .iter()
        .map(|(rva, before, after)| {
            format!(r#"{{"kind":"edit","rva":{rva},"before":"{before}","after":"{after}"}}"#)
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"unit_schema":1,"name":"{name}","plugin":"services-test","module":"game.dll","source_sha256":"{SOURCE_SHA}","unit_bytes":16,"code_offset":0,"cells":[],"writes":[{writes}],"fixups":[],"natives":[],"anchors":[],"sites":[{sites}],"verified_sha":"{SOURCE_SHA}"}}"#
    )
}

fn install_patch_units(api: &Api, patch: &PatchV1, descriptors: &[String]) -> (i32, *mut c_void) {
    let code = [0x90; 16];
    let units: Vec<_> = descriptors
        .iter()
        .map(|descriptor| PatchUnitV1 {
            descriptor: descriptor.as_ptr(),
            descriptor_len: descriptor.len(),
            code: code.as_ptr(),
            code_len: code.len(),
        })
        .collect();
    unsafe {
        let prepared = (patch.prepare)(api, units.as_ptr(), units.len());
        assert!(
            !prepared.is_null(),
            "PatchV1 rejected a synthetic descriptor"
        );
        (
            (patch.install)(api, prepared, std::ptr::null(), 0, std::ptr::null_mut()),
            prepared,
        )
    }
}

fn bytes(base: *mut u8, rva: usize, length: usize) -> Vec<u8> {
    unsafe { std::slice::from_raw_parts(base.add(rva), length).to_vec() }
}

fn owns(owner: &str, base: *mut u8, rva: usize, length: usize) -> bool {
    defiance_loader::test_host::installed()
        .iter()
        .any(|(name, target, size, _)| {
            name == owner && *target == unsafe { base.add(rva) } as usize && *size == length
        })
}
