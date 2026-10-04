//! Persist retained shared-ammo rounds through the stock depot serializer.
//!
//! Marker rows exist only while the stock writer runs and while the stock load
//! constructor has finished but has not returned to gameplay. The native row
//! format and stream framing remain unchanged.

use crate::equipment::ItemBounds;
use core::ffi::c_void;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

const RECORDS: usize = 0x20;
const STRIDE: usize = 0x48;
const MAX_ROWS: usize = 128;
const MAX_SERIALIZED_ROWS: usize = MAX_ROWS * 2;
const CAPACITY: usize = 0x28;
const ROUNDS: usize = 0x2c;
const RESERVED: usize = 0x30;
const CARRIERS: usize = 0x34;
const TAG_A: u32 = 0x5744_5231;
const TAG_B: u32 = 0x5253_5631;
const MAX_NAME: usize = 256;

type Writer = unsafe extern "C" fn(usize, usize);
type Loader = unsafe extern "C" fn(usize, usize) -> usize;
type Destructor = unsafe extern "C" fn(usize, usize) -> usize;
type StringCopy = unsafe extern "C" fn(usize, usize) -> usize;
type StringDestroy = unsafe extern "C" fn(usize);

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Bindings {
    /// Steam 2026-09-25 logic.dll `AmmunitionDepotHelper::vfunc_1`, RVA 0x121f20.
    pub writer_original: usize,
    /// Steam 2026-09-25 logic.dll ammo-depot load constructor, RVA 0x121830.
    pub load_original: usize,
    /// Steam 2026-09-25 logic.dll `AmmunitionDepotHelper` deleting destructor, RVA 0x121760.
    pub destructor_original: usize,
    /// Steam 2026-09-25 logic.dll std::string copy constructor, RVA 0x24b70.
    pub string_copy: usize,
    /// Steam 2026-09-25 logic.dll std::string destructor, RVA 0x24c90.
    pub string_destroy: usize,
    pub log: usize,
}

#[derive(Clone, Debug, Default)]
struct PoolReserves {
    rounds: BTreeMap<String, u32>,
    /// Ammo metadata pointers are world-local; canonical names are serialized.
    ammo_info: BTreeMap<String, usize>,
}

static BINDINGS: Mutex<Bindings> = Mutex::new(Bindings {
    writer_original: 0,
    load_original: 0,
    destructor_original: 0,
    string_copy: 0,
    string_destroy: 0,
    log: 0,
});
pub(super) static ORIGINAL_WRITER: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
pub(super) static ORIGINAL_LOAD: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
pub(super) static ORIGINAL_DESTROY: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static SAVE_REFUSALS: AtomicUsize = AtomicUsize::new(0);
static STATES: OnceLock<Mutex<BTreeMap<usize, PoolReserves>>> = OnceLock::new();

fn states() -> &'static Mutex<BTreeMap<usize, PoolReserves>> {
    STATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(super) fn configure(bindings: Bindings) {
    if bindings.writer_original != 0 {
        ORIGINAL_WRITER.store(bindings.writer_original as *mut c_void, Ordering::Release);
    }
    if bindings.load_original != 0 {
        ORIGINAL_LOAD.store(bindings.load_original as *mut c_void, Ordering::Release);
    }
    if bindings.destructor_original != 0 {
        ORIGINAL_DESTROY.store(
            bindings.destructor_original as *mut c_void,
            Ordering::Release,
        );
    }
    *BINDINGS.lock().unwrap_or_else(|error| error.into_inner()) = bindings;
}

fn binding_snapshot() -> Bindings {
    let mut bindings = *BINDINGS.lock().unwrap_or_else(|error| error.into_inner());
    let writer = ORIGINAL_WRITER.load(Ordering::Acquire);
    let loader = ORIGINAL_LOAD.load(Ordering::Acquire);
    let destructor = ORIGINAL_DESTROY.load(Ordering::Acquire);
    if !writer.is_null() {
        bindings.writer_original = writer as usize;
    }
    if !loader.is_null() {
        bindings.load_original = loader as usize;
    }
    if !destructor.is_null() {
        bindings.destructor_original = destructor as usize;
    }
    bindings
}

fn word(address: usize) -> usize {
    unsafe { core::ptr::read_unaligned(address as *const usize) }
}

fn dword(address: usize) -> u32 {
    unsafe { core::ptr::read_unaligned(address as *const u32) }
}

fn put_word(address: usize, value: usize) {
    unsafe { core::ptr::write_unaligned(address as *mut usize, value) }
}

fn put_dword(address: usize, value: u32) {
    unsafe { core::ptr::write_unaligned(address as *mut u32, value) }
}

fn valid_ptr(pointer: usize) -> bool {
    pointer != 0 && pointer.is_multiple_of(8)
}

fn pool_bounds_limit(pool: usize, max_rows: usize) -> Result<ItemBounds, &'static str> {
    if !valid_ptr(pool) || !valid_ptr(word(pool)) {
        return Err("invalid ammo depot object");
    }
    let bounds = ItemBounds {
        begin: word(pool + RECORDS),
        end: word(pool + RECORDS + 8),
        capacity: word(pool + RECORDS + 16),
    };
    if bounds.begin == 0 && bounds.end == 0 && bounds.capacity == 0 {
        return Ok(bounds);
    }
    if !valid_ptr(bounds.begin)
        || bounds.end < bounds.begin
        || bounds.capacity < bounds.end
        || !(bounds.end - bounds.begin).is_multiple_of(STRIDE)
        || (bounds.end - bounds.begin) / STRIDE > max_rows
    {
        return Err("invalid ammo depot row vector");
    }
    Ok(bounds)
}

fn pool_bounds(pool: usize) -> Result<ItemBounds, &'static str> {
    pool_bounds_limit(pool, MAX_ROWS)
}

fn row_count(bounds: ItemBounds) -> usize {
    if bounds.begin == 0 {
        0
    } else {
        (bounds.end - bounds.begin) / STRIDE
    }
}

fn descriptor_name(info: usize) -> Result<String, &'static str> {
    if !valid_ptr(info) {
        return Err("invalid ammo metadata pointer");
    }
    let string = info + 8;
    let length = word(string + 0x10);
    let capacity = word(string + 0x18);
    if length == 0 || length > MAX_NAME || length > capacity {
        return Err("invalid ammo metadata name length");
    }
    let bytes = if capacity > 15 {
        let data = word(string);
        if !valid_ptr(data) {
            return Err("invalid ammo metadata name storage");
        }
        unsafe { core::slice::from_raw_parts(data as *const u8, length) }
    } else {
        unsafe { core::slice::from_raw_parts(string as *const u8, length) }
    };
    if bytes.contains(&0) {
        return Err("ammo metadata name contains a nul byte");
    }
    let name = core::str::from_utf8(bytes).map_err(|_| "ammo metadata name is not UTF-8")?;
    Ok(name.to_owned())
}

/// Copies the ammo descriptor identity before a native inventory mutation.
pub(super) unsafe fn canonical_name(info: usize) -> Result<String, &'static str> {
    descriptor_name(info)
}

fn native_strings(bindings: Bindings) -> Result<(StringCopy, StringDestroy), &'static str> {
    if bindings.string_copy == 0 || bindings.string_destroy == 0 {
        return Err("native string helpers are unavailable");
    }
    Ok(unsafe {
        (
            core::mem::transmute::<usize, StringCopy>(bindings.string_copy),
            core::mem::transmute::<usize, StringDestroy>(bindings.string_destroy),
        )
    })
}

fn state_snapshot(pool: usize) -> Option<PoolReserves> {
    states()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&pool)
        .cloned()
}

/// Adds rounds from native `[AmmoScriptInfo*, rounds]` rows to this depot's
/// unused reserve. The metadata names are copied so saved state never depends
/// on pointers surviving a world or process reload.
fn checked_deposit(
    pool: usize,
    current: PoolReserves,
    rows: &[[usize; 2]],
) -> Result<PoolReserves, &'static str> {
    if !valid_ptr(pool) || rows.len() > MAX_ROWS {
        return Err("invalid reserve deposit");
    }
    let mut additions = Vec::new();
    for [info, count] in rows {
        if *count == 0 {
            continue;
        }
        if *count > u32::MAX as usize {
            return Err("reserve deposit exceeds 32-bit rounds");
        }
        additions.push((descriptor_name(*info)?, *info, *count as u32));
    }
    let mut next = current;
    for (name, info, count) in additions {
        let rounds = next
            .rounds
            .get(&name)
            .copied()
            .unwrap_or(0)
            .checked_add(count)
            .ok_or("unused ammunition reserve overflow")?;
        next.rounds.insert(name.clone(), rounds);
        next.ammo_info.insert(name, info);
    }
    if next.rounds.len() > MAX_ROWS {
        return Err("too many unused reserve identities");
    }
    let stock_rows = row_count(pool_bounds(pool)?);
    if stock_rows + next.rounds.len() > MAX_SERIALIZED_ROWS {
        return Err("ammo reserve exceeds stock serializer row limit");
    }
    Ok(next)
}

/// Validates the complete deposit without changing retained state.
pub(super) fn preflight_deposit(pool: usize, rows: &[[usize; 2]]) -> Result<(), &'static str> {
    let all = states().lock().unwrap_or_else(|error| error.into_inner());
    let current = all.get(&pool).cloned().unwrap_or_default();
    checked_deposit(pool, current, rows).map(|_| ())
}

/// Adds rounds from native `[AmmoScriptInfo*, rounds]` rows to this depot's
/// unused reserve. The metadata names are copied so saved state never depends
/// on pointers surviving a world or process reload.
pub(super) fn deposit_rows(pool: usize, rows: &[[usize; 2]]) -> Result<(), &'static str> {
    let mut all = states().lock().unwrap_or_else(|error| error.into_inner());
    let current = all.get(&pool).cloned().unwrap_or_default();
    let next = checked_deposit(pool, current, rows)?;
    all.insert(pool, next);
    Ok(())
}

/// Returns retained rounds as native `[AmmoScriptInfo*, rounds]` rows.
pub(super) fn snapshot(pool: usize) -> Result<Vec<[usize; 2]>, &'static str> {
    let Some(state) = state_snapshot(pool) else {
        return Ok(Vec::new());
    };
    state
        .rounds
        .iter()
        .map(|(name, rounds)| {
            let info = state
                .ammo_info
                .get(name)
                .copied()
                .filter(|info| valid_ptr(*info))
                .ok_or("retained ammo metadata is not resolved in this world")?;
            Ok([info, *rounds as usize])
        })
        .collect()
}

/// Removes exactly the listed rounds, atomically, after a successful import.
pub(super) fn withdraw(pool: usize, rows: &[[usize; 2]]) -> Result<(), &'static str> {
    let mut withdrawals = Vec::new();
    for [info, count] in rows {
        if *count == 0 {
            continue;
        }
        if *count > u32::MAX as usize {
            return Err("reserve withdrawal exceeds 32-bit rounds");
        }
        withdrawals.push((descriptor_name(*info)?, *count as u32));
    }
    let mut all = states().lock().unwrap_or_else(|error| error.into_inner());
    let Some(current) = all.get(&pool).cloned() else {
        return Err("unused reserve is empty");
    };
    let mut next = current;
    for (name, count) in withdrawals {
        let remaining = next
            .rounds
            .get(&name)
            .copied()
            .unwrap_or(0)
            .checked_sub(count)
            .ok_or("unused reserve withdrawal exceeds ownership")?;
        if remaining == 0 {
            next.rounds.remove(&name);
            next.ammo_info.remove(&name);
        } else {
            next.rounds.insert(name, remaining);
        }
    }
    if next.rounds.is_empty() {
        all.remove(&pool);
    } else {
        all.insert(pool, next);
    }
    Ok(())
}

/// Forgets world-local metadata pointers before native pool destruction or
/// construction at a recycled address.
pub(super) fn forget(pool: usize) {
    states()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&pool);
}

unsafe fn report(message: &'static core::ffi::CStr) {
    let log = binding_snapshot().log;
    if log != 0 {
        let logger: unsafe extern "C" fn(u32, *const i8) = core::mem::transmute(log);
        logger(defiance_api::LOG_DEBUG, message.as_ptr());
    }
}

unsafe fn report_save_refusal() {
    if SAVE_REFUSALS.fetch_add(1, Ordering::Relaxed) < 8 {
        report(c"retained ammo was not added to save: reserve rows could not be serialized safely");
    }
}

/// Native wrapper for `AmmunitionDepotHelper::vfunc_1` (Steam 2026-09-25).
///
/// # Safety
/// The host must detour only the supported native vfunc with its original ABI.
pub(super) unsafe extern "C" fn writer(pool: usize, stream: usize) {
    let bindings = binding_snapshot();
    let Some(original) = (bindings.writer_original != 0)
        .then(|| unsafe { core::mem::transmute::<usize, Writer>(bindings.writer_original) })
    else {
        if state_snapshot(pool).is_some_and(|state| !state.rounds.is_empty()) {
            unsafe {
                report_save_refusal();
            }
        }
        return;
    };
    let reserves = match state_snapshot(pool) {
        Some(state) if !state.rounds.is_empty() => state,
        _ => {
            original(pool, stream);
            return;
        }
    };
    let original_bounds = match pool_bounds(pool) {
        Ok(bounds) => bounds,
        Err(_) => {
            unsafe {
                report_save_refusal();
            }
            original(pool, stream);
            return;
        }
    };
    let count = row_count(original_bounds);
    if count + reserves.rounds.len() > MAX_SERIALIZED_ROWS {
        unsafe {
            report_save_refusal();
        }
        original(pool, stream);
        return;
    }
    let (copy_string, destroy_string) = match native_strings(bindings) {
        Ok(functions) => functions,
        Err(_) => {
            unsafe {
                report_save_refusal();
            }
            original(pool, stream);
            return;
        }
    };
    let marker_rows: Option<Vec<(usize, u32)>> = reserves
        .rounds
        .iter()
        .map(|(name, rounds)| {
            reserves
                .ammo_info
                .get(name)
                .copied()
                .filter(|info| valid_ptr(*info))
                .map(|info| (info, *rounds))
        })
        .collect();
    let Some(marker_rows) = marker_rows else {
        unsafe {
            report_save_refusal();
        }
        original(pool, stream);
        return;
    };
    let mut scratch = vec![[0usize; STRIDE / 8]; count + reserves.rounds.len()];
    let scratch_begin = scratch.as_mut_ptr() as usize;
    let scratch_end = scratch_begin + scratch.len() * STRIDE;
    if count != 0 {
        core::ptr::copy_nonoverlapping(
            original_bounds.begin as *const u8,
            scratch_begin as *mut u8,
            count * STRIDE,
        );
        for index in 0..count {
            let source = original_bounds.begin + index * STRIDE + 8;
            let destination = scratch_begin + index * STRIDE + 8;
            core::ptr::write_bytes(destination as *mut u8, 0, 0x20);
            let _ = copy_string(destination, source);
        }
    }
    for (index, (info, rounds)) in marker_rows.into_iter().enumerate() {
        let marker = scratch_begin + (count + index) * STRIDE;
        put_word(marker, info);
        let _ = copy_string(marker + 8, info + 8);
        put_dword(marker + CAPACITY, rounds);
        put_dword(marker + ROUNDS, rounds);
        put_dword(marker + RESERVED, 0);
        put_dword(marker + CARRIERS, 1);
        put_dword(marker + 0x38, TAG_A);
        put_dword(marker + 0x3c, TAG_B);
    }

    let restore = BoundsRestore::new(pool, original_bounds);
    put_word(pool + RECORDS, scratch_begin);
    put_word(pool + RECORDS + 8, scratch_end);
    put_word(pool + RECORDS + 16, scratch_end);
    original(pool, stream);
    drop(restore);
    for index in 0..scratch.len() {
        destroy_string(scratch_begin + index * STRIDE + 8);
    }
}

struct BoundsRestore {
    pool: usize,
    bounds: ItemBounds,
}

impl BoundsRestore {
    fn new(pool: usize, bounds: ItemBounds) -> Self {
        Self { pool, bounds }
    }
}

impl Drop for BoundsRestore {
    fn drop(&mut self) {
        put_word(self.pool + RECORDS, self.bounds.begin);
        put_word(self.pool + RECORDS + 8, self.bounds.end);
        put_word(self.pool + RECORDS + 16, self.bounds.capacity);
    }
}

/// Native wrapper for the stock load constructor. Tagged reserve rows are
/// removed before the constructed depot becomes visible to game code.
///
/// # Safety
/// The host must detour only the supported native constructor with its ABI.
pub(super) unsafe extern "C" fn load(pool: usize, stream: usize) -> usize {
    forget(pool);
    let bindings = binding_snapshot();
    let Some(original) = (bindings.load_original != 0)
        .then(|| unsafe { core::mem::transmute::<usize, Loader>(bindings.load_original) })
    else {
        return 0;
    };
    let result = original(pool, stream);
    if result == 0 || result != pool {
        return result;
    }
    let Ok(bounds) = pool_bounds_limit(pool, MAX_SERIALIZED_ROWS) else {
        unsafe {
            report(c"retained ammo rows skipped: invalid loaded depot bounds");
        }
        return result;
    };
    let count = row_count(bounds);
    if count == 0 {
        return result;
    }
    let mut markers = Vec::new();
    for index in 0..count {
        let row = bounds.begin + index * STRIDE;
        if dword(row + 0x38) == TAG_A && dword(row + 0x3c) == TAG_B {
            let info = word(row);
            let rounds = dword(row + ROUNDS);
            let parsed = if rounds == 0
                || dword(row + CAPACITY) != rounds
                || dword(row + RESERVED) != 0
                || dword(row + CARRIERS) != 1
            {
                None
            } else {
                descriptor_name(info).ok().map(|name| (name, info, rounds))
            };
            markers.push((row, parsed));
        }
    }
    if markers.is_empty() {
        return result;
    }

    if let Ok((_, destroy_string)) = native_strings(bindings) {
        let mut write_index = 0;
        let mut marker_index = 0;
        for read_index in 0..count {
            let source = bounds.begin + read_index * STRIDE;
            if markers
                .get(marker_index)
                .is_some_and(|(row, _)| *row == source)
            {
                let (_, parsed) = &markers[marker_index];
                if let Some((name, info, rounds)) = parsed {
                    let mut all = states().lock().unwrap_or_else(|error| error.into_inner());
                    let state = all.entry(pool).or_default();
                    let next = state
                        .rounds
                        .get(name)
                        .copied()
                        .unwrap_or(0)
                        .checked_add(*rounds);
                    if let Some(next) = next.filter(|_| {
                        state.rounds.len() < MAX_ROWS || state.rounds.contains_key(name)
                    }) {
                        state.rounds.insert(name.clone(), next);
                        state.ammo_info.insert(name.clone(), *info);
                    }
                }
                destroy_string(source + 8);
                core::ptr::write_bytes(source as *mut u8, 0, STRIDE);
                marker_index += 1;
                continue;
            }
            if write_index != read_index {
                let destination = bounds.begin + write_index * STRIDE;
                core::ptr::copy(source as *const u8, destination as *mut u8, STRIDE);
                // Ownership of this string moved with the record bytes.
                core::ptr::write_bytes((source + 8) as *mut u8, 0, 0x20);
            }
            write_index += 1;
        }
        put_word(pool + RECORDS + 8, bounds.begin + write_index * STRIDE);
    } else {
        unsafe {
            report(c"retained ammo rows skipped: native string helpers unavailable");
        }
    }
    result
}

/// Clears pointer-keyed reserve state before stock depot destruction.
///
/// # Safety
/// The host must detour only the supported native destructor with its ABI.
pub(super) unsafe extern "C" fn destroy(pool: usize, deleting: usize) -> usize {
    forget(pool);
    let bindings = binding_snapshot();
    if bindings.destructor_original == 0 {
        return 0;
    }
    let original: Destructor = core::mem::transmute(bindings.destructor_original);
    original(pool, deleting)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[repr(align(16))]
    struct Region(Box<[u64]>);

    impl Region {
        fn new(bytes: usize) -> Self {
            Self(vec![0; bytes.div_ceil(8).max(1)].into_boxed_slice())
        }
        fn ptr(&self) -> usize {
            self.0.as_ptr() as usize
        }
    }

    struct Fixture {
        regions: Vec<Region>,
        pool: usize,
        info: usize,
    }

    impl Fixture {
        fn alloc(&mut self, bytes: usize) -> usize {
            self.regions.push(Region::new(bytes));
            self.regions.last().unwrap().ptr()
        }
        fn new() -> Self {
            Self::named("rifle_ammo")
        }
        fn named(name: &str) -> Self {
            let mut value = Self {
                regions: Vec::new(),
                pool: 0,
                info: 0,
            };
            value.regions.push(Region::new(0x40));
            value.info = value.regions.last().unwrap().ptr();
            value.set_string(value.info, 8, name.as_bytes());

            value.pool = value.alloc(0x200);
            let vtable = value.alloc(8);
            put_word(value.pool, vtable);
            put_word(value.pool + 0x58, value.info);
            let old = value.alloc(STRIDE);
            put_word(old, value.info);
            value.set_string(old, 8, name.as_bytes());
            put_dword(old + CAPACITY, 100);
            put_dword(old + ROUNDS, 40);
            put_dword(old + CARRIERS, 1);
            put_word(value.pool + RECORDS, old);
            put_word(value.pool + RECORDS + 8, old + STRIDE);
            put_word(value.pool + RECORDS + 16, old + STRIDE);
            value
        }
        fn set_string(&mut self, object: usize, offset: usize, bytes: &[u8]) {
            unsafe {
                if bytes.len() <= 15 {
                    core::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        (object + offset) as *mut u8,
                        bytes.len(),
                    );
                    put_word(object + offset + 0x10, bytes.len());
                    put_word(object + offset + 0x18, 15);
                } else {
                    let data = self.alloc(bytes.len() + 1);
                    core::ptr::copy_nonoverlapping(bytes.as_ptr(), data as *mut u8, bytes.len());
                    core::ptr::write((data as *mut u8).add(bytes.len()), 0);
                    put_word(object + offset, data);
                    put_word(object + offset + 0x10, bytes.len());
                    put_word(object + offset + 0x18, bytes.len());
                }
            }
        }
    }

    unsafe fn copy_test_string(destination: usize, bytes: &[u8]) {
        if bytes.len() > 15 {
            let mut heap = vec![0u8; bytes.len() + 1].into_boxed_slice();
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), heap.as_mut_ptr(), bytes.len());
            let data = heap.as_mut_ptr() as usize;
            TEST_HEAP_STRINGS.with(|strings| strings.borrow_mut().insert(data, heap));
            put_word(destination, data);
            put_word(destination + 0x18, bytes.len());
        } else {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), destination as *mut u8, bytes.len());
            put_word(destination + 0x18, 15);
        }
        put_word(destination + 0x10, bytes.len());
    }

    thread_local! {
        static TEST_HEAP_STRINGS: std::cell::RefCell<BTreeMap<usize, Box<[u8]>>> = const { std::cell::RefCell::new(BTreeMap::new()) };
        static STRING_DESTROYS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    unsafe extern "C" fn fake_string_copy(destination: usize, source: usize) -> usize {
        let len = word(source + 0x10);
        let capacity = word(source + 0x18);
        let source_data = if capacity > 15 { word(source) } else { source };
        if len > 15 {
            let mut heap = vec![0u8; len + 1].into_boxed_slice();
            core::ptr::copy_nonoverlapping(source_data as *const u8, heap.as_mut_ptr(), len);
            let data = heap.as_mut_ptr() as usize;
            TEST_HEAP_STRINGS.with(|strings| strings.borrow_mut().insert(data, heap));
            put_word(destination, data);
            put_word(destination + 0x18, len);
        } else {
            core::ptr::copy_nonoverlapping(source_data as *const u8, destination as *mut u8, len);
            put_word(destination + 0x18, 15);
        }
        put_word(destination + 0x10, len);
        destination
    }

    unsafe extern "C" fn fake_string_destroy(value: usize) {
        let capacity = word(value + 0x18);
        if capacity > 15 {
            let data = word(value);
            assert!(TEST_HEAP_STRINGS
                .with(|strings| strings.borrow_mut().remove(&data))
                .is_some());
        }
        STRING_DESTROYS.with(|count| count.set(count.get() + 1));
        core::ptr::write_bytes(value as *mut u8, 0, 0x20);
        put_word(value + 0x10, 0);
        put_word(value + 0x18, 15);
    }

    unsafe extern "C" fn fake_writer(pool: usize, _stream: usize) {
        let bounds = pool_bounds(pool).unwrap();
        let mut saved = Vec::new();
        for index in 0..row_count(bounds) {
            let row = bounds.begin + index * STRIDE;
            let info = word(row);
            let name = descriptor_name(info).unwrap();
            saved.push((
                name,
                dword(row + ROUNDS),
                dword(row + 0x38),
                dword(row + 0x3c),
            ));
        }
        WRITER_ROWS.with(|rows| *rows.borrow_mut() = saved);
    }

    thread_local! { static WRITER_ROWS: std::cell::RefCell<Vec<(String,u32,u32,u32)>> = const { std::cell::RefCell::new(Vec::new()) }; }

    unsafe extern "C" fn fake_loader(pool: usize, _stream: usize) -> usize {
        let base = pool + 0x80;
        let info = word(pool + 0x58);
        let name = descriptor_name(info).unwrap();
        for (index, (_, rounds, tag_a, tag_b)) in
            SAVED.with(|rows| rows.borrow().clone()).iter().enumerate()
        {
            let row = base + index * STRIDE;
            core::ptr::write_bytes(row as *mut u8, 0, STRIDE);
            put_word(row, info);
            copy_test_string(row + 8, name.as_bytes());
            put_dword(row + CAPACITY, *rounds);
            put_dword(row + ROUNDS, *rounds);
            put_dword(row + CARRIERS, 1);
            put_dword(row + 0x38, *tag_a);
            put_dword(row + 0x3c, *tag_b);
        }
        put_word(pool + RECORDS, base);
        let count = SAVED.with(|rows| rows.borrow().len());
        put_word(pool + RECORDS + 8, base + count * STRIDE);
        put_word(pool + RECORDS + 16, base + count * STRIDE);
        pool
    }

    unsafe extern "C" fn malformed_marker_loader(pool: usize, stream: usize) -> usize {
        let result = fake_loader(pool, stream);
        let bounds = pool_bounds_limit(pool, MAX_SERIALIZED_ROWS).unwrap();
        put_dword(bounds.end - STRIDE + RESERVED, 1);
        result
    }

    thread_local! { static SAVED: std::cell::RefCell<Vec<(String,u32,u32,u32)>> = const { std::cell::RefCell::new(Vec::new()) }; }

    unsafe extern "C" fn fake_destructor(_pool: usize, _deleting: usize) -> usize {
        1
    }

    fn install(fixture: &Fixture) {
        configure(Bindings {
            writer_original: fake_writer as *const () as usize,
            load_original: fake_loader as *const () as usize,
            destructor_original: fake_destructor as *const () as usize,
            string_copy: fake_string_copy as *const () as usize,
            string_destroy: fake_string_destroy as *const () as usize,
            log: 0,
        });
        let _ = fixture;
    }

    #[test]
    fn writer_borrows_tagged_rows_then_restores_native_bounds() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = Fixture::new();
        install(&fixture);
        deposit_rows(fixture.pool, &[[fixture.info, 23]]).unwrap();
        let before = pool_bounds(fixture.pool).unwrap();
        unsafe {
            writer(fixture.pool, 0x1000);
        }
        let after = pool_bounds(fixture.pool).unwrap();
        assert_eq!(
            (after.begin, after.end, after.capacity),
            (before.begin, before.end, before.capacity)
        );
        let saved = WRITER_ROWS.with(|rows| rows.borrow().clone());
        assert_eq!(
            saved,
            [
                ("rifle_ammo".into(), 40, 0, 0),
                ("rifle_ammo".into(), 23, TAG_A, TAG_B)
            ]
        );
        forget(fixture.pool);
    }

    #[test]
    fn writer_copies_and_destroys_heap_strings_without_changing_stock_rows() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let name = "exceptionally_long_rifle_ammunition_identity";
        let fixture = Fixture::named(name);
        install(&fixture);
        deposit_rows(fixture.pool, &[[fixture.info, 9]]).unwrap();
        let stock_row = pool_bounds(fixture.pool).unwrap().begin;
        let stock_string = stock_row + 8;
        let source_storage = word(stock_string);
        STRING_DESTROYS.with(|count| count.set(0));

        unsafe {
            writer(fixture.pool, 0x1000);
        }

        assert_eq!(word(stock_string), source_storage);
        assert_eq!(descriptor_name(word(stock_row)).unwrap(), name);
        assert_eq!(STRING_DESTROYS.with(std::cell::Cell::get), 2);
        assert!(TEST_HEAP_STRINGS.with(|strings| strings.borrow().is_empty()));
        forget(fixture.pool);
    }

    #[test]
    fn deposit_preflight_and_withdraw_use_owned_state_atomically() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = Fixture::new();
        forget(fixture.pool);
        assert!(preflight_deposit(fixture.pool, &[[fixture.info, 100]]).is_ok());
        assert!(snapshot(fixture.pool).unwrap().is_empty());

        deposit_rows(fixture.pool, &[[fixture.info, 100]]).unwrap();
        assert!(preflight_deposit(fixture.pool, &[[fixture.info, u32::MAX as usize]]).is_err());
        assert_eq!(snapshot(fixture.pool).unwrap(), [[fixture.info, 100]]);
        deposit_rows(fixture.pool, &[[fixture.info, 80]]).unwrap();
        assert_eq!(snapshot(fixture.pool).unwrap(), [[fixture.info, 180]]);

        assert!(withdraw(fixture.pool, &[[fixture.info, 181]]).is_err());
        assert_eq!(snapshot(fixture.pool).unwrap(), [[fixture.info, 180]]);
        withdraw(fixture.pool, &[[fixture.info, 50]]).unwrap();
        assert_eq!(snapshot(fixture.pool).unwrap(), [[fixture.info, 130]]);
        withdraw(fixture.pool, &[[fixture.info, 130]]).unwrap();
        assert!(snapshot(fixture.pool).unwrap().is_empty());
    }

    #[test]
    fn malformed_marker_shape_is_removed_without_crediting_rounds() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = Fixture::new();
        install(&fixture);
        ORIGINAL_LOAD.store(
            malformed_marker_loader as *mut c_void,
            std::sync::atomic::Ordering::Release,
        );
        SAVED.with(|rows| {
            *rows.borrow_mut() = vec![
                ("rifle_ammo".into(), 40, 0, 0),
                ("rifle_ammo".into(), 23, TAG_A, TAG_B),
            ]
        });
        unsafe {
            load(fixture.pool, 0x1000);
        }
        assert_eq!(row_count(pool_bounds(fixture.pool).unwrap()), 1);
        assert!(snapshot(fixture.pool).unwrap().is_empty());
        forget(fixture.pool);
    }

    #[test]
    fn load_strips_markers_before_return_and_restores_owned_reserves() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = Fixture::new();
        install(&fixture);
        SAVED.with(|rows| {
            *rows.borrow_mut() = vec![
                ("rifle_ammo".into(), 40, 0, 0),
                ("rifle_ammo".into(), 23, TAG_A, TAG_B),
            ]
        });
        unsafe {
            load(fixture.pool, 0x1000);
        }
        assert_eq!(row_count(pool_bounds(fixture.pool).unwrap()), 1);
        assert_eq!(snapshot(fixture.pool).unwrap(), [[fixture.info, 23]]);
        forget(fixture.pool);
    }

    #[test]
    fn load_strips_a_malformed_zero_round_marker_instead_of_exposing_it() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = Fixture::new();
        install(&fixture);
        SAVED.with(|rows| *rows.borrow_mut() = vec![("rifle_ammo".into(), 0, TAG_A, TAG_B)]);
        unsafe {
            load(fixture.pool, 0x1000);
        }
        assert_eq!(row_count(pool_bounds(fixture.pool).unwrap()), 0);
        assert!(snapshot(fixture.pool).unwrap().is_empty());
    }

    #[test]
    fn stock_record_and_canonical_reserve_name_survive_full_roundtrip() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let name = "exceptionally_long_rifle_ammunition_identity";
        let fixture = Fixture::named(name);
        install(&fixture);
        forget(fixture.pool);
        deposit_rows(fixture.pool, &[[fixture.info, 17]]).unwrap();
        unsafe {
            writer(fixture.pool, 0x1000);
        }
        let serialized = WRITER_ROWS.with(|rows| rows.borrow().clone());
        assert_eq!(serialized[0], (name.to_owned(), 40, 0, 0));
        assert_eq!(serialized[1], (name.to_owned(), 17, TAG_A, TAG_B));
        SAVED.with(|rows| *rows.borrow_mut() = serialized);

        unsafe {
            load(fixture.pool, 0x1000);
        }

        assert_eq!(row_count(pool_bounds(fixture.pool).unwrap()), 1);
        assert_eq!(snapshot(fixture.pool).unwrap(), [[fixture.info, 17]]);
        let bounds = pool_bounds(fixture.pool).unwrap();
        unsafe {
            fake_string_destroy(bounds.begin + 8);
        }
        assert!(TEST_HEAP_STRINGS.with(|strings| strings.borrow().is_empty()));
        forget(fixture.pool);
    }

    #[test]
    fn destructor_and_load_at_reused_address_clear_prior_pointer_state() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = Fixture::new();
        install(&fixture);
        deposit_rows(fixture.pool, &[[fixture.info, 5]]).unwrap();
        unsafe {
            destroy(fixture.pool, 1);
        }
        assert!(snapshot(fixture.pool).unwrap().is_empty());
        deposit_rows(fixture.pool, &[[fixture.info, 5]]).unwrap();
        SAVED.with(|rows| *rows.borrow_mut() = vec![("rifle_ammo".into(), 40, 0, 0)]);
        unsafe {
            load(fixture.pool, 0x1000);
        }
        assert!(snapshot(fixture.pool).unwrap().is_empty());
    }
}
