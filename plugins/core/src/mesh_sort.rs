//! Sort the main view's mesh list with each mesh's key read once.
//!
//! Before it batches meshes for instancing, the main-view pass (`world2.dll`,
//! GOG 2026-09-14 `fn_18f940`) orders its mesh list (`+0x140`) with MSVC's
//! `std::sort` (`fn_1983b0`, called at `0x1902f5`; [`SITE_PATTERN`],
//! [`SORT_PATTERN`]). Its comparison ([`COMPARE_PATTERN`], inlined in the
//! sort) orders by batch key (vt+0x98, signed 64-bit), then by the byte
//! vt+0x100 returns, unsigned: up to four virtual calls per comparison, about
//! 0.5 ms of a 16 ms frame on a busy CPU-bound scene.
//!
//! Core redirects that one call ([`sort`]): each mesh's key is read once and
//! the same algorithm ([`super::grass::msvc_sort`], a port of MSVC's
//! `_Sort_unchecked`) runs over (key, mesh) triples. Every comparison has the
//! game's outcome, so the order, ties included, is the game's;
//! `tests::matches_the_games_sort` checks it against the game's own function.
//! On by default; `[loader] mesh_sort = engine` keeps the game's sort.
use core::cell::RefCell;
use core::ffi::c_void;
use defiance_api::{Api, LOG_INFO, LOG_WARN};
use std::sync::atomic::{AtomicUsize, Ordering};

/// `cmp byte [rsi+0x451], 0` (batching on), the mesh list's size above one,
/// then `fn_1983b0(begin, end, count, predicate)`.
pub const SITE_PATTERN: &str = "80 be 51 04 00 00 00 0f 84 ?? ?? ?? ?? 48 8b 96 48 01 00 00 \
     48 8b 8e 40 01 00 00 4c 8b c2 4c 2b c1 49 c1 f8 03 49 83 f8 01 76 ?? \
     44 0f b6 8d 50 11 00 00 e8 ?? ?? ?? ??";
/// Offset of the call in [`SITE_PATTERN`].
const SITE_CALL_AT: usize = 0x33;
/// The head of `fn_1983b0`: more than 32 elements (`0x100` bytes) partition
/// while the budget lasts. Other instantiations of `std::sort` share it, so it
/// is matched at the call's target, not searched for.
pub const SORT_PATTERN: &str = "48 89 5c 24 20 55 56 57 41 55 41 57 48 83 ec 40 48 8b c2 \
     41 0f b6 d9 48 2b c1 49 8b f8 48 83 e0 f8 4c 8b ea 4c 8b f9 48 3d 00 01 00 00 0f 8e 88 00 00 00 \
     48 85 ff 0f 8e 07 01 00 00 44 0f b6 cb 48 8d 4c 24 30 4d 8b c5 49 8b d7 e8 ?? ?? ?? ??";
/// Offset in `fn_1983b0` of [`COMPARE_PATTERN`].
const COMPARE_AT: usize = 0xf0;
/// The insertion sort's first comparison of `a` (`rsi`) with `b` (`rdi`):
/// `key(a) != key(b)` → `key(a) < key(b)` (vt+0x98, `setl`); else `flag(a) <
/// flag(b)` (vt+0x100, bytes, `setb`), with the heap fallback between them.
pub const COMPARE_PATTERN: &str = "49 8b 34 24 49 8b ec 49 8b 3f 48 8b ce 48 8b 06 ff 90 98 00 00 00 \
     48 8b 17 48 8b cf 48 8b d8 ff 92 98 00 00 00 48 8b 16 48 8b ce 48 3b d8 0f 85 9e 00 00 00 \
     ff 92 00 01 00 00 48 8b 17 48 8b cf 0f b6 d8 ff 92 00 01 00 00 3a d8 0f 92 c0 e9 9a 00 00 00 \
     44 0f b6 c3 49 8b d5 49 8b cf e8 ?? ?? ?? ?? 49 8b c5 49 2b c7 48 83 e0 f8 48 83 f8 10 \
     0f 8c 31 01 00 00 be 08 00 00 00 49 8d 7d f8 49 2b f7 0f 1f 40 00 66 66 0f 1f 84 00 00 00 00 00 \
     48 8b 07 4c 8d 4c 24 70 48 89 44 24 70 4c 8b c7 49 8b 07 4d 2b c7 49 c1 f8 03 33 d2 49 8b cf \
     48 89 07 88 5c 24 20 e8 ?? ?? ?? ?? 48 83 ef 08 48 8d 04 3e 48 83 e0 f8 48 83 f8 10 7d c3 \
     e9 d5 00 00 00 ff 92 98 00 00 00 48 8b 17 48 8b cf 48 8b d8 ff 92 98 00 00 00 48 3b d8 \
     0f 9c c0 84 c0";

const MESH_KEY: usize = 0x98;
const MESH_FLAG: usize = 0x100;

/// `fn_1983b0(begin, end, ideal, predicate)`, the predicate an empty lambda
/// passed by value.
pub type Sort = unsafe extern "C" fn(*mut *mut c_void, *mut *mut c_void, isize, u8);

static ORIGINAL: AtomicUsize = AtomicUsize::new(0);

/// A mesh's key, its flag and the mesh.
type Keyed = (i64, u8, *mut c_void);

thread_local! {
    /// Keys and meshes, kept between frames.
    static SCRATCH: RefCell<Vec<Keyed>> = const { RefCell::new(Vec::new()) };
}

/// The function in slot `offset` of the vtable of the object at `object`.
unsafe fn method<F: Copy>(object: *mut c_void, offset: usize) -> F {
    let vtable = unsafe { *(object as *const *const usize) };
    let entry = unsafe { *vtable.add(offset / 8) };
    unsafe { core::mem::transmute_copy(&entry) }
}

/// Sorts `meshes` as the game's `std::sort` does, `ideal` the partition budget
/// as the game passes it (the count).
///
/// # Safety
/// Each mesh must be one of the game's meshes (or behave like one).
pub unsafe fn sort_meshes(meshes: &mut [*mut c_void], ideal: isize, keyed: &mut Vec<Keyed>) {
    keyed.clear();
    keyed.extend(meshes.iter().map(|&mesh| {
        let key: unsafe extern "C" fn(*mut c_void) -> i64 = unsafe { method(mesh, MESH_KEY) };
        let flag: unsafe extern "C" fn(*mut c_void) -> u8 = unsafe { method(mesh, MESH_FLAG) };
        unsafe { (key(mesh), flag(mesh), mesh) }
    }));
    super::grass::msvc_sort(keyed, ideal, &|a: &Keyed, b: &Keyed| {
        if a.0 != b.0 {
            a.0 < b.0
        } else {
            a.1 < b.1
        }
    });
    for (slot, &(_, _, mesh)) in meshes.iter_mut().zip(keyed.iter()) {
        *slot = mesh;
    }
}

/// The detour for the main-view pass's mesh sort.
unsafe extern "C" fn sort(
    begin: *mut *mut c_void,
    end: *mut *mut c_void,
    ideal: isize,
    predicate: u8,
) {
    let original: Sort = unsafe { core::mem::transmute(ORIGINAL.load(Ordering::Acquire)) };
    if begin.is_null() || end < begin {
        return unsafe { original(begin, end, ideal, predicate) };
    }
    let meshes = unsafe { core::slice::from_raw_parts_mut(begin, end.offset_from(begin) as usize) };
    let done = SCRATCH.with(|scratch| match scratch.try_borrow_mut() {
        Ok(mut keyed) => {
            unsafe { sort_meshes(meshes, ideal, &mut keyed) };
            true
        }
        Err(_) => false,
    });
    if !done {
        unsafe { original(begin, end, ideal, predicate) };
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

/// The number of bytes `pattern` matches.
fn pattern_len(pattern: &str) -> usize {
    pattern.split_whitespace().count()
}

fn redirect(api: &Api) -> Result<(), String> {
    let base = unsafe { (api.module_base)(c"world2.dll".as_ptr()) };
    if base.is_null() {
        return Err("world2.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base) };
    let site = find(api, base, size, SITE_PATTERN);
    let compare = find(api, base, size, COMPARE_PATTERN);
    if site.is_null() || compare.is_null() {
        return Err("the main view's mesh sort was not found".into());
    }
    let call = unsafe { site.add(SITE_CALL_AT) };
    let sort_fn = unsafe { call_target(call) } as *mut u8;
    let inside = sort_fn as usize > base as usize
        && (sort_fn as usize) + pattern_len(SORT_PATTERN) <= base as usize + size;
    if !inside
        || find(
            api,
            sort_fn as *mut c_void,
            pattern_len(SORT_PATTERN),
            SORT_PATTERN,
        ) != sort_fn
        || sort_fn as usize + COMPARE_AT != compare as usize
    {
        return Err("the main view calls a different mesh sort".into());
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
    if setting == "engine" {
        return;
    }
    match redirect(api) {
        Ok(()) => super::say(
            api,
            LOG_INFO,
            "mesh sort: each mesh's key read once per frame",
        ),
        Err(e) => super::say(
            api,
            LOG_WARN,
            &format!("mesh sort: {e}; the game's own is used"),
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

    /// A stand-in mesh with just the two slots the comparison calls.
    #[repr(C)]
    struct Mesh {
        vtable: *const [usize; 33],
        key: i64,
        flag: u8,
    }

    unsafe extern "C" fn mesh_key(this: *mut Mesh) -> i64 {
        unsafe { (*this).key }
    }
    unsafe extern "C" fn mesh_flag(this: *mut Mesh) -> u8 {
        unsafe { (*this).flag }
    }

    fn vtable() -> Box<[usize; 33]> {
        let mut v = Box::new([0usize; 33]);
        v[MESH_KEY / 8] = mesh_key as unsafe extern "C" fn(_) -> _ as usize;
        v[MESH_FLAG / 8] = mesh_flag as unsafe extern "C" fn(_) -> _ as usize;
        v
    }

    /// `n` meshes: few distinct keys (negative ones too) and flags, so most
    /// comparisons tie on the key.
    fn meshes(rng: &mut Rng, n: usize, vtable: &[usize; 33]) -> Vec<Box<Mesh>> {
        let keys = 1 + rng.below(40) as i64;
        (0..n)
            .map(|_| {
                Box::new(Mesh {
                    vtable,
                    key: rng.below(keys as u64) as i64 - keys / 3,
                    flag: [0u8, 1, 255][rng.below(3) as usize],
                })
            })
            .collect()
    }

    fn pointers(meshes: &[Box<Mesh>]) -> Vec<*mut c_void> {
        meshes
            .iter()
            .map(|m| &**m as *const Mesh as *mut c_void)
            .collect()
    }

    #[test]
    fn orders_by_key_then_flag() {
        let vt = vtable();
        let mut rng = Rng(9);
        let meshes = meshes(&mut rng, 500, &vt);
        let mut v = pointers(&meshes);
        let ideal = v.len() as isize;
        unsafe { sort_meshes(&mut v, ideal, &mut Vec::new()) };
        let seen: Vec<(i64, u8)> = v
            .iter()
            .map(|&p| unsafe { ((*(p as *const Mesh)).key, (*(p as *const Mesh)).flag) })
            .collect();
        assert!(seen.windows(2).all(|w| w[0] <= w[1]));
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn VirtualProtect(address: *mut c_void, size: usize, protect: u32, old: *mut u32) -> i32;
    }

    /// Offset in `fn_1983b0` of its call to the `memmove` import thunk.
    const MEMMOVE_CALL_AT: usize = 0x1ed;

    unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
        unsafe { core::ptr::copy(src, dst, n) };
        dst
    }

    /// The game's mesh sort in its `world2.dll`, mapped without running it,
    /// with its one outside call (`memmove`) bound.
    fn world2() -> Option<Sort> {
        let dir = std::env::var_os("DEFIANCE_GAME_DIR")?;
        let path = std::path::Path::new(&dir).join("bin").join("world2.dll");
        let image = defiance_core::pe::map_file(&path).ok()?;
        let find = |text: &str| -> Option<usize> {
            let pattern = defiance_core::pattern::parse(text).ok()?;
            let hits = defiance_core::scan::scan(&image, &pattern);
            (hits.len() == 1).then(|| hits[0])
        };
        let (site, compare) = (find(SITE_PATTERN)?, find(COMPARE_PATTERN)?);
        let sort_pattern = defiance_core::pattern::parse(SORT_PATTERN).ok()?;
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
        let sort_rva = unsafe { call_target(at(site + SITE_CALL_AT)) } - base as usize;
        assert!(defiance_core::scan::scan(&image, &sort_pattern).contains(&sort_rva));
        assert_eq!(sort_rva + COMPARE_AT, compare);
        let call = at(sort_rva + MEMMOVE_CALL_AT);
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
        Some(unsafe { core::mem::transmute::<*mut u8, Sort>(at(sort_rva)) })
    }

    #[test]
    fn matches_the_games_sort() {
        let Some(game) = world2() else {
            eprintln!(
                "DEFIANCE_GAME_DIR's bin/world2.dll with the mesh sort is not available; skipped"
            );
            return;
        };
        let vt = vtable();
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut keyed = Vec::new();
        for round in 0..400 {
            let n = match round % 4 {
                0 => 2 + rng.below(40) as usize,
                1 => 2 + rng.below(300) as usize,
                2 => 2 + rng.below(5000) as usize,
                _ => 2000,
            };
            let meshes = meshes(&mut rng, n, &vt);
            // A small budget forces the heap fallback.
            let ideal = if round % 7 == 0 {
                rng.below(3) as isize
            } else {
                n as isize
            };
            let mut theirs = pointers(&meshes);
            let range = theirs.as_mut_ptr_range();
            unsafe { game(range.start, range.end, ideal, 0) };
            let mut ours = pointers(&meshes);
            unsafe { sort_meshes(&mut ours, ideal, &mut keyed) };
            assert!(ours == theirs, "round {round}, n {n}, ideal {ideal}");
        }
    }

    /// Time both on the same lists: `cargo test -p defiance-plugin-core
    /// --release --lib mesh_sort::tests::timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing() {
        let Some(game) = world2() else {
            return;
        };
        let vt = vtable();
        let mut rng = Rng(42);
        let mut keyed = Vec::new();
        for n in [200usize, 1000, 3000] {
            let meshes = meshes(&mut rng, n, &vt);
            let rounds = 1_000_000 / n;
            let (mut theirs, mut ours) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
            for _ in 0..rounds {
                let mut v = pointers(&meshes);
                let range = v.as_mut_ptr_range();
                let start = std::time::Instant::now();
                unsafe { game(range.start, range.end, n as isize, 0) };
                theirs += start.elapsed();
                let mut w = pointers(&meshes);
                let start = std::time::Instant::now();
                unsafe { sort_meshes(&mut w, n as isize, &mut keyed) };
                ours += start.elapsed();
                assert!(v == w);
            }
            eprintln!(
                "n {n:6}: game {:8.1} us, keyed {:8.1} us, {:.2}x",
                theirs.as_secs_f64() * 1e6 / rounds as f64,
                ours.as_secs_f64() * 1e6 / rounds as f64,
                theirs.as_secs_f64() / ours.as_secs_f64()
            );
        }
    }
}
