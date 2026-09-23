//! Build feature-owned patch plans without changing the game's code. All
//! features share the existing payload blocks, preserving their state layout.
//!
//! Preparation takes a feature mask: the set of features the host's accepted
//! plan will install. Every write is still laid out at its fixed offset and
//! every internal block fixup is still resolved, but only the enabled features'
//! module bytes are read and validated. A disabled feature's site may be
//! damaged without blocking the rest; it is not installed, and it is validated
//! when it is installed.
use crate::apply::{resolve_export, Process};
use crate::{GamePatch, Patch, Target};

#[derive(Clone, Debug)]
pub struct Write {
    pub feature: u32,
    pub address: usize,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

/// The features to validate, as a bit per legacy numeric feature ID.
pub type FeatureMask = u64;

/// Whether `feature` is in `mask`.
pub fn mask_has(mask: FeatureMask, feature: u32) -> bool {
    feature < 64 && mask & (1 << feature) != 0
}

/// Every known feature, for a caller that has not been told otherwise.
pub const ALL_FEATURES: FeatureMask = u64::MAX;

/// The highest legacy numeric feature ID a patch can own.
pub const KNOWN_FEATURES: u32 = 10;

pub struct Prepared {
    pub block: usize,
    pub writes: Vec<Write>,
    /// The features whose module bytes were validated here.
    pub validated: FeatureMask,
}

#[link(name = "kernel32")]
extern "system" {
    fn VirtualFree(address: *mut core::ffi::c_void, size: usize, kind: u32) -> i32;
    fn GetCurrentProcessId() -> u32;
}

impl Drop for Prepared {
    fn drop(&mut self) {
        unsafe { VirtualFree(self.block as *mut _, 0, 0x8000) };
    }
}

fn branch(op: u8, from: usize, to: usize, length: usize) -> Result<Vec<u8>, String> {
    if length < 5 {
        return Err("branch span is shorter than five bytes".into());
    }
    let delta = i32::try_from(to as i128 - from as i128 - 5)
        .map_err(|_| "payload is outside rel32 reach")?;
    let mut bytes = vec![op];
    bytes.extend_from_slice(&delta.to_le_bytes());
    bytes.resize(length, 0x90);
    Ok(bytes)
}

fn put(code: &mut [u8], offset: usize, bytes: &[u8]) -> Result<(), String> {
    let end = offset.checked_add(bytes.len()).ok_or("fixup overflow")?;
    code.get_mut(offset..end)
        .ok_or("fixup outside payload")?
        .copy_from_slice(bytes);
    Ok(())
}

fn validate(
    target: &Target,
    anchor_rva: usize,
    anchor: &[u8],
    sha: &str,
) -> Result<Process, String> {
    if target.process_id != unsafe { GetCurrentProcessId() } {
        return Err("feature plans are for the current process only".into());
    }
    if !target.path.as_os_str().is_empty()
        && crate::sha256::file(&target.path).map_err(|e| e.to_string())? != sha
    {
        return Err("module build was not validated for this payload".into());
    }
    if anchor_rva
        .checked_add(anchor.len())
        .is_none_or(|end| end > target.size)
    {
        return Err("anchor outside module".into());
    }
    let process = Process::open(target.process_id)?;
    if process.read(unsafe { target.base.add(anchor_rva) }, anchor.len())? != anchor {
        return Err("module anchor mismatch".into());
    }
    Ok(process)
}

fn finish(
    process: &Process,
    target: &Target,
    mut plan: Prepared,
    code: &[u8],
    enabled: FeatureMask,
) -> Result<Prepared, String> {
    let start = target.base as usize;
    // The block is laid out whole and every write keeps its fixed offset. Only
    // an enabled feature's writes need a valid, non-overlapping span here; the
    // bytes at each site are validated when that feature installs, so one
    // feature's damaged site refuses only that feature instead of failing the
    // whole preparation.
    let mut checked: Vec<&Write> = Vec::new();
    for write in &plan.writes {
        if !mask_has(enabled, write.feature) {
            continue;
        }
        if write.before.len() != write.after.len()
            || write.before.is_empty()
            || write.address < start
            || write
                .address
                .checked_add(write.before.len())
                .is_none_or(|end| end > start + target.size)
        {
            return Err("invalid patch span".into());
        }
        for other in &checked {
            if write.address < other.address + other.before.len()
                && other.address < write.address + write.before.len()
            {
                return Err("feature plans overlap".into());
            }
        }
        checked.push(write);
    }
    process.write(plan.block as *mut u8, code)?;
    plan.validated = enabled;
    Ok(plan)
}

/// Build the same bytes as apply(), partitioned by descriptor feature IDs.
pub fn prepare_logic(patch: &Patch, target: &Target, payload: &[u8]) -> Result<Prepared, String> {
    prepare_logic_enabled(patch, target, payload, ALL_FEATURES)
}

/// As `prepare_logic`, but only the features in `enabled` are validated.
pub fn prepare_logic_enabled(
    patch: &Patch,
    target: &Target,
    payload: &[u8],
    enabled: FeatureMask,
) -> Result<Prepared, String> {
    let process = validate(
        target,
        patch.anchor_rva,
        &patch.anchor,
        &patch.source_sha256,
    )?;
    if payload.len() > patch.cursor_offset || patch.cursor_offset + 4 > patch.block_bytes {
        return Err("payload does not fit its block".into());
    }
    let block = process.reserve_near(target.base, patch.block_bytes)? as usize;
    let mut plan = Prepared {
        block,
        writes: Vec::new(),
        validated: 0,
    };
    let base = target.base as usize;
    let mut code = payload.to_vec();
    for fix in &patch.trace_fixups {
        put(
            &mut code,
            fix.offset,
            &((base + fix.target_rva) as u64).to_le_bytes(),
        )?;
    }
    for fix in &patch.rel_fixups {
        let delta =
            i32::try_from((base + fix.target_rva) as i128 - (block + fix.offset + 4) as i128)
                .map_err(|_| "payload branch outside reach")?;
        put(&mut code, fix.offset, &delta.to_le_bytes())?;
    }
    for fix in &patch.delta_fixups {
        let delta = i32::try_from(fix.to as i128 - fix.from as i128)
            .map_err(|_| "pose delta outside reach")?;
        put(&mut code, fix.offset, &delta.to_le_bytes())?;
    }
    // Pickup=1, selection=2, movement=3; other ownership comes from assembly metadata.
    for (feature, rva, before, entry) in [
        (1, patch.call_site, &patch.call_before, 0),
        (
            3,
            patch.move_call_site,
            &patch.move_displaced,
            patch.move_offset,
        ),
    ] {
        plan.writes.push(Write {
            feature,
            address: base + rva,
            before: before.clone(),
            after: branch(0xe8, base + rva, block + entry, before.len())?,
        });
    }
    for call in &patch.pose_calls {
        let mut after = branch(0xe8, base + call.rva, block + call.entry, call.before.len())?;
        if after.len() != 5 + call.tail.len() {
            return Err("call tail length mismatch".into());
        }
        after[5..].copy_from_slice(&call.tail);
        plan.writes.push(Write {
            feature: call.feature,
            address: base + call.rva,
            before: call.before.clone(),
            after,
        });
    }
    for hook in &patch.detours {
        plan.writes.push(Write {
            feature: hook.feature,
            address: base + hook.rva,
            before: hook.displaced.clone(),
            after: branch(
                0xe9,
                base + hook.rva,
                block + hook.entry,
                hook.displaced.len(),
            )?,
        });
    }
    for (rva, before, after) in [
        (
            patch.select_is_rva,
            &patch.select_is_before,
            &patch.select_is_after,
        ),
        (
            patch.select_squad_rva,
            &patch.select_squad_before,
            &patch.select_squad_after,
        ),
        (
            patch.select_toggle_rva,
            &patch.select_toggle_before,
            &patch.select_toggle_after,
        ),
        (
            patch.select_type_rva,
            &patch.select_type_before,
            &patch.select_type_after,
        ),
    ] {
        // A zero-length edit is the descriptor's disabled legacy getter edit.
        if !before.is_empty() {
            plan.writes.push(Write {
                feature: 2,
                address: base + rva,
                before: before.clone(),
                after: after.clone(),
            });
        }
    }
    finish(&process, target, plan, &code, enabled)
}

pub fn prepare_game(
    patch: &GamePatch,
    target: &Target,
    payload: &[u8],
) -> Result<Prepared, String> {
    prepare_game_enabled(patch, target, payload, ALL_FEATURES)
}

/// As `prepare_game`, but only the features in `enabled` are validated.
pub fn prepare_game_enabled(
    patch: &GamePatch,
    target: &Target,
    payload: &[u8],
    enabled: FeatureMask,
) -> Result<Prepared, String> {
    let process = validate(
        target,
        patch.anchor_rva,
        &patch.anchor,
        &patch.source_sha256,
    )?;
    if payload.len() > patch.block_bytes {
        return Err("game payload exceeds block".into());
    }
    let block = process.reserve_near(target.base, patch.block_bytes)? as usize;
    let mut plan = Prepared {
        block,
        writes: Vec::new(),
        validated: 0,
    };
    let base = target.base as usize;
    let mut code = payload.to_vec();
    for fix in &patch.fixups {
        put(
            &mut code,
            fix.offset,
            &((base + fix.target_rva) as u64).to_le_bytes(),
        )?;
    }
    for export in &patch.exports {
        let address = resolve_export(&export.dll, &export.name)? as u64;
        put(&mut code, export.offset, &address.to_le_bytes())?;
    }
    for hook in &patch.hooks {
        plan.writes.push(Write {
            feature: hook.feature,
            address: base + hook.rva,
            before: hook.displaced.clone(),
            after: branch(
                0xe9,
                base + hook.rva,
                block + hook.entry,
                hook.displaced.len(),
            )?,
        });
    }
    finish(&process, target, plan, &code, enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_feature_mask_covers_exactly_its_bits() {
        let mask = (1 << 2) | (1 << 5);
        assert!(mask_has(mask, 2));
        assert!(mask_has(mask, 5));
        assert!(!mask_has(mask, 1));
        assert!(!mask_has(mask, 6));
        assert!(mask_has(ALL_FEATURES, 1) && mask_has(ALL_FEATURES, 9));
        assert!(!mask_has(ALL_FEATURES, 64));
    }
}
