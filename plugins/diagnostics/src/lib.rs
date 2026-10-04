//! Reloadable diagnostic configuration, decoding and output.
//! Breakpoints and bounded at-hit snapshots belong to the loader.
use defiance_api::{Api, Plugin, ABI_VERSION};

mod config;
mod presets;
mod runtime;
defiance_feature_sdk::service_handshake!();

/// Hardware observation requests no loader-owned code writes.
#[no_mangle]
pub unsafe extern "C" fn defiance_patch_contract_v1(
    api: *const Api,
) -> *const defiance_api::PatchContractV1 {
    unsafe { defiance_feature_sdk::contract::build(api, Vec::new()) }
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    unsafe { runtime::start(api) }
}

unsafe extern "C" fn stop() {
    runtime::stop();
}
#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: b"defiance.diagnostics\0".as_ptr().cast(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: Some(stop),
    })
}

defiance_feature_sdk::crash_handshake!();
