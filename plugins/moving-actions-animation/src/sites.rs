//! Where the hooked, called and compared native code lives, found by signature
//! and RTTI instead of looked up by build hash.
//!
//! The animation update, both samplers and the quaternion helper are found by
//! byte signature (`tools/site_signatures.py`'s encoding) and checked against
//! the entry bytes the hooks relocate. The two facet vtables the owner checks
//! compare against are the primary vtables of their RTTI classes. Each must
//! resolve to exactly one place, so a build where any site moved or changed
//! shape resolves to an error and the plugin hooks nothing.
use defiance_core::sites::{sig, Image, Signature};

/// The logic.dll sites, as rvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sites {
    /// HumanAnimationFacet update.
    pub update: usize,
    /// Native position sampler.
    pub position: usize,
    /// Native rotation sampler.
    pub rotation: usize,
    /// Quaternion interpolation helper.
    pub slerp: usize,
    /// The HumanAnimationFacet vtable.
    pub human_animation_vt: usize,
    /// The HumanChassisFacet vtable.
    pub human_chassis_vt: usize,
}

const UPDATE: Signature = sig(
    "animation update",
    &[("488bc4488958105556574881ece00000000f2970d80f2978c8", 0x0)],
);
const POSITION: Signature = sig(
    "position sampler",
    &[("48895c240848897c2410488b4108498bf94c8b194c8bc04d2bc3", 0x0)],
);
const ROTATION: Signature = sig(
    "rotation sampler",
    &[(
        "48895c241048896c24184889742420574883ec30488bf148bdcdcccccccccccccc",
        0x0,
    )],
);
const SLERP: Signature = sig(
    "quaternion interpolation helper",
    &[(
        "488bc448895810574881ece0000000f30f106908488bfaf30f106104",
        0x0,
    )],
);

const ANIMATION: &str = ".?AVHumanAnimationFacet@Leonardo@@";
const CHASSIS: &str = ".?AVHumanChassisFacet@Leonardo@@";

/// The entry bytes the hooks relocate, and the helper's entry the plugin
/// calls.
const UPDATE_PREFIX: &[u8] = b"\x48\x8b\xc4\x48\x89\x58\x10\x55\x56\x57\x48\x81\xec\xe0\x00\x00";
const POSITION_PREFIX: &[u8] = b"\x48\x89\x5c\x24\x08\x48\x89\x7c\x24\x10\x48\x8b\x41\x08\x49\x8b";
const ROTATION_PREFIX: &[u8] = b"\x48\x89\x5c\x24\x10\x48\x89\x6c\x24\x18\x48\x89\x74\x24\x20\x57";
const SLERP_PREFIX: &[u8] = b"\x48\x8b\xc4\x48\x89\x58\x10\x57\x48\x81\xec\xe0\x00\x00\x00\xf3";

/// `cmp byte ptr [rdi+0xf9], 0` inside the update: the HumanAnimationFacet
/// layout the plugin reads. The 2025-12-23 builds test this field at `0xe1`,
/// with the facet's later fields shifted to match, so they are refused.
const FACET_LAYOUT: (usize, &[u8]) = (0x56, b"\x80\xbf\xf9\x00\x00\x00\x00");

/// The sites, or why this build is not supported.
pub(crate) fn sites(image: &Image) -> Result<Sites, String> {
    let primary = |class: &str| image.primary_vtable(class).map(|vtable| vtable.methods_at);
    let sites = Sites {
        update: image.find(&UPDATE)?,
        position: image.find(&POSITION)?,
        rotation: image.find(&ROTATION)?,
        slerp: image.find(&SLERP)?,
        human_animation_vt: primary(ANIMATION)?,
        human_chassis_vt: primary(CHASSIS)?,
    };
    image.expect("animation update entry", sites.update, UPDATE_PREFIX)?;
    image.expect("position sampler entry", sites.position, POSITION_PREFIX)?;
    image.expect("rotation sampler entry", sites.rotation, ROTATION_PREFIX)?;
    image.expect("quaternion helper entry", sites.slerp, SLERP_PREFIX)?;
    image.expect(
        "HumanAnimationFacet layout",
        sites.update + FACET_LAYOUT.0,
        FACET_LAYOUT.1,
    )?;
    Ok(sites)
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS, SEPTEMBER_BUILDS};

    /// The rvas the plugin's per-build table held before it resolved them, in
    /// [`SEPTEMBER_BUILDS`] order.
    const TABLE: [(&str, Sites); 4] = [
        (
            "gog/2026-09-14",
            Sites {
                update: 0x2c3900,
                position: 0x436260,
                rotation: 0x436460,
                slerp: 0x136880,
                human_animation_vt: 0x72b0d8,
                human_chassis_vt: 0x72b370,
            },
        ),
        (
            "steam/2026-09-22",
            Sites {
                update: 0x2c3990,
                position: 0x4362f0,
                rotation: 0x4364f0,
                slerp: 0x136910,
                human_animation_vt: 0x72b118,
                human_chassis_vt: 0x72b3b0,
            },
        ),
        (
            "gog/2026-09-25",
            Sites {
                update: 0x2c3900,
                position: 0x4368a0,
                rotation: 0x436aa0,
                slerp: 0x136880,
                human_animation_vt: 0x72c0d8,
                human_chassis_vt: 0x72c370,
            },
        ),
        (
            "steam/2026-09-25",
            Sites {
                update: 0x2c3990,
                position: 0x436930,
                rotation: 0x436b30,
                slerp: 0x136910,
                human_animation_vt: 0x72c118,
                human_chassis_vt: 0x72c3b0,
            },
        ),
    ];

    #[test]
    fn sites_resolve_where_the_build_table_had_them() {
        for (build, expected) in TABLE {
            assert!(SEPTEMBER_BUILDS.contains(&build), "{build}");
            let Some(mapped) = reference(build, "logic.dll") else {
                eprintln!("skipping {build}: no bin/{build}/logic.dll");
                continue;
            };
            assert_eq!(sites(&Image::mapped(&mapped)), Ok(expected), "{build}");
        }
    }

    #[test]
    fn sites_resolve_on_every_september_build() {
        for build in SEPTEMBER_BUILDS {
            if let Some(mapped) = reference(build, "logic.dll") {
                assert!(sites(&Image::mapped(&mapped)).is_ok(), "{build}");
            }
        }
    }

    /// The December 2025 builds have the older HumanAnimationFacet layout, so
    /// they are not supported.
    #[test]
    fn the_december_builds_are_refused() {
        for build in BUILDS.into_iter().filter(|b| !SEPTEMBER_BUILDS.contains(b)) {
            if let Some(mapped) = reference(build, "logic.dll") {
                let error = sites(&Image::mapped(&mapped)).unwrap_err();
                assert!(
                    error.contains("HumanAnimationFacet layout"),
                    "{build}: {error}"
                );
            }
        }
    }

    #[test]
    fn a_changed_facet_layout_is_refused() {
        let (build, expected) = TABLE[0];
        let Some(mapped) = reference(build, "logic.dll") else {
            return;
        };
        let mut changed = mapped.image.clone();
        // Past the update signature's end, so only the layout check catches
        // this.
        changed[expected.update + FACET_LAYOUT.0 + 2] ^= 0x01;
        let image = Image {
            image: &changed,
            base: mapped.base,
        };
        assert!(sites(&image).is_err());
    }
}
