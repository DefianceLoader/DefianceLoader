//! Ordinary soldier getter/setter logic. Selection owns its mark/parent fields;
//! roster traversal and common member snapshots come from Core.
use core::ffi::c_void;
use defiance_api::{GameAccessV1, MemberStateV1};
use std::sync::OnceLock;
pub static GAME: OnceLock<&'static GameAccessV1> = OnceLock::new();

unsafe fn ptr(p: *mut c_void, offset: usize) -> *mut c_void {
    unsafe { *p.cast::<u8>().add(offset).cast::<*mut c_void>() }
}
unsafe fn byte(p: *mut c_void, offset: usize) -> u8 {
    unsafe { *p.cast::<u8>().add(offset) }
}
unsafe fn mark(p: *mut c_void, value: u8) {
    unsafe { *p.cast::<u8>().add(0x30) = value }
}
unsafe fn parent(facet: *mut c_void) -> Option<*mut c_void> {
    let parent = unsafe { ptr(facet, 0x28) };
    // Same plausibility gate as the assembly, not general pointer validation.
    if !(0x10000..0x800000000000).contains(&(parent as usize))
        || unsafe { ptr(parent, 0) }.is_null()
    {
        None
    } else {
        Some(parent)
    }
}
unsafe fn selected(facet: *mut c_void) -> u8 {
    let call: unsafe extern "C" fn(*mut c_void) -> u8 =
        unsafe { core::mem::transmute(*ptr(facet, 0).cast::<usize>().add(0x58 / 8)) };
    unsafe { call(facet) }
}
unsafe fn forward(facet: *mut c_void, value: u8) {
    let call: unsafe extern "C" fn(*mut c_void, u8) =
        unsafe { core::mem::transmute(*ptr(facet, 0).cast::<usize>().add(0x50 / 8)) };
    unsafe { call(facet, value) };
}
pub unsafe extern "C" fn get(facet: *mut c_void) -> u8 {
    if facet.is_null() {
        return 0;
    }
    if let Some(parent) = unsafe { parent(facet) } {
        if unsafe { selected(parent) } == 0 {
            return 0;
        }
    }
    u8::from(unsafe { byte(facet, 0x18) } != 0 && unsafe { byte(facet, 0x30) } != 0)
}
unsafe fn roster(parent: *mut c_void) -> Option<Vec<*mut c_void>> {
    let game = GAME.get()?;
    let entity = unsafe { (game.entity_from_facet)(parent) };
    unsafe { defiance_feature_sdk::services::members(game, entity) }
}
pub unsafe extern "C" fn set(facet: *mut c_void, value: u8) {
    if facet.is_null() {
        return;
    }
    let Some(parent) = (unsafe { parent(facet) }) else {
        unsafe { mark(facet, value) };
        return;
    };
    let squad_selected = unsafe { selected(parent) } != 0;
    if !squad_selected && value == 0 {
        unsafe { mark(facet, 0) };
        return;
    }
    if squad_selected && value != 0 {
        unsafe {
            mark(facet, value);
            forward(parent, value);
        }
        return;
    }
    // If the squad cannot be listed, unmarking conservatively keeps it selected.
    let mut another_marked = true;
    if let Some(members) = unsafe { roster(parent) } {
        another_marked = false;
        let game = GAME.get().unwrap();
        for entity in members {
            let mut member = MemberStateV1::default();
            if unsafe { (game.read_member)(entity, parent, &mut member) } != 0
                || member.selectable == facet
            {
                continue;
            }
            if value != 0 {
                unsafe { mark(member.selectable, 0) };
            } else if member.raw_mark != 0 && member.enabled != 0 {
                another_marked = true;
                break;
            }
        }
    }
    unsafe { mark(facet, value) };
    if value != 0 || !another_marked {
        unsafe { forward(parent, value) };
    }
}

#[cfg(feature = "parity-test")]
#[no_mangle]
pub unsafe extern "C" fn defiance_test_selection_api(game: *const GameAccessV1) -> i32 {
    if game.is_null() {
        return 1;
    }
    if GAME.set(unsafe { &*game }).is_ok() {
        0
    } else {
        1
    }
}
#[cfg(feature = "parity-test")]
#[no_mangle]
pub unsafe extern "C" fn defiance_test_selection_set(facet: *mut c_void, value: u8) {
    unsafe { set(facet, value) }
}
#[cfg(feature = "parity-test")]
#[no_mangle]
pub unsafe extern "C" fn defiance_test_selection_get(facet: *mut c_void) -> u8 {
    unsafe { get(facet) }
}
