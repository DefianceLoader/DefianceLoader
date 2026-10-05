//! Experimental locomotion during grenade throws and weapon changes.
//!
//! The chassis speed getter maps these actions to zero while retaining its
//! movement path. Redirect only their switch-table entries to the existing
//! posture-dependent speed calculation; action timing remains in the gunner.
use defiance_api::{Api, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO, LOG_WARN};
use defiance_core::sites::Image;

mod sites;

unsafe fn log(api: &Api, level: u32, message: &str) {
    if let Ok(message) = std::ffi::CString::new(message) {
        (api.log)(level, message.as_ptr());
    }
}

enum InstallError {
    /// The getter did not resolve: this build is not one the plugin supports.
    UnsupportedBuild(String),
    Failed(String),
}

unsafe fn install(api: &Api) -> Result<(), InstallError> {
    let base = (api.module_base)(c"logic.dll".as_ptr());
    if base.is_null() {
        return Err(InstallError::Failed("logic.dll is not loaded".into()));
    }
    let size = (api.module_size)(base);
    let base = base.cast::<u8>();
    let getter =
        sites::getter(&Image::loaded(base, size)).map_err(InstallError::UnsupportedBuild)?;
    for rva in getter.patches {
        let before = 1u8;
        let after = 3u8;
        // All sites are checked together above. The loader owns these writes
        // and restores them if init fails or the plugin is removed.
        if (api.patch_bytes)(base.add(rva).cast(), &before, &after, 1) != 0 {
            return Err(InstallError::Failed(format!(
                "action-table patch failed at {rva:#x}"
            )));
        }
    }
    Ok(())
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else {
        return 1;
    };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    match install(api) {
        Ok(()) => {
            log(api, LOG_INFO, "moving actions installed: grenade throws and weapon changes retain movement; 3 table bytes patched");
            0
        }
        Err(InstallError::UnsupportedBuild(reason)) => {
            log(
                api,
                LOG_WARN,
                &format!("moving actions: not a supported build ({reason}); no writes made"),
            );
            0
        }
        Err(InstallError::Failed(error)) => {
            log(api, LOG_ERROR, &format!("moving actions refused: {error}"));
            1
        }
    }
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.moving-actions".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();
