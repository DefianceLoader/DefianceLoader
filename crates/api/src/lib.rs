//! The plugin ABI. This is the whole contract between the loader and a plugin:
//! one exported symbol, returning a `Plugin`, whose `init` receives an `Api`.
//!
//! Native code has no reflection and no name mangling a plugin could rely on,
//! so nothing here is a Rust trait or a generic. Everything is `#[repr(C)]`,
//! pointers stay opaque to the plugin, and a plugin author in any language can
//! follow `include/defiance.h`, which mirrors these definitions byte for byte.
//!
//! The loader owns the game; a plugin never touches the module image directly.
//! It asks the loader to resolve a name, scan for a signature, or install a
//! hook, and the loader keeps the bookkeeping (the trampoline, the original
//! bytes, whether the site has already moved under an update).

use core::ffi::{c_char, c_void, CStr};

/// Bumped whenever a field changes meaning or position. The loader refuses a
/// plugin whose `abi_version` it does not know, and a plugin that does not
/// recognise the loader's `Api::abi_version` returns non-zero from `init`.
///
/// The rule from here on: a field is only ever *appended* to `Api`, never
/// inserted or removed, so a plugin compiled against an older, shorter `Api`
/// still reads the fields it knows at the offsets it knows. `reserved` is the
/// escape hatch for growing the struct without moving anything. A plugin reads
/// `abi_version` and uses only what it was compiled against.
pub const ABI_VERSION: u32 = 5;

/// The export a plugin DLL must define, named without decoration:
/// `extern "C" fn() -> *const Plugin`.
pub const PLUGIN_ENTRY: &[u8] = b"defiance_plugin\0";

/// Optional extension handshake. Does not change the ABI-5 Api layout.
/// Export `extern "C" fn(*const ServiceApiV1) -> i32` under this name.
pub const SERVICES_ENTRY: &[u8] = b"defiance_plugin_services\0";

/// Service discovery is permitted only on the thread executing plugin init.
/// Providers register permanent C-compatible tables; consumers resolve tables
/// from declared dependencies and cache them. The loader never owns table memory.
#[repr(C)]
pub struct ServiceApiV1 {
    pub version: u32,
    pub size: u32,
    /// Zero on success. Names are case-insensitive ASCII [a-z0-9._-], 1..128
    /// bytes. Version and size must be nonzero. Duplicate versions are refused.
    /// Tables become visible only after successful init; failed init removes them.
    pub register: unsafe extern "C" fn(
        name: *const c_char,
        version: u32,
        table: *const c_void,
        size: usize,
    ) -> i32,
    /// Exact service version and at least min_size bytes; null on failure.
    /// Provider is a manifest plugin ID listed in the consumer's dependencies,
    /// or [`LOADER_PROVIDER`].
    pub query: unsafe extern "C" fn(
        provider: *const c_char,
        name: *const c_char,
        version: u32,
        min_size: usize,
    ) -> *const c_void,
}

/// The provider ID of the services the loader itself provides. Any plugin may
/// query them without declaring a dependency, since the loader is always
/// present; no plugin may register under this ID.
pub const LOADER_PROVIDER: &CStr = c"defiance.loader";

/// `defiance.loader` / `crash-ranges`, service version 1. Names mod code in
/// crash reports: a fault inside a mapped range is reported as `label+offset`.
/// Attribution only; it changes no protection or handling. Both calls may be
/// made from any thread at any time after the table is resolved.
#[repr(C)]
pub struct CrashRangesV1 {
    /// Map `[start, end)` under `label`: UTF-8, 1..=128 bytes, printable, no
    /// line breaks, copied. A range with the same start replaces the earlier
    /// one. Returns 0, or 1 for an empty range or an invalid label.
    pub map: unsafe extern "C" fn(start: usize, end: usize, label: *const c_char) -> i32,
    /// Stop attributing faults to the range that starts at `start`. Returns 0,
    /// or 1 for a zero start.
    pub unmap: unsafe extern "C" fn(start: usize) -> i32,
}

/// `defiance.loader` / `trace`, service version 1. Diagnostic call-stack
/// tracing with a hardware breakpoint: each hit at the address is logged to the
/// loader log with its registers and stack, and execution resumes unchanged.
/// Four sites at most, shared with `[trace] sites` in `core.ini`. Both calls may
/// be made from any thread once the table is resolved. Leave it out of
/// shipping builds; it is for finding callers while developing.
#[repr(C)]
pub struct TraceV1 {
    /// Log the next `hits` (1..=1000) hits at `address`, executable code, under
    /// `label` (as for [`CrashRangesV1::map`]); the site is then released.
    /// Returns 0; 1 for an invalid argument; 2 when all four sites are in use;
    /// 3 when `address` is already traced; 4 when tracing could not start.
    pub trace: unsafe extern "C" fn(address: usize, hits: u32, label: *const c_char) -> i32,
    /// Release `address` before its hits are logged. Returns 0, or 1 when it
    /// is not traced.
    pub stop: unsafe extern "C" fn(address: usize) -> i32,
}

/// `defiance.loader` / `multiplayer`, service version 1. Which active plugins
/// block multiplayer: those whose manifest does not declare `multiplayer_safe`.
/// Callable from any thread.
#[repr(C)]
pub struct MultiplayerV1 {
    /// The blocking plugins' IDs, comma-separated, copied NUL-terminated into
    /// `buffer` when `capacity` exceeds their length. Returns that length; 0
    /// when nothing blocks. Before startup has finished it reports a
    /// placeholder, never 0.
    pub blockers: unsafe extern "C" fn(buffer: *mut c_char, capacity: usize) -> usize,
    /// For Core: the guard is installed. Until it is called, the loader starts
    /// no plugin that is not multiplayer-safe.
    pub guard_installed: unsafe extern "C" fn(),
}

/// `defiance.selection` / `selection`, service version 1. Read-only, game-thread
/// only. A non-null argument must be a live selectable facet of this game build.
/// Null returns zero; pointers are never retained. No Rust-owned values cross ABI.
#[repr(C)]
pub struct SelectionV1 {
    pub is_selected: unsafe extern "C" fn(selectable: *mut c_void) -> u8,
}

/// A copied view, not ownership of a game object. All fields describe the same
/// game-thread observation. A pin of zero means no valid override.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MemberStateV1 {
    pub selectable: *mut c_void,
    pub squad_selectable: *mut c_void,
    pub selected: u8,
    pub enabled: u8,
    pub firing_pin: u8,
    pub posture_pin: u8,
    /// Bit 0 firing marker valid; bit 1 posture marker valid. Distinguishes
    /// uninitialized storage from a valid marker with a zero pin byte.
    pub pin_flags: u8,
    pub raw_mark: u8,
    pub reserved: [u8; 2],
}

/// `defiance.core` / `game-access`, version 1. All calls require live game
/// objects on their owning game thread. Null objects are handled; arbitrary
/// non-null pointers are not validated. No pointers or buffers are retained.
#[repr(C)]
pub struct GameAccessV1 {
    pub entity_from_facet: unsafe extern "C" fn(facet: *mut c_void) -> *mut c_void,
    pub selectable: unsafe extern "C" fn(entity: *mut c_void) -> *mut c_void,
    /// Required count, or usize::MAX on unavailable/invalid roster. Capacity 0
    /// queries the count. Copies nothing if capacity is smaller than the count.
    pub copy_members:
        unsafe extern "C" fn(entity: *mut c_void, out: *mut *mut c_void, capacity: usize) -> usize,
    /// Requires a soldier with SquadUnitSelectableFacet. 0 success;
    /// 1 unavailable/not a member of expected_squad (if non-null). No RTTI check.
    pub read_member: unsafe extern "C" fn(
        entity: *mut c_void,
        expected_squad: *mut c_void,
        out: *mut MemberStateV1,
    ) -> i32,
    /// Requires SquadUnitSelectableFacet. has_pin==0 clears only a valid firing pin. Otherwise stores value+1,
    /// wrapping as the assembly does, and initializes its shared marker if needed.
    pub set_firing_pin:
        unsafe extern "C" fn(selectable: *mut c_void, value: u8, has_pin: u8) -> i32,
    pub squad_firing: unsafe extern "C" fn(ai: *mut c_void) -> u8,
    pub set_squad_firing: unsafe extern "C" fn(ai: *mut c_void, value: u8) -> i32,
}

/// Built-in migration interface: replace a verified named function entry while
/// retaining the rest of its feature's patch plan. Internal, not a game service.
#[repr(C)]
pub struct NativeReplacementV1 {
    pub name: *const c_char,
    pub detour: *mut c_void,
}

pub const LOG_INFO: u32 = 0;
pub const LOG_WARN: u32 = 1;
pub const LOG_ERROR: u32 = 2;

/// What the loader hands a plugin at `init`. Every function pointer is valid
/// for the life of the process. Strings passed in are NUL-terminated UTF-8;
/// strings a plugin passes in must outlive the call.
///
/// `reserved` must be zero: a plugin checks it so a future ABI can add fields
/// without old plugins reading them.
#[repr(C)]
pub struct Api {
    pub abi_version: u32,
    pub reserved: u32,
    /// level: one of the `LOG_*` constants.
    pub log: unsafe extern "C" fn(level: u32, message: *const c_char),
    /// The base address of a loaded module by file name (`logic.dll`), or null.
    pub module_base: unsafe extern "C" fn(name: *const c_char) -> *mut c_void,
    /// The mapped size of a module, or zero.
    pub module_size: unsafe extern "C" fn(base: *mut c_void) -> usize,
    /// The address of the one match of `pattern` in `[base, base + size)`, or
    /// null if there is none or more than one. `pattern` is the same text
    /// `tools/sigs.py` writes: hex bytes with `??` for a wildcard, spaces
    /// optional.
    pub find_pattern:
        unsafe extern "C" fn(base: *mut c_void, size: usize, pattern: *const c_char) -> *mut c_void,
    /// As `find_pattern`, but the address asked for is `offset` bytes into the
    /// signature's window, as `tools/sigs.py`'s `site_offset` records. The
    /// signature vouches for every address in its window, so an offset that is
    /// one of those is resolved with the same guarantee; an offset past the
    /// window is refused. The common `offset` is zero, which is `find_pattern`.
    pub find_pattern_at: unsafe extern "C" fn(
        base: *mut c_void,
        size: usize,
        pattern: *const c_char,
        offset: usize,
    ) -> *mut c_void,
    /// Redirect `target` to `detour`, writing a trampoline that runs the
    /// instructions overwritten at `target` and then returns. The loader works
    /// out how many bytes to displace by decoding whole instructions. On
    /// success `*original` receives the trampoline to call for the stock
    /// behaviour, and the loader remembers how to `unhook`. Returns zero on
    /// success, non-zero otherwise. Displaced instructions requiring relocation
    /// (relative branches/calls or RIP/EIP-relative operands) are refused.
    /// A distant detour uses an owned nearby relay when the displaced span is
    /// too short for an absolute jump; the original trampoline is unchanged.
    pub hook: unsafe extern "C" fn(
        target: *mut c_void,
        detour: *mut c_void,
        original: *mut *mut c_void,
    ) -> i32,
    /// As `hook`, but with an explicit byte count. The loader still validates
    /// instruction boundaries and rejects instructions requiring relocation.
    /// The count is never expanded to accommodate a distant detour.
    pub hook_exact: unsafe extern "C" fn(
        target: *mut c_void,
        detour: *mut c_void,
        displaced: usize,
        original: *mut *mut c_void,
    ) -> i32,
    /// Redirect the direct call at `site` to `detour`, and put the address the
    /// call reached before into `*original` for the stock behaviour. Unlike
    /// `hook`, which takes a function over for every caller, this changes one
    /// call site: the function's other callers are untouched. A stub near the
    /// site carries the jump, so `detour` may be anywhere. Returns zero on
    /// success; `unhook(site)` puts the call back.
    pub hook_call: unsafe extern "C" fn(
        site: *mut c_void,
        detour: *mut c_void,
        original: *mut *mut c_void,
    ) -> i32,
    /// Put the bytes `hook` displaced back. Returns zero on success.
    pub unhook: unsafe extern "C" fn(target: *mut c_void) -> i32,
    /// The address of a method of an RTTI class by name, or null. `class` is
    /// the decorated or bare class name (`Squad`, `.?AVSquad@@`), matched as a
    /// substring. The name is resolved through the loader's `defiance-rtti.ini`
    /// table (section = class, `name = slot`); with no table, or no such name,
    /// this is null — RTTI itself carries no method names. Use `vtable_slot`
    /// when only the slot is known.
    pub rtti_method:
        unsafe extern "C" fn(class: *const c_char, method: *const c_char) -> *mut c_void,
    /// The function in vtable slot `slot` (zero-based) of an RTTI class, or
    /// null. The class is matched as a substring and the first vtable found in
    /// `logic.dll` then `game.dll` is used.
    pub vtable_slot: unsafe extern "C" fn(class: *const c_char, slot: usize) -> *mut c_void,
    /// A validated setting from the loader's configuration, by its stable
    /// `plugin_id` and key, both matched case-insensitively. A plugin's section
    /// is its own ID (`[defiance.movement]`), so it gets its own namespace.
    ///
    /// The host returns the *canonical* string form of the declared setting:
    /// booleans are `true`/`false`, integers are decimal, a choice keeps its
    /// declared spelling. A setting that was declared but not written in its
    /// file still returns its default. Null means there is no such value, or
    /// the owning plugin is blocked by invalid configuration; a blocked plugin
    /// must not initialize.
    ///
    /// The returned string is owned by the loader and stays valid for the
    /// process's life; copy it if you mean to keep it elsewhere. A legacy
    /// ABI 5 plugin's own bootstrap sections (in `defiance-loader.ini`) remain
    /// readable here until it ships a manifest. Rust plugins should prefer the
    /// typed accessors in `defiance-feature-sdk`.
    pub config_get:
        unsafe extern "C" fn(section: *const c_char, key: *const c_char) -> *const c_char,
    /// Compare and replace an exact byte span, owned by the initializing plugin.
    /// Conflicts/mismatches are refused. unhook restores it; failed init rolls
    /// it back with all other owned hooks. Caller supplies valid readable spans.
    pub patch_bytes: unsafe extern "C" fn(
        target: *mut c_void,
        before: *const u8,
        after: *const u8,
        length: usize,
    ) -> i32,
}

/// What a plugin exports. `defiance_plugin()` returns a pointer to one of
/// these; it must stay valid for the process.
#[repr(C)]
pub struct Plugin {
    /// Must be `ABI_VERSION`.
    pub abi_version: u32,
    pub name: *const c_char,
    pub version: *const c_char,
    /// Called once, after the game's modules are loaded. Return zero to stay
    /// loaded, non-zero to be treated as failed (and, if it already hooked
    /// anything, `stop` is still called so it can undo it).
    pub init: unsafe extern "C" fn(api: *const Api) -> i32,
    /// Called on unload or after a failed `init`. Optional.
    pub stop: Option<unsafe extern "C" fn()>,
}

/// The type of the exported entry point, for a plugin to define. The loader
/// resolves it by that exact name with `GetProcAddress`, so it must be
/// `extern "C"` and `#[no_mangle]` on the Rust side.
pub type Entry = unsafe extern "C" fn() -> *const Plugin;

/// Leak a `Plugin` so it can be returned from `defiance_plugin`. A plugin
/// exports something like:
///
/// ```ignore
/// #[no_mangle]
/// pub extern "C" fn defiance_plugin() -> *const defiance_api::Plugin {
///     defiance_api::leak(defiance_api::Plugin { /* ... */ })
/// }
/// ```
///
/// The leak is deliberate and happens once: the loader holds the pointer for
/// the process's life. `Plugin` is not `Sync` (it holds raw pointers), so it
/// cannot be a `static` a plugin returns a reference to; this is the way round
/// that.
pub fn leak(plugin: Plugin) -> *const Plugin {
    Box::into_raw(Box::new(plugin)) as *const Plugin
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, size_of};

    #[test]
    fn layout_is_stable() {
        // The C header has to agree with these; a change here is a change to
        // `include/defiance.h` and to ABI_VERSION.
        assert_eq!(align_of::<Api>(), align_of::<*const c_void>());
        // abi_version, padding, name, version, init, stop
        assert_eq!(size_of::<Plugin>(), 8 + 4 * size_of::<*const c_void>());
        assert_eq!(
            core::mem::offset_of!(Api, patch_bytes),
            8 + 12 * size_of::<*const c_void>()
        );
        assert_eq!(size_of::<Api>(), 8 + 13 * size_of::<*const c_void>());
        assert_eq!(core::mem::offset_of!(ServiceApiV1, register), 8);
        assert_eq!(
            core::mem::offset_of!(ServiceApiV1, query),
            8 + size_of::<*const c_void>()
        );
        assert_eq!(
            size_of::<ServiceApiV1>(),
            8 + 2 * size_of::<*const c_void>()
        );
        assert_eq!(size_of::<SelectionV1>(), size_of::<*const c_void>());
        assert_eq!(size_of::<AmmoMenuV1>(), 2 * size_of::<*const c_void>());
        assert_eq!(size_of::<GameAccessV1>(), 7 * size_of::<*const c_void>());
        assert_eq!(size_of::<MemberStateV1>(), 24);
        assert_eq!(core::mem::offset_of!(MemberStateV1, selected), 16);
    }
}

/// Core service "ammo-menu", version 1. Process-lifetime, thread-safe callbacks.
/// capacity returns installed UI slots (stock: 9), not a configuration request.
/// publish is reserved for the menu layout provider during startup. Call only
/// after all patches succeed, as the last fallible operation in init; after a
/// successful publication init MUST succeed. No hot reload/unload is supported.
/// Returns 0 once, 1 for invalid capacity, 2 if already published.
#[repr(C)]
pub struct AmmoMenuV1 {
    pub capacity: unsafe extern "C" fn() -> u32,
    pub publish: unsafe extern "C" fn(u32) -> i32,
}
