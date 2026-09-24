//! Which active plugins block multiplayer.
//!
//! The gameplay plugins change local simulation and send nothing over the
//! network, so a multiplayer game with one active would desync. A plugin whose
//! manifest declares `multiplayer_safe` does not count; every other active
//! plugin does, a legacy one (no manifest) included. The host records the list
//! once startup has run; Core's guard on the game's lobby connection reads it
//! through the loader's `multiplayer` service ([`API`]) and refuses to go
//! online while it is not empty.
//!
//! The loader fails closed: Core reports the guard installed through the same
//! service, and until it has, no plugin that is not multiplayer-safe starts
//! (`host::run_plan`).
use crate::plan::{Decision, Plan};
use core::ffi::c_char;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static BLOCKERS: OnceLock<String> = OnceLock::new();
static GUARDED: AtomicBool = AtomicBool::new(false);

/// Whether Core has reported the multiplayer guard installed.
pub fn guarded() -> bool {
    GUARDED.load(Ordering::Acquire)
}

/// The IDs of the planned plugins that would block multiplayer if active:
/// `active` says which ones initialized.
pub fn blockers(plan: &Plan, active: &[bool]) -> Vec<String> {
    plan.nodes
        .iter()
        .zip(active)
        .filter(|&(node, &active)| {
            active
                && matches!(node.decision, Decision::Initialize)
                && !crate::plan::multiplayer_safe(node)
        })
        .map(|(node, _)| node.id.clone())
        .collect()
}

/// Record the blocking plugins, once, and log the outcome.
pub fn record(ids: &[String]) {
    let text = ids.join(", ");
    if ids.is_empty() {
        crate::log::info("multiplayer: allowed (every active plugin is multiplayer-safe)");
    } else {
        crate::log::info(&format!("multiplayer: blocked while active: {text}"));
    }
    let _ = BLOCKERS.set(text);
}

/// Copy the comma-separated IDs, NUL-terminated, into `buffer` when it holds
/// them. Returns their length without the NUL: 0 when nothing blocks.
fn copy_blockers(text: &str, buffer: *mut c_char, capacity: usize) -> usize {
    if !buffer.is_null() && capacity > text.len() {
        unsafe {
            core::ptr::copy_nonoverlapping(text.as_ptr(), buffer.cast::<u8>(), text.len());
            *buffer.add(text.len()) = 0;
        }
    }
    text.len()
}

unsafe extern "C" fn service_blockers(buffer: *mut c_char, capacity: usize) -> usize {
    // Before startup has finished the answer is not known yet: say something
    // blocks, so the guard never lets a game go online early.
    let text = BLOCKERS.get().map_or("(startup)", String::as_str);
    copy_blockers(text, buffer, capacity)
}

unsafe extern "C" fn service_guard_installed() {
    GUARDED.store(true, Ordering::Release);
}

/// `defiance.loader` / `multiplayer` v1.
pub static API: defiance_api::MultiplayerV1 = defiance_api::MultiplayerV1 {
    blockers: service_blockers,
    guard_installed: service_guard_installed,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blockers_are_copied_only_when_they_fit() {
        let mut buffer = [0x55 as c_char; 8];
        assert_eq!(copy_blockers("a, b", buffer.as_mut_ptr(), 8), 4);
        assert_eq!(
            unsafe { core::ffi::CStr::from_ptr(buffer.as_ptr()) },
            c"a, b"
        );
        let mut short = [0x55 as c_char; 4];
        assert_eq!(copy_blockers("a, b", short.as_mut_ptr(), 4), 4);
        assert_eq!(short[0], 0x55, "a buffer too small is left alone");
        assert_eq!(copy_blockers("", core::ptr::null_mut(), 0), 0);
    }
}
