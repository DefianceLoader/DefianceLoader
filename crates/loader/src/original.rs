//! Code as it was before any plugin wrote to it.
//!
//! A plugin that checks bytes it only reads or calls, such as a function it
//! validates before calling, must not fail because another plugin hooked that
//! function first: which one starts first would then decide whether it works.
//! Every hook and byte patch goes through the loader, which keeps the bytes it
//! replaced, so the original is the live memory with those put back
//! ([`crate::hooks::put_back_originals`]). Signature scans read the same view
//! (`resolve::find_one`). A plugin still checks the live bytes where it writes,
//! and ownership refuses a write over another plugin's.
//!
//! The loader's `original` service ([`API`]) hands the view to plugins. Writes
//! a plugin makes without the loader (Core's renderer table slots) are not
//! seen.

/// Memory at `start` with the loader-owned writes it overlaps undone, or None
/// when the range is not readable.
pub fn read(start: usize, length: usize) -> Option<Vec<u8>> {
    if length == 0 || start.checked_add(length).is_none() || !crate::win::is_readable(start, length)
    {
        return None;
    }
    let mut bytes = unsafe { core::slice::from_raw_parts(start as *const u8, length) }.to_vec();
    crate::hooks::put_back_originals(start, &mut bytes);
    Some(bytes)
}

unsafe extern "C" fn api_read(address: usize, out: *mut u8, length: usize) -> i32 {
    if address == 0 || out.is_null() || length == 0 {
        return 1;
    }
    let Some(bytes) = read(address, length) else {
        return 2;
    };
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, length) };
    0
}

/// `defiance.loader` / `original` v1.
pub static API: defiance_api::OriginalV1 = defiance_api::OriginalV1 { read: api_read };

#[cfg(test)]
mod tests {
    #[test]
    fn unreadable_or_empty_ranges_are_refused() {
        let mut out = [0u8; 4];
        assert_eq!(unsafe { super::api_read(0, out.as_mut_ptr(), 4) }, 1);
        assert_eq!(unsafe { super::api_read(0x1000, out.as_mut_ptr(), 0) }, 1);
        assert_eq!(unsafe { super::api_read(0x1000, out.as_mut_ptr(), 4) }, 2);
        let data = [1u8, 2, 3, 4];
        assert_eq!(
            unsafe { super::api_read(data.as_ptr() as usize, out.as_mut_ptr(), 4) },
            0
        );
        assert_eq!(out, data);
    }
}
