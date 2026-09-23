//! How the loader gets into the game, without an injector.
//!
//! The DLL this crate builds is a *proxy*: it is placed beside `trm.exe` under
//! the name of a system DLL the game imports (`version.dll`, `dxgi.dll`,
//! `winmm.dll`, `dinput8.dll`, ...). Windows finds ours first when it resolves
//! that import, so the game loads it as if it were the system one. Which name
//! to take comes from `trm.exe`'s import table, read by `tools/proxy.py`,
//! which generates a tail-jump forwarder for every export in
//! `proxy_generated.rs`; see `proxy.rs`. This replaces "start the injector,
//! then start the game" with "double-click the game": the store's shortcut
//! keeps working and there is no window to leave open.
//!
//! Windows resolves the real DLL as a load-time dependency before DllMain.
//! The entry point disables thread notifications and starts the host worker;
//! forwarding needs no initialization and works from another DLL's DllMain.

use crate::win;
use core::ffi::c_void;

/// The DLL entry point. `module` is our own handle; `reason` is one of the
/// `DLL_*` constants.
///
/// # Safety
/// Called by the OS loader. Must not be called by hand.
#[no_mangle]
pub unsafe extern "system" fn DllMain(
    module: win::Handle,
    reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    if reason == win::DLL_PROCESS_ATTACH {
        unsafe { win::DisableThreadLibraryCalls(module) };
        let mut id = 0u32;
        let thread = unsafe {
            win::CreateThread(
                core::ptr::null_mut(),
                0,
                host_thread,
                core::ptr::null_mut(),
                0,
                &mut id,
            )
        };
        if thread.is_null() {
            // The log is not open under the loader lock; the debugger channel
            // is safe here. Forwarding still works without the host.
            const MESSAGE: &[u8] = b"DefianceLoader: could not create the host thread\n\0";
            let mut wide = [0u16; MESSAGE.len()];
            for (to, from) in wide.iter_mut().zip(MESSAGE) {
                *to = *from as u16;
            }
            unsafe {
                win::OutputDebugStringW(wide.as_ptr());
            }
        } else {
            // Nothing joins this thread; closing the handle does not stop it.
            unsafe { win::CloseHandle(thread) };
        }
    }
    1
}

unsafe extern "system" fn host_thread(_parameter: *mut c_void) -> u32 {
    crate::host::run();
    0
}
