//! Show allied, neutral and enemy squads' full details in the unit info panel.
//!
//! In a mission the game's unit info panel (`GuiUnitInfoMenu`) shows a squad
//! the player does not own reduced: name, relation label, weapon-type icon and
//! preview. Two ownership tests decide it, both asking the squad's owner
//! (entity vt+0xb0 → `+0x20`) vt+0x80, "is this the player":
//!
//! - the squad sub-panel's setter (`UnitInfoMenuSquad`, GOG 2026-09-14
//!   `game.dll` `fn_3432b0`, [`SQUAD_PATTERN`]) hides the commander's name, the
//!   count, rank and experience when it answers no;
//! - the ammo menu's refresh (`AmmunitionMenu` slot 7, `fn_3fed0`,
//!   [`AMMO_PATTERN`]) drops the shown entity, so its weapons grid stays empty.
//!
//! Each of those calls is replaced by a stub ([`squad_stub`], [`ammo_stub`])
//! that makes the call, and when it answers no, answers yes instead if the
//! squad's relation is one the settings reveal ([`reveal`]). The relation
//! comes from `logic.dll`'s `LogicUtilsImpl`, which the panel holds at
//! `+0x410`: vt+0x628 ally, vt+0x630 enemy, vt+0x638 neutral, vt+0x640
//! abandoned (counted as neutral), as the panel's own relation label asks
//! (`fn_366ad0`, [`LABEL_PATTERN`]).
//!
//! The revealed ammo menu's toggles would act on the other player's squad
//! (the AI keeps what they set), so in the grid's click handler (`fn_3fa40`,
//! [`CLICK_PATTERN`]) its call for the shown entity (`fn_40c20`) answers none
//! for a squad the player does not own, unless it is allied and
//! `ally_weapon_toggles` allows it ([`toggles`]); the handler then writes
//! nothing.
//!
//! The expanded ammo menu changes the same code at startup: it rewrites the
//! menu's layout operands (`+0x820`, `+0x828`, `+0x678`), which the patterns
//! leave as wildcards, and it may take over the click handler's first 15
//! bytes, so [`CLICK_PATTERN`] starts after them and the gate redirects one
//! call inside the handler rather than hooking it. Its combined view takes its
//! squads from the player's selection, never a squad the player does not own.
//! The relation label sits on the commander name's line, which the full panel
//! fills, so it is hidden when a squad is revealed ([`label`]).
//!
//! Each ammo card's reload fill takes the shown squad's colour
//! ([`fill_colour`], [`COLOUR_SETTINGS`]): the player's own teal (the stock
//! colour), allied yellow, neutral grey-blue, enemy red by default. After the menu fills a card (`fillSlot`,
//! `fn_3f370`, [`FILL_PATTERN`]) the fill image's sprite (card `+0x48`
//! progress bar → `+0x1a0` fill → `+0xa8` view → `+0x18` sprite, a `world2.dll`
//! `Sprite`) is given the colour through its colour setter (vt+0xc8), which
//! multiplies the texture. The stock texture is teal, which a colour only
//! darkens, so the unit-inspection companion mod
//! (`tools/package_unit_inspection.py`) replaces it with a greyscale copy of
//! [`GREY_BAR`]'s size; the view records the loaded texture's size (`+0x38`,
//! `+0x3c`), and a card showing any other texture keeps the game's colour.
//! The expanded ammo menu checks `fillSlot`'s first bytes when it starts and
//! then calls it for its combined view; it loads first (plugins start in ID
//! order), and its combined cards are the player's own.
//!
//! The panel offsets are the 2026 builds' (GOG 2026-09-14, Steam 2026-09-22);
//! the relation label's function differs in the 2025 builds, where the plugin
//! finds nothing and changes nothing. Vehicles and buildings keep the game's
//! panel. Not multiplayer-safe: the ally toggles change another player's squad.
use core::ffi::c_void;
use defiance_api::{Api, Plugin, ABI_VERSION, LOG_INFO, LOG_WARN};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicUsize, Ordering};

const ID: &str = "defiance.unit-inspection";

/// In the squad setter: the owner from entity vt+0xb0 → `+0x20`, `call
/// [rax+0x80]`, `sete al`, the flag at `+0x28`, then the panel at `+0x18`
/// and its entity at `+0x3f0`.
pub const SQUAD_PATTERN: &str = "ff 90 b0 00 00 00 48 8b 48 20 48 85 c9 74 11 48 8b 01 \
     ff 90 80 00 00 00 84 c0 0f 94 c0 88 47 28 48 8b 47 18 40 b6 01 48 8b 88 f0 03 00 00";
/// Offset in [`SQUAD_PATTERN`] of its `call [rax+0x80]` (six bytes).
const SQUAD_CALL_AT: usize = 0x12;
/// In the ammo menu's refresh: the shown entity in `rsi`, its owner asked
/// `call [rdx+0x80]`, and dropped (`xor esi, esi`) when it answers no.
pub const AMMO_PATTERN: &str = "48 8b 10 48 8b c8 ff 92 b0 00 00 00 48 8b 48 20 48 85 c9 74 0d \
     48 8b 11 ff 92 80 00 00 00 84 c0 75 02 33 f6 48 8b af ?? ?? ?? ?? 48 8b 9f ?? ?? ?? ??";
/// Offset in [`AMMO_PATTERN`] of its `call [rdx+0x80]` (six bytes).
const AMMO_CALL_AT: usize = 0x18;
/// The ammo grid's click handler from its `sub rsp, 0x40` (its first 20 bytes
/// may be another plugin's hook), through its call to the menu's shown-entity
/// getter (`fn_40c20`).
pub const CLICK_PATTERN: &str = "48 83 ec 40 48 8b f9 45 33 ff 48 8d a9 80 01 00 00 \
     48 8d 85 ?? ?? ?? ?? 48 8b dd 48 3b e8 \
     74 2a 48 39 53 18 74 2d 48 39 53 20 74 1e 48 39 53 30 74 18 48 39 53 38 74 12 48 39 53 28 \
     74 0c 48 81 c3 b8 00 00 00 48 3b d8 75 d6 48 3b d8 0f 84 ?? ?? ?? ?? 48 8b cb e8 ?? ?? ?? ?? \
     84 c0 0f 85 ?? ?? ?? ?? 48 8b f3 48 2b f5 48 c1 fe 03 48 b8 a7 37 bd e9 4d 6f 7a d3 \
     48 0f af f0 8b 6b 08 ff c5 89 6b 08 48 8b cf e8 ?? ?? ?? ??";
/// Offset in [`CLICK_PATTERN`] of its call to the shown-entity getter.
const CLICK_SHOWN_CALL_AT: usize = 0x86;
/// The start of the panel's relation label (`fn_366ad0`): the label at
/// `+0x580` (visible at `+0x5b`, shown by vt+0x48), then `LogicUtilsImpl` at
/// `+0x410` asked vt+0x620, the player's own.
pub const LABEL_PATTERN: &str =
    "48 89 74 24 20 57 48 83 ec 40 48 8b f9 48 8b f2 48 8b 89 80 05 00 00 \
     48 85 c9 74 0e 80 79 5b 00 74 08 48 8b 01 33 d2 ff 50 48 48 8b 8f 10 04 00 00 48 8b d6 \
     48 8b 01 ff 90 20 06 00 00 84 c0 0f 85 ?? ?? ?? ??";
/// The label function's relation calls, at their offsets: ally (`mov r8,
/// [rdx+0x628]`), abandoned, enemy and neutral. They vouch for [`RELATION`].
const LABEL_SLOTS: [(usize, &[u8]); 4] = [
    (0x18a, &[0x4c, 0x8b, 0x82, 0x28, 0x06, 0, 0]),
    (0x1be, &[0xff, 0x90, 0x40, 0x06, 0, 0]),
    (0x1de, &[0xff, 0x90, 0x30, 0x06, 0, 0]),
    (0x1fe, &[0xff, 0x90, 0x38, 0x06, 0, 0]),
];

/// The start of the ammo menu's `fillSlot(menu, index, record)`: the card at
/// `menu + 0x180 + index * 0xb8`.
pub const FILL_PATTERN: &str = "48 89 5c 24 10 48 89 74 24 18 55 57 41 54 41 56 41 57 \
     48 8d ac 24 60 ff ff ff 48 81 ec a0 01 00 00 49 8b f8 48 8b d9 45 33 e4 44 89 a5 d0 00 00 00 \
     48 69 c2 b8 00 00 00 48 8d b1 80 01 00 00 48 03 f0 41 8b 50 3c 85 d2 0f 84 ?? ?? ?? ?? \
     83 fa 01 74 6c";
/// The companion mod's greyscale reload fill, (width, height); the stock one
/// is 88x4. `tools/package_unit_inspection.py` writes it.
const GREY_BAR: (u32, u32) = (88, 8);
const MENU_CARDS: usize = 0x180;
const CARD_SIZE: usize = 0xb8;
const CARD_RELOAD_BAR: usize = 0x48;
const BAR_FILL: usize = 0x1a0;
const WIDGET_VIEW: usize = 0xa8;
const VIEW_SPRITE: usize = 0x18;
const VIEW_TEXTURE_SIZE: usize = 0x38;
const SPRITE_SET_COLOUR: usize = 0xc8;
/// The start of `world2.dll`'s `Sprite` colour setter (vt+0xc8): it reads a
/// BGRA dword through `rdx` into floats, stores them at `+0xb8`, and redraws.
const SET_COLOUR_START: [u8; 19] = [
    0x48, 0x83, 0xec, 0x38, 0x48, 0x8b, 0xc2, 0x4c, 0x8b, 0xc1, 0x48, 0x8b, 0xc8, 0x48, 0x8d, 0x54,
    0x24, 0x20, 0xe8,
];

const SUB_PANEL: usize = 0x18;
const PANEL_ENTITY: usize = 0x3f0;
const WEAK_OBJECT: usize = 0x10;
const PANEL_LOGIC: usize = 0x410;
const PANEL_LABEL: usize = 0x580;
const WIDGET_VISIBLE: usize = 0x5b;
const WIDGET_SHOW: usize = 0x48;
const ENTITY_CONTROL: usize = 0xb0;
const CONTROL_OWNER: usize = 0x20;
const OWNER_IS_PLAYER: usize = 0x80;

/// A squad's relation to the player, as `LogicUtilsImpl` answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    Ally,
    Neutral,
    Enemy,
    Unknown,
}

/// `LogicUtilsImpl`'s relation queries, `bool (this, entity)`, in the order
/// asked: ally, enemy, neutral, abandoned (neutral).
const RELATION: [(usize, Relation); 4] = [
    (0x628, Relation::Ally),
    (0x630, Relation::Enemy),
    (0x638, Relation::Neutral),
    (0x640, Relation::Neutral),
];

/// The settings.
#[derive(Clone, Copy, Debug, Default)]
pub struct Settings {
    pub allied: bool,
    pub neutral: bool,
    pub enemy: bool,
    pub ally_toggles: bool,
}

/// A BGRA dword, as the sprite's colour setter reads it.
const fn bgra(r: u8, g: u8, b: u8) -> u32 {
    b as u32 | (g as u32) << 8 | (r as u32) << 16 | 0xff << 24
}

/// The named reload fill colours. Teal is the stock gradient's: the grey copy
/// times it is the original.
pub const NAMED_COLOURS: [(&str, u32); 5] = [
    ("teal", bgra(0x5c, 0xbe, 0xdd)),
    ("green", bgra(0x5a, 0xdc, 0x5a)),
    ("yellow", bgra(0xff, 0xd7, 0x3c)),
    ("grey-blue", bgra(0x96, 0xaa, 0xc8)),
    ("red", bgra(0xff, 0x46, 0x46)),
];

/// A colour setting: one of [`NAMED_COLOURS`] (`gray` for `grey` too), or
/// `#RRGGBB`.
pub fn parse_colour(text: &str) -> Option<u32> {
    let text = text.trim().to_ascii_lowercase().replace("gray", "grey");
    if let Some(&(_, colour)) = NAMED_COLOURS.iter().find(|(name, _)| *name == text) {
        return Some(colour);
    }
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(0xff00_0000 | value)
}

/// The reload fill colours: the player's own squads', then by relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colours {
    pub own: u32,
    pub allied: u32,
    pub neutral: u32,
    pub enemy: u32,
}

/// Each colour setting's key and default name.
pub const COLOUR_SETTINGS: [(&str, &str); 4] = [
    ("own_colour", "teal"),
    ("allied_colour", "yellow"),
    ("neutral_colour", "grey-blue"),
    ("enemy_colour", "red"),
];

/// The reload fill colour of a card: the player's own squads' colour, or the
/// shown squad's relation; a squad the player does not own whose relation is
/// unknown counts as neutral.
pub fn fill_colour(colours: Colours, owned: bool, relation: Relation) -> u32 {
    if owned {
        return colours.own;
    }
    match relation {
        Relation::Ally => colours.allied,
        Relation::Enemy => colours.enemy,
        Relation::Neutral | Relation::Unknown => colours.neutral,
    }
}

/// Whether a squad of this relation, not the player's, shows its details.
pub fn reveal(settings: Settings, relation: Relation) -> bool {
    match relation {
        Relation::Ally => settings.allied,
        Relation::Neutral => settings.neutral,
        Relation::Enemy => settings.enemy,
        Relation::Unknown => false,
    }
}

/// Whether the ammo grid's toggles act on a squad: the player's own, or an
/// ally shown with `ally_weapon_toggles`.
pub fn toggles(settings: Settings, owned: bool, relation: Relation) -> bool {
    owned || (relation == Relation::Ally && settings.allied && settings.ally_toggles)
}

static ALLIED: AtomicBool = AtomicBool::new(false);
static NEUTRAL: AtomicBool = AtomicBool::new(false);
static ENEMY: AtomicBool = AtomicBool::new(false);
static ALLY_TOGGLES: AtomicBool = AtomicBool::new(false);
/// `LogicUtilsImpl`, recorded from the panel (the ammo menu does not hold it;
/// the panel takes a clicked squad before the menu refreshes it).
static LOGIC: AtomicUsize = AtomicUsize::new(0);
// Read by the stubs as plain qwords.
static SQUAD_RESUME: AtomicUsize = AtomicUsize::new(0);
static AMMO_RESUME: AtomicUsize = AtomicUsize::new(0);

static LABEL_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// The shown-entity getter, as the click handler's call reached it.
static SHOWN: AtomicUsize = AtomicUsize::new(0);
static FILL_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// The fill colours, in [`COLOUR_SETTINGS`] order.
static COLOURS: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
/// A `Sprite` vtable whose colour setter has been checked.
static SPRITE_VTABLE: AtomicUsize = AtomicUsize::new(0);
/// What the fills last showed, for one log line per change: 0 not yet, 1 the
/// grey texture (coloured), 2 another texture (left alone).
static FILL_STATE: AtomicU8 = AtomicU8::new(0);
static LOG: AtomicUsize = AtomicUsize::new(0);

fn settings() -> Settings {
    Settings {
        allied: ALLIED.load(Ordering::Relaxed),
        neutral: NEUTRAL.load(Ordering::Relaxed),
        enemy: ENEMY.load(Ordering::Relaxed),
        ally_toggles: ALLY_TOGGLES.load(Ordering::Relaxed),
    }
}

/// The function in slot `offset` of the vtable of the object at `object`.
unsafe fn method<F: Copy>(object: usize, offset: usize) -> F {
    let vtable = unsafe { *(object as *const *const usize) };
    let entry = unsafe { *vtable.add(offset / 8) };
    unsafe { core::mem::transmute_copy(&entry) }
}

unsafe fn field(object: usize, offset: usize) -> usize {
    unsafe { *((object + offset) as *const usize) }
}

unsafe fn relation(logic: usize, entity: usize) -> Relation {
    if logic == 0 || entity == 0 {
        return Relation::Unknown;
    }
    for (slot, relation) in RELATION {
        let ask: unsafe extern "C" fn(usize, usize) -> u8 = unsafe { method(logic, slot) };
        if unsafe { ask(logic, entity) } != 0 {
            return relation;
        }
    }
    Relation::Unknown
}

/// Whether the player owns `entity`, as both tests ask it.
unsafe fn owned(entity: usize) -> bool {
    let control: unsafe extern "C" fn(usize) -> usize = unsafe { method(entity, ENTITY_CONTROL) };
    let control = unsafe { control(entity) };
    if control == 0 {
        return false;
    }
    let owner = unsafe { field(control, CONTROL_OWNER) };
    if owner == 0 {
        return false;
    }
    let is_player: unsafe extern "C" fn(usize) -> u8 = unsafe { method(owner, OWNER_IS_PLAYER) };
    unsafe { is_player(owner) != 0 }
}

/// The squad setter's owner said no: answer yes if the panel's squad is one
/// the settings reveal. `sub` is the squad sub-panel.
unsafe extern "C" fn squad_reveals(sub: usize) -> u8 {
    let panel = unsafe { field(sub, SUB_PANEL) };
    if panel == 0 {
        return 0;
    }
    let logic = unsafe { field(panel, PANEL_LOGIC) };
    LOGIC.store(logic, Ordering::Relaxed);
    let weak = unsafe { field(panel, PANEL_ENTITY) };
    let entity = if weak == 0 {
        0
    } else {
        unsafe { field(weak, WEAK_OBJECT) }
    };
    reveal(settings(), unsafe { relation(logic, entity) }) as u8
}

/// The ammo menu's owner said no: answer yes if `entity` is revealed.
unsafe extern "C" fn ammo_reveals(entity: usize) -> u8 {
    reveal(settings(), unsafe {
        relation(LOGIC.load(Ordering::Relaxed), entity)
    }) as u8
}

/// In place of the squad setter's `call [rax+0x80]` (the owner in `rcx`, the
/// sub-panel in `rdi`); returns to its `test al, al`. Volatile registers are
/// dead there.
#[unsafe(naked)]
unsafe extern "C" fn squad_stub() {
    core::arch::naked_asm!(
        "call qword ptr [rax + 0x80]",
        "test al, al",
        "jnz 2f",
        "sub rsp, 0x20",
        "mov rcx, rdi",
        "call {reveals}",
        "add rsp, 0x20",
        "2:",
        "jmp qword ptr [rip + {resume}]",
        reveals = sym squad_reveals,
        resume = sym SQUAD_RESUME,
    );
}

/// In place of the ammo menu's `call [rdx+0x80]` (the owner in `rcx`, the
/// shown entity in `rsi`); returns to its `test al, al`.
#[unsafe(naked)]
unsafe extern "C" fn ammo_stub() {
    core::arch::naked_asm!(
        "call qword ptr [rdx + 0x80]",
        "test al, al",
        "jnz 2f",
        "sub rsp, 0x20",
        "mov rcx, rsi",
        "call {reveals}",
        "add rsp, 0x20",
        "2:",
        "jmp qword ptr [rip + {resume}]",
        reveals = sym ammo_reveals,
        resume = sym AMMO_RESUME,
    );
}

type Shown = unsafe extern "C" fn(usize) -> usize;
type Label = unsafe extern "C" fn(usize, usize) -> usize;

/// The click handler's call for the menu's shown entity: none for a squad the
/// player does not own, unless it is an ally the settings let the player
/// direct, so the handler writes nothing.
unsafe extern "C" fn clicked_entity(menu: usize) -> usize {
    let shown: Shown = unsafe { core::mem::transmute(SHOWN.load(Ordering::Acquire)) };
    let entity = unsafe { shown(menu) };
    if entity != 0 && !unsafe { owned(entity) } {
        let relation = unsafe { relation(LOGIC.load(Ordering::Relaxed), entity) };
        if !toggles(settings(), false, relation) {
            return 0;
        }
    }
    entity
}

/// The panel's relation label: hidden when the squad is revealed, since the
/// commander's name then fills its line.
unsafe extern "C" fn label(panel: usize, entity: usize) -> usize {
    let original: Label = unsafe { core::mem::transmute(LABEL_ORIGINAL.load(Ordering::Acquire)) };
    let result = unsafe { original(panel, entity) };
    let logic = unsafe { field(panel, PANEL_LOGIC) };
    LOGIC.store(logic, Ordering::Relaxed);
    if entity != 0
        && !unsafe { owned(entity) }
        && reveal(settings(), unsafe { relation(logic, entity) })
    {
        let widget = unsafe { field(panel, PANEL_LABEL) };
        if widget != 0 && unsafe { *((widget + WIDGET_VISIBLE) as *const u8) } != 0 {
            let show: unsafe extern "C" fn(usize, u8) = unsafe { method(widget, WIDGET_SHOW) };
            unsafe { show(widget, 0) };
        }
    }
    result
}

type Fill = unsafe extern "C" fn(usize, usize, usize) -> usize;

/// One log line when the fills change between the grey texture and another.
fn note_fill(state: u8, size: (u32, u32)) {
    if FILL_STATE.swap(state, Ordering::Relaxed) == state {
        return;
    }
    let log = LOG.load(Ordering::Relaxed);
    if log == 0 {
        return;
    }
    let log: unsafe extern "C" fn(u32, *const core::ffi::c_char) =
        unsafe { core::mem::transmute(log) };
    let text = if state == 1 {
        "unit inspection: colouring the ammo cards' reload bars by relation".to_string()
    } else {
        format!(
            "unit inspection: the reload bar texture is {}x{}, not the companion mod's grey \
             {}x{}; the bars keep the game's colour (is the unit-inspection mod enabled in MODS?)",
            size.0, size.1, GREY_BAR.0, GREY_BAR.1
        )
    };
    let text = std::ffi::CString::new(text).unwrap_or_default();
    unsafe { log(LOG_INFO, text.as_ptr()) };
}

/// Colour a card's reload fill, when it shows the grey texture.
unsafe fn colour_fill(card: usize, colour: u32) {
    let bar = unsafe { field(card, CARD_RELOAD_BAR) };
    let fill = if bar == 0 {
        0
    } else {
        unsafe { field(bar, BAR_FILL) }
    };
    let view = if fill == 0 {
        0
    } else {
        unsafe { field(fill, WIDGET_VIEW) }
    };
    if view == 0 {
        return;
    }
    let sprite = unsafe { field(view, VIEW_SPRITE) };
    let [width, height] = unsafe { *((view + VIEW_TEXTURE_SIZE) as *const [u32; 2]) };
    let size = (width, height);
    if sprite == 0 || size == (0, 0) {
        return;
    }
    if size != GREY_BAR && (size.1, size.0) != GREY_BAR {
        note_fill(2, size);
        return;
    }
    let vtable = unsafe { field(sprite, 0) };
    if SPRITE_VTABLE.load(Ordering::Relaxed) != vtable {
        let setter = unsafe { field(vtable, SPRITE_SET_COLOUR) };
        let start =
            unsafe { core::slice::from_raw_parts(setter as *const u8, SET_COLOUR_START.len()) };
        if start != SET_COLOUR_START {
            return;
        }
        SPRITE_VTABLE.store(vtable, Ordering::Relaxed);
    }
    note_fill(1, size);
    let set: unsafe extern "C" fn(usize, *const u32) = unsafe { method(sprite, SPRITE_SET_COLOUR) };
    unsafe { set(sprite, &colour) };
}

/// The ammo menu's `fillSlot`: fill the card, then colour its reload bar by
/// the shown squad.
unsafe extern "C" fn fill(menu: usize, index: usize, record: usize) -> usize {
    let original: Fill = unsafe { core::mem::transmute(FILL_ORIGINAL.load(Ordering::Acquire)) };
    let result = unsafe { original(menu, index, record) };
    let shown = SHOWN.load(Ordering::Acquire);
    if shown != 0 {
        let shown: Shown = unsafe { core::mem::transmute(shown) };
        let entity = unsafe { shown(menu) };
        let owned = entity == 0 || unsafe { owned(entity) };
        let relation = if owned {
            Relation::Unknown
        } else {
            unsafe { relation(LOGIC.load(Ordering::Relaxed), entity) }
        };
        let [own, allied, neutral, enemy] =
            [0, 1, 2, 3].map(|i| COLOURS[i].load(Ordering::Relaxed));
        let colours = Colours {
            own,
            allied,
            neutral,
            enemy,
        };
        unsafe {
            colour_fill(
                menu + MENU_CARDS + index * CARD_SIZE,
                fill_colour(colours, owned, relation),
            )
        };
    }
    result
}

fn find(api: &Api, base: *mut c_void, size: usize, pattern: &str) -> usize {
    let pattern = std::ffi::CString::new(pattern).unwrap_or_default();
    unsafe { (api.find_pattern)(base, size, pattern.as_ptr()) as usize }
}

fn say(api: &Api, level: u32, text: &str) {
    let text = std::ffi::CString::new(text).unwrap_or_default();
    unsafe { (api.log)(level, text.as_ptr()) };
}

/// The four sites, found and vouched for: (squad call, ammo call, the click
/// handler's shown-entity call, label function).
fn sites(api: &Api) -> Result<(usize, usize, usize, usize), String> {
    let base = unsafe { (api.module_base)(c"game.dll".as_ptr()) };
    if base.is_null() {
        return Err("game.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base) };
    let (squad, ammo, click, label) = (
        find(api, base, size, SQUAD_PATTERN),
        find(api, base, size, AMMO_PATTERN),
        find(api, base, size, CLICK_PATTERN),
        find(api, base, size, LABEL_PATTERN),
    );
    if squad == 0 || ammo == 0 || click == 0 || label == 0 {
        return Err("not a supported build (the 2026 updates are)".into());
    }
    let slots_match = LABEL_SLOTS.iter().all(|(at, bytes)| {
        (0..bytes.len()).all(|i| unsafe { *((label + at + i) as *const u8) } == bytes[i])
    });
    if !slots_match {
        return Err("the relation queries are laid out differently".into());
    }
    Ok((
        squad + SQUAD_CALL_AT,
        ammo + AMMO_CALL_AT,
        click + CLICK_SHOWN_CALL_AT,
        label,
    ))
}

fn install(api: &Api) -> Result<(), String> {
    let (squad, ammo, click_call, label_fn) = sites(api)?;
    SQUAD_RESUME.store(squad + 6, Ordering::Release);
    AMMO_RESUME.store(ammo + 6, Ordering::Release);
    for (site, stub) in [
        (squad, squad_stub as unsafe extern "C" fn() as *mut c_void),
        (ammo, ammo_stub as unsafe extern "C" fn() as *mut c_void),
    ] {
        let mut trampoline = core::ptr::null_mut();
        if unsafe { (api.hook_exact)(site as *mut c_void, stub, 6, &mut trampoline) } != 0 {
            return Err(format!(
                "the ownership test at game+{site:#x} could not be hooked"
            ));
        }
    }
    let mut shown = core::ptr::null_mut();
    let detour = clicked_entity as Shown as *mut c_void;
    if unsafe { (api.hook_call)(click_call as *mut c_void, detour, &mut shown) } != 0
        || shown.is_null()
    {
        return Err("the ammo grid's click handler could not be redirected".into());
    }
    SHOWN.store(shown as usize, Ordering::Release);
    let mut original = core::ptr::null_mut();
    let detour = label as Label as *mut c_void;
    if unsafe { (api.hook)(label_fn as *mut c_void, detour, &mut original) } != 0
        || original.is_null()
    {
        return Err("the relation label could not be hooked".into());
    }
    LABEL_ORIGINAL.store(original as usize, Ordering::Release);
    Ok(())
}

/// Hook `fillSlot` to colour the reload bars; the rest works without it.
fn install_colours(api: &Api) -> Result<(), String> {
    let base = unsafe { (api.module_base)(c"game.dll".as_ptr()) };
    let size = unsafe { (api.module_size)(base) };
    let target = find(api, base, size, FILL_PATTERN);
    if target == 0 {
        return Err("the ammo card fill was not found".into());
    }
    let mut original = core::ptr::null_mut();
    let detour = fill as Fill as *mut c_void;
    if unsafe { (api.hook)(target as *mut c_void, detour, &mut original) } != 0
        || original.is_null()
    {
        return Err("the ammo card fill could not be hooked".into());
    }
    FILL_ORIGINAL.store(original as usize, Ordering::Release);
    Ok(())
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    if api.is_null() {
        return 1;
    }
    let api = unsafe { &*api };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    let flag = |key: &str| unsafe { defiance_feature_sdk::boolean(api, ID, key) };
    let (Ok(allied), Ok(neutral), Ok(enemy), Ok(ally_toggles)) = (
        flag("show_allied"),
        flag("show_neutral"),
        flag("show_enemy"),
        flag("ally_weapon_toggles"),
    ) else {
        return 1;
    };
    ALLIED.store(allied, Ordering::Relaxed);
    NEUTRAL.store(neutral, Ordering::Relaxed);
    ENEMY.store(enemy, Ordering::Relaxed);
    ALLY_TOGGLES.store(ally_toggles, Ordering::Relaxed);
    for (slot, (key, default)) in COLOURS.iter().zip(COLOUR_SETTINGS) {
        let text = unsafe { defiance_feature_sdk::string(api, ID, key) }.unwrap_or_default();
        let colour = parse_colour(&text).unwrap_or_else(|| {
            say(
                api,
                LOG_WARN,
                &format!(
                    "unit inspection: {key} = {text:?} is not a colour (teal, green, yellow, \
                     grey-blue, red or #RRGGBB); using {default}"
                ),
            );
            parse_colour(default).unwrap_or(u32::MAX)
        });
        slot.store(colour, Ordering::Relaxed);
    }
    LOG.store(api.log as usize, Ordering::Relaxed);
    match install(api) {
        Ok(()) => {
            if let Err(e) = install_colours(api) {
                say(
                    api,
                    LOG_WARN,
                    &format!("unit inspection: {e}; the reload bars keep the game's colour"),
                );
            }
            let who: Vec<&str> = [(allied, "allied"), (neutral, "neutral"), (enemy, "enemy")]
                .into_iter()
                .filter(|(on, _)| *on)
                .map(|(_, name)| name)
                .collect();
            say(
                api,
                LOG_INFO,
                &format!(
                    "unit inspection: full details for {} squads{}",
                    if who.is_empty() {
                        "no".to_string()
                    } else {
                        who.join(", ")
                    },
                    if allied && ally_toggles {
                        "; allies' weapon toggles work"
                    } else {
                        ""
                    }
                ),
            );
            0
        }
        Err(e) => {
            say(
                api,
                LOG_WARN,
                &format!("unit inspection: {e}; the game's panel stays"),
            );
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.unit-inspection".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Settings = Settings {
        allied: true,
        neutral: true,
        enemy: true,
        ally_toggles: false,
    };

    #[test]
    fn each_relation_follows_its_setting() {
        assert!(
            reveal(ALL, Relation::Ally)
                && reveal(ALL, Relation::Neutral)
                && reveal(ALL, Relation::Enemy)
        );
        assert!(!reveal(ALL, Relation::Unknown));
        let allies_only = Settings {
            neutral: false,
            enemy: false,
            ..ALL
        };
        assert!(reveal(allies_only, Relation::Ally));
        assert!(!reveal(allies_only, Relation::Neutral) && !reveal(allies_only, Relation::Enemy));
    }

    #[test]
    fn only_the_players_squads_and_directed_allies_take_toggles() {
        assert!(toggles(ALL, true, Relation::Unknown));
        assert!(
            !toggles(ALL, false, Relation::Ally),
            "ally toggles are off by default"
        );
        let direct = Settings {
            ally_toggles: true,
            ..ALL
        };
        assert!(toggles(direct, false, Relation::Ally));
        assert!(
            !toggles(direct, false, Relation::Enemy) && !toggles(direct, false, Relation::Neutral)
        );
        // An ally not shown is not directed either.
        assert!(!toggles(
            Settings {
                allied: false,
                ..direct
            },
            false,
            Relation::Ally
        ));
    }

    fn defaults() -> Colours {
        let [own, allied, neutral, enemy] =
            COLOUR_SETTINGS.map(|(_, default)| parse_colour(default).unwrap());
        Colours {
            own,
            allied,
            neutral,
            enemy,
        }
    }

    #[test]
    fn fills_follow_ownership_then_relation() {
        let colours = defaults();
        assert_eq!(
            fill_colour(colours, true, Relation::Enemy),
            0xff5cbedd,
            "ownership decides first; teal by default"
        );
        assert_eq!(fill_colour(colours, false, Relation::Enemy), 0xffff4646);
        assert_eq!(fill_colour(colours, false, Relation::Ally), 0xffffd73c);
        assert_eq!(fill_colour(colours, false, Relation::Neutral), 0xff96aac8);
        assert_eq!(
            fill_colour(colours, false, Relation::Unknown),
            colours.neutral
        );
    }

    #[test]
    fn colours_parse_as_names_or_hex() {
        assert_eq!(parse_colour(" Green "), Some(0xff5adc5a));
        assert_eq!(parse_colour("Gray-Blue"), parse_colour("grey-blue"));
        assert_eq!(parse_colour("#12aBef"), Some(0xff12abef));
        for bad in ["blue", "12abef", "#12abe", "#12abeg", ""] {
            assert_eq!(parse_colour(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_manifest_declares_each_colour_with_its_default() {
        let manifest = include_str!("../defiance_plugin_unit_inspection.plugin.json");
        for (key, default) in COLOUR_SETTINGS {
            let at = manifest.find(&format!("\"key\": \"{key}\"")).expect(key);
            let entry = &manifest[at..];
            let entry = &entry[..entry.find('}').unwrap()];
            assert!(
                entry.contains(&format!("\"default\": \"{default}\"")),
                "{key}"
            );
        }
    }

    #[test]
    fn the_grey_bar_differs_from_the_stock_one() {
        assert_ne!(GREY_BAR, (88, 4));
        assert_ne!((GREY_BAR.1, GREY_BAR.0), (88, 4));
    }

    #[test]
    fn the_call_sites_are_six_byte_indirect_calls() {
        let bytes = |pattern: &str, at: usize| -> Vec<String> {
            pattern
                .split_whitespace()
                .skip(at)
                .take(6)
                .map(str::to_string)
                .collect()
        };
        assert_eq!(
            bytes(SQUAD_PATTERN, SQUAD_CALL_AT),
            ["ff", "90", "80", "00", "00", "00"]
        );
        assert_eq!(
            bytes(AMMO_PATTERN, AMMO_CALL_AT),
            ["ff", "92", "80", "00", "00", "00"]
        );
        assert_eq!(bytes(CLICK_PATTERN, CLICK_SHOWN_CALL_AT)[0], "e8");
    }
}
