//! A patch unit: one plugin's assembled code for one module, and what it
//! writes there. `tools/units.py` writes the descriptors and `tools/variant.py`
//! resolves them per build; `tools/units.py` documents each field.
//!
//! A unit is self-contained: its blob is position independent once its fixups
//! are resolved, its private cells are inside it, and a cell several units
//! share is imported by name from whoever links it. Relocating a unit to a
//! build it was not written for moves every address it names with the site
//! whose signature holds it, and re-reads the bytes each write replaces.

use crate::json::{self, Value};
use crate::scan::{Moves, Site};

/// What a write puts at its site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A `jmp rel32` to the entry, then `tail`.
    Jmp,
    /// A `call rel32` to the entry, then `tail`.
    Call,
    /// `after` in place of `before`.
    Edit,
}

#[derive(Clone, Debug)]
pub struct Write {
    pub kind: Kind,
    pub rva: usize,
    pub before: Vec<u8>,
    /// The unit offset a branch enters, and its routine's label.
    pub entry: usize,
    pub label: String,
    pub tail: Vec<u8>,
    pub after: Vec<u8>,
    /// The function a call's rel32 reached before it was retargeted; zero
    /// where the unit does not rely on it.
    pub stock: usize,
}

/// A slot in the blob resolved where the unit lands.
#[derive(Clone, Debug)]
pub enum Fixup {
    /// The module base plus `target`, as a qword.
    Abs64 { offset: usize, target: usize },
    /// A branch's rel32 to the module's `target`.
    Rel32 { offset: usize, target: usize },
    /// `to - from`, as a disp32.
    Delta {
        offset: usize,
        from: usize,
        to: usize,
    },
    /// A Win32 function's address, as a qword.
    Export {
        offset: usize,
        dll: String,
        name: String,
    },
    /// A disp32 to an imported cell, from an instruction ending at `end`.
    Cell {
        offset: usize,
        end: usize,
        cell: String,
    },
    /// A rel32 inside the edit at `rva`, reaching `target`: nothing to resolve
    /// in the blob, but relocation re-aims it.
    Edit {
        rva: usize,
        offset: usize,
        target: usize,
    },
}

/// A named span of the blob its plugin (or Core, for it) writes.
#[derive(Clone, Debug)]
pub struct Cell {
    pub name: String,
    pub offset: usize,
    pub bytes: usize,
}

/// A function a plugin may replace outright: the jmp write at `rva`.
#[derive(Clone, Debug)]
pub struct Native {
    pub name: String,
    pub rva: usize,
}

/// Bytes the module must hold before anything is written.
#[derive(Clone, Debug)]
pub struct Anchor {
    pub rva: usize,
    pub bytes: Vec<u8>,
}

/// A write as it lands: (address, what must be there, what is written).
pub type Placed = (usize, Vec<u8>, Vec<u8>);

#[derive(Clone, Debug)]
pub struct Unit {
    pub name: String,
    /// The plugin that owns it (`core` for Core's own).
    pub plugin: String,
    /// "logic.dll" or "game.dll".
    pub module: String,
    pub source_sha256: String,
    pub unit_bytes: usize,
    pub cells: Vec<Cell>,
    pub writes: Vec<Write>,
    pub fixups: Vec<Fixup>,
    pub natives: Vec<Native>,
    pub anchors: Vec<Anchor>,
    pub sites: Vec<Site>,
    /// Other builds checked against the signatures, relocated without asking.
    pub verified: Vec<String>,
}

fn hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err(format!("odd-length hex {text:?}"));
    }
    (0..text.len() / 2)
        .map(|i| {
            u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|_| format!("bad hex {text:?}"))
        })
        .collect()
}

fn rel32(from_end: usize, to: usize) -> Result<[u8; 4], String> {
    i32::try_from(to as i128 - from_end as i128)
        .map(i32::to_le_bytes)
        .map_err(|_| format!("{to:#x} is outside rel32 reach of {from_end:#x}"))
}

fn bytes(image: &[u8], rva: usize, len: usize) -> Result<Vec<u8>, String> {
    image
        .get(rva..rva + len)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| format!("{rva:#x} is outside the module"))
}

struct Fields<'a>(&'a Value, &'a str);

impl Fields<'_> {
    fn value(&self, key: &str) -> Result<&Value, String> {
        self.0
            .get(key)
            .ok_or_else(|| format!("{}: no {key}", self.1))
    }
    fn number(&self, key: &str) -> Result<usize, String> {
        self.value(key)?
            .as_usize()
            .ok_or_else(|| format!("{}: {key} is not a whole number", self.1))
    }
    fn text(&self, key: &str) -> Result<&str, String> {
        self.value(key)?
            .as_str()
            .ok_or_else(|| format!("{}: {key} is not a string", self.1))
    }
    fn hex(&self, key: &str) -> Result<Vec<u8>, String> {
        hex(self.text(key)?)
    }
    fn list(&self, key: &str) -> Result<&[Value], String> {
        self.value(key)?
            .as_array()
            .ok_or_else(|| format!("{}: {key} is not a list", self.1))
    }
}

fn site(value: &Value, unit: &str) -> Result<Site, String> {
    let f = Fields(value, unit);
    let text = f.text("site_pattern")?;
    let pattern = (0..text.len() / 2)
        .map(|i| match &text[i * 2..i * 2 + 2] {
            "??" => Ok(None),
            pair => u8::from_str_radix(pair, 16)
                .map(Some)
                .map_err(|_| format!("{unit}: bad signature")),
        })
        .collect::<Result<_, _>>()?;
    Ok(Site::new(
        f.text("site_name")?,
        f.number("site_start")?,
        f.number("site_group")?,
        pattern,
    ))
}

impl Unit {
    /// The unit a `tools/units.py` descriptor describes.
    pub fn parse(text: &str) -> Result<Unit, String> {
        let document = json::parse(text)?;
        let top = Fields(&document, "unit");
        if top.number("unit_schema")? != 1 {
            return Err("unit: regenerate the unit descriptors (unit_schema is not 1)".into());
        }
        let name = top.text("name")?.to_string();
        let at = name.as_str();
        let mut writes = Vec::new();
        for value in top.list("writes")? {
            let f = Fields(value, at);
            let kind = match f.text("kind")? {
                "jmp" => Kind::Jmp,
                "call" => Kind::Call,
                "edit" => Kind::Edit,
                other => return Err(format!("{at}: unknown write kind {other}")),
            };
            let branch = kind != Kind::Edit;
            writes.push(Write {
                kind,
                rva: f.number("rva")?,
                before: f.hex("before")?,
                entry: if branch { f.number("entry")? } else { 0 },
                label: if branch {
                    f.text("label")?.to_string()
                } else {
                    String::new()
                },
                tail: if branch { f.hex("tail")? } else { Vec::new() },
                after: if branch { Vec::new() } else { f.hex("after")? },
                stock: if kind == Kind::Call {
                    f.number("stock")?
                } else {
                    0
                },
            });
        }
        let mut fixups = Vec::new();
        for value in top.list("fixups")? {
            let f = Fields(value, at);
            fixups.push(match f.text("kind")? {
                "abs64" => Fixup::Abs64 {
                    offset: f.number("offset")?,
                    target: f.number("target")?,
                },
                "rel32" => Fixup::Rel32 {
                    offset: f.number("offset")?,
                    target: f.number("target")?,
                },
                "delta" => Fixup::Delta {
                    offset: f.number("offset")?,
                    from: f.number("from")?,
                    to: f.number("to")?,
                },
                "export" => Fixup::Export {
                    offset: f.number("offset")?,
                    dll: f.text("dll")?.to_string(),
                    name: f.text("name")?.to_string(),
                },
                "cell" => Fixup::Cell {
                    offset: f.number("offset")?,
                    end: f.number("end")?,
                    cell: f.text("cell")?.to_string(),
                },
                "edit" => Fixup::Edit {
                    rva: f.number("rva")?,
                    offset: f.number("offset")?,
                    target: f.number("target")?,
                },
                other => return Err(format!("{at}: unknown fixup kind {other}")),
            });
        }
        let cells = top
            .list("cells")?
            .iter()
            .map(|v| {
                let f = Fields(v, at);
                Ok(Cell {
                    name: f.text("name")?.to_string(),
                    offset: f.number("offset")?,
                    bytes: f.number("bytes")?,
                })
            })
            .collect::<Result<_, String>>()?;
        let natives = top
            .list("natives")?
            .iter()
            .map(|v| {
                let f = Fields(v, at);
                Ok(Native {
                    name: f.text("name")?.to_string(),
                    rva: f.number("rva")?,
                })
            })
            .collect::<Result<_, String>>()?;
        let anchors = top
            .list("anchors")?
            .iter()
            .map(|v| {
                let f = Fields(v, at);
                Ok(Anchor {
                    rva: f.number("rva")?,
                    bytes: f.hex("bytes")?,
                })
            })
            .collect::<Result<_, String>>()?;
        let sites = top
            .list("sites")?
            .iter()
            .map(|v| site(v, at))
            .collect::<Result<_, _>>()?;
        let unit = Unit {
            plugin: top.text("plugin")?.to_string(),
            module: top.text("module")?.to_string(),
            source_sha256: top.text("source_sha256")?.to_string(),
            unit_bytes: top.number("unit_bytes")?,
            verified: top
                .text("verified_sha")?
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            name,
            cells,
            writes,
            fixups,
            natives,
            anchors,
            sites,
        };
        unit.check()?;
        Ok(unit)
    }

    /// The shape rules every descriptor keeps, whoever wrote it.
    fn check(&self) -> Result<(), String> {
        let name = &self.name;
        for w in &self.writes {
            let fits = match w.kind {
                Kind::Edit => w.after.len() == w.before.len(),
                _ => w.before.len() == 5 + w.tail.len() && w.entry < self.unit_bytes,
            };
            if !fits || w.before.is_empty() {
                return Err(format!("{name}: the write at {:#x} is malformed", w.rva));
            }
            if w.stock != 0 && !matches!(w.before[0], 0xe8 | 0xe9) {
                return Err(format!(
                    "{name}: the call at {:#x} names a stock function but holds no rel32",
                    w.rva
                ));
            }
        }
        let size = self.unit_bytes;
        for fixup in &self.fixups {
            let (offset, width) = match fixup {
                Fixup::Abs64 { offset, .. } | Fixup::Export { offset, .. } => (*offset, 8),
                Fixup::Rel32 { offset, .. }
                | Fixup::Delta { offset, .. }
                | Fixup::Cell { offset, .. } => (*offset, 4),
                Fixup::Edit { rva, offset, .. } => {
                    let edit = self.writes.iter().find(|w| w.rva == *rva);
                    match edit {
                        Some(w) if w.kind == Kind::Edit && offset + 4 <= w.after.len() => continue,
                        _ => {
                            return Err(format!("{name}: an edit fixup names no edit at {rva:#x}"))
                        }
                    }
                }
            };
            if offset + width > size {
                return Err(format!(
                    "{name}: a fixup at +{offset:#x} lies outside the unit"
                ));
            }
        }
        for cell in &self.cells {
            if cell.offset + cell.bytes > size {
                return Err(format!(
                    "{name}: the cell {} lies outside the unit",
                    cell.name
                ));
            }
        }
        Ok(())
    }

    pub fn cell(&self, name: &str) -> Option<&Cell> {
        self.cells.iter().find(|cell| cell.name == name)
    }

    /// This unit for the build whose image is `image` and whose file hashes to
    /// `sha`: every address moved with the site whose signature holds it, and
    /// what each write replaces read from this build.
    pub fn relocate(&self, image: &[u8], sha: &str) -> Result<Unit, String> {
        let moves = Moves::locate(&self.sites, image).map_err(|e| format!("{}: {e}", self.name))?;
        let at = |rva: usize| moves.at(rva).map_err(|e| format!("{}: {e}", self.name));
        let mut out = self.clone();
        out.source_sha256 = sha.to_string();
        out.verified.clear();
        // A retargeted call keeps reaching what this build's reaches; sites
        // that reached one function must still reach one, the same one.
        let mut stocks: Vec<(usize, usize)> = Vec::new();
        let mut moved = Vec::new();
        for write in &mut out.writes {
            let old = write.rva;
            write.rva = at(old)?;
            write.before = bytes(image, write.rva, write.before.len())?;
            moved.push((old, write.rva));
            if write.stock == 0 {
                continue;
            }
            let rel = i32::from_le_bytes(write.before[1..5].try_into().unwrap()) as isize;
            let reached = write.rva as isize + 5 + rel;
            if reached < 0 || reached as usize >= image.len() {
                return Err(format!(
                    "{}: the branch at {:#x} leaves the module",
                    self.name, write.rva
                ));
            }
            match stocks.iter().find(|&&(from, _)| from == write.stock) {
                Some(&(_, seen)) if seen != reached as usize => {
                    return Err(format!(
                        "{}: the sites that reached {:#x} now reach {seen:#x} and {reached:#x}",
                        self.name, write.stock
                    ))
                }
                Some(_) => {}
                None => stocks.push((write.stock, reached as usize)),
            }
            write.stock = reached as usize;
        }
        for fixup in &mut out.fixups {
            match fixup {
                Fixup::Abs64 { target, .. } | Fixup::Rel32 { target, .. } => *target = at(*target)?,
                Fixup::Delta { from, to, .. } => {
                    *from = at(*from)?;
                    *to = stocks
                        .iter()
                        .find(|&&(old, _)| old == *to)
                        .map(|&(_, new)| new)
                        .ok_or_else(|| {
                            format!("{}: no retargeted call reaches {to:#x}", self.name)
                        })?;
                }
                Fixup::Edit {
                    rva,
                    offset,
                    target,
                } => {
                    let new = moved
                        .iter()
                        .find(|&&(old, _)| old == *rva)
                        .map(|&(_, new)| new)
                        .ok_or_else(|| format!("{}: no edit at {rva:#x}", self.name))?;
                    *rva = new;
                    *target = at(*target)?;
                    let edit = out
                        .writes
                        .iter_mut()
                        .find(|w| w.rva == new && w.kind == Kind::Edit)
                        .ok_or_else(|| format!("{}: no edit at {new:#x}", self.name))?;
                    edit.after[*offset..*offset + 4]
                        .copy_from_slice(&rel32(new + *offset + 4, *target)?);
                }
                Fixup::Export { .. } | Fixup::Cell { .. } => {}
            }
        }
        for native in &mut out.natives {
            native.rva = at(native.rva)?;
        }
        for anchor in &mut out.anchors {
            anchor.rva = at(anchor.rva)?;
            anchor.bytes = bytes(image, anchor.rva, anchor.bytes.len())?;
        }
        for site in &mut out.sites {
            site.start = at(site.start)?;
        }
        Ok(out)
    }

    /// The blob as it runs at `at`, for a module loaded at `base`: every
    /// fixup resolved. `cell` names an imported cell's address, and `export`
    /// a Win32 function's.
    pub fn link(
        &self,
        blob: &[u8],
        base: usize,
        at: usize,
        cell: &dyn Fn(&str) -> Option<usize>,
        export: &dyn Fn(&str, &str) -> Result<usize, String>,
    ) -> Result<Vec<u8>, String> {
        if blob.len() != self.unit_bytes {
            return Err(format!(
                "{}: the blob is not {} bytes",
                self.name, self.unit_bytes
            ));
        }
        let mut code = blob.to_vec();
        let mut put = |offset: usize, value: &[u8]| {
            code[offset..offset + value.len()].copy_from_slice(value);
        };
        for fixup in &self.fixups {
            match fixup {
                Fixup::Abs64 { offset, target } => {
                    put(*offset, &((base + target) as u64).to_le_bytes())
                }
                Fixup::Rel32 { offset, target } => {
                    put(*offset, &rel32(at + offset + 4, base + target)?)
                }
                Fixup::Delta { offset, from, to } => put(*offset, &rel32(*from, *to)?),
                Fixup::Export { offset, dll, name } => {
                    put(*offset, &(export(dll, name)? as u64).to_le_bytes())
                }
                Fixup::Cell {
                    offset,
                    end,
                    cell: name,
                } => {
                    let address =
                        cell(name).ok_or_else(|| format!("{}: no {name} cell", self.name))?;
                    put(*offset, &rel32(at + end, address)?)
                }
                Fixup::Edit { .. } => {}
            }
        }
        Ok(code)
    }

    /// What each write puts in the module at `base` once the unit is at `at`:
    /// (address, what must be there, what is written).
    pub fn placed(&self, base: usize, at: usize) -> Result<Vec<Placed>, String> {
        self.writes
            .iter()
            .map(|w| {
                let address = base + w.rva;
                let after = match w.kind {
                    Kind::Edit => w.after.clone(),
                    Kind::Jmp | Kind::Call => {
                        let op = if w.kind == Kind::Jmp { 0xe9 } else { 0xe8 };
                        let mut after = vec![op];
                        after.extend_from_slice(&rel32(address + 5, at + w.entry)?);
                        after.extend_from_slice(&w.tail);
                        after
                    }
                };
                Ok((address, w.before.clone(), after))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT: &str = r#"{
 "unit_schema": 1, "name": "test-logic", "plugin": "test",
 "module": "logic.dll", "source_sha256": "aa", "unit_bytes": 32, "code_offset": 16,
 "cells": [{"name": "state", "offset": 0, "bytes": 16}],
 "writes": [
  {"kind": "call", "rva": 16, "before": "e8eb000000", "labels": [], "entry": 16, "label": "go", "tail": "", "stock": 0},
  {"kind": "jmp", "rva": 64, "before": "48895c2408", "entry": 20, "label": "back", "tail": ""},
  {"kind": "edit", "rva": 80, "before": "e800000000", "after": "e810000000"}
 ],
 "fixups": [
  {"kind": "rel32", "offset": 17, "target": 256},
  {"kind": "abs64", "offset": 22, "target": 69},
  {"kind": "cell", "offset": 0, "end": 4, "cell": "ring"},
  {"kind": "edit", "rva": 80, "offset": 1, "target": 101}
 ],
 "natives": [{"name": "back", "rva": 64}], "anchors": [], "sites": [], "verified_sha": "bb,cc"
}"#;

    #[test]
    fn a_unit_parses_and_links_where_it_lands() {
        let unit = Unit::parse(UNIT).unwrap();
        assert_eq!(unit.verified, ["bb", "cc"]);
        assert_eq!(unit.cell("state").unwrap().bytes, 16);
        let blob = vec![0u8; 32];
        let (base, at) = (0x1000_0000usize, 0x2000_0000usize);
        let code = unit
            .link(
                &blob,
                base,
                at,
                &|name| (name == "ring").then_some(at + 0x100),
                &|_, _| Err("no exports".into()),
            )
            .unwrap();
        assert_eq!(
            i32::from_le_bytes(code[17..21].try_into().unwrap()) as isize,
            (base + 256) as isize - (at + 21) as isize
        );
        assert_eq!(
            u64::from_le_bytes(code[22..30].try_into().unwrap()),
            base as u64 + 69
        );
        assert_eq!(
            i32::from_le_bytes(code[0..4].try_into().unwrap()),
            0x100 - 4
        );
        let placed = unit.placed(base, at).unwrap();
        assert_eq!(placed[0].2[0], 0xe8);
        assert_eq!(
            i32::from_le_bytes(placed[1].2[1..5].try_into().unwrap()) as isize,
            (at + 20) as isize - (base + 64 + 5) as isize
        );
        assert_eq!(
            placed[2].2,
            bytes(&hex("e810000000").unwrap(), 0, 5).unwrap()
        );
    }

    #[test]
    fn malformed_writes_are_refused() {
        let bad = UNIT.replace(r#""tail": "", "stock": 0"#, r#""tail": "90", "stock": 0"#);
        assert!(Unit::parse(&bad).is_err());
        let outside = UNIT.replace(
            r#""offset": 22, "target": 69"#,
            r#""offset": 28, "target": 69"#,
        );
        assert!(Unit::parse(&outside).is_err());
    }
}
