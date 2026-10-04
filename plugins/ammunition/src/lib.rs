// Ammunition feature: its logic.dll unit (patch/ammo-mode.asm) and game.dll unit
// (patch/ammo-panel.asm), which Core's patch service installs for the build.
use core::ffi::c_void;
use defiance_api::{AmmoStepHandlerV1, AmmoStepV1, Api, Plugin, ABI_VERSION};
use defiance_feature_sdk::units::Embedded;
use std::sync::atomic::{AtomicUsize, Ordering};
defiance_feature_sdk::service_handshake!();

/// Every supported build's units (`build.rs`).
static UNITS: &[Embedded] = include!(concat!(env!("OUT_DIR"), "/units.rs"));
defiance_feature_sdk::export_unit_patch_contract!(UNITS, &[], false);
static STEP_HANDLER: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn set_step_handler(handler: Option<AmmoStepHandlerV1>) {
    STEP_HANDLER.store(
        handler.map_or(0, |handler| handler as *const () as usize),
        Ordering::Release,
    );
}

static STEP_API: AmmoStepV1 = AmmoStepV1 {
    set_handler: set_step_handler,
};

unsafe extern "C" fn step_dispatch(menu: *mut c_void, widget: *mut c_void, direction: i32) -> i32 {
    let handler = STEP_HANDLER.load(Ordering::Acquire);
    if handler == 0 {
        return 0;
    }
    let handler = std::mem::transmute::<usize, AmmoStepHandlerV1>(handler);
    unsafe { handler(menu, widget, direction) }
}

/// A setting of this plugin, lower case; empty when it has none.
fn setting(api: *const Api, key: &str) -> String {
    unsafe { defiance_feature_sdk::raw(&*api, "defiance.ammunition", key) }
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// The virtual key for `step_modifier`: Ctrl by default, Shift, Alt, or none.
fn step_modifier_key(setting: &str) -> u32 {
    match setting {
        "shift" => 0x10,
        "alt" => 0x12,
        "none" => 0,
        _ => 0x11,
    }
}

/// Fill the step cell the game unit's wheel and click hooks read, before they
/// go live: the modifier's virtual key (+0), whether clicks step (+4), and the
/// hooks' own state, which starts at zero: the wheel's unspent delta (+8), the
/// camera wheel's last message (+10..+30), and the combined-step bridge (+30).
fn fill_cells(api: *const Api, cell: &dyn Fn(&core::ffi::CStr) -> Option<usize>) {
    if let Some(address) = cell(c"step") {
        let key = step_modifier_key(&setting(api, "step_modifier"));
        let click =
            unsafe { defiance_feature_sdk::boolean(&*api, "defiance.ammunition", "step_click") };
        let click = u32::from(click.unwrap_or(false));
        let cell = address as *mut u32;
        unsafe {
            cell.write_volatile(key);
            cell.add(1).write_volatile(click);
            for word in 2..14 {
                cell.add(word).write_volatile(0);
            }
            core::ptr::write_volatile(
                (address + 0x30) as *mut usize,
                step_dispatch as *const () as usize,
            );
        }
    }
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    if api.is_null() {
        return 1;
    }
    if defiance_feature_sdk::services::register(c"combined-step", 1, &STEP_API).is_err() {
        return 1;
    }
    unsafe {
        defiance_feature_sdk::units::install(
            api,
            UNITS,
            |cell| fill_cells(api, cell),
            &[],
            core::ptr::null_mut(),
        )
    }
}
#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: b"defiance.ammunition\0".as_ptr().cast(),
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
    fn the_step_modifier_is_ctrl_unless_shift_alt_or_none() {
        assert_eq!(step_modifier_key("shift"), 0x10);
        assert_eq!(step_modifier_key("alt"), 0x12);
        assert_eq!(step_modifier_key("none"), 0);
        assert_eq!(step_modifier_key(""), 0x11);
        assert_eq!(step_modifier_key("ctrl"), 0x11);
    }
}
