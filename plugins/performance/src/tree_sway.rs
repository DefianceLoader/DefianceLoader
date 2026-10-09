//! Optional reduced living-tree sway sampling.
//!
//! The hooked functions are found by signature ([`crate::sites::tree`]); a build
//! where any of them does not resolve keeps the engine rate and is not patched.
//! The manager and sway functions both receive a time step. The original sway
//! function advances its phase by that time step times its own rate. Half mode
//! omits alternate sway samples and gives the next sample the sum of both
//! manager time steps. It also omits intermediate oscillator events and
//! transform samples, so its long-term behavior remains approximate.

use core::cell::{Cell, RefCell};
use core::ffi::c_void;
use defiance_api::{Api, LOG_ERROR, LOG_INFO, LOG_WARN};
use defiance_core::sites::Image;
use std::collections::HashMap;
#[cfg(feature = "tree-rate-probe")]
use std::sync::atomic::AtomicU8;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
#[cfg(feature = "tree-rate-probe")]
use std::sync::OnceLock;

use crate::plan::{Kind, Plan, Write};
use crate::sites::{Tree, TREE_ENTRY, TREE_MANAGER_ENTRY};

type Update = unsafe extern "C" fn(*mut c_void, f32);
type Sway = unsafe extern "C" fn(*mut c_void, f32) -> u8;

static MANAGER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static SWAY_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static FACET_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
struct ManagerTick {
    generation: usize,
    rate: u8,
    phase: u8,
    elapsed: f32,
}

#[cfg(feature = "tree-rate-probe")]
type Log = unsafe extern "C" fn(u32, *const core::ffi::c_char);
#[cfg(feature = "tree-rate-probe")]
static RATE_LOG: OnceLock<Log> = OnceLock::new();
#[cfg(feature = "tree-rate-probe")]
static RATE: AtomicU8 = AtomicU8::new(2);
#[cfg(feature = "tree-rate-probe")]
static RATE_GENERATION: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "tree-rate-probe")]
static RATE_KEY_HELD: AtomicBool = AtomicBool::new(false);
static FACET_HALF: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "facet-half-probe")]
static FACET_KEY_HELD: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "tree-rate-probe")]
pub(crate) fn diagnostic_rate() -> &'static str {
    if RATE.load(Ordering::Acquire) == 2 {
        "half"
    } else {
        "third"
    }
}

#[cfg(feature = "facet-half-probe")]
pub(crate) fn diagnostic_facet() -> &'static str {
    if FACET_HALF.load(Ordering::Acquire) {
        "on"
    } else {
        "off"
    }
}

#[cfg(feature = "tree-rate-probe")]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetAsyncKeyState(key: i32) -> i16;
}

thread_local! {
    // Managers may survive several missions or reuse addresses. A stale entry
    // only affects the first sampling phase and elapsed step; it is never dereferenced.
    static MANAGER_TICKS: RefCell<HashMap<usize, ManagerTick>> = RefCell::new(HashMap::new());
    // None: outside the manager; Some(None): omitted tick; Some(Some(dt)): due tick.
    static CURRENT_STEP: Cell<Option<Option<f32>>> = const { Cell::new(None) };
}

#[cfg(feature = "tree-rate-probe")]
fn poll_rate() {
    const VK_SHIFT: i32 = 0x10;
    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12;
    const VK_J: i32 = 0x4a;
    let held = |key| unsafe { GetAsyncKeyState(key) as u16 & 0x8000 != 0 };
    let key_state = unsafe { GetAsyncKeyState(VK_J) as u16 };
    let pressed = key_state & 0x8000 != 0;
    let was_pressed = RATE_KEY_HELD.swap(pressed, Ordering::Relaxed);
    if (was_pressed || (!pressed && key_state & 1 == 0))
        || !held(VK_CONTROL)
        || !held(VK_MENU)
        || !held(VK_SHIFT)
    {
        return;
    }
    let rate = if RATE.load(Ordering::Acquire) == 2 {
        3
    } else {
        2
    };
    RATE.store(rate, Ordering::Release);
    RATE_GENERATION.fetch_add(1, Ordering::AcqRel);
    #[cfg(feature = "facet-half-probe")]
    if rate != 2 && FACET_HALF.swap(false, Ordering::AcqRel) {
        if let Some(log) = RATE_LOG.get() {
            unsafe {
                log(
                    LOG_INFO,
                    c"tree facet half probe: OFF (tree rate changed)".as_ptr(),
                )
            };
        }
    }
    crate::render_profile::reset_cadence();
    if let Some(log) = RATE_LOG.get() {
        let message = if rate == 2 {
            c"tree sway rate probe: half"
        } else {
            c"tree sway rate probe: third"
        };
        unsafe { log(LOG_INFO, message.as_ptr()) };
    }
}

#[cfg(feature = "facet-half-probe")]
fn poll_facet() {
    const VK_SHIFT: i32 = 0x10;
    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12;
    const VK_L: i32 = 0x4c;
    let held = |key| unsafe { GetAsyncKeyState(key) as u16 & 0x8000 != 0 };
    let key_state = unsafe { GetAsyncKeyState(VK_L) as u16 };
    let pressed = key_state & 0x8000 != 0;
    let was_pressed = FACET_KEY_HELD.swap(pressed, Ordering::Relaxed);
    if (was_pressed || (!pressed && key_state & 1 == 0))
        || !held(VK_CONTROL)
        || !held(VK_MENU)
        || !held(VK_SHIFT)
    {
        return;
    }
    if RATE.load(Ordering::Acquire) != 2 {
        if let Some(log) = RATE_LOG.get() {
            unsafe {
                log(
                    LOG_INFO,
                    c"tree facet half probe: requires half tree rate".as_ptr(),
                )
            };
        }
        return;
    }
    let enabled = !FACET_HALF.load(Ordering::Acquire);
    FACET_HALF.store(enabled, Ordering::Release);
    crate::render_profile::reset_cadence();
    if let Some(log) = RATE_LOG.get() {
        let message = if enabled {
            c"tree facet half probe: ON"
        } else {
            c"tree facet half probe: OFF"
        };
        unsafe { log(LOG_INFO, message.as_ptr()) };
    }
}

unsafe extern "C" fn manager_update(manager: *mut c_void, dt: f32) {
    let original = MANAGER_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return;
    }
    let original: Update = unsafe { core::mem::transmute(original) };
    #[cfg(feature = "tree-rate-probe")]
    poll_rate();
    #[cfg(feature = "facet-half-probe")]
    poll_facet();
    if SWAY_ORIGINAL.load(Ordering::Acquire) == 0 || manager.is_null() {
        unsafe { original(manager, dt) };
        return;
    }
    #[cfg(feature = "tree-rate-probe")]
    let rate = RATE.load(Ordering::Acquire);
    #[cfg(feature = "tree-rate-probe")]
    let generation = RATE_GENERATION.load(Ordering::Acquire);
    #[cfg(not(feature = "tree-rate-probe"))]
    let rate = 2;
    #[cfg(not(feature = "tree-rate-probe"))]
    let generation = 0;
    let step = MANAGER_TICKS.with(|state| {
        let mut state = state.borrow_mut();
        if state.len() > 32 {
            state.clear();
        }
        let tick = state.entry(manager as usize).or_default();
        if tick.rate != rate || tick.generation != generation {
            *tick = ManagerTick {
                generation,
                rate,
                ..ManagerTick::default()
            };
        }
        tick.phase += 1;
        tick.elapsed += dt;
        if tick.phase >= rate {
            tick.phase = 0;
            Some(core::mem::take(&mut tick.elapsed))
        } else {
            None
        }
    });
    let prior = CURRENT_STEP.with(|state| state.replace(Some(step)));
    unsafe { original(manager, dt) };
    CURRENT_STEP.with(|state| state.set(prior));
}

unsafe extern "C" fn sway_update(animation: *mut c_void, dt: f32) -> u8 {
    let original = SWAY_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return 0;
    }
    let original: Sway = unsafe { core::mem::transmute(original) };
    match CURRENT_STEP.with(Cell::get) {
        Some(None) => 0,
        Some(Some(elapsed)) => unsafe { original(animation, elapsed) },
        None => unsafe { original(animation, dt) },
    }
}

/// Omits only mode-zero visual/provider work on the manager's skipped sway tick.
unsafe extern "C" fn facet_update(facet: *mut c_void, dt: f32) {
    let original = FACET_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return;
    }
    let original: Update = unsafe { core::mem::transmute(original) };
    let omitted_tick = CURRENT_STEP.with(Cell::get) == Some(None);
    #[cfg(feature = "tree-rate-probe")]
    let half_rate = RATE.load(Ordering::Acquire) == 2;
    #[cfg(not(feature = "tree-rate-probe"))]
    let half_rate = true;
    if FACET_HALF.load(Ordering::Acquire) && half_rate && omitted_tick && !facet.is_null() {
        // Mode one completes and releases state; it always reaches the engine.
        let mode = unsafe { core::ptr::read_unaligned(facet.cast::<u8>().add(0x40).cast::<u32>()) };
        if mode == 0 {
            return;
        }
    }
    unsafe { original(facet, dt) };
}

/// The loaded `logic.dll`'s base and its tree-sway functions, read from the
/// original view, or why this build is unsupported.
fn target(api: &Api) -> Result<(usize, Tree), String> {
    let base = unsafe { (api.module_base)(c"logic.dll".as_ptr()) } as *const u8;
    if base.is_null() {
        return Err("logic.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base.cast_mut().cast()) };
    let image = crate::plan::original_image(base, size).ok_or("logic.dll is unreadable")?;
    let image = Image {
        image: &image,
        base: base as usize,
    };
    Ok((base as usize, crate::sites::tree(&image)?))
}

/// The hooks a mode installs, in install order. Sway goes first: without the
/// manager hook it forwards every call.
fn hooks(base: usize, sites: &Tree, facet: bool) -> Vec<Write> {
    let hook = |rva: usize, entry: &[u8], detour: *mut c_void, original| Write {
        target: base + rva,
        kind: Kind::Entry(Some(entry.len())),
        detour,
        original: Some(original),
    };
    let mut hooks = vec![hook(
        sites.sway,
        TREE_ENTRY,
        sway_update as *mut c_void,
        &SWAY_ORIGINAL,
    )];
    if facet {
        hooks.push(hook(
            sites.facet,
            TREE_ENTRY,
            facet_update as *mut c_void,
            &FACET_ORIGINAL,
        ));
    }
    hooks.push(hook(
        sites.manager,
        TREE_MANAGER_ENTRY,
        manager_update as *mut c_void,
        &MANAGER_ORIGINAL,
    ));
    hooks
}

/// The hooks `install` makes for `mode`: none unless it is `half` or
/// `half_facet`.
pub(super) fn plan(api: &Api, mode: &str) -> Result<Plan, String> {
    if mode != "half" && mode != "half_facet" {
        return Ok(Plan::default());
    }
    let wants_facet = mode == "half_facet" || cfg!(feature = "facet-half-probe");
    let (base, sites) = target(api)?;
    Ok(Plan {
        writes: hooks(base, &sites, wants_facet),
    })
}

pub(super) fn install(api: &Api, mode: &str) {
    // Every function resolves and matches before the first hook.
    let plan = match plan(api, mode) {
        Ok(plan) if plan.writes.is_empty() => return,
        Ok(plan) => plan,
        Err(error) => {
            crate::say(
                api,
                LOG_WARN,
                &format!("tree sway: not a supported build ({error}); engine rate retained, no writes made"),
            );
            return;
        }
    };
    if let Err(error) = crate::plan::apply(api, &plan) {
        crate::say(
            api,
            LOG_ERROR,
            &format!("tree sway: {error}; engine rate retained, no hooks installed"),
        );
        return;
    }
    FACET_HALF.store(mode == "half_facet", Ordering::Release);
    crate::say(
        api,
        LOG_INFO,
        if mode == "half_facet" {
            "tree sway: half-rate sway and facet refresh enabled"
        } else {
            "tree sway: half rate enabled"
        },
    );
    #[cfg(feature = "tree-rate-probe")]
    {
        let _ = RATE_LOG.set(api.log);
        crate::say(
            api,
            LOG_INFO,
            "tree sway rate probe: half at startup; Ctrl+Alt+Shift+J toggles half and third",
        );
    }
    #[cfg(feature = "facet-half-probe")]
    crate::say(
        api,
        LOG_INFO,
        "tree facet half probe: OFF at startup; Ctrl+Alt+Shift+L toggles outer work",
    );
}
