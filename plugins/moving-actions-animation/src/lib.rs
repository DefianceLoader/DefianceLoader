//! Experimental lower-body sampler overlay for standing infantry actions.
//!
//! The native update and both native samplers always run. During the update
//! scope only, validated thigh/calf/foot/toe samples are read from embedded
//! locomotion composites and written to the sampler output buffers.

use core::cell::RefCell;
use core::ffi::{c_char, c_void};
use defiance_api::{Api, Plugin, ABI_VERSION, LOG_DEBUG, LOG_ERROR, LOG_INFO};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::{fs, path::PathBuf};

#[derive(Clone, Copy)]
struct BuildBindings {
    sha256: &'static str,
    update: usize,
    position: usize,
    rotation: usize,
    slerp: usize,
    human_animation_vt: usize,
    human_chassis_vt: usize,
}

// Exact logic.dll identities and locations, derived from unique masked
// function signatures plus independent MSVC RTTI vtable recovery.
const BUILDS: &[BuildBindings] = &[
    BuildBindings {
        sha256: "eb8674f1d16595a3e9cf6a9ec0062735b1184976495d8d6ade36f7e2574e8aab",
        update: 0x2c3900,
        position: 0x436260,
        rotation: 0x436460,
        slerp: 0x136880,
        human_animation_vt: 0x72b0d8,
        human_chassis_vt: 0x72b370,
    },
    BuildBindings {
        sha256: "30264904e1d5199b954bafbd7828cf7190930c246d35fa7b94eefa915e8f0c38",
        update: 0x2c3990,
        position: 0x4362f0,
        rotation: 0x4364f0,
        slerp: 0x136910,
        human_animation_vt: 0x72b118,
        human_chassis_vt: 0x72b3b0,
    },
    BuildBindings {
        sha256: "1216d627c7288c7db6940168363be582232ed4d3cb860b8b8c8d7489652eca74",
        update: 0x2c3900,
        position: 0x4368a0,
        rotation: 0x436aa0,
        slerp: 0x136880,
        human_animation_vt: 0x72c0d8,
        human_chassis_vt: 0x72c370,
    },
    BuildBindings {
        sha256: "adb3ad95926036809b4e554b466bef33d4ac7aa5303e59a9e4a940890bc334b5",
        update: 0x2c3990,
        position: 0x436930,
        rotation: 0x436b30,
        slerp: 0x136910,
        human_animation_vt: 0x72c118,
        human_chassis_vt: 0x72c3b0,
    },
];
const MAX_TRACKS: usize = 256;
const MAX_KEYS_PER_CHANNEL: usize = 16_384;
const MAX_CLIP_BYTES: usize = 16 * 1024 * 1024;
const MAX_ONCE_LOGS: usize = 64;

const UPDATE_PREFIX: &[u8] = &[
    0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10, 0x55, 0x56, 0x57, 0x48, 0x81, 0xec, 0xe0, 0x00, 0x00,
];
const POSITION_PREFIX: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x7c, 0x24, 0x10, 0x48, 0x8b, 0x41, 0x08, 0x49, 0x8b,
];
const ROTATION_PREFIX: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x6c, 0x24, 0x18, 0x48, 0x89, 0x74, 0x24, 0x20, 0x57,
];
const SLERP_PREFIX: &[u8] = &[
    0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10, 0x57, 0x48, 0x81, 0xec, 0xe0, 0x00, 0x00, 0x00, 0xf3,
];

static BASE: AtomicUsize = AtomicUsize::new(0);
static SIZE: AtomicUsize = AtomicUsize::new(0);
static ACTIVE_BUILD: AtomicUsize = AtomicUsize::new(0);
static LOGGER: AtomicUsize = AtomicUsize::new(0);
static ORIGINALS: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
static ASSETS: OnceLock<Result<AssetSet, String>> = OnceLock::new();
static APPLIED: LazyLock<Mutex<HashSet<(usize, i32, u8)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static MOTION: LazyLock<Mutex<HashMap<usize, MotionSample>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static GAIT_REPORTS: LazyLock<Mutex<HashSet<(usize, u8)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(Clone, Copy)]
struct MotionSample {
    unit: usize,
    facet: usize,
    manager: usize,
    nav: i32,
    point: [f32; 2],
    time: u64,
    direction: Option<([f32; 2], u64)>,
}

static PHASES: LazyLock<Mutex<HashMap<(usize, i32), PhaseObservation>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct PhaseObservation {
    first_rotation: [f32; 4],
    min_time: f32,
    max_time: f32,
    max_rotation_degrees: f32,
    samples: u32,
    first_local_rotation: Option<[f32; 4]>,
    local_rotation_degrees: f32,
    latest_pose_mismatch_degrees: Option<f32>,
    post_pose_samples: u32,
    reported: bool,
}

thread_local! {
    /// Exists only while the native HumanAnimationFacet update is executing.
    /// It deliberately does not persist any engine draw pointer between frames.
    static FRAME: RefCell<Option<FrameContext>> = const { RefCell::new(None) };
}

#[link(name = "kernel32")]
extern "system" {
    fn GetTickCount64() -> u64;
    fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, capacity: u32) -> u32;
    fn VirtualQuery(
        address: *const c_void,
        info: *mut MemoryBasicInformation,
        length: usize,
    ) -> usize;
}

#[repr(C)]
struct MemoryBasicInformation {
    base: *mut c_void,
    allocation_base: *mut c_void,
    allocation_protect: u32,
    partition_id: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

type Update = unsafe extern "C" fn(*mut u8, f32);
type Slerp = unsafe extern "C" fn(*const f32, *mut f32, *const f32, f32) -> *mut f32;

fn active_build() -> Option<&'static BuildBindings> {
    BUILDS.get(ACTIVE_BUILD.load(Ordering::Acquire).checked_sub(1)?)
}

extern "C" {
    #[link_name = "defiance_animation_position"]
    fn position(descriptor: *const u8, output: *mut f32, time: f32, cursor: *mut u32) -> *mut f32;
    #[link_name = "defiance_animation_rotation"]
    fn rotation(descriptor: *const u8, output: *mut f32, time: f32, cursor: *mut u32) -> *mut f32;
}

// The native evaluator knows these internal samplers' register usage. Its
// unblended path reuses XMM2 after calling the position sampler, rather than
// reloading time before the rotation sampler. A normal Rust/C detour can
// clobber that volatile register. Run the original with untouched arguments,
// then preserve its returned GPR/SIMD state around the Rust overlay callback.
// The extra saved arguments retain the original time even if a native sampler
// changes it. Each wrapper has a Win64 unwind record and aligned shadow space.
macro_rules! sampler_wrapper {
    ($wrapper:ident, $overlay:ident, $offset:literal) => {
        core::arch::global_asm!(
            ".text",
            ".globl {wrapper}",
            ".def {wrapper}; .scl 2; .type 32; .endef",
            ".seh_proc {wrapper}",
            "{wrapper}:",
            "sub rsp, 0xe8",
            ".seh_stackalloc 0xe8",
            ".seh_endprologue",
            "mov [rsp + 0x20], rcx",
            "mov [rsp + 0x28], rdx",
            "movss [rsp + 0x30], xmm2",
            "2:",
            "mov rax, [rip + {originals} + {offset}]",
            "test rax, rax",
            "jne 3f",
            "pause",
            "jmp 2b",
            "3:",
            "call rax",
            "mov [rsp + 0x40], rax",
            "mov [rsp + 0x48], rcx",
            "mov [rsp + 0x50], rdx",
            "mov [rsp + 0x58], r8",
            "mov [rsp + 0x60], r9",
            "mov [rsp + 0x68], r10",
            "mov [rsp + 0x70], r11",
            "movaps [rsp + 0x80], xmm0",
            "movaps [rsp + 0x90], xmm1",
            "movaps [rsp + 0xa0], xmm2",
            "movaps [rsp + 0xb0], xmm3",
            "movaps [rsp + 0xc0], xmm4",
            "movaps [rsp + 0xd0], xmm5",
            "mov rcx, [rsp + 0x20]",
            "mov rdx, [rsp + 0x28]",
            "movss xmm2, [rsp + 0x30]",
            "mov r9, [rsp + 0x40]",
            "call {overlay}",
            "movaps xmm0, [rsp + 0x80]",
            "movaps xmm1, [rsp + 0x90]",
            "movaps xmm2, [rsp + 0xa0]",
            "movaps xmm3, [rsp + 0xb0]",
            "movaps xmm4, [rsp + 0xc0]",
            "movaps xmm5, [rsp + 0xd0]",
            "mov rcx, [rsp + 0x48]",
            "mov rdx, [rsp + 0x50]",
            "mov r8, [rsp + 0x58]",
            "mov r9, [rsp + 0x60]",
            "mov r10, [rsp + 0x68]",
            "mov r11, [rsp + 0x70]",
            "mov rax, [rsp + 0x40]",
            "add rsp, 0xe8",
            "ret",
            ".seh_endproc",
            wrapper = sym $wrapper,
            overlay = sym $overlay,
            originals = sym ORIGINALS,
            offset = const $offset,
        );
    };
}

sampler_wrapper!(position, augment_position, 8);
sampler_wrapper!(rotation, augment_rotation, 16);

#[derive(Clone, Debug)]
struct Track {
    name: Vec<u8>,
    positions: Vec<[f32; 4]>,
    rotations: Vec<[f32; 5]>,
}

#[derive(Clone, Debug)]
struct Clip {
    duration: f32,
    tracks: Vec<Track>,
}

#[derive(Clone, Debug)]
struct Pair {
    stock: Clip,
    moving: Clip,
    changed: Vec<usize>,
}

#[derive(Clone, Debug)]
struct AssetSet {
    throw: Pair,
    switch: Pair,
    throw_back: Pair,
}

#[derive(Clone, Copy)]
struct Owner {
    facet: usize,
    unit: usize,
    chassis: usize,
}

#[derive(Clone, Copy)]
struct ClipFrame {
    action: i32,
    kind: u8,
    geometry: usize,
    binding: usize,
    animation: usize,
    tracks: usize,
    valid: bool,
}

#[derive(Clone, Copy)]
struct FrameContext {
    owner: Owner,
    clip: Option<ClipFrame>,
    thigh: Option<(usize, [f32; 4])>,
    motion: Option<([f32; 2], u8)>,
}

fn original(index: usize) -> usize {
    loop {
        let value = ORIGINALS[index].load(Ordering::Acquire);
        if value != 0 {
            return value;
        }
        core::hint::spin_loop();
    }
}

unsafe fn read<T: Copy>(base: *const u8, offset: usize) -> Option<T> {
    if base.is_null() {
        return None;
    }
    let address = (base as usize).checked_add(offset)?;
    Some((address as *const T).read_unaligned())
}

unsafe fn log(level: u32, message: &str) {
    let logger = LOGGER.load(Ordering::Acquire);
    if logger == 0 {
        return;
    }
    if let Ok(message) = std::ffi::CString::new(message) {
        let callback: unsafe extern "C" fn(u32, *const c_char) = core::mem::transmute(logger);
        callback(level, message.as_ptr());
    }
}

fn log_first_application(chassis: usize, action: i32, channel: Channel, time: f32) {
    let (channel_id, channel_name) = match channel {
        Channel::Position => (0, "position"),
        Channel::Rotation => (1, "rotation"),
    };
    let should_log = if let Ok(mut applied) = APPLIED.lock() {
        if applied.contains(&(chassis, action, channel_id)) || applied.len() >= MAX_ONCE_LOGS {
            false
        } else {
            applied.insert((chassis, action, channel_id));
            true
        }
    } else {
        false
    };
    if should_log {
        unsafe {
            log(
                LOG_DEBUG,
                &format!("moving action lower-body sample overlay applied: chassis={chassis:#x} action={action:#x} channel={channel_name} sample_time={time:.3}"),
            );
        }
    }
}

/// Keep only copied scalar observations; no draw/clip pointers survive a frame.
/// One summary per soldier/action, capped at 32 pairs for this diagnostic build.
fn rotation_distance_degrees(a: &[f32; 4], b: &[f32; 4]) -> Option<f32> {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let a_norm: f32 = a.iter().map(|v| v * v).sum();
    let b_norm: f32 = b.iter().map(|v| v * v).sum();
    let cosine = (dot / (a_norm * b_norm).sqrt()).abs().clamp(0.0, 1.0);
    cosine.is_finite().then(|| 2.0 * cosine.acos().to_degrees())
}

unsafe fn observe_phase(owner: Owner, clip: ClipFrame, time: f32, output: *const f32) {
    let rotation = core::array::from_fn(|i| output.add(i).read());
    if let Ok(mut phases) = PHASES.lock() {
        let key = (owner.chassis, clip.action);
        if !phases.contains_key(&key) && phases.len() >= MAX_ONCE_LOGS / 2 {
            return;
        }
        let phase = phases.entry(key).or_insert(PhaseObservation {
            first_rotation: rotation,
            min_time: time,
            max_time: time,
            max_rotation_degrees: 0.0,
            samples: 0,
            first_local_rotation: None,
            local_rotation_degrees: 0.0,
            latest_pose_mismatch_degrees: None,
            post_pose_samples: 0,
            reported: false,
        });
        if phase.reported {
            return;
        }
        phase.min_time = phase.min_time.min(time);
        phase.max_time = phase.max_time.max(time);
        phase.samples += 1;
        if let Some(angle) = rotation_distance_degrees(&phase.first_rotation, &rotation) {
            phase.max_rotation_degrees = phase.max_rotation_degrees.max(angle);
        }
    }
}

unsafe fn post_pose(context: FrameContext, clip: ClipFrame, index: usize) -> Option<[f32; 4]> {
    let (_, geometry) = current_components(context.owner)?;
    let binding: *const u8 = read(geometry, 0x1c8)?;
    if geometry as usize != clip.geometry
        || binding as usize != clip.binding
        || read::<usize>(binding, 0x20)? != clip.animation
    {
        return None;
    }
    let nodes = vector(binding, 0x30, 0x18, MAX_TRACKS)?;
    if index >= (nodes.end - nodes.begin) / 0x18 {
        return None;
    }
    let node: *const u8 = read(nodes.begin as *const u8, index * 0x18)?;
    let getter = method(node, 0x88)?;
    // Validate the live NodeImpl local-rotation getter before reading its field.
    // lea rax,[rcx+0x3dc]; ret. Unknown node implementations remain unobserved.
    if core::slice::from_raw_parts(getter as *const u8, 8)
        != [0x48, 0x8d, 0x81, 0xdc, 0x03, 0x00, 0x00, 0xc3]
    {
        return None;
    }
    let rotation: [f32; 4] = read(node, 0x3dc)?;
    rotation.iter().all(|v| v.is_finite()).then_some(rotation)
}

unsafe fn finish_phase(context: FrameContext) {
    let Some(clip) = context.clip.filter(|clip| clip.valid) else {
        return;
    };
    let Some((index, sampled)) = context.thigh else {
        return;
    };
    let key = (context.owner.chassis, clip.action);
    // Do not read draw objects after this observation has already been reported.
    if !PHASES
        .lock()
        .ok()
        .is_some_and(|phases| phases.get(&key).is_some_and(|p| !p.reported))
    {
        return;
    }
    let local = post_pose(context, clip, index);
    let summary = if let Ok(mut phases) = PHASES.lock() {
        let Some(phase) = phases.get_mut(&key) else {
            return;
        };
        if let Some(local) = local {
            phase.post_pose_samples += 1;
            let first = phase.first_local_rotation.get_or_insert(local);
            if let Some(angle) = rotation_distance_degrees(first, &local) {
                phase.local_rotation_degrees = phase.local_rotation_degrees.max(angle);
            }
            phase.latest_pose_mismatch_degrees = rotation_distance_degrees(&sampled, &local);
        }
        if phase.samples >= 32 || phase.max_time - phase.min_time >= 0.6 {
            phase.reported = true;
            Some((
                phase.min_time,
                phase.max_time,
                phase.max_rotation_degrees,
                phase.samples,
                phase.local_rotation_degrees,
                phase.latest_pose_mismatch_degrees,
                phase.post_pose_samples,
            ))
        } else {
            None
        }
    } else {
        None
    };
    if let Some((min_time, max_time, rotation_span, samples, local_span, mismatch, post_samples)) =
        summary
    {
        let geometry = clip.geometry as *const u8;
        let ik: *const u8 = read(geometry, 0x148).unwrap_or(core::ptr::null());
        let ik_count = vector(ik, 0, 0x10, 64).map_or(0, |v| (v.end - v.begin) / 0x10);
        let duration = ASSETS
            .get()
            .and_then(|a| a.as_ref().ok())
            .map_or(f32::NAN, |a| {
                if clip.kind == 0 {
                    a.throw.stock.duration
                } else {
                    a.switch.stock.duration
                }
            });
        let mismatch = mismatch.map_or_else(|| "unavailable".to_string(), |v| format!("{v:.1}"));
        log(LOG_DEBUG, &format!(
            "moving action leg phase: chassis={:#x} action={:#x} samples={} sample_time_min={:.3} sample_time_max={:.3} duration={:.3} thigh_rotation_span_deg={:.1} post_pose_samples={} local_thigh_span_deg={:.1} latest_pose_mismatch_deg={} binding_time={:.3} looping={} previous_binding={} blend={:.3} ik_solvers={}",
            context.owner.chassis, clip.action, samples, min_time, max_time, duration, rotation_span,
            post_samples, local_span, mismatch,
            read::<f32>(geometry, 0x1d8).unwrap_or(f32::NAN),
            read::<u8>(geometry, 0x1dc).unwrap_or(0),
            read::<usize>(geometry, 0x1e0).unwrap_or(0) != 0,
            read::<f32>(geometry, 0x2ac).unwrap_or(f32::NAN), ik_count,
        ));
    }
}

fn take<const N: usize>(bytes: &[u8], offset: &mut usize) -> Result<[u8; N], String> {
    let end = offset.checked_add(N).ok_or("ANIM offset overflow")?;
    let src = bytes.get(*offset..end).ok_or("truncated ANIM data")?;
    let mut out = [0; N];
    out.copy_from_slice(src);
    *offset = end;
    Ok(out)
}

fn read_u32(bytes: &[u8], offset: &mut usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(take(bytes, offset)?))
}

fn read_f32(bytes: &[u8], offset: &mut usize) -> Result<f32, String> {
    let value = f32::from_bits(read_u32(bytes, offset)?);
    if !value.is_finite() {
        return Err("ANIM contains a non-finite float".into());
    }
    Ok(value)
}

fn parse_clip(bytes: &[u8]) -> Result<Clip, String> {
    if bytes.len() > MAX_CLIP_BYTES || bytes.len() < 16 {
        return Err("ANIM size outside accepted bounds".into());
    }
    let mut offset = 0;
    if take::<4>(bytes, &mut offset)? != *b"ANIM" {
        return Err("ANIM magic mismatch".into());
    }
    if read_u32(bytes, &mut offset)? != 1 {
        return Err("unsupported ANIM version".into());
    }
    let duration = read_f32(bytes, &mut offset)?;
    if duration <= 0.0 {
        return Err("ANIM duration must be positive".into());
    }
    let track_count = read_u32(bytes, &mut offset)? as usize;
    if track_count == 0 || track_count > MAX_TRACKS {
        return Err("ANIM track count outside accepted bounds".into());
    }
    let mut tracks = Vec::with_capacity(track_count);
    let mut total_keys = 0usize;
    for _ in 0..track_count {
        let rest = bytes.get(offset..).ok_or("truncated ANIM name")?;
        let nul = rest
            .iter()
            .position(|b| *b == 0)
            .ok_or("unterminated ANIM track name")?;
        if nul == 0 || nul > 4096 {
            return Err("ANIM track name length outside accepted bounds".into());
        }
        let name = rest[..nul].to_vec();
        offset = offset.checked_add(nul + 1).ok_or("ANIM offset overflow")?;
        let pos_count = read_u32(bytes, &mut offset)? as usize;
        if pos_count == 0 || pos_count > MAX_KEYS_PER_CHANNEL {
            return Err("ANIM position-key count outside accepted bounds".into());
        }
        total_keys = total_keys
            .checked_add(pos_count)
            .ok_or("ANIM key count overflow")?;
        let mut positions = Vec::with_capacity(pos_count);
        for _ in 0..pos_count {
            let key = [
                read_f32(bytes, &mut offset)?,
                read_f32(bytes, &mut offset)?,
                read_f32(bytes, &mut offset)?,
                read_f32(bytes, &mut offset)?,
            ];
            positions.push(key);
        }
        let rot_count = read_u32(bytes, &mut offset)? as usize;
        if rot_count == 0 || rot_count > MAX_KEYS_PER_CHANNEL {
            return Err("ANIM rotation-key count outside accepted bounds".into());
        }
        total_keys = total_keys
            .checked_add(rot_count)
            .ok_or("ANIM key count overflow")?;
        let mut rotations = Vec::with_capacity(rot_count);
        for _ in 0..rot_count {
            let key = [
                read_f32(bytes, &mut offset)?,
                read_f32(bytes, &mut offset)?,
                read_f32(bytes, &mut offset)?,
                read_f32(bytes, &mut offset)?,
                read_f32(bytes, &mut offset)?,
            ];
            rotations.push(key);
        }
        if !strict_times(positions.iter().map(|k| k[0]))
            || !strict_times(rotations.iter().map(|k| k[0]))
            || positions.iter().any(|k| k[0] > duration + 0.0001)
            || rotations.iter().any(|k| k[0] > duration + 0.0001)
        {
            return Err("ANIM key times are not strictly ordered within duration".into());
        }
        if total_keys > 1_000_000 {
            return Err("ANIM total key count outside accepted bounds".into());
        }
        tracks.push(Track {
            name,
            positions,
            rotations,
        });
    }
    if offset != bytes.len() {
        return Err("ANIM has trailing bytes".into());
    }
    for i in 0..tracks.len() {
        if tracks[..i].iter().any(|t| t.name == tracks[i].name) {
            return Err("ANIM contains duplicate track names".into());
        }
    }
    Ok(Clip { duration, tracks })
}

fn strict_times(times: impl Iterator<Item = f32>) -> bool {
    let mut previous = None;
    for time in times {
        if time < 0.0 || previous.is_some_and(|p| time <= p) {
            return false;
        }
        previous = Some(time);
    }
    previous.is_some()
}

const LEG_NAMES: [&[u8]; 8] = [
    b"Bip01_L_Thigh",
    b"Bip01_L_Calf",
    b"Bip01_L_Foot",
    b"Bip01_L_Toe0",
    b"Bip01_R_Thigh",
    b"Bip01_R_Calf",
    b"Bip01_R_Foot",
    b"Bip01_R_Toe0",
];

fn pair(stock_bytes: &[u8], moving_bytes: &[u8]) -> Result<Pair, String> {
    let stock = parse_clip(stock_bytes)?;
    let moving = parse_clip(moving_bytes)?;
    if stock.duration.to_bits() != moving.duration.to_bits()
        || stock.tracks.len() != moving.tracks.len()
    {
        return Err("composite duration/track count differs from its stock clip".into());
    }
    let mut changed = Vec::new();
    for (index, (a, b)) in stock.tracks.iter().zip(&moving.tracks).enumerate() {
        if a.name != b.name
            || a.positions.len() != b.positions.len() && !LEG_NAMES.contains(&a.name.as_slice())
        {
            return Err("composite track names/order differ from stock clip".into());
        }
        let differs = a.positions != b.positions || a.rotations != b.rotations;
        if differs {
            if !LEG_NAMES.contains(&a.name.as_slice())
                || a.positions == b.positions
                || a.rotations == b.rotations
            {
                return Err("composite changes a non-leg track or only one leg channel".into());
            }
            changed.push(index);
        }
    }
    if changed.len() != LEG_NAMES.len()
        || LEG_NAMES
            .iter()
            .any(|name| !changed.iter().any(|i| stock.tracks[*i].name == *name))
    {
        return Err("composite must replace exactly the eight thigh/calf/foot/toe tracks".into());
    }
    Ok(Pair {
        stock,
        moving,
        changed,
    })
}

fn assets() -> Result<AssetSet, String> {
    let root = if let Some(override_dir) = std::env::var_os("MOVING_ACTIONS_ANIMATION_ASSET_DIR") {
        PathBuf::from(override_dir)
    } else {
        let mut path = [0u16; 32768];
        let length = unsafe {
            GetModuleFileNameW(core::ptr::null_mut(), path.as_mut_ptr(), path.len() as u32)
        } as usize;
        if length == 0 || length >= path.len() {
            return Err("cannot resolve game executable path for animation assets".into());
        }
        use std::os::windows::ffi::OsStringExt;
        let executable = PathBuf::from(std::ffi::OsString::from_wide(&path[..length]));
        let game_root = executable
            .parent()
            .and_then(|bin| bin.parent())
            .ok_or("game executable is not under <game>/bin")?;
        game_root
            .join("mods")
            .join("defiance_moving_actions")
            .join("assets")
    };
    let load = |name: &str| -> Result<Vec<u8>, String> {
        let path = root.join(name);
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("cannot read animation asset {}: {error}", path.display()))?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() as usize > MAX_CLIP_BYTES {
            return Err(format!(
                "animation asset {} has invalid size",
                path.display()
            ));
        }
        let data = fs::read(&path)
            .map_err(|error| format!("cannot load animation asset {}: {error}", path.display()))?;
        if data.len() > MAX_CLIP_BYTES {
            return Err(format!(
                "animation asset {} exceeds size limit",
                path.display()
            ));
        }
        Ok(data)
    };
    let stock_throw = load("stock_throw.anim")?;
    let stock_switch = load("stock_switch.anim")?;
    let moving_throw = load("moving_throw.anim")?;
    let moving_switch = load("moving_switch.anim")?;
    let moving_throw_back = load("moving_throw_back.anim")?;
    Ok(AssetSet {
        throw: pair(&stock_throw, &moving_throw)?,
        switch: pair(&stock_switch, &moving_switch)?,
        throw_back: pair(&stock_throw, &moving_throw_back)?,
    })
}

unsafe fn code_pointer(p: usize) -> bool {
    if p == 0 {
        return false;
    }
    // UnitEntity getters live in essence.dll, while the hooked animation code
    // lives in logic.dll. Validate executable memory rather than module identity.
    let mut info = core::mem::MaybeUninit::<MemoryBasicInformation>::uninit();
    if VirtualQuery(
        p as *const c_void,
        info.as_mut_ptr(),
        core::mem::size_of::<MemoryBasicInformation>(),
    ) != core::mem::size_of::<MemoryBasicInformation>()
    {
        return false;
    }
    let info = info.assume_init();
    info.state == 0x1000
        && info.protect & 0x100 == 0
        && matches!(info.protect & 0xff, 0x10 | 0x20 | 0x40 | 0x80)
}

unsafe fn vt(object: *const u8, rva: usize) -> bool {
    read::<usize>(object, 0).is_some_and(|p| p == BASE.load(Ordering::Acquire) + rva)
}

unsafe fn reference(object: *const u8, offset: usize) -> Option<*mut u8> {
    let junction: *const u8 = read(object, offset)?;
    if junction.is_null() {
        None
    } else {
        read(junction, 0x10)
    }
}

unsafe fn method(object: *const u8, slot: usize) -> Option<usize> {
    let table: *const u8 = read(object, 0)?;
    let address: usize = read(table, slot)?;
    (address != 0 && code_pointer(address)).then_some(address)
}

/// Validate only ownership at update entry. Action, posture and moving-route
/// state are tested later, inside each sampler, after the native update runs.
unsafe fn owner(facet: *mut u8) -> Option<Owner> {
    if !active_build().is_some_and(|b| vt(facet, b.human_animation_vt)) {
        return None;
    }
    let unit = reference(facet, 0x10)?;
    let get_components = method(unit, 0xb0)?;
    let get: unsafe extern "C" fn(*mut u8) -> *mut u8 = core::mem::transmute(get_components);
    let components = get(unit);
    if components.is_null() || read::<*mut u8>(components, 0x58)? != facet {
        return None;
    }
    let chassis: *mut u8 = read(components, 0x38)?;
    if !active_build().is_some_and(|b| vt(chassis, b.human_chassis_vt))
        || reference(chassis, 0x10)? != unit
    {
        return None;
    }
    Some(Owner {
        facet: facet as usize,
        unit: unit as usize,
        chassis: chassis as usize,
    })
}

unsafe fn moving_route(chassis: *mut u8) -> bool {
    if read::<u8>(chassis, 0x18) != Some(1)
        || read::<u32>(chassis, 0x28).is_none_or(|flags| flags & 1 == 0)
        || read::<i32>(chassis, 0xec) != Some(1)
    {
        return false;
    }
    let manager: *const u8 = match read::<*const u8>(chassis, 0x30) {
        Some(v) if !v.is_null() => v,
        _ => return false,
    };
    let nav = match read::<i32>(chassis, 0xc4) {
        Some(v) if (0..4096).contains(&v) => v as usize,
        _ => return false,
    };
    let header: *const u8 = match read::<*const u8>(manager, 0x288) {
        Some(v) if !v.is_null() => v,
        _ => return false,
    };
    let count = match read::<i32>(header, 4) {
        Some(v) if (0..=4096).contains(&v) => v as usize,
        _ => return false,
    };
    if nav >= count {
        return false;
    }
    let records: *const u8 = match read::<*const u8>(header, 8) {
        Some(v) if !v.is_null() => v,
        _ => return false,
    };
    let Some(record_address) = (records as usize).checked_add(nav.saturating_mul(0x2f0)) else {
        return false;
    };
    let record = record_address as *const u8;
    read::<*mut u8>(record, 0x258) == Some(chassis)
        && read::<u8>(record, 1) == Some(1)
        && read::<u8>(record, 0x2cc).is_some_and(|state| state == 2 || state == 3)
}

unsafe fn current_components(owner: Owner) -> Option<(*mut u8, *mut u8)> {
    let facet = owner.facet as *mut u8;
    let unit = owner.unit as *mut u8;
    let chassis = owner.chassis as *mut u8;
    if reference(facet, 0x10)? != unit
        || !active_build().is_some_and(|b| vt(chassis, b.human_chassis_vt))
        || reference(chassis, 0x10)? != unit
    {
        return None;
    }
    let get_components = method(unit, 0xb0)?;
    let get: unsafe extern "C" fn(*mut u8) -> *mut u8 = core::mem::transmute(get_components);
    let components = get(unit);
    if components.is_null()
        || read::<*mut u8>(components, 0x58)? != facet
        || read::<*mut u8>(components, 0x38)? != chassis
    {
        return None;
    }
    let geometry: *mut u8 = read(facet, 0xb0)?;
    if geometry.is_null() {
        return None;
    }
    Some((components, geometry))
}

#[derive(Clone, Copy)]
struct Vector {
    begin: usize,
    end: usize,
}

unsafe fn vector(object: *const u8, offset: usize, stride: usize, max: usize) -> Option<Vector> {
    let begin: usize = read(object, offset)?;
    let end: usize = read(object, offset + 8)?;
    let cap: usize = read(object, offset + 16)?;
    if begin == 0 || end < begin || cap < end || stride == 0 {
        return None;
    }
    let bytes = end.checked_sub(begin)?;
    if bytes % stride != 0
        || bytes / stride > max
        || cap.checked_sub(begin)? > max.checked_mul(stride)?
    {
        return None;
    }
    Some(Vector { begin, end })
}

unsafe fn string_bytes(object: *const u8) -> Option<Vec<u8>> {
    let len: usize = read(object, 0x10)?;
    let cap: usize = read(object, 0x18)?;
    if len == 0 || len > 4096 || cap < len {
        return None;
    }
    let source = if cap > 15 {
        read::<*const u8>(object, 0)? as usize
    } else {
        object as usize
    };
    if source == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let byte = read::<u8>(source as *const u8, i)?;
        if byte == 0 {
            return None;
        }
        out.push(byte);
    }
    Some(out)
}

unsafe fn runtime_clip_matches(animation: *const u8, stock: &Clip) -> Option<(usize, usize)> {
    let duration: f32 = read(animation, 0)?;
    if duration.to_bits() != stock.duration.to_bits() {
        return None;
    }
    let tracks = vector(animation, 8, 0x50, MAX_TRACKS)?;
    let count = (tracks.end - tracks.begin) / 0x50;
    if count != stock.tracks.len() {
        return None;
    }
    for (index, expected) in stock.tracks.iter().enumerate() {
        let track_address = tracks.begin.checked_add(index.checked_mul(0x50)?)?;
        let track = track_address as *const u8;
        if string_bytes(track)? != expected.name {
            return None;
        }
        let positions = vector(track, 0x20, 0x10, MAX_KEYS_PER_CHANNEL)?;
        let rotations = vector(track, 0x38, 0x14, MAX_KEYS_PER_CHANNEL)?;
        if (positions.end - positions.begin) / 0x10 != expected.positions.len()
            || (rotations.end - rotations.begin) / 0x14 != expected.rotations.len()
        {
            return None;
        }
        for (i, key) in expected.positions.iter().enumerate() {
            let at = (positions.begin as *const u8).add(i * 0x10);
            for (component, value) in key.iter().enumerate() {
                if !read::<f32>(at, component * 4)?.is_finite()
                    || read::<f32>(at, component * 4)?.to_bits() != value.to_bits()
                {
                    return None;
                }
            }
        }
        for (i, key) in expected.rotations.iter().enumerate() {
            let at = (rotations.begin as *const u8).add(i * 0x14);
            for (component, value) in key.iter().enumerate() {
                if !read::<f32>(at, component * 4)?.is_finite()
                    || read::<f32>(at, component * 4)?.to_bits() != value.to_bits()
                {
                    return None;
                }
            }
        }
    }
    Some((tracks.begin, count))
}

unsafe fn current_clip(owner: Owner, context: &mut FrameContext) -> Option<ClipFrame> {
    let facet = owner.facet as *mut u8;
    let chassis = owner.chassis as *mut u8;
    let action = read::<i32>(facet, 0x6c)?;
    let (kind, pair) = match action {
        0x17 | 0x2b => (0u8, &ASSETS.get()?.as_ref().ok()?.throw),
        0x18 => (1u8, &ASSETS.get()?.as_ref().ok()?.switch),
        _ => return None,
    };
    if read::<i32>(facet, 0x74)? != 1 || !moving_route(chassis) {
        return None;
    }
    let (_, geometry) = current_components(owner)?;
    let binding: *mut u8 = read(geometry, 0x1c8)?;
    if binding.is_null() {
        return None;
    }
    let animation: *mut u8 = read(binding, 0x20)?;
    if animation.is_null() {
        return None;
    }
    if let Some(cached) = context.clip {
        if cached.action == action
            && cached.kind == kind
            && cached.geometry == geometry as usize
            && cached.binding == binding as usize
            && cached.animation == animation as usize
        {
            return cached.valid.then_some(cached);
        }
    }
    let matched = runtime_clip_matches(animation, &pair.stock);
    let clip = ClipFrame {
        action,
        kind,
        geometry: geometry as usize,
        binding: binding as usize,
        animation: animation as usize,
        tracks: matched.map_or(0, |(begin, _)| begin),
        valid: matched.is_some(),
    };
    context.clip = Some(clip);
    clip.valid.then_some(clip)
}

#[derive(Clone, Copy)]
enum Channel {
    Position,
    Rotation,
}

// Blend toward the stock backpedal when the body turns away from travel.
// This preserves the existing forward cycle exactly for aligned directions.
fn backward_weight(facing: [f32; 2], travel: [f32; 2]) -> f32 {
    let dot = facing[0] * travel[0] + facing[1] * travel[1];
    let cross = facing[0] * travel[1] - facing[1] * travel[0];
    let length2 = dot * dot + cross * cross;
    if !length2.is_finite() || length2 < 1e-12 {
        return 0.0;
    }
    (-dot / length2.sqrt() / 0.35).clamp(0.0, 1.0)
}

unsafe fn travel_direction(chassis: *mut u8) -> Option<[f32; 2]> {
    let manager: *const u8 = read(chassis, 0x30)?;
    let header: *const u8 = read(manager, 0x288)?;
    let records: *const u8 = read(header, 8)?;
    let nav = read::<i32>(chassis, 0xc4)?;
    let count = read::<i32>(header, 4)?;
    if nav < 0 || nav >= count || count > 4096 || records.is_null() {
        return None;
    }
    let record = records.add(nav as usize * 0x2f0);
    if read::<*mut u8>(record, 0x258)? != chassis
        || read::<u8>(record, 1)? != 1
        || read::<u8>(record, 0x2cc)? != 2
    {
        return None;
    }
    Some([
        read::<f32>(record, 0x260)? - read::<f32>(record, 0x1e8)?,
        read::<f32>(record, 0x268)? - read::<f32>(record, 0x1f0)?,
    ])
}

// Keep scalar motion observations only; all engine objects/getters are
// revalidated before each snapshot. Stop and action completion clear history.
unsafe fn observed_motion(owner: Owner) -> Option<([f32; 2], u8)> {
    let chassis = owner.chassis as *mut u8;
    let facet = owner.facet as *const u8;
    let throwing = matches!(read::<i32>(facet, 0x6c), Some(0x17 | 0x2b))
        || matches!(read::<i32>(facet, 0x68), Some(0x17 | 0x2b));
    if !throwing || !moving_route(chassis) {
        if let Ok(mut history) = MOTION.lock() {
            history.remove(&owner.chassis);
        }
        return None;
    }
    let (components, _) = current_components(owner)?;
    let position: *mut u8 = read(components, 0)?;
    if position.is_null() || read::<*mut u8>(chassis, 0x48)? != position {
        return None;
    }
    let get: unsafe extern "C" fn(*mut u8) -> *const f32 =
        core::mem::transmute(method(position, 0x58)?);
    let point = get(position);
    if point.is_null() {
        return None;
    }
    let point = [point.read_unaligned(), point.add(1).read_unaligned()];
    if point.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let now = GetTickCount64();
    let manager: usize = read(chassis, 0x30)?;
    let nav: i32 = read(chassis, 0xc4)?;
    let mut history = MOTION.lock().ok()?;
    let previous = history.get(&owner.chassis).copied().filter(|previous| {
        previous.unit == owner.unit
            && previous.facet == owner.facet
            && previous.manager == manager
            && previous.nav == nav
            && now.saturating_sub(previous.time) <= 250
    });
    let mut result = None;
    let mut direction = None;
    if let Some(previous) = previous {
        let delta = [point[0] - previous.point[0], point[1] - previous.point[1]];
        let length2 = delta[0] * delta[0] + delta[1] * delta[1];
        // Reject stationary snapshots and discontinuities such as save loads.
        if length2.is_finite() && (1e-8..64.0).contains(&length2) {
            direction = Some((delta, now));
            result = Some((delta, 0));
        } else if length2 < 1e-8 {
            direction = previous
                .direction
                .filter(|(_, time)| now.saturating_sub(*time) <= 250);
            result = direction.map(|(value, _)| (value, 1));
        }
    }
    if history.len() >= 256 && !history.contains_key(&owner.chassis) {
        if let Some(oldest) = history
            .iter()
            .min_by_key(|(_, sample)| sample.time)
            .map(|(key, _)| *key)
        {
            history.remove(&oldest);
        }
    }
    history.insert(
        owner.chassis,
        MotionSample {
            unit: owner.unit,
            facet: owner.facet,
            manager,
            nav,
            point,
            time: now,
            direction,
        },
    );
    result
}

unsafe fn gait_amount(owner: Owner, context: &FrameContext, action: i32) -> f32 {
    let chassis = owner.chassis as *mut u8;
    let (travel, source) = context
        .motion
        .unwrap_or_else(|| travel_direction(chassis).map_or(([0.0; 2], 3), |v| (v, 2)));
    let facing = [
        read(chassis, 0x84).unwrap_or(0.0),
        read(chassis, 0x88).unwrap_or(0.0),
    ];
    let weight = backward_weight(facing, travel);
    let bucket = if weight >= 0.99 {
        2
    } else if weight > 0.01 {
        1
    } else {
        0
    };
    let report = GAIT_REPORTS.lock().is_ok_and(|mut reports| {
        reports.len() < 64 && reports.insert((owner.chassis, source * 3 + bucket))
    });
    if report {
        let source = [
            "displacement",
            "held-displacement",
            "waypoint",
            "unavailable",
        ][source as usize];
        log(LOG_DEBUG, &format!(
            "moving throw gait: chassis={:#x} action={action:#x} source={source} backward_weight={weight:.3} facing=({:.3},{:.3}) travel=({:.3},{:.3})",
            owner.chassis, facing[0], facing[1], travel[0], travel[1],
        ));
    }
    weight
}

unsafe fn overlay(
    descriptor: *const u8,
    output: *mut f32,
    time: f32,
    owner: Owner,
    context: &mut FrameContext,
    channel: Channel,
) -> bool {
    if descriptor.is_null() || output.is_null() || !time.is_finite() {
        return false;
    }
    let Some(clip) = current_clip(owner, context) else {
        return false;
    };
    let Some(assets) = ASSETS.get().and_then(|result| result.as_ref().ok()) else {
        return false;
    };
    let Some(bindings) = active_build() else {
        return false;
    };
    let pair = match clip.kind {
        0 => &assets.throw,
        1 => &assets.switch,
        _ => return false,
    };
    for index in &pair.changed {
        let track_address = match clip.tracks.checked_add(index.saturating_mul(0x50)) {
            Some(v) => v,
            None => return false,
        };
        let field = match channel {
            Channel::Position => 0x20,
            Channel::Rotation => 0x38,
        };
        if descriptor as usize != track_address + field {
            continue;
        }
        let amount = if clip.kind == 0 {
            gait_amount(owner, context, clip.action)
        } else {
            0.0
        };
        let changed_track = &pair.moving.tracks[*index];
        match channel {
            Channel::Position => sample_position(&changed_track.positions, output, time),
            Channel::Rotation => sample_rotation(&changed_track.rotations, output, time),
        }
        if amount > 0.0 {
            let other = &assets.throw_back.moving.tracks[*index];
            let mut sample = [0.0f32; 4];
            match channel {
                Channel::Position => {
                    sample_position(&other.positions, sample.as_mut_ptr(), time);
                    for (i, value) in sample.iter().enumerate().take(3) {
                        let a = output.add(i).read();
                        output.add(i).write(a + (*value - a) * amount);
                    }
                }
                Channel::Rotation => {
                    sample_rotation(&other.rotations, sample.as_mut_ptr(), time);
                    let left = core::array::from_fn::<_, 4, _>(|i| output.add(i).read());
                    let slerp: Slerp =
                        core::mem::transmute(BASE.load(Ordering::Acquire) + bindings.slerp);
                    let _ = slerp(left.as_ptr(), output, sample.as_ptr(), amount);
                }
            }
        }
        log_first_application(owner.chassis, clip.action, channel, time);
        if matches!(channel, Channel::Rotation) && changed_track.name == b"Bip01_L_Thigh" {
            let rotation = core::array::from_fn(|i| output.add(i).read());
            context.thigh = Some((*index, rotation));
            observe_phase(owner, clip, time, output);
        }
        return true;
    }
    false
}

fn bracket<T>(keys: &[T], time: f32, key_time: impl Fn(&T) -> f32) -> (usize, usize, f32) {
    if keys.len() == 1 || time <= key_time(&keys[0]) {
        return (0, 0, 0.0);
    }
    let last = keys.len() - 1;
    if time >= key_time(&keys[last]) {
        return (last, last, 0.0);
    }
    let mut low = 0usize;
    let mut high = keys.len();
    while low < high {
        let mid = low + (high - low) / 2;
        if key_time(&keys[mid]) <= time {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    let right = low;
    let left = right - 1;
    let span = key_time(&keys[right]) - key_time(&keys[left]);
    if span <= 0.0 {
        (right, right, 0.0)
    } else {
        (
            left,
            right,
            ((time - key_time(&keys[left])) / span).clamp(0.0, 1.0),
        )
    }
}

fn sample_position(keys: &[[f32; 4]], output: *mut f32, time: f32) {
    let (left, right, amount) = bracket(keys, time, |k| k[0]);
    for component in 0..3 {
        let a = keys[left][component + 1];
        let b = keys[right][component + 1];
        unsafe {
            output.add(component).write(a + (b - a) * amount);
        }
    }
}

fn sample_rotation(keys: &[[f32; 5]], output: *mut f32, time: f32) {
    let (left, right, amount) = bracket(keys, time, |k| k[0]);
    if left == right {
        for component in 0..4 {
            unsafe {
                output.add(component).write(keys[left][component + 1]);
            }
        }
        return;
    }
    let Some(bindings) = active_build() else {
        return;
    };
    let helper = BASE.load(Ordering::Acquire) + bindings.slerp;
    let callback: Slerp = unsafe { core::mem::transmute(helper) };
    unsafe {
        let _ = callback(
            keys[left].as_ptr().add(1),
            output,
            keys[right].as_ptr().add(1),
            amount,
        );
    }
}

unsafe extern "C" fn update(facet: *mut u8, dt: f32) {
    let callback: Update = core::mem::transmute(original(0));
    let frame = owner(facet).map(|owner| FrameContext {
        owner,
        clip: None,
        thigh: None,
        motion: observed_motion(owner),
    });
    let previous = FRAME.with(|slot| slot.replace(frame));
    callback(facet, dt);
    let completed = FRAME.with(|slot| slot.replace(previous));
    if let Some(context) = completed {
        finish_phase(context);
    }
}

unsafe extern "C" fn augment_position(
    descriptor: *const u8,
    output: *mut f32,
    time: f32,
    result: *mut f32,
) {
    if !result.is_null() {
        FRAME.with(|slot| {
            if let Some(context) = slot.borrow_mut().as_mut() {
                let owner = context.owner;
                let _ = overlay(descriptor, output, time, owner, context, Channel::Position);
            }
        });
    }
}

unsafe extern "C" fn augment_rotation(
    descriptor: *const u8,
    output: *mut f32,
    time: f32,
    result: *mut f32,
) {
    if !result.is_null() {
        FRAME.with(|slot| {
            if let Some(context) = slot.borrow_mut().as_mut() {
                let owner = context.owner;
                let _ = overlay(descriptor, output, time, owner, context, Channel::Rotation);
            }
        });
    }
}

struct BuildInfo {
    base: usize,
    size: usize,
    bindings: BuildBindings,
}

unsafe fn validate(api: &Api) -> Result<BuildInfo, String> {
    let base = (api.module_base)(c"logic.dll".as_ptr()) as usize;
    if base == 0 {
        return Err("logic.dll is not loaded".into());
    }
    let size = (api.module_size)(base as *mut c_void);
    let mut path = [0u16; 32768];
    let length =
        GetModuleFileNameW(base as *mut c_void, path.as_mut_ptr(), path.len() as u32) as usize;
    if length == 0 || length >= path.len() {
        return Err("cannot resolve logic.dll path".into());
    }
    use std::os::windows::ffi::OsStringExt;
    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&path[..length]));
    let hash =
        defiance_core::sha256::file(&path).map_err(|e| format!("cannot hash logic.dll: {e}"))?;
    let bindings = BUILDS
        .iter()
        .find(|build| build.sha256 == hash)
        .copied()
        .ok_or_else(|| format!("unsupported logic.dll SHA-256 {hash}"))?;
    for (rva, prefix, name) in [
        (bindings.update, UPDATE_PREFIX, "update"),
        (bindings.position, POSITION_PREFIX, "position sampler"),
        (bindings.rotation, ROTATION_PREFIX, "rotation sampler"),
        (
            bindings.slerp,
            SLERP_PREFIX,
            "quaternion interpolation helper",
        ),
    ] {
        let end = rva
            .checked_add(prefix.len())
            .ok_or("binding range overflow")?;
        if end > size || base.checked_add(end).is_none() {
            return Err(format!("{name} at {rva:#x} outside logic.dll"));
        }
        if core::slice::from_raw_parts((base + rva) as *const u8, prefix.len()) != prefix {
            return Err(format!("{name} byte check failed at RVA {rva:#x}"));
        }
    }
    Ok(BuildInfo {
        base,
        size,
        bindings,
    })
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else {
        return 1;
    };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    LOGGER.store(api.log as usize, Ordering::Release);
    let result = (|| -> Result<(), String> {
        let build = validate(api)?;
        ASSETS.get_or_init(assets).as_ref().map_err(Clone::clone)?;
        BASE.store(build.base, Ordering::Release);
        SIZE.store(build.size, Ordering::Release);
        let bindings = build.bindings;
        let build_index = BUILDS
            .iter()
            .position(|entry| entry.sha256 == bindings.sha256)
            .ok_or("validated bindings are not in the build catalog")?;
        ACTIVE_BUILD.store(build_index + 1, Ordering::Release);
        let sites = [bindings.update, bindings.position, bindings.rotation];
        for (index, rva, detour) in [
            (0usize, bindings.update, update as *mut c_void),
            (1, bindings.position, position as *mut c_void),
            (2, bindings.rotation, rotation as *mut c_void),
        ] {
            let mut trampoline = core::ptr::null_mut();
            let hook_result =
                (api.hook)((build.base + rva) as *mut c_void, detour, &mut trampoline);
            if hook_result != 0 || trampoline.is_null() {
                // A nonzero hook result is atomic and does not transfer
                // ownership of the requested site; only earlier successful
                // installs belong to this plugin. Success with a null
                // trampoline is malformed, but does transfer ownership, so
                // include that current site in rollback.
                let rollback_end = if hook_result == 0 {
                    Some(index)
                } else {
                    index.checked_sub(1)
                };
                if let Some(first_owned) = rollback_end {
                    for installed in (0..=first_owned).rev() {
                        (api.unhook)((build.base + sites[installed]) as *mut c_void);
                        ORIGINALS[installed].store(0, Ordering::Release);
                    }
                }
                BASE.store(0, Ordering::Release);
                SIZE.store(0, Ordering::Release);
                ACTIVE_BUILD.store(0, Ordering::Release);
                return Err(format!("hook refused at RVA {rva:#x}"));
            }
            ORIGINALS[index].store(trampoline as usize, Ordering::Release);
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            log(
                LOG_INFO,
                "moving action animation overlay installed (GOG/Steam 2026-09)",
            );
            0
        }
        Err(error) => {
            log(
                LOG_ERROR,
                &format!("moving action animation overlay refused: {error}"),
            );
            1
        }
    }
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.moving-actions-animation".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_asset_limit_matches_parser_limit() {
        assert_eq!(MAX_CLIP_BYTES, 16 * 1024 * 1024);
    }

    #[test]
    fn parser_rejects_truncation_and_extra_data() {
        assert!(parse_clip(&[0; 15]).is_err());
    }

    #[test]
    fn sampling_clamps_and_interpolates_positions() {
        let keys = [[0.0, 2.0, 4.0, -2.0], [2.0, 6.0, 8.0, 2.0]];
        let mut out = [0.0; 3];
        sample_position(&keys, out.as_mut_ptr(), -1.0);
        assert_eq!(out, [2.0, 4.0, -2.0]);
        sample_position(&keys, out.as_mut_ptr(), 1.0);
        assert_eq!(out, [4.0, 6.0, 0.0]);
        sample_position(&keys, out.as_mut_ptr(), 3.0);
        assert_eq!(out, [6.0, 8.0, 2.0]);
    }

    #[test]
    fn gait_direction_blends_forward_backward_and_rejects_bad_vectors() {
        let facing = [1.0, 0.0];
        assert_eq!(backward_weight(facing, [1.0, 0.0]), 0.0);
        assert_eq!(backward_weight(facing, [-1.0, 0.0]), 1.0);
        assert_eq!(backward_weight(facing, [0.0, 1.0]), 0.0);
        assert_eq!(backward_weight(facing, [0.0, -1.0]), 0.0);
        assert_eq!(backward_weight(facing, [0.0, 0.0]), 0.0);
        assert_eq!(backward_weight([f32::NAN, 0.0], facing), 0.0);
    }
}
