//! Inspect the firing gun and its actual render-node binding, not just the
//! model-vector descriptor observed at switch completion.
use core::ffi::{c_char, c_void};
use defiance_api::{Api, Plugin, ABI_VERSION, LOG_DEBUG, LOG_ERROR, LOG_INFO, LOG_WARN};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

const WORLD_SHA: &str = "c39827bec79c0c4e1358259b5a2b3a6762e9270c6f5e1f25ce32d3f95b6a15c2";
const PRIMARY_SHOT_BYTES: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x18, 0x55, 0x56, 0x57, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8b, 0xec, 0x48,
    0x81, 0xec, 0x80, 0x00, 0x00, 0x00, 0x0f, 0x29, 0x74, 0x24, 0x70, 0x0f, 0x29, 0x7c, 0x24, 0x60,
];
const GUNNER_TICK_BYTES: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x48, 0x89, 0x7c, 0x24, 0x20, 0x55,
    0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8b, 0xec, 0x48, 0x83, 0xec, 0x60, 0x0f,
];
const CLIENT_TICK_BYTES: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x6c, 0x24, 0x18, 0x48, 0x89, 0x74, 0x24, 0x20, 0x57,
    0x41, 0x56, 0x41, 0x57, 0x48, 0x83, 0xec, 0x60, 0x0f, 0x29, 0x74, 0x24, 0x50, 0x0f, 0x29, 0x7c,
];
const ATTACH: usize = 0x154d40;
const DETACH: usize = 0x1551a0;
#[derive(Clone, Copy)]
struct Build {
    sha: &'static str,
    shot: usize,
    primary_shot: usize,
    gunner_tick: usize,
    client_tick: usize,
    rebind: usize,
    handoff: usize,
    animation_vt: usize,
    gunner_vt: usize,
    gunner_client_vt: usize,
}
// Generated and cross-checked against .pdata and exact MSVC RTTI class names.
// The GOG and Steam releases are close in time, but their layouts have no
// global RVA delta; bind every supported SHA independently.
const BUILDS: [Build; 4] = [
    Build {
        sha: "adb3ad95926036809b4e554b466bef33d4ac7aa5303e59a9e4a940890bc334b5",
        shot: 0x29ae10,
        primary_shot: 0x29a690,
        gunner_tick: 0x2d7b00,
        client_tick: 0x2dca80,
        rebind: 0x4327b0,
        handoff: 0x2d8590,
        animation_vt: 0x72c118,
        gunner_vt: 0x72cd00,
        gunner_client_vt: 0x72ced0,
    },
    Build {
        sha: "30264904e1d5199b954bafbd7828cf7190930c246d35fa7b94eefa915e8f0c38",
        shot: 0x29ae10,
        primary_shot: 0x29a690,
        gunner_tick: 0x2d7b00,
        client_tick: 0x2dca80,
        rebind: 0x432170,
        handoff: 0x2d8590,
        animation_vt: 0x72b118,
        gunner_vt: 0x72bd00,
        gunner_client_vt: 0x72bed0,
    },
    Build {
        sha: "1216d627c7288c7db6940168363be582232ed4d3cb860b8b8c8d7489652eca74",
        shot: 0x29ad80,
        primary_shot: 0x29a600,
        gunner_tick: 0x2d7a70,
        client_tick: 0x2dc9f0,
        rebind: 0x432720,
        handoff: 0x2d8500,
        animation_vt: 0x72c0d8,
        gunner_vt: 0x72ccc0,
        gunner_client_vt: 0x72ce78,
    },
    Build {
        sha: "eb8674f1d16595a3e9cf6a9ec0062735b1184976495d8d6ade36f7e2574e8aab",
        shot: 0x29ad80,
        primary_shot: 0x29a600,
        gunner_tick: 0x2d7a70,
        client_tick: 0x2dc9f0,
        rebind: 0x4320e0,
        handoff: 0x2d8500,
        animation_vt: 0x72b0d8,
        gunner_vt: 0x72bcc0,
        gunner_client_vt: 0x72be78,
    },
];
const SHOT_BYTES: &[u8] = &[
    0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10, 0x48, 0x89, 0x70, 0x20, 0x55, 0x57, 0x41, 0x56, 0x48,
    0x8d, 0x68, 0xa1, 0x48, 0x81, 0xec, 0x90, 0x00, 0x00, 0x00, 0x0f, 0x29, 0x70, 0xd8, 0x0f, 0x29,
];
const REBIND_BYTES: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x55, 0x57, 0x41, 0x56, 0x48, 0x8b,
    0xec, 0x48, 0x83, 0xec, 0x60, 0x48, 0x8b, 0xf1, 0x33, 0xff, 0x4c, 0x8b, 0x81, 0x08, 0x01, 0x00,
];
const ATTACH_BYTES: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x18, 0x48, 0x89, 0x6c, 0x24, 0x20, 0x48, 0x89, 0x54, 0x24, 0x10, 0x56,
    0x57, 0x41, 0x54, 0x41, 0x56, 0x41, 0x57, 0x48, 0x83, 0xec, 0x50, 0x4c, 0x8b, 0xfa, 0x48, 0x8b,
];
const DETACH_BYTES: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x10, 0x57, 0x48, 0x83, 0xec, 0x40, 0x4c, 0x8b, 0xc1, 0x48, 0x8b, 0x91,
    0x70, 0x03, 0x00, 0x00, 0x48, 0x85, 0xd2, 0x0f, 0x84, 0xe0, 0x00, 0x00, 0x00, 0x83, 0x7a, 0x08,
];
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_PRIMARY_SHOT: AtomicUsize = AtomicUsize::new(0);
static LOGGER: AtomicUsize = AtomicUsize::new(0);
static LOGIC: AtomicUsize = AtomicUsize::new(0);
static WORLD: AtomicUsize = AtomicUsize::new(0);
static SHOT_RVA: AtomicUsize = AtomicUsize::new(0);
static PRIMARY_SHOT_RVA: AtomicUsize = AtomicUsize::new(0);
static GUNNER_TICK_RVA: AtomicUsize = AtomicUsize::new(0);
static CLIENT_TICK_RVA: AtomicUsize = AtomicUsize::new(0);
static REBIND_RVA: AtomicUsize = AtomicUsize::new(0);
static HANDOFF_RVA: AtomicUsize = AtomicUsize::new(0);
static ANIMATION_VT_RVA: AtomicUsize = AtomicUsize::new(0);
static GUNNER_VT_RVA: AtomicUsize = AtomicUsize::new(0);
static GUNNER_CLIENT_VT_RVA: AtomicUsize = AtomicUsize::new(0);
static ANOMALIES: AtomicUsize = AtomicUsize::new(0);
static SKIPS: [AtomicUsize; 13] = [const { AtomicUsize::new(0) }; 13];
static ORIGINAL_TICK: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_CLIENT_TICK: AtomicUsize = AtomicUsize::new(0);
static WATCHES: LazyLock<Mutex<HashMap<usize, Watch>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
#[derive(Default)]
struct Watch {
    elapsed: f32,
    samples: usize,
    signature: [usize; 8],
    repair_attempts: usize,
}
type Shot = unsafe extern "C" fn(*mut u8);
type Tick = unsafe extern "C" fn(*mut u8, u8, u8, u32, f32);
type Rebind = unsafe extern "C" fn(*mut u8, *const u8);
type Log = unsafe extern "C" fn(u32, *const c_char);

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, capacity: u32) -> u32;
}

unsafe fn read<T: Copy>(p: *const u8, offset: usize) -> T {
    p.add(offset).cast::<T>().read_unaligned()
}
unsafe fn method(p: *const u8, slot: usize) -> usize {
    if p.is_null() {
        return 0;
    }
    let vt: *const u8 = read(p, 0);
    if vt.is_null() {
        0
    } else {
        read(vt, slot)
    }
}
unsafe fn reference(p: *const u8, offset: usize) -> *mut u8 {
    let junction: *const u8 = read(p, offset);
    if junction.is_null() {
        core::ptr::null_mut()
    } else {
        read(junction, 0x10)
    }
}
unsafe fn log(level: u32, value: &str) {
    let logger = LOGGER.load(Ordering::Acquire);
    if logger != 0 {
        if let Ok(text) = std::ffi::CString::new(value) {
            let callback: Log = core::mem::transmute(logger);
            callback(level, text.as_ptr());
        }
    }
}
unsafe fn model_name(descriptor: *const u8) -> String {
    if descriptor.is_null() {
        return "?".into();
    }
    let length = read::<usize>(descriptor, 0x1b8);
    let capacity = read::<usize>(descriptor, 0x1c0);
    if length == 0 || length > 192 || length > capacity {
        return "?".into();
    }
    let text = if capacity > 15 {
        read::<*const u8>(descriptor, 0x1a8)
    } else {
        descriptor.add(0x1a8)
    };
    if text.is_null() {
        return "?".into();
    }
    String::from_utf8_lossy(core::slice::from_raw_parts(text, length)).replace(['\r', '\n'], " ")
}

struct State {
    unit: *mut u8,
    gunner: *mut u8,
    geometry: *mut u8,
    animation: *const u8,
    descriptor: *const u8,
    selected: *mut u8,
    front: *const u8,
    node: *mut u8,
    hands: *mut u8,
    binding_known: bool,
    parent: *mut u8,
    hand_child: *mut u8,
}

struct Skip {
    stage: usize,
    reason: &'static str,
    observed: usize,
}
unsafe fn snapshot(gun: *mut u8) -> Result<State, Skip> {
    let skip = |stage, reason, observed| {
        Err(Skip {
            stage,
            reason,
            observed,
        })
    };
    let logic = LOGIC.load(Ordering::Acquire);
    let world = WORLD.load(Ordering::Acquire);
    let mut unit = reference(gun, 0x20);
    if unit.is_null() {
        unit = reference(gun, 0x18);
    }
    let getter = method(unit, 0xb0);
    if getter == 0 {
        return skip(0, "owner-components-getter", unit as usize);
    }
    let get: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(getter);
    let components = get(unit);
    if components.is_null() {
        return skip(1, "owner-components-null", unit as usize);
    }
    let animation: *const u8 = read(components, 0x58);
    if animation.is_null()
        || read::<usize>(animation, 0) != logic + ANIMATION_VT_RVA.load(Ordering::Acquire)
    {
        return skip(
            2,
            "animation-type",
            if animation.is_null() {
                0
            } else {
                read(animation, 0)
            },
        );
    }
    let ai: *const u8 = read(components, 0x28);
    if ai.is_null() {
        return skip(3, "ai-null", components as usize);
    }
    let gunner = reference(ai, 0x1f0);
    if gunner.is_null()
        || ![
            logic + GUNNER_VT_RVA.load(Ordering::Acquire),
            logic + GUNNER_CLIENT_VT_RVA.load(Ordering::Acquire),
        ]
        .contains(&read::<usize>(gunner, 0))
    {
        return skip(
            4,
            "gunner-type",
            if gunner.is_null() { 0 } else { read(gunner, 0) },
        );
    }
    let guns: *const u8 = read(gunner, 0x38);
    let end = read::<usize>(gunner, 0x40);
    let index = read::<i32>(gunner, 0x94);
    if guns.is_null() || end < guns as usize || index < 0 {
        return skip(5, "weapon-list-index", index as usize);
    }
    let count = (end - guns as usize) / 8;
    if count > 256
        || index as usize >= count
        || !(0..count).any(|i| read::<*mut u8>(guns, i * 8) == gun)
    {
        return skip(6, "gun-not-in-owner-list", count);
    }
    let selected: *mut u8 = read(guns, index as usize * 8);
    let descriptor_method = method(gun, 0x160);
    if descriptor_method == 0 {
        return skip(7, "descriptor-getter", gun as usize);
    }
    let getter: unsafe extern "C" fn(*mut u8) -> *const u8 =
        core::mem::transmute(descriptor_method);
    let descriptor = getter(gun);
    if descriptor.is_null() || read::<usize>(descriptor, 0x1b8) == 0 {
        return skip(8, "descriptor-model-empty", descriptor as usize);
    }
    let geometry: *mut u8 = read(components, 8);
    if geometry.is_null() {
        return skip(9, "geometry-null", components as usize);
    }
    let models: *const u8 = read(geometry, 0x108);
    let end = read::<usize>(geometry, 0x110);
    if models.is_null() || end <= models as usize {
        return skip(10, "models-empty", models as usize);
    }
    let bytes = end - models as usize;
    if !bytes.is_multiple_of(0xb0) || bytes / 0xb0 > 256 {
        return skip(11, "models-layout", bytes);
    }
    let Some(row) = (0..bytes / 0xb0).find(|&i| read::<*const u8>(models, i * 0xb0) == descriptor)
    else {
        return skip(12, "descriptor-not-in-models", descriptor as usize);
    };
    let node: *mut u8 = read(models, row * 0xb0 + 0x20);
    let hands: *mut u8 = read(geometry, 0x158);
    let life: *const u8 = read(geometry, 0x160);
    let binding_known = !node.is_null()
        && !hands.is_null()
        && !life.is_null()
        && read::<i32>(life, 8) > 0
        && method(node, 0x208) == world + DETACH
        && method(hands, 0x1f8) == world + ATTACH;
    Ok(State {
        unit,
        gunner,
        geometry,
        animation,
        descriptor,
        selected,
        front: read(models, 0),
        node,
        hands,
        binding_known,
        parent: if binding_known {
            read(node, 0x368)
        } else {
            core::ptr::null_mut()
        },
        hand_child: if binding_known {
            read(hands, 0x378)
        } else {
            core::ptr::null_mut()
        },
    })
}

unsafe extern "C" fn shot(gun: *mut u8) {
    fire(&ORIGINAL, gun, SHOT_RVA.load(Ordering::Acquire));
}
unsafe extern "C" fn primary_shot(gun: *mut u8) {
    fire(
        &ORIGINAL_PRIMARY_SHOT,
        gun,
        PRIMARY_SHOT_RVA.load(Ordering::Acquire),
    );
}
unsafe fn fire(storage: &AtomicUsize, gun: *mut u8, entry: usize) {
    let address = loop {
        let p = storage.load(Ordering::Acquire);
        if p != 0 {
            break p;
        }
        core::hint::spin_loop();
    };
    let original: Shot = core::mem::transmute(address);
    let before = snapshot(gun);
    let mut rebound = false;
    if let Ok(state) = before.as_ref() {
        // Only the selected human firearm is eligible. Grenade/reload/change
        // animations intentionally alter the attachment and must not be forced.
        let action = read::<i32>(state.animation, 0x6c);
        let category = read::<i32>(state.descriptor, 0xc4);
        if state.selected == gun
            && read::<i32>(state.gunner, 0x90) == 2
            && matches!(category, 1 | 2)
            && matches!(action, 1 | 3 | 4)
            && state.binding_known
            && (state.front != state.descriptor
                || state.parent != state.hands
                || state.hand_child != state.node)
        {
            let rebind: Rebind = core::mem::transmute(
                LOGIC.load(Ordering::Acquire) + REBIND_RVA.load(Ordering::Acquire),
            );
            rebind(state.geometry, state.descriptor);
            rebound = true;
        }
    }
    let rounds = read::<i32>(gun, 0xdc);
    original(gun);
    let state = match before {
        Ok(state) => state,
        Err(skip) => {
            if SKIPS[skip.stage].fetch_add(1, Ordering::Relaxed) < 8 {
                log(LOG_DEBUG, &format!("moving weapon render skipped: gun={gun:p} reason={} observed={:#x} logic={:#x} rounds={rounds}->{}", skip.reason, skip.observed, LOGIC.load(Ordering::Acquire), read::<i32>(gun, 0xdc)));
            }
            return;
        }
    };
    let after = snapshot(gun);
    let mismatch = state.selected != gun
        || state.front != state.descriptor
        || (state.binding_known && (state.parent != state.hands || state.hand_child != state.node));
    if (mismatch || rebound) && ANOMALIES.fetch_add(1, Ordering::Relaxed) < 300 {
        let repaired = after.as_ref().is_ok_and(|s| {
            s.binding_known
                && s.front == s.descriptor
                && s.parent == s.hands
                && s.hand_child == s.node
        });
        log(LOG_DEBUG, &format!("moving weapon mismatch: entry={entry:#x} unit={:p} gunner={:p} firing={gun:p} model={} descriptor={:p} selected={:p} front={:p} node={:p} hands={:p} parent={:p}->{:#x} hands_child={:p} binding_known={} rebound={rebound} repaired={} rounds={rounds}->{} state={} action={:#x} target={:#x} gun_mode={}",
            state.unit,state.gunner,model_name(state.descriptor),state.descriptor,state.selected,state.front,state.node,state.hands,state.parent,
            after.as_ref().map_or(0,|s|s.parent as usize),state.hand_child,state.binding_known,rebound&&repaired,
            read::<i32>(gun,0xdc),read::<i32>(state.gunner,0x90),read::<i32>(state.animation,0x6c),
            reference(gun,0xc8) as usize,read::<i32>(gun,0x140)));
    }
}

// Read-only: inspect an already loaded mismatch without requiring a shot or
// any particular posture. The original update and all five arguments survive.
unsafe fn observe(gunner: *mut u8, dt: f32) {
    let Ok(mut watches) = WATCHES.lock() else {
        return;
    };
    if watches.len() >= 512 && !watches.contains_key(&(gunner as usize)) {
        return;
    }
    let watch = watches.entry(gunner as usize).or_default();
    if dt.is_finite() && dt > 0.0 {
        watch.elapsed += dt;
    }
    if watch.samples > 0 && watch.elapsed < 1.0 {
        return;
    }
    watch.elapsed = 0.0;
    let guns: *const u8 = read(gunner, 0x38);
    let end = read::<usize>(gunner, 0x40);
    let index = read::<i32>(gunner, 0x94);
    if guns.is_null() || end < guns as usize || !(end - guns as usize).is_multiple_of(8) {
        return;
    }
    let count = (end - guns as usize) / 8;
    if count > 256 || index < 0 || index as usize >= count {
        return;
    }
    let gun: *mut u8 = read(guns, index as usize * 8);
    if gun.is_null() {
        return;
    }
    match snapshot(gun) {
        Ok(s) => {
            let signature = [
                gun as usize,
                s.front as usize,
                s.node as usize,
                s.parent as usize,
                s.hand_child as usize,
                read::<i32>(gunner, 0x90) as usize,
                read::<i32>(s.animation, 0x6c) as usize,
                read::<i32>(s.animation, 0x68) as usize,
            ];
            if signature[0] != watch.signature[0] {
                watch.repair_attempts = 0;
            }
            let stale = s.front != s.descriptor || s.parent != s.hands || s.hand_child != s.node;
            let mismatch = s.front != s.descriptor
                || (s.binding_known && (s.parent != s.hands || s.hand_child != s.node));
            let changed = watch.samples == 0 || signature != watch.signature;
            let repair_ready = s.gunner == gunner
                && s.binding_known
                && stale
                && matches!(signature[5], 2 | 6)
                && matches!(signature[6], 1 | 3)
                && signature[6] == signature[7]
                && read::<f32>(gunner, 0x9c).is_finite()
                && read::<f32>(gunner, 0x9c) <= 0.0;
            let repair = watch.samples > 0
                && signature[..5] == watch.signature[..5]
                && watch.repair_attempts < 3
                && repair_ready;
            if !repair
                && !mismatch
                && (watch.samples >= 16 || (watch.samples >= 2 && signature == watch.signature))
            {
                return;
            }
            if repair {
                watch.repair_attempts += 1;
            }
            watch.signature = signature;
            watch.samples = watch.samples.saturating_add(1);
            // Drop the lock before logging/calling virtual getters.
            drop(watches);
            if repair {
                repair_loaded(&s);
            } else if mismatch && changed && ANOMALIES.fetch_add(1, Ordering::Relaxed) < 300 {
                log(LOG_DEBUG, &format!("moving weapon mismatch observed: unit={:p} gunner={gunner:p} selected={gun:p} index={index} model={} descriptor={:p} front={:p} node={:p} hands={:p} parent={:p} hands_child={:p} binding_known={} state={} action={:#x} requested={:#x} repair_ready={repair_ready} shot_rva={:#x}", s.unit,model_name(s.descriptor),s.descriptor,s.front,s.node,s.hands,s.parent,s.hand_child,s.binding_known,signature[5],signature[6],signature[7],method(gun,0xe0).wrapping_sub(LOGIC.load(Ordering::Acquire))));
            }
        }
        Err(skip) => {
            if watch.samples >= 2 {
                return;
            }
            watch.samples += 1;
            drop(watches);
            log(LOG_DEBUG, &format!("moving weapon watch skipped: gunner={gunner:p} unit={:p} gun={gun:p} reason={} observed={:#x} logic={:#x}",read::<*const u8>(gunner,0x20),skip.reason,skip.observed,LOGIC.load(Ordering::Acquire)));
        }
    }
}
unsafe fn repair_loaded(s: &State) {
    let logic = LOGIC.load(Ordering::Acquire);
    let timer = read::<f32>(s.gunner, 0x9c);
    let script = read::<usize>(s.animation, 0xb8);
    if s.front != s.descriptor {
        // Execute the engine's complete model AND animation-script handoff.
        // The selected index was validated and the stock window is (0,1.5).
        s.gunner.add(0x9c).cast::<f32>().write_unaligned(1.0);
        let handoff: unsafe extern "C" fn(*mut u8, f32) -> bool =
            core::mem::transmute(logic + HANDOFF_RVA.load(Ordering::Acquire));
        let _ = handoff(s.gunner, 0.0);
        s.gunner.add(0x9c).cast::<f32>().write_unaligned(timer);
    } else {
        let rebind: Rebind = core::mem::transmute(logic + REBIND_RVA.load(Ordering::Acquire));
        rebind(s.geometry, s.descriptor);
    }
    let after = snapshot(s.selected);
    let repaired = after.as_ref().is_ok_and(|a| {
        a.binding_known
            && a.front == a.descriptor
            && a.parent == a.hands
            && a.hand_child == a.node
            && a.selected == s.selected
    });
    log(if repaired { LOG_DEBUG } else { LOG_WARN }, &format!("moving weapon loaded repair: gunner={:p} unit={:p} selected={:p} model={} old_model={} repaired={repaired} timer={timer:.3}->{:.3} script={script:#x}->{:#x}",s.gunner,s.unit,s.selected,model_name(s.descriptor),model_name(s.front),read::<f32>(s.gunner,0x9c),read::<usize>(s.animation,0xb8)));
}
unsafe fn update(original: &AtomicUsize, gunner: *mut u8, a: u8, b: u8, c: u32, dt: f32) {
    let address = loop {
        let p = original.load(Ordering::Acquire);
        if p != 0 {
            break p;
        }
        core::hint::spin_loop();
    };
    let tick: Tick = core::mem::transmute(address);
    tick(gunner, a, b, c, dt);
    observe(gunner, dt);
}
unsafe extern "C" fn tick(gunner: *mut u8, a: u8, b: u8, c: u32, dt: f32) {
    update(&ORIGINAL_TICK, gunner, a, b, c, dt);
}
unsafe extern "C" fn client_tick(gunner: *mut u8, a: u8, b: u8, c: u32, dt: f32) {
    update(&ORIGINAL_CLIENT_TICK, gunner, a, b, c, dt);
}

unsafe fn checked_module(
    api: &Api,
    name: &core::ffi::CStr,
    sha: &str,
    windows: &[(usize, &[u8])],
) -> Result<usize, String> {
    let base = (api.module_base)(name.as_ptr());
    if base.is_null() {
        return Err(format!("{} missing", name.to_string_lossy()));
    }
    let size = (api.module_size)(base);
    if windows.iter().any(|(rva, bytes)| rva + bytes.len() > size) {
        return Err("module too small".into());
    }
    let mut path = [0u16; 32768];
    let length = GetModuleFileNameW(base, path.as_mut_ptr(), path.len() as u32) as usize;
    if length == 0 || length >= path.len() {
        return Err("module path unavailable".into());
    }
    use std::os::windows::ffi::OsStringExt;
    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&path[..length]));
    if defiance_core::sha256::file(&path).map_err(|e| e.to_string())? != sha {
        return Err(format!("unsupported {} build", name.to_string_lossy()));
    }
    for (rva, bytes) in windows {
        if core::slice::from_raw_parts(base.cast::<u8>().add(*rva), bytes.len()) != *bytes {
            return Err(format!(
                "{} helper {rva:#x} differs",
                name.to_string_lossy()
            ));
        }
    }
    Ok(base as usize)
}

unsafe fn checked_logic(api: &Api) -> Result<(usize, Build), String> {
    let name = c"logic.dll";
    let base = (api.module_base)(name.as_ptr());
    if base.is_null() {
        return Err("logic.dll missing".into());
    }
    let size = (api.module_size)(base);
    let mut path = [0u16; 32768];
    let length = GetModuleFileNameW(base, path.as_mut_ptr(), path.len() as u32) as usize;
    if length == 0 || length >= path.len() {
        return Err("logic.dll path unavailable".into());
    }
    use std::os::windows::ffi::OsStringExt;
    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&path[..length]));
    let sha = defiance_core::sha256::file(&path).map_err(|e| e.to_string())?;
    let build = BUILDS
        .iter()
        .copied()
        .find(|build| build.sha == sha)
        .ok_or_else(|| "unsupported logic.dll build".to_string())?;
    let windows = [
        (build.shot, SHOT_BYTES),
        (build.primary_shot, PRIMARY_SHOT_BYTES),
        (build.rebind, REBIND_BYTES),
        (build.gunner_tick, GUNNER_TICK_BYTES),
        (build.client_tick, CLIENT_TICK_BYTES),
    ];
    if windows.iter().any(|(rva, bytes)| rva + bytes.len() > size) {
        return Err("logic.dll is too small for its known build".into());
    }
    for (rva, bytes) in windows {
        if core::slice::from_raw_parts(base.cast::<u8>().add(rva), bytes.len()) != bytes {
            return Err(format!("logic.dll helper at {rva:#x} differs"));
        }
    }
    Ok((base as usize, build))
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
        let (logic, build) = checked_logic(api)?;
        let world = checked_module(
            api,
            c"world2.dll",
            WORLD_SHA,
            &[(ATTACH, ATTACH_BYTES), (DETACH, DETACH_BYTES)],
        )?;
        LOGIC.store(logic, Ordering::Release);
        WORLD.store(world, Ordering::Release);
        SHOT_RVA.store(build.shot, Ordering::Release);
        PRIMARY_SHOT_RVA.store(build.primary_shot, Ordering::Release);
        GUNNER_TICK_RVA.store(build.gunner_tick, Ordering::Release);
        CLIENT_TICK_RVA.store(build.client_tick, Ordering::Release);
        REBIND_RVA.store(build.rebind, Ordering::Release);
        HANDOFF_RVA.store(build.handoff, Ordering::Release);
        ANIMATION_VT_RVA.store(build.animation_vt, Ordering::Release);
        GUNNER_VT_RVA.store(build.gunner_vt, Ordering::Release);
        GUNNER_CLIENT_VT_RVA.store(build.gunner_client_vt, Ordering::Release);
        ANOMALIES.store(0, Ordering::Relaxed);
        for count in &SKIPS {
            count.store(0, Ordering::Relaxed);
        }
        let mut original = core::ptr::null_mut();
        if (api.hook)(
            (logic + build.shot) as *mut c_void,
            shot as Shot as *mut c_void,
            &mut original,
        ) != 0
            || original.is_null()
        {
            return Err("shot hook refused".into());
        }
        ORIGINAL.store(original as usize, Ordering::Release);
        let mut original = core::ptr::null_mut();
        if (api.hook)(
            (logic + build.primary_shot) as *mut c_void,
            primary_shot as Shot as *mut c_void,
            &mut original,
        ) != 0
            || original.is_null()
        {
            return Err("primary shot hook refused".into());
        }
        ORIGINAL_PRIMARY_SHOT.store(original as usize, Ordering::Release);
        if let Ok(mut watches) = WATCHES.lock() {
            watches.clear();
        }
        for (rva, detour, storage) in [
            (build.gunner_tick, tick as Tick, &ORIGINAL_TICK),
            (
                build.client_tick,
                client_tick as Tick,
                &ORIGINAL_CLIENT_TICK,
            ),
        ] {
            let mut original = core::ptr::null_mut();
            if (api.hook)(
                (logic + rva) as *mut c_void,
                detour as *mut c_void,
                &mut original,
            ) != 0
                || original.is_null()
            {
                return Err("gunner observer hook refused".into());
            }
            storage.store(original as usize, Ordering::Release);
        }
        log(
            LOG_INFO,
            &format!(
                "moving weapon render installed: logic.dll sha256={}",
                &build.sha[..16]
            ),
        );
        Ok(())
    })();
    match result {
        Ok(()) => 0,
        Err(error) => {
            let callback: Log = api.log;
            if let Ok(text) =
                std::ffi::CString::new(format!("moving weapon render refused: {error}"))
            {
                callback(LOG_ERROR, text.as_ptr());
            }
            1
        }
    }
}
#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.moving-actions-render-sync".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}
defiance_feature_sdk::crash_handshake!();
