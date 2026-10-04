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

/// Commit executable memory as close to `hint` as the address space allows:
/// a slot [`crate::near`] held since the module loaded, or else the first
/// free one walking outward that a rel32 from the target could reach.
///
/// A candidate that is already in use fails with `ERROR_INVALID_ADDRESS`.
/// Any other error is the system refusing executable memory outright (an
/// exploit-protection policy, say), so the first one is named in the failure
/// rather than reported as a full address space.
pub fn alloc_near(hint: usize, size: usize) -> Result<usize, String> {
    const GRANULARITY: usize = 0x10000;
    const REACH: usize = 0x8000;
    const ERROR_INVALID_ADDRESS: u32 = 487;
    if let Some(held) = crate::near::take(hint, size) {
        return Ok(held);
    }
    let base = hint & !(GRANULARITY - 1);
    let mut refused: Option<(u32, usize)> = None;
    for step in 1..REACH {
        for candidate in [
            base.wrapping_add(step * GRANULARITY),
            base.checked_sub(step * GRANULARITY).unwrap_or(0),
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
            let error = unsafe { win::GetLastError() };
            if error != ERROR_INVALID_ADDRESS && refused.is_none() {
                refused = Some((error, candidate));
            }
        }
    }
    Err(match refused {
        Some((error, at)) => format!(
            "no executable page within reach of the hook: VirtualAlloc at {at:#x} \
             failed with error {error}"
        ),
        None => "no free page within reach of the hook".to_string(),
    })
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

/// Classify a pre-write failure by whether its protection changes were
/// restored. Callers retain patch ownership if protection recovery fails.
fn prewrite_failure(regions: &[Protection], error: String) -> CommitError {
    match restore(regions) {
        Ok(()) => CommitError::BeforeWrite(error),
        Err(restore_error) => CommitError::AfterWrite(format!("{error}; {restore_error}")),
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
            return Err(prewrite_failure(
                &regions,
                format!("VirtualQuery at {at:#x} failed"),
            ));
        }
        if info.state != win::MEM_COMMIT || info.protect & win::PAGE_GUARD != 0 {
            return Err(prewrite_failure(
                &regions,
                format!("{at:#x} is not committed readable code"),
            ));
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
            return Err(prewrite_failure(
                &regions,
                format!("VirtualProtect at {at:#x} failed with error {}", unsafe {
                    win::GetLastError()
                }),
            ));
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

/// One checked write in a multi-plugin patch plan.
pub struct PlannedWrite<'a> {
    pub address: *mut u8,
    pub before: &'a [u8],
    pub after: &'a [u8],
    pub invariant: usize,
}

/// Publish a resolved patch plan at one stop-the-world safe point.
///
/// Every expected span is checked again while threads are suspended. If a
/// write or instruction-cache flush fails, all writes made by this transaction
/// are restored before threads resume. `AfterWrite` means that restoration or
/// protection recovery failed and callers must retain every owner's code and
/// bookkeeping.
pub fn publish_transaction(writes: &[PlannedWrite<'_>]) -> Result<(), CommitError> {
    if writes.is_empty() {
        return Ok(());
    }
    if writes.iter().any(|write| {
        write.before.is_empty()
            || write.before.len() != write.after.len()
            || write.invariant > write.after.len()
    }) {
        return Err(CommitError::BeforeWrite(
            "invalid span in the unified patch plan".into(),
        ));
    }

    let mut regions = Vec::new();
    for write in writes {
        match protect_span(write.address, write.after.len()) {
            Ok(mut protected) => regions.append(&mut protected),
            Err(CommitError::BeforeWrite(error)) => {
                return Err(prewrite_failure(&regions, error));
            }
            Err(CommitError::AfterWrite(error)) => {
                if let Err(restore_error) = restore(&regions) {
                    return Err(CommitError::AfterWrite(format!("{error}; {restore_error}")));
                }
                return Err(CommitError::AfterWrite(error));
            }
        }
    }

    enum Outcome {
        Written,
        Refused,
        Changed,
        FlushFailed {
            index: usize,
            error: u32,
            rollback_error: Option<u32>,
        },
    }

    let outcome = crate::threads::stop_the_world(|ips| {
        for write in writes {
            let start = write.address as usize;
            let end = start.saturating_add(write.invariant);
            if ips.iter().any(|&rip| rip > start && rip < end) {
                return Outcome::Refused;
            }
            let live = unsafe {
                core::slice::from_raw_parts(write.address.cast_const(), write.before.len())
            };
            if live != write.before {
                return Outcome::Changed;
            }
        }

        for (index, write) in writes.iter().enumerate() {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    write.after.as_ptr(),
                    write.address,
                    write.after.len(),
                )
            };
            if unsafe {
                win::FlushInstructionCache(
                    win::GetCurrentProcess(),
                    write.address.cast(),
                    write.after.len(),
                )
            } == 0
            {
                let error = unsafe { win::GetLastError() };
                let mut rollback_error = None;
                for rollback in writes[..=index].iter().rev() {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            rollback.before.as_ptr(),
                            rollback.address,
                            rollback.before.len(),
                        )
                    };
                    if unsafe {
                        win::FlushInstructionCache(
                            win::GetCurrentProcess(),
                            rollback.address.cast(),
                            rollback.before.len(),
                        )
                    } == 0
                    {
                        rollback_error.get_or_insert_with(|| unsafe { win::GetLastError() });
                    }
                }
                return Outcome::FlushFailed {
                    index,
                    error,
                    rollback_error,
                };
            }
        }
        Outcome::Written
    });

    let protection = restore(&regions).err();
    let protection_failed = protection.is_some();
    let append_protection = |mut error: String| {
        if let Some(reason) = &protection {
            error.push_str(&format!("; {reason}"));
        }
        error
    };
    match outcome {
        Ok(Outcome::Written) => match protection.as_ref() {
            None => Ok(()),
            Some(error) => Err(CommitError::AfterWrite(error.clone())),
        },
        Ok(Outcome::Refused) => {
            let error = append_protection(
                "a thread is executing inside a span to be published".into(),
            );
            Err(if protection_failed {
                CommitError::AfterWrite(error)
            } else {
                CommitError::BeforeWrite(error)
            })
        }
        Ok(Outcome::Changed) => {
            let error = append_protection("a patch site's live bytes changed after planning".into());
            Err(if protection_failed {
                CommitError::AfterWrite(error)
            } else {
                CommitError::BeforeWrite(error)
            })
        }
        Ok(Outcome::FlushFailed {
            index,
            error,
            rollback_error: None,
        }) => {
            let error = append_protection(format!(
                "flushing patch span {index} failed with error {error}; the transaction was restored"
            ));
            Err(if protection_failed {
                CommitError::AfterWrite(error)
            } else {
                CommitError::BeforeWrite(error)
            })
        }
        Ok(Outcome::FlushFailed {
            index,
            error,
            rollback_error: Some(rollback_error),
        }) => Err(CommitError::AfterWrite(append_protection(format!(
            "flushing patch span {index} failed with error {error}; restoring the transaction also failed with error {rollback_error}"
        )))),
        Err(error) => {
            let error = append_protection(error);
            Err(if protection_failed {
                CommitError::AfterWrite(error)
            } else {
                CommitError::BeforeWrite(error)
            })
        }
    }
}
