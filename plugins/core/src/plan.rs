//! Native writes as data (Core's and the performance plugin's): resolved first, then staged, then bound.
//!
//! An installer resolves a [`Plan`] from its setting and the module's original
//! view, with no side effects. [`apply`] stages the plan through the loader
//! and only then stores the returned trampolines in their slots; [`contract`]
//! turns the same plan into the expected-write contract. Inspecting a plan
//! therefore cannot install a hook or overwrite a live trampoline, and the
//! contract states what the installer intends rather than what a replay of it
//! submitted.
use core::ffi::c_void;
use defiance_api::{Api, PATCH_KIND_BYTES, PATCH_KIND_CALL, PATCH_KIND_ENTRY};
use defiance_feature_sdk::contract::Patch;
use std::sync::atomic::{AtomicUsize, Ordering};

/// What one write does at its target.
pub(crate) enum Kind {
    /// A function-entry hook; `Some(n)` displaces exactly n bytes, `None`
    /// lets the loader decode the shortest copy of at least 5.
    Entry(Option<usize>),
    /// A redirect of the `e8 rel32` call at the target.
    Call,
    /// A plain byte patch from `before` to `after`.
    Bytes { before: Vec<u8>, after: Vec<u8> },
}

/// One intended write and where its continuation goes.
pub(crate) struct Write {
    pub target: usize,
    pub kind: Kind,
    /// The detour; unused for [`Kind::Bytes`].
    pub detour: *mut c_void,
    /// Receives the trampoline (or the original callee for [`Kind::Call`])
    /// once the whole plan has staged.
    pub original: Option<&'static AtomicUsize>,
}

/// An installer's writes, in staging order. Empty when it is off.
#[derive(Default)]
pub(crate) struct Plan {
    pub writes: Vec<Write>,
}

/// The modules Core writes to, in the order the contract names them.
const MODULES: [&core::ffi::CStr; 4] = [c"logic.dll", c"game.dll", c"world2.dll", c"galileo.dll"];

fn location(api: &Api, target: usize) -> Option<(&'static core::ffi::CStr, usize, usize)> {
    MODULES.into_iter().find_map(|module| {
        let base = unsafe { (api.module_base)(module.as_ptr()) } as usize;
        if base == 0 {
            return None;
        }
        let size = unsafe { (api.module_size)(base as *mut c_void) };
        (base..base.checked_add(size)?)
            .contains(&target)
            .then_some((module, base, size))
    })
}

/// Bytes at `start` as they were before any plugin wrote to them; the live
/// bytes from a loader that predates the original view.
pub(crate) fn original(start: usize, length: usize) -> Option<Vec<u8>> {
    let mut bytes = vec![0; length];
    match unsafe { defiance_feature_sdk::services::original() } {
        Some(view) => {
            (unsafe { (view.read)(start, bytes.as_mut_ptr(), length) } == 0).then_some(bytes)
        }
        None => Some(unsafe { core::slice::from_raw_parts(start as *const u8, length) }.to_vec()),
    }
}

/// The loaded module at `base` as it was before any plugin wrote to it: the
/// live image with each executable section, where loader-owned writes land,
/// re-read through the original view. Signatures that cover a hooked entry
/// still match it, so a site resolves the same before and after its hook.
pub(crate) fn original_image(base: *const u8, size: usize) -> Option<Vec<u8>> {
    let mut image = unsafe { core::slice::from_raw_parts(base, size) }.to_vec();
    let Some(view) = (unsafe { defiance_feature_sdk::services::original() }) else {
        return Some(image);
    };
    let at = |o: usize, n: usize| -> Option<usize> {
        let bytes = image.get(o..o + n)?;
        Some(bytes.iter().rev().fold(0, |v, &b| v << 8 | b as usize))
    };
    let pe = at(0x3c, 4)?;
    let table = pe + 24 + at(pe + 20, 2)?;
    let sections: Vec<(usize, usize)> = (0..at(pe + 6, 2)?)
        .map(|i| table + i * 40)
        .filter(|&s| at(s + 36, 4).is_some_and(|c| c & 0x2000_0000 != 0))
        .map(|s| Some((at(s + 12, 4)?, at(s + 8, 4)?)))
        .collect::<Option<_>>()?;
    for (va, len) in sections {
        let len = len.min(size.checked_sub(va)?);
        let out = image[va..va + len].as_mut_ptr();
        if unsafe { (view.read)(base as usize + va, out, len) } != 0 {
            return None;
        }
    }
    Some(image)
}

/// The target of the original `e8 rel32` at `site`.
pub(crate) fn call_target(site: usize) -> Option<usize> {
    let bytes = original(site, 5)?;
    if bytes[0] != 0xe8 {
        return None;
    }
    let rel = i32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
    Some((site as isize + 5 + rel as isize) as usize)
}

fn entry(api: &Api, write: &Write) -> Result<Patch, String> {
    let at = write.target;
    let (module, base, size) =
        location(api, at).ok_or_else(|| format!("{at:#x} is outside Core's modules"))?;
    let rva = at - base;
    let (kind, before, after) = match &write.kind {
        Kind::Entry(displaced) => {
            let window = original(at, (size - rva).min(16))
                .ok_or_else(|| format!("{module:?}+{rva:#x} is unreadable"))?;
            let len = match displaced {
                Some(len) => *len,
                None => defiance_core::decode::displaced(&window, 5)
                    .map_err(|e| format!("{module:?}+{rva:#x}: {e}"))?,
            };
            let before = window
                .get(..len)
                .ok_or_else(|| format!("{module:?}+{rva:#x}: {len} bytes exceed the window"))?
                .to_vec();
            defiance_core::decode::validate_copy(&before)
                .map_err(|e| format!("{module:?}+{rva:#x}: {e}"))?;
            (PATCH_KIND_ENTRY, before, None)
        }
        Kind::Call => {
            let before =
                original(at, 5).ok_or_else(|| format!("{module:?}+{rva:#x} is unreadable"))?;
            if before[0] != 0xe8 {
                return Err(format!("{module:?}+{rva:#x} is not a call"));
            }
            (PATCH_KIND_CALL, before, None)
        }
        Kind::Bytes { before, after } => {
            if before.is_empty() || before.len() != after.len() {
                return Err(format!("{module:?}+{rva:#x}: unequal byte spans"));
            }
            if original(at, before.len()).as_deref() != Some(before.as_slice()) {
                return Err(format!(
                    "{module:?}+{rva:#x} does not hold the expected bytes"
                ));
            }
            (PATCH_KIND_BYTES, before.clone(), Some(after.clone()))
        }
    };
    if rva + before.len() > size {
        return Err(format!("{module:?}+{rva:#x} runs past the module"));
    }
    Ok(Patch {
        module,
        rva,
        kind,
        before,
        after,
    })
}

/// The plan's contract entries, read from the original view: the same before
/// and after initialization.
pub(crate) fn contract(api: &Api, plan: &Plan) -> Result<Vec<Patch>, String> {
    plan.writes.iter().map(|write| entry(api, write)).collect()
}

/// Stage every write, then bind the continuations. A refused write withdraws
/// the ones already staged and binds nothing.
pub(crate) fn apply(api: &Api, plan: &Plan) -> Result<(), String> {
    let mut continuations = Vec::with_capacity(plan.writes.len());
    for (i, write) in plan.writes.iter().enumerate() {
        let target = write.target as *mut c_void;
        let mut original = core::ptr::null_mut();
        let result = unsafe {
            match &write.kind {
                Kind::Entry(None) => (api.hook)(target, write.detour, &mut original),
                Kind::Entry(Some(len)) => {
                    (api.hook_exact)(target, write.detour, *len, &mut original)
                }
                Kind::Call => (api.hook_call)(target, write.detour, &mut original),
                Kind::Bytes { before, after } => {
                    (api.patch_bytes)(target, before.as_ptr(), after.as_ptr(), before.len())
                }
            }
        };
        let bound = matches!(write.kind, Kind::Bytes { .. }) || !original.is_null();
        if result != 0 || !bound {
            if result == 0 {
                unsafe { (api.unhook)(target) };
            }
            for prior in plan.writes[..i].iter().rev() {
                unsafe { (api.unhook)(prior.target as *mut c_void) };
            }
            return Err(format!("the write at {:#x} was refused", write.target));
        }
        continuations.push(original as usize);
    }
    for (write, original) in plan.writes.iter().zip(continuations) {
        if let Some(slot) = write.original {
            slot.store(original, Ordering::Release);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::c_char;
    use std::sync::atomic::AtomicBool;

    /// A stand-in game.dll: an entry at 0, a call at 16, bytes at 32.
    static mut MODULE: [u8; 48] = {
        let mut image = [0xcc; 48];
        let entry = [0x55, 0x48, 0x89, 0xe5, 0x90];
        let mut i = 0;
        while i < 5 {
            image[i] = entry[i];
            i += 1;
        }
        image[16] = 0xe8;
        image[17] = 0x10;
        image[18] = 0;
        image[19] = 0;
        image[20] = 0;
        image[32] = 0x90;
        image[33] = 0x90;
        image
    };
    static WRITES: AtomicUsize = AtomicUsize::new(0);
    static UNHOOKS: AtomicUsize = AtomicUsize::new(0);
    static REFUSE_CALL: AtomicBool = AtomicBool::new(false);
    static ENTRY_SLOT: AtomicUsize = AtomicUsize::new(0);
    static CALL_SLOT: AtomicUsize = AtomicUsize::new(0);

    fn base() -> usize {
        (&raw const MODULE) as usize
    }

    unsafe extern "C" fn log(_: u32, _: *const c_char) {}
    unsafe extern "C" fn module_base(name: *const c_char) -> *mut c_void {
        if unsafe { core::ffi::CStr::from_ptr(name) } == c"game.dll" {
            base() as *mut c_void
        } else {
            core::ptr::null_mut()
        }
    }
    unsafe extern "C" fn module_size(_: *mut c_void) -> usize {
        48
    }
    unsafe extern "C" fn find_pattern(_: *mut c_void, _: usize, _: *const c_char) -> *mut c_void {
        core::ptr::null_mut()
    }
    unsafe extern "C" fn find_pattern_at(
        _: *mut c_void,
        _: usize,
        _: *const c_char,
        _: usize,
    ) -> *mut c_void {
        core::ptr::null_mut()
    }
    unsafe extern "C" fn hook(_: *mut c_void, _: *mut c_void, _: *mut *mut c_void) -> i32 {
        WRITES.fetch_add(1, Ordering::SeqCst);
        1
    }
    unsafe extern "C" fn hook_exact(
        _: *mut c_void,
        _: *mut c_void,
        _: usize,
        original: *mut *mut c_void,
    ) -> i32 {
        WRITES.fetch_add(1, Ordering::SeqCst);
        unsafe { original.write(0x1000 as *mut c_void) };
        0
    }
    unsafe extern "C" fn hook_call(
        _: *mut c_void,
        _: *mut c_void,
        original: *mut *mut c_void,
    ) -> i32 {
        WRITES.fetch_add(1, Ordering::SeqCst);
        if REFUSE_CALL.load(Ordering::SeqCst) {
            return 1;
        }
        unsafe { original.write(0x2000 as *mut c_void) };
        0
    }
    unsafe extern "C" fn unhook(_: *mut c_void) -> i32 {
        UNHOOKS.fetch_add(1, Ordering::SeqCst);
        0
    }
    unsafe extern "C" fn rtti_method(_: *const c_char, _: *const c_char) -> *mut c_void {
        core::ptr::null_mut()
    }
    unsafe extern "C" fn vtable_slot(_: *const c_char, _: usize) -> *mut c_void {
        core::ptr::null_mut()
    }
    unsafe extern "C" fn config_get(_: *const c_char, _: *const c_char) -> *const c_char {
        core::ptr::null()
    }
    unsafe extern "C" fn patch_bytes(_: *mut c_void, _: *const u8, _: *const u8, _: usize) -> i32 {
        WRITES.fetch_add(1, Ordering::SeqCst);
        0
    }

    static API: Api = Api {
        abi_version: defiance_api::ABI_VERSION,
        reserved: 0,
        log,
        module_base,
        module_size,
        find_pattern,
        find_pattern_at,
        hook,
        hook_exact,
        hook_call,
        unhook,
        rtti_method,
        vtable_slot,
        config_get,
        patch_bytes,
    };

    fn plan() -> Plan {
        let detour = 0x3000 as *mut c_void;
        Plan {
            writes: vec![
                Write {
                    target: base(),
                    kind: Kind::Entry(Some(5)),
                    detour,
                    original: Some(&ENTRY_SLOT),
                },
                Write {
                    target: base() + 32,
                    kind: Kind::Bytes {
                        before: vec![0x90, 0x90],
                        after: vec![0xeb, 0x00],
                    },
                    detour: core::ptr::null_mut(),
                    original: None,
                },
                Write {
                    target: base() + 16,
                    kind: Kind::Call,
                    detour,
                    original: Some(&CALL_SLOT),
                },
            ],
        }
    }

    fn summary(patches: &[Patch]) -> Vec<(usize, u32, Vec<u8>, Option<Vec<u8>>)> {
        patches
            .iter()
            .map(|p| (p.rva, p.kind, p.before.clone(), p.after.clone()))
            .collect()
    }

    /// Reading a contract writes nothing and binds nothing, so repeated
    /// queries agree; a refused write withdraws the staged ones and leaves
    /// every slot unbound; a staged plan binds each continuation.
    #[test]
    fn contract_is_pure_and_apply_is_all_or_nothing() {
        let plan = plan();
        let first = summary(&contract(&API, &plan).unwrap());
        let second = summary(&contract(&API, &plan).unwrap());
        assert_eq!(first, second);
        assert_eq!(
            first,
            vec![
                (
                    0,
                    PATCH_KIND_ENTRY,
                    vec![0x55, 0x48, 0x89, 0xe5, 0x90],
                    None
                ),
                (
                    32,
                    PATCH_KIND_BYTES,
                    vec![0x90, 0x90],
                    Some(vec![0xeb, 0x00])
                ),
                (16, PATCH_KIND_CALL, vec![0xe8, 0x10, 0, 0, 0], None),
            ]
        );
        assert_eq!(WRITES.load(Ordering::SeqCst), 0);

        REFUSE_CALL.store(true, Ordering::SeqCst);
        assert!(apply(&API, &plan).is_err());
        assert_eq!(UNHOOKS.load(Ordering::SeqCst), 2);
        assert_eq!(ENTRY_SLOT.load(Ordering::SeqCst), 0);
        assert_eq!(CALL_SLOT.load(Ordering::SeqCst), 0);

        REFUSE_CALL.store(false, Ordering::SeqCst);
        apply(&API, &plan).unwrap();
        assert_eq!(ENTRY_SLOT.load(Ordering::SeqCst), 0x1000);
        assert_eq!(CALL_SLOT.load(Ordering::SeqCst), 0x2000);
        assert_eq!(summary(&contract(&API, &plan).unwrap()), first);
    }
}
