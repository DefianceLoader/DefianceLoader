// Movement feature; uses the verified assembly in the shared runtime.
use defiance_api::{Api, Plugin, ABI_VERSION};
unsafe extern "C" fn init(api: *const Api) -> i32 {
    unsafe { defiance_feature_sdk::install(api, 3) }
}
#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: b"defiance.movement\0".as_ptr().cast(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();
