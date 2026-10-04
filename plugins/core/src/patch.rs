//! Core's `patch` service (`PatchV1`): patch units handed over by a plugin,
//! relocated if the build calls for it, linked near their module, and written
//! under that plugin's ownership.
//!
//! A unit is linked once per content and stays for the process's life: game
//! code may still be returning through it after its plugin is gone. A plugin
//! that installs the same unit again (hot reload) gets the same copy back.

use crate::{say, Runtime, RUNTIME};
use core::ffi::{c_char, c_void, CStr};
use defiance_api::{
    Api, NativeReplacementV1, PatchContractEntryV1, PatchContractV1, PatchUnitV1, PatchV1,
    ABI_VERSION, LOG_DEBUG, LOG_ERROR, PATCH_KIND_BYTES, PATCH_KIND_CALL, PATCH_KIND_ENTRY,
};
use defiance_core::apply::resolve_export;
use defiance_core::unit::{Kind, Placed, Unit};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A unit linked into its module's pool.
pub(crate) struct Prepared {
    /// The unit as it applies to this build (relocated if it had to be).
    pub unit: Unit,
    pub at: usize,
    pub writes: Vec<Placed>,
}

impl Prepared {
    pub fn cell(&self, name: &str) -> Option<(usize, usize)> {
        self.unit
            .cell(name)
            .map(|cell| (self.at + cell.offset, cell.bytes))
    }
}

/// A linked unit and the content it was linked from.
pub(crate) struct Linked {
    descriptor: String,
    code: Vec<u8>,
    pub prepared: Arc<Prepared>,
}

/// The units one `prepare` call linked, which `install` writes.
pub(crate) struct Handle {
    pub units: Vec<Arc<Prepared>>,
    contract_kinds: std::sync::Mutex<Option<Vec<u32>>>,
}

struct OwnedContractEntry {
    module: std::ffi::CString,
    rva: usize,
    kind: u32,
    before: Box<[u8]>,
}

struct ContractStorage {
    _owned: Box<[OwnedContractEntry]>,
    _entries: Box<[PatchContractEntryV1]>,
    contract: PatchContractV1,
}

impl Handle {
    pub fn cell(&self, name: &str) -> Option<(usize, usize)> {
        self.units.iter().find_map(|unit| unit.cell(name))
    }
}

/// Relocate `unit` if the build calls for it, link it into its module's pool
/// and work out its writes.
fn link(runtime: &Runtime, api: &Api, unit: &Unit, code: &[u8]) -> Result<Prepared, String> {
    let module = runtime.module(unit);
    let unit = match &module.pristine {
        Some(image) => unit.relocate(image, &module.sha)?,
        None => unit.clone(),
    };
    let at = module
        .pool
        .take(unit.unit_bytes.max(1))
        .ok_or("the unit pool is full")?;
    let base = module.base;
    let linked = unit.link(
        code,
        base,
        at,
        &|cell| (cell == "trace_ring").then_some(runtime.trace_ring),
        &|dll, name| resolve_export(dll, name).map(|address| address as usize),
    )?;
    let writes = unit.placed(base, at)?;
    unsafe { core::ptr::copy_nonoverlapping(linked.as_ptr(), at as *mut u8, linked.len()) };
    let label = std::ffi::CString::new(format!("{} unit", unit.name)).unwrap_or_default();
    if let Some(ranges) = runtime.crash_ranges {
        unsafe { (ranges.map)(at, at + unit.unit_bytes.max(1), label.as_ptr()) };
    }
    say(
        api,
        LOG_DEBUG,
        &format!(
            "{} at {at:#x}..{:#x}",
            label.to_string_lossy(),
            at + unit.unit_bytes
        ),
    );
    for write in unit.writes.iter().filter(|w| w.kind != Kind::Edit) {
        say(
            api,
            LOG_DEBUG,
            &format!(
                "payload-entry {} {} address={:#x} site-rva={:#x}",
                unit.name,
                write.label,
                at + write.entry,
                write.rva
            ),
        );
    }
    Ok(Prepared { unit, at, writes })
}

/// Link `units` (descriptor, blob), reusing a unit already linked from the
/// same content.
pub(crate) fn prepare(api: &Api, units: &[(&str, &[u8])]) -> Result<Handle, String> {
    let runtime = RUNTIME.get().ok_or("core runtime did not initialize")?;
    let mut linked = runtime.linked.lock().unwrap();
    let mut out: Vec<Arc<Prepared>> = Vec::new();
    for &(descriptor, code) in units {
        let unit = Unit::parse(descriptor)?;
        if out.iter().any(|p| p.unit.name == unit.name) {
            return Err(format!("{} is given twice", unit.name));
        }
        let reuse = linked
            .get(&unit.name)
            .filter(|l| l.descriptor == descriptor && l.code == code)
            .map(|l| l.prepared.clone());
        let prepared = match reuse {
            Some(prepared) => prepared,
            None => {
                let prepared = Arc::new(link(runtime, api, &unit, code)?);
                linked.insert(
                    unit.name.clone(),
                    Linked {
                        descriptor: descriptor.to_string(),
                        code: code.to_vec(),
                        prepared: prepared.clone(),
                    },
                );
                prepared
            }
        };
        out.push(prepared);
    }
    Ok(Handle {
        units: out,
        contract_kinds: std::sync::Mutex::new(None),
    })
}

/// Stage every site of `handle` under the calling plugin's ownership, after
/// checking all of them: 0 when accepted, or nonzero with no live game writes
/// from this call. The host publishes managed requests with the runtime plan.
/// `replacements` take their named
/// functions instead of the units' jmps; `detour`, when not null, takes the
/// units' one call write.
pub(crate) fn install(
    api: &Api,
    handle: &Handle,
    replacements: &[NativeReplacementV1],
    detour: *mut c_void,
) -> i32 {
    let Some(runtime) = RUNTIME.get() else {
        return 1;
    };
    *handle
        .contract_kinds
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    let writes: Vec<&Placed> = handle.units.iter().flat_map(|unit| &unit.writes).collect();
    let mut native = BTreeMap::new();
    for replacement in replacements {
        if replacement.name.is_null() || replacement.detour.is_null() {
            return 1;
        }
        let Ok(name) = (unsafe { CStr::from_ptr(replacement.name) }).to_str() else {
            return 1;
        };
        let Some(address) = handle.units.iter().find_map(|unit| {
            unit.unit
                .natives
                .iter()
                .find(|n| n.name == name)
                .map(|n| runtime.module(&unit.unit).base + n.rva)
        }) else {
            say(api, LOG_ERROR, &format!("no unit names the native {name}"));
            return 1;
        };
        if !writes.iter().any(|w| w.0 == address)
            || native.insert(address, replacement.detour).is_some()
        {
            return 1;
        }
    }
    if !detour.is_null() && (writes.len() != 1 || writes[0].1.len() != 5 || writes[0].1[0] != 0xe8)
    {
        say(
            api,
            LOG_ERROR,
            "call detour refused: expected one verified rel32 call",
        );
        return 1;
    }
    let contract_kinds: Vec<u32> = writes
        .iter()
        .map(|(address, before, _)| {
            if native.contains_key(address) {
                if before.len() >= 14 {
                    PATCH_KIND_BYTES
                } else {
                    PATCH_KIND_ENTRY
                }
            } else if detour.is_null() {
                PATCH_KIND_BYTES
            } else {
                PATCH_KIND_CALL
            }
        })
        .collect();
    // Validate every site before changing either module.
    for (address, before, _) in &writes {
        let now = unsafe { core::slice::from_raw_parts(*address as *const u8, before.len()) };
        if now != before.as_slice() {
            say(
                api,
                LOG_ERROR,
                &format!("patch refused: the site at {address:#x} is not what the unit expects"),
            );
            return 1;
        }
    }
    for (address, before, after) in &writes {
        let result = if let Some(&replacement) = native.get(address) {
            if before.len() >= 14 {
                // Complete replacement needs no original trampoline. An absolute
                // branch can cover bodies containing relative instructions.
                let mut branch = vec![0xff, 0x25, 0, 0, 0, 0];
                branch.extend_from_slice(&(replacement as usize as u64).to_le_bytes());
                branch.resize(before.len(), 0x90);
                unsafe {
                    (api.patch_bytes)(
                        *address as *mut c_void,
                        before.as_ptr(),
                        branch.as_ptr(),
                        branch.len(),
                    )
                }
            } else {
                let mut original = core::ptr::null_mut();
                unsafe {
                    (api.hook_exact)(
                        *address as *mut c_void,
                        replacement,
                        before.len(),
                        &mut original,
                    )
                }
            }
        } else if detour.is_null() {
            unsafe {
                (api.patch_bytes)(
                    *address as *mut c_void,
                    before.as_ptr(),
                    after.as_ptr(),
                    before.len(),
                )
            }
        } else {
            let mut original = core::ptr::null_mut();
            unsafe { (api.hook_call)(*address as *mut c_void, detour, &mut original) }
        };
        if result != 0 {
            // Propagating this failure from init makes the host discard every
            // staged request owned by this plugin.
            say(
                api,
                LOG_ERROR,
                "patch staging failed; return init failure so the host discards owned spans",
            );
            return 1;
        }
    }
    *handle
        .contract_kinds
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(contract_kinds);
    // The diagnostics record follows whatever units are linked by now.
    runtime.record();
    let names: Vec<&str> = handle
        .units
        .iter()
        .map(|unit| unit.unit.name.as_str())
        .collect();
    say(
        api,
        LOG_DEBUG,
        &format!(
            "staged {} patch spans from {}{}",
            writes.len(),
            names.join(", "),
            if native.is_empty() {
                String::new()
            } else {
                format!(", {} of them Rust function replacements", native.len())
            }
        ),
    );
    0
}

fn host(api: *const Api) -> Option<&'static Api> {
    let api = unsafe { api.as_ref() }?;
    (api.abi_version == ABI_VERSION && api.reserved == 0).then_some(api)
}

unsafe extern "C" fn service_prepare(
    api: *const Api,
    units: *const PatchUnitV1,
    count: usize,
) -> *mut c_void {
    let Some(api) = host(api) else {
        return core::ptr::null_mut();
    };
    if units.is_null() || count == 0 {
        return core::ptr::null_mut();
    }
    let mut given = Vec::new();
    for unit in unsafe { core::slice::from_raw_parts(units, count) } {
        if unit.descriptor.is_null() || (unit.code.is_null() && unit.code_len != 0) {
            return core::ptr::null_mut();
        }
        let descriptor =
            unsafe { core::slice::from_raw_parts(unit.descriptor, unit.descriptor_len) };
        let Ok(descriptor) = core::str::from_utf8(descriptor) else {
            say(
                api,
                LOG_ERROR,
                "patch refused: a unit descriptor is not UTF-8",
            );
            return core::ptr::null_mut();
        };
        let code: &[u8] = if unit.code_len == 0 {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(unit.code, unit.code_len) }
        };
        given.push((descriptor, code));
    }
    match prepare(api, &given) {
        Ok(handle) => Box::into_raw(Box::new(handle)).cast(),
        Err(e) => {
            say(
                api,
                LOG_ERROR,
                &format!("patch refused: its site could not be prepared in this build: {e}"),
            );
            core::ptr::null_mut()
        }
    }
}

unsafe extern "C" fn service_cell(prepared: *mut c_void, name: *const c_char) -> usize {
    if prepared.is_null() || name.is_null() {
        return 0;
    }
    let handle = unsafe { &*prepared.cast::<Handle>() };
    let Ok(name) = (unsafe { CStr::from_ptr(name) }).to_str() else {
        return 0;
    };
    handle.cell(name).map_or(0, |(address, _)| address)
}

unsafe extern "C" fn service_install(
    api: *const Api,
    prepared: *mut c_void,
    replacements: *const NativeReplacementV1,
    count: usize,
    call_detour: *mut c_void,
) -> i32 {
    let Some(api) = host(api) else {
        return 1;
    };
    if prepared.is_null() || (replacements.is_null() && count != 0) || count > 16 {
        return 1;
    }
    let replacements = if count == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(replacements, count) }
    };
    install(
        api,
        unsafe { &*prepared.cast::<Handle>() },
        replacements,
        call_detour,
    )
}

unsafe extern "C" fn service_contract(
    api: *const Api,
    prepared: *mut c_void,
) -> *const PatchContractV1 {
    let Some(api) = host(api) else {
        return core::ptr::null();
    };
    if prepared.is_null() {
        return core::ptr::null();
    }
    let handle = unsafe { &*prepared.cast::<Handle>() };
    let Some(kinds) = handle
        .contract_kinds
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    else {
        say(
            api,
            LOG_ERROR,
            "patch contract requested before a successful install",
        );
        return core::ptr::null();
    };

    let mut owned = Vec::new();
    for unit in &handle.units {
        if unit.unit.writes.len() != unit.writes.len() {
            say(
                api,
                LOG_ERROR,
                "patch contract refused: prepared write count changed",
            );
            return core::ptr::null();
        }
        let Ok(module) = std::ffi::CString::new(unit.unit.module.as_bytes()) else {
            say(
                api,
                LOG_ERROR,
                "patch contract refused: invalid module name",
            );
            return core::ptr::null();
        };
        for (write, placed) in unit.unit.writes.iter().zip(&unit.writes) {
            let (_, before, _) = placed;
            if before.is_empty() || before.len() > 4096 {
                say(
                    api,
                    LOG_ERROR,
                    "patch contract refused: invalid prepared write span",
                );
                return core::ptr::null();
            }
            owned.push(OwnedContractEntry {
                module: module.clone(),
                rva: write.rva,
                kind: 0,
                before: before.clone().into_boxed_slice(),
            });
        }
    }
    if owned.len() != kinds.len() || owned.len() > 4096 {
        say(
            api,
            LOG_ERROR,
            "patch contract refused: prepared write count mismatch",
        );
        return core::ptr::null();
    }
    for (entry, kind) in owned.iter_mut().zip(kinds) {
        entry.kind = kind;
    }
    for first in 0..owned.len() {
        if owned[..first]
            .iter()
            .any(|entry| entry.module == owned[first].module && entry.rva == owned[first].rva)
        {
            say(
                api,
                LOG_ERROR,
                "patch contract refused: duplicate prepared write site",
            );
            return core::ptr::null();
        }
    }
    let entries: Box<[PatchContractEntryV1]> = owned
        .iter()
        .map(|entry| PatchContractEntryV1 {
            module: entry.module.as_ptr(),
            rva: entry.rva,
            kind: entry.kind,
            before: entry.before.as_ptr(),
            before_len: entry.before.len(),
            after: core::ptr::null(),
            after_len: 0,
        })
        .collect();
    let contract = PatchContractV1 {
        version: 1,
        size: core::mem::size_of::<PatchContractV1>() as u32,
        entries: entries.as_ptr(),
        count: entries.len(),
    };
    let storage = Box::new(ContractStorage {
        _owned: owned.into_boxed_slice(),
        _entries: entries,
        contract,
    });
    let contract = &storage.contract as *const PatchContractV1;
    let _ = Box::into_raw(storage);
    contract
}

pub(crate) static API: PatchV1 = PatchV1 {
    prepare: service_prepare,
    cell: service_cell,
    install: service_install,
    contract: service_contract,
};
