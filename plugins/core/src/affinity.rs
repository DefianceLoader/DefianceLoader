//! Let the game's main thread run on more than the first CPU.
//!
//! The engine pins its main thread to CPU 0 while it sets up its timer:
//! `galileo.dll` calls `SetThreadAffinityMask(GetCurrentThread(), 1)` once
//! (found by [`PIN_PATTERN`]). That thread renders and simulates; on the
//! machine it was measured on, CPU 0 also serviced the graphics card's
//! interrupts.
//!
//! Core removes that call (a six-byte no-op in place of the indirect call), so
//! the engine never narrows the thread, and sets the thread's CPUs itself, at
//! full width, in the processor group the thread is in. The two together give
//! the same result whichever of Core and the engine's setup runs first: if the
//! pin already ran, Core's mask replaces it; if not, it never runs.
//!
//! `[loader] main_thread_cpus` chooses the mask ([`Mode`], [`choose_mask`]):
//! `all`, the default, every CPU the process may use in that group; `spread`
//! every CPU but the first core and its SMT siblings (the whole group when
//! fewer than [`MIN_SPREAD_CORES`] other cores would remain, so a small CPU
//! never has its main thread squeezed onto one core); `engine` the stock pin,
//! untouched. `spread` gave repeated 100–500 ms stalls in `Present` on the
//! machine it was tried on (2026-09-25), which `all` and `engine` did not, so
//! it is not the default. None of them changes what the thread computes, so
//! all are safe in multiplayer.
use core::ffi::c_void;
use defiance_api::{Api, LOG_INFO, LOG_WARN};

/// `call [GetCurrentThread]; mov edx, 1; mov rcx, rax; call
/// [SetThreadAffinityMask]`, in `galileo.dll`'s timer setup (GOG 2026-09-14
/// `galileo+0x1d880`, `fn_1d270`). Its result is not used.
pub const PIN_PATTERN: &str = "ff 15 ?? ?? ?? ?? ba 01 00 00 00 48 8b c8 ff 15 ?? ?? ?? ??";
/// Offset of the `SetThreadAffinityMask` call in [`PIN_PATTERN`].
const PIN_CALL_AT: usize = 14;
/// The six-byte no-op (`nop word ptr [rax + rax]`) that replaces that call.
const NOP6: [u8; 6] = [0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00];
/// With fewer cores than this left after leaving out the first, `spread` keeps
/// every CPU instead.
pub const MIN_SPREAD_CORES: u32 = 3;

/// `[loader] main_thread_cpus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Spread,
    All,
    Engine,
}

impl Mode {
    /// The setting's value; anything unknown (or an older loader that does
    /// not declare it) is the default, `all`.
    pub fn parse(text: &str) -> Mode {
        match text {
            "spread" => Mode::Spread,
            "engine" => Mode::Engine,
            _ => Mode::All,
        }
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn GetCurrentProcessId() -> u32;
    fn GetProcessAffinityMask(process: *mut c_void, mask: *mut usize, system: *mut usize) -> i32;
    fn GetActiveProcessorCount(group: u16) -> u32;
    fn GetLogicalProcessorInformationEx(relation: u32, buffer: *mut u8, length: *mut u32) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, process: u32) -> *mut c_void;
    fn Thread32First(snapshot: *mut c_void, entry: *mut ThreadEntry) -> i32;
    fn Thread32Next(snapshot: *mut c_void, entry: *mut ThreadEntry) -> i32;
    fn OpenThread(access: u32, inherit: i32, id: u32) -> *mut c_void;
    fn GetThreadTimes(
        thread: *mut c_void,
        creation: *mut u64,
        exit: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
    fn GetThreadGroupAffinity(thread: *mut c_void, affinity: *mut GroupAffinity) -> i32;
    fn SetThreadGroupAffinity(
        thread: *mut c_void,
        affinity: *const GroupAffinity,
        previous: *mut GroupAffinity,
    ) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

#[repr(C)]
struct ThreadEntry {
    size: u32,
    usage: u32,
    thread: u32,
    owner: u32,
    base_priority: i32,
    delta_priority: i32,
    flags: u32,
}

/// `GROUP_AFFINITY`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct GroupAffinity {
    mask: usize,
    group: u16,
    reserved: [u16; 3],
}

const RELATION_PROCESSOR_CORE: u32 = 0;
const SNAPSHOT_THREAD: u32 = 0x4;
const INVALID_HANDLE: *mut c_void = usize::MAX as *mut c_void;
/// THREAD_SET_INFORMATION | THREAD_QUERY_INFORMATION | THREAD_QUERY_LIMITED_INFORMATION
const THREAD_ACCESS: u32 = 0x20 | 0x40 | 0x800;

/// The main thread's mask within one processor group. `available` is every
/// CPU the thread may use there; `cores` holds that group's physical cores,
/// one mask each (SMT siblings together). `spread` leaves out the core holding
/// the lowest available CPU when at least [`MIN_SPREAD_CORES`] cores remain,
/// and otherwise, like `all`, gives `available`. `engine` is not a mask.
pub fn choose_mask(mode: Mode, available: u64, cores: &[u64]) -> u64 {
    if available == 0 || mode == Mode::All {
        return available;
    }
    let first = available & available.wrapping_neg();
    let first_core = cores
        .iter()
        .copied()
        .find(|core| core & first != 0)
        .unwrap_or(first);
    let rest = available & !first_core;
    let remaining = cores.iter().filter(|core| *core & rest != 0).count() as u32;
    if remaining >= MIN_SPREAD_CORES {
        rest
    } else {
        available
    }
}

/// Every active CPU of `group`, as a mask.
fn group_mask(group: u16) -> u64 {
    match unsafe { GetActiveProcessorCount(group) } {
        0 => 0,
        n if n >= 64 => u64::MAX,
        n => (1u64 << n) - 1,
    }
}

/// The CPUs the main thread may use in `group`: the process's mask when the
/// process lives in one group (the normal case), else every active CPU of the
/// group, which Windows lets a thread of a multi-group process use.
fn available(group: u16) -> u64 {
    let (mut process, mut system) = (0usize, 0usize);
    let known = unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) };
    // Zero masks mean the process has threads in several groups.
    if known != 0 && process != 0 {
        process as u64 & group_mask(group)
    } else {
        group_mask(group)
    }
}

/// One mask per physical core of `group`, or empty on failure.
fn core_masks(group: u16) -> Vec<u64> {
    let mut length = 0u32;
    unsafe {
        GetLogicalProcessorInformationEx(
            RELATION_PROCESSOR_CORE,
            core::ptr::null_mut(),
            &mut length,
        )
    };
    if length == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u8; length as usize];
    if unsafe {
        GetLogicalProcessorInformationEx(RELATION_PROCESSOR_CORE, buffer.as_mut_ptr(), &mut length)
    } == 0
    {
        return Vec::new();
    }
    parse_cores(&buffer[..length as usize], group)
}

/// The masks of `group`'s cores in a `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX`
/// list of processor cores: each record is `Relationship` (u32), `Size` (u32),
/// then a `PROCESSOR_RELATIONSHIP` whose `GroupCount` (u16) is at +30 and whose
/// `GROUP_AFFINITY` entries (`Mask` u64, `Group` u16, 6 reserved) start at +32.
fn parse_cores(buffer: &[u8], group: u16) -> Vec<u64> {
    let u16_at = |at: usize| {
        buffer
            .get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let u32_at = |at: usize| {
        buffer
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let u64_at = |at: usize| {
        buffer
            .get(at..at + 8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    };
    let mut cores = Vec::new();
    let mut at = 0;
    while let (Some(relation), Some(size)) = (u32_at(at), u32_at(at + 4)) {
        if size == 0 {
            break;
        }
        if relation == RELATION_PROCESSOR_CORE {
            let groups = u16_at(at + 30).unwrap_or(0) as usize;
            for g in 0..groups {
                let entry = at + 32 + g * 16;
                if let (Some(mask), Some(owner)) = (u64_at(entry), u16_at(entry + 8)) {
                    if owner == group {
                        cores.push(mask);
                    }
                }
            }
        }
        at += size as usize;
    }
    cores
}

/// The process's first thread: the one Windows started it on, which runs the
/// game's main loop. Its handle carries [`THREAD_ACCESS`].
fn main_thread() -> Option<(u32, *mut c_void)> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(SNAPSHOT_THREAD, 0) };
    if snapshot.is_null() || snapshot == INVALID_HANDLE {
        return None;
    }
    let own = unsafe { GetCurrentProcessId() };
    let mut entry = ThreadEntry {
        size: core::mem::size_of::<ThreadEntry>() as u32,
        usage: 0,
        thread: 0,
        owner: 0,
        base_priority: 0,
        delta_priority: 0,
        flags: 0,
    };
    let mut best: Option<(u64, u32, *mut c_void)> = None;
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while more {
        if entry.owner == own {
            let handle = unsafe { OpenThread(THREAD_ACCESS, 0, entry.thread) };
            if !handle.is_null() {
                let (mut created, mut unused) = (0u64, [0u64; 3]);
                let known = unsafe {
                    GetThreadTimes(
                        handle,
                        &mut created,
                        &mut unused[0],
                        &mut unused[1],
                        &mut unused[2],
                    )
                } != 0;
                if known && best.is_none_or(|(earliest, _, _)| created < earliest) {
                    if let Some((_, _, old)) = best {
                        unsafe { CloseHandle(old) };
                    }
                    best = Some((created, entry.thread, handle));
                } else {
                    unsafe { CloseHandle(handle) };
                }
            }
        }
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    best.map(|(_, id, handle)| (id, handle))
}

/// The absolute target of the `call [rip+disp32]` at `site`.
unsafe fn indirect_target(site: *const u8) -> usize {
    let disp = unsafe { core::ptr::read_unaligned(site.add(2) as *const i32) };
    let slot = (site as isize + 6 + disp as isize) as *const usize;
    unsafe { core::ptr::read_unaligned(slot) }
}

fn kernel32(name: &[u8]) -> usize {
    let module: Vec<u16> = "kernel32.dll"
        .encode_utf16()
        .chain(core::iter::once(0))
        .collect();
    let handle = unsafe { GetModuleHandleW(module.as_ptr()) };
    if handle.is_null() {
        return 0;
    }
    unsafe { GetProcAddress(handle, name.as_ptr()) as usize }
}

/// Replace the engine's `SetThreadAffinityMask` call with a no-op, after
/// checking that the matched code calls what [`PIN_PATTERN`] says it does.
fn remove_pin(api: &Api) -> Result<(), String> {
    let base = unsafe { (api.module_base)(c"galileo.dll".as_ptr()) };
    if base.is_null() {
        return Err("galileo.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base) };
    let pattern = std::ffi::CString::new(PIN_PATTERN).unwrap_or_default();
    let site = unsafe { (api.find_pattern)(base, size, pattern.as_ptr()) } as *mut u8;
    if site.is_null() {
        return Err("the engine's pin was not found".into());
    }
    let (current, pin) = (
        kernel32(b"GetCurrentThread\0"),
        kernel32(b"SetThreadAffinityMask\0"),
    );
    let call = unsafe { site.add(PIN_CALL_AT) };
    if current == 0
        || pin == 0
        || unsafe { indirect_target(site) } != current
        || unsafe { indirect_target(call) } != pin
    {
        return Err(
            "the matched code does not call GetCurrentThread and SetThreadAffinityMask".into(),
        );
    }
    let mut before = [0u8; 6];
    unsafe { core::ptr::copy_nonoverlapping(call, before.as_mut_ptr(), before.len()) };
    match unsafe {
        (api.patch_bytes)(
            call as *mut c_void,
            before.as_ptr(),
            NOP6.as_ptr(),
            NOP6.len(),
        )
    } {
        0 => Ok(()),
        _ => Err("the engine's pin could not be removed".into()),
    }
}

/// Give the main thread `mode`'s CPUs in its own processor group. The group
/// and the mask applied, or why not.
fn apply(mode: Mode) -> Result<(u16, u64), String> {
    let (_, handle) = main_thread().ok_or("the main thread was not found")?;
    let result = (|| {
        let mut current = GroupAffinity::default();
        if unsafe { GetThreadGroupAffinity(handle, &mut current) } == 0 {
            return Err("its processor group is unknown".to_string());
        }
        let group = current.group;
        let mask = choose_mask(mode, available(group), &core_masks(group));
        if mask == 0 {
            return Err(format!("no CPU is available in group {group}"));
        }
        let wanted = GroupAffinity {
            mask: mask as usize,
            group,
            reserved: [0; 3],
        };
        if unsafe { SetThreadGroupAffinity(handle, &wanted, core::ptr::null_mut()) } == 0 {
            return Err(format!("CPUs {mask:#x} in group {group} were refused"));
        }
        Ok((group, mask))
    })();
    unsafe { CloseHandle(handle) };
    result
}

pub fn install(api: &Api, setting: &str) {
    let say = |level, text: &str| super::say(api, level, &format!("main thread: {text}"));
    let mode = Mode::parse(setting);
    if mode == Mode::Engine {
        say(
            LOG_INFO,
            "left on the engine's CPU 0 (main_thread_cpus = engine)",
        );
        return;
    }
    let removed = remove_pin(api);
    match (apply(mode), removed) {
        (Ok((group, mask)), Ok(())) => say(
            LOG_INFO,
            &format!("CPUs {mask:#x} in group {group} (the engine's pin is removed)"),
        ),
        (Ok((group, mask)), Err(e)) => say(
            LOG_WARN,
            &format!(
                "CPUs {mask:#x} in group {group} for now, but {e}; if the engine pins it later it stays on CPU 0"
            ),
        ),
        (Err(e), Ok(())) => say(
            LOG_WARN,
            &format!("the engine's pin is removed, but {e}; if it already ran the thread stays on CPU 0"),
        ),
        (Err(e), Err(pin)) => say(LOG_WARN, &format!("{e}, and {pin}; left on CPU 0")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cores of `threads` logical CPUs each, numbered as Windows numbers SMT
    /// siblings: adjacently.
    fn cores(count: u32, threads: u32) -> Vec<u64> {
        let width = (1u64 << threads) - 1;
        (0..count).map(|c| width << (c * threads)).collect()
    }

    #[test]
    fn spread_leaves_out_the_first_core_and_its_sibling() {
        // 8 cores, 16 threads (the 7800X3D this was measured on).
        assert_eq!(choose_mask(Mode::Spread, 0xffff, &cores(8, 2)), 0xfffc);
        // 4 cores without SMT: three remain.
        assert_eq!(choose_mask(Mode::Spread, 0xf, &cores(4, 1)), 0xe);
        // 4 cores, 8 threads: three remain.
        assert_eq!(choose_mask(Mode::Spread, 0xff, &cores(4, 2)), 0xfc);
    }

    #[test]
    fn spread_keeps_every_cpu_of_a_small_processor() {
        assert_eq!(choose_mask(Mode::Spread, 0xf, &cores(2, 2)), 0xf);
        assert_eq!(choose_mask(Mode::Spread, 0x7, &cores(3, 1)), 0x7);
        assert_eq!(choose_mask(Mode::Spread, 0x3, &cores(1, 2)), 0x3);
        assert_eq!(choose_mask(Mode::Spread, 0x1, &cores(1, 1)), 0x1);
    }

    #[test]
    fn spread_follows_the_available_cpus() {
        // The player already keeps the game off CPU 0: the first core is then
        // the one holding CPU 1.
        assert_eq!(choose_mask(Mode::Spread, 0xfffe, &cores(8, 2)), 0xfffc);
        // Restricted to two cores: nothing is left out.
        assert_eq!(choose_mask(Mode::Spread, 0x0f00, &cores(8, 2)), 0x0f00);
    }

    #[test]
    fn spread_uses_the_full_width() {
        // 32 cores, 64 threads: CPUs above 31 stay in the mask.
        assert_eq!(
            choose_mask(Mode::Spread, u64::MAX, &cores(32, 2)),
            u64::MAX & !0x3
        );
    }

    #[test]
    fn all_is_every_available_cpu() {
        assert_eq!(choose_mask(Mode::All, 0xffff, &cores(8, 2)), 0xffff);
        assert_eq!(choose_mask(Mode::All, 0xfffe, &[]), 0xfffe);
    }

    #[test]
    fn unknown_topology_or_mask_is_never_narrowed() {
        assert_eq!(choose_mask(Mode::Spread, 0xffff, &[]), 0xffff);
        assert_eq!(choose_mask(Mode::Spread, 0, &cores(8, 2)), 0);
    }

    #[test]
    fn parses_the_setting() {
        assert_eq!(Mode::parse("spread"), Mode::Spread);
        assert_eq!(Mode::parse("all"), Mode::All);
        assert_eq!(Mode::parse("engine"), Mode::Engine);
        assert_eq!(Mode::parse(""), Mode::All);
    }

    #[test]
    fn parses_core_records_of_one_group() {
        // Group 0: two cores of two threads; group 1: one core; then a record
        // of another relation.
        let mut buffer = Vec::new();
        for (mask, group) in [(0x3u64, 0u16), (0xc, 0), (0x3, 1)] {
            let mut record = vec![0u8; 48];
            record[4..8].copy_from_slice(&48u32.to_le_bytes());
            record[30..32].copy_from_slice(&1u16.to_le_bytes());
            record[32..40].copy_from_slice(&mask.to_le_bytes());
            record[40..42].copy_from_slice(&group.to_le_bytes());
            buffer.extend(record);
        }
        let mut other = vec![0u8; 16];
        other[0..4].copy_from_slice(&2u32.to_le_bytes());
        other[4..8].copy_from_slice(&16u32.to_le_bytes());
        buffer.extend(other);
        assert_eq!(parse_cores(&buffer, 0), vec![0x3, 0xc]);
        assert_eq!(parse_cores(&buffer, 1), vec![0x3]);
    }

    #[test]
    fn the_pattern_ends_with_the_pin_call() {
        let tokens: Vec<&str> = PIN_PATTERN.split_whitespace().collect();
        assert_eq!(tokens.len(), PIN_CALL_AT + NOP6.len());
        assert_eq!(&tokens[PIN_CALL_AT..PIN_CALL_AT + 2], ["ff", "15"]);
        assert_eq!(&tokens[6..11], ["ba", "01", "00", "00", "00"]);
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn GetCurrentThread() -> *mut c_void;
    }

    #[test]
    fn reads_this_machines_cores() {
        let mut current = GroupAffinity::default();
        assert_ne!(
            unsafe { GetThreadGroupAffinity(GetCurrentThread(), &mut current) },
            0
        );
        let cores = core_masks(current.group);
        assert!(!cores.is_empty());
        let available = available(current.group);
        for mode in [Mode::Spread, Mode::All] {
            let mask = choose_mask(mode, available, &cores);
            assert!(mask != 0 && mask & !available == 0);
        }
    }

    #[test]
    fn applies_and_reads_back_the_main_threads_mask() {
        let (_, handle) = main_thread().expect("the process's threads");
        let mut original = GroupAffinity::default();
        assert_ne!(unsafe { GetThreadGroupAffinity(handle, &mut original) }, 0);
        for mode in [Mode::Spread, Mode::All] {
            let (group, mask) = apply(mode).expect("applied");
            let mut now = GroupAffinity::default();
            assert_ne!(unsafe { GetThreadGroupAffinity(handle, &mut now) }, 0);
            assert_eq!((now.group, now.mask as u64), (group, mask));
        }
        unsafe { SetThreadGroupAffinity(handle, &original, core::ptr::null_mut()) };
        unsafe { CloseHandle(handle) };
    }

    #[test]
    fn finds_the_first_thread_not_this_one() {
        // The test harness runs each test on a thread of its own.
        let (id, handle) = main_thread().expect("the process's threads");
        unsafe { CloseHandle(handle) };
        assert_ne!(id, unsafe { GetCurrentThreadId() });
    }
}
