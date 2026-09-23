//! Native hook lifecycle in a real process with real thread suspension.
//!
//! The library's own unit tests run `stop_the_world` as a no-op so they do not
//! suspend the test harness. An integration test links the library normally, so
//! this exercises the production safe point: a hook is published and taken back
//! out while worker threads are calling the target.
#![cfg(feature = "test-host")]

use core::ffi::c_void;
use defiance_loader::test_host;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The stock function the workers call.
#[inline(never)]
extern "C" fn target() -> u64 {
    1
}

/// The trampoline the loader stores *before* it publishes the branch, so a
/// detour that fires immediately has it.
static mut ORIGINAL: *mut c_void = core::ptr::null_mut();

/// The detour: return 2, after proving the stock original still works.
unsafe extern "C" fn detour() -> u64 {
    let original: unsafe extern "C" fn() -> u64 = unsafe { core::mem::transmute(ORIGINAL) };
    let stock = unsafe { original() };
    assert_eq!(stock, 1, "the trampoline must run the displaced original");
    2
}

#[test]
fn hooks_publish_and_remove_safely_under_thread_load() {
    let api = test_host::build_api();
    let stop = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();
    for _ in 0..4 {
        let stop = stop.clone();
        workers.push(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let value = target();
                assert!(value == 1 || value == 2, "target returned {value}");
            }
        }));
    }
    std::thread::sleep(Duration::from_millis(20));

    test_host::begin_plugin(1, "lifecycle");
    let hooked = unsafe {
        (api.hook)(
            target as *const () as *mut c_void,
            detour as *const () as *mut c_void,
            core::ptr::addr_of_mut!(ORIGINAL),
        )
    };
    test_host::end_plugin();
    assert_eq!(hooked, 0, "publishing the hook failed");
    assert!(
        !unsafe { ORIGINAL }.is_null(),
        "the original pointer must be stored"
    );
    assert_eq!(target(), 2, "the detour must be published");

    // The workers are running the detour now; remove it while they spin.
    test_host::begin_plugin(1, "lifecycle");
    let removed = unsafe { (api.unhook)(target as *const () as *mut c_void) };
    test_host::end_plugin();
    assert_eq!(removed, 0, "removing the hook failed");
    assert_eq!(target(), 1, "the site must be stock again");

    stop.store(true, Ordering::Relaxed);
    for worker in workers {
        worker.join().unwrap();
    }
}
