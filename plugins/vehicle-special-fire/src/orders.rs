//! Passenger-gun eligibility for the game's attack-order button.
//!
//! The native button first checks the vehicle's own ammunition pool. A mounted
//! passenger gun can have ammunition while that vehicle pool is empty, so this
//! predicate checks native ammo availability on live bound passenger mounts.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Selected build's AI vtable offset for `Gunner` count, initialized at install.
pub static GUNNER_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Selected build's AI vtable offset for indexed `Gunner` access.
pub static GUNNER_GET: AtomicUsize = AtomicUsize::new(0);

const MAX_ENTITIES: usize = 4096;
const MAX_GUNNERS: usize = 128;
const GUNNER_PRIMARY: usize = 0x20;
const GUNNER_SECONDARY: usize = 0x50;
const GUNNER_SOURCES: usize = 0x68;
const GUN_DESCRIPTION: usize = 0x40;
const DUMMY_ENABLED: usize = 0xe2;

type GetFacets = unsafe extern "system" fn(usize) -> usize;
type GunnerCount = unsafe extern "system" fn(usize) -> usize;
type GunnerGet = unsafe extern "system" fn(usize, usize) -> usize;
type AvailableAmmo = unsafe extern "system" fn(usize, i32, i32) -> usize;

#[inline]
unsafe fn read_usize(at: usize) -> usize {
    unsafe { (at as *const usize).read_unaligned() }
}

#[inline]
unsafe fn read_u8(at: usize) -> u8 {
    unsafe { (at as *const u8).read() }
}

/// Read a bounded pointer vector. Native state is mutable, so reject malformed
/// or unexpectedly large vectors instead of walking beyond their allocation.
unsafe fn vector(owner: usize, offset: usize, bound: usize) -> Option<Vec<usize>> {
    let begin = unsafe { read_usize(owner + offset) };
    let end = unsafe { read_usize(owner + offset + 8) };
    let bytes = end.checked_sub(begin)?;
    if bytes % 8 != 0 || bytes / 8 > bound || (begin == 0 && end != 0) {
        return None;
    }
    Some(
        (0..bytes / 8)
            .map(|index| unsafe { read_usize(begin + index * 8) })
            .collect(),
    )
}

unsafe fn has_bound_passenger_gun(gunner: usize) -> bool {
    if gunner == 0 {
        return false;
    }
    let Some(guns) = (unsafe { vector(gunner, GUNNER_PRIMARY, MAX_GUNNERS) }) else {
        return false;
    };
    let Some(sources) = (unsafe { vector(gunner, GUNNER_SOURCES, MAX_GUNNERS) }) else {
        return false;
    };
    let Some(secondary) = (unsafe { vector(gunner, GUNNER_SECONDARY, MAX_GUNNERS) }) else {
        return false;
    };
    if guns.is_empty() || guns.len() != sources.len() || !secondary.is_empty() {
        return false;
    }

    guns.into_iter().zip(sources).any(|(gun, source)| {
        if gun == 0 || source == 0 {
            return false;
        }
        // Binding copies the source description pointer into the live Gun;
        // compare the paired objects instead of matching its ordinary name.
        unsafe {
            let (gun_vtable, source_vtable) = (read_usize(gun), read_usize(source));
            let (description, source_description) = (
                read_usize(gun + GUN_DESCRIPTION),
                read_usize(source + GUN_DESCRIPTION),
            );
            gun_vtable != 0
                && gun_vtable == source_vtable
                && source_description != 0
                && description == source_description
                && read_u8(gun + DUMMY_ENABLED) != 0
                && read_u8(source + DUMMY_ENABLED) != 0
                && read_usize(gun_vtable + 0xa0) != 0
                && core::mem::transmute::<usize, AvailableAmmo>(read_usize(gun_vtable + 0xa0))(
                    gun, -1, 1,
                ) != 0
        }
    })
}

unsafe fn entity_has_bound_passenger_gun(
    entity: usize,
    count_offset: usize,
    get_offset: usize,
) -> bool {
    if entity == 0 {
        return false;
    }
    let entity_vtable = unsafe { read_usize(entity) };
    if entity_vtable == 0 {
        return false;
    }
    let facets_getter = unsafe { read_usize(entity_vtable + 0xb0) };
    if facets_getter == 0 {
        return false;
    }
    let facets = unsafe { core::mem::transmute::<usize, GetFacets>(facets_getter)(entity) };
    if facets == 0 {
        return false;
    }
    let ai = unsafe { read_usize(facets + 0x28) };
    if ai == 0 {
        return false;
    }
    let ai_vtable = unsafe { read_usize(ai) };
    if ai_vtable == 0 {
        return false;
    }
    let count_fn = unsafe { read_usize(ai_vtable + count_offset) };
    let get_fn = unsafe { read_usize(ai_vtable + get_offset) };
    if count_fn == 0 || get_fn == 0 {
        return false;
    }
    let count = unsafe { core::mem::transmute::<usize, GunnerCount>(count_fn)(ai) };
    if count == 0 || count > MAX_GUNNERS {
        return false;
    }
    let get = unsafe { core::mem::transmute::<usize, GunnerGet>(get_fn) };
    (0..count).any(|index| unsafe { has_bound_passenger_gun(get(ai, index)) })
}

/// Return nonzero when the selected entities include an enabled, live bound
/// passenger gun with native ammo availability. `begin..end` is the attack
/// button's selected entity array.
pub unsafe extern "system" fn available(begin: usize, end: usize) -> u8 {
    let count_offset = GUNNER_COUNT.load(Ordering::Acquire);
    let get_offset = GUNNER_GET.load(Ordering::Acquire);
    if count_offset == 0 || get_offset == 0 || begin == 0 {
        return 0;
    }
    let Some(bytes) = end.checked_sub(begin) else {
        return 0;
    };
    if bytes % 8 != 0 || bytes / 8 > MAX_ENTITIES {
        return 0;
    }
    for index in 0..bytes / 8 {
        let entity = unsafe { read_usize(begin + index * 8) };
        if unsafe { entity_has_bound_passenger_gun(entity, count_offset, get_offset) } {
            return 1;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    const TEST_COUNT_OFFSET: usize = 0x130;
    const TEST_GET_OFFSET: usize = 0x120;
    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    static TEST_GUNNER: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "system" fn facets_get(entity: usize) -> usize {
        unsafe { read_usize(entity + 0x100) }
    }

    unsafe extern "system" fn gunner_count(_: usize) -> usize {
        1
    }

    unsafe extern "system" fn gunner_get(_: usize, _: usize) -> usize {
        TEST_GUNNER.load(Ordering::Relaxed)
    }

    unsafe extern "system" fn available_ammo(gun: usize, mask: i32, mode: i32) -> usize {
        assert_eq!((mask, mode), (-1, 1));
        unsafe { read_usize(gun + 0x50) }
    }

    struct Arena(Vec<Box<[u64]>>);

    impl Arena {
        fn object(&mut self, bytes: usize) -> usize {
            let object = vec![0u64; bytes.div_ceil(8)].into_boxed_slice();
            let at = object.as_ptr() as usize;
            self.0.push(object);
            at
        }

        unsafe fn put(&mut self, object: usize, offset: usize, value: usize) {
            unsafe {
                (object as *mut u8)
                    .add(offset)
                    .cast::<usize>()
                    .write_unaligned(value)
            };
        }

        unsafe fn byte(&mut self, object: usize, offset: usize, value: u8) {
            unsafe { (object as *mut u8).add(offset).write(value) };
        }

        fn vector(&mut self, values: &[usize]) -> usize {
            let data = self.object(values.len().max(1) * 8);
            for (index, value) in values.iter().copied().enumerate() {
                unsafe { self.put(data, index * 8, value) };
            }
            data
        }
    }

    struct Fixture {
        arena: Arena,
        entities: usize,
        gunner: usize,
        gun: usize,
        source: usize,
    }

    fn make_fixture() -> Fixture {
        let mut arena = Arena(Vec::new());
        let entity_vtable = arena.object(0xc0);
        let ai_vtable = arena.object(TEST_COUNT_OFFSET + 8);
        let shared_mount_vtable = arena.object(0xa8);
        let entity = arena.object(0x110);
        let facets = arena.object(0x40);
        let ai = arena.object(8);
        let gunner = arena.object(0x80);
        let gun = arena.object(0x100);
        let source = arena.object(0x100);
        let description = arena.object(8);
        let guns = arena.vector(&[gun]);
        let sources = arena.vector(&[source]);
        let secondary = arena.vector(&[]);
        let selected = arena.vector(&[entity]);
        unsafe {
            arena.put(entity_vtable, 0xb0, facets_get as *const () as usize);
            arena.put(
                ai_vtable,
                TEST_COUNT_OFFSET,
                gunner_count as *const () as usize,
            );
            arena.put(ai_vtable, TEST_GET_OFFSET, gunner_get as *const () as usize);
            arena.put(entity, 0, entity_vtable);
            arena.put(entity, 0x100, facets);
            arena.put(facets, 0x28, ai);
            arena.put(ai, 0, ai_vtable);
            arena.put(gunner, GUNNER_PRIMARY, guns);
            arena.put(gunner, GUNNER_PRIMARY + 8, guns + 8);
            arena.put(gunner, GUNNER_SECONDARY, secondary);
            arena.put(gunner, GUNNER_SECONDARY + 8, secondary);
            arena.put(gunner, GUNNER_SOURCES, sources);
            arena.put(gunner, GUNNER_SOURCES + 8, sources + 8);
            arena.put(gun, 0, shared_mount_vtable);
            arena.put(
                shared_mount_vtable,
                0xa0,
                available_ammo as *const () as usize,
            );
            arena.put(source, 0, shared_mount_vtable);
            arena.put(gun, GUN_DESCRIPTION, description);
            arena.put(source, GUN_DESCRIPTION, description);
            arena.put(gun, 0x50, description);
            arena.byte(gun, DUMMY_ENABLED, 1);
            arena.byte(source, DUMMY_ENABLED, 1);
        }
        TEST_GUNNER.store(gunner, Ordering::Relaxed);
        GUNNER_COUNT.store(TEST_COUNT_OFFSET, Ordering::Release);
        GUNNER_GET.store(TEST_GET_OFFSET, Ordering::Release);
        Fixture {
            arena,
            entities: selected,
            gunner,
            gun,
            source,
        }
    }

    #[test]
    fn empty_or_unbound_selection_is_not_attack_available() {
        let _guard = TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let mut fixture = make_fixture();
        let empty = fixture.entities + 8;
        assert_eq!(unsafe { available(empty, empty) }, 0);
        assert_eq!(
            unsafe { available(fixture.entities, fixture.entities + 8) },
            1
        );
        unsafe { fixture.arena.put(fixture.gunner, GUNNER_SOURCES, 0) };
        assert_eq!(
            unsafe { available(fixture.entities, fixture.entities + 8) },
            0
        );
    }

    #[test]
    fn live_enabled_same_vtable_binding_is_attack_available() {
        let _guard = TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let fixture = make_fixture();
        assert_ne!(fixture.gun, fixture.source);
        assert_eq!(
            unsafe { available(fixture.entities, fixture.entities + 8) },
            1
        );
    }

    #[test]
    fn disabled_or_stale_binding_is_not_attack_available() {
        let _guard = TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let mut fixture = make_fixture();
        unsafe { fixture.arena.byte(fixture.gun, DUMMY_ENABLED, 0) };
        assert_eq!(
            unsafe { available(fixture.entities, fixture.entities + 8) },
            0
        );

        fixture = make_fixture();
        unsafe { fixture.arena.put(fixture.source, GUN_DESCRIPTION, 0) };
        assert_eq!(
            unsafe { available(fixture.entities, fixture.entities + 8) },
            0
        );
    }

    #[test]
    fn active_secondary_mounts_keep_stock_availability() {
        let _guard = TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let mut fixture = make_fixture();
        let gun = fixture.gun;
        let secondary = fixture.arena.vector(&[gun]);
        unsafe {
            fixture
                .arena
                .put(fixture.gunner, GUNNER_SECONDARY, secondary);
            fixture
                .arena
                .put(fixture.gunner, GUNNER_SECONDARY + 8, secondary + 8);
        }
        assert_eq!(
            unsafe { available(fixture.entities, fixture.entities + 8) },
            0
        );
    }

    #[test]
    fn empty_passenger_ammo_does_not_enable_attack() {
        let _guard = TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let mut fixture = make_fixture();
        unsafe { fixture.arena.put(fixture.gun, 0x50, 0) };
        assert_eq!(
            unsafe { available(fixture.entities, fixture.entities + 8) },
            0
        );
    }

    #[test]
    fn ui_adapter_preserves_native_state_and_adds_only_attack_bit() {
        let _guard = TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let fixture = make_fixture();
        unsafe {
            crate::ui::test_adapter::check(fixture.entities, fixture.entities + 8, 0x42);
            crate::ui::test_adapter::check(fixture.entities, fixture.entities, 0x40);
        }
    }
}
