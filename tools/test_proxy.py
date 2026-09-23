"""Verify the built proxy's load-time imports and calls from a real DllMain.

Run through mise after cargo build -p defiance-loader [--release].
"""
import argparse
import ctypes
import pathlib
import shutil
import subprocess
import tempfile
import pefile

ROOT = pathlib.Path(__file__).resolve().parent.parent

def run(loader):
    pe = pefile.PE(str(loader))
    # pefile sanitizes path-bearing DLL names to '*invalid*'; inspect raw bytes.
    imports = {pe.get_string_at_rva(item.struct.Name): item for item in pe.DIRECTORY_ENTRY_IMPORT}
    system = br"\\?\GLOBALROOT\SystemRoot\System32\dxgi.dll"
    assert system in imports, list(imports)
    exports = {item.name for item in pe.DIRECTORY_ENTRY_EXPORT.symbols if item.name and item.name != b"DllMain"}
    imported = {item.name for item in imports[system].imports}
    assert exports == imported, (exports - imported, imported - exports)
    assert b"dxgi.dll" not in imports, "unqualified imports can recurse into the proxy"
    pe.close()
    with tempfile.TemporaryDirectory(prefix="defiance-proxy-") as folder:
        folder = pathlib.Path(folder)
        shutil.copy2(loader, folder / "dxgi.dll")
        (folder / "caller.rs").write_text(r'''
use std::sync::atomic::{AtomicBool, Ordering};
static CALLED: AtomicBool = AtomicBool::new(false);
#[link(name="dxgi", kind="raw-dylib")]
extern "system" {
    fn PIXGetCaptureState() -> u32;
    fn CreateDXGIFactory1(iid: *const u8, out: *mut *mut core::ffi::c_void) -> i32;
}
#[no_mangle]
pub unsafe extern "system" fn DllMain(_: *mut (), reason: u32, _: *mut ()) -> i32 {
    if reason == 1 {
        let _ = PIXGetCaptureState();
        CALLED.store(true, Ordering::Release);
    }
    1
}
#[no_mangle]
pub unsafe extern "system" fn Probe() -> i32 {
    if !CALLED.load(Ordering::Acquire) { return 0; }
        // IID_IDXGIFactory1; verifies argument/return preservation as well.
        let iid = [0x78,0xae,0x0a,0x77,0x6f,0xf2,0xba,0x4d,0xa8,0x29,0x25,0x3c,0x83,0xd1,0xb3,0x87];
        let mut factory = core::ptr::null_mut();
        if CreateDXGIFactory1(iid.as_ptr(), &mut factory) != 0 || factory.is_null() { return 0; }
        let vtable = *(factory as *const *const usize);
        let release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32 = core::mem::transmute(*vtable.add(2));
        release(factory);
    1
}
''', encoding="utf-8")
        (folder / "runner.rs").write_text(r'''
#[link(name="caller", kind="raw-dylib")]
extern "system" { fn Probe() -> i32; }
#[link(name="kernel32")]
extern "system" { fn GetModuleHandleW(name: *const u16) -> *mut (); }
fn main() {
    assert_eq!(unsafe { Probe() }, 1, "caller DllMain did not forward successfully");
    let local = std::env::current_exe().unwrap().with_file_name("dxgi.dll");
    let local: Vec<u16> = local.to_str().unwrap().encode_utf16().chain(Some(0)).collect();
    assert!(!unsafe { GetModuleHandleW(local.as_ptr()) }.is_null(), "fixture bypassed the local proxy");
}
''', encoding="utf-8")
        for source, flags in [("caller", ["--crate-type=cdylib"]), ("runner", [])]:
            suffix = ".dll" if flags else ".exe"
            subprocess.run(["rustc", str(folder / (source + ".rs")), *flags,
                            "-o", str(folder / (source + suffix))], check=True, timeout=90)
        # Children inherit this; a startup error must fail instead of opening a dialog.
        old = ctypes.windll.kernel32.SetErrorMode(0x8003)
        try:
            result = subprocess.run([str(folder / "runner.exe")], cwd=folder,
                                    capture_output=True, text=True, timeout=20)
        finally:
            ctypes.windll.kernel32.SetErrorMode(old)
        if result.returncode != 0:
            for dll in [folder / "caller.dll", folder / "runner.exe", folder / "dxgi.dll"]:
                image = pefile.PE(str(dll))
                print(dll.name, [image.get_string_at_rva(i.struct.Name) for i in image.DIRECTORY_ENTRY_IMPORT]); image.close()
        assert result.returncode == 0, (result.returncode, result.stdout, result.stderr)
    print("Proxy import table and forwarding from another DLL's DllMain passed.")

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("loader", nargs="?", type=pathlib.Path, default=ROOT / "target/release/defiance_loader.dll")
    run(parser.parse_args().loader.resolve())


