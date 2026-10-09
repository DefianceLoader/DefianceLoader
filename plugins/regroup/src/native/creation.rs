//! Squad creation: spawns the destination squad through the native factory
//! and, through the scoped call-site hooks, fills it with the planned
//! soldiers instead of the species' default members and weapon templates.
use super::*;

pub(super) static CREATE: AtomicUsize = AtomicUsize::new(0);
pub(super) static SPAWN_FALLBACK: AtomicUsize = AtomicUsize::new(0);
pub(super) type AssignString = unsafe extern "C" fn(usize, usize, usize) -> usize;

/// The spawn-menu factory substitutes a default script/species name when the
/// requested name is absent from its menu list. Existing squads need not belong
/// to that list. Keep the factory's already-copied requested name for regroup;
/// all ordinary game spawns still call the original string assignment.
pub(super) unsafe extern "C" fn keep_requested_species(
    dst: usize,
    src: usize,
    len: usize,
) -> usize {
    if CONTEXT.with(|p| p.borrow().is_some()) {
        log(
            LOG_DEBUG,
            "regroup factory: retained requested squad type instead of menu default",
        );
        dst
    } else {
        std::mem::transmute::<usize, AssignString>(SPAWN_FALLBACK.load(Ordering::Relaxed))(
            dst, src, len,
        )
    }
}
pub(super) static WIRE: AtomicUsize = AtomicUsize::new(0);
pub(super) static TEMPLATES: AtomicUsize = AtomicUsize::new(0);

/// Replaces only the call inside SquadHolderFacet::add. Normal spawns and
/// gameplay use the original. During regrouping retain every gun's loaded
/// magazine, chosen ammo and reload state; the stock binder resets them.
pub(super) unsafe extern "C" fn wire(holder: usize, e: usize) {
    if BINDING_HOLDER.with(|p| p.get()) != holder {
        std::mem::transmute::<usize, Pair>(WIRE.load(Ordering::Relaxed))(holder, e);
        return;
    }
    let ai = q(facets(entity(holder)), 0x28);
    let pool = junction(q(ai, 0x148));
    let member_ai = q(facets(e), 0x28);
    let gunner = junction(q(member_ai, 0x1f0));
    for n in 0..get(gunner, 0xd0) {
        let gun = std::mem::transmute::<usize, unsafe extern "C" fn(usize, usize) -> usize>(
            method(gunner, 0xe0),
        )(gunner, n);
        pair(sites::BIND, gun + 0x58, pool);
    }
    pair(sites::BIND, member_ai + 0x148, pool);
    // The stock reallocation helper is used only when the vector is full.
    let append: unsafe extern "C" fn(usize, usize, *const usize) =
        std::mem::transmute(address(sites::APPEND));
    let end = q(ai, 0x1f0);
    if end < q(ai, 0x1f8) {
        *(end as *mut usize) = gunner;
        *((ai + 0x1f0) as *mut usize) = end + 8;
    } else {
        append(ai + 0x1e8, end, &gunner);
    }
}

// The factory resolves a species by its MSVC string at +8; pointer identity
// is not a stable identifier across separately instantiated species objects.
pub(super) unsafe fn species_id(species: usize) -> Option<Vec<u8>> {
    if species == 0 {
        return None;
    }
    let name = species + 8;
    let len = q(name, 0x10);
    let capacity = q(name, 0x18);
    if len == 0 || len > 4096 || len > capacity || (capacity < 16 && len > 15) {
        return None;
    }
    let data = if capacity < 16 { name } else { q(name, 0) };
    if data == 0 {
        return None;
    }
    let bytes = std::slice::from_raw_parts(data as *const u8, len);
    if bytes.contains(&0) {
        return None;
    }
    Some(bytes.to_vec())
}
pub(super) unsafe fn same_species(actual: usize, expected: usize) -> bool {
    if actual == 0 || expected == 0 {
        return false;
    }
    if actual == expected {
        return true;
    }
    // Never accept a different native species class just because its text matches.
    if q(actual, 0) == 0 || q(actual, 0) != q(expected, 0) {
        return false;
    }
    match (species_id(actual), species_id(expected)) {
        (Some(actual), Some(expected)) => actual == expected,
        _ => false,
    }
}
pub(super) unsafe fn species_label(species: usize) -> String {
    species_id(species).map_or_else(
        || "<invalid>".into(),
        |id| format!("{:?}", String::from_utf8_lossy(&id)),
    )
}

pub(super) unsafe fn populate(holder: usize, p: &Plan) -> Result<(), &'static str> {
    let e = entity(holder);
    if e == 0 || get(e, 0x78) != p.world || !same_species(q(holder, 0xc0), p.species) {
        return Err("constructor did not produce the requested squad type");
    }
    let ai = squad_ai(e);
    if ai == 0 {
        return Err("destination has no squad AI");
    }
    let pool = junction(q(ai, 0x148));
    if pool == 0 {
        return Err("destination has no ammunition pool");
    }
    if !pointers(holder, 0xa0, 64)?.is_empty() {
        return Err("destination already contains soldier entities");
    }
    if !pointers(ai, 0x1e8, 64)?.is_empty() {
        return Err("destination already contains combat gunners");
    }
    if q(holder, 0x138) != q(holder, 0x140) {
        return Err("destination still contains species weapon templates");
    }
    if q(holder, 0x130) != 0 {
        return Err("destination still contains pending weapon assignments");
    }
    if !pool_slots(pool)?.is_empty() {
        return Err("destination already contains ammunition records");
    }
    put(holder, 0xd8, 0);
    transfer_into(holder, p, &[])
}

pub(super) unsafe fn transfer_into(
    holder: usize,
    p: &Plan,
    existing: &[Member],
) -> Result<(), &'static str> {
    let e = entity(holder);
    let ai = q(facets(e), 0x28);
    let pool = junction(q(ai, 0x148));
    if p.sources
        .iter()
        .any(|s| s.holder == holder || s.pool == pool)
    {
        return Err("restore destination is also a transfer source");
    }
    let total = existing.len() + p.moved.len();
    if total > limits().soldiers {
        return Err("restored squad would exceed effective max_soldiers (current native command ceiling: 20)");
    }
    let capacity = d(holder, 0xd8)
        .checked_add(p.moved.len() as u32)
        .ok_or("squad capacity overflow")?;
    let old = pool_slots(pool)?;
    let old_slots: Vec<_> = old.iter().map(|s| s.ammo).collect();
    let mut combined = p.pools.clone();
    for slot in &old {
        let sum = match combined.get(&slot.ammo) {
            Some(incoming) => slot.supply.combine(*incoming)?,
            None => slot.supply,
        };
        combined.insert(slot.ammo, sum);
    }
    if combined.len() > ammo_limit() {
        return Err("restored ammunition types exceed max_weapon_types or installed menu capacity");
    }
    pair(sites::RESERVE, holder + 0xa0, total);
    pair(sites::RESERVE, ai + 0x1e8, total);
    let insert: unsafe extern "C" fn(usize, usize, u32, u32, u32) =
        std::mem::transmute(method(pool, 0x30));
    // Allocate missing records with zero counts. Commit counts only after all
    // source snapshots have been rechecked, avoiding duplicate ammo on refusal.
    for &ammo in combined.keys() {
        if record(pool, ammo) == 0 {
            insert(pool, ammo, 0, 0, 0);
        }
    }
    // Native insertion sorts slots, so resolve identities afresh afterward.

    let new_slots: Vec<_> = pool_slots(pool)?.iter().map(|s| s.ammo).collect();
    for m in existing {
        let pins = remap_pins(&old_slots, &new_slots, m.pins.0, m.pins.1);
        *((m.select + 0x1e) as *mut u8) = pins.0;
        *((m.select + 0x1f) as *mut u8) = pins.1;
    }
    // All fallible validation and destination allocation precedes detachment.
    for source in &p.sources {
        if entity(source.holder) != source.entity
            || pointers(source.holder, 0xa0, 64)?
                != source.members.iter().map(|m| m.entity).collect::<Vec<_>>()
        {
            return Err("source roster changed during construction");
        }
    }
    for change in &p.changes {
        let current = pool_slots(change.source)?
            .into_iter()
            .find(|s| s.ammo == change.ammo);
        if current.map(|s| s.supply) != Some(change.before) {
            return Err("source ammunition changed during construction");
        }
    }
    for (&ammo, &s) in &combined {
        write_supply(pool, ammo, s);
    }
    for change in &p.changes {
        write_supply(change.source, change.ammo, change.left);
    }
    BINDING_HOLDER.with(|v| v.set(holder));
    for source in &p.sources {
        let old_slots: Vec<_> = source.slots.iter().map(|s| s.ammo).collect();
        let moved = source.members.iter().filter(|m| m.picked).count() as u32;
        for m in source.members.iter().filter(|m| m.picked) {
            pair(sites::REMOVE_GUNNER, source.ai, m.gunner);
            pair(sites::REMOVE, source.holder, m.entity);
            pair(sites::ADD, holder, m.entity);
            let pins = remap_pins(&old_slots, &new_slots, m.pins.0, m.pins.1);
            *((m.select + 0x1e) as *mut u8) = pins.0;
            *((m.select + 0x1f) as *mut u8) = pins.1;
        }
        put(source.holder, 0xd8, source.capacity - moved);
    }
    BINDING_HOLDER.with(|v| v.set(0));
    put(holder, 0xd8, capacity);
    if pointers(holder, 0xa0, 64)?.len() != total
        || pointers(ai, 0x1e8, 64)?.len() != total
        || existing.iter().chain(p.moved.iter()).any(|m| {
            let parent = q(m.select, 0x28);
            parent == 0
                || entity(parent) != e
                || m.guns
                    .iter()
                    .any(|g| q(g.ptr, 0x50) != g.ammo || d(g.ptr, 0xdc) != g.loaded)
        })
    {
        return Err("transfer completed but roster/weapon verification failed; inspect the resulting squads");
    }
    history::wake(e);
    Ok(())
}

pub(super) unsafe extern "C" fn prepare_templates(holder: usize, config: usize) {
    let active = CONTEXT.with(|p| p.borrow().clone());
    if let Some(p) = active {
        let e = entity(holder);
        let actual_species = q(holder, 0xc0);
        let actual_world = if e == 0 { 0 } else { get(e, 0x78) };
        if !same_species(actual_species, p.species) || actual_world != p.world {
            log(LOG_ERROR, &format!("constructor mismatch: holder={holder:#x}, entity={e:#x}, species={actual_species:#x} id={} vtable={:#x} expected={:#x} id={} vtable={:#x}, world={actual_world:#x} expected={:#x}; default soldiers will be suppressed", species_label(actual_species), if actual_species == 0 { 0 } else { q(actual_species, 0) }, p.species, species_label(p.species), if p.species == 0 { 0 } else { q(p.species, 0) }, p.world));
        }
        if !CREATION_ABORTED.with(Cell::get)
            && PREPARED_HOLDER.with(|v| v.get()) == 0
            && e != 0
            && same_species(q(holder, 0xc0), p.species)
            && get(e, 0x78) == p.world
        {
            // Native 443460 -> 444110 unconditionally copies species weapon
            // slots, even with an empty weapons override. Skip before either
            // templates or their default ammunition are allocated.
            if actual_species != p.species {
                log(LOG_DEBUG, &format!("regroup destination: matched separately allocated species {} by identifier and native type", species_label(actual_species)));
            }
            PREPARED_HOLDER.with(|v| v.set(holder));
            log(
                LOG_DEBUG,
                "regroup destination: skipped default weapon templates",
            );
            return;
        }
    }
    std::mem::transmute::<usize, Pair>(TEMPLATES.load(Ordering::Relaxed))(holder, config);
}

pub(super) unsafe extern "C" fn create_members(holder: usize, owned_names: usize) {
    let active = CONTEXT.with(|p| p.borrow().clone());
    let Some(p) = active else {
        std::mem::transmute::<usize, Pair>(CREATE.load(Ordering::Relaxed))(holder, owned_names);
        return;
    };
    // Consume one matching constructor only; nested/unrelated construction
    // must never steal the pending members.
    if CREATION_ABORTED.with(Cell::get)
        || PREPARED_HOLDER.with(|v| v.get()) != holder
        || DESTINATION.with(|v| v.get()) != 0
    {
        CREATION_ABORTED.with(|v| v.set(true));
        log(LOG_ERROR, "regroup constructor was not claimed or was repeated; suppressed default soldier creation");
        // This by-value vector must be consumed even when the request aborts.
        call(sites::FREE_STRINGS, owned_names);
        return;
    }
    DESTINATION.with(|v| v.set(entity(holder)));
    match populate(holder, &p) {
        Ok(()) => CONSTRUCTED.with(|v| v.set(true)),
        Err(reason) => {
            CREATION_ABORTED.with(|v| v.set(true));
            log(LOG_ERROR, reason);
        }
    }
    // Stock takes this MSVC vector by value and destroys it. Do the same even
    // though the default member names are deliberately unused.
    call(sites::FREE_STRINGS, owned_names);
}

pub(super) unsafe fn regroup(manager: usize, p: Plan) -> usize {
    if CREATION_BLOCKED.with(Cell::get) {
        log(LOG_ERROR, "regroup/restore creation blocked after an unverified result; restart and reload a pre-error save");
        return 0;
    }
    CREATION_ABORTED.with(|v| v.set(false));
    let empty_string = [0usize, 0, 0, 15];
    let empty_vec = [0usize; 3];
    type Spawn = unsafe extern "C" fn(
        *const f32,
        usize,
        usize,
        usize,
        f32,
        *const usize,
        usize,
        *const usize,
        *const usize,
        usize,
        usize,
        usize,
        u32,
    ) -> usize;
    let spawn: Spawn = std::mem::transmute(address(sites::SPAWN));
    DESTINATION.with(|v| v.set(0));
    PREPARED_HOLDER.with(|v| v.set(0));
    CONSTRUCTED.with(|v| v.set(false));
    CONTEXT.with(|v| *v.borrow_mut() = Some(p.clone()));
    // The scoped template hook suppresses species slots; an empty weapons
    // argument alone does not. The member hook supplies existing soldiers.
    let result = spawn(
        p.position.as_ptr(),
        p.world,
        p.team,
        p.species + 8,
        0.,
        empty_string.as_ptr(),
        0,
        std::ptr::null(),
        empty_vec.as_ptr(),
        0,
        0,
        0,
        u32::MAX,
    );
    CONTEXT.with(|v| *v.borrow_mut() = None);
    PREPARED_HOLDER.with(|v| v.set(0));
    BINDING_HOLDER.with(|v| v.set(0));
    let destination = DESTINATION.with(|v| v.get());
    let complete = CONSTRUCTED.with(|v| v.get()) && !CREATION_ABORTED.with(Cell::get);
    if result == 0 || result != destination || !complete {
        CREATION_BLOCKED.with(|v| v.set(true));
        log(LOG_ERROR, &format!("native squad creation did not complete: result={result:#x}, claimed={destination:#x}, transferred={}, aborted={}; further regroup/restore attempts blocked until restart",
            CONSTRUCTED.with(Cell::get), CREATION_ABORTED.with(Cell::get)));
        if result != 0 && !complete {
            let ai = squad_ai(result);
            // Cleanup is only valid for an empty squad. Never delete a populated
            // result or one holding transferred soldiers after a partial failure.
            if ai != 0 {
                let holder = get(ai, offsets().roster);
                if holder != 0 && pointers(holder, 0xa0, 64).is_ok_and(|m| m.is_empty()) {
                    call(sites::CLEANUP, ai);
                    log(
                        LOG_DEBUG,
                        "requested native cleanup of empty failed destination",
                    );
                } else {
                    log(LOG_ERROR, "failed destination contains members; retained for inspection, reload a pre-error save");
                }
            }
        }
        return 0;
    }
    std::mem::transmute::<usize, Unary>(method(manager, 0x98))(manager);
    for source in &p.sources {
        call(sites::CLEANUP, source.ai);
    }
    std::mem::transmute::<usize, Pair>(method(manager, 0x60))(manager, result);
    let actual = q(facets(result), 0x28);
    let pool = junction(q(actual, 0x148));
    let valid = p.moved.iter().all(|m| {
        let parent = q(m.select, 0x28);
        parent != 0
            && entity(parent) == result
            && q(facets(m.entity), 0x28) == m.ai
            && m.guns
                .iter()
                .all(|g| q(g.ptr, 0x50) == g.ammo && d(g.ptr, 0xdc) == g.loaded)
    }) && pointers(actual, 0x1e8, 64)
        .map(|v| v.len() == p.moved.len())
        .unwrap_or(false)
        && pool_slots(pool)
            .map(|slots| {
                slots.len() == p.pools.len()
                    && slots
                        .iter()
                        .all(|s| p.pools.get(&s.ammo) == Some(&s.supply))
            })
            .unwrap_or(false);
    log(if valid {LOG_INFO}else{LOG_ERROR},&format!("formed squad from {} soldiers in {} source squads; identity/magazine/ammunition verification={valid}",p.moved.len(),p.sources.len()));
    if valid {
        result
    } else {
        CREATION_BLOCKED.with(|v| v.set(true));
        log(
            LOG_ERROR,
            "verification failed; further regroup/restore attempts blocked until restart",
        );
        0
    }
}
