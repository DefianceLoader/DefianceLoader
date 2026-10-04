//! Drop one squad's declared primary-slot loadout when its final infantry dies.

use crate::ammo;
use crate::equipment::{self, Candidate, ItemBounds};
use core::ffi::c_void;
use std::cell::RefCell;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

pub(super) static ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

/// Absolute addresses resolved from the validated build's native functions.
pub(super) struct Bindings {
    pub canonical: usize,
    pub holder_get: usize,
    pub item_override: usize,
    pub slot_context: usize,
    pub slot_type: usize,
    pub ammo_mode: usize,
    pub weak_bind: usize,
    pub manager_get: usize,
    pub spawn: usize,
    pub human_vtable: usize,
}

static CANONICAL: AtomicUsize = AtomicUsize::new(0);
static HOLDER_GET: AtomicUsize = AtomicUsize::new(0);
static ITEM_OVERRIDE: AtomicUsize = AtomicUsize::new(0);
static SLOT_CONTEXT: AtomicUsize = AtomicUsize::new(0);
static SLOT_TYPE: AtomicUsize = AtomicUsize::new(0);
static AMMO_MODE: AtomicUsize = AtomicUsize::new(0);
static WEAK_BIND: AtomicUsize = AtomicUsize::new(0);
static MANAGER_GET: AtomicUsize = AtomicUsize::new(0);
static SPAWN: AtomicUsize = AtomicUsize::new(0);
static HUMAN_VTABLE: AtomicUsize = AtomicUsize::new(0);

type Unary = unsafe extern "C" fn(usize) -> usize;
type Binary = unsafe extern "C" fn(usize, usize) -> usize;
type Pair = unsafe extern "C" fn(*mut usize, usize);
type Slot = unsafe extern "C" fn(usize, usize) -> i32;
type Original = unsafe extern "C" fn(usize, usize);
type Spawn = unsafe extern "C" fn(
    usize,
    usize,
    *const [f32; 3],
    *const [f32; 4],
    usize,
    *const [usize; 3],
    bool,
    i32,
);

thread_local! {
    /// Suppress duplicate snapshots for reentry on the same entity only.
    static IN_FLIGHT: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

struct Flight(usize);

impl Flight {
    fn enter(entity: usize) -> Option<Self> {
        if entity == 0 {
            return None;
        }
        IN_FLIGHT.with(|entries| {
            let mut entries = entries.borrow_mut();
            if entries.contains(&entity) {
                return None;
            }
            entries.push(entity);
            Some(Self(entity))
        })
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        IN_FLIGHT.with(|entries| {
            let mut entries = entries.borrow_mut();
            if let Some(index) = entries.iter().position(|entity| *entity == self.0) {
                entries.remove(index);
            }
        });
    }
}

pub(super) fn configure(bindings: Bindings) {
    CANONICAL.store(bindings.canonical, Ordering::Release);
    HOLDER_GET.store(bindings.holder_get, Ordering::Release);
    ITEM_OVERRIDE.store(bindings.item_override, Ordering::Release);
    SLOT_CONTEXT.store(bindings.slot_context, Ordering::Release);
    SLOT_TYPE.store(bindings.slot_type, Ordering::Release);
    AMMO_MODE.store(bindings.ammo_mode, Ordering::Release);
    WEAK_BIND.store(bindings.weak_bind, Ordering::Release);
    MANAGER_GET.store(bindings.manager_get, Ordering::Release);
    SPAWN.store(bindings.spawn, Ordering::Release);
    HUMAN_VTABLE.store(bindings.human_vtable, Ordering::Release);
}

/// Owns a native weak junction so teardown cannot leave a captured raw pointer
/// that may alias a later object allocation.
struct WeakHandle(usize);

impl WeakHandle {
    unsafe fn new(target: usize) -> Option<Self> {
        if target == 0 {
            return None;
        }
        let address = WEAK_BIND.load(Ordering::Acquire);
        if address == 0 {
            return None;
        }
        let bind: Pair = core::mem::transmute(address);
        let mut handle = 0;
        bind(&mut handle, target);
        (handle != 0).then_some(Self(handle))
    }

    unsafe fn get(&self) -> usize {
        if self.0 == 0 {
            0
        } else {
            word(self.0, 0x10)
        }
    }
}

impl Drop for WeakHandle {
    fn drop(&mut self) {
        unsafe {
            let address = WEAK_BIND.load(Ordering::Acquire);
            if address != 0 && self.0 != 0 {
                let bind: Pair = core::mem::transmute(address);
                bind(&mut self.0, 0);
            }
        }
    }
}

unsafe fn word(object: usize, offset: usize) -> usize {
    core::ptr::read_unaligned((object + offset) as *const usize)
}

unsafe fn virtual_unary(object: usize, offset: usize) -> Option<usize> {
    if object == 0 {
        return None;
    }
    let table = word(object, 0);
    if table == 0 {
        return None;
    }
    let address = word(table, offset);
    if address == 0 {
        return None;
    }
    let function: Unary = core::mem::transmute(address);
    Some(function(object))
}

struct Snapshot {
    holder: WeakHandle,
    manager: usize,
    pool: Option<WeakHandle>,
    items: Vec<PrimaryDrop>,
    position: [f32; 3],
    rotation: [f32; 4],
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PrimaryDrop {
    item: usize,
    ammo_types: Vec<usize>,
}

unsafe fn bounded_vector(
    object: usize,
    offset: usize,
    stride: usize,
    limit: usize,
) -> Option<ItemBounds> {
    let begin = word(object, offset);
    let end = word(object, offset + 8);
    let capacity = word(object, offset + 16);
    if begin == 0
        || !begin.is_multiple_of(8)
        || end < begin
        || capacity < end
        || !(end - begin).is_multiple_of(stride)
        || (end - begin) / stride > limit
    {
        return None;
    }
    Some(ItemBounds {
        begin,
        end,
        capacity,
    })
}

unsafe fn human_gunner(entity: usize, facets: usize) -> Option<usize> {
    if entity == 0 || facets == 0 {
        return None;
    }
    let ai = word(facets, 0x28);
    let reference = if ai == 0 { 0 } else { word(ai, 0x1f0) };
    let gunner = if reference == 0 {
        0
    } else {
        word(reference, 0x10)
    };
    if gunner == 0
        || word(gunner, 0) != HUMAN_VTABLE.load(Ordering::Acquire)
        || word(gunner, 0x20) != entity
    {
        return None;
    }
    Some(gunner)
}

unsafe fn declared_items(gunner: usize, context: usize) -> Option<Vec<PrimaryDrop>> {
    // Resolve script metadata from the canonical squad context. The member's
    // live inventory is authoritative; cached holder pairs can lag a live swap.
    let script = virtual_unary(context, 0x50)?;
    if script == 0 || !script.is_multiple_of(8) {
        return None;
    }
    let items = bounded_vector(gunner, 0x50, 8, 128)?;
    let candidates = equipment::compatible_primary_items(script, items).ok()?;
    let resolver: Binary = core::mem::transmute(ITEM_OVERRIDE.load(Ordering::Acquire));
    let slot_type: Slot = core::mem::transmute(SLOT_TYPE.load(Ordering::Acquire));
    let ammo_mode: Unary = core::mem::transmute(AMMO_MODE.load(Ordering::Acquire));
    let drops = eligible_slot_items(&candidates, script, context, resolver, slot_type, ammo_mode);
    (!drops.is_empty()).then_some(drops)
}

unsafe fn eligible_slot_items(
    candidates: &[Candidate],
    script: usize,
    context: usize,
    resolver: Binary,
    slot_type: Slot,
    ammo_mode: Unary,
) -> Vec<PrimaryDrop> {
    let mut visited = Vec::with_capacity(candidates.len());
    let mut drops = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let slot_index = candidate.slot_index;
        if visited.contains(&slot_index) {
            continue;
        }
        visited.push(slot_index);
        let mut eligible = candidates.iter().filter_map(|entry| {
            (entry.slot_index == slot_index)
                .then(|| eligible_item(entry.item, script, context, resolver, slot_type, ammo_mode))
                .flatten()
        });
        let Some(drop) = eligible.next() else {
            continue;
        };
        if eligible.next().is_none() {
            // Each declared slot produces exactly one item only when its live
            // inventory mapping is unambiguous. Keep duplicates across slots.
            drops.push(drop);
        }
    }
    drops
}

unsafe fn eligible_item(
    item: usize,
    script: usize,
    context: usize,
    resolver: Binary,
    slot_type: Slot,
    ammo_mode: Unary,
) -> Option<PrimaryDrop> {
    if item == 0 || !item.is_multiple_of(8) || *((item + 0x2c) as *const u32) != 1 {
        return None;
    }
    let default = word(item, 0x140);
    if default == 0 {
        return None;
    }
    let effective = if context == 0 {
        default
    } else {
        resolver(item, script + 8)
    };
    if effective == 0 {
        return None;
    }
    let gun = word(effective, 0xb8);
    let default_gun = word(default, 0xb8);
    if gun == 0 || default_gun == 0 || gun != default_gun || slot_type(gun, context) != 0 {
        return None;
    }
    let mesh_string_len = word(gun, 0x1b8);
    if mesh_string_len == 0 || mesh_string_len > 1024 || *((gun + 0xa9) as *const u8) != 0 {
        return None;
    }
    let ammo_offset = if ammo_mode(context) as u8 != 0 {
        0x228
    } else {
        0x210
    };
    let begin = word(gun, ammo_offset);
    let end = word(gun, ammo_offset + 8);
    let capacity = word(gun, ammo_offset + 16);
    let mut ammo_types = Vec::new();
    if begin != 0 || end != 0 || capacity != 0 {
        let ammo_rows = bounded_vector(gun, ammo_offset, 0x10, 128)?;
        for index in 0..(ammo_rows.end - ammo_rows.begin) / 0x10 {
            let ammo = word(ammo_rows.begin, index * 0x10);
            if ammo == 0 || !ammo.is_multiple_of(8) {
                return None;
            }
            if !ammo_types.contains(&ammo) {
                ammo_types.push(ammo);
            }
        }
    }
    Some(PrimaryDrop { item, ammo_types })
}

unsafe fn snapshot(entity: usize, damageable: usize) -> Option<Snapshot> {
    if damageable == 0
        || !death_eligible(
            *((damageable + 0x18) as *const u8),
            *((damageable + 0x5c) as *const i32),
        )
    {
        return None;
    }
    let junction = word(damageable, 0x10);
    if junction == 0 || word(junction, 0x10) != entity {
        return None;
    }
    let facets = virtual_unary(entity, 0xb0)?;
    let gunner = human_gunner(entity, facets)?;
    let slot_context: Unary = core::mem::transmute(SLOT_CONTEXT.load(Ordering::Acquire));
    let owner_context = slot_context(gunner);
    if owner_context == 0 {
        return None;
    }
    let canonical_fn: Unary = core::mem::transmute(CANONICAL.load(Ordering::Acquire));
    let canonical = canonical_fn(owner_context);
    if canonical == 0 {
        return None;
    }
    let holder_fn: Unary = core::mem::transmute(HOLDER_GET.load(Ordering::Acquire));
    let holder = holder_fn(canonical);
    if holder == 0 {
        return None;
    }
    let roster = bounded_vector(holder, 0xa0, 8, 64)?;
    let roster_count = (roster.end - roster.begin) / 8;
    let mut contains_entity = false;
    for index in 0..roster_count {
        if word(roster.begin, index * 8) == entity {
            if contains_entity {
                return None;
            }
            contains_entity = true;
        }
    }
    if !contains_entity {
        return None;
    }
    let holder = WeakHandle::new(holder)?;
    let items = declared_items(gunner, canonical)?;
    let position_facet = word(damageable, 0x20);
    let position = virtual_unary(position_facet, 0x58)?;
    let rotation = virtual_unary(position_facet, 0x60)?;
    if position == 0 || rotation == 0 {
        return None;
    }
    let position = core::ptr::read_unaligned(position as *const [f32; 3]);
    let rotation = core::ptr::read_unaligned(rotation as *const [f32; 4]);
    let norm = rotation.iter().map(|value| value * value).sum::<f32>();
    if !position.into_iter().chain(rotation).all(f32::is_finite)
        || !norm.is_finite()
        || norm < 0.000001
    {
        return None;
    }
    let world = virtual_unary(entity, 0x78)?;
    if world == 0 {
        return None;
    }
    let get_manager: Unary = core::mem::transmute(MANAGER_GET.load(Ordering::Acquire));
    let manager = get_manager(world);
    let ai = word(facets, 0x28);
    let pool_junction = if ai == 0 { 0 } else { word(ai, 0x148) };
    let pool = if pool_junction == 0 {
        None
    } else {
        WeakHandle::new(word(pool_junction, 0x10))
    };
    (manager != 0).then_some(Snapshot {
        holder,
        manager,
        pool,
        items,
        position,
        rotation,
    })
}

/// Checks the post-bookkeeping roster state after the current damageable's
/// active byte has been cleared. Unknown members fail closed.
unsafe fn has_active_infantry_peer(holder: usize, current: usize) -> bool {
    let Some(roster) = bounded_vector(holder, 0xa0, 8, 64) else {
        // A malformed live vector must fail closed rather than create duplicates.
        return true;
    };
    let mut active_peers = Vec::new();
    for index in 0..(roster.end - roster.begin) / 8 {
        let entity = word(roster.begin, index * 8);
        if entity == current {
            continue;
        }
        if entity == 0 {
            return true;
        }
        let Some(facets) = virtual_unary(entity, 0xb0) else {
            return true;
        };
        let damageable = word(facets, 0x18);
        if damageable == 0 {
            return true;
        }
        let junction = word(damageable, 0x10);
        if junction == 0 || word(junction, 0x10) != entity {
            return true;
        }
        if *((damageable + 0x18) as *const u8) != 0 {
            active_peers.push(true);
        }
    }
    !final_batch_ready(active_peers.into_iter())
}

fn death_eligible(active: u8, health: i32) -> bool {
    active != 0 && health <= 0
}

fn final_batch_ready(mut peers: impl Iterator<Item = bool>) -> bool {
    !peers.any(|active| active)
}

unsafe fn emit_drops(snapshot: &Snapshot, spawn: Spawn) {
    let payloads = if crate::policy::current().death_ammo == crate::policy::DeathAmmo::Transfer {
        transfer_ammo(snapshot)
    } else {
        vec![Vec::new(); snapshot.items.len()]
    };
    for (item, rows) in snapshot.items.iter().zip(payloads) {
        let mut bounds = [0usize; 3];
        if !rows.is_empty() {
            let begin = rows.as_ptr() as usize;
            let end = begin + std::mem::size_of_val(rows.as_slice());
            bounds = [begin, end, end];
        }
        spawn(
            snapshot.manager,
            0,
            &snapshot.position,
            &snapshot.rotation,
            item.item,
            &bounds,
            false,
            -1,
        );
    }
}

fn allocate_ammo(
    drops: &[PrimaryDrop],
    pool: &ammo::PoolSnapshot,
) -> (Vec<ammo::Rows>, ammo::Rows) {
    let mut payloads = vec![Vec::new(); drops.len()];
    let mut debit = Vec::new();
    for [identity, rounds] in ammo::available(pool) {
        if let Some(index) = drops
            .iter()
            .position(|drop| drop.ammo_types.contains(&identity))
        {
            payloads[index].push([identity, rounds]);
            debit.push([identity, rounds]);
        }
    }
    (payloads, debit)
}

unsafe fn transfer_ammo(snapshot: &Snapshot) -> Vec<ammo::Rows> {
    // Only shared rounds outside loaded-magazine reservations transfer. The
    // native record setter leaves reservations and carrier counts unchanged.
    let empty = || vec![Vec::new(); snapshot.items.len()];
    let Some(pool) = snapshot.pool.as_ref().map(|weak| weak.get()) else {
        return empty();
    };
    if pool == 0 {
        return empty();
    }
    let Ok(pool_state) = ammo::pool_snapshot(pool) else {
        return empty();
    };
    let (payloads, debit) = allocate_ammo(&snapshot.items, &pool_state);
    if debit.is_empty() || ammo::debit_free(pool, &debit).is_err() {
        return empty();
    }
    payloads
}

/// The guarded Steam call site supplies its damageable facet in RDI. This
/// tail shim preserves the native arguments and exposes RDI as the third one.
#[unsafe(naked)]
pub unsafe extern "C" fn primary_death(_manager: usize, _entity: usize) {
    core::arch::naked_asm!("mov r8, rdi", "jmp {}", sym death);
}

unsafe extern "C" fn death(manager: usize, entity: usize, damageable: usize) {
    let flight = Flight::enter(entity);
    let drop = flight.as_ref().and_then(|_| snapshot(entity, damageable));
    let original: Original = core::mem::transmute(ORIGINAL.load(Ordering::Acquire));
    original(manager, entity);

    if let Some(drop) = drop {
        // The caller performs this same store immediately after the hooked
        // call. Mirroring it here lets nested callbacks observe completion.
        core::ptr::write_volatile((damageable + 0x18) as *mut u8, 0);
        let holder = drop.holder.get();
        if holder != 0 && !has_active_infantry_peer(holder, entity) {
            let spawn: Spawn = core::mem::transmute(SPAWN.load(Ordering::Acquire));
            emit_drops(&drop, spawn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::Mutex;

    static ENGINE_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Debug, PartialEq)]
    enum Event {
        Original(usize, usize),
        Drop(
            usize,
            usize,
            [f32; 3],
            [f32; 4],
            usize,
            Vec<[usize; 2]>,
            bool,
            i32,
        ),
    }

    unsafe extern "C" fn default_resolver(item: usize, _script_key: usize) -> usize {
        word(item, 0x140)
    }

    unsafe extern "C" fn override_resolver(item: usize, script_key: usize) -> usize {
        assert_eq!(script_key, SCRIPT_KEY.with(Cell::get));
        RESOLVED.with(|state| state.set(script_key));
        let override_info = word(item, 0x148);
        if override_info == 0 {
            word(item, 0x140)
        } else {
            override_info
        }
    }

    unsafe extern "C" fn test_slot_type(gun: usize, _context: usize) -> i32 {
        *((gun + 0x108) as *const i32)
    }

    unsafe extern "C" fn test_spawn(
        manager: usize,
        source: usize,
        position: *const [f32; 3],
        rotation: *const [f32; 4],
        item: usize,
        ammo: *const [usize; 3],
        follow: bool,
        id: i32,
    ) {
        let bounds = *ammo;
        let rows = if bounds[0] == 0 && bounds[1] == 0 && bounds[2] == 0 {
            Vec::new()
        } else {
            assert!(bounds[1] >= bounds[0]);
            let count = (bounds[1] - bounds[0]) / 0x10;
            assert_eq!((bounds[1] - bounds[0]) % 0x10, 0);
            unsafe { std::slice::from_raw_parts(bounds[0] as *const [usize; 2], count) }.to_vec()
        };
        EVENTS.with(|events| {
            events.borrow_mut().push(Event::Drop(
                manager, source, *position, *rotation, item, rows, follow, id,
            ));
        });
    }

    thread_local! {
        static RESOLVED: Cell<usize> = const { Cell::new(0) };
        static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
        static CONTEXT: Cell<usize> = const { Cell::new(0) };
        static HOLDER: Cell<usize> = const { Cell::new(0) };
        static SCRIPT_KEY: Cell<usize> = const { Cell::new(0) };
        static NESTED: RefCell<Option<(usize, usize, usize)>> = const { RefCell::new(None) };
        static NATIVE_POOL_ROUNDS: Cell<Option<(usize, u32)>> = const { Cell::new(None) };
    }

    unsafe extern "C" fn test_facets(entity: usize) -> usize {
        word(entity, 8)
    }

    unsafe extern "C" fn test_script(context: usize) -> usize {
        word(context, 0x18)
    }

    unsafe extern "C" fn test_world(entity: usize) -> usize {
        word(entity, 0x10)
    }

    unsafe extern "C" fn test_position(facet: usize) -> usize {
        word(facet, 8)
    }

    unsafe extern "C" fn test_rotation(facet: usize) -> usize {
        word(facet, 0x10)
    }

    unsafe extern "C" fn test_canonical(context: usize) -> usize {
        context
    }

    unsafe extern "C" fn test_holder(context: usize) -> usize {
        assert_eq!(context, CONTEXT.with(Cell::get));
        HOLDER.with(Cell::get)
    }

    unsafe extern "C" fn test_slot_context(_gunner: usize) -> usize {
        CONTEXT.with(Cell::get)
    }

    unsafe extern "C" fn test_slot(gun: usize, context: usize) -> i32 {
        assert_eq!(context, CONTEXT.with(Cell::get));
        *((gun + 0x108) as *const i32)
    }

    unsafe extern "C" fn test_ammo_mode(_context: usize) -> usize {
        0
    }

    unsafe extern "C" fn test_weak_bind(destination: *mut usize, target: usize) {
        core::ptr::write_unaligned(destination, target);
    }

    unsafe extern "C" fn test_set_record(
        pool: usize,
        ammo: usize,
        rounds: u32,
        capacity: u32,
        carriers: u32,
    ) {
        let begin = word(pool, 0x20);
        let end = word(pool, 0x28);
        let count = (end - begin) / 0x48;
        for index in 0..count {
            let record = begin + index * 0x48;
            if word(record, 0) == ammo {
                core::ptr::write_unaligned((record + 0x2c) as *mut u32, rounds);
                core::ptr::write_unaligned((record + 0x28) as *mut u32, capacity);
                core::ptr::write_unaligned((record + 0x34) as *mut u32, carriers);
                return;
            }
        }
    }

    unsafe extern "C" fn test_manager(_world: usize) -> usize {
        0x9000
    }

    unsafe extern "C" fn test_original(manager: usize, entity: usize) {
        EVENTS.with(|events| events.borrow_mut().push(Event::Original(manager, entity)));
        if let Some((pool, rounds)) = NATIVE_POOL_ROUNDS.with(Cell::take) {
            let record = word(pool, 0x20);
            core::ptr::write_unaligned((record + 0x2c) as *mut u32, rounds);
        }
        let nested = NESTED.with(|state| state.borrow_mut().take());
        if let Some((nested_manager, nested_entity, nested_damageable)) = nested {
            core::ptr::write_unaligned((nested_damageable + 0x5c) as *mut i32, 0);
            death(nested_manager, nested_entity, nested_damageable);
            // The native caller performs this same guarded store after the
            // nested call site returns.
            core::ptr::write_volatile((nested_damageable + 0x18) as *mut u8, 0);
        }
    }

    struct Fixture {
        #[allow(
            clippy::vec_box,
            reason = "Native pointers must remain stable as fixture storage grows."
        )]
        blocks: Vec<Box<[usize; 128]>>,
        context: usize,
        script: usize,
        holder: usize,
        pool: usize,
        rifle_ammo: usize,
        members: [usize; 2],
        damageables: [usize; 2],
        live_items: [usize; 2],
    }

    impl Fixture {
        fn new() -> Self {
            let mut fixture = Self {
                blocks: Vec::new(),
                context: 0,
                script: 0,
                holder: 0,
                pool: 0,
                rifle_ammo: 0,
                members: [0; 2],
                damageables: [0; 2],
                live_items: [0; 2],
            };
            let table = fixture.alloc();
            fixture.put(table, 0xb0, test_facets as *const () as usize);
            fixture.put(table, 0x50, test_script as *const () as usize);
            fixture.put(table, 0x78, test_world as *const () as usize);

            fixture.context = fixture.alloc();
            fixture.put(fixture.context, 0, table);
            fixture.script = fixture.alloc();
            fixture.put(fixture.context, 0x18, fixture.script);
            let script = fixture.script;
            let slot_vector = fixture.alloc();

            let rifle_default = fixture.alloc();
            let pistol_default = fixture.alloc();
            let shotgun_default = fixture.alloc();
            let slot_categories = ["rifles", "pistols", "shotguns"];
            let defaults = [rifle_default, pistol_default, shotgun_default];
            for (index, category) in slot_categories.into_iter().enumerate() {
                let slot = fixture.alloc();
                fixture.put_string(slot + 0x28, category);
                fixture.put_u32(slot + 0x70, 0);
                fixture.put(slot + 0x68, 0, defaults[index]);
                fixture.put(slot_vector, index * 8, slot);
            }
            fixture.put(script, 0x228, slot_vector);
            fixture.put(script, 0x230, slot_vector + 3 * 8);
            fixture.put(script, 0x238, slot_vector + 3 * 8);

            let rifle_ammo = fixture.alloc();
            fixture.rifle_ammo = rifle_ammo;
            let shotgun_ammo = fixture.alloc();
            let rifle_gun = fixture.make_gun_with_ammo(rifle_ammo);
            let pistol_gun = fixture.make_gun_with_ammo(rifle_ammo);
            let shotgun_gun = fixture.make_gun_with_ammo(shotgun_ammo);
            let rifle_default_info = fixture.make_gun_info(rifle_gun);
            let rifle_override_info = fixture.make_gun_info(rifle_gun);
            let _pistol_info = fixture.make_gun_info(pistol_gun);
            let shotgun_info = fixture.make_gun_info(shotgun_gun);
            let shared_item = fixture.make_item(
                &["rifles", "pistols"],
                rifle_default_info,
                Some(rifle_override_info),
            );
            let shotgun_item = fixture.make_item(&["shotguns"], shotgun_info, None);
            let special_item = fixture.make_item(&["hand_grenade"], 0, None);
            let item_vector = fixture.alloc();
            fixture.put(item_vector, 0, shared_item);
            fixture.put(item_vector, 8, shotgun_item);
            fixture.put(item_vector, 16, special_item);
            fixture.live_items = [shared_item, shotgun_item];

            let roster = fixture.alloc();
            fixture.holder = fixture.alloc();
            fixture.put(fixture.holder, 0xa0, roster);
            fixture.put(fixture.holder, 0xa8, roster + 2 * 8);
            fixture.put(fixture.holder, 0xb0, roster + 2 * 8);
            fixture.put(fixture.holder, 0xc0, script);
            fixture.put(fixture.holder, 0x10, fixture.holder);

            fixture.pool = fixture.make_pool(rifle_ammo, 100, 70, 20, 2);
            let pool_junction = fixture.alloc();
            fixture.put(pool_junction, 0x10, fixture.pool);

            for index in 0..2 {
                let entity = fixture.alloc();
                let facets = fixture.alloc();
                let ai = fixture.alloc();
                let gunner_ref = fixture.alloc();
                let gunner = fixture.alloc();
                let junction = fixture.alloc();
                let damageable = fixture.alloc();
                let position_facet = fixture.alloc();
                let position_table = fixture.alloc();
                let position = fixture.alloc();
                let rotation = fixture.alloc();

                fixture.put(entity, 0, table);
                fixture.put(entity, 8, facets);
                fixture.put(entity, 0x10, 0x8000 + index * 0x100);
                fixture.put(facets, 0x18, damageable);
                fixture.put(facets, 0x28, ai);
                fixture.put(ai, 0x148, pool_junction);
                fixture.put(ai, 0x1f0, gunner_ref);
                fixture.put(gunner_ref, 0x10, gunner);
                fixture.put(gunner, 0, 0x123456);
                fixture.put(gunner, 0x20, entity);
                fixture.put(gunner, 0x50, item_vector);
                fixture.put(gunner, 0x58, item_vector + 3 * 8);
                fixture.put(gunner, 0x60, item_vector + 3 * 8);

                fixture.put(junction, 0x10, entity);
                fixture.put(damageable, 0x10, junction);
                fixture.put(damageable, 0x20, position_facet);
                fixture.put_byte(damageable + 0x18, 1);
                fixture.put_u32(damageable + 0x5c, 100);

                fixture.put(position_facet, 0, position_table);
                fixture.put(position_facet, 8, position);
                fixture.put(position_facet, 0x10, rotation);
                fixture.put(position_table, 0x58, test_position as *const () as usize);
                fixture.put(position_table, 0x60, test_rotation as *const () as usize);
                fixture.put_f32x3(position, [index as f32 + 1.0, 2.0, 3.0]);
                fixture.put_f32x4(rotation, [0.0, 0.0, 0.0, 1.0]);

                fixture.put(roster, index * 8, entity);
                fixture.members[index] = entity;
                fixture.damageables[index] = damageable;
            }
            CONTEXT.with(|state| state.set(fixture.context));
            HOLDER.with(|state| state.set(fixture.holder));
            SCRIPT_KEY.with(|state| state.set(fixture.script + 8));
            fixture
        }

        fn alloc(&mut self) -> usize {
            let block = Box::new([0usize; 128]);
            let address = block.as_ptr() as usize;
            self.blocks.push(block);
            address
        }

        fn put(&mut self, object: usize, offset: usize, value: usize) {
            unsafe { core::ptr::write_unaligned((object + offset) as *mut usize, value) }
        }

        fn put_u32(&mut self, object: usize, value: u32) {
            unsafe { core::ptr::write_unaligned(object as *mut u32, value) }
        }

        fn put_byte(&mut self, object: usize, value: u8) {
            unsafe { core::ptr::write_volatile(object as *mut u8, value) }
        }

        fn put_string(&mut self, object: usize, value: &str) {
            assert!(value.len() <= 15);
            unsafe {
                core::ptr::copy_nonoverlapping(value.as_ptr(), object as *mut u8, value.len());
                core::ptr::write_unaligned((object + 0x10) as *mut usize, value.len());
                core::ptr::write_unaligned((object + 0x18) as *mut usize, 15);
            }
        }

        fn put_f32x3(&mut self, object: usize, value: [f32; 3]) {
            unsafe { core::ptr::write_unaligned(object as *mut [f32; 3], value) }
        }

        fn put_f32x4(&mut self, object: usize, value: [f32; 4]) {
            unsafe { core::ptr::write_unaligned(object as *mut [f32; 4], value) }
        }

        fn make_gun(&mut self) -> usize {
            let gun = self.alloc();
            self.put(gun, 0x1b8, 12);
            self.put_byte(gun + 0xa9, 0);
            self.put_u32(gun + 0x108, 0);
            gun
        }

        fn make_gun_with_ammo(&mut self, ammo: usize) -> usize {
            let gun = self.make_gun();
            let rows = self.alloc();
            self.put(rows, 0, ammo);
            unsafe { core::ptr::write_unaligned((rows + 8) as *mut f32, 1.0) };
            self.put(gun, 0x210, rows);
            self.put(gun, 0x218, rows + 0x10);
            self.put(gun, 0x220, rows + 0x10);
            gun
        }

        fn make_pool(
            &mut self,
            ammo: usize,
            capacity: u32,
            rounds: u32,
            reserved: u32,
            carriers: u32,
        ) -> usize {
            let pool = self.alloc();
            let vtable = self.alloc();
            let record = self.alloc();
            self.put(pool, 0, vtable);
            self.put(pool, 0x10, pool);
            self.put(pool, 0x20, record);
            self.put(pool, 0x28, record + 0x48);
            self.put(pool, 0x30, record + 0x48);
            self.put(record, 0, ammo);
            self.put_u32(record + 0x28, capacity);
            self.put_u32(record + 0x2c, rounds);
            self.put_u32(record + 0x30, reserved);
            self.put_u32(record + 0x34, carriers);
            unsafe { core::ptr::write_unaligned((pool + 0x48) as *mut f32, 1.0) };
            pool
        }

        fn make_gun_info(&mut self, gun: usize) -> usize {
            let info = self.alloc();
            self.put(info, 0xb8, gun);
            info
        }

        fn make_item(
            &mut self,
            categories: &[&str],
            default: usize,
            override_info: Option<usize>,
        ) -> usize {
            let item = self.alloc();
            self.put_u32(item + 0x2c, 1);
            self.put(item, 0x140, default);
            if let Some(info) = override_info {
                self.put(item, 0x148, info);
            }
            let types = self.alloc();
            for (index, category) in categories.iter().enumerate() {
                self.put_string(types + index * 0x20, category);
            }
            self.put(item, 0x30, types);
            self.put(item, 0x38, types + categories.len() * 0x20);
            self.put(item, 0x40, types + categories.len() * 0x20);
            item
        }

        fn death(&self, index: usize) {
            unsafe {
                core::ptr::write_volatile((self.damageables[index] + 0x18) as *mut u8, 1);
                core::ptr::write_unaligned((self.damageables[index] + 0x5c) as *mut i32, 0);
                death(0x7000, self.members[index], self.damageables[index]);
            }
        }
    }

    fn configure_test_engine(fixture: &Fixture) {
        ORIGINAL.store(test_original as *mut c_void, Ordering::Release);
        ammo::configure(0, test_set_record as *const () as usize);
        configure(Bindings {
            canonical: test_canonical as *const () as usize,
            holder_get: test_holder as *const () as usize,
            item_override: override_resolver as *const () as usize,
            slot_context: test_slot_context as *const () as usize,
            slot_type: test_slot as *const () as usize,
            ammo_mode: test_ammo_mode as *const () as usize,
            weak_bind: test_weak_bind as *const () as usize,
            manager_get: test_manager as *const () as usize,
            spawn: test_spawn as *const () as usize,
            human_vtable: 0x123456,
        });
        CONTEXT.with(|state| state.set(fixture.context));
        HOLDER.with(|state| state.set(fixture.holder));
        SCRIPT_KEY.with(|state| state.set(fixture.script + 8));
        NESTED.with(|state| *state.borrow_mut() = None);
        NATIVE_POOL_ROUNDS.with(|state| state.set(None));
        EVENTS.with(|events| events.borrow_mut().clear());
    }

    #[test]
    fn wrapper_snapshots_before_native_and_emits_only_after_last_active_clear() {
        let _lock = ENGINE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fixture = Fixture::new();
        let _policy = crate::policy::set_for_test(crate::policy::Settings {
            ammo_policy: crate::policy::AmmoPolicy::Discard,
            death_ammo: crate::policy::DeathAmmo::Transfer,
        });
        configure_test_engine(&fixture);
        assert_eq!(
            unsafe { word(fixture.holder, 0xa8) - word(fixture.holder, 0xa0) },
            16
        );
        let peer_facets = unsafe { virtual_unary(fixture.members[1], 0xb0).unwrap() };
        let peer_damageable = unsafe { word(peer_facets, 0x18) };
        assert_eq!(peer_damageable, fixture.damageables[1]);
        let peer_junction = unsafe { word(peer_damageable, 0x10) };
        assert_eq!(unsafe { word(peer_junction, 0x10) }, fixture.members[1]);
        assert_eq!(unsafe { word(fixture.damageables[1], 0x18) }, 1);
        assert_eq!(
            unsafe { *((fixture.damageables[1] + 0x18) as *const u8) },
            1
        );
        assert!(!final_batch_ready([true].into_iter()));
        assert!(unsafe { has_active_infantry_peer(fixture.holder, fixture.members[0]) });

        // The first member's loss preserves the native queue and leaves the
        // second active, so it emits no batch.
        fixture.death(0);
        assert_eq!(
            EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut())),
            [Event::Original(0x7000, fixture.members[0])]
        );
        assert_eq!(
            unsafe { *((fixture.damageables[0] + 0x18) as *const u8) },
            0
        );
        assert_eq!(
            unsafe { core::ptr::read_unaligned((word(fixture.pool, 0x20) + 0x2c) as *const u32) },
            70
        );

        // The final member's callback uses the live inventory metadata and
        // cached defaults differ: the resulting drops are current selections.
        // Model a stock drop removing five shared rounds in the original
        // callback; only the remaining free pool is transferred afterward.
        NATIVE_POOL_ROUNDS.with(|state| state.set(Some((fixture.pool, 65))));
        fixture.death(1);
        let events = EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
        assert_eq!(events[0], Event::Original(0x7000, fixture.members[1]));
        assert_eq!(events.len(), 4);
        let mut dropped = Vec::new();
        for event in &events[1..] {
            let Event::Drop(manager, source, position, rotation, item, ammo, follow, id) = event
            else {
                panic!("expected pickup event");
            };
            assert_eq!(*manager, 0x9000);
            assert_eq!(*source, 0);
            assert_eq!(*position, [2.0, 2.0, 3.0]);
            assert_eq!(*rotation, [0.0, 0.0, 0.0, 1.0]);
            let expected = if *item == fixture.live_items[0] && dropped.is_empty() {
                vec![[fixture.rifle_ammo, 45]]
            } else {
                vec![]
            };
            assert_eq!(*ammo, expected);
            assert!(!follow);
            assert_eq!(*id, -1);
            dropped.push(*item);
        }
        assert_eq!(RESOLVED.with(Cell::get), fixture.script + 8);
        assert_eq!(
            dropped,
            [
                fixture.live_items[0],
                fixture.live_items[0],
                fixture.live_items[1]
            ]
        );
        let after =
            unsafe { core::ptr::read_unaligned((word(fixture.pool, 0x20) + 0x2c) as *const u32) };
        let reserved =
            unsafe { core::ptr::read_unaligned((word(fixture.pool, 0x20) + 0x30) as *const u32) };
        assert_eq!((after, reserved), (20, 20));

        // A repeated post-mortem callback still reaches native bookkeeping,
        // but the consumed active byte prevents a second loadout.
        unsafe { death(0x7000, fixture.members[1], fixture.damageables[1]) };
        assert_eq!(
            EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut())),
            [Event::Original(0x7000, fixture.members[1])]
        );
    }

    #[test]
    fn discard_death_policy_emits_empty_drops_without_debiting_pool() {
        let _lock = ENGINE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fixture = Fixture::new();
        let _policy = crate::policy::set_for_test(crate::policy::Settings::default());
        configure_test_engine(&fixture);
        fixture.death(0);
        fixture.death(1);
        let events = EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
        let drops = events
            .iter()
            .filter_map(|event| match event {
                Event::Drop(_, _, _, _, _, ammo, _, _) => Some(ammo),
                Event::Original(_, _) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(drops.len(), 3);
        assert!(drops.iter().all(|rows| rows.is_empty()));
        let record = unsafe { word(fixture.pool, 0x20) };
        assert_eq!(
            unsafe { core::ptr::read_unaligned((record + 0x2c) as *const u32) },
            70
        );
        assert_eq!(
            unsafe { core::ptr::read_unaligned((record + 0x30) as *const u32) },
            20
        );
    }

    #[test]
    fn nested_callbacks_for_different_and_same_members_emit_one_batch() {
        let _lock = ENGINE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fixture = Fixture::new();
        configure_test_engine(&fixture);
        NESTED.with(|state| {
            *state.borrow_mut() = Some((0x7000, fixture.members[1], fixture.damageables[1]));
        });
        fixture.death(0);
        let events = EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
        assert_eq!(events[0], Event::Original(0x7000, fixture.members[0]));
        assert_eq!(events[1], Event::Original(0x7000, fixture.members[1]));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::Drop(..)))
                .count(),
            3
        );

        let mut same = Fixture::new();
        configure_test_engine(&same);
        same.put_byte(same.damageables[1] + 0x18, 0);
        same.put_u32(same.damageables[1] + 0x5c, 0);
        NESTED.with(|state| {
            *state.borrow_mut() = Some((0x7000, same.members[0], same.damageables[0]));
        });
        same.death(0);
        let events = EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
        assert_eq!(events[0], Event::Original(0x7000, same.members[0]));
        assert_eq!(events[1], Event::Original(0x7000, same.members[0]));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::Drop(..)))
                .count(),
            3
        );
    }

    #[test]
    fn partial_final_simultaneous_and_scripted_removals_are_distinguished() {
        assert!(death_eligible(1, 0));
        assert!(!death_eligible(0, 0));
        assert!(!death_eligible(1, 1));

        struct Roster {
            active: [bool; 2],
            batches: usize,
        }
        impl Roster {
            fn complete(&mut self, index: usize) {
                self.active[index] = false;
                let peers = self
                    .active
                    .iter()
                    .enumerate()
                    .filter_map(|(peer, active)| (peer != index).then_some(*active));
                if final_batch_ready(peers) {
                    self.batches += 1;
                }
            }
        }

        // A partial casualty sees its peer active; the second completed death
        // emits one batch after the first has already left the roster active set.
        let mut sequential = Roster {
            active: [true, true],
            batches: 0,
        };
        sequential.complete(0);
        assert_eq!(sequential.batches, 0);
        sequential.complete(1);
        assert_eq!(sequential.batches, 1);

        // If B's native callback is nested inside A's, B still sees A active;
        // only the outer callback can observe the completed squad.
        let mut nested = Roster {
            active: [true, true],
            batches: 0,
        };
        nested.complete(1);
        assert_eq!(nested.batches, 0);
        nested.complete(0);
        assert_eq!(nested.batches, 1);
    }

    #[test]
    fn declared_slots_keep_duplicate_item_across_slots_and_resolve_defaults_and_overrides() {
        let mut item = Box::new([0usize; 48]);
        let mut default = Box::new([0usize; 64]);
        let mut override_info = Box::new([0usize; 64]);
        let mut gun = Box::new([0usize; 80]);
        let mut alternate_gun = Box::new([0usize; 80]);
        default[0xb8 / 8] = gun.as_mut_ptr() as usize;
        override_info[0xb8 / 8] = gun.as_mut_ptr() as usize;
        alternate_gun[0xb8 / 8] = alternate_gun.as_mut_ptr() as usize;
        gun[0x1b8 / 8] = 12;
        alternate_gun[0x1b8 / 8] = 12;
        unsafe {
            *((item.as_mut_ptr() as usize + 0x2c) as *mut u32) = 1;
        }
        item[0x140 / 8] = default.as_mut_ptr() as usize;
        let first = Candidate {
            slot_index: 0,
            slot: 0x1000,
            category: "rifles".into(),
            item: item.as_mut_ptr() as usize,
        };
        let second = Candidate {
            slot_index: 1,
            slot: 0x1010,
            category: "pistols".into(),
            item: item.as_mut_ptr() as usize,
        };
        let slots = [first.clone(), second.clone()];
        unsafe {
            let selected = eligible_slot_items(
                &slots,
                0x3000,
                0,
                default_resolver,
                test_slot_type,
                test_ammo_mode,
            );
            assert_eq!(
                selected,
                [
                    PrimaryDrop {
                        item: first.item,
                        ammo_types: vec![],
                    },
                    PrimaryDrop {
                        item: second.item,
                        ammo_types: vec![],
                    },
                ]
            );

            SCRIPT_KEY.with(|state| state.set(0x3008));
            item[0x148 / 8] = override_info.as_mut_ptr() as usize;
            let selected = eligible_slot_items(
                &slots,
                0x3000,
                0x4000,
                override_resolver,
                test_slot_type,
                test_ammo_mode,
            );
            assert_eq!(selected[0].item, first.item);
            assert_eq!(selected[1].item, second.item);
            assert_eq!(RESOLVED.with(Cell::get), 0x3008);

            override_info[0xb8 / 8] = alternate_gun.as_mut_ptr() as usize;
            let selected = eligible_slot_items(
                &slots,
                0x3000,
                0x4000,
                override_resolver,
                test_slot_type,
                test_ammo_mode,
            );
            assert!(selected.is_empty());
            override_info[0xb8 / 8] = gun.as_mut_ptr() as usize;

            *((gun.as_mut_ptr() as usize + 0xa9) as *mut u8) = 1;
            assert!(eligible_slot_items(
                &slots,
                0x3000,
                0,
                default_resolver,
                test_slot_type,
                test_ammo_mode,
            )
            .is_empty());
            *((gun.as_mut_ptr() as usize + 0xa9) as *mut u8) = 0;

            *((gun.as_mut_ptr() as usize + 0x108) as *mut i32) = 1;
            assert!(eligible_slot_items(
                &slots,
                0x3000,
                0,
                default_resolver,
                test_slot_type,
                test_ammo_mode,
            )
            .is_empty());
            *((gun.as_mut_ptr() as usize + 0xa9) as *mut u8) = 0;

            *((gun.as_mut_ptr() as usize + 0x108) as *mut i32) = 0;
            let ambiguous = [first.clone(), first.clone(), second.clone()];
            let selected = eligible_slot_items(
                &ambiguous,
                0x3000,
                0,
                default_resolver,
                test_slot_type,
                test_ammo_mode,
            );
            assert_eq!(
                selected,
                [PrimaryDrop {
                    item: second.item,
                    ammo_types: vec![]
                }]
            );
        }
    }

    #[test]
    fn shared_free_ammo_goes_to_one_matching_primary_and_excludes_reserved_rounds() {
        let drops = [
            PrimaryDrop {
                item: 1,
                ammo_types: vec![0x1000],
            },
            PrimaryDrop {
                item: 2,
                ammo_types: vec![0x1000, 0x2000],
            },
            PrimaryDrop {
                item: 3,
                ammo_types: vec![0x4000],
            },
        ];
        let pool = ammo::PoolSnapshot {
            scale: 1.0,
            records: vec![
                ammo::PoolRecord {
                    ammo: 0x1000,
                    capacity: 100,
                    rounds: 70,
                    reserved: 20,
                    carriers: 2,
                },
                ammo::PoolRecord {
                    ammo: 0x2000,
                    capacity: 50,
                    rounds: 50,
                    reserved: 50,
                    carriers: 1,
                },
                ammo::PoolRecord {
                    ammo: 0x3000,
                    capacity: 40,
                    rounds: 10,
                    reserved: 0,
                    carriers: 1,
                },
            ],
        };

        let (payloads, debit) = allocate_ammo(&drops, &pool);
        assert_eq!(payloads, [vec![[0x1000, 50]], vec![], vec![]]);
        assert_eq!(debit, [[0x1000, 50]]);
    }

    #[test]
    fn drops_copy_identity_transform_and_empty_ammo() {
        EVENTS.with(|events| events.borrow_mut().clear());
        let snapshot = Snapshot {
            holder: WeakHandle(0),
            manager: 2,
            pool: None,
            items: vec![
                PrimaryDrop {
                    item: 3,
                    ammo_types: vec![],
                },
                PrimaryDrop {
                    item: 3,
                    ammo_types: vec![],
                },
                PrimaryDrop {
                    item: 4,
                    ammo_types: vec![],
                },
            ],
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
        };
        unsafe { emit_drops(&snapshot, test_spawn) };
        assert_eq!(
            EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut())),
            [
                Event::Drop(
                    2,
                    0,
                    [1.0, 2.0, 3.0],
                    [0.0, 0.0, 0.0, 1.0],
                    3,
                    vec![],
                    false,
                    -1
                ),
                Event::Drop(
                    2,
                    0,
                    [1.0, 2.0, 3.0],
                    [0.0, 0.0, 0.0, 1.0],
                    3,
                    vec![],
                    false,
                    -1
                ),
                Event::Drop(
                    2,
                    0,
                    [1.0, 2.0, 3.0],
                    [0.0, 0.0, 0.0, 1.0],
                    4,
                    vec![],
                    false,
                    -1
                ),
            ]
        );
    }
}
