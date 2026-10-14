//! The info panel's experience meter, laid out for the squad's level count.
//!
//! The stock panel layout places four `exp_lvl_separator*` progress bars over
//! the `exp_progress` bar, one per boundary between the six stock levels. After
//! each panel update this spreads the separators evenly over the meter, one per
//! boundary between the squad's levels, and makes the extra ones it needs as
//! copies of the first. A copy is a child of the separators' parent named
//! `exp_lvl_dyn_NN`, so a later update finds it again; the game frees it with
//! its parent. Separators beyond the squad's count are hidden.
use crate::native::{log_once, MsvcString};
use defiance_api::{LOG_INFO, LOG_WARN};
use std::sync::OnceLock;

/// The panel's squad state; the update skips the meter while its
/// [`STATE_BUSY`] is set.
const PANEL_STATE: usize = 0x288;
const STATE_BUSY: usize = 0x68;
/// The squad's info the update stores, its byte that hides the meter, and its
/// vector of `u32` level thresholds, one per level.
const PANEL_INFO: usize = 0x2b8;
const INFO_HIDDEN: usize = 0x1cb;
const INFO_LEVELS_BEGIN: usize = 0x28;
const INFO_LEVELS_END: usize = 0x30;
/// The `exp_progress` bar and the vector of separators.
const PANEL_PROGRESS: usize = 0x278;
const PANEL_SEPARATORS_BEGIN: usize = 0x250;
const PANEL_SEPARATORS_END: usize = 0x258;

/// A widget's id string, parent, children vector and draw order.
const WIDGET_ID: usize = 0x18;
const WIDGET_PARENT: usize = 0x38;
const WIDGET_CHILDREN_BEGIN: usize = 0x40;
const WIDGET_CHILDREN_END: usize = 0x48;
const WIDGET_ORDER: usize = 0x160;
/// Virtual slots: the region getter, show, and the subtree order seed.
const SLOT_REGION: usize = 0x40;
const SLOT_SHOW: usize = 0x48;
const SLOT_SEED: usize = 0xa0;

/// A progress bar's image child, fill, fill direction and inverted flag.
const BAR_IMAGE: usize = 0x1a0;
const BAR_FILL: usize = 0x1a8;
const BAR_DIRECTION: usize = 0x1ac;
const BAR_INVERTED: usize = 0x1c8;
/// An image's texture name.
const IMAGE_TEXTURE: usize = 0x60;

/// The id prefix of the separators this module makes.
const DYNAMIC_PREFIX: &[u8] = b"exp_lvl_dyn_";
/// The most levels a meter is laid out for; a larger count is not a level
/// vector, and the meter is left as the game drew it.
const MOST_LEVELS: usize = 65;

type Factory = unsafe extern "C" fn(*mut usize, *const Record) -> *mut usize;
type SetImage = unsafe extern "C" fn(usize, usize);
type Clip = unsafe extern "C" fn(usize);
type Attach = unsafe extern "C" fn(usize, *mut usize) -> usize;
type Bind = unsafe extern "C" fn(usize);
type MoveWidget = unsafe extern "C" fn(usize, *const [i32; 2]);

/// The game functions the layout calls.
pub(crate) struct Game {
    pub(crate) factory: Factory,
    pub(crate) image: SetImage,
    pub(crate) clip: Clip,
    pub(crate) attach: Attach,
    pub(crate) bind: Bind,
    pub(crate) move_widget: MoveWidget,
}

impl Game {
    /// The functions at these addresses.
    ///
    /// # Safety
    /// Each address must be the game function its field names.
    pub(crate) unsafe fn at(
        factory: usize,
        image: usize,
        clip: usize,
        attach: usize,
        bind: usize,
        move_widget: usize,
    ) -> Self {
        Game {
            factory: core::mem::transmute::<usize, Factory>(factory),
            image: core::mem::transmute::<usize, SetImage>(image),
            clip: core::mem::transmute::<usize, Clip>(clip),
            attach: core::mem::transmute::<usize, Attach>(attach),
            bind: core::mem::transmute::<usize, Bind>(bind),
            move_widget: core::mem::transmute::<usize, MoveWidget>(move_widget),
        }
    }
}

pub(crate) static GAME: OnceLock<Game> = OnceLock::new();

/// The widget record the factory builds a widget from.
#[repr(C)]
pub(crate) struct Record {
    id: MsvcString,
    kind: MsvcString,
    points: [*const [i32; 2]; 3],
    anchors: MsvcString,
    tooltip: MsvcString,
    /// An empty `std::map`: its head node and size.
    map_head: *mut MapNode,
    map_size: usize,
}

const _: () = assert!(core::mem::size_of::<Record>() == 0xa8);

/// An MSVC `std::map` node: left, parent, right, colour, isnil, then the value.
#[repr(C, align(8))]
struct MapNode([u8; 0x60]);

/// The head node of an empty map: its links point to itself, it is black and
/// nil. The factory only reads it, so one serves every record.
fn empty_map_head() -> *mut MapNode {
    static HEAD: OnceLock<usize> = OnceLock::new();
    *HEAD.get_or_init(|| {
        let node = Box::into_raw(Box::new(MapNode([0; 0x60])));
        let me = (node as usize).to_le_bytes();
        // SAFETY: `node` is a fresh 0x60-byte allocation.
        let bytes = unsafe { &mut (*node).0 };
        for link in 0..3 {
            bytes[link * 8..link * 8 + 8].copy_from_slice(&me);
        }
        bytes[0x18] = 1;
        bytes[0x19] = 1;
        node as usize
    }) as *mut MapNode
}

unsafe fn q(address: usize) -> usize {
    *(address as *const usize)
}

unsafe fn d(address: usize) -> i32 {
    *(address as *const i32)
}

unsafe fn slot(widget: usize, offset: usize) -> usize {
    q(q(widget) + offset)
}

unsafe fn region(widget: usize) -> [i32; 4] {
    let getter: unsafe extern "C" fn(usize) -> *const [i32; 4] =
        core::mem::transmute(slot(widget, SLOT_REGION));
    *getter(widget)
}

unsafe fn show(widget: usize, shown: bool) {
    let show: unsafe extern "C" fn(usize, u8) = core::mem::transmute(slot(widget, SLOT_SHOW));
    show(widget, shown as u8);
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

/// The pointers in the vector `[begin, end)`.
unsafe fn pointers(begin: usize, end: usize) -> Vec<usize> {
    let (begin, end) = (q(begin), q(end));
    if begin == 0 || end < begin {
        return Vec::new();
    }
    core::slice::from_raw_parts(begin as *const usize, (end - begin) / 8)
        .iter()
        .copied()
        .filter(|&widget| widget != 0)
        .collect()
}

/// Lays out the infantry experience meter after Core finishes the panel refresh.
pub(crate) unsafe extern "C" fn panel_refresh(
    panel: *mut core::ffi::c_void,
    _context: *mut core::ffi::c_void,
) {
    if panel.is_null() {
        return;
    }
    if let Some(game) = GAME.get() {
        lay_out(game, panel as usize);
    }
}

unsafe fn lay_out(game: &Game, panel: usize) {
    let state = q(panel + PANEL_STATE);
    if state == 0 || d(state + STATE_BUSY) != 0 {
        return;
    }
    let info = q(panel + PANEL_INFO);
    let progress = q(panel + PANEL_PROGRESS);
    if info == 0 || progress == 0 {
        return;
    }
    let (begin, end) = (q(info + INFO_LEVELS_BEGIN), q(info + INFO_LEVELS_END));
    let levels = end.wrapping_sub(begin) / 4;
    if begin == 0 || end < begin || !(1..=MOST_LEVELS).contains(&levels) {
        return;
    }
    let shown = *((info + INFO_HIDDEN) as *const u8) == 0;
    let stock = pointers(panel + PANEL_SEPARATORS_BEGIN, panel + PANEL_SEPARATORS_END);
    let Some(&template) = stock.first() else {
        return;
    };
    let parent = q(template + WIDGET_PARENT);
    if parent == 0 {
        return;
    }
    let stock_count = stock.len();
    let mut separators = stock;
    separators.extend(
        pointers(parent + WIDGET_CHILDREN_BEGIN, parent + WIDGET_CHILDREN_END)
            .into_iter()
            .filter(|&child| msvc_str(child + WIDGET_ID).starts_with(DYNAMIC_PREFIX)),
    );
    // Level 0 is the meter's left end and the last level its right end: there
    // are `levels - 1` segments, with a separator between each two.
    let segments = levels - 1;
    let needed = segments.saturating_sub(1);
    let found = separators.len();
    while separators.len() < needed {
        let Some(made) = make(game, template, parent, separators.len()) else {
            log_once(
                LOG_WARN,
                format!(
                    "experience levels: could not make meter separator {}; the meter shows none",
                    separators.len() + 1
                ),
            );
            for &separator in &separators {
                show(separator, false);
            }
            return;
        };
        separators.push(made);
    }
    if separators.len() > found {
        log_once(
            LOG_INFO,
            format!(
                "experience levels: meter laid out for {levels} levels ({} separators made)",
                separators.len() - stock_count
            ),
        );
    }
    let [x0, _, x1, _] = region(progress);
    let [tx0, ty0, tx1, _] = region(template);
    let width = tx1 - tx0;
    let segments = segments as i64;
    for (k, &separator) in separators.iter().enumerate() {
        if k >= needed {
            show(separator, false);
            continue;
        }
        let boundary = (k + 1) as i64;
        let centre =
            x0 + ((x1 - x0) as i64 * boundary * 2 + segments).div_euclid(2 * segments) as i32;
        let [sx0, sy0, ..] = region(separator);
        let delta = [centre - width / 2 - sx0, ty0 - sy0];
        if delta != [0, 0] {
            (game.move_widget)(separator, &delta);
            (game.clip)(separator);
        }
        show(separator, shown);
    }
}

/// A new separator copied from `template`, attached to `parent` as the
/// `index`th separator, or `None` when the factory made nothing.
unsafe fn make(game: &Game, template: usize, parent: usize, index: usize) -> Option<usize> {
    let [x0, y0, x1, y1] = region(template);
    let points = [[x0, y0], [x1, y1]];
    let mut heap = Vec::new();
    let record = Record {
        id: MsvcString::new(format!("exp_lvl_dyn_{index:02}").as_bytes(), &mut heap),
        kind: MsvcString::new(b"progress_bar", &mut heap),
        points: [
            points.as_ptr(),
            points.as_ptr().add(points.len()),
            points.as_ptr().add(points.len()),
        ],
        anchors: MsvcString::new(b"", &mut heap),
        tooltip: MsvcString::new(b"", &mut heap),
        map_head: empty_map_head(),
        map_size: 0,
    };
    let mut handle = 0usize;
    (game.factory)(&mut handle, &record);
    let bar = handle;
    if bar == 0 {
        return None;
    }
    for offset in [BAR_FILL, BAR_DIRECTION] {
        *((bar + offset) as *mut u32) = *((template + offset) as *const u32);
    }
    *((bar + BAR_INVERTED) as *mut u8) = *((template + BAR_INVERTED) as *const u8);
    let image = q(template + BAR_IMAGE);
    if image != 0 {
        (game.image)(bar, image + IMAGE_TEXTURE);
    }
    (game.bind)(bar);
    let child = (game.attach)(parent, &mut handle);
    if child == 0 {
        return None;
    }
    let seed: unsafe extern "C" fn(usize, i32) = core::mem::transmute(slot(child, SLOT_SEED));
    seed(child, d(template + WIDGET_ORDER));
    (game.clip)(child);
    Some(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_map_head_links_to_itself() {
        let head = empty_map_head();
        let bytes = unsafe { &(*head).0 };
        for link in 0..3 {
            assert_eq!(
                usize::from_le_bytes(bytes[link * 8..link * 8 + 8].try_into().unwrap()),
                head as usize
            );
        }
        assert_eq!((bytes[0x18], bytes[0x19]), (1, 1));
    }
}
