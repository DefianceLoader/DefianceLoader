//! The required Core and the loader's own settings. Core is the one built-in:
//! the loader knows its ID, DLL and version, so a missing or mismatched Core
//! manifest is a packaging error. Every other plugin, the feature plugins
//! shipped with the loader included, declares its ID, version, group,
//! dependencies and settings in its own committed sidecar manifest.

use super::schema::{Restart, SettingDecl, ValueType};
use std::sync::OnceLock;

/// The required infrastructure plugin. It is not a gameplay toggle: its
/// availability is separate from any feature's enabled state.
pub const CORE_ID: &str = "defiance.core";

/// The performance tuning plugin. Its settings lived in `[loader]` before it
/// left Core, so [`moved_from_loader`] names them for the fallback.
pub const PERFORMANCE_ID: &str = "defiance.performance";

/// Config groups, by player-facing purpose. A group is a filename under the
/// config directory; the sections inside are the plugin IDs.
pub const GROUPS: [&str; 4] = ["core", "infantry", "weapons", "diagnostics"];

/// A built-in plugin: one the loader requires and checks its manifest against.
/// It declares no settings, no dependencies and no `enabled` toggle, may not
/// be unloaded while the game runs, and is multiplayer-safe.
#[derive(Debug, Clone, Copy)]
pub struct Builtin {
    /// Stable plugin ID, used as the INI section and the dependency key.
    pub id: &'static str,
    /// Relative DLL file name in the discovery directory.
    pub dll: &'static str,
    pub version: &'static str,
    /// Config group, i.e. the filename stem under the config directory.
    pub group: &'static str,
    /// Whether successful init must match its versioned expected-write export.
    pub patch_contract: bool,
}

/// Plugins shipped with the loader that change gameplay: their manifests may
/// not declare `multiplayer_safe`, so an edit cannot let them online.
pub const NOT_MULTIPLAYER_SAFE: &[&str] = &[
    "defiance.ability-groups",
    "defiance.ammunition",
    "defiance.attack",
    "defiance.diagnostics",
    "defiance.expanded-ammo-menu",
    "defiance.firing",
    "defiance.garrison",
    "defiance.legion-vehicle-hacking",
    "defiance.movement",
    "defiance.pickup",
    "defiance.posture",
    "defiance.regroup",
    "defiance.selection",
    "defiance.squad-management-scroll",
    "defiance.unit-inspection",
    "defiance.vehicle-arrival",
    "defiance.vehicle-special-fire",
    "defiance.weapon-drops",
];

pub const BUILTINS: &[Builtin] = &[Builtin {
    id: CORE_ID,
    dll: "defiance_plugin_core.dll",
    version: "0.7.0",
    group: "core",
    patch_contract: true,
}];

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

/// The performance plugin's committed manifest, compiled in for the settings
/// [`moved_from_loader`] names: the fallback must work whether or not the
/// plugin is installed.
const PERFORMANCE_MANIFEST: &str =
    include_str!("../../../../plugins/performance/defiance_plugin_feature_performance.plugin.json");

/// The performance plugin's declared settings besides `enabled`, from its
/// committed manifest. Strings are leaked once, for the process's life.
pub fn performance_settings() -> &'static [SettingDecl] {
    static SETTINGS: OnceLock<Vec<SettingDecl>> = OnceLock::new();
    SETTINGS.get_or_init(|| {
        let manifest = crate::manifest::parse(
            PERFORMANCE_MANIFEST,
            "defiance_plugin_feature_performance.dll",
        )
        .expect("the committed performance manifest parses");
        manifest
            .settings
            .iter()
            .filter(|setting| setting.key != "enabled")
            .map(crate::manifest::leak_decl)
            .collect()
    })
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
        description: "Apply plugin config changes while the game runs: after changing a plugin's enabled setting, or any of its other settings, in its config file and saving, the plugin is unloaded, loaded, or loaded again with the new values at the main menu or as the next mission starts or save loads, instead of at the next restart. Plugins that only load at startup (Core, performance tuning, the expanded ammo menu, squad scrolling, unit inspection, vehicle arrival braking, primary weapon drops, moving grenades, the movement animation overlay) still need a restart; the log names each one that waits. Restart required to turn this on.",
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
pub const LOGGING_SETTINGS: &[SettingDecl] = &[
    SettingDecl {
        key: "level",
        ty: ValueType::Choice(&["error", "warn", "info", "debug"]),
        default: "info",
        description: "Amount of information written to the loader log: error for failures only, warn to include warnings, info for normal startup details, or debug for troubleshooting. Does not enable or disable gameplay features. Restart required.",
        restart: Restart::Startup,
        sensitive: false,
    },
    SettingDecl {
        key: "include_plugins",
        ty: ValueType::Text,
        default: "",
        description: "Comma-separated plugin IDs whose messages are written to the log and debugger; empty includes all plugins. IDs are matched exactly, ignoring case. The global level also applies. Loader messages and crash reports remain available. Restart required.",
        restart: Restart::Startup,
        sensitive: false,
    },
    SettingDecl {
        key: "exclude_plugins",
        ty: ValueType::Text,
        default: "",
        description: "Comma-separated plugin IDs whose messages are omitted from the log and debugger; empty excludes none. Exclusion wins over inclusion. Does not disable plugins or diagnostic probes. Loader messages and crash reports remain available. Restart required.",
        restart: Restart::Startup,
        sensitive: false,
    },
];

/// The legacy unsectioned bootstrap key for a loader setting, if there is
/// one. `wait` and `allow_unknown_build` were top-level keys; `level` was not
/// configurable and has no legacy form. The performance plugin's settings
/// were loader settings, so they keep their top-level form too.
pub fn legacy_bootstrap_key(section: &str, key: &str) -> Option<&'static str> {
    if section.eq_ignore_ascii_case(LOADER_SECTION) {
        return LOADER_SETTINGS
            .iter()
            .find(|decl| decl.key.eq_ignore_ascii_case(key))
            .map(|decl| decl.key);
    }
    moved_from_loader(section, key)
}

/// The key, when `(section, key)` is a performance setting that `[loader]`
/// of `core.ini` held before the plugin left Core. Such a `[loader]` value
/// still applies when `[defiance.performance]` does not set the key, and is
/// not reported as unknown; migration moves it.
pub fn moved_from_loader(section: &str, key: &str) -> Option<&'static str> {
    if !section.eq_ignore_ascii_case(PERFORMANCE_ID) {
        return None;
    }
    performance_settings()
        .iter()
        .find(|decl| decl.key.eq_ignore_ascii_case(key))
        .map(|decl| decl.key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_is_the_only_builtin() {
        assert_eq!(BUILTINS.len(), 1);
        assert_eq!(BUILTINS[0].id, CORE_ID);
        assert!(safe_group(BUILTINS[0].group) && GROUPS.contains(&BUILTINS[0].group));
        assert!(find("DEFIANCE.CORE").is_some());
        assert!(find("defiance.selection").is_none());
        assert!(by_dll("defiance_plugin_feature_selection.dll").is_none());
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
    fn performance_settings_come_from_its_manifest_without_enabled() {
        let keys: Vec<&str> = performance_settings().iter().map(|d| d.key).collect();
        assert_eq!(
            keys,
            [
                "main_thread_cpus",
                "grass_sort",
                "view_sort",
                "mesh_sort",
                "matrix_inverse",
                "shadow_fit",
                "shadow_cascades",
                "tree_sway",
            ]
        );
        let cascades = performance_settings()
            .iter()
            .find(|d| d.key == "shadow_cascades")
            .unwrap();
        assert_eq!(cascades.default, "far_half");
        assert!(matches!(cascades.ty, ValueType::Choice(c) if c.contains(&"rotate")));
        assert_eq!(
            moved_from_loader("DEFIANCE.PERFORMANCE", "GRASS_SORT"),
            Some("grass_sort")
        );
        assert_eq!(moved_from_loader(PERFORMANCE_ID, "enabled"), None);
        assert_eq!(moved_from_loader("defiance.selection", "grass_sort"), None);
    }
}
