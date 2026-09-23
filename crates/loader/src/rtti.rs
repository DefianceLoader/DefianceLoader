//! Looking a class up by name at run time: the native substitute for the
//! reflection BepInEx's Harmony gets for free.
//!
//! `vtable_slot` walks the live module with `defiance_core::rtti` and returns
//! the function at a vtable slot. The module image is read once and cached;
//! RTTI walking is a full scan, so the results are cached per class too, and
//! this is meant for setup, not per-frame use.
//!
//! A class has addresses, not names — RTTI stores the slots in declaration
//! order with no labels. Naming them needs a table, and there is no PDB beside
//! the game to build one from. So `method` reads `defiance-rtti.ini`, if
//! present, whose sections are classes and whose entries are `name = slot`:
//!
//! ```text
//! [build]
//! logic.dll = 17ef4835...
//! game.dll  = f0184b9f...
//!
//! [.?AVSquadUnitSelectableFacet@@]
//! setSelected = 3
//! ```
//!
//! The `[build]` hashes make the table per-build: the loader refuses to use the
//! names unless both modules on disk hash to what it records, so a game update
//! leaves `method` returning null rather than resolving a name to the wrong
//! slot. Without a table, a plugin uses `vtable_slot`, or a signature.

use crate::resolve;
use crate::win;
use defiance_core::rtti;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The modules the game's classes live in, in the order they are searched.
const MODULES: [&str; 2] = ["logic.dll", "game.dll"];
/// Each module's image, read once. A full RTTI scan is expensive, so it is
/// done lazily and the result cached per class in `RESOLVED`.
static IMAGES: OnceLock<Mutex<HashMap<&'static str, (usize, Vec<u8>)>>> = OnceLock::new();
/// Resolved classes, as (module base, method rvas).
static RESOLVED: OnceLock<Mutex<HashMap<String, Option<(usize, Vec<usize>)>>>> = OnceLock::new();

/// `defiance-rtti.ini`: the `[build]` hashes it was written for, and the
/// class -> { name -> slot } entries.
struct Names {
    valid: bool,
    methods: HashMap<String, HashMap<String, usize>>,
}

static NAMES: OnceLock<Names> = OnceLock::new();

fn images() -> &'static Mutex<HashMap<&'static str, (usize, Vec<u8>)>> {
    IMAGES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The module's image, read once and kept. The loader is in the target, so the
/// bytes are directly addressable.
fn image(name: &'static str) -> Option<(usize, Vec<u8>)> {
    let mut cache = images().lock().unwrap();
    if let Some(found) = cache.get(name) {
        return Some(found.clone());
    }
    let (base, size) = resolve::module(name)?;
    let bytes = unsafe { core::slice::from_raw_parts(base as *const u8, size) }.to_vec();
    let entry = (base as usize, bytes);
    cache.insert(name, entry.clone());
    Some(entry)
}

fn is_code(base: usize) -> impl Fn(usize) -> bool {
    move |rva| win::is_executable(base + rva)
}

/// The first vtable of `class` in logic.dll then game.dll, as its module base
/// and method rvas.
fn resolve_class(class: &str) -> Option<(usize, Vec<usize>)> {
    let mut resolved = RESOLVED
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    if let Some(found) = resolved.get(class) {
        return found.clone();
    }
    let mut result = None;
    for name in MODULES {
        let Some((base, bytes)) = image(name) else {
            continue;
        };
        let predicate = is_code(base);
        if let Some(vtable) = rtti::find(&bytes, base, class, &predicate)
            .into_iter()
            .next()
        {
            result = Some((base, vtable.methods));
            break;
        }
    }
    resolved.insert(class.to_string(), result.clone());
    result
}

/// The address of `class`'s vtable slot `slot`, or null.
pub fn vtable_slot(class: &str, slot: usize) -> Option<usize> {
    let (base, methods) = resolve_class(class)?;
    methods.get(slot).map(|rva| base + rva)
}

/// The address of a named method of `class`, from `defiance-rtti.ini`, or null
/// when there is no table or no such name.
pub fn method(class: &str, name: &str) -> Option<usize> {
    let game_dir = crate::config::game_dir();
    let table = NAMES.get_or_init(|| read_names(&game_dir.join("defiance-rtti.ini"), &game_dir));
    if !table.valid {
        return None;
    }
    let class_lower = class.to_ascii_lowercase();
    let name_lower = name.to_ascii_lowercase();
    for (section, methods) in &table.methods {
        if section.contains(&class_lower) {
            if let Some(slot) = methods.get(&name_lower) {
                return vtable_slot(class, *slot);
            }
        }
    }
    None
}

/// Parse `defiance-rtti.ini`. `[build]` holds the module hashes the table was
/// written for; every other section is a class, each entry `name = slot`.
/// Sections and names are lower-cased; a slot that is not a number is skipped.
/// The table is only `valid` if `[build]` names both modules and both match the
/// files on disk.
fn read_names(path: &std::path::Path, game_dir: &std::path::Path) -> Names {
    let mut builds: HashMap<String, String> = HashMap::new();
    let mut methods: HashMap<String, HashMap<String, usize>> = HashMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return Names {
            valid: false,
            methods,
        };
    };
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.split([';', '#']).next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = name.trim().to_ascii_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim().to_string());
        if section == "build" {
            builds.insert(key, value);
        } else if !section.is_empty() {
            if let Ok(slot) = value.parse::<usize>() {
                methods
                    .entry(section.clone())
                    .or_default()
                    .insert(key, slot);
            }
        }
    }
    let valid = MODULES.iter().all(|module| {
        builds.get(*module).is_some_and(|want| {
            defiance_core::sha256::file(&game_dir.join(module))
                .map(|sha| sha == *want)
                .unwrap_or(false)
        })
    });
    if !valid && !methods.is_empty() {
        crate::log::warn(
            "defiance-rtti.ini does not name this build's logic.dll and game.dll; names ignored",
        );
    }
    Names { valid, methods }
}

#[cfg(test)]
mod tests {
    use super::read_names;

    #[test]
    fn parses_a_name_table_and_checks_the_build() {
        let path = std::env::temp_dir().join("defiance-rtti-test.ini");
        // no [build] section: the names parse, but the table is not usable
        std::fs::write(
            &path,
            "; a comment\n[.?AVWidget@@]\nsetSelected = 3\nspin = 0 ; trailing\n\n[Other]\nrun = 7\n",
        )
        .unwrap();
        let table = read_names(&path, std::path::Path::new("."));
        assert!(!table.valid);
        assert_eq!(table.methods[".?avwidget@@"]["setselected"], 3);
        assert_eq!(table.methods[".?avwidget@@"]["spin"], 0);
        assert_eq!(table.methods["other"]["run"], 7);

        // a [build] that cannot match the files on disk is refused too
        std::fs::write(
            &path,
            "[build]\nlogic.dll = deadbeef\ngame.dll = deadbeef\n[.?AVWidget@@]\nspin = 1\n",
        )
        .unwrap();
        let table = read_names(&path, std::path::Path::new("."));
        assert!(!table.valid);
        let _ = std::fs::remove_file(&path);
    }
}
