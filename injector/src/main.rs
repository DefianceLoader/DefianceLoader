//! Applies the patches to a running game, so nothing in the game directory is
//! modified. Two modules: logic.dll for the simulation and game.dll for the
//! squad panel.
//!
//! Start this, then start the game normally. It waits for trm.exe to appear,
//! waits for logic.dll to be loaded, checks that the DLL on disk is the build
//! the patch was written for, and then applies the patch. The patch itself —
//! the descriptor, the signature relocation and the writes — lives in
//! `defiance-core`, shared with the loader; this file is the process discovery
//! and the command line around it.
//!
//! The block is committed executable and writable near the module, because the
//! one call that reaches it is a rel32 and so must land within 2GB. The
//! rotation cursor sits at a fixed offset inside the same block, addressed
//! rip-relative, which is what lets the payload be position independent.

mod settings;

use defiance_core::apply::Process;
use defiance_core::descriptor::{GamePatch, Patch};
use defiance_core::install::{self, Scan};
use defiance_core::{relocate, sha256, Target};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

// --- the patch, baked in at build time by tools/payload.py -----------------
const PAYLOAD: &[u8] = include_bytes!("../../tools/variants/reference/logic.bin");
const DESCRIPTOR: &str = include_str!("../../tools/variants/reference/logic.json");

// The one patch that targets game.dll rather than logic.dll. It never becomes
// a file patch, so this is the only way it installs.
const GAME_PAYLOAD: &[u8] = include_bytes!("../../tools/variants/reference/game.bin");
const GAME_DESCRIPTOR: &str = include_str!("../../tools/variants/reference/game.json");

fn logic_patch() -> Patch {
    Patch::parse(DESCRIPTOR)
}

fn game_patch() -> GamePatch {
    GamePatch::parse(GAME_DESCRIPTOR)
}

// --- the Win32 surface, declared rather than pulled in ----------------------
type Handle = *mut std::ffi::c_void;
const TH32CS_SNAPPROCESS: u32 = 0x2;
const TH32CS_SNAPMODULE: u32 = 0x8;
const TH32CS_SNAPMODULE32: u32 = 0x10;
const INVALID_HANDLE: Handle = -1isize as Handle;

#[repr(C)]
struct ProcessEntry32W {
    size: u32,
    usage: u32,
    process_id: u32,
    default_heap_id: usize,
    module_id: u32,
    threads: u32,
    parent_process_id: u32,
    base_priority: i32,
    flags: u32,
    exe_file: [u16; 260],
}

#[repr(C)]
struct ModuleEntry32W {
    size: u32,
    module_id: u32,
    process_id: u32,
    global_usage_count: u32,
    process_usage_count: u32,
    base_address: *mut u8,
    module_size: u32,
    module_handle: Handle,
    module_name: [u16; 256],
    exe_path: [u16; 260],
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> Handle;
    fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
    fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
    fn Module32FirstW(snapshot: Handle, entry: *mut ModuleEntry32W) -> i32;
    fn Module32NextW(snapshot: Handle, entry: *mut ModuleEntry32W) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
}

/// A hex number as the probes print it, with or without the `0x`.
fn parse_hex(text: &str) -> Option<usize> {
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    usize::from_str_radix(digits, 16).ok()
}

fn wide_to_string(chars: &[u16]) -> String {
    let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
    OsString::from_wide(&chars[..end])
        .to_string_lossy()
        .into_owned()
}

// --- finding the game -------------------------------------------------------
fn find_process(name: &str) -> Option<u32> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE {
            return None;
        }
        let mut entry: ProcessEntry32W = std::mem::zeroed();
        entry.size = std::mem::size_of::<ProcessEntry32W>() as u32;
        let mut found = None;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                if wide_to_string(&entry.exe_file).eq_ignore_ascii_case(name) {
                    found = Some(entry.process_id);
                    break;
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        found
    }
}

/// Every loaded module, for the probes to annotate a pointer against.
fn all_modules(process_id: u32) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    unsafe {
        let snapshot =
            CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, process_id);
        if snapshot == INVALID_HANDLE {
            return out;
        }
        let mut entry: ModuleEntry32W = std::mem::zeroed();
        entry.size = std::mem::size_of::<ModuleEntry32W>() as u32;
        if Module32FirstW(snapshot, &mut entry) != 0 {
            loop {
                out.push((
                    wide_to_string(&entry.module_name),
                    entry.base_address as usize,
                    entry.module_size as usize,
                ));
                if Module32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        out
    }
}

fn find_module(process_id: u32, name: &str) -> Option<Target> {
    unsafe {
        let snapshot =
            CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, process_id);
        if snapshot == INVALID_HANDLE {
            return None;
        }
        let mut entry: ModuleEntry32W = std::mem::zeroed();
        entry.size = std::mem::size_of::<ModuleEntry32W>() as u32;
        let mut found = None;
        if Module32FirstW(snapshot, &mut entry) != 0 {
            loop {
                if wide_to_string(&entry.module_name).eq_ignore_ascii_case(name) {
                    found = Some(Target {
                        process_id,
                        base: entry.base_address,
                        size: entry.module_size as usize,
                        path: PathBuf::from(wide_to_string(&entry.exe_path)),
                    });
                    break;
                }
                if Module32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        found
    }
}

fn wait_for_game(timeout: Duration) -> Result<Target, String> {
    let started = Instant::now();
    let mut announced = false;
    loop {
        if let Some(process_id) = find_process("trm.exe") {
            if let Some(target) = find_module(process_id, "logic.dll") {
                return Ok(target);
            }
            if !announced {
                println!("the game is starting; waiting for logic.dll");
                announced = true;
            }
        } else if !announced {
            println!("waiting for the game to start (start it now)");
            announced = true;
        }
        if started.elapsed() > timeout {
            return Err(format!(
                "gave up after {}s waiting for the game",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

// --- the probes -------------------------------------------------------------

/// Reads back what squad_of recorded, for a game that is already patched and
/// has had an icon clicked. The block is not remembered between runs, so it is
/// recovered the way the self test does: decode the hook's rel32 and subtract
/// the entry offset.
fn probe(patch: &GamePatch, target: &Target) -> Result<(), String> {
    let process = Process::open(target.process_id)?;
    let at = |rva: usize| unsafe { target.base.add(rva) };

    let hook = patch.hooks.first().ok_or("the patch has no hooks")?;
    let site = process.read(at(hook.rva), 5)?;
    if site[0] != 0xe9 {
        return Err(format!(
            "game.dll rva {:#x} is not hooked; run the injector with --game, click an \
             icon, then probe again",
            hook.rva
        ));
    }
    let rel = i32::from_le_bytes([site[1], site[2], site[3], site[4]]) as isize;
    let entry = unsafe { at(hook.rva + 5).offset(rel) };
    let block = unsafe { entry.sub(hook.entry) };
    println!("block at {block:p}");

    let trace = process.read(unsafe { block.add(patch.trace_offset) }, 0x60)?;
    let slot = |i: usize| u64::from_le_bytes(trace[i * 8..i * 8 + 8].try_into().unwrap());
    let (single, ctrl, double, calls) = (slot(8), slot(9), slot(10), slot(7));
    println!(
        "hook entries: single click {single}, double click {double}, \
              squad_of {calls} (last Ctrl flag {ctrl})"
    );
    if single == 0 && double == 0 {
        return Err(
            "neither hook has been reached. If the game was already running \
                    when this build was injected, restart it and inject again; \
                    otherwise fn_1f4370 is not on the icon click path after all"
                .to_string(),
        );
    }
    if calls == 0 {
        return Err(format!(
            "the hooks ran ({single} single, {double} double) but squad_of never did. \
             For the single click that means the Ctrl branch was taken every time, \
             with the flag last seen as {ctrl}"
        ));
    }
    println!("the last chain walk went:");
    for (i, what) in [
        "the entity handed in",
        "its facets, from vt+0xb0",
        "its selectable facet, facets+0x50",
        "the parent facet, selectable+0x28",
        "parent+0x10",
        "parent+0x10+0x10",
        "what it returned",
    ]
    .iter()
    .enumerate()
    {
        println!("  +{:#04x} {:#018x}  {what}", i * 8, slot(i));
    }
    if slot(6) == slot(0) {
        println!("\nit fell back to the member, so one of the hops above is wrong");
    }

    // Dump the objects the chain reached, so the real layout can be read off
    // rather than guessed. A qword is flagged when it points at something whose
    // first qword is itself a plausible vtable, which is what an object looks
    // like from the outside.
    let modules = all_modules(target.process_id);
    let describe = |value: u64| -> String {
        for (name, base, size) in &modules {
            if value as usize >= *base && (value as usize) < base + size {
                return format!(" -> {name}+{:#x}", value as usize - base);
            }
        }
        String::new()
    };

    for (label, object) in [
        ("the selectable facet", slot(2)),
        ("the parent facet", slot(3)),
    ] {
        if object == 0 {
            continue;
        }
        println!("\n{label} at {object:#x}:");
        let body = process.read(object as *const u8, 0x60)?;
        for i in 0..body.len() / 8 {
            let value = u64::from_le_bytes(body[i * 8..i * 8 + 8].try_into().unwrap());
            if value == 0 {
                continue;
            }
            let mut note = describe(value);
            if note.is_empty() {
                // does it look like an object? read its first qword as a vtable
                if let Ok(head) = process.read(value as *const u8, 8) {
                    let vtable = u64::from_le_bytes(head.try_into().unwrap());
                    let where_ = describe(vtable);
                    if !where_.is_empty() {
                        note = format!(" object, vtable{where_}");
                    }
                }
            }
            println!("  +{:#04x} {value:#018x}{note}", i * 8);
        }
    }
    Ok(())
}

/// One object's qwords, annotated, with a zero line dropped. Used by the select
/// probe to read an entity's facet chain without calling into the game.
#[allow(dead_code)] // kept for the next structure that needs reading
fn dump_object(
    process: &Process,
    base: u64,
    len: usize,
    describe: &dyn Fn(u64) -> String,
    indent: &str,
) {
    let Ok(body) = process.read(base as *const u8, len) else {
        println!("{indent}(not readable)");
        return;
    };
    for (k, chunk) in body.chunks(8).enumerate() {
        let value = u64::from_le_bytes(chunk.try_into().unwrap());
        if value == 0 {
            continue;
        }
        let mut note = describe(value);
        if note.is_empty() {
            if let Ok(head) = process.read(value as *const u8, 8) {
                let vtable = u64::from_le_bytes(head.try_into().unwrap());
                let where_ = describe(vtable);
                if !where_.is_empty() {
                    note = format!(" object, vtable{where_}");
                }
            }
        }
        println!("{indent}+{:#04x} {value:#018x}{note}", k * 8);
    }
}

/// Reads back the manager trace: every select, toggle, deselect and clear-all
/// the selection manager received, in order, each with its caller. The block
/// is not remembered between runs, so it is recovered from the first trace
/// hook's jump, the same way the self test finds it.
/// Where the last injection recorded the block it allocated. In a build whose
/// sites moved, the patched bytes no longer match the signatures, so
/// `--select-probe` cannot find the hook again; it reads the address here
/// instead, after checking it still belongs to this process and module.
fn block_record_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|dir| dir.join("defiance-pickup-inject.block"))
}

/// The block the last injection allocated, and the trace hook that reaches it,
/// if the record is present and still matches this process.
fn recorded_block(target: &Target, process: &Process) -> Option<(usize, *mut u8)> {
    let text = block_record_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .or_else(|| {
            target
                .path
                .parent()
                .and_then(|p| std::fs::read_to_string(p.join("defiance-pickup-inject.block")).ok())
        })?;
    let field = |key: &str| -> Option<usize> {
        let line = text.lines().find(|line| line.starts_with(key))?;
        let value = line.split_once('=')?.1.trim();
        usize::from_str_radix(value.strip_prefix("0x").unwrap_or(value), 16).ok()
    };
    let pid = field("pid")?;
    let base = field("base")?;
    let block = field("block")?;
    let trace = field("trace")?;
    let entry = field("entry")?;
    if pid != target.process_id as usize || base != target.base as usize {
        return None;
    }
    let at = |rva: usize| unsafe { target.base.add(rva) };
    let site = process.read(at(trace), 5).ok()?;
    if site[0] != 0xe9 {
        return None;
    }
    let rel = i32::from_le_bytes([site[1], site[2], site[3], site[4]]) as isize;
    if unsafe { at(trace + 5).offset(rel) as usize } != block + entry {
        return None;
    }
    Some((trace, block as *mut u8))
}

fn select_probe(patch: &Patch, target: &Target) -> Result<(), String> {
    const SOLDIER_FACET: usize = 0x7289d8;
    const SQUAD_FACET: usize = 0x7268b8;
    const ENTRIES: usize = 32;
    let process = Process::open(target.process_id)?;
    let at = |rva: usize| unsafe { target.base.add(rva) };
    let base = target.base as usize;

    // In a build whose sites moved, the patched hook does not hold the
    // signature, so the block is taken from the record the injector wrote.
    // Failing that, the build the patch was written for still has its hook.
    let block = match recorded_block(target, &process) {
        Some((_, block)) => block,
        None => {
            let hook = patch
                .detours
                .first()
                .ok_or("the patch carries no trace hooks")?;
            let site = process.read(at(hook.rva), 5)?;
            if site[0] != 0xe9 {
                return Err(format!(
                    "logic.dll rva {:#x} is not hooked; restart the game, run the injector, \
                     and make some selections first",
                    hook.rva
                ));
            }
            let rel = i32::from_le_bytes([site[1], site[2], site[3], site[4]]) as isize;
            unsafe { at(hook.rva + 5).offset(rel).sub(hook.entry) }
        }
    };

    let ring = process.read(
        unsafe { block.add(patch.trace_offset) },
        0x10 + ENTRIES * 16,
    )?;
    let qword = |o: usize| u64::from_le_bytes(ring[o..o + 8].try_into().unwrap());
    let calls = qword(0);
    if calls == 0 {
        println!("the trace has not recorded anything yet (the census below may have)");
    }

    let modules = all_modules(target.process_id);
    let describe = |value: u64| -> String {
        for (name, module_base, size) in &modules {
            if value as usize >= *module_base && (value as usize) < module_base + size {
                return format!("{name}+{:#x}", value as usize - module_base);
            }
        }
        format!("{value:#x}")
    };
    let read_q = |p: u64| -> u64 {
        process
            .read(p as *const u8, 8)
            .ok()
            .and_then(|b| b.try_into().ok())
            .map(u64::from_le_bytes)
            .unwrap_or(0)
    };
    let read_b = |p: u64| -> u8 {
        process
            .read(p as *const u8, 1)
            .map(|b| b[0])
            .unwrap_or(0xff)
    };
    let plausible = |v: u64| (0x10000..0x800000000000).contains(&v);

    // what an entity is, and its squad if it is a soldier
    let classify = |entity: u64| -> (&'static str, u64, String) {
        if !plausible(entity) {
            return ("?", 0, String::new());
        }
        let facet = read_q(entity + 0xb8);
        if !plausible(facet) {
            return ("?", 0, String::new());
        }
        let vtable = read_q(facet) as usize;
        if vtable == base + SOLDIER_FACET {
            let parent = read_q(facet + 0x28);
            let squad = if plausible(parent) {
                let holder = read_q(parent + 0x10);
                if plausible(holder) {
                    read_q(holder + 0x10)
                } else {
                    0
                }
            } else {
                0
            };
            let marks = format!(
                "enabled {} own-mark {} squad-selected {}",
                read_b(facet + 0x18),
                read_b(facet + 0x30),
                if plausible(parent) {
                    read_b(parent + 0x28)
                } else {
                    0xff
                }
            );
            ("soldier", squad, marks)
        } else if vtable == base + SQUAD_FACET {
            let marks = format!(
                "enabled {} selected {}",
                read_b(facet + 0x18),
                read_b(facet + 0x28)
            );
            ("squad", 0, marks)
        } else {
            ("?", 0, format!("facet vtable {}", describe(vtable as u64)))
        }
    };

    let shown = calls.min(ENTRIES as u64) as usize;
    let first = if calls > ENTRIES as u64 {
        (calls % ENTRIES as u64) as usize
    } else {
        0
    };
    let mut aliases: Vec<u64> = Vec::new();
    let alias = |entity: u64, aliases: &mut Vec<u64>| -> String {
        if entity == 0 {
            return "-".to_string();
        }
        let n = match aliases.iter().position(|&e| e == entity) {
            Some(n) => n,
            None => {
                aliases.push(entity);
                aliases.len() - 1
            }
        };
        format!("E{}", n + 1)
    };

    println!("manager trace at {block:p}; {calls} call(s), the last {shown} in order:\n");
    for n in 0..shown {
        let i = (first + n) % ENTRIES;
        let tagged = qword(0x10 + i * 16);
        let subject = qword(0x10 + i * 16 + 8);
        let caller = tagged & 0x00ff_ffff_ffff_ffff;
        // bits 56-59 the manager's four entries; 60 and 61 the pose split, which
        // records the entity the panel sent and then each soldier it ordered;
        // 0x40 up the ammo setter's own trace: what it was handed and which
        // branch it took
        let code = ((tagged >> 56) & 0xff) as u8;
        let kind = match code {
            1 => "select  ",
            2 => "toggle  ",
            4 => "deselect",
            8 => "clear   ",
            0x10 => "lie down",
            0x20 => "stand up",
            0x40 => "ammo set",
            0x41 => "ammo slt",
            0x42 => "ammo cts",
            0x43 => "ammo all",
            0x44 => "ammo mkd",
            0x45 => "ammo one",
            0x46 => "write ftc",
            0x47 => "write dat",
            0x48 => "entry dat",
            0x49 => "ammo pin",
            _ => "?       ",
        };
        let who = match code {
            1 | 2 | 4 | 0x10 | 0x20 => {
                let name = alias(subject, &mut aliases);
                let (what, squad, _) = classify(subject);
                if what == "soldier" && squad != 0 {
                    format!("{name} soldier of {}", alias(squad, &mut aliases))
                } else {
                    format!("{name} {what}")
                }
            }
            8 => "(manager)".to_string(),
            0x40 => {
                // the subject is the AI facet the setter was called on; its
                // entity is facet +0x10 -> holder +0x10 -> entity
                let holder = read_q(subject + 0x10);
                let entity = if plausible(holder) {
                    read_q(holder + 0x10)
                } else {
                    0
                };
                let name = alias(entity, &mut aliases);
                let (what, squad, _) = classify(entity);
                let at = format!("facet {}", describe(subject));
                if what == "soldier" && squad != 0 {
                    format!("{name} soldier of {} ({at})", alias(squad, &mut aliases))
                } else {
                    format!("{name} {what} ({at})")
                }
            }
            0x41 => format!("slot {} value {}", subject & 0xffff_ffff, subject >> 32),
            0x42 => format!(
                "marked {} selectable {}",
                subject & 0xffff_ffff,
                subject >> 32
            ),
            0x43 => "the squad".to_string(),
            0x44 => "the marked soldiers".to_string(),
            0x45 => "one entity".to_string(),
            0x46 => format!("facet {}", describe(subject)),
            0x47 | 0x48 => format!("data {}", describe(subject)),
            0x49 => {
                // the facet the pin landed on, with its +0x18..+0x1f bytes
                let bytes = process
                    .read((subject + 0x18) as *const u8, 8)
                    .unwrap_or_default();
                let at = |i: usize| bytes.get(i).copied().unwrap_or(0xff);
                format!(
                    "pinned {} enabled {:#x} marker {:#x} set {:#x} value {:#x}",
                    describe(subject),
                    at(0),
                    at(1),
                    at(6),
                    at(7)
                )
            }
            _ => String::new(),
        };
        let from = if caller == 0 {
            "-".to_string()
        } else {
            describe(caller)
        };
        println!("  [{n:2}] {kind} {who:<34} from {from}");
    }

    println!("\nstate now:");
    for (n, &entity) in aliases.iter().enumerate() {
        let (what, _, marks) = classify(entity);
        let facet = if plausible(entity) {
            read_q(entity + 0xb8)
        } else {
            0
        };
        println!(
            "  E{:<3} {entity:#014x} {what:<8} {marks}  facet {}",
            n + 1,
            describe(facet)
        );
    }

    // Counts pin-disabled queries, including loaded-round readiness. This is
    // diagnostic only; the patch never retrieves game pointers from this cell.
    if let Ok(cell) = process.read(unsafe { block.add(patch.ammo_scratch) }, 0x58) {
        let at = |o: usize| u64::from_le_bytes(cell[o..o + 8].try_into().unwrap());
        println!(
            "\nammo pin: {} query(s) answered disabled; last facet {} slot {}",
            at(0x40),
            describe(at(0x48)),
            at(0x50)
        );
    }

    // who read a squad's behaviour, from the census the getter's replacement
    // keeps: 32 entries of return address and count
    let census = process.read(unsafe { block.add(patch.census_offset) }, 32 * 16)?;
    let entry = |i: usize, half: usize| {
        u64::from_le_bytes(
            census[i * 16 + half * 8..i * 16 + half * 8 + 8]
                .try_into()
                .unwrap(),
        )
    };
    let mut readers: Vec<(u64, u64)> = (0..32)
        .map(|i| (entry(i, 0), entry(i, 1)))
        .filter(|&(at, _)| at != 0)
        .collect();
    readers.sort_by(|a, b| b.1.cmp(&a.1));
    println!(
        "\nwho read a squad's behaviour ({} callers):",
        readers.len()
    );
    for (at, count) in readers {
        println!("  {count:>10}  from {}", describe(at));
    }
    Ok(())
}

// --- the command line -------------------------------------------------------

/// Prints the exact address of a slot's enabled dword, and nothing else. Safe:
/// no handler is installed and no page is touched, so it can be used to feed a
/// breakpoint to an external debugger (Cheat Engine, x64dbg).
fn slot_address(target: &Target, data: u64, slot: u64) -> Result<(), String> {
    let process = Process::open(target.process_id)?;
    let read_q = |p: u64| -> Option<u64> {
        process
            .read(p as *const u8, 8)
            .ok()
            .and_then(|b| b.try_into().ok())
            .map(u64::from_le_bytes)
    };
    let read_d = |p: u64| -> Option<u32> {
        process
            .read(p as *const u8, 4)
            .ok()
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
    };
    let begin = read_q(data + 0x20).ok_or("the data object has no vector")?;
    let end = read_q(data + 0x28).ok_or("the data object has no vector")?;
    if end <= begin || (end - begin) % 0x48 != 0 {
        return Err(format!(
            "the slot vector {begin:#x}..{end:#x} does not look like records"
        ));
    }
    let count = (end - begin) / 0x48;
    if slot >= count {
        return Err(format!(
            "slot {slot} is past the {count} records of the vector"
        ));
    }
    let address = begin + slot * 0x48 + 0x3c;
    println!(
        "data object {data:#x}: {count} records at {begin:#x}; \
         slot {slot}'s enabled dword is {address:#x} (now {})",
        read_d(address)
            .map(|v| v.to_string())
            .unwrap_or("?".to_string())
    );
    println!(
        "give that address to Cheat Engine: 4 Bytes, then 'find out what accesses this address'"
    );
    Ok(())
}

fn main() {
    // The core's progress notes go to this console.
    defiance_core::report::set(Box::new(|line| println!("{line}")));

    // command-line options, each overriding its line in the settings file
    let mut timeout: Option<u64> = None;
    let mut with_game: Option<bool> = None;
    let mut scan: Option<Scan> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--wait" => {
                let seconds = args
                    .next()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or_else(|| {
                        eprintln!("--wait needs a number of seconds");
                        std::process::exit(2);
                    });
                timeout = Some(seconds);
            }
            "--probe" => {
                let patch = game_patch();
                if let Err(message) = wait_for_game(Duration::from_secs(10)) {
                    eprintln!("{message}");
                    std::process::exit(1);
                }
                let game = find_process("trm.exe").and_then(|pid| find_module(pid, "game.dll"));
                let Some(game) = game else {
                    eprintln!("game.dll is not loaded");
                    std::process::exit(1);
                };
                if let Err(message) = probe(&patch, &game) {
                    eprintln!("{message}");
                    std::process::exit(1);
                }
                return;
            }
            "--select-probe" => {
                let patch = logic_patch();
                let target = match wait_for_game(Duration::from_secs(10)) {
                    Ok(target) => target,
                    Err(message) => {
                        eprintln!("{message}");
                        std::process::exit(1);
                    }
                };
                if let Err(message) = select_probe(&patch, &target) {
                    eprintln!("{message}");
                    std::process::exit(1);
                }
                return;
            }
            // diagnostic: just print a slot's enabled-dword address for an
            // external debugger to watch. Writes and arms nothing.
            "--slot-address" => {
                let usage = || -> ! {
                    eprintln!("--slot-address needs the ammo data object (hex) and the slot");
                    std::process::exit(2);
                };
                let data = args
                    .next()
                    .and_then(|v| parse_hex(&v))
                    .unwrap_or_else(|| usage());
                let slot = args
                    .next()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or_else(|| usage());
                let target = match wait_for_game(Duration::from_secs(10)) {
                    Ok(target) => target,
                    Err(message) => {
                        eprintln!("{message}");
                        std::process::exit(1);
                    }
                };
                if let Err(message) = slot_address(&target, data as u64, slot) {
                    eprintln!("{message}");
                    std::process::exit(1);
                }
                return;
            }
            // The game.dll half is the selection mode, so it is on by default.
            "--game" => with_game = Some(true),
            "--no-game" => with_game = Some(false),
            // an unknown build is searched for the patch's signatures
            "--scan" => scan = Some(Scan::Unknown),
            "--force-scan" => scan = Some(Scan::Always),
            // would --scan find every site in the DLLs in a folder? writes nothing
            "--scan-check" => {
                let dir = args.next().unwrap_or_else(|| {
                    eprintln!("--scan-check needs the game's bin folder");
                    std::process::exit(2);
                });
                let found =
                    relocate::check_dir(&logic_patch(), &game_patch(), std::path::Path::new(&dir));
                std::process::exit(if found { 0 } else { 1 });
            }
            // RTTI of a DLL on disk, for cross-checking the loader's runtime walk
            // against tools/rtti.py without a game running
            "--rtti" => {
                let class = args.next().unwrap_or_else(|| {
                    eprintln!("--rtti needs a class name, or * for every class");
                    std::process::exit(2);
                });
                // `*` enumerates everything, for feeding a name table into a
                // decompiler project.
                let class = if class == "*" { String::new() } else { class };
                let path = args
                    .next()
                    .unwrap_or_else(|| "bin/logic.orig.dll".to_string());
                match defiance_core::pe::map(std::path::Path::new(&path)) {
                    Ok(mapped) => {
                        let found =
                            defiance_core::rtti::find(&mapped.image, mapped.base, &class, &|rva| {
                                mapped.is_code(rva)
                            });
                        if found.is_empty() {
                            println!("no class matching {class} in {path}");
                        }
                        for vtable in found {
                            println!("{}  descriptor {:#x}", vtable.mangled, vtable.descriptor);
                            println!(
                                "  locator {:#x}  methods at {:#x}  {} method(s)",
                                vtable.locator,
                                vtable.methods_at,
                                vtable.methods.len()
                            );
                            for (slot, rva) in vtable.methods.iter().enumerate() {
                                println!("    [{slot}] {rva:#x}");
                            }
                        }
                    }
                    Err(message) => {
                        eprintln!("{message}");
                        std::process::exit(1);
                    }
                }
                return;
            }
            "--self-test" => {
                let patch = logic_patch();
                println!("self test against a stand-in image");
                let game = game_patch();
                let result = defiance_core::apply::self_test(&patch, PAYLOAD)
                    .and_then(|()| defiance_core::apply::self_test_game(&game, GAME_PAYLOAD))
                    .and_then(|()| relocate::self_test(&patch, &game, PAYLOAD, GAME_PAYLOAD));
                match result {
                    Ok(()) => println!("the write path works"),
                    Err(message) => {
                        eprintln!("self test failed: {message}");
                        std::process::exit(1);
                    }
                }
                return;
            }
            "--sha" => {
                let path = args.next().unwrap_or_else(|| {
                    eprintln!("--sha needs a file");
                    std::process::exit(2);
                });
                match sha256::file(std::path::Path::new(&path)) {
                    Ok(sha) => println!("{sha}  {path}"),
                    Err(error) => {
                        eprintln!("{path}: {error}");
                        std::process::exit(1);
                    }
                }
                return;
            }
            "-h" | "--help" => {
                println!(
                    "defiance-pickup-inject [--wait SECONDS] [--sha FILE] [--scan] \
                     [--game|--no-game] [--probe] [--select-probe]\n\n\
                     Settings live in defiance-pickup-inject.ini beside this program,\n\
                     written with its defaults on the first run; the options below\n\
                     override it for one run.\n\n\
                     --scan patches a build this was not written for, by finding each\n\
                     patch site from its signature, and falls back to that for any\n\
                     build the patch as built does not fit; it refuses if a site is\n\
                     missing, ambiguous, or has moved apart from the rest of its function.\n\
                     --force-scan does that even for the build this was written for,\n\
                     to try the signature path in game.\n\
                     --scan-check DIR only reports whether that would work for the\n\
                     logic.dll and game.dll in DIR, and writes nothing.\n\
                     --rtti CLASS [DLL] prints a class's RTTI vtable and methods from a\n\
                     DLL on disk (default bin/logic.orig.dll), to cross-check the\n\
                     loader's runtime walk against tools/rtti.py. CLASS of * lists every\n\
                     class, for building a name table for a decompiler project.\n\n\
                     The game.dll hooks are installed by default: a plain click selects\n\
                     the squad, Shift toggles the squad, Ctrl selects the one soldier and\n\
                     Ctrl+Shift toggles him. --no-game leaves them out, and --probe reads\n\
                     back what they recorded.\n\n\
                     --select-probe reads back the selection trace, which records who\n\
                     asked the selection manager to select what.\n\n\
                     --slot-address DATA SLOT prints the exact address of a slot's enabled\n\
                     dword and nothing else, for an external debugger to watch. Safe: it\n\
                     installs no handler and touches no page.\n\n\
                     Patches the weapon-pickup chooser in a running game. Start this,\n\
                     then start the game; nothing in the game directory is touched, so\n\
                     the patch is gone as soon as the game exits.\n\n\
                     The patch: in a squad whose members hold the same kind of weapon,\n\
                     clicking a weapon on the ground dispatches each member in turn\n\
                     instead of always the first one."
                );
                return;
            }
            other => {
                eprintln!("unknown argument {other:?}; try --help");
                std::process::exit(2);
            }
        }
    }

    let settings = settings::load();
    let (wait, with_game, scan) = (
        timeout.unwrap_or(settings.wait),
        with_game.unwrap_or(settings.game),
        scan.unwrap_or(settings.builds),
    );
    println!(
        "settings: builds {}, game.dll {}, wait {wait} s{}",
        match scan {
            Scan::Default => "known",
            Scan::Unknown => "scan",
            Scan::Always => "force",
        },
        if with_game { "on" } else { "off" },
        settings
            .path
            .map(|p| format!(" ({}; options given here override it)", p.display()))
            .unwrap_or_default()
    );
    let code = inject(Duration::from_secs(wait), with_game, scan);
    if settings.pause.keep_open() {
        println!();
        println!("Press Enter to close this window.");
        let _ = std::io::stdin().read_line(&mut String::new());
    }
    std::process::exit(code);
}

/// Wait for the game, then patch logic.dll and, unless told not to, game.dll.
/// Returns the exit code, so the window can be held open before exiting.
fn inject(timeout: Duration, with_game: bool, scan: Scan) -> i32 {
    let patch = logic_patch();
    let game_patch = if with_game { Some(game_patch()) } else { None };
    println!(
        "defiance-pickup-inject: {} bytes of chooser, into a {:#x}-byte block \
         allocated near the module{}",
        PAYLOAD.len(),
        patch.block_bytes,
        if with_game {
            format!(
                ", and {} bytes for the game.dll squad panel",
                GAME_PAYLOAD.len()
            )
        } else {
            String::new()
        }
    );

    let target = match wait_for_game(timeout) {
        Ok(target) => target,
        Err(message) => {
            eprintln!("{message}");
            return 1;
        }
    };
    println!(
        "  logic.dll at {:p}, {} bytes, pid {}",
        target.base, target.size, target.process_id
    );
    // The injector has always installed the assembled chooser; the loader
    // leaves that call to the defiance.pickup plugin.
    match install::install_logic(&patch, &target, scan, PAYLOAD, true) {
        Ok(applied) => {
            if let Some(moves) = &applied.moves {
                println!("  logic.dll: {}", moves.report());
            }
            if target.size != patch.image_bytes {
                println!(
                    "  note: the image is {} bytes, the stock one is {}",
                    target.size, patch.image_bytes
                );
            }
            println!("logic.dll: {}", applied.message);
        }
        Err(message) => {
            eprintln!("{message}");
            return 1;
        }
    }

    // game.dll is the client half: the squad panel icons. It is applied after
    // the simulation, and a failure here is reported without undoing that,
    // since the two are independent.
    let Some(game_patch) = game_patch else {
        println!("  game.dll half skipped (game = false)");
        return 0;
    };
    let game = match find_module(target.process_id, "game.dll") {
        Some(found) => found,
        None => {
            eprintln!("game.dll is not loaded; the squad panel icons are unpatched");
            return 1;
        }
    };
    println!("  game.dll at {:p}, {} bytes", game.base, game.size);
    match install::install_game(&game_patch, &game, scan, GAME_PAYLOAD) {
        Ok(applied) => {
            if let Some(moves) = &applied.moves {
                println!("  game.dll: {}", moves.report());
            }
            if game.size != game_patch.image_bytes {
                println!(
                    "  note: the image is {} bytes, the stock one is {}",
                    game.size, game_patch.image_bytes
                );
            }
            println!("game.dll:  {}", applied.message);
        }
        Err(message) => {
            eprintln!("{message}");
            return 1;
        }
    }
    0
}
