//! Address space held near the game's modules from the moment they load.
//!
//! Hooks and Core's unit pools must sit within rel32 reach (2GB) of the module
//! they patch. Both game DLLs prefer the same base, so at least one is
//! relocated, and on some machines both land low in the address space, where
//! `VirtualAlloc`'s bottom-up search places the game's own allocations. By the
//! time plugins ask, every page within reach can be taken. So the loader
//! reserves [`SLOTS`] 64K slots nearest each module as soon as it sees it
//! load, and [`take`] commits one in place when code needs it: committing a
//! reservation leaves no window in which another allocation could take it.
//! [`API`] offers the same to plugins as `defiance.loader` / `near-memory`.

use crate::win;
use core::ffi::c_void;
use std::sync::Mutex;

/// The modules whose neighbourhood is held.
const MODULES: [&str; 2] = ["logic.dll", "game.dll"];
/// Slots held near each module: Core takes one per module, and each loader
/// hook takes one for its stub. Reserved address space costs no memory, so
/// this is sized for every shipped plugin's hooks with room to spare.
const SLOTS: usize = 256;
/// `VirtualAlloc`'s allocation granularity: the smallest reservation.
const SLOT: usize = 0x10000;
/// The farthest a byte may be from the code that reaches it, matching the
/// outward search in [`crate::code::alloc_near`].
const REACH: usize = 0x7fff * SLOT;

/// Each held module's base and how many slots were reserved near it.
static MODULES_HELD: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
/// Reserved, uncommitted slots not yet handed out.
static FREE: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Holds slots near any watched module already loaded, then watches for the
/// rest to load. Call once, as early as the host can.
pub fn watch() {
    let ntdll = unsafe { win::GetModuleHandleW(win::wide("ntdll.dll").as_ptr()) };
    let register = if ntdll.is_null() {
        core::ptr::null_mut()
    } else {
        unsafe { win::GetProcAddress(ntdll, c"LdrRegisterDllNotification".as_ptr().cast()) }
    };
    if !register.is_null() {
        let register: win::RegisterDllNotification = unsafe { core::mem::transmute(register) };
        let mut cookie = core::ptr::null_mut();
        unsafe { register(0, loaded, core::ptr::null_mut(), &mut cookie) };
    }
    for name in MODULES {
        if let Some((base, size)) = crate::resolve::module(name) {
            hold(base as usize, size);
        }
    }
}

/// The loader's notification for each DLL that maps. It runs under the loader
/// lock, so it reserves and records and nothing else: no logging.
unsafe extern "system" fn loaded(reason: u32, data: *const win::DllNotification, _: *mut c_void) {
    if reason != win::DLL_LOADED || data.is_null() {
        return;
    }
    let data = unsafe { &*data };
    if data.base_name.is_null() {
        return;
    }
    let name = unsafe { &*data.base_name };
    if name.buffer.is_null() {
        return;
    }
    let name = unsafe { core::slice::from_raw_parts(name.buffer, name.length as usize / 2) };
    if MODULES.iter().any(|watched| same_name(name, watched)) {
        hold(data.base as usize, data.size as usize);
    }
}

/// Whether a UTF-16 file name is `watched`, ignoring ASCII case.
fn same_name(name: &[u16], watched: &str) -> bool {
    name.len() == watched.len()
        && name
            .iter()
            .zip(watched.bytes())
            .all(|(&c, w)| c < 0x80 && (c as u8).eq_ignore_ascii_case(&w))
}

/// Reserves up to [`SLOTS`] slots nearest the module at `base`, each within
/// reach of every byte of its `size`, and returns how many are held for it.
/// A module already held returns its earlier count.
pub fn hold(base: usize, size: usize) -> usize {
    let mut modules = MODULES_HELD.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(_, held)) = modules.iter().find(|(at, _)| *at == base) {
        return held;
    }
    let end = (base + size).next_multiple_of(SLOT);
    let start = base & !(SLOT - 1);
    let mut slots = Vec::with_capacity(SLOTS);
    for step in 0.. {
        let above = end + step * SLOT;
        let below = start.checked_sub((step + 1) * SLOT);
        let above = (above + SLOT - base <= REACH).then_some(above);
        let below = below.filter(|&at| at != 0 && base + size - at <= REACH);
        if above.is_none() && below.is_none() {
            break;
        }
        for at in [above, below].into_iter().flatten() {
            if slots.len() < SLOTS && reserve(at) {
                slots.push(at);
            }
        }
        if slots.len() == SLOTS {
            break;
        }
    }
    let held = slots.len();
    FREE.lock().unwrap_or_else(|e| e.into_inner()).extend(slots);
    modules.push((base, held));
    held
}

/// Reserves one slot at `at`; false when anything already occupies it.
fn reserve(at: usize) -> bool {
    let got = unsafe {
        win::VirtualAlloc(
            at as *mut c_void,
            SLOT,
            win::MEM_RESERVE,
            win::PAGE_EXECUTE_READWRITE,
        )
    };
    !got.is_null()
}

/// Commits `size` bytes of executable, writable memory in the held slot
/// nearest `hint` that a rel32 from `hint` reaches. None when `size` exceeds a
/// slot or no held slot reaches; the caller then searches as before. The
/// memory is released with `VirtualFree(address, 0, MEM_RELEASE)`.
pub fn take(hint: usize, size: usize) -> Option<usize> {
    if size == 0 || size > SLOT {
        return None;
    }
    let mut free = FREE.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let (index, &at) = free
            .iter()
            .enumerate()
            .filter(|(_, &at)| reaches(hint, at))
            .min_by_key(|(_, &at)| at.abs_diff(hint))?;
        free.swap_remove(index);
        let got = unsafe {
            win::VirtualAlloc(
                at as *mut c_void,
                size,
                win::MEM_COMMIT,
                win::PAGE_EXECUTE_READWRITE,
            )
        };
        if !got.is_null() {
            return Some(got as usize);
        }
        // Not committable (an exploit-protection policy, say): give the slot
        // back to the address space and try the next.
        unsafe { win::VirtualFree(at as *mut c_void, 0, win::MEM_RELEASE) };
    }
}

/// Whether every byte of the slot at `at` is within rel32 reach of `hint`.
fn reaches(hint: usize, at: usize) -> bool {
    if at >= hint {
        at + SLOT - hint <= REACH
    } else {
        hint - at <= REACH
    }
}

/// `defiance.loader` / `near-memory` v1.
pub static API: defiance_api::NearMemoryV1 = defiance_api::NearMemoryV1 { take: api_take };

unsafe extern "C" fn api_take(hint: usize, size: usize) -> usize {
    take(hint, size).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_ignoring_ascii_case() {
        let wide: Vec<u16> = "Logic.DLL".encode_utf16().collect();
        assert!(same_name(&wide, "logic.dll"));
        assert!(!same_name(&wide, "game.dll"));
        let longer: Vec<u16> = "logic.dll2".encode_utf16().collect();
        assert!(!same_name(&longer, "logic.dll"));
    }

    #[test]
    fn reach_covers_the_whole_slot() {
        let hint = 0x1_0000_0000;
        assert!(reaches(hint, hint + REACH - SLOT));
        assert!(!reaches(hint, hint + REACH - SLOT + 1));
        assert!(reaches(hint, hint - REACH));
        assert!(!reaches(hint, hint - REACH - 1));
    }

    #[test]
    fn held_slots_are_committed_near_the_module_and_counted_once() {
        // A stand-in module: a reservation of our own, so the slots land next
        // to it rather than next to a real DLL another test may hold.
        let base = unsafe {
            win::VirtualAlloc(
                core::ptr::null_mut(),
                4 * SLOT,
                win::MEM_RESERVE,
                win::PAGE_EXECUTE_READWRITE,
            )
        } as usize;
        assert_ne!(base, 0);
        let held = hold(base, 4 * SLOT);
        assert!(held > 0);
        assert_eq!(hold(base, 4 * SLOT), held);
        let at = take(base, 0x100).expect("a held slot");
        assert!(reaches(base, at));
        unsafe { (at as *mut u8).write_volatile(0xc3) };
        assert!(take(base, SLOT + 1).is_none());
        assert!(take(base, 0).is_none());
        unsafe {
            win::VirtualFree(at as *mut c_void, 0, win::MEM_RELEASE);
            win::VirtualFree(base as *mut c_void, 0, win::MEM_RELEASE);
        }
    }
}
