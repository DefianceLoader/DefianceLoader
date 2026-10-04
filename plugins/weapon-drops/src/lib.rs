//! Experimental primary weapon pickups without duplicating shared squad ammo.
use core::ffi::c_void;
use defiance_api::{
    Api, PatchContractV1, Plugin, ABI_VERSION, LOG_DEBUG, PATCH_KIND_CALL, PATCH_KIND_ENTRY,
};

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

struct Guard {
    rva: usize,
    bytes: usize,
    sha: &'static str,
}

struct Build {
    name: &'static str,
    sha: &'static str,
    death_call: usize,
    death_before: &'static [u8],
    collect: usize,
    collect_before: &'static [u8],
    collect_drop: usize,
    collect_drop_before: &'static [u8],
    collect_add: usize,
    collect_add_before: &'static [u8],
    rebuild: usize,
    rebuild_before: &'static [u8],
    reserve_writer: usize,
    reserve_writer_before: &'static [u8],
    reserve_load: usize,
    reserve_load_before: &'static [u8],
    reserve_destroy: usize,
    reserve_destroy_before: &'static [u8],
    string_copy: usize,
    string_destroy: usize,
    detach: usize,
    ammo_dispose: usize,
    primary_add: usize,
    ammo_mode: usize,
    ammo_remove: usize,
    import_rounds: usize,
    ammo_set_record: usize,
    weak_bind: usize,
    visual_switch: usize,
    visual_sync: usize,
    manager_get: usize,
    spawn: usize,
    slot_type: usize,
    slot_context: usize,
    item_override: usize,
    canonical: usize,
    holder_get: usize,
    human_vtable: usize,
    guards: &'static [Guard],
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, capacity: u32) -> u32;
}

fn validate(image: &[u8], build: &Build) -> Result<(), &'static str> {
    let code = image
        .get(
            build.death_call
                ..build
                    .death_call
                    .checked_add(build.death_before.len())
                    .ok_or("death call out of bounds")?,
        )
        .ok_or("death call out of bounds")?;
    if code != build.death_before {
        return Err("death call differs; no writes made");
    }
    for (rva, before) in [
        (build.collect, build.collect_before),
        (build.collect_drop, build.collect_drop_before),
        (build.collect_add, build.collect_add_before),
        (build.rebuild, build.rebuild_before),
        (build.reserve_writer, build.reserve_writer_before),
        (build.reserve_load, build.reserve_load_before),
        (build.reserve_destroy, build.reserve_destroy_before),
    ] {
        if image.get(rva..rva + before.len()) != Some(before) {
            return Err("collection site differs; no writes made");
        }
    }
    for guard in build.guards {
        let end = guard
            .rva
            .checked_add(guard.bytes)
            .ok_or("guard out of bounds")?;
        let body = image.get(guard.rva..end).ok_or("guard out of bounds")?;
        let mut sha = defiance_core::sha256::Sha256::new();
        sha.update(body);
        if defiance_core::sha256::hex(&sha.finish()) != guard.sha {
            return Err("native weapon drop code differs; no writes made");
        }
    }
    Ok(())
}

unsafe fn selected_build(api: &Api) -> Result<(*mut c_void, &'static Build), String> {
    let base = (api.module_base)(c"logic.dll".as_ptr());
    if base.is_null() {
        return Err("logic.dll is not loaded".into());
    }
    let mut path = [0u16; 32768];
    let length = GetModuleFileNameW(base, path.as_mut_ptr(), path.len() as u32) as usize;
    if length == 0 || length >= path.len() {
        return Err("cannot resolve logic.dll path".into());
    }
    use std::os::windows::ffi::OsStringExt;
    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&path[..length]));
    let sha = defiance_core::sha256::file(&path).map_err(|error| error.to_string())?;
    let build = sites::BUILDS
        .iter()
        .find(|build| build.sha == sha)
        .ok_or("unsupported logic.dll; no writes made")?;
    validate(
        core::slice::from_raw_parts(base.cast::<u8>(), (api.module_size)(base)),
        build,
    )?;
    Ok((base, build))
}

unsafe fn install(api: &Api) -> Result<String, String> {
    let settings = policy::configure(api)?;
    let (base, build) = selected_build(api)?;
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
        base.cast::<u8>().add(build.death_call).cast(),
        squad_death::primary_death as *mut c_void,
        squad_death::ORIGINAL.as_ptr(),
    ) != 0
    {
        return Err("primary death hook refused; host will finish owned rollback".into());
    }
    if (api.hook_exact)(
        base.cast::<u8>().add(build.collect).cast(),
        collection::collect as *mut c_void,
        build.collect_before.len(),
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
        if (api.hook_call)(base.cast::<u8>().add(rva).cast(), detour, original) != 0 {
            return Err(
                "primary collection call hook refused; host will finish owned rollback".into(),
            );
        }
    }
    if (api.hook_exact)(
        base.cast::<u8>().add(build.rebuild).cast(),
        persistence::rebuild as *mut c_void,
        build.rebuild_before.len(),
        persistence::ORIGINAL.as_ptr(),
    ) != 0
    {
        return Err("squad loadout rebuild hook refused; host will finish owned rollback".into());
    }
    for (rva, before, detour, original) in [
        (
            build.reserve_writer,
            build.reserve_writer_before,
            reserve::writer as *mut c_void,
            reserve::ORIGINAL_WRITER.as_ptr(),
        ),
        (
            build.reserve_load,
            build.reserve_load_before,
            reserve::load as *mut c_void,
            reserve::ORIGINAL_LOAD.as_ptr(),
        ),
        (
            build.reserve_destroy,
            build.reserve_destroy_before,
            reserve::destroy as *mut c_void,
            reserve::ORIGINAL_DESTROY.as_ptr(),
        ),
    ] {
        if (api.hook_exact)(
            base.cast::<u8>().add(rva).cast(),
            detour,
            before.len(),
            original,
        ) != 0
        {
            return Err("ammo reserve hook refused; host will finish owned rollback".into());
        }
    }
    Ok(format!(
        "experimental squad primary drops and swaps installed ({}): swap ammo {:?}; death ammo {:?}",
        build.name, settings.ammo_policy, settings.death_ammo
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
    let Ok((_, build)) = selected_build(host) else {
        return core::ptr::null();
    };
    defiance_feature_sdk::contract::build(
        api,
        vec![
            defiance_feature_sdk::contract::Patch {
                module: c"logic.dll",
                rva: build.reserve_writer,
                kind: PATCH_KIND_ENTRY,
                before: build.reserve_writer_before.to_vec(),
                after: None,
            },
            defiance_feature_sdk::contract::Patch {
                module: c"logic.dll",
                rva: build.reserve_load,
                kind: PATCH_KIND_ENTRY,
                before: build.reserve_load_before.to_vec(),
                after: None,
            },
            defiance_feature_sdk::contract::Patch {
                module: c"logic.dll",
                rva: build.reserve_destroy,
                kind: PATCH_KIND_ENTRY,
                before: build.reserve_destroy_before.to_vec(),
                after: None,
            },
            defiance_feature_sdk::contract::Patch {
                module: c"logic.dll",
                rva: build.rebuild,
                kind: PATCH_KIND_ENTRY,
                before: build.rebuild_before.to_vec(),
                after: None,
            },
            defiance_feature_sdk::contract::Patch {
                module: c"logic.dll",
                rva: build.death_call,
                kind: PATCH_KIND_CALL,
                before: build.death_before.to_vec(),
                after: None,
            },
            defiance_feature_sdk::contract::Patch {
                module: c"logic.dll",
                rva: build.collect,
                kind: PATCH_KIND_ENTRY,
                before: build.collect_before.to_vec(),
                after: None,
            },
            defiance_feature_sdk::contract::Patch {
                module: c"logic.dll",
                rva: build.collect_drop,
                kind: PATCH_KIND_CALL,
                before: build.collect_drop_before.to_vec(),
                after: None,
            },
            defiance_feature_sdk::contract::Patch {
                module: c"logic.dll",
                rva: build.collect_add,
                kind: PATCH_KIND_CALL,
                before: build.collect_add_before.to_vec(),
                after: None,
            },
        ],
    )
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else { return 1 };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    let (message, result) = match install(api) {
        Ok(message) => (message, 0),
        Err(error) => (format!("primary weapon drops refused: {error}"), 1),
    };
    if let Ok(message) = std::ffi::CString::new(message) {
        (api.log)(LOG_DEBUG, message.as_ptr());
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

#[cfg(feature = "parity-test")]
#[no_mangle]
/// Validate a mapped fixture without calling native game functions.
///
/// # Safety
/// A nonnull `image` must point to at least `size` readable bytes.
pub unsafe extern "C" fn weapon_drops_test_validate(
    image: *const u8,
    size: usize,
    build: usize,
) -> i32 {
    let Some(build) = sites::BUILDS.get(build) else {
        return 1;
    };
    if image.is_null() {
        return 1;
    }
    i32::from(validate(core::slice::from_raw_parts(image, size), build).is_err())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_image_is_refused() {
        assert!(validate(&[], &sites::BUILDS[0]).is_err());
        assert!(validate(&vec![0; 0x600000], &sites::BUILDS[0]).is_err());
    }
}
