//! Where the unit info panel's code lives in game.dll, found by signature
//! instead of looked up by build.
//!
//! Each pattern in the crate root must match exactly once, the relation
//! label's queries must sit where [`LABEL_SLOTS`] says, and every hooked site
//! lies inside its pattern, so a build where any of it moved or changed shape
//! resolves to an error and the plugin installs nothing. The 2025 builds are
//! such builds: their relation label differs.
use crate::{
    AMMO_CALL_AT, AMMO_PATTERN, CLICK_PATTERN, CLICK_SHOWN_CALL_AT, FILL_PATTERN, LABEL_PATTERN,
    LABEL_SLOTS, SQUAD_CALL_AT, SQUAD_PATTERN,
};
use defiance_core::sites::{sig, Image, Signature};

const SQUAD: Signature = sig("squad ownership test", &[(SQUAD_PATTERN, SQUAD_CALL_AT)]);
const AMMO: Signature = sig("ammo ownership test", &[(AMMO_PATTERN, AMMO_CALL_AT)]);
const CLICK: Signature = sig(
    "ammo click shown-entity call",
    &[(CLICK_PATTERN, CLICK_SHOWN_CALL_AT)],
);
const LABEL: Signature = sig("relation label", &[(LABEL_PATTERN, 0)]);
const FILL: Signature = sig("ammo card fill", &[(FILL_PATTERN, 0)]);

/// The game.dll sites, as rvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sites {
    /// The squad setter's `call [rax+0x80]`, hooked over its six bytes.
    pub squad: usize,
    /// The ammo refresh's `call [rdx+0x80]`, hooked over its six bytes.
    pub ammo: usize,
    /// The click handler's call to the shown-entity getter, hooked as a call.
    pub click: usize,
    /// The relation label function, hooked at entry.
    pub label: usize,
    /// The ammo menu's `fillSlot`, hooked at entry.
    pub fill: usize,
}

/// The resolved sites, or why this build is not supported.
pub(crate) fn sites(game: &Image) -> Result<Sites, String> {
    let label = game.find(&LABEL)?;
    for (at, bytes) in LABEL_SLOTS {
        game.expect("relation label query", label + at, bytes)?;
    }
    Ok(Sites {
        squad: game.find(&SQUAD)?,
        ammo: game.find(&AMMO)?,
        click: game.find(&CLICK)?,
        label,
        fill: game.find(&FILL)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS, SEPTEMBER_BUILDS};

    #[test]
    fn sites_resolve_on_every_2026_build_and_not_on_2025() {
        for build in BUILDS {
            let Some(game) = reference(build, "game.dll") else {
                eprintln!("skipping {build}: no bin/{build} DLLs");
                continue;
            };
            let resolved = sites(&Image::mapped(&game));
            if SEPTEMBER_BUILDS.contains(&build) {
                let resolved = resolved.unwrap_or_else(|error| panic!("{build}: {error}"));
                let image = Image::mapped(&game);
                assert!(image.starts_with(resolved.squad, &[0xff, 0x90, 0x80, 0, 0, 0]));
                assert!(image.starts_with(resolved.ammo, &[0xff, 0x92, 0x80, 0, 0, 0]));
                assert!(image.starts_with(resolved.click, &[0xe8]));
            } else {
                assert!(resolved.is_err(), "{build} is not supported");
            }
        }
    }

    #[test]
    fn a_changed_label_query_or_prologue_is_refused() {
        let build = SEPTEMBER_BUILDS[0];
        let Some(game) = reference(build, "game.dll") else {
            return;
        };
        let resolved = sites(&Image::mapped(&game)).unwrap();
        for at in [resolved.label + LABEL_SLOTS[2].0, resolved.fill + 1] {
            let mut changed = game.image.clone();
            changed[at] ^= 1;
            let image = Image {
                image: &changed,
                base: game.base,
            };
            assert!(sites(&image).is_err(), "{at:#x}");
        }
    }

    #[test]
    fn invalid_images_are_refused() {
        let empty = Image {
            image: &[],
            base: 0x180000000,
        };
        assert!(sites(&empty).is_err());
    }
}
