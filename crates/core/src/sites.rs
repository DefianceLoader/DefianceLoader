//! Finding a plugin's native sites by signature and RTTI instead of by build.
//!
//! A plugin names each function, call site or vtable it needs by what the code
//! looks like, not by where one build put it: a byte signature
//! (`tools/sigs.py`'s encoding: rel32 targets and RIP displacements
//! wildcarded, struct offsets kept) or an RTTI class. Each must resolve to
//! exactly one place, so a build where a site moved or changed shape resolves
//! to an error and the plugin writes nothing. The same code reads a running
//! module and a DLL mapped from disk, which is how plugin tests check every
//! build in `bin/`. `tools/site_signatures.py` writes the signatures from the
//! addresses known for each build.

use crate::{pattern, rtti, scan};

/// One site's signatures and how far into each match the site lies. Builds
/// whose code differs at the site get one alternative each; together they must
/// find exactly one place.
pub struct Signature {
    pub name: &'static str,
    pub alternatives: &'static [(&'static str, usize)],
}

/// A site found by `alternatives`: (signature, offset of the site in it).
pub const fn sig(name: &'static str, alternatives: &'static [(&'static str, usize)]) -> Signature {
    Signature { name, alternatives }
}

/// A module image laid out as mapped: `image[rva]` is the byte at `base + rva`.
/// `base` is where the image's absolute addresses point (the load address for a
/// running module, the preferred base for a file mapped from disk).
pub struct Image<'a> {
    pub image: &'a [u8],
    pub base: usize,
}

impl<'a> Image<'a> {
    /// A loaded module's image, readable in place.
    ///
    /// # Safety
    /// `base..base + size` must be a mapped module that stays loaded for `'a`.
    pub unsafe fn loaded(base: *const u8, size: usize) -> Self {
        Image {
            image: unsafe { core::slice::from_raw_parts(base, size) },
            base: base as usize,
        }
    }

    /// A DLL mapped from disk.
    pub fn mapped(mapped: &'a crate::pe::Mapped) -> Self {
        Image {
            image: &mapped.image,
            base: mapped.base,
        }
    }

    /// The rva of the one place `signature` finds.
    pub fn find(&self, signature: &Signature) -> Result<usize, String> {
        let mut found = Vec::new();
        for (text, at) in signature.alternatives {
            let pattern = pattern::parse(text).map_err(|e| format!("{}: {e}", signature.name))?;
            found.extend(
                scan::scan(self.image, &pattern)
                    .into_iter()
                    .map(|hit| hit + at),
            );
        }
        found.sort_unstable();
        found.dedup();
        match found.as_slice() {
            [hit] => Ok(*hit),
            [] => Err(format!("{} not found", signature.name)),
            hits => Err(format!("{} matches {} places", signature.name, hits.len())),
        }
    }

    /// The rva of a direct `call rel32` found by `signature`.
    pub fn call(&self, signature: &Signature) -> Result<usize, String> {
        let rva = self.find(signature)?;
        if self.image.get(rva) != Some(&0xe8) {
            return Err(format!("{} is not a direct call", signature.name));
        }
        Ok(rva)
    }

    /// The target rva of the `call rel32` or `jmp rel32` at `rva`.
    pub fn branch_target(&self, rva: usize) -> Option<usize> {
        if !matches!(self.image.get(rva), Some(0xe8 | 0xe9)) {
            return None;
        }
        let rel = i32::from_le_bytes(self.image.get(rva + 1..rva + 5)?.try_into().ok()?);
        rva.checked_add(5)?.checked_add_signed(rel as isize)
    }

    /// The rva a RIP-relative operand points at: `field` is the rva of its
    /// four-byte displacement and `next` the rva of the next instruction.
    pub fn rip_target(&self, field: usize, next: usize) -> Option<usize> {
        let rel = i32::from_le_bytes(self.image.get(field..field + 4)?.try_into().ok()?);
        next.checked_add_signed(rel as isize)
    }

    /// Whether the code at `rva` starts with `bytes`.
    pub fn starts_with(&self, rva: usize, bytes: &[u8]) -> bool {
        rva.checked_add(bytes.len())
            .and_then(|end| self.image.get(rva..end))
            == Some(bytes)
    }

    /// Ok if the code at `rva` starts with `bytes`: an entry hook relocates
    /// exactly these instructions, so a build whose prologue differs is refused
    /// rather than half-copied.
    pub fn expect(&self, what: &str, rva: usize, bytes: &[u8]) -> Result<(), String> {
        if self.starts_with(rva, bytes) {
            Ok(())
        } else {
            Err(format!("{what} at {rva:#x} starts differently"))
        }
    }

    /// The vtables of exactly `class` (`rtti::find` matches substrings).
    pub fn vtables(&self, class: &str) -> Vec<rtti::Vtable> {
        let code = code_ranges(self.image);
        let is_code = |rva: usize| code.iter().any(|(start, end)| rva >= *start && rva < *end);
        rtti::find(self.image, self.base, class, &is_code)
            .into_iter()
            .filter(|vtable| vtable.mangled == class)
            .collect()
    }

    /// The one vtable of exactly `class`.
    pub fn vtable(&self, class: &str) -> Result<rtti::Vtable, String> {
        let mut found = self.vtables(class);
        match found.len() {
            1 => Ok(found.remove(0)),
            0 => Err(format!("no {class} vtable")),
            n => Err(format!("{n} {class} vtables")),
        }
    }

    /// The one vtable of exactly `class` that its complete object locator
    /// places at offset zero (the primary one, not a secondary base's).
    pub fn primary_vtable(&self, class: &str) -> Result<rtti::Vtable, String> {
        let mut found: Vec<_> = self
            .vtables(class)
            .into_iter()
            .filter(|vtable| self.u32(vtable.locator + 4) == Some(0))
            .collect();
        match found.len() {
            1 => Ok(found.remove(0)),
            0 => Err(format!("no primary {class} vtable")),
            n => Err(format!("{n} primary {class} vtables")),
        }
    }

    /// The method rva in `slot` of the primary vtable of `class`.
    pub fn method(&self, class: &str, slot: usize) -> Result<usize, String> {
        let vtable = self.primary_vtable(class)?;
        vtable
            .methods
            .get(slot)
            .copied()
            .ok_or_else(|| format!("{class} has no method {slot}"))
    }

    pub fn u32(&self, rva: usize) -> Option<u32> {
        Some(u32::from_le_bytes(
            self.image.get(rva..rva + 4)?.try_into().ok()?,
        ))
    }

    pub fn u64(&self, rva: usize) -> Option<u64> {
        Some(u64::from_le_bytes(
            self.image.get(rva..rva + 8)?.try_into().ok()?,
        ))
    }
}

const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;

/// The (start, end) rvas of the executable sections, read from the image's own
/// section table, which a mapped module keeps at its start.
pub fn code_ranges(image: &[u8]) -> Vec<(usize, usize)> {
    let u16_at = |o: usize| {
        image
            .get(o..o + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let u32_at = |o: usize| {
        image
            .get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut out = Vec::new();
    let Some(pe) = u32_at(0x3c).map(|v| v as usize) else {
        return out;
    };
    if image.get(pe..pe + 4) != Some(b"PE\0\0") {
        return out;
    }
    let (Some(sections), Some(optional_size)) = (u16_at(pe + 6), u16_at(pe + 20)) else {
        return out;
    };
    let table = pe + 24 + optional_size as usize;
    for i in 0..sections as usize {
        let s = table + i * 40;
        let (Some(size), Some(va), Some(characteristics)) =
            (u32_at(s + 8), u32_at(s + 12), u32_at(s + 36))
        else {
            break;
        };
        if characteristics & IMAGE_SCN_MEM_EXECUTE != 0 {
            out.push((va as usize, (va + size) as usize));
        }
    }
    out
}

/// Every game build kept in `bin/<store>/<date>/`, oldest first.
pub const BUILDS: [&str; 8] = [
    "gog/2025-12-23",
    "steam/2025-12-23",
    "gog/2026-09-14",
    "steam/2026-09-22",
    "gog/2026-09-25",
    "steam/2026-09-25",
    "gog/2026-10-07",
    "steam/2026-10-07",
];

/// The 2026 builds: [`BUILDS`] without the December 2025 ones, which predate
/// much of what the plugins hook.
pub const BUILDS_2026: [&str; 6] = [
    "gog/2026-09-14",
    "steam/2026-09-22",
    "gog/2026-09-25",
    "steam/2026-09-25",
    "gog/2026-10-07",
    "steam/2026-10-07",
];

/// `bin/<build>/<dll>` mapped, or `None` when this checkout lacks it (the game
/// files are never committed). For tests.
pub fn reference(build: &str, dll: &str) -> Option<crate::pe::Mapped> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../bin")
        .join(build)
        .join(dll);
    path.exists()
        .then(|| crate::pe::map(&path).unwrap_or_else(|e| panic!("{e}")))
}

/// Every `world2.dll` in `bin/`, as (build, mapped).
pub fn reference_world2() -> Vec<(&'static str, crate::pe::Mapped)> {
    BUILDS
        .iter()
        .filter_map(|build| reference(build, "world2.dll").map(|m| (*build, m)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternatives_must_find_one_place_together() {
        let mut image = vec![0u8; 64];
        image[10..14].copy_from_slice(&[1, 2, 3, 4]);
        image[30..34].copy_from_slice(&[5, 6, 7, 8]);
        let image = Image {
            image: &image,
            base: 0,
        };
        let one = sig("one", &[("01020304", 1), ("09090909", 0)]);
        assert_eq!(image.find(&one), Ok(11));
        let two = sig("two", &[("01020304", 0), ("05060708", 0)]);
        assert_eq!(image.find(&two), Err("two matches 2 places".into()));
        let same = sig("same", &[("01020304", 2), ("0304", 0)]);
        assert_eq!(image.find(&same), Ok(12));
    }
}
