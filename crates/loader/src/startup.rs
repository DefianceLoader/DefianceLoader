//! Stable process-local startup status for EXE launchers that load the neutral
//! loader DLL directly instead of installing it as a system-DLL proxy.

use std::sync::atomic::{AtomicU32, Ordering};

pub const ABI_VERSION: u32 = 1;
pub const DORMANT: u32 = 0;
pub const STARTING: u32 = 1;
pub const READY: u32 = 2;
pub const FAILED: u32 = 3;

/// Exported status block. Its field order and widths are the public ABI.
#[repr(C)]
pub struct LoaderState {
    pub magic: [u8; 8],
    pub abi: u32,
    pub state: AtomicU32,
    pub process_id: AtomicU32,
    pub reserved: u32,
}

impl LoaderState {
    const fn new() -> Self {
        Self {
            magic: *b"DFLBOOT1",
            abi: ABI_VERSION,
            state: AtomicU32::new(DORMANT),
            process_id: AtomicU32::new(0),
            reserved: 0,
        }
    }

    /// Publish the attaching process before its startup worker can run.
    pub fn begin(&self, process_id: u32) {
        self.process_id.store(process_id, Ordering::Relaxed);
        let _ =
            self.state
                .compare_exchange(DORMANT, STARTING, Ordering::Release, Ordering::Relaxed);
    }

    /// Mark a startup failure without overwriting a terminal status.
    pub fn fail(&self) {
        let _ = self
            .state
            .compare_exchange(STARTING, FAILED, Ordering::AcqRel, Ordering::Acquire);
    }

    /// Publish readiness only if startup is still in progress.
    pub fn ready(&self) {
        let _ = self
            .state
            .compare_exchange(STARTING, READY, Ordering::Release, Ordering::Acquire);
    }
}

/// EXEs locate this data export by name after `LoadLibraryW` returns.
#[no_mangle]
pub static DEFIANCE_LOADER_STATE: LoaderState = LoaderState::new();

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn exported_layout_matches_the_abi() {
        assert_eq!(size_of::<LoaderState>(), 24);
        assert_eq!(offset_of!(LoaderState, magic), 0);
        assert_eq!(offset_of!(LoaderState, abi), 8);
        assert_eq!(offset_of!(LoaderState, state), 12);
        assert_eq!(offset_of!(LoaderState, process_id), 16);
        assert_eq!(offset_of!(LoaderState, reserved), 20);
        let state = LoaderState::new();
        assert_eq!(state.magic, *b"DFLBOOT1");
        assert_eq!(state.abi, 1);
        assert_eq!(state.state.load(Ordering::Relaxed), DORMANT);
        assert_eq!(state.process_id.load(Ordering::Relaxed), 0);
        assert_eq!(state.reserved, 0);
    }

    #[test]
    fn lifecycle_has_one_terminal_result() {
        let ready = LoaderState::new();
        ready.begin(42);
        assert_eq!(ready.state.load(Ordering::Acquire), STARTING);
        assert_eq!(ready.process_id.load(Ordering::Relaxed), 42);
        ready.ready();
        ready.fail();
        assert_eq!(ready.state.load(Ordering::Acquire), READY);

        let failed = LoaderState::new();
        failed.begin(7);
        failed.fail();
        failed.ready();
        assert_eq!(failed.state.load(Ordering::Acquire), FAILED);
    }
}
