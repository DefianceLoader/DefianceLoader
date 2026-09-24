//! Keep the game offline while gameplay plugins are active.
//!
//! The gameplay plugins change local simulation and send nothing over the
//! network, so an online game with one active would desync. The loader lists
//! the active plugins that do not declare themselves multiplayer-safe
//! (`defiance.loader` / `multiplayer`); while that list is not empty, the game's
//! connection to its lobby server (`Leonardo::Network::connectToLobbyServer`,
//! the `lobby_connect` site in `tools/icon.py`) fails at once, and the log names
//! them. Skirmish and the campaign never make that connection.
//!
//! The game's multiplayer error window shows the result's message as a string
//! table key ([`MESSAGE_KEY`]). `galileo.dll`'s lookup (found by
//! [`LOOKUP_PATTERN`]) answers that one key with [`MESSAGE`], in every
//! language; a mod cannot add it to the table, since the table is one file
//! (`locale/strres`) and a mod's language layers are all mounted at once.
//! Without the lookup hook the window shows `String [key] not found`.
//!
//! The connection runs on a worker thread and returns its result through a
//! hidden pointer, a 0x48-byte struct:
//!
//! ```text
//! +0x00 u8      connected
//! +0x08 u32     error code (4: lobby server not found)
//! +0x10 string  the error message (MSVC std::string: 16-byte buffer or pointer,
//!               +0x20 size, +0x28 capacity)
//! +0x30 u32     -1
//! +0x38, +0x40  zero on failure
//! ```
//!
//! The message longer than the inline buffer is allocated with the game's own
//! C runtime (`ucrtbase` `malloc`), which its `std::string` frees.
use core::ffi::{c_char, c_void};
use defiance_api::{Api, MultiplayerV1, LOG_INFO, LOG_WARN};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;

type Connect = unsafe extern "system" fn(network: *mut c_void, out: *mut u8) -> *mut u8;
type Malloc = unsafe extern "C" fn(size: usize) -> *mut u8;

static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static SERVICE: OnceLock<&'static MultiplayerV1> = OnceLock::new();
static MALLOC: OnceLock<Malloc> = OnceLock::new();
static LOG: OnceLock<unsafe extern "C" fn(u32, *const c_char)> = OnceLock::new();
static LOGGED: AtomicBool = AtomicBool::new(false);

const RESULT_BYTES: usize = 0x48;
const NOT_FOUND: u32 = 4;

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

/// The string table key of the player-facing message.
pub const MESSAGE_KEY: &str = "defiance_multiplayer_blocked";
/// The message the error window shows for [`MESSAGE_KEY`].
pub const MESSAGE: &str =
    "Gameplay plugins are active. Disable them in DefianceLoader and restart the game to play online.";
/// `galileo.dll`'s string table lookup, `(table, const char *key) -> const
/// std::string *`: a binary search that, on a miss, inserts and returns
/// `String [key] not found`. The prologue and the search loop.
pub const LOOKUP_PATTERN: &str =
    "48 89 5c 24 10 48 89 74 24 18 48 89 7c 24 20 55 41 54 41 55 41 56 41 57 \
     48 8d 6c 24 b0 48 81 ec 50 01 00 00 4c 8b fa 33 db 89 9d 80 00 00 00 48 8b 41 08 48 8b 48 08 \
     48 89 8d 80 00 00 00 4c 8b 28 48 8b f1 49 2b f5 48 c1 fe 06 4c 8d 73 ff 48 85 f6 7e 63 \
     0f 1f 40 00 0f 1f 84 00 00 00 00 00 4c 8b e6 49 d1 ec 49 8b fc 48 c1 e7 06 49 03 fd 49 8b de \
     48 ff c3 41 80 3c 1f 00 75 f6 48 8b 47 10 48 8b cf 48 83 7f 18 10 72 03 48 8b 0f 4c 8b c0 \
     48 3b d8 4c 0f 42 c3 49 8b d7 e8 ?? ?? ?? ??";

type Lookup = unsafe extern "system" fn(table: *mut c_void, key: *const c_char) -> *const u8;
static LOOKUP: AtomicUsize = AtomicUsize::new(0);
/// [`MESSAGE`] as a permanent MSVC `std::string`: pointer, unused, size,
/// capacity (at least 16, so the pointer form).
static MESSAGE_STRING: OnceLock<usize> = OnceLock::new();

/// A permanent MSVC `std::string` holding `text`, in its pointer form.
fn std_string(text: &str) -> usize {
    let bytes: &'static [u8] = Box::leak(format!("{text}\0").into_bytes().into_boxed_slice());
    let string: &'static [usize; 4] = Box::leak(Box::new([
        bytes.as_ptr() as usize,
        0,
        text.len(),
        text.len().max(16),
    ]));
    string as *const [usize; 4] as usize
}

unsafe extern "system" fn lookup(table: *mut c_void, key: *const c_char) -> *const u8 {
    if !key.is_null()
        && unsafe { core::ffi::CStr::from_ptr(key) }.to_bytes() == MESSAGE_KEY.as_bytes()
    {
        if let Some(&string) = MESSAGE_STRING.get() {
            return string as *const u8;
        }
    }
    let original: Lookup = unsafe { core::mem::transmute(LOOKUP.load(Ordering::Acquire)) };
    unsafe { original(table, key) }
}

/// Hook galileo's string lookup so [`MESSAGE_KEY`] reads as [`MESSAGE`].
fn install_message(api: &Api) {
    let name = c"galileo.dll";
    let base = unsafe { (api.module_base)(name.as_ptr()) };
    if base.is_null() {
        say(
            api,
            LOG_WARN,
            "multiplayer: galileo.dll is not loaded; the refusal shows its message key",
        );
        return;
    }
    let size = unsafe { (api.module_size)(base) };
    let pattern = std::ffi::CString::new(LOOKUP_PATTERN).unwrap_or_default();
    let target = unsafe { (api.find_pattern)(base, size, pattern.as_ptr()) };
    if target.is_null() {
        say(api, LOG_WARN, "multiplayer: galileo.dll's string lookup was not found; the refusal shows its message key");
        return;
    }
    let _ = MESSAGE_STRING.set(std_string(MESSAGE));
    let mut original = core::ptr::null_mut();
    let result = unsafe { (api.hook)(target, lookup as *mut c_void, &mut original) };
    if result != 0 || original.is_null() {
        say(api, LOG_WARN, &format!("multiplayer: galileo.dll's string lookup could not be hooked ({result}); the refusal shows its message key"));
        return;
    }
    LOOKUP.store(original as usize, Ordering::Release);
}

/// The blocking plugins, or `None` when nothing blocks. A service that cannot
/// be read counts as blocking: the guard fails closed.
unsafe fn blockers() -> Option<String> {
    let Some(service) = SERVICE.get() else {
        return Some("(unknown)".into());
    };
    let mut buffer = vec![0 as c_char; 1024];
    let length = unsafe { (service.blockers)(buffer.as_mut_ptr(), buffer.len()) };
    if length == 0 {
        return None;
    }
    if length >= buffer.len() {
        buffer.resize(length + 1, 0);
        unsafe { (service.blockers)(buffer.as_mut_ptr(), buffer.len()) };
    }
    let text = unsafe { core::ffi::CStr::from_ptr(buffer.as_ptr()) };
    Some(text.to_string_lossy().into_owned())
}

/// Fill the connection result with a failure carrying `text`. `alloc` provides
/// the message's storage when it does not fit inline.
///
/// # Safety
/// `out` must be the connection's 0x48-byte result.
pub unsafe fn fail(out: *mut u8, text: &str, alloc: impl Fn(usize) -> *mut u8) {
    unsafe {
        core::ptr::write_bytes(out, 0, RESULT_BYTES);
        (out.add(0x08) as *mut u32).write_unaligned(NOT_FOUND);
        let string = out.add(0x10);
        let bytes = text.as_bytes();
        let (size, capacity) = if bytes.len() <= 15 {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), string, bytes.len());
            (bytes.len(), 15)
        } else {
            let heap = alloc(bytes.len() + 1);
            if heap.is_null() {
                (0, 15)
            } else {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), heap, bytes.len());
                *heap.add(bytes.len()) = 0;
                (string as *mut *mut u8).write_unaligned(heap);
                (bytes.len(), bytes.len())
            }
        };
        (string.add(0x10) as *mut usize).write_unaligned(size);
        (string.add(0x18) as *mut usize).write_unaligned(capacity);
        (out.add(0x30) as *mut u32).write_unaligned(u32::MAX);
    }
}

unsafe extern "system" fn connect(network: *mut c_void, out: *mut u8) -> *mut u8 {
    match unsafe { blockers() } {
        None => {
            let original: Connect =
                unsafe { core::mem::transmute(ORIGINAL.load(Ordering::Acquire)) };
            unsafe { original(network, out) }
        }
        Some(blockers) => {
            let malloc = MALLOC.get().copied();
            unsafe {
                fail(out, MESSAGE_KEY, |size| {
                    malloc.map_or(core::ptr::null_mut(), |malloc| malloc(size))
                })
            };
            if !LOGGED.swap(true, Ordering::Relaxed) {
                if let Some(log) = LOG.get() {
                    let line = std::ffi::CString::new(format!(
                        "multiplayer: refused the lobby connection; active: {blockers}"
                    ))
                    .unwrap_or_default();
                    unsafe { log(LOG_INFO, line.as_ptr()) };
                }
            }
            out
        }
    }
}

fn say(api: &Api, level: u32, text: &str) {
    let text = std::ffi::CString::new(text).unwrap_or_default();
    unsafe { (api.log)(level, text.as_ptr()) };
}

/// Hook the lobby connection at `address`, during Core's `init`. Without the
/// loader's service or the game's allocator the guard is not installed and
/// the log says so; multiplayer then behaves as it does without plugins.
pub fn install(api: &Api, address: usize) {
    let _ = LOG.set(api.log);
    let Some(service) = (unsafe { defiance_feature_sdk::services::multiplayer() }) else {
        say(
            api,
            LOG_WARN,
            "multiplayer: this loader has no multiplayer service; online play is not guarded",
        );
        return;
    };
    let _ = SERVICE.set(service);
    let ucrt: Vec<u16> = "ucrtbase.dll\0".encode_utf16().collect();
    let module = unsafe { GetModuleHandleW(ucrt.as_ptr()) };
    let malloc = if module.is_null() {
        core::ptr::null_mut()
    } else {
        unsafe { GetProcAddress(module, b"malloc\0".as_ptr()) }
    };
    if malloc.is_null() {
        say(
            api,
            LOG_WARN,
            "multiplayer: the game's malloc was not found; online play is not guarded",
        );
        return;
    }
    let _ = MALLOC.set(unsafe { core::mem::transmute::<*mut c_void, Malloc>(malloc) });
    let mut original = core::ptr::null_mut();
    let result = unsafe {
        (api.hook)(
            address as *mut c_void,
            connect as *mut c_void,
            &mut original,
        )
    };
    if result != 0 || original.is_null() {
        say(api, LOG_WARN, &format!("multiplayer: the lobby connection could not be hooked ({result}); online play is not guarded"));
        return;
    }
    ORIGINAL.store(original as usize, Ordering::Release);
    unsafe { (service.guard_installed)() };
    install_message(api);
    say(api, LOG_INFO, "multiplayer: lobby connection guarded");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string_of(out: &[u8]) -> String {
        let size = usize::from_le_bytes(out[0x20..0x28].try_into().unwrap());
        let capacity = usize::from_le_bytes(out[0x28..0x30].try_into().unwrap());
        let bytes = if capacity <= 15 {
            out[0x10..0x10 + size].to_vec()
        } else {
            let pointer = usize::from_le_bytes(out[0x10..0x18].try_into().unwrap());
            unsafe { core::slice::from_raw_parts(pointer as *const u8, size) }.to_vec()
        };
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn a_refused_connection_is_a_failure_with_the_message() {
        let mut out = [0xaau8; RESULT_BYTES];
        let mut heap = vec![0u8; 512];
        let text = MESSAGE_KEY;
        let storage = heap.as_mut_ptr();
        unsafe { fail(out.as_mut_ptr(), text, |_| storage) };
        assert_eq!(out[0], 0, "not connected");
        assert_eq!(
            u32::from_le_bytes(out[8..12].try_into().unwrap()),
            NOT_FOUND
        );
        assert_eq!(string_of(&out), text);
        assert_eq!(heap[text.len()], 0, "NUL-terminated");
        assert_eq!(
            u32::from_le_bytes(out[0x30..0x34].try_into().unwrap()),
            u32::MAX
        );
        assert!(out[0x38..0x48].iter().all(|&b| b == 0));
    }

    #[test]
    fn the_message_is_a_permanent_heap_form_string() {
        let string = std_string(MESSAGE) as *const usize;
        let words = unsafe { core::slice::from_raw_parts(string, 4) };
        assert_eq!(words[2], MESSAGE.len());
        assert!(words[3] >= 16, "the pointer form");
        let bytes = unsafe { core::slice::from_raw_parts(words[0] as *const u8, words[2] + 1) };
        assert_eq!(&bytes[..MESSAGE.len()], MESSAGE.as_bytes());
        assert_eq!(bytes[MESSAGE.len()], 0);
        // The pattern is what the loader's find_pattern takes.
        assert!(LOOKUP_PATTERN
            .split_whitespace()
            .all(|b| b == "??" || u8::from_str_radix(b, 16).is_ok()));
    }

    #[test]
    fn a_short_message_stays_inline() {
        let mut out = [0xaau8; RESULT_BYTES];
        unsafe { fail(out.as_mut_ptr(), "offline", |_| panic!("no allocation")) };
        assert_eq!(string_of(&out), "offline");
    }
}
