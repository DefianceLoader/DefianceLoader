#[path = "history.rs"]
mod history;
use crate::{
    bindings as b,
    model::{remap_pins, Supply},
};
use defiance_api::{Api, ABI_VERSION, LOG_ERROR, LOG_INFO, LOG_WARN};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    ffi::c_char,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        OnceLock,
    },
};

type Unary = unsafe extern "C" fn(usize);
type Pair = unsafe extern "C" fn(usize, usize);
type Get = unsafe extern "C" fn(usize) -> usize;
type Predicate = unsafe extern "C" fn(usize) -> u8;
static ADDRESSES: OnceLock<Vec<usize>> = OnceLock::new();
static LOGGER: AtomicUsize = AtomicUsize::new(0);
static SELECTION: OnceLock<&'static defiance_api::SelectionV1> = OnceLock::new();
static LIMITS: OnceLock<crate::limits::Limits> = OnceLock::new();
static MENU: OnceLock<&'static defiance_api::AmmoMenuV1> = OnceLock::new();

/// Class offsets the 2026-09 update moved without moving the functions regroup
/// calls. The reference build's values are the defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Offsets {
    pub roster: usize,
    pub world_manager: usize,
    pub input_player: usize,
    pub input_ui: usize,
}
const REFERENCE_OFFSETS: Offsets = Offsets {
    roster: 0x3b8,
    world_manager: 0x700,
    input_player: 0x860,
    input_ui: 0x888,
};
const UPDATE_OFFSETS: Offsets = Offsets {
    roster: 0x3d0,
    world_manager: 0x708,
    input_player: 0x870,
    input_ui: 0x890,
};
/// The supported (logic.dll, game.dll) pairs and the offsets each reads. A
/// pair, as in Core, because the offsets span both modules: the roster getter
/// is a logic.dll class slot and the input fields are game.dll's, so a
/// logic.dll from one build with a game.dll from another would read one of
/// them stale.
const PAIRS: &[(&str, &str, Offsets)] = &[
    // GOG and Steam, 23 Dec 2025
    (
        "17ef48350153306e210e14a24b0ad398c56fb99d3d46c88f246df260ade85780",
        "f0184b9fe358172c83261419c8ba3d822a0aa6b06ed3cddb2f7aa3ebb9653db4",
        REFERENCE_OFFSETS,
    ),
    (
        "d320f848508c45c9f04df235204b5fbc9ffbb1b7f869e4c5a58d80bb2ecc10ed",
        "dc10419f417aed4ecff348b7c96b3c7c574a9a2541f76c5fbc35eb92dbc716d6",
        REFERENCE_OFFSETS,
    ),
    // GOG 14 Sep 2026, and the 22 Sep 2026 (DLC3) build both stores share
    (
        "eb8674f1d16595a3e9cf6a9ec0062735b1184976495d8d6ade36f7e2574e8aab",
        "bc2af42369f9f6fe70e206ae4846f8f0e46ac01cca9a325c8159ee04a9b5e405",
        UPDATE_OFFSETS,
    ),
    (
        "30264904e1d5199b954bafbd7828cf7190930c246d35fa7b94eefa915e8f0c38",
        "d926a213731d73bac8ccc56b50e2b4292fe8613c7a9bb7c8b9fc9132c122ed25",
        UPDATE_OFFSETS,
    ),
];
fn offsets_for(logic_sha: &str, game_sha: &str) -> Option<Offsets> {
    PAIRS
        .iter()
        .find(|&&(logic, game, _)| logic == logic_sha && game == game_sha)
        .map(|&(_, _, o)| o)
}
static OFFSETS: OnceLock<Offsets> = OnceLock::new();

pub(crate) fn offsets() -> Offsets {
    *OFFSETS.get().unwrap_or(&REFERENCE_OFFSETS)
}
#[cfg(test)]
thread_local! {
    static TEST_LIMITS: Cell<Option<(crate::limits::Limits, u32)>> = const { Cell::new(None) };
}
fn limits() -> crate::limits::Limits {
    #[cfg(test)]
    if let Some((limits, _)) = TEST_LIMITS.with(Cell::get) {
        return limits;
    }
    LIMITS.get().copied().unwrap_or_default().supported()
}
fn ammo_limit() -> usize {
    #[cfg(test)]
    if let Some((limits, slots)) = TEST_LIMITS.with(Cell::get) {
        return limits.ammo(slots);
    }
    limits().ammo(MENU.get().map_or(9, |menu| unsafe { (menu.capacity)() }))
}
static INPUT: AtomicUsize = AtomicUsize::new(0);
// Three five-byte register saves; room for a distant absolute detour.
const INPUT_DISPLACED: usize = 15;
static CREATE: AtomicUsize = AtomicUsize::new(0);
static SPAWN_FALLBACK: AtomicUsize = AtomicUsize::new(0);
// Do not risk repeated creation after an unverified native result. Thread-local
// because input, creation and restoration all run on the game's owning thread.
thread_local! {
    static CREATION_BLOCKED: Cell<bool> = const { Cell::new(false) };
    static CREATION_ABORTED: Cell<bool> = const { Cell::new(false) };
}
type AssignString = unsafe extern "C" fn(usize, usize, usize) -> usize;

/// The spawn-menu factory substitutes a default script/species name when the
/// requested name is absent from its menu list. Existing squads need not belong
/// to that list. Keep the factory's already-copied requested name for regroup;
/// all ordinary game spawns still call the original string assignment.
unsafe extern "C" fn keep_requested_species(dst: usize, src: usize, len: usize) -> usize {
    if CONTEXT.with(|p| p.borrow().is_some()) {
        log(
            LOG_INFO,
            "regroup factory: retained requested squad type instead of menu default",
        );
        dst
    } else {
        std::mem::transmute::<usize, AssignString>(SPAWN_FALLBACK.load(Ordering::Relaxed))(
            dst, src, len,
        )
    }
}
static WIRE: AtomicUsize = AtomicUsize::new(0);
static TEMPLATES: AtomicUsize = AtomicUsize::new(0);
static CLEANUP_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static UPDATE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
const CLEANUP_DISPLACED: usize = 16;
const UPDATE_DISPLACED: usize = 14;
const PERK_DISPLACED: usize = 19;
static PERK_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

/// Stock perk refresh copies the entire squad into Entity*[20] on its stack.
/// At 22 members the copy overwrites its return address. Retain its ordinary
/// branches, replacing only the oversized squad branch with owned storage.
unsafe extern "C" fn refresh_perks(perk: usize) {
    let original = || unsafe {
        std::mem::transmute::<usize, Unary>(PERK_ORIGINAL.load(Ordering::Relaxed))(perk)
    };
    if junction(q(perk, 0x160)) != 0 {
        original();
        return;
    }
    let e = entity(perk);
    if e == 0 || !kind(e, 0x10) {
        original();
        return;
    }
    let ai = q(facets(e), 0x28);
    let holder = if ai == 0 {
        0
    } else {
        get(ai, offsets().roster)
    };
    if holder == 0 {
        original();
        return;
    }
    let count = match pointers(holder, 0xa0, 64) {
        Ok(members) => members.len(),
        Err(reason) => {
            log(LOG_ERROR, &format!("perk refresh refused: {reason}"));
            return;
        }
    };
    if count <= 20 {
        original();
        return;
    }
    // Same order as the native squad branch: prepare shared effects, update
    // squad state, then snapshot the current roster and refresh each member.
    call(b::PERK_PREPARE, perk);
    call(b::PERK_UPDATE, perk);
    let members = match pointers(holder, 0xa0, 64) {
        Ok(members) => members,
        Err(reason) => {
            log(LOG_ERROR, reason);
            return;
        }
    };
    for member in members {
        let member_perk = q(facets(member), 0x70);
        if member_perk != 0 {
            call(b::PERK_MEMBER, member_perk);
        }
    }
}
// The stock UI export resizes to 20, copies the full roster, then resizes to
// the returned count. Besides overflowing a fresh allocation, the final resize
// zeroes entries 21 onward. Use the game's allocator before copying.
type ExportRoster = unsafe extern "C" fn(usize, usize, usize);
static ROSTER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
const ROSTER_DISPLACED: usize = 16;

unsafe extern "C" fn export_roster(service: usize, e: usize, out: usize) {
    let original = || unsafe {
        std::mem::transmute::<usize, ExportRoster>(ROSTER_ORIGINAL.load(Ordering::Relaxed))(
            service, e, out,
        )
    };
    if e == 0 || !kind(e, 0x10) {
        original();
        return;
    }
    let ai = q(facets(e), 0x28);
    let holder = if ai == 0 {
        0
    } else {
        get(ai, offsets().roster)
    };
    if holder == 0 {
        original();
        return;
    }
    let members = match pointers(holder, 0xa0, 64) {
        Ok(members) => members,
        Err(reason) => {
            log(LOG_ERROR, &format!("UI roster export refused: {reason}"));
            std::mem::transmute::<usize, Pair>(address(b::RESIZE_ROSTER))(out, 0);
            return;
        }
    };
    if members.len() <= 20 {
        original();
        return;
    }
    std::mem::transmute::<usize, Pair>(address(b::RESIZE_ROSTER))(out, members.len());
    std::ptr::copy_nonoverlapping(members.as_ptr(), q(out, 0) as *mut usize, members.len());
}
static HOVER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
const HOVER_DISPLACED: usize = 16;

// SquadHoverIcon::update, also called by PlayerSquadHoverIcon::update.
// Its root widget is +140; virtual +48 is the stock visibility setter used
// by HoverIcon::update. Do not retain UI pointers or overwrite its hide flags:
// the next populated update must resume normal visibility/position handling.
unsafe extern "C" fn update_hover(icon: usize) {
    if history::is_parked(junction(q(icon, 0x160))) {
        let root = q(icon, 0x140);
        if root != 0 {
            std::mem::transmute::<usize, unsafe extern "C" fn(usize, u8)>(q(q(root, 0), 0x48))(
                root, 0,
            );
        }
    } else {
        std::mem::transmute::<usize, Unary>(HOVER_ORIGINAL.load(Ordering::Relaxed))(icon);
    }
}
static BUSY: AtomicBool = AtomicBool::new(false);
static INPUT_SEEN: AtomicBool = AtomicBool::new(false);

thread_local! {
    static CONTEXT: RefCell<Option<Plan>> = const { RefCell::new(None) };
    static DESTINATION: Cell<usize> = const { Cell::new(0) };
    static PREPARED_HOLDER: Cell<usize> = const { Cell::new(0) };
    static BINDING_HOLDER: Cell<usize> = const { Cell::new(0) };
    static CONSTRUCTED: Cell<bool> = const { Cell::new(false) };
}

#[link(name = "user32")]
extern "system" {
    fn GetForegroundWindow() -> usize;
    fn GetWindowThreadProcessId(window: usize, process: *mut u32) -> u32;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcessId() -> u32;
    fn GetModuleFileNameW(module: usize, path: *mut u16, size: u32) -> u32;
}

fn address(id: usize) -> usize {
    ADDRESSES.get().unwrap()[id]
}
fn log(level: u32, message: &str) {
    let ptr = LOGGER.load(Ordering::Relaxed);
    if ptr == 0 {
        return;
    }
    let text = format!("defiance.regroup: {message}\0");
    unsafe {
        std::mem::transmute::<usize, unsafe extern "C" fn(u32, *const c_char)>(ptr)(
            level,
            text.as_ptr().cast(),
        );
    }
}
unsafe fn q(p: usize, offset: usize) -> usize {
    *((p + offset) as *const usize)
}
unsafe fn d(p: usize, offset: usize) -> u32 {
    *((p + offset) as *const u32)
}
unsafe fn byte(p: usize, offset: usize) -> u8 {
    *((p + offset) as *const u8)
}
unsafe fn put(p: usize, offset: usize, value: u32) {
    *((p + offset) as *mut u32) = value;
}
unsafe fn method(p: usize, offset: usize) -> usize {
    q(q(p, 0), offset)
}
unsafe fn get(p: usize, offset: usize) -> usize {
    std::mem::transmute::<usize, Get>(method(p, offset))(p)
}
unsafe fn call(id: usize, p: usize) {
    std::mem::transmute::<usize, Unary>(address(id))(p);
}
unsafe fn pair(id: usize, p: usize, arg: usize) {
    std::mem::transmute::<usize, Pair>(address(id))(p, arg);
}
unsafe fn junction(p: usize) -> usize {
    if p == 0 {
        0
    } else {
        q(p, 0x10)
    }
}
unsafe fn entity(facet: usize) -> usize {
    junction(q(facet, 0x10))
}
unsafe fn facets(e: usize) -> usize {
    get(e, 0xb0)
}
unsafe fn kind(e: usize, flag: u32) -> bool {
    std::mem::transmute::<usize, unsafe extern "C" fn(usize, u32) -> u8>(method(e, 0x98))(e, flag)
        != 0
}
unsafe fn selected(facet: usize) -> bool {
    (SELECTION
        .get()
        .expect("selection service resolved before hooks")
        .is_selected)(facet as *mut _)
        != 0
}
unsafe fn pointers(p: usize, offset: usize, max: usize) -> Result<Vec<usize>, &'static str> {
    let (begin, end) = (q(p, offset), q(p, offset + 8));
    if end < begin || (end - begin) % 8 != 0 || (end - begin) / 8 > max || (begin == 0 && end != 0)
    {
        return Err("invalid or oversized entity vector");
    }
    Ok((begin..end)
        .step_by(8)
        .map(|p| *(p as *const usize))
        .collect())
}

#[derive(Clone)]
struct Gun {
    ptr: usize,
    ammo: usize,
    loaded: u32,
    supported: Vec<usize>,
}
#[derive(Clone)]
struct Member {
    entity: usize,
    ai: usize,
    gunner: usize,
    select: usize,
    picked: bool,
    guns: Vec<Gun>,
    pins: (u8, u8),
}
#[derive(Clone)]
struct Slot {
    ammo: usize,
    supply: Supply,
}
#[derive(Clone)]
struct Source {
    entity: usize,
    ai: usize,
    holder: usize,
    pool: usize,
    slots: Vec<Slot>,
    members: Vec<Member>,
    capacity: u32,
}
#[derive(Clone)]
struct PoolChange {
    source: usize,
    ammo: usize,
    before: Supply,
    left: Supply,
}
#[derive(Clone)]
struct Plan {
    world: usize,
    species: usize,
    team: usize,
    position: [f32; 3],
    sources: Vec<Source>,
    moved: Vec<Member>,
    pools: BTreeMap<usize, Supply>,
    changes: Vec<PoolChange>,
}

unsafe fn pool_slots(pool: usize) -> Result<Vec<Slot>, &'static str> {
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
unsafe fn record(pool: usize, ammo: usize) -> usize {
    let (begin, end) = (q(pool, 0x20), q(pool, 0x28));
    (begin..end)
        .step_by(0x48)
        .find(|p| q(*p, 0) == ammo)
        .unwrap_or(0)
}
unsafe fn write_supply(pool: usize, ammo: usize, s: Supply) {
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

unsafe fn member(e: usize, pool: usize) -> Result<Member, &'static str> {
    if e == 0 || !kind(e, 0x20) {
        return Err("selection contains a non-infantry member");
    }
    let f = facets(e);
    let (ai, select, team) = (q(f, 0x28), q(f, 0x50), q(f, 0x20));
    if ai == 0
        || q(ai, 0) != address(b::HUMAN_AI)
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
        if gun == 0 || q(gun, 0) != address(b::GUN) || junction(q(gun, 0x58)) != pool {
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

unsafe fn plan(manager: usize) -> Result<Plan, &'static str> {
    plan_for(manager, None, false)
}
unsafe fn plan_for(
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
        let ai = q(facets(e), 0x28);
        if ai == 0 || q(ai, 0) != address(b::SQUAD_AI) || byte(ai, 0x130) != 0 {
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

fn prepare_ammo(result: &mut Plan) -> Result<(), &'static str> {
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

/// Replaces only the call inside SquadHolderFacet::add. Normal spawns and
/// gameplay use the original. During regrouping retain every gun's loaded
/// magazine, chosen ammo and reload state; the stock binder resets them.
unsafe extern "C" fn wire(holder: usize, e: usize) {
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
        pair(b::BIND, gun + 0x58, pool);
    }
    pair(b::BIND, member_ai + 0x148, pool);
    // The stock reallocation helper is used only when the vector is full.
    let append: unsafe extern "C" fn(usize, usize, *const usize) =
        std::mem::transmute(address(b::APPEND));
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
unsafe fn species_id(species: usize) -> Option<Vec<u8>> {
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
unsafe fn same_species(actual: usize, expected: usize) -> bool {
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
unsafe fn species_label(species: usize) -> String {
    species_id(species).map_or_else(
        || "<invalid>".into(),
        |id| format!("{:?}", String::from_utf8_lossy(&id)),
    )
}

unsafe fn populate(holder: usize, p: &Plan) -> Result<(), &'static str> {
    let e = entity(holder);
    if e == 0 || get(e, 0x78) != p.world || !same_species(q(holder, 0xc0), p.species) {
        return Err("constructor did not produce the requested squad type");
    }
    let ai = q(facets(e), 0x28);
    if ai == 0 || q(ai, 0) != address(b::SQUAD_AI) {
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

unsafe fn transfer_into(holder: usize, p: &Plan, existing: &[Member]) -> Result<(), &'static str> {
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
    pair(b::RESERVE, holder + 0xa0, total);
    pair(b::RESERVE, ai + 0x1e8, total);
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
            pair(b::REMOVE_GUNNER, source.ai, m.gunner);
            pair(b::REMOVE, source.holder, m.entity);
            pair(b::ADD, holder, m.entity);
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

unsafe extern "C" fn cleanup_empty(ai: usize) {
    if !history::park_if_needed(ai) {
        std::mem::transmute::<usize, Unary>(CLEANUP_ORIGINAL.load(Ordering::Relaxed))(ai);
    }
}
unsafe extern "C" fn update_squad(ai: usize, elapsed: f32) {
    if !history::park_if_needed(ai) {
        std::mem::transmute::<usize, unsafe extern "C" fn(usize, f32)>(
            UPDATE_ORIGINAL.load(Ordering::Relaxed),
        )(ai, elapsed);
    }
}

unsafe extern "C" fn prepare_templates(holder: usize, config: usize) {
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
                log(LOG_INFO, &format!("regroup destination: matched separately allocated species {} by identifier and native type", species_label(actual_species)));
            }
            PREPARED_HOLDER.with(|v| v.set(holder));
            log(
                LOG_INFO,
                "regroup destination: skipped default weapon templates",
            );
            return;
        }
    }
    std::mem::transmute::<usize, Pair>(TEMPLATES.load(Ordering::Relaxed))(holder, config);
}

unsafe extern "C" fn create_members(holder: usize, owned_names: usize) {
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
        call(b::FREE_STRINGS, owned_names);
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
    call(b::FREE_STRINGS, owned_names);
}

unsafe fn regroup(manager: usize, p: Plan) -> usize {
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
    let spawn: Spawn = std::mem::transmute(address(b::SPAWN));
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
            let ai = q(facets(result), 0x28);
            // Cleanup is only valid for an empty squad. Never delete a populated
            // result or one holding transferred soldiers after a partial failure.
            if ai != 0 && q(ai, 0) == address(b::SQUAD_AI) {
                let holder = get(ai, offsets().roster);
                if holder != 0 && pointers(holder, 0xa0, 64).is_ok_and(|m| m.is_empty()) {
                    call(b::CLEANUP, ai);
                    log(
                        LOG_INFO,
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
        call(b::CLEANUP, source.ai);
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

// The game dispatch passes pointers to the Win32 message and its parameters.
type InputDispatch = unsafe extern "C" fn(usize, *const u32, *const usize, *const usize);

static HOTKEYS: OnceLock<[crate::hotkeys::Chord; 2]> = OnceLock::new();

unsafe fn configure_hotkeys(api: &Api) -> Result<(), String> {
    let mut chords = Vec::new();
    for (name, default) in [
        ("regroup_hotkey", "Ctrl+Alt+R"),
        ("restore_hotkey", "Ctrl+Alt+U"),
    ] {
        let key = std::ffi::CString::new(name).unwrap();
        let value = (api.config_get)(b"defiance.regroup\0".as_ptr().cast(), key.as_ptr());
        let text = if value.is_null() {
            default
        } else {
            std::ffi::CStr::from_ptr(value)
                .to_str()
                .map_err(|_| format!("{name}: invalid UTF-8"))?
        };
        let chord = crate::hotkeys::Chord::parse(text).map_err(|e| format!("{name}: {e}"))?;
        log(LOG_INFO, &format!("{name} = {text}"));
        chords.push(chord);
    }
    if chords[0] == chords[1] {
        return Err("regroup and restore hotkeys must differ".into());
    }
    HOTKEYS
        .set([chords[0], chords[1]])
        .map_err(|_| "hotkeys already initialized".into())
}

unsafe fn input_manager(input: usize) -> Result<usize, &'static str> {
    let player = q(input, offsets().input_player);
    if player == 0 {
        return Err("no active player context");
    }
    let simulation = get(player, 0x68);
    if simulation == 0 {
        return Err("no active simulation");
    }
    let world = get(simulation, 0x38);
    if world == 0 {
        return Err("no active world");
    }
    let team = get(player, 0x40);
    type Lookup = unsafe extern "C" fn(usize, usize) -> usize;
    let manager =
        std::mem::transmute::<usize, Lookup>(method(world, offsets().world_manager))(world, team);
    if manager == 0 {
        return Err("no selection manager for the current player");
    }
    Ok(manager)
}

unsafe extern "C" fn input_dispatch(
    dispatch: usize,
    message: *const u32,
    key: *const usize,
    flags: *const usize,
) {
    let (msg, vk, bits) = (*message, *key, *flags);
    // Let the game update its held-key bitset and route the event first.
    std::mem::transmute::<usize, InputDispatch>(INPUT.load(Ordering::Relaxed))(
        dispatch, message, key, flags,
    );
    if !INPUT_SEEN.swap(true, Ordering::Relaxed) {
        log(LOG_INFO, "game input dispatch hook reached");
    }
    let input = q(dispatch, 8);
    if input == 0 {
        return;
    }
    let modifiers = u8::from(byte(input, 0x880) != 0)
        | (u8::from(byte(input, 0x881) != 0) << 1)
        | (u8::from(byte(input, 0x882) != 0) << 2);
    let Some(chords) = HOTKEYS.get() else {
        return;
    };
    let Some(action) = chords
        .iter()
        .position(|c| c.matches(msg, vk, bits, modifiers))
    else {
        return;
    };
    if CREATION_BLOCKED.with(Cell::get) {
        log(LOG_ERROR, "regroup/restore blocked after an unverified creation; restart and reload a pre-error save");
        return;
    }
    if BUSY.swap(true, Ordering::Acquire) {
        return;
    }
    log(
        LOG_INFO,
        if action == 1 {
            "restore keyboard event detected"
        } else {
            "regroup keyboard event detected"
        },
    );
    let mut pid = 0;
    GetWindowThreadProcessId(GetForegroundWindow(), &mut pid);
    let ui = q(input, offsets().input_ui);
    if pid != GetCurrentProcessId() {
        log(
            LOG_WARN,
            "regroup hotkey ignored: game is not the foreground process",
        );
    } else if ui == 0 || q(ui, 0x18) != 0 {
        // Same UI-capture guard used before stock keyboard shortcuts.
        log(
            LOG_WARN,
            "regroup hotkey ignored: UI has captured keyboard input",
        );
    } else if action == 1 {
        log(
            LOG_INFO,
            &format!(
                "restore limits: {} soldiers, {} ammunition types",
                limits().soldiers,
                ammo_limit()
            ),
        );
        if let Err(reason) = input_manager(input).and_then(|manager| history::restore(manager)) {
            log(
                LOG_WARN,
                &format!("restore stopped: {reason}; any earlier completed groups remain restored"),
            );
        }
    } else {
        log(
            LOG_INFO,
            &format!(
                "regroup limits: {} soldiers, {} ammunition types",
                limits().soldiers,
                ammo_limit()
            ),
        );
        match input_manager(input).and_then(|manager| plan(manager).map(|p| (manager, p))) {
            Ok((manager, p)) => {
                log(
                    LOG_INFO,
                    &format!(
                        "regroup preflight passed: {} soldiers from {} squads; starting creation",
                        p.moved.len(),
                        p.sources.len()
                    ),
                );
                match history::remember(&p) {
                    Ok(()) => {
                        regroup(manager, p);
                    }
                    Err(reason) => log(LOG_WARN, &format!("cannot record origins: {reason}")),
                }
            }
            Err(reason) => log(LOG_WARN, &format!("regroup rejected: {reason}")),
        }
    }
    BUSY.store(false, Ordering::Release);
}

unsafe fn validated_build(
    api: &Api,
    name: &[u8],
    builds: &'static [b::Build],
) -> Result<(usize, &'static b::Build), &'static str> {
    let base = (api.module_base)(name.as_ptr().cast()) as usize;
    if base == 0 {
        return Err("required game module is not loaded");
    }
    let mut path = vec![0u16; 32768];
    let size = GetModuleFileNameW(base, path.as_mut_ptr(), path.len() as u32) as usize;
    if size == 0 || size >= path.len() {
        return Err("cannot resolve module path");
    }
    use std::os::windows::ffi::OsStringExt;
    let path = std::ffi::OsString::from_wide(&path[..size]);
    let bytes = std::fs::read(path).map_err(|_| "cannot validate module on disk")?;
    let mut hasher = defiance_core::sha256::Sha256::new();
    hasher.update(&bytes);
    let hash = defiance_core::sha256::hex(&hasher.finish());
    let build = builds
        .iter()
        .find(|b| b.sha == hash)
        .ok_or("unsupported module build")?;
    let module_size = (api.module_size)(base as *mut _);
    for &(rva, expected) in build.checks {
        if rva + expected.len() > module_size
            || std::slice::from_raw_parts((base + rva) as *const u8, expected.len()) != expected
        {
            return Err("native entry validation failed");
        }
    }
    Ok((base, build))
}

pub unsafe extern "C" fn init(api: *const Api) -> i32 {
    if api.is_null() || (*api).abi_version != ABI_VERSION {
        return 1;
    }
    let api = &*api;
    LOGGER.store(api.log as usize, Ordering::Relaxed);
    let configured = (|| -> Result<crate::limits::Limits, String> {
        let soldiers = defiance_feature_sdk::integer(api, "defiance.regroup", "max_soldiers")
            .map_err(|e| e.to_string())?;
        let weapons = defiance_feature_sdk::integer(api, "defiance.regroup", "max_weapon_types")
            .map_err(|e| e.to_string())?;
        crate::limits::Limits::new(soldiers, weapons).map_err(str::to_owned)
    })();
    let configured = match configured {
        Ok(value) => value,
        Err(error) => {
            log(LOG_ERROR, &error);
            return 1;
        }
    };
    let Some(menu) = defiance_feature_sdk::services::ammo_menu() else {
        log(
            LOG_ERROR,
            "Core ammo-menu service v1 unavailable; update Core with regroup",
        );
        return 1;
    };
    if configured.soldiers > configured.supported().soldiers {
        log(LOG_WARN, "max_soldiers is temporarily capped at 20: native squad-command paths still have 20-member stack buffers");
    }
    if LIMITS.set(configured).is_err() || MENU.set(menu).is_err() {
        return 1;
    }
    log(LOG_INFO, &format!("limits: max_soldiers={}, max_weapon_types={} (0=automatic); effective ammo cap read from Core at each operation", configured.soldiers, configured.weapon_types));
    let Some(selection) = defiance_feature_sdk::services::selection() else {
        log(
            LOG_ERROR,
            "selection service v1 unavailable; update the loader and selection plugin together",
        );
        return 1;
    };
    if SELECTION.set(selection).is_err() {
        return 1;
    }
    if let Err(reason) = configure_hotkeys(api) {
        log(LOG_ERROR, &reason);
        return 1;
    }
    let (base, build) = match validated_build(api, b"logic.dll\0", b::BUILDS) {
        Ok(v) => v,
        Err(reason) => {
            log(LOG_ERROR, &format!("logic.dll: {reason}"));
            return 1;
        }
    };
    let (game_base, game_build) = match validated_build(api, b"game.dll\0", b::GAME_BUILDS) {
        Ok(v) => v,
        Err(reason) => {
            log(LOG_ERROR, &format!("game.dll: {reason}"));
            return 1;
        }
    };
    // The 2026-09 builds moved the roster getter, the world manager getter and
    // two input fields; the two modules are selected together, as a pair.
    let Some(offsets) = offsets_for(build.sha, game_build.sha) else {
        log(
            LOG_ERROR,
            "logic.dll and game.dll are from different builds; no hooks installed",
        );
        return 1;
    };
    let _ = OFFSETS.set(offsets);
    if ADDRESSES
        .set(build.rvas.iter().map(|rva| base + rva).collect())
        .is_err()
    {
        return 1;
    }
    for (index, detour, original) in [
        (
            b::SPAWN_FALLBACK_CALL,
            keep_requested_species as *const () as usize,
            &SPAWN_FALLBACK,
        ),
        (
            b::CREATE_CALL,
            create_members as *const () as usize,
            &CREATE,
        ),
        (b::WIRE_CALL, wire as *const () as usize, &WIRE),
        (
            b::TEMPLATES_CALL,
            prepare_templates as *const () as usize,
            &TEMPLATES,
        ),
    ] {
        // The loader stores the original into this atomic *before* it publishes
        // the branch, so a detour that fires immediately never sees a zero.
        if (api.hook_call)(
            address(index) as *mut _,
            detour as *mut _,
            original.as_ptr().cast(),
        ) != 0
        {
            return 1;
        }
    }
    if (api.hook_exact)(
        (game_base + game_build.rvas[0]) as *mut _,
        input_dispatch as *mut _,
        INPUT_DISPLACED,
        INPUT.as_ptr().cast(),
    ) != 0
    {
        return 1;
    }
    if (api.hook_exact)(
        (game_base + game_build.rvas[4]) as *mut _,
        update_hover as *mut _,
        HOVER_DISPLACED,
        HOVER_ORIGINAL.as_ptr().cast(),
    ) != 0
    {
        return 1;
    }
    for (index, detour, span, original) in [
        (
            b::PERK_REFRESH,
            refresh_perks as *const () as usize,
            PERK_DISPLACED,
            &PERK_ORIGINAL,
        ),
        (
            b::EXPORT_ROSTER,
            export_roster as *const () as usize,
            ROSTER_DISPLACED,
            &ROSTER_ORIGINAL,
        ),
        (
            b::CLEANUP,
            cleanup_empty as *const () as usize,
            CLEANUP_DISPLACED,
            &CLEANUP_ORIGINAL,
        ),
        (
            b::SQUAD_UPDATE,
            update_squad as *const () as usize,
            UPDATE_DISPLACED,
            &UPDATE_ORIGINAL,
        ),
    ] {
        // As above: the loader fills the atomic before publication.
        if (api.hook_exact)(
            address(index) as *mut _,
            detour as *mut _,
            span,
            original.as_ptr().cast(),
        ) != 0
        {
            return 1;
        }
    }
    log(
        LOG_INFO,
        "experimental regroup loaded (large squad UI roster fix 11): configured regroup/restore shortcuts; live-world history only; single-player only",
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    static HISTORY_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Every generated build belongs to exactly one pair, and a logic.dll is
    /// never accepted with another build's game.dll.
    #[test]
    fn offsets_are_chosen_by_the_module_pair() {
        for build in b::BUILDS {
            assert_eq!(
                PAIRS.iter().filter(|p| p.0 == build.sha).count(),
                1,
                "logic {}",
                build.sha
            );
        }
        for build in b::GAME_BUILDS {
            assert_eq!(
                PAIRS.iter().filter(|p| p.1 == build.sha).count(),
                1,
                "game {}",
                build.sha
            );
        }
        for &(logic, game, offsets) in PAIRS {
            assert_eq!(offsets_for(logic, game), Some(offsets));
            for &(_, other, _) in PAIRS.iter().filter(|p| p.1 != game) {
                assert_eq!(offsets_for(logic, other), None, "{logic} with {other}");
            }
        }
    }
    #[test]
    fn preservation_hook_spans_are_copyable_in_both_builds() {
        for build in b::GAME_BUILDS {
            let bytes = build
                .checks
                .iter()
                .find(|(at, _)| *at == build.rvas[4])
                .unwrap()
                .1;
            defiance_core::decode::validate_copy(&bytes[..HOVER_DISPLACED]).unwrap();
        }
        for build in b::BUILDS {
            for (index, span) in [
                (b::PERK_REFRESH, PERK_DISPLACED),
                (b::EXPORT_ROSTER, ROSTER_DISPLACED),
                (b::CLEANUP, CLEANUP_DISPLACED),
                (b::SQUAD_UPDATE, UPDATE_DISPLACED),
            ] {
                let bytes = build
                    .checks
                    .iter()
                    .find(|(at, _)| *at == build.rvas[index])
                    .unwrap()
                    .1;
                assert!(span >= 14);
                defiance_core::decode::validate_copy(&bytes[..span]).unwrap();
            }
        }
    }
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
    #[test]
    fn input_entry_supports_an_absolute_jump_in_every_supported_build() {
        assert!(INPUT_DISPLACED >= 14);
        for build in b::GAME_BUILDS {
            let (_, bytes) = build
                .checks
                .iter()
                .find(|(rva, _)| *rva == build.rvas[0])
                .unwrap();
            // The runtime fingerprint covers the entire displaced span.
            assert!(bytes.len() >= INPUT_DISPLACED);
            assert_eq!(
                defiance_core::decode::displaced(bytes, 14).unwrap(),
                INPUT_DISPLACED
            );
            defiance_core::decode::validate_copy(&bytes[..INPUT_DISPLACED]).unwrap();
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
        ADDRESSES.get_or_init(|| {
            let mut v = vec![0; b::BUILDS[0].rvas.len()];
            for (i, f) in [
                (b::SPAWN, mock_spawn as *const () as usize),
                (b::PERK_PREPARE, perk_prepare as *const () as usize),
                (b::PERK_UPDATE, perk_update as *const () as usize),
                (b::PERK_MEMBER, perk_member as *const () as usize),
                (b::RESIZE_ROSTER, resize_roster as *const () as usize),
                (b::RESERVE, reserve as *const () as usize),
                (b::BIND, bind as *const () as usize),
                (b::WEAK_BIND, weak_bind as *const () as usize),
                (b::REMOVE_GUNNER, remove_gunner as *const () as usize),
                (b::REMOVE, remove as *const () as usize),
                (b::ADD, add as *const () as usize),
                (b::FREE_STRINGS, free_strings as *const () as usize),
            ] {
                v[i] = f;
            }
            unsafe extern "C" fn holder(ai: usize) -> usize {
                junction(q(ai, 0x1c8))
            }
            let mut table = Box::new([0usize; 128]);
            table[0x3b8 / 8] = holder as *const () as usize;
            v[b::SQUAD_AI] = Box::into_raw(table) as usize;
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
        set(ai, 0, address(b::SQUAD_AI));
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
                TEST_LIMITS
                    .with(|v| v.set(Some((crate::limits::Limits::new(64, 0).unwrap(), 126))));
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

                TEST_LIMITS
                    .with(|v| v.set(Some((crate::limits::Limits::new(64, 0).unwrap(), 126))));
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
            // No template claim: this used to call the stock soldier factory.
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
}
