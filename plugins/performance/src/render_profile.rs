//! Temporary main-view and shadow timing probe (feature `render-profile`).
//!
//! The probe finds its `world2.dll` sites by signature
//! ([`crate::sites::render`]) and hooks all of them or none. It records paired
//! main-view/shadow samples. Formatting runs after a measured pass, only when
//! a report window fills.

use core::ffi::{c_char, c_void};
#[cfg(feature = "render-profile-toggle")]
use core::sync::atomic::{AtomicBool, AtomicU8};
use core::sync::atomic::{AtomicUsize, Ordering};
#[cfg(feature = "shadow-cull-profile")]
use std::cell::Cell;
use std::cell::RefCell;
use std::sync::OnceLock;
#[cfg(feature = "render-profile-toggle")]
use std::time::Duration;
use std::time::Instant;

use defiance_api::{Api, LOG_ERROR, LOG_INFO, LOG_WARN};
use defiance_core::sites::Image;

use crate::sites::Render;

const REPORT_SAMPLES: usize = 240;
const MAX_GROUPS: usize = 64;
#[cfg(feature = "render-profile-toggle")]
const MODE_OFF: u8 = 0;
#[cfg(feature = "render-profile-toggle")]
const MODE_CPU: u8 = 1;
#[cfg(feature = "render-profile-toggle")]
const MODE_GPU: u8 = 2;

type Log = unsafe extern "C" fn(level: u32, message: *const c_char);
type MainFn = unsafe extern "C" fn(*mut c_void) -> usize;
type BatchFn = unsafe extern "C" fn(usize, *mut c_void, *mut c_void, *mut c_void) -> usize;
#[cfg(feature = "shadow-cull-profile")]
type CullFn = unsafe extern "C" fn(usize, usize, usize, usize, usize, usize) -> usize;
#[cfg(not(feature = "render-profile-light"))]
type DirtyFn = unsafe extern "C" fn(*mut c_void) -> usize;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
}
#[cfg(feature = "render-profile-toggle")]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetAsyncKeyState(key: i32) -> i16;
}

static LOG: OnceLock<Log> = OnceLock::new();
/// The `world2.dll` base and sites, set once every hook is installed.
static HOOKED: OnceLock<(usize, Render)> = OnceLock::new();
static MAIN_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static BATCH_ORIGINAL: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];
#[cfg(feature = "shadow-cull-profile")]
static CULL_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
#[cfg(not(feature = "render-profile-light"))]
static DIRTY_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "render-profile-toggle")]
static PROFILE_MODE: AtomicU8 = AtomicU8::new(MODE_OFF);
#[cfg(feature = "render-profile-toggle")]
static KEY_HELD: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "tree-rate-probe")]
static CADENCE_RESET: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Default)]
struct Frame {
    view: usize,
    thread_id: u32,
    main_ms: f64,
    batch_ms: [f64; 2],
    #[cfg(not(feature = "render-profile-light"))]
    dirty_ms: f64,
    #[cfg(not(feature = "render-profile-light"))]
    dirty_calls: u32,
    #[cfg(not(feature = "render-profile-light"))]
    dirty_roots: u32,
    shadow_ms: f64,
    #[cfg(feature = "shadow-cull-profile")]
    cull_ms: f64,
    #[cfg(feature = "shadow-cull-profile")]
    cull_calls: u32,
    #[cfg(feature = "shadow-cull-profile")]
    cull_candidates: u32,
    effective_mask: u32,
}

#[derive(Default)]
struct ThreadProfile {
    frame: Option<Frame>,
    completed: Option<Frame>,
    #[cfg(not(feature = "render-profile-light"))]
    batch_depth: u32,
    #[cfg(not(feature = "render-profile-light"))]
    dirty_depth: u32,
    groups: Vec<Group>,
    #[cfg(feature = "render-profile-toggle")]
    cadence: Vec<Cadence>,
    #[cfg(feature = "tree-rate-probe")]
    cadence_generation: usize,
}

#[cfg(feature = "render-profile-toggle")]
struct Cadence {
    view: usize,
    tree_rate: &'static str,
    facet_mode: &'static str,
    started: Instant,
    previous: Option<Instant>,
    calls: u32,
    intervals_ms: Vec<f64>,
}

thread_local! {
    static THREAD: RefCell<ThreadProfile> = RefCell::new(ThreadProfile::default());
    #[cfg(feature = "shadow-cull-profile")]
    static CULL_ACTIVE_VIEW: Cell<usize> = const { Cell::new(0) };
}

#[derive(Clone, Copy)]
struct Sample {
    frame: Frame,
}

struct Group {
    view: usize,
    thread_id: u32,
    mask: Option<u32>,
    samples: Vec<Sample>,
}

struct Report {
    view: usize,
    thread_id: u32,
    mask: Option<u32>,
    samples: Vec<Sample>,
}

/// The loaded `world2.dll` and its sites, or why this build is unsupported.
fn target(api: &Api) -> Result<(*mut u8, Render), String> {
    let base = unsafe { (api.module_base)(c"world2.dll".as_ptr()) } as *mut u8;
    if base.is_null() {
        return Err("world2.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base.cast()) };
    let image = crate::plan::original_image(base, size).ok_or("world2.dll is unreadable")?;
    let image = Image {
        image: &image,
        base: base as usize,
    };
    Ok((base, crate::sites::render(&image)?))
}

/// How a hook attaches: by redirecting one call, or at a function entry.
#[derive(Clone, Copy)]
#[cfg_attr(
    all(feature = "render-profile-light", not(feature = "shadow-cull-profile")),
    allow(dead_code)
)]
enum Kind {
    Call,
    Entry,
}

/// Every hook this feature set installs, in install order.
fn hooks(sites: &Render) -> Vec<(Kind, usize, *mut c_void, &'static AtomicUsize)> {
    #[allow(unused_mut)]
    let mut hooks = vec![
        (
            Kind::Call,
            sites.main_call,
            main_view as *mut c_void,
            &MAIN_ORIGINAL,
        ),
        (
            Kind::Call,
            sites.batch_calls[0],
            batch_0 as *mut c_void,
            &BATCH_ORIGINAL[0],
        ),
        (
            Kind::Call,
            sites.batch_calls[1],
            batch_1 as *mut c_void,
            &BATCH_ORIGINAL[1],
        ),
    ];
    #[cfg(feature = "shadow-cull-profile")]
    hooks.push((
        Kind::Entry,
        sites.cull_fn,
        shadow_cull as *mut c_void,
        &CULL_ORIGINAL,
    ));
    #[cfg(not(feature = "render-profile-light"))]
    hooks.push((
        Kind::Entry,
        sites.dirty_fn,
        dirty_transform as *mut c_void,
        &DIRTY_ORIGINAL,
    ));
    hooks
}

/// Every hook as a plan in `world2.dll`, or why this build is unsupported.
pub(super) fn plan(api: &Api) -> Result<crate::plan::Plan, String> {
    let (base, sites) = target(api)?;
    Ok(writes(base, &sites))
}

fn writes(base: *mut u8, sites: &Render) -> crate::plan::Plan {
    let writes = hooks(sites)
        .into_iter()
        .map(|(kind, rva, detour, original)| crate::plan::Write {
            target: base as usize + rva,
            kind: match kind {
                Kind::Call => crate::plan::Kind::Call,
                Kind::Entry => crate::plan::Kind::Entry(Some(5)),
            },
            detour,
            original: Some(original),
        })
        .collect();
    crate::plan::Plan { writes }
}

fn emit(level: u32, message: &str) {
    let Some(log) = LOG.get() else {
        return;
    };
    let message = std::ffi::CString::new(message).unwrap_or_default();
    unsafe { log(level, message.as_ptr()) };
}

#[inline]
fn enabled() -> bool {
    #[cfg(feature = "render-profile-toggle")]
    {
        PROFILE_MODE.load(Ordering::Acquire) != MODE_OFF
    }
    #[cfg(not(feature = "render-profile-toggle"))]
    {
        true
    }
}

#[cfg(feature = "render-profile-toggle")]
pub(super) fn gpu_enabled() -> bool {
    PROFILE_MODE.load(Ordering::Acquire) == MODE_GPU
}

/// A mode switch starts a new reporting window before the next main-view call.
#[cfg(feature = "render-profile-toggle")]
fn poll_toggle() {
    const VK_SHIFT: i32 = 0x10;
    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12;
    const VK_K: i32 = 0x4b;
    let held = |key| unsafe { GetAsyncKeyState(key) as u16 & 0x8000 != 0 };
    let key_state = unsafe { GetAsyncKeyState(VK_K) as u16 };
    let pressed = key_state & 0x8000 != 0;
    let was_pressed = KEY_HELD.swap(pressed, Ordering::Relaxed);
    let new_press = !was_pressed && (pressed || key_state & 1 != 0);
    if !new_press || !held(VK_CONTROL) || !held(VK_MENU) || !held(VK_SHIFT) {
        return;
    }
    let next = match PROFILE_MODE.load(Ordering::Acquire) {
        MODE_OFF => MODE_CPU,
        MODE_CPU => MODE_GPU,
        _ => MODE_OFF,
    };
    PROFILE_MODE.store(next, Ordering::Release);
    THREAD.with(|profile| {
        if let Ok(mut profile) = profile.try_borrow_mut() {
            profile.frame = None;
            profile.completed = None;
            profile.groups.clear();
            profile.cadence.clear();
        }
    });
    crate::shadow::reset_profile_timing();
    let name = match next {
        MODE_CPU => "CPU timings",
        MODE_GPU => "CPU and GPU timings",
        _ => "OFF",
    };
    emit(LOG_INFO, &format!("render profile toggle: {name}"));
}

/// View-call cadence gives a low-cost average across FPS fluctuations.
#[cfg(feature = "render-profile-toggle")]
fn record_cadence(view: usize) {
    let now = Instant::now();
    let report = THREAD.with(|profile| {
        let Ok(mut profile) = profile.try_borrow_mut() else {
            return None;
        };
        #[cfg(feature = "tree-rate-probe")]
        {
            let generation = CADENCE_RESET.load(Ordering::Acquire);
            if profile.cadence_generation != generation {
                profile.cadence.clear();
                profile.cadence_generation = generation;
            }
        }
        let index = profile.cadence.iter().position(|entry| entry.view == view);
        let entry = if let Some(index) = index {
            &mut profile.cadence[index]
        } else {
            #[cfg(feature = "tree-rate-probe")]
            let tree_rate = crate::tree_sway::diagnostic_rate();
            #[cfg(not(feature = "tree-rate-probe"))]
            let tree_rate = "";
            #[cfg(feature = "facet-half-probe")]
            let facet_mode = crate::tree_sway::diagnostic_facet();
            #[cfg(not(feature = "facet-half-probe"))]
            let facet_mode = "";
            profile.cadence.push(Cadence {
                view,
                tree_rate,
                facet_mode,
                started: now,
                previous: None,
                calls: 0,
                intervals_ms: Vec::with_capacity(4096),
            });
            profile.cadence.last_mut()?
        };
        if let Some(previous) = entry.previous.replace(now) {
            entry
                .intervals_ms
                .push(now.duration_since(previous).as_secs_f64() * 1000.0);
        }
        entry.calls += 1;
        let elapsed = now.duration_since(entry.started);
        if elapsed < Duration::from_secs(30) {
            return None;
        }
        let calls = entry.calls;
        entry.calls = 0;
        entry.started = now;
        let intervals = core::mem::replace(&mut entry.intervals_ms, Vec::with_capacity(4096));
        Some((
            calls,
            elapsed.as_secs_f64(),
            intervals,
            entry.tree_rate,
            entry.facet_mode,
        ))
    });
    if let Some((calls, seconds, mut intervals, rate, facet)) = report {
        let mode = match PROFILE_MODE.load(Ordering::Acquire) {
            MODE_CPU => "cpu",
            MODE_GPU => "cpu_gpu",
            _ => "off",
        };
        let tree_rate = if rate.is_empty() {
            String::new()
        } else {
            format!(" tree_rate={rate}")
        };
        let facet_mode = if facet.is_empty() {
            String::new()
        } else {
            format!(" facet_half={facet}")
        };
        intervals.sort_by(f64::total_cmp);
        let count = intervals.len();
        let interval = |percent: usize| {
            let index = (count * percent).div_ceil(100).saturating_sub(1);
            intervals.get(index).copied().unwrap_or_default()
        };
        emit(
            LOG_INFO,
            &format!(
                "render profile cadence: mode={mode}{tree_rate}{facet_mode} view={view:#x} calls={calls} seconds={seconds:.2} calls_per_second={:.2} view_interval_ms p50/p95/p99={:.2}/{:.2}/{:.2} n={count}",
                calls as f64 / seconds,
                interval(50),
                interval(95),
                interval(99)
            ),
        );
    }
}

/// Starts a fresh cadence window when the temporary tree-rate mode changes.
#[cfg(feature = "tree-rate-probe")]
pub(crate) fn reset_cadence() {
    CADENCE_RESET.fetch_add(1, Ordering::AcqRel);
}

fn percentiles(mut values: Vec<f64>) -> (f64, f64) {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    (
        values[n / 2],
        values[(n * 95).div_ceil(100).saturating_sub(1)],
    )
}

fn report(report: Report) -> String {
    let Report {
        view,
        thread_id,
        mask,
        samples,
    } = report;
    let metric = |get: fn(&Frame) -> f64| {
        percentiles(samples.iter().map(|sample| get(&sample.frame)).collect())
    };
    let (main50, main95) = metric(|frame| frame.main_ms);
    let (batch0_50, batch0_95) = metric(|frame| frame.batch_ms[0]);
    let (batch1_50, batch1_95) = metric(|frame| frame.batch_ms[1]);
    let (batch50, batch95) = metric(|frame| frame.batch_ms[0] + frame.batch_ms[1]);
    let (shadow50, shadow95) = metric(|frame| frame.shadow_ms);
    #[cfg(not(feature = "render-profile-light"))]
    let (dirty50, dirty95) = metric(|frame| frame.dirty_ms);
    #[cfg(not(feature = "render-profile-light"))]
    let (dirty_calls50, dirty_calls95) = percentiles(
        samples
            .iter()
            .map(|sample| sample.frame.dirty_calls as f64)
            .collect(),
    );
    #[cfg(not(feature = "render-profile-light"))]
    let (dirty_roots50, dirty_roots95) = percentiles(
        samples
            .iter()
            .map(|sample| sample.frame.dirty_roots as f64)
            .collect(),
    );
    #[cfg(not(feature = "render-profile-light"))]
    let dirty = format!(
        "dirty_ms={dirty50:.3}/{dirty95:.3} dirty_calls={dirty_calls50:.0}/{dirty_calls95:.0} dirty_roots={dirty_roots50:.0}/{dirty_roots95:.0}"
    );
    #[cfg(feature = "render-profile-light")]
    let dirty = "dirty=unmeasured";
    #[cfg(feature = "shadow-cull-profile")]
    let cull = if HOOKED.get().is_some() {
        let (cull50, cull95) = metric(|frame| frame.cull_ms);
        let (calls50, calls95) = percentiles(
            samples
                .iter()
                .map(|sample| sample.frame.cull_calls as f64)
                .collect(),
        );
        let (candidates50, candidates95) = percentiles(
            samples
                .iter()
                .map(|sample| sample.frame.cull_candidates as f64)
                .collect(),
        );
        format!(
            " cull_ms={cull50:.3}/{cull95:.3} cull_calls={calls50:.0}/{calls95:.0} cull_candidates={candidates50:.0}/{candidates95:.0}"
        )
    } else {
        " cull=unavailable".to_string()
    };
    #[cfg(not(feature = "shadow-cull-profile"))]
    let cull = "";
    let label = mask.map_or_else(|| "combined".to_string(), |mask| format!("{mask:#x}"));
    format!(
        "render profile: thread_id={thread_id} view={view:#x} mask={label} n={} main_ms p50/p95={main50:.3}/{main95:.3} batch_ms={batch50:.3}/{batch95:.3} batch0_ms={batch0_50:.3}/{batch0_95:.3} batch1_ms={batch1_50:.3}/{batch1_95:.3} {dirty}{cull} shadow_wall_ms={shadow50:.3}/{shadow95:.3}",
        samples.len()
    )
}

fn record(frame: Frame) {
    let reports = THREAD.try_with(|profile| {
        let Ok(mut profile) = profile.try_borrow_mut() else {
            return Vec::new();
        };
        let groups = &mut profile.groups;
        let mut reports = Vec::new();
        for mask in [None, Some(frame.effective_mask)] {
            let index = groups.iter().position(|group| {
                group.view == frame.view && group.thread_id == frame.thread_id && group.mask == mask
            });
            let index = index.or_else(|| {
                if groups.len() >= MAX_GROUPS {
                    return None;
                }
                groups.push(Group {
                    view: frame.view,
                    thread_id: frame.thread_id,
                    mask,
                    samples: Vec::with_capacity(REPORT_SAMPLES),
                });
                Some(groups.len() - 1)
            });
            let Some(index) = index else {
                continue;
            };
            let group = &mut groups[index];
            group.samples.push(Sample { frame });
            let required = if group.mask.is_some() {
                REPORT_SAMPLES / 2
            } else {
                REPORT_SAMPLES
            };
            if group.samples.len() == required {
                let samples = core::mem::replace(&mut group.samples, Vec::with_capacity(required));
                reports.push(Report {
                    view: group.view,
                    thread_id: group.thread_id,
                    mask: group.mask,
                    samples,
                });
            }
        }
        reports
    });
    for completed in reports.unwrap_or_default() {
        emit(LOG_INFO, &report(completed));
    }
}

unsafe extern "C" fn main_view(view: *mut c_void) -> usize {
    let original = MAIN_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return 0;
    }
    let original: MainFn = unsafe { core::mem::transmute(original) };
    #[cfg(feature = "render-profile-toggle")]
    {
        poll_toggle();
        record_cadence(view as usize);
    }
    if !enabled() {
        return unsafe { original(view) };
    }
    THREAD.with(|profile| {
        if let Ok(mut profile) = profile.try_borrow_mut() {
            profile.frame = Some(Frame {
                view: view as usize,
                thread_id: unsafe { GetCurrentThreadId() },
                ..Frame::default()
            });
            #[cfg(not(feature = "render-profile-light"))]
            {
                profile.batch_depth = 0;
                profile.dirty_depth = 0;
            }
        }
    });
    let started = Instant::now();
    let result = unsafe { original(view) };
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    THREAD.with(|profile| {
        if let Ok(mut profile) = profile.try_borrow_mut() {
            if let Some(frame) = profile
                .frame
                .as_mut()
                .filter(|frame| frame.view == view as usize)
            {
                frame.main_ms = elapsed;
            }
        }
    });
    result
}

unsafe extern "C" fn batch_0(
    first: usize,
    list: *mut c_void,
    out: *mut c_void,
    predicate: *mut c_void,
) -> usize {
    batch(0, first, list, out, predicate)
}

unsafe extern "C" fn batch_1(
    first: usize,
    list: *mut c_void,
    out: *mut c_void,
    predicate: *mut c_void,
) -> usize {
    batch(1, first, list, out, predicate)
}

unsafe fn batch(
    index: usize,
    first: usize,
    list: *mut c_void,
    out: *mut c_void,
    predicate: *mut c_void,
) -> usize {
    let original = BATCH_ORIGINAL[index].load(Ordering::Acquire);
    if original == 0 {
        return 0;
    }
    let original: BatchFn = unsafe { core::mem::transmute(original) };
    if !enabled() {
        return unsafe { original(first, list, out, predicate) };
    }
    #[cfg(not(feature = "render-profile-light"))]
    THREAD.with(|profile| {
        if let Ok(mut profile) = profile.try_borrow_mut() {
            profile.batch_depth = profile.batch_depth.saturating_add(1);
        }
    });
    let started = Instant::now();
    let result = unsafe { original(first, list, out, predicate) };
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    THREAD.with(|profile| {
        if let Ok(mut profile) = profile.try_borrow_mut() {
            #[cfg(not(feature = "render-profile-light"))]
            {
                profile.batch_depth = profile.batch_depth.saturating_sub(1);
            }
            if let Some(frame) = profile.frame.as_mut() {
                frame.batch_ms[index] += elapsed;
            }
        }
    });
    result
}

/// Times calls to the verified VisibleMgr query within the active shadow pass.
#[cfg(feature = "shadow-cull-profile")]
unsafe extern "C" fn shadow_cull(
    manager: usize,
    frustum: usize,
    cascade: usize,
    camera: usize,
    visitor: usize,
    output: usize,
) -> usize {
    let original = CULL_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return 0;
    }
    let original: CullFn = unsafe { core::mem::transmute(original) };
    let view = CULL_ACTIVE_VIEW.with(Cell::get);
    let started = (view != 0).then(Instant::now);
    let result = unsafe { original(manager, frustum, cascade, camera, visitor, output) };
    if let Some(started) = started {
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        // The resolved caller consumes output as an 8-byte pointer vector.
        let candidates = if output == 0 {
            0
        } else {
            let begin = unsafe { core::ptr::read_unaligned(output as *const usize) };
            let end = unsafe { core::ptr::read_unaligned((output + 8) as *const usize) };
            if end >= begin && (end - begin) % 8 == 0 {
                u32::try_from((end - begin) / 8).unwrap_or(u32::MAX)
            } else {
                0
            }
        };
        let _ = THREAD.try_with(|profile| {
            if let Ok(mut profile) = profile.try_borrow_mut() {
                if let Some(frame) = profile.frame.as_mut().filter(|frame| frame.view == view) {
                    frame.cull_ms += elapsed;
                    frame.cull_calls = frame.cull_calls.saturating_add(1);
                    frame.cull_candidates = frame.cull_candidates.saturating_add(candidates);
                }
            }
        });
    }
    result
}

#[cfg(not(feature = "render-profile-light"))]
unsafe extern "C" fn dirty_transform(node: *mut c_void) -> usize {
    let original = DIRTY_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return 0;
    }
    let original: DirtyFn = unsafe { core::mem::transmute(original) };
    let (inside_batch, top_level) = THREAD
        .try_with(|profile| {
            let Ok(mut profile) = profile.try_borrow_mut() else {
                return (false, false);
            };
            if profile.batch_depth == 0 {
                return (false, false);
            }
            if let Some(frame) = profile.frame.as_mut() {
                frame.dirty_calls = frame.dirty_calls.saturating_add(1);
            }
            let top_level = profile.dirty_depth == 0;
            if top_level {
                if let Some(frame) = profile.frame.as_mut() {
                    frame.dirty_roots = frame.dirty_roots.saturating_add(1);
                }
            }
            profile.dirty_depth = profile.dirty_depth.saturating_add(1);
            (true, top_level)
        })
        .unwrap_or((false, false));
    let started = (inside_batch && top_level).then(Instant::now);
    let result = unsafe { original(node) };
    if inside_batch {
        let elapsed = started.map(|started| started.elapsed().as_secs_f64() * 1000.0);
        THREAD.with(|profile| {
            if let Ok(mut profile) = profile.try_borrow_mut() {
                profile.dirty_depth = profile.dirty_depth.saturating_sub(1);
                if top_level {
                    if let Some(frame) = profile.frame.as_mut() {
                        frame.dirty_ms += elapsed.unwrap_or_default();
                    }
                }
            }
        });
    }
    result
}

/// Starts the CPU wall-clock interval around the engine's shadow pass.
pub(super) fn shadow_begin(view: *mut c_void) -> Option<Instant> {
    let started = enabled().then(Instant::now);
    #[cfg(feature = "shadow-cull-profile")]
    CULL_ACTIVE_VIEW.with(|active| active.set(if started.is_some() { view as usize } else { 0 }));
    #[cfg(not(feature = "shadow-cull-profile"))]
    let _ = view;
    started
}

/// Completes one paired main-view and shadow sample.
pub(super) fn shadow_end(view: *mut c_void, effective_mask: u32, started: Option<Instant>) {
    #[cfg(feature = "shadow-cull-profile")]
    CULL_ACTIVE_VIEW.with(|active| active.set(0));
    let Some(started) = started else { return };
    let shadow_ms = started.elapsed().as_secs_f64() * 1000.0;
    THREAD.with(|profile| {
        let Ok(mut profile) = profile.try_borrow_mut() else {
            return;
        };
        let Some(mut frame) = profile.frame.take() else {
            return;
        };
        if frame.view != view as usize {
            profile.frame = Some(frame);
            return;
        }
        frame.shadow_ms = shadow_ms;
        frame.effective_mask = effective_mask;
        profile.completed = Some(frame);
    });
}

/// Emits a completed sample after the other pass timers have stopped.
pub(super) fn flush() {
    if !enabled() {
        return;
    }
    let frame = THREAD.try_with(|profile| profile.try_borrow_mut().ok()?.completed.take());
    if let Ok(Some(frame)) = frame {
        record(frame);
    }
}

pub(super) fn install(api: &Api) {
    let _ = LOG.set(api.log);
    // Every site resolves and matches before the first hook.
    let (base, sites) = match target(api) {
        Ok(target) => target,
        Err(error) => {
            crate::say(
                api,
                LOG_WARN,
                &format!("render profile: not a supported build ({error}); no writes made"),
            );
            return;
        }
    };
    if let Err(error) = crate::plan::apply(api, &writes(base, &sites)) {
        crate::say(
            api,
            LOG_ERROR,
            &format!("render profile: {error}; no hooks installed"),
        );
        return;
    }
    let _ = HOOKED.set((base as usize, sites));
    #[cfg(not(feature = "render-profile-light"))]
    crate::say(
        api,
        LOG_INFO,
        "render profile: main, both batch lists, and dirty transforms hooked",
    );
    #[cfg(feature = "render-profile-light")]
    crate::say(
        api,
        LOG_INFO,
        "render profile: main and both batch lists hooked; dirty transforms unmeasured",
    );
    #[cfg(feature = "render-profile-toggle")]
    crate::say(
        api,
        LOG_INFO,
        "render profile toggle: OFF at startup; Ctrl+Alt+Shift+K cycles OFF, CPU timings, CPU and GPU timings",
    );
    #[cfg(feature = "shadow-cull-profile")]
    crate::say(
        api,
        LOG_INFO,
        "render profile: shadow caster cull timer hooked",
    );
}
