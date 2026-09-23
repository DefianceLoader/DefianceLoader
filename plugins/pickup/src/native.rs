//! Game-specific unsafe bindings. Offsets match patch/pickup.asm.
//! The virtual getters must remain stable and read-only during this call.
use crate::model::{choose, Member};
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

static CURSOR: AtomicU32 = AtomicU32::new(0);

pub unsafe extern "system" fn detour(facet: *mut c_void, want: i32, allow: u8) -> *mut c_void {
    let mut cursor = CURSOR.load(Ordering::Relaxed);
    let before = cursor;
    let result = unsafe { choose_game(facet, want, allow, &mut cursor) };
    if cursor != before {
        CURSOR.store(cursor, Ordering::Relaxed);
    }
    result
}

// Explicit cursor only for the differential test DLL; absent from normal builds.
#[cfg(feature = "parity-test")]
#[no_mangle]
pub unsafe extern "system" fn defiance_pickup_test_choose(
    facet: *mut c_void,
    want: i32,
    allow: u8,
    cursor: *mut u32,
) -> *mut c_void {
    unsafe { choose_game(facet, want, allow, &mut *cursor) }
}

unsafe fn read_ptr(object: *mut c_void, offset: usize) -> *mut c_void {
    unsafe { *((object as *const u8).add(offset) as *const *mut c_void) }
}

unsafe fn read_u8(object: *mut c_void, offset: usize) -> u8 {
    unsafe { *((object as *const u8).add(offset)) }
}

unsafe fn read_i32(object: *mut c_void, offset: usize) -> i32 {
    unsafe { *((object as *const u8).add(offset) as *const i32) }
}

/// The function in a virtual slot: the object's vtable, then the pointer at
/// `offset` in it.
unsafe fn virtual_at(object: *mut c_void, offset: usize) -> usize {
    let vtable = unsafe { *(object as *const *const usize) };
    unsafe { *vtable.add(offset / 8) }
}

/// `member -> the held slot type`, 0 for nothing (vtable slot `+0x180`).
unsafe fn held(member: *mut c_void) -> i32 {
    let call: unsafe extern "system" fn(*mut c_void) -> i32 =
        unsafe { core::mem::transmute(virtual_at(member, 0x180)) };
    unsafe { call(member) }
}

/// `member -> the man`, which is the entity (vtable slot `+0xc8`).
unsafe fn man(member: *mut c_void) -> *mut c_void {
    let call: unsafe extern "system" fn(*mut c_void) -> *mut c_void =
        unsafe { core::mem::transmute(virtual_at(member, 0xc8)) };
    unsafe { call(member) }
}

/// Whether the player marked this member: member -> man -> facets (`+0xb0`) ->
/// the selectable facet at `facets + 0x50` -> `is marked` (`+0x58`).
unsafe fn marked(member: *mut c_void) -> bool {
    let man = unsafe { man(member) };
    if man.is_null() {
        return false;
    }
    let facets = {
        let call: unsafe extern "system" fn(*mut c_void) -> *mut c_void =
            unsafe { core::mem::transmute(virtual_at(man, 0xb0)) };
        unsafe { call(man) }
    };
    if facets.is_null() {
        return false;
    }
    let selectable = unsafe { read_ptr(facets, 0x50) };
    if selectable.is_null() {
        return false;
    }
    let call: unsafe extern "system" fn(*mut c_void) -> u8 =
        unsafe { core::mem::transmute(virtual_at(selectable, 0x58)) };
    unsafe { call(selectable) != 0 }
}

// --- the detour -------------------------------------------------------------

/// What runs in the game's stead, with the stock chooser's signature: `rcx` the
/// squad holder, `edx` the slot type, `r8b` allowSwap, returning the chosen
/// member's entity or null.
pub unsafe fn choose_game(
    facet: *mut c_void,
    want: i32,
    allow: u8,
    cursor: &mut u32,
) -> *mut c_void {
    if facet.is_null() || want < 0 {
        return core::ptr::null_mut();
    }
    let allow_swap = allow != 0;

    // noPickupGun is a property of the squad type, tested once
    let script = unsafe { read_ptr(facet, 0x240) };
    if script.is_null() || unsafe { read_u8(script, 0x1a9) } != 0 {
        return core::ptr::null_mut();
    }

    let begin = unsafe { read_ptr(facet, 0x1e8) } as *const *mut c_void;
    let end = unsafe { read_ptr(facet, 0x1f0) } as usize;
    let count = end.saturating_sub(begin as usize) / 8;
    if count == 0 {
        return core::ptr::null_mut();
    }

    let used = unsafe { read_i32(facet, 0x260 + want as usize * 8) };
    let max = unsafe { read_i32(facet, 0x264 + want as usize * 8) };
    let free_slot = used < max;

    let mut members = Vec::with_capacity(count);
    for i in 0..count {
        let member = unsafe { *begin.add(i) };
        if member.is_null() {
            return core::ptr::null_mut();
        }
        let held = unsafe { held(member) };
        let is_candidate = (held == 0 && free_slot) || (held != 0 && held == want && allow_swap);
        // the mark is only consulted for a candidate, as the assembly does
        let marked = is_candidate && unsafe { marked(member) };
        members.push(Member { held, marked });
    }

    match choose(&members, want, allow_swap, free_slot, *cursor) {
        Some((index, new_cursor)) => {
            if let Some(next) = new_cursor {
                *cursor = next;
            }
            unsafe { man(*begin.add(index)) }
        }
        None => core::ptr::null_mut(),
    }
}
