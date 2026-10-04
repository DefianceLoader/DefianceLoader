//! The host: what runs once the game is up, on the thread `bootstrap` starts.
//!
//! It waits for `logic.dll` and `game.dll` (a proxy DLL is in the process
//! before they are), builds the `Api` once and leaks it so the pointer stays
//! valid for the process, then plans initialization from the manifests
//! configuration loading found ([`crate::config::Snapshot::catalog`]) and loads
//! the planned plugins in dependency order.
//!
//! Discovery reads sidecar manifests rather than executing DLLs, so a disabled
//! or blocked managed plugin is never loaded. A plugin whose exported identity,
//! ABI or version disagrees with its manifest is refused before `init`.
//!
//! Hooks belong to the plugin whose `init` requests them. Managed requests are
//! staged and preflighted as a transaction before the next legacy initializer;
//! a manifest dependency can place a legacy plugin between managed groups.

use crate::plan::{Decision, Planned};
#[cfg(feature = "test-host")]
use crate::plugin::unload;
use crate::plugin::{load, LoadFailure};
use crate::win;
use defiance_api::Api;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The game executable. A helper process has no `logic.dll` to patch.
const GAME_EXE: &str = "trm.exe";

/// One host per process, however many times the proxy is attached.
static STARTED: AtomicBool = AtomicBool::new(false);

/// Arm `[trace] sites`, if any. Diagnostic only: a bad value is logged and
/// startup goes on without tracing.
fn start_trace(config: &crate::config::Snapshot) {
    use crate::config::builtin::TRACE_SECTION;
    let text = config.text(TRACE_SECTION, "sites").unwrap_or_default();
    let sites = match crate::trace::parse(&text) {
        Ok(sites) if sites.is_empty() => return,
        Ok(sites) => sites,
        Err(e) => {
            crate::log::warn(&format!("trace: {e}; not tracing"));
            return;
        }
    };
    let mut addresses = Vec::new();
    for site in &sites {
        let wide = crate::win::wide(&site.module);
        let base = unsafe { crate::win::GetModuleHandleW(wide.as_ptr()) } as usize;
        if base == 0 {
            crate::log::warn(&format!(
                "trace: {} is not loaded; not tracing",
                site.module
            ));
            return;
        }
        let address = base + site.rva;
        if !crate::win::is_executable(address) {
            crate::log::warn(&format!(
                "trace: {}+{:#x} is not code; not tracing",
                site.module, site.rva
            ));
            return;
        }
        addresses.push((address, format!("{}+{:#x}", site.module, site.rva)));
    }
    let hits = config
        .integer(TRACE_SECTION, "hits")
        .unwrap_or(20)
        .clamp(1, 1000) as u32;
    if config.text(TRACE_SECTION, "when").as_deref() == Some("mission") {
        let labels: Vec<&str> = addresses.iter().map(|(_, label)| label.as_str()).collect();
        crate::log::info(&format!(
            "trace: {} armed when the first mission loads, {hits} hits each",
            labels.join(", ")
        ));
        crate::trace::defer(addresses, hits);
        return;
    }
    match crate::trace::start(&addresses, hits, crate::log::info) {
        Ok(()) => crate::log::info(&format!(
            "trace: {} armed on every thread, {hits} hits each",
            addresses
                .iter()
                .map(|(_, label)| label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Err(e) => crate::log::warn(&format!("trace: {e}; not tracing")),
    }
}

pub fn run() {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    // The proxy is loaded into every process that imports it, and the embedded
    // browser's helpers are among them. Only the game should act, and the rest
    // should do nothing at all, not even open the log, or their entries would
    // sit in the same file as the game's and read like repeated starts.
    if !is_game_process() {
        return;
    }
    // The patching core's progress notes go to the same log.
    defiance_core::report::set(Box::new(crate::log::info as fn(&str)));
    let config = crate::config::load();
    match crate::crash::initialize(
        &config.paths.log_dir.join("crashes"),
        &config.paths.exe_dir.join("defiance-crash-helper.exe"),
    ) {
        Ok(stem) => {
            crate::log::info(&format!("crash reports: {}.*", stem.display()));
            crate::crash::note(&config.report());
            for name in [GAME_EXE, crate::proxy::REAL, "logic.dll", "game.dll"] {
                let path = config.paths.exe_dir.join(name);
                if let Ok(hash) = defiance_core::sha256::file(&path) {
                    crate::log::info(&format!("build sha256={hash} {}", path.display()));
                }
            }
            let unavailable = crate::proxy::unavailable();
            if !unavailable.is_empty() {
                crate::log::info(&format!(
                    "{}: not provided by this Windows, calls return E_NOTIMPL: {}",
                    crate::proxy::REAL,
                    unavailable.join(", ")
                ));
            }
        }
        Err(e) => crate::log::warn(&format!("crash reporting unavailable: {e}")),
    }
    crate::log::configure(config);
    if let Some(reason) = config.startup_error() {
        crate::log::error(&reason);
        return;
    }
    let wait = config
        .integer(crate::config::builtin::LOADER_SECTION, "wait")
        .map(|seconds| Duration::from_secs(seconds.max(0) as u64))
        .unwrap_or(Duration::from_secs(60));
    let allow_unknown_build = config
        .get(
            crate::config::builtin::LOADER_SECTION,
            "allow_unknown_build",
        )
        .and_then(|resolved| resolved.value.as_bool())
        .unwrap_or(false);
    crate::log::info(&format!(
        "game directory {}; unknown builds {}; plugins from {}",
        config.paths.exe_dir.display(),
        if allow_unknown_build {
            "allowed"
        } else {
            "refused"
        },
        config.paths.plugin_dir.display()
    ));
    for name in ["logic.dll", "game.dll"] {
        match crate::resolve::wait_for(name, wait) {
            Some((base, size)) => {
                crate::log::debug(&format!("{name} at {base:p}, {size:#x} bytes"))
            }
            None => {
                crate::log::error(&format!(
                    "{name} did not load within {}s; no plugins installed",
                    wait.as_secs()
                ));
                return;
            }
        }
    }
    start_trace(&config);

    // Leaked on purpose: a plugin may keep this pointer for the process's life.
    let api: &'static Api = Box::leak(Box::new(crate::resolve::build_api()));
    // Shadow copies of earlier runs; this run makes its own.
    crate::lifecycle::clean_cache(&config.paths.root.join("cache").join("plugins"));

    // The scan configuration loading made, so the plan and the settings agree.
    for warning in &config.catalog.warnings {
        crate::log::warn(warning);
    }
    let plan = crate::plan::plan(config.catalog.entries.clone(), config);
    for node in &plan.nodes {
        // Provenance and manifest path for diagnostics: where the effective
        // enablement came from, and which sidecar declared the plugin.
        let provenance = config
            .provenance(&node.id, "enabled")
            .map(|origin| format!(" [{origin}]"))
            .unwrap_or_default();
        let manifest = if node.manifest.is_some() {
            format!(
                "; manifest {}",
                config
                    .paths
                    .plugin_dir
                    .join(crate::manifest::sidecar_name(&node.dll))
                    .display()
            )
        } else {
            String::new()
        };
        match &node.decision {
            Decision::Initialize => {
                let version = node
                    .manifest
                    .as_ref()
                    .map(|manifest| manifest.version.to_string())
                    .unwrap_or_else(|| "legacy".into());
                crate::log::debug(&format!(
                    "{} {version} scheduled{}{}{manifest}{provenance}",
                    node.id,
                    if node.legacy {
                        " (legacy, no manifest)"
                    } else {
                        ""
                    },
                    if node.manifest.is_some() {
                        format!("; group {}", node.manifest.as_ref().unwrap().group)
                    } else {
                        String::new()
                    }
                ));
            }
            Decision::Disabled { reason } => {
                crate::log::warn(&format!("{} disabled: {reason}{provenance}", node.id))
            }
            Decision::Blocked { reason } => {
                crate::log::error(&format!("{} blocked: {reason}{provenance}", node.id))
            }
            Decision::Ignored { reason } => {
                crate::log::warn(&format!("{} ignored: {reason}", node.id))
            }
        }
    }

    let result = run_plan(
        &plan,
        |node, index| load(node, api, index),
        crate::multiplayer::guarded,
    );
    for (node, state) in plan.nodes.iter().zip(&result.states) {
        match state {
            RunState::Active => crate::log::debug(&format!("{} active", node.id)),
            RunState::Blocked(reason) => {
                crate::log::error(&format!("{} blocked: {reason}", node.id))
            }
            RunState::Failed(reason) => crate::log::error(&format!("{} failed: {reason}", node.id)),
            _ => {}
        }
    }
    let active: Vec<bool> = result
        .states
        .iter()
        .map(|state| *state == RunState::Active)
        .collect();
    crate::multiplayer::record(&crate::multiplayer::blockers(&plan, &active));
    let summary = result.summary();
    if result.degraded {
        crate::log::error(&format!(
            "startup degraded: {summary}; rollback incomplete, no further plugins initialized"
        ));
        crate::startup::DEFIANCE_LOADER_STATE.fail();
    } else {
        crate::log::info(&format!("startup summary: {summary}"));
        crate::startup::DEFIANCE_LOADER_STATE.ready();
    }
    crate::log::info("loader ready");
    let flag = |key: &str| {
        config
            .get(crate::config::builtin::LOADER_SECTION, key)
            .and_then(|resolved| resolved.value.as_bool())
            .unwrap_or(false)
    };
    let (develop, toggle) = (flag("hot_reload"), flag("live_toggle"));
    if develop || toggle {
        crate::reload::start_watcher(api, develop, toggle);
    }
}

/// This is the production executor. Tests replace only the DLL operation;
/// planning, prerequisite checks, failure propagation and counts are shared.
#[derive(Debug, PartialEq, Eq)]
enum RunState {
    Active,
    Disabled,
    Blocked(String),
    Ignored,
    Failed(String),
}

struct StartupResult {
    states: Vec<RunState>,
    degraded: bool,
}

impl StartupResult {
    fn summary(&self) -> String {
        let mut counts = [0usize; 5];
        for state in &self.states {
            counts[match state {
                RunState::Active => 0,
                RunState::Disabled => 1,
                RunState::Blocked(_) => 2,
                RunState::Ignored => 3,
                RunState::Failed(_) => 4,
            }] += 1;
        }
        format!(
            "{} active, {} disabled, {} blocked, {} ignored, {} failed",
            counts[0], counts[1], counts[2], counts[3], counts[4]
        )
    }
}

/// The plan without the multiplayer guard's gate, for tests of the rest.
#[cfg(test)]
fn execute_plan(
    plan: &crate::plan::Plan,
    initialize: impl FnMut(&Planned, usize) -> Result<(), LoadFailure>,
) -> StartupResult {
    run_plan(plan, initialize, || true)
}

/// Initialize the plan in order, committing staged managed writes before each
/// legacy plugin and at the end. A plugin that is not multiplayer-safe starts
/// only once `guarded` says the multiplayer guard is installed; the plan orders
/// every such plugin after Core, which installs it.
fn run_plan(
    plan: &crate::plan::Plan,
    mut initialize: impl FnMut(&Planned, usize) -> Result<(), LoadFailure>,
    guarded: impl Fn() -> bool,
) -> StartupResult {
    let _patch_plan = crate::hooks::PatchPlanGuard::begin();
    let mut result = StartupResult {
        states: plan
            .nodes
            .iter()
            .map(|node| match &node.decision {
                Decision::Initialize => RunState::Blocked("not initialized".into()),
                Decision::Disabled { .. } => RunState::Disabled,
                Decision::Blocked { reason } => RunState::Blocked(reason.clone()),
                Decision::Ignored { .. } => RunState::Ignored,
            })
            .collect(),
        degraded: false,
    };
    let mut active = BTreeSet::new();
    for &index in &plan.order {
        let node = &plan.nodes[index];
        if node.legacy {
            finalize_patch_plan(plan, &mut result, &mut active);
        }
        if matches!(
            &result.states[index],
            RunState::Blocked(reason) if reason != "not initialized"
        ) {
            continue;
        }
        if result.degraded {
            result.states[index] =
                RunState::Blocked("startup stopped after incomplete rollback".into());
            continue;
        }
        if let Some(dependency) = node
            .depends
            .iter()
            .find(|id| !active.contains(&id.to_ascii_lowercase()))
        {
            result.states[index] =
                RunState::Blocked(format!("required `{dependency}` is not active"));
            continue;
        }
        if !crate::plan::multiplayer_safe(node) && !guarded() {
            result.states[index] = RunState::Blocked(
                "the multiplayer guard is not installed (defiance.core); \
                 plugins that are not multiplayer-safe need it"
                    .into(),
            );
            continue;
        }
        match initialize(node, index) {
            Ok(()) => {
                active.insert(node.id.to_ascii_lowercase());
                result.states[index] = RunState::Active;
            }
            Err(failure) => {
                result.degraded = failure.degraded;
                result.states[index] = RunState::Failed(failure.reason);
            }
        }
    }
    finalize_patch_plan(plan, &mut result, &mut active);
    result
}

/// Resolve the current staged managed writes after their initializers run. The
/// first owner in plan order wins an overlap; later owners and transitive
/// dependants are stopped before the surviving transaction is published.
fn finalize_patch_plan(
    plan: &crate::plan::Plan,
    result: &mut StartupResult,
    active: &mut BTreeSet<String>,
) {
    let owners: Vec<usize> = plan
        .order
        .iter()
        .copied()
        .filter(|&index| !plan.nodes[index].legacy && result.states[index] == RunState::Active)
        .collect();
    if result.degraded {
        let reason = "startup abandoned staged writes after an incomplete plugin rollback";
        for &owner in owners.iter().rev() {
            if let Err(error) = crate::lifecycle::discard_started(owner) {
                crate::log::error(&format!(
                    "{}: could not discard staged patches: {error}",
                    plan.nodes[owner].id
                ));
            }
            active.remove(&plan.nodes[owner].id.to_ascii_lowercase());
            result.states[owner] = RunState::Blocked(reason.into());
        }
        crate::hooks::discard_staged(&owners);
        return;
    }

    let conflicts = crate::hooks::staged_conflicts();
    let mut accepted = BTreeSet::new();
    let mut refused = BTreeMap::new();
    for &owner in &owners {
        let matching: Vec<&crate::hooks::PatchConflict> = conflicts
            .iter()
            .filter(|conflict| {
                let other = if conflict.first.owner == owner {
                    Some(&conflict.second)
                } else if conflict.second.owner == owner {
                    Some(&conflict.first)
                } else {
                    None
                };
                other.is_some_and(|side| {
                    side.owner != owner && (side.published || accepted.contains(&side.owner))
                })
            })
            .collect();
        if matching.is_empty() {
            accepted.insert(owner);
            continue;
        }
        let reason = format!(
            "unified patch conflict: {}",
            crate::hooks::describe_conflict(matching[0])
        );
        for conflict in matching {
            crate::log::error(&format!(
                "unified patch plan: refusing {}: {}",
                plan.nodes[owner].id,
                crate::hooks::describe_conflict(conflict)
            ));
        }
        refused.insert(owner, reason);
    }

    let mut blocked = refused.clone();
    loop {
        let mut changed = false;
        for &owner in &owners {
            if blocked.contains_key(&owner) {
                continue;
            }
            let dependency = plan.nodes[owner].depends.iter().find(|dependency| {
                plan.nodes
                    .iter()
                    .position(|node| node.id.eq_ignore_ascii_case(dependency))
                    .is_some_and(|provider| blocked.contains_key(&provider))
            });
            if let Some(dependency) = dependency {
                blocked.insert(
                    owner,
                    format!("required `{dependency}` was refused by the unified patch plan"),
                );
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for &owner in plan.order.iter().rev() {
        let Some(reason) = blocked.get(&owner) else {
            continue;
        };
        if result.states[owner] != RunState::Active {
            continue;
        }
        if let Err(error) = crate::lifecycle::discard_started(owner) {
            result.degraded = true;
            crate::log::error(&format!(
                "{}: could not roll back the refused plugin: {error}",
                plan.nodes[owner].id
            ));
        }
        active.remove(&plan.nodes[owner].id.to_ascii_lowercase());
        result.states[owner] = if refused.contains_key(&owner) {
            RunState::Failed(reason.clone())
        } else {
            RunState::Blocked(reason.clone())
        };
    }
    accepted.retain(|owner| !blocked.contains_key(owner));

    if result.degraded {
        for &owner in owners.iter().rev() {
            if result.states[owner] != RunState::Active {
                continue;
            }
            if let Err(error) = crate::lifecycle::discard_started(owner) {
                crate::log::error(&format!(
                    "{}: could not discard the remaining staged plan: {error}",
                    plan.nodes[owner].id
                ));
            }
            active.remove(&plan.nodes[owner].id.to_ascii_lowercase());
            result.states[owner] =
                RunState::Blocked("startup stopped after an incomplete patch-plan rollback".into());
        }
        crate::hooks::discard_staged(&owners);
        return;
    }

    let mut discarded: BTreeSet<usize> = conflicts
        .iter()
        .flat_map(|conflict| [&conflict.first, &conflict.second])
        .filter(|side| !side.published && !accepted.contains(&side.owner))
        .map(|side| side.owner)
        .collect();
    discarded.extend(
        owners
            .iter()
            .copied()
            .filter(|owner| !accepted.contains(owner)),
    );
    crate::hooks::discard_staged(&discarded.iter().copied().collect::<Vec<_>>());

    let commit_owners: Vec<usize> = accepted.iter().copied().collect();
    match crate::hooks::commit_staged(&commit_owners) {
        Ok(()) => crate::lifecycle::commit_pending(&commit_owners),
        Err(crate::code::CommitError::AfterWrite(error)) => {
            crate::lifecycle::commit_pending(&commit_owners);
            result.degraded = true;
            crate::log::error(&format!(
                "startup degraded: the unified patch transaction may be partial; ownership and code were retained: {error}"
            ));
        }
        Err(crate::code::CommitError::BeforeWrite(error)) => {
            crate::log::error(&format!(
                "unified patch plan was refused before publication: {error}"
            ));
            for &owner in owners.iter().rev() {
                if result.states[owner] != RunState::Active {
                    continue;
                }
                if let Err(cleanup) = crate::lifecycle::discard_started(owner) {
                    result.degraded = true;
                    crate::log::error(&format!(
                        "{}: cleanup after a refused patch plan failed: {cleanup}",
                        plan.nodes[owner].id
                    ));
                }
                active.remove(&plan.nodes[owner].id.to_ascii_lowercase());
                result.states[owner] = RunState::Failed(format!(
                    "unified patch transaction was not published: {error}"
                ));
            }
            crate::hooks::discard_staged(&owners);
        }
    }
}

/// Whether this process's executable is the game.
fn is_game_process() -> bool {
    let mut buffer = vec![0u16; 1024];
    let length = unsafe {
        win::GetModuleFileNameW(
            core::ptr::null_mut(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    let path = PathBuf::from(win::from_wide(&buffer[..length.min(buffer.len())]));
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(GAME_EXE))
}

#[cfg(feature = "test-host")]
pub fn test_plugins(exe_dir: &std::path::Path) -> Vec<(String, String)> {
    let states = test_load(exe_dir);
    unload();
    states
}

/// The test host's `Api`, kept for reloads.
#[cfg(feature = "test-host")]
static TEST_API: std::sync::OnceLock<&'static Api> = std::sync::OnceLock::new();

/// The IDs the last [`test_load`] planned to initialize, in order, including
/// any whose `init` then failed.
#[cfg(feature = "test-host")]
pub static TEST_ORDER: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Startup as the host runs it, without waiting for game modules, leaving the
/// plugins loaded.
#[cfg(feature = "test-host")]
pub fn test_load(exe_dir: &std::path::Path) -> Vec<(String, String)> {
    assert_eq!(exe_dir, crate::config::game_dir());
    let config = crate::config::load();
    crate::log::configure(config);
    crate::lifecycle::clean_cache(&config.paths.root.join("cache").join("plugins"));
    let plan = crate::plan::plan(config.catalog.entries.clone(), config);
    *TEST_ORDER.lock().unwrap() = plan
        .order
        .iter()
        .map(|&index| plan.nodes[index].id.clone())
        .collect();
    let api: &'static Api =
        TEST_API.get_or_init(|| Box::leak(Box::new(crate::resolve::build_api())));
    let result = run_plan(
        &plan,
        |node, owner| load(node, api, owner),
        crate::multiplayer::guarded,
    );
    let active: Vec<bool> = result
        .states
        .iter()
        .map(|state| *state == RunState::Active)
        .collect();
    crate::multiplayer::record(&crate::multiplayer::blockers(&plan, &active));
    plan.nodes
        .iter()
        .zip(result.states)
        .map(|(node, state)| (node.id.clone(), format!("{state:?}")))
        .collect()
}

/// A hot reload, as the watcher runs it.
#[cfg(feature = "test-host")]
pub fn test_reload(id: &str) -> Result<(), String> {
    crate::reload::reload(TEST_API.get().expect("test_load first"), id)
}

#[cfg(feature = "test-host")]
pub fn test_reload_and_config(file_id: &str, config_id: &str) -> Result<(), String> {
    crate::reload::test_reload_and_config(
        TEST_API.get().expect("test_load first"),
        file_id,
        config_id,
    )
}

/// An added plugin loaded, as the watcher does it.
#[cfg(feature = "test-host")]
pub fn test_add(id: &str) -> Result<(), String> {
    crate::reload::add(TEST_API.get().expect("test_load first"), id)
}

/// A removed plugin unloaded, as the watcher does it.
#[cfg(feature = "test-host")]
pub fn test_remove(id: &str) -> Result<(), String> {
    crate::reload::remove(TEST_API.get().expect("test_load first"), id)
}

/// A plugin loaded again for changed settings, as the watcher does it.
#[cfg(feature = "test-host")]
pub fn test_resettle(id: &str) -> Result<(), String> {
    let api = TEST_API.get().expect("test_load first");
    crate::reload::resettle(api, id)
}

/// A plugin switched on or off, as the watcher does it.
#[cfg(feature = "test-host")]
pub fn test_toggle(id: &str, on: bool) -> Result<(), String> {
    let api = TEST_API.get().expect("test_load first");
    if on {
        crate::reload::enable(api, id)
    } else {
        crate::reload::disable(api, id)
    }
}

#[cfg(test)]
mod startup_tests {
    use super::*;
    use defiance_api::ABI_VERSION;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "defiance-startup-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(path.join("bin")).unwrap();
            std::fs::create_dir_all(path.join("DefianceLoader/plugins")).unwrap();
            Self(path)
        }
        fn plugin(&self, id: &str, depends: &[&str], conflicts: &[&str], abi: u32) {
            let dir = self.0.join("DefianceLoader/plugins");
            let deps = depends
                .iter()
                .map(|id| format!("{{\"id\":\"{id}\"}}"))
                .collect::<Vec<_>>()
                .join(",");
            let conflicts = conflicts
                .iter()
                .map(|id| format!("\"{id}\""))
                .collect::<Vec<_>>()
                .join(",");
            let json = format!(
                r#"{{"schema":1,"id":"{id}","dll":"{id}.dll","version":"1.0.0","abi":{abi},"group":"test","settings":[],"depends":[{deps}],"conflicts":[{conflicts}]}}"#
            );
            std::fs::write(
                dir.join(format!("{id}.dll")),
                b"fixture; must never reach LoadLibrary",
            )
            .unwrap();
            std::fs::write(dir.join(format!("{id}.plugin.json")), json).unwrap();
        }
        fn plan(&self) -> crate::plan::Plan {
            let config = crate::config::inspect(&self.0.join("bin"));
            crate::plan::plan(config.catalog.entries.clone(), &config)
        }
        fn config(&self, name: &str, bytes: &[u8]) {
            let dir = self.0.join("DefianceLoader/config");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(name), bytes).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// One plugin's runtime state, by stable ID.
    fn state<'a>(plan: &crate::plan::Plan, result: &'a StartupResult, id: &str) -> &'a RunState {
        let index = plan
            .nodes
            .iter()
            .position(|node| node.id == id)
            .unwrap_or_else(|| panic!("no plugin `{id}` in the plan"));
        &result.states[index]
    }

    #[test]
    fn without_the_guard_only_multiplayer_safe_plugins_start() {
        let f = Fixture::new();
        f.plugin("gameplay", &[], &[], ABI_VERSION);
        f.plugin("display", &[], &[], ABI_VERSION);
        let sidecar = f.0.join("DefianceLoader/plugins/display.plugin.json");
        let json = std::fs::read_to_string(&sidecar).unwrap();
        std::fs::write(
            &sidecar,
            json.replace(
                "\"conflicts\":[]",
                "\"conflicts\":[],\"multiplayer_safe\":true",
            ),
        )
        .unwrap();
        let plan = f.plan();
        let result = run_plan(&plan, |_, _| Ok(()), || false);
        assert_eq!(*state(&plan, &result, "display"), RunState::Active);
        assert!(
            matches!(state(&plan, &result, "gameplay"), RunState::Blocked(reason) if reason.contains("multiplayer guard")),
            "{:?}",
            state(&plan, &result, "gameplay")
        );
        let result = run_plan(&plan, |_, _| Ok(()), || true);
        assert_eq!(*state(&plan, &result, "gameplay"), RunState::Active);
    }

    #[test]
    fn only_active_plugins_without_the_flag_block_multiplayer() {
        let f = Fixture::new();
        f.plugin("gameplay", &[], &[], ABI_VERSION);
        f.plugin("display", &[], &[], ABI_VERSION);
        f.plugin("broken", &[], &[], ABI_VERSION);
        let sidecar = f.0.join("DefianceLoader/plugins/display.plugin.json");
        let json = std::fs::read_to_string(&sidecar).unwrap();
        std::fs::write(
            &sidecar,
            json.replace(
                "\"conflicts\":[]",
                "\"conflicts\":[],\"multiplayer_safe\":true",
            ),
        )
        .unwrap();
        let plan = f.plan();
        let result = execute_plan(&plan, |node, _| {
            if node.id == "broken" {
                Err(LoadFailure::plain("init returned 1"))
            } else {
                Ok(())
            }
        });
        let active: Vec<bool> = result
            .states
            .iter()
            .map(|state| *state == RunState::Active)
            .collect();
        assert_eq!(crate::multiplayer::blockers(&plan, &active), ["gameplay"]);
    }

    #[test]
    fn the_plan_uses_the_scan_configuration_made() {
        let f = Fixture::new();
        f.plugin("a", &[], &[], ABI_VERSION);
        let config = crate::config::inspect(&f.0.join("bin"));
        // A plugin that appears after configuration loaded has no settings
        // resolved for it, so startup must not plan it either.
        f.plugin("late", &[], &[], ABI_VERSION);
        let plan = crate::plan::plan(config.catalog.entries.clone(), &config);
        let ids: Vec<&str> = plan.nodes.iter().map(|node| node.id.as_str()).collect();
        assert_eq!(ids, ["a"]);
    }

    #[test]
    fn cycle_consumers_never_reach_the_dll_loader() {
        let f = Fixture::new();
        f.plugin("a", &["b"], &[], ABI_VERSION);
        f.plugin("b", &["a"], &[], ABI_VERSION);
        f.plugin("c", &["a"], &[], ABI_VERSION);
        f.plugin("d", &["c"], &[], ABI_VERSION);
        let plan = f.plan();
        let result = execute_plan(&plan, |_, _| panic!("blocked DLL loaded"));
        for id in ["a", "b"] {
            assert!(
                matches!(state(&plan, &result, id), RunState::Blocked(reason) if reason.contains("cycle")),
                "{id}: {:?}",
                state(&plan, &result, id)
            );
        }
        for id in ["c", "d"] {
            assert!(
                matches!(state(&plan, &result, id), RunState::Blocked(reason) if reason.contains("required")),
                "{id}: {:?}",
                state(&plan, &result, id)
            );
        }
    }

    #[test]
    fn one_sided_conflicts_work_in_both_filename_orders() {
        for reverse in [false, true] {
            let f = Fixture::new();
            f.plugin("a", &[], if reverse { &[] } else { &["b"] }, ABI_VERSION);
            f.plugin("b", &[], if reverse { &["a"] } else { &[] }, ABI_VERSION);
            let plan = f.plan();
            let result = execute_plan(&plan, |_, _| panic!("conflicting DLL loaded"));
            for id in ["a", "b"] {
                assert!(
                    matches!(state(&plan, &result, id), RunState::Blocked(reason) if reason.contains("declared conflict")),
                    "reverse={reverse} {id}: {:?}",
                    state(&plan, &result, id)
                );
            }
        }
    }

    #[test]
    fn incompatible_abi_blocks_managed_dll_and_dependents_before_loading() {
        let f = Fixture::new();
        // Not 99: a future bump must not turn this into an accidentally valid ABI.
        f.plugin("a", &[], &[], ABI_VERSION.wrapping_add(1));
        f.plugin("b", &["a"], &[], ABI_VERSION);
        let plan = f.plan();
        assert!(
            matches!(&plan.nodes[0].decision, Decision::Blocked { reason } if reason.contains("ABI"))
        );
        let result = execute_plan(&plan, |_, _| panic!("incompatible DLL loaded"));
        assert!(
            matches!(state(&plan, &result, "a"), RunState::Blocked(reason) if reason.contains("ABI"))
        );
        assert!(
            matches!(state(&plan, &result, "b"), RunState::Blocked(reason) if reason.contains("required"))
        );
    }

    #[test]
    fn shared_policy_and_schema_failures_block_core_and_legacy_plugins() {
        for (name, data) in [
            ("core.ini", "[loader]\nallow_unknown_build=maybe\n"),
            ("core.ini", "[logging]\nlevel=invalid\n"),
            ("defiance-config.ini", "[config]\nschema_version=99\n"),
        ] {
            let f = Fixture::new();
            let dir = f.0.join("DefianceLoader/plugins");
            let core = crate::config::builtin::find("defiance.core").unwrap();
            std::fs::write(dir.join(core.dll), b"core fixture").unwrap();
            std::fs::write(
                dir.join(crate::manifest::sidecar_name(core.dll)),
                crate::manifest::render_builtin(core),
            )
            .unwrap();
            std::fs::write(dir.join("legacy.dll"), b"legacy fixture").unwrap();
            f.config(name, data.as_bytes());
            let plan = f.plan();
            let result = execute_plan(&plan, |_, _| {
                panic!("DLL loaded with invalid shared config")
            });
            for id in ["defiance.core", "legacy"] {
                assert!(
                    matches!(state(&plan, &result, id), RunState::Blocked(reason) if !reason.is_empty()),
                    "{name} {id}: {:?}",
                    state(&plan, &result, id)
                );
            }
        }
    }

    #[test]
    fn execution_counts_failures_and_blocks_transitive_consumers() {
        let f = Fixture::new();
        f.plugin("a", &[], &[], ABI_VERSION);
        f.plugin("b", &["a"], &[], ABI_VERSION);
        f.plugin("c", &["b"], &[], ABI_VERSION);
        f.plugin("z", &[], &[], ABI_VERSION);
        let plan = f.plan();
        let mut called = Vec::new();
        let result = execute_plan(&plan, |node, _| {
            called.push(node.id.clone());
            if node.id == "a" {
                Err(LoadFailure::plain("init failed"))
            } else {
                Ok(())
            }
        });
        assert_eq!(called, ["a", "z"]);
        assert!(matches!(state(&plan, &result, "z"), RunState::Active));
        assert!(
            matches!(state(&plan, &result, "a"), RunState::Failed(reason) if reason.contains("init failed"))
        );
        for id in ["b", "c"] {
            assert!(
                matches!(state(&plan, &result, id), RunState::Blocked(reason) if reason.contains("not active")),
                "{id}: {:?}",
                state(&plan, &result, id)
            );
        }
        assert!(!result.degraded);
    }

    #[test]
    fn incomplete_rollback_discards_prior_managed_plugins_and_stops_later_loads() {
        let f = Fixture::new();
        for id in ["a", "b", "c"] {
            f.plugin(id, &[], &[], ABI_VERSION);
        }
        let plan = f.plan();
        let mut called = Vec::new();
        let result = execute_plan(&plan, |node, _| {
            called.push(node.id.clone());
            if node.id == "b" {
                Err(LoadFailure {
                    reason: "restore failed".into(),
                    degraded: true,
                })
            } else {
                Ok(())
            }
        });
        assert_eq!(called, ["a", "b"]);
        assert!(result.degraded);
        assert!(
            matches!(state(&plan, &result, "a"), RunState::Blocked(reason) if reason.contains("abandoned staged writes"))
        );
        assert!(
            matches!(state(&plan, &result, "b"), RunState::Failed(reason) if reason.contains("restore failed"))
        );
        assert!(
            matches!(state(&plan, &result, "c"), RunState::Blocked(reason) if reason.contains("incomplete rollback"))
        );
    }

    #[test]
    fn summary_prose_counts_each_state() {
        let result = StartupResult {
            states: vec![
                RunState::Active,
                RunState::Disabled,
                RunState::Blocked("x".into()),
                RunState::Ignored,
                RunState::Failed("y".into()),
            ],
            degraded: false,
        };
        assert_eq!(
            result.summary(),
            "1 active, 1 disabled, 1 blocked, 1 ignored, 1 failed"
        );
    }
}
