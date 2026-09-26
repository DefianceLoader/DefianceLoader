//! Declarative plugin discovery and startup planning.
//!
//! Discovery reads sidecar manifests instead of executing DLLs, so a disabled
//! or blocked managed plugin is never loaded. The planner then resolves
//! enablement, dependencies, version ranges and conflicts into concrete states
//! and a deterministic initialization order (stable plugin ID breaks ties).
//!
//! The planner is pure: it takes discovered nodes and the configuration
//! snapshot and returns a plan. The host does the loading, checking exported
//! identity against the manifest before calling `init`.

use super::config::builtin::{self, Builtin};
use super::config::Snapshot;
use super::manifest::{self, Dependency, Manifest};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

/// A discovered DLL and its sidecar, from the one catalog in `manifest`.
pub use super::manifest::Entry as Discovered;

/// What the host should do with one discovered node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Load and initialize it.
    Initialize,
    /// Explicitly disabled by configuration.
    Disabled { reason: String },
    /// Refused, with the reason; never loaded.
    Blocked { reason: String },
    /// Not a plugin to load (obsolete pilot, orphan manifest).
    Ignored { reason: String },
}

#[derive(Debug, Clone)]
pub struct Planned {
    pub id: String,
    pub dll: String,
    pub path: PathBuf,
    pub manifest: Option<Manifest>,
    pub builtin: Option<&'static Builtin>,
    pub legacy: bool,
    pub decision: Decision,
    /// Effective dependency IDs, for post-init failure propagation.
    pub depends: Vec<String>,
}

pub struct Plan {
    pub nodes: Vec<Planned>,
    /// Indices into `nodes`, in initialization order (only `Initialize`).
    pub order: Vec<usize>,
}

impl Plan {
    /// The legacy numeric feature IDs this plan will install, as a bit mask.
    /// Passed to the core plugin so it validates only those features' sites.
    pub fn feature_mask(&self) -> u64 {
        let mut mask = 0u64;
        for &index in &self.order {
            if let Some(builtin) = self.nodes[index].builtin {
                if builtin.feature != 0 && builtin.feature < 64 {
                    mask |= 1 << builtin.feature;
                }
            }
        }
        mask
    }
}

/// A fresh scan of `dir`, for tests that plan without a configuration;
/// startup plans from the snapshot's [`crate::config::Snapshot::catalog`].
#[cfg(test)]
pub fn discover(dir: &Path) -> (Vec<Discovered>, Vec<String>) {
    let catalog = manifest::catalog(dir);
    (catalog.entries, catalog.warnings)
}

/// Whether a plugin may stay active in multiplayer: its manifest says so. A
/// legacy plugin (no manifest) is not.
pub fn multiplayer_safe(node: &Planned) -> bool {
    node.manifest.as_ref().is_some_and(|m| m.multiplayer_safe)
}

/// Resolve the nodes into decisions and an initialization order.
pub fn plan(nodes: Vec<Discovered>, snapshot: &Snapshot) -> Plan {
    let mut planned: Vec<Planned> = nodes
        .into_iter()
        .map(|node| {
            let id = node.id();
            let legacy = node.is_legacy();
            let manifest = node.manifest.clone();
            let depends = manifest
                .as_ref()
                .map(|manifest| manifest.depends.iter().map(|dep| dep.id.clone()).collect())
                .unwrap_or_default();
            let decision = initial_decision(&node, snapshot);
            Planned {
                id,
                dll: node.dll,
                path: node.path,
                manifest,
                builtin: node.builtin,
                legacy,
                decision,
                depends,
            }
        })
        .collect();

    // Duplicate stable IDs: both sides are blocked.
    let mut by_id: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, node) in planned.iter().enumerate() {
        by_id
            .entry(node.id.to_ascii_lowercase())
            .or_default()
            .push(index);
    }
    for indices in by_id.values().filter(|indices| indices.len() > 1) {
        for &index in indices {
            planned[index].decision = Decision::Blocked {
                reason: format!("duplicate plugin ID `{}`", planned[index].id),
            };
        }
    }

    // Conflicts between two enabled plugins block both.
    let conflicts = conflict_pairs(&planned);
    for (a, b, reason) in &conflicts {
        for index in [*a, *b] {
            planned[index].decision = Decision::Blocked {
                reason: reason.clone(),
            };
        }
    }

    // Dependencies: missing, disabled, version-incompatible or blocked.
    propagate_dependencies(&mut planned, &by_id);

    // Cycles among still-initializing nodes.
    let cycle_nodes = find_cycles(&planned, &by_id);
    for indices in &cycle_nodes {
        let names: Vec<String> = indices.iter().map(|&i| planned[i].id.clone()).collect();
        let reason = format!("dependency cycle: {}", names.join(" -> "));
        for &index in indices {
            planned[index].decision = Decision::Blocked {
                reason: reason.clone(),
            };
        }
    }

    // A cycle also makes every transitive consumer unavailable.
    propagate_dependencies(&mut planned, &by_id);
    let order = order_active(&planned, &by_id);
    Plan {
        nodes: planned,
        order,
    }
}

fn initial_decision(node: &Discovered, snapshot: &Snapshot) -> Decision {
    if let Some(reason) = &node.ignored {
        return Decision::Ignored {
            reason: reason.clone(),
        };
    }
    if let Some(reason) = &node.error {
        return Decision::Blocked {
            reason: reason.clone(),
        };
    }
    if let Some(reason) = snapshot.startup_error() {
        return Decision::Blocked { reason };
    }
    if let Some(manifest) = &node.manifest {
        if manifest.abi != defiance_api::ABI_VERSION {
            return Decision::Blocked {
                reason: format!(
                    "manifest requires ABI {}, host provides {}",
                    manifest.abi,
                    defiance_api::ABI_VERSION
                ),
            };
        }
    }
    if snapshot.is_blocked(&node.id()) {
        return Decision::Blocked {
            reason: format!("invalid configuration blocks `{}`", node.id()),
        };
    }
    // Core is required infrastructure: always on, never a toggle.
    if node
        .builtin
        .is_some_and(|builtin| builtin.id == builtin::CORE_ID)
    {
        return Decision::Initialize;
    }
    let enabled = match &node.manifest {
        Some(manifest) => match snapshot.enabled(&manifest.id) {
            Some(enabled) => enabled,
            // A managed plugin outside the built-in schema uses its declared
            // `enabled` default until third-party settings are materialized.
            None => manifest_enabled_default(manifest),
        },
        None => true,
    };
    if enabled {
        Decision::Initialize
    } else {
        Decision::Disabled {
            reason: format!("`{}` enabled=false", node.id()),
        }
    }
}

fn manifest_enabled_default(manifest: &Manifest) -> bool {
    manifest
        .settings
        .iter()
        .find(|setting| setting.key.eq_ignore_ascii_case("enabled"))
        .map(|setting| {
            matches!(
                setting.default.trim().to_ascii_lowercase().as_str(),
                "true" | "yes" | "on" | "1"
            )
        })
        .unwrap_or(true)
}

fn conflict_pairs(planned: &[Planned]) -> Vec<(usize, usize, String)> {
    let mut pairs = Vec::new();
    for (index, node) in planned.iter().enumerate() {
        if node.decision != Decision::Initialize {
            continue;
        }
        let Some(manifest) = &node.manifest else {
            continue;
        };
        for other in &manifest.conflicts {
            let Some((other_index, other_node)) = planned
                .iter()
                .enumerate()
                .find(|(_, candidate)| candidate.id.eq_ignore_ascii_case(other))
            else {
                continue;
            };
            if other_index == index || other_node.decision != Decision::Initialize {
                continue;
            }
            pairs.push((
                index,
                other_index,
                format!(
                    "declared conflict between `{}` and `{}`",
                    node.id, other_node.id
                ),
            ));
        }
    }
    pairs
}

fn propagate_dependencies(planned: &mut [Planned], by_id: &BTreeMap<String, Vec<usize>>) {
    loop {
        let mut changed = false;
        for index in 0..planned.len() {
            if planned[index].decision != Decision::Initialize {
                continue;
            }
            let Some(manifest) = planned[index].manifest.clone() else {
                continue;
            };
            let mut block = None;
            for dependency in &manifest.depends {
                match by_id.get(&dependency.id.to_ascii_lowercase()) {
                    None => {
                        block = Some(format!("required `{}` is not installed", dependency.id));
                        break;
                    }
                    Some(indices) => {
                        // The dependency is the manifest-bearing node, or the
                        // first match if duplicates (already blocked).
                        let dependency_node = &planned[indices[0]];
                        match &dependency_node.decision {
                            Decision::Initialize => {}
                            Decision::Disabled { .. } => {
                                block = Some(format!("required `{}` is disabled", dependency.id));
                                break;
                            }
                            Decision::Ignored { .. } => {
                                block =
                                    Some(format!("required `{}` is not a plugin", dependency.id));
                                break;
                            }
                            Decision::Blocked { .. } => {
                                block = Some(format!("required `{}` is blocked", dependency.id));
                                break;
                            }
                        }
                        if !version_ok(dependency, dependency_node) {
                            block = Some(format!(
                                "required `{}` version does not satisfy the declared range",
                                dependency.id
                            ));
                            break;
                        }
                    }
                }
            }
            if let Some(reason) = block {
                planned[index].decision = Decision::Blocked { reason };
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

fn version_ok(dependency: &Dependency, provider: &Planned) -> bool {
    if dependency.min.is_none() && dependency.max.is_none() {
        return true;
    }
    match provider.manifest.as_ref().map(|manifest| manifest.version) {
        Some(version) => manifest::satisfies(version, dependency),
        // A legacy plugin has no declared version, so a range cannot be met.
        None => false,
    }
}

/// Nodes that are part of a dependency cycle, via DFS with a colour marking.
fn find_cycles(planned: &[Planned], by_id: &BTreeMap<String, Vec<usize>>) -> Vec<Vec<usize>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Grey,
        Black,
    }
    let mut marks = vec![Mark::White; planned.len()];
    let mut stack = Vec::new();
    let mut cycles: Vec<Vec<usize>> = Vec::new();

    fn visit(
        index: usize,
        planned: &[Planned],
        by_id: &BTreeMap<String, Vec<usize>>,
        marks: &mut [Mark],
        stack: &mut Vec<usize>,
        cycles: &mut Vec<Vec<usize>>,
    ) {
        marks[index] = Mark::Grey;
        stack.push(index);
        if let Some(manifest) = &planned[index].manifest {
            for dependency in &manifest.depends {
                let Some(indices) = by_id.get(&dependency.id.to_ascii_lowercase()) else {
                    continue;
                };
                let target = indices[0];
                if planned[target].decision != Decision::Initialize {
                    continue;
                }
                match marks[target] {
                    Mark::White => visit(target, planned, by_id, marks, stack, cycles),
                    Mark::Grey => {
                        if let Some(start) = stack.iter().position(|&i| i == target) {
                            cycles.push(stack[start..].to_vec());
                        }
                    }
                    Mark::Black => {}
                }
            }
        }
        stack.pop();
        marks[index] = Mark::Black;
    }

    for index in 0..planned.len() {
        if marks[index] == Mark::White && planned[index].decision == Decision::Initialize {
            visit(index, planned, by_id, &mut marks, &mut stack, &mut cycles);
        }
    }
    cycles
}

/// A deterministic initialization order: dependencies first, ties broken by
/// stable ID, with legacy (manifest-less) plugins after managed ones. Every
/// plugin that is not multiplayer-safe also comes after Core, which installs
/// the multiplayer guard it needs.
fn order_active(planned: &[Planned], by_id: &BTreeMap<String, Vec<usize>>) -> Vec<usize> {
    let active: Vec<usize> = (0..planned.len())
        .filter(|&i| planned[i].decision == Decision::Initialize)
        .collect();
    let active_set: BTreeSet<usize> = active.iter().copied().collect();
    let core = by_id
        .get(builtin::CORE_ID)
        .map(|indices| indices[0])
        .filter(|core| active_set.contains(core));
    let mut remaining: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for &index in &active {
        let mut needs = BTreeSet::new();
        if let Some(manifest) = &planned[index].manifest {
            for dependency in &manifest.depends {
                if let Some(indices) = by_id.get(&dependency.id.to_ascii_lowercase()) {
                    let target = indices[0];
                    if active_set.contains(&target) && target != index {
                        needs.insert(target);
                    }
                }
            }
        }
        if let Some(core) = core.filter(|&core| core != index) {
            if !multiplayer_safe(&planned[index]) {
                needs.insert(core);
            }
        }
        remaining.insert(index, needs);
    }

    let mut order = Vec::new();
    while !remaining.is_empty() {
        // Ready nodes have no unmet dependencies. Sort managed before legacy,
        // then by ID, then by index for full determinism.
        let ready: Vec<usize> = remaining
            .iter()
            .filter(|(_, needs)| needs.is_empty())
            .map(|(&index, _)| index)
            .collect();
        if ready.is_empty() {
            // A cycle slipped through; stop rather than loop forever.
            break;
        }
        let mut chosen: Vec<usize> = ready;
        chosen.sort_by(|&a, &b| {
            (planned[a].legacy, &planned[a].id, a).cmp(&(planned[b].legacy, &planned[b].id, b))
        });
        let pick = chosen[0];
        remaining.remove(&pick);
        for needs in remaining.values_mut() {
            needs.remove(&pick);
        }
        order.push(pick);
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_loads_in_place_and_features_from_copies() {
        // The feature plugins find Core's module by its file name.
        for builtin in builtin::BUILTINS {
            let manifest =
                manifest::parse(&manifest::render_builtin(builtin), builtin.dll).unwrap();
            let node = Planned {
                id: builtin.id.into(),
                dll: builtin.dll.into(),
                path: PathBuf::from(builtin.dll),
                manifest: Some(manifest),
                builtin: Some(builtin),
                legacy: false,
                decision: Decision::Initialize,
                depends: Vec::new(),
            };
            assert_eq!(
                crate::plugin::loads_from_copy(&node),
                builtin.id != builtin::CORE_ID,
                "{}",
                builtin.id
            );
        }
    }
    use crate::config::builtin;
    use crate::config::parse::parse;
    use crate::config::paths::Paths;
    use crate::config::snapshot::{GroupInput, Snapshot};

    fn snapshot(enabled: &[(&str, bool)]) -> Snapshot {
        let text = enabled
            .iter()
            .map(|(id, on)| format!("[{id}]\nenabled = {on}\n"))
            .collect::<String>();
        let inputs = vec![
            GroupInput::present(
                "core",
                PathBuf::from("core.ini"),
                "[loader]\nwait = 60\n[logging]\nlevel = info\n",
            ),
            GroupInput::present("infantry", PathBuf::from("infantry.ini"), &text),
            GroupInput::present("weapons", PathBuf::from("weapons.ini"), ""),
            GroupInput::present("diagnostics", PathBuf::from("diagnostics.ini"), ""),
        ];
        let paths = Paths::resolve(&PathBuf::from("C:/Game/bin"), &parse("")).0;
        Snapshot::build(paths, &inputs, &parse(""))
    }

    fn node(dll: &str) -> Discovered {
        let builtin = builtin::by_dll(dll);
        let manifest = builtin
            .map(|builtin| manifest::parse(&manifest::render_builtin(builtin), dll).unwrap());
        Discovered {
            dll: dll.into(),
            path: PathBuf::from(dll),
            builtin,
            manifest,
            error: None,
            ignored: None,
        }
    }

    fn with_manifest(dll: &str, json: &str) -> Discovered {
        let builtin = builtin::by_dll(dll);
        let manifest = manifest::parse(json, dll).unwrap();
        Discovered {
            dll: dll.into(),
            path: PathBuf::from(dll),
            builtin,
            manifest: Some(manifest),
            error: None,
            ignored: None,
        }
    }

    fn builtin_json(id: &str) -> String {
        let builtin = builtin::find(id).unwrap();
        manifest::render_builtin(builtin)
    }

    /// A manifest with the real `ABI_VERSION`, so a version bump cannot make a
    /// rejection test pass because of an accidental ABI mismatch.
    fn test_manifest(
        id: &str,
        dll: &str,
        depends: &[(&str, Option<&str>)],
        conflicts: &[&str],
    ) -> String {
        let depends: Vec<String> = depends
            .iter()
            .map(|(id, min)| match min {
                Some(min) => format!(r#"{{"id":"{id}","min":"{min}"}}"#),
                None => format!(r#"{{"id":"{id}"}}"#),
            })
            .collect();
        let conflicts: Vec<String> = conflicts.iter().map(|id| format!("\"{id}\"")).collect();
        format!(
            r#"{{"schema":1,"id":"{id}","dll":"{dll}","version":"1.0.0","abi":{},"group":"g","settings":[],"depends":[{}],"conflicts":[{}]}}"#,
            defiance_api::ABI_VERSION,
            depends.join(","),
            conflicts.join(","),
        )
    }

    #[test]
    fn dependencies_are_ordered_before_dependents() {
        let nodes = vec![
            node("defiance_plugin_feature_movement.dll"),
            node("defiance_plugin_feature_selection.dll"),
            node("defiance_plugin_feature_posture.dll"),
            node("defiance_plugin_core.dll"),
        ];
        let plan = plan(nodes, &snapshot(&[]));
        let ids: Vec<String> = plan
            .order
            .iter()
            .map(|&i| plan.nodes[i].id.clone())
            .collect();
        let position = |id: &str| ids.iter().position(|c| c == id).unwrap();
        assert!(position(builtin::CORE_ID) < position("defiance.selection"));
        assert!(position("defiance.selection") < position("defiance.posture"));
        assert!(position("defiance.posture") < position("defiance.movement"));
    }

    #[test]
    fn a_disabled_dependency_blocks_its_dependents_but_not_unrelated_plugins() {
        let nodes = vec![
            node("defiance_plugin_core.dll"),
            node("defiance_plugin_feature_selection.dll"),
            node("defiance_plugin_feature_movement.dll"),
            node("defiance_plugin_feature_firing.dll"),
            node("defiance_plugin_feature_diagnostics.dll"),
        ];
        let plan = plan(nodes, &snapshot(&[("defiance.selection", false)]));
        let state = |id: &str| {
            plan.nodes
                .iter()
                .find(|n| n.id == id)
                .unwrap()
                .decision
                .clone()
        };
        assert!(matches!(
            state("defiance.selection"),
            Decision::Disabled { .. }
        ));
        assert!(matches!(
            state("defiance.movement"),
            Decision::Blocked { .. }
        ));
        assert!(matches!(state("defiance.firing"), Decision::Blocked { .. }));
        assert!(matches!(
            state("defiance.diagnostics"),
            Decision::Initialize
        ));
        let ids: Vec<String> = plan
            .order
            .iter()
            .map(|&i| plan.nodes[i].id.clone())
            .collect();
        assert!(!ids.contains(&"defiance.movement".to_string()));
    }

    #[test]
    fn missing_core_blocks_its_dependents() {
        let nodes = vec![node("defiance_plugin_feature_selection.dll")];
        let plan = plan(nodes, &snapshot(&[]));
        assert!(matches!(plan.nodes[0].decision, Decision::Blocked { .. }));
        assert!(plan.order.is_empty());
    }

    #[test]
    fn a_missing_manifest_for_a_builtin_is_a_packaging_error() {
        let mut node = node("defiance_plugin_feature_selection.dll");
        node.error = Some("... is a managed built-in but has no sidecar".into());
        let plan = plan(vec![node], &snapshot(&[]));
        assert!(matches!(plan.nodes[0].decision, Decision::Blocked { .. }));
    }

    #[test]
    fn a_manifest_identity_mismatch_blocks_the_plugin() {
        let mut node = node("defiance_plugin_feature_selection.dll");
        node.error = Some("packaging error: manifest version 9.9.9 is not 0.3.0".into());
        let plan = plan(vec![node], &snapshot(&[]));
        assert!(matches!(plan.nodes[0].decision, Decision::Blocked { .. }));
    }

    #[test]
    fn duplicate_ids_block_both_sides() {
        let first = with_manifest(
            "defiance_plugin_feature_selection.dll",
            &builtin_json("defiance.selection"),
        );
        let mut second = first.clone();
        second.dll = "other.dll".into();
        second.path = PathBuf::from("other.dll");
        second.builtin = None;
        let plan = plan(vec![first, second], &snapshot(&[]));
        assert!(plan
            .nodes
            .iter()
            .all(|n| matches!(n.decision, Decision::Blocked { .. })));
    }

    #[test]
    fn a_cycle_is_reported_with_the_involved_ids() {
        let a = test_manifest("a.p", "a.dll", &[("b.p", None)], &[]);
        let b = test_manifest("b.p", "b.dll", &[("a.p", None)], &[]);
        let plan = plan(
            vec![with_manifest("a.dll", &a), with_manifest("b.dll", &b)],
            &snapshot(&[]),
        );
        for node in &plan.nodes {
            match &node.decision {
                Decision::Blocked { reason } => {
                    assert!(reason.contains("cycle"), "{reason}");
                    assert!(reason.contains("a.p") && reason.contains("b.p"), "{reason}");
                }
                other => panic!("expected blocked, got {other:?}"),
            }
        }
        assert!(plan.order.is_empty());
    }

    #[test]
    fn a_declared_conflict_blocks_both_sides() {
        let a = test_manifest("a.p", "a.dll", &[], &["b.p"]);
        let b = test_manifest("b.p", "b.dll", &[], &[]);
        let plan = plan(
            vec![with_manifest("a.dll", &a), with_manifest("b.dll", &b)],
            &snapshot(&[]),
        );
        assert!(plan
            .nodes
            .iter()
            .all(|n| matches!(n.decision, Decision::Blocked { .. })));
    }

    #[test]
    fn a_version_mismatch_blocks_the_dependent() {
        let core = test_manifest("defiance.core", "defiance_plugin_core.dll", &[], &[]);
        let dependent = test_manifest("x.p", "x.dll", &[("defiance.core", Some("9.0.0"))], &[]);
        let plan = plan(
            vec![
                with_manifest("defiance_plugin_core.dll", &core),
                with_manifest("x.dll", &dependent),
            ],
            &snapshot(&[]),
        );
        let x = plan.nodes.iter().find(|n| n.id == "x.p").unwrap();
        assert!(
            matches!(x.decision, Decision::Blocked { .. }),
            "{:?}",
            x.decision
        );
    }

    #[test]
    fn a_legacy_plugin_loads_after_managed_ones_with_no_guarantees() {
        let legacy = node("third_party.dll");
        let core = node("defiance_plugin_core.dll");
        let diagnostics = node("defiance_plugin_feature_diagnostics.dll");
        let plan = plan(vec![legacy, diagnostics, core], &snapshot(&[]));
        let ids: Vec<String> = plan
            .order
            .iter()
            .map(|&i| plan.nodes[i].id.clone())
            .collect();
        assert_eq!(*ids.last().unwrap(), "third_party");
        let node = plan.nodes.iter().find(|n| n.id == "third_party").unwrap();
        assert!(node.legacy);
        assert_eq!(node.decision, Decision::Initialize);
    }

    #[test]
    fn order_is_deterministic_for_equal_ranks() {
        let nodes = vec![
            node("b_plugin.dll"),
            node("a_plugin.dll"),
            node("c_plugin.dll"),
        ];
        let first = plan(nodes.clone(), &snapshot(&[]));
        let second = plan(nodes, &snapshot(&[]));
        let ids = |plan: &Plan| {
            plan.order
                .iter()
                .map(|&i| plan.nodes[i].id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&first), ids(&second));
        assert_eq!(ids(&first), vec!["a_plugin", "b_plugin", "c_plugin"]);
    }

    #[test]
    fn a_disabled_plugin_is_never_in_the_initialization_order() {
        let nodes = vec![
            node("defiance_plugin_core.dll"),
            node("defiance_plugin_feature_selection.dll"),
            node("defiance_plugin_feature_diagnostics.dll"),
        ];
        let plan = plan(nodes, &snapshot(&[("defiance.selection", false)]));
        assert!(plan.nodes.iter().any(
            |n| n.id == "defiance.selection" && matches!(n.decision, Decision::Disabled { .. })
        ));
        assert!(!plan
            .order
            .iter()
            .any(|&i| plan.nodes[i].id == "defiance.selection"));
    }

    #[test]
    fn renaming_a_dll_and_manifest_changes_no_dependency_result() {
        // Dependencies are on the stable manifest ID, not the filename.
        let renamed = |dll: &str| {
            let mut node = node("defiance_plugin_feature_selection.dll");
            node.dll = dll.into();
            node.path = PathBuf::from(dll);
            node
        };
        let original = vec![
            node("defiance_plugin_core.dll"),
            node("defiance_plugin_feature_selection.dll"),
            node("defiance_plugin_feature_movement.dll"),
        ];
        let renamed_nodes = vec![
            node("defiance_plugin_core.dll"),
            renamed("whatever.dll"),
            node("defiance_plugin_feature_movement.dll"),
        ];
        let ids = |plan: &Plan| {
            plan.order
                .iter()
                .map(|&i| plan.nodes[i].id.clone())
                .collect::<Vec<_>>()
        };
        let first = plan(original, &snapshot(&[]));
        let second = plan(renamed_nodes, &snapshot(&[]));
        assert_eq!(ids(&first), ids(&second));
        assert!(second
            .order
            .iter()
            .all(|&i| second.nodes[i].id != "whatever"));
    }

    #[test]
    fn discovery_reads_manifests_without_loading_dlls() {
        let base = std::env::temp_dir().join(format!(
            "defiance-plan-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let builtin = builtin::find("defiance.core").unwrap();
        std::fs::write(base.join("defiance_plugin_core.dll"), b"not a real dll").unwrap();
        std::fs::write(
            base.join("defiance_plugin_core.plugin.json"),
            manifest::render_builtin(builtin),
        )
        .unwrap();
        std::fs::write(base.join("stray.plugin.json"), "{}").unwrap();
        let (nodes, warnings) = discover(&base);
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].manifest.is_some());
        assert!(nodes[0].error.is_none());
        assert!(warnings.iter().any(|w| w.contains("stray")));
    }

    #[test]
    fn discovery_blocks_a_managed_builtin_without_a_manifest() {
        let base = std::env::temp_dir().join(format!(
            "defiance-plan-missing-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("defiance_plugin_feature_selection.dll"), b"x").unwrap();
        let (nodes, _) = discover(&base);
        assert!(nodes[0].error.is_some());
        let plan = plan(nodes, &snapshot(&[]));
        assert!(matches!(plan.nodes[0].decision, Decision::Blocked { .. }));
    }
}
