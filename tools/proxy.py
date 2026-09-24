"""Emit the loader's proxy DLL forwarders.

The loader is a *proxy*: it takes the place of a system DLL the game imports,
forwards every export to the real one in System32, and starts the mod host on
the way. Which system DLL is decided by reading `trm.exe`'s import table; this
tool picks one, reads the real DLL's export table, and writes
`crates/loader/src/proxy_generated.rs`.

Each export becomes a *naked* forwarder: a function whose whole body is

    jmp qword ptr [rip + slot]

A tail jump preserves every register and stack word, so no argument types are
needed — which is the whole point, because an export table has none. (A `.def`
forwarder would resolve the module name through the normal search and land back
on this proxy, so it is not usable here.)

Only one export, the *anchor*, is imported at load time: enough for Windows to
load and initialize the real DLL before this one, and old enough to exist on
every supported Windows. The loader's DllMain fills every other slot from the
real DLL with GetProcAddress before any module can call the proxy. The export
list comes from this machine's System32; an export an older Windows lacks keeps
a fallback that returns E_NOTIMPL, instead of failing the whole load (importing
every export at load time made the game unable to start on Windows 10, whose
dxgi.dll lacks Windows 11's DXGIDisableVBlankVirtualization).

    python tools/proxy.py --scan BIN_DIR --list   what could be proxied, and by whom
    python tools/proxy.py --scan BIN_DIR          pick the best, write the file
    python tools/proxy.py --scan BIN_DIR --dll dxgi.dll
    python tools/proxy.py --dll dxgi.dll          no game: proxy that DLL as a whole
    python tools/proxy.py --dll X.dll --anchor F  a DLL without a known anchor

Scan the whole game directory, not just `trm.exe`: the friendly system DLLs are
imported by a graphics, sound or network module (`world2.dll` pulls `dxgi.dll`,
`sound.dll` pulls `winmm.dll`), not the executable.
"""
import argparse
import os
import sys

import pefile

from rustfmt import rustfmt

DEFAULT_EXE = os.path.join("bin", "trm.exe")
DEFAULT_OUT = os.path.join("crates", "loader", "src", "proxy_generated.rs")

# Proxying these is safe and common; earlier in the list wins.
PREFERRED = [
    "version.dll",
    "dxgi.dll",
    "winmm.dll",
    "dinput8.dll",
    "dinput.dll",
    "xinput1_4.dll",
    "xinput1_3.dll",
    "winhttp.dll",
    "dbghelp.dll",
    "wtsapi32.dll",
    "d3d9.dll",
    "d3d11.dll",
    "dsound.dll",
    "avrt.dll",
]

# The game's own modules, loaded at startup. A target these import is reached
# early and unconditionally; `libcef.dll` and the browser helpers are not in
# this set, so a DLL only Chromium imports (version.dll) can load late or not
# at all and is ranked below one the engine pulls in.
CORE = {
    "trm.exe", "logic.dll", "game.dll", "world2.dll", "sound.dll", "platform.dll",
    "ml.dll", "mll_core.dll", "storage.dll", "essence.dll", "editor.dll",
    "logicbox.dll", "psyfx.dll",
}


# The load-time anchor per proxy target: an export every supported Windows
# (10 and later) has, imported so Windows loads the real DLL before this one.
ANCHORS = {
    "dxgi.dll": "CreateDXGIFactory",
    "version.dll": "GetFileVersionInfoW",
    "winmm.dll": "timeGetTime",
    "dinput8.dll": "DirectInput8Create",
    "xinput1_4.dll": "XInputGetState",
    "xinput1_3.dll": "XInputGetState",
    "winhttp.dll": "WinHttpOpen",
    "dbghelp.dll": "SymInitialize",
    "d3d9.dll": "Direct3DCreate9",
    "d3d11.dll": "D3D11CreateDevice",
    "dsound.dll": "DirectSoundCreate",
}


# The C++ runtime is technically proxyable but a poor first choice: its exports
# are many and some are data, which a jump thunk cannot forward. Out unless the
# name is asked for with --dll.
RUNTIME = ("msvcp", "vcruntime", "concrt", "ucrtbase", "msvcrt", "vccorlib")


# KnownDLLs are resolved before the application directory, so a file next to
# the exe never wins for these; a known-dll name is not a candidate at all.
def known_dlls():
    try:
        import winreg
    except ImportError:
        return set()
    try:
        key = winreg.OpenKey(
            winreg.HKEY_LOCAL_MACHINE,
            r"SYSTEM\CurrentControlSet\Control\Session Manager\KnownDLLs",
        )
    except OSError:
        return set()
    names = set()
    index = 0
    while True:
        try:
            _name, value, _type = winreg.EnumValue(key, index)
        except OSError:
            break
        names.add(str(value).lower())
        index += 1
    return names


def system32():
    root = os.environ.get("SystemRoot", r"C:\Windows")
    return os.path.join(root, "System32")


def imports(exe):
    """{dll_name_lower: [imported name or #ordinal, ...]} for a module."""
    pe = pefile.PE(exe, fast_load=True)
    pe.parse_data_directories(
        directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"]]
    )
    found = {}
    for entry in getattr(pe, "DIRECTORY_ENTRY_IMPORT", []):
        name = entry.dll.decode("ascii", "replace").lower()
        names = found.setdefault(name, [])
        for imp in entry.imports:
            names.append(imp.name.decode("ascii", "replace") if imp.name else f"#{imp.ordinal}")
    return found


def proxyable(name, known, sysdir):
    """Whether a system DLL of this name could stand in as a proxy target."""
    if name.startswith("api-ms-win-") or name.startswith("ext-ms-win-"):
        return False
    if name in known or name.startswith(RUNTIME):
        return False
    return os.path.isfile(os.path.join(sysdir, name))


def rank(names):
    """Best first: the known-good proxies in order, then alphabetically."""
    return sorted(
        names,
        key=lambda name: (PREFERRED.index(name) if name in PREFERRED else len(PREFERRED), name),
    )


def candidates(exe):
    """One module's imports that may be proxied, best first. An exe alone is a
    weak source: the friendly DLLs are usually imported by a graphics, sound or
    network module, not the executable, which is why scanning the directory is
    the default."""
    known, sysdir = known_dlls(), system32().lower()
    return rank(name for name in imports(exe) if proxyable(name, known, sysdir))


def rank_imported(found):
    """Best first for a directory scan: a target a core module imports, then
    the known-good order."""
    def core(name):
        return bool(CORE & found.get(name, set()))

    return sorted(
        found,
        key=lambda name: (
            0 if core(name) else 1,
            PREFERRED.index(name) if name in PREFERRED else len(PREFERRED),
            name,
        ),
    )


def importers_in_dir(directory):
    """{dll_name_lower: {module, ...}} across every PE in `directory`, and how
    many modules were read."""
    found = {}
    modules = 0
    for name in sorted(os.listdir(directory)):
        if not name.lower().endswith((".dll", ".exe")):
            continue
        modules += 1
        try:
            for dll in imports(os.path.join(directory, name)):
                found.setdefault(dll, set()).add(name)
        except Exception as error:  # a non-PE, a packed file, ...
            print(f"  ! {name}: {error}")
    return modules, found


def exports(path):
    """[(name, forwarder_or_None)] for every named export, sorted by name."""
    pe = pefile.PE(path, fast_load=True)
    pe.parse_data_directories(
        directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_EXPORT"]]
    )
    directory = getattr(pe, "DIRECTORY_ENTRY_EXPORT", None)
    if directory is None:
        return [], 0
    named, ordinal_only = [], 0
    for symbol in directory.symbols:
        if symbol.name is None:
            ordinal_only += 1
            continue
        forwarder = symbol.forwarder
        named.append((symbol.name.decode("ascii", "replace"), forwarder))
    named.sort()
    return named, ordinal_only


def identifier(name, used):
    """A unique Rust identifier for an export name."""
    stem = "".join(c if c.isalnum() else "_" for c in name)
    if not stem or stem[0].isdigit():
        stem = "x" + stem
    candidate = stem
    suffix = 2
    while candidate in used:
        candidate = f"{stem}_{suffix}"
        suffix += 1
    used.add(candidate)
    return candidate


HEADER = """//! Generated by tools/proxy.py; do not edit by hand.
//! Naked tail jumps through slots filled from the real System32 DLL. Only
//! `ANCHOR` is imported at load time, so Windows loads and initializes the real
//! DLL before this one; DllMain then fills every slot with GetProcAddress
//! (`proxy::resolve`) before any other module can call these exports. No
//! resolver, allocation or LoadLibrary runs on the call path. An export this
//! Windows lacks keeps `proxy::missing`, which returns E_NOTIMPL.
//! Regenerate: python tools/proxy.py --dll {real}
//! {count} named exports{notes}
#![allow(non_upper_case_globals)]
use crate::proxy::{{missing, Target}};
use core::arch::naked_asm;
pub const REAL: &str = "{real}";
/// Read by build/proxy_imports.rs, which imports only this export.
#[allow(dead_code)]
pub const ANCHOR: &str = "{anchor}";

#[link(name = "defiance_system_proxy")]
extern "system" {{
    /// The anchor, imported as data: rustc reaches it through its import slot,
    /// which Windows binds at load time, so its address is the real export's
    /// and locates the real module for `proxy::resolve`.
    #[link_name = "{anchor}"]
    pub static ANCHOR_EXPORT: u8;
}}
"""
ENTRY = '''
static mut SLOT_{ident}: Target = missing;
#[unsafe(naked)]
#[allow(non_snake_case)]
#[export_name = "{name}"]
pub unsafe extern "system" fn f_{ident}() {{
    naked_asm!("jmp qword ptr [rip + {{slot}}]", slot = sym SLOT_{ident});
}}
'''
SLOTS = '''
/// Every export's NUL-terminated name and slot, for `proxy::resolve`.
///
/// # Safety
/// The slots are written only by `proxy::resolve`, under the loader lock.
pub unsafe fn slots() -> [(&'static [u8], *mut Target); {count}] {{
    [
{rows}
    ]
}}
'''


def emit(real, anchor, named, ordinal_only, forwarders, out):
    used = set()
    entries, rows = [], []
    for name, _forwarder in named:
        ident = identifier(name, used)
        entries.append(ENTRY.format(ident=ident, name=name))
        rows.append(f'        (b"{name}\\0", &raw mut SLOT_{ident}),')
    notes = []
    if ordinal_only:
        notes.append(f"; {ordinal_only} ordinal-only, not exported")
    if forwarders:
        notes.append(f"; {forwarders} are themselves forwarded, resolved by Windows at load time")
    text = HEADER.format(
        real=real,
        anchor=anchor,
        count=len(named),
        notes=("\n//! " + "\n//! ".join(notes)) if notes else "",
    ) + "\n" + "\n".join(entries) + SLOTS.format(count=len(named), rows="\n".join(rows))
    with open(out, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)
    rustfmt(out)
    return len(named)


def show_directory(directory):
    """Every proxyable system DLL imported by a module in `directory`, best
    first, with its export count and the modules that pull it in."""
    modules, found = importers_in_dir(directory)
    known, sysdir = known_dlls(), system32().lower()
    names = [name for name in rank_imported(found) if proxyable(name, known, sysdir)]
    print(f"{directory}: {modules} modules, {len(names)} proxyable system DLLs")
    for name in names:
        count = len(exports(os.path.join(sysdir, name))[0])
        importers = ", ".join(sorted(found[name]))
        core = "*" if CORE & found[name] else " "
        print(f"  {core} {name:<20} {count:>4} exports  <- {importers}")
    return names


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("path", nargs="?", default=None, help="the game exe or its bin directory")
    parser.add_argument("--exe", default=None, help=f"the game executable (default {DEFAULT_EXE})")
    parser.add_argument("--scan", default=None, metavar="DIR", help="scan every PE in DIR for a target")
    parser.add_argument("--dll", default=None, help="the system DLL to proxy, by name")
    parser.add_argument("--out", default=DEFAULT_OUT, help=f"where to write (default {DEFAULT_OUT})")
    parser.add_argument("--list", action="store_true", help="list the candidates and stop")
    parser.add_argument("--anchor", default=None,
                        help="the export imported at load time (default: the known one for --dll)")
    args = parser.parse_args(argv)

    target = args.scan or args.exe or args.path
    if target is None and os.path.isfile(DEFAULT_EXE):
        target = DEFAULT_EXE

    if args.dll is None:
        # A directory is the better source: the friendly DLLs are usually
        # imported by a graphics, sound or network module, not `trm.exe`.
        if target and os.path.isdir(target):
            found = show_directory(target)
        elif target:
            found = candidates(target)
            print(f"{target} imports {len(imports(target))} DLLs; {len(found)} could be proxied:")
            for name in found:
                print(f"  {'*' if name in PREFERRED else ' '} {name}")
            print("  (scan the whole bin directory with --scan DIR for a better list)")
        else:
            print(f"no {DEFAULT_EXE}; pass a directory, --scan DIR, or --dll NAME")
            return 1
        if args.list:
            return 0
        if not found:
            print("none: every import is a KnownDLL, an API set, or not in System32")
            return 1
        args.dll = found[0]
        print(f"\nchose {args.dll}")

    path = os.path.join(system32(), args.dll)
    if not os.path.isfile(path):
        print(f"{path} is not there; is the name right?")
        return 1
    named, ordinal_only = exports(path)
    if not named:
        print(f"{args.dll} has no named exports")
        return 1
    forwarders = sum(1 for _name, forwarder in named if forwarder)
    anchor = args.anchor or ANCHORS.get(args.dll.lower())
    if anchor is None:
        print(f"no known load-time anchor for {args.dll}; pass --anchor with an export "
              "every supported Windows has")
        return 1
    if anchor not in {name for name, _forwarder in named}:
        print(f"{args.dll} does not export the anchor {anchor}")
        return 1
    written = emit(args.dll, anchor, named, ordinal_only, forwarders, args.out)
    print(f"{path}: {written} named exports ({forwarders} forwarded, {ordinal_only} ordinal-only)")
    print(f"wrote {args.out}; rebuild the loader and place it beside trm.exe as {args.dll}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
