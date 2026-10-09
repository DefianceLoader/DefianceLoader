//! Where the vehicle arrival callback lives in a logic.dll, found by RTTI and
//! checked against its whole body instead of looked up by build hash.
//!
//! The callback is slot 48 (`+0x180`) of the primary `BaseTechChassisFacet`
//! vtable, which `CarChassisFacet` and `TankChassisFacet` inherit unchanged.
//! [`UPDATE`] is the whole callback (`0x14a` bytes) with only its two helper
//! calls and its `1.0f` constant wildcarded, so every struct offset the detour
//! in [`crate::native`] reads or writes is fixed: a build that changed the
//! algorithm or the layout does not match, and nothing is hooked.
use defiance_core::sites::{sig, Image, Signature};

/// The entry instructions the hook relocates.
pub(crate) const ENTRY_BEFORE: &[u8] =
    &[0x48, 0x89, 0x5c, 0x24, 0x10, 0x57, 0x48, 0x83, 0xec, 0x20];

const CHASSIS: &str = ".?AVBaseTechChassisFacet@Leonardo@@";
const SHARING: [&str; 2] = [
    ".?AVCarChassisFacet@Leonardo@@",
    ".?AVTankChassisFacet@Leonardo@@",
];
const SLOT: usize = 48;

const UPDATE: Signature = sig(
    "vehicle arrival callback",
    &[(
        "48895c2410574883ec20488bf9488bda8b892003000083e901743d83f9020f85????????488b07488bcfff5058f30f1183d00100008b87540300008987640400008b8758030000898768040000488b5c24384883c4205fc3f6824402000001488bcb4889742430488db200020000488bd67407e8????????eb05e8????????488b07488bcfff505848638bc8020000f30f1183d001000085c975050f57d2eb45f68419a702000002744e8d04490f57d24863c8f30f108c8b54020000f30f5c8be8010000f30f10848b5c020000f30f5c83f0010000f30f59c9f30f59c0f30f58c8f30f51d1f30f1083340200000f2fd07706f30f5ed0eb08f30f1015????????f30f1193d4010000f30f5993d0010000f30f1193d00100000f28c2f30f59060f28caf30f1106f30f594e04f30f114e04f30f595608f30f115608488b742430488b5c24384883c4205fc3",
        0,
    )],
);

/// The rva of the vehicle arrival callback, or why this build is not supported.
pub(crate) fn update(image: &Image) -> Result<usize, String> {
    let rva = image.method(CHASSIS, SLOT)?;
    for class in SHARING {
        if image.method(class, SLOT)? != rva {
            return Err(format!(
                "{class} does not share the vehicle arrival callback"
            ));
        }
    }
    if image.find(&UPDATE)? != rva {
        return Err("the vehicle arrival callback differs".into());
    }
    image.expect("the vehicle arrival callback", rva, ENTRY_BEFORE)?;
    Ok(rva)
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};

    /// The rvas the per-build hash table held before the plugin resolved its
    /// site, in [`BUILDS`] order.
    const TABLE: [usize; 7] = [
        0x124be0, 0x124c70, 0x12fd30, 0x12fdc0, 0x12fd30, 0x12fdc0, 0x12fd30,
    ];

    #[test]
    fn the_callback_resolves_where_the_build_table_had_it() {
        for (build, expected) in BUILDS.iter().zip(TABLE) {
            let Some(mapped) = reference(build, "logic.dll") else {
                eprintln!("skipping {build}: no bin/{build}/logic.dll");
                continue;
            };
            assert_eq!(update(&Image::mapped(&mapped)), Ok(expected), "{build}");
        }
    }

    #[test]
    fn a_changed_callback_is_refused() {
        let Some(mapped) = reference(BUILDS[0], "logic.dll") else {
            return;
        };
        let mut changed = mapped.image.clone();
        changed[TABLE[0] + 0xe5] ^= 1;
        let image = Image {
            image: &changed,
            base: mapped.base,
        };
        assert!(update(&image).is_err());
    }
}
