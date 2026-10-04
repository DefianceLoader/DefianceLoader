//! Bounded snapshots and conserved transfers for native squad ammunition.

use crate::equipment::ItemBounds;
#[cfg(not(test))]
use std::sync::atomic::{AtomicUsize, Ordering};

const POOL_RECORD_STRIDE: usize = 0x48;
const MAX_AMMO_ROWS: usize = 128;
const POOL_RECORDS: usize = 0x20;
const POOL_SCALE: usize = 0x48;
const CAPACITY: usize = 0x28;
const ROUNDS: usize = 0x2c;
const CARRIERS: usize = 0x34;

type ImportRounds = unsafe extern "C" fn(usize, usize, u32) -> u32;
type SetRecord = unsafe extern "C" fn(usize, usize, u32, u32, u32);

#[cfg(not(test))]
static IMPORT_ROUNDS: AtomicUsize = AtomicUsize::new(0);
#[cfg(not(test))]
static SET_RECORD: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
thread_local! {
    static TEST_IMPORT_ROUNDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TEST_SET_RECORD: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Native pickup/depot ammunition rows: AmmoScriptInfo pointer and round count.
pub(super) type Rows = Vec<[usize; 2]>;

/// One existing pool row. The ammo pointer is valid only for the current game
/// world; persisted reserves must use the ammo descriptor name instead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PoolRecord {
    pub ammo: usize,
    pub capacity: u32,
    pub rounds: u32,
    pub reserved: u32,
    pub carriers: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PoolSnapshot {
    pub scale: f32,
    pub records: Vec<PoolRecord>,
}

pub(super) fn configure(import_rounds: usize, set_record: usize) {
    #[cfg(not(test))]
    IMPORT_ROUNDS.store(import_rounds, Ordering::Release);
    #[cfg(not(test))]
    SET_RECORD.store(set_record, Ordering::Release);
    #[cfg(test)]
    TEST_IMPORT_ROUNDS.with(|address| address.set(import_rounds));
    #[cfg(test)]
    TEST_SET_RECORD.with(|address| address.set(set_record));
}

fn import_rounds_address() -> usize {
    #[cfg(test)]
    {
        TEST_IMPORT_ROUNDS.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        IMPORT_ROUNDS.load(Ordering::Acquire)
    }
}

fn set_record_address() -> usize {
    #[cfg(test)]
    {
        TEST_SET_RECORD.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        SET_RECORD.load(Ordering::Acquire)
    }
}

fn word(address: usize) -> usize {
    unsafe { core::ptr::read_unaligned(address as *const usize) }
}

fn dword(address: usize) -> u32 {
    unsafe { core::ptr::read_unaligned(address as *const u32) }
}

fn valid_pointer(pointer: usize) -> bool {
    pointer != 0 && pointer.is_multiple_of(8)
}

fn bounded_rows(bounds: ItemBounds, stride: usize) -> Result<(usize, usize), &'static str> {
    if bounds.begin == 0 && bounds.end == 0 && bounds.capacity == 0 {
        return Ok((0, 0));
    }
    if !valid_pointer(bounds.begin)
        || bounds.end < bounds.begin
        || bounds.capacity < bounds.end
        || !(bounds.end - bounds.begin).is_multiple_of(stride)
    {
        return Err("invalid ammunition vector bounds");
    }
    let count = (bounds.end - bounds.begin) / stride;
    if count > MAX_AMMO_ROWS {
        return Err("too many ammunition rows");
    }
    Ok((bounds.begin, count))
}

fn normalize_rows(rows: &[[usize; 2]]) -> Result<Rows, &'static str> {
    let mut merged = Vec::with_capacity(rows.len());
    merge(&mut merged, rows)?;
    Ok(merged)
}

/// Reads and coalesces copied `[AmmoScriptInfo*, rounds]` rows from a native
/// extraction vector. No native storage is retained after this call.
pub(super) fn read_payload(bounds: ItemBounds) -> Result<Rows, &'static str> {
    let (begin, count) = bounded_rows(bounds, 0x10)?;
    let mut rows = Vec::with_capacity(count);
    for index in 0..count {
        let row = begin + index * 0x10;
        rows.push([word(row), dword(row + 8) as usize]);
    }
    normalize_rows(&rows)
}

/// Returns the current pool state, after validating its native vector, records
/// and round/capacity invariants.
pub(super) unsafe fn pool_snapshot(pool: usize) -> Result<PoolSnapshot, &'static str> {
    if !valid_pointer(pool) || !valid_pointer(word(pool)) {
        return Err("invalid ammunition pool object");
    }
    let begin = word(pool + POOL_RECORDS);
    let end = word(pool + POOL_RECORDS + 8);
    let capacity = word(pool + POOL_RECORDS + 16);
    let (begin, count) = bounded_rows(
        ItemBounds {
            begin,
            end,
            capacity,
        },
        POOL_RECORD_STRIDE,
    )?;
    let scale = core::ptr::read_unaligned((pool + POOL_SCALE) as *const f32);
    if !scale.is_finite() || scale < 0.0 {
        return Err("invalid ammunition pool transfer scale");
    }
    let mut records = Vec::with_capacity(count);
    for index in 0..count {
        let record = begin + index * POOL_RECORD_STRIDE;
        let ammo = word(record);
        let capacity = dword(record + CAPACITY);
        let rounds = dword(record + ROUNDS);
        let reserved = dword(record + 0x30);
        let carriers = dword(record + CARRIERS);
        if !valid_pointer(ammo) || carriers == 0 || rounds > capacity || reserved > rounds {
            return Err("invalid ammunition pool record");
        }
        if records
            .iter()
            .any(|existing: &PoolRecord| existing.ammo == ammo)
        {
            return Err("duplicate ammunition pool identity");
        }
        records.push(PoolRecord {
            ammo,
            capacity,
            rounds,
            reserved,
            carriers,
        });
    }
    Ok(PoolSnapshot { scale, records })
}

/// Validates that each stock row has an unambiguous, live carrier before any
/// native rounds import is attempted.
pub(super) unsafe fn validate_pool(pool: usize) -> Result<(), &'static str> {
    let snapshot = pool_snapshot(pool)?;
    if snapshot.scale != 1.0 {
        return Err("ammunition pool uses a non-unit transfer scale");
    }
    Ok(())
}

/// Returns the unreserved, shared rounds from a validated pool snapshot.
pub(super) fn available(snapshot: &PoolSnapshot) -> Rows {
    snapshot
        .records
        .iter()
        .filter_map(|record| {
            let rounds = record.rounds - record.reserved;
            (rounds != 0).then_some([record.ammo, rounds as usize])
        })
        .collect()
}

/// Debits only unreserved shared rounds through AmmoDepotHelper vfunc+0x30
/// (`0x122520`). The native record setter preserves the loaded-reservation
/// field while updating rounds, capacity and carrier count.
pub(super) unsafe fn debit_free(pool: usize, rows: &[[usize; 2]]) -> Result<(), &'static str> {
    let snapshot = pool_snapshot(pool)?;
    if snapshot.scale != 1.0 {
        return Err("ammunition pool uses a non-unit transfer scale");
    }
    let normalized = normalize_rows(rows)?;
    let mut updates = Vec::with_capacity(normalized.len());
    for [ammo, amount] in normalized {
        let record = snapshot
            .records
            .iter()
            .find(|record| record.ammo == ammo)
            .ok_or("ammunition identity has no pool carrier")?;
        let amount = amount as u32;
        let free = record.rounds - record.reserved;
        if amount > free {
            return Err("ammunition debit exceeds unreserved rounds");
        }
        if amount != 0 {
            updates.push((
                ammo,
                record.rounds - amount,
                record.capacity,
                record.carriers,
                record.reserved,
            ));
        }
    }
    let address = set_record_address();
    if address == 0 {
        return Err("native ammunition record setter is unavailable");
    }
    let setter: SetRecord = core::mem::transmute(address);
    for &(ammo, rounds, capacity, carriers, _) in &updates {
        setter(pool, ammo, rounds, capacity, carriers);
    }
    let after = pool_snapshot(pool)?;
    for (ammo, rounds, capacity, carriers, reserved) in updates {
        let record = after
            .records
            .iter()
            .find(|record| record.ammo == ammo)
            .ok_or("ammunition record disappeared during debit")?;
        if record.rounds != rounds
            || record.capacity != capacity
            || record.carriers != carriers
            || record.reserved != reserved
        {
            return Err("native ammunition record setter changed unexpected fields");
        }
    }
    Ok(())
}

/// Adds compatible rows to an ordered payload, coalescing duplicate identities
/// with checked 32-bit round arithmetic.
pub(super) fn merge(rows: &mut Rows, incoming: &[[usize; 2]]) -> Result<(), &'static str> {
    if rows.len() > MAX_AMMO_ROWS {
        return Err("too many ammunition rows");
    }
    let mut merged = rows.clone();
    for [ammo, amount] in incoming {
        if !valid_pointer(*ammo) || *amount > u32::MAX as usize {
            return Err("invalid ammunition identity or round count");
        }
        if *amount == 0 {
            continue;
        }
        if let Some(existing) = merged.iter_mut().find(|row| row[0] == *ammo) {
            let combined = existing[1]
                .checked_add(*amount)
                .filter(|combined| *combined <= u32::MAX as usize)
                .ok_or("ammunition round count overflow")?;
            existing[1] = combined;
        } else {
            if merged.len() == MAX_AMMO_ROWS {
                return Err("too many ammunition identities");
            }
            merged.push([*ammo, *amount]);
        }
    }
    *rows = merged;
    Ok(())
}

/// Imports rounds through AmmoDepotHelper vfunc+0xa0 (`0x1229d0`). The native
/// method changes rounds only; it does not register another carrier. Unknown
/// rows fail preflight before the first mutation. The result contains only
/// unaccepted rounds, suitable for leaving on the ground pickup.
pub(super) unsafe fn import(pool: usize, rows: &[[usize; 2]]) -> Result<Rows, &'static str> {
    let snapshot = pool_snapshot(pool)?;
    if snapshot.scale != 1.0 {
        return Err("ammunition pool uses a non-unit transfer scale");
    }
    let normalized = normalize_rows(rows)?;
    for [ammo, _] in &normalized {
        if !snapshot.records.iter().any(|record| record.ammo == *ammo) {
            return Err("ammunition identity has no pool carrier");
        }
    }
    let address = import_rounds_address();
    if address == 0 {
        return Err("native rounds importer is unavailable");
    }
    let importer: ImportRounds = core::mem::transmute(address);
    let mut leftovers = Vec::with_capacity(normalized.len());
    for [ammo, requested] in normalized {
        let requested = requested as u32;
        let accepted = importer(pool, ammo, requested);
        if accepted > requested {
            return Err("native rounds importer returned an impossible count");
        }
        if accepted < requested {
            leftovers.push([ammo, (requested - accepted) as usize]);
        }
    }
    Ok(leftovers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    struct Fixture {
        blocks: Vec<Box<[usize]>>,
    }

    impl Fixture {
        fn new() -> Self {
            Self { blocks: Vec::new() }
        }

        fn alloc(&mut self, bytes: usize) -> usize {
            let block = vec![0usize; bytes.div_ceil(8).max(1)].into_boxed_slice();
            let pointer = block.as_ptr() as usize;
            self.blocks.push(block);
            pointer
        }

        fn put(&mut self, pointer: usize, offset: usize, value: usize) {
            unsafe { core::ptr::write_unaligned((pointer + offset) as *mut usize, value) }
        }

        fn put_u32(&mut self, pointer: usize, offset: usize, value: u32) {
            unsafe { core::ptr::write_unaligned((pointer + offset) as *mut u32, value) }
        }

        fn pool(&mut self, ammo: &[(usize, u32, u32, u32, u32)], scale: f32) -> usize {
            let pool = self.alloc(0x50);
            let vtable = self.alloc(8);
            let records = if ammo.is_empty() {
                0
            } else {
                self.alloc(ammo.len() * POOL_RECORD_STRIDE)
            };
            self.put(pool, 0, vtable);
            self.put(pool, POOL_RECORDS, records);
            self.put(
                pool,
                POOL_RECORDS + 8,
                records + ammo.len() * POOL_RECORD_STRIDE,
            );
            self.put(
                pool,
                POOL_RECORDS + 16,
                records + ammo.len() * POOL_RECORD_STRIDE,
            );
            unsafe { core::ptr::write_unaligned((pool + POOL_SCALE) as *mut f32, scale) };
            for (index, (identity, capacity, rounds, reserved, carriers)) in
                ammo.iter().copied().enumerate()
            {
                let record = records + index * POOL_RECORD_STRIDE;
                self.put(record, 0, identity);
                self.put_u32(record, CAPACITY, capacity);
                self.put_u32(record, ROUNDS, rounds);
                self.put_u32(record, 0x30, reserved);
                self.put_u32(record, CARRIERS, carriers);
            }
            pool
        }

        fn payload(&mut self, rows: &[[usize; 2]]) -> ItemBounds {
            let begin = if rows.is_empty() {
                0
            } else {
                self.alloc(rows.len() * 0x10)
            };
            for (index, [ammo, rounds]) in rows.iter().copied().enumerate() {
                let row = begin + index * 0x10;
                self.put(row, 0, ammo);
                self.put_u32(row, 8, rounds as u32);
            }
            ItemBounds {
                begin,
                end: begin + rows.len() * 0x10,
                capacity: begin + rows.len() * 0x10,
            }
        }
    }

    unsafe extern "C" fn mock_import(pool: usize, ammo: usize, requested: u32) -> u32 {
        let begin = word(pool + POOL_RECORDS);
        let end = word(pool + POOL_RECORDS + 8);
        let count = (end - begin) / POOL_RECORD_STRIDE;
        for index in 0..count {
            let record = begin + index * POOL_RECORD_STRIDE;
            if word(record) == ammo {
                let capacity = dword(record + CAPACITY);
                let rounds = dword(record + ROUNDS);
                let accepted = requested.min(capacity.saturating_sub(rounds));
                core::ptr::write_unaligned((record + ROUNDS) as *mut u32, rounds + accepted);
                return accepted;
            }
        }
        0
    }

    unsafe extern "C" fn mock_set_record(
        pool: usize,
        ammo: usize,
        rounds: u32,
        capacity: u32,
        carriers: u32,
    ) {
        let begin = word(pool + POOL_RECORDS);
        let end = word(pool + POOL_RECORDS + 8);
        let count = (end - begin) / POOL_RECORD_STRIDE;
        for index in 0..count {
            let record = begin + index * POOL_RECORD_STRIDE;
            if word(record) == ammo {
                core::ptr::write_unaligned((record + ROUNDS) as *mut u32, rounds);
                core::ptr::write_unaligned((record + CAPACITY) as *mut u32, capacity);
                core::ptr::write_unaligned((record + CARRIERS) as *mut u32, carriers);
                return;
            }
        }
    }

    #[test]
    fn payload_copies_and_coalesces_duplicate_ammo_ids() {
        let mut fixture = Fixture::new();
        let bounds = fixture.payload(&[[0x1000, 4], [0x2000, 7], [0x1000, 5]]);
        assert_eq!(read_payload(bounds).unwrap(), [[0x1000, 9], [0x2000, 7]]);
    }

    #[test]
    fn malformed_bounds_identities_and_round_overflow_are_rejected() {
        let mut fixture = Fixture::new();
        assert!(read_payload(ItemBounds {
            begin: 0x1000,
            end: 0x1010,
            capacity: 0x1008
        })
        .is_err());
        let bounds = fixture.payload(&[[0, 1]]);
        assert!(read_payload(bounds).is_err());
        let mut rows = vec![[0x1000, u32::MAX as usize]];
        assert!(merge(&mut rows, &[[0x1000, 1]]).is_err());
    }

    #[test]
    fn pool_snapshot_rejects_bad_records_duplicate_ids_and_nonunit_scale() {
        let mut fixture = Fixture::new();
        let ammo = fixture.alloc(8);
        let duplicate = fixture.pool(&[(ammo, 90, 20, 0, 2), (ammo, 90, 20, 0, 2)], 1.0);
        assert!(unsafe { validate_pool(duplicate) }.is_err());
        let scaled = fixture.pool(&[(ammo, 90, 20, 0, 2)], 0.5);
        assert!(unsafe { validate_pool(scaled) }.is_err());
        let invalid = fixture.pool(&[(ammo, 10, 11, 0, 1)], 1.0);
        assert!(unsafe { validate_pool(invalid) }.is_err());
        let reserved = fixture.pool(&[(ammo, 20, 5, 6, 1)], 1.0);
        assert!(unsafe { validate_pool(reserved) }.is_err());
    }

    #[test]
    fn import_respects_pool_capacity_and_returns_leftover_without_changing_carriers() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut fixture = Fixture::new();
        let ammo = fixture.alloc(8);
        let pool = fixture.pool(&[(ammo, 100, 90, 10, 3)], 1.0);
        configure(
            mock_import as *const () as usize,
            mock_set_record as *const () as usize,
        );
        let leftovers = unsafe { import(pool, &[[ammo, 25]]) }.unwrap();
        assert_eq!(leftovers, [[ammo, 15]]);
        let after = unsafe { pool_snapshot(pool) }.unwrap();
        assert_eq!(after.records[0].rounds, 100);
        assert_eq!(after.records[0].carriers, 3);
    }

    #[test]
    fn unknown_ammo_is_rejected_before_any_import() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut fixture = Fixture::new();
        let known = fixture.alloc(8);
        let unknown = fixture.alloc(8);
        let pool = fixture.pool(&[(known, 100, 10, 0, 1)], 1.0);
        configure(
            mock_import as *const () as usize,
            mock_set_record as *const () as usize,
        );
        assert!(unsafe { import(pool, &[[known, 5], [unknown, 5]]) }.is_err());
        assert_eq!(
            unsafe { pool_snapshot(pool) }.unwrap().records[0].rounds,
            10
        );
    }

    #[test]
    fn merge_preserves_order_and_sums_shared_ammo_once() {
        let mut merged = vec![[0x1000, 5], [0x2000, 8]];
        merge(&mut merged, &[[0x2000, 2], [0x3000, 4]]).unwrap();
        assert_eq!(merged, [[0x1000, 5], [0x2000, 10], [0x3000, 4]]);
    }

    #[test]
    fn debit_removes_only_free_rounds_and_preserves_loaded_reservations() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut fixture = Fixture::new();
        let ammo = fixture.alloc(8);
        let pool = fixture.pool(&[(ammo, 100, 73, 25, 4)], 1.0);
        configure(
            mock_import as *const () as usize,
            mock_set_record as *const () as usize,
        );

        let before = unsafe { pool_snapshot(pool) }.unwrap();
        let payload = available(&before);
        assert_eq!(payload, [[ammo, 48]]);
        unsafe { debit_free(pool, &payload) }.unwrap();

        let after = unsafe { pool_snapshot(pool) }.unwrap();
        assert_eq!(after.records[0].rounds, 25);
        assert_eq!(after.records[0].reserved, 25);
        assert_eq!(after.records[0].capacity, 100);
        assert_eq!(after.records[0].carriers, 4);
    }

    #[test]
    fn debit_preflights_every_row_before_mutating_pool() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut fixture = Fixture::new();
        let first = fixture.alloc(8);
        let second = fixture.alloc(8);
        let pool = fixture.pool(&[(first, 100, 40, 10, 2)], 1.0);
        configure(
            mock_import as *const () as usize,
            mock_set_record as *const () as usize,
        );

        assert!(unsafe { debit_free(pool, &[[first, 5], [second, 5]]) }.is_err());
        let after = unsafe { pool_snapshot(pool) }.unwrap();
        assert_eq!(after.records[0].rounds, 40);
        assert_eq!(after.records[0].reserved, 10);
    }
}
