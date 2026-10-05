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
/// game.dll's ammo-menu redraw, which counts and fetches a vehicle's Gunners.
const REDRAW: Signature = sig(
    "ammo menu redraw",
    &[("488954241048894c24085741544881ece8000000488b02488bf9", 0)],
);

const SAVE_RBX_10: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x10];
const SAVE_RBX_18: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x18];
const SAVE_RBX_08: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x08];
const SAVE_RDX_10: &[u8] = &[0x48, 0x89, 0x54, 0x24, 0x10];
/// `mov rax, rsp; mov [rax+0x20], rbx`.
const FRAME_RAX: &[u8] = &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x20];
/// `mov rcx, [r14+0xe10]`.
const UI_BEFORE: &[u8] = &[0x49, 0x8b, 0x8e, 0x10, 0x0e, 0x00, 0x00];

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
    /// The logic.dll entry hooks.
    pub(crate) fn logic_hooks(&self) -> [Site; 9] {
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
        ]
    }
}

/// `rva` as a hook site, if its code starts with `before` and those bytes are
/// whole, position-independent instructions.
fn site(image: &Image, what: &str, rva: usize, before: &'static [u8]) -> Result<Site, String> {
    image.expect(what, rva, before)?;
    decode::validate_copy(before).map_err(|e| format!("{what}: {e}"))?;
    Ok(Site { rva, before })
}

/// The resolved sites, or why this build is not supported.
pub(crate) fn sites(logic: &Image, game: &Image) -> Result<Sites, String> {
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
    let ui = game.find(&UI)?;
    // Each offset is the disp32 of `call [rax+disp32]` or
    // `mov r8, [rcx+disp32]` at a fixed place in the redraw.
    let redraw = game.find(&REDRAW)?;
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
    const TABLE: [[usize; 14]; 6] = [
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
        for (build, expected) in BUILDS.iter().zip(TABLE) {
            let (Some(logic), Some(game)) =
                (reference(build, "logic.dll"), reference(build, "game.dll"))
            else {
                eprintln!("skipping {build}: no bin/{build} DLLs");
                continue;
            };
            let resolved = sites(&Image::mapped(&logic), &Image::mapped(&game))
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
        let resolved = sites(&Image::mapped(&logic), &game).unwrap();
        // A hooked Gunner method's prologue, then a byte inside a signature.
        for at in [resolved.tick.rva + 1, resolved.capable.rva + 0x18] {
            let mut changed = logic.image.clone();
            changed[at] ^= 1;
            let image = Image {
                image: &changed,
                base: logic.base,
            };
            assert!(sites(&image, &game).is_err(), "{at:#x}");
        }
    }

    #[test]
    fn invalid_images_are_refused() {
        let empty = Image {
            image: &[],
            base: 0x180000000,
        };
        assert!(sites(&empty, &empty).is_err());
    }
}
