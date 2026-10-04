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
use core::ffi::c_void;
use defiance_api::{
    Api, PatchContractV1, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO, PATCH_KIND_ENTRY,
};

mod native;
mod sites;

pub struct Site {
    rva: usize,
    before: &'static [u8],
}

/// A supported `game.dll`, from `tools/ability_groups_bindings.py`. Each
/// function is described in [`native`], where it is called or hooked.
pub struct Build {
    name: &'static str,
    sha: &'static str,
    update: Site,
    reset: Site,
    key: Site,
    bar_destroy: Site,
    order_key: Site,
    click: Site,
    hide: Site,
    move_widget: Site,
    label: Site,
    close_submenus: Site,
    clear_order: Site,
    order_reset: Site,
    hide_orders: Site,
    /// The game menu's byte that keeps the order panel hidden while a submenu
    /// is open.
    orders_hidden: usize,
    /// The game menu's submenu state: 3 while one is open, else 0.
    submenu: usize,
}

impl Build {
    fn sites(&self) -> [&Site; 13] {
        [
            &self.update,
            &self.reset,
            &self.key,
            &self.bar_destroy,
            &self.order_key,
            &self.click,
            &self.hide,
            &self.move_widget,
            &self.label,
            &self.close_submenus,
            &self.clear_order,
            &self.order_reset,
            &self.hide_orders,
        ]
    }
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

unsafe fn selected_build(api: &Api) -> Result<(*mut c_void, usize, &'static Build), String> {
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
    let sha = defiance_core::sha256::file(&path).map_err(|e| e.to_string())?;
    let build = sites::BUILDS
        .iter()
        .find(|b| b.sha == sha)
        .ok_or("unsupported game.dll; no writes made")?;
    for site in build.sites() {
        if site
            .rva
            .checked_add(site.before.len())
            .is_none_or(|end| end > size)
            || core::slice::from_raw_parts(base.cast::<u8>().add(site.rva), site.before.len())
                != site.before
        {
            return Err(format!(
                "native code differs at {:#x}; no writes made",
                site.rva
            ));
        }
    }
    Ok((base, size, build))
}

unsafe fn install(api: &Api) -> Result<&'static Build, String> {
    let (base, _, build) = unsafe { selected_build(api) }?;
    native::install(api, base as usize, build)?;
    Ok(build)
}

unsafe extern "C" fn patch_contract(api: *const Api) -> *const PatchContractV1 {
    let Some(api_ref) = (unsafe { api.as_ref() }) else {
        return core::ptr::null();
    };
    let (_, _, build) = match unsafe { selected_build(api_ref) } {
        Ok(selected) => selected,
        Err(_) => return core::ptr::null(),
    };
    let patches = [
        &build.update,
        &build.reset,
        &build.key,
        &build.bar_destroy,
        &build.order_key,
        &build.click,
    ]
    .into_iter()
    .map(|site| defiance_feature_sdk::contract::Patch {
        module: c"game.dll",
        rva: site.rva,
        kind: PATCH_KIND_ENTRY,
        before: site.before.to_vec(),
        after: None,
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
    match install(api) {
        Ok(build) => {
            log(
                api,
                LOG_INFO,
                &format!("ability groups installed ({})", build.name),
            );
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

#[cfg(test)]
mod tests {
    #[test]
    fn hooked_prologues_need_no_relocation() {
        for b in super::sites::BUILDS {
            for site in [
                &b.update,
                &b.reset,
                &b.key,
                &b.bar_destroy,
                &b.order_key,
                &b.click,
            ] {
                assert!(site.before.len() >= 14);
                defiance_core::decode::validate_copy(site.before).unwrap();
            }
        }
    }
}
