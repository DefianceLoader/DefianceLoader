//! Where Core's session hooks find their native code, by signature instead of
//! by build hash.
//!
//! Functions are found by byte signature (`tools/sigs.py`'s encoding: rel32
//! targets and RIP displacements wildcarded, struct offsets kept, written by
//! `tools/site_signatures.py`) and checked against the entry bytes the hooks
//! relocate. Each must resolve to exactly one place, so a build where a site
//! moved or changed shape resolves to an error and the hook installs nothing.

use defiance_core::sites::{sig, Image, Signature};

/// world2.dll's mission frame, `SceneViewImpl` slot 3 (`this`, time step),
/// which [`crate::session`] hooks to feed mission frame events.
const MISSION_FRAME: Signature = sig(
    "mission frame",
    &[("488bc4488958105556574154415541564157488da868fdffff", 0)],
);
/// The entry bytes the mission frame hook relocates.
pub(crate) const MISSION_FRAME_ENTRY: &[u8] = &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10];

/// The world2.dll mission frame's rva, or why this build is not supported.
pub(crate) fn mission_frame(image: &Image) -> Result<usize, String> {
    let rva = image.find(&MISSION_FRAME)?;
    image.expect("mission frame", rva, MISSION_FRAME_ENTRY)?;
    Ok(rva)
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::reference_world2;

    /// The mission frame per world2.dll build. Builds not listed share the
    /// GOG 2026-09-25 world2.dll.
    fn expected(build: &str) -> usize {
        match build {
            "gog/2026-10-07" | "steam/2026-10-07" => 0x191c50,
            _ => 0x18d7a0,
        }
    }

    #[test]
    fn the_mission_frame_resolves_on_every_world2_in_bin() {
        for (build, mapped) in reference_world2() {
            let image = Image::mapped(&mapped);
            let frame = expected(build);
            assert_eq!(mission_frame(&image), Ok(frame), "{build}");
            let mut changed = mapped.image.clone();
            changed[frame + 4] ^= 1;
            let changed = Image {
                image: &changed,
                base: mapped.base,
            };
            assert!(mission_frame(&changed).is_err(), "{build}");
        }
    }
}
