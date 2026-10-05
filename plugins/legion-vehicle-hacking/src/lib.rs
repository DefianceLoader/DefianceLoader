//! Allows hacking Legion vehicles while they are repairing themselves.
//!
//! The plugin removes only the final deny branch in `SmartCursorHacking`;
//! the game's earlier target and range checks remain in place. [`sites`]
//! finds the branch by signature and refuses a build where it does not resolve.

use defiance_api::{Api, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO, LOG_WARN};
use defiance_core::sites::Image;

mod sites;

const BEFORE: [u8; 2] = [0x75, 0x11];
const AFTER: [u8; 2] = [0x90, 0x90];

enum InstallError {
    /// The deny branch did not resolve: this build is not one the plugin
    /// supports.
    UnsupportedBuild(String),
    Failed(String),
}

unsafe fn log(api: &Api, level: u32, text: &str) {
    if let Ok(text) = std::ffi::CString::new(text) {
        (api.log)(level, text.as_ptr());
    }
}

unsafe fn install(api: &Api) -> Result<usize, InstallError> {
    let base = (api.module_base)(c"game.dll".as_ptr()).cast::<u8>();
    if base.is_null() {
        return Err(InstallError::Failed("game.dll is not loaded".into()));
    }
    let image = Image::loaded(base, (api.module_size)(base.cast()));
    let deny_branch = sites::deny_branch(&image).map_err(InstallError::UnsupportedBuild)?;
    let target = base.add(deny_branch);
    if (api.patch_bytes)(target.cast(), BEFORE.as_ptr(), AFTER.as_ptr(), BEFORE.len()) != 0 {
        return Err(InstallError::Failed(format!(
            "the deny branch at {deny_branch:#x} could not be patched"
        )));
    }
    Ok(deny_branch)
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else {
        return 1;
    };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    match install(api) {
        Ok(deny_branch) => {
            log(
                api,
                LOG_INFO,
                &format!("Legion vehicle hacking installed (deny branch at {deny_branch:#x})"),
            );
            0
        }
        Err(InstallError::UnsupportedBuild(error)) => {
            log(
                api,
                LOG_WARN,
                &format!("Legion vehicle hacking: not a supported build ({error}); no writes made"),
            );
            1
        }
        Err(InstallError::Failed(error)) => {
            log(
                api,
                LOG_ERROR,
                &format!("Legion vehicle hacking refused: {error}"),
            );
            1
        }
    }
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.legion-vehicle-hacking".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}
