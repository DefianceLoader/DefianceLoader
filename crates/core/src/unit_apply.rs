//! Install assembled units in a local or remote process. Both modules are
//! checked and linked before any hook is written; mutable cells stay private
//! to the target, and repeat runs verify the installed code before reusing it.

use crate::apply::Process;
use crate::sha256::Sha256;
use crate::unit::{Kind, Placed, Unit};
use crate::Target;

const HEADER_BYTES: usize = 32;
const TRACE_RING_BYTES: usize = 0x10 + 32 * 16;

pub struct Payload<'a> {
    pub unit: &'a Unit,
    pub code: &'a [u8],
}

pub struct Module<'a> {
    pub target: &'a Target,
    pub payloads: &'a [Payload<'a>],
}

struct Layout {
    offsets: Vec<usize>,
    ring: usize,
    bytes: usize,
    identity: [u8; HEADER_BYTES],
}

fn layout(payloads: &[Payload<'_>]) -> Result<Layout, String> {
    let mut hash = Sha256::new();
    hash.update(b"Defiance injector units v1");
    let mut end = HEADER_BYTES;
    let mut offsets = Vec::new();
    for payload in payloads {
        if payload.code.len() != payload.unit.unit_bytes {
            return Err(format!("{}: payload length differs", payload.unit.name));
        }
        offsets.push(end);
        end = end
            .checked_add(payload.code.len())
            .and_then(|v| v.checked_add(15))
            .ok_or("unit allocation size overflow")?
            & !15;
        hash.update(format!("{:?}", payload.unit).as_bytes());
        hash.update(payload.code);
    }
    Ok(Layout {
        offsets,
        ring: end,
        bytes: end + TRACE_RING_BYTES,
        identity: hash.finish(),
    })
}

/// Check anchors, write bounds and disjoint spans against a pristine image.
pub fn validate(payloads: &[Payload<'_>], image: &[u8]) -> Result<(), String> {
    spans(payloads, image.len())?;
    for payload in payloads {
        for (rva, before) in payload
            .unit
            .anchors
            .iter()
            .map(|a| (a.rva, &a.bytes))
            .chain(payload.unit.writes.iter().map(|w| (w.rva, &w.before)))
        {
            if image.get(rva..rva + before.len()) != Some(before.as_slice()) {
                return Err(format!(
                    "{}: unexpected bytes at {rva:#x}",
                    payload.unit.name
                ));
            }
        }
    }
    layout(payloads)?;
    Ok(())
}

fn spans(payloads: &[Payload<'_>], size: usize) -> Result<(), String> {
    let mut spans = Vec::new();
    for payload in payloads {
        for write in &payload.unit.writes {
            let end = write
                .rva
                .checked_add(write.before.len())
                .ok_or("write overflow")?;
            if end > size {
                return Err(format!("{}: write outside the module", payload.unit.name));
            }
            spans.push((write.rva, end));
        }
        for anchor in &payload.unit.anchors {
            if anchor
                .rva
                .checked_add(anchor.bytes.len())
                .is_none_or(|end| end > size)
            {
                return Err(format!("{}: anchor outside the module", payload.unit.name));
            }
        }
    }
    spans.sort_unstable();
    if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err("unit writes overlap".into());
    }
    Ok(())
}

fn linked(
    module: &Module<'_>,
    layout: &Layout,
    block: usize,
    export: &dyn Fn(&str, &str) -> Result<usize, String>,
    initialize: &dyn Fn(&Unit, &mut [u8]) -> Result<(), String>,
) -> Result<(Vec<u8>, Vec<Placed>), String> {
    let mut code = vec![0; layout.bytes];
    code[..HEADER_BYTES].copy_from_slice(&layout.identity);
    let mut writes = Vec::new();
    for (payload, &offset) in module.payloads.iter().zip(&layout.offsets) {
        let mut unit = payload.unit.link(
            payload.code,
            module.target.base as usize,
            block + offset,
            &|name| (name == "trace_ring").then_some(block + layout.ring),
            export,
        )?;
        initialize(payload.unit, &mut unit)?;
        code[offset..offset + unit.len()].copy_from_slice(&unit);
        writes.extend(
            payload
                .unit
                .placed(module.target.base as usize, block + offset)?,
        );
    }
    Ok((code, writes))
}

/// Recover this installer's allocation from one unit hook. The identity and
/// all hook bytes must agree, so another injector or loader is never reused.
pub fn installed_block(process: &Process, module: &Module<'_>) -> Result<Option<usize>, String> {
    spans(module.payloads, module.target.size)?;
    let layout = layout(module.payloads)?;
    for (payload, &offset) in module.payloads.iter().zip(&layout.offsets) {
        for write in &payload.unit.writes {
            if write.kind == Kind::Edit {
                continue;
            }
            let address = module.target.base as usize + write.rva;
            let now = process.read(address as *const u8, write.before.len())?;
            if now == write.before {
                continue;
            }
            let op = if write.kind == Kind::Call { 0xe8 } else { 0xe9 };
            if now.len() < 5 || now[0] != op {
                return Err("a unit site is modified; restart the game".into());
            }
            let disp = i32::from_le_bytes(now[1..5].try_into().unwrap()) as isize;
            let block = (address + 5)
                .checked_add_signed(disp)
                .and_then(|entry| entry.checked_sub(write.entry + offset))
                .ok_or("invalid installed unit branch")?;
            if process.read(block as *const u8, HEADER_BYTES)? != layout.identity {
                return Err("unit hooks belong to another patch; restart the game".into());
            }
            for (payload, &offset) in module.payloads.iter().zip(&layout.offsets) {
                for (address, _, after) in payload
                    .unit
                    .placed(module.target.base as usize, block + offset)?
                {
                    if process.read(address as *const u8, after.len())? != after {
                        return Err("unit hooks are partially installed; restart the game".into());
                    }
                }
            }
            return Ok(Some(block));
        }
    }
    Ok(None)
}

/// The address of an installed unit's named cell (or the shared trace ring).
pub fn cell(module: &Module<'_>, block: usize, unit: &str, name: &str) -> Option<usize> {
    let layout = layout(module.payloads).ok()?;
    if name == "trace_ring" {
        return Some(block + layout.ring);
    }
    module
        .payloads
        .iter()
        .zip(layout.offsets)
        .find_map(|(payload, offset)| {
            (payload.unit.name == unit)
                .then(|| {
                    payload
                        .unit
                        .cell(name)
                        .map(|cell| block + offset + cell.offset)
                })
                .flatten()
        })
}

struct Prepared {
    block: usize,
    code: Vec<u8>,
    writes: Vec<Placed>,
    fresh: bool,
}

/// Apply every module as one transaction. Failed hook writes are restored;
/// payload allocations are retained if restoration fails, so live branches
/// never point into freed memory. `export` resolves addresses in the target.
pub fn apply(
    modules: &[Module<'_>],
    export: &dyn Fn(&str, &str) -> Result<usize, String>,
    initialize: &dyn Fn(&Unit, &mut [u8]) -> Result<(), String>,
) -> Result<&'static str, String> {
    let first = modules.first().ok_or("no modules to patch")?;
    let pid = first.target.process_id;
    if modules.iter().any(|m| m.target.process_id != pid) {
        return Err("modules belong to different processes".into());
    }
    let mut ranges: Vec<_> = modules
        .iter()
        .map(|module| {
            let base = module.target.base as usize;
            base.checked_add(module.target.size)
                .map(|end| (base, end))
                .ok_or("module address overflow")
        })
        .collect::<Result<_, _>>()?;
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err("module address ranges overlap".into());
    }
    let process = Process::open(pid)?;
    let mut prepared: Vec<Prepared> = Vec::new();
    let result = (|| {
        // Preflight all modules before reserving or writing a payload.
        let mut layouts = Vec::new();
        for module in modules {
            spans(module.payloads, module.target.size)?;
            if module.target.path.exists() {
                let sha = crate::sha256::file(&module.target.path).map_err(|e| e.to_string())?;
                if module.payloads.iter().any(|p| p.unit.source_sha256 != sha) {
                    return Err("module hash differs from its units".into());
                }
            }
            let layout = layout(module.payloads)?;
            let installed = installed_block(&process, module)?;
            for payload in module.payloads {
                for anchor in &payload.unit.anchors {
                    let address = (module.target.base as usize + anchor.rva) as *const u8;
                    if process.read(address, anchor.bytes.len())? != anchor.bytes {
                        return Err(format!("{}: module anchor differs", payload.unit.name));
                    }
                }
                if installed.is_none() {
                    for write in &payload.unit.writes {
                        let address = (module.target.base as usize + write.rva) as *const u8;
                        if process.read(address, write.before.len())? != write.before {
                            return Err(format!(
                                "{}: site {:#x} differs",
                                payload.unit.name, write.rva
                            ));
                        }
                    }
                }
            }
            layouts.push((layout, installed));
        }
        for (module, (layout, installed)) in modules.iter().zip(layouts) {
            let block = match installed {
                Some(block) => block,
                None => process.reserve_near(module.target.base, layout.bytes)? as usize,
            };
            // Register fresh allocations before linking, so failures release them.
            prepared.push(Prepared {
                block,
                code: Vec::new(),
                writes: Vec::new(),
                fresh: installed.is_none(),
            });
            let (code, writes) = linked(module, &layout, block, export, initialize)?;
            if installed.is_some() {
                let mut now = process.read(block as *const u8, code.len())?;
                // Hooks mutate their named cells and the shared ring, never code.
                now[layout.ring..].copy_from_slice(&code[layout.ring..]);
                for (payload, offset) in module.payloads.iter().zip(layout.offsets) {
                    for cell in &payload.unit.cells {
                        let range = offset + cell.offset..offset + cell.offset + cell.bytes;
                        now[range.clone()].copy_from_slice(&code[range]);
                    }
                }
                if now != code {
                    return Err("installed unit code differs; restart the game".into());
                }
                for (address, _, after) in &writes {
                    if process.read(*address as *const u8, after.len())? != *after {
                        return Err("unit hooks are partially installed; restart the game".into());
                    }
                }
            }
            let last = prepared.last_mut().unwrap();
            last.code = code;
            last.writes = writes;
        }
        for p in prepared.iter().filter(|p| p.fresh) {
            process.write(p.block as *mut u8, &p.code)?;
            if process.read(p.block as *const u8, p.code.len())? != p.code {
                return Err("unit payload write did not take".into());
            }
        }
        let mut changed: Vec<&Placed> = Vec::new();
        for p in prepared.iter().filter(|p| p.fresh) {
            for write in &p.writes {
                changed.push(write);
                let result = process.write(write.0 as *mut u8, &write.2).and_then(|()| {
                    (process.read(write.0 as *const u8, write.2.len())? == write.2)
                        .then_some(())
                        .ok_or("unit hook write did not take".into())
                });
                if let Err(error) = result {
                    let mut restored = true;
                    for write in changed.iter().rev() {
                        restored &= process
                            .write(write.0 as *mut u8, &write.1)
                            .and_then(|()| process.read(write.0 as *const u8, write.1.len()))
                            .is_ok_and(|bytes| bytes == write.1);
                    }
                    if !restored {
                        for p in &mut prepared {
                            p.fresh = false;
                        }
                        return Err(format!("{error}; rollback failed; close the game"));
                    }
                    return Err(error);
                }
            }
        }
        Ok(if prepared.iter().any(|p| p.fresh) {
            "patched"
        } else {
            "already patched; nothing to do"
        })
    })();
    if result.is_err() {
        for p in prepared.iter().filter(|p| p.fresh) {
            process.release(p.block as *mut u8);
        }
    }
    result
}
