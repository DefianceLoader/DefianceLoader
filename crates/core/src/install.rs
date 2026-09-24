//! The policy around `apply`: whether a running build's sites have to be found
//! by signature, and the "try as built, fall back to signatures" sequence the
//! injector used. Shared so the loader's core plugin makes the same decision.
//!
//! Nothing here prints; the result names the outcome and whether the sites were
//! found by signature, and the caller logs it however it likes.

use crate::apply::{apply, apply_game, module_image, Target};
use crate::descriptor::{GamePatch, Patch};
use crate::scan::Moves;
use crate::sha256;

/// When to find the sites by signature (src/relocate.rs).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Scan {
    /// never for the build the patch was written for; always for a verified one
    Default,
    /// --scan: also for any other build
    Unknown,
    /// --force-scan: for every build, the one it was written for included
    Always,
}

/// What an install did.
pub struct Applied {
    /// "patched", "already patched; nothing to do", ...
    pub message: String,
    /// whether the sites were found by signature rather than used as built
    pub relocated: bool,
    /// the moves report, when it relocated
    pub moves: Option<Moves>,
}

/// Whether this build's sites have to be found by signature rather than used as
/// built. `sha` is the module on disk, `source` the build the patch was written
/// for, `verified` the other builds already checked.
pub fn needs_relocation(sha: &str, source: &str, verified: &[String], scan: Scan) -> bool {
    if scan == Scan::Always {
        return true;
    }
    if sha == source {
        return false;
    }
    verified.iter().any(|v| v == sha) || scan == Scan::Unknown
}

/// A named site of the game.dll descriptor as an rva in the running build: the
/// descriptor's own address when it describes this build (the reference or a
/// variant), else wherever the site's signature is (a verified or, with
/// `Scan::Unknown`, an unrecognized build).
pub fn game_site(
    patch: &GamePatch,
    target: &Target,
    scan: Scan,
    name: &str,
) -> Result<usize, String> {
    let site = patch
        .sites
        .iter()
        .find(|site| site.name == name)
        .ok_or_else(|| format!("the descriptor has no {name} site"))?;
    let sha = sha256::file(&target.path).map_err(|e| format!("hashing {:?}: {e}", target.path))?;
    if !needs_relocation(&sha, &patch.source_sha256, &patch.verified, scan) {
        return Ok(site.start);
    }
    let image = module_image(target)?;
    Moves::locate(core::slice::from_ref(site), &image)?.at(site.start)
}

/// Move the patch to wherever its sites are in the running logic.dll.
pub fn relocate_logic(patch: &Patch, target: &Target) -> Result<(Patch, Moves), String> {
    let sha = sha256::file(&target.path).map_err(|e| format!("hashing {:?}: {e}", target.path))?;
    let image = module_image(target)?;
    patch
        .relocate(&image, &sha)
        .map_err(|e| format!("logic.dll: {e}; if this game is already patched, restart it"))
}

/// As for logic.dll, the game.dll half.
pub fn relocate_game(patch: &GamePatch, target: &Target) -> Result<(GamePatch, Moves), String> {
    let sha = sha256::file(&target.path).map_err(|e| format!("hashing {:?}: {e}", target.path))?;
    let image = module_image(target)?;
    patch
        .relocate(&image, &sha)
        .map_err(|e| format!("game.dll: {e}; if this game is already patched, restart it"))
}

/// Move the patch to wherever its sites are in the running logic.dll, only for
/// the features the accepted plan will install.
pub fn relocate_logic_masked(
    patch: &Patch,
    target: &Target,
    enabled: crate::features::FeatureMask,
) -> Result<(Patch, Moves), String> {
    let sha = sha256::file(&target.path).map_err(|e| format!("hashing {:?}: {e}", target.path))?;
    let image = module_image(target)?;
    patch
        .relocate_selected(&image, &sha, enabled)
        .map_err(|e| format!("logic.dll: {e}; if this game is already patched, restart it"))
}

/// As for logic.dll, the game.dll half.
pub fn relocate_game_masked(
    patch: &GamePatch,
    target: &Target,
    enabled: crate::features::FeatureMask,
) -> Result<(GamePatch, Moves), String> {
    let sha = sha256::file(&target.path).map_err(|e| format!("hashing {:?}: {e}", target.path))?;
    let image = module_image(target)?;
    patch
        .relocate_selected(&image, &sha, enabled)
        .map_err(|e| format!("game.dll: {e}; if this game is already patched, restart it"))
}

/// Keep only the features whose signatures are all present: a feature whose
/// site is missing is dropped, and the rest still relocate. The anchor is
/// required by every feature, so a missing anchor drops all of them.
fn retain_relocatable(
    enabled: crate::features::FeatureMask,
    mut present: impl FnMut(u32) -> bool,
) -> crate::features::FeatureMask {
    let mut available = enabled;
    for feature in 1..=crate::features::KNOWN_FEATURES {
        if crate::features::mask_has(enabled, feature) && !present(feature) {
            available &= !(1u64 << feature);
        }
    }
    available
}

/// The subset of `enabled` whose sites are all present in the running
/// logic.dll. On the build the patch was written for nothing has to be found,
/// so every feature is available.
pub fn available_features_logic(
    patch: &Patch,
    target: &Target,
    scan: Scan,
    enabled: crate::features::FeatureMask,
) -> Result<crate::features::FeatureMask, String> {
    let sha = sha256::file(&target.path).map_err(|e| format!("hashing {:?}: {e}", target.path))?;
    if !needs_relocation(&sha, &patch.source_sha256, &patch.verified, scan) {
        return Ok(enabled);
    }
    let image = module_image(target)?;
    Ok(retain_relocatable(enabled, |feature| {
        patch.feature_relocatable(&image, feature)
    }))
}

/// As `available_features_logic`, for the game.dll half.
pub fn available_features_game(
    patch: &GamePatch,
    target: &Target,
    scan: Scan,
    enabled: crate::features::FeatureMask,
) -> Result<crate::features::FeatureMask, String> {
    let sha = sha256::file(&target.path).map_err(|e| format!("hashing {:?}: {e}", target.path))?;
    if !needs_relocation(&sha, &patch.source_sha256, &patch.verified, scan) {
        return Ok(enabled);
    }
    let image = module_image(target)?;
    Ok(retain_relocatable(enabled, |feature| {
        patch.feature_relocatable(&image, feature)
    }))
}

/// The patch as built for the running logic.dll, relocated if the build calls
/// for it.
pub fn logic_for_build(
    patch: &Patch,
    target: &Target,
    scan: Scan,
) -> Result<(Patch, bool, Option<Moves>), String> {
    let sha = sha256::file(&target.path).map_err(|e| format!("hashing {:?}: {e}", target.path))?;
    if !needs_relocation(&sha, &patch.source_sha256, &patch.verified, scan) {
        return Ok((patch.clone(), false, None));
    }
    let (moved, moves) = relocate_logic(patch, target)?;
    Ok((moved, true, Some(moves)))
}

/// As `logic_for_build`, but relocating only the enabled features' sites on a
/// build that has to be found by signature.
pub fn logic_for_build_masked(
    patch: &Patch,
    target: &Target,
    scan: Scan,
    enabled: crate::features::FeatureMask,
) -> Result<(Patch, bool, Option<Moves>), String> {
    let sha = sha256::file(&target.path).map_err(|e| format!("hashing {:?}: {e}", target.path))?;
    if !needs_relocation(&sha, &patch.source_sha256, &patch.verified, scan) {
        return Ok((patch.clone(), false, None));
    }
    let (moved, moves) = relocate_logic_masked(patch, target, enabled)?;
    Ok((moved, true, Some(moves)))
}

/// As for logic.dll, the game.dll half.
pub fn game_for_build(
    patch: &GamePatch,
    target: &Target,
    scan: Scan,
) -> Result<(GamePatch, bool, Option<Moves>), String> {
    let sha = sha256::file(&target.path).map_err(|e| format!("hashing {:?}: {e}", target.path))?;
    if !needs_relocation(&sha, &patch.source_sha256, &patch.verified, scan) {
        return Ok((patch.clone(), false, None));
    }
    let (moved, moves) = relocate_game(patch, target)?;
    Ok((moved, true, Some(moves)))
}

/// As `game_for_build`, but relocating only the enabled features' sites.
pub fn game_for_build_masked(
    patch: &GamePatch,
    target: &Target,
    scan: Scan,
    enabled: crate::features::FeatureMask,
) -> Result<(GamePatch, bool, Option<Moves>), String> {
    let sha = sha256::file(&target.path).map_err(|e| format!("hashing {:?}: {e}", target.path))?;
    if !needs_relocation(&sha, &patch.source_sha256, &patch.verified, scan) {
        return Ok((patch.clone(), false, None));
    }
    let (moved, moves) = relocate_game_masked(patch, target, enabled)?;
    Ok((moved, true, Some(moves)))
}

/// Prepare and apply the logic.dll patch, with the injector's fallback: if the
/// patch as built does not fit and `scan` allows it, find the sites and retry.
/// `chooser` says whether the assembled weapon-pickup chooser is installed here
/// (the injector) or left for a plugin (the loader).
pub fn install_logic(
    patch: &Patch,
    target: &Target,
    scan: Scan,
    payload: &[u8],
    chooser: bool,
) -> Result<Applied, String> {
    let (prepared, relocated, moves) = logic_for_build(patch, target, scan)?;
    match apply(&prepared, target, payload, chooser) {
        Ok(message) => Ok(Applied {
            message: message.to_string(),
            relocated,
            moves,
        }),
        Err(_message) if scan == Scan::Unknown && !relocated => {
            let (moved, moves) = relocate_logic(patch, target)?;
            let message = apply(&moved, target, payload, chooser)?;
            Ok(Applied {
                message: message.to_string(),
                relocated: true,
                moves: Some(moves),
            })
        }
        Err(message) => Err(message),
    }
}

/// As for logic.dll, the game.dll half.
pub fn install_game(
    patch: &GamePatch,
    target: &Target,
    scan: Scan,
    payload: &[u8],
) -> Result<Applied, String> {
    let (prepared, relocated, moves) = game_for_build(patch, target, scan)?;
    match apply_game(&prepared, target, payload) {
        Ok(message) => Ok(Applied {
            message: message.to_string(),
            relocated,
            moves,
        }),
        Err(_message) if scan == Scan::Unknown && !relocated => {
            let (moved, moves) = relocate_game(patch, target)?;
            let message = apply_game(&moved, target, payload)?;
            Ok(Applied {
                message: message.to_string(),
                relocated: true,
                moves: Some(moves),
            })
        }
        Err(message) => Err(message),
    }
}
