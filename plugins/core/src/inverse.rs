//! Invert the renderer's 4×4 matrices without the game's per-element calls.
//!
//! `world2.dll`'s matrix inverse (GOG 2026-09-14 `fn_daeb0`, [`INVERSE_PATTERN`])
//! inverts a row-major 4×4 float matrix in place by cofactors: each of the
//! sixteen 3×3 minors goes through a nine-pointer call (`fn_9b390`,
//! [`DET3_PATTERN`]), the determinant through a column-0 expansion
//! (`fn_9aa00`, [`DET4_PATTERN`]) that makes four more, and each transposed
//! cofactor is scaled by `1.0 / det`. The scene's nodes invert their world
//! matrix with it whenever they move (`fn_154030`), about 1 ms of a frame on a
//! busy CPU-bound scene.
//!
//! Core replaces the function ([`invert`]) with the same arithmetic in the same
//! order, so every result is bit for bit the game's (a NaN's payload aside);
//! `tests::matches_the_games_inverse` checks that against the game's own
//! function. On by default; `[loader] matrix_inverse = engine` keeps the
//! game's.
use core::ffi::c_void;
use defiance_api::{Api, LOG_INFO, LOG_WARN};

/// All of `fn_daeb0`: the minor loop, the sign select (`1.0`, `-1.0`), the
/// 3×3 and 4×4 determinant calls, and the transposed scaled stores.
pub const INVERSE_PATTERN: &str = "48 8b c4 48 89 58 10 48 89 70 18 48 89 78 20 55 41 56 41 57 \
     48 8d 68 a1 48 81 ec e0 00 00 00 0f 29 70 d8 48 8d 75 e7 f3 0f 10 35 ?? ?? ?? ?? 33 ff \
     0f 29 78 c8 33 db f3 0f 10 3d ?? ?? ?? ?? 4c 8b f9 0f 1f 40 00 66 66 0f 1f 84 00 00 00 00 00 \
     45 33 d2 45 33 db 66 66 0f 1f 84 00 00 00 00 00 33 c9 49 8d 57 08 45 33 c0 0f 1f 80 00 00 00 00 \
     4c 3b c3 74 3e 4d 85 db 74 0a 8b 42 f8 89 44 8d b7 48 ff c1 41 83 fa 01 74 10 8b 42 fc 89 44 8d b7 \
     48 ff c1 41 83 fa 02 74 0f 8b 02 89 44 8d b7 48 ff c1 41 83 fa 03 74 0a 8b 42 04 89 44 8d b7 \
     48 ff c1 49 ff c0 48 83 c2 10 49 83 f8 04 7c b0 41 8d 04 3a a8 01 74 05 0f 28 d7 eb 03 0f 28 d6 \
     48 8d 45 d7 48 89 44 24 48 4c 8d 4d bf 48 8d 45 d3 48 89 44 24 40 4c 8d 45 bb 48 8d 45 cf \
     48 89 44 24 38 48 8d 55 b7 48 8d 45 cb 48 89 44 24 30 48 8d 45 c7 48 89 44 24 28 48 8d 45 c3 \
     48 89 44 24 20 e8 ?? ?? ?? ?? f3 0f 59 c2 41 ff c2 49 ff c3 f3 0f 11 06 48 83 c6 04 49 83 fb 04 \
     0f 8c 2d ff ff ff ff c7 48 ff c3 48 83 fb 04 0f 8c 0e ff ff ff 49 8b cf e8 ?? ?? ?? ?? \
     f3 0f 10 4d e7 4c 8d 9c 24 e0 00 00 00 49 8b 5b 28 49 8b c7 49 8b 73 30 49 8b 7b 38 41 0f 28 7b e0 \
     f3 0f 5e f0 f3 0f 10 45 f7 f3 0f 59 ce f3 0f 59 c6 f3 41 0f 11 0f f3 0f 10 4d 07 f3 41 0f 11 47 04 \
     f3 0f 10 45 17 f3 0f 59 ce f3 0f 59 c6 f3 41 0f 11 4f 08 f3 0f 10 4d eb f3 41 0f 11 47 0c \
     f3 0f 10 45 fb f3 0f 59 ce f3 0f 59 c6 f3 41 0f 11 4f 10 f3 0f 10 4d 0b f3 41 0f 11 47 14 \
     f3 0f 10 45 1b f3 0f 59 ce f3 0f 59 c6 f3 41 0f 11 4f 18 f3 0f 10 4d ef f3 41 0f 11 47 1c \
     f3 0f 10 45 ff f3 0f 59 ce f3 0f 59 c6 f3 41 0f 11 4f 20 f3 0f 10 4d 0f f3 41 0f 11 47 24 \
     f3 0f 10 45 1f f3 0f 59 ce f3 0f 59 c6 f3 41 0f 11 4f 28 f3 0f 10 4d f3 f3 41 0f 11 47 2c \
     f3 0f 10 45 03 f3 0f 59 ce f3 0f 59 c6 f3 41 0f 11 4f 30 f3 0f 10 4d 13 f3 41 0f 11 47 34 \
     f3 0f 10 45 23 f3 0f 59 ce f3 0f 59 c6 41 0f 28 73 f0 f3 41 0f 11 4f 38 f3 41 0f 11 47 3c \
     49 8b e3 41 5f 41 5e 5d c3";
/// Offsets in [`INVERSE_PATTERN`] of the `1.0` and `-1.0` loads' displacements
/// (each instruction ends 4 bytes later), and of its two calls.
const ONE_DISP_AT: usize = 43;
const MINUS_ONE_DISP_AT: usize = 59;
const DET3_CALL_AT: usize = 0x112;
const DET4_CALL_AT: usize = 0x145;

/// All of `fn_9aa00`: `det(m) = ((d0·m00 − d1·m10) + d2·m20) − d3·m30`, `dk` the
/// 3×3 determinant of `m` without row k and column 0.
pub const DET4_PATTERN: &str = "48 8b c4 48 89 58 10 48 89 68 18 48 89 70 20 48 89 48 08 57 \
     41 54 41 55 41 56 41 57 48 83 ec 50 4c 8d 51 3c 4c 89 50 d0 4c 8d 59 38 4c 89 58 c8 48 8d 59 34 \
     48 89 58 c0 48 8d 79 2c 48 89 78 b8 48 8d 71 28 4c 8d 71 1c 48 89 70 b0 4c 8d 79 18 4d 8b ce \
     4c 8d 61 14 4d 8b c7 48 8d 69 24 49 8b d4 4c 8d 69 0c 48 89 68 a8 48 83 c1 08 e8 ?? ?? ?? ?? \
     48 8b 84 24 80 00 00 00 0f 28 d0 4c 89 54 24 48 4d 8b cd 4c 89 5c 24 40 4c 8b c1 48 89 5c 24 38 \
     f3 0f 59 10 48 8d 50 04 48 89 7c 24 30 48 89 74 24 28 48 89 6c 24 20 e8 ?? ?? ?? ?? \
     48 8b 84 24 80 00 00 00 4c 89 54 24 48 4c 89 5c 24 40 48 89 5c 24 38 f3 0f 59 40 10 \
     4c 89 74 24 30 4c 89 7c 24 28 4c 89 64 24 20 f3 0f 5c d0 e8 ?? ?? ?? ?? \
     4c 8b 94 24 80 00 00 00 48 89 7c 24 48 48 89 74 24 40 48 89 6c 24 38 f3 41 0f 59 42 20 \
     4c 89 74 24 30 4c 89 7c 24 28 4c 89 64 24 20 f3 0f 58 d0 e8 ?? ?? ?? ?? f3 41 0f 59 42 30 \
     4c 8d 5c 24 50 49 8b 5b 38 49 8b 6b 40 49 8b 73 48 f3 0f 5c d0 0f 28 c2 49 8b e3 \
     41 5f 41 5e 41 5d 41 5c 5f c3";
/// Offsets of [`DET4_PATTERN`]'s four calls to the 3×3 determinant.
const DET4_DET3_CALLS: [usize; 4] = [0x6d, 0xa9, 0xdd, 0x112];

/// All of `fn_9b390`, the 3×3 determinant of nine floats passed by pointer
/// (the first argument unused), in [`det3`]'s order.
pub const DET3_PATTERN: &str = "48 83 ec 58 48 8b 84 24 88 00 00 00 f3 41 0f 10 28 f3 0f 10 02 \
     0f 29 74 24 40 f3 0f 10 20 48 8b 84 24 a8 00 00 00 0f 29 7c 24 30 0f 28 fd 44 0f 29 44 24 20 \
     44 0f 29 4c 24 10 f3 44 0f 10 08 48 8b 84 24 98 00 00 00 44 0f 29 14 24 f3 0f 59 c4 f3 0f 10 18 \
     48 8b 84 24 90 00 00 00 f3 0f 59 fb f3 41 0f 59 c1 f3 44 0f 10 10 48 8b 84 24 80 00 00 00 \
     f3 41 0f 59 fa f3 0f 59 dc f3 0f 10 30 f3 0f 58 f8 48 8b 84 24 a0 00 00 00 f3 41 0f 59 19 \
     f3 44 0f 10 00 41 0f 28 c8 f3 44 0f 59 02 f3 0f 59 ce f3 45 0f 59 c2 f3 41 0f 59 09 \
     44 0f 28 14 24 f3 0f 59 f5 f3 0f 58 f9 f3 41 0f 59 f1 44 0f 28 4c 24 10 f3 0f 5c fb f3 0f 5c fe \
     0f 28 74 24 40 f3 41 0f 5c f8 44 0f 28 44 24 20 0f 28 c7 0f 28 7c 24 30 48 83 c4 58 c3";

/// `fn_9b390`: the determinant of `[a0 a1 a2; a3 a4 a5; a6 a7 a8]`, each
/// product and sum in the game's order.
#[inline(always)]
fn det3(a: [f32; 9]) -> f32 {
    let p0 = a[0] * a[4] * a[8];
    let p1 = a[1] * a[6] * a[5];
    let p2 = a[7] * a[3] * a[2];
    let p3 = a[6] * a[4] * a[2];
    let p4 = a[3] * a[1] * a[8];
    let p5 = a[7] * a[0] * a[5];
    p1 + p0 + p2 - p3 - p4 - p5
}

/// `fn_daeb0`: `m` inverted in place, as the game does it. A singular matrix
/// gives infinities and NaNs, as the game's does.
///
/// Written out in full: the release profile optimises for size and would
/// keep loops over the minors as loops, slower than the game's. `cij` is the
/// cofactor of row i, column j (the minor's determinant times ±1, as the game
/// multiplies it); `tk` is the determinant's term for row k.
pub fn invert(m: &mut [f32; 16]) {
    let [m00, m01, m02, m03, m10, m11, m12, m13, m20, m21, m22, m23, m30, m31, m32, m33] = *m;
    let c00 = det3([m11, m12, m13, m21, m22, m23, m31, m32, m33]) * 1.0;
    let c01 = det3([m10, m12, m13, m20, m22, m23, m30, m32, m33]) * -1.0;
    let c02 = det3([m10, m11, m13, m20, m21, m23, m30, m31, m33]) * 1.0;
    let c03 = det3([m10, m11, m12, m20, m21, m22, m30, m31, m32]) * -1.0;
    let c10 = det3([m01, m02, m03, m21, m22, m23, m31, m32, m33]) * -1.0;
    let c11 = det3([m00, m02, m03, m20, m22, m23, m30, m32, m33]) * 1.0;
    let c12 = det3([m00, m01, m03, m20, m21, m23, m30, m31, m33]) * -1.0;
    let c13 = det3([m00, m01, m02, m20, m21, m22, m30, m31, m32]) * 1.0;
    let c20 = det3([m01, m02, m03, m11, m12, m13, m31, m32, m33]) * 1.0;
    let c21 = det3([m00, m02, m03, m10, m12, m13, m30, m32, m33]) * -1.0;
    let c22 = det3([m00, m01, m03, m10, m11, m13, m30, m31, m33]) * 1.0;
    let c23 = det3([m00, m01, m02, m10, m11, m12, m30, m31, m32]) * -1.0;
    let c30 = det3([m01, m02, m03, m11, m12, m13, m21, m22, m23]) * -1.0;
    let c31 = det3([m00, m02, m03, m10, m12, m13, m20, m22, m23]) * 1.0;
    let c32 = det3([m00, m01, m03, m10, m11, m13, m20, m21, m23]) * -1.0;
    let c33 = det3([m00, m01, m02, m10, m11, m12, m20, m21, m22]) * 1.0;
    let t0 = det3([m11, m12, m13, m21, m22, m23, m31, m32, m33]) * m00;
    let t1 = det3([m01, m02, m03, m21, m22, m23, m31, m32, m33]) * m10;
    let t2 = det3([m01, m02, m03, m11, m12, m13, m31, m32, m33]) * m20;
    let t3 = det3([m01, m02, m03, m11, m12, m13, m21, m22, m23]) * m30;
    let scale = 1.0 / (t0 - t1 + t2 - t3);
    *m = [
        c00 * scale,
        c10 * scale,
        c20 * scale,
        c30 * scale, //
        c01 * scale,
        c11 * scale,
        c21 * scale,
        c31 * scale, //
        c02 * scale,
        c12 * scale,
        c22 * scale,
        c32 * scale, //
        c03 * scale,
        c13 * scale,
        c23 * scale,
        c33 * scale,
    ];
}

/// The detour for `fn_daeb0`: returns its argument, as the game's does.
unsafe extern "C" fn inverse(m: *mut [f32; 16]) -> *mut [f32; 16] {
    invert(unsafe { &mut *m });
    m
}

fn find(api: &Api, base: *mut c_void, size: usize, pattern: &str) -> *mut u8 {
    let pattern = std::ffi::CString::new(pattern).unwrap_or_default();
    unsafe { (api.find_pattern)(base, size, pattern.as_ptr()) as *mut u8 }
}

/// The target of the `e8 rel32` at `site`.
unsafe fn call_target(site: *const u8) -> usize {
    let rel = unsafe { core::ptr::read_unaligned(site.add(1) as *const i32) };
    (site as isize + 5 + rel as isize) as usize
}

/// The float a rip-relative load reads, from its displacement's address.
unsafe fn rip_float(disp: *const u8) -> f32 {
    let rel = unsafe { core::ptr::read_unaligned(disp as *const i32) };
    unsafe { *((disp as isize + 4 + rel as isize) as *const f32) }
}

/// Whether the inverse at `inverse` calls these determinants and loads `1.0`
/// and `-1.0`, so that [`invert`] computes what it does.
unsafe fn vouched(inverse: *const u8, det4: *const u8, det3: *const u8) -> bool {
    unsafe {
        call_target(inverse.add(DET3_CALL_AT)) == det3 as usize
            && call_target(inverse.add(DET4_CALL_AT)) == det4 as usize
            && DET4_DET3_CALLS
                .iter()
                .all(|&at| call_target(det4.add(at)) == det3 as usize)
            && rip_float(inverse.add(ONE_DISP_AT)) == 1.0
            && rip_float(inverse.add(MINUS_ONE_DISP_AT)) == -1.0
    }
}

fn redirect(api: &Api) -> Result<(), String> {
    let base = unsafe { (api.module_base)(c"world2.dll".as_ptr()) };
    if base.is_null() {
        return Err("world2.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base) };
    let (inverse_fn, det4, det3) = (
        find(api, base, size, INVERSE_PATTERN),
        find(api, base, size, DET4_PATTERN),
        find(api, base, size, DET3_PATTERN),
    );
    if inverse_fn.is_null() || det4.is_null() || det3.is_null() {
        return Err("the matrix inverse was not found".into());
    }
    if !unsafe { vouched(inverse_fn, det4, det3) } {
        return Err("the matrix inverse computes differently".into());
    }
    let mut original = core::ptr::null_mut();
    let detour = inverse as unsafe extern "C" fn(_) -> _ as *mut c_void;
    if unsafe { (api.hook)(inverse_fn as *mut c_void, detour, &mut original) } != 0
        || original.is_null()
    {
        return Err("it could not be replaced".into());
    }
    Ok(())
}

pub fn install(api: &Api, setting: &str) {
    if setting == "engine" {
        return;
    }
    match redirect(api) {
        Ok(()) => super::say(
            api,
            LOG_INFO,
            "matrix inverse: the game's arithmetic without its per-element calls",
        ),
        Err(e) => super::say(
            api,
            LOG_WARN,
            &format!("matrix inverse: {e}; the game's own is used"),
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
        fn unit(&mut self) -> f32 {
            (self.next() >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
        }
    }

    fn product(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
        let mut out = [0.0; 16];
        for r in 0..4 {
            for c in 0..4 {
                out[4 * r + c] = (0..4).map(|k| a[4 * r + k] * b[4 * k + c]).sum();
            }
        }
        out
    }

    #[test]
    fn inverts() {
        // A rotation about y, a scale and a translation, as a node's world matrix.
        let (s, c) = 0.6f32.sin_cos();
        let m = [
            2.0 * c,
            0.0,
            -2.0 * s,
            0.0, //
            0.0,
            2.0,
            0.0,
            0.0, //
            2.0 * s,
            0.0,
            2.0 * c,
            0.0, //
            10.0,
            -3.0,
            7.5,
            1.0,
        ];
        let mut inv = m;
        invert(&mut inv);
        let identity = product(&m, &inv);
        for (i, v) in identity.iter().enumerate() {
            let want = if i % 5 == 0 { 1.0 } else { 0.0 };
            assert!((v - want).abs() < 1e-5, "{i}: {v}");
        }
    }

    #[test]
    fn a_singular_matrix_gives_non_finite_values() {
        let mut m = [1.0f32; 16];
        invert(&mut m);
        assert!(m.iter().all(|v| !v.is_finite()));
    }

    // The game's `world2.dll` is mapped without running it: the inverse and
    // its determinants call nothing outside it.
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    }

    type Inverse = unsafe extern "C" fn(*mut [f32; 16]) -> *mut [f32; 16];

    fn world2() -> Option<Inverse> {
        let dir = std::env::var_os("DEFIANCE_GAME_DIR")?;
        let path = std::path::Path::new(&dir).join("bin").join("world2.dll");
        // Locate by rva in the file laid out as mapped, then use the loaded copy.
        let image = defiance_core::pe::map_file(&path).ok()?;
        let find = |text: &str| -> Option<usize> {
            let pattern = defiance_core::pattern::parse(text).ok()?;
            let hits = defiance_core::scan::scan(&image, &pattern);
            (hits.len() == 1).then(|| hits[0])
        };
        let (inverse_rva, det4_rva, det3_rva) = (
            find(INVERSE_PATTERN)?,
            find(DET4_PATTERN)?,
            find(DET3_PATTERN)?,
        );
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
        assert!(unsafe { vouched(at(inverse_rva), at(det4_rva), at(det3_rva)) });
        Some(unsafe { core::mem::transmute::<*mut u8, Inverse>(at(inverse_rva)) })
    }

    /// Equal bits, or both NaN (the payload can differ).
    fn same(a: &[f32; 16], b: &[f32; 16]) -> bool {
        a.iter()
            .zip(b)
            .all(|(x, y)| x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()))
    }

    #[test]
    fn matches_the_games_inverse() {
        let Some(game) = world2() else {
            eprintln!("DEFIANCE_GAME_DIR's bin/world2.dll with the matrix inverse is not available; skipped");
            return;
        };
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let specials = [
            0.0f32,
            -0.0,
            1.0,
            -1.0,
            f32::MIN_POSITIVE / 4.0,
            1e30,
            f32::INFINITY,
            f32::NAN,
        ];
        for round in 0..200_000 {
            let scale = [1.0f32, 1e-3, 1e3, 1e-20, 1e20][round % 5];
            let mut m: [f32; 16] = core::array::from_fn(|_| rng.unit() * scale);
            match round % 7 {
                // Affine, as world matrices are.
                0 => {
                    m[3] = 0.0;
                    m[7] = 0.0;
                    m[11] = 0.0;
                    m[15] = 1.0;
                }
                // Singular: two equal rows.
                1 => m.copy_within(0..4, 8),
                // Special values in a few places.
                2 => {
                    for _ in 0..3 {
                        m[(rng.next() % 16) as usize] = specials[(rng.next() % 8) as usize];
                    }
                }
                // Small integers, many equal products.
                3 => m.iter_mut().for_each(|v| *v = (*v * 4.0).round()),
                _ => {}
            }
            let mut theirs = m;
            let returned = unsafe { game(&mut theirs) };
            assert_eq!(returned, &mut theirs as *mut _);
            let mut ours = m;
            invert(&mut ours);
            assert!(
                same(&ours, &theirs),
                "round {round}: {m:?}\nours {ours:?}\ntheirs {theirs:?}"
            );
        }
    }

    /// Time both on the same matrices: `cargo test -p defiance-plugin-core
    /// --release --lib inverse::tests::timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing() {
        let Some(game) = world2() else {
            return;
        };
        let mut rng = Rng(42);
        let matrices: Vec<[f32; 16]> = (0..4096)
            .map(|_| core::array::from_fn(|_| rng.unit() * 10.0))
            .collect();
        let rounds = 200;
        let mut work = matrices.clone();
        let start = std::time::Instant::now();
        for _ in 0..rounds {
            work.copy_from_slice(&matrices);
            for m in work.iter_mut() {
                unsafe { game(m) };
            }
        }
        let theirs = start.elapsed();
        let start = std::time::Instant::now();
        for _ in 0..rounds {
            work.copy_from_slice(&matrices);
            for m in work.iter_mut() {
                invert(core::hint::black_box(m));
            }
        }
        let ours = start.elapsed();
        let per = |d: std::time::Duration| d.as_secs_f64() * 1e9 / (rounds * matrices.len()) as f64;
        eprintln!(
            "game {:.1} ns, ours {:.1} ns per inverse, {:.2}x",
            per(theirs),
            per(ours),
            theirs.as_secs_f64() / ours.as_secs_f64()
        );
    }
}
