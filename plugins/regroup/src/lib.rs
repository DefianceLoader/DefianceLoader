//! Experimental real squad regrouping. See README.md before live testing.
mod bindings;
mod hotkeys;
mod limits;
mod model;
#[cfg(windows)]
mod native;

defiance_feature_sdk::service_handshake!();

#[cfg(windows)]
#[no_mangle]
pub unsafe extern "C" fn defiance_plugin() -> *const defiance_api::Plugin {
    defiance_api::leak(defiance_api::Plugin {
        abi_version: defiance_api::ABI_VERSION,
        name: b"defiance.regroup\0".as_ptr().cast(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init: native::init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();
