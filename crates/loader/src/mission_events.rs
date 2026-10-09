//! Mission load, end and frame events for plugins (`mission-events` v1), so
//! plugins share one hook on each instead of competing for the address.
//!
//! [`crate::session`] dispatches [`MISSION_LOADED`] and [`MISSION_ENDED`] when
//! the loaded missions go from 0 to 1 and back. Core drives
//! [`MISSION_FRAME`] through `mission-feed` v1 ([`FEED`]) from its hook on
//! world2.dll's mission frame; frames are dispatched only while
//! [`crate::session::GATE`] reports a mission.
//!
//! Subscriptions belong to the initializing plugin and are dropped by
//! [`close_owner`] when it fails init, withdraws or unloads. Dispatch holds the
//! read lock for the whole call, so a plugin's callbacks have returned before
//! its subscriptions are dropped and its library is freed.
use defiance_api::{
    MissionEventFn, MissionEventsV1, MissionFeedV1, MissionFrameV1, MISSION_ENDED, MISSION_FRAME,
    MISSION_LOADED,
};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::RwLock;

/// The events a subscription may ask for.
const EVENTS: u32 = MISSION_LOADED | MISSION_ENDED | MISSION_FRAME;

struct Subscription {
    owner: usize,
    mask: u32,
    callback: MissionEventFn,
    context: usize,
}

static SUBSCRIPTIONS: RwLock<Vec<Subscription>> = RwLock::new(Vec::new());
static NEXT: AtomicU64 = AtomicU64::new(1);
static FRAMES: AtomicBool = AtomicBool::new(false);

fn subscribe(owner: usize, mask: u32, callback: MissionEventFn, context: *mut c_void) -> u64 {
    let mask = mask & EVENTS;
    if mask == 0 {
        return 0;
    }
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    SUBSCRIPTIONS
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .push(Subscription {
            owner,
            mask,
            callback,
            context: context as usize,
        });
    id
}

/// Drop every subscription an owner made, during rollback, withdrawal or
/// unload.
pub(crate) fn close_owner(owner: usize) {
    SUBSCRIPTIONS
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|s| s.owner != owner);
}

fn dispatch(event: u32, frame: *const MissionFrameV1) {
    let subscriptions = SUBSCRIPTIONS.read().unwrap_or_else(|p| p.into_inner());
    for s in subscriptions.iter().filter(|s| s.mask & event != 0) {
        unsafe { (s.callback)(s.context as *mut c_void, event, frame) };
    }
}

/// The loaded missions went from 0 to 1 ([`MISSION_LOADED`]) or from 1 to 0
/// ([`MISSION_ENDED`]). Called by [`crate::session`].
pub(crate) fn transition(before: i32, delta: i32) {
    if before == 0 && delta > 0 {
        dispatch(MISSION_LOADED, core::ptr::null());
    } else if before == 1 && delta < 0 {
        dispatch(MISSION_ENDED, core::ptr::null());
    }
}

unsafe extern "C" fn service_subscribe(
    mask: u32,
    callback: MissionEventFn,
    context: *mut c_void,
) -> u64 {
    match crate::services::current_owner() {
        Some(owner) => subscribe(owner, mask, callback, context),
        None => 0,
    }
}

unsafe extern "C" fn service_frames() -> i32 {
    FRAMES.load(Ordering::Acquire) as i32
}

unsafe extern "C" fn feed_frames() {
    FRAMES.store(true, Ordering::Release);
}

unsafe extern "C" fn feed_frame(scene: *mut c_void, dt: f32) {
    if !crate::session::GATE.in_mission() {
        return;
    }
    let frame = MissionFrameV1 { scene, dt };
    dispatch(MISSION_FRAME, &frame);
}

/// `defiance.loader` / `mission-events` v1.
pub static API: MissionEventsV1 = MissionEventsV1 {
    subscribe: service_subscribe,
    frames: service_frames,
};

/// `defiance.loader` / `mission-feed` v1, for Core.
pub static FEED: MissionFeedV1 = MissionFeedV1 {
    frames: feed_frames,
    frame: feed_frame,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Tests share the global subscriptions; each uses its own owners.
    static SERIAL: Mutex<()> = Mutex::new(());
    static SEEN: Mutex<Vec<(usize, u32, f32)>> = Mutex::new(Vec::new());

    unsafe extern "C" fn record(context: *mut c_void, event: u32, frame: *const MissionFrameV1) {
        let dt = unsafe { frame.as_ref() }.map_or(-1.0, |f| f.dt);
        SEEN.lock().unwrap().push((context as usize, event, dt));
    }

    fn take() -> Vec<(usize, u32, f32)> {
        std::mem::take(&mut *SEEN.lock().unwrap())
    }

    #[test]
    fn subscribing_outside_init_or_to_nothing_is_refused() {
        let _serial = SERIAL.lock().unwrap();
        let none = unsafe { service_subscribe(EVENTS, record, core::ptr::null_mut()) };
        assert_eq!(none, 0);
        assert_eq!(subscribe(0x5100, 0, record, core::ptr::null_mut()), 0);
        assert_eq!(subscribe(0x5100, 0x100, record, core::ptr::null_mut()), 0);
    }

    #[test]
    fn transitions_reach_their_subscribers_until_the_owner_closes() {
        let _serial = SERIAL.lock().unwrap();
        take();
        let a = subscribe(0x5200, MISSION_LOADED, record, 0xa as *mut c_void);
        let b = subscribe(
            0x5201,
            MISSION_LOADED | MISSION_ENDED,
            record,
            0xb as *mut c_void,
        );
        assert!(a != 0 && b != 0 && a != b);
        transition(0, 1);
        transition(1, 1);
        transition(2, -1);
        transition(1, -1);
        assert_eq!(
            take(),
            [
                (0xa, MISSION_LOADED, -1.0),
                (0xb, MISSION_LOADED, -1.0),
                (0xb, MISSION_ENDED, -1.0),
            ]
        );
        close_owner(0x5200);
        close_owner(0x5201);
        transition(0, 1);
        assert_eq!(take(), []);
    }

    #[test]
    fn frames_carry_the_scene_and_step() {
        let _serial = SERIAL.lock().unwrap();
        take();
        subscribe(0x5300, MISSION_FRAME, record, 0xc as *mut c_void);
        let frame = MissionFrameV1 {
            scene: 0x1234 as *mut c_void,
            dt: 0.25,
        };
        dispatch(MISSION_FRAME, &frame);
        transition(0, 1);
        assert_eq!(take(), [(0xc, MISSION_FRAME, 0.25)]);
        close_owner(0x5300);
    }
}
