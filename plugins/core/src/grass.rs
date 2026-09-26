//! Sort the grass renderer's items with each distance computed once.
//!
//! `GrassRenderer` (`world2.dll`, GOG 2026-09-14 `fn_10ea60`) orders its items
//! by distance from a reference point, far to near, with MSVC's `std::sort`
//! (`fn_1109d0`, called at `0x10ec52`; [`SITE_PATTERN`], [`SORT_PATTERN`]).
//! Its comparison is inlined and recomputes both items' squared distances on
//! every call ([`KEY_PATTERN`], in the insertion sort `fn_110c10`), so a sort
//! of n items computes about 2·n·log n distances.
//!
//! Core redirects that one call ([`sort`]): each item's key is computed once,
//! bit for bit as the game computes it ([`key`]), and the same algorithm
//! ([`msvc_sort`], a port of MSVC's `_Sort_unchecked` with its median guess,
//! fat partition, insertion sort and heap fallback) runs over (key, index)
//! pairs. Every comparison therefore has the same outcome as the game's, so
//! the order, ties included, is identical; `tests::matches_the_games_sort`
//! checks it against the game's own function. On by default; `[loader]
//! grass_sort = engine` keeps the game's sort. In game the game's sort took
//! 1.8–1.96x as long per item on the same scenes.
use core::cell::RefCell;
use core::ffi::c_void;
use defiance_api::{Api, LOG_INFO, LOG_WARN};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The call in `GrassRenderer`'s render: `sort(indices.begin, indices.end,
/// count, &predicate)`, the predicate `{&positions, &reference}` at `rbp-0x19`.
pub const SITE_PATTERN: &str = "49 8b 57 50 49 8b 4f 48 48 89 5d e7 48 8d 45 b7 48 89 45 ef \
     4c 8b c2 4c 2b c1 49 c1 f8 02 4c 8d 4d e7 e8 ?? ?? ?? ??";
/// Offset of the call in [`SITE_PATTERN`].
const SITE_CALL_AT: usize = 34;
/// The start of `_Sort_unchecked` (`fn_1109d0`): 32-element insertion-sort
/// threshold (`0x80` bytes), the `ideal` test, the median-guess partition.
pub const SORT_PATTERN: &str = "48 89 5c 24 10 48 89 6c 24 18 48 89 74 24 20 57 41 56 41 57 \
     48 83 ec 40 48 8b c2 49 8b d9 48 2b c1 49 8b e8 48 83 e0 fc 48 8b f2 48 8b f9 \
     48 3d 80 00 00 00 0f 8e ?? ?? ?? ?? 66 0f 1f 44 00 00 48 85 ed 0f 8e ?? ?? ?? ?? \
     4c 8b cb 48 8d 4c 24 30 4c 8b c6 48 8b d7 e8 ?? ?? ?? ??";
/// Offset in `fn_1109d0` of its call to the insertion sort (`fn_110c10`).
const SORT_INSERTION_CALL_AT: usize = 0xcf;
/// The insertion sort's first comparison: positions are 12-byte float3s, the
/// key `(dx² + dy²) + dz²` from the reference point, the test `key(a) >
/// key(b)` (`comiss`, `jbe` past). It is this many bytes into `fn_110c10`.
pub const KEY_PATTERN: &str = "48 8b 03 4c 8b df 44 8b 37 48 8b 10 8b 06 4f 8d 14 76 \
     f3 42 0f 10 14 92 f3 42 0f 10 6c 92 04 48 8d 0c 40 48 8b 43 08 f3 0f 10 1c 8a \
     f3 0f 10 64 8a 04 f3 42 0f 10 44 92 08 f3 0f 10 30 f3 0f 10 78 04 f3 0f 5c d6 \
     f3 44 0f 10 40 08 f3 0f 5c ef f3 0f 10 4c 8a 08 f3 0f 5c e7 f3 0f 5c de \
     f3 41 0f 5c c0 f3 0f 59 d2 f3 41 0f 5c c8 f3 0f 59 ed f3 0f 59 e4 f3 0f 59 db \
     f3 0f 58 ea f3 0f 59 c0 f3 0f 58 e3 f3 0f 59 c9 f3 0f 58 e8 f3 0f 58 e1 \
     0f 2f ec 76 ??";
const KEY_AT: usize = 0x52;

/// MSVC's `_ISORT_MAX`: ranges this short are insertion-sorted.
const ISORT_MAX: usize = 32;

/// The predicate the game passes: pointers to its positions vector
/// (`std::vector<float3>`: begin, end) and to the reference point.
#[repr(C)]
pub struct Predicate {
    positions: *const [*const [f32; 3]; 2],
    reference: *const [f32; 3],
}

type Sort = unsafe extern "C" fn(*mut u32, *mut u32, isize, *const Predicate);
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// Scratch for the (key, index) pairs, kept between frames.
    static SCRATCH: RefCell<Vec<(f32, u32)>> = const { RefCell::new(Vec::new()) };
}

/// The squared distance exactly as the game's comparison computes it, in
/// single precision and in its order: `(dy² + dx²) + dz²`.
#[inline]
pub fn key(position: &[f32; 3], reference: &[f32; 3]) -> f32 {
    let dx = position[0] - reference[0];
    let dy = position[1] - reference[1];
    let dz = position[2] - reference[2];
    (dy * dy + dx * dx) + dz * dz
}

/// MSVC's `std::sort` (`_Sort_unchecked`) over `v`, with `ideal` the
/// remaining partition budget as the caller passes it and `lt` the predicate.
pub fn msvc_sort<T: Copy>(v: &mut [T], ideal: isize, lt: &impl Fn(&T, &T) -> bool) {
    sort_range(v, 0, v.len(), ideal, lt);
}

fn sort_range<T: Copy>(
    v: &mut [T],
    mut first: usize,
    mut last: usize,
    mut ideal: isize,
    lt: &impl Fn(&T, &T) -> bool,
) {
    loop {
        if last - first <= ISORT_MAX {
            insertion_sort(v, first, last, lt);
            return;
        }
        if ideal <= 0 {
            make_heap(v, first, last, lt);
            sort_heap(v, first, last, lt);
            return;
        }
        let (mid_first, mid_last) = partition_by_median_guess(v, first, last, lt);
        ideal = (ideal >> 1) + (ideal >> 2);
        if mid_first - first < last - mid_last {
            sort_range(v, first, mid_first, ideal, lt);
            first = mid_last;
        } else {
            sort_range(v, mid_last, last, ideal, lt);
            last = mid_first;
        }
    }
}

fn insertion_sort<T: Copy>(v: &mut [T], first: usize, last: usize, lt: &impl Fn(&T, &T) -> bool) {
    if first == last {
        return;
    }
    for mid in first + 1..last {
        let val = v[mid];
        if lt(&val, &v[first]) {
            v.copy_within(first..mid, first + 1);
            v[first] = val;
        } else {
            let mut hole = mid;
            while lt(&val, &v[hole - 1]) {
                v[hole] = v[hole - 1];
                hole -= 1;
            }
            v[hole] = val;
        }
    }
}

fn med3<T: Copy>(v: &mut [T], first: usize, mid: usize, last: usize, lt: &impl Fn(&T, &T) -> bool) {
    if lt(&v[mid], &v[first]) {
        v.swap(mid, first);
    }
    if lt(&v[last], &v[mid]) {
        v.swap(last, mid);
        if lt(&v[mid], &v[first]) {
            v.swap(mid, first);
        }
    }
}

/// `_Guess_median_unchecked`, `last` inclusive.
fn guess_median<T: Copy>(
    v: &mut [T],
    first: usize,
    mid: usize,
    last: usize,
    lt: &impl Fn(&T, &T) -> bool,
) {
    let count = last - first;
    if count > 40 {
        let step = (count + 1) >> 3;
        let two_step = step << 1;
        med3(v, first, first + step, first + two_step, lt);
        med3(v, mid - step, mid, mid + step, lt);
        med3(v, last - two_step, last - step, last, lt);
        med3(v, first + step, mid, last - step, lt);
    } else {
        med3(v, first, mid, last, lt);
    }
}

/// `_Partition_by_median_guess_unchecked`: the range equal to the pivot.
fn partition_by_median_guess<T: Copy>(
    v: &mut [T],
    first: usize,
    last: usize,
    lt: &impl Fn(&T, &T) -> bool,
) -> (usize, usize) {
    let mid = first + ((last - first) >> 1);
    guess_median(v, first, mid, last - 1, lt);
    let mut pfirst = mid;
    let mut plast = pfirst + 1;
    while first < pfirst && !lt(&v[pfirst - 1], &v[pfirst]) && !lt(&v[pfirst], &v[pfirst - 1]) {
        pfirst -= 1;
    }
    while plast < last && !lt(&v[plast], &v[pfirst]) && !lt(&v[pfirst], &v[plast]) {
        plast += 1;
    }
    let mut gfirst = plast;
    let mut glast = pfirst;
    loop {
        while gfirst < last {
            if lt(&v[pfirst], &v[gfirst]) {
            } else if lt(&v[gfirst], &v[pfirst]) {
                break;
            } else if plast != gfirst {
                v.swap(plast, gfirst);
                plast += 1;
            } else {
                plast += 1;
            }
            gfirst += 1;
        }
        while first < glast {
            let prev = glast - 1;
            if lt(&v[prev], &v[pfirst]) {
            } else if lt(&v[pfirst], &v[prev]) {
                break;
            } else {
                pfirst -= 1;
                if pfirst != prev {
                    v.swap(pfirst, prev);
                }
            }
            glast -= 1;
        }
        if glast == first && gfirst == last {
            return (pfirst, plast);
        }
        if glast == first {
            if plast != gfirst {
                v.swap(pfirst, plast);
            }
            plast += 1;
            v.swap(pfirst, gfirst);
            pfirst += 1;
            gfirst += 1;
        } else if gfirst == last {
            glast -= 1;
            pfirst -= 1;
            if glast != pfirst {
                v.swap(glast, pfirst);
            }
            plast -= 1;
            v.swap(pfirst, plast);
        } else {
            glast -= 1;
            v.swap(gfirst, glast);
            gfirst += 1;
        }
    }
}

/// `_Pop_heap_hole_by_index` then `_Push_heap_by_index`, on `v[base..]`.
fn pop_heap_hole<T: Copy>(
    v: &mut [T],
    base: usize,
    mut hole: usize,
    bottom: usize,
    val: T,
    lt: &impl Fn(&T, &T) -> bool,
) {
    let top = hole;
    let mut idx = hole;
    let max_non_leaf = (bottom - 1) >> 1;
    while idx < max_non_leaf {
        idx = 2 * idx + 2;
        if lt(&v[base + idx], &v[base + idx - 1]) {
            idx -= 1;
        }
        v[base + hole] = v[base + idx];
        hole = idx;
    }
    if idx == max_non_leaf && bottom % 2 == 0 {
        v[base + hole] = v[base + bottom - 1];
        hole = bottom - 1;
    }
    while top < hole {
        let parent = (hole - 1) >> 1;
        if !lt(&v[base + parent], &val) {
            break;
        }
        v[base + hole] = v[base + parent];
        hole = parent;
    }
    v[base + hole] = val;
}

fn make_heap<T: Copy>(v: &mut [T], first: usize, last: usize, lt: &impl Fn(&T, &T) -> bool) {
    let bottom = last - first;
    let mut hole = bottom >> 1;
    while hole > 0 {
        hole -= 1;
        let val = v[first + hole];
        pop_heap_hole(v, first, hole, bottom, val, lt);
    }
}

fn sort_heap<T: Copy>(v: &mut [T], first: usize, mut last: usize, lt: &impl Fn(&T, &T) -> bool) {
    while last - first >= 2 {
        last -= 1;
        let val = v[last];
        v[last] = v[first];
        pop_heap_hole(v, first, 0, last - first, val, lt);
    }
}

/// Sort `indices` as the game would, far to near from `reference`. False when
/// an index is outside `positions`, leaving `indices` untouched.
pub fn sort_indices(
    indices: &mut [u32],
    positions: &[[f32; 3]],
    reference: &[f32; 3],
    ideal: isize,
) -> bool {
    if indices.iter().any(|&i| i as usize >= positions.len()) {
        return false;
    }
    let mut fill = |pairs: &mut Vec<(f32, u32)>| {
        pairs.clear();
        pairs.extend(
            indices
                .iter()
                .map(|&i| (key(&positions[i as usize], reference), i)),
        );
        msvc_sort(pairs, ideal, &|a: &(f32, u32), b: &(f32, u32)| a.0 > b.0);
        for (slot, &(_, i)) in indices.iter_mut().zip(pairs.iter()) {
            *slot = i;
        }
    };
    // A nested or re-entrant call uses a scratch of its own.
    SCRATCH.with(|scratch| match scratch.try_borrow_mut() {
        Ok(mut pairs) => fill(&mut pairs),
        Err(_) => fill(&mut Vec::new()),
    });
    true
}

/// Diagnostic, never shipped (feature `grass-compare`), kept privately.
#[cfg(feature = "grass-compare")]
#[path = "grass_compare.rs"]
mod compare;

/// The detour for the grass renderer's sort call.
unsafe extern "C" fn sort(
    first: *mut u32,
    last: *mut u32,
    ideal: isize,
    predicate: *const Predicate,
) {
    let original: Sort = unsafe { core::mem::transmute(ORIGINAL.load(Ordering::Acquire)) };
    let fallback = || unsafe { original(first, last, ideal, predicate) };
    if first.is_null() || last < first || predicate.is_null() {
        return fallback();
    }
    let (vector, reference) = unsafe { ((*predicate).positions, (*predicate).reference) };
    if vector.is_null() || reference.is_null() {
        return fallback();
    }
    let [begin, end] = unsafe { *vector };
    if begin.is_null() || end < begin {
        return fallback();
    }
    let count = unsafe { last.offset_from(first) } as usize;
    let positions = unsafe { core::slice::from_raw_parts(begin, end.offset_from(begin) as usize) };
    let indices = unsafe { core::slice::from_raw_parts_mut(first, count) };
    #[cfg(feature = "grass-compare")]
    {
        let game = compare::games_turn();
        let start = std::time::Instant::now();
        if game || !sort_indices(indices, positions, unsafe { &*reference }, ideal) {
            fallback();
        }
        compare::record(game, count, start.elapsed());
        return;
    }
    #[allow(unreachable_code)]
    if !sort_indices(indices, positions, unsafe { &*reference }, ideal) {
        fallback();
    }
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

fn redirect(api: &Api) -> Result<(), String> {
    let base = unsafe { (api.module_base)(c"world2.dll".as_ptr()) };
    if base.is_null() {
        return Err("world2.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base) };
    let site = find(api, base, size, SITE_PATTERN);
    let sort_fn = find(api, base, size, SORT_PATTERN);
    let keys = find(api, base, size, KEY_PATTERN);
    if site.is_null() || sort_fn.is_null() || keys.is_null() {
        return Err("the grass renderer's sort was not found".into());
    }
    let call = unsafe { site.add(SITE_CALL_AT) };
    let insertion = unsafe { call_target(sort_fn.add(SORT_INSERTION_CALL_AT)) };
    if unsafe { call_target(call) } != sort_fn as usize || insertion + KEY_AT != keys as usize {
        return Err("the grass renderer calls a different sort".into());
    }
    let mut original = core::ptr::null_mut();
    let detour = sort as unsafe extern "C" fn(_, _, _, _) as *mut c_void;
    if unsafe { (api.hook_call)(call as *mut c_void, detour, &mut original) } != 0
        || original.is_null()
    {
        return Err("its call could not be redirected".into());
    }
    ORIGINAL.store(original as usize, Ordering::Release);
    Ok(())
}

pub fn install(api: &Api, setting: &str) {
    #[cfg(feature = "grass-compare")]
    {
        let _ = compare::LOG.set(api.log);
        super::say(api, LOG_INFO, "grass sort: comparison build (diagnostic), alternating the game's sort and the cached one");
    }
    #[cfg(not(feature = "grass-compare"))]
    if setting == "engine" {
        return;
    }
    let _ = setting;
    match redirect(api) {
        Ok(()) => super::say(
            api,
            LOG_INFO,
            "grass sort: distances computed once per item",
        ),
        Err(e) => super::say(
            api,
            LOG_WARN,
            &format!("grass sort: {e}; the game's own is used"),
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
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    #[test]
    fn sorts_far_to_near() {
        let positions = [
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 3.0],
            [0.0, 2.0, 0.0],
            [5.0, 0.0, 0.0],
        ];
        let mut indices = [0u32, 1, 2, 3];
        assert!(sort_indices(&mut indices, &positions, &[0.0; 3], 4));
        assert_eq!(indices, [3, 1, 2, 0]);
    }

    #[test]
    fn refuses_an_index_outside_the_positions() {
        let mut indices = [0u32, 7];
        assert!(!sort_indices(&mut indices, &[[0.0; 3]], &[0.0; 3], 2));
        assert_eq!(indices, [0, 7]);
    }

    #[test]
    fn heap_fallback_and_partition_sort_correctly() {
        let mut rng = Rng(7);
        for (n, ideal) in [(100usize, 0isize), (1000, 1), (1000, 1000), (33, 33)] {
            let mut v: Vec<u32> = (0..n).map(|_| rng.below(50) as u32).collect();
            msvc_sort(&mut v, ideal, &|a: &u32, b: &u32| a > b);
            assert!(v.windows(2).all(|w| w[0] >= w[1]), "n={n} ideal={ideal}");
        }
    }

    /// The game's `world2.dll` mapped without running it, with its one
    /// outside call (`memmove`) bound, for comparing against its own sort.
    struct World2 {
        sort: Sort,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn VirtualProtect(address: *mut c_void, size: usize, protect: u32, old: *mut u32) -> i32;
    }

    /// `fn_110c10+0xf4` (GOG 2026-09-14 `world2+0x110d04`).
    const MEMMOVE_CALL_AT: usize = 0xf4;

    unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
        unsafe { core::ptr::copy(src, dst, n) };
        dst
    }

    fn world2() -> Option<World2> {
        let dir = std::env::var_os("DEFIANCE_GAME_DIR")?;
        let path = std::path::Path::new(&dir).join("bin").join("world2.dll");
        // Locate by rva in the file laid out as mapped, then use the loaded copy.
        let image = defiance_core::pe::map_file(&path).ok()?;
        let find = |text: &str| -> Option<usize> {
            let pattern = defiance_core::pattern::parse(text).ok()?;
            let hits = defiance_core::scan::scan(&image, &pattern);
            (hits.len() == 1).then(|| hits[0])
        };
        let (sort_rva, keys_rva) = (find(SORT_PATTERN)?, find(KEY_PATTERN)?);
        let wide: Vec<u16> = path
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // DONT_RESOLVE_DLL_REFERENCES: mapped and relocated, DllMain not run.
        let base = unsafe { LoadLibraryExW(wide.as_ptr(), core::ptr::null_mut(), 1) } as *mut u8;
        assert!(!base.is_null());
        let sort = unsafe { base.add(sort_rva) };
        let insertion = unsafe { call_target(sort.add(SORT_INSERTION_CALL_AT)) } as *mut u8;
        assert_eq!(insertion as usize + KEY_AT, base as usize + keys_rva);
        // The insertion sort's memmove, at a fixed offset: a call to a
        // `jmp [rip+disp]` import thunk.
        let call = unsafe { insertion.add(MEMMOVE_CALL_AT) };
        assert_eq!(unsafe { *call }, 0xe8);
        let thunk = unsafe { call_target(call) } as *mut u8;
        assert_eq!(
            unsafe { core::ptr::read_unaligned(thunk as *const u16) },
            0x25ff
        );
        let disp = unsafe { core::ptr::read_unaligned(thunk.add(2) as *const i32) };
        let slot = unsafe { thunk.offset(6 + disp as isize) } as *mut usize;
        let mut old = 0;
        unsafe {
            VirtualProtect(slot as *mut c_void, 8, 0x04, &mut old);
            *slot = memmove as unsafe extern "C" fn(_, _, _) -> _ as usize;
        }
        Some(World2 {
            sort: unsafe { core::mem::transmute::<*mut u8, Sort>(sort) },
        })
    }

    #[test]
    fn matches_the_games_sort() {
        let Some(game) = world2() else {
            eprintln!(
                "DEFIANCE_GAME_DIR's bin/world2.dll with the grass sort is not available; skipped"
            );
            return;
        };
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut cases = 0;
        for round in 0..400 {
            let n = match round % 4 {
                0 => rng.below(40) as usize,
                1 => rng.below(300) as usize,
                2 => rng.below(5000) as usize,
                _ => 2000,
            };
            let spread = [0.5f32, 4.0, 100.0][round % 3];
            let grid = round % 5 == 0; // many equal distances
            let count = n + rng.below(50) as usize;
            let positions: Vec<[f32; 3]> = (0..count.max(1))
                .map(|_| {
                    let mut c = || {
                        let r = rng.below(1 << 20) as f32 / (1 << 20) as f32 * spread;
                        if grid {
                            r.floor()
                        } else {
                            r
                        }
                    };
                    [c(), c(), c()]
                })
                .collect();
            let reference = [spread / 2.0, spread / 3.0, spread / 4.0];
            let indices: Vec<u32> = (0..n)
                .map(|_| rng.below(positions.len() as u64) as u32)
                .collect();
            // A small budget forces the heap fallback.
            let ideal = if round % 7 == 0 {
                rng.below(3) as isize
            } else {
                n as isize
            };
            let begin_end = [positions.as_ptr(), unsafe {
                positions.as_ptr().add(positions.len())
            }];
            let predicate = Predicate {
                positions: &begin_end,
                reference: &reference,
            };
            let mut theirs = indices.clone();
            let range = theirs.as_mut_ptr_range();
            unsafe { (game.sort)(range.start, range.end, ideal, &predicate) };
            let mut ours = indices.clone();
            assert!(sort_indices(&mut ours, &positions, &reference, ideal));
            assert_eq!(ours, theirs, "round {round}, n {n}, ideal {ideal}");
            cases += 1;
        }
        assert_eq!(cases, 400);
    }

    /// Time both sorts on the same random scenes: `cargo test -p
    /// defiance-plugin-core --release --lib grass::tests::timing -- --ignored
    /// --nocapture`. Offline only; a frame's real cost needs an in-game run.
    #[test]
    #[ignore]
    fn timing() {
        let Some(game) = world2() else {
            return;
        };
        let mut rng = Rng(42);
        for n in [200usize, 1000, 5000, 20000] {
            let positions: Vec<[f32; 3]> = (0..n)
                .map(|_| {
                    let mut c = || rng.below(1 << 20) as f32 / 1024.0;
                    [c(), c(), c()]
                })
                .collect();
            let reference = [512.0, 30.0, 512.0];
            let indices: Vec<u32> = (0..n as u32).collect();
            let begin_end = [positions.as_ptr(), unsafe { positions.as_ptr().add(n) }];
            let predicate = Predicate {
                positions: &begin_end,
                reference: &reference,
            };
            let rounds = 2_000_000 / n;
            let (mut theirs, mut ours) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
            for _ in 0..rounds {
                let mut v = indices.clone();
                let range = v.as_mut_ptr_range();
                let start = std::time::Instant::now();
                unsafe { (game.sort)(range.start, range.end, n as isize, &predicate) };
                theirs += start.elapsed();
                let mut w = indices.clone();
                let start = std::time::Instant::now();
                sort_indices(&mut w, &positions, &reference, n as isize);
                ours += start.elapsed();
                assert_eq!(v, w);
            }
            eprintln!(
                "n {n:6}: game {:8.1} us, cached {:8.1} us, {:.2}x",
                theirs.as_secs_f64() * 1e6 / rounds as f64,
                ours.as_secs_f64() * 1e6 / rounds as f64,
                theirs.as_secs_f64() / ours.as_secs_f64()
            );
        }
    }
}
