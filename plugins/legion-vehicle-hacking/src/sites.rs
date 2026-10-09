//! Where `SmartCursorHacking`'s last two target checks live in a game.dll,
//! and `AiUnitHackingState`'s target check in a logic.dll, found by signature
//! instead of looked up by build hash.
//!
//! [`DENY`] is the final deny branch and the two stores around it
//! (`tools/sigs.py`'s encoding; this window has no relative operands, so every
//! byte is fixed). [`SUSPENSION`] starts at the `empvulnerable` test and runs
//! through the suspension and deny calls; their displacements are wildcards
//! because the AI facet's vtable grew by two slots between the 2025-12-23 and
//! 2026 builds. [`STATE_CHECK`] is the hacker's per-tick check that its
//! target is still suspended. Each must match exactly once and start with the bytes the
//! plugin replaces, so a build where the check moved or changed shape resolves
//! to an error and the plugin writes nothing.
use crate::BEFORE;
use defiance_core::sites::{sig, Image, Signature};

/// `jnz +0x11; mov dword [rbx+0x38], 0x30; mov [rbx+0x28], rdi;
/// mov byte [rbx+0x18], 1; jmp +0xc; mov qword [rbx+0x28], 0`.
const DENY: Signature = sig(
    "the hacking deny branch",
    &[("7511c743383000000048897b28c6431801eb0c48c7432800000000", 0)],
);

/// `cmp byte [rbp+empvulnerable], 0; je refuse; mov rax, [rsi]; mov rcx, rsi;
/// call [rax+suspended]; test al, al; je refuse; mov rax, [rsi];
/// mov rcx, rsi; call [rax+engaged]; test al, al; jnz refuse`. The site is
/// the suspension call, 15 bytes in.
const SUSPENSION: Signature = sig(
    "the hacking suspension test",
    &[(
        "80bd????????007431488b06488bceff90????????84c07421488b06488bceff90????????84c07511",
        15,
    )],
);

/// `AiUnitHackingState`'s target check (logic.dll): `mov rax, [rdi];
/// mov rcx, rdi; call [rax+0xb0]; mov rcx, [rax+0x28]; test rcx, rcx;
/// je done; mov rax, [rcx]; mov [rsp+0x40], rsi; call [rax+suspended];
/// test al, al; jne done; mov rcx, rbx; call finish`. RDI is the target
/// entity; the site is the suspension call, 29 bytes in.
const STATE_CHECK: Signature = sig(
    "the hacking state's suspension check",
    &[(
        "488b07488bcfff90b0000000488b48284885c97451488b014889742440ff90????????84c07508488bcbe8",
        29,
    )],
);

/// The bytes each suspension hook displaces: `call qword ptr [rax+disp32]`.
pub(crate) const SUSPENSION_CALL: [u8; 2] = [0xff, 0x90];
pub(crate) const SUSPENSION_LENGTH: usize = 6;

/// The rva of the deny branch, or why this build is not supported.
pub(crate) fn deny_branch(image: &Image) -> Result<usize, String> {
    let rva = image.find(&DENY)?;
    image.expect(DENY.name, rva, &BEFORE)?;
    Ok(rva)
}

/// The rva of the suspension call and the AI facet vtable offset it calls,
/// or why this build is not supported.
pub(crate) fn suspension_test(image: &Image) -> Result<(usize, usize), String> {
    let rva = image.find(&SUSPENSION)?;
    image.expect(SUSPENSION.name, rva, &SUSPENSION_CALL)?;
    let slot = image
        .u32(rva + 2)
        .ok_or_else(|| format!("{} has no slot", SUSPENSION.name))?;
    Ok((rva, slot as usize))
}

/// The rva of the hacking state's suspension call and the AI facet vtable
/// offset it calls, or why this logic.dll is not supported.
pub(crate) fn state_check(image: &Image) -> Result<(usize, usize), String> {
    let rva = image.find(&STATE_CHECK)?;
    image.expect(STATE_CHECK.name, rva, &SUSPENSION_CALL)?;
    let slot = image
        .u32(rva + 2)
        .ok_or_else(|| format!("{} has no slot", STATE_CHECK.name))?;
    Ok((rva, slot as usize))
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};

    /// The rvas the per-build hash table held before the plugin resolved its
    /// site, in [`BUILDS`] order.
    const TABLE: [usize; 7] = [
        0x3373c5, 0x33d875, 0x3395a5, 0x33fa85, 0x3395a5, 0x33fa85, 0x347dc5,
    ];

    /// The suspension check's vtable offset, in [`BUILDS`] order.
    const SLOTS: [usize; 7] = [0x170, 0x170, 0x180, 0x180, 0x180, 0x180, 0x180];

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
    fn the_suspension_call_precedes_the_branch_in_every_build() {
        for ((build, branch), slot) in BUILDS.iter().zip(TABLE).zip(SLOTS) {
            let Some(mapped) = reference(build, "game.dll") else {
                eprintln!("skipping {build}: no bin/{build}/game.dll");
                continue;
            };
            assert_eq!(
                suspension_test(&Image::mapped(&mapped)),
                Ok((branch - 0x18, slot)),
                "{build}"
            );
        }
    }

    /// The hacking state's suspension call in each logic.dll, in [`BUILDS`]
    /// order.
    const STATE: [usize; 7] = [
        0x7d057, 0x7d0e7, 0x85717, 0x857a7, 0x85717, 0x857a7, 0x85717,
    ];

    #[test]
    fn the_state_check_resolves_in_every_build() {
        for ((build, site), slot) in BUILDS.iter().zip(STATE).zip(SLOTS) {
            let Some(mapped) = reference(build, "logic.dll") else {
                eprintln!("skipping {build}: no bin/{build}/logic.dll");
                continue;
            };
            assert_eq!(
                state_check(&Image::mapped(&mapped)),
                Ok((site, slot)),
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
        assert!(suspension_test(&image).is_err());
    }
}
