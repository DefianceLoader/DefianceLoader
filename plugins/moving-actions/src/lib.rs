//! Experimental locomotion during grenade throws and weapon changes.
//!
//! The chassis speed getter maps these actions to zero while retaining its
//! movement path. Redirect only their switch-table entries to the existing
//! posture-dependent speed calculation; action timing remains in the gunner.
use core::ffi::c_void;
use defiance_api::{Api, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO};

mod sites;

struct Build {
    name: &'static str,
    sha: &'static str,
    rva: usize,
    before: &'static [u8],
    offsets: [usize; 3],
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, capacity: u32) -> u32;
}

unsafe fn log(api: &Api, level: u32, message: &str) {
    if let Ok(message) = std::ffi::CString::new(message) {
        (api.log)(level, message.as_ptr());
    }
}

unsafe fn install(api: &Api) -> Result<&'static Build, String> {
    let base = (api.module_base)(c"logic.dll".as_ptr());
    if base.is_null() {
        return Err("logic.dll is not loaded".into());
    }
    let size = (api.module_size)(base);
    let mut path = [0u16; 32768];
    let length = GetModuleFileNameW(base, path.as_mut_ptr(), path.len() as u32) as usize;
    if length == 0 || length >= path.len() {
        return Err("cannot resolve logic.dll path".into());
    }
    use std::os::windows::ffi::OsStringExt;
    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&path[..length]));
    let sha = defiance_core::sha256::file(&path).map_err(|e| e.to_string())?;
    let build = sites::BUILDS
        .iter()
        .find(|build| build.sha == sha)
        .ok_or("unsupported logic.dll")?;
    if build
        .rva
        .checked_add(build.before.len())
        .is_none_or(|end| end > size)
        || core::slice::from_raw_parts(base.cast::<u8>().add(build.rva), build.before.len())
            != build.before
    {
        return Err("movement-speed code or action table differs; no writes made".into());
    }
    for offset in build.offsets {
        let before = 1u8;
        let after = 3u8;
        // All sites are checked together above. The loader owns these writes
        // and restores them if init fails or the plugin is removed.
        if (api.patch_bytes)(
            base.cast::<u8>().add(build.rva + offset).cast(),
            &before,
            &after,
            1,
        ) != 0
        {
            return Err(format!(
                "action-table patch failed at {:#x}",
                build.rva + offset
            ));
        }
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
            log(api, LOG_INFO, &format!(
                "moving actions installed ({}): grenade throws and weapon changes retain movement; 3 table bytes patched",
                build.name
            ));
            0
        }
        Err(error) => {
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
