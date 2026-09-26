//! The authoritative built-in registry: stable plugin IDs, DLL basenames,
//! config group, dependencies and the legacy numeric feature IDs. Everything
//! that must agree about a built-in — startup planning, defaults generation and
//! (in the next milestone) the generated sidecar manifests — is derived from
//! this one table, so feature IDs and dependencies cannot drift apart.
//!
//! Third-party plugins declare the same facts in their own sidecar manifest.

use super::schema::{Restart, SettingDecl, ValueType};

/// The required infrastructure plugin. It is not a gameplay toggle: its
/// availability is separate from any feature's enabled state.
pub const CORE_ID: &str = "defiance.core";

/// Config groups, by player-facing purpose. A group is a filename under the
/// config directory; the sections inside are the plugin IDs.
pub const GROUPS: [&str; 4] = ["core", "infantry", "weapons", "diagnostics"];

/// The settings every gameplay feature gets. A feature declares more only
/// where its implementation reads them (`EXTRA_SETTINGS`).
pub const ENABLED: SettingDecl = SettingDecl {
    key: "enabled",
    ty: ValueType::Bool,
    default: "true",
    description: "Enable or disable this feature. Restart required.",
    restart: Restart::Startup,
    sensitive: false,
};

/// A built-in feature or infrastructure plugin.
#[derive(Debug, Clone, Copy)]
pub struct Builtin {
    /// Stable plugin ID, used as the INI section and the dependency key.
    pub id: &'static str,
    /// Relative DLL file name in the discovery directory.
    pub dll: &'static str,
    pub version: &'static str,
    /// Config group, i.e. the filename stem under the config directory.
    pub group: &'static str,
    /// One line for the generated defaults file.
    pub summary: &'static str,
    /// Hard dependencies by stable plugin ID.
    pub depends: &'static [&'static str],
    /// The legacy numeric feature ID used by the shared runtime, or 0 for
    /// infrastructure.
    pub feature: u32,
    /// Whether the plugin can stay active in multiplayer: it changes nothing
    /// another player's game would need to match (display only, or nothing at
    /// all). Anything else blocks multiplayer while it is active.
    pub multiplayer_safe: bool,
}

/// Whether a built-in may be unloaded while the game runs: every feature
/// plugin, not Core, which holds the shared payload and the guards.
pub fn hot_reload(builtin: &Builtin) -> bool {
    builtin.id != CORE_ID
}

/// Standalone plugins shipped with the loader that change gameplay: their
/// manifests may not declare `multiplayer_safe`, so an edit cannot let them
/// online. The built-ins take their flag from this table instead.
pub const NOT_MULTIPLAYER_SAFE: &[&str] = &[
    "defiance.expanded-ammo-menu",
    "defiance.regroup",
    "defiance.squad-management-scroll",
    "defiance.unit-inspection",
];

pub const BUILTINS: &[Builtin] = &[
    Builtin {
        id: CORE_ID,
        dll: "defiance_plugin_core.dll",
        version: "0.3.0",
        group: "core",
        summary: "Required support for the infantry and weapon features. Core has no enabled toggle; disable individual features instead.",
        depends: &[],
        feature: 0,
        multiplayer_safe: true,
    },
    Builtin {
        id: "defiance.selection",
        dll: "defiance_plugin_feature_selection.dll",
        version: "0.5.0",
        group: "infantry",
        summary: "Select individual soldiers within a squad and show which soldiers are selected. Disabling this also prevents posture, movement, attack, garrison, firing, ammunition, expanded ammo menu and regroup from loading. Pickup and squad-management scrolling can remain enabled. Restart required.",
        depends: &[CORE_ID],
        feature: 2,
        multiplayer_safe: false,
    },
    Builtin {
        id: "defiance.posture",
        dll: "defiance_plugin_feature_posture.dll",
        version: "0.3.1",
        group: "infantry",
        summary: "Give selected soldiers their own standing, crouching or prone posture instead of changing the entire squad. Requires selection. Disabling this also disables individual movement; it does not remove the game's normal squad posture controls. Restart required.",
        depends: &["defiance.selection"],
        feature: 4,
        multiplayer_safe: false,
    },
    Builtin {
        id: "defiance.movement",
        dll: "defiance_plugin_feature_movement.dll",
        version: "0.3.0",
        group: "infantry",
        summary: "Move only the selected soldiers when part of a squad is selected, leaving unselected squadmates in place. Requires selection and posture. Disable to use the game's normal movement orders while keeping the other enabled controls. Restart required.",
        depends: &["defiance.selection", "defiance.posture"],
        feature: 3,
        multiplayer_safe: false,
    },
    Builtin {
        id: "defiance.attack",
        dll: "defiance_plugin_feature_attack.dll",
        version: "0.4.0",
        group: "infantry",
        summary: "Direct an attack order to the selected soldiers instead of every member of their squad. Requires selection. Disabling this restores normal squad attack orders; it does not disable combat or the firing-mode setting. Restart required.",
        depends: &["defiance.selection"],
        feature: 8,
        multiplayer_safe: false,
    },
    Builtin {
        id: "defiance.garrison",
        dll: "defiance_plugin_feature_garrison.dll",
        version: "0.3.1",
        group: "infantry",
        summary: "Send selected soldiers into buildings without sending their unselected squadmates; support exit orders for soldiers occupying the building. Requires selection. Disable to keep the game's normal building-entry and exit behavior. Restart required.",
        depends: &["defiance.selection"],
        feature: 9,
        multiplayer_safe: false,
    },
    Builtin {
        id: "defiance.firing",
        dll: "defiance_plugin_feature_firing.dll",
        version: "0.3.0",
        group: "weapons",
        summary: "Change firing mode for selected soldiers without changing their unselected squadmates. Select the whole squad to change everyone. Requires selection. Disabling this restores normal squad firing-mode controls; ammunition toggles are a separate setting. Restart required.",
        depends: &[CORE_ID, "defiance.selection"],
        feature: 5,
        multiplayer_safe: false,
    },
    Builtin {
        id: "defiance.ammunition",
        dll: "defiance_plugin_feature_ammunition.dll",
        version: "0.4.0",
        group: "weapons",
        summary: "Use the in-mission ammo panel to enable or disable weapons/ammo for selected soldiers. Show relevant weapons and selected-user counts, including mixed on/off states. Select the whole squad to apply a toggle to everyone. Individual overrides support only the first eight ammo slots; later slots need whole-squad selection. Requires selection. Expanded ammo menu also requires this feature; out-of-mission squad scrolling does not. Restart required.",
        depends: &["defiance.selection"],
        feature: 6,
        multiplayer_safe: false,
    },
    Builtin {
        id: "defiance.pickup",
        dll: "defiance_plugin_feature_pickup.dll",
        version: "0.3.0",
        group: "weapons",
        summary: "Prefer individually selected soldiers when choosing who picks up a weapon, and rotate replacement choices across eligible soldiers on repeated pickups. Disable to use the game's original pickup choice. Can remain enabled without the individual-selection feature. Restart required.",
        depends: &[CORE_ID],
        feature: 1,
        multiplayer_safe: false,
    },
    Builtin {
        id: "defiance.diagnostics",
        dll: "defiance_plugin_feature_diagnostics.dll",
        version: "0.3.0",
        group: "diagnostics",
        summary: "Collect extra troubleshooting information about soldier selection and squad behavior. Adds no player controls. You can disable this without disabling gameplay features. Restart required.",
        depends: &[CORE_ID],
        feature: 7,
        multiplayer_safe: false,
    },
    Builtin {
        id: "defiance.preview-weapon",
        dll: "defiance_plugin_feature_preview_weapon.dll",
        version: "0.1.0",
        group: "infantry",
        summary: "Show the first matching weapon instead of the last matching weapon in squad previews, including in-mission unit info and out-of-mission squad management. Does not track later changes to the weapon a soldier is holding. Disable to restore the game's original preview choice. Works independently of selection and does not change combat. Restart required.",
        depends: &[CORE_ID],
        feature: 10,
        multiplayer_safe: true,
    },
];

/// The legacy numeric feature ID for `id`, or 0 for infrastructure and unknown
/// IDs.
pub fn feature_id(id: &str) -> u32 {
    BUILTINS
        .iter()
        .find(|builtin| builtin.id.eq_ignore_ascii_case(id))
        .map(|builtin| builtin.feature)
        .unwrap_or(0)
}

/// The built-in with this stable ID, case-insensitively.
pub fn find(id: &str) -> Option<&'static Builtin> {
    BUILTINS
        .iter()
        .find(|builtin| builtin.id.eq_ignore_ascii_case(id))
}

/// The built-in whose DLL basename is `dll`, case-insensitively.
pub fn by_dll(dll: &str) -> Option<&'static Builtin> {
    BUILTINS
        .iter()
        .find(|builtin| builtin.dll.eq_ignore_ascii_case(dll))
}

/// The built-ins that call themselves a gameplay feature (have `enabled`).
pub fn features() -> impl Iterator<Item = &'static Builtin> {
    BUILTINS.iter().filter(|builtin| builtin.feature != 0)
}

/// Selection's squad TAB modifier: held with TAB on a selected building, each
/// press narrows the selection to one squad's occupants. Core writes its key
/// into the game payload when selection installs.
pub const SQUAD_TAB_MODIFIER: SettingDecl = SettingDecl {
    key: "squad_tab_modifier",
    ty: ValueType::Choice(&["ctrl", "shift", "off"]),
    default: "ctrl",
    description: "Hold this with TAB on a selected building to select one squad's occupants at a time; plain TAB selects all occupants. ctrl, shift or off. Restart required.",
    restart: Restart::Startup,
    sensitive: false,
};

/// Selection's marquee: whole squads, as the base game selects them, or the
/// soldiers inside the box. Core writes the mode into the logic payload when
/// selection installs (`patch/region-individual.asm`).
pub const MARQUEE: SettingDecl = SettingDecl {
    key: "marquee",
    ty: ValueType::Choice(&["squads", "soldiers"]),
    default: "squads",
    description: "What dragging a box selects. squads: every squad with a soldier or its icon inside, as in the base game; hold Ctrl to select the soldiers inside instead. soldiers: only the soldiers inside. Restart required.",
    restart: Restart::Startup,
    sensitive: false,
};

/// Settings a gameplay feature declares besides `enabled`.
const EXTRA_SETTINGS: &[(&str, SettingDecl)] = &[
    ("defiance.selection", SQUAD_TAB_MODIFIER),
    ("defiance.selection", MARQUEE),
];

/// The most settings any one built-in declares besides `enabled`.
const MOST_EXTRAS: usize = 2;

/// The declared settings for a plugin ID: `enabled` and its extras for a
/// gameplay feature, none for core (its policy lives in `[loader]` and
/// `[logging]`).
const BUILTIN_SETTINGS: [[SettingDecl; 1 + MOST_EXTRAS]; BUILTINS.len()] = {
    let mut settings = [[ENABLED; 1 + MOST_EXTRAS]; BUILTINS.len()];
    let mut i = 0;
    while i < BUILTINS.len() {
        settings[i][0].description = BUILTINS[i].summary;
        let mut slot = 1;
        let mut e = 0;
        while e < EXTRA_SETTINGS.len() {
            if const_eq(EXTRA_SETTINGS[e].0, BUILTINS[i].id) {
                // more extras than MOST_EXTRAS fail the build here
                settings[i][slot] = EXTRA_SETTINGS[e].1;
                slot += 1;
            }
            e += 1;
        }
        i += 1;
    }
    settings
};

const fn const_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// How many of a built-in's `BUILTIN_SETTINGS` row are declared.
fn declared(id: &str) -> usize {
    1 + EXTRA_SETTINGS
        .iter()
        .filter(|(owner, _)| owner.eq_ignore_ascii_case(id))
        .count()
}

pub fn settings(id: &str) -> &'static [SettingDecl] {
    match BUILTINS
        .iter()
        .position(|b| b.feature != 0 && b.id.eq_ignore_ascii_case(id))
    {
        Some(index) => &BUILTIN_SETTINGS[index][..declared(BUILTINS[index].id)],
        None => &[],
    }
}

/// The declared setting `(plugin_id, key)`, or `None`.
pub fn setting(id: &str, key: &str) -> Option<&'static SettingDecl> {
    settings(id)
        .iter()
        .find(|decl| decl.key.eq_ignore_ascii_case(key))
}

/// A group name is a safe filename stem: no separators, no parent traversal,
/// no absolute path. A namespaced third-party name (`author.plugin`) is
/// allowed, but not a leading, trailing or doubled dot.
pub fn safe_group(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// Loader-owned settings, in `[loader]` of `core.ini`. Their legacy
/// unsectioned bootstrap keys are read as a fallback.
pub const LOADER_SECTION: &str = "loader";
pub const LOADER_SETTINGS: &[SettingDecl] = &[
    SettingDecl {
        key: "wait",
        ty: ValueType::Integer { min: 0, max: 3600 },
        default: "60",
        description: "How many seconds to wait for each required game component at startup. If it does not load in time, no loader plugins start. This does not change game speed or mission timers. Restart required.",
        restart: Restart::Startup,
    sensitive: false,
    },
    SettingDecl {
        key: "allow_unknown_build",
        ty: ValueType::Bool,
        default: "false",
        description: "Allow Core to attempt loading gameplay changes on an unrecognized game version. Leave false for normal play; true does not guarantee compatibility or bypass every plugin's version checks. Restart required.",
        restart: Restart::Startup,
    sensitive: false,
    },
    SettingDecl {
        key: "hot_reload",
        ty: ValueType::Bool,
        default: "false",
        description: "For plugin development: when a plugin's DLL in the plugins folder is replaced while the game runs, unload the old one and load the new one, as soon as no mission is loaded. Leave false for normal play. Restart required.",
        restart: Restart::Startup,
        sensitive: false,
    },
    SettingDecl {
        key: "live_toggle",
        ty: ValueType::Bool,
        default: "false",
        description: "Switch plugins on and off while the game runs: after changing a plugin's enabled setting in its config file and saving, the plugin is unloaded or loaded at the main menu or as the next mission starts or save loads, instead of at the next restart. Plugins that only load at startup (Core, the expanded ammo menu, squad scrolling) still need a restart. Restart required to turn this on.",
        restart: Restart::Startup,
        sensitive: false,
    },
    SettingDecl {
        key: "main_thread_cpus",
        ty: ValueType::Choice(&["all", "spread", "engine"]),
        default: "all",
        description: "Which CPUs the game's main thread may run on. all: every CPU. spread: every CPU except the first core (experimental: it caused long stutters in testing). engine: the game's own choice, the first CPU only. Restart required.",
        restart: Restart::Startup,
        sensitive: false,
    },
    SettingDecl {
        key: "grass_sort",
        ty: ValueType::Choice(&["cached", "engine"]),
        default: "cached",
        description: "How grass is ordered by distance each frame. cached: each distance is worked out once per frame, giving the same order as the game's own sort in about half the time. engine: the game's own sort. Restart required.",
        restart: Restart::Startup,
        sensitive: false,
    },
    SettingDecl {
        key: "view_sort",
        ty: ValueType::Choice(&["cached", "engine"]),
        default: "cached",
        description: "How the objects in view are put in order each frame. cached: what the order depends on is read once per object, giving the same order as the game's own sort. engine: the game's own sort. Restart required.",
        restart: Restart::Startup,
        sensitive: false,
    },
    SettingDecl {
        key: "mesh_sort",
        ty: ValueType::Choice(&["cached", "engine"]),
        default: "cached",
        description: "How the meshes in view are grouped for drawing each frame. cached: what the order depends on is read once per mesh, giving the same order as the game's own sort. engine: the game's own sort. Restart required.",
        restart: Restart::Startup,
        sensitive: false,
    },
    SettingDecl {
        key: "matrix_inverse",
        ty: ValueType::Choice(&["direct", "engine"]),
        default: "direct",
        description: "How the renderer inverts the matrices of objects that moved. direct: the game's own arithmetic in the same order, so the results are identical, without its many small calls. engine: the game's own function. Restart required.",
        restart: Restart::Startup,
        sensitive: false,
    },
    SettingDecl {
        key: "shadow_fit",
        ty: ValueType::Choice(&["trimmed", "engine"]),
        default: "trimmed",
        description: "How each shadow band is fitted to what it covers. trimmed: skips a pass over every shadow caster in view whose result the game throws away; the shadows are the same. engine: the game's own. Restart required.",
        restart: Restart::Startup,
        sensitive: false,
    },
    SettingDecl {
        key: "shadow_cascades",
        ty: ValueType::Choice(&["all", "rotate", "near", "far_half"]),
        default: "far_half",
        description: "Which parts of the shadow map are redrawn each frame. far_half: one of its distance bands a frame, in turn, the farthest (which costs the most) half as often as the others; on busy scenes with high shadows this can take the frame rate from about 31 to 55 fps. rotate: one band a frame, each equally often. near: the nearest band every frame and one of the others in turn. all: every part every frame, as the game does. Restart required.",
        restart: Restart::Startup,
        sensitive: false,
    },
];

/// Diagnostic call-stack tracing (crate::trace), in `[trace]` of `core.ini`.
/// Off unless `sites` names an address; a bad value is reported, never fatal.
pub const TRACE_SECTION: &str = "trace";
pub const TRACE_SETTINGS: &[SettingDecl] = &[
    SettingDecl {
        key: "sites",
        ty: ValueType::Text,
        default: "",
        description: "For troubleshooting: up to four code addresses as module+offset (for example logic+0x42a940, game+0x366fa7). Each time the game runs one, the log records who called it. Leave empty for normal play. Restart required.",
        restart: Restart::Startup,
        sensitive: false,
    },
    SettingDecl {
        key: "hits",
        ty: ValueType::Integer { min: 1, max: 1000 },
        default: "20",
        description: "How many times each traced address is logged before tracing it goes quiet. Restart required.",
        restart: Restart::Startup,
        sensitive: false,
    },
    SettingDecl {
        key: "when",
        ty: ValueType::Choice(&["startup", "mission"]),
        default: "startup",
        description: "When tracing starts: at startup, or when the first mission loads (so the main menu's scene does not use up the hits). startup or mission. Restart required.",
        restart: Restart::Startup,
        sensitive: false,
    },
];

pub const LOGGING_SECTION: &str = "logging";
pub const LOGGING_SETTINGS: &[SettingDecl] = &[SettingDecl {
    key: "level",
    ty: ValueType::Choice(&["error", "warn", "info", "debug"]),
    default: "info",
    description: "Amount of information written to the loader log: error for failures only, warn to include warnings, info for normal startup details, or debug for troubleshooting. Does not enable or disable gameplay features. Restart required.",
    restart: Restart::Startup,
    sensitive: false,
}];

/// The legacy unsectioned bootstrap key for a loader setting, if there is
/// one. `wait` and `allow_unknown_build` were top-level keys; `level` was not
/// configurable and has no legacy form.
pub fn legacy_bootstrap_key(section: &str, key: &str) -> Option<&'static str> {
    if section.eq_ignore_ascii_case(LOADER_SECTION) {
        return LOADER_SETTINGS
            .iter()
            .find(|decl| decl.key.eq_ignore_ascii_case(key))
            .map(|decl| decl.key);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_dlls_and_feature_numbers_are_unique() {
        let ids: HashSet<&str> = BUILTINS.iter().map(|builtin| builtin.id).collect();
        assert_eq!(ids.len(), BUILTINS.len(), "duplicate plugin ID");
        let dlls: HashSet<&str> = BUILTINS.iter().map(|builtin| builtin.dll).collect();
        assert_eq!(dlls.len(), BUILTINS.len(), "duplicate DLL basename");
        let features: HashSet<u32> = BUILTINS
            .iter()
            .map(|builtin| builtin.feature)
            .filter(|f| *f != 0)
            .collect();
        assert_eq!(
            features.len(),
            BUILTINS.iter().filter(|b| b.feature != 0).count()
        );
    }

    #[test]
    fn dependencies_name_known_plugins_and_are_not_self_referential() {
        for builtin in BUILTINS {
            for dependency in builtin.depends {
                assert!(
                    find(dependency).is_some(),
                    "{} depends on unknown {dependency}",
                    builtin.id
                );
                assert_ne!(*dependency, builtin.id, "{} depends on itself", builtin.id);
            }
        }
    }

    #[test]
    fn every_group_is_safe_and_known() {
        for builtin in BUILTINS {
            assert!(safe_group(builtin.group), "unsafe group {}", builtin.group);
            assert!(
                GROUPS.contains(&builtin.group),
                "unknown group {}",
                builtin.group
            );
        }
    }

    #[test]
    fn safe_group_rejects_traversal_and_separators() {
        assert!(safe_group("infantry"));
        assert!(safe_group("author.weapons-v2"));
        assert!(!safe_group(".."));
        assert!(!safe_group("../x"));
        assert!(!safe_group("a/b"));
        assert!(!safe_group("a\\b"));
        assert!(!safe_group("C:evil"));
        assert!(!safe_group(""));
    }

    #[test]
    fn only_gameplay_features_declare_enabled() {
        assert!(setting("defiance.movement", "ENABLED").is_some());
        assert!(setting(CORE_ID, "enabled").is_none());
        assert!(settings(CORE_ID).is_empty());
    }

    #[test]
    fn selection_declares_its_squad_tab_modifier_and_marquee_only() {
        let keys = |id| settings(id).iter().map(|d| d.key).collect::<Vec<_>>();
        assert_eq!(
            keys("defiance.selection"),
            ["enabled", "squad_tab_modifier", "marquee"]
        );
        assert_eq!(keys("defiance.movement"), ["enabled"]);
        let decl = setting("defiance.selection", "squad_tab_modifier").unwrap();
        assert_eq!(decl.default, "ctrl");
        assert_eq!(
            setting("defiance.selection", "marquee").unwrap().default,
            "squads"
        );
        assert!(setting("defiance.selection", "enabled")
            .unwrap()
            .description
            .starts_with("Select individual soldiers"));
    }
}
