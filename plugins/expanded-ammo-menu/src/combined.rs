//! Optional union view. All native pointers are reacquired on the game thread.
use super::{read, Offsets, Pair};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicUsize, Ordering},
        OnceLock,
    },
};
static SELECT: OnceLock<&'static defiance_api::SelectionV1> = OnceLock::new();
static FUNCTIONS: OnceLock<[usize; 8]> = OnceLock::new();
pub static CLICK: AtomicUsize = AtomicUsize::new(0);
/// The selected build's class offsets, set by `install` before any render.
static OFFSETS: OnceLock<Offsets> = OnceLock::new();
pub fn set_offsets(offsets: Offsets) {
    let _ = OFFSETS.set(offsets);
}
/// The reference build's values, used only if offsets were never set.
const REFERENCE: Offsets = Offsets {
    roster: 0x3b8,
    gunner_count: 0x130,
    gunner_get: 0x120,
    pool_get: 0x1b8,
    world_player: 0x700,
    ai_set: 0x3e0,
};
fn offsets() -> Offsets {
    *OFFSETS.get().unwrap_or(&REFERENCE)
}
type Get = unsafe extern "C" fn(usize) -> usize;
type Set = unsafe extern "C" fn(usize, usize, u32);
type Fill = unsafe extern "C" fn(usize, usize, usize);
unsafe fn get(p: usize, offset: usize) -> usize {
    std::mem::transmute::<usize, Get>(read(read(p) + offset))(p)
}
unsafe fn kind(p: usize, flag: u32) -> bool {
    std::mem::transmute::<usize, unsafe extern "C" fn(usize, u32) -> u8>(read(read(p) + 0x98))(
        p, flag,
    ) != 0
}
unsafe fn vector(header: usize, stride: usize, bound: usize) -> Option<Vec<usize>> {
    let (a, b) = (read(header), read(header + 8));
    if b < a || (b - a) % stride != 0 || (b - a) / stride > bound || (a == 0 && b != 0) {
        return None;
    }
    Some((a..b).step_by(stride).collect())
}
#[derive(Clone)]
struct Source {
    entity: usize,
    ai: usize,
    records: Vec<[u64; 9]>,
    states: Vec<State>,
}
#[derive(Clone, Copy, Default)]
struct State {
    enabled: u32,
    total: u32,
    /// The users' summed readiness ([`readiness`]); a disabled user adds 0.
    ready: f32,
}
/// A user's readiness from the reload progress (Gun::vt+c8) of each gun that
/// has this ammunition loaded: the lowest progress in [0,1), or 1 when none
/// is reloading. A ready gun reports 1; NaN and out-of-range values are not
/// reloads. The same rule as the single-squad panel's `ammo_ui_ready`.
fn readiness(progress: impl IntoIterator<Item = f32>) -> f32 {
    progress
        .into_iter()
        .filter(|p| (0.0..1.0).contains(p))
        .fold(1.0, f32::min)
}
impl State {
    fn merge(&mut self, other: Self) {
        self.enabled = self.enabled.saturating_add(other.enabled);
        self.total = self.total.saturating_add(other.total);
        self.ready += other.ready;
    }
    /// The reload bar: the ready share of the selected users. Full when every
    /// user is enabled and ready; 2/3 at rest with two of three enabled.
    fn reload_share(self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.ready / self.total as f32).clamp(0.0, 1.0)
        }
    }
    fn mixed(self) -> bool {
        self.enabled > 0 && self.enabled < self.total
    }
    fn label(self) -> String {
        if self.mixed() {
            format!("{}/{}", self.enabled, self.total)
        } else {
            self.total.to_string()
        }
    }
}
#[derive(Clone)]
struct Card {
    record: [u64; 9],
    state: State,
}
fn field(r: &[u64; 9], at: usize) -> u32 {
    (r[at / 8] >> ((at % 8) * 8)) as u32
}
fn set_field(r: &mut [u64; 9], at: usize, v: u32) {
    let shift = (at % 8) * 8;
    r[at / 8] = (r[at / 8] & !(0xffff_ffffu64 << shift)) | ((v as u64) << shift);
}
fn cards(sources: &[Source]) -> Vec<Card> {
    let mut result = BTreeMap::<u64, Card>::new();
    for source in sources {
        for (r, state) in source.records.iter().zip(&source.states) {
            if r[0] == 0 || state.total == 0 {
                continue;
            }
            if let Some(card) = result.get_mut(&r[0]) {
                for at in [0x28, 0x2c, 0x30, 0x34] {
                    let sum = field(&card.record, at).saturating_add(field(r, at));
                    set_field(&mut card.record, at, sum);
                }
                let disabled = u32::from(field(&card.record, 0x3c) != 0 && field(r, 0x3c) != 0);
                set_field(&mut card.record, 0x3c, disabled);
                card.state.merge(*state);
            } else {
                result.insert(
                    r[0],
                    Card {
                        record: *r,
                        state: *state,
                    },
                );
            }
        }
    }
    result.into_values().collect()
}
unsafe fn indexed(p: usize, offset: usize, index: usize) -> usize {
    std::mem::transmute::<usize, unsafe extern "C" fn(usize, usize) -> usize>(read(
        read(p) + offset,
    ))(p, index)
}
// Same recipient rules as ammunition's single-squad panel: live members of
// this squad; a partial individual selection takes precedence over the squad.
// A unit without a squad roster (a vehicle: a plain AiFacet whose gunners are
// its turrets) is one user, with no individual pins (facet 0).
unsafe fn members(ai: usize, parent: usize) -> Option<Vec<(usize, usize)>> {
    let roster = get(ai, offsets().roster);
    if roster == 0 {
        return Some(vec![(ai, 0)]);
    }
    let header = get(roster, 0x68);
    if header == 0 {
        return None;
    }
    let mut members = Vec::new();
    for at in vector(header, 8, 100_000)? {
        let entity = read(at);
        if entity == 0 {
            continue;
        }
        let facets = get(entity, 0xb0);
        if facets == 0 {
            continue;
        }
        let facet = read(facets + 0x50);
        if facet == 0 || *((facet + 0x18) as *const u8) == 0 || read(facet + 0x28) != parent {
            continue;
        }
        members.push((read(facets + 0x28), facet));
    }
    let marked = members
        .iter()
        .filter(|(_, f)| get(*f, 0x58) as u8 != 0)
        .count();
    if marked > 0 && marked < members.len() {
        members.retain(|(_, f)| get(*f, 0x58) as u8 != 0);
    }
    Some(members)
}
unsafe fn guns(ai: usize) -> Vec<usize> {
    let mut result = Vec::new();
    if ai == 0 {
        return result;
    }
    for index in 0..get(ai, offsets().gunner_count).min(1024) {
        let gunner = indexed(ai, offsets().gunner_get, index);
        if gunner == 0 {
            continue;
        }
        for index in 0..get(gunner, 0xf0).min(1024) {
            let gun = indexed(gunner, 0xf8, index);
            if gun != 0 {
                result.push(gun);
            }
        }
    }
    result
}
unsafe fn state(record: &[u64; 9], index: usize, users: &[(Vec<usize>, usize)]) -> State {
    let mut state = State::default();
    for (guns, facet) in users {
        let mut usable = false;
        let mut disabled = field(record, 0x3c) != 0;
        if index < 8
            && *facet != 0
            && *((facet + 0x19) as *const u8) == 0xa5
            && *((facet + 0x1e) as *const u8) & (1 << index) != 0
        {
            disabled = *((facet + 0x1f) as *const u8) & (1 << index) != 0;
        }
        let mut progress = Vec::new();
        for &gun in guns {
            // Compatibility includes alternative ammo; reload belongs only to
            // the ammo currently loaded into this gun (native vt+158).
            usable |= indexed(gun, 0x148, record[0] as usize) as u8 != 0;
            if get(gun, 0x158) != record[0] as usize {
                continue;
            }
            progress.push(std::mem::transmute::<
                usize,
                unsafe extern "C" fn(usize) -> f32,
            >(read(read(gun) + 0xc8))(gun));
        }
        if usable {
            state.total += 1;
            if !disabled {
                state.enabled += 1;
                state.ready += readiness(progress);
            }
        }
    }
    state
}
unsafe fn sources(menu: usize) -> Option<Vec<Source>> {
    // AmmunitionMenu's constructor (game+3df21) copies the server context
    // to +128 and LogicUtils world facade to +130. Other UI classes use the
    // opposite order. Calling the facade's +40 as a player getter enters an
    // unrelated multi-argument utility and crashes on the first redraw.
    let context = read(menu + 0x128);
    let world = read(menu + 0x130);
    if world == 0 || context == 0 {
        return None;
    }
    let player = get(context, 0x40);
    if player == 0 {
        return None;
    }
    let manager = std::mem::transmute::<usize, unsafe extern "C" fn(usize, usize) -> usize>(read(
        read(world) + offsets().world_player,
    ))(world, player);
    if manager == 0 {
        return None;
    }
    let select = SELECT.get()?;
    let mut entities = BTreeSet::new();
    for at in vector(manager + 0x40, 8, 100_000)? {
        let mut e = read(at);
        if e == 0 {
            continue;
        }
        let f = get(e, 0xb0);
        if f == 0 {
            continue;
        }
        let facet = read(f + 0x50);
        if facet == 0 || (select.is_selected)(facet as *mut _) == 0 {
            continue;
        }
        // A soldier stands for his squad. Any other selected unit (a squad or
        // a vehicle) is its own source; the ownership, AI, pool and gun checks
        // below leave out anything without ammunition of its own.
        if kind(e, 0x20) {
            let parent = read(facet + 0x28);
            if parent == 0 {
                continue;
            }
            let weak = read(parent + 0x10);
            if weak == 0 {
                continue;
            }
            e = read(weak + 0x10);
        }
        if e != 0 {
            entities.insert(e);
        }
    }
    let mut result = Vec::new();
    for e in entities {
        let facets = get(e, 0xb0);
        if facets == 0 {
            continue;
        }
        let team = read(facets + 0x20);
        if team == 0 || get(team, 0x80) as u8 == 0 {
            continue;
        }
        let ai = read(facets + 0x28);
        if ai == 0 {
            continue;
        }
        let pool = get(ai, offsets().pool_get);
        if pool == 0 {
            continue;
        }
        let header = get(pool, 0x48);
        if header == 0 {
            continue;
        }
        let users: Vec<_> = members(ai, read(facets + 0x50))?
            .into_iter()
            .map(|(ai, facet)| (guns(ai), facet))
            .collect();
        // Only a source with guns has ammunition of its own to show.
        if users.iter().all(|(guns, _)| guns.is_empty()) {
            continue;
        }
        let mut records = Vec::new();
        let mut states = Vec::new();
        for at in vector(header, 0x48, 128)? {
            let mut record = *(at as *const [u64; 9]);
            // Count selected compatible users, including partial-squad pins.
            let index = records.len();
            let state = state(&record, index, &users);
            set_field(&mut record, 0x3c, u32::from(state.enabled == 0));
            set_field(&mut record, 0x34, state.total);
            records.push(record);
            states.push(state);
        }
        result.push(Source {
            entity: e,
            ai,
            records,
            states,
        });
    }
    Some(result)
}
#[derive(Default)]
struct View {
    menu: usize,
    entities: Vec<usize>,
    types: Vec<u64>,
}
thread_local! { static VIEW: RefCell<View> = RefCell::new(View::default()); }
pub unsafe fn configure(base: usize, rvas: &[(usize, &[u8])]) -> Result<(), String> {
    let selection =
        defiance_feature_sdk::services::selection().ok_or("selection service unavailable")?;
    SELECT.get_or_init(|| selection);
    FUNCTIONS.get_or_init(|| std::array::from_fn(|i| base + rvas[i].0));
    Ok(())
}
unsafe fn visibility(widget: usize, value: u8) {
    if widget != 0 {
        std::mem::transmute::<usize, unsafe extern "C" fn(usize, u8)>(read(read(widget) + 0x48))(
            widget, value,
        );
    }
}
unsafe fn progress(widget: usize, value: f32, functions: &[usize; 8]) {
    if widget == 0 {
        return;
    }
    let value = value.clamp(0.0, 1.0);
    let stored = (widget + 0x1a8) as *mut f32;
    if *stored != value {
        *stored = value;
        std::mem::transmute::<usize, unsafe extern "C" fn(usize)>(functions[7])(widget);
    }
    visibility(widget, 1);
}
unsafe fn decorate(slot: usize, card: &Card, functions: &[usize; 8]) {
    let state = card.state;
    // fillSlot sets the quantity colour, so a mixed card keeps the native one.
    if state.mixed() {
        *((slot + 8) as *mut u32) = 1; // Like single-squad: next click enables all.
    }
    let label = read(slot + 0x40);
    if label != 0 {
        let text = state.label();
        // Borrowed MSVC long-string layout. Setter copies; it never owns text.
        let string = [text.as_ptr() as usize, 0, text.len(), text.len().max(16)];
        std::mem::transmute::<usize, Pair>(functions[6])(label, string.as_ptr() as usize);
    }
    progress(read(slot + 0x48), state.reload_share(), functions);
    let capacity = field(&card.record, 0x28);
    let fraction = if capacity == 0 {
        0.0
    } else {
        field(&card.record, 0x2c) as f32 / capacity as f32
    };
    progress(read(slot + 0x50), fraction, functions);
}
pub unsafe fn render(menu: usize, count: usize) {
    VIEW.with(|v| *v.borrow_mut() = View::default());
    let Some(sources) = sources(menu) else {
        return;
    };
    if sources.len() < 2 {
        return;
    }
    let Some(functions) = FUNCTIONS.get() else {
        return;
    };
    let cards = cards(&sources);
    for index in 0..count {
        let slot = menu + 0x180 + index * 0xb8;
        if let Some(card) = cards.get(index) {
            // Helpers change only this card's widgets. Never rewrite game pools.
            std::mem::transmute::<usize, Get>(functions[3])(slot);
            std::mem::transmute::<usize, Fill>(functions[1])(
                menu,
                index,
                card.record.as_ptr() as usize,
            );
            if field(&card.record, 0x3c) == 0 {
                std::mem::transmute::<usize, Get>(functions[5])(slot);
            } else {
                std::mem::transmute::<usize, Get>(functions[4])(slot);
            }
            decorate(slot, card, functions);
        } else {
            std::mem::transmute::<usize, Pair>(functions[2])(menu, index);
        }
    }
    VIEW.with(|v| {
        *v.borrow_mut() = View {
            menu,
            entities: sources.iter().map(|s| s.entity).collect(),
            types: cards.iter().take(count).map(|c| c.record[0]).collect(),
        }
    });
}
pub unsafe extern "C" fn click(menu: usize, widget: usize) {
    let cached = VIEW.with(|v| {
        let v = v.borrow();
        (v.menu, v.entities.clone(), v.types.clone())
    });
    if cached.0 != menu {
        std::mem::transmute::<usize, Pair>(CLICK.load(Ordering::Relaxed))(menu, widget);
        return;
    }
    let index = (0..cached.2.len()).find(|i| {
        let slot = menu + 0x180 + i * 0xb8;
        [0x18, 0x20, 0x28, 0x30, 0x38]
            .iter()
            .any(|&at| read(slot + at) == widget)
    });
    let Some(index) = index else {
        return;
    };
    // Never apply a stale displayed type to a newly selected set of squads.
    let Some(current) = sources(menu) else {
        return;
    };
    if current.iter().map(|s| s.entity).collect::<Vec<_>>() != cached.1 {
        return;
    }
    let ammo = cached.2[index];
    let targets: Vec<_> = current
        .iter()
        .flat_map(|s| {
            s.records
                .iter()
                .enumerate()
                .filter(move |(_, r)| r[0] == ammo && field(r, 0x34) > 0)
                .map(move |(i, r)| (s.ai, i, field(r, 0x3c)))
        })
        .collect();
    let combined = cards(&current);
    let Some(card) = combined.iter().find(|card| card.record[0] == ammo) else {
        return;
    };
    let disable = u32::from(card.state.enabled == card.state.total);
    for (ai, index, _) in targets {
        std::mem::transmute::<usize, Set>(read(read(ai) + offsets().ai_set))(ai, index, disable);
    }
    // Request the ordinary menu refresh; do not retain record/string pointers.
    std::mem::transmute::<usize, unsafe extern "C" fn(usize, f32)>(read(read(menu) + 0x38))(
        menu, 0.0,
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_is_the_lowest_reload_or_ready() {
        assert_eq!(readiness([]), 1.0);
        assert_eq!(readiness([1.0, f32::NAN, f32::INFINITY, -1.0, 2.0]), 1.0);
        assert_eq!(readiness([0.75, 1.0, 0.25]), 0.25);
        assert_eq!(readiness([0.0]), 0.0);
    }
    #[test]
    fn reload_share_counts_disabled_users_as_empty() {
        let at_rest = State {
            enabled: 2,
            total: 3,
            ready: 2.0,
        };
        assert!((at_rest.reload_share() - 2.0 / 3.0).abs() < 1e-6);
        let mut all = State {
            enabled: 3,
            total: 3,
            ready: 3.0,
        };
        assert_eq!(all.reload_share(), 1.0);
        all.merge(State {
            enabled: 1,
            total: 1,
            ready: 0.5,
        });
        assert_eq!(all.reload_share(), 3.5 / 4.0);
        assert_eq!(State::default().reload_share(), 0.0);
    }
    fn record(id: u64, capacity: u32, rounds: u32, disabled: u32) -> [u64; 9] {
        let mut r = [0; 9];
        r[0] = id;
        for (at, v) in [
            (0x28, capacity),
            (0x2c, rounds),
            (0x34, 1),
            (0x3c, disabled),
        ] {
            set_field(&mut r, at, v);
        }
        r
    }
    #[test]
    fn union_merges_identity_not_local_slot_and_handles_mixed_state() {
        let sources = vec![
            Source {
                entity: 1,
                ai: 11,
                records: vec![record(100, 10, 8, 0), record(200, 20, 9, 1)],
                states: vec![
                    State {
                        enabled: 1,
                        total: 1,
                        ..State::default()
                    },
                    State {
                        total: 1,
                        ..State::default()
                    },
                ],
            },
            Source {
                entity: 2,
                ai: 22,
                records: vec![record(300, 5, 2, 0), record(100, 10, 3, 1)],
                states: vec![
                    State {
                        enabled: 1,
                        total: 1,
                        ..State::default()
                    },
                    State {
                        total: 1,
                        ..State::default()
                    },
                ],
            },
        ];
        let cards = cards(&sources);
        assert_eq!(
            cards.iter().map(|c| c.record[0]).collect::<Vec<_>>(),
            vec![100, 200, 300]
        );
        assert_eq!(field(&cards[0].record, 0x2c), 11);
        assert_eq!(field(&cards[0].record, 0x28), 20);
        assert_eq!(field(&cards[0].record, 0x34), 2);
        assert_eq!(field(&cards[0].record, 0x3c), 0);
        assert_eq!(field(&cards[1].record, 0x3c), 1);
        assert_eq!(cards[0].state.label(), "1/2");
        assert_eq!(cards[1].state.label(), "1");
    }
}
