//! Where the hooked and called code lives, found by signature and RTTI instead
//! of looked up by build hash.
//!
//! Functions are found by byte signature (`tools/sigs.py`'s encoding: rel32
//! targets and RIP displacements wildcarded, struct offsets kept) and checked
//! against the entry bytes the hooks relocate; vtables are found by their RTTI
//! class. Each must resolve to exactly one place, so a build where any site
//! moved or changed shape resolves to an error and the plugin hooks nothing.
use defiance_core::sites::{sig, Image, Signature};

/// The logic.dll sites, as rvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Logic {
    pub shot: usize,
    pub primary_shot: usize,
    pub gunner_tick: usize,
    pub client_tick: usize,
    pub rebind: usize,
    pub handoff: usize,
    pub animation_vt: usize,
    pub gunner_vt: usize,
    pub gunner_client_vt: usize,
}

/// The world2.dll render-node callbacks the binding check compares against, as
/// rvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct World {
    pub attach: usize,
    pub detach: usize,
}

const SHOT: Signature = sig(
    "gun shot",
    &[("488bc4488958104889702055574156488d68a14881ec90000000", 0)],
);
const PRIMARY_SHOT: Signature = sig(
    "primary gun shot",
    &[("48895c241855565741564157488bec4881ec800000000f29742470", 0)],
);
const GUNNER_TICK: Signature = sig(
    "gunner update",
    &[(
        "48895c2410488974241848897c2420554154415541564157488bec4883ec600f29742450",
        0,
    )],
);
const CLIENT_TICK: Signature = sig(
    "client gunner update",
    &[(
        "48895c241048896c2418488974242057415641574883ec600f29742450",
        0,
    )],
);
const REBIND: Signature = sig(
    "model record rebind",
    &[("48895c2410488974241855574156488bec4883ec60488bf1", 0)],
);
const HANDOFF: Signature = sig(
    "model and animation handoff",
    &[("40574883ec5048638194000000488bf90f297c24200f28f9", 0)],
);
const ATTACH: Signature = sig(
    "render node attach",
    &[("48895c241848896c2420488954241056574154415641574883ec50", 0)],
);
const DETACH: Signature = sig(
    "render node detach",
    &[(
        "48895c2410574883ec404c8bc1488b91700300004885d20f84????????",
        0,
    )],
);

const ANIMATION: &str = ".?AVHumanAnimationFacet@Leonardo@@";
const GUNNER: &str = ".?AVHumanGunner@Leonardo@@";
const GUNNER_CLIENT: &str = ".?AVHumanGunnerClient@Leonardo@@";

const SHOT_BYTES: &[u8] = &[
    0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10, 0x48, 0x89, 0x70, 0x20, 0x55, 0x57, 0x41, 0x56, 0x48,
    0x8d, 0x68, 0xa1, 0x48, 0x81, 0xec, 0x90, 0x00, 0x00, 0x00, 0x0f, 0x29, 0x70, 0xd8, 0x0f, 0x29,
];
const PRIMARY_SHOT_BYTES: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x18, 0x55, 0x56, 0x57, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8b, 0xec, 0x48,
    0x81, 0xec, 0x80, 0x00, 0x00, 0x00, 0x0f, 0x29, 0x74, 0x24, 0x70, 0x0f, 0x29, 0x7c, 0x24, 0x60,
];
const GUNNER_TICK_BYTES: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x48, 0x89, 0x7c, 0x24, 0x20, 0x55,
    0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8b, 0xec, 0x48, 0x83, 0xec, 0x60, 0x0f,
];
const CLIENT_TICK_BYTES: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x6c, 0x24, 0x18, 0x48, 0x89, 0x74, 0x24, 0x20, 0x57,
    0x41, 0x56, 0x41, 0x57, 0x48, 0x83, 0xec, 0x60, 0x0f, 0x29, 0x74, 0x24, 0x50, 0x0f, 0x29, 0x7c,
];
const REBIND_BYTES: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x55, 0x57, 0x41, 0x56, 0x48, 0x8b,
    0xec, 0x48, 0x83, 0xec, 0x60, 0x48, 0x8b, 0xf1, 0x33, 0xff, 0x4c, 0x8b, 0x81, 0x08, 0x01, 0x00,
];

/// The world2.dll entries the binding check identifies callbacks by.
const ATTACH_BYTES: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x18, 0x48, 0x89, 0x6c, 0x24, 0x20, 0x48, 0x89, 0x54, 0x24, 0x10, 0x56,
    0x57, 0x41, 0x54, 0x41, 0x56, 0x41, 0x57, 0x48, 0x83, 0xec, 0x50, 0x4c, 0x8b, 0xfa, 0x48, 0x8b,
];
const DETACH_BYTES: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x10, 0x57, 0x48, 0x83, 0xec, 0x40, 0x4c, 0x8b, 0xc1, 0x48, 0x8b, 0x91,
    0x70, 0x03, 0x00, 0x00, 0x48, 0x85, 0xd2, 0x0f, 0x84, 0xe0, 0x00, 0x00, 0x00, 0x83, 0x7a, 0x08,
];

/// The logic.dll sites, or why this build is not supported.
pub(crate) fn logic(image: &Image) -> Result<Logic, String> {
    let sites = Logic {
        shot: image.find(&SHOT)?,
        primary_shot: image.find(&PRIMARY_SHOT)?,
        gunner_tick: image.find(&GUNNER_TICK)?,
        client_tick: image.find(&CLIENT_TICK)?,
        rebind: image.find(&REBIND)?,
        handoff: image.find(&HANDOFF)?,
        animation_vt: image.primary_vtable(ANIMATION)?.methods_at,
        gunner_vt: image.primary_vtable(GUNNER)?.methods_at,
        gunner_client_vt: image.primary_vtable(GUNNER_CLIENT)?.methods_at,
    };
    for (what, rva, bytes) in [
        ("gun shot", sites.shot, SHOT_BYTES),
        ("primary gun shot", sites.primary_shot, PRIMARY_SHOT_BYTES),
        ("gunner update", sites.gunner_tick, GUNNER_TICK_BYTES),
        ("client gunner update", sites.client_tick, CLIENT_TICK_BYTES),
        ("model record rebind", sites.rebind, REBIND_BYTES),
    ] {
        image.expect(what, rva, bytes)?;
    }
    Ok(sites)
}

/// The world2.dll sites, or why this build is not supported.
pub(crate) fn world(image: &Image) -> Result<World, String> {
    let sites = World {
        attach: image.find(&ATTACH)?,
        detach: image.find(&DETACH)?,
    };
    image.expect("render node attach", sites.attach, ATTACH_BYTES)?;
    image.expect("render node detach", sites.detach, DETACH_BYTES)?;
    Ok(sites)
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, reference_world2, BUILDS_2026};

    /// The rvas the per-build hash table held before the plugin resolved its
    /// sites, in [`BUILDS_2026`] order.
    const TABLE: [Logic; 5] = [
        Logic {
            shot: 0x29ad80,
            primary_shot: 0x29a600,
            gunner_tick: 0x2d7a70,
            client_tick: 0x2dc9f0,
            rebind: 0x4320e0,
            handoff: 0x2d8500,
            animation_vt: 0x72b0d8,
            gunner_vt: 0x72bcc0,
            gunner_client_vt: 0x72be78,
        },
        Logic {
            shot: 0x29ae10,
            primary_shot: 0x29a690,
            gunner_tick: 0x2d7b00,
            client_tick: 0x2dca80,
            rebind: 0x432170,
            handoff: 0x2d8590,
            animation_vt: 0x72b118,
            gunner_vt: 0x72bd00,
            gunner_client_vt: 0x72bed0,
        },
        Logic {
            shot: 0x29ad80,
            primary_shot: 0x29a600,
            gunner_tick: 0x2d7a70,
            client_tick: 0x2dc9f0,
            rebind: 0x432720,
            handoff: 0x2d8500,
            animation_vt: 0x72c0d8,
            gunner_vt: 0x72ccc0,
            gunner_client_vt: 0x72ce78,
        },
        Logic {
            shot: 0x29ae10,
            primary_shot: 0x29a690,
            gunner_tick: 0x2d7b00,
            client_tick: 0x2dca80,
            rebind: 0x4327b0,
            handoff: 0x2d8590,
            animation_vt: 0x72c118,
            gunner_vt: 0x72cd00,
            gunner_client_vt: 0x72ced0,
        },
        Logic {
            shot: 0x29b5c0,
            primary_shot: 0x29ae40,
            gunner_tick: 0x2d84b0,
            client_tick: 0x2dd430,
            rebind: 0x436170,
            handoff: 0x2d8f40,
            animation_vt: 0x731090,
            gunner_vt: 0x731c70,
            gunner_client_vt: 0x731e28,
        },
    ];

    /// The world2.dll rvas per build that has a world2.dll in `bin/`: the
    /// 2026-09-25 file the plugin's old table held (Steam ships the same one),
    /// then GOG 2026-10-07.
    const WORLDS: [(&str, World); 2] = [
        (
            "gog/2026-09-25",
            World {
                attach: 0x154d40,
                detach: 0x1551a0,
            },
        ),
        (
            "gog/2026-10-07",
            World {
                attach: 0x1591e0,
                detach: 0x159640,
            },
        ),
    ];

    #[test]
    fn logic_sites_resolve_where_the_build_table_had_them() {
        for (build, expected) in BUILDS_2026.iter().zip(TABLE) {
            let Some(mapped) = reference(build, "logic.dll") else {
                eprintln!("skipping {build}: no bin/{build}/logic.dll");
                continue;
            };
            assert_eq!(logic(&Image::mapped(&mapped)), Ok(expected), "{build}");
        }
    }

    #[test]
    fn world_sites_resolve_on_every_world2_in_bin() {
        for (build, mapped) in reference_world2() {
            let (_, expected) = WORLDS
                .iter()
                .find(|(b, _)| *b == build)
                .unwrap_or_else(|| panic!("{build}: no expected world2 sites"));
            assert_eq!(world(&Image::mapped(&mapped)), Ok(*expected), "{build}");
        }
    }

    /// The December 2025 logic.dll sites match too. No world2.dll for those
    /// builds is in `bin/`, so whether the plugin installs there is unverified.
    #[test]
    fn logic_sites_also_resolve_on_the_2025_builds() {
        for build in ["gog/2025-12-23", "steam/2025-12-23"] {
            if let Some(mapped) = reference(build, "logic.dll") {
                assert!(logic(&Image::mapped(&mapped)).is_ok(), "{build}");
            }
        }
    }

    #[test]
    fn a_changed_entry_is_refused() {
        let Some(mapped) = reference(BUILDS_2026[0], "logic.dll") else {
            return;
        };
        let mut changed = mapped.image.clone();
        changed[TABLE[0].rebind + 0x1f] ^= 1;
        let image = Image {
            image: &changed,
            base: mapped.base,
        };
        assert!(logic(&image).is_err());
    }
}
