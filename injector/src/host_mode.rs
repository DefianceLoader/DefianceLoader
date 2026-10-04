//! Start the shared in-process plugin host through a neutral DLL filename.
//! Only a UTF-16 filename is written remotely; Windows loads and initializes
//! the DLL. Readiness comes from the host's exported startup status block.

use super::{ModuleEntry32W, Target, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32};
use defiance_core::apply::Process;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::time::{Duration, Instant};

type Handle = *mut c_void;
const EXPORT: &str = "DEFIANCE_LOADER_STATE";
const STATUS_BYTES: usize = 24;

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
    fn VirtualAllocEx(
        process: Handle,
        address: *mut c_void,
        bytes: usize,
        kind: u32,
        protect: u32,
    ) -> *mut c_void;
    fn VirtualFreeEx(process: Handle, address: *mut c_void, bytes: usize, kind: u32) -> i32;
    fn WriteProcessMemory(
        process: Handle,
        address: *mut c_void,
        source: *const c_void,
        bytes: usize,
        written: *mut usize,
    ) -> i32;
    fn CreateRemoteThread(
        process: Handle,
        attributes: *mut c_void,
        stack: usize,
        start: *const c_void,
        parameter: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn GetExitCodeThread(thread: Handle, code: *mut u32) -> i32;
    fn IsWow64Process(process: Handle, wow64: *mut i32) -> i32;
    fn K32GetModuleFileNameExW(
        process: Handle,
        module: Handle,
        filename: *mut u16,
        size: u32,
    ) -> u32;
    fn CreateMutexW(attributes: *mut c_void, owner: i32, name: *const u16) -> Handle;
    fn ReleaseMutex(mutex: Handle) -> i32;
}

#[link(name = "version")]
extern "system" {
    fn GetFileVersionInfoSizeW(path: *const u16, handle: *mut u32) -> u32;
    fn GetFileVersionInfoW(path: *const u16, handle: u32, bytes: u32, data: *mut c_void) -> i32;
    fn VerQueryValueW(
        data: *const c_void,
        query: *const u16,
        value: *mut *mut c_void,
        bytes: *mut u32,
    ) -> i32;
}

struct Owned(Handle);
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct LaunchLock(Owned);
impl Drop for LaunchLock {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0 .0);
        }
    }
}

fn milliseconds(timeout: Duration) -> u32 {
    timeout.as_millis().min(u32::MAX as u128 - 1) as u32
}

fn lock(pid: u32, timeout: Duration) -> Result<LaunchLock, String> {
    let name: Vec<u16> = format!("Local\\DefianceLoader.Start.{pid}\0")
        .encode_utf16()
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(format!(
            "cannot serialize loader startup: {}",
            std::io::Error::last_os_error()
        ));
    }
    let handle = Owned(handle);
    match unsafe { WaitForSingleObject(handle.0, milliseconds(timeout)) } {
        0 | 0x80 => Ok(LaunchLock(handle)),
        0x102 => Err("another launcher is still starting this game; startup timed out".into()),
        _ => Err(format!(
            "cannot wait for launcher: {}",
            std::io::Error::last_os_error()
        )),
    }
}

fn modules(pid: u32) -> Result<Vec<Target>, String> {
    let handle = unsafe { OpenProcess(0x400 | 0x10, 0, pid) };
    if handle.is_null() {
        return Err(format!(
            "cannot inspect game modules: {}",
            std::io::Error::last_os_error()
        ));
    }
    let process = Owned(handle);
    let mut filename = vec![0u16; 32768];
    for attempt in 0..4 {
        let snapshot = unsafe {
            super::CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid)
        };
        if snapshot == super::INVALID_HANDLE {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(24) && attempt < 3 {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            return Err(format!("cannot list the game's modules: {error}"));
        }
        let snapshot = Owned(snapshot);
        let mut entry: ModuleEntry32W = unsafe { std::mem::zeroed() };
        entry.size = std::mem::size_of::<ModuleEntry32W>() as u32;
        let mut out = Vec::new();
        if unsafe { super::Module32FirstW(snapshot.0, &mut entry) } != 0 {
            loop {
                // Toolhelp's filename fields can lose non-ASCII characters;
                // fetch the full path from the target's Unicode module data.
                let count = unsafe {
                    K32GetModuleFileNameExW(
                        process.0,
                        entry.module_handle,
                        filename.as_mut_ptr(),
                        filename.len() as u32,
                    )
                } as usize;
                let path = if count > 0 && count < filename.len() {
                    super::wide_to_string(&filename[..count]).into()
                } else {
                    super::wide_to_string(&entry.exe_path).into()
                };
                out.push(Target {
                    process_id: pid,
                    base: entry.base_address,
                    size: entry.module_size as usize,
                    path,
                });
                if unsafe { super::Module32NextW(snapshot.0, &mut entry) } == 0 {
                    break;
                }
            }
        }
        return Ok(out);
    }
    unreachable!()
}

fn word(bytes: &[u8], offset: usize) -> Result<usize, String> {
    bytes
        .get(offset..offset + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
        .ok_or_else(|| "truncated PE value".into())
}

/// Read a named data export without loading its DLL into the launcher. The
/// reader works with either mapped file bytes or a running module's memory.
fn export_rva(
    read: &dyn Fn(usize, usize) -> Result<Vec<u8>, String>,
    size: usize,
    name: &str,
) -> Result<Option<usize>, String> {
    let get = |offset: usize, length: usize| -> Result<Vec<u8>, String> {
        if offset.checked_add(length).is_none_or(|end| end > size) {
            return Err("PE export span outside image".into());
        }
        let bytes = read(offset, length)?;
        if bytes.len() != length {
            return Err("truncated PE export read".into());
        }
        Ok(bytes)
    };
    let dos = get(0, 64)?;
    if &dos[..2] != b"MZ" {
        return Err("not a PE image".into());
    }
    let nt = word(&dos, 0x3c)?;
    let header = get(nt, 24 + 120)?;
    if &header[..4] != b"PE\0\0" || &header[24..26] != [0x0b, 0x02] {
        return Err("loader requires a 64-bit PE image".into());
    }
    if word(&header, 24 + 108)? == 0 {
        return Ok(None);
    }
    let dir = word(&header, 24 + 112)?;
    let length = word(&header, 24 + 116)?;
    if dir == 0 || length == 0 {
        return Ok(None);
    }
    let table = get(dir, 40)?;
    let count = word(&table, 24)?;
    if count > size / 4 {
        return Err("invalid PE export count".into());
    }
    let names = get(word(&table, 32)?, count * 4)?;
    let mut low = 0;
    let mut high = count;
    while low < high {
        let index = low + (high - low) / 2;
        let offset = word(&names, index * 4)?;
        let mut text = Vec::new();
        for at in 0..512 {
            let byte = get(offset.checked_add(at).ok_or("export name overflow")?, 1)?[0];
            if byte == 0 {
                break;
            }
            text.push(byte);
        }
        if text.len() == 512 {
            return Err("unterminated PE export name".into());
        }
        match text.as_slice().cmp(name.as_bytes()) {
            std::cmp::Ordering::Less => low = index + 1,
            std::cmp::Ordering::Greater => high = index,
            std::cmp::Ordering::Equal => {
                let ordinal_at = word(&table, 36)?
                    .checked_add(index * 2)
                    .ok_or("export ordinal overflow")?;
                let ordinal = u16::from_le_bytes(get(ordinal_at, 2)?.try_into().unwrap()) as usize;
                if ordinal >= word(&table, 20)? {
                    return Err("invalid PE export ordinal".into());
                }
                let function_at = word(&table, 28)?
                    .checked_add(ordinal * 4)
                    .ok_or("export RVA overflow")?;
                let rva = word(&get(function_at, 4)?, 0)?;
                if rva == 0 || (rva >= dir && rva - dir < length) {
                    return Err("startup export is missing or forwarded".into());
                }
                get(rva, STATUS_BYTES)?;
                return Ok(Some(rva));
            }
        }
    }
    Ok(None)
}

fn status(bytes: &[u8], pid: u32) -> Result<u32, String> {
    if bytes.len() != STATUS_BYTES || bytes.get(..8) != Some(b"DFLBOOT1") {
        return Err("invalid loader startup magic or size".into());
    }
    if word(bytes, 8)? != 1 {
        return Err("unsupported loader startup ABI; update the EXE and host DLL together".into());
    }
    let state = word(bytes, 12)? as u32;
    let owner = word(bytes, 16)? as u32;
    if owner != pid && !(state == 0 && owner == 0) {
        return Err("loader startup status belongs to another process".into());
    }
    if state > 3 || word(bytes, 20)? != 0 {
        return Err("invalid loader startup state".into());
    }
    Ok(state)
}

fn legacy_loader(path: &Path) -> bool {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let size = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), std::ptr::null_mut()) };
    if size == 0 || size > 1024 * 1024 {
        return false;
    }
    let mut data = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(wide.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return false;
    }
    let query: Vec<u16> = "\\StringFileInfo\\040904B0\\InternalName\0"
        .encode_utf16()
        .collect();
    let mut value = std::ptr::null_mut();
    let mut count = 0;
    if unsafe { VerQueryValueW(data.as_ptr().cast(), query.as_ptr(), &mut value, &mut count) } == 0
        || value.is_null()
        || count == 0
    {
        return false;
    }
    let name = unsafe { std::slice::from_raw_parts(value.cast::<u16>(), count as usize) };
    super::wide_to_string(name).eq_ignore_ascii_case("defiance-loader")
}

fn existing(pid: u32) -> Result<Option<(Target, usize)>, String> {
    let process = Process::open(pid)?;
    let mut found = None;
    for module in modules(pid)? {
        let read = |rva: usize, bytes: usize| process.read(unsafe { module.base.add(rva) }, bytes);
        if let Ok(Some(rva)) = export_rva(&read, module.size, EXPORT) {
            status(&read(rva, STATUS_BYTES)?, pid)?;
            if found.is_some() {
                return Err(
                    "multiple plugin hosts are loaded; restart the game with one startup method"
                        .into(),
                );
            }
            found = Some((module, rva));
        } else if legacy_loader(&module.path) {
            return Err("an older plugin host is already loaded; restart the game with the updated EXE package".into());
        }
    }
    Ok(found)
}

/// The allocated filename remains valid until the loading thread finishes.
/// A timeout retains it in the target rather than freeing a live argument.
fn load_library(pid: u32, path: &Path, timeout: Duration) -> Result<(), String> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if wide[..wide.len() - 1].contains(&0) {
        return Err("host DLL path contains a NUL".into());
    }
    let address = super::remote_export(pid, "kernel32.dll", "LoadLibraryW")?;
    let handle = unsafe { OpenProcess(0x100000 | 0x400 | 0x2 | 0x8 | 0x10 | 0x20, 0, pid) };
    if handle.is_null() {
        return Err(format!(
            "cannot open the game to start its host: {}",
            std::io::Error::last_os_error()
        ));
    }
    let process = Owned(handle);
    let mut wow64 = 0;
    if unsafe { IsWow64Process(process.0, &mut wow64) } == 0 || wow64 != 0 {
        return Err("the plugin host requires a 64-bit game process".into());
    }
    let bytes = wide.len() * 2;
    let memory = unsafe { VirtualAllocEx(process.0, std::ptr::null_mut(), bytes, 0x3000, 0x04) };
    if memory.is_null() {
        return Err(format!(
            "cannot allocate host filename: {}",
            std::io::Error::last_os_error()
        ));
    }
    struct Filename<'a> {
        process: &'a Owned,
        address: *mut c_void,
        release: bool,
    }
    impl Drop for Filename<'_> {
        fn drop(&mut self) {
            if self.release {
                unsafe {
                    VirtualFreeEx(self.process.0, self.address, 0, 0x8000);
                }
            }
        }
    }
    let mut filename = Filename {
        process: &process,
        address: memory,
        release: true,
    };
    let mut written = 0;
    if unsafe { WriteProcessMemory(process.0, memory, wide.as_ptr().cast(), bytes, &mut written) }
        == 0
        || written != bytes
    {
        return Err(format!(
            "cannot write host filename: {}",
            std::io::Error::last_os_error()
        ));
    }
    let thread = unsafe {
        CreateRemoteThread(
            process.0,
            std::ptr::null_mut(),
            0,
            address as *const c_void,
            memory,
            0,
            std::ptr::null_mut(),
        )
    };
    if thread.is_null() {
        return Err(format!(
            "cannot start Windows DLL loading: {}",
            std::io::Error::last_os_error()
        ));
    }
    let thread = Owned(thread);
    match unsafe { WaitForSingleObject(thread.0, milliseconds(timeout)) } {
        0 => {}
        0x102 => {
            filename.release = false;
            return Err("Windows DLL loading is still in progress; startup timed out".into());
        }
        _ => {
            filename.release = false;
            return Err(format!(
                "cannot wait for Windows DLL loading: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    let mut code = 0;
    if unsafe { GetExitCodeThread(thread.0, &mut code) } == 0 {
        return Err(format!(
            "cannot read DLL loading result: {}",
            std::io::Error::last_os_error()
        ));
    }
    // A thread exit code is only 32 bits. Find the module by its full filename,
    // rather than interpreting a truncated 64-bit HMODULE as a handle.
    let normalize = |path: &Path| {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        path.to_string_lossy()
            .trim_start_matches("\\\\?\\")
            .to_ascii_lowercase()
    };
    if !modules(pid)?
        .iter()
        .any(|module| normalize(&module.path) == normalize(path))
    {
        return Err(format!(
            "Windows did not load {} (thread result {code:#x})",
            path.display()
        ));
    }
    Ok(())
}

fn wait_ready(pid: u32, module: &Target, rva: usize, timeout: Duration) -> Result<(), String> {
    let process = Process::open(pid)?;
    let began = Instant::now();
    loop {
        match status(
            &process.read(unsafe { module.base.add(rva) }, STATUS_BYTES)?,
            pid,
        )? {
            2 => return Ok(()),
            3 => {
                return Err(
                    "plugin host startup failed; see DefianceLoader/logs/defiance-loader.log"
                        .into(),
                )
            }
            _ => {}
        }
        if began.elapsed() >= timeout {
            return Err("plugin host is still starting; timed out waiting for readiness (see DefianceLoader/logs/defiance-loader.log)".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Return whether this call loaded the host. An existing compatible host is
/// observed without adding a DLL reference or installing patches a second time.
pub fn start(pid: u32, dll: &Path, timeout: Duration) -> Result<bool, String> {
    let dll = std::fs::canonicalize(dll).map_err(|error| format!("{}: {error}", dll.display()))?;
    let mapped = defiance_core::pe::map(&dll)?;
    let read = |rva: usize, bytes: usize| {
        mapped
            .image
            .get(rva..rva.saturating_add(bytes))
            .map(|bytes| bytes.to_vec())
            .ok_or_else(|| "loader startup span outside image".into())
    };
    let rva = export_rva(&read, mapped.image.len(), EXPORT)?
        .ok_or("host DLL has no startup status export; update the EXE and loader DLL together")?;
    status(&read(rva, STATUS_BYTES)?, 0)?;
    if mapped.is_code(rva) {
        return Err("loader startup export is not data".into());
    }
    let _lock = lock(pid, timeout)?;
    if let Some((module, rva)) = existing(pid)? {
        wait_ready(pid, &module, rva, timeout)?;
        return Ok(false);
    }
    load_library(pid, &dll, timeout)?;
    let (module, rva) = existing(pid)?.ok_or("loaded DLL did not publish its host status")?;
    wait_ready(pid, &module, rva, timeout)?;
    Ok(true)
}

#[cfg(test)]
#[path = "host_mode_tests.rs"]
mod tests;
