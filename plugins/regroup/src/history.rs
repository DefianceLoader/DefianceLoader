//! Live-world origin ledger. Native weak junctions prevent stale entity pointers
//! from becoming a different soldier after deletion/address reuse. This ledger
//! deliberately does not claim to survive save/load or campaign transitions.
use super::*;
use std::sync::{Arc, Mutex};

struct Handle(usize);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            pair(b::WEAK_BIND, &mut self.0 as *mut usize as usize, 0);
        }
    }
}
#[derive(Clone)]
struct Weak(Arc<Handle>);
impl Weak {
    unsafe fn new(e: usize) -> Self {
        let mut handle = 0;
        pair(b::WEAK_BIND, &mut handle as *mut usize as usize, e);
        Self(Arc::new(Handle(handle)))
    }
    unsafe fn get(&self) -> usize {
        junction(self.0 .0)
    }
    fn key(&self) -> usize {
        self.0 .0
    }
}

#[derive(Clone)]
struct Origin {
    // Keep the first junction allocated even after recreation changes the
    // destination, so another entity cannot reuse our origin-map key.
    _identity: Weak,
    destination: Weak,
    species: usize,
    team: Vec<u8>,
    parked_select: Option<u8>,
}
#[derive(Default)]
struct Ledger {
    world: usize,
    origins: BTreeMap<usize, Origin>,
    soldiers: BTreeMap<usize, (Weak, usize)>,
}
static LEDGER: OnceLock<Mutex<Ledger>> = OnceLock::new();
fn ledger() -> &'static Mutex<Ledger> {
    LEDGER.get_or_init(Default::default)
}

unsafe fn string_bytes(s: usize) -> Result<Vec<u8>, &'static str> {
    let length = q(s, 0x10);
    let capacity = q(s, 0x18);
    if length > capacity || length > 4096 {
        return Err("invalid original squad team string");
    }
    let data = if capacity < 16 { s } else { q(s, 0) };
    if data == 0 {
        return Err("missing original squad team string");
    }
    let mut value = std::slice::from_raw_parts(data as *const u8, length).to_vec();
    value.push(0);
    Ok(value)
}

unsafe fn reset_if_new_world(h: &mut Ledger, world: usize) {
    // A loaded save can reuse the same world address. Old weak junctions must
    // still resolve to living objects in this world before retaining history.
    let any_live = h.soldiers.values().any(|(w, _)| {
        let e = w.get();
        e != 0 && get(e, 0x78) == world
    });
    if h.world != world || (!h.soldiers.is_empty() && !any_live) {
        *h = Ledger {
            world,
            ..Default::default()
        };
    }
}

pub(super) unsafe fn remember(p: &Plan) -> Result<(), &'static str> {
    let mut h = ledger().lock().unwrap();
    reset_if_new_world(&mut h, p.world);
    for source in &p.sources {
        let original = Weak::new(source.entity);
        let key = original.key();
        for m in source.members.iter().filter(|m| m.picked) {
            let soldier = Weak::new(m.entity);
            // Never replace a soldier's first origin on a later regroup.
            if h.soldiers.contains_key(&soldier.key()) {
                continue;
            }
            // A recreated origin has a new junction but remains the same origin.
            let origin_key = h
                .origins
                .iter()
                .find(|(_, o)| o.destination.key() == key)
                .map(|(&k, _)| k)
                .unwrap_or(key);
            if !h.origins.contains_key(&origin_key) {
                let team = string_bytes(source.holder + 0xf8)?;
                h.origins.insert(
                    origin_key,
                    Origin {
                        _identity: original.clone(),
                        destination: original.clone(),
                        species: q(source.holder, 0xc0),
                        team,
                        parked_select: None,
                    },
                );
            }
            h.soldiers.insert(soldier.key(), (soldier, origin_key));
        }
    }
    Ok(())
}

// Gate only empty-origin cleanup and AI updates, never world teardown.
pub(super) unsafe fn park_if_needed(ai: usize) -> bool {
    if ai == 0 || byte(ai, 0x130) != 0 {
        return false;
    }
    let e = entity(ai);
    if e == 0 {
        return false;
    }
    let mut h = ledger().lock().unwrap();
    let Some(key) = h
        .origins
        .iter()
        .find(|(_, o)| o.destination.get() == e)
        .map(|(&k, _)| k)
    else {
        return false;
    };
    let holder = junction(q(ai, 0x1c8));
    if holder == 0 {
        return false;
    }
    let empty = q(holder, 0xa0) == q(holder, 0xa8);
    let keep = empty
        && get(e, 0x78) == h.world
        && h.soldiers.values().any(|(w, origin)| {
            if *origin != key {
                return false;
            }
            let soldier = w.get();
            if soldier == 0 || get(soldier, 0x78) != h.world {
                return false;
            }
            let soldier_ai = q(facets(soldier), 0x28);
            soldier_ai != 0 && byte(soldier_ai, 0x130) == 0
        });
    let select = q(facets(e), 0x50);
    let origin = h.origins.get_mut(&key).unwrap();
    if keep {
        if select == 0 {
            return false;
        }
        if origin.parked_select.is_none() {
            origin.parked_select = Some(byte(select, 0x18));
            *((select + 0x18) as *mut u8) = 0;
            log(
                LOG_INFO,
                "parked empty original squad; retaining its entity and metadata",
            );
        }
    } else if let Some(enabled) = origin.parked_select.take() {
        if select != 0 {
            *((select + 0x18) as *mut u8) = enabled;
        }
        log(
            LOG_INFO,
            if empty {
                "released empty original squad: no surviving tracked soldiers"
            } else {
                "reactivated original squad with its existing metadata"
            },
        );
    }
    keep
}

// Read-only UI query: never run native cleanup or change the ledger from here.
pub(super) unsafe fn is_parked(e: usize) -> bool {
    e != 0
        && ledger()
            .lock()
            .unwrap()
            .origins
            .values()
            .any(|o| o.parked_select.is_some() && o.destination.get() == e)
}

pub(super) unsafe fn wake(e: usize) {
    let ai = q(facets(e), 0x28);
    if ai != 0 {
        park_if_needed(ai);
    }
}

unsafe fn destination_members(
    e: usize,
    world: usize,
) -> Result<(usize, Vec<Member>), &'static str> {
    if get(e, 0x78) != world || !kind(e, 0x10) {
        return Err("original squad is not in this world");
    }
    let ai = q(facets(e), 0x28);
    if ai == 0 || q(ai, 0) != address(b::SQUAD_AI) || byte(ai, 0x130) != 0 {
        return Err("original squad is being removed or has unsupported AI");
    }
    let holder = get(ai, offsets().roster);
    let pool = junction(q(ai, 0x148));
    if holder == 0 || pool == 0 {
        return Err("original squad is incomplete");
    }
    let owner = q(facets(e), 0x20);
    if owner == 0 || std::mem::transmute::<usize, Predicate>(method(owner, 0x80))(owner) == 0 {
        return Err("original squad no longer belongs to the player");
    }
    let members = pointers(holder, 0xa0, 64)?
        .into_iter()
        .map(|e| member(e, pool))
        .collect::<Result<Vec<_>, _>>()?;
    let gunners = pointers(ai, 0x1e8, 64)?;
    if gunners.len() != members.len()
        || gunners.iter().copied().collect::<BTreeSet<_>>()
            != members.iter().map(|m| m.gunner).collect()
    {
        return Err("original squad combat roster is inconsistent");
    }
    if d(holder, 0xd8) < members.len() as u32 {
        return Err("original squad capacity is inconsistent");
    }
    let slots = pool_slots(pool)?;
    if members
        .iter()
        .flat_map(|m| &m.guns)
        .any(|g| g.loaded != 0 && !slots.iter().any(|s| s.ammo == g.ammo))
    {
        return Err("original squad has loaded ammunition without a pool record");
    }
    for slot in slots {
        slot.supply.split(0, members.len().max(1) as u32, 0)?;
        let loaded = members
            .iter()
            .flat_map(|m| &m.guns)
            .filter(|g| g.ammo == slot.ammo)
            .try_fold(0u32, |n, g| {
                n.checked_add(g.loaded).ok_or("loaded round overflow")
            })?;
        if loaded != slot.supply.reserved {
            return Err("original squad ammo reservations are inconsistent");
        }
    }
    Ok((holder, members))
}

pub(super) unsafe fn restore(manager: usize) -> Result<(), &'static str> {
    let selected = plan_for(manager, None, true)?;
    let tasks = {
        let mut h = ledger().lock().unwrap();
        reset_if_new_world(&mut h, selected.world);
        let mut groups: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for m in &selected.moved {
            let soldier = Weak::new(m.entity);
            if let Some((_, origin)) = h.soldiers.get(&soldier.key()) {
                groups.entry(*origin).or_default().insert(m.entity);
            }
        }
        groups
            .into_iter()
            .map(|(key, members)| (key, h.origins[&key].clone(), members))
            .collect::<Vec<_>>()
    };
    if tasks.is_empty() {
        return Err("no recorded origins for selected soldiers; history begins with regrouping in this build and lasts only in this live world");
    }
    let mut restored = 0;
    let mut destinations = Vec::new();
    for (key, origin, mut members) in tasks {
        let destination = origin.destination.get();
        if destination != 0 {
            // Already-home soldiers stay put, including on repeated restore.
            members.retain(|e| entity(q(q(facets(*e), 0x50), 0x28)) != destination);
        }
        if members.is_empty() {
            continue;
        }
        // Rebuild after each group: another group may share the same ammo pool.
        let mut p = plan_for(manager, Some(&members), true)?;
        if destination != 0 {
            let (holder, existing) = destination_members(destination, p.world)?;
            transfer_into(holder, &p, &existing)?;
            std::mem::transmute::<usize, Unary>(method(manager, 0x98))(manager);
            for source in &p.sources {
                call(b::CLEANUP, source.ai);
            }
            destinations.push(destination);
        } else {
            // Recreate the original species, not the temporary squad's type.
            // This restores grouping, not the deleted entity's campaign ID.
            p.species = origin.species;
            let mut team = origin.team.clone();
            // Force external MSVC string representation with sufficient capacity.
            team.resize(team.len().max(17), 0);
            let native_team = [
                team.as_ptr() as usize,
                0,
                origin.team.len() - 1,
                team.len() - 1,
            ];
            p.team = native_team.as_ptr() as usize;
            let e = regroup(manager, p);
            if e == 0 {
                return Err(
                    "original squad recreation failed; earlier completed restores remain recorded",
                );
            }
            ledger()
                .lock()
                .unwrap()
                .origins
                .get_mut(&key)
                .unwrap()
                .destination = Weak::new(e);
            destinations.push(e);
            log(LOG_WARN, "recreated an emptied original squad's type; campaign identity, original training and custom metadata are not restored");
        }
        restored += members.len();
        log(
            LOG_INFO,
            &format!("restore group completed: {} soldiers", members.len()),
        );
    }
    if !destinations.is_empty() {
        std::mem::transmute::<usize, Unary>(method(manager, 0x98))(manager);
        for e in destinations {
            std::mem::transmute::<usize, Pair>(method(manager, 0x60))(manager, e);
        }
    }
    log(LOG_INFO, &format!("restored {restored} selected soldiers to their recorded squad groups; already-home soldiers were unchanged"));
    Ok(())
}

#[cfg(test)]
pub(super) unsafe fn test_origin(e: usize) -> Option<usize> {
    let soldier = Weak::new(e);
    ledger()
        .lock()
        .unwrap()
        .soldiers
        .get(&soldier.key())
        .map(|(_, id)| *id)
}
#[cfg(test)]
pub(super) fn test_clear() {
    *ledger().lock().unwrap() = Ledger::default();
}
