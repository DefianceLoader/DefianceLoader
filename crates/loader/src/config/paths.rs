//! Where the loader's files live, from the bootstrap file beside the
//! executable. Pure: given an executable directory and a parsed bootstrap it
//! returns the paths, so staging and the runtime can be tested against the same
//! fixtures regardless of the process working directory.
//!
//! `defiance-loader.ini` keeps only bootstrap settings:
//!
//! ```text
//! root = ../DefianceLoader             ; resolved against the executable dir
//! plugins = ../DefianceLoader/plugins  ; optional legacy override
//! ```
//!
//! Everything else lives under `root`: `config/`, `logs/` and `plugins/`. The
//! legacy unsectioned `plugins` key is an override, resolved against the
//! executable directory exactly as before; it is never reinterpreted against
//! `root`. An absolute `root` or `plugins` is used as written.

use super::parse::Document;
use std::path::{Component, Path, PathBuf};

pub const BOOTSTRAP_FILE: &str = "defiance-loader.ini";
pub const FALLBACK_LOG_FILE: &str = "defiance-loader.log";
pub const DEFAULT_ROOT: &str = "../DefianceLoader";
pub const DEFAULT_PLUGINS: &str = "plugins";
pub const CONFIG_DIR: &str = "config";
pub const LOG_DIR: &str = "logs";

/// The bootstrap keys this version understands. Anything else is kept and
/// reported rather than acted on.
pub const BOOTSTRAP_KEYS: [&str; 2] = ["root", "plugins"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// The directory `trm.exe` and the proxy DLL sit in.
    pub exe_dir: PathBuf,
    /// The add-on tree; `config`, `logs` and `plugins` hang off it by default.
    pub root: PathBuf,
    pub config_dir: PathBuf,
    pub log_dir: PathBuf,
    /// Where plugin DLLs are discovered.
    pub plugin_dir: PathBuf,
    /// Whether `plugin_dir` came from the legacy `plugins` override rather than
    /// `root/plugins`.
    pub plugin_override: bool,
    /// The bootstrap file's own path.
    pub bootstrap: PathBuf,
    /// A best-effort log beside the executable, used when `log_dir` cannot be
    /// opened. Startup failure must still leave a trace.
    pub fallback_log: PathBuf,
}

/// Resolve a path string against the executable directory: absolute values are
/// kept, relative ones are joined to `base`. Never resolves against the
/// process working directory. `.` and `..` components are folded lexically, so
/// a resolved path reads cleanly in the log.
pub fn resolve_against(base: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        normalize(path)
    } else {
        normalize(&base.join(path))
    }
}

/// Fold `.` and `..` without consulting the filesystem. A leading `..` with
/// nothing to pop is kept only for a relative path.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let popable = matches!(out.components().next_back(), Some(Component::Normal(_)));
                if popable {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

impl Paths {
    /// The layout on disk, from the executable directory and the parsed
    /// bootstrap. Returns the paths and any warnings (unknown bootstrap keys,
    /// for which the declared default is kept).
    pub fn resolve(exe_dir: &Path, bootstrap: &Document) -> (Paths, Vec<String>) {
        let mut warnings = Vec::new();
        let mut root_value = DEFAULT_ROOT.to_string();
        if let Some((entry, earlier)) = bootstrap.top("root") {
            root_value = entry.value.clone();
            if !earlier.is_empty() {
                warnings.push(format!(
                    "{}:{}: `root` is declared more than once; using line {}",
                    BOOTSTRAP_FILE, entry.line, entry.line
                ));
            }
        }
        let root = resolve_against(exe_dir, &root_value);

        let mut plugin_override = false;
        let mut plugin_dir = root.join(DEFAULT_PLUGINS);
        if let Some((entry, _)) = bootstrap.top("plugins") {
            plugin_override = true;
            plugin_dir = resolve_against(exe_dir, &entry.value);
        }

        for entry in &bootstrap.entries {
            if entry.section.is_empty()
                && !BOOTSTRAP_KEYS.contains(&entry.key.as_str())
                && !super::builtin::LOADER_SETTINGS
                    .iter()
                    .any(|decl| decl.key == entry.key)
            {
                warnings.push(format!(
                    "{}:{}: unknown bootstrap key `{}` is kept but not used",
                    BOOTSTRAP_FILE, entry.line, entry.key
                ));
            }
        }

        let paths = Paths {
            exe_dir: exe_dir.to_path_buf(),
            config_dir: root.join(CONFIG_DIR),
            log_dir: root.join(LOG_DIR),
            plugin_dir,
            plugin_override,
            root,
            bootstrap: exe_dir.join(BOOTSTRAP_FILE),
            fallback_log: exe_dir.join(FALLBACK_LOG_FILE),
        };
        (paths, warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse::parse;

    fn exe() -> PathBuf {
        PathBuf::from("C:/Game/bin")
    }

    #[test]
    fn defaults_hang_off_the_root_beside_the_game() {
        let (paths, warnings) = Paths::resolve(&exe(), &parse(""));
        assert!(warnings.is_empty());
        let top = PathBuf::from("C:/Game/DefianceLoader");
        assert_eq!(paths.root, top);
        assert_eq!(paths.config_dir, top.join("config"));
        assert_eq!(paths.log_dir, top.join("logs"));
        assert_eq!(paths.plugin_dir, top.join("plugins"));
        assert!(!paths.plugin_override);
        assert_eq!(paths.bootstrap, exe().join("defiance-loader.ini"));
        assert_eq!(paths.fallback_log, exe().join("defiance-loader.log"));
    }

    #[test]
    fn absolute_root_is_used_as_written() {
        let (paths, _) = Paths::resolve(&exe(), &parse("root = D:/Mods/Defiance\n"));
        assert_eq!(paths.root, PathBuf::from("D:/Mods/Defiance"));
        assert_eq!(paths.plugin_dir, PathBuf::from("D:/Mods/Defiance/plugins"));
    }

    #[test]
    fn legacy_plugins_override_resolves_against_the_executable() {
        let (paths, _) = Paths::resolve(&exe(), &parse("plugins = ../custom/plugins\n"));
        assert_eq!(paths.plugin_dir, PathBuf::from("C:/Game/custom/plugins"));
        assert!(paths.plugin_override);
        // The root default is untouched: the override is not reinterpreted.
        assert_eq!(paths.root, PathBuf::from("C:/Game/DefianceLoader"));
    }

    #[test]
    fn absolute_plugin_override_wins() {
        let (paths, _) = Paths::resolve(&exe(), &parse("plugins = D:/ThirdParty/plugins\n"));
        assert_eq!(paths.plugin_dir, PathBuf::from("D:/ThirdParty/plugins"));
        assert!(paths.plugin_override);
    }

    #[test]
    fn unknown_bootstrap_keys_are_reported_not_used() {
        let (_, warnings) = Paths::resolve(
            &exe(),
            &parse("wait = 15\nallow_unknown_build = yes\nunknown = value\nroot = ../X\n"),
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unknown bootstrap key `unknown`"));
    }
}
