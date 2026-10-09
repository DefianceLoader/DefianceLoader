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
//! squad's relation is one the settings reveal ([`reveal`]). Ownership and
//! the relation come from Core's `relation` service (`RelationV1`), which asks
//! the owner's predicates in the order the panel's own relation label does
//! (`fn_366ad0`, [`LABEL_PATTERN`]); abandoned counts as neutral
//! ([`Relation::from_service`]). Without the service the plugin changes
//! nothing.
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
//! The expanded ammo menu checks `fillSlot`'s first bytes as they were before
//! any hook (the loader's `original` service), so either plugin may start
//! first, and then calls it for its combined view, whose cards are the
//! player's own.
//!
//! The five sites are found by signature in `game.dll`'s bytes from before any
//! hook ([`sites`]) before anything is hooked; when one does not match exactly
//! once, the plugin logs a warning and changes nothing. The panel offsets are
//! the 2026 builds'; the relation label's function differs in the 2025 builds,
//! which do not resolve. Vehicles and platforms need nothing of
//! their own: the ammo refresh asks the same ownership question for any shown
//! unit, and the click gate applies to them too. Not multiplayer-safe: the
//! ally toggles change another player's units.
use core::ffi::c_void;
use defiance_api::{
    Api, PatchContractV1, Plugin, RelationV1, ABI_VERSION, LOG_DEBUG, LOG_INFO, LOG_WARN,
    PATCH_KIND_CALL, PATCH_KIND_ENTRY, RELATION_ABANDONED, RELATION_ALLY, RELATION_ENEMY,
    RELATION_NEUTRAL,
};
use defiance_core::sites::{code_ranges, Image};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU8, AtomicUsize, Ordering};

mod patterns;
mod sites;

defiance_feature_sdk::service_handshake!();

pub use patterns::*;

const ID: &str = "defiance.unit-inspection";

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
const PANEL_LABEL: usize = 0x580;
const WIDGET_VISIBLE: usize = 0x5b;
const WIDGET_SHOW: usize = 0x48;

/// A squad's relation to the player, as Core's relation service answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    Ally,
    Neutral,
    Enemy,
    Unknown,
}

impl Relation {
    /// The service's answer: abandoned counts as neutral, none as unknown.
    pub fn from_service(relation: u32) -> Self {
        match relation {
            RELATION_ALLY => Self::Ally,
            RELATION_ENEMY => Self::Enemy,
            RELATION_NEUTRAL | RELATION_ABANDONED => Self::Neutral,
            _ => Self::Unknown,
        }
    }
}

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
/// Core's relation service, set by each `init` before anything is hooked.
static RELATIONS: AtomicPtr<RelationV1> = AtomicPtr::new(core::ptr::null_mut());

fn relations() -> Option<&'static RelationV1> {
    unsafe { RELATIONS.load(Ordering::Acquire).as_ref() }
}
// Read by the stubs as plain qwords.
static SQUAD_RESUME: AtomicUsize = AtomicUsize::new(0);
static AMMO_RESUME: AtomicUsize = AtomicUsize::new(0);

static LABEL_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// The squad the squad sub-panel last revealed, for the label.
static REVEALED_SQUAD: AtomicUsize = AtomicUsize::new(0);
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

/// `entity`'s relation to the player; unknown for none or without the service.
unsafe fn relation(entity: usize) -> Relation {
    match relations() {
        Some(service) => {
            Relation::from_service(unsafe { (service.relation)(entity as *mut c_void) })
        }
        None => Relation::Unknown,
    }
}

/// Whether the player owns `entity`, as both tests ask it.
unsafe fn owned(entity: usize) -> bool {
    relations().is_some_and(|service| unsafe { (service.owned)(entity as *mut c_void) } != 0)
}

/// The squad setter's owner said no: answer yes if the panel's squad is one
/// the settings reveal. `sub` is the squad sub-panel.
unsafe extern "C" fn squad_reveals(sub: usize) -> u8 {
    let panel = unsafe { field(sub, SUB_PANEL) };
    if panel == 0 {
        return 0;
    }
    let weak = unsafe { field(panel, PANEL_ENTITY) };
    let entity = if weak == 0 {
        0
    } else {
        unsafe { field(weak, WEAK_OBJECT) }
    };
    let revealed = reveal(settings(), unsafe { relation(entity) });
    REVEALED_SQUAD.store(if revealed { entity } else { 0 }, Ordering::Relaxed);
    revealed as u8
}

/// The ammo menu's owner said no: answer yes if `entity` is revealed.
unsafe extern "C" fn ammo_reveals(entity: usize) -> u8 {
    reveal(settings(), unsafe { relation(entity) }) as u8
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
        let relation = unsafe { relation(entity) };
        if !toggles(settings(), false, relation) {
            return 0;
        }
    }
    entity
}

/// The panel's relation label: hidden when the squad sub-panel has just
/// revealed this squad, since the commander's name then fills its line. The
/// panel's set-entity (`fn_366850`) calls the sub-panel's setter just before
/// this; a vehicle's sub-panel is not revealed, so its label stays.
unsafe extern "C" fn label(panel: usize, entity: usize) -> usize {
    let original: Label = unsafe { core::mem::transmute(LABEL_ORIGINAL.load(Ordering::Acquire)) };
    let result = unsafe { original(panel, entity) };
    if entity != 0 && REVEALED_SQUAD.load(Ordering::Relaxed) == entity {
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
    let (level, text) = if state == 1 {
        (
            LOG_DEBUG,
            "unit inspection: colouring the ammo cards' reload bars by relation".to_string(),
        )
    } else {
        (
            LOG_INFO,
            format!(
                "unit inspection: the reload bar texture is {}x{}, not the companion mod's grey \
                 {}x{}; the bars keep the game's colour (is the unit-inspection mod enabled in MODS?)",
                size.0, size.1, GREY_BAR.0, GREY_BAR.1
            ),
        )
    };
    let text = std::ffi::CString::new(text).unwrap_or_default();
    unsafe { log(level, text.as_ptr()) };
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
            unsafe { relation(entity) }
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

fn say(api: &Api, level: u32, text: &str) {
    let text = std::ffi::CString::new(text).unwrap_or_default();
    unsafe { (api.log)(level, text.as_ptr()) };
}

enum InstallError {
    UnsupportedBuild(String),
    Failed(String),
}

/// game.dll's base, its five sites, and a copy of its image whose code is the
/// bytes from before any hook, so the signatures match whichever plugin
/// started first.
fn resolve(api: &Api) -> Result<(usize, sites::Sites, Vec<u8>), InstallError> {
    let base = unsafe { (api.module_base)(c"game.dll".as_ptr()) }.cast::<u8>();
    if base.is_null() {
        return Err(InstallError::Failed("game.dll is not loaded".into()));
    }
    let size = unsafe { (api.module_size)(base.cast()) };
    let mut image = unsafe { core::slice::from_raw_parts(base, size) }.to_vec();
    if let Some(original) = unsafe { defiance_feature_sdk::services::original() } {
        for (start, end) in code_ranges(&image) {
            let end = end.min(size);
            if start < end {
                // A failed read keeps the live bytes, which the signatures
                // then judge.
                unsafe {
                    (original.read)(
                        base as usize + start,
                        image[start..end].as_mut_ptr(),
                        end - start,
                    )
                };
            }
        }
    }
    let game = Image {
        image: &image,
        base: base as usize,
    };
    let sites = sites::sites(&game).map_err(InstallError::UnsupportedBuild)?;
    Ok((base as usize, sites, image))
}

/// Places all five hooks. A failure leaves the earlier ones for the loader to
/// remove when `init` refuses.
fn install(api: &Api, base: usize, sites: &sites::Sites) -> Result<(), InstallError> {
    let (squad, ammo) = (base + sites.squad, base + sites.ammo);
    SQUAD_RESUME.store(squad + 6, Ordering::Release);
    AMMO_RESUME.store(ammo + 6, Ordering::Release);
    for (site, stub) in [
        (squad, squad_stub as unsafe extern "C" fn() as *mut c_void),
        (ammo, ammo_stub as unsafe extern "C" fn() as *mut c_void),
    ] {
        let mut trampoline = core::ptr::null_mut();
        if unsafe { (api.hook_exact)(site as *mut c_void, stub, 6, &mut trampoline) } != 0 {
            return Err(InstallError::Failed(format!(
                "the ownership test at game+{:#x} could not be hooked",
                site - base
            )));
        }
    }
    let mut shown = core::ptr::null_mut();
    let detour = clicked_entity as Shown as *mut c_void;
    if unsafe { (api.hook_call)((base + sites.click) as *mut c_void, detour, &mut shown) } != 0
        || shown.is_null()
    {
        return Err(InstallError::Failed(
            "the ammo grid's click handler could not be redirected".into(),
        ));
    }
    SHOWN.store(shown as usize, Ordering::Release);
    let mut original = core::ptr::null_mut();
    let detour = label as Label as *mut c_void;
    if unsafe { (api.hook)((base + sites.label) as *mut c_void, detour, &mut original) } != 0
        || original.is_null()
    {
        return Err(InstallError::Failed(
            "the relation label could not be hooked".into(),
        ));
    }
    LABEL_ORIGINAL.store(original as usize, Ordering::Release);
    let mut original = core::ptr::null_mut();
    let detour = fill as Fill as *mut c_void;
    if unsafe { (api.hook)((base + sites.fill) as *mut c_void, detour, &mut original) } != 0
        || original.is_null()
    {
        return Err(InstallError::Failed(
            "the ammo card fill could not be hooked".into(),
        ));
    }
    FILL_ORIGINAL.store(original as usize, Ordering::Release);
    Ok(())
}

fn contract_entry(
    image: &[u8],
    rva: usize,
    kind: u32,
    exact_len: Option<usize>,
) -> Option<defiance_feature_sdk::contract::Patch> {
    let bytes = image.get(rva..rva.checked_add(64)?.min(image.len()))?;
    let len = match exact_len {
        Some(len) => len,
        None => defiance_core::decode::displaced(bytes, 5).ok()?,
    };
    Some(defiance_feature_sdk::contract::Patch {
        module: c"game.dll",
        rva,
        kind,
        before: bytes.get(..len)?.to_vec(),
        after: None,
    })
}

unsafe extern "C" fn patch_contract(api: *const Api) -> *const PatchContractV1 {
    let Some(api_ref) = (unsafe { api.as_ref() }) else {
        return core::ptr::null();
    };
    if api_ref.abi_version != ABI_VERSION || api_ref.reserved != 0 {
        return core::ptr::null();
    }
    let (_, sites, image) = match resolve(api_ref) {
        Ok(resolved) => resolved,
        Err(InstallError::UnsupportedBuild(_)) => {
            return unsafe { defiance_feature_sdk::contract::build(api, Vec::new()) };
        }
        Err(InstallError::Failed(_)) => return core::ptr::null(),
    };
    let mut patches = Vec::with_capacity(5);
    for (rva, kind, exact_len) in [
        (sites.squad, PATCH_KIND_ENTRY, Some(6)),
        (sites.ammo, PATCH_KIND_ENTRY, Some(6)),
        (sites.click, PATCH_KIND_CALL, Some(5)),
        (sites.label, PATCH_KIND_ENTRY, None),
        (sites.fill, PATCH_KIND_ENTRY, None),
    ] {
        let Some(patch) = contract_entry(&image, rva, kind, exact_len) else {
            return core::ptr::null();
        };
        patches.push(patch);
    }
    unsafe { defiance_feature_sdk::contract::build(api, patches) }
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
    let Some(service) = (unsafe { defiance_feature_sdk::services::relation() }) else {
        say(
            api,
            LOG_WARN,
            "unit inspection: Core's relation service is unavailable; the game's panel stays",
        );
        return 1;
    };
    RELATIONS.store((service as *const RelationV1).cast_mut(), Ordering::Release);
    match resolve(api).and_then(|(base, sites, _)| install(api, base, &sites)) {
        Ok(()) => {
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
        Err(InstallError::UnsupportedBuild(reason)) => {
            say(
                api,
                LOG_WARN,
                &format!(
                    "unit inspection: not a supported build ({reason}); the game's panel stays"
                ),
            );
            0
        }
        Err(InstallError::Failed(e)) => {
            say(
                api,
                LOG_WARN,
                &format!("unit inspection: {e}; the game's panel stays"),
            );
            1
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

#[no_mangle]
pub unsafe extern "C" fn defiance_patch_contract_v1(api: *const Api) -> *const PatchContractV1 {
    unsafe { patch_contract(api) }
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
    fn service_relations_map_abandoned_to_neutral() {
        use defiance_api::RELATION_NONE;
        assert_eq!(Relation::from_service(RELATION_ALLY), Relation::Ally);
        assert_eq!(Relation::from_service(RELATION_ENEMY), Relation::Enemy);
        assert_eq!(Relation::from_service(RELATION_NEUTRAL), Relation::Neutral);
        assert_eq!(
            Relation::from_service(RELATION_ABANDONED),
            Relation::Neutral
        );
        assert_eq!(Relation::from_service(RELATION_NONE), Relation::Unknown);
        assert_eq!(Relation::from_service(99), Relation::Unknown);
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
    fn the_plugin_loads_only_at_startup() {
        // Live unloading and reloading is untried in game; the docs list it
        // among the plugins that need a restart.
        let manifest = include_str!("../defiance_plugin_unit_inspection.plugin.json");
        assert!(manifest.contains("\"hot_reload\": false"));
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
