#[cfg(any(test, feature = "rust-chooser"))]
mod model;
#[cfg(feature = "rust-chooser")]
mod native;
// Rust is the default after differential tests and user-confirmed live testing.
// --no-default-features retains the assembly implementation for comparisons.
use defiance_api::{Api, Plugin, ABI_VERSION};
use defiance_feature_sdk::units::Embedded;
defiance_feature_sdk::service_handshake!();

/// Every supported build's units (`build.rs`): the chooser call and, for the
/// assembly build, the chooser itself (patch/pickup.asm).
static UNITS: &[Embedded] = include!(concat!(env!("OUT_DIR"), "/units.rs"));
defiance_feature_sdk::export_unit_patch_contract!(UNITS, &[], cfg!(feature = "rust-chooser"));

unsafe extern "C" fn init(api: *const Api) -> i32 {
    // The Rust chooser takes the unit's one call; the host owns that hook,
    // including any near relay.
    #[cfg(feature = "rust-chooser")]
    let detour = native::detour as *mut core::ffi::c_void;
    #[cfg(not(feature = "rust-chooser"))]
    let detour = core::ptr::null_mut();
    unsafe { defiance_feature_sdk::units::install(api, UNITS, |_| {}, &[], detour) }
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
