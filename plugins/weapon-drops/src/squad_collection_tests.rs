use super::*;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::sync::Mutex;

const HUMAN_VTABLE: usize = 0x1a11_0000;
const PICKUP: usize = 0x9000;
const RIFLE_AMMO: usize = 0xaa10;
const SECOND_AMMO: usize = 0xbb20;
const ALTERNATE_AMMO: usize = 0xcc30;
const OLD_AMMO: usize = 0xdd40;

#[derive(Debug, PartialEq)]
enum NativeEvent {
    Collect,
    StockDrop,
    StockAdd,
    Detach(usize, usize, usize),
    Dispose,
    AmmoRemove(Vec<[usize; 2]>),
    PrimaryAdd(usize, Vec<[usize; 2]>),
    Switch(usize, usize),
    Sync(usize),
    Spawn(usize, [usize; 3]),
}

thread_local! {
    static ACTIVE: Cell<*mut FakeGame> = const { Cell::new(std::ptr::null_mut()) };
    static NATIVE_EVENTS: RefCell<Vec<NativeEvent>> = const { RefCell::new(Vec::new()) };
}

static GAME_LOCK: Mutex<()> = Mutex::new(());

struct Region(Box<[u64]>);

impl Region {
    fn new(bytes: usize) -> Self {
        Self(vec![0; bytes.div_ceil(8).max(1)].into_boxed_slice())
    }

    fn ptr(&self) -> usize {
        self.0.as_ptr() as usize
    }
}

#[derive(Clone)]
struct MemberRefs {
    entity: usize,
    gunner: usize,
    items: usize,
    guns: usize,
    item_count: usize,
    gun_count: usize,
    incoming_gun: usize,
}

struct FakeGame {
    regions: Vec<Region>,
    manager: usize,
    collector: usize,
    squad: usize,
    holder: usize,
    squad_script: usize,
    old_item: usize,
    incoming: usize,
    old_config: usize,
    incoming_config: usize,
    pistol_config: usize,
    cache_row: usize,
    members: Vec<MemberRefs>,
    nested_once: bool,
    ammo_pool: BTreeMap<usize, (u32, u32, u32)>,
    pool: usize,
    spawned: Vec<(usize, ammo::Rows)>,
}

impl FakeGame {
    fn new(incoming: Option<usize>) -> Self {
        let mut game = Self {
            regions: Vec::new(),
            manager: 0,
            collector: 0,
            squad: 0,
            holder: 0,
            squad_script: 0,
            old_item: 0,
            incoming: 0,
            old_config: 0,
            incoming_config: 0,
            pistol_config: 0,
            cache_row: 0,
            members: Vec::new(),
            nested_once: false,
            ammo_pool: BTreeMap::new(),
            pool: 0,
            spawned: Vec::new(),
        };
        unsafe { game.initialize(incoming) };
        game
    }

    fn alloc(&mut self, bytes: usize) -> usize {
        self.regions.push(Region::new(bytes));
        self.regions.last().unwrap().ptr()
    }

    unsafe fn ammo_info(&mut self, name: &str) -> usize {
        let info = self.alloc(0x40);
        Region::string_at(info + 8, name);
        info
    }

    unsafe fn sync_pool(&mut self) {
        let rows: Vec<_> = self
            .ammo_pool
            .iter()
            .map(|(&identity, &supply)| (identity, supply))
            .collect();
        let begin = self.alloc(rows.len() * 0x48);
        for (index, (identity, supply)) in rows.iter().enumerate() {
            let row = begin + index * 0x48;
            Self::put(row, 0, *identity);
            Self::put_u32(row, 0x28, supply.0);
            Self::put_u32(row, 0x2c, supply.1);
            Self::put_u32(row, 0x34, supply.2);
        }
        let end = begin + rows.len() * 0x48;
        Self::put(self.pool, 0x20, begin);
        Self::put(self.pool, 0x28, end);
        Self::put(self.pool, 0x30, end);
    }

    unsafe fn put(address: usize, offset: usize, value: usize) {
        *((address + offset) as *mut usize) = value;
    }

    unsafe fn put_u32(address: usize, offset: usize, value: u32) {
        *((address + offset) as *mut u32) = value;
    }

    unsafe fn put_i32(address: usize, offset: usize, value: i32) {
        *((address + offset) as *mut i32) = value;
    }

    unsafe fn setup_item(&mut self, category: &str, config: usize) -> usize {
        let item = self.alloc(0x180);
        Self::put_u32(item, 0x2c, 1);
        let override_info = self.alloc(0xc0);
        Self::put(override_info, 0xb8, config);
        Self::put(item, 0x140, override_info);
        let type_storage = self.alloc(0x20);
        Region::string_at(type_storage, category);
        Self::put(item, 0x30, type_storage);
        Self::put(item, 0x38, type_storage + 0x20);
        Self::put(item, 0x40, type_storage + 0x20);
        item
    }

    unsafe fn setup_config(&mut self, grenade: bool) -> usize {
        let config = self.alloc(0x250);
        Region::string_at(config + 0x1a8, "rifle_mesh");
        Self::put_u32(config, 0x108, 0);
        *((config + 0xa9) as *mut u8) = u8::from(grenade);
        let ammo = self.alloc(0x20);
        Self::put(ammo, 0, RIFLE_AMMO);
        Self::put_u32(ammo, 8, 120);
        Self::put(ammo, 0x10, SECOND_AMMO);
        Self::put_u32(ammo, 0x18, 80);
        for (offset, value) in [(0x210, ammo), (0x218, ammo + 0x20), (0x220, ammo + 0x20)] {
            Self::put(config, offset, value);
        }
        let alternate = self.alloc(0x10);
        Self::put(alternate, 0, ALTERNATE_AMMO);
        Self::put_u32(alternate, 8, 60);
        for (offset, value) in [
            (0x228, alternate),
            (0x230, alternate + 0x10),
            (0x238, alternate + 0x10),
        ] {
            Self::put(config, offset, value);
        }
        config
    }

    unsafe fn initialize(&mut self, incoming: Option<usize>) {
        self.old_config = self.setup_config(false);
        for offset in [0x210, 0x228] {
            let ammo = word(self.old_config, offset);
            Self::put(ammo, 0, OLD_AMMO);
            Self::put_u32(ammo, 8, 100);
            Self::put(self.old_config, offset + 8, ammo + 0x10);
        }
        self.ammo_pool.insert(OLD_AMMO, (200, 100, 2));
        let pool_vtable = self.alloc(0x48);
        Self::put(pool_vtable, 0x40, ammo_remove as *const () as usize);
        self.pool = self.alloc(0x50);
        Self::put(self.pool, 0, pool_vtable);
        *((self.pool + 0x48) as *mut f32) = 1.0;
        let record = self.alloc(0x48);
        Self::put(record, 0, OLD_AMMO);
        Self::put_u32(record, 0x28, 200);
        Self::put_u32(record, 0x2c, 100);
        Self::put_u32(record, 0x34, 2);
        Self::put(self.pool, 0x20, record);
        Self::put(self.pool, 0x28, record + 0x48);
        Self::put(self.pool, 0x30, record + 0x48);
        let provider = self.alloc(0x20);
        Self::put(provider, 0x10, self.pool);
        self.incoming_config = self.setup_config(false);
        self.old_item = self.setup_item("rifles", self.old_config);
        self.incoming =
            incoming.unwrap_or_else(|| unsafe { self.setup_item("rifles", self.incoming_config) });
        let grenade_config = self.setup_config(true);
        self.pistol_config = self.setup_config(false);
        let grenade_item = self.setup_item("inf_hg", grenade_config);
        let pistol_item = self.setup_item("pistols", self.pistol_config);

        let slot = self.alloc(0x90);
        Region::string_at(slot + 0x28, "rifles");
        Self::put(slot, 0x68, self.old_item);
        let slot_ptrs = self.alloc(8);
        Self::put(slot_ptrs, 0, slot);
        self.squad_script = self.alloc(0x260);
        Self::put(self.squad_script, 0x228, slot_ptrs);
        Self::put(self.squad_script, 0x230, slot_ptrs + 8);
        Self::put(self.squad_script, 0x238, slot_ptrs + 8);

        self.holder = self.alloc(0x180);
        Self::put(self.holder, 0xc0, self.squad_script);
        self.squad = self.alloc(0x20);
        Self::put(self.squad, 8, self.holder);
        let cache = self.alloc(0x10);
        self.cache_row = cache;
        Self::put(cache, 0, slot);
        Self::put(cache, 8, self.old_item);
        Self::put(self.holder, 0x138, cache);
        Self::put(self.holder, 0x140, cache + 0x10);
        Self::put(self.holder, 0x148, cache + 0x10);

        let entity_vtable = self.alloc(0xc0);
        Self::put(entity_vtable, 0xb0, entity_facets as *const () as usize);
        let context_vtable = self.alloc(0x60);
        Self::put(context_vtable, 0x50, context_script as *const () as usize);
        let position_vtable = self.alloc(0x68);
        Self::put(
            position_vtable,
            0x58,
            transform_position as *const () as usize,
        );
        Self::put(
            position_vtable,
            0x60,
            transform_rotation as *const () as usize,
        );

        let mut roster_members = Vec::new();
        for index in 0..2 {
            let items = self.alloc(8 * 8);
            Self::put(items, 0, self.old_item);
            Self::put(items, 8, grenade_item);
            Self::put(items, 16, pistol_item);
            let old_gun = self.alloc(0x80);
            Self::put(old_gun, 0x40, self.old_config);
            Self::put(old_gun, 0x58, provider);
            let grenade_gun = self.alloc(0x80);
            Self::put(grenade_gun, 0x40, grenade_config);
            let pistol_gun = self.alloc(0x80);
            Self::put(pistol_gun, 0x40, self.pistol_config);
            let incoming_gun = self.alloc(0x80);
            Self::put(incoming_gun, 0x40, self.incoming_config);
            Self::put(incoming_gun, 0x58, provider);
            let guns = self.alloc(8 * 8);
            Self::put(guns, 0, old_gun);
            Self::put(guns, 8, grenade_gun);
            Self::put(guns, 16, pistol_gun);

            let gunner = self.alloc(0x120);
            Self::put(gunner, 0, HUMAN_VTABLE);
            Self::put(gunner, 0x38, guns);
            Self::put(gunner, 0x40, guns + 3 * 8);
            Self::put(gunner, 0x48, guns + 8 * 8);
            Self::put(gunner, 0x50, items);
            Self::put(gunner, 0x58, items + 3 * 8);
            Self::put(gunner, 0x60, items + 8 * 8);
            Self::put(gunner, 0xf8, 0); // set after context creation
            Self::put_i32(gunner, 0x94, 0);

            let context = self.alloc(0x20);
            Self::put(context, 0, context_vtable);
            Self::put(context, 8, self.squad_script);
            Self::put(gunner, 0xf8, context);

            let entity = self.alloc(0x40);
            Self::put(entity, 0, entity_vtable);
            let facets = self.alloc(0x40);
            Self::put(entity, 8, facets);
            let ai = self.alloc(0x210);
            Self::put(ai, 0x148, provider);
            let junction = self.alloc(0x20);
            Self::put(ai, 0x1f0, junction);
            Self::put(junction, 0x10, gunner);
            Self::put(facets, 0x28, ai);
            let damageable = self.alloc(0x40);
            *((damageable + 0x18) as *mut u8) = 1;
            Self::put(facets, 0x18, damageable);
            let visual = self.alloc(0x130);
            let visual_rows = self.alloc(0xb0);
            Self::put(visual_rows, 0, self.old_config);
            Self::put(visual, 0x108, visual_rows);
            Self::put(visual, 0x110, visual_rows + 0xb0);
            Self::put(visual, 0x118, visual_rows + 0xb0);
            Self::put(facets, 8, visual);

            let position = self.alloc(0x20);
            let rotation = self.alloc(0x20);
            *((position) as *mut [f32; 3]) = [index as f32, 2.0, 3.0];
            *((rotation) as *mut [f32; 4]) = [0.0, 0.0, 0.0, 1.0];
            let position_facet = self.alloc(0x20);
            Self::put(position_facet, 0, position_vtable);
            Self::put(position_facet, 8, position);
            Self::put(position_facet, 0x10, rotation);
            Self::put(facets, 0, position_facet);

            Self::put(entity, 0x38, self.squad);
            Self::put(gunner, 0x20, entity);
            roster_members.push(entity);
            self.members.push(MemberRefs {
                entity,
                gunner,
                items,
                guns,
                item_count: 3,
                gun_count: 3,
                incoming_gun,
            });
        }
        self.collector = roster_members[0];
        let roster = self.alloc(8 * 8);
        Self::put(roster, 0, roster_members[0]);
        Self::put(roster, 8, roster_members[1]);
        Self::put(self.holder, 0xa0, roster);
        Self::put(self.holder, 0xa8, roster + 2 * 8);
        Self::put(self.holder, 0xb0, roster + 8 * 8);

        let record = self.alloc(0x90);
        Self::put(record, 0x10, PICKUP);
        Self::put(record, 0x18, self.incoming);
        Self::put(record, 0x20, 0);
        Self::put(record, 0x28, 0);
        let records = self.alloc(0x90);
        core::ptr::copy_nonoverlapping(record as *const u8, records as *mut u8, 0x90);
        self.manager = self.alloc(0x40);
        Self::put(self.manager, 0x20, records);
        Self::put(self.manager, 0x28, records + 0x90);
        Self::put(self.manager, 0x30, records + 0x90);
    }

    fn member(&self, gunner: usize) -> usize {
        self.members
            .iter()
            .position(|member| member.gunner == gunner)
            .unwrap()
    }

    unsafe fn set_ground_ammo(&mut self, rows: &[[usize; 2]]) {
        let begin = self.alloc(rows.len() * 0x10);
        for (index, row) in rows.iter().enumerate() {
            *((begin + index * 0x10) as *mut [usize; 2]) = *row;
        }
        let record = word(self.manager, 0x20);
        let end = begin + rows.len() * 0x10;
        for (offset, value) in [(0x20, begin), (0x28, end), (0x30, end)] {
            Self::put(record, offset, value);
        }
    }

    unsafe fn same_primary_pickup(&mut self) {
        self.incoming = self.old_item;
        self.incoming_config = self.old_config;
        Self::put(word(self.manager, 0x20), 0x18, self.old_item);
    }

    unsafe fn remove_member_entry(&mut self, index: usize, item: usize, gun: usize) {
        let member = &mut self.members[index];
        let selected = *((member.gunner + 0x94) as *const i32);
        if selected >= 0 && word(member.guns, selected as usize * 8) == gun {
            Self::put_i32(member.gunner, 0x94, -1);
        }
        Self::remove_from_vector(member.items, &mut member.item_count, item);
        Self::put(member.gunner, 0x58, member.items + member.item_count * 8);
        let removed = (0..member.gun_count)
            .find(|index| word(member.guns, index * 8) == gun)
            .unwrap();
        let last = word(member.guns, (member.gun_count - 1) * 8);
        Self::put(member.guns, removed * 8, last);
        member.gun_count -= 1;
        Self::put(member.gunner, 0x40, member.guns + member.gun_count * 8);
    }

    unsafe fn remove_from_vector(storage: usize, count: &mut usize, sought: usize) {
        let found = (0..*count)
            .find(|i| *((storage + i * 8) as *const usize) == sought)
            .unwrap();
        for i in found..(*count - 1) {
            let value = *((storage + (i + 1) * 8) as *const usize);
            *((storage + i * 8) as *mut usize) = value;
        }
        *count -= 1;
    }

    unsafe fn add_member_entry(&mut self, index: usize, item: usize, config: usize) {
        let member = &mut self.members[index];
        let slot = member.item_count;
        Self::put(member.items, slot * 8, item);
        member.item_count += 1;
        Self::put(member.gunner, 0x58, member.items + member.item_count * 8);
        let gun_slot = member.gun_count;
        Self::put(member.guns, gun_slot * 8, member.incoming_gun);
        member.gun_count += 1;
        Self::put(member.gunner, 0x40, member.guns + member.gun_count * 8);
        Self::put(member.incoming_gun, 0x40, config);
        // A different first geometry row makes native standard addition leave
        // selection unchanged; the replacement handler must select explicitly.
        let visual = word(word(member.entity, 8), 8);
        let rows = word(visual, 0x108);
        if word(rows, 0) == config {
            let first = word(member.guns, 0);
            Self::put(member.guns, 0, member.incoming_gun);
            Self::put(member.guns, gun_slot * 8, first);
            Self::put_i32(member.gunner, 0x94, 0);
        }
    }
}

impl Drop for FakeGame {
    fn drop(&mut self) {
        reserve::forget(self.pool);
    }
}

impl Region {
    unsafe fn string_at(address: usize, text: &str) {
        assert!(text.len() <= 15);
        core::ptr::copy_nonoverlapping(text.as_ptr(), address as *mut u8, text.len());
        *((address + 0x10) as *mut usize) = text.len();
        *((address + 0x18) as *mut usize) = 15;
    }
}

unsafe fn active_game() -> &'static mut FakeGame {
    ACTIVE.with(|active| &mut *active.get())
}

unsafe extern "C" fn entity_facets(entity: usize) -> usize {
    *((entity + 8) as *const usize)
}

unsafe extern "C" fn context_script(context: usize) -> usize {
    *((context + 8) as *const usize)
}

unsafe extern "C" fn transform_position(facet: usize) -> usize {
    *((facet + 8) as *const usize)
}

unsafe extern "C" fn transform_rotation(facet: usize) -> usize {
    *((facet + 0x10) as *const usize)
}

unsafe extern "C" fn canonical_squad(entity: usize) -> usize {
    *((entity + 0x38) as *const usize)
}

unsafe extern "C" fn holder_for(squad: usize) -> usize {
    *((squad + 8) as *const usize)
}

unsafe extern "C" fn member_context(gunner: usize) -> usize {
    *((gunner + 0xf8) as *const usize)
}

unsafe extern "C" fn resolve(item: usize, script_key: usize) -> usize {
    let script = active_game().squad_script;
    assert_eq!(
        script_key,
        script + 8,
        "override resolver must receive ScriptInfo + 8"
    );
    *((item + 0x140) as *const usize)
}

unsafe extern "C" fn slot_type(gun: usize, _context: usize) -> i32 {
    *((gun + 0x108) as *const i32)
}

unsafe extern "C" fn ammo_mode(_entity: usize) -> u8 {
    *((active_game().squad_script + 0x1be) as *const u8)
}

unsafe extern "C" fn original_collect(manager: usize, _pickup: usize, collector: usize) {
    NATIVE_EVENTS.with(|events| events.borrow_mut().push(NativeEvent::Collect));
    let facets = *((collector + 8) as *const usize);
    let ai = *((facets + 0x28) as *const usize);
    let gunner = *((ai + 0x1f0) as *const usize);
    let gunner = *((gunner + 0x10) as *const usize);
    drop_special(ai, 0, 0, 0);
    let nested = {
        let game = active_game();
        let nested = game.nested_once;
        game.nested_once = false;
        nested
    };
    if nested {
        collect(manager, PICKUP + 1, collector);
    }
    add(gunner, active_game().incoming, &[0; 3]);
}

unsafe extern "C" fn original_drop(_ai: usize, _target: usize, _owner: usize, _flag: u8) {
    NATIVE_EVENTS.with(|events| events.borrow_mut().push(NativeEvent::StockDrop));
}

unsafe extern "C" fn original_add(_gunner: usize, _item: usize, _ammo: *const [usize; 3]) {
    NATIVE_EVENTS.with(|events| events.borrow_mut().push(NativeEvent::StockAdd));
}

unsafe extern "C" fn detach(gunner: usize, output: *mut [usize; 4]) -> usize {
    let item = *((gunner + 0x30) as *const usize);
    let gun = *((gunner + 0x28) as *const usize);
    let cc = *((gunner + 0xcc) as *const i32);
    NATIVE_EVENTS.with(|events| {
        events
            .borrow_mut()
            .push(NativeEvent::Detach(gunner, item, gun))
    });
    let game = active_game();
    let index = game.member(gunner);
    let config = word(gun, 0x40);
    let offset = if *((game.squad_script + 0x1be) as *const u8) == 0 {
        0x210
    } else {
        0x228
    };
    let rows: Vec<_> = (word(config, offset)..word(config, offset + 8))
        .step_by(0x10)
        .filter_map(|row| {
            let identity = word(row, 0);
            game.ammo_pool
                .get(&identity)
                .map(|supply| [identity, (supply.1 / supply.2) as usize])
        })
        .collect();
    let begin = game.alloc(rows.len() * 0x10);
    for (index, row) in rows.iter().enumerate() {
        *((begin + index * 0x10) as *mut [usize; 2]) = *row;
    }
    let end = begin + rows.len() * 0x10;
    game.remove_member_entry(index, item, gun);
    *((gunner + 0x28) as *mut usize) = 0;
    *((gunner + 0x30) as *mut usize) = 0;
    *((gunner + 0xcc) as *mut i32) = 0;
    *output = [item, begin, end, end];
    cc as usize
}

unsafe extern "C" fn ammo_remove(pool: usize, vector: *const usize) {
    let begin = *vector;
    let end = *vector.add(1);
    let rows =
        std::slice::from_raw_parts(begin as *const [usize; 2], (end - begin) / 0x10).to_vec();
    NATIVE_EVENTS.with(|events| {
        events
            .borrow_mut()
            .push(NativeEvent::AmmoRemove(rows.clone()))
    });
    let game = active_game();
    assert_eq!(pool, game.pool);
    for row in rows {
        let supply = game.ammo_pool.get_mut(&row[0]).unwrap();
        let remaining = supply.2 - 1;
        supply.0 = remaining * supply.0 / supply.2;
        supply.1 = supply.1.saturating_sub(row[1] as u32).min(supply.0);
        supply.2 = remaining;
        if remaining == 0 {
            game.ammo_pool.remove(&row[0]);
        }
    }
    game.sync_pool();
}

unsafe extern "C" fn dispose(_output: usize) {
    NATIVE_EVENTS.with(|events| events.borrow_mut().push(NativeEvent::Dispose));
}

unsafe extern "C" fn primary_add(gunner: usize, item: usize, ammo: *const [usize; 3]) {
    let ammo = *ammo;
    let rows = if ammo[0] == ammo[1] {
        Vec::new()
    } else {
        std::slice::from_raw_parts(ammo[0] as *const [usize; 2], (ammo[1] - ammo[0]) / 0x10)
            .to_vec()
    };
    NATIVE_EVENTS.with(|events| {
        events
            .borrow_mut()
            .push(NativeEvent::PrimaryAdd(item, rows.clone()))
    });
    let game = active_game();
    let index = game.member(gunner);
    let config = game.incoming_config;
    // Model the native matching-row branch: an empty transfer vector skips
    // capacity registration, while matching rows add capacity and their rounds.
    let offset = if *((game.squad_script + 0x1be) as *const u8) == 0 {
        0x210
    } else {
        0x228
    };
    for row in (word(config, offset)..word(config, offset + 8)).step_by(0x10) {
        let identity = word(row, 0);
        if let Some(transfer) = rows.iter().find(|transfer| transfer[0] == identity) {
            let supply = game.ammo_pool.entry(identity).or_default();
            supply.0 += *((row + 8) as *const u32);
            supply.1 += transfer[1] as u32;
            supply.2 += 1;
        }
    }
    game.sync_pool();
    game.add_member_entry(index, item, config);
}

unsafe extern "C" fn switch(visual: usize, config: usize) {
    NATIVE_EVENTS.with(|events| {
        events
            .borrow_mut()
            .push(NativeEvent::Switch(visual, config))
    });
}

unsafe extern "C" fn visual_sync(gunner: usize) {
    NATIVE_EVENTS.with(|events| events.borrow_mut().push(NativeEvent::Sync(gunner)));
}

unsafe extern "C" fn spawn(
    _manager: usize,
    _source: usize,
    _position: *const [f32; 3],
    _rotation: *const [f32; 4],
    item: usize,
    ammo: *const [usize; 3],
    _follow: bool,
    _id: i32,
) {
    NATIVE_EVENTS.with(|events| events.borrow_mut().push(NativeEvent::Spawn(item, *ammo)));
    let bounds = *ammo;
    let rows = if bounds[0] == bounds[1] {
        Vec::new()
    } else {
        std::slice::from_raw_parts(
            bounds[0] as *const [usize; 2],
            (bounds[1] - bounds[0]) / 0x10,
        )
        .to_vec()
    };
    active_game().spawned.push((item, rows));
}

unsafe extern "C" fn import_rounds(pool: usize, identity: usize, requested: u32) -> u32 {
    let game = active_game();
    assert_eq!(pool, game.pool);
    let supply = game.ammo_pool.get_mut(&identity).unwrap();
    let accepted = requested.min(supply.0 - supply.1);
    supply.1 += accepted;
    game.sync_pool();
    accepted
}

fn install_fake_bindings() {
    ammo::configure(import_rounds as *const () as usize, 0);
    configure(Bindings {
        collect_original: original_collect as *const () as usize,
        drop_original: original_drop as *const () as usize,
        add_original: original_add as *const () as usize,
        canonical_squad: canonical_squad as *const () as usize,
        holder: holder_for as *const () as usize,
        context: member_context as *const () as usize,
        resolve: resolve as *const () as usize,
        slot: slot_type as *const () as usize,
        human_vtable: HUMAN_VTABLE,
        detach: detach as *const () as usize,
        dispose: dispose as *const () as usize,
        primary_add: primary_add as *const () as usize,
        ammo_mode: ammo_mode as *const () as usize,
        ammo_remove: ammo_remove as *const () as usize,
        switch: switch as *const () as usize,
        visual_sync: visual_sync as *const () as usize,
        spawn: spawn as *const () as usize,
        ..Default::default()
    });
}

#[test]
fn primary_selection_is_valid_after_empty_native_add_in_active_firing_state() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for (selected_before, state_before) in [(0, 2), (-1, 0)] {
        let mut game = FakeGame::new(None);
        for member in &game.members {
            unsafe {
                FakeGame::put_i32(member.gunner, 0x90, state_before);
                FakeGame::put_i32(member.gunner, 0x94, selected_before);
            }
        }
        ACTIVE.with(|active| active.set(&mut game));
        NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
        install_fake_bindings();
        unsafe { collect(game.manager, PICKUP, game.collector) };
        for member in &game.members {
            let selected = unsafe { *((member.gunner + 0x94) as *const i32) };
            assert!(selected >= 0 && (selected as usize) < member.gun_count);
            assert_eq!(
                unsafe { word(member.guns, selected as usize * 8) },
                member.incoming_gun
            );
            assert_eq!(
                unsafe { *((member.gunner + 0x90) as *const i32) },
                state_before
            );
            assert!(NATIVE_EVENTS
                .with(|events| events.borrow().contains(&NativeEvent::Sync(member.gunner))));
        }
    }
}

#[test]
fn selected_auxiliary_gun_is_remapped_after_native_swap_removal() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut game = FakeGame::new(None);
    let selected_before: Vec<_> = game
        .members
        .iter()
        .map(|member| unsafe { word(member.guns, 16) })
        .collect();
    for member in &game.members {
        unsafe { FakeGame::put_i32(member.gunner, 0x94, 2) };
    }
    ACTIVE.with(|active| active.set(&mut game));
    NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
    install_fake_bindings();
    unsafe { collect(game.manager, PICKUP, game.collector) };
    for (member, previous) in game.members.iter().zip(selected_before) {
        let selected = unsafe { *((member.gunner + 0x94) as *const i32) };
        assert!(selected >= 0 && (selected as usize) < member.gun_count);
        assert_eq!(
            unsafe { word(member.guns, selected as usize * 8) },
            previous
        );
        assert_eq!(unsafe { word(member.gunner, 0x28) }, 0);
        let visual = unsafe { word(word(member.entity, 8), 8) };
        assert!(NATIVE_EVENTS.with(|events| events
            .borrow()
            .contains(&NativeEvent::Switch(visual, game.pistol_config))));
    }
}

#[test]
fn collection_swaps_all_members_and_emits_one_ammo_free_old_pickup() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut game = FakeGame::new(None);
    ACTIVE.with(|active| active.set(&mut game));
    NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
    install_fake_bindings();

    unsafe { collect(game.manager, PICKUP, game.collector) };

    let events = NATIVE_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, NativeEvent::Collect))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, NativeEvent::Detach(..)))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, NativeEvent::Dispose))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, NativeEvent::PrimaryAdd(_, ammo) if *ammo == [[RIFLE_AMMO, 0], [SECOND_AMMO, 0]]))
            .count(),
        2
    );
    assert_eq!(game.ammo_pool.get(&RIFLE_AMMO), Some(&(240, 0, 2)));
    assert_eq!(game.ammo_pool.get(&SECOND_AMMO), Some(&(160, 0, 2)));
    assert!(!game.ammo_pool.contains_key(&OLD_AMMO));
    assert_eq!(
        events
            .iter()
            .filter(
                |event| matches!(event, NativeEvent::Spawn(item, ammo) if *item == game.old_item && *ammo == [0; 3])
            )
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|event| matches!(event, NativeEvent::StockDrop | NativeEvent::StockAdd)));
    for member in &game.members {
        assert_eq!(
            unsafe { *((member.gunner + 0x58) as *const usize) - member.items },
            3 * 8
        );
        assert!(!(0..member.item_count).any(|index| unsafe {
            *((member.items + index * 8) as *const usize) == game.old_item
        }));
        assert!((0..member.item_count).any(|index| unsafe {
            *((member.items + index * 8) as *const usize) == game.incoming
        }));
        assert_eq!(
            unsafe { *((member.gunner + 0x40) as *const usize) - member.guns },
            3 * 8
        );
    }
    assert_eq!(
        unsafe { *((game.cache_row + 8) as *const usize) },
        game.incoming
    );
}

#[test]
fn primary_registration_preserves_existing_ammo_and_selects_the_squad_ammo_mode() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for alternate in [false, true] {
        let mut game = FakeGame::new(None);
        game.ammo_pool.insert(RIFLE_AMMO, (300, 170, 1));
        game.ammo_pool.insert(ALTERNATE_AMMO, (90, 40, 1));
        unsafe { *((game.squad_script + 0x1be) as *mut u8) = u8::from(alternate) };
        ACTIVE.with(|active| active.set(&mut game));
        NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
        install_fake_bindings();
        unsafe { collect(game.manager, PICKUP, game.collector) };
        if alternate {
            assert_eq!(game.ammo_pool.get(&ALTERNATE_AMMO), Some(&(210, 40, 3)));
            assert_eq!(game.ammo_pool.get(&RIFLE_AMMO), Some(&(300, 170, 1)));
            assert!(!game.ammo_pool.contains_key(&SECOND_AMMO));
        } else {
            assert_eq!(game.ammo_pool.get(&RIFLE_AMMO), Some(&(540, 170, 3)));
            assert_eq!(game.ammo_pool.get(&SECOND_AMMO), Some(&(160, 0, 2)));
            assert_eq!(game.ammo_pool.get(&ALTERNATE_AMMO), Some(&(90, 40, 1)));
        }
    }
}

#[test]
fn repeated_primary_swaps_do_not_accumulate_ammo_capacity_or_carriers() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut game = FakeGame::new(None);
    let rifle_item = game.incoming;
    let rifle_config = game.incoming_config;
    ACTIVE.with(|active| active.set(&mut game));
    NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
    install_fake_bindings();
    for (item, config, expected) in [
        (rifle_item, rifle_config, RIFLE_AMMO),
        (game.old_item, game.old_config, OLD_AMMO),
        (rifle_item, rifle_config, RIFLE_AMMO),
    ] {
        game.incoming = item;
        game.incoming_config = config;
        unsafe {
            FakeGame::put(word(game.manager, 0x20), 0x18, item);
            collect(game.manager, PICKUP, game.collector);
        }
        if expected == OLD_AMMO {
            assert_eq!(game.ammo_pool.len(), 1);
            assert_eq!(game.ammo_pool.get(&OLD_AMMO), Some(&(200, 0, 2)));
        } else {
            assert_eq!(game.ammo_pool.len(), 2);
            assert_eq!(game.ammo_pool.get(&RIFLE_AMMO), Some(&(240, 0, 2)));
            assert_eq!(game.ammo_pool.get(&SECOND_AMMO), Some(&(160, 0, 2)));
        }
    }
}

#[test]
fn outgoing_shared_ammo_is_removed_before_any_replacement_registers() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut game = FakeGame::new(None);
    unsafe {
        let ammo = word(game.old_config, 0x210);
        FakeGame::put(ammo, 0, RIFLE_AMMO);
        game.ammo_pool.clear();
        game.ammo_pool.insert(RIFLE_AMMO, (200, 100, 2));
        game.sync_pool();
    }
    ACTIVE.with(|active| active.set(&mut game));
    NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
    install_fake_bindings();
    unsafe { collect(game.manager, PICKUP, game.collector) };
    assert_eq!(game.ammo_pool.get(&RIFLE_AMMO), Some(&(240, 0, 2)));
    let events = NATIVE_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
    let last_remove = events
        .iter()
        .rposition(|event| matches!(event, NativeEvent::AmmoRemove(_)))
        .unwrap();
    let first_add = events
        .iter()
        .position(|event| matches!(event, NativeEvent::PrimaryAdd(..)))
        .unwrap();
    assert!(last_remove < first_add);
}

#[test]
fn zero_carrier_outgoing_ammo_refuses_before_native_division() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut game = FakeGame::new(None);
    unsafe {
        let record = word(game.pool, 0x20);
        FakeGame::put_u32(record, 0x34, 0);
    }
    ACTIVE.with(|active| active.set(&mut game));
    NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
    install_fake_bindings();
    unsafe { collect(game.manager, PICKUP, game.collector) };
    assert!(NATIVE_EVENTS.with(|events| events.borrow().is_empty()));
    assert_eq!(game.ammo_pool.get(&OLD_AMMO), Some(&(200, 100, 2)));
}

#[test]
fn malformed_primary_ammo_types_refuse_before_stock_collection_or_detachment() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for invalid in 0..4 {
        let mut game = FakeGame::new(None);
        unsafe {
            let begin = word(game.incoming_config, 0x210);
            match invalid {
                0 => FakeGame::put(game.incoming_config, 0x218, begin),
                1 => FakeGame::put(begin, 0, 0),
                2 => FakeGame::put(begin, 0x10, RIFLE_AMMO),
                _ => FakeGame::put(game.incoming_config, 0x218, begin + 8),
            }
        }
        ACTIVE.with(|active| active.set(&mut game));
        NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
        install_fake_bindings();
        unsafe { collect(game.manager, PICKUP, game.collector) };
        assert!(NATIVE_EVENTS.with(|events| events.borrow().is_empty()));
        assert_eq!(game.ammo_pool.len(), 1);
        assert_eq!(game.ammo_pool.get(&OLD_AMMO), Some(&(200, 100, 2)));
    }
}

#[test]
fn identical_empty_primary_exchanges_and_drops_the_outgoing_ammo() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _policy = policy::set_for_test(policy::parse_settings("transfer", "discard").unwrap());
    let mut game = FakeGame::new(Some(0));
    unsafe {
        game.same_primary_pickup();
    }
    ACTIVE.with(|active| active.set(&mut game));
    NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
    install_fake_bindings();

    unsafe { collect(game.manager, PICKUP, game.collector) };

    let events = NATIVE_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, NativeEvent::Detach(..)))
            .count(),
        2
    );
    assert_eq!(game.ammo_pool.get(&OLD_AMMO), Some(&(200, 0, 2)));
    assert_eq!(game.spawned, [(game.old_item, vec![[OLD_AMMO, 100]])]);
    for member in &game.members {
        assert_eq!(unsafe { *((member.gunner + 0x94) as *const i32) }, 0);
        assert_eq!(
            unsafe { *((member.gunner + 0x58) as *const usize) - member.items },
            3 * 8
        );
        assert_eq!(
            unsafe { *((member.gunner + 0x40) as *const usize) - member.guns },
            3 * 8
        );
    }
}

#[test]
fn transfer_swaps_a_squad_once_and_preserves_excess_pickup_rounds() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _policy = policy::set_for_test(policy::parse_settings("transfer", "discard").unwrap());
    let mut game = FakeGame::new(None);
    unsafe { game.set_ground_ammo(&[[RIFLE_AMMO, 350], [SECOND_AMMO, 40]]) };
    ACTIVE.with(|active| active.set(&mut game));
    NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
    install_fake_bindings();
    unsafe { collect(game.manager, PICKUP, game.collector) };
    assert_eq!(game.ammo_pool.get(&RIFLE_AMMO), Some(&(240, 240, 2)));
    assert_eq!(game.ammo_pool.get(&SECOND_AMMO), Some(&(160, 40, 2)));
    assert!(!game.ammo_pool.contains_key(&OLD_AMMO));
    assert_eq!(
        game.spawned,
        [
            (game.old_item, vec![[OLD_AMMO, 100]]),
            (game.incoming, vec![[RIFLE_AMMO, 110]]),
        ]
    );
    let stored: usize = game
        .spawned
        .iter()
        .flat_map(|(_, rows)| rows)
        .map(|row| row[1])
        .sum();
    let held: u32 = game.ammo_pool.values().map(|supply| supply.1).sum();
    assert_eq!(stored + held as usize, 100 + 350 + 40);
}

#[test]
fn retain_keeps_outgoing_rounds_off_the_drop_then_restores_compatible_reserve() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _policy = policy::set_for_test(policy::parse_settings("retain", "discard").unwrap());
    let mut game = FakeGame::new(None);
    let retained_info = unsafe { game.ammo_info("old_rifle") };
    unsafe {
        for offset in [0x210, 0x228] {
            let row = word(game.old_config, offset);
            FakeGame::put(row, 0, retained_info);
        }
        game.ammo_pool.clear();
        game.ammo_pool.insert(retained_info, (200, 100, 2));
        game.sync_pool();
        let incoming_row = word(game.incoming_config, 0x210);
        FakeGame::put(incoming_row, 0, retained_info);
    }
    ACTIVE.with(|active| active.set(&mut game));
    NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
    install_fake_bindings();

    unsafe { collect(game.manager, PICKUP, game.collector) };

    assert_eq!(
        reserve::snapshot(game.pool).unwrap(),
        Vec::<[usize; 2]>::new()
    );
    assert_eq!(game.ammo_pool.get(&retained_info), Some(&(240, 100, 2)));
    assert_eq!(
        game.spawned,
        [(game.old_item, Vec::<[usize; 2]>::new())],
        "retain leaves outgoing rounds in reserve rather than on the dropped gun"
    );
    let outgoing: usize = NATIVE_EVENTS.with(|events| {
        events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                NativeEvent::AmmoRemove(rows) => Some(rows.iter().map(|row| row[1]).sum::<usize>()),
                _ => None,
            })
            .sum()
    });
    let loaded_after: usize = game
        .ammo_pool
        .values()
        .map(|supply| supply.1 as usize)
        .sum();
    assert_eq!(outgoing, 100);
    assert_eq!(loaded_after, 100);
}

#[test]
fn retain_reserve_overflow_refuses_before_consuming_or_mutating_the_pickup() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _policy = policy::set_for_test(policy::parse_settings("retain", "discard").unwrap());
    let mut game = FakeGame::new(None);
    let retained_info = unsafe { game.ammo_info("old_rifle") };
    unsafe {
        for offset in [0x210, 0x228] {
            let row = word(game.old_config, offset);
            FakeGame::put(row, 0, retained_info);
        }
        game.ammo_pool.clear();
        game.ammo_pool.insert(retained_info, (200, 100, 2));
        game.sync_pool();
    }
    reserve::deposit_rows(game.pool, &[[retained_info, u32::MAX as usize - 50]]).unwrap();
    ACTIVE.with(|active| active.set(&mut game));
    NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
    install_fake_bindings();

    unsafe { collect(game.manager, PICKUP, game.collector) };

    assert!(NATIVE_EVENTS.with(|events| events.borrow().is_empty()));
    assert_eq!(game.ammo_pool.get(&retained_info), Some(&(200, 100, 2)));
    assert_eq!(
        reserve::snapshot(game.pool).unwrap(),
        [[retained_info, u32::MAX as usize - 50]]
    );
    assert!(game
        .members
        .iter()
        .all(|member| member.item_count == 3 && member.gun_count == 3));
}

#[test]
fn retain_only_withdraws_accepted_compatible_reserve_and_keeps_the_remainder() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _policy = policy::set_for_test(policy::parse_settings("retain", "discard").unwrap());
    let mut game = FakeGame::new(None);
    let old_info = unsafe { game.ammo_info("old_rifle") };
    let incoming_info = unsafe { game.ammo_info("new_rifle") };
    unsafe {
        for offset in [0x210, 0x228] {
            let row = word(game.old_config, offset);
            FakeGame::put(row, 0, old_info);
        }
        let incoming_row = word(game.incoming_config, 0x210);
        FakeGame::put(incoming_row, 0, incoming_info);
        game.ammo_pool.clear();
        game.ammo_pool.insert(old_info, (200, 100, 2));
        game.sync_pool();
        game.set_ground_ammo(&[[incoming_info, 200]]);
    }
    reserve::deposit_rows(game.pool, &[[incoming_info, 100]]).unwrap();
    ACTIVE.with(|active| active.set(&mut game));
    NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
    install_fake_bindings();

    unsafe { collect(game.manager, PICKUP, game.collector) };

    let reserve = reserve::snapshot(game.pool).unwrap();
    assert_eq!(reserve.len(), 2);
    assert!(reserve.contains(&[old_info, 100]));
    assert!(reserve.contains(&[incoming_info, 60]));
    assert_eq!(game.ammo_pool.get(&incoming_info), Some(&(240, 240, 2)));
    assert_eq!(game.spawned, [(game.old_item, Vec::<[usize; 2]>::new())]);
}

#[test]
fn identical_exchange_preserves_excess_rounds_once_across_repeated_ground_rows() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _policy = policy::set_for_test(policy::parse_settings("transfer", "discard").unwrap());
    let mut game = FakeGame::new(None);
    unsafe {
        game.same_primary_pickup();
        game.set_ground_ammo(&[[OLD_AMMO, 4], [OLD_AMMO, 250]]);
    }
    ACTIVE.with(|active| active.set(&mut game));
    NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
    install_fake_bindings();
    unsafe { collect(game.manager, PICKUP, game.collector) };
    assert_eq!(game.ammo_pool.get(&OLD_AMMO), Some(&(200, 200, 2)));
    assert_eq!(
        game.spawned,
        [
            (game.old_item, vec![[OLD_AMMO, 100]]),
            (game.old_item, vec![[OLD_AMMO, 54]]),
        ]
    );
    let ground: usize = game
        .spawned
        .iter()
        .flat_map(|(_, rows)| rows)
        .map(|row| row[1])
        .sum();
    assert_eq!(ground + 200, 100 + 4 + 250);
    for member in &game.members {
        assert_eq!(member.gun_count, 3);
        assert_eq!(member.item_count, 3);
        assert_eq!(unsafe { *((member.gunner + 0x94) as *const i32) }, 0);
    }
}

#[test]
fn identical_primary_exchanges_its_own_ammo_instead_of_topping_up() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _policy = policy::set_for_test(policy::parse_settings("transfer", "discard").unwrap());
    for outgoing_rounds in [100, 200] {
        let mut game = FakeGame::new(None);
        game.ammo_pool.get_mut(&OLD_AMMO).unwrap().1 = outgoing_rounds;
        unsafe {
            game.sync_pool();
            game.same_primary_pickup();
            game.set_ground_ammo(&[[OLD_AMMO, 50]]);
        }
        ACTIVE.with(|active| active.set(&mut game));
        NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
        install_fake_bindings();
        unsafe { collect(game.manager, PICKUP, game.collector) };
        assert_eq!(game.ammo_pool.get(&OLD_AMMO), Some(&(200, 50, 2)));
        assert_eq!(
            game.spawned,
            [(game.old_item, vec![[OLD_AMMO, outgoing_rounds as usize]])]
        );
        assert_eq!(
            NATIVE_EVENTS.with(|events| events
                .borrow()
                .iter()
                .filter(|event| matches!(event, NativeEvent::Detach(..)))
                .count()),
            2
        );
        assert!(game
            .members
            .iter()
            .all(|member| member.gun_count == 3 && member.item_count == 3));
    }
}

#[test]
fn loaded_squad_with_null_or_allocated_empty_cache_still_swaps_live_primaries() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for allocated in [false, true] {
        let mut game = FakeGame::new(None);
        unsafe {
            let begin = if allocated { game.cache_row } else { 0 };
            FakeGame::put(game.holder, 0x138, begin);
            FakeGame::put(game.holder, 0x140, begin);
            FakeGame::put(game.holder, 0x148, if allocated { begin + 0x10 } else { 0 });
        }
        ACTIVE.with(|active| active.set(&mut game));
        NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
        install_fake_bindings();
        unsafe { collect(game.manager, PICKUP, game.collector) };

        let events = NATIVE_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, NativeEvent::Detach(..)))
                .count(),
            2
        );
        assert_eq!(
            events
                .iter()
                .filter(
                    |event| matches!(event, NativeEvent::PrimaryAdd(_, ammo) if *ammo == [[RIFLE_AMMO, 0], [SECOND_AMMO, 0]])
                )
                .count(),
            2
        );
        assert_eq!(game.ammo_pool.get(&RIFLE_AMMO), Some(&(240, 0, 2)));
        assert_eq!(game.ammo_pool.get(&SECOND_AMMO), Some(&(160, 0, 2)));
        assert_eq!(events.iter().filter(|event| matches!(event, NativeEvent::Spawn(item, ammo) if *item == game.old_item && *ammo == [0; 3])).count(), 1);
        assert!(!events
            .iter()
            .any(|event| matches!(event, NativeEvent::StockDrop | NativeEvent::StockAdd)));
        for member in &game.members {
            assert!((0..member.item_count)
                .any(|index| unsafe { word(member.items, index * 8) == game.incoming }));
            assert!(!(0..member.item_count)
                .any(|index| unsafe { word(member.items, index * 8) == game.old_item }));
        }
        assert_eq!(unsafe { word(game.cache_row, 8) }, game.old_item);
        assert_eq!(
            unsafe { word(game.holder, 0x138) },
            if allocated { game.cache_row } else { 0 }
        );
    }
}

#[test]
fn malformed_slot_cache_headers_leave_the_pickup_unconsumed() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for malformed in 0..3 {
        let mut game = FakeGame::new(None);
        let header = match malformed {
            0 => [0, 0, 8],
            1 => [game.cache_row, game.cache_row + 8, game.cache_row + 16],
            _ => [game.cache_row, game.cache_row + 16, game.cache_row + 8],
        };
        unsafe {
            for (index, value) in header.into_iter().enumerate() {
                FakeGame::put(game.holder, 0x138 + index * 8, value);
            }
        }
        ACTIVE.with(|active| active.set(&mut game));
        NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
        install_fake_bindings();
        unsafe { collect(game.manager, PICKUP, game.collector) };
        assert!(NATIVE_EVENTS.with(|events| events.borrow().is_empty()));
    }
}

#[test]
fn unsupported_member_leaves_ground_pickup_unconsumed_before_stock_handler() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut game = FakeGame::new(None);
    unsafe {
        let extra_config = game.setup_config(false);
        let extra_item = game.setup_item("rifles", extra_config);
        let member = &mut game.members[1];
        FakeGame::put(member.items, member.item_count * 8, extra_item);
        member.item_count += 1;
        FakeGame::put(member.gunner, 0x58, member.items + member.item_count * 8);
    }
    ACTIVE.with(|active| active.set(&mut game));
    NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
    install_fake_bindings();

    unsafe { collect(game.manager, PICKUP, game.collector) };

    assert!(NATIVE_EVENTS.with(|events| events.borrow().is_empty()));
    for member in &game.members {
        assert!(unsafe {
            (0..member.item_count)
                .any(|index| *((member.items + index * 8) as *const usize) == game.old_item)
        });
    }
}

#[test]
fn selected_held_special_is_restored_after_each_primary_vector_mutation() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut game = FakeGame::new(None);
    for member in &game.members {
        unsafe {
            let special_gun = *((member.guns + 2 * 8) as *const usize);
            let special_item = *((member.items + 2 * 8) as *const usize);
            FakeGame::put(member.gunner, 0x28, special_gun);
            FakeGame::put(member.gunner, 0x30, special_item);
            FakeGame::put_i32(member.gunner, 0xcc, 1);
            FakeGame::put_i32(member.gunner, 0x94, 2);
        }
    }
    ACTIVE.with(|active| active.set(&mut game));
    NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
    install_fake_bindings();

    unsafe { collect(game.manager, PICKUP, game.collector) };

    let events = NATIVE_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
    for member in &game.members {
        let special_gun = unsafe { *((member.gunner + 0x28) as *const usize) };
        let special_item = unsafe { *((member.gunner + 0x30) as *const usize) };
        let selected = unsafe { *((member.gunner + 0x94) as *const i32) };
        assert!(selected >= 0 && (selected as usize) < member.gun_count);
        assert_eq!(special_gun, unsafe {
            word(member.guns, selected as usize * 8)
        });
        assert_eq!(special_item, unsafe {
            *((member.items + 8) as *const usize)
        });
        assert_eq!(unsafe { *((member.gunner + 0xcc) as *const i32) }, 1);
        assert_eq!(selected, 0);
        let visual = unsafe {
            let facets = *((member.entity + 8) as *const usize);
            *((facets + 8) as *const usize)
        };
        assert!(events.contains(&NativeEvent::Switch(visual, game.pistol_config)));
        assert!(events.contains(&NativeEvent::Sync(member.gunner)));
    }
}

#[test]
fn nested_same_holder_collection_is_refused_while_outer_exchange_completes() {
    let _guard = GAME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut game = FakeGame::new(None);
    game.nested_once = true;
    ACTIVE.with(|active| active.set(&mut game));
    NATIVE_EVENTS.with(|events| events.borrow_mut().clear());
    install_fake_bindings();

    unsafe { collect(game.manager, PICKUP, game.collector) };

    let events = NATIVE_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, NativeEvent::Collect))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, NativeEvent::Spawn(..)))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, NativeEvent::Detach(..)))
            .count(),
        2
    );
}

fn member(entity: usize, gunner: usize, category: &str, item: usize, gun: usize) -> MemberPlan {
    MemberPlan {
        entity,
        ai: entity + 0x10,
        gunner,
        slot: PrimarySlot {
            slot_index: 0,
            definition: 0x5000,
            category: category.to_owned(),
            current_item: item,
        },
        old_item: item,
        old_config: gun,
        old_gun: gunner + 0x100,
        special_gun: 0,
        special_item: 0,
        special_slot: 0,
        selected_gun_index: 0,
        selected_gun: gunner + 0x100,
        visual: entity + 0x200,
        empty_ammo: Vec::new(),
        pool: 0,
    }
}

fn candidate(slot_index: usize, item: usize, category: &str) -> Candidate {
    Candidate {
        slot_index,
        slot: 0x5000 + slot_index * 0x100,
        item,
        category: category.to_owned(),
    }
}

#[test]
fn batch_targets_every_member_with_the_same_declared_primary() {
    let members = [
        member(1, 11, "rifles", 101, 201),
        member(2, 12, "rifles", 101, 201),
        member(3, 13, "rifles", 101, 201),
    ];
    assert_eq!(validate_member_batch(&members, "rifles"), Ok(()));
    assert_eq!(members.len(), 3);
}

#[test]
fn mismatched_category_or_current_item_refuses_the_whole_batch() {
    let mut members = [
        member(1, 11, "rifles", 101, 201),
        member(2, 12, "pistols", 101, 201),
    ];
    assert!(validate_member_batch(&members, "rifles").is_err());
    members[1] = member(2, 12, "rifles", 102, 201);
    assert!(validate_member_batch(&members, "rifles").is_err());
}

#[test]
fn held_special_state_survives_preflight_unchanged() {
    let mut first = member(1, 11, "shotguns", 101, 201);
    first.special_gun = 301;
    first.special_item = 401;
    first.special_slot = 2;
    first.selected_gun_index = 3;
    let members = [first.clone(), member(2, 12, "shotguns", 101, 201)];
    assert_eq!(validate_member_batch(&members, "shotguns"), Ok(()));
    assert_eq!(members[0].special_gun, first.special_gun);
    assert_eq!(members[0].special_item, first.special_item);
    assert_eq!(members[0].special_slot, first.special_slot);
    assert_eq!(members[0].selected_gun_index, first.selected_gun_index);
}

#[test]
fn invalid_held_special_state_refuses_before_mutation() {
    let mut first = member(1, 11, "rifles", 101, 201);
    first.special_gun = 301;
    first.special_slot = 2;
    let members = [first, member(2, 12, "rifles", 101, 201)];
    assert_eq!(
        validate_member_batch(&members, "rifles"),
        Err("human squad member has invalid held-special state")
    );
}

#[test]
fn identical_primary_requires_every_member_to_have_the_incoming_item() {
    let members = [
        member(1, 11, "rifles", 101, 201),
        member(2, 12, "rifles", 101, 201),
    ];
    assert!(already_has_item(&members, 101));
    assert!(!already_has_item(&members, 102));
    let mixed_members = [
        member(1, 11, "rifles", 101, 201),
        member(2, 12, "rifles", 102, 202),
    ];
    assert!(!already_has_item(&mixed_members, 101));
    assert!(!already_has_item(&[], 101));
}

#[test]
fn category_selection_rejects_zero_or_ambiguous_declared_slots() {
    assert!(target_category(&[]).is_err());
    assert!(target_category(&[candidate(0, 10, "rifles"), candidate(1, 10, "rifles")]).is_err());
    assert_eq!(
        target_category(&[candidate(0, 10, "pistols")]).unwrap(),
        "pistols"
    );
}

#[test]
fn nested_same_holder_is_detected_but_unrelated_holder_is_allowed() {
    let plan = Plan {
        manager: 1,
        holder: 20,
        script: 21,
        incoming: 30,
        incoming_gun: 40,
        incoming_ammo: Vec::new(),
        outgoing_ammo: Vec::new(),
        leftovers: Vec::new(),
        ammo_policy: policy::AmmoPolicy::Discard,
        category: "rifles".to_owned(),
        members: vec![member(2, 12, "rifles", 101, 201)],
        same_item: false,
        applying: false,
        swapped: false,
        position: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
    };
    let stack = [Some(plan)];
    assert!(has_holder_conflict(&stack, 20));
    assert!(!has_holder_conflict(&stack, 21));
}

#[test]
fn bad_transform_is_rejected_before_any_native_mutation() {
    assert!(validate_transform([0.0, f32::NAN, 0.0], [0.0, 0.0, 0.0, 1.0]).is_err());
    assert!(validate_transform([0.0; 3], [0.0; 4]).is_err());
}
