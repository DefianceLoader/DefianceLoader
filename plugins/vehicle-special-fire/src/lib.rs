use defiance_api::{Api, Plugin, ABI_VERSION, LOG_WARN};
use defiance_feature_sdk::units::Embedded;

mod native;
mod orders;
mod sites;
mod targeting;
mod ui;
defiance_feature_sdk::service_handshake!();

/// Every supported build's unit (`build.rs`).
static UNITS: &[Embedded] = include!(concat!(env!("OUT_DIR"), "/units.rs"));
#[no_mangle]
pub unsafe extern "C" fn defiance_patch_contract_v1(
    api: *const Api,
) -> *const defiance_api::PatchContractV1 {
    let Some(resolved) =
        (unsafe { api.as_ref() }).and_then(|api| unsafe { native::resolve(api) }.ok())
    else {
        return core::ptr::null();
    };
    let units = unsafe { defiance_feature_sdk::units::patch_contract(api, UNITS, &[], false) };
    unsafe { native::contract(units, &resolved) }
}

/// Resolves every native site before writing anything, so a build where one
/// is missing gets neither the units nor the hooks.
unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(host) = (unsafe { api.as_ref() }) else {
        return 1;
    };
    let resolved = match unsafe { native::resolve(host) } {
        Ok(resolved) => resolved,
        Err(error) => {
            native::log(
                host,
                LOG_WARN,
                &format!(
                    "vehicle special fire: not a supported build ({error}); no hooks installed"
                ),
            );
            return 1;
        }
    };
    let status = unsafe {
        defiance_feature_sdk::units::install(api, UNITS, |_| {}, &[], core::ptr::null_mut())
    };
    if status != 0 {
        return status;
    }
    unsafe { native::install(host, &resolved) }
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: b"defiance.vehicle-special-fire\0".as_ptr().cast(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();
