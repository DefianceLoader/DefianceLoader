//! Finding the patch sites in a build the patch was not written for.
//!
//! A game update usually recompiles everything, which moves every function
//! even where the code the patch touches is unchanged. Each site carries a
//! signature (tools/sigs.py): its bytes, with every field that encodes a
//! distance to other code wildcarded, so that the struct offsets and vtable
//! slots inside it still have to match. Here each signature must be found in
//! the running module exactly once, the sites of one function must all have
//! moved by the same distance, and every address the patch uses is moved with
//! the site whose signature holds it. Anything else is refused, and the patch
//! is then not applied at all.

use crate::descriptor::{fields, GamePatch, Patch};
use crate::pe::map_file;
use crate::scan::{Moves, Site};

/// A rel32 inside an in-place edit, re-aimed once its target has moved.
#[derive(Clone)]
pub struct EditFixup {
    pub rva: usize,
    pub offset: usize,
    pub target: usize,
}

pub fn parse_sites(text: &str) -> Vec<Site> {
    let number = |name: &str| -> Vec<usize> {
        fields(text, name)
            .iter()
            .map(|v| v.parse().expect(name))
            .collect()
    };
    fields(text, "site_name")
        .into_iter()
        .zip(number("site_start"))
        .zip(number("site_group"))
        .zip(fields(text, "site_pattern"))
        .map(|(((name, start), group), pattern)| Site {
            name,
            start,
            group,
            pattern: (0..pattern.len() / 2)
                .map(|i| match &pattern[i * 2..i * 2 + 2] {
                    "??" => None,
                    pair => Some(u8::from_str_radix(pair, 16).expect("bad signature")),
                })
                .collect(),
        })
        .collect()
}

/// The other builds whose signatures have been checked: one comma-separated
/// string of sha256s, since the descriptor parser reads strings, not arrays.
pub fn parse_verified(text: &str) -> Vec<String> {
    fields(text, "verified_sha")
        .first()
        .map(|list| {
            list.split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_edit_fixups(text: &str) -> Vec<EditFixup> {
    let number = |name: &str| -> Vec<usize> {
        fields(text, name)
            .iter()
            .map(|v| v.parse().expect(name))
            .collect()
    };
    number("edit_rva")
        .into_iter()
        .zip(number("edit_offset"))
        .zip(number("edit_target"))
        .map(|((rva, offset), target)| EditFixup {
            rva,
            offset,
            target,
        })
        .collect()
}

fn bytes(image: &[u8], rva: usize, len: usize) -> Result<Vec<u8>, String> {
    image
        .get(rva..rva + len)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| format!("{rva:#x} is outside the module"))
}

fn rel32(from_end: usize, to: usize) -> [u8; 4] {
    ((to as isize - from_end as isize) as i32).to_le_bytes()
}

/// Whether `enabled` asks for every known feature, in which case relocation is
/// the original all-site walk and its bytes are unchanged.
fn all_known(enabled: crate::features::FeatureMask) -> bool {
    (1..=crate::features::KNOWN_FEATURES).all(|feature| crate::features::mask_has(enabled, feature))
}

/// Whether a feature-tagged element is required: feature 0 is shared code that
/// more than one feature reaches, so it is always kept.
fn wanted(enabled: crate::features::FeatureMask, feature: u32) -> bool {
    feature == 0 || crate::features::mask_has(enabled, feature)
}

fn site_covers(site: &Site, address: usize) -> bool {
    site.start <= address && address < site.start + site.pattern.len()
}

/// The sites a selective relocation has to find: those whose window holds any
/// required address. A site referenced only by a disabled feature is left out.
fn select_sites(sites: &[Site], required: &[usize]) -> Vec<Site> {
    sites
        .iter()
        .filter(|site| required.iter().any(|address| site_covers(site, *address)))
        .cloned()
        .collect()
}

impl Patch {
    /// This patch, for the build whose image is `image` and whose file hashes
    /// to `sha`. What each site must hold before patching is read from this
    /// build (its signature has vouched for it); what is written after is the
    /// same, with each rel32 re-aimed.
    pub fn relocate(&self, image: &[u8], sha: &str) -> Result<(Patch, Moves), String> {
        let moves = Moves::locate(&self.sites, image)?;
        let mut out = self.clone();
        out.source_sha256 = sha.to_string();
        out.image_bytes = image.len();
        out.stock_chooser = moves.at(self.stock_chooser)?;
        out.call_site = moves.at(self.call_site)?;
        out.call_before = [&[0xe8u8][..], &rel32(out.call_site + 5, out.stock_chooser)].concat();
        out.move_call_site = moves.at(self.move_call_site)?;
        out.move_displaced = bytes(image, out.move_call_site, self.move_displaced.len())?;
        out.anchor_rva = moves.at(self.anchor_rva)?;
        out.anchor = bytes(image, out.anchor_rva, self.anchor.len())?;
        for hook in &mut out.detours {
            hook.rva = moves.at(hook.rva)?;
            hook.displaced = bytes(image, hook.rva, hook.displaced.len())?;
        }
        for fix in &mut out.trace_fixups {
            fix.target_rva = moves.at(fix.target_rva)?;
        }
        for fix in &mut out.rel_fixups {
            fix.target_rva = moves.at(fix.target_rva)?;
        }
        // A retargeted call or jmp keeps reaching what this build's reaches, so
        // the pose split calls the stock function the site would have. Sites
        // that reached one function must still reach one, the same one.
        let mut stocks: Vec<(usize, usize)> = Vec::new();
        for call in &mut out.pose_calls {
            call.rva = moves.at(call.rva)?;
            call.before = bytes(image, call.rva, call.before.len())?;
            if call.stock == 0 {
                continue;
            }
            let rel = i32::from_le_bytes(call.before[1..5].try_into().unwrap()) as isize;
            let reached = call.rva as isize + 5 + rel;
            if reached < 0 || reached as usize >= image.len() {
                return Err(format!("the branch at {:#x} leaves the module", call.rva));
            }
            match stocks.iter().find(|&&(old, _)| old == call.stock) {
                Some(&(_, seen)) if seen != reached as usize => {
                    return Err(format!(
                        "the sites that reached {:#x} now reach {seen:#x} and {reached:#x}",
                        call.stock
                    ))
                }
                Some(_) => {}
                None => stocks.push((call.stock, reached as usize)),
            }
            call.stock = reached as usize;
        }
        for fix in &mut out.delta_fixups {
            fix.from = moves.at(fix.from)?;
            fix.to = stocks
                .iter()
                .find(|&&(old, _)| old == fix.to)
                .map(|&(_, new)| new)
                .ok_or_else(|| format!("no retargeted site reaches {:#x}", fix.to))?;
        }
        let edits = [
            (
                &mut out.select_is_rva,
                &mut out.select_is_before,
                &mut out.select_is_after,
            ),
            (
                &mut out.select_squad_rva,
                &mut out.select_squad_before,
                &mut out.select_squad_after,
            ),
            (
                &mut out.select_toggle_rva,
                &mut out.select_toggle_before,
                &mut out.select_toggle_after,
            ),
            (
                &mut out.select_type_rva,
                &mut out.select_type_before,
                &mut out.select_type_after,
            ),
        ];
        for (rva, before, after) in edits {
            let old = *rva;
            *rva = moves.at(old)?;
            *before = bytes(image, *rva, before.len())?;
            for fix in self.edit_fixups.iter().filter(|f| f.rva == old) {
                let field = &mut after[fix.offset..fix.offset + 4];
                field.copy_from_slice(&rel32(*rva + fix.offset + 4, moves.at(fix.target)?));
            }
        }
        Ok((out, moves))
    }
}

impl GamePatch {
    /// As for logic.dll: the same patch, for the build in `image`.
    pub fn relocate(&self, image: &[u8], sha: &str) -> Result<(GamePatch, Moves), String> {
        let moves = Moves::locate(&self.sites, image)?;
        let mut out = self.clone();
        out.source_sha256 = sha.to_string();
        out.image_bytes = image.len();
        out.anchor_rva = moves.at(self.anchor_rva)?;
        out.anchor = bytes(image, out.anchor_rva, self.anchor.len())?;
        for hook in &mut out.hooks {
            hook.rva = moves.at(hook.rva)?;
            hook.displaced = bytes(image, hook.rva, hook.displaced.len())?;
        }
        for fix in &mut out.fixups {
            fix.target_rva = moves.at(fix.target_rva)?;
        }
        Ok((out, moves))
    }
}

// --- selective relocation ----------------------------------------------------

impl Patch {
    /// The addresses a selective relocation must be able to resolve for
    /// `enabled`: every module address its writes, sites, retargeted calls and
    /// fixups name, plus the always-required anchor. Shared elements (feature
    /// 0) are always included.
    fn required_addresses(&self, enabled: crate::features::FeatureMask) -> Vec<usize> {
        let mut required = vec![self.anchor_rva];
        if crate::features::mask_has(enabled, 1) {
            required.push(self.stock_chooser);
            required.push(self.call_site);
        }
        if crate::features::mask_has(enabled, 3) {
            required.push(self.move_call_site);
        }
        for hook in &self.detours {
            if wanted(enabled, hook.feature) {
                required.push(hook.rva);
            }
        }
        for fix in &self.trace_fixups {
            if wanted(enabled, fix.feature) {
                required.push(fix.target_rva);
            }
        }
        for fix in &self.rel_fixups {
            if wanted(enabled, fix.feature) {
                required.push(fix.target_rva);
            }
        }
        for call in &self.pose_calls {
            if wanted(enabled, call.feature) {
                required.push(call.rva);
            }
        }
        if wanted(enabled, 4) {
            for fix in &self.delta_fixups {
                required.push(fix.from);
            }
        }
        if wanted(enabled, 2) {
            required.extend([
                self.select_is_rva,
                self.select_squad_rva,
                self.select_toggle_rva,
                self.select_type_rva,
            ]);
            for fix in &self.edit_fixups {
                required.push(fix.target);
            }
        }
        required
    }

    /// Whether every site this one feature needs can be found in `image`. The
    /// host uses it to drop a feature whose signature is missing, and keep the
    /// rest, instead of refusing the whole module.
    pub fn feature_relocatable(&self, image: &[u8], feature: u32) -> bool {
        let required = self.required_addresses(1u64 << feature);
        Moves::locate(&select_sites(&self.sites, &required), image).is_ok()
    }

    /// As `relocate`, but only the parts the accepted plan needs. Sites whose
    /// window holds nothing an enabled feature references are not scanned, and
    /// disabled features' addresses are left as they are; they are never
    /// installed. With every known feature enabled this is exactly `relocate`.
    ///
    /// A shared region (feature 0) is always required, so a helper reached by
    /// more than one feature is never dropped because its nominal owner is off.
    pub fn relocate_selected(
        &self,
        image: &[u8],
        sha: &str,
        enabled: crate::features::FeatureMask,
    ) -> Result<(Patch, Moves), String> {
        if all_known(enabled) {
            return self.relocate(image, sha);
        }
        let required = self.required_addresses(enabled);
        let moves = Moves::locate(&select_sites(&self.sites, &required), image)?;
        let mut out = self.clone();
        out.source_sha256 = sha.to_string();
        out.image_bytes = image.len();
        out.anchor_rva = moves.at(self.anchor_rva)?;
        out.anchor = bytes(image, out.anchor_rva, self.anchor.len())?;
        if crate::features::mask_has(enabled, 1) {
            out.stock_chooser = moves.at(self.stock_chooser)?;
            out.call_site = moves.at(self.call_site)?;
            out.call_before =
                [&[0xe8u8][..], &rel32(out.call_site + 5, out.stock_chooser)].concat();
        }
        if crate::features::mask_has(enabled, 3) {
            out.move_call_site = moves.at(self.move_call_site)?;
            out.move_displaced = bytes(image, out.move_call_site, self.move_displaced.len())?;
        }
        for hook in &mut out.detours {
            if !wanted(enabled, hook.feature) {
                continue;
            }
            hook.rva = moves.at(hook.rva)?;
            hook.displaced = bytes(image, hook.rva, hook.displaced.len())?;
        }
        for fix in &mut out.trace_fixups {
            if wanted(enabled, fix.feature) {
                fix.target_rva = moves.at(fix.target_rva)?;
            }
        }
        for fix in &mut out.rel_fixups {
            if wanted(enabled, fix.feature) {
                fix.target_rva = moves.at(fix.target_rva)?;
            }
        }
        let mut stocks: Vec<(usize, usize)> = Vec::new();
        for call in &mut out.pose_calls {
            if !wanted(enabled, call.feature) {
                continue;
            }
            call.rva = moves.at(call.rva)?;
            call.before = bytes(image, call.rva, call.before.len())?;
            if call.stock == 0 {
                continue;
            }
            let rel = i32::from_le_bytes(call.before[1..5].try_into().unwrap()) as isize;
            let reached = call.rva as isize + 5 + rel;
            if reached < 0 || reached as usize >= image.len() {
                return Err(format!("the branch at {:#x} leaves the module", call.rva));
            }
            match stocks.iter().find(|&&(old, _)| old == call.stock) {
                Some(&(_, seen)) if seen != reached as usize => {
                    return Err(format!(
                        "the sites that reached {:#x} now reach {seen:#x} and {reached:#x}",
                        call.stock
                    ))
                }
                Some(_) => {}
                None => stocks.push((call.stock, reached as usize)),
            }
            call.stock = reached as usize;
        }
        if wanted(enabled, 4) {
            for fix in &mut out.delta_fixups {
                fix.from = moves.at(fix.from)?;
                fix.to = stocks
                    .iter()
                    .find(|&&(old, _)| old == fix.to)
                    .map(|&(_, new)| new)
                    .ok_or_else(|| format!("no retargeted site reaches {:#x}", fix.to))?;
            }
        }
        if wanted(enabled, 2) {
            let edits = [
                (
                    &mut out.select_is_rva,
                    &mut out.select_is_before,
                    &mut out.select_is_after,
                ),
                (
                    &mut out.select_squad_rva,
                    &mut out.select_squad_before,
                    &mut out.select_squad_after,
                ),
                (
                    &mut out.select_toggle_rva,
                    &mut out.select_toggle_before,
                    &mut out.select_toggle_after,
                ),
                (
                    &mut out.select_type_rva,
                    &mut out.select_type_before,
                    &mut out.select_type_after,
                ),
            ];
            for (rva, before, after) in edits {
                let old = *rva;
                *rva = moves.at(old)?;
                *before = bytes(image, *rva, before.len())?;
                for fix in self.edit_fixups.iter().filter(|f| f.rva == old) {
                    let field = &mut after[fix.offset..fix.offset + 4];
                    field.copy_from_slice(&rel32(*rva + fix.offset + 4, moves.at(fix.target)?));
                }
            }
        }
        Ok((out, moves))
    }
}

impl GamePatch {
    /// The addresses a selective game.dll relocation must resolve for `enabled`.
    fn required_addresses(&self, enabled: crate::features::FeatureMask) -> Vec<usize> {
        let mut required = vec![self.anchor_rva];
        for hook in &self.hooks {
            if wanted(enabled, hook.feature) {
                required.push(hook.rva);
            }
        }
        for fix in &self.fixups {
            if wanted(enabled, fix.feature) {
                required.push(fix.target_rva);
            }
        }
        required
    }

    /// Whether every site this one feature needs can be found in `image`.
    pub fn feature_relocatable(&self, image: &[u8], feature: u32) -> bool {
        let required = self.required_addresses(1u64 << feature);
        Moves::locate(&select_sites(&self.sites, &required), image).is_ok()
    }

    /// As `Patch::relocate_selected`, for the game.dll half.
    pub fn relocate_selected(
        &self,
        image: &[u8],
        sha: &str,
        enabled: crate::features::FeatureMask,
    ) -> Result<(GamePatch, Moves), String> {
        if all_known(enabled) {
            return self.relocate(image, sha);
        }
        let required = self.required_addresses(enabled);
        let moves = Moves::locate(&select_sites(&self.sites, &required), image)?;
        let mut out = self.clone();
        out.source_sha256 = sha.to_string();
        out.image_bytes = image.len();
        out.anchor_rva = moves.at(self.anchor_rva)?;
        out.anchor = bytes(image, out.anchor_rva, self.anchor.len())?;
        for hook in &mut out.hooks {
            if !wanted(enabled, hook.feature) {
                continue;
            }
            hook.rva = moves.at(hook.rva)?;
            hook.displaced = bytes(image, hook.rva, hook.displaced.len())?;
        }
        for fix in &mut out.fixups {
            if wanted(enabled, fix.feature) {
                fix.target_rva = moves.at(fix.target_rva)?;
            }
        }
        Ok((out, moves))
    }
}

// --- the self test -----------------------------------------------------------

/// A stand-in image holding each site's signature, its wildcards filled with
/// 0xcc, every function moved by a distance of its own: the functions keep
/// their order and each is moved 0x100 further than the one before. Sites
/// whose windows overlap or touch move together, since the bytes they share
/// can only be in one place. Returns the image and each group's distance.
fn plant(sites: &[Site], size: usize) -> Result<(Vec<u8>, impl Fn(usize) -> isize), String> {
    // which sites move together: one function's, and any whose windows meet
    let mut cluster: Vec<usize> = (0..sites.len()).collect();
    fn root(cluster: &[usize], mut i: usize) -> usize {
        while cluster[i] != i {
            i = cluster[i];
        }
        i
    }
    for i in 0..sites.len() {
        for j in i + 1..sites.len() {
            let (a, b) = (&sites[i], &sites[j]);
            let meet = a.start <= b.start + b.pattern.len() && b.start <= a.start + a.pattern.len();
            if a.group == b.group || meet {
                let (ri, rj) = (root(&cluster, i), root(&cluster, j));
                cluster[ri] = rj;
            }
        }
    }
    let mut order: Vec<(usize, usize)> = Vec::new();
    for (i, site) in sites.iter().enumerate() {
        let r = root(&cluster, i);
        match order.iter_mut().find(|(_, other)| *other == r) {
            Some(entry) => entry.0 = entry.0.min(site.start),
            None => order.push((site.start, r)),
        }
    }
    order.sort();
    let shifts: Vec<(usize, isize)> = (0..sites.len())
        .map(|i| {
            let r = root(&cluster, i);
            (
                sites[i].group,
                0x10000 + 0x100 * order.iter().position(|&(_, o)| o == r).unwrap() as isize,
            )
        })
        .collect();
    let shift = move |group: usize| -> isize {
        shifts
            .iter()
            .find(|&&(g, _)| g == group)
            .expect("a group")
            .1
    };
    let mut image = vec![0u8; size + 0x20000];
    for site in sites {
        let at = (site.start as isize + shift(site.group)) as usize;
        // a window may land on its own cluster's bytes, never on another's
        let clash = site
            .pattern
            .iter()
            .enumerate()
            .any(|(i, byte)| image[at + i] != 0 && image[at + i] != byte.unwrap_or(0xcc));
        if clash {
            return Err(format!(
                "the stand-in's {} site lands on another site",
                site.name
            ));
        }
        put(&mut image, site, at);
    }
    Ok((image, shift))
}

fn put(image: &mut [u8], site: &Site, at: usize) {
    for (i, byte) in site.pattern.iter().enumerate() {
        image[at + i] = byte.unwrap_or(0xcc);
    }
}

/// Where the rel32 at `field` in `image` reaches, as an address; the block a
/// retargeted call reaches lies outside the image.
fn reach(image: &[u8], field: usize) -> usize {
    let rel = i32::from_le_bytes(image[field..field + 4].try_into().unwrap()) as isize;
    (image.as_ptr() as isize + field as isize + 4 + rel) as usize
}

/// What the block holds at `block + offset`.
fn held<T: Copy>(block: usize, offset: usize) -> T {
    unsafe { std::ptr::read_unaligned((block + offset) as *const T) }
}

fn read_rel32(image: &[u8], field: usize) -> usize {
    let rel = i32::from_le_bytes(image[field..field + 4].try_into().unwrap()) as isize;
    (field as isize + 4 + rel) as usize
}

/// Both halves, patched into stand-ins where every site has moved: each must
/// be found, every write must land at the moved site, and every rel32 must
/// reach the moved target. Then the refusals: a site missing, a site found
/// twice, and two sites of one function that have moved apart.
pub fn self_test(
    patch: &Patch,
    game: &GamePatch,
    payload: &[u8],
    game_payload: &[u8],
) -> Result<(), String> {
    use crate::apply::{apply, apply_game, Target};
    use std::path::PathBuf;

    // logic.dll
    let (mut image, shift) = plant(&patch.sites, patch.image_bytes)?;
    let moves = Moves::locate(&patch.sites, &image)?;
    for site in &patch.sites {
        let want = (site.start as isize + shift(site.group)) as usize;
        if moves.at(site.start)? != want {
            return Err(format!(
                "the {} site was not found where it was put",
                site.name
            ));
        }
    }
    // the chooser call's rel32 is a wildcard: aim it at the moved chooser, as
    // the call in a real build would be
    let (call, chooser) = (moves.at(patch.call_site)?, moves.at(patch.stock_chooser)?);
    image[call + 1..call + 5].copy_from_slice(&rel32(call + 5, chooser));
    // and so are the retargeted branches': aim each at the function it
    // reaches, moved by a distance none of the sites has moved
    let away = |stock: usize| stock + 0x18000;
    for call in patch.pose_calls.iter().filter(|c| c.stock != 0) {
        let at = moves.at(call.rva)?;
        image[at + 1..at + 5].copy_from_slice(&rel32(at + 5, away(call.stock)));
    }
    let pristine = image.clone();

    let (moved, found) = patch.relocate(&image, "stand-in")?;
    println!("  logic.dll stand-in: {}", found.report());
    let target = Target {
        process_id: std::process::id(),
        base: image.as_mut_ptr(),
        size: image.len(),
        path: PathBuf::new(),
    };
    let outcome = apply(&moved, &target, payload, true)?;
    if outcome != "patched" {
        return Err(format!("the moved patch did not apply: {outcome:?}"));
    }
    for hook in &moved.detours {
        if image[hook.rva] != 0xe9 {
            return Err(format!("no detour at the moved {:#x}", hook.rva));
        }
    }
    for (rva, after) in [
        (moved.select_is_rva, &moved.select_is_after),
        (moved.select_squad_rva, &moved.select_squad_after),
        (moved.select_toggle_rva, &moved.select_toggle_after),
        (moved.select_type_rva, &moved.select_type_after),
    ] {
        if image[rva..rva + after.len()] != after[..] {
            return Err(format!("the edit at the moved {rva:#x} was not written"));
        }
    }
    for fix in &patch.edit_fixups {
        let reached = read_rel32(&image, moves.at(fix.rva)? + fix.offset);
        if reached != moves.at(fix.target)? {
            return Err(format!(
                "the edit at {:#x} reaches {reached:#x}, not the moved {:#x}",
                fix.rva, fix.target
            ));
        }
    }
    // the block, from the chooser call that now reaches it: each branch from
    // it into the module, each resume imm64 and each of the pose split's
    // distances must follow the moved sites, and each retargeted call reach
    // its entry in the block
    let block = reach(&image, moves.at(patch.call_site)? + 1);
    for call in &moved.pose_calls {
        if image[call.rva] != 0xe8 || reach(&image, call.rva + 1) != block + call.entry {
            return Err(format!(
                "the call at the moved {:#x} does not reach its entry",
                call.rva
            ));
        }
        if image[call.rva + 5..call.rva + 5 + call.tail.len()] != call.tail[..] {
            return Err(format!(
                "the call at the moved {:#x} is missing its tail",
                call.rva
            ));
        }
    }
    for fix in &patch.rel_fixups {
        let rel = held::<i32>(block, fix.offset) as isize;
        let want = image.as_ptr() as usize + moves.at(fix.target_rva)?;
        if (block + fix.offset + 4) as isize + rel != want as isize {
            return Err(format!(
                "the block's branch at +{:#x} misses the moved {:#x}",
                fix.offset, fix.target_rva
            ));
        }
    }
    for fix in &patch.trace_fixups {
        let want = image.as_ptr() as u64 + moves.at(fix.target_rva)? as u64;
        if held::<u64>(block, fix.offset) != want {
            return Err(format!(
                "the resume at +{:#x} misses the moved {:#x}",
                fix.offset, fix.target_rva
            ));
        }
    }
    for fix in &patch.delta_fixups {
        let want = away(fix.to) as isize - moves.at(fix.from)? as isize;
        if held::<i32>(block, fix.offset) as isize != want {
            return Err(format!(
                "the pose split's distance at +{:#x} was not reworked",
                fix.offset
            ));
        }
    }
    println!(
        "  every logic.dll write landed at its moved site; its rel32s, the block's {} branches \
         and the pose split's {} distances follow",
        patch.rel_fixups.len(),
        patch.delta_fixups.len()
    );

    // the refusals
    let (_, split_site) = patch
        .pose_calls
        .iter()
        .enumerate()
        .find_map(|(i, a)| {
            patch.pose_calls[i + 1..]
                .iter()
                .find(|b| b.stock != 0 && b.stock == a.stock)
                .map(|b| (a, b))
        })
        .ok_or("no two retargeted sites reach one function")?;
    let mut split = pristine.clone();
    let at = moves.at(split_site.rva)?;
    split[at + 1..at + 5].copy_from_slice(&rel32(at + 5, away(split_site.stock) + 0x40));
    match patch.relocate(&split, "stand-in") {
        Err(message) if message.contains("now reach") => {
            println!("  refused sites of one function reaching two: {message}")
        }
        Err(message) => {
            return Err(format!(
                "sites reaching two functions were refused for the wrong reason: {message}"
            ))
        }
        Ok(_) => return Err("sites of one function reaching two were not refused".to_string()),
    }
    let first = &patch.sites[0];
    let at = moves.at(first.start)?;
    let mut missing = pristine.clone();
    missing[at..at + first.pattern.len()].fill(0);
    let mut twice = pristine.clone();
    put(&mut twice, first, patch.image_bytes + 0x18000);
    let pair = patch
        .sites
        .iter()
        .enumerate()
        .find_map(|(i, a)| {
            patch.sites[i + 1..]
                .iter()
                .find(|b| {
                    // two sites of one function whose signatures do not
                    // overlap, so zeroing one leaves the other findable
                    b.group == a.group
                        && (a.start + a.pattern.len() <= b.start
                            || b.start + b.pattern.len() <= a.start)
                })
                .map(|b| (a, b))
        })
        .ok_or("no function carries two sites")?;
    let mut apart = pristine.clone();
    let from = moves.at(pair.1.start)?;
    apart[from..from + pair.1.pattern.len()].fill(0);
    put(&mut apart, pair.1, from + 0x40);
    for (label, stand_in, expect) in [
        ("a missing site", &missing, "not in this build"),
        ("a site found twice", &twice, "matches 2 places"),
        ("sites of one function moved apart", &apart, "moved apart"),
    ] {
        match Moves::locate(&patch.sites, stand_in) {
            Err(message) if message.contains(expect) => println!("  refused {label}: {message}"),
            Err(message) => {
                return Err(format!(
                    "{label} was refused for the wrong reason: {message}"
                ))
            }
            Ok(_) => return Err(format!("{label} was not refused")),
        }
    }

    // game.dll
    let (mut image, _) = plant(&game.sites, game.image_bytes)?;
    let (moved, found) = game.relocate(&image, "stand-in")?;
    println!("  game.dll stand-in: {}", found.report());
    let target = Target {
        process_id: std::process::id(),
        base: image.as_mut_ptr(),
        size: image.len(),
        path: PathBuf::new(),
    };
    let outcome = apply_game(&moved, &target, game_payload)?;
    if outcome != "patched" {
        return Err(format!(
            "the moved game.dll patch did not apply: {outcome:?}"
        ));
    }
    let hook = &moved.hooks[0];
    if image[hook.rva] != 0xe9 {
        return Err("no game.dll detour at its moved site".to_string());
    }
    let block = unsafe {
        image
            .as_ptr()
            .add(hook.rva + 5)
            .offset(read_rel32(&image, hook.rva + 1) as isize - (hook.rva + 5) as isize)
            .sub(hook.entry)
    };
    for fix in &moved.fixups {
        let slot = unsafe { std::ptr::read_unaligned(block.add(fix.offset) as *const u64) };
        if slot != image.as_ptr() as u64 + fix.target_rva as u64 {
            return Err(format!(
                "the game.dll slot at +{:#x} does not reach the moved target",
                fix.offset
            ));
        }
    }
    println!("  every game.dll hook and fixup follows its moved site");
    Ok(())
}

// --- checking DLLs on disk --------------------------------------------------

/// What the patch as built expects each site to hold, (rva, bytes): for the
/// build it was written for, every one must be in the file exactly, or the
/// injector refuses that build.
fn logic_expectations(p: &Patch) -> Vec<(usize, Vec<u8>)> {
    let mut out = vec![
        (p.call_site, p.call_before.clone()),
        (p.move_call_site, p.move_displaced.clone()),
        (p.anchor_rva, p.anchor.clone()),
        (p.select_is_rva, p.select_is_before.clone()),
        (p.select_squad_rva, p.select_squad_before.clone()),
        (p.select_toggle_rva, p.select_toggle_before.clone()),
        (p.select_type_rva, p.select_type_before.clone()),
    ];
    out.extend(p.detours.iter().map(|h| (h.rva, h.displaced.clone())));
    out.extend(p.pose_calls.iter().map(|c| (c.rva, c.before.clone())));
    out
}

fn game_expectations(p: &GamePatch) -> Vec<(usize, Vec<u8>)> {
    let mut out = vec![(p.anchor_rva, p.anchor.clone())];
    out.extend(p.hooks.iter().map(|h| (h.rva, h.displaced.clone())));
    out
}

/// Would the patch find its sites in the logic.dll and game.dll in `dir`?
/// Always searches, even in the build the patch was written for, where every
/// site must then turn up exactly where it was. Writes nothing.
pub fn check_dir(patch: &Patch, game: &GamePatch, dir: &std::path::Path) -> bool {
    let mut ok = true;
    for (name, known, verified) in [
        ("logic.dll", &patch.source_sha256, &patch.verified),
        ("game.dll", &game.source_sha256, &game.verified),
    ] {
        let path = dir.join(name);
        let sha = match super::sha256::file(&path) {
            Ok(sha) => sha,
            Err(e) => {
                println!("{name}: {}: {e}", path.display());
                ok = false;
                continue;
            }
        };
        let build = if &sha == known {
            "the build this was written for"
        } else if verified.contains(&sha) {
            "a verified build, relocated automatically"
        } else {
            "an unknown build, relocated only with --scan"
        };
        println!("{name}  sha256 {sha}  ({build})");
        if &sha == known {
            // the patch as built, without any searching, is what runs here
            let expected = if name == "logic.dll" {
                logic_expectations(patch)
            } else {
                game_expectations(game)
            };
            match map_file(&path) {
                Ok(image) => {
                    let unmet: Vec<String> = expected
                        .iter()
                        .filter(|(rva, bytes)| {
                            image.get(*rva..*rva + bytes.len()) != Some(&bytes[..])
                        })
                        .map(|(rva, _)| format!("{rva:#x}"))
                        .collect();
                    if unmet.is_empty() {
                        println!("  as built: all {} expected sites hold", expected.len());
                    } else {
                        println!(
                            "  as built: other bytes than expected at {}",
                            unmet.join(", ")
                        );
                        ok = false;
                    }
                }
                Err(e) => {
                    println!("  {e}");
                    ok = false;
                }
            }
        }
        let found = map_file(&path).and_then(|image| {
            if name == "logic.dll" {
                patch
                    .relocate(&image, &sha)
                    .map(|(_, moves)| moves.report())
            } else {
                game.relocate(&image, &sha).map(|(_, moves)| moves.report())
            }
        });
        match found {
            Ok(report) => println!("  {report}"),
            Err(e) => {
                println!("  refused: {e}");
                ok = false;
            }
        }
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_feature_is_wanted_unless_it_is_disabled() {
        let enabled = 1u64 << 2;
        assert!(wanted(enabled, 0), "shared code is always wanted");
        assert!(wanted(enabled, 2));
        assert!(!wanted(enabled, 6));
    }

    #[test]
    fn the_all_sites_path_is_taken_only_when_every_feature_is_set() {
        let all: u64 =
            (1..=crate::features::KNOWN_FEATURES).fold(0, |mask, feature| mask | (1 << feature));
        assert!(all_known(all));
        assert!(all_known(u64::MAX));
        assert!(!all_known(all & !(1 << 6)));
        assert!(!all_known(0));
    }

    #[test]
    fn selection_keeps_only_the_sites_covering_a_required_address() {
        let sites = vec![
            Site::new("a", 0x100, 0x100, vec![Some(0); 4]),
            Site::new("b", 0x200, 0x200, vec![Some(0); 4]),
        ];
        let chosen = select_sites(&sites, &[0x201, 0x100]);
        assert_eq!(chosen.len(), 2);
        let chosen = select_sites(&sites, &[0x204]);
        assert!(chosen.is_empty());
        let chosen = select_sites(&sites, &[0x203]);
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].name, "b");
    }
}
