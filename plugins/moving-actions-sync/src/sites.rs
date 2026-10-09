//! Where the weapon-change step lives in a logic.dll, found by signature
//! instead of looked up by build hash.
//!
//! The step and the animation-action helper it calls are found by byte
//! signature (`tools/sigs.py`'s encoding: rel32 targets and RIP displacements
//! wildcarded, struct offsets kept) and checked against the entry bytes the
//! hook relocates. Each must resolve to exactly one place, so a build where
//! either moved or changed shape resolves to an error and nothing is hooked.
use defiance_core::sites::{sig, Image, Signature};

const STEP: Signature = sig(
    "weapon-change step",
    &[("40574883ec5048638194000000488bf90f297c24200f28f9", 0)],
);
const HELPER: Signature = sig(
    "animation action helper",
    &[("48895c2408574883ec2048837928008bfa488bd90f85????????", 0)],
);

/// The step's entry, which the hook relocates.
const STEP_BYTES: &[u8] = &[
    0x40, 0x57, 0x48, 0x83, 0xec, 0x50, 0x48, 0x63, 0x81, 0x94, 0x00, 0x00, 0x00, 0x48, 0x8b, 0xf9,
    0x0f, 0x29, 0x7c, 0x24, 0x20, 0x0f, 0x28, 0xf9, 0x83, 0xf8, 0xff, 0x75, 0x17, 0xc7, 0x81, 0x90,
];
/// The helper's entry, whose handoff branch the repair re-enters through the
/// step.
const HELPER_BYTES: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x08, 0x57, 0x48, 0x83, 0xec, 0x20, 0x48, 0x83, 0x79, 0x28, 0x00, 0x8b,
];

/// The rva of the weapon-change step, or why this build is not supported.
pub(crate) fn step(image: &Image) -> Result<usize, String> {
    let step = image.find(&STEP)?;
    let helper = image.find(&HELPER)?;
    image.expect("weapon-change step", step, STEP_BYTES)?;
    image.expect("animation action helper", helper, HELPER_BYTES)?;
    Ok(step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS, BUILDS_2026};

    /// The step and helper rvas the per-build hash table held before the plugin
    /// resolved them, in [`BUILDS_2026`] order.
    const TABLE: [(usize, usize); 5] = [
        (0x2d8500, 0x2c2860),
        (0x2d8590, 0x2c28f0),
        (0x2d8500, 0x2c2860),
        (0x2d8590, 0x2c28f0),
        (0x2d8f40, 0x2c31a0),
    ];

    #[test]
    fn the_sites_resolve_where_the_build_table_had_them() {
        for (build, (expected_step, expected_helper)) in BUILDS_2026.iter().zip(TABLE) {
            let Some(mapped) = reference(build, "logic.dll") else {
                eprintln!("skipping {build}: no bin/{build}/logic.dll");
                continue;
            };
            let image = Image::mapped(&mapped);
            assert_eq!(step(&image), Ok(expected_step), "{build}");
            assert_eq!(image.find(&HELPER), Ok(expected_helper), "{build}");
        }
    }

    /// The sites resolve uniquely on every build in `bin/`, the December 2025
    /// builds included.
    #[test]
    fn the_sites_resolve_on_every_build() {
        for build in BUILDS {
            if let Some(mapped) = reference(build, "logic.dll") {
                assert!(step(&Image::mapped(&mapped)).is_ok(), "{build}");
            }
        }
    }

    #[test]
    fn a_changed_entry_is_refused() {
        let Some(mapped) = reference(BUILDS_2026[0], "logic.dll") else {
            return;
        };
        let mut changed = mapped.image.clone();
        changed[TABLE[0].0 + 0x1f] ^= 1;
        let image = Image {
            image: &changed,
            base: mapped.base,
        };
        assert!(step(&image).is_err());
    }
}
