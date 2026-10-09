//! Where the hooked and called logic.dll code lives, found by signature and
//! RTTI instead of looked up by build hash.
//!
//! The hooked entries, the call sites and the called helpers are found by
//! byte signature (`tools/site_signatures.py`'s encoding); the squad manager
//! and holder getters are the targets of calls inside the spawn and the
//! damage callback, and the human gunner vtable is its RTTI class's primary
//! vtable. The stock functions the replacements reimplement or rely on are
//! required to resolve too, and each hooked call must still call the function
//! the plugin chains to. Each must resolve to exactly one place and every
//! hooked entry must start with the bytes its hook relocates, so a build where
//! any of them moved or changed shape resolves to an error and the plugin
//! writes nothing.
use defiance_api::{PATCH_KIND_CALL, PATCH_KIND_ENTRY};
use defiance_core::sites::{sig, Image, Signature};

/// The logic.dll sites, as rvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sites {
    /// The damage callback's call to the original death handler, hooked as a
    /// call.
    pub death_call: usize,
    /// The squad pickup collection, hooked at entry.
    pub collect: usize,
    /// The collection's call to the special-weapon drop, hooked as a call.
    pub collect_drop: usize,
    /// The collection's call to the pickup add, hooked as a call.
    pub collect_add: usize,
    /// The squad cache rebuild, hooked at entry.
    pub rebuild: usize,
    /// The ammo reserve save writer, hooked at entry.
    pub reserve_writer: usize,
    /// The ammo reserve load, hooked at entry.
    pub reserve_load: usize,
    /// The ammo reserve destructor, hooked at entry.
    pub reserve_destroy: usize,
    pub string_copy: usize,
    pub string_destroy: usize,
    pub detach: usize,
    pub ammo_dispose: usize,
    pub primary_add: usize,
    pub ammo_mode: usize,
    pub ammo_remove: usize,
    pub import_rounds: usize,
    pub ammo_set_record: usize,
    pub weak_bind: usize,
    pub visual_switch: usize,
    pub visual_sync: usize,
    pub spawn: usize,
    pub slot_type: usize,
    pub slot_context: usize,
    pub item_override: usize,
    pub canonical: usize,
    /// The squad manager getter the spawn calls.
    pub manager_get: usize,
    /// The weapon holder getter the damage callback calls.
    pub holder_get: usize,
    /// HumanGunner's primary vtable.
    pub human_vtable: usize,
}

const DEATH_CALL: Signature = sig(
    "death call",
    &[(
        "e8????????c64718004c8b7d68488d5f5c833b007e23498b542438",
        0x0,
    )],
);
const COLLECT: Signature = sig(
    "squad pickup collection",
    &[(
        "48895c240848896c241048897424205741544155415641574883ec40",
        0x0,
    )],
);
const COLLECT_DROP: Signature = sig(
    "collection drop call",
    &[("e8????????4c8d442420498bd6488bcde8????????488b5e28", 0x0)],
);
const COLLECT_ADD: Signature = sig(
    "collection add call",
    &[("e8????????488b5e28488bc3482b46204c3bf80f84????????", 0x0)],
);
const REBUILD: Signature = sig(
    "squad cache rebuild",
    &[(
        "48894c2408555356574154415541564157488d6c24e14881ecd8000000488bf1",
        0x0,
    )],
);
const RESERVE_WRITER: Signature = sig(
    "reserve writer",
    &[("48895c241048896c2418565741564883ec20488b02488bf1", 0x0)],
);
const RESERVE_LOAD: Signature = sig(
    "reserve load",
    &[(
        "488954241048894c2408555356574154415541564157488dac24e8fdffff",
        0x0,
    )],
);
const RESERVE_DESTROY: Signature = sig(
    "reserve destroy",
    &[(
        "48895c24084889742410574883ec208bfa488bd9488d05????????488901488d05????????48894110",
        0x0,
    )],
);
const STRING_COPY: Signature = sig(
    "string copy",
    &[("48895c240848896c24104889742418574883ec2033c0488bfa", 0x0)],
);
const STRING_DESTROY: Signature = sig(
    "string destroy",
    &[("40534883ec20488b5118488bd94883fa10722c488b0948ffc24881fa0010000072184c8b41f84883c227492bc8488d41f84883f81f7721498bc8e8????????48c743100000000048c743180f000000c603004883c4205bc3ff15????????cccc33c0", 0x0)],
);
const DETACH: Signature = sig(
    "detach",
    &[(
        "48895c240848896c241048897424185741544155415641574883ec604c8bfa",
        0x0,
    )],
);
const AMMO_DISPOSE: Signature = sig(
    "ammo dispose",
    &[("40534883ec20488bd9488b094885c9743e488b5310482bd14883e2f04881fa0010000072184c8b41f84883c227492bc8488d41f84883f81f771b498bc8e8????????33c048890348894308488943104883c4205bc3ff15????????cccccccccce9????????", 0x0)],
);
const PRIMARY_ADD: Signature = sig(
    "primary add",
    &[(
        "48895c2418488954241055565741544155415641574883ec40488b7920",
        0x0,
    )],
);
const AMMO_MODE: Signature = sig(
    "ammo mode",
    &[("48895c2418574883ec20488bd94885c90f84????????488b01ba10000000ff909800000084c07519488b03ba20000000488bcbff909800000084c00f84????????488b03ba10000000488bcbff909800000084c0755b", 0x0)],
);
const AMMO_REMOVE: Signature = sig(
    "ammo remove",
    &[(
        "48895c240848896c241048897424185741544155415641574883ec70488b7a08",
        0x0,
    )],
);
const IMPORT_ROUNDS: Signature = sig(
    "import rounds",
    &[(
        "4883ec38488b41204c8bd94c8b51284533c94c2bd048b9398ee3388ee3388e",
        0x0,
    )],
);
const AMMO_SET_RECORD: Signature = sig(
    "ammo set record",
    &[("48895c24104889742418574883ec70418bf0488bd9488b4128", 0x0)],
);
const WEAK_BIND: Signature = sig(
    "weak bind",
    &[("48895c2418565741564883ec204c8bc24c8bf133c089442440", 0x0)],
);
const VISUAL_SWITCH: Signature = sig(
    "visual switch",
    &[("48895c2410488974241855574156488bec4883ec60488bf1", 0x0)],
);
const VISUAL_SYNC: Signature = sig(
    "visual sync",
    &[("48895c2408574883ec20488bf9488b4920488b01ff90b0000000", 0x0)],
);
const SPAWN: Signature = sig(
    "spawn",
    &[(
        "48895c24104c894c24204c8944241848894c24085556574154415541564157488d6c24f9",
        0x0,
    )],
);
const SLOT_TYPE: Signature = sig(
    "slot type",
    &[("40534883ec70488bd94885d2750c8b81080100004883c470", 0x0)],
);
const SLOT_CONTEXT: Signature = sig(
    "slot context",
    &[("40534883ec20488b01ff90c8000000488bd84885c07455488b00", 0x0)],
);
const ITEM_OVERRIDE: Signature = sig(
    "item override",
    &[("48895c2408574883ec3048837a1810488bd94c8b5210488bca", 0x0)],
);
const CANONICAL: Signature = sig(
    "canonical squad",
    &[(
        "40534883ec20488bd94885c97452488b01ba10000000ff909800000084c07409",
        0x0,
    )],
);
/// The spawn's call to the squad manager getter.
const MANAGER_GET_CALL: Signature = sig(
    "manager getter call",
    &[("e8????????488b4820488b5028483bca0f84????????48397110", 0x0)],
);
/// The damage callback's call to the weapon holder getter.
const HOLDER_GET_CALL: Signature = sig(
    "holder getter call",
    &[("e8????????488bc84885c00f84????????488b80a8000000", 0x0)],
);
const ORIGINAL_DEATH: Signature = sig(
    "original death",
    &[("48895c2418555657415641574883ec400f297424304c8bf2", 0x0)],
);
const DROP_SPECIAL: Signature = sig(
    "drop special",
    &[(
        "488bc44889581048897018488978205541564157488da828ffffff",
        0x0,
    )],
);
const PICKUP_ADD: Signature = sig(
    "pickup add",
    &[(
        "4c89442418488954241048894c24085355565741544155415641574883ec48",
        0x0,
    )],
);
/// HumanGunner's destructor and its first virtual, which identify the vtable
/// the replacements compare gunners against.
const HUMAN_METHODS: [Signature; 2] = [
    sig(
        "HumanGunner destructor",
        &[("48895c2408574883ec208bfa488bd9e8????????40f6c7017427488bcb40f6c7047514ff15????????488bc3488b5c24304883c4205fc3ba38010000e8????????488bc3488b5c24304883c4205fc3cc488bc4", 0x0)],
    ),
    sig(
        "HumanGunner method 1",
        &[("48895c2410488974241848897c2420554154415541564157488bec4883ec60488bfa", 0x0)],
    ),
];
/// Stock functions whose behaviour the replacements reproduce or depend on;
/// each must still be present exactly once.
const STOCK: &[Signature] = &[
    sig(
        "damage callback",
        &[("488bc44c89401848895010555356574154415541564157488d68a8", 0x0)],
    ),
    sig(
        "squad member equipment",
        &[("488954241048894c2408555356574154415541564157488d6c24e14881ecb80000004c8be9", 0x0)],
    ),
    sig(
        "squad member refresh",
        &[("48895c240848896c2410565741564881ec900000004c8bf2", 0x0)],
    ),
    sig(
        "squad reinforcement",
        &[("4889542410555356574154415541564157488dac2438ffffff", 0x0)],
    ),
    sig(
        "ammo share getter",
        &[("807938004c8bc2488bd17428418b801c0100003d00800000741a3d00400000741383f820740e3d0001000074073d800000007535488b5228", 0x0)],
    ),
    sig(
        "ammo capacity registration",
        &[("4883ec38488b412049bb398ee3388ee3388e4c8b51284533c9", 0x0)],
    ),
    sig(
        "ammo carrier registration",
        &[("48895c24084889742410574c8b4208488bf9488b0248be398ee3388ee3388e", 0x0)],
    ),
    sig(
        "gun constructor",
        &[("48895c241848894c2408555657415641574883ec20498be9", 0x0)],
    ),
    sig(
        "carried geometry builder",
        &[("488bc4488958105556574154415541564157488da868feffff", 0x0)],
    ),
    sig(
        "visual swap",
        &[("40534881ecd0000000488bda4c8bd9488bd1488d4c2420e8????????", 0x0)],
    ),
    sig(
        "visual move",
        &[("48895c2408574883ec20488b02488bda488901488bf94883c1084883c208e8????????488d5320", 0x0)],
    ),
];

const HUMAN: &str = ".?AVHumanGunner@Leonardo@@";
/// How far into the collection its drop and add calls may sit.
const COLLECT_SPAN: usize = 0x200;

/// The bytes each entry hook relocates.
const COLLECT_BEFORE: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x08];
const REBUILD_BEFORE: &[u8] = &[0x48, 0x89, 0x4c, 0x24, 0x08];
const RESERVE_WRITER_BEFORE: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x10];
const RESERVE_LOAD_BEFORE: &[u8] = &[0x48, 0x89, 0x54, 0x24, 0x10];
const RESERVE_DESTROY_BEFORE: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x08];
/// The length of a hooked `call rel32`.
const CALL_LEN: usize = 5;

/// The sites, or why this build is not supported.
pub(crate) fn sites(image: &Image) -> Result<Sites, String> {
    let call_to = |site: usize, target: usize, what: &str| {
        if image.branch_target(site) == Some(target) {
            Ok(())
        } else {
            Err(format!("{what} at {site:#x} does not call {target:#x}"))
        }
    };
    let called = |call: &Signature| -> Result<usize, String> {
        let site = image.call(call)?;
        image
            .branch_target(site)
            .ok_or_else(|| format!("{} has no target", call.name))
    };
    let human = image.primary_vtable(HUMAN)?;
    for (slot, method) in HUMAN_METHODS.iter().enumerate() {
        let expected = image.find(method)?;
        if human.methods.get(slot) != Some(&expected) {
            return Err(format!("{HUMAN} slot {slot} is not the {}", method.name));
        }
    }
    for stock in STOCK {
        image.find(stock)?;
    }

    let sites = Sites {
        death_call: image.call(&DEATH_CALL)?,
        collect: image.find(&COLLECT)?,
        collect_drop: image.call(&COLLECT_DROP)?,
        collect_add: image.call(&COLLECT_ADD)?,
        rebuild: image.find(&REBUILD)?,
        reserve_writer: image.find(&RESERVE_WRITER)?,
        reserve_load: image.find(&RESERVE_LOAD)?,
        reserve_destroy: image.find(&RESERVE_DESTROY)?,
        string_copy: image.find(&STRING_COPY)?,
        string_destroy: image.find(&STRING_DESTROY)?,
        detach: image.find(&DETACH)?,
        ammo_dispose: image.find(&AMMO_DISPOSE)?,
        primary_add: image.find(&PRIMARY_ADD)?,
        ammo_mode: image.find(&AMMO_MODE)?,
        ammo_remove: image.find(&AMMO_REMOVE)?,
        import_rounds: image.find(&IMPORT_ROUNDS)?,
        ammo_set_record: image.find(&AMMO_SET_RECORD)?,
        weak_bind: image.find(&WEAK_BIND)?,
        visual_switch: image.find(&VISUAL_SWITCH)?,
        visual_sync: image.find(&VISUAL_SYNC)?,
        spawn: image.find(&SPAWN)?,
        slot_type: image.find(&SLOT_TYPE)?,
        slot_context: image.find(&SLOT_CONTEXT)?,
        item_override: image.find(&ITEM_OVERRIDE)?,
        canonical: image.find(&CANONICAL)?,
        manager_get: called(&MANAGER_GET_CALL)?,
        holder_get: called(&HOLDER_GET_CALL)?,
        human_vtable: human.methods_at,
    };

    // The hooked calls still call what their detours chain to.
    call_to(sites.death_call, image.find(&ORIGINAL_DEATH)?, "death call")?;
    call_to(sites.collect_drop, image.find(&DROP_SPECIAL)?, "drop call")?;
    call_to(sites.collect_add, image.find(&PICKUP_ADD)?, "add call")?;
    for (what, site) in [
        ("drop call", sites.collect_drop),
        ("add call", sites.collect_add),
    ] {
        if !(sites.collect..sites.collect + COLLECT_SPAN).contains(&site) {
            return Err(format!("{what} at {site:#x} is outside the collection"));
        }
    }
    image.expect("collection entry", sites.collect, COLLECT_BEFORE)?;
    image.expect("rebuild entry", sites.rebuild, REBUILD_BEFORE)?;
    image.expect(
        "reserve writer entry",
        sites.reserve_writer,
        RESERVE_WRITER_BEFORE,
    )?;
    image.expect(
        "reserve load entry",
        sites.reserve_load,
        RESERVE_LOAD_BEFORE,
    )?;
    image.expect(
        "reserve destroy entry",
        sites.reserve_destroy,
        RESERVE_DESTROY_BEFORE,
    )?;
    Ok(sites)
}

impl Sites {
    /// Every hooked site, as (rva, relocated length, patch kind, name), in
    /// the order the patch contract declares them.
    pub(crate) fn hooks(&self) -> [(usize, usize, u32, &'static str); 8] {
        [
            (
                self.reserve_writer,
                RESERVE_WRITER_BEFORE.len(),
                PATCH_KIND_ENTRY,
                "reserve writer",
            ),
            (
                self.reserve_load,
                RESERVE_LOAD_BEFORE.len(),
                PATCH_KIND_ENTRY,
                "reserve load",
            ),
            (
                self.reserve_destroy,
                RESERVE_DESTROY_BEFORE.len(),
                PATCH_KIND_ENTRY,
                "reserve destroy",
            ),
            (
                self.rebuild,
                REBUILD_BEFORE.len(),
                PATCH_KIND_ENTRY,
                "rebuild",
            ),
            (self.death_call, CALL_LEN, PATCH_KIND_CALL, "death call"),
            (
                self.collect,
                COLLECT_BEFORE.len(),
                PATCH_KIND_ENTRY,
                "collection",
            ),
            (self.collect_drop, CALL_LEN, PATCH_KIND_CALL, "drop call"),
            (self.collect_add, CALL_LEN, PATCH_KIND_CALL, "add call"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS, BUILDS_2026};

    /// The rvas the plugin's per-build table held before it resolved them,
    /// in [`BUILDS_2026`] order.
    const TABLE: [(&str, Sites); 6] = [
        (
            "gog/2026-09-14",
            Sites {
                death_call: 0x2d14a8,
                collect: 0x5638b0,
                collect_drop: 0x5639e7,
                collect_add: 0x5639f7,
                rebuild: 0x458ae0,
                reserve_writer: 0x121e90,
                reserve_load: 0x1217a0,
                reserve_destroy: 0x1216d0,
                string_copy: 0x24b70,
                string_destroy: 0x24c90,
                detach: 0x2d7760,
                ammo_dispose: 0x5d9a0,
                primary_add: 0x2d6a50,
                ammo_mode: 0x113e90,
                ammo_remove: 0x122130,
                import_rounds: 0x122940,
                ammo_set_record: 0x122490,
                weak_bind: 0x24670,
                visual_switch: 0x4320e0,
                visual_sync: 0x2d99b0,
                spawn: 0x562cb0,
                slot_type: 0x116e40,
                slot_context: 0x2d8240,
                item_override: 0x2dc7b0,
                canonical: 0x10fb00,
                manager_get: 0x540ec0,
                holder_get: 0x2d27f0,
                human_vtable: 0x72bcc0,
            },
        ),
        (
            "steam/2026-09-22",
            Sites {
                death_call: 0x2d1538,
                collect: 0x563940,
                collect_drop: 0x563a77,
                collect_add: 0x563a87,
                rebuild: 0x458b70,
                reserve_writer: 0x121f20,
                reserve_load: 0x121830,
                reserve_destroy: 0x121760,
                string_copy: 0x24b70,
                string_destroy: 0x24c90,
                detach: 0x2d77f0,
                ammo_dispose: 0x5da30,
                primary_add: 0x2d6ae0,
                ammo_mode: 0x113f20,
                ammo_remove: 0x1221c0,
                import_rounds: 0x1229d0,
                ammo_set_record: 0x122520,
                weak_bind: 0x24670,
                visual_switch: 0x432170,
                visual_sync: 0x2d9a40,
                spawn: 0x562d40,
                slot_type: 0x116ed0,
                slot_context: 0x2d82d0,
                item_override: 0x2dc840,
                canonical: 0x10fb90,
                manager_get: 0x540f50,
                holder_get: 0x2d2880,
                human_vtable: 0x72bd00,
            },
        ),
        (
            "gog/2026-09-25",
            Sites {
                death_call: 0x2d14a8,
                collect: 0x564030,
                collect_drop: 0x564167,
                collect_add: 0x564177,
                rebuild: 0x459120,
                reserve_writer: 0x121e90,
                reserve_load: 0x1217a0,
                reserve_destroy: 0x1216d0,
                string_copy: 0x24b70,
                string_destroy: 0x24c90,
                detach: 0x2d7760,
                ammo_dispose: 0x5d9a0,
                primary_add: 0x2d6a50,
                ammo_mode: 0x113e90,
                ammo_remove: 0x122130,
                import_rounds: 0x122940,
                ammo_set_record: 0x122490,
                weak_bind: 0x24670,
                visual_switch: 0x432720,
                visual_sync: 0x2d99b0,
                spawn: 0x563430,
                slot_type: 0x116e40,
                slot_context: 0x2d8240,
                item_override: 0x2dc7b0,
                canonical: 0x10fb00,
                manager_get: 0x541640,
                holder_get: 0x2d27f0,
                human_vtable: 0x72ccc0,
            },
        ),
        (
            "steam/2026-09-25",
            Sites {
                death_call: 0x2d1538,
                collect: 0x5640c0,
                collect_drop: 0x5641f7,
                collect_add: 0x564207,
                rebuild: 0x4591b0,
                reserve_writer: 0x121f20,
                reserve_load: 0x121830,
                reserve_destroy: 0x121760,
                string_copy: 0x24b70,
                string_destroy: 0x24c90,
                detach: 0x2d77f0,
                ammo_dispose: 0x5da30,
                primary_add: 0x2d6ae0,
                ammo_mode: 0x113f20,
                ammo_remove: 0x1221c0,
                import_rounds: 0x1229d0,
                ammo_set_record: 0x122520,
                weak_bind: 0x24670,
                visual_switch: 0x4327b0,
                visual_sync: 0x2d9a40,
                spawn: 0x5634c0,
                slot_type: 0x116ed0,
                slot_context: 0x2d82d0,
                item_override: 0x2dc840,
                canonical: 0x10fb90,
                manager_get: 0x5416d0,
                holder_get: 0x2d2880,
                human_vtable: 0x72cd00,
            },
        ),
        (
            "gog/2026-10-07",
            Sites {
                death_call: 0x2d1ee8,
                collect: 0x5686e0,
                collect_drop: 0x568817,
                collect_add: 0x568827,
                rebuild: 0x45cb60,
                reserve_writer: 0x121e90,
                reserve_load: 0x1217a0,
                reserve_destroy: 0x1216d0,
                string_copy: 0x24b70,
                string_destroy: 0x24c90,
                detach: 0x2d81a0,
                ammo_dispose: 0x5d9a0,
                primary_add: 0x2d7490,
                ammo_mode: 0x113e90,
                ammo_remove: 0x122130,
                import_rounds: 0x122940,
                ammo_set_record: 0x122490,
                weak_bind: 0x24670,
                visual_switch: 0x436170,
                visual_sync: 0x2da3f0,
                spawn: 0x567ae0,
                slot_type: 0x116e40,
                slot_context: 0x2d8c80,
                item_override: 0x2dd1f0,
                canonical: 0x10fb00,
                manager_get: 0x545c10,
                holder_get: 0x2d3230,
                human_vtable: 0x731c70,
            },
        ),
        (
            "steam/2026-10-07",
            Sites {
                death_call: 0x2d1f78,
                collect: 0x568770,
                collect_drop: 0x5688a7,
                collect_add: 0x5688b7,
                rebuild: 0x45cbf0,
                reserve_writer: 0x121f20,
                reserve_load: 0x121830,
                reserve_destroy: 0x121760,
                string_copy: 0x24b70,
                string_destroy: 0x24c90,
                detach: 0x2d8230,
                ammo_dispose: 0x5da30,
                primary_add: 0x2d7520,
                ammo_mode: 0x113f20,
                ammo_remove: 0x1221c0,
                import_rounds: 0x1229d0,
                ammo_set_record: 0x122520,
                weak_bind: 0x24670,
                visual_switch: 0x436200,
                visual_sync: 0x2da480,
                spawn: 0x567b70,
                slot_type: 0x116ed0,
                slot_context: 0x2d8d10,
                item_override: 0x2dd280,
                canonical: 0x10fb90,
                manager_get: 0x545ca0,
                holder_get: 0x2d32c0,
                human_vtable: 0x731d10,
            },
        ),
    ];

    #[test]
    fn sites_resolve_where_the_build_table_had_them() {
        for (build, expected) in TABLE {
            assert!(BUILDS_2026.contains(&build), "{build}");
            let Some(mapped) = reference(build, "logic.dll") else {
                eprintln!("skipping {build}: no bin/{build}/logic.dll");
                continue;
            };
            assert_eq!(sites(&Image::mapped(&mapped)), Ok(expected), "{build}");
        }
    }

    #[test]
    fn sites_resolve_on_every_september_build() {
        for build in BUILDS_2026 {
            if let Some(mapped) = reference(build, "logic.dll") {
                assert!(sites(&Image::mapped(&mapped)).is_ok(), "{build}");
            }
        }
    }

    /// The December 2025 builds predate the squad ammo the plugin shares, so
    /// they are not supported.
    #[test]
    fn the_december_builds_are_refused() {
        for build in BUILDS.into_iter().filter(|b| !BUILDS_2026.contains(b)) {
            if let Some(mapped) = reference(build, "logic.dll") {
                assert!(sites(&Image::mapped(&mapped)).is_err(), "{build}");
            }
        }
    }

    /// Every entry hook relocates whole instructions that need no fixup.
    #[test]
    fn displaced_prologues_need_no_relocation() {
        for build in BUILDS_2026 {
            let Some(mapped) = reference(build, "logic.dll") else {
                continue;
            };
            let sites = sites(&Image::mapped(&mapped)).unwrap();
            for (rva, len, kind, name) in sites.hooks() {
                let before = &mapped.image[rva..rva + len];
                if kind == PATCH_KIND_CALL {
                    assert_eq!(before[0], 0xe8, "{build} {name}");
                } else {
                    assert!(
                        defiance_core::decode::validate_copy(before).is_ok(),
                        "{build} {name}"
                    );
                }
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
        changed[expected.rebuild + 1] ^= 0x01;
        let image = Image {
            image: &changed,
            base: mapped.base,
        };
        assert!(sites(&image).is_err());
    }

    #[test]
    fn a_retargeted_call_is_refused() {
        let (build, expected) = TABLE[0];
        let Some(mapped) = reference(build, "logic.dll") else {
            return;
        };
        let mut changed = mapped.image.clone();
        // The call's displacement is a wildcard in its signature, so only the
        // target check catches this.
        changed[expected.death_call + 1] ^= 0x10;
        let image = Image {
            image: &changed,
            base: mapped.base,
        };
        assert!(sites(&image).is_err());
    }
}
