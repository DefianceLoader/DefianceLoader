use crate::apply::Process;
use crate::unit::{Anchor, Fixup, Unit};
use crate::unit_apply::{self, Module, Payload};
use crate::Target;
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[link(name = "kernel32")]
extern "system" {
    fn VirtualFree(address: *mut u8, size: usize, kind: u32) -> i32;
}

const UNIT: &str = r#"{
 "unit_schema": 1, "name": "test-logic", "plugin": "test",
 "module": "logic.dll", "source_sha256": "aa", "unit_bytes": 48,
 "code_offset": 16,
 "cells": [{"name": "state", "offset": 0, "bytes": 16}],
 "writes": [
  {"kind": "jmp", "rva": 16, "before": "48895c2408", "entry": 16,
   "label": "entry", "tail": ""},
  {"kind": "edit", "rva": 64, "before": "7502", "after": "9090"}
 ],
 "fixups": [], "natives": [],
 "anchors": [{"rva": 96, "bytes": "c3"}], "sites": [], "verified_sha": ""
}"#;

struct Image {
    target: Target,
    blocks: RefCell<Vec<usize>>,
}

impl Image {
    fn new(unit: &Unit) -> Self {
        let process = Process::open(std::process::id()).unwrap();
        let base = process
            .reserve_near(Self::new as *const () as *mut u8, 4096)
            .unwrap();
        let target = Target {
            process_id: std::process::id(),
            base,
            size: 4096,
            path: PathBuf::new(),
        };
        for write in &unit.writes {
            process
                .write(unsafe { base.add(write.rva) }, &write.before)
                .unwrap();
        }
        for anchor in &unit.anchors {
            process
                .write(unsafe { base.add(anchor.rva) }, &anchor.bytes)
                .unwrap();
        }
        Self {
            target,
            blocks: RefCell::new(Vec::new()),
        }
    }

    fn read(&self) -> Vec<u8> {
        Process::open(self.target.process_id)
            .unwrap()
            .read(self.target.base, self.target.size)
            .unwrap()
    }

    fn installed(&self, module: &Module<'_>) -> usize {
        let process = Process::open(self.target.process_id).unwrap();
        let block = unit_apply::installed_block(&process, module)
            .unwrap()
            .unwrap();
        if !self.blocks.borrow().contains(&block) {
            self.blocks.borrow_mut().push(block);
        }
        block
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        for block in self.blocks.get_mut() {
            unsafe { VirtualFree(*block as *mut u8, 0, 0x8000) };
        }
        unsafe { VirtualFree(self.target.base, 0, 0x8000) };
    }
}

fn no_export(_: &str, _: &str) -> Result<usize, String> {
    Err("unexpected export".into())
}

fn no_initialize(_: &Unit, _: &mut [u8]) -> Result<(), String> {
    Ok(())
}

#[test]
fn a_later_module_mismatch_leaves_both_images_pristine() {
    let logic = Unit::parse(UNIT).unwrap();
    let game = Unit::parse(
        &UNIT
            .replace("test-logic", "test-game")
            .replace("logic.dll", "game.dll"),
    )
    .unwrap();
    let logic_image = Image::new(&logic);
    let game_image = Image::new(&game);
    Process::open(std::process::id())
        .unwrap()
        .write(unsafe { game_image.target.base.add(64) }, &[0xcc, 0xcc])
        .unwrap();
    let before_logic = logic_image.read();
    let before_game = game_image.read();
    let code = vec![0xc3; 48];
    let logic_payloads = [Payload {
        unit: &logic,
        code: &code,
    }];
    let game_payloads = [Payload {
        unit: &game,
        code: &code,
    }];
    let modules = [
        Module {
            target: &logic_image.target,
            payloads: &logic_payloads,
        },
        Module {
            target: &game_image.target,
            payloads: &game_payloads,
        },
    ];
    assert!(unit_apply::apply(&modules, &no_export, &no_initialize).is_err());
    assert_eq!(logic_image.read(), before_logic);
    assert_eq!(game_image.read(), before_game);
}

#[test]
fn repeat_install_preserves_mutated_private_cells_and_shared_ring() {
    let unit = Unit::parse(UNIT).unwrap();
    let image = Image::new(&unit);
    let code = vec![0xc3; 48];
    let payloads = [Payload {
        unit: &unit,
        code: &code,
    }];
    let module = Module {
        target: &image.target,
        payloads: &payloads,
    };
    assert_eq!(
        unit_apply::apply(std::slice::from_ref(&module), &no_export, &no_initialize).unwrap(),
        "patched"
    );
    let block = image.installed(&module);
    let private = unit_apply::cell(&module, block, &unit.name, "state").unwrap();
    let ring = unit_apply::cell(&module, block, &unit.name, "trace_ring").unwrap();
    let process = Process::open(std::process::id()).unwrap();
    process.write(private as *mut u8, &[0x42; 16]).unwrap();
    process.write(ring as *mut u8, &[0x73; 16]).unwrap();
    let installed_image = image.read();
    assert_eq!(
        unit_apply::apply(std::slice::from_ref(&module), &no_export, &no_initialize).unwrap(),
        "already patched; nothing to do"
    );
    assert_eq!(image.installed(&module), block);
    assert_eq!(image.read(), installed_image);
    assert_eq!(process.read(private as *const u8, 16).unwrap(), [0x42; 16]);
    assert_eq!(process.read(ring as *const u8, 16).unwrap(), [0x73; 16]);
}

#[test]
fn a_partial_install_is_refused_without_repairing_the_image() {
    let unit = Unit::parse(UNIT).unwrap();
    let image = Image::new(&unit);
    let code = vec![0xc3; 48];
    let payloads = [Payload {
        unit: &unit,
        code: &code,
    }];
    let module = Module {
        target: &image.target,
        payloads: &payloads,
    };
    unit_apply::apply(std::slice::from_ref(&module), &no_export, &no_initialize).unwrap();
    image.installed(&module);
    Process::open(std::process::id())
        .unwrap()
        .write(unsafe { image.target.base.add(64) }, &unit.writes[1].before)
        .unwrap();
    let partial = image.read();
    let error =
        unit_apply::apply(std::slice::from_ref(&module), &no_export, &no_initialize).unwrap_err();
    assert!(error.contains("partially installed"), "{error}");
    assert_eq!(image.read(), partial);
}

#[test]
fn a_foreign_hook_and_modified_installed_code_are_refused() {
    let unit = Unit::parse(UNIT).unwrap();
    let image = Image::new(&unit);
    let code = vec![0xc3; 48];
    let payloads = [Payload {
        unit: &unit,
        code: &code,
    }];
    let module = Module {
        target: &image.target,
        payloads: &payloads,
    };
    let process = Process::open(std::process::id()).unwrap();
    process
        .write(unsafe { image.target.base.add(16) }, &[0xe9, 0xeb, 0, 0, 0])
        .unwrap();
    let foreign = image.read();
    let error =
        unit_apply::apply(std::slice::from_ref(&module), &no_export, &no_initialize).unwrap_err();
    assert!(error.contains("another patch"), "{error}");
    assert_eq!(image.read(), foreign);
    process
        .write(unsafe { image.target.base.add(16) }, &unit.writes[0].before)
        .unwrap();
    unit_apply::apply(std::slice::from_ref(&module), &no_export, &no_initialize).unwrap();
    let block = image.installed(&module);
    let cell = unit_apply::cell(&module, block, &unit.name, "state").unwrap();
    process.write((cell + 16) as *mut u8, &[0xcc]).unwrap();
    let error =
        unit_apply::apply(std::slice::from_ref(&module), &no_export, &no_initialize).unwrap_err();
    assert!(error.contains("code differs"), "{error}");
}

#[test]
fn a_later_linker_failure_publishes_no_module_hooks() {
    let first = Unit::parse(UNIT).unwrap();
    let mut second = first.clone();
    second.name = "test-game".into();
    second.module = "game.dll".into();
    second.fixups.push(Fixup::Export {
        offset: 24,
        dll: "missing.dll".into(),
        name: "missing".into(),
    });
    let first_image = Image::new(&first);
    let second_image = Image::new(&second);
    let before_first = first_image.read();
    let before_second = second_image.read();
    let code = vec![0xc3; 48];
    let first_payloads = [Payload {
        unit: &first,
        code: &code,
    }];
    let second_payloads = [Payload {
        unit: &second,
        code: &code,
    }];
    let modules = [
        Module {
            target: &first_image.target,
            payloads: &first_payloads,
        },
        Module {
            target: &second_image.target,
            payloads: &second_payloads,
        },
    ];
    let error = unit_apply::apply(&modules, &no_export, &no_initialize).unwrap_err();
    assert!(error.contains("unexpected export"), "{error}");
    assert_eq!(first_image.read(), before_first);
    assert_eq!(second_image.read(), before_second);
}

#[test]
fn validation_rejects_overlaps_and_out_of_bounds_writes_and_anchors() {
    let mut unit = Unit::parse(UNIT).unwrap();
    let image = Image::new(&unit).read();
    let code = vec![0xc3; 48];
    let validate = |unit: &Unit| unit_apply::validate(&[Payload { unit, code: &code }], &image);
    validate(&unit).unwrap();
    let mut overlapping = unit.writes[1].clone();
    overlapping.rva = unit.writes[0].rva + 1;
    unit.writes.push(overlapping);
    assert!(validate(&unit).unwrap_err().contains("overlap"));
    unit.writes.pop();
    unit.writes[1].rva = image.len() - 1;
    assert!(validate(&unit).unwrap_err().contains("outside"));
    unit.writes[1].rva = usize::MAX;
    assert!(validate(&unit).unwrap_err().contains("overflow"));
    unit.writes[1].rva = 64;
    unit.anchors.push(Anchor {
        rva: image.len(),
        bytes: vec![0xc3],
    });
    assert!(validate(&unit).unwrap_err().contains("outside"));
}

const REMOTE_FIXTURE_MARKER: &str = "DEFIANCE_UNIT_APPLY_TEST_CHILD";

struct RemoteFixture {
    child: Child,
    reader: Option<JoinHandle<()>>,
}

impl RemoteFixture {
    fn start() -> (Self, usize, usize) {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "unit_apply_tests::remote_process_fixture",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(REMOTE_FIXTURE_MARKER, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .creation_flags(0x0800_0000)
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || {
            let mut ready = Some(sender);
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let (Some(sender), Some(at)) = (ready.as_ref(), line.find("REMOTE_IMAGES ")) {
                    let values: Result<Vec<usize>, _> = line[at + "REMOTE_IMAGES ".len()..]
                        .split_whitespace()
                        .map(|address| usize::from_str_radix(address.trim_start_matches("0x"), 16))
                        .collect();
                    let _ = sender.send(values.map_err(|error| error.to_string()));
                    ready = None;
                }
            }
            if let Some(sender) = ready {
                let _ = sender.send(Err("child exited before reporting its images".into()));
            }
        });
        let fixture = Self {
            child,
            reader: Some(reader),
        };
        let addresses = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("child fixture readiness timed out")
            .expect("child fixture did not report image addresses");
        assert_eq!(addresses.len(), 2);
        (fixture, addresses[0], addresses[1])
    }

    fn finish(mut self) {
        self.child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"stop\n")
            .unwrap();
        self.child.stdin.as_mut().unwrap().flush().unwrap();
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "child fixture failed: {status}");
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "child fixture did not stop"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for RemoteFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[test]
fn remote_process_fixture() {
    if std::env::var(REMOTE_FIXTURE_MARKER).as_deref() != Ok("1") {
        return;
    }
    let logic = Unit::parse(UNIT).unwrap();
    let mut game = logic.clone();
    game.name = "test-game".into();
    game.module = "game.dll".into();
    let logic_image = Image::new(&logic);
    let game_image = Image::new(&game);
    println!(
        "\nREMOTE_IMAGES {:#x} {:#x}",
        logic_image.target.base as usize, game_image.target.base as usize
    );
    std::io::stdout().flush().unwrap();
    let mut command = String::new();
    std::io::stdin().read_line(&mut command).unwrap();
    assert_eq!(command.trim(), "stop");
}

#[test]
fn remote_preflight_and_first_repeat_use_the_cross_process_write_path() {
    let mut logic = Unit::parse(UNIT).unwrap();
    logic.fixups.push(Fixup::Export {
        offset: 24,
        dll: "fixture.dll".into(),
        name: "unused_target".into(),
    });
    let mut game = logic.clone();
    game.name = "test-game".into();
    game.module = "game.dll".into();
    let (fixture, logic_base, game_base) = RemoteFixture::start();
    let pid = fixture.child.id();
    assert_ne!(pid, std::process::id());
    let process = Process::open(pid).unwrap();
    let export = |dll: &str, name: &str| {
        assert_eq!((dll, name), ("fixture.dll", "unused_target"));
        Ok(game_base + 96)
    };
    let logic_target = Target {
        process_id: pid,
        base: logic_base as *mut u8,
        size: 4096,
        path: PathBuf::new(),
    };
    let game_target = Target {
        process_id: pid,
        base: game_base as *mut u8,
        size: 4096,
        path: PathBuf::new(),
    };
    let code = vec![0xc3; 48];
    let logic_payloads = [Payload {
        unit: &logic,
        code: &code,
    }];
    let game_payloads = [Payload {
        unit: &game,
        code: &code,
    }];
    let modules = [
        Module {
            target: &logic_target,
            payloads: &logic_payloads,
        },
        Module {
            target: &game_target,
            payloads: &game_payloads,
        },
    ];
    process
        .write((game_base + 64) as *mut u8, &[0xcc; 2])
        .unwrap();
    let before_logic = process.read(logic_target.base, logic_target.size).unwrap();
    let before_game = process.read(game_target.base, game_target.size).unwrap();
    assert!(unit_apply::apply(&modules, &export, &no_initialize).is_err());
    assert_eq!(
        process.read(logic_target.base, logic_target.size).unwrap(),
        before_logic
    );
    assert_eq!(
        process.read(game_target.base, game_target.size).unwrap(),
        before_game
    );
    process
        .write((game_base + 64) as *mut u8, &game.writes[1].before)
        .unwrap();
    assert_eq!(
        unit_apply::apply(&modules, &export, &no_initialize).unwrap(),
        "patched"
    );
    let blocks: Vec<_> = modules
        .iter()
        .map(|module| {
            unit_apply::installed_block(&process, module)
                .unwrap()
                .unwrap()
        })
        .collect();
    let state = unit_apply::cell(&modules[0], blocks[0], &logic.name, "state").unwrap();
    let linked_export = process.read((state + 24) as *const u8, 8).unwrap();
    assert_eq!(
        u64::from_le_bytes(linked_export.try_into().unwrap()),
        (game_base + 96) as u64
    );
    process.write(state as *mut u8, &[0x5a; 16]).unwrap();
    assert_eq!(
        unit_apply::apply(&modules, &export, &no_initialize).unwrap(),
        "already patched; nothing to do"
    );
    assert_eq!(process.read(state as *const u8, 16).unwrap(), [0x5a; 16]);
    for (module, block) in modules.iter().zip(blocks) {
        assert_eq!(
            unit_apply::installed_block(&process, module).unwrap(),
            Some(block)
        );
        for (address, _, after) in module.payloads[0]
            .unit
            .placed(module.target.base as usize, block + 32)
            .unwrap()
        {
            assert_eq!(
                process.read(address as *const u8, after.len()).unwrap(),
                after
            );
        }
    }
    // The fixture owns only unused synthetic images; no injected code runs.
    fixture.finish();
}
