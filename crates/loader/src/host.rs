//! The host: what runs once the game is up, on the thread `bootstrap` starts.
//!
//! It waits for `logic.dll` and `game.dll` (a proxy DLL is in the process
//! before they are), builds the `Api` once and leaks it so the pointer stays
//! valid for the process, then discovers manifests, plans initialization and
//! loads the planned plugins in dependency order.
//!
//! Discovery reads sidecar manifests rather than executing DLLs, so a disabled
//! or blocked managed plugin is never loaded. A plugin whose exported identity,
//! ABI or version disagrees with its manifest is refused before `init`.
//!
//! Hooks belong to the plugin that installed them (`hooks::begin_plugin` marks
//! the one whose `init` is running), so a plugin that fails has its hooks taken
//! back out, and `unload` — in reverse order — stops them and removes theirs.

use crate::plan::{Decision, Planned};
#[cfg(feature = "test-host")]
use crate::plugin::unload;
use crate::plugin::{load, LoadFailure};
use crate::win;
use defiance_api::Api;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The game executable. A helper process has no `logic.dll` to patch.
const GAME_EXE: &str = "trm.exe";

/// One host per process, however many times the proxy is attached.
static STARTED: AtomicBool = AtomicBool::new(false);

pub fn run() {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    // The proxy is loaded into every process that imports it, and the embedded
    // browser's helpers are among them. Only the game should act, and the rest
    // should do nothing at all, not even open the log: their entries used to
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
    if let Some(level) = config.text(crate::config::builtin::LOGGING_SECTION, "level") {
        crate::log::set_level(&level);
    }
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
            Some((base, size)) => crate::log::info(&format!("{name} at {base:p}, {size:#x} bytes")),
            None => {
                crate::log::error(&format!(
                    "{name} did not load within {}s; no plugins installed",
                    wait.as_secs()
                ));
                return;
            }
        }
    }

    // Leaked on purpose: a plugin may keep this pointer for the process's life.
    let api: &'static Api = Box::leak(Box::new(crate::resolve::build_api()));

    let (nodes, warnings) = crate::plan::discover(&config.paths.plugin_dir);
    for warning in &warnings {
        crate::log::warn(warning);
    }
    let plan = crate::plan::plan(nodes, config);
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
                crate::log::info(&format!(
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

    let result = execute_plan(&plan, |node, index, mask| load(node, api, index, mask));
    for (node, state) in plan.nodes.iter().zip(&result.states) {
        match state {
            RunState::Active => crate::log::info(&format!("{} active", node.id)),
            RunState::Blocked(reason) => {
                crate::log::error(&format!("{} blocked: {reason}", node.id))
            }
            RunState::Failed(reason) => crate::log::error(&format!("{} failed: {reason}", node.id)),
            _ => {}
        }
    }
    let summary = result.summary();
    if result.degraded {
        crate::log::error(&format!(
            "startup degraded: {summary}; rollback incomplete, no further plugins initialized"
        ));
    } else {
        crate::log::info(&format!("startup summary: {summary}"));
    }
    crate::log::info("loader ready");
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

fn execute_plan(
    plan: &crate::plan::Plan,
    mut initialize: impl FnMut(&Planned, usize, u64) -> Result<(), LoadFailure>,
) -> StartupResult {
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
    let mask = plan.feature_mask();
    for &index in &plan.order {
        let node = &plan.nodes[index];
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
        match initialize(node, index, mask) {
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
    result
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
    assert_eq!(exe_dir, crate::config::game_dir());
    let config = crate::config::load();
    let (nodes, _) = crate::plan::discover(&config.paths.plugin_dir);
    let plan = crate::plan::plan(nodes, config);
    let api = Box::leak(Box::new(crate::resolve::build_api()));
    let result = execute_plan(&plan, |node, owner, mask| load(node, api, owner, mask));
    let states = plan
        .nodes
        .iter()
        .zip(result.states)
        .map(|(node, state)| (node.id.clone(), format!("{state:?}")))
        .collect();
    unload();
    states
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
            let (nodes, _) = crate::plan::discover(&config.paths.plugin_dir);
            crate::plan::plan(nodes, &config)
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
    fn cycle_consumers_never_reach_the_dll_loader() {
        let f = Fixture::new();
        f.plugin("a", &["b"], &[], ABI_VERSION);
        f.plugin("b", &["a"], &[], ABI_VERSION);
        f.plugin("c", &["a"], &[], ABI_VERSION);
        f.plugin("d", &["c"], &[], ABI_VERSION);
        let plan = f.plan();
        let result = execute_plan(&plan, |_, _, _| panic!("blocked DLL loaded"));
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
            let result = execute_plan(&plan, |_, _, _| panic!("conflicting DLL loaded"));
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
        let result = execute_plan(&plan, |_, _, _| panic!("incompatible DLL loaded"));
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
            let result = execute_plan(&plan, |_, _, _| {
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
        let result = execute_plan(&plan, |node, _, _| {
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
    fn incomplete_rollback_stops_all_later_loads_and_keeps_earlier_successes() {
        let f = Fixture::new();
        for id in ["a", "b", "c"] {
            f.plugin(id, &[], &[], ABI_VERSION);
        }
        let plan = f.plan();
        let mut called = Vec::new();
        let result = execute_plan(&plan, |node, _, _| {
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
        assert!(matches!(state(&plan, &result, "a"), RunState::Active));
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
