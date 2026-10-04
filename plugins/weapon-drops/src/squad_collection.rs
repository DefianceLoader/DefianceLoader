//! Exchange one compatible primary across every human in a canonical squad.
//!
//! The original collection callback remains the owner of pickup bookkeeping.
//! This module only plans a whole-squad replacement before that callback can
//! consume the pickup, then substitutes native loadout and conserved ammo operations.

use super::equipment::{self, Candidate, ItemBounds, PrimarySlot, ReadError};
use super::native::{virtual_unary, word};
use super::reserve;
use super::{ammo, policy};
use core::ffi::c_void;
use std::cell::RefCell;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

pub(super) static ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
pub(super) static DROP_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
pub(super) static ADD_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static CANONICAL_SQUAD: AtomicUsize = AtomicUsize::new(0);
static HOLDER: AtomicUsize = AtomicUsize::new(0);
static CONTEXT: AtomicUsize = AtomicUsize::new(0);
static RESOLVE: AtomicUsize = AtomicUsize::new(0);
static SLOT: AtomicUsize = AtomicUsize::new(0);
static HUMAN: AtomicUsize = AtomicUsize::new(0);
static DETACH: AtomicUsize = AtomicUsize::new(0);
static DISPOSE: AtomicUsize = AtomicUsize::new(0);
static PRIMARY_ADD: AtomicUsize = AtomicUsize::new(0);
static AMMO_MODE: AtomicUsize = AtomicUsize::new(0);
static AMMO_REMOVE: AtomicUsize = AtomicUsize::new(0);
static SWITCH: AtomicUsize = AtomicUsize::new(0);
static VISUAL_SYNC: AtomicUsize = AtomicUsize::new(0);
static SPAWN: AtomicUsize = AtomicUsize::new(0);
static LOG: AtomicUsize = AtomicUsize::new(0);
static REFUSALS: AtomicUsize = AtomicUsize::new(0);
static SUCCESSES: AtomicUsize = AtomicUsize::new(0);

const MEMBER_ITEMS: usize = 0x50;
const MEMBER_GUNS: usize = 0x38;
const HOLDER_SCRIPT: usize = 0xc0;
const HOLDER_ROSTER: usize = 0xa0;
const HOLDER_CACHE: usize = 0x138;
const MAX_MEMBERS: usize = 64;
const MAX_ITEMS: usize = 64;
const MAX_GUNS: usize = 64;
const MAX_GROUND_RECORDS: usize = 4096;
const MAX_VISUAL_ROWS: usize = 128;

type Unary = unsafe extern "C" fn(usize) -> usize;
type Binary = unsafe extern "C" fn(usize, usize) -> usize;
type Slot = unsafe extern "C" fn(usize, usize) -> i32;
type Collect = unsafe extern "C" fn(usize, usize, usize);
type DropSpecial = unsafe extern "C" fn(usize, usize, usize, u8);
type Add = unsafe extern "C" fn(usize, usize, *const [usize; 3]);
type Detach = unsafe extern "C" fn(usize, *mut [usize; 4]) -> usize;
type VoidUnary = unsafe extern "C" fn(usize);
type VoidBinary = unsafe extern "C" fn(usize, usize);
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

/// Absolute native helpers required by this module; the parent configures these
/// from the build-specific bindings table before installing its detours.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Bindings {
    pub collect_original: usize,
    pub drop_original: usize,
    pub add_original: usize,
    pub canonical_squad: usize,
    pub holder: usize,
    pub context: usize,
    pub resolve: usize,
    pub slot: usize,
    pub human_vtable: usize,
    pub detach: usize,
    pub dispose: usize,
    pub primary_add: usize,
    pub ammo_mode: usize,
    pub ammo_remove: usize,
    pub switch: usize,
    pub visual_sync: usize,
    pub spawn: usize,
    pub log: usize,
}

pub(super) fn configure(bindings: Bindings) {
    ORIGINAL.store(bindings.collect_original as *mut c_void, Ordering::Release);
    DROP_ORIGINAL.store(bindings.drop_original as *mut c_void, Ordering::Release);
    ADD_ORIGINAL.store(bindings.add_original as *mut c_void, Ordering::Release);
    for (target, value) in [
        (&CANONICAL_SQUAD, bindings.canonical_squad),
        (&HOLDER, bindings.holder),
        (&CONTEXT, bindings.context),
        (&RESOLVE, bindings.resolve),
        (&SLOT, bindings.slot),
        (&HUMAN, bindings.human_vtable),
        (&DETACH, bindings.detach),
        (&DISPOSE, bindings.dispose),
        (&PRIMARY_ADD, bindings.primary_add),
        (&AMMO_MODE, bindings.ammo_mode),
        (&AMMO_REMOVE, bindings.ammo_remove),
        (&SWITCH, bindings.switch),
        (&VISUAL_SYNC, bindings.visual_sync),
        (&SPAWN, bindings.spawn),
        (&LOG, bindings.log),
    ] {
        target.store(value, Ordering::Release);
    }
}

#[derive(Clone, Debug)]
struct MemberPlan {
    entity: usize,
    ai: usize,
    gunner: usize,
    slot: PrimarySlot,
    old_item: usize,
    old_config: usize,
    old_gun: usize,
    special_gun: usize,
    special_item: usize,
    special_slot: i32,
    selected_gun_index: i32,
    selected_gun: usize,
    visual: usize,
    empty_ammo: Vec<[usize; 2]>,
    pool: usize,
}

#[derive(Clone, Debug)]
struct Plan {
    manager: usize,
    holder: usize,
    script: usize,
    incoming: usize,
    incoming_gun: usize,
    incoming_ammo: ammo::Rows,
    outgoing_ammo: ammo::Rows,
    leftovers: ammo::Rows,
    ammo_policy: policy::AmmoPolicy,
    category: String,
    members: Vec<MemberPlan>,
    same_item: bool,
    applying: bool,
    swapped: bool,
    position: [f32; 3],
    rotation: [f32; 4],
}

thread_local! {
    static PLANS: RefCell<Vec<Option<Plan>>> = const { RefCell::new(Vec::new()) };
}

unsafe fn refuse(reason: &str) {
    let log = LOG.load(Ordering::Acquire);
    if log != 0 && REFUSALS.fetch_add(1, Ordering::Relaxed) < 16 {
        let log: unsafe extern "C" fn(u32, *const i8) = core::mem::transmute(log);
        if let Ok(message) =
            std::ffi::CString::new(format!("squad primary exchange refused: {reason}"))
        {
            log(defiance_api::LOG_DEBUG, message.as_ptr());
        }
    }
}

unsafe fn report_success(members: usize, identical: bool) {
    let log = LOG.load(Ordering::Acquire);
    if log != 0 && SUCCESSES.fetch_add(1, Ordering::Relaxed) < 16 {
        let log: unsafe extern "C" fn(u32, *const i8) = core::mem::transmute(log);
        if let Ok(message) = std::ffi::CString::new(format!(
            "squad primary collection completed: {members} members; identical={identical}"
        )) {
            log(defiance_api::LOG_DEBUG, message.as_ptr());
        }
    }
}

unsafe fn vector(object: usize, offset: usize, stride: usize, limit: usize) -> Option<ItemBounds> {
    let bounds = ItemBounds {
        begin: word(object, offset),
        end: word(object, offset + 8),
        capacity: word(object, offset + 16),
    };
    if bounds.begin == 0
        || !bounds.begin.is_multiple_of(8)
        || bounds.end < bounds.begin
        || bounds.capacity < bounds.end
        || !(bounds.end - bounds.begin).is_multiple_of(stride)
        || (bounds.end - bounds.begin) / stride > limit
    {
        return None;
    }
    Some(bounds)
}

fn count(bounds: ItemBounds, stride: usize) -> usize {
    (bounds.end - bounds.begin) / stride
}

/// Saved holders omit the transient slot cache. A null or allocated-empty
/// vector is valid; malformed headers still stop collection before mutation.
unsafe fn slot_cache(holder: usize) -> Result<Option<ItemBounds>, &'static str> {
    if word(holder, HOLDER_CACHE) == 0 {
        return if word(holder, HOLDER_CACHE + 8) == 0 && word(holder, HOLDER_CACHE + 16) == 0 {
            Ok(None)
        } else {
            Err("invalid slot cache header")
        };
    }
    let cache =
        vector(holder, HOLDER_CACHE, 0x10, MAX_MEMBERS).ok_or("invalid slot cache header")?;
    Ok((cache.begin != cache.end).then_some(cache))
}

unsafe fn item_gun(item: usize, script: usize) -> Option<usize> {
    if item == 0 || *((item + 0x2c) as *const u32) != 1 {
        return None;
    }
    let resolve: Binary = core::mem::transmute(RESOLVE.load(Ordering::Acquire));
    let effective = if script == 0 {
        word(item, 0x140)
    } else {
        resolve(item, script + 8)
    };
    if effective == 0 {
        return None;
    }
    let gun = word(effective, 0xb8);
    (gun != 0).then_some(gun)
}

unsafe fn member_parts(entity: usize) -> Option<(usize, usize, usize, usize)> {
    let facets = virtual_unary(entity, 0xb0)?;
    if facets == 0 {
        return None;
    }
    let damageable = word(facets, 0x18);
    if damageable == 0 || *((damageable + 0x18) as *const u8) == 0 {
        return Some((facets, 0, 0, damageable));
    }
    let ai = word(facets, 0x28);
    if ai == 0 {
        return None;
    }
    let junction = word(ai, 0x1f0);
    if junction == 0 {
        return None;
    }
    let gunner = word(junction, 0x10);
    (gunner != 0).then_some((facets, ai, gunner, damageable))
}

unsafe fn has_mesh(config: usize) -> bool {
    let address = config + 0x1a8;
    let length = word(address, 0x10);
    let capacity = word(address, 0x18);
    length != 0
        && length <= 512
        && capacity >= length
        && capacity <= 1024 * 1024
        && (capacity <= 15 || word(address, 0) != 0)
}

unsafe fn collector_holder(collector: usize) -> usize {
    let squad_fn: Unary = core::mem::transmute(CANONICAL_SQUAD.load(Ordering::Acquire));
    let holder_fn: Unary = core::mem::transmute(HOLDER.load(Ordering::Acquire));
    let squad = squad_fn(collector);
    if squad == 0 {
        0
    } else {
        holder_fn(squad)
    }
}

unsafe fn find_gun(guns: ItemBounds, config: usize) -> Option<usize> {
    let mut found = None;
    for index in 0..count(guns, 8) {
        let gun = word(guns.begin, index * 8);
        if gun != 0 && word(gun, 0x40) == config && found.replace(gun).is_some() {
            return None;
        }
    }
    found
}

unsafe fn cache_row(cache: ItemBounds, definition: usize) -> Option<usize> {
    for index in 0..count(cache, 0x10) {
        let row = cache.begin + index * 0x10;
        if word(row, 0) == definition {
            return Some(row);
        }
    }
    None
}

/// Include supported identities with zero rounds: native loadout addition uses
/// these rows to register resupply capacity, and skips that work for no rows.
unsafe fn empty_ammo(config: usize, entity: usize) -> Result<Vec<[usize; 2]>, &'static str> {
    let mode: unsafe extern "C" fn(usize) -> u8 =
        core::mem::transmute(AMMO_MODE.load(Ordering::Acquire));
    let offset = if mode(entity) == 0 { 0x210 } else { 0x228 };
    let types = vector(config, offset, 0x10, 128).ok_or("invalid primary ammunition types")?;
    let mut rows = Vec::with_capacity(count(types, 0x10));
    for index in 0..count(types, 0x10) {
        let ammo = word(types.begin, index * 0x10);
        if ammo == 0 || rows.iter().any(|row: &[usize; 2]| row[0] == ammo) {
            return Err("missing or duplicate primary ammunition type");
        }
        rows.push([ammo, 0]);
    }
    if rows.is_empty() {
        return Err("primary has no supported ammunition types");
    }
    Ok(rows)
}

/// Bounds outgoing detached rounds by the current total for each shared ammo
/// identity. Detaching Guns can only reduce these validated depot totals.
unsafe fn outgoing_ammo_ceiling(members: &[MemberPlan]) -> Result<ammo::Rows, &'static str> {
    let mode: unsafe extern "C" fn(usize) -> u8 =
        core::mem::transmute(AMMO_MODE.load(Ordering::Acquire));
    let pool = members
        .first()
        .ok_or("squad has no outgoing ammunition carriers")?
        .pool;
    let snapshot = ammo::pool_snapshot(pool)?;
    if snapshot.scale != 1.0 {
        return Err("ammunition pool uses a non-unit transfer scale");
    }
    let mut identities = Vec::new();
    for member in members {
        let offset = if mode(member.entity) == 0 {
            0x210
        } else {
            0x228
        };
        let types = vector(member.old_config, offset, 0x10, 128)
            .ok_or("invalid outgoing primary ammunition types")?;
        for index in 0..count(types, 0x10) {
            let info = word(types.begin + index * 0x10, 0);
            if info == 0 || !info.is_multiple_of(8) {
                return Err("invalid outgoing primary ammunition type");
            }
            reserve::canonical_name(info)?;
            if !identities.contains(&info) {
                identities.push(info);
            }
        }
    }
    let rows = identities
        .into_iter()
        .filter_map(|info| {
            snapshot
                .records
                .iter()
                .find(|record| record.ammo == info)
                .filter(|record| record.rounds != 0)
                .map(|record| [info, record.rounds as usize])
        })
        .collect();
    Ok(rows)
}

unsafe fn preflight_retain(plan: &Plan) -> Result<(), &'static str> {
    let pool = plan.members[0].pool;
    reserve::snapshot(pool)?;
    let ceiling = outgoing_ammo_ceiling(&plan.members)?;
    reserve::preflight_deposit(pool, &ceiling)
}

unsafe fn member_pool(ai: usize, gun: usize, entity: usize) -> Result<usize, &'static str> {
    let junction = word(ai, 0x148);
    let pool = if junction == 0 {
        0
    } else {
        word(junction, 0x10)
    };
    let provider = word(gun, 0x58);
    if pool == 0 || provider == 0 || word(provider, 0x10) != pool {
        return Err("primary has no matching squad ammunition provider");
    }
    let vtable = word(pool, 0);
    if vtable == 0 || word(vtable, 0x40) != AMMO_REMOVE.load(Ordering::Acquire) {
        return Err("unsupported squad ammunition pool");
    }
    let old_types = empty_ammo(word(gun, 0x40), entity)?;
    let records = if word(pool, 0x20) == 0 && word(pool, 0x28) == 0 && word(pool, 0x30) == 0 {
        ItemBounds {
            begin: 0,
            end: 0,
            capacity: 0,
        }
    } else {
        vector(pool, 0x20, 0x48, 128).ok_or("invalid squad ammunition pool")?
    };
    let mut identities = Vec::new();
    for index in 0..count(records, 0x48) {
        let record = records.begin + index * 0x48;
        let identity = word(record, 0);
        if identity == 0 || identities.contains(&identity) {
            return Err("missing or duplicate squad ammunition identity");
        }
        identities.push(identity);
        if old_types.iter().any(|row| row[0] == identity) && *((record + 0x34) as *const u32) == 0 {
            return Err("squad ammunition has no registered carriers");
        }
    }
    Ok(pool)
}

unsafe fn plan(
    manager: usize,
    pickup: usize,
    collector: usize,
) -> Result<Option<Plan>, &'static str> {
    let records =
        vector(manager, 0x20, 0x90, MAX_GROUND_RECORDS).ok_or("unsupported ground inventory")?;
    let record = (0..count(records, 0x90))
        .map(|index| records.begin + index * 0x90)
        .find(|record| word(*record, 0x10) == pickup);
    let Some(record) = record else {
        return Ok(None);
    };
    let incoming = word(record, 0x18);
    let incoming_default = item_gun(incoming, 0).ok_or("invalid ground weapon item")?;
    let slot_fn: Slot = core::mem::transmute(SLOT.load(Ordering::Acquire));
    if slot_fn(incoming_default, 0) != 0 {
        return Ok(None); // Stock special-slot collection remains unchanged.
    }
    if *((incoming_default + 0xa9) as *const u8) != 0 {
        return Err("primary item resolves to a grenade");
    }
    if !has_mesh(incoming_default) {
        return Err("incoming primary has no ground mesh");
    }
    let ground_ammo = ItemBounds {
        begin: word(record, 0x20),
        end: word(record, 0x28),
        capacity: word(record, 0x30),
    };
    let incoming_ammo = ammo::read_payload(ground_ammo)?;
    let squad_fn: Unary = core::mem::transmute(CANONICAL_SQUAD.load(Ordering::Acquire));
    let squad = squad_fn(collector);
    let holder_fn: Unary = core::mem::transmute(HOLDER.load(Ordering::Acquire));
    let holder = if squad == 0 { 0 } else { holder_fn(squad) };
    if holder == 0 {
        return Err("missing canonical squad holder");
    }
    let script = word(holder, HOLDER_SCRIPT);
    if script == 0 {
        return Err("missing canonical squad script");
    }
    let roster = vector(holder, HOLDER_ROSTER, 8, MAX_MEMBERS).ok_or("invalid squad roster")?;
    let cache = slot_cache(holder)?;
    let slots = equipment::read_primary_slots(script, cache).map_err(equipment_error)?;

    let incoming_ptrs = [incoming];
    let incoming_candidates = equipment::compatible_primary_items(
        script,
        ItemBounds {
            begin: incoming_ptrs.as_ptr() as usize,
            end: incoming_ptrs.as_ptr() as usize + 8,
            capacity: incoming_ptrs.as_ptr() as usize + 8,
        },
    )
    .map_err(equipment_error)?;
    let category = target_category(&incoming_candidates)?;
    let target_slot = slots
        .iter()
        .find(|slot| slot.category == category)
        .cloned()
        .ok_or("primary category has no visible squad slot")?;
    let context_fn: Unary = core::mem::transmute(CONTEXT.load(Ordering::Acquire));
    let human_vtable = HUMAN.load(Ordering::Acquire);
    let mut members = Vec::new();
    let mut shared_old_item = None;
    let mut shared_old_config = None;
    let mut common_transform = None;

    for index in 0..count(roster, 8) {
        let entity = word(roster.begin, index * 8);
        if entity == 0 {
            continue;
        }
        let Some((facets, ai, gunner, _damageable)) = member_parts(entity) else {
            return Err("squad roster contains an unreadable member");
        };
        if ai == 0 || gunner == 0 {
            continue;
        }
        if word(gunner, 0) != human_vtable || word(gunner, 0x20) != entity {
            return Err("active squad member is not a supported human gunner");
        }
        let context = context_fn(gunner);
        if context == 0 {
            return Err("human squad member has no equipment context");
        }
        let member_script = virtual_unary(context, 0x50).unwrap_or(0);
        if member_script == 0 {
            return Err("human squad member has no equipment script");
        }
        let item_bounds =
            vector(gunner, MEMBER_ITEMS, 8, MAX_ITEMS).ok_or("invalid member item vector")?;
        let gun_bounds =
            vector(gunner, MEMBER_GUNS, 8, MAX_GUNS).ok_or("invalid member Gun vector")?;
        let candidates =
            equipment::compatible_primary_items(script, item_bounds).map_err(equipment_error)?;
        let matched = candidates
            .into_iter()
            .filter(|candidate| candidate.category == category)
            .collect::<Vec<Candidate>>();
        if matched.len() != 1 {
            return Err("human squad member has zero or ambiguous compatible primary items");
        }
        let old_item = matched[0].item;
        if shared_old_item
            .replace(old_item)
            .is_some_and(|previous| previous != old_item)
        {
            return Err("squad members do not share the same primary item definition");
        }
        let old_config =
            item_gun(old_item, member_script).ok_or("member primary has no effective gun")?;
        let default_config = item_gun(old_item, 0).ok_or("member primary has no default gun")?;
        if old_config != default_config || slot_fn(old_config, context) != 0 {
            return Err("member primary is not the default non-grenade primary");
        }
        if *((old_config + 0xa9) as *const u8) != 0 {
            return Err("member primary resolves to a grenade");
        }
        if shared_old_config
            .replace(old_config)
            .is_some_and(|previous| previous != old_config)
        {
            return Err("squad members resolve the shared primary item to different guns");
        }
        if item_gun(incoming, member_script) != Some(incoming_default) {
            return Err("ground primary has a member-specific gun override");
        }
        let slot_fn: Slot = core::mem::transmute(SLOT.load(Ordering::Acquire));
        if slot_fn(incoming_default, context) != 0 {
            return Err("incoming item is not primary in every member context");
        }
        let old_gun = find_gun(gun_bounds, old_config)
            .ok_or("member primary Gun object is missing or duplicated")?;
        let special_gun = word(gunner, 0x28);
        let special_item = word(gunner, 0x30);
        let special_slot = *((gunner + 0xcc) as *const i32);
        let selected_gun_index = *((gunner + 0x94) as *const i32);
        if selected_gun_index < -1
            || selected_gun_index as usize >= count(gun_bounds, 8) && selected_gun_index >= 0
        {
            return Err("member has an invalid selected Gun index");
        }
        let selected_gun = if selected_gun_index < 0 {
            0
        } else {
            word(gun_bounds.begin, selected_gun_index as usize * 8)
        };
        if (special_gun == 0) != (special_item == 0)
            || (special_gun != 0
                && (special_slot <= 0
                    || !(0..count(gun_bounds, 8))
                        .any(|i| word(gun_bounds.begin, i * 8) == special_gun)
                    || !(0..count(item_bounds, 8))
                        .any(|i| word(item_bounds.begin, i * 8) == special_item)))
        {
            return Err("human squad member has invalid held-special state");
        }
        let visual = word(facets, 8);
        if visual == 0 {
            return Err("human squad member has no weapon geometry facet");
        }
        let visual_rows = vector(visual, 0x108, 0xb0, MAX_VISUAL_ROWS)
            .ok_or("invalid member weapon geometry table")?;
        if count(visual_rows, 0xb0) == 0 {
            return Err("human squad member weapon geometry table is empty");
        }
        let position_facet = word(facets, 0);
        let position_ptr = virtual_unary(position_facet, 0x58).unwrap_or(0);
        let rotation_ptr = virtual_unary(position_facet, 0x60).unwrap_or(0);
        if position_ptr == 0 || rotation_ptr == 0 {
            return Err("human squad member has no transform");
        }
        let position = *(position_ptr as *const [f32; 3]);
        let rotation = *(rotation_ptr as *const [f32; 4]);
        validate_transform(position, rotation)?;
        if common_transform.is_none() {
            common_transform = Some((position, rotation));
        }
        if entity == collector {
            common_transform = Some((position, rotation));
        }
        members.push(MemberPlan {
            entity,
            ai,
            gunner,
            slot: target_slot.clone(),
            old_item,
            old_config,
            old_gun,
            special_gun,
            special_item,
            special_slot,
            selected_gun_index,
            selected_gun,
            visual,
            empty_ammo: empty_ammo(incoming_default, entity)?,
            pool: member_pool(ai, old_gun, entity)?,
        });
    }
    if members.is_empty() {
        return Err("squad has no active human members");
    }
    if !members.iter().any(|member| member.entity == collector) {
        return Err("collector is not an active human in the canonical squad");
    }
    validate_member_batch(&members, &category)?;
    if members.iter().any(|member| member.pool != members[0].pool) {
        return Err("squad members have different ammunition pools");
    }
    if !incoming_ammo.is_empty() {
        ammo::validate_pool(members[0].pool)?;
        if incoming_ammo
            .iter()
            .any(|row| !members[0].empty_ammo.iter().any(|ty| ty[0] == row[0]))
        {
            return Err("ground ammo is incompatible with the incoming primary");
        }
    }

    let incoming_context = context_fn(members[0].gunner);
    let incoming_script = virtual_unary(incoming_context, 0x50).unwrap_or(0);
    let incoming_gun = item_gun(incoming, incoming_script)
        .ok_or("ground primary has no effective gun for the squad")?;
    if incoming_gun != incoming_default {
        return Err("ground primary uses a non-default gun override");
    }
    let same_item = already_has_item(&members, incoming);
    if !same_item
        && members.iter().any(|member| {
            vector(member.gunner, MEMBER_GUNS, 8, MAX_GUNS).is_none_or(|guns| {
                (0..count(guns, 8)).any(|i| {
                    let gun = word(guns.begin, i * 8);
                    gun != 0 && word(gun, 0x40) == incoming_gun
                })
            })
        })
    {
        return Err("incoming primary already has a squad Gun object");
    }
    let (position, rotation) = common_transform.ok_or("squad transform is missing")?;
    Ok(Some(Plan {
        manager,
        holder,
        script,
        incoming,
        incoming_gun,
        incoming_ammo,
        outgoing_ammo: Vec::new(),
        leftovers: Vec::new(),
        ammo_policy: policy::current().ammo_policy,
        category,
        members,
        same_item,
        applying: false,
        swapped: false,
        position,
        rotation,
    }))
}

fn equipment_error(error: ReadError) -> &'static str {
    match error {
        ReadError::InvalidSquad => "invalid squad script",
        ReadError::InvalidVector => "invalid squad equipment vector",
        ReadError::TooManyEntries => "squad equipment vector exceeds its bound",
        ReadError::InvalidSlot => "invalid squad slot definition",
        ReadError::InvalidItem => "invalid squad item definition",
        ReadError::InvalidString => "invalid squad equipment string",
    }
}

fn validate_transform(position: [f32; 3], rotation: [f32; 4]) -> Result<(), &'static str> {
    let norm = rotation.iter().map(|value| value * value).sum::<f32>();
    if !position.into_iter().chain(rotation).all(f32::is_finite)
        || !norm.is_finite()
        || norm < 0.000001
    {
        return Err("invalid squad transform");
    }
    Ok(())
}

fn validate_member_batch(members: &[MemberPlan], category: &str) -> Result<(), &'static str> {
    if members.is_empty() || members.len() > MAX_MEMBERS {
        return Err("squad has no active human members");
    }
    let mut entities = Vec::with_capacity(members.len());
    let mut gunners = Vec::with_capacity(members.len());
    let first = &members[0];
    for member in members {
        if member.slot.category != category {
            return Err("squad member primary category does not match the pickup");
        }
        if member.old_item != first.old_item || member.old_config != first.old_config {
            return Err("squad members do not share one primary item and gun");
        }
        if entities.contains(&member.entity) || gunners.contains(&member.gunner) {
            return Err("squad roster contains duplicate active members");
        }
        entities.push(member.entity);
        gunners.push(member.gunner);
        if (member.special_gun == 0) != (member.special_item == 0)
            || (member.special_gun != 0 && member.special_slot <= 0)
        {
            return Err("human squad member has invalid held-special state");
        }
    }
    Ok(())
}

fn target_category(candidates: &[Candidate]) -> Result<String, &'static str> {
    if candidates.len() != 1 {
        return Err("primary item matches zero or multiple declared slots");
    }
    Ok(candidates[0].category.clone())
}

fn already_has_item(members: &[MemberPlan], incoming: usize) -> bool {
    !members.is_empty() && members.iter().all(|member| member.old_item == incoming)
}

fn has_holder_conflict(plans: &[Option<Plan>], holder: usize) -> bool {
    plans.iter().flatten().any(|active| active.holder == holder)
}

pub(super) unsafe extern "C" fn collect(manager: usize, pickup: usize, collector: usize) {
    let original: Collect = core::mem::transmute(ORIGINAL.load(Ordering::Acquire));
    let current_holder = collector_holder(collector);
    if current_holder != 0
        && PLANS.with(|plans| has_holder_conflict(&plans.borrow(), current_holder))
    {
        refuse("nested collection on the same squad holder");
        return;
    }
    let pending = match plan(manager, pickup, collector) {
        Ok(plan) => plan,
        Err(reason) => {
            refuse(reason);
            return;
        }
    };
    if let Some(plan) = pending
        .as_ref()
        .filter(|plan| plan.ammo_policy == policy::AmmoPolicy::Retain)
    {
        if let Err(reason) = preflight_retain(plan) {
            refuse(reason);
            return;
        }
    }
    PLANS.with(|plans| plans.borrow_mut().push(pending));
    original(manager, pickup, collector);
    let completed = PLANS.with(|plans| plans.borrow_mut().pop().flatten());
    if let Some(plan) = completed.filter(|plan| plan.swapped) {
        report_success(plan.members.len(), plan.same_item);
        let rows = if plan.ammo_policy == policy::AmmoPolicy::Transfer {
            &plan.outgoing_ammo[..]
        } else {
            &[]
        };
        spawn_payload(&plan, plan.members[0].old_item, rows);
        if !plan.leftovers.is_empty() {
            spawn_payload(&plan, plan.incoming, &plan.leftovers);
        }
    }
}

unsafe fn spawn_payload(plan: &Plan, item: usize, rows: &[[usize; 2]]) {
    let spawn: Spawn = core::mem::transmute(SPAWN.load(Ordering::Acquire));
    let bounds = if rows.is_empty() {
        [0; 3]
    } else {
        let begin = rows.as_ptr() as usize;
        let end = begin + rows.len() * 0x10;
        [begin, end, end]
    };
    spawn(
        plan.manager,
        0,
        &plan.position,
        &plan.rotation,
        item,
        &bounds,
        false,
        -1,
    );
}

pub(super) unsafe extern "C" fn drop_special(ai: usize, target: usize, owner: usize, flag: u8) {
    if PLANS.with(|plans| {
        plans
            .borrow()
            .last()
            .and_then(Option::as_ref)
            .is_some_and(|plan| plan.members.iter().any(|member| member.ai == ai))
    }) {
        return;
    }
    let original: DropSpecial = core::mem::transmute(DROP_ORIGINAL.load(Ordering::Acquire));
    original(ai, target, owner, flag);
}

unsafe fn revalidate(plan: &Plan) -> Result<(), &'static str> {
    if plan.members.is_empty() || plan.members.len() > MAX_MEMBERS {
        return Err("invalid planned squad size");
    }
    if word(plan.holder, HOLDER_SCRIPT) != plan.script {
        return Err("canonical squad script changed after preflight");
    }
    let roster = vector(plan.holder, HOLDER_ROSTER, 8, MAX_MEMBERS)
        .ok_or("squad roster changed after preflight")?;
    slot_cache(plan.holder)?;
    let context_fn: Unary = core::mem::transmute(CONTEXT.load(Ordering::Acquire));
    let slot_fn: Slot = core::mem::transmute(SLOT.load(Ordering::Acquire));
    for member in &plan.members {
        if !(0..count(roster, 8)).any(|i| word(roster.begin, i * 8) == member.entity) {
            return Err("planned member left the canonical squad");
        }
        let Some((_facets, ai, gunner, damageable)) = member_parts(member.entity) else {
            return Err("planned member facet disappeared");
        };
        if ai != member.ai
            || gunner != member.gunner
            || damageable == 0
            || *((damageable + 0x18) as *const u8) == 0
        {
            return Err("planned member is no longer an active squad human");
        }
        if word(member.gunner, 0) != HUMAN.load(Ordering::Acquire)
            || word(member.gunner, 0x20) != member.entity
            || word(member.gunner, 0x28) != member.special_gun
            || word(member.gunner, 0x30) != member.special_item
            || *((member.gunner + 0xcc) as *const i32) != member.special_slot
            || *((member.gunner + 0x94) as *const i32) != member.selected_gun_index
        {
            return Err("squad member changed after primary preflight");
        }
        let items = vector(member.gunner, MEMBER_ITEMS, 8, MAX_ITEMS)
            .ok_or("member inventory changed after preflight")?;
        let guns = vector(member.gunner, MEMBER_GUNS, 8, MAX_GUNS)
            .ok_or("member Guns changed after preflight")?;
        if member.selected_gun_index >= 0
            && (member.selected_gun_index as usize >= count(guns, 8)
                || word(guns.begin, member.selected_gun_index as usize * 8) != member.selected_gun)
        {
            return Err("selected Gun changed after preflight");
        }
        if !(0..count(items, 8)).any(|i| word(items.begin, i * 8) == member.old_item)
            || !(0..count(guns, 8)).any(|i| word(guns.begin, i * 8) == member.old_gun)
        {
            return Err("planned primary left the member inventory");
        }
        let context = context_fn(member.gunner);
        let member_script = virtual_unary(context, 0x50).unwrap_or(0);
        if context == 0
            || member_script == 0
            || item_gun(member.old_item, member_script) != Some(member.old_config)
            || item_gun(member.old_item, 0) != Some(member.old_config)
            || slot_fn(member.old_config, context) != 0
        {
            return Err("planned primary metadata changed after preflight");
        }
        let matches = equipment::compatible_primary_items(plan.script, items)
            .map_err(equipment_error)?
            .into_iter()
            .filter(|candidate| {
                candidate.category == plan.category && candidate.item == member.old_item
            })
            .count();
        if matches != 1 {
            return Err("planned primary slot compatibility changed");
        }
        if empty_ammo(plan.incoming_gun, member.entity)? != member.empty_ammo {
            return Err("primary ammunition types changed after preflight");
        }
        if member_pool(member.ai, member.old_gun, member.entity)? != member.pool {
            return Err("primary ammunition provider changed after preflight");
        }
        if !plan.same_item
            && (0..count(guns, 8)).any(|i| {
                let gun = word(guns.begin, i * 8);
                gun != 0 && word(gun, 0x40) == plan.incoming_gun
            })
        {
            return Err("incoming primary Gun appeared after preflight");
        }
    }
    Ok(())
}

unsafe fn remove_member_primary(member: &MemberPlan) -> ammo::Rows {
    // Revalidate the whole batch before the first mutation in `add`.
    let detach: Detach = core::mem::transmute(DETACH.load(Ordering::Acquire));
    let mut held = [0usize; 3];
    held[0] = word(member.gunner, 0x28);
    held[1] = word(member.gunner, 0x30);
    held[2] = *((member.gunner + 0xcc) as *const i32) as usize;
    *((member.gunner + 0x28) as *mut usize) = member.old_gun;
    *((member.gunner + 0x30) as *mut usize) = member.old_item;
    *((member.gunner + 0xcc) as *mut i32) = 0;
    let mut output = [0usize; 4];
    detach(member.gunner, &mut output);
    let payload = ammo::read_payload(ItemBounds {
        begin: output[1],
        end: output[2],
        capacity: output[3],
    })
    .expect("guarded native primary extraction returns validated ammo rows");
    *((member.gunner + 0x28) as *mut usize) = held[0];
    *((member.gunner + 0x30) as *mut usize) = held[1];
    *((member.gunner + 0xcc) as *mut i32) = held[2] as i32;
    // Extraction destroys the Gun and releases loaded-round reservations;
    // stock drop bookkeeping separately removes its capacity and carrier.
    let remove: unsafe extern "C" fn(usize, *const usize) =
        core::mem::transmute(AMMO_REMOVE.load(Ordering::Acquire));
    remove(member.pool, output.as_ptr().add(1));
    let dispose: VoidUnary = core::mem::transmute(DISPOSE.load(Ordering::Acquire));
    dispose(output.as_mut_ptr().add(1) as usize);
    payload
}

unsafe fn replace_member(plan: &Plan, member: &MemberPlan) {
    let add: Add = core::mem::transmute(PRIMARY_ADD.load(Ordering::Acquire));
    let begin = member.empty_ammo.as_ptr() as usize;
    let end = begin + member.empty_ammo.len() * 0x10;
    add(member.gunner, plan.incoming, &[begin, end, end]);
    let Some(guns) = vector(member.gunner, MEMBER_GUNS, 8, MAX_GUNS) else {
        refuse("invalid Gun vector after native primary add");
        return;
    };
    let selected_gun = if member.selected_gun != 0 && member.selected_gun != member.old_gun {
        member.selected_gun
    } else {
        find_gun(guns, plan.incoming_gun).unwrap_or(0)
    };
    let Some(selected) = (0..count(guns, 8))
        .find(|index| selected_gun != 0 && word(guns.begin, index * 8) == selected_gun)
    else {
        refuse("selected Gun disappeared after native primary add");
        return;
    };
    // Native loadout addition only selects on a matching first geometry row.
    // An active gunner requires a valid index even when the new Gun is empty.
    *((member.gunner + 0x94) as *mut i32) = selected as i32;
    let switch: VoidBinary = core::mem::transmute(SWITCH.load(Ordering::Acquire));
    switch(member.visual, word(selected_gun, 0x40));
    let sync: VoidUnary = core::mem::transmute(VISUAL_SYNC.load(Ordering::Acquire));
    sync(member.gunner);
}

pub(super) unsafe extern "C" fn add(gunner: usize, item: usize, ammo: *const [usize; 3]) {
    let active = PLANS.with(|plans| plans.borrow().last().and_then(Option::as_ref).cloned());
    let Some(mut plan) = active.filter(|plan| {
        plan.incoming == item && plan.members.iter().any(|member| member.gunner == gunner)
    }) else {
        let original: Add = core::mem::transmute(ADD_ORIGINAL.load(Ordering::Acquire));
        original(gunner, item, ammo);
        return;
    };
    if plan.swapped {
        return;
    }
    if let Err(reason) = revalidate(&plan) {
        refuse(reason);
        return;
    }
    if plan.ammo_policy == policy::AmmoPolicy::Retain {
        if let Err(reason) = preflight_retain(&plan) {
            refuse(reason);
            return;
        }
    }
    PLANS.with(|plans| {
        if let Some(Some(active)) = plans.borrow_mut().last_mut() {
            if active.applying {
                return;
            }
            active.applying = true;
        }
    });
    // Remove the complete outgoing loadout before registering replacements:
    // their shared ammo types must not count as outgoing carriers mid-batch.
    for member in &plan.members {
        let outgoing = remove_member_primary(member);
        ammo::merge(&mut plan.outgoing_ammo, &outgoing)
            .expect("outgoing ammo shares fit the validated squad pool");
    }
    if plan.ammo_policy == policy::AmmoPolicy::Retain {
        reserve::deposit_rows(plan.members[0].pool, &plan.outgoing_ammo)
            .expect("outgoing ammo reserve passed metadata and capacity preflight");
    }
    for member in &plan.members {
        replace_member(&plan, member);
    }
    if !plan.incoming_ammo.is_empty() {
        plan.leftovers = ammo::import(plan.members[0].pool, &plan.incoming_ammo)
            .expect("validated incoming types have registered native capacity");
    }
    if plan.ammo_policy == policy::AmmoPolicy::Retain {
        let pool = plan.members[0].pool;
        let compatible_types = &plan.members[0].empty_ammo;
        let retained = reserve::snapshot(pool)
            .expect("retained ammo metadata remains resolvable in the active world");
        let compatible: ammo::Rows = retained
            .into_iter()
            .filter(|row| compatible_types.iter().any(|kind| kind[0] == row[0]))
            .collect();
        if !compatible.is_empty() {
            let leftovers = ammo::import(pool, &compatible)
                .expect("replacement ammo types have registered native capacity");
            let mut accepted = Vec::with_capacity(compatible.len());
            for requested in &compatible {
                let leftover = leftovers
                    .iter()
                    .find(|row| row[0] == requested[0])
                    .map_or(0, |row| row[1]);
                let rounds = requested[1]
                    .checked_sub(leftover)
                    .expect("native ammo importer cannot exceed the requested reserve");
                if rounds != 0 {
                    accepted.push([requested[0], rounds]);
                }
            }
            if !accepted.is_empty() {
                reserve::withdraw(pool, &accepted)
                    .expect("imported reserve rounds are owned by this squad");
            }
        }
    }
    // Resolve transient rows after native callbacks, without retaining a row
    // pointer across them. Loaded holders can have no cache at all.
    if let Ok(Some(cache)) = slot_cache(plan.holder) {
        if let Some(row) = cache_row(cache, plan.members[0].slot.definition) {
            *((row + 8) as *mut usize) = plan.incoming;
        }
    }
    plan.swapped = true;
    PLANS.with(|plans| {
        if let Some(Some(active)) = plans.borrow_mut().last_mut() {
            active.swapped = true;
            active.outgoing_ammo = plan.outgoing_ammo;
            active.leftovers = plan.leftovers;
        }
    });
}

#[cfg(test)]
#[path = "squad_collection_tests.rs"]
mod tests;
