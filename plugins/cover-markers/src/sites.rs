//! Where the hooked and called code lives in each supported build.
//!
//! Functions and call sites are found by byte signature (`tools/sigs.py`'s
//! encoding: rel32 targets and RIP displacements wildcarded, struct offsets
//! kept), vtables by their RTTI class. Each signature must match exactly once,
//! so a build where any site moved or changed shape resolves to an error and
//! the plugin writes nothing. The same code reads a running module and a DLL
//! mapped from disk, which is how the tests check every reference build.

use defiance_core::{pattern, rtti, scan};

/// One signature: its text, and how far into the match the site lies.
struct Signature {
    name: &'static str,
    text: &'static str,
    at: usize,
}

const fn sig(name: &'static str, text: &'static str) -> Signature {
    Signature { name, text, at: 0 }
}

const CURSOR_UPDATE: Signature = sig(
    "move cursor update",
    "4883ec580f297424404c8bc10f297c24300f57f6f30f10791c",
);
const CURSOR_CONSTRUCTOR: Signature = sig(
    "move cursor constructor",
    "4883ec28e8????????33c0488991d00000004c8d0d????????",
);
const CURSOR_FACTORY: Signature = sig(
    "move cursor factory",
    "48895c2410488974241848897c2420554154415541564157488d6c24c94881ecd0000000488bf1",
);
const CURSOR_RENDER_UPDATE: Signature = sig(
    "move cursor render update",
    "48895c2420555741544883ec504883795000488bd97505e8????????",
);
const CURSOR_DESTRUCTOR: Signature = sig(
    "move cursor destructor",
    "48895c2408574883ec20488bf98bda4881c1d8000000e8????????",
);
/// The smart cursor facet's per-frame update, `(facet, dt)`.
const FACET_UPDATE: Signature = sig(
    "smart cursor update",
    "48895c241048896c2418565741564883ec500f297424400f28f1488bf1488b4130",
);
const ISSUE_ORDER: Signature = sig(
    "vehicle move order",
    "48895c24205556574154415541564157488d6c24d94881eca00000000f29b42490000000",
);
const FORMATION_SOLVER: Signature = sig(
    "formation solver",
    "48895c241855564157488d6c24c14881ec90000000f3410f10490c",
);
const MOVE_POINT_CALL: Signature = sig(
    "move destination call",
    "e8????????488bf84889442448488d4c2450e8????????488d542448",
);
const SQUAD_ORDER_DISPATCH: Signature = sig(
    "squad order dispatch",
    "488bc44c89401848895010555356574154415541564157488da888feffff",
);
// The per-entity Stop path constructs its order before the vehicle AI receives
// it. The call's own function is shared, so the window runs past it.
const STOP_ORDER_FACTORY_CALL: Signature = sig(
    "Stop order factory call",
    "e8????????488bd0488d4c2430e8????????488b4424304885c0740a48837810007403ff40180f57d2\
     488d542430488bcfffd6488b03488bcbff90b0000000488b742438488b48684885c9740b488b01ba05000000",
);
// These calls hand the newly built order to one soldier after native recipient
// filtering.
const GARRISON_HANDOFF_CALL: Signature = sig(
    "garrison handoff call",
    "e8????????904584e4751148837b1000740a488bcbff15????????",
);
const BOARDING_HANDOFF_CALL: Signature = sig(
    "boarding handoff call",
    "e8????????904885ff741148837f1000740a488bcfff15????????904885ff7410834708ff750a\
     488b07488bcfff50109048ffc5",
);
const MAP_SAMPLE_HEIGHT: Signature = sig(
    "map height sample",
    "488bc45556574881ecb0000000f3410f10100f57db0f2fd3",
);
const RENDERER_SCALE_SETTER: Signature = sig(
    "terrain renderer scale setter",
    "40534883ec20488bd94881c1ec030000e8????????84c07538",
);

const STOP_ORDER: &str = ".?AVAiStopOrder@Leonardo@@";
/// The move command, whose cursor update and destructor are hooked.
const MOVE_COMMAND: &str = ".?AVSmartCursorCmdMove@Leonardo@@";
const TERRAIN_RENDERER: &str = ".?AVTerrainRenderer@World2@Galileo@@";
const MAP_IMPL: &str = ".?AVMapImpl@Landscape@Galileo@@";
/// The tactical map's game state. Its object size differs between builds, so
/// its scalar deleting destructor (slot 2) is reached through the vtable.
const TACTICAL_MAP_STATE: &str = ".?AVTacticalMapGameState@Leonardo@@";
/// The full TacticalMapGameState vtable; a short secondary one also exists.
const TACTICAL_MAP_STATE_METHODS: usize = 19;
const DELETING_DESTRUCTOR_SLOT: usize = 2;

/// game.dll sites, as rvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Game {
    pub cursor_update: usize,
    pub cursor_constructor: usize,
    pub cursor_factory: usize,
    pub cursor_render_update: usize,
    pub cursor_destructor: usize,
    pub issue_order: usize,
    pub mission_deleting_destructor: usize,
    pub facet_update: usize,
    pub move_command_vtable: usize,
}

/// logic.dll sites, as rvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Logic {
    pub formation_solver: usize,
    pub move_point_call: usize,
    pub squad_order_dispatch: usize,
    pub stop_order_factory_call: usize,
    pub garrison_handoff_call: usize,
    pub boarding_handoff_call: usize,
    pub stop_order_vtable: usize,
}

/// world2.dll sites, as rvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct World {
    pub map_sample_height: usize,
    pub renderer_scale_setter: usize,
    pub terrain_renderer_vtable: usize,
    pub map_impl_vtable: usize,
}

/// A module image laid out as mapped: `image[rva]` is the byte at `base + rva`.
/// `base` is where the image's absolute addresses point (the load address for a
/// running module, the preferred base for a file mapped from disk).
pub struct Image<'a> {
    pub image: &'a [u8],
    pub base: usize,
}

impl Image<'_> {
    fn find(&self, signature: &Signature) -> Result<usize, String> {
        let pattern = pattern::parse(signature.text)
            .map_err(|error| format!("{}: {error}", signature.name))?;
        match scan::scan(self.image, &pattern).as_slice() {
            [hit] => Ok(hit + signature.at),
            [] => Err(format!("{} not found", signature.name)),
            hits => Err(format!("{} matches {} places", signature.name, hits.len())),
        }
    }

    fn call(&self, signature: &Signature) -> Result<usize, String> {
        let rva = self.find(signature)?;
        if self.image.get(rva) != Some(&0xe8) {
            return Err(format!("{} is not a direct call", signature.name));
        }
        Ok(rva)
    }

    /// The vtables of exactly `class` (`rtti::find` matches substrings).
    fn vtables(&self, class: &str) -> Vec<rtti::Vtable> {
        let code = code_ranges(self.image);
        let is_code = |rva: usize| code.iter().any(|(start, end)| rva >= *start && rva < *end);
        rtti::find(self.image, self.base, class, &is_code)
            .into_iter()
            .filter(|vtable| vtable.mangled == class)
            .collect()
    }

    fn vtable(&self, class: &str) -> Result<usize, String> {
        match self.vtables(class).as_slice() {
            [vtable] => Ok(vtable.methods_at),
            [] => Err(format!("no {class} vtable")),
            found => Err(format!("{} {class} vtables", found.len())),
        }
    }

    fn tactical_map_destructor(&self) -> Result<usize, String> {
        let full: Vec<_> = self
            .vtables(TACTICAL_MAP_STATE)
            .into_iter()
            .filter(|vtable| vtable.methods.len() == TACTICAL_MAP_STATE_METHODS)
            .collect();
        match full.as_slice() {
            [vtable] => Ok(vtable.methods[DELETING_DESTRUCTOR_SLOT]),
            found => Err(format!("{} full {TACTICAL_MAP_STATE} vtables", found.len())),
        }
    }

    pub fn game(&self) -> Result<Game, String> {
        Ok(Game {
            cursor_update: self.find(&CURSOR_UPDATE)?,
            cursor_constructor: self.find(&CURSOR_CONSTRUCTOR)?,
            cursor_factory: self.find(&CURSOR_FACTORY)?,
            cursor_render_update: self.find(&CURSOR_RENDER_UPDATE)?,
            cursor_destructor: self.find(&CURSOR_DESTRUCTOR)?,
            issue_order: self.find(&ISSUE_ORDER)?,
            mission_deleting_destructor: self.tactical_map_destructor()?,
            facet_update: self.find(&FACET_UPDATE)?,
            move_command_vtable: self.vtable(MOVE_COMMAND)?,
        })
    }

    pub fn logic(&self) -> Result<Logic, String> {
        Ok(Logic {
            formation_solver: self.find(&FORMATION_SOLVER)?,
            move_point_call: self.call(&MOVE_POINT_CALL)?,
            squad_order_dispatch: self.find(&SQUAD_ORDER_DISPATCH)?,
            stop_order_factory_call: self.call(&STOP_ORDER_FACTORY_CALL)?,
            garrison_handoff_call: self.call(&GARRISON_HANDOFF_CALL)?,
            boarding_handoff_call: self.call(&BOARDING_HANDOFF_CALL)?,
            stop_order_vtable: self.vtable(STOP_ORDER)?,
        })
    }

    pub fn world(&self) -> Result<World, String> {
        Ok(World {
            map_sample_height: self.find(&MAP_SAMPLE_HEIGHT)?,
            renderer_scale_setter: self.find(&RENDERER_SCALE_SETTER)?,
            terrain_renderer_vtable: self.vtable(TERRAIN_RENDERER)?,
            map_impl_vtable: self.vtable(MAP_IMPL)?,
        })
    }
}

const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;

/// The (start, end) rvas of the executable sections, read from the image's own
/// section table, which a mapped module keeps at its start.
fn code_ranges(image: &[u8]) -> Vec<(usize, usize)> {
    let u16_at = |o: usize| {
        image
            .get(o..o + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let u32_at = |o: usize| {
        image
            .get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut out = Vec::new();
    let Some(pe) = u32_at(0x3c).map(|v| v as usize) else {
        return out;
    };
    if image.get(pe..pe + 4) != Some(b"PE\0\0") {
        return out;
    }
    let (Some(sections), Some(optional_size)) = (u16_at(pe + 6), u16_at(pe + 20)) else {
        return out;
    };
    let table = pe + 24 + optional_size as usize;
    for i in 0..sections as usize {
        let s = table + i * 40;
        let (Some(size), Some(va), Some(characteristics)) =
            (u32_at(s + 8), u32_at(s + 12), u32_at(s + 36))
        else {
            break;
        };
        if characteristics & IMAGE_SCN_MEM_EXECUTE != 0 {
            out.push((va as usize, (va + size) as usize));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const BUILDS: [&str; 6] = [
        "gog/2026-09-14",
        "gog/2026-09-25",
        "steam/2026-09-22",
        "steam/2026-09-25",
        "gog/2026-10-07",
        "steam/2026-10-07",
    ];
    /// The 2026-09-25 world2.dll; GOG and Steam 2026-09-25 ship the same file.
    const WORLD: &str = "gog/2026-09-25";
    /// Every build with a world2.dll in `bin/`.
    const WORLDS: [&str; 2] = [WORLD, "gog/2026-10-07"];

    fn mapped(build: &str, dll: &str) -> Option<defiance_core::pe::Mapped> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../bin")
            .join(build)
            .join(dll);
        path.exists()
            .then(|| defiance_core::pe::map(&path).expect("maps"))
    }

    fn image(mapped: &defiance_core::pe::Mapped) -> Image<'_> {
        Image {
            image: &mapped.image,
            base: mapped.base,
        }
    }

    #[test]
    fn every_site_resolves_in_every_reference_build() {
        for build in BUILDS {
            let (Some(game), Some(logic)) = (mapped(build, "game.dll"), mapped(build, "logic.dll"))
            else {
                eprintln!("skipping {build}: bin/ is absent");
                continue;
            };
            let game = image(&game)
                .game()
                .unwrap_or_else(|e| panic!("{build}: {e}"));
            let logic = image(&logic)
                .logic()
                .unwrap_or_else(|e| panic!("{build}: {e}"));
            assert_ne!(game.cursor_update, game.cursor_destructor);
            assert_ne!(logic.move_point_call, logic.boarding_handoff_call);
        }
        for build in WORLDS {
            if let Some(world) = mapped(build, "world2.dll") {
                image(&world)
                    .world()
                    .unwrap_or_else(|e| panic!("{build} world2: {e}"));
            }
        }
    }

    /// `install` refuses a build whose entry prologue differs; every
    /// supported build must pass that check.
    #[test]
    fn entry_hooks_steal_the_same_instructions_in_every_reference_build() {
        let starts = |mapped: &defiance_core::pe::Mapped, rva: usize, before: &[u8]| {
            assert_eq!(&mapped.image[rva..rva + before.len()], before, "{rva:#x}");
        };
        for build in BUILDS {
            let (Some(game_dll), Some(logic_dll)) =
                (mapped(build, "game.dll"), mapped(build, "logic.dll"))
            else {
                continue;
            };
            let game = image(&game_dll).game().unwrap();
            let logic = image(&logic_dll).logic().unwrap();
            starts(&game_dll, game.cursor_update, crate::CURSOR_BEFORE);
            starts(
                &game_dll,
                game.cursor_destructor,
                crate::CURSOR_DESTRUCTOR_BEFORE,
            );
            starts(&game_dll, game.issue_order, crate::ISSUE_ORDER_BEFORE);
            starts(&game_dll, game.facet_update, crate::FACET_UPDATE_BEFORE);
            starts(
                &game_dll,
                game.mission_deleting_destructor,
                crate::MISSION_DELETING_DESTRUCTOR_BEFORE,
            );
            starts(
                &logic_dll,
                logic.squad_order_dispatch,
                crate::SQUAD_ORDER_DISPATCH_BEFORE,
            );
        }
    }

    #[test]
    fn steam_2026_09_25_matches_the_verified_rvas() {
        let (Some(game), Some(logic), Some(world)) = (
            mapped("steam/2026-09-25", "game.dll"),
            mapped("steam/2026-09-25", "logic.dll"),
            mapped(WORLD, "world2.dll"),
        ) else {
            eprintln!("skipping: bin/ is absent");
            return;
        };
        assert_eq!(
            image(&game).game().unwrap(),
            Game {
                cursor_update: 0x32d7f0,
                cursor_constructor: 0x339690,
                cursor_factory: 0x32d950,
                cursor_render_update: 0x32d5b0,
                cursor_destructor: 0x3396f0,
                issue_order: 0x339be0,
                mission_deleting_destructor: 0x350220,
                facet_update: 0x33cc80,
                move_command_vtable: 0x52b4d8,
            }
        );
        assert_eq!(
            image(&logic).logic().unwrap(),
            Logic {
                formation_solver: 0x455890,
                move_point_call: 0x44d4af,
                squad_order_dispatch: 0x44e950,
                stop_order_factory_call: 0x10f20d,
                garrison_handoff_call: 0x4501aa,
                boarding_handoff_call: 0x44df75,
                stop_order_vtable: 0x70e5d8,
            }
        );
        assert_eq!(
            image(&world).world().unwrap(),
            World {
                map_sample_height: 0x1256a0,
                renderer_scale_setter: 0x152990,
                terrain_renderer_vtable: 0x3eca20,
                map_impl_vtable: 0x3e2f00,
            }
        );
    }

    #[test]
    fn gog_tactical_map_destructor_comes_from_its_own_vtable() {
        let Some(game) = mapped("gog/2026-09-25", "game.dll") else {
            eprintln!("skipping: bin/ is absent");
            return;
        };
        let game = image(&game).game().unwrap();
        assert_eq!(game.mission_deleting_destructor, 0x349d40);
        assert_eq!(game.facet_update, 0x3367a0);
    }
}
