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

/// The most ticks a tree may wait between sway samples.
const MAX_RATE: u8 = 16;

/// A per-tree sampling schedule: every tree nearer than a band's distance
/// samples once every that band's rate of manager ticks; farther trees use
/// `beyond`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Schedule {
    /// (squared distance, rate), distances ascending.
    bands: Vec<(f32, u8)>,
    beyond: u8,
}

impl Schedule {
    /// The rate for a tree at squared distance `distance`; with no camera,
    /// the nearest band's.
    fn rate(&self, distance: Option<f32>) -> u8 {
        let Some(distance) = distance else {
            return self.bands.first().map_or(self.beyond, |band| band.1);
        };
        self.bands
            .iter()
            .find(|band| distance < band.0)
            .map_or(self.beyond, |band| band.1)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Mode {
    Engine,
    Half,
    HalfFacet,
    Rates(Schedule),
}

fn parse_rate(text: &str) -> Result<u8, String> {
    match text.trim().parse::<u8>() {
        Ok(rate) if (1..=MAX_RATE).contains(&rate) => Ok(rate),
        _ => Err(format!(
            "`{}` is not a rate from 1 to {MAX_RATE}",
            text.trim()
        )),
    }
}

/// `engine`, `half`, `half_facet`, a rate `N` (each tree every Nth tick), or
/// bands `D:R, D:R, R`: rate R for trees nearer than D, ascending, then a
/// final rate for the rest.
pub(crate) fn parse(text: &str) -> Result<Mode, String> {
    match text.trim() {
        "" | "engine" => return Ok(Mode::Engine),
        "half" => return Ok(Mode::Half),
        "half_facet" => return Ok(Mode::HalfFacet),
        _ => {}
    }
    let mut bands = Vec::new();
    let mut beyond = None;
    for part in text.split(',') {
        if beyond.is_some() {
            return Err("only the last entry may be a bare rate".into());
        }
        match part.split_once(':') {
            Some((distance, rate)) => {
                let distance = distance
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|d| d.is_finite() && *d > 0.0);
                let distance = distance
                    .ok_or_else(|| format!("`{}` has no positive distance", part.trim()))?;
                if bands
                    .last()
                    .is_some_and(|&(last, _)| last >= distance * distance)
                {
                    return Err("distances must ascend".into());
                }
                bands.push((distance * distance, parse_rate(rate)?));
            }
            None => beyond = Some(parse_rate(part)?),
        }
    }
    let beyond = beyond.ok_or("the last entry must be a bare rate for farther trees")?;
    if bands.is_empty() && beyond == 1 {
        return Ok(Mode::Engine);
    }
    Ok(Mode::Rates(Schedule { bands, beyond }))
}

static SCHEDULE: OnceLock<Schedule> = OnceLock::new();

/// One tree facet's place in its schedule.
struct FacetTick {
    /// The manager tick it was last called on.
    last: u64,
    /// Ticks since its last sample, and their summed time step.
    count: u8,
    elapsed: f32,
    rate: u8,
}

#[cfg(feature = "tree-distance-count")]
#[derive(Default)]
struct Counts {
    /// Per band, then beyond: (samples, skips).
    bands: Vec<(u32, u32)>,
    nearest: f32,
    farthest: f32,
    no_camera: u32,
}

thread_local! {
    /// Manager updates seen on this thread.
    static TICK: Cell<u64> = const { Cell::new(0) };
    static FACET_TICKS: RefCell<HashMap<usize, FacetTick>> = RefCell::new(HashMap::new());
    // diagnostic: per-band sample and skip counts and the distance range, to
    // check that camera and tree positions share a space and the bands fit.
    #[cfg(feature = "tree-distance-count")]
    static COUNTS: RefCell<Counts> = RefCell::new(Counts::default());
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
    if SCHEDULE.get().is_some() {
        let tick = TICK.with(|t| {
            t.set(t.get() + 1);
            t.get()
        });
        unsafe { original(manager, dt) };
        if tick % 1024 == 0 {
            FACET_TICKS.with(|ticks| ticks.borrow_mut().retain(|_, facet| facet.last + 1 >= tick));
        }
        #[cfg(feature = "tree-distance-count")]
        if tick % 1200 == 0 {
            report_counts();
        }
        return;
    }
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
    if let Some(schedule) = SCHEDULE.get() {
        unsafe { scheduled_update(original, schedule, facet, dt) };
        return;
    }
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

/// The tree's position, from the provider at `+0x98` (vtable `+0x58`), as
/// mode zero reads it; [`crate::sites::tree`] checks that code.
unsafe fn position(facet: *mut c_void) -> Option<[f32; 3]> {
    let provider = unsafe { *facet.cast::<u8>().add(0x98).cast::<*mut c_void>() };
    if provider.is_null() {
        return None;
    }
    let vtable = unsafe { *provider.cast::<*const usize>() };
    let get: unsafe extern "C" fn(*mut c_void) -> *const [f32; 3] =
        unsafe { core::mem::transmute(*vtable.add(0x58 / 8)) };
    let position = unsafe { get(provider) };
    (!position.is_null()).then(|| unsafe { *position })
}

/// Where in its first interval a newly seen tree starts, so trees that come
/// into view together sample on different ticks.
fn stagger(facet: usize, rate: u8) -> u8 {
    ((facet as u64 >> 4).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 32) as u8 % rate
}

/// Mode zero under a [`Schedule`]: a tree samples sway and refreshes its
/// transform when due, with the time steps since its last sample; a tree
/// not called on the previous tick (new, or back in view) samples at once.
unsafe fn scheduled_update(original: Update, schedule: &Schedule, facet: *mut c_void, dt: f32) {
    // Mode one completes and releases state; it always reaches the engine.
    if facet.is_null()
        || unsafe { core::ptr::read_unaligned(facet.cast::<u8>().add(0x40).cast::<u32>()) } != 0
    {
        unsafe { original(facet, dt) };
        return;
    }
    let tick = TICK.with(Cell::get);
    let due = FACET_TICKS.with(|ticks| {
        let mut ticks = ticks.borrow_mut();
        let state = ticks.entry(facet as usize).or_insert(FacetTick {
            last: 0,
            count: 0,
            elapsed: 0.0,
            rate: 0,
        });
        let fresh = state.rate == 0 || state.last + 1 != tick;
        state.last = tick;
        if fresh {
            state.count = 0;
            state.elapsed = 0.0;
            state.rate = 1;
        }
        state.count += 1;
        state.elapsed += dt;
        if state.count < state.rate {
            #[cfg(feature = "tree-distance-count")]
            count(schedule, None, false);
            return None;
        }
        let elapsed = core::mem::take(&mut state.elapsed);
        let distance = crate::view_sort::camera()
            .zip(unsafe { position(facet) })
            .map(|(camera, position)| crate::view_sort::distance(&camera, &position));
        #[cfg(feature = "tree-distance-count")]
        count(schedule, Some(distance), true);
        state.rate = schedule.rate(distance);
        state.count = if fresh {
            stagger(facet as usize, state.rate)
        } else {
            0
        };
        Some(elapsed)
    });
    // Mode zero hands its time step to the sway update unchanged.
    if let Some(elapsed) = due {
        unsafe { original(facet, elapsed) };
    }
}

// diagnostic: tallies a sample or skip; `distance` is known only for samples,
// so skips go to the band of the tree's last sample via its rate.
#[cfg(feature = "tree-distance-count")]
fn count(schedule: &Schedule, distance: Option<Option<f32>>, sample: bool) {
    COUNTS.with(|counts| {
        let mut counts = counts.borrow_mut();
        let band_count = schedule.bands.len() + 1;
        counts.bands.resize(band_count, (0, 0));
        match distance {
            Some(Some(distance)) => {
                let band = schedule
                    .bands
                    .iter()
                    .position(|band| distance < band.0)
                    .unwrap_or(band_count - 1);
                counts.bands[band].0 += 1;
                let d = distance.sqrt();
                if counts.nearest == 0.0 || d < counts.nearest {
                    counts.nearest = d;
                }
                counts.farthest = counts.farthest.max(d);
            }
            Some(None) => counts.no_camera += 1,
            None => {}
        }
        if !sample {
            // Skips are tallied together in the beyond slot's second field.
            counts.bands[band_count - 1].1 += 1;
        }
    });
}

#[cfg(feature = "tree-distance-count")]
static COUNT_LOG: OnceLock<unsafe extern "C" fn(u32, *const core::ffi::c_char)> = OnceLock::new();

// diagnostic: logs and clears the tallies every 1200 manager ticks.
#[cfg(feature = "tree-distance-count")]
fn report_counts() {
    let Some(log) = COUNT_LOG.get() else {
        return;
    };
    let text = COUNTS.with(|counts| {
        let counts = core::mem::take(&mut *counts.borrow_mut());
        let samples: Vec<String> = counts.bands.iter().map(|band| band.0.to_string()).collect();
        let skips = counts.bands.last().map_or(0, |band| band.1);
        format!(
            "tree distance count: samples per band [{}], skips {skips}, no camera {}, distance {:.1}..{:.1}",
            samples.join(", "),
            counts.no_camera,
            counts.nearest,
            counts.farthest,
        )
    });
    let text = std::ffi::CString::new(text).unwrap_or_default();
    unsafe { log(LOG_INFO, text.as_ptr()) };
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

/// The hooks a mode installs, in install order. Sway and facet go first:
/// without the manager hook they forward every call. Rate mode needs no sway
/// hook, since the facet hands sway its time step.
fn hooks(base: usize, sites: &Tree, sway: bool, facet: bool) -> Vec<Write> {
    let hook = |rva: usize, entry: &[u8], detour: *mut c_void, original| Write {
        target: base + rva,
        kind: Kind::Entry(Some(entry.len())),
        detour,
        original: Some(original),
    };
    let mut hooks = Vec::new();
    if sway {
        hooks.push(hook(
            sites.sway,
            TREE_ENTRY,
            sway_update as *mut c_void,
            &SWAY_ORIGINAL,
        ));
    }
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

/// The hooks `install` makes for `mode`: none for `engine` or an unreadable
/// setting.
pub(super) fn plan(api: &Api, mode: &str) -> Result<Plan, String> {
    let (sway, facet) = match parse(mode) {
        Ok(Mode::Half) => (true, cfg!(feature = "facet-half-probe")),
        Ok(Mode::HalfFacet) => (true, true),
        Ok(Mode::Rates(_)) => (false, true),
        Ok(Mode::Engine) | Err(_) => return Ok(Plan::default()),
    };
    let (base, sites) = target(api)?;
    Ok(Plan {
        writes: hooks(base, &sites, sway, facet),
    })
}

pub(super) fn install(api: &Api, mode: &str) {
    let parsed = match parse(mode) {
        Ok(parsed) => parsed,
        Err(error) => {
            crate::say(
                api,
                LOG_WARN,
                &format!("tree sway: `{mode}` is not a setting ({error}); engine rate retained"),
            );
            return;
        }
    };
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
    // The schedule is in place before the hooks can read it.
    if let Mode::Rates(schedule) = &parsed {
        let _ = SCHEDULE.set(schedule.clone());
    }
    if let Err(error) = crate::plan::apply(api, &plan) {
        crate::say(
            api,
            LOG_ERROR,
            &format!("tree sway: {error}; engine rate retained, no hooks installed"),
        );
        return;
    }
    FACET_HALF.store(parsed == Mode::HalfFacet, Ordering::Release);
    let message = match &parsed {
        Mode::HalfFacet => "tree sway: half-rate sway and facet refresh enabled".to_string(),
        Mode::Rates(schedule) if !schedule.bands.is_empty() => {
            format!("tree sway: per-tree rates by distance enabled ({mode}); without view_sort = cached every tree uses the nearest rate")
        }
        Mode::Rates(_) => format!("tree sway: each tree samples every {} updates", mode.trim()),
        _ => "tree sway: half rate enabled".to_string(),
    };
    crate::say(api, LOG_INFO, &message);
    #[cfg(feature = "tree-distance-count")]
    {
        let _ = COUNT_LOG.set(api.log);
        crate::say(
            api,
            LOG_INFO,
            "tree distance count: logging every 1200 updates",
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rates(bands: &[(f32, u8)], beyond: u8) -> Mode {
        Mode::Rates(Schedule {
            bands: bands.iter().map(|&(d, r)| (d * d, r)).collect(),
            beyond,
        })
    }

    #[test]
    fn settings_parse() {
        assert_eq!(parse("engine"), Ok(Mode::Engine));
        assert_eq!(parse(""), Ok(Mode::Engine));
        assert_eq!(parse("1"), Ok(Mode::Engine));
        assert_eq!(parse("half"), Ok(Mode::Half));
        assert_eq!(parse("half_facet"), Ok(Mode::HalfFacet));
        assert_eq!(parse(" 3 "), Ok(rates(&[], 3)));
        assert_eq!(
            parse("100:1, 200:2,400.5:4, 8"),
            Ok(rates(&[(100.0, 1), (200.0, 2), (400.5, 4)], 8))
        );
    }

    #[test]
    fn bad_settings_are_refused() {
        for text in [
            "0",
            "17",
            "fast",
            "100:2",
            "100:2, 50:3, 4",
            "4, 100:2",
            "-5:2, 4",
            "100:0, 4",
            "1, 2",
            "100:2,,4",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn rates_follow_distance() {
        let Ok(Mode::Rates(schedule)) = parse("100:1, 200:2, 6") else {
            panic!();
        };
        assert_eq!(schedule.rate(Some(0.0)), 1);
        assert_eq!(schedule.rate(Some(99.0 * 99.0)), 1);
        assert_eq!(schedule.rate(Some(100.0 * 100.0)), 2);
        assert_eq!(schedule.rate(Some(250.0 * 250.0)), 6);
        assert_eq!(schedule.rate(None), 1);
    }

    #[test]
    fn stagger_spreads_trees() {
        let phases: std::collections::HashSet<u8> = (0..64usize)
            .map(|i| stagger(0x2000_0000 + i * 0x60, 4))
            .collect();
        assert_eq!(phases.len(), 4);
    }
}
