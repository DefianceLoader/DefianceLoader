//! Finding the game's modules, and building the `Api` a plugin receives.
//!
//! The loader runs inside `trm.exe`, so a module is already loaded and its base
//! is a `GetModuleHandleW` away; there is no snapshot of another process and no
//! file hash to check. What a plugin asks for instead is a signature unique in
//! the running module, which is the same guarantee `--scan` gives the injector
//! and the reason a plugin survives a game update that only moves code.

use crate::win;
use core::ffi::{c_char, c_void, CStr};
use core::ptr;
use defiance_api::{Api, ABI_VERSION, LOG_ERROR, LOG_WARN};
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The `CString`s handed to `config_get`, kept so the pointers stay valid.
static CONFIG_CACHE: OnceLock<Mutex<HashMap<(String, String), CString>>> = OnceLock::new();

/// A loaded module's base and mapped size, or `None` if it is not loaded.
pub fn module(name: &str) -> Option<(*mut c_void, usize)> {
    let handle = unsafe { win::GetModuleHandleW(win::wide(name).as_ptr()) };
    if handle.is_null() {
        return None;
    }
    let mut info = win::ModuleInfo {
        base: ptr::null_mut(),
        size: 0,
        entry: ptr::null_mut(),
    };
    let ok = unsafe {
        win::GetModuleInformation(
            win::GetCurrentProcess(),
            handle,
            &mut info,
            core::mem::size_of::<win::ModuleInfo>() as u32,
        )
    };
    (ok != 0).then_some((info.base, info.size as usize))
}

/// Wait for a module to appear, polling, because a proxy DLL is loaded before
/// the game's own DLLs are.
pub fn wait_for(name: &str, timeout: Duration) -> Option<(*mut c_void, usize)> {
    let started = Instant::now();
    loop {
        if let Some(found) = module(name) {
            return Some(found);
        }
        if started.elapsed() > timeout {
            return None;
        }
        unsafe { win::Sleep(200) };
    }
}

/// The `Api` the host hands to each plugin's `init`.
pub fn build_api() -> Api {
    Api {
        abi_version: ABI_VERSION,
        reserved: 0,
        log: api_log,
        module_base: api_module_base,
        module_size: api_module_size,
        find_pattern: api_find_pattern,
        find_pattern_at: api_find_pattern_at,
        hook: api_hook,
        hook_exact: api_hook_exact,
        hook_call: api_hook_call,
        unhook: api_unhook,
        rtti_method: api_rtti_method,
        vtable_slot: api_vtable_slot,
        config_get: api_config_get,
        patch_bytes: api_patch_bytes,
    }
}

unsafe fn c_string(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

unsafe extern "C" fn api_log(level: u32, message: *const c_char) {
    let text = unsafe { c_string(message) };
    match level {
        LOG_WARN => crate::log::warn(&text),
        LOG_ERROR => crate::log::error(&text),
        _ => crate::log::info(&text),
    }
}

unsafe extern "C" fn api_module_base(name: *const c_char) -> *mut c_void {
    let name = unsafe { c_string(name) };
    module(&name)
        .map(|(base, _)| base)
        .unwrap_or(ptr::null_mut())
}

unsafe extern "C" fn api_module_size(base: *mut c_void) -> usize {
    if base.is_null() {
        return 0;
    }
    // For a loaded module the HMODULE is the base address.
    let mut info = win::ModuleInfo {
        base: ptr::null_mut(),
        size: 0,
        entry: ptr::null_mut(),
    };
    let ok = unsafe {
        win::GetModuleInformation(
            win::GetCurrentProcess(),
            base,
            &mut info,
            core::mem::size_of::<win::ModuleInfo>() as u32,
        )
    };
    if ok == 0 {
        0
    } else {
        info.size as usize
    }
}

/// The one match of `pattern`, or an error naming why there is not one.
unsafe fn find_one(
    base: *mut c_void,
    size: usize,
    pattern: *const c_char,
) -> Result<usize, String> {
    if base.is_null() || size == 0 {
        return Err("no module given".to_string());
    }
    let text = unsafe { c_string(pattern) };
    let parsed =
        defiance_core::parse_pattern(&text).map_err(|e| format!("bad signature `{text}`: {e}"))?;
    let image = unsafe { core::slice::from_raw_parts(base as *const u8, size) };
    match defiance_core::scan(image, &parsed).as_slice() {
        [one] => Ok(*one),
        [] => Err(format!("signature `{text}` is not in the module")),
        many => Err(format!("signature `{text}` matches {} places", many.len())),
    }
}

unsafe extern "C" fn api_find_pattern(
    base: *mut c_void,
    size: usize,
    pattern: *const c_char,
) -> *mut c_void {
    match unsafe { find_one(base, size, pattern) } {
        Ok(at) => unsafe { (base as *mut u8).add(at) as *mut c_void },
        Err(message) => {
            crate::log::warn(&message);
            ptr::null_mut()
        }
    }
}

unsafe extern "C" fn api_find_pattern_at(
    base: *mut c_void,
    size: usize,
    pattern: *const c_char,
    offset: usize,
) -> *mut c_void {
    let text = unsafe { c_string(pattern) };
    let length = text.chars().filter(|c| !c.is_whitespace()).count() / 2;
    if offset > length {
        crate::log::error(&format!(
            "offset {offset} is outside the {length}-byte `{text}` signature"
        ));
        return ptr::null_mut();
    }
    match unsafe { find_one(base, size, pattern) } {
        Ok(at) => unsafe { (base as *mut u8).add(at + offset) as *mut c_void },
        Err(message) => {
            crate::log::warn(&message);
            ptr::null_mut()
        }
    }
}

unsafe extern "C" fn api_hook(
    target: *mut c_void,
    detour: *mut c_void,
    original: *mut *mut c_void,
) -> i32 {
    // The loader stores `original` before publishing the branch, so a detour
    // that fires immediately already has its stock pointer.
    match unsafe { crate::hooks::install_auto_out(target, detour, original) } {
        Ok(_) => 0,
        Err(e) => {
            crate::log::error(&format!("hook at {target:p} failed: {e}"));
            -1
        }
    }
}

unsafe extern "C" fn api_hook_exact(
    target: *mut c_void,
    detour: *mut c_void,
    displaced: usize,
    original: *mut *mut c_void,
) -> i32 {
    match unsafe { crate::hooks::install_out(target, detour, displaced, original) } {
        Ok(_) => 0,
        Err(e) => {
            crate::log::error(&format!("hook at {target:p} failed: {e}"));
            -1
        }
    }
}

unsafe extern "C" fn api_hook_call(
    site: *mut c_void,
    detour: *mut c_void,
    original: *mut *mut c_void,
) -> i32 {
    match unsafe { crate::hooks::install_call_out(site, detour, original) } {
        Ok(_) => 0,
        Err(e) => {
            crate::log::error(&format!("call hook at {site:p} failed: {e}"));
            -1
        }
    }
}

unsafe extern "C" fn api_unhook(target: *mut c_void) -> i32 {
    match crate::hooks::remove(target) {
        Ok(()) => 0,
        Err(e) => {
            crate::log::error(&format!("unhook at {target:p} failed: {e}"));
            -1
        }
    }
}

unsafe extern "C" fn api_rtti_method(class: *const c_char, method: *const c_char) -> *mut c_void {
    let class = unsafe { c_string(class) };
    let method = unsafe { c_string(method) };
    crate::rtti::method(&class, &method)
        .map(|address| address as *mut c_void)
        .unwrap_or(ptr::null_mut())
}

unsafe extern "C" fn api_config_get(section: *const c_char, key: *const c_char) -> *const c_char {
    let section = unsafe { c_string(section) };
    let key = unsafe { c_string(key) };
    let Some(value) = crate::config::get(&section, &key) else {
        return ptr::null();
    };
    let cache = CONFIG_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap();
    let stored = cache
        .entry((section, key))
        .or_insert_with(|| CString::new(value.replace('\0', "")).unwrap_or_default());
    stored.as_ptr()
}

unsafe extern "C" fn api_vtable_slot(class: *const c_char, slot: usize) -> *mut c_void {
    let class = unsafe { c_string(class) };
    crate::rtti::vtable_slot(&class, slot)
        .map(|address| address as *mut c_void)
        .unwrap_or(ptr::null_mut())
}

unsafe extern "C" fn api_patch_bytes(
    target: *mut c_void,
    before: *const u8,
    after: *const u8,
    length: usize,
) -> i32 {
    if target.is_null() || before.is_null() || after.is_null() || length == 0 {
        return -1;
    }
    match unsafe {
        crate::hooks::patch_bytes(
            target,
            core::slice::from_raw_parts(before, length),
            core::slice::from_raw_parts(after, length),
        )
    } {
        Ok(()) => 0,
        Err(e) => {
            crate::log::error(&e);
            -1
        }
    }
}
