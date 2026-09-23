// Ordinary soldier methods are Rust; manager/input adapters remain assembly.
#[cfg(feature = "rust-selection")]
mod native;
use defiance_api::{Api, Plugin, ABI_VERSION};
defiance_feature_sdk::service_handshake!();

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

unsafe extern "C" fn init(api: *const Api) -> i32 {
    #[cfg(feature = "rust-selection")]
    let result = {
        let Some(game) = (unsafe { defiance_feature_sdk::services::game_access() }) else {
            return 1;
        };
        if native::GAME.set(game).is_err() {
            return 1;
        }
        let replacements = [
            defiance_api::NativeReplacementV1 {
                name: c"setter".as_ptr(),
                detour: native::set as *mut _,
            },
            defiance_api::NativeReplacementV1 {
                name: c"is_selected".as_ptr(),
                detour: native::get as *mut _,
            },
        ];
        unsafe { defiance_feature_sdk::install_native(api, 2, &replacements) }
    };
    #[cfg(not(feature = "rust-selection"))]
    let result = unsafe { defiance_feature_sdk::install(api, 2) };
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
