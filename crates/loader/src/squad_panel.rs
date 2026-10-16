//! Loader-owned dispatch for Core's squad-panel refresh hook.
//!
//! Core owns the single game hook. Its loader feed dispatches to observers
//! registered through Core's public service table. Each observer is tied to
//! the plugin that registered it and is retired before that plugin is stopped.

use core::ffi::c_void;
use defiance_api::{SquadPanelFeedV1, SquadPanelRefreshCallback, SquadPanelRefreshV1};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Default)]
struct Gate {
    committed: bool,
    retired: bool,
    in_flight: usize,
}

struct Subscription {
    token: u64,
    owner: usize,
    id: String,
    slot: usize,
    callback: SquadPanelRefreshCallback,
    context: usize,
    gate: Mutex<Gate>,
}

#[derive(Default)]
struct Registry {
    subscriptions: Vec<Arc<Subscription>>,
    plan_order: Vec<String>,
    feed_owner: Option<usize>,
    pending_feed: Option<usize>,
    feed_retiring: Option<usize>,
}

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
thread_local! {
    static INIT: RefCell<Option<(usize, String, usize)>> = const { RefCell::new(None) };
    static ACTIVE_TOKENS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static DISPATCHING: Cell<bool> = const { Cell::new(false) };
}

fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

/// Begin the loader's subscription transaction for one plugin init.
pub(crate) fn begin(owner: usize, id: &str) {
    INIT.with(|init| *init.borrow_mut() = Some((owner, id.to_ascii_lowercase(), 0)));
}

/// Commit or discard subscriptions and Core's pending feed publication.
pub(crate) fn finish(owner: usize, success: bool) {
    let id = INIT.with(|init| init.borrow().as_ref().map(|(_, id, _)| id.clone()));
    INIT.with(|init| *init.borrow_mut() = None);
    let mut state = registry().lock().unwrap_or_else(|p| p.into_inner());
    if success {
        // Lifecycle recording or patch-plan commit activates this owner.
    } else {
        let is_provider = id.as_deref() == Some("defiance.core")
            && (state.feed_owner == Some(owner)
                || state.pending_feed == Some(owner)
                || state.feed_retiring == Some(owner));
        if is_provider {
            retire_provider(&mut state, owner);
        } else {
            if state.pending_feed == Some(owner) {
                state.pending_feed = None;
            }
            retire_matching(&mut state, |s| s.owner == owner);
        }
    }
}

/// Disable callbacks owned by a plugin. `false` means one is still running;
/// callers must leave the DLL mapped and skip its stop function until retry.
pub(crate) fn retire_owner(owner: usize) -> bool {
    let mut state = registry().lock().unwrap_or_else(|p| p.into_inner());
    let provider = state.feed_owner == Some(owner)
        || state.pending_feed == Some(owner)
        || state.feed_retiring == Some(owner);
    if provider {
        retire_provider(&mut state, owner)
    } else {
        retire_matching(&mut state, |s| s.owner == owner)
    }
}

fn retire_provider(state: &mut Registry, owner: usize) -> bool {
    if state.feed_owner == Some(owner) {
        state.feed_owner = None;
        state.feed_retiring = Some(owner);
    }
    if state.pending_feed == Some(owner) {
        state.pending_feed = None;
        state.feed_retiring = Some(owner);
    }
    let quiescent = retire_matching(state, |_| true);
    if quiescent && state.feed_retiring == Some(owner) {
        state.feed_retiring = None;
    }
    quiescent
}

/// Make an initialized owner's registrations callable after lifecycle recording
/// or managed patch-plan publication.
pub(crate) fn activate_owner(owner: usize) {
    let mut state = registry().lock().unwrap_or_else(|p| p.into_inner());
    for subscription in state.subscriptions.iter().filter(|s| s.owner == owner) {
        subscription
            .gate
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .committed = true;
    }
    if state.pending_feed == Some(owner) {
        state.feed_owner = Some(owner);
        state.pending_feed = None;
    }
}

fn retire_matching(state: &mut Registry, matches: impl Fn(&Subscription) -> bool) -> bool {
    let mut quiescent = true;
    let mut keep = Vec::new();
    for subscription in state.subscriptions.iter().filter(|s| matches(s)) {
        let mut gate = subscription.gate.lock().unwrap_or_else(|p| p.into_inner());
        gate.retired = true;
        gate.committed = false;
        if gate.in_flight != 0 {
            quiescent = false;
            keep.push(subscription.token);
        }
    }
    state
        .subscriptions
        .retain(|s| !matches(s) || keep.contains(&s.token));
    quiescent
}

fn subscribe(callback: Option<SquadPanelRefreshCallback>, context: *mut c_void) -> u64 {
    let Some(callback) = callback else { return 0 };
    let Some((owner, id)) = INIT.with(|init| {
        init.borrow()
            .as_ref()
            .map(|current| (current.0, current.1.clone()))
    }) else {
        return 0;
    };
    if id == "defiance.core" || !crate::services::current_has_use("defiance.core") {
        return 0;
    }
    let slot = INIT.with(|init| {
        let mut init = init.borrow_mut();
        let current = init.as_mut().expect("init context was checked above");
        let slot = current.2;
        current.2 += 1;
        slot
    });
    let mut state = registry().lock().unwrap_or_else(|p| p.into_inner());
    if state.feed_owner.is_none() && state.pending_feed.is_none() {
        return 0;
    }
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    state.subscriptions.push(Arc::new(Subscription {
        token,
        owner,
        id,
        slot,
        callback,
        context: context as usize,
        gate: Mutex::new(Gate::default()),
    }));
    token
}

fn unsubscribe(token: u64) -> i32 {
    if token == 0 {
        return 1;
    }
    let mut state = registry().lock().unwrap_or_else(|p| p.into_inner());
    let Some(index) = state.subscriptions.iter().position(|s| s.token == token) else {
        return 1;
    };
    let subscription = state.subscriptions[index].clone();
    let mut gate = subscription.gate.lock().unwrap_or_else(|p| p.into_inner());
    gate.retired = true;
    gate.committed = false;
    let active_here = ACTIVE_TOKENS.with(|tokens| tokens.borrow().contains(&token));
    if gate.in_flight != 0 || active_here {
        2
    } else {
        state.subscriptions.remove(index);
        0
    }
}

fn dispatch(panel: *mut c_void) {
    if panel.is_null() {
        return;
    }
    let recursive = DISPATCHING.with(|dispatching| dispatching.replace(true));
    if recursive {
        DISPATCHING.with(|dispatching| dispatching.set(true));
        return;
    }
    struct DispatchGuard;
    impl Drop for DispatchGuard {
        fn drop(&mut self) {
            DISPATCHING.with(|dispatching| dispatching.set(false));
        }
    }
    let _guard = DispatchGuard;
    let (mut subscriptions, plan_order) = {
        let state = registry().lock().unwrap_or_else(|p| p.into_inner());
        if state.feed_owner.is_none() {
            return;
        }
        (
            state.subscriptions.iter().cloned().collect::<Vec<_>>(),
            state.plan_order.clone(),
        )
    };
    subscriptions.sort_by_key(|s| {
        (
            plan_order
                .iter()
                .position(|id| id == &s.id)
                .unwrap_or(usize::MAX),
            s.id.clone(),
            s.slot,
        )
    });
    for subscription in subscriptions {
        {
            let mut gate = subscription.gate.lock().unwrap_or_else(|p| p.into_inner());
            if gate.retired || !gate.committed {
                continue;
            }
            gate.in_flight += 1;
        }
        ACTIVE_TOKENS.with(|tokens| tokens.borrow_mut().push(subscription.token));
        unsafe { (subscription.callback)(panel, subscription.context as *mut c_void) };
        ACTIVE_TOKENS.with(|tokens| {
            let mut tokens = tokens.borrow_mut();
            if let Some(index) = tokens
                .iter()
                .rposition(|&token| token == subscription.token)
            {
                tokens.remove(index);
            }
        });
        let mut gate = subscription.gate.lock().unwrap_or_else(|p| p.into_inner());
        gate.in_flight -= 1;
        let _ = gate.in_flight;
    }
}

/// Snapshot the plan's stable rank for every candidate, including disabled
/// plugins so enabling one later does not move existing observers.
pub(crate) fn set_plan_order(nodes: &[crate::plan::Planned], order: &[usize]) {
    let mut active: Vec<String> = order
        .iter()
        .filter_map(|&index| nodes.get(index).map(|node| node.id.to_ascii_lowercase()))
        .collect();
    let already_active: std::collections::HashSet<_> = active.iter().cloned().collect();
    let all: Vec<String> = crate::plan::order_all(nodes)
        .into_iter()
        .map(|index| nodes[index].id.to_ascii_lowercase())
        .collect();
    for (position, id) in all
        .iter()
        .enumerate()
        .filter(|(_, id)| !already_active.contains(*id))
    {
        let next = all[position + 1..]
            .iter()
            .find(|candidate| active.contains(candidate))
            .cloned();
        if let Some(next) = next {
            let index = active
                .iter()
                .position(|candidate| candidate == &next)
                .unwrap();
            active.insert(index, id.clone());
        } else {
            let previous = all[..position]
                .iter()
                .rev()
                .find(|candidate| active.contains(candidate))
                .cloned();
            if let Some(previous) = previous {
                let index = active
                    .iter()
                    .position(|candidate| candidate == &previous)
                    .unwrap();
                active.insert(index + 1, id.clone());
            } else {
                active.push(id.clone());
            }
        }
    }
    let mut state = registry().lock().unwrap_or_else(|p| p.into_inner());
    let provided: std::collections::HashSet<_> = active.iter().cloned().collect();
    let absent: Vec<_> = state
        .plan_order
        .iter()
        .filter(|id| !provided.contains(*id))
        .cloned()
        .collect();
    let mut merged = active;
    for id in absent {
        let old_position = state.plan_order.iter().position(|old| old == &id).unwrap();
        let previous = state.plan_order[..old_position]
            .iter()
            .rev()
            .find_map(|old| merged.iter().position(|candidate| candidate == old));
        let next = state.plan_order[old_position + 1..]
            .iter()
            .find_map(|old| merged.iter().position(|candidate| candidate == old));
        let insert_at = match (previous, next) {
            (Some(before), Some(after)) if before < after => after,
            (Some(before), _) => before + 1,
            (_, Some(after)) => after,
            _ => merged.len(),
        };
        merged.insert(insert_at, id);
    }
    state.plan_order = merged;
}

unsafe extern "C" fn service_subscribe(
    callback: Option<SquadPanelRefreshCallback>,
    context: *mut c_void,
) -> u64 {
    subscribe(callback, context)
}

unsafe extern "C" fn service_unsubscribe(token: u64) -> i32 {
    unsubscribe(token)
}

unsafe extern "C" fn feed_publish() -> i32 {
    let Some((owner, id, _)) = INIT.with(|init| init.borrow().clone()) else {
        return 1;
    };
    if id != "defiance.core" {
        return 1;
    }
    let mut state = registry().lock().unwrap_or_else(|p| p.into_inner());
    if state.feed_owner.is_some_and(|current| current != owner)
        || state.pending_feed.is_some_and(|current| current != owner)
        || state.feed_retiring.is_some()
    {
        return 1;
    }
    state.pending_feed = Some(owner);
    0
}

unsafe extern "C" fn feed_dispatch(panel: *mut c_void) {
    dispatch(panel);
}

/// Loader-only table Core forwards to its consumers.
pub static API: SquadPanelRefreshV1 = SquadPanelRefreshV1 {
    subscribe: service_subscribe,
    unsubscribe: service_unsubscribe,
};

/// Loader-only feed table for Core's single physical refresh hook.
pub static FEED: SquadPanelFeedV1 = SquadPanelFeedV1 {
    publish: feed_publish,
    dispatch: feed_dispatch,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, OnceLock};

    static SERIAL: Mutex<()> = Mutex::new(());
    static SEEN: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    static BLOCK: OnceLock<Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>>> = OnceLock::new();
    static SELF_TOKEN: AtomicU64 = AtomicU64::new(0);
    static STOPPED: AtomicU64 = AtomicU64::new(0);

    unsafe extern "C" fn record(_: *mut c_void, context: *mut c_void) {
        SEEN.lock().unwrap().push(context as usize);
    }

    unsafe extern "C" fn block(_: *mut c_void, _: *mut c_void) {
        let pair = BLOCK
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap()
            .clone()
            .unwrap();
        pair.0.wait();
        pair.1.wait();
    }

    unsafe extern "C" fn nested(_: *mut c_void, context: *mut c_void) {
        SEEN.lock().unwrap().push(context as usize);
        dispatch(2usize as *mut c_void);
    }

    unsafe extern "C" fn self_unsubscribe(_: *mut c_void, context: *mut c_void) {
        assert_eq!(unsubscribe(SELF_TOKEN.load(Ordering::Acquire)), 2);
        SEEN.lock().unwrap().push(context as usize);
    }

    unsafe extern "C" fn stop_counter() {
        STOPPED.fetch_add(1, Ordering::AcqRel);
    }

    fn setup_core(owner: usize, activate: bool) {
        crate::services::begin(owner, "defiance.core", vec![]);
        assert_eq!(
            unsafe {
                (crate::services::API.register)(
                    c"squad-panel-refresh".as_ptr(),
                    1,
                    &API as *const _ as *const c_void,
                    core::mem::size_of::<SquadPanelRefreshV1>(),
                )
            },
            0
        );
        assert_eq!(unsafe { feed_publish() }, 0);
        crate::services::finish(owner, true);
        if activate {
            activate_owner(owner);
        }
    }

    fn subscribe_consumer(
        owner: usize,
        id: &str,
        callback: Option<SquadPanelRefreshCallback>,
        context: *mut c_void,
    ) -> u64 {
        crate::services::begin(owner, id, vec!["defiance.core".into()]);
        assert!(!unsafe {
            (crate::services::API.query)(
                c"defiance.core".as_ptr(),
                c"squad-panel-refresh".as_ptr(),
                1,
                core::mem::size_of::<SquadPanelRefreshV1>(),
            )
        }
        .is_null());
        unsafe { service_subscribe(callback, context) }
    }

    fn clear(owner: usize, core_owner: usize) {
        crate::services::remove(owner);
        crate::services::remove(core_owner);
        SEEN.lock().unwrap().clear();
    }

    #[test]
    fn failed_init_discards_subscriptions_and_feed_dispatch_is_ordered_by_plan() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let core_owner = 0x5101;
        let a_owner = 0x5102;
        let b_owner = 0x5103;
        setup_core(core_owner, true);
        let failed = subscribe_consumer(a_owner, "test.a", Some(record), 1usize as *mut c_void);
        crate::services::finish(a_owner, false);
        let b = subscribe_consumer(b_owner, "test.b", Some(record), 2usize as *mut c_void);
        crate::services::finish(b_owner, true);
        activate_owner(b_owner);
        let nodes = [
            crate::plan::Planned {
                id: "test.b".into(),
                dll: String::new(),
                path: Default::default(),
                manifest: None,
                legacy: true,
                decision: crate::plan::Decision::Initialize,
                depends: vec![],
            },
            crate::plan::Planned {
                id: "test.a".into(),
                dll: String::new(),
                path: Default::default(),
                manifest: None,
                legacy: true,
                decision: crate::plan::Decision::Initialize,
                depends: vec![],
            },
        ];
        set_plan_order(&nodes, &[0, 1]);
        dispatch(1usize as *mut c_void);
        assert_eq!(*SEEN.lock().unwrap(), [2]);
        assert_eq!(unsubscribe(failed), 1);
        assert_eq!(unsubscribe(b), 0);
        clear(b_owner, core_owner);
    }

    #[test]
    fn retirement_blocks_snapshot_races_and_unsubscribe_can_be_retried() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let core_owner = 0x5201;
        let consumer = 0x5202;
        let queued_consumer = 0x5203;
        setup_core(core_owner, true);
        let token = subscribe_consumer(
            consumer,
            "test.blocking",
            Some(block),
            core::ptr::null_mut(),
        );
        crate::services::finish(consumer, true);
        activate_owner(consumer);
        let queued = subscribe_consumer(
            queued_consumer,
            "test.queued",
            Some(record),
            99usize as *mut c_void,
        );
        crate::services::finish(queued_consumer, true);
        activate_owner(queued_consumer);
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        *BLOCK.get_or_init(|| Mutex::new(None)).lock().unwrap() =
            Some((entered.clone(), release.clone()));
        let thread = std::thread::spawn(|| dispatch(1usize as *mut c_void));
        entered.wait();
        assert_eq!(unsubscribe(token), 2);
        assert!(!retire_owner(core_owner));
        dispatch(1usize as *mut c_void);
        release.wait();
        thread.join().unwrap();
        assert_eq!(unsubscribe(token), 0);
        assert!(
            SEEN.lock().unwrap().is_empty(),
            "queued callback was retired before entry"
        );
        assert!(retire_owner(core_owner));
        assert_eq!(unsubscribe(queued), 1);
        *BLOCK.get().unwrap().lock().unwrap() = None;
        clear(consumer, core_owner);
        crate::services::remove(queued_consumer);
    }

    #[test]
    fn subscriptions_need_core_acquisition_and_feed_publish_is_core_init_only() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let core_owner = 0x5301;
        setup_core(core_owner, true);
        assert_eq!(unsafe { feed_publish() }, 1);
        crate::services::begin(0x5302, "test.no-dependency", vec![]);
        assert_eq!(
            unsafe { service_subscribe(Some(record), core::ptr::null_mut()) },
            0
        );
        crate::services::finish(0x5302, false);
        clear(0, core_owner);
    }

    #[test]
    fn reload_keeps_two_consumers_in_plan_order_and_slot_order() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let core_owner = 0x5401;
        let a_owner = 0x5402;
        let b_owner = 0x5403;
        let a_reload_owner = 0x5404;
        setup_core(core_owner, true);
        let a0 = subscribe_consumer(a_owner, "test.a", Some(record), 10usize as *mut c_void);
        let a1 = unsafe { service_subscribe(Some(nested), 11usize as *mut c_void) };
        crate::services::finish(a_owner, true);
        activate_owner(a_owner);
        let b = subscribe_consumer(b_owner, "test.b", Some(record), 20usize as *mut c_void);
        crate::services::finish(b_owner, true);
        activate_owner(b_owner);
        let nodes = [
            crate::plan::Planned {
                id: "test.a".into(),
                dll: String::new(),
                path: Default::default(),
                manifest: None,
                legacy: true,
                decision: crate::plan::Decision::Initialize,
                depends: vec![],
            },
            crate::plan::Planned {
                id: "test.b".into(),
                dll: String::new(),
                path: Default::default(),
                manifest: None,
                legacy: true,
                decision: crate::plan::Decision::Initialize,
                depends: vec![],
            },
        ];
        set_plan_order(&nodes, &[0, 1]);
        dispatch(1usize as *mut c_void);
        assert_eq!(*SEEN.lock().unwrap(), [10, 11, 20]);
        SEEN.lock().unwrap().clear();
        assert_eq!(unsubscribe(a0), 0);
        assert_eq!(unsubscribe(a1), 0);
        crate::services::remove(a_owner);
        let a0 = subscribe_consumer(
            a_reload_owner,
            "test.a",
            Some(record),
            12usize as *mut c_void,
        );
        let a1 = unsafe { service_subscribe(Some(record), 13usize as *mut c_void) };
        crate::services::finish(a_reload_owner, true);
        activate_owner(a_reload_owner);
        set_plan_order(&nodes, &[0, 1]);
        dispatch(1usize as *mut c_void);
        assert_eq!(*SEEN.lock().unwrap(), [12, 13, 20]);
        assert_eq!(unsubscribe(a0), 0);
        assert_eq!(unsubscribe(a1), 0);
        assert_eq!(unsubscribe(b), 0);
        clear(a_reload_owner, core_owner);
        crate::services::remove(b_owner);
    }

    #[test]
    fn init_and_patch_candidates_are_invisible_until_commit_and_nested_dispatch_is_suppressed() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let core_owner = 0x5501;
        let consumer = 0x5502;
        setup_core(core_owner, false);
        let nested_token = subscribe_consumer(
            consumer,
            "test.pending",
            Some(nested),
            30usize as *mut c_void,
        );
        crate::services::finish(consumer, true);
        dispatch(1usize as *mut c_void);
        assert!(SEEN.lock().unwrap().is_empty());
        activate_owner(core_owner);
        dispatch(1usize as *mut c_void);
        assert!(SEEN.lock().unwrap().is_empty());
        activate_owner(consumer);
        dispatch(1usize as *mut c_void);
        assert_eq!(*SEEN.lock().unwrap(), [30]);
        assert_eq!(unsubscribe(nested_token), 0);

        let self_owner = 0x5503;
        let self_token = subscribe_consumer(
            self_owner,
            "test.self-unsubscribe",
            Some(self_unsubscribe),
            31usize as *mut c_void,
        );
        crate::services::finish(self_owner, true);
        activate_owner(self_owner);
        SELF_TOKEN.store(self_token, Ordering::Release);
        SEEN.lock().unwrap().clear();
        dispatch(1usize as *mut c_void);
        assert_eq!(*SEEN.lock().unwrap(), [31]);
        assert_eq!(unsubscribe(self_token), 0);
        assert_eq!(unsubscribe(self_token), 1);
        clear(self_owner, core_owner);
        crate::services::remove(consumer);
    }

    #[test]
    fn failed_new_core_init_does_not_retire_the_active_provider() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let core_owner = 0x5601;
        let consumer = 0x5602;
        setup_core(core_owner, true);
        let token = subscribe_consumer(
            consumer,
            "test.active-provider",
            Some(record),
            56usize as *mut c_void,
        );
        crate::services::finish(consumer, true);
        activate_owner(consumer);

        begin(0x5603, "defiance.core");
        finish(0x5603, false);
        dispatch(1usize as *mut c_void);
        assert_eq!(*SEEN.lock().unwrap(), [56]);
        assert_eq!(unsubscribe(token), 0);
        clear(consumer, core_owner);
    }

    #[test]
    fn disabled_candidate_keeps_its_dependency_plan_rank_when_enabled() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let mut nodes = [
            crate::plan::Planned {
                id: "rank.a".into(),
                dll: String::new(),
                path: Default::default(),
                manifest: None,
                legacy: true,
                decision: crate::plan::Decision::Initialize,
                depends: vec![],
            },
            crate::plan::Planned {
                id: "rank.b".into(),
                dll: String::new(),
                path: Default::default(),
                manifest: None,
                legacy: true,
                decision: crate::plan::Decision::Initialize,
                depends: vec!["rank.c".into()],
            },
            crate::plan::Planned {
                id: "rank.c".into(),
                dll: String::new(),
                path: Default::default(),
                manifest: None,
                legacy: true,
                decision: crate::plan::Decision::Disabled {
                    reason: "off".into(),
                },
                depends: vec!["rank.a".into()],
            },
        ];
        set_plan_order(&nodes, &[0, 1]);
        let ranks = registry().lock().unwrap().plan_order.clone();
        assert!(
            ranks.iter().position(|id| id == "rank.a") < ranks.iter().position(|id| id == "rank.c")
        );
        assert!(
            ranks.iter().position(|id| id == "rank.c") < ranks.iter().position(|id| id == "rank.b")
        );
        nodes[2].decision = crate::plan::Decision::Initialize;
        set_plan_order(&nodes, &[0, 2, 1]);
        assert_eq!(registry().lock().unwrap().plan_order, ranks);
    }

    #[test]
    fn lifecycle_does_not_call_stop_while_an_observer_is_in_flight() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let core_owner = 0x5701;
        let owner = 0x5702;
        setup_core(core_owner, true);
        let token = subscribe_consumer(owner, "test.lifecycle", Some(block), core::ptr::null_mut());
        crate::services::finish(owner, true);
        activate_owner(owner);
        STOPPED.store(0, Ordering::Release);
        crate::lifecycle::record(crate::lifecycle::Loaded {
            owner,
            id: "test.lifecycle".into(),
            version: None,
            name: "lifecycle fixture".into(),
            path: Default::default(),
            shadow: Default::default(),
            module: 0,
            code: vec![],
            stop: Some(stop_counter),
            reloadable: true,
            stamp: None,
            multiplayer_safe: true,
            manifest_settings: None,
        });
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        *BLOCK.get_or_init(|| Mutex::new(None)).lock().unwrap() =
            Some((entered.clone(), release.clone()));
        let thread = std::thread::spawn(|| dispatch(1usize as *mut c_void));
        entered.wait();
        assert_eq!(
            crate::lifecycle::unload(owner, true).err(),
            Some(crate::lifecycle::UnloadError::Busy)
        );
        assert_eq!(STOPPED.load(Ordering::Acquire), 0);
        release.wait();
        thread.join().unwrap();
        assert!(crate::lifecycle::unload(owner, true).is_ok());
        assert_eq!(STOPPED.load(Ordering::Acquire), 1);
        assert_eq!(unsubscribe(token), 1);
        crate::lifecycle::forget_retained(owner);
        *BLOCK.get().unwrap().lock().unwrap() = None;
        clear(owner, core_owner);
    }

    #[test]
    fn unload_refused_on_its_own_stack_keeps_observers() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let core_owner = 0x5801;
        let owner = 0x5802;
        setup_core(core_owner, true);
        let token = subscribe_consumer(
            owner,
            "test.own-stack",
            Some(record),
            58usize as *mut c_void,
        );
        crate::services::finish(owner, true);
        activate_owner(owner);
        STOPPED.store(0, Ordering::Release);
        crate::lifecycle::record(crate::lifecycle::Loaded {
            owner,
            id: "test.own-stack".into(),
            version: None,
            name: "own-stack fixture".into(),
            path: Default::default(),
            shadow: Default::default(),
            module: 0,
            // Every address, so the calling thread is always inside it.
            code: vec![(0, usize::MAX)],
            stop: Some(stop_counter),
            reloadable: true,
            stamp: None,
            multiplayer_safe: true,
            manifest_settings: None,
        });
        assert_eq!(
            crate::lifecycle::unload(owner, true).err(),
            Some(crate::lifecycle::UnloadError::Busy)
        );
        assert_eq!(STOPPED.load(Ordering::Acquire), 0);
        dispatch(1usize as *mut c_void);
        assert_eq!(*SEEN.lock().unwrap(), [58]);
        assert_eq!(unsubscribe(token), 0);
        crate::lifecycle::forget(owner);
        clear(owner, core_owner);
    }
}
