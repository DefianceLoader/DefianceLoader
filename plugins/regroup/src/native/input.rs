//! Hotkey configuration and the input-dispatch hook that starts regroup
//! and restore.
use super::*;

pub(super) static INPUT: AtomicUsize = AtomicUsize::new(0);
pub(super) static BUSY: AtomicBool = AtomicBool::new(false);
pub(super) static INPUT_SEEN: AtomicBool = AtomicBool::new(false);

#[link(name = "user32")]
extern "system" {
    fn GetForegroundWindow() -> usize;
    fn GetWindowThreadProcessId(window: usize, process: *mut u32) -> u32;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcessId() -> u32;
}

// The game dispatch passes pointers to the Win32 message and its parameters.
pub(super) type InputDispatch = unsafe extern "C" fn(usize, *const u32, *const usize, *const usize);

pub(super) static HOTKEYS: OnceLock<[crate::hotkeys::Chord; 2]> = OnceLock::new();

pub(super) unsafe fn configure_hotkeys(api: &Api) -> Result<(), String> {
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
        log(LOG_DEBUG, &format!("{name} = {text}"));
        chords.push(chord);
    }
    if chords[0] == chords[1] {
        return Err("regroup and restore hotkeys must differ".into());
    }
    HOTKEYS
        .set([chords[0], chords[1]])
        .map_err(|_| "hotkeys already initialized".into())
}

pub(super) unsafe fn input_manager(input: usize) -> Result<usize, &'static str> {
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

pub(super) unsafe extern "C" fn input_dispatch(
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
        log(LOG_DEBUG, "game input dispatch hook reached");
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
        LOG_DEBUG,
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
            LOG_DEBUG,
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
            LOG_DEBUG,
            &format!(
                "regroup limits: {} soldiers, {} ammunition types",
                limits().soldiers,
                ammo_limit()
            ),
        );
        match input_manager(input).and_then(|manager| plan(manager).map(|p| (manager, p))) {
            Ok((manager, p)) => {
                log(
                    LOG_DEBUG,
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
