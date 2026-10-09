//! Experimental primary weapon pickups without duplicating shared squad ammo.
use core::ffi::c_void;
use defiance_api::{Api, PatchContractV1, Plugin, ABI_VERSION, LOG_DEBUG, LOG_WARN};
use defiance_core::sites::{code_ranges, Image};

mod ammo;
#[path = "squad_collection.rs"]
mod collection;
mod equipment;
mod native;
mod persistence;
mod policy;
mod reserve;
mod sites;
mod squad_death;

defiance_feature_sdk::service_handshake!();

enum InstallError {
    /// The build is not one the plugin supports; nothing was written.
    UnsupportedBuild(String),
    Failed(String),
}

impl From<String> for InstallError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

impl From<&str> for InstallError {
    fn from(error: &str) -> Self {
        Self::Failed(error.into())
    }
}

/// logic.dll's base, its sites and its image with original code bytes, so a
/// hook another plugin already placed neither hides a site nor changes the
/// bytes this plugin's contract expects.
unsafe fn resolve(api: &Api) -> Result<(*mut u8, sites::Sites, Vec<u8>), InstallError> {
    let base = (api.module_base)(c"logic.dll".as_ptr()).cast::<u8>();
    if base.is_null() {
        return Err("logic.dll is not loaded".into());
    }
    let size = (api.module_size)(base.cast());
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
    let logic = Image {
        image: &image,
        base: base as usize,
    };
    let sites = sites::sites(&logic).map_err(InstallError::UnsupportedBuild)?;
    Ok((base, sites, image))
}

unsafe fn install(api: &Api) -> Result<String, InstallError> {
    let settings = policy::configure(api)?;
    let (base, build, _) = resolve(api)?;
    let address = |rva| base as usize + rva;
    ammo::configure(address(build.import_rounds), address(build.ammo_set_record));
    reserve::configure(reserve::Bindings {
        string_copy: address(build.string_copy),
        string_destroy: address(build.string_destroy),
        log: api.log as usize,
        ..Default::default()
    });
    persistence::configure(persistence::Bindings {
        rebuild_original: 0,
        human_vtable: address(build.human_vtable),
        item_override: address(build.item_override),
        log: api.log as usize,
    });
    squad_death::configure(squad_death::Bindings {
        canonical: address(build.canonical),
        holder_get: address(build.holder_get),
        item_override: address(build.item_override),
        slot_context: address(build.slot_context),
        slot_type: address(build.slot_type),
        ammo_mode: address(build.ammo_mode),
        weak_bind: address(build.weak_bind),
        manager_get: address(build.manager_get),
        spawn: address(build.spawn),
        human_vtable: address(build.human_vtable),
    });
    collection::configure(collection::Bindings {
        canonical_squad: address(build.canonical),
        holder: address(build.holder_get),
        context: address(build.slot_context),
        resolve: address(build.item_override),
        slot: address(build.slot_type),
        human_vtable: address(build.human_vtable),
        detach: address(build.detach),
        dispose: address(build.ammo_dispose),
        primary_add: address(build.primary_add),
        ammo_mode: address(build.ammo_mode),
        ammo_remove: address(build.ammo_remove),
        switch: address(build.visual_switch),
        visual_sync: address(build.visual_sync),
        spawn: address(build.spawn),
        log: api.log as usize,
        ..Default::default()
    });
    if (api.hook_call)(
        base.add(build.death_call).cast(),
        squad_death::primary_death as *mut c_void,
        squad_death::ORIGINAL.as_ptr(),
    ) != 0
    {
        return Err("primary death hook refused; host will finish owned rollback".into());
    }
    if (api.hook_exact)(
        base.add(build.collect).cast(),
        collection::collect as *mut c_void,
        5,
        collection::ORIGINAL.as_ptr(),
    ) != 0
    {
        return Err("primary collection hook refused; host will finish owned rollback".into());
    }
    for (rva, detour, original) in [
        (
            build.collect_drop,
            collection::drop_special as *mut c_void,
            collection::DROP_ORIGINAL.as_ptr(),
        ),
        (
            build.collect_add,
            collection::add as *mut c_void,
            collection::ADD_ORIGINAL.as_ptr(),
        ),
    ] {
        if (api.hook_call)(base.add(rva).cast(), detour, original) != 0 {
            return Err(
                "primary collection call hook refused; host will finish owned rollback".into(),
            );
        }
    }
    if (api.hook_exact)(
        base.add(build.rebuild).cast(),
        persistence::rebuild as *mut c_void,
        5,
        persistence::ORIGINAL.as_ptr(),
    ) != 0
    {
        return Err("squad loadout rebuild hook refused; host will finish owned rollback".into());
    }
    for (rva, detour, original) in [
        (
            build.reserve_writer,
            reserve::writer as *mut c_void,
            reserve::ORIGINAL_WRITER.as_ptr(),
        ),
        (
            build.reserve_load,
            reserve::load as *mut c_void,
            reserve::ORIGINAL_LOAD.as_ptr(),
        ),
        (
            build.reserve_destroy,
            reserve::destroy as *mut c_void,
            reserve::ORIGINAL_DESTROY.as_ptr(),
        ),
    ] {
        if (api.hook_exact)(base.add(rva).cast(), detour, 5, original) != 0 {
            return Err("ammo reserve hook refused; host will finish owned rollback".into());
        }
    }
    Ok(format!(
        "experimental squad primary drops and swaps installed: swap ammo {:?}; death ammo {:?}",
        settings.ammo_policy, settings.death_ammo
    ))
}

#[no_mangle]
/// Declare the loader-owned death and collection replacements.
///
/// # Safety
/// A nonnull `api` must reference the live host API with valid module callbacks.
pub unsafe extern "C" fn defiance_patch_contract_v1(api: *const Api) -> *const PatchContractV1 {
    let Some(host) = api.as_ref() else {
        return core::ptr::null();
    };
    if host.abi_version != ABI_VERSION || host.reserved != 0 {
        return core::ptr::null();
    }
    let Ok((_, sites, image)) = resolve(host) else {
        return core::ptr::null();
    };
    let patches = sites
        .hooks()
        .into_iter()
        .map(
            |(rva, len, kind, _)| defiance_feature_sdk::contract::Patch {
                module: c"logic.dll",
                rva,
                kind,
                before: image[rva..rva + len].to_vec(),
                after: None,
            },
        )
        .collect();
    defiance_feature_sdk::contract::build(api, patches)
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else { return 1 };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    let (level, message, result) = match install(api) {
        Ok(message) => (LOG_DEBUG, message, 0),
        Err(InstallError::UnsupportedBuild(error)) => (
            LOG_WARN,
            format!("weapon drops: not a supported build ({error}); no hooks installed"),
            1,
        ),
        Err(InstallError::Failed(error)) => (
            LOG_DEBUG,
            format!("primary weapon drops refused: {error}"),
            1,
        ),
    };
    if let Ok(message) = std::ffi::CString::new(message) {
        (api.log)(level, message.as_ptr());
    }
    result
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.weapon-drops".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();

/// 0 if every site resolves in a logic.dll image laid out as mapped at
/// `base`, otherwise 1.
///
/// # Safety
/// A nonnull `image` must point to at least `size` readable bytes.
#[cfg(feature = "parity-test")]
#[no_mangle]
pub unsafe extern "C" fn weapon_drops_test_resolve(
    image: *const u8,
    size: usize,
    base: usize,
) -> i32 {
    if image.is_null() {
        return 1;
    }
    let image = Image {
        image: core::slice::from_raw_parts(image, size),
        base,
    };
    i32::from(sites::sites(&image).is_err())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_image_is_refused() {
        for image in [&[][..], &vec![0; 0x600000]] {
            assert!(sites::sites(&Image {
                image,
                base: 0x180000000
            })
            .is_err());
        }
    }
}
