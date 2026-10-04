"""Run the sync detour against mapped stock weapon-change machine code.

Fabricated actor/weapon data uses empty attachment vectors and no observers;
the model reorder and animation-script handoff still execute stock code.
No installed game files or running game memory are modified.
"""
import ctypes as C
import argparse
import hashlib
import json
import pathlib
from moving_test_memory import K, KEEP, alloc, p64, i32, f32, check
from capstone.x86 import X86_OP_MEM
from pe import Image

ROOT = pathlib.Path(__file__).resolve().parents[1]
LOGIC = ROOT / "bin/steam/2026-09-25/logic.dll"
DLL = ROOT / "plugins/moving-actions-sync/target/release/defiance_plugin_moving_actions_sync.dll"
STEP = 0x2D8590
WEAPON_DESCRIPTOR_SLOT = 0x160
UNIT_COMPONENTS_SLOT = 0xB0
STEP_FN = C.CFUNCTYPE(C.c_bool, C.c_void_p, C.c_float)
GETTER = C.CFUNCTYPE(C.c_void_p, C.c_void_p)


def getter(obj, slot, result):
    @GETTER
    def callback(_self):
        return result
    KEEP.append(callback)
    vt = C.c_void_p.from_address(obj).value
    p64(vt, slot, C.cast(callback, C.c_void_p).value)


def actor(timer, requested=3, index=1, resource=True, available=True,
          weapon_slot=WEAPON_DESCRIPTOR_SLOT, unit_slot=UNIT_COMPONENTS_SLOT):
    gunner, unit, uvtable = alloc(0x180), alloc(0x100), alloc(0xC0)
    components, visual, animation = alloc(0x80), alloc(0x200), alloc(0x180)
    info, models, weapons = alloc(0x40), alloc(0x160), alloc(0x10)
    old, selected, script = alloc(0x220), alloc(0x220), alloc(0x100)
    p64(unit, 0, uvtable)
    getter(unit, unit_slot, components)
    getter(unit, 0x50, info)
    p64(components, 8, visual)
    p64(components, 0x58, animation)
    p64(visual, 0x108, models)
    p64(visual, 0x110, models + 0x160)
    p64(visual, 0x118, models + 0x160)
    p64(models, 0, old)
    p64(models, 0xB0, selected if available else alloc(0x220))
    p64(selected, 0x1B8, script if resource else 0)
    p64(selected, 0x1C8, script)
    for j, descriptor in enumerate((old, selected)):
        weapon, vtable = alloc(0x80), alloc(0x170)
        p64(weapon, 0, vtable)
        getter(weapon, weapon_slot, descriptor)
        p64(weapons, j * 8, weapon)
    p64(gunner, 0x20, unit)
    p64(gunner, 0x38, weapons)
    p64(gunner, 0x40, weapons + 0x10)
    i32(gunner, 0x94, index)
    i32(gunner, 0x90, 4)
    f32(gunner, 0x9C, timer)
    i32(animation, 0x68, requested)
    i32(animation, 0x6C, 0x18)
    f32(animation, 0xEC, 0.5)
    return gunner, models, animation, selected, old, script


def front(models):
    return C.c_uint64.from_address(models).value


def timer(gunner):
    return C.c_float.from_address(gunner + 0x9C).value


def verify_build_bindings(selected_name):
    rows = json.loads((ROOT / "out/moving-actions-sync-sites.json").read_text())
    check(len(rows) == 4, "expected four supported 2026 bindings")
    for row in rows:
        store, date = row["name"].split("-", 1)
        path = ROOT / "bin" / store / date / "logic.dll"
        data = path.read_bytes()
        check(hashlib.sha256(data).hexdigest() == row["sha"], f"{row['name']} hash")
        pe = int.from_bytes(data[0x3C:0x40], "little")
        image_base = int.from_bytes(data[pe + 24 + 24:pe + 24 + 32], "little")
        # RVA-to-file mapping is supplied by PE section records below.
        sections = pe + 24 + int.from_bytes(data[pe + 20:pe + 22], "little")
        count = int.from_bytes(data[pe + 6:pe + 8], "little")
        table = {}
        for i in range(count):
            off = sections + i * 40
            va = int.from_bytes(data[off + 12:off + 16], "little")
            raw_size = int.from_bytes(data[off + 16:off + 20], "little")
            raw = int.from_bytes(data[off + 20:off + 24], "little")
            table[va] = (raw, raw_size)
        def code(rva, value):
            start = next(raw + rva - va for va, (raw, size) in table.items() if va <= rva < va + size)
            check(data[start:start + len(bytes.fromhex(value))] == bytes.fromhex(value), f"{row['name']} code at {rva:#x}")
        code(row["step"], row["step_bytes"])
        code(row["helper"], row["helper_bytes"])
        image = Image(str(path))
        calls = [i.operands[0].mem.disp for i in image.disasm(row["step"], row["step"] + 0x300)
                 if i.mnemonic == "call" and i.operands and i.operands[0].type == X86_OP_MEM]
        check(UNIT_COMPONENTS_SLOT in calls and WEAPON_DESCRIPTOR_SLOT in calls,
              f"{row['name']} vtable slots differ from tested runtime contract: {calls}")
    selected = next((row for row in rows if row["name"] == selected_name), None)
    check(selected is not None, f"unsupported test build {selected_name}")
    return selected

def main():
    global LOGIC, STEP
    rows = json.loads((ROOT / "out/moving-actions-sync-sites.json").read_text())
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build", choices=[row["name"] for row in rows],
                        default="steam-2026-09-25",
                        help="supported stock logic.dll profile to exercise")
    args = parser.parse_args()
    row = verify_build_bindings(args.build)
    store, date = row["name"].split("-", 1)
    LOGIC = ROOT / "bin" / store / date / "logic.dll"
    STEP = row["step"]
    print(f"Testing native behavior on {row['name']} (step {STEP:#x}, helper {row['helper']:#x})", flush=True)
    base = K.LoadLibraryExW(str(LOGIC), None, 1)
    check(base, "could not map stock logic.dll")
    data = LOGIC.read_bytes()
    pe = int.from_bytes(data[0x3C:0x40], "little")
    size = int.from_bytes(data[pe + 24 + 56:pe + 24 + 60], "little")
    original = STEP_FN(base + STEP)
    logs, hooks = [], []
    module = {"offset": 0, "size": size, "refuse": False, "null": False}
    LOG = C.CFUNCTYPE(None, C.c_uint32, C.c_char_p)
    BASE = C.CFUNCTYPE(C.c_void_p, C.c_char_p)
    SIZE = C.CFUNCTYPE(C.c_size_t, C.c_void_p)
    HOOK = C.CFUNCTYPE(C.c_int, C.c_void_p, C.c_void_p, C.POINTER(C.c_void_p))

    @LOG
    def log(level, text):
        logs.append(text.decode(errors="replace"))

    @BASE
    def module_base(name):
        return base + module["offset"] if name == b"logic.dll" else None

    @SIZE
    def module_size(_base):
        return module["size"]

    @HOOK
    def hook(target, detour, out):
        if module["refuse"]:
            return 1
        if module["null"]:
            return 0
        hooks.append((target, detour))
        out[0] = base + STEP
        return 0

    KEEP.extend((log, module_base, module_size, hook))
    class Api(C.Structure):
        _fields_ = [("abi_version", C.c_uint32), ("reserved", C.c_uint32)] + [
            (n, C.c_void_p) for n in ("log", "module_base", "module_size", "find_pattern",
            "find_pattern_at", "hook", "hook_exact", "hook_call", "unhook", "rtti_method",
            "vtable_slot", "config_get", "patch_bytes")]
    api = Api(5, 0, *[C.cast(fn, C.c_void_p).value if fn else None for fn in
        (log, module_base, module_size, None, None, hook, None, None, None, None, None, None, None)])
    class Plugin(C.Structure):
        _fields_ = [("abi_version", C.c_uint32), ("name", C.c_char_p), ("version", C.c_char_p),
                    ("init", C.c_void_p), ("stop", C.c_void_p)]
    lib = C.CDLL(str(DLL))
    lib.defiance_plugin.restype = C.c_void_p
    plugin = C.cast(lib.defiance_plugin(), C.POINTER(Plugin)).contents
    check(plugin.name == b"defiance.moving-actions-sync" and plugin.abi_version == 5, "metadata")
    init = C.CFUNCTYPE(C.c_int, C.POINTER(Api))(plugin.init)
    check(init(None) != 0, "null API accepted")
    for field, value in (("abi_version", 4), ("reserved", 1)):
        bad = Api.from_buffer_copy(api)
        setattr(bad, field, value)
        check(init(C.byref(bad)) != 0, f"bad {field} accepted")
    module["size"] = STEP
    check(init(C.byref(api)) != 0, "small image accepted")
    module.update(size=size - 0x1000, offset=0x1000)
    check(init(C.byref(api)) != 0, "changed live bytes accepted")
    module.update(size=size, offset=0, refuse=True)
    check(init(C.byref(api)) != 0, "refused hook accepted")
    module.update(refuse=False, null=True)
    check(init(C.byref(api)) != 0, "null trampoline accepted")
    module["null"] = False
    check(init(C.byref(api)) == 0, f"valid init refused: {logs}")
    check(len(hooks) == 1 and hooks[0][0] == base + STEP, "hook target/count")
    detour = STEP_FN(hooks[0][1])

    # Reproduce the stock issue using the actual helper, then exercise the
    # built plugin with exactly the same input and stock original callback.
    g, models, animation, selected, old, script = actor(2.0)
    check(original(g, 2.1), "stock hitch did not complete")
    check(front(models) == old, "stock hitch unexpectedly updated model")
    stock_timer = timer(g)
    print("PASS stock machine code reproduces completed switch with stale held weapon", flush=True)
    g, models, animation, selected, old, script = actor(2.0)
    check(detour(g, 2.1), "repaired hitch did not complete")
    check(front(models) == selected, "hitch handoff missing")
    check(C.c_uint64.from_address(models + 0xB0).value == old, "stock record reorder missing")
    check(C.c_uint64.from_address(animation + 0xB8).value == script, "animation script not updated")
    check(timer(g) == stock_timer, "repair changed completion timer")
    check(C.c_int32.from_address(g + 0x90).value == 4, "repair changed gunner state")
    check(any("repaired=true" in line for line in logs), "missing repair diagnostic")
    print("PASS sync repairs hitch using stock model reorder and animation-script handoff", flush=True)

    for remaining, dt, done, swapped in ((2.0, .1, False, False),
                                       (1.0, .1, False, True), (.5, .6, True, True)):
        g, models, animation, selected, old, script = actor(remaining)
        check(detour(g, dt) == done, "normal completion changed")
        check(front(models) == (selected if swapped else old), "normal handoff changed")
        check(abs(timer(g) - (remaining - dt)) < 1e-6, "normal timer changed")
    # A completed handoff stays idempotent on a later completion call.
    g, models, animation, selected, old, script = actor(.5)
    check(detour(g, .6) and detour(g, 0.0), "repeated completion changed")
    check(front(models) == selected and C.c_uint64.from_address(models + 0xB0).value == old,
          "completed handoff swapped twice")
    for requested in (0x17, 0x2B):
        g, models, animation, selected, old, script = actor(.5, requested=requested)
        check(not detour(g, 2.1) and timer(g) == .5 and front(models) == old,
              "active grenade action no longer blocks switch")
    for index in (-1, 2):
        g, models, animation, selected, old, script = actor(.5, index=index)
        check(not detour(g, 2.1), "invalid selection completed")
        check(C.c_int32.from_address(g + 0x90).value == 6, "stock invalid state changed")
    for kwargs in ({"resource": False}, {"available": False}):
        g, models, animation, selected, old, script = actor(2.0, **kwargs)
        check(detour(g, 2.1) and front(models) == old, "unsupported model forced")
    print("PASS normal switches, repeat completion, grenade gates, invalid and absent models, ABI/signature/hook guards")
    K.FreeLibrary(base)


if __name__ == "__main__":
    main()
