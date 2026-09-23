//! The Win32 surface the loader uses, declared here rather than pulled in.
//!
//! Same reason as the injector: the project depends on nothing but a
//! toolchain. Only the calls the loader actually makes are declared. On x64
//! there is one calling convention, so `extern "system"` and `extern "C"` are
//! the same thing; the API uses `"C"` to match the C header.

use core::ffi::c_void;

pub type Handle = *mut c_void;

pub const DLL_PROCESS_ATTACH: u32 = 1;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub const MEM_COMMIT: u32 = 0x1000;
pub const MEM_COMMIT_RESERVE: u32 = 0x3000;
pub const MEM_RELEASE: u32 = 0x8000;
/// Used by the restore-failure tests: decommit but keep the reservation.
#[cfg(test)]
pub const MEM_DECOMMIT: u32 = 0x4000;
pub const PAGE_GUARD: u32 = 0x100;

/// Access rights used to suspend and read thread contexts.
pub const THREAD_SUSPEND_RESUME: u32 = 0x0002;
pub const THREAD_GET_CONTEXT: u32 = 0x0008;
pub const THREAD_QUERY_INFORMATION: u32 = 0x0040;

/// The protections that mean "this page can execute".
const EXECUTE_PROTECTIONS: [u32; 4] = [0x10, 0x20, 0x40, 0x80];

#[repr(C)]
pub struct MemoryBasicInformation {
    pub base_address: *mut c_void,
    pub allocation_base: *mut c_void,
    pub allocation_protect: u32,
    pub region_size: usize,
    pub state: u32,
    pub protect: u32,
    pub kind: u32,
}

/// Whether the page holding `address` is executable: how the RTTI walk knows
/// where a vtable's methods end.
pub fn is_executable(address: usize) -> bool {
    let mut info: MemoryBasicInformation = unsafe { core::mem::zeroed() };
    let asked = unsafe {
        VirtualQuery(
            address as *const c_void,
            &mut info,
            core::mem::size_of::<MemoryBasicInformation>(),
        )
    };
    if asked == 0 || info.protect & PAGE_GUARD != 0 {
        return false;
    }
    EXECUTE_PROTECTIONS.contains(&(info.protect & 0xff))
}

#[link(name = "kernel32")]
extern "system" {
    pub fn DisableThreadLibraryCalls(module: Handle) -> i32;
    pub fn CreateThread(
        attributes: *mut c_void,
        stack: usize,
        start: unsafe extern "system" fn(*mut c_void) -> u32,
        parameter: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    pub fn Sleep(milliseconds: u32);
    pub fn GetModuleHandleW(name: *const u16) -> Handle;
    pub fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    pub fn LoadLibraryW(name: *const u16) -> Handle;
    pub fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    pub fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    pub fn VirtualFree(address: *mut c_void, size: usize, kind: u32) -> i32;
    pub fn VirtualProtect(
        address: *mut c_void,
        size: usize,
        protect: u32,
        previous: *mut u32,
    ) -> i32;
    pub fn FlushInstructionCache(process: Handle, address: *const c_void, size: usize) -> i32;
    pub fn GetCurrentProcess() -> Handle;
    pub fn GetCurrentThreadId() -> u32;
    pub fn GetThreadId(thread: Handle) -> u32;
    pub fn TerminateProcess(process: Handle, code: u32) -> i32;
    pub fn GetLastError() -> u32;
    pub fn SuspendThread(thread: Handle) -> u32;
    pub fn ResumeThread(thread: Handle) -> u32;
    pub fn CloseHandle(handle: Handle) -> i32;
    pub fn GetThreadContext(thread: Handle, context: *mut c_void) -> i32;
    pub fn OutputDebugStringW(message: *const u16);
    pub fn VirtualQuery(
        address: *const c_void,
        info: *mut MemoryBasicInformation,
        size: usize,
    ) -> usize;
}

// Native enumeration avoids Toolhelp heap allocation while peers are stopped.
#[link(name = "ntdll")]
extern "system" {
    pub fn NtGetNextThread(
        process: Handle,
        previous: Handle,
        access: u32,
        attributes: u32,
        flags: u32,
        next: *mut Handle,
    ) -> i32;
    pub fn NtQueryInformationThread(
        thread: Handle,
        class: u32,
        information: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

#[link(name = "psapi")]
extern "system" {
    pub fn GetModuleInformation(
        process: Handle,
        module: Handle,
        info: *mut ModuleInfo,
        size: u32,
    ) -> i32;
}

#[repr(C)]
pub struct ModuleInfo {
    pub base: *mut c_void,
    pub size: u32,
    pub entry: *mut c_void,
}

/// A NUL-terminated UTF-16 string, from `&str`.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(core::iter::once(0)).collect()
}

/// The `&str` up to the first NUL of a UTF-16 buffer.
pub fn from_wide(chars: &[u16]) -> String {
    let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
    String::from_utf16_lossy(&chars[..end])
}
