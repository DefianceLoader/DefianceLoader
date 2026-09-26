//! Which active plugins block multiplayer.
//!
//! The gameplay plugins change local simulation and send nothing over the
//! network, so a multiplayer game with one active would desync. A plugin whose
//! manifest declares `multiplayer_safe` does not count; every other active
//! plugin does, a legacy one (no manifest) included, and so does an old copy
//! a hot reload kept mapped (its code still runs for whoever holds its tables).
//! The host records the list once startup has run; a hot reload blocks a
//! plugin that is not safe before loading it and recomputes the list from
//! what is active afterwards ([`block`], [`refresh`]). Core's guard on the
//! game's lobby connection reads it through the loader's `multiplayer`
//! service ([`API`]) and refuses to go online while it is not empty.
//!
//! The loader fails closed: Core reports the guard installed through the same
//! service, and until it has, no plugin that is not multiplayer-safe starts
//! (`host::run_plan`).
use crate::plan::{Decision, Plan};
use core::ffi::c_char;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// The blocking IDs; None until startup has recorded them.
static BLOCKERS: Mutex<Option<Vec<String>>> = Mutex::new(None);
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

/// Record the blocking plugins and log the outcome.
pub fn record(ids: &[String]) {
    let text = ids.join(", ");
    if ids.is_empty() {
        crate::log::info("multiplayer: allowed (every active plugin is multiplayer-safe)");
    } else {
        crate::log::info(&format!("multiplayer: blocked while active: {text}"));
    }
    *BLOCKERS.lock().unwrap_or_else(|p| p.into_inner()) = Some(ids.to_vec());
}

/// The IDs among `(id, multiplayer_safe)` that block, each once, in order.
pub fn blocking(plugins: impl IntoIterator<Item = (String, bool)>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (id, safe) in plugins {
        if !safe && !out.iter().any(|o| o.eq_ignore_ascii_case(&id)) {
            out.push(id);
        }
    }
    out
}

/// Add `id` to the blockers now, before a plugin that is not safe loads.
pub fn block(id: &str) {
    let mut blockers = BLOCKERS.lock().unwrap_or_else(|p| p.into_inner());
    let list = blockers.get_or_insert_with(Vec::new);
    if !list.iter().any(|o| o.eq_ignore_ascii_case(id)) {
        list.push(id.to_string());
    }
}

/// Recompute the blockers from the loaded plugins and the retained copies,
/// logging a change.
pub fn refresh() {
    let active = crate::lifecycle::loaded()
        .into_iter()
        .chain(crate::lifecycle::retained())
        .map(|p| (p.id, p.multiplayer_safe));
    let now = blocking(active);
    let mut blockers = BLOCKERS.lock().unwrap_or_else(|p| p.into_inner());
    if blockers.as_ref() != Some(&now) {
        crate::log::info(&if now.is_empty() {
            "multiplayer: allowed (every active plugin is multiplayer-safe)".to_string()
        } else {
            format!("multiplayer: blocked while active: {}", now.join(", "))
        });
    }
    *blockers = Some(now);
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
    let text = BLOCKERS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map_or_else(|| "(startup)".to_string(), |ids| ids.join(", "));
    copy_blockers(&text, buffer, capacity)
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
    fn only_plugins_that_are_not_safe_block_each_once() {
        let ids = blocking([
            ("safe".to_string(), true),
            ("gameplay".to_string(), false),
            ("Gameplay".to_string(), false), // its old copy, kept mapped
            ("other".to_string(), false),
        ]);
        assert_eq!(ids, ["gameplay", "other"]);
        assert!(blocking([("safe".to_string(), true)]).is_empty());
    }

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
