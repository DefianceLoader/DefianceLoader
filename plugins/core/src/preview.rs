//! The squad preview's dimmed materials, for `patch/preview-dim.asm`.
//!
//! logic.dll's preview builder gives each renderer of a dimmed soldier an
//! override material through the callback this module provides. The material is
//! the part's own one, copied into the squad scroll mod as
//! `materials/defiance_dim/<rest of its path>` with a darker albedo
//! (`tools/package_squad_scroll.py`), and loaded the way the preview context
//! loads its `materials/dead.material`: the context's server (`[ctx]`) vt+0x90,
//! that result's vt+0x38 (the resource manager), and its vt+0x188(manager,
//! &out shared_ptr, &path).
//!
//! Each path is loaded once; its shared_ptr is leaked, so the address handed
//! back stays valid for the life of the process. A path that fails to load is
//! remembered as 0 and the part keeps its own material.
use std::collections::HashMap;
use std::sync::Mutex;

/// Paths already asked for: the leaked shared_ptr's address, or 0.
static LOADED: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);
/// The logger, set when Core initializes.
static LOG: std::sync::OnceLock<unsafe extern "C" fn(u32, *const core::ffi::c_char)> =
    std::sync::OnceLock::new();
/// Load failures logged so far; the first few are enough to diagnose a mod
/// that is missing or out of date.
static FAILURES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
const FAILURES_LOGGED: u32 = 5;

/// The dimmed copies' folder under `materials/`.
pub const DIM_FOLDER: &str = "defiance_dim";

pub fn set_logger(log: unsafe extern "C" fn(u32, *const core::ffi::c_char)) {
    let _ = LOG.set(log);
}

/// The dimmed copy of a material path, or `None` for a path outside
/// `materials/` or already dimmed.
pub fn dim_path(name: &str) -> Option<String> {
    let normal = name.replace('\\', "/");
    let rest = normal.strip_prefix("materials/")?;
    if rest.is_empty() || rest.starts_with(&format!("{DIM_FOLDER}/")) {
        return None;
    }
    Some(format!("materials/{DIM_FOLDER}/{rest}"))
}

unsafe fn read(address: usize) -> usize {
    unsafe { *(address as *const usize) }
}

unsafe fn method(object: usize, slot: usize) -> usize {
    unsafe { read(read(object) + slot) }
}

/// A StandardMaterial's name, its MSVC std::string at +0x110.
unsafe fn material_name(material: usize) -> Option<String> {
    let length = unsafe { read(material + 0x120) };
    let capacity = unsafe { read(material + 0x128) };
    if length == 0 || length > 260 || capacity < length {
        return None;
    }
    let text = if capacity >= 16 {
        unsafe { read(material + 0x110) }
    } else {
        material + 0x110
    };
    let bytes = unsafe { core::slice::from_raw_parts(text as *const u8, length) };
    core::str::from_utf8(bytes).ok().map(str::to_owned)
}

/// Load `path` through the preview context's resource manager. Returns the
/// leaked shared_ptr's address, or 0 when the manager gives back no material.
unsafe fn load(context: usize, path: &str) -> usize {
    type Getter = unsafe extern "system" fn(usize) -> usize;
    type Load = unsafe extern "system" fn(usize, *mut [usize; 2], *const [usize; 4]) -> usize;
    let server = unsafe { read(context) };
    if server == 0 {
        return 0;
    }
    let resources: Getter = unsafe { core::mem::transmute(method(server, 0x90)) };
    let holder = unsafe { resources(server) };
    if holder == 0 {
        return 0;
    }
    let manager_of: Getter = unsafe { core::mem::transmute(method(holder, 0x38)) };
    let manager = unsafe { manager_of(holder) };
    if manager == 0 {
        return 0;
    }
    // An MSVC std::string in its heap form (capacity >= 16), which the callee
    // only reads; both are leaked with the result.
    let text: &'static [u8] = Box::leak(format!("{path}\0").into_bytes().into_boxed_slice());
    let string: &'static [usize; 4] = Box::leak(Box::new([
        text.as_ptr() as usize,
        0,
        path.len(),
        path.len().max(16),
    ]));
    let out: &'static mut [usize; 2] = Box::leak(Box::new([0, 0]));
    let load: Load = unsafe { core::mem::transmute(method(manager, 0x188)) };
    unsafe { load(manager, out, string) };
    if out[0] == 0 {
        0
    } else {
        out as *mut [usize; 2] as usize
    }
}

fn failed(path: &str) {
    let count = FAILURES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if count >= FAILURES_LOGGED {
        return;
    }
    if let Some(log) = LOG.get() {
        let text = std::ffi::CString::new(format!(
            "preview: no dimmed material {path}; the part keeps its own"
        ))
        .unwrap_or_default();
        unsafe { log(defiance_api::LOG_WARN, text.as_ptr()) };
    }
}

/// The callback `patch/preview-dim.asm` calls for each renderer of a dimmed
/// soldier: the address of a shared_ptr to its own material's dimmed copy, or
/// 0 to leave the renderer as it is. `part` is the renderer (its own material
/// at +0x50), `context` the preview context.
pub unsafe extern "C" fn dim_material(part: usize, context: usize) -> usize {
    if part == 0 || context == 0 {
        return 0;
    }
    let own = unsafe { read(part + 0x50) };
    if own == 0 {
        return 0;
    }
    let Some(path) = (unsafe { material_name(own) })
        .as_deref()
        .and_then(dim_path)
    else {
        return 0;
    };
    let Ok(mut loaded) = LOADED.lock() else {
        return 0;
    };
    let loaded = loaded.get_or_insert_with(HashMap::new);
    if let Some(&shared) = loaded.get(&path) {
        return shared;
    }
    let shared = unsafe { load(context, &path) };
    if shared == 0 {
        failed(&path);
    }
    loaded.insert(path, shared);
    shared
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dim_paths() {
        assert_eq!(
            dim_path("materials/units/soldier.material").as_deref(),
            Some("materials/defiance_dim/units/soldier.material")
        );
        assert_eq!(
            dim_path("materials\\units\\soldier.material").as_deref(),
            Some("materials/defiance_dim/units/soldier.material")
        );
        assert_eq!(dim_path("textures/soldier.material"), None);
        assert_eq!(
            dim_path("materials/defiance_dim/units/soldier.material"),
            None
        );
        assert_eq!(dim_path("materials/"), None);
    }

    #[test]
    fn reads_inline_and_heap_names() {
        let mut material = vec![0u8; 0x130];
        let short = b"materials/a";
        material[0x110..0x110 + short.len()].copy_from_slice(short);
        material[0x120..0x128].copy_from_slice(&short.len().to_le_bytes());
        material[0x128..0x130].copy_from_slice(&15usize.to_le_bytes());
        let base = material.as_ptr() as usize;
        assert_eq!(
            unsafe { material_name(base) }.as_deref(),
            Some("materials/a")
        );

        let long = b"materials/units/soldier.material".to_vec();
        material[0x110..0x118].copy_from_slice(&(long.as_ptr() as usize).to_le_bytes());
        material[0x120..0x128].copy_from_slice(&long.len().to_le_bytes());
        material[0x128..0x130].copy_from_slice(&47usize.to_le_bytes());
        assert_eq!(
            unsafe { material_name(base) }.as_deref(),
            Some("materials/units/soldier.material")
        );

        material[0x120..0x128].copy_from_slice(&0usize.to_le_bytes());
        assert_eq!(unsafe { material_name(base) }, None);
    }

    #[test]
    fn missing_parts_are_left_alone() {
        let part = vec![0usize; 16];
        assert_eq!(unsafe { dim_material(0, 1) }, 0);
        assert_eq!(unsafe { dim_material(part.as_ptr() as usize, 1) }, 0);
    }
}
