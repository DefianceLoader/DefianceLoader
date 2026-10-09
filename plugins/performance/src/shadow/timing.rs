//! Diagnostic, never shipped (feature `shadow-timing`): record CPU pass time
//! and nonblocking D3D11 timestamp intervals without changing cascade behavior.
use super::{d3d, layout, method, Cascades};
use core::ffi::{c_char, c_void};
use defiance_api::LOG_INFO;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::CString;
use std::sync::{
    atomic::{AtomicI32, AtomicUsize, Ordering},
    OnceLock,
};
use std::time::Instant;

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentThread() -> *mut c_void;
    fn GetCurrentThreadId() -> u32;
    fn GetThreadTimes(
        thread: *mut c_void,
        creation: *mut u64,
        exit: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
}

type Log = unsafe extern "C" fn(level: u32, message: *const c_char);
type CreateQuery = unsafe extern "system" fn(
    device: *mut c_void,
    desc: *const QueryDesc,
    query: *mut *mut c_void,
) -> i32;
type ContextBegin = unsafe extern "system" fn(context: *mut c_void, query: *mut c_void);
type ContextEnd = unsafe extern "system" fn(context: *mut c_void, query: *mut c_void);
type ContextGetDevice = unsafe extern "system" fn(context: *mut c_void, device: *mut *mut c_void);
type ContextGetData = unsafe extern "system" fn(
    context: *mut c_void,
    query: *mut c_void,
    data: *mut c_void,
    size: u32,
    flags: u32,
) -> i32;
type SceneCamera =
    unsafe extern "system" fn(scene: *mut c_void, out: *mut SharedCamera) -> *mut SharedCamera;
type CameraPosition = unsafe extern "system" fn(camera: *mut c_void) -> *const f32;

const DEVICE_CREATE_QUERY: usize = 24 * 8;
const CONTEXT_GET_DEVICE: usize = 3 * 8;
const CONTEXT_BEGIN: usize = 27 * 8;
const CONTEXT_END: usize = 28 * 8;
const CONTEXT_GET_DATA: usize = 29 * 8;
const QUERY_TIMESTAMP: u32 = 2;
const QUERY_TIMESTAMP_DISJOINT: u32 = 3;
const GET_DATA_DONOTFLUSH: u32 = 1;
const HRESULT_S_OK: i32 = 0;
const HRESULT_S_FALSE: i32 = 1;
const QUERY_RING_SIZE: usize = 16;
const REPORT_SAMPLES: usize = 120;
const VIEW_SCENE: usize = 0x90;
const SCENE_MAIN_CAMERA: usize = 0x60;
const CAMERA_GET_POSITION: usize = 0x30;

static LOG: OnceLock<Log> = OnceLock::new();
static MODE: AtomicUsize = AtomicUsize::new(0);
static FAILED_DEVICE: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

#[repr(C)]
struct QueryDesc {
    query: u32,
    misc_flags: u32,
}

#[repr(C)]
#[derive(Default)]
struct TimestampDisjoint {
    frequency: u64,
    disjoint: i32,
    padding: i32,
}

/// The scene's camera getter returns an MSVC `shared_ptr<Camera>`.
#[repr(C)]
struct SharedCamera {
    object: *mut c_void,
    control: *mut c_void,
}

struct Slot {
    disjoint: *mut c_void,
    start: *mut c_void,
    end: *mut c_void,
    pending: bool,
    pass_wall_ms: f64,
    thread_cpu_ticks: Option<u64>,
    cascades: u32,
    view: usize,
    camera_xyz: Option<[f32; 3]>,
}

impl Slot {
    unsafe fn create(device: *mut c_void) -> Result<Self, String> {
        let mut slot = Self {
            disjoint: core::ptr::null_mut(),
            start: core::ptr::null_mut(),
            end: core::ptr::null_mut(),
            pending: false,
            pass_wall_ms: 0.0,
            thread_cpu_ticks: None,
            cascades: 0,
            view: 0,
            camera_xyz: None,
        };
        slot.disjoint = create_query(device, QUERY_TIMESTAMP_DISJOINT)?;
        slot.start = create_query(device, QUERY_TIMESTAMP)?;
        slot.end = create_query(device, QUERY_TIMESTAMP)?;
        Ok(slot)
    }

    unsafe fn poll(&self, context: *mut c_void) -> Poll {
        let get_data: ContextGetData = unsafe { method(context, CONTEXT_GET_DATA) };
        let mut disjoint = TimestampDisjoint::default();
        let result = unsafe {
            get_data(
                context,
                self.disjoint,
                (&mut disjoint as *mut TimestampDisjoint).cast(),
                core::mem::size_of::<TimestampDisjoint>() as u32,
                GET_DATA_DONOTFLUSH,
            )
        };
        if result == HRESULT_S_FALSE {
            return Poll::Pending;
        }
        if result != HRESULT_S_OK {
            return Poll::Invalid;
        }

        let mut start = 0u64;
        let mut end = 0u64;
        for (query, value) in [(self.start, &mut start), (self.end, &mut end)] {
            let result = unsafe {
                get_data(
                    context,
                    query,
                    (value as *mut u64).cast(),
                    core::mem::size_of::<u64>() as u32,
                    GET_DATA_DONOTFLUSH,
                )
            };
            if result == HRESULT_S_FALSE {
                return Poll::Pending;
            }
            if result != HRESULT_S_OK {
                return Poll::Invalid;
            }
        }
        if disjoint.disjoint != 0 || disjoint.frequency == 0 || end < start {
            return Poll::Invalid;
        }
        let gpu_ms = (end - start) as f64 * 1000.0 / disjoint.frequency as f64;
        if gpu_ms.is_finite() {
            Poll::Ready(gpu_ms)
        } else {
            Poll::Invalid
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        for query in [self.disjoint, self.start, self.end] {
            if query.is_null() {
                continue;
            }
            let release: unsafe extern "system" fn(*mut c_void) -> u32 =
                unsafe { method(query, 2 * 8) };
            unsafe { release(query) };
        }
    }
}

struct State {
    device: usize,
    context: usize,
    slots: Vec<Slot>,
    cursor: usize,
    active: bool,
    samples: Vec<Sample>,
    skipped: u32,
    invalid: u32,
}

impl State {
    unsafe fn create(device: *mut c_void, context: *mut c_void) -> Result<Self, String> {
        let mut slots = Vec::with_capacity(QUERY_RING_SIZE);
        for _ in 0..QUERY_RING_SIZE {
            slots.push(unsafe { Slot::create(device)? });
        }
        Ok(Self {
            device: device as usize,
            context: context as usize,
            slots,
            cursor: 0,
            active: false,
            samples: Vec::with_capacity(REPORT_SAMPLES),
            skipped: 0,
            invalid: 0,
        })
    }

    fn record(&mut self, sample: Sample) -> Option<Vec<String>> {
        self.samples.push(sample);
        if self.samples.len() < REPORT_SAMPLES {
            return None;
        }
        let mut groups: BTreeMap<usize, Vec<&Sample>> = BTreeMap::new();
        for sample in &self.samples {
            groups.entry(sample.view).or_default().push(sample);
        }
        let thread_id = unsafe { GetCurrentThreadId() };
        let messages = groups
            .into_iter()
            .map(|(view, samples)| {
                let n = samples.len();
                let mut wall: Vec<f64> = samples.iter().map(|s| s.pass_wall_ms).collect();
                let mut gpu: Vec<f64> = samples.iter().map(|s| s.gpu_ms).collect();
                wall.sort_by(f64::total_cmp);
                gpu.sort_by(f64::total_cmp);
                let p95 = (n * 95 + 99) / 100 - 1;
                let cpu_ticks: u128 = samples
                    .iter()
                    .filter_map(|s| s.thread_cpu_ticks)
                    .map(u128::from)
                    .sum();
                let cpu_n = samples
                    .iter()
                    .filter(|s| s.thread_cpu_ticks.is_some())
                    .count();
                let thread_cpu = if cpu_n == 0 {
                    "unavailable".to_string()
                } else {
                    format!("{:.2} n={cpu_n}", cpu_ticks as f64 / 10_000.0 / cpu_n as f64)
                };
                let cascades = samples
                    .iter()
                    .map(|s| s.cascades)
                    .min()
                    .zip(samples.iter().map(|s| s.cascades).max())
                    .map(|(min, max)| {
                        if min == max {
                            min.to_string()
                        } else {
                            format!("{min}-{max}")
                        }
                    })
                    .unwrap_or_else(|| "?".into());
                let camera = samples
                    .iter()
                    .rev()
                    .find_map(|s| s.camera_xyz)
                    .map(|[x, y, z]| format!("({x:.2},{y:.2},{z:.2})"))
                    .unwrap_or_else(|| "unavailable".to_string());
                format!(
                    "shadow timing: mode={} thread_id={thread_id} view={view:#x} n={n}/{REPORT_SAMPLES} cascades={} pass_wall_ms median/p95={:.2}/{:.2} thread_cpu_pooled_ms/pass={} gpu_elapsed_ms median/p95={:.2}/{:.2} skipped={} invalid={} camera_xyz_at_report={camera}",
                    mode_name(MODE.load(Ordering::Relaxed)),
                    cascades,
                    wall[n / 2],
                    wall[p95],
                    thread_cpu,
                    gpu[n / 2],
                    gpu[p95],
                    self.skipped,
                    self.invalid,
                )
            })
            .collect();
        self.samples.clear();
        self.skipped = 0;
        self.invalid = 0;
        Some(messages)
    }
}

#[derive(Clone, Copy)]
struct Sample {
    pass_wall_ms: f64,
    thread_cpu_ticks: Option<u64>,
    gpu_ms: f64,
    cascades: u32,
    view: usize,
    camera_xyz: Option<[f32; 3]>,
}

pub(super) struct Pass {
    slot: usize,
    started: Instant,
    thread_cpu_start: Option<u64>,
    device: usize,
    context: usize,
    cascades: u32,
    view: usize,
    camera_xyz: Option<[f32; 3]>,
}

enum Poll {
    Pending,
    Ready(f64),
    Invalid,
}

fn create_query(device: *mut c_void, kind: u32) -> Result<*mut c_void, String> {
    let create: CreateQuery = unsafe { method(device, DEVICE_CREATE_QUERY) };
    let desc = QueryDesc {
        query: kind,
        misc_flags: 0,
    };
    let mut query = core::ptr::null_mut();
    let result = unsafe { create(device, &desc, &mut query) };
    if result < 0 || query.is_null() {
        Err(format!("ID3D11Device::CreateQuery failed ({result:#x})"))
    } else {
        Ok(query)
    }
}

pub(super) fn install(log: Log, mode: Cascades) {
    let _ = LOG.set(log);
    MODE.store(mode_code(mode), Ordering::Relaxed);
}

/// A mode switch drops pending queries and partial report windows on this thread.
#[cfg(feature = "render-profile-toggle")]
pub(super) fn reset() {
    let _ = STATE.try_with(|cell| {
        if let Ok(mut cell) = cell.try_borrow_mut() {
            *cell = None;
        }
    });
}

pub(super) fn begin(view: *mut c_void) -> Option<Pass> {
    let Some((context, _)) = (unsafe { d3d(view) }) else {
        return None;
    };
    let get_device: ContextGetDevice = unsafe { method(context, CONTEXT_GET_DEVICE) };
    let mut device = core::ptr::null_mut();
    unsafe { get_device(context, &mut device) };
    if device.is_null() {
        return None;
    }
    if FAILED_DEVICE.load(Ordering::Relaxed) == device as usize {
        unsafe { super::release(device) };
        return None;
    }
    let cascades = unsafe { *((view as *const u8).add(layout().cascade_count) as *const u32) };
    let camera_xyz = unsafe { camera_position(view) };
    let mut message = None;
    let mut report = None;
    let token = STATE
        .try_with(|cell| {
            let mut cell = cell.try_borrow_mut().ok()?;
            if cell.as_ref().is_some_and(|state| {
                state.device != device as usize || state.context != context as usize
            }) {
                *cell = None;
            }
            if cell.is_none() {
                match unsafe { State::create(device, context) } {
                    Ok(state) => *cell = Some(state),
                    Err(error) => {
                        FAILED_DEVICE.store(device as usize, Ordering::Relaxed);
                        message =
                            Some(format!("shadow timing: {error}; pass measurement disabled"));
                        return None;
                    }
                }
            }
            let state = cell.as_mut()?;
            if state.active {
                return None;
            }
            let mut selected = None;
            let mut waiting = false;
            for offset in 0..state.slots.len() {
                let index = (state.cursor + offset) % state.slots.len();
                if state.slots[index].pending && !waiting {
                    match unsafe { state.slots[index].poll(context) } {
                        Poll::Pending => waiting = true,
                        Poll::Invalid => {
                            state.slots[index].pending = false;
                            state.invalid += 1;
                        }
                        Poll::Ready(gpu_ms) => {
                            let sample = {
                                let slot = &mut state.slots[index];
                                slot.pending = false;
                                Sample {
                                    pass_wall_ms: slot.pass_wall_ms,
                                    thread_cpu_ticks: slot.thread_cpu_ticks,
                                    gpu_ms,
                                    cascades: slot.cascades,
                                    view: slot.view,
                                    camera_xyz: slot.camera_xyz,
                                }
                            };
                            report = state.record(sample);
                        }
                    }
                }
                if !state.slots[index].pending {
                    selected = Some(index);
                    break;
                }
            }
            let Some(index) = selected else {
                state.skipped += 1;
                state.cursor = (state.cursor + 1) % state.slots.len();
                return None;
            };
            let slot = &mut state.slots[index];
            let begin: ContextBegin = unsafe { method(context, CONTEXT_BEGIN) };
            let end: ContextEnd = unsafe { method(context, CONTEXT_END) };
            unsafe {
                begin(context, slot.disjoint);
                end(context, slot.start);
            }
            let thread_cpu_start = thread_time();
            let started = Instant::now();
            state.active = true;
            state.cursor = (index + 1) % state.slots.len();
            Some(Pass {
                slot: index,
                started,
                thread_cpu_start,
                device: device as usize,
                context: context as usize,
                cascades,
                view: view as usize,
                camera_xyz,
            })
        })
        .ok()
        .flatten();
    unsafe { super::release(device) };
    if let Some(message) = message {
        emit(message);
    }
    if let Some(messages) = report {
        for message in messages {
            emit(message);
        }
    }
    token
}

pub(super) fn end(pass: Option<Pass>) {
    let Some(pass) = pass else { return };
    let pass_wall_ms = pass.started.elapsed().as_secs_f64() * 1000.0;
    let thread_cpu_ticks = pass
        .thread_cpu_start
        .zip(thread_time())
        .and_then(|(start, end)| end.checked_sub(start));
    let _ = STATE.try_with(|cell| {
        let Ok(mut cell) = cell.try_borrow_mut() else {
            return;
        };
        let Some(state) = cell.as_mut() else { return };
        if !state.active
            || state.device != pass.device
            || state.context != pass.context
            || pass.slot >= state.slots.len()
        {
            state.active = false;
            return;
        }
        let slot = &mut state.slots[pass.slot];
        let end: ContextEnd = unsafe { method(pass.context as *mut c_void, CONTEXT_END) };
        unsafe {
            end(pass.context as *mut c_void, slot.end);
            end(pass.context as *mut c_void, slot.disjoint);
        }
        slot.pass_wall_ms = pass_wall_ms;
        slot.thread_cpu_ticks = thread_cpu_ticks;
        slot.cascades = pass.cascades;
        slot.view = pass.view;
        slot.camera_xyz = pass.camera_xyz;
        slot.pending = true;
        state.active = false;
    });
}

fn thread_time() -> Option<u64> {
    let mut creation = 0u64;
    let mut exit = 0u64;
    let mut kernel = 0u64;
    let mut user = 0u64;
    if unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return None;
    }
    kernel.checked_add(user)
}

/// Read the main camera through the same scene and camera slots used by the
/// game's main-view pass, and release the getter's temporary shared reference.
unsafe fn camera_position(view: *mut c_void) -> Option<[f32; 3]> {
    let scene = unsafe { *((view as *const u8).add(VIEW_SCENE) as *const *mut c_void) };
    if scene.is_null() {
        return None;
    }
    let get_camera: SceneCamera = unsafe { method(scene, SCENE_MAIN_CAMERA) };
    let mut camera = SharedCamera {
        object: core::ptr::null_mut(),
        control: core::ptr::null_mut(),
    };
    unsafe { get_camera(scene, &mut camera) };
    let position = if camera.object.is_null() {
        None
    } else {
        let get_position: CameraPosition = unsafe { method(camera.object, CAMERA_GET_POSITION) };
        let pointer = unsafe { get_position(camera.object) };
        if pointer.is_null() {
            None
        } else {
            let xyz = unsafe { [*pointer, *pointer.add(1), *pointer.add(2)] };
            xyz.iter().all(|v| v.is_finite()).then_some(xyz)
        }
    };
    unsafe { release_shared(camera.control) };
    position
}

/// MSVC's `_Ref_count_base` drops the managed object when the strong count
/// reaches zero, then drops the control block when the weak count reaches zero.
unsafe fn release_shared(control: *mut c_void) {
    if control.is_null() {
        return;
    }
    let strong = unsafe { &*((control as *const u8).add(8) as *const AtomicI32) };
    if strong.fetch_sub(1, Ordering::AcqRel) != 1 {
        return;
    }
    let destroy: unsafe extern "C" fn(*mut c_void) = unsafe { method(control, 0) };
    unsafe { destroy(control) };
    let weak = unsafe { &*((control as *const u8).add(0xc) as *const AtomicI32) };
    if weak.fetch_sub(1, Ordering::AcqRel) == 1 {
        let delete: unsafe extern "C" fn(*mut c_void) = unsafe { method(control, 8) };
        unsafe { delete(control) };
    }
}

fn mode_code(mode: Cascades) -> usize {
    match mode {
        Cascades::All => 0,
        Cascades::Rotate => 1,
        Cascades::Near => 2,
        Cascades::FarHalf => 3,
    }
}

fn mode_name(mode: usize) -> &'static str {
    match mode {
        1 => "rotate",
        2 => "near",
        3 => "far_half",
        _ => "all",
    }
}

fn emit(message: String) {
    let Some(log) = LOG.get() else { return };
    let Ok(message) = CString::new(message) else {
        return;
    };
    unsafe { log(LOG_INFO, message.as_ptr()) };
}
