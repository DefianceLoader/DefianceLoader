//! One ability button for several abilities, with a row to pick among them.
//!
//! The ability bar has four buttons (F1–F4). Several abilities share each
//! one, and the game shows one ability per button: when the selected squads
//! have two abilities of a button, it empties the button until the selection
//! changes. This plugin keeps the button: it shows one
//! of the abilities, and pressing it (key or mouse) opens a row of all of them
//! over the order panel, the way the mine button opens its mine types. An
//! ability in the row is picked with the order key of the cell under it or
//! with the mouse; the button then shows that ability. [`native`] has the
//! hooks and the state.
//!
//! Hot reloadable without a `stop`: the loader reloads only outside a
//! mission, where no ability bar exists, so
//! no row has the game's widgets moved.
use defiance_api::{
    Api, PatchContractV1, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO, LOG_WARN, PATCH_KIND_ENTRY,
};
use defiance_core::sites::Image;

mod native;
mod sites;

unsafe fn log(api: &Api, level: u32, text: &str) {
    if let Ok(text) = std::ffi::CString::new(text) {
        (api.log)(level, text.as_ptr());
    }
}

/// The loaded game.dll and its resolved sites.
unsafe fn resolve(api: &Api) -> Result<(Image<'static>, sites::Sites), String> {
    let base = (api.module_base)(c"game.dll".as_ptr());
    if base.is_null() {
        return Err("game.dll is not loaded".into());
    }
    let image = Image::loaded(base as *const u8, (api.module_size)(base));
    let sites = sites::sites(&image)?;
    Ok((image, sites))
}

unsafe extern "C" fn patch_contract(api: *const Api) -> *const PatchContractV1 {
    let Some(api_ref) = (unsafe { api.as_ref() }) else {
        return core::ptr::null();
    };
    let Ok((image, sites)) = (unsafe { resolve(api_ref) }) else {
        return core::ptr::null();
    };
    let patches = sites::HOOKS
        .into_iter()
        .map(|(index, span)| {
            let rva = sites.rvas[index];
            defiance_feature_sdk::contract::Patch {
                module: c"game.dll",
                rva,
                kind: PATCH_KIND_ENTRY,
                before: image.image[rva..rva + span].to_vec(),
                after: None,
            }
        })
        .collect();
    unsafe { defiance_feature_sdk::contract::build(api, patches) }
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else {
        return 1;
    };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    let (image, sites) = match resolve(api) {
        Ok(resolved) => resolved,
        Err(error) => {
            log(
                api,
                LOG_WARN,
                &format!("ability groups: not a supported build ({error}); no writes made"),
            );
            return 1;
        }
    };
    match native::install(api, image.base, &sites) {
        Ok(()) => {
            log(api, LOG_INFO, "ability groups installed");
            0
        }
        Err(error) => {
            log(api, LOG_ERROR, &format!("ability groups refused: {error}"));
            1
        }
    }
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.ability-groups".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

#[no_mangle]
pub unsafe extern "C" fn defiance_patch_contract_v1(api: *const Api) -> *const PatchContractV1 {
    unsafe { patch_contract(api) }
}
defiance_feature_sdk::crash_handshake!();
