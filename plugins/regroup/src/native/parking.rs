//! Hooks that keep an emptied original squad parked (alive but inert and
//! hidden) while [`history`] can still restore soldiers to it.
use super::*;

pub(super) static CLEANUP_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub(super) static UPDATE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub(super) static HOVER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

// SquadHoverIcon::update, also called by PlayerSquadHoverIcon::update.
// Its root widget is +140; virtual +48 is the stock visibility setter used
// by HoverIcon::update. Do not retain UI pointers or overwrite its hide flags:
// the next populated update must resume normal visibility/position handling.
pub(super) unsafe extern "C" fn update_hover(icon: usize) {
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

pub(super) unsafe extern "C" fn cleanup_empty(ai: usize) {
    if !history::park_if_needed(ai) {
        std::mem::transmute::<usize, Unary>(CLEANUP_ORIGINAL.load(Ordering::Relaxed))(ai);
    }
}
pub(super) unsafe extern "C" fn update_squad(ai: usize, elapsed: f32) {
    if !history::park_if_needed(ai) {
        std::mem::transmute::<usize, unsafe extern "C" fn(usize, f32)>(
            UPDATE_ORIGINAL.load(Ordering::Relaxed),
        )(ai, elapsed);
    }
}
