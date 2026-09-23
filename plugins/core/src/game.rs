//! Shared game-thread access. These layouts match the reference assembly;
//! Core publishes this table only after supported-build preparation succeeds.
//!
//! One offset is build-specific: the squad AI facet's roster getter. The
//! 2026-09 update moved it from vtable slot +0x3b8 to +0x3d0, so Core sets it
//! to the selected build's value before publishing this table.
use core::ffi::c_void;
use defiance_api::{GameAccessV1, MemberStateV1};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The squad AI facet's roster getter slot; the reference build's value.
static ROSTER_SLOT: AtomicUsize = AtomicUsize::new(0x3b8);

/// Adopt a build's roster getter slot. Called once at preparation, before any
/// feature reads the table.
pub fn set_roster_slot(slot: usize) {
    ROSTER_SLOT.store(slot, Ordering::Relaxed);
}

unsafe fn ptr(p: *mut c_void, offset: usize) -> *mut c_void {
    unsafe { *p.cast::<u8>().add(offset).cast::<*mut c_void>() }
}
unsafe fn byte(p: *mut c_void, offset: usize) -> u8 {
    unsafe { *p.cast::<u8>().add(offset) }
}
unsafe fn word(p: *mut c_void, offset: usize) -> u16 {
    unsafe { *p.cast::<u8>().add(offset).cast::<u16>() }
}
unsafe fn method(p: *mut c_void, slot: usize) -> usize {
    unsafe { *ptr(p, 0).cast::<usize>().add(slot / 8) }
}
unsafe fn get(p: *mut c_void, slot: usize) -> *mut c_void {
    if p.is_null() {
        return core::ptr::null_mut();
    }
    let call: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
        unsafe { core::mem::transmute(method(p, slot)) };
    unsafe { call(p) }
}
unsafe extern "C" fn entity_from_facet(facet: *mut c_void) -> *mut c_void {
    if facet.is_null() {
        return core::ptr::null_mut();
    }
    let holder = unsafe { ptr(facet, 0x10) };
    if holder.is_null() {
        core::ptr::null_mut()
    } else {
        unsafe { ptr(holder, 0x10) }
    }
}
unsafe extern "C" fn selectable(entity: *mut c_void) -> *mut c_void {
    let facets = unsafe { get(entity, 0xb0) };
    if facets.is_null() {
        core::ptr::null_mut()
    } else {
        unsafe { ptr(facets, 0x50) }
    }
}
unsafe extern "C" fn copy_members(
    entity: *mut c_void,
    out: *mut *mut c_void,
    capacity: usize,
) -> usize {
    let facets = unsafe { get(entity, 0xb0) };
    if facets.is_null() {
        return usize::MAX;
    }
    let ai = unsafe { ptr(facets, 0x28) };
    let roster = unsafe { get(ai, ROSTER_SLOT.load(Ordering::Relaxed)) };
    let vector = unsafe { get(roster, 0x68) };
    if vector.is_null() {
        return usize::MAX;
    }
    let begin = unsafe { ptr(vector, 0) } as usize;
    let end = unsafe { ptr(vector, 8) } as usize;
    let Some(bytes) = end.checked_sub(begin) else {
        return usize::MAX;
    };
    if bytes % 8 != 0 || (begin == 0 && bytes != 0) || bytes > isize::MAX as usize {
        return usize::MAX;
    }
    let count = bytes / 8;
    if count != 0 && capacity >= count {
        if out.is_null() {
            return usize::MAX;
        }
        unsafe { core::ptr::copy_nonoverlapping(begin as *const *mut c_void, out, count) };
    }
    count
}
unsafe extern "C" fn read_member(
    entity: *mut c_void,
    expected: *mut c_void,
    out: *mut MemberStateV1,
) -> i32 {
    if out.is_null() {
        return 1;
    }
    unsafe { out.write(MemberStateV1::default()) };
    let facet = unsafe { selectable(entity) };
    if facet.is_null() || (!expected.is_null() && unsafe { ptr(facet, 0x28) } != expected) {
        return 1;
    }
    let selected: unsafe extern "C" fn(*mut c_void) -> u8 =
        unsafe { core::mem::transmute(method(facet, 0x58)) };
    unsafe {
        out.write(MemberStateV1 {
            selectable: facet,
            squad_selectable: ptr(facet, 0x28),
            selected: u8::from(selected(facet) != 0),
            enabled: byte(facet, 0x18),
            firing_pin: if word(facet, 0x1c) == 0x7a5f {
                byte(facet, 0x1b)
            } else {
                0
            },
            posture_pin: if word(facet, 0x32) == 0x7a5e {
                byte(facet, 0x31)
            } else {
                0
            },
            pin_flags: u8::from(word(facet, 0x1c) == 0x7a5f)
                | (u8::from(word(facet, 0x32) == 0x7a5e) << 1),
            raw_mark: byte(facet, 0x30),
            reserved: [0; 2],
        })
    };
    0
}
unsafe extern "C" fn set_firing_pin(facet: *mut c_void, value: u8, has_pin: u8) -> i32 {
    if facet.is_null() {
        return 1;
    }
    unsafe {
        let valid = word(facet, 0x1c) == 0x7a5f;
        if has_pin == 0 {
            if valid {
                *facet.cast::<u8>().add(0x1b) = 0;
            }
        } else {
            if !valid {
                *facet.cast::<u8>().add(0x1a).cast::<u16>() = 0;
                *facet.cast::<u8>().add(0x1c).cast::<u16>() = 0x7a5f;
            }
            *facet.cast::<u8>().add(0x1b) = value.wrapping_add(1);
        }
    }
    0
}
unsafe extern "C" fn squad_firing(ai: *mut c_void) -> u8 {
    if ai.is_null() {
        0
    } else {
        unsafe { byte(ai, 0x228) }
    }
}
unsafe extern "C" fn set_squad_firing(ai: *mut c_void, value: u8) -> i32 {
    if ai.is_null() {
        return 1;
    }
    unsafe { *ai.cast::<u8>().add(0x228) = value };
    0
}
pub static API: GameAccessV1 = GameAccessV1 {
    entity_from_facet,
    selectable,
    copy_members,
    read_member,
    set_firing_pin,
    squad_firing,
    set_squad_firing,
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roster_queries_never_partially_write_and_reject_malformed_ranges() {
        unsafe extern "C" fn field(p: *mut c_void) -> *mut c_void {
            unsafe { ptr(p, 8) }
        }
        let mut vt = vec![0usize; 128];
        for slot in [0xb0, 0x3b8, 0x68] {
            vt[slot / 8] = field as *const () as usize;
        }
        let members = [1usize, 2];
        let mut vector = [members.as_ptr() as usize, members.as_ptr() as usize + 16];
        let roster = [vt.as_ptr() as usize, vector.as_mut_ptr() as usize];
        let ai = [vt.as_ptr() as usize, roster.as_ptr() as usize];
        let mut facets = [0usize; 12];
        facets[0x28 / 8] = ai.as_ptr() as usize;
        let entity = [vt.as_ptr() as usize, facets.as_ptr() as usize];
        let p = entity.as_ptr() as *mut c_void;
        let mut out = [9usize as *mut c_void; 2];
        unsafe {
            assert_eq!(copy_members(p, out.as_mut_ptr(), 1), 2);
            assert_eq!(out, [9usize as *mut c_void; 2]);
            assert_eq!(copy_members(p, out.as_mut_ptr(), 2), 2);
            assert_eq!(out, [1usize as *mut c_void, 2usize as *mut c_void]);
            vector[1] = vector[0] + 3;
            assert_eq!(copy_members(p, out.as_mut_ptr(), 2), usize::MAX);
        }
    }
    #[test]
    fn pin_initialization_clearing_and_wrapping_preserve_other_bytes() {
        let mut bytes = [0xaau64; 8];
        let p = bytes.as_mut_ptr().cast::<c_void>();
        unsafe {
            assert_eq!(set_firing_pin(p, 255, 1), 0);
            assert_eq!(word(p, 0x1c), 0x7a5f);
            assert_eq!(word(p, 0x1a), 0);
            *p.cast::<u8>().add(0x1a) = 7;
            set_firing_pin(p, 0, 1);
            assert_eq!(byte(p, 0x1b), 1);
            assert_eq!(byte(p, 0x1a), 7);
            set_firing_pin(p, 0, 0);
            assert_eq!(byte(p, 0x1b), 0);
            assert_eq!(byte(p, 0x1a), 7);
        }
        assert_eq!(bytes[0], 0xaa);
    }
}
