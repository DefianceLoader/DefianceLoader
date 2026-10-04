//! Reconstruct primary slot overrides from the surviving members of one holder.
//!
//! The native holder cache is transient across saves. Its rebuild hook snapshots
//! each available human's item and Gun references before native reconstruction,
//! then restores only unanimous, category-compatible choices into that holder's
//! newly built cache. This keeps the override scoped to the holder instance.

use crate::equipment::{self, ItemBounds, PrimarySlot};
use crate::native::{virtual_unary, word};
use core::ffi::c_void;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

pub(super) static ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

const HOLDER_ROSTER: usize = 0xa0;
const HOLDER_CACHE: usize = 0x138;
// `HumanGunnerClient::vfunc1` at Steam 2026-09-25 RVA 0x2d6650 serializes
// the item vector at this adjusted object offset; `0x2d4f90` restores it.
const GUNNER_ITEMS: usize = 0x50;
// The live Gun vector maps inventory items to configs through Gun +0x40.
const GUNNER_GUNS: usize = 0x38;
const MEMBER_FACETS: usize = 0xb0;
const FACET_DAMAGEABLE: usize = 0x18;
const FACET_AI: usize = 0x28;
const DAMAGEABLE_ACTIVE: usize = 0x18;
const AI_GUNNER: usize = 0x1f0;
const GUNNER_ENTITY: usize = 0x20;
const GUNNER_VTABLE: usize = 0;
const GUN_CONFIG: usize = 0x40;
#[cfg(test)]
const ITEM_OVERRIDE: usize = 0x140;
const OVERRIDE_GUN: usize = 0xb8;
const MAX_MEMBERS: usize = 64;
const MAX_ITEMS: usize = 128;
const MAX_GUNS: usize = 128;
const MAX_SLOTS: usize = 32;

static HUMAN_VTABLE: AtomicUsize = AtomicUsize::new(0);
static ITEM_OVERRIDE_FN: AtomicUsize = AtomicUsize::new(0);
static LOG: AtomicUsize = AtomicUsize::new(0);
static REBUILD_SUCCESSES: AtomicUsize = AtomicUsize::new(0);
static REBUILD_REFUSALS: AtomicUsize = AtomicUsize::new(0);

type Rebuild = unsafe extern "C" fn(usize);
type ItemOverride = unsafe extern "C" fn(usize, usize) -> usize;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Bindings {
    /// Steam 2026-09-25 `FUN_1804591b0`, which rebuilds `holder + 0x138`.
    pub rebuild_original: usize,
    pub human_vtable: usize,
    /// Steam 2026-09-25 `FUN_1802dc840(item, squad_script + 8)`.
    pub item_override: usize,
    /// Optional plugin API logger.
    pub log: usize,
}

pub(super) fn configure(bindings: Bindings) {
    ORIGINAL.store(bindings.rebuild_original as *mut c_void, Ordering::Release);
    HUMAN_VTABLE.store(bindings.human_vtable, Ordering::Release);
    ITEM_OVERRIDE_FN.store(bindings.item_override, Ordering::Release);
    LOG.store(bindings.log, Ordering::Release);
}

#[derive(Clone, Debug)]
struct MemberInventory {
    /// Copied before `0x4591b0`, which may replace member vectors during load.
    items: Vec<usize>,
    gun_configs: Vec<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Reconcile {
    Applied,
    Unchanged,
    Refused,
}

/// Native detour for `FUN_1804591b0(holder)` (Steam 2026-09-25).
///
/// The saved member refs already exist when `SquadHolderFacet::vfunc18`
/// reaches this function. Snapshot their inventories, let native rebuild the
/// holder cache, then reconcile unanimous primary choices before vfunc18 starts
/// creating reinforcement members.
pub(super) unsafe extern "C" fn rebuild(holder: usize) {
    if holder == 0 || !holder.is_multiple_of(8) {
        call_original(holder);
        report_refusal();
        return;
    }
    let snapshot = snapshot_members(holder);
    call_original(holder);
    let Some(snapshot) = snapshot else {
        report_refusal();
        return;
    };
    match reconcile(holder, &snapshot) {
        Reconcile::Applied => report_success(),
        Reconcile::Refused => report_refusal(),
        Reconcile::Unchanged => {}
    }
}

unsafe fn call_original(holder: usize) {
    let address = ORIGINAL.load(Ordering::Acquire);
    if !address.is_null() {
        let original: Rebuild = core::mem::transmute(address);
        original(holder);
    }
}

unsafe fn report_success() {
    if REBUILD_SUCCESSES.fetch_add(1, Ordering::Relaxed) < 8 {
        report(c"primary loadout restored from saved squad members");
    }
}

unsafe fn report_refusal() {
    if REBUILD_REFUSALS.fetch_add(1, Ordering::Relaxed) < 8 {
        report(c"primary loadout restore skipped: invalid or conflicting member inventory");
    }
}

unsafe fn report(message: &core::ffi::CStr) {
    let address = LOG.load(Ordering::Acquire);
    if address == 0 {
        return;
    }
    let log: unsafe extern "C" fn(u32, *const i8) = core::mem::transmute(address);
    log(defiance_api::LOG_DEBUG, message.as_ptr());
}

unsafe fn snapshot_members(holder: usize) -> Option<Vec<MemberInventory>> {
    let roster = read_vector(holder, HOLDER_ROSTER, 8, MAX_MEMBERS)?;
    let roster_count = vector_count(roster, 8);
    if roster_count == 0 {
        return Some(Vec::new());
    }

    let mut active = Vec::new();
    let mut stored = Vec::new();
    for index in 0..roster_count {
        let entity = word(roster.begin, index * 8);
        if entity == 0 || !entity.is_multiple_of(8) {
            return None;
        }
        let facets = virtual_unary(entity, MEMBER_FACETS)?;
        if facets == 0 || !facets.is_multiple_of(8) {
            return None;
        }
        let damageable = word(facets, FACET_DAMAGEABLE);
        if damageable == 0 || !damageable.is_multiple_of(8) {
            return None;
        }
        let owner = word(damageable, 0x10);
        if owner == 0 || !owner.is_multiple_of(8) || word(owner, 0x10) != entity {
            return None;
        }
        let is_active = *((damageable + DAMAGEABLE_ACTIVE) as *const u8) != 0;
        let ai = word(facets, FACET_AI);
        if ai == 0 || !ai.is_multiple_of(8) {
            if is_active {
                return None;
            }
            continue;
        }
        let junction = word(ai, AI_GUNNER);
        if junction == 0 || !junction.is_multiple_of(8) {
            if is_active {
                return None;
            }
            continue;
        }
        let gunner = word(junction, 0x10);
        if gunner == 0 || !gunner.is_multiple_of(8) {
            if is_active {
                return None;
            }
            continue;
        }
        if word(gunner, GUNNER_VTABLE) != HUMAN_VTABLE.load(Ordering::Acquire)
            || word(gunner, GUNNER_ENTITY) != entity
        {
            // Active non-human members do not define an infantry weapon slot.
            continue;
        }
        let inventory = snapshot_inventory(gunner)?;
        stored.push(inventory.clone());
        if is_active {
            active.push(inventory);
        }
    }

    // A squad with living humans uses their current loadouts. A wiped roster
    // can still restore from serialized dead members if those refs remain.
    Some(if active.is_empty() { stored } else { active })
}

unsafe fn snapshot_inventory(gunner: usize) -> Option<MemberInventory> {
    let items = read_vector(gunner, GUNNER_ITEMS, 8, MAX_ITEMS)?;
    let guns = read_vector(gunner, GUNNER_GUNS, 8, MAX_GUNS)?;
    let mut item_ptrs = Vec::with_capacity(vector_count(items, 8));
    for index in 0..vector_count(items, 8) {
        let item = word(items.begin, index * 8);
        if item == 0 || !item.is_multiple_of(8) {
            return None;
        }
        item_ptrs.push(item);
    }
    let mut gun_configs = Vec::with_capacity(vector_count(guns, 8));
    for index in 0..vector_count(guns, 8) {
        let gun = word(guns.begin, index * 8);
        if gun == 0 || !gun.is_multiple_of(8) {
            return None;
        }
        let config = word(gun, GUN_CONFIG);
        if config == 0 || !config.is_multiple_of(8) {
            return None;
        }
        gun_configs.push(config);
    }
    Some(MemberInventory {
        items: item_ptrs,
        gun_configs,
    })
}

unsafe fn reconcile(holder: usize, members: &[MemberInventory]) -> Reconcile {
    if members.is_empty() {
        return Reconcile::Unchanged;
    }
    let script = word(holder, 0xc0);
    if script == 0 || !script.is_multiple_of(8) {
        return Reconcile::Refused;
    }
    let Some(cache) = read_vector(holder, HOLDER_CACHE, 0x10, MAX_SLOTS * 2) else {
        return Reconcile::Refused;
    };
    let Some(slots) = equipment::read_primary_slots(script, Some(cache))
        .ok()
        .filter(|slots| !slots.is_empty() && slots.len() <= MAX_SLOTS)
    else {
        return Reconcile::Refused;
    };
    let override_address = ITEM_OVERRIDE_FN.load(Ordering::Acquire);
    if override_address == 0 {
        return Reconcile::Refused;
    }
    let resolver: ItemOverride = core::mem::transmute(override_address);

    // Resolve every member first; conflicting evidence leaves the native cache
    // intact instead of mixing two different squad loadouts.
    let mut unanimous = Vec::with_capacity(slots.len());
    for slot in &slots {
        let Some(first) = member_item_for_slot(&members[0], slot, script, resolver) else {
            return Reconcile::Refused;
        };
        for member in &members[1..] {
            if member_item_for_slot(member, slot, script, resolver) != Some(first) {
                return Reconcile::Refused;
            }
        }
        unanimous.push((slot, first));
    }

    let updates: Option<Vec<(usize, usize)>> = unanimous
        .into_iter()
        .filter(|(slot, item)| *item != slot.current_item)
        .map(|(slot, item)| cache_row(cache, slot.definition).map(|row| (row, item)))
        .collect();
    let Some(updates) = updates else {
        return Reconcile::Refused;
    };
    if updates.is_empty() {
        return Reconcile::Unchanged;
    }
    for (row, item) in updates {
        core::ptr::write_unaligned((row + 8) as *mut usize, item);
    }
    Reconcile::Applied
}

unsafe fn member_item_for_slot(
    member: &MemberInventory,
    slot: &PrimarySlot,
    script: usize,
    resolver: ItemOverride,
) -> Option<usize> {
    if member.items.is_empty() {
        return None;
    }
    let begin = member.items.as_ptr() as usize;
    let end = begin + member.items.len() * core::mem::size_of::<usize>();
    let candidates = equipment::compatible_primary_items(
        script,
        ItemBounds {
            begin,
            end,
            capacity: end,
        },
    )
    .ok()?;
    let mut found = None;
    for candidate in candidates
        .iter()
        .filter(|item| item.slot_index == slot.slot_index)
    {
        let override_info = resolver(candidate.item, script + 8);
        if override_info == 0 || !override_info.is_multiple_of(8) {
            continue;
        }
        let config = word(override_info, OVERRIDE_GUN);
        if config != 0
            && member.gun_configs.contains(&config)
            && found.replace(candidate.item).is_some()
        {
            return None;
        }
    }
    found
}

unsafe fn read_vector(
    object: usize,
    offset: usize,
    stride: usize,
    limit: usize,
) -> Option<ItemBounds> {
    let bounds = ItemBounds {
        begin: word(object, offset),
        end: word(object, offset + 8),
        capacity: word(object, offset + 16),
    };
    if bounds.begin == 0 && bounds.end == 0 && bounds.capacity == 0 {
        return Some(bounds);
    }
    if bounds.begin == 0
        || !bounds.begin.is_multiple_of(8)
        || bounds.end < bounds.begin
        || bounds.capacity < bounds.end
        || !(bounds.end - bounds.begin).is_multiple_of(stride)
        || vector_count(bounds, stride) > limit
    {
        return None;
    }
    Some(bounds)
}

fn vector_count(bounds: ItemBounds, stride: usize) -> usize {
    (bounds.end - bounds.begin) / stride
}

unsafe fn cache_row(cache: ItemBounds, definition: usize) -> Option<usize> {
    for index in 0..vector_count(cache, 0x10) {
        let row = cache.begin + index * 0x10;
        if word(row, 0) == definition {
            return Some(row);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const FAKE_HUMAN_VTABLE: usize = 0x4141_0000;
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    struct Region(Box<[u64]>);

    impl Region {
        fn new(bytes: usize) -> Self {
            Self(vec![0; bytes.div_ceil(8).max(1)].into_boxed_slice())
        }

        fn ptr(&self) -> usize {
            self.0.as_ptr() as usize
        }

        unsafe fn put(&self, offset: usize, value: usize) {
            core::ptr::write_unaligned((self.ptr() + offset) as *mut usize, value);
        }

        unsafe fn string(&self, offset: usize, value: &str) {
            assert!(value.len() <= 15);
            core::ptr::copy_nonoverlapping(
                value.as_ptr(),
                (self.ptr() + offset) as *mut u8,
                value.len(),
            );
            self.put(offset + 0x10, value.len());
            self.put(offset + 0x18, 15);
        }
    }

    struct Fixture {
        regions: Vec<Region>,
        script: usize,
        slot: usize,
        default_item: usize,
        default_config: usize,
        holder: usize,
        cache: usize,
    }

    impl Fixture {
        fn alloc(&mut self, bytes: usize) -> usize {
            self.regions.push(Region::new(bytes));
            self.regions.last().unwrap().ptr()
        }

        unsafe fn put(&self, object: usize, offset: usize, value: usize) {
            core::ptr::write_unaligned((object + offset) as *mut usize, value);
        }

        unsafe fn vector(&mut self, values: &[usize]) -> usize {
            let storage = self.alloc(values.len().max(1) * 8);
            for (index, value) in values.iter().enumerate() {
                self.put(storage, index * 8, *value);
            }
            storage
        }

        unsafe fn new() -> Self {
            let mut fixture = Self {
                regions: Vec::new(),
                script: 0,
                slot: 0,
                default_item: 0,
                default_config: 0,
                holder: 0,
                cache: 0,
            };
            let default = fixture.make_item("rifles", 0x5100);
            fixture.default_item = default.0;
            fixture.default_config = default.1;

            let slot = fixture.alloc(0x90);
            fixture.slot = slot;
            Region::string(fixture.regions.last().unwrap(), 0x28, "rifles");
            fixture.put(slot, 0x68, fixture.default_item);
            fixture.put(slot, 0x70, 0);

            let slots = fixture.vector(&[slot]);
            let script = fixture.alloc(0x280);
            fixture.script = script;
            fixture.put(script, 0x228, slots);
            fixture.put(script, 0x230, slots + 8);
            fixture.put(script, 0x238, slots + 8);

            let holder = fixture.alloc(0x240);
            fixture.holder = holder;
            // The native callback supplies the script and cache at rebuild.
            fixture.put(holder, 0x200, script);
            fixture.put(holder, 0x208, fixture.default_item);
            fixture.put(holder, 0x210, 0);
            fixture.cache = fixture.alloc(0x10);
            fixture.put(holder, 0x218, fixture.cache);

            fixture
        }

        unsafe fn make_item(&mut self, category: &str, config_tag: usize) -> (usize, usize) {
            let item = self.alloc(0x180);
            self.put(item, 0x2c, 1);
            let config = self.alloc(0x260);
            self.put(config, 0, config_tag);
            let override_info = self.alloc(0xc0);
            self.put(override_info, 0xb8, config);
            self.put(item, ITEM_OVERRIDE, override_info);

            let category_storage = self.alloc(0x20);
            Region::string(self.regions.last().unwrap(), 0, category);
            self.put(item, 0x30, category_storage);
            self.put(item, 0x38, category_storage + 0x20);
            self.put(item, 0x40, category_storage + 0x20);
            (item, config)
        }

        unsafe fn add_member(&mut self, item: usize, config: usize, active: bool) -> usize {
            let gun = self.alloc(0x80);
            self.put(gun, GUN_CONFIG, config);
            let guns = self.vector(&[gun]);
            let items = self.vector(&[item]);
            let gunner = self.alloc(0x180);
            self.put(gunner, GUNNER_VTABLE, FAKE_HUMAN_VTABLE);
            self.put(gunner, GUNNER_ENTITY, 0);
            self.put(gunner, GUNNER_ITEMS, items);
            self.put(gunner, GUNNER_ITEMS + 8, items + 8);
            self.put(gunner, GUNNER_ITEMS + 16, items + 8);
            self.put(gunner, GUNNER_GUNS, guns);
            self.put(gunner, GUNNER_GUNS + 8, guns + 8);
            self.put(gunner, GUNNER_GUNS + 16, guns + 8);

            let junction = self.alloc(0x20);
            self.put(junction, 0x10, gunner);
            let ai = self.alloc(0x220);
            self.put(ai, AI_GUNNER, junction);
            let damageable = self.alloc(0x70);
            core::ptr::write_unaligned(
                (damageable + DAMAGEABLE_ACTIVE) as *mut u8,
                u8::from(active),
            );
            let owner = self.alloc(0x20);
            let facets = self.alloc(0x40);
            self.put(facets, FACET_DAMAGEABLE, damageable);
            self.put(facets, FACET_AI, ai);
            let vtable = self.alloc(0xc0);
            self.put(vtable, MEMBER_FACETS, fake_facets as *const () as usize);
            let entity = self.alloc(0x60);
            self.put(entity, 0, vtable);
            self.put(entity, 8, facets);
            self.put(damageable, 0x10, owner);
            self.put(owner, 0x10, entity);
            self.put(gunner, GUNNER_ENTITY, entity);

            let roster = if word(self.holder, HOLDER_ROSTER) == 0 {
                self.alloc(8 * MAX_MEMBERS)
            } else {
                word(self.holder, HOLDER_ROSTER)
            };
            let count = if word(self.holder, HOLDER_ROSTER) == 0 {
                0
            } else {
                (word(self.holder, HOLDER_ROSTER + 8) - roster) / 8
            };
            self.put(roster, count * 8, entity);
            self.put(self.holder, HOLDER_ROSTER, roster);
            self.put(self.holder, HOLDER_ROSTER + 8, roster + (count + 1) * 8);
            self.put(self.holder, HOLDER_ROSTER + 16, roster + MAX_MEMBERS * 8);
            entity
        }

        unsafe fn set_other_script(&mut self, script: usize, default_item: usize) {
            self.put(self.holder, 0x200, script);
            self.put(self.holder, 0x208, default_item);
        }
    }

    unsafe extern "C" fn fake_facets(entity: usize) -> usize {
        word(entity, 8)
    }

    unsafe extern "C" fn fake_item_override(item: usize, _script_data: usize) -> usize {
        word(item, ITEM_OVERRIDE)
    }

    unsafe extern "C" fn fake_rebuild(holder: usize) {
        let script = word(holder, 0x200);
        let item = word(holder, 0x208);
        let cache = word(holder, 0x218);
        let slots = word(script, 0x228);
        core::ptr::write_unaligned(cache as *mut usize, word(slots, 0));
        core::ptr::write_unaligned((cache + 8) as *mut usize, item);
        core::ptr::write_unaligned((holder + 0xc0) as *mut usize, script);
        core::ptr::write_unaligned((holder + HOLDER_CACHE) as *mut usize, cache);
        core::ptr::write_unaligned((holder + HOLDER_CACHE + 8) as *mut usize, cache + 0x10);
        core::ptr::write_unaligned((holder + HOLDER_CACHE + 16) as *mut usize, cache + 0x10);
        let gunner = word(holder, 0x220);
        let replacement_items = word(holder, 0x228);
        if gunner != 0 && replacement_items != 0 {
            core::ptr::write_unaligned((gunner + GUNNER_ITEMS) as *mut usize, replacement_items);
            core::ptr::write_unaligned(
                (gunner + GUNNER_ITEMS + 8) as *mut usize,
                word(holder, 0x230),
            );
            core::ptr::write_unaligned(
                (gunner + GUNNER_ITEMS + 16) as *mut usize,
                word(holder, 0x238),
            );
        }
    }

    unsafe fn install() {
        configure(Bindings {
            rebuild_original: fake_rebuild as *const () as usize,
            human_vtable: FAKE_HUMAN_VTABLE,
            item_override: fake_item_override as *const () as usize,
            log: 0,
        });
    }

    unsafe fn cached_item(holder: usize) -> usize {
        word(word(holder, HOLDER_CACHE), 8)
    }

    #[test]
    fn null_saved_script_cache_is_rebuilt_from_surviving_custom_inventory() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        unsafe {
            let mut game = Fixture::new();
            let custom = game.make_item("rifles", 0x5200);
            let entity = game.add_member(custom.0, custom.1, true);
            let facets = word(entity, 8);
            let ai = word(facets, FACET_AI);
            let gunner = word(word(ai, AI_GUNNER), 0x10);
            let item_begin = word(gunner, GUNNER_ITEMS);
            let item_end = word(gunner, GUNNER_ITEMS + 8);
            assert_eq!((item_end - item_begin) / 8, 1);
            let slots = equipment::read_primary_slots(game.script, None)
                .unwrap_or_else(|error| panic!("slot parse failed: {error:?}"));
            assert_eq!(
                core::ptr::read_unaligned((game.slot + 0x70) as *const u32),
                0
            );
            assert_eq!(slots.len(), 1, "slot={slots:?}");
            assert_eq!(slots[0].category, "rifles");
            assert_eq!(word(custom.0, 0x38) - word(custom.0, 0x30), 0x20);
            assert_eq!(word(custom.0, 0x40), word(custom.0, 0x38));
            let candidates = equipment::compatible_primary_items(
                game.script,
                ItemBounds {
                    begin: item_begin,
                    end: item_end,
                    capacity: word(gunner, GUNNER_ITEMS + 16),
                },
            )
            .unwrap_or_else(|error| panic!("candidate parse failed: {error:?}"));
            assert!(
                candidates
                    .iter()
                    .any(|candidate| candidate.item == custom.0),
                "candidates={candidates:?} custom={:#x}",
                custom.0
            );
            install();
            assert_eq!(word(game.holder, 0xc0), 0);
            assert_eq!(word(game.holder, HOLDER_CACHE), 0);
            rebuild(game.holder);
            assert_eq!(word(game.holder, 0xc0), game.script);
            assert_eq!(cached_item(game.holder), custom.0);
        }
    }

    #[test]
    fn inventory_snapshot_survives_native_reallocation_during_rebuild() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        unsafe {
            let mut game = Fixture::new();
            let custom = game.make_item("rifles", 0x5200);
            let entity = game.add_member(custom.0, custom.1, true);
            let facets = word(entity, 8);
            let ai = word(facets, FACET_AI);
            let gunner = word(word(ai, AI_GUNNER), 0x10);
            let default_item = game.default_item;
            let replacement = game.vector(&[default_item]);
            let replacement_end = replacement + 8;
            game.put(game.holder, 0x220, gunner);
            game.put(game.holder, 0x228, replacement);
            game.put(game.holder, 0x230, replacement_end);
            game.put(game.holder, 0x238, replacement_end);
            install();
            rebuild(game.holder);
            assert_eq!(word(gunner, GUNNER_ITEMS), replacement);
            assert_eq!(word(replacement, 0), default_item);
            assert_eq!(cached_item(game.holder), custom.0);
        }
    }

    #[test]
    fn stock_default_inventory_keeps_the_scripted_default() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        unsafe {
            let mut game = Fixture::new();
            let default_item = game.default_item;
            let default_config = game.default_config;
            game.add_member(default_item, default_config, true);
            install();
            rebuild(game.holder);
            assert_eq!(cached_item(game.holder), game.default_item);
        }
    }

    #[test]
    fn zero_member_holder_keeps_native_script_default() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        unsafe {
            let game = Fixture::new();
            install();
            rebuild(game.holder);
            assert_eq!(cached_item(game.holder), game.default_item);
        }
    }

    #[test]
    fn two_holders_sharing_a_script_keep_overrides_in_separate_caches() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        unsafe {
            let mut first = Fixture::new();
            let custom = first.make_item("rifles", 0x5200);
            first.add_member(custom.0, custom.1, true);
            let mut second = Fixture::new();
            second.script = first.script;
            second.default_item = first.default_item;
            second.default_config = first.default_config;
            second.set_other_script(first.script, first.default_item);
            second.add_member(first.default_item, first.default_config, true);
            install();
            rebuild(first.holder);
            rebuild(second.holder);
            assert_eq!(cached_item(first.holder), custom.0);
            assert_eq!(cached_item(second.holder), first.default_item);
            assert_ne!(
                word(first.holder, HOLDER_CACHE),
                word(second.holder, HOLDER_CACHE)
            );
        }
    }

    #[test]
    fn conflicting_active_member_primary_items_leave_native_cache_unchanged() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        unsafe {
            let mut game = Fixture::new();
            let first = game.make_item("rifles", 0x5200);
            let second = game.make_item("rifles", 0x5300);
            game.add_member(first.0, first.1, true);
            game.add_member(second.0, second.1, true);
            install();
            rebuild(game.holder);
            assert_eq!(cached_item(game.holder), game.default_item);
        }
    }
}
