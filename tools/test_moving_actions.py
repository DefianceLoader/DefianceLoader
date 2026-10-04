"""Exercise the moving-actions plugin against mapped stock logic.dll builds.

The test maps DLLs without loading game dependencies, calls the real chassis
speed getter with a small fabricated facet, then runs the production plugin
through a fake ABI 5 host. No installed game files are modified.
"""
import ctypes as C
import hashlib
import json
import pathlib
import struct
import sys
from ctypes import wintypes as W

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import builds
from pe import Image

ROOT = pathlib.Path(__file__).resolve().parents[1]
DLL = ROOT / "plugins/moving-actions/target/release/defiance_plugin_moving_actions.dll"
SITES = ROOT / "out/moving-actions-sites.json"
K = C.WinDLL("kernel32", use_last_error=True)
K.LoadLibraryExW.argtypes = [W.LPCWSTR, C.c_void_p, W.DWORD]
K.LoadLibraryExW.restype = C.c_void_p
K.FreeLibrary.argtypes = [C.c_void_p]
K.VirtualAlloc.argtypes = [C.c_void_p, C.c_size_t, W.DWORD, W.DWORD]
K.VirtualAlloc.restype = C.c_void_p
K.VirtualProtect.argtypes = [C.c_void_p, C.c_size_t, W.DWORD, C.POINTER(W.DWORD)]
K.FlushInstructionCache.argtypes = [C.c_void_p, C.c_void_p, C.c_size_t]
KEEP = []


def alloc(size):
    p = K.VirtualAlloc(None, max(size, 8), 0x3000, 0x40)
    assert p, C.get_last_error()
    return p


def put(p, data):
    old = W.DWORD()
    assert K.VirtualProtect(p, len(data), 0x40, C.byref(old)), C.get_last_error()
    C.memmove(p, data, len(data))
    ignored = W.DWORD()
    assert K.VirtualProtect(p, len(data), old, C.byref(ignored)), C.get_last_error()
    K.FlushInstructionCache(C.c_void_p(-1), p, len(data))


def f32(p, value=None):
    obj = C.c_float.from_address(p)
    if value is not None:
        obj.value = value
    return obj.value


def speed_matrix(base, speed_rva):
    """Run the actual speed getter for every encoded action and stance.

    Action 3 has a separate path that multiplies the posture-selected speed by
    chassis fields +0x128 (weapon mode) and +0x140 (running factor); those
    multipliers are also checked independently below.
    """
    getter = C.CFUNCTYPE(C.c_float, C.c_void_p)(base + speed_rva)
    chassis, animation, stats, script = alloc(0x200), alloc(0x100), alloc(0x200), alloc(0x400)
    C.c_uint64.from_address(chassis + 0x58).value = animation
    C.c_uint64.from_address(chassis + 0x68).value = stats
    C.c_uint64.from_address(chassis + 0xb0).value = script
    f32(chassis + 0x12c, 10.0)  # current speed ceiling
    f32(chassis + 0x128, 2.0)   # action-3 weapon-mode multiplier
    f32(chassis + 0x140, 3.0)   # action-3 running multiplier
    f32(stats + 0x17c, 0.8)     # ordinary posture speed
    for offset, value in ((0x31c, 0.25), (0x320, 0.40), (0x324, 0.55), (0x328, 0.70)):
        f32(script + offset, value)
    results = {}
    for stance in (1, 2, 3):
        C.c_int32.from_address(animation + 0x74).value = stance
        # Selector 3 uses the posture tables when weapon/mode is active.
        C.c_int32.from_address(animation + 0x7c).value = 1
        for action in range(0x31):
            C.c_int32.from_address(animation + 0x6c).value = action
            results[(action, stance)] = getter(chassis)
    # The active action 3 branch must retain both chassis multipliers.
    C.c_int32.from_address(animation + 0x6c).value = 3
    C.c_int32.from_address(animation + 0x74).value = 1
    one = getter(chassis)
    f32(chassis + 0x128, 1.0)
    f32(chassis + 0x140, 1.0)
    unit = getter(chassis)
    assert abs(one - unit * 6.0) < 1e-5, (one, unit)
    f32(chassis + 0x128, 2.0)
    f32(chassis + 0x140, 3.0)
    return results


def assert_selective(before, after):
    changed = {key for key in before if abs(before[key] - after[key]) > 1e-5}
    expected = {(action, stance) for action in (0x17, 0x18, 0x2b) for stance in (1, 2, 3)}
    assert changed == expected, ("changed action/stance results", sorted(changed), sorted(expected))
    for action in (0x17, 0x18, 0x2b):
        # Stance 1 uses the chassis base speed; 2 and 3 use script-info speeds.
        for stance, expected_speed in enumerate((0.80, 0.40, 0.55), start=1):
            assert abs(after[(action, stance)] - expected_speed) < 1e-5, (action, stance, after[(action, stance)])


def one_build(row):
    path = builds.build(row["name"]).logic.resolve()
    image = Image(path)
    assert hashlib.sha256(path.read_bytes()).hexdigest() == row["sha"]
    original = bytes.fromhex(row["before"])
    assert image.read(row["rva"], len(original)) == original
    base = K.LoadLibraryExW(str(path), None, 1)  # DONT_RESOLVE_DLL_REFERENCES
    assert base, (row["name"], C.get_last_error())
    size = image.pe.OPTIONAL_HEADER.SizeOfImage
    mapped_before = C.string_at(base + row["rva"], len(original))
    assert mapped_before == original, row["name"]
    baseline = speed_matrix(base, row["speed_rva"])

    calls, owned, failure = [], {}, 0
    messages = []

    @C.CFUNCTYPE(None, C.c_uint32, C.c_char_p)
    def log(level, message):
        messages.append((level, message.decode(errors="replace")))

    @C.CFUNCTYPE(C.c_void_p, C.c_char_p)
    def module_base(name):
        assert name == b"logic.dll"
        return base

    @C.CFUNCTYPE(C.c_size_t, C.c_void_p)
    def module_size(module):
        assert module == base
        return size

    @C.CFUNCTYPE(C.c_int, C.c_void_p, C.c_void_p, C.c_void_p, C.c_size_t)
    def patch_bytes(at, expected, replacement, count):
        nonlocal failure
        calls.append((at, count))
        if failure and len(calls) == failure:
            return 1
        old = C.string_at(at, count)
        if old != C.string_at(expected, count) or at in owned:
            return 1
        owned[at] = old
        put(at, C.string_at(replacement, count))
        return 0

    @C.CFUNCTYPE(C.c_int, C.c_void_p)
    def unhook(at):
        if at not in owned:
            return 1
        put(at, owned.pop(at))
        return 0

    class Api(C.Structure):
        _fields_ = [("abi", C.c_uint32), ("reserved", C.c_uint32)] + [
            (name, C.c_void_p) for name in (
                "log", "module_base", "module_size", "find_pattern", "find_pattern_at",
                "hook", "hook_exact", "hook_call", "unhook", "rtti_method",
                "vtable_slot", "config_get", "patch_bytes")]

    api = Api(5, 0, *[C.cast(fn, C.c_void_p) if fn else None for fn in
                      (log, module_base, module_size, None, None, None, None, None,
                       unhook, None, None, None, patch_bytes)])
    lib = C.CDLL(str(DLL))
    lib.defiance_plugin.restype = C.c_void_p
    # The public Plugin layout is ABI, name, version, init, stop.
    class Plugin(C.Structure):
        _fields_ = [("abi", C.c_uint32), ("name", C.c_char_p), ("version", C.c_char_p),
                    ("init", C.c_void_p), ("stop", C.c_void_p)]

    plugin = C.cast(lib.defiance_plugin(), C.POINTER(Plugin)).contents
    assert plugin.abi == 5 and plugin.name == b"defiance.moving-actions"
    assert plugin.version == b"0.1.0"
    assert plugin.stop is None
    init = C.CFUNCTYPE(C.c_int, C.POINTER(Api))(plugin.init)
    assert init(None) != 0
    wrong_abi = Api.from_buffer_copy(api)
    wrong_abi.abi = 4
    assert init(C.byref(wrong_abi)) != 0
    assert not calls

    # A live-byte mismatch must be refused before the first patch request.
    tamper_at = base + row["rva"] + row["offsets"][0]
    saved = C.string_at(tamper_at, 1)
    put(tamper_at, b"\x00")
    assert init(C.byref(api)) != 0 and not calls
    put(tamper_at, saved)

    # Fail each write in turn; the host owns rollback after init returns.
    for index in (1, 2, 3):
        failure = index
        calls.clear()
        assert init(C.byref(api)) != 0
        for address, previous in list(owned.items()):
            put(address, previous)
        owned.clear()
        assert C.string_at(base + row["rva"], len(original)) == original
    failure = 0
    calls.clear()
    assert init(C.byref(api)) == 0, messages
    expected_addresses = {base + row["rva"] + offset for offset in row["offsets"]}
    assert set(owned) == expected_addresses
    assert len(calls) == 3 and all(length == 1 for _, length in calls)
    assert [C.string_at(address, 1) for address in sorted(owned)] == [b"\x03"] * 3
    changed = speed_matrix(base, row["speed_rva"])
    assert_selective(baseline, changed)

    # Simulate the loader's owned-patch cleanup and verify exact restoration.
    for address in list(owned):
        assert unhook(address) == 0
    assert not owned
    assert C.string_at(base + row["rva"], len(original)) == original
    assert speed_matrix(base, row["speed_rva"]) == baseline
    K.FreeLibrary(base)
    print(f"PASS {row['name']}: real speed getter, 0x00..0x30 x 3 stances, selective action changes, 3 writes and rollback", flush=True)


def main():
    if sys.platform != "win32":
        raise SystemExit("moving-actions integration harness requires Windows x64")
    if not DLL.is_file():
        raise SystemExit(f"missing release plugin: {DLL}")
    if not SITES.is_file():
        raise SystemExit(f"missing generated bindings: {SITES}; run tools/moving_actions_bindings.py")
    rows = json.loads(SITES.read_text(encoding="utf-8"))
    assert rows
    for row in rows:
        if builds.build(row["name"]).logic.is_file():
            one_build(row)


if __name__ == "__main__":
    main()
