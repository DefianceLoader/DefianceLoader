#![cfg(feature = "test-host")]
//! Regression probes run in bounded children: a broken hook must not hang or
//! crash the rest of the suite. Executable fixtures have explicit byte layouts.
use core::ffi::c_void;
use defiance_loader::test_host;
use std::sync::atomic::{AtomicBool, Ordering};
#[link(name = "kernel32")]
extern "system" {
    fn VirtualAlloc(p: *mut c_void, n: usize, kind: u32, protection: u32) -> *mut c_void;
    fn FlushInstructionCache(h: *mut c_void, p: *const c_void, n: usize) -> i32;
}
static ENTERED: AtomicBool = AtomicBool::new(false);
static RELEASE: AtomicBool = AtomicBool::new(false);
static mut ORIGINAL: *mut c_void = core::ptr::null_mut();
unsafe extern "C" fn detour() -> u32 {
    ENTERED.store(true, Ordering::Release);
    while !RELEASE.load(Ordering::Acquire) {
        std::hint::spin_loop();
    }
    let original: unsafe extern "C" fn() -> u32 = unsafe { core::mem::transmute(ORIGINAL) };
    unsafe { original() }
}
unsafe fn page(bytes: &[u8]) -> *mut u8 {
    let page = unsafe { VirtualAlloc(core::ptr::null_mut(), 4096, 0x3000, 0x40) }.cast::<u8>();
    assert!(!page.is_null());
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), page, bytes.len());
        assert_ne!(
            FlushInstructionCache(-1isize as _, page.cast(), bytes.len()),
            0
        );
    }
    page
}
fn bounded_child(name: &str) -> bool {
    if std::env::var_os("DEFIANCE_HOOK_CHILD").is_some() {
        return true;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env("DEFIANCE_HOOK_CHILD", "1")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{name}: {status}");
            return false;
        }
        if std::time::Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("{name} timed out");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
#[test]
fn an_interior_instruction_pointer_refuses_publication() {
    if !bounded_child("an_interior_instruction_pointer_refuses_publication") {
        return;
    }
    unsafe {
        // mov rax,&ENTERED; mov byte ptr [rax],1; loop: jmp loop.
        let mut before = vec![0x48, 0xb8];
        before.extend_from_slice(&(ENTERED.as_ptr() as usize).to_le_bytes());
        before.extend_from_slice(&[0xc6, 0x00, 0x01, 0xeb, 0xfe]);
        let code = page(&before);
        let address = code as usize;
        std::thread::spawn(move || {
            let run: unsafe extern "C" fn() = core::mem::transmute(address);
            run();
        });
        while !ENTERED.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        let after = vec![0x90; before.len()];
        let api = test_host::build_api();
        test_host::begin_plugin(42, "refusal");
        assert_ne!(
            (api.patch_bytes)(code.cast(), before.as_ptr(), after.as_ptr(), before.len()),
            0
        );
        assert_eq!(core::slice::from_raw_parts(code, before.len()), before);
        assert_eq!(
            test_host::remove_owned_report(42),
            (0, 0),
            "a refused patch must not acquire ownership"
        );
        // The deliberately spinning native worker ends with this child.
        std::process::exit(0);
    }
}
#[test]
fn an_active_detour_can_call_original_after_removal() {
    if !bounded_child("an_active_detour_can_call_original_after_removal") {
        return;
    }
    unsafe {
        let code = page(&[0xb8, 1, 0, 0, 0, 0xc3]); // mov eax,1; ret
        let api = test_host::build_api();
        test_host::begin_plugin(42, "retention");
        assert_eq!(
            (api.hook_exact)(
                code.cast(),
                detour as *const () as _,
                5,
                core::ptr::addr_of_mut!(ORIGINAL)
            ),
            0
        );
        let address = code as usize;
        let worker = std::thread::spawn(move || {
            let run: unsafe extern "C" fn() -> u32 = core::mem::transmute(address);
            run()
        });
        while !ENTERED.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        assert_eq!((api.unhook)(code.cast()), 0);
        RELEASE.store(true, Ordering::Release);
        assert_eq!(worker.join().unwrap(), 1);
        let run: unsafe extern "C" fn() -> u32 = core::mem::transmute(code);
        assert_eq!(run(), 1);
    }
}
