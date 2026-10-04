// Rust controls with assembly retained for the register-sensitive game adapters
// (patch/firing-mode.asm, this plugin's logic.dll unit).
use defiance_api::{Api, Plugin, ABI_VERSION};
use defiance_feature_sdk::units::Embedded;
#[cfg(any(test, feature = "rust-controls"))]
mod model;
#[cfg(feature = "rust-controls")]
mod native;
defiance_feature_sdk::service_handshake!();

/// Every supported build's units (`build.rs`).
static UNITS: &[Embedded] = include!(concat!(env!("OUT_DIR"), "/units.rs"));
defiance_feature_sdk::export_unit_patch_contract!(
    UNITS,
    if cfg!(feature = "rust-controls") {
        &["firing_set", "firing_ui"]
    } else {
        &[]
    },
    false,
);

unsafe extern "C" fn init(api: *const Api) -> i32 {
    #[cfg(feature = "rust-controls")]
    let replacements = {
        let Some(game) = (unsafe { defiance_feature_sdk::services::game_access() }) else {
            return 1;
        };
        if native::GAME.set(game).is_err() {
            return 1;
        }
        [
            defiance_api::NativeReplacementV1 {
                name: c"firing_set".as_ptr(),
                detour: native::set as *mut _,
            },
            defiance_api::NativeReplacementV1 {
                name: c"firing_ui".as_ptr(),
                detour: native::ui as *mut _,
            },
        ]
    };
    #[cfg(not(feature = "rust-controls"))]
    let replacements: [defiance_api::NativeReplacementV1; 0] = [];
    unsafe {
        defiance_feature_sdk::units::install(
            api,
            UNITS,
            |_| {},
            &replacements,
            core::ptr::null_mut(),
        )
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
