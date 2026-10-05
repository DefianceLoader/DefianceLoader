//! Where the ability bar's native code and game menu fields live, found by
//! signature instead of looked up by build hash.
//!
//! Functions are found by byte signature (`tools/sigs.py`'s encoding: rel32
//! targets and RIP displacements wildcarded, struct offsets kept). The two
//! game menu fields are read from the order panel reset, which clears both.
//! Each site must resolve to exactly one place and every hooked entry must
//! decode to the span the hook displaces, so a build where any of it moved or
//! changed shape resolves to an error and the plugin hooks nothing. Each
//! function is described in [`crate::native`], where it is called or hooked.
use defiance_core::{
    decode,
    sites::{sig, Image, Signature},
};

/// The ability bar update, hooked.
pub const UPDATE: usize = 0;
/// The ability bar reset on a selection change, hooked.
pub const RESET: usize = 1;
/// The ability key, hooked.
pub const KEY: usize = 2;
/// The ability bar destructor, hooked.
pub const BAR_DESTROY: usize = 3;
/// The order key, hooked.
pub const ORDER_KEY: usize = 4;
/// Every GUI button's click, hooked.
pub const CLICK: usize = 5;
pub const HIDE: usize = 6;
pub const MOVE_WIDGET: usize = 7;
pub const LABEL: usize = 8;
pub const CLOSE_SUBMENUS: usize = 9;
pub const CLEAR_ORDER: usize = 10;
/// The order panel reset, which also clears the two game menu fields.
pub const ORDER_RESET: usize = 11;
pub const HIDE_ORDERS: usize = 12;
/// The number of sites.
pub const COUNT: usize = 13;

/// The hooked sites and the bytes each hook displaces.
pub const HOOKS: [(usize, usize); 6] = [
    (UPDATE, 16),
    (RESET, 15),
    (KEY, 17),
    (BAR_DESTROY, 15),
    (ORDER_KEY, 16),
    (CLICK, 15),
];

/// The resolved sites, as rvas, and the game menu fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sites {
    pub rvas: [usize; COUNT],
    /// The game menu's byte that keeps the order panel hidden while a submenu
    /// is open.
    pub orders_hidden: usize,
    /// The game menu's submenu state: 3 while one is open, else 0.
    pub submenu: usize,
}

const SIGNATURES: [Signature; COUNT] = [
    sig(
        "ability update",
        &[("89542410564883ec30488bf1440fb6d248b9b301000000010000", 0x0)],
    ),
    sig(
        "ability reset",
        &[("48895c24084889742410574883ec20488bf1488b49784885c9", 0x0)],
    ),
    sig(
        "ability key",
        &[("4056574883ec780fb632488bf9488b490848634710488d1440", 0x0)],
    ),
    sig(
        "ability bar destructor",
        &[(
            "48895c242048894c240855565741544155415641574881ec80000000",
            0x0,
        )],
    ),
    sig(
        "order key",
        &[("48895c2408574883ec20488bf90fb6da488b494080b98901000000", 0x0)],
    ),
    sig(
        "button click",
        &[("48895c24084889742410574883ec7083b96802000001498bf8", 0x0)],
    ),
    sig(
        "hide",
        &[(
            "89542410534883ec204c8bc9440fb6c248b9b30100000001000048b825232284e49cf2cb4c33c00fb64424394c0fafc14c33c00fb644243a4c0fafc14c33c00fb644243b4c0fafc14c33c0498b4120",
            0x0,
        )],
    ),
    sig(
        "move widget",
        &[("4889742410574883ec40833a00488bf2488bf9750a837a0400", 0x0)],
    ),
    sig(
        "label",
        &[("48895c240848896c24104889742418574883ec3048837a1810", 0x0)],
    ),
    sig(
        "close submenus",
        &[("48895c2408574883ec20488bf9488b49784885c97406488b01", 0x0)],
    ),
    sig(
        "clear order",
        &[
            (
                "40534883ec20488b81180e0000488bd9488b882001000080b99c08000000",
                0x0,
            ),
            (
                "40534883ec20488b81180e0000488bd9488b884001000080b99c08000000",
                0x0,
            ),
        ],
    ),
    sig(
        "order reset",
        &[("40534883ec20488bd9e8????????488b8be80d0000488b01", 0x0)],
    ),
    sig(
        "hide orders",
        &[("48895c24084889742410574883ec20488d99b8010000488bf1", 0x0)],
    ),
];

/// The resolved sites, or why this build is not supported.
pub fn sites(image: &Image) -> Result<Sites, String> {
    let mut rvas = [0; COUNT];
    for (rva, signature) in rvas.iter_mut().zip(&SIGNATURES) {
        *rva = image.find(signature)?;
    }
    for (index, span) in HOOKS {
        entry(image, SIGNATURES[index].name, rvas[index], span)?;
    }
    // The order panel reset clears both fields: `mov byte [rbx+disp32], 0`
    // and then `mov dword [rbx+disp32], 0`.
    let field = |what: &str, rva: usize, opcode: &[u8]| -> Result<usize, String> {
        image.expect(what, rva, opcode)?;
        image
            .u32(rva + 2)
            .map(|disp| disp as usize)
            .ok_or_else(|| format!("{what} runs off the image"))
    };
    Ok(Sites {
        rvas,
        orders_hidden: field("orders hidden field", rvas[ORDER_RESET] + 0x5a, b"\xc6\x83")?,
        submenu: field("submenu field", rvas[ORDER_RESET] + 0x61, b"\xc7\x83")?,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};

    /// The rvas and fields the per-build hash table held before the plugin
    /// resolved them, in [`BUILDS`] order.
    const TABLE: [(&str, [usize; COUNT], usize, usize); 6] = [
        (
            "gog/2025-12-23",
            [
                0x1cd70, 0x1cca0, 0x1fba0, 0x1ab90, 0x23c5f0, 0x2aecb0, 0x1cfa0, 0x2d1920,
                0x240950, 0x1cc30, 0x241340, 0x23ebe0, 0x240ea0,
            ],
            0xe58,
            0xe78,
        ),
        (
            "steam/2025-12-23",
            [
                0x1cd70, 0x1cca0, 0x1fba0, 0x1ab90, 0x240ee0, 0x2b4040, 0x1cfa0, 0x2d6cb0,
                0x245240, 0x1cc30, 0x245c30, 0x2434d0, 0x245790,
            ],
            0xe58,
            0xe78,
        ),
        (
            "gog/2026-09-14",
            [
                0x1cec0, 0x1cdf0, 0x1fd40, 0x1acd0, 0x23d900, 0x2b0d30, 0x1d0f0, 0x2d3ab0,
                0x241c70, 0x1cd80, 0x242660, 0x23fef0, 0x2421c0,
            ],
            0xe58,
            0xe78,
        ),
        (
            "steam/2026-09-22",
            [
                0x1cec0, 0x1cdf0, 0x1fd40, 0x1acd0, 0x242220, 0x2b60f0, 0x1d0f0, 0x2d8e70,
                0x246590, 0x1cd80, 0x246f80, 0x244810, 0x246ae0,
            ],
            0xe58,
            0xe78,
        ),
        (
            "gog/2026-09-25",
            [
                0x1cec0, 0x1cdf0, 0x1fd40, 0x1acd0, 0x23d900, 0x2b0d30, 0x1d0f0, 0x2d3ab0,
                0x241c70, 0x1cd80, 0x242660, 0x23fef0, 0x2421c0,
            ],
            0xe58,
            0xe78,
        ),
        (
            "steam/2026-09-25",
            [
                0x1cec0, 0x1cdf0, 0x1fd40, 0x1acd0, 0x242220, 0x2b60f0, 0x1d0f0, 0x2d8e70,
                0x246590, 0x1cd80, 0x246f80, 0x244810, 0x246ae0,
            ],
            0xe58,
            0xe78,
        ),
    ];

    #[test]
    fn every_build_resolves_to_the_old_table() {
        assert_eq!(TABLE.map(|(build, ..)| build), BUILDS);
        for (build, rvas, orders_hidden, submenu) in TABLE {
            let Some(game) = reference(build, "game.dll") else {
                continue;
            };
            let resolved = sites(&Image::mapped(&game)).unwrap_or_else(|e| panic!("{build}: {e}"));
            assert_eq!(
                resolved,
                Sites {
                    rvas,
                    orders_hidden,
                    submenu
                },
                "{build}"
            );
        }
    }

    #[test]
    fn a_changed_entry_is_refused() {
        let Some(game) = reference(BUILDS[0], "game.dll") else {
            return;
        };
        let image = Image::mapped(&game);
        let resolved = sites(&image).unwrap();
        for (index, _) in HOOKS {
            let mut changed = image.image.to_vec();
            changed[resolved.rvas[index] + 4] ^= 1;
            let changed = Image {
                image: &changed,
                base: image.base,
            };
            assert!(sites(&changed).is_err(), "{}", SIGNATURES[index].name);
        }
    }
}
