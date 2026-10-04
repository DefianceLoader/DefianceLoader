//! Select the assembled class layout from the pair of DLL hashes. Modern
//! builds use their own units; signature scanning never substitutes an older
//! layout for a recognized module from a mismatched pair.

use defiance_core::apply::Process;
use defiance_core::unit::Unit;
use defiance_core::unit_apply::{self, Module, Payload};
use defiance_core::{sha256, Target};
use defiance_feature_sdk::units::Embedded;
use std::path::Path;

static UNITS: &[Embedded] = include!(concat!(env!("OUT_DIR"), "/units.rs"));

pub fn units(build: &str) -> Result<Vec<(Unit, &'static [u8])>, String> {
    let out: Vec<_> = UNITS
        .iter()
        .filter(|u| u.build == build)
        .map(|u| Unit::parse(u.descriptor).map(|unit| (unit, u.code)))
        .collect::<Result<_, _>>()?;
    if out.is_empty() {
        return Err(format!("no embedded units for {build}"));
    }
    Ok(out)
}

pub fn names() -> Vec<&'static str> {
    let mut names: Vec<_> = UNITS
        .iter()
        .map(|u| u.build)
        .filter(|b| *b != "reference")
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

fn hashes(build: &str) -> Result<(Unit, Unit), String> {
    let mut own = units(build)?
        .into_iter()
        .map(|(unit, _)| unit)
        .filter(|u| u.plugin == "core");
    let game = own
        .find(|u| u.module == "game.dll")
        .ok_or("no game build anchor")?;
    let logic = own
        .find(|u| u.module == "logic.dll")
        .ok_or("no logic build anchor")?;
    Ok((logic, game))
}

/// None keeps the reference installer for old or wholly unknown builds.
pub fn select(logic_sha: &str, game_sha: &str) -> Result<Option<&'static str>, String> {
    let mut recognized = false;
    for build in names() {
        let (logic, game) = hashes(build)?;
        if logic.source_sha256 == logic_sha && game.source_sha256 == game_sha {
            return Ok(Some(build));
        }
        recognized |= logic.source_sha256 == logic_sha || game.source_sha256 == game_sha;
    }
    let (logic, game) = hashes("reference")?;
    let reference = logic.source_sha256 == logic_sha && game.source_sha256 == game_sha;
    let verified = logic
        .verified
        .iter()
        .zip(&game.verified)
        .any(|(logic, game)| logic == logic_sha && game == game_sha);
    if reference || verified {
        return Ok(None);
    }
    recognized |= logic.source_sha256 == logic_sha
        || game.source_sha256 == game_sha
        || logic.verified.iter().any(|sha| sha == logic_sha)
        || game.verified.iter().any(|sha| sha == game_sha);
    if recognized {
        return Err(
            "logic.dll and game.dll do not form a supported build pair; no writes made".into(),
        );
    }
    Ok(None)
}

pub fn for_targets(logic: &Target, game: &Target) -> Result<Option<&'static str>, String> {
    select(
        &sha256::file(&logic.path).map_err(|e| e.to_string())?,
        &sha256::file(&game.path).map_err(|e| e.to_string())?,
    )
}

pub fn payloads<'a>(units: &'a [(Unit, &'static [u8])], module: &str) -> Vec<Payload<'a>> {
    units
        .iter()
        .filter(|(unit, _)| unit.module == module)
        .map(|(unit, code)| Payload { unit, code })
        .collect()
}

/// Defaults the assembled hooks read. A zero material callback leaves the
/// preview's own materials in use; Rust callbacks require the in-game loader.
pub fn initialize(unit: &Unit, code: &mut [u8], key_state: usize) -> Result<(), String> {
    for (name, value) in [
        ("marquee", key_state as u64),
        ("tab_modifier", 0x11),
        ("step", 0x11),
    ] {
        if let Some(cell) = unit.cell(name) {
            if cell.bytes < 8 {
                return Err(format!("{}: {name} cell is too small", unit.name));
            }
            code[cell.offset..cell.offset + 8].copy_from_slice(&value.to_le_bytes());
        }
    }
    Ok(())
}

pub fn install(
    logic: &Target,
    game: &Target,
    build: &str,
    with_game: bool,
) -> Result<&'static str, String> {
    let units = units(build)?;
    let logic_units = payloads(&units, "logic.dll");
    let game_units = payloads(&units, "game.dll");
    let mut modules = vec![Module {
        target: logic,
        payloads: &logic_units,
    }];
    if with_game {
        modules.push(Module {
            target: game,
            payloads: &game_units,
        });
    }
    let export = |dll: &str, name: &str| super::remote_export(logic.process_id, dll, name);
    let key_state = export("user32.dll", "GetAsyncKeyState")?;
    unit_apply::apply(&modules, &export, &|unit, code| {
        initialize(unit, code, key_state)
    })
}

pub fn check_dir(dir: &Path) -> Result<bool, String> {
    let logic = sha256::file(&dir.join("logic.dll")).map_err(|e| e.to_string())?;
    let game = sha256::file(&dir.join("game.dll")).map_err(|e| e.to_string())?;
    let Some(build) = select(&logic, &game)? else {
        return Ok(defiance_core::relocate::check_dir(
            &super::logic_patch(),
            &super::game_patch(),
            dir,
        ));
    };
    println!("both DLLs match {build}; using its assembled class layout");
    let units = units(build)?;
    for module in ["logic.dll", "game.dll"] {
        let image = defiance_core::pe::map(&dir.join(module))?;
        let payloads = payloads(&units, module);
        unit_apply::validate(&payloads, &image.image)?;
        // Link every fixup and branch at a representative allocation, without
        // opening a process or writing the DLLs on disk.
        let base = 0x180000000;
        for payload in &payloads {
            payload.unit.link(
                payload.code,
                base,
                base + 0x1000000,
                &|name| (name == "trace_ring").then_some(base + 0x2000000),
                &|_, _| Ok(base + 0x3000000),
            )?;
            payload.unit.placed(base, base + 0x1000000)?;
        }
        println!(
            "  {module}: all {} writes and {} anchors hold; {} units link",
            payloads.iter().map(|p| p.unit.writes.len()).sum::<usize>(),
            payloads.iter().map(|p| p.unit.anchors.len()).sum::<usize>(),
            payloads.len()
        );
    }
    Ok(true)
}

/// Read trace cells without interpreting entity fields from another layout.
pub fn probe(target: &Target, build: &str, icon: bool) -> Result<(), String> {
    let units = units(build)?;
    let payloads = payloads(&units, if icon { "game.dll" } else { "logic.dll" });
    let module = Module {
        target,
        payloads: &payloads,
    };
    let process = Process::open(target.process_id)?;
    let block = unit_apply::installed_block(&process, &module)?
        .ok_or("the injector's units are not installed; inject and make some selections first")?;
    let read = |unit: &str, name: &str, bytes: usize| -> Result<Vec<u8>, String> {
        let address = unit_apply::cell(&module, block, unit, name).ok_or("trace cell is absent")?;
        process.read(address as *const u8, bytes)
    };
    let qword = |bytes: &[u8], offset: usize| {
        u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
    };
    if icon {
        let trace = read("selection-game", "trace", 0x60)?;
        println!(
            "{build}: single clicks {}, double clicks {}, squad walks {}, last Ctrl flag {}",
            qword(&trace, 0x40),
            qword(&trace, 0x50),
            qword(&trace, 0x38),
            qword(&trace, 0x48)
        );
        for (i, label) in [
            "entity",
            "facets",
            "selectable",
            "parent",
            "holder",
            "squad",
            "result",
        ]
        .iter()
        .enumerate()
        {
            println!("  {label}: {:#x}", qword(&trace, i * 8));
        }
    } else {
        let ring = read("", "trace_ring", 0x10 + 32 * 16)?;
        let calls = qword(&ring, 0);
        println!("{build}: {calls} manager trace calls; subjects are raw addresses");
        let first = if calls > 32 { calls as usize % 32 } else { 0 };
        for n in 0..calls.min(32) as usize {
            let i = (first + n) % 32;
            let tagged = qword(&ring, 0x10 + i * 16);
            println!(
                "  [{n:2}] kind {:#x}, subject {:#x}, caller {:#x}",
                tagged >> 56,
                qword(&ring, 0x18 + i * 16),
                tagged & 0x00ff_ffff_ffff_ffff
            );
        }
        let scratch = read("ammunition-logic", "scratch", 0x58)?;
        println!(
            "ammo pin: {} disabled queries; last facet {:#x}, slot {}",
            qword(&scratch, 0x40),
            qword(&scratch, 0x48),
            qword(&scratch, 0x50)
        );
        let census = read("diagnostics-logic", "census", 32 * 16)?;
        for i in 0..32 {
            let caller = qword(&census, i * 16);
            if caller != 0 {
                println!(
                    "behaviour reader {caller:#x}: {} calls",
                    qword(&census, i * 16 + 8)
                );
            }
        }
    }
    Ok(())
}

/// Exercise the real installer with each build's expectations planted into
/// stand-in modules. Compatibility with stock files is checked by check_dir.
pub fn self_test() -> Result<(), String> {
    let process = Process::open(std::process::id())?;
    for build in names() {
        let units = units(build)?;
        let mut images = Vec::new();
        for module in ["logic.dll", "game.dll"] {
            let payloads = payloads(&units, module);
            let size = payloads
                .iter()
                .flat_map(|p| {
                    p.unit
                        .writes
                        .iter()
                        .map(|w| w.rva + w.before.len())
                        .chain(p.unit.anchors.iter().map(|a| a.rva + a.bytes.len()))
                })
                .max()
                .unwrap_or(0)
                + 0x1000;
            let mut image = vec![0; size];
            for payload in payloads {
                for (rva, bytes) in payload
                    .unit
                    .writes
                    .iter()
                    .map(|w| (w.rva, &w.before))
                    .chain(payload.unit.anchors.iter().map(|a| (a.rva, &a.bytes)))
                {
                    image[rva..rva + bytes.len()].copy_from_slice(bytes);
                }
            }
            images.push(image);
        }
        let targets: Vec<_> = images
            .iter_mut()
            .map(|image| Target {
                process_id: std::process::id(),
                base: image.as_mut_ptr(),
                size: image.len(),
                path: Default::default(),
            })
            .collect();
        let logic = payloads(&units, "logic.dll");
        let game = payloads(&units, "game.dll");
        let modules = [
            Module {
                target: &targets[0],
                payloads: &logic,
            },
            Module {
                target: &targets[1],
                payloads: &game,
            },
        ];
        let export = |dll: &str, name: &str| {
            defiance_core::apply::resolve_export(dll, name).map(|p| p as usize)
        };
        let key_state = export("user32.dll", "GetAsyncKeyState")?;
        let init = |unit: &Unit, code: &mut [u8]| initialize(unit, code, key_state);
        let mut blocks = TestBlocks(Vec::new());
        if unit_apply::apply(&modules[..1], &export, &init)? != "patched" {
            return Err(format!("{build}: logic-only installation failed"));
        }
        blocks.0.push(
            unit_apply::installed_block(&process, &modules[0])?
                .ok_or("logic allocation is missing")?,
        );
        if unit_apply::installed_block(&process, &modules[1])?.is_some() {
            return Err(format!("{build}: logic-only installation changed game.dll"));
        }
        if unit_apply::apply(&modules, &export, &init)? != "patched" {
            return Err(format!("{build}: adding game.dll installation failed"));
        }
        blocks.0.push(
            unit_apply::installed_block(&process, &modules[1])?
                .ok_or("game allocation is missing")?,
        );
        if unit_apply::apply(&modules[..1], &export, &init)? != "already patched; nothing to do"
            || unit_apply::apply(&modules, &export, &init)? != "already patched; nothing to do"
        {
            return Err(format!("{build}: first/repeat installation failed"));
        }
        println!(
            "  {build}: logic-only, both modules and repeat installs verify every hook and payload"
        );
    }
    Ok(())
}

/// No hook executes in a stand-in; its allocations are released even when
/// later verification returns an error.
struct TestBlocks(Vec<usize>);

impl Drop for TestBlocks {
    fn drop(&mut self) {
        for &block in &self.0 {
            unsafe {
                VirtualFree(block as *mut core::ffi::c_void, 0, 0x8000);
            }
        }
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn VirtualFree(address: *mut core::ffi::c_void, size: usize, kind: u32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_units_cover_every_committed_build_and_unit() {
        use std::collections::BTreeSet;
        use std::fs;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let variants = root.join("tools/variants");
        let mut committed = BTreeSet::new();
        for folder in fs::read_dir(&variants).unwrap() {
            let folder = folder.unwrap();
            if !folder.file_type().unwrap().is_dir() {
                continue;
            }
            let build = folder.file_name().into_string().unwrap();
            for descriptor in fs::read_dir(folder.path().join("units")).unwrap() {
                let path = descriptor.unwrap().path();
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let text = fs::read_to_string(&path).unwrap();
                let unit = Unit::parse(&text).unwrap();
                assert!(committed.insert((build.clone(), unit.name.clone())));
                let embedded = UNITS
                    .iter()
                    .find(|u| {
                        u.build == build && Unit::parse(u.descriptor).unwrap().name == unit.name
                    })
                    .unwrap_or_else(|| panic!("missing embedded unit {build}/{}", unit.name));
                assert_eq!(embedded.descriptor, text);
                assert_eq!(embedded.code, fs::read(path.with_extension("bin")).unwrap());
            }
        }
        let embedded: BTreeSet<_> = UNITS
            .iter()
            .map(|u| (u.build.to_string(), Unit::parse(u.descriptor).unwrap().name))
            .collect();
        assert_eq!(UNITS.len(), embedded.len(), "duplicate embedded units");
        assert_eq!(embedded, committed);
        let profiles: BTreeSet<_> = fs::read_dir(root.join("tools/layouts"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names()
                .into_iter()
                .map(str::to_string)
                .collect::<BTreeSet<_>>(),
            profiles
        );
    }

    #[test]
    fn default_cells_preserve_stock_wheel_and_leave_loader_callbacks_absent() {
        for build in names() {
            for (unit, original) in units(build).unwrap() {
                let mut code = original.to_vec();
                initialize(&unit, &mut code, 0x12345678).unwrap();
                for (name, value) in [
                    ("marquee", 0x12345678),
                    ("tab_modifier", 0x11),
                    ("step", 0x11),
                    ("preview_dim", 0),
                ] {
                    if let Some(cell) = unit.cell(name) {
                        assert_eq!(
                            u64::from_le_bytes(
                                code[cell.offset..cell.offset + 8].try_into().unwrap()
                            ),
                            value
                        );
                    }
                }
                if let Some(cell) = unit.cell("step") {
                    assert!(code[cell.offset + 8..cell.offset + cell.bytes]
                        .iter()
                        .all(|&byte| byte == 0));
                }
            }
        }
    }

    #[test]
    fn pairs_choose_their_layout_and_mixed_known_modules_are_refused() {
        for build in names() {
            let (logic, game) = hashes(build).unwrap();
            assert_eq!(
                select(&logic.source_sha256, &game.source_sha256).unwrap(),
                Some(build)
            );
            assert!(select(&logic.source_sha256, "unknown").is_err());
            assert!(select("unknown", &game.source_sha256).is_err());
            for other in names().into_iter().filter(|other| *other != build) {
                let (_, other_game) = hashes(other).unwrap();
                assert!(select(&logic.source_sha256, &other_game.source_sha256).is_err());
            }
        }
        let (logic, game) = hashes("reference").unwrap();
        assert_eq!(
            select(&logic.source_sha256, &game.source_sha256).unwrap(),
            None
        );
        assert_eq!(select(&logic.verified[0], &game.verified[0]).unwrap(), None);
    }
}
