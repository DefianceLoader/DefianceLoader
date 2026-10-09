//! Core's `game-symbols` service: game functions that several features bind,
//! resolved once by name.
//!
//! Each [`Symbol`] carries what a feature used to keep beside its own copy of
//! the signature: the module, the ABI it is called with, the uses it allows,
//! and the bytes that must be at the site. Resolution reads the module's
//! original image, so another feature's hook on the same entry does not hide
//! it, and a symbol resolves only when its signature finds exactly one place
//! and its expected bytes are there.
//!
//! The service only names addresses. A feature still hooks through the
//! loader's exclusive `hook_exact` and declares the write in its contract, so
//! a symbol is owned and refused exactly as a site the feature found itself.
use core::ffi::{c_char, CStr};
use std::sync::OnceLock;

use defiance_api::{
    GameSymbolV1, GameSymbolsV1, SYMBOL_ABI_MISMATCH, SYMBOL_AMBIGUOUS, SYMBOL_INVALID,
    SYMBOL_KNOWN_BUILD, SYMBOL_MISUSE, SYMBOL_OK, SYMBOL_UNAVAILABLE, SYMBOL_UNKNOWN_NAME,
    SYMBOL_USE_ADDRESS, SYMBOL_USE_CALL, SYMBOL_USE_CALL_SITE, SYMBOL_USE_ENTRY_HOOK,
};
use defiance_core::sites::Image;

/// One catalog entry.
pub(crate) struct Symbol {
    pub name: &'static str,
    pub module: &'static CStr,
    /// The ABI version a caller must ask for; documented with the entry in
    /// docs/plugin-api.md.
    pub abi: u32,
    /// The `SYMBOL_USE_*` values allowed beyond `SYMBOL_USE_ADDRESS`, as bits.
    pub uses: u32,
    /// `tools/sigs.py`'s encoding; the symbol is at its start.
    pub signature: &'static str,
    /// The bytes that must be at the symbol in the original image.
    pub expected: &'static [u8],
    /// The entry hook's instruction-aligned span: a prefix of `expected`, 0
    /// unless `uses` allows an entry hook.
    pub span: u32,
}

const fn bit(use_: u32) -> u32 {
    1 << use_
}

/// The symbols Core resolves. Each entry is documented in docs/plugin-api.md.
pub(crate) const CATALOG: &[Symbol] = &[
    // `(menu, entity)`: redraws the vehicle ammunition menu for the entity.
    Symbol {
        name: "ammo menu redraw",
        module: c"game.dll",
        abi: 1,
        uses: bit(SYMBOL_USE_CALL) | bit(SYMBOL_USE_ENTRY_HOOK),
        signature: "488954241048894c24085741544881ece8000000488b02488bf9",
        // mov [rsp+10],rdx; mov [rsp+8],rcx; push rdi; push r12;
        // sub rsp,0xe8
        expected: &[
            0x48, 0x89, 0x54, 0x24, 0x10, 0x48, 0x89, 0x4c, 0x24, 0x08, 0x57, 0x41, 0x54, 0x48,
            0x81, 0xec, 0xe8, 0x00, 0x00, 0x00,
        ],
        span: 20,
    },
    // `(widget, *const [i32; 2] delta)`: moves a widget by the delta.
    Symbol {
        name: "widget move",
        module: c"game.dll",
        abi: 1,
        uses: bit(SYMBOL_USE_CALL),
        signature: "4889742410574883ec40833a00488bf2488bf9750a837a0400",
        expected: &[
            0x48, 0x89, 0x74, 0x24, 0x10, 0x57, 0x48, 0x83, 0xec, 0x40, 0x83, 0x3a, 0x00, 0x48,
            0x8b, 0xf2,
        ],
        span: 0,
    },
];

/// The rva of `symbol` in `image`, or the status that refuses it.
pub(crate) fn find(symbol: &Symbol, image: &Image) -> Result<usize, u32> {
    let pattern = defiance_core::pattern::parse(symbol.signature).map_err(|_| SYMBOL_INVALID)?;
    match defiance_core::scan::scan(image.image, &pattern)[..] {
        [rva] if image.image.get(rva..rva + symbol.expected.len()) == Some(symbol.expected) => {
            Ok(rva)
        }
        [] | [_] => Err(SYMBOL_UNAVAILABLE),
        _ => Err(SYMBOL_AMBIGUOUS),
    }
}

/// The catalog index of `name` if `abi` and `use_` fit it, else the status
/// that refuses the request.
pub(crate) fn check(catalog: &[Symbol], name: &str, abi: u32, use_: u32) -> Result<usize, u32> {
    if use_ > SYMBOL_USE_CALL_SITE {
        return Err(SYMBOL_INVALID);
    }
    let index = catalog
        .iter()
        .position(|symbol| symbol.name == name)
        .ok_or(SYMBOL_UNKNOWN_NAME)?;
    let symbol = &catalog[index];
    if symbol.abi != abi {
        return Err(SYMBOL_ABI_MISMATCH);
    }
    if use_ != SYMBOL_USE_ADDRESS && symbol.uses & bit(use_) == 0 {
        return Err(SYMBOL_MISUSE);
    }
    Ok(index)
}

struct Resolved {
    /// Per catalog entry: (module base, rva), or the status that refuses it.
    found: Vec<Result<(usize, usize), u32>>,
    /// Core's build name when the build is one Core knows.
    build: Option<&'static CStr>,
}

static RESOLVED: OnceLock<Resolved> = OnceLock::new();

/// Resolves every catalog entry in the loaded modules' original images.
/// `build` is Core's build name, or None for a build Core does not know.
pub(crate) fn resolve_loaded(api: &defiance_api::Api, build: Option<&'static CStr>) {
    let mut images: Vec<(&CStr, Option<(usize, Vec<u8>)>)> = Vec::new();
    let found = CATALOG
        .iter()
        .map(|symbol| {
            let at = match images.iter().position(|(m, _)| *m == symbol.module) {
                Some(at) => at,
                None => {
                    let base = unsafe { (api.module_base)(symbol.module.as_ptr()) } as *const u8;
                    let image = (!base.is_null())
                        .then(|| {
                            let size = unsafe { (api.module_size)(base.cast_mut().cast()) };
                            crate::plan::original_image(base, size)
                        })
                        .flatten()
                        .map(|image| (base as usize, image));
                    images.push((symbol.module, image));
                    images.len() - 1
                }
            };
            let (base, image) = images[at].1.as_ref().ok_or(SYMBOL_UNAVAILABLE)?;
            let rva = find(symbol, &Image { image, base: *base })?;
            Ok((*base, rva))
        })
        .collect();
    let _ = RESOLVED.set(Resolved { found, build });
}

/// The names that did not resolve, with their statuses, for Core's log.
pub(crate) fn unresolved() -> Vec<(&'static str, u32)> {
    RESOLVED.get().map_or(Vec::new(), |resolved| {
        CATALOG
            .iter()
            .zip(&resolved.found)
            .filter_map(|(symbol, found)| found.err().map(|status| (symbol.name, status)))
            .collect()
    })
}

fn fill(symbol: &Symbol, base: usize, rva: usize, build: Option<&'static CStr>) -> GameSymbolV1 {
    GameSymbolV1 {
        module: symbol.module.as_ptr(),
        build: build.map_or(core::ptr::null(), CStr::as_ptr),
        rva,
        address: base + rva,
        expected: symbol.expected.as_ptr(),
        expected_len: symbol.expected.len(),
        span: if symbol.uses & bit(SYMBOL_USE_ENTRY_HOOK) != 0 {
            symbol.span
        } else {
            0
        },
        flags: if build.is_some() {
            SYMBOL_KNOWN_BUILD
        } else {
            0
        },
    }
}

unsafe extern "C" fn resolve(
    name: *const c_char,
    abi: u32,
    use_: u32,
    out: *mut GameSymbolV1,
) -> u32 {
    if name.is_null() || out.is_null() {
        return SYMBOL_INVALID;
    }
    let Ok(name) = unsafe { CStr::from_ptr(name) }.to_str() else {
        return SYMBOL_INVALID;
    };
    let index = match check(CATALOG, name, abi, use_) {
        Ok(index) => index,
        Err(status) => return status,
    };
    let Some(resolved) = RESOLVED.get() else {
        return SYMBOL_UNAVAILABLE;
    };
    match resolved.found[index] {
        Ok((base, rva)) => {
            unsafe { out.write(fill(&CATALOG[index], base, rva, resolved.build)) };
            SYMBOL_OK
        }
        Err(status) => status,
    }
}

pub static API: GameSymbolsV1 = GameSymbolsV1 { resolve };

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};

    /// Per build: the redraw's and the widget move's game.dll rvas.
    const RVAS: [(&str, usize, usize); 8] = [
        ("gog/2025-12-23", 0x3ed10, 0x2d1920),
        ("steam/2025-12-23", 0x3ed10, 0x2d6cb0),
        ("gog/2026-09-14", 0x3eeb0, 0x2d3ab0),
        ("steam/2026-09-22", 0x3eeb0, 0x2d8e70),
        ("gog/2026-09-25", 0x3eeb0, 0x2d3ab0),
        ("steam/2026-09-25", 0x3eeb0, 0x2d8e70),
        ("gog/2026-10-07", 0x3efc0, 0x2e2650),
        ("steam/2026-10-07", 0x3efc0, 0x2e79e0),
    ];

    #[test]
    fn the_catalog_resolves_on_every_build_in_bin() {
        assert_eq!(RVAS.map(|(build, ..)| build), BUILDS);
        for (build, redraw, widget_move) in RVAS {
            let Some(mapped) = reference(build, "game.dll") else {
                continue;
            };
            let image = Image::mapped(&mapped);
            let found: Vec<_> = CATALOG.iter().map(|s| find(s, &image)).collect();
            assert_eq!(found, [Ok(redraw), Ok(widget_move)], "{build}");
        }
    }

    #[test]
    fn the_catalog_is_consistent() {
        for (i, symbol) in CATALOG.iter().enumerate() {
            let hook = symbol.uses & bit(SYMBOL_USE_ENTRY_HOOK) != 0;
            assert_eq!(hook, symbol.span > 0, "{}", symbol.name);
            assert!(
                symbol.span as usize <= symbol.expected.len(),
                "{}",
                symbol.name
            );
            assert!(defiance_core::pattern::parse(symbol.signature).is_ok());
            assert!(CATALOG[..i].iter().all(|s| s.name != symbol.name));
            if symbol.uses & bit(SYMBOL_USE_CALL_SITE) != 0 {
                assert_eq!(symbol.expected.first(), Some(&0xe8), "{}", symbol.name);
            }
        }
    }

    /// A catalog with a call site, which no shipped entry is yet.
    const SYNTHETIC: &[Symbol] = &[
        Symbol {
            name: "entry",
            module: c"game.dll",
            abi: 2,
            uses: bit(SYMBOL_USE_CALL) | bit(SYMBOL_USE_ENTRY_HOOK),
            signature: "5548 89e5 ????",
            expected: &[0x55, 0x48, 0x89, 0xe5],
            span: 4,
        },
        Symbol {
            name: "call site",
            module: c"game.dll",
            abi: 1,
            uses: bit(SYMBOL_USE_CALL_SITE),
            signature: "e8???????? 90 c3",
            expected: &[0xe8],
            span: 0,
        },
    ];

    #[test]
    fn requests_are_checked_against_the_entry() {
        assert_eq!(check(SYNTHETIC, "entry", 2, SYMBOL_USE_ENTRY_HOOK), Ok(0));
        assert_eq!(check(SYNTHETIC, "entry", 2, SYMBOL_USE_ADDRESS), Ok(0));
        assert_eq!(
            check(SYNTHETIC, "call site", 1, SYMBOL_USE_CALL_SITE),
            Ok(1)
        );
        assert_eq!(
            check(SYNTHETIC, "missing", 1, SYMBOL_USE_CALL),
            Err(SYMBOL_UNKNOWN_NAME)
        );
        assert_eq!(
            check(SYNTHETIC, "entry", 1, SYMBOL_USE_CALL),
            Err(SYMBOL_ABI_MISMATCH)
        );
        // Entry versus call site, either way round.
        assert_eq!(
            check(SYNTHETIC, "call site", 1, SYMBOL_USE_ENTRY_HOOK),
            Err(SYMBOL_MISUSE)
        );
        assert_eq!(
            check(SYNTHETIC, "call site", 1, SYMBOL_USE_CALL),
            Err(SYMBOL_MISUSE)
        );
        assert_eq!(
            check(SYNTHETIC, "entry", 2, SYMBOL_USE_CALL_SITE),
            Err(SYMBOL_MISUSE)
        );
        assert_eq!(check(SYNTHETIC, "entry", 2, 9), Err(SYMBOL_INVALID));
        // The shipped widget move allows no entry hook.
        assert_eq!(
            check(CATALOG, "widget move", 1, SYMBOL_USE_ENTRY_HOOK),
            Err(SYMBOL_MISUSE)
        );
    }

    #[test]
    fn missing_ambiguous_and_altered_sites_are_refused() {
        let entry = &SYNTHETIC[0];
        let at = |image: &[u8]| {
            find(
                entry,
                &Image {
                    image,
                    base: 0x1000,
                },
            )
        };
        let one = [0xcc, 0x55, 0x48, 0x89, 0xe5, 1, 2, 0xcc];
        assert_eq!(at(&one), Ok(1));
        assert_eq!(at(&[0xcc; 8]), Err(SYMBOL_UNAVAILABLE));
        let two = [one, one].concat();
        assert_eq!(at(&two), Err(SYMBOL_AMBIGUOUS));
        // The signature still matches, but a byte it wildcards is not the
        // expected one.
        let call = Symbol {
            expected: &[0xe8, 1],
            ..SYNTHETIC[1]
        };
        let site = [0xe8, 2, 0, 0, 0, 0x90, 0xc3];
        let image = Image {
            image: &site,
            base: 0,
        };
        assert_eq!(find(&call, &image), Err(SYMBOL_UNAVAILABLE));
        assert_eq!(find(&SYNTHETIC[1], &image), Ok(0));
    }

    #[test]
    fn a_changed_redraw_prologue_is_refused() {
        let Some(mapped) = reference(RVAS[0].0, "game.dll") else {
            return;
        };
        for at in [0, 10, 16] {
            let mut changed = mapped.image.clone();
            changed[RVAS[0].1 + at] ^= 8;
            let image = Image {
                image: &changed,
                base: mapped.base,
            };
            assert_eq!(find(&CATALOG[0], &image), Err(SYMBOL_UNAVAILABLE), "{at}");
        }
    }

    #[test]
    fn a_resolved_symbol_reports_its_build() {
        let known = fill(&CATALOG[0], 0x1000, 0x20, Some(c"gog/2025-12-23"));
        assert_eq!(
            (known.address, known.span, known.flags),
            (0x1020, 20, SYMBOL_KNOWN_BUILD)
        );
        assert_eq!(unsafe { CStr::from_ptr(known.build) }, c"gog/2025-12-23");
        // On an unknown build the address is still given, without the claim.
        let unknown = fill(&CATALOG[1], 0x1000, 0x20, None);
        assert_eq!((unknown.span, unknown.flags), (0, 0));
        assert!(unknown.build.is_null());
    }

    #[test]
    fn invalid_arguments_are_refused() {
        let mut out = core::mem::MaybeUninit::<GameSymbolV1>::uninit();
        unsafe {
            assert_eq!(
                resolve(core::ptr::null(), 1, 0, out.as_mut_ptr()),
                SYMBOL_INVALID
            );
            assert_eq!(
                resolve(c"widget move".as_ptr(), 1, 0, core::ptr::null_mut()),
                SYMBOL_INVALID
            );
            assert_eq!(
                resolve(c"nothing".as_ptr(), 1, 0, out.as_mut_ptr()),
                SYMBOL_UNKNOWN_NAME
            );
        }
    }
}
