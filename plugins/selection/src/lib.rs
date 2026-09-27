// Ordinary soldier methods are Rust; manager/input adapters remain assembly:
// this plugin's logic.dll and game.dll units, which Core's patch service
// installs for the running build.
mod marquee;
#[cfg(feature = "rust-selection")]
mod native;
mod preview;
use defiance_api::{Api, Plugin, ABI_VERSION};
use defiance_feature_sdk::units::Embedded;
defiance_feature_sdk::service_handshake!();

/// Every supported build's units (`build.rs`).
static UNITS: &[Embedded] = include!(concat!(env!("OUT_DIR"), "/units.rs"));

/// Call the verified build's selectable virtual getter. The provider does not
/// retain the facet. Null is allowed; every other pointer must be live.
unsafe extern "C" fn is_selected(facet: *mut core::ffi::c_void) -> u8 {
    if facet.is_null() {
        return 0;
    }
    let vtable = unsafe { *facet.cast::<*const usize>() };
    let call: unsafe extern "C" fn(*mut core::ffi::c_void) -> u8 =
        unsafe { core::mem::transmute(*vtable.add(0x58 / 8)) };
    u8::from(unsafe { call(facet) } != 0)
}
static SELECTION: defiance_api::SelectionV1 = defiance_api::SelectionV1 { is_selected };

/// A setting of this plugin, lower case; empty when it has none.
fn setting(api: *const Api, key: &str) -> String {
    unsafe { defiance_feature_sdk::raw(&*api, "defiance.selection", key) }
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// The virtual key for `squad_tab_modifier`: Ctrl by default, Shift, or none.
fn tab_modifier_key(setting: &str) -> u32 {
    match setting {
        "shift" => 0x10,
        "off" => 0,
        _ => 0x11,
    }
}

/// Fill the cells the units' hooks read, before they go live: the squad
/// preview's material callback, the marquee's key test and mode, and the
/// squad TAB modifier's key (0 leaves only the plain TAB cycle).
fn fill_cells(api: *const Api, cell: &dyn Fn(&core::ffi::CStr) -> Option<usize>) {
    if let Some(address) = cell(c"preview_dim") {
        unsafe {
            (address as *mut usize).write_volatile(
                preview::dim_material as unsafe extern "C" fn(usize, usize) -> usize as usize,
            )
        };
    }
    if let Some(address) = cell(c"marquee") {
        unsafe { marquee::write(address, &setting(api, "marquee")) };
    }
    if let Some(address) = cell(c"tab_modifier") {
        let key = tab_modifier_key(&setting(api, "squad_tab_modifier"));
        unsafe { (address as *mut u32).write_volatile(key) };
    }
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    if api.is_null() {
        return 1;
    }
    preview::set_logger(unsafe { (*api).log });
    #[cfg(feature = "rust-selection")]
    let replacements = {
        let Some(game) = (unsafe { defiance_feature_sdk::services::game_access() }) else {
            return 1;
        };
        if native::GAME.set(game).is_err() {
            return 1;
        }
        [
            defiance_api::NativeReplacementV1 {
                name: c"setter".as_ptr(),
                detour: native::set as *mut _,
            },
            defiance_api::NativeReplacementV1 {
                name: c"is_selected".as_ptr(),
                detour: native::get as *mut _,
            },
        ]
    };
    #[cfg(not(feature = "rust-selection"))]
    let replacements: [defiance_api::NativeReplacementV1; 0] = [];
    let result = unsafe {
        defiance_feature_sdk::units::install(
            api,
            UNITS,
            |cell| fill_cells(api, cell),
            &replacements,
            core::ptr::null_mut(),
        )
    };
    if result != 0 {
        return result;
    }
    // Preserve compatibility with older ABI-5 loaders. Consumers requiring
    // selection services will refuse their own init if the extension is absent.
    if defiance_feature_sdk::services::available()
        && unsafe { defiance_feature_sdk::services::register(c"selection", 1, &SELECTION) }.is_err()
    {
        return 1;
    }
    0
}
#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: b"defiance.selection\0".as_ptr().cast(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tab_modifier_is_ctrl_unless_shift_or_off() {
        assert_eq!(tab_modifier_key("shift"), 0x10);
        assert_eq!(tab_modifier_key("off"), 0);
        assert_eq!(tab_modifier_key(""), 0x11);
        assert_eq!(tab_modifier_key("ctrl"), 0x11);
    }
}
