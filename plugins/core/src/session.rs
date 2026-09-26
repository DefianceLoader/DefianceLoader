//! Report loaded missions to the loader, for hot reload.
//!
//! A mission is the game's `TacticalMapGameState`: its constructor and
//! destructor (the `tactical_state_ctor` / `tactical_state_dtor` sites in
//! `tools/icon.py`) are hooked, and each reports to the loader's `session`
//! service, which reloads plugins only while no mission is loaded or being
//! built. Before the constructor runs, Core calls `before_mission` (pending
//! reloads land then, and the state counts as being built); once it returns,
//! `mission(1)`; once the destructor returns, `mission(-1)`. So the whole of a
//! construction and a destruction counts as a mission.
use core::ffi::c_void;
use defiance_api::{Api, SessionV1, LOG_INFO, LOG_WARN};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

/// The constructor: the state, three register arguments and a fifth on the
/// stack (a mode, 0 to 2, which among other things decides whether the
/// mission can be saved), all forwarded as they came.
type Constructor =
    unsafe extern "system" fn(this: usize, a: usize, b: usize, c: usize, mode: usize) -> usize;
/// The destructor: the state alone; the rest pass through untouched.
type Method = unsafe extern "system" fn(this: usize, a: usize, b: usize, c: usize) -> usize;

static CTOR: AtomicUsize = AtomicUsize::new(0);
static DTOR: AtomicUsize = AtomicUsize::new(0);
static SERVICE: OnceLock<&'static SessionV1> = OnceLock::new();

unsafe extern "system" fn constructed(
    this: usize,
    a: usize,
    b: usize,
    c: usize,
    mode: usize,
) -> usize {
    // A mission starting or a save loading: pending reloads apply first.
    if let Some(service) = SERVICE.get() {
        unsafe { (service.before_mission)() };
    }
    let original: Constructor = unsafe { core::mem::transmute(CTOR.load(Ordering::Acquire)) };
    let result = unsafe { original(this, a, b, c, mode) };
    if let Some(service) = SERVICE.get() {
        unsafe { (service.mission)(1) };
    }
    result
}

unsafe extern "system" fn destroyed(this: usize, a: usize, b: usize, c: usize) -> usize {
    let original: Method = unsafe { core::mem::transmute(DTOR.load(Ordering::Acquire)) };
    let result = unsafe { original(this, a, b, c) };
    // Only now is the mission gone: a reload during the destructor would run
    // under a half-destroyed state.
    if let Some(service) = SERVICE.get() {
        unsafe { (service.mission)(-1) };
    }
    result
}

fn say(api: &Api, level: u32, text: &str) {
    let text = std::ffi::CString::new(text).unwrap_or_default();
    unsafe { (api.log)(level, text.as_ptr()) };
}

/// Hook the tactical state's constructor and destructor, during Core's
/// `init`. Without both, or without the loader's service, the loader does not
/// learn about missions and hot reload stays off.
pub fn install(api: &Api, constructor: usize, destructor: usize) {
    let Some(service) = (unsafe { defiance_feature_sdk::services::session() }) else {
        return;
    };
    let _ = SERVICE.set(service);
    for (address, detour, slot) in [
        (constructor, constructed as Constructor as usize, &CTOR),
        (destructor, destroyed as Method as usize, &DTOR),
    ] {
        let mut original = core::ptr::null_mut();
        let result =
            unsafe { (api.hook)(address as *mut c_void, detour as *mut c_void, &mut original) };
        if result != 0 || original.is_null() {
            say(
                api,
                LOG_WARN,
                &format!("session: the mission state could not be hooked ({result}); hot reload stays off"),
            );
            return;
        }
        slot.store(original as usize, Ordering::Release);
    }
    unsafe { (service.tracking)() };
    say(api, LOG_INFO, "session: missions tracked");
}

#[cfg(test)]
mod tests {
    use super::*;

    static SEEN: std::sync::Mutex<Option<[usize; 5]>> = std::sync::Mutex::new(None);

    unsafe extern "system" fn original(
        this: usize,
        a: usize,
        b: usize,
        c: usize,
        mode: usize,
    ) -> usize {
        *SEEN.lock().unwrap() = Some([this, a, b, c, mode]);
        this
    }

    static EVENTS: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    /// The hooks share the service and the stock slots, so the tests take turns.
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

    unsafe extern "C" fn fake_tracking() {}
    unsafe extern "C" fn fake_mission(delta: i32) {
        EVENTS.lock().unwrap().push(if delta > 0 {
            "mission +1"
        } else {
            "mission -1"
        });
    }
    unsafe extern "C" fn fake_before() {
        EVENTS.lock().unwrap().push("before mission");
    }
    static FAKE: SessionV1 = SessionV1 {
        tracking: fake_tracking,
        mission: fake_mission,
        before_mission: fake_before,
    };
    unsafe extern "system" fn stock_ctor(
        this: usize,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
    ) -> usize {
        EVENTS.lock().unwrap().push("constructor");
        this
    }
    unsafe extern "system" fn stock_dtor(this: usize, _: usize, _: usize, _: usize) -> usize {
        EVENTS.lock().unwrap().push("destructor");
        this
    }

    #[test]
    fn a_mission_is_reported_around_the_whole_construction_and_destruction() {
        let _turn = TURN.lock().unwrap_or_else(|p| p.into_inner());
        let _ = SERVICE.set(&FAKE);
        let saved = (CTOR.load(Ordering::Acquire), DTOR.load(Ordering::Acquire));
        CTOR.store(stock_ctor as Constructor as usize, Ordering::Release);
        DTOR.store(stock_dtor as Method as usize, Ordering::Release);
        EVENTS.lock().unwrap().clear();
        unsafe { constructed(1, 0, 0, 0, 0) };
        unsafe { destroyed(1, 0, 0, 0) };
        CTOR.store(saved.0, Ordering::Release);
        DTOR.store(saved.1, Ordering::Release);
        assert_eq!(
            *EVENTS.lock().unwrap(),
            [
                "before mission",
                "constructor",
                "mission +1",
                "destructor",
                "mission -1"
            ]
        );
    }

    #[test]
    fn the_constructor_hook_forwards_all_five_arguments() {
        let _turn = TURN.lock().unwrap_or_else(|p| p.into_inner());
        CTOR.store(original as Constructor as usize, Ordering::Release);
        let result = unsafe { constructed(0x10, 0x20, 0x30, 0x40, 2) };
        assert_eq!(result, 0x10);
        assert_eq!(*SEEN.lock().unwrap(), Some([0x10, 0x20, 0x30, 0x40, 2]));
    }
}
