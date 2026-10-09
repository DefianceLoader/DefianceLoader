//! A process's memory and the module a patch is written into: reading,
//! writing, allocating near a module, and resolving the Win32 exports injected
//! code calls. Shared by the injector, the loader's Core plugin and the
//! injector's descriptor writes in [`crate::compat::apply`].
//!
//! `Process::open` on this process's own pid uses local calls; that is how the
//! loader and the injector's self test address memory. Another pid is a game
//! that is already running.

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
    fn VirtualFree(address: *mut u8, size: usize, kind: u32) -> i32;
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
    fn VirtualFreeEx(process: Handle, address: *mut u8, size: usize, kind: u32) -> i32;
    fn FlushInstructionCache(process: Handle, address: *const u8, size: usize) -> i32;
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn GetLastError() -> u32;
}

/// A Win32 export the injected code calls, resolved by name. The process that
/// resolves it may not have user32 loaded, so it loads it; system DLLs sit at
/// the same base in every process, so the address resolved here is the one the
/// target can call.
pub fn resolve_export(dll: &str, name: &str) -> Result<*mut c_void, String> {
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
/// self test — allocations and writes use local calls rather than the `*Ex`
/// ones. Reads use `ReadProcessMemory` in both cases so a diagnostic pointer
/// or a foreign hook can fail cleanly instead of dereferencing invalid memory.
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

    /// Release an allocation whose hooks are absent or have been restored.
    pub(crate) fn release(&self, address: *mut u8) {
        const MEM_RELEASE: u32 = 0x8000;
        unsafe {
            if self.local {
                VirtualFree(address, 0, MEM_RELEASE);
            } else {
                VirtualFreeEx(self.handle, address, 0, MEM_RELEASE);
            }
        }
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
        let write_error = (ok == 0 || written != data.len()).then(|| unsafe { GetLastError() });
        if write_error.is_none() {
            unsafe { FlushInstructionCache(self.handle, address, data.len()) };
        }
        let restored =
            unsafe { VirtualProtectEx(self.handle, address, data.len(), previous, &mut previous) };
        if let Some(code) = write_error {
            return Err(format!("WriteProcessMemory failed with error {code}"));
        }
        if restored == 0 {
            return Err(format!(
                "restoring protection failed with error {}",
                unsafe { GetLastError() }
            ));
        }
        Ok(())
    }

    /// Commits `size` bytes of executable, writable memory as close to `hint`
    /// as the address space allows, so that a rel32 from the call site can
    /// still reach it. Walks outward in allocation-granularity steps and stays
    /// inside 2GB either way.
    ///
    /// A candidate that is already in use fails with `ERROR_INVALID_ADDRESS`.
    /// Any other error is the system refusing executable memory outright (an
    /// exploit-protection policy, say), so the first one is named in the
    /// failure rather than reported as a full address space.
    pub fn reserve_near(&self, hint: *mut u8, size: usize) -> Result<*mut u8, String> {
        const GRANULARITY: usize = 0x10000;
        const REACH: usize = 0x8000; // 0x8000 * 64K is 2GB
        const ERROR_INVALID_ADDRESS: u32 = 487;
        let base = (hint as usize) & !(GRANULARITY - 1);
        let mut refused: Option<(u32, usize)> = None;
        for step in 1..REACH {
            for candidate in [
                base.wrapping_add(step * GRANULARITY),
                base.checked_sub(step * GRANULARITY).unwrap_or(0),
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
                let error = unsafe { GetLastError() };
                if error != ERROR_INVALID_ADDRESS && refused.is_none() {
                    refused = Some((error, candidate));
                }
            }
        }
        Err(match refused {
            Some((error, at)) => format!(
                "no executable page within reach of the call site: VirtualAlloc at {at:#x} \
                 failed with error {error}"
            ),
            None => "no free page within reach of the call site".to_string(),
        })
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
