//! A PE file laid out as the loader maps it, each section at its rva, so a DLL
//! on disk can be searched exactly like a running module's image.

use std::path::Path;

/// A file mapped the way the OS would map it, plus what a walk needs to know
/// about it: its image base (the absolute addresses in a vtable are relative to
/// it) and which rvas are executable.
pub struct Mapped {
    pub base: usize,
    pub image: Vec<u8>,
    /// (start, end) rva ranges of the executable sections
    pub code: Vec<(usize, usize)>,
}

impl Mapped {
    /// Whether an rva lies in one of the executable sections.
    pub fn is_code(&self, rva: usize) -> bool {
        self.code
            .iter()
            .any(|(start, end)| rva >= *start && rva < *end)
    }
}

const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;

/// The file at `path`, mapped: headers at their rvas and every section copied
/// to its virtual address. The bytes the loader will actually execute, without
/// attaching to the game.
pub fn map(path: &Path) -> Result<Mapped, String> {
    let file = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bad = || format!("{} is not a PE file", path.display());
    let u16_at = |o: usize| {
        file.get(o..o + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
    };
    let u32_at = |o: usize| {
        file.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let u64_at = |o: usize| {
        file.get(o..o + 8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()) as usize)
    };
    if file.get(0..2) != Some(b"MZ") {
        return Err(bad());
    }
    let pe = u32_at(0x3c).ok_or_else(bad)?;
    if file.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err(bad());
    }
    let sections = u16_at(pe + 6).ok_or_else(bad)?;
    let optional = pe + 24;
    let table = optional + u16_at(pe + 20).ok_or_else(bad)?;
    let image_size = u32_at(optional + 56).ok_or_else(bad)?;
    let headers = u32_at(optional + 60)
        .ok_or_else(bad)?
        .min(file.len())
        .min(image_size);
    // PE32+ has a 64-bit image base; PE32 a 32-bit one. The game is PE32+.
    let base = match u16_at(optional) {
        Some(0x20b) => u64_at(optional + 24).ok_or_else(bad)?,
        Some(0x10b) => u32_at(optional + 28).ok_or_else(bad)?,
        _ => return Err(bad()),
    };
    let mut image = vec![0u8; image_size];
    image[..headers].copy_from_slice(&file[..headers]);
    let mut code = Vec::new();
    for i in 0..sections {
        let s = table + i * 40;
        let (va, virtual_size, raw_size, raw, characteristics) = (
            u32_at(s + 12).ok_or_else(bad)?,
            u32_at(s + 8).ok_or_else(bad)?,
            u32_at(s + 16).ok_or_else(bad)?,
            u32_at(s + 20).ok_or_else(bad)?,
            u32_at(s + 36).ok_or_else(bad)? as u32,
        );
        let n = raw_size
            .min(file.len().saturating_sub(raw))
            .min(image_size.saturating_sub(va));
        image[va..va + n].copy_from_slice(&file[raw..raw + n]);
        if characteristics & IMAGE_SCN_MEM_EXECUTE != 0 {
            code.push((va, va + virtual_size.max(raw_size)));
        }
    }
    Ok(Mapped { base, image, code })
}

/// The mapped image alone, for callers that do not need to walk addresses.
pub fn map_file(path: &Path) -> Result<Vec<u8>, String> {
    map(path).map(|mapped| mapped.image)
}
