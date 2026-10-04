//! Optional reduced living-tree sway sampling for verified game builds.
//!
//! The manager and sway functions both receive a time step. The original sway
//! function advances its phase by that time step times its own rate. Half mode
//! omits alternate sway samples and gives the next sample the sum of both
//! manager time steps. It also omits intermediate oscillator events and
//! transform samples, so its long-term behavior remains approximate.

use core::cell::{Cell, RefCell};
use core::ffi::c_void;
use defiance_api::{Api, LOG_INFO, LOG_WARN};
use std::collections::HashMap;
#[cfg(feature = "tree-rate-probe")]
use std::sync::atomic::AtomicU8;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
#[cfg(feature = "tree-rate-probe")]
use std::sync::OnceLock;

type Update = unsafe extern "C" fn(*mut c_void, f32);
type Sway = unsafe extern "C" fn(*mut c_void, f32) -> u8;

#[derive(Clone, Copy)]
struct Site {
    rva: usize,
    entry: &'static [u8],
}

struct Build {
    name: &'static str,
    logic_sha: &'static str,
    manager: Site,
    sway: Site,
    facet: Site,
}

const BUILDS: &[Build] = &[
    Build {
        name: "gog-2025-12-23",
        logic_sha: "17ef48350153306e210e14a24b0ad398c56fb99d3d46c88f246df260ade85780",
        manager: Site {
            rva: 0x46df70,
            entry: &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10],
        },
        sway: Site {
            rva: 0x46b890,
            entry: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        },
        facet: Site {
            rva: 0x46c6b0,
            entry: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        },
    },
    Build {
        name: "gog-2026-09-14",
        logic_sha: "eb8674f1d16595a3e9cf6a9ec0062735b1184976495d8d6ade36f7e2574e8aab",
        manager: Site {
            rva: 0x4807f0,
            entry: &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10],
        },
        sway: Site {
            rva: 0x47e110,
            entry: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        },
        facet: Site {
            rva: 0x47ef30,
            entry: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        },
    },
    Build {
        name: "gog-2026-09-25",
        logic_sha: "1216d627c7288c7db6940168363be582232ed4d3cb860b8b8c8d7489652eca74",
        manager: Site {
            rva: 0x480e30,
            entry: &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10],
        },
        sway: Site {
            rva: 0x47e750,
            entry: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        },
        facet: Site {
            rva: 0x47f570,
            entry: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        },
    },
    Build {
        name: "steam-2025-12-23",
        logic_sha: "d320f848508c45c9f04df235204b5fbc9ffbb1b7f869e4c5a58d80bb2ecc10ed",
        manager: Site {
            rva: 0x46e000,
            entry: &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10],
        },
        sway: Site {
            rva: 0x46b920,
            entry: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        },
        facet: Site {
            rva: 0x46c740,
            entry: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        },
    },
    Build {
        name: "steam-2026-09-22",
        logic_sha: "30264904e1d5199b954bafbd7828cf7190930c246d35fa7b94eefa915e8f0c38",
        manager: Site {
            rva: 0x480880,
            entry: &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10],
        },
        sway: Site {
            rva: 0x47e1a0,
            entry: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        },
        facet: Site {
            rva: 0x47efc0,
            entry: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        },
    },
    Build {
        name: "steam-2026-09-25",
        logic_sha: "adb3ad95926036809b4e554b466bef33d4ac7aa5303e59a9e4a940890bc334b5",
        manager: Site {
            rva: 0x480ec0,
            entry: &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10],
        },
        sway: Site {
            rva: 0x47e7e0,
            entry: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        },
        facet: Site {
            rva: 0x47f600,
            entry: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        },
    },
];

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

fn module(api: &Api) -> Option<(*mut u8, usize, &'static Build)> {
    let selected = crate::RUNTIME.get().and_then(|runtime| {
        BUILDS
            .iter()
            .find(|entry| entry.logic_sha == runtime.logic.sha.as_str())
    });
    let Some(selected) = selected else {
        crate::say(
            api,
            LOG_WARN,
            "tree sway: requested mode has no verified hooks for this build; engine rate retained",
        );
        return None;
    };
    let base = unsafe { (api.module_base)(c"logic.dll".as_ptr()) } as *mut u8;
    if base.is_null() {
        crate::say(api, LOG_WARN, "tree sway: logic.dll is not loaded");
        return None;
    }
    Some((base, unsafe { (api.module_size)(base.cast()) }, selected))
}

fn verified_site(api: &Api, base: *mut u8, size: usize, site: Site) -> bool {
    let valid = site
        .rva
        .checked_add(site.entry.len())
        .is_some_and(|end| end <= size)
        && unsafe { core::slice::from_raw_parts(base.add(site.rva), site.entry.len()) }
            == site.entry;
    if !valid {
        crate::say(
            api,
            LOG_WARN,
            "tree sway: a function entry differs; engine rate retained",
        );
    }
    valid
}

fn request_hook(api: &Api, base: *mut u8, site: Site, detour: *mut c_void) -> Option<usize> {
    let mut original = core::ptr::null_mut();
    if unsafe {
        (api.hook_exact)(
            base.add(site.rva).cast(),
            detour,
            site.entry.len(),
            &mut original,
        )
    } != 0
        || original.is_null()
    {
        crate::say(
            api,
            LOG_WARN,
            "tree sway: hook failed; engine rate retained",
        );
        return None;
    }
    Some(original as usize)
}

pub(super) fn install(api: &Api, mode: &str) {
    if mode != "half" && mode != "half_facet" {
        return;
    }
    let wants_facet = mode == "half_facet" || cfg!(feature = "facet-half-probe");
    let Some((base, size, selected)) = module(api) else {
        return;
    };
    #[cfg(feature = "tree-rate-probe")]
    if selected.name != "steam-2026-09-25" {
        crate::say(api, LOG_WARN, "tree sway rate probe: Steam 2026-09-25 only");
        return;
    }
    let facet_valid = !wants_facet || verified_site(api, base, size, selected.facet);
    if !verified_site(api, base, size, selected.manager)
        || !verified_site(api, base, size, selected.sway)
        || !facet_valid
    {
        return;
    }
    // Install sway first: without the manager hook it forwards every call.
    let Some(sway) = request_hook(api, base, selected.sway, sway_update as *mut c_void) else {
        return;
    };
    SWAY_ORIGINAL.store(sway, Ordering::Release);
    if wants_facet {
        let Some(facet) = request_hook(api, base, selected.facet, facet_update as *mut c_void)
        else {
            return;
        };
        FACET_ORIGINAL.store(facet, Ordering::Release);
    }
    let Some(manager) = request_hook(api, base, selected.manager, manager_update as *mut c_void)
    else {
        return;
    };
    MANAGER_ORIGINAL.store(manager, Ordering::Release);
    FACET_HALF.store(mode == "half_facet", Ordering::Release);
    crate::say(
        api,
        LOG_INFO,
        &format!(
            "tree sway: {} enabled on {}",
            if mode == "half_facet" {
                "half-rate sway and facet refresh"
            } else {
                "half rate"
            },
            selected.name
        ),
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

/// Declare only the hooks actually requested at init, without changing their trampolines.
pub(super) fn contract(api: &Api) {
    let facet_uninstalled = FACET_ORIGINAL.load(Ordering::Acquire) == 0;
    if MANAGER_ORIGINAL.load(Ordering::Acquire) == 0
        && SWAY_ORIGINAL.load(Ordering::Acquire) == 0
        && facet_uninstalled
    {
        return;
    }
    let Some((base, size, selected)) = module(api) else {
        return;
    };
    let facet_valid = facet_uninstalled || verified_site(api, base, size, selected.facet);
    if !verified_site(api, base, size, selected.manager)
        || !verified_site(api, base, size, selected.sway)
        || !facet_valid
    {
        return;
    }
    if SWAY_ORIGINAL.load(Ordering::Acquire) != 0 {
        let _ = request_hook(api, base, selected.sway, sway_update as *mut c_void);
    }
    if FACET_ORIGINAL.load(Ordering::Acquire) != 0 {
        let _ = request_hook(api, base, selected.facet, facet_update as *mut c_void);
    }
    if MANAGER_ORIGINAL.load(Ordering::Acquire) != 0 {
        let _ = request_hook(api, base, selected.manager, manager_update as *mut c_void);
    }
}
