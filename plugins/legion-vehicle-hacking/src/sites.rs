//! Where the final deny branch in `SmartCursorHacking` lives in a game.dll,
//! found by signature instead of looked up by build hash.
//!
//! [`DENY`] is the branch and the two stores around it (`tools/sigs.py`'s
//! encoding; this window has no relative operands, so every byte is fixed).
//! It must match exactly once and start with the bytes the patch replaces, so
//! a build where the branch moved or changed shape resolves to an error and
//! the plugin writes nothing.
use crate::BEFORE;
use defiance_core::sites::{sig, Image, Signature};

/// `jnz +0x11; mov dword [rbx+0x38], 0x30; mov [rbx+0x28], rdi;
/// mov byte [rbx+0x18], 1; jmp +0xc; mov qword [rbx+0x28], 0`.
const DENY: Signature = sig(
    "the hacking deny branch",
    &[("7511c743383000000048897b28c6431801eb0c48c7432800000000", 0)],
);

/// The rva of the deny branch, or why this build is not supported.
pub(crate) fn deny_branch(image: &Image) -> Result<usize, String> {
    let rva = image.find(&DENY)?;
    image.expect(DENY.name, rva, &BEFORE)?;
    Ok(rva)
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};

    /// The rvas the per-build hash table held before the plugin resolved its
    /// site, in [`BUILDS`] order.
    const TABLE: [usize; 6] = [0x3373c5, 0x33d875, 0x3395a5, 0x33fa85, 0x3395a5, 0x33fa85];

    #[test]
    fn the_branch_resolves_where_the_build_table_had_it() {
        for (build, expected) in BUILDS.iter().zip(TABLE) {
            let Some(mapped) = reference(build, "game.dll") else {
                eprintln!("skipping {build}: no bin/{build}/game.dll");
                continue;
            };
            assert_eq!(
                deny_branch(&Image::mapped(&mapped)),
                Ok(expected),
                "{build}"
            );
        }
    }

    #[test]
    fn a_changed_branch_is_refused() {
        let Some(mapped) = reference(BUILDS[0], "game.dll") else {
            return;
        };
        let mut changed = mapped.image.clone();
        changed[TABLE[0] + 1] ^= 1;
        let image = Image {
            image: &changed,
            base: mapped.base,
        };
        assert!(deny_branch(&image).is_err());
    }
}
