//! Verified MSVC layouts for the hash-gated builds in sites.rs. The 2026-09
//! update did not move the panel, widget, slider or record fields this reads:
//! the functions it mirrors (refresh, weapon, script, slider_dispatch, thumb,
//! ammo) align with no non-stack offset change, and refresh still reads the
//! panel at +0x288, +0x210 and +0x270.
//! All object access runs synchronously on the panel's UI thread. Only offsets
//! and object identity survive callbacks; inventory record addresses do not.
use super::{
    viewport::{Viewport, PERK_VISIBLE, UPGRADE_VISIBLE, VISIBLE},
    Build, ACTIVE, ENGINE, LOG_WARN, ORIGINAL, ORIGINAL_SQUAD_CHOOSER, ORIGINAL_THUMB,
    ORIGINAL_VEHICLE, ORIGINAL_VEHICLE_CHOOSER,
};
use core::{ffi::c_char, ptr};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    sync::atomic::{AtomicPtr, Ordering},
};

/// The range of the `perk_slots` setting (the manifest declares the same and
/// the default, five, which matches stock). Fewer than five would blank cards
/// for perks the squad owns, so five is also the minimum.
pub const PERK_SLOTS_MIN: usize = PERK_VISIBLE;
pub const PERK_SLOTS_MAX: usize = 20;
/// The range of the `upgrade_slots` setting (as the manifest declares; default
/// 20). The column is this long whatever the unit owns (blank cards past its
/// upgrades) and shows five widgets at a time, so a blank card can be scrolled
/// into view and a sixth or later upgrade dropped onto it. Five reproduces
/// stock.
pub const UPGRADE_SLOTS_MIN: usize = 5;
pub const UPGRADE_SLOTS_MAX: usize = 30;
/// Row indices in a `Layout`. Weapons and ammunition exist on both panels; the
/// upgrade column and the perk pick row are separate lists with the same five
/// widgets each. The vehicle panel has no perk row.
const WEAPON: usize = 0;
const AMMO: usize = 1;
const UPGRADE: usize = 2;
const PERK: usize = 3;
/// Whether the squad may pick a perk now: the stock perk refresh highlights the
/// first open card only then. tools/squad_scroll_bindings.py checks the offset
/// in every build's refresh.
const PANEL_MAY_PICK: usize = 0x352;
/// Whether cards past the squad's rank are drawn locked; when clear, every card
/// past its perks is open. Checked per build like `PANEL_MAY_PICK`.
const PANEL_LOCKS: usize = 0x210;
/// A perk card's state, which its redraw draws from and the panel's click reads:
/// open (a perk may go here) and highlighted (a click picks for it). The perk
/// itself is at +0x1d8, set by the bind.
const CARD_OPEN: usize = 0x1d0;
const CARD_HIGHLIGHT: usize = 0x1d1;

/// The fixed geometry of one panel class. Both panels are the same base widget
/// with different child offsets; the unit object hangs off a different member,
/// and the vehicle panel has no perk row. The record vectors live on the unit
/// at the same offsets for both.
struct Layout {
    /// The panel member holding the selected unit (squad or vehicle).
    unit: usize,
    /// The panel member holding the layout root whose children are the cards.
    /// The classes differ: the squad root is at +0x120, the vehicle at +0x128.
    root: usize,
    /// Per-row widget-array base on the panel.
    section: [usize; 4],
    /// How many cards each row shows.
    visible: [usize; 4],
    /// Per-row vector offset on the unit (weapons, ammunition, upgrades).
    vector: [usize; 3],
    /// Per-row element stride (weapons, ammunition, upgrades).
    stride: [usize; 3],
    /// Whether the squad-only validity check and perk row apply.
    squad: bool,
}
const SQUAD: Layout = Layout {
    unit: 0x288,
    root: 0x120,
    section: [0x160, 0x190, 0x1e8, 0x1c0],
    visible: [VISIBLE, VISIBLE, UPGRADE_VISIBLE, PERK_VISIBLE],
    vector: [0x270, 0x210, 0x288],
    stride: [0x48, 0x28, 0x20],
    squad: true,
};
const VEHICLE: Layout = Layout {
    unit: 0x278,
    root: 0x128,
    section: [0x150, 0x180, 0x1b0, 0],
    visible: [VISIBLE, VISIBLE, UPGRADE_VISIBLE, 0],
    vector: [0x270, 0x210, 0x288],
    stride: [0x48, 0x28, 0x20],
    squad: false,
};

type One = unsafe extern "C" fn(usize);
type Two = unsafe extern "C" fn(usize, usize);
type Resolve = unsafe extern "C" fn(usize, usize) -> usize;
type Event = unsafe extern "C" fn(usize, usize, usize, usize);
pub(super) struct Engine {
    dispatch: Event,
    slider_dispatch: Event,
    destroy: unsafe extern "C" fn(usize, u32) -> usize,
    vehicle_destroy: unsafe extern "C" fn(usize, u32) -> usize,
    ammo: Two,
    weapon: One,
    perk: Two,
    upgrade: Two,
    script: Resolve,
    listen: Two,
    thumb: One,
    slider_vtable: usize,
    squad_vtable: usize,
    vehicle_vtable: usize,
    context_service: usize,
    perk_limit: usize,
    training_key: unsafe extern "C" fn(usize) -> usize,
    upgrade_key: usize,
    perk_slots: usize,
    upgrade_slots: usize,
    /// Settings: whether the upgrade columns and the vehicle panel take part.
    upgrades: bool,
    vehicles: bool,
    log: unsafe extern "C" fn(u32, *const c_char),
}
impl Engine {
    pub unsafe fn new(
        base: usize,
        b: &Build,
        log: unsafe extern "C" fn(u32, *const c_char),
        perk_slots: usize,
        upgrade_slots: usize,
        upgrades: bool,
        vehicles: bool,
    ) -> Self {
        Self {
            dispatch: std::mem::transmute::<usize, Event>(base + b.dispatch.rva),
            slider_dispatch: std::mem::transmute::<usize, Event>(base + b.slider_dispatch.rva),
            destroy: std::mem::transmute::<usize, unsafe extern "C" fn(usize, u32) -> usize>(
                base + b.destroy.rva,
            ),
            vehicle_destroy: std::mem::transmute::<usize, unsafe extern "C" fn(usize, u32) -> usize>(
                base + b.vehicle_destroy.rva,
            ),
            ammo: std::mem::transmute::<usize, Two>(base + b.ammo.rva),
            weapon: std::mem::transmute::<usize, One>(base + b.weapon.rva),
            perk: std::mem::transmute::<usize, Two>(base + b.perk.rva),
            upgrade: std::mem::transmute::<usize, Two>(base + b.upgrade.rva),
            script: std::mem::transmute::<usize, Resolve>(base + b.script.rva),
            listen: std::mem::transmute::<usize, Two>(base + b.listen.rva),
            thumb: std::mem::transmute::<usize, One>(base + b.thumb.rva),
            slider_vtable: base + b.slider_vtable,
            squad_vtable: base + b.panel_vtable,
            vehicle_vtable: base + b.vehicle_vtable,
            context_service: b.context_service,
            perk_limit: b.perk_limit,
            training_key: std::mem::transmute::<usize, unsafe extern "C" fn(usize) -> usize>(
                base + b.training_key.rva,
            ),
            upgrade_key: base + b.upgrade_key,
            perk_slots: perk_slots.clamp(PERK_SLOTS_MIN, PERK_SLOTS_MAX),
            upgrade_slots: upgrade_slots.clamp(UPGRADE_SLOTS_MIN, UPGRADE_SLOTS_MAX),
            upgrades,
            vehicles,
            log,
        }
    }
}
#[derive(Clone, Copy)]
struct State {
    layout: &'static Layout,
    unit: usize,
    root: usize,
    views: [Viewport; 4],
}
impl State {
    fn new(layout: &'static Layout) -> Self {
        Self {
            layout,
            unit: 0,
            root: 0,
            views: [
                Viewport::new(VISIBLE),
                Viewport::new(VISIBLE),
                Viewport::new(UPGRADE_VISIBLE),
                Viewport::new(PERK_VISIBLE),
            ],
        }
    }
}
thread_local! {
    static STATES: RefCell<HashMap<usize, State>> = RefCell::new(HashMap::new());
    static BUSY: Cell<bool> = const { Cell::new(false) };
    static WARNED: Cell<bool> = const { Cell::new(false) };
}
#[link(name = "user32")]
extern "system" {
    fn GetKeyState(key: i32) -> i16;
}
struct Guard;
impl Guard {
    fn enter() -> Option<Self> {
        BUSY.with(|b| if b.replace(true) { None } else { Some(Self) })
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        BUSY.with(|b| b.set(false));
    }
}
unsafe fn read<T: Copy>(at: usize) -> T {
    std::ptr::read_unaligned(at as *const T)
}
unsafe fn write<T>(at: usize, value: T) {
    std::ptr::write_unaligned(at as *mut T, value);
}

fn count(begin: usize, end: usize, stride: usize) -> Option<usize> {
    let bytes = end.checked_sub(begin)?;
    if bytes % stride != 0 || bytes / stride > 4096 || (begin == 0 && bytes != 0) {
        None
    } else {
        Some(bytes / stride)
    }
}
unsafe fn vector(owner: usize, offset: usize, stride: usize) -> Option<Vec<usize>> {
    let begin = read::<usize>(owner + offset);
    let end = read::<usize>(owner + offset + 8);
    Some(
        (0..count(begin, end, stride)?)
            .map(|i| begin + i * stride)
            .collect(),
    )
}
unsafe fn child(root: usize, name: &[u8]) -> Option<usize> {
    for at in vector(root, 0x40, 8)? {
        let widget = read::<usize>(at);
        if widget == 0 || read::<usize>(widget + 0x28) != name.len() {
            continue;
        }
        let chars = if read::<usize>(widget + 0x30) <= 15 {
            widget + 0x18
        } else {
            read(widget + 0x18)
        };
        if chars != 0 && std::slice::from_raw_parts(chars as *const u8, name.len()) == name {
            return Some(widget);
        }
    }
    None
}
unsafe fn is_slider(e: &Engine, widget: usize) -> bool {
    read::<usize>(widget) == e.slider_vtable && read::<usize>(widget + 0x1a8) != 0
}

/// The weapon and ammunition sliders are required (the existing feature); the
/// upgrade and perk sliders are optional so an older companion mod keeps the
/// rows it already knows working. Unset slots are zero.
unsafe fn sliders(e: &Engine, root: usize) -> Option<[usize; 4]> {
    if root == 0 {
        return None;
    }
    let weapons = child(root, b"df_weapons")?;
    let ammo = child(root, b"df_ammo")?;
    if !is_slider(e, weapons) || !is_slider(e, ammo) {
        return None;
    }
    let optional =
        |name: &[u8]| -> usize { child(root, name).filter(|&w| is_slider(e, w)).unwrap_or(0) };
    Some([
        weapons,
        ammo,
        optional(b"df_upgrades"),
        optional(b"df_perks"),
    ])
}
unsafe fn listen(e: &Engine, widget: usize, panel: usize) {
    let listeners = read::<usize>(widget + 0xb0);
    if listeners == 0 {
        return;
    }
    let Some(entries) = vector(listeners, 8, 8) else {
        return;
    };
    if !entries.into_iter().any(|at| read::<usize>(at) == panel) {
        (e.listen)(widget, panel);
    }
}
unsafe fn visible(widget: usize, value: bool) {
    if (read::<u8>(widget + 0x5b) != 0) != value {
        let set: unsafe extern "C" fn(usize, bool) =
            std::mem::transmute(read::<usize>(read::<usize>(widget) + 0x48));
        set(widget, value);
    }
}
/// Hide every present slider; absent slots are zero.
unsafe fn hide(sliders: [usize; 4]) {
    for slider in sliders {
        if slider != 0 {
            visible(slider, false);
        }
    }
}
/// Whether the perk row takes part: its companion slider is present and the
/// stock refresh left all five perk cards shown. That refresh hides them in
/// some panel modes, and the 2026-09 builds also hide every card after an
/// "exclusive" perk; either way the stock row stands, unscrolled.
unsafe fn perk_row(panel: usize, layout: &Layout, sliders: &[usize; 4]) -> bool {
    sliders[PERK] != 0
        && (0..PERK_VISIBLE).all(|i| {
            let widget = read::<usize>(panel + layout.section[PERK] + i * 8);
            widget != 0 && read::<u8>(widget + 0x5b) != 0
        })
}
/// Whether the upgrade column takes part: its companion slider is present and
/// the stock refresh left all five upgrade widgets shown. A panel mode hides
/// the whole column; the stock row then stands, unscrolled.
unsafe fn upgrade_row(panel: usize, layout: &Layout, sliders: &[usize; 4]) -> bool {
    sliders[UPGRADE] != 0
        && (0..UPGRADE_VISIBLE).all(|i| {
            let widget = read::<usize>(panel + layout.section[UPGRADE] + i * 8);
            widget != 0 && read::<u8>(widget + 0x5b) != 0
        })
}
unsafe fn sync_slider(e: &Engine, slider: usize, v: Viewport, seek: bool) {
    let maximum = v.maximum() as i32;
    let changed =
        read::<i32>(slider + 0x1b8) != maximum || read::<i32>(slider + 0x1bc) != v.offset as i32;
    write(slider + 0x1b8, maximum);
    write(slider + 0x1bc, v.offset as i32);
    if changed || seek {
        write(slider + 0x1c0, v.offset as f32);
    }
    visible(slider, maximum > 0);
    // Native slider math divides by maximum. Do not update a hidden empty range.
    if maximum > 0 {
        let rect: unsafe extern "C" fn(usize) -> *const i32 =
            std::mem::transmute(read::<usize>(read::<usize>(slider) + 0x40));
        let r = rect(slider);
        // Either orientation: the track runs along the longer axis.
        let span = (*r.add(2) - *r).max(*r.add(3) - *r.add(1)).max(1);
        write(
            slider + 0x1b0,
            ((span as usize * v.visible / v.count) as i32).clamp(16.min(span), span),
        );
        write(slider + 0x1b4, 4i32);
        (e.thumb)(slider);
    }
}
/// The four rows' records for one unit. Weapons are filtered as the stock
/// refresh filters them (a hidden weapon takes no visible index); ammunition
/// and upgrades are the raw element pointers, in order.
unsafe fn inventory(
    e: &Engine,
    panel: usize,
    layout: &Layout,
    unit: usize,
) -> Option<[Vec<usize>; 4]> {
    let ammo = vector(unit, layout.vector[AMMO], layout.stride[AMMO])?;
    let weapons = vector(unit, layout.vector[WEAPON], layout.stride[WEAPON])?;
    let mut upgrades = vector(unit, layout.vector[UPGRADE], layout.stride[UPGRADE])?;
    // The upgrade column's list (item-string elements, stride 0x20), capped at
    // the configured slot count. The default exposes every realistic count;
    // five reproduces stock (no scrollbar above the five widgets).
    upgrades.truncate(e.upgrade_slots);
    let mut perks = if layout.visible[PERK] > 0 {
        vector(unit, 0x2c8, 0x20)?
    } else {
        Vec::new()
    };
    // The perk pick row's full list (name-string records, stride 0x20), capped
    // at the configured slot count so the default five match stock.
    perks.truncate(e.perk_slots);
    let context = read::<usize>(panel + 0x118);
    if context == 0 {
        return None;
    }
    let holder = read::<usize>(context + e.context_service);
    if holder == 0 {
        return None;
    }
    let service: unsafe extern "C" fn(usize) -> usize =
        std::mem::transmute(read::<usize>(read::<usize>(holder) + 0x58));
    let service = service(holder);
    if service == 0 {
        return None;
    }
    let mut filtered = Vec::with_capacity(weapons.len());
    for record in weapons {
        let script = (e.script)(service, record);
        if script == 0 {
            return None;
        }
        if read::<u8>(script + 0x70) & 1 == 0 {
            filtered.push(record);
        }
    }
    Some([filtered, ammo, upgrades, perks])
}
/// The squad's rank, obtained as the stock perk refresh does:
/// [[panel+0x118]+context_service] -> vt+0x68 -> vt+0x38 (logic.dll's
/// LogicUtilsImpl), then its method at the build's `perk_limit` slot with the
/// squad's experience (+0x70) and the rank thresholds at [panel+0x2b8]+0x28: the
/// last threshold the experience reaches, 0 to 5. None when a link is missing.
unsafe fn squad_rank(e: &Engine, panel: usize, squad: usize) -> Option<usize> {
    type Get = unsafe extern "C" fn(usize) -> usize;
    type Limit = unsafe extern "C" fn(usize, usize, usize) -> usize;
    let method = |object: usize, slot: usize| read::<usize>(read::<usize>(object) + slot);
    let context = read::<usize>(panel + 0x118);
    let holder = if context == 0 {
        0
    } else {
        read::<usize>(context + e.context_service)
    };
    let info = read::<usize>(panel + 0x2b8);
    if holder == 0 || info == 0 {
        return None;
    }
    let services = std::mem::transmute::<usize, Get>(method(holder, 0x68))(holder);
    if services == 0 {
        return None;
    }
    let rules = std::mem::transmute::<usize, Get>(method(services, 0x38))(services);
    if rules == 0 {
        return None;
    }
    let limit = std::mem::transmute::<usize, Limit>(method(rules, e.perk_limit))(
        rules,
        read::<usize>(squad + 0x70),
        info + 0x28,
    );
    (limit <= 4096).then_some(limit)
}
/// A MSVC std::string's bytes: inline below capacity 16, else on the heap.
unsafe fn msvc_string<'a>(at: usize) -> Option<&'a [u8]> {
    let size = read::<usize>(at + 0x10);
    let data = if read::<usize>(at + 0x18) < 16 {
        at
    } else {
        read::<usize>(at)
    };
    (size <= 4096 && data != 0).then(|| std::slice::from_raw_parts(data as *const u8, size))
}
/// How many trainings the squad's type can ever take: the rows of the training
/// table whose `squads` list names the squad (its name at +0x28). The walk is
/// the one the TrainingWindow constructor makes for its list: the key from the
/// build's getter (its static pointer + 8), the table's path from the script
/// service's vt+0x20, the table from the holder's vt+0x70 -> vt+0x20 (cells at
/// +0, columns at +0x18, rows at +0x20, a header row first), and each row's
/// training by name through the script service's vt+8. The window then drops
/// what the rank or a missing parent rules out now; this counts them all.
unsafe fn trainings_available(e: &Engine, panel: usize, squad: usize) -> Option<usize> {
    type Get = unsafe extern "C" fn(usize) -> usize;
    type Path = unsafe extern "C" fn(usize, usize) -> usize;
    type Lookup = unsafe extern "C" fn(usize, usize, usize) -> usize;
    let method = |object: usize, slot: usize| read::<usize>(read::<usize>(object) + slot);
    let context = read::<usize>(panel + 0x118);
    let holder = if context == 0 {
        0
    } else {
        read::<usize>(context + e.context_service)
    };
    let name = msvc_string(squad + 0x28)?;
    if holder == 0 || name.is_empty() {
        return None;
    }
    let scripts = std::mem::transmute::<usize, Get>(method(holder, 0x58))(holder);
    let mut slot = 0usize;
    (e.training_key)(&mut slot as *mut usize as usize);
    if scripts == 0 || slot == 0 {
        return None;
    }
    let key = slot + 8;
    let path = std::mem::transmute::<usize, Path>(method(scripts, 0x20))(scripts, key);
    let tables = std::mem::transmute::<usize, Get>(method(holder, 0x70))(holder);
    if path == 0 || tables == 0 {
        return None;
    }
    let table = std::mem::transmute::<usize, Path>(method(tables, 0x20))(tables, path);
    if table == 0 {
        return None;
    }
    let (cells, columns, rows) = (
        read::<usize>(table),
        read::<usize>(table + 0x18),
        read::<usize>(table + 0x20),
    );
    if cells == 0 || !(1..=256).contains(&columns) || rows > 4096 {
        return None;
    }
    let lookup = std::mem::transmute::<usize, Lookup>(method(scripts, 0x8));
    let mut count = 0;
    for row in 1..rows {
        let cell = cells + row * columns * 0x20;
        if read::<usize>(cell + 0x10) == 0 {
            continue;
        }
        let training = lookup(scripts, key, cell);
        if training == 0 {
            continue;
        }
        if vector(training, 0x28, 0x20)?
            .into_iter()
            .any(|squad| msvc_string(squad) == Some(name))
        {
            count += 1;
        }
    }
    Some(count)
}
/// The display object the stock upgrade loop builds for one element: the script
/// service at [[panel+0x118]+context_service] -> vt+0x58, then its vt+8 called
/// with the lazily-initialised key static (+8) and the element. The stock
/// refresh runs this for the first five elements, so the key is initialised
/// whenever an element exists; a zero key or a missing link clears the card.
unsafe fn upgrade_display(e: &Engine, panel: usize, record: usize) -> usize {
    type Get = unsafe extern "C" fn(usize) -> usize;
    type Resolve = unsafe extern "C" fn(usize, usize, usize) -> usize;
    let context = read::<usize>(panel + 0x118);
    if context == 0 {
        return 0;
    }
    let holder = read::<usize>(context + e.context_service);
    if holder == 0 {
        return 0;
    }
    let service =
        std::mem::transmute::<usize, Get>(read::<usize>(read::<usize>(holder) + 0x58))(holder);
    if service == 0 {
        return 0;
    }
    let key = read::<usize>(e.upgrade_key);
    if key == 0 {
        return 0;
    }
    let resolve = std::mem::transmute::<usize, Resolve>(read::<usize>(read::<usize>(service) + 8));
    resolve(service, key + 8, record)
}
/// Draw a card without a perk as the stock refresh draws one: open (a perk may
/// go there) or locked, highlighted when it is the next one and the squad may
/// pick now. The bind, given an empty name, clears the card's perk and redraws
/// it from these.
unsafe fn blank_card(e: &Engine, widget: usize, open: bool, highlight: bool) {
    write(widget + CARD_OPEN, u8::from(open));
    write(widget + CARD_HIGHLIGHT, u8::from(highlight));
    // an empty MSVC std::string: no characters, size 0, inline capacity 15
    let empty: [usize; 4] = [0, 0, 0, 15];
    (e.perk)(widget, empty.as_ptr() as usize);
}
enum Input {
    Refresh,
    Wheel(usize, i16),
    Slider(usize),
    /// Scroll the least that brings this entry into view.
    Show(usize, usize),
}

unsafe fn hover(panel: usize, event: usize) {
    let mouse_move: Event = std::mem::transmute(read::<usize>(read::<usize>(panel) + 0x90));
    mouse_move(panel, 0, 0, event);
}

unsafe fn update(panel: usize, layout: &'static Layout, input: Input, event: usize) {
    let e = ENGINE.get().unwrap();
    let root = read::<usize>(panel + layout.root);
    let Some(sliders) = sliders(e, root) else {
        if !WARNED.with(|w| w.replace(true)) {
            (e.log)(LOG_WARN, c"squad scrolling: companion UI controls missing or incompatible; stock panels retained".as_ptr());
        }
        STATES.with(|s| s.borrow_mut().remove(&panel));
        return;
    };
    let unit = read::<usize>(panel + layout.unit);
    if unit == 0 || (layout.squad && read::<u32>(unit + 0x68) != 0) {
        STATES.with(|s| s.borrow_mut().remove(&panel));
        hide(sliders);
        return;
    }
    // The vehicle panel's own setting, and the shared upgrade-column setting,
    // turn their scrollbars off (hidden) and leave the stock rows in place.
    if !layout.squad && !e.vehicles {
        STATES.with(|s| s.borrow_mut().remove(&panel));
        hide(sliders);
        return;
    }
    let Some(mut items) = inventory(e, panel, layout, unit) else {
        STATES.with(|s| s.borrow_mut().remove(&panel));
        hide(sliders);
        return;
    };
    // A row without its companion slider, or a card the stock refresh hid,
    // leaves that row as stock drew it.
    let perks = layout.visible[PERK] > 0 && perk_row(panel, layout, &sliders);
    let upgrades = e.upgrades && upgrade_row(panel, layout, &sliders);
    // The perk row is as many cards as the squad's type has trainings, up to
    // `perk_slots`, whatever it owns: its perks, then blank cards by the stock
    // refresh's rule for its five. When the panel locks cards (+0x210), a blank
    // card below the squad's rank is open and the rest locked; otherwise every
    // blank card is open. A zero record marks a blank. Cards past a row shorter
    // than five keep the stock drawing.
    let owned = items[PERK].len();
    let mut locked_from = None;
    if perks {
        let locks = read::<u8>(panel + PANEL_LOCKS) != 0;
        let rank = if locks {
            squad_rank(e, panel, unit)
        } else {
            None
        };
        if !locks || rank.is_some() {
            locked_from = rank;
            let cards = trainings_available(e, panel, unit)
                .map_or(e.perk_slots, |trainings| trainings.min(e.perk_slots));
            items[PERK].resize(cards.max(owned), 0);
        }
    }
    // The upgrade column is `upgrade_slots` long whatever the unit owns: its
    // upgrades, then blank cards (zero records). The stock drag-start chooser
    // targets the first visible card with no bound upgrade, so scrolling a
    // blank card into view is what lets a sixth and later upgrade be dropped.
    if upgrades {
        let owned = items[UPGRADE].len();
        items[UPGRADE].resize(e.upgrade_slots.max(owned), 0);
    }
    // Drop the state-map borrow before invoking any native code; redraws can
    // dispatch synchronous events. Reentrant callbacks retain stock behavior.
    let mut state = STATES
        .with(|s| s.borrow().get(&panel).copied())
        .unwrap_or_else(|| State::new(layout));
    if state.unit != unit || state.root != root || !ptr::eq(state.layout, layout) {
        state = State::new(layout);
        state.unit = unit;
        state.root = root;
    }
    for (view, list) in state.views.iter_mut().zip(&items) {
        view.resize(list.len());
    }
    match input {
        Input::Wheel(section, delta) => state.views[section].wheel(delta),
        Input::Slider(section) => state.views[section].seek(read(sliders[section] + 0x1bc)),
        Input::Show(section, index) => {
            let view = &mut state.views[section];
            if index < view.offset {
                view.seek(index as i32);
            } else if index >= view.offset + view.visible {
                view.seek((index + 1 - view.visible) as i32);
            }
        }
        Input::Refresh => (),
    }
    if event != 0 {
        let mut outside = [0u8; 40];
        write(outside.as_mut_ptr() as usize + 0x18, [i32::MIN / 2; 2]);
        hover(panel, outside.as_ptr() as usize);
    }
    if sliders[PERK] != 0 && !perks {
        visible(sliders[PERK], false);
    }
    if sliders[UPGRADE] != 0 && !upgrades {
        visible(sliders[UPGRADE], false);
    }
    let may_pick = read::<u8>(panel + PANEL_MAY_PICK) != 0;
    for section in 0..4 {
        if layout.visible[section] == 0
            || (section == UPGRADE && !upgrades)
            || (section == PERK && !perks)
        {
            continue;
        }
        for i in 0..layout.visible[section] {
            let widget = read::<usize>(panel + layout.section[section] + i * 8);
            if widget == 0 {
                continue;
            }
            let index = state.views[section].offset + i;
            let record = items[section].get(index).copied();
            match (section, record) {
                (WEAPON, _) => {
                    write(widget + 0x200, record.unwrap_or(0));
                    (e.weapon)(widget);
                }
                (AMMO, _) => (e.ammo)(widget, record.unwrap_or(0)),
                // The stock loop binds the resolved display, or null for an
                // empty slot.
                (UPGRADE, _) => {
                    let display = record
                        .filter(|&r| r != 0)
                        .map_or(0, |r| upgrade_display(e, panel, r));
                    (e.upgrade)(widget, display);
                }
                // Past the perk row, which happens only while it is unscrolled
                // (it scrolls only past five cards): the stock refresh drew that
                // card, locked or open, and it stays as drawn.
                (PERK, None) => {}
                // The first blank card, when open, is the one a pick goes to.
                (PERK, Some(0)) => {
                    let open = locked_from.is_none_or(|rank| index < rank);
                    blank_card(e, widget, open, open && index == owned && may_pick);
                }
                (PERK, Some(record)) => {
                    // as the stock refresh does before binding a perk, so a card
                    // that was open is not left clickable
                    write(widget + CARD_OPEN, 0u16);
                    (e.perk)(widget, record);
                }
                _ => {}
            }
            listen(e, widget, panel);
        }
        if sliders[section] != 0 {
            listen(e, sliders[section], panel);
            sync_slider(
                e,
                sliders[section],
                state.views[section],
                matches!(input, Input::Wheel(..)),
            );
        }
    }
    STATES.with(|s| s.borrow_mut().insert(panel, state));
    if event != 0 {
        hover(panel, event);
    }
}

unsafe fn run_refresh(
    panel: usize,
    layout: &'static Layout,
    original: &AtomicPtr<core::ffi::c_void>,
) {
    let original: One = std::mem::transmute(original.load(Ordering::Acquire));
    original(panel);
    if ACTIVE.load(Ordering::Acquire) {
        if let Some(_guard) = Guard::enter() {
            update(panel, layout, Input::Refresh, 0);
        }
    }
}
pub(super) unsafe extern "C" fn refresh(panel: usize) {
    run_refresh(panel, &SQUAD, &ORIGINAL);
}
pub(super) unsafe extern "C" fn refresh_vehicle(panel: usize) {
    run_refresh(panel, &VEHICLE, &ORIGINAL_VEHICLE);
}
pub(super) unsafe extern "C" fn destroy(panel: usize, flags: u32) -> usize {
    STATES.with(|s| s.borrow_mut().remove(&panel));
    let e = ENGINE.get().unwrap();
    // Each panel class has its own deleting destructor; both slots point here,
    // so the original is chosen from the object's own vtable before it runs.
    let original = if read::<usize>(panel) == e.vehicle_vtable {
        e.vehicle_destroy
    } else {
        e.destroy
    };
    original(panel, flags)
}
/// Which panel class a vtable pointer names, so one dispatch detour serves both
/// vtables. The panel's own pointer is untouched (only the vtable's slots are
/// patched), so it still names the original class vtable.
unsafe fn layout_of(e: &Engine, panel: usize) -> Option<&'static Layout> {
    match read::<usize>(panel) {
        vt if vt == e.squad_vtable => Some(&SQUAD),
        vt if vt == e.vehicle_vtable => Some(&VEHICLE),
        _ => None,
    }
}
unsafe fn section(layout: &Layout, panel: usize, mut widget: usize) -> Option<usize> {
    // A card's image/text child can be the source. Bound the parent traversal.
    for _ in 0..16 {
        if widget == 0 {
            break;
        }
        for section in 0..4 {
            if layout.visible[section] == 0 {
                continue;
            }
            for slot in 0..layout.visible[section] {
                if read::<usize>(panel + layout.section[section] + slot * 8) == widget {
                    return Some(section);
                }
            }
        }
        widget = read(widget + 0x38);
    }
    None
}
pub(super) unsafe extern "C" fn dispatch(panel: usize, source: usize, arg: usize, event: usize) {
    let e = ENGINE.get().unwrap();
    if event != 0 && ACTIVE.load(Ordering::Acquire) {
        let message = read::<u32>(event);
        if message == 0x20a || message == 0x481 {
            if let Some(_guard) = Guard::enter() {
                if let Some(layout) = layout_of(e, panel) {
                    if !layout.squad && !e.vehicles {
                        (e.dispatch)(panel, source, arg, event);
                        return;
                    }
                    if let Some(sliders) = sliders(e, read(panel + layout.root)) {
                        let input = if message == 0x481 {
                            sliders
                                .iter()
                                .position(|&w| w != 0 && w == source)
                                .map(Input::Slider)
                        } else {
                            // a stock row keeps its own wheel handling
                            section(layout, panel, source)
                                .filter(|&s| match s {
                                    UPGRADE => e.upgrades && upgrade_row(panel, layout, &sliders),
                                    PERK => perk_row(panel, layout, &sliders),
                                    _ => true,
                                })
                                .map(|s| Input::Wheel(s, read(event + 10)))
                        };
                        if let Some(input) = input {
                            // Do not rebind a card while an item drag holds the button.
                            if message != 0x20a || GetKeyState(1) >= 0 {
                                update(panel, layout, input, event);
                            }
                            return;
                        }
                    }
                }
            }
        }
    }
    (e.dispatch)(panel, source, arg, event);
}

/// An upgrade record's conflict tags (its `slot_type`s): the sorted
/// `std::vector<std::string>` at +0x30 that the stock chooser intersects
/// (`0x960f0`). Two upgrades sharing a tag are exclusive.
unsafe fn tags<'a>(record: usize) -> Vec<&'a [u8]> {
    vector(record, 0x30, 0x20)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|at| msvc_string(at))
        .collect()
}
/// Before the stock chooser picks a card for a dragged upgrade, scroll the
/// column so the card it should pick is in view. The chooser walks the five
/// visible cards and takes the first empty one or the first bound to an upgrade
/// the item conflicts with (a shared tag), and refuses an item a visible card
/// already holds. With scrolling, that upgrade can be out of view, and the drop
/// then replaces it while an empty card was highlighted. So: the first
/// installed upgrade that is this item or shares a tag with it is scrolled into
/// view (it precedes every empty card, so the chooser picks it, or refuses a
/// duplicate); otherwise the first empty card is.
unsafe fn show_target(panel: usize, layout: &'static Layout, item: usize) {
    let e = ENGINE.get().unwrap();
    if !ACTIVE.load(Ordering::Acquire) || item == 0 || read::<u32>(item + 0x2c) != 0 {
        return;
    }
    if (!layout.squad && !e.vehicles) || !e.upgrades {
        return;
    }
    let Some(_guard) = Guard::enter() else { return };
    let unit = read::<usize>(panel + layout.unit);
    if unit == 0 {
        return;
    }
    let Some(entries) = vector(unit, layout.vector[UPGRADE], layout.stride[UPGRADE]) else {
        return;
    };
    let wanted = tags(item);
    let conflict = entries.iter().take(e.upgrade_slots).position(|&entry| {
        let display = upgrade_display(e, panel, entry);
        display == item || (display != 0 && tags(display).iter().any(|t| wanted.contains(t)))
    });
    let target = conflict.unwrap_or(entries.len());
    if target < e.upgrade_slots {
        update(panel, layout, Input::Show(UPGRADE, target), 0);
    }
}
type Choose = unsafe extern "C" fn(usize, usize, usize) -> usize;
pub(super) unsafe extern "C" fn choose_squad(panel: usize, out: usize, item: usize) -> usize {
    show_target(panel, &SQUAD, item);
    let original: Choose = std::mem::transmute(ORIGINAL_SQUAD_CHOOSER.load(Ordering::Acquire));
    original(panel, out, item)
}
pub(super) unsafe extern "C" fn choose_vehicle(panel: usize, out: usize, item: usize) -> usize {
    show_target(panel, &VEHICLE, item);
    let original: Choose = std::mem::transmute(ORIGINAL_VEHICLE_CHOOSER.load(Ordering::Acquire));
    original(panel, out, item)
}
/// The panel, layout and row a companion slider belongs to.
unsafe fn owner(e: &Engine, slider: usize) -> Option<(usize, &'static Layout, usize)> {
    let panels: Vec<_> = STATES.with(|s| s.borrow().iter().map(|(&p, s)| (p, s.layout)).collect());
    panels.into_iter().find_map(|(panel, layout)| {
        let controls = sliders(e, read(panel + layout.root))?;
        let section = controls.iter().position(|&w| w != 0 && w == slider)?;
        Some((panel, layout, section))
    })
}
/// The slider's rect when it is taller than wide: the companion mod's upgrade
/// sliders. The stock slider lays itself out and maps the pointer along x only;
/// no shipped slider is vertical, so a tall one is always ours.
unsafe fn vertical(slider: usize) -> Option<[i32; 4]> {
    let rect: unsafe extern "C" fn(usize) -> *const [i32; 4] =
        std::mem::transmute(read::<usize>(read::<usize>(slider) + 0x40));
    let r = ptr::read_unaligned(rect(slider));
    (r[3] - r[1] > r[2] - r[0]).then_some(r)
}
/// A widget's geometry as widget vt+0x28 takes it (and vt+0x40 returns it, at
/// widget+0x120): its rect, its corner quad (a `std::vector` the setter copies)
/// and a flags word.
#[repr(C)]
struct Geometry {
    rect: [i32; 4],
    begin: *const [i32; 2],
    end: *const [i32; 2],
    capacity: *const [i32; 2],
    flags: i32,
}
/// The thumb layout hook: the stock layout for a horizontal slider; for a
/// vertical one, the same layout along y. The thumb child (+0x1a8) is placed at
/// value (+0x1c0) / maximum (+0x1b8) of the track less its length (+0x1b0),
/// centred across the track at its thickness (+0x1b4), as stock does along x.
pub(super) unsafe extern "C" fn thumb_layout(slider: usize) {
    let thumb = read::<usize>(slider + 0x1a8);
    if thumb != 0 {
        if let Some(r) = vertical(slider) {
            let length = read::<i32>(slider + 0x1b0);
            let thickness = read::<i32>(slider + 0x1b4);
            let maximum = read::<i32>(slider + 0x1b8);
            let along = if maximum != 0 {
                read::<f32>(slider + 0x1c0) / maximum as f32 * (r[3] - r[1] - length) as f32
            } else {
                0.0
            };
            let x = r[0] + ((r[2] - r[0] - thickness) as f32 * 0.5) as i32;
            let y = r[1] + along as i32;
            let quad = [
                [x, y],
                [x + thickness, y],
                [x + thickness, y + length],
                [x, y + length],
            ];
            let get: unsafe extern "C" fn(usize) -> usize =
                std::mem::transmute(read::<usize>(read::<usize>(thumb) + 0x40));
            let geometry = Geometry {
                rect: [x, y, x + thickness, y + length],
                begin: quad.as_ptr(),
                end: quad.as_ptr().add(quad.len()),
                capacity: quad.as_ptr().add(quad.len()),
                flags: read::<i32>(get(thumb) + 0x28),
            };
            let set: unsafe extern "C" fn(usize, *const Geometry) =
                std::mem::transmute(read::<usize>(read::<usize>(thumb) + 0x28));
            set(thumb, &geometry);
            return;
        }
    }
    let original: One = std::mem::transmute(ORIGINAL_THUMB.load(Ordering::Acquire));
    original(slider);
}
/// A vertical slider's value under the pointer (event+0x18, screen), as the
/// stock drag maps x: the thumb's centre follows the pointer over the track's
/// screen rect (+0x154 top, +0x15c height), in the track's scale.
unsafe fn drag(panel: usize, layout: &'static Layout, section: usize, slider: usize, event: usize) {
    let Some(r) = vertical(slider) else { return };
    let top = read::<i32>(slider + 0x154) as f32;
    let height = read::<i32>(slider + 0x15c) as f32;
    let length = read::<i32>(slider + 0x1b0) as f32 * height / (r[3] - r[1]).max(1) as f32;
    let maximum = read::<i32>(slider + 0x1b8);
    if height - length <= 0.0 || maximum <= 0 {
        return;
    }
    let at = (read::<i32>(event + 0x1c) as f32 - top - length * 0.5) / (height - length);
    let value = at.clamp(0.0, 1.0) * maximum as f32;
    // The float is the thumb's smooth position; the release snaps it to the
    // integer, as stock does.
    write(slider + 0x1c0, value);
    write(slider + 0x1bc, value.round() as i32);
    update(panel, layout, Input::Slider(section), 0);
}
pub(super) unsafe extern "C" fn slider_dispatch(
    ctrl: usize,
    source: usize,
    arg: usize,
    event: usize,
) {
    let e = ENGINE.get().unwrap();
    let message = if event != 0 { read::<u32>(event) } else { 0 };
    if message == 0x20a && ACTIVE.load(Ordering::Acquire) {
        if let Some(_guard) = Guard::enter() {
            if let Some((panel, layout, section)) = owner(e, read(ctrl + 8)) {
                // Normalize the stock horizontal slider's reversed wheel
                // direction, and retain sub-tick deltas as on the cards.
                if GetKeyState(1) >= 0 {
                    update(
                        panel,
                        layout,
                        Input::Wheel(section, read(event + 10)),
                        event,
                    );
                }
                return;
            }
        }
    }
    // A vertical slider: the press runs stock (the drag flag at ctrl+0x30 and
    // the pointer capture; a track click's x value is corrected at once), then
    // the value comes from the pointer's y. Moves while dragging never reach
    // the stock x mapping. The release runs stock, snapping and re-laying the
    // thumb through the layout hook.
    if (message == 0x200 || message == 0x201) && ACTIVE.load(Ordering::Acquire) {
        let slider = read::<usize>(ctrl + 8);
        if slider != 0 && vertical(slider).is_some() {
            if let Some((panel, layout, section)) = owner(e, slider) {
                if message == 0x201 {
                    (e.slider_dispatch)(ctrl, source, arg, event);
                }
                if read::<u8>(ctrl + 0x30) != 0 {
                    if let Some(_guard) = Guard::enter() {
                        drag(panel, layout, section, slider, event);
                    }
                }
                return;
            }
        }
    }
    (e.slider_dispatch)(ctrl, source, arg, event);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_malformed_vector_ranges() {
        assert_eq!(count(0, 0, 0x48), Some(0));
        assert_eq!(count(0x1000, 0x1000 + 7 * 0x48, 0x48), Some(7));
        assert_eq!(count(0x1000, 0xff0, 0x48), None);
        assert_eq!(count(0x1000, 0x1001, 0x48), None);
        assert_eq!(count(0, 0x48, 0x48), None);
        assert_eq!(count(0x1000, 0x1000 + 4097 * 0x48, 0x48), None);
    }
}
