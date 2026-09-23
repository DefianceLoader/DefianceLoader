//! Loading a planned plugin DLL, and taking it back out.
//!
//! Identity checks, the feature-mask and crash handshakes, the service
//! extension handshake and the hook/service lifecycle live here, apart from the
//! host's startup orchestration in `host.rs`. A plugin that fails has its
//! `stop` called and its hooks removed; the DLL itself is left loaded, since
//! pulling it out from under anything it started is worse.

use crate::plan::Planned;
use crate::win;
use core::ffi::CStr;
use defiance_api::{Api, Entry, ABI_VERSION, PLUGIN_ENTRY};
#[cfg(feature = "test-host")]
use std::sync::Mutex;

/// Plugins that initialised, for `unload` to stop in reverse. Only the
/// in-process test host unloads, so a shipping run does not record them.
#[cfg(feature = "test-host")]
struct Loaded {
    owner: usize,
    name: String,
    stop: Option<unsafe extern "C" fn()>,
}

#[cfg(feature = "test-host")]
static LOADED: Mutex<Vec<Loaded>> = Mutex::new(Vec::new());

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
    let wide = win::wide(&node.path.to_string_lossy());
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
    #[cfg(feature = "test-host")]
    LOADED.lock().unwrap().push(Loaded {
        owner,
        name,
        stop: plugin.stop,
    });
    Ok(())
}

/// Stop every plugin, newest first, and take its hooks out. Nothing calls this
/// in a shipping run — the loader lives as long as the process — but it is what
/// the in-process test host uses. DLLs and published stubs stay mapped: a real
/// unload would additionally need to wait for all in-flight plugin calls.
#[cfg(feature = "test-host")]
pub fn unload() {
    let plugins: Vec<Loaded> = std::mem::take(&mut *LOADED.lock().unwrap());
    for plugin in plugins.into_iter().rev() {
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
    }
}
