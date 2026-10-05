//! Where regroup's native code and class fields live, found by signature and
//! RTTI instead of looked up by build hash.
//!
//! Functions are found by byte signature (`tools/sigs.py`'s encoding: rel32
//! targets and RIP displacements wildcarded, struct offsets kept), the three
//! vtables regroup compares against by RTTI, and the four hooked call sites by
//! the code around them plus the function each must call. The class offsets
//! are read from the game code that uses the same fields. Each site must
//! resolve to exactly one place and every hooked entry must decode to the span
//! the hook displaces, so a build where any of it moved or changed shape
//! resolves to an error and the plugin hooks nothing.
use defiance_core::{
    decode,
    sites::{sig, Image, Signature},
};

pub const SPAWN: usize = 0;
pub const ADD: usize = 1;
pub const REMOVE: usize = 2;
pub const REMOVE_GUNNER: usize = 3;
pub const CLEANUP: usize = 4;
pub const RESERVE: usize = 5;
pub const BIND: usize = 6;
pub const FREE_STRINGS: usize = 7;
pub const APPEND: usize = 8;
/// The member creator [`CREATE_CALL`] calls.
pub const MEMBERS: usize = 9;
/// The wiring [`WIRE_CALL`] calls.
pub const WIRE: usize = 10;
/// The template preparation [`TEMPLATES_CALL`] calls.
pub const TEMPLATES: usize = 11;
/// The string assignment [`SPAWN_FALLBACK_CALL`] calls.
pub const STRING_ASSIGN: usize = 12;
pub const WEAK_BIND: usize = 13;
pub const SQUAD_UPDATE: usize = 14;
pub const PERK_REFRESH: usize = 15;
pub const PERK_PREPARE: usize = 16;
pub const PERK_UPDATE: usize = 17;
pub const PERK_MEMBER: usize = 18;
pub const EXPORT_ROSTER: usize = 19;
pub const RESIZE_ROSTER: usize = 20;
pub const CREATE_CALL: usize = 21;
pub const WIRE_CALL: usize = 22;
pub const TEMPLATES_CALL: usize = 23;
pub const SPAWN_FALLBACK_CALL: usize = 24;
/// The SquadAiFacet primary vtable.
pub const SQUAD_AI: usize = 25;
/// The HumanAiFacet primary vtable.
pub const HUMAN_AI: usize = 26;
/// The Gun primary vtable.
pub const GUN: usize = 27;
/// The number of logic.dll sites.
pub const COUNT: usize = 28;

/// The game.dll input dispatch, hooked.
pub const INPUT: usize = 0;
/// The game.dll stock keyboard shortcuts.
pub const SHORTCUTS: usize = 1;
/// The game.dll selection-manager update.
pub const SELECTION_MANAGER: usize = 2;
/// The game.dll hover update, hooked.
pub const HOVER: usize = 3;
/// The number of game.dll sites.
pub const GAME_COUNT: usize = 4;

/// Class offsets regroup reads, which differ between builds while the
/// functions that use them do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offsets {
    /// The SquadAiFacet vtable slot of the roster-holder getter.
    pub roster: usize,
    /// The world vtable slot of the per-team selection-manager lookup.
    pub world_manager: usize,
    /// The input object's player-context field.
    pub input_player: usize,
    /// The input object's UI field, whose +0x18 is set while the UI has
    /// captured the keyboard.
    pub input_ui: usize,
}

/// The resolved sites of both modules, as rvas, and the class offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sites {
    pub logic: [usize; COUNT],
    pub game: [usize; GAME_COUNT],
    pub offsets: Offsets,
}

const LOGIC: [Signature; 25] = [
    sig(
        "spawn",
        &[(
            "488bc44c8948204c894018488950104889480853565741544155415641574881ecb0030000",
            0x0,
        )],
    ),
    sig(
        "add",
        &[("48895c24204889542410565741564883ec30488bf2488bf9", 0x0)],
    ),
    sig(
        "remove",
        &[
            (
                "4053564883ec38488bf1488bda488b89a8000000488b86a0000000",
                0x0,
            ),
            (
                "4055564883ec38488be9488bf2488b89a8000000488b85a0000000",
                0x0,
            ),
        ],
    ),
    sig(
        "remove gunner",
        &[("4c8b89e801000033c04c8b81f00100004c8bda4d2bc14c8bd1", 0x0)],
    ),
    sig(
        "cleanup",
        &[(
            "40534883ec2080b93001000000488bd90f85????????488b89c8010000",
            0x0,
        )],
    ),
    sig(
        "reserve",
        &[(
            "48895c24104889742418574883ec20488b590848b8ffffffffffffff1f",
            0x0,
        )],
    ),
    sig(
        "bind",
        &[(
            "48895c2408574883ec20488bd9488bfa488b094885c974094c8b41104d85c075034533c0493bf8742f4885c9740d48837910007406ff15????????488bd7488bcbe8????????488b034885c0740a48837810007403ff4018488bc3",
            0x0,
        )],
    ),
    sig(
        "free strings",
        &[(
            "48895c2410564883ec20488b19488bf14885db746548897c2430488b7908483bdf7411488bcbe8????????4883c320483bdf75ef488b0e488b5610488b7c2430482bd14883e2e04881fa0010000072184c8b41f84883c227492bc8488d41f84883f81f7720498bc8e8????????33c04889064889460848894610488b5c24384883c4205ec3ff15????????cccccccccc488b09",
            0x0,
        )],
    ),
    sig(
        "append",
        &[(
            "405355415541574883ec28488b41084c8bfa488b11498bef482bea482bc24d8be848c1fd03",
            0x0,
        )],
    ),
    sig(
        "members",
        &[("4889542410555356574154415541564157488dac2438ffffff", 0x0)],
    ),
    sig(
        "wire",
        &[(
            "488954241048894c2408555356574154415541564157488d6c24e14881ecb80000004c8be9",
            0x0,
        )],
    ),
    sig(
        "templates",
        &[(
            "48894c2408555356574154415541564157488d6c24e14881eca80000004533ff",
            0x0,
        )],
    ),
    sig(
        "string assign",
        &[("48895c241048896c2418565741574883ec20488b6918498bf0", 0x0)],
    ),
    sig(
        "weak bind",
        &[("48895c2418565741564883ec204c8bc24c8bf133c089442440", 0x0)],
    ),
    sig(
        "squad update",
        &[
            (
                "40534883ec400f29742430488bd90f297c24200f28f1e8????????",
                0x0,
            ),
            (
                "40534883ec300f29742420488bd90f28f1e8????????488bcb",
                0x0,
            ),
        ],
    ),
    sig(
        "perk refresh",
        &[("40534881ecc0000000488bd9488b89600100004885c9743c", 0x0)],
    ),
    sig(
        "perk prepare",
        &[("48895c240848896c24104889742418574883ec40488bf10f57c0", 0x0)],
    ),
    sig(
        "perk update",
        &[("48895c2408574883ec40488bf90f29742430488b8960010000", 0x0)],
    ),
    sig(
        "perk member",
        &[(
            "48895c24084889742410574883ec40488bf90f29742430488b8960010000",
            0x0,
        )],
    ),
    sig(
        "export roster",
        &[("48895c2408574883ec20488bda498bc8ba14000000498bf8", 0x0)],
    ),
    sig(
        "resize roster",
        &[(
            "4889742410574883ec20488b7108488bf94c8b01488bce492bc848c1f903",
            0x0,
        )],
    ),
    sig(
        "create call",
        &[("e8????????4c8b6dd7488b5dcf488b75c74c8b75ff4d85f6", 0x0)],
    ),
    sig(
        "wire call",
        &[("e8????????488b06488bceff90b0000000488b48184885c97409", 0x0)],
    ),
    sig(
        "templates call",
        &[("e8????????488bd6498bcee8????????90498bc6488b5c2478", 0x0)],
    ),
    sig(
        "spawn fallback call",
        &[(
            "e8????????4c8bac24c00000004c89ac24e0000000488bbc24b8000000",
            0x0,
        )],
    ),
];

const GAME: [Signature; GAME_COUNT] = [
    sig(
        "input dispatch",
        &[("48895c241048896c24184889742420574883ec308b1a488bf9", 0x0)],
    ),
    sig(
        "stock shortcuts",
        &[(
            "48895c241055565741564157488dac2440ffffff4881ecc0010000",
            0x0,
        )],
    ),
    sig(
        "selection manager",
        &[("40564883ec20488bf1488b49184885c90f84????????488b4910", 0x0)],
    ),
    sig(
        "hover",
        &[("40574883ec20488bf9488b89700100004885c90f84????????", 0x0)],
    ),
];

const SQUAD_AI_CLASS: &str = ".?AVSquadAiFacet@Leonardo@@";
const HUMAN_AI_CLASS: &str = ".?AVHumanAiFacet@Leonardo@@";
const GUN_CLASS: &str = ".?AVGun@Leonardo@@";

/// Each hooked call site and the function it must call.
const CALLS: [(usize, usize); 4] = [
    (CREATE_CALL, MEMBERS),
    (WIRE_CALL, WIRE),
    (TEMPLATES_CALL, TEMPLATES),
    (SPAWN_FALLBACK_CALL, STRING_ASSIGN),
];
/// The bytes each entry hook displaces: whole instructions covering the
/// loader's 14-byte absolute jump, so a detour of any distance fits.
pub const PERK_DISPLACED: usize = 19;
pub const ROSTER_DISPLACED: usize = 16;
pub const CLEANUP_DISPLACED: usize = 16;
pub const UPDATE_DISPLACED: usize = 14;
/// Three five-byte register saves.
pub const INPUT_DISPLACED: usize = 15;
pub const HOVER_DISPLACED: usize = 16;
/// The hooked logic.dll entries and the bytes each hook displaces.
pub const LOGIC_ENTRIES: [(usize, usize); 4] = [
    (PERK_REFRESH, PERK_DISPLACED),
    (EXPORT_ROSTER, ROSTER_DISPLACED),
    (CLEANUP, CLEANUP_DISPLACED),
    (SQUAD_UPDATE, UPDATE_DISPLACED),
];
/// The hooked game.dll entries and the bytes each hook displaces.
pub const GAME_ENTRIES: [(usize, usize); 2] = [(INPUT, INPUT_DISPLACED), (HOVER, HOVER_DISPLACED)];

/// The roster-holder getter: `mov rax, [rcx+0x1c8]; test rax, rax; jz ...;
/// mov rax, [rax+0x10]`. Exactly one SquadAiFacet slot holds it.
const ROSTER_GETTER: &[u8] = b"\x48\x8b\x81\xc8\x01\x00\x00\x48\x85\xc0\x74";
const ROSTER_GETTER_TAIL: &[u8] = b"\x48\x8b\x40\x10";

/// The resolved sites, or why this build is not supported.
pub fn sites(logic: &Image, game: &Image) -> Result<Sites, String> {
    let mut sites = Sites {
        logic: [0; COUNT],
        game: [0; GAME_COUNT],
        offsets: Offsets {
            roster: 0,
            world_manager: 0,
            input_player: 0,
            input_ui: 0,
        },
    };
    for (rva, signature) in sites.logic.iter_mut().zip(&LOGIC) {
        *rva = logic.find(signature)?;
    }
    for (index, class) in [
        (SQUAD_AI, SQUAD_AI_CLASS),
        (HUMAN_AI, HUMAN_AI_CLASS),
        (GUN, GUN_CLASS),
    ] {
        sites.logic[index] = logic.primary_vtable(class)?.methods_at;
    }
    for (site, callee) in CALLS {
        if logic.branch_target(sites.logic[site]) != Some(sites.logic[callee]) {
            return Err(format!(
                "{} does not call {}",
                LOGIC[site].name, LOGIC[callee].name
            ));
        }
    }
    for (index, span) in LOGIC_ENTRIES {
        entry(logic, LOGIC[index].name, sites.logic[index], span)?;
    }
    for (rva, signature) in sites.game.iter_mut().zip(&GAME) {
        *rva = game.find(signature)?;
    }
    for (index, span) in GAME_ENTRIES {
        entry(game, GAME[index].name, sites.game[index], span)?;
    }
    sites.offsets = offsets(logic, game, &sites)?;
    Ok(sites)
}

/// Checks that the hook at `rva` displaces exactly `span` bytes of whole,
/// position-independent instructions.
fn entry(image: &Image, what: &str, rva: usize, span: usize) -> Result<(), String> {
    let code = image
        .image
        .get(rva..rva + 32)
        .ok_or_else(|| format!("{what} runs off the image"))?;
    let displaced = decode::displaced(code, 14).map_err(|e| format!("{what}: {e}"))?;
    if displaced != span {
        return Err(format!(
            "{what} displaces {displaced} bytes where the hook copies {span}"
        ));
    }
    decode::validate_copy(&code[..span]).map_err(|e| format!("{what}: {e}"))
}

/// The class offsets, each read from game code that uses the same field.
fn offsets(logic: &Image, game: &Image, sites: &Sites) -> Result<Offsets, String> {
    let squad_ai = logic.primary_vtable(SQUAD_AI_CLASS)?;
    let mut roster = None;
    for (slot, &method) in squad_ai.methods.iter().enumerate() {
        let Some(code) = logic.image.get(method..method + 16) else {
            continue;
        };
        if code.starts_with(ROSTER_GETTER) && &code[12..16] == ROSTER_GETTER_TAIL {
            if roster.is_some() {
                return Err("more than one SquadAiFacet slot reads the roster".into());
            }
            roster = Some(slot * 8);
        }
    }
    let roster = roster.ok_or("no SquadAiFacet slot reads the roster")?;
    // Each field is the disp32 of a `mov r64, [reg+disp32]` at a fixed place
    // in the function the plugin reproduces.
    let field = |what: &str, rva: usize, opcode: &[u8]| -> Result<usize, String> {
        game.expect(what, rva, opcode)?;
        game.u32(rva + 3)
            .map(|disp| disp as usize)
            .ok_or_else(|| format!("{what} runs off the image"))
    };
    Ok(Offsets {
        roster,
        // The selection-manager update loads the lookup from the world vtable.
        world_manager: field(
            "world manager lookup",
            sites.game[SELECTION_MANAGER] + 0x5a,
            b"\x48\x8b\x99",
        )?,
        // The stock shortcuts read the player context from the input object.
        input_player: field(
            "input player field",
            sites.game[SHORTCUTS] + 0x90,
            b"\x48\x8b\x8e",
        )?,
        // The input dispatch guards its stock shortcuts on the UI field.
        input_ui: field("input ui field", sites.game[INPUT] + 0x7c, b"\x48\x8b\x81")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::{
        pe::Mapped,
        sites::{reference, BUILDS},
    };

    /// The logic.dll rvas the per-build hash table held before the plugin
    /// resolved them, in [`BUILDS`] order.
    const LOGIC_TABLE: [(&str, [usize; COUNT]); 6] = [
        (
            "gog/2025-12-23",
            [
                0x5138f0, 0x446140, 0x4463d0, 0x43dc70, 0x43d8c0, 0x55c20, 0x9dcb0, 0x5f4e0,
                0x5a070, 0x444de0, 0x4476a0, 0x443460, 0x24690, 0x244d0, 0x43d3c0, 0x331420,
                0x3311c0, 0x332e80, 0x3315b0, 0x110240, 0xcf4a0, 0x443f7d, 0x446260, 0x442a42,
                0x513c9a, 0x728218, 0x715640, 0x714738,
            ],
        ),
        (
            "steam/2025-12-23",
            [
                0x513980, 0x4461d0, 0x446460, 0x43dd00, 0x43d950, 0x55cb0, 0x9dd40, 0x5f570,
                0x5a100, 0x444e70, 0x447730, 0x4434f0, 0x24690, 0x244d0, 0x43d450, 0x3314b0,
                0x331250, 0x332f10, 0x331640, 0x1102d0, 0xcf530, 0x44400d, 0x4462f0, 0x442ad2,
                0x513d2a, 0x728290, 0x715690, 0x714568,
            ],
        ),
        (
            "gog/2026-09-14",
            [
                0x526590, 0x459470, 0x4597f0, 0x450fa0, 0x450bf0, 0x56010, 0xa60b0, 0x5f560,
                0x5a0e0, 0x458110, 0x45a500, 0x456790, 0x24830, 0x24670, 0x4507a0, 0x3400c0,
                0x33fe60, 0x341b20, 0x340250, 0x118360, 0xd7540, 0x4572ad, 0x459672, 0x455d72,
                0x52693a, 0x73d860, 0x72a980, 0x729858,
            ],
        ),
        (
            "steam/2026-09-22",
            [
                0x526620, 0x459500, 0x459880, 0x451030, 0x450c80, 0x560a0, 0xa6140, 0x5f5f0,
                0x5a170, 0x4581a0, 0x45a590, 0x456820, 0x24830, 0x24670, 0x450830, 0x340150,
                0x33fef0, 0x341bb0, 0x3402e0, 0x1183f0, 0xd75d0, 0x45733d, 0x459702, 0x455e02,
                0x5269ca, 0x73d8b8, 0x72a9c0, 0x729888,
            ],
        ),
        (
            "gog/2026-09-25",
            [
                0x526d10, 0x459ab0, 0x459e30, 0x4515e0, 0x451230, 0x56010, 0xa60b0, 0x5f560,
                0x5a0e0, 0x458750, 0x45ab40, 0x456dd0, 0x24830, 0x24670, 0x450de0, 0x3400c0,
                0x33fe60, 0x341b20, 0x340250, 0x118360, 0xd7540, 0x4578ed, 0x459cb2, 0x4563b2,
                0x5270ba, 0x73e8f8, 0x72b980, 0x72a858,
            ],
        ),
        (
            "steam/2026-09-25",
            [
                0x526da0, 0x459b40, 0x459ec0, 0x451670, 0x4512c0, 0x560a0, 0xa6140, 0x5f5f0,
                0x5a170, 0x4587e0, 0x45abd0, 0x456e60, 0x24830, 0x24670, 0x450e70, 0x340150,
                0x33fef0, 0x341bb0, 0x3402e0, 0x1183f0, 0xd75d0, 0x45797d, 0x459d42, 0x456442,
                0x52714a, 0x73e988, 0x72b9c0, 0x72a888,
            ],
        ),
    ];

    /// The game.dll rvas the per-build hash table held, in [`BUILDS`] order.
    const GAME_TABLE: [(&str, [usize; GAME_COUNT]); 6] = [
        ("gog/2025-12-23", [0x2db9a0, 0x2da980, 0x332510, 0x1d0290]),
        ("steam/2025-12-23", [0x2e0cf0, 0x2dfcd0, 0x3389c0, 0x1d4b80]),
        ("gog/2026-09-14", [0x2ddb30, 0x2dcb10, 0x3346f0, 0x1d1b10]),
        ("steam/2026-09-22", [0x2e2eb0, 0x2e1e90, 0x33abd0, 0x1d6430]),
        ("gog/2026-09-25", [0x2ddb30, 0x2dcb10, 0x3346f0, 0x1d1b10]),
        ("steam/2026-09-25", [0x2e2eb0, 0x2e1e90, 0x33abd0, 0x1d6430]),
    ];

    /// The class offsets each build reads, in [`BUILDS`] order. The roster
    /// slot and the world-manager slot equal the old per-build table. The two
    /// input fields are where every build's own input code reads them; the old
    /// table had 0x870 and 0x890 for the 2026 builds, which no game code uses.
    const OFFSETS: [(&str, Offsets); 6] = {
        const DECEMBER: Offsets = Offsets {
            roster: 0x3b8,
            world_manager: 0x700,
            input_player: 0x860,
            input_ui: 0x888,
        };
        const SEPTEMBER: Offsets = Offsets {
            roster: 0x3d0,
            world_manager: 0x708,
            input_player: 0x860,
            input_ui: 0x888,
        };
        [
            ("gog/2025-12-23", DECEMBER),
            ("steam/2025-12-23", DECEMBER),
            ("gog/2026-09-14", SEPTEMBER),
            ("steam/2026-09-22", SEPTEMBER),
            ("gog/2026-09-25", SEPTEMBER),
            ("steam/2026-09-25", SEPTEMBER),
        ]
    };

    fn modules(build: &str) -> Option<(Mapped, Mapped)> {
        Some((
            reference(build, "logic.dll")?,
            reference(build, "game.dll")?,
        ))
    }

    #[test]
    fn every_build_resolves_to_the_old_table() {
        assert_eq!(LOGIC_TABLE.map(|(build, _)| build), BUILDS);
        assert_eq!(GAME_TABLE.map(|(build, _)| build), BUILDS);
        assert_eq!(OFFSETS.map(|(build, _)| build), BUILDS);
        for (((build, logic), (_, game)), (_, offsets)) in
            LOGIC_TABLE.iter().zip(&GAME_TABLE).zip(&OFFSETS)
        {
            let Some((logic_dll, game_dll)) = modules(build) else {
                continue;
            };
            let resolved = sites(&Image::mapped(&logic_dll), &Image::mapped(&game_dll))
                .unwrap_or_else(|e| panic!("{build}: {e}"));
            assert_eq!(resolved.logic, *logic, "{build} logic.dll");
            assert_eq!(resolved.game, *game, "{build} game.dll");
            assert_eq!(resolved.offsets, *offsets, "{build} offsets");
        }
    }

    #[test]
    fn a_changed_entry_or_call_site_is_refused() {
        let Some((logic_dll, game_dll)) = modules(BUILDS[0]) else {
            return;
        };
        let (logic, game) = (Image::mapped(&logic_dll), Image::mapped(&game_dll));
        let resolved = sites(&logic, &game).unwrap();
        for (index, _) in LOGIC_ENTRIES {
            let mut changed = logic.image.to_vec();
            changed[resolved.logic[index] + 4] ^= 1;
            let image = Image {
                image: &changed,
                base: logic.base,
            };
            assert!(sites(&image, &game).is_err(), "{}", LOGIC[index].name);
        }
        for (index, _) in GAME_ENTRIES {
            let mut changed = game.image.to_vec();
            changed[resolved.game[index] + 4] ^= 1;
            let image = Image {
                image: &changed,
                base: game.base,
            };
            assert!(sites(&logic, &image).is_err(), "{}", GAME[index].name);
        }
        // A call site that calls elsewhere is refused even though its
        // surroundings still match.
        let mut changed = logic.image.to_vec();
        changed[resolved.logic[CREATE_CALL] + 1] ^= 1;
        let image = Image {
            image: &changed,
            base: logic.base,
        };
        assert!(sites(&image, &game).is_err());
    }
}
