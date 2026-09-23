//! Writing a patch into a module, and the byte-level self test for it.
//!
//! This is the injector's `apply`/`apply_game`, unchanged except that the
//! payload block is a parameter rather than a compile-time include, so the same
//! code serves the injector (which carries `out/payload.bin`) and the loader's
//! core plugin (which carries it too, but resolves the sites in-process).
//!
//! The memory is addressed the same way in both cases: `Process::open` on the
//! target's own pid is what the injector's self test has always done. The
//! loader calls the same functions with the game's pid, which is its own.

use crate::descriptor::{GamePatch, Patch};
use crate::sha256;
use core::ffi::c_void;
use std::path::PathBuf;

// --- the Win32 surface, declared rather than pulled in ----------------------
type Handle = *mut c_void;
const PROCESS_VM_OPERATION: u32 = 0x8;
const PROCESS_VM_READ: u32 = 0x10;
const PROCESS_VM_WRITE: u32 = 0x20;
const PROCESS_QUERY_INFORMATION: u32 = 0x400;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const MEM_COMMIT_RESERVE: u32 = 0x3000;

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentProcessId() -> u32;
    fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
    fn VirtualAlloc(address: *mut u8, size: usize, kind: u32, protect: u32) -> *mut u8;
    fn VirtualProtect(address: *mut u8, size: usize, protect: u32, previous: *mut u32) -> i32;
    fn ReadProcessMemory(
        process: Handle,
        address: *const u8,
        buffer: *mut u8,
        size: usize,
        read: *mut usize,
    ) -> i32;
    fn WriteProcessMemory(
        process: Handle,
        address: *mut u8,
        buffer: *const u8,
        size: usize,
        written: *mut usize,
    ) -> i32;
    fn VirtualProtectEx(
        process: Handle,
        address: *mut u8,
        size: usize,
        protect: u32,
        previous: *mut u32,
    ) -> i32;
    fn VirtualAllocEx(
        process: Handle,
        address: *mut u8,
        size: usize,
        allocation_type: u32,
        protect: u32,
    ) -> *mut u8;
    fn FlushInstructionCache(process: Handle, address: *const u8, size: usize) -> i32;
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn GetLastError() -> u32;
}

/// A Win32 export the injected code calls, resolved by name. The process that
/// resolves it may not have user32 loaded, so it loads it; system DLLs sit at
/// the same base in every process, so the address resolved here is the one the
/// target can call.
pub(crate) fn resolve_export(dll: &str, name: &str) -> Result<*mut c_void, String> {
    let mut wide: Vec<u16> = dll.encode_utf16().collect();
    wide.push(0);
    let module = unsafe { LoadLibraryW(wide.as_ptr()) };
    if module.is_null() {
        return Err(format!("{dll} could not be loaded"));
    }
    let mut symbol: Vec<u8> = name.bytes().collect();
    symbol.push(0);
    let address = unsafe { GetProcAddress(module, symbol.as_ptr()) };
    if address.is_null() {
        return Err(format!("{dll} has no export {name}"));
    }
    Ok(address)
}

/// What a patch is applied to: a module's loaded image in a process.
pub struct Target {
    pub process_id: u32,
    pub base: *mut u8,
    pub size: usize,
    pub path: PathBuf,
}

// --- reading and writing ----------------------------------------------------

/// A process's memory, opened for the work in this module. Public so the
/// injector's diagnostic probes can read without a second implementation.
///
/// When the pid is this process's own — the loader's plugin, and the injector's
/// self test — reads and writes go through the local calls rather than the
/// `*Ex` ones. Those remote calls are what a heuristic reads as injection, and
/// there is no reason to reach across a process boundary into yourself.
pub struct Process {
    handle: Handle,
    local: bool,
}

impl Process {
    /// Open a process for reading and writing. This process's own pid selects
    /// the local path; a game that is already running is the remote one.
    pub fn open(process_id: u32) -> Result<Self, String> {
        if process_id == unsafe { GetCurrentProcessId() } {
            return Ok(Self {
                handle: unsafe { GetCurrentProcess() },
                local: true,
            });
        }
        let handle = unsafe {
            OpenProcess(
                PROCESS_VM_OPERATION
                    | PROCESS_VM_READ
                    | PROCESS_VM_WRITE
                    | PROCESS_QUERY_INFORMATION,
                0,
                process_id,
            )
        };
        if handle.is_null() {
            let code = unsafe { GetLastError() };
            return Err(match code {
                5 => "access denied opening the game process; run this as the same user \
                      as the game, or as administrator"
                    .to_string(),
                _ => format!("OpenProcess failed with error {code}"),
            });
        }
        Ok(Self {
            handle,
            local: false,
        })
    }

    pub fn read(&self, address: *const u8, len: usize) -> Result<Vec<u8>, String> {
        if self.local {
            return Ok(unsafe { core::slice::from_raw_parts(address, len) }.to_vec());
        }
        let mut buffer = vec![0u8; len];
        let mut read = 0usize;
        let ok =
            unsafe { ReadProcessMemory(self.handle, address, buffer.as_mut_ptr(), len, &mut read) };
        if ok == 0 || read != len {
            return Err(format!(
                "reading {len} bytes at {address:p} failed with error {}",
                unsafe { GetLastError() }
            ));
        }
        Ok(buffer)
    }

    /// Write code or data, making the page writable only for the copy and
    /// putting the previous protection back: a module's `.text` returns to
    /// execute-read afterward rather than being left writable and executable.
    /// The allocated block carries writable state (the cursor and the ring), so
    /// its protection is read back unchanged.
    pub(crate) fn write(&self, address: *mut u8, data: &[u8]) -> Result<(), String> {
        let mut previous = 0u32;
        if self.local {
            let ok = unsafe {
                VirtualProtect(address, data.len(), PAGE_EXECUTE_READWRITE, &mut previous)
            };
            if ok == 0 {
                return Err(format!("VirtualProtect failed with error {}", unsafe {
                    GetLastError()
                }));
            }
            unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), address, data.len()) };
            unsafe { FlushInstructionCache(GetCurrentProcess(), address, data.len()) };
            if unsafe { VirtualProtect(address, data.len(), previous, &mut previous) } == 0 {
                return Err(format!(
                    "restoring protection failed with error {}",
                    unsafe { GetLastError() }
                ));
            }
            return Ok(());
        }
        let ok = unsafe {
            VirtualProtectEx(
                self.handle,
                address,
                data.len(),
                PAGE_EXECUTE_READWRITE,
                &mut previous,
            )
        };
        if ok == 0 {
            return Err(format!("VirtualProtectEx failed with error {}", unsafe {
                GetLastError()
            }));
        }
        let mut written = 0usize;
        let ok = unsafe {
            WriteProcessMemory(
                self.handle,
                address,
                data.as_ptr(),
                data.len(),
                &mut written,
            )
        };
        if ok == 0 || written != data.len() {
            return Err(format!("WriteProcessMemory failed with error {}", unsafe {
                GetLastError()
            }));
        }
        unsafe { FlushInstructionCache(self.handle, address, data.len()) };
        unsafe { VirtualProtectEx(self.handle, address, data.len(), previous, &mut previous) };
        Ok(())
    }

    /// Commits `size` bytes of executable, writable memory as close to `hint`
    /// as the address space allows, so that a rel32 from the call site can
    /// still reach it. Walks outward in allocation-granularity steps and stays
    /// inside 2GB either way.
    pub(crate) fn reserve_near(&self, hint: *mut u8, size: usize) -> Result<*mut u8, String> {
        const GRANULARITY: usize = 0x10000;
        const REACH: usize = 0x8000; // 0x8000 * 64K is 2GB
        let base = (hint as usize) & !(GRANULARITY - 1);
        for step in 1..REACH {
            for candidate in [
                base.wrapping_add(step * GRANULARITY),
                base.wrapping_sub(step * GRANULARITY),
            ] {
                if candidate == 0 {
                    continue;
                }
                let got = if self.local {
                    unsafe {
                        VirtualAlloc(
                            candidate as *mut u8,
                            size,
                            MEM_COMMIT_RESERVE,
                            PAGE_EXECUTE_READWRITE,
                        )
                    }
                } else {
                    unsafe {
                        VirtualAllocEx(
                            self.handle,
                            candidate as *mut u8,
                            size,
                            MEM_COMMIT_RESERVE,
                            PAGE_EXECUTE_READWRITE,
                        )
                    }
                };
                if !got.is_null() {
                    return Ok(got);
                }
            }
        }
        Err("no free page within reach of the call site".to_string())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // GetCurrentProcess is a pseudo-handle; closing it does nothing good.
        if !self.local {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

// --- the work ---------------------------------------------------------------

/// The target module's loaded image, for the signature scan.
pub fn module_image(target: &Target) -> Result<Vec<u8>, String> {
    Process::open(target.process_id)?.read(target.base, target.size)
}

/// Record location for the external injector's relocated-build probes.
fn block_record_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|dir| dir.join("defiance-pickup-inject.block"))
}

/// Apply the logic.dll patch, and return "patched" or "already patched".
/// `chooser` controls whether this host also redirects the pickup call.
pub fn apply(
    patch: &Patch,
    target: &Target,
    payload: &[u8],
    chooser: bool,
) -> Result<&'static str, String> {
    let process = Process::open(target.process_id)?;
    let at = |rva: usize| unsafe { target.base.add(rva) };

    // A byte edit that must already read `after`, or read `before` and is then
    // written. Reports whether it had anything to do.
    let patch_bytes = |rva: usize, before: &[u8], after: &[u8]| -> Result<bool, String> {
        let now = process.read(at(rva), before.len())?;
        if now == after {
            return Ok(false);
        }
        if now != before {
            return Err(format!(
                "rva {rva:#x} reads {} but this patch expects {}; \
                 this is not the build it was written for",
                sha256::hex(&now),
                sha256::hex(before)
            ));
        }
        process.write(at(rva), after)?;
        if process.read(at(rva), after.len())? != after {
            return Err(format!("the write at rva {rva:#x} did not take"));
        }
        Ok(true)
    };

    // Is the chooser already patched, and is it the build the patch was made
    // for? The individual-selection branches are checked separately, so a run
    // that only missed those still applies them.
    // The block is allocated at run time, so the patched call cannot be a
    // known byte string. Decode the target instead: anything other than the
    // stock chooser means the chooser has already been redirected.
    let call = process.read(at(patch.call_site), patch.call_before.len())?;
    let chooser_done = call[0] == 0xe8 && call != patch.call_before && {
        let rel = i32::from_le_bytes([call[1], call[2], call[3], call[4]]) as isize;
        (patch.call_site as isize + 5 + rel) as usize != patch.stock_chooser
    };
    if !chooser_done && call != patch.call_before {
        return Err(format!(
            "the call at rva {:#x} reads {} but this patch expects {}; \
             this is not the build it was written for",
            patch.call_site,
            sha256::hex(&call),
            sha256::hex(&patch.call_before)
        ));
    }
    // Whether the block is already installed. When this host owns the chooser,
    // its redirected call is the marker; when a plugin does, the call is stock
    // (or the plugin's), so the move filter's call is the marker instead.
    let block_done = if chooser {
        chooser_done
    } else {
        let now = process.read(at(patch.move_call_site), patch.move_displaced.len())?;
        now != patch.move_displaced
    };
    let anchor = process.read(at(patch.anchor_rva), patch.anchor.len())?;
    if anchor != patch.anchor {
        return Err(format!(
            "the stock chooser at rva {:#x} does not match this patch's build",
            patch.anchor_rva
        ));
    }
    // Validate every selection span before changing any code. In particular,
    // an older getter bypass must not leave a newly written chooser or setter
    // behind when this version refuses it.
    for (rva, before, after) in [
        (
            patch.select_is_rva,
            &patch.select_is_before,
            &patch.select_is_after,
        ),
        (
            patch.select_squad_rva,
            &patch.select_squad_before,
            &patch.select_squad_after,
        ),
        (
            patch.select_type_rva,
            &patch.select_type_before,
            &patch.select_type_after,
        ),
        (
            patch.select_toggle_rva,
            &patch.select_toggle_before,
            &patch.select_toggle_after,
        ),
    ] {
        let now = process.read(at(rva), before.len())?;
        if now != *before && now != *after {
            return Err(format!(
                "selection at rva {rva:#x} is incompatible; restart with the stock DLL before injecting"
            ));
        }
    }
    if target.path.exists() {
        let sha =
            sha256::file(&target.path).map_err(|e| format!("hashing {:?}: {e}", target.path))?;
        if sha != patch.source_sha256 {
            return Err(format!(
                "{} is sha256 {sha}\n  the patch was built from {} ({} bytes)\n\
                 The game has been updated; the addresses have to be re-derived,\n\
                 or found by signature with --scan.",
                target.path.display(),
                patch.source_sha256,
                patch.module_bytes
            ));
        }
    }

    if !block_done {
        if payload.len() > patch.cursor_offset || patch.cursor_offset + 4 > patch.block_bytes {
            return Err("the code and the cursor do not both fit the block".to_string());
        }

        // Every detour displaces whole instructions. Check them all before a
        // page is even allocated, so a build that carries some and not others
        // is refused without the block or the chooser being written.
        let mut sites = vec![(patch.move_call_site, &patch.move_displaced, "move filter")];
        for hook in &patch.detours {
            sites.push((hook.rva, &hook.displaced, "detour"));
        }
        for call in &patch.pose_calls {
            sites.push((call.rva, &call.before, "pose split"));
        }
        for (site, displaced, what) in sites {
            let now = process.read(at(site), displaced.len())?;
            if now != *displaced {
                return Err(format!(
                    "rva {site:#x} reads {} but the {what} expects {}; \
                     this is not the build it was written for",
                    sha256::hex(&now),
                    sha256::hex(displaced)
                ));
            }
        }

        // A page of its own, rather than the 417 bytes of alignment padding
        // the first version squeezed into. It is requested near the module so
        // that the one call can still reach it with a rel32.
        let block = process.reserve_near(at(0), patch.block_bytes)?;

        // VirtualAllocEx hands back zeroed pages, so the cursor and the trace
        // ring start at zero. Each trace stub jumps back into the module, which
        // an allocated block cannot reach with a baked rel32, so those imm64s
        // are resolved here.
        let mut payload = payload.to_vec();
        for fix in &patch.trace_fixups {
            let resume = (at(fix.target_rva) as u64).to_le_bytes();
            payload[fix.offset..fix.offset + 8].copy_from_slice(&resume);
        }
        // and each branch into the module gets the rel32 it needs from here
        for fix in &patch.rel_fixups {
            let delta = at(fix.target_rva) as isize - (block as isize + fix.offset as isize + 4);
            if delta < i32::MIN as isize || delta > i32::MAX as isize {
                return Err(format!(
                    "the block cannot reach rva {:#x} with a rel32",
                    fix.target_rva
                ));
            }
            payload[fix.offset..fix.offset + 4].copy_from_slice(&(delta as i32).to_le_bytes());
        }
        // and each distance the pose split keeps between two rvas is this
        // build's, which differs from the assembled one only once relocated
        for fix in &patch.delta_fixups {
            let delta = fix.to as isize - fix.from as isize;
            if delta < i32::MIN as isize || delta > i32::MAX as isize {
                return Err(format!(
                    "rva {:#x} is out of reach of rva {:#x}",
                    fix.to, fix.from
                ));
            }
            payload[fix.offset..fix.offset + 4].copy_from_slice(&(delta as i32).to_le_bytes());
        }
        process.write(block, &payload)?;
        if process.read(block, payload.len())? != payload {
            return Err("the chooser did not take the write".to_string());
        }

        // The chooser and the move filter are direct calls whose rel32 is
        // worked out from where the block actually landed.
        let retarget = |site: usize, entry: usize| -> Result<(), String> {
            let from = at(site + 5) as isize;
            let delta = unsafe { block.add(entry) } as isize - from;
            if delta < i32::MIN as isize || delta > i32::MAX as isize {
                return Err(format!("rva {site:#x} cannot reach the block with a rel32"));
            }
            let mut bytes = vec![0xe8u8];
            bytes.extend_from_slice(&(delta as i32).to_le_bytes());
            process.write(at(site), &bytes)?;
            if process.read(at(site), bytes.len())? != bytes {
                return Err(format!("the write at rva {site:#x} did not take"));
            }
            Ok(())
        };
        if chooser {
            retarget(patch.call_site, 0)?;
        }
        retarget(patch.move_call_site, patch.move_offset)?;
        for call in &patch.pose_calls {
            retarget(call.rva, call.entry)?;
            if !call.tail.is_empty() {
                process.write(at(call.rva + 5), &call.tail)?;
                if process.read(at(call.rva + 5), call.tail.len())? != call.tail {
                    return Err(format!(
                        "the pose tail at rva {:#x} did not take",
                        call.rva + 5
                    ));
                }
            }
        }

        // The detours are jmps rather than calls. Each trace stub performs its
        // displaced instructions itself and resumes just past them, so the
        // caller's return address is still on the stack for it to record; the
        // setter replaces its function outright and returns to that caller. A
        // five-byte jump, and nops for the rest of the displaced run.
        for hook in &patch.detours {
            let from = at(hook.rva + 5) as isize;
            let delta = unsafe { block.add(hook.entry) } as isize - from;
            if delta < i32::MIN as isize || delta > i32::MAX as isize {
                return Err(format!(
                    "rva {:#x} cannot reach the block with a rel32",
                    hook.rva
                ));
            }
            let mut detour = vec![0xe9u8];
            detour.extend_from_slice(&(delta as i32).to_le_bytes());
            detour.resize(hook.displaced.len(), 0x90);
            process.write(at(hook.rva), &detour)?;
            if process.read(at(hook.rva), detour.len())? != detour {
                return Err(format!("the detour at rva {:#x} did not take", hook.rva));
            }
        }

        crate::report::note(format!(
            "  chooser at {block:p}, move filter at +{:#x}, setter at +{:#x}, \
             pose split at +{:#x} ({} calls), {} detours, ring at +{:#x}, cursor at +{:#x}",
            patch.move_offset,
            patch.setter_offset,
            patch.pose_offset,
            patch.pose_calls.len(),
            patch.detours.len(),
            patch.trace_offset,
            patch.cursor_offset
        ));
        // Remember the block for --select-probe, which cannot look the hook up
        // again once the patched bytes have replaced the signature. A stand-in
        // self-test target has no file and is not the game.
        if !target.path.as_os_str().is_empty() {
            if let (Some(path), Some(hook)) = (block_record_path(), patch.detours.first()) {
                let record = format!(
                    "pid={:#x}\nbase={:#x}\nblock={:#x}\ntrace={:#x}\nentry={:#x}\n",
                    target.process_id, target.base as usize, block as usize, hook.rva, hook.entry
                );
                let _ = std::fs::write(path, record);
            }
        }
    }

    // Selection: a getter that requires both the parent squad and the
    // soldier's own mark, and the manager's squad, toggle and type paths.
    // Existing unwind behavior is unchanged. The getter span includes its
    // parent guard so an older bypass patch is refused, not silently retained.
    let is_written = patch_bytes(
        patch.select_is_rva,
        &patch.select_is_before,
        &patch.select_is_after,
    )?;
    let squad_written = patch_bytes(
        patch.select_squad_rva,
        &patch.select_squad_before,
        &patch.select_squad_after,
    )?;

    let toggle_written = patch_bytes(
        patch.select_toggle_rva,
        &patch.select_toggle_before,
        &patch.select_toggle_after,
    )?;

    let type_written = patch_bytes(
        patch.select_type_rva,
        &patch.select_type_before,
        &patch.select_type_after,
    )?;

    if block_done && !is_written && !squad_written && !toggle_written && !type_written {
        return Ok("already patched; nothing to do");
    }
    Ok("patched")
}

/// The game.dll half: one allocated block carrying the squad expansion, three
/// imm64 slots in it resolved against the module, and two hooks jumping into
/// it. Nothing here is written until every check has passed.
pub fn apply_game(
    patch: &GamePatch,
    target: &Target,
    payload: &[u8],
) -> Result<&'static str, String> {
    let process = Process::open(target.process_id)?;
    let at = |rva: usize| unsafe { target.base.add(rva) };

    let mut already = 0;
    for hook in &patch.hooks {
        let now = process.read(at(hook.rva), hook.displaced.len())?;
        if now[0] == 0xe9 {
            already += 1;
            continue;
        }
        if now != hook.displaced {
            return Err(format!(
                "game.dll rva {:#x} reads {} but this patch expects {}; \
                 this is not the build it was written for",
                hook.rva,
                sha256::hex(&now),
                sha256::hex(&hook.displaced)
            ));
        }
    }
    if already == patch.hooks.len() {
        return Ok("already patched; nothing to do");
    }
    if already != 0 {
        return Err("game.dll has some hooks written and not others; restart the game".to_string());
    }

    let anchor = process.read(at(patch.anchor_rva), patch.anchor.len())?;
    if anchor != patch.anchor {
        return Err(format!(
            "game.dll rva {:#x} does not match this patch's build",
            patch.anchor_rva
        ));
    }
    if target.path.exists() {
        let sha =
            sha256::file(&target.path).map_err(|e| format!("hashing {:?}: {e}", target.path))?;
        if sha != patch.source_sha256 {
            return Err(format!(
                "{} is sha256 {sha}\n  the patch was built from {}\n\
                 The game has been updated; the addresses have to be re-derived,\n\
                 or found by signature with --scan.",
                target.path.display(),
                patch.source_sha256
            ));
        }
    }

    let block = process.reserve_near(at(0), patch.block_bytes)?;

    // the three references back into the module, which an allocated block
    // cannot reach with a baked rel32
    let mut code = payload.to_vec();
    for fix in &patch.fixups {
        if fix.offset + 8 > code.len() {
            return Err(format!("fixup at +{:#x} is past the payload", fix.offset));
        }
        let value = at(fix.target_rva) as u64;
        code[fix.offset..fix.offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    for export in &patch.exports {
        if export.offset + 8 > code.len() {
            return Err(format!(
                "export at +{:#x} is past the payload",
                export.offset
            ));
        }
        let address = resolve_export(&export.dll, &export.name)? as u64;
        code[export.offset..export.offset + 8].copy_from_slice(&address.to_le_bytes());
    }
    process.write(block, &code)?;
    if process.read(block, code.len())? != code {
        return Err("the squad expansion did not take the write".to_string());
    }

    for hook in &patch.hooks {
        let from = at(hook.rva + 5) as isize;
        let delta = unsafe { block.add(hook.entry) } as isize - from;
        if delta < i32::MIN as isize || delta > i32::MAX as isize {
            return Err(format!(
                "game.dll rva {:#x} cannot reach the block with a rel32",
                hook.rva
            ));
        }
        // a five-byte jump, then nops out to the end of what was displaced
        let mut bytes = vec![0xe9u8];
        bytes.extend_from_slice(&(delta as i32).to_le_bytes());
        bytes.resize(hook.displaced.len(), 0x90);
        process.write(at(hook.rva), &bytes)?;
        if process.read(at(hook.rva), bytes.len())? != bytes {
            return Err(format!(
                "the hook at game.dll rva {:#x} did not take",
                hook.rva
            ));
        }
    }

    crate::report::note(format!(
        "  squad expansion at {block:p}, {} bytes, {} hooks",
        code.len(),
        patch.hooks.len()
    ));
    Ok("patched")
}

/// Exercises the whole write path without the game: a stand-in module is
/// allocated in this process with the stock bytes planted where the patch
/// expects them, and `apply` runs against it. `path` is left empty so the file
/// hash step is skipped, since there is no file behind a stand-in.
pub fn self_test(patch: &Patch, payload: &[u8]) -> Result<(), String> {
    let span = patch.image_bytes;
    let mut image = vec![0u8; span];
    image[patch.call_site..patch.call_site + patch.call_before.len()]
        .copy_from_slice(&patch.call_before);
    image[patch.anchor_rva..patch.anchor_rva + patch.anchor.len()].copy_from_slice(&patch.anchor);
    image[patch.select_is_rva..patch.select_is_rva + patch.select_is_before.len()]
        .copy_from_slice(&patch.select_is_before);
    image[patch.select_squad_rva..patch.select_squad_rva + patch.select_squad_before.len()]
        .copy_from_slice(&patch.select_squad_before);
    image[patch.select_type_rva..patch.select_type_rva + patch.select_type_before.len()]
        .copy_from_slice(&patch.select_type_before);
    image[patch.select_toggle_rva..patch.select_toggle_rva + patch.select_toggle_before.len()]
        .copy_from_slice(&patch.select_toggle_before);
    image[patch.move_call_site..patch.move_call_site + patch.move_displaced.len()]
        .copy_from_slice(&patch.move_displaced);
    for call in &patch.pose_calls {
        image[call.rva..call.rva + call.before.len()].copy_from_slice(&call.before);
    }
    for hook in &patch.detours {
        image[hook.rva..hook.rva + hook.displaced.len()].copy_from_slice(&hook.displaced);
    }
    let target = Target {
        process_id: std::process::id(),
        base: image.as_mut_ptr(),
        size: span,
        path: PathBuf::new(),
    };

    let outcome = apply(patch, &target, payload, true)?;
    if outcome != "patched" {
        return Err(format!("expected a patch, got {outcome:?}"));
    }
    println!("  first run:  {outcome}");

    // The block is allocated, so where it landed is only knowable by decoding
    // the call that now points at it. That checks the rel32 arithmetic too.
    let call = &image[patch.call_site..patch.call_site + 5];
    if call[0] != 0xe8 {
        return Err("the call site is no longer a direct call".to_string());
    }
    let rel = i32::from_le_bytes([call[1], call[2], call[3], call[4]]) as isize;
    let block = unsafe { image.as_ptr().add(patch.call_site + 5).offset(rel) } as *mut u8;
    // The block is the payload with every trace resume imm64 resolved, so
    // compare against that rather than the raw payload.
    let mut expected = payload.to_vec();
    for fix in &patch.trace_fixups {
        let resume = unsafe { image.as_ptr().add(fix.target_rva) } as u64;
        expected[fix.offset..fix.offset + 8].copy_from_slice(&resume.to_le_bytes());
    }
    for fix in &patch.rel_fixups {
        let target = unsafe { image.as_ptr().add(fix.target_rva) } as isize;
        let delta = (target - (block as isize + fix.offset as isize + 4)) as i32;
        expected[fix.offset..fix.offset + 4].copy_from_slice(&delta.to_le_bytes());
    }
    // the pose split's distances, as assembled: rewriting them changes nothing
    for fix in &patch.delta_fixups {
        let delta = (fix.to as isize - fix.from as isize) as i32;
        if expected[fix.offset..fix.offset + 4] != delta.to_le_bytes() {
            return Err(format!(
                "the payload at +{:#x} does not hold {delta:#x}",
                fix.offset
            ));
        }
    }
    let landed = unsafe { std::slice::from_raw_parts(block, expected.len()) };
    if landed != expected.as_slice() {
        return Err(format!(
            "the chooser is not at {block:p}, where the call points"
        ));
    }
    // and the move filter's detour must point at its own offset in that block
    let detour = &image[patch.move_call_site..patch.move_call_site + 5];
    if detour[0] != 0xe8 {
        return Err("the move filter's detour was not written".to_string());
    }
    let move_rel = i32::from_le_bytes([detour[1], detour[2], detour[3], detour[4]]) as isize;
    let move_entry = unsafe {
        image
            .as_ptr()
            .add(patch.move_call_site + 5)
            .offset(move_rel)
    } as *const u8;
    if move_entry != unsafe { block.add(patch.move_offset) } {
        return Err(format!(
            "the detour points at {move_entry:p}, not the filter at +{:#x}",
            patch.move_offset
        ));
    }
    // and every detour must be a jump to its own stub, tail nop'd
    for hook in &patch.detours {
        let site = &image[hook.rva..hook.rva + hook.displaced.len()];
        if site[0] != 0xe9 {
            return Err(format!("the detour at {:#x} was not written", hook.rva));
        }
        let rel = i32::from_le_bytes([site[1], site[2], site[3], site[4]]) as isize;
        let entry = unsafe { image.as_ptr().add(hook.rva + 5).offset(rel) } as *const u8;
        if entry != unsafe { block.add(hook.entry) } {
            return Err(format!(
                "the detour at {:#x} points at {entry:p}, not its stub at +{:#x}",
                hook.rva, hook.entry
            ));
        }
        if site[5..].iter().any(|&b| b != 0x90) {
            return Err(format!("the detour at {:#x} left its tail dirty", hook.rva));
        }
    }
    // and each pose call must now call its own entry in the block
    for call in &patch.pose_calls {
        let site = &image[call.rva..call.rva + 5];
        if site[0] != 0xe8 {
            return Err(format!(
                "the pose call at {:#x} is no longer a call",
                call.rva
            ));
        }
        let rel = i32::from_le_bytes([site[1], site[2], site[3], site[4]]) as isize;
        let entry = unsafe { image.as_ptr().add(call.rva + 5).offset(rel) } as *const u8;
        if entry != unsafe { block.add(call.entry) } {
            return Err(format!(
                "the pose call at {:#x} points at {entry:p}, not its entry at +{:#x}",
                call.rva, call.entry
            ));
        }
        if image[call.rva + 5..call.rva + 5 + call.tail.len()] != call.tail[..] {
            return Err(format!(
                "the pose call at {:#x} is missing its tail",
                call.rva
            ));
        }
    }

    let cursor = unsafe { block.add(patch.cursor_offset) as *mut u32 };
    if unsafe { std::ptr::read(cursor) } != 0 {
        return Err("the cursor did not start at zero".to_string());
    }
    // the cursor is written through, which is what the game needs at runtime
    unsafe { std::ptr::write(cursor, 7) };
    if unsafe { std::ptr::read(cursor) } != 7 {
        return Err("the cursor is not writable".to_string());
    }
    if image[patch.select_squad_rva..patch.select_squad_rva + patch.select_squad_after.len()]
        != patch.select_squad_after[..]
    {
        return Err("squad member selection loop was not installed".to_string());
    }
    if image[patch.select_is_rva..patch.select_is_rva + patch.select_is_after.len()]
        != patch.select_is_after[..]
    {
        return Err("the isSelected branch was not flipped".to_string());
    }

    if image[patch.select_toggle_rva..patch.select_toggle_rva + patch.select_toggle_after.len()]
        != patch.select_toggle_after[..]
    {
        return Err("toggle selection rewrite was not installed".to_string());
    }

    if image[patch.select_type_rva..patch.select_type_rva + patch.select_type_after.len()]
        != patch.select_type_after[..]
    {
        return Err("type selection rewrite was not installed".to_string());
    }

    let again = apply(patch, &target, payload, true)?;
    if !again.starts_with("already") {
        return Err(format!("a second run should be a no-op, got {again:?}"));
    }
    println!("  second run: {again}");

    // Refuse the previous getter bypass before doing any other writes, both
    // with a stock chooser and with an already-injected chooser.
    for chooser_installed in [false, true] {
        let mut legacy = image.clone();
        legacy[patch.select_is_rva..patch.select_is_rva + patch.select_is_before.len()]
            .copy_from_slice(&patch.select_is_before);
        legacy[patch.select_is_rva] = 0xeb;
        if !chooser_installed {
            legacy[patch.call_site..patch.call_site + patch.call_before.len()]
                .copy_from_slice(&patch.call_before);
            legacy[patch.move_call_site..patch.move_call_site + patch.move_displaced.len()]
                .copy_from_slice(&patch.move_displaced);
            for hook in &patch.detours {
                legacy[hook.rva..hook.rva + hook.displaced.len()].copy_from_slice(&hook.displaced);
            }
            for call in &patch.pose_calls {
                legacy[call.rva..call.rva + call.before.len()].copy_from_slice(&call.before);
            }
        }
        let before = legacy.clone();
        let old_target = Target {
            process_id: std::process::id(),
            base: legacy.as_mut_ptr(),
            size: span,
            path: PathBuf::new(),
        };
        match apply(patch, &old_target, payload, true) {
            Err(message) if message.contains("incompatible") => {}
            other => return Err(format!("legacy selection was not refused: {other:?}")),
        }
        if legacy != before {
            return Err("legacy rejection changed the image".to_string());
        }
    }
    println!("  legacy selection is refused without writes");

    // and a build it was not written for must be refused
    let mut wrong = vec![0u8; span];
    wrong[patch.call_site] = 0xe8;
    let stray = Target {
        process_id: std::process::id(),
        base: wrong.as_mut_ptr(),
        size: span,
        path: PathBuf::new(),
    };
    // Which guard catches it is not the point, only that one does and that
    // nothing was written. The zeroed stand-in trips the stock-chooser anchor.
    let untouched = wrong.clone();
    match apply(patch, &stray, payload, true) {
        Err(message) => println!("  a foreign build is refused: {message}"),
        Ok(outcome) => return Err(format!("a foreign build was not refused: {outcome:?}")),
    }
    if wrong != untouched {
        return Err("a foreign build was refused, but only after writing".to_string());
    }
    Ok(())
}

/// The game.dll half of the self test: a stand-in module with the displaced
/// bytes planted, then a check that each hook jumps where it should and that
/// every fixup was resolved against the module.
pub fn self_test_game(patch: &GamePatch, payload: &[u8]) -> Result<(), String> {
    let span = patch.image_bytes;
    let mut image = vec![0u8; span];
    for hook in &patch.hooks {
        image[hook.rva..hook.rva + hook.displaced.len()].copy_from_slice(&hook.displaced);
    }
    image[patch.anchor_rva..patch.anchor_rva + patch.anchor.len()].copy_from_slice(&patch.anchor);
    let target = Target {
        process_id: std::process::id(),
        base: image.as_mut_ptr(),
        size: span,
        path: PathBuf::new(),
    };

    let outcome = apply_game(patch, &target, payload)?;
    if outcome != "patched" {
        return Err(format!("expected a patch, got {outcome:?}"));
    }
    println!("  game.dll first run:  {outcome}");

    let mut block = std::ptr::null::<u8>();
    for hook in &patch.hooks {
        let site = &image[hook.rva..hook.rva + hook.displaced.len()];
        if site[0] != 0xe9 {
            return Err(format!("the hook at {:#x} was not written", hook.rva));
        }
        if site[5..].iter().any(|&b| b != 0x90) {
            return Err(format!(
                "the hook at {:#x} left the displaced tail dirty",
                hook.rva
            ));
        }
        let rel = i32::from_le_bytes([site[1], site[2], site[3], site[4]]) as isize;
        let entry = unsafe { image.as_ptr().add(hook.rva + 5).offset(rel) };
        let start = unsafe { entry.sub(hook.entry) };
        if block.is_null() {
            block = start;
        } else if block != start {
            return Err("the two hooks point into different blocks".to_string());
        }
    }

    let landed = unsafe { std::slice::from_raw_parts(block, payload.len()) };
    for fix in &patch.fixups {
        let got = u64::from_le_bytes(landed[fix.offset..fix.offset + 8].try_into().unwrap());
        let want = unsafe { image.as_ptr().add(fix.target_rva) } as u64;
        if got != want {
            return Err(format!(
                "the fixup at +{:#x} reads {got:#x}, not {want:#x}",
                fix.offset
            ));
        }
    }
    println!(
        "  game.dll hooks agree on one block, and all {} fixups resolved",
        patch.fixups.len()
    );

    let again = apply_game(patch, &target, payload)?;
    if !again.starts_with("already") {
        return Err(format!("a second run should be a no-op, got {again:?}"));
    }
    println!("  game.dll second run: {again}");

    // a build it was not written for must be refused without writing
    let mut wrong = vec![0u8; span];
    for hook in &patch.hooks {
        wrong[hook.rva..hook.rva + hook.displaced.len()].copy_from_slice(&hook.displaced);
    }
    let untouched = wrong.clone();
    let stray = Target {
        process_id: std::process::id(),
        base: wrong.as_mut_ptr(),
        size: span,
        path: PathBuf::new(),
    };
    match apply_game(patch, &stray, payload) {
        Err(message) => println!("  game.dll foreign build refused: {message}"),
        Ok(outcome) => return Err(format!("a foreign build was not refused: {outcome:?}")),
    }
    if wrong != untouched {
        return Err("a foreign build was refused, but only after writing".to_string());
    }
    Ok(())
}
