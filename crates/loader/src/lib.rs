//! The loader: a mod host that runs inside the game.
//!
//! This is the native answer to BepInEx's two halves. The bootstrap can be a
//! proxy DLL (see `bootstrap.rs` and `proxy.rs`): it is named after a system
//! DLL the game already imports, so the game loads it for us, forwards that
//! DLL's exports to the real one, and starts the host on a thread. An EXE can
//! also load the neutral DLL directly and inspect `DEFIANCE_LOADER_STATE` to
//! learn when the host finishes startup. The host
//! (`host.rs`) waits for the game's own modules, builds the plugin API
//! (`resolve.rs`, `hooks.rs`, `rtti.rs`) and loads every plugin in `plugins/`.
//!
//! What the injector does with fixed, assembled descriptors, a plugin does at
//! runtime through the API: find a site by signature, install a detour, call
//! the trampoline for the stock behaviour, and look a class and method up by
//! name. The machinery underneath is the same — a trampoline copied from the
//! target's own bytes, a rel32 or an absolute jump, pages committed near the
//! module so the branch can reach — but the addresses are resolved when the
//! game starts, not baked into a build.
//!
//! Nothing here is compiled on a target other than Windows; the crate is empty
//! elsewhere so a workspace build still succeeds.

#![cfg(windows)]

mod bootstrap;
mod code;
pub mod config;
pub mod crash;
mod hooks;
mod host;
use defiance_core::json;
mod lifecycle;
mod log;
mod manifest;
mod multiplayer;
mod near;
mod original;
mod plan;
mod plugin;
mod plugin_log;
mod proxy;
mod proxy_generated;
mod reload;
mod resolve;
mod rtti;
mod services;
mod session;
mod startup;
mod threads;
mod trace;
mod win;

/// In-process regression harness; not enabled in the shipped proxy build.
#[cfg(feature = "test-host")]
pub mod test_host {
    pub use crate::hooks::{
        begin_plugin, end_plugin, installed, remove_owned, remove_owned_report,
    };
    pub use crate::resolve::build_api;
    pub use crate::services::{
        begin as begin_services, finish as finish_services, remove as remove_services,
    };
    pub fn service_api() -> &'static defiance_api::ServiceApiV1 {
        &crate::services::API
    }
    /// Actual discovery, configuration, planner and DLL initialization, without
    /// waiting for game modules. Used by the game-independent author examples.
    pub fn run_plugins(exe_dir: &std::path::Path) -> Vec<(String, String)> {
        crate::host::test_plugins(exe_dir)
    }
    /// Break initialization-order ties by descending plugin ID instead of
    /// ascending, for runs after this call.
    pub fn reverse_ties(on: bool) {
        crate::plan::REVERSE_TIES.store(on, std::sync::atomic::Ordering::Relaxed);
    }
    /// The IDs the last `load_plugins` planned to initialize, in order,
    /// including any whose `init` then failed.
    pub fn planned_order() -> Vec<String> {
        crate::host::TEST_ORDER.lock().unwrap().clone()
    }
    /// As `run_plugins`, leaving the plugins loaded.
    pub fn load_plugins(exe_dir: &std::path::Path) -> Vec<(String, String)> {
        crate::host::test_load(exe_dir)
    }
    /// Hot-reload one loaded plugin (and its dependants).
    pub fn reload_plugin(id: &str) -> Result<(), String> {
        crate::host::test_reload(id)
    }
    /// Apply simultaneous settled DLL and config changes through the watcher queue.
    pub fn apply_reload_and_config(file_id: &str, config_id: &str) -> Result<(), String> {
        crate::host::test_reload_and_config(file_id, config_id)
    }
    /// Load a plugin added to the plugins directory after startup.
    pub fn add_plugin(id: &str) -> Result<(), String> {
        crate::host::test_add(id)
    }
    /// Unload a plugin removed from the plugins directory.
    pub fn remove_plugin(id: &str) -> Result<(), String> {
        crate::host::test_remove(id)
    }
    /// Switch a plugin on or off, as the watcher does after its config file's
    /// `enabled` changed.
    pub fn toggle_plugin(id: &str, on: bool) -> Result<(), String> {
        crate::host::test_toggle(id, on)
    }
    /// Load a plugin again with its settings as its config file says now, as
    /// the watcher does after they changed.
    pub fn resettle_plugin(id: &str) -> Result<(), String> {
        crate::host::test_resettle(id)
    }
    /// The IDs of the old copies a reload kept mapped.
    pub fn retained_plugins() -> Vec<String> {
        crate::lifecycle::retained()
            .into_iter()
            .map(|plugin| plugin.id)
            .collect()
    }
    /// The loaded plugins' IDs and owner numbers, in load order.
    pub fn loaded_plugins() -> Vec<(String, usize)> {
        crate::lifecycle::loaded()
            .into_iter()
            .map(|plugin| (plugin.id, plugin.owner))
            .collect()
    }
}

/// Tooling surface: the built-in manifests, generated from the single
/// authoritative table in `config::builtin`. Used by `tools/manifest-gen` so
/// the packaged manifests and the runtime registry cannot drift apart.
pub fn builtin_manifests() -> Vec<(String, String)> {
    manifest::builtin_manifests()
}

/// The sidecar filename for a DLL basename, from the single filename
/// interpretation discovery, configuration and staging all share.
pub fn sidecar_name(dll: &str) -> String {
    manifest::sidecar_name(dll)
}
