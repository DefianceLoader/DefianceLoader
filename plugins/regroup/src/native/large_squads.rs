//! Replacements for the stock perk refresh and UI roster export branches
//! that copy a squad into 20-entry buffers.
use super::*;

pub(super) static PERK_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

/// Stock perk refresh copies the entire squad into `Entity*[20]` on its stack.
/// At 22 members the copy overwrites its return address. Retain its ordinary
/// branches, replacing only the oversized squad branch with owned storage.
pub(super) unsafe extern "C" fn refresh_perks(perk: usize) {
    let original = || unsafe {
        std::mem::transmute::<usize, Unary>(PERK_ORIGINAL.load(Ordering::Relaxed))(perk)
    };
    if junction(q(perk, 0x160)) != 0 {
        original();
        return;
    }
    let e = entity(perk);
    if e == 0 || !kind(e, 0x10) {
        original();
        return;
    }
    let ai = q(facets(e), 0x28);
    let holder = if ai == 0 {
        0
    } else {
        get(ai, offsets().roster)
    };
    if holder == 0 {
        original();
        return;
    }
    let count = match pointers(holder, 0xa0, 64) {
        Ok(members) => members.len(),
        Err(reason) => {
            log(LOG_ERROR, &format!("perk refresh refused: {reason}"));
            return;
        }
    };
    if count <= 20 {
        original();
        return;
    }
    // Same order as the native squad branch: prepare shared effects, update
    // squad state, then snapshot the current roster and refresh each member.
    call(sites::PERK_PREPARE, perk);
    call(sites::PERK_UPDATE, perk);
    let members = match pointers(holder, 0xa0, 64) {
        Ok(members) => members,
        Err(reason) => {
            log(LOG_ERROR, reason);
            return;
        }
    };
    for member in members {
        let member_perk = q(facets(member), 0x70);
        if member_perk != 0 {
            call(sites::PERK_MEMBER, member_perk);
        }
    }
}
// The stock UI export resizes to 20, copies the full roster, then resizes to
// the returned count. Besides overflowing a fresh allocation, the final resize
// zeroes entries 21 onward. Use the game's allocator before copying.
pub(super) type ExportRoster = unsafe extern "C" fn(usize, usize, usize);
pub(super) static ROSTER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

pub(super) unsafe extern "C" fn export_roster(service: usize, e: usize, out: usize) {
    let original = || unsafe {
        std::mem::transmute::<usize, ExportRoster>(ROSTER_ORIGINAL.load(Ordering::Relaxed))(
            service, e, out,
        )
    };
    if e == 0 || !kind(e, 0x10) {
        original();
        return;
    }
    let ai = q(facets(e), 0x28);
    let holder = if ai == 0 {
        0
    } else {
        get(ai, offsets().roster)
    };
    if holder == 0 {
        original();
        return;
    }
    let members = match pointers(holder, 0xa0, 64) {
        Ok(members) => members,
        Err(reason) => {
            log(LOG_ERROR, &format!("UI roster export refused: {reason}"));
            std::mem::transmute::<usize, Pair>(address(sites::RESIZE_ROSTER))(out, 0);
            return;
        }
    };
    if members.len() <= 20 {
        original();
        return;
    }
    std::mem::transmute::<usize, Pair>(address(sites::RESIZE_ROSTER))(out, members.len());
    std::ptr::copy_nonoverlapping(members.as_ptr(), q(out, 0) as *mut usize, members.len());
}
