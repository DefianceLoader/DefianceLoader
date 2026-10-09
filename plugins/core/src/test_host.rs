use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::{GetCurrentProcessId, Module, Pool, Runtime, RUNTIME};
use defiance_api::PatchV1;
use defiance_core::apply::Target;

static BUILD: &std::ffi::CStr = c"synthetic";

/// Exposes Core's patch service and a synthetic runtime to the service harness.
///
/// # Safety
/// Both bases must point to writable memory that remains mapped for this process.
pub unsafe fn initialize_runtime(
    logic_base: *mut u8,
    logic_sha: &str,
    logic_image: Vec<u8>,
    game_base: *mut u8,
    game_sha: &str,
    game_image: Vec<u8>,
) -> Result<(), String> {
    let module = |base: *mut u8, sha: &str, image: Vec<u8>| {
        if base.is_null() || image.is_empty() {
            return Err("synthetic module has no mapped image".to_string());
        }
        let target = Target {
            process_id: unsafe { GetCurrentProcessId() },
            base,
            size: image.len(),
            path: PathBuf::new(),
        };
        Ok(Module {
            base: base as usize,
            sha: sha.to_string(),
            pristine: Some(image),
            pool: Pool::near(&target)?,
        })
    };
    let runtime = Runtime {
        logic: module(logic_base, logic_sha, logic_image)?,
        game: module(game_base, game_sha, game_image)?,
        trace_ring: 0,
        build: BUILD,
        known_build: true,
        linked: Mutex::new(BTreeMap::new()),
        crash_ranges: None,
        lobby_connect: None,
        tactical_state: None,
    };
    RUNTIME
        .set(runtime)
        .map_err(|_| "synthetic runtime is already initialized".to_string())
}

pub fn patch_api() -> &'static PatchV1 {
    &crate::patch::API
}
