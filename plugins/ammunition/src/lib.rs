// Ammunition feature: its logic.dll unit (patch/ammo-mode.asm) and game.dll unit
// (patch/ammo-panel.asm), which Core's patch service installs for the build.
use defiance_api::{Api, Plugin, ABI_VERSION};
use defiance_feature_sdk::units::Embedded;
defiance_feature_sdk::service_handshake!();

/// Every supported build's units (`build.rs`).
static UNITS: &[Embedded] = include!(concat!(env!("OUT_DIR"), "/units.rs"));

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
/// hooks' own state, which starts at zero: the wheel's unspent delta (+8) and
/// the camera wheel's last message (+10..+30).
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
            for word in 2..12 {
                cell.add(word).write_volatile(0);
            }
        }
    }
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    if api.is_null() {
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
