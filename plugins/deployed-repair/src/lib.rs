//! Permit crew self-repair while deployed and preserve deployment during crew exit.
use core::ffi::c_void;
use defiance_api::{
    Api, PatchContractV1, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO, LOG_WARN, PATCH_KIND_CALL,
    PATCH_KIND_ENTRY,
};
use defiance_core::sites::Image;
use std::sync::atomic::Ordering;

mod guard;
mod logic_sites;
mod menu;
mod repair_position;
mod sites;

enum InstallError {
    UnsupportedBuild(String),
    Failed(String),
}

struct Resolved {
    game: *mut u8,
    menu: usize,
    helicopter: sites::Helicopter,
    position: repair_position::Site,
    logic: *mut u8,
    native: logic_sites::Sites,
    // diagnostic: AiFacet::vfunc_12, the order intake that never starts a deployed repair.
    #[cfg(feature = "diagnostics")]
    intake: usize,
}

// diagnostic: the whole instructions displaced by the intake hook.
#[cfg(feature = "diagnostics")]
const INTAKE_BEFORE: &[u8] = b"\x48\x89\x5c\x24\x18\x48\x89\x54\x24\x10\x57\x48\x83\xec\x30";

unsafe fn log(api: &Api, level: u32, message: &str) {
    if let Ok(message) = std::ffi::CString::new(message) {
        (api.log)(level, message.as_ptr());
    }
}

/// Resolve and validate every site before staging any mutation.
unsafe fn resolve(api: &Api) -> Result<Resolved, InstallError> {
    let game = (api.module_base)(c"game.dll".as_ptr()).cast::<u8>();
    let logic = (api.module_base)(c"logic.dll".as_ptr()).cast::<u8>();
    if game.is_null() || logic.is_null() {
        return Err(InstallError::Failed(
            "game.dll or logic.dll is not loaded".into(),
        ));
    }
    let game_image = Image::loaded(game, (api.module_size)(game.cast()));
    let logic_image = Image::loaded(logic, (api.module_size)(logic.cast()));
    let menu = sites::menu(&game_image).map_err(InstallError::UnsupportedBuild)?;
    let helicopter =
        sites::helicopter(&game_image, &logic_image).map_err(InstallError::UnsupportedBuild)?;
    let native = logic_sites::resolve(&logic_image).map_err(InstallError::UnsupportedBuild)?;
    let position =
        repair_position::resolve(&logic_image).map_err(InstallError::UnsupportedBuild)?;
    // diagnostic: AiFacet::vfunc_12, the order intake that never starts a deployed repair.
    #[cfg(feature = "diagnostics")]
    let intake = {
        let intake = logic_image
            .method(".?AVAiFacet@Leonardo@@", 12)
            .map_err(InstallError::UnsupportedBuild)?;
        logic_image
            .expect("AI order intake", intake, INTAKE_BEFORE)
            .map_err(InstallError::UnsupportedBuild)?;
        intake
    };
    Ok(Resolved {
        game,
        menu,
        helicopter,
        position,
        logic,
        native,
        #[cfg(feature = "diagnostics")]
        intake,
    })
}

unsafe fn install(api: &Api, resolved: &Resolved) -> Result<(), InstallError> {
    menu::RESUME.store(
        resolved.game as usize + resolved.menu + sites::MENU_BEFORE.len(),
        Ordering::Release,
    );
    menu::HELICOPTER_RESUME.store(
        resolved.game as usize + resolved.helicopter.site + resolved.helicopter.before().len(),
        Ordering::Release,
    );
    menu::HELICOPTER_QUERY_SLOT.store(resolved.helicopter.slot, Ordering::Release);
    repair_position::RESUME.store(
        resolved.logic as usize + resolved.position.site + resolved.position.before().len(),
        Ordering::Release,
    );
    repair_position::POINTS_OFFSET.store(resolved.position.offset, Ordering::Release);
    guard::PUSH_TASK.store(
        resolved.logic as usize + resolved.native.push,
        Ordering::Release,
    );
    guard::TASK_ID.store(resolved.native.task_id, Ordering::Release);
    guard::SELF_REPAIR_ID.store(resolved.native.self_repair_id as usize, Ordering::Release);
    let hooks = [
        (
            resolved.logic.add(resolved.native.recharge),
            guard::recharge_order as *mut c_void,
            logic_sites::RECHARGE_BEFORE.len(),
            &guard::RECHARGE_ORIGINAL,
        ),
        (
            resolved.logic.add(resolved.native.sit_on_land),
            guard::sit_on_land_order as *mut c_void,
            logic_sites::SIT_ON_LAND_BEFORE.len(),
            &guard::SIT_ON_LAND_ORIGINAL,
        ),
        (
            resolved.logic.add(resolved.native.order),
            guard::self_repair_order as *mut c_void,
            resolved.native.order_before.len(),
            &guard::ORDER_ORIGINAL,
        ),
        (
            resolved.game.add(resolved.menu),
            menu::mask as *mut c_void,
            sites::MENU_BEFORE.len(),
            &menu::ORIGINAL,
        ),
        (
            resolved.game.add(resolved.helicopter.site),
            menu::helicopter as *mut c_void,
            resolved.helicopter.before().len(),
            &menu::HELICOPTER_ORIGINAL,
        ),
        (
            resolved.logic.add(resolved.position.site),
            repair_position::points as *mut c_void,
            resolved.position.before().len(),
            &repair_position::ORIGINAL,
        ),
        // diagnostic: logs the intake state for self-repair orders.
        #[cfg(feature = "diagnostics")]
        (
            resolved.logic.add(resolved.intake),
            guard::diagnostics::intake as *mut c_void,
            INTAKE_BEFORE.len(),
            &guard::diagnostics::INTAKE_ORIGINAL,
        ),
    ];
    for (address, detour, span, original) in hooks {
        if (api.hook_exact)(address.cast(), detour, span, original.as_ptr().cast()) != 0 {
            return Err(InstallError::Failed(
                "hook refused; host will complete owned rollback".into(),
            ));
        }
    }
    if (api.hook_call)(
        resolved.logic.add(resolved.native.stop).cast(),
        guard::stop_deployment as *mut c_void,
        guard::STOP_ORIGINAL.as_ptr().cast(),
    ) != 0
    {
        return Err(InstallError::Failed(
            "crew-exit call hook refused; host will complete owned rollback".into(),
        ));
    }
    Ok(())
}

#[no_mangle]
pub unsafe extern "C" fn defiance_patch_contract_v1(api: *const Api) -> *const PatchContractV1 {
    let Some(api_ref) = api.as_ref() else {
        return core::ptr::null();
    };
    if api_ref.abi_version != ABI_VERSION || api_ref.reserved != 0 {
        return core::ptr::null();
    }
    let patches = match resolve(api_ref) {
        Ok(sites) => {
            #[allow(unused_mut)]
            let mut patches = vec![
                defiance_feature_sdk::contract::Patch {
                    module: c"game.dll",
                    rva: sites.menu,
                    kind: PATCH_KIND_ENTRY,
                    before: sites::MENU_BEFORE.to_vec(),
                    after: None,
                },
                defiance_feature_sdk::contract::Patch {
                    module: c"game.dll",
                    rva: sites.helicopter.site,
                    kind: PATCH_KIND_ENTRY,
                    before: sites.helicopter.before().to_vec(),
                    after: None,
                },
                defiance_feature_sdk::contract::Patch {
                    module: c"logic.dll",
                    rva: sites.native.order,
                    kind: PATCH_KIND_ENTRY,
                    before: sites.native.order_before,
                    after: None,
                },
                defiance_feature_sdk::contract::Patch {
                    module: c"logic.dll",
                    rva: sites.native.stop,
                    kind: PATCH_KIND_CALL,
                    before: sites.native.stop_before,
                    after: None,
                },
                defiance_feature_sdk::contract::Patch {
                    module: c"logic.dll",
                    rva: sites.native.recharge,
                    kind: PATCH_KIND_ENTRY,
                    before: logic_sites::RECHARGE_BEFORE.to_vec(),
                    after: None,
                },
                defiance_feature_sdk::contract::Patch {
                    module: c"logic.dll",
                    rva: sites.native.sit_on_land,
                    kind: PATCH_KIND_ENTRY,
                    before: logic_sites::SIT_ON_LAND_BEFORE.to_vec(),
                    after: None,
                },
                defiance_feature_sdk::contract::Patch {
                    module: c"logic.dll",
                    rva: sites.position.site,
                    kind: PATCH_KIND_ENTRY,
                    before: sites.position.before().to_vec(),
                    after: None,
                },
            ];
            // diagnostic: the intake hook is declared like the shipping ones.
            #[cfg(feature = "diagnostics")]
            patches.push(defiance_feature_sdk::contract::Patch {
                module: c"logic.dll",
                rva: sites.intake,
                kind: PATCH_KIND_ENTRY,
                before: INTAKE_BEFORE.to_vec(),
                after: None,
            });
            patches
        }
        Err(InstallError::UnsupportedBuild(_)) => Vec::new(),
        Err(InstallError::Failed(_)) => return core::ptr::null(),
    };
    defiance_feature_sdk::contract::build(api, patches)
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else {
        return 1;
    };
    // diagnostic: lets the self-repair detour report what native startup sees.
    #[cfg(feature = "diagnostics")]
    guard::diagnostics::LOG.store(api.log as usize, Ordering::Release);
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    match resolve(api).and_then(|resolved| {
        install(api, &resolved)?;
        Ok(resolved)
    }) {
        Ok(sites) => {
            log(
                api,
                LOG_INFO,
                &format!(
                    "deployed repair installed (game.dll+{:#x}, +{:#x}; logic.dll+{:#x}, +{:#x}, +{:#x}, +{:#x}, +{:#x})",
                    sites.menu,
                    sites.helicopter.site,
                    sites.native.order,
                    sites.native.stop,
                    sites.native.recharge,
                    sites.native.sit_on_land,
                    sites.position.site
                ),
            );
            0
        }
        Err(InstallError::UnsupportedBuild(reason)) => {
            log(
                api,
                LOG_WARN,
                &format!("deployed repair: unsupported build ({reason}); no writes made"),
            );
            0
        }
        Err(InstallError::Failed(reason)) => {
            log(
                api,
                LOG_ERROR,
                &format!("deployed repair refused: {reason}"),
            );
            1
        }
    }
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.deployed-repair".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();

#[cfg(feature = "parity-test")]
#[no_mangle]
pub unsafe extern "C" fn deployed_repair_test_resolve(
    game: *const u8,
    game_size: usize,
    game_base: usize,
    logic: *const u8,
    logic_size: usize,
    logic_base: usize,
    output: *mut usize,
) -> i32 {
    if game.is_null() || logic.is_null() || output.is_null() {
        return 1;
    }
    let game = Image {
        image: core::slice::from_raw_parts(game, game_size),
        base: game_base,
    };
    let logic = Image {
        image: core::slice::from_raw_parts(logic, logic_size),
        base: logic_base,
    };
    let Ok(menu) = sites::menu(&game) else {
        return 1;
    };
    if sites::helicopter(&game, &logic).is_err() {
        return 1;
    }
    let Ok(native) = logic_sites::resolve(&logic) else {
        return 1;
    };
    if repair_position::resolve(&logic).is_err() {
        return 1;
    }
    core::slice::from_raw_parts_mut(output, 3).copy_from_slice(&[menu, native.order, native.stop]);
    0
}
