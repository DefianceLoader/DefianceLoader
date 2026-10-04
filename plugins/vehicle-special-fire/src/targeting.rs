//! Per passenger-mount target acquisition.
//!
//! The native selector reads the first gun in its gunner view for range and
//! ammo compatibility. A private, stack-backed view lets it run those checks
//! for one mount without changing the live gunner's vectors.

use defiance_api::Api;
use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

mod rebind;

pub static TICK_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub static DEPLOYMENT_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub static CHOOSE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub static COMMAND_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub static SHARED_REFRESH_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub static MOVE_ACQUIRE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub static CANDIDATE_QUERY_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub static CAPABLE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// Native `Gunner::vfunc_12`; it is called on the temporary view.
pub static QUERY: AtomicUsize = AtomicUsize::new(0);

pub(super) static SET_TARGET: AtomicUsize = AtomicUsize::new(0);
static IN_RANGE: AtomicUsize = AtomicUsize::new(0);
static TARGET_EQUALS: AtomicUsize = AtomicUsize::new(0);
static RELEASE_HANDLE: AtomicUsize = AtomicUsize::new(0);
static LOGIC_BASE: AtomicUsize = AtomicUsize::new(0);
static LOGIC_SIZE: AtomicUsize = AtomicUsize::new(0);

const GUNNER_VIEW_SIZE: usize = 0x168;
const GUNNER_GUNS_BEGIN: usize = 0x20;
const GUNNER_GUNS_END: usize = 0x28;
const GUNNER_GUNS_CAP: usize = 0x30;
const GUNNER_SOURCES_BEGIN: usize = 0x68;
const GUNNER_SOURCES_END: usize = 0x70;
const GUNNER_AUTOMATIC: usize = 0xc0;
const GUNNER_FIRING_PHASE: usize = 0xf4;
const GUNNER_MOUNT: usize = 0x80;
const MOUNT_INDEPENDENT_AIM: usize = 0xf9;
const GUN_TARGET: usize = 0xc8;
const GUN_ENABLED: usize = 0xe2;
const GUNNER_GUN_LIMIT: usize = 32;
/// The AI facet's `AiStateMachine` junction, stored by the `AiFacet`
/// constructor in all six builds.
const AI_STATE_MACHINE: usize = 0xe8;
const AI_ATTACK_STATE: u32 = 0x15;
const AI_ATTACK_WITH_MOVE_STATE: u32 = 0x17;
/// Order flag on orders the player gives.
const ORDER_EXPLICIT: u32 = 0x4;
/// Order flag on the Attack the AI issues after a Move on its attack target.
const ORDER_AUTOMATIC_ATTACK: u32 = 0x40;

// Native current time arrives in XMM3; dt is the fifth stack argument.
type Tick = unsafe extern "system" fn(usize, u8, u8, f32, f32);
type Deployment = unsafe extern "system" fn(usize, *mut f32, *mut u8) -> u8;
type Choose = unsafe extern "system" fn(usize, *mut usize, *mut usize, usize, usize) -> *mut usize;
type Query = unsafe extern "system" fn(usize, *mut usize) -> *mut usize;
type AssignTarget = unsafe extern "system" fn(usize, *mut usize, u8);
type SharedRefresh = unsafe extern "system" fn(usize, *mut usize);
type MoveAcquire = unsafe extern "system" fn(usize, *mut usize);
type CandidateQuery = unsafe extern "system" fn(*mut usize, usize, u8) -> *mut usize;
type Capable = unsafe extern "system" fn(usize, usize) -> u8;
type ObjectGetter = unsafe extern "system" fn(usize) -> usize;
type GunnerGetter = unsafe extern "system" fn(usize, usize) -> usize;
type TargetTest = unsafe extern "system" fn(usize, *mut usize) -> u8;
type ReleaseHandle = unsafe extern "system" fn(*mut usize);
type TargetEquals = unsafe extern "system" fn(*mut usize, *mut usize) -> u64;
type TargetIdentity = unsafe extern "system" fn(usize) -> usize;

#[derive(Clone, Copy, Default)]
struct HookContext {
    owner: usize,
    view: usize,
    gun: usize,
    firing_tick: bool,
    shared_refresh: bool,
}

thread_local! {
    static CONTEXT: Cell<HookContext> = Cell::new(HookContext::default());
}

#[derive(Clone, Copy)]
struct OrderMode {
    entity: usize,
    target: usize,
    automatic: bool,
    /// The attack order an explicit record came from, captured from the AI's
    /// top attack state the first time the record is checked against it.
    explicit_order: usize,
    last_seen: Instant,
}

impl OrderMode {
    /// Expires an explicit record once the AI replaces its order with one it
    /// issued itself. A Move given during an explicit attack becomes an
    /// automatic Attack ([`ORDER_AUTOMATIC_ATTACK`] without [`ORDER_EXPLICIT`])
    /// on the same target, with the Move deferred behind it; the record would
    /// otherwise keep that Attack, and the vehicle, in place.
    fn observe(&mut self, current: Option<(usize, u32)>) {
        let Some((order, flags)) = current else {
            return;
        };
        if self.automatic {
            return;
        }
        if self.explicit_order == 0 {
            self.explicit_order = order;
        } else if order != self.explicit_order
            && flags & ORDER_AUTOMATIC_ATTACK != 0
            && flags & ORDER_EXPLICIT == 0
        {
            self.automatic = true;
        }
    }

    fn allows(self, entity: usize, target: usize) -> bool {
        self.automatic && self.matches(entity, target)
    }

    fn matches(self, entity: usize, target: usize) -> bool {
        self.entity == entity && target != 0 && self.target == target
    }
}

static ORDER_MODES: OnceLock<Mutex<HashMap<usize, OrderMode>>> = OnceLock::new();

#[derive(Debug, Eq, PartialEq)]
enum TargetAction {
    Skip,
    Release,
    Set,
}

fn target_action(mode: u32, selected: bool, same_target: bool) -> TargetAction {
    if mode != 1 {
        TargetAction::Skip
    } else if !selected {
        TargetAction::Release
    } else if same_target {
        TargetAction::Skip
    } else {
        TargetAction::Set
    }
}

fn apply_selected_target(
    current: usize,
    selected: &mut usize,
    mut same: impl FnMut(usize, usize) -> bool,
    mut set: impl FnMut(&mut usize) -> bool,
    mut discard: impl FnMut(&mut usize),
) -> TargetAction {
    let action = target_action(
        1,
        *selected != 0,
        current != 0 && *selected != 0 && same(current, *selected),
    );
    match action {
        TargetAction::Skip => discard(selected),
        TargetAction::Release if current != 0 => {
            if !set(selected) {
                discard(selected);
            }
        }
        TargetAction::Release => discard(selected),
        TargetAction::Set => {
            if !set(selected) {
                discard(selected);
            }
        }
    }
    action
}

fn should_acquire_from_facts(
    automatic: bool,
    passenger: bool,
    live_source: bool,
    source_enabled: bool,
    gun_enabled: bool,
) -> bool {
    automatic && passenger && live_source && source_enabled && gun_enabled
}

/// Resolve the gun methods and the native `PtrJunction` release helper. The
/// hook entry points and their trampolines are owned by the plugin registry.
pub unsafe fn configure(api: &Api) -> Result<(), String> {
    if api.abi_version != defiance_api::ABI_VERSION || api.reserved != 0 {
        return Err("vehicle targeting: incompatible API".into());
    }
    let method = |class: &CStr, slot| unsafe { (api.vtable_slot)(class.as_ptr(), slot) as usize };
    let gun = c"Gun@Leonardo";
    let setter = method(gun, 5);
    let range = method(gun, 60);
    if [setter, range].contains(&0) {
        return Err("vehicle targeting: Gun RTTI methods are unavailable".into());
    }
    let base = (api.module_base)(c"logic.dll".as_ptr());
    if base.is_null() {
        return Err("vehicle targeting: logic.dll is unavailable".into());
    }
    let size = (api.module_size)(base);
    if !inside_module(base as usize, size, setter) || !inside_module(base as usize, size, range) {
        return Err("vehicle targeting: native setter or range method is outside logic.dll".into());
    }
    // This signature is unique in every supported logic.dll image.
    let target_equals = (api.find_pattern)(
        base,
        size,
        c"48 89 54 24 10 48 89 4c 24 08 53 56 57 48 83 ec 20 48 8b f2".as_ptr(),
    ) as usize;
    if !inside_module(base as usize, size, target_equals) {
        return Err("vehicle targeting: native target identity helper was not found".into());
    }
    let release = (api.find_pattern)(
        base,
        (api.module_size)(base),
        c"40 53 48 83 ec 20 48 8b d9 48 8b 09 48 85 c9 74 ?? 48 83 79 10 00 74 ?? ff 15 ?? ?? ?? ?? 90 48 8b 0b 48 85 c9 74 ?? 83 41 08 ff 75 ?? 48 8b 01 ff 50 10 90 48 83 c4 20 5b c3".as_ptr(),
    ) as usize;
    if release == 0 {
        return Err("vehicle targeting: native owning-target release helper was not found".into());
    }
    // The helper is the game's own destructor path; retain its address only
    // after its complete signature has matched uniquely in logic.dll.
    RELEASE_HANDLE.store(release, Ordering::Release);
    SET_TARGET.store(setter, Ordering::Release);
    IN_RANGE.store(range, Ordering::Release);
    TARGET_EQUALS.store(target_equals, Ordering::Release);
    LOGIC_BASE.store(base as usize, Ordering::Release);
    LOGIC_SIZE.store(size, Ordering::Release);
    rebind::LOG.store(api.log as usize, Ordering::Release);
    Ok(())
}

/// Gunner update hook. Passenger mounts first rebind to a gun that can fire
/// ([`rebind::update`]); idle mounts acquire after native updates and rebindings.
pub unsafe extern "system" fn tick(gunner: usize, a: u8, b: u8, time: f32, dt: f32) {
    let original = TICK_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return;
    }
    let previous = CONTEXT.with(|context| {
        let old = context.get();
        context.set(HookContext {
            owner: gunner,
            firing_tick: true,
            ..HookContext::default()
        });
        old
    });
    unsafe { rebind::update(gunner) };
    unsafe {
        update_without_passenger_chassis_aim(gunner, || {
            std::mem::transmute::<usize, Tick>(original)(gunner, a, b, time, dt)
        })
    };
    unsafe { acquire_idle_mounts(gunner) };
    CONTEXT.with(|context| context.set(previous));
}

/// Passenger mounts aim through their joints without commandeering the driver.
/// GunMount's independent-aim flag suppresses its chassis-turn fallback, which
/// cancels the chassis movement target. Preserve it outside the native update.
unsafe fn update_without_passenger_chassis_aim(owner: usize, update: impl FnOnce()) {
    let mount = unsafe { q(owner, GUNNER_MOUNT) };
    let saved = if mount != 0
        && unsafe { q(mount, 0xd8) } == 0
        && unsafe { automatic_passenger_gunner(owner) }
    {
        let flag = (mount + MOUNT_INDEPENDENT_AIM) as *mut u8;
        let saved = unsafe { flag.read() };
        unsafe { flag.write(1) };
        Some((flag, saved))
    } else {
        None
    };
    update();
    if let Some((flag, saved)) = saved {
        unsafe { flag.write(saved) };
    }
}

unsafe fn automatic_passenger_gunner(owner: usize) -> bool {
    if owner == 0
        || unsafe { q(owner, 0x38) } != unsafe { q(owner, 0x40) }
        || !unsafe { automatic_intent(owner) }
    {
        return false;
    }
    let begin = unsafe { q(owner, GUNNER_GUNS_BEGIN) };
    let end = unsafe { q(owner, GUNNER_GUNS_END) };
    let Some(bytes) = end.checked_sub(begin).filter(|bytes| bytes % 8 == 0) else {
        return false;
    };
    if begin == 0 || bytes / 8 > GUNNER_GUN_LIMIT {
        return false;
    }
    let sources = unsafe { q(owner, GUNNER_SOURCES_BEGIN) };
    if sources == 0
        || unsafe { q(owner, GUNNER_SOURCES_END) }.checked_sub(sources) != Some(bytes)
        || unsafe { q(owner, 0x50) } != unsafe { q(owner, 0x58) }
    {
        return false;
    }
    let mut passenger = false;
    for index in 0..bytes / 8 {
        let gun = unsafe { q(begin + index * 8, 0) };
        if gun == 0 {
            continue;
        }
        if unsafe { passenger_source(owner, gun) }.is_some() {
            passenger = true;
        } else if unsafe { q(sources + index * 8, 0) } != 0
            || !unsafe { empty_passenger_mount(gun) }
        {
            // Queued aim requests outlive a weapon's enabled state. Only its
            // binding identity can distinguish passenger and chassis weapons.
            return false;
        }
    }
    passenger
}

unsafe fn empty_passenger_mount(gun: usize) -> bool {
    let descriptor = unsafe { q(gun, 0x40) };
    // Gunner::vfunc_44 identifies unused passenger mounts by this descriptor
    // name. A bound mount instead carries its passenger weapon's descriptor.
    let name = b"passenger_dummy_gun";
    if descriptor == 0
        || unsafe { q(descriptor, 0x18) } != name.len()
        || unsafe { q(descriptor, 0x20) } <= 15
    {
        return false;
    }
    let data = unsafe { q(descriptor, 8) };
    data != 0 && unsafe { std::slice::from_raw_parts(data as *const u8, name.len()) } == name
}

/// Native vehicle AI treats an enemy that any Gunner can engage as a reason
/// to leave Move for an attack state. Passenger mounts acquire their own
/// targets in [`tick`], so for a passenger-only vehicle these three entry
/// points report what a vehicle without combat Gunners reports: no candidate.
///
/// `AiMoveState`'s helper owns its candidate handle and can replace the state.
pub unsafe extern "system" fn move_acquire(state: usize, target: *mut usize) {
    let original = MOVE_ACQUIRE_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return;
    }
    if !target.is_null()
        && unsafe { *target } != 0
        && RELEASE_HANDLE.load(Ordering::Acquire) != 0
        && unsafe { passenger_only_entity(state_entity(state)) }
    {
        unsafe { discard(&mut *target) };
        return;
    }
    unsafe { std::mem::transmute::<usize, MoveAcquire>(original)(state, target) };
}

/// The enemy query behind idle deferral of an incoming Move and the attack
/// helpers. `output` is an empty result slot, not an owning handle to release.
pub unsafe extern "system" fn candidate_query(
    output: *mut usize,
    entity: usize,
    mode: u8,
) -> *mut usize {
    if !output.is_null() && unsafe { passenger_only_entity(entity) } {
        unsafe { output.write(0) };
        return output;
    }
    let original = CANDIDATE_QUERY_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return output;
    }
    unsafe { std::mem::transmute::<usize, CandidateQuery>(original)(output, entity, mode) }
}

/// Whether any Gunner can engage `enemy`; the AI keeps or enters combat on it.
pub unsafe extern "system" fn capable(entity: usize, enemy: usize) -> u8 {
    if unsafe { passenger_only_entity(entity) } {
        return 0;
    }
    let original = CAPABLE_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return 0;
    }
    unsafe { std::mem::transmute::<usize, Capable>(original)(entity, enemy) }
}

unsafe fn weak_object(handle: usize) -> usize {
    if handle == 0 {
        0
    } else {
        unsafe { q(handle, 0x10) }
    }
}

unsafe fn virtual_method(object: usize, offset: usize) -> usize {
    if object == 0 {
        return 0;
    }
    let table = unsafe { q(object, 0) };
    if table == 0 {
        0
    } else {
        unsafe { q(table, offset) }
    }
}

/// The entity an AI state's controller drives (`AiStateMachine::vfunc_10`).
unsafe fn state_entity(state: usize) -> usize {
    if state == 0 {
        return 0;
    }
    let controller = unsafe { weak_object(q(state, 0x10)) };
    let getter = unsafe { virtual_method(controller, 0x50) };
    if getter == 0 {
        return 0;
    }
    unsafe { std::mem::transmute::<usize, ObjectGetter>(getter)(controller) }
}

/// Whether every Gunner of `entity` is an automatic passenger gunner. A native
/// hull weapon or an explicit firing order leaves the native combat AI in
/// charge; units without Gunners are not vehicles this plugin serves.
/// The entity's AI facet, or 0.
unsafe fn entity_ai(entity: usize) -> usize {
    let facets_getter = unsafe { virtual_method(entity, 0xb0) };
    if facets_getter == 0 {
        return 0;
    }
    let facets = unsafe { std::mem::transmute::<usize, ObjectGetter>(facets_getter)(entity) };
    if facets == 0 {
        return 0;
    }
    unsafe { q(facets, 0x28) }
}

/// The AI facet's top state and its ID (state vfunc `+0x48`). The facet holds
/// its `AiStateMachine` junction at [`AI_STATE_MACHINE`]; the machine's active
/// states are a vector of junctions at `+0x38`/`+0x40`, the top one last.
unsafe fn top_ai_state(ai: usize) -> Option<(usize, u32)> {
    let machine = unsafe { weak_object(q(ai, AI_STATE_MACHINE)) };
    if machine == 0 {
        return None;
    }
    let begin = unsafe { q(machine, 0x38) };
    let end = unsafe { q(machine, 0x40) };
    let bytes = end.checked_sub(begin).filter(|bytes| *bytes % 8 == 0)?;
    if begin == 0 || bytes == 0 || bytes / 8 > 64 {
        return None;
    }
    let state = unsafe { weak_object(q(end - 8, 0)) };
    let id = unsafe { virtual_method(state, 0x48) };
    if id == 0 {
        return None;
    }
    Some((
        state,
        unsafe { std::mem::transmute::<usize, ObjectGetter>(id)(state) } as u32,
    ))
}

/// The order of the AI's top attack state and its flags, or `None` when the
/// top state is not an attack. Both attack states hold the order junction at
/// `+0x20`; the order keeps its flags at `+0x14`.
unsafe fn current_attack_order(ai: usize) -> Option<(usize, u32)> {
    let (state, id) = unsafe { top_ai_state(ai) }?;
    if id != AI_ATTACK_STATE && id != AI_ATTACK_WITH_MOVE_STATE {
        return None;
    }
    let order = unsafe { weak_object(q(state, 0x20)) };
    (order != 0).then(|| (order, unsafe { read_u32(order, 0x14) }))
}

unsafe fn passenger_only_entity(entity: usize) -> bool {
    let ai = unsafe { entity_ai(entity) };
    if ai == 0 {
        return false;
    }
    let count_offset = crate::orders::GUNNER_COUNT.load(Ordering::Acquire);
    let get_offset = crate::orders::GUNNER_GET.load(Ordering::Acquire);
    if count_offset == 0 || get_offset == 0 {
        return false;
    }
    let count_fn = unsafe { virtual_method(ai, count_offset) };
    let get_fn = unsafe { virtual_method(ai, get_offset) };
    if count_fn == 0 || get_fn == 0 {
        return false;
    }
    let count = unsafe { std::mem::transmute::<usize, ObjectGetter>(count_fn)(ai) };
    if count == 0 || count > GUNNER_GUN_LIMIT {
        return false;
    }
    let get = unsafe { std::mem::transmute::<usize, GunnerGetter>(get_fn) };
    if let Some(Ok(mut orders)) = ORDER_MODES.get().map(Mutex::lock) {
        let current = unsafe { current_attack_order(ai) };
        for index in 0..count {
            if let Some(order) = orders.get_mut(&unsafe { get(ai, index) }) {
                order.observe(current);
            }
        }
    }
    (0..count).all(|index| unsafe { automatic_passenger_gunner(get(ai, index)) })
}

unsafe fn acquire_idle_mounts(owner: usize) {
    let begin = unsafe { q(owner, GUNNER_GUNS_BEGIN) };
    let end = unsafe { q(owner, GUNNER_GUNS_END) };
    if begin == 0 || end < begin || (end - begin) % 8 != 0 {
        return;
    }
    let count = (end - begin) / 8;
    if count > GUNNER_GUN_LIMIT {
        return;
    }
    // Read the bindings after the native tick: readiness is not called while
    // idle or deploying, and the tick can replace passenger bindings.
    for index in 0..count {
        let gun = unsafe { q(begin + index * 8, 0) };
        if gun != 0 && unsafe { should_acquire(owner, gun) } && throttle(gun) {
            unsafe { retarget(owner, gun) };
        }
    }
}

/// The passenger firing loop checks deployment per gun. Let a ready bound gun
/// start that loop while other mounts deploy. Client progress reporting retains
/// aggregate deployment after the native tick advances the firing phase.
pub unsafe extern "system" fn deployment(
    owner: usize,
    progress: *mut f32,
    blocking: *mut u8,
) -> u8 {
    let original = DEPLOYMENT_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return 1;
    }
    let deploying =
        unsafe { std::mem::transmute::<usize, Deployment>(original)(owner, progress, blocking) };
    let context = CONTEXT.with(Cell::get);
    if deploying == 0
        || owner == 0
        || context.owner != owner
        || !context.firing_tick
        || unsafe { read_u32(owner, GUNNER_FIRING_PHASE) } != 0
        || unsafe { *((owner + 0x90) as *const u8) } != 0
        || (unsafe { *((owner + 0xf1) as *const u8) } != 0
            && unsafe { *((owner + 0x160) as *const u8) } == 0)
    {
        return deploying;
    }
    let begin = unsafe { q(owner, GUNNER_GUNS_BEGIN) };
    let end = unsafe { q(owner, GUNNER_GUNS_END) };
    let Some(bytes) = end.checked_sub(begin).filter(|bytes| bytes % 8 == 0) else {
        return deploying;
    };
    if begin == 0 || bytes / 8 > GUNNER_GUN_LIMIT {
        return deploying;
    }
    for index in 0..bytes / 8 {
        let gun = unsafe { q(begin + index * 8, 0) };
        if gun == 0 || !unsafe { bound_passenger(owner, gun, true) } {
            continue;
        }
        let method = unsafe { q(q(gun, 0), 0x200) };
        if method != 0 {
            let mut gun_progress = 1.0;
            let mut gun_blocking = 0;
            if unsafe {
                std::mem::transmute::<usize, Deployment>(method)(
                    gun,
                    &mut gun_progress,
                    &mut gun_blocking,
                )
            } == 0
            {
                return 0;
            }
        }
    }
    deploying
}

/// Native candidate selector hook. The native implementation sorts the
/// candidate array in place, so the private-view path retries against
/// one-element slices from that sorted array.
pub unsafe extern "system" fn choose(
    gunner: usize,
    output: *mut usize,
    candidates: *mut usize,
    count: usize,
    flags: usize,
) -> *mut usize {
    let original = CHOOSE_ORIGINAL.load(Ordering::Acquire);
    if original == 0 || output.is_null() || candidates.is_null() || count == 0 {
        return if original == 0 {
            output
        } else {
            unsafe {
                std::mem::transmute::<usize, Choose>(original)(
                    gunner, output, candidates, count, flags,
                )
            }
        };
    }
    let context = CONTEXT.with(Cell::get);
    if context.view != gunner || context.gun == 0 {
        return unsafe {
            std::mem::transmute::<usize, Choose>(original)(gunner, output, candidates, count, flags)
        };
    }
    let mut selected = 0usize;
    unsafe {
        std::mem::transmute::<usize, Choose>(original)(
            gunner,
            &mut selected,
            candidates,
            count,
            flags,
        )
    };
    let sorted = unsafe { std::slice::from_raw_parts(candidates, count) };
    let current = unsafe { q(context.gun, GUN_TARGET) };
    let current_entity = unsafe { target_identity(current) };
    let retained = retain_visible_target(
        current,
        current_entity,
        sorted,
        |entity| {
            if entity == 0 {
                return None;
            }
            let mut one = entity;
            let mut target = 0usize;
            unsafe {
                std::mem::transmute::<usize, Choose>(original)(
                    gunner,
                    &mut target,
                    &mut one,
                    1,
                    flags,
                )
            };
            (target != 0).then_some(target)
        },
        |left, right| unsafe { same_native_target(left, right) },
        |target| unsafe { candidate_works(context.gun, target) },
        |mut target| unsafe { discard(&mut target) },
    );
    if let Some(target) = retained {
        if selected != 0 {
            unsafe { discard(&mut selected) };
        }
        unsafe { *output = target };
        return output;
    }
    if selected != 0 {
        if unsafe { candidate_works(context.gun, selected) } {
            unsafe { *output = selected };
            return output;
        }
        unsafe { discard(&mut selected) };
    }
    let target = first_usable_candidate(
        sorted,
        |entity| {
            if entity == 0 {
                return None;
            }
            let mut one = entity;
            let mut target = 0usize;
            unsafe {
                std::mem::transmute::<usize, Choose>(original)(
                    gunner,
                    &mut target,
                    &mut one,
                    1,
                    flags,
                )
            };
            (target != 0).then_some(target)
        },
        |target| unsafe { candidate_works(context.gun, *target) },
        |mut target| unsafe { discard(&mut target) },
    );
    if let Some(target) = target {
        unsafe { *output = target };
        return output;
    }
    unsafe { *output = 0 };
    output
}

/// Keep automatic passenger selections independent of the shared Gunner target.
/// Null assignments and explicit orders retain the native setter and its flag.
pub unsafe extern "system" fn shared_target(gun: usize, target: *mut usize, explicit: u8) {
    if target.is_null() {
        return;
    }
    let context = CONTEXT.with(Cell::get);
    let independent = explicit == 0
        && context.owner != 0
        && unsafe { automatic_intent(context.owner) }
        && unsafe { passenger_source(context.owner, gun) }.is_some();
    // The native shared-target refresh clears every gun through vfunc_7.
    // Private passenger targets survive that broadcast, including cooldown;
    // standalone Stop, cancellation and invalid-target clears still run.
    if independent && context.shared_refresh && unsafe { *target } == 0 {
        let current = unsafe { q(gun, GUN_TARGET) };
        if current != 0 && unsafe { q(current, 0x10) } != 0 {
            return;
        }
    }
    let shared = if independent {
        unsafe { q(context.owner, 0x98) }
    } else {
        0
    };
    route_shared_target(
        independent,
        shared,
        unsafe { &mut *target },
        |target| {
            let original = SET_TARGET.load(Ordering::Acquire);
            if original != 0 {
                unsafe {
                    std::mem::transmute::<usize, AssignTarget>(original)(gun, target, explicit)
                };
            }
        },
        |target| unsafe { discard(target) },
    );
}

/// Scope the automatic shared-target helper without changing its Gunner state
/// updates, target ownership or firing-phase reset.
pub unsafe extern "system" fn shared_refresh(owner: usize, target: *mut usize) {
    let original = SHARED_REFRESH_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return;
    }
    let previous = CONTEXT.with(|context| {
        let old = context.get();
        context.set(HookContext {
            owner,
            firing_tick: old.owner == owner && old.firing_tick,
            shared_refresh: true,
            ..HookContext::default()
        });
        old
    });
    unsafe { std::mem::transmute::<usize, SharedRefresh>(original)(owner, target) };
    CONTEXT.with(|context| context.set(previous));
}

/// Native attack dispatch supplies zero for automatic orders and one for the
/// explicit Attack command. Record that intent before its shared broadcast.
pub unsafe extern "system" fn command(owner: usize, target: *mut usize, explicit: u8) {
    let original = COMMAND_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return;
    }
    let begin = unsafe { q(owner, GUNNER_GUNS_BEGIN) };
    let end = unsafe { q(owner, GUNNER_GUNS_END) };
    let count = end
        .checked_sub(begin)
        .filter(|bytes| bytes % 8 == 0)
        .map(|bytes| bytes / 8);
    let passenger = begin != 0
        && count.is_some_and(|count| {
            count <= GUNNER_GUN_LIMIT
                && (0..count).any(|index| {
                    let gun = unsafe { q(begin + index * 8, 0) };
                    gun != 0 && unsafe { passenger_source(owner, gun) }.is_some()
                })
        });
    if let Ok(mut orders) = ORDER_MODES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        if passenger && !target.is_null() && unsafe { *target } != 0 {
            // Numeric identity checks retain no target reference and prevent an
            // old record from following a reused Gunner or a different order.
            let now = Instant::now();
            if orders.len() >= 4096 {
                orders.retain(|_, order| {
                    now.duration_since(order.last_seen) < Duration::from_secs(300)
                });
            }
            if orders.len() < 4096 || orders.contains_key(&owner) {
                orders.insert(
                    owner,
                    OrderMode {
                        entity: unsafe { q(owner, 0x18) },
                        target: unsafe { *target },
                        automatic: explicit == 0,
                        explicit_order: 0,
                        last_seen: now,
                    },
                );
            }
        } else {
            orders.remove(&owner);
        }
    }
    let previous = CONTEXT.with(|context| {
        context.replace(HookContext {
            owner,
            ..HookContext::default()
        })
    });
    unsafe { std::mem::transmute::<usize, AssignTarget>(original)(owner, target, explicit) };
    CONTEXT.with(|context| context.set(previous));
    if explicit != 0 && !target.is_null() && unsafe { *target } != 0 {
        // The order may need another gun: a target inside a rifle's minimum
        // range. The next tick rebinds, outside the native order broadcast.
        rebind::force(owner);
    }
}

fn route_shared_target(
    independent: bool,
    shared: usize,
    target: &mut usize,
    mut forward: impl FnMut(&mut usize),
    mut discard: impl FnMut(&mut usize),
) {
    // Broadcasts copy the Gunner's PtrJunction and increment its references.
    // A different handle retains the native per-gun assignment path.
    if independent && *target != 0 && *target == shared {
        discard(target);
    } else {
        forward(target);
    }
}

unsafe fn retarget(owner: usize, gun: usize) {
    let query = QUERY.load(Ordering::Acquire);
    if query == 0 {
        return;
    }
    let mut view = View([0; GUNNER_VIEW_SIZE]);
    unsafe {
        std::ptr::copy_nonoverlapping(owner as *const u8, view.0.as_mut_ptr(), GUNNER_VIEW_SIZE)
    };
    let mut one_gun = [gun];
    set_single_gun_view(&mut view, &mut one_gun);
    let view_ptr = view.0.as_mut_ptr() as usize;
    let previous = CONTEXT.with(|context| {
        let old = context.get();
        context.set(HookContext {
            owner,
            view: view_ptr,
            gun,
            firing_tick: false,
            shared_refresh: false,
        });
        old
    });
    let mut target = 0usize;
    unsafe { std::mem::transmute::<usize, Query>(query)(view_ptr, &mut target) };
    CONTEXT.with(|context| context.set(previous));
    let old_handle = unsafe { q(gun, GUN_TARGET) };
    apply_selected_target(
        old_handle,
        &mut target,
        |current, selected| unsafe { same_native_target(current, selected) },
        |selected| unsafe { set_target(gun, selected) },
        |selected| unsafe { discard(selected) },
    );
}

unsafe fn candidate_works(gun: usize, target: usize) -> bool {
    let range = IN_RANGE.load(Ordering::Acquire);
    if range == 0 {
        return false;
    }
    let mut check = unsafe { clone_owned(target) };
    if check == 0 {
        return false;
    }
    let in_range = unsafe { std::mem::transmute::<usize, TargetTest>(range)(gun, &mut check) != 0 };
    // vfunc_60 consumes `check`.
    in_range
}

unsafe fn same_native_target(left: usize, right: usize) -> bool {
    let equals = TARGET_EQUALS.load(Ordering::Acquire);
    if equals == 0 {
        return false;
    }
    let mut left_clone = unsafe { clone_owned(left) };
    let mut right_clone = unsafe { clone_owned(right) };
    if left_clone == 0 || right_clone == 0 {
        if left_clone != 0 {
            unsafe { discard(&mut left_clone) };
        }
        if right_clone != 0 {
            unsafe { discard(&mut right_clone) };
        }
        return false;
    }
    // FUN_1801124f0 consumes both owned copies through the normal
    // PtrJunction release path after comparing resolved target identity.
    unsafe {
        std::mem::transmute::<usize, TargetEquals>(equals)(&mut left_clone, &mut right_clone) & 0xff
            != 0
    }
}

unsafe fn set_target(gun: usize, target: &mut usize) -> bool {
    let setter = SET_TARGET.load(Ordering::Acquire);
    if setter == 0 {
        return false;
    }
    unsafe { std::mem::transmute::<usize, AssignTarget>(setter)(gun, target, 0) };
    // Gun::vfunc_5 consumes the supplied owning handle but leaves the local
    // pointer value unchanged; clear it to avoid a second release.
    *target = 0;
    true
}

fn inside_module(base: usize, size: usize, address: usize) -> bool {
    base != 0 && size != 0 && address >= base && address < base.saturating_add(size)
}

unsafe fn should_acquire(owner: usize, gun: usize) -> bool {
    unsafe { bound_passenger(owner, gun, automatic_intent(owner)) }
}

unsafe fn automatic_intent(owner: usize) -> bool {
    let native_automatic = unsafe { read_u32(owner, GUNNER_AUTOMATIC) } == 1;
    let reset = native_automatic && unsafe { *((owner + 0x90) as *const u8) } != 0;
    let current_order = ORDER_MODES
        .get()
        .and_then(|orders| orders.lock().ok())
        .and_then(|mut orders| {
            let Some(order) = orders.get_mut(&owner) else {
                return None;
            };
            let entity = unsafe { q(owner, 0x18) };
            let target = unsafe { q(owner, 0x98) };
            if !reset && order.matches(entity, target) {
                order.last_seen = Instant::now();
                Some(order.allows(entity, target))
            } else {
                orders.remove(&owner);
                None
            }
        });
    // A current explicit order wins over mode one. The native reset marks
    // the Gunner suspended in that mode, cancelling the recorded order.
    current_order.unwrap_or(native_automatic)
}

unsafe fn bound_passenger(owner: usize, gun: usize, automatic: bool) -> bool {
    let Some(source) = (unsafe { passenger_source(owner, gun) }) else {
        return false;
    };
    // Enabled state gates new acquisition, but never the lifetime of a private
    // target or the mount's ownership of queued aim requests.
    let source_enabled = unsafe { *((source + GUN_ENABLED) as *const u8) } != 0;
    let gun_enabled = unsafe { *((gun + GUN_ENABLED) as *const u8) } != 0;
    should_acquire_from_facts(automatic, true, true, source_enabled, gun_enabled)
}

unsafe fn passenger_source(owner: usize, gun: usize) -> Option<usize> {
    let begin = unsafe { q(owner, GUNNER_GUNS_BEGIN) };
    let end = unsafe { q(owner, GUNNER_GUNS_END) };
    let sources = unsafe { q(owner, GUNNER_SOURCES_BEGIN) };
    let source_end = unsafe { q(owner, GUNNER_SOURCES_END) };
    if begin == 0 || end < begin || (end - begin) % 8 != 0 {
        return None;
    }
    let count = (end - begin) / 8;
    if count == 0
        || count > GUNNER_GUN_LIMIT
        || sources == 0
        || source_end < sources
        || source_end - sources != count * 8
        || unsafe { q(owner, 0x50) } != unsafe { q(owner, 0x58) }
    {
        return None;
    }
    let Some(index) = (0..count).find(|&i| unsafe { q(begin + i * 8, 0) } == gun) else {
        return None;
    };
    let source = unsafe { q(sources + index * 8, 0) };
    if source == 0 {
        return None;
    }
    let live_source = source != 0
        && unsafe { q(source, 0) } != 0
        && unsafe { q(source, 0) } == unsafe { q(gun, 0) }
        && unsafe { q(source, 0x40) } != 0
        && unsafe { q(source, 0x40) } == unsafe { q(gun, 0x40) };
    live_source.then_some(source)
}

unsafe fn clone_owned(handle: usize) -> usize {
    if handle == 0 {
        return 0;
    }
    let refs = (handle + 8) as *mut i32;
    unsafe { refs.write_unaligned(refs.read_unaligned().wrapping_add(1)) };
    if unsafe { q(handle, 0x10) } != 0 {
        let aux = (handle + 0x18) as *mut i32;
        unsafe { aux.write_unaligned(aux.read_unaligned().wrapping_add(1)) };
    }
    handle
}

unsafe fn discard(handle: &mut usize) {
    let release = RELEASE_HANDLE.load(Ordering::Acquire);
    discard_with(handle, |handle| {
        if release != 0 {
            unsafe { std::mem::transmute::<usize, ReleaseHandle>(release)(handle) };
        }
    });
}

fn discard_with(handle: &mut usize, mut release: impl FnMut(&mut usize)) {
    if *handle != 0 {
        release(handle);
    }
    *handle = 0;
}

#[repr(align(16))]
struct View([u8; GUNNER_VIEW_SIZE]);

fn set_single_gun_view(view: &mut View, guns: &mut [usize; 1]) {
    let begin = guns.as_mut_ptr() as usize;
    let end = unsafe { guns.as_mut_ptr().add(1) as usize };
    put_usize(&mut view.0, GUNNER_GUNS_BEGIN, begin);
    put_usize(&mut view.0, GUNNER_GUNS_END, end);
    put_usize(&mut view.0, GUNNER_GUNS_CAP, end);
}

fn put_usize(bytes: &mut [u8], offset: usize, value: usize) {
    bytes[offset..offset + 8].copy_from_slice(&(value as u64).to_ne_bytes());
}

#[cfg(test)]
fn read_usize(bytes: &[u8], offset: usize) -> usize {
    usize::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

unsafe fn q(address: usize, offset: usize) -> usize {
    unsafe { ((address + offset) as *const usize).read_unaligned() }
}

unsafe fn read_u32(address: usize, offset: usize) -> u32 {
    unsafe { ((address + offset) as *const u32).read_unaligned() }
}

fn throttle(gun: usize) -> bool {
    struct Schedule {
        guns: HashMap<usize, Instant>,
        cleanup: Instant,
    }
    static LAST: OnceLock<Mutex<Schedule>> = OnceLock::new();
    let now = Instant::now();
    let mut schedule = LAST
        .get_or_init(|| {
            Mutex::new(Schedule {
                guns: HashMap::new(),
                cleanup: now,
            })
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if now.saturating_duration_since(schedule.cleanup) >= Duration::from_secs(10) {
        schedule
            .guns
            .retain(|_, at| now.saturating_duration_since(*at) < Duration::from_secs(10));
        schedule.cleanup = now;
    }
    let last = &mut schedule.guns;
    if should_throttle_at(last.get(&gun).copied(), now) {
        return false;
    }
    last.insert(gun, now);
    if last.len() > 4096 {
        if let Some(oldest) = last.iter().min_by_key(|(_, at)| **at).map(|(key, _)| *key) {
            last.remove(&oldest);
        }
    }
    true
}

fn should_throttle_at(previous: Option<Instant>, now: Instant) -> bool {
    previous.map_or(false, |at| {
        now.saturating_duration_since(at) < Duration::from_secs(1)
    })
}

fn first_usable_candidate<C: Copy, H>(
    candidates: &[C],
    mut select: impl FnMut(C) -> Option<H>,
    mut usable: impl FnMut(&H) -> bool,
    mut reject: impl FnMut(H),
) -> Option<H> {
    for &candidate in candidates {
        let Some(target) = select(candidate) else {
            continue;
        };
        if usable(&target) {
            return Some(target);
        }
        reject(target);
    }
    None
}

fn retain_visible_target(
    current: usize,
    current_entity: usize,
    candidates: &[usize],
    mut select: impl FnMut(usize) -> Option<usize>,
    mut same: impl FnMut(usize, usize) -> bool,
    mut works: impl FnMut(usize) -> bool,
    mut discard: impl FnMut(usize),
) -> Option<usize> {
    if current == 0 || current_entity == 0 {
        return None;
    }
    for &candidate in candidates {
        if candidate != current_entity {
            continue;
        }
        let Some(target) = select(candidate) else {
            return None;
        };
        if same(current, target) && works(target) {
            return Some(target);
        }
        discard(target);
        return None;
    }
    None
}

unsafe fn target_identity(handle: usize) -> usize {
    if handle == 0 {
        return 0;
    }
    let base = LOGIC_BASE.load(Ordering::Acquire);
    let size = LOGIC_SIZE.load(Ordering::Acquire);
    let object = unsafe { q(handle, 0x10) };
    if object == 0 {
        return 0;
    }
    let vtable = unsafe { q(object, 0) };
    if vtable == 0 {
        return 0;
    }
    let getter = unsafe { q(vtable, 0x58) };
    if !inside_module(base, size, getter) {
        return 0;
    }
    unsafe { std::mem::transmute::<usize, TargetIdentity>(getter)(object) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fleet_of_passenger_mounts_keeps_its_acquisition_throttle() {
        for gun in 1..=256 {
            assert!(throttle(gun));
        }
        for gun in 1..=256 {
            assert!(!throttle(gun));
        }
    }

    #[repr(align(16))]
    struct MockObject<const N: usize>([u8; N]);

    static IDLE_TICKS: AtomicUsize = AtomicUsize::new(0);
    static IDLE_QUERIES: AtomicUsize = AtomicUsize::new(0);

    static MOVE_TRANSITIONS: AtomicUsize = AtomicUsize::new(0);
    static MOVE_RELEASES: AtomicUsize = AtomicUsize::new(0);
    static NATIVE_QUERIES: AtomicUsize = AtomicUsize::new(0);
    static NATIVE_CAPABLE: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "system" fn move_test_getter(object: usize) -> usize {
        unsafe { q(object, 8) }
    }

    unsafe extern "system" fn move_test_gunner(ai: usize, index: usize) -> usize {
        unsafe { q(q(ai, 0x10), index * 8) }
    }

    unsafe extern "system" fn move_test_release(target: *mut usize) {
        assert_ne!(unsafe { *target }, 0);
        MOVE_RELEASES.fetch_add(1, Ordering::SeqCst);
        unsafe { *target = 0 };
    }

    unsafe extern "system" fn move_test_transition(_state: usize, target: *mut usize) {
        MOVE_TRANSITIONS.fetch_add(1, Ordering::SeqCst);
        if !target.is_null() && unsafe { *target } != 0 {
            unsafe { move_test_release(target) };
        }
    }

    unsafe extern "system" fn native_test_query(
        output: *mut usize,
        _entity: usize,
        _mode: u8,
    ) -> *mut usize {
        NATIVE_QUERIES.fetch_add(1, Ordering::SeqCst);
        unsafe { output.write(0xface) };
        output
    }

    unsafe extern "system" fn native_test_capable(_entity: usize, _enemy: usize) -> u8 {
        NATIVE_CAPABLE.fetch_add(1, Ordering::SeqCst);
        1
    }

    #[test]
    fn passenger_only_vehicles_report_no_combat_candidate() {
        let globals = [
            &MOVE_ACQUIRE_ORIGINAL,
            &CANDIDATE_QUERY_ORIGINAL,
            &CAPABLE_ORIGINAL,
            &RELEASE_HANDLE,
            &crate::orders::GUNNER_COUNT,
            &crate::orders::GUNNER_GET,
        ];
        let saved = globals.map(|slot| slot.load(Ordering::Acquire));
        MOVE_ACQUIRE_ORIGINAL.store(
            move_test_transition as *const () as usize,
            Ordering::Release,
        );
        CANDIDATE_QUERY_ORIGINAL.store(native_test_query as *const () as usize, Ordering::Release);
        CAPABLE_ORIGINAL.store(native_test_capable as *const () as usize, Ordering::Release);
        RELEASE_HANDLE.store(move_test_release as *const () as usize, Ordering::Release);
        crate::orders::GUNNER_COUNT.store(0x140, Ordering::Release);
        crate::orders::GUNNER_GET.store(0x130, Ordering::Release);

        let mut state = MockObject([0u8; 0x28]);
        let mut controller = MockObject([0u8; 0x18]);
        let mut controller_handle = MockObject([0u8; 0x20]);
        let mut entity = MockObject([0u8; 0x10]);
        let mut facets = MockObject([0u8; 0x30]);
        let mut ai = MockObject([0u8; 0xf0]);
        let mut owner = MockObject([0u8; GUNNER_VIEW_SIZE]);
        let mut hull_owner = MockObject([0u8; GUNNER_VIEW_SIZE]);
        let mut gun = MockObject([0u8; 0x150]);
        let mut source = MockObject([0u8; 0x150]);
        let descriptor = MockObject([0u8; 0x28]);
        let mut guns = [gun.0.as_mut_ptr() as usize];
        let sources = [source.0.as_mut_ptr() as usize];
        let gunners = [
            owner.0.as_mut_ptr() as usize,
            hull_owner.0.as_mut_ptr() as usize,
        ];
        let mut controller_table = [0usize; 11];
        let mut entity_table = [0usize; 23];
        let mut ai_table = [0usize; 41];
        controller_table[10] = move_test_getter as *const () as usize;
        entity_table[22] = move_test_getter as *const () as usize;
        ai_table[40] = move_test_getter as *const () as usize;
        ai_table[38] = move_test_gunner as *const () as usize;
        put_usize(&mut controller.0, 0, controller_table.as_ptr() as usize);
        put_usize(&mut controller.0, 8, entity.0.as_ptr() as usize);
        put_usize(
            &mut controller_handle.0,
            0x10,
            controller.0.as_ptr() as usize,
        );
        put_usize(&mut state.0, 0x10, controller_handle.0.as_ptr() as usize);
        put_usize(&mut entity.0, 0, entity_table.as_ptr() as usize);
        put_usize(&mut entity.0, 8, facets.0.as_ptr() as usize);
        put_usize(&mut facets.0, 0x28, ai.0.as_ptr() as usize);
        put_usize(&mut ai.0, 0, ai_table.as_ptr() as usize);
        put_usize(&mut ai.0, 8, 1);
        put_usize(&mut ai.0, 0x10, gunners.as_ptr() as usize);
        for object in [&mut gun.0, &mut source.0] {
            put_usize(object, 0, 0x9999);
            put_usize(object, 0x40, descriptor.0.as_ptr() as usize);
        }
        for object in [&mut owner.0, &mut hull_owner.0] {
            put_usize(object, GUNNER_GUNS_BEGIN, guns.as_ptr() as usize);
            put_usize(object, GUNNER_GUNS_END, unsafe { guns.as_ptr().add(1) }
                as usize);
            put_usize(object, GUNNER_SOURCES_BEGIN, sources.as_ptr() as usize);
            put_usize(
                object,
                GUNNER_SOURCES_END,
                unsafe { sources.as_ptr().add(1) } as usize,
            );
            put_usize(object, GUNNER_AUTOMATIC, 1);
        }
        let state_ptr = state.0.as_ptr() as usize;
        let entity_ptr = entity.0.as_ptr() as usize;
        let untouched = state.0;
        let verify = |passenger_only: bool| {
            MOVE_TRANSITIONS.store(0, Ordering::SeqCst);
            MOVE_RELEASES.store(0, Ordering::SeqCst);
            let mut target = 0xabcd;
            unsafe { move_acquire(state_ptr, &mut target) };
            // The candidate's ownership ends exactly once on either path.
            assert_eq!(
                MOVE_TRANSITIONS.load(Ordering::SeqCst),
                usize::from(!passenger_only)
            );
            assert_eq!(MOVE_RELEASES.load(Ordering::SeqCst), 1);
            assert_eq!(target, 0);
            assert_eq!(state.0, untouched);
            NATIVE_QUERIES.store(0, Ordering::SeqCst);
            for mode in [0, 1] {
                let mut output = usize::MAX;
                let output_pointer = &mut output as *mut usize;
                assert_eq!(
                    unsafe { candidate_query(output_pointer, entity_ptr, mode) },
                    output_pointer
                );
                assert_eq!(output, if passenger_only { 0 } else { 0xface });
            }
            assert_eq!(
                NATIVE_QUERIES.load(Ordering::SeqCst),
                if passenger_only { 0 } else { 2 }
            );
            NATIVE_CAPABLE.store(0, Ordering::SeqCst);
            assert_eq!(
                unsafe { capable(entity_ptr, 0x7777) },
                u8::from(!passenger_only)
            );
            assert_eq!(
                NATIVE_CAPABLE.load(Ordering::SeqCst),
                usize::from(!passenger_only)
            );
        };
        // Enabled/deployment state does not surrender the driver's Move.
        gun.0[GUN_ENABLED] = 0;
        source.0[GUN_ENABLED] = 0;
        verify(true);
        assert_eq!(source.0[GUN_ENABLED], 0);
        put_usize(&mut owner.0, GUNNER_AUTOMATIC, 0);
        verify(false); // An explicit firing mode retains native combat AI.
        put_usize(&mut owner.0, GUNNER_AUTOMATIC, 1);
        put_usize(&mut owner.0, 0x18, 0xaaaa);
        put_usize(&mut owner.0, 0x98, 0xbbbb);
        let modes = ORDER_MODES.get_or_init(|| Mutex::new(HashMap::new()));
        modes.lock().unwrap().insert(
            gunners[0],
            OrderMode {
                entity: 0xaaaa,
                target: 0xbbbb,
                automatic: false,
                explicit_order: 0,
                last_seen: Instant::now(),
            },
        );
        verify(false); // Recorded explicit orders also win over mode one.
                       // The AI's top attack state, with its order.
        let mut first_order = MockObject([0u8; 0x18]);
        let mut second_order = MockObject([0u8; 0x18]);
        let mut order_handle = MockObject([0u8; 0x18]);
        let mut attack = MockObject([0u8; 0x28]);
        let mut attack_handle = MockObject([0u8; 0x18]);
        let mut machine = MockObject([0u8; 0x48]);
        let mut machine_handle = MockObject([0u8; 0x18]);
        let mut attack_table = [0usize; 10];
        attack_table[9] = move_test_getter as *const () as usize;
        let states = [attack_handle.0.as_ptr() as usize];
        put_usize(&mut attack.0, 0, attack_table.as_ptr() as usize);
        put_usize(&mut attack.0, 8, AI_ATTACK_STATE as usize);
        put_usize(&mut attack.0, 0x20, order_handle.0.as_ptr() as usize);
        put_usize(&mut attack_handle.0, 0x10, attack.0.as_ptr() as usize);
        put_usize(&mut machine.0, 0x38, states.as_ptr() as usize);
        put_usize(&mut machine.0, 0x40, unsafe { states.as_ptr().add(1) }
            as usize);
        put_usize(&mut machine_handle.0, 0x10, machine.0.as_ptr() as usize);
        put_usize(
            &mut ai.0,
            AI_STATE_MACHINE,
            machine_handle.0.as_ptr() as usize,
        );
        let set_order =
            |handle: &mut MockObject<0x18>, order: &mut MockObject<0x18>, flags: u32| {
                order.0[0x14..0x18].copy_from_slice(&flags.to_le_bytes());
                put_usize(&mut handle.0, 0x10, order.0.as_ptr() as usize);
            };
        set_order(&mut order_handle, &mut first_order, ORDER_EXPLICIT);
        verify(false); // The explicit order is captured.
        set_order(&mut order_handle, &mut first_order, ORDER_AUTOMATIC_ATTACK);
        verify(false); // The same order is never a replacement.
        set_order(
            &mut order_handle,
            &mut second_order,
            ORDER_AUTOMATIC_ATTACK | ORDER_EXPLICIT,
        );
        verify(false); // Neither is an explicit replacement.
        set_order(&mut order_handle, &mut second_order, ORDER_AUTOMATIC_ATTACK);
        put_usize(&mut attack.0, 8, 0x10);
        verify(false); // Nor an order outside an attack state.
        put_usize(&mut attack.0, 8, AI_ATTACK_WITH_MOVE_STATE as usize);
        verify(true); // The AI's own Attack after a Move expires the record.
        set_order(&mut order_handle, &mut first_order, ORDER_EXPLICIT);
        verify(true); // Expiry is final for the record.
        put_usize(&mut ai.0, AI_STATE_MACHINE, 0);
        modes.lock().unwrap().remove(&gunners[0]);
        verify(true);
        put_usize(&mut ai.0, 8, 2);
        put_usize(&mut hull_owner.0, GUNNER_SOURCES_BEGIN, 0);
        verify(false); // Every gunner must belong to passenger weapons.
        put_usize(&mut ai.0, 8, 0);
        verify(false); // A unit without Gunners keeps native behavior.
        put_usize(&mut ai.0, 8, 1);
        guns[0] = 0;
        verify(false); // Empty bindings do not change native vehicle behavior.
        guns[0] = gun.0.as_ptr() as usize;
        verify(true);
        // Null and empty inputs remain native calls, with no spurious release.
        MOVE_TRANSITIONS.store(0, Ordering::SeqCst);
        MOVE_RELEASES.store(0, Ordering::SeqCst);
        let mut empty = 0;
        unsafe {
            move_acquire(state_ptr, &mut empty);
            move_acquire(state_ptr, std::ptr::null_mut());
        }
        assert_eq!(MOVE_TRANSITIONS.load(Ordering::SeqCst), 2);
        assert_eq!(MOVE_RELEASES.load(Ordering::SeqCst), 0);
        // A state without a controller, or an entity without facets, is not
        // passenger-only.
        put_usize(&mut state.0, 0x10, 0);
        MOVE_TRANSITIONS.store(0, Ordering::SeqCst);
        let mut target = 0xabcd;
        unsafe { move_acquire(state_ptr, &mut target) };
        assert_eq!(MOVE_TRANSITIONS.load(Ordering::SeqCst), 1);
        assert_eq!(unsafe { capable(0, 0x7777) }, 1);
        for (slot, value) in globals.into_iter().zip(saved) {
            slot.store(value, Ordering::Release);
        }
    }

    #[test]
    fn automatic_passenger_aim_preserves_the_driver_and_restores_mount_flags() {
        let mut owner = MockObject([0u8; GUNNER_VIEW_SIZE]);
        let mut mount = MockObject([0u8; 0x100]);
        let mut smg = MockObject([0u8; 0x150]);
        let mut javelin = MockObject([0u8; 0x150]);
        let mut smg_source = MockObject([0u8; 0x150]);
        let mut javelin_source = MockObject([0u8; 0x150]);
        let mut empty = MockObject([0u8; 0x150]);
        let mut smg_descriptor = MockObject([0u8; 0x28]);
        let mut javelin_descriptor = MockObject([0u8; 0x28]);
        let mut dummy_descriptor = MockObject([0u8; 0x28]);
        let dummy_name = b"passenger_dummy_gun";
        put_usize(&mut dummy_descriptor.0, 8, dummy_name.as_ptr() as usize);
        put_usize(&mut dummy_descriptor.0, 0x18, dummy_name.len());
        put_usize(&mut dummy_descriptor.0, 0x20, 31);
        put_usize(&mut empty.0, 0x40, dummy_descriptor.0.as_ptr() as usize);
        let mut guns = [
            smg.0.as_mut_ptr() as usize,
            javelin.0.as_mut_ptr() as usize,
            empty.0.as_mut_ptr() as usize,
        ];
        let mut sources = [
            smg_source.0.as_mut_ptr() as usize,
            javelin_source.0.as_mut_ptr() as usize,
            0,
        ];
        let ptr = owner.0.as_mut_ptr() as usize;
        put_usize(&mut owner.0, GUNNER_MOUNT, mount.0.as_mut_ptr() as usize);
        put_usize(&mut owner.0, GUNNER_GUNS_BEGIN, guns.as_mut_ptr() as usize);
        put_usize(
            &mut owner.0,
            GUNNER_GUNS_END,
            unsafe { guns.as_ptr().add(3) } as usize,
        );
        put_usize(
            &mut owner.0,
            GUNNER_SOURCES_BEGIN,
            sources.as_mut_ptr() as usize,
        );
        put_usize(
            &mut owner.0,
            GUNNER_SOURCES_END,
            unsafe { sources.as_ptr().add(3) } as usize,
        );
        for (gun, source, description) in [
            (
                &mut smg.0,
                &mut smg_source.0,
                smg_descriptor.0.as_mut_ptr() as usize,
            ),
            (
                &mut javelin.0,
                &mut javelin_source.0,
                javelin_descriptor.0.as_mut_ptr() as usize,
            ),
        ] {
            for object in [gun, source] {
                put_usize(object, 0, 0x1234);
                put_usize(object, 0x40, description);
                object[GUN_ENABLED] = 1;
            }
        }
        put_usize(&mut owner.0, 0x18, 0xaaaa);
        put_usize(&mut owner.0, 0x98, 0xbbbb);
        let orders = ORDER_MODES.get_or_init(|| Mutex::new(HashMap::new()));
        orders.lock().unwrap().insert(
            ptr,
            OrderMode {
                entity: 0xaaaa,
                target: 0xbbbb,
                automatic: true,
                explicit_order: 0,
                last_seen: Instant::now(),
            },
        );
        let flag = unsafe { mount.0.as_ptr().add(MOUNT_INDEPENDENT_AIM) };
        let verify = |expected| {
            let mut updates = 0;
            unsafe {
                update_without_passenger_chassis_aim(ptr, || {
                    assert_eq!(*flag, expected);
                    updates += 1;
                })
            };
            assert_eq!(updates, 1);
        };
        // Automatic native mode zero enables the chassis-turn fallback;
        // suppress it for both bound guns even while the Javelin deploys.
        javelin.0[0x147] = 1;
        verify(1);
        assert_eq!(mount.0[MOUNT_INDEPENDENT_AIM], 0);
        mount.0[MOUNT_INDEPENDENT_AIM] = 7;
        verify(1);
        assert_eq!(mount.0[MOUNT_INDEPENDENT_AIM], 7);
        mount.0[MOUNT_INDEPENDENT_AIM] = 0;

        // Cooling down or disabling a passenger gun cannot give queued aim
        // requests control of the driver. Acquisition still observes e2.
        smg_source.0[GUN_ENABLED] = 0;
        verify(1);
        assert!(!unsafe { should_acquire(ptr, guns[0]) });
        smg_source.0[GUN_ENABLED] = 1;
        assert_eq!(smg_source.0[GUN_ENABLED], 1);
        assert!(unsafe { should_acquire(ptr, guns[0]) });
        smg.0[GUN_ENABLED] = 0;
        javelin.0[GUN_ENABLED] = 0;
        verify(1);
        assert!(!unsafe { should_acquire(ptr, guns[1]) });
        smg.0[GUN_ENABLED] = 1;
        assert_eq!(smg.0[GUN_ENABLED], 1);
        javelin.0[GUN_ENABLED] = 1;
        assert!(unsafe { should_acquire(ptr, guns[0]) });

        orders.lock().unwrap().get_mut(&ptr).unwrap().automatic = false;
        verify(0);
        orders.lock().unwrap().get_mut(&ptr).unwrap().automatic = true;
        // Unbound real guns and mismatched bindings retain chassis control.
        sources[1] = 0;
        verify(0);
        sources[1] = javelin_source.0.as_mut_ptr() as usize;
        assert_ne!(sources[1], 0);
        put_usize(&mut javelin.0, 0x40, smg_descriptor.0.as_ptr() as usize);
        verify(0);
        put_usize(&mut javelin.0, 0x40, javelin_descriptor.0.as_ptr() as usize);
        empty.0[GUN_ENABLED] = 1;
        verify(1);
        empty.0[GUN_ENABLED] = 0;
        put_usize(&mut empty.0, 0x40, smg_descriptor.0.as_ptr() as usize);
        verify(0);
        put_usize(&mut empty.0, 0x40, dummy_descriptor.0.as_ptr() as usize);
        assert_eq!(empty.0[GUN_ENABLED], 0);
        put_usize(&mut owner.0, 0x40, 8);
        verify(0);
        put_usize(&mut owner.0, 0x40, 0);
        verify(1);
        orders.lock().unwrap().remove(&ptr);
        verify(0);
        put_usize(&mut owner.0, GUNNER_AUTOMATIC, 1);
        verify(1);
        assert_eq!(mount.0[MOUNT_INDEPENDENT_AIM], 0);
        put_usize(&mut mount.0, 0xd8, 0x1234);
        verify(0);
        put_usize(&mut mount.0, 0xd8, 0);
        verify(1);
        put_usize(&mut owner.0, GUNNER_MOUNT, 0);
        verify(0);
    }

    unsafe extern "system" fn mock_gun_deployment(
        gun: usize,
        progress: *mut f32,
        blocking: *mut u8,
    ) -> u8 {
        unsafe {
            progress.write(*((gun + 0x14c) as *const f32));
            blocking.write(1);
            *((gun + 0x147) as *const u8)
        }
    }

    unsafe extern "system" fn mock_gunner_deployment(
        owner: usize,
        progress: *mut f32,
        blocking: *mut u8,
    ) -> u8 {
        let begin = unsafe { q(owner, GUNNER_GUNS_BEGIN) };
        let end = unsafe { q(owner, GUNNER_GUNS_END) };
        let mut deploying = 0;
        unsafe {
            progress.write(1.0);
            blocking.write(0)
        };
        for index in 0..(end - begin) / 8 {
            let mut gun_progress = 1.0;
            let mut gun_blocking = 0;
            let pending = unsafe {
                mock_gun_deployment(
                    q(begin + index * 8, 0),
                    &mut gun_progress,
                    &mut gun_blocking,
                )
            };
            if pending != 0 {
                deploying = 1;
                unsafe {
                    progress.write((*progress).min(gun_progress));
                    blocking.write(*blocking | gun_blocking);
                }
            }
        }
        deploying
    }

    #[test]
    fn passenger_primary_starts_while_secondary_deploys() {
        let mut owner = MockObject([0u8; GUNNER_VIEW_SIZE]);
        let mut secondary = MockObject([0u8; 0x158]);
        let mut primary = MockObject([0u8; 0x158]);
        let mut secondary_source = MockObject([0u8; 0x158]);
        let mut primary_source = MockObject([0u8; 0x158]);
        let mut vtable = [0usize; 65];
        vtable[64] = mock_gun_deployment as *const () as usize;
        for gun in [
            &mut secondary.0,
            &mut primary.0,
            &mut secondary_source.0,
            &mut primary_source.0,
        ] {
            put_usize(gun, 0, vtable.as_ptr() as usize);
            put_usize(gun, 0x40, 0x5678);
            gun[GUN_ENABLED] = 1;
        }
        secondary.0[0x147] = 1;
        secondary.0[0x14c..0x150].copy_from_slice(&0.25f32.to_ne_bytes());
        primary.0[0x14c..0x150].copy_from_slice(&1.0f32.to_ne_bytes());
        let mut guns = [
            secondary.0.as_mut_ptr() as usize,
            primary.0.as_mut_ptr() as usize,
        ];
        let mut sources = [
            secondary_source.0.as_mut_ptr() as usize,
            primary_source.0.as_mut_ptr() as usize,
        ];
        put_usize(&mut owner.0, GUNNER_GUNS_BEGIN, guns.as_mut_ptr() as usize);
        put_usize(
            &mut owner.0,
            GUNNER_GUNS_END,
            unsafe { guns.as_ptr().add(2) } as usize,
        );
        put_usize(
            &mut owner.0,
            GUNNER_SOURCES_BEGIN,
            sources.as_mut_ptr() as usize,
        );
        put_usize(
            &mut owner.0,
            GUNNER_SOURCES_END,
            unsafe { sources.as_ptr().add(2) } as usize,
        );
        let owner_ptr = owner.0.as_mut_ptr() as usize;
        let old = DEPLOYMENT_ORIGINAL.swap(
            mock_gunner_deployment as *const () as usize,
            Ordering::SeqCst,
        );
        let mut progress = 1.0;
        let mut blocking = 0;
        let mut check = || unsafe {
            let result = deployment(owner_ptr, &mut progress, &mut blocking);
            if read_u32(owner_ptr, GUNNER_FIRING_PHASE) == 1 {
                assert_eq!((progress, blocking), (0.75, 1));
            }
            result
        };

        // Calls outside this Gunner's update retain the aggregate deployment.
        assert_eq!(check(), 1);
        let previous = CONTEXT.with(|context| {
            context.replace(HookContext {
                owner: owner_ptr,
                firing_tick: true,
                ..HookContext::default()
            })
        });
        // Manual and automatic orders both use each mount's native readiness.
        for mode in [0, 1] {
            put_usize(&mut owner.0, GUNNER_AUTOMATIC, mode);
            assert_eq!(check(), 0);
        }
        assert_eq!(secondary.0[0x147], 1);
        // Startup advances the phase before sendDataToClient reports progress.
        // Reporting within the same tick must keep the pending secondary flag.
        put_usize(&mut owner.0, GUNNER_FIRING_PHASE, 1);
        secondary.0[0x14c..0x150].copy_from_slice(&0.75f32.to_ne_bytes());
        assert_eq!(check(), 1);
        put_usize(&mut owner.0, GUNNER_FIRING_PHASE, 0);
        assert_eq!(check(), 0);
        CONTEXT.with(|context| {
            let mut reporting = context.get();
            reporting.firing_tick = false;
            context.set(reporting);
        });
        assert_eq!(check(), 1);
        CONTEXT.with(|context| {
            let mut startup = context.get();
            startup.firing_tick = true;
            context.set(startup);
        });
        owner.0[0x90] = 1;
        assert_eq!(check(), 1);
        owner.0[0x90] = 0;
        owner.0[0xf1] = 1;
        assert_eq!(check(), 1);
        owner.0[0x160] = 1;
        assert_eq!(check(), 0);
        owner.0[0xf1] = 0;
        owner.0[0x160] = 0;
        // Missing, disabled, former, and unmatched sources cannot unlock firing.
        sources[1] = 0;
        assert_eq!(check(), 1);
        sources[1] = primary_source.0.as_mut_ptr() as usize;
        primary_source.0[GUN_ENABLED] = 0;
        assert_eq!(check(), 1);
        primary_source.0[GUN_ENABLED] = 1;
        primary.0[GUN_ENABLED] = 0;
        assert_eq!(check(), 1);
        primary.0[GUN_ENABLED] = 1;
        put_usize(&mut primary_source.0, 0, 0x1234);
        assert_eq!(check(), 1);
        put_usize(&mut primary_source.0, 0, vtable.as_ptr() as usize);
        put_usize(&mut primary_source.0, 0x40, 0x9999);
        assert_eq!(check(), 1);
        put_usize(&mut primary_source.0, 0x40, 0x5678);
        put_usize(&mut owner.0, GUNNER_SOURCES_END, sources.as_ptr() as usize);
        assert_eq!(check(), 1);
        put_usize(
            &mut owner.0,
            GUNNER_SOURCES_END,
            unsafe { sources.as_ptr().add(2) } as usize,
        );
        // Native alternate weapon-vector selection retains its own wait.
        put_usize(&mut owner.0, 0x58, 8);
        assert_eq!(check(), 1);
        put_usize(&mut owner.0, 0x58, 0);
        primary.0[0x147] = 1;
        assert_eq!(check(), 1);
        secondary.0[0x147] = 0;
        primary.0[0x147] = 0;
        assert_eq!(check(), 0);
        assert_eq!((secondary.0[0x147], primary.0[0x147]), (0, 0));
        drop(check);
        assert_eq!((progress, blocking), (1.0, 0));
        CONTEXT.with(|context| context.set(previous));
        DEPLOYMENT_ORIGINAL.store(old, Ordering::SeqCst);
    }

    static COMMAND_CALLS: AtomicUsize = AtomicUsize::new(0);
    static ASSIGN_CALLS: AtomicUsize = AtomicUsize::new(0);
    static ASSIGN_FLAG: AtomicUsize = AtomicUsize::new(0);
    static BROADCAST_RELEASES: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "system" fn mock_assign(gun: usize, target: *mut usize, flag: u8) {
        ASSIGN_CALLS.fetch_add(1, Ordering::SeqCst);
        ASSIGN_FLAG.store(flag as usize, Ordering::SeqCst);
        unsafe { ((gun + GUN_TARGET) as *mut usize).write_unaligned(*target) };
        unsafe { *target = 0 };
    }

    unsafe extern "system" fn mock_release_broadcast(target: *mut usize) {
        BROADCAST_RELEASES.fetch_add(1, Ordering::SeqCst);
        unsafe { *target = 0 };
    }

    unsafe extern "system" fn mock_command(owner: usize, target: *mut usize, flag: u8) {
        COMMAND_CALLS.fetch_add(1, Ordering::SeqCst);
        unsafe { ((owner + 0x98) as *mut usize).write_unaligned(*target) };
        unsafe { ((owner + GUNNER_AUTOMATIC) as *mut u32).write_unaligned(0) };
        let gun = unsafe { q(q(owner, GUNNER_GUNS_BEGIN), 0) };
        let mut copy = unsafe { *target };
        unsafe { shared_target(gun, &mut copy, flag) };
        unsafe { *target = 0 };
    }

    unsafe extern "system" fn mock_shared_refresh(owner: usize, target: *mut usize) {
        unsafe {
            (owner as *mut u8).add(0x90).write(0);
            ((owner + GUNNER_AUTOMATIC) as *mut u32).write_unaligned(1);
            ((owner + 0x98) as *mut usize).write_unaligned(*target);
            let gun = q(q(owner, GUNNER_GUNS_BEGIN), 0);
            let mut clear = 0;
            shared_target(gun, &mut clear, 0);
            ((owner + GUNNER_FIRING_PHASE) as *mut u32).write_unaligned(0);
            *target = 0;
        }
    }

    #[test]
    fn shared_refresh_keeps_private_passenger_targets_but_not_stop_or_invalid_targets() {
        let mut owner = MockObject([0u8; GUNNER_VIEW_SIZE]);
        let mut gun = MockObject([0u8; 0x150]);
        let mut source = MockObject([0u8; 0x150]);
        let mut private = MockObject([0u8; 0x20]);
        put_usize(&mut private.0, 0x10, 0x1111);
        let private_ptr = private.0.as_mut_ptr() as usize;
        let owner_ptr = owner.0.as_mut_ptr() as usize;
        let guns = [gun.0.as_mut_ptr() as usize];
        let sources = [source.0.as_mut_ptr() as usize];
        for (offset, vector) in [(GUNNER_GUNS_BEGIN, &guns), (GUNNER_SOURCES_BEGIN, &sources)] {
            put_usize(&mut owner.0, offset, vector.as_ptr() as usize);
            put_usize(&mut owner.0, offset + 8, unsafe { vector.as_ptr().add(1) }
                as usize);
        }
        for object in [&mut gun.0, &mut source.0] {
            put_usize(object, 0, 0x1234);
            put_usize(object, 0x40, 0x5678);
        }
        put_usize(&mut gun.0, GUN_TARGET, private_ptr);
        gun.0[0x147] = 1;
        gun.0[0x14c..0x150].copy_from_slice(&0.625f32.to_ne_bytes());
        put_usize(&mut owner.0, GUNNER_FIRING_PHASE, 1);
        let old_refresh = SHARED_REFRESH_ORIGINAL
            .swap(mock_shared_refresh as *const () as usize, Ordering::SeqCst);
        let old_assign = SET_TARGET.swap(mock_assign as *const () as usize, Ordering::SeqCst);
        let mut shared = 0xbbbb;
        unsafe { shared_refresh(owner_ptr, &mut shared) };
        assert_eq!(shared, 0);
        assert_eq!(read_usize(&gun.0, GUN_TARGET), private_ptr);
        assert_eq!(gun.0[0x147], 1);
        assert_eq!(
            f32::from_ne_bytes(gun.0[0x14c..0x150].try_into().unwrap()),
            0.625
        );
        assert_eq!(read_usize(&owner.0, 0x98), 0xbbbb);
        assert_eq!(read_usize(&owner.0, GUNNER_FIRING_PHASE), 0);
        assert!(!CONTEXT.with(Cell::get).shared_refresh);

        // A native Stop in the tick has owner context, but no refresh scope.
        let previous = CONTEXT.with(|context| {
            context.replace(HookContext {
                owner: owner_ptr,
                firing_tick: true,
                ..HookContext::default()
            })
        });
        let mut clear = 0;
        unsafe { shared_target(guns[0], &mut clear, 0) };
        assert_eq!(read_usize(&gun.0, GUN_TARGET), 0);
        CONTEXT.with(|context| context.set(previous));

        put_usize(&mut gun.0, GUN_TARGET, private_ptr);
        put_usize(&mut private.0, 0x10, 0);
        shared = 0xcccc;
        unsafe { shared_refresh(owner_ptr, &mut shared) };
        assert_eq!(read_usize(&gun.0, GUN_TARGET), 0);
        SHARED_REFRESH_ORIGINAL.store(old_refresh, Ordering::SeqCst);
        SET_TARGET.store(old_assign, Ordering::SeqCst);
    }

    #[test]
    fn native_command_distinguishes_auto_and_explicit_and_preserves_null_cleanup() {
        let mut owner = MockObject([0u8; GUNNER_VIEW_SIZE]);
        let mut gun = MockObject([0u8; 0x150]);
        let mut source = MockObject([0u8; 0x150]);
        let guns = [gun.0.as_mut_ptr() as usize];
        let sources = [source.0.as_mut_ptr() as usize];
        let owner_ptr = owner.0.as_mut_ptr() as usize;
        for (offset, vector) in [(GUNNER_GUNS_BEGIN, &guns), (GUNNER_SOURCES_BEGIN, &sources)] {
            put_usize(&mut owner.0, offset, vector.as_ptr() as usize);
            put_usize(&mut owner.0, offset + 8, unsafe { vector.as_ptr().add(1) }
                as usize);
        }
        put_usize(&mut owner.0, 0x18, 0x9876);
        for object in [&mut gun.0, &mut source.0] {
            put_usize(object, 0, 0x1234);
            put_usize(object, 0x40, 0x5678);
            object[GUN_ENABLED] = 1;
        }
        put_usize(&mut gun.0, GUN_TARGET, 0xaaaa);
        let old_command =
            COMMAND_ORIGINAL.swap(mock_command as *const () as usize, Ordering::SeqCst);
        let old_assign = SET_TARGET.swap(mock_assign as *const () as usize, Ordering::SeqCst);
        let old_release = RELEASE_HANDLE.swap(
            mock_release_broadcast as *const () as usize,
            Ordering::SeqCst,
        );
        COMMAND_CALLS.store(0, Ordering::SeqCst);
        ASSIGN_CALLS.store(0, Ordering::SeqCst);
        BROADCAST_RELEASES.store(0, Ordering::SeqCst);

        let mut target = 0xbbbb;
        unsafe { command(owner_ptr, &mut target, 0) };
        assert_eq!(target, 0);
        assert_eq!(read_usize(&gun.0, GUN_TARGET), 0xaaaa);
        assert_eq!(BROADCAST_RELEASES.load(Ordering::SeqCst), 1);
        assert!(unsafe { should_acquire(owner_ptr, guns[0]) });
        assert_eq!(read_usize(&owner.0, GUNNER_AUTOMATIC), 0);

        source.0[GUN_ENABLED] = 0;
        target = 0xbbbb;
        unsafe { command(owner_ptr, &mut target, 0) };
        assert_eq!(read_usize(&gun.0, GUN_TARGET), 0xaaaa);
        assert!(!unsafe { should_acquire(owner_ptr, guns[0]) });
        source.0[GUN_ENABLED] = 1;
        assert_eq!(source.0[GUN_ENABLED], 1);
        assert!(unsafe { should_acquire(owner_ptr, guns[0]) });
        gun.0[GUN_ENABLED] = 0;
        target = 0xbbbb;
        unsafe { command(owner_ptr, &mut target, 0) };
        assert_eq!(read_usize(&gun.0, GUN_TARGET), 0xaaaa);
        assert!(!unsafe { should_acquire(owner_ptr, guns[0]) });
        gun.0[GUN_ENABLED] = 1;

        target = 0xcccc;
        unsafe { command(owner_ptr, &mut target, 1) };
        assert_eq!(target, 0);
        assert_eq!(read_usize(&gun.0, GUN_TARGET), 0xcccc);
        assert_eq!(ASSIGN_FLAG.load(Ordering::SeqCst), 1);
        assert!(!unsafe { should_acquire(owner_ptr, guns[0]) });
        put_usize(&mut owner.0, GUNNER_AUTOMATIC, 1);
        assert!(!unsafe { should_acquire(owner_ptr, guns[0]) });
        owner.0[0x90] = 1;
        assert!(unsafe { should_acquire(owner_ptr, guns[0]) });
        unsafe { command(owner_ptr, &mut target, 0) };
        assert_eq!(read_usize(&gun.0, GUN_TARGET), 0);
        assert!(!unsafe { should_acquire(owner_ptr, guns[0]) });
        put_usize(&mut owner.0, GUNNER_AUTOMATIC, 1);
        assert!(unsafe { should_acquire(owner_ptr, guns[0]) });
        assert_eq!(COMMAND_CALLS.load(Ordering::SeqCst), 5);
        assert_eq!(ASSIGN_CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(BROADCAST_RELEASES.load(Ordering::SeqCst), 3);
        assert_eq!(CONTEXT.with(Cell::get).owner, 0);
        COMMAND_ORIGINAL.store(old_command, Ordering::SeqCst);
        SET_TARGET.store(old_assign, Ordering::SeqCst);
        RELEASE_HANDLE.store(old_release, Ordering::SeqCst);
    }

    #[test]
    fn automatic_order_record_does_not_follow_a_reused_owner_or_replaced_target() {
        let record = OrderMode {
            entity: 0x10,
            target: 0x20,
            automatic: true,
            explicit_order: 0,
            last_seen: Instant::now(),
        };
        assert!(record.allows(0x10, 0x20));
        assert!(!record.allows(0x11, 0x20));
        assert!(!record.allows(0x10, 0x21));
        assert!(!record.allows(0x10, 0));
        assert!(!OrderMode {
            automatic: false,
            ..record
        }
        .allows(0x10, 0x20));
    }

    unsafe extern "system" fn mock_range_accept(_: usize, target: *mut usize) -> u8 {
        let refs = unsafe { (*target + 8) as *mut i32 };
        unsafe { refs.write_unaligned(refs.read_unaligned() - 1) };
        unsafe { *target = 0 };
        1
    }

    #[test]
    fn acquisition_accepts_native_range_without_a_zero_rotation_joint_veto() {
        let gun = MockObject([0u8; 0x150]);
        let mut target = MockObject([0u8; 0x20]);
        let old_range = IN_RANGE.swap(mock_range_accept as *const () as usize, Ordering::SeqCst);
        assert!(unsafe {
            candidate_works(gun.0.as_ptr() as usize, target.0.as_mut_ptr() as usize)
        });
        assert_eq!(read_usize(&target.0, 8), 0);
        IN_RANGE.store(old_range, Ordering::SeqCst);
    }

    unsafe extern "system" fn mock_idle_tick(owner: usize, a: u8, b: u8, time: f32, dt: f32) {
        assert_eq!((a, b, time, dt), (2, 3, 37.625, 0.25));
        assert_eq!(CONTEXT.with(Cell::get).owner, owner);
        IDLE_TICKS.fetch_add(1, Ordering::SeqCst);
        // Native updates enable a freshly rebound gun without calling ready.
        let gun = unsafe { q(q(owner, GUNNER_GUNS_BEGIN), 0) };
        unsafe { ((gun + GUN_ENABLED) as *mut u8).write(1) };
    }

    unsafe extern "system" fn mock_idle_query(view: usize, out: *mut usize) -> *mut usize {
        let context = CONTEXT.with(Cell::get);
        assert_eq!(context.view, view);
        assert_ne!(context.owner, view);
        assert_eq!(unsafe { q(q(view, GUNNER_GUNS_BEGIN), 0) }, context.gun);
        IDLE_QUERIES.fetch_add(1, Ordering::SeqCst);
        unsafe { out.write(0) };
        out
    }

    #[test]
    fn idle_tick_acquires_without_readiness_and_respects_binding_state() {
        let mut owner = MockObject([0u8; GUNNER_VIEW_SIZE]);
        let mut gun = MockObject([0u8; 0x150]);
        let mut source = MockObject([0u8; 0x150]);
        let mut guns = [gun.0.as_mut_ptr() as usize];
        let mut sources = [source.0.as_mut_ptr() as usize];
        let owner_ptr = owner.0.as_mut_ptr() as usize;
        put_usize(&mut owner.0, GUNNER_GUNS_BEGIN, guns.as_mut_ptr() as usize);
        put_usize(
            &mut owner.0,
            GUNNER_GUNS_END,
            unsafe { guns.as_ptr().add(1) } as usize,
        );
        put_usize(
            &mut owner.0,
            GUNNER_SOURCES_BEGIN,
            sources.as_mut_ptr() as usize,
        );
        put_usize(
            &mut owner.0,
            GUNNER_SOURCES_END,
            unsafe { sources.as_ptr().add(1) } as usize,
        );
        for object in [&mut gun.0, &mut source.0] {
            put_usize(object, 0, 0x1234);
            put_usize(object, 0x40, 0x5678);
        }
        let old_tick = TICK_ORIGINAL.swap(mock_idle_tick as *const () as usize, Ordering::SeqCst);
        let old_query = QUERY.swap(mock_idle_query as *const () as usize, Ordering::SeqCst);
        IDLE_TICKS.store(0, Ordering::SeqCst);
        IDLE_QUERIES.store(0, Ordering::SeqCst);

        // Explicit mode, disabled source, and missing source skip acquisition.
        source.0[GUN_ENABLED] = 1;
        unsafe { tick(owner_ptr, 2, 3, 37.625, 0.25) };
        put_usize(&mut owner.0, GUNNER_AUTOMATIC, 1);
        source.0[GUN_ENABLED] = 0;
        unsafe { tick(owner_ptr, 2, 3, 37.625, 0.25) };
        source.0[GUN_ENABLED] = 1;
        sources[0] = 0;
        unsafe { tick(owner_ptr, 2, 3, 37.625, 0.25) };
        assert_eq!(IDLE_QUERIES.load(Ordering::SeqCst), 0);

        sources[0] = source.0.as_mut_ptr() as usize;
        gun.0[GUN_ENABLED] = 0;
        assert_ne!(sources[0], 0);
        assert_eq!(gun.0[GUN_ENABLED], 0);
        unsafe { tick(owner_ptr, 2, 3, 37.625, 0.25) };
        assert_eq!(IDLE_QUERIES.load(Ordering::SeqCst), 1);
        unsafe { tick(owner_ptr, 2, 3, 37.625, 0.25) };
        assert_eq!(IDLE_QUERIES.load(Ordering::SeqCst), 1);
        assert_eq!(IDLE_TICKS.load(Ordering::SeqCst), 5);
        assert_eq!(CONTEXT.with(Cell::get).owner, 0);
        TICK_ORIGINAL.store(old_tick, Ordering::SeqCst);
        QUERY.store(old_query, Ordering::SeqCst);
    }

    #[test]
    fn manual_orders_and_nonpassenger_mounts_are_untouched() {
        assert!(!should_acquire_from_facts(false, true, true, true, true));
        assert!(!should_acquire_from_facts(true, false, true, true, true));
        assert!(!should_acquire_from_facts(true, true, false, true, true));
        assert!(!should_acquire_from_facts(true, true, true, false, true));
        assert!(!should_acquire_from_facts(true, true, true, true, false));
        assert!(should_acquire_from_facts(true, true, true, true, true));
    }

    #[test]
    fn shared_broadcast_preserves_independent_target_and_consumes_its_handle() {
        let mut mounted_target = 0xaaaausize;
        let mut shared = 0xbbbbusize;
        let mut released = Vec::new();
        route_shared_target(
            true,
            shared,
            &mut shared,
            |incoming| mounted_target = *incoming,
            |incoming| {
                released.push(*incoming);
                *incoming = 0;
            },
        );
        assert_eq!(mounted_target, 0xaaaa);
        assert_eq!(shared, 0);
        assert_eq!(released, [0xbbbb]);

        // Lifecycle clears still reach the native setter, even in auto mode.
        route_shared_target(
            true,
            0xbbbb,
            &mut shared,
            |incoming| mounted_target = *incoming,
            |_| panic!("null cleanup must be forwarded"),
        );
        assert_eq!(mounted_target, 0);
        shared = 0xcccc;
        route_shared_target(
            false,
            shared,
            &mut shared,
            |incoming| mounted_target = *incoming,
            |_| panic!("manual or unbound assignment must be forwarded"),
        );
        assert_eq!(mounted_target, 0xcccc);
        assert_eq!(released, [0xbbbb]);
        shared = 0xdddd;
        route_shared_target(
            true,
            0xbbbb,
            &mut shared,
            |incoming| mounted_target = *incoming,
            |_| panic!("independent native assignment must be forwarded"),
        );
        assert_eq!(mounted_target, 0xdddd);
    }

    #[test]
    fn same_target_does_not_call_target_setter() {
        assert_eq!(target_action(0, true, false), TargetAction::Skip);
        // Native queries allocate a fresh wrapper for the same logical enemy.
        assert_eq!(target_action(1, true, true), TargetAction::Skip);
        assert_eq!(target_action(1, false, false), TargetAction::Release);
        assert_eq!(target_action(1, true, false), TargetAction::Set);
    }

    #[test]
    fn visible_current_target_is_retained_and_missing_target_reacquires() {
        let identity = [(0x10usize, 0xaaaausize), (0x20, 0xaaaa), (0x30, 0xbbbb)];
        let mut discarded = Vec::new();
        let retained = retain_visible_target(
            0x10,
            0xaaaa,
            &[0xaaaa, 0xbbbb],
            |_| Some(0x20),
            |left, right| {
                identity.iter().find(|entry| entry.0 == left).unwrap().1
                    == identity.iter().find(|entry| entry.0 == right).unwrap().1
            },
            |_| true,
            |target| discarded.push(target),
        );
        assert_eq!(retained, Some(0x20));
        assert!(discarded.is_empty());

        let missing = retain_visible_target(
            0x10,
            0xaaaa,
            &[0xbbbb],
            |_| Some(0x30),
            |left, right| {
                identity.iter().find(|entry| entry.0 == left).unwrap().1
                    == identity.iter().find(|entry| entry.0 == right).unwrap().1
            },
            |_| true,
            |target| discarded.push(target),
        );
        assert_eq!(missing, None);
        let reacquired = first_usable_candidate(
            &[0xbbbb],
            |entity| Some(if entity == 0xbbbb { 0x30 } else { 0 }),
            |_| true,
            |target| discarded.push(target),
        );
        assert_eq!(reacquired, Some(0x30));
        assert!(discarded.is_empty());
    }

    #[test]
    fn mock_retarget_keeps_same_enemy_and_updates_only_the_other_gun() {
        let owner_guns = [0x1010usize, 0x2020];
        let owner_before = owner_guns;
        let mut current = [0x01usize, 0];
        let mut selected = [0x02usize, 0x03];
        let enemy = [(0x01usize, 0xaaaausize), (0x02, 0xaaaa), (0x03, 0xbbbb)];
        let mut setters = Vec::new();
        let mut releases = Vec::new();

        for gun_index in 0..2 {
            let action = apply_selected_target(
                current[gun_index],
                &mut selected[gun_index],
                |left, right| {
                    enemy.iter().find(|entry| entry.0 == left).unwrap().1
                        == enemy.iter().find(|entry| entry.0 == right).unwrap().1
                },
                |target| {
                    setters.push((gun_index, *target));
                    current[gun_index] = *target;
                    *target = 0;
                    true
                },
                |target| {
                    releases.push(*target);
                    *target = 0;
                },
            );
            assert_eq!(
                action,
                if gun_index == 0 {
                    TargetAction::Skip
                } else {
                    TargetAction::Set
                }
            );
        }

        assert_eq!(setters, [(1, 0x03)]);
        assert_eq!(releases, [0x02]);
        assert_eq!(current, [0x01, 0x03]);
        assert_eq!(owner_guns, owner_before);
    }

    #[test]
    fn private_gunner_view_changes_only_local_single_gun_vector() {
        let mut owner = [0x5au8; GUNNER_VIEW_SIZE];
        put_usize(&mut owner, GUNNER_GUNS_BEGIN, 0x1111);
        put_usize(&mut owner, GUNNER_GUNS_END, 0x2222);
        put_usize(&mut owner, GUNNER_SOURCES_BEGIN, 0x3333);
        put_usize(&mut owner, GUNNER_SOURCES_END, 0x4444);
        let original = owner;
        let mut view = View(owner);
        let mut single = [0xabcdefusize];

        set_single_gun_view(&mut view, &mut single);

        assert_eq!(owner, original);
        assert_eq!(
            read_usize(&view.0, GUNNER_GUNS_BEGIN),
            single.as_ptr() as usize
        );
        assert_eq!(read_usize(&view.0, GUNNER_GUNS_END), unsafe {
            single.as_ptr().add(1) as usize
        });
        assert_eq!(read_usize(&view.0, GUNNER_SOURCES_BEGIN), 0x3333);
        assert_eq!(read_usize(&view.0, GUNNER_SOURCES_END), 0x4444);
    }

    #[test]
    fn throttle_expires_using_monotonic_elapsed_time() {
        let now = Instant::now();
        assert!(throttled_at(None, now));
        assert!(!throttled_at(Some(now), now + Duration::from_millis(999)));
        assert!(throttled_at(Some(now), now + Duration::from_secs(1)));
    }

    #[test]
    fn native_candidate_order_is_kept_while_unsuitable_results_are_released() {
        let mut released = Vec::new();
        let selected = first_usable_candidate(
            &[1, 2, 3],
            |candidate| Some(candidate * 10),
            |target| *target == 30,
            |target| released.push(target),
        );
        assert_eq!(selected, Some(30));
        assert_eq!(released, [10, 20]);
    }

    #[test]
    fn candidate_search_skips_native_rejections_and_frees_bad_handles() {
        let mut released = Vec::new();
        let selected = first_usable_candidate(
            &[1, 2, 3],
            |candidate| (candidate != 1).then_some(candidate * 10),
            |_| false,
            |target| released.push(target),
        );
        assert_eq!(selected, None);
        assert_eq!(released, [20, 30]);
    }

    #[test]
    fn discarded_target_handle_is_released_once_and_cleared() {
        let mut handle = 0x1234usize;
        let mut released = 0;
        discard_with(&mut handle, |_| released += 1);
        assert_eq!(handle, 0);
        assert_eq!(released, 1);
        discard_with(&mut handle, |_| released += 1);
        assert_eq!(released, 1);
    }

    fn throttled_at(previous: Option<Instant>, now: Instant) -> bool {
        !should_throttle_at(previous, now)
    }
}
