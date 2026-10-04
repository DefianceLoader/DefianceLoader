//! Native entry hooks complement the passenger binding and mount exchange units.
use defiance_api::{
    Api, PatchContractEntryV1, PatchContractV1, LOG_ERROR, LOG_INFO, PATCH_KIND_ENTRY,
};
use std::{
    ffi::{c_void, CStr, CString},
    sync::{
        atomic::{AtomicUsize, Ordering},
        OnceLock,
    },
};

#[derive(Clone, Copy)]
pub(super) struct Site {
    rva: usize,
    before: &'static [u8],
}
pub(super) struct Build {
    name: &'static str,
    tick: Site,
    deployment: Site,
    query: Site,
    choose: Site,
    command: Site,
    shared_refresh: Site,
    setter: Site,
    range: Site,
    move_acquire: Site,
    candidate_query: Site,
    capable: Site,
    ui: Site,
    gunner_count: usize,
    gunner_get: usize,
}
#[path = "sites.rs"]
mod sites;
static ACTIVE: OnceLock<&'static Build> = OnceLock::new();

fn log(api: &Api, level: u32, message: &str) {
    if let Ok(text) = CString::new(message) {
        unsafe { (api.log)(level, text.as_ptr()) };
    }
}

/// Whether `site`'s expected prologue lies inside the module and matches it.
unsafe fn site_matches(base: usize, size: usize, site: Site) -> bool {
    base != 0
        && site
            .rva
            .checked_add(site.before.len())
            .is_some_and(|end| end <= size)
        && unsafe { std::slice::from_raw_parts((base + site.rva) as *const u8, site.before.len()) }
            == site.before
}

pub(super) unsafe fn install(api: &Api) -> i32 {
    let name = unsafe { defiance_feature_sdk::services::build() }
        .map(|service| unsafe { (service.name)() })
        .filter(|name| !name.is_null());
    let Some(name) = name else {
        log(
            api,
            LOG_ERROR,
            "passenger targeting: the loader's build service is unavailable",
        );
        return 1;
    };
    let name = unsafe { CStr::from_ptr(name) }.to_string_lossy();
    let base = unsafe { (api.module_base)(c"logic.dll".as_ptr()) } as usize;
    let tick = unsafe { (api.vtable_slot)(c".?AVGunner@Leonardo@@".as_ptr(), 5) } as usize;
    let Some(build) = sites::BUILDS.iter().find(|build| {
        (build.name == name || (name == "reference" && build.name == "steam-2025-12-23"))
            && base + build.tick.rva == tick
    }) else {
        log(
            api,
            LOG_ERROR,
            "passenger targeting: unsupported native build",
        );
        return 1;
    };
    let size = unsafe { (api.module_size)(base as *mut c_void) };
    for (site, class, slot) in [
        (build.tick, c".?AVGunner@Leonardo@@", 5),
        (build.deployment, c".?AVGunner@Leonardo@@", 40),
        (build.query, c".?AVGunner@Leonardo@@", 12),
        (build.choose, c".?AVGunner@Leonardo@@", 13),
        (build.command, c".?AVGunner@Leonardo@@", 7),
        (build.setter, c".?AVGun@Leonardo@@", 5),
        (build.range, c".?AVGun@Leonardo@@", 60),
    ] {
        let address = unsafe { (api.vtable_slot)(class.as_ptr(), slot) } as usize;
        if address != base + site.rva || !unsafe { site_matches(base, size, site) } {
            log(
                api,
                LOG_ERROR,
                &format!(
                    "passenger targeting: {} vf{slot} does not match {}",
                    class.to_string_lossy(),
                    build.name
                ),
            );
            return 1;
        }
    }
    for (label, site) in [
        ("shared-target refresh", build.shared_refresh),
        ("move-target helper", build.move_acquire),
        ("candidate query", build.candidate_query),
        ("gunner capability", build.capable),
    ] {
        if !unsafe { site_matches(base, size, site) } {
            log(
                api,
                LOG_ERROR,
                &format!(
                    "passenger targeting: {label} does not match {} logic.dll+{:#x}",
                    build.name, site.rva
                ),
            );
            return 1;
        }
    }
    let game = unsafe { (api.module_base)(c"game.dll".as_ptr()) } as usize;
    let game_size = unsafe { (api.module_size)(game as *mut c_void) };
    if !unsafe { site_matches(game, game_size, build.ui) } {
        log(
            api,
            LOG_ERROR,
            "passenger targeting: attack-button continuation does not match",
        );
        return 1;
    }
    crate::targeting::QUERY.store(base + build.query.rva, Ordering::Release);
    if let Err(message) = unsafe { crate::targeting::configure(api) } {
        log(api, LOG_ERROR, &message);
        return 1;
    }
    crate::orders::GUNNER_COUNT.store(build.gunner_count, Ordering::Release);
    crate::orders::GUNNER_GET.store(build.gunner_get, Ordering::Release);
    for (site, detour, original) in [
        (
            build.tick,
            crate::targeting::tick as *mut c_void,
            &crate::targeting::TICK_ORIGINAL,
        ),
        (
            build.choose,
            crate::targeting::choose as *mut c_void,
            &crate::targeting::CHOOSE_ORIGINAL,
        ),
        (
            build.deployment,
            crate::targeting::deployment as *mut c_void,
            &crate::targeting::DEPLOYMENT_ORIGINAL,
        ),
        (
            build.command,
            crate::targeting::command as *mut c_void,
            &crate::targeting::COMMAND_ORIGINAL,
        ),
        (
            build.setter,
            crate::targeting::shared_target as *mut c_void,
            &crate::targeting::SET_TARGET,
        ),
        (
            build.shared_refresh,
            crate::targeting::shared_refresh as *mut c_void,
            &crate::targeting::SHARED_REFRESH_ORIGINAL,
        ),
        (
            build.move_acquire,
            crate::targeting::move_acquire as *mut c_void,
            &crate::targeting::MOVE_ACQUIRE_ORIGINAL,
        ),
        (
            build.candidate_query,
            crate::targeting::candidate_query as *mut c_void,
            &crate::targeting::CANDIDATE_QUERY_ORIGINAL,
        ),
        (
            build.capable,
            crate::targeting::capable as *mut c_void,
            &crate::targeting::CAPABLE_ORIGINAL,
        ),
    ] {
        if unsafe { install_one(api, base, site, detour, original) } != 0 {
            return 1;
        }
    }
    if unsafe {
        install_one(
            api,
            game,
            build.ui,
            crate::ui::availability as *mut c_void,
            &crate::ui::ORIGINAL,
        )
    } != 0
    {
        return 1;
    }
    if ACTIVE.set(build).is_err() {
        return 1;
    }
    log(
        api,
        LOG_INFO,
        "passenger targeting: independent automatic targets and native attack orders enabled",
    );
    0
}

unsafe fn install_one(
    api: &Api,
    base: usize,
    site: Site,
    detour: *mut c_void,
    slot: &AtomicUsize,
) -> i32 {
    let mut original = std::ptr::null_mut();
    let result = unsafe {
        (api.hook_exact)(
            (base + site.rva) as *mut c_void,
            detour,
            site.before.len(),
            &mut original,
        )
    };
    if result != 0 || original.is_null() {
        log(
            api,
            LOG_ERROR,
            &format!("passenger targeting: entry hook failed at {:#x}", site.rva),
        );
        return 1;
    }
    slot.store(original as usize, Ordering::Release);
    0
}

pub(super) unsafe fn contract(units: *const PatchContractV1) -> *const PatchContractV1 {
    let (Some(units), Some(build)) = (unsafe { units.as_ref() }, ACTIVE.get()) else {
        return std::ptr::null();
    };
    if units.entries.is_null() || units.version != 1 {
        return std::ptr::null();
    }
    let mut entries = unsafe { std::slice::from_raw_parts(units.entries, units.count) }.to_vec();
    for (module, site) in [
        (c"logic.dll", build.tick),
        (c"logic.dll", build.deployment),
        (c"logic.dll", build.choose),
        (c"logic.dll", build.command),
        (c"logic.dll", build.setter),
        (c"logic.dll", build.shared_refresh),
        (c"logic.dll", build.move_acquire),
        (c"logic.dll", build.candidate_query),
        (c"logic.dll", build.capable),
        (c"game.dll", build.ui),
    ] {
        entries.push(PatchContractEntryV1 {
            module: module.as_ptr(),
            rva: site.rva,
            kind: PATCH_KIND_ENTRY,
            before: site.before.as_ptr(),
            before_len: site.before.len(),
            after: std::ptr::null(),
            after_len: 0,
        });
    }
    let entries = Box::leak(entries.into_boxed_slice());
    Box::into_raw(Box::new(PatchContractV1 {
        version: 1,
        size: std::mem::size_of::<PatchContractV1>() as u32,
        entries: entries.as_ptr(),
        count: entries.len(),
    }))
}
