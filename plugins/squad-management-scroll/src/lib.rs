//! Opt-in, startup-only squad-management viewport. Never hot unload.
use core::ffi::c_void;
use defiance_api::{Api, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO, LOG_WARN};
use std::sync::{
    atomic::{AtomicBool, AtomicPtr, Ordering},
    OnceLock,
};
mod native;
mod sites;
mod viewport;

struct Site {
    rva: usize,
    before: &'static [u8],
}
struct Build {
    sha: &'static str,
    refresh: Site,
    dispatch: Site,
    destroy: Site,
    ammo: Site,
    weapon: Site,
    perk: Site,
    /// The upgrade_slot_* column bind, shared by both panels: (widget, display).
    upgrade: Site,
    script: Site,
    listen: Site,
    thumb: Site,
    slider_dispatch: Site,
    panel_vtable: usize,
    vehicle_vtable: usize,
    vehicle_destroy: Site,
    vehicle_refresh: Site,
    slider_vtable: usize,
    slider_ctrl_vtable: usize,
    context_service: usize,
    /// The vtable slot of the squad's rank (from its experience) on the stock
    /// perk refresh's service object (0x878; 0x880 in the 2026-09 builds).
    perk_limit: usize,
    /// The getter of the training table's key: it stores a static pointer in
    /// its out-parameter, and pointer + 8 resolves training names.
    training_key: Site,
    /// The lazily-initialised upgrade key static: the stock upgrade loop reads
    /// the pointer here and passes pointer + 8 to the script service's display
    /// resolver. The stock refresh initialises it before the rerun.
    upgrade_key: usize,
    /// The squad and vehicle panels' drop-target choosers, which the army
    /// presets window calls as an item drag starts: (panel, &out, item).
    squad_chooser: Site,
    vehicle_chooser: Site,
}
impl Build {
    fn functions(&self) -> [&Site; 16] {
        [
            &self.refresh,
            &self.dispatch,
            &self.destroy,
            &self.ammo,
            &self.weapon,
            &self.perk,
            &self.upgrade,
            &self.script,
            &self.listen,
            &self.thumb,
            &self.slider_dispatch,
            &self.training_key,
            &self.vehicle_destroy,
            &self.vehicle_refresh,
            &self.squad_chooser,
            &self.vehicle_chooser,
        ]
    }
}
static ENGINE: OnceLock<native::Engine> = OnceLock::new();
static ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ORIGINAL_VEHICLE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ORIGINAL_THUMB: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ORIGINAL_SQUAD_CHOOSER: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ORIGINAL_VEHICLE_CHOOSER: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ACTIVE: AtomicBool = AtomicBool::new(false);

unsafe fn log(api: &Api, level: u32, text: &str) {
    if let Ok(text) = std::ffi::CString::new(text) {
        (api.log)(level, text.as_ptr());
    }
}
#[link(name = "kernel32")]
extern "system" {
    fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, capacity: u32) -> u32;
}

unsafe fn install(api: &Api) -> Result<(), String> {
    let base = (api.module_base)(c"game.dll".as_ptr());
    if base.is_null() {
        return Err("game.dll is not loaded".into());
    }
    let size = (api.module_size)(base);
    let mut path = [0u16; 32768];
    let length = GetModuleFileNameW(base, path.as_mut_ptr(), path.len() as u32) as usize;
    if length == 0 || length >= path.len() {
        return Err("cannot resolve game.dll path".into());
    }
    use std::os::windows::ffi::OsStringExt;
    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&path[..length]));
    let sha = defiance_core::sha256::file(&path).map_err(|e| e.to_string())?;
    let build = sites::BUILDS
        .iter()
        .find(|b| b.sha == sha)
        .ok_or("unsupported game.dll; no writes made")?;
    for site in build.functions() {
        if site
            .rva
            .checked_add(site.before.len())
            .is_none_or(|end| end > size)
            || core::slice::from_raw_parts(base.cast::<u8>().add(site.rva), site.before.len())
                != site.before
        {
            return Err(format!(
                "native code differs at {:#x}; no writes made",
                site.rva
            ));
        }
    }
    if build.panel_vtable + 16 > size
        || build.vehicle_vtable + 16 > size
        || build.slider_vtable + 8 > size
        || build.slider_ctrl_vtable + 16 > size
    {
        return Err("vtable outside game.dll".into());
    }
    let base = base as usize;
    let patches = [
        (
            base + build.panel_vtable,
            base + build.destroy.rva,
            native::destroy as *const () as usize,
        ),
        (
            base + build.panel_vtable + 8,
            base + build.dispatch.rva,
            native::dispatch as *const () as usize,
        ),
        (
            base + build.vehicle_vtable,
            base + build.vehicle_destroy.rva,
            native::destroy as *const () as usize,
        ),
        (
            base + build.vehicle_vtable + 8,
            base + build.dispatch.rva,
            native::dispatch as *const () as usize,
        ),
        (
            base + build.slider_ctrl_vtable + 8,
            base + build.slider_dispatch.rva,
            native::slider_dispatch as *const () as usize,
        ),
    ];
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
    // leaving the stock row and hiding its scrollbar.
    let flag = |key: &str| {
        defiance_feature_sdk::boolean(api, "defiance.squad-management-scroll", key)
            .map_err(|e| format!("{key}: {e}"))
    };
    let upgrades = flag("upgrades")?;
    let vehicles = flag("vehicles")?;
    ENGINE
        .set(native::Engine::new(
            base,
            build,
            api.log,
            perk_slots,
            upgrade_slots,
            upgrades,
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
    for (site, detour, original, name) in [
        (
            &build.refresh,
            native::refresh as *mut _,
            ORIGINAL.as_ptr(),
            "refresh",
        ),
        (
            &build.vehicle_refresh,
            native::refresh_vehicle as *mut _,
            ORIGINAL_VEHICLE.as_ptr(),
            "vehicle refresh",
        ),
        // The slider thumb layout, for the vertical upgrade sliders.
        (
            &build.thumb,
            native::thumb_layout as *mut _,
            ORIGINAL_THUMB.as_ptr(),
            "thumb layout",
        ),
        // The drag-start choosers, which the upgrade column scrolls ahead of.
        (
            &build.squad_chooser,
            native::choose_squad as *mut _,
            ORIGINAL_SQUAD_CHOOSER.as_ptr(),
            "squad chooser",
        ),
        (
            &build.vehicle_chooser,
            native::choose_vehicle as *mut _,
            ORIGINAL_VEHICLE_CHOOSER.as_ptr(),
            "vehicle chooser",
        ),
    ] {
        if (api.hook_exact)(
            (base + site.rva) as *mut _,
            detour,
            site.before.len(),
            original,
        ) != 0
        {
            for address in applied.into_iter().rev() {
                (api.unhook)(address);
            }
            return Err(format!(
                "{name} hook refused; host will finish owned rollback"
            ));
        }
        applied.push((base + site.rva) as *mut c_void);
    }
    ACTIVE.store(true, Ordering::Release);
    log(
        api,
        LOG_INFO,
        &format!(
            "squad scrolling installed: perk_slots={perk_slots}, upgrade_slots={upgrade_slots}, \
             upgrades={upgrades}, vehicles={vehicles}; companion UI mod required; \
             startup-only, no hot unload"
        ),
    );
    Ok(())
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
        Err(error) => {
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
defiance_feature_sdk::crash_handshake!();

#[cfg(test)]
mod tests {
    #[test]
    fn displaced_prologues_need_no_relocation() {
        for b in super::sites::BUILDS {
            assert!(b.refresh.before.len() >= 14);
            defiance_core::decode::validate_copy(b.refresh.before).unwrap();
        }
    }
}
