//! Build identity, patch units and shared services for the built-in feature
//! plugins.
//!
//! Core recognises the (logic.dll, game.dll) pair the game loaded: the
//! reference build, a verified build whose sites are found by signature, or a
//! layout variant resolved offline. Each built-in feature's code is a set of
//! patch units (`tools/units.py`), one per module it patches, resolved per
//! build (`tools/variant.py`). Core embeds every supported build's units; when
//! a feature plugin installs, Core relocates that feature's units if the build
//! calls for it, links them into a pool near their module and writes their
//! sites under the calling plugin's ownership (`Api::patch_bytes`).

use core::ffi::{c_char, c_void, CStr};
use defiance_api::{
    Api, PatchContractV1, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO, LOG_WARN, PATCH_KIND_CALL,
    PATCH_KIND_ENTRY,
};
use defiance_core::apply::{module_image, Process};
use defiance_core::install::{needs_relocation, Scan};
use defiance_core::unit::Unit;
use defiance_core::Target;
use defiance_feature_sdk::units::Embedded;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
mod affinity;
mod ammo_menu;
mod game;
mod grass;
/// Diagnostic, never shipped (feature `inspect-probe`), kept privately.
#[cfg(feature = "inspect-probe")]
mod inspect_probe;
mod inverse;
mod mesh_sort;
mod multiplayer;
mod patch;
#[cfg(feature = "render-profile")]
mod render_profile;
mod session;
mod shadow;
mod shadow_fit;
mod sites;
#[cfg(feature = "test-host")]
pub mod test_host;
mod tree_sway;
mod view_sort;
defiance_feature_sdk::service_handshake!();

#[cfg(feature = "parity-test")]
#[no_mangle]
pub extern "C" fn defiance_test_game_access() -> *const defiance_api::GameAccessV1 {
    &game::API
}

static NAME: &[u8] = b"defiance.core\0";
static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

/// Every supported build's units, from `tools/variants/<build>/units/`
/// (`build.rs`).
static UNITS: &[Embedded] = include!(concat!(env!("OUT_DIR"), "/units.rs"));

/// The build the units were written for, and relocated from.
const REFERENCE: &str = "reference";

/// A build whose classes gained members: its units were assembled from the
/// same source with that build's layout (`tools/layouts/*.json`) and resolved
/// against its DLLs (`tools/variant.py`), so they apply without relocation.
/// Both modules' hashes are the variant's identity: a logic.dll from one build
/// with a game.dll from another is refused, never patched.
struct Variant {
    /// The folder under `tools/variants/` its units come from.
    build: &'static str,
    logic_sha: &'static str,
    game_sha: &'static str,
    /// The build's squad AI facet roster getter slot (reference 0x3b8); one
    /// class offset the shared `GameAccessV1` table reads directly.
    roster_slot: usize,
}

static VARIANTS: &[Variant] = &[
    Variant {
        build: "gog-2026-09-14",
        logic_sha: "eb8674f1d16595a3e9cf6a9ec0062735b1184976495d8d6ade36f7e2574e8aab",
        game_sha: "bc2af42369f9f6fe70e206ae4846f8f0e46ac01cca9a325c8159ee04a9b5e405",
        roster_slot: 0x3d0,
    },
    Variant {
        build: "steam-2026-09-22",
        logic_sha: "30264904e1d5199b954bafbd7828cf7190930c246d35fa7b94eefa915e8f0c38",
        game_sha: "d926a213731d73bac8ccc56b50e2b4292fe8613c7a9bb7c8b9fc9132c122ed25",
        roster_slot: 0x3d0,
    },
    Variant {
        build: "gog-2026-09-25",
        logic_sha: "1216d627c7288c7db6940168363be582232ed4d3cb860b8b8c8d7489652eca74",
        game_sha: "8ec30a0b59aebf2240f00229d54a0f58e2e338f9ab3511046c9ff36970dd1489",
        roster_slot: 0x3d0,
    },
    Variant {
        build: "steam-2026-09-25",
        logic_sha: "adb3ad95926036809b4e554b466bef33d4ac7aa5303e59a9e4a940890bc334b5",
        game_sha: "c336b5ed4a367628a9c370457d82b1d4e75cab9007cffe5354e27688e836f98e",
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
    /// A layout-changing build: its own units, applied without relocation.
    Variant(&'static Variant),
    /// The reference pair, or a verified pair relocation carries.
    Reference,
    /// `allow_unknown_build`: neither module is recognized; patch by signature.
    Unknown,
}

/// The units for a pair, or None to refuse it. `logic` and `game` are the
/// reference build's Core units, which name its hashes and the verified
/// builds. `allow_unknown_build` opens the signature path only when neither
/// module is one any supported pair names (the reference, a verified build or
/// a variant), so a recognized module is never patched beside one from
/// another build.
fn choose(
    logic_sha: &str,
    game_sha: &str,
    logic: &Unit,
    game: &Unit,
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

/// A build's units, parsed, each beside its embedded form.
fn units_of(build: &str) -> Result<Vec<(Unit, &'static Embedded)>, String> {
    UNITS
        .iter()
        .filter(|embedded| embedded.build == build)
        .map(|embedded| {
            Unit::parse(embedded.descriptor)
                .map(|unit| (unit, embedded))
                .map_err(|e| format!("{build} {}: {e}", embedded.name))
        })
        .collect()
}

/// Core's own unit for `module` ("logic.dll" or "game.dll"): the build
/// anchors, and the sites of the functions Core hooks itself.
fn core_unit(units: &[(Unit, &'static Embedded)], module: &str) -> Result<Unit, String> {
    units
        .iter()
        .find(|(unit, _)| unit.plugin == "core" && unit.module == module)
        .map(|(unit, _)| unit.clone())
        .ok_or_else(|| format!("no Core unit for {module}"))
}

/// Resolve Core's optional self-hook sites with the same build pair and
/// relocation rules used during init, without allocating pools or publishing
/// services.
fn native_hook_sites(api: &Api) -> Result<(Option<usize>, Option<(usize, usize)>), String> {
    let logic_target = module_target(api, "logic.dll").ok_or("logic.dll is not loaded")?;
    let game_target = module_target(api, "game.dll").ok_or("game.dll is not loaded")?;
    let logic_sha = defiance_core::sha256::file(&logic_target.path)
        .map_err(|e| format!("hashing logic.dll: {e}"))?;
    let game_sha = defiance_core::sha256::file(&game_target.path)
        .map_err(|e| format!("hashing game.dll: {e}"))?;
    let scan = if config_flag(api, "", "allow_unknown_build") {
        Scan::Unknown
    } else {
        Scan::Default
    };
    let reference = units_of(REFERENCE)?;
    let choice = choose(
        &logic_sha,
        &game_sha,
        &core_unit(&reference, "logic.dll")?,
        &core_unit(&reference, "game.dll")?,
        scan == Scan::Unknown,
    )
    .ok_or("logic.dll and game.dll are not a supported build pair")?;
    let (_, units) = match choice {
        Choice::Variant(variant) => (variant.build, units_of(variant.build)?),
        Choice::Reference | Choice::Unknown => (REFERENCE, reference),
    };
    let own = core_unit(&units, "game.dll")?;
    let pristine = needs_relocation(&game_sha, &own.source_sha256, &own.verified, scan)
        .then(|| module_image(&game_target))
        .transpose()?;
    let own = match pristine {
        Some(image) => own
            .relocate(&image, &game_sha)
            .map_err(|e| format!("game.dll: {e}"))?,
        None => own,
    };
    let site = |name: &str| {
        own.sites
            .iter()
            .find(|site| site.name == name)
            .map(|site| game_target.base as usize + site.start)
    };
    let tactical_state = site("tactical_state_ctor").zip(site("tactical_state_dtor"));
    Ok((site("lobby_connect"), tactical_state))
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
/// build can be hashed.
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

/// A plugin's setting as text, lower case; empty when it has none. An empty
/// `section` is the loader's own top level (`allow_unknown_build`).
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

/// A yes/no setting from `defiance-loader.ini`.
fn config_flag(api: &Api, section: &str, key: &str) -> bool {
    matches!(
        config_text(api, section, key).as_str(),
        "true" | "yes" | "on" | "1"
    )
}

/// Executable memory near a module, handed out in 16-byte-aligned pieces
/// that stay for the process's life: a unit's code must be within rel32 reach
/// of its module and of the cells it imports.
struct Pool {
    start: usize,
    size: usize,
    used: Mutex<usize>,
}

/// Each module's pool. Every built-in unit of a build fits with room to spare
/// (tools/units.py prints their sizes).
const POOL_BYTES: usize = 0x10000;

impl Pool {
    /// A slot the loader has held near the module since it loaded, or else the
    /// nearest free one: a loader that predates near-memory, or holds none
    /// within reach.
    fn near(target: &Target) -> Result<Self, String> {
        let held = unsafe { defiance_feature_sdk::services::near_memory() }
            .map(|near| unsafe { (near.take)(target.base as usize, POOL_BYTES) })
            .unwrap_or(0);
        let start = match held {
            0 => Process::open(target.process_id)?.reserve_near(target.base, POOL_BYTES)? as usize,
            held => held,
        };
        Ok(Self {
            start,
            size: POOL_BYTES,
            used: Mutex::new(0),
        })
    }

    fn take(&self, bytes: usize) -> Option<usize> {
        let mut used = self.used.lock().unwrap();
        let at = used.next_multiple_of(16);
        if at + bytes > self.size {
            return None;
        }
        *used = at + bytes;
        Some(self.start + at)
    }
}

/// One loaded game module.
struct Module {
    base: usize,
    sha: String,
    /// The module as it was when Core initialized, before any feature wrote
    /// to it, when this build's sites have to be found by signature; None when
    /// the units describe the build as it is.
    pristine: Option<Vec<u8>>,
    pool: Pool,
}

struct Runtime {
    logic: Module,
    game: Module,
    /// The logic manager's trace ring, which diagnostics, posture and
    /// ammunition units import.
    trace_ring: usize,
    /// The build whose units apply (`BuildV1::name`).
    build: &'static CStr,
    /// Every unit linked so far, by name ([`patch`]).
    linked: Mutex<BTreeMap<String, patch::Linked>>,
    crash_ranges: Option<&'static defiance_api::CrashRangesV1>,
    /// The game's lobby connection, which the multiplayer guard hooks; None
    /// when its signature was not found in this build.
    lobby_connect: Option<usize>,
    /// The tactical state's constructor and destructor, which the mission
    /// report hooks; None when either signature was not found.
    tactical_state: Option<(usize, usize)>,
}
static RUNTIME: std::sync::OnceLock<Runtime> = std::sync::OnceLock::new();

/// The logic manager's trace ring: an index, then 32 sixteen-byte entries.
const TRACE_RING_BYTES: usize = 0x10 + 32 * 16;

pub(crate) fn say(api: &Api, level: u32, message: &str) {
    let text = std::ffi::CString::new(message.replace('\0', "")).unwrap_or_default();
    unsafe { (api.log)(level, text.as_ptr()) };
}

impl Runtime {
    pub(crate) fn module(&self, unit: &Unit) -> &Module {
        if unit.module == "game.dll" {
            &self.game
        } else {
            &self.logic
        }
    }
}

impl Runtime {
    /// Where `--select-probe` finds the diagnostics: the trace hook it checks
    /// still reaches the diagnostics unit, the trace ring, the census and the
    /// ammunition scratch, each once its unit is linked.
    pub(crate) fn record(&self) {
        let linked = self.linked.lock().unwrap();
        let Some(diagnostics) = linked.get("diagnostics-logic").map(|l| &l.prepared) else {
            return;
        };
        let Some(hook) = diagnostics
            .unit
            .writes
            .iter()
            .find(|w| w.label == "select_trace")
        else {
            return;
        };
        let cell = |unit: &str, name: &str| {
            linked
                .get(unit)
                .and_then(|unit| unit.prepared.cell(name))
                .map_or(0, |(address, _)| address)
        };
        let record = format!(
            "pid={:#x}\nbase={:#x}\nblock={:#x}\ntrace={:#x}\nentry={:#x}\nring={:#x}\ncensus={:#x}\nscratch={:#x}\n",
            unsafe { GetCurrentProcessId() },
            self.logic.base,
            diagnostics.at,
            hook.rva,
            hook.entry,
            self.trace_ring,
            cell("diagnostics-logic", "census"),
            cell("ammunition-logic", "scratch"),
        );
        let _ = std::fs::write(game_dir().join("defiance-pickup-inject.block"), record);
    }
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    if api.is_null() {
        return 1;
    }
    let api = unsafe { &*api };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    let prepare = || -> Result<Runtime, String> {
        let logic_target = module_target(api, "logic.dll").ok_or("logic.dll is not loaded")?;
        let game_target = module_target(api, "game.dll").ok_or("game.dll is not loaded")?;
        let scan = if config_flag(api, "", "allow_unknown_build") {
            Scan::Unknown
        } else {
            Scan::Default
        };
        // The two hashes together choose the units: a build's classes moved
        // relative to another's, so a logic.dll and game.dll from different
        // builds must never be mixed.
        let logic_sha = defiance_core::sha256::file(&logic_target.path)
            .map_err(|e| format!("hashing logic.dll: {e}"))?;
        let game_sha = defiance_core::sha256::file(&game_target.path)
            .map_err(|e| format!("hashing game.dll: {e}"))?;
        let reference = units_of(REFERENCE)?;
        let choice = choose(
            &logic_sha,
            &game_sha,
            &core_unit(&reference, "logic.dll")?,
            &core_unit(&reference, "game.dll")?,
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
        let (build, units) = match choice {
            Choice::Variant(v) => {
                say(
                    api,
                    LOG_INFO,
                    &format!(
                        "both modules match the supported build {}; using its units",
                        v.build
                    ),
                );
                (v.build, units_of(v.build)?)
            }
            Choice::Reference => (REFERENCE, reference),
            Choice::Unknown => {
                say(api, LOG_WARN, "allow_unknown_build: patching an unrecognized logic.dll/game.dll pair by signature");
                (REFERENCE, reference)
            }
        };
        // Resolve both unmodified modules before any feature installs hooks:
        // a build found by signature keeps the image as it is now, so a later
        // feature's signatures are matched against it, not against another
        // feature's writes.
        let mut modules = Vec::new();
        for (target, sha) in [(logic_target, logic_sha), (game_target, game_sha)] {
            let name = target
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            let own = core_unit(&units, &name)?;
            let pristine = needs_relocation(&sha, &own.source_sha256, &own.verified, scan)
                .then(|| module_image(&target))
                .transpose()?;
            let own = match &pristine {
                Some(image) => own
                    .relocate(image, &sha)
                    .map_err(|e| format!("{name}: {e}"))?,
                None => own,
            };
            // The build anchors every unit relies on: bytes the module must
            // hold before anything is written.
            for anchor in &own.anchors {
                let now = unsafe {
                    core::slice::from_raw_parts(target.base.add(anchor.rva), anchor.bytes.len())
                };
                if anchor.rva + anchor.bytes.len() > target.size || now != anchor.bytes {
                    return Err(format!("{name}: module anchor mismatch"));
                }
            }
            let pool = Pool::near(&target)?;
            modules.push((
                Module {
                    base: target.base as usize,
                    sha,
                    pristine,
                    pool,
                },
                own,
            ));
        }
        let (game, core_game) = modules.pop().unwrap();
        let (logic, _) = modules.pop().unwrap();
        let crash_ranges = unsafe { defiance_feature_sdk::services::crash_ranges() };
        for (label, module) in [
            (c"core.logic unit pool", &logic),
            (c"core.game unit pool", &game),
        ] {
            say(
                api,
                LOG_INFO,
                &format!(
                    "{} at {:#x}..{:#x}",
                    label.to_string_lossy(),
                    module.pool.start,
                    module.pool.start + module.pool.size
                ),
            );
        }
        let trace_ring = logic
            .pool
            .take(TRACE_RING_BYTES)
            .ok_or("no room for the trace ring")?;
        if let Some(ranges) = crash_ranges {
            unsafe {
                (ranges.map)(
                    trace_ring,
                    trace_ring + TRACE_RING_BYTES,
                    c"core.logic trace ring".as_ptr(),
                )
            };
        }
        let site = |name: &str| {
            core_game
                .sites
                .iter()
                .find(|site| site.name == name)
                .map(|site| game.base + site.start)
                .ok_or_else(|| format!("the build has no {name} site"))
        };
        let lobby_connect = match site("lobby_connect") {
            Ok(address) => Some(address),
            Err(e) => {
                say(
                    api,
                    LOG_WARN,
                    &format!("multiplayer: {e}; online play is not guarded"),
                );
                None
            }
        };
        let tactical_state = match (site("tactical_state_ctor"), site("tactical_state_dtor")) {
            (Ok(ctor), Ok(dtor)) => Some((ctor, dtor)),
            (Err(e), _) | (_, Err(e)) => {
                say(
                    api,
                    LOG_WARN,
                    &format!("session: {e}; hot reload stays off"),
                );
                None
            }
        };
        Ok(Runtime {
            logic,
            game,
            trace_ring,
            build: std::ffi::CString::new(build)
                .map(|name| &*Box::leak(name.into_boxed_c_str()))
                .map_err(|_| "a build name holds a NUL")?,
            linked: Mutex::new(BTreeMap::new()),
            crash_ranges,
            lobby_connect,
            tactical_state,
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
            if defiance_feature_sdk::services::available()
                && unsafe { defiance_feature_sdk::services::register(c"patch", 1, &patch::API) }
                    .is_err()
            {
                return 1;
            }
            if defiance_feature_sdk::services::available()
                && unsafe { defiance_feature_sdk::services::register(c"build", 1, &BUILD) }.is_err()
            {
                return 1;
            }
            if let Some(address) = RUNTIME.get().and_then(|runtime| runtime.lobby_connect) {
                multiplayer::install(api, address);
            }
            if let Some((ctor, dtor)) = RUNTIME.get().and_then(|runtime| runtime.tactical_state) {
                session::install(api, ctor, dtor);
            }
            affinity::install(api, &config_text(api, "loader", "main_thread_cpus"));
            grass::install(api, &config_text(api, "loader", "grass_sort"));
            inverse::install(api, &config_text(api, "loader", "matrix_inverse"));
            view_sort::install(api, &config_text(api, "loader", "view_sort"));
            mesh_sort::install(api, &config_text(api, "loader", "mesh_sort"));
            shadow::install(api, &config_text(api, "loader", "shadow_cascades"));
            shadow_fit::install(api, &config_text(api, "loader", "shadow_fit"));
            tree_sway::install(api, &config_text(api, "loader", "tree_sway"));
            #[cfg(feature = "render-profile")]
            render_profile::install(api);
            #[cfg(feature = "inspect-probe")]
            inspect_probe::install(api);
            0
        }
        Err(e) => {
            say(api, LOG_ERROR, &format!("runtime preparation failed: {e}"));
            1
        }
    }
}

unsafe extern "C" fn build_name() -> *const c_char {
    RUNTIME
        .get()
        .map_or(core::ptr::null(), |runtime| runtime.build.as_ptr())
}

type ContractRequest = (usize, defiance_feature_sdk::contract::Patch);
static CONTRACT_API: AtomicUsize = AtomicUsize::new(0);
static CONTRACT_REQUESTS: Mutex<Vec<ContractRequest>> = Mutex::new(Vec::new());
static CONTRACT_EXPORT_LOCK: Mutex<()> = Mutex::new(());

fn contract_location(target: usize) -> Option<(&'static CStr, usize, usize)> {
    let api = CONTRACT_API.load(Ordering::Acquire) as *const Api;
    let api = unsafe { api.as_ref() }?;
    for module in [c"logic.dll", c"game.dll", c"world2.dll", c"galileo.dll"] {
        let base = unsafe { (api.module_base)(module.as_ptr()) } as usize;
        if base == 0 {
            continue;
        }
        let size = unsafe { (api.module_size)(base as *mut c_void) };
        let Some(end) = base.checked_add(size) else {
            continue;
        };
        if (base..end).contains(&target) {
            return Some((module, base, size));
        }
    }
    None
}

fn contract_patch(
    target: usize,
    kind: u32,
    before: Vec<u8>,
    after: Option<Vec<u8>>,
) -> Option<ContractRequest> {
    let (module, base, size) = contract_location(target)?;
    let rva = target.checked_sub(base)?;
    if before.is_empty()
        || rva.checked_add(before.len())? > size
        || after
            .as_ref()
            .is_some_and(|replacement| replacement.len() != before.len())
    {
        return None;
    }
    if kind == PATCH_KIND_CALL && (before.len() != 5 || before[0] != 0xe8) {
        return None;
    }
    if kind == PATCH_KIND_ENTRY && defiance_core::decode::validate_copy(&before).is_err() {
        return None;
    }
    Some((
        target,
        defiance_feature_sdk::contract::Patch {
            module,
            rva,
            kind,
            before,
            after,
        },
    ))
}

unsafe fn record_contract_hook(
    target: *mut c_void,
    kind: u32,
    exact_len: Option<usize>,
    original: *mut *mut c_void,
) -> i32 {
    if target.is_null() {
        return -1;
    }
    let address = target as usize;
    let Some((_, base, size)) = contract_location(address) else {
        return -1;
    };
    let Some(rva) = address.checked_sub(base) else {
        return -1;
    };
    let Some(available) = size.checked_sub(rva) else {
        return -1;
    };
    let window = available.min(16);
    if window == 0 {
        return -1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(target.cast::<u8>(), window) };
    let len = match exact_len {
        Some(len) => len,
        None => match defiance_core::decode::displaced(bytes, 5) {
            Ok(len) => len,
            Err(_) => return -1,
        },
    };
    let Some(before) = bytes.get(..len).map(<[u8]>::to_vec) else {
        return -1;
    };
    let Some(request) = contract_patch(address, kind, before, None) else {
        return -1;
    };
    // Contract collection runs after plugin init. A fake call hook must return
    // the call's original callee just like the real API; returning the site
    // makes a wrapper recurse through its own detour.
    let original_target = if kind == PATCH_KIND_CALL {
        if request.1.before.len() != 5 || request.1.before[0] != 0xe8 {
            return -1;
        }
        let rel = i32::from_le_bytes([
            request.1.before[1],
            request.1.before[2],
            request.1.before[3],
            request.1.before[4],
        ]) as isize;
        (address as isize + 5 + rel) as *mut c_void
    } else {
        target
    };
    CONTRACT_REQUESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(request);
    if !original.is_null() {
        unsafe { original.write(original_target) };
    }
    0
}

unsafe extern "C" fn contract_hook(
    target: *mut c_void,
    _detour: *mut c_void,
    original: *mut *mut c_void,
) -> i32 {
    unsafe { record_contract_hook(target, PATCH_KIND_ENTRY, None, original) }
}

unsafe extern "C" fn contract_hook_exact(
    target: *mut c_void,
    _detour: *mut c_void,
    displaced: usize,
    original: *mut *mut c_void,
) -> i32 {
    unsafe { record_contract_hook(target, PATCH_KIND_ENTRY, Some(displaced), original) }
}

unsafe extern "C" fn contract_hook_call(
    target: *mut c_void,
    _detour: *mut c_void,
    original: *mut *mut c_void,
) -> i32 {
    unsafe { record_contract_hook(target, PATCH_KIND_CALL, Some(5), original) }
}

unsafe extern "C" fn contract_patch_bytes(
    target: *mut c_void,
    before: *const u8,
    after: *const u8,
    length: usize,
) -> i32 {
    if target.is_null() || before.is_null() || after.is_null() || length == 0 {
        return -1;
    }
    let before = unsafe { core::slice::from_raw_parts(before, length) }.to_vec();
    let after = unsafe { core::slice::from_raw_parts(after, length) }.to_vec();
    if unsafe { core::slice::from_raw_parts(target.cast::<u8>(), length) } != before.as_slice() {
        return -1;
    }
    let Some(request) = contract_patch(
        target as usize,
        defiance_api::PATCH_KIND_BYTES,
        before,
        Some(after),
    ) else {
        return -1;
    };
    CONTRACT_REQUESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(request);
    0
}

unsafe extern "C" fn contract_unhook(target: *mut c_void) -> i32 {
    CONTRACT_REQUESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .retain(|(address, _)| *address != target as usize);
    0
}

unsafe extern "C" fn quiet_contract_log(_: u32, _: *const c_char) {}

fn collect_core_contract(api: &Api) -> Vec<defiance_feature_sdk::contract::Patch> {
    let mut fake = unsafe { core::ptr::read(api) };
    fake.log = quiet_contract_log;
    fake.hook = contract_hook;
    fake.hook_exact = contract_hook_exact;
    fake.hook_call = contract_hook_call;
    fake.unhook = contract_unhook;
    fake.patch_bytes = contract_patch_bytes;
    let fake = &fake;

    let affinity = config_text(fake, "loader", "main_thread_cpus");
    if affinity != "engine" {
        let _ = affinity::remove_pin(fake);
    }
    grass::install(fake, &config_text(fake, "loader", "grass_sort"));
    inverse::install(fake, &config_text(fake, "loader", "matrix_inverse"));
    view_sort::install(fake, &config_text(fake, "loader", "view_sort"));
    mesh_sort::install(fake, &config_text(fake, "loader", "mesh_sort"));
    shadow::with_preserved_hook_state(|| {
        shadow::install(fake, &config_text(fake, "loader", "shadow_cascades"));
    });
    shadow_fit::install(fake, &config_text(fake, "loader", "shadow_fit"));
    tree_sway::contract(fake);
    #[cfg(feature = "render-profile")]
    render_profile::contract(fake);
    #[cfg(feature = "inspect-probe")]
    inspect_probe::install(fake);

    if let Ok((lobby, tactical_state)) = native_hook_sites(api) {
        if let Some(lobby) = lobby.filter(|_| multiplayer::guard_available()) {
            let mut original = core::ptr::null_mut();
            let result = unsafe {
                (fake.hook)(
                    lobby as *mut c_void,
                    (fake as *const Api).cast_mut().cast(),
                    &mut original,
                )
            };
            if result == 0 && !original.is_null() {
                if let Some(message) = multiplayer::message_target(fake) {
                    let mut message_original = core::ptr::null_mut();
                    unsafe {
                        (fake.hook)(
                            message as *mut c_void,
                            (fake as *const Api).cast_mut().cast(),
                            &mut message_original,
                        )
                    };
                }
            }
        }
        if let Some((constructor, destructor)) = tactical_state
            .filter(|_| unsafe { defiance_feature_sdk::services::session() }.is_some())
        {
            for target in [constructor, destructor] {
                let mut original = core::ptr::null_mut();
                unsafe {
                    (fake.hook)(
                        target as *mut c_void,
                        (fake as *const Api).cast_mut().cast(),
                        &mut original,
                    )
                };
            }
        }
    }
    CONTRACT_REQUESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .drain(..)
        .map(|(_, patch)| patch)
        .collect()
}

unsafe extern "C" fn patch_contract(api: *const Api) -> *const PatchContractV1 {
    let Some(host) = (unsafe { api.as_ref() }) else {
        return core::ptr::null();
    };
    if host.abi_version != ABI_VERSION || host.reserved != 0 {
        return core::ptr::null();
    }
    let _guard = CONTRACT_EXPORT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    CONTRACT_API.store(api as usize, Ordering::Release);
    CONTRACT_REQUESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    let patches = collect_core_contract(host);
    CONTRACT_API.store(0, Ordering::Release);
    unsafe { defiance_feature_sdk::contract::build(api, patches) }
}

/// `defiance.core` / `build`, version 1.
static BUILD: defiance_api::BuildV1 = defiance_api::BuildV1 { name: build_name };

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

#[no_mangle]
pub unsafe extern "C" fn defiance_patch_contract_v1(api: *const Api) -> *const PatchContractV1 {
    unsafe { patch_contract(api) }
}

defiance_feature_sdk::crash_handshake!();

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::unit::Kind;

    /// Every build's units parse, each feature unit has a Core pair beside it,
    /// and each build carries the same set of units.
    #[test]
    fn every_build_has_every_unit() {
        let names = |build: &str| {
            let mut names: Vec<String> = units_of(build)
                .unwrap()
                .into_iter()
                .map(|(unit, _)| unit.name)
                .collect();
            names.sort();
            names
        };
        let reference = names(REFERENCE);
        assert!(reference.contains(&"core-logic".to_string()));
        assert!(reference.contains(&"core-game".to_string()));
        for variant in VARIANTS {
            assert_eq!(names(variant.build), reference, "{}", variant.build);
            let units = units_of(variant.build).unwrap();
            assert_eq!(
                core_unit(&units, "logic.dll").unwrap().source_sha256,
                variant.logic_sha
            );
            assert_eq!(
                core_unit(&units, "game.dll").unwrap().source_sha256,
                variant.game_sha
            );
        }
    }

    /// A tracked unit of a build, read from `tools/variants`: selection's and
    /// firing's are theirs, not Core's.
    fn tracked(build: &str, name: &str) -> Unit {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/variants")
            .join(build)
            .join("units")
            .join(format!("{name}.json"));
        Unit::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Each variant's roster slot must be the one selection's unit was
    /// assembled with, read back from select-squad's `call [rax+disp32]`, so
    /// the shared service and the payload cannot drift apart.
    #[test]
    fn a_variant_roster_slot_matches_its_units() {
        for (build, slot) in VARIANTS
            .iter()
            .map(|v| (v.build, v.roster_slot))
            .chain([(REFERENCE, REFERENCE_ROSTER_SLOT)])
        {
            let selection = tracked(build, "selection-logic");
            let called: Vec<usize> = selection
                .writes
                .iter()
                .filter(|w| w.kind == Kind::Edit)
                .flat_map(|w| {
                    w.after
                        .windows(6)
                        .filter(|w| w[0] == 0xff && w[1] == 0x90)
                        .map(|w| u32::from_le_bytes(w[2..6].try_into().unwrap()) as usize)
                        .collect::<Vec<_>>()
                })
                .collect();
            assert!(
                called.contains(&slot),
                "{build}: the selection edits call {called:x?}, not the roster slot {slot:#x}"
            );
        }
    }

    /// The Rust replacements name a jmp write of their own unit.
    #[test]
    fn a_native_entry_is_a_jmp_of_its_unit() {
        for build in VARIANTS.iter().map(|v| v.build).chain([REFERENCE]) {
            let units = [
                tracked(build, "selection-logic"),
                tracked(build, "firing-logic"),
            ];
            let mut names = Vec::new();
            for unit in &units {
                for native in &unit.natives {
                    assert!(
                        unit.writes
                            .iter()
                            .any(|w| w.rva == native.rva && w.kind == Kind::Jmp),
                        "{build} {}: {} has no jmp",
                        unit.name,
                        native.name
                    );
                    names.push(native.name.clone());
                }
            }
            names.sort();
            assert_eq!(names, ["firing_set", "firing_ui", "is_selected", "setter"]);
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
    /// otherwise get the reference units and their stale class offsets.
    #[test]
    fn unknown_builds_are_never_mixed_with_known_modules() {
        let reference = units_of(REFERENCE).unwrap();
        let logic = core_unit(&reference, "logic.dll").unwrap();
        let game = core_unit(&reference, "game.dll").unwrap();
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

    /// The pair table and the reference units' verified lists must agree: a
    /// pair whose hashes are not both listed would never be relocated, and a
    /// listed hash without a pair would be unreachable.
    #[test]
    fn verified_pairs_are_listed_by_both_core_units() {
        let reference = units_of(REFERENCE).unwrap();
        for (unit, _) in &reference {
            let module_shas: Vec<&str> = VERIFIED_PAIRS
                .iter()
                .map(|&(l, g)| if unit.module == "logic.dll" { l } else { g })
                .collect();
            for sha in module_shas {
                assert!(
                    unit.verified.iter().any(|s| s == sha),
                    "{}: {sha} is not verified",
                    unit.name
                );
            }
        }
    }
}
