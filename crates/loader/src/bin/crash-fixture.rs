//! Deliberately crashes a disposable subprocess; never packaged or run in game.
use core::ffi::c_void;
use defiance_loader::crash;
static CHAIN_FILE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
#[link(name = "kernel32")]
extern "system" {
    fn SetErrorMode(mode: u32) -> u32;
    fn SetUnhandledExceptionFilter(
        filter: unsafe extern "system" fn(*mut crash::Pointers) -> i32,
    ) -> usize;
    fn AddVectoredExceptionHandler(
        first: u32,
        handler: unsafe extern "system" fn(*mut crash::Pointers) -> i32,
    ) -> *mut c_void;
    fn RaiseException(code: u32, flags: u32, count: u32, args: *const usize);
    fn LoadLibraryW(path: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}
unsafe extern "system" fn previous(_: *mut crash::Pointers) -> i32 {
    std::fs::write(CHAIN_FILE.get().unwrap(), b"previous filter called").unwrap();
    1 // Standard terminate behavior, without invoking WER UI in this test.
}
unsafe extern "system" fn handled(pointers: *mut crash::Pointers) -> i32 {
    if (*(*pointers).record).code == 0xe0421234 {
        -1
    } else {
        0
    }
}
unsafe extern "system" fn recover_owned(pointers: *mut crash::Pointers) -> i32 {
    if (*(*pointers).record).code != 0xc0000005 {
        return 0;
    }
    let rip = (*pointers).context.cast::<u8>().add(248).cast::<u64>();
    *rip += 3; // fixture's exact "mov [rcx], rax", then continue to RET
    -1
}
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let directory = std::path::PathBuf::from(&args[1]);
    let helper = std::path::PathBuf::from(&args[2]);
    let mode = args[3].to_str().unwrap();
    CHAIN_FILE.set(directory.join("chained.txt")).unwrap();
    unsafe {
        SetErrorMode(0x0002 | 0x8000);
        SetUnhandledExceptionFilter(previous);
    }
    let stem = crash::initialize(&directory, &helper).unwrap();
    std::fs::write(
        directory.join("stem.txt"),
        stem.to_string_lossy().as_bytes(),
    )
    .unwrap();
    if mode == "normal" {
        return;
    }
    if mode == "handled" {
        unsafe {
            AddVectoredExceptionHandler(1, handled);
            RaiseException(0xe0421234, 0, 0, core::ptr::null());
        }
        return;
    }
    if mode == "panic" {
        panic!("intentional crash regression panic");
    }
    if mode == "plugin-panic" {
        let path: Vec<u16> = args[4]
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe {
            let module = LoadLibraryW(path.as_ptr());
            assert!(!module.is_null());
            let accept = GetProcAddress(module, b"defiance_plugin_crash_v1\0".as_ptr());
            assert!(!accept.is_null());
            let accept: unsafe extern "C" fn(unsafe extern "C" fn(*const u8, usize)) =
                core::mem::transmute(accept);
            accept(crash::panic_report);
            let fail = GetProcAddress(module, b"defiance_test_panic\0".as_ptr());
            assert!(!fail.is_null());
            core::mem::transmute::<_, unsafe extern "C" fn()>(fail)();
        }
    }
    // The mapping journal must ignore a removed range and retain the live one.
    // Typed registration, not crash-map log text.
    let start = fail as *const () as usize;
    crash::map(start, start + 256, "fixture.removed");
    crash::unmap(start);
    crash::map(start - 1, start + 256, "fixture.live");
    // A log line that looks like a mapping is journaled as log text and must
    // not map anything.
    crash::note(&format!(
        "[info] crash-map {:#x} {:#x} fixture.logtext\n",
        start - 2,
        start + 256
    ));
    if mode == "removed-range" {
        crash::unmap(start - 1);
    }
    if mode == "replaced-filter" || mode == "removed-range" {
        unsafe {
            SetUnhandledExceptionFilter(previous);
        }
    }
    if mode == "handled-owned" {
        unsafe {
            AddVectoredExceptionHandler(0, recover_owned);
        }
    }
    unsafe {
        fail();
    }
}
#[inline(never)]
unsafe fn fail() {
    core::arch::asm!("mov rax, 0x123456789abcdef0", "xor rcx, rcx", "mov [rcx], rax", out("rax") _, out("rcx") _, options(nostack));
}
