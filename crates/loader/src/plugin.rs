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
use defiance_api::{Api, Entry, ABI_VERSION, PLUGIN_ENTRY};

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
pub fn load(
    node: &Planned,
    api: &'static Api,
    owner: usize,
    feature_mask: u64,
) -> Result<(), LoadFailure> {
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
        let matches = crate::manifest::Version::parse(&version)
            .map(|parsed| parsed == manifest.version)
            .unwrap_or_else(|_| version == manifest.version.to_string());
        if !matches {
            return Err(LoadFailure::plain(format!(
                "exports version `{version}` but its manifest declares `{}`",
                manifest.version
            )));
        }
    }
    // The core plugin's internal handshake: tell it which features the accepted
    // plan will install, so it validates only those sites. Optional, so a plugin
    // that does not export it is unaffected.
    let configure =
        unsafe { win::GetProcAddress(module, b"defiance_configure_enabled_v1\0".as_ptr()) };
    if !configure.is_null() {
        let configure: unsafe extern "C" fn(u64) = unsafe { core::mem::transmute(configure) };
        unsafe { configure(feature_mask) };
    }
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
        crate::log::info(&format!("build sha256={hash} {}", node.path.display()));
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
    crate::services::begin(
        owner,
        &name,
        node.manifest
            .as_ref()
            .map(|m| m.depends.iter().map(|d| d.id.clone()).collect())
            .unwrap_or_default(),
    );
    // Hooks installed during this call are charged to this plugin.
    crate::hooks::begin_plugin(owner, &name);
    let code = unsafe { (plugin.init)(api as *const Api) };
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
            reason: format!("init returned {code}"),
            degraded: failed > 0,
        });
    }
    crate::lifecycle::record(crate::lifecycle::Loaded {
        owner,
        id: node.id.clone(),
        name,
        path: node.path.clone(),
        reloadable: shadow != node.path && node.manifest.as_ref().is_some_and(|m| m.hot_reload),
        shadow,
        module: module as usize,
        code: crate::lifecycle::code_ranges(module as usize),
        stop: plugin.stop,
        feature: node.builtin.map_or(0, |builtin| builtin.feature),
        stamp: crate::lifecycle::stamp(&node.path),
        multiplayer_safe: crate::plan::multiplayer_safe(node),
    });
    Ok(())
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
