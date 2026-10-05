//! Configurable destination slowdown for wheeled and tracked vehicles.
use core::ffi::c_void;
use defiance_api::{
    Api, PatchContractV1, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO, LOG_WARN, PATCH_KIND_ENTRY,
};
use defiance_core::sites::Image;
use std::sync::atomic::Ordering;

mod native;
mod sites;

const ID: &str = "defiance.vehicle-arrival";
enum InstallError {
    /// The callback did not resolve: this build is not one the plugin supports.
    UnsupportedBuild(String),
    Failed(String),
}

unsafe fn log(api: &Api, level: u32, text: &str) {
    if let Ok(text) = std::ffi::CString::new(text) {
        (api.log)(level, text.as_ptr());
    }
}

/// logic.dll's base and the callback's rva in it.
unsafe fn resolve(api: &Api) -> Result<(*mut u8, usize), InstallError> {
    let base = (api.module_base)(c"logic.dll".as_ptr()).cast::<u8>();
    if base.is_null() {
        return Err(InstallError::Failed("logic.dll is not loaded".into()));
    }
    let image = Image::loaded(base, (api.module_size)(base.cast()));
    let update = sites::update(&image).map_err(InstallError::UnsupportedBuild)?;
    Ok((base, update))
}

unsafe fn configured_multiplier(api: &Api) -> Result<f32, String> {
    let percent = defiance_feature_sdk::integer(api, ID, "braking_window_percent")
        .map_err(|error| error.to_string())?;
    native::multiplier(percent).map_err(str::to_owned)
}

unsafe fn install(api: &Api) -> Result<String, InstallError> {
    let multiplier = configured_multiplier(api).map_err(InstallError::Failed)?;
    if multiplier == 1.0 {
        return Ok("vehicle arrival: stock braking window; no hook installed".into());
    }
    let (base, update) = resolve(api)?;
    native::MULTIPLIER.store(multiplier.to_bits(), Ordering::Relaxed);
    if (api.hook_exact)(
        base.add(update).cast(),
        native::vehicle_arrival_update as *mut c_void,
        sites::ENTRY_BEFORE.len(),
        native::ORIGINAL.as_ptr(),
    ) != 0
    {
        return Err(InstallError::Failed(
            "vehicle arrival hook refused; host will finish owned rollback".into(),
        ));
    }
    Ok(format!(
        "vehicle arrival installed (logic+{update:#x}): {:.0}% braking window",
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
        match resolve(host) {
            Ok((_, update)) => vec![defiance_feature_sdk::contract::Patch {
                module: c"logic.dll",
                rva: update,
                kind: PATCH_KIND_ENTRY,
                before: sites::ENTRY_BEFORE.to_vec(),
                after: None,
            }],
            // An unsupported build installs nothing, so claims nothing.
            Err(InstallError::UnsupportedBuild(_)) => vec![],
            Err(InstallError::Failed(_)) => return core::ptr::null(),
        }
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
        Err(InstallError::UnsupportedBuild(reason)) => {
            log(
                api,
                LOG_WARN,
                &format!("vehicle arrival: not a supported build ({reason}); no writes made"),
            );
            0
        }
        Err(InstallError::Failed(error)) => {
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

/// The callback's rva in a logic.dll image laid out as mapped at `base`, or
/// -1 if it does not resolve.
#[cfg(feature = "parity-test")]
#[no_mangle]
pub unsafe extern "C" fn vehicle_arrival_test_resolve(
    image: *const u8,
    size: usize,
    base: usize,
) -> i64 {
    if image.is_null() {
        return -1;
    }
    let image = Image {
        image: core::slice::from_raw_parts(image, size),
        base,
    };
    sites::update(&image).map_or(-1, |rva| rva as i64)
}
