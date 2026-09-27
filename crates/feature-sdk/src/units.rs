//! A plugin's own patch units (`tools/units.py`), compiled in for every build
//! it supports and installed through Core's `patch` service.
//!
//! A plugin's `build.rs` calls `defiance_build_support::embed_units` with its
//! plugin name; the plugin includes the result as its [`Embedded`] slice and
//! calls [`install`] from its init.

use core::ffi::{c_void, CStr};
use defiance_api::{Api, NativeReplacementV1, PatchUnitV1, ABI_VERSION, LOG_ERROR};

/// One unit compiled into a plugin: the build it was resolved for
/// (`reference` or a variant's name, `tools/variants/<build>/units/`), its
/// descriptor and its blob.
pub struct Embedded {
    pub build: &'static str,
    pub name: &'static str,
    pub descriptor: &'static str,
    pub code: &'static [u8],
}

fn log(api: &Api, message: &str) {
    let text = std::ffi::CString::new(message.replace('\0', "")).unwrap_or_default();
    unsafe { (api.log)(LOG_ERROR, text.as_ptr()) };
}

/// Install the units of `units` resolved for the build Core recognised:
/// Core prepares them, `cells` fills any named cell the hooks read (given a
/// lookup from cell name to address) before they go live, and Core writes
/// every site under this plugin's ownership. `replacements` and `call_detour`
/// are `PatchV1::install`'s. Returns 0 when everything was written.
///
/// # Safety
/// Call only during init, with the `api` the host passed; the replacements'
/// detours must match their functions' ABI and stay for the process's life.
pub unsafe fn install(
    api: *const Api,
    units: &[Embedded],
    cells: impl FnOnce(&dyn Fn(&CStr) -> Option<usize>),
    replacements: &[NativeReplacementV1],
    call_detour: *mut c_void,
) -> i32 {
    let Some(host) = (unsafe { api.as_ref() }) else {
        return 1;
    };
    if host.abi_version != ABI_VERSION || host.reserved != 0 {
        return 1;
    }
    let (Some(patch), Some(build)) = (unsafe { crate::services::patch() }, unsafe {
        crate::services::build()
    }) else {
        log(host, "this plugin needs Core's patch and build services");
        return 1;
    };
    let name = unsafe { (build.name)() };
    if name.is_null() {
        return 1;
    }
    let build = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    let chosen: Vec<PatchUnitV1> = units
        .iter()
        .filter(|unit| unit.build == build)
        .map(|unit| PatchUnitV1 {
            descriptor: unit.descriptor.as_ptr(),
            descriptor_len: unit.descriptor.len(),
            code: unit.code.as_ptr(),
            code_len: unit.code.len(),
        })
        .collect();
    if chosen.is_empty() {
        log(
            host,
            &format!("this plugin has no units for the build {build}"),
        );
        return 1;
    }
    let prepared = unsafe { (patch.prepare)(api, chosen.as_ptr(), chosen.len()) };
    if prepared.is_null() {
        return 1;
    }
    cells(&|name: &CStr| {
        let address = unsafe { (patch.cell)(prepared, name.as_ptr()) };
        (address != 0).then_some(address)
    });
    unsafe {
        (patch.install)(
            api,
            prepared,
            replacements.as_ptr(),
            replacements.len(),
            call_detour,
        )
    }
}
