//! Where the movement-speed getter's action table lives in a logic.dll, found
//! by signature instead of looked up by build hash.
//!
//! The getter maps an action id through a byte table to one of four branches
//! of a jump table; both tables follow the code and are addressed from the
//! module base, so their RVAs are the only bytes that differ between builds.
//! The getter is found by signature (`tools/sigs.py`'s encoding: those RVAs
//! wildcarded), then the code and both tables are checked against
//! [`GETTER`] with this build's RVAs filled in. A build where the getter does
//! not resolve to exactly one place, or where any of those bytes differ, is
//! refused and nothing is patched.
use defiance_core::sites::{sig, Image, Signature};

const SIGNATURE: Signature = sig(
    "movement speed getter",
    &[(
        "4c8b4158418b406c83c0fd83f82877704c8d0d????????4898410fb68401????????\
         418b9481????????4903d1ffe2418b507c85d2743883fa017513",
        0,
    )],
);

/// The getter's code, jump table and action table, with the module-base
/// displacement at [`BASE_FIELD`], the table RVAs at [`ACTIONS_FIELD`] and
/// [`JUMPS_FIELD`], and the jump table at [`JUMPS`] zeroed.
const GETTER: [u8; 0xe9] = [
    0x4c, 0x8b, 0x41, 0x58, 0x41, 0x8b, 0x40, 0x6c, 0x83, 0xc0, 0xfd, 0x83, 0xf8, 0x28, 0x77, 0x70,
    0x4c, 0x8d, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x48, 0x98, 0x41, 0x0f, 0xb6, 0x84, 0x01, 0x00, 0x00,
    0x00, 0x00, 0x41, 0x8b, 0x94, 0x81, 0x00, 0x00, 0x00, 0x00, 0x49, 0x03, 0xd1, 0xff, 0xe2, 0x41,
    0x8b, 0x50, 0x7c, 0x85, 0xd2, 0x74, 0x38, 0x83, 0xfa, 0x01, 0x75, 0x13, 0x41, 0x8b, 0x50, 0x74,
    0x83, 0xea, 0x01, 0x74, 0x1a, 0x83, 0xea, 0x01, 0x74, 0x54, 0x83, 0xfa, 0x01, 0x74, 0x3f, 0x48,
    0x8b, 0x81, 0xb0, 0x00, 0x00, 0x00, 0xf3, 0x0f, 0x10, 0x80, 0x28, 0x03, 0x00, 0x00, 0xc3, 0x48,
    0x8b, 0x81, 0xb0, 0x00, 0x00, 0x00, 0xf3, 0x0f, 0x10, 0x80, 0x1c, 0x03, 0x00, 0x00, 0xc3, 0x48,
    0x8b, 0x41, 0x68, 0xf3, 0x0f, 0x10, 0x80, 0x7c, 0x01, 0x00, 0x00, 0xc3, 0x0f, 0x57, 0xc0, 0xc3,
    0x41, 0x8b, 0x50, 0x74, 0x83, 0xea, 0x02, 0x74, 0x15, 0x83, 0xfa, 0x01, 0x75, 0xe1, 0x48, 0x8b,
    0x81, 0xb0, 0x00, 0x00, 0x00, 0xf3, 0x0f, 0x10, 0x80, 0x24, 0x03, 0x00, 0x00, 0xc3, 0x48, 0x8b,
    0x81, 0xb0, 0x00, 0x00, 0x00, 0xf3, 0x0f, 0x10, 0x80, 0x20, 0x03, 0x00, 0x00, 0xc3, 0x66, 0x90,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x01, 0x03, 0x03, 0x01, 0x03, 0x03, 0x03, 0x03, 0x03, 0x02, 0x02, 0x03, 0x03, 0x03, 0x03,
    0x03, 0x03, 0x03, 0x03, 0x01, 0x01, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03,
    0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x01,
];
/// The `lea r9` displacement, which must make R9 the module base.
const BASE_FIELD: usize = 0x13;
/// The action table's RVA in the `movzx` that reads it.
const ACTIONS_FIELD: usize = 0x1e;
/// The jump table's RVA in the `mov` that reads it.
const JUMPS_FIELD: usize = 0x26;
/// The jump table, four branch RVAs.
const JUMPS: usize = 0xb0;
/// The jump table's branches, as offsets from the getter.
const BRANCHES: [usize; 4] = [0x2f, 0x7c, 0x4f, 0x80];
/// Where the action table starts. It covers action ids from 3.
const ACTIONS: usize = 0xc0;
/// Grenade throw (0x17), weapon change (0x18) and the second throw action
/// (0x2b), which select branch 1 (zero speed).
const PATCHED: [usize; 3] = [0x17, 0x18, 0x2b];

/// The getter's entry and the three action-table bytes to switch from branch
/// 1 to branch 3 (the posture-dependent speed).
#[derive(Debug, PartialEq)]
pub(crate) struct Getter {
    pub rva: usize,
    pub patches: [usize; 3],
}

/// The getter as this build lays it out.
fn expected(rva: usize) -> [u8; 0xe9] {
    let mut bytes = GETTER;
    let put = |bytes: &mut [u8; 0xe9], at: usize, value: u32| {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    };
    put(
        &mut bytes,
        BASE_FIELD,
        (rva + BASE_FIELD + 4).wrapping_neg() as u32,
    );
    put(&mut bytes, ACTIONS_FIELD, (rva + ACTIONS) as u32);
    put(&mut bytes, JUMPS_FIELD, (rva + JUMPS) as u32);
    for (slot, branch) in BRANCHES.iter().enumerate() {
        put(&mut bytes, JUMPS + 4 * slot, (rva + branch) as u32);
    }
    bytes
}

/// The getter and its patch sites, or why this build is not supported.
pub(crate) fn getter(image: &Image) -> Result<Getter, String> {
    let rva = image.find(&SIGNATURE)?;
    image.expect("movement speed getter", rva, &expected(rva))?;
    Ok(Getter {
        rva,
        patches: PATCHED.map(|action| rva + ACTIONS + action - 3),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};

    /// The getter rvas and patch offsets the per-build hash table held before
    /// the plugin resolved them, in [`BUILDS`] order.
    const TABLE: [(&str, usize); 6] = [
        ("gog/2025-12-23", 0x2bcac0),
        ("steam/2025-12-23", 0x2bcb50),
        ("gog/2026-09-14", 0x2caf20),
        ("steam/2026-09-22", 0x2cafb0),
        ("gog/2026-09-25", 0x2caf20),
        ("steam/2026-09-25", 0x2cafb0),
    ];
    const OFFSETS: [usize; 3] = [0xd4, 0xd5, 0xe8];

    #[test]
    fn the_getter_resolves_where_the_build_table_had_it() {
        for (build, rva) in TABLE {
            assert!(BUILDS.contains(&build), "{build}");
            let Some(mapped) = reference(build, "logic.dll") else {
                eprintln!("skipping {build}: no bin/{build}/logic.dll");
                continue;
            };
            assert_eq!(
                getter(&Image::mapped(&mapped)),
                Ok(Getter {
                    rva,
                    patches: OFFSETS.map(|offset| rva + offset),
                }),
                "{build}"
            );
        }
    }

    #[test]
    fn the_getter_resolves_on_every_build() {
        for build in BUILDS {
            if let Some(mapped) = reference(build, "logic.dll") {
                assert!(getter(&Image::mapped(&mapped)).is_ok(), "{build}");
            }
        }
    }

    #[test]
    fn a_changed_action_table_is_refused() {
        let (build, rva) = TABLE[2];
        let Some(mapped) = reference(build, "logic.dll") else {
            return;
        };
        let mut changed = mapped.image.clone();
        changed[rva + OFFSETS[0]] = 0;
        let image = Image {
            image: &changed,
            base: mapped.base,
        };
        assert!(getter(&image).is_err());
    }
}
