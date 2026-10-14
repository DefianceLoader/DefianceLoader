//! Scope the native deploy-preservation exception to one active self-repair order.

use core::sync::atomic::{AtomicUsize, Ordering};

pub(crate) static ORDER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub(crate) static STOP_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static SELF_REPAIR_ACTOR: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

struct ActorScope(usize);

impl ActorScope {
    fn enter(actor: usize) -> Self {
        let previous = SELF_REPAIR_ACTOR.with(|active| active.replace(actor));
        Self(previous)
    }
}

impl Drop for ActorScope {
    fn drop(&mut self) {
        SELF_REPAIR_ACTOR.with(|active| active.set(self.0));
    }
}

unsafe fn virtual_method(object: usize, offset: usize) -> usize {
    if object == 0 {
        return 0;
    }
    let table = unsafe { *(object as *const usize) };
    if table == 0 {
        return 0;
    }
    unsafe { *((table + offset) as *const usize) }
}

unsafe fn order_actor(order: usize) -> usize {
    let getter = unsafe { virtual_method(order, 0x60) };
    if getter == 0 {
        return 0;
    }
    let get: unsafe extern "C" fn(usize) -> usize = unsafe { core::mem::transmute(getter) };
    unsafe { get(order) }
}

unsafe fn deployed_in_state_1a(entity: usize) -> bool {
    let get_facets = unsafe { virtual_method(entity, 0xb0) };
    if get_facets == 0 {
        return false;
    }
    let get: unsafe extern "C" fn(usize) -> usize = unsafe { core::mem::transmute(get_facets) };
    let facets = unsafe { get(entity) };
    if facets == 0 {
        return false;
    }
    let ai = unsafe { *((facets + 0x28) as *const usize) };
    let is_state = unsafe { virtual_method(ai, 0x70) };
    if is_state == 0 {
        return false;
    }
    let check: unsafe extern "C" fn(usize, usize) -> u8 = unsafe { core::mem::transmute(is_state) };
    unsafe { check(ai, 0x1a) != 0 }
}

/// Wrap native self-repair order initialization while recording its exact actor.
///
/// The original still owns order initialization. The scope is thread-local and
/// restores an outer actor after nested/re-entrant orders.
pub(crate) unsafe extern "C" fn self_repair_order(order: usize) {
    let actor = unsafe { order_actor(order) };
    // diagnostic: the startup checks the native order reads before crew exit.
    #[cfg(feature = "diagnostics")]
    let before = unsafe { diagnostics::startup(actor) };
    let _scope = ActorScope::enter(actor);
    let original = ORDER_ORIGINAL.load(Ordering::Acquire);
    if original != 0 {
        let call: unsafe extern "C" fn(usize) = unsafe { core::mem::transmute(original) };
        unsafe { call(order) };
    }
    // diagnostic: whether the native startup produced a repairing crew member.
    #[cfg(feature = "diagnostics")]
    unsafe {
        diagnostics::outcome(order, &before)
    };
}

/// diagnostic: log what the self-repair startup sees for one actor.
#[cfg(feature = "diagnostics")]
pub(crate) mod diagnostics {
    use super::virtual_method;
    use core::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) static LOG: AtomicUsize = AtomicUsize::new(0);

    fn log(message: String) {
        let log = LOG.load(Ordering::Acquire);
        if log == 0 {
            return;
        }
        let log: unsafe extern "C" fn(u32, *const core::ffi::c_char) =
            unsafe { core::mem::transmute(log) };
        if let Ok(message) = std::ffi::CString::new(message) {
            unsafe { log(defiance_api::LOG_INFO, message.as_ptr()) };
        }
    }

    unsafe fn call0(object: usize, offset: usize) -> usize {
        let method = unsafe { virtual_method(object, offset) };
        if method == 0 {
            return 0;
        }
        let call: unsafe extern "C" fn(usize) -> usize = unsafe { core::mem::transmute(method) };
        unsafe { call(object) }
    }

    pub(crate) unsafe fn startup(actor: usize) -> String {
        let facets = unsafe { call0(actor, 0xb0) };
        let ai = if facets == 0 {
            0
        } else {
            unsafe { *((facets + 0x28) as *const usize) }
        };
        let mut states = String::new();
        let is_state = unsafe { virtual_method(ai, 0x70) };
        if is_state != 0 {
            let check: unsafe extern "C" fn(usize, usize) -> u8 =
                unsafe { core::mem::transmute(is_state) };
            for state in [0x18, 0x19, 0x1a, 0x1c] {
                if unsafe { check(ai, state) } != 0 {
                    states.push_str(&format!(" {state:#x}"));
                }
            }
        }
        let helper = unsafe { call0(ai, 0x2a0) };
        let crew_data = if facets == 0 {
            0
        } else {
            unsafe { *((facets + 0x20) as *const usize) }
        };
        let mut text = format!(
            "actor {actor:#x} ai {ai:#x} states [{states} ] helper {helper:#x} facet+0x20 {crew_data:#x}"
        );
        if helper != 0 {
            let can_unload = unsafe { call0(helper, 0x1e8) } as u8;
            let locked = unsafe { call0(helper, 0x248) } as u8;
            let count = unsafe { *((helper + 100) as *const i32) };
            let listed = unsafe {
                (*((helper + 0x100) as *const usize))
                    .wrapping_sub(*((helper + 0xf8) as *const usize))
                    / 8
            };
            let parent = unsafe { call0(actor, 0x50) };
            text.push_str(&format!(
                " vfunc_61 {can_unload} vfunc_73 {locked} +100 {count} +0xf8 len {listed} parent {parent:#x}"
            ));
        }
        log(format!("deployed repair diag: start {text}"));
        text
    }

    pub(crate) fn skipped(entity: usize) {
        log(format!(
            "deployed repair diag: kept {entity:#x} deployed during crew exit"
        ));
    }

    pub(crate) fn pushed(state: usize) {
        log(format!(
            "deployed repair diag: pushed self-repair over {} {state:#x}",
            unsafe { type_name(state) }
        ));
    }

    pub(crate) static INTAKE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

    unsafe fn read<T: Copy>(address: usize) -> T {
        unsafe { *(address as *const T) }
    }

    /// The MSVC RTTI class name of a polymorphic object, read through its COL.
    unsafe fn type_name(object: usize) -> String {
        if object == 0 {
            return "null".into();
        }
        let table: usize = unsafe { read(object) };
        let col: usize = unsafe { read(table - 8) };
        if col == 0 || unsafe { read::<u32>(col) } != 1 {
            return format!("{object:#x}");
        }
        let base = col - unsafe { read::<u32>(col + 0x14) } as usize;
        let name = base + unsafe { read::<u32>(col + 0xc) } as usize + 0x10;
        unsafe { std::ffi::CStr::from_ptr(name as *const core::ffi::c_char) }
            .to_string_lossy()
            .into_owned()
    }

    /// The object behind a Storage::PtrJunction pointer.
    unsafe fn junction_object(junction: usize) -> usize {
        if junction == 0 {
            0
        } else {
            unsafe { read(junction + 0x10) }
        }
    }

    unsafe fn intake_state(ai: usize) -> String {
        let size: usize = unsafe { read(ai + 0x1f8) };
        let map: usize = unsafe { read(ai + 0x1e0) };
        let map_size: usize = unsafe { read(ai + 0x1e8) };
        let offset: usize = unsafe { read(ai + 0x1f0) };
        let mut queued = String::new();
        for index in 0..size.min(4) {
            let at = offset + index;
            let block: usize = unsafe { read(map + ((at / 2) & (map_size.wrapping_sub(1))) * 8) };
            let junction: usize = unsafe { read(block + (at % 2) * 8) };
            queued.push_str(&format!(" {}", unsafe {
                type_name(junction_object(junction))
            }));
        }
        let current = unsafe { type_name(junction_object(read(ai + 0x138))) };
        // The controller FUN_1800cc380 posts the wrapped order to (vtable +0x60).
        let controller = unsafe { junction_object(read(ai + 0xe8)) };
        // AiBaseStateMachine::vfunc_8 hands a new state to the top of the stack at +0x38..+0x40.
        let mut fields = String::new();
        if controller != 0 {
            let (begin, end): (usize, usize) =
                unsafe { (read(controller + 0x38), read(controller + 0x40)) };
            let mut entry = begin;
            while entry < end && entry - begin < 0x80 {
                let state = unsafe { junction_object(read(entry)) };
                fields.push_str(&format!(" {} {state:#x}", unsafe { type_name(state) }));
                // Nested machines keep their own stack at the same offsets.
                if unsafe { type_name(state) }.contains("StateMachine") {
                    let (inner, inner_end): (usize, usize) =
                        unsafe { (read(state + 0x38), read(state + 0x40)) };
                    let mut slot = inner;
                    fields.push_str(" {");
                    while slot < inner_end && slot - inner < 0x80 {
                        fields.push_str(&format!(" {}", unsafe {
                            type_name(junction_object(read(slot)))
                        }));
                        slot += 8;
                    }
                    fields.push_str(" }");
                }
                entry += 8;
            }
        }
        format!(
            "controller {} {controller:#x} [{fields} ] +0x18 {} +0x1d2 {} +0x130 {} current {current} queue {size} [{queued} ]",
            unsafe { type_name(controller) },
            unsafe { read::<u8>(ai + 0x18) },
            unsafe { read::<u8>(ai + 0x1d2) },
            unsafe { read::<u8>(ai + 0x130) },
        )
    }

    /// diagnostic: AiFacet::vfunc_12 (order intake) for self-repair orders.
    pub(crate) unsafe extern "C" fn intake(ai: usize, order: *const usize, delay: f32) {
        let object = if order.is_null() {
            0
        } else {
            unsafe { junction_object(*order) }
        };
        let name = unsafe { type_name(object) };
        let watched = name.contains("SelfRepair");
        if watched {
            let owner: usize = unsafe { read(ai + 0x10) };
            let entity = if owner == 0 {
                0
            } else {
                unsafe { read::<usize>(owner + 0x10) }
            };
            let flagged = unsafe { virtual_method(entity, 0x98) };
            let flag = if flagged == 0 {
                0
            } else {
                let check: unsafe extern "C" fn(usize, u32) -> u8 =
                    unsafe { core::mem::transmute(flagged) };
                unsafe { check(entity, 0x4000) }
            };
            log(format!(
                "deployed repair diag: intake {name} ai {} {ai:#x} delay {delay} flag4000 {flag} {}",
                unsafe { type_name(ai) },
                unsafe { intake_state(ai) }
            ));
        }
        let original = INTAKE_ORIGINAL.load(Ordering::Acquire);
        if original != 0 {
            let call: unsafe extern "C" fn(usize, *const usize, f32) =
                unsafe { core::mem::transmute(original) };
            unsafe { call(ai, order, delay) };
        }
        if watched {
            log(format!("deployed repair diag: intake after {}", unsafe {
                intake_state(ai)
            }));
        }
    }

    pub(crate) unsafe fn outcome(order: usize, _before: &str) {
        let fields = order as *const usize;
        let crew = unsafe { *fields.add(4) };
        let crew_object = if crew == 0 {
            0
        } else {
            unsafe { *((crew + 0x10) as *const usize) }
        };
        let members = unsafe { (*fields.add(6)).wrapping_sub(*fields.add(5)) / 8 };
        log(format!(
            "deployed repair diag: after order {order:#x} crew {crew_object:#x} members {members}"
        ));
    }
}

pub(crate) static RECHARGE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SIT_ON_LAND_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PUSH_TASK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TASK_ID: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SELF_REPAIR_ID: AtomicUsize = AtomicUsize::new(0);

/// The state id of the task behind a `Storage::PtrJunction` handle.
unsafe fn task_id(task: *const usize) -> Option<usize> {
    if task.is_null() {
        return None;
    }
    let junction = unsafe { *task };
    if junction == 0 {
        return None;
    }
    let object = unsafe { *((junction + 0x10) as *const usize) };
    let getter = unsafe { virtual_method(object, TASK_ID.load(Ordering::Acquire)) };
    if getter == 0 {
        return None;
    }
    let get: unsafe extern "C" fn(usize) -> u32 = unsafe { core::mem::transmute(getter) };
    Some(unsafe { get(object) } as usize)
}

/// Stack a self-repair task over a deployed unit's recharge state.
///
/// The recharge state drops tasks it does not forward, and the only path
/// that accepts them leaves deployment first. The push helper is the one an
/// idle unit's handler uses, so the repair state runs above the recharge
/// state and returns to it when done. Both consume the task handle.
pub(crate) unsafe extern "C" fn recharge_order(state: usize, task: *mut usize) -> usize {
    unsafe { push_self_repair(state, task, &RECHARGE_ORIGINAL) }
}

/// Stack a self-repair task over a landed helicopter's state.
///
/// The landed state drops the task like the recharge state does. While its
/// nested machine runs (landing or take-off) every task stays with the stock
/// handler, which hands it to that machine.
pub(crate) unsafe extern "C" fn sit_on_land_order(state: usize, task: *mut usize) -> usize {
    let nested = unsafe { *((state + crate::logic_sites::SIT_ON_LAND_NESTED) as *const usize) };
    if nested != 0 {
        return unsafe { call_original(&SIT_ON_LAND_ORIGINAL, state, task) };
    }
    unsafe { push_self_repair(state, task, &SIT_ON_LAND_ORIGINAL) }
}

/// Push a self-repair task through the push helper, or pass any other task
/// to the hooked handler.
unsafe fn push_self_repair(state: usize, task: *mut usize, original: &AtomicUsize) -> usize {
    let push = PUSH_TASK.load(Ordering::Acquire);
    if push != 0 && unsafe { task_id(task) } == Some(SELF_REPAIR_ID.load(Ordering::Acquire)) {
        // diagnostic: the self-repair task was stacked over the deployed or landed state.
        #[cfg(feature = "diagnostics")]
        diagnostics::pushed(state);
        let call: unsafe extern "C" fn(usize, *mut usize) -> usize =
            unsafe { core::mem::transmute(push) };
        return unsafe { call(state, task) };
    }
    unsafe { call_original(original, state, task) }
}

unsafe fn call_original(original: &AtomicUsize, state: usize, task: *mut usize) -> usize {
    let call: unsafe extern "C" fn(usize, *mut usize) -> usize =
        unsafe { core::mem::transmute(original.load(Ordering::Acquire)) };
    unsafe { call(state, task) }
}

/// Keep a deployed actor in place only for its own synchronous self-repair exit.
pub(crate) unsafe extern "C" fn stop_deployment(entity: usize) {
    let guarded_actor = SELF_REPAIR_ACTOR.with(core::cell::Cell::get);
    if entity != 0 && entity == guarded_actor && unsafe { deployed_in_state_1a(entity) } {
        // diagnostic: the crew-exit stop order was skipped for the repairing actor.
        #[cfg(feature = "diagnostics")]
        diagnostics::skipped(entity);
        return;
    }
    let original = STOP_ORIGINAL.load(Ordering::Acquire);
    if original != 0 {
        let call: unsafe extern "C" fn(usize) = unsafe { core::mem::transmute(original) };
        unsafe { call(entity) };
    }
}

#[cfg(feature = "parity-test")]
#[no_mangle]
pub extern "C" fn deployed_repair_test_bind(original_order: usize, original_stop: usize) {
    ORDER_ORIGINAL.store(original_order, Ordering::Release);
    STOP_ORIGINAL.store(original_stop, Ordering::Release);
}

#[cfg(feature = "parity-test")]
#[no_mangle]
pub unsafe extern "C" fn deployed_repair_test_order(order: usize) {
    unsafe { self_repair_order(order) };
}

#[cfg(feature = "parity-test")]
#[no_mangle]
pub unsafe extern "C" fn deployed_repair_test_stop(entity: usize) {
    unsafe { stop_deployment(entity) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    static STOP_CALLS: AtomicUsize = AtomicUsize::new(0);
    static ORDER_CALLS: AtomicUsize = AtomicUsize::new(0);
    static MODE: AtomicU8 = AtomicU8::new(0);
    static NESTED_ORDER: AtomicUsize = AtomicUsize::new(0);

    #[repr(C)]
    struct Unit {
        vtable: usize,
        facets: usize,
    }

    #[repr(C)]
    struct Ai {
        vtable: usize,
        state: u8,
    }

    #[repr(C)]
    struct Order {
        vtable: usize,
        actor: usize,
        other: usize,
    }

    struct UnitFixture {
        unit_methods: [usize; 23],
        ai_methods: [usize; 15],
        facets: [usize; 6],
        ai: Ai,
        unit: Unit,
    }

    impl UnitFixture {
        fn new(state: u8) -> Box<Self> {
            let mut fixture = Box::new(Self {
                unit_methods: [0; 23],
                ai_methods: [0; 15],
                facets: [0; 6],
                ai: Ai { vtable: 0, state },
                unit: Unit {
                    vtable: 0,
                    facets: 0,
                },
            });
            fixture.unit_methods[0xb0 / 8] = get_facets as *const () as usize;
            fixture.ai_methods[0x70 / 8] = is_state as *const () as usize;
            fixture.facets[0x28 / 8] = &fixture.ai as *const Ai as usize;
            fixture.ai.vtable = fixture.ai_methods.as_ptr() as usize;
            fixture.unit.vtable = fixture.unit_methods.as_ptr() as usize;
            fixture.unit.facets = fixture.facets.as_ptr() as usize;
            fixture
        }

        fn entity(&self) -> usize {
            &self.unit as *const Unit as usize
        }
    }

    unsafe extern "C" fn get_facets(unit: usize) -> usize {
        unsafe { (*(unit as *const Unit)).facets }
    }

    unsafe extern "C" fn is_state(ai: usize, state: usize) -> u8 {
        u8::from(unsafe { (*(ai as *const Ai)).state as usize == state })
    }

    unsafe extern "C" fn order_actor(order: usize) -> usize {
        unsafe { (*(order as *const Order)).actor }
    }

    unsafe extern "C" fn mock_stop(_: usize) {
        STOP_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    unsafe extern "C" fn mock_order(order: usize) {
        ORDER_CALLS.fetch_add(1, Ordering::SeqCst);
        let order = unsafe { &*(order as *const Order) };
        match MODE.load(Ordering::SeqCst) {
            1 => {
                unsafe { stop_deployment(order.actor) };
                unsafe { stop_deployment(order.other) };
            }
            2 => {
                let nested = NESTED_ORDER.swap(0, Ordering::SeqCst);
                if nested != 0 {
                    unsafe { self_repair_order(nested) };
                }
                unsafe { stop_deployment(order.actor) };
            }
            3 => {
                let actor = order.actor;
                std::thread::spawn(move || unsafe { stop_deployment(actor) })
                    .join()
                    .unwrap();
                unsafe { stop_deployment(order.actor) };
            }
            _ => {}
        }
    }

    fn fixture_order(actor: usize, other: usize) -> Box<Order> {
        let methods = Box::leak(Box::new([0usize; 13]));
        methods[0x60 / 8] = order_actor as *const () as usize;
        Box::new(Order {
            vtable: methods.as_ptr() as usize,
            actor,
            other,
        })
    }

    fn bind(mode: u8) {
        STOP_CALLS.store(0, Ordering::SeqCst);
        ORDER_CALLS.store(0, Ordering::SeqCst);
        MODE.store(mode, Ordering::SeqCst);
        NESTED_ORDER.store(0, Ordering::SeqCst);
        ORDER_ORIGINAL.store(mock_order as *const () as usize, Ordering::SeqCst);
        STOP_ORIGINAL.store(mock_stop as *const () as usize, Ordering::SeqCst);
    }

    #[test]
    fn only_the_same_fully_deployed_actor_skips_stop() {
        let _lock = TEST_LOCK.lock().unwrap();
        let actor = UnitFixture::new(0x1a);
        let other = UnitFixture::new(0x1a);
        let order = fixture_order(actor.entity(), other.entity());
        bind(1);
        unsafe { self_repair_order(&*order as *const Order as usize) };
        assert_eq!(ORDER_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(STOP_CALLS.load(Ordering::SeqCst), 1);
        unsafe { stop_deployment(actor.entity()) };
        assert_eq!(STOP_CALLS.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn nondeployed_same_actor_and_null_actor_forward() {
        let _lock = TEST_LOCK.lock().unwrap();
        let actor = UnitFixture::new(0x18);
        let order = fixture_order(actor.entity(), 0);
        bind(1);
        unsafe { self_repair_order(&*order as *const Order as usize) };
        assert_eq!(STOP_CALLS.load(Ordering::SeqCst), 2);
        unsafe { stop_deployment(0) };
        assert_eq!(STOP_CALLS.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn nested_orders_restore_the_outer_actor() {
        let _lock = TEST_LOCK.lock().unwrap();
        let outer = UnitFixture::new(0x1a);
        let inner = UnitFixture::new(0x1a);
        let outer_order = fixture_order(outer.entity(), 0);
        let inner_order = fixture_order(inner.entity(), 0);
        bind(2);
        NESTED_ORDER.store(&*inner_order as *const Order as usize, Ordering::SeqCst);
        unsafe { self_repair_order(&*outer_order as *const Order as usize) };
        assert_eq!(ORDER_CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(STOP_CALLS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_scope_does_not_cross_threads() {
        let _lock = TEST_LOCK.lock().unwrap();
        let actor = UnitFixture::new(0x1a);
        let order = fixture_order(actor.entity(), 0);
        bind(3);
        unsafe { self_repair_order(&*order as *const Order as usize) };
        assert_eq!(STOP_CALLS.load(Ordering::SeqCst), 1);
    }
}
