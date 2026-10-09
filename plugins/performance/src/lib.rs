//! Performance tuning for the engine's render and main-thread hot spots.
//!
//! Each module replaces one engine routine with one that computes the same
//! result for less work, or relaxes one engine choice, found by signature in
//! the module it patches (`galileo.dll` or `world2.dll`). None changes what
//! the simulation computes, so all are safe in multiplayer. Every setting
//! lives in `[defiance.performance]` and is read once, at startup: the hooks
//! are not undone while the game runs, so a change takes effect on restart.
//!
//! The plugin depends on Core for its build checks and for its place in the
//! load order; its writes are plain byte plans ([`plan`]) that it applies
//! under its own ownership.

use core::ffi::{c_char, c_void, CStr};
use defiance_api::{Api, PatchContractV1, Plugin, ABI_VERSION};

mod affinity;
mod grass;
mod inverse;
mod mesh_sort;
/// The byte-plan primitives Core's own hooks use, shared as one source file.
#[path = "../../core/src/plan.rs"]
mod plan;
#[cfg(feature = "render-profile")]
mod render_profile;
mod shadow;
mod shadow_fit;
mod sites;
mod tree_sway;
mod view_sort;

static NAME: &[u8] = b"defiance.performance\0";
static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

/// The section that holds this plugin's settings.
const SECTION: &str = "defiance.performance";

/// One of this plugin's settings as text, lower case; empty when unset.
fn setting(api: &Api, key: &str) -> String {
    let section: Vec<u8> = SECTION.bytes().chain(core::iter::once(0)).collect();
    let name: Vec<u8> = key.bytes().chain(core::iter::once(0)).collect();
    let value = unsafe {
        (api.config_get)(
            section.as_ptr() as *const c_char,
            name.as_ptr() as *const c_char,
        )
    };
    if value.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .to_ascii_lowercase()
}

pub(crate) fn say(api: &Api, level: u32, message: &str) {
    let text = std::ffi::CString::new(message.replace('\0', "")).unwrap_or_default();
    unsafe { (api.log)(level, text.as_ptr()) };
}

/// The indices of `patterns`, those that match once in the live bytes of the
/// module at `base` first, the rest after in their own order.
///
/// A feature with one signature per build tries them in this order, so the
/// loader's `find_pattern`, which warns on every miss, reaches the build's
/// own signature first. The scan is silent and reads the live bytes; a
/// signature another plugin has hooked over misses here and is still tried,
/// against the original bytes, among the rest.
pub(crate) fn matching_first(base: *mut c_void, size: usize, patterns: &[&str]) -> Vec<usize> {
    let image = if base.is_null() || size == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(base as *const u8, size) }
    };
    let (mut hits, misses): (Vec<usize>, Vec<usize>) = (0..patterns.len()).partition(|&i| {
        defiance_core::pattern::parse(patterns[i])
            .is_ok_and(|pattern| defiance_core::scan::scan(image, &pattern).len() == 1)
    });
    hits.extend(misses);
    hits
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = (unsafe { api.as_ref() }) else {
        return 1;
    };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    affinity::install(api, &setting(api, "main_thread_cpus"));
    grass::install(api, &setting(api, "grass_sort"));
    inverse::install(api, &setting(api, "matrix_inverse"));
    view_sort::install(api, &setting(api, "view_sort"));
    mesh_sort::install(api, &setting(api, "mesh_sort"));
    shadow::install(api, &setting(api, "shadow_cascades"));
    shadow_fit::install(api, &setting(api, "shadow_fit"));
    tree_sway::install(api, &setting(api, "tree_sway"));
    #[cfg(feature = "render-profile")]
    render_profile::install(api);
    0
}

/// Every write the installers plan for the current configuration, read from
/// the original view: the same before and after init, and resolving it
/// writes nothing. An installer whose plan does not resolve logs a warning and
/// writes nothing, so it contributes nothing; a resolved plan whose contract
/// cannot be read fails the whole contract.
fn collect_contract(api: &Api) -> Result<Vec<defiance_feature_sdk::contract::Patch>, String> {
    #[allow(unused_mut)]
    let mut plans = vec![
        affinity::plan(api, &setting(api, "main_thread_cpus")),
        grass::plan(api, &setting(api, "grass_sort")),
        inverse::plan(api, &setting(api, "matrix_inverse")),
        view_sort::plan(api, &setting(api, "view_sort")),
        mesh_sort::plan(api, &setting(api, "mesh_sort")),
        shadow_fit::plan(api, &setting(api, "shadow_fit")),
        shadow::plan(api, &setting(api, "shadow_cascades")),
        tree_sway::plan(api, &setting(api, "tree_sway")),
    ];
    #[cfg(feature = "render-profile")]
    plans.push(render_profile::plan(api));
    let mut patches = Vec::new();
    for plan in plans.iter().flatten() {
        patches.extend(plan::contract(api, plan)?);
    }
    Ok(patches)
}

#[no_mangle]
pub unsafe extern "C" fn defiance_patch_contract_v1(api: *const Api) -> *const PatchContractV1 {
    let Some(host) = (unsafe { api.as_ref() }) else {
        return core::ptr::null();
    };
    if host.abi_version != ABI_VERSION || host.reserved != 0 {
        return core::ptr::null();
    }
    match collect_contract(host) {
        Ok(patches) => unsafe { defiance_feature_sdk::contract::build(api, patches) },
        Err(_) => core::ptr::null(),
    }
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: NAME.as_ptr().cast(),
        version: VERSION.as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();
