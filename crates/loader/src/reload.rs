//! Development hot reload: replace a plugin's DLL while the game runs and it is
//! unloaded and loaded again (`[loader] hot_reload`).
//!
//! A watcher thread polls the loaded plugins' files once a second. A DLL that
//! has changed, and stayed the same for one more poll (a copy has finished), is
//! queued. The queue is applied at one of two points: at the menu, once no
//! mission has been loaded or built for two polls in a row (a save loading
//! from inside a mission passes through no mission for a moment, and must not
//! count), deciding and reloading inside `session::Gate::while_at_menu` so a
//! mission cannot start in between; or just before a mission's state is built
//! ([`before_mission`], from Core's hook, on the game thread, inside the same
//! gate), so a mission started or a save loaded runs the new plugins. A reload unloads the plugins holding the plugin's service
//! tables (its dependants, `services::consumers`), deepest first, then the
//! plugin (`lifecycle::unload`); a plugin that only starts after it stays
//! loaded. A holder that can only load at startup (the expanded ammo menu holds
//! selection's table) stays too, and the old copy it calls into stays mapped,
//! stopped and unhooked, until a restart: a reloadable plugin's service
//! functions must keep working after its `stop`. It loads them again from fresh manifests through the ordinary
//! `plugin::load`. It refuses a
//! plugin whose replacement manifest forbids it (`hot_reload: false`, Core),
//! one whose manifest declares a different settings schema, and one that is
//! disabled in its last accepted configuration. All group members are checked
//! before any is unloaded. Incomplete rollback stops plugin changes until a
//! restart, and the failed copy keeps its multiplayer policy.
//!
//! The watcher also compares the plugins directory with what is loaded. A
//! plugin that appears (it was not there at startup, or was removed since) is
//! added ([`add`]): its settings come from a fresh read of the configuration
//! (`config::refresh_later`, which writes its defaults into its group file as
//! startup would), and it loads if enabled, with its dependencies loaded, and
//! allowed to load while the game runs. A built-in plugin added later needs a
//! restart, since Core prepares the built-in features at startup. A loaded
//! plugin whose files are gone is removed ([`remove`]): unloaded with the
//! plugins holding its tables, which load again only if they still can, and
//! kept mapped for a startup-only holder, as a reload keeps it. Each change
//! holds for two polls before it is queued, like a changed DLL.
//!
//! All of that is for development (`[loader] hot_reload`). A player can switch
//! plugins on and off instead (`[loader] live_toggle`, which starts the watcher
//! for this alone): when a config file changes, the watcher reads the
//! configuration (read-only), and a plugin whose `enabled` value changed, and
//! read the same on two polls, is switched: off unloads it as a removal does,
//! on loads it as an addition does ([`enable`], [`disable`]), and brings back
//! the plugins that were unloaded because they needed it. Only a change of the
//! value counts, so a plugin that failed at startup is not retried. A
//! built-in plugin comes back only if it was loaded earlier in the session.
//!
//! The same read catches a loaded plugin's other settings changing (read the
//! same on two polls, and different from what it loaded with): the plugin is
//! loaded again ([`resettle`]) with its section answered from a fresh read of
//! the configuration (`config::stage_reload`), as a changed DLL reloads. A plugin
//! that can only load at startup keeps its values until a restart, and says
//! so. A plugin switched on loads with the values as the files are then. The
//! latest settled configuration replaces any earlier queued configuration for
//! that plugin, including a reversed toggle; independent DLL changes remain.
use crate::lifecycle::{self, Loaded};
use crate::plan::Decision;
use defiance_api::Api;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

struct IncompleteRollback {
    id: String,
    reason: String,
    multiplayer_safe: bool,
}

/// A live span left by failed cleanup makes further plugin changes unsafe.
static INCOMPLETE_ROLLBACK: Mutex<Option<IncompleteRollback>> = Mutex::new(None);

fn ensure_running() -> Result<(), String> {
    match &*INCOMPLETE_ROLLBACK
        .lock()
        .unwrap_or_else(|p| p.into_inner())
    {
        Some(failure) => Err(format!(
            "plugin changes stopped after incomplete rollback of {} ({}); restart the game",
            failure.id, failure.reason
        )),
        None => Ok(()),
    }
}

/// Failed copies may still own live hooks even though initialization did not
/// record them as loaded. Their multiplayer policy remains in effect.
pub(crate) fn degraded_plugin() -> Option<(String, bool)> {
    INCOMPLETE_ROLLBACK
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|failure| (failure.id.clone(), failure.multiplayer_safe))
}

fn halt(id: &str, multiplayer_safe: bool, reason: &str) {
    let mut failure = INCOMPLETE_ROLLBACK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if failure.is_none() {
        *failure = Some(IncompleteRollback {
            id: id.to_string(),
            reason: reason.to_string(),
            multiplayer_safe,
        });
        crate::log::error(&format!(
            "hot reload: {id}: incomplete rollback ({reason}); plugin changes stop until restart"
        ));
    }
}

fn load_result(
    id: &str,
    multiplayer_safe: bool,
    result: Result<(), crate::plugin::LoadFailure>,
) -> Result<(), crate::plugin::LoadFailure> {
    if let Err(failure) = &result {
        if failure.degraded {
            halt(id, multiplayer_safe, &failure.reason);
        }
    }
    result
}

fn initialize(
    node: &crate::plan::Planned,
    api: &'static Api,
    owner: usize,
) -> Result<(), crate::plugin::LoadFailure> {
    ensure_running().map_err(crate::plugin::LoadFailure::plain)?;
    load_result(
        &node.id,
        crate::plan::multiplayer_safe(node),
        crate::plugin::load(node, api, owner),
    )
}

fn unload(plugin: &Loaded, retain: bool) -> Result<Loaded, lifecycle::UnloadError> {
    let result = lifecycle::unload(plugin.owner, retain);
    if let Err(error @ lifecycle::UnloadError::Degraded(_)) = &result {
        halt(&plugin.id, plugin.multiplayer_safe, &error.to_string());
    }
    result
}

/// The owners in `group` whose old copy must stay mapped: each whose table is
/// held by a plugin that stays (`holders`: startup-only plugins and copies
/// retained before), and, since a kept copy's code keeps running and calling
/// what it holds, each whose table a kept copy holds, and so on. `consumers`
/// is the service graph before anything is unloaded.
pub fn retained_closure(
    group: &[&Loaded],
    holders: &[usize],
    consumers: impl Fn(&str) -> Vec<usize>,
) -> Vec<usize> {
    let mut keep: Vec<usize> = Vec::new();
    loop {
        let before = keep.len();
        for plugin in group {
            if keep.contains(&plugin.owner) {
                continue;
            }
            if consumers(&plugin.id)
                .iter()
                .any(|owner| holders.contains(owner) || keep.contains(owner))
            {
                keep.push(plugin.owner);
            }
        }
        if keep.len() == before {
            return keep;
        }
    }
}

/// Unload `id` and its dependants and load them again from the plugins
/// directory. The multiplayer blockers follow what is active afterwards,
/// whatever the outcome.
pub fn reload(api: &'static Api, id: &str) -> Result<(), String> {
    let result = crate::config::reload_snapshot()
        .ok_or_else(|| "no configuration".to_string())
        .and_then(|config| reload_group(api, id, &config, None));
    crate::multiplayer::refresh();
    result
}

fn reload_group(
    api: &'static Api,
    id: &str,
    config: &crate::config::Snapshot,
    candidate: Option<&'static crate::config::Snapshot>,
) -> Result<(), String> {
    ensure_running()?;
    let plugins = lifecycle::loaded();
    let target = plugins
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(id))
        .cloned()
        .ok_or_else(|| format!("{id} is not loaded; a new plugin needs a restart"))?;
    if !target.reloadable {
        return Err(format!("{} can only be loaded at startup", target.id));
    }
    // The holders of its service tables reload with it; one that can only load
    // at startup stays, and the old copy of what it holds stays mapped for it.
    let (dependants, fixed): (Vec<Loaded>, Vec<Loaded>) =
        lifecycle::dependants(&target.id, &plugins, &crate::services::consumers)
            .into_iter()
            .partition(|p| p.reloadable);
    let group: Vec<&Loaded> = std::iter::once(&target).chain(&dependants).collect();
    // Who stays: the startup-only holders, and every copy retained before.
    let staying: Vec<Loaded> = fixed.iter().cloned().chain(lifecycle::retained()).collect();
    let staying_owners: Vec<usize> = staying.iter().map(|p| p.owner).collect();
    // Decided on the graph as it is now, before unloading drops records.
    let keep = retained_closure(&group, &staying_owners, crate::services::consumers);
    let holders_of = |plugin: &Loaded| -> Vec<String> {
        let held = crate::services::consumers(&plugin.id);
        staying
            .iter()
            .chain(group.iter().copied())
            .filter(|p| {
                held.contains(&p.owner)
                    && (staying_owners.contains(&p.owner) || keep.contains(&p.owner))
            })
            .map(|p| p.id.clone())
            .collect()
    };

    // Plan current plugin files against the configuration supplied for this reload.
    let fresh = crate::manifest::catalog(&config.paths.plugin_dir);
    let plan = crate::plan::plan(fresh.entries, config);
    for plugin in &group {
        let node = plan
            .nodes
            .iter()
            .find(|n| n.id.eq_ignore_ascii_case(&plugin.id))
            .ok_or_else(|| format!("{} is no longer in the plugins directory", plugin.id))?;
        if let Decision::Disabled { reason }
        | Decision::Blocked { reason }
        | Decision::Ignored { reason } = &node.decision
        {
            return Err(format!("{}: {reason}", plugin.id));
        }
        if !node
            .manifest
            .as_ref()
            .is_some_and(|manifest| manifest.hot_reload)
        {
            return Err(format!(
                "{}'s replacement can only be loaded at startup; restart the game to load it",
                plugin.id
            ));
        }
        let before = plugin.manifest_settings.as_deref();
        let after = node
            .manifest
            .as_ref()
            .map(|manifest| manifest.settings.as_slice());
        if before != after {
            return Err(format!(
                "{}'s settings changed; restart the game to pick them up",
                plugin.id
            ));
        }
    }

    for plugin in dependants.iter().chain(std::iter::once(&target)) {
        let retain = keep.contains(&plugin.owner);
        let holders = if retain {
            holders_of(plugin)
        } else {
            Vec::new()
        };
        unload(plugin, retain).map_err(|e| {
            format!(
                "{} could not be unloaded ({e}); restart the game to load it again",
                plugin.id
            )
        })?;
        if retain {
            crate::log::info(&format!(
                "hot reload: the old {} stays loaded for {}, which holds its service table; a restart releases it",
                plugin.id,
                holders.join(", ")
            ));
        }
    }
    for plugin in std::iter::once(&target).chain(dependants.iter().rev()) {
        let node = plan
            .nodes
            .iter()
            .find(|n| n.id.eq_ignore_ascii_case(&plugin.id))
            .expect("checked above");
        if !crate::plan::multiplayer_safe(node) {
            if !crate::multiplayer::guarded() {
                return Err(format!(
                    "{}: the multiplayer guard is not installed",
                    plugin.id
                ));
            }
            // Blocked before any of its code runs.
            crate::multiplayer::block(&plugin.id);
        }
        let settings = candidate.map(|snapshot| crate::config::stage_reload(&plugin.id, snapshot));
        let loaded = initialize(node, api, lifecycle::next_owner());
        if loaded.is_ok() {
            if let Some(settings) = settings {
                settings.commit();
            }
        }
        loaded.map_err(|e| format!("{} failed to load again: {}", plugin.id, e.reason))?;
        crate::log::info(&format!("hot reload: {} loaded again", plugin.id));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Decision, Plan, Planned};

    fn run_bounded_child(test_name: &str) -> bool {
        const CHILD: &str = "DEFIANCE_DEGRADED_RELOAD_CHILD";
        if std::env::var_os(CHILD).is_some() {
            return true;
        }

        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test_name, "--nocapture"])
            .env(CHILD, "1")
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "{test_name}: {status}");
                return false;
            }
            if std::time::Instant::now() > deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("{test_name} timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    unsafe extern "system" fn degraded_test_detour() -> u32 {
        2
    }

    #[test]
    fn a_degraded_reload_stops_the_queue_and_keeps_its_multiplayer_blocker() {
        const TEST: &str =
            "reload::tests::a_degraded_reload_stops_the_queue_and_keeps_its_multiplayer_blocker";
        if !run_bounded_child(TEST) {
            return;
        }

        // Ordinary load failures are recorded, but must not stop later work.
        let mut failed = HashMap::new();
        let mut attempted = Vec::new();
        apply_changes(
            vec![
                Change::Add("ordinary-failure".into(), stamp(1)),
                Change::Add("after-ordinary-failure".into(), stamp(2)),
            ],
            &mut failed,
            |change| {
                attempted.push(change.id().to_string());
                if change.id() == "ordinary-failure" {
                    Err("ordinary initialization failure".into())
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(attempted, ["ordinary-failure", "after-ordinary-failure"]);
        assert!(ensure_running().is_ok());

        // Publish a real hook, then decommit its target so the same native
        // restore path used by failed plugin initialization reports degraded
        // cleanup. The child process contains the retained hook and latch.
        let target = crate::code::alloc_near(degraded_test_detour as *const () as usize, 0x1000)
            .unwrap() as *mut u8;
        unsafe {
            core::slice::from_raw_parts_mut(target, 6).copy_from_slice(&[0xb8, 1, 0, 0, 0, 0xc3]);
            // mov eax,1; ret
        }
        const OWNER: usize = 0x5a71;
        crate::hooks::begin_plugin(OWNER, "degraded unsafe fixture");
        let installed =
            unsafe { crate::hooks::install_auto(target.cast(), degraded_test_detour as *mut _) };
        crate::hooks::end_plugin();
        let trampoline = installed.expect("the fixture hook must publish");
        assert_ne!(
            unsafe { crate::win::VirtualFree(target.cast(), 0x1000, crate::win::MEM_DECOMMIT) },
            0,
            "the fixture target must be decommitted to force restoration failure"
        );
        let (_removed, restore_failures) = crate::hooks::remove_owned_report(OWNER);
        assert!(restore_failures > 0, "native hook cleanup must fail");

        // Convert the observed native cleanup failure into the exact failure
        // value consumed by the production reload path.
        let load_failure = crate::plugin::LoadFailure {
            reason: format!("{restore_failures} hook span(s) could not be restored"),
            degraded: restore_failures > 0,
        };
        let mut load_failure = Some(load_failure);
        let mut attempted = Vec::new();
        *PENDING.lock().unwrap_or_else(|p| p.into_inner()) =
            vec![Change::Add("watcher-pending".into(), stamp(5))];
        apply_changes(
            vec![
                Change::Add("degraded-unsafe".into(), stamp(3)),
                Change::Add("must-not-run".into(), stamp(4)),
            ],
            &mut failed,
            |change| {
                attempted.push(change.id().to_string());
                if change.id() == "degraded-unsafe" {
                    load_result(
                        "degraded-unsafe",
                        false,
                        Err(load_failure.take().expect("failure is consumed once")),
                    )
                    .map_err(|failure| failure.reason)
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(attempted, ["degraded-unsafe"]);
        assert!(ensure_running().unwrap_err().contains("degraded-unsafe"));
        assert!(PENDING.lock().unwrap_or_else(|p| p.into_inner()).is_empty());

        // Refresh runs after failed reloads; the uncertain unsafe copy must
        // continue blocking through the service exposed to Core.
        crate::multiplayer::refresh();
        let mut buffer = [0 as core::ffi::c_char; 128];
        unsafe {
            (crate::multiplayer::API.blockers)(buffer.as_mut_ptr(), buffer.len());
        }
        let blockers = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }
            .to_str()
            .unwrap();
        assert!(blockers.contains("degraded-unsafe"), "blockers: {blockers}");

        // The target and trampoline are deliberately retained with the hook
        // record; this child exits immediately after the assertions.
        assert!(!trampoline.is_null());
    }

    fn plugin(owner: usize, id: &str) -> Loaded {
        Loaded {
            owner,
            id: id.into(),
            version: None,
            name: id.into(),
            path: Default::default(),
            shadow: Default::default(),
            module: 0,
            code: Vec::new(),
            stop: None,
            reloadable: true,
            stamp: None,
            multiplayer_safe: true,
            manifest_settings: None,
        }
    }

    fn planned(id: &str, depends: &[&str], decision: Decision) -> Planned {
        Planned {
            id: id.into(),
            dll: format!("{id}.dll"),
            path: std::path::PathBuf::from(format!("{id}.dll")),
            manifest: None,
            builtin: None,
            legacy: false,
            decision,
            depends: depends.iter().map(|id| (*id).into()).collect(),
        }
    }

    fn plan(nodes: Vec<Planned>, order: &[&str]) -> Plan {
        let order = order
            .iter()
            .map(|id| {
                nodes
                    .iter()
                    .position(|node| node.id == *id)
                    .unwrap_or_else(|| panic!("missing planned plugin {id}"))
            })
            .collect();
        Plan { nodes, order }
    }

    #[test]
    fn restore_dependency_closure_follows_order_through_branches_and_levels() {
        // The node vector is alphabetical; the dependency order starts with z-root.
        let plan = plan(
            vec![
                planned("a-branch", &["z-root"], Decision::Initialize),
                planned("b-join", &["a-branch", "c-branch"], Decision::Initialize),
                planned("c-branch", &["z-root"], Decision::Initialize),
                planned("d-leaf", &["b-join"], Decision::Initialize),
                planned("z-root", &[], Decision::Initialize),
            ],
            &["z-root", "a-branch", "c-branch", "b-join", "d-leaf"],
        );
        let mut active: HashSet<String> = ["z-root".into()].into();
        let ever: HashSet<String> = [
            "a-branch".into(),
            "b-join".into(),
            "c-branch".into(),
            "d-leaf".into(),
        ]
        .into();
        let restored =
            restore_dependants_from_plan(&plan, "z-root", &mut active, &ever, |_| Ok(()));
        assert_eq!(
            restored
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            ["a-branch", "c-branch", "b-join", "d-leaf"]
        );
        assert!(restored.iter().all(|(_, result)| result.is_ok()));
        assert!(active.contains("d-leaf"));
    }

    #[test]
    fn restore_dependency_closure_skips_disabled_consumers() {
        let plan = plan(
            vec![
                planned(
                    "disabled-consumer",
                    &["provider"],
                    Decision::Disabled {
                        reason: "disabled in config".into(),
                    },
                ),
                planned(
                    "transitive-consumer",
                    &["disabled-consumer"],
                    Decision::Blocked {
                        reason: "required dependency is disabled".into(),
                    },
                ),
                planned("provider", &[], Decision::Initialize),
            ],
            &["provider"],
        );
        let mut active: HashSet<String> = ["provider".into()].into();
        let ever: HashSet<String> =
            ["disabled-consumer".into(), "transitive-consumer".into()].into();
        let restored = restore_dependants_from_plan(&plan, "provider", &mut active, &ever, |_| {
            panic!("a disabled consumer must not load")
        });
        assert!(restored.is_empty());
        assert_eq!(active, ["provider".into()].into());
    }

    #[test]
    fn restore_dependency_closure_stops_at_a_failed_provider() {
        let plan = plan(
            vec![
                planned("leaf", &["middle"], Decision::Initialize),
                planned("middle", &["root"], Decision::Initialize),
                planned("root", &[], Decision::Initialize),
            ],
            &["root", "middle", "leaf"],
        );
        let ever: HashSet<String> = ["middle".into(), "leaf".into()].into();
        let mut active: HashSet<String> = ["root".into()].into();
        let mut attempted = Vec::new();
        let restored = restore_dependants_from_plan(&plan, "root", &mut active, &ever, |id| {
            attempted.push(id.to_string());
            Err("initialization failed".into())
        });
        assert_eq!(attempted, ["middle"]);
        assert_eq!(restored.len(), 1);
        assert!(!active.contains("middle"));
        assert!(!active.contains("leaf"));

        // A provider that did not load cannot release any of its dependants.
        active.clear();
        let restored = restore_dependants_from_plan(&plan, "root", &mut active, &ever, |_| {
            panic!("dependants require an active root")
        });
        assert!(restored.is_empty());
    }

    #[test]
    fn retention_follows_the_tables_a_kept_copy_holds() {
        // C (owner 3) can only load at startup and holds B's table; B holds
        // A's. Reloading A takes B with it: old B stays for C, and old A stays
        // for old B.
        let (a, b) = (plugin(1, "a"), plugin(2, "b"));
        let consumers = |id: &str| match id {
            "a" => vec![2],
            "b" => vec![3],
            _ => vec![],
        };
        let mut kept = retained_closure(&[&a, &b], &[3], consumers);
        kept.sort();
        assert_eq!(kept, [1, 2]);
        // Nobody startup-only holds anything: both go.
        assert!(retained_closure(&[&a, &b], &[], consumers).is_empty());
    }

    fn stamp(n: u64) -> Stamp {
        Some((n, std::time::SystemTime::UNIX_EPOCH))
    }

    fn config_state(enabled: Option<bool>, speed: &str) -> ConfigState {
        ConfigState {
            enabled,
            settings: [("speed".to_string(), speed.to_string())]
                .into_iter()
                .collect(),
        }
    }

    fn config_map(state: ConfigState) -> HashMap<String, ConfigState> {
        [("a".to_string(), state)].into_iter().collect()
    }

    fn queue_stable_config(
        pending: &mut Vec<Change>,
        desired: &mut HashMap<String, ConfigState>,
        state: ConfigState,
        reloadable: &HashSet<String>,
    ) {
        let now = config_map(state);
        update_desired_config(&now, &now, desired, reloadable, |change| {
            queue_into(pending, change);
        });
    }

    #[test]
    fn a_new_plugin_is_added_once_its_copy_has_settled() {
        let loaded: HashMap<String, bool> = [("core".to_string(), false)].into();
        let known: HashSet<String> = ["core".to_string(), "off".to_string()].into();
        let before: HashMap<String, Stamp> = [
            ("core".into(), stamp(1)),
            ("new".into(), stamp(5)),
            ("off".into(), stamp(2)),
        ]
        .into();
        // Still being copied: the stamp differs from the previous poll.
        let copying: HashMap<String, Stamp> = [
            ("core".into(), stamp(1)),
            ("new".into(), stamp(6)),
            ("off".into(), stamp(2)),
        ]
        .into();
        let none = HashMap::new();
        assert!(directory_changes(&copying, &before, &loaded, &known, &none)
            .0
            .is_empty());
        // Settled: added. A plugin known at startup (disabled then) is not.
        let (adds, removes) = directory_changes(&before, &before, &loaded, &known, &none);
        assert_eq!(adds, [("new".to_string(), stamp(5))]);
        assert!(removes.is_empty());
        // A failed add is not retried until its DLL changes.
        let failed: HashMap<String, Stamp> = [("new".to_string(), stamp(5))].into();
        assert!(
            directory_changes(&before, &before, &loaded, &known, &failed)
                .0
                .is_empty()
        );
    }

    #[test]
    fn a_plugin_whose_files_are_gone_is_removed_after_two_polls() {
        let loaded: HashMap<String, bool> = [
            ("core".to_string(), false),
            ("gone".to_string(), true),
            ("fixed".to_string(), false),
        ]
        .into();
        let known: HashSet<String> = loaded.keys().cloned().collect();
        let with: HashMap<String, Stamp> =
            [("core".into(), stamp(1)), ("gone".into(), stamp(1))].into();
        let without: HashMap<String, Stamp> = [("core".into(), stamp(1))].into();
        let none = HashMap::new();
        // Missing on one poll only: maybe a copy replacing it.
        assert!(directory_changes(&without, &with, &loaded, &known, &none)
            .1
            .is_empty());
        // Missing on both; a startup-only plugin whose files are gone stays.
        let (adds, removes) = directory_changes(&without, &without, &loaded, &known, &none);
        assert!(adds.is_empty());
        assert_eq!(removes, ["gone"]);
    }

    #[test]
    fn a_pending_disable_survives_a_later_settings_edit() {
        let applied = config_state(Some(true), "1");
        let mut desired = config_map(applied.clone());
        let mut pending = Vec::new();
        let reloadable: HashSet<String> = ["a".to_string()].into();
        let off = config_state(Some(false), "1");

        // A first poll sees the edit but does not change desired state.
        let first = config_map(off.clone());
        let before = config_map(applied.clone());
        assert!(config_changes(&first, &before, &desired, &reloadable).is_empty());
        queue_stable_config(&mut pending, &mut desired, off.clone(), &reloadable);

        // Another edit settles before the safe point while the plugin is loaded.
        let settings_edit = config_state(Some(false), "2");
        let first = config_map(settings_edit.clone());
        let before = config_map(off);
        assert!(config_changes(&first, &before, &desired, &reloadable).is_empty());
        queue_stable_config(&mut pending, &mut desired, settings_edit, &reloadable);
        let Change::Config(id, final_state) = pending.last().unwrap() else {
            panic!("expected the final config snapshot");
        };
        assert_eq!(id, "a");
        assert_eq!(final_state.enabled, Some(false));
        assert_eq!(final_state.settings["speed"], "2");
        assert_eq!(
            config_action(final_state, Some(&applied), true, true, false),
            ConfigAction::Disable
        );
    }

    #[test]
    fn a_later_disable_wins_over_a_pending_settings_reload() {
        let applied = config_state(Some(true), "1");
        let mut desired = config_map(applied.clone());
        let mut pending = Vec::new();
        let reloadable: HashSet<String> = ["a".to_string()].into();

        queue_stable_config(
            &mut pending,
            &mut desired,
            config_state(Some(true), "2"),
            &reloadable,
        );
        queue_stable_config(
            &mut pending,
            &mut desired,
            config_state(Some(false), "2"),
            &reloadable,
        );
        let Change::Config(_, final_state) = pending.last().unwrap() else {
            panic!("expected the final config snapshot");
        };
        assert_eq!(final_state.enabled, Some(false));
        assert_eq!(final_state.settings["speed"], "2");
        assert_eq!(
            config_action(final_state, Some(&applied), true, true, false),
            ConfigAction::Disable
        );
    }

    #[test]
    fn disable_then_reenable_before_application_keeps_the_final_enabled_state() {
        let applied = config_state(Some(true), "1");
        let mut desired = config_map(applied.clone());
        let mut pending = Vec::new();
        let reloadable: HashSet<String> = ["a".to_string()].into();

        queue_stable_config(
            &mut pending,
            &mut desired,
            config_state(Some(false), "1"),
            &reloadable,
        );
        queue_stable_config(
            &mut pending,
            &mut desired,
            config_state(Some(true), "1"),
            &reloadable,
        );
        let Change::Config(_, final_state) = pending.last().unwrap() else {
            panic!("expected the final config snapshot");
        };
        assert_eq!(final_state.enabled, Some(true));
        assert_eq!(
            config_action(final_state, Some(&applied), true, true, false),
            ConfigAction::Keep
        );
    }

    #[test]
    fn a_successful_file_reload_applies_settings_without_a_second_resettle() {
        let applied = config_state(Some(true), "1");
        let desired = config_state(Some(true), "2");
        assert_eq!(
            config_action(&desired, Some(&applied), true, true, true),
            ConfigAction::Keep
        );

        let mut states = config_map(applied.clone());
        record_applied_config(&mut states, "a", &desired, true, true);
        assert_eq!(states["a"], desired);
    }

    #[test]
    fn a_file_reload_group_covers_pending_settings_for_its_dependants() {
        let before = [
            plugin(1, "provider"),
            plugin(2, "dependent"),
            plugin(3, "other"),
        ];
        let after = [
            plugin(4, "provider"),
            plugin(5, "dependent"),
            plugin(3, "other"),
        ];
        let reloaded = reloaded_ids(&before, &after);

        assert_eq!(
            reloaded,
            ["provider".to_string(), "dependent".to_string()].into()
        );
        let applied = config_state(Some(true), "1");
        let desired = config_state(Some(true), "2");
        assert_eq!(
            config_action(
                &desired,
                Some(&applied),
                true,
                true,
                reloaded.contains("dependent")
            ),
            ConfigAction::Keep
        );
    }

    #[test]
    fn an_unsuccessful_resettle_keeps_the_last_applied_settings() {
        let applied = config_state(Some(true), "1");
        let desired = config_state(Some(true), "2");
        let mut states = config_map(applied.clone());

        record_applied_config(&mut states, "a", &desired, true, false);

        assert_eq!(states["a"], applied);
    }

    #[test]
    fn a_copy_kept_by_an_earlier_reload_still_holds_what_it_called() {
        // Old B (owner 9), kept by an earlier reload of B, still holds A's
        // table; reloading A now must keep old A for it.
        let a = plugin(1, "a");
        let consumers = |id: &str| if id == "a" { vec![9] } else { vec![] };
        assert_eq!(retained_closure(&[&a], &[9], consumers), [1]);
    }
}

type Stamp = Option<(u64, std::time::SystemTime)>;

/// A stable config snapshot that the watcher wants the loaded plugin to use.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ConfigState {
    enabled: Option<bool>,
    settings: BTreeMap<String, String>,
}

/// A change waiting for the menu or a mission's start.
#[derive(Clone, Debug, PartialEq)]
enum Change {
    /// A loaded plugin's DLL changed; the stamp seen.
    Reload(String, Stamp),
    /// A plugin appeared in the directory; its DLL's stamp.
    Add(String, Stamp),
    /// A loaded plugin's files are gone.
    Remove(String),
    /// A settled enabled state and settings snapshot from the config files.
    Config(String, ConfigState),
}

impl Change {
    fn id(&self) -> &str {
        match self {
            Change::Reload(id, _)
            | Change::Add(id, _)
            | Change::Remove(id)
            | Change::Config(id, _) => id,
        }
    }
    fn stamp(&self) -> Stamp {
        match self {
            Change::Reload(_, stamp) | Change::Add(_, stamp) => *stamp,
            Change::Remove(_) | Change::Config(_, _) => None,
        }
    }
    fn describe(&self) -> &'static str {
        match self {
            Change::Reload(..) => "changed; it reloads",
            Change::Add(..) => "added; it loads",
            Change::Remove(_) => "removed; it unloads",
            Change::Config(..) => "config changed; it reconciles at the safe point",
        }
    }
}

/// Changes waiting to be applied.
static PENDING: Mutex<Vec<Change>> = Mutex::new(Vec::new());
/// The watcher baseline from startup, updated after successful reconciliation.
static APPLIED_CONFIG: Mutex<Option<HashMap<String, ConfigState>>> = Mutex::new(None);
/// The plugin IDs (lowercase) that are not new: present at startup (loaded or
/// not), or added since. A removed plugin leaves it, so it can come back.
static KNOWN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// The plugin IDs (lowercase) loaded at any time in this session: a built-in
/// plugin can be switched back on only if Core prepared its feature.
static EVER_LOADED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn ever_loaded() -> std::sync::MutexGuard<'static, Option<HashSet<String>>> {
    EVER_LOADED.lock().unwrap_or_else(|p| p.into_inner())
}

/// Record what is loaded now as loaded in this session.
fn note_loaded() {
    let mut ever = ever_loaded();
    let ever = ever.get_or_insert_with(HashSet::new);
    for plugin in lifecycle::loaded() {
        ever.insert(plugin.id.to_ascii_lowercase());
    }
}

fn known() -> std::sync::MutexGuard<'static, Option<HashSet<String>>> {
    KNOWN.lock().unwrap_or_else(|p| p.into_inner())
}

/// Queue `change`, replacing an earlier change of the same kind for that plugin.
fn queue(change: Change) {
    if ensure_running().is_err() {
        return;
    }
    let mut pending = PENDING.lock().unwrap_or_else(|p| p.into_inner());
    if !queue_into(&mut pending, change.clone()) {
        return;
    }
    crate::log::info(&format!(
        "hot reload: {} {} at the menu or when a mission starts or a save loads",
        change.id(),
        change.describe()
    ));
}

/// Keep only the latest config snapshot while retaining independent file changes.
fn queue_into(pending: &mut Vec<Change>, change: Change) -> bool {
    if pending.contains(&change) {
        return false;
    }
    if matches!(&change, Change::Config(..)) {
        pending.retain(|queued| {
            !matches!(queued, Change::Config(id, _) if id.eq_ignore_ascii_case(change.id()))
        });
    } else {
        pending.retain(|queued| {
            !queued.id().eq_ignore_ascii_case(change.id()) || matches!(queued, Change::Config(..))
        });
    }
    pending.push(change);
    true
}

/// A reload group gives every successfully loaded member a new owner, including its dependants.
fn reloaded_ids(before: &[Loaded], after: &[Loaded]) -> HashSet<String> {
    after
        .iter()
        .filter(|loaded| {
            before
                .iter()
                .find(|old| old.id.eq_ignore_ascii_case(&loaded.id))
                .is_some_and(|old| old.owner != loaded.owner)
        })
        .map(|loaded| loaded.id.to_ascii_lowercase())
        .collect()
}

/// One reload at a time, from the watcher or a game thread.
static APPLYING: Mutex<()> = Mutex::new(());
static API: OnceLock<&'static Api> = OnceLock::new();

/// Reload everything queued. A failure is logged and not retried until the
/// file changes again.
fn apply_pending(failed: &mut HashMap<String, Stamp>) {
    let Some(api) = API.get().copied() else {
        return;
    };
    let _one = APPLYING.lock().unwrap_or_else(|p| p.into_inner());
    let mut queued = std::mem::take(&mut *PENDING.lock().unwrap_or_else(|p| p.into_inner()));
    // Apply file changes first; the config snapshot then reconciles the final
    // enabled state and settings against what those changes left loaded.
    queued.sort_by_key(|change| matches!(change, Change::Config(..)));
    let mut successfully_reloaded = HashSet::new();
    apply_changes(queued, failed, |change| {
        let settings_reloaded = match change {
            Change::Config(id, desired) => {
                successfully_reloaded.contains(&id.to_ascii_lowercase())
                    && crate::config::accepted_settings(id).as_ref() == Some(&desired.settings)
            }
            _ => false,
        };
        let current = lifecycle::loaded()
            .into_iter()
            .find(|p| p.id.eq_ignore_ascii_case(change.id()));
        match change {
            // A reload of an earlier one may already have reloaded this one as
            // a dependant.
            Change::Reload(id, stamp) if current.as_ref().is_none_or(|p| p.stamp != *stamp) => {
                let before = lifecycle::loaded();
                let result = reload(api, id);
                successfully_reloaded.extend(reloaded_ids(&before, &lifecycle::loaded()));
                result
            }
            Change::Add(id, _) if current.is_none() => add(api, id).map(|()| {
                known()
                    .get_or_insert_with(HashSet::new)
                    .insert(id.to_ascii_lowercase());
            }),
            Change::Remove(id) if current.is_some() => remove(api, id).map(|()| {
                if let Some(known) = known().as_mut() {
                    known.remove(&id.to_ascii_lowercase());
                }
            }),
            Change::Config(id, desired) => {
                reconcile_config(api, id, desired.clone(), settings_reloaded)
            }
            _ => Ok(()),
        }
    });
}

/// Drain only while rollback is complete, including changes already taken out
/// of the shared queue before an initializer fails.
fn apply_changes(
    queued: Vec<Change>,
    failed: &mut HashMap<String, Stamp>,
    mut apply: impl FnMut(&Change) -> Result<(), String>,
) {
    for change in queued {
        if ensure_running().is_err() {
            PENDING.lock().unwrap_or_else(|p| p.into_inner()).clear();
            break;
        }
        let result = apply(&change);
        note_loaded();
        if let Err(e) = result {
            crate::log::warn(&format!("hot reload: {e}"));
            failed.insert(change.id().to_ascii_lowercase(), change.stamp());
        }
    }
}

/// Apply settled file/config observations through the watcher path without
/// starting its polling thread in the native service fixtures.
#[cfg(feature = "test-host")]
pub fn test_reload_and_config(
    api: &'static Api,
    file_id: &str,
    config_id: &str,
) -> Result<(), String> {
    let _ = API.set(api);
    {
        let mut applied = APPLIED_CONFIG.lock().unwrap_or_else(|p| p.into_inner());
        applied.get_or_insert_with(startup_config);
    }
    let plugin = lifecycle::loaded()
        .into_iter()
        .find(|plugin| plugin.id.eq_ignore_ascii_case(file_id))
        .ok_or_else(|| format!("{file_id} is not loaded"))?;
    let desired = config_now()
        .remove(&config_id.to_ascii_lowercase())
        .ok_or_else(|| format!("{config_id} has no configuration"))?;
    queue(Change::Reload(
        file_id.to_string(),
        lifecycle::stamp(&plugin.path),
    ));
    queue(Change::Config(config_id.to_string(), desired));
    let mut failed = HashMap::new();
    apply_pending(&mut failed);
    if failed.is_empty() {
        Ok(())
    } else {
        let mut ids: Vec<String> = failed.into_keys().collect();
        ids.sort();
        Err(format!("queued changes failed: {}", ids.join(", ")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfigAction {
    Enable,
    Disable,
    Resettle,
    Keep,
}

fn config_action(
    desired: &ConfigState,
    applied: Option<&ConfigState>,
    loaded: bool,
    reloadable: bool,
    settings_reloaded: bool,
) -> ConfigAction {
    let settings_changed = applied.is_some_and(|last| last.settings != desired.settings);
    match desired.enabled {
        Some(false) if loaded => ConfigAction::Disable,
        Some(false) => ConfigAction::Keep,
        Some(true) if !loaded => ConfigAction::Enable,
        Some(true) if settings_reloaded => ConfigAction::Keep,
        Some(true) if settings_changed && reloadable => ConfigAction::Resettle,
        None if loaded && settings_reloaded => ConfigAction::Keep,
        None if loaded && settings_changed && reloadable => ConfigAction::Resettle,
        _ => ConfigAction::Keep,
    }
}

/// Reconcile the last settled config snapshot with the plugin that is loaded
/// at the safe point. The applied snapshot advances only for work that succeeds.
fn reconcile_config(
    api: &'static Api,
    id: &str,
    desired: ConfigState,
    settings_reloaded: bool,
) -> Result<(), String> {
    let current = lifecycle::loaded()
        .into_iter()
        .find(|plugin| plugin.id.eq_ignore_ascii_case(id));
    let applied = APPLIED_CONFIG
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(|states| states.get(&id.to_ascii_lowercase()))
        .cloned();
    let action = config_action(
        &desired,
        applied.as_ref(),
        current.is_some(),
        current.as_ref().is_some_and(|plugin| plugin.reloadable),
        settings_reloaded,
    );
    let (result, settings_applied) = match action {
        ConfigAction::Enable => (enable(api, id), true),
        ConfigAction::Disable => (disable(api, id), false),
        ConfigAction::Resettle => (resettle(api, id), true),
        ConfigAction::Keep => (
            Ok(()),
            settings_reloaded && current.is_some() && desired.enabled != Some(false),
        ),
    };
    let loaded = lifecycle::loaded()
        .iter()
        .any(|plugin| plugin.id.eq_ignore_ascii_case(id));
    let mut states = APPLIED_CONFIG.lock().unwrap_or_else(|p| p.into_inner());
    record_applied_config(
        states.get_or_insert_with(HashMap::new),
        id,
        &desired,
        loaded,
        result.is_ok() && settings_applied,
    );
    result
}

fn record_applied_config(
    states: &mut HashMap<String, ConfigState>,
    id: &str,
    desired: &ConfigState,
    loaded: bool,
    settings_applied: bool,
) {
    let state = states
        .entry(id.to_ascii_lowercase())
        .or_insert_with(|| ConfigState {
            enabled: None,
            settings: BTreeMap::new(),
        });
    if let Some(enabled) = desired.enabled {
        if enabled == loaded {
            state.enabled = Some(enabled);
        }
    }
    if settings_applied {
        state.settings = desired.settings.clone();
    }
}

/// Load a plugin that appeared in the plugins directory after startup.
pub fn add(api: &'static Api, id: &str) -> Result<(), String> {
    let result = add_plugin(api, id, false);
    crate::multiplayer::refresh();
    result
}

/// Load a plugin switched on in its config file, then restore eligible
/// previously loaded dependants in dependency order.
pub fn enable(api: &'static Api, id: &str) -> Result<(), String> {
    note_loaded();
    let prepared = ever_loaded()
        .as_ref()
        .is_some_and(|ever| ever.contains(&id.to_ascii_lowercase()));
    let result = add_plugin(api, id, prepared).map(|()| {
        crate::log::info(&format!("hot reload: {id} switched on"));
        note_loaded();
        restore_dependants(api, id);
    });
    crate::multiplayer::refresh();
    result
}

/// Restore the previously loaded, eligible dependency closure of `id`.
fn restore_dependants(api: &'static Api, id: &str) {
    let config = crate::config::refresh_later();
    let plan = crate::plan::plan(config.catalog.entries.clone(), config);
    let loaded = lifecycle::loaded();
    let mut active: HashSet<String> = loaded
        .iter()
        .map(|plugin| plugin.id.to_ascii_lowercase())
        .collect();
    let ever = ever_loaded().clone().unwrap_or_default();
    for (dependant, result) in
        restore_dependants_from_plan(&plan, id, &mut active, &ever, |dependant| {
            add_plugin(api, dependant, true)
        })
    {
        match result {
            Ok(()) => crate::log::info(&format!("hot reload: {dependant} loaded again")),
            Err(e) => crate::log::warn(&format!("hot reload: {e}")),
        }
    }
}

/// Restore the eligible previously loaded dependency closure from `plan`.
fn restore_dependants_from_plan(
    plan: &crate::plan::Plan,
    id: &str,
    active: &mut HashSet<String>,
    ever: &HashSet<String>,
    mut load: impl FnMut(&str) -> Result<(), String>,
) -> Vec<(String, Result<(), String>)> {
    let mut closure = HashSet::from([id.to_ascii_lowercase()]);
    let mut results = Vec::new();
    for &index in &plan.order {
        let node = &plan.nodes[index];
        let node_id = node.id.to_ascii_lowercase();
        if !node
            .depends
            .iter()
            .any(|dependency| closure.contains(&dependency.to_ascii_lowercase()))
        {
            continue;
        }
        closure.insert(node_id.clone());
        if !matches!(node.decision, Decision::Initialize)
            || active.contains(&node_id)
            || !ever.contains(&node_id)
            || node
                .depends
                .iter()
                .any(|dependency| !active.contains(&dependency.to_ascii_lowercase()))
        {
            continue;
        }
        let result = load(&node.id);
        if result.is_ok() {
            active.insert(node_id);
        }
        results.push((node.id.clone(), result));
    }
    results
}

/// Load a plugin again, with the plugins holding its tables, for settings
/// changed in its config file: from now on its section is answered from the
/// files as they are now. Refused for invalid settings, which leave it running
/// with the values it has.
pub fn resettle(api: &'static Api, id: &str) -> Result<(), String> {
    ensure_running()?;
    // What is loaded now may be unloaded with it and come back with it.
    note_loaded();
    let config = crate::config::refresh_later();
    if config.is_blocked(id) {
        return Err(format!(
            "{id}'s changed settings are invalid; it keeps the values it loaded with"
        ));
    }
    let result = reload_group(api, id, config, Some(config));
    crate::multiplayer::refresh();
    result.map(|()| crate::log::info(&format!("hot reload: {id} uses its new settings")))
}

/// Unload a plugin switched off in its config file, with the plugins holding
/// its tables; those load again if they still can without it.
pub fn disable(api: &'static Api, id: &str) -> Result<(), String> {
    // What is loaded now may be unloaded with it and come back with it.
    note_loaded();
    let result = remove_plugin(api, id, "switched off");
    crate::multiplayer::refresh();
    result
}

/// Load `id` from the plugins directory as the files are now. A built-in
/// plugin only with `prepared` (Core prepared its feature this session).
fn add_plugin(api: &'static Api, id: &str, prepared: bool) -> Result<(), String> {
    ensure_running()?;
    if lifecycle::loaded()
        .iter()
        .any(|p| p.id.eq_ignore_ascii_case(id))
    {
        return Err(format!("{id} is already loaded"));
    }
    // The configuration as the files are now: the plugin's settings, which
    // may have changed since startup while it was off.
    let config = crate::config::refresh_later();
    let loaded = lifecycle::loaded();
    let loaded_versions: Vec<(String, Option<crate::manifest::Version>)> = loaded
        .iter()
        .map(|plugin| (plugin.id.clone(), plugin.version))
        .collect();
    let plan = crate::plan::plan_with_loaded_versions(
        config.catalog.entries.clone(),
        config,
        &loaded_versions,
    );
    let node = plan
        .nodes
        .iter()
        .find(|n| n.id.eq_ignore_ascii_case(id))
        .ok_or_else(|| format!("{id} is not in the plugins directory"))?;
    if node.builtin.is_some() && !prepared {
        return Err(format!(
            "{id} is a built-in plugin not loaded this session; restart the game to load it (Core prepares the built-in features at startup)"
        ));
    }
    if let Decision::Disabled { reason }
    | Decision::Blocked { reason }
    | Decision::Ignored { reason } = &node.decision
    {
        return Err(format!("{id}: {reason}"));
    }
    let manifest = node.manifest.as_ref();
    if manifest.is_some_and(|m| !m.hot_reload) {
        return Err(format!(
            "{id} can only be loaded at startup; restart the game to load it"
        ));
    }
    for dependency in manifest.map(|m| m.depends.as_slice()).unwrap_or_default() {
        if !loaded
            .iter()
            .any(|p| p.id.eq_ignore_ascii_case(&dependency.id))
        {
            return Err(format!(
                "{id} needs {}, which is not loaded; restart the game to load them",
                dependency.id
            ));
        }
    }
    if !crate::plan::multiplayer_safe(node) {
        if !crate::multiplayer::guarded() {
            return Err(format!("{id}: the multiplayer guard is not installed"));
        }
        crate::multiplayer::block(&node.id);
    }
    let settings = crate::config::stage_reload(id, config);
    let loaded = initialize(node, api, lifecycle::next_owner());
    if loaded.is_ok() {
        settings.commit();
    }
    loaded.map_err(|e| format!("{id} failed to load: {}", e.reason))?;
    crate::log::info(&format!("hot reload: {id} loaded"));
    Ok(())
}

/// Unload a plugin whose files are gone, with the plugins holding its tables;
/// those load again if they still can without it.
pub fn remove(api: &'static Api, id: &str) -> Result<(), String> {
    let result = remove_plugin(api, id, "removed");
    crate::multiplayer::refresh();
    result
}

fn remove_plugin(api: &'static Api, id: &str, why: &str) -> Result<(), String> {
    ensure_running()?;
    let plugins = lifecycle::loaded();
    let target = plugins
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(id))
        .cloned()
        .ok_or_else(|| format!("{id} is not loaded"))?;
    if !target.reloadable {
        return Err(format!(
            "{} can only be unloaded by a restart; it stays loaded",
            target.id
        ));
    }
    let (dependants, fixed): (Vec<Loaded>, Vec<Loaded>) =
        lifecycle::dependants(&target.id, &plugins, &crate::services::consumers)
            .into_iter()
            .partition(|p| p.reloadable);
    let group: Vec<&Loaded> = std::iter::once(&target).chain(&dependants).collect();
    let staying: Vec<usize> = fixed
        .iter()
        .chain(lifecycle::retained().iter())
        .map(|p| p.owner)
        .collect();
    let keep = retained_closure(&group, &staying, crate::services::consumers);
    for plugin in dependants.iter().chain(std::iter::once(&target)) {
        let retain = keep.contains(&plugin.owner);
        unload(plugin, retain).map_err(|e| format!("{} could not be unloaded ({e})", plugin.id))?;
        if retain {
            crate::log::info(&format!(
                "hot reload: the old {} stays loaded for a plugin holding its service table; a restart releases it",
                plugin.id
            ));
        }
    }
    crate::log::info(&format!("hot reload: {} {why}", target.id));
    // The holders load again where they can without it, judged by the files
    // as they are now (read-only), in which it is gone or switched off.
    let config = crate::config::inspect(&crate::config::game_dir());
    let plan = crate::plan::plan(config.catalog.entries.clone(), &config);
    for plugin in dependants.iter().rev() {
        let node = plan
            .nodes
            .iter()
            .find(|n| n.id.eq_ignore_ascii_case(&plugin.id));
        match node.map(|n| &n.decision) {
            Some(Decision::Initialize) => {
                let node = node.expect("matched");
                if !crate::plan::multiplayer_safe(node) {
                    crate::multiplayer::block(&node.id);
                }
                match initialize(node, api, lifecycle::next_owner()) {
                    Ok(()) => crate::log::info(&format!("hot reload: {} loaded again", plugin.id)),
                    Err(e) => crate::log::warn(&format!(
                        "hot reload: {} stays unloaded: {}",
                        plugin.id, e.reason
                    )),
                }
            }
            Some(
                Decision::Disabled { reason }
                | Decision::Blocked { reason }
                | Decision::Ignored { reason },
            ) => crate::log::info(&format!(
                "hot reload: {} stays unloaded: {reason}",
                plugin.id
            )),
            None => crate::log::info(&format!(
                "hot reload: {} stays unloaded: it is gone too",
                plugin.id
            )),
        }
    }
    Ok(())
}

/// Settled snapshots (lowercase IDs) that differ from the watcher's desired
/// state. Settings-only changes reload only copies that support reload.
fn config_changes(
    now: &HashMap<String, ConfigState>,
    before: &HashMap<String, ConfigState>,
    desired: &HashMap<String, ConfigState>,
    reloadable: &HashSet<String>,
) -> Vec<Change> {
    let mut out = Vec::new();
    for (id, state) in now {
        if before.get(id) != Some(state) {
            continue;
        }
        let Some(previous) = desired.get(id) else {
            continue;
        };
        let enabled_changed = previous.enabled != state.enabled;
        let settings_changed = previous.settings != state.settings;
        if enabled_changed || (settings_changed && reloadable.contains(id)) {
            out.push(Change::Config(id.clone(), state.clone()));
        }
    }
    out.sort_by(|a, b| a.id().cmp(b.id()));
    out
}

/// Advance desired config only after a stable observation, keeping queue
/// coalescing on the same path that the watcher uses.
fn update_desired_config(
    now: &HashMap<String, ConfigState>,
    before: &HashMap<String, ConfigState>,
    desired: &mut HashMap<String, ConfigState>,
    reloadable: &HashSet<String>,
    mut enqueue: impl FnMut(Change),
) {
    for change in config_changes(now, before, desired, reloadable) {
        enqueue(change);
    }
    *desired = now.clone();
}

/// Every plugin's `enabled` and other settings as the configuration files say
/// now (read-only), by lowercase ID; Core, which is always on, and legacy
/// plugins left out.
fn config_now() -> HashMap<String, ConfigState> {
    let config = crate::config::inspect(&crate::config::game_dir());
    let ids: Vec<String> = config
        .catalog
        .entries
        .iter()
        .map(|entry| entry.id())
        .filter(|id| !id.eq_ignore_ascii_case(crate::config::builtin::CORE_ID))
        .collect();
    ids.iter()
        .map(|id| {
            (
                id.to_ascii_lowercase(),
                ConfigState {
                    enabled: config.enabled(id),
                    settings: config.settings(id),
                },
            )
        })
        .collect()
}

/// Config values the plugins started with, before the watcher observes edits.
fn startup_config() -> HashMap<String, ConfigState> {
    let Some(config) = crate::config::current() else {
        return HashMap::new();
    };
    config
        .catalog
        .entries
        .iter()
        .map(|entry| {
            let id = entry.id();
            (
                id.to_ascii_lowercase(),
                ConfigState {
                    enabled: config.enabled(&id),
                    settings: config.settings(&id),
                },
            )
        })
        .filter(|(id, _)| !id.eq_ignore_ascii_case(crate::config::builtin::CORE_ID))
        .collect()
}

/// The config files' stamps, to read the configuration only when one changed.
fn config_stamps(dir: &std::path::Path) -> Vec<(std::path::PathBuf, Stamp)> {
    let mut out: Vec<(std::path::PathBuf, Stamp)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("ini"))
        })
        .map(|path| {
            let stamp = lifecycle::stamp(&path);
            (path, stamp)
        })
        .collect();
    out.sort();
    out
}

/// The directory's plugins (lowercase ID -> DLL stamp) as the watcher reads it.
fn directory(plugin_dir: &std::path::Path) -> HashMap<String, Stamp> {
    crate::manifest::catalog(plugin_dir)
        .entries
        .iter()
        .map(|entry| {
            (
                entry.id().to_ascii_lowercase(),
                lifecycle::stamp(&entry.path),
            )
        })
        .collect()
}

/// The plugins to add and to remove, from two polls of the directory (`now`,
/// `before`, lowercase ID -> DLL stamp). Added: here on both polls with the
/// same stamp (a copy has finished), not loaded, not `known`, and not a
/// failure with this stamp. Removed: loaded and reloadable (`loaded`: lowercase
/// ID -> reloadable), and missing on both polls.
fn directory_changes(
    now: &HashMap<String, Stamp>,
    before: &HashMap<String, Stamp>,
    loaded: &HashMap<String, bool>,
    known: &HashSet<String>,
    failed: &HashMap<String, Stamp>,
) -> (Vec<(String, Stamp)>, Vec<String>) {
    let mut adds: Vec<(String, Stamp)> = now
        .iter()
        .filter(|(id, stamp)| {
            stamp.is_some()
                && before.get(*id) == Some(stamp)
                && !loaded.contains_key(*id)
                && !known.contains(*id)
                && failed.get(*id) != Some(stamp)
        })
        .map(|(id, stamp)| (id.clone(), *stamp))
        .collect();
    let mut removes: Vec<String> = loaded
        .iter()
        .filter(|(id, reloadable)| {
            **reloadable
                && !now.contains_key(*id)
                && !before.contains_key(*id)
                && !failed.contains_key(*id)
        })
        .map(|(id, _)| id.clone())
        .collect();
    adds.sort();
    removes.sort();
    (adds, removes)
}

/// Failures of reloads applied on a game thread, shared with the watcher.
static FAILED: Mutex<Option<HashMap<String, Stamp>>> = Mutex::new(None);

/// Called as a mission's state is about to be built, inside the session gate:
/// apply the queue now, so the mission runs the new plugins.
pub fn before_mission() {
    if PENDING.lock().unwrap_or_else(|p| p.into_inner()).is_empty() {
        return;
    }
    let mut failed = FAILED.lock().unwrap_or_else(|p| p.into_inner());
    apply_pending(failed.get_or_insert_with(HashMap::new));
}

/// Start the watcher thread: with `develop` (`hot_reload`), for changed,
/// added and removed plugin DLLs; with `toggle` (`live_toggle`), for plugins
/// switched on and off in their config files.
pub fn start_watcher(api: &'static Api, develop: bool, toggle: bool) {
    let _ = API.set(api);
    note_loaded();
    if toggle {
        *APPLIED_CONFIG.lock().unwrap_or_else(|p| p.into_inner()) = Some(startup_config());
    }
    let spawned = std::thread::Builder::new()
        .name("defiance hot reload".into())
        .spawn(move || watch(develop, toggle));
    let what = match (develop, toggle) {
        (true, true) => "the plugins directory and the config files",
        (true, false) => "the plugins directory",
        _ => "the config files",
    };
    match spawned {
        Ok(_) => crate::log::info(&format!(
            "hot reload: watching {what}; changes apply at the menu or when a mission starts or a save loads"
        )),
        Err(e) => crate::log::warn(&format!("hot reload: no watcher thread ({e})")),
    }
}

fn watch(develop: bool, toggle: bool) {
    // The last stamp seen per plugin, to wait until a copy has finished.
    let mut seen: HashMap<String, Stamp> = HashMap::new();
    let plugin_dir = crate::config::current()
        .filter(|_| develop)
        .map(|c| c.paths.plugin_dir.clone());
    let config_dir = crate::config::current()
        .filter(|_| toggle)
        .map(|c| c.paths.config_dir.clone());
    // Keep stable observations separate from the latest desired and
    // successfully applied config snapshots.
    let mut desired = if config_dir.is_some() {
        APPLIED_CONFIG
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .unwrap_or_else(config_now)
    } else {
        HashMap::new()
    };
    let mut read = desired.clone();
    let mut config_seen = config_dir.as_deref().map(config_stamps).unwrap_or_default();
    let mut settling = false;
    if let Some(config) = crate::config::current() {
        *known() = Some(
            config
                .catalog
                .entries
                .iter()
                .map(|e| e.id().to_ascii_lowercase())
                .collect(),
        );
    }
    // The directory as the previous poll saw it.
    let mut listed: HashMap<String, Stamp> =
        plugin_dir.as_deref().map(directory).unwrap_or_default();
    let mut untracked = false;
    // Polls in a row with no mission loaded.
    let mut at_menu = 0;
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if ensure_running().is_err() {
            return;
        }
        let failed_now = FAILED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .unwrap_or_default();
        for plugin in lifecycle::loaded()
            .iter()
            .filter(|p| develop && p.reloadable)
        {
            let now = lifecycle::stamp(&plugin.path);
            if now.is_some()
                && now != plugin.stamp
                && seen.get(&plugin.id) == Some(&now)
                && failed_now.get(&plugin.id) != Some(&now)
            {
                queue(Change::Reload(plugin.id.clone(), now));
            }
            seen.insert(plugin.id.clone(), now);
        }
        if let Some(dir) = plugin_dir.as_deref() {
            let now = directory(dir);
            let loaded: HashMap<String, bool> = lifecycle::loaded()
                .iter()
                .map(|p| (p.id.to_ascii_lowercase(), p.reloadable))
                .collect();
            let known_now = known().clone().unwrap_or_default();
            let failed_lower: HashMap<String, Stamp> = failed_now
                .iter()
                .map(|(id, stamp)| (id.to_ascii_lowercase(), *stamp))
                .collect();
            let (adds, removes) =
                directory_changes(&now, &listed, &loaded, &known_now, &failed_lower);
            for (id, stamp) in adds {
                queue(Change::Add(id, stamp));
            }
            for id in removes {
                queue(Change::Remove(id));
            }
            listed = now;
        }
        if let Some(dir) = config_dir.as_deref() {
            // Read the configuration when a file changed, and once more after,
            // so a value must read the same twice.
            let stamps = config_stamps(dir);
            let stamps_changed = stamps != config_seen;
            if stamps_changed || settling {
                settling = stamps_changed;
                config_seen = stamps;
                let now = config_now();
                if !settling {
                    let plugins = lifecycle::loaded();
                    let reloadable: HashSet<String> = plugins
                        .iter()
                        .filter(|plugin| plugin.reloadable)
                        .map(|plugin| plugin.id.to_ascii_lowercase())
                        .collect();
                    let loaded: HashSet<String> = plugins
                        .iter()
                        .map(|plugin| plugin.id.to_ascii_lowercase())
                        .collect();
                    for (id, state) in &now {
                        if loaded.contains(id)
                            && state.enabled != Some(false)
                            && !reloadable.contains(id)
                            && desired
                                .get(id)
                                .is_some_and(|previous| previous.settings != state.settings)
                        {
                            crate::log::info(&format!(
                                "hot reload: {id}'s settings changed; it can only load at startup, so they apply at the next restart"
                            ));
                        }
                    }
                    update_desired_config(&now, &read, &mut desired, &reloadable, queue);
                }
                read = now;
            }
        }
        if !crate::session::tracking() {
            if !untracked && !PENDING.lock().unwrap_or_else(|p| p.into_inner()).is_empty() {
                crate::log::warn(
                    "hot reload: Core does not report missions in this build; plugins are not reloaded",
                );
                untracked = true;
            }
            continue;
        }
        at_menu = if crate::session::at_menu() {
            at_menu + 1
        } else {
            0
        };
        if at_menu >= 2 && !PENDING.lock().unwrap_or_else(|p| p.into_inner()).is_empty() {
            // Decided again inside the gate: a mission that started since the
            // polls waits for the reload, or the reload waits for the menu.
            crate::session::GATE.while_at_menu(|| {
                let mut failed = FAILED.lock().unwrap_or_else(|p| p.into_inner());
                apply_pending(failed.get_or_insert_with(HashMap::new));
            });
        }
    }
}
