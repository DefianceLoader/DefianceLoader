//! The marquee's cell in the logic block (`patch/region-individual.asm`):
//! `GetAsyncKeyState` at +0, which it asks whether Ctrl is held, and the mode
//! byte at +8. Core fills it when selection installs, before the region hooks.

use core::ffi::c_void;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

/// The mode byte for selection's `marquee` setting: 0 selects the soldiers
/// inside the box, 1 (the default) whole squads, with Ctrl for soldiers.
pub fn mode(setting: &str) -> u8 {
    match setting {
        "soldiers" => 0,
        _ => 1,
    }
}

/// `GetAsyncKeyState`, or 0 when user32 cannot be loaded, which the patch
/// reads as Ctrl never held. The game has user32 loaded already.
fn key_state() -> usize {
    let name: Vec<u16> = "user32.dll\0".encode_utf16().collect();
    let module = unsafe { LoadLibraryW(name.as_ptr()) };
    if module.is_null() {
        return 0;
    }
    unsafe { GetProcAddress(module, b"GetAsyncKeyState\0".as_ptr()) as usize }
}

/// Fill the cell for `setting`.
///
/// # Safety
/// `cell` is the logic block's marquee cell: 16 writable bytes.
pub unsafe fn write(cell: usize, setting: &str) {
    (cell as *mut usize).write_volatile(key_state());
    ((cell + 8) as *mut u8).write_volatile(mode(setting));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squads_unless_soldiers_is_asked_for() {
        assert_eq!(mode("soldiers"), 0);
        assert_eq!(mode("squads"), 1);
        assert_eq!(mode(""), 1);
    }

    #[test]
    fn the_cell_holds_the_key_state_function_and_the_mode() {
        let mut cell = [0u64; 2];
        unsafe { write(cell.as_mut_ptr() as usize, "soldiers") };
        assert_ne!(cell[0], 0, "GetAsyncKeyState is found");
        assert_eq!(cell[1] & 0xff, 0);
        unsafe { write(cell.as_mut_ptr() as usize, "squads") };
        assert_eq!(cell[1] & 0xff, 1);
    }
}
