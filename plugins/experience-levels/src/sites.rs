//! Where the experience-levels code lives in a logic.dll, found by signature
//! instead of looked up by build hash.
//!
//! Each signature is the whole function (up to 512 bytes) with only rel32
//! branches and RIP displacements wildcarded, so every struct offset and
//! constant [`crate::native`] relies on is fixed: the reader's six-column loop,
//! the row view's header and cell layout, and each adapter's `size == 6` check
//! and resolver call. A build that changed any of them does not match, and
//! nothing is hooked. They resolve in the 2026 builds; the December 2025
//! builds differ and are not supported.
use defiance_core::sites::{sig, Image, Signature};

/// The row reader's entry instructions the hook relocates.
pub(crate) const READER_BEFORE: &[u8] = &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x18];

/// One cell adapter: parses a row's six cells into an output vector.
pub(crate) struct Adapter {
    pub(crate) what: &'static str,
    signature: Signature,
    /// The entry instructions the hook relocates.
    pub(crate) before: &'static [u8],
    /// Where in the adapter it calls the resolver that maps the field's row
    /// name to its levels object.
    resolve_call: usize,
}

/// The adapters, in the order of `native::ADAPTER_DETOURS`: pairs, triples,
/// ints, floats, floats scaled by 10, ints scaled by 100. The first two call
/// one resolver, the other four another.
pub(crate) const ADAPTERS: [Adapter; 6] = [
    Adapter {
        what: "the pairs adapter",
        signature: PAIRS,
        before: &[0x48, 0x89, 0x5c, 0x24, 0x10],
        resolve_call: 0xa1,
    },
    Adapter {
        what: "the triples adapter",
        signature: TRIPLES,
        before: &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10],
        resolve_call: 0xd5,
    },
    Adapter {
        what: "the ints adapter",
        signature: INTS,
        before: &[0x4c, 0x8b, 0xdc, 0x49, 0x89, 0x5b, 0x08],
        resolve_call: 0x40,
    },
    Adapter {
        what: "the floats adapter",
        signature: FLOATS,
        before: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        resolve_call: 0xd1,
    },
    Adapter {
        what: "the scaled floats adapter",
        signature: FLOATS10,
        before: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        resolve_call: 0xd1,
    },
    Adapter {
        what: "the scaled ints adapter",
        signature: INTS100,
        before: &[0x48, 0x89, 0x5c, 0x24, 0x08],
        resolve_call: 0xbe,
    },
];

/// Which resolver each adapter calls, as an index into [`Sites::resolvers`].
const RESOLVER_OF: [usize; 6] = [0, 0, 1, 1, 1, 1];

/// Every rva the plugin hooks or calls.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Sites {
    pub(crate) reader: usize,
    pub(crate) lookup: usize,
    /// The adapters' entries, in [`ADAPTERS`] order.
    pub(crate) adapters: [usize; 6],
    /// Each adapter's resolver call.
    pub(crate) calls: [usize; 6],
    /// The two resolvers those calls reach.
    pub(crate) resolvers: [usize; 2],
}

/// The sites in this image, or why this build is not supported.
pub(crate) fn find(image: &Image) -> Result<Sites, String> {
    let reader = image.find(&READER)?;
    image.expect("the row reader", reader, READER_BEFORE)?;
    let lookup = image.find(&LOOKUP)?;
    let mut adapters = [0; 6];
    let mut calls = [0; 6];
    let mut resolvers = [None; 2];
    for (i, adapter) in ADAPTERS.iter().enumerate() {
        let rva = image.find(&adapter.signature)?;
        image.expect(adapter.what, rva, adapter.before)?;
        let call = rva + adapter.resolve_call;
        let target = image
            .starts_with(call, &[0xe8])
            .then(|| image.branch_target(call))
            .flatten()
            .ok_or_else(|| format!("{} has no resolver call at {call:#x}", adapter.what))?;
        match resolvers[RESOLVER_OF[i]].replace(target) {
            Some(other) if other != target => {
                return Err(format!(
                    "{} calls {target:#x}, not the resolver {other:#x}",
                    adapter.what
                ));
            }
            _ => {}
        }
        adapters[i] = rva;
        calls[i] = call;
    }
    let [Some(a), Some(b)] = resolvers else {
        unreachable!("every resolver has an adapter");
    };
    if a == b {
        return Err("the adapters' two resolvers are one function".into());
    }
    Ok(Sites {
        reader,
        lookup,
        adapters,
        calls,
        resolvers: [a, b],
    })
}

const READER: Signature = sig(
    "the row reader",
    &[(
        "488bc4488958185556574154415541564157488da8c8fdffff4881ec000300000f2970b80f2978a84c8bf133ff488bb1c80000004885f67445488d8d40020000e8????????488b18488b06488d9548020000488bceff5008488b008b4b50394850721b483bc3740c488b4828488bc14885c975ef4885c07405483bc37403488bf74d8b4e184d8d4620498b16488d4dc0e8????????4889b5880000004883c6084c8bc6488b15????????488d4de0e8????????483bf0741a4c8b401048837818107203488b00488bd0488bcee8????????90488d4d40e8????????488d4de8e8????????488bf7488d44243048898540020000488d05????????488944245048c74424580600000048c74424204d000000488d45a0488944242841bc0f0000004c8d2d????????0f287424500f287c242048897c24304c8964244848c7442440060000008b05????????894424300fb705????????6689442434c644243600488d442430488944242048c74424280600000048897424500f104424200f1145a00f104c24500f114db048897dd04c896dc0488d45e0488945c848c745d8f4010000660f7f7d80660f7f75904c8bcf4c8d4580488d5590488d4dc0e8????????48897c246048897c24704c89642478c6442460004c8b45d0488b55c8488d4c2460e8????????904c896dc0488d45e0488b4dc8483bc8742f488b55d8488bc1",
        0,
    )],
);

const LOOKUP: Signature = sig(
    "the cell lookup",
    &[(
        "48895c240848896c241048897424185741544155415641574881ec300100004d8bf0488bfa4c8be9488b41584c8b204c8964242048c7442428010000000f28442420660f7f442420488b680848c1e5054903ec498bdc498bc44c8b7c24286690483bc575064983ff017436488bd748837f18107203488b174c8b4310488bcb48837b18107203488b0b4c3b47107509e8????????85c074094883c320488bc3ebbf483bdd0f85????????488d4c2430e8????????90488d15????????488d4c2440e8????????498b95980000004c8b421048837a18107203488b12488bc8e8????????488d15????????488bc8e8????????4c8b471048837f18107203488b3f488bd7488bc8e8????????49837e1000742b488d15????????488d4c2440e8????????498bd649837e18107203498b164d8b4610488bc8e8????????90488b44243048634804488d05????????4889440c30488b442430486348048d9168ffffff89540c2c488d4c2448e8????????488d4c2450ff15????????488d8c24c8000000ff15????????498bc6eb27498b4d50492bdc48c1fb05488b4118490faf45304803c348c1e0054803014883781000490f44c64c8d9c2430010000498b5b30498b6b38498b7340498be3415f415e415d415c5fc3",
        0,
    )],
);

const PAIRS: Signature = sig(
    "the pairs adapter",
    &[(
        "48895c2410488974241848897c24205541564157488bec4883ec40498bf8488bda4c8bf14c8d05????????488bd74883c120e8????????488bf04c8b40104d85c00f84????????488bc848837818107203488b084983f8017514488d15????????e8????????85c00f84????????488bd7498bcee8????????84c00f85????????33ff48897de048897df048c745f80f00000040887de04c8d45e0488bd6498bcee8????????488bf0488b55f84883fa10723448ffc2488b4de0488bc14881fa00100000721c4883c227488b49f8482bc14883c0f84883f81f7607ff15????????cce8????????48897df048c745f80f000000c645e0004885f67478488b4e28488b4630483bc8746b482bc14883e0e0483dc0000000755c0f1f84000000000048c7452000000000488b4e284803cf4c8d4524488d5520e8????????488b5308483b53107410f20f104520f20f11024883430808eb0c4c8d4520488bcbe8????????4883c7204881ffc000000072b1e9????????bf060000000f1f800000000048c7452000000000488b5308483b53107410f20f104520f20f11024883430808eb0c4c8d4520488bcbe8????????4883ef0175cceb6648c745200000000048837e100074234c8d05????????488bd7498d4e20e8????????4c8d4524488d5520488bc8e8????????bf060000000f1f00488b5308483b53107410f20f104520",
        0,
    )],
);

const TRIPLES: Signature = sig(
    "the triples adapter",
    &[(
        "488bc4488958104889701848897820554154415541564157488d68a14881ec000100000f2970c80f2978b84d8be0488bfa4c8bf94c8d05????????498bd44883c120e8????????488bd8488b70104885f60f84????????488bc84c8b70184983fe107203488b084883fe0175174c8bc6488d15????????e8????????85c00f84????????488bcb4983fe107203488b0b48c7442420010000004c8d0d????????4533c0488bd6e8????????4883f8ff0f85????????33f6488975b7488975c748c745cf0f000000408875b74c8d45b7488bd3498bcfe8????????4c8bf8488b55cf4883fa10723448ffc2488b4db7488bc14881fa00100000721c4883c227488b49f8482bc14883c0f84883f81f7607ff15????????cce8????????488975c748c745cf0f000000c645b7004d85ff0f84????????498b4f28498b4730483bc80f84????????482bc148c1f8054883f8060f85????????0f2805????????0f11459ff30f1035????????0f28fe4c8bf64c8d25????????66660f1f8400000000000f57c0f30f7f458748897597488974243048c74424480f00000048c744244004000000c74424303b7c2c20c644243400498b4f284903ce4c8965d7488d4587488945df488d45d74889450f4c8d45d7488d542430e8????????90488b5424484883fa10723248ffc2488b4c2430488bc14881fa0010000072194883c227",
        0,
    )],
);

const INTS: Signature = sig(
    "the ints adapter",
    &[(
        "4c8bdc49895b0849896b104989731845894b20574883ec50498bc0488bda33ff897c242049897bd049897be049c743e80f00000040887c24284d8d43d0488bd0e8????????488be8488b5424404883fa10723548ffc2488b4c2428488bc14881fa00100000721c4883c227488b49f8482bc14883c0f84883f81f7607ff15????????cce8????????4885ed7472488b4d28488b4530483bc87465482bc14883e0e0483dc00000007556488bf70f1f4000488b4d284803ce4c8d05????????8bd7e8????????89442478488b5308483b5310740989024883430804eb0d4c8d442478488bcbe8????????488b43088b78fc4883c6204881fec000000072b3eb2fbe06000000488b5308483b53107409893a4883430804eb114c8d442420488bcbe8????????8b7c24204883ee0175d6488b5c2460488b6c2468488b7424704883c4505fc3",
        0,
    )],
);

const FLOATS: Signature = sig(
    "the floats adapter",
    &[(
        "48895c240848896c24104889742418f30f115c2420574883ec600f29742450498bf0488bda488be9f30f1035????????f30f11742420498bd0e8????????84c0746a4c8d842488000000488bd6488bcde8????????84c07409f30f10b42488000000f30f11b42488000000bf06000000488b5308483b5310740ef30f1132488d420448894308eb194c8d842488000000488bcbe8????????f30f10b424880000004883ef0175c9e9????????33ff48897c242848897c243848c74424400f00000040887c24284c8d442428488bd6488bcde8????????488bf0488b5424404883fa10723548ffc2488b4c2428488bc14881fa00100000721c4883c227488b49f8482bc14883c0f84883f81f7607ff15????????cce8????????4885f60f84????????488b4e28488b4630483bc87470482bc14883e0e0483dc000000075616690488b4e284803cf4c8d05????????0f28cee8????????f30f11842488000000488b5308483b5310740ef30f1102488d420448894308eb104c8d842488000000488bcbe8????????488b4308f30f1070fc4883c7204881ffc000000072a3eb36bf06000000488b5308483b5310740ef30f1132488d420448894308eb134c8d442420488bcbe8????????f30f107424204883ef0175cf488b5c2470488b6c2478488bb424800000000f287424504883c4605fc3",
        0,
    )],
);

const FLOATS10: Signature = sig(
    "the scaled floats adapter",
    &[(
        "48895c240848896c24104889742418f30f115c2420574883ec600f29742450498bf0488bda488be9f30f1035????????f30f11742420498bd0e8????????84c0746a4c8d842488000000488bd6488bcde8????????84c07409f30f10b42488000000f30f11b42488000000bf06000000488b5308483b5310740ef30f1132488d420448894308eb194c8d842488000000488bcbe8????????f30f10b424880000004883ef0175c9e9????????33ff48897c242848897c243848c74424400f00000040887c24284c8d442428488bd6488bcde8????????488bf0488b5424404883fa10723548ffc2488b4c2428488bc14881fa00100000721c4883c227488b49f8482bc14883c0f84883f81f7607ff15????????cce8????????4885f60f84????????488b4e28488b4630483bc80f84????????482bc14883e0e0483dc00000007571ba0a000000660f1f840000000000488b4e284803cf660f6eca0f5bc94c8d05????????e8????????f30f11842488000000488b5308483b5310740ef30f1102488d420448894308eb104c8d842488000000488bcbe8????????488b4308f30f2c50fc4883c7204881ffc0000000729feb3ebf060000000f1f840000000000488b5308483b5310740ef30f1132488d420448894308eb134c8d442420488bcbe8????????f30f107424204883ef0175cf488b5c2470488b6c2478",
        0,
    )],
);

const INTS100: Signature = sig(
    "the scaled ints adapter",
    &[(
        "48895c240848896c241044894c2420565741564883ec50498be8488bda4c8bf1be6400000089742420498bd0e8????????84c07464498d4e204c8d05????????488bd5e8????????4c8d8424880000008bd6488bc8e8????????898424880000008d7ea2488b5308483b5310740989024883430804eb174c8d842488000000488bcbe8????????8b8424880000004883ef0175d0e9????????33ff48897c242848897c243848c74424400f00000040887c24284c8d442428488bd5498bcee8????????488be8488b5424404883fa10723548ffc2488b4c2428488bc14881fa00100000721c4883c227488b49f8482bc14883c0f84883f81f7607ff15????????cce8????????4885ed747a488b4d28488b4530483bc8746d482bc14883e0e0483dc0000000755e660f1f840000000000488b4d284803cf4c8d05????????8bd6e8????????89842488000000488b5308483b5310740989024883430804eb104c8d842488000000488bcbe8????????488b43088b70fc4883c7204881ffc000000072adeb35bf06000000660f1f440000488b5308483b5310740989324883430804eb114c8d442420488bcbe8????????8b7424204883ef0175d6488b5c2470488b6c24784883c450415e5f5ec3",
        0,
    )],
);

/// Core's name for the widget move ([`crate::resolve_meter`] asks for it).
pub(crate) const MOVE_WIDGET_SYMBOL: &std::ffi::CStr = c"widget move";

/// Where the experience meter code lives in a game.dll. The signatures fix the
/// panel's widget offsets and the progress bar's layout that
/// [`crate::meter`] reads and writes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct GameSites {
    /// The progress bar factory: `(out, record) -> out`.
    pub(crate) factory: usize,
    /// Gives a progress bar its image: `(bar, texture)`.
    pub(crate) image: usize,
    /// Recomputes a progress bar's clip: `(bar)`.
    pub(crate) clip: usize,
    /// Appends a child to a widget: `(parent, handle) -> child`.
    pub(crate) attach: usize,
    /// Binds a new widget to the GUI: `(widget)`.
    pub(crate) bind: usize,
}

/// The game.dll sites, or why this build's experience meter is not supported.
pub(crate) fn find_game(image: &Image) -> Result<GameSites, String> {
    Ok(GameSites {
        factory: image.find(&PROGRESS_FACTORY)?,
        image: image.find(&PROGRESS_IMAGE)?,
        clip: image.find(&PROGRESS_CLIP)?,
        attach: image.find(&ATTACH)?,
        bind: image.find(&BIND)?,
    })
}

/// The GOG and Steam builds differ only in one field offset near the end.

const PROGRESS_FACTORY: Signature = sig(
    "the progress bar factory",
    &[(
        "48895c241048894c24085556574883ec30488bfa488bf133ed48896c2450b9d0010000e8????????488bd84889442460488bd7488bc8e8????????488d05????????4889034889aba0010000c783a80100000000003f4889abb00100004889abb80100004889abc00100004088abc801000048895c2450488bd7488bcbe8????????48891e488bc6488b5c24584883c4305f5e5dc3",
        0,
    )],
);

const PROGRESS_IMAGE: Signature = sig(
    "the progress bar image",
    &[(
        "48895c240855565741544155415641574883ec404c8bea4c8bf933db899c2490000000b9a0010000e8????????4c8be04889842498000000498d7f18488b6f1048b8ffffffffffffff7f4c8bc04c2bc54983f8080f82????????48837f18107203488b3f48895c24204c8d7508bb0f000000488d7424204c3bf3763a498bde4983fe10480f421d????????4883cb0f483bd87605488bd8eb0cb816000000483bd8480f42d8488d4b01e8????????488bf048894424204c8974243048895c24384c8bc5488bd7488bcee8????????488b0d????????48890c2e41c6043600c784249000000003000000488d542420498bcce8????????488bf84889842498000000488b5424384883fa10723548ffc2488b4c2420488bc14881fa00100000721c4883c227488b49f8482bc14883c0f84883f81f7607ff15????????cce8????????488b07498d9770010000488bcfff9088000000488b07488b5828498b07498bcfff5040488bd0488bcfffd3498bd5488bcfe8????????48c7842498000000000000004889bc2490000000488d942490000000498bcfe8????????498987a0010000488b9c24800000004883c440415f415e415d415c5f5e5dc3e8????????90",
        0,
    )],
);

const PROGRESS_CLIP: Signature = sig(
    "the progress bar clip",
    &[(
        "48895c2418488974242055574156488d6c24b94881ec90000000488bf94883b9a0010000000f84????????488b01ff5040488bd88b108955ef8b50048955f38b4808894df78b480c894dfb488d5010488d4dffe8????????8b4328894517448b45f7418bf0448b5def412bf3448b4dfb458bf18b5df3442bf333c98bd1448b97ac0100004585d274284183fa017541f30f100d????????f30f5c8fa801000066410f6ec60f5bc0f30f59c8f30f2cc9eb1ff30f100d????????f30f5c8fa8010000660f6ec60f5bc0f30f59c8f30f2cd180bfc801000000741b428d041a894567895d6b442bc94489456f44894d73488d4d1feb19442bc24489456744894d6b8d041944895d6f894573488d4d334c8d4567488d556fe8????????4c8bc00fb6008845d7418b40048945db418b40088945df418b400c8945e3418b40108945e7488b8fa0010000488b01488d55d7ff5068488b87a0010000488b88a80000004885c97407488b01ff502090488d4dffe8????????4c8d9c2490000000498b5b30498b7338498be3415e5f5dc3",
        0,
    )],
);

const ATTACH: Signature = sig(
    "the widget attach",
    &[(
        "48895c2418488954241055565741544155415641574883ec404c8be24c8be94883c14041b91600000049b8ffffffffffffff7f0f1f4000660f1f840000000000498b1c24498b7548488b39483bfe7446488b0f4883c118488d531848837b30107204488b531848837918107203488b09e8????????84c075094883c708483bfe75ce498d4d4049b8ffffffffffffff7f41b916000000493b7d480f84????????498b3c244883c7184c8b7710498bc0492bc64883f8010f82????????4c8bff48837f181072034c8b3f48c744242000000000498d6e01bb0f000000488d742420483beb762a488bdd4883cb0f493bd87605498bd8eb084883fb16490f42d9488d4b01e8????????488bf0488944242048896c243048895c24384d8bc6498bd7488bcee8????????42c6043663c6042e00488d442420483bf8741f488d54242048837c243810480f435424204c8b442430488bcfe8????????90488b5424384883fa10498d4d4049b8ffffffffffffff7f41b9160000000f82????????48ffc2488b4c2420488bc14881fa0010000072154883c227488b49f8482bc14883c0f84883f81f770ee8????????498d4d40e9????????ff15????????cc498b042449c70424000000004889842480000000488b5108483b5110740a4889024883410808eb0d4c8d842480000000e8????????498b4548488b58f84c896b38488b03",
        0,
    )],
);

const BIND: Signature = sig(
    "the widget bind",
    &[(
        "40574883ec2048833d????????00488bf97459488b41484c8b4140488bd0",
        0,
    )],
);

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS_2026};

    /// Each build's game.dll sites, in [`BUILDS_2026`] order, as mapped by hand
    /// from Steam 2026-09-25: factory, image, clip, attach, bind.
    const GAME_EXPECTED: [[usize; 5]; 6] = [
        [0x2aef60, 0x2c5590, 0x2c5400, 0x2d5680, 0x2afa60],
        [0x2b4320, 0x2ca950, 0x2ca7c0, 0x2daa40, 0x2b4e20],
        [0x2aef60, 0x2c5590, 0x2c5400, 0x2d5680, 0x2afa60],
        [0x2b4320, 0x2ca950, 0x2ca7c0, 0x2daa40, 0x2b4e20],
        [0x2bd970, 0x2d3fa0, 0x2d3e10, 0x2e4230, 0x2be470],
        [0x2c2d00, 0x2d9330, 0x2d91a0, 0x2e95c0, 0x2c3800],
    ];

    #[test]
    fn the_game_sites_resolve_in_every_2026_build() {
        for (build, [factory, image, clip, attach, bind]) in BUILDS_2026.iter().zip(GAME_EXPECTED) {
            let Some(mapped) = reference(build, "game.dll") else {
                eprintln!("skipping {build}: no bin/{build}/game.dll");
                continue;
            };
            let expected = GameSites {
                factory,
                image,
                clip,
                attach,
                bind,
            };
            assert_eq!(find_game(&Image::mapped(&mapped)), Ok(expected), "{build}");
        }
    }

    /// Each build's sites, in [`BUILDS_2026`] order, as mapped by hand from
    /// Steam 2026-09-25.
    const EXPECTED: [(usize, usize, [usize; 6], [usize; 2]); 6] = [
        (
            0x39ce00,
            0x3bbad0,
            [0x380660, 0x380bc0, 0x381df0, 0x381bf0, 0x381f40, 0x382150],
            [0x3ccd40, 0x3cceb0],
        ),
        (
            0x39ce90,
            0x3bbb60,
            [0x3806f0, 0x380c50, 0x381e80, 0x381c80, 0x381fd0, 0x3821e0],
            [0x3ccdd0, 0x3ccf40],
        ),
        (
            0x39ce00,
            0x3bbca0,
            [0x380660, 0x380bc0, 0x381df0, 0x381bf0, 0x381f40, 0x382150],
            [0x3ccfe0, 0x3cd150],
        ),
        (
            0x39ce90,
            0x3bbd30,
            [0x3806f0, 0x380c50, 0x381e80, 0x381c80, 0x381fd0, 0x3821e0],
            [0x3cd070, 0x3cd1e0],
        ),
        (
            0x3a0830,
            0x3bf6d0,
            [0x384090, 0x3845f0, 0x385820, 0x385620, 0x385970, 0x385b80],
            [0x3d0a10, 0x3d0b80],
        ),
        (
            0x3a08c0,
            0x3bf760,
            [0x384120, 0x384680, 0x3858b0, 0x3856b0, 0x385a00, 0x385c10],
            [0x3d0aa0, 0x3d0c10],
        ),
    ];

    #[test]
    fn the_sites_resolve_in_every_2026_build() {
        for (build, (reader, lookup, adapters, resolvers)) in BUILDS_2026.iter().zip(EXPECTED) {
            let Some(mapped) = reference(build, "logic.dll") else {
                eprintln!("skipping {build}: no bin/{build}/logic.dll");
                continue;
            };
            let calls = core::array::from_fn(|i| adapters[i] + ADAPTERS[i].resolve_call);
            let expected = Sites {
                reader,
                lookup,
                adapters,
                calls,
                resolvers,
            };
            assert_eq!(find(&Image::mapped(&mapped)), Ok(expected), "{build}");
        }
    }

    #[test]
    fn a_changed_adapter_is_refused() {
        let Some(mapped) = reference(BUILDS_2026[0], "logic.dll") else {
            return;
        };
        let mut changed = mapped.image.clone();
        // The triples adapter's resolver call becomes a jump.
        changed[EXPECTED[0].2[1] + ADAPTERS[1].resolve_call] = 0xe9;
        let image = Image {
            image: &changed,
            base: mapped.base,
        };
        assert!(find(&image).is_err());
    }
}
