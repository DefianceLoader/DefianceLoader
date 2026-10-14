//! Keep each shadow cascade's map still while the camera moves.
//!
//! The caster pass fits each cascade's projection to its content with
//! `fn_194c50` ([`FIT_PATTERN`], GOG 2026-10-07 `0x199140`, called only from
//! the caster pass): it takes the light's matrix, transforms the cascade
//! band's box into light space, clamps it to its casters' box, and returns
//! the orthographic projection of that box (`out[0]`/`out[5]` the scale,
//! `out[12]`/`out[13]` the offset, `out[10]`/`out[14]` the depth). The box
//! follows the camera exactly, so from one frame to the next every texel of
//! the map lands on a slightly different patch of the world and shadow edges
//! crawl; and it is a tight fit, so a cascade that the rotation
//! (`shadow_cascades`, [`crate::shadow`]) leaves for a frame or two no
//! longer covers its band once the camera has moved.
//!
//! `[defiance.performance] shadow_stable = stable` (experimental) hooks the
//! fit and, on its result ([`stabilize`]): widens the box by [`PAD`] of its
//! size on every side; gives it its cascade's held size ([`Cascades`]); and
//! moves its corner
//! onto a grid of `size/256` fixed to the world's origin in light space
//! (`light[12]`, `light[13]`). A grid of `size/256` is a whole number of
//! texels for any power-of-two map of 256 or more, so the map's texels stay
//! on the same world positions while the size holds.
//!
//! Panning only moves the band's light-space box, but turning the camera
//! changes its extent on every frame, and so does the casters' box it is
//! clamped to. A size that followed the extent, even in steps, rescales the
//! whole cascade each time it steps, which shows as a flicker while the
//! camera turns. So each cascade, told apart by its band's world box
//! ([`band_key`]), holds its own size per axis ([`Held`]): a cascade that
//! needs more takes [`ROOM`] times what it needs at once, rounded up to a
//! step of `2^(1/8)` ([`STEPS_PER_OCTAVE`]), and shrinks only after needing
//! less than [`KEEP`] of it for [`SETTLE`] fits in a row. A turn swings the
//! extent by at most `√2`, so a turning camera grows a size once and never
//! shrinks it. Depth (`out[10]`, `out[14]`) holds its scale the same way,
//! centred on the fitted range: the game fits it to the casters' near side
//! and the band's far side, both of which move as the camera turns. The
//! price is resolution: a cascade spans up to about two and a half times
//! its fitted size on each axis, which shows in game as softer
//! shadows ending nearer the camera. `engine` (the default) keeps the game's
//! fit.
use super::shadow_fit::{find, vouched, BOX_PATTERN, FIT_PATTERN};
use crate::plan::{self, Kind, Plan, Write};
use core::cell::RefCell;
use core::ffi::c_void;
use defiance_api::{Api, LOG_INFO, LOG_WARN};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The margin added on each side, as a fraction of the fitted width.
pub const PAD: f64 = 0.1;
/// Grid cells across a cascade: the snap is `size / TEXELS`.
pub const TEXELS: f64 = 256.0;
/// Steps of size per doubling.
pub const STEPS_PER_OCTAVE: f64 = 8.0;
/// A new size is this many times what the cascade needs, before the step.
pub const ROOM: f64 = 1.25;
/// A held size shrinks once the cascade has needed less than this fraction
/// of it for [`SETTLE`] fits in a row.
pub const KEEP: f64 = 0.5;
/// Fits in a row below [`KEEP`] before a held size shrinks: about two
/// seconds of frames, longer than a turn of the camera keeps a band narrow.
pub const SETTLE: u32 = 120;
/// A band whose box's diagonal is within this ratio of a known cascade's is
/// that cascade.
pub const MATCH: f64 = 1.25;
/// A cascade not fitted in this many fits is forgotten.
const FORGET: u64 = 4096;
/// The most cascades known at once.
const KNOWN: usize = 16;

/// `need` rounded up to a step.
pub fn step(need: f64) -> f64 {
    ((need.log2() * STEPS_PER_OCTAVE).ceil() / STEPS_PER_OCTAVE).exp2()
}

/// One axis's held size: it grows at once when the cascade needs more, and
/// shrinks only when it has needed much less for a while.
#[derive(Clone, Copy, Default)]
pub struct Held {
    size: f64,
    below: u32,
}

impl Held {
    /// The size for a cascade that needs `need`.
    pub fn size(&mut self, need: f64) -> f64 {
        if need > self.size {
            *self = Self {
                size: step(need * ROOM),
                below: 0,
            };
        } else if need < self.size * KEEP {
            self.below += 1;
            if self.below >= SETTLE {
                *self = Self {
                    size: step(need * ROOM),
                    below: 0,
                };
            }
        } else {
            self.below = 0;
        }
        self.size
    }
}

/// One cascade's held sizes, and the diagonal of its band's world box that
/// tells it apart.
#[derive(Clone, Copy, Default)]
pub struct Cascade {
    key: f64,
    used: u64,
    pub x: Held,
    pub y: Held,
    pub z: Held,
}

/// The cascades seen so far. The fit is not told which cascade it fits, but
/// each cascade's band is a slice of the camera's frustum whose size changes
/// little from one frame to the next and differs between cascades, so the
/// band's diagonal finds it.
pub struct Cascades {
    known: Vec<Cascade>,
    tick: u64,
}

impl Cascades {
    pub const fn new() -> Self {
        Self {
            known: Vec::new(),
            tick: 0,
        }
    }

    /// The cascade whose band's diagonal is `key`, new if none is near it.
    pub fn find(&mut self, key: f64) -> &mut Cascade {
        self.tick += 1;
        let tick = self.tick;
        let off = |c: &Cascade| (key / c.key).ln().abs();
        let near = (0..self.known.len())
            .filter(|&i| off(&self.known[i]) <= MATCH.ln())
            .min_by(|&a, &b| off(&self.known[a]).total_cmp(&off(&self.known[b])));
        let at = match near {
            Some(i) => i,
            None => {
                self.known.retain(|c| tick - c.used < FORGET);
                if self.known.len() >= KNOWN {
                    if let Some(oldest) = (0..self.known.len()).min_by_key(|&i| self.known[i].used)
                    {
                        self.known.swap_remove(oldest);
                    }
                }
                self.known.push(Cascade::default());
                self.known.len() - 1
            }
        };
        let cascade = &mut self.known[at];
        cascade.key = key;
        cascade.used = tick;
        cascade
    }
}

impl Default for Cascades {
    fn default() -> Self {
        Self::new()
    }
}

/// The diagonal of the band's world box the fit reads (`+0x80` empty flag,
/// `+0x84` min, `+0x90` max of the frustum `*frustum`), or `None` for an
/// empty or broken box.
///
/// # Safety
/// `frustum` is the fit's last argument, as the caster pass passes it.
pub unsafe fn band_key(frustum: *const c_void) -> Option<f64> {
    if frustum.is_null() {
        return None;
    }
    let at = unsafe { *(frustum as *const *const u8) };
    if at.is_null() || unsafe { *at.add(0x80) } != 0 {
        return None;
    }
    let min = unsafe { *(at.add(0x84) as *const [f32; 3]) };
    let max = unsafe { *(at.add(0x90) as *const [f32; 3]) };
    let key = (0..3)
        .map(|i| (max[i] as f64 - min[i] as f64).powi(2))
        .sum::<f64>()
        .sqrt();
    (key.is_finite() && key > 0.0).then_some(key)
}

/// The box an axis's scale and offset (`2/(max-min)` and
/// `-(min+max)/(max-min)`) project onto `-1..1`, widened by [`PAD`] on each
/// side; `None` when it is not a finite, non-empty box.
pub fn padded(scale: f32, offset: f32) -> Option<(f64, f64)> {
    let (scale, offset) = (scale as f64, offset as f64);
    if !(scale.is_finite() && offset.is_finite()) || scale <= 0.0 {
        return None;
    }
    let (min, max) = ((-1.0 - offset) / scale, (1.0 - offset) / scale);
    let width = max - min;
    if !(width > 0.0 && width.is_finite()) {
        return None;
    }
    Some((min - PAD * width, width * (1.0 + 2.0 * PAD)))
}

/// The size an axis of padded `width` needs: centring a box of that size
/// and snapping its corner down moves it by less than a cell, which this
/// makes room for (`size/2 - size/TEXELS >= width/2`).
pub fn need(width: f64) -> f64 {
    width * TEXELS / (TEXELS - 2.0)
}

/// The scale and offset of a box of `size` centred on `min..min+width`,
/// its corner snapped down to the grid about `origin`.
pub fn place(min: f64, width: f64, size: f64, origin: f32) -> Option<(f32, f32)> {
    let origin = origin as f64;
    let grid = size / TEXELS;
    let lo = min + (width - size) / 2.0;
    let min = origin + ((lo - origin) / grid).floor() * grid;
    let max = min + size;
    let (scale, offset) = ((2.0 / size) as f32, (-(min + max) / size) as f32);
    (origin.is_finite() && scale.is_finite() && offset.is_finite() && scale > 0.0)
        .then_some((scale, offset))
}

/// The fit `m` for the light matrix `light`, made stable in x and y and its
/// depth scale held, with `cascade`'s sizes; an axis that is not a box is
/// left as fitted (x and y together).
pub fn stabilize(m: &mut [f32; 16], light: &[f32; 16], cascade: &mut Cascade) {
    if let (Some((x, w)), Some((y, h))) = (padded(m[0], m[12]), padded(m[5], m[13])) {
        let (sx, sy) = (cascade.x.size(need(w)), cascade.y.size(need(h)));
        if let (Some((sx, ox)), Some((sy, oy))) =
            (place(x, w, sx, light[12]), place(y, h, sy, light[13]))
        {
            m[0] = sx;
            m[12] = ox;
            m[5] = sy;
            m[13] = oy;
        }
    }
    // Depth maps `near..near+1/m[10]` to `0..1`.
    let (scale, offset) = (m[10] as f64, m[14] as f64);
    if scale.is_finite() && offset.is_finite() && scale > 0.0 {
        let (near, depth) = (-offset / scale, 1.0 / scale);
        let width = depth * (1.0 + 2.0 * PAD);
        if width.is_finite() && width > 0.0 {
            let size = cascade.z.size(width);
            let near = near + (depth - size) / 2.0;
            let (scale, offset) = ((1.0 / size) as f32, (-near / size) as f32);
            if scale.is_finite() && offset.is_finite() && scale > 0.0 {
                m[10] = scale;
                m[14] = offset;
            }
        }
    }
}

thread_local! {
    static CASCADES: RefCell<Cascades> = const { RefCell::new(Cascades::new()) };
}

/// `fn_194c50(_, out, light, &casters, &receivers, &&frustum)`.
type Fit = unsafe extern "C" fn(
    *mut c_void,
    *mut [f32; 16],
    *const [f32; 16],
    *const c_void,
    *const c_void,
    *const c_void,
) -> *mut [f32; 16];
static ORIGINAL_FIT: AtomicUsize = AtomicUsize::new(0);

/// The detour for the fit: the game's, then [`stabilize`].
unsafe extern "C" fn stable_fit(
    this: *mut c_void,
    out: *mut [f32; 16],
    light: *const [f32; 16],
    casters: *const c_void,
    receivers: *const c_void,
    frustum: *const c_void,
) -> *mut [f32; 16] {
    let original: Fit = unsafe { core::mem::transmute(ORIGINAL_FIT.load(Ordering::Acquire)) };
    let result = unsafe { original(this, out, light, casters, receivers, frustum) };
    // A band that cannot be told apart keeps the game's fit.
    if let (false, false, Some(key)) =
        (out.is_null(), light.is_null(), unsafe { band_key(frustum) })
    {
        CASCADES.with(|c| {
            if let Ok(mut c) = c.try_borrow_mut() {
                unsafe { stabilize(&mut *out, &*light, c.find(key)) };
            }
        });
    }
    result
}

/// The fit's hook, or nothing when `setting` keeps the game's fit.
pub fn plan(api: &Api, setting: &str) -> Result<Plan, String> {
    if setting == "engine" {
        return Ok(Plan::default());
    }
    let base = unsafe { (api.module_base)(c"world2.dll".as_ptr()) };
    if base.is_null() {
        return Err("world2.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base) };
    let fit = find(api, base, size, FIT_PATTERN);
    let boxed = find(api, base, size, BOX_PATTERN);
    if fit.is_null() || boxed.is_null() {
        return Err("the cascade fitting was not found".into());
    }
    if !unsafe { vouched(fit, boxed) } {
        return Err("the cascade fitting is laid out differently".into());
    }
    Ok(Plan {
        writes: vec![Write {
            target: fit as usize,
            kind: Kind::Entry(None),
            detour: stable_fit as unsafe extern "C" fn(_, _, _, _, _, _) -> _ as *mut c_void,
            original: Some(&ORIGINAL_FIT),
        }],
    })
}

pub fn install(api: &Api, setting: &str) {
    if setting == "engine" {
        return;
    }
    let result = plan(api, setting)
        .and_then(|plan| plan::apply(api, &plan).map_err(|_| "it could not be hooked".to_string()));
    match result {
        Ok(()) => super::say(
            api,
            LOG_INFO,
            "shadow map: cascades padded, held in size and snapped to whole texels",
        ),
        Err(e) => super::say(
            api,
            LOG_WARN,
            &format!("shadow map: {e}; the cascades follow the camera as the game fits them"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small deterministic generator (xorshift64*).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        }
    }

    /// The fit's scale and offset for the box `min..max`, as the game computes them.
    fn fitted(min: f32, max: f32) -> (f32, f32) {
        (2.0 / (max - min), -(min + max) / (max - min))
    }

    /// The box a scale and offset project onto `-1..1`.
    fn bounds((scale, offset): (f32, f32)) -> (f64, f64) {
        let (s, o) = (scale as f64, offset as f64);
        ((-1.0 - o) / s, (1.0 - o) / s)
    }

    /// One axis through [`padded`], [`Held`] and [`place`], as [`stabilize`] runs it.
    fn stable_axis(held: &mut Held, scale: f32, offset: f32, origin: f32) -> Option<(f32, f32)> {
        let (min, width) = padded(scale, offset)?;
        place(min, width, held.size(need(width)), origin)
    }

    #[test]
    fn the_box_holds_the_padded_fit_and_sits_on_the_grid() {
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        for _ in 0..20000 {
            let width = (rng.unit() * 8.0).exp2() * 50.0;
            // Within a few hundred maps of light space's zero, as a cascade is:
            // further out the f32 offset cannot hold the corner to a cell.
            let origin = rng.unit() * 300.0 * width;
            let min = rng.unit() * 300.0 * width;
            let (lo, hi) = (min as f32, (min + width) as f32);
            let (s, o) = fitted(lo, hi);
            let (min, max) = bounds((s, o));
            let stable = stable_axis(&mut Held::default(), s, o, origin as f32).unwrap();
            let (smin, smax) = bounds(stable);
            let size = smax - smin;
            let pad = PAD * (max - min);
            let slack = size * 1e-5;
            assert!(smin <= min - pad + slack, "{smin} {min} {pad}");
            assert!(smax >= max + pad - slack, "{smax} {max} {pad}");
            // A new size: room and one step beyond the padded width and its cells.
            assert!(size <= need(max - min + 2.0 * pad) * ROOM * 1.0906 + slack);
            let steps = size.log2() * STEPS_PER_OCTAVE;
            assert!((steps - steps.round()).abs() < 1e-4, "{size}");
            // The offset is the one for a corner on the origin, less a whole
            // number of cells (2/TEXELS each): reading the corner back through
            // the f32 scale would blur it by the scale's rounding times the
            // distance, the same on every frame, so the offset is what to check.
            let size = (steps.round() / STEPS_PER_OCTAVE).exp2();
            let on_origin = -2.0 * (origin as f32 as f64) / size - 1.0;
            let cells = (on_origin - stable.1 as f64) * TEXELS / 2.0;
            assert!((cells - cells.round()).abs() < 1e-2, "{cells}");
        }
    }

    #[test]
    fn a_small_move_shifts_by_whole_cells() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..5000 {
            let width = 100.0 + rng.unit().abs() * 400.0;
            let min = rng.unit() * 1000.0;
            let mut sizes = Held::default();
            let mut at = |min: f64| {
                let (s, o) = fitted(min as f32, (min + width) as f32);
                stable_axis(&mut sizes, s, o, 0.0).unwrap()
            };
            let (a, b) = (at(min), at(min + rng.unit() * width * 0.05));
            // The same size, and corners a whole number of cells apart.
            assert_eq!(a.0, b.0);
            let (amin, _) = bounds(a);
            let (bmin, bmax) = bounds(b);
            let cell = (bmax - bmin) / TEXELS;
            let cells = (bmin - amin) / cell;
            assert!((cells - cells.round()).abs() < 1e-2, "{cells}");
        }
    }

    /// Turning the camera swings each cascade's extent between its narrow
    /// and its wide side; each cascade settles on one size and keeps it.
    #[test]
    fn a_turning_camera_keeps_each_cascades_size() {
        let mut cascades = Cascades::new();
        // Four cascades, each twice the last, whose extent swings by up to √2;
        // the band's world box swings with it.
        let base = [40.0, 80.0, 160.0, 320.0];
        let mut last = [0.0f64; 4];
        let mut changes = [0; 4];
        for frame in 0..2000 {
            let turn = (frame as f64 * 0.013).sin().abs();
            for (i, &b) in base.iter().enumerate() {
                let swing = 1.0 + (2f64.sqrt() - 1.0) * turn;
                let width = b * swing;
                let size = cascades.find(b * 3.0 * swing).x.size(need(width));
                assert!(size >= need(width), "{size} {width}");
                if frame > 0 && size != last[i] {
                    changes[i] += 1;
                }
                last[i] = size;
            }
        }
        // Growing out of the first size picked at the narrow side is allowed once.
        assert!(changes.iter().all(|&c| c <= 1), "{changes:?}");
    }

    #[test]
    fn a_held_size_gives_way_when_the_cascade_shrinks_or_grows() {
        let mut held = Held::default();
        let first = held.size(100.0);
        assert_eq!(held.size(100.0 * 0.9), first);
        // A short dip below KEEP holds; a sustained one gives way.
        for _ in 1..SETTLE {
            assert_eq!(held.size(first * KEEP * 0.9), first);
        }
        assert_eq!(held.size(first), first);
        for _ in 1..SETTLE {
            held.size(first * KEEP * 0.9);
        }
        assert!(held.size(first * KEEP * 0.9) < first);
        let small = held.size(first * KEEP * 0.9);
        assert!(held.size(small * 1.01) > small);
    }

    #[test]
    fn depth_holds_its_scale_and_covers_the_fitted_range() {
        let mut cascade = Cascade::default();
        let mut held = None;
        for i in 0..100 {
            // A range from the casters' near side to the band's far side, moving.
            let (near, far) = (-50.0 - i as f32 * 0.3, 400.0 + i as f32 * 0.5);
            let mut m = [0.0f32; 16];
            m[10] = 1.0 / (far - near);
            m[14] = -near * m[10];
            stabilize(&mut m, &[0.0; 16], &mut cascade);
            let depth = |z: f32| z * m[10] + m[14];
            assert!(depth(near) >= 0.0 && depth(far) <= 1.0);
            assert_eq!(*held.get_or_insert(m[10]), m[10]);
        }
    }

    #[test]
    fn a_box_that_is_not_one_is_left_alone() {
        for (s, o) in [
            (f32::INFINITY, 0.0),
            (f32::NAN, 0.0),
            (0.0, 1.0),
            (-1.0, 0.0),
            (1.0, f32::NAN),
        ] {
            assert_eq!(padded(s, o), None);
            let mut m = [0.0; 16];
            m[0] = s;
            m[12] = o;
            m[5] = 1.0;
            m[10] = s;
            m[14] = o;
            let before = m.map(f32::to_bits);
            stabilize(&mut m, &[0.0; 16], &mut Cascade::default());
            assert_eq!(m.map(f32::to_bits), before);
        }
    }

    #[test]
    fn a_band_is_known_by_its_box_and_an_empty_one_is_not() {
        let mut frustum = Frustum {
            pad: [0; 0x80],
            bounds: Bounds {
                empty: 0,
                min: [-1.0, -2.0, -2.0],
                max: [1.0, 2.0, 2.0],
            },
        };
        let key = |f: &Frustum| {
            let holder: *const Frustum = f;
            unsafe { band_key(&holder as *const _ as *const c_void) }
        };
        assert_eq!(key(&frustum), Some(6.0));
        frustum.bounds.empty = 1;
        assert_eq!(key(&frustum), None);
        frustum.bounds.empty = 0;
        frustum.bounds.max = frustum.bounds.min;
        assert_eq!(key(&frustum), None);
        assert_eq!(unsafe { band_key(core::ptr::null()) }, None);
    }

    #[test]
    fn cascades_are_told_apart_and_followed() {
        let mut cascades = Cascades::new();
        // Two cascades twice apart; each drifts a little per frame.
        let mut a = 100.0;
        let mut b = 200.0;
        cascades.find(a).x.size(10.0);
        cascades.find(b).x.size(40.0);
        for _ in 0..500 {
            a *= 1.0003;
            b *= 0.9997;
            assert_eq!(cascades.find(a).x.size(10.0), step(10.0 * ROOM));
            assert_eq!(cascades.find(b).x.size(40.0), step(40.0 * ROOM));
        }
        assert_eq!(cascades.known.len(), 2);
    }

    /// The box layout the bounds getter fills: a flag byte, then min and max.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Bounds {
        empty: u8,
        min: [f32; 3],
        max: [f32; 3],
    }

    /// A stand-in object whose vt+0x20 copies its bounds out.
    #[repr(C)]
    struct Object {
        vtable: *const [usize; 5],
        bounds: Bounds,
    }

    unsafe extern "C" fn object_bounds(this: *mut Object, out: *mut Bounds) -> *mut Bounds {
        unsafe { *out = (*this).bounds };
        out
    }

    /// The cascade frustum: its box at +0x80, as the fit reads it.
    #[repr(C)]
    struct Frustum {
        pad: [u8; 0x80],
        bounds: Bounds,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    }

    /// The game's fit in its `world2.dll`, mapped without running it.
    fn world2() -> Option<Fit> {
        let dir = std::env::var_os("DEFIANCE_GAME_DIR")?;
        let path = std::path::Path::new(&dir).join("bin").join("world2.dll");
        let image = defiance_core::pe::map_file(&path).ok()?;
        let find = |text: &str| -> Option<usize> {
            let pattern = defiance_core::pattern::parse(text).ok()?;
            let hits = defiance_core::scan::scan(&image, &pattern);
            (hits.len() == 1).then(|| hits[0])
        };
        let (fit, boxed) = (find(FIT_PATTERN)?, find(BOX_PATTERN)?);
        let wide: Vec<u16> = path
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // DONT_RESOLVE_DLL_REFERENCES: mapped and relocated, DllMain not run.
        let base = unsafe { LoadLibraryExW(wide.as_ptr(), core::ptr::null_mut(), 1) } as *mut u8;
        assert!(!base.is_null());
        assert!(unsafe { vouched(base.add(fit), base.add(boxed)) });
        Some(unsafe { core::mem::transmute::<*mut u8, Fit>(base.add(fit)) })
    }

    /// The light-space box of `min..max` through the affine `m`.
    fn light_box(m: &[f32; 16], min: [f32; 3], max: [f32; 3]) -> ([f32; 2], [f32; 2]) {
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for corner in 0..8 {
            let p: [f32; 3] =
                core::array::from_fn(|i| if corner >> i & 1 != 0 { max[i] } else { min[i] });
            for axis in 0..2 {
                let v = p[0] * m[axis] + p[1] * m[4 + axis] + p[2] * m[8 + axis] + m[12 + axis];
                lo[axis] = lo[axis].min(v);
                hi[axis] = hi[axis].max(v);
            }
        }
        (lo, hi)
    }

    /// The game's fit projects the frustum's light-space box clamped to its
    /// casters', the box [`stable_axis`] reads back from its result; and the
    /// stable fit still holds that box with its margin.
    #[test]
    fn the_games_fit_is_the_box_read_back() {
        let Some(fit) = world2() else {
            eprintln!(
                "DEFIANCE_GAME_DIR's bin/world2.dll with the shadow fit is not available; skipped"
            );
            return;
        };
        let mut vtable = [0usize; 5];
        vtable[4] = object_bounds as unsafe extern "C" fn(_, _) -> _ as usize;
        let mut rng = Rng(0x0123_4567_89ab_cdef);
        for round in 0..500 {
            // A rotation about z and a translation: an affine light view.
            let (angle, t) = (
                rng.unit() * 3.14,
                [rng.unit() * 900.0, rng.unit() * 900.0, rng.unit() * 900.0],
            );
            let (c, s) = (angle.cos() as f32, angle.sin() as f32);
            let mut light = [0.0f32; 16];
            light[0] = c;
            light[1] = s;
            light[4] = -s;
            light[5] = c;
            light[10] = 1.0;
            light[12] = t[0] as f32;
            light[13] = t[1] as f32;
            light[14] = t[2] as f32;
            light[15] = 1.0;
            let frustum = Box::new(Frustum {
                pad: [0; 0x80],
                bounds: Bounds {
                    empty: 0,
                    min: [-300.0, -200.0, -50.0],
                    max: [250.0, 320.0, 400.0],
                },
            });
            let casters: Vec<Box<Object>> = (0..(round % 5))
                .map(|_| {
                    let c: [f32; 3] = core::array::from_fn(|_| (rng.unit() * 300.0) as f32);
                    let e: [f32; 3] = core::array::from_fn(|_| (rng.unit().abs() * 80.0) as f32);
                    Box::new(Object {
                        vtable: &vtable,
                        bounds: Bounds {
                            empty: 0,
                            min: core::array::from_fn(|i| c[i] - e[i]),
                            max: core::array::from_fn(|i| c[i] + e[i]),
                        },
                    })
                })
                .collect();
            let pointers: Vec<*mut Object> = casters
                .iter()
                .map(|o| &**o as *const Object as *mut Object)
                .collect();
            let none: Vec<*mut Object> = Vec::new();
            let vector = |p: &Vec<*mut Object>| -> [*const *mut Object; 2] {
                [p.as_ptr(), unsafe { p.as_ptr().add(p.len()) }]
            };
            let (casters_vec, receivers_vec) = (vector(&pointers), vector(&none));
            let holder: *const Frustum = &*frustum;
            let mut out = [f32::NAN; 16];
            unsafe {
                fit(
                    core::ptr::null_mut(),
                    &mut out,
                    &light,
                    &casters_vec as *const _ as *const c_void,
                    &receivers_vec as *const _ as *const c_void,
                    &holder as *const _ as *const c_void,
                )
            };
            let (mut lo, mut hi) = light_box(&light, frustum.bounds.min, frustum.bounds.max);
            if !casters.is_empty() {
                let (mut clo, mut chi) = ([f32::MAX; 2], [f32::MIN; 2]);
                for o in &casters {
                    let (a, b) = light_box(&light, o.bounds.min, o.bounds.max);
                    for i in 0..2 {
                        clo[i] = clo[i].min(a[i]);
                        chi[i] = chi[i].max(b[i]);
                    }
                }
                for i in 0..2 {
                    lo[i] = lo[i].max(clo[i]);
                    hi[i] = hi[i].min(chi[i]);
                }
            }
            if (0..2).any(|i| lo[i] >= hi[i]) {
                continue;
            }
            for (axis, (s, o)) in [(0, (out[0], out[12])), (1, (out[5], out[13]))] {
                let (min, max) = bounds((s, o));
                let tolerance = 1e-3 * (hi[axis] - lo[axis]).max(1.0) as f64;
                assert!(
                    (min - lo[axis] as f64).abs() < tolerance,
                    "round {round} axis {axis}: {min} {}",
                    lo[axis]
                );
                assert!(
                    (max - hi[axis] as f64).abs() < tolerance,
                    "round {round} axis {axis}: {max} {}",
                    hi[axis]
                );
            }
            let mut stable = out;
            // The band's diagonal, read through the argument the fit took.
            let key = unsafe { band_key(&holder as *const _ as *const c_void) }.unwrap();
            let (bmin, bmax) = (frustum.bounds.min, frustum.bounds.max);
            let diagonal = (0..3)
                .map(|i| (bmax[i] as f64 - bmin[i] as f64).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!((key - diagonal).abs() <= 1e-9 * diagonal, "round {round}");
            stabilize(&mut stable, &light, Cascades::new().find(key));
            for (axis, (s, o)) in [(0, (stable[0], stable[12])), (1, (stable[5], stable[13]))] {
                let (min, max) = bounds((s, o));
                let pad = PAD * (hi[axis] - lo[axis]) as f64 * 0.99;
                assert!(
                    min <= lo[axis] as f64 - pad && max >= hi[axis] as f64 + pad,
                    "round {round}"
                );
            }
            // The fitted depth range still lands inside 0..1.
            let near = -out[14] / out[10];
            let far = near + 1.0 / out[10];
            for z in [near, far] {
                let d = z * stable[10] + stable[14];
                assert!((-1e-4..=1.0 + 1e-4).contains(&d), "round {round}: {d}");
            }
            for i in [1, 2, 3, 4, 6, 7, 8, 9, 11, 15] {
                assert_eq!(stable[i].to_bits(), out[i].to_bits(), "round {round} [{i}]");
            }
        }
    }
}
