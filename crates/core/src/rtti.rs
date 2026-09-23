//! MSVC RTTI, read from a live module: class name to vtable to methods.
//!
//! This is `tools/rtti.py`'s walk, on a module image in memory rather than a PE
//! on disk. The layout is the same:
//!
//! - a *type descriptor* holds the mangled name at `+16` (`.?AVWidget@@`);
//! - a *complete object locator* stores the descriptor as an image-relative
//!   offset in its fourth field, and is found by searching for that offset;
//! - a *vtable*'s `[-1]` slot holds the locator's absolute address, so its
//!   methods begin eight bytes after that pointer.
//!
//! What RTTI does not carry is a *name* per method: the slots are in
//! declaration order with no labels. `find` gives the addresses; naming them is
//! the loader's `rtti_method`, from a table the analysis tools produce.

/// One class's vtable, with the methods that could be read from it.
pub struct Vtable {
    /// the mangled name, `.?AVWidget@@`
    pub mangled: String,
    /// the descriptor's rva
    pub descriptor: usize,
    /// the complete object locator's rva
    pub locator: usize,
    /// the first method slot, just past the locator pointer
    pub methods_at: usize,
    /// method rvas, in slot order, stopping at the first non-code slot
    pub methods: Vec<usize>,
}

/// The classes whose mangled name contains `class`, each with its vtable and
/// methods. `is_code` says whether a method rva is executable, which is how the
/// walk knows where a vtable ends and the next data begins.
pub fn find(
    image: &[u8],
    base: usize,
    class: &str,
    is_code: &dyn Fn(usize) -> bool,
) -> Vec<Vtable> {
    let mut out = Vec::new();
    for (mangled, descriptor) in descriptors(image) {
        if !mangled.contains(class) {
            continue;
        }
        for locator in locators(image, descriptor) {
            for methods_at in vtables(image, base, locator) {
                let methods = methods(image, base, methods_at, is_code);
                out.push(Vtable {
                    mangled: mangled.clone(),
                    descriptor,
                    locator,
                    methods_at,
                    methods,
                });
            }
        }
    }
    out
}

/// The method address in `slot` (zero-based) of the first vtable of `class`, as
/// an rva, or `None`.
pub fn method(
    image: &[u8],
    base: usize,
    class: &str,
    slot: usize,
    is_code: &dyn Fn(usize) -> bool,
) -> Option<usize> {
    let found = find(image, base, class, is_code);
    found
        .first()
        .and_then(|vtable| vtable.methods.get(slot).copied())
}

/// Every `.?AV...@@` / `.?AU...@@` string, as (mangled, descriptor rva). The
/// descriptor is sixteen bytes before the name.
fn descriptors(image: &[u8]) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(found) = image[at..].windows(3).position(|w| w == b".?A") {
        let start = at + found;
        let kind = image.get(start + 3);
        if kind == Some(&b'V') || kind == Some(&b'U') {
            if let Some(end) = image[start..].windows(2).position(|w| w == b"@@") {
                let text_end = start + end + 2;
                if start >= 16 {
                    if let Ok(text) = std::str::from_utf8(&image[start..text_end]) {
                        out.push((text.to_string(), start - 16));
                    }
                }
                at = text_end;
                continue;
            }
        }
        at = start + 1;
    }
    out
}

/// The complete object locators whose descriptor field points at `descriptor`.
fn locators(image: &[u8], descriptor: usize) -> Vec<usize> {
    let want = (descriptor as u32).to_le_bytes();
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(found) = image[at..].windows(4).position(|w| w == want) {
        let field = at + found;
        if field >= 12 {
            let locator = field - 12;
            // the record's first field is a signature of 0 or 1
            if let Some(signature) = u32_at(image, locator) {
                if signature <= 1 {
                    out.push(locator);
                }
            }
        }
        at = field + 1;
    }
    out
}

/// The vtables whose `[-1]` slot holds this locator's absolute address.
fn vtables(image: &[u8], base: usize, locator: usize) -> Vec<usize> {
    let want = (base.wrapping_add(locator) as u64).to_le_bytes();
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(found) = image[at..].windows(8).position(|w| w == want) {
        out.push(at + found + 8);
        at = at + found + 1;
    }
    out
}

/// The method rvas from a vtable, stopping at the first slot that is null,
/// outside the module, or not code.
fn methods(
    image: &[u8],
    base: usize,
    mut at: usize,
    is_code: &dyn Fn(usize) -> bool,
) -> Vec<usize> {
    let mut out = Vec::new();
    while let Some(value) = u64_at(image, at) {
        if value < base as u64 {
            break;
        }
        let rva = value as usize - base;
        if rva >= image.len() || !is_code(rva) {
            break;
        }
        out.push(rva);
        at += 8;
    }
    out
}

fn u32_at(image: &[u8], at: usize) -> Option<u32> {
    image
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

fn u64_at(image: &[u8], at: usize) -> Option<u64> {
    image
        .get(at..at + 8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in module with one class, `.?AVWidget@@`, its locator and a
    /// three-method vtable. `text` is where the walk treats pointers as code.
    fn stand_in() -> (Vec<u8>, usize) {
        const BASE: usize = 0x1800_0000;
        const TEXT: std::ops::Range<usize> = 0x1000..0x2000;
        let mut image = vec![0u8; 0x3000];
        // a few executable bytes for the method targets
        image[TEXT][..0x40].fill(0xcc);
        let descriptor = 0x2000;
        image[descriptor + 16..descriptor + 16 + 12].copy_from_slice(b".?AVWidget@@");
        let locator = 0x2100;
        image[locator..locator + 4].copy_from_slice(&0u32.to_le_bytes());
        image[locator + 12..locator + 16].copy_from_slice(&(descriptor as u32).to_le_bytes());
        let methods_at = 0x2208;
        let pointer = ((BASE + locator) as u64).to_le_bytes();
        image[0x2200..0x2208].copy_from_slice(&pointer);
        for (n, method) in [0x1000usize, 0x1010, 0x1020].into_iter().enumerate() {
            let pointer = ((BASE + method) as u64).to_le_bytes();
            image[methods_at + n * 8..methods_at + n * 8 + 8].copy_from_slice(&pointer);
        }
        (image, BASE)
    }

    #[test]
    fn finds_a_class_and_its_methods() {
        let (image, base) = stand_in();
        let is_code = |rva: usize| (0x1000..0x2000).contains(&rva);
        let found = find(&image, base, "Widget", &is_code);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].mangled, ".?AVWidget@@");
        assert_eq!(found[0].locator, 0x2100);
        assert_eq!(found[0].methods, vec![0x1000, 0x1010, 0x1020]);
        assert_eq!(method(&image, base, "Widget", 1, &is_code), Some(0x1010));
        assert_eq!(method(&image, base, "Widget", 3, &is_code), None);
        assert!(find(&image, base, "Nothing", &is_code).is_empty());
    }

    #[test]
    fn a_method_off_the_code_stops_the_walk() {
        let (mut image, base) = stand_in();
        // the middle slot points outside .text, so only the first is a method
        let outside = ((base + 0x2fff) as u64).to_le_bytes();
        image[0x2210..0x2218].copy_from_slice(&outside);
        let is_code = |rva: usize| (0x1000..0x2000).contains(&rva);
        let found = find(&image, base, "Widget", &is_code);
        assert_eq!(found[0].methods, vec![0x1000]);
    }
}
