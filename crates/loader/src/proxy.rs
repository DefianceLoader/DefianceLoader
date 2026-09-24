//! The proxy forwards every export of the real system DLL (`REAL`) with a
//! naked tail jump through a slot (see proxy_generated.rs).
//!
//! Only one export, `ANCHOR`, is a load-time import, through
//! `\\?\GLOBALROOT\SystemRoot\System32` (independent of the Windows drive; see
//! build.rs for the import library). That makes Windows load and initialize
//! the real DLL before this one. `resolve`, the first thing DllMain does, then
//! fills every slot from the real module with GetProcAddress, under the loader
//! lock and before any module that imports from us is initialized. An export
//! this Windows lacks keeps `missing`: the export list comes from the build
//! machine, and importing every export at load time made the game unable to
//! start on Windows 10, whose dxgi.dll lacks Windows 11's exports.
pub use crate::proxy_generated::REAL;
use crate::proxy_generated::{slots, ANCHOR_EXPORT};
use crate::win;

/// Every forwarded export, as far as its slot is concerned: the tail jump keeps
/// the real signature, whatever it is.
pub(crate) type Target = unsafe extern "system" fn() -> i32;

const E_NOTIMPL: i32 = 0x8000_4001_u32 as i32;

/// The target of an export the real DLL does not have on this Windows: returns
/// E_NOTIMPL in the return register, whatever the caller expected.
pub(crate) unsafe extern "system" fn missing() -> i32 {
    E_NOTIMPL
}

/// Fill every slot from the real DLL. Called once, from DllMain, before any
/// importer of this DLL runs. Needs no allocation and loads nothing: the
/// anchor import has already loaded the real module.
///
/// # Safety
/// Only from DllMain's process attach.
pub(crate) unsafe fn resolve() {
    let mut module: win::Handle = core::ptr::null_mut();
    // FROM_ADDRESS | UNCHANGED_REFCOUNT: the module holding the anchor.
    let found = unsafe {
        win::GetModuleHandleExW(
            0x4 | 0x2,
            (&raw const ANCHOR_EXPORT).cast::<u16>(),
            &mut module,
        )
    };
    if found == 0 {
        return;
    }
    unsafe { fill(module) };
}

/// Point each slot at the export of the same name in `module`, when it has one.
///
/// # Safety
/// `module` must be a loaded module; slots must not be in use concurrently.
unsafe fn fill(module: win::Handle) {
    for (name, slot) in unsafe { slots() } {
        let address = unsafe { win::GetProcAddress(module, name.as_ptr()) };
        if !address.is_null() {
            unsafe { *slot = core::mem::transmute::<*mut core::ffi::c_void, Target>(address) };
        }
    }
}

/// The exports this Windows's real DLL does not provide, for the startup log.
pub fn unavailable() -> Vec<&'static str> {
    unsafe { slots() }
        .iter()
        .filter(|(_, slot)| unsafe { **slot } as usize == missing as Target as usize)
        .map(|(name, _)| core::str::from_utf8(&name[..name.len() - 1]).unwrap_or("?"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_export_returns_e_notimpl() {
        assert_eq!(unsafe { missing() }, E_NOTIMPL);
    }

    #[test]
    fn exports_a_module_lacks_keep_the_fallback() {
        // kernel32 has none of the forwarded names: every slot stays `missing`,
        // and a call through the export itself returns E_NOTIMPL.
        let name: Vec<u16> = "kernel32.dll\0".encode_utf16().collect();
        let kernel32 = unsafe { win::GetModuleHandleW(name.as_ptr()) };
        assert!(!kernel32.is_null());
        unsafe { fill(kernel32) };
        let all = unsafe { slots() }.len();
        assert_eq!(unavailable().len(), all);
        let first = unsafe { slots() }[0].1;
        assert_eq!(unsafe { (*first)() }, E_NOTIMPL);
    }
}
