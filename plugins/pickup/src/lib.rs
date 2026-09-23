#[cfg(any(test, feature = "rust-chooser"))]
mod model;
#[cfg(feature = "rust-chooser")]
mod native;
// Rust is the default after differential tests and user-confirmed live testing.
// --no-default-features retains the assembly implementation for comparisons.
use defiance_api::{Api, Plugin, ABI_VERSION};
unsafe extern "C" fn init(api: *const Api) -> i32 {
    #[cfg(feature = "rust-chooser")]
    {
        unsafe {
            defiance_feature_sdk::install_pickup_rust(api, native::detour as *mut core::ffi::c_void)
        }
    }
    #[cfg(not(feature = "rust-chooser"))]
    {
        unsafe { defiance_feature_sdk::install(api, 1) }
    }
}
#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: b"defiance.pickup\0".as_ptr().cast(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();
