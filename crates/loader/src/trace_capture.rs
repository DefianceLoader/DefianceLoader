//! Loader-owned, bounded hardware-breakpoint snapshots. The exception path
//! only try-locks and copies fixed-size values; it never calls plugin code or logs.

use crate::trace::{self, Context, MAX_SITES};
use crate::win;
use defiance_api::{
    TraceCaptureV1, TraceEventV1, TraceFieldV1, TraceRequestV1, TraceStatsV1, TRACE_F32, TRACE_F64,
    TRACE_FIELDS, TRACE_FRAMES, TRACE_NO_FILTER, TRACE_REGISTERS, TRACE_U16, TRACE_U32, TRACE_U64,
    TRACE_U8,
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex, OnceLock,
};

const QUEUE: usize = 128;
const SESSIONS: usize = 64;
const REG_OFFSETS: [usize; TRACE_REGISTERS] = [
    0x78, 0x80, 0x88, 0x90, 0x98, 0xa0, 0xa8, 0xb0, 0xb8, 0xc0, 0xc8, 0xd0, 0xd8, 0xe0, 0xe8, 0xf0,
    0xf8,
];

#[link(name = "kernel32")]
extern "system" {
    fn ReadProcessMemory(
        process: win::Handle,
        address: *const core::ffi::c_void,
        buffer: *mut core::ffi::c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;
    fn GetTickCount64() -> u64;
}

#[derive(Clone, Copy)]
struct Session {
    id: u64,
    owner: usize,
}
struct Capture {
    session: u64,
    handle: u64,
    generation: u32,
    address: usize,
    request: TraceRequestV1,
    active: bool,
    last_capture_ms: Option<u64>,
    queue: Vec<TraceEventV1>,
    head: usize,
    len: usize,
    captured: u64,
}
#[derive(Default)]
struct State {
    sessions: Vec<Session>,
    sites: [Option<Capture>; MAX_SITES],
}
#[derive(Default)]
struct Counters {
    hits: AtomicU64,
    dropped: AtomicU64,
}
static COUNTERS: [Counters; MAX_SITES] = [const {
    Counters {
        hits: AtomicU64::new(0),
        dropped: AtomicU64::new(0),
    }
}; MAX_SITES];
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
static NEXT_GENERATION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| Mutex::new(State::default()))
}
fn next_id(counter: &AtomicU64) -> u64 {
    loop {
        let id = counter.fetch_add(1, Ordering::Relaxed);
        if id != 0 {
            return id;
        }
    }
}
fn next_generation() -> Option<u32> {
    NEXT_GENERATION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |generation| {
            (generation < (1 << 31) - 1).then_some(generation + 1)
        })
        .ok()
}
fn packed(generation: u32, count: u32) -> u64 {
    (u64::from(generation) << 32) | u64::from(count)
}
fn count_for(counter: &AtomicU64, generation: u32) -> u32 {
    let value = counter.load(Ordering::Acquire);
    if (value >> 32) as u32 == generation {
        value as u32
    } else {
        0
    }
}
fn increment_for(counter: &AtomicU64, generation: u32) -> Option<u32> {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        if (current >> 32) as u32 != generation || current as u32 == u32::MAX {
            return None;
        }
        let next = packed(generation, current as u32 + 1);
        match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(next as u32),
            Err(observed) => current = observed,
        }
    }
}
fn increment_for_limit(counter: &AtomicU64, generation: u32, limit: u32) -> Option<u32> {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        let count = current as u32;
        if (current >> 32) as u32 != generation || count >= limit {
            return None;
        }
        let next = packed(generation, count + 1);
        match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(count + 1),
            Err(observed) => current = observed,
        }
    }
}

unsafe extern "C" fn service_open() -> u64 {
    let Some(owner) = crate::services::current_owner() else {
        return 0;
    };
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    if state.sessions.len() >= SESSIONS {
        return 0;
    }
    let id = next_id(&NEXT_SESSION);
    state.sessions.push(Session { id, owner });
    id
}

unsafe extern "C" fn service_start(
    session: u64,
    request: *const TraceRequestV1,
    handle: *mut u64,
) -> i32 {
    if request.is_null() || handle.is_null() || session == 0 {
        return 1;
    }
    let request = unsafe { *request };
    if !valid(&request) {
        return 1;
    }
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    if !state.sessions.iter().any(|s| s.id == session) {
        return 5;
    }
    let Some(generation) = next_generation() else {
        return 4;
    };
    let id = next_id(&NEXT_HANDLE);
    let result = trace::register_capture(request.address, generation, request.hits, |slot| {
        COUNTERS[slot]
            .hits
            .store(packed(generation, 0), Ordering::Release);
        COUNTERS[slot]
            .dropped
            .store(packed(generation, 0), Ordering::Release);
        state.sites[slot] = Some(Capture {
            session,
            handle: id,
            generation,
            address: request.address,
            request,
            active: true,
            last_capture_ms: None,
            queue: vec![TraceEventV1::default(); QUEUE],
            head: 0,
            len: 0,
            captured: 0,
        });
    });
    if let Err(error) = result {
        return match error {
            trace::TraceError::Invalid => 1,
            trace::TraceError::Full => 2,
            trace::TraceError::Duplicate => 3,
            trace::TraceError::Unavailable(_) => 4,
        };
    }
    unsafe { *handle = id };
    0
}

unsafe extern "C" fn service_poll(
    handle: u64,
    events: *mut TraceEventV1,
    capacity: usize,
    stats: *mut TraceStatsV1,
) -> i32 {
    if handle == 0 || (capacity != 0 && events.is_null()) {
        return -1;
    }
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    let Some(slot) = state
        .sites
        .iter()
        .position(|site| site.as_ref().is_some_and(|c| c.handle == handle))
    else {
        return -2;
    };
    let capture = state.sites[slot].as_mut().unwrap();
    let count = capacity.min(capture.len);
    for i in 0..count {
        unsafe { events.add(i).write(capture.queue[capture.head]) };
        capture.head = (capture.head + 1) % QUEUE;
        capture.len -= 1;
    }
    if !stats.is_null() {
        let hits = u64::from(count_for(&COUNTERS[slot].hits, capture.generation));
        unsafe {
            stats.write(TraceStatsV1 {
                hits,
                captured: capture.captured,
                dropped: u64::from(count_for(&COUNTERS[slot].dropped, capture.generation)),
                active: u32::from(capture.active && hits < u64::from(capture.request.hits)),
                reserved: 0,
            })
        };
    }
    count as i32
}

unsafe extern "C" fn service_stop(handle: u64) -> i32 {
    if handle == 0 {
        return 1;
    }
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    let Some(slot) = state
        .sites
        .iter()
        .position(|site| site.as_ref().is_some_and(|c| c.handle == handle))
    else {
        return 1;
    };
    let capture = state.sites[slot].as_ref().unwrap();
    let (address, generation) = (capture.address, capture.generation);
    if !trace::release_capture_slot(slot, address, generation) {
        return 1;
    }
    state.sites[slot] = None;
    0
}

unsafe extern "C" fn service_close(session: u64) -> i32 {
    if session == 0 {
        return 1;
    }
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    let Some(index) = state.sessions.iter().position(|s| s.id == session) else {
        return 1;
    };
    state.sessions.swap_remove(index);
    close_session(&mut state, session);
    0
}

/// Close every session/site belonging to an owner during rollback or unload.
pub(crate) fn close_owner(owner: usize) {
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    let mut sessions = Vec::new();
    state.sessions.retain(|s| {
        if s.owner == owner {
            sessions.push(s.id);
            false
        } else {
            true
        }
    });
    for session in sessions {
        close_session(&mut state, session);
    }
}
fn close_session(state: &mut State, session: u64) {
    for slot in 0..MAX_SITES {
        if state.sites[slot]
            .as_ref()
            .is_some_and(|c| c.session == session)
        {
            let capture = state.sites[slot].as_ref().unwrap();
            let (address, generation) = (capture.address, capture.generation);
            if trace::release_capture_slot(slot, address, generation) {
                state.sites[slot] = None;
            }
        }
    }
}

/// No allocation or blocking lock on the exception path. Contention is a drop.
#[cfg(test)]
fn on_hit(slot: usize, context: &Context) {
    on_hit_generation(
        slot,
        context,
        super::CAPTURE_GENERATIONS[slot].load(Ordering::Acquire),
    );
}

pub(super) fn on_hit_generation(slot: usize, context: &Context, generation: u32) {
    let address = context.u64(REG_OFFSETS[16]) as usize;
    if address == 0
        || generation == 0
        || generation & super::CAPTURE_RETIRING != 0
        || super::CAPTURE_GENERATIONS[slot].load(Ordering::Acquire) != generation
        || super::SITES[slot].load(Ordering::Acquire) != address
        || !super::CAPTURE_SLOTS[slot].load(Ordering::Acquire)
    {
        return;
    }
    let limit = super::CAPTURE_GENERATION_LIMITS[slot].load(Ordering::Acquire);
    if (limit >> 32) as u32 != generation {
        return;
    }
    let Some(hit) = increment_for_limit(&COUNTERS[slot].hits, generation, limit as u32) else {
        return;
    };
    if hit == limit as u32 {
        let _ = trace::retire_capture_site(slot, address, generation);
    }
    let Ok(mut state) = state().try_lock() else {
        let _ = increment_for(&COUNTERS[slot].dropped, generation);
        return;
    };
    let Some(capture) = state.sites[slot].as_mut() else {
        let _ = increment_for(&COUNTERS[slot].dropped, generation);
        return;
    };
    if capture.address != address || capture.generation != generation {
        return;
    }
    if hit == capture.request.hits {
        capture.active = false;
    }
    if (hit - 1) % capture.request.every != 0 {
        return;
    }
    if capture.len == QUEUE {
        let _ = increment_for(&COUNTERS[slot].dropped, generation);
        return;
    }
    let timestamp_ms = unsafe { GetTickCount64() };
    if capture.last_capture_ms.is_some_and(|last| {
        timestamp_ms.saturating_sub(last) < u64::from(capture.request.min_interval_ms)
    }) {
        return;
    }
    let mut event = TraceEventV1 {
        address: capture.address,
        sequence: u64::from(hit),
        timestamp_ms,
        thread_id: unsafe { win::GetCurrentThreadId() },
        ..TraceEventV1::default()
    };
    for (i, offset) in REG_OFFSETS.iter().enumerate() {
        event.registers[i] = context.u64(*offset);
    }
    for index in 0..capture.request.field_count as usize {
        if let Some(value) = unsafe { read_field(&capture.request.fields[index], &event.registers) }
        {
            event.values[index] = value;
            event.valid_fields |= 1 << index;
        }
    }
    let filter = capture.request.filter_field;
    if filter != TRACE_NO_FILTER {
        let bit = 1u32 << filter;
        if event.valid_fields & bit == 0
            || (event.values[filter as usize] & capture.request.filter_mask)
                != (capture.request.filter_value & capture.request.filter_mask)
        {
            return;
        }
    }
    capture_stack(context, capture.request.stack_frames as usize, &mut event);
    let tail = (capture.head + capture.len) % QUEUE;
    capture.queue[tail] = event;
    capture.len += 1;
    capture.last_capture_ms = Some(timestamp_ms);
    capture.captured += 1;
}

fn valid(request: &TraceRequestV1) -> bool {
    if request.size as usize != core::mem::size_of::<TraceRequestV1>()
        || !(1..=1_000_000).contains(&request.hits)
        || request.every == 0
        || request.field_count as usize > TRACE_FIELDS
        || request.stack_frames as usize > TRACE_FRAMES
        || request.reserved != 0
        || (request.filter_field != TRACE_NO_FILTER && request.filter_field >= request.field_count)
    {
        return false;
    }
    request.fields[..request.field_count as usize]
        .iter()
        .all(valid_field)
}
fn valid_field(field: &TraceFieldV1) -> bool {
    (field.register as usize) < TRACE_REGISTERS
        && (TRACE_U8..=TRACE_F64).contains(&field.kind)
        && field.depth <= 4
        && field.reserved == 0
}
unsafe fn read_field(field: &TraceFieldV1, registers: &[u64; TRACE_REGISTERS]) -> Option<u64> {
    let mut address = registers[field.register as usize] as usize;
    if field.depth == 0 {
        let mask = match field.kind {
            TRACE_U8 => 0xff,
            TRACE_U16 => 0xffff,
            TRACE_U32 | TRACE_F32 => 0xffff_ffff,
            _ => u64::MAX,
        };
        return Some((address as u64) & mask);
    }
    for level in 0..field.depth as usize {
        address = address.checked_add_signed(field.offsets[level] as isize)?;
        if level + 1 == field.depth as usize {
            let size = match field.kind {
                TRACE_U8 => 1,
                TRACE_U16 => 2,
                TRACE_U32 | TRACE_F32 => 4,
                TRACE_U64 | TRACE_F64 => 8,
                _ => return None,
            };
            let mut bytes = [0u8; 8];
            if !unsafe { read_memory(address, &mut bytes[..size]) } {
                return None;
            }
            return Some(u64::from_le_bytes(bytes));
        }
        let mut bytes = [0u8; core::mem::size_of::<usize>()];
        if !unsafe { read_memory(address, &mut bytes) } {
            return None;
        }
        address = usize::from_le_bytes(bytes);
    }
    None
}
unsafe fn read_memory(address: usize, bytes: &mut [u8]) -> bool {
    unsafe { read_memory_count(address, bytes) == bytes.len() }
}
unsafe fn read_memory_count(address: usize, bytes: &mut [u8]) -> usize {
    if address == 0 || bytes.is_empty() || address.checked_add(bytes.len()).is_none() {
        return 0;
    }
    let mut read = 0usize;
    unsafe {
        let _ = ReadProcessMemory(
            win::GetCurrentProcess(),
            address as *const _,
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            &mut read,
        );
    }
    read.min(bytes.len())
}
fn capture_stack(context: &Context, limit: usize, event: &mut TraceEventV1) {
    if limit == 0 {
        return;
    }
    let rip = context.u64(REG_OFFSETS[16]) as usize;
    if rip != 0 {
        event.frames[0] = rip;
        event.frame_count = 1;
    }
    let address = context.u64(REG_OFFSETS[4]) as usize;
    let mut stack = [0u8; 512];
    let available = unsafe { read_memory_count(address, &mut stack) } / 8;
    for i in 0..available {
        if event.frame_count >= limit as u32 {
            break;
        }
        let candidate = u64::from_le_bytes(stack[i * 8..i * 8 + 8].try_into().unwrap()) as usize;
        if candidate <= 8 {
            continue;
        }
        let mut module = core::ptr::null_mut();
        if unsafe { win::GetModuleHandleExW(0x4 | 0x2, candidate as *const u16, &mut module) } == 0
        {
            continue;
        }
        let mut before = [0u8; 7];
        if !unsafe { read_memory(candidate - 7, &mut before) } || !super::after_call(&before) {
            continue;
        }
        let index = event.frame_count as usize;
        event.frames[index] = candidate;
        event.scanned_frames |= 1 << index;
        event.frame_count += 1;
    }
}

pub static API: TraceCaptureV1 = TraceCaptureV1 {
    open: service_open,
    start: service_start,
    poll: service_poll,
    stop: service_stop,
    close: service_close,
};

#[cfg(test)]
mod tests {
    use super::*;
    fn test_capture(
        request: TraceRequestV1,
        session: u64,
        handle: u64,
        generation: u32,
    ) -> Capture {
        Capture {
            session,
            handle,
            generation,
            address: request.address,
            request,
            active: true,
            last_capture_ms: None,
            queue: vec![TraceEventV1::default(); QUEUE],
            head: 0,
            len: 0,
            captured: 0,
        }
    }
    fn setup(slot: usize, request: TraceRequestV1, session: u64, handle: u64) {
        let generation = next_generation().unwrap();
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        assert!(state.sites[slot].is_none());
        state.sites[slot] = Some(test_capture(request, session, handle, generation));
        COUNTERS[slot]
            .hits
            .store(packed(generation, 0), Ordering::Release);
        COUNTERS[slot]
            .dropped
            .store(packed(generation, 0), Ordering::Release);
        super::super::SITES[slot].store(request.address, Ordering::Release);
        super::super::CAPTURE_SLOTS[slot].store(true, Ordering::Release);
        super::super::CAPTURE_ADDRESSES[slot].store(request.address, Ordering::Release);
        super::super::CAPTURE_GENERATIONS[slot].store(generation, Ordering::Release);
        super::super::CAPTURE_GENERATION_LIMITS[slot]
            .store(packed(generation, request.hits), Ordering::Release);
    }
    fn context(rip: usize, rcx: u64) -> Context {
        let mut context = Context([0; 1232]);
        context.set_u64(REG_OFFSETS[16], rip as u64);
        context.set_u64(REG_OFFSETS[1], rcx);
        context
    }
    fn clean(slot: usize, address: usize) {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(capture) = state.sites[slot].as_ref() {
            let _ = trace::release_capture_slot(slot, address, capture.generation);
        }
        state.sites[slot] = None;
    }

    #[test]
    fn synthetic_hit_snapshots_memory_and_filters_samples() {
        let _serial = crate::trace::CAPTURE_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let address = 0x1234_5000;
        let mut value = 0xabcdu64;
        let request = TraceRequestV1 {
            size: core::mem::size_of::<TraceRequestV1>() as u32,
            hits: 4,
            address,
            every: 2,
            field_count: 1,
            filter_field: 0,
            filter_value: 0xabcd,
            filter_mask: u64::MAX,
            fields: {
                let mut fields = [TraceFieldV1::default(); TRACE_FIELDS];
                fields[0] = TraceFieldV1 {
                    register: 1,
                    kind: TRACE_U64,
                    depth: 1,
                    ..TraceFieldV1::default()
                };
                fields
            },
            ..TraceRequestV1::default()
        };
        setup(0, request, 77, 88);
        let snapshot_context = context(address, (&value as *const u64) as u64);
        for _ in 0..4 {
            on_hit(0, &snapshot_context);
        }
        unsafe { core::ptr::write_volatile(&mut value, 0xeeee) };
        assert_eq!(unsafe { core::ptr::read_volatile(&value) }, 0xeeee);
        let mut events = [TraceEventV1::default(); 4];
        let count =
            unsafe { service_poll(88, events.as_mut_ptr(), events.len(), core::ptr::null_mut()) };
        assert_eq!(count, 2);
        assert_eq!((events[0].sequence, events[1].sequence), (1, 3));
        assert_eq!((events[0].values[0], events[1].values[0]), (0xabcd, 0xabcd));
        clean(0, address);
    }

    #[test]
    fn synthetic_invalid_read_and_queue_overflow_are_bounded() {
        let _serial = crate::trace::CAPTURE_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let address = 0x1234_6000;
        let request = TraceRequestV1 {
            size: core::mem::size_of::<TraceRequestV1>() as u32,
            hits: 129,
            address,
            every: 1,
            field_count: 1,
            filter_field: TRACE_NO_FILTER,
            fields: {
                let mut fields = [TraceFieldV1::default(); TRACE_FIELDS];
                fields[0] = TraceFieldV1 {
                    register: 1,
                    kind: TRACE_U64,
                    depth: 1,
                    ..TraceFieldV1::default()
                };
                fields
            },
            ..TraceRequestV1::default()
        };
        setup(0, request, 79, 89);
        let invalid_context = context(address, 0x1000);
        on_hit(0, &invalid_context);
        let mut one = [TraceEventV1::default(); 1];
        assert_eq!(
            unsafe { service_poll(89, one.as_mut_ptr(), 1, core::ptr::null_mut()) },
            1
        );
        assert_eq!(one[0].valid_fields, 0);
        clean(0, address);

        let address = 0x1234_7000;
        let request = TraceRequestV1 {
            size: core::mem::size_of::<TraceRequestV1>() as u32,
            hits: 129,
            address,
            every: 1,
            filter_field: TRACE_NO_FILTER,
            ..TraceRequestV1::default()
        };
        setup(0, request, 80, 90);
        let hit_context = context(address, 0);
        for _ in 0..129 {
            on_hit(0, &hit_context);
        }
        let mut stats = TraceStatsV1::default();
        assert_eq!(
            unsafe { service_poll(90, core::ptr::null_mut(), 0, &mut stats) },
            0
        );
        assert_eq!(
            (stats.hits, stats.captured, stats.dropped, stats.active),
            (129, 128, 1, 0)
        );
        clean(0, address);
    }

    #[test]
    fn failed_init_cleanup_closes_session_handle_and_prevents_aba_poll() {
        let _serial = crate::trace::CAPTURE_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let owner = 0x7f_000;
        crate::services::begin(owner, "capture-cleanup-test", vec![]);
        let session = unsafe { service_open() };
        assert_ne!(session, 0);
        let address = 0x1234_8000;
        let request = TraceRequestV1 {
            size: core::mem::size_of::<TraceRequestV1>() as u32,
            hits: 2,
            address,
            every: 1,
            filter_field: TRACE_NO_FILTER,
            ..TraceRequestV1::default()
        };
        setup(0, request, session, 0x901);
        crate::services::finish(owner, false);
        assert_eq!(
            unsafe { service_poll(0x901, core::ptr::null_mut(), 0, core::ptr::null_mut()) },
            -2
        );
        assert_eq!(unsafe { service_stop(0x901) }, 1);
        assert_eq!(unsafe { service_close(session) }, 1);
        let next = next_id(&NEXT_HANDLE);
        assert_ne!(next, 0x901);
        clean(0, address);
    }

    #[inline(never)]
    extern "C" fn hardware_probe(value: *const u64) -> u64 {
        std::hint::black_box(unsafe { core::ptr::read_volatile(value) })
    }

    #[inline(never)]
    extern "C" fn hardware_probe_spare(value: u64) -> u64 {
        std::hint::black_box(value).wrapping_add(1)
    }

    #[test]
    fn real_hardware_hit_snapshots_then_retires_and_reuses_a_new_generation() {
        let _serial = crate::trace::CAPTURE_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let owner = 0x7f_100;
        crate::services::begin(owner, "capture-hardware-test", vec![]);
        let session = unsafe { service_open() };
        assert_ne!(session, 0);
        let address = hardware_probe as extern "C" fn(*const u64) -> u64 as usize;
        let request = TraceRequestV1 {
            size: core::mem::size_of::<TraceRequestV1>() as u32,
            hits: 1,
            address,
            every: 1,
            field_count: 1,
            filter_field: TRACE_NO_FILTER,
            fields: {
                let mut fields = [TraceFieldV1::default(); TRACE_FIELDS];
                fields[0] = TraceFieldV1 {
                    register: 1,
                    kind: TRACE_U64,
                    depth: 1,
                    ..TraceFieldV1::default()
                };
                fields
            },
            ..TraceRequestV1::default()
        };
        let mut handle = 0;
        assert_eq!(unsafe { service_start(session, &request, &mut handle) }, 0);
        let mut duplicate = 0;
        assert_eq!(
            unsafe { service_start(session, &request, &mut duplicate) },
            3
        );
        assert!(
            !trace::release(address),
            "legacy trace-v1 stop must not release a capture site"
        );
        assert_eq!(
            trace::add(address, 1, "capture-duplicate", crate::log::info),
            Err(trace::TraceError::Duplicate),
            "legacy trace-v1 registration must see the capture reservation"
        );

        let slot = super::super::CAPTURE_ADDRESSES
            .iter()
            .position(|site| site.load(Ordering::Acquire) == address)
            .unwrap();
        let occupied: Vec<_> = (0..MAX_SITES).filter(|&other| other != slot).collect();
        for &other in &occupied {
            assert!(!super::super::CLAIMED[other].swap(true, Ordering::AcqRel));
        }
        let spare = TraceRequestV1 {
            address: hardware_probe_spare as extern "C" fn(u64) -> u64 as usize,
            ..request
        };
        assert_eq!(
            unsafe { service_start(session, &spare, &mut duplicate) },
            2,
            "capture and legacy sites share all four claims"
        );
        for other in occupied {
            super::super::CLAIMED[other].store(false, Ordering::Release);
        }
        crate::services::finish(owner, true);

        let mut value = 0x1234_5678_9abc_def0u64;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut event = TraceEventV1::default();
        let mut count = 0;
        while std::time::Instant::now() < deadline && count == 0 {
            let _ = hardware_probe(&value);
            unsafe { core::ptr::write_volatile(&mut value, 0xeeee) };
            count = unsafe { service_poll(handle, &mut event, 1, core::ptr::null_mut()) };
            if count == 0 {
                unsafe { core::ptr::write_volatile(&mut value, 0x1234_5678_9abc_def0) };
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        assert_eq!(
            count, 1,
            "the helper thread eventually arms this test thread"
        );
        assert_eq!(event.values[0], 0x1234_5678_9abc_def0);
        assert_eq!(event.valid_fields & 1, 1);
        let mut stats = TraceStatsV1::default();
        assert_eq!(
            unsafe { service_poll(handle, core::ptr::null_mut(), 0, &mut stats) },
            0
        );
        assert_eq!((stats.hits, stats.captured, stats.active), (1, 1, 0));
        assert_eq!(
            unsafe { service_start(session, &request, &mut duplicate) },
            3,
            "a completed queue retains its address until stop"
        );
        assert_eq!(unsafe { service_stop(handle) }, 0);
        assert_eq!(
            unsafe { service_poll(handle, core::ptr::null_mut(), 0, core::ptr::null_mut()) },
            -2
        );
        let mut replacement = 0;
        assert_eq!(
            unsafe { service_start(session, &request, &mut replacement) },
            0
        );
        assert_ne!(replacement, handle);
        assert_eq!(
            unsafe { service_poll(handle, core::ptr::null_mut(), 0, core::ptr::null_mut()) },
            -2
        );
        assert!(
            !super::super::release_slot(slot, address),
            "a delayed legacy handler must not release a reused capture slot"
        );
        let mut replacement_stats = TraceStatsV1::default();
        assert_eq!(
            unsafe {
                service_poll(
                    replacement,
                    core::ptr::null_mut(),
                    0,
                    &mut replacement_stats,
                )
            },
            0
        );
        assert_eq!(replacement_stats.active, 1);
        assert_eq!(unsafe { service_stop(replacement) }, 0);
        assert_eq!(unsafe { service_close(session) }, 0);
    }

    #[test]
    fn malformed_plans_are_rejected() {
        let mut request = TraceRequestV1 {
            size: core::mem::size_of::<TraceRequestV1>() as u32,
            hits: 1,
            every: 1,
            filter_field: TRACE_NO_FILTER,
            ..TraceRequestV1::default()
        };
        assert!(valid(&request));
        request.every = 0;
        assert!(!valid(&request));
    }
}
