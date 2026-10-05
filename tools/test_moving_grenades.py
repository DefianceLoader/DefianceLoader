"""Native ABI harness for the moving-grenades route-retention plugin.

Maps the selected supported 2026 GOG/Steam logic.dll without imports, and installs hooks in
a fake ABI host. The native movement resume routine is replaced only in that
throwaway mapped image by a callback shim; game files are never modified.
"""
import ctypes as C
import pathlib
import struct
import sys
from ctypes import wintypes as W

from moving_test_memory import K, KEEP, alloc, p64, i32, f32, check

ROOT = pathlib.Path(__file__).resolve().parents[1]
import json
import os
import tomllib
import builds

BUILD_NAME = os.environ.get("MOVING_GRENADES_BUILD", "steam-2026-09-25")
if "--build" in sys.argv:
    BUILD_NAME = sys.argv[sys.argv.index("--build") + 1]
PROFILE = next(row for row in json.loads((ROOT / "out/moving-grenades-sites.json").read_text())
               if row["name"] == BUILD_NAME)
MAPPING = {int(key, 16): value for key, value in
           {**PROFILE["mapping"], **PROFILE["test_mapping"]}.items()}

def native_rva(reference):
    if reference == 0x46A741:  # Deliberately invalid getter, one byte past the mapped entry.
        return MAPPING[0x46A740] + 1
    return MAPPING[reference]

DLL = ROOT / "plugins/moving-grenades/target/release/defiance_plugin_moving_grenades.dll"
VERSION = tomllib.loads((ROOT / "plugins/moving-grenades/Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
LOGIC = builds.build(BUILD_NAME).require().logic
SITES = (native_rva(0x2CABB0), native_rva(0x2CC9E0), native_rva(0x2CCAF0), native_rva(0x2C28F0), native_rva(0xD9470), native_rva(0x9E960),
         native_rva(0x2D8350), native_rva(0x299330), native_rva(0x2996E0), native_rva(0x2CCBE0), native_rva(0x2D9640), native_rva(0x2CC990),
         native_rva(0x333239), native_rva(0x2BAFA0), native_rva(0x103250), native_rva(0x2BC400))
PREFIXES = tuple(bytes.fromhex(s) for s in (
    "48895c2408574883ec200fb6fa488bd9",
    "48895c2408574883ec20488bfa488bd9",
    "48895c2408574883ec20488bfa488bd9",
    "48895c2408574883ec2048837928008b",
    "4889542410555356574156488bec4883ec",
    "48895c240848895424105556574154415541564157",
    "405341544883ec384532e4488bd94883",
    "48895c24084889742410574881ece000",
    "40534883ec20488b4140488bd98b90c4",
    "48895c2408574883ec30488db984000000",
    "48895c241048896c2418488974242057",
    "40534883ec208b91c4000000488bd983",
    "4863c233d24869c8f002000049034808",
    "48895c24184889542410574883ec30",
    "48895c2418488974242041564883ec20",
    "48895c241048896c2420565741564883ec50",
))
COPYREF = native_rva(0x123F0)
COPYREF_PREFIX = bytes.fromhex("40534883ec20488bd9488b0a48890b48")
RESUME = native_rva(0x2CB7A0)
RESUME_PREFIX = bytes.fromhex("488bc448895818488950105556574154")
CHASSIS_VT = native_rva(0x72C3B0)
ANIMATION_VT = native_rva(0x72C118)
ATTACK_STATE_VT = native_rva(0x705F08)
ATTACK_ORDER_VT = native_rva(0x705D50)
MOVE_ORDER_VT = native_rva(0x70C068)
GUNNER_VTS = (native_rva(0x72CD00), native_rva(0x72CED0))
SELECTOR = native_rva(0x2D8350)
GUN_ELIGIBLE = native_rva(0x299330)
GUN_REQUIRES_IDLE = native_rva(0x2996E0)
GRENADE_CANDIDATE = native_rva(0x2D9640)
CHASSIS_CAN_MOVE = native_rva(0x2CC990)
CANDIDATE_CONTINUATION = native_rva(0x2D9744)
CANDIDATE_CONTINUATION_PREFIX = bytes.fromhex("84c00f84cb020000")
FLARE_TARGET_VT = native_rva(0x70F430)
FLARE_POINT_GETTER = native_rva(0x116D80)
FLARE_POINT_GETTER_PREFIX = bytes.fromhex("488d4110c3")
PATH_CANCEL_ENTRY = native_rva(0x333220)
PATH_CANCEL_ENTRY_PREFIX = bytes.fromhex("85d20f889a0000004c8b8188020000413b5004")
PATH_CANCEL_RVA = native_rva(0x333239)
PATH_CANCEL_PREFIX = bytes.fromhex("4863c233d24869c8f002000049034808")
SUBMIT_RVA = native_rva(0x2BAFA0)
SUBMIT_PREFIX = bytes.fromhex("48895c24184889542410574883ec30")
RELEASE_REF = native_rva(0x24070)
COPYREF = native_rva(0x123F0)
RELEASE_REF_PREFIX = bytes.fromhex("40534883ec20488bd9")
SUBMIT_CALLER = native_rva(0x2D9138)
SUBMIT_CALLER_PREFIX = bytes.fromhex("0f57d2488d5567488bcbffd7")
GUNNER_CLEAR_TARGET = native_rva(0x2D9DC0)
GUNNER_CLEAR_PREFIX = bytes.fromhex("48895c24104889742418574883ec20")
MOVE_ORDER_FACTORY = native_rva(0x114FB0)
MOVE_ORDER_FACTORY_PREFIX = bytes.fromhex("48895c241048896c2418488974242048")
ATTACK_ORDER_FACTORY = native_rva(0x8A0C0)
ATTACK_ORDER_FACTORY_PREFIX = bytes.fromhex("48895c24104c8944241848894c240855")
BIND_OBJECT = native_rva(0x240C0)
BIND_OBJECT_PREFIX = bytes.fromhex("48895c24104889742418574883ec2048")
MOVE_FLAGS_SETTER = native_rva(0x762F0)
MOVE_FLAGS_SETTER_PREFIX = bytes.fromhex("095114c3")
AUX_RELEASE_IAT = native_rva(0x6D67E8)
FLARE_MASK_GETTER = native_rva(0x116DD0)

CHECKS = {native_rva(reference): bytes.fromhex(prefix) for reference, prefix in PROFILE["checks"]}
PREFIXES = tuple(CHECKS[site] for site in SITES)
for _name, _reference in {
    "COPYREF_PREFIX": 0x123F0, "RESUME_PREFIX": 0x2CB7A0,
    "CANDIDATE_CONTINUATION_PREFIX": 0x2D9744, "FLARE_POINT_GETTER_PREFIX": 0x116D80,
    "PATH_CANCEL_ENTRY_PREFIX": 0x333220, "PATH_CANCEL_PREFIX": 0x333239,
    "SUBMIT_PREFIX": 0x2BAFA0, "RELEASE_REF_PREFIX": 0x24070,
    "SUBMIT_CALLER_PREFIX": 0x2D912E, "GUNNER_CLEAR_PREFIX": 0x2D9DC0,
    "MOVE_ORDER_FACTORY_PREFIX": 0x114FB0, "ATTACK_ORDER_FACTORY_PREFIX": 0x8A0C0,
    "BIND_OBJECT_PREFIX": 0x240C0, "MOVE_FLAGS_SETTER_PREFIX": 0x762F0,
}.items():
    globals()[_name] = CHECKS[native_rva(_reference)]

K.VirtualProtect.argtypes = [C.c_void_p, C.c_size_t, W.DWORD, C.POINTER(W.DWORD)]
K.VirtualProtect.restype = W.BOOL
K.GetCurrentProcess.restype = C.c_void_p
K.FlushInstructionCache.argtypes = [C.c_void_p, C.c_void_p, C.c_size_t]
K.FlushInstructionCache.restype = W.BOOL


def u8(address, offset=0):
    return C.c_uint8.from_address(address + offset).value


def u32(address, offset=0):
    return C.c_uint32.from_address(address + offset).value


def ptr(address, offset=0):
    return C.c_void_p.from_address(address + offset).value or 0


def put_code(address, code):
    old = W.DWORD()
    check(K.VirtualProtect(address, len(code), 0x40, C.byref(old)),
          f"VirtualProtect failed at {address:#x}: {C.get_last_error()}")
    C.memmove(address, code, len(code))
    restore = W.DWORD()
    check(K.VirtualProtect(address, len(code), old.value, C.byref(restore)),
          f"VirtualProtect restore failed at {address:#x}: {C.get_last_error()}")
    return old.value


def route_fixture(base, stop_detour=None, attack_move=False):
    """Build both owner-AI and HumanAiFacet paths plus one active route record."""
    chassis, animation, components = alloc(0x300), alloc(0x180), alloc(0x80)
    chassis_vt, animation_j, chassis_j = alloc(0x100), alloc(0x40), alloc(0x40)
    unit, unit_vt, unit_j = alloc(0x100), alloc(0x100), alloc(0x40)
    owner, owner_vt, owner_j = alloc(0x100), alloc(0x100), alloc(0x40)
    human_ai = alloc(0x240)
    p64(human_ai, 0x10, unit_j)
    manager, header, records = alloc(0x300), alloc(0x20), alloc(4 * 0x2F0)
    move_j, move_obj = alloc(0x40), alloc(0x40)
    other_move_j, other_move_obj = alloc(0x40), alloc(0x40)
    other_manager, other_header, other_records = alloc(0x300), alloc(0x20), alloc(4 * 0x2F0)
    state, state_j, old_move, old_move_j = alloc(0xA0), alloc(0x40), alloc(0x80), alloc(0x40)
    attack_state, attack_state_j = alloc(0x80), alloc(0x40)
    attack_order, attack_order_j, target, target_j = (alloc(n) for n in (0x80, 0x40, 0x100, 0x40))
    other_target, other_target_j = alloc(0x100), alloc(0x40)
    other_unit, other_unit_vt = alloc(0x100), alloc(0x100)
    incoming = alloc(8)
    gunner, gunner_j = alloc(0x180), alloc(0x40)
    gun_array, gun, gun_vt, descriptor = alloc(16), alloc(0x200), alloc(0x200), alloc(0x200)
    position = alloc(0x20)
    position_facet, position_vt = alloc(0x100), alloc(0x100)
    position_getter = alloc(0x40)
    attack_target_vt = alloc(0x100)

    @C.CFUNCTYPE(C.c_void_p, C.c_void_p)
    def get_components(_unit):
        return components

    @C.CFUNCTYPE(C.c_void_p, C.c_void_p)
    def get_unit(_owner):
        return unit

    @C.CFUNCTYPE(C.c_void_p, C.c_void_p)
    def get_descriptor(_gun):
        return ptr(_gun, 0x1A0)

    KEEP.extend((get_components, get_unit, get_descriptor))
    p64(chassis, 0, base + CHASSIS_VT)
    p64(chassis, 0x10, chassis_j); p64(chassis_j, 0x10, unit)
    p64(chassis, 0x30, manager); p64(chassis, 0xD8, move_j)
    p64(chassis, 0x48, position_facet)
    i32(chassis, 0xEC, 1); i32(chassis, 0xC4, 0)
    C.c_uint8.from_address(chassis + 0x18).value = 1
    C.c_uint8.from_address(chassis + 0x28).value = 1
    C.c_uint8.from_address(chassis + 0x10B).value = 0
    f32(chassis, 0x12C, 1.25)
    f32(chassis, 0x110, 0.0); f32(chassis, 0x114, 0.0)
    p64(unit, 0, unit_vt); p64(unit_vt, 0xB0, C.cast(get_components, C.c_void_p).value)
    p64(unit_j, 0x10, unit)
    p64(components, 0, position_facet); p64(components, 0x38, chassis)
    p64(components, 0x28, human_ai); p64(components, 0x58, animation)
    p64(position_facet, 0, position_vt); p64(position_vt, 0x58, position_getter)
    put_code(position_getter, b"\x48\xb8" + position.to_bytes(8, "little") + b"\xc3")
    p64(animation, 0, base + ANIMATION_VT); p64(animation, 0x10, animation_j)
    p64(animation_j, 0x10, unit)
    p64(chassis, 0x58, animation)
    p64(owner, 0, owner_vt); p64(owner_vt, 0x50, C.cast(get_unit, C.c_void_p).value)
    p64(owner_j, 0x10, owner); p64(state, 0x10, state_j); p64(state_j, 0x10, owner)
    p64(old_move_j, 0x10, old_move); p64(state, 0x20, old_move_j)
    i32(old_move_j, 8, 1); i32(old_move_j, 0x18, 1)
    p64(old_move, 0, base + (ATTACK_ORDER_VT if attack_move else MOVE_ORDER_VT))
    p64(attack_state_j, 0x10, attack_state); p64(attack_state, 0, base + ATTACK_STATE_VT)
    p64(attack_state, 0x20, attack_order_j); p64(attack_order_j, 0x10, attack_order)
    p64(attack_order, 0, base + ATTACK_ORDER_VT); p64(attack_order, 0x28, target_j)
    p64(target_j, 0x10, target); p64(incoming, 0, attack_state_j)
    p64(target, 0, attack_target_vt); p64(attack_target_vt, 0x28, base + native_rva(0x46A740))
    i32(target, 0x10, 1)
    p64(other_target_j, 0x10, other_target)

    p64(manager, 0x288, header); i32(header, 4, 4); p64(header, 8, records)
    record = records
    p64(record, 0x258, chassis); C.c_uint8.from_address(record + 1).value = 1
    C.c_uint8.from_address(record + 0x2CC).value = 2

    p64(move_j, 8, 8); p64(move_j, 0x10, move_obj); i32(move_j, 0x18, 18)
    p64(move_obj, 0x10, 1)  # Non-null auxiliary reference required by native copyref.
    p64(other_move_j, 8, 8); p64(other_move_j, 0x10, other_move_obj)
    p64(other_move_obj, 0x10, 1)
    p64(other_manager, 0x288, other_header); i32(other_header, 4, 4); p64(other_header, 8, other_records)
    p64(other_records, 0x258, chassis); C.c_uint8.from_address(other_records + 1).value = 1
    C.c_uint8.from_address(other_records + 0x2CC).value = 0
    p64(other_unit, 0, other_unit_vt); p64(other_unit_vt, 0xB0, C.cast(get_components, C.c_void_p).value)

    p64(human_ai, 0x1F0, gunner_j); p64(gunner_j, 0x10, gunner)
    p64(gunner, 0, base + GUNNER_VTS[0]); p64(gunner, 0x20, unit)
    p64(gunner, 0x38, gun_array); p64(gunner, 0x40, gun_array + 8)
    i32(gunner, 0x90, 3); i32(gunner, 0x94, 0); p64(gunner, 0x88, target_j)
    p64(gun_array, 0, gun); p64(gun, 0, gun_vt)
    p64(gun_vt, 0x160, C.cast(get_descriptor, C.c_void_p).value)
    p64(gun, 0x1A0, descriptor)
    p64(gun_vt, 0x1C0, base + GUN_ELIGIBLE)
    p64(gun_vt, 0x1F0, base + GUN_REQUIRES_IDLE)
    C.c_uint8.from_address(descriptor + 0xA9).value = 1; i32(descriptor, 0xC4, 3)
    i32(animation, 0x68, 0); i32(animation, 0x6C, 0)

    return locals()


def main():
    reject_manager_trace = "--reject-manager-trace" in sys.argv[1:]
    reject_completion = "--reject-completion" in sys.argv[1:]
    if sys.platform != "win32" or C.sizeof(C.c_void_p) != 8:
        raise SystemExit("moving-grenades test requires Windows x64")
    check(DLL.is_file(), f"missing release plugin: {DLL}")
    check(LOGIC.is_file(), f"missing supported logic.dll: {LOGIC}")
    base = K.LoadLibraryExW(str(LOGIC), None, 1)
    check(base, f"LoadLibraryExW failed: {C.get_last_error()}")
    image = LOGIC.read_bytes(); pe = int.from_bytes(image[0x3C:0x40], "little")
    size = int.from_bytes(image[pe + 24 + 56:pe + 24 + 60], "little")
    for rva, prefix in zip(SITES, PREFIXES):
        check(C.string_at(base + rva, len(prefix)) == prefix, f"hook prefix differs at {rva:#x}")
    check(C.string_at(base + COPYREF, len(COPYREF_PREFIX)) == COPYREF_PREFIX, "copyref prefix differs")
    check(C.string_at(base + RESUME, len(RESUME_PREFIX)) == RESUME_PREFIX, "resume prefix differs")
    check(C.string_at(base + FLARE_POINT_GETTER, len(FLARE_POINT_GETTER_PREFIX)) ==
          FLARE_POINT_GETTER_PREFIX, "native flare-target point getter differs")
    for rva, prefix, label in (
        (MOVE_ORDER_FACTORY, MOVE_ORDER_FACTORY_PREFIX, "ordinary Move factory"),
        (ATTACK_ORDER_FACTORY, ATTACK_ORDER_FACTORY_PREFIX, "AttackMove factory"),
        (BIND_OBJECT, BIND_OBJECT_PREFIX, "native object binder"),
        (MOVE_FLAGS_SETTER, MOVE_FLAGS_SETTER_PREFIX, "Move flags setter"),
    ):
        check(C.string_at(base + rva, len(prefix)) == prefix,
              f"native {label} prefix differs")

    logs, hooks, unhooks, calls, state_chassis = [], [], [], [], {}
    detours = {}
    callbacks = []
    LOG = C.CFUNCTYPE(None, C.c_uint32, C.c_char_p)
    MODULE_BASE = C.CFUNCTYPE(C.c_void_p, C.c_char_p)
    MODULE_SIZE = C.CFUNCTYPE(C.c_size_t, C.c_void_p)
    HOOK = C.CFUNCTYPE(C.c_int, C.c_void_p, C.c_void_p, C.POINTER(C.c_void_p))
    UNHOOK = C.CFUNCTYPE(C.c_int, C.c_void_p)
    STOP = C.CFUNCTYPE(None, C.c_void_p, C.c_uint8)
    STOP_INIT = C.CFUNCTYPE(None, C.c_void_p)
    AI_UPDATE = C.CFUNCTYPE(None, C.c_void_p, C.c_float)
    TURN = C.CFUNCTYPE(None, C.c_void_p, C.POINTER(C.c_float))
    QUEUE = C.CFUNCTYPE(None, C.c_void_p, C.c_int32)
    ORDER = C.CFUNCTYPE(None, C.c_void_p, C.POINTER(C.c_void_p))
    SELECTOR = C.CFUNCTYPE(C.c_uint8, C.c_void_p)
    GUN_QUERY = C.CFUNCTYPE(C.c_uint64, C.c_void_p)
    CANDIDATE = C.CFUNCTYPE(C.c_uint64, C.c_void_p, C.c_size_t, C.c_uint32, C.c_uint32)
    CHASSIS_QUERY = C.CFUNCTYPE(C.c_uint64, C.c_void_p)
    PATH_CANCEL = C.CFUNCTYPE(C.c_uint64, C.c_void_p, C.c_int32, C.c_void_p)
    SUBMIT = C.CFUNCTYPE(None, C.c_void_p, C.POINTER(C.c_void_p), C.c_float)
    WEAPON_CLEAR = C.CFUNCTYPE(None, C.c_void_p, C.POINTER(C.c_void_p))
    AUX_RELEASE = C.CFUNCTYPE(None, C.c_void_p)
    HELPER = C.CFUNCTYPE(C.c_uint64, C.c_void_p, C.c_void_p, C.c_float)
    MOVE_FACTORY = C.CFUNCTYPE(C.c_void_p, C.c_void_p, C.c_void_p, C.c_uint32, C.c_void_p)
    ATTACK_FACTORY = C.CFUNCTYPE(C.c_void_p, C.c_void_p, C.c_void_p, C.POINTER(C.c_void_p))
    BIND = C.CFUNCTYPE(None, C.POINTER(C.c_void_p), C.c_void_p)

    @LOG
    def log(level, text):
        logs.append((level, text.decode(errors="replace")))

    @MODULE_BASE
    def module_base(name):
        return base if name == b"logic.dll" else None

    @MODULE_SIZE
    def module_size(module):
        return size if module == base else 0

    @STOP
    def original_stop(chassis, force):
        calls.append(("stop", chassis, force))
        i32(chassis, 0xEC, 0)
        manager = ptr(chassis, 0x30); header = ptr(manager, 0x288); records = ptr(header, 8)
        nav = C.c_int32.from_address(chassis + 0xC4).value
        if 0 <= nav < C.c_int32.from_address(header + 4).value:
            C.c_uint8.from_address(records + nav * 0x2F0 + 0x2CC).value = 0

    @TURN
    def original_turn(chassis, point):
        calls.append(("turn", chassis, C.cast(point, C.c_void_p).value))

    @QUEUE
    def original_queue(animation, action):
        calls.append(("queue", animation, action))
        i32(animation, 0x68, action)

    @ORDER
    def original_order(state, incoming):
        calls.append(("order", state, C.cast(incoming, C.c_void_p).value,
                      C.c_void_p.from_address(C.cast(incoming, C.c_void_p).value).value))
        chassis = state_chassis.get(state)
        if chassis:
            stop_fn = detours.get(0)
            check(stop_fn is not None, "stop hook missing during original order")
            stop_fn(chassis, 1)  # Initial replacement stop happens inside order forwarding.

    @ORDER
    def original_order_attack(state, incoming):
        calls.append(("order_attack_move", state, C.cast(incoming, C.c_void_p).value,
                      C.c_void_p.from_address(C.cast(incoming, C.c_void_p).value).value))
        chassis = state_chassis.get(state)
        if chassis:
            stop_fn = detours.get(0)
            check(stop_fn is not None, "stop hook missing during original attack-move order")
            stop_fn(chassis, 4)
        nested = nested_orders.get(state)
        if nested is not None:
            f = nested["fixture"]
            # Model the native attack-move callback dispatching an ordinary
            # MoveState order on this same state after its initial stop.
            p64(f["old_move"], 0, base + MOVE_ORDER_VT)
            move_order = detours.get(4)
            check(move_order is not None, "ordinary move-order hook missing in nested callback")
            move_order(state, C.cast(nested["incoming"], C.POINTER(C.c_void_p)))
            p64(f["old_move"], 0, base + ATTACK_ORDER_VT)

    nested_orders = {}
    selector_outputs, selector_configs, selector_fixtures = {}, {}, {}
    selector_snapshots = []
    eligible_outputs, requires_idle_outputs = {}, {}
    @SELECTOR
    def original_selector(gunner):
        calls.append(("selector", gunner))
        f = selector_fixtures.get(gunner)
        config = selector_configs.get(gunner, {})
        if f is not None:
            selector_snapshots.append((gunner, i32_read(f["chassis"], 0xEC),
                                       u8(f["records"], 0x2CC), i32_read(gunner, 0xB8),
                                       selector_outputs.get(gunner, 0)))
        if f is not None and config.get("call_eligible", True):
            eligible_fn = detours.get(7)
            check(eligible_fn is not None, "gun eligibility hook missing during selector original")
            calls.append(("selector_eligible_result", eligible_fn(f["gun"])))
        if f is not None and config.get("call_requires_idle", True):
            idle_fn = detours.get(8)
            check(idle_fn is not None, "requires-idle hook missing during selector original")
            calls.append(("selector_requires_idle_result", idle_fn(f["gun"])))
        if f is not None and config.get("change_selected") is not None:
            i32(gunner, 0x94, config["change_selected"])
        if f is not None and config.get("change_state") is not None:
            i32(gunner, 0x90, config["change_state"])
        return selector_outputs.get(gunner, 0)

    @GUN_QUERY
    def original_eligible(gun):
        calls.append(("eligible", gun))
        return eligible_outputs.get(gun, 0)

    @GUN_QUERY
    def original_requires_idle(gun):
        calls.append(("requires_idle", gun))
        return requires_idle_outputs.get(gun, 0)

    helper_calls = []
    helper_result = 0xA5A55AA512345678
    @HELPER
    def original_helper(chassis, direction, dt):
        values = None if not direction else (
            C.c_float.from_address(direction).value,
            C.c_float.from_address(direction + 4).value)
        helper_calls.append((chassis, direction, float(dt), values))
        return helper_result

    candidate_calls, candidate_results, candidate_readiness = [], {}, {}
    candidate_ready_calls, candidate_ready_results = [], {}
    candidate_fixtures, candidate_nested = {}, {}
    candidate_detour = candidate_can_move_detour = None
    invoke_candidate_readiness = None

    @CANDIDATE
    def original_candidate(gunner, index, target_arg, flags):
        candidate_calls.append((gunner, index, target_arg, flags))
        nested = candidate_nested.get((gunner, index))
        if nested is not None:
            nested()
        fixture = candidate_fixtures.get(gunner)
        if fixture is not None:
            # Native candidate continues through its readiness check and then
            # its independently controlled range/suitability tail.
            candidate_readiness[(gunner, index)] = invoke_candidate_readiness(fixture["chassis"])
        return candidate_results.get((gunner, index), 0)

    @CHASSIS_QUERY
    def original_chassis_can_move(chassis):
        candidate_ready_calls.append(chassis)
        return candidate_ready_results.get(chassis, 0)

    path_cancel_calls, path_cancel_results, path_cancel_fixtures = [], {}, {}
    @PATH_CANCEL
    def original_path_cancel(manager, nav, header):
        path_cancel_calls.append((manager, nav, header))
        fixture = path_cancel_fixtures.get(manager)
        count = i32_read(header, 4) if fixture is not None and header else 0
        if 0 <= nav < count:
            records = ptr(header, 8)
            if records:
                C.c_uint8.from_address(records + nav * 0x2F0 + 0x2CC).value = 0
        return path_cancel_results.get((manager, nav), 0xD1A6000000000001)

    submit_calls, submitted_refs, release_calls = [], [], []
    movement_fixtures_by_unit, fresh_order_junctions = {}, {}
    move_factory_calls, attack_factory_calls, bind_calls = [], [], []
    factory_fail_units, factory_finished_units = set(), set()
    stop_init_fixtures, ai_update_fixtures = {}, {}
    stop_init_calls, ai_update_calls = [], []
    submit_cleanup_snapshots = []
    weapon_clear_calls = []
    @WEAPON_CLEAR
    def clear_weapon_target(weapon, output):
        weapon_clear_calls.append((weapon, (output[0] or 0) if output else 0))
    auxiliary_release_calls, auxiliary_release_errors = [], []
    @AUX_RELEASE
    def release_auxiliary(junction):
        count = i32_read(junction, 0x18) if junction else 0
        auxiliary_release_calls.append((junction or 0, count))
        if not junction or count <= 1:
            auxiliary_release_errors.append((junction or 0, count))
            return
        i32(junction, 0x18, count - 1)
    copyref_calls = []
    @C.CFUNCTYPE(None, C.POINTER(C.c_void_p), C.POINTER(C.c_void_p))
    def copy_reference(output, source):
        junction = source[0] if source else 0
        copyref_calls.append(junction or 0)
        if junction:
            C.c_int32.from_address(junction + 8).value += 1
            C.c_int32.from_address(junction + 0x18).value += 1
        output[0] = junction

    @C.CFUNCTYPE(None, C.POINTER(C.c_void_p))
    def release_ref(reference):
        if reference:
            value = reference[0]
            release_calls.append(value or 0)
            if value:
                C.c_int32.from_address(value + 8).value -= 1
                C.c_int32.from_address(value + 0x18).value -= 1
    @SUBMIT
    def original_submit(ai, incoming, delay):
        junction = incoming[0] if incoming else 0
        obj = ptr(junction, 0x10) if junction else 0
        fixture = ai_update_fixtures.get(ai)
        if fixture and obj and ptr(obj, 0) == base + native_rva(0x70E5D8):
            # Replacing the old native order completes it before the deferred
            # fresh Move is submitted. This is the state that MoveState sees.
            C.c_uint8.from_address(fixture["old_order"] + 0x11).value = 1
        gunner_junction = ptr(ai, 0x1F0) if ai else 0
        current_gunner = ptr(gunner_junction, 0x10) if gunner_junction else 0
        submit_cleanup_snapshots.append((ptr(current_gunner, 0x88) if current_gunner else 0,
                                         ptr(current_gunner, 0x98) if current_gunner else 0,
                                         len(weapon_clear_calls)))
        submit_calls.append((ai, C.cast(incoming, C.c_void_p).value,
                             junction, obj, float(delay)))
        if junction:
            submitted_refs.append(junction)
            release_ref(incoming)

    @MOVE_FACTORY
    def native_move_factory(repo, unit, speed, destination):
        fixture = movement_fixtures_by_unit.get(unit)
        check(fixture is not None, f"ordinary Move factory received unknown unit {unit:#x}")
        check(repo == fixture["repository"], "ordinary Move factory did not receive the unit repository")
        check(destination == fixture["destination_object"],
              "ordinary Move factory did not receive the saved raw destination")
        move_factory_calls.append((repo, unit, speed, destination))
        if unit in factory_fail_units:
            return 0
        fresh = alloc(0x80)
        p64(fresh, 0, base + MOVE_ORDER_VT)
        p64(fresh, 0x18, fixture["unit_j"])
        p64(fresh, 0x48, fixture["destination_j"])
        i32(fresh, 0x60, speed)
        if unit in factory_finished_units:
            C.c_uint8.from_address(fresh + 0x11).value = 1
        fixture["fresh_order"] = fresh
        return fresh

    @ATTACK_FACTORY
    def native_attack_factory(repo, unit, target_ref):
        fixture = movement_fixtures_by_unit.get(unit)
        check(fixture is not None, f"AttackMove factory received unknown unit {unit:#x}")
        check(repo == fixture["repository"], "AttackMove factory did not receive the unit repository")
        target_j = target_ref[0] if target_ref else 0
        check(target_j == fixture["attack_target_j"],
              "AttackMove factory did not receive the saved target junction")
        attack_factory_calls.append((repo, unit, target_j))
        if unit in factory_fail_units:
            return 0
        fresh = alloc(0x80)
        p64(fresh, 0, base + ATTACK_ORDER_VT)
        p64(fresh, 0x18, fixture["unit_j"])
        p64(fresh, 0x28, target_j)
        if unit in factory_finished_units:
            C.c_uint8.from_address(fresh + 0x11).value = 1
        fixture["fresh_order"] = fresh
        return fresh

    @BIND
    def bind_object(output, raw_object):
        junction = alloc(0x40)
        i32(junction, 8, 1)
        i32(junction, 0x18, 0)
        p64(junction, 0x10, raw_object)
        output[0] = junction
        fresh_order_junctions[raw_object] = junction
        bind_calls.append((junction, raw_object))

    @STOP_INIT
    def original_stop_init(stop_order):
        stop_init_calls.append(stop_order)
        fixture = stop_init_fixtures.get(stop_order)
        if fixture is None:
            return
        # Native Stop initialization finishes the order after clearing its
        # active gunner/weapon target and retiring the reserved route.
        stop_fn = detours.get(0)
        check(stop_fn is not None, "chassis Stop hook missing during native Stop init")
        stop_fn(fixture["chassis"], 1)
        C.c_uint8.from_address(stop_order + 0x11).value = 1
        p64(fixture["gunner"], 0x88, 0)
        i32(fixture["gunner"], 0x98, 0)
        for weapon in fixture["weapons"]:
            clear_weapon_target(weapon, C.pointer(C.c_void_p(0)))
        i32(fixture["chassis"], 0xEC, 0)
        C.c_uint8.from_address(fixture["records"] + 0x2CC).value = 0

    @AI_UPDATE
    def original_ai_update(ai, dt):
        ai_update_calls.append((ai, float(dt)))
        fixture = ai_update_fixtures.get(ai)
        if fixture is None or not fixture.get("run_stop_init", True):
            return
        stop_init = detours.get(14)
        check(stop_init is not None, "Stop-order init hook missing inside AI update")
        stop_init(fixture["stop_order"])
        mutate = fixture.get("after_stop_init")
        if mutate is not None:
            mutate(fixture)

    originals = (original_stop, original_turn, original_turn, original_queue, original_order,
                 original_order_attack, original_selector, original_eligible,
                 original_requires_idle, original_helper, original_candidate,
                 original_chassis_can_move, original_path_cancel, original_submit,
                 original_stop_init, original_ai_update)

    @HOOK
    def hook(target, detour, out):
        index = len(hooks); hooks.append((target, detour)); detours[index] = (
            STOP if index == 0 else TURN if index in (1, 2) else QUEUE if index == 3 else
            ORDER if index in (4, 5) else SELECTOR if index == 6 else
            HELPER if index == 9 else GUN_QUERY if index in (7, 8) else
            CANDIDATE if index == 10 else CHASSIS_QUERY if index == 11 else
            PATH_CANCEL if index == 12 else SUBMIT if index == 13 else
            STOP_INIT if index == 14 else AI_UPDATE)(detour)
        if reject_manager_trace and index == 12:
            return -1
        if reject_completion and index == 15:
            return -1
        original_ptr = C.cast(originals[index], C.c_void_p).value
        C.cast(out, C.POINTER(C.c_void_p))[0] = original_ptr
        return 0

    @UNHOOK
    def unhook(target):
        unhooks.append(target); return 0

    callbacks.extend((log, module_base, module_size, hook, unhook, copy_reference,
                      release_ref, clear_weapon_target, release_auxiliary, native_move_factory,
                      native_attack_factory, bind_object, *originals))
    KEEP.extend(callbacks)

    class Api(C.Structure):
        _fields_ = [("abi_version", C.c_uint32), ("reserved", C.c_uint32)] + [
            (name, C.c_void_p) for name in (
                "log", "module_base", "module_size", "find_pattern", "find_pattern_at",
                "hook", "hook_exact", "hook_call", "unhook", "rtti_method", "vtable_slot",
                "config_get", "patch_bytes")]

    api = Api(5, 0, *[C.cast(fn, C.c_void_p).value if fn else None for fn in
        (log, module_base, module_size, None, None, hook, None, None, unhook, None, None, None, None)])

    class Plugin(C.Structure):
        _fields_ = [("abi_version", C.c_uint32), ("name", C.c_char_p), ("version", C.c_char_p),
                    ("init", C.c_void_p), ("stop", C.c_void_p)]

    lib = C.CDLL(str(DLL)); lib.defiance_plugin.restype = C.c_void_p
    plugin = C.cast(lib.defiance_plugin(), C.POINTER(Plugin)).contents
    check(plugin.abi_version == 5 and plugin.name == b"defiance.moving-grenades", "plugin ABI/name")
    check(plugin.version == VERSION.encode() and plugin.stop is None, "plugin version/stop export")
    init = C.CFUNCTYPE(C.c_int, C.POINTER(Api))(plugin.init)
    check(init(None) != 0 and not hooks, "null API should be refused")
    wrong = Api.from_buffer_copy(api); wrong.abi_version = 4
    check(init(C.byref(wrong)) != 0 and not hooks, "wrong ABI should be refused")
    # Changed bytes in the cancellation bounds check leave the build
    # unsupported: a warning and no hook.
    bounds = base + SITES[12] - 0x19 + 4
    saved_bounds = C.string_at(bounds, 1)
    put_code(bounds, bytes([saved_bounds[0] ^ 1]))
    check(init(C.byref(api)) == 0 and not hooks, f"changed bounds check was hooked: {hooks}")
    check(any(level == 1 and text.startswith("grenade movement fix: not a supported build (")
              for level, text in logs), f"unsupported build was not reported: {logs}")
    put_code(bounds, saved_bounds)
    logs.clear()
    init_result = init(C.byref(api))
    # A refused hook removes every hook installed before it, so the plugin
    # never runs with part of its hooks.
    refused = 12 if reject_manager_trace else 15 if reject_completion else None
    if refused is not None:
        check(init_result != 0, f"refusal of hook {refused} was accepted")
        expected = [base + SITES[i] for i in reversed(range(refused))]
        check(unhooks == expected,
              f"rollback touched an unowned site or missed an owned hook: {unhooks}")
        check(any(level == 2 and text.startswith("grenade movement fix refused: hook refused")
                  for level, text in logs), f"hook refusal was not reported: {logs}")
        K.FreeLibrary(base)
        print(f"PASS: {BUILD_NAME}: refusing hook {refused} removed the {refused} hooks installed before it")
        return
    check(init_result == 0, f"plugin init failed: {logs}")
    check([h[0] for h in hooks] == [base + r for r in SITES], "expected sixteen hooks at verified sites")
    stop_detour, turn_point, turn_direction, queue, order, order_attack_move, selector, eligible, requires_idle = [
        detours[i] for i in range(9)]
    heading_detour = detours[9]
    candidate_detour, candidate_can_move_detour = detours[10], detours[11]
    path_cancel_detour = detours[12]
    submit_detour = detours[13]
    stop_init_detour, ai_update_detour = detours[14], detours[15]
    check(C.string_at(base + PATH_CANCEL_RVA, len(PATH_CANCEL_PREFIX)) ==
          PATH_CANCEL_PREFIX, "path manager cancellation prefix differs")
    check(C.string_at(base + PATH_CANCEL_ENTRY, len(PATH_CANCEL_ENTRY_PREFIX)) ==
          PATH_CANCEL_ENTRY_PREFIX, "native path manager bounds-check entry differs")
    check(C.string_at(base + SUBMIT_RVA, len(SUBMIT_PREFIX)) == SUBMIT_PREFIX,
          "HumanAI submit hook prefix differs")
    check(C.string_at(base + RELEASE_REF, len(RELEASE_REF_PREFIX)) == RELEASE_REF_PREFIX,
          "native release-reference helper prefix differs")
    check(C.string_at(base + SUBMIT_CALLER - 10, len(SUBMIT_CALLER_PREFIX)) ==
          SUBMIT_CALLER_PREFIX, "native gunner-release submit caller differs")
    check(C.string_at(base + GUNNER_CLEAR_TARGET, len(GUNNER_CLEAR_PREFIX)) ==
          GUNNER_CLEAR_PREFIX, "native gunner target-clear helper differs")
    check(ptr(base + GUNNER_VTS[0], 0x58) == base + GUNNER_CLEAR_TARGET,
          "native gunner vtable does not identify the verified target-clear helper")
    path_cancel_entry = C.CFUNCTYPE(C.c_uint64, C.c_void_p, C.c_int32)(base + PATH_CANCEL_ENTRY)

    def invoke_submit_from_native_caller(ai, incoming, delay=0.0):
        """Enter the verified native call instruction so the detour sees its exact caller RVA."""
        call_site = base + SUBMIT_CALLER
        return_site = call_site + 2
        setup_site, epilogue = base + native_rva(0x2D8B6C), base + native_rva(0x2D9145)
        saved_setup, saved_return = C.string_at(setup_site, 15), C.string_at(return_site, 5)
        detour_address = C.cast(submit_detour, C.c_void_p).value
        rel_to_call = call_site - (setup_site + 15)
        setup_patch = b"\x48\xbf" + detour_address.to_bytes(8, "little") + b"\xe9" + struct.pack("<i", rel_to_call)
        rel_to_epilogue = epilogue - (return_site + 5)
        return_patch = b"\xe9" + struct.pack("<i", rel_to_epilogue)
        put_code(setup_site, setup_patch)
        put_code(return_site, return_patch)
        check(K.FlushInstructionCache(K.GetCurrentProcess(), C.c_void_p(setup_site), len(setup_patch))
              and K.FlushInstructionCache(K.GetCurrentProcess(), C.c_void_p(return_site), len(return_patch)),
              "cannot flush temporary HumanAI submit caller")
        try:
            native_submit = C.CFUNCTYPE(None, C.c_void_p, C.c_void_p, C.c_float)(base + native_rva(0x2D8B50))
            native_submit(ai, incoming, delay)
        finally:
            put_code(setup_site, saved_setup)
            put_code(return_site, saved_return)
            K.FlushInstructionCache(K.GetCurrentProcess(), C.c_void_p(setup_site), len(saved_setup))
            K.FlushInstructionCache(K.GetCurrentProcess(), C.c_void_p(return_site), len(saved_return))

    def prepare_submit_fixture(fixture, attack_move=False):
        ai = fixture["human_ai"]
        ai_vtable = alloc(0x100)
        p64(ai, 0, ai_vtable); p64(ai_vtable, 0x60, base + SUBMIT_RVA)
        C.c_uint8.from_address(ai + 0x18).value = 1
        C.c_uint8.from_address(ai + 0x130).value = 0
        p64(ai, 0x1E0, 0)
        gunner = fixture["gunner"]
        i32(gunner, 0x98, 0x1234)
        weapon_array = alloc(16)
        second_weapon, second_vtable = alloc(0x200), alloc(0x200)
        p64(fixture["gun_vt"], 0x30, C.cast(clear_weapon_target, C.c_void_p).value)
        p64(second_weapon, 0, second_vtable)
        p64(second_vtable, 0x30, C.cast(clear_weapon_target, C.c_void_p).value)
        p64(weapon_array, 0, fixture["gun"]); p64(weapon_array, 8, second_weapon)
        p64(gunner, 0x38, weapon_array); p64(gunner, 0x40, weapon_array + 16)
        old_order = fixture["old_move"]
        p64(old_order, 0, base + (ATTACK_ORDER_VT if attack_move else MOVE_ORDER_VT))
        p64(old_order, 0x18, fixture["unit_j"])
        repository, repo_getter = alloc(0x100), alloc(0x40)
        put_code(repo_getter, b"\x48\xb8" + repository.to_bytes(8, "little") + b"\xc3")
        p64(fixture["unit_vt"], 0x80, repo_getter)
        destination_object, destination_j = alloc(0x80), alloc(0x40)
        p64(destination_j, 8, 1); p64(destination_j, 0x10, destination_object)
        i32(destination_j, 0x18, 1)
        p64(old_order, 0x48, destination_j)
        i32(old_order, 0x50, 0)
        i32(old_order, 0x14, 0x5A31)
        f32(old_order, 0x54, 12.25); f32(old_order, 0x58, -3.5)
        C.c_uint8.from_address(old_order + 0x5C).value = 1
        i32(old_order, 0x60, 7)
        C.c_uint8.from_address(old_order + 0x68).value = 0x42
        attack_target_object, attack_target_j = alloc(0x80), alloc(0x40)
        p64(attack_target_j, 8, 1); p64(attack_target_j, 0x10, attack_target_object)
        i32(attack_target_j, 0x18, 1)
        if attack_move:
            p64(old_order, 0x28, attack_target_j)
        stop_order, stop_junction = alloc(0x80), alloc(0x40)
        stop_owner_junction = alloc(0x40)
        i32(stop_junction, 8, 1); i32(stop_junction, 0x18, 1)
        p64(stop_owner_junction, 0x10, fixture["unit"])
        p64(stop_order, 0, base + native_rva(0x70E5D8))
        p64(stop_order, 0x18, stop_owner_junction)
        p64(stop_junction, 0x10, stop_order)
        incoming = alloc(8); p64(incoming, 0, stop_junction)
        result = dict(fixture)
        result.update({"ai": ai, "incoming": incoming, "stop_order": stop_order,
                       "stop_junction": stop_junction, "old_order": old_order,
                       "repository": repository, "destination_object": destination_object,
                       "destination_j": destination_j, "attack_target_j": attack_target_j,
                       "attack_target_object": attack_target_object,
                       "weapons": (fixture["gun"], second_weapon), "weapon_array": weapon_array})
        movement_fixtures_by_unit[fixture["unit"]] = result
        stop_init_fixtures[stop_order] = result
        ai_update_fixtures[ai] = result
        return result

    def invoke_native_path_cancel(manager, nav):
        """Run the real native bounds checks, then redirect its checked body to the hook."""
        site = base + PATH_CANCEL_RVA
        original = C.string_at(site, len(PATH_CANCEL_PREFIX))
        detour_address = C.cast(path_cancel_detour, C.c_void_p).value
        # Test host hooks are recorded rather than installed, so patch only this
        # disposable mapped clone to model the loader's body detour.
        jump = b"\xff\x25\x00\x00\x00\x00" + detour_address.to_bytes(8, "little") + b"\x90\x90"
        put_code(site, jump)
        try:
            return path_cancel_entry(manager, nav)
        finally:
            put_code(site, original)
    try:
        import capstone
    except ImportError:
        capstone = None
    if capstone is not None:
        decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
        decoder.detail = True
        cancel_body_insns = list(decoder.disasm(PATH_CANCEL_PREFIX, base + PATH_CANCEL_RVA))
        check(sum(insn.size for insn in cancel_body_insns) == len(PATH_CANCEL_PREFIX)
              and [insn.mnemonic for insn in cancel_body_insns] == ["movsxd", "xor", "imul", "add"],
              f"path manager hooked body no longer decodes to the expected four-instruction sequence: {cancel_body_insns}")
        check(all(not insn.group(capstone.CS_GRP_JUMP) and
                  not any(operand.type == capstone.x86.X86_OP_MEM and
                          operand.mem.base == capstone.x86.X86_REG_RIP
                          for operand in insn.operands) for insn in cancel_body_insns),
              "path manager hooked body contains a branch or RIP-relative memory operand")

    check(C.string_at(base + CANDIDATE_CONTINUATION, len(CANDIDATE_CONTINUATION_PREFIX)) ==
          CANDIDATE_CONTINUATION_PREFIX, "grenade candidate continuation differs")

    # The new helper hook dispatches by its immediate native return address.
    # Execute the real call instruction at each verified site and patch only
    # that return address in the throwaway mapped image to capture its result.
    def movabs(opcode, value):
        return opcode + struct.pack("<Q", value)

    def make_heading_caller(call_rva, chassis, incoming, dt):
        return_rva = call_rva + 5
        result = alloc(0x100)
        capture = alloc(0x100)
        cleanup = alloc(0x40)
        caller = alloc(0x100)
        # Store volatile GPRs and XMM0..5 from the native helper return.
        code = bytearray()
        for opcode, offset in ((b"\x48\x89\x87", 0), (b"\x48\x89\x8f", 8),
                               (b"\x48\x89\x97", 16), (b"\x4c\x89\x87", 24),
                               (b"\x4c\x89\x8f", 32), (b"\x4c\x89\x97", 40),
                               (b"\x4c\x89\x9f", 48)):
            code += opcode + struct.pack("<I", offset)
        for i in range(6):
            code += b"\xf3\x0f\x7f" + bytes((0x87 + i * 8,)) + struct.pack("<I", 0x40 + 16 * i)
        code += b"\xff\x25\x00\x00\x00\x00" + cleanup.to_bytes(8, "little")
        put_code(capture, bytes(code))
        put_code(cleanup, b"\x48\x8b\x7c\x24\x20\x48\x83\xc4\x28\xc3")
        # Caller frame supplies Win64 shadow space and keeps RDI stable for
        # capture. Push the exact stock return RVA and jump directly to the
        # detour: the helper reads this same caller address from [rsp].
        body = bytearray(b"\x48\x83\xec\x28\x48\x89\x7c\x24\x20")
        body += movabs(b"\x48\xbf", result)
        body += movabs(b"\x48\xb9", chassis)
        body += movabs(b"\x48\xba", incoming)
        body += b"\xb8" + struct.pack("<f", dt) + b"\x66\x0f\x6e\xd0"
        body += movabs(b"\x48\xb8", base + return_rva) + b"\x50"
        body += movabs(b"\x48\xb8", C.cast(heading_detour, C.c_void_p).value) + b"\xff\xe0"
        put_code(caller, bytes(body))
        saved = C.string_at(base + return_rva, 14)
        put_code(base + return_rva,
                 b"\xff\x25\x00\x00\x00\x00" + capture.to_bytes(8, "little"))
        check(K.FlushInstructionCache(K.GetCurrentProcess(), C.c_void_p(base + return_rva), 14),
              "cannot flush temporary native return thunk")
        check(K.FlushInstructionCache(K.GetCurrentProcess(), C.c_void_p(caller), len(body)),
              "cannot flush native caller stub")
        return C.CFUNCTYPE(None)(caller), result, (base + return_rva, saved)

    def invoke_heading(fixture, call_rva, point, dt=0.016):
        direction = point if isinstance(point, C.Array) else (C.c_float * 2)(*point)
        caller, result, restore = make_heading_caller(
            call_rva, fixture["chassis"], C.addressof(direction), dt)
        try:
            before = len(helper_calls)
            caller()
            check(len(helper_calls) == before + 1,
                  "shared native heading helper was not forwarded exactly once")
            actual = helper_calls[-1]
            check(actual[0] == fixture["chassis"] and abs(actual[2] - dt) < 1e-6,
                  f"native helper arguments changed: {actual}")
            check(C.c_uint64.from_address(result).value == helper_result,
                  "native helper return bits were not preserved")
            return actual, result
        finally:
            put_code(*restore)

    def invoke_candidate_readiness(chassis, return_rva=CANDIDATE_CONTINUATION):
        """Call the chassis vfunc detour with a synthetic native return site."""
        result, capture, cleanup, caller = alloc(8), alloc(0x40), alloc(0x40), alloc(0x80)
        capture_code = (b"\x48\xa3" + result.to_bytes(8, "little") +
                        b"\xff\x25\x00\x00\x00\x00" + cleanup.to_bytes(8, "little"))
        put_code(capture, capture_code)
        put_code(cleanup, b"\x48\x83\xc4\x28\xc3")
        body = (b"\x48\x83\xec\x28\x48\xb9" + chassis.to_bytes(8, "little") +
                b"\x48\xb8" + (base + return_rva).to_bytes(8, "little") + b"\x50" +
                b"\x48\xb8" + C.cast(candidate_can_move_detour, C.c_void_p).value.to_bytes(8, "little") +
                b"\xff\xe0")
        put_code(caller, body)
        saved = C.string_at(base + return_rva, 14)
        put_code(base + return_rva,
                 b"\xff\x25\x00\x00\x00\x00" + capture.to_bytes(8, "little"))
        check(K.FlushInstructionCache(K.GetCurrentProcess(), C.c_void_p(base + return_rva), 14),
              "cannot flush temporary candidate return thunk")
        check(K.FlushInstructionCache(K.GetCurrentProcess(), C.c_void_p(caller), len(body)),
              "cannot flush candidate caller stub")
        try:
            before = len(candidate_ready_calls)
            C.CFUNCTYPE(None)(caller)()
            check(len(candidate_ready_calls) == before + 1,
                  "native chassis readiness was not forwarded exactly once")
            return C.c_uint64.from_address(result).value
        finally:
            put_code(base + return_rva, saved)

    def invoke_candidate(fixture, index=1, native_result=0,
                         target_arg=0x12345678, flags=0x90ABCDEF):
        gunner = fixture["gunner"]
        candidate_fixtures[gunner] = fixture
        candidate_results[(gunner, index)] = native_result
        before = len(candidate_calls)
        expected = (gunner, index, target_arg, flags)
        result = candidate_detour(gunner, index, target_arg, flags)
        check(len(candidate_calls) > before and candidate_calls[before] == expected and
              sum(call == expected for call in candidate_calls[before:]) == 1,
              "native grenade-candidate callback arguments/count changed")
        return result

    # Replace the native resume target only after plugin validation/init. The
    # real copy-reference routine remains mapped and executes in the test image.
    ResumeShim = C.CFUNCTYPE(C.c_uint8, C.c_void_p, C.POINTER(C.c_void_p), C.c_float,
                             C.POINTER(C.c_float))
    resumes, resume_results = [], {}
    @ResumeShim
    def resume_shim(chassis, out_target, speed, direction):
        target_ref = out_target[0]
        resumes.append((chassis, target_ref, speed,
                        None if not direction else (direction[0], direction[1])))
        if target_ref:
            C.c_int32.from_address(target_ref + 8).value -= 1
            C.c_int32.from_address(target_ref + 0x18).value -= 1
        result = resume_results.get(chassis, 1)
        if result:
            i32(chassis, 0xEC, 1)
            manager = ptr(chassis, 0x30); header = ptr(manager, 0x288)
            nav = C.c_int32.from_address(chassis + 0xC4).value
            count = i32_read(header, 4) if header else 0
            records = ptr(header, 8) if header else 0
            if records and 0 <= nav < count:
                C.c_uint8.from_address(records + nav * 0x2F0 + 0x2CC).value = 2
        return result
    KEEP.append(resume_shim)
    resume_addr = base + RESUME
    resume_code = b"\x48\x83\xec\x28\x48\xb8" + C.cast(resume_shim, C.c_void_p).value.to_bytes(8, "little") + b"\xff\xd0\x48\x83\xc4\x28\xc3"
    saved_resume = C.string_at(resume_addr, len(resume_code))
    put_code(resume_addr, resume_code)
    saved_copyref = C.string_at(base + COPYREF, 16)
    saved_release_ref = C.string_at(base + RELEASE_REF, 16)
    saved_aux_release = ptr(base, AUX_RELEASE_IAT)
    def callback_jump(callback):
        return (b"\xff\x25\x00\x00\x00\x00" +
                C.cast(callback, C.c_void_p).value.to_bytes(8, "little") + b"\x90\x90")
    native_factory_patches = {}
    for rva, callback in ((MOVE_ORDER_FACTORY, native_move_factory),
                          (ATTACK_ORDER_FACTORY, native_attack_factory),
                          (BIND_OBJECT, bind_object)):
        address = base + rva
        native_factory_patches[address] = C.string_at(address, 14)
        put_code(address, callback_jump(callback))
    put_code(base + COPYREF, callback_jump(copy_reference))
    put_code(base + RELEASE_REF, callback_jump(release_ref))
    put_code(base + AUX_RELEASE_IAT, C.cast(release_auxiliary, C.c_void_p).value.to_bytes(8, "little"))
    check(K.FlushInstructionCache(K.GetCurrentProcess(), C.c_void_p(base + COPYREF), 16)
          and K.FlushInstructionCache(K.GetCurrentProcess(), C.c_void_p(base + RELEASE_REF), 16)
          and all(K.FlushInstructionCache(K.GetCurrentProcess(), C.c_void_p(address), 14)
                  for address in native_factory_patches),
          "cannot flush native junction shims")
    aim_restore = None

    def order_route(f, order_fn=None, attack_move=False):
        order_fn = order_fn or order
        state_chassis[f["state"]] = f["chassis"]
        arg = C.cast(f["incoming"], C.POINTER(C.c_void_p))
        prior_stops = len([c for c in calls if c[0] == "stop"])
        order_name = "order_attack_move" if attack_move else "order"
        prior_orders = len([c for c in calls if c[0] == order_name])
        order_fn(f["state"], arg)
        check(len([c for c in calls if c[0] == order_name]) == prior_orders + 1,
              "order original not forwarded exactly once")
        check(len([c for c in calls if c[0] == "stop"]) == prior_stops + 1,
              "initial replacement stop not forwarded inside original order")
        check(i32_read(f["chassis"], 0xEC) == 0, "initial stop shim did not enter mode 0")
        check(u8(f["records"], 0x2CC) == 0, "initial stop shim did not clear reservation state")

    def queued(f, action):
        prior = len([c for c in calls if c[0] == "queue"])
        queue(f["animation"], action)
        check(len([c for c in calls if c[0] == "queue"]) == prior + 1,
              f"queue original not forwarded once for {action:#x}")
        check(i32_read(f["animation"], 0x68) == action, "queue original action update missing")

    def selector_call(f, native_result=0, eligible_result=1, requires_result=1, **config):
        selector_fixtures[f["gunner"]] = f
        selector_outputs[f["gunner"]] = native_result
        selector_configs[f["gunner"]] = config
        eligible_outputs[f["gun"]] = eligible_result
        requires_idle_outputs[f["gun"]] = requires_result
        before = len(calls)
        result = selector(f["gunner"])
        new_calls = calls[before:]
        check([c for c in new_calls if c[0] == "selector"] == [("selector", f["gunner"])],
              "native selector was not forwarded exactly once")
        expected_eligible = config.get("call_eligible", True)
        expected_idle = config.get("call_requires_idle", True)
        check(len([c for c in new_calls if c[0] == "eligible"]) == int(expected_eligible)
              and len([c for c in new_calls if c[0] == "requires_idle"]) == int(expected_idle),
              f"selector did not call expected readiness originals: {new_calls}")
        return result, new_calls

    def readiness_fixture():
        f = route_fixture(base)
        i32(f["gunner"], 0x90, 2)
        i32(f["gunner"], 0xB8, 2)
        i32(f["chassis"], 0xEC, 1)
        C.c_uint8.from_address(f["records"] + 0x2CC).value = 2
        i32(f["animation"], 0x6C, 3)
        return f

    def candidate_fixture():
        f = route_fixture(base)
        # The rifle remains selected at slot 0. Two unselected hand grenades
        # at slots 1 and 2 exercise the native candidate scan independently.
        C.c_uint8.from_address(f["descriptor"] + 0xA9).value = 0
        i32(f["descriptor"], 0xC4, 1)
        hand_guns = []
        candidate_array = alloc(24)
        p64(candidate_array, 0, f["gun"])
        for index in (1, 2):
            hand_gun, hand_vt, hand_descriptor = alloc(0x200), alloc(0x200), alloc(0x200)
            p64(hand_gun, 0, hand_vt)
            p64(hand_vt, 0x160, C.cast(f["get_descriptor"], C.c_void_p).value)
            p64(hand_gun, 0x1A0, hand_descriptor)
            C.c_uint8.from_address(hand_descriptor + 0xA9).value = 1
            i32(hand_descriptor, 0xC4, 3)
            p64(candidate_array, index * 8, hand_gun)
            hand_guns.append((hand_gun, hand_vt, hand_descriptor))
        p64(f["gunner"], 0x38, candidate_array)
        p64(f["gunner"], 0x40, candidate_array + 24)
        i32(f["gunner"], 0x94, 0)  # The grenade candidate remains unselected.
        i32(f["gunner"], 0x90, 2)
        i32(f["gunner"], 0xB8, 1)
        i32(f["chassis"], 0xEC, 1)
        C.c_uint8.from_address(f["records"] + 0x2CC).value = 2
        i32(f["animation"], 0x68, 3)
        i32(f["animation"], 0x6C, 3)
        i32(f["animation"], 0x74, 1)
        f["candidate_guns"] = hand_guns
        f["gun_array"] = candidate_array
        return f

    def attach_flare_target(fixture, kind=0x100, point=(3.0, 4.0, 7.0), vtable=None):
        target = alloc(0x40)
        p64(target, 0, base + FLARE_TARGET_VT if vtable is None else vtable)
        for offset, value in zip((0x10, 0x14, 0x18), point):
            f32(target, offset, value)
        i32(target, 0x28, kind)
        i32(fixture["target_j"], 8, 3)
        i32(fixture["target_j"], 0x18, 3)
        p64(fixture["target_j"], 0x10, target)
        return target

    def resumed_flare_fixture():
        fixture = route_fixture(base)
        attach_flare_target(fixture)
        order_route(fixture)
        before = len(resumes)
        queued(fixture, 0x17)
        check(len(resumes) == before + 1 and i32_read(fixture["chassis"], 0xEC) == 1,
              "FlareTarget grenade throw did not successfully resume the native route")
        return fixture

    def captured_flare_fixture(branch=2):
        fixture = route_fixture(base)
        submit_fixture = prepare_submit_fixture(fixture)
        ai_vtable = alloc(0x100)
        p64(fixture["human_ai"], 0, ai_vtable)
        p64(ai_vtable, 0x60, base + SUBMIT_RVA)
        C.c_uint8.from_address(fixture["human_ai"] + 0x18).value = 1
        C.c_uint8.from_address(fixture["human_ai"] + 0x130).value = 0
        p64(fixture["human_ai"], 0x1E0, 0)
        attach_flare_target(fixture)
        order_route(fixture)
        i32(fixture["gunner"], 0x90, 2)
        i32(fixture["gunner"], 0xB8, branch)
        i32(fixture["animation"], 0x68, 1)
        i32(fixture["animation"], 0x6C, 1)
        i32(fixture["animation"], 0x74, 1)
        fixture["submit_fixture"] = submit_fixture
        return fixture

    def readiness_resumed_flare_fixture():
        fixture = captured_flare_fixture()
        before = len(resumes)
        selector_call(fixture, native_result=1, eligible_result=1)
        check(len(resumes) == before and i32_read(fixture["chassis"], 0xEC) == 0
              and u8(fixture["records"], 0x2CC) == 0
              and selector_snapshots[-1][1:4] == (0, 0, 2),
              f"native selector did not run before queue resume while stopped: {selector_snapshots[-1]}")
        queued(fixture, 0x17)
        check(len(resumes) == before + 1 and i32_read(fixture["chassis"], 0xEC) == 1,
              "confirmed grenade queue did not resume the retained route")
        return fixture

    def expect_selector_guard(label, f, native=0, eligible_result=1, idle_result=1,
                              expected_eligible=True, expected_idle=True, **config):
        result, trace = selector_call(
            f, native, eligible_result, idle_result,
            call_eligible=expected_eligible, call_requires_idle=expected_idle, **config)
        check(result == native, f"{label}: readiness guard improperly changed native result")
        check(any(c[0] == "selector_eligible_result" for c in trace) == expected_eligible
              and any(c[0] == "selector_requires_idle_result" for c in trace) == expected_idle,
              f"{label}: readiness results captured incorrectly: {trace}")
        return trace

    try:
        # The new candidate hook scopes a narrow chassis-readiness override
        # around the native unselected grenade candidate test. The original
        # candidate still owns all later range and suitability decisions.
        moving_candidate = candidate_fixture()
        g = moving_candidate["gunner"]
        check(i32_read(g, 0x94) == 0 and
              C.c_uint8.from_address(moving_candidate["candidate_guns"][0][2] + 0xA9).value == 1,
              "candidate fixture did not leave its hand grenade unselected at index 1")
        candidate_fixtures[g] = moving_candidate
        native_ready_false = 0xAABBCCDDEEFF1200
        candidate_ready_results[moving_candidate["chassis"]] = native_ready_false
        candidate_accept = 0x1122334400000001
        before_candidates, before_readiness = len(candidate_calls), len(candidate_ready_calls)
        result = invoke_candidate(moving_candidate, 1, candidate_accept)
        check(result == candidate_accept and len(candidate_calls) == before_candidates + 1,
              "native grenade candidate result/full u64 bits changed")
        check(len(candidate_ready_calls) == before_readiness + 1 and
              candidate_readiness[(g, 1)] == (native_ready_false | 1),
              "owned moving route did not narrowly pass the candidate readiness gate")

        # The hook preserves true low-byte results and all upper return bits.
        native_ready_true = 0xABCDEF1200000001
        candidate_ready_results[moving_candidate["chassis"]] = native_ready_true
        native_candidate_true = 0x9988776600000001
        result = invoke_candidate(moving_candidate, 1, native_candidate_true)
        check(result == native_candidate_true and
              candidate_readiness[(g, 1)] == native_ready_true,
              "native-true candidate/readiness result was rewritten")

        # A later native range/suitability failure stays false even after the
        # earlier moving-readiness predicate was narrowly promoted.
        candidate_ready_results[moving_candidate["chassis"]] = native_ready_false
        result = invoke_candidate(moving_candidate, 1, 0)
        check(result == 0 and candidate_readiness[(g, 1)] == (native_ready_false | 1),
              "candidate hook replaced the native range/suitability result")

        def rejected_candidate_gate(label, mutate=None, return_rva=CANDIDATE_CONTINUATION,
                                    ready_chassis=None):
            fixture = candidate_fixture()
            if mutate:
                mutate(fixture)
            candidate_fixtures[fixture["gunner"]] = None
            native = 0x5566778800000000
            candidate_ready_results[fixture["chassis"]] = native
            candidate_ready_results[ready_chassis or fixture["chassis"]] = native
            observed = []
            candidate_nested[(fixture["gunner"], 1)] = lambda: observed.append(
                invoke_candidate_readiness(ready_chassis or fixture["chassis"], return_rva))
            result = invoke_candidate(fixture, 1, 0)
            candidate_nested.pop((fixture["gunner"], 1), None)
            check(result == 0 and observed == [native],
                  f"{label}: candidate readiness gate changed an ineligible native result: {observed}")
            return fixture

        rejected_candidate_gate("unrelated readiness caller", return_rva=native_rva(0x2D9750))
        rejected_candidate_gate("unowned chassis", ready_chassis=route_fixture(base)["chassis"])
        rejected_candidate_gate("inactive route", lambda f: (
            i32(f["chassis"], 0xEC, 0), C.c_uint8.from_address(f["records"] + 0x2CC).__setattr__("value", 0)))
        rejected_candidate_gate("non-standing posture", lambda f: i32(f["animation"], 0x74, 0))
        rejected_candidate_gate("wrong native gunner state", lambda f: i32(f["gunner"], 0x90, 3))
        rejected_candidate_gate("wrong candidate branch", lambda f: i32(f["gunner"], 0xB8, 2))
        rejected_candidate_gate("active throw action with grenade pending", lambda f: (
            i32(f["animation"], 0x68, 0x17), i32(f["animation"], 0x6C, 3)))
        rejected_candidate_gate("wrong pending action", lambda f: i32(f["animation"], 0x6C, 0x17))
        rejected_candidate_gate("unrelated current action", lambda f: i32(f["animation"], 0x68, 0))
        rejected_candidate_gate("non-grenade candidate", lambda f: (
            C.c_uint8.from_address(f["candidate_guns"][0][2] + 0xA9).__setattr__("value", 0)))
        rejected_candidate_gate("wrong target getter", lambda f: p64(f["attack_target_vt"], 0x28, base + native_rva(0x46A741)))
        rejected_candidate_gate("animation owned by another unit", lambda f: p64(f["animation_j"], 0x10, f["other_unit"]))

        # Nested candidates share a gunner/unit but refer to different grenade
        # slots. Invalidate index 2 while it is nested, then restore it and
        # verify the outer index-1 TLS context still independently revalidates.
        nested = candidate_fixture()
        nested_gunner = nested["gunner"]
        candidate_fixtures[nested_gunner] = nested
        candidate_ready_results[nested["chassis"]] = native_ready_false
        second_descriptor = nested["candidate_guns"][1][2]
        def nested_other_candidate():
            old_flag = u8(second_descriptor, 0xA9)
            old_mode = i32_read(second_descriptor, 0xC4)
            C.c_uint8.from_address(second_descriptor + 0xA9).value = 0
            i32(second_descriptor, 0xC4, 1)
            result2 = invoke_candidate(nested, 2, 0x1234000000000001)
            C.c_uint8.from_address(second_descriptor + 0xA9).value = old_flag
            i32(second_descriptor, 0xC4, old_mode)
            check(result2 == 0x1234000000000001,
                  "nested second-candidate native result/full bits changed")
        candidate_nested[(nested_gunner, 1)] = nested_other_candidate
        outer_result = invoke_candidate(nested, 1, 0x5678000000000001)
        candidate_nested.pop((nested_gunner, 1), None)
        check(outer_result == 0x5678000000000001 and
              candidate_readiness[(nested_gunner, 2)] == native_ready_false and
              candidate_readiness[(nested_gunner, 1)] == (native_ready_false | 1),
              "nested same-unit/different-candidate TLS was not restored after callback")

        # Replacing the gunner target after candidate TLS capture must fail
        # closed even when the replacement is another valid AttackTarget.
        target_race = candidate_fixture()
        target_gunner = target_race["gunner"]
        candidate_fixtures[target_gunner] = target_race
        candidate_ready_results[target_race["chassis"]] = native_ready_false
        p64(target_race["other_target"], 0, target_race["attack_target_vt"])
        target_observed = []
        def swap_target_during_candidate():
            original_target_ref = ptr(target_gunner, 0x88)
            p64(target_gunner, 0x88, target_race["other_target_j"])
            target_observed.append(invoke_candidate_readiness(target_race["chassis"]))
            p64(target_gunner, 0x88, original_target_ref)
        candidate_nested[(target_gunner, 1)] = swap_target_during_candidate
        target_tail = invoke_candidate(target_race, 1, 0)
        candidate_nested.pop((target_gunner, 1), None)
        check(target_tail == 0 and target_observed == [native_ready_false] and
              candidate_readiness[(target_gunner, 1)] == (native_ready_false | 1),
              "target change after TLS capture escaped revalidation or altered native tail")

        # The shared helper is called from two stock chassis sites. At the
        # first site, derive a fresh target bearing and verify it flips after
        # the actor passes the target. The route-facing second call remains
        # neutral after the first turn, preventing a second native turn step.
        facing = route_fixture(base)
        i32(facing["animation"], 0x6C, 0x17)
        i32(facing["animation"], 0x74, 1)
        f32(facing["target"], 0x14, 10.0); f32(facing["target"], 0x18, 0.0)
        native, _ = invoke_heading(facing, native_rva(0x2C92B7), (0.0, 1.0))
        check(native[3] is not None and abs(native[3][0] - 1.0) < 1e-5
              and abs(native[3][1]) < 1e-5,
              f"initial native turn did not receive fresh target bearing: {native}")
        f32(facing["position"], 0, 20.0)
        native, _ = invoke_heading(facing, native_rva(0x2C92B7), (0.0, 1.0))
        check(native[3] is not None and abs(native[3][0] + 1.0) < 1e-5
              and abs(native[3][1]) < 1e-5,
              f"bearing did not flip after passing target: {native}")
        f32(facing["chassis"], 0x84, 0.0); f32(facing["chassis"], 0x88, 1.0)
        C.c_uint8.from_address(facing["chassis"] + 0x108).value = 1
        C.c_uint8.from_address(facing["chassis"] + 0x10B).value = 1
        native, _ = invoke_heading(facing, native_rva(0x2CA0F7), (1.0, 0.0))
        check(native[3] is not None and abs(native[3][0]) < 1e-5
              and abs(native[3][1] - 1.0) < 1e-5,
              f"later route turn was not neutralized at the current heading: {native}")
        C.c_uint8.from_address(facing["chassis"] + 0x10B).value = 0
        native, _ = invoke_heading(facing, native_rva(0x2CA0F7), (0.0, 1.0))
        check(native[3] is not None and abs(native[3][0] + 1.0) < 1e-5
              and abs(native[3][1]) < 1e-5,
              f"route caller without completed aim did not use fresh target bearing: {native}")

        # Dynamic AttackTarget objects refresh through their entity's +0x70
        # position getter on each helper call; no cached target coordinates.
        dynamic = route_fixture(base)
        i32(dynamic["animation"], 0x6C, 0x2B); i32(dynamic["animation"], 0x74, 1)
        C.c_uint8.from_address(dynamic["target"] + 0x10).value = 0
        entity, entity_vt, entity_j, entity_pos = alloc(0x100), alloc(0x100), alloc(0x40), alloc(0x20)
        @C.CFUNCTYPE(C.c_void_p, C.c_void_p)
        def get_entity_position(_entity):
            return entity_pos
        KEEP.append(get_entity_position)
        p64(entity, 0, entity_vt); p64(entity_vt, 0x70, C.cast(get_entity_position, C.c_void_p).value)
        p64(entity_j, 0x10, entity); p64(dynamic["target"], 0x28, entity_j)
        f32(entity_pos, 0, 10.0); f32(entity_pos, 4, 0.0)
        native, _ = invoke_heading(dynamic, native_rva(0x2C92B7), (0.0, 1.0))
        check(native[3] is not None and abs(native[3][0] - 1.0) < 1e-5,
              f"dynamic target's first live bearing was not read: {native}")
        f32(entity_pos, 0, -10.0)
        native, _ = invoke_heading(dynamic, native_rva(0x2C92B7), (0.0, 1.0))
        check(native[3] is not None and abs(native[3][0] + 1.0) < 1e-5,
              f"dynamic target movement was not refreshed between calls: {native}")

        # FlareTarget uses its native mapped point getter directly. Its point
        # is embedded at +0x10, rather than reached through an entity getter.
        flare = route_fixture(base)
        i32(flare["animation"], 0x6C, 0x17); i32(flare["animation"], 0x74, 1)
        flare_target = alloc(0x40)
        p64(flare_target, 0, base + FLARE_TARGET_VT)
        # The object uses the real mapped vtable and embeds x/y/z at +0x10.
        f32(flare_target, 0x10, 3.0); f32(flare_target, 0x14, 4.0); f32(flare_target, 0x18, 7.0)
        p64(flare["target_j"], 0x10, flare_target)
        native_flare_getter = C.CFUNCTYPE(C.c_void_p, C.c_void_p)(base + FLARE_POINT_GETTER)
        check(native_flare_getter(flare_target) == flare_target + 0x10,
              "mapped native FlareTarget getter did not return its embedded point")
        native, _ = invoke_heading(flare, native_rva(0x2C92B7), (0.0, 1.0))
        check(native[3] is not None and abs(native[3][0] - 0.6) < 1e-5
              and abs(native[3][1] - 0.8) < 1e-5,
              f"FlareTarget embedded point did not redirect heading: {native}")

        def trace_lines(fixture, event=None):
            chassis_tag = f"chassis={fixture['chassis']:#x}"
            return [text for _, text in logs
                    if text.startswith("moving flare route trace:") and chassis_tag in text
                    and (event is None or f"event={event} " in text)]

        # A successful native resume saves a bounded scalar context. Clearing
        # the live gunner target afterwards still lets stop/turn diagnostics
        # report the FlareTarget kind without dereferencing the old target.
        retained_stop = resumed_flare_fixture()
        p64(retained_stop["gunner"], 0x88, 0)
        prior_stops = len([call for call in calls if call[0] == "stop"])
        stop_detour(retained_stop["chassis"], 7)
        check(len([call for call in calls if call[0] == "stop"]) == prior_stops + 1
              and calls[-1] == ("stop", retained_stop["chassis"], 7)
              and i32_read(retained_stop["chassis"], 0xEC) == 0,
              "retained FlareTarget stop changed or skipped the native stop")
        stop_trace = trace_lines(retained_stop, "stop-before")
        check(any("source=retained" in text and "kind=0x100" in text and "target=0x0" in text
                  for text in stop_trace),
              f"stop trace did not use retained FlareTarget kind after live target clear: {stop_trace}")

        retained_turn = resumed_flare_fixture()
        p64(retained_turn["gunner"], 0x88, 0)
        direction = (C.c_float * 2)(0.25, -0.5)
        prior_turns = len([call for call in calls if call[0] == "turn"])
        turn_direction(retained_turn["chassis"], direction)
        check(len([call for call in calls if call[0] == "turn"]) == prior_turns + 1
              and calls[-1] == ("turn", retained_turn["chassis"], C.addressof(direction)),
              "retained FlareTarget direction trace changed or skipped the native turn")
        turn_trace = trace_lines(retained_turn, "turn-direction-before")
        check(any("source=retained" in text and "kind=0x100" in text and "target=0x0" in text
                  for text in turn_trace),
              f"turn-direction trace did not use retained FlareTarget context: {turn_trace}")

        retained_idle = resumed_flare_fixture()
        p64(retained_idle["gunner"], 0x88, 0)
        before_resumes = len(resumes)
        queued(retained_idle, 3)
        check(len(resumes) == before_resumes,
              "idle movement queue incorrectly ran a grenade route resume")
        idle_trace = trace_lines(retained_idle, "idle-queue-before")
        check(any("source=retained" in text and "kind=0x100" in text and "target=0x0" in text
                  for text in idle_trace),
              f"idle movement queue missed the retained FlareTarget context: {idle_trace}")

        # Copied context survives forget for diagnostics only. Stale route
        # identity is reported explicitly, while changed ownership drops it.
        stale_contexts = (
            ("navigation", lambda f: i32(f["chassis"], 0xC4, 1)),
            ("move target", lambda f: p64(f["chassis"], 0xD8, f["other_move_j"])),
            ("manager", lambda f: p64(f["chassis"], 0x30, f["other_manager"])),
            ("owner", lambda f: p64(f["gunner"], 0x20, f["other_unit"])),
        )
        for label, mutate in stale_contexts:
            stale = resumed_flare_fixture()
            p64(stale["gunner"], 0x88, 0)
            mutate(stale)
            before = len(logs)
            stop_detour(stale["chassis"], 7)
            new_lines = [text for _, text in logs[before:]
                         if text.startswith("moving flare route trace:")
                         and f"chassis={stale['chassis']:#x}" in text]
            stop_lines = [text for text in new_lines if "event=stop-before " in text]
            if label == "owner":
                check(not any("source=retained" in text for text in stop_lines),
                      f"changed owner incorrectly reused its retained context: {new_lines}")
            else:
                check(any("source=retained" in text and "identity_match=false" in text
                          and "context_forgotten=false" in text for text in stop_lines),
                      f"{label}-stale route identity was not marked mismatched: {new_lines}")

        forgotten = resumed_flare_fixture()
        p64(forgotten["gunner"], 0x88, 0)
        stop_detour(forgotten["chassis"], 7)
        forget_lines = trace_lines(forgotten, "cache-forget-before")
        check(any("source=retained" in text and "context_forgotten=false" in text
                  for text in forget_lines),
              "generic cache forget was not traced before marking its copied context")
        stop_detour(forgotten["chassis"], 7)
        repeated_lines = trace_lines(forgotten, "stop-before")
        check(sum("source=retained" in text for text in repeated_lines) == 2
              and any("source=retained" in text and "context_forgotten=true" in text
                      for text in repeated_lines),
              "diagnostic-only FlareTarget context did not survive forget for repeated stop")

        # The path-manager cancellation hook observes a valid owned record
        # after Stop, while forwarding the native call and its full RAX value.
        cancel_fixture = resumed_flare_fixture()
        p64(cancel_fixture["gunner"], 0x88, 0)
        stop_detour(cancel_fixture["chassis"], 7)
        path_cancel_fixtures[cancel_fixture["manager"]] = cancel_fixture
        C.c_uint8.from_address(cancel_fixture["records"] + 0x2CC).value = 2
        cancel_result = 0xA1B2C3D400000001
        path_cancel_results[(cancel_fixture["manager"], 0)] = cancel_result
        before_cancel = len(path_cancel_calls)
        observed_cancel = invoke_native_path_cancel(cancel_fixture["manager"], 0)
        check(observed_cancel == cancel_result and len(path_cancel_calls) == before_cancel + 1
              and path_cancel_calls[-1] == (cancel_fixture["manager"], 0,
                                            ptr(cancel_fixture["manager"], 0x288)),
              "path-manager cancel changed native arguments, count, or full RAX result")
        check(u8(cancel_fixture["records"], 0x2CC) == 0,
              "native path-manager callback did not clear a valid route record")
        manager_cancel_lines = trace_lines(cancel_fixture, "manager-cancel-before")
        check(any("source=retained" in text and "context_forgotten=true" in text
                  and "identity_match=true" in text and "kind=0x100" in text
                  for text in manager_cancel_lines),
              f"path-manager cancel missed forgotten FlareTarget scalar context: {manager_cancel_lines}")

        mismatched_header = resumed_flare_fixture()
        p64(mismatched_header["gunner"], 0x88, 0)
        stop_detour(mismatched_header["chassis"], 7)
        path_cancel_fixtures[mismatched_header["manager"]] = mismatched_header
        C.c_uint8.from_address(mismatched_header["records"] + 0x2CC).value = 2
        foreign_header = ptr(route_fixture(base)["manager"], 0x288)
        mismatched_result = 0xBCDE123400000001
        path_cancel_results[(mismatched_header["manager"], 0)] = mismatched_result
        before_cancel = len(path_cancel_calls)
        observed_cancel = path_cancel_detour(
            mismatched_header["manager"], 0, foreign_header)
        check(observed_cancel == mismatched_result and len(path_cancel_calls) == before_cancel + 1
              and path_cancel_calls[-1] == (mismatched_header["manager"], 0, foreign_header),
              "manager wrapper did not preserve a mismatched incoming header through native call")
        mismatch_lines = trace_lines(mismatched_header, "manager-cancel-before")
        check(not mismatch_lines,
              f"manager cancellation diagnostic trusted an unrelated R8 header: {mismatch_lines}")

        # A foreign record owner or invalid index must not produce a retained
        # cancellation diagnostic, and both paths still reach the native call.
        foreign_record = resumed_flare_fixture()
        p64(foreign_record["gunner"], 0x88, 0)
        stop_detour(foreign_record["chassis"], 7)
        path_cancel_fixtures[foreign_record["manager"]] = foreign_record
        foreign_chassis = route_fixture(base)["chassis"]
        p64(foreign_record["records"], 0x258, foreign_chassis)
        C.c_uint8.from_address(foreign_record["records"] + 0x2CC).value = 2
        foreign_result = 0x0FEDCBA900000001
        path_cancel_results[(foreign_record["manager"], 0)] = foreign_result
        before_cancel = len(path_cancel_calls)
        observed_cancel = invoke_native_path_cancel(foreign_record["manager"], 0)
        check(observed_cancel == foreign_result and len(path_cancel_calls) == before_cancel + 1
              and path_cancel_calls[-1] == (foreign_record["manager"], 0,
                                            ptr(foreign_record["manager"], 0x288)),
              "foreign-record cancellation was not transparently forwarded")
        foreign_lines = trace_lines(foreign_record, "manager-cancel-before")
        check(not any("source=retained" in text for text in foreign_lines),
              f"foreign route record owner was reported as an owned FlareTarget cancellation: {foreign_lines}")

        invalid_nav = resumed_flare_fixture()
        p64(invalid_nav["gunner"], 0x88, 0)
        stop_detour(invalid_nav["chassis"], 7)
        path_cancel_fixtures[invalid_nav["manager"]] = invalid_nav
        invalid_result = 0x7654321000000001
        path_cancel_results[(invalid_nav["manager"], 99)] = invalid_result
        before_cancel = len(path_cancel_calls)
        observed_cancel = invoke_native_path_cancel(invalid_nav["manager"], 99)
        check(observed_cancel & 0xFF == 0 and len(path_cancel_calls) == before_cancel,
              "native path-manager bounds check failed to reject an invalid index before its hooked body")
        invalid_lines = trace_lines(invalid_nav, "manager-cancel-before")
        check(not invalid_lines,
              f"invalid navigation index produced a cancellation diagnostic: {invalid_lines}")

        # Calling the shared helper from an unrelated caller must preserve its
        # original direction pointer; it still forwards once and preserves
        # the full 64-bit native return value.
        unrelated_chassis, unrelated_direction = facing["chassis"], (C.c_float * 2)(-1.0, 0.0)
        before = len(helper_calls)
        result = HELPER(heading_detour)(unrelated_chassis, unrelated_direction, C.c_float(0.025))
        check(len(helper_calls) == before + 1 and helper_calls[-1][1] == C.addressof(unrelated_direction)
              and helper_calls[-1][3] == (-1.0, 0.0) and result == helper_result,
              "unrelated helper caller was modified or not forwarded exactly once")

        def expect_heading_passthrough(label, mutate=None, caller=native_rva(0x2C92B7)):
            fixture = route_fixture(base)
            i32(fixture["animation"], 0x6C, 0x17); i32(fixture["animation"], 0x74, 1)
            f32(fixture["target"], 0x14, 4.0); f32(fixture["target"], 0x18, 3.0)
            if mutate:
                mutate(fixture)
            direction = (C.c_float * 2)(0.25, -0.5)
            before = len(helper_calls)
            native, _ = invoke_heading(fixture, caller, direction)
            check(len(helper_calls) == before + 1 and native[1] == C.addressof(direction)
                  and native[3] == (0.25, -0.5) and native[2] == float(C.c_float(0.016).value),
                  f"{label}: invalid heading was not passed through unchanged: {native}, expectedptr={C.addressof(direction):#x}")

        expect_heading_passthrough("wrong selected descriptor",
                                   lambda f: setattr(C.c_uint8.from_address(f["descriptor"] + 0xA9), "value", 0))
        expect_heading_passthrough("wrong grenade shot mode",
                                   lambda f: i32(f["descriptor"], 0xC4, 1))
        expect_heading_passthrough("wrong action field",
                                   lambda f: (i32(f["animation"], 0x68, 0x17), i32(f["animation"], 0x6C, 0)))
        expect_heading_passthrough("wrong posture", lambda f: i32(f["animation"], 0x74, 0))
        expect_heading_passthrough("inactive route", lambda f: C.c_uint8.from_address(f["records"] + 0x2CC).__setattr__("value", 0))
        expect_heading_passthrough("wrong target getter",
                                   lambda f: p64(f["attack_target_vt"], 0x28, base + native_rva(0x46A741)))
        def matching_flare_getter_wrong_vtable(f):
            flare_target, wrong_vt = alloc(0x40), alloc(0x100)
            p64(wrong_vt, 0x28, base + FLARE_POINT_GETTER)
            p64(flare_target, 0, C.cast(wrong_vt, C.c_void_p).value)
            f32(flare_target, 0x10, 3.0); f32(flare_target, 0x14, 4.0); f32(flare_target, 0x18, 0.0)
            p64(f["target_j"], 0x10, flare_target)
        expect_heading_passthrough("FlareTarget getter with wrong vtable",
                                   matching_flare_getter_wrong_vtable)
        expect_heading_passthrough("wrong components animation",
                                   lambda f: p64(f["components"], 0x58, f["other_target"]))
        expect_heading_passthrough("zero target delta", lambda f: (
            f32(f["target"], 0x14, 0.0), f32(f["target"], 0x18, 0.0)))
        expect_heading_passthrough("nonfinite target",
                                   lambda f: f32(f["target"], 0x14, float("inf")))
        expect_heading_passthrough("nonfinite actor position",
                                   lambda f: f32(f["position"], 0, float("nan")))
        huge_neutral = route_fixture(base)
        i32(huge_neutral["animation"], 0x6C, 0x17); i32(huge_neutral["animation"], 0x74, 1)
        f32(huge_neutral["target"], 0x14, 4.0); f32(huge_neutral["target"], 0x18, 3.0)
        f32(huge_neutral["chassis"], 0x84, 1.0e38); f32(huge_neutral["chassis"], 0x88, 1.0e38)
        C.c_uint8.from_address(huge_neutral["chassis"] + 0x108).value = 1
        C.c_uint8.from_address(huge_neutral["chassis"] + 0x10B).value = 1
        direction = (C.c_float * 2)(0.25, -0.5)
        native, _ = invoke_heading(huge_neutral, native_rva(0x2CA0F7), direction)
        check(native[1] == C.addressof(direction) and native[3] == (0.25, -0.5),
              "nonfinite neutral-vector norm did not fail closed")

        # The selector shim calls the two hooked native weapon predicates on
        # the selected grenade. Only the low byte controls the patch guard; the
        # standalone vtable hooks must preserve all u64 bits.
        f = readiness_fixture()
        eligible_value, idle_value = 0xA1B2C3D400000001, 0xE5F6071800000001
        eligible_outputs[f["gun"]] = eligible_value
        requires_idle_outputs[f["gun"]] = idle_value
        check(eligible(f["gun"]) == eligible_value and requires_idle(f["gun"]) == idle_value,
              "weapon predicate hooks did not preserve full native u64 values")

        # Positive moving grenade case: native selected-weapon branch (2),
        # state 2 before and after, pending grenade action 3, exact real weapon
        # predicate slots, and a reserved moving route. Only native false may
        # be promoted, and the two native readiness checks must both return AL=1.
        f = readiness_fixture()
        i32(f["gun"], 0x130, 0x55)
        i32(f["gun"], 0x146, 0x66); i32(f["gun"], 0x147, 0x77)
        selection_fields = (C.string_at(f["gunner"] + 0x90, 4),
                            C.string_at(f["gunner"] + 0x94, 4),
                            C.string_at(f["gunner"] + 0xB8, 4),
                            C.string_at(f["chassis"] + 0xEC, 4),
                            C.string_at(f["animation"] + 0x6C, 4),
                            C.string_at(f["records"] + 0x2CC, 1),
                            C.string_at(f["gun"] + 0x130, 4),
                            C.string_at(f["gun"] + 0x146, 2))
        native_result, call_trace = selector_call(
            f, 0, eligible_value, idle_value)
        check(native_result == 1, "fully eligible moving grenade was not narrowly promoted")
        check((C.string_at(f["gunner"] + 0x90, 4),
               C.string_at(f["gunner"] + 0x94, 4),
               C.string_at(f["gunner"] + 0xB8, 4),
               C.string_at(f["chassis"] + 0xEC, 4),
               C.string_at(f["animation"] + 0x6C, 4),
               C.string_at(f["records"] + 0x2CC, 1),
               C.string_at(f["gun"] + 0x130, 4),
               C.string_at(f["gun"] + 0x146, 2)) == selection_fields,
              "selector hook changed gameplay or weapon-timer fields")
        check([c[0] for c in call_trace if c[0] in
               ("selector", "eligible", "requires_idle")] ==
              ["selector", "eligible", "requires_idle"],
              f"selector and native checks were not called once in order: {call_trace}")

        # Native true is never rewritten. Each listed mismatch keeps native
        # result false; covers failed/absent checks and every narrow state gate.
        f = readiness_fixture()
        expect_selector_guard("eligibility AL false", f,
                              eligible_result=0x1122334400000000)
        f = readiness_fixture()
        trace = expect_selector_guard("requires-idle AL false", f,
                                      idle_result=0x5566778800000000)
        check(any(c == ("selector_requires_idle_result", 0x5566778800000000) for c in trace),
              "requires-idle false AL not observed by selector shim")
        for missing in ("eligible", "requires_idle"):
            f = readiness_fixture()
            expect_selector_guard(f"{missing} not called", f,
                                  expected_eligible=(missing != "eligible"),
                                  expected_idle=(missing != "requires_idle"))
        f = readiness_fixture(); i32(f["gunner"], 0xB8, 1)
        expect_selector_guard("wrong selected branch", f)
        for pending in (1, 4):
            f = readiness_fixture(); i32(f["animation"], 0x6C, pending)
            expect_selector_guard(f"pending action {pending}", f)
        f = readiness_fixture(); i32(f["gunner"], 0x90, 6)
        expect_selector_guard("wrong gunner state", f)
        f = readiness_fixture()
        expect_selector_guard("state changes during native selector", f, change_state=6)
        f = readiness_fixture(); C.c_uint8.from_address(f["records"] + 0x2CC).value = 0
        expect_selector_guard("inactive moving route", f)
        f = readiness_fixture()
        expect_selector_guard("selection changed in native callback", f, change_selected=1)
        f = readiness_fixture(); C.c_uint8.from_address(f["descriptor"] + 0xA9).value = 0
        expect_selector_guard("ordinary weapon descriptor", f)
        for slot in (0x1C0, 0x1F0):
            f = readiness_fixture(); p64(f["gun_vt"], slot, base + 0x123456)
            expect_selector_guard(f"weapon method slot {slot:#x} changed", f)
        f = readiness_fixture()
        result, _ = selector_call(f, 1, eligible_value, idle_value)
        check(result == 1, "native true selector result changed")

        # Moving attack-move may keep the primary rifle selected while a hand
        # grenade remains available as a candidate. The diagnostic must see
        # that inventory without broadening the exception or changing result.
        f = readiness_fixture()
        C.c_uint8.from_address(f["descriptor"] + 0xA9).value = 0
        i32(f["descriptor"], 0xC4, 1)
        grenade_gun, grenade_vt, grenade_descriptor = alloc(0x200), alloc(0x200), alloc(0x200)
        p64(grenade_gun, 0, grenade_vt)
        p64(grenade_vt, 0x160, C.cast(f["get_descriptor"], C.c_void_p).value)
        p64(grenade_vt, 0x1C0, base + GUN_ELIGIBLE)
        p64(grenade_vt, 0x1F0, base + GUN_REQUIRES_IDLE)
        p64(grenade_gun, 0x1A0, grenade_descriptor)
        C.c_uint8.from_address(grenade_descriptor + 0xA9).value = 1
        i32(grenade_descriptor, 0xC4, 3)
        p64(f["gun_array"], 8, grenade_gun)
        p64(f["gunner"], 0x40, f["gun_array"] + 16)
        i32(f["gunner"], 0x94, 0)
        result, _ = selector_call(f, 0)
        check(result == 0, "diagnostic changed native primary-selection result")
        check(any("grenade movement selector diagnostic:" in text
                  and "selected=0" in text and "candidate_mask=0x2" in text
                  for _, text in logs),
              "moving selector diagnostics missed an available grenade while primary stayed selected")

        # Diagnostics must expose selector states outside the narrow gameplay
        # exception gates instead of filtering those observations away.
        f = readiness_fixture(); i32(f["gunner"], 0x90, 1)
        result, _ = selector_call(f, 0)
        check(result == 0, "diagnostic changed native result for unexpected gunner state")
        check(any("grenade movement selector diagnostic:" in text
                  and f"chassis={f['chassis']:#x}" in text and "state=1" in text
                  for _, text in logs),
              "moving selector diagnostics filtered out the state-1 case")
        f = readiness_fixture(); i32(f["gunner"], 0xB8, 1)
        result, _ = selector_call(f, 0)
        check(result == 0, "diagnostic changed native result for unexpected selector branch")
        check(any("grenade movement selector diagnostic:" in text
                  and f"chassis={f['chassis']:#x}" in text and "branch=1" in text
                  for _, text in logs),
              "moving selector diagnostics filtered out the branch-1 case")

        # The new attack-move state hook sees an outer AttackState wrapping an
        # AiAttackOrder, just like the ordinary-move hook, but expects the old
        # order's type to also be AiAttackOrder. It forwards one initial stop
        # with force 4 and retains the route for either grenade action.
        for action in (0x17, 0x2B):
            f = route_fixture(base, attack_move=True)
            order_route(f, order_attack_move, attack_move=True)
            check(any(c[0] == "stop" and c[2] == 4 for c in calls),
                  "attack-move original did not simulate its force-4 initial stop")
            before = len(resumes); queued(f, action)
            check(len(resumes) == before + 1,
                  f"attack-move route did not resume for grenade action {action:#x}")

        def nested_attack_order(f, nested_nonattack=False):
            state_chassis[f["state"]] = f["chassis"]
            nested_incoming = f["incoming"]
            if nested_nonattack:
                other_state, other_state_j, other_incoming = alloc(0x80), alloc(0x40), alloc(8)
                p64(other_state, 0, base + 0x123456)
                p64(other_state_j, 0x10, other_state)
                p64(other_incoming, 0, other_state_j)
                nested_incoming = other_incoming
            nested_orders[f["state"]] = {"fixture": f, "incoming": nested_incoming}
            before_calls = len(calls)
            before_outer = len([c for c in calls if c[0] == "order_attack_move"])
            before_inner = len([c for c in calls if c[0] == "order"])
            order_attack_move(f["state"], C.cast(f["incoming"], C.POINTER(C.c_void_p)))
            nested_orders.pop(f["state"])
            check(len([c for c in calls if c[0] == "order_attack_move"]) == before_outer + 1,
                  "outer attack-move original was not forwarded exactly once")
            check(len([c for c in calls if c[0] == "order"]) == before_inner + 1,
                  "nested ordinary-move original was not forwarded exactly once")
            nested_stops = [c for c in calls[before_calls:] if c[0] == "stop"]
            check(nested_stops == [("stop", f["chassis"], 4), ("stop", f["chassis"], 1)],
                  f"both initial and nested stops must forward once: {nested_stops}")
            check(i32_read(f["chassis"], 0xEC) == 0 and u8(f["records"], 0x2CC) == 0,
                  "nested native-stop shims did not leave route idle")

        # The stock attack-move callback performs its own initial stop, then
        # dispatches an ordinary MoveState order for the same soldier. The
        # inner capture fails because the route is already mode 0, but it must
        # inherit the outer replacement scope and preserve the saved route.
        f = route_fixture(base, attack_move=True)
        nested_attack_order(f)
        before = len(resumes); queued(f, 0x17)
        check(len(resumes) == before + 1 and resumes[-1][0] == f["chassis"],
              "nested ordinary move callback lost the outer saved attack-move route")

        # Once the outer callback has returned, an explicit stop is outside
        # that replacement scope and must invalidate the retained route.
        f = route_fixture(base, attack_move=True)
        nested_attack_order(f)
        stop_detour(f["chassis"], 3)
        before = len(resumes); queued(f, 0x17)
        check(len(resumes) == before, "post-callback explicit stop still resumed movement")

        # Non-attack orders remain explicit invalidations even when nested
        # inside the outer replacement scope.
        f = route_fixture(base, attack_move=True)
        nested_attack_order(f, nested_nonattack=True)
        before = len(resumes); queued(f, 0x17)
        check(len(resumes) == before, "nested non-attack order retained stale movement")

        # The attack-move hook must not accept an ordinary MoveOrder as the
        # old action, and a later non-attack incoming state must evict a cache.
        f = route_fixture(base)
        order_route(f, order_attack_move, attack_move=True)
        before = len(resumes); queued(f, 0x17)
        check(len(resumes) == before, "attack-move hook accepted old MoveOrder")

        f = route_fixture(base, attack_move=True)
        order_route(f, order_attack_move, attack_move=True)
        unrelated, unrelated_j = alloc(0x80), alloc(0x40)
        p64(unrelated, 0, base + 0x123456)  # Deliberately unrecognized incoming state type.
        p64(unrelated_j, 0x10, unrelated); p64(f["incoming"], 0, unrelated_j)
        order_route(f, order_attack_move, attack_move=True)
        before = len(resumes); queued(f, 0x17)
        check(len(resumes) == before, "non-attack incoming state retained attack-move cache")

        # Baseline: capture a route at order time; native copyref and resume run
        # once on an eligible actual grenade, and the requested action queues once.
        f = route_fixture(base)
        order_route(f)
        start_resume = len(resumes); queued(f, 0x17)
        check(len(resumes) == start_resume + 1, "confirmed grenade did not resume native route")
        call = resumes[-1]
        check(call[0] == f["chassis"] and call[1] == f["move_j"], "resume got wrong chassis/target reference")
        check(abs(call[2] - 1.25) < 1e-6 and call[3] is None, "resume speed or null end direction changed")
        check(C.c_int32.from_address(f["move_j"] + 8).value == 8 and
              C.c_int32.from_address(f["move_j"] + 0x18).value == 18,
              "native copy-reference counts did not balance")
        check(i32_read(f["chassis"], 0xEC) == 1, "resume shim did not model restored movement mode")

        # A retained aim endpoint is passed to native resume, with direction
        # normalized and route mode/navigation preserved by the aim-turn tail.
        f = route_fixture(base)
        aim_stub_addr = base + native_rva(0x2D385C)
        aim_stub_len = 41
        aim_saved = C.string_at(aim_stub_addr, aim_stub_len)
        aim_restore = (aim_stub_addr, aim_saved)
        direction = (C.c_float * 2)(3.0, 4.0)
        detour_addr = C.cast(turn_direction, C.c_void_p).value
        aim_code = (b"\x48\x83\xec\x28\x48\xb9" + f["chassis"].to_bytes(8, "little") +
                    b"\x48\xba" + C.addressof(direction).to_bytes(8, "little") +
                    b"\x48\xb8" + detour_addr.to_bytes(8, "little") + b"\xff\xd0"
                    b"\x48\x83\xc4\x28\xc3")
        check(len(aim_code) <= aim_stub_len and native_rva(0x2D385C) + 4 + 10 + 10 + 10 + 2 == native_rva(0x2D3880),
              "aim-call stub does not return at the recognized RVA")
        put_code(aim_stub_addr, aim_code)
        old_turns = len([c for c in calls if c[0] == "turn"])
        C.CFUNCTYPE(None)(aim_stub_addr)()
        check(len([c for c in calls if c[0] == "turn"]) == old_turns,
              "recognized grenade aim direction was forwarded to stock clearing path")
        check(i32_read(f["chassis"], 0xEC) == 1 and i32_read(f["chassis"], 0xC4) == 0 and
              u8(f["chassis"], 0x10B) == 1, "aim path changed mode/navigation/latch")
        check(abs(C.c_float.from_address(f["chassis"] + 0x110).value - 0.6) < 1e-6 and
              abs(C.c_float.from_address(f["chassis"] + 0x114).value - 0.8) < 1e-6,
              "aim direction tail did not normalize direction")
        order_route(f)
        start_resume = len(resumes); queued(f, 0x2B)
        check(len(resumes) == start_resume + 1 and resumes[-1][3] is not None and
              abs(resumes[-1][3][0] - 0.6) < 1e-6 and abs(resumes[-1][3][1] - 0.8) < 1e-6,
              "retained endpoint direction was not delivered to native resume")

        # The manager may mark a freshly queued moving route as state 3 before
        # its attack order transitions into state 2. The aim guard accepts both.
        f = route_fixture(base)
        C.c_uint8.from_address(f["records"] + 0x2CC).value = 3
        direction3 = (C.c_float * 2)(0.0, 2.0)
        p3 = b"\x48\x83\xec\x28\x48\xb9" + f["chassis"].to_bytes(8, "little") + b"\x48\xba" + C.addressof(direction3).to_bytes(8, "little") + b"\x48\xb8" + detour_addr.to_bytes(8, "little") + b"\xff\xd0\x48\x83\xc4\x28\xc3"
        put_code(aim_stub_addr, p3)
        before_turns = len([c for c in calls if c[0] == "turn"])
        C.CFUNCTYPE(None)(aim_stub_addr)()
        check(len([c for c in calls if c[0] == "turn"]) == before_turns and
              C.c_uint8.from_address(f["chassis"] + 0x10B).value == 1 and
              C.c_uint8.from_address(f["records"] + 0x2CC).value == 3,
              "state-3 aim request did not preserve route and avoid stock turn")
        check(abs(C.c_float.from_address(f["chassis"] + 0x110).value) < 1e-6 and
              abs(C.c_float.from_address(f["chassis"] + 0x114).value - 1.0) < 1e-6,
              "state-3 aim direction was not normalized")
        C.c_uint8.from_address(f["records"] + 0x2CC).value = 2
        order_route(f)
        start_resume = len(resumes); queued(f, 0x17)
        check(len(resumes) == start_resume + 1 and resumes[-1][3] is not None and
              abs(resumes[-1][3][1] - 1.0) < 1e-6,
              "state-3 aim endpoint was not retained through attack-order resume")
        put_code(aim_stub_addr, aim_saved)
        aim_restore = None

        # A non-grenade animation forwards normally and leaves the route cache
        # available for a subsequent verified grenade queue.
        f = route_fixture(base); order_route(f)
        start_resume = len(resumes); queued(f, 0x18)
        check(len(resumes) == start_resume, "weapon switch incorrectly resumed route")
        queued(f, 0x17)
        check(len(resumes) == start_resume + 1, "non-grenade queue destroyed the retained route")

        # Each changed identity/state invalidates the one-shot cached route.
        def expect_no_resume(label, change=None, stop_first=False, no_cache=False, turn_first=False):
            fixture = route_fixture(base)
            if not no_cache:
                order_route(fixture)
            if change:
                change(fixture)
            if stop_first:
                stop_detour(fixture["chassis"], 7)
            if turn_first:
                point = (C.c_float * 3)(1.0, 2.0, 3.0)
                turn_point(fixture["chassis"], point)
            before = len(resumes); queued(fixture, 0x17)
            check(len(resumes) == before, f"{label} incorrectly resumed stale movement")
            return fixture

        expect_no_resume("no cache", no_cache=True)
        expect_no_resume("explicit stop", stop_first=True)
        expect_no_resume("ordinary turn", turn_first=True)
        expect_no_resume("changed attack target", lambda f: (
            p64(f["gunner"], 0x88, f["other_target_j"])))
        expect_no_resume("changed navigation id", lambda f: i32(f["chassis"], 0xC4, 1))
        expect_no_resume("changed unit", lambda f: p64(f["chassis_j"], 0x10, f["other_unit"]))
        expect_no_resume("changed manager", lambda f: p64(f["chassis"], 0x30, f["other_manager"]))
        expect_no_resume("changed move target", lambda f: p64(f["chassis"], 0xD8, f["other_move_j"]))
        expect_no_resume("selected weapon not grenade", lambda f: i32(f["descriptor"], 0xC4, 4))
        expect_no_resume("selected weapon deploy-trailer mode", lambda f: i32(f["descriptor"], 0xC4, 5))
        expect_no_resume("selected weapon incompatible", lambda f: setattr(C.c_uint8.from_address(f["descriptor"] + 0xA9), "value", 0))
        expect_no_resume("gunner mode not grenade", lambda f: i32(f["gunner"], 0x90, 1))

        # Weapon mode 3 is the grenade mode; gunner runtime states 2, 3, and 5
        # all remain valid for that selected weapon.
        for gunner_state in (2, 3, 5):
            f = route_fixture(base); i32(f["gunner"], 0x90, gunner_state); order_route(f)
            before = len(resumes); queued(f, 0x17 if gunner_state != 5 else 0x2B)
            check(len(resumes) == before + 1,
                  f"grenade selected in gunner state {gunner_state} did not resume movement")

        f = route_fixture(base); order_route(f)
        before = len(resumes); queued(f, 0x18)
        check(len(resumes) == before, "ordinary action unexpectedly resumed movement")

        # Selector results never move the actor early. Failed native readiness
        # retains the stopped flare route so a later successful throw queue can
        # resume it exactly once.
        def select_rifle_with_unselected_grenade(f):
            C.c_uint8.from_address(f["descriptor"] + 0xA9).value = 0
            second_gun, second_vt, second_descriptor = alloc(0x200), alloc(0x200), alloc(0x200)
            p64(second_gun, 0, second_vt)
            p64(second_vt, 0x160, C.cast(f["get_descriptor"], C.c_void_p).value)
            p64(second_gun, 0x1A0, second_descriptor)
            C.c_uint8.from_address(second_descriptor + 0xA9).value = 1
            i32(second_descriptor, 0xC4, 3)
            array = alloc(16); p64(array, 0, f["gun"]); p64(array, 8, second_gun)
            p64(f["gunner"], 0x38, array); p64(f["gunner"], 0x40, array + 16)
            i32(f["gunner"], 0x94, 0)

        for label, mutate in (
            ("unselected hand grenade while rifle selected", select_rifle_with_unselected_grenade),
            ("wrong gunner state", lambda f: i32(f["gunner"], 0x90, 6)),
            ("wrong flare kind", lambda f: i32(f["target"], 0x28, 0x200)),
            ("changed selected target", lambda f: p64(f["gunner"], 0x88, f["other_target_j"])),
        ):
            f = route_fixture(base); attach_flare_target(f); order_route(f)
            i32(f["gunner"], 0x90, 2); i32(f["gunner"], 0xB8, 2)
            i32(f["animation"], 0x68, 1); i32(f["animation"], 0x6C, 1)
            i32(f["animation"], 0x74, 1)
            mutate(f)
            before = len(resumes)
            selector_call(f, native_result=1)
            check(len(resumes) == before,
                  f"native selector resumed movement early for {label}")

        f = captured_flare_fixture()
        before = len(resumes)
        failed_trace = selector_call(f, native_result=0, eligible_result=0)
        check(failed_trace[0] == 0 and len(resumes) == before
              and i32_read(f["chassis"], 0xEC) == 0 and u8(f["records"], 0x2CC) == 0,
              "native-false/eligibility-false selector consumed or resumed the stopped route")
        check(selector_snapshots[-1][1:4] == (0, 0, 2),
              f"failed readiness callback did not observe stopped route: {selector_snapshots[-1]}")
        selector_call(f, native_result=1, eligible_result=1)
        check(len(resumes) == before and i32_read(f["chassis"], 0xEC) == 0,
              "successful native readiness resumed movement before the throw queue")
        queued(f, 0x17)
        check(len(resumes) == before + 1 and i32_read(f["chassis"], 0xEC) == 1,
              "later grenade queue did not resume the route retained through failed readiness")
        queued(f, 0x17)
        check(len(resumes) == before + 1,
              "repeated grenade queue resumed the one-shot route more than once")

        for branch in (2, 1):
            f = captured_flare_fixture(branch=branch)
            before = len(resumes)
            result, _ = selector_call(f, native_result=0, eligible_result=1)
            check(result == 0 and len(resumes) == before
                  and i32_read(f["chassis"], 0xEC) == 0
                  and u8(f["records"], 0x2CC) == 0,
                  f"native-false selected branch {branch} was overridden before queue")

        # Native selector readiness runs with movement stopped. Only the
        # confirmed throw queue resumes; a later gunner-release Stop still
        # reaches native Submit unchanged before deferred Move restoration.
        for attack_move, kind in ((False, 0x100), (True, 0x100), (False, 0x4000)):
            f = route_fixture(base, attack_move=attack_move)
            submit = prepare_submit_fixture(f, attack_move=attack_move)
            attach_flare_target(f, kind=kind)
            route_order_fn = order_attack_move if attack_move else order
            order_route(f, route_order_fn, attack_move=attack_move)
            before_resume = len(resumes)
            i32(f["gunner"], 0x90, 2)
            i32(f["animation"], 0x68, 1); i32(f["animation"], 0x6C, 1)
            i32(f["animation"], 0x74, 1)
            selector_call(f, native_result=1)
            check(len(resumes) == before_resume and i32_read(f["chassis"], 0xEC) == 0
                  and selector_snapshots[-1][1] == 0,
                  "native readiness callback did not observe a stopped route")
            queued(f, 0x17)
            check(len(resumes) == before_resume + 1,
                  "throw queue did not resume movement after native readiness")
            i32(f["gunner"], 0x90, 5)
            submits_before = len(submit_calls)
            cleanup_before = len(weapon_clear_calls)
            incoming_value = ptr(submit["incoming"])
            invoke_submit_from_native_caller(submit["ai"], submit["incoming"])
            check(len(submit_calls) == submits_before + 1,
                  f"native HumanAI submit was not forwarded exactly once: calls={submit_calls} logs={logs[-8:]}")
            native_submit = submit_calls[-1]
            check(native_submit[0] == submit["ai"] and native_submit[2] == submit["stop_junction"]
                  and native_submit[3] == submit["stop_order"] and native_submit[4] == 0.0,
                  f"native Stop was not forwarded unchanged: got={native_submit} stop_order={submit['stop_order']:#x}")
            check(ptr(submit["incoming"]) == incoming_value,
                  "successful handoff mutated the caller's incoming Stop junction")
            check(ptr(f["gunner"], 0x88) == f["target_j"]
                  and len(weapon_clear_calls) == cleanup_before,
                  "Stop/Move continuation cleared the live flare before native Stop initialization")
            check(len(submit_calls) == submits_before + 1,
                  "deferred movement was submitted before AI update completion")
            stop_init_fixtures[submit["stop_order"]] = submit
            ai_update_fixtures[submit["ai"]] = submit
            move_factory_before = len(move_factory_calls)
            attack_factory_before = len(attack_factory_calls)
            bind_before = len(bind_calls)
            ai_update_detour(submit["ai"], 0.016)
            check(len(stop_init_calls) >= 1 and stop_init_calls[-1] == submit["stop_order"]
                  and u8(submit["stop_order"], 0x11) == 1,
                  "native Stop init did not finish the retained Stop order")
            check(ptr(f["gunner"], 0x88) == 0 and i32_read(f["gunner"], 0x98) == 0
                  and i32_read(f["chassis"], 0xEC) == 0 and u8(f["records"], 0x2CC) == 0,
                  "Stop initialization did not clear flare state and retire route mode")
            cleanup = weapon_clear_calls[-2:]
            check(len(weapon_clear_calls) == cleanup_before + 2,
                  "Stop initialization did not clear exactly the two fixture weapon targets")
            check([entry[0] for entry in cleanup] == list(submit["weapons"])
                  and all(entry[1] == 0 for entry in cleanup),
                  f"native Stop initialization did not clear each gun target: {cleanup}")
            check(len(submit_calls) == submits_before + 2,
                  "AI update tail did not submit exactly one deferred movement order")
            native_move = submit_calls[-1]
            old_order = submit["old_order"]
            fresh = submit.get("fresh_order", 0)
            check(native_move[0] == submit["ai"] and native_move[2] != submit["stop_junction"]
                  and fresh != 0 and native_move[3] == fresh and fresh != old_order and native_move[4] == 0.0,
                  f"AI update tail did not submit a fresh native movement order: {native_move} fresh={fresh:#x}")
            check(u8(old_order, 0x11) == 1 and u8(fresh, 0x11) == 0,
                  "native order interruption did not finish only the old order")
            done_getter = C.CFUNCTYPE(C.c_uint8, C.c_void_p)(
                ptr(base + (ATTACK_ORDER_VT if attack_move else MOVE_ORDER_VT), 0x50))
            check(done_getter(old_order) == 1 and done_getter(fresh) == 0,
                  "native order done-state query did not distinguish the interrupted and fresh order")
            check(ptr(fresh, 0x18) == submit["unit_j"] and ptr(submit["unit_j"], 0x10) == submit["unit"],
                  "fresh native movement order lost the original owning unit")
            if attack_move:
                check(len(attack_factory_calls) == attack_factory_before + 1
                      and len(move_factory_calls) == move_factory_before,
                      "AttackMove handoff did not use its native fresh-order factory")
                check(ptr(fresh, 0x28) == submit["attack_target_j"],
                      "fresh AttackMove order did not preserve its native target junction")
            else:
                check(len(move_factory_calls) == move_factory_before + 1
                      and len(attack_factory_calls) == attack_factory_before,
                      "ordinary Move handoff did not use its native fresh-order factory")
                check(move_factory_calls[-1] == (submit["repository"], submit["unit"], 7,
                                                  submit["destination_object"]),
                      f"ordinary Move factory arguments did not preserve repository/speed/destination: {move_factory_calls[-1]}")
                check(ptr(ptr(fresh, 0x48), 0x10) == submit["destination_object"]
                      and i32_read(fresh, 0x60) == i32_read(old_order, 0x60),
                      "fresh ordinary Move lost its destination or native speed enum")
                check(i32_read(fresh, 0x14) == i32_read(old_order, 0x14)
                      and f32_read(fresh, 0x54) == f32_read(old_order, 0x54)
                      and f32_read(fresh, 0x58) == f32_read(old_order, 0x58)
                      and u8(fresh, 0x5C) == u8(old_order, 0x5C)
                      and u8(fresh, 0x68) == u8(old_order, 0x68),
                      "fresh ordinary Move did not preserve flags and end-direction settings")
            check(len(bind_calls) == bind_before + 1 and bind_calls[-1] == (native_move[2], fresh),
                  "fresh order did not pass through the native BindObject ownership boundary")
            check(submit_cleanup_snapshots[-1][0:2] == (0, 0),
                  "deferred Move was submitted before Stop cleanup became visible")
            check(submit["stop_order"] not in [native_move[3]],
                  "deferred movement reused the Stop order object")
            check(i32_read(native_move[2], 8) == 0 and i32_read(native_move[2], 0x18) == 0,
                  "native Submit did not consume the fresh order junction references")
            check(i32_read(submit["stop_junction"], 8) == 0
                  and i32_read(submit["stop_junction"], 0x18) == 0
                  and i32_read(f["old_move_j"], 8) == 1
                  and i32_read(f["old_move_j"], 0x18) == 1,
                  "deferred Stop/Move transfer did not balance both owned junction references")
            check(any(text.startswith("moving flare order handoff:") and
                      f"kind={kind:#x}" in text and f"attack_move={str(attack_move).lower()}" in text
                      and "native_stop_finished=true" in text and "target_cleared=true" in text
                      and "fresh_order=true" in text and "old_finished=true" in text
                      for _, text in logs),
                  "successful FlareTarget deferred handoff diagnostic missing")

        # Stop initialization alone cannot run a deferred Move: only a later
        # AI update tail is allowed to complete the transfer.
        f = readiness_resumed_flare_fixture(); submit = f["submit_fixture"]
        i32(f["gunner"], 0x90, 5)
        invoke_submit_from_native_caller(submit["ai"], submit["incoming"])
        submits_before = len(submit_calls)
        stop_init_detour(submit["stop_order"])
        check(len(submit_calls) == submits_before and u8(submit["stop_order"], 0x11) == 1,
              "finished Stop incorrectly restored movement without an enclosing AI update")

        # A controller update that does not finish native Stop cannot restore
        # the Move; finishing it afterward still waits for the next update.
        f = readiness_resumed_flare_fixture(); submit = f["submit_fixture"]
        i32(f["gunner"], 0x90, 5)
        invoke_submit_from_native_caller(submit["ai"], submit["incoming"])
        submits_before = len(submit_calls)
        ai_update_fixtures[submit["ai"]]["run_stop_init"] = False
        ai_update_detour(submit["ai"], 0.016)
        check(len(submit_calls) == submits_before,
              "unfinished Stop restored movement during AI update")
        stop_init_detour(submit["stop_order"])
        check(len(submit_calls) == submits_before,
              "Stop completion outside the controller tick restored movement immediately")
        ai_update_detour(submit["ai"], 0.016)
        check(len(submit_calls) == submits_before + 1
              and submit_calls[-1][3] == submit.get("fresh_order")
              and submit_calls[-1][3] != submit["old_move"],
              "completed Stop was not restored on the next AI update tail")

        # A changed post-Stop identity rejects and drops the owned continuation.
        for label, mutate in (
            ("unit", lambda f: p64(f["chassis_j"], 0x10, f["other_unit"])),
            ("manager", lambda f: p64(f["chassis"], 0x30, f["other_manager"])),
            ("navigation", lambda f: i32(f["chassis"], 0xC4, 1)),
            ("move target", lambda f: p64(f["chassis"], 0xD8, f["other_move_j"])),
        ):
            f = readiness_resumed_flare_fixture(); submit = f["submit_fixture"]
            i32(f["gunner"], 0x90, 5)
            invoke_submit_from_native_caller(submit["ai"], submit["incoming"])
            mutate(f)
            ai_update_fixtures[submit["ai"]]["run_stop_init"] = False
            stop_init_detour(submit["stop_order"])
            before = len(submit_calls); release_before = len(release_calls)
            ai_update_detour(submit["ai"], 0.016)
            check(len(submit_calls) == before,
                  f"changed {label} identity restored the retained movement order")
            released = release_calls[release_before:]
            check(submit["stop_junction"] in released and f["old_move_j"] in released,
                  f"rejected {label} identity leaked retained junction ownership: {released}")

        f = readiness_resumed_flare_fixture(); submit = f["submit_fixture"]
        i32(f["gunner"], 0x90, 5)
        invoke_submit_from_native_caller(submit["ai"], submit["incoming"])
        ai_update_fixtures[submit["ai"]]["run_stop_init"] = True
        ai_update_fixtures[submit["ai"]]["after_stop_init"] = lambda fixture: p64(
            fixture["gunner"], 0x88, fixture["other_target_j"])
        before = len(submit_calls)
        ai_update_detour(submit["ai"], 0.016)
        check(len(submit_calls) == before and ptr(f["gunner"], 0x88) == f["other_target_j"],
              "a changed live target after Stop initialization restored the retained movement")

        # A newly submitted order cancels a completed Stop continuation.
        f = readiness_resumed_flare_fixture(); submit = f["submit_fixture"]
        i32(f["gunner"], 0x90, 5)
        invoke_submit_from_native_caller(submit["ai"], submit["incoming"])
        new_order, new_junction, new_incoming = alloc(0x80), alloc(0x40), alloc(8)
        p64(new_order, 0, base + MOVE_ORDER_VT); p64(new_order, 0x18, f["unit_j"])
        i32(new_junction, 8, 1); i32(new_junction, 0x18, 1); p64(new_junction, 0x10, new_order)
        p64(new_incoming, 0, new_junction)
        submit_detour(submit["ai"], C.cast(new_incoming, C.POINTER(C.c_void_p)), 0.0)
        check(submit_calls[-1][2] == new_junction and submit_calls[-1][3] == new_order,
              "new explicit order was not forwarded unchanged")
        before = len(submit_calls)
        ai_update_fixtures[submit["ai"]]["run_stop_init"] = False
        ai_update_detour(submit["ai"], 0.016)
        check(len(submit_calls) == before,
              "a later AI update restored movement after a new explicit order")
        # Explicit cancellation forgets the retained order/completion, so a
        # later Stop or AI update cannot resurrect it.
        f = route_fixture(base); submit = prepare_submit_fixture(f)
        attach_flare_target(f); order_route(f); queued(f, 0x17); i32(f["gunner"], 0x90, 5)
        release_before, submit_before, cleanup_before = len(release_calls), len(submit_calls), len(weapon_clear_calls)
        stop_detour(f["chassis"], 7)
        invoke_submit_from_native_caller(submit["ai"], submit["incoming"])
        check(len(submit_calls) == submit_before + 1 and submit_calls[-1][3] == submit["stop_order"],
              "explicit Stop incorrectly restored the retained movement order")
        check(len(release_calls) - release_before == 2,
              "explicit Stop and subsequent native submit did not release both owned junctions")
        check(len(weapon_clear_calls) == cleanup_before and ptr(f["gunner"], 0x88) == f["target_j"],
              "explicit Stop rejection unexpectedly cleared the live FlareTarget")

        # A changed live navigation identity rejects an otherwise valid
        # retained order and still forwards/consumes the caller's Stop order.
        f = route_fixture(base); submit = prepare_submit_fixture(f)
        attach_flare_target(f); order_route(f); queued(f, 0x17); i32(f["gunner"], 0x90, 5)
        i32(f["chassis"], 0xC4, 1)
        release_before, submit_before, cleanup_before = len(release_calls), len(submit_calls), len(weapon_clear_calls)
        invoke_submit_from_native_caller(submit["ai"], submit["incoming"])
        check(len(submit_calls) == submit_before + 1 and submit_calls[-1][3] == submit["stop_order"],
              "changed navigation identity was allowed to restore the retained order")
        check(len(release_calls) - release_before == 2,
              "rejected retained route leaked its junction or failed to consume native Stop")
        check(len(weapon_clear_calls) == cleanup_before and ptr(f["gunner"], 0x88) == f["target_j"],
              "stale navigation rejection unexpectedly cleared the live FlareTarget")

        # No successful grenade resume means the saved order is ineligible.
        f = route_fixture(base); submit = prepare_submit_fixture(f)
        attach_flare_target(f); order_route(f)
        release_before, submit_before, cleanup_before = len(release_calls), len(submit_calls), len(weapon_clear_calls)
        invoke_submit_from_native_caller(submit["ai"], submit["incoming"])
        check(len(submit_calls) == submit_before + 1 and submit_calls[-1][3] == submit["stop_order"],
              "unresumed route was incorrectly restored during Submit")
        check(len(release_calls) - release_before == 2,
              "unresumed retained order did not release its junction")
        check(len(weapon_clear_calls) == cleanup_before and ptr(f["gunner"], 0x88) == f["target_j"],
              "unresumed route rejection unexpectedly cleared the live FlareTarget")

        # A Submit from an unrelated caller consumes the pending continuation
        # rather than letting a later genuine gunner-release restore it.
        f = route_fixture(base); submit = prepare_submit_fixture(f)
        attach_flare_target(f); order_route(f); queued(f, 0x17); i32(f["gunner"], 0x90, 5)
        release_before, submit_before = len(release_calls), len(submit_calls)
        submit_detour(submit["ai"], C.cast(submit["incoming"], C.POINTER(C.c_void_p)), 0.0)
        check(len(submit_calls) == submit_before + 1 and submit_calls[-1][3] == submit["stop_order"],
              "unrelated Submit caller restored the retained movement order")
        check(len(release_calls) - release_before == 2,
              "unrelated Submit failed to release pending and incoming junctions")
        incoming_value = ptr(submit["incoming"])
        release_before = len(release_calls)
        invoke_submit_from_native_caller(submit["ai"], submit["incoming"])
        check(submit_calls[-1][3] == submit["stop_order"] and ptr(submit["incoming"]) == incoming_value
              and len(release_calls) - release_before == 1,
              "consumed continuation was restored on a later genuine release")

        # Diagnostics are deduplicated by full moving selector outcome and
        # capped at 64. The original v1.6 gameplay gate still remains exact.
        selector_reports = [text for _, text in logs
                            if text.startswith("grenade movement selector diagnostic:")]
        check(0 < len(selector_reports) <= 64,
              f"bounded moving-selector diagnostic cap failed: {len(selector_reports)}")
        check(all(level == 3 for level, text in logs
                  if text.startswith("grenade movement selector diagnostic:")),
              "moving-selector diagnostics are not debug")
        check(logs[0][0] == 0, f"startup report is not info: {logs[0]}")
        check(logs[0][1] == "grenade movement fix installed",
              f"unexpected plugin startup report: {logs[0]}")

        # A zero result from the native movement handoff is an exceptional
        # condition. Verify it is reported, but capped independently of the
        # number of subsequent failed grenade requests.
        failed = []
        for _ in range(10):
            fixture = route_fixture(base); order_route(fixture)
            resume_results[fixture["chassis"]] = 0
            before = len(resumes); queued(fixture, 0x17)
            check(len(resumes) == before + 1, "failed native resume skipped its original call")
            failed.append(fixture["chassis"])
        failures = [text for _, text in logs
                    if text.startswith("grenade movement native resume failed:")]
        check(len(failures) == 8, f"native failure log cap changed: {len(failures)}")
    finally:
        if aim_restore is not None:
            put_code(*aim_restore)
        put_code(resume_addr, saved_resume)
        put_code(base + COPYREF, saved_copyref)
        put_code(base + RELEASE_REF, saved_release_ref)
        put_code(base + AUX_RELEASE_IAT, saved_aux_release.to_bytes(8, "little"))
        for address, saved in native_factory_patches.items():
            put_code(address, saved)
        K.FreeLibrary(base)
    print(f"PASS: {BUILD_NAME}: verified ABI/sixteen hooks, selector transparency and guards, unselected-grenade candidate gate/TLS and target revalidation, FlareTarget heading and deferred Stop/Move completion, retained-route and manager-cancel diagnostics, "
          "nested attack-to-move route retention/invalidation, initial-stop TLS, native copyref balance, "
          "grenade-only resume, stale-route rejection, aim preservation for route states 2/3, "
          "target-based movement-facing including dynamic targets and the one-turn route branch, "
          "bounded moving-selector diagnostics and native-resume error logging; "
          "shim harness only, not live gameplay evidence.")


def i32_read(address, offset=0):
    return C.c_int32.from_address(address + offset).value


def f32_read(address, offset=0):
    return C.c_float.from_address(address + offset).value


if __name__ == "__main__":
    main()
