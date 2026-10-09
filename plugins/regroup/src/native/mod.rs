mod creation;
mod history;
mod input;
mod large_squads;
mod parking;
mod plan;
use crate::{
    model::{remap_pins, Supply},
    sites::{
        self, CLEANUP_DISPLACED, HOVER_DISPLACED, INPUT_DISPLACED, PERK_DISPLACED,
        ROSTER_DISPLACED, UPDATE_DISPLACED,
    },
};
use creation::*;
use defiance_api::{
    Api, PatchContractV1, ABI_VERSION, LOG_DEBUG, LOG_ERROR, LOG_INFO, LOG_WARN, PATCH_KIND_CALL,
    PATCH_KIND_ENTRY,
};
use defiance_core::sites::Image;
use input::*;
use large_squads::*;
use parking::*;
use plan::*;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    ffi::c_char,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        OnceLock,
    },
};

type Unary = unsafe extern "C" fn(usize);
type Pair = unsafe extern "C" fn(usize, usize);
type Get = unsafe extern "C" fn(usize) -> usize;
type Predicate = unsafe extern "C" fn(usize) -> u8;
static ADDRESSES: OnceLock<Vec<usize>> = OnceLock::new();
static LOGGER: AtomicUsize = AtomicUsize::new(0);
static SELECTION: OnceLock<&'static defiance_api::SelectionV1> = OnceLock::new();
static FACETS: OnceLock<&'static defiance_api::FacetsV1> = OnceLock::new();
static LIMITS: OnceLock<crate::limits::Limits> = OnceLock::new();
static MENU: OnceLock<&'static defiance_api::AmmoMenuV1> = OnceLock::new();

/// The class offsets [`init`] resolved for this build.
static OFFSETS: OnceLock<sites::Offsets> = OnceLock::new();

/// The resolved class offsets. Tests that never run [`init`] read the
/// December 2025 builds' values, which their fake objects are laid out for.
pub(crate) fn offsets() -> sites::Offsets {
    #[cfg(test)]
    if OFFSETS.get().is_none() {
        return sites::Offsets {
            roster: 0x3b8,
            world_manager: 0x700,
            input_player: 0x860,
            input_ui: 0x888,
        };
    }
    *OFFSETS
        .get()
        .expect("regroup reads offsets only after init")
}
#[cfg(test)]
thread_local! {
    static TEST_LIMITS: Cell<Option<(crate::limits::Limits, u32)>> = const { Cell::new(None) };
}
fn limits() -> crate::limits::Limits {
    #[cfg(test)]
    if let Some((limits, _)) = TEST_LIMITS.with(Cell::get) {
        return limits;
    }
    LIMITS.get().copied().unwrap_or_default().supported()
}
fn ammo_limit() -> usize {
    #[cfg(test)]
    if let Some((limits, slots)) = TEST_LIMITS.with(Cell::get) {
        return limits.ammo(slots);
    }
    limits().ammo(MENU.get().map_or(9, |menu| unsafe { (menu.capacity)() }))
}
// Do not risk repeated creation after an unverified native result. Thread-local
// because input, creation and restoration all run on the game's owning thread.
thread_local! {
    static CREATION_BLOCKED: Cell<bool> = const { Cell::new(false) };
    static CREATION_ABORTED: Cell<bool> = const { Cell::new(false) };
}

thread_local! {
    static CONTEXT: RefCell<Option<Plan>> = const { RefCell::new(None) };
    static DESTINATION: Cell<usize> = const { Cell::new(0) };
    static PREPARED_HOLDER: Cell<usize> = const { Cell::new(0) };
    static BINDING_HOLDER: Cell<usize> = const { Cell::new(0) };
    static CONSTRUCTED: Cell<bool> = const { Cell::new(false) };
}

fn address(id: usize) -> usize {
    ADDRESSES.get().unwrap()[id]
}
fn log(level: u32, message: &str) {
    let ptr = LOGGER.load(Ordering::Relaxed);
    if ptr == 0 {
        return;
    }
    let text = format!("defiance.regroup: {message}\0");
    unsafe {
        std::mem::transmute::<usize, unsafe extern "C" fn(u32, *const c_char)>(ptr)(
            level,
            text.as_ptr().cast(),
        );
    }
}
unsafe fn q(p: usize, offset: usize) -> usize {
    *((p + offset) as *const usize)
}
unsafe fn d(p: usize, offset: usize) -> u32 {
    *((p + offset) as *const u32)
}
unsafe fn byte(p: usize, offset: usize) -> u8 {
    *((p + offset) as *const u8)
}
unsafe fn put(p: usize, offset: usize, value: u32) {
    *((p + offset) as *mut u32) = value;
}
unsafe fn method(p: usize, offset: usize) -> usize {
    q(q(p, 0), offset)
}
unsafe fn get(p: usize, offset: usize) -> usize {
    std::mem::transmute::<usize, Get>(method(p, offset))(p)
}
unsafe fn call(id: usize, p: usize) {
    std::mem::transmute::<usize, Unary>(address(id))(p);
}
unsafe fn pair(id: usize, p: usize, arg: usize) {
    std::mem::transmute::<usize, Pair>(address(id))(p, arg);
}
unsafe fn junction(p: usize) -> usize {
    if p == 0 {
        0
    } else {
        q(p, 0x10)
    }
}
unsafe fn entity(facet: usize) -> usize {
    junction(q(facet, 0x10))
}
unsafe fn facets(e: usize) -> usize {
    get(e, 0xb0)
}
/// The entity's `SquadAiFacet`, or 0 for a missing facet or another AI type.
unsafe fn squad_ai(e: usize) -> usize {
    (FACETS.get().unwrap().squad_ai)(e as *mut core::ffi::c_void) as usize
}
unsafe fn kind(e: usize, flag: u32) -> bool {
    std::mem::transmute::<usize, unsafe extern "C" fn(usize, u32) -> u8>(method(e, 0x98))(e, flag)
        != 0
}
unsafe fn selected(facet: usize) -> bool {
    (SELECTION
        .get()
        .expect("selection service resolved before hooks")
        .is_selected)(facet as *mut _)
        != 0
}
unsafe fn pointers(p: usize, offset: usize, max: usize) -> Result<Vec<usize>, &'static str> {
    let (begin, end) = (q(p, offset), q(p, offset + 8));
    if end < begin || (end - begin) % 8 != 0 || (end - begin) / 8 > max || (begin == 0 && end != 0)
    {
        return Err("invalid or oversized entity vector");
    }
    Ok((begin..end)
        .step_by(8)
        .map(|p| *(p as *const usize))
        .collect())
}

#[derive(Clone)]
struct Gun {
    ptr: usize,
    ammo: usize,
    loaded: u32,
    supported: Vec<usize>,
}
#[derive(Clone)]
struct Member {
    entity: usize,
    ai: usize,
    gunner: usize,
    select: usize,
    picked: bool,
    guns: Vec<Gun>,
    pins: (u8, u8),
}
#[derive(Clone)]
struct Slot {
    ammo: usize,
    supply: Supply,
}
#[derive(Clone)]
struct Source {
    entity: usize,
    ai: usize,
    holder: usize,
    pool: usize,
    slots: Vec<Slot>,
    members: Vec<Member>,
    capacity: u32,
}
#[derive(Clone)]
struct PoolChange {
    source: usize,
    ammo: usize,
    before: Supply,
    left: Supply,
}
#[derive(Clone)]
struct Plan {
    world: usize,
    species: usize,
    team: usize,
    position: [f32; 3],
    sources: Vec<Source>,
    moved: Vec<Member>,
    pools: BTreeMap<usize, Supply>,
    changes: Vec<PoolChange>,
}

/// The loaded logic.dll and game.dll, as images.
unsafe fn modules(api: &Api) -> Result<(Image<'static>, Image<'static>), String> {
    let image = |name: &[u8]| -> Result<Image<'static>, String> {
        let base = unsafe { (api.module_base)(name.as_ptr().cast()) } as usize;
        if base == 0 {
            return Err(format!(
                "{} is not loaded",
                String::from_utf8_lossy(&name[..name.len() - 1])
            ));
        }
        Ok(unsafe { Image::loaded(base as *const u8, (api.module_size)(base as *mut _)) })
    };
    Ok((image(b"logic.dll\0")?, image(b"game.dll\0")?))
}

pub unsafe fn patch_contract(api: *const Api) -> *const PatchContractV1 {
    let Some(api_ref) = (unsafe { api.as_ref() }) else {
        return core::ptr::null();
    };
    if api_ref.abi_version != ABI_VERSION || api_ref.reserved != 0 {
        return core::ptr::null();
    }
    let Ok((logic, game)) = (unsafe { modules(api_ref) }) else {
        return core::ptr::null();
    };
    let Ok(sites) = sites::sites(&logic, &game) else {
        return core::ptr::null();
    };
    let patch = |module, image: &Image, rva: usize, kind, length: usize| {
        defiance_feature_sdk::contract::Patch {
            module,
            rva,
            kind,
            before: image.image[rva..rva + length].to_vec(),
            after: None,
        }
    };
    let mut patches = Vec::with_capacity(10);
    for index in [
        sites::SPAWN_FALLBACK_CALL,
        sites::CREATE_CALL,
        sites::WIRE_CALL,
        sites::TEMPLATES_CALL,
    ] {
        patches.push(patch(
            c"logic.dll",
            &logic,
            sites.logic[index],
            PATCH_KIND_CALL,
            5,
        ));
    }
    for (index, length) in sites::GAME_ENTRIES {
        patches.push(patch(
            c"game.dll",
            &game,
            sites.game[index],
            PATCH_KIND_ENTRY,
            length,
        ));
    }
    for (index, length) in sites::LOGIC_ENTRIES {
        patches.push(patch(
            c"logic.dll",
            &logic,
            sites.logic[index],
            PATCH_KIND_ENTRY,
            length,
        ));
    }
    unsafe { defiance_feature_sdk::contract::build(api, patches) }
}

pub unsafe extern "C" fn init(api: *const Api) -> i32 {
    if api.is_null() || (*api).abi_version != ABI_VERSION {
        return 1;
    }
    let api = &*api;
    LOGGER.store(api.log as usize, Ordering::Relaxed);
    let configured = (|| -> Result<crate::limits::Limits, String> {
        let soldiers = defiance_feature_sdk::integer(api, "defiance.regroup", "max_soldiers")
            .map_err(|e| e.to_string())?;
        let weapons = defiance_feature_sdk::integer(api, "defiance.regroup", "max_weapon_types")
            .map_err(|e| e.to_string())?;
        crate::limits::Limits::new(soldiers, weapons).map_err(str::to_owned)
    })();
    let configured = match configured {
        Ok(value) => value,
        Err(error) => {
            log(LOG_ERROR, &error);
            return 1;
        }
    };
    let Some(menu) = defiance_feature_sdk::services::ammo_menu() else {
        log(
            LOG_ERROR,
            "Core ammo-menu service v1 unavailable; update Core with regroup",
        );
        return 1;
    };
    if configured.soldiers > configured.supported().soldiers {
        log(LOG_WARN, "max_soldiers is temporarily capped at 20: native squad-command paths still have 20-member stack buffers");
    }
    if LIMITS.set(configured).is_err() || MENU.set(menu).is_err() {
        return 1;
    }
    log(LOG_INFO, &format!("limits: max_soldiers={}, max_weapon_types={} (0=automatic); effective ammo cap read from Core at each operation", configured.soldiers, configured.weapon_types));
    let Some(selection) = defiance_feature_sdk::services::selection() else {
        log(
            LOG_ERROR,
            "selection service v1 unavailable; update the loader and selection plugin together",
        );
        return 1;
    };
    if SELECTION.set(selection).is_err() {
        return 1;
    }
    let Some(facets) = defiance_feature_sdk::services::facets() else {
        log(
            LOG_ERROR,
            "Core facets service v1 unavailable; update Core with regroup",
        );
        return 1;
    };
    if FACETS.set(facets).is_err() {
        return 1;
    }
    if let Err(reason) = configure_hotkeys(api) {
        log(LOG_ERROR, &reason);
        return 1;
    }
    let resolved = modules(api).and_then(|(logic, game)| {
        sites::sites(&logic, &game).map(|sites| (logic.base, game.base, sites))
    });
    let (base, game_base, sites) = match resolved {
        Ok(resolved) => resolved,
        Err(e) => {
            log(
                LOG_WARN,
                &format!("regroup: not a supported build ({e}); no writes made"),
            );
            return 1;
        }
    };
    if OFFSETS.set(sites.offsets).is_err()
        || ADDRESSES
            .set(sites.logic.iter().map(|rva| base + rva).collect())
            .is_err()
    {
        return 1;
    }
    // Every hook goes in or none stays: a refusal removes the ones before it.
    let mut installed: Vec<usize> = Vec::with_capacity(10);
    let refused = |installed: &[usize], what: &str| {
        for &target in installed.iter().rev() {
            (api.unhook)(target as *mut _);
        }
        log(
            LOG_WARN,
            &format!("regroup: the loader refused the {what} hook; no hooks left installed"),
        );
        1
    };
    for (index, detour, original) in [
        (
            sites::SPAWN_FALLBACK_CALL,
            keep_requested_species as *const () as usize,
            &SPAWN_FALLBACK,
        ),
        (
            sites::CREATE_CALL,
            create_members as *const () as usize,
            &CREATE,
        ),
        (sites::WIRE_CALL, wire as *const () as usize, &WIRE),
        (
            sites::TEMPLATES_CALL,
            prepare_templates as *const () as usize,
            &TEMPLATES,
        ),
    ] {
        // The loader stores the original into this atomic *before* it publishes
        // the branch, so a detour that fires immediately never sees a zero.
        let target = address(index);
        if (api.hook_call)(target as *mut _, detour as *mut _, original.as_ptr().cast()) != 0 {
            return refused(&installed, "call-site");
        }
        installed.push(target);
    }
    for (index, detour, span, original) in [
        (
            sites::INPUT,
            input_dispatch as *const () as usize,
            INPUT_DISPLACED,
            &INPUT,
        ),
        (
            sites::HOVER,
            update_hover as *const () as usize,
            HOVER_DISPLACED,
            &HOVER_ORIGINAL,
        ),
    ] {
        let target = game_base + sites.game[index];
        if (api.hook_exact)(
            target as *mut _,
            detour as *mut _,
            span,
            original.as_ptr().cast(),
        ) != 0
        {
            return refused(&installed, "game.dll entry");
        }
        installed.push(target);
    }
    for (index, detour, span, original) in [
        (
            sites::PERK_REFRESH,
            refresh_perks as *const () as usize,
            PERK_DISPLACED,
            &PERK_ORIGINAL,
        ),
        (
            sites::EXPORT_ROSTER,
            export_roster as *const () as usize,
            ROSTER_DISPLACED,
            &ROSTER_ORIGINAL,
        ),
        (
            sites::CLEANUP,
            cleanup_empty as *const () as usize,
            CLEANUP_DISPLACED,
            &CLEANUP_ORIGINAL,
        ),
        (
            sites::SQUAD_UPDATE,
            update_squad as *const () as usize,
            UPDATE_DISPLACED,
            &UPDATE_ORIGINAL,
        ),
    ] {
        // As above: the loader fills the atomic before publication.
        let target = address(index);
        if (api.hook_exact)(
            target as *mut _,
            detour as *mut _,
            span,
            original.as_ptr().cast(),
        ) != 0
        {
            return refused(&installed, "logic.dll entry");
        }
        installed.push(target);
    }
    log(
        LOG_INFO,
        "experimental regroup loaded (large squad UI roster fix 11): configured regroup/restore shortcuts; live-world history only; single-player only",
    );
    0
}

#[cfg(test)]
mod tests;
