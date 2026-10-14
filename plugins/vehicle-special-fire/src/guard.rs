//! A `Gun`'s weapon record (`[gun+0x48]`) is null while the gun has no
//! weapon: `Gun` vf49 stores null there, and most guns run that way, with
//! vf56 falling back to its own range fields. A few branches of `Gun` vf10,
//! its fire helper, vf36 and vf56 read the record's turn rate and ranges
//! without checking, so a gun that reaches one of them without a record
//! (seen with rangers aboard a helicopter) crashes the game.
//!
//! Each guard sits on one of those reads ([`crate::sites::Guard`]) and, only
//! when the record is null, resumes at the branch's own exit instead of the
//! read; every other path through these methods is untouched. Flags are dead
//! at every site, so the `cmp` may clobber them.
use std::{ffi::c_void, sync::atomic::AtomicUsize};

macro_rules! guard {
    ($name:ident, $original:ident, $skip:ident, $gun:literal) => {
        pub(crate) static $original: AtomicUsize = AtomicUsize::new(0);
        pub(crate) static $skip: AtomicUsize = AtomicUsize::new(0);

        #[unsafe(naked)]
        pub(crate) unsafe extern "C" fn $name() {
            core::arch::naked_asm!(
                concat!("cmp qword ptr [", $gun, "+0x48], 0"),
                "je 2f",
                "jmp qword ptr [rip+{original}]",
                "2:",
                "jmp qword ptr [rip+{skip}]",
                original = sym $original,
                skip = sym $skip,
            );
        }
    };
}

guard!(aim_mode2, AIM_MODE2_ORIGINAL, AIM_MODE2_SKIP, "rbx");
guard!(aim_mode0, AIM_MODE0_ORIGINAL, AIM_MODE0_SKIP, "rbx");
guard!(fire_mode1, FIRE_MODE1_ORIGINAL, FIRE_MODE1_SKIP, "rbx");
guard!(fire, FIRE_ORIGINAL, FIRE_SKIP, "rbx");
guard!(vf36, VF36_ORIGINAL, VF36_SKIP, "rbx");
guard!(vf56, VF56_ORIGINAL, VF56_SKIP, "rdi");

/// Each guard's detour, trampoline slot and skip slot, in
/// [`crate::sites::Sites::gun_guards`] order.
pub(crate) fn detours() -> [(*mut c_void, &'static AtomicUsize, &'static AtomicUsize); 6] {
    [
        (
            aim_mode2 as *mut c_void,
            &AIM_MODE2_ORIGINAL,
            &AIM_MODE2_SKIP,
        ),
        (
            aim_mode0 as *mut c_void,
            &AIM_MODE0_ORIGINAL,
            &AIM_MODE0_SKIP,
        ),
        (
            fire_mode1 as *mut c_void,
            &FIRE_MODE1_ORIGINAL,
            &FIRE_MODE1_SKIP,
        ),
        (fire as *mut c_void, &FIRE_ORIGINAL, &FIRE_SKIP),
        (vf36 as *mut c_void, &VF36_ORIGINAL, &VF36_SKIP),
        (vf56 as *mut c_void, &VF56_ORIGINAL, &VF56_SKIP),
    ]
}
