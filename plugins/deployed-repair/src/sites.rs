//! Checked menu windows bind the repair gates to their native prerequisites.
use defiance_core::sites::{sig, Image, Signature};

pub(crate) const MENU_BEFORE: &[u8] = &[0x81, 0xe7, 0xba, 0x65, 0xf7, 0xff];
const MENU: Signature = sig(
    "the deployed command mask",
    &[(
        "498b4500ba1a000000498bcdff507084c0753d498b4500ba19000000498bcdff507084c0752a498b4500ba1c000000498bcdff507084c07517498b4500ba18000000498bcdff507084c00f842affffff81e7ba65f7ffe91fffffff",
        0x50,
    )],
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Helicopter {
    pub site: usize,
    pub slot: usize,
}

impl Helicopter {
    pub fn before(self) -> [u8; 6] {
        [0xff, 0x90, self.slot as u8, (self.slot >> 8) as u8, 0, 0]
    }
}

// Damage and spare parts precede the helicopter veto. The fallthrough runs
// the native script/crew checks; the taken branch skips repair permission.
const HELICOPTER: Signature = sig(
    "the repair helicopter veto",
    &[(
        concat!(
            "498b0424498bccff90b00000004c8b70184d85f60f84cf000000",
            "498b06498bceff90800000008bd8498b16498bceff52783bc30f8db0000000",
            "498b06498bceff90d00000004c8b75674885c00f849b000000",
            "498b8e100e0000488b01498bd4ff90??020000",
            "84c00f8580000000498b0424498bccff5050",
        ),
        0x5f,
    )],
);

/// Validate the native veto's type query as well as its menu call site.
pub(crate) fn helicopter(game: &Image, logic: &Image) -> Result<Helicopter, String> {
    let mask = menu(game)?;
    let site = game.find(&HELICOPTER)?;
    if site.checked_add(0x306) != Some(mask) {
        return Err("the helicopter veto is outside the characterized menu update".into());
    }
    let slot = game.u32(site + 2).unwrap_or_default() as usize;
    if ![0x2e0, 0x2f0].contains(&slot) {
        return Err("the helicopter query slot differs".into());
    }
    let query = logic.method(".?AVAiUtilsImpl@Leonardo@@", slot / 8)?;
    logic.expect(
        "the helicopter entity flag query",
        query,
        b"\x48\x83\xec\x28\x4c\x8b\xc2\x48\x85\xd2\x74\x1c\x48\x8b\x02\x49\x8b\xc8\xba\x00\x20\x00\x00\xff\x90\x98\x00\x00\x00\x84\xc0\x74\x07\xb0\x01\x48\x83\xc4\x28\xc3\x32\xc0\x48\x83\xc4\x28\xc3",
    )?;
    let mut ai_top = b"\x48\x8b\x81\xe8\x00\x00\x00\x48\x85\xc0\x74\x13\x48\x8b\x48\x10\x48\x85\xc9\x74\x0a\x48\x8b\x01\x48\xff\xa0\x98\x00\x00\x00\x32\xc0\xc3".to_vec();
    let machine_slot = if slot == 0x2e0 { 18 } else { 19 };
    ai_top[0x1b] = (machine_slot * 8) as u8;
    logic.expect(
        "the AI top-state query",
        logic.method(".?AVAiFacet@Leonardo@@", 13)?,
        &ai_top,
    )?;
    let mut top_state = b"\x40\x53\x48\x83\xec\x20\x48\x8b\x41\x40\x8b\xda\x48\x39\x41\x38\x74\x23\x48\x8b\x48\xf8\x48\x85\xc9\x74\x1a\x48\x8b\x49\x10\x48\x85\xc9\x74\x11\x48\x8b\x01\xff\x50\x48\x3b\xc3\x0f\x94\xc0\x48\x83\xc4\x20\x5b\xc3\x32\xc0\x48\x83\xc4\x20\x5b\xc3".to_vec();
    // The 2025 builds have one fewer state virtual before the id getter.
    if slot == 0x2e0 {
        top_state[0x29] = 0x40;
    }
    for class in [
        ".?AVAiBaseStateMachine@Leonardo@@",
        ".?AVAiStateMachine@Leonardo@@",
    ] {
        logic.expect(
            "the machine top-state query",
            logic.method(class, machine_slot)?,
            &top_state,
        )?;
    }
    let resolved = Helicopter { site, slot };
    game.expect(HELICOPTER.name, site, &resolved.before())?;
    Ok(resolved)
}

pub(crate) fn menu(image: &Image) -> Result<usize, String> {
    let update = image.method(".?AVGameMenu@Leonardo@@", 7)?;
    let mask = image.find(&MENU)?;
    if !mask
        .checked_sub(update)
        .is_some_and(|delta| (0x800..0x1000).contains(&delta))
    {
        return Err("the deployment mask is outside the characterized menu update".into());
    }
    image.expect(MENU.name, mask, MENU_BEFORE)?;
    Ok(mask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};

    const TABLE: [usize; 8] = [
        0x23f9e4, 0x2442d4, 0x240d04, 0x245624, 0x240d04, 0x245624, 0x24dfc4, 0x2528b4,
    ];

    #[test]
    fn the_menu_mask_resolves_on_every_reference_build() {
        for (build, expected) in BUILDS.iter().zip(TABLE) {
            let Some(mapped) = reference(build, "game.dll") else {
                eprintln!("skipping {build}: no bin/{build}/game.dll");
                continue;
            };
            assert_eq!(menu(&Image::mapped(&mapped)), Ok(expected), "{build}");
            let logic = reference(build, "logic.dll").expect("matching logic.dll");
            assert_eq!(
                helicopter(&Image::mapped(&mapped), &Image::mapped(&logic)),
                Ok(Helicopter {
                    site: expected - 0x306,
                    slot: if build.contains("2025-12-23") {
                        0x2e0
                    } else {
                        0x2f0
                    }
                }),
                "{build}"
            );
        }
    }

    #[test]
    fn changed_state_queries_masks_and_continuations_are_refused() {
        let Some(mapped) = reference(BUILDS[0], "game.dll") else {
            return;
        };
        for offset in [0, 0x0d, 0x50, 0x51, 0x52, 0x56] {
            let mut changed = mapped.image.clone();
            changed[TABLE[0] - 0x50 + offset] ^= 1;
            assert!(
                menu(&Image {
                    image: &changed,
                    base: mapped.base
                })
                .is_err(),
                "offset {offset:#x}"
            );
        }
    }

    #[test]
    fn changed_repair_prerequisites_and_helicopter_query_are_refused() {
        let Some(game) = reference(BUILDS[0], "game.dll") else {
            return;
        };
        let logic = reference(BUILDS[0], "logic.dll").expect("matching logic.dll");
        let site = helicopter(&Image::mapped(&game), &Image::mapped(&logic))
            .unwrap()
            .site;
        for offset in [
            0, 0x20, 0x31, 0x3f, 0x49, 0x5b, 0x61, 0x65, 0x67, 0x69, 0x6d,
        ] {
            let mut changed = game.image.clone();
            changed[site - 0x5f + offset] ^= 1;
            assert!(helicopter(
                &Image {
                    image: &changed,
                    base: game.base
                },
                &Image::mapped(&logic)
            )
            .is_err());
        }
        for (class, slot, offset) in [
            (".?AVAiUtilsImpl@Leonardo@@", 92, 0x13),
            (".?AVAiFacet@Leonardo@@", 13, 0x1b),
            (".?AVAiBaseStateMachine@Leonardo@@", 18, 0x15),
        ] {
            let query = Image::mapped(&logic).method(class, slot).unwrap();
            let mut changed = logic.image.clone();
            changed[query + offset] ^= 1;
            assert!(helicopter(
                &Image::mapped(&game),
                &Image {
                    image: &changed,
                    base: logic.base
                }
            )
            .is_err());
        }
    }
}
