//! Optional panic reporting to the loader without changing the base plugin ABI.
use core::fmt::Write as _;

/// Export once per DLL. Old loaders ignore it. Each DLL has its own Rust panic
/// runtime, so installing a hook only in the loader would miss plugin panics.
#[macro_export]
macro_rules! crash_handshake {
    () => {
        #[no_mangle]
        pub unsafe extern "C" fn defiance_plugin_crash_v1(
            report: unsafe extern "C" fn(*const u8, usize),
        ) {
            unsafe {
                $crate::crash::install(report);
            }
        }
    };
}
/// # Safety
/// The callback must remain valid for the process lifetime, accept borrowed
/// bytes synchronously, and avoid locks/allocations in a failing process.
pub unsafe fn install(report: unsafe extern "C" fn(*const u8, usize)) {
    std::panic::set_hook(Box::new(move |info| {
        struct Buffer {
            bytes: [u8; 2048],
            used: usize,
        }
        impl core::fmt::Write for Buffer {
            fn write_str(&mut self, value: &str) -> core::fmt::Result {
                let mut count = value.len().min(self.bytes.len() - self.used);
                while !value.is_char_boundary(count) {
                    count -= 1;
                }
                self.bytes[self.used..self.used + count]
                    .copy_from_slice(&value.as_bytes()[..count]);
                self.used += count;
                Ok(())
            }
        }
        let mut buffer = Buffer {
            bytes: [0; 2048],
            used: 0,
        };
        let _ = writeln!(buffer, "plugin Rust panic: {info}");
        unsafe {
            report(buffer.bytes.as_ptr(), buffer.used);
        }
    }));
}
