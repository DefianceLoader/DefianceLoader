//! Configurable destination slowdown for wheeled and tracked vehicles.
use core::ffi::c_void;
use defiance_api::{
    Api, PatchContractV1, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO, PATCH_KIND_ENTRY,
};
use std::sync::atomic::Ordering;

mod native;
mod sites;

const ID: &str = "defiance.vehicle-arrival";
const ENTRY: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x10, 0x57, 0x48, 0x83, 0xec, 0x20];
const METHOD_BYTES: usize = 0x14a;

struct Build {
    name: &'static str,
    sha: &'static str,
    update: usize,
    method_sha: &'static str,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, capacity: u32) -> u32;
}

unsafe fn log(api: &Api, level: u32, text: &str) {
    if let Ok(text) = std::ffi::CString::new(text) {
        (api.log)(level, text.as_ptr());
    }
}

fn validate(image: &[u8], build: &Build) -> Result<(), &'static str> {
    let end = build
        .update
        .checked_add(METHOD_BYTES)
        .ok_or("callback out of bounds")?;
    let body = image
        .get(build.update..end)
        .ok_or("callback out of bounds")?;
    let mut sha = defiance_core::sha256::Sha256::new();
    sha.update(body);
    if !body.starts_with(ENTRY) || defiance_core::sha256::hex(&sha.finish()) != build.method_sha {
        return Err("vehicle arrival callback differs; no writes made");
    }
    Ok(())
}

unsafe fn selected_build(api: &Api) -> Result<(*mut c_void, &'static Build), String> {
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
    let sha = defiance_core::sha256::file(&path).map_err(|error| error.to_string())?;
    let build = sites::BUILDS
        .iter()
        .find(|build| build.sha == sha)
        .ok_or("unsupported logic.dll; no writes made")?;
    validate(core::slice::from_raw_parts(base.cast::<u8>(), size), build)?;
    Ok((base, build))
}

unsafe fn configured_multiplier(api: &Api) -> Result<f32, String> {
    let percent = defiance_feature_sdk::integer(api, ID, "braking_window_percent")
        .map_err(|error| error.to_string())?;
    native::multiplier(percent).map_err(str::to_owned)
}

unsafe fn install(api: &Api) -> Result<String, String> {
    let multiplier = configured_multiplier(api)?;
    if multiplier == 1.0 {
        return Ok("vehicle arrival: stock braking window; no hook installed".into());
    }
    let (base, build) = selected_build(api)?;
    native::MULTIPLIER.store(multiplier.to_bits(), Ordering::Relaxed);
    if (api.hook_exact)(
        base.cast::<u8>().add(build.update).cast(),
        native::vehicle_arrival_update as *mut c_void,
        ENTRY.len(),
        native::ORIGINAL.as_ptr(),
    ) != 0
    {
        return Err("vehicle arrival hook refused; host will finish owned rollback".into());
    }
    Ok(format!(
        "vehicle arrival installed ({}): {:.0}% braking window",
        build.name,
        100.0 / multiplier
    ))
}

#[no_mangle]
pub unsafe extern "C" fn defiance_patch_contract_v1(api: *const Api) -> *const PatchContractV1 {
    let Some(host) = api.as_ref() else {
        return core::ptr::null();
    };
    if host.abi_version != ABI_VERSION || host.reserved != 0 {
        return core::ptr::null();
    }
    let Ok(multiplier) = configured_multiplier(host) else {
        return core::ptr::null();
    };
    let patches = if multiplier == 1.0 {
        vec![]
    } else {
        let Ok((_, build)) = selected_build(host) else {
            return core::ptr::null();
        };
        vec![defiance_feature_sdk::contract::Patch {
            module: c"logic.dll",
            rva: build.update,
            kind: PATCH_KIND_ENTRY,
            before: ENTRY.to_vec(),
            after: None,
        }]
    };
    defiance_feature_sdk::contract::build(api, patches)
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else {
        return 1;
    };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    match install(api) {
        Ok(message) => {
            log(api, LOG_INFO, &message);
            0
        }
        Err(error) => {
            log(api, LOG_ERROR, &format!("vehicle arrival refused: {error}"));
            1
        }
    }
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.vehicle-arrival".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();

#[cfg(feature = "parity-test")]
#[no_mangle]
pub unsafe extern "C" fn vehicle_arrival_test_validate(
    image: *const u8,
    size: usize,
    build: usize,
) -> i32 {
    let Some(build) = sites::BUILDS.get(build) else {
        return 1;
    };
    if image.is_null() {
        return 1;
    }
    i32::from(validate(core::slice::from_raw_parts(image, size), build).is_err())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_validation_rejects_out_of_bounds_and_modified_code() {
        let build = &sites::BUILDS[0];
        assert_eq!(validate(&[], build), Err("callback out of bounds"));
        let image = vec![0; build.update + METHOD_BYTES];
        assert!(validate(&image, build).is_err());
        let overflow = Build {
            update: usize::MAX,
            ..*build
        };
        assert_eq!(validate(&[], &overflow), Err("callback out of bounds"));
    }
}
