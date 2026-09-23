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

; Which builds of the game to patch:
;   known  the GOG build, and other builds whose patch sites have been checked
;          (the Steam build); any other build is left alone
;   scan   as known, but when the patch does not fit a build, find each patch
;          site from its signature instead, in any build; it refuses if any
;          site is missing or ambiguous. At your own risk.
;   force  find every patch site by signature, even in the known builds (testing)
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
pub enum Pause {
    Auto,
    Always,
    Never,
}

pub struct Settings {
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
    let mut settings = Settings {
        builds: Scan::Default,
        game: true,
        wait: 180,
        pause: Pause::Auto,
        path: path(),
    };
    let Some(path) = settings.path.clone() else {
        return settings;
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => {
            match std::fs::write(&path, DEFAULTS) {
                Ok(()) => println!("settings: wrote {} with the defaults", path.display()),
                Err(e) => println!("settings: could not write {}: {e}", path.display()),
            }
            DEFAULTS.to_string()
        }
    };
    for (number, line) in text.lines().enumerate() {
        let line = line.split([';', '#']).next().unwrap_or("").trim();
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
