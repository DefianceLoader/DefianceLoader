//! The loaded plugins, and taking one out of the running game.
//!
//! Every plugin that initializes is recorded here: its owner number (the key
//! its hooks and services are filed under), identity, module and code ranges.
//! Plugins load from a shadow copy ([`shadow_copy`]) so the DLL in the plugins
//! directory can be replaced while the game runs.
//!
//! [`unload`] reverses a plugin's startup: its `stop`, then its hooks and patch
//! spans restored, its services withdrawn, and, once no thread is executing or
//! returning into its code or into a trampoline, relay or call stub of its
//! hooks ([`quiet`], `hooks::owned_code`), its module freed. A plugin that never
//! goes quiet stays mapped but inert, and the log says so. The caller unloads a
//! plugin's dependants first; `host` does that for a reload.
use crate::win;
use core::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// A plugin that initialized.
#[derive(Clone)]
pub struct Loaded {
    pub owner: usize,
    pub id: String,
    pub name: String,
    /// The DLL in the plugins directory, as discovered.
    pub path: PathBuf,
    /// What was loaded: a copy of `path`, or `path` itself when no copy could
    /// be made.
    pub shadow: PathBuf,
    pub module: usize,
    /// The module's executable sections, `[start, end)`.
    pub code: Vec<(usize, usize)>,
    pub stop: Option<unsafe extern "C" fn()>,
    /// The built-in feature number, or 0.
    pub feature: u32,
    /// Whether the manifest allows unloading while the game runs.
    pub reloadable: bool,
    /// `path`'s size and modification time when it was loaded.
    pub stamp: Option<(u64, std::time::SystemTime)>,
    /// Whether its manifest declares it multiplayer-safe.
    pub multiplayer_safe: bool,
}

static LOADED: Mutex<Vec<Loaded>> = Mutex::new(Vec::new());
/// Old copies unloaded with `retain`: stopped and unhooked, but mapped until
/// exit because a startup-only plugin still calls a service table of theirs.
/// Their code still runs through those tables, so they still hold the tables
/// they took from others (`services::withdraw` keeps those records) and still
/// count for multiplayer.
static RETAINED: Mutex<Vec<Loaded>> = Mutex::new(Vec::new());

/// The old copies kept mapped for a startup-only plugin.
pub fn retained() -> Vec<Loaded> {
    RETAINED.lock().unwrap_or_else(|p| p.into_inner()).clone()
}
/// Owner numbers for plugins loaded after startup; startup uses plan indices.
static NEXT_OWNER: AtomicUsize = AtomicUsize::new(1 << 16);
/// Shadow copies made so far, for unique names.
static COPIES: AtomicUsize = AtomicUsize::new(0);

/// A fresh owner number, distinct from every startup index.
pub fn next_owner() -> usize {
    NEXT_OWNER.fetch_add(1, Ordering::Relaxed)
}

pub fn record(loaded: Loaded) {
    LOADED.lock().unwrap().push(loaded);
}

/// The recorded plugins, in load order.
pub fn loaded() -> Vec<Loaded> {
    LOADED.lock().unwrap().clone()
}

pub fn forget(owner: usize) {
    LOADED
        .lock()
        .unwrap()
        .retain(|plugin| plugin.owner != owner);
}

/// A file's size and modification time.
pub fn stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

/// Remove the shadow copies of earlier runs. One still loaded by another game
/// process is locked and stays.
pub fn clean_cache(cache: &Path) {
    let Ok(entries) = std::fs::read_dir(cache) else {
        return;
    };
    for entry in entries.flatten() {
        let _ = std::fs::remove_file(entry.path());
    }
}

/// Copy `original` into `cache` under a name no loaded module has, so the
/// original can be replaced while its copy is loaded. Keeps the stem, so logs
/// and crash reports still read naturally.
pub fn shadow_copy(original: &Path, cache: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(cache)?;
    let stem = original
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "plugin".into());
    let copy = cache.join(format!(
        "{stem}.{}.{}.dll",
        std::process::id(),
        COPIES.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::copy(original, &copy)?;
    Ok(copy)
}

/// The executable sections of a loaded module, from its PE headers.
pub fn code_ranges(module: usize) -> Vec<(usize, usize)> {
    const EXECUTE: u32 = 0x2000_0000;
    let read_u32 = |at: usize| unsafe { (at as *const u32).read_unaligned() };
    let read_u16 = |at: usize| unsafe { (at as *const u16).read_unaligned() };
    if module == 0 || !win::is_readable(module, 0x40) {
        return Vec::new();
    }
    let nt = module + read_u32(module + 0x3c) as usize;
    if !win::is_readable(nt, 0x18) || read_u32(nt) != 0x4550 {
        return Vec::new();
    }
    let sections = read_u16(nt + 6) as usize;
    let optional = read_u16(nt + 20) as usize;
    let first = nt + 24 + optional;
    let mut ranges = Vec::new();
    for index in 0..sections {
        let header = first + index * 40;
        if !win::is_readable(header, 40) {
            break;
        }
        let size = read_u32(header + 8) as usize;
        let rva = read_u32(header + 12) as usize;
        if read_u32(header + 36) & EXECUTE != 0 && size != 0 {
            ranges.push((module + rva, module + rva + size));
        }
    }
    ranges
}

/// Whether no thread is in `code`: no instruction pointer inside it, and no
/// word of a live stack (`[stack pointer, stack base)`) pointing into it, which
/// is where a return into it would come from. A thread whose stack base is
/// unknown (0) counts as busy. `read` reads one stack word. Allocation-free:
/// it runs while every other thread is suspended.
pub fn quiet(
    ips: &[usize],
    stacks: &[(usize, usize)],
    code: &[(usize, usize)],
    read: impl Fn(usize) -> usize,
) -> bool {
    let inside = |value: usize| {
        code.iter()
            .any(|&(start, end)| start <= value && value < end)
    };
    if ips.iter().any(|&ip| inside(ip)) {
        return false;
    }
    for &(sp, base) in stacks {
        if base == 0 || sp == 0 || sp > base {
            return false;
        }
        let mut at = sp & !7;
        while at + 8 <= base {
            if inside(read(at)) {
                return false;
            }
            at += 8;
        }
    }
    true
}

/// Whether the calling thread is running `code`, by its return addresses.
fn on_own_stack(code: &[(usize, usize)]) -> bool {
    let mut frames = [core::ptr::null_mut::<c_void>(); 64];
    let count = unsafe {
        win::RtlCaptureStackBackTrace(
            0,
            frames.len() as u32,
            frames.as_mut_ptr(),
            core::ptr::null_mut(),
        )
    } as usize;
    frames[..count].iter().any(|&frame| {
        code.iter()
            .any(|&(s, e)| s <= frame as usize && (frame as usize) < e)
    })
}

/// Wait, briefly, until no thread is in `code`.
fn wait_quiet(code: &[(usize, usize)]) -> bool {
    for _ in 0..20 {
        let outcome = crate::threads::suspend_with_stacks(|ips, stacks| {
            // Live stacks are committed memory: `[sp, base)` of a suspended
            // thread is readable without a lock.
            quiet(ips, stacks, code, |at| unsafe { *(at as *const usize) })
        });
        if outcome == Ok(true) {
            return true;
        }
        unsafe { win::Sleep(100) };
    }
    false
}

/// Why an unload did not finish.
#[derive(Debug, PartialEq, Eq)]
pub enum UnloadError {
    /// Not a recorded plugin.
    Unknown,
    /// Its manifest does not allow it while the game runs.
    NotReloadable,
    /// Some patch spans could not be restored; everything is retained.
    Degraded(usize),
    /// Stopped and unhooked, but a thread was still in its code: the module
    /// stays mapped, inert.
    Busy,
}

impl core::fmt::Display for UnloadError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            UnloadError::Unknown => write!(f, "not loaded"),
            UnloadError::NotReloadable => write!(f, "it can only be loaded at startup"),
            UnloadError::Degraded(count) => {
                write!(f, "{count} patch span(s) could not be restored")
            }
            UnloadError::Busy => write!(
                f,
                "a thread was still running its code; it stays loaded but inactive"
            ),
        }
    }
}

/// Take a recorded plugin out of the running game. `forget_feature` tells
/// Core a built-in feature's spans are gone, before the module is freed.
/// With `retain`, the module stays mapped (stopped, unhooked, its services
/// withdrawn) instead: a plugin that can only load at startup still holds one
/// of its service tables and keeps calling it.
pub fn unload(
    owner: usize,
    forget_feature: impl FnOnce(u32),
    retain: bool,
) -> Result<Loaded, UnloadError> {
    let Some(plugin) = loaded().into_iter().find(|p| p.owner == owner) else {
        return Err(UnloadError::Unknown);
    };
    if !plugin.reloadable {
        return Err(UnloadError::NotReloadable);
    }
    // The safe point skips the calling thread; a reload on a game thread
    // (before a mission is built) must not be inside the plugin itself.
    if on_own_stack(&plugin.code) {
        return Err(UnloadError::Busy);
    }
    if let Some(stop) = plugin.stop {
        unsafe { stop() };
    }
    let (removed, failed) = crate::hooks::remove_owned_report(owner);
    if failed > 0 {
        // Code reachable from a live span must not be freed or reloaded.
        return Err(UnloadError::Degraded(failed));
    }
    if retain {
        // Its code keeps running for whoever holds its tables, calling the
        // tables it holds: those records stay, so what it calls is kept too.
        crate::services::withdraw(owner);
    } else {
        crate::services::remove(owner);
    }
    if plugin.feature != 0 {
        forget_feature(plugin.feature);
    }
    // A thread paused in one of its hooks' relays or stubs has no address of
    // the module anywhere, yet jumps into it next.
    let mut code = plugin.code.clone();
    code.extend(crate::hooks::owned_code(owner));
    crate::log::info(&format!(
        "{} ({}): stopped, {removed} hook(s) and patch span(s) restored",
        plugin.id, plugin.name
    ));
    forget(owner);
    if retain {
        RETAINED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(plugin.clone());
        return Ok(plugin);
    }
    if !wait_quiet(&code) {
        return Err(UnloadError::Busy);
    }
    crate::crash::unmap(plugin.module);
    unsafe { win::FreeLibrary(plugin.module as win::Handle) };
    if plugin.shadow != plugin.path {
        let _ = std::fs::remove_file(&plugin.shadow);
    }
    Ok(plugin)
}

/// The recorded plugins that hold a service table from `id`, directly or
/// through others, deepest first: the order to unload them in before `id`.
/// `consumers` gives the owners holding a table from a provider ID. A plugin
/// that only depends on `id` for its start order holds nothing of it and
/// stays loaded.
pub fn dependants(
    id: &str,
    plugins: &[Loaded],
    consumers: &impl Fn(&str) -> Vec<usize>,
) -> Vec<Loaded> {
    let mut out: Vec<Loaded> = Vec::new();
    fn visit(
        id: &str,
        plugins: &[Loaded],
        consumers: &impl Fn(&str) -> Vec<usize>,
        out: &mut Vec<Loaded>,
    ) {
        let holders = consumers(id);
        for plugin in plugins {
            if holders.contains(&plugin.owner)
                && !plugin.id.eq_ignore_ascii_case(id)
                && !out.iter().any(|o| o.owner == plugin.owner)
            {
                visit(&plugin.id, plugins, consumers, out);
                if !out.iter().any(|o| o.owner == plugin.owner) {
                    out.push(plugin.clone());
                }
            }
        }
    }
    visit(id, plugins, consumers, &mut out);
    out
}

/// Call a loaded module's optional export `name` with one `u32`, if present.
pub fn call_export(module: usize, name: &core::ffi::CStr, value: u32) -> bool {
    let symbol = unsafe { win::GetProcAddress(module as win::Handle, name.as_ptr().cast()) };
    if symbol.is_null() {
        return false;
    }
    let function: unsafe extern "C" fn(u32) =
        unsafe { core::mem::transmute::<*mut c_void, unsafe extern "C" fn(u32)>(symbol) };
    unsafe { function(value) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_in_or_returning_into_the_code_is_busy() {
        let code = [(0x1000, 0x2000)];
        let stack = [0x10usize, 0x1500, 0x20, 0x3000];
        let base = stack.as_ptr() as usize;
        let read = |at: usize| unsafe { *(at as *const usize) };
        let top = base + 4 * 8;
        // An instruction pointer inside.
        assert!(!quiet(&[0x1800], &[], &code, read));
        // A return address on the live stack.
        assert!(!quiet(&[0x5000], &[(base, top)], &code, read));
        // The same word below the stack pointer is dead stack: quiet.
        assert!(quiet(&[0x5000], &[(base + 16, top)], &code, read));
        // Nothing inside at all.
        assert!(quiet(
            &[0x5000],
            &[(base + 16, top)],
            &[(0x8000, 0x9000)],
            read
        ));
        // An unreadable stack counts as busy.
        assert!(!quiet(&[0x5000], &[(base, 0)], &code, read));
    }

    unsafe extern "system" fn idle(_: *mut c_void) -> u32 {
        loop {
            unsafe { win::Sleep(1000) };
        }
    }

    #[test]
    fn a_thread_paused_on_a_hook_stub_keeps_its_owner_loaded() {
        // A plugin (owner 0x7707) whose one hook redirects `call <ret>` on a
        // code page.
        let owner = 0x7707;
        let page = crate::code::alloc_near(
            idle as unsafe extern "system" fn(*mut c_void) -> u32 as usize,
            0x1000,
        )
        .unwrap() as *mut u8;
        let code = [0xe8u8, 0xfb, 0x00, 0x00, 0x00]; // call page+0x100
        unsafe {
            core::ptr::copy_nonoverlapping(code.as_ptr(), page, code.len());
            *page.add(0x100) = 0xc3;
        }
        crate::hooks::begin_plugin(owner, "stub-test");
        unsafe { crate::hooks::install_call(page.cast(), idle as *mut c_void) }.unwrap();
        crate::hooks::end_plugin();
        let owned = crate::hooks::owned_code(owner);
        assert_eq!(owned.len(), 1);
        record(plugin(owner, "stub-test"));
        // A thread paused on the stub's jump: it enters the plugin next, with
        // no address of the plugin anywhere.
        let thread = unsafe {
            win::CreateThread(
                core::ptr::null_mut(),
                0,
                idle,
                core::ptr::null_mut(),
                4, // CREATE_SUSPENDED
                core::ptr::null_mut(),
            )
        };
        assert!(!thread.is_null());
        let mut context = crate::crash::Context([0; 1232]);
        context.0[48..52].copy_from_slice(&0x0010_0001u32.to_le_bytes()); // CONTEXT_CONTROL
        assert_ne!(
            unsafe { win::GetThreadContext(thread, context.0.as_mut_ptr().cast()) },
            0
        );
        context.0[0xf8..0x100].copy_from_slice(&(owned[0].0 as u64).to_le_bytes());
        assert_ne!(
            unsafe { win::SetThreadContext(thread, context.0.as_ptr().cast()) },
            0
        );
        // Unhooked, but not freed: the removed hook's stub stays charged to it.
        assert_eq!(unload(owner, |_| {}, false).err(), Some(UnloadError::Busy));
        assert_eq!(crate::hooks::owned_code(owner), owned);
        unsafe {
            win::TerminateThread(thread, 0);
            win::CloseHandle(thread);
        }
    }

    #[test]
    fn this_module_has_code_ranges() {
        let here = code_ranges as fn(usize) -> Vec<(usize, usize)> as usize;
        let mut module = core::ptr::null_mut();
        assert_ne!(
            unsafe { win::GetModuleHandleExW(0x4 | 0x2, here as *const u16, &mut module) },
            0
        );
        let ranges = code_ranges(module as usize);
        assert!(
            ranges.iter().any(|&(s, e)| s <= here && here < e),
            "{ranges:x?}"
        );
        assert!(code_ranges(0).is_empty());
    }

    #[test]
    fn shadow_copies_are_unique_and_keep_the_stem() {
        let dir = std::env::temp_dir().join(format!("defiance-shadow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("demo.dll");
        std::fs::write(&original, b"one").unwrap();
        let cache = dir.join("cache");
        let a = shadow_copy(&original, &cache).unwrap();
        let b = shadow_copy(&original, &cache).unwrap();
        assert_ne!(a, b);
        assert!(a
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("demo."));
        assert_eq!(std::fs::read(&a).unwrap(), b"one");
        // The original stays replaceable.
        std::fs::write(&original, b"two").unwrap();
        clean_cache(&cache);
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn plugin(owner: usize, id: &str) -> Loaded {
        Loaded {
            owner,
            id: id.into(),
            name: id.into(),
            path: PathBuf::new(),
            shadow: PathBuf::new(),
            module: 0,
            code: Vec::new(),
            stop: None,
            feature: 0,
            reloadable: true,
            stamp: None,
            multiplayer_safe: true,
        }
    }

    #[test]
    fn dependants_are_the_service_holders_deepest_first() {
        let plugins = [
            plugin(1, "core"),
            plugin(2, "selection"),
            plugin(3, "regroup"),
            plugin(4, "ammo-menu"),
        ];
        // regroup holds selection's table and core's; selection holds core's;
        // ammo-menu depends on selection only for its start order.
        let consumers = |provider: &str| match provider {
            "core" => vec![2, 3],
            "selection" => vec![3],
            _ => vec![],
        };
        let ids = |id| -> Vec<String> {
            dependants(id, &plugins, &consumers)
                .into_iter()
                .map(|p| p.id)
                .collect()
        };
        assert_eq!(ids("selection"), ["regroup"]);
        assert_eq!(ids("core"), ["regroup", "selection"]);
        assert!(ids("ammo-menu").is_empty());
    }
}
