//! Dedicated dump writer. Started hidden before a crash; one 16-byte request
//! arrives on stdin, while the faulting thread waits with its context intact.
use core::ffi::c_void;
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
type Handle = *mut c_void;
#[repr(C, packed(4))] // minidumpapiset.h uses pshpack4.h, including on x64.
struct ExceptionInfo {
    thread: u32,
    pointers: u64,
    client_pointers: i32,
}
#[repr(C)]
struct ModuleInfo {
    base: Handle,
    size: u32,
    entry: Handle,
}
#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> Handle;
    fn SetEvent(event: Handle) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn ReadProcessMemory(
        process: Handle,
        address: *const c_void,
        buffer: *mut u8,
        size: usize,
        read: *mut usize,
    ) -> i32;
    fn GetLastError() -> u32;
}
#[link(name = "dbghelp")]
extern "system" {
    fn MiniDumpWriteDump(
        process: Handle,
        pid: u32,
        file: Handle,
        kind: u32,
        exception: *const ExceptionInfo,
        user: Handle,
        callback: Handle,
    ) -> i32;
}
#[link(name = "psapi")]
extern "system" {
    fn EnumProcessModulesEx(
        process: Handle,
        modules: *mut Handle,
        size: u32,
        needed: *mut u32,
        filter: u32,
    ) -> i32;
    fn GetModuleInformation(
        process: Handle,
        module: Handle,
        info: *mut ModuleInfo,
        size: u32,
    ) -> i32;
    fn GetModuleFileNameExW(process: Handle, module: Handle, path: *mut u16, size: u32) -> u32;
}
fn read(process: Handle, address: u64, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    let mut count = 0;
    unsafe {
        ReadProcessMemory(
            process,
            address as *const _,
            bytes.as_mut_ptr(),
            size,
            &mut count,
        );
    }
    bytes.truncate(count);
    bytes
}
fn word(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}
fn describe(process: Handle, pointers: u64, output: &mut std::fs::File, session: &str) {
    let _ = writeln!(
        output,
        "defiance-loader crash helper: addresses identify location, not necessarily the cause"
    );
    let ptrs = read(process, pointers, 16);
    let context = word(&ptrs, 8)
        .map(|at| read(process, at, 256))
        .unwrap_or_default();
    let rip = word(&context, 248).unwrap_or(0);
    let mut modules = vec![core::ptr::null_mut(); 2048];
    let mut needed = 0;
    if unsafe {
        EnumProcessModulesEx(
            process,
            modules.as_mut_ptr(),
            (modules.len() * 8) as u32,
            &mut needed,
            3,
        )
    } != 0
    {
        if needed as usize > modules.len() * 8 {
            let _ = writeln!(output, "module list truncated");
        }
        for &module in modules
            .iter()
            .take((needed as usize / 8).min(modules.len()))
        {
            let mut info: ModuleInfo = unsafe { core::mem::zeroed() };
            if unsafe {
                GetModuleInformation(
                    process,
                    module,
                    &mut info,
                    core::mem::size_of::<ModuleInfo>() as u32,
                )
            } == 0
            {
                continue;
            }
            let mut path = [0u16; 1024];
            let length = unsafe {
                GetModuleFileNameExW(process, module, path.as_mut_ptr(), path.len() as u32)
            } as usize;
            let path = String::from_utf16_lossy(&path[..length.min(path.len())]);
            let base = info.base as u64;
            let _ = writeln!(output, "module {base:#x} size={:#x} {path}", info.size);
            if rip >= base && rip - base < info.size as u64 {
                let _ = writeln!(output, "fault location: {path}+{:#x}", rip - base);
            }
        }
    }
    // Replay journal additions/removals; mapping attribution is evidence of
    // location, not proof that the owner caused the corruption.
    let mut ranges = std::collections::BTreeMap::new();
    for line in session.lines() {
        let line = line.strip_prefix("[info] ").unwrap_or(line);
        let fields: Vec<_> = line.splitn(4, ' ').collect();
        if fields.len() >= 2 && fields[0] == "crash-unmap" {
            if let Ok(start) = u64::from_str_radix(fields[1].trim_start_matches("0x"), 16) {
                ranges.remove(&start);
            }
        }
        if fields.len() == 4 && fields[0] == "crash-map" {
            if let (Ok(start), Ok(end)) = (
                u64::from_str_radix(fields[1].trim_start_matches("0x"), 16),
                u64::from_str_radix(fields[2].trim_start_matches("0x"), 16),
            ) {
                ranges.insert(start, (end, fields[3]));
            }
        }
    }
    for (start, (end, label)) in ranges {
        if start <= rip && rip < end {
            let _ = writeln!(
                output,
                "fault mapping: {label}+{:#x} (range {start:#x}..{end:#x})",
                rip - start
            );
        }
    }
    if let Some(rsp) = word(&context, 152) {
        let stack = read(process, rsp, 256);
        let _ = writeln!(
            output,
            "raw stack words at RSP (NOT an unwound call stack):"
        );
        for (index, bytes) in stack.chunks_exact(8).enumerate() {
            let _ = writeln!(
                output,
                "{:#x}: {:#018x}",
                rsp + index as u64 * 8,
                u64::from_le_bytes(bytes.try_into().unwrap())
            );
        }
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 4 {
        return Err("expected PID, output stem, completion event".into());
    }
    let pid: u32 = args[1].to_string_lossy().parse()?;
    let stem = std::path::PathBuf::from(&args[2]);
    let event_name: Vec<u16> = args[3]
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let event = unsafe { OpenEventW(2, 0, event_name.as_ptr()) };
    let process = unsafe { OpenProcess(0x0400 | 0x0010, 0, pid) }; // QUERY_INFORMATION | VM_READ
    let mut packet = [0; 16];
    // EOF on a normal game exit causes a quiet exit without creating a dump.
    if std::io::stdin().read_exact(&mut packet).is_err() {
        unsafe {
            if !process.is_null() {
                CloseHandle(process);
            }
            if !event.is_null() {
                CloseHandle(event);
            }
        }
        return Ok(());
    }
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        if process.is_null() {
            return Err("could not open target process for dump capture".into());
        }
        let pointers = u64::from_le_bytes(packet[..8].try_into()?);
        let exception = ExceptionInfo {
            thread: u32::from_le_bytes(packet[8..12].try_into()?),
            pointers,
            client_pointers: 1,
        };
        let file = std::fs::File::create(stem.with_extension("dmp"))?;
        let ok = unsafe {
            MiniDumpWriteDump(
                process,
                pid,
                file.as_raw_handle(),
                0x1040 | 0x20,
                &exception,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(format!("MiniDumpWriteDump failed: Win32 {}", unsafe {
                GetLastError()
            })
            .into());
        }
        file.sync_all()?;
        let mut details = std::fs::File::create(stem.with_extension("details.txt"))?;
        let session =
            std::fs::read_to_string(stem.with_extension("session.txt")).unwrap_or_default();
        describe(process, pointers, &mut details, &session);
        Ok(())
    })();
    let status = match &result {
        Ok(()) => "dump complete\n".to_string(),
        Err(e) => format!("dump failed: {e}\n"),
    };
    let _ = std::fs::write(stem.with_extension("status.txt"), status);
    unsafe {
        if !event.is_null() {
            SetEvent(event);
            CloseHandle(event);
        }
        if !process.is_null() {
            CloseHandle(process);
        }
    }
    result
}
fn main() {
    if run().is_err() {
        std::process::exit(1);
    }
}
