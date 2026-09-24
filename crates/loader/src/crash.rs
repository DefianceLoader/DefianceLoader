//! Best-effort crash capture. The exception path uses only preopened handles,
//! a stack buffer, and Win32 calls: no logger, heap allocation, or Rust locks.
use core::ffi::c_void;
use core::fmt::Write as _;
use std::os::windows::io::{AsRawHandle, IntoRawHandle};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

type Handle = *mut c_void;
type Filter = unsafe extern "system" fn(*mut Pointers) -> i32;
#[repr(C)]
pub struct Record {
    pub code: u32,
    pub flags: u32,
    pub nested: *mut Record,
    pub address: *mut c_void,
    pub count: u32,
    pub information: [usize; 15],
}
#[repr(C)]
pub struct Pointers {
    pub record: *mut Record,
    pub context: *mut c_void,
}
#[repr(C, align(16))]
pub struct Context(pub [u8; 1232]); // Windows AMD64 CONTEXT, including XMM state.
#[link(name = "kernel32")]
extern "system" {
    fn SetUnhandledExceptionFilter(filter: Option<Filter>) -> Option<Filter>;
    fn AddVectoredExceptionHandler(first: u32, handler: Filter) -> Handle;
    fn GetCurrentThreadId() -> u32;
    fn WriteFile(
        file: Handle,
        bytes: *const u8,
        size: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn CreateEventW(attributes: *mut c_void, manual: i32, initial: i32, name: *const u16)
        -> Handle;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn CloseHandle(handle: Handle) -> i32;
    fn RtlCaptureContext(context: *mut Context);
}
static FILE: AtomicUsize = AtomicUsize::new(0);
static SESSION: AtomicUsize = AtomicUsize::new(0);
static PIPE: AtomicUsize = AtomicUsize::new(0);
static DONE: AtomicUsize = AtomicUsize::new(0);
static PREVIOUS: AtomicUsize = AtomicUsize::new(0);
static CAPTURING: AtomicBool = AtomicBool::new(false);

// Published during normal initialization, never allocated or locked from an
// exception handler. Nodes live until exit; removal only clears the active bit.
struct Range {
    start: usize,
    end: usize,
    /// Leaked with the node, for the tracer's frame names.
    label: &'static str,
    active: AtomicBool,
    next: *mut Range,
}
static RANGES: AtomicPtr<Range> = AtomicPtr::new(core::ptr::null_mut());

/// Deactivate every range that starts at `start`.
fn deactivate(start: usize) {
    let mut entry = RANGES.load(Ordering::Acquire);
    while !entry.is_null() {
        unsafe {
            if (*entry).start == start {
                (*entry).active.store(false, Ordering::Release);
            }
            entry = (*entry).next;
        }
    }
}

/// Register mod code `[start, end)` with the exception observer, under a
/// human-readable `label`. Called from the hook engine at install time; nodes
/// live until exit, so this never allocates from the exception path.
pub fn map(start: usize, end: usize, label: &str) {
    if end <= start {
        return;
    }
    note(&format!("crash-map {start:#x} {end:#x} {label}\n"));
    deactivate(start);
    let node = Box::into_raw(Box::new(Range {
        start,
        end,
        label: Box::leak(label.to_string().into_boxed_str()),
        active: AtomicBool::new(true),
        next: core::ptr::null_mut(),
    }));
    let mut head = RANGES.load(Ordering::Acquire);
    loop {
        unsafe {
            (*node).next = head;
        }
        match RANGES.compare_exchange_weak(head, node, Ordering::Release, Ordering::Acquire) {
            Ok(_) => break,
            Err(current) => head = current,
        }
    }
}

/// The active range holding `address`: its start and label. Allocation-free.
pub fn range_of(address: usize) -> Option<(usize, &'static str)> {
    let mut entry = RANGES.load(Ordering::Acquire);
    while !entry.is_null() {
        let range = unsafe { &*entry };
        if range.active.load(Ordering::Acquire) && range.start <= address && address < range.end {
            return Some((range.start, range.label));
        }
        entry = range.next;
    }
    None
}

/// Stop watching a registered range. Called from the hook engine at removal.
pub fn unmap(start: usize) {
    if start == 0 {
        return;
    }
    note(&format!("crash-unmap {start:#x}\n"));
    deactivate(start);
}

/// A plugin's label for a crash range or trace site, if it keeps to the
/// services' contract: UTF-8, 1..=128 bytes, printable (no line breaks, which
/// would let a label forge journal or log entries).
pub fn service_label(label: *const core::ffi::c_char) -> Option<String> {
    if label.is_null() {
        return None;
    }
    let bytes = unsafe { core::ffi::CStr::from_ptr(label) }.to_bytes();
    let text = core::str::from_utf8(bytes).ok()?;
    (!text.is_empty() && text.len() <= 128 && !text.chars().any(char::is_control))
        .then(|| text.to_owned())
}

unsafe extern "C" fn service_map(start: usize, end: usize, label: *const core::ffi::c_char) -> i32 {
    match service_label(label) {
        Some(label) if start < end => {
            map(start, end, &label);
            0
        }
        _ => 1,
    }
}

unsafe extern "C" fn service_unmap(start: usize) -> i32 {
    if start == 0 {
        return 1;
    }
    unmap(start);
    0
}

/// `defiance.loader` / `crash-ranges` v1: [`map`] and [`unmap`] for plugins,
/// whose ranges (Core's payload blocks, for one) the loader cannot see.
pub static RANGES_API: defiance_api::CrashRangesV1 = defiance_api::CrashRangesV1 {
    map: service_map,
    unmap: service_unmap,
};
unsafe extern "system" fn owned_exception(pointers: *mut Pointers) -> i32 {
    if pointers.is_null() || (*pointers).record.is_null() || CAPTURING.load(Ordering::Acquire) {
        return 0;
    }
    let record = &*(*pointers).record;
    if !matches!(
        record.code,
        0xc0000005 | 0xc0000006 | 0xc000001d | 0xc0000094
    ) {
        return 0;
    }
    let address = record.address as usize;
    let mut entry = RANGES.load(Ordering::Acquire);
    while !entry.is_null() {
        if (*entry).active.load(Ordering::Acquire)
            && address >= (*entry).start
            && address < (*entry).end
        {
            write(
                FILE.load(Ordering::Acquire),
                b"first-chance exception in registered mod code; may subsequently be handled\n",
            );
            capture(pointers);
            break;
        }
        entry = (*entry).next;
    }
    0 // Always CONTINUE_SEARCH, never swallow an exception or change context.
}

struct Buffer {
    bytes: [u8; 4096],
    len: usize,
}
impl Buffer {
    fn new() -> Self {
        Self {
            bytes: [0; 4096],
            len: 0,
        }
    }
}
impl core::fmt::Write for Buffer {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let mut count = text.len().min(self.bytes.len() - self.len);
        while !text.is_char_boundary(count) {
            count -= 1;
        }
        self.bytes[self.len..self.len + count].copy_from_slice(&text.as_bytes()[..count]);
        self.len += count;
        Ok(())
    }
}
fn write(handle: usize, bytes: &[u8]) {
    if handle != 0 {
        let mut written = 0;
        unsafe {
            WriteFile(
                handle as Handle,
                bytes.as_ptr(),
                bytes.len() as u32,
                &mut written,
                core::ptr::null_mut(),
            );
        }
    }
}
/// Write progress text to the session file. Range bookkeeping is not inferred
/// from this text; `map` and `unmap` maintain it directly.
pub fn note(text: &str) {
    if !available() {
        return;
    }
    write(SESSION.load(Ordering::Acquire), text.as_bytes());
}
pub fn available() -> bool {
    FILE.load(Ordering::Acquire) != 0
}

/// Called on the host thread, never under DllMain. Handles intentionally live
/// until process exit. A missing helper leaves the text capture operational.
pub fn initialize(directory: &Path, helper: &Path) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(directory)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let stem = directory.join(format!("defiance-{}-{nonce}", std::process::id()));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stem.with_extension("crash.txt"))?;
    let session = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stem.with_extension("session.txt"))?;
    // Do not replace a working capture installation.
    if FILE
        .compare_exchange(
            0,
            file.as_raw_handle() as usize,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "crash capture already initialized",
        ));
    }
    let _ = file.into_raw_handle();
    SESSION.store(session.into_raw_handle() as usize, Ordering::Release);
    note(&format!(
        "defiance-loader crash session pid={} version={}\n",
        std::process::id(),
        env!("CARGO_PKG_VERSION")
    ));
    let event_name = format!("Local\\DefianceCrash-{}-{nonce}", std::process::id());
    let wide: Vec<u16> = event_name.encode_utf16().chain(Some(0)).collect();
    let event = unsafe { CreateEventW(core::ptr::null_mut(), 1, 0, wide.as_ptr()) };
    if !event.is_null() {
        match std::process::Command::new(helper)
            .args([
                std::process::id().to_string(),
                stem.to_string_lossy().into_owned(),
                event_name,
            ])
            .creation_flags(0x08000000)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                PIPE.store(
                    child.stdin.take().unwrap().into_raw_handle() as usize,
                    Ordering::Release,
                );
                DONE.store(event as usize, Ordering::Release);
                note("dump helper launched; completion/status is recorded separately on a crash\n");
            }
            Err(error) => {
                unsafe {
                    CloseHandle(event);
                }
                note(&format!(
                    "dump helper unavailable: {error}; text capture only\n"
                ));
            }
        }
    } else {
        note("dump event unavailable; text capture only\n");
    }
    if unsafe { AddVectoredExceptionHandler(1, owned_exception) }.is_null() {
        note("owned-code exception observer unavailable; unhandled capture remains active\n");
    }
    let previous = unsafe { SetUnhandledExceptionFilter(Some(filter)) };
    PREVIOUS.store(previous.map_or(0, |f| f as usize), Ordering::Release);
    std::panic::set_hook(Box::new(|info| {
        let mut buffer = Buffer::new();
        let _ = writeln!(buffer, "loader Rust panic: {info}");
        unsafe {
            panic_report(buffer.bytes.as_ptr(), buffer.len);
        }
    }));
    Ok(stem)
}

unsafe extern "system" fn filter(pointers: *mut Pointers) -> i32 {
    capture(pointers);
    let previous = PREVIOUS.load(Ordering::Acquire);
    if previous != 0 {
        core::mem::transmute::<usize, Filter>(previous)(pointers)
    } else {
        0
    } // CONTINUE_SEARCH
}

/// Optional plugin panic callback. The caller retains the UTF-8 bytes for the
/// duration of this synchronous call. This records evidence, then returns so
/// the originating Rust runtime follows its configured panic policy.
pub unsafe extern "C" fn panic_report(message: *const u8, length: usize) {
    if message.is_null() || FILE.load(Ordering::Acquire) == 0 {
        return;
    }
    write(
        FILE.load(Ordering::Acquire),
        core::slice::from_raw_parts(message, length.min(4096)),
    );
    let mut context = Context([0; 1232]);
    RtlCaptureContext(&mut context);
    let rip = u64::from_le_bytes(context.0[248..256].try_into().unwrap());
    let mut record = Record {
        code: 0xe042444c,
        flags: 0,
        nested: core::ptr::null_mut(),
        address: rip as *mut _,
        count: 0,
        information: [0; 15],
    };
    let mut pointers = Pointers {
        record: &mut record,
        context: (&mut context as *mut Context).cast(),
    };
    capture(&mut pointers);
}

unsafe fn capture(pointers: *mut Pointers) {
    if pointers.is_null()
        || (*pointers).record.is_null()
        || (*pointers).context.is_null()
        || CAPTURING.swap(true, Ordering::AcqRel)
    {
        return;
    }
    let record = &*(*pointers).record;
    let tid = GetCurrentThreadId();
    let mut buffer = Buffer::new();
    let _ = writeln!(buffer, "exception={:#010x} instruction={:#x} thread={}\ncontext=Windows AMD64; panic code 0xe042444c is a reporting-time snapshot", record.code, record.address as usize, tid);
    if (record.code == 0xc0000005 || record.code == 0xc0000006) && record.count >= 2 {
        let _ = writeln!(
            buffer,
            "access={} address={:#x} (0=read,1=write,8=execute)",
            record.information[0], record.information[1]
        );
    }
    // These are fixed ABI offsets, not guessed game layouts. Context stays
    // live throughout the helper's ReadProcessMemory/MiniDumpWriteDump calls.
    let context = core::slice::from_raw_parts((*pointers).context.cast::<u8>(), 256);
    for (name, offset) in [
        ("RAX", 120),
        ("RCX", 128),
        ("RDX", 136),
        ("RBX", 144),
        ("RSP", 152),
        ("RBP", 160),
        ("RSI", 168),
        ("RDI", 176),
        ("R8", 184),
        ("R9", 192),
        ("R10", 200),
        ("R11", 208),
        ("R12", 216),
        ("R13", 224),
        ("R14", 232),
        ("R15", 240),
        ("RIP", 248),
    ] {
        let value = u64::from_le_bytes(context[offset..offset + 8].try_into().unwrap());
        let _ = writeln!(buffer, "{name}={value:#018x}");
    }
    let flags = u32::from_le_bytes(context[68..72].try_into().unwrap());
    let _ = writeln!(buffer, "EFLAGS={flags:#010x}");
    write(FILE.load(Ordering::Acquire), &buffer.bytes[..buffer.len]);
    let pipe = PIPE.load(Ordering::Acquire);
    if pipe != 0 {
        let mut packet = [0u8; 16];
        packet[..8].copy_from_slice(&(pointers as u64).to_le_bytes());
        packet[8..12].copy_from_slice(&tid.to_le_bytes());
        write(pipe, &packet);
        let result = WaitForSingleObject(DONE.load(Ordering::Acquire) as Handle, 10000);
        write(
            FILE.load(Ordering::Acquire),
            if result == 0 {
                b"helper completed; see dump status\n"
            } else {
                b"helper timed out/failed; dump may be incomplete\n"
            },
        );
    } else {
        write(
            FILE.load(Ordering::Acquire),
            b"no dump helper; text report only\n",
        );
    }
    // Remain latched: recursion, concurrent faults and a later panic abort
    // must not overwrite the first evidence or send a second pipe request.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_labels_keep_to_the_contract() {
        assert_eq!(
            service_label(c"core.logic payload".as_ptr()).as_deref(),
            Some("core.logic payload")
        );
        assert_eq!(service_label(c"".as_ptr()), None);
        assert_eq!(service_label(c"two\nlines".as_ptr()), None);
        assert_eq!(service_label(core::ptr::null()), None);
        let long = std::ffi::CString::new("x".repeat(129)).unwrap();
        assert_eq!(service_label(long.as_ptr()), None);
        assert_eq!(
            unsafe { service_map(0x2000, 0x1000, c"backwards".as_ptr()) },
            1
        );
        assert_eq!(unsafe { service_unmap(0) }, 1);
    }
}
