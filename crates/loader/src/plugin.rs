//! Loading a planned plugin DLL, and taking it back out.
//!
//! Identity checks, the feature-mask and crash handshakes, the service
//! extension handshake and the hook/service lifecycle live here, apart from the
//! host's startup orchestration in `host.rs`. A plugin loads from a shadow copy
//! (`lifecycle::shadow_copy`) and, once initialized, is recorded for
//! `lifecycle::unload`. A plugin that fails has its `stop` called and its hooks
//! removed; the DLL itself is left loaded, since pulling it out from under
//! anything it started is worse.

use crate::plan::Planned;
use crate::win;
use core::ffi::CStr;
use defiance_api::{
    Api, Entry, PatchContractEntry, PatchContractV1, ABI_VERSION, PATCH_CONTRACT_ENTRY,
    PATCH_KIND_BYTES, PATCH_KIND_CALL, PATCH_KIND_ENTRY, PLUGIN_ENTRY,
};

/// Why loading a planned plugin did not finish. `degraded` is set when its
/// spans could not all be restored: the host must then stop installing, since
/// code reachable from a live hook may not be freed or overwritten.
pub struct LoadFailure {
    pub reason: String,
    pub degraded: bool,
}

impl LoadFailure {
    pub fn plain(reason: impl Into<String>) -> LoadFailure {
        LoadFailure {
            reason: reason.into(),
            degraded: false,
        }
    }
}

fn c_string(text: *const core::ffi::c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// A warning when the version a DLL exports differs from its manifest's, or
/// `None` when they agree. The manifest is the authority: dependency ranges,
/// the built-in table check and the lifecycle all use its version, so a player
/// can relabel a plugin by editing the manifest alone. The DLL's own version
/// is only reported.
fn version_disagreement(exported: &str, declared: crate::manifest::Version) -> Option<String> {
    let agrees = crate::manifest::Version::parse(exported)
        .map(|parsed| parsed == declared)
        .unwrap_or_else(|_| exported == declared.to_string());
    (!agrees).then(|| {
        format!(
            "exports version `{exported}` but its manifest declares `{declared}`; using {declared}"
        )
    })
}

/// Whether a plugin loads from a shadow copy: only one its manifest allows to
/// be reloaded while the game runs.
pub fn loads_from_copy(node: &Planned) -> bool {
    node.manifest.as_ref().is_some_and(|m| m.hot_reload)
}

/// Load one planned plugin DLL, verify its exported identity against the
/// manifest, and run its `init`. A plugin that fails is logged, `stop` is
/// called, and any hook it managed to install before failing is removed; the
/// DLL itself is left loaded, since pulling it out from under anything it
/// started is worse.
pub fn load(node: &Planned, api: &'static Api, owner: usize) -> Result<(), LoadFailure> {
    if node
        .manifest
        .as_ref()
        .is_some_and(|manifest| manifest.abi != ABI_VERSION)
    {
        return Err(LoadFailure::plain("manifest ABI does not match the host"));
    }
    // A reloadable plugin loads from a copy, so the DLL in the plugins
    // directory stays replaceable. The others load in place under their own
    // file names, which other plugins may look them up by (the feature
    // plugins find Core's module as `defiance_plugin_core.dll`).
    let shadow = if !loads_from_copy(node) {
        node.path.clone()
    } else {
        crate::config::current()
            .map(|config| config.paths.root.join("cache").join("plugins"))
            .ok_or_else(|| "no configuration".to_string())
            .and_then(|cache| {
                crate::lifecycle::shadow_copy(&node.path, &cache).map_err(|e| e.to_string())
            })
            .unwrap_or_else(|e| {
                crate::log::warn(&format!(
                    "{}: loading in place, not from a copy ({e}); it cannot be reloaded",
                    node.id
                ));
                node.path.clone()
            })
    };
    let wide = win::wide(&shadow.to_string_lossy());
    let module = unsafe { win::LoadLibraryW(wide.as_ptr()) };
    if module.is_null() {
        return Err(LoadFailure::plain(format!(
            "could not be loaded (error {})",
            unsafe { win::GetLastError() }
        )));
    }
    let symbol = unsafe { win::GetProcAddress(module, PLUGIN_ENTRY.as_ptr()) };
    if symbol.is_null() {
        return Err(LoadFailure::plain("has no `defiance_plugin` export"));
    }
    let entry: Entry = unsafe { core::mem::transmute(symbol) };
    let plugin = unsafe { entry() };
    if plugin.is_null() {
        return Err(LoadFailure::plain("returned no plugin"));
    }
    let plugin = unsafe { &*plugin };
    if plugin.abi_version != ABI_VERSION {
        return Err(LoadFailure::plain(format!(
            "wants ABI {} but this loader is {ABI_VERSION}",
            plugin.abi_version
        )));
    }
    let name = c_string(plugin.name);
    let version = c_string(plugin.version);
    if let Some(manifest) = &node.manifest {
        if !name.eq_ignore_ascii_case(&manifest.id) {
            return Err(LoadFailure::plain(format!(
                "exports identity `{name}` but its manifest declares `{}`",
                manifest.id
            )));
        }
        if let Some(note) = version_disagreement(&version, manifest.version) {
            crate::log::warn(&format!("{name}: {note}"));
        }
    }
    let contract_symbol = unsafe { win::GetProcAddress(module, PATCH_CONTRACT_ENTRY.as_ptr()) };
    let contract_required = node
        .manifest
        .as_ref()
        .is_some_and(|manifest| manifest.patch_contract);
    if contract_required && contract_symbol.is_null() {
        return Err(LoadFailure::plain(
            "manifest requires `defiance_patch_contract_v1`, but the export is missing",
        ));
    }
    let patch_contract: Option<PatchContractEntry> =
        (!contract_symbol.is_null()).then(|| unsafe { core::mem::transmute(contract_symbol) });
    crate::log::info(&format!(
        "plugin {name} {version} from {}",
        node.path.display()
    ));
    let mut module_info: win::ModuleInfo = unsafe { core::mem::zeroed() };
    if unsafe {
        win::GetModuleInformation(
            win::GetCurrentProcess(),
            module,
            &mut module_info,
            core::mem::size_of::<win::ModuleInfo>() as u32,
        )
    } != 0
    {
        crate::crash::map(
            module_info.base as usize,
            module_info.base as usize + module_info.size as usize,
            &format!("{name} DLL"),
        );
    }
    if let Ok(hash) = defiance_core::sha256::file(&node.path) {
        crate::log::debug(&format!("build sha256={hash} {}", node.path.display()));
    }
    let crash_handshake =
        unsafe { win::GetProcAddress(module, b"defiance_plugin_crash_v1\0".as_ptr()) };
    if crate::crash::available() && !crash_handshake.is_null() {
        let accept: unsafe extern "C" fn(unsafe extern "C" fn(*const u8, usize)) =
            unsafe { core::mem::transmute(crash_handshake) };
        unsafe {
            accept(crate::crash::panic_report);
        }
    }
    let handshake = unsafe { win::GetProcAddress(module, defiance_api::SERVICES_ENTRY.as_ptr()) };
    if !handshake.is_null() {
        if node.manifest.is_none() {
            return Err(LoadFailure::plain("service plugins require a manifest"));
        }
        let handshake: unsafe extern "C" fn(*const defiance_api::ServiceApiV1) -> i32 =
            unsafe { core::mem::transmute(handshake) };
        if unsafe { handshake(&crate::services::API) } != 0 {
            return Err(LoadFailure::plain("service extension handshake failed"));
        }
    }
    let api = crate::plugin_log::bind(api, &node.id).map_err(LoadFailure::plain)?;
    crate::services::begin(
        owner,
        &name,
        node.manifest
            .as_ref()
            .map(|m| m.depends.iter().map(|d| d.id.clone()).collect())
            .unwrap_or_default(),
    );
    // Managed plugins stage hooks for the runtime-wide plan. Legacy plugins
    // keep their immediate, ownership-checked installation path.
    if node.legacy {
        crate::hooks::begin_plugin(owner, &name);
    } else {
        crate::hooks::begin_managed_plugin(owner, &name);
    }
    let mut code = unsafe { (plugin.init)(api as *const Api) };
    let mut failure_reason = format!("init returned {code}");
    if code == 0 && !node.legacy {
        if let Some(contract) = patch_contract {
            match unsafe { read_patch_contract(api, contract) }
                .and_then(|expected| crate::hooks::validate_staged_contract(owner, &expected))
            {
                Ok(()) => {}
                Err(reason) => {
                    code = 1;
                    failure_reason = format!("patch contract mismatch: {reason}");
                    crate::log::error(&format!("{name}: {failure_reason}"));
                }
            }
        }
    }
    crate::hooks::end_plugin();
    crate::services::finish(owner, code == 0);
    if code != 0 {
        if let Some(stop) = plugin.stop {
            unsafe { stop() };
        }
        let (removed, failed) = crate::hooks::remove_owned_report(owner);
        if removed > 0 {
            crate::log::info(&format!(
                "removed {removed} hook(s) left by the failed {name}"
            ));
        }
        if failed > 0 {
            crate::log::error(&format!(
                "{name}: {failed} span(s) could not be restored; bookkeeping and storage retained"
            ));
        }
        return Err(LoadFailure {
            reason: failure_reason,
            degraded: failed > 0,
        });
    }
    let loaded = crate::lifecycle::Loaded {
        owner,
        id: node.id.clone(),
        version: node.manifest.as_ref().map(|manifest| manifest.version),
        name,
        path: node.path.clone(),
        reloadable: shadow != node.path && node.manifest.as_ref().is_some_and(|m| m.hot_reload),
        shadow,
        module: module as usize,
        code: crate::lifecycle::code_ranges(module as usize),
        stop: plugin.stop,
        stamp: crate::lifecycle::stamp(&node.path),
        multiplayer_safe: crate::plan::multiplayer_safe(node),
        manifest_settings: node
            .manifest
            .as_ref()
            .map(|manifest| manifest.settings.clone()),
    };
    if node.legacy {
        crate::lifecycle::record(loaded);
        return Ok(());
    }
    crate::lifecycle::stage(loaded);
    if !crate::hooks::patch_plan_active() {
        match crate::hooks::commit_staged(&[owner]) {
            Ok(()) => crate::lifecycle::commit_pending(&[owner]),
            Err(crate::code::CommitError::BeforeWrite(reason)) => {
                let cleanup = crate::lifecycle::discard_started(owner);
                let degraded = cleanup.is_err();
                let detail = cleanup
                    .err()
                    .map(|error| format!("; cleanup was incomplete: {error}"))
                    .unwrap_or_default();
                return Err(LoadFailure {
                    reason: format!("unified patch commit failed: {reason}{detail}"),
                    degraded,
                });
            }
            Err(crate::code::CommitError::AfterWrite(reason)) => {
                crate::lifecycle::commit_pending(&[owner]);
                crate::log::error(&format!(
                    "{}: unified patch commit may be partial; code and ownership were retained: {reason}",
                    node.id
                ));
                return Err(LoadFailure {
                    reason: format!("unified patch commit may be partial: {reason}"),
                    degraded: true,
                });
            }
        }
    }
    Ok(())
}

unsafe fn read_patch_contract(
    api: &Api,
    entry: PatchContractEntry,
) -> Result<Vec<crate::hooks::ExpectedPatch>, String> {
    let contract = unsafe { entry(api as *const Api) };
    if contract.is_null() {
        return Err("contract export returned null".into());
    }
    let contract = unsafe { &*contract };
    if contract.version != 1 || contract.size < core::mem::size_of::<PatchContractV1>() as u32 {
        return Err(format!(
            "unsupported contract version/size {}/{}",
            contract.version, contract.size
        ));
    }
    if contract.count > 4096 || (contract.count != 0 && contract.entries.is_null()) {
        return Err("contract has an invalid entry list".into());
    }
    let entries = if contract.count == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(contract.entries, contract.count) }
    };
    let mut expected: Vec<crate::hooks::ExpectedPatch> = Vec::with_capacity(entries.len());
    for declaration in entries {
        if declaration.module.is_null()
            || declaration.before.is_null()
            || declaration.before_len == 0
            || declaration.before_len > 4096
            || (declaration.after_len != 0 && declaration.after.is_null())
            || (declaration.after_len != 0 && declaration.after_len != declaration.before_len)
            || (declaration.after_len == 0 && !declaration.after.is_null())
        {
            return Err("contract entry has invalid byte spans".into());
        }
        if !matches!(
            declaration.kind,
            PATCH_KIND_ENTRY | PATCH_KIND_BYTES | PATCH_KIND_CALL
        ) {
            return Err(format!(
                "contract entry has unknown patch kind {}",
                declaration.kind
            ));
        }
        let module = unsafe { CStr::from_ptr(declaration.module) }
            .to_str()
            .map_err(|_| "contract module name is not UTF-8")?;
        if !crate::manifest::safe_dll(module) {
            return Err(format!("contract module `{module}` is not a DLL basename"));
        }
        let base = unsafe { (api.module_base)(declaration.module) } as usize;
        if base == 0 {
            return Err(format!("contract module `{module}` is not loaded"));
        }
        let module_size = unsafe { (api.module_size)(base as *mut core::ffi::c_void) };
        let end = declaration
            .rva
            .checked_add(declaration.before_len)
            .ok_or_else(|| format!("contract range in `{module}` overflows"))?;
        if end > module_size {
            return Err(format!(
                "contract range `{module}+{:#x}` is outside the loaded module",
                declaration.rva
            ));
        }
        let target = base.checked_add(declaration.rva).ok_or_else(|| {
            format!(
                "contract address `{module}+{:#x}` overflows",
                declaration.rva
            )
        })?;
        if expected.iter().any(|entry| entry.target == target) {
            return Err(format!(
                "contract declares `{module}+{:#x}` twice",
                declaration.rva
            ));
        }
        let before = unsafe {
            core::slice::from_raw_parts(declaration.before, declaration.before_len).to_vec()
        };
        let after = (declaration.after_len != 0).then(|| unsafe {
            core::slice::from_raw_parts(declaration.after, declaration.after_len).to_vec()
        });
        expected.push(crate::hooks::ExpectedPatch {
            target,
            kind: declaration.kind,
            before,
            after,
        });
    }
    Ok(expected)
}

/// Stop every plugin, newest first, and take its hooks out. Nothing calls this
/// in a shipping run, which unloads one plugin at a time
/// (`lifecycle::unload`); it is what the in-process test host uses. DLLs stay
/// mapped.
#[cfg(feature = "test-host")]
pub fn unload() {
    for plugin in crate::lifecycle::loaded().into_iter().rev() {
        if let Some(stop) = plugin.stop {
            unsafe { stop() };
        }
        crate::services::remove(plugin.owner);
        let (removed, failed) = crate::hooks::remove_owned_report(plugin.owner);
        if removed > 0 {
            crate::log::info(&format!(
                "stopped {} and removed {removed} hook(s)",
                plugin.name
            ));
        }
        if failed > 0 {
            crate::log::error(&format!(
                "{}: {failed} span(s) could not be restored; bookkeeping and storage retained",
                plugin.name
            ));
        }
        crate::lifecycle::forget(plugin.owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Version;

    #[test]
    fn a_version_that_differs_from_the_manifest_only_warns() {
        let declared = Version::parse("0.6.0").unwrap();
        assert_eq!(version_disagreement("0.6.0", declared), None);
        assert_eq!(version_disagreement("0.6", declared), None);
        let note = version_disagreement("0.6.1", declared).unwrap();
        assert!(
            note.contains("`0.6.1`") && note.ends_with("using 0.6.0"),
            "{note}"
        );
        assert!(version_disagreement("dev", declared).is_some());
    }
}
