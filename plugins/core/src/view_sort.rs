//! Order the main view's visible objects with each object's key read once.
//!
//! The main-view pass (`world2.dll`, GOG 2026-09-14 `fn_18f940`) stable-sorts
//! the objects its visibility query returns before it files them into its draw
//! lists, with MSVC's `std::stable_sort` (the buffered merge sort `fn_198a20`
//! for more than 32 objects, called at `0x18fd1b`; [`SITE_PATTERN`],
//! [`STABLE_PATTERN`]). Its comparison ([`COMPARE_PATTERN`], inlined in the
//! insertion sort `fn_198660` and the merges) orders by type id (vt+0x68),
//! then, for type 80000, by squared distance from the camera, far to near
//! (through the object's node, vt+0x8 → vt+0x70, a `shared_ptr` copy each
//! time), and otherwise by the byte vt+0x148 returns. It makes up to six
//! virtual calls and four atomic reference-count changes per comparison, about
//! 2 ms of an 18.5 ms frame on a busy CPU-bound scene.
//!
//! Core redirects that one call ([`sort`]): each object's key is read once
//! ([`key`]), each distance bit for bit as the game computes it, and a stable
//! sort orders the keys. With no NaN distance the comparison is a strict weak
//! order, under which a stable sort's result is unique, so the order, ties
//! included, is the game's; with a NaN the game's own sort runs.
//! `tests::matches_the_games_sort` checks the order against the game's
//! function. On by default; `[loader] view_sort = engine` keeps the game's
//! sort.
use core::cell::RefCell;
use core::ffi::c_void;
use defiance_api::{Api, LOG_INFO, LOG_WARN};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

/// The camera position the predicate holds, then the stable sort's dispatch:
/// `n <= 32` insertion-sorts, else a temporary buffer (`fn_198080`) and
/// `fn_198a20(begin, end, n, buffer, capacity, &camera)`.
pub const SITE_PATTERN: &str = "48 8b 4c 24 50 48 8b 01 ff 50 30 f3 0f 10 00 f3 0f 11 45 80 \
     f3 0f 10 48 04 f3 0f 11 4d 84 f3 0f 10 40 08 f3 0f 11 45 88 4c 8b be 20 03 00 00 49 8b 3e \
     49 8b df 48 2b df 48 c1 fb 03 48 83 fb 20 7f 11 4c 8d 45 80 49 8b d7 48 8b cf e8 ?? ?? ?? ?? \
     eb 62 48 8b c3 48 99 48 2b c2 48 d1 f8 48 8b d3 48 2b d0 48 8d 8d 90 00 00 00 e8 ?? ?? ?? ?? 90 \
     48 8d 45 80 48 89 44 24 28 48 8b 85 98 00 00 00 48 89 44 24 20 4c 8b 8d 90 00 00 00 \
     4c 8b c3 49 8b d7 48 8b cf e8 ?? ?? ?? ??";
/// Offset of the call to `fn_198a20` in [`SITE_PATTERN`].
const SITE_CALL_AT: usize = 0x96;
/// The head of `fn_198a20`: at most 32 elements go to the insertion sort,
/// then the two halves' buffered merge sorts (`fn_19c170`). Other
/// instantiations of `std::stable_sort` share it, so it is matched at the
/// call's target, not searched for.
pub const STABLE_PATTERN: &str = "40 55 57 41 56 41 57 48 83 ec 48 4d 8b f9 49 8b f8 48 8b ea \
     4c 8b f1 49 83 f8 20 7f 17 4c 8b 84 24 98 00 00 00 48 83 c4 48 41 5f 41 5e 5f 5d e9 ?? ?? ?? ?? \
     48 89 5c 24 70 48 8b 9c 24 98 00 00 00 48 89 74 24 78 48 8b f7 48 d1 ee 48 2b fe \
     4c 89 a4 24 80 00 00 00 4c 89 6c 24 40 4c 8b c7 4c 8b ac 24 90 00 00 00 4c 8d 24 f9 49 8b d4 \
     49 3b fd 7f 22 48 89 5c 24 20 e8 ?? ?? ?? ??";
/// Offsets in `fn_198a20` of its call to `fn_19c170`, in `fn_19c170` of its
/// call to the insertion sort, and in `fn_198660` of [`COMPARE_PATTERN`].
const STABLE_MERGE_CALL_AT: usize = 0x78;
const MERGE_INSERTION_CALL_AT: usize = 0x4d;
const COMPARE_AT: usize = 0x53;
/// The insertion sort's first comparison of `a` (`rsi`) with `b` (`r14`),
/// the camera position at `rbx`: `type(a) != type(b)` → `type(a) < type(b)`;
/// type `0x13880` → `dist²(a) > dist²(b)`, each `((cx−x)² + (cy−y)²) + (cz−z)²`
/// with the node's position from vt+0x70, both `shared_ptr`s released;
/// else `flag(a) < flag(b)` (vt+0x148, unsigned bytes).
pub const COMPARE_PATTERN: &str = "49 8b 75 00 4d 8b 37 48 8b 06 48 8b ce ff 50 68 8b f8 49 8b 16 \
     49 8b ce ff 52 68 48 8b 16 48 8b ce 3b f8 0f 85 3e 01 00 00 ff 52 68 4c 8b 06 48 8b ce \
     3d 80 38 01 00 0f 85 0b 01 00 00 48 8d 54 24 30 41 ff 50 08 90 49 8b 06 48 8d 54 24 20 \
     49 8b ce ff 50 08 90 48 8b 4c 24 30 48 8b 01 ff 50 70 f3 0f 10 4b 08 f3 0f 5c 48 08 \
     f3 0f 10 43 04 f3 0f 5c 40 04 f3 0f 10 33 f3 0f 5c 30 f3 0f 59 f6 f3 0f 59 c0 f3 0f 58 f0 \
     f3 0f 59 c9 f3 0f 58 f1 48 8b 4c 24 20 48 8b 01 ff 50 70 f3 0f 10 53 08 f3 0f 5c 50 08 \
     f3 0f 10 43 04 f3 0f 5c 40 04 f3 0f 10 0b f3 0f 5c 08 f3 0f 59 c9 f3 0f 59 c0 f3 0f 58 c8 \
     f3 0f 59 d2 f3 0f 58 ca 0f 2f f1 40 0f 97 c5 48 8b 7c 24 28 48 85 ff 74 30 b8 ff ff ff ff \
     f0 0f c1 47 08 83 f8 01 75 21 48 8b 07 48 8b cf ff 10 b8 ff ff ff ff f0 0f c1 47 0c 83 f8 01 \
     75 0a 48 8b 07 48 8b cf ff 50 08 90 48 8b 7c 24 38 48 85 ff 74 64 b8 ff ff ff ff \
     f0 0f c1 47 08 83 f8 01 75 55 48 8b 07 48 8b cf ff 10 b8 ff ff ff ff f0 0f c1 47 0c 83 f8 01 \
     75 3e 48 8b 07 48 8b cf ff 50 08 eb 33 41 ff 90 48 01 00 00 0f b6 f8 49 8b 16 49 8b ce \
     ff 92 48 01 00 00 40 3a f8 40 0f 92 c5 eb 14 ff 52 68 8b f8 49 8b 16 49 8b ce ff 52 68 \
     3b f8 40 0f 9c c5";

/// The type id whose objects are ordered by distance.
const BY_DISTANCE: i32 = 80000;
const OBJECT_NODE: usize = 0x8;
const OBJECT_TYPE: usize = 0x68;
const OBJECT_FLAG: usize = 0x148;
const NODE_POSITION: usize = 0x70;

/// `fn_198a20(begin, end, count, buffer, capacity, &camera)`.
pub type Stable = unsafe extern "C" fn(
    *mut *mut c_void,
    *mut *mut c_void,
    isize,
    *mut c_void,
    isize,
    *const [f32; 3],
);

static ORIGINAL: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// Keys and objects, kept between frames to avoid reallocating.
    static SCRATCH: RefCell<Vec<(Key, *mut c_void)>> = const { RefCell::new(Vec::new()) };
}

/// What the game's comparison reads of an object: its type, and its distance
/// (type 80000) or its flag (the rest).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    kind: i32,
    distance: f32,
    flag: u8,
}

/// The game's `less(a, b)` over keys. A total order while no distance is NaN
/// (a distance is a sum of squares, never -0.0).
fn order(a: &Key, b: &Key) -> core::cmp::Ordering {
    a.kind.cmp(&b.kind).then_with(|| {
        if a.kind == BY_DISTANCE {
            b.distance.total_cmp(&a.distance)
        } else {
            a.flag.cmp(&b.flag)
        }
    })
}

/// The squared distance as the game computes it: `((cx−x)² + (cy−y)²) + (cz−z)²`.
pub fn distance(camera: &[f32; 3], position: &[f32; 3]) -> f32 {
    let dx = camera[0] - position[0];
    let dy = camera[1] - position[1];
    let dz = camera[2] - position[2];
    dx * dx + dy * dy + dz * dz
}

/// The function in slot `offset` of the vtable of the object at `object`.
unsafe fn method<F: Copy>(object: *mut c_void, offset: usize) -> F {
    let vtable = unsafe { *(object as *const *const usize) };
    let entry = unsafe { *vtable.add(offset / 8) };
    unsafe { core::mem::transmute_copy(&entry) }
}

/// MSVC's `shared_ptr` release of a control block (`_Ref_count_base`: uses at
/// +8, weak count at +0xc, `_Destroy` and `_Delete_this` its first two slots).
unsafe fn release(control: *mut c_void) {
    if control.is_null() {
        return;
    }
    let count =
        |offset: usize| unsafe { &*((control as *const u8).add(offset) as *const AtomicI32) };
    if count(8).fetch_sub(1, Ordering::AcqRel) == 1 {
        let destroy: unsafe extern "C" fn(*mut c_void) = unsafe { method(control, 0) };
        unsafe { destroy(control) };
        if count(0xc).fetch_sub(1, Ordering::AcqRel) == 1 {
            let delete: unsafe extern "C" fn(*mut c_void) = unsafe { method(control, 8) };
            unsafe { delete(control) };
        }
    }
}

/// An object's key, read through the same virtual calls as the game's
/// comparison.
unsafe fn key(object: *mut c_void, camera: &[f32; 3]) -> Key {
    let kind: unsafe extern "C" fn(*mut c_void) -> i32 = unsafe { method(object, OBJECT_TYPE) };
    let kind = unsafe { kind(object) };
    if kind != BY_DISTANCE {
        let flag: unsafe extern "C" fn(*mut c_void) -> u8 = unsafe { method(object, OBJECT_FLAG) };
        return Key {
            kind,
            distance: 0.0,
            flag: unsafe { flag(object) },
        };
    }
    let mut node = [core::ptr::null_mut::<c_void>(); 2];
    let get: unsafe extern "C" fn(*mut c_void, *mut [*mut c_void; 2]) -> usize =
        unsafe { method(object, OBJECT_NODE) };
    unsafe { get(object, &mut node) };
    let position: unsafe extern "C" fn(*mut c_void) -> *const [f32; 3] =
        unsafe { method(node[0], NODE_POSITION) };
    let position = unsafe { *position(node[0]) };
    unsafe { release(node[1]) };
    Key {
        kind,
        distance: distance(camera, &position),
        flag: 0,
    }
}

/// Stable-sorts `objects` as the game's comparison orders them, or returns
/// false, leaving them alone, when a distance is NaN.
///
/// # Safety
/// Each object must be one of the game's scene objects (or behave like one).
pub unsafe fn sort_objects(
    objects: &mut [*mut c_void],
    camera: &[f32; 3],
    keyed: &mut Vec<(Key, *mut c_void)>,
) -> bool {
    keyed.clear();
    for &object in objects.iter() {
        let key = unsafe { key(object, camera) };
        if key.distance.is_nan() {
            return false;
        }
        keyed.push((key, object));
    }
    keyed.sort_by(|a, b| order(&a.0, &b.0));
    for (slot, (_, object)) in objects.iter_mut().zip(keyed.iter()) {
        *slot = *object;
    }
    true
}

/// The detour for the main-view pass's stable sort.
unsafe extern "C" fn sort(
    begin: *mut *mut c_void,
    end: *mut *mut c_void,
    count: isize,
    buffer: *mut c_void,
    capacity: isize,
    camera: *const [f32; 3],
) {
    let original: Stable = unsafe { core::mem::transmute(ORIGINAL.load(Ordering::Acquire)) };
    let length = unsafe { end.offset_from(begin) };
    let sorted = length == count
        && SCRATCH.with(|scratch| match scratch.try_borrow_mut() {
            Ok(mut keyed) => unsafe {
                let objects = core::slice::from_raw_parts_mut(begin, length as usize);
                sort_objects(objects, &*camera, &mut keyed)
            },
            Err(_) => false,
        });
    if !sorted {
        unsafe { original(begin, end, count, buffer, capacity, camera) };
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

/// Whether the stable sort at `stable` reaches its insertion sort through its
/// merge sort, with [`COMPARE_PATTERN`] at its place there.
unsafe fn vouched(stable: *const u8, compare: *const u8) -> bool {
    unsafe {
        let merge = call_target(stable.add(STABLE_MERGE_CALL_AT)) as *const u8;
        let insertion = call_target(merge.add(MERGE_INSERTION_CALL_AT));
        insertion + COMPARE_AT == compare as usize
    }
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
        return Err("the main view's sort was not found".into());
    }
    let call = unsafe { site.add(SITE_CALL_AT) };
    let stable = unsafe { call_target(call) } as *mut u8;
    let inside = stable as usize > base as usize
        && (stable as usize) + pattern_len(STABLE_PATTERN) <= base as usize + size;
    if !inside
        || find(
            api,
            stable as *mut c_void,
            pattern_len(STABLE_PATTERN),
            STABLE_PATTERN,
        ) != stable
        || !unsafe { vouched(stable, compare) }
    {
        return Err("the main view calls a different sort".into());
    }
    let mut original = core::ptr::null_mut();
    let detour = sort as unsafe extern "C" fn(_, _, _, _, _, _) as *mut c_void;
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
            "view sort: each visible object's key read once per frame",
        ),
        Err(e) => super::say(
            api,
            LOG_WARN,
            &format!("view sort: {e}; the game's own is used"),
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

    /// Stand-ins for a scene object, its node and the node's `shared_ptr`
    /// control block, with just the slots the comparison calls.
    #[repr(C)]
    struct Control {
        vtable: *const [usize; 2],
        uses: AtomicI32,
        weaks: AtomicI32,
    }
    #[repr(C)]
    struct Node {
        vtable: *const [usize; 15],
        position: [f32; 3],
    }
    #[repr(C)]
    struct Object {
        vtable: *const [usize; 42],
        kind: i32,
        flag: u8,
        node: *mut Node,
        control: *mut Control,
    }

    unsafe extern "C" fn object_node(this: *mut Object, out: *mut [*mut c_void; 2]) -> usize {
        let this = unsafe { &*this };
        unsafe { (*this.control).uses.fetch_add(1, Ordering::Relaxed) };
        unsafe { *out = [this.node as *mut c_void, this.control as *mut c_void] };
        out as usize
    }
    unsafe extern "C" fn object_kind(this: *mut Object) -> i32 {
        unsafe { (*this).kind }
    }
    unsafe extern "C" fn object_flag(this: *mut Object) -> u8 {
        unsafe { (*this).flag }
    }
    unsafe extern "C" fn node_position(this: *mut Node) -> *const [f32; 3] {
        unsafe { &(*this).position }
    }
    unsafe extern "C" fn never(_: *mut Control) {
        panic!("a control block's last reference was released");
    }

    fn vtables() -> (Box<[usize; 42]>, Box<[usize; 15]>, Box<[usize; 2]>) {
        let mut object = Box::new([0usize; 42]);
        object[OBJECT_NODE / 8] = object_node as unsafe extern "C" fn(_, _) -> _ as usize;
        object[OBJECT_TYPE / 8] = object_kind as unsafe extern "C" fn(_) -> _ as usize;
        object[OBJECT_FLAG / 8] = object_flag as unsafe extern "C" fn(_) -> _ as usize;
        let mut node = Box::new([0usize; 15]);
        node[NODE_POSITION / 8] = node_position as unsafe extern "C" fn(_) -> _ as usize;
        let control = Box::new([never as unsafe extern "C" fn(_) as usize; 2]);
        (object, node, control)
    }

    /// `n` objects: a few types, many equal flags and distances.
    fn scene(
        rng: &mut Rng,
        n: usize,
        vt: &(Box<[usize; 42]>, Box<[usize; 15]>, Box<[usize; 2]>),
    ) -> Vec<Box<Object>> {
        let kinds = [0, 5, 20000, 20001, 50000, BY_DISTANCE, BY_DISTANCE, 90000];
        let grid = rng.below(2) == 0;
        (0..n)
            .map(|_| {
                let mut c = || {
                    let v = rng.below(1 << 16) as f32 / 64.0;
                    if grid {
                        v.floor()
                    } else {
                        v
                    }
                };
                let position = [c(), c(), c()];
                let node = Box::into_raw(Box::new(Node {
                    vtable: &*vt.1,
                    position,
                }));
                let control = Box::into_raw(Box::new(Control {
                    vtable: &*vt.2,
                    uses: AtomicI32::new(1),
                    weaks: AtomicI32::new(1),
                }));
                Box::new(Object {
                    vtable: &*vt.0,
                    kind: kinds[rng.below(kinds.len() as u64) as usize],
                    flag: rng.below(3) as u8,
                    node,
                    control,
                })
            })
            .collect()
    }

    #[test]
    fn orders_by_type_then_distance_or_flag() {
        let vt = vtables();
        let mut rng = Rng(3);
        let objects = scene(&mut rng, 200, &vt);
        let mut pointers: Vec<*mut c_void> = objects
            .iter()
            .map(|o| &**o as *const Object as *mut c_void)
            .collect();
        let camera = [100.0, 20.0, 100.0];
        assert!(unsafe { sort_objects(&mut pointers, &camera, &mut Vec::new()) });
        let seen: Vec<&Object> = pointers
            .iter()
            .map(|&p| unsafe { &*(p as *const Object) })
            .collect();
        for pair in seen.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert!(a.kind <= b.kind);
            if a.kind == b.kind && a.kind == BY_DISTANCE {
                let d = |o: &Object| distance(&camera, unsafe { &(*o.node).position });
                assert!(d(a) >= d(b));
            } else if a.kind == b.kind {
                assert!(a.flag <= b.flag);
            }
        }
        assert!(objects
            .iter()
            .all(|o| unsafe { (*o.control).uses.load(Ordering::Relaxed) } == 1));
    }

    #[test]
    fn a_nan_distance_leaves_the_order_to_the_game() {
        let vt = vtables();
        let mut rng = Rng(5);
        let objects = scene(&mut rng, 40, &vt);
        let mut pointers: Vec<*mut c_void> = objects
            .iter()
            .map(|o| &**o as *const Object as *mut c_void)
            .collect();
        let before = pointers.clone();
        let nan = objects.iter().find(|o| o.kind == BY_DISTANCE).unwrap();
        unsafe { (*nan.node).position[1] = f32::NAN };
        assert!(!unsafe { sort_objects(&mut pointers, &[0.0; 3], &mut Vec::new()) });
        assert_eq!(pointers, before);
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn VirtualProtect(address: *mut c_void, size: usize, protect: u32, old: *mut u32) -> i32;
    }

    /// Offset in `fn_198660` of its call to the `memmove` import thunk.
    const MEMMOVE_CALL_AT: usize = 0x1e0;

    unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
        unsafe { core::ptr::copy(src, dst, n) };
        dst
    }

    /// The game's stable sort in its `world2.dll`, mapped without running it,
    /// with its one outside call (`memmove`) bound.
    fn world2() -> Option<Stable> {
        let dir = std::env::var_os("DEFIANCE_GAME_DIR")?;
        let path = std::path::Path::new(&dir).join("bin").join("world2.dll");
        let image = defiance_core::pe::map_file(&path).ok()?;
        let find = |text: &str| -> Option<usize> {
            let pattern = defiance_core::pattern::parse(text).ok()?;
            let hits = defiance_core::scan::scan(&image, &pattern);
            (hits.len() == 1).then(|| hits[0])
        };
        let (site, compare) = (find(SITE_PATTERN)?, find(COMPARE_PATTERN)?);
        let stable_pattern = defiance_core::pattern::parse(STABLE_PATTERN).ok()?;
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
        let stable = unsafe { call_target(at(site + SITE_CALL_AT)) } - base as usize;
        assert!(defiance_core::scan::scan(&image, &stable_pattern).contains(&stable));
        assert!(unsafe { vouched(at(stable), at(compare)) });
        let call = unsafe { at(compare).sub(COMPARE_AT).add(MEMMOVE_CALL_AT) };
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
        Some(unsafe { core::mem::transmute::<*mut u8, Stable>(at(stable)) })
    }

    #[test]
    fn matches_the_games_sort() {
        let Some(game) = world2() else {
            eprintln!(
                "DEFIANCE_GAME_DIR's bin/world2.dll with the view sort is not available; skipped"
            );
            return;
        };
        let vt = vtables();
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut keyed = Vec::new();
        for round in 0..300 {
            let n = match round % 3 {
                0 => 33 + rng.below(100) as usize,
                1 => 33 + rng.below(3000) as usize,
                _ => 5000,
            };
            let objects = scene(&mut rng, n, &vt);
            let pointers: Vec<*mut c_void> = objects
                .iter()
                .map(|o| &**o as *const Object as *mut c_void)
                .collect();
            let camera = [rng.below(1024) as f32, 30.0, rng.below(1024) as f32];
            // The game's buffer holds half the objects, rounded up; a smaller
            // one makes it merge in place.
            let capacity = if round % 5 == 0 {
                rng.below(8) as usize
            } else {
                n - n / 2
            };
            let mut buffer = vec![0usize; capacity.max(1)];
            let mut theirs = pointers.clone();
            let range = theirs.as_mut_ptr_range();
            unsafe {
                game(
                    range.start,
                    range.end,
                    n as isize,
                    buffer.as_mut_ptr() as *mut c_void,
                    capacity as isize,
                    &camera,
                )
            };
            let mut ours = pointers.clone();
            assert!(unsafe { sort_objects(&mut ours, &camera, &mut keyed) });
            assert!(ours == theirs, "round {round}, n {n}, capacity {capacity}");
            assert!(objects
                .iter()
                .all(|o| unsafe { (*o.control).uses.load(Ordering::Relaxed) } == 1));
        }
    }

    /// Time both on the same scenes: `cargo test -p defiance-plugin-core
    /// --release --lib view_sort::tests::timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing() {
        let Some(game) = world2() else {
            return;
        };
        let vt = vtables();
        let mut rng = Rng(42);
        let mut keyed = Vec::new();
        for n in [200usize, 1000, 3000, 10000] {
            let objects = scene(&mut rng, n, &vt);
            let pointers: Vec<*mut c_void> = objects
                .iter()
                .map(|o| &**o as *const Object as *mut c_void)
                .collect();
            let camera = [512.0, 30.0, 512.0];
            let mut buffer = vec![0usize; n - n / 2];
            let rounds = 1_000_000 / n;
            let (mut theirs, mut ours) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
            for _ in 0..rounds {
                let mut v = pointers.clone();
                let range = v.as_mut_ptr_range();
                let start = std::time::Instant::now();
                unsafe {
                    game(
                        range.start,
                        range.end,
                        n as isize,
                        buffer.as_mut_ptr() as *mut c_void,
                        (n - n / 2) as isize,
                        &camera,
                    )
                };
                theirs += start.elapsed();
                let mut w = pointers.clone();
                let start = std::time::Instant::now();
                unsafe { sort_objects(&mut w, &camera, &mut keyed) };
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
