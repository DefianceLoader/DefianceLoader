//! Executable-memory operations shared by every hook kind.
//!
//! Allocation, instruction-cache flushing, per-region page protection, the
//! commit outcome and the release of unpublished storage live here so entry
//! hooks, call stubs and byte patches behave identically.
//!
//! A commit tells the caller whether it failed *before* the target bytes
//! changed (nothing was published; unpublished allocation may be freed) or
//! *after* (the target is patched and reachable, so ownership and storage must
//! be retained). It never reports a post-write failure as uncommitted.

use crate::win;
use core::ffi::c_void;

/// Commit failed before or after the target bytes changed.
#[derive(Debug)]
pub enum CommitError {
    /// No target byte changed; the caller may free unpublished storage.
    BeforeWrite(String),
    /// The bytes changed but a follow-up step (cache flush or protection
    /// restore) failed. The caller retains the allocation and ownership: the
    /// target is patched and may already be executing through it.
    AfterWrite(String),
}

impl core::fmt::Display for CommitError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CommitError::BeforeWrite(message) => write!(formatter, "not committed: {message}"),
            CommitError::AfterWrite(message) => {
                write!(formatter, "committed with follow-up failure: {message}")
            }
        }
    }
}

impl std::error::Error for CommitError {}

/// Unpublished executable storage, released on every failure path. Once the
/// target is patched the owner must `forget` it, so the live code stays mapped.
pub struct PendingCode(pub usize);

impl Drop for PendingCode {
    fn drop(&mut self) {
        unsafe { win::VirtualFree(self.0 as *mut c_void, 0, win::MEM_RELEASE) };
    }
}

impl PendingCode {
    pub fn address(&self) -> usize {
        self.0
    }

    /// Keep the allocation alive; the published patch now owns it.
    pub fn forget(self) {
        core::mem::forget(self);
    }
}

/// Flush the instruction cache for `[address, address + size)`, reporting a
/// failure instead of ignoring it.
pub fn flush(address: usize, size: usize) -> Result<(), String> {
    if unsafe {
        win::FlushInstructionCache(win::GetCurrentProcess(), address as *const c_void, size)
    } == 0
    {
        return Err(format!(
            "flushing executable code failed with error {}",
            unsafe { win::GetLastError() }
        ));
    }
    Ok(())
}

/// Commit executable memory as close to `hint` as the address space allows,
/// walking outward until a rel32 from the target could reach it.
pub fn alloc_near(hint: usize, size: usize) -> Result<usize, String> {
    const GRANULARITY: usize = 0x10000;
    const REACH: usize = 0x8000;
    let base = hint & !(GRANULARITY - 1);
    for step in 1..REACH {
        for candidate in [
            base.wrapping_add(step * GRANULARITY),
            base.wrapping_sub(step * GRANULARITY),
        ] {
            if candidate == 0 {
                continue;
            }
            let got = unsafe {
                win::VirtualAlloc(
                    candidate as *mut c_void,
                    size,
                    win::MEM_COMMIT_RESERVE,
                    win::PAGE_EXECUTE_READWRITE,
                )
            };
            if !got.is_null() {
                return Ok(got as usize);
            }
        }
    }
    Err("no free page within reach of the hook".to_string())
}

/// One page region whose original protection is saved for restore.
struct Protection {
    address: *mut u8,
    size: usize,
    previous: u32,
}

/// Put every saved protection back. Reports the first failure, logging any
/// later ones, so a partial restore is never silent.
fn restore(regions: &[Protection]) -> Result<(), String> {
    let mut first = None;
    for region in regions.iter().rev() {
        let mut ignored = 0;
        if unsafe {
            win::VirtualProtect(
                region.address as *mut c_void,
                region.size,
                region.previous,
                &mut ignored,
            )
        } == 0
        {
            let message = format!(
                "restoring protection at {:p} failed with error {}",
                region.address,
                unsafe { win::GetLastError() }
            );
            if first.is_none() {
                first = Some(message);
            } else {
                crate::log::error(&message);
            }
        }
    }
    match first {
        None => Ok(()),
        Some(message) => Err(message),
    }
}

/// Make every region in the span writable-executable, saving each one's
/// original protection. A span may cross pages with different protections; each
/// region is changed and restored on its own.
///
/// Fails before changing any byte when a region is not committed/readable or a
/// protection change fails, restoring regions already changed.
fn protect_span(address: *mut u8, length: usize) -> Result<Vec<Protection>, CommitError> {
    let mut regions = Vec::new();
    let end = address as usize + length;
    let mut at = address as usize;
    while at < end {
        let mut info: win::MemoryBasicInformation = unsafe { core::mem::zeroed() };
        if unsafe {
            win::VirtualQuery(
                at as *const c_void,
                &mut info,
                core::mem::size_of_val(&info),
            )
        } == 0
        {
            let _ = restore(&regions);
            return Err(CommitError::BeforeWrite(format!(
                "VirtualQuery at {at:#x} failed"
            )));
        }
        if info.state != win::MEM_COMMIT || info.protect & win::PAGE_GUARD != 0 {
            let _ = restore(&regions);
            return Err(CommitError::BeforeWrite(format!(
                "{at:#x} is not committed readable code"
            )));
        }
        let region_end = (info.base_address as usize + info.region_size).min(end);
        let size = region_end - at;
        let mut previous = 0;
        if unsafe {
            win::VirtualProtect(
                at as *mut c_void,
                size,
                win::PAGE_EXECUTE_READWRITE,
                &mut previous,
            )
        } == 0
        {
            let _ = restore(&regions);
            return Err(CommitError::BeforeWrite(format!(
                "VirtualProtect at {at:#x} failed with error {}",
                unsafe { win::GetLastError() }
            )));
        }
        regions.push(Protection {
            address: at as *mut u8,
            size,
            previous,
        });
        at = region_end;
    }
    Ok(regions)
}

/// Write `bytes` at `address` at a safe point, making each page
/// writable-executable and restoring its original protection once the bytes are
/// in place, then flush the instruction cache.
///
/// `invariant` is the leading byte count that must contain no instruction
/// boundary: for a published branch that is the displaced span, because a
/// suspended thread at an internal boundary would execute part of the branch.
/// Restores also refuse interior pointers: an instruction boundary in the
/// patched span need not be a boundary in the original bytes.
fn commit_span(address: *mut u8, bytes: &[u8], invariant: usize) -> Result<(), CommitError> {
    enum Outcome {
        Refused,
        Written(Result<(), u32>),
    }
    let regions = protect_span(address, bytes.len())?;
    // No heap operations in this closure, including the failure paths.
    let outcome = crate::threads::stop_the_world(|ips| {
        let start = address as usize;
        let end = start.saturating_add(invariant);
        if ips.iter().any(|&rip| rip > start && rip < end) {
            return Outcome::Refused;
        }
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), address, bytes.len()) };
        let ok = unsafe {
            win::FlushInstructionCache(win::GetCurrentProcess(), address.cast(), bytes.len())
        };
        Outcome::Written(if ok != 0 {
            Ok(())
        } else {
            Err(unsafe { win::GetLastError() })
        })
    });
    let mut failure = match outcome {
        Ok(Outcome::Written(Ok(()))) => None,
        Ok(Outcome::Written(Err(error))) => Some(format!(
            "flushing executable code failed with error {error}"
        )),
        Ok(Outcome::Refused) => {
            let restored = restore(&regions);
            let mut error = "a thread is executing inside the span to be published".to_string();
            if let Err(reason) = restored {
                error.push_str(&format!("; {reason}"));
            }
            return Err(CommitError::BeforeWrite(error));
        }
        Err(mut error) => {
            // No byte changed: the protect changes are rolled back and the
            // caller may treat this as an uncommitted write.
            if let Err(reason) = restore(&regions) {
                error.push_str(&format!("; {reason}"));
            }
            return Err(CommitError::BeforeWrite(error));
        }
    };
    if let Err(error) = restore(&regions) {
        failure = Some(match failure {
            Some(first) => format!("{first}; {error}"),
            None => error,
        });
    }
    match failure {
        None => Ok(()),
        Some(message) => Err(CommitError::AfterWrite(message)),
    }
}

/// Restore or set instructions, refusing interior instruction pointers.
pub fn write(address: *mut u8, bytes: &[u8]) -> Result<(), CommitError> {
    commit_span(address, bytes, bytes.len())
}

/// Publish a branch over `invariant` bytes, refusing when a suspended thread is
/// at an instruction boundary inside it.
pub fn publish(address: *mut u8, bytes: &[u8], invariant: usize) -> Result<(), CommitError> {
    commit_span(address, bytes, invariant)
}
