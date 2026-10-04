//! Allows hacking Legion vehicles while they are repairing themselves.
//!
//! The plugin removes only the final deny branch in `SmartCursorHacking`;
//! the game's earlier target and range checks remain in place. Supported
//! `game.dll` builds and verified branch locations are in [`sites`].

use core::ffi::c_void;
use defiance_api::{Api, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO};

mod sites;

const BEFORE: [u8; 2] = [0x75, 0x11];
const AFTER: [u8; 2] = [0x90, 0x90];

#[derive(Debug, PartialEq, Eq)]
enum SiteValidationError {
    OutOfBounds,
    UnexpectedBytes,
}

fn validate_site(image: &[u8], deny_branch: usize) -> Result<(), SiteValidationError> {
    let end = deny_branch
        .checked_add(BEFORE.len())
        .ok_or(SiteValidationError::OutOfBounds)?;
    let bytes = image
        .get(deny_branch..end)
        .ok_or(SiteValidationError::OutOfBounds)?;
    if bytes != BEFORE {
        return Err(SiteValidationError::UnexpectedBytes);
    }
    Ok(())
}

unsafe fn log(api: &Api, level: u32, text: &str) {
    if let Ok(text) = std::ffi::CString::new(text) {
        (api.log)(level, text.as_ptr());
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, capacity: u32) -> u32;
}

unsafe fn install(api: &Api) -> Result<&'static sites::Build, String> {
    let base = (api.module_base)(c"game.dll".as_ptr());
    if base.is_null() {
        return Err("game.dll is not loaded".into());
    }
    let size = (api.module_size)(base);
    let mut path = [0u16; 32768];
    let length = GetModuleFileNameW(base, path.as_mut_ptr(), path.len() as u32) as usize;
    if length == 0 || length >= path.len() {
        return Err("cannot resolve game.dll path".into());
    }
    use std::os::windows::ffi::OsStringExt;
    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&path[..length]));
    let sha = defiance_core::sha256::file(&path).map_err(|error| error.to_string())?;
    let build = sites::BUILDS
        .iter()
        .find(|build| build.sha == sha)
        .ok_or("unsupported game.dll; no writes made")?;

    let image = core::slice::from_raw_parts(base.cast::<u8>(), size);
    match validate_site(image, build.deny_branch) {
        Ok(()) => {}
        Err(SiteValidationError::OutOfBounds) => {
            return Err(format!(
                "the deny branch at {:#x} is outside game.dll; no writes made",
                build.deny_branch
            ));
        }
        Err(SiteValidationError::UnexpectedBytes) => {
            return Err(format!(
                "native code differs at {:#x}; no writes made",
                build.deny_branch
            ));
        }
    }
    let target = base.cast::<u8>().add(build.deny_branch);
    if (api.patch_bytes)(target.cast(), BEFORE.as_ptr(), AFTER.as_ptr(), BEFORE.len()) != 0 {
        return Err(format!(
            "the deny branch at {:#x} could not be patched",
            build.deny_branch
        ));
    }
    Ok(build)
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else {
        return 1;
    };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    match install(api) {
        Ok(build) => {
            log(
                api,
                LOG_INFO,
                &format!("Legion vehicle hacking installed ({})", build.name),
            );
            0
        }
        Err(error) => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_builds_keep_their_verified_deny_branch_sites() {
        let expected = [
            (
                "GOG 2025-12-23",
                "f0184b9fe358172c83261419c8ba3d822a0aa6b06ed3cddb2f7aa3ebb9653db4",
                0x3373c5,
            ),
            (
                "GOG 2026-09-14",
                "bc2af42369f9f6fe70e206ae4846f8f0e46ac01cca9a325c8159ee04a9b5e405",
                0x3395a5,
            ),
            (
                "GOG 2026-09-25",
                "8ec30a0b59aebf2240f00229d54a0f58e2e338f9ab3511046c9ff36970dd1489",
                0x3395a5,
            ),
            (
                "Steam 2025-12-23",
                "dc10419f417aed4ecff348b7c96b3c7c574a9a2541f76c5fbc35eb92dbc716d6",
                0x33d875,
            ),
            (
                "Steam 2026-09-22",
                "d926a213731d73bac8ccc56b50e2b4292fe8613c7a9bb7c8b9fc9132c122ed25",
                0x33fa85,
            ),
            (
                "Steam 2026-09-25",
                "c336b5ed4a367628a9c370457d82b1d4e75cab9007cffe5354e27688e836f98e",
                0x33fa85,
            ),
        ];

        assert_eq!(sites::BUILDS.len(), expected.len());
        for (actual, expected) in sites::BUILDS.iter().zip(expected) {
            assert_eq!((actual.name, actual.sha, actual.deny_branch), expected);
        }
    }

    #[test]
    fn accepts_the_expected_branch_at_the_end_of_the_image() {
        let mut image = vec![0; 8];
        image[6..].copy_from_slice(&BEFORE);

        assert_eq!(validate_site(&image, 6), Ok(()));
        assert_eq!(AFTER, [0x90, 0x90]);
    }

    #[test]
    fn refuses_out_of_bounds_and_overflowing_sites() {
        let image = [0; 8];

        assert_eq!(
            validate_site(&image, 7),
            Err(SiteValidationError::OutOfBounds)
        );
        assert_eq!(
            validate_site(&image, usize::MAX),
            Err(SiteValidationError::OutOfBounds)
        );
    }

    #[test]
    fn refuses_unexpected_native_bytes() {
        let image = [0; 8];

        assert_eq!(
            validate_site(&image, 2),
            Err(SiteValidationError::UnexpectedBytes)
        );
    }
}
