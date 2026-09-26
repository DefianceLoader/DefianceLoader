//! Diagnostic call-stack tracing (`[trace]` in core.ini): log who reaches an
//! address, as module+RVA frames, without patching it. A hardware execution
//! breakpoint changes no code, so a site another plugin already hooks or
//! patches can be traced too, which a hook could not.
//!
//! Sites come from `[trace] sites` at startup and from plugins through the
//! loader's `trace` service ([`API`]); both share four slots, one per debug
//! register. A site is released once it has logged its hits.
//!
//! A helper thread arms every thread's debug registers and re-arms them every
//! two seconds, so threads the game starts later and released slots are
//! covered. On a hit the vectored handler walks the stack from the exception
//! context with the Windows unwinder, logs it with the argument registers, and
//! resumes with the resume flag set. A released address stays handled (it may
//! still be armed in a thread the helper has not reached) until
//! [`RETIRED_SLOTS`] later releases have passed.
use crate::win;
use core::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use std::sync::OnceLock;

pub const MAX_SITES: usize = 4;
const EXCEPTION_SINGLE_STEP: u32 = 0x8000_0004;
const CONTINUE_EXECUTION: i32 = -1;
const CONTINUE_SEARCH: i32 = 0;
const CONTEXT_DEBUG_REGISTERS: u32 = 0x0010_0010;
const THREAD_SET_CONTEXT: u32 = 0x0010;
const TH32CS_SNAPTHREAD: u32 = 0x4;
// Windows AMD64 CONTEXT offsets.
const FLAGS: usize = 0x30;
const EFLAGS: usize = 0x44;
const DR0: usize = 0x48;
const DR6: usize = 0x68;
const DR7: usize = 0x70;
const RAX: usize = 0x78;
const RCX: usize = 0x80;
const RDX: usize = 0x88;
const R8: usize = 0xb8;
const R9: usize = 0xc0;
const RIP: usize = 0xf8;
const RESUME_FLAG: u32 = 0x1_0000;

#[repr(C, align(16))]
struct Context([u8; 1232]);
impl Context {
    fn new() -> Box<Self> {
        Box::new(Context([0; 1232]))
    }
    fn u64(&self, at: usize) -> u64 {
        u64::from_le_bytes(self.0[at..at + 8].try_into().unwrap())
    }
    fn set_u64(&mut self, at: usize, value: u64) {
        self.0[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
}

#[repr(C)]
struct Pointers {
    record: *mut u32,
    context: *mut u8,
}

#[repr(C)]
struct ThreadEntry {
    size: u32,
    usage: u32,
    thread: u32,
    process: u32,
    base_priority: i32,
    delta_priority: i32,
    flags: u32,
}

// crash.rs declares AddVectoredExceptionHandler with its own Pointers type; the
// ABI is the same pointer either way.
#[allow(clashing_extern_declarations)]
#[link(name = "kernel32")]
extern "system" {
    fn AddVectoredExceptionHandler(
        first: u32,
        handler: unsafe extern "system" fn(*mut Pointers) -> i32,
    ) -> *mut c_void;
    fn OpenThread(access: u32, inherit: i32, id: u32) -> win::Handle;
    fn SetThreadContext(thread: win::Handle, context: *const c_void) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, process: u32) -> win::Handle;
    fn Thread32First(snapshot: win::Handle, entry: *mut ThreadEntry) -> i32;
    fn Thread32Next(snapshot: win::Handle, entry: *mut ThreadEntry) -> i32;
    fn GetCurrentProcessId() -> u32;
    fn RtlLookupFunctionEntry(pc: u64, image_base: *mut u64, history: *mut c_void) -> *mut c_void;
    fn RtlVirtualUnwind(
        handler_type: u32,
        image_base: u64,
        pc: u64,
        function: *mut c_void,
        context: *mut Context,
        handler_data: *mut *mut c_void,
        establisher: *mut u64,
        pointers: *mut c_void,
    ) -> *mut c_void;
}

/// Each slot's address while it is traced, else 0. The handler matches on it.
static SITES: [AtomicUsize; MAX_SITES] = [const { AtomicUsize::new(0) }; MAX_SITES];
/// Whether a slot is taken, including while it is being filled in.
static CLAIMED: [AtomicBool; MAX_SITES] = [const { AtomicBool::new(false) }; MAX_SITES];
static HITS: [AtomicU32; MAX_SITES] = [const { AtomicU32::new(0) }; MAX_SITES];
static LIMITS: [AtomicU32; MAX_SITES] = [const { AtomicU32::new(0) }; MAX_SITES];
/// Each slot's label, leaked so a hit being reported never reads a freed one.
static LABELS: [AtomicPtr<String>; MAX_SITES] =
    [const { AtomicPtr::new(core::ptr::null_mut()) }; MAX_SITES];
/// Recently released addresses, still handled while threads are re-armed.
pub const RETIRED_SLOTS: usize = 64;
static RETIRED: [AtomicUsize; RETIRED_SLOTS] = [const { AtomicUsize::new(0) }; RETIRED_SLOTS];
static RETIRED_NEXT: AtomicUsize = AtomicUsize::new(0);
static RUNNING: OnceLock<Result<(), String>> = OnceLock::new();
static SINK: OnceLock<fn(&str)> = OnceLock::new();

/// Why a site could not be traced.
#[derive(Debug, PartialEq, Eq)]
pub enum TraceError {
    /// Not executable code, or no hits asked for.
    Invalid,
    /// All four slots are in use.
    Full,
    /// The address is already traced.
    Duplicate,
    /// The handler or the arming thread could not be started.
    Unavailable(String),
}

impl core::fmt::Display for TraceError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            TraceError::Invalid => write!(f, "not code, or no hits"),
            TraceError::Full => write!(f, "all {MAX_SITES} debug registers in use"),
            TraceError::Duplicate => write!(f, "already traced"),
            TraceError::Unavailable(reason) => write!(f, "{reason}"),
        }
    }
}

/// A requested site: `module+rva` (`logic+0x42a940`, `game.dll+366fa7`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub module: String,
    pub rva: usize,
}

/// The sites in a `[trace] sites` value: comma-separated `module+rva`, the
/// module named with or without `.dll`, the RVA in hex with or without `0x`.
pub fn parse(text: &str) -> Result<Vec<Site>, String> {
    let mut sites = Vec::new();
    for item in text
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let (module, rva) = item
            .split_once('+')
            .ok_or_else(|| format!("`{item}` is not module+rva"))?;
        let module = module.trim().to_ascii_lowercase();
        if module.is_empty()
            || !module
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
        {
            return Err(format!("`{item}` has no valid module name"));
        }
        let module = if module.ends_with(".dll") || module.ends_with(".exe") {
            module
        } else {
            format!("{module}.dll")
        };
        let digits = rva.trim().trim_start_matches("0x").trim_start_matches("0X");
        let rva =
            usize::from_str_radix(digits, 16).map_err(|_| format!("`{item}` has no hex RVA"))?;
        sites.push(Site { module, rva });
    }
    if sites.len() > MAX_SITES {
        return Err(format!(
            "at most {MAX_SITES} sites (the debug registers), not {}",
            sites.len()
        ));
    }
    Ok(sites)
}

/// Install the handler and the arming thread, once. `sink` receives the
/// reports; the first caller's is kept.
fn ensure_running(sink: fn(&str)) -> Result<(), TraceError> {
    let _ = SINK.set(sink);
    RUNNING
        .get_or_init(|| {
            if unsafe { AddVectoredExceptionHandler(1, on_exception) }.is_null() {
                return Err("no vectored exception handler".into());
            }
            let thread = unsafe {
                win::CreateThread(
                    core::ptr::null_mut(),
                    0,
                    arm_forever,
                    core::ptr::null_mut(),
                    0,
                    core::ptr::null_mut(),
                )
            };
            if thread.is_null() {
                return Err("no arming thread".into());
            }
            unsafe { win::CloseHandle(thread) };
            Ok(())
        })
        .clone()
        .map_err(TraceError::Unavailable)
}

/// Trace `address` for `hits` hits under `label`, reporting through `sink`
/// (the first sink given is kept). Every other thread is armed before this
/// returns; this one, and threads started later, within two seconds.
pub fn add(address: usize, hits: u32, label: &str, sink: fn(&str)) -> Result<(), TraceError> {
    if address == 0 || hits == 0 || !win::is_executable(address) {
        return Err(TraceError::Invalid);
    }
    ensure_running(sink)?;
    if SITES.iter().any(|s| s.load(Ordering::Acquire) == address) {
        return Err(TraceError::Duplicate);
    }
    let slot = CLAIMED
        .iter()
        .position(|claimed| {
            claimed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        })
        .ok_or(TraceError::Full)?;
    HITS[slot].store(0, Ordering::Relaxed);
    LIMITS[slot].store(hits, Ordering::Relaxed);
    LABELS[slot].store(
        Box::into_raw(Box::new(label.to_string())),
        Ordering::Release,
    );
    SITES[slot].store(address, Ordering::Release);
    arm_all();
    Ok(())
}

/// Stop tracing the slot holding `address`. Nothing is allocated or locked, so
/// the handler calls it when a site has logged its hits.
fn release_slot(slot: usize, address: usize) -> bool {
    if SITES[slot]
        .compare_exchange(address, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    let next = RETIRED_NEXT.fetch_add(1, Ordering::Relaxed) % RETIRED_SLOTS;
    RETIRED[next].store(address, Ordering::Release);
    CLAIMED[slot].store(false, Ordering::Release);
    true
}

/// Stop tracing `address` before its hits are logged. The helper clears the
/// debug registers within two seconds; hits until then resume silently.
pub fn release(address: usize) -> bool {
    address != 0 && (0..MAX_SITES).any(|slot| release_slot(slot, address))
}

/// Arm `addresses` (with their labels) from `[trace] sites`, logging `hits`
/// hits each through `sink`.
pub fn start(addresses: &[(usize, String)], hits: u32, sink: fn(&str)) -> Result<(), String> {
    if addresses.is_empty() || addresses.len() > MAX_SITES {
        return Err(format!("1 to {MAX_SITES} sites required"));
    }
    for (address, label) in addresses {
        add(*address, hits, label, sink).map_err(|e| format!("{label}: {e}"))?;
    }
    Ok(())
}

/// `[trace] when = mission`: the sites to arm once a mission loads.
static DEFERRED: std::sync::Mutex<Option<(Vec<(usize, String)>, u32)>> =
    std::sync::Mutex::new(None);

/// Keep `addresses` to arm when the first mission loads ([`start_deferred`]).
pub fn defer(addresses: Vec<(usize, String)>, hits: u32) {
    *DEFERRED.lock().unwrap() = Some((addresses, hits));
}

/// Arm the deferred sites, once. Called as a mission's state is created.
pub fn start_deferred() {
    let Some((addresses, hits)) = DEFERRED.lock().unwrap().take() else {
        return;
    };
    match start(&addresses, hits, crate::log::info) {
        Ok(()) => crate::log::info("trace: armed now that a mission is loading"),
        Err(e) => crate::log::warn(&format!("trace: {e}; not tracing")),
    }
}

unsafe extern "C" fn service_trace(address: usize, hits: u32, label: *const c_char) -> i32 {
    let Some(label) = crate::crash::service_label(label) else {
        return 1;
    };
    if !(1..=1000).contains(&hits) {
        return 1;
    }
    match add(address, hits, &label, crate::log::info) {
        Ok(()) => {
            crate::log::info(&format!(
                "trace: {label} at {} armed by a plugin, {hits} hits",
                describe(address)
            ));
            0
        }
        Err(TraceError::Invalid) => 1,
        Err(TraceError::Full) => 2,
        Err(TraceError::Duplicate) => 3,
        Err(TraceError::Unavailable(reason)) => {
            crate::log::warn(&format!("trace: {reason}"));
            4
        }
    }
}

unsafe extern "C" fn service_stop(address: usize) -> i32 {
    if release(address) {
        0
    } else {
        1
    }
}

/// `defiance.loader` / `trace` v1: [`add`] and [`release`] for plugins.
pub static API: defiance_api::TraceV1 = defiance_api::TraceV1 {
    trace: service_trace,
    stop: service_stop,
};

unsafe extern "system" fn arm_forever(_: *mut c_void) -> u32 {
    loop {
        arm_all();
        unsafe { win::Sleep(2000) };
    }
}

/// Arm every other thread of this process once.
fn arm_all() -> usize {
    let own = unsafe { win::GetCurrentThreadId() };
    let process = unsafe { GetCurrentProcessId() };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot.is_null() || snapshot as isize == -1 {
        return 0;
    }
    let mut entry = ThreadEntry {
        size: core::mem::size_of::<ThreadEntry>() as u32,
        usage: 0,
        thread: 0,
        process: 0,
        base_priority: 0,
        delta_priority: 0,
        flags: 0,
    };
    let mut ids = Vec::new();
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while more {
        if entry.process == process && entry.thread != own {
            ids.push(entry.thread);
        }
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe { win::CloseHandle(snapshot) };
    ids.into_iter().filter(|&id| arm(id)).count()
}

/// Set the debug registers of one thread. Nothing is allocated while it is
/// suspended.
fn arm(id: u32) -> bool {
    let access = win::THREAD_SUSPEND_RESUME | win::THREAD_GET_CONTEXT | THREAD_SET_CONTEXT;
    let thread = unsafe { OpenThread(access, 0, id) };
    if thread.is_null() {
        return false;
    }
    let mut context = Context::new();
    context.0[FLAGS..FLAGS + 4].copy_from_slice(&CONTEXT_DEBUG_REGISTERS.to_le_bytes());
    let mut control = 0u64;
    let addresses: Vec<usize> = SITES.iter().map(|s| s.load(Ordering::Acquire)).collect();
    for (i, &address) in addresses.iter().enumerate() {
        if address != 0 {
            control |= 1 << (2 * i); // local enable; execute, one byte
        }
    }
    let armed = unsafe {
        if win::SuspendThread(thread) == u32::MAX {
            false
        } else {
            let ok = win::GetThreadContext(thread, context.0.as_mut_ptr().cast()) != 0 && {
                for (i, &address) in addresses.iter().enumerate() {
                    context.set_u64(DR0 + 8 * i, address as u64);
                }
                context.set_u64(DR7, control);
                SetThreadContext(thread, context.0.as_ptr().cast()) != 0
            };
            win::ResumeThread(thread);
            ok
        }
    };
    unsafe { win::CloseHandle(thread) };
    armed
}

unsafe extern "system" fn on_exception(pointers: *mut Pointers) -> i32 {
    let (record, context) = unsafe { ((*pointers).record, (*pointers).context) };
    if unsafe { *record } != EXCEPTION_SINGLE_STEP {
        return CONTINUE_SEARCH;
    }
    let context = unsafe { &mut *(context as *mut Context) };
    let rip = context.u64(RIP) as usize;
    if rip == 0 {
        return CONTINUE_SEARCH;
    }
    match SITES.iter().position(|s| s.load(Ordering::Acquire) == rip) {
        Some(index) => {
            let hit = HITS[index].fetch_add(1, Ordering::Relaxed) + 1;
            let limit = LIMITS[index].load(Ordering::Relaxed);
            if hit <= limit {
                report(index, hit, context);
            }
            if hit >= limit {
                release_slot(index, rip);
            }
        }
        // Released, but still armed in this thread until the helper clears it.
        None if RETIRED.iter().any(|r| r.load(Ordering::Acquire) == rip) => {}
        None => return CONTINUE_SEARCH,
    }
    // Step over the breakpoint once, and leave DR6 clean for the next one.
    let eflags = u32::from_le_bytes(context.0[EFLAGS..EFLAGS + 4].try_into().unwrap());
    context.0[EFLAGS..EFLAGS + 4].copy_from_slice(&(eflags | RESUME_FLAG).to_le_bytes());
    context.set_u64(DR6, 0);
    CONTINUE_EXECUTION
}

fn report(index: usize, hit: u32, context: &Context) {
    let label = LABELS[index].load(Ordering::Acquire);
    // Labels are leaked, never freed, so this one is valid even if the slot
    // has since been reused.
    let label = unsafe { label.as_ref() }.map_or("?", String::as_str);
    let frames = walk(context, 32);
    // Each register, and what it points at when that is module data or code:
    // for an object, its vtable, which names the class through RTTI.
    let registers = [
        ("rax", RAX),
        ("rcx", RCX),
        ("rdx", RDX),
        ("r8", R8),
        ("r9", R9),
    ]
    .iter()
    .map(|&(name, at)| {
        let value = context.u64(at) as usize;
        let read = |address: usize| {
            (address > 0x10000 && win::is_readable(address, 8))
                .then(|| unsafe { *(address as *const usize) })
        };
        // One level for an object, two for a pointer to one.
        match read(value) {
            Some(first) if in_module(first) => format!("{name}={value:#x}(*{})", describe(first)),
            Some(first) => match read(first).filter(|&second| in_module(second)) {
                Some(second) => format!("{name}={value:#x}(**{})", describe(second)),
                None => format!("{name}={value:#x}"),
            },
            None => format!("{name}={value:#x}"),
        }
    })
    .collect::<Vec<_>>()
    .join(" ");
    let text = format!(
        "trace {label} hit {hit}: {registers} stack {}",
        frames
            .iter()
            .map(|&(pc, scanned)| format!("{}{}", if scanned { "?" } else { "" }, describe(pc)))
            .collect::<Vec<_>>()
            .join(" < ")
    );
    if let Some(sink) = SINK.get() {
        sink(&text);
    }
}

/// A frame: its code address, and whether it was recovered by scanning the
/// stack rather than unwound.
type Frame = (usize, bool);

/// Frames from the context outward, the first being the site.
fn walk(start: &Context, limit: usize) -> Vec<Frame> {
    const RSP: usize = 0x98;
    let mut context = Context::new();
    context.0.copy_from_slice(&start.0);
    let mut frames: Vec<Frame> = Vec::new();
    let mut scanned = false;
    // Dropped frames add none, so bound the steps too.
    let mut steps = 0;
    while frames.len() < limit && steps < 4 * limit {
        steps += 1;
        let pc = context.u64(RIP);
        if pc == 0 {
            break;
        }
        if !frames.is_empty() && !win::is_executable(pc as usize) {
            // Unwinding from a scanned frame found data, not code: that frame
            // was a stale return address. Scan on from here instead.
            let rsp = context.u64(0x98) as usize;
            let Some(slot) = scan(rsp) else { break };
            context.set_u64(RIP, unsafe { *(slot as *const u64) });
            context.set_u64(0x98, slot as u64 + 8);
            scanned = true;
            continue;
        }
        frames.push((pc as usize, scanned));
        scanned = false;
        let mut base = 0u64;
        let function = unsafe { RtlLookupFunctionEntry(pc, &mut base, core::ptr::null_mut()) };
        let rsp = context.u64(RSP) as usize;
        if function.is_null() && frames.len() == 1 {
            // The site is a function's first instruction: its return address
            // is on top of the stack, whatever unwind data it has.
            if !win::is_readable(rsp, 8) {
                break;
            }
            context.set_u64(RIP, unsafe { *(rsp as *const u64) });
            context.set_u64(RSP, rsp as u64 + 8);
        } else if function.is_null() {
            // Code without unwind data (a patch payload): find the next
            // return address on the stack instead.
            let Some(slot) = scan(rsp) else { break };
            context.set_u64(RIP, unsafe { *(slot as *const u64) });
            context.set_u64(RSP, slot as u64 + 8);
            scanned = true;
        } else {
            let control = if frames.len() == 1 {
                unwind_pc(pc as usize, base as usize, function) as u64
            } else {
                pc
            };
            let mut data = core::ptr::null_mut();
            let mut establisher = 0u64;
            unsafe {
                RtlVirtualUnwind(
                    0,
                    base,
                    control,
                    function,
                    &mut *context,
                    &mut data,
                    &mut establisher,
                    core::ptr::null_mut(),
                )
            };
        }
    }
    frames
}

/// The length of the jump a hook writes at `code`'s start (`jmp rel32`, `jmp
/// rel8`, or the loader's `jmp [rip+0]` with its address after it), if it is one.
fn hook_jump(code: &[u8]) -> Option<usize> {
    match code {
        [0xe9, ..] => Some(5),
        [0xeb, ..] => Some(2),
        [0xff, 0x25, 0, 0, 0, 0, ..] => Some(14),
        [0xff, 0x25, ..] => Some(6),
        _ => None,
    }
}

/// Where to unwind the site's frame from. The unwinder takes a jump out of
/// the function at the pc for a tail-call epilogue and reads the frame as
/// already torn down, and a traced site that another patch owns starts with
/// just such a jump. Past the prolog, the instruction after the jump has the
/// same frame (a hook displaces no stack or nonvolatile change), so unwind
/// from there. At the function's entry, or when the jump ends the function,
/// the pc stands.
fn unwind_pc(pc: usize, base: usize, function: *mut c_void) -> usize {
    // RUNTIME_FUNCTION: begin, end, unwind info; the info's byte 1 is the
    // prolog size.
    let entry = function as *const u32;
    let (begin, end, info) = unsafe {
        (
            base + *entry as usize,
            base + *entry.add(1) as usize,
            base + *entry.add(2) as usize,
        )
    };
    if !win::is_readable(pc, 14) || !win::is_readable(info, 2) {
        return pc;
    }
    let prolog = unsafe { *((info + 1) as *const u8) } as usize;
    let code = unsafe { core::slice::from_raw_parts(pc as *const u8, 14) };
    match hook_jump(code) {
        Some(length) if pc - begin >= prolog && pc + length < end => pc + length,
        _ => pc,
    }
}

/// The first stack slot at or above `rsp` (within 512 bytes) holding a return
/// address into a loaded module: just after a direct or indirect call.
fn scan(rsp: usize) -> Option<usize> {
    (0..64)
        .map(|i| rsp + 8 * i)
        .take_while(|&slot| win::is_readable(slot, 8))
        .find(|&slot| {
            let value = unsafe { *(slot as *const usize) };
            let mut module = core::ptr::null_mut();
            value > 8
                && unsafe { win::GetModuleHandleExW(0x4 | 0x2, value as *const u16, &mut module) }
                    != 0
                && win::is_readable(value - 7, 7)
                && after_call(unsafe { core::slice::from_raw_parts((value - 7) as *const u8, 7) })
        })
}

/// Whether the seven bytes before a return address end in a call: `e8 rel32`,
/// or `ff /2` with a register, disp8, disp32 or RIP-relative operand.
fn after_call(before: &[u8]) -> bool {
    if before[2] == 0xe8 {
        return true;
    }
    (1..=5).any(|length| {
        let at = 7 - length - 1;
        let modrm = before[at + 1];
        before[at] == 0xff && (modrm >> 3) & 7 == 2 && {
            let operand = match modrm >> 6 {
                3 => 1,
                1 => 2 + usize::from(modrm & 7 == 4),
                2 => 5 + usize::from(modrm & 7 == 4),
                _ if modrm & 7 == 5 => 5,
                _ => 1 + usize::from(modrm & 7 == 4),
            };
            operand == length
        }
    })
}

/// Whether an address lies in a loaded module's image.
fn in_module(address: usize) -> bool {
    let mut module = core::ptr::null_mut();
    address > 0x10000
        && unsafe { win::GetModuleHandleExW(0x4 | 0x2, address as *const u16, &mut module) } != 0
}

/// `module+0xrva` for a code address, `label+0xoffset` for one in a crash
/// range (a hook's trampoline, Core's payload blocks), or the bare address.
fn describe(address: usize) -> String {
    let mut module = core::ptr::null_mut();
    // FROM_ADDRESS | UNCHANGED_REFCOUNT
    if unsafe { win::GetModuleHandleExW(0x4 | 0x2, address as *const u16, &mut module) } == 0 {
        return match crate::crash::range_of(address) {
            Some((start, label)) => format!("{label}+{:#x}", address - start),
            None => format!("{address:#x}"),
        };
    }
    let mut name = [0u16; 260];
    let length = unsafe { win::GetModuleFileNameW(module, name.as_mut_ptr(), 260) } as usize;
    let path = win::from_wide(&name[..length]);
    let file = path.rsplit(['\\', '/']).next().unwrap_or("?").to_string();
    format!("{file}+{:#x}", address - module as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn sites_parse_with_or_without_dll_and_0x() {
        assert_eq!(
            parse(" logic+0x42a940, game.dll+366FA7 ,").unwrap(),
            vec![
                Site {
                    module: "logic.dll".into(),
                    rva: 0x42a940
                },
                Site {
                    module: "game.dll".into(),
                    rva: 0x366fa7
                }
            ]
        );
        assert_eq!(parse("").unwrap(), vec![]);
        assert!(parse("logic").is_err());
        assert!(parse("logic+xyz").is_err());
        assert!(parse("../x+10").is_err());
        assert!(parse("a+1,b+2,c+3,d+4,e+5").is_err());
    }

    #[test]
    fn frames_in_a_crash_range_are_named_by_it() {
        let block = vec![0u8; 64];
        let start = block.as_ptr() as usize;
        crate::crash::map(start, start + 64, "test.payload");
        assert_eq!(describe(start + 0x24), "test.payload+0x24");
        crate::crash::unmap(start);
        assert_eq!(describe(start + 0x24), format!("{:#x}", start + 0x24));
    }

    /// The walk through a real hooked site: the preview builder's part loop in
    /// GOG 2026-09-14 logic.dll (fn_2108f0, frame 0xcc8 plus eight pushes),
    /// whose first instruction is dim_part's jump. Needs the local game file;
    /// skipped without it.
    #[test]
    fn a_hooked_site_in_logic_unwinds_to_its_caller() {
        #[link(name = "kernel32")]
        extern "system" {
            fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../bin/gog/2026-09-14/logic.dll");
        if !path.is_file() {
            eprintln!("skipped: no {}", path.display());
            return;
        }
        let wide = win::wide(&path.to_string_lossy());
        // DONT_RESOLVE_DLL_REFERENCES: mapped with its function table, never run.
        let base = unsafe { LoadLibraryExW(wide.as_ptr(), core::ptr::null_mut(), 1) } as usize;
        assert_ne!(base, 0, "could not map {}", path.display());
        let site = base + 0x211760;
        let caller = base + 0x210000; // any code address in the module
        let mut old = 0u32;
        unsafe {
            assert_ne!(
                win::VirtualProtect(site as *mut c_void, 5, 0x40, &mut old),
                0
            );
            core::ptr::copy_nonoverlapping([0xe9u8, 0, 0, 0, 0x10].as_ptr(), site as *mut u8, 5);
        }
        // The frame at the site: 0xcc8 of locals, then the eight pushes, then
        // the return address.
        let stack = vec![0u64; 0x400];
        let rsp = stack.as_ptr() as usize;
        let slot = rsp + 0xcc8 + 8 * 8;
        assert!(slot + 8 <= rsp + stack.len() * 8);
        unsafe { *(slot as *mut u64) = caller as u64 };
        let mut context = Context::new();
        context.set_u64(RIP, site as u64);
        context.set_u64(0x98, rsp as u64);
        let frames = walk(&context, 3);
        assert!(frames.len() >= 2, "{frames:x?}");
        assert_eq!(frames[1], (caller, false), "{frames:x?}");
    }

    #[test]
    fn hook_jumps_are_recognized() {
        assert_eq!(hook_jump(&[0xe9, 1, 2, 3, 4]), Some(5));
        assert_eq!(hook_jump(&[0xeb, 0x10]), Some(2));
        assert_eq!(hook_jump(&[0xff, 0x25, 0, 0, 0, 0, 1, 2]), Some(14));
        assert_eq!(hook_jump(&[0xff, 0x25, 8, 0, 0, 0]), Some(6));
        assert_eq!(hook_jump(&[0x48, 0x8b, 0x0e]), None);
        assert_eq!(hook_jump(&[0xc3]), None);
    }

    #[test]
    fn a_hooked_site_unwinds_from_after_its_jump() {
        // A fake RUNTIME_FUNCTION over a buffer: prolog 4 bytes, function 64.
        let mut image = vec![0x90u8; 0x100];
        let base = image.as_mut_ptr() as usize;
        image[0x81] = 4; // unwind info at 0x80, prolog size in byte 1
        image[0x10] = 0xe9; // a hook's jmp past the prolog
        image[0x02] = 0xe9; // a jmp inside the prolog
        image[0x3c] = 0xe9; // a jmp that ends the function
        let entry = [0u32, 0x40, 0x80];
        let function = entry.as_ptr() as *mut c_void;
        assert_eq!(unwind_pc(base + 0x10, base, function), base + 0x15);
        assert_eq!(unwind_pc(base + 0x02, base, function), base + 0x02);
        assert_eq!(unwind_pc(base + 0x3c, base, function), base + 0x3c);
        assert_eq!(unwind_pc(base + 0x20, base, function), base + 0x20);
    }

    #[test]
    fn calls_are_recognized_before_a_return_address() {
        let pad = |tail: &[u8]| {
            let mut bytes = vec![0x90; 7 - tail.len()];
            bytes.extend_from_slice(tail);
            bytes
        };
        assert!(after_call(&pad(&[0xe8, 1, 2, 3, 4]))); // call rel32
        assert!(after_call(&pad(&[0xff, 0xd0]))); // call rax
        assert!(after_call(&pad(&[0x41, 0xff, 0xd3]))); // call r11
        assert!(after_call(&pad(&[0xff, 0x50, 0x68]))); // call [rax+0x68]
        assert!(after_call(&pad(&[0xff, 0x90, 0x98, 0, 0, 0]))); // call [rax+0x98]
        assert!(after_call(&pad(&[0xff, 0x15, 1, 2, 3, 4]))); // call [rip+x]
        assert!(!after_call(&pad(&[0xff, 0xe0]))); // jmp rax
        assert!(!after_call(&pad(&[0x48, 0x89, 0xc1]))); // mov rcx, rax
    }

    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    fn collect(text: &str) {
        SEEN.lock().unwrap().push(text.to_string());
    }

    #[inline(never)]
    extern "C" fn traced(value: u64) -> u64 {
        std::hint::black_box(value) * 3
    }

    #[inline(never)]
    extern "C" fn untraced(value: u64) -> u64 {
        std::hint::black_box(value) + 1
    }

    /// One test, since the slots are process-wide.
    #[test]
    fn a_hit_logs_its_stack_and_resumes() {
        let address = traced as extern "C" fn(u64) -> u64 as usize;
        start(&[(address, "traced".into())], 2, collect).unwrap();
        assert_eq!(
            add(address, 1, "again", collect),
            Err(TraceError::Duplicate)
        );
        assert_eq!(add(0x10, 1, "data", collect), Err(TraceError::Invalid));
        // This thread is armed by the helper: call until it is.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut result = 0;
        while SEEN.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            result = traced(std::hint::black_box(7));
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(result, 21, "the traced function still runs");
        for _ in 0..5 {
            traced(1);
        }
        let seen = SEEN.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "logged no more than `hits` times: {seen:?}");
        assert!(
            SITES.iter().all(|s| s.load(Ordering::Acquire) != address),
            "a site that has logged its hits is released"
        );
        assert!(
            seen[0].starts_with("trace traced hit 1: rax=") && seen[0].contains(" rcx=0x7 "),
            "{}",
            seen[0]
        );
        assert!(
            seen[0].contains("stack ") && seen[0].contains(" < "),
            "{}",
            seen[0]
        );

        // The service: its codes, a full set of slots, and an early stop.
        let other = untraced as extern "C" fn(u64) -> u64 as usize;
        let label = c"service".as_ptr();
        assert_eq!(unsafe { service_trace(other, 0, label) }, 1);
        assert_eq!(unsafe { service_trace(other, 5, core::ptr::null()) }, 1);
        assert_eq!(unsafe { service_trace(other, 5, label) }, 0);
        assert_eq!(unsafe { service_trace(other, 5, label) }, 3);
        let spare: Vec<usize> = (1..MAX_SITES).map(|i| other + i).collect();
        for &address in &spare {
            add(address, 1, "filler", collect).unwrap();
        }
        assert_eq!(unsafe { service_trace(address, 5, label) }, 2);
        assert_eq!(untraced(1), 2, "a traced function still runs");
        assert_eq!(unsafe { service_stop(other) }, 0);
        assert_eq!(unsafe { service_stop(other) }, 1);
        for &address in &spare {
            assert!(release(address));
        }
        // Released but perhaps still armed: hits resume silently.
        for _ in 0..3 {
            assert_eq!(untraced(1), 2);
            assert_eq!(traced(1), 3);
        }
    }
}
