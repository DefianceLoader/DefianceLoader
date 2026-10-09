//! Regroup preflight: reads the selected soldiers, their guns and their
//! squads' ammunition pools into a [`Plan`] without changing anything.
use super::*;

pub(super) unsafe fn pool_slots(pool: usize) -> Result<Vec<Slot>, &'static str> {
    let (begin, end) = (q(pool, 0x20), q(pool, 0x28));
    if end < begin
        || (end - begin) % 0x48 != 0
        || (end - begin) / 0x48 > 128
        || (begin == 0 && end != 0)
    {
        return Err("invalid ammunition slot vector");
    }
    let slots: Vec<_> = (begin..end)
        .step_by(0x48)
        .map(|p| Slot {
            ammo: q(p, 0),
            supply: Supply {
                capacity: d(p, 0x28),
                rounds: d(p, 0x2c),
                reserved: d(p, 0x30),
                carriers: d(p, 0x34),
                disabled: d(p, 0x3c),
            },
        })
        .collect();
    let keys: BTreeSet<_> = slots.iter().map(|s| s.ammo).collect();
    if keys.contains(&0) || keys.len() != slots.len() {
        return Err("duplicate or missing ammunition identity");
    }
    Ok(slots)
}
pub(super) unsafe fn record(pool: usize, ammo: usize) -> usize {
    let (begin, end) = (q(pool, 0x20), q(pool, 0x28));
    (begin..end)
        .step_by(0x48)
        .find(|p| q(*p, 0) == ammo)
        .unwrap_or(0)
}
pub(super) unsafe fn write_supply(pool: usize, ammo: usize, s: Supply) {
    let p = record(pool, ammo);
    // All records are preflighted or allocated before any soldier is detached.
    assert_ne!(p, 0);
    for (offset, n) in [
        (0x28, s.capacity),
        (0x2c, s.rounds),
        (0x30, s.reserved),
        (0x34, s.carriers),
        (0x3c, s.disabled),
    ] {
        put(p, offset, n);
    }
}

pub(super) unsafe fn member(e: usize, pool: usize) -> Result<Member, &'static str> {
    if e == 0 || !kind(e, 0x20) {
        return Err("selection contains a non-infantry member");
    }
    let f = facets(e);
    let (ai, select, team) = (q(f, 0x28), q(f, 0x50), q(f, 0x20));
    if ai == 0
        || q(ai, 0) != address(sites::HUMAN_AI)
        || select == 0
        || byte(select, 0x18) == 0
        || byte(ai, 0x130) != 0
    {
        return Err("only living selectable infantry are supported");
    }
    if team == 0 || std::mem::transmute::<usize, Predicate>(method(team, 0x80))(team) == 0 {
        return Err("all members must belong to the player");
    }
    for offset in [0x210, 0x2a8, 0x2b0] {
        if junction(q(ai, offset)) != 0 {
            return Err("leave buildings and vehicles before regrouping");
        }
    }
    if junction(q(ai, 0x148)) != pool {
        return Err("member has a different ammunition pool");
    }
    let gunner = junction(q(ai, 0x1f0));
    if gunner == 0 {
        return Err("member has no infantry gunner");
    }
    let count = get(gunner, 0xd0);
    if count > 16 {
        return Err("unsupported weapon count");
    }
    let mut guns = Vec::new();
    for index in 0..count {
        let gun = std::mem::transmute::<usize, unsafe extern "C" fn(usize, usize) -> usize>(
            method(gunner, 0xe0),
        )(gunner, index);
        if gun == 0 || q(gun, 0) != address(sites::GUN) || junction(q(gun, 0x58)) != pool {
            return Err("unsupported weapon or ammunition ownership");
        }
        let spec = q(gun, 0x40);
        if spec == 0 {
            return Err("weapon has no specification");
        }
        let begin = q(spec, 0x210);
        let end = q(spec, 0x218);
        if end < begin || (end - begin) % 16 != 0 || (end - begin) / 16 > 128 {
            return Err("invalid weapon ammunition types");
        }
        guns.push(Gun {
            ptr: gun,
            ammo: q(gun, 0x50),
            loaded: d(gun, 0xdc),
            supported: (begin..end).step_by(16).map(|p| q(p, 0)).collect(),
        });
    }
    let pins = if byte(select, 0x19) == 0xa5 {
        (byte(select, 0x1e), byte(select, 0x1f))
    } else {
        (0, 0)
    };
    Ok(Member {
        entity: e,
        ai,
        gunner,
        select,
        picked: selected(select),
        guns,
        pins,
    })
}

pub(super) unsafe fn plan(manager: usize) -> Result<Plan, &'static str> {
    plan_for(manager, None, false)
}
pub(super) unsafe fn plan_for(
    manager: usize,
    requested: Option<&BTreeSet<usize>>,
    allow_whole: bool,
) -> Result<Plan, &'static str> {
    let mut squads = BTreeSet::new();
    let candidates = match requested {
        Some(members) => members.iter().copied().collect(),
        None => pointers(manager, 0x40, 100_000)?,
    };
    for e in candidates {
        if e == 0 {
            continue;
        }
        let f = facets(e);
        let select = q(f, 0x50);
        if requested.is_none() && !selected(select) {
            continue;
        }
        if kind(e, 0x10) {
            squads.insert(e);
        } else if kind(e, 0x20) {
            let parent = q(select, 0x28);
            if parent == 0 {
                return Err("standalone infantry cannot be regrouped yet");
            }
            squads.insert(entity(parent));
        } else {
            return Err("select only infantry before regrouping");
        }
    }
    let mut result = Plan {
        world: 0,
        species: 0,
        team: 0,
        position: [0.; 3],
        sources: vec![],
        moved: vec![],
        pools: BTreeMap::new(),
        changes: vec![],
    };
    let mut unique = BTreeSet::new();
    for e in squads {
        if e == 0 {
            return Err("missing source squad");
        }
        let ai = squad_ai(e);
        if ai == 0 || byte(ai, 0x130) != 0 {
            return Err("unsupported source squad AI");
        }
        let holder = get(ai, offsets().roster);
        let pool = junction(q(ai, 0x148));
        if holder == 0 || pool == 0 {
            return Err("source squad is incomplete");
        }
        let roster = pointers(holder, 0xa0, 64)?;
        if roster.is_empty() {
            return Err("source squad is empty");
        }
        let mut members = Vec::new();
        for e in roster {
            if !unique.insert(e) {
                return Err("duplicate source member");
            }
            let mut m = member(e, pool)?;
            if let Some(requested) = requested {
                m.picked = requested.contains(&e);
            }
            members.push(m);
        }
        if !members.iter().any(|m| m.picked) {
            continue;
        }
        let native_gunners = pointers(ai, 0x1e8, 64)?;
        let expected: BTreeSet<_> = members.iter().map(|m| m.gunner).collect();
        if expected.len() != members.len()
            || native_gunners.len() != members.len()
            || native_gunners.iter().copied().collect::<BTreeSet<_>>() != expected
        {
            return Err("squad roster and combat roster disagree");
        }
        let world = get(e, 0x78);
        if world == 0 {
            return Err("source squad has no world");
        }
        if result.world != 0 && result.world != world {
            return Err("selection spans different worlds");
        }
        result.world = world;
        if result.sources.is_empty() {
            result.species = q(holder, 0xc0);
            result.team = holder + 0xf8;
            let pos = q(facets(members.iter().find(|m| m.picked).unwrap().entity), 0);
            if pos == 0 {
                return Err("member has no world position");
            }
            result.position = *(get(pos, 0x58) as *const [f32; 3]);
        }
        if result.sources.iter().any(|s| s.pool == pool) {
            return Err("source squads share an ammunition pool");
        }
        let slots = pool_slots(pool)?;
        let capacity = d(holder, 0xd8);
        if capacity < members.len() as u32 {
            return Err("source squad capacity is inconsistent");
        }
        result
            .moved
            .extend(members.iter().filter(|m| m.picked).cloned());
        result.sources.push(Source {
            entity: e,
            ai,
            holder,
            pool,
            slots,
            members,
            capacity,
        });
    }
    if result.moved.is_empty() || result.moved.len() > limits().soldiers {
        return Err("selection exceeds effective max_soldiers (current native command ceiling: 20) or contains no infantry");
    }
    if !allow_whole
        && result.sources.len() == 1
        && result.sources[0].members.len() == result.moved.len()
    {
        return Err("those soldiers already form one squad");
    }
    prepare_ammo(&mut result)?;
    Ok(result)
}

pub(super) fn prepare_ammo(result: &mut Plan) -> Result<(), &'static str> {
    result.pools.clear();
    result.changes.clear();
    for source in &result.sources {
        for slot in &source.slots {
            let compatible = |m: &&Member| m.guns.iter().any(|g| g.supported.contains(&slot.ammo));
            let mut total = source.members.iter().filter(compatible).count() as u32;
            let mut picked = source
                .members
                .iter()
                .filter(|m| m.picked)
                .filter(compatible)
                .count() as u32;
            let loaded = |picked_only: bool| -> Result<u32, &'static str> {
                source
                    .members
                    .iter()
                    .filter(|m| !picked_only || m.picked)
                    .flat_map(|m| &m.guns)
                    .filter(|g| g.ammo == slot.ammo)
                    .try_fold(0u32, |n, g| {
                        n.checked_add(g.loaded).ok_or("loaded round overflow")
                    })
            };
            if loaded(false)? != slot.supply.reserved {
                return Err("loaded ammunition accounting differs from the squad pool");
            }
            if total == 0 {
                total = source.members.len() as u32;
                picked = source.members.iter().filter(|m| m.picked).count() as u32;
            }
            let (moved, left) = slot.supply.split(picked, total, loaded(true)?)?;
            if picked != 0 {
                let merged = match result.pools.get(&slot.ammo) {
                    Some(old) => old.combine(moved)?,
                    None => moved,
                };
                result.pools.insert(slot.ammo, merged);
            }
            result.changes.push(PoolChange {
                source: source.pool,
                ammo: slot.ammo,
                before: slot.supply,
                left,
            });
        }
        // Every loaded weapon must have a known slot, including types not used
        // by the first source squad's default loadout.
        if source
            .members
            .iter()
            .flat_map(|m| &m.guns)
            .any(|g| g.loaded != 0 && !source.slots.iter().any(|s| s.ammo == g.ammo))
        {
            return Err("a loaded weapon has no matching ammunition slot");
        }
    }
    if result.pools.len() > ammo_limit() {
        return Err("combined ammunition types exceed max_weapon_types or installed menu capacity");
    }
    Ok(())
}
