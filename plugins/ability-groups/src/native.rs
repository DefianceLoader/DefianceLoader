//! The hooks and the per-bar state.
//!
//! The game menu's update asks, each frame, which of the 15 abilities the
//! selected squads have, and tells the ability bar through
//! [`update`] (`game+1cec0`, GOG 2026-09-25) in enum order: (bar, ability,
//! available, the first squad with it). Each ability has a window (a button)
//! in the bar's map, and the window's `+0x144` names its slot. The stock
//! function shows an available ability in its slot, and when a second one of
//! the same slot is available it sets the slot's conflict flag and empties
//! the slot until [`reset`] (`game+1cdf0`, on a selection change).
//!
//! [`update`] never lets that happen: it keeps, for each slot, the abilities
//! available last frame, and passes an ability on as available only when it
//! is the one the slot should show (the one picked last, else the first).
//! A slot with two or more is a group. Its key ([`key`], `game+1fd40`) or a
//! click on its button ([`click`], `game+2b0d30`, every GUI button's click)
//! opens the row: the order panel is put away as the mine button does it
//! (a stock submenu closed first, then the three calls and the two game menu
//! fields of [`crate::Build`]), each of the group's windows is shown and
//! moved from the slot to a cell of the order panel ([`CELLS`]), and the order key label of that cell is shown.
//! The order keys reach [`order_key`] (`game+23d900`, one call per order
//! button bound to the key), which picks the entry under the key; a click
//! on an entry picks it too. Picking closes the row, makes the entry the
//! slot's ability and activates it as the ability key would (for a key, at
//! the next frame, once the key has reached every binding). The group's key
//! again, another slot's key, any other button or the game clearing the
//! submenu (a right click, Escape) closes the row without picking. [`reset`] and the bar's destructor ([`bar_destroy`],
//! `game+1acd0`) put the windows back and forget the row.
//!
//! All of it runs on the game's main thread. No state borrow is held across
//! a call into the game, which may call these hooks again.
use crate::Build;
use core::ffi::c_void;
use defiance_api::{Api, LOG_DEBUG};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

/// Ability ids 0..15 (the `string_view` table at `game+4f16c0`).
pub const ABILITIES: usize = 15;
/// The bar's slots.
pub const SLOTS: usize = 4;
/// A slot record's ability when the slot shows none.
const EMPTY: i32 = 0x10;
/// The order panel cells the row's entries cover, left to right: the row
/// starts at the panel's left edge with the bar's pitch, and each entry takes
/// the key of the cell under its centre (Q, R, F, G with the default keys).
pub const CELLS: [usize; SLOTS] = [0, 2, 3, 5];
/// The order panel's cells (the game menu's `Game.slot0..11` keys).
const ORDER_CELLS: usize = 12;

// The bar.
const BAR_MAP_END: usize = 0x10;
const BAR_MAP_BUCKETS: usize = 0x20;
const BAR_MAP_MASK: usize = 0x38;
const BAR_GAME_MENU: usize = 0x50;
const BAR_RECORDS: usize = 0x88;
const RECORD_SIZE: usize = 0x18;
const RECORD_CONFLICT: usize = 8;
// A map node: previous at +8, the ability at +0x10, the window at +0x18.
const NODE_PREVIOUS: usize = 8;
const NODE_KEY: usize = 0x10;
const NODE_WINDOW: usize = 0x18;
// An ability window.
const WINDOW_ROOT: usize = 0x128;
const WINDOW_BUTTON: usize = 0x130;
/// The second button of a two-state window (Pack, Hacking, Sticky).
const WINDOW_SECOND_BUTTON: usize = 0x238;
const WINDOW_SLOT: usize = 0x144;
const TWO_STATE: [u32; 3] = [7, 8, 9];
/// A widget's rectangle getter: `int[4]` (left, top, right, bottom).
const WIDGET_RECT: usize = 0x40;
/// An order button's key binding: the name it is registered under
/// (`slot<cell>`, a `std::string`).
const ORDER_INFO_NAME: usize = 0x20;
/// The submenu state while one is open.
const SUBMENU_OPEN: i32 = 3;
/// The layout's bar pitch and the drop from the bar to the row, per the
/// ability window's 76 units (`AbilitiesWidget.txt`, `GameMenu.txt`).
const WINDOW_UNITS: i32 = 76;
const PITCH_UNITS: i32 = 72;
const DROP_UNITS: i32 = 96;

type Update = unsafe extern "C" fn(usize, u32, u8, usize);
type Bar = unsafe extern "C" fn(usize);
type BarDestroy = unsafe extern "C" fn(usize) -> usize;
type Key = unsafe extern "C" fn(usize, *const u8) -> u64;
type OrderKey = unsafe extern "C" fn(usize, u8) -> u64;
type Click = unsafe extern "C" fn(usize, usize, usize);
type Hide = unsafe extern "C" fn(usize, u32);
type MoveWidget = unsafe extern "C" fn(usize, *const [i32; 2]);
type Label = unsafe extern "C" fn(usize, *const MsvcString, u8);
type Menu = unsafe extern "C" fn(usize);
type Log = unsafe extern "C" fn(u32, *const core::ffi::c_char);

#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(key: i32) -> i16;
}
const VK_LBUTTON: i32 = 1;

struct Game {
    hide: Hide,
    move_widget: MoveWidget,
    label: Label,
    close_submenus: Bar,
    clear_order: Menu,
    order_reset: Menu,
    hide_orders: Menu,
    orders_hidden: usize,
    submenu: usize,
    log: Log,
}

static GAME: OnceLock<Game> = OnceLock::new();
static UPDATE: AtomicUsize = AtomicUsize::new(0);
static RESET: AtomicUsize = AtomicUsize::new(0);
static KEY: AtomicUsize = AtomicUsize::new(0);
static BAR_DESTROY: AtomicUsize = AtomicUsize::new(0);
static ORDER_KEY: AtomicUsize = AtomicUsize::new(0);
static CLICK: AtomicUsize = AtomicUsize::new(0);

fn game() -> &'static Game {
    GAME.get().expect("hooks run after install")
}

unsafe fn original<T: Copy>(slot: &AtomicUsize) -> T {
    let address = slot.load(Ordering::Acquire);
    core::mem::transmute_copy(&address)
}

unsafe fn log(text: &str) {
    if let Ok(text) = std::ffi::CString::new(text) {
        (game().log)(LOG_DEBUG, text.as_ptr());
    }
}

/// An MSVC `std::string` holding a short text in place.
#[repr(C)]
pub struct MsvcString {
    text: [u8; 16],
    length: usize,
    capacity: usize,
}

impl MsvcString {
    fn short(text: &str) -> Self {
        assert!(text.len() < 16);
        let mut buffer = [0; 16];
        buffer[..text.len()].copy_from_slice(text.as_bytes());
        Self {
            text: buffer,
            length: text.len(),
            capacity: 15,
        }
    }
}

unsafe fn q(address: usize) -> usize {
    *(address as *const usize)
}

unsafe fn d(address: usize) -> i32 {
    *(address as *const i32)
}

/// The text of the `std::string` at `address`.
unsafe fn msvc_str<'a>(address: usize) -> &'a [u8] {
    let length = q(address + 0x10);
    let data = if q(address + 0x18) > 15 {
        q(address) as *const u8
    } else {
        address as *const u8
    };
    core::slice::from_raw_parts(data, length)
}

/// FNV-1a over the id's four bytes, as the bar's map hashes it.
pub fn hash(ability: u32) -> u64 {
    ability
        .to_le_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| {
            (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
        })
}

/// The ability's window, or 0: a lookup in the bar's `unordered_map`.
unsafe fn window_of(bar: usize, ability: u32) -> usize {
    let end = q(bar + BAR_MAP_END);
    let bucket = q(bar + BAR_MAP_BUCKETS) + (hash(ability) as usize & q(bar + BAR_MAP_MASK)) * 0x10;
    let first = q(bucket);
    let mut node = q(bucket + 8);
    if node == end || node == 0 {
        return 0;
    }
    loop {
        if d(node + NODE_KEY) as u32 == ability {
            return q(node + NODE_WINDOW);
        }
        if node == first {
            return 0;
        }
        node = q(node + NODE_PREVIOUS);
    }
}

fn record(bar: usize, slot: usize) -> usize {
    bar + BAR_RECORDS + slot * RECORD_SIZE
}

unsafe fn shown(bar: usize, slot: usize) -> i32 {
    d(record(bar, slot))
}

unsafe fn set_shown(bar: usize, slot: usize, ability: i32) {
    *(record(bar, slot) as *mut i32) = ability;
    *((record(bar, slot) + RECORD_CONFLICT) as *mut u8) = 0;
}

unsafe fn rect(widget: usize) -> [i32; 4] {
    let getter: unsafe extern "C" fn(usize) -> *const [i32; 4] =
        core::mem::transmute(q(q(widget) + WIDGET_RECT));
    *getter(widget)
}

/// The window's buttons: one, or two for a two-state window.
unsafe fn buttons(window: usize) -> [usize; 2] {
    let ability = d(window + 0x120) as u32;
    let second = if TWO_STATE.contains(&ability) {
        q(window + WINDOW_SECOND_BUTTON)
    } else {
        0
    };
    [q(window + WINDOW_BUTTON), second]
}

/// The ability a slot should show: the one picked last if it is still
/// available, else the first; none when no ability of the slot is.
pub fn desired(available: u32, picked: Option<u8>) -> Option<u32> {
    match picked {
        Some(p) if available & (1 << p) != 0 => Some(p as u32),
        _ if available == 0 => None,
        _ => Some(available.trailing_zeros()),
    }
}

/// The row's abilities, in enum order, one per cell.
pub fn entries(available: u32) -> Vec<u32> {
    (0..ABILITIES as u32)
        .filter(|a| available & (1 << a) != 0)
        .take(CELLS.len())
        .collect()
}

/// The order cell a key binding's name (`slot<cell>`) names.
pub fn cell_of(name: &[u8]) -> Option<usize> {
    let digits = name.strip_prefix(b"slot")?;
    let cell: usize = std::str::from_utf8(digits).ok()?.parse().ok()?;
    (cell < ORDER_CELLS).then_some(cell)
}

struct Entry {
    ability: u32,
    window: usize,
    moved: [i32; 2],
    cell: usize,
}

struct Row {
    slot: usize,
    available: u32,
    entries: Vec<Entry>,
    /// Cells whose key went down while the row was open.
    pressed: u16,
}

#[derive(Default)]
struct State {
    bar: usize,
    /// Each slot's available abilities this frame, and last frame.
    now: [u32; SLOTS],
    last: [u32; SLOTS],
    actor: [usize; ABILITIES],
    picked: [Option<u8>; SLOTS],
    row: Option<Row>,
    /// A slot picked by key, activated at the next frame: the key's release
    /// still goes on to the other bindings of its cell, and one of them may
    /// belong to the submenu the activation opens (Mine's).
    activate: Option<usize>,
}

impl State {
    fn group(&self, slot: usize) -> bool {
        self.last[slot].count_ones() >= 2
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    /// Set while the plugin activates an entry, so [`click`] passes it on.
    static ACTIVATING: Cell<bool> = const { Cell::new(false) };
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

unsafe fn show_label(game_menu: usize, cell: usize, visible: bool) {
    let name = MsvcString::short(&format!("slot{cell}_hotkey"));
    (game().label)(game_menu, &name, visible as u8);
}

/// Opens the row for `slot`, whose button shows one of its abilities.
unsafe fn open(bar: usize, slot: usize) {
    let (available, actors) = with(|s| (s.last[slot], s.actor));
    let leader = shown(bar, slot);
    let home = window_of(bar, leader as u32);
    if leader == EMPTY || home == 0 {
        return;
    }
    let [left, top, right, bottom] = rect(q(home + WINDOW_ROOT));
    let pitch = (right - left) * PITCH_UNITS / WINDOW_UNITS;
    let drop = (bottom - top) * DROP_UNITS / WINDOW_UNITS;
    let start = left - slot as i32 * pitch;
    let g = game();
    let game_menu = q(bar + BAR_GAME_MENU);
    // A stock submenu opened from the row (Mine's) goes first.
    (g.close_submenus)(bar);
    (g.clear_order)(game_menu);
    (g.order_reset)(game_menu);
    *((game_menu + g.orders_hidden) as *mut u8) = 1;
    (g.hide_orders)(game_menu);
    *((game_menu + g.submenu) as *mut i32) = SUBMENU_OPEN;
    let update: Update = original(&UPDATE);
    let mut row = Row {
        slot,
        available,
        entries: Vec::new(),
        pressed: 0,
    };
    for (k, ability) in entries(available).into_iter().enumerate() {
        let window = window_of(bar, ability);
        if window == 0 {
            continue;
        }
        if ability as i32 != leader {
            // Shown as the slot's own would be, then the slot's record back.
            set_shown(bar, slot, EMPTY);
            update(bar, ability, 1, actors[ability as usize]);
            set_shown(bar, slot, leader);
        }
        let root = q(window + WINDOW_ROOT);
        let [x, y, ..] = rect(root);
        let moved = [start + k as i32 * pitch - x, top + drop - y];
        (g.move_widget)(root, &moved);
        show_label(game_menu, CELLS[k], true);
        row.entries.push(Entry {
            ability,
            window,
            moved,
            cell: CELLS[k],
        });
    }
    log(&format!(
        "ability row opened for slot {slot}: abilities {:?}",
        row.entries.iter().map(|e| e.ability).collect::<Vec<_>>()
    ));
    with(|s| s.row = Some(row));
}

/// Puts the row's windows back in the slot and hides its labels.
unsafe fn restore(bar: usize, row: &Row, labels: bool) {
    let game_menu = q(bar + BAR_GAME_MENU);
    for entry in &row.entries {
        let back = [-entry.moved[0], -entry.moved[1]];
        (game().move_widget)(q(entry.window + WINDOW_ROOT), &back);
        if labels {
            show_label(game_menu, entry.cell, false);
        }
    }
}

/// Closes the row; with `pick`, the slot shows that entry from now on.
unsafe fn close(bar: usize, pick: Option<u32>) {
    close_with(bar, pick, true);
}

/// [`close`], hiding the row's key labels only with `labels`: when the game
/// has cleared the submenu itself, the order panel is back and owns them.
unsafe fn close_with(bar: usize, pick: Option<u32>, labels: bool) {
    let Some(row) = with(|s| s.row.take()) else {
        return;
    };
    restore(bar, &row, labels);
    let leader = pick.unwrap_or(shown(bar, row.slot) as u32);
    for entry in &row.entries {
        if entry.ability != leader {
            (game().hide)(bar, entry.ability);
        }
    }
    set_shown(bar, row.slot, leader as i32);
    let game_menu = q(bar + BAR_GAME_MENU);
    let submenu = (game_menu + game().submenu) as *mut i32;
    if *submenu == SUBMENU_OPEN {
        // The frame update lifts the order panel's hidden flag.
        *submenu = 0;
    }
    if let Some(pick) = pick {
        with(|s| s.picked[row.slot] = Some(pick as u8));
    }
    log(&format!(
        "ability row closed for slot {}: {}{}",
        row.slot,
        pick.map_or("nothing picked".into(), |p| format!("picked {p}")),
        if labels {
            ""
        } else {
            " (the game cleared the submenu)"
        }
    ));
}

/// Activates the slot's ability as its key does on release.
unsafe fn activate(bar: usize, slot: usize) {
    let binding: [usize; 3] = [0, bar, slot];
    let released = 0u8;
    ACTIVATING.set(true);
    original::<Key>(&KEY)(binding.as_ptr() as usize, &released);
    ACTIVATING.set(false);
}

pub unsafe extern "C" fn update(bar: usize, ability: u32, available: u8, actor: usize) {
    let stock: Update = original(&UPDATE);
    let index = ability as usize;
    if index == 0 {
        let pending = with(|s| {
            if s.bar == bar {
                s.activate.take()
            } else {
                None
            }
        });
        if let Some(slot) = pending {
            activate(bar, slot);
        }
    }
    let window = if index < ABILITIES {
        window_of(bar, ability)
    } else {
        0
    };
    let slot = if window != 0 {
        d(window + WINDOW_SLOT)
    } else {
        -1
    };
    enum Then {
        Stock(u8),
        Replace(i32),
        Skip,
        Close,
        Cleared,
        Hold,
    }
    let then = with(|s| {
        if s.bar != bar {
            *s = State {
                bar,
                ..State::default()
            };
        }
        if index == 0 {
            s.last = s.now;
            s.now = [0; SLOTS];
        }
        let Ok(slot) = usize::try_from(slot) else {
            return Then::Stock(available);
        };
        if slot >= SLOTS {
            return Then::Stock(available);
        }
        if available != 0 {
            s.now[slot] |= 1 << ability;
            s.actor[index] = actor;
        }
        if let Some(row) = &s.row {
            // Frozen while open; closed when its abilities change, or when
            // the game clears the submenu itself (a right click, Escape, any
            // left press). A left press may be a click on an entry, which
            // arrives on release: held open until then.
            let submenu = unsafe { d(q(bar + BAR_GAME_MENU) + game().submenu) };
            if submenu != SUBMENU_OPEN {
                let left = unsafe { GetAsyncKeyState(VK_LBUTTON) } as u16 & 0x8000 != 0;
                return if left { Then::Hold } else { Then::Cleared };
            }
            let changed = index == ABILITIES - 1 && s.now[row.slot] != row.available;
            return if changed { Then::Close } else { Then::Skip };
        }
        if available == 0 {
            return Then::Stock(0);
        }
        let current = unsafe { shown(bar, slot) };
        match desired(s.last[slot], s.picked[slot]) {
            None if current == EMPTY => Then::Stock(1),
            None => Then::Stock(0),
            Some(d) if d == ability && current != EMPTY && current != ability as i32 => {
                Then::Replace(current)
            }
            Some(d) if d == ability => Then::Stock(1),
            Some(_) => Then::Stock(0),
        }
    });
    if let Ok(slot) = usize::try_from(slot) {
        if slot < SLOTS {
            *((record(bar, slot) + RECORD_CONFLICT) as *mut u8) = 0;
        }
    }
    match then {
        Then::Stock(available) => stock(bar, ability, available, actor),
        Then::Replace(current) => {
            stock(bar, current as u32, 0, 0);
            stock(bar, ability, 1, actor);
        }
        Then::Skip => {}
        Then::Close => close(bar, None),
        Then::Cleared => {
            close_with(bar, None, false);
            // This frame's call for the ability, as if the row were closed.
            update(bar, ability, available, actor);
        }
        Then::Hold => {
            let g = game();
            let game_menu = q(bar + BAR_GAME_MENU);
            *((game_menu + g.submenu) as *mut i32) = SUBMENU_OPEN;
            let hidden = (game_menu + g.orders_hidden) as *mut u8;
            if *hidden == 0 {
                *hidden = 1;
                (g.hide_orders)(game_menu);
            }
        }
    }
}

pub unsafe extern "C" fn key(binding: usize, pressed: *const u8) -> u64 {
    let stock: Key = original(&KEY);
    let bar = q(binding + 8);
    let slot = d(binding + 0x10);
    let released = *pressed == 0;
    enum Then {
        Stock,
        Swallow,
        Open,
        Close,
        CloseThenStock,
    }
    let then = with(|s| {
        let Ok(slot) = usize::try_from(slot) else {
            return Then::Stock;
        };
        if s.bar != bar || slot >= SLOTS {
            return Then::Stock;
        }
        match &s.row {
            Some(row) if row.slot == slot => {
                if released {
                    Then::Close
                } else {
                    Then::Swallow
                }
            }
            Some(_) if released => Then::CloseThenStock,
            Some(_) => Then::Stock,
            None if s.group(slot) => {
                if released {
                    Then::Open
                } else {
                    Then::Swallow
                }
            }
            None => Then::Stock,
        }
    });
    match then {
        Then::Stock => stock(binding, pressed),
        Then::Swallow => 0,
        Then::Open => {
            open(bar, slot as usize);
            1
        }
        Then::Close => {
            close(bar, None);
            1
        }
        Then::CloseThenStock => {
            close(bar, None);
            stock(binding, pressed)
        }
    }
}

pub unsafe extern "C" fn click(button: usize, target: usize, event: usize) {
    let stock: Click = original(&CLICK);
    if ACTIVATING.get() || button == 0 {
        return stock(button, target, event);
    }
    enum Then {
        Stock,
        Open(usize),
        Pick(u32),
        Close,
    }
    let (bar, then) = with(|s| {
        let bar = s.bar;
        if bar == 0 {
            return (bar, Then::Stock);
        }
        if let Some(row) = &s.row {
            let entry = row
                .entries
                .iter()
                .find(|e| unsafe { buttons(e.window) }.contains(&button));
            return (bar, entry.map_or(Then::Close, |e| Then::Pick(e.ability)));
        }
        for slot in 0..SLOTS {
            if !s.group(slot) {
                continue;
            }
            let leader = unsafe { shown(bar, slot) };
            if leader == EMPTY {
                continue;
            }
            let window = unsafe { window_of(bar, leader as u32) };
            if window != 0 && unsafe { buttons(window) }.contains(&button) {
                return (bar, Then::Open(slot));
            }
        }
        (bar, Then::Stock)
    });
    match then {
        Then::Stock => stock(button, target, event),
        Then::Open(slot) => open(bar, slot),
        Then::Pick(ability) => {
            close(bar, Some(ability));
            stock(button, target, event);
        }
        Then::Close => {
            close(bar, None);
            stock(button, target, event);
        }
    }
}

pub unsafe extern "C" fn order_key(info: usize, pressed: u8) -> u64 {
    let stock: OrderKey = original(&ORDER_KEY);
    let cell = cell_of(msvc_str(info + ORDER_INFO_NAME));
    enum Then {
        Stock,
        Swallow,
        Pick(usize, u32),
    }
    let then = with(|s| {
        let (Some(row), Some(cell)) = (s.row.as_mut(), cell) else {
            return Then::Stock;
        };
        let Some(entry) = row.entries.iter().find(|e| e.cell == cell) else {
            return Then::Stock;
        };
        let bit = 1 << cell;
        if pressed != 0 {
            row.pressed |= bit;
            Then::Swallow
        } else if row.pressed & bit != 0 {
            // One key reaches every order button bound to it: act once.
            row.pressed &= !bit;
            Then::Pick(row.slot, entry.ability)
        } else {
            Then::Swallow
        }
    });
    match then {
        Then::Stock => stock(info, pressed),
        Then::Swallow => 1,
        Then::Pick(slot, ability) => {
            let bar = with(|s| s.bar);
            close(bar, Some(ability));
            with(|s| s.activate = Some(slot));
            1
        }
    }
}

pub unsafe extern "C" fn reset(bar: usize) {
    let row = with(|s| {
        if s.bar != bar {
            return None;
        }
        s.activate = None;
        s.row.take()
    });
    if let Some(row) = row {
        // The stock reset hides every window and empties the slots after.
        restore(bar, &row, true);
        log("ability row closed by a selection change");
    }
    original::<Bar>(&RESET)(bar)
}

pub unsafe extern "C" fn bar_destroy(bar: usize) -> usize {
    let row = with(|s| {
        if s.bar != bar {
            return None;
        }
        let row = s.row.take();
        *s = State::default();
        row
    });
    if let Some(row) = row {
        // The game menu may already be going; its labels go with it.
        restore(bar, &row, false);
    }
    original::<BarDestroy>(&BAR_DESTROY)(bar)
}

pub unsafe fn install(api: &Api, base: usize, build: &'static Build) -> Result<(), String> {
    let function = |site: &crate::Site| base + site.rva;
    GAME.set(Game {
        hide: core::mem::transmute::<usize, Hide>(function(&build.hide)),
        move_widget: core::mem::transmute::<usize, MoveWidget>(function(&build.move_widget)),
        label: core::mem::transmute::<usize, Label>(function(&build.label)),
        close_submenus: core::mem::transmute::<usize, Bar>(function(&build.close_submenus)),
        clear_order: core::mem::transmute::<usize, Menu>(function(&build.clear_order)),
        order_reset: core::mem::transmute::<usize, Menu>(function(&build.order_reset)),
        hide_orders: core::mem::transmute::<usize, Menu>(function(&build.hide_orders)),
        orders_hidden: build.orders_hidden,
        submenu: build.submenu,
        log: api.log,
    })
    .map_err(|_| "already initialized")?;
    let hooks: [(&crate::Site, *mut c_void, &AtomicUsize, &str); 6] = [
        (
            &build.update,
            update as *mut c_void,
            &UPDATE,
            "ability update",
        ),
        (&build.reset, reset as *mut c_void, &RESET, "ability reset"),
        (&build.key, key as *mut c_void, &KEY, "ability key"),
        (
            &build.bar_destroy,
            bar_destroy as *mut c_void,
            &BAR_DESTROY,
            "ability bar destructor",
        ),
        (
            &build.order_key,
            order_key as *mut c_void,
            &ORDER_KEY,
            "order key",
        ),
        (&build.click, click as *mut c_void, &CLICK, "button click"),
    ];
    let mut applied = Vec::new();
    for (site, detour, original, name) in hooks {
        if (api.hook_exact)(
            function(site) as *mut c_void,
            detour,
            site.before.len(),
            original.as_ptr().cast(),
        ) != 0
        {
            for address in applied.into_iter().rev() {
                (api.unhook)(address);
            }
            return Err(format!(
                "{name} hook refused; host will finish owned rollback"
            ));
        }
        applied.push(function(site) as *mut c_void);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_shows_the_ability_picked_last_while_it_lasts() {
        assert_eq!(desired(0, Some(6)), None);
        assert_eq!(desired(1 << 6 | 1 << 10, None), Some(6));
        assert_eq!(desired(1 << 6 | 1 << 10, Some(10)), Some(10));
        assert_eq!(desired(1 << 6, Some(10)), Some(6));
    }

    #[test]
    fn the_row_takes_one_cell_per_ability_in_enum_order() {
        assert_eq!(entries(1 << 10 | 1 << 6), vec![6, 10]);
        assert_eq!(entries(0b1_1111), vec![0, 1, 2, 3]);
    }

    #[test]
    fn order_keys_name_their_cell() {
        assert_eq!(cell_of(b"slot0"), Some(0));
        assert_eq!(cell_of(b"slot11"), Some(11));
        assert_eq!(cell_of(b"slot12"), None);
        assert_eq!(cell_of(b"slotx"), None);
        assert_eq!(cell_of(b"0"), None);
    }

    #[test]
    fn the_map_hash_is_the_games() {
        // The update's own formula, low byte first.
        let (a, p) = (0x0403_0201_u64, 0x100_0000_01b3_u64);
        let step = |h: u64, b: u64| (h ^ b).wrapping_mul(p);
        let expected = step(
            step(
                step(step(0xcbf2_9ce4_8422_2325, a & 0xff), a >> 8 & 0xff),
                a >> 16 & 0xff,
            ),
            a >> 24 & 0xff,
        );
        assert_eq!(hash(a as u32), expected);
    }

    #[test]
    fn labels_fit_in_place() {
        for cell in 0..ORDER_CELLS {
            let name = format!("slot{cell}_hotkey");
            assert!(name.len() < 16);
            assert_eq!(MsvcString::short(&name).length, name.len());
        }
    }
}
