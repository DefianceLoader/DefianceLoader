//! Experimental real squad regrouping. See README.md before live testing.
mod hotkeys;
mod limits;
mod model;
#[cfg(windows)]
mod native;
mod sites;

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

#[cfg(windows)]
#[no_mangle]
pub unsafe extern "C" fn defiance_patch_contract_v1(
    api: *const defiance_api::Api,
) -> *const defiance_api::PatchContractV1 {
    unsafe { native::patch_contract(api) }
}

defiance_feature_sdk::crash_handshake!();
