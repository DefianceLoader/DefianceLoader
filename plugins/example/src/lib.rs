//! A template for a third-party plugin, in Rust. It shows the whole contract:
//! export `defiance_plugin`, return a `Plugin`, and do the work in `init`
//! through the `Api` — never by writing to the game's memory directly.
//!
//! The commented block in `init` is the shape of a real patch. `detour` would
//! be an `extern "system"` function with the hooked function's signature that
//! calls `original` for the stock behaviour. `hook` decodes whole instructions
//! itself; `hook_exact` is there only for a plugin that already knows the byte
//! count. When the address wanted is *inside* a signature, `find_pattern_at`
//! takes the offset, and the signature vouches for it.

use core::ffi::{c_char, CStr};
use defiance_api::{Api, Plugin, ABI_VERSION, LOG_INFO};

static NAME: &[u8] = b"defiance.example\0";
static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

unsafe extern "C" fn init(api: *const Api) -> i32 {
    if api.is_null() {
        return 1;
    }
    let api = unsafe { &*api };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    let log = api.log;
    let say = |message: String| {
        let mut text = message.into_bytes();
        text.push(0);
        unsafe { log(LOG_INFO, text.as_ptr() as *const c_char) };
    };
    say("defiance.example: loaded; nothing patched".to_string());

    // This legacy example has no manifest and reads defiance-loader.ini.
    // New plugins should use manifest-declared grouped settings; see
    // docs/plugin-authoring.md and examples/services for the complete workflow.
    //
    //   [defiance.example]
    //   greeting = hello
    //
    // config_get returns a string the loader owns, or null.
    let greeting = unsafe {
        (api.config_get)(
            b"defiance.example\0".as_ptr() as *const c_char,
            b"greeting\0".as_ptr() as *const c_char,
        )
    };
    if !greeting.is_null() {
        let text = unsafe { CStr::from_ptr(greeting) }.to_string_lossy();
        say(format!("defiance.example: greeting is {text}"));
    }

    // A real plugin looks a site up and hooks it:
    //
    //   let name = b"logic.dll\0";
    //   let base = unsafe { (api.module_base)(name.as_ptr() as *const c_char) };
    //   let size = unsafe { (api.module_size)(base) };
    //   let pattern = b"488b05????????4885c0\0";
    //   let site = unsafe {
    //       (api.find_pattern)(base, size, pattern.as_ptr() as *const c_char)
    //   };
    //   if site.is_null() {
    //       return 1; // this build is not one the signature fits
    //   }
    //   let mut original: *mut c_void = core::ptr::null_mut();
    //   if unsafe { (api.hook)(site, detour as *mut c_void, &mut original) } != 0 {
    //       return 1;
    //   }
    0
}

#[no_mangle]
pub unsafe extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: NAME.as_ptr() as *const c_char,
        version: VERSION.as_ptr() as *const c_char,
        init,
        stop: None,
    })
}
