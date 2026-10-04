//! Shared accessors for validated native engine objects.
use core::ffi::c_void;

pub(super) unsafe fn word(object: usize, offset: usize) -> usize {
    core::ptr::read_unaligned((object + offset) as *const usize)
}

pub(super) unsafe fn virtual_unary(object: usize, offset: usize) -> Option<usize> {
    if object == 0 {
        return None;
    }
    let table = word(object, 0);
    if table == 0 {
        return None;
    }
    let address = word(table, offset);
    if address == 0 {
        return None;
    }
    let function: unsafe extern "C" fn(usize) -> usize =
        core::mem::transmute::<*const c_void, _>(address as *const c_void);
    Some(function(object))
}
