//! Helpers for describing loader-managed writes in a plugin's patch contract.

use core::ffi::CStr;
use defiance_api::{Api, PatchContractEntryV1, PatchContractV1, ABI_VERSION, PATCH_KIND_BYTES};
use std::ffi::CString;

/// One expected loader-managed write, addressed by module-relative offset.
pub struct Patch {
    pub module: &'static CStr,
    pub rva: usize,
    pub kind: u32,
    pub before: Vec<u8>,
    pub after: Option<Vec<u8>>,
}

impl Patch {
    /// Describe a byte patch whose replacement is computed during init.
    pub fn bytes(module: &'static CStr, rva: usize, before: &[u8]) -> Self {
        Self {
            module,
            rva,
            kind: PATCH_KIND_BYTES,
            before: before.to_vec(),
            after: None,
        }
    }
}

struct OwnedPatch {
    module: CString,
    rva: usize,
    kind: u32,
    before: Box<[u8]>,
    after: Option<Box<[u8]>>,
}

struct Storage {
    _owned: Box<[OwnedPatch]>,
    _entries: Box<[PatchContractEntryV1]>,
    contract: PatchContractV1,
}

/// Build a v1 expected-write export from the site's runtime-selected patches.
///
/// The returned storage is retained for the module's lifetime because the C
/// ABI has no release callback. Loader callers copy the declarations before
/// `init`, so each invocation can reflect current build and config selection.
pub unsafe fn build(api: *const Api, patches: Vec<Patch>) -> *const PatchContractV1 {
    let Some(host) = (unsafe { api.as_ref() }) else {
        return core::ptr::null();
    };
    if host.abi_version != ABI_VERSION || host.reserved != 0 {
        return core::ptr::null();
    }
    let Some(storage) = storage(patches) else {
        let message = c"could not build expected-write contract";
        unsafe { (host.log)(defiance_api::LOG_ERROR, message.as_ptr()) };
        return core::ptr::null();
    };
    let contract = &storage.contract as *const PatchContractV1;
    let _ = Box::into_raw(storage);
    contract
}

fn storage(patches: Vec<Patch>) -> Option<Box<Storage>> {
    let owned = patches
        .into_iter()
        .map(|patch| {
            if patch.before.is_empty()
                || patch.before.len() > 4096
                || patch
                    .after
                    .as_ref()
                    .is_some_and(|after| after.len() != patch.before.len())
            {
                return None;
            }
            Some(OwnedPatch {
                module: CString::new(patch.module.to_bytes()).ok()?,
                rva: patch.rva,
                kind: patch.kind,
                before: patch.before.into_boxed_slice(),
                after: patch.after.map(Vec::into_boxed_slice),
            })
        })
        .collect::<Option<Vec<_>>>();
    let owned = owned?;
    let owned = owned.into_boxed_slice();
    let entries: Box<[PatchContractEntryV1]> = owned
        .iter()
        .map(|patch| PatchContractEntryV1 {
            module: patch.module.as_ptr(),
            rva: patch.rva,
            kind: patch.kind,
            before: patch.before.as_ptr(),
            before_len: patch.before.len(),
            after: patch
                .after
                .as_ref()
                .map_or(core::ptr::null(), |after| after.as_ptr()),
            after_len: patch.after.as_ref().map_or(0, |after| after.len()),
        })
        .collect();
    let mut storage = Box::new(Storage {
        _owned: owned,
        _entries: entries,
        contract: PatchContractV1 {
            version: 1,
            size: core::mem::size_of::<PatchContractV1>() as u32,
            entries: core::ptr::null(),
            count: 0,
        },
    });
    storage.contract.entries = storage._entries.as_ptr();
    storage.contract.count = storage._entries.len();
    Some(storage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_keeps_each_module_and_byte_span_alive() {
        let storage = storage(vec![Patch {
            module: c"game.dll",
            rva: 0x1234,
            kind: PATCH_KIND_BYTES,
            before: vec![1, 2, 3],
            after: Some(vec![4, 5, 6]),
        }])
        .unwrap();
        let contract = &storage.contract;
        assert_eq!(contract.version, 1);
        assert_eq!(contract.count, 1);
        let entry = unsafe { &*contract.entries };
        assert_eq!(
            unsafe { CStr::from_ptr(entry.module) }.to_bytes(),
            b"game.dll"
        );
        assert_eq!(entry.rva, 0x1234);
        assert_eq!(entry.kind, PATCH_KIND_BYTES);
        assert_eq!(
            unsafe { core::slice::from_raw_parts(entry.before, entry.before_len) },
            [1, 2, 3]
        );
        assert_eq!(
            unsafe { core::slice::from_raw_parts(entry.after, entry.after_len) },
            [4, 5, 6]
        );
    }

    #[test]
    fn contract_rejects_empty_or_mismatched_byte_spans() {
        assert!(storage(vec![Patch::bytes(c"game.dll", 1, &[])]).is_none());
        assert!(storage(vec![Patch {
            module: c"game.dll",
            rva: 1,
            kind: PATCH_KIND_BYTES,
            before: vec![1],
            after: Some(vec![2, 3]),
        }])
        .is_none());
    }
}
