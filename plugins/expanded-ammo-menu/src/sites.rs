//! Where the ammo menu's native code and class fields live, found by
//! signature instead of looked up by build hash.
//!
//! Every patched instruction is found by byte signature (`tools/sigs.py`'s
//! encoding: rel32 targets and RIP displacements wildcarded, struct offsets
//! kept) and must hold the bytes the patch replaces; those bytes are the same
//! on every supported build. The class offsets the combined view reads are
//! taken from game code that uses the same fields, and the roster offset from
//! the SquadAiFacet vtable in logic.dll. Each site must resolve to exactly
//! one place and both hooked entries must decode to the span the hook
//! displaces, so a build where any of it moved or changed shape resolves to an
//! error and the plugin writes nothing.
use crate::Offsets;
use defiance_core::{
    decode,
    sites::{sig, Image, Signature},
};

/// How a patched field's new value follows from the slot count.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    /// The slot count itself.
    Count,
    /// The slot array's size in bytes.
    Extent,
    /// A member after the slot array, moved by the added slots.
    Shift,
    /// Cleared.
    Zero,
}

/// One patched instruction: where it is, its bytes, and the little-endian
/// field in them the patch rewrites.
pub struct Site {
    pub signature: Signature,
    pub before: &'static [u8],
    pub field: usize,
    pub width: usize,
    pub kind: Kind,
}

/// The number of patched instructions.
pub const PATCH_COUNT: usize = 53;
/// The bytes the redraw hook displaces.
pub const REDRAW_SPAN: usize = 20;
/// The bytes the combined click hook displaces.
pub const CLICK_SPAN: usize = 15;
/// The bytes the combined hover hook displaces.
pub const HOVER_SPAN: usize = 14;

/// The resolved sites, as rvas, and the class offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sites {
    pub patches: [usize; PATCH_COUNT],
    /// The menu redraw, hooked.
    pub redraw: usize,
    /// The widget move the compacted grid calls.
    pub layout: usize,
    /// The functions the combined view calls; the first, the slot click, is
    /// hooked when it is enabled.
    pub combined: [usize; 8],
    /// The card hover, hooked when the combined view is enabled, then the
    /// functions it calls: slot of a card index, highlight, carrier list,
    /// the two range circles, the carrier markers, and the vector free.
    pub hover: [usize; 8],
    pub offsets: Offsets,
}

/// The patched instructions, in the menu's constructor, destructor, layout,
/// click and cleanup code, and the two allocations of the menu.
pub const SITES: [Site; PATCH_COUNT] = [
    // lea r8d, [r12 + 9]
    Site {
        signature: sig(
            "ammo menu site 0",
            &[("458d442409e8????????90488d9ef807000048895d704c8923", 0x0)],
        ),
        before: &[0x45, 0x8d, 0x44, 0x24, 0x09],
        field: 4,
        width: 1,
        kind: Kind::Count,
    },
    // lea rbx, [rsi + 0x7f8]
    Site {
        signature: sig(
            "ammo menu site 1",
            &[(
                "488d9ef807000048895d704c89234c896308418d4c2428e8????????",
                0x0,
            )],
        ),
        before: &[0x48, 0x8d, 0x9e, 0xf8, 0x07, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov qword ptr [rsi + 0x808], r12
    Site {
        signature: sig(
            "ammo menu site 2",
            &[(
                "4c89a6080800004c89a6100800004c89a6180800004c89a620080000",
                0x0,
            )],
        ),
        before: &[0x4c, 0x89, 0xa6, 0x08, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov qword ptr [rsi + 0x810], r12
    Site {
        signature: sig(
            "ammo menu site 3",
            &[(
                "4c89a6100800004c89a6180800004c89a6200800004c89a628080000",
                0x0,
            )],
        ),
        before: &[0x4c, 0x89, 0xa6, 0x10, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov qword ptr [rsi + 0x818], r12
    Site {
        signature: sig(
            "ammo menu site 4",
            &[(
                "4c89a6180800004c89a6200800004c89a6280800004c89a630080000",
                0x0,
            )],
        ),
        before: &[0x4c, 0x89, 0xa6, 0x18, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov qword ptr [rsi + 0x820], r12
    Site {
        signature: sig(
            "ammo menu site 5",
            &[(
                "4c89a6200800004c89a6280800004c89a630080000488b9620010000",
                0x0,
            )],
        ),
        before: &[0x4c, 0x89, 0xa6, 0x20, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov qword ptr [rsi + 0x828], r12
    Site {
        signature: sig(
            "ammo menu site 6",
            &[(
                "4c89a6200800004c89a6280800004c89a630080000488b9620010000",
                0x7,
            )],
        ),
        before: &[0x4c, 0x89, 0xa6, 0x28, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov qword ptr [rsi + 0x830], r12
    Site {
        signature: sig(
            "ammo menu site 7",
            &[
                (
                    "4c89a630080000488b9620010000488b820801000048898618010000",
                    0x0,
                ),
                (
                    "4c89a630080000488b9620010000488b822801000048898618010000",
                    0x0,
                ),
            ],
        ),
        before: &[0x4c, 0x89, 0xa6, 0x30, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov r13d, 2
    Site {
        signature: sig(
            "ammo menu site 8",
            &[("41bd02000000e9????????4533e4488b8e28010000488b01", 0x0)],
        ),
        before: &[0x41, 0xbd, 0x02, 0x00, 0x00, 0x00],
        field: 2,
        width: 4,
        kind: Kind::Zero,
    },
    // mov r13d, 3
    Site {
        signature: sig(
            "ammo menu site 9",
            &[(
                "41bd03000000eb78488b8e28010000488b01ff50504c8db0f8080000",
                0x0,
            )],
        ),
        before: &[0x41, 0xbd, 0x03, 0x00, 0x00, 0x00],
        field: 2,
        width: 4,
        kind: Kind::Zero,
    },
    // mov r13d, 1
    Site {
        signature: sig(
            "ammo menu site 10",
            &[(
                "41bd010000004533ff458bf7418bff4c8da6800100006666660f1f840000000000",
                0x0,
            )],
        ),
        before: &[0x41, 0xbd, 0x01, 0x00, 0x00, 0x00],
        field: 2,
        width: 4,
        kind: Kind::Zero,
    },
    // cmp rdi, 0x678
    Site {
        signature: sig(
            "ammo menu site 11",
            &[(
                "4881ff780600000f82????????488bcee8????????4d8dbc2478060000",
                0x0,
            )],
        ),
        before: &[0x48, 0x81, 0xff, 0x78, 0x06, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Extent,
    },
    // lea r15, [r12 + 0x678]
    Site {
        signature: sig(
            "ammo menu site 12",
            &[("4d8dbc24780600004d3be774484d8db42480000000498b1e", 0x0)],
        ),
        before: &[0x4d, 0x8d, 0xbc, 0x24, 0x78, 0x06, 0x00, 0x00],
        field: 4,
        width: 4,
        kind: Kind::Extent,
    },
    // mov edx, 0x838
    Site {
        signature: sig(
            "ammo menu site 13",
            &[("ba38080000488bcfe8????????488b5c2430488bc74883c420", 0x0)],
        ),
        before: &[0xba, 0x38, 0x08, 0x00, 0x00],
        field: 1,
        width: 4,
        kind: Kind::Shift,
    },
    // mov r8d, 9
    Site {
        signature: sig(
            "ammo menu site 14",
            &[("41b809000000e8????????904883c428c3cccccccccccccc", 0x0)],
        ),
        before: &[0x41, 0xb8, 0x09, 0x00, 0x00, 0x00],
        field: 2,
        width: 4,
        kind: Kind::Count,
    },
    // mov rdi, qword ptr [rcx + 0x828]
    Site {
        signature: sig(
            "ammo menu site 15",
            &[("488bb928080000488b9920080000483bdf7422660f1f440000", 0x0)],
        ),
        before: &[0x48, 0x8b, 0xb9, 0x28, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov rbx, qword ptr [rcx + 0x820]
    Site {
        signature: sig(
            "ammo menu site 16",
            &[("488b9920080000483bdf7422660f1f440000488b8e30010000", 0x0)],
        ),
        before: &[0x48, 0x8b, 0x99, 0x20, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov rdi, qword ptr [rsi + 0x810]
    Site {
        signature: sig(
            "ammo menu site 17",
            &[(
                "488bbe10080000488b9e08080000483bdf741d90488b8e30010000",
                0x0,
            )],
        ),
        before: &[0x48, 0x8b, 0xbe, 0x10, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov rbx, qword ptr [rsi + 0x808]
    Site {
        signature: sig(
            "ammo menu site 18",
            &[("488b9e08080000483bdf741d90488b8e30010000488b01488b13", 0x0)],
        ),
        before: &[0x48, 0x8b, 0x9e, 0x08, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // lea r10, [rbx + 0x678]
    Site {
        signature: sig(
            "ammo menu site 19",
            &[(
                "4c8d9378060000493bda74604c8d8b8000000066660f1f840000000000",
                0x0,
            )],
        ),
        before: &[0x4c, 0x8d, 0x93, 0x78, 0x06, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Extent,
    },
    // lea rcx, [rsi + 0x820]
    Site {
        signature: sig(
            "ammo menu site 20",
            &[("488d8e20080000e8????????488d8e08080000e8????????", 0x0)],
        ),
        before: &[0x48, 0x8d, 0x8e, 0x20, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // lea rcx, [rsi + 0x808]
    Site {
        signature: sig(
            "ammo menu site 21",
            &[("488d8e08080000e8????????488d8ef8070000e8????????", 0x0)],
        ),
        before: &[0x48, 0x8d, 0x8e, 0x08, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // lea rcx, [rsi + 0x7f8]
    Site {
        signature: sig(
            "ammo menu site 22",
            &[("488d8ef8070000e8????????904c8d0d????????bab8000000", 0x0)],
        ),
        before: &[0x48, 0x8d, 0x8e, 0xf8, 0x07, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov r8d, 9
    Site {
        signature: sig(
            "ammo menu site 23",
            &[("41b809000000488bcbe8????????90488d05????????488906", 0x0)],
        ),
        before: &[0x41, 0xb8, 0x09, 0x00, 0x00, 0x00],
        field: 2,
        width: 4,
        kind: Kind::Count,
    },
    // mov rsi, qword ptr [rdi + 0x7f8]
    Site {
        signature: sig(
            "ammo menu site 24",
            &[(
                "488bb7f80700004889af500100004c89742440488b5e08807b1900",
                0x0,
            )],
        ),
        before: &[0x48, 0x8b, 0xb7, 0xf8, 0x07, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // lea rdx, [rdi + 0x7f8]
    Site {
        signature: sig(
            "ammo menu site 25",
            &[(
                "488d97f8070000488d8ff8070000e8????????488bcbba28000000",
                0x0,
            )],
        ),
        before: &[0x48, 0x8d, 0x97, 0xf8, 0x07, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // lea rcx, [rdi + 0x7f8]
    Site {
        signature: sig(
            "ammo menu site 26",
            &[(
                "488d8ff8070000e8????????488bcbba28000000488b1be8????????",
                0x0,
            )],
        ),
        before: &[0x48, 0x8d, 0x8f, 0xf8, 0x07, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // lea rsi, [rbx + 0x678]
    Site {
        signature: sig(
            "ammo menu site 27",
            &[("488db3780600004889870008000089877c010000483bde7415", 0x0)],
        ),
        before: &[0x48, 0x8d, 0xb3, 0x78, 0x06, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Extent,
    },
    // mov qword ptr [rdi + 0x800], rax
    Site {
        signature: sig(
            "ammo menu site 28",
            &[(
                "4889870008000089877c010000483bde741590488bcbe8????????",
                0x0,
            )],
        ),
        before: &[0x48, 0x89, 0x87, 0x00, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // lea rax, [rcx + 0x7f8]
    Site {
        signature: sig(
            "ammo menu site 29",
            &[("488d81f807000048c78170010000000000004881c180010000", 0x0)],
        ),
        before: &[0x48, 0x8d, 0x81, 0xf8, 0x07, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // add rcx, 0x7f8
    Site {
        signature: sig(
            "ammo menu site 30",
            &[("4881c1f80700004c8b094d8bc1498b410880781900751739501c", 0x0)],
        ),
        before: &[0x48, 0x81, 0xc1, 0xf8, 0x07, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov r8, qword ptr [rdi + 0x7f8]
    Site {
        signature: sig(
            "ammo menu site 31",
            &[("4c8b87f80700008b942418010000498bc8498b400880781900", 0x0)],
        ),
        before: &[0x4c, 0x8b, 0x87, 0xf8, 0x07, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // lea rcx, [rdi + 0x7f8]
    Site {
        signature: sig(
            "ammo menu site 32",
            &[(
                "488d8ff8070000e8????????4863084869d9b8000000488b8c3bd0010000",
                0x0,
            )],
        ),
        before: &[0x48, 0x8d, 0x8f, 0xf8, 0x07, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // cmp ebx, 9
    Site {
        signature: sig(
            "ammo menu site 33",
            &[(
                "83fb090f8c????????440f285c2450440f28542460440f284c2470",
                0x0,
            )],
        ),
        before: &[0x83, 0xfb, 0x09],
        field: 2,
        width: 1,
        kind: Kind::Count,
    },
    // mov ebp, 9
    Site {
        signature: sig(
            "ammo menu site 34",
            &[("bd09000000488b7740488b5f38483bde74220f1f00488b0b", 0x0)],
        ),
        before: &[0xbd, 0x09, 0x00, 0x00, 0x00],
        field: 1,
        width: 4,
        kind: Kind::Count,
    },
    // lea rax, [rbp + 0x678]
    Site {
        signature: sig(
            "ammo menu site 35",
            &[("488d8578060000488bdd483be8742a48395318742d48395320", 0x0)],
        ),
        before: &[0x48, 0x8d, 0x85, 0x78, 0x06, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Extent,
    },
    // lea rax, [rcx + 0x7f8]
    Site {
        signature: sig(
            "ammo menu site 36",
            &[("488d81f80700004881c180010000483bc87434483951187429", 0x0)],
        ),
        before: &[0x48, 0x8d, 0x81, 0xf8, 0x07, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov rbp, qword ptr [rdi + 0x828]
    Site {
        signature: sig(
            "ammo menu site 37",
            &[("488baf28080000488b9f20080000483bdd741a488b03488b08", 0x0)],
        ),
        before: &[0x48, 0x8b, 0xaf, 0x28, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov rbx, qword ptr [rdi + 0x820]
    Site {
        signature: sig(
            "ammo menu site 38",
            &[(
                "488b9f20080000483bdd741a488b03488b08488b0133d2ff9080010000",
                0x0,
            )],
        ),
        before: &[0x48, 0x8b, 0x9f, 0x20, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov rbp, qword ptr [rdi + 0x810]
    Site {
        signature: sig(
            "ammo menu site 39",
            &[("488baf10080000488b9f08080000483bdd741c6690488b03", 0x0)],
        ),
        before: &[0x48, 0x8b, 0xaf, 0x10, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov rbx, qword ptr [rdi + 0x808]
    Site {
        signature: sig(
            "ammo menu site 40",
            &[("488b9f08080000483bdd741c6690488b03488b08488b0133d2", 0x0)],
        ),
        before: &[0x48, 0x8b, 0x9f, 0x08, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov r14, qword ptr [rsi + 0x828]
    Site {
        signature: sig(
            "ammo menu site 41",
            &[("4c8bb628080000488b9e20080000493bde7466488b3b48897d28", 0x0)],
        ),
        before: &[0x4c, 0x8b, 0xb6, 0x28, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov rbx, qword ptr [rsi + 0x820]
    Site {
        signature: sig(
            "ammo menu site 42",
            &[("488b9e20080000493bde7466488b3b48897d28488b0f488b01", 0x0)],
        ),
        before: &[0x48, 0x8b, 0x9e, 0x20, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov r14, qword ptr [rsi + 0x810]
    Site {
        signature: sig(
            "ammo menu site 43",
            &[("4c8bb610080000488b9e08080000493bde74490f1f4000488b3b", 0x0)],
        ),
        before: &[0x4c, 0x8b, 0xb6, 0x10, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov rbx, qword ptr [rsi + 0x808]
    Site {
        signature: sig(
            "ammo menu site 44",
            &[("488b9e08080000493bde74490f1f4000488b3b48897d28488b0f", 0x0)],
        ),
        before: &[0x48, 0x8b, 0x9e, 0x08, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // lea r14, [rcx + 0x820]
    Site {
        signature: sig(
            "ammo menu site 45",
            &[("4c8db120080000498b1e488b3a4c8b7a08493bff0f84????????", 0x0)],
        ),
        before: &[0x4c, 0x8d, 0xb1, 0x20, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // cmp rbx, qword ptr [rsi + 0x828]
    Site {
        signature: sig(
            "ammo menu site 46",
            &[("483b9e28080000744b488b03488b08488b01b201ff9080010000", 0x0)],
        ),
        before: &[0x48, 0x3b, 0x9e, 0x28, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov rbx, qword ptr [rsi + 0x828]
    Site {
        signature: sig(
            "ammo menu site 47",
            &[(
                "488b9e280800004883c708493bff65488b142558000000b920000000",
                0x0,
            )],
        ),
        before: &[0x48, 0x8b, 0x9e, 0x28, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // lea r14, [rcx + 0x808]
    Site {
        signature: sig(
            "ammo menu site 48",
            &[("4c8db108080000498b1e488b3a4c8b7a08493bff0f84????????", 0x0)],
        ),
        before: &[0x4c, 0x8d, 0xb1, 0x08, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // cmp rbx, qword ptr [rsi + 0x810]
    Site {
        signature: sig(
            "ammo menu site 49",
            &[("483b9e10080000744b488b03488b08488b01b201ff9080010000", 0x0)],
        ),
        before: &[0x48, 0x3b, 0x9e, 0x10, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov rbx, qword ptr [rsi + 0x810]
    Site {
        signature: sig(
            "ammo menu site 50",
            &[(
                "488b9e100800004883c708493bff65488b142558000000b920000000",
                0x0,
            )],
        ),
        before: &[0x48, 0x8b, 0x9e, 0x10, 0x08, 0x00, 0x00],
        field: 3,
        width: 4,
        kind: Kind::Shift,
    },
    // mov ecx, 0x838
    Site {
        signature: sig(
            "ammo menu site 51",
            &[("b938080000e8????????48898510010000488b8e40010000", 0x0)],
        ),
        before: &[0xb9, 0x38, 0x08, 0x00, 0x00],
        field: 1,
        width: 4,
        kind: Kind::Shift,
    },
    // mov edx, 0x838
    Site {
        signature: sig(
            "ammo menu site 52",
            &[("ba38080000488b8d10020000e8????????4883c4205dc34055", 0x0)],
        ),
        before: &[0xba, 0x38, 0x08, 0x00, 0x00],
        field: 1,
        width: 4,
        kind: Kind::Shift,
    },
];

const REDRAW: Signature = sig(
    "ammo menu redraw",
    &[("488954241048894c24085741544881ece8000000488b02488bf9", 0x0)],
);
pub const REDRAW_BEFORE: &[u8] = &[
    0x48, 0x89, 0x54, 0x24, 0x10, 0x48, 0x89, 0x4c, 0x24, 0x08, 0x57, 0x41, 0x54, 0x48, 0x81, 0xec,
    0xe8, 0x00, 0x00, 0x00,
];
const LAYOUT: Signature = sig(
    "widget move",
    &[("4889742410574883ec40833a00488bf2488bf9750a837a0400", 0x0)],
);
const LAYOUT_BEFORE: &[u8] = &[
    0x48, 0x89, 0x74, 0x24, 0x10, 0x57, 0x48, 0x83, 0xec, 0x40, 0x83, 0x3a, 0x00, 0x48, 0x8b, 0xf2,
];
pub const COMBINED: [(Signature, &[u8]); 8] = [
    (sig("combined function 0", &[("48895c241048896c2418488974242057415641574883ec40488bf9", 0x0)]), &[0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x6c, 0x24, 0x18, 0x48, 0x89, 0x74, 0x24, 0x20]),
    (sig("combined function 1", &[("48895c241048897424185557415441564157488dac2460ffffff", 0x0)]), &[0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x55, 0x57, 0x41, 0x54, 0x41, 0x56]),
    (sig("combined function 2", &[("48895c24084889742410574883ec204869f2b80000004803f1", 0x0)]), &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x74, 0x24, 0x10, 0x57, 0x48, 0x83, 0xec, 0x20]),
    (sig("combined function 3", &[("48895c2408574883ec20488bb988000000488b9980000000483bdf74220f1f00488b0b4885c9740e80795b007508", 0x0)]), &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x57, 0x48, 0x83, 0xec, 0x20, 0x48, 0x8b, 0xb9, 0x88, 0x00]),
    (sig("combined function 4", &[("48895c241048896c2418488974242057415641574883ec40488bf133c9384e58", 0x0)]), &[0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x6c, 0x24, 0x18, 0x48, 0x89, 0x74, 0x24, 0x20]),
    (sig("combined function 5", &[("48895c2410488974241848897c2420554154415541564157488bec4883ec604c8be9", 0x0)]), &[0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x48, 0x89, 0x7c, 0x24, 0x20]),
    (sig("combined function 6", &[("48895c240848896c24104889742418574883ec2048837a1810", 0x0)]), &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x6c, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x57]),
    (sig("combined function 7", &[("48895c2418488974242055574156488d6c24b94881ec90000000488bf9", 0x0)]), &[0x48, 0x89, 0x5c, 0x24, 0x18, 0x48, 0x89, 0x74, 0x24, 0x20, 0x55, 0x57, 0x41, 0x56, 0x48, 0x8d]),
];

/// The card hover: the whole body up to the carrier vector's free, with the
/// pool getter's disp32, the rel32 calls and the free path's `ja` wildcarded.
const HOVER: Signature = sig(
    "ammo menu hover",
    &[("48895c2408488974241855574156488bec4883ec700f297424600f297c2450488bda488bf1488b02488bcaff90b0000000488b48284885c90f841f010000488b01ff90????????488b10488bc8ff52484863967801000085d20f88fe0000004c8b00488b4008492bc048c1f80348b9398ee3388ee3388e480fafc1483bc20f86d9000000488d04d2498b3cc0488bcee8????????488d968001000048984869c8b80000004803cae8????????4c8bcf4c8bc3488d55c8488bcee8????????90f30f10bfb4010000f30f10b7b0010000488b8e28010000488b01ff5050f30f1080a80200000f2fc6771e0f28d7488d55c8488bcee8????????0f28d6488d55c8488bcee8????????4c8bc7488d55c8488bcee8????????90488b4dc84885c9743d488b45d8482bc148c1f803488d14c500000000488bc14881fa0010000072194883c227488b49f8482bc14883c0f84883f81f0f87????????e8????????", 0x0)],
);
pub const HOVER_BEFORE: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x74, 0x24, 0x18, 0x55, 0x57, 0x41, 0x56,
];
/// The hover's calls, as offsets from its entry, in [`Sites::hover`] order.
const HOVER_CALLS: [usize; 7] = [0x8f, 0xa7, 0xb9, 0xf3, 0x102, 0x111, 0x158];

/// The selection-manager update, which loads the world's player lookup.
const SELECTION_MANAGER: Signature = sig(
    "selection manager",
    &[("40564883ec20488bf1488b49184885c90f84????????488b4910", 0x0)],
);

const SQUAD_AI_CLASS: &str = ".?AVSquadAiFacet@Leonardo@@";
/// The roster-holder getter: `mov rax, [rcx+0x1c8]; test rax, rax; jz ...;
/// mov rax, [rax+0x10]`. Exactly one SquadAiFacet slot holds it.
const ROSTER_GETTER: &[u8] = b"\x48\x8b\x81\xc8\x01\x00\x00\x48\x85\xc0\x74";
const ROSTER_GETTER_TAIL: &[u8] = b"\x48\x8b\x40\x10";

/// The resolved sites, or why this build is not supported.
pub fn sites(game: &Image, logic: &Image) -> Result<Sites, String> {
    let mut patches = [0; PATCH_COUNT];
    for (rva, site) in patches.iter_mut().zip(&SITES) {
        *rva = game.find(&site.signature)?;
        game.expect(site.signature.name, *rva, site.before)?;
    }
    let redraw = game.find(&REDRAW)?;
    game.expect(REDRAW.name, redraw, REDRAW_BEFORE)?;
    entry(game, REDRAW.name, redraw, REDRAW_SPAN)?;
    let layout = game.find(&LAYOUT)?;
    game.expect(LAYOUT.name, layout, LAYOUT_BEFORE)?;
    let mut combined = [0; 8];
    for (rva, (signature, before)) in combined.iter_mut().zip(&COMBINED) {
        *rva = game.find(signature)?;
        game.expect(signature.name, *rva, before)?;
    }
    entry(game, COMBINED[0].0.name, combined[0], CLICK_SPAN)?;
    let hover_entry = game.find(&HOVER)?;
    game.expect(HOVER.name, hover_entry, HOVER_BEFORE)?;
    entry(game, HOVER.name, hover_entry, HOVER_SPAN)?;
    let mut hover = [hover_entry; 8];
    for (target, offset) in hover[1..].iter_mut().zip(HOVER_CALLS) {
        *target = game
            .branch_target(hover_entry + offset)
            .ok_or_else(|| format!("{} call at +{offset:#x} does not decode", HOVER.name))?;
    }
    // Each field is the disp32 of an instruction at a fixed place in code
    // that uses it: `call [rax+disp32]` or `mov r64, [reg+disp32]`.
    let field = |what: &str, rva: usize, opcode: &[u8]| -> Result<usize, String> {
        game.expect(what, rva, opcode)?;
        game.u32(rva + opcode.len())
            .map(|disp| disp as usize)
            .ok_or_else(|| format!("{what} runs off the image"))
    };
    let selection = game.find(&SELECTION_MANAGER)?;
    let offsets = Offsets {
        roster: roster(logic)?,
        // The redraw fills each slot from the squad's ammo pool and gunners.
        pool_get: field("ammo pool getter", redraw + 0x91, b"\xff\x90")?,
        gunner_count: field("gunner count", redraw + 0x14b, b"\xff\x90")?,
        gunner_get: field("gunner getter", redraw + 0x169, b"\x4c\x8b\x81")?,
        world_player: field("world player lookup", selection + 0x5a, b"\x48\x8b\x99")?,
        // The slot click sets the chosen ammo type on the AI.
        ai_set: field("ai ammo setter", combined[0] + 0xd0, b"\xff\x90")?,
        // The constructor reads the owner's tooltip controller.
        tooltip: field("tooltip controller", patches[0] + 0x428, b"\x48\x8b\x92")?,
    };
    // The hover reads the same pool the redraw fills the cards from.
    if field("hover pool getter", hover[0] + 0x41, b"\xff\x90")? != offsets.pool_get {
        return Err(format!("{} reads a different ammo pool getter", HOVER.name));
    }
    Ok(Sites {
        patches,
        redraw,
        layout,
        combined,
        hover,
        offsets,
    })
}

/// The SquadAiFacet's roster-holder getter's vtable offset.
fn roster(logic: &Image) -> Result<usize, String> {
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
    roster.ok_or_else(|| "no SquadAiFacet slot reads the roster".into())
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

    /// The rvas and offsets the per-build hash table held before the plugin
    /// resolved them, in [`BUILDS`] order.
    const TABLE: [(&str, Sites); 6] = [
        (
            "gog/2025-12-23",
            Sites {
                patches: [
                    0x3debc, 0x3dec7, 0x3def7, 0x3defe, 0x3df05, 0x3df0c, 0x3df13, 0x3df1a,
                    0x3e0b5, 0x3e135, 0x3e1af, 0x3e3e6, 0x3e3fb, 0x3e4a9, 0x3e4e0, 0x3e557,
                    0x3e55e, 0x3e58c, 0x3e593, 0x3e5c3, 0x3e647, 0x3e653, 0x3e65f, 0x3e678,
                    0x3e712, 0x3e734, 0x3e73b, 0x3e776, 0x3e77d, 0x3e830, 0x3ec8c, 0x3ef9c,
                    0x3eff2, 0x3f0ba, 0x3f14b, 0x3f8c5, 0x3fa70, 0x3fd7e, 0x3fd85, 0x3fdab,
                    0x3fdb2, 0x4008d, 0x40094, 0x40119, 0x40120, 0x406d3, 0x40705, 0x40801,
                    0x408c3, 0x408f5, 0x409f1, 0x34d2c6, 0x4b6af9,
                ],
                redraw: 0x3ed10,
                layout: 0x2d1920,
                combined: [
                    0x3f8a0, 0x3f1d0, 0x3f800, 0x3b540, 0x3b9f0, 0x3bba0, 0x2cb730, 0x2c3380,
                ],
                hover: [
                    0x3ff20, 0x3ec80, 0x3be20, 0x40200, 0x406a0, 0x40890, 0x40460, 0x499474,
                ],
                offsets: Offsets {
                    roster: 0x3b8,
                    gunner_count: 0x130,
                    gunner_get: 0x120,
                    pool_get: 0x1b8,
                    world_player: 0x700,
                    ai_set: 0x3e0,
                    tooltip: 0x238,
                },
            },
        ),
        (
            "steam/2025-12-23",
            Sites {
                patches: [
                    0x3debc, 0x3dec7, 0x3def7, 0x3defe, 0x3df05, 0x3df0c, 0x3df13, 0x3df1a,
                    0x3e0b5, 0x3e135, 0x3e1af, 0x3e3e6, 0x3e3fb, 0x3e4a9, 0x3e4e0, 0x3e557,
                    0x3e55e, 0x3e58c, 0x3e593, 0x3e5c3, 0x3e647, 0x3e653, 0x3e65f, 0x3e678,
                    0x3e712, 0x3e734, 0x3e73b, 0x3e776, 0x3e77d, 0x3e830, 0x3ec8c, 0x3ef9c,
                    0x3eff2, 0x3f0ba, 0x3f14b, 0x3f8c5, 0x3fa70, 0x3fd7e, 0x3fd85, 0x3fdab,
                    0x3fdb2, 0x4008d, 0x40094, 0x40119, 0x40120, 0x406d3, 0x40705, 0x40801,
                    0x408c3, 0x408f5, 0x409f1, 0x353776, 0x4bd3d9,
                ],
                redraw: 0x3ed10,
                layout: 0x2d6cb0,
                combined: [
                    0x3f8a0, 0x3f1d0, 0x3f800, 0x3b540, 0x3b9f0, 0x3bba0, 0x2d0ac0, 0x2c8710,
                ],
                hover: [
                    0x3ff20, 0x3ec80, 0x3be20, 0x40200, 0x406a0, 0x40890, 0x40460, 0x49f924,
                ],
                offsets: Offsets {
                    roster: 0x3b8,
                    gunner_count: 0x130,
                    gunner_get: 0x120,
                    pool_get: 0x1b8,
                    world_player: 0x700,
                    ai_set: 0x3e0,
                    tooltip: 0x258,
                },
            },
        ),
        (
            "gog/2026-09-14",
            Sites {
                patches: [
                    0x3e05c, 0x3e067, 0x3e097, 0x3e09e, 0x3e0a5, 0x3e0ac, 0x3e0b3, 0x3e0ba,
                    0x3e255, 0x3e2d5, 0x3e34f, 0x3e586, 0x3e59b, 0x3e649, 0x3e680, 0x3e6f7,
                    0x3e6fe, 0x3e72c, 0x3e733, 0x3e763, 0x3e7e7, 0x3e7f3, 0x3e7ff, 0x3e818,
                    0x3e8b2, 0x3e8d4, 0x3e8db, 0x3e916, 0x3e91d, 0x3e9d0, 0x3ee2c, 0x3f13c,
                    0x3f192, 0x3f25a, 0x3f2eb, 0x3fa65, 0x3fc10, 0x3ff1e, 0x3ff25, 0x3ff4b,
                    0x3ff52, 0x4022d, 0x40234, 0x402b9, 0x402c0, 0x40873, 0x408a5, 0x409a1,
                    0x40a63, 0x40a95, 0x40b91, 0x34f4a6, 0x4b8ee9,
                ],
                redraw: 0x3eeb0,
                layout: 0x2d3ab0,
                combined: [
                    0x3fa40, 0x3f370, 0x3f9a0, 0x3b6e0, 0x3bb90, 0x3bd40, 0x2cd8c0, 0x2c5400,
                ],
                hover: [
                    0x400c0, 0x3ee20, 0x3bfc0, 0x403a0, 0x40840, 0x40a30, 0x40600, 0x49b884,
                ],
                offsets: Offsets {
                    roster: 0x3d0,
                    gunner_count: 0x140,
                    gunner_get: 0x130,
                    pool_get: 0x1c8,
                    world_player: 0x708,
                    ai_set: 0x3f8,
                    tooltip: 0x238,
                },
            },
        ),
        (
            "steam/2026-09-22",
            Sites {
                patches: [
                    0x3e05c, 0x3e067, 0x3e097, 0x3e09e, 0x3e0a5, 0x3e0ac, 0x3e0b3, 0x3e0ba,
                    0x3e255, 0x3e2d5, 0x3e34f, 0x3e586, 0x3e59b, 0x3e649, 0x3e680, 0x3e6f7,
                    0x3e6fe, 0x3e72c, 0x3e733, 0x3e763, 0x3e7e7, 0x3e7f3, 0x3e7ff, 0x3e818,
                    0x3e8b2, 0x3e8d4, 0x3e8db, 0x3e916, 0x3e91d, 0x3e9d0, 0x3ee2c, 0x3f13c,
                    0x3f192, 0x3f25a, 0x3f2eb, 0x3fa65, 0x3fc10, 0x3ff1e, 0x3ff25, 0x3ff4b,
                    0x3ff52, 0x4022d, 0x40234, 0x402b9, 0x402c0, 0x40873, 0x408a5, 0x409a1,
                    0x40a63, 0x40a95, 0x40b91, 0x355986, 0x4bf889,
                ],
                redraw: 0x3eeb0,
                layout: 0x2d8e70,
                combined: [
                    0x3fa40, 0x3f370, 0x3f9a0, 0x3b6e0, 0x3bb90, 0x3bd40, 0x2d2c80, 0x2ca7c0,
                ],
                hover: [
                    0x400c0, 0x3ee20, 0x3bfc0, 0x403a0, 0x40840, 0x40a30, 0x40600, 0x4a1df4,
                ],
                offsets: Offsets {
                    roster: 0x3d0,
                    gunner_count: 0x140,
                    gunner_get: 0x130,
                    pool_get: 0x1c8,
                    world_player: 0x708,
                    ai_set: 0x3f8,
                    tooltip: 0x258,
                },
            },
        ),
        (
            "gog/2026-09-25",
            Sites {
                patches: [
                    0x3e05c, 0x3e067, 0x3e097, 0x3e09e, 0x3e0a5, 0x3e0ac, 0x3e0b3, 0x3e0ba,
                    0x3e255, 0x3e2d5, 0x3e34f, 0x3e586, 0x3e59b, 0x3e649, 0x3e680, 0x3e6f7,
                    0x3e6fe, 0x3e72c, 0x3e733, 0x3e763, 0x3e7e7, 0x3e7f3, 0x3e7ff, 0x3e818,
                    0x3e8b2, 0x3e8d4, 0x3e8db, 0x3e916, 0x3e91d, 0x3e9d0, 0x3ee2c, 0x3f13c,
                    0x3f192, 0x3f25a, 0x3f2eb, 0x3fa65, 0x3fc10, 0x3ff1e, 0x3ff25, 0x3ff4b,
                    0x3ff52, 0x4022d, 0x40234, 0x402b9, 0x402c0, 0x40873, 0x408a5, 0x409a1,
                    0x40a63, 0x40a95, 0x40b91, 0x34f4a6, 0x4b8ef9,
                ],
                redraw: 0x3eeb0,
                layout: 0x2d3ab0,
                combined: [
                    0x3fa40, 0x3f370, 0x3f9a0, 0x3b6e0, 0x3bb90, 0x3bd40, 0x2cd8c0, 0x2c5400,
                ],
                hover: [
                    0x400c0, 0x3ee20, 0x3bfc0, 0x403a0, 0x40840, 0x40a30, 0x40600, 0x49b894,
                ],
                offsets: Offsets {
                    roster: 0x3d0,
                    gunner_count: 0x140,
                    gunner_get: 0x130,
                    pool_get: 0x1c8,
                    world_player: 0x708,
                    ai_set: 0x3f8,
                    tooltip: 0x238,
                },
            },
        ),
        (
            "steam/2026-09-25",
            Sites {
                patches: [
                    0x3e05c, 0x3e067, 0x3e097, 0x3e09e, 0x3e0a5, 0x3e0ac, 0x3e0b3, 0x3e0ba,
                    0x3e255, 0x3e2d5, 0x3e34f, 0x3e586, 0x3e59b, 0x3e649, 0x3e680, 0x3e6f7,
                    0x3e6fe, 0x3e72c, 0x3e733, 0x3e763, 0x3e7e7, 0x3e7f3, 0x3e7ff, 0x3e818,
                    0x3e8b2, 0x3e8d4, 0x3e8db, 0x3e916, 0x3e91d, 0x3e9d0, 0x3ee2c, 0x3f13c,
                    0x3f192, 0x3f25a, 0x3f2eb, 0x3fa65, 0x3fc10, 0x3ff1e, 0x3ff25, 0x3ff4b,
                    0x3ff52, 0x4022d, 0x40234, 0x402b9, 0x402c0, 0x40873, 0x408a5, 0x409a1,
                    0x40a63, 0x40a95, 0x40b91, 0x355986, 0x4bf899,
                ],
                redraw: 0x3eeb0,
                layout: 0x2d8e70,
                combined: [
                    0x3fa40, 0x3f370, 0x3f9a0, 0x3b6e0, 0x3bb90, 0x3bd40, 0x2d2c80, 0x2ca7c0,
                ],
                hover: [
                    0x400c0, 0x3ee20, 0x3bfc0, 0x403a0, 0x40840, 0x40a30, 0x40600, 0x4a1e04,
                ],
                offsets: Offsets {
                    roster: 0x3d0,
                    gunner_count: 0x140,
                    gunner_get: 0x130,
                    pool_get: 0x1c8,
                    world_player: 0x708,
                    ai_set: 0x3f8,
                    tooltip: 0x258,
                },
            },
        ),
    ];

    #[test]
    fn every_build_resolves_to_the_old_table() {
        assert_eq!(TABLE.map(|(build, _)| build), BUILDS);
        for (build, expected) in TABLE {
            let (Some(game), Some(logic)) =
                (reference(build, "game.dll"), reference(build, "logic.dll"))
            else {
                continue;
            };
            let resolved = sites(&Image::mapped(&game), &Image::mapped(&logic))
                .unwrap_or_else(|e| panic!("{build}: {e}"));
            assert_eq!(resolved, expected, "{build}");
        }
    }

    #[test]
    fn a_changed_entry_is_refused() {
        let (Some(game), Some(logic)) = (
            reference(BUILDS[0], "game.dll"),
            reference(BUILDS[0], "logic.dll"),
        ) else {
            return;
        };
        let image = Image::mapped(&game);
        let logic = Image::mapped(&logic);
        let resolved = sites(&image, &logic).unwrap();
        for rva in [
            resolved.redraw,
            resolved.combined[0],
            resolved.hover[0],
            resolved.patches[0],
        ] {
            let mut changed = image.image.to_vec();
            changed[rva + 4] ^= 1;
            let changed = Image {
                image: &changed,
                base: image.base,
            };
            assert!(sites(&changed, &logic).is_err(), "{rva:#x}");
        }
    }
}
