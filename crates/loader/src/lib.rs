//! The loader: a mod host that runs inside the game.
//!
//! This is the native answer to BepInEx's two halves. The bootstrap is a proxy
//! DLL (see `bootstrap.rs` and `proxy.rs`): it is named after a system DLL the
//! game already imports, so the game loads it for us, forwards that DLL's
//! exports to the real one, and starts the host on a thread. The host
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
mod json;
mod log;
mod manifest;
mod multiplayer;
mod plan;
mod plugin;
mod proxy;
mod proxy_generated;
mod resolve;
mod rtti;
mod services;
mod threads;
mod trace;
mod win;

/// In-process regression harness; not enabled in the shipped proxy build.
#[cfg(feature = "test-host")]
pub mod test_host {
    pub use crate::hooks::{begin_plugin, end_plugin, remove_owned, remove_owned_report};
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
