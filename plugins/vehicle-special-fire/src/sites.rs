//! Where passenger targeting's native code lives in logic.dll and game.dll,
//! found by RTTI and signature instead of looked up by build hash.
//!
//! The Gunner and Gun methods are slots of their classes' primary vtables.
//! The other functions and the attack-button continuation are found by byte
//! signature (`tools/sigs.py`'s encoding: rel32 targets and RIP displacements
//! wildcarded, struct offsets kept). Every hooked site must start with the
//! bytes its entry hook relocates, and the Gunner offsets are read from the
//! game's ammo-menu redraw, which calls the same methods. Each signature must
//! match exactly once, so a build where any of it moved or changed shape
//! resolves to an error and the plugin installs nothing.
use defiance_core::{
    decode,
    sites::{sig, Image, Signature},
};

const GUNNER: &str = ".?AVGunner@Leonardo@@";
const GUN: &str = ".?AVGun@Leonardo@@";

/// The Gunner reset of the shared target that `Gunner` vf8 calls.
const SHARED_REFRESH: Signature = sig(
    "shared-target refresh",
    &[("48895c24184889542410565741564883ec204c8bf2488bf9", 0)],
);
/// The target-owning helper an `AiMoveState` method calls.
const MOVE_ACQUIRE: Signature = sig(
    "move-target helper",
    &[(
        "48895424105553565741564157488bec4883ec384c8bfa488bf1c7453800000000",
        0,
    )],
);
/// `candidate_query(out, entity, mode)`, which reads the entity's AI facet
/// (vf0xb0, +0x28) and writes an enemy handle.
const CANDIDATE_QUERY: Signature = sig(
    "candidate query",
    &[("48895c240848897424185557415441564157488bec4883ec30", 0)],
);
/// `capable(entity, enemy)`, which reports whether any of the entity's
/// Gunners can engage.
const CAPABLE: Signature = sig(
    "gunner capability",
    &[(
        "48895c240848896c24104889742418574883ec20488b01488beaff90b0000000",
        0,
    )],
);
/// The target identity comparison.
const TARGET_EQUALS: Signature = sig(
    "target identity helper",
    &[("488954241048894c24085356574883ec20488bf2", 0)],
);
/// The owning target handle's release, the game's own destructor path.
const RELEASE: Signature = sig(
    "owning-target release helper",
    &[(
        "40534883ec20488bd9488b094885c974??488379100074??ff15????????90488b0b4885c974??834108ff75??488b01ff5010904883c4205bc3",
        0,
    )],
);
/// game.dll's attack-button availability check, after `mov rcx, [r14+0xe10]`;
/// the builds differ in the vtable slot it calls.
const UI: Signature = sig(
    "attack-button continuation",
    &[
        (
            "498b8e100e0000488b01498bd4ff905002000084c0743f498b8e100e0000",
            0,
        ),
        (
            "498b8e100e0000488b01498bd4ff904002000084c0743f498b8e100e0000",
            0,
        ),
    ],
);
/// Core's name for game.dll's ammo-menu redraw, which counts and fetches a
/// vehicle's Gunners; [`sites`] reads their vtable offsets from its body.
pub(crate) const REDRAW: &std::ffi::CStr = c"ammo menu redraw";

const SAVE_RBX_10: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x10];
const SAVE_RBX_18: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x18];
const SAVE_RBX_08: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x08];
const SAVE_RDX_10: &[u8] = &[0x48, 0x89, 0x54, 0x24, 0x10];
/// `mov rax, rsp; mov [rax+0x20], rbx`.
const FRAME_RAX: &[u8] = &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x20];
/// `mov rcx, [r14+0xe10]`.
const UI_BEFORE: &[u8] = &[0x49, 0x8b, 0x8e, 0x10, 0x0e, 0x00, 0x00];

/// Where a weapon-record guard sends a gun without a record.
enum Skip {
    /// The target of the `je rel32` this many bytes before the site: the
    /// branch's own exit when its aim source is missing.
    JeBefore(usize),
    /// The target of the `je rel32` this many bytes after the site: the
    /// method's `return false`.
    JeAfter(usize),
    /// This many bytes after the site, the end of the signature.
    At(usize),
}

/// One unchecked read of `[gun+0x48]` (see `guard.rs`). The signature starts
/// or ends with the branch around the read; the site holds the read's whole
/// instructions.
struct GuardSpec {
    signature: Signature,
    before: &'static [u8],
    /// The `Gun` slot the read lies in or after, and how far past its entry.
    slot: usize,
    within: usize,
    skip: Skip,
    /// The instructions the skip lands on.
    lands_on: &'static [u8],
}

/// The guarded reads, in [`crate::guard::detours`] order.
const GUARDS: [GuardSpec; 6] = [
    GuardSpec {
        signature: sig(
            "Gun vf10 mode 2 turn rate",
            &[(
                "0f84????????488b01ff90880000008b08898b040100008b4004898308010000488b4b28488b5348488b01488b92c0000000",
                0x20,
            )],
        ),
        // mov rcx, [rbx+0x28]; mov rdx, [rbx+0x48]; mov rax, [rcx];
        // mov rdx, [rdx+0xc0]
        before: &[
            0x48, 0x8b, 0x4b, 0x28, 0x48, 0x8b, 0x53, 0x48, 0x48, 0x8b, 0x01, 0x48, 0x8b, 0x92,
            0xc0, 0x00, 0x00, 0x00,
        ],
        slot: 10,
        within: 0x400,
        skip: Skip::JeBefore(0x20),
        // mov dl, 1
        lands_on: &[0xb2, 0x01],
    },
    GuardSpec {
        signature: sig(
            "Gun vf10 mode 0 turn rate",
            &[(
                "0f84????????488b01ff9088000000488b53488b08488b92c0000000",
                0xf,
            )],
        ),
        // mov rdx, [rbx+0x48]; mov ecx, [rax]; mov rdx, [rdx+0xc0]
        before: &[
            0x48, 0x8b, 0x53, 0x48, 0x8b, 0x08, 0x48, 0x8b, 0x92, 0xc0, 0x00, 0x00, 0x00,
        ],
        slot: 10,
        within: 0x400,
        skip: Skip::JeBefore(0xf),
        // movaps xmm1, xmm6; mov rcx, rbx
        lands_on: &[0x0f, 0x28, 0xce, 0x48, 0x8b, 0xcb],
    },
    GuardSpec {
        signature: sig(
            "Gun fire helper mode 1 turn rate",
            &[(
                "0f84????????488b01ff90880000008b08898b040100008b4004898308010000488b4b28488b01488b5348488b92c0000000",
                0x20,
            )],
        ),
        // mov rcx, [rbx+0x28]; mov rax, [rcx]; mov rdx, [rbx+0x48];
        // mov rdx, [rdx+0xc0]
        before: &[
            0x48, 0x8b, 0x4b, 0x28, 0x48, 0x8b, 0x01, 0x48, 0x8b, 0x53, 0x48, 0x48, 0x8b, 0x92,
            0xc0, 0x00, 0x00, 0x00,
        ],
        // The helper vf10 calls follows it.
        slot: 10,
        within: 0x1000,
        skip: Skip::JeBefore(0x20),
        // movss xmm0, [rbx+0x13c]
        lands_on: &[0xf3, 0x0f, 0x10, 0x83, 0x3c, 0x01, 0x00, 0x00],
    },
    GuardSpec {
        signature: sig(
            "Gun fire helper ranges",
            &[(
                "488b4b48488b81c0000000488945a7448845aff3440f1155b3f30f104210f30f104a14f30f105218f30f1175b7f30f117dbbf3440f1145bff30f1145c3f30f114dc7f30f1155cb488b81c8000000488945cf448845d7f3440f115ddb488b4b28488b01488d558fff9080000000488d55b7488b4b28488b01ff9080000000",
                0,
            )],
        ),
        // mov rcx, [rbx+0x48]; mov rax, [rcx+0xc0]
        before: &[
            0x48, 0x8b, 0x4b, 0x48, 0x48, 0x8b, 0x81, 0xc0, 0x00, 0x00, 0x00,
        ],
        slot: 10,
        within: 0x1000,
        // After both fire calls.
        skip: Skip::At(0x7e),
        // addss xmm9, [rbx+0xe4]
        lands_on: &[0xf3, 0x44, 0x0f, 0x58, 0x8b, 0xe4, 0x00, 0x00, 0x00],
    },
    GuardSpec {
        signature: sig(
            "Gun vf36 ranges",
            &[(
                "488b43484c8b88c80000004d85c97412488b4b28488b01488d542420ff50580fb6f0488b43484c8b88c00000004d85c97415488b4b28488b010f28d6488d542420ff50580fb6f84084f67409",
                0,
            )],
        ),
        // mov rax, [rbx+0x48]; mov r9, [rax+0xc8]
        before: &[
            0x48, 0x8b, 0x43, 0x48, 0x4c, 0x8b, 0x88, 0xc8, 0x00, 0x00, 0x00,
        ],
        slot: 36,
        within: 0x400,
        // The target of the closing `je +9`.
        skip: Skip::At(0x55),
        // xor al, al
        lands_on: &[0x32, 0xc0],
    },
    GuardSpec {
        signature: sig(
            "Gun vf56 ranges",
            &[(
                "488b4f28488b5748488b01488b92c0000000ff5040488b4f280fb6d8488b114c8b4240488b5748488b92c800000041ffd084db0f84????????",
                0,
            )],
        ),
        // mov rcx, [rdi+0x28]; mov rdx, [rdi+0x48]; mov rax, [rcx];
        // mov rdx, [rdx+0xc0]
        before: &[
            0x48, 0x8b, 0x4f, 0x28, 0x48, 0x8b, 0x57, 0x48, 0x48, 0x8b, 0x01, 0x48, 0x8b, 0x92,
            0xc0, 0x00, 0x00, 0x00,
        ],
        slot: 56,
        within: 0x400,
        skip: Skip::JeAfter(0x33),
        // xor al, al
        lands_on: &[0x32, 0xc0],
    },
];

/// One entry hook: the rva and the whole instructions it relocates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Site {
    pub rva: usize,
    pub before: &'static [u8],
}

/// The resolved sites, as rvas, and the Gunner vtable offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sites {
    pub tick: Site,
    pub deployment: Site,
    pub choose: Site,
    pub command: Site,
    pub setter: Site,
    pub shared_refresh: Site,
    pub move_acquire: Site,
    pub candidate_query: Site,
    pub capable: Site,
    /// The unchecked weapon-record reads, in [`crate::guard::detours`]
    /// order.
    pub gun_guards: [Guard; 6],
    /// In game.dll.
    pub ui: Site,
    /// Called, not hooked.
    pub query: usize,
    pub range: usize,
    pub target_equals: usize,
    pub release: usize,
    /// The vtable offsets of a vehicle AI's Gunner count and Gunner getter.
    pub gunner_count: usize,
    pub gunner_get: usize,
}

impl Sites {
    /// The logic.dll hooks.
    pub(crate) fn logic_hooks(&self) -> [Site; 15] {
        let [a, b, c, d, e, f] = self.gun_guards.map(|guard| guard.site);
        [
            self.tick,
            self.choose,
            self.deployment,
            self.command,
            self.setter,
            self.shared_refresh,
            self.move_acquire,
            self.candidate_query,
            self.capable,
            a,
            b,
            c,
            d,
            e,
            f,
        ]
    }
}

/// A weapon-record guard: the hooked read and the rva a gun without a record
/// resumes at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Guard {
    pub site: Site,
    pub skip: usize,
}

/// `spec`'s read in `logic`, which must lie within `spec.within` bytes past
/// the entry of `Gun` method `method`, and a skip just past it that lands on
/// `spec.lands_on`.
fn guard(logic: &Image, spec: &GuardSpec, method: usize) -> Result<Guard, String> {
    let name = spec.signature.name;
    let rva = logic.find(&spec.signature)?;
    if !rva
        .checked_sub(method)
        .is_some_and(|delta| delta < spec.within)
    {
        return Err(format!("{name} is outside its Gun method"));
    }
    let site = site(logic, name, rva, spec.before)?;
    let je = |at: usize| {
        logic
            .starts_with(at, &[0x0f, 0x84])
            .then(|| logic.rip_target(at + 2, at + 6))
            .flatten()
    };
    let skip = match spec.skip {
        Skip::JeBefore(back) => rva.checked_sub(back).and_then(je),
        Skip::JeAfter(ahead) => je(rva + ahead),
        Skip::At(ahead) => Some(rva + ahead),
    }
    .filter(|skip| (rva + spec.before.len()..rva + 0x200).contains(skip))
    .ok_or_else(|| format!("{name}: the skip target is out of place"))?;
    logic.expect(name, skip, spec.lands_on)?;
    Ok(Guard { site, skip })
}

/// `rva` as a hook site, if its code starts with `before` and those bytes are
/// whole, position-independent instructions.
fn site(image: &Image, what: &str, rva: usize, before: &'static [u8]) -> Result<Site, String> {
    image.expect(what, rva, before)?;
    decode::validate_copy(before).map_err(|e| format!("{what}: {e}"))?;
    Ok(Site { rva, before })
}

/// The resolved sites, or why this build is not supported. `redraw` is the
/// rva Core resolved for [`REDRAW`].
pub(crate) fn sites(logic: &Image, game: &Image, redraw: usize) -> Result<Sites, String> {
    let gunner = logic.primary_vtable(GUNNER)?;
    let gun = logic.primary_vtable(GUN)?;
    let method = |class: &str, methods: &[usize], slot: usize| {
        methods
            .get(slot)
            .copied()
            .ok_or_else(|| format!("{class} has no method {slot}"))
    };
    let gunner_method = |slot| method(GUNNER, &gunner.methods, slot);
    let gun_method = |slot| method(GUN, &gun.methods, slot);
    let found = |signature: &Signature, before| -> Result<Site, String> {
        site(logic, signature.name, logic.find(signature)?, before)
    };
    let mut gun_guards = Vec::with_capacity(GUARDS.len());
    for spec in &GUARDS {
        gun_guards.push(guard(logic, spec, gun_method(spec.slot)?)?);
    }
    let gun_guards: [Guard; 6] = gun_guards.try_into().expect("one guard per spec");
    let ui = game.find(&UI)?;
    // Each offset is the disp32 of `call [rax+disp32]` or
    // `mov r8, [rcx+disp32]` at a fixed place in the redraw.
    let field = |what: &str, rva: usize, opcode: &[u8]| -> Result<usize, String> {
        game.expect(what, rva, opcode)?;
        game.u32(rva + opcode.len())
            .map(|disp| disp as usize)
            .ok_or_else(|| format!("{what} runs off the image"))
    };
    Ok(Sites {
        tick: site(logic, "Gunner vf5", gunner_method(5)?, SAVE_RBX_10)?,
        deployment: site(logic, "Gunner vf40", gunner_method(40)?, SAVE_RBX_18)?,
        choose: site(logic, "Gunner vf13", gunner_method(13)?, SAVE_RBX_08)?,
        command: site(logic, "Gunner vf7", gunner_method(7)?, FRAME_RAX)?,
        setter: site(logic, "Gun vf5", gun_method(5)?, SAVE_RBX_18)?,
        shared_refresh: found(&SHARED_REFRESH, SAVE_RBX_18)?,
        move_acquire: found(&MOVE_ACQUIRE, SAVE_RDX_10)?,
        candidate_query: found(&CANDIDATE_QUERY, SAVE_RBX_08)?,
        capable: found(&CAPABLE, SAVE_RBX_08)?,
        gun_guards,
        ui: site(game, UI.name, ui, UI_BEFORE)?,
        query: gunner_method(12)?,
        range: gun_method(60)?,
        target_equals: logic.find(&TARGET_EQUALS)?,
        release: logic.find(&RELEASE)?,
        gunner_count: field("gunner count", redraw + 0x14b, b"\xff\x90")?,
        gunner_get: field("gunner getter", redraw + 0x169, b"\x4c\x8b\x81")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};

    /// The rvas and offsets the per-build table held before the plugin
    /// resolved them, in [`BUILDS`] order: tick, deployment, query, choose,
    /// command, setter, range, shared refresh, move acquire, candidate query,
    /// capable, ui, gunner count, gunner get.
    const TABLE: [[usize; 14]; 7] = [
        [
            0x2999c0, 0x2989a0, 0x2980d0, 0x2982b0, 0x297740, 0x28a030, 0x28f860, 0x297c20,
            0xe0350, 0x10a2c0, 0x10b2f0, 0x23f7df, 0x130, 0x120,
        ],
        [
            0x299a50, 0x298a30, 0x298160, 0x298340, 0x2977d0, 0x28a0c0, 0x28f8f0, 0x297cb0,
            0xe03e0, 0x10a350, 0x10b380, 0x2440cf, 0x130, 0x120,
        ],
        [
            0x2a7df0, 0x2a6dd0, 0x2a6500, 0x2a66e0, 0x2a5b70, 0x298590, 0x29ddc0, 0x2a6050,
            0xe82a0, 0x111010, 0x112600, 0x240aff, 0x140, 0x130,
        ],
        [
            0x2a7e80, 0x2a6e60, 0x2a6590, 0x2a6770, 0x2a5c00, 0x298620, 0x29de50, 0x2a60e0,
            0xe8330, 0x1110a0, 0x112690, 0x24541f, 0x140, 0x130,
        ],
        [
            0x2a7df0, 0x2a6dd0, 0x2a6500, 0x2a66e0, 0x2a5b70, 0x298590, 0x29ddc0, 0x2a6050,
            0xe82a0, 0x111010, 0x112600, 0x240aff, 0x140, 0x130,
        ],
        [
            0x2a7e80, 0x2a6e60, 0x2a6590, 0x2a6770, 0x2a5c00, 0x298620, 0x29de50, 0x2a60e0,
            0xe8330, 0x1110a0, 0x112690, 0x24541f, 0x140, 0x130,
        ],
        [
            0x2a8630, 0x2a7610, 0x2a6d40, 0x2a6f20, 0x2a63b0, 0x298dd0, 0x29e600, 0x2a6890,
            0xe82a0, 0x111010, 0x112600, 0x24ddbf, 0x140, 0x130,
        ],
    ];

    /// Core's "ammo menu redraw" rva per build, in [`BUILDS`] order.
    const REDRAW_RVAS: [usize; 8] = [
        0x3ed10, 0x3ed10, 0x3eeb0, 0x3eeb0, 0x3eeb0, 0x3eeb0, 0x3efc0, 0x3efc0,
    ];

    fn table(sites: &Sites) -> [usize; 14] {
        [
            sites.tick.rva,
            sites.deployment.rva,
            sites.query,
            sites.choose.rva,
            sites.command.rva,
            sites.setter.rva,
            sites.range,
            sites.shared_refresh.rva,
            sites.move_acquire.rva,
            sites.candidate_query.rva,
            sites.capable.rva,
            sites.ui.rva,
            sites.gunner_count,
            sites.gunner_get,
        ]
    }

    #[test]
    fn sites_resolve_where_the_build_table_had_them() {
        for ((build, expected), redraw) in BUILDS.iter().zip(TABLE).zip(REDRAW_RVAS) {
            let (Some(logic), Some(game)) =
                (reference(build, "logic.dll"), reference(build, "game.dll"))
            else {
                eprintln!("skipping {build}: no bin/{build} DLLs");
                continue;
            };
            let resolved = sites(&Image::mapped(&logic), &Image::mapped(&game), redraw)
                .unwrap_or_else(|error| panic!("{build}: {error}"));
            assert_eq!(table(&resolved), expected, "{build}");
        }
    }

    #[test]
    fn a_changed_prologue_or_signature_is_refused() {
        let build = BUILDS[0];
        let (Some(logic), Some(game)) =
            (reference(build, "logic.dll"), reference(build, "game.dll"))
        else {
            return;
        };
        let game = Image::mapped(&game);
        let resolved = sites(&Image::mapped(&logic), &game, REDRAW_RVAS[0]).unwrap();
        // A hooked Gunner method's prologue, then a byte inside a signature.
        for at in [resolved.tick.rva + 1, resolved.capable.rva + 0x18] {
            let mut changed = logic.image.clone();
            changed[at] ^= 1;
            let image = Image {
                image: &changed,
                base: logic.base,
            };
            assert!(sites(&image, &game, REDRAW_RVAS[0]).is_err(), "{at:#x}");
        }
    }

    /// The weapon-record guards per build, in [`BUILDS`] order, as (site,
    /// skip) in [`GUARDS`] order.
    const GUN_GUARDS: [[(usize, usize); 6]; 8] = [
        [
            (0x28b567, 0x28b6a9),
            (0x28b609, 0x28b683),
            (0x28b821, 0x28b88a),
            (0x28bafa, 0x28bb78),
            (0x28c028, 0x28c07d),
            (0x28af74, 0x28b0d4),
        ],
        [
            (0x28b5f7, 0x28b739),
            (0x28b699, 0x28b713),
            (0x28b8b1, 0x28b91a),
            (0x28bb8a, 0x28bc08),
            (0x28c0b8, 0x28c10d),
            (0x28b004, 0x28b164),
        ],
        [
            (0x299ac7, 0x299c09),
            (0x299b69, 0x299be3),
            (0x299d81, 0x299dea),
            (0x29a05a, 0x29a0d8),
            (0x29a588, 0x29a5dd),
            (0x2994d4, 0x299634),
        ],
        [
            (0x299b57, 0x299c99),
            (0x299bf9, 0x299c73),
            (0x299e11, 0x299e7a),
            (0x29a0ea, 0x29a168),
            (0x29a618, 0x29a66d),
            (0x299564, 0x2996c4),
        ],
        [
            (0x299ac7, 0x299c09),
            (0x299b69, 0x299be3),
            (0x299d81, 0x299dea),
            (0x29a05a, 0x29a0d8),
            (0x29a588, 0x29a5dd),
            (0x2994d4, 0x299634),
        ],
        [
            (0x299b57, 0x299c99),
            (0x299bf9, 0x299c73),
            (0x299e11, 0x299e7a),
            (0x29a0ea, 0x29a168),
            (0x29a618, 0x29a66d),
            (0x299564, 0x2996c4),
        ],
        [
            (0x29a307, 0x29a449),
            (0x29a3a9, 0x29a423),
            (0x29a5c1, 0x29a62a),
            (0x29a89a, 0x29a918),
            (0x29adc8, 0x29ae1d),
            (0x299d14, 0x299e74),
        ],
        [
            (0x29a397, 0x29a4d9),
            (0x29a439, 0x29a4b3),
            (0x29a651, 0x29a6ba),
            (0x29a92a, 0x29a9a8),
            (0x29ae58, 0x29aead),
            (0x299da4, 0x299f04),
        ],
    ];

    #[test]
    fn the_gun_guards_resolve_on_every_build() {
        for ((build, expected), redraw) in BUILDS.iter().zip(GUN_GUARDS).zip(REDRAW_RVAS) {
            let (Some(logic), Some(game)) =
                (reference(build, "logic.dll"), reference(build, "game.dll"))
            else {
                eprintln!("skipping {build}: no bin/{build} DLLs");
                continue;
            };
            let resolved = sites(&Image::mapped(&logic), &Image::mapped(&game), redraw)
                .unwrap_or_else(|error| panic!("{build}: {error}"));
            let guarded = resolved
                .gun_guards
                .map(|guard| (guard.site.rva, guard.skip));
            assert_eq!(guarded, expected, "{build}");
        }
    }

    #[test]
    fn a_changed_guarded_read_is_refused() {
        let build = BUILDS[0];
        let (Some(logic), Some(game)) =
            (reference(build, "logic.dll"), reference(build, "game.dll"))
        else {
            return;
        };
        let game = Image::mapped(&game);
        // A byte of each read, then of each skip's landing.
        let reads = GUN_GUARDS[0].map(|(site, _)| site + 2);
        let landings = GUN_GUARDS[0].map(|(_, skip)| skip + 1);
        for at in reads.into_iter().chain(landings) {
            let mut changed = logic.image.clone();
            changed[at] ^= 1;
            let image = Image {
                image: &changed,
                base: logic.base,
            };
            assert!(sites(&image, &game, REDRAW_RVAS[0]).is_err(), "{at:#x}");
        }
    }

    #[test]
    fn invalid_images_are_refused() {
        let empty = Image {
            image: &[],
            base: 0x180000000,
        };
        assert!(sites(&empty, &empty, 0).is_err());
    }
}
