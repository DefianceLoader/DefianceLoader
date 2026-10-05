//! Where the hooked, called and compared native code lives, found by signature
//! and RTTI instead of looked up by build hash.
//!
//! Functions with a stable vtable slot are read from the primary vtable of
//! their RTTI class; the rest are found by byte signature (`tools/sigs.py`'s
//! encoding: rel32 targets and RIP displacements wildcarded, struct offsets
//! kept). Return addresses the hooks compare against are found by the
//! signature of the code around them. Every site is then checked against the
//! bytes the hooks relocate or the plugin relies on. Each must resolve to
//! exactly one place, so a build where any site moved or changed shape resolves
//! to an error and the plugin hooks nothing.
use defiance_core::sites::{sig, Image, Signature};

/// The logic.dll sites, as rvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sites {
    /// HumanChassisFacet stop (vtable slot 22).
    pub stop: usize,
    /// HumanChassisFacet turn toward a point (slot 31).
    pub turn_point: usize,
    /// HumanChassisFacet turn toward a direction (slot 30).
    pub turn_direction: usize,
    /// Animation action queue.
    pub queue: usize,
    /// AiHumanMoveState order initialization (slot 16).
    pub order: usize,
    /// AiAttackWithMoveState order initialization (slot 16).
    pub order_attack_move: usize,
    /// Gunner weapon selection.
    pub select: usize,
    /// Gun eligibility (slot 56).
    pub weapon_eligible: usize,
    /// Gun idle requirement (slot 62).
    pub weapon_requires_idle: usize,
    /// Chassis steering helper shared by the two turns.
    pub steer: usize,
    /// Gunner target candidate.
    pub candidate: usize,
    /// HumanChassisFacet can-move query (slot 29).
    pub chassis_can_move: usize,
    /// Navigation manager cancellation body, past its two bounds checks.
    pub cancel_trace: usize,
    /// HumanAiFacet order submission (slot 12).
    pub submit: usize,
    /// AiStopOrder initialization (slot 6).
    pub stop_order_init: usize,
    /// AI facet update.
    pub ai_update: usize,
    /// Copies a strong reference.
    pub copy_ref: usize,
    /// Releases a junction reference.
    pub release_ref: usize,
    /// Binds a PersistentBase object into an owned junction.
    pub bind_junction: usize,
    /// Builds an attack-move order.
    pub attack_move_order_factory: usize,
    /// Builds a move order.
    pub move_order_factory: usize,
    /// The move and attack orders' set-flags method (slot 13).
    pub set_flags: usize,
    /// FlareTarget method 5, which returns `this + 0x10`.
    pub flare_target_slot: usize,
    /// The attack target's method 5.
    pub target_getter: usize,
    /// HumanChassisFacet movement resume (slot 41).
    pub native_resume: usize,
    /// HumanGunner and HumanGunnerClient method 11.
    pub gunner_slot: usize,
    /// Return address of the point turn's steering call.
    pub turn_return: usize,
    /// Return address of the movement update's steering call.
    pub move_return: usize,
    /// Return address of the gunner's order release.
    pub release_order_return: usize,
    /// Return address inside the gunner's aim update.
    pub aim_return: usize,
    /// Return address of the candidate gate check.
    pub candidate_gate_return: usize,
    /// The AiAttackOrder vtable.
    pub attack_order_vt: usize,
    /// The AiAttackState vtable.
    pub attack_state_vt: usize,
    /// The AiMoveOrder vtable.
    pub move_order_vt: usize,
    /// The AiStopOrder vtable.
    pub stop_order_vt: usize,
    /// The FlareTarget vtable.
    pub flare_target_vt: usize,
    /// The HumanAnimationFacet vtable.
    pub animation_vt: usize,
    /// The HumanChassisFacet vtable.
    pub chassis_vt: usize,
    /// The HumanGunner vtable.
    pub gunner_vt: usize,
    /// The HumanGunnerClient vtable.
    pub gunner_client_vt: usize,
}

impl Sites {
    /// The hooked entries, in the order of [`HOOK_ENTRIES`].
    pub fn hooks(&self) -> [usize; 16] {
        [
            self.stop,
            self.turn_point,
            self.turn_direction,
            self.queue,
            self.order,
            self.order_attack_move,
            self.select,
            self.weapon_eligible,
            self.weapon_requires_idle,
            self.steer,
            self.candidate,
            self.chassis_can_move,
            self.cancel_trace,
            self.submit,
            self.stop_order_init,
            self.ai_update,
        ]
    }
}

const COPY_REF: Signature = sig(
    "strong reference copy",
    &[(
        "40534883ec20488bd9488b0a48890b4885c974148b51088d420189410885d27507488b01ff500890488b03",
        0x0,
    )],
);
const RELEASE_REF: Signature = sig(
    "junction release",
    &[("40534883ec20488bd9488b094885c9740e48837910007407", 0x0)],
);
const BIND_JUNCTION: Signature = sig(
    "junction bind",
    &[(
        "48895c24104889742418574883ec20488bc2488bf133c9894c2430",
        0x0,
    )],
);
const ATTACK_MOVE_ORDER_FACTORY: Signature = sig(
    "attack-move order factory",
    &[(
        "48895c24104c8944241848894c24085556574883ec30498bf8488bea488bf1488bd1b938000000",
        0x0,
    )],
);
const MOVE_ORDER_FACTORY: Signature = sig(
    "move order factory",
    &[(
        "48895c241048896c2418488974242048894c2408574883ec500f29742440",
        0x0,
    )],
);
const AI_UPDATE: Signature = sig(
    "AI facet update",
    &[(
        "48895c241048896c2420565741564883ec500f297424400f297c2430",
        0x0,
    )],
);
const QUEUE: Signature = sig(
    "animation action queue",
    &[("48895c2408574883ec2048837928008bfa488bd90f85????????", 0x0)],
);
const STEER: Signature = sig(
    "chassis steering helper",
    &[("48895c2408574883ec30488db9840000000f29742420488bd9", 0x0)],
);
const AIM_RETURN: Signature = sig(
    "gunner aim return",
    &[("410f2ff97209440f28cff3450f5dca410f2ff0720d440f28c6", 0x0)],
);
const SELECT: Signature = sig(
    "gunner weapon selection",
    &[("405341544883ec384532e4488bd94883b980000000007518", 0x0)],
);
const RELEASE_ORDER_RETURN: Signature = sig(
    "gunner order release return",
    &[(
        "0f57d2488d5567488bcbffd7498b06b201498bceff5058488b9c2450010000",
        0xc,
    )],
);
const CANDIDATE: Signature = sig(
    "gunner target candidate",
    &[(
        "48895c241048896c2418488974242057415641574883ec300f29742420",
        0x0,
    )],
);
const CANDIDATE_GATE_RETURN: Signature = sig(
    "candidate gate return",
    &[("84c00f84????????488b4f38488b34e933db488b4740482bc1", 0x0)],
);
const CANCEL_TRACE: Signature = sig("navigation manager cancellation", &[("85d20f88????????4c8b8188020000413b50040f8d????????4863c233d24869c8f002000049034808c6410300488991d0020000", 0x19)]);
const TARGET_GETTER: Signature = sig(
    "attack target getter",
    &[("40534883ec2080791000488bd97533488b41284885c0742a", 0x0)],
);
const TURN_STEER_CALL: Signature = sig(
    "point turn steering call",
    &[("e8????????88870b010000eb15f30f108730010000f3410f5806", 0)],
);
const MOVE_STEER_CALL: Signature = sig(
    "movement update steering call",
    &[("e8????????488b4b58837968030f84????????ba03000000", 0)],
);

const ATTACK_ORDER: &str = ".?AVAiAttackOrder@Leonardo@@";
const ATTACK_STATE: &str = ".?AVAiAttackState@Leonardo@@";
const MOVE_ORDER: &str = ".?AVAiMoveOrder@Leonardo@@";
const STOP_ORDER: &str = ".?AVAiStopOrder@Leonardo@@";
const FLARE_TARGET: &str = ".?AVFlareTarget@Leonardo@@";
const ANIMATION: &str = ".?AVHumanAnimationFacet@Leonardo@@";
const CHASSIS: &str = ".?AVHumanChassisFacet@Leonardo@@";
const GUNNER: &str = ".?AVHumanGunner@Leonardo@@";
const GUNNER_CLIENT: &str = ".?AVHumanGunnerClient@Leonardo@@";
const HUMAN_MOVE_STATE: &str = ".?AVAiHumanMoveState@Leonardo@@";
const ATTACK_WITH_MOVE_STATE: &str = ".?AVAiAttackWithMoveState@Leonardo@@";
const GUN: &str = ".?AVGun@Leonardo@@";
const AI: &str = ".?AVHumanAiFacet@Leonardo@@";

/// The entry bytes each hook relocates, in detour order.
pub(crate) const HOOK_ENTRIES: [&[u8]; 16] = [
    b"\x48\x89\x5c\x24\x08\x57\x48\x83\xec\x20\x0f\xb6\xfa\x48\x8b\xd9",
    b"\x48\x89\x5c\x24\x08\x57\x48\x83\xec\x20\x48\x8b\xfa\x48\x8b\xd9",
    b"\x48\x89\x5c\x24\x08\x57\x48\x83\xec\x20\x48\x8b\xfa\x48\x8b\xd9",
    b"\x48\x89\x5c\x24\x08\x57\x48\x83\xec\x20\x48\x83\x79\x28\x00\x8b",
    b"\x48\x89\x54\x24\x10\x55\x53\x56\x57\x41\x56\x48\x8b\xec\x48\x83\xec\x20",
    b"\x48\x89\x5c\x24\x08\x48\x89\x54\x24\x10\x55\x56\x57\x41\x54\x41\x55\x41\x56\x41",
    b"\x40\x53\x41\x54\x48\x83\xec\x38\x45\x32\xe4\x48\x8b\xd9\x48\x83",
    b"\x48\x89\x5c\x24\x08\x48\x89\x74\x24\x10\x57\x48\x81\xec\xe0\x00\x00",
    b"\x40\x53\x48\x83\xec\x20\x48\x8b\x41\x40\x48\x8b\xd9\x8b\x90\xc4",
    b"\x48\x89\x5c\x24\x08\x57\x48\x83\xec\x30\x48\x8d\xb9\x84\x00\x00\x00",
    b"\x48\x89\x5c\x24\x10\x48\x89\x6c\x24\x18\x48\x89\x74\x24\x20\x57",
    b"\x40\x53\x48\x83\xec\x20\x8b\x91\xc4\x00\x00\x00\x48\x8b\xd9\x83\xfa",
    b"\x48\x63\xc2\x33\xd2\x48\x69\xc8\xf0\x02\x00\x00\x49\x03\x48\x08",
    b"\x48\x89\x5c\x24\x18\x48\x89\x54\x24\x10\x57\x48\x83\xec\x30\x0f",
    b"\x48\x89\x5c\x24\x18\x48\x89\x74\x24\x20\x41\x56\x48\x83\xec\x20\x48",
    b"\x48\x89\x5c\x24\x10\x48\x89\x6c\x24\x20\x56\x57\x41\x56\x48\x83\xec\x50\x0f\x29",
];
/// The bounds checks in front of [`Sites::cancel_trace`], which the hook relies
/// on having run.
const CANCEL_BOUNDS: &[u8] = b"\x85\xd2\x0f\x88\x9a\x00\x00\x00\x4c\x8b\x81\x88\x02\x00\x00\x41\x3b\x50\x04\x0f\x8d\x89\x00\x00\x00";

/// The sites, or why this build is not supported.
pub(crate) fn sites(image: &Image) -> Result<Sites, String> {
    let primary = |class: &str| image.primary_vtable(class).map(|vtable| vtable.methods_at);
    let steer = image.find(&STEER)?;
    let steer_return = |signature: &Signature| -> Result<usize, String> {
        let call = image.call(signature)?;
        if image.branch_target(call) != Some(steer) {
            return Err(format!(
                "{} does not call the steering helper",
                signature.name
            ));
        }
        Ok(call + 5)
    };
    let sites = Sites {
        stop: image.method(CHASSIS, 22)?,
        turn_point: image.method(CHASSIS, 31)?,
        turn_direction: image.method(CHASSIS, 30)?,
        queue: image.find(&QUEUE)?,
        order: image.method(HUMAN_MOVE_STATE, 16)?,
        order_attack_move: image.method(ATTACK_WITH_MOVE_STATE, 16)?,
        select: image.find(&SELECT)?,
        weapon_eligible: image.method(GUN, 56)?,
        weapon_requires_idle: image.method(GUN, 62)?,
        steer,
        candidate: image.find(&CANDIDATE)?,
        chassis_can_move: image.method(CHASSIS, 29)?,
        cancel_trace: image.find(&CANCEL_TRACE)?,
        submit: image.method(AI, 12)?,
        stop_order_init: image.method(STOP_ORDER, 6)?,
        ai_update: image.find(&AI_UPDATE)?,
        copy_ref: image.find(&COPY_REF)?,
        release_ref: image.find(&RELEASE_REF)?,
        bind_junction: image.find(&BIND_JUNCTION)?,
        attack_move_order_factory: image.find(&ATTACK_MOVE_ORDER_FACTORY)?,
        move_order_factory: image.find(&MOVE_ORDER_FACTORY)?,
        set_flags: image.method(MOVE_ORDER, 13)?,
        flare_target_slot: image.method(FLARE_TARGET, 5)?,
        target_getter: image.find(&TARGET_GETTER)?,
        native_resume: image.method(CHASSIS, 41)?,
        gunner_slot: image.method(GUNNER, 11)?,
        turn_return: steer_return(&TURN_STEER_CALL)?,
        move_return: steer_return(&MOVE_STEER_CALL)?,
        release_order_return: image.find(&RELEASE_ORDER_RETURN)?,
        aim_return: image.find(&AIM_RETURN)?,
        candidate_gate_return: image.find(&CANDIDATE_GATE_RETURN)?,
        attack_order_vt: primary(ATTACK_ORDER)?,
        attack_state_vt: primary(ATTACK_STATE)?,
        move_order_vt: primary(MOVE_ORDER)?,
        stop_order_vt: primary(STOP_ORDER)?,
        flare_target_vt: primary(FLARE_TARGET)?,
        animation_vt: primary(ANIMATION)?,
        chassis_vt: primary(CHASSIS)?,
        gunner_vt: primary(GUNNER)?,
        gunner_client_vt: primary(GUNNER_CLIENT)?,
    };
    // The plugin compares one slot against both order classes and both
    // gunner classes.
    if image.method(ATTACK_ORDER, 13)? != sites.set_flags {
        return Err("the attack order has a different set-flags method".into());
    }
    if image.method(GUNNER_CLIENT, 11)? != sites.gunner_slot {
        return Err("the client gunner has a different method 11".into());
    }
    for ((rva, bytes), what) in sites.hooks().into_iter().zip(HOOK_ENTRIES).zip(HOOK_NAMES) {
        image.expect(what, rva, bytes)?;
    }
    let others: [(&str, usize, &[u8]); 16] = [
        (
            "copy ref",
            sites.copy_ref,
            b"\x40\x53\x48\x83\xec\x20\x48\x8b\xd9\x48\x8b\x0a\x48\x89\x0b\x48",
        ),
        (
            "release ref",
            sites.release_ref,
            b"\x40\x53\x48\x83\xec\x20\x48\x8b\xd9",
        ),
        (
            "bind junction",
            sites.bind_junction,
            b"\x48\x89\x5c\x24\x10\x48\x89\x74\x24\x18\x57\x48\x83\xec\x20\x48",
        ),
        (
            "attack move order factory",
            sites.attack_move_order_factory,
            b"\x48\x89\x5c\x24\x10\x4c\x89\x44\x24\x18\x48\x89\x4c\x24\x08\x55",
        ),
        (
            "move order factory",
            sites.move_order_factory,
            b"\x48\x89\x5c\x24\x10\x48\x89\x6c\x24\x18\x48\x89\x74\x24\x20\x48",
        ),
        ("set flags", sites.set_flags, b"\x09\x51\x14\xc3"),
        (
            "flare target slot",
            sites.flare_target_slot,
            b"\x48\x8d\x41\x10\xc3",
        ),
        (
            "target getter",
            sites.target_getter,
            b"\x40\x53\x48\x83\xec\x20\x80\x79\x10\x00\x48\x8b\xd9\x75\x33\x48",
        ),
        (
            "native resume",
            sites.native_resume,
            b"\x48\x8b\xc4\x48\x89\x58\x18\x48\x89\x50\x10\x55\x56\x57\x41\x54",
        ),
        (
            "gunner slot",
            sites.gunner_slot,
            b"\x48\x89\x5c\x24\x10\x48\x89\x74\x24\x18\x57\x48\x83\xec\x20",
        ),
        (
            "turn return",
            sites.turn_return,
            b"\x88\x87\x0b\x01\x00\x00\xeb\x15\xf3\x0f\x10\x87\x30\x01\x00\x00",
        ),
        (
            "move return",
            sites.move_return,
            b"\x48\x8b\x4b\x58\x83\x79\x68\x03\x0f\x84\xc5\x00\x00\x00\xba\x03",
        ),
        (
            "release order return",
            sites.release_order_return,
            b"\x49\x8b\x06\xb2\x01\x49\x8b\xce\xff\x50\x58\x48\x8b\x9c\x24\x50",
        ),
        (
            "aim return",
            sites.aim_return,
            b"\x41\x0f\x2f\xf9\x72\x09\x44\x0f\x28\xcf\xf3\x45\x0f\x5d\xca\x41",
        ),
        (
            "candidate gate return",
            sites.candidate_gate_return,
            b"\x84\xc0\x0f\x84\xcb\x02\x00\x00",
        ),
        (
            "cancellation bounds",
            sites.cancel_trace - 0x19,
            CANCEL_BOUNDS,
        ),
    ];
    for (what, rva, bytes) in others {
        image.expect(what, rva, bytes)?;
    }
    Ok(sites)
}

const HOOK_NAMES: [&str; 16] = [
    "stop",
    "turn point",
    "turn direction",
    "queue",
    "order",
    "order attack move",
    "select",
    "weapon eligible",
    "weapon requires idle",
    "steer",
    "candidate",
    "chassis can move",
    "cancel trace",
    "submit",
    "stop order init",
    "ai update",
];

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS, SEPTEMBER_BUILDS};

    /// The rvas the per-build hash table held before the plugin resolved them,
    /// in [`SEPTEMBER_BUILDS`] order.
    const TABLE: [(&str, Sites); 4] = [
        (
            "gog/2026-09-14",
            Sites {
                stop: 0x2cab20,
                turn_point: 0x2cc950,
                turn_direction: 0x2cca60,
                queue: 0x2c2860,
                order: 0xd93e0,
                order_attack_move: 0x9e8d0,
                select: 0x2d82c0,
                weapon_eligible: 0x2992a0,
                weapon_requires_idle: 0x299650,
                steer: 0x2ccb50,
                candidate: 0x2d95b0,
                chassis_can_move: 0x2cc900,
                cancel_trace: 0x3331a9,
                submit: 0x2baf10,
                stop_order_init: 0x1031c0,
                ai_update: 0x2bc370,
                copy_ref: 0x123f0,
                release_ref: 0x24070,
                bind_junction: 0x240c0,
                attack_move_order_factory: 0x8a030,
                move_order_factory: 0x114f20,
                set_flags: 0x76260,
                flare_target_slot: 0x116cf0,
                target_getter: 0x46a070,
                native_resume: 0x2cb710,
                gunner_slot: 0x2d9d30,
                turn_return: 0x2c922c,
                move_return: 0x2ca06c,
                release_order_return: 0x2d90aa,
                aim_return: 0x2d37f0,
                candidate_gate_return: 0x2d96b4,
                attack_order_vt: 0x704cf0,
                attack_state_vt: 0x704da8,
                move_order_vt: 0x70aff8,
                stop_order_vt: 0x70d560,
                flare_target_vt: 0x70e3c0,
                animation_vt: 0x72b0d8,
                chassis_vt: 0x72b370,
                gunner_vt: 0x72bcc0,
                gunner_client_vt: 0x72be78,
            },
        ),
        (
            "steam/2026-09-22",
            Sites {
                stop: 0x2cabb0,
                turn_point: 0x2cc9e0,
                turn_direction: 0x2ccaf0,
                queue: 0x2c28f0,
                order: 0xd9470,
                order_attack_move: 0x9e960,
                select: 0x2d8350,
                weapon_eligible: 0x299330,
                weapon_requires_idle: 0x2996e0,
                steer: 0x2ccbe0,
                candidate: 0x2d9640,
                chassis_can_move: 0x2cc990,
                cancel_trace: 0x333239,
                submit: 0x2bafa0,
                stop_order_init: 0x103250,
                ai_update: 0x2bc400,
                copy_ref: 0x123f0,
                release_ref: 0x24070,
                bind_junction: 0x240c0,
                attack_move_order_factory: 0x8a0c0,
                move_order_factory: 0x114fb0,
                set_flags: 0x762f0,
                flare_target_slot: 0x116d80,
                target_getter: 0x46a100,
                native_resume: 0x2cb7a0,
                gunner_slot: 0x2d9dc0,
                turn_return: 0x2c92bc,
                move_return: 0x2ca0fc,
                release_order_return: 0x2d913a,
                aim_return: 0x2d3880,
                candidate_gate_return: 0x2d9744,
                attack_order_vt: 0x704d50,
                attack_state_vt: 0x704f08,
                move_order_vt: 0x70b068,
                stop_order_vt: 0x70d5d8,
                flare_target_vt: 0x70e430,
                animation_vt: 0x72b118,
                chassis_vt: 0x72b3b0,
                gunner_vt: 0x72bd00,
                gunner_client_vt: 0x72bed0,
            },
        ),
        (
            "gog/2026-09-25",
            Sites {
                stop: 0x2cab20,
                turn_point: 0x2cc950,
                turn_direction: 0x2cca60,
                queue: 0x2c2860,
                order: 0xd93e0,
                order_attack_move: 0x9e8d0,
                select: 0x2d82c0,
                weapon_eligible: 0x2992a0,
                weapon_requires_idle: 0x299650,
                steer: 0x2ccb50,
                candidate: 0x2d95b0,
                chassis_can_move: 0x2cc900,
                cancel_trace: 0x3331a9,
                submit: 0x2baf10,
                stop_order_init: 0x1031c0,
                ai_update: 0x2bc370,
                copy_ref: 0x123f0,
                release_ref: 0x24070,
                bind_junction: 0x240c0,
                attack_move_order_factory: 0x8a030,
                move_order_factory: 0x114f20,
                set_flags: 0x76260,
                flare_target_slot: 0x116cf0,
                target_getter: 0x46a6b0,
                native_resume: 0x2cb710,
                gunner_slot: 0x2d9d30,
                turn_return: 0x2c922c,
                move_return: 0x2ca06c,
                release_order_return: 0x2d90aa,
                aim_return: 0x2d37f0,
                candidate_gate_return: 0x2d96b4,
                attack_order_vt: 0x705cf0,
                attack_state_vt: 0x705da8,
                move_order_vt: 0x70bff8,
                stop_order_vt: 0x70e560,
                flare_target_vt: 0x70f3c0,
                animation_vt: 0x72c0d8,
                chassis_vt: 0x72c370,
                gunner_vt: 0x72ccc0,
                gunner_client_vt: 0x72ce78,
            },
        ),
        (
            "steam/2026-09-25",
            Sites {
                stop: 0x2cabb0,
                turn_point: 0x2cc9e0,
                turn_direction: 0x2ccaf0,
                queue: 0x2c28f0,
                order: 0xd9470,
                order_attack_move: 0x9e960,
                select: 0x2d8350,
                weapon_eligible: 0x299330,
                weapon_requires_idle: 0x2996e0,
                steer: 0x2ccbe0,
                candidate: 0x2d9640,
                chassis_can_move: 0x2cc990,
                cancel_trace: 0x333239,
                submit: 0x2bafa0,
                stop_order_init: 0x103250,
                ai_update: 0x2bc400,
                copy_ref: 0x123f0,
                release_ref: 0x24070,
                bind_junction: 0x240c0,
                attack_move_order_factory: 0x8a0c0,
                move_order_factory: 0x114fb0,
                set_flags: 0x762f0,
                flare_target_slot: 0x116d80,
                target_getter: 0x46a740,
                native_resume: 0x2cb7a0,
                gunner_slot: 0x2d9dc0,
                turn_return: 0x2c92bc,
                move_return: 0x2ca0fc,
                release_order_return: 0x2d913a,
                aim_return: 0x2d3880,
                candidate_gate_return: 0x2d9744,
                attack_order_vt: 0x705d50,
                attack_state_vt: 0x705f08,
                move_order_vt: 0x70c068,
                stop_order_vt: 0x70e5d8,
                flare_target_vt: 0x70f430,
                animation_vt: 0x72c118,
                chassis_vt: 0x72c3b0,
                gunner_vt: 0x72cd00,
                gunner_client_vt: 0x72ced0,
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

    /// The December 2025 builds lack the point-turn steering call the plugin
    /// relies on, so they are not supported.
    #[test]
    fn the_december_builds_are_refused() {
        for build in BUILDS.into_iter().filter(|b| !SEPTEMBER_BUILDS.contains(b)) {
            if let Some(mapped) = reference(build, "logic.dll") {
                assert!(sites(&Image::mapped(&mapped)).is_err(), "{build}");
            }
        }
    }

    #[test]
    fn a_changed_entry_is_refused() {
        let (build, expected) = TABLE[0];
        let Some(mapped) = reference(build, "logic.dll") else {
            return;
        };
        let mut changed = mapped.image.clone();
        // The bounds check's rel32 is wildcarded in the signature, so only the
        // byte check catches this.
        changed[expected.cancel_trace - 0x19 + 4] ^= 0x01;
        let image = Image {
            image: &changed,
            base: mapped.base,
        };
        assert!(sites(&image).is_err());
    }
}
