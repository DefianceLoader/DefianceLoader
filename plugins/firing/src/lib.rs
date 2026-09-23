// Rust controls with assembly retained for the register-sensitive game adapters.
use defiance_api::{Api, Plugin, ABI_VERSION};
#[cfg(any(test, feature = "rust-controls"))]
mod model;
#[cfg(feature = "rust-controls")]
mod native;
defiance_feature_sdk::service_handshake!();
unsafe extern "C" fn init(api: *const Api) -> i32 {
    #[cfg(feature = "rust-controls")]
    {
        let Some(game) = (unsafe { defiance_feature_sdk::services::game_access() }) else {
            return 1;
        };
        if native::GAME.set(game).is_err() {
            return 1;
        }
        let replacements = [
            defiance_api::NativeReplacementV1 {
                name: c"firing_set".as_ptr(),
                detour: native::set as *mut _,
            },
            defiance_api::NativeReplacementV1 {
                name: c"firing_ui".as_ptr(),
                detour: native::ui as *mut _,
            },
        ];
        unsafe { defiance_feature_sdk::install_native(api, 5, &replacements) }
    }
    #[cfg(not(feature = "rust-controls"))]
    {
        unsafe { defiance_feature_sdk::install(api, 5) }
    }
}
#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: b"defiance.firing\0".as_ptr().cast(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();
