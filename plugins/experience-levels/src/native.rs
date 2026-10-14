//! The detours. The row reader records each `exp_levels.txt` row's extra
//! `level_6..` cells in a side table keyed by the row's levels object, which
//! keeps its stock six cells so the game's own `size == 6` checks still pass.
//! Each cell adapter parses the stock six levels as before; then, for every
//! extra level, it is run again with the levels object showing the six cells
//! that end at that level, and only that run's last element is kept. The
//! adapters parse a cell by inheriting from the one before it, so a window of
//! six gives the extra level the value the stock parse would.
use core::ffi::c_void;
use defiance_api::{Api, LOG_DEBUG, LOG_INFO, LOG_WARN};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// The stock number of levels: `level_0` to `level_5`.
pub(crate) const STOCK_LEVELS: usize = 6;
/// The bytes of an MSVC `std::string`.
const STRING_SIZE: usize = 0x20;
/// Where a levels object keeps its `std::vector<std::string>` of cells.
const CELLS_BEGIN: usize = 0x28;
const CELLS_END: usize = 0x30;
/// The reader's levels object and row view.
const READER_OBJECT: usize = 0xc8;
const READER_VIEW: usize = 0x20;
/// The row view's column header: `{ names, count }`.
const VIEW_HEADER: usize = 0x58;

pub(crate) static API: AtomicPtr<Api> = AtomicPtr::new(core::ptr::null_mut());
pub(crate) static MAX_LEVELS: AtomicUsize = AtomicUsize::new(32);
pub(crate) static READER: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
pub(crate) static LOOKUP: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

/// An MSVC `std::string`: the text inline when `cap` is at most 15, else a
/// pointer to it in the first eight bytes. The game only reads the strings
/// this plugin builds, and never frees them.
#[repr(C)]
pub(crate) struct MsvcString {
    buf: [u8; 16],
    size: usize,
    cap: usize,
}

const _: () = assert!(core::mem::size_of::<MsvcString>() == STRING_SIZE);

impl MsvcString {
    /// A NUL-terminated copy of `bytes`, inline when it fits; a longer one is
    /// kept in `heap`, which must outlive the string.
    pub(crate) fn new(bytes: &[u8], heap: &mut Vec<Box<[u8]>>) -> Self {
        let mut buf = [0; 16];
        if bytes.len() < 16 {
            buf[..bytes.len()].copy_from_slice(bytes);
            return MsvcString {
                buf,
                size: bytes.len(),
                cap: 15,
            };
        }
        let mut text = Vec::with_capacity(bytes.len() + 1);
        text.extend_from_slice(bytes);
        text.push(0);
        let text = text.into_boxed_slice();
        buf[..8].copy_from_slice(&(text.as_ptr() as usize).to_le_bytes());
        heap.push(text);
        MsvcString {
            buf,
            size: bytes.len(),
            cap: bytes.len(),
        }
    }

    /// The text.
    ///
    /// # Safety
    /// `self` must be a live MSVC string.
    pub(crate) unsafe fn bytes(&self) -> &[u8] {
        let text = if self.cap > 15 {
            usize::from_le_bytes(self.buf[..8].try_into().unwrap()) as *const u8
        } else {
            self.buf.as_ptr()
        };
        core::slice::from_raw_parts(text, self.size)
    }
}

/// One row's cells: copies of the six stock ones, then the extra levels.
pub(crate) struct Row {
    pub(crate) cells: Vec<MsvcString>,
    _heap: Vec<Box<[u8]>>,
}

/// Rows with extra levels, by the address of their levels object.
static ROWS: Mutex<BTreeMap<usize, Arc<Row>>> = Mutex::new(BTreeMap::new());

fn rows() -> std::sync::MutexGuard<'static, BTreeMap<usize, Arc<Row>>> {
    ROWS.lock().unwrap_or_else(PoisonError::into_inner)
}

unsafe fn log(level: u32, text: &str) {
    let Some(api) = API.load(Ordering::Acquire).as_ref() else {
        return;
    };
    if let Ok(text) = std::ffi::CString::new(text) {
        (api.log)(level, text.as_ptr());
    }
}

/// Logs `text` the first time it is seen.
pub(crate) unsafe fn log_once(level: u32, text: String) {
    static SEEN: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    let first = SEEN
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(text.clone());
    if first {
        log(level, &text);
    }
}

type Reader = unsafe extern "C" fn(usize);
type Lookup =
    unsafe extern "C" fn(usize, *const MsvcString, *const MsvcString, usize) -> *const MsvcString;

/// The row reader: runs the stock reader, then records the row's extra
/// levels, or forgets its levels object when it has none.
pub(crate) unsafe extern "C" fn read_row(reader: usize) {
    let original: Reader = core::mem::transmute(READER.load(Ordering::Acquire));
    original(reader);
    let object = *((reader + READER_OBJECT) as *const usize);
    if object == 0 {
        return;
    }
    let lookup: Lookup = core::mem::transmute(LOOKUP.load(Ordering::Acquire));
    let row = extra_levels(
        object,
        reader + READER_VIEW,
        MAX_LEVELS.load(Ordering::Relaxed),
        |view, key, default| lookup(view, key, default, 0),
    );
    match row {
        Some(row) => {
            log_once(
                LOG_INFO,
                "experience levels: first row with extra levels read".into(),
            );
            log(
                LOG_DEBUG,
                &format!(
                    "experience levels: levels object {object:#x} has {} levels",
                    row.cells.len()
                ),
            );
            rows().insert(object, Arc::new(row));
        }
        None => {
            rows().remove(&object);
        }
    }
}

/// The row's cells when it has a non-empty `level_6` column, up to
/// `max_levels` or the first missing or empty one. `lookup` is the game's
/// cell lookup on the row view; it is asked only for columns the header has,
/// so it never takes its missing-column path.
unsafe fn extra_levels(
    object: usize,
    view: usize,
    max_levels: usize,
    lookup: impl Fn(usize, *const MsvcString, *const MsvcString) -> *const MsvcString,
) -> Option<Row> {
    let begin = *((object + CELLS_BEGIN) as *const usize);
    let end = *((object + CELLS_END) as *const usize);
    if end.wrapping_sub(begin) != STOCK_LEVELS * STRING_SIZE {
        return None;
    }
    let header = *((view + VIEW_HEADER) as *const *const usize);
    if header.is_null() || *header == 0 {
        return None;
    }
    let names = core::slice::from_raw_parts(*header as *const MsvcString, *header.add(1));
    let mut heap = Vec::new();
    let stock = core::slice::from_raw_parts(begin as *const MsvcString, STOCK_LEVELS);
    let mut cells: Vec<MsvcString> = stock
        .iter()
        .map(|cell| MsvcString::new(cell.bytes(), &mut heap))
        .collect();
    let empty = MsvcString::new(b"", &mut heap);
    for level in STOCK_LEVELS..max_levels {
        let name = format!("level_{level}");
        if !names.iter().any(|column| column.bytes() == name.as_bytes()) {
            break;
        }
        let key = MsvcString::new(name.as_bytes(), &mut heap);
        let cell = &*lookup(view, &key, &empty);
        if cell.size == 0 {
            break;
        }
        let cell = MsvcString::new(cell.bytes(), &mut heap);
        cells.push(cell);
    }
    (cells.len() > STOCK_LEVELS).then_some(Row { cells, _heap: heap })
}

/// What the adapter running on this thread has seen.
#[derive(Default)]
struct Call {
    active: bool,
    /// The levels object the stock parse resolved, and its row if it has
    /// extra levels.
    object: usize,
    row: Option<Arc<Row>>,
    /// While non-zero, the first cell the levels object shows.
    window: usize,
    /// The levels object's own cell range while a window replaces it.
    swapped: Option<(usize, usize, usize)>,
}

thread_local! {
    static CALL: RefCell<Call> = RefCell::default();
}

/// Called with each object an adapter's resolver returns. The stock parse
/// records it; a window run makes it show the window's cells if it is the
/// same object.
unsafe fn resolved(object: usize) {
    CALL.with_borrow_mut(|call| {
        if !call.active || object == 0 {
            return;
        }
        if call.window == 0 {
            call.object = object;
            call.row = rows().get(&object).cloned();
            return;
        }
        let (Some(row), None) = (&call.row, call.swapped) else {
            return;
        };
        if object != call.object {
            return;
        }
        let cells = row.cells[call.window..call.window + STOCK_LEVELS].as_ptr_range();
        let begin = (object + CELLS_BEGIN) as *mut usize;
        let end = (object + CELLS_END) as *mut usize;
        call.swapped = Some((object, *begin, *end));
        *begin = cells.start as usize;
        *end = cells.end as usize;
    });
}

/// Puts back the levels object's own cells, and says whether a window was
/// shown.
fn restore() -> bool {
    CALL.with_borrow_mut(|call| {
        let Some((object, begin, end)) = call.swapped.take() else {
            return false;
        };
        unsafe {
            *((object + CELLS_BEGIN) as *mut usize) = begin;
            *((object + CELLS_END) as *mut usize) = end;
        }
        true
    })
}

/// Ends the adapter's call even if it unwinds, so a later one starts clean
/// and no window stays shown.
struct EndCall;

impl Drop for EndCall {
    fn drop(&mut self) {
        restore();
        CALL.with_borrow_mut(|call| *call = Call::default());
    }
}

/// An `std::vector` of plain elements: begin, end, capacity end.
type Vector = [usize; 3];

/// Runs one adapter call: `run` calls the stock adapter appending to the
/// vector at `out`. Rows with extra levels get one more element per level.
unsafe fn adapt(what: &str, out: *mut Vector, run: impl Fn() -> usize) -> usize {
    if CALL.with_borrow(|call| call.active) {
        return run();
    }
    CALL.with_borrow_mut(|call| call.active = true);
    let _end = EndCall;
    let base = (*out)[1].wrapping_sub((*out)[0]);
    let result = run();
    let Some(row) = CALL.with_borrow(|call| call.row.clone()) else {
        return result;
    };
    let mut kept = (*out)[1].wrapping_sub((*out)[0]);
    let growth = kept.wrapping_sub(base);
    if growth == 0 || growth > kept || !growth.is_multiple_of(STOCK_LEVELS) {
        log_once(
            LOG_WARN,
            format!("experience levels: {what} added {growth:#x} bytes for six levels; extra levels skipped"),
        );
        return result;
    }
    let element = growth / STOCK_LEVELS;
    for level in STOCK_LEVELS..row.cells.len() {
        CALL.with_borrow_mut(|call| call.window = level + 1 - STOCK_LEVELS);
        run();
        let shown = restore();
        let begin = (*out)[0];
        let now = (*out)[1].wrapping_sub(begin);
        if !shown || now != kept + growth {
            log_once(
                LOG_WARN,
                format!("experience levels: {what} did not parse level {level} as expected; levels from it on skipped"),
            );
            if now >= kept {
                (*out)[1] = begin + kept;
            }
            break;
        }
        core::ptr::copy_nonoverlapping(
            (begin + now - element) as *const u8,
            (begin + kept) as *mut u8,
            element,
        );
        kept += element;
        (*out)[1] = begin + kept;
    }
    result
}

/// An adapter detour and its original, with the type of its fourth argument
/// (a default value: integer adapters take it in `r9`, float ones in `xmm3`).
macro_rules! adapter {
    ($detour:ident, $original:ident, $what:literal, $fourth:ty) => {
        pub(crate) static $original: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

        pub(crate) unsafe extern "C" fn $detour(
            reader: usize,
            out: *mut Vector,
            key: usize,
            fourth: $fourth,
        ) -> usize {
            let original: unsafe extern "C" fn(usize, *mut Vector, usize, $fourth) -> usize =
                core::mem::transmute($original.load(Ordering::Acquire));
            adapt($what, out, || original(reader, out, key, fourth))
        }
    };
}

adapter!(pairs, PAIRS, "the pairs adapter", usize);
adapter!(triples, TRIPLES, "the triples adapter", usize);
adapter!(ints, INTS, "the ints adapter", usize);
adapter!(floats, FLOATS, "the floats adapter", f32);
adapter!(floats10, FLOATS10, "the scaled floats adapter", f32);
adapter!(ints100, INTS100, "the scaled ints adapter", usize);

/// A resolver call detour and its original.
macro_rules! resolver {
    ($detour:ident, $original:ident) => {
        pub(crate) static $original: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

        pub(crate) unsafe extern "C" fn $detour(a: usize, b: usize, c: usize, d: usize) -> usize {
            let original: unsafe extern "C" fn(usize, usize, usize, usize) -> usize =
                core::mem::transmute($original.load(Ordering::Acquire));
            let object = original(a, b, c, d);
            resolved(object);
            object
        }
    };
}

resolver!(resolve_pairs, RESOLVE_PAIRS);
resolver!(resolve_triples, RESOLVE_TRIPLES);
resolver!(resolve_ints, RESOLVE_INTS);
resolver!(resolve_floats, RESOLVE_FLOATS);
resolver!(resolve_floats10, RESOLVE_FLOATS10);
resolver!(resolve_ints100, RESOLVE_INTS100);

/// Each adapter's (entry detour, its original, resolver call detour, its
/// original), in [`crate::sites::ADAPTERS`] order.
pub(crate) fn adapter_hooks() -> [(
    *mut c_void,
    &'static AtomicPtr<c_void>,
    *mut c_void,
    &'static AtomicPtr<c_void>,
); 6] {
    [
        (pairs as _, &PAIRS, resolve_pairs as _, &RESOLVE_PAIRS),
        (
            triples as _,
            &TRIPLES,
            resolve_triples as _,
            &RESOLVE_TRIPLES,
        ),
        (ints as _, &INTS, resolve_ints as _, &RESOLVE_INTS),
        (floats as _, &FLOATS, resolve_floats as _, &RESOLVE_FLOATS),
        (
            floats10 as _,
            &FLOATS10,
            resolve_floats10 as _,
            &RESOLVE_FLOATS10,
        ),
        (
            ints100 as _,
            &INTS100,
            resolve_ints100 as _,
            &RESOLVE_INTS100,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(texts: &[&str], heap: &mut Vec<Box<[u8]>>) -> Vec<MsvcString> {
        texts
            .iter()
            .map(|text| MsvcString::new(text.as_bytes(), heap))
            .collect()
    }

    #[test]
    fn strings_are_inline_up_to_fifteen_bytes() {
        let mut heap = Vec::new();
        let short = MsvcString::new(b"level_6", &mut heap);
        assert_eq!((short.size, short.cap), (7, 15));
        assert!(heap.is_empty());
        let long = MsvcString::new(b"sixteen bytes!!!", &mut heap);
        assert_eq!((long.size, long.cap), (16, 16));
        assert_eq!(heap[0].last(), Some(&0));
        unsafe {
            assert_eq!(short.bytes(), b"level_6");
            assert_eq!(long.bytes(), b"sixteen bytes!!!");
        }
    }

    /// A levels object: its cell vector at the game's offsets.
    struct Object {
        memory: Box<[usize; 8]>,
        _cells: Vec<MsvcString>,
    }

    impl Object {
        fn new(texts: &[&str], heap: &mut Vec<Box<[u8]>>) -> Self {
            let cells = strings(texts, heap);
            let range = cells.as_ptr_range();
            let mut memory = Box::new([0; 8]);
            memory[CELLS_BEGIN / 8] = range.start as usize;
            memory[CELLS_END / 8] = range.end as usize;
            Object {
                memory,
                _cells: cells,
            }
        }

        fn address(&self) -> usize {
            self.memory.as_ptr() as usize
        }
    }

    /// A row view over `columns` whose lookup returns `values`.
    fn read(object: &Object, columns: &[&str], values: &[&str], max: usize) -> Option<Row> {
        let mut heap = Vec::new();
        let names = strings(columns, &mut heap);
        let values = strings(values, &mut heap);
        let header = [names.as_ptr() as usize, names.len()];
        let mut view = [0usize; 12];
        view[VIEW_HEADER / 8] = header.as_ptr() as usize;
        unsafe {
            extra_levels(
                object.address(),
                view.as_ptr() as usize,
                max,
                |_, key, _| {
                    let key = (*key).bytes();
                    let at = names.iter().position(|n| n.bytes() == key).unwrap();
                    &values[at]
                },
            )
        }
    }

    const STOCK: [&str; 6] = ["0", "1", "2", "3", "4", "5"];

    fn texts(row: &Row) -> Vec<String> {
        row.cells
            .iter()
            .map(|c| String::from_utf8(unsafe { c.bytes() }.to_vec()).unwrap())
            .collect()
    }

    #[test]
    fn extra_levels_stop_at_a_missing_or_empty_column_or_the_maximum() {
        let mut heap = Vec::new();
        let object = Object::new(&STOCK, &mut heap);
        let columns = ["name", "level_6", "level_7", "level_8"];
        let row = read(&object, &columns, &["x", "6", "7", ""], 64).unwrap();
        assert_eq!(texts(&row), ["0", "1", "2", "3", "4", "5", "6", "7"]);
        let row = read(&object, &columns, &["x", "6", "7", "8"], 8).unwrap();
        assert_eq!(texts(&row).len(), 8);
        assert!(read(&object, &["name", "level_7"], &["x", "7"], 64).is_none());
        assert!(read(&object, &["level_6"], &[""], 64).is_none());
        let short = Object::new(&STOCK[..5], &mut heap);
        assert!(read(&short, &columns, &["x", "6", "7", "8"], 64).is_none());
    }

    /// A stock-like ints adapter: resolves `object`, then appends one parsed
    /// int per cell, an empty cell inheriting the one before it.
    unsafe fn stock_ints(object: usize, out: &RefCell<Vec<u32>>) -> usize {
        resolved(object);
        let begin = *((object + CELLS_BEGIN) as *const usize);
        let end = *((object + CELLS_END) as *const usize);
        assert_eq!(end - begin, 0xc0);
        let cells = core::slice::from_raw_parts(begin as *const MsvcString, 6);
        let mut previous = 0;
        for cell in cells {
            let text = core::str::from_utf8(cell.bytes()).unwrap();
            previous = text.parse().unwrap_or(previous);
            out.borrow_mut().push(previous);
        }
        0
    }

    /// Runs `adapt` over a real `Vec<u32>` through its raw parts.
    fn run_ints(object: &Object, before: &[u32]) -> Vec<u32> {
        let out = RefCell::new(before.to_vec());
        let vector: RefCell<Vector> = RefCell::new([0; 3]);
        let sync = |out: &Vec<u32>, vector: &mut Vector| {
            let begin = out.as_ptr() as usize;
            *vector = [begin, begin + out.len() * 4, begin + out.capacity() * 4];
        };
        sync(&out.borrow(), &mut vector.borrow_mut());
        unsafe {
            adapt("test", vector.as_ptr(), || {
                // Take back what `adapt` trimmed, run, and publish again.
                let len = (vector.borrow()[1] - vector.borrow()[0]) / 4;
                out.borrow_mut().truncate(len);
                stock_ints(object.address(), &out);
                sync(&out.borrow(), &mut vector.borrow_mut());
                0
            });
        }
        let len = (vector.borrow()[1] - vector.borrow()[0]) / 4;
        let mut out = out.into_inner();
        out.truncate(len);
        out
    }

    #[test]
    fn an_adapter_gains_one_element_per_extra_level() {
        let mut heap = Vec::new();
        let object = Object::new(&STOCK, &mut heap);
        let columns = ["level_6", "level_7", "level_8"];
        let row = read(&object, &columns, &["60", " ", "80"], 64).unwrap();
        rows().insert(object.address(), Arc::new(row));
        let cells = run_ints(&object, &[9]);
        rows().remove(&object.address());
        // " " parses as nothing, so level 7 inherits level 6.
        assert_eq!(cells, [9, 0, 1, 2, 3, 4, 5, 60, 60, 80]);
        // The object shows its own six cells again.
        let begin = object.memory[CELLS_BEGIN / 8];
        let end = object.memory[CELLS_END / 8];
        assert_eq!(end - begin, 0xc0);
        assert_eq!(begin, object._cells.as_ptr() as usize);
    }

    #[test]
    fn an_object_without_extra_levels_is_untouched() {
        let mut heap = Vec::new();
        let object = Object::new(&STOCK, &mut heap);
        assert_eq!(run_ints(&object, &[]), [0, 1, 2, 3, 4, 5]);
    }
}
