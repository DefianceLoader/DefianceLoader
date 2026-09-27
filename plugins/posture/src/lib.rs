// Posture feature: its logic.dll unit (patch/posture-gate.asm, prone-query.asm and
// the pose split, pose.asm), which Core's patch service installs for the build.
use defiance_api::{Api, Plugin, ABI_VERSION};
use defiance_feature_sdk::units::Embedded;
defiance_feature_sdk::service_handshake!();

/// Every supported build's units (`build.rs`).
static UNITS: &[Embedded] = include!(concat!(env!("OUT_DIR"), "/units.rs"));

unsafe extern "C" fn init(api: *const Api) -> i32 {
    unsafe { defiance_feature_sdk::units::install(api, UNITS, |_| {}, &[], core::ptr::null_mut()) }
}
#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: b"defiance.posture\0".as_ptr().cast(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();
