//! Shared payload storage and build validation for the built-in feature plugins.
//!
//! The assembled payload and the descriptors are the same files the injector
//! carries; the difference is that the sites are found and the writes made
//! in-process, through `defiance-core`, instead of from an injector watching
//! for the process. The known-build / verified-build / relocate decision is the
//! same code (`install::logic_for_build` and `game_for_build`). The runtime
//! prepares storage; feature plugins own the writes through Api::patch_bytes.

use core::ffi::{c_char, c_void, CStr};
use defiance_api::{Api, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO, LOG_WARN};
use defiance_core::install::{self, Scan};
use defiance_core::{GamePatch, Patch, Target};
use std::path::PathBuf;
mod ammo_menu;
mod game;
mod multiplayer;
mod preview;
defiance_feature_sdk::service_handshake!();

#[cfg(feature = "parity-test")]
#[no_mangle]
pub extern "C" fn defiance_test_game_access() -> *const defiance_api::GameAccessV1 {
    &game::API
}

static NAME: &[u8] = b"defiance.core\0";
static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

const PAYLOAD: &[u8] = include_bytes!("../../../tools/variants/reference/logic.bin");
const DESCRIPTOR: &str = include_str!("../../../tools/variants/reference/logic.json");
const GAME_PAYLOAD: &[u8] = include_bytes!("../../../tools/variants/reference/game.bin");
const GAME_DESCRIPTOR: &str = include_str!("../../../tools/variants/reference/game.json");

/// A build whose classes gained members: its payload was assembled from the
/// same source with that build's layout (`tools/layouts/*.json`) and resolved
/// against its DLLs (`tools/variant.py`), so it applies without relocation.
/// Both modules' hashes are the variant's identity: a logic.dll from one build
/// with a game.dll from another is refused, never patched.
struct Variant {
    logic_sha: &'static str,
    game_sha: &'static str,
    logic_payload: &'static [u8],
    logic_descriptor: &'static str,
    game_payload: &'static [u8],
    game_descriptor: &'static str,
    /// The build's squad AI facet roster getter slot (reference 0x3b8); one
    /// class offset the shared `GameAccessV1` table reads directly.
    roster_slot: usize,
}

static VARIANTS: &[Variant] = &[
    Variant {
        logic_sha: "eb8674f1d16595a3e9cf6a9ec0062735b1184976495d8d6ade36f7e2574e8aab",
        game_sha: "bc2af42369f9f6fe70e206ae4846f8f0e46ac01cca9a325c8159ee04a9b5e405",
        logic_payload: include_bytes!("../../../tools/variants/gog-2026-09-14/logic.bin"),
        logic_descriptor: include_str!("../../../tools/variants/gog-2026-09-14/logic.json"),
        game_payload: include_bytes!("../../../tools/variants/gog-2026-09-14/game.bin"),
        game_descriptor: include_str!("../../../tools/variants/gog-2026-09-14/game.json"),
        roster_slot: 0x3d0,
    },
    Variant {
        logic_sha: "30264904e1d5199b954bafbd7828cf7190930c246d35fa7b94eefa915e8f0c38",
        game_sha: "d926a213731d73bac8ccc56b50e2b4292fe8613c7a9bb7c8b9fc9132c122ed25",
        logic_payload: include_bytes!("../../../tools/variants/steam-2026-09-22/logic.bin"),
        logic_descriptor: include_str!("../../../tools/variants/steam-2026-09-22/logic.json"),
        game_payload: include_bytes!("../../../tools/variants/steam-2026-09-22/game.bin"),
        game_descriptor: include_str!("../../../tools/variants/steam-2026-09-22/game.json"),
        roster_slot: 0x3d0,
    },
];

/// The reference build's roster getter slot.
const REFERENCE_ROSTER_SLOT: usize = 0x3b8;

fn variant_for(logic_sha: &str, game_sha: &str) -> Option<&'static Variant> {
    VARIANTS
        .iter()
        .find(|v| v.logic_sha == logic_sha && v.game_sha == game_sha)
}

/// Builds whose addresses moved but whose class layout did not, as one
/// (logic.dll, game.dll) hash pair each: relocation carries them. A pair, not
/// two lists, so a logic.dll from one build cannot be paired with a game.dll
/// from another. Each hash must also appear in that module's `verified_sha`.
static VERIFIED_PAIRS: &[(&str, &str)] = &[(
    "d320f848508c45c9f04df235204b5fbc9ffbb1b7f869e4c5a58d80bb2ecc10ed",
    "dc10419f417aed4ecff348b7c96b3c7c574a9a2541f76c5fbc35eb92dbc716d6",
)];

fn verified_pair(logic_sha: &str, game_sha: &str) -> bool {
    VERIFIED_PAIRS
        .iter()
        .any(|&(logic, game)| logic == logic_sha && game == game_sha)
}

/// How Core patches a (logic.dll, game.dll) pair.
enum Choice {
    /// A layout-changing build: its own payload, applied without relocation.
    Variant(&'static Variant),
    /// The reference pair, or a verified pair relocation carries.
    Reference,
    /// `allow_unknown_build`: neither module is recognized; patch by signature.
    Unknown,
}

/// The payload for a pair, or None to refuse it. `allow_unknown_build` opens
/// the signature path only when neither module is one any supported pair
/// names (the reference, a verified build or a variant), so a recognized
/// module is never patched beside one from another build.
fn choose(
    logic_sha: &str,
    game_sha: &str,
    logic: &Patch,
    game: &GamePatch,
    allow_unknown: bool,
) -> Option<Choice> {
    if let Some(v) = variant_for(logic_sha, game_sha) {
        return Some(Choice::Variant(v));
    }
    if (logic_sha == logic.source_sha256 && game_sha == game.source_sha256)
        || verified_pair(logic_sha, game_sha)
    {
        return Some(Choice::Reference);
    }
    let logic_known = logic_sha == logic.source_sha256
        || logic.verified.iter().any(|v| v == logic_sha)
        || VARIANTS.iter().any(|v| v.logic_sha == logic_sha);
    let game_known = game_sha == game.source_sha256
        || game.verified.iter().any(|v| v == game_sha)
        || VARIANTS.iter().any(|v| v.game_sha == game_sha);
    (allow_unknown && !logic_known && !game_known).then_some(Choice::Unknown)
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcessId() -> u32;
    fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, size: u32) -> u32;
}

/// The game directory, from this process's executable; `logic.dll` and
/// `game.dll` sit beside it.
fn game_dir() -> PathBuf {
    let mut buffer = vec![0u16; 1024];
    let length = unsafe {
        GetModuleFileNameW(
            core::ptr::null_mut(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    let end = buffer[..length.min(buffer.len())]
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(length);
    let exe = PathBuf::from(String::from_utf16_lossy(&buffer[..end]));
    exe.parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// A loaded game module as the core's `Target`, with its path on disk so the
/// build can be hashed and relocated if it is not the one the patch was
/// written for.
fn module_target(api: &Api, name: &str) -> Option<Target> {
    let symbol: Vec<u8> = name.bytes().chain(core::iter::once(0)).collect();
    let base: *mut c_void = unsafe { (api.module_base)(symbol.as_ptr() as *const c_char) };
    if base.is_null() {
        return None;
    }
    let size = unsafe { (api.module_size)(base) };
    Some(Target {
        process_id: unsafe { GetCurrentProcessId() },
        base: base as *mut u8,
        size,
        path: game_dir().join(name),
    })
}

/// A yes/no setting from `defiance-loader.ini`. An empty `section` is the
/// loader's own top level (`allow_unknown_build`); `defiance.core` is this
/// plugin's own section.
/// A plugin's setting as text, lower case; empty when it has none.
fn config_text(api: &Api, section: &str, key: &str) -> String {
    let section: Vec<u8> = section.bytes().chain(core::iter::once(0)).collect();
    let name: Vec<u8> = key.bytes().chain(core::iter::once(0)).collect();
    let value = unsafe {
        (api.config_get)(
            section.as_ptr() as *const c_char,
            name.as_ptr() as *const c_char,
        )
    };
    if value.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .to_ascii_lowercase()
}

/// The virtual key for `squad_tab_modifier`: Ctrl by default, Shift, or none.
fn tab_modifier_key(setting: &str) -> u32 {
    match setting {
        "shift" => 0x10,
        "off" => 0,
        _ => 0x11,
    }
}

fn config_flag(api: &Api, section: &str, key: &str) -> bool {
    let section: Vec<u8> = section.bytes().chain(core::iter::once(0)).collect();
    let name: Vec<u8> = key.bytes().chain(core::iter::once(0)).collect();
    let value = unsafe {
        (api.config_get)(
            section.as_ptr() as *const c_char,
            name.as_ptr() as *const c_char,
        )
    };
    if value.is_null() {
        return false;
    }
    let text = unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .to_ascii_lowercase();
    matches!(text.as_str(), "true" | "yes" | "on" | "1")
}

struct Runtime {
    native_entries: std::collections::BTreeMap<String, (u32, usize)>,
    logic: defiance_core::features::Prepared,
    game: defiance_core::features::Prepared,
    installed: std::sync::Mutex<u32>,
    /// The features core actually prepared; one whose signature was missing on
    /// a relocated build is absent and refuses to install.
    available: u64,
    record: String,
    /// The game's lobby connection, which the multiplayer guard hooks; None
    /// when its signature was not found in this build.
    lobby_connect: Option<usize>,
    /// The game block cell for the squad TAB modifier's virtual key, and the
    /// block's size, which bounds it.
    tab_modifier_offset: usize,
    game_block_bytes: usize,
    /// The logic block cell for the squad preview's material callback.
    preview_dim_cell: usize,
    logic_block_bytes: usize,
}
static RUNTIME: std::sync::OnceLock<Runtime> = std::sync::OnceLock::new();

/// The host's accepted plan, as a bit per legacy feature ID. Set before `init`
/// through `defiance_configure_enabled_v1`; all features when it is not, so the
/// injector's test host and an older host keep the previous behavior.
static ENABLED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);

/// Called by the host after loading this DLL and before `init`, with the
/// features its accepted plan will install. Only those are validated during
/// preparation; the rest keep their fixed offsets but are not installed.
#[no_mangle]
pub extern "C" fn defiance_configure_enabled_v1(mask: u64) {
    ENABLED.store(mask, std::sync::atomic::Ordering::Relaxed);
}

fn say(api: &Api, level: u32, message: &str) {
    let text = std::ffi::CString::new(message.replace('\0', "")).unwrap_or_default();
    unsafe { (api.log)(level, text.as_ptr()) };
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    if api.is_null() {
        return 1;
    }
    let api = unsafe { &*api };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    preview::set_logger(api.log);
    let prepare = || -> Result<Runtime, String> {
        let logic_target = module_target(api, "logic.dll").ok_or("logic.dll is not loaded")?;
        let game_target = module_target(api, "game.dll").ok_or("game.dll is not loaded")?;
        let scan = if config_flag(api, "", "allow_unknown_build") {
            Scan::Unknown
        } else {
            Scan::Default
        };
        // Resolve both unmodified modules before any feature installs hooks.
        let enabled = ENABLED.load(std::sync::atomic::Ordering::Relaxed);
        // The two hashes together choose the payload: a build's classes moved
        // relative to another's, so a logic.dll and game.dll from different
        // builds must never be mixed.
        let logic_sha = defiance_core::sha256::file(&logic_target.path)
            .map_err(|e| format!("hashing logic.dll: {e}"))?;
        let game_sha = defiance_core::sha256::file(&game_target.path)
            .map_err(|e| format!("hashing game.dll: {e}"))?;
        let reference_logic = Patch::parse(DESCRIPTOR);
        let reference_game = GamePatch::parse(GAME_DESCRIPTOR);
        let choice = choose(
            &logic_sha,
            &game_sha,
            &reference_logic,
            &reference_game,
            scan == Scan::Unknown,
        )
        .ok_or_else(|| {
            format!(
                "logic.dll {logic_sha} and game.dll {game_sha} are not a supported build pair; \
                 refusing rather than patching mismatched modules"
            )
        })?;
        game::set_roster_slot(match choice {
            Choice::Variant(v) => v.roster_slot,
            Choice::Reference | Choice::Unknown => REFERENCE_ROSTER_SLOT,
        });
        let (logic_raw, game_raw, logic_payload, game_payload) = match choice {
            Choice::Variant(v) => {
                say(
                    api,
                    LOG_INFO,
                    "both modules match a supported build pair; using its payload",
                );
                (
                    Patch::parse(v.logic_descriptor),
                    GamePatch::parse(v.game_descriptor),
                    v.logic_payload,
                    v.game_payload,
                )
            }
            Choice::Reference => (reference_logic, reference_game, PAYLOAD, GAME_PAYLOAD),
            Choice::Unknown => {
                say(api, LOG_WARN, "allow_unknown_build: patching an unrecognized logic.dll/game.dll pair by signature");
                (reference_logic, reference_game, PAYLOAD, GAME_PAYLOAD)
            }
        };
        // A feature whose signature is missing on a relocated build is dropped
        // here; the rest still prepare. Only features available in both modules
        // are prepared, so a feature installs into neither or both.
        let logic_ok = install::available_features_logic(&logic_raw, &logic_target, scan, enabled)?;
        let game_ok = install::available_features_game(&game_raw, &game_target, scan, enabled)?;
        let available = logic_ok & game_ok;
        let unavailable = enabled & !available;
        if unavailable != 0 {
            say(api, LOG_WARN, &format!("features {unavailable:#x} could not be prepared; they and their dependents will be refused"));
        }
        let (logic_patch, _, _) =
            install::logic_for_build_masked(&logic_raw, &logic_target, scan, available)?;
        let (game_patch, _, _) =
            install::game_for_build_masked(&game_raw, &game_target, scan, available)?;
        let logic = defiance_core::features::prepare_logic_enabled(
            &logic_patch,
            &logic_target,
            logic_payload,
            available,
        )?;
        let game = defiance_core::features::prepare_game_enabled(
            &game_patch,
            &game_target,
            game_payload,
            available,
        )?;
        if logic
            .writes
            .iter()
            .chain(&game.writes)
            .any(|w| !(1..=defiance_core::features::KNOWN_FEATURES).contains(&w.feature))
        {
            return Err("payload contains an unknown feature ID".into());
        }
        // Name the blocks in crash reports through the loader's service; the
        // log line is for people reading the log.
        let ranges = unsafe { defiance_feature_sdk::services::crash_ranges() };
        for (label, block, size) in [
            (
                c"core.logic assembly payload",
                logic.block,
                logic_patch.block_bytes,
            ),
            (
                c"core.game assembly payload",
                game.block,
                game_patch.block_bytes,
            ),
        ] {
            if let Some(ranges) = ranges {
                unsafe { (ranges.map)(block, block + size, label.as_ptr()) };
            }
            say(
                api,
                LOG_INFO,
                &format!(
                    "{} at {block:#x}..{:#x}",
                    label.to_string_lossy(),
                    block + size
                ),
            );
        }
        for detour in &logic_patch.detours {
            let name = logic_raw
                .detours
                .iter()
                .find(|raw| raw.entry == detour.entry)
                .and_then(|raw| logic_raw.sites.iter().find(|site| site.start == raw.rva))
                .map_or("unnamed", |site| site.name.as_str());
            say(
                api,
                LOG_INFO,
                &format!(
                    "payload-entry core.logic {name} feature={} address={:#x} site-rva={:#x}",
                    detour.feature,
                    logic.block + detour.entry,
                    detour.rva
                ),
            );
        }
        for detour in &game_patch.hooks {
            say(
                api,
                LOG_INFO,
                &format!(
                    "payload-entry core.game feature={} address={:#x} site-rva={:#x}",
                    detour.feature,
                    game.block + detour.entry,
                    detour.rva
                ),
            );
        }
        let hook = logic_patch
            .detours
            .iter()
            .find(|h| h.feature == 7)
            .ok_or("missing diagnostics hook")?;
        let lobby_connect = match install::game_site(&game_raw, &game_target, scan, "lobby_connect")
        {
            Ok(rva) => Some(game_target.base as usize + rva),
            Err(e) => {
                say(
                    api,
                    LOG_WARN,
                    &format!("multiplayer: {e}; online play is not guarded"),
                );
                None
            }
        };
        let record = format!(
            "pid={:#x}\nbase={:#x}\nblock={:#x}\ntrace={:#x}\nentry={:#x}\n",
            logic_target.process_id, logic_target.base as usize, logic.block, hook.rva, hook.entry
        );
        let mut native_entries = std::collections::BTreeMap::new();
        // Names are resolved through the same descriptor/relocation decision as
        // assembly patches. Only ordinary function entries are eligible here.
        for name in ["firing_set", "firing_ui", "setter", "is_selected"] {
            let site = logic_raw
                .sites
                .iter()
                .find(|s| s.name == name)
                .ok_or("native entry missing its signature")?;
            let raw = logic_raw
                .detours
                .iter()
                .find(|h| h.rva == site.start)
                .ok_or("native entry missing its patch")?;
            let resolved = logic_patch
                .detours
                .iter()
                .find(|h| h.entry == raw.entry && h.feature == raw.feature)
                .ok_or("native entry missing after relocation")?;
            native_entries.insert(
                name.to_owned(),
                (resolved.feature, logic_target.base as usize + resolved.rva),
            );
        }
        Ok(Runtime {
            tab_modifier_offset: game_patch.tab_modifier_offset,
            game_block_bytes: game_patch.block_bytes,
            preview_dim_cell: logic_patch.preview_dim_cell,
            logic_block_bytes: logic_patch.block_bytes,
            logic,
            game,
            installed: std::sync::Mutex::new(0),
            available,
            record,
            lobby_connect,
            native_entries,
        })
    };
    match prepare() {
        Ok(runtime) => {
            if RUNTIME.set(runtime).is_err() {
                return 1;
            }
            if defiance_feature_sdk::services::available()
                && unsafe {
                    defiance_feature_sdk::services::register(c"game-access", 1, &game::API)
                }
                .is_err()
            {
                return 1;
            }
            if defiance_feature_sdk::services::available()
                && unsafe {
                    defiance_feature_sdk::services::register(c"ammo-menu", 1, &ammo_menu::API)
                }
                .is_err()
            {
                return 1;
            }
            if let Some(address) = RUNTIME.get().and_then(|runtime| runtime.lobby_connect) {
                multiplayer::install(api, address);
            }
            say(
                api,
                LOG_INFO,
                "shared payloads prepared; gameplay patches belong to feature plugins",
            );
            0
        }
        Err(e) => {
            say(api, LOG_ERROR, &format!("runtime preparation failed: {e}"));
            1
        }
    }
}

/// Called during a feature's init, so Api::patch_bytes charges that feature.
/// No feature can run after failed preparation. Full process restart is the
/// lifecycle; the shared blocks are never freed while game code can reach them.
#[no_mangle]
pub unsafe extern "C" fn defiance_install_feature_v1(api: *const Api, feature: u32) -> i32 {
    unsafe { install_feature(api, feature, core::ptr::null_mut(), &[]) }
}

/// Rust pickup uses exactly the site approved by Core's normal
/// build checks. The host owns its call hook, including any near relay.
#[no_mangle]
pub unsafe extern "C" fn defiance_install_pickup_rust_v1(
    api: *const Api,
    detour: *mut c_void,
) -> i32 {
    if detour.is_null() {
        return 1;
    }
    unsafe { install_feature(api, 1, detour, &[]) }
}

#[no_mangle]
pub unsafe extern "C" fn defiance_install_native_v1(
    api: *const Api,
    feature: u32,
    replacements: *const defiance_api::NativeReplacementV1,
    count: usize,
) -> i32 {
    if replacements.is_null() || count == 0 || count > 16 {
        return 1;
    }
    unsafe {
        install_feature(
            api,
            feature,
            core::ptr::null_mut(),
            core::slice::from_raw_parts(replacements, count),
        )
    }
}

unsafe fn install_feature(
    api: *const Api,
    feature: u32,
    detour: *mut c_void,
    replacements: &[defiance_api::NativeReplacementV1],
) -> i32 {
    if api.is_null() || !(1..=defiance_core::features::KNOWN_FEATURES).contains(&feature) {
        return 1;
    }
    let api = unsafe { &*api };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    let Some(runtime) = RUNTIME.get() else {
        say(
            api,
            LOG_ERROR,
            "feature refused: core runtime did not initialize",
        );
        return 1;
    };
    let mut installed = runtime.installed.lock().unwrap();
    if *installed & (1 << feature) != 0 {
        return 0;
    }
    if !defiance_core::features::mask_has(runtime.available, feature) {
        say(
            api,
            LOG_ERROR,
            "feature refused: its site could not be prepared in this build",
        );
        return 1;
    }
    let writes: Vec<_> = runtime
        .logic
        .writes
        .iter()
        .chain(&runtime.game.writes)
        .filter(|w| w.feature == feature)
        .collect();
    let mut native = std::collections::BTreeMap::new();
    for replacement in replacements {
        if replacement.name.is_null() || replacement.detour.is_null() {
            return 1;
        }
        let Ok(name) = (unsafe { CStr::from_ptr(replacement.name) }).to_str() else {
            return 1;
        };
        let Some(&(owner, address)) = runtime.native_entries.get(name) else {
            return 1;
        };
        if owner != feature
            || !writes.iter().any(|w| w.address == address)
            || native.insert(address, replacement.detour).is_some()
        {
            return 1;
        }
    }
    if !detour.is_null()
        && (writes.len() != 1 || writes[0].before.len() != 5 || writes[0].before[0] != 0xe8)
    {
        say(
            api,
            LOG_ERROR,
            "Rust pickup refused: expected one verified rel32 call",
        );
        return 1;
    }
    // Validate the whole feature before changing either module.
    for write in &writes {
        let now =
            unsafe { core::slice::from_raw_parts(write.address as *const u8, write.before.len()) };
        if now != write.before {
            say(
                api,
                LOG_ERROR,
                "feature refused: patch site changed since preparation",
            );
            return 1;
        }
    }
    if feature == 2 && runtime.preview_dim_cell + 8 <= runtime.logic_block_bytes {
        // The preview's hooks read the callback, so it is in place before them.
        unsafe {
            ((runtime.logic.block + runtime.preview_dim_cell) as *mut usize).write_volatile(
                preview::dim_material as unsafe extern "C" fn(usize, usize) -> usize as usize,
            )
        };
    }
    for write in &writes {
        let result = if let Some(&replacement) = native.get(&write.address) {
            if write.before.len() >= 14 {
                // Complete replacement needs no original trampoline. An absolute
                // branch can cover bodies containing relative instructions.
                let mut branch = vec![0xff, 0x25, 0, 0, 0, 0];
                branch.extend_from_slice(&(replacement as usize as u64).to_le_bytes());
                branch.resize(write.before.len(), 0x90);
                unsafe {
                    (api.patch_bytes)(
                        write.address as *mut c_void,
                        write.before.as_ptr(),
                        branch.as_ptr(),
                        branch.len(),
                    )
                }
            } else {
                let mut original = core::ptr::null_mut();
                unsafe {
                    (api.hook_exact)(
                        write.address as *mut c_void,
                        replacement,
                        write.before.len(),
                        &mut original,
                    )
                }
            }
        } else if detour.is_null() {
            unsafe {
                (api.patch_bytes)(
                    write.address as *mut c_void,
                    write.before.as_ptr(),
                    write.after.as_ptr(),
                    write.before.len(),
                )
            }
        } else {
            let mut original = core::ptr::null_mut();
            unsafe { (api.hook_call)(write.address as *mut c_void, detour, &mut original) }
        };
        if result != 0 {
            // The host rolls back everything owned by this failed feature.
            say(
                api,
                LOG_ERROR,
                "feature patch failed; host will roll back its owned spans",
            );
            return 1;
        }
    }
    *installed |= 1 << feature;
    if feature == 2 {
        // The building TAB code reads the modifier from the game block; 0
        // leaves only the plain TAB cycle.
        let key = tab_modifier_key(&config_text(
            api,
            "defiance.selection",
            "squad_tab_modifier",
        ));
        if runtime.tab_modifier_offset + 4 <= runtime.game_block_bytes {
            unsafe {
                ((runtime.game.block + runtime.tab_modifier_offset) as *mut u32).write_volatile(key)
            };
        }
    }
    if !detour.is_null() {
        say(api, LOG_INFO, "pickup: Rust chooser installed");
    }
    if !native.is_empty() {
        say(
            api,
            LOG_INFO,
            &format!(
                "feature {feature}: {} verified Rust function replacements",
                native.len()
            ),
        );
    }
    if feature == 7 {
        let _ = std::fs::write(
            game_dir().join("defiance-pickup-inject.block"),
            &runtime.record,
        );
    }
    say(
        api,
        LOG_INFO,
        &format!(
            "feature {feature}: installed {} owned patch spans",
            writes.len()
        ),
    );
    0
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: NAME.as_ptr().cast(),
        version: VERSION.as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();

#[cfg(test)]
mod tests {
    use super::*;

    /// Each variant's roster slot must be the one its payload was assembled
    /// with, read back from select-squad's `call [rax+disp32]`, so the shared
    /// service and the payload cannot drift apart.
    #[test]
    fn a_variant_roster_slot_matches_its_descriptor() {
        for variant in VARIANTS {
            let descriptor = Patch::parse(variant.logic_descriptor);
            let after = &descriptor.select_squad_after;
            let called: Vec<usize> = after
                .windows(6)
                .filter(|w| w[0] == 0xff && w[1] == 0x90)
                .map(|w| u32::from_le_bytes(w[2..6].try_into().unwrap()) as usize)
                .collect();
            assert!(
                called.contains(&variant.roster_slot),
                "select_squad_after calls {called:x?}, not the roster slot {:#x}",
                variant.roster_slot
            );
        }
    }

    /// The runtime ties a native entry to its patch by `site.start == hook.rva`.
    /// A variant whose sites stayed at reference addresses failed here with
    /// "native entry missing its patch"; this keeps them in step.
    #[test]
    fn a_variant_ties_each_native_entry_to_its_detour() {
        for variant in VARIANTS {
            let logic = Patch::parse(variant.logic_descriptor);
            for name in ["firing_set", "firing_ui", "setter", "is_selected"] {
                let site = logic
                    .sites
                    .iter()
                    .find(|s| s.name == name)
                    .unwrap_or_else(|| panic!("{name} has no site"));
                assert!(
                    logic.detours.iter().any(|h| h.rva == site.start),
                    "{name}: no detour at its site {:#x}",
                    site.start
                );
            }
        }
    }

    /// A variant is chosen only when both module hashes match: a logic.dll
    /// from one build with a game.dll from another is never mixed.
    #[test]
    fn a_variant_needs_both_hashes() {
        for variant in VARIANTS {
            assert!(variant_for(variant.logic_sha, variant.game_sha).is_some());
            assert!(variant_for(variant.logic_sha, "not a real hash").is_none());
            assert!(variant_for("not a real hash", variant.game_sha).is_none());
        }
        assert!(variant_for("no", "no").is_none());
    }

    /// `allow_unknown_build` never pairs a recognized module with a stranger:
    /// a variant's logic.dll with an unknown game.dll (or the reverse) would
    /// otherwise get the reference payload and its stale class offsets.
    #[test]
    fn unknown_builds_are_never_mixed_with_known_modules() {
        let logic = Patch::parse(DESCRIPTOR);
        let game = GamePatch::parse(GAME_DESCRIPTOR);
        let pick = |l: &str, g: &str, allow: bool| choose(l, g, &logic, &game, allow);
        let (logic_ref, game_ref) = (logic.source_sha256.clone(), game.source_sha256.clone());
        assert!(matches!(
            pick(&logic_ref, &game_ref, false),
            Some(Choice::Reference)
        ));
        for &(l, g) in VERIFIED_PAIRS {
            assert!(matches!(pick(l, g, false), Some(Choice::Reference)));
        }
        for v in VARIANTS {
            assert!(
                matches!(pick(v.logic_sha, v.game_sha, false), Some(Choice::Variant(found)) if std::ptr::eq(found, v))
            );
            for allow in [false, true] {
                assert!(
                    pick(v.logic_sha, "unknown", allow).is_none(),
                    "variant logic with an unknown game"
                );
                assert!(
                    pick("unknown", v.game_sha, allow).is_none(),
                    "unknown logic with a variant game"
                );
                assert!(
                    pick(v.logic_sha, &game_ref, allow).is_none(),
                    "variant logic with the reference game"
                );
                assert!(
                    pick(&logic_ref, v.game_sha, allow).is_none(),
                    "reference logic with a variant game"
                );
            }
        }
        assert!(pick(&logic_ref, "unknown", true).is_none());
        assert!(pick("unknown", &game_ref, true).is_none());
        assert!(pick("unknown", "unknown", false).is_none());
        assert!(matches!(
            pick("unknown", "also unknown", true),
            Some(Choice::Unknown)
        ));
    }

    /// The pair table and the per-module verified lists must agree: a pair
    /// whose hashes are not both listed would never be selected, and a listed
    /// hash without a pair would be unreachable.
    #[test]
    fn verified_pairs_are_listed_by_both_descriptors() {
        let logic = Patch::parse(DESCRIPTOR);
        let game = GamePatch::parse(GAME_DESCRIPTOR);
        for &(logic_sha, game_sha) in VERIFIED_PAIRS {
            assert!(
                logic.verified.iter().any(|s| s == logic_sha),
                "logic {logic_sha} is not verified"
            );
            assert!(
                game.verified.iter().any(|s| s == game_sha),
                "game {game_sha} is not verified"
            );
        }
    }
}
