//! Skip a loop in the shadow cascades' fitting whose results the game discards.
//!
//! For each cascade it draws, the caster pass (`world2.dll`, GOG 2026-09-14
//! `fn_1916b0`, at `0x191f5a`) fits the cascade's projection to its content
//! with `fn_194c50` ([`FIT_PATTERN`]). The function transforms the bounds of
//! the cascade's casters into light space and merges them, then walks the main
//! view's shadow casters (`view+0x330`, every visible caster) doing the same
//! into one scratch box, which it overwrites with the cascade frustum's box
//! straight after the loop without reading it. The transform (`fn_195030`,
//! [`BOX_PATTERN`]) calls nothing and writes only its output box, so the loop
//! does no work that is kept; the bounds getter it calls (vt+0x20) was already
//! called on each of those objects by the main-view pass earlier in the frame.
//! The loop was about 0.65 ms of every frame on a busy CPU-bound scene.
//!
//! Core turns the loop's entry test (`je` past it when the list is empty) into
//! an unconditional jump ([`install`]). `tests::the_fit_is_unchanged` runs the
//! game's function before and after the change on the same inputs and
//! requires the same result, bit for bit. On by default; `[loader] shadow_fit
//! = engine` keeps the loop.
use core::ffi::c_void;
use defiance_api::{Api, LOG_INFO, LOG_WARN};

/// `fn_194c50` from its start through the call after the discarded loop: the
/// casters' merge, the loop over `view+0x330` into `rbp-0x79`, and the
/// frustum box that replaces it.
pub const FIT_PATTERN: &str =
    "48 8b c4 55 41 57 48 8d 68 b1 48 81 ec f8 00 00 00 0f 29 70 c8 0f 57 c0 0f 29 78 b8 4c \
     8b fa 44 0f 29 40 a8 44 0f 29 48 98 44 0f 29 50 88 48 89 58 10 49 8b 19 48 89 70 18 49 \
     8b f0 48 89 78 e8 40 b7 01 0f 11 44 24 24 4c 89 60 e0 4d 8b e1 f3 44 0f 10 44 24 30 f3 \
     0f 10 74 24 2c f3 44 0f 10 4c 24 28 f3 44 0f 10 54 24 24 4c 89 70 d8 33 c0 4d 8b 71 08 \
     48 89 44 24 34 f3 0f 10 7c 24 34 40 88 7c 24 20 48 89 45 8b 89 45 93 48 89 45 97 89 45 \
     9f 49 3b de 0f 84 51 01 00 00 44 0f 29 9c 24 80 00 00 00 f3 44 0f 10 5c 24 38 0f 1f 44 \
     00 00 48 8b 0b 48 8d 55 87 48 8b 01 ff 50 20 4c 8b c6 48 8d 4d a7 48 8b d0 e8 ?? ?? ?? \
     ?? 4c 8b c0 80 38 00 0f 85 fe 00 00 00 48 8d 48 04 48 8d 50 10 40 84 ff 74 4e f3 44 0f \
     10 11 40 32 ff f3 44 0f 10 49 04 f3 0f 10 71 08 f3 44 0f 10 02 f3 0f 10 7a 04 f3 44 0f \
     10 5a 08 f3 44 0f 11 54 24 24 f3 44 0f 11 4c 24 28 f3 0f 11 74 24 2c f3 44 0f 11 44 24 \
     30 f3 0f 11 7c 24 34 40 88 7c 24 20 e9 9c 00 00 00 44 0f 2f 11 48 8d 44 24 24 48 0f 47 \
     c1 48 8d 4c 24 28 45 0f 2f 48 08 f3 44 0f 10 10 49 8d 40 08 f3 44 0f 11 54 24 24 48 0f \
     47 c8 49 8d 40 0c 0f 2f 30 f3 44 0f 10 09 48 8d 4c 24 2c f3 44 0f 11 4c 24 28 48 0f 47 \
     c8 48 8d 44 24 30 44 0f 2f 02 f3 0f 10 31 48 8d 4c 24 34 f3 0f 11 74 24 2c 48 0f 42 c2 \
     41 0f 2f 78 14 f3 44 0f 10 00 49 8d 40 14 f3 44 0f 11 44 24 30 48 0f 42 c8 49 8d 40 18 \
     44 0f 2f 18 f3 0f 10 39 48 8d 4c 24 38 f3 0f 11 7c 24 34 48 0f 42 c8 f3 44 0f 10 19 f3 \
     44 0f 11 5c 24 38 48 83 c3 08 49 3b de 0f 85 cd fe ff ff 44 0f 28 9c 24 80 00 00 00 48 \
     8b 45 77 4c 8b b4 24 e0 00 00 00 48 8b 78 08 48 8b 18 48 3b df 74 31 0f 1f 40 00 0f 1f \
     84 00 00 00 00 00 48 8b 0b 48 8d 55 a7 48 8b 01 ff 50 20 4c 8b c6 48 8d 4d 87 48 8b d0 \
     e8 ?? ?? ?? ?? 48 83 c3 08 48 3b df 75 db 48 8b 45 7f 48 8d 55 87 4c 8b c6 48 8b 08 0f \
     10 81 84 00 00 00 0f b6 81 80 00 00 00 f3 0f 10 89 98 00 00 00 0f 11 45 8b 88 45 87 f3 \
     0f 10 81 94 00 00 00 48 8d 4c 24 20 f3 0f 11 45 9b f3 0f 11 4d 9f e8 ?? ?? ?? ??";
/// All of `fn_195030`: an axis-aligned box through a matrix, into `out`.
pub const BOX_PATTERN: &str =
    "48 8b c4 48 89 58 08 48 89 68 10 48 89 70 18 57 41 56 41 57 48 81 ec a0 00 00 00 0f 29 \
     70 d8 4c 8d 59 04 0f 29 78 c8 48 8d 59 10 44 0f 29 40 b8 48 8d 71 08 44 0f 29 48 a8 4c \
     8d 71 0c 44 0f 29 50 98 4c 8d 79 14 c6 01 01 48 8d 69 18 44 0f 29 58 88 33 c0 49 89 03 \
     45 33 c9 41 89 43 08 44 0f 29 64 24 30 48 89 03 44 0f 29 6c 24 20 89 43 08 0f b6 39 44 \
     0f 29 74 24 10 f3 44 0f 10 35 ?? ?? ?? ?? 41 8b c1 d1 f8 41 33 c1 a8 01 74 07 f3 0f 10 \
     7a 10 eb 05 f3 0f 10 7a 04 41 f6 c1 02 74 07 f3 0f 10 62 14 eb 05 f3 0f 10 62 08 41 f6 \
     c1 04 74 07 f3 0f 10 72 18 eb 05 f3 0f 10 72 0c 0f 28 d4 0f 28 ec f3 41 0f 59 50 1c 0f \
     28 ce f3 41 0f 59 48 2c 0f 28 c7 f3 41 0f 59 40 0c 41 0f 28 de f3 41 0f 59 68 10 f3 0f \
     58 d0 0f 28 c7 f3 41 0f 59 00 f3 41 0f 58 48 3c f3 0f 58 e8 0f 28 c7 f3 41 0f 59 40 04 \
     f3 41 0f 59 78 08 f3 0f 58 d1 0f 28 ce f3 41 0f 59 48 20 f3 0f 5e da 0f 28 d4 f3 41 0f \
     59 60 18 f3 41 0f 59 50 14 f3 0f 58 e7 f3 41 0f 58 48 30 f3 0f 58 d0 f3 0f 58 e9 0f 28 \
     ce f3 41 0f 59 48 24 f3 41 0f 59 70 28 f3 0f 59 eb f3 41 0f 58 48 34 f3 41 0f 58 70 38 \
     f3 0f 11 2c 24 f3 0f 58 d1 f3 0f 58 e6 f3 0f 59 d3 f3 0f 59 e3 f3 0f 11 54 24 04 f3 0f \
     11 64 24 08 40 84 ff 74 46 f3 0f 11 2b 44 0f 28 dd f3 0f 11 53 04 40 32 ff f3 0f 11 63 \
     08 f3 41 0f 11 2b f3 41 0f 11 53 04 f3 41 0f 11 63 08 c6 01 00 f3 44 0f 10 49 08 f3 44 \
     0f 10 51 0c f3 44 0f 10 61 14 f3 44 0f 10 69 18 e9 8c 00 00 00 41 0f 2f 2b 0f b6 39 48 \
     8d 04 24 49 0f 43 c3 f3 0f 10 00 48 8d 44 24 04 f3 41 0f 11 03 0f 2f 16 48 0f 43 c6 f3 \
     44 0f 10 08 48 8d 44 24 08 f3 44 0f 11 0e 41 0f 2f 26 49 0f 43 c6 f3 44 0f 10 10 48 8d \
     04 24 f3 45 0f 11 16 0f 2f 2b 0f 28 e8 48 0f 46 c3 f3 44 0f 10 18 48 8d 44 24 04 f3 44 \
     0f 11 1b 41 0f 2f 17 49 0f 46 c7 f3 44 0f 10 20 48 8d 44 24 08 f3 45 0f 11 27 0f 2f 65 \
     00 48 0f 46 c5 f3 44 0f 10 28 f3 44 0f 11 6d 00 45 8d 51 01 41 8b c2 d1 f8 41 33 c2 a8 \
     01 74 08 f3 44 0f 10 42 10 eb 06 f3 44 0f 10 42 04 41 f6 c2 02 74 07 f3 0f 10 72 14 eb \
     05 f3 0f 10 72 08 41 f6 c2 04 74 07 f3 0f 10 7a 18 eb 05 f3 0f 10 7a 0c 0f 28 d6 48 8d \
     04 24 f3 41 0f 59 50 1c 0f 28 cf f3 41 0f 59 48 2c 0f 28 e6 f3 41 0f 59 60 10 41 0f 28 \
     c0 f3 41 0f 59 40 0c 41 0f 28 de f3 41 0f 58 48 3c f3 0f 58 d0 41 0f 28 c0 f3 41 0f 59 \
     00 f3 0f 58 d1 0f 28 cf f3 41 0f 59 48 20 f3 0f 58 e0 41 0f 28 c0 f3 45 0f 59 40 08 f3 \
     41 0f 58 48 30 f3 41 0f 59 40 04 f3 0f 5e da 0f 28 d6 f3 41 0f 59 70 18 f3 41 0f 59 50 \
     14 f3 0f 58 e1 0f 28 cf f3 41 0f 59 78 28 f3 41 0f 59 48 24 f3 41 0f 58 f0 f3 0f 59 e3 \
     f3 0f 58 d0 f3 41 0f 58 48 34 f3 41 0f 58 78 38 0f 2f ec f3 0f 11 24 24 f3 0f 58 d1 f3 \
     0f 58 f7 49 0f 46 c3 f3 0f 59 d3 f3 0f 59 f3 44 0f 2f ca f3 0f 11 54 24 04 f3 0f 11 74 \
     24 08 8b 00 41 89 03 48 8d 44 24 04 48 0f 46 c6 44 0f 2f d6 8b 00 89 06 48 8d 44 24 08 \
     49 0f 46 c6 41 0f 2f e3 8b 00 41 89 06 48 8d 04 24 48 0f 46 c3 41 0f 2f d4 8b 00 89 03 \
     48 8d 44 24 04 49 0f 46 c7 41 0f 2f f5 8b 00 41 89 07 48 8d 44 24 08 48 0f 46 c5 41 83 \
     c1 02 8b 00 89 45 00 41 83 f9 08 0f 8c b4 fc ff ff 44 0f 28 74 24 10 4c 8d 9c 24 a0 00 \
     00 00 49 8b 5b 20 48 8b c1 49 8b 6b 28 49 8b 73 30 41 0f 28 73 f0 41 0f 28 7b e0 45 0f \
     28 43 d0 45 0f 28 4b c0 45 0f 28 53 b0 45 0f 28 5b a0 45 0f 28 63 90 45 0f 28 6b 80 49 \
     8b e3 41 5f 41 5e 5f c3";
/// Offsets in [`FIT_PATTERN`] of its three calls to `fn_195030` (casters,
/// the discarded loop, the frustum) and of the loop's `je rel8`.
const BOX_CALLS: [usize; 3] = [0xc7, 0x227, 0x277];
const LOOP_JE_AT: usize = 0x202;
const BEFORE: [u8; 2] = [0x74, 0x31];
const AFTER: [u8; 2] = [0xeb, 0x31];

fn find(api: &Api, base: *mut c_void, size: usize, pattern: &str) -> *mut u8 {
    let pattern = std::ffi::CString::new(pattern).unwrap_or_default();
    unsafe { (api.find_pattern)(base, size, pattern.as_ptr()) as *mut u8 }
}

/// The target of the `e8 rel32` at `site`.
unsafe fn call_target(site: *const u8) -> usize {
    let rel = unsafe { core::ptr::read_unaligned(site.add(1) as *const i32) };
    (site as isize + 5 + rel as isize) as usize
}

/// Whether the fit at `fit` calls the box transform at `boxed` all three times.
unsafe fn vouched(fit: *const u8, boxed: *const u8) -> bool {
    BOX_CALLS
        .iter()
        .all(|&at| unsafe { call_target(fit.add(at)) } == boxed as usize)
}

fn patch(api: &Api) -> Result<(), String> {
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
    let target = unsafe { fit.add(LOOP_JE_AT) } as *mut c_void;
    if unsafe { (api.patch_bytes)(target, BEFORE.as_ptr(), AFTER.as_ptr(), BEFORE.len()) } != 0 {
        return Err("it could not be patched".into());
    }
    Ok(())
}

pub fn install(api: &Api, setting: &str) {
    if setting == "engine" {
        return;
    }
    match patch(api) {
        Ok(()) => super::say(
            api,
            LOG_INFO,
            "shadow fit: skipping the receiver loop whose result is discarded",
        ),
        Err(e) => super::say(
            api,
            LOG_WARN,
            &format!("shadow fit: {e}; the game's own is used"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A small deterministic generator (xorshift64*).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        fn unit(&mut self) -> f32 {
            (self.next() >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
        }
    }

    /// The box layout the bounds getter fills: a flag byte, then min and max.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Bounds {
        empty: u8,
        min: [f32; 3],
        max: [f32; 3],
    }

    /// A stand-in object whose vt+0x20 copies its bounds out, counting calls.
    #[repr(C)]
    struct Object {
        vtable: *const [usize; 5],
        bounds: Bounds,
    }

    static CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn bounds(this: *mut Object, out: *mut Bounds) -> *mut Bounds {
        CALLS.fetch_add(1, Ordering::Relaxed);
        unsafe { *out = (*this).bounds };
        out
    }

    /// The cascade frustum: its box at +0x80, as the fit reads it.
    #[repr(C)]
    struct Frustum {
        pad: [u8; 0x80],
        bounds: Bounds,
    }

    /// `fn_194c50(_, out, matrix, &casters, &receivers, &&frustum)`.
    type Fit = unsafe extern "C" fn(
        *mut c_void,
        *mut [f32; 16],
        *const [f32; 16],
        *const [*const *mut Object; 2],
        *const [*const *mut Object; 2],
        *const *const Frustum,
    ) -> *mut [f32; 16];

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn VirtualProtect(address: *mut c_void, size: usize, protect: u32, old: *mut u32) -> i32;
    }

    /// The game's fit in its `world2.dll`, mapped without running it (the fit
    /// and the box transform call nothing outside it), and the address of the
    /// loop's `je`.
    fn world2() -> Option<(Fit, *mut u8)> {
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
        let at = |rva: usize| unsafe { base.add(rva) };
        assert!(unsafe { vouched(at(fit), at(boxed)) });
        let je = at(fit + LOOP_JE_AT);
        assert_eq!(unsafe { [*je, *je.add(1)] }, BEFORE);
        Some((unsafe { core::mem::transmute::<*mut u8, Fit>(at(fit)) }, je))
    }

    fn set(je: *mut u8, bytes: [u8; 2]) {
        let mut old = 0;
        unsafe {
            VirtualProtect(je as *mut c_void, 2, 0x40, &mut old);
            *je = bytes[0];
            *je.add(1) = bytes[1];
            VirtualProtect(je as *mut c_void, 2, old, &mut old);
        }
    }

    fn objects(rng: &mut Rng, n: usize, vtable: &[usize; 5]) -> Vec<Box<Object>> {
        (0..n)
            .map(|_| {
                let c: [f32; 3] = core::array::from_fn(|_| rng.unit() * 500.0);
                let e: [f32; 3] = core::array::from_fn(|_| rng.unit().abs() * 20.0);
                Box::new(Object {
                    vtable,
                    bounds: Bounds {
                        empty: (rng.next() % 9 == 0) as u8,
                        min: core::array::from_fn(|i| c[i] - e[i]),
                        max: core::array::from_fn(|i| c[i] + e[i]),
                    },
                })
            })
            .collect()
    }

    #[test]
    fn the_fit_is_unchanged() {
        let Some((fit, je)) = world2() else {
            eprintln!(
                "DEFIANCE_GAME_DIR's bin/world2.dll with the shadow fit is not available; skipped"
            );
            return;
        };
        let mut vtable = [0usize; 5];
        vtable[4] = bounds as unsafe extern "C" fn(_, _) -> _ as usize;
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for round in 0..2000 {
            let (nc, nr) = ((rng.next() % 60) as usize, (rng.next() % 200) as usize);
            let casters = objects(&mut rng, nc, &vtable);
            let receivers = objects(&mut rng, nr, &vtable);
            let pointers = |v: &[Box<Object>]| -> Vec<*mut Object> {
                v.iter()
                    .map(|o| &**o as *const Object as *mut Object)
                    .collect()
            };
            let (cp, rp) = (pointers(&casters), pointers(&receivers));
            // A `std::vector<Object*>`: begin and end of the pointer array.
            let vector = |p: &Vec<*mut Object>| -> [*const *mut Object; 2] {
                [p.as_ptr(), unsafe { p.as_ptr().add(p.len()) }]
            };
            let (casters_vec, receivers_vec) = (vector(&cp), vector(&rp));
            let frustum = Box::new(Frustum {
                pad: [0; 0x80],
                bounds: Bounds {
                    empty: 0,
                    min: [-300.0, -300.0, -50.0],
                    max: [300.0, 300.0, 400.0],
                },
            });
            let holder: *const Frustum = &*frustum;
            let mut matrix: [f32; 16] = core::array::from_fn(|_| rng.unit());
            if round % 2 == 0 {
                matrix[3] = 0.0;
                matrix[7] = 0.0;
                matrix[11] = 0.0;
                matrix[15] = 1.0;
            }
            let run = |bytes| {
                set(je, bytes);
                let mut out = [f32::NAN; 16];
                CALLS.store(0, Ordering::Relaxed);
                unsafe {
                    fit(
                        core::ptr::null_mut(),
                        &mut out,
                        &matrix,
                        &casters_vec,
                        &receivers_vec,
                        &holder,
                    )
                };
                (out, CALLS.load(Ordering::Relaxed))
            };
            let (theirs, their_calls) = run(BEFORE);
            let (ours, our_calls) = run(AFTER);
            set(je, BEFORE);
            let bits = |m: &[f32; 16]| m.map(f32::to_bits);
            assert!(
                theirs.iter().all(|v| v.is_finite()),
                "round {round}: {theirs:?}"
            );
            assert_eq!(bits(&ours), bits(&theirs), "round {round}");
            assert_eq!(their_calls, casters.len() + receivers.len());
            assert_eq!(our_calls, casters.len());
        }
    }
}
