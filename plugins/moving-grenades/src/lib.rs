//! Resume the native movement request only after a hand grenade is confirmed.
use core::cell::Cell;
use core::ffi::{c_char, c_void};
use defiance_api::{Api, Plugin, ABI_VERSION, LOG_DEBUG, LOG_ERROR, LOG_INFO};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

mod sites;
struct Build {
    name: &'static str,
    sha: &'static str,
    mapping: &'static [(usize, usize)],
    checks: &'static [(usize, &'static [u8])],
}
impl Build {
    fn rva(&self, reference: usize) -> usize {
        let index = self
            .mapping
            .binary_search_by_key(&reference, |&(rva, _)| rva)
            .expect("generated native binding missing");
        self.mapping[index].1
    }
}
static BUILD_INDEX: AtomicUsize = AtomicUsize::new(usize::MAX);
const SITES: [usize; 16] = [
    0x2cabb0, 0x2cc9e0, 0x2ccaf0, 0x2c28f0, 0xd9470, 0x9e960, 0x2d8350, 0x299330, 0x2996e0,
    0x2ccbe0, 0x2d9640, 0x2cc990, 0x333239, 0x2bafa0, 0x103250, 0x2bc400,
];
static BASE: AtomicUsize = AtomicUsize::new(0);
static SIZE: AtomicUsize = AtomicUsize::new(0);
static LOGGER: AtomicUsize = AtomicUsize::new(0);
static ORIGINALS: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
static NATIVE_FAILURE_REPORTS: AtomicUsize = AtomicUsize::new(0);
static FACING_REPORTS: LazyLock<Mutex<HashSet<(usize, bool)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static ROUTES: LazyLock<Mutex<HashMap<usize, Route>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static SELECTOR_REPORTS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static CANDIDATE_GATE_REPORTS: AtomicUsize = AtomicUsize::new(0);
static FLARE_REPORTS: AtomicUsize = AtomicUsize::new(0);
// Diagnostic identity only: never retain or dereference native target pointers.
static FLARE_ROUTES: LazyLock<Mutex<HashMap<usize, FlareRoute>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
// Own one native strong/weak junction reference per intercepted ability.
// Diagnostic contexts remain separate and cannot reissue an order.
static FLARE_ORDERS: LazyLock<Mutex<HashMap<usize, FlareOrder>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static FLARE_ORDER_SWEEP: AtomicUsize = AtomicUsize::new(0);
static FLARE_COMPLETIONS: LazyLock<Mutex<HashMap<usize, FlareCompletion>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
thread_local! { static REPLACING: Cell<usize> = const { Cell::new(0) }; }
thread_local! { static COMPLETING: Cell<usize> = const { Cell::new(0) }; }
#[derive(Clone, Copy)]
struct SelectionTrace {
    gun: usize,
    eligible: i32,
    idle: i32,
}
#[derive(Clone, Copy)]
struct SelectorDiagnostic {
    selected: i32,
    grenade_mask: u64,
    grenade_count: usize,
    native: u8,
    trace: SelectionTrace,
    overridden: bool,
}
thread_local! { static SELECTING: Cell<SelectionTrace> = const { Cell::new(SelectionTrace { gun: 0, eligible: -1, idle: -1 }) }; }

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, capacity: u32) -> u32;
    fn GetTickCount64() -> u64;
    fn VirtualQuery(
        address: *const c_void,
        info: *mut MemoryBasicInformation,
        length: usize,
    ) -> usize;
    fn RtlCaptureStackBackTrace(
        skip: u32,
        count: u32,
        stack: *mut *mut c_void,
        hash: *mut u32,
    ) -> u16;
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
type Stop = unsafe extern "C" fn(*mut u8, u8);
type Turn = unsafe extern "C" fn(*mut u8, *const f32);
type Queue = unsafe extern "C" fn(*mut u8, i32);
type Order = unsafe extern "C" fn(*mut u8, *const *const u8);
type Submit = unsafe extern "C" fn(*mut u8, *mut *mut u8, f32);
type ClearTarget = unsafe extern "C" fn(*mut u8);
type AiUpdate = unsafe extern "C" fn(*mut u8, f32);
type ReleaseRef = unsafe extern "C" fn(*mut *mut u8);
type CopyRef = unsafe extern "C" fn(*mut *mut u8, *const *mut u8) -> *mut *mut u8;
type Resume = unsafe extern "C" fn(*mut u8, *mut *mut u8, f32, *const f32) -> u8;
type Select = unsafe extern "C" fn(*mut u8) -> u8;
type Candidate = unsafe extern "C" fn(*mut u8, usize, u32, u32) -> u64;
// Gun vfuncs pack a boolean in AL; preserve all original return bits.
type WeaponCheck = unsafe extern "C" fn(*mut u8) -> u64;
type ChassisCanMove = unsafe extern "C" fn(*mut u8) -> u64;

#[derive(Clone, Copy)]
struct CandidateGate {
    chassis: usize,
    gunner: usize,
    index: usize,
    target: usize,
}
thread_local! { static CANDIDATE_GATE: Cell<Option<CandidateGate>> = const { Cell::new(None) }; }

extern "C" {
    #[link_name = "defiance_grenade_steer"]
    fn steer();
    #[link_name = "defiance_grenade_cancel_trace"]
    fn cancel_trace();
}

#[derive(Clone, Copy)]
struct Route {
    time: u64,
    unit: usize,
    manager: usize,
    nav: i32,
    move_target: usize,
    attack_target: usize,
    speed: f32,
    end_direction: Option<[f32; 2]>,
}
#[derive(Clone, Copy)]
struct FlareRoute {
    route: Route,
    gunner: usize,
    kind: u32,
    forgotten: bool,
}
struct FlareOrder {
    route: Route,
    chassis: usize,
    junction: usize,
    gunner: usize,
    kind: u32,
    resumed: bool,
    attack_move: bool,
}
struct FlareCompletion {
    order: FlareOrder,
    stop_junction: usize,
    finished: bool,
}
impl Drop for FlareCompletion {
    fn drop(&mut self) {
        unsafe {
            let release: ReleaseRef = core::mem::transmute(address(0x24070));
            let mut junction = self.stop_junction as *mut u8;
            release(&mut junction);
        }
    }
}
impl Drop for FlareOrder {
    fn drop(&mut self) {
        if self.junction != 0 {
            unsafe {
                let release: ReleaseRef = core::mem::transmute(address(0x24070));
                let mut junction = self.junction as *mut u8;
                release(&mut junction);
            }
        }
    }
}
struct Soldier {
    unit: *mut u8,
    ai: *mut u8,
    components: *mut u8,
}
struct Gunner {
    object: *mut u8,
    target: *mut u8,
    grenade: bool,
}

unsafe fn read<T: Copy>(p: *const u8, offset: usize) -> T {
    p.add(offset).cast::<T>().read_unaligned()
}
unsafe fn write<T>(p: *mut u8, offset: usize, value: T) {
    p.add(offset).cast::<T>().write_unaligned(value);
}
fn address(rva: usize) -> usize {
    BASE.load(Ordering::Acquire) + sites::BUILDS[BUILD_INDEX.load(Ordering::Acquire)].rva(rva)
}
unsafe fn vt(p: *const u8, rva: usize) -> bool {
    !p.is_null() && read::<usize>(p, 0) == address(rva)
}
unsafe fn reference(p: *const u8, offset: usize) -> *mut u8 {
    if p.is_null() {
        return core::ptr::null_mut();
    }
    let j: *const u8 = read(p, offset);
    if j.is_null() {
        core::ptr::null_mut()
    } else {
        read(j, 0x10)
    }
}
unsafe fn method(p: *const u8, slot: usize) -> usize {
    if p.is_null() {
        return 0;
    }
    let table: *const u8 = read(p, 0);
    if table.is_null() {
        0
    } else {
        read(table, slot)
    }
}
unsafe fn executable(p: usize) -> bool {
    if p == 0 {
        return false;
    }
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
fn original(index: usize) -> usize {
    loop {
        let a = ORIGINALS[index].load(Ordering::Acquire);
        if a != 0 {
            return a;
        }
        core::hint::spin_loop();
    }
}
unsafe fn log(level: u32, text: &str) {
    let logger = LOGGER.load(Ordering::Acquire);
    if logger != 0 {
        if let Ok(s) = std::ffi::CString::new(text) {
            let callback: unsafe extern "C" fn(u32, *const c_char) = core::mem::transmute(logger);
            callback(level, s.as_ptr());
        }
    }
}
unsafe fn forget(chassis: *mut u8) {
    trace_flare(chassis, "cache-forget-before", core::ptr::null_mut());
    if let Ok(mut routes) = ROUTES.lock() {
        routes.remove(&(chassis as usize));
    }
    forget_flare_order(chassis);
    if let Ok(mut routes) = FLARE_ROUTES.lock() {
        if let Some(context) = routes.get_mut(&(chassis as usize)) {
            // Cancellation diagnostics outlive the gameplay cache. This map
            // never participates in movement decisions or retains native refs.
            context.forgotten = true;
        }
    }
}
fn forget_flare_order(chassis: *mut u8) {
    // Drop native references after unlocking: destruction can call back into AI.
    let removed = FLARE_ORDERS.lock().ok().and_then(|mut orders| {
        let key = orders
            .iter()
            .find(|(_, o)| o.chassis == chassis as usize)
            .map(|(key, _)| *key)?;
        orders.remove(&key)
    });
    drop(removed);
    if !COMPLETING.with(|v| v.get() == chassis as usize) {
        let removed = FLARE_COMPLETIONS.lock().ok().and_then(|mut orders| {
            let key = orders
                .iter()
                .find(|(_, o)| o.order.chassis == chassis as usize)
                .map(|(key, _)| *key)?;
            orders.remove(&key)
        });
        drop(removed);
    }
}
unsafe fn sweep_flare_orders() {
    let now = GetTickCount64() as usize;
    let last = FLARE_ORDER_SWEEP.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 1000
        || FLARE_ORDER_SWEEP
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return;
    }
    let expired: Vec<_> = if let Ok(mut orders) = FLARE_ORDERS.lock() {
        let keys: Vec<_> = orders
            .iter()
            .filter(|(_, o)| (now as u64).saturating_sub(o.route.time) > 8000)
            .map(|(key, _)| *key)
            .collect();
        keys.into_iter()
            .filter_map(|key| orders.remove(&key))
            .collect()
    } else {
        Vec::new()
    };
    drop(expired);
    let expired: Vec<_> = if let Ok(mut orders) = FLARE_COMPLETIONS.lock() {
        let keys: Vec<_> = orders
            .iter()
            .filter(|(_, o)| (now as u64).saturating_sub(o.order.route.time) > 8000)
            .map(|(key, _)| *key)
            .collect();
        keys.into_iter()
            .filter_map(|key| orders.remove(&key))
            .collect()
    } else {
        Vec::new()
    };
    drop(expired);
}
unsafe fn retain_flare_order(
    s: &Soldier,
    chassis: *mut u8,
    state: *mut u8,
    route: Route,
    attack_move: bool,
) {
    let target = route.attack_target as *const u8;
    if !flare_target(target) || !matches!(read::<u32>(target, 0x28), 0x100 | 0x4000) {
        return;
    }
    let old = reference(state, 0x20);
    let g = reference(s.ai, 0x1f0);
    if reference(old, 0x18) != s.unit
        // A second positional ability must not replay the first ability as if
        // it were an ordinary attack-move order.
        || (attack_move && flare_target(reference(old, 0x28)))
        || (!vt(g, 0x72cd00) && !vt(g, 0x72ced0))
        || read::<*mut u8>(g, 0x20) != s.unit
        || method(s.ai, 0x60) != address(0x2bafa0)
    {
        return;
    }
    let copy: CopyRef = core::mem::transmute(address(0x123f0));
    let mut junction = core::ptr::null_mut();
    copy(&mut junction, state.add(0x20).cast());
    let pending = FlareOrder {
        route,
        chassis: chassis as usize,
        junction: junction as usize,
        gunner: g as usize,
        kind: read(target, 0x28),
        resumed: false,
        attack_move,
    };
    let removed = if let Ok(mut orders) = FLARE_ORDERS.lock() {
        let evicted = if orders.len() >= 128 && !orders.contains_key(&(s.ai as usize)) {
            let key = orders
                .iter()
                .min_by_key(|(_, o)| o.route.time)
                .map(|(key, _)| *key);
            key.and_then(|key| orders.remove(&key))
        } else {
            None
        };
        let previous = orders.insert(s.ai as usize, pending);
        (evicted, previous)
    } else {
        (Some(pending), None)
    };
    drop(removed);
}
unsafe fn release_order_call() -> bool {
    let mut frames = [core::ptr::null_mut(); 12];
    let n = RtlCaptureStackBackTrace(0, 12, frames.as_mut_ptr(), core::ptr::null_mut());
    let base = BASE.load(Ordering::Acquire);
    let size = SIZE.load(Ordering::Acquire);
    frames[..n as usize]
        .iter()
        .find(|p| {
            (**p as usize)
                .checked_sub(base)
                .is_some_and(|rva| rva < size)
        })
        .is_some_and(|p| *p as usize == address(0x2d913a))
}
unsafe extern "C" fn submit(ai: *mut u8, incoming: *mut *mut u8, delay: f32) {
    sweep_flare_orders();
    let cancelled = FLARE_COMPLETIONS
        .lock()
        .ok()
        .and_then(|mut orders| orders.remove(&(ai as usize)));
    drop(cancelled);
    // Every new submitted order consumes any previous continuation. Only the
    // exact gunner-release Stop may transfer ownership of the saved native order.
    let pending = FLARE_ORDERS
        .lock()
        .ok()
        .and_then(|mut orders| orders.remove(&(ai as usize)));
    if let Some(pending) = pending {
        let replacement = (|| {
            if incoming.is_null()
                || delay != 0.0
                || !pending.resumed
                || GetTickCount64().saturating_sub(pending.route.time) > 8000
                || !release_order_call()
                || read::<u8>(ai, 0x18) == 0
                || read::<u8>(ai, 0x130) != 0
                || read::<usize>(ai, 0x1e0) != 0
            {
                return None;
            }
            let next = reference(incoming.cast(), 0);
            if !vt(next, 0x70e5d8) {
                return None;
            }
            let unit = reference(next, 0x18);
            if unit.is_null() || unit as usize != pending.route.unit {
                return None;
            }
            let getter = method(unit, 0xb0);
            if !executable(getter) {
                return None;
            }
            let get: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(getter);
            let components = get(unit);
            if components.is_null() || read::<*mut u8>(components, 0x28) != ai {
                return None;
            }
            let chassis: *mut u8 = read(components, 0x38);
            if chassis as usize != pending.chassis
                || !soldier(chassis).is_some_and(|s| s.unit == unit && s.ai == ai)
                || !moving_route(chassis)
                || read::<usize>(chassis, 0x30) != pending.route.manager
                || read::<i32>(chassis, 0xc4) != pending.route.nav
                || read::<usize>(chassis, 0xd8) != pending.route.move_target
            {
                return None;
            }
            let g = reference(ai, 0x1f0);
            if g as usize != pending.gunner
                || (!vt(g, 0x72cd00) && !vt(g, 0x72ced0))
                || read::<*mut u8>(g, 0x20) != unit
                || method(g, 0x58) != address(0x2d9dc0)
                || read::<i32>(g, 0x90) != 5
                || reference(g, 0x88) as usize != pending.route.attack_target
            {
                return None;
            }
            let target = reference(g, 0x88);
            if !flare_target(target) || read::<u32>(target, 0x28) != pending.kind {
                return None;
            }
            // This junction owns a native reference. Its object is read through
            // the live junction, never through a cached unowned object pointer.
            let old = reference((&pending.junction as *const usize).cast(), 0);
            if !vt(
                old,
                if pending.attack_move {
                    0x705d50
                } else {
                    0x70c068
                },
            ) || reference(old, 0x18) != unit
                || (pending.attack_move && flare_target(reference(old, 0x28)))
            {
                return None;
            }
            Some((chassis, g))
        })();
        if let Some((chassis, _)) = replacement {
            // The real Stop retires the active attack state. Substituting Move
            // here leaves that state able to reinstall the same smoke target.
            // Own both references until Stop initialization and the enclosing
            // native AI update have completed; then submit the saved movement.
            let copy: CopyRef = core::mem::transmute(address(0x123f0));
            let mut stop_junction = core::ptr::null_mut();
            copy(&mut stop_junction, incoming.cast());
            let completion = FlareCompletion {
                order: pending,
                stop_junction: stop_junction as usize,
                finished: false,
            };
            if let Ok(mut orders) = FLARE_COMPLETIONS.lock() {
                let evicted = if orders.len() >= 128 && !orders.contains_key(&(ai as usize)) {
                    let key = orders
                        .iter()
                        .min_by_key(|(_, o)| o.order.route.time)
                        .map(|(key, _)| *key);
                    key.and_then(|key| orders.remove(&key))
                } else {
                    None
                };
                let previous = orders.insert(ai as usize, completion);
                drop(orders);
                drop(previous);
                drop(evicted);
                let previous = COMPLETING.with(|v| v.replace(chassis as usize));
                let callback: Submit = core::mem::transmute(original(13));
                callback(ai, incoming, delay);
                COMPLETING.with(|v| v.set(previous));
                return;
            }
        } else {
            drop(pending);
        }
    }
    let callback: Submit = core::mem::transmute(original(13));
    callback(ai, incoming, delay);
}
unsafe extern "C" fn stop_order_init(order: *mut u8) {
    let unit = reference(order, 0x18);
    let getter = method(unit, 0xb0);
    let ai: *mut u8 = if executable(getter) {
        let get: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(getter);
        let components = get(unit);
        if components.is_null() {
            core::ptr::null_mut()
        } else {
            read(components, 0x28)
        }
    } else {
        core::ptr::null_mut()
    };
    let chassis = FLARE_COMPLETIONS.lock().ok().and_then(|orders| {
        let saved = orders.get(&(ai as usize))?;
        (vt(order, 0x70e5d8)
            && reference((&saved.stop_junction as *const usize).cast(), 0) == order
            && saved.order.route.unit == unit as usize)
            .then_some(saved.order.chassis)
    });
    let previous = COMPLETING.with(|v| v.replace(chassis.unwrap_or(0)));
    let callback: ClearTarget = core::mem::transmute(original(14));
    callback(order);
    COMPLETING.with(|v| v.set(previous));
    if let Some(chassis) = chassis {
        if let Ok(mut orders) = FLARE_COMPLETIONS.lock() {
            if let Some(saved) = orders.get_mut(&(ai as usize)) {
                if saved.order.chassis == chassis
                    && reference((&saved.stop_junction as *const usize).cast(), 0) == order
                {
                    // Native +0x90 marks the Stop finished at the end of init.
                    saved.finished = read::<u8>(order, 0x11) != 0;
                }
            }
        }
    }
}
unsafe extern "C" fn ai_update(ai: *mut u8, dt: f32) {
    sweep_flare_orders();
    let callback: AiUpdate = core::mem::transmute(original(15));
    callback(ai, dt);
    let completion = FLARE_COMPLETIONS.lock().ok().and_then(|mut orders| {
        if orders.get(&(ai as usize)).is_some_and(|o| o.finished) {
            orders.remove(&(ai as usize))
        } else {
            None
        }
    });
    let Some(completion) = completion else {
        return;
    };
    let pending = &completion.order;
    let chassis = pending.chassis as *mut u8;
    // The callback provides a live AI. Resolve the live unit's current chassis
    // before dereferencing a cached component address.
    let unit = reference(ai, 0x10);
    let getter = method(unit, 0xb0);
    if unit as usize != pending.route.unit || !executable(getter) {
        return;
    }
    let get: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(getter);
    let components = get(unit);
    if components.is_null()
        || read::<*mut u8>(components, 0x28) != ai
        || read::<*mut u8>(components, 0x38) != chassis
        || !soldier(chassis).is_some_and(|s| s.ai == ai && s.unit == unit)
        || read::<u8>(ai, 0x18) == 0
        || read::<u8>(ai, 0x130) != 0
        || read::<usize>(ai, 0x1e0) != 0
        || read::<usize>(chassis, 0x30) != pending.route.manager
        || read::<i32>(chassis, 0xc4) != pending.route.nav
        || read::<usize>(chassis, 0xd8) != pending.route.move_target
        || reference(chassis, 0xd8).is_null()
        || read::<i32>(chassis, 0xec) != 0
        || !reserved_route(chassis).is_some_and(|r| read::<u8>(r, 0x2cc) == 0)
        || GetTickCount64().saturating_sub(pending.route.time) > 8000
    {
        return;
    }
    let g = reference(ai, 0x1f0);
    if g as usize != pending.gunner
        || (!vt(g, 0x72cd00) && !vt(g, 0x72ced0))
        || read::<*mut u8>(g, 0x20) != unit
        || !reference(g, 0x88).is_null()
    {
        return;
    }
    let old = reference((&pending.junction as *const usize).cast(), 0);
    if !vt(
        old,
        if pending.attack_move {
            0x705d50
        } else {
            0x70c068
        },
    ) || reference(old, 0x18) != unit
        || (pending.attack_move && flare_target(reference(old, 0x28)))
    {
        return;
    }
    let saved_speed = pending.route.speed;
    let kind = pending.kind;
    let attack_move = pending.attack_move;
    let old_finished = read::<u8>(old, 0x11) != 0;
    let Some(mut resumed) = fresh_movement_order(old, unit, attack_move) else {
        log(
            LOG_ERROR,
            "moving flare continuation rejected: native movement order could not be rebuilt",
        );
        return;
    };
    // Use the original Submit: no recursive cancellation of this continuation.
    let submit: Submit = core::mem::transmute(original(13));
    submit(ai, &mut resumed, 0.0);
    log(LOG_DEBUG, &format!("moving flare order handoff: chassis={chassis:p} kind={kind:#x} attack_move={attack_move} native_stop_finished=true target_cleared=true fresh_order=true old_finished={old_finished} saved_speed={saved_speed:.3} restored_speed={:.3}", read::<f32>(chassis, 0x12c)));
}
unsafe fn fresh_movement_order(old: *mut u8, unit: *mut u8, attack_move: bool) -> Option<*mut u8> {
    let getter = method(unit, 0x80);
    if !executable(getter) {
        return None;
    }
    let get_repo: unsafe extern "C" fn(*mut u8) -> *mut u8 = core::mem::transmute(getter);
    let repo = get_repo(unit);
    if repo.is_null() {
        return None;
    }
    // Native interruption finishes the old order. MoveState checks its done
    // flag before initialization and otherwise runs only the final turn. Build
    // a new order through the same factories as player commands; retain intent,
    // never copy execution status, context references or retry counters.
    let fresh = if attack_move {
        if reference(old, 0x28).is_null() {
            return None;
        }
        let factory: unsafe extern "C" fn(*mut u8, *mut u8, *const *mut u8) -> *mut u8 =
            core::mem::transmute(address(0x8a0c0));
        factory(repo, unit, old.add(0x28).cast())
    } else {
        let destination = reference(old, 0x48);
        if read::<i32>(old, 0x50) != 0 || destination.is_null() {
            return None;
        }
        let factory: unsafe extern "C" fn(*mut u8, *mut u8, u32, *mut u8) -> *mut u8 =
            core::mem::transmute(address(0x114fb0));
        factory(repo, unit, read(old, 0x60), destination)
    };
    if fresh.is_null() {
        return None;
    }
    // Player-command submission converts the PersistentBase object into an
    // owned junction via 240c0, then acquires its auxiliary reference.
    let bind: unsafe extern "C" fn(*mut *mut u8, *mut u8) -> *mut *mut u8 =
        core::mem::transmute(address(0x240c0));
    let mut junction = core::ptr::null_mut();
    bind(&mut junction, fresh);
    if junction.is_null() {
        return None;
    }
    if !reference((&junction as *const *mut u8).cast(), 0).is_null() {
        write(junction, 0x18, read::<i32>(junction, 0x18) + 1);
    }
    if !vt(fresh, if attack_move { 0x705d50 } else { 0x70c068 })
        || reference(fresh, 0x18) != unit
        || read::<u8>(fresh, 0x11) != 0
        || method(fresh, 0x68) != address(0x762f0)
    {
        let release: ReleaseRef = core::mem::transmute(address(0x24070));
        release(&mut junction);
        return None;
    }
    let set_flags: unsafe extern "C" fn(*mut u8, u32) = core::mem::transmute(address(0x762f0));
    set_flags(fresh, read(old, 0x14));
    if !attack_move {
        write(fresh, 0x54, read::<[f32; 2]>(old, 0x54));
        write(fresh, 0x5c, read::<u8>(old, 0x5c));
        write(fresh, 0x68, read::<u8>(old, 0x68));
    }
    Some(junction)
}
// FlareTarget has a different layout from AttackTarget: vfunc5 returns +0x10.
unsafe fn flare_target(target: *const u8) -> bool {
    vt(target, 0x70f430) && method(target, 0x28) == address(0x116d80)
}
unsafe fn trace_flare(chassis: *mut u8, event: &str, target: *mut u8) {
    if FLARE_REPORTS.load(Ordering::Relaxed) >= 96 {
        return;
    }
    let Some(s) = soldier(chassis) else {
        return;
    };
    let object = reference(s.ai, 0x1f0);
    if !vt(object, 0x72cd00) && !vt(object, 0x72ced0) {
        return;
    }
    let target = if target.is_null() {
        reference(object, 0x88)
    } else {
        target
    };
    if read::<*mut u8>(object, 0x20) != s.unit {
        return;
    }
    let retained = FLARE_ROUTES
        .lock()
        .ok()
        .and_then(|routes| routes.get(&(chassis as usize)).copied())
        .filter(|r| {
            GetTickCount64().saturating_sub(r.route.time) <= 8000
                && r.route.unit == s.unit as usize
                && r.gunner == object as usize
        });
    let (kind, source) = if flare_target(target) {
        (read::<u32>(target, 0x28), "live")
    } else {
        let Some(r) = retained else {
            return;
        };
        (r.kind, "retained")
    };
    let animation: *const u8 = read(chassis, 0x58);
    if !vt(animation, 0x72c118) || reference(animation, 0x10) != s.unit {
        return;
    }
    if FLARE_REPORTS.fetch_add(1, Ordering::Relaxed) >= 96 {
        return;
    }
    let saved = ROUTES
        .lock()
        .ok()
        .and_then(|routes| routes.get(&(chassis as usize)).copied());
    let route_state = reserved_route(chassis).map_or(-1, |r| i32::from(read::<u8>(r, 0x2cc)));
    let mut frames = [core::ptr::null_mut(); 16];
    let n = RtlCaptureStackBackTrace(0, 16, frames.as_mut_ptr(), core::ptr::null_mut());
    let base = BASE.load(Ordering::Acquire);
    let size = SIZE.load(Ordering::Acquire);
    let stack: Vec<_> = frames[..n as usize]
        .iter()
        .filter_map(|p| {
            let rva = (*p as usize).checked_sub(base)?;
            (rva < size).then(|| format!("{rva:#x}"))
        })
        .collect();
    log(LOG_DEBUG, &format!(
        "moving flare route trace: event={event} chassis={chassis:p} target={target:p} kind={kind:#x} source={source} context_age={} context_forgotten={} identity_match={} mode={} nav={} route_state={route_state} saved={} saved_age={} saved_target={:#x} action6c={:#x} action68={:#x} posture={} gunner_state={} selected={} stack={}",
        retained.map_or(0, |r| GetTickCount64().saturating_sub(r.route.time)), retained.is_some_and(|r| r.forgotten),
        retained.is_some_and(|r| r.route.manager == read::<usize>(chassis, 0x30) && r.route.nav == read::<i32>(chassis, 0xc4) && r.route.move_target == read::<usize>(chassis, 0xd8)),
        read::<i32>(chassis, 0xec), read::<i32>(chassis, 0xc4),
        saved.is_some(), saved.map_or(0, |r| GetTickCount64().saturating_sub(r.time)), saved.map_or(0, |r| r.attack_target),
        read::<i32>(animation, 0x6c), read::<i32>(animation, 0x68), read::<i32>(animation, 0x74),
        read::<i32>(object, 0x90), read::<i32>(object, 0x94), stack.join(","),
    ));
}
unsafe fn trace_flare_order(state: *mut u8, incoming: *const *const u8, event: &str) {
    if incoming.is_null() {
        return;
    }
    let next = reference(incoming.cast(), 0);
    if !vt(next, 0x705f08) {
        return;
    }
    let attack = reference(next, 0x20);
    if !vt(attack, 0x705d50) {
        return;
    }
    let target = reference(attack, 0x28);
    if !flare_target(target) {
        return;
    }
    let owner = reference(state, 0x10);
    let getter = method(owner, 0x50);
    if !executable(getter) {
        return;
    }
    let get: unsafe extern "C" fn(*mut u8) -> *mut u8 = core::mem::transmute(getter);
    let unit = get(owner);
    let getter = method(unit, 0xb0);
    if !executable(getter) {
        return;
    }
    let get: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(getter);
    let components = get(unit);
    if components.is_null() {
        return;
    }
    let chassis: *mut u8 = read(components, 0x38);
    if soldier(chassis).is_some_and(|s| s.unit == unit) {
        trace_flare(chassis, event, target);
    }
}
unsafe fn soldier(chassis: *mut u8) -> Option<Soldier> {
    if !vt(chassis, 0x72c3b0) {
        return None;
    }
    let unit = reference(chassis, 0x10);
    let a = method(unit, 0xb0);
    if a == 0 {
        return None;
    }
    let get: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(a);
    let components = get(unit);
    if components.is_null() || read::<*mut u8>(components, 0x38) != chassis {
        return None;
    }
    Some(Soldier {
        unit,
        ai: read(components, 0x28),
        components: components.cast_mut(),
    })
}
unsafe fn gunner(ai: *mut u8) -> Option<Gunner> {
    let g = reference(ai, 0x1f0);
    if !vt(g, 0x72cd00) && !vt(g, 0x72ced0) {
        return None;
    }
    let begin = read::<usize>(g, 0x38);
    let end = read::<usize>(g, 0x40);
    let bytes = end.checked_sub(begin)?;
    let index = read::<i32>(g, 0x94);
    if begin == 0 || bytes % 8 != 0 || bytes / 8 > 256 || index < 0 || index as usize >= bytes / 8 {
        return None;
    }
    let gun: *mut u8 = read(begin as *const u8, index as usize * 8);
    let a = method(gun, 0x160);
    if a == 0 {
        return None;
    }
    let get: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(a);
    let descriptor = get(gun);
    if descriptor.is_null() {
        return None;
    }
    let is_grenade = read::<u8>(descriptor, 0xa9);
    let shot_mode = read::<i32>(descriptor, 0xc4);
    let state = read::<i32>(g, 0x90);
    Some(Gunner {
        object: g,
        target: reference(g, 0x88),
        // Stock enum initializer 0x3c4ec0 maps move=3, deploy_trailer=5.
        grenade: is_grenade != 0 && shot_mode == 3 && matches!(state, 2 | 3 | 5),
    })
}
unsafe fn reserved_route(chassis: *mut u8) -> Option<*const u8> {
    let manager: *const u8 = read(chassis, 0x30);
    let nav = read::<i32>(chassis, 0xc4);
    if manager.is_null() || nav < 0 {
        return None;
    }
    let header: *const u8 = read(manager, 0x288);
    if header.is_null() || nav >= read::<i32>(header, 4) {
        return None;
    }
    let records: *const u8 = read(header, 8);
    if records.is_null() {
        return None;
    }
    let record = records.add(nav as usize * 0x2f0);
    (read::<*mut u8>(record, 0x258) == chassis && read::<u8>(record, 1) == 1).then_some(record)
}
unsafe fn moving_route(chassis: *mut u8) -> bool {
    read::<i32>(chassis, 0xec) == 1
        && read::<u8>(chassis, 0x18) != 0
        && read::<u8>(chassis, 0x28) & 1 != 0
        // Native request 0x333140 marks a newly queued route as 3; it must
        // survive aiming while the manager prepares state 2 movement.
        && reserved_route(chassis).is_some_and(|r| matches!(read::<u8>(r, 0x2cc), 2 | 3))
}
unsafe fn active_route(chassis: *mut u8) -> bool {
    moving_route(chassis) && reserved_route(chassis).is_some_and(|r| read::<u8>(r, 0x2cc) == 2)
}
unsafe fn capture(
    state: *mut u8,
    incoming: *const *const u8,
    attack_move: bool,
) -> Option<*mut u8> {
    if incoming.is_null() {
        return None;
    }
    let junction = incoming.read();
    if junction.is_null() {
        return None;
    }
    let next: *mut u8 = read(junction, 0x10);
    let attack = if vt(next, 0x705f08) {
        reference(next, 0x20)
    } else {
        core::ptr::null_mut()
    };
    let old = reference(state, 0x20);
    let owner = reference(state, 0x10);
    let a = method(owner, 0x50);
    if a == 0 {
        return None;
    }
    let get: unsafe extern "C" fn(*mut u8) -> *mut u8 = core::mem::transmute(a);
    let unit = get(owner);
    let a = method(unit, 0xb0);
    if a == 0 {
        return None;
    }
    let get: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(a);
    let components = get(unit);
    if components.is_null() {
        return None;
    }
    let chassis: *mut u8 = read(components, 0x38);
    let s = soldier(chassis)?;
    if s.unit != unit {
        return None;
    }
    if !vt(next, 0x705f08) {
        forget(chassis);
        return None;
    }
    // Attack-move stores its outer attack order at +0x20. Its live route is
    // in the chassis, while ordinary move stores an AiMoveOrder at +0x20.
    if !vt(attack, 0x705d50) || !vt(old, if attack_move { 0x705d50 } else { 0x70c068 }) {
        return None;
    }
    if s.unit != unit || !active_route(chassis) {
        return None;
    }
    let move_target = read::<usize>(chassis, 0xd8);
    let target = reference(attack, 0x28);
    let speed = read::<f32>(chassis, 0x12c);
    if move_target == 0
        || reference(chassis, 0xd8).is_null()
        || target.is_null()
        || !speed.is_finite()
        || speed <= 0.0
    {
        return None;
    }
    let end_direction = if read::<u8>(chassis, 0x10b) != 0 {
        let d = [read::<f32>(chassis, 0x110), read::<f32>(chassis, 0x114)];
        if d.iter().any(|v| !v.is_finite()) {
            return None;
        }
        Some(d)
    } else {
        None
    };
    let route = Route {
        time: GetTickCount64(),
        unit: unit as usize,
        manager: read(chassis, 0x30),
        nav: read(chassis, 0xc4),
        move_target,
        attack_target: target as usize,
        speed,
        end_direction,
    };
    if let Ok(mut routes) = ROUTES.lock() {
        if routes.len() >= 512 && !routes.contains_key(&(chassis as usize)) {
            if let Some(key) = routes.iter().min_by_key(|(_, r)| r.time).map(|(k, _)| *k) {
                routes.remove(&key);
            }
        }
        routes.insert(chassis as usize, route);
        drop(routes);
        retain_flare_order(&s, chassis, state, route, attack_move);
        Some(chassis)
    } else {
        None
    }
}
unsafe extern "C" fn order(state: *mut u8, incoming: *const *const u8) {
    trace_flare_order(state, incoming, "move-order-before");
    let callback: Order = core::mem::transmute(original(4));
    let chassis = capture(state, incoming, false);
    trace_flare_order(state, incoming, "move-order-capture-result");
    let previous = REPLACING.with(|v| v.get());
    // Attack-move can dispatch to its nested ordinary move state after its
    // first Stop. That nested capture misses (mode=0), but its second Stop is
    // still part of the outer replacement. Retain that dynamic scope only for
    // an incoming attack; other incoming commands already invalidate the cache.
    let inherited = if vt(reference(incoming.cast(), 0), 0x705f08) {
        previous
    } else {
        0
    };
    REPLACING.with(|v| v.set(chassis.map_or(inherited, |p| p as usize)));
    callback(state, incoming);
    REPLACING.with(|v| v.set(previous));
}
unsafe extern "C" fn order_attack_move(state: *mut u8, incoming: *const *const u8) {
    trace_flare_order(state, incoming, "attack-move-order-before");
    let callback: Order = core::mem::transmute(original(5));
    let chassis = capture(state, incoming, true);
    trace_flare_order(state, incoming, "attack-move-order-capture-result");
    let previous = REPLACING.with(|v| v.get());
    let inherited = if vt(reference(incoming.cast(), 0), 0x705f08) {
        previous
    } else {
        0
    };
    REPLACING.with(|v| v.set(chassis.map_or(inherited, |p| p as usize)));
    callback(state, incoming);
    REPLACING.with(|v| v.set(previous));
}
unsafe extern "C" fn stop(chassis: *mut u8, force: u8) {
    trace_flare(chassis, "stop-before", core::ptr::null_mut());
    if !REPLACING.with(|v| v.get() == chassis as usize) {
        forget(chassis);
    }
    let callback: Stop = core::mem::transmute(original(0));
    callback(chassis, force);
}
unsafe extern "C" fn turn_point(chassis: *mut u8, point: *const f32) {
    trace_flare(chassis, "turn-point-before", core::ptr::null_mut());
    forget(chassis);
    let callback: Turn = core::mem::transmute(original(1));
    callback(chassis, point);
}
unsafe fn aim_call() -> bool {
    let mut frames = [core::ptr::null_mut(); 16];
    let n = RtlCaptureStackBackTrace(0, 16, frames.as_mut_ptr(), core::ptr::null_mut());
    frames[..n as usize]
        .iter()
        .any(|p| *p as usize == address(0x2d3880))
}
unsafe fn candidate_chassis(g: *mut u8, index: usize) -> Option<*mut u8> {
    if !vt(g, 0x72cd00) && !vt(g, 0x72ced0) {
        return None;
    }
    let begin = read::<usize>(g, 0x38);
    let bytes = read::<usize>(g, 0x40).checked_sub(begin)?;
    if begin == 0 || bytes % 8 != 0 || bytes / 8 > 256 || index >= bytes / 8 {
        return None;
    }
    let gun: *mut u8 = read(begin as *const u8, index * 8);
    let a = method(gun, 0x160);
    if a == 0 {
        return None;
    }
    let get: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(a);
    let descriptor = get(gun);
    if descriptor.is_null()
        || read::<u8>(descriptor, 0xa9) == 0
        || read::<i32>(descriptor, 0xc4) != 3
    {
        return None;
    }
    // Native HumanGunner owns its unit directly at +0x20.
    let unit: *mut u8 = read(g, 0x20);
    let a = method(unit, 0xb0);
    if a == 0 {
        return None;
    }
    let get: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(a);
    let components = get(unit);
    if components.is_null() {
        return None;
    }
    let chassis: *mut u8 = read(components, 0x38);
    let s = soldier(chassis)?;
    (s.unit == unit && reference(s.ai, 0x1f0) == g).then_some(chassis)
}
unsafe fn grenade_inventory(g: *mut u8) -> (u64, usize) {
    if !vt(g, 0x72cd00) && !vt(g, 0x72ced0) {
        return (0, 0);
    }
    let begin = read::<usize>(g, 0x38);
    let Some(bytes) = read::<usize>(g, 0x40).checked_sub(begin) else {
        return (0, 0);
    };
    if begin == 0 || bytes % 8 != 0 || bytes / 8 > 256 {
        return (0, 0);
    }
    let mut mask = 0u64;
    let mut count = 0usize;
    for index in 0..bytes / 8 {
        let gun: *mut u8 = read(begin as *const u8, index * 8);
        let get = method(gun, 0x160);
        if gun.is_null() || get == 0 {
            continue;
        }
        let get: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(get);
        let descriptor = get(gun);
        if !descriptor.is_null()
            && read::<u8>(descriptor, 0xa9) != 0
            && read::<i32>(descriptor, 0xc4) == 3
        {
            count += 1;
            if index < 64 {
                mask |= 1u64 << index;
            }
        }
    }
    (mask, count)
}
unsafe fn selector_chassis(g: *mut u8) -> Option<*mut u8> {
    if !vt(g, 0x72cd00) && !vt(g, 0x72ced0) {
        return None;
    }
    let unit: *mut u8 = read(g, 0x20);
    let a = method(unit, 0xb0);
    if a == 0 {
        return None;
    }
    let get: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(a);
    let components = get(unit);
    if components.is_null() {
        return None;
    }
    let chassis: *mut u8 = read(components, 0x38);
    let s = soldier(chassis)?;
    (s.unit == unit && reference(s.ai, 0x1f0) == g).then_some(chassis)
}
unsafe fn report_selector(g: *mut u8, chassis: *mut u8, diagnostic: SelectorDiagnostic) {
    let animation: *mut u8 = read(chassis, 0x58);
    let action = if animation.is_null() {
        -1
    } else {
        read(animation, 0x68)
    };
    let pending = if animation.is_null() {
        -1
    } else {
        read(animation, 0x6c)
    };
    let posture = if animation.is_null() {
        -1
    } else {
        read(animation, 0x74)
    };
    let mode = read::<i32>(chassis, 0xec);
    let nav = read::<i32>(chassis, 0xc4);
    let route_state = reserved_route(chassis).map_or(-1, |r| read::<u8>(r, 0x2cc) as i32);
    let branch = read::<i32>(g, 0xb8);
    let state = read::<i32>(g, 0x90);
    let line = format!(
        "grenade movement selector diagnostic: chassis={chassis:p} selected={} grenade_candidates={} candidate_mask={:#x} branch={branch} state={state} action={action:#x} pending={pending:#x} posture={posture} mode={mode} nav={nav} route_state={route_state} active_route={} eligible={} requires_idle={} native_result={} overridden={}",
        diagnostic.selected,
        diagnostic.grenade_count,
        diagnostic.grenade_mask,
        active_route(chassis),
        diagnostic.trace.eligible,
        diagnostic.trace.idle,
        diagnostic.native,
        diagnostic.overridden
    );
    if let Ok(mut reports) = SELECTOR_REPORTS.lock() {
        if reports.len() < 64 && reports.insert(line.clone()) {
            log(LOG_DEBUG, &line);
        }
    }
}
unsafe extern "C" fn weapon_eligible(gun: *mut u8) -> u64 {
    let callback: WeaponCheck = core::mem::transmute(original(7));
    let result = callback(gun);
    SELECTING.with(|v| {
        let mut trace = v.get();
        if trace.gun != 0 && trace.gun == gun as usize {
            trace.eligible = i32::from(result as u8);
            v.set(trace);
        }
    });
    result
}
unsafe extern "C" fn weapon_requires_idle(gun: *mut u8) -> u64 {
    let callback: WeaponCheck = core::mem::transmute(original(8));
    let result = callback(gun);
    SELECTING.with(|v| {
        let mut trace = v.get();
        if trace.gun != 0 && trace.gun == gun as usize {
            trace.idle = i32::from(result as u8);
            v.set(trace);
        }
    });
    result
}
unsafe fn candidate_gate(g: *mut u8, index: usize) -> Option<CandidateGate> {
    if read::<i32>(g, 0x90) != 2 || read::<i32>(g, 0xb8) != 1 {
        return None;
    }
    let chassis = candidate_chassis(g, index)?;
    if !active_route(chassis) {
        return None;
    }
    let animation: *mut u8 = read(chassis, 0x58);
    let s = soldier(chassis)?;
    if !vt(animation, 0x72c118)
        || reference(animation, 0x10) != s.unit
        || read::<i32>(animation, 0x74) != 1
        || !matches!(read::<i32>(animation, 0x68), 1 | 3 | 4)
        || !matches!(read::<i32>(animation, 0x6c), 1 | 3 | 4)
    {
        return None;
    }
    let target = reference(g, 0x88);
    if target.is_null() || method(target, 0x28) != address(0x46a740) {
        return None;
    }
    Some(CandidateGate {
        chassis: chassis as usize,
        gunner: g as usize,
        index,
        target: target as usize,
    })
}
unsafe extern "C" fn candidate(g: *mut u8, index: usize, target: u32, flags: u32) -> u64 {
    let callback: Candidate = core::mem::transmute(original(10));
    let gate = candidate_gate(g, index);
    let previous = CANDIDATE_GATE.with(|v| v.replace(gate));
    let result = callback(g, index, target, flags);
    CANDIDATE_GATE.with(|v| v.set(previous));
    result
}
unsafe fn candidate_gate_callsite() -> bool {
    let mut frames = [core::ptr::null_mut(); 16];
    let n = RtlCaptureStackBackTrace(0, 16, frames.as_mut_ptr(), core::ptr::null_mut());
    frames[..n as usize]
        .iter()
        .any(|p| *p as usize == address(0x2d9744))
}
unsafe extern "C" fn chassis_can_move(chassis: *mut u8) -> u64 {
    let callback: ChassisCanMove = core::mem::transmute(original(11));
    let result = callback(chassis);
    if result as u8 != 0 {
        return result;
    }
    let gate = CANDIDATE_GATE.with(Cell::get);
    let Some(gate) = gate else {
        return result;
    };
    if !candidate_gate_callsite() {
        return result;
    }
    let g = gate.gunner as *mut u8;
    let Some(current) = candidate_gate(g, gate.index) else {
        return result;
    };
    if chassis as usize != gate.chassis
        || current.chassis != gate.chassis
        || current.target != gate.target
    {
        return result;
    }
    if CANDIDATE_GATE_REPORTS.fetch_add(1, Ordering::Relaxed) < 32 {
        log(
            LOG_DEBUG,
            &format!(
                "moving grenade candidate gate passed: chassis={chassis:p} gunner={g:p} index={} callsite=0x2d9744 native={result:#x}",
                gate.index,
            ),
        );
    }
    // The native caller tests AL only. Preserve all native return bits while
    // satisfying that boolean check so its peer, target and range gates run.
    result | 1
}
unsafe extern "C" fn select(g: *mut u8) -> u8 {
    sweep_flare_orders();
    let callback: Select = core::mem::transmute(original(6));
    let index = if vt(g, 0x72cd00) {
        read::<i32>(g, 0x94)
    } else {
        -1
    };
    let chassis = if index >= 0 {
        candidate_chassis(g, index as usize)
    } else {
        None
    };
    let diagnostic_chassis = chassis.or_else(|| selector_chassis(g));
    let diagnostic_context = diagnostic_chassis.is_some_and(|chassis| {
        let animation: *mut u8 = read(chassis, 0x58);
        !animation.is_null()
            && (moving_route(chassis)
                || read::<i32>(animation, 0x68) == 3
                || read::<i32>(animation, 0x6c) == 3)
    });
    let gun = chassis.map_or(core::ptr::null_mut(), |_| {
        read::<*mut u8>(read::<*const u8>(g, 0x38), index as usize * 8)
    });
    let initial_state = if gun.is_null() {
        -1
    } else {
        read::<i32>(g, 0x90)
    };
    let previous = SELECTING.with(|v| {
        v.replace(SelectionTrace {
            gun: gun as usize,
            eligible: -1,
            idle: -1,
        })
    });
    let native = callback(g);
    let trace = SELECTING.with(|v| v.replace(previous));
    let Some(chassis) = chassis else {
        if diagnostic_context {
            let chassis = diagnostic_chassis.unwrap();
            let (grenade_mask, grenade_count) = grenade_inventory(g);
            if grenade_count != 0 {
                report_selector(
                    g,
                    chassis,
                    SelectorDiagnostic {
                        selected: index,
                        grenade_mask,
                        grenade_count,
                        native,
                        trace,
                        overridden: false,
                    },
                );
            }
        }
        return native;
    };
    // Revalidate the current inventory and actor after native selection. A
    // native switch to any other weapon cannot inherit this exception.
    if read::<i32>(g, 0x94) != index
        || candidate_chassis(g, index as usize) != Some(chassis)
        || read::<*mut u8>(read::<*const u8>(g, 0x38), index as usize * 8) != gun
    {
        return native;
    }
    let animation: *mut u8 = read(chassis, 0x58);
    let Some(s) = soldier(chassis) else {
        return native;
    };
    if !vt(animation, 0x72c118) || reference(animation, 0x10) != s.unit {
        return native;
    }
    let branch = read::<i32>(g, 0xb8);
    let state = read::<i32>(g, 0x90);
    let pending = read::<i32>(animation, 0x6c);
    // Stock 2d8350's selected-weapon branch has already passed its status and
    // range/aim tests here. Only replace its final pending-idle restriction.
    // Do not override a candidate scan, failed/uncalled weapon check, timer,
    // unrelated animation, standing order, or changed selection.
    let overridden = native == 0
        && initial_state == 2
        && state == 2
        && branch == 2
        && trace.eligible == 1
        && trace.idle == 1
        && pending == 3
        && method(gun, 0x1c0) == address(0x299330)
        && method(gun, 0x1f0) == address(0x2996e0)
        && moving_route(chassis);
    if diagnostic_context {
        let (grenade_mask, grenade_count) = grenade_inventory(g);
        if grenade_count != 0 {
            report_selector(
                g,
                chassis,
                SelectorDiagnostic {
                    selected: index,
                    grenade_mask,
                    grenade_count,
                    native,
                    trace,
                    overridden,
                },
            );
        }
    }
    if overridden {
        1
    } else {
        native
    }
}
unsafe extern "C" fn turn_direction(chassis: *mut u8, direction: *const f32) {
    let aim = aim_call();
    if !aim {
        trace_flare(chassis, "turn-direction-before", core::ptr::null_mut());
        forget(chassis);
    }
    if aim
        && !direction.is_null()
        && soldier(chassis).is_some_and(|s| gunner(s.ai).is_some_and(|g| g.grenade))
        && moving_route(chassis)
    {
        let x = direction.read_unaligned();
        let y = direction.add(1).read_unaligned();
        let length2 = x * x + y * y;
        if x.is_finite() && y.is_finite() && length2.is_finite() {
            let scale = if length2 == 0.0 {
                1.0
            } else {
                1.0 / length2.sqrt()
            };
            // Stock 2ccaf0's direction tail, without clearing the active route.
            write(chassis, 0x110, x * scale);
            write(chassis, 0x114, y * scale);
            write(chassis, 0x10b, 1u8);
            return;
        }
    }
    let callback: Turn = core::mem::transmute(original(2));
    callback(chassis, direction);
}

/// Recompute a live grenade target bearing for the two native chassis turns.
/// Return addresses are passed from the entry wrapper so other callers of the
/// shared native helper always keep their original path vector.
unsafe extern "C" fn grenade_heading(
    chassis: *mut u8,
    _original_direction: *const f32,
    caller: usize,
    output: *mut f32,
) -> u8 {
    if caller != address(0x2c92bc) && caller != address(0x2ca0fc) {
        return 0;
    }
    if !vt(chassis, 0x72c3b0) || !active_route(chassis) {
        return 0;
    }
    let Some(s) = soldier(chassis) else {
        return 0;
    };
    let Some(g) = gunner(s.ai) else {
        return 0;
    };
    if !g.grenade || g.target.is_null() || read::<*mut u8>(g.object, 0x20) != s.unit {
        return 0;
    }
    let animation: *mut u8 = read(chassis, 0x58);
    if !vt(animation, 0x72c118)
        || reference(animation, 0x10) != s.unit
        || read::<i32>(animation, 0x74) != 1
        || !matches!(read::<i32>(animation, 0x6c), 0x17 | 0x2b)
    {
        return 0;
    }
    // Only the verified AttackTarget getter or exact stock FlareTarget layout
    // is supported, including before the neutral-heading path.
    let target_getter = method(g.target, 0x28);
    if target_getter != address(0x46a740) && !flare_target(g.target) {
        return 0;
    }

    // The aim helper already consumed this tick's one native turn-rate step.
    // Keep the later route-facing call neutral until that aim turn completes.
    if caller == address(0x2ca0fc)
        && read::<u8>(chassis, 0x108) != 0
        && read::<u8>(chassis, 0x10b) != 0
    {
        let x = read::<f32>(chassis, 0x84);
        let y = read::<f32>(chassis, 0x88);
        let length2 = x * x + y * y;
        if !x.is_finite() || !y.is_finite() || !length2.is_finite() || length2 <= f32::EPSILON {
            return 0;
        }
        output.write_unaligned(x);
        output.add(1).write_unaligned(y);
        return 1;
    }

    // AttackTarget vfunc 5 (+0x28) returns its native world point, refreshing
    // dynamic targets through their current position getter. The chassis
    // position facet exposes its float3 through vfunc +0x58.
    let position_facet: *mut u8 = read(chassis, 0x48);
    if read::<*mut u8>(s.components, 0) != position_facet
        || read::<*mut u8>(s.components, 0x58) != animation
    {
        return 0;
    }
    let position_getter = method(position_facet, 0x58);
    if !executable(position_getter) {
        return 0;
    }
    let get_position: unsafe extern "C" fn(*mut u8) -> *const f32 =
        core::mem::transmute(position_getter);
    let position = get_position(position_facet);
    let get_target: unsafe extern "C" fn(*mut u8) -> *const f32 =
        core::mem::transmute(target_getter);
    let target = get_target(g.target);
    if position.is_null() || target.is_null() {
        return 0;
    }
    let x = target.read_unaligned() - position.read_unaligned();
    let y = target.add(1).read_unaligned() - position.add(1).read_unaligned();
    let length2 = x * x + y * y;
    if !x.is_finite() || !y.is_finite() || !length2.is_finite() || length2 <= f32::EPSILON {
        return 0;
    }
    let scale = length2.sqrt().recip();
    output.write_unaligned(x * scale);
    output.add(1).write_unaligned(y * scale);
    let behind = read::<f32>(chassis, 0x84) * x + read::<f32>(chassis, 0x88) * y < 0.0;
    let report = FACING_REPORTS
        .lock()
        .is_ok_and(|mut seen| seen.len() < 64 && seen.insert((chassis as usize, behind)));
    if report {
        log(LOG_DEBUG, &format!(
            "moving grenade facing redirected: chassis={chassis:p} caller={:#x} action={:#x} target_behind={} bearing=({:.3},{:.3}) actor=({:.3},{:.3}) target=({:.3},{:.3})",
            caller - BASE.load(Ordering::Acquire), read::<i32>(animation, 0x6c), behind,
            x * scale, y * scale, position.read_unaligned(), position.add(1).read_unaligned(),
            target.read_unaligned(), target.add(1).read_unaligned(),
        ));
    }
    1
}

// Preserve the incoming volatile argument state while Rust checks ownership,
// action, route and target. The original helper is the final call so its
// native return/register contract reaches the original caller unchanged.
core::arch::global_asm!(
    ".text",
    ".globl defiance_grenade_steer",
    ".def defiance_grenade_steer; .scl 2; .type 32; .endef",
    ".seh_proc defiance_grenade_steer",
    "defiance_grenade_steer:",
    "sub rsp, 0xe8",
    ".seh_stackalloc 0xe8",
    ".seh_endprologue",
    "mov [rsp + 0x20], rcx",
    "mov [rsp + 0x28], rdx",
    "mov [rsp + 0x30], r8",
    "mov [rsp + 0x38], r9",
    "mov [rsp + 0x40], r10",
    "mov [rsp + 0x48], r11",
    "mov [rsp + 0x50], rax",
    "movaps [rsp + 0x80], xmm0",
    "movaps [rsp + 0x90], xmm1",
    "movaps [rsp + 0xa0], xmm2",
    "movaps [rsp + 0xb0], xmm3",
    "movaps [rsp + 0xc0], xmm4",
    "movaps [rsp + 0xd0], xmm5",
    "mov rcx, [rsp + 0x20]",
    "mov rdx, [rsp + 0x28]",
    "mov r8, [rsp + 0xe8]",
    "lea r9, [rsp + 0x60]",
    "call {selector}",
    "mov [rsp + 0x58], al",
    "2:",
    "mov rax, qword ptr [rip + {originals} + 72]",
    "test rax, rax",
    "jne 3f",
    "pause",
    "jmp 2b",
    "3:",
    "mov [rsp + 0x70], rax",
    "mov rcx, [rsp + 0x20]",
    "mov r8, [rsp + 0x30]",
    "mov r9, [rsp + 0x38]",
    "mov r10, [rsp + 0x40]",
    "mov r11, [rsp + 0x48]",
    "mov rax, [rsp + 0x50]",
    "movaps xmm0, [rsp + 0x80]",
    "movaps xmm1, [rsp + 0x90]",
    "movaps xmm2, [rsp + 0xa0]",
    "movaps xmm3, [rsp + 0xb0]",
    "movaps xmm4, [rsp + 0xc0]",
    "movaps xmm5, [rsp + 0xd0]",
    "cmp byte ptr [rsp + 0x58], 0",
    "je 2f",
    "lea rdx, [rsp + 0x60]",
    "jmp 3f",
    "2:",
    "mov rdx, [rsp + 0x28]",
    "3:",
    "call qword ptr [rsp + 0x70]",
    "add rsp, 0xe8",
    "ret",
    ".seh_endproc",
    selector = sym grenade_heading,
    originals = sym ORIGINALS,
);
/// Trace the reserved owner before the native manager clears its route. The
/// original function remains the final call, including invalid-index returns.
unsafe extern "C" fn manager_cancel_trace(manager: *const u8, nav: i32, native_header: *const u8) {
    if manager.is_null() || nav < 0 || FLARE_REPORTS.load(Ordering::Relaxed) >= 96 {
        return;
    }
    let header: *const u8 = read(manager, 0x288);
    if header.is_null() || header != native_header || nav >= read::<i32>(header, 4) {
        return;
    }
    let records: *const u8 = read(header, 8);
    if records.is_null() {
        return;
    }
    let record = records.add(nav as usize * 0x2f0);
    if read::<u8>(record, 1) != 1 {
        return;
    }
    let chassis: *mut u8 = read(record, 0x258);
    let relevant = FLARE_ROUTES.lock().ok().is_some_and(|routes| {
        routes
            .get(&(chassis as usize))
            .is_some_and(|r| GetTickCount64().saturating_sub(r.route.time) <= 8000)
    });
    if relevant
        && !chassis.is_null()
        && read::<usize>(chassis, 0x30) == manager as usize
        && read::<i32>(chassis, 0xc4) == nav
        && reserved_route(chassis) == Some(record)
    {
        trace_flare(chassis, "manager-cancel-before", core::ptr::null_mut());
    }
}

// Read-only tracing must not replace the path manager's native register/return
// behavior. Save inputs for the trace, restore them, then call native last.
core::arch::global_asm!(
    ".text",
    ".globl defiance_grenade_cancel_trace",
    ".def defiance_grenade_cancel_trace; .scl 2; .type 32; .endef",
    ".seh_proc defiance_grenade_cancel_trace",
    "defiance_grenade_cancel_trace:",
    "sub rsp, 0xe8",
    ".seh_stackalloc 0xe8",
    ".seh_endprologue",
    "mov [rsp + 0x20], rcx",
    "mov [rsp + 0x28], rdx",
    "mov [rsp + 0x30], r8",
    "mov [rsp + 0x38], r9",
    "mov [rsp + 0x40], r10",
    "mov [rsp + 0x48], r11",
    "mov [rsp + 0x50], rax",
    "movaps [rsp + 0x80], xmm0",
    "movaps [rsp + 0x90], xmm1",
    "movaps [rsp + 0xa0], xmm2",
    "movaps [rsp + 0xb0], xmm3",
    "movaps [rsp + 0xc0], xmm4",
    "movaps [rsp + 0xd0], xmm5",
    "call {selector}",
    "2:",
    "mov rax, qword ptr [rip + {originals} + 96]",
    "test rax, rax",
    "jne 3f",
    "pause",
    "jmp 2b",
    "3:",
    "mov [rsp + 0x70], rax",
    "mov rcx, [rsp + 0x20]",
    "mov rdx, [rsp + 0x28]",
    "mov r8, [rsp + 0x30]",
    "mov r9, [rsp + 0x38]",
    "mov r10, [rsp + 0x40]",
    "mov r11, [rsp + 0x48]",
    "mov rax, [rsp + 0x50]",
    "movaps xmm0, [rsp + 0x80]",
    "movaps xmm1, [rsp + 0x90]",
    "movaps xmm2, [rsp + 0xa0]",
    "movaps xmm3, [rsp + 0xb0]",
    "movaps xmm4, [rsp + 0xc0]",
    "movaps xmm5, [rsp + 0xd0]",
    "call qword ptr [rsp + 0x70]",
    "add rsp, 0xe8",
    "ret",
    ".seh_endproc",
    selector = sym manager_cancel_trace,
    originals = sym ORIGINALS,
);
unsafe fn resume(chassis: *mut u8, s: &Soldier, g: &Gunner) {
    let route = ROUTES
        .lock()
        .ok()
        .and_then(|mut r| r.remove(&(chassis as usize)));
    let Some(r) = route else {
        return;
    };
    if GetTickCount64().saturating_sub(r.time) > 8000
        || r.unit != s.unit as usize
        || r.manager != read::<usize>(chassis, 0x30)
        || r.nav != read::<i32>(chassis, 0xc4)
        || r.move_target != read::<usize>(chassis, 0xd8)
        || r.attack_target != g.target as usize
        || read::<i32>(chassis, 0xec) != 0
        || reference(chassis, 0xd8).is_null()
        || read::<u8>(chassis, 0x18) == 0
        || read::<u8>(chassis, 0x28) & 1 == 0
    {
        return;
    }
    if !reserved_route(chassis).is_some_and(|record| read::<u8>(record, 0x2cc) == 0) {
        return;
    }
    let copy: CopyRef = core::mem::transmute(address(0x123f0));
    let native: Resume = core::mem::transmute(address(0x2cb7a0));
    let mut target = core::ptr::null_mut();
    copy(&mut target, chassis.add(0xd8).cast());
    let direction = r
        .end_direction
        .as_ref()
        .map_or(core::ptr::null(), |d| d.as_ptr());
    let flare_kind = flare_target(g.target).then(|| read::<u32>(g.target, 0x28));
    let success = native(chassis, &mut target, r.speed, direction);
    if let Ok(mut orders) = FLARE_ORDERS.lock() {
        if let Some(pending) = orders.get_mut(&(s.ai as usize)) {
            if pending.chassis == chassis as usize && pending.route.attack_target == r.attack_target
            {
                pending.resumed = success != 0 && moving_route(chassis);
                pending.route.time = GetTickCount64();
            }
        }
    }
    if let Some(kind) = flare_kind.filter(|_| success != 0 && read::<i32>(chassis, 0xec) == 1) {
        if let Ok(mut routes) = FLARE_ROUTES.lock() {
            if routes.len() >= 256 && !routes.contains_key(&(chassis as usize)) {
                if let Some(key) = routes
                    .iter()
                    .min_by_key(|(_, r)| r.route.time)
                    .map(|(k, _)| *k)
                {
                    routes.remove(&key);
                }
            }
            routes.insert(
                chassis as usize,
                FlareRoute {
                    route: Route {
                        time: GetTickCount64(),
                        ..r
                    },
                    gunner: g.object as usize,
                    kind,
                    forgotten: false,
                },
            );
        }
    }
    if success == 0 && NATIVE_FAILURE_REPORTS.fetch_add(1, Ordering::Relaxed) < 8 {
        log(
            LOG_ERROR,
            &format!(
                "grenade movement native resume failed: chassis={chassis:p} nav={} age_ms={}",
                read::<i32>(chassis, 0xc4),
                GetTickCount64().saturating_sub(r.time)
            ),
        );
    }
}
unsafe extern "C" fn queue(animation: *mut u8, action: i32) {
    let callback: Queue = core::mem::transmute(original(3));
    if matches!(action, 3 | 0x17 | 0x2b) && vt(animation, 0x72c118) {
        let unit = reference(animation, 0x10);
        let a = method(unit, 0xb0);
        if a != 0 {
            let get: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(a);
            let components = get(unit);
            if !components.is_null() {
                let chassis: *mut u8 = read(components, 0x38);
                if let Some(s) = soldier(chassis) {
                    if s.unit == unit {
                        if action == 3 {
                            // Native movement helper 2ca960 cancels its route
                            // and then queues action 3. Trace that path as well.
                            trace_flare(chassis, "idle-queue-before", core::ptr::null_mut());
                        } else {
                            trace_flare(
                                chassis,
                                "throw-queue-before-resume",
                                core::ptr::null_mut(),
                            );
                            if let Some(g) = gunner(s.ai) {
                                if g.grenade {
                                    resume(chassis, &s, &g);
                                }
                            }
                            trace_flare(chassis, "throw-queue-after-resume", core::ptr::null_mut());
                        }
                    }
                }
            }
        }
    }
    callback(animation, action);
}
unsafe fn install(api: &Api) -> Result<(), String> {
    let base = (api.module_base)(c"logic.dll".as_ptr());
    if base.is_null() {
        return Err("logic.dll missing".into());
    }
    let size = (api.module_size)(base);
    let mut path = [0u16; 32768];
    let len = GetModuleFileNameW(base, path.as_mut_ptr(), path.len() as u32) as usize;
    if len == 0 || len >= path.len() {
        return Err("logic.dll path unavailable".into());
    }
    use std::os::windows::ffi::OsStringExt;
    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&path[..len]));
    let sha = defiance_core::sha256::file(&path).map_err(|e| e.to_string())?;
    let (build_index, build) = sites::BUILDS
        .iter()
        .enumerate()
        .find(|(_, build)| build.sha == sha)
        .ok_or_else(|| format!("unsupported logic.dll SHA-256 {sha}"))?;
    let mut manager_trace_ready = true;
    // Exact fingerprints include hook spans, helper entry points and the two
    // signed navigation bounds checks preceding the optional diagnostic body.
    for &(reference, prefix) in build.checks {
        let rva = build.rva(reference);
        if rva + prefix.len() > size
            || core::slice::from_raw_parts(base.cast::<u8>().add(rva), prefix.len()) != prefix
        {
            if reference == SITES[12] || reference == 0x333220 {
                manager_trace_ready = false;
                log(LOG_ERROR, "manager cancellation trace unavailable: bytes differ; core grenade movement remains active");
                continue;
            }
            return Err(format!(
                "native code differs at logic+{rva:#x} ({})",
                build.name
            ));
        }
    }
    BUILD_INDEX.store(build_index, Ordering::Release);
    BASE.store(base as usize, Ordering::Release);
    SIZE.store(size, Ordering::Release);
    let detours = [
        stop as *mut c_void,
        turn_point as *mut c_void,
        turn_direction as *mut c_void,
        queue as *mut c_void,
        order as *mut c_void,
        order_attack_move as *mut c_void,
        select as *mut c_void,
        weapon_eligible as *mut c_void,
        weapon_requires_idle as *mut c_void,
        steer as *mut c_void,
        candidate as *mut c_void,
        chassis_can_move as *mut c_void,
        cancel_trace as *mut c_void,
        submit as *mut c_void,
        stop_order_init as *mut c_void,
        ai_update as *mut c_void,
    ];
    let mut installed = Vec::with_capacity(SITES.len());
    for (i, (reference, detour)) in SITES.into_iter().zip(detours).enumerate() {
        let rva = build.rva(reference);
        if i == 12 && !manager_trace_ready {
            continue;
        }
        let mut trampoline = core::ptr::null_mut();
        let result = (api.hook)(base.cast::<u8>().add(rva).cast(), detour, &mut trampoline);
        if result != 0 || trampoline.is_null() {
            if i == 12 {
                if result == 0 {
                    (api.unhook)(base.cast::<u8>().add(rva).cast());
                }
                log(LOG_ERROR, "manager cancellation trace unavailable: hook refused; core grenade movement remains active");
                continue;
            }
            for prior in installed.iter().rev() {
                (api.unhook)(base.cast::<u8>().add(*prior).cast());
            }
            return Err(format!("hook refused at logic+{rva:#x}"));
        }
        ORIGINALS[i].store(trampoline as usize, Ordering::Release);
        installed.push(rva);
    }
    log(
        LOG_INFO,
        &format!(
            "grenade movement fix installed: build={}; manager_cancel_trace={}",
            build.name,
            ORIGINALS[12].load(Ordering::Acquire) != 0
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
    LOGGER.store(api.log as usize, Ordering::Release);
    match install(api) {
        Ok(()) => 0,
        Err(e) => {
            log(LOG_ERROR, &format!("grenade movement fix refused: {e}"));
            1
        }
    }
}
#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.moving-grenades".as_ptr(),
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
    fn all_builds_bind_every_hook_with_copyable_spans() {
        assert_eq!(sites::BUILDS.len(), 4);
        for build in sites::BUILDS {
            assert!(build.mapping.windows(2).all(|pair| pair[0].0 < pair[1].0));
            for reference in SITES {
                assert!(build.rva(reference) != 0);
                let (_, prefix) = build
                    .checks
                    .iter()
                    .find(|&&(rva, _)| rva == reference)
                    .unwrap();
                let span = defiance_core::decode::displaced(prefix, 5).unwrap();
                defiance_core::decode::validate_copy(&prefix[..span]).unwrap();
            }
        }
    }
    #[test]
    fn cancellation_body_is_copyable_by_the_actual_loader_decoder() {
        let body = sites::BUILDS[0]
            .checks
            .iter()
            .find(|&&(rva, _)| rva == SITES[12])
            .unwrap()
            .1;
        let short = defiance_core::decode::displaced(body, 5).unwrap();
        assert_eq!(short, 5);
        defiance_core::decode::validate_copy(&body[..short]).unwrap();
        let long = defiance_core::decode::displaced(body, 14).unwrap();
        assert_eq!(long, 16);
        defiance_core::decode::validate_copy(&body[..long]).unwrap();
    }

    #[test]
    fn rejected_cancellation_entry_reproduces_live_relocation_failure() {
        let entry = &[0x85, 0xd2, 0x0f, 0x88, 0x9a, 0, 0, 0];
        assert!(defiance_core::decode::validate_copy(entry)
            .unwrap_err()
            .contains("requires relocation"));
    }
}
