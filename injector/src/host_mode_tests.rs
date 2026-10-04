use super::*;

const MAGIC: &[u8; 8] = b"DFLBOOT1";

fn header(state: u32, pid: u32) -> Vec<u8> {
    let mut bytes = vec![0; 24];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&state.to_le_bytes());
    bytes[16..20].copy_from_slice(&pid.to_le_bytes());
    bytes
}

#[test]
fn startup_header_checks_magic_abi_pid_and_state() {
    assert_eq!(status(&header(0, 42), 42).unwrap(), 0);
    assert_eq!(status(&header(1, 42), 42).unwrap(), 1);
    assert_eq!(status(&header(2, 42), 42).unwrap(), 2);
    assert_eq!(status(&header(3, 42), 42).unwrap(), 3);

    let mut bad = header(2, 42);
    bad[0] ^= 1;
    assert!(status(&bad, 42).is_err());
    let mut bad = header(2, 42);
    bad[8] = 2;
    assert!(status(&bad, 42).is_err());
    assert!(status(&header(2, 43), 42).is_err());
    assert!(status(&header(4, 42), 42).is_err());
    assert!(status(&header(2, 42)[..23], 42).is_err());
}

#[test]
fn export_lookup_rejects_out_of_image_and_truncated_reads() {
    let image = export_image("DEFIANCE_LOADER_STATE", 0x500);
    let reader = |address: usize, len: usize| {
        let end = address
            .checked_add(len)
            .ok_or_else(|| "overflow".to_string())?;
        image
            .get(address..end)
            .map(|bytes| bytes.to_vec())
            .ok_or_else(|| "outside image".to_string())
    };
    assert_eq!(
        export_rva(&reader, image.len(), "DEFIANCE_LOADER_STATE").unwrap(),
        Some(0x500)
    );
    assert_eq!(export_rva(&reader, image.len(), "MISSING").unwrap(), None);

    let mut corrupt = image.clone();
    // NumberOfNames says one entry, but the name pointer table is outside the image.
    corrupt[0x210..0x214].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
    let reader = |address: usize, len: usize| {
        address
            .checked_add(len)
            .and_then(|end| corrupt.get(address..end))
            .map(|bytes| bytes.to_vec())
            .ok_or_else(|| "outside image".to_string())
    };
    assert!(export_rva(&reader, corrupt.len(), "DEFIANCE_LOADER_STATE").is_err());
}

fn export_image(name: &str, data_rva: u32) -> Vec<u8> {
    // A mapped PE32+ image with a real DOS/NT header and export directory.
    let mut image = vec![0; 0x600];
    image[..2].copy_from_slice(b"MZ");
    image[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes()); // e_lfanew
    image[0x80..0x84].copy_from_slice(b"PE\0\0");
    image[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes()); // PE32+
    image[0x104..0x108].copy_from_slice(&1u32.to_le_bytes()); // NumberOfRvaAndSizes
    image[0x108..0x10c].copy_from_slice(&0x180u32.to_le_bytes()); // export RVA
    image[0x10c..0x110].copy_from_slice(&40u32.to_le_bytes()); // export size
    image[0x194..0x198].copy_from_slice(&1u32.to_le_bytes()); // NumberOfFunctions
    image[0x198..0x19c].copy_from_slice(&1u32.to_le_bytes()); // NumberOfNames
    image[0x19c..0x1a0].copy_from_slice(&0x200u32.to_le_bytes()); // AddressOfFunctions
    image[0x1a0..0x1a4].copy_from_slice(&0x210u32.to_le_bytes()); // AddressOfNames
    image[0x1a4..0x1a8].copy_from_slice(&0x220u32.to_le_bytes()); // AddressOfNameOrdinals
    image[0x200..0x204].copy_from_slice(&data_rva.to_le_bytes());
    image[0x210..0x214].copy_from_slice(&0x300u32.to_le_bytes());
    image[0x220..0x222].copy_from_slice(&0u16.to_le_bytes());
    image[0x300..0x300 + name.len()].copy_from_slice(name.as_bytes());
    image[0x300 + name.len()] = 0;
    image
}

const CHILD_MARKER: &str = "DEFIANCE_HOST_MODE_TEST_CHILD";

struct ChildProcess(Child);

impl ChildProcess {
    fn start() -> Self {
        let marker = std::env::temp_dir().join(format!(
            "defiance-host-child-{}-{}.pid",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&marker);
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "host_mode::tests::remote_process_fixture",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_MARKER, &marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("spawn isolated child process");
        let mut fixture = Self(child);
        let started = Instant::now();
        while !marker.exists() {
            if let Some(exit) = fixture.0.try_wait().unwrap() {
                panic!("child exited before readiness: {exit}");
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "child readiness timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid = std::fs::read_to_string(&marker)
            .expect("read child PID")
            .parse::<u32>()
            .expect("parse child PID");
        assert_eq!(pid, fixture.pid());
        let _ = std::fs::remove_file(marker);
        fixture
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn remote_loader_loads_unicode_path_and_repeat_is_idempotent() {
    let fixture = FixtureDll::build("2");
    let child = ChildProcess::start();
    assert!(start(child.pid(), &fixture.path, Duration::from_secs(10)).unwrap());
    assert!(!start(child.pid(), &fixture.path, Duration::from_secs(2)).unwrap());
}

#[test]
fn remote_loader_reports_failure_and_dormant_timeout() {
    let failed = FixtureDll::build("3");
    let child = ChildProcess::start();
    let error = start(child.pid(), &failed.path, Duration::from_secs(10)).unwrap_err();
    assert!(error.to_ascii_lowercase().contains("fail"), "{error}");
    drop(child);

    let dormant = FixtureDll::build("0");
    let child = ChildProcess::start();
    let error = start(child.pid(), &dormant.path, Duration::from_millis(100)).unwrap_err();
    assert!(error.to_ascii_lowercase().contains("tim"), "{error}");
}

#[test]
fn remote_process_fixture() {
    let Ok(marker) = std::env::var(CHILD_MARKER) else {
        return;
    };
    std::fs::write(marker, std::process::id().to_string()).unwrap();
    let mut stop = String::new();
    std::io::stdin().read_line(&mut stop).unwrap();
    assert_eq!(stop.trim(), "stop");
}

struct FixtureDll {
    path: PathBuf,
    _dir: TempDir,
}

impl FixtureDll {
    fn build(initial_state: &str) -> Self {
        let dir = TempDir::new("fixture");
        let source = dir.path.join("host.rs");
        // This DLL is deliberately tiny: DllMain publishes the same exported
        // ABI as the loader and uses only the synthetic target's own PID.
        std::fs::write(
            &source,
            format!(
                r#"
#[repr(C)]
pub struct State {{ magic: [u8; 8], abi: u32, state: u32, pid: u32, reserved: u32 }}
#[no_mangle]
pub static mut DEFIANCE_LOADER_STATE: State = State {{
    magic: *b"DFLBOOT1", abi: 1, state: {initial_state}, pid: 0, reserved: 0
}};
#[link(name = "kernel32")]
extern "system" {{ fn GetCurrentProcessId() -> u32; }}
#[no_mangle]
pub unsafe extern "system" fn DllMain(_: *mut core::ffi::c_void, reason: u32, _: *mut core::ffi::c_void) -> i32 {{
    if reason == 1 {{
        core::ptr::addr_of_mut!(DEFIANCE_LOADER_STATE.pid).write_volatile(GetCurrentProcessId());
    }}
    1
}}
"#
            ),
        )
        .expect("write test DLL source");
        let output = dir.path.join("🦊-loader-fixture.dll");
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let result = Command::new(rustc)
            .args(["--crate-type=cdylib", "--edition=2021"])
            .arg(&source)
            .arg("-o")
            .arg(&output)
            .output()
            .expect("compile synthetic loader DLL");
        assert!(
            result.status.success(),
            "rustc failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        Self {
            path: output,
            _dir: dir,
        }
    }
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "defiance-host-{label}-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

static TEMP_ID: AtomicUsize = AtomicUsize::new(0);

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
