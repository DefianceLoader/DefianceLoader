//! Native entry hooks complement the passenger binding and mount exchange units.
use crate::sites::{self, Site, Sites};
use defiance_api::{
    Api, PatchContractEntryV1, PatchContractV1, LOG_ERROR, LOG_INFO, PATCH_KIND_ENTRY,
};
use defiance_core::sites::{code_ranges, Image};
use std::{
    ffi::{c_void, CStr, CString},
    sync::atomic::{AtomicUsize, Ordering},
};

/// The module bases and the sites resolved in them.
pub(super) struct Resolved {
    logic: usize,
    game: usize,
    sites: Sites,
}

pub(super) fn log(api: &Api, level: u32, message: &str) {
    if let Ok(text) = CString::new(message) {
        unsafe { (api.log)(level, text.as_ptr()) };
    }
}

/// `module`'s base and a copy of its image with the code ranges read back
/// from the loader's original bytes, so a hook another plugin already placed
/// does not hide a site.
unsafe fn original_image(api: &Api, module: &CStr) -> Result<(usize, Vec<u8>), String> {
    let base = unsafe { (api.module_base)(module.as_ptr()) }.cast::<u8>();
    if base.is_null() {
        return Err(format!("{} is not loaded", module.to_string_lossy()));
    }
    let size = unsafe { (api.module_size)(base.cast()) };
    let mut image = unsafe { std::slice::from_raw_parts(base, size) }.to_vec();
    if let Some(original) = unsafe { defiance_feature_sdk::services::original() } {
        for (start, end) in code_ranges(&image) {
            let end = end.min(size);
            if start < end {
                // A failed read keeps the live bytes, which the signatures
                // then judge.
                unsafe {
                    (original.read)(
                        base as usize + start,
                        image[start..end].as_mut_ptr(),
                        end - start,
                    )
                };
            }
        }
    }
    Ok((base as usize, image))
}

/// Every site in the loaded logic.dll and game.dll, or why this build is not
/// supported. Nothing is written.
pub(super) unsafe fn resolve(api: &Api) -> Result<Resolved, String> {
    let (logic, logic_image) = unsafe { original_image(api, c"logic.dll") }?;
    let (game, game_image) = unsafe { original_image(api, c"game.dll") }?;
    let sites = sites::sites(
        &Image {
            image: &logic_image,
            base: logic,
        },
        &Image {
            image: &game_image,
            base: game,
        },
    )?;
    Ok(Resolved { logic, game, sites })
}

/// Configures targeting and places every entry hook at `resolved`'s sites.
pub(super) unsafe fn install(api: &Api, resolved: &Resolved) -> i32 {
    let Resolved { logic, game, sites } = resolved;
    let (logic, game) = (*logic, *game);
    crate::targeting::QUERY.store(logic + sites.query, Ordering::Release);
    let configured = unsafe {
        crate::targeting::configure(
            api,
            logic,
            logic + sites.setter.rva,
            logic + sites.range,
            logic + sites.target_equals,
            logic + sites.release,
        )
    };
    if let Err(message) = configured {
        log(api, LOG_ERROR, &message);
        return 1;
    }
    crate::orders::GUNNER_COUNT.store(sites.gunner_count, Ordering::Release);
    crate::orders::GUNNER_GET.store(sites.gunner_get, Ordering::Release);
    for (site, detour, original) in [
        (
            sites.tick,
            crate::targeting::tick as *mut c_void,
            &crate::targeting::TICK_ORIGINAL,
        ),
        (
            sites.choose,
            crate::targeting::choose as *mut c_void,
            &crate::targeting::CHOOSE_ORIGINAL,
        ),
        (
            sites.deployment,
            crate::targeting::deployment as *mut c_void,
            &crate::targeting::DEPLOYMENT_ORIGINAL,
        ),
        (
            sites.command,
            crate::targeting::command as *mut c_void,
            &crate::targeting::COMMAND_ORIGINAL,
        ),
        (
            sites.setter,
            crate::targeting::shared_target as *mut c_void,
            &crate::targeting::SET_TARGET,
        ),
        (
            sites.shared_refresh,
            crate::targeting::shared_refresh as *mut c_void,
            &crate::targeting::SHARED_REFRESH_ORIGINAL,
        ),
        (
            sites.move_acquire,
            crate::targeting::move_acquire as *mut c_void,
            &crate::targeting::MOVE_ACQUIRE_ORIGINAL,
        ),
        (
            sites.candidate_query,
            crate::targeting::candidate_query as *mut c_void,
            &crate::targeting::CANDIDATE_QUERY_ORIGINAL,
        ),
        (
            sites.capable,
            crate::targeting::capable as *mut c_void,
            &crate::targeting::CAPABLE_ORIGINAL,
        ),
    ] {
        if unsafe { install_one(api, logic, site, detour, original) } != 0 {
            return 1;
        }
    }
    if unsafe {
        install_one(
            api,
            game,
            sites.ui,
            crate::ui::availability as *mut c_void,
            &crate::ui::ORIGINAL,
        )
    } != 0
    {
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

/// `units` with the entry hooks at `resolved`'s sites appended.
pub(super) unsafe fn contract(
    units: *const PatchContractV1,
    resolved: &Resolved,
) -> *const PatchContractV1 {
    let Some(units) = (unsafe { units.as_ref() }) else {
        return std::ptr::null();
    };
    if units.entries.is_null() || units.version != 1 {
        return std::ptr::null();
    }
    let mut entries = unsafe { std::slice::from_raw_parts(units.entries, units.count) }.to_vec();
    let logic = resolved
        .sites
        .logic_hooks()
        .map(|site| (c"logic.dll", site));
    for (module, site) in logic.into_iter().chain([(c"game.dll", resolved.sites.ui)]) {
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
