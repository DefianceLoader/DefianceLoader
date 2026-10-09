//! Where the performance plugin's tree-sway hooks and diagnostic features
//! find their native code, by signature instead of by build hash.
//!
//! Functions and call sites are found by byte signature (`tools/sigs.py`'s
//! encoding: rel32 targets and RIP displacements wildcarded, struct offsets
//! kept, written by `tools/site_signatures.py`) and checked against the entry
//! bytes the hooks relocate. Each must resolve to exactly one place, so a
//! build where any site moved or changed shape resolves to an error and the
//! feature hooks nothing. The diagnostic sites are compiled into every build of
//! the plugin so the tests check them whether or not a diagnostic feature is on.
#![cfg_attr(not(feature = "render-profile"), allow(dead_code))]

use defiance_core::sites::{sig, Image, Signature};

/// The logic.dll functions `tree_sway` hooks, as rvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tree {
    /// The living-tree manager update, which passes each tree its time step.
    pub manager: usize,
    /// One tree's sway update.
    pub sway: usize,
    /// One tree facet's refresh.
    pub facet: usize,
}

const TREE_MANAGER: Signature = sig(
    "living-tree manager update",
    &[(
        "488bc4488958104889701855574156488da858feffff4881ec90020000",
        0,
    )],
);
const TREE_SWAY: Signature = sig(
    "tree sway update",
    &[("48895c2408574883ec60488bd90f297424500f297c24400f28c1", 0)],
);
const TREE_FACET: Signature = sig(
    "tree facet refresh",
    &[("48895c2408488974241048897c241841564881ec90000000", 0)],
);

/// The entry bytes each `tree_sway` hook relocates.
pub(crate) const TREE_MANAGER_ENTRY: &[u8] = &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10];
pub(crate) const TREE_ENTRY: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x08];

/// The logic.dll tree-sway functions, or why this build is not supported.
pub(crate) fn tree(image: &Image) -> Result<Tree, String> {
    let sites = Tree {
        manager: image.find(&TREE_MANAGER)?,
        sway: image.find(&TREE_SWAY)?,
        facet: image.find(&TREE_FACET)?,
    };
    image.expect(
        "living-tree manager update",
        sites.manager,
        TREE_MANAGER_ENTRY,
    )?;
    image.expect("tree sway update", sites.sway, TREE_ENTRY)?;
    image.expect("tree facet refresh", sites.facet, TREE_ENTRY)?;
    Ok(sites)
}

/// The world2.dll sites of the `render-profile` features, as rvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Render {
    /// The main-view preparation call, redirected to time `main_fn`.
    pub main_call: usize,
    pub main_fn: usize,
    /// The two batch-builder calls inside `main_fn`.
    pub batch_calls: [usize; 2],
    pub batch_fn: usize,
    /// The dirty world-transform recomputation, hooked at its entry.
    pub dirty_fn: usize,
    /// The shadow-caster visibility query, hooked at its entry.
    pub cull_fn: usize,
}

const MAIN_CALL: Signature = sig(
    "main-view preparation call",
    &[
        (
            "e8????????488bcee8????????80be38040000007431488b8e90000000",
            0,
        ),
        // From GOG 2026-10-07 the view's fields lie 8 bytes on.
        (
            "e8????????488bcee8????????80be40040000007431488b8e90000000",
            0,
        ),
    ],
);
const MAIN_FN: Signature = sig(
    "main-view preparation",
    &[("40555356574154415541564157488dac24f8eeffffb808120000", 0)],
);
const BATCH_CALL_0: Signature = sig(
    "first batch-builder call",
    &[("e8????????90488b4d404885c97410488b01488d5508483bca", 0)],
);
const BATCH_CALL_1: Signature = sig(
    "second batch-builder call",
    &[("e8????????90488b8d800000004885c97410488b01488d5548", 0)],
);
const BATCH_FN: Signature = sig(
    "batch builder",
    &[("488bc4488958184c89482048894808555657415441554156", 0)],
);
const DIRTY_FN: Signature = sig(
    "dirty world transform",
    &[(
        "48895c2408488974241048897c24184c8974242055488bec4883ec70488bf9488b9170030000",
        0,
    )],
);
const CULL_FN: Signature = sig(
    "shadow-caster cull",
    &[("48895c2410488974241848897c242055488dac2410feffff", 0)],
);
/// The shadow pass's indirect call that reaches `cull_fn`; the cull timer
/// relies on this caller consuming the output as an 8-byte pointer vector.
const CULL_CALLER: Signature = sig(
    "shadow-caster cull caller",
    &[("ffd390488b8d680100004885c97413488b01488d9530010000", 0)],
);

/// The entry bytes each hook relocates or the probe checks.
pub(crate) const MAIN_ENTRY: &[u8] = &[
    0x40, 0x55, 0x53, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57,
];
pub(crate) const BATCH_ENTRY: &[u8] = &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x18];
pub(crate) const DIRTY_ENTRY: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x08];
pub(crate) const CULL_ENTRY: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x48, 0x89, 0x7c, 0x24, 0x20, 0x55,
];

/// The world2.dll render-profile sites, or why this build is not supported.
pub(crate) fn render(image: &Image) -> Result<Render, String> {
    let sites = Render {
        main_call: image.call(&MAIN_CALL)?,
        main_fn: image.find(&MAIN_FN)?,
        batch_calls: [image.call(&BATCH_CALL_0)?, image.call(&BATCH_CALL_1)?],
        batch_fn: image.find(&BATCH_FN)?,
        dirty_fn: image.find(&DIRTY_FN)?,
        cull_fn: image.find(&CULL_FN)?,
    };
    image.find(&CULL_CALLER)?;
    if image.branch_target(sites.main_call) != Some(sites.main_fn) {
        return Err("the main-view call does not reach main-view preparation".into());
    }
    for call in sites.batch_calls {
        if image.branch_target(call) != Some(sites.batch_fn) {
            return Err(format!(
                "the batch call at {call:#x} does not reach the batch builder"
            ));
        }
    }
    for (what, rva, bytes) in [
        ("main-view preparation", sites.main_fn, MAIN_ENTRY),
        ("batch builder", sites.batch_fn, BATCH_ENTRY),
        ("dirty world transform", sites.dirty_fn, DIRTY_ENTRY),
        ("shadow-caster cull", sites.cull_fn, CULL_ENTRY),
    ] {
        image.expect(what, rva, bytes)?;
    }
    Ok(sites)
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, reference_world2, BUILDS};

    /// The rvas `tree_sway` held per build before it resolved them.
    const TREES: [(&str, Tree); 8] = [
        (
            "gog/2025-12-23",
            Tree {
                manager: 0x46df70,
                sway: 0x46b890,
                facet: 0x46c6b0,
            },
        ),
        (
            "steam/2025-12-23",
            Tree {
                manager: 0x46e000,
                sway: 0x46b920,
                facet: 0x46c740,
            },
        ),
        (
            "gog/2026-09-14",
            Tree {
                manager: 0x4807f0,
                sway: 0x47e110,
                facet: 0x47ef30,
            },
        ),
        (
            "steam/2026-09-22",
            Tree {
                manager: 0x480880,
                sway: 0x47e1a0,
                facet: 0x47efc0,
            },
        ),
        (
            "gog/2026-09-25",
            Tree {
                manager: 0x480e30,
                sway: 0x47e750,
                facet: 0x47f570,
            },
        ),
        (
            "steam/2026-09-25",
            Tree {
                manager: 0x480ec0,
                sway: 0x47e7e0,
                facet: 0x47f600,
            },
        ),
        (
            "gog/2026-10-07",
            Tree {
                manager: 0x4843e0,
                sway: 0x481d00,
                facet: 0x482b20,
            },
        ),
        (
            "steam/2026-10-07",
            Tree {
                manager: 0x484470,
                sway: 0x481d90,
                facet: 0x482bb0,
            },
        ),
    ];

    #[test]
    fn tree_sites_resolve_on_every_build_in_bin() {
        assert_eq!(TREES.map(|(build, _)| build), BUILDS);
        for (build, expected) in TREES {
            let Some(mapped) = reference(build, "logic.dll") else {
                continue;
            };
            assert_eq!(tree(&Image::mapped(&mapped)), Ok(expected), "{build}");
        }
    }

    #[test]
    fn a_changed_tree_entry_is_refused() {
        let (build, expected) = TREES[5];
        let Some(mapped) = reference(build, "logic.dll") else {
            return;
        };
        let mut changed = mapped.image.clone();
        changed[expected.facet + 4] ^= 1;
        let image = Image {
            image: &changed,
            base: mapped.base,
        };
        assert!(tree(&image).is_err());
    }

    /// The rvas `render_profile` held before it resolved them: the Steam
    /// 2026-09-25 world2.dll, which is byte-identical to the GOG 2026-09-25
    /// one in `bin/`.
    const RENDER: Render = Render {
        main_call: 0x18d9d9,
        main_fn: 0x18f940,
        batch_calls: [0x19031f, 0x190391],
        batch_fn: 0x18f0c0,
        dirty_fn: 0x154160,
        cull_fn: 0x1da490,
    };

    /// Per world2.dll build: the render sites and the cull caller. Builds
    /// not listed share the GOG 2026-09-25 world2.dll.
    fn world2_expected(build: &str) -> (Render, usize) {
        match build {
            "gog/2026-10-07" | "steam/2026-10-07" => (
                Render {
                    main_call: 0x191e89,
                    main_fn: 0x193e40,
                    batch_calls: [0x19481c, 0x19488e],
                    batch_fn: 0x1935c0,
                    dirty_fn: 0x158600,
                    cull_fn: 0x1de980,
                },
                0x19611c,
            ),
            _ => (RENDER, 0x191c2c),
        }
    }

    #[test]
    fn render_sites_resolve_on_every_world2_in_bin() {
        for (build, mapped) in reference_world2() {
            let image = Image::mapped(&mapped);
            let (expected, caller) = world2_expected(build);
            assert_eq!(render(&image), Ok(expected), "{build}");
            // The caller check the probe made before it resolved its sites.
            assert!(image.starts_with(caller, &[0xff, 0xd3, 0x90]), "{build}");
        }
    }

    #[test]
    fn a_changed_render_entry_is_refused() {
        let Some((build, mapped)) = reference_world2().into_iter().next() else {
            return;
        };
        let mut changed = mapped.image.clone();
        changed[world2_expected(build).0.dirty_fn] ^= 1;
        let image = Image {
            image: &changed,
            base: mapped.base,
        };
        assert!(render(&image).is_err());
    }
}
