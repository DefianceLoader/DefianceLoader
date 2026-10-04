//! Temporary main-view and shadow timing probe (feature `render-profile`).
//!
//! The probe is pinned to the Steam 2026-09-25 `world2.dll` layout. It records
//! paired main-view/shadow samples. Formatting runs after a measured pass,
//! only when a report window fills.

use core::ffi::{c_char, c_void};
#[cfg(feature = "render-profile-toggle")]
use core::sync::atomic::AtomicBool;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
#[cfg(feature = "shadow-cull-profile")]
use std::cell::Cell;
use std::cell::RefCell;
use std::sync::OnceLock;
#[cfg(feature = "render-profile-toggle")]
use std::time::Duration;
use std::time::Instant;

use defiance_api::{Api, LOG_INFO, LOG_WARN};

const GAME_SHA: &str = "c336b5ed4a367628a9c370457d82b1d4e75cab9007cffe5354e27688e836f98e";
#[cfg(feature = "shadow-cull-profile")]
const WORLD2_SHA: &str = "c39827bec79c0c4e1358259b5a2b3a6762e9270c6f5e1f25ce32d3f95b6a15c2";
const WORLD2_IMAGE_SIZE: usize = 0x4bd000;
const MAIN_CALL: usize = 0x18d9d9;
const MAIN_FN: usize = 0x18f940;
const BATCH_CALLS: [usize; 2] = [0x19031f, 0x190391];
const BATCH_FN: usize = 0x18f0c0;
const DIRTY_FN: usize = 0x154160;
#[cfg(feature = "shadow-cull-profile")]
const CULL_FN: usize = 0x1da490;
const REPORT_SAMPLES: usize = 240;
const MAX_GROUPS: usize = 64;
#[cfg(feature = "render-profile-toggle")]
const MODE_OFF: u8 = 0;
#[cfg(feature = "render-profile-toggle")]
const MODE_CPU: u8 = 1;
#[cfg(feature = "render-profile-toggle")]
const MODE_GPU: u8 = 2;

const INSTALLED_MAIN: u8 = 1;
const INSTALLED_BATCH_0: u8 = 2;
const INSTALLED_BATCH_1: u8 = 4;
#[cfg(not(feature = "render-profile-light"))]
const INSTALLED_DIRTY: u8 = 8;
#[cfg(feature = "shadow-cull-profile")]
const INSTALLED_CULL: u8 = 16;

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
    #[cfg(feature = "shadow-cull-profile")]
    fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, capacity: u32) -> u32;
}
#[cfg(feature = "render-profile-toggle")]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetAsyncKeyState(key: i32) -> i16;
}

static LOG: OnceLock<Log> = OnceLock::new();
static INSTALLED: AtomicU8 = AtomicU8::new(0);
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

fn call_target(call: *const u8) -> usize {
    let displacement = unsafe { core::ptr::read_unaligned(call.add(1).cast::<i32>()) } as isize;
    (call as isize + 5 + displacement) as usize
}

fn exact(base: *mut u8, size: usize, rva: usize, bytes: &[u8]) -> bool {
    rva.checked_add(bytes.len()).is_some_and(|end| end <= size)
        && unsafe { core::slice::from_raw_parts(base.add(rva), bytes.len()) } == bytes
}

fn range(base: *mut u8, size: usize, rva: usize, length: usize) -> bool {
    !base.is_null() && rva.checked_add(length).is_some_and(|end| end <= size)
}

#[cfg(feature = "shadow-cull-profile")]
fn expected_world2(base: *mut u8) -> bool {
    use std::os::windows::ffi::OsStringExt;
    let mut path = [0u16; 32768];
    let length =
        unsafe { GetModuleFileNameW(base.cast(), path.as_mut_ptr(), path.len() as u32) } as usize;
    if length == 0 || length >= path.len() {
        return false;
    }
    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&path[..length]));
    defiance_core::sha256::file(&path).is_ok_and(|sha| sha == WORLD2_SHA)
}

fn target(api: &Api) -> Result<(*mut u8, usize), &'static str> {
    let Some(runtime) = crate::RUNTIME.get() else {
        return Err("Core runtime is unavailable");
    };
    if runtime.game.sha != GAME_SHA {
        return Err("game.dll SHA-256 is not the verified Steam 2026-09-25 build");
    }
    let base = unsafe { (api.module_base)(c"world2.dll".as_ptr()) } as *mut u8;
    if base.is_null() {
        return Err("world2.dll is not loaded");
    }
    let size = unsafe { (api.module_size)(base.cast()) };
    if size != WORLD2_IMAGE_SIZE {
        return Err("world2.dll image size does not match the verified Steam build");
    }
    #[cfg(feature = "shadow-cull-profile")]
    if !expected_world2(base) {
        return Err("world2.dll SHA-256 is not the verified Steam 2026-09-25 build");
    }
    if !range(base, size, DIRTY_FN, 5)
        || !range(base, size, MAIN_CALL, 5)
        || !range(base, size, MAIN_FN, 13)
        || !range(base, size, BATCH_CALLS[0], 5)
        || !range(base, size, BATCH_CALLS[1], 5)
        || !range(base, size, BATCH_FN, 7)
    {
        return Err("a render-profile hook site lies outside world2.dll");
    }
    let calls_ok = exact(base, size, MAIN_CALL, &[0xe8, 0x62, 0x1f, 0x00, 0x00])
        && exact(base, size, BATCH_CALLS[0], &[0xe8, 0x9c, 0xed, 0xff, 0xff])
        && exact(base, size, BATCH_CALLS[1], &[0xe8, 0x2a, 0xed, 0xff, 0xff])
        && call_target(unsafe { base.add(MAIN_CALL) }) == base as usize + MAIN_FN
        && call_target(unsafe { base.add(BATCH_CALLS[0]) }) == base as usize + BATCH_FN
        && call_target(unsafe { base.add(BATCH_CALLS[1]) }) == base as usize + BATCH_FN;
    if !calls_ok
        || !exact(
            base,
            size,
            MAIN_FN,
            &[
                0x40, 0x55, 0x53, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57,
            ],
        )
        || !exact(
            base,
            size,
            BATCH_FN,
            &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x18],
        )
        || !exact(base, size, DIRTY_FN, &[0x48, 0x89, 0x5c, 0x24, 0x08])
    {
        return Err("world2.dll render-profile bytes or call targets do not match");
    }
    #[cfg(feature = "shadow-cull-profile")]
    if !range(base, size, CULL_FN, 16)
        || !exact(
            base,
            size,
            CULL_FN,
            &[
                0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x48, 0x89, 0x7c, 0x24,
                0x20, 0x55,
            ],
        )
        || !exact(base, size, 0x191c2c, &[0xff, 0xd3, 0x90])
    {
        return Err("world2.dll shadow-cull entry does not match the verified Steam build");
    }
    Ok((base, size))
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
    let cull = if INSTALLED.load(Ordering::Acquire) & INSTALLED_CULL != 0 {
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
        // The Steam 2026-09-25 caller consumes output as an 8-byte pointer vector.
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

fn install_one(api: &Api, site: *mut u8, detour: *mut c_void, original: &AtomicUsize) -> bool {
    let mut trampoline = core::ptr::null_mut();
    if unsafe { (api.hook_call)(site.cast(), detour, &mut trampoline) } != 0 || trampoline.is_null()
    {
        return false;
    }
    original.store(trampoline as usize, Ordering::Release);
    true
}

pub(super) fn install(api: &Api) {
    let _ = LOG.set(api.log);
    let (base, _) = match target(api) {
        Ok(target) => target,
        Err(error) => {
            crate::say(
                api,
                LOG_WARN,
                &format!("render profile: {error}; probe disabled"),
            );
            return;
        }
    };
    let mut installed = 0;
    if install_one(
        api,
        unsafe { base.add(MAIN_CALL) },
        main_view as *mut c_void,
        &MAIN_ORIGINAL,
    ) {
        installed |= INSTALLED_MAIN;
    }
    if install_one(
        api,
        unsafe { base.add(BATCH_CALLS[0]) },
        batch_0 as *mut c_void,
        &BATCH_ORIGINAL[0],
    ) {
        installed |= INSTALLED_BATCH_0;
    }
    if install_one(
        api,
        unsafe { base.add(BATCH_CALLS[1]) },
        batch_1 as *mut c_void,
        &BATCH_ORIGINAL[1],
    ) {
        installed |= INSTALLED_BATCH_1;
    }
    #[cfg(feature = "shadow-cull-profile")]
    {
        let mut trampoline = core::ptr::null_mut();
        if unsafe {
            (api.hook_exact)(
                base.add(CULL_FN).cast(),
                shadow_cull as *mut c_void,
                5,
                &mut trampoline,
            )
        } == 0
            && !trampoline.is_null()
        {
            CULL_ORIGINAL.store(trampoline as usize, Ordering::Release);
            installed |= INSTALLED_CULL;
        }
    }
    #[cfg(not(feature = "render-profile-light"))]
    {
        let mut trampoline = core::ptr::null_mut();
        if unsafe {
            (api.hook_exact)(
                base.add(DIRTY_FN).cast(),
                dirty_transform as *mut c_void,
                5,
                &mut trampoline,
            )
        } == 0
            && !trampoline.is_null()
        {
            DIRTY_ORIGINAL.store(trampoline as usize, Ordering::Release);
            installed |= INSTALLED_DIRTY;
        }
    }
    INSTALLED.store(installed, Ordering::Release);
    #[cfg(not(feature = "render-profile-light"))]
    let expected = INSTALLED_MAIN | INSTALLED_BATCH_0 | INSTALLED_BATCH_1 | INSTALLED_DIRTY;
    #[cfg(feature = "render-profile-light")]
    let expected = INSTALLED_MAIN | INSTALLED_BATCH_0 | INSTALLED_BATCH_1;
    #[cfg(feature = "shadow-cull-profile")]
    let expected = expected | INSTALLED_CULL;
    if installed == expected {
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
    } else {
        crate::say(
            api,
            LOG_WARN,
            &format!(
                "render profile: partial hooks installed ({installed:#x}); timings are incomplete"
            ),
        );
    }
}

pub(super) fn contract(api: &Api) {
    let installed = INSTALLED.load(Ordering::Acquire);
    if installed == 0 {
        return;
    }
    let Ok((base, _)) = target(api) else {
        return;
    };
    if installed & INSTALLED_MAIN != 0 {
        let mut original = core::ptr::null_mut();
        unsafe {
            (api.hook_call)(
                base.add(MAIN_CALL).cast(),
                main_view as *mut c_void,
                &mut original,
            )
        };
    }
    if installed & INSTALLED_BATCH_0 != 0 {
        let mut original = core::ptr::null_mut();
        unsafe {
            (api.hook_call)(
                base.add(BATCH_CALLS[0]).cast(),
                batch_0 as *mut c_void,
                &mut original,
            )
        };
    }
    if installed & INSTALLED_BATCH_1 != 0 {
        let mut original = core::ptr::null_mut();
        unsafe {
            (api.hook_call)(
                base.add(BATCH_CALLS[1]).cast(),
                batch_1 as *mut c_void,
                &mut original,
            )
        };
    }
    #[cfg(feature = "shadow-cull-profile")]
    if installed & INSTALLED_CULL != 0 {
        let mut original = core::ptr::null_mut();
        unsafe {
            (api.hook_exact)(
                base.add(CULL_FN).cast(),
                shadow_cull as *mut c_void,
                5,
                &mut original,
            )
        };
    }
    #[cfg(not(feature = "render-profile-light"))]
    if installed & INSTALLED_DIRTY != 0 {
        let mut original = core::ptr::null_mut();
        unsafe {
            (api.hook_exact)(
                base.add(DIRTY_FN).cast(),
                dirty_transform as *mut c_void,
                5,
                &mut original,
            )
        };
    }
}
