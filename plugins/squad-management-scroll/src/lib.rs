//! Opt-in, startup-only squad-management viewport. Never hot unload.
use core::ffi::c_void;
use defiance_api::{
    Api, PatchContractV1, Plugin, ABI_VERSION, LOG_DEBUG, LOG_ERROR, LOG_INFO, LOG_WARN,
    PATCH_KIND_ENTRY,
};
use defiance_core::sites::{code_ranges, Image};
use std::sync::{
    atomic::{AtomicBool, AtomicPtr, Ordering},
    OnceLock,
};
mod native;
mod sites;
mod viewport;
use sites::Sites;

static ENGINE: OnceLock<native::Engine> = OnceLock::new();
static ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ORIGINAL_VEHICLE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ORIGINAL_THUMB: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ORIGINAL_SQUAD_CHOOSER: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ORIGINAL_VEHICLE_CHOOSER: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ORIGINAL_TRAINING_SHOW: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ACTIVE: AtomicBool = AtomicBool::new(false);

unsafe fn log(api: &Api, level: u32, text: &str) {
    if let Ok(text) = std::ffi::CString::new(text) {
        (api.log)(level, text.as_ptr());
    }
}
/// game.dll's sites, resolved on its original bytes, and the entry bytes each
/// hook relocates, from [`Sites::hooks`].
struct Resolved {
    base: *mut u8,
    size: usize,
    sites: Sites,
    entries: Vec<(usize, Vec<u8>, &'static str)>,
}

unsafe fn resolve(api: &Api) -> Result<Resolved, InstallError> {
    let base = (api.module_base)(c"game.dll".as_ptr()).cast::<u8>();
    if base.is_null() {
        return Err(InstallError::Failed("game.dll is not loaded".into()));
    }
    let size = (api.module_size)(base.cast());
    let mut image = core::slice::from_raw_parts(base, size).to_vec();
    if let Some(original) = defiance_feature_sdk::services::original() {
        for (start, end) in code_ranges(&image) {
            let end = end.min(size);
            if start < end {
                // A failed read keeps the live bytes, which the signatures
                // then judge.
                (original.read)(
                    base as usize + start,
                    image[start..end].as_mut_ptr(),
                    end - start,
                );
            }
        }
    }
    let game = Image {
        image: &image,
        base: base as usize,
    };
    let sites = sites::sites(&game).map_err(InstallError::UnsupportedBuild)?;
    if sites.training_vtable + 16 > size
        || sites.panel_vtable + 16 > size
        || sites.vehicle_vtable + 16 > size
        || sites.slider_vtable + 8 > size
        || sites.slider_ctrl_vtable + 16 > size
    {
        return Err(InstallError::UnsupportedBuild(
            "vtable outside game.dll".into(),
        ));
    }
    let entries = sites
        .hooks()
        .into_iter()
        .map(|(rva, len, name)| (rva, image[rva..rva + len].to_vec(), name))
        .collect();
    Ok(Resolved {
        base,
        size,
        sites,
        entries,
    })
}

enum InstallError {
    /// The build is not one the plugin supports; nothing was written.
    UnsupportedBuild(String),
    Failed(String),
}

impl From<&str> for InstallError {
    fn from(error: &str) -> Self {
        Self::Failed(error.into())
    }
}

impl From<String> for InstallError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

fn vtable_patches(base: usize, sites: &Sites) -> [(usize, usize, usize); 7] {
    [
        (
            base + sites.training_vtable,
            base + sites.training_destroy,
            native::destroy as *const () as usize,
        ),
        (
            base + sites.training_vtable + 8,
            base + sites.dispatch,
            native::dispatch as *const () as usize,
        ),
        (
            base + sites.panel_vtable,
            base + sites.destroy,
            native::destroy as *const () as usize,
        ),
        (
            base + sites.panel_vtable + 8,
            base + sites.dispatch,
            native::dispatch as *const () as usize,
        ),
        (
            base + sites.vehicle_vtable,
            base + sites.vehicle_destroy,
            native::destroy as *const () as usize,
        ),
        (
            base + sites.vehicle_vtable + 8,
            base + sites.dispatch,
            native::dispatch as *const () as usize,
        ),
        (
            base + sites.slider_ctrl_vtable + 8,
            base + sites.slider_dispatch,
            native::slider_dispatch as *const () as usize,
        ),
    ]
}

unsafe fn install(api: &Api) -> Result<(), InstallError> {
    let Resolved {
        base,
        size,
        sites,
        entries,
    } = resolve(api)?;
    // The bytes the plugin overwrites or relocates must still be the
    // original ones; another patch's edit refuses the plugin before any write.
    for (rva, before, name) in &entries {
        if rva + before.len() > size
            || core::slice::from_raw_parts(base.add(*rva), before.len()) != before.as_slice()
        {
            return Err(format!("{name} entry already modified; no writes made").into());
        }
    }
    let base = base as usize;
    let patches = vtable_patches(base, &sites);
    for &(at, before, _) in &patches {
        if *(at as *const usize) != before {
            return Err("panel vtable already modified; no writes made".into());
        }
    }
    // Settings, as the other plugins read them: the loader materialises the
    // manifest defaults and validates each value's range before init, so a
    // missing or invalid value refuses the plugin (before any write) rather
    // than silently becoming a default. The range check repeats the manifest's.
    let slots = |key: &str, range: core::ops::RangeInclusive<usize>| {
        let value = defiance_feature_sdk::integer(api, "defiance.squad-management-scroll", key)
            .map_err(|e| format!("{key}: {e}"))?;
        usize::try_from(value)
            .ok()
            .filter(|value| range.contains(value))
            .ok_or_else(|| format!("{key}: {value} is outside {range:?}"))
    };
    // How many perk entries the perk pick row exposes (five matches stock), and
    // the upgrade column's length.
    let perk_slots = slots(
        "perk_slots",
        native::PERK_SLOTS_MIN..=native::PERK_SLOTS_MAX,
    )?;
    let upgrade_slots = slots(
        "upgrade_slots",
        native::UPGRADE_SLOTS_MIN..=native::UPGRADE_SLOTS_MAX,
    )?;
    // The upgrade columns and the vehicle panel; each can be turned off,
    // leaving the stock row and hiding its scrollbar. `fit_upgrades` sizes the
    // upgrade column to what the unit's type can hold.
    let flag = |key: &str| {
        defiance_feature_sdk::boolean(api, "defiance.squad-management-scroll", key)
            .map_err(|e| format!("{key}: {e}"))
    };
    let upgrades = flag("upgrades")?;
    let fit_upgrades = flag("fit_upgrades")?;
    let vehicles = flag("vehicles")?;
    ENGINE
        .set(native::Engine::new(
            base,
            &sites,
            api.log,
            perk_slots,
            upgrade_slots,
            upgrades,
            fit_upgrades,
            vehicles,
        ))
        .map_err(|_| "already initialized")?;
    let mut applied = Vec::new();
    for (at, before, after) in patches {
        if (api.patch_bytes)(
            at as *mut _,
            before.to_le_bytes().as_ptr(),
            after.to_le_bytes().as_ptr(),
            8,
        ) != 0
        {
            for address in applied.into_iter().rev() {
                (api.unhook)(address);
            }
            return Err("vtable patch refused; host will finish owned rollback".into());
        }
        applied.push(at as *mut c_void);
    }
    // The loader publishes each ORIGINAL before opening its detour to other
    // threads. Both panels have their own refresh and share one detour body.
    let detours = [
        native::training_show as *mut c_void,
        native::refresh as *mut _,
        native::refresh_vehicle as *mut _,
        // The slider thumb layout, for the vertical upgrade sliders.
        native::thumb_layout as *mut _,
        // The drag-start choosers, which the upgrade column scrolls ahead of.
        native::choose_squad as *mut _,
        native::choose_vehicle as *mut _,
    ];
    let originals = [
        ORIGINAL_TRAINING_SHOW.as_ptr(),
        ORIGINAL.as_ptr(),
        ORIGINAL_VEHICLE.as_ptr(),
        ORIGINAL_THUMB.as_ptr(),
        ORIGINAL_SQUAD_CHOOSER.as_ptr(),
        ORIGINAL_VEHICLE_CHOOSER.as_ptr(),
    ];
    for (((rva, before, name), detour), original) in entries.iter().zip(detours).zip(originals) {
        if (api.hook_exact)((base + rva) as *mut _, detour, before.len(), original) != 0 {
            for address in applied.into_iter().rev() {
                (api.unhook)(address);
            }
            return Err(format!("{name} hook refused; host will finish owned rollback").into());
        }
        applied.push((base + rva) as *mut c_void);
    }
    ACTIVE.store(true, Ordering::Release);
    log(
        api,
        LOG_INFO,
        &format!(
            "squad scrolling installed: perk_slots={perk_slots}, upgrade_slots={upgrade_slots}, \
             upgrades={upgrades}, fit_upgrades={fit_upgrades}, vehicles={vehicles}; training chooser scrolling; companion UI mod required; \
             startup-only, no hot unload"
        ),
    );
    Ok(())
}

unsafe extern "C" fn patch_contract(api: *const Api) -> *const PatchContractV1 {
    let Some(api_ref) = (unsafe { api.as_ref() }) else {
        return core::ptr::null();
    };
    let Ok(Resolved {
        base,
        sites,
        entries,
        ..
    }) = (unsafe { resolve(api_ref) })
    else {
        return core::ptr::null();
    };
    let base = base as usize;
    let mut patches = vtable_patches(base, &sites)
        .into_iter()
        .map(|(at, before, _)| {
            defiance_feature_sdk::contract::Patch::bytes(
                c"game.dll",
                at - base,
                &before.to_le_bytes(),
            )
        })
        .collect::<Vec<_>>();
    patches.extend(entries.into_iter().map(|(rva, before, _)| {
        defiance_feature_sdk::contract::Patch {
            module: c"game.dll",
            rva,
            kind: PATCH_KIND_ENTRY,
            before,
            after: None,
        }
    }));
    unsafe { defiance_feature_sdk::contract::build(api, patches) }
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else {
        return 1;
    };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    match install(api) {
        Ok(()) => 0,
        Err(InstallError::UnsupportedBuild(error)) => {
            log(
                api,
                LOG_WARN,
                &format!("squad scrolling: not a supported build ({error}); no hooks installed"),
            );
            1
        }
        Err(InstallError::Failed(error)) => {
            log(api, LOG_ERROR, &format!("squad scrolling refused: {error}"));
            1
        }
    }
}
#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.squad-management-scroll".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

#[no_mangle]
pub unsafe extern "C" fn defiance_patch_contract_v1(api: *const Api) -> *const PatchContractV1 {
    unsafe { patch_contract(api) }
}
defiance_feature_sdk::crash_handshake!();
