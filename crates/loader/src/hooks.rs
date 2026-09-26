//! The detour engine a plugin uses through `Api::hook`.
//!
//! This is the injector's mechanism, generalized. The injector knew the
//! displaced bytes from its assembled descriptor; a plugin only knows the
//! address, so it passes how many bytes it is taking (a whole number of
//! instructions, at least five) and this copies those bytes into a trampoline,
//! appends a jump back to the instruction after them, and writes a jump from
//! the target to the plugin's detour.
//!
//! A trampoline is allocated near the target, inside the 2GB a rel32 can
//! reach. The jump back is absolute (`FF 25`, an indirect jump through a
//! pointer) so it works no matter how far the target moved; the jump from the
//! target to the detour uses a rel32 when the detour is in reach (it usually
//! is, if the plugin DLL is loaded near the game) and an absolute jump when it
//! is not and fourteen bytes are available. Shorter entry spans use a rel32
//! to a nearby relay containing an absolute jump to the detour. The relay
//! shares the trampoline allocation, after its unconditional jump back.
//!
//! A hook is owned by the plugin whose `init` installed it. Plugin loading is
//! sequential, so a single "who is initialising" slot is enough: `install`
//! charges the hook to whoever is current. That is what lets a plugin's `stop`
//! take its hooks out again, and what lets a conflict say who got there first.
//! Two plugins cannot hook one target; the second is refused rather than
//! silently chaining, since a chain changes what the first plugin's trampoline
//! means.

use crate::code::{alloc_near, flush, publish, write, CommitError, PendingCode};
#[cfg(test)]
use crate::win;
use core::cell::RefCell;
use core::ffi::c_void;
use std::sync::Mutex;

const ABSOLUTE_JUMP: usize = 14;
const MIN_DISPLACED: usize = 5;

#[derive(PartialEq)]
enum Kind {
    /// a function entry, taken over for every caller
    Entry,
    Bytes,
    /// one direct call, redirected; the function itself is untouched
    Call,
}

impl Kind {
    fn label(&self) -> &'static str {
        match self {
            Kind::Entry => "function",
            Kind::Bytes => "byte patch",
            Kind::Call => "call site",
        }
    }
}

struct Installed {
    target: usize,
    kind: Kind,
    /// the bytes to put back at `target`
    original: Vec<u8>,
    owner: usize,
    owner_name: String,
    /// The executable allocation serving the hook, `[start, end)`: the
    /// trampoline with its relay, or a call stub. Empty for a byte patch.
    code: (usize, usize),
}

static INSTALLED: Mutex<Vec<Installed>> = Mutex::new(Vec::new());
/// The allocations of removed hooks, with their owners: `(owner, start, end)`.
/// They stay mapped, since a thread may still be in one, and a relay or stub
/// still jumps into its owner's code, so unloading that owner must wait until
/// no thread is in them either ([`owned_code`]).
static RETIRED: Mutex<Vec<(usize, usize, usize)>> = Mutex::new(Vec::new());

/// Every executable allocation `owner`'s hooks use or used: the trampolines,
/// relays and call stubs of its installed and removed hooks. A relay or stub
/// jumps into the owner's code, so a thread inside one reaches that code next
/// even with no address of it on its stack.
pub fn owned_code(owner: usize) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = INSTALLED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|hook| hook.owner == owner && hook.code.1 > hook.code.0)
        .map(|hook| hook.code)
        .collect();
    out.extend(
        RETIRED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|&&(o, _, _)| o == owner)
            .map(|&(_, start, end)| (start, end)),
    );
    out
}

/// The plugin whose `init` is running on this thread.
#[derive(Clone)]
struct Owner {
    index: usize,
    name: String,
}

thread_local! {
    /// Set by the host around each plugin's `init`, on the thread running it.
    /// Thread-local, not process-global, so a background call cannot inherit
    /// another plugin's ownership or collide with plugin zero.
    static CURRENT: RefCell<Option<Owner>> = const { RefCell::new(None) };
}

/// Mark `name` as the plugin installing hooks on this thread now.
pub fn begin_plugin(owner: usize, name: &str) {
    CURRENT.with(|slot| {
        *slot.borrow_mut() = Some(Owner {
            index: owner,
            name: name.to_string(),
        })
    });
}

/// Clear the current plugin once its `init` has returned.
pub fn end_plugin() {
    CURRENT.with(|slot| *slot.borrow_mut() = None);
}

fn overlaps(hook: &Installed, target: usize, length: usize) -> bool {
    target < hook.target.saturating_add(hook.original.len())
        && hook.target < target.saturating_add(length)
}

/// The plugin charged with a new hook, or why the call is not supported.
///
/// Hook installation is only meaningful inside a plugin's `init`: the host
/// charges the hook to that plugin so its `stop` can take it back out, and a
/// conflict can name who got there first. A call from a background thread or
/// outside `init` is refused rather than silently charged to plugin zero.
fn current_owner() -> Result<Owner, String> {
    CURRENT
        .with(|slot| slot.borrow().clone())
        .ok_or_else(|| "hook installation is only allowed during a plugin's init".to_string())
}

/// A 14-byte jump to an absolute address: `FF 25 00000000 <addr>`.
fn absolute_jump(to: usize) -> Vec<u8> {
    let mut bytes = vec![0xffu8, 0x25, 0x00, 0x00, 0x00, 0x00];
    bytes.extend_from_slice(&(to as u64).to_le_bytes());
    bytes
}

/// Check the actual displacement, including the five-byte source instruction.
fn relative32(from: usize, to: usize) -> Option<i32> {
    i32::try_from(to as i128 - (from as i128 + MIN_DISPLACED as i128)).ok()
}

/// A jump from `from` to `to`, padded with nops to `displaced` bytes. Uses a
/// rel32 where one reaches, an absolute jump otherwise.
fn jump(from: usize, to: usize, displaced: usize) -> Result<Vec<u8>, String> {
    if let Some(relative) = relative32(from, to) {
        let mut bytes = vec![0xe9u8];
        bytes.extend_from_slice(&relative.to_le_bytes());
        bytes.resize(displaced, 0x90);
        return Ok(bytes);
    }
    if displaced >= ABSOLUTE_JUMP {
        let mut bytes = absolute_jump(to);
        bytes.resize(displaced, 0x90);
        return Ok(bytes);
    }
    Err(format!(
        "{to:#x} is out of reach of {from:#x} and only {displaced} bytes may be displaced"
    ))
}

unsafe fn read(address: *const u8, length: usize) -> Vec<u8> {
    unsafe { core::slice::from_raw_parts(address, length).to_vec() }
}

/// The longest x86-64 instruction is fifteen bytes, so sixteen are enough to
/// cover at least one whole instruction and the five bytes a jump needs.
const INSTRUCTION_WINDOW: usize = 16;

/// Redirect `target` to `detour`, working out how many bytes to displace by
/// decoding whole instructions until at least a jump's worth is covered. A site
/// whose bytes this decoder cannot read is refused, not guessed at.
///
/// # Safety
/// `target` must be the start of an instruction.
#[cfg(test)]
pub unsafe fn install_auto(
    target: *mut c_void,
    detour: *mut c_void,
) -> Result<*mut c_void, String> {
    unsafe { install_auto_out(target, detour, core::ptr::null_mut()) }
}

/// As `install_auto`, but stores the trampoline in `original` *before* the
/// branch becomes visible, so a detour that runs immediately already has it.
///
/// # Safety
/// `target` must be the start of an instruction; `original` may be null.
pub unsafe fn install_auto_out(
    target: *mut c_void,
    detour: *mut c_void,
    original: *mut *mut c_void,
) -> Result<*mut c_void, String> {
    let head = unsafe { read(target as *const u8, INSTRUCTION_WINDOW) };
    let displaced = defiance_core::decode::displaced(&head, MIN_DISPLACED)?;
    unsafe { install_out(target, detour, displaced, original) }
}

/// Redirect `target` to `detour`, displacing exactly `displaced` bytes. On
/// success returns the trampoline address the caller uses for the stock
/// behaviour. `displaced` must cover whole instructions at `target` and be at
/// least five bytes. Both entry APIs reject instructions requiring relocation
/// and counts that end inside an instruction, before allocating or patching.
///
/// # Safety
/// `target` must be the start of an instruction, and `displaced` must not cut
/// an instruction in half: the trampoline executes those bytes out of context.
#[cfg(test)]
pub unsafe fn install(
    target: *mut c_void,
    detour: *mut c_void,
    displaced: usize,
) -> Result<*mut c_void, String> {
    unsafe { install_out(target, detour, displaced, core::ptr::null_mut()) }
}

/// As `install`, but stores the trampoline in `original` before publication.
///
/// # Safety
/// `target` must be the start of an instruction; `original` may be null.
pub unsafe fn install_out(
    target: *mut c_void,
    detour: *mut c_void,
    displaced: usize,
    original: *mut *mut c_void,
) -> Result<*mut c_void, String> {
    // Publish at a safe point, refusing a branch over a span a thread is
    // executing. `original` is set before the branch becomes visible.
    unsafe {
        install_entry(
            target,
            detour,
            displaced,
            original,
            alloc_near,
            |target, patch| publish(target, patch, displaced),
        )
    }
}

// Production uses the real allocator and writer. Tests force failures at those
// boundaries without exhausting address space or racing page protection. A
// `BeforeWrite` commit error means no target bytes changed; an `AfterWrite`
// error means the target is patched, so the hook is completed and owned.
unsafe fn install_entry(
    target: *mut c_void,
    detour: *mut c_void,
    displaced: usize,
    original_out: *mut *mut c_void,
    allocate: impl FnOnce(usize, usize) -> Result<usize, String>,
    commit: impl FnOnce(*mut u8, &[u8]) -> Result<(), CommitError>,
) -> Result<*mut c_void, String> {
    if displaced < MIN_DISPLACED {
        return Err(format!("{displaced} bytes is less than a jump"));
    }
    let owner = current_owner()?;
    let mut installed = INSTALLED.lock().unwrap();
    if let Some(existing) = installed
        .iter()
        .find(|hook| overlaps(hook, target as usize, displaced))
    {
        return Err(format!(
            "{target:p} is already hooked as a {} by {} (plugin {})",
            existing.kind.label(),
            existing.owner_name,
            existing.owner
        ));
    }

    let original = unsafe { read(target as *const u8, displaced) };
    defiance_core::decode::validate_copy(&original)?;
    let needs_relay =
        displaced < ABSOLUTE_JUMP && relative32(target as usize, detour as usize).is_none();
    let trampoline_size = displaced
        .checked_add(ABSOLUTE_JUMP)
        .ok_or("trampoline size overflow")?;
    let allocation_size = trampoline_size
        .checked_add(if needs_relay { ABSOLUTE_JUMP } else { 0 })
        .ok_or("entry allocation size overflow")?;
    let allocation = PendingCode(allocate(target as usize, allocation_size)?);
    let trampoline = allocation.0;
    let destination = if needs_relay {
        let relay = trampoline
            .checked_add(trampoline_size)
            .ok_or("relay address overflow")?;
        relative32(target as usize, relay).ok_or("the entry relay is out of rel32 reach")?;
        let code = absolute_jump(detour as usize);
        unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), relay as *mut u8, code.len()) };
        relay
    } else {
        detour as usize
    };

    // the displaced instructions, then a jump back to just past them
    unsafe { core::ptr::copy_nonoverlapping(original.as_ptr(), trampoline as *mut u8, displaced) };
    let back = absolute_jump(target as usize + displaced);
    unsafe {
        core::ptr::copy_nonoverlapping(
            back.as_ptr(),
            (trampoline as *mut u8).add(displaced),
            back.len(),
        )
    };

    let patch = jump(target as usize, destination, displaced)?;
    // Publish neither the entry jump nor the returned original until all new
    // executable bytes (including the relay) have been flushed. Store the
    // original pointer before the branch is visible, so a detour that fires
    // immediately already has it.
    flush(trampoline, allocation_size)?;
    if !original_out.is_null() {
        unsafe { &*original_out.cast::<core::sync::atomic::AtomicPtr<c_void>>() }.store(
            trampoline as *mut c_void,
            core::sync::atomic::Ordering::Release,
        );
    }
    match commit(target as *mut u8, &patch) {
        Ok(()) => {}
        // Nothing changed: the allocation is released by the drop below.
        Err(CommitError::BeforeWrite(error)) => {
            if !original_out.is_null() {
                unsafe { &*original_out.cast::<core::sync::atomic::AtomicPtr<c_void>>() }
                    .store(core::ptr::null_mut(), core::sync::atomic::Ordering::Release);
            }
            return Err(error);
        }
        // The target now branches to the detour. Finish the installation and
        // keep the trampoline: freeing it would strand a live path.
        Err(error @ CommitError::AfterWrite(_)) => {
            crate::log::error(&format!("hook at {target:p}: {error}"));
        }
    }

    record_mapping(
        target as usize,
        displaced,
        trampoline,
        allocation_size,
        &owner.name,
    );
    installed.push(Installed {
        target: target as usize,
        kind: Kind::Entry,
        original,
        owner: owner.index,
        owner_name: owner.name,
        code: (trampoline, trampoline + allocation_size),
    });
    allocation.forget(); // the installed hook owns the entire block
    Ok(trampoline as *mut c_void)
}

/// Redirect the direct call at `site` to `detour`, returning the address the
/// call reached before, which the caller calls for the stock behaviour. One
/// call site only: the function's other callers are untouched, which is what
/// distinguishes this from `install`.
///
/// The call is a rel32 and the plugin is not near the module, so a stub holding
/// an absolute jump is placed within reach and the call is aimed at that.
///
/// # Safety
/// `site` must be the start of a direct `call`.
#[cfg(test)]
pub unsafe fn install_call(site: *mut c_void, detour: *mut c_void) -> Result<usize, String> {
    unsafe { install_call_out(site, detour, core::ptr::null_mut()) }
}

/// As `install_call`, but stores the address the call reached in `original`
/// before the call is redirected.
///
/// # Safety
/// `site` must be the start of a direct `call`; `original` may be null.
pub unsafe fn install_call_out(
    site: *mut c_void,
    detour: *mut c_void,
    original_out: *mut *mut c_void,
) -> Result<usize, String> {
    let at = site as *mut u8;
    let original = unsafe { read(at as *const u8, MIN_DISPLACED) };
    let owner = current_owner()?;
    let mut installed = INSTALLED.lock().unwrap();
    if let Some(existing) = installed
        .iter()
        .find(|hook| overlaps(hook, site as usize, MIN_DISPLACED))
    {
        return Err(format!(
            "{site:p} is already hooked as a {} by {} (plugin {})",
            existing.kind.label(),
            existing.owner_name,
            existing.owner
        ));
    }
    if original[0] != 0xe8 {
        return Err(format!("{site:p} is not a direct call"));
    }
    let rel = i32::from_le_bytes([original[1], original[2], original[3], original[4]]) as isize;
    let reached = (site as isize + MIN_DISPLACED as isize + rel) as usize;

    let stub_size = ABSOLUTE_JUMP;
    let allocation = PendingCode(alloc_near(site as usize, stub_size)?);
    let stub = allocation.address();
    let jump = absolute_jump(detour as usize);
    unsafe { core::ptr::copy_nonoverlapping(jump.as_ptr(), stub as *mut u8, jump.len()) };
    // The call site is about to branch here: flush the stub first, unlike the
    // entry path where the trampoline is flushed as a whole. A failure leaves
    // the allocation unpublished and the drop below releases it.
    flush(stub, stub_size)?;

    let delta = stub as isize - (site as isize + MIN_DISPLACED as isize);
    if !(i32::MIN as isize..=i32::MAX as isize).contains(&delta) {
        return Err(format!("the stub for {site:p} is out of rel32 reach"));
    }
    let mut patch = vec![0xe8u8];
    patch.extend_from_slice(&(delta as i32).to_le_bytes());
    // The stock target is known before publication, so store it first.
    if !original_out.is_null() {
        unsafe { &*original_out.cast::<core::sync::atomic::AtomicPtr<c_void>>() }.store(
            reached as *mut c_void,
            core::sync::atomic::Ordering::Release,
        );
    }
    // A call has no instruction boundary inside its five bytes, so any
    // suspended thread can only be at the start; publishing is safe.
    match publish(at, &patch, MIN_DISPLACED) {
        Ok(()) => {}
        Err(CommitError::BeforeWrite(error)) => {
            if !original_out.is_null() {
                unsafe { &*original_out.cast::<core::sync::atomic::AtomicPtr<c_void>>() }
                    .store(core::ptr::null_mut(), core::sync::atomic::Ordering::Release);
            }
            return Err(error);
        }
        Err(error @ CommitError::AfterWrite(_)) => {
            crate::log::error(&format!("call hook at {site:p}: {error}"));
        }
    }

    record_mapping(site as usize, MIN_DISPLACED, stub, stub_size, &owner.name);
    installed.push(Installed {
        target: site as usize,
        kind: Kind::Call,
        original,
        owner: owner.index,
        owner_name: owner.name,
        code: (stub, stub + stub_size),
    });
    allocation.forget();
    Ok(reached)
}

/// An assembled patch owns its whole span in the same registry as hooks.
pub unsafe fn patch_bytes(target: *mut c_void, before: &[u8], after: &[u8]) -> Result<(), String> {
    if before.is_empty() || before.len() != after.len() {
        return Err("invalid patch length".into());
    }
    let owner = current_owner()?;
    let mut installed = INSTALLED.lock().unwrap();
    if let Some(existing) = installed
        .iter()
        .find(|hook| overlaps(hook, target as usize, before.len()))
    {
        return Err(format!("byte patch overlaps {}", existing.owner_name));
    }
    if unsafe { read(target.cast(), before.len()) } != before {
        return Err("byte patch expected bytes mismatch".into());
    }
    match publish(target.cast(), after, after.len()) {
        Ok(()) => {}
        Err(CommitError::BeforeWrite(error)) => return Err(error),
        Err(error @ CommitError::AfterWrite(_)) => {
            crate::log::error(&format!("byte patch at {target:p}: {error}"));
        }
    }
    record_mapping(target as usize, before.len(), 0, 0, &owner.name);
    installed.push(Installed {
        target: target as usize,
        kind: Kind::Bytes,
        original: before.to_vec(),
        owner: owner.index,
        owner_name: owner.name,
        code: (0, 0),
    });
    Ok(())
}

/// Restore the site. Published executable storage lives until process exit.
/// A detour outside the trampoline may still hold its address for a later call.
pub fn remove(target: *mut c_void) -> Result<(), String> {
    let mut installed = INSTALLED.lock().unwrap();
    let Some(index) = installed
        .iter()
        .position(|hook| hook.target == target as usize)
    else {
        return Err(format!("{target:p} was not hooked"));
    };
    // Keep ownership and the live trampoline if restoring the site fails.
    match write(target as *mut u8, &installed[index].original) {
        Ok(()) => {}
        Err(CommitError::BeforeWrite(error)) => return Err(error),
        // The original bytes are back even if a follow-up failed, so the
        // published storage must still be retained for in-flight callers.
        Err(error @ CommitError::AfterWrite(_)) => {
            crate::log::error(&format!("unhook at {target:p}: {error}"));
        }
    }
    let hook = installed.remove(index);
    crate::crash::unmap(hook.target);
    // Keep the stub allocation and crash mapping: in-flight calls can return
    // here even when no suspended instruction pointer currently points at it.
    // Its owner's unload still waits for threads inside it (`owned_code`).
    if hook.code.1 > hook.code.0 {
        RETIRED.lock().unwrap_or_else(|p| p.into_inner()).push((
            hook.owner,
            hook.code.0,
            hook.code.1,
        ));
    }
    Ok(())
}

/// Remove every hook a plugin installed, for its `stop`. Returns how many were
/// taken out. A span whose restore failed keeps its ownership and trampoline;
/// use `remove_owned_report` when that distinction matters. Used by the
/// in-process test host; the shipping loader calls `remove_owned_report`.
#[cfg(any(test, feature = "test-host"))]
pub fn remove_owned(owner: usize) -> usize {
    remove_owned_report(owner).0
}

/// As `remove_owned`, but also reports how many spans could not be restored.
/// The host must not announce a clean rollback while a restored hook's code is
/// still reachable, so a non-zero second value degrades startup.
pub fn remove_owned_report(owner: usize) -> (usize, usize) {
    let targets: Vec<usize> = INSTALLED
        .lock()
        .unwrap()
        .iter()
        .filter(|hook| hook.owner == owner)
        .map(|hook| hook.target)
        .collect();
    let mut removed = 0;
    let mut failed = 0;
    for target in targets {
        match remove(target as *mut c_void) {
            Ok(()) => removed += 1,
            Err(e) => {
                failed += 1;
                crate::log::error(&format!("could not restore {target:#x}: {e}"));
            }
        }
    }
    (removed, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The installed hooks and the current plugin are process-wide, so the
    /// tests that touch them take turns.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    unsafe extern "system" fn two() -> u32 {
        2
    }

    unsafe extern "system" fn three() -> u32 {
        3
    }

    fn code_page() -> *mut u8 {
        let page = alloc_near(two as *const () as usize, 0x1000).unwrap() as *mut u8;
        // `mov eax, 1; ret`
        unsafe {
            core::slice::from_raw_parts_mut(page, 6)
                .copy_from_slice(&[0xb8, 0x01, 0x00, 0x00, 0x00, 0xc3]);
        }
        page
    }

    unsafe fn as_call(page: *mut u8) -> unsafe extern "system" fn() -> u32 {
        unsafe { core::mem::transmute::<*mut u8, unsafe extern "system" fn() -> u32>(page) }
    }

    /// Run `f` as if a plugin's `init` were on this thread. Hook installation is
    /// only supported inside that window; most tests just need a valid owner.
    fn as_plugin<T>(owner: usize, f: impl FnOnce() -> T) -> T {
        begin_plugin(owner, "test plugin");
        let result = f();
        end_plugin();
        result
    }

    #[test]
    fn a_hook_runs_the_detour_and_calls_the_original() {
        let _guard = TEST_LOCK.lock().unwrap();
        let page = code_page();
        let call = unsafe { as_call(page) };
        assert_eq!(unsafe { call() }, 1);

        let trampoline = as_plugin(0, || unsafe {
            install(page as *mut c_void, two as *mut c_void, 6)
        })
        .unwrap();
        assert_eq!(unsafe { call() }, 2);
        let stock = unsafe {
            core::mem::transmute::<*mut c_void, unsafe extern "system" fn() -> u32>(trampoline)
        };
        assert_eq!(unsafe { stock() }, 1);

        remove(page as *mut c_void).unwrap();
        assert_eq!(unsafe { call() }, 1);
        unsafe { win::VirtualFree(page as *mut c_void, 0, win::MEM_RELEASE) };
    }

    #[test]
    fn an_auto_hook_decodes_its_own_displacement() {
        let _guard = TEST_LOCK.lock().unwrap();
        let page = code_page();
        let call = unsafe { as_call(page) };
        assert_eq!(unsafe { call() }, 1);
        let trampoline = as_plugin(0, || unsafe {
            install_auto(page as *mut c_void, two as *mut c_void)
        })
        .unwrap();
        assert_eq!(unsafe { call() }, 2);
        let stock = unsafe {
            core::mem::transmute::<*mut c_void, unsafe extern "system" fn() -> u32>(trampoline)
        };
        assert_eq!(unsafe { stock() }, 1);
        remove(page as *mut c_void).unwrap();
        unsafe { win::VirtualFree(page as *mut c_void, 0, win::MEM_RELEASE) };
    }

    #[test]
    fn a_call_site_can_be_redirected_to_another_function() {
        let _guard = TEST_LOCK.lock().unwrap();
        // `call two; ret`, in executable memory near `two`
        let page = alloc_near(two as *const () as usize, 0x1000).unwrap() as *mut u8;
        let rel = (two as *const () as usize as isize - (page as usize + 5) as isize) as i32;
        let mut code = vec![0xe8u8];
        code.extend_from_slice(&rel.to_le_bytes());
        code.push(0xc3);
        unsafe { core::slice::from_raw_parts_mut(page, code.len()).copy_from_slice(&code) };
        let call = unsafe { as_call(page) };
        assert_eq!(unsafe { call() }, 2);

        // the function's entry is untouched, only this one call
        let reached = as_plugin(0, || unsafe {
            install_call(page as *mut c_void, three as *mut c_void)
        })
        .unwrap();
        assert_eq!(reached, two as *const () as usize);
        assert_eq!(unsafe { call() }, 3);

        remove(page as *mut c_void).unwrap();
        assert_eq!(unsafe { call() }, 2);
        unsafe { win::VirtualFree(page as *mut c_void, 0, win::MEM_RELEASE) };
    }

    #[test]
    fn a_hook_is_owned_and_a_second_plugin_is_refused() {
        let _guard = TEST_LOCK.lock().unwrap();
        let page = code_page();
        begin_plugin(1, "first");
        unsafe { install_auto(page as *mut c_void, two as *mut c_void) }.unwrap();
        end_plugin();

        begin_plugin(2, "second");
        let error = unsafe { install_auto(page as *mut c_void, two as *mut c_void) }.unwrap_err();
        end_plugin();
        assert!(
            error.contains("first"),
            "the conflict should name the owner: {error}"
        );

        // the owner's stop takes its hook out, and the site is stock again
        assert_eq!(remove_owned(1), 1);
        let call = unsafe { as_call(page) };
        assert_eq!(unsafe { call() }, 1);
        unsafe { win::VirtualFree(page as *mut c_void, 0, win::MEM_RELEASE) };
    }

    #[test]
    fn hook_installation_outside_init_is_refused() {
        let _guard = TEST_LOCK.lock().unwrap();
        let page = code_page();
        let original = unsafe { read(page, 6) };
        // No plugin `init` is running: ownership cannot be invented.
        for error in [
            unsafe { install_auto(page.cast(), two as *mut c_void) }.unwrap_err(),
            unsafe { install(page.cast(), two as *mut c_void, 6) }.unwrap_err(),
            unsafe { install_call(page.cast(), two as *mut c_void) }.unwrap_err(),
        ] {
            assert!(error.contains("init"), "{error}");
        }
        assert_eq!(unsafe { read(page, 6) }, original);
        assert!(!INSTALLED
            .lock()
            .unwrap()
            .iter()
            .any(|hook| hook.target == page as usize));
        unsafe { win::VirtualFree(page.cast(), 0, win::MEM_RELEASE) };
    }

    #[test]
    fn hook_ownership_does_not_leak_across_threads() {
        let _guard = TEST_LOCK.lock().unwrap();
        let page = code_page();
        begin_plugin(7, "main thread");
        let address = page as usize;
        // A background thread must not inherit the owner set on this thread.
        let error = std::thread::spawn(move || {
            unsafe { install_auto(address as *mut c_void, three as *mut c_void) }.unwrap_err()
        })
        .join()
        .unwrap();
        end_plugin();
        assert!(
            error.contains("init"),
            "a background thread must not inherit the owner: {error}"
        );

        let _trampoline = as_plugin(7, || unsafe {
            install_auto(page.cast(), two as *mut c_void)
        })
        .unwrap();
        assert_eq!(unsafe { as_call(page)() }, 2);
        assert_eq!(remove_owned(7), 1);
        assert_eq!(unsafe { as_call(page)() }, 1);
        unsafe { win::VirtualFree(page.cast(), 0, win::MEM_RELEASE) };
    }

    #[test]
    fn unsafe_entry_spans_are_refused_without_changing_memory() {
        let _guard = TEST_LOCK.lock().unwrap();
        let page = code_page();
        for bytes in [
            &[0xe8, 0, 0, 0, 0][..],
            &[0x48, 0x8b, 0x05, 0, 0, 0, 0][..],
            &[0x90, 0xb8, 1, 0, 0][..], // cut mov immediate
        ] {
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), page, bytes.len()) };
            assert!(as_plugin(0, || unsafe {
                install(page.cast(), two as *mut c_void, bytes.len())
            })
            .is_err());
            assert_eq!(unsafe { read(page, bytes.len()) }, bytes);
            assert!(!INSTALLED
                .lock()
                .unwrap()
                .iter()
                .any(|hook| hook.target == page as usize));
        }
        unsafe { win::VirtualFree(page.cast(), 0, win::MEM_RELEASE) };
    }

    #[test]
    fn overlapping_entry_and_call_hooks_are_refused() {
        let _guard = TEST_LOCK.lock().unwrap();
        let page = code_page();
        // Put an E8 in the immediate, so the call API sees a direct-call byte
        // even though its span would overlap an already owned entry span.
        let bytes = [0xb8, 0xe8, 0, 0, 0, 0xc3];
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), page, bytes.len()) };
        begin_plugin(41, "span-owner");
        unsafe { install(page.cast(), two as *mut c_void, 6) }.unwrap();
        end_plugin();
        let error = as_plugin(42, || unsafe {
            install(page.add(2).cast(), three as *mut c_void, 5)
        })
        .unwrap_err();
        assert!(error.contains("span-owner"), "{error}");
        // Check conflicts before decoding the current call opcode: the first
        // hook replaced it, but the ownership must still be reported.
        let error = as_plugin(42, || unsafe {
            install_call(page.add(1).cast(), three as *mut c_void)
        })
        .unwrap_err();
        assert!(error.contains("span-owner"), "{error}");
        assert_eq!(remove_owned(41), 1);
        assert_eq!(unsafe { read(page, bytes.len()) }, bytes);
        unsafe { win::VirtualFree(page.cast(), 0, win::MEM_RELEASE) };
    }

    #[test]
    fn removing_a_hook_retains_its_published_trampoline() {
        let _guard = TEST_LOCK.lock().unwrap();
        let page = code_page();
        let trampoline = as_plugin(0, || unsafe {
            install_auto(page.cast(), two as *mut c_void)
        })
        .unwrap();
        remove(page.cast()).unwrap();
        let mut info: win::MemoryBasicInformation = unsafe { core::mem::zeroed() };
        assert_ne!(
            unsafe { win::VirtualQuery(trampoline, &mut info, core::mem::size_of_val(&info)) },
            0
        );
        assert_eq!(info.state, win::MEM_COMMIT); // retained for in-flight calls
        unsafe { win::VirtualFree(page.cast(), 0, win::MEM_RELEASE) };
    }

    #[test]
    fn failed_restore_keeps_hook_ownership_and_trampoline() {
        let _guard = TEST_LOCK.lock().unwrap();
        let page = code_page();
        let trampoline = as_plugin(0, || unsafe {
            install_auto(page.cast(), two as *mut c_void)
        })
        .unwrap();
        // Decommit the target but keep its reservation, so no other test can
        // reuse the address while VirtualProtect fails against the page.
        assert_ne!(
            unsafe { win::VirtualFree(page.cast(), 0x1000, win::MEM_DECOMMIT) },
            0
        );
        assert!(remove(page.cast()).is_err());
        let mut installed = INSTALLED.lock().unwrap();
        let index = installed
            .iter()
            .position(|hook| hook.target == page as usize)
            .unwrap();
        assert!(win::is_executable(trampoline as usize));
        // Only the test can discard this record: its synthetic target is gone.
        installed.remove(index);
        drop(installed);
        assert_ne!(
            unsafe { win::VirtualFree(trampoline, 0, win::MEM_RELEASE) },
            0
        );
        assert_ne!(
            unsafe { win::VirtualFree(page.cast(), 0, win::MEM_RELEASE) },
            0
        );
    }

    #[test]
    fn remove_owned_report_counts_a_failed_restore() {
        let _guard = TEST_LOCK.lock().unwrap();
        let page = code_page();
        let trampoline = as_plugin(0, || unsafe {
            install_auto(page.cast(), two as *mut c_void)
        })
        .unwrap();
        // The synthetic target is decommitted (its reservation retained)
        // before removal, so restoring it fails and its ownership and
        // trampoline must be retained.
        assert_ne!(
            unsafe { win::VirtualFree(page.cast(), 0x1000, win::MEM_DECOMMIT) },
            0
        );
        let (_removed, failed) = remove_owned_report(0);
        assert!(failed >= 1, "the decommitted target must fail to restore");
        let index = INSTALLED
            .lock()
            .unwrap()
            .iter()
            .position(|hook| hook.target == page as usize)
            .unwrap();
        assert_ne!(
            unsafe { win::VirtualFree(trampoline, 0, win::MEM_RELEASE) },
            0
        );
        INSTALLED.lock().unwrap().remove(index);
        assert_ne!(
            unsafe { win::VirtualFree(page.cast(), 0, win::MEM_RELEASE) },
            0
        );
    }

    #[test]
    fn remove_owned_report_restores_what_it_can_and_keeps_the_rest() {
        let _guard = TEST_LOCK.lock().unwrap();
        let good = code_page();
        let bad = code_page();
        begin_plugin(0, "mixed");
        unsafe { install_auto(good.cast(), two as *mut c_void) }.unwrap();
        let retained = unsafe { install_auto(bad.cast(), three as *mut c_void) }.unwrap();
        end_plugin();
        // Keep the reservation so other tests cannot reuse the failing target.
        assert_ne!(
            unsafe { win::VirtualFree(bad.cast(), 0x1000, win::MEM_DECOMMIT) },
            0
        );
        assert_eq!(remove_owned_report(0), (1, 1));
        let index = INSTALLED
            .lock()
            .unwrap()
            .iter()
            .position(|hook| hook.target == bad as usize)
            .unwrap();
        assert!(
            win::is_executable(retained as usize),
            "a live trampoline must not be freed"
        );
        INSTALLED.lock().unwrap().remove(index);
        assert_ne!(
            unsafe { win::VirtualFree(retained, 0, win::MEM_RELEASE) },
            0
        );
        assert_ne!(
            unsafe { win::VirtualFree(bad.cast(), 0, win::MEM_RELEASE) },
            0
        );
        unsafe { win::VirtualFree(good.cast(), 0, win::MEM_RELEASE) };
    }

    #[test]
    fn byte_patches_share_hook_ownership_and_restore_protection() {
        let _guard = TEST_LOCK.lock().unwrap();
        let page = code_page();
        let original = unsafe { read(page, 6) };
        let replacement = [0xb8, 7, 0, 0, 0, 0xc3];
        let mut previous = 0;
        assert_ne!(
            unsafe { win::VirtualProtect(page.cast(), 0x1000, 0x20, &mut previous) },
            0
        );
        begin_plugin(52, "assembled feature");
        unsafe { patch_bytes(page.cast(), &original, &replacement) }.unwrap();
        end_plugin();
        assert_eq!(unsafe { as_call(page)() }, 7);
        let error = as_plugin(53, || unsafe {
            install(page.add(1).cast(), two as *mut c_void, 5)
        })
        .unwrap_err();
        assert!(error.contains("assembled feature"));
        let mut info: win::MemoryBasicInformation = unsafe { core::mem::zeroed() };
        unsafe { win::VirtualQuery(page.cast(), &mut info, core::mem::size_of_val(&info)) };
        assert_eq!(info.protect, 0x20);
        assert_eq!(remove_owned(52), 1);
        assert_eq!(unsafe { read(page, 6) }, original);
        unsafe { win::VirtualFree(page.cast(), 0, win::MEM_RELEASE) };
    }
    // Explicit address windows make the distant-detour cases independent of
    // where ASLR puts the test executable and its Rust functions.
    fn test_allocation(base: usize) -> PendingCode {
        for slot in 0..256 {
            let address = base + slot * 0x10000;
            let page = unsafe {
                win::VirtualAlloc(
                    address as *mut c_void,
                    0x1000,
                    win::MEM_COMMIT_RESERVE,
                    win::PAGE_EXECUTE_READWRITE,
                )
            };
            if !page.is_null() {
                return PendingCode(page as usize);
            }
        }
        panic!("could not reserve native test address window {base:#x}");
    }

    fn distant_code(displaced: usize) -> (PendingCode, PendingCode, Vec<u8>) {
        let entry = test_allocation(0x1000_0000);
        let detour = test_allocation(0x0200_0000_0000);
        assert!(relative32(entry.0, detour.0).is_none());
        let mut original = vec![0xb8, 1, 0, 0, 0]; // mov eax, 1
        original.resize(displaced, 0x90);
        // The original trampoline MUST execute the continuation to return 5.
        original.extend_from_slice(&[0x83, 0xc0, 4, 0xc3]); // add eax, 4; ret
        original.resize(32, 0x90);
        write(entry.0 as *mut u8, &original).unwrap();
        write(detour.0 as *mut u8, &[0xb8, 9, 0, 0, 0, 0xc3]).unwrap();
        (entry, detour, original)
    }

    fn memory_state(address: usize) -> u32 {
        let mut info: win::MemoryBasicInformation = unsafe { core::mem::zeroed() };
        assert_ne!(
            unsafe {
                win::VirtualQuery(
                    address as *const c_void,
                    &mut info,
                    core::mem::size_of_val(&info),
                )
            },
            0
        );
        info.state
    }

    fn relay_reached(entry: usize) -> usize {
        let patch = unsafe { read(entry as *const u8, 5) };
        assert_eq!(patch[0], 0xe9);
        let displacement = i32::from_le_bytes(patch[1..5].try_into().unwrap());
        (entry as i128 + 5 + displacement as i128) as usize
    }

    #[test]
    fn public_entry_apis_use_a_relay_for_distant_five_byte_hooks() {
        let _guard = TEST_LOCK.lock().unwrap();
        for exact in [false, true] {
            let (entry, detour, bytes) = distant_code(5);
            let api = crate::resolve::build_api();
            let mut trampoline = core::ptr::null_mut();
            let result = as_plugin(0, || unsafe {
                if exact {
                    (api.hook_exact)(
                        entry.0 as *mut c_void,
                        detour.0 as *mut c_void,
                        5,
                        &mut trampoline,
                    )
                } else {
                    (api.hook)(
                        entry.0 as *mut c_void,
                        detour.0 as *mut c_void,
                        &mut trampoline,
                    )
                }
            });
            assert_eq!(result, 0);
            let relay = relay_reached(entry.0);
            assert_eq!(relay, trampoline as usize + 5 + ABSOLUTE_JUMP);
            assert_eq!(
                unsafe { read(relay as *const u8, ABSOLUTE_JUMP) },
                absolute_jump(detour.0)
            );
            assert_eq!(
                unsafe { read((entry.0 + 5) as *const u8, bytes.len() - 5) },
                bytes[5..]
            );
            assert_eq!(unsafe { as_call(entry.0 as *mut u8)() }, 9);
            assert_eq!(unsafe { as_call(trampoline.cast())() }, 5);
            assert_eq!(unsafe { (api.unhook)(entry.0 as *mut c_void) }, 0);
            assert_eq!(unsafe { read(entry.0 as *const u8, bytes.len()) }, bytes);
            assert_eq!(unsafe { as_call(entry.0 as *mut u8)() }, 5);
            assert_eq!(memory_state(trampoline as usize), win::MEM_COMMIT); // retained after restore
            assert_eq!(memory_state(relay), win::MEM_COMMIT);
        }
    }

    #[test]
    fn public_exact_entry_keeps_direct_absolute_jumps_for_fourteen_byte_spans() {
        let _guard = TEST_LOCK.lock().unwrap();
        let (entry, detour, bytes) = distant_code(14);
        let api = crate::resolve::build_api();
        let mut trampoline = core::ptr::null_mut();
        assert_eq!(
            as_plugin(0, || unsafe {
                (api.hook_exact)(
                    entry.0 as *mut c_void,
                    detour.0 as *mut c_void,
                    14,
                    &mut trampoline,
                )
            }),
            0
        );
        assert_eq!(
            unsafe { read(entry.0 as *const u8, 14) },
            absolute_jump(detour.0)
        );
        assert_eq!(unsafe { as_call(entry.0 as *mut u8)() }, 9);
        assert_eq!(unsafe { as_call(trampoline.cast())() }, 5);
        assert_eq!(
            unsafe { read((entry.0 + 14) as *const u8, bytes.len() - 14) },
            bytes[14..]
        );
        assert_eq!(unsafe { (api.unhook)(entry.0 as *mut c_void) }, 0);
        assert_eq!(unsafe { read(entry.0 as *const u8, bytes.len()) }, bytes);
        assert_eq!(memory_state(trampoline as usize), win::MEM_COMMIT);
    }

    #[test]
    fn public_entry_apis_still_jump_directly_to_nearby_detours() {
        let _guard = TEST_LOCK.lock().unwrap();
        for exact in [false, true] {
            let (entry, _far, bytes) = distant_code(5);
            let detour = entry.0 + 0x100;
            write(detour as *mut u8, &[0xb8, 9, 0, 0, 0, 0xc3]).unwrap();
            let api = crate::resolve::build_api();
            let mut trampoline = core::ptr::null_mut();
            let result = as_plugin(0, || unsafe {
                if exact {
                    (api.hook_exact)(
                        entry.0 as *mut c_void,
                        detour as *mut c_void,
                        5,
                        &mut trampoline,
                    )
                } else {
                    (api.hook)(
                        entry.0 as *mut c_void,
                        detour as *mut c_void,
                        &mut trampoline,
                    )
                }
            });
            assert_eq!(result, 0);
            assert_eq!(relay_reached(entry.0), detour);
            assert_eq!(unsafe { as_call(entry.0 as *mut u8)() }, 9);
            assert_eq!(unsafe { as_call(trampoline.cast())() }, 5);
            assert_eq!(unsafe { (api.unhook)(entry.0 as *mut c_void) }, 0);
            assert_eq!(unsafe { read(entry.0 as *const u8, bytes.len()) }, bytes);
        }
    }

    #[test]
    fn failed_plugin_cleanup_retains_original_and_relay_together() {
        let _guard = TEST_LOCK.lock().unwrap();
        let (entry, detour, bytes) = distant_code(5);
        let api = crate::resolve::build_api();
        let mut trampoline = core::ptr::null_mut();
        begin_plugin(93, "failed distant plugin");
        assert_eq!(
            unsafe {
                (api.hook)(
                    entry.0 as *mut c_void,
                    detour.0 as *mut c_void,
                    &mut trampoline,
                )
            },
            0
        );
        let relay = relay_reached(entry.0);
        // A later hook fails during the same init; the host removes that owner's
        // earlier hooks using this registry, without an explicit unhook call.
        let invalid = entry.0 + 0x100;
        write(invalid as *mut u8, &[0xe8, 0, 0, 0, 0]).unwrap();
        assert_ne!(
            unsafe {
                (api.hook_exact)(
                    invalid as *mut c_void,
                    detour.0 as *mut c_void,
                    5,
                    core::ptr::null_mut(),
                )
            },
            0
        );
        end_plugin();
        assert_eq!(remove_owned_report(93), (1, 0));
        assert_eq!(unsafe { read(entry.0 as *const u8, bytes.len()) }, bytes);
        assert_eq!(memory_state(trampoline as usize), win::MEM_COMMIT);
        assert_eq!(memory_state(relay), win::MEM_COMMIT);
    }

    #[test]
    fn a_failed_restore_retains_both_code_paths_until_retry_succeeds() {
        let _guard = TEST_LOCK.lock().unwrap();
        let (entry, detour, bytes) = distant_code(5);
        let trampoline = as_plugin(0, || unsafe {
            install(entry.0 as *mut c_void, detour.0 as *mut c_void, 5)
        })
        .unwrap();
        let relay = relay_reached(entry.0);
        let hooked = unsafe { read(entry.0 as *const u8, bytes.len()) };
        // Keep the reservation so another test cannot reuse our target while
        // VirtualProtect fails against its decommitted page.
        assert_ne!(
            unsafe { win::VirtualFree(entry.0 as *mut c_void, 0x1000, win::MEM_DECOMMIT) },
            0
        );
        assert!(remove(entry.0 as *mut c_void).is_err());
        assert!(INSTALLED
            .lock()
            .unwrap()
            .iter()
            .any(|hook| hook.target == entry.0));
        assert_eq!(memory_state(trampoline as usize), 0x1000); // MEM_COMMIT
        assert_eq!(memory_state(relay), 0x1000);
        let restored = unsafe {
            win::VirtualAlloc(
                entry.0 as *mut c_void,
                0x1000,
                0x1000,
                win::PAGE_EXECUTE_READWRITE,
            )
        };
        assert_eq!(restored as usize, entry.0);
        write(entry.0 as *mut u8, &hooked).unwrap();
        assert_eq!(unsafe { as_call(entry.0 as *mut u8)() }, 9);
        assert_eq!(unsafe { as_call(trampoline.cast())() }, 5);
        remove(entry.0 as *mut c_void).unwrap();
        assert_eq!(unsafe { read(entry.0 as *const u8, bytes.len()) }, bytes);
        assert_eq!(memory_state(trampoline as usize), win::MEM_COMMIT);
        assert_eq!(memory_state(relay), win::MEM_COMMIT);
    }

    #[test]
    fn entry_allocation_range_and_commit_failures_leave_no_patch_or_allocation() {
        let _guard = TEST_LOCK.lock().unwrap();
        let (entry, detour, bytes) = distant_code(5);
        for failure in ["allocation", "range", "commit"] {
            let allocated = std::cell::Cell::new(0usize);
            let mut original_out = 0x1234usize as *mut c_void;
            let result = as_plugin(0, || unsafe {
                install_entry(
                    entry.0 as *mut c_void,
                    detour.0 as *mut c_void,
                    5,
                    &mut original_out,
                    |hint, size| {
                        if failure == "allocation" {
                            return Err("injected allocation failure".into());
                        }
                        let code = if failure == "range" {
                            test_allocation(0x0300_0000_0000)
                        } else {
                            PendingCode(alloc_near(hint, size)?)
                        };
                        allocated.set(code.0);
                        let address = code.0;
                        core::mem::forget(code); // ownership passes to install_entry
                        Ok(address)
                    },
                    |_, _| {
                        assert_eq!(
                            failure, "commit",
                            "failed preparation must not publish a patch"
                        );
                        Err(CommitError::BeforeWrite(
                            "injected target-write failure".into(),
                        ))
                    },
                )
            });
            let error = result.unwrap_err();
            assert_eq!(
                original_out as usize,
                if failure == "commit" { 0 } else { 0x1234 },
                "a refused publication must not leave a freed trampoline in the output pointer"
            );
            assert!(
                error.contains(if failure == "range" {
                    "rel32 reach"
                } else {
                    "injected"
                }),
                "{error}"
            );
            assert_eq!(unsafe { read(entry.0 as *const u8, bytes.len()) }, bytes);
            assert!(!INSTALLED
                .lock()
                .unwrap()
                .iter()
                .any(|hook| hook.target == entry.0));
            if allocated.get() != 0 {
                assert_eq!(memory_state(allocated.get()), 0x10000);
                assert_eq!(memory_state(allocated.get() + 5 + ABSOLUTE_JUMP), 0x10000);
            }
        }
    }

    #[test]
    fn a_commit_that_changes_bytes_before_failing_still_installs_the_hook() {
        let _guard = TEST_LOCK.lock().unwrap();
        let (entry, detour, bytes) = distant_code(5);
        // The injected writer changes the target, then reports a follow-up
        // failure. The allocation must be retained, not freed as uncommitted.
        let trampoline = as_plugin(0, || unsafe {
            install_entry(
                entry.0 as *mut c_void,
                detour.0 as *mut c_void,
                5,
                core::ptr::null_mut(),
                alloc_near,
                |target, patch| {
                    write(target, patch).unwrap();
                    Err(CommitError::AfterWrite("injected flush failure".into()))
                },
            )
        })
        .unwrap();
        assert_eq!(unsafe { as_call(entry.0 as *mut u8)() }, 9);
        assert_eq!(unsafe { as_call(trampoline.cast())() }, 5);
        assert!(INSTALLED
            .lock()
            .unwrap()
            .iter()
            .any(|hook| hook.target == entry.0));
        assert_eq!(memory_state(trampoline as usize), 0x1000); // MEM_COMMIT, not freed
        assert_eq!(remove_owned(0), 1);
        assert_eq!(unsafe { read(entry.0 as *const u8, bytes.len()) }, bytes);
        assert_eq!(memory_state(trampoline as usize), win::MEM_COMMIT); // retained after restore
    }

    #[test]
    fn the_original_pointer_is_stored_before_publication() {
        let _guard = TEST_LOCK.lock().unwrap();
        let (entry, detour, _bytes) = distant_code(5);
        let mut original = core::ptr::null_mut();
        let out = &mut original as *mut *mut c_void;
        let observed = std::cell::Cell::new(usize::MAX);
        // The commit closure runs at publication, so the out pointer must
        // already hold the trampoline when the branch becomes visible.
        let trampoline = as_plugin(0, || unsafe {
            install_entry(
                entry.0 as *mut c_void,
                detour.0 as *mut c_void,
                5,
                out,
                alloc_near,
                |target, patch| {
                    observed.set(out.read() as usize);
                    write(target, patch)
                },
            )
        })
        .unwrap();
        assert_eq!(observed.get(), trampoline as usize);
        assert_eq!(original as usize, trampoline as usize);
        assert_eq!(remove_owned(0), 1);
    }

    #[test]
    fn a_patch_across_differently_protected_pages_restores_each_region() {
        let _guard = TEST_LOCK.lock().unwrap();
        const PAGE_EXECUTE_READ: u32 = 0x20;
        // One reservation of two executable pages with different protections,
        // so a single saved protection would be wrong for the span.
        let base = unsafe {
            win::VirtualAlloc(
                core::ptr::null_mut(),
                0x2000,
                win::MEM_COMMIT_RESERVE,
                win::PAGE_EXECUTE_READWRITE,
            )
        } as *mut u8;
        assert!(!base.is_null());
        let mut previous = 0;
        assert_ne!(
            unsafe { win::VirtualProtect(base.cast(), 0x1000, PAGE_EXECUTE_READ, &mut previous) },
            0
        );

        let at = unsafe { base.add(0x1000 - 3) };
        let original = unsafe { read(at, 6) };
        let replacement = [0xb8, 7, 0, 0, 0, 0xc3];
        begin_plugin(0, "cross-page patch");
        unsafe { patch_bytes(at.cast(), &original, &replacement) }.unwrap();
        end_plugin();
        assert_eq!(unsafe { as_call(at)() }, 7);

        let protect = |page: *mut u8| {
            let mut info: win::MemoryBasicInformation = unsafe { core::mem::zeroed() };
            unsafe {
                win::VirtualQuery(
                    page as *const c_void,
                    &mut info,
                    core::mem::size_of_val(&info),
                )
            };
            info.protect
        };
        assert_eq!(protect(base), PAGE_EXECUTE_READ);
        assert_eq!(
            protect(unsafe { base.add(0x1000) }),
            win::PAGE_EXECUTE_READWRITE
        );
        assert_eq!(remove_owned(0), 1);
        assert_eq!(unsafe { read(at, 6) }, original);
        unsafe { win::VirtualFree(base.cast(), 0, win::MEM_RELEASE) };
    }

    #[test]
    fn relative_range_checks_include_instruction_length_and_do_not_wrap() {
        let from = 0x1_0000_0000usize;
        let origin = from + MIN_DISPLACED;
        assert_eq!(relative32(from, origin + i32::MAX as usize), Some(i32::MAX));
        assert_eq!(relative32(from, origin - 0x8000_0000), Some(i32::MIN));
        assert_eq!(relative32(from, origin + 0x8000_0000), None);
        assert_eq!(relative32(from, origin - 0x8000_0001), None);
        assert_eq!(relative32(usize::MAX, 0), None);
    }
}

fn record_mapping(target: usize, length: usize, stub: usize, stub_size: usize, owner: &str) {
    crate::crash::map(
        target,
        target.saturating_add(length),
        &format!("{owner} patch site"),
    );
    if stub != 0 {
        crate::crash::map(
            stub,
            stub.saturating_add(stub_size),
            &format!("{owner} trampoline/relay for {target:#x}"),
        );
    }
}
