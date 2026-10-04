//! The settings file, so the injector can be used by double-clicking it.
//!
//! `defiance-pickup-inject.ini` sits beside the executable (it follows the
//! executable's name) and is written with its defaults, commented, on the first
//! run. Anything given on the command line overrides it. A line it does not
//! understand is reported and its default kept, so a typo never stops a run.

use defiance_core::install::Scan;
use std::path::PathBuf;

const DEFAULTS: &str = "\
; Settings for defiance-pickup-inject. Edit this file, save it, and run the
; injector again. Anything given on the command line overrides it.

; loader starts the complete plugin host without a proxy DLL. Gameplay
; settings remain in DefianceLoader/config/*.ini in the game installation.
; patches uses the standalone assembled patches, with their embedded defaults.
mode = loader

; The host DLL, relative to this EXE or an absolute path.
loader_dll = defiance_loader.dll

; The following builds/game options apply only to mode = patches.
; Which builds of the game to patch:
;   known  supported GOG and Steam DLL pairs, including the September 2026
;          layouts; any other build is left alone
;   scan   as known, but when the patch does not fit a build, find each patch
;          site from its signature instead, in any build; it refuses if any
;          site is missing or ambiguous. At your own risk.
;   force  test signature relocation on the December 2025 layouts; supported
;          newer layouts always use their DLL hash pair
builds = known

; The click controls in game.dll: Ctrl+click and Ctrl+Shift+click on soldiers,
; and the squad panel icons. false leaves game.dll alone.
game = true

; How long to wait for the game to start, in seconds.
wait = 180

; Keep this window open at the end, so the result can be read:
;   auto    only when the injector was started by double-clicking it
;   always  every time
;   never   close as soon as it is done
pause = auto
";

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Loader,
    Patches,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Pause {
    Auto,
    Always,
    Never,
}

pub struct Settings {
    pub mode: Mode,
    pub loader_dll: PathBuf,
    pub builds: Scan,
    pub game: bool,
    pub wait: u64,
    pub pause: Pause,
    /// where they were read from, for the summary line
    pub path: Option<PathBuf>,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
}

impl Pause {
    /// Whether to wait for Enter before exiting. `Auto` does when this process
    /// is the only one attached to its console, which is the case when it was
    /// started from Explorer: that window closes the moment it exits.
    pub fn keep_open(self) -> bool {
        match self {
            Pause::Always => true,
            Pause::Never => false,
            Pause::Auto => {
                let mut ids = [0u32; 4];
                unsafe { GetConsoleProcessList(ids.as_mut_ptr(), ids.len() as u32) == 1 }
            }
        }
    }
}

fn path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_extension("ini"))
}

fn flag(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// The settings, from the file beside the executable, which is created with
/// the defaults if it is not there yet.
pub fn load() -> Settings {
    load_from(path())
}

fn load_from(path: Option<PathBuf>) -> Settings {
    let mut settings = Settings {
        mode: Mode::Loader,
        loader_dll: PathBuf::from("defiance_loader.dll"),
        builds: Scan::Default,
        game: true,
        wait: 180,
        pause: Pause::Auto,
        path,
    };
    let Some(path) = settings.path.clone() else {
        return settings;
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => {
            // An older launcher INI keeps its direct-patch behavior. Add the
            // new launcher keys once, preserving every existing value/comment.
            let upgraded = complete_launcher(&text);
            if upgraded != text {
                if let Err(error) = std::fs::write(&path, &upgraded) {
                    println!("settings: could not add launcher options: {error}");
                }
            }
            upgraded
        }
        Err(_) => {
            match std::fs::write(&path, DEFAULTS) {
                Ok(()) => println!("settings: wrote {} with the defaults", path.display()),
                Err(e) => println!("settings: could not write {}: {e}", path.display()),
            }
            DEFAULTS.to_string()
        }
    };
    for (number, line) in text.lines().enumerate() {
        let line = without_comment(line).trim();
        if line.is_empty() || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            println!(
                "settings: line {} is not `key = value`; ignored",
                number + 1
            );
            continue;
        };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        let understood = match key.as_str() {
            "mode" => match value.to_ascii_lowercase().as_str() {
                "loader" => Some(settings.mode = Mode::Loader),
                "patches" => Some(settings.mode = Mode::Patches),
                _ => None,
            },
            "loader_dll" => {
                let value = value.trim_matches('"');
                (!value.is_empty()).then(|| settings.loader_dll = PathBuf::from(value))
            }
            "builds" => match value.to_ascii_lowercase().as_str() {
                "known" => Some(settings.builds = Scan::Default),
                "scan" => Some(settings.builds = Scan::Unknown),
                "force" => Some(settings.builds = Scan::Always),
                _ => None,
            },
            "game" => flag(value).map(|on| settings.game = on),
            "wait" => value.parse().ok().map(|seconds| settings.wait = seconds),
            "pause" => match value.to_ascii_lowercase().as_str() {
                "auto" => Some(settings.pause = Pause::Auto),
                "always" => Some(settings.pause = Pause::Always),
                "never" => Some(settings.pause = Pause::Never),
                _ => None,
            },
            _ => {
                println!(
                    "settings: line {}: unknown setting `{key}`; ignored",
                    number + 1
                );
                continue;
            }
        };
        if understood.is_none() {
            println!(
                "settings: line {}: `{value}` is not a value for {key}; kept the default",
                number + 1
            );
        }
    }
    settings
}

fn complete_launcher(text: &str) -> String {
    let has = |name: &str| {
        text.lines().any(|line| {
            line.split([';', '#'])
                .next()
                .unwrap_or("")
                .split_once('=')
                .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case(name))
        })
    };
    let mut extra = String::new();
    if !has("mode") {
        extra.push_str(
            "; Set mode = loader for the full plugin host and its gameplay INIs.\nmode = patches\n",
        );
    }
    if !has("loader_dll") {
        extra.push_str("; Host DLL path, relative to this EXE or absolute.\nloader_dll = defiance_loader.dll\n");
    }
    if extra.is_empty() {
        text.to_owned()
    } else {
        format!("{}\n\n{}", text.replace("\r\n", "\n").trim_end(), extra)
    }
}

fn without_comment(line: &str) -> &str {
    let mut quoted = false;
    for (at, character) in line.char_indices() {
        if character == '"' {
            quoted = !quoted;
        }
        if !quoted && matches!(character, ';' | '#') {
            return &line[..at];
        }
    }
    line
}

impl Settings {
    pub fn loader_path(&self) -> PathBuf {
        if self.loader_dll.is_absolute() {
            self.loader_dll.clone()
        } else {
            self.path
                .as_ref()
                .and_then(|path| path.parent())
                .map(|folder| folder.join(&self.loader_dll))
                .unwrap_or_else(|| self.loader_dll.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_values_survive_and_new_launcher_keys_are_added_once() {
        let text = "; user's comment\r\nwait = 17\r\ngame = false\r\npause = never\r\n";
        let upgraded = complete_launcher(text);
        assert!(upgraded.contains("; user's comment\nwait = 17\ngame = false\npause = never"));
        assert!(upgraded.contains("mode = patches\n"));
        assert_eq!(complete_launcher(&upgraded), upgraded);
        let explicit = "mode = loader\nloader_dll = host/custom.dll\nwait = 12\n";
        assert_eq!(complete_launcher(explicit), explicit);
    }

    #[test]
    fn ini_loading_preserves_legacy_mode_and_resolves_dll_beside_exe() {
        let dir =
            std::env::temp_dir().join(format!("defiance-launcher-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ini = dir.join("launcher.ini");
        std::fs::write(&ini, "wait = 17\ngame = false\npause = never\n").unwrap();
        let settings = load_from(Some(ini.clone()));
        assert!(settings.mode == Mode::Patches);
        assert_eq!(settings.wait, 17);
        assert!(!settings.game);
        assert!(settings.pause == Pause::Never);
        assert_eq!(settings.loader_path(), dir.join("defiance_loader.dll"));
        std::fs::write(
            &ini,
            "mode = loader\nloader_dll = \"custom;# directory/host.dll\" ; comment\n",
        )
        .unwrap();
        let settings = load_from(Some(ini.clone()));
        assert!(settings.mode == Mode::Loader);
        assert_eq!(
            settings.loader_path(),
            dir.join("custom;# directory/host.dll")
        );
        std::fs::remove_file(&ini).unwrap();
        let settings = load_from(Some(ini.clone()));
        assert!(settings.mode == Mode::Loader);
        assert!(std::fs::read_to_string(&ini)
            .unwrap()
            .contains("mode = loader"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
