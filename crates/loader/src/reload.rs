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
//! plugin whose manifest forbids it (`hot_reload: false`, Core), one whose
//! settings changed (they were resolved at startup), and one that is no longer
//! enabled.
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
use crate::lifecycle::{self, Loaded};
use crate::plan::Decision;
use defiance_api::Api;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// Tell Core a feature plugin's spans are gone, so it installs them again.
fn forget_feature(feature: u32) {
    if let Some(core) = lifecycle::loaded()
        .into_iter()
        .find(|p| p.id.eq_ignore_ascii_case(crate::config::builtin::CORE_ID))
    {
        lifecycle::call_export(core.module, c"defiance_feature_removed_v1", feature);
    }
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
    let result = reload_group(api, id);
    crate::multiplayer::refresh();
    result
}

fn reload_group(api: &'static Api, id: &str) -> Result<(), String> {
    let config = crate::config::current().ok_or("no configuration")?;
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

    // The files as they are now, planned against the startup configuration.
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
        let before = config
            .catalog
            .entries
            .iter()
            .find(|e| e.id().eq_ignore_ascii_case(&plugin.id))
            .and_then(|e| e.manifest.as_ref())
            .map(|m| &m.settings);
        if before != node.manifest.as_ref().map(|m| &m.settings) {
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
        lifecycle::unload(plugin.owner, forget_feature, retain).map_err(|e| {
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
        crate::plugin::load(node, api, lifecycle::next_owner(), plan.feature_mask())
            .map_err(|e| format!("{} failed to load again: {}", plugin.id, e.reason))?;
        crate::log::info(&format!("hot reload: {} loaded again", plugin.id));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(owner: usize, id: &str) -> Loaded {
        Loaded {
            owner,
            id: id.into(),
            name: id.into(),
            path: Default::default(),
            shadow: Default::default(),
            module: 0,
            code: Vec::new(),
            stop: None,
            feature: 0,
            reloadable: true,
            stamp: None,
            multiplayer_safe: true,
        }
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
    fn only_a_settled_change_of_enabled_switches_a_plugin() {
        let map = |pairs: &[(&str, bool)]| -> HashMap<String, bool> {
            pairs.iter().map(|(id, on)| (id.to_string(), *on)).collect()
        };
        let loaded: HashSet<String> = ["a".to_string(), "b".to_string()].into();
        // At startup: a and b on (loaded), c off, d on but it failed to load.
        let applied = map(&[("a", true), ("b", true), ("c", false), ("d", true)]);
        // a switched off and c on, read once: not yet.
        let first = map(&[("a", false), ("b", true), ("c", true), ("d", true)]);
        assert_eq!(
            toggles(&first, &applied, &applied, &loaded),
            (vec![], vec![])
        );
        // Read the same again: switched. d, unchanged, is not retried.
        let (on, off) = toggles(&first, &first, &applied, &loaded);
        assert_eq!((on, off), (vec!["c".to_string()], vec!["a".to_string()]));
        // A plugin new since startup (no applied value) is not a toggle.
        let new = map(&[("e", true)]);
        assert_eq!(toggles(&new, &new, &applied, &loaded), (vec![], vec![]));
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

/// A change waiting for the menu or a mission's start.
#[derive(Clone, Debug, PartialEq)]
enum Change {
    /// A loaded plugin's DLL changed; the stamp seen.
    Reload(String, Stamp),
    /// A plugin appeared in the directory; its DLL's stamp.
    Add(String, Stamp),
    /// A loaded plugin's files are gone.
    Remove(String),
    /// A plugin switched on in its config file.
    Enable(String),
    /// A plugin switched off in its config file.
    Disable(String),
}

impl Change {
    fn id(&self) -> &str {
        match self {
            Change::Reload(id, _)
            | Change::Add(id, _)
            | Change::Remove(id)
            | Change::Enable(id)
            | Change::Disable(id) => id,
        }
    }
    fn stamp(&self) -> Stamp {
        match self {
            Change::Reload(_, stamp) | Change::Add(_, stamp) => *stamp,
            Change::Remove(_) | Change::Enable(_) | Change::Disable(_) => None,
        }
    }
    fn describe(&self) -> &'static str {
        match self {
            Change::Reload(..) => "changed; it reloads",
            Change::Add(..) => "added; it loads",
            Change::Remove(_) => "removed; it unloads",
            Change::Enable(_) => "switched on; it loads",
            Change::Disable(_) => "switched off; it unloads",
        }
    }
}

/// Changes waiting to be applied.
static PENDING: Mutex<Vec<Change>> = Mutex::new(Vec::new());
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

/// Queue `change`, replacing an earlier one for the same plugin.
fn queue(change: Change) {
    let mut pending = PENDING.lock().unwrap_or_else(|p| p.into_inner());
    if pending.contains(&change) {
        return;
    }
    pending.retain(|c| !c.id().eq_ignore_ascii_case(change.id()));
    crate::log::info(&format!(
        "hot reload: {} {} at the menu or when a mission starts or a save loads",
        change.id(),
        change.describe()
    ));
    pending.push(change);
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
    let queued = std::mem::take(&mut *PENDING.lock().unwrap_or_else(|p| p.into_inner()));
    for change in queued {
        let current = lifecycle::loaded()
            .into_iter()
            .find(|p| p.id.eq_ignore_ascii_case(change.id()));
        let result = match &change {
            // A reload of an earlier one may already have reloaded this one as
            // a dependant.
            Change::Reload(id, stamp) if current.as_ref().is_none_or(|p| p.stamp != *stamp) => {
                reload(api, id)
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
            Change::Enable(id) if current.is_none() => enable(api, id),
            Change::Disable(id) if current.is_some() => disable(api, id),
            _ => Ok(()),
        };
        note_loaded();
        if let Err(e) = result {
            crate::log::warn(&format!("hot reload: {e}"));
            failed.insert(change.id().to_ascii_lowercase(), change.stamp());
        }
    }
}

/// Load a plugin that appeared in the plugins directory after startup.
pub fn add(api: &'static Api, id: &str) -> Result<(), String> {
    let result = add_plugin(api, id, false);
    crate::multiplayer::refresh();
    result
}

/// Load a plugin switched on in its config file, then the plugins that were
/// unloaded because they needed it, where they can load now.
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

/// Load again the plugins loaded earlier in the session that depend on `id`,
/// are not loaded, and can load now.
fn restore_dependants(api: &'static Api, id: &str) {
    let config = crate::config::refresh_later();
    let plan = crate::plan::plan(config.catalog.entries.clone(), config);
    let loaded = lifecycle::loaded();
    let ever = ever_loaded().clone().unwrap_or_default();
    for node in &plan.nodes {
        let depends_on_it = node
            .manifest
            .as_ref()
            .is_some_and(|m| m.depends.iter().any(|d| d.id.eq_ignore_ascii_case(id)));
        if !depends_on_it
            || !matches!(node.decision, Decision::Initialize)
            || loaded.iter().any(|p| p.id.eq_ignore_ascii_case(&node.id))
            || !ever.contains(&node.id.to_ascii_lowercase())
        {
            continue;
        }
        match add_plugin(api, &node.id, true) {
            Ok(()) => crate::log::info(&format!("hot reload: {} loaded again", node.id)),
            Err(e) => crate::log::warn(&format!("hot reload: {e}")),
        }
    }
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
    if lifecycle::loaded()
        .iter()
        .any(|p| p.id.eq_ignore_ascii_case(id))
    {
        return Err(format!("{id} is already loaded"));
    }
    // The configuration as the files are now: the new plugin's settings.
    let config = crate::config::refresh_later();
    let plan = crate::plan::plan(config.catalog.entries.clone(), config);
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
    let loaded = lifecycle::loaded();
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
    crate::plugin::load(node, api, lifecycle::next_owner(), plan.feature_mask())
        .map_err(|e| format!("{id} failed to load: {}", e.reason))?;
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
        lifecycle::unload(plugin.owner, forget_feature, retain)
            .map_err(|e| format!("{} could not be unloaded ({e})", plugin.id))?;
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
                match crate::plugin::load(node, api, lifecycle::next_owner(), plan.feature_mask()) {
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

/// The plugins to switch on and off (lowercase IDs): `enabled` read the same
/// on this poll (`now`) and the previous one (`before`), and different from
/// the value last acted on (`applied`), for a plugin that is not loaded (on)
/// or loaded (off). A plugin with no value in `applied` is new and not a
/// toggle. Returns (on, off).
fn toggles(
    now: &HashMap<String, bool>,
    before: &HashMap<String, bool>,
    applied: &HashMap<String, bool>,
    loaded: &HashSet<String>,
) -> (Vec<String>, Vec<String>) {
    let (mut on, mut off) = (Vec::new(), Vec::new());
    for (id, &value) in now {
        let settled = before.get(id) == Some(&value);
        let changed = applied.get(id).is_some_and(|&last| last != value);
        if !settled || !changed {
            continue;
        }
        if value && !loaded.contains(id) {
            on.push(id.clone());
        } else if !value && loaded.contains(id) {
            off.push(id.clone());
        }
    }
    on.sort();
    off.sort();
    (on, off)
}

/// Every plugin's `enabled` as the configuration files say now (read-only),
/// by lowercase ID; Core, which is always on, and legacy plugins left out.
fn enabled_now() -> HashMap<String, bool> {
    let config = crate::config::inspect(&crate::config::game_dir());
    config
        .catalog
        .entries
        .iter()
        .map(|entry| entry.id())
        .filter(|id| !id.eq_ignore_ascii_case(crate::config::builtin::CORE_ID))
        .filter_map(|id| config.enabled(&id).map(|on| (id.to_ascii_lowercase(), on)))
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
    // `enabled` as last acted on (the startup values), as last read, and the
    // config files' stamps when last read.
    let mut applied: HashMap<String, bool> = HashMap::new();
    if let Some(config) = crate::config::current().filter(|_| toggle) {
        for entry in &config.catalog.entries {
            if let Some(on) = config.enabled(&entry.id()) {
                applied.insert(entry.id().to_ascii_lowercase(), on);
            }
        }
    }
    let mut read: HashMap<String, bool> = applied.clone();
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
            if stamps != config_seen || settling {
                settling = stamps != config_seen;
                config_seen = stamps;
                let now = enabled_now();
                let loaded: HashSet<String> = lifecycle::loaded()
                    .iter()
                    .map(|p| p.id.to_ascii_lowercase())
                    .collect();
                let (on, off) = toggles(&now, &read, &applied, &loaded);
                for id in on {
                    applied.insert(id.clone(), true);
                    queue(Change::Enable(id));
                }
                for id in off {
                    applied.insert(id.clone(), false);
                    queue(Change::Disable(id));
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
