//! Leave casters much smaller than a texel out of the far shadow cascades.
//!
//! For each cascade it draws, the caster pass (`world2.dll`, GOG 2026-10-07
//! `0x196184`, [`LIST_PATTERN`]) walks the casters picked against the
//! cascade's receivers: it sets each one's cascade range
//! ([`RANGE_PATTERN`]), adds it to the set that is drawn, and fits the
//! cascade to their bounds. In the far cascades a texel spans metres, and
//! most of the thousands of casters there (2,594 in the farthest on a busy
//! scene) are props a few texels across or less, whose shadow is a speck.
//!
//! The plugin hooks the walk's first load ([`cull_stub`]) and compacts the
//! caster vector in place before it, dropping each caster of a cascade past
//! the first whose bounds' diagonal is under [`Setting`] texels, the texel
//! being the band box's largest extent over the shadow map's resolution (a
//! lower bound on the light-space texel, so a kept caster is never one the
//! game would draw under a texel). The vector holds raw pointers and is freed
//! by capacity, so a shorter vector is only a shorter walk and a tighter fit.
//! Off by default; `[defiance.performance] shadow_small_casters = tiny`
//! (under 2 texels), `small` (4), `medium` (8) or `large` (16).
use crate::plan::{self, Kind, Plan, Write};
use crate::shadow::{BAND_BOX, BOX_MAX, BOX_MIN, CASTERS_SIZE, LAYOUTS, LOOP_BAND, RANGE_PATTERN};
use core::ffi::c_void;
use defiance_api::{Api, LOG_INFO, LOG_WARN};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// The walk over the cascade's casters: `mov r13, [rsp + 0x68]` (the vector's
/// end), `mov rsi, [rsp + 0x60]` (its begin), `cmp rsi, r13; je` past it. The
/// view is in `rsi` until here, the cascade index in `r15d`, the loop frame in
/// `rbp`.
pub const LIST_PATTERN: &str = "4c 8b 6c 24 68 48 8b 74 24 60 49 3b f5 74 ??";
/// Offset of [`RANGE_PATTERN`] from [`LIST_PATTERN`].
const RANGE_AT: usize = 0x1c;

/// Smallest and largest shadow-map resolution taken as plausible.
const RESOLUTIONS: core::ops::RangeInclusive<i32> = 64..=16384;

/// Which casters are left out, in texels of their bounds' diagonal.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Setting {
    Engine,
    Tiny,
    Small,
    Medium,
    Large,
}

impl Setting {
    pub fn parse(text: &str) -> Self {
        match text {
            "tiny" => Self::Tiny,
            "small" => Self::Small,
            "medium" => Self::Medium,
            "large" => Self::Large,
            _ => Self::Engine,
        }
    }

    fn texels(self) -> f32 {
        match self {
            Self::Engine => 0.0,
            Self::Tiny => 2.0,
            Self::Small => 4.0,
            Self::Medium => 8.0,
            Self::Large => 16.0,
        }
    }
}

/// The view's shadow-map resolution (an int just before
/// [`crate::shadow::Layout::cascade_count`]), set by [`plan`].
static RESOLUTION_AT: AtomicUsize = AtomicUsize::new(0);
/// [`Setting::texels`], as `f32` bits.
static TEXELS: AtomicU32 = AtomicU32::new(0);
static TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);

/// A caster's bounds as its vt+0x20 answers them: an empty flag, then min and
/// max.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Bounds {
    empty: u8,
    min: [f32; 3],
    max: [f32; 3],
    pad: u32,
}

/// Whether `object`'s bounds are non-empty, finite and with a squared
/// diagonal under `limit`.
unsafe fn small(object: *mut c_void, limit: f32) -> bool {
    let mut out = Bounds {
        empty: 1,
        min: [0.0; 3],
        max: [0.0; 3],
        pad: 0,
    };
    let get: unsafe extern "C" fn(*mut c_void, *mut Bounds) -> *const Bounds =
        unsafe { crate::shadow::method(object, 0x20) };
    let bounds = unsafe { get(object, &mut out) };
    if bounds.is_null() {
        return false;
    }
    let bounds = unsafe { bounds.read_unaligned() };
    if bounds.empty != 0 {
        return false;
    }
    let diagonal: f32 = (0..3)
        .map(|k| (bounds.max[k] - bounds.min[k]).powi(2))
        .sum();
    diagonal.is_finite() && diagonal < limit
}

/// The squared diagonal under which cascade `index`'s casters are dropped,
/// or nothing for the first cascade or a band or resolution out of reach.
unsafe fn limit(view: *const u8, index: u32, frame: *const u8) -> Option<f32> {
    let texels = f32::from_bits(TEXELS.load(Ordering::Relaxed));
    let at = RESOLUTION_AT.load(Ordering::Relaxed);
    if index == 0 || texels <= 0.0 || at == 0 || view.is_null() {
        return None;
    }
    let resolution = unsafe { (view.add(at) as *const i32).read_unaligned() };
    let band = unsafe { *(frame.add(LOOP_BAND) as *const *const u8) };
    if !RESOLUTIONS.contains(&resolution) || band.is_null() || unsafe { *band.add(BAND_BOX) } != 0 {
        return None;
    }
    let read3 = |p: *const u8| unsafe { (p as *const [f32; 3]).read_unaligned() };
    let (min, max) = unsafe {
        (
            read3(band.add(BAND_BOX + BOX_MIN)),
            read3(band.add(BAND_BOX + BOX_MAX)),
        )
    };
    let extent = (0..3).map(|k| max[k] - min[k]).fold(0.0f32, f32::max);
    let size = texels * extent / resolution as f32;
    (size.is_finite() && size > 0.0).then_some(size * size)
}

/// Compacts the cascade's caster vector (`[begin, end]`), dropping the small
/// casters, and writes its new end.
unsafe extern "C" fn cull(
    view: *const u8,
    index: u32,
    frame: *const u8,
    list: *mut [*mut *mut c_void; 2],
) {
    let Some(limit) = (unsafe { limit(view, index, frame) }) else {
        return;
    };
    let [begin, end] = unsafe { *list };
    if begin.is_null() || end <= begin {
        return;
    }
    let count = unsafe { end.offset_from(begin) } as usize;
    let mut kept = 0;
    for k in 0..count {
        let object = unsafe { *begin.add(k) };
        if object.is_null() || !unsafe { small(object, limit) } {
            unsafe { *begin.add(kept) = object };
            kept += 1;
        }
    }
    unsafe { (*list)[1] = begin.add(kept) };
    #[cfg(feature = "small-caster-count")]
    count::add(index, kept, count - kept);
}

/// At [`LIST_PATTERN`], before its first load: [`cull`] on the vector at
/// `[rsp + 0x60]`, then the walk, which reads the new end. Volatile registers
/// and flags are dead here.
#[unsafe(naked)]
unsafe extern "C" fn cull_stub() {
    core::arch::naked_asm!(
        "lea r9, [rsp + 0x60]",
        "push rbx",
        "mov rbx, rsp",
        "and rsp, -16",
        "sub rsp, 0x20",
        "mov rcx, rsi",
        "mov edx, r15d",
        "mov r8, rbp",
        "call {cull}",
        "mov rsp, rbx",
        "pop rbx",
        "jmp qword ptr [rip + {original}]",
        cull = sym cull,
        original = sym TRAMPOLINE,
    );
}

/// The layout whose loop head and latch bracket `site` within the caster
/// pass, by its index in [`LAYOUTS`].
fn layout_around(api: &Api, base: *mut c_void, size: usize, site: usize) -> Option<usize> {
    let find = |p| crate::shadow_fit::find(api, base, size, p) as usize;
    super::matching_first(
        base,
        size,
        &LAYOUTS.iter().map(|l| l.head).collect::<Vec<_>>(),
    )
    .into_iter()
    .find(|&k| {
        let (head, latch) = (find(LAYOUTS[k].head), find(LAYOUTS[k].latch));
        head != 0 && head < site && site < latch && latch - head < CASTERS_SIZE
    })
}

/// The hook on the caster walk, or nothing when `setting` keeps every
/// caster.
pub fn plan(api: &Api, setting: &str) -> Result<Plan, String> {
    let setting = Setting::parse(setting);
    if setting == Setting::Engine {
        return Ok(Plan::default());
    }
    let base = unsafe { (api.module_base)(c"world2.dll".as_ptr()) };
    if base.is_null() {
        return Err("world2.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base) };
    let site = crate::shadow_fit::find(api, base, size, LIST_PATTERN) as usize;
    let range = crate::shadow_fit::find(api, base, size, RANGE_PATTERN) as usize;
    if site == 0 || range != site + RANGE_AT {
        return Err("the cascade's caster walk was not found".into());
    }
    let layout =
        layout_around(api, base, size, site).ok_or("the cascade loop is laid out differently")?;
    RESOLUTION_AT.store(LAYOUTS[layout].cascade_count - 4, Ordering::Relaxed);
    TEXELS.store(setting.texels().to_bits(), Ordering::Relaxed);
    Ok(Plan {
        writes: vec![Write {
            target: site,
            kind: Kind::Entry(None),
            detour: cull_stub as unsafe extern "C" fn() as *mut c_void,
            original: Some(&TRAMPOLINE),
        }],
    })
}

pub fn install(api: &Api, setting: &str) {
    if Setting::parse(setting) == Setting::Engine {
        return;
    }
    let result = plan(api, setting).and_then(|plan| {
        plan::apply(api, &plan).map_err(|_| "it could not be patched".to_string())
    });
    match result {
        Ok(()) => {
            #[cfg(feature = "small-caster-count")]
            let _ = count::LOG.set(api.log);
            super::say(
                api,
                LOG_INFO,
                &format!("shadow small casters: leaving out casters under {setting} texels"),
            )
        }
        Err(e) => super::say(
            api,
            LOG_WARN,
            &format!("shadow small casters: {e}; every caster is drawn"),
        ),
    }
}

impl core::fmt::Display for Setting {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{}", self.texels())
    }
}

// diagnostic: per cascade, how many casters the cull kept and dropped, logged
// every few seconds, to weigh its saving against what it leaves out.
#[cfg(feature = "small-caster-count")]
mod count {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    pub static LOG: OnceLock<unsafe extern "C" fn(u32, *const core::ffi::c_char)> = OnceLock::new();
    const PERIOD: Duration = Duration::from_secs(5);

    struct Totals {
        since: Option<Instant>,
        walks: [u64; 8],
        kept: [u64; 8],
        dropped: [u64; 8],
    }

    static TOTALS: Mutex<Totals> = Mutex::new(Totals {
        since: None,
        walks: [0; 8],
        kept: [0; 8],
        dropped: [0; 8],
    });

    pub fn add(index: u32, kept: usize, dropped: usize) {
        let i = (index as usize).min(7);
        let Ok(mut t) = TOTALS.lock() else {
            return;
        };
        t.walks[i] += 1;
        t.kept[i] += kept as u64;
        t.dropped[i] += dropped as u64;
        let now = Instant::now();
        let since = *t.since.get_or_insert(now);
        if now - since < PERIOD {
            return;
        }
        let mut text = String::from("shadow small casters (per walk, kept/dropped):");
        for c in 0..8 {
            if t.walks[c] > 0 {
                text += &format!(
                    " c{c} {}/{}",
                    t.kept[c] / t.walks[c],
                    t.dropped[c] / t.walks[c]
                );
            }
        }
        *t = Totals {
            since: Some(now),
            walks: [0; 8],
            kept: [0; 8],
            dropped: [0; 8],
        };
        if let Some(log) = LOG.get() {
            let text = std::ffi::CString::new(text).unwrap_or_default();
            unsafe { log(defiance_api::LOG_INFO, text.as_ptr()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every world2.dll in `bin/` and the install: the walk once, the range
    /// update right after it, and one layout's loop around it.
    #[test]
    fn the_walk_is_found_in_every_build() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bin");
        let mut paths: Vec<_> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|store| {
                std::fs::read_dir(store.path())
                    .into_iter()
                    .flatten()
                    .flatten()
            })
            .map(|build| build.path().join("world2.dll"))
            .filter(|p| p.is_file())
            .collect();
        if let Some(dir) = std::env::var_os("DEFIANCE_GAME_DIR") {
            paths.push(std::path::Path::new(&dir).join("bin").join("world2.dll"));
        }
        let mut checked = 0;
        for path in paths {
            let Ok(image) = defiance_core::pe::map_file(&path) else {
                continue;
            };
            let find = |text: &str| -> Vec<usize> {
                defiance_core::scan::scan(&image, &defiance_core::pattern::parse(text).unwrap())
            };
            let site = find(LIST_PATTERN);
            // Builds before the band hooks have no walk of this shape.
            if site.is_empty() {
                continue;
            }
            assert_eq!(site.len(), 1, "{path:?}");
            let site = site[0];
            assert_eq!(find(RANGE_PATTERN), [site + RANGE_AT], "{path:?}");
            let bracketed = LAYOUTS.iter().any(|l| {
                let (head, latch) = (find(l.head), find(l.latch));
                head.len() == 1
                    && latch.len() == 1
                    && head[0] < site
                    && site < latch[0]
                    && latch[0] - head[0] < CASTERS_SIZE
            });
            assert!(bracketed, "{path:?}");
            checked += 1;
        }
        if checked == 0 {
            eprintln!("no world2.dll with the caster walk is available; skipped");
        }
    }

    #[test]
    fn settings_parse() {
        assert_eq!(Setting::parse("engine"), Setting::Engine);
        assert_eq!(Setting::parse("tiny").texels(), 2.0);
        assert_eq!(Setting::parse("small").texels(), 4.0);
        assert_eq!(Setting::parse("medium").texels(), 8.0);
        assert_eq!(Setting::parse("large").texels(), 16.0);
        assert_eq!(Setting::parse("other"), Setting::Engine);
    }
}
