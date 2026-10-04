//! The available-training chooser keeps its stock cards and actions in one
//! row. Hidden cards retain their bindings for the game's own click handler.
use super::*;

pub(super) type Place = unsafe extern "C" fn(usize, i32, usize) -> i32;

#[derive(Clone, Copy)]
struct Choice {
    root: usize,
    first: usize,
    slider: usize,
    view: Viewport,
}
thread_local! {
    static CHOICES: RefCell<HashMap<usize, Choice>> = RefCell::new(HashMap::new());
}

unsafe fn cards(window: usize) -> Option<Vec<usize>> {
    Some(
        vector(window, 0x1d8, 8)?
            .into_iter()
            .map(|at| read(at))
            .collect(),
    )
}

unsafe fn setup(window: usize) -> Option<Choice> {
    let e = ENGINE.get()?;
    let root = read::<usize>(window + 0x128);
    let first = read::<usize>(window + 0x148);
    if root == 0 || first == 0 {
        return None;
    }
    let slider = child(root, b"df_trainings").filter(|&w| is_slider(e, w))?;
    let items = cards(window)?;
    let card = *items.first()?;
    if card == 0 || items.iter().any(|&w| w == 0) {
        return None;
    }
    let rect = |widget: usize| {
        let get: unsafe extern "C" fn(usize) -> *const [i32; 4] =
            std::mem::transmute(read::<usize>(read::<usize>(widget) + 0x40));
        ptr::read_unaligned(get(widget))
    };
    let row = rect(first);
    let card = rect(card);
    let pitch = card[2].checked_sub(card[0])?.checked_add(10)?;
    let width = row[2].checked_sub(row[0])?;
    if pitch <= 10 || width < pitch {
        return None;
    }
    let mut state = CHOICES
        .with(|s| s.borrow().get(&window).copied())
        .unwrap_or(Choice {
            root,
            first,
            slider,
            view: Viewport::new((width / pitch) as usize),
        });
    if state.root != root || state.first != first || state.slider != slider {
        state = Choice {
            root,
            first,
            slider,
            view: Viewport::new((width / pitch) as usize),
        };
    }
    state.view.visible = (width / pitch) as usize;
    state.view.resize(items.len());
    Some(state)
}

unsafe fn apply(window: usize, state: Choice, seek: bool) {
    let e = ENGINE.get().unwrap();
    let Some(items) = cards(window) else {
        return;
    };
    // Hide before moving so the second row has no visible or clickable cards.
    for &card in &items {
        visible(card, false);
    }
    (e.training_layout)(window, state.view.offset as i32, state.first);
    for &card in items
        .iter()
        .skip(state.view.offset)
        .take(state.view.visible)
    {
        visible(card, true);
    }
    listen(e, state.slider, window);
    sync_slider(e, state.slider, state.view, seek);
    CHOICES.with(|s| s.borrow_mut().insert(window, state));
}

/// Stock show takes the squad, payment mode, currency and selection callback.
/// Run it first so the card vector and every training action are fully bound.
pub(crate) unsafe extern "C" fn training_show(
    window: usize,
    squad: usize,
    mode: u8,
    currency: i32,
    callback: usize,
) {
    type Show = unsafe extern "C" fn(usize, usize, u8, i32, usize);
    let original: Show =
        std::mem::transmute(super::super::ORIGINAL_TRAINING_SHOW.load(Ordering::Acquire));
    original(window, squad, mode, currency, callback);
    if ACTIVE.load(Ordering::Acquire) {
        if let Some(_guard) = Guard::enter() {
            forget(window);
            if let Some(state) = setup(window) {
                apply(window, state, true);
            } else {
                let root = read::<usize>(window + 0x128);
                if root != 0 {
                    if let Some(slider) = child(root, b"df_trainings")
                        .filter(|&w| is_slider(ENGINE.get().unwrap(), w))
                    {
                        visible(slider, false);
                    }
                }
            }
        }
    }
}

pub(super) unsafe fn forget(window: usize) {
    CHOICES.with(|s| s.borrow_mut().remove(&window));
}

pub(super) unsafe fn wheel_slider(slider: usize, event: usize) -> bool {
    let owner = CHOICES.with(|s| {
        s.borrow()
            .iter()
            .find_map(|(&w, c)| (c.slider == slider).then_some(w))
    });
    owner.is_some_and(|window| input(window, slider, event))
}

pub(super) unsafe fn input(window: usize, source: usize, event: usize) -> bool {
    if event == 0 {
        return false;
    }
    let message = read::<u32>(event);
    if message != 0x20a && message != 0x481 {
        return false;
    }
    let Some(mut state) = setup(window) else {
        return false;
    };
    if source == state.slider {
        if message == 0x481 {
            state.view.seek(read(state.slider + 0x1bc));
        } else {
            state.view.wheel(read(event + 10));
        }
    } else if message == 0x20a {
        let Some(items) = cards(window) else {
            return false;
        };
        let mut widget = source;
        let mut found = false;
        for _ in 0..16 {
            if widget == 0 {
                break;
            }
            if items.contains(&widget) || widget == state.first {
                found = true;
                break;
            }
            widget = read(widget + 0x38);
        }
        if !found {
            return false;
        }
        state.view.wheel(read(event + 10));
    } else {
        return false;
    }
    if message != 0x20a || GetKeyState(1) >= 0 {
        let mut outside = [0u8; 40];
        write(outside.as_mut_ptr() as usize + 0x18, [i32::MIN / 2; 2]);
        hover(window, outside.as_ptr() as usize);
        apply(window, state, message == 0x20a);
        hover(window, event);
    }
    true
}
