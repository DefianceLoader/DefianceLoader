//! Complete a weapon model handoff when a frame skips its timer window.
use core::ffi::{c_char, c_void};
use defiance_api::{Api, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO, LOG_WARN};
use defiance_core::sites::Image;
use std::sync::atomic::{AtomicUsize, Ordering};

mod sites;
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static LOGGER: AtomicUsize = AtomicUsize::new(0);
static REPORTS: AtomicUsize = AtomicUsize::new(0);
type Step = unsafe extern "C" fn(*mut u8, f32) -> bool;
type Log = unsafe extern "C" fn(u32, *const c_char);

unsafe fn read<T: Copy>(base: *const u8, offset: usize) -> T {
    base.add(offset).cast::<T>().read_unaligned()
}

unsafe fn message(level: u32, value: &str) {
    let logger = LOGGER.load(Ordering::Acquire);
    if logger != 0 {
        if let Ok(text) = std::ffi::CString::new(value) {
            let log: Log = core::mem::transmute(logger);
            log(level, text.as_ptr());
        }
    }
}

struct Handoff {
    selected: *const u8,
    visual: *const u8,
    animation: *const u8,
    available: bool,
}

unsafe fn handoff(gunner: *mut u8) -> Option<Handoff> {
    let index = read::<i32>(gunner, 0x94);
    let weapons: *const u8 = read(gunner, 0x38);
    let end = read::<usize>(gunner, 0x40);
    if index < 0 || weapons.is_null() || end < weapons as usize {
        return None;
    }
    let count = (end - weapons as usize) / 8;
    if index as usize >= count || count > 256 {
        return None;
    }
    let weapon: *mut u8 = read(weapons, index as usize * 8);
    if weapon.is_null() {
        return None;
    }
    let weapon_vtable: *const u8 = read(weapon, 0);
    if weapon_vtable.is_null() {
        return None;
    }
    let address = read::<usize>(weapon_vtable, 0x160);
    if address == 0 {
        return None;
    }
    let descriptor: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(address);
    let selected = descriptor(weapon);
    if selected.is_null() || read::<usize>(selected, 0x1b8) == 0 {
        return None;
    }
    let unit: *mut u8 = read(gunner, 0x20);
    if unit.is_null() {
        return None;
    }
    let unit_vtable: *const u8 = read(unit, 0);
    if unit_vtable.is_null() {
        return None;
    }
    let address = read::<usize>(unit_vtable, 0xb0);
    if address == 0 {
        return None;
    }
    let getter: unsafe extern "C" fn(*mut u8) -> *const u8 = core::mem::transmute(address);
    let components = getter(unit);
    if components.is_null() {
        return None;
    }
    let visual: *const u8 = read(components, 8);
    let animation: *const u8 = read(components, 0x58);
    if visual.is_null() || animation.is_null() {
        return None;
    }
    let models: *const u8 = read(visual, 0x108);
    let end = read::<usize>(visual, 0x110);
    if models.is_null() || end < models as usize {
        return None;
    }
    let bytes = end - models as usize;
    if bytes == 0 || !bytes.is_multiple_of(0xb0) || bytes / 0xb0 > 256 {
        return None;
    }
    let available = (0..bytes / 0xb0).any(|i| read::<*const u8>(models, i * 0xb0) == selected);
    Some(Handoff {
        selected,
        visual: read(models, 0),
        animation,
        available,
    })
}

unsafe extern "C" fn step(gunner: *mut u8, dt: f32) -> bool {
    let address = loop {
        let address = ORIGINAL.load(Ordering::Acquire);
        if address != 0 {
            break address;
        }
        core::hint::spin_loop();
    };
    let original: Step = core::mem::transmute(address);
    let done = original(gunner, dt);
    if !done {
        return false;
    }
    let Some(before) = handoff(gunner) else {
        return done;
    };
    let timer = read::<f32>(gunner, 0x9c);
    let mut repaired = false;
    if before.selected != before.visual && before.available && timer.is_finite() && timer <= 0.0 {
        // Re-enter the stock handoff branch without advancing the switch.
        // 1.0 is inside its (0, 1.5) second window. Restore the completed
        // timer and return value; gunner state and firing tick run only once.
        gunner.add(0x9c).cast::<f32>().write_unaligned(1.0);
        let _ = original(gunner, 0.0);
        gunner.add(0x9c).cast::<f32>().write_unaligned(timer);
        repaired = handoff(gunner).is_some_and(|after| after.selected == after.visual);
    }
    if REPORTS.fetch_add(1, Ordering::Relaxed) < 150 {
        let after = handoff(gunner);
        message(LOG_INFO, &format!(
            "moving weapon sync: gunner={gunner:p} dt={dt:.5} timer={timer:.3} selected={:p} displayed={:p}->{:#x} available={} repaired={repaired} action={:#x} requested={:#x}",
            before.selected, before.visual, after.map_or(0, |state| state.visual as usize),
            before.available, read::<i32>(before.animation, 0x6c), read::<i32>(before.animation, 0x68)
        ));
    }
    done
}

enum InstallError {
    /// A site did not resolve: this build is not one the plugin supports.
    UnsupportedBuild(String),
    Failed(String),
}

unsafe fn install(api: &Api) -> Result<(), InstallError> {
    let base = (api.module_base)(c"logic.dll".as_ptr());
    if base.is_null() {
        return Err(InstallError::Failed("logic.dll missing".into()));
    }
    let size = (api.module_size)(base);
    let bytes = base.cast::<u8>();
    let step_rva =
        sites::step(&Image::loaded(bytes, size)).map_err(InstallError::UnsupportedBuild)?;
    REPORTS.store(0, Ordering::Relaxed);
    let mut original = core::ptr::null_mut();
    if (api.hook)(
        bytes.add(step_rva).cast(),
        step as Step as *mut c_void,
        &mut original,
    ) != 0
        || original.is_null()
    {
        return Err(InstallError::Failed(
            "weapon-change helper hook refused".into(),
        ));
    }
    ORIGINAL.store(original as usize, Ordering::Release);
    message(LOG_INFO, "moving weapon sync installed: completed switches check the displayed weapon; missed handoffs use the stock zero-time handoff branch");
    Ok(())
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else { return 1 };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    LOGGER.store(api.log as usize, Ordering::Release);
    match install(api) {
        Ok(()) => 0,
        Err(InstallError::UnsupportedBuild(reason)) => {
            message(
                LOG_WARN,
                &format!("moving weapon sync: not a supported build ({reason}); no writes made"),
            );
            0
        }
        Err(InstallError::Failed(error)) => {
            message(LOG_ERROR, &format!("moving weapon sync refused: {error}"));
            1
        }
    }
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.moving-actions-sync".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();
