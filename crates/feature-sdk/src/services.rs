//! Typed helpers for the optional loader service extension. Each DLL has its
//! own copy of this client pointer; the shared registry lives in the loader.
use core::ffi::CStr;
use defiance_api::ServiceApiV1;
use std::sync::atomic::{AtomicPtr, Ordering};

static API: AtomicPtr<ServiceApiV1> = AtomicPtr::new(core::ptr::null_mut());

/// Export once from a plugin that uses services. Old loaders ignore the export;
/// required consumers must handle `query` returning None from their init.
#[macro_export]
macro_rules! service_handshake {
    () => {
        #[no_mangle]
        pub unsafe extern "C" fn defiance_plugin_services(
            api: *const defiance_api::ServiceApiV1,
        ) -> i32 {
            unsafe { $crate::services::accept(api) }
        }
    };
}

/// # Safety
/// Pointer must be the process-lifetime extension supplied by the loader.
pub unsafe fn accept(api: *const ServiceApiV1) -> i32 {
    if api.is_null() {
        return 1;
    }
    let header = api.cast::<u32>();
    if unsafe { *header } != 1
        || unsafe { *header.add(1) } < core::mem::size_of::<ServiceApiV1>() as u32
    {
        return 1;
    }
    API.store(api.cast_mut(), Ordering::Release);
    0
}

pub fn available() -> bool {
    !API.load(Ordering::Acquire).is_null()
}

/// # Safety
/// Call only during init. Table must have a documented C-compatible layout,
/// immutable function pointers and process-lifetime storage. Provider owns all
/// state accessed by callbacks and defines their threading/lifetime contract.
pub unsafe fn register<T: Sync + 'static>(
    name: &CStr,
    version: u32,
    table: &'static T,
) -> Result<(), i32> {
    let api = API.load(Ordering::Acquire);
    if api.is_null() {
        return Err(2);
    }
    let result = unsafe {
        ((*api).register)(
            name.as_ptr(),
            version,
            (table as *const T).cast(),
            core::mem::size_of::<T>(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(result)
    }
}

/// # Safety
/// Call only during init and declare provider in the manifest dependencies.
/// T must exactly match the provider's documented C layout for this version.
/// The returned pointer does not authorize calls from arbitrary threads or
/// after stopping. The current loader has no hot unload; stop consumers first.
pub unsafe fn query<T: 'static>(provider: &CStr, name: &CStr, version: u32) -> Option<&'static T> {
    let api = API.load(Ordering::Acquire);
    if api.is_null() {
        return None;
    }
    let table = unsafe {
        ((*api).query)(
            provider.as_ptr(),
            name.as_ptr(),
            version,
            core::mem::size_of::<T>(),
        )
    };
    if table.is_null() || (table as usize) % core::mem::align_of::<T>() != 0 {
        return None;
    }
    unsafe { table.cast::<T>().as_ref() }
}

/// Resolve the public selection-v1 service. The callback still requires a live
/// facet and the game thread; discovery itself happens on the init thread.
/// # Safety
/// Same lifecycle requirements as query; manifest must depend on defiance.selection.
pub unsafe fn selection() -> Option<&'static defiance_api::SelectionV1> {
    unsafe { query(c"defiance.selection", c"selection", 1) }
}

/// Resolve the loader's crash-ranges-v1 table, which names mod code in crash
/// reports. Needs no manifest dependency; None from a loader that predates it.
/// # Safety
/// Same lifecycle requirements as query; the table's calls may then be made
/// from any thread.
pub unsafe fn crash_ranges() -> Option<&'static defiance_api::CrashRangesV1> {
    unsafe { query(defiance_api::LOADER_PROVIDER, c"crash-ranges", 1) }
}

/// Resolve the loader's trace-v1 table, for diagnostic call-stack tracing.
/// Needs no manifest dependency; None from a loader that predates it.
/// # Safety
/// Same lifecycle requirements as query; the table's calls may then be made
/// from any thread.
pub unsafe fn trace() -> Option<&'static defiance_api::TraceV1> {
    unsafe { query(defiance_api::LOADER_PROVIDER, c"trace", 1) }
}

/// Resolve the loader's multiplayer-v1 table: which active plugins block
/// multiplayer. Needs no manifest dependency; None from a loader that
/// predates it.
/// # Safety
/// Same lifecycle requirements as query; the table's call may then be made
/// from any thread.
pub unsafe fn multiplayer() -> Option<&'static defiance_api::MultiplayerV1> {
    unsafe { query(defiance_api::LOADER_PROVIDER, c"multiplayer", 1) }
}

/// Resolve Core's game-thread access table. Declare a direct defiance.core dependency.
/// # Safety
/// Calls through this table require live objects on their owning game thread.
pub unsafe fn game_access() -> Option<&'static defiance_api::GameAccessV1> {
    unsafe { query(c"defiance.core", c"game-access", 1) }
}

/// Copy the current roster using Core's capacity/query protocol. The returned
/// vector owns only its pointer array, not the entities. None means unavailable,
/// malformed, allocation failure, or a roster that changed during the copy.
/// # Safety
/// The entity must be live on the owning game thread; getters must be stable.
pub unsafe fn members(
    game: &defiance_api::GameAccessV1,
    entity: *mut core::ffi::c_void,
) -> Option<Vec<*mut core::ffi::c_void>> {
    let count = unsafe { (game.copy_members)(entity, core::ptr::null_mut(), 0) };
    if count == usize::MAX {
        return None;
    }
    let mut members = Vec::new();
    members.try_reserve_exact(count).ok()?;
    members.resize(count, core::ptr::null_mut());
    if unsafe { (game.copy_members)(entity, members.as_mut_ptr(), count) } == count {
        Some(members)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handshake_refuses_unknown_or_truncated_extension() {
        assert_eq!(unsafe { accept(core::ptr::null()) }, 1);
        let wrong = [2u32, 24];
        assert_eq!(unsafe { accept(wrong.as_ptr().cast()) }, 1);
        let short = [1u32, 8];
        assert_eq!(unsafe { accept(short.as_ptr().cast()) }, 1);
    }
}

/// Resolve Core's thread-safe installed ammo-menu capacity service.
/// # Safety
/// Query during init with a direct defiance.core manifest dependency.
/// Publication is exclusively for the startup menu-layout provider.
pub unsafe fn ammo_menu() -> Option<&'static defiance_api::AmmoMenuV1> {
    unsafe { query(c"defiance.core", c"ammo-menu", 1) }
}
