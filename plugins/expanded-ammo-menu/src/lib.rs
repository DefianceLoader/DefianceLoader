//! Startup-only, opt-in expansion of the stock three-row ammunition menu.
//! All layout-dependent operands are validated before any write. The optional
//! combined-step callback is cleared on stop; native layout patches are not hot-unloaded.
use defiance_api::{
    AmmoStepV1, Api, PatchContractV1, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO, LOG_WARN,
    PATCH_KIND_ENTRY,
};
use defiance_core::sites::{code_ranges, Image};
mod combined;
mod sites;
mod tooltip;
use std::sync::atomic::{AtomicUsize, Ordering};
static REDRAW: AtomicUsize = AtomicUsize::new(0);
static LAYOUT: AtomicUsize = AtomicUsize::new(0);
static ALL_SELECTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
static STEP_SERVICE: AtomicUsize = AtomicUsize::new(0);
static TOOLTIP: AtomicUsize = AtomicUsize::new(0);
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
    tooltip::place(
        menu,
        COUNT.load(Ordering::Relaxed),
        TOOLTIP.load(Ordering::Relaxed),
        std::mem::transmute::<usize, Pair>(LAYOUT.load(Ordering::Relaxed)),
    );
}

/// The class offsets the menu helpers read, which move between builds while the
/// patch source stays one. The reference build's values are the defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offsets {
    pub roster: usize,
    pub gunner_count: usize,
    pub gunner_get: usize,
    pub pool_get: usize,
    pub world_player: usize,
    /// The AI's "set this ammo type" virtual method, used by the union click.
    pub ai_set: usize,
    /// The GUI owner's live ammunition-tooltip controller.
    pub tooltip: usize,
}

fn slots(columns: i64) -> Result<usize, &'static str> {
    if !(3..=42).contains(&columns) {
        return Err("columns must be between 3 and 42 (9 to 126 slots)");
    }
    Ok(columns as usize * 3)
}
fn replacement(site: &sites::Site, count: usize) -> Vec<u8> {
    let old = site.before[site.field..site.field + site.width]
        .iter()
        .enumerate()
        .fold(0usize, |sum, (i, b)| sum | ((*b as usize) << (8 * i)));
    let value = match site.kind {
        sites::Kind::Count => count,
        sites::Kind::Extent => count * 0xb8,
        sites::Kind::Shift => old + (count - 9) * 0xb8,
        sites::Kind::Zero => 0,
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
/// game.dll's base, its size and its resolved sites. The sites are found in
/// game.dll as it was before any plugin hooked it, since another plugin may
/// hook a function the combined view only calls (unit inspection hooks
/// `fillSlot`); the bytes this plugin writes are checked live before writing.
unsafe fn resolve(api: &Api) -> Result<(*mut u8, usize, sites::Sites), String> {
    let base = (api.module_base)(c"game.dll".as_ptr()).cast::<u8>();
    if base.is_null() {
        return Err("game.dll is not loaded".into());
    }
    let size = (api.module_size)(base.cast());
    let logic = (api.module_base)(c"logic.dll".as_ptr());
    if logic.is_null() {
        return Err("logic.dll is not loaded".into());
    }
    let logic = Image::loaded(logic as *const u8, (api.module_size)(logic));
    let mut image = core::slice::from_raw_parts(base, size).to_vec();
    if let Some(original) = defiance_feature_sdk::services::original() {
        for (start, end) in code_ranges(&image) {
            let end = end.min(size);
            if start < end {
                // A failed read keeps the live bytes, which the signatures
                // then judge.
                (original.read)(
                    base as usize + start,
                    image[start..end].as_mut_ptr(),
                    end - start,
                );
            }
        }
    }
    let game = Image {
        image: &image,
        base: base as usize,
    };
    Ok((base, size, sites::sites(&game, &logic)?))
}

/// Whether the live bytes at `rva` are `before`.
unsafe fn live(base: *mut u8, size: usize, rva: usize, before: &[u8]) -> bool {
    rva.checked_add(before.len()).is_some_and(|end| end <= size)
        && core::slice::from_raw_parts(base.add(rva), before.len()) == before
}

unsafe fn install(
    api: &Api,
    (base, size, sites): (*mut u8, usize, sites::Sites),
) -> Result<(), String> {
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
    let step_service = if all_selected {
        defiance_feature_sdk::services::query::<AmmoStepV1>(
            c"defiance.ammunition",
            c"combined-step",
            1,
        )
    } else {
        None
    };
    if all_selected && step_service.is_none() {
        log(
            api,
            LOG_WARN,
            "ammunition combined-step service v1 unavailable; combined slot steps stay disabled until the ammunition plugin is updated",
        );
    }
    let menu = defiance_feature_sdk::services::ammo_menu()
        .ok_or("Core ammo-menu service v1 unavailable; update Core with this plugin")?;
    combined::set_offsets(sites.offsets);
    // In particular do not edit the nine-slot loop before storage, destruction,
    // click handling, and exception cleanup have all been validated together.
    for (site, &rva) in sites::SITES.iter().zip(&sites.patches) {
        if !live(base, size, rva, site.before) {
            return Err(format!(
                "menu bytes differ at game.dll+{rva:#x}; no menu writes made"
            ));
        }
    }
    if !live(base, size, sites.redraw, sites::REDRAW_BEFORE) {
        return Err("menu redraw hook bytes differ; no writes made".into());
    }
    if all_selected {
        // The combined view calls these functions, which [`sites::sites`]
        // checked as they were before any plugin hooked them; the one it
        // hooks is checked live as well.
        if !live(base, size, sites.combined[0], sites::COMBINED[0].1)
            || !live(base, size, sites.hover[0], sites::HOVER_BEFORE)
        {
            return Err("combined menu bytes differ; no writes made".into());
        }
        combined::configure(base as usize, &sites.combined, &sites.hover)?;
    }
    let mut applied = Vec::new();
    for (site, &rva) in sites::SITES.iter().zip(&sites.patches) {
        let after = replacement(site, count);
        if after == site.before {
            continue;
        }
        let address = base.add(rva).cast();
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
    LAYOUT.store(base as usize + sites.layout, Ordering::Relaxed);
    TOOLTIP.store(sites.offsets.tooltip, Ordering::Relaxed);
    let address = base.add(sites.redraw).cast();
    let mut trampoline = std::ptr::null_mut();
    if (api.hook_exact)(
        address,
        redraw as *mut _,
        sites::REDRAW_SPAN,
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
        let address = base.add(sites.combined[0]).cast();
        let mut trampoline = std::ptr::null_mut();
        if (api.hook_exact)(
            address,
            combined::click as *mut _,
            sites::CLICK_SPAN,
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
        let address = base.add(sites.hover[0]).cast();
        let mut trampoline = std::ptr::null_mut();
        if (api.hook_exact)(
            address,
            combined::hover as *mut _,
            sites::HOVER_SPAN,
            &mut trampoline,
        ) != 0
        {
            for address in applied.into_iter().rev() {
                (api.unhook)(address);
            }
            return Err("combined menu hover hook failed; patches rolled back".into());
        }
        combined::HOVER.store(trampoline as usize, Ordering::Relaxed);
        applied.push(address);
    }
    ALL_SELECTED.store(all_selected, Ordering::Relaxed);
    log(api,LOG_INFO,&format!("expanded ammo menu: 3 rows x {columns} columns ({count} slots); compact visible slots v2 (relative root movement); tooltip beside visible grid v1; all_selected_squads={all_selected}; menu context fix v3; combined counts/reload share v2, vehicles, hover range v1; startup-only, no hot unload"));
    // Nothing fallible may follow successful publication. On refusal the host
    // rolls back this plugin's owned patches; capacity remains unchanged.
    if (menu.publish)(count as u32) != 0 {
        for address in applied.into_iter().rev() {
            (api.unhook)(address);
        }
        return Err("ammo-menu capacity publication refused; patches rolled back".into());
    }
    if let Some(service) = step_service {
        STEP_SERVICE.store(service as *const AmmoStepV1 as usize, Ordering::Release);
        (service.set_handler)(Some(combined::step));
    }
    Ok(())
}

unsafe extern "C" fn patch_contract(api: *const Api) -> *const PatchContractV1 {
    let Some(api_ref) = (unsafe { api.as_ref() }) else {
        return core::ptr::null();
    };
    let columns = match unsafe {
        defiance_feature_sdk::integer(api_ref, "defiance.expanded-ammo-menu", "columns")
    } {
        Ok(columns) => columns,
        Err(_) => return core::ptr::null(),
    };
    let count = match slots(columns) {
        Ok(count) => count,
        Err(_) => return core::ptr::null(),
    };
    let all_selected = match unsafe {
        defiance_feature_sdk::boolean(
            api_ref,
            "defiance.expanded-ammo-menu",
            "all_selected_squads",
        )
    } {
        Ok(value) => value,
        Err(defiance_feature_sdk::ConfigError::Unavailable) => false,
        Err(_) => return core::ptr::null(),
    };
    let (_, _, sites) = match unsafe { resolve(api_ref) } {
        Ok(selected) => selected,
        Err(_) => return core::ptr::null(),
    };
    let mut patches = sites::SITES
        .iter()
        .zip(sites.patches)
        .filter_map(|(site, rva)| {
            let after = replacement(site, count);
            (after != site.before).then(|| defiance_feature_sdk::contract::Patch {
                module: c"game.dll",
                rva,
                kind: defiance_api::PATCH_KIND_BYTES,
                before: site.before.to_vec(),
                after: Some(after),
            })
        })
        .collect::<Vec<_>>();
    patches.push(defiance_feature_sdk::contract::Patch {
        module: c"game.dll",
        rva: sites.redraw,
        kind: PATCH_KIND_ENTRY,
        before: sites::REDRAW_BEFORE.to_vec(),
        after: None,
    });
    if all_selected {
        patches.push(defiance_feature_sdk::contract::Patch {
            module: c"game.dll",
            rva: sites.combined[0],
            kind: PATCH_KIND_ENTRY,
            before: sites::COMBINED[0].1.to_vec(),
            after: None,
        });
        patches.push(defiance_feature_sdk::contract::Patch {
            module: c"game.dll",
            rva: sites.hover[0],
            kind: PATCH_KIND_ENTRY,
            before: sites::HOVER_BEFORE.to_vec(),
            after: None,
        });
    }
    unsafe { defiance_feature_sdk::contract::build(api, patches) }
}
unsafe extern "C" fn stop() {
    let service = STEP_SERVICE.swap(0, Ordering::AcqRel);
    if service != 0 {
        let service = &*(service as *const AmmoStepV1);
        (service.set_handler)(None);
    }
}
unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else {
        return 1;
    };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    let resolved = match resolve(api) {
        Ok(resolved) => resolved,
        Err(error) => {
            log(
                api,
                LOG_WARN,
                &format!("expanded ammo menu: not a supported build ({error}); no writes made"),
            );
            return 1;
        }
    };
    match install(api, resolved) {
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
        stop: Some(stop),
    })
}

#[no_mangle]
pub unsafe extern "C" fn defiance_patch_contract_v1(api: *const Api) -> *const PatchContractV1 {
    unsafe { patch_contract(api) }
}
defiance_feature_sdk::crash_handshake!();
defiance_feature_sdk::service_handshake!();

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hook_spans_are_copyable() {
        defiance_core::decode::validate_copy(&sites::COMBINED[0].1[..sites::CLICK_SPAN]).unwrap();
        defiance_core::decode::validate_copy(&sites::REDRAW_BEFORE[..sites::REDRAW_SPAN]).unwrap();
        defiance_core::decode::validate_copy(&sites::HOVER_BEFORE[..sites::HOVER_SPAN]).unwrap();
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
        {
            for site in &sites::SITES {
                for count in [9, 12, 36, 126] {
                    let after = replacement(site, count);
                    assert_eq!(after.len(), site.before.len());
                    assert_eq!(&after[..site.field], &site.before[..site.field]);
                    assert_eq!(
                        &after[site.field + site.width..],
                        &site.before[site.field + site.width..]
                    );
                    if count == 9 && !matches!(site.kind, sites::Kind::Zero) {
                        assert_eq!(after, site.before);
                    }
                }
            }
        }
    }
}
