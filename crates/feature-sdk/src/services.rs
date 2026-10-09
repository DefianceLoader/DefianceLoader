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

/// Resolve cover-markers' move-preview-v1 service.
/// # Safety
/// Same lifecycle requirements as query; manifest must depend on
/// defiance.cover-markers.
pub unsafe fn move_preview() -> Option<&'static defiance_api::MovePreviewV1> {
    unsafe { query(c"defiance.cover-markers", c"move-preview", 1) }
}

/// Resolve Core's patch-v1 table, which installs patch units ([`crate::units`]).
/// Declare a dependency on `defiance.core`.
/// # Safety
/// Same lifecycle requirements as query; call its functions during init.
pub unsafe fn patch() -> Option<&'static defiance_api::PatchV1> {
    unsafe { query(c"defiance.core", c"patch", 1) }
}

/// Resolve Core's build-v1 table: the build whose units apply. Declare a
/// dependency on `defiance.core`.
/// # Safety
/// Same lifecycle requirements as query.
pub unsafe fn build() -> Option<&'static defiance_api::BuildV1> {
    unsafe { query(c"defiance.core", c"build", 1) }
}

/// Resolve the loader's crash-ranges-v1 table, which names mod code in crash
/// reports. Needs no manifest dependency; None from a loader that predates it.
/// # Safety
/// Same lifecycle requirements as query; the table's calls may then be made
/// from any thread.
pub unsafe fn crash_ranges() -> Option<&'static defiance_api::CrashRangesV1> {
    unsafe { query(defiance_api::LOADER_PROVIDER, c"crash-ranges", 1) }
}

/// Resolve the loader's near-memory-v1 table: executable memory held within
/// rel32 reach of the game's modules. Needs no manifest dependency; None from
/// a loader that predates it.
/// # Safety
/// Same lifecycle requirements as query; the table's call may then be made
/// from any thread.
pub unsafe fn near_memory() -> Option<&'static defiance_api::NearMemoryV1> {
    unsafe { query(defiance_api::LOADER_PROVIDER, c"near-memory", 1) }
}

/// Resolve the loader's trace-v1 table, for diagnostic call-stack tracing.
/// Needs no manifest dependency; None from a loader that predates it.
/// # Safety
/// Same lifecycle requirements as query; the table's calls may then be made
/// from any thread.
pub unsafe fn trace() -> Option<&'static defiance_api::TraceV1> {
    unsafe { query(defiance_api::LOADER_PROVIDER, c"trace", 1) }
}

/// Resolve the loader's buffered at-hit capture service. No plugin dependency
/// is needed. Open during init; configure and poll from a worker, then close.
/// # Safety
/// Same lifecycle requirements as [`query`].
pub unsafe fn trace_capture() -> Option<&'static defiance_api::TraceCaptureV1> {
    unsafe { query(defiance_api::LOADER_PROVIDER, c"trace-capture", 1) }
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

/// Resolve the loader's original-v1 table: memory as it was before any plugin
/// wrote to it through the loader. Needs no manifest dependency; None from a
/// loader that predates it.
/// # Safety
/// Same lifecycle requirements as query; the call may be made from any thread.
pub unsafe fn original() -> Option<&'static defiance_api::OriginalV1> {
    unsafe { query(defiance_api::LOADER_PROVIDER, c"original", 1) }
}

/// Resolve the loader's session-v1 table, for Core's mission reports.
/// # Safety
/// Same lifecycle requirements as query; the calls may be made from any thread.
pub unsafe fn session() -> Option<&'static defiance_api::SessionV1> {
    unsafe { query(defiance_api::LOADER_PROVIDER, c"session", 1) }
}

/// Resolve the loader's mission-events-v1 table: mission load, end and frame
/// callbacks. Needs no manifest dependency; None from a loader that predates it.
/// # Safety
/// Same lifecycle requirements as query; subscribe only during plugin init.
pub unsafe fn mission_events() -> Option<&'static defiance_api::MissionEventsV1> {
    unsafe { query(defiance_api::LOADER_PROVIDER, c"mission-events", 1) }
}

/// Resolve the loader's mission-feed-v1 table, for Core's mission frame hook.
/// # Safety
/// Same lifecycle requirements as query.
pub unsafe fn mission_feed() -> Option<&'static defiance_api::MissionFeedV1> {
    unsafe { query(defiance_api::LOADER_PROVIDER, c"mission-feed", 1) }
}

/// Resolve Core's game-thread access table. Declare a direct defiance.core dependency.
/// # Safety
/// Calls through this table require live objects on their owning game thread.
pub unsafe fn game_access() -> Option<&'static defiance_api::GameAccessV1> {
    unsafe { query(c"defiance.core", c"game-access", 1) }
}

/// Resolve Core's relation table. Declare a direct defiance.core dependency.
/// # Safety
/// Calls through this table require null or live entities on the game thread.
pub unsafe fn relation() -> Option<&'static defiance_api::RelationV1> {
    unsafe { query(c"defiance.core", c"relation", 1) }
}

/// Resolve Core's checked facet queries. Declare a direct defiance.core dependency.
/// # Safety
/// Calls through this table require null or live entities on the game thread.
pub unsafe fn facets() -> Option<&'static defiance_api::FacetsV1> {
    unsafe { query(c"defiance.core", c"facets", 1) }
}

/// Resolve Core's selection snapshots. Declare a direct defiance.core dependency.
/// # Safety
/// Calls through this table require a null or live player context on the game thread.
pub unsafe fn selection_snapshot() -> Option<&'static defiance_api::SelectionSnapshotV1> {
    unsafe { query(c"defiance.core", c"selection-snapshot", 1) }
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

/// The soldiers of a squad that an order to it reaches.
pub struct Recipients {
    pub members: Vec<*mut core::ffi::c_void>,
    /// The members are a proper subset of the squad, named by individual
    /// marks; otherwise they are the whole roster.
    pub subset: bool,
}

/// The soldiers of `squad` that an order reaches, as the native order filters
/// read individual marks: the marked soldiers when they are a proper subset,
/// otherwise every soldier. None as for [`members`].
/// # Safety
/// As for [`members`].
pub unsafe fn recipients(
    game: &defiance_api::GameAccessV1,
    squad: *mut core::ffi::c_void,
) -> Option<Recipients> {
    let members = unsafe { self::members(game, squad) }?;
    let selectable = unsafe { (game.selectable)(squad) };
    let marks: Vec<bool> = members
        .iter()
        .map(|&member| {
            let mut state = defiance_api::MemberStateV1::default();
            !selectable.is_null()
                && unsafe { (game.read_member)(member, selectable, &mut state) } == 0
                && state.selected != 0
        })
        .collect();
    let marked = marks.iter().filter(|&&selected| selected).count();
    if marked == 0 || marked == members.len() {
        return Some(Recipients {
            members,
            subset: false,
        });
    }
    Some(Recipients {
        members: members
            .into_iter()
            .zip(marks)
            .filter_map(|(member, marked)| marked.then_some(member))
            .collect(),
        subset: true,
    })
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

    /// Without the handshake export the loader never binds a plugin's client,
    /// so each of its queries answers None.
    #[test]
    fn every_plugin_that_queries_services_exports_the_handshake() {
        let plugins = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        let mut missing = Vec::new();
        for plugin in std::fs::read_dir(&plugins).unwrap() {
            let src = plugin.unwrap().path().join("src");
            let Ok(files) = std::fs::read_dir(&src) else {
                continue;
            };
            let source: String = files
                .filter_map(|file| std::fs::read_to_string(file.unwrap().path()).ok())
                .collect();
            if source.contains("defiance_feature_sdk::services::")
                && !source.contains("service_handshake!()")
            {
                missing.push(src.display().to_string());
            }
        }
        assert!(missing.is_empty(), "no service handshake: {missing:?}");
    }
}

/// Resolve Core's thread-safe installed ammo-menu capacity service.
/// # Safety
/// Query during init with a direct defiance.core manifest dependency.
/// Publication is exclusively for the startup menu-layout provider.
pub unsafe fn ammo_menu() -> Option<&'static defiance_api::AmmoMenuV1> {
    unsafe { query(c"defiance.core", c"ammo-menu", 1) }
}

/// Resolve Core's game-symbol catalog. Declare a direct defiance.core
/// dependency at the version that publishes the names you use.
/// # Safety
/// Query during or after init.
pub unsafe fn game_symbols() -> Option<&'static defiance_api::GameSymbolsV1> {
    unsafe { query(c"defiance.core", c"game-symbols", 1) }
}

/// A game function Core resolved by name ([`game_symbol`]).
pub struct GameSymbol {
    pub module: &'static CStr,
    pub rva: usize,
    pub address: usize,
    /// The bytes Core checked at the symbol in the original image.
    pub expected: &'static [u8],
    /// The entry hook's span; 0 when the symbol allows none.
    pub span: usize,
    /// False when Core runs an unknown build: the signature matched, but the
    /// ABI is unproven.
    pub known_build: bool,
}

impl GameSymbol {
    /// The contract entry for an entry hook over [`GameSymbol::span`].
    pub fn entry_patch(&self) -> crate::contract::Patch {
        crate::contract::Patch {
            module: self.module,
            rva: self.rva,
            kind: defiance_api::PATCH_KIND_ENTRY,
            before: self.expected[..self.span].to_vec(),
            after: None,
        }
    }
}

/// Resolve `name` from Core's catalog for ABI version `abi` and `use_` (a
/// `SYMBOL_USE_*`). The error names the symbol and the refusal.
/// # Safety
/// Query during or after init.
pub unsafe fn game_symbol(name: &CStr, abi: u32, use_: u32) -> Result<GameSymbol, String> {
    use defiance_api::*;
    let label = name.to_string_lossy();
    let table = unsafe { game_symbols() }
        .ok_or_else(|| format!("{label}: Core's game-symbols service is unavailable"))?;
    let mut out = core::mem::MaybeUninit::<GameSymbolV1>::uninit();
    let status = unsafe { (table.resolve)(name.as_ptr(), abi, use_, out.as_mut_ptr()) };
    if status != SYMBOL_OK {
        let why = match status {
            SYMBOL_UNKNOWN_NAME => "not in Core's catalog",
            SYMBOL_UNAVAILABLE => "not found in this build",
            SYMBOL_AMBIGUOUS => "matches more than one place",
            SYMBOL_ABI_MISMATCH => "published with another ABI version",
            SYMBOL_MISUSE => "does not allow that use",
            _ => "invalid request",
        };
        return Err(format!("{label}: {why}"));
    }
    let out = unsafe { out.assume_init() };
    if out.module.is_null() || out.expected.is_null() || out.span as usize > out.expected_len {
        return Err(format!("{label}: Core answered a malformed symbol"));
    }
    Ok(GameSymbol {
        module: unsafe { CStr::from_ptr(out.module) },
        rva: out.rva,
        address: out.address,
        expected: unsafe { core::slice::from_raw_parts(out.expected, out.expected_len) },
        span: out.span as usize,
        known_build: out.flags & SYMBOL_KNOWN_BUILD != 0,
    })
}
