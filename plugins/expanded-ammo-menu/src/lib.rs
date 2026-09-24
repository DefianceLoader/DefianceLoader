//! Startup-only, opt-in expansion of the stock three-row ammunition menu.
//! All layout-dependent operands are validated before any write. Never hot unload.
use core::ffi::c_void;
use defiance_api::{Api, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO};
mod combined;
mod sites;
use std::sync::atomic::{AtomicUsize, Ordering};
static REDRAW: AtomicUsize = AtomicUsize::new(0);
static LAYOUT: AtomicUsize = AtomicUsize::new(0);
static ALL_SELECTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
type Pair = unsafe extern "C" fn(usize, usize);
unsafe fn read(at: usize) -> usize {
    *(at as *const usize)
}

/// Slot +0xc is a presentation index, not the ammunition index. The native
/// click handler derives the ammo index from the slot's address in the array.
/// Move the root by a delta once; native movement propagates to its children.
unsafe fn compact(menu: usize, count: usize, translate: Pair) {
    let mut display = 0i32;
    for index in 0..count {
        let slot = menu + 0x180 + index * 0xb8;
        let root = read(slot + 0x18);
        if root == 0 || *((root + 0x5b) as *const u8) == 0 {
            continue;
        }
        let position = (slot + 0xc) as *mut i32;
        let previous = *position;
        if previous != display && (0..count as i32).contains(&previous) {
            let rect = std::mem::transmute::<usize, unsafe extern "C" fn(usize) -> *const i32>(
                read(read(root) + 0x40),
            )(root);
            let width = (*rect.add(2) - *rect).saturating_add(2);
            let height = (*rect.add(3) - *rect.add(1)).saturating_add(2);
            let delta = [
                (display / 3 - previous / 3).saturating_mul(width),
                (previous % 3 - display % 3).saturating_mul(height),
            ];
            translate(root, delta.as_ptr() as usize);
            *position = display;
        }
        display += 1;
    }
}
unsafe extern "C" fn redraw(menu: usize, entity: usize) {
    std::mem::transmute::<usize, Pair>(REDRAW.load(Ordering::Relaxed))(menu, entity);
    if ALL_SELECTED.load(Ordering::Relaxed) {
        combined::render(menu, COUNT.load(Ordering::Relaxed));
    }
    compact(
        menu,
        COUNT.load(Ordering::Relaxed),
        std::mem::transmute::<usize, Pair>(LAYOUT.load(Ordering::Relaxed)),
    );
}

#[derive(Clone, Copy)]
enum Kind {
    Count,
    Extent,
    Shift,
    Zero,
}
struct Site {
    rva: usize,
    before: &'static [u8],
    field: usize,
    width: usize,
    kind: Kind,
}
/// The class offsets `combined.rs` reads, which move between builds while the
/// patch source stays one. The reference build's values are the defaults.
#[derive(Clone, Copy)]
pub struct Offsets {
    pub roster: usize,
    pub gunner_count: usize,
    pub gunner_get: usize,
    pub pool_get: usize,
    pub world_player: usize,
    /// The AI's "set this ammo type" virtual method, used by the union click.
    pub ai_set: usize,
}

struct Build {
    sha: &'static str,
    sites: &'static [Site],
    redraw: usize,
    redraw_before: &'static [u8],
    layout: usize,
    layout_before: &'static [u8],
    combined: &'static [(usize, &'static [u8])],
    offsets: Offsets,
}

fn slots(columns: i64) -> Result<usize, &'static str> {
    if !(3..=42).contains(&columns) {
        return Err("columns must be between 3 and 42 (9 to 126 slots)");
    }
    Ok(columns as usize * 3)
}
fn replacement(site: &Site, count: usize) -> Vec<u8> {
    let old = site.before[site.field..site.field + site.width]
        .iter()
        .enumerate()
        .fold(0usize, |sum, (i, b)| sum | ((*b as usize) << (8 * i)));
    let value = match site.kind {
        Kind::Count => count,
        Kind::Extent => count * 0xb8,
        Kind::Shift => old + (count - 9) * 0xb8,
        Kind::Zero => 0,
    };
    let mut result = site.before.to_vec();
    result[site.field..site.field + site.width].copy_from_slice(&value.to_le_bytes()[..site.width]);
    result
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
unsafe fn install(api: &Api) -> Result<(), String> {
    let columns = defiance_feature_sdk::integer(api, "defiance.expanded-ammo-menu", "columns")
        .map_err(|e| e.to_string())?;
    let count = slots(columns)?;
    let all_selected = match defiance_feature_sdk::boolean(
        api,
        "defiance.expanded-ammo-menu",
        "all_selected_squads",
    ) {
        Ok(value) => value,
        Err(defiance_feature_sdk::ConfigError::Unavailable) => false,
        Err(error) => return Err(error.to_string()),
    };
    let menu = defiance_feature_sdk::services::ammo_menu()
        .ok_or("Core ammo-menu service v1 unavailable; update Core with this plugin")?;
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
        .ok_or("unsupported game.dll build; no menu writes made")?;
    combined::set_offsets(build.offsets);
    // In particular do not edit the nine-slot loop before storage, destruction,
    // click handling, and exception cleanup have all been validated together.
    for site in build.sites {
        if site
            .rva
            .checked_add(site.before.len())
            .is_none_or(|end| end > size)
        {
            return Err("menu patch falls outside game.dll".into());
        }
        if core::slice::from_raw_parts(base.cast::<u8>().add(site.rva), site.before.len())
            != site.before
        {
            return Err(format!(
                "menu bytes differ at game.dll+{:#x}; no menu writes made",
                site.rva
            ));
        }
    }
    for (rva, before) in [
        (build.redraw, build.redraw_before),
        (build.layout, build.layout_before),
    ] {
        if rva.checked_add(before.len()).is_none_or(|end| end > size)
            || core::slice::from_raw_parts(base.cast::<u8>().add(rva), before.len()) != before
        {
            return Err("menu layout hook bytes differ; no writes made".into());
        }
    }
    if all_selected {
        for &(rva, before) in build.combined {
            if rva.checked_add(before.len()).is_none_or(|end| end > size)
                || core::slice::from_raw_parts(base.cast::<u8>().add(rva), before.len()) != before
            {
                return Err("combined menu bytes differ; no writes made".into());
            }
        }
        combined::configure(base as usize, build.combined)?;
    }
    let mut applied = Vec::new();
    for site in build.sites {
        let after = replacement(site, count);
        if after == site.before {
            continue;
        }
        let address = base.cast::<u8>().add(site.rva).cast();
        if (api.patch_bytes)(address, site.before.as_ptr(), after.as_ptr(), after.len()) != 0 {
            // Reverse order here, then let host rollback retry anything that
            // could not be restored. Never mark a partial expansion active.
            for address in applied.into_iter().rev() {
                (api.unhook)(address);
            }
            return Err("menu patch failed; host will complete owned rollback".into());
        }
        applied.push(address);
    }
    COUNT.store(count, Ordering::Relaxed);
    LAYOUT.store(base as usize + build.layout, Ordering::Relaxed);
    let address = base.cast::<u8>().add(build.redraw).cast();
    let mut trampoline = std::ptr::null_mut();
    if (api.hook_exact)(
        address,
        redraw as *mut _,
        build.redraw_before.len(),
        &mut trampoline,
    ) != 0
    {
        for address in applied.into_iter().rev() {
            (api.unhook)(address);
        }
        return Err("menu redraw hook failed; patches rolled back".into());
    }
    REDRAW.store(trampoline as usize, Ordering::Relaxed);
    applied.push(address);
    if all_selected {
        let (rva, before) = build.combined[0];
        let address = base.cast::<u8>().add(rva).cast();
        let mut trampoline = std::ptr::null_mut();
        if (api.hook_exact)(
            address,
            combined::click as *mut _,
            before.len(),
            &mut trampoline,
        ) != 0
        {
            for address in applied.into_iter().rev() {
                (api.unhook)(address);
            }
            return Err("combined menu click hook failed; patches rolled back".into());
        }
        combined::CLICK.store(trampoline as usize, Ordering::Relaxed);
        applied.push(address);
    }
    ALL_SELECTED.store(all_selected, Ordering::Relaxed);
    log(api,LOG_INFO,&format!("expanded ammo menu: 3 rows x {columns} columns ({count} slots); compact visible slots v2 (relative root movement); all_selected_squads={all_selected}; menu context fix v3; combined counts/reload share v2, vehicles; startup-only, no hot unload"));
    // Nothing fallible may follow successful publication. On refusal the host
    // rolls back this plugin's owned patches; capacity remains unchanged.
    if (menu.publish)(count as u32) != 0 {
        for address in applied.into_iter().rev() {
            (api.unhook)(address);
        }
        return Err("ammo-menu capacity publication refused; patches rolled back".into());
    }
    Ok(())
}
unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else {
        return 1;
    };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    match install(api) {
        Ok(()) => 0,
        Err(error) => {
            log(
                api,
                LOG_ERROR,
                &format!("expanded ammo menu refused: {error}"),
            );
            1
        }
    }
}
#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.expanded-ammo-menu".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}
defiance_feature_sdk::crash_handshake!();
defiance_feature_sdk::service_handshake!();

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redraw_hook_spans_are_copyable() {
        for build in sites::BUILDS {
            defiance_core::decode::validate_copy(build.combined[0].1).unwrap();
            assert!(build.redraw_before.len() >= 14);
            defiance_core::decode::validate_copy(build.redraw_before).unwrap();
        }
    }
    #[test]
    fn bounds_preserve_signed_byte_loop_operands() {
        assert!(slots(2).is_err());
        assert!(slots(43).is_err());
        assert_eq!(slots(3), Ok(9));
        assert_eq!(slots(12), Ok(36));
        assert_eq!(slots(42), Ok(126));
    }
    #[test]
    fn every_generated_patch_preserves_instruction_size_and_surrounding_bytes() {
        for build in sites::BUILDS {
            for site in build.sites {
                for count in [9, 12, 36, 126] {
                    let after = replacement(site, count);
                    assert_eq!(after.len(), site.before.len());
                    assert_eq!(&after[..site.field], &site.before[..site.field]);
                    assert_eq!(
                        &after[site.field + site.width..],
                        &site.before[site.field + site.width..]
                    );
                    if count == 9 && !matches!(site.kind, Kind::Zero) {
                        assert_eq!(after, site.before);
                    }
                }
            }
        }
    }
}
