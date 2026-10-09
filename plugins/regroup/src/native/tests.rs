use super::*;
static HISTORY_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn empty_original_parks_wakes_and_releases_after_last_survivor_dies() {
    let _serial = HISTORY_TEST.lock().unwrap();
    unsafe extern "C" fn stock_cleanup(_: usize) {
        event("stock_cleanup");
    }
    unsafe extern "C" fn stock_update(_: usize, elapsed: f32) {
        assert_eq!(elapsed, 0.25);
        event("stock_update");
    }
    unsafe extern "C" fn stock_hover(_: usize) {
        event("stock_hover");
    }
    unsafe extern "C" fn visibility(root: usize, visible: u8) {
        *((root + 8) as *mut u8) = visible;
        event("hide_hover");
    }
    unsafe {
        HOVER_ORIGINAL.store(stock_hover as *const () as usize, Ordering::Relaxed);
        CLEANUP_ORIGINAL.store(stock_cleanup as *const () as usize, Ordering::Relaxed);
        UPDATE_ORIGINAL.store(stock_update as *const () as usize, Ordering::Relaxed);
        let (mut p, dest) = fixture();
        history::test_clear();
        // Move all of one source, leaving its actual original entity empty.
        p.sources.truncate(1);
        for m in &mut p.sources[0].members {
            m.picked = true;
        }
        p.moved = p.sources[0].members.clone();
        prepare_ammo(&mut p).unwrap();
        let original = p.sources[0].clone();
        let icon = alloc(0x180);
        let root = alloc(0x20);
        let widget_vt = alloc(0x50);
        set(widget_vt, 0x48, visibility as *const () as usize);
        set(root, 0, widget_vt);
        set(icon, 0x140, root);
        bind(icon + 0x160, original.entity);
        let select = q(facets(original.entity), 0x50);
        *((select + 0x18) as *mut u8) = 1;
        set(original.ai, 0x280, 0xabcdef);
        history::remember(&p).unwrap();
        assert!(!history::park_if_needed(original.ai));
        populate(dest.holder, &p).unwrap();
        assert!(history::park_if_needed(original.ai));
        assert!(history::park_if_needed(original.ai));
        assert_eq!(byte(select, 0x18), 0);
        assert!(history::is_parked(original.entity));
        assert!(!history::is_parked(dest.entity));
        assert!(!history::is_parked(0));
        *((root + 8) as *mut u8) = 1;
        EVENTS.with(|v| v.borrow_mut().clear());
        update_hover(icon);
        assert_eq!(byte(root, 8), 0);
        assert_eq!(EVENTS.with(|v| v.borrow().clone()), vec!["hide_hover"]);
        EVENTS.with(|v| v.borrow_mut().clear());
        cleanup_empty(original.ai);
        update_squad(original.ai, 0.25);
        assert!(EVENTS.with(|v| v.borrow().is_empty()));
        cleanup_empty(dest.ai);
        update_squad(dest.ai, 0.25);
        assert_eq!(
            EVENTS.with(|v| v.borrow().clone()),
            vec!["stock_cleanup", "stock_update"]
        );
        assert_eq!(q(original.ai, 0x280), 0xabcdef);
        assert!(!history::park_if_needed(dest.ai));
        let mut current = dest.clone();
        current.members = p.moved.clone();
        current.slots = pool_slots(dest.pool).unwrap();
        current.capacity = d(dest.holder, 0xd8);
        let mut back = p.clone();
        back.sources = vec![current];
        prepare_ammo(&mut back).unwrap();
        transfer_into(original.holder, &back, &[]).unwrap();
        assert_eq!(byte(select, 0x18), 1);
        assert!(!history::is_parked(original.entity));
        EVENTS.with(|v| v.borrow_mut().clear());
        update_hover(icon);
        assert_eq!(EVENTS.with(|v| v.borrow().clone()), vec!["stock_hover"]);
        assert_eq!(q(original.ai, 0x280), 0xabcdef);
        assert!(!history::park_if_needed(original.ai));
        // Repeat transfer, then mark every recorded soldier destroyed.
        transfer_into(dest.holder, &p, &[]).unwrap();
        assert!(history::park_if_needed(original.ai));
        for m in &p.moved {
            *((m.ai + 0x130) as *mut u8) = 1;
        }
        assert!(!history::park_if_needed(original.ai));
        assert_eq!(byte(select, 0x18), 1);
        history::test_clear();
    }
}
#[test]
fn input_resolves_the_current_manager_without_caching_across_worlds() {
    unsafe extern "C" fn field8(p: usize) -> usize {
        q(p, 8)
    }
    unsafe extern "C" fn field16(p: usize) -> usize {
        q(p, 16)
    }
    unsafe extern "C" fn lookup(world: usize, team: usize) -> usize {
        if team == q(world, 16) {
            q(world, 8)
        } else {
            0
        }
    }
    unsafe {
        let world_vt = alloc(0x708);
        set(world_vt, 0x700, lookup as *const () as usize);
        let world = alloc(24);
        set(world, 0, world_vt);
        set(world, 8, 123);
        set(world, 16, 456);
        let sim_vt = alloc(0x40);
        set(sim_vt, 0x38, field8 as *const () as usize);
        let sim = alloc(16);
        set(sim, 0, sim_vt);
        set(sim, 8, world);
        let player_vt = alloc(0x70);
        set(player_vt, 0x68, field8 as *const () as usize);
        set(player_vt, 0x40, field16 as *const () as usize);
        let player = alloc(24);
        set(player, 0, player_vt);
        set(player, 8, sim);
        set(player, 16, 456);
        let input = alloc(0x868);
        set(input, 0x860, player);
        assert_eq!(input_manager(input), Ok(123));
        set(world, 8, 789);
        assert_eq!(input_manager(input), Ok(789));
        set(player, 16, 999);
        assert!(input_manager(input).is_err());
        set(sim, 8, 0);
        assert!(input_manager(input).is_err());
        set(player, 8, 0);
        assert!(input_manager(input).is_err());
        set(input, 0x860, 0);
        assert!(input_manager(input).is_err());
    }
}
thread_local! {
    static MEMORY: RefCell<Vec<Box<[usize]>>> = const { RefCell::new(Vec::new()) };
    static EVENTS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}
fn alloc(bytes: usize) -> usize {
    let mut block = vec![0usize; bytes.div_ceil(8)].into_boxed_slice();
    let p = block.as_mut_ptr() as usize;
    MEMORY.with(|m| m.borrow_mut().push(block));
    p
}
unsafe fn set(p: usize, offset: usize, value: usize) {
    *((p + offset) as *mut usize) = value;
}
fn event(e: &'static str) {
    EVENTS.with(|v| v.borrow_mut().push(e));
}
unsafe extern "C" fn get_facets(e: usize) -> usize {
    q(e, 8)
}
unsafe extern "C" fn get_world(e: usize) -> usize {
    q(e, 16)
}
unsafe extern "C" fn gun_count(g: usize) -> usize {
    q(g, 8)
}
unsafe extern "C" fn gun_at(g: usize, i: usize) -> usize {
    q(g, 16 + i * 8)
}
unsafe extern "C" fn bind(p: usize, target: usize) {
    let handle = alloc(0x20);
    set(handle, 0x10, target);
    set(p, 0, handle);
}
unsafe extern "C" fn weak_bind(p: usize, target: usize) {
    if target == 0 {
        set(p, 0, 0);
        return;
    }
    let mut handle = q(target, 24);
    if handle == 0 {
        handle = alloc(0x20);
        set(handle, 0x10, target);
        set(target, 24, handle);
    }
    set(p, 0, handle);
}
unsafe extern "C" fn reserve(v: usize, n: usize) {
    event("reserve");
    let begin = q(v, 0);
    let len = (q(v, 8) - begin) / 8;
    assert!(n >= len);
    let data = alloc(n * 8);
    if len != 0 {
        std::ptr::copy_nonoverlapping(begin as *const usize, data as *mut usize, len);
    }
    set(v, 0, data);
    set(v, 8, data + len * 8);
    set(v, 16, data + n * 8);
}
unsafe extern "C" fn insert(pool: usize, ammo: usize, rounds: u32, cap: u32, carriers: u32) {
    event("ammo");
    let end = q(pool, 0x28);
    set(end, 0, ammo);
    put(end, 0x28, cap);
    put(end, 0x2c, rounds);
    put(end, 0x34, carriers);
    set(pool, 0x28, end + 0x48);
    let begin = q(pool, 0x20);
    let records =
        std::slice::from_raw_parts_mut(begin as *mut [usize; 9], (end + 0x48 - begin) / 0x48);
    records.sort_by_key(|r| r[0]);
}
unsafe extern "C" fn remove_gunner(ai: usize, g: usize) {
    event("remove_gunner");
    let begin = q(ai, 0x1e8);
    let end = q(ai, 0x1f0);
    let at = (begin..end).step_by(8).find(|p| q(*p, 0) == g).unwrap();
    set(at, 0, q(end - 8, 0));
    set(ai, 0x1f0, end - 8);
}
unsafe extern "C" fn remove(holder: usize, e: usize) {
    event("remove");
    let begin = q(holder, 0xa0);
    let end = q(holder, 0xa8);
    let at = (begin..end).step_by(8).find(|p| q(*p, 0) == e).unwrap();
    std::ptr::copy(
        (at + 8) as *const usize,
        at as *mut usize,
        (end - at - 8) / 8,
    );
    set(holder, 0xa8, end - 8);
    set(q(facets(e), 0x50), 0x28, 0);
}
unsafe extern "C" fn add(holder: usize, e: usize) {
    event("add");
    let end = q(holder, 0xa8);
    set(end, 0, e);
    set(holder, 0xa8, end + 8);
    set(q(facets(e), 0x50), 0x28, q(facets(entity(holder)), 0x50));
    wire(holder, e);
}
unsafe extern "C" fn free_strings(_: usize) {
    event("free_strings");
}
thread_local! {
    static MOCK_SPAWN_RESULT: Cell<usize> = const { Cell::new(0) };
}
unsafe extern "C" fn mock_spawn(
    _: *const f32,
    _: usize,
    _: usize,
    _: usize,
    _: f32,
    _: *const usize,
    _: usize,
    _: *const usize,
    _: *const usize,
    _: usize,
    _: usize,
    _: usize,
    _: u32,
) -> usize {
    event("spawn");
    MOCK_SPAWN_RESULT.with(Cell::get)
}
unsafe extern "C" fn perk_prepare(_: usize) {
    event("perk_prepare");
}
unsafe extern "C" fn perk_update(_: usize) {
    event("perk_update");
}
unsafe extern "C" fn perk_member(_: usize) {
    event("perk_member");
}
unsafe extern "C" fn perk_original(_: usize) {
    event("perk_original");
}
unsafe extern "C" fn resize_roster(out: usize, count: usize) {
    event("resize_roster");
    let old = (q(out, 8) - q(out, 0)) / 8;
    if count > old {
        reserve(out, count);
    }
    let begin = q(out, 0);
    if count > old {
        std::ptr::write_bytes((begin + old * 8) as *mut usize, 0, count - old);
    }
    set(out, 8, begin + count * 8);
}
unsafe extern "C" fn roster_original(_: usize, _: usize, _: usize) {
    event("roster_original");
}
/// The fixtures' `SquadAiFacet` vtable.
static MOCK_SQUAD_AI: OnceLock<usize> = OnceLock::new();
fn setup() {
    unsafe extern "C" fn mock_selection(facet: *mut core::ffi::c_void) -> u8 {
        let facet = facet as usize;
        if facet == 0 {
            0
        } else {
            std::mem::transmute::<usize, Predicate>(method(facet, 0x58))(facet)
        }
    }
    static MOCK_SELECTION: defiance_api::SelectionV1 = defiance_api::SelectionV1 {
        is_selected: mock_selection,
    };
    SELECTION.get_or_init(|| &MOCK_SELECTION);
    /// Core's answer for the fixtures: the AI facet when its vtable is
    /// the mocked `SquadAiFacet` one.
    unsafe extern "C" fn mock_squad_ai(e: *mut core::ffi::c_void) -> *mut core::ffi::c_void {
        let ai = q(facets(e as usize), 0x28);
        if ai != 0 && q(ai, 0) == *MOCK_SQUAD_AI.get().unwrap() {
            ai as *mut core::ffi::c_void
        } else {
            core::ptr::null_mut()
        }
    }
    static MOCK_FACETS: defiance_api::FacetsV1 = defiance_api::FacetsV1 {
        squad_ai: mock_squad_ai,
    };
    FACETS.get_or_init(|| &MOCK_FACETS);
    ADDRESSES.get_or_init(|| {
        let mut v = vec![0; sites::COUNT];
        for (i, f) in [
            (sites::SPAWN, mock_spawn as *const () as usize),
            (sites::PERK_PREPARE, perk_prepare as *const () as usize),
            (sites::PERK_UPDATE, perk_update as *const () as usize),
            (sites::PERK_MEMBER, perk_member as *const () as usize),
            (sites::RESIZE_ROSTER, resize_roster as *const () as usize),
            (sites::RESERVE, reserve as *const () as usize),
            (sites::BIND, bind as *const () as usize),
            (sites::WEAK_BIND, weak_bind as *const () as usize),
            (sites::REMOVE_GUNNER, remove_gunner as *const () as usize),
            (sites::REMOVE, remove as *const () as usize),
            (sites::ADD, add as *const () as usize),
            (sites::FREE_STRINGS, free_strings as *const () as usize),
        ] {
            v[i] = f;
        }
        unsafe extern "C" fn holder(ai: usize) -> usize {
            junction(q(ai, 0x1c8))
        }
        let mut table = Box::new([0usize; 128]);
        table[0x3b8 / 8] = holder as *const () as usize;
        MOCK_SQUAD_AI.get_or_init(|| Box::into_raw(table) as usize);
        v
    });
    EVENTS.with(|v| v.borrow_mut().clear());
    CONTEXT.with(|v| *v.borrow_mut() = None);
    DESTINATION.with(|v| v.set(0));
    PREPARED_HOLDER.with(|v| v.set(0));
    CONSTRUCTED.with(|v| v.set(false));
    CREATION_ABORTED.with(|v| v.set(false));
    CREATION_BLOCKED.with(|v| v.set(false));
}
unsafe fn entity_with(ai: usize, world: usize) -> (usize, usize) {
    let vt = alloc(0xb8);
    set(vt, 0xb0, get_facets as *const () as usize);
    set(vt, 0x78, get_world as *const () as usize);
    let e = alloc(0x20);
    let f = alloc(0x80);
    set(e, 0, vt);
    set(e, 8, f);
    set(e, 16, world);
    set(f, 0x28, ai);
    let select = alloc(0x38);
    bind(select + 0x10, e);
    set(f, 0x50, select);
    (e, select)
}
unsafe fn source(world: usize, species: usize, picks: &[bool], base_ammo: usize) -> Source {
    let ai = alloc(0x300);
    set(ai, 0, *MOCK_SQUAD_AI.get().unwrap());
    let (e, select) = entity_with(ai, world);
    let holder = alloc(0x160);
    bind(ai + 0x10, e);
    bind(ai + 0x1c8, holder);
    bind(holder + 0x10, e);
    set(holder, 0xc0, species);
    put(holder, 0xd8, picks.len() as u32);
    let pool = alloc(0x60);
    let poolvt = alloc(0x50);
    set(poolvt, 0x30, insert as *const () as usize);
    set(pool, 0, poolvt);
    let slots = alloc(0x48 * 128);
    set(pool, 0x20, slots);
    set(pool, 0x28, slots);
    bind(ai + 0x148, pool);
    let s = Supply {
        capacity: 100,
        rounds: 80,
        reserved: 5 * picks.len() as u32,
        carriers: picks.len() as u32,
        disabled: 1,
    };
    insert(pool, base_ammo, s.rounds, s.capacity, s.carriers);
    write_supply(pool, base_ammo, s);
    reserve(holder + 0xa0, picks.len());
    reserve(ai + 0x1e8, picks.len());
    let mut members = vec![];
    for &picked in picks {
        let mai = alloc(0x300);
        let (me, ms) = entity_with(mai, world);
        set(ms, 0x28, select);
        let gun = alloc(0x180);
        set(gun, 0x50, base_ammo);
        put(gun, 0xdc, 5);
        bind(gun + 0x58, pool);
        bind(mai + 0x148, pool);
        let gunner = alloc(0x30);
        let gvt = alloc(0xe8);
        set(gvt, 0xd0, gun_count as *const () as usize);
        set(gvt, 0xe0, gun_at as *const () as usize);
        set(gunner, 0, gvt);
        set(gunner, 8, 1);
        set(gunner, 16, gun);
        bind(mai + 0x1f0, gunner);
        let end = q(holder, 0xa8);
        set(end, 0, me);
        set(holder, 0xa8, end + 8);
        let end = q(ai, 0x1f0);
        set(end, 0, gunner);
        set(ai, 0x1f0, end + 8);
        members.push(Member {
            entity: me,
            ai: mai,
            gunner,
            select: ms,
            picked,
            guns: vec![Gun {
                ptr: gun,
                ammo: base_ammo,
                loaded: 5,
                supported: vec![base_ammo],
            }],
            pins: (1, 1),
        });
    }
    Source {
        entity: e,
        ai,
        holder,
        pool,
        slots: vec![Slot {
            ammo: base_ammo,
            supply: s,
        }],
        members,
        capacity: picks.len() as u32,
    }
}
unsafe fn species_fixture(name: &[u8], vtable: usize) -> usize {
    let species = alloc(0x80);
    set(species, 0, vtable);
    let data = if name.len() < 16 {
        species + 8
    } else {
        let data = alloc(name.len() + 1);
        set(species, 8, data);
        data
    };
    std::ptr::copy_nonoverlapping(name.as_ptr(), data as *mut u8, name.len());
    set(species, 0x18, name.len());
    set(species, 0x20, name.len().max(15));
    species
}
unsafe fn fixture() -> (Plan, Source) {
    setup();
    let world = 0x1111;
    let species = species_fixture(b"rifle", 0xface);
    let sources = vec![
        source(world, species, &[true, false, true], 11),
        source(world, species, &[false, true], 22),
    ];
    let mut dest = source(world, species, &[], 11);
    set(dest.pool, 0x28, q(dest.pool, 0x20));
    dest.slots.clear();
    let mut p = Plan {
        world,
        species,
        team: 0,
        position: [0.; 3],
        moved: vec![],
        sources,
        pools: BTreeMap::new(),
        changes: vec![],
    };
    for src in &p.sources {
        let n = src.members.iter().filter(|m| m.picked).count() as u32;
        p.moved
            .extend(src.members.iter().filter(|m| m.picked).cloned());
        for slot in &src.slots {
            let (moved, left) = slot
                .supply
                .split(n, src.members.len() as u32, n * 5)
                .unwrap();
            p.pools.insert(slot.ammo, moved);
            p.changes.push(PoolChange {
                source: src.pool,
                ammo: slot.ammo,
                before: slot.supply,
                left,
            });
        }
    }
    EVENTS.with(|e| e.borrow_mut().clear());
    (p, dest)
}
#[test]
fn constructor_transfers_only_picked_members_and_preserves_loaded_guns() {
    unsafe {
        let (p, dest) = fixture();
        CONTEXT.with(|v| *v.borrow_mut() = Some(p.clone()));
        prepare_templates(dest.holder, 0);
        create_members(dest.holder, 0);
        assert!(CONSTRUCTED.with(|v| v.get()));
        assert_eq!(DESTINATION.with(|v| v.get()), dest.entity);
        assert_eq!(
            pointers(dest.holder, 0xa0, 64).unwrap(),
            p.moved.iter().map(|m| m.entity).collect::<Vec<_>>()
        );
        assert_eq!(pointers(dest.ai, 0x1e8, 64).unwrap().len(), 3);
        for src in &p.sources {
            assert_eq!(
                pointers(src.holder, 0xa0, 64).unwrap(),
                src.members
                    .iter()
                    .filter(|m| !m.picked)
                    .map(|m| m.entity)
                    .collect::<Vec<_>>()
            );
            for m in &src.members {
                let target = if m.picked { dest.pool } else { src.pool };
                assert_eq!(junction(q(m.ai, 0x148)), target);
                assert_eq!(
                    entity(q(m.select, 0x28)),
                    if m.picked { dest.entity } else { src.entity }
                );
                for gun in &m.guns {
                    assert_eq!(junction(q(gun.ptr, 0x58)), target);
                    assert_eq!(d(gun.ptr, 0xdc), 5);
                    assert_eq!(q(gun.ptr, 0x50), gun.ammo);
                }
            }
        }
        for (&ammo, &s) in &p.pools {
            assert_eq!(
                pool_slots(dest.pool)
                    .unwrap()
                    .into_iter()
                    .find(|s| s.ammo == ammo)
                    .unwrap()
                    .supply,
                s
            );
        }
        let events = EVENTS.with(|v| v.borrow().clone());
        assert!(
            events.iter().rposition(|e| *e == "ammo").unwrap()
                < events.iter().position(|e| *e == "remove_gunner").unwrap()
        );
        assert_eq!(events.last(), Some(&"free_strings"));
        CONTEXT.with(|v| *v.borrow_mut() = None);
    }
}
#[test]
fn first_origin_survives_repeated_regroup_and_expired_handles_do_not_alias() {
    let _serial = HISTORY_TEST.lock().unwrap();
    unsafe {
        let (p, dest) = fixture();
        history::test_clear();
        history::remember(&p).unwrap();
        let soldier = p.moved[0].entity;
        let first = history::test_origin(soldier).unwrap();
        assert!(history::test_origin(p.sources[0].members[1].entity).is_none());
        populate(dest.holder, &p).unwrap();
        let mut temp = dest.clone();
        temp.members = p.moved.clone();
        let mut again = p.clone();
        again.sources = vec![temp];
        history::remember(&again).unwrap();
        assert_eq!(history::test_origin(soldier), Some(first));
        // Invalidate the native junction, as destruction does. Reusing the
        // raw entity address with a fresh junction must not inherit history.
        set(q(soldier, 24), 0x10, 0);
        set(soldier, 24, 0);
        assert!(history::test_origin(soldier).is_none());
        // A new world clears old entries even when some soldiers survive.
        again.world = 0x9999;
        history::remember(&again).unwrap();
        assert_ne!(history::test_origin(soldier), Some(first));
        history::test_clear();
    }
}

#[test]
fn restoring_two_origins_preserves_unselected_members_and_conserves_ammo() {
    unsafe {
        let (p, dest) = fixture();
        populate(dest.holder, &p).unwrap();
        for original in &p.sources {
            let wanted: BTreeSet<_> = original
                .members
                .iter()
                .filter(|m| m.picked)
                .map(|m| m.entity)
                .collect();
            let present = pointers(dest.holder, 0xa0, 64).unwrap();
            let mut current = dest.clone();
            current.members = p
                .moved
                .iter()
                .filter(|m| present.contains(&m.entity))
                .cloned()
                .collect();
            for m in &mut current.members {
                m.picked = wanted.contains(&m.entity);
            }
            current.slots = pool_slots(dest.pool).unwrap();
            current.capacity = d(dest.holder, 0xd8);
            let mut back = p.clone();
            back.sources = vec![current];
            back.moved = back.sources[0]
                .members
                .iter()
                .filter(|m| m.picked)
                .cloned()
                .collect();
            prepare_ammo(&mut back).unwrap();
            let stayed: Vec<_> = original
                .members
                .iter()
                .filter(|m| !m.picked)
                .cloned()
                .collect();
            transfer_into(original.holder, &back, &stayed).unwrap();
            assert_eq!(
                pointers(original.holder, 0xa0, 64)
                    .unwrap()
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
                original.members.iter().map(|m| m.entity).collect()
            );
            assert_eq!(d(original.holder, 0xd8), original.capacity);
            let actual = pool_slots(original.pool).unwrap();
            for slot in &original.slots {
                assert_eq!(
                    actual.iter().find(|s| s.ammo == slot.ammo).unwrap().supply,
                    slot.supply
                );
            }
            for m in &original.members {
                assert_eq!(entity(q(m.select, 0x28)), original.entity);
                assert_eq!(d(m.guns[0].ptr, 0xdc), m.guns[0].loaded);
            }
        }
        assert!(pointers(dest.holder, 0xa0, 64).unwrap().is_empty());
        assert!(pool_slots(dest.pool)
            .unwrap()
            .iter()
            .all(|s| s.supply.rounds == 0 && s.supply.reserved == 0));
    }
}

#[test]
fn joining_existing_squad_remaps_residents_pins_after_pool_sorting() {
    unsafe {
        let (p, _) = fixture();
        let dest = source(p.world, p.species, &[false], 44);
        let resident = &dest.members[0];
        transfer_into(dest.holder, &p, &dest.members).unwrap();
        assert_eq!(byte(resident.select, 0x1e), 4);
        assert_eq!(byte(resident.select, 0x1f), 4);
        assert_eq!(
            pool_slots(dest.pool)
                .unwrap()
                .iter()
                .map(|s| s.ammo)
                .collect::<Vec<_>>(),
            vec![11, 22, 44]
        );
        assert_eq!(pointers(dest.holder, 0xa0, 64).unwrap().len(), 4);
        assert_eq!(d(resident.guns[0].ptr, 0xdc), 5);
        assert_eq!(
            pool_slots(dest.pool).unwrap()[2].supply,
            dest.slots[0].supply
        );
    }
}

#[test]
fn expanded_limits_transfer_many_types_and_refuse_before_detachment() {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_LIMITS.with(|v| v.set(None));
        }
    }
    let _reset = Reset;
    unsafe {
        for count in [9, 10, 36, 64, 126] {
            let (mut p, dest) = fixture();
            let soldiers = count.min(64);
            p.sources = (1..=soldiers)
                .map(|n| source(p.world, p.species, &[true], n * 11))
                .collect();
            // Ammunition types can outnumber soldiers. Include unloaded
            // reserve-only types through the maximum menu capacity.
            for n in soldiers + 1..=count {
                insert(p.sources[0].pool, n * 11, 10, 20, 1);
            }
            p.sources[0].slots = pool_slots(p.sources[0].pool).unwrap();
            p.moved = p.sources.iter().flat_map(|s| s.members.clone()).collect();
            TEST_LIMITS.with(|v| v.set(Some((crate::limits::Limits::new(64, 0).unwrap(), 126))));
            prepare_ammo(&mut p).unwrap();
            assert_eq!(p.pools.len(), count);
            EVENTS.with(|v| v.borrow_mut().clear());

            // Restore rejects a smaller configured cap before touching sources.
            TEST_LIMITS.with(|v| {
                v.set(Some((
                    crate::limits::Limits::new(64, count as i64 - 1).unwrap(),
                    126,
                )))
            });
            assert!(transfer_into(dest.holder, &p, &[]).is_err());
            assert!(EVENTS.with(|v| v.borrow().is_empty()));
            assert!(prepare_ammo(&mut p).is_err());

            // Installed stock capacity also wins over an explicit larger request.
            if count > 9 {
                TEST_LIMITS
                    .with(|v| v.set(Some((crate::limits::Limits::new(64, 126).unwrap(), 9))));
                assert!(transfer_into(dest.holder, &p, &[]).is_err());
                assert!(EVENTS.with(|v| v.borrow().is_empty()));
            }
            TEST_LIMITS.with(|v| {
                v.set(Some((
                    crate::limits::Limits::new(soldiers as i64 - 1, 0).unwrap(),
                    126,
                )))
            });
            assert!(transfer_into(dest.holder, &p, &[]).is_err());
            assert!(EVENTS.with(|v| v.borrow().is_empty()));

            TEST_LIMITS.with(|v| v.set(Some((crate::limits::Limits::new(64, 0).unwrap(), 126))));
            prepare_ammo(&mut p).unwrap();
            transfer_into(dest.holder, &p, &[]).unwrap();
            assert_eq!(pointers(dest.holder, 0xa0, 64).unwrap().len(), soldiers);
            let actual = pool_slots(dest.pool).unwrap();
            assert_eq!(actual.len(), count);
            for slot in actual {
                assert_eq!(slot.supply, p.pools[&slot.ammo]);
            }
            for source in &p.sources {
                assert!(pointers(source.holder, 0xa0, 64).unwrap().is_empty());
            }
        }
    }
}

#[test]
fn restore_rejects_capacity_or_ammo_overflow_before_detaching() {
    unsafe {
        let (p, dest) = fixture();
        let repeated = vec![p.moved[0].clone(); 16];
        assert!(transfer_into(dest.holder, &p, &repeated).is_err());
        let record = q(dest.pool, 0x20);
        set(dest.pool, 0x28, record + 0x48);
        put(record, 0x28, u32::MAX);
        assert!(transfer_into(dest.holder, &p, &[]).is_err());
        assert!(EVENTS.with(|v| v.borrow().is_empty()));
        assert_eq!(pointers(p.sources[0].holder, 0xa0, 64).unwrap().len(), 3);
    }
}

#[test]
fn template_suppression_only_applies_to_the_pending_destination() {
    unsafe extern "C" fn original(holder: usize, _: usize) {
        // Model the native preparation leaving a pending assignment.
        set(holder, 0x130, 1);
        event("stock_templates");
    }
    unsafe {
        let (p, dest) = fixture();
        TEMPLATES.store(original as *const () as usize, Ordering::Relaxed);
        prepare_templates(dest.holder, 0);
        assert_eq!(q(dest.holder, 0x130), 1);
        set(dest.holder, 0x130, 0);
        EVENTS.with(|v| v.borrow_mut().clear());
        CONTEXT.with(|v| *v.borrow_mut() = Some(p.clone()));
        // A different species must use native preparation.
        set(dest.holder, 0xc0, species_fixture(b"other", 0xface));
        prepare_templates(dest.holder, 0);
        assert_eq!(PREPARED_HOLDER.with(|v| v.get()), 0);
        assert_eq!(q(dest.holder, 0x130), 1);
        set(dest.holder, 0xc0, p.species);
        set(dest.holder, 0x130, 0);
        EVENTS.with(|v| v.borrow_mut().clear());
        prepare_templates(dest.holder, 0);
        assert_eq!(PREPARED_HOLDER.with(|v| v.get()), dest.holder);
        assert_eq!(q(dest.holder, 0x130), 0);
        assert!(EVENTS.with(|v| v.borrow().is_empty()));
        // Another holder during the same request must still be forwarded.
        let other = alloc(0x150);
        set(other, 0x10, q(dest.holder, 0x10));
        set(other, 0xc0, p.species);
        prepare_templates(other, 0);
        assert_eq!(q(other, 0x130), 1);
        assert_eq!(PREPARED_HOLDER.with(|v| v.get()), dest.holder);
        CONTEXT.with(|v| *v.borrow_mut() = None);
        PREPARED_HOLDER.with(|v| v.set(0));
    }
}
#[test]
fn perk_refresh_uses_dynamic_roster_above_twenty_and_preserves_stock_paths() {
    unsafe extern "C" fn infantry(_: usize, _: u32) -> u8 {
        1
    }
    unsafe {
        for count in [0, 16, 20, 21, 22, 23, 25, 64] {
            let (p, _) = fixture();
            let src = source(p.world, p.species, &vec![true; count], 11);
            set(q(src.entity, 0), 0x98, infantry as *const () as usize);
            let perk = alloc(0x180);
            bind(perk + 0x10, src.entity);
            for member in &src.members {
                set(facets(member.entity), 0x70, alloc(0x180));
            }
            PERK_ORIGINAL.store(perk_original as *const () as usize, Ordering::Relaxed);
            EVENTS.with(|v| v.borrow_mut().clear());
            refresh_perks(perk);
            let events = EVENTS.with(|v| v.borrow().clone());
            if count <= 20 {
                assert_eq!(events, vec!["perk_original"]);
            } else {
                assert_eq!(&events[..2], &["perk_prepare", "perk_update"]);
                assert_eq!(&events[2..], vec!["perk_member"; count]);
            }
            // Member-owned facets take the original path even if their
            // parent squad is large.
            bind(perk + 0x160, src.entity);
            EVENTS.with(|v| v.borrow_mut().clear());
            refresh_perks(perk);
            assert_eq!(EVENTS.with(|v| v.borrow().clone()), vec!["perk_original"]);
        }
    }
}

#[test]
fn ui_export_resizes_before_copy_and_preserves_every_large_squad_member() {
    unsafe extern "C" fn infantry(_: usize, _: u32) -> u8 {
        1
    }
    unsafe extern "C" fn other(_: usize, _: u32) -> u8 {
        0
    }
    unsafe {
        for count in [0, 16, 20, 21, 22, 23, 25, 64] {
            let (p, _) = fixture();
            let src = source(p.world, p.species, &vec![true; count], 11);
            set(q(src.entity, 0), 0x98, infantry as *const () as usize);
            ROSTER_ORIGINAL.store(roster_original as *const () as usize, Ordering::Relaxed);
            let out = alloc(24);
            // Fresh and reused UI vectors, including shrink and growth.
            for previous in [0, 20, 64] {
                resize_roster(out, previous);
                EVENTS.with(|v| v.borrow_mut().clear());
                export_roster(0, src.entity, out);
                let events = EVENTS.with(|v| v.borrow().clone());
                if count <= 20 {
                    assert_eq!(events, vec!["roster_original"]);
                } else {
                    assert_eq!(events.first(), Some(&"resize_roster"));
                    assert!(!events.contains(&"roster_original"));
                    assert_eq!(
                        pointers(out, 0, 64).unwrap(),
                        src.members.iter().map(|m| m.entity).collect::<Vec<_>>()
                    );
                }
            }
            set(q(src.entity, 0), 0x98, other as *const () as usize);
            EVENTS.with(|v| v.borrow_mut().clear());
            export_roster(0, src.entity, out);
            export_roster(0, 0, out);
            assert_eq!(
                EVENTS.with(|v| v.borrow().clone()),
                vec!["roster_original"; 2]
            );
            set(q(src.entity, 0), 0x98, infantry as *const () as usize);
            set(src.holder, 0xa8, q(src.holder, 0xa0) + 65 * 8);
            EVENTS.with(|v| v.borrow_mut().clear());
            export_roster(0, src.entity, out);
            assert_eq!(q(out, 0), q(out, 8));
            assert_eq!(EVENTS.with(|v| v.borrow().clone()), vec!["resize_roster"]);
        }
    }
}

#[test]
fn species_identity_checks_native_type_and_exact_nonempty_identifier() {
    unsafe {
        for name in [
            b"rifle".as_slice(),
            b"long_squad_species_identifier".as_slice(),
        ] {
            let a = species_fixture(name, 0xface);
            let b = species_fixture(name, 0xface);
            assert_ne!(a, b);
            assert!(same_species(a, b));
            assert!(!same_species(a, species_fixture(name, 0xbeef)));
            assert!(!same_species(a, species_fixture(b"different", 0xface)));
            set(b, 0x18, 4097);
            assert!(!same_species(a, b));
        }
        assert!(!same_species(0, 0));
        assert!(!same_species(
            species_fixture(b"", 0xface),
            species_fixture(b"", 0xface)
        ));
    }
}

#[test]
fn separately_allocated_species_is_claimed_and_transfers_existing_soldiers() {
    unsafe {
        let (p, dest) = fixture();
        let separate = species_fixture(b"rifle", 0xface);
        set(dest.holder, 0xc0, separate);
        CONTEXT.with(|v| *v.borrow_mut() = Some(p.clone()));
        prepare_templates(dest.holder, 0);
        assert_eq!(PREPARED_HOLDER.with(Cell::get), dest.holder);
        create_members(dest.holder, 0);
        assert!(CONSTRUCTED.with(Cell::get));
        assert!(!CREATION_ABORTED.with(Cell::get));
        assert_eq!(
            pointers(dest.holder, 0xa0, 64).unwrap().len(),
            p.moved.len()
        );
        for m in &p.moved {
            assert_eq!(entity(q(m.select, 0x28)), dest.entity);
        }
        CONTEXT.with(|v| *v.borrow_mut() = None);
    }
}

#[test]
fn factory_fallback_preserves_requested_name_only_during_regroup() {
    unsafe extern "C" fn assign(dst: usize, src: usize, _: usize) -> usize {
        set(dst, 0, q(src, 0));
        event("fallback_assign");
        dst
    }
    unsafe {
        let (p, _) = fixture();
        SPAWN_FALLBACK.store(assign as *const () as usize, Ordering::Relaxed);
        let dst = alloc(32);
        let src = alloc(32);
        set(dst, 0, 123);
        set(src, 0, 456);
        CONTEXT.with(|v| *v.borrow_mut() = Some(p));
        assert_eq!(keep_requested_species(dst, src, 3), dst);
        assert_eq!(q(dst, 0), 123);
        assert!(EVENTS.with(|v| v.borrow().is_empty()));
        CONTEXT.with(|v| *v.borrow_mut() = None);
        assert_eq!(keep_requested_species(dst, src, 3), dst);
        assert_eq!(q(dst, 0), 456);
        assert!(EVENTS.with(|v| v.borrow().contains(&"fallback_assign")));
    }
}

#[test]
fn failed_factory_result_blocks_retries_and_never_deletes_populated_squads() {
    unsafe {
        for populated in [false, true] {
            let (p, _) = fixture();
            MOCK_SPAWN_RESULT.with(|v| v.set(if populated { p.sources[0].entity } else { 0 }));
            assert_eq!(regroup(0, p.clone()), 0);
            assert!(CREATION_BLOCKED.with(Cell::get));
            assert_eq!(regroup(0, p.clone()), 0);
            // No second factory call and no cleanup of a populated result.
            assert_eq!(EVENTS.with(|v| v.borrow().clone()), vec!["spawn"]);
            for src in &p.sources {
                assert_eq!(
                    pointers(src.holder, 0xa0, 64).unwrap().len(),
                    src.members.len()
                );
            }
        }
        CREATION_BLOCKED.with(|v| v.set(false));
        MOCK_SPAWN_RESULT.with(|v| v.set(0));
    }
}

#[test]
fn unclaimed_constructor_consumes_names_without_spawning_or_moving_soldiers() {
    unsafe {
        let (p, dest) = fixture();
        CONTEXT.with(|v| *v.borrow_mut() = Some(p.clone()));
        // No template claim, so the stock soldier factory must not run.
        create_members(dest.holder, 0);
        assert!(CREATION_ABORTED.with(Cell::get));
        assert!(!CONSTRUCTED.with(Cell::get));
        assert_eq!(EVENTS.with(|v| v.borrow().clone()), vec!["free_strings"]);
        for src in &p.sources {
            assert_eq!(
                pointers(src.holder, 0xa0, 64).unwrap().len(),
                src.members.len()
            );
        }
        // A later matching hook cannot revive this failed transaction.
        create_members(dest.holder, 0);
        assert_eq!(PREPARED_HOLDER.with(Cell::get), 0);
        assert_eq!(
            EVENTS.with(|v| v.borrow().clone()),
            vec!["free_strings", "free_strings"]
        );
        CONTEXT.with(|v| *v.borrow_mut() = None);
        CREATION_BLOCKED.with(|v| v.set(true));
        assert_eq!(regroup(0, p), 0); // SPAWN is deliberately unset in this fixture.
        CREATION_BLOCKED.with(|v| v.set(false));
    }
}

#[test]
fn unexpected_default_loadout_rejects_before_any_source_mutation() {
    unsafe {
        let (p, dest) = fixture();
        set(dest.pool, 0x28, q(dest.pool, 0x20) + 0x48);
        assert!(populate(dest.holder, &p).is_err());
        for src in &p.sources {
            assert_eq!(
                pointers(src.holder, 0xa0, 64).unwrap().len(),
                src.members.len()
            );
        }
        assert!(EVENTS.with(|v| v.borrow().is_empty()));
    }
}
#[test]
fn changed_source_ammunition_rejects_without_detachment() {
    unsafe {
        let (p, dest) = fixture();
        let src = &p.sources[0];
        put(q(src.pool, 0x20), 0x2c, 79);
        assert!(populate(dest.holder, &p).is_err());
        assert!(!EVENTS.with(|v| v.borrow().contains(&"remove")));
        assert_eq!(pointers(src.holder, 0xa0, 64).unwrap().len(), 3);
    }
}
