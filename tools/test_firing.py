"""Run patch/firing-mode.asm natively on fabricated squads and soldiers.

Each routine is entered the way its site enters it: the squad's setter and
getter and the soldier's getter by a call (their jmps sit at function entry),
fn_2caeb0's read by a call with the soldier's AI in rsi. The soldier getter's
one branch back into logic.dll is re-aimed, as the injector does, at a
stand-in for the stock path that answers 0x5a, so taking it is visible.
Fabricated objects have the layout the routines read: squad AI +0x10 ->
holder +0x10 -> squad entity; entity vt+0xb0 -> facets, facets +0x28 the AI
and +0x50 the selectable facet; a soldier's facet +0x18 enabled, +0x28 his
squad's facet, +0x30 his mark (vt+0x58 answers enabled and marked), +0x1b the
firing pin, +0x1c its marker.
"""
import ctypes, pathlib, struct, sys
from ctypes import wintypes
import keystone
sys.path.insert(0, "tools")
import build as b

ks = keystone.Ks(keystone.KS_ARCH_X86, keystone.KS_MODE_64)
asm = lambda text: bytes(ks.asm(text.encode())[0])

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
kernel32.VirtualAlloc.restype = ctypes.c_void_p
kernel32.VirtualAlloc.argtypes = [ctypes.c_void_p, ctypes.c_size_t, wintypes.DWORD, wintypes.DWORD]


HEAP = []

def scratch(size):
    addr = kernel32.VirtualAlloc(None, max(size, 8), 0x3000, 0x40)
    if not addr:
        raise OSError("VirtualAlloc failed")
    HEAP.append(addr)
    return addr


def put(addr, data):
    ctypes.memmove(addr, data, len(data))


def q(addr):
    return ctypes.c_uint64.from_address(addr).value


def byte(addr):
    return ctypes.c_ubyte.from_address(addr).value


def obj(size, fields=()):
    addr = scratch(size)
    for offset, value in fields:
        put(addr + offset, struct.pack("<Q", value))
    HEAP.append(addr)
    return addr


region = scratch(0x10000)
block = region
code, labels = b.assemble(pathlib.Path("patch/firing-mode.asm").read_text().splitlines(),
                          b.FIRING_OFFSET, b.CURSOR_OFFSET)
put(block + b.FIRING_OFFSET, code)
STOCK = region + 0x8000
put(STOCK, asm("mov eax, 0x5a; ret"))
aimed = 0
for offset, target in b.module_branches(code, b.FIRING_OFFSET):
    assert target == 0x2ae8f5, hex(target)
    put(block + offset, struct.pack("<i", STOCK - (block + offset + 4)))
    aimed += 1

FIELD8 = obj(0x10)
put(FIELD8, asm("mov rax, qword ptr [rcx + 8]; ret"))
GETTER = obj(0x10)
put(GETTER, asm("movzx eax, byte ptr [rcx + 0x30]; and al, byte ptr [rcx + 0x18]; ret"))
VEHICLE_Q = obj(0x10)
put(VEHICLE_Q, asm("movzx eax, byte ptr [rcx + 8]; ret"))
ENTITY_VT = obj(0xb8, [(0xb0, FIELD8)])
AI_VT = obj(0x3c0, [(0x3b8, FIELD8)])
ROSTER_VT = obj(0x70, [(0x68, FIELD8)])
FACET_VT = obj(0x60, [(0x58, GETTER)])
VEHICLE_VT = obj(0x90, [(0x88, VEHICLE_Q)])


def soldier(marked=False, enabled=True, pin=None, junk=False):
    facet = obj(0x40, [(0, FACET_VT)])
    put(facet + 0x18, bytes([int(enabled)]))
    put(facet + 0x30, bytes([int(marked)]))
    if junk:
        put(facet + 0x1a, bytes([0x33, 0x44]))           # heap leftovers, no marker
    if pin is not None:
        put(facet + 0x1a, bytes([0x77, pin]) + struct.pack("<H", 0x7a5f))
    facets = obj(0x60, [(0x50, facet)])
    entity = obj(0x28, [(0, ENTITY_VT), (8, facets)])
    return entity, facet


def squad(members, flag):
    """members: (entity, facet) pairs. Returns the squad's AI."""
    pointers = obj(max(8, 8 * len(members)), [(i * 8, e) for i, (e, _) in enumerate(members)])
    vector = obj(0x10, [(0, pointers), (8, pointers + 8 * len(members))])
    roster = obj(0x10, [(0, ROSTER_VT), (8, vector)])
    ai_list = obj(0x10, [(0, AI_VT), (8, roster)])
    own = obj(0x40, [(0, FACET_VT)])
    for _, f in members:
        put(f + 0x28, struct.pack("<Q", own))
    facets = obj(0x60, [(0x28, ai_list), (0x50, own)])
    entity = obj(0x28, [(0, ENTITY_VT), (8, facets)])
    holder = obj(0x18, [(0x10, entity)])
    squad_ai = obj(0x300, [(0x10, holder)])
    put(squad_ai + 0x228, bytes([flag]))
    return squad_ai


def soldier_ai(entity, in_vehicle=None):
    holder = obj(0x18, [(0x10, entity)])
    ai = obj(0x200, [(0x10, holder)])
    if in_vehicle is not None:
        v = obj(0x10, [(0, VEHICLE_VT)])
        put(v + 8, bytes([int(in_vehicle)]))
        put(ai + 0x1f0, struct.pack("<Q", obj(0x18, [(0x10, v)])))
    return ai


def callable_(label, *argtypes, ret=ctypes.c_uint64):
    return ctypes.CFUNCTYPE(ret, *argtypes)(block + labels[label])


SET = callable_("firing_set", ctypes.c_void_p, ctypes.c_ubyte, ret=None)
UI = callable_("firing_ui", ctypes.c_void_p)
SOLDIER = callable_("firing_soldier", ctypes.c_void_p)
FLAG = callable_("squad_flag", ctypes.c_void_p)
# fn_2caeb0's read: rsi the soldier's AI, rcx the squad's
CALL_ARGS = scratch(0x10)
site = scratch(0x100)
put(site, asm(f"push rsi; sub rsp, 0x20; mov rsi, {CALL_ARGS}; mov rcx, qword ptr [rsi]; "
              "mov rsi, qword ptr [rsi + 8]; "
              f"mov rax, {block + labels['firing_call']}; call rax; add rsp, 0x20; pop rsi; ret"))
CALL = ctypes.CFUNCTYPE(ctypes.c_uint64)(site)

failures = 0


def check(label, ok, detail=""):
    global failures
    if not ok:
        failures += 1
    print(f"{'PASS' if ok else 'FAIL'}  {label}{'' if ok else '  ' + detail}")


def pin_of(facet):
    return (byte(facet + 0x1b), ctypes.c_uint16.from_address(facet + 0x1c).value)


def main():
    global failures
    failures = 0
    print(f"== {aimed} branch into the module re-aimed at the stock stand-in\n")
    check("the soldier's getter jumps back into fn_2ae8f0", aimed == 1)

    print("\n== T sets the picked soldiers, or the squad\n")
    men = [soldier(), soldier(marked=True, junk=True), soldier()]
    ai = squad(men, 1)
    SET(ai, 0)
    check("one of three picked: he is pinned to the value", pin_of(men[1][1]) == (1, 0x7a5f),
          str(pin_of(men[1][1])))
    check("and the behaviour byte beside it is zeroed, not left as it was",
          byte(men[1][1] + 0x1a) == 0, hex(byte(men[1][1] + 0x1a)))
    check("the others are not pinned", all(pin_of(f)[1] != 0x7a5f for _, f in (men[0], men[2])))
    check("the squad keeps its own mode", byte(ai + 0x228) == 1)
    SET(ai, 1)
    check("setting him again rewrites his pin", pin_of(men[1][1]) == (2, 0x7a5f), str(pin_of(men[1][1])))
    for _, f in men:
        put(f + 0x30, bytes([1]))
    SET(ai, 0)
    check("everybody picked: the squad is set", byte(ai + 0x228) == 0)
    check("and his pin is cleared", pin_of(men[1][1])[0] == 0, str(pin_of(men[1][1])))
    nobody = [soldier(), soldier()]
    ai2 = squad(nobody, 0)
    SET(ai2, 1)
    check("nobody picked: the squad is set", byte(ai2 + 0x228) == 1)

    print("\n== what T shows: the soldiers an order would reach\n")
    for label, specs, flag, want in (
            ("one picked, pinned to stop: stops", [dict(), dict(marked=True, pin=2), dict()], 0, 1),
            ("one picked, pinned to move: moves", [dict(), dict(marked=True, pin=1), dict()], 1, 0),
            ("one picked, unpinned: the squad's", [dict(), dict(marked=True), dict()], 1, 1),
            ("all picked, one pinned to stop: stops", [dict(marked=True, pin=2), dict(marked=True)], 0, 1),
            ("all picked, all pinned to move: moves", [dict(marked=True, pin=1), dict(marked=True, pin=1)], 1, 0),
            ("unselectable soldiers only: the squad's", [dict(enabled=False, pin=2)], 0, 0)):
        ai = squad([soldier(**s) for s in specs], flag)
        got = UI(ai) & 0xff
        check(label, got == want, f"answered {got}")

    print("\n== what a soldier answers\n")
    for label, spec, vehicle, want in (
            ("pinned to stop", dict(pin=2), None, 1),
            ("pinned to move", dict(pin=1), None, 0),
            ("pinned, but in a vehicle: the stock false", dict(pin=2), True, 0),
            ("pinned, a vehicle link but not in one", dict(pin=2), False, 1),
            ("unpinned: the stock path", dict(), None, 0x5a),
            ("a pin value without its marker: the stock path", dict(junk=True), None, 0x5a)):
        entity, _ = soldier(**spec)
        got = SOLDIER(soldier_ai(entity, vehicle)) & 0xff
        check(label, got == want, f"answered {got:#x}")
    flagged = obj(0x300)
    put(flagged + 0x228, bytes([1]))
    check("the tail reads the squad's byte, not its getter", FLAG(flagged) & 0xff == 1)

    print("\n== fn_2caeb0's read\n")
    for label, spec, flag, want in (("pinned to move, squad stops", dict(pin=1), 1, 0),
                                    ("pinned to stop, squad moves", dict(pin=2), 0, 1),
                                    ("unpinned: the squad's byte", dict(), 1, 1)):
        entity, _ = soldier(**spec)
        squad_ai = obj(0x300)
        put(squad_ai + 0x228, bytes([flag]))
        put(CALL_ARGS, struct.pack("<QQ", squad_ai, soldier_ai(entity)))
        got = CALL() & 0xff
        check(label, got == want, f"answered {got}")

    print("\n== passenger source-gun selection\n")
    special_base = 0x9000
    special_targets = {"vehicle_special_fire_original": 0x9200,
                       "vehicle_special_fire_guard_resume": 0xa000,
                       "vehicle_special_fire_guard_skip": 0xa040}
    special_code, special_labels = b.assemble(
        pathlib.Path("patch/vehicle-special-fire.asm").read_text().splitlines(),
        special_base, b.CURSOR_OFFSET, symbols=special_targets)
    put(region + special_base, special_code)
    put(region + 0x9200, asm("mov rax, qword ptr [rcx + 0x2f0]; ret"))
    PASSENGER_GUN = ctypes.CFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p)(region + special_base)
    gun_vt = obj(8)
    primary_descriptor = obj(0x110)
    primary = obj(0x48, [(0, gun_vt), (0x40, primary_descriptor)])
    special_descriptor = obj(0x110)
    special = obj(0x48, [(0, gun_vt), (0x40, special_descriptor)])
    guns = obj(0x10, [(0, primary), (8, special)])
    human = obj(0x50, [(0x38, guns), (0x40, guns + 0x10)])
    holder = obj(0x18, [(0x10, human)])
    ai = obj(0x300, [(0x1f0, holder), (0x2f0, primary)])
    for weapon_type, label in ((1, "sniper rifle"), (2, "machine gun"),
                               (3, "mounted heavy gun"), (4, "antitank missile"),
                               (5, "antiair missile"), (6, "RPG"),
                               (17, "future special weapon")):
        put(special_descriptor + 0x108, struct.pack("<I", weapon_type))
        check(f"a passenger's {label} takes the dummy mount",
              PASSENGER_GUN(ai) == special)
    put(primary_descriptor + 0x108, struct.pack("<I", 2))
    check("a machine-gun primary does not hide a later RPG",
          PASSENGER_GUN(ai) == special)
    put(special, struct.pack("<Q", obj(8)))
    check("a former gun in the source list keeps the native choice",
          PASSENGER_GUN(ai) == primary)
    put(special, struct.pack("<Q", gun_vt))
    put(special_descriptor + 0xa9, b"\x01")
    check("a grenade-marked gun keeps the native primary",
          PASSENGER_GUN(ai) == primary)
    put(special_descriptor + 0xa9, b"\x00")
    put(special_descriptor + 0x108, struct.pack("<I", 0))
    check("a usual weapon type keeps the native primary",
          PASSENGER_GUN(ai) == primary)
    put(human + 0x40, struct.pack("<Q", guns + 8))
    check("a passenger without a special gun keeps the native primary",
          PASSENGER_GUN(ai) == primary)
    put(ai + 0x2f0, struct.pack("<Q", 0))
    check("the native choice of no gun remains no gun",
          PASSENGER_GUN(ai) is None)

    print("\n== passenger dummy-gun rebinding\n")
    released = []

    @ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)
    def reset_dummy(dummy_arg, source_arg):
        released.append((dummy_arg, source_arg))

    dummy_vt = obj(0x68, [(0x60, ctypes.cast(reset_dummy, ctypes.c_void_p).value)])
    other_vt = obj(0x68)
    dummy = obj(8, [(0, dummy_vt)])
    dummy_mounts = obj(8, [(0, dummy)])
    live_source = obj(8, [(0, dummy_vt)])
    stale_source = obj(8, [(0, other_vt)])
    sources = obj(8, [(0, live_source)])
    gunner = obj(0x70, [(0x20, dummy_mounts), (0x68, sources)])
    epilogue = "pop rdi; pop rsi; pop r14; pop r13; pop rbx; ret"
    put(region + 0xa000, asm(f"mov eax, 1; {epilogue}"))
    put(region + 0xa040, asm(f"mov eax, 2; {epilogue}"))
    guard_thunk = 0xa080
    thunk_code, _ = ks.asm(
        "push rbx; push r13; push r14; push rsi; push rdi; "
        "mov rbx, rcx; mov esi, edx; mov r14d, r8d; "
        f"jmp {special_labels['vehicle_special_fire_rebind_guard']}", guard_thunk)
    put(region + guard_thunk, bytes(thunk_code))
    REBIND = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_int,
                              ctypes.c_int)(region + guard_thunk)
    check("a live source continues through the native rebind",
          REBIND(gunner, 0, 0) == 1 and q(sources) == live_source and not released)
    put(sources, struct.pack("<Q", stale_source))
    check("a former gun is released before the native rebind",
          REBIND(gunner, 0, 0) == 2 and q(sources) == 0 and released == [(dummy, None)],
          f"binding {q(sources):#x}, releases {released}")
    released.clear()
    check("an empty source slot is safe to release",
          REBIND(gunner, 0, 0) == 2 and q(sources) == 0 and released == [(dummy, None)])
    released.clear()
    put(sources, struct.pack("<Q", live_source))
    check("an unused mount skips rebinding without releasing its source",
          REBIND(gunner, -1, 0) == 2 and q(sources) == live_source and not released)

    print("\n== scarce passenger mounts prefer special-weapon carriers\n")
    events = []

    @ctypes.CFUNCTYPE(ctypes.c_ubyte, ctypes.c_void_p, ctypes.c_void_p)
    def bind_mount(gunner_arg, source_arg):
        if q(mount_sources):
            return 0
        put(mount_sources, struct.pack("<Q", source_arg))
        events.append(("bind", source_arg))
        return 1

    @ctypes.CFUNCTYPE(ctypes.c_ubyte, ctypes.c_void_p, ctypes.c_void_p)
    def unbind_mount(gunner_arg, source_arg):
        if q(mount_sources) != source_arg:
            return 0
        put(mount_sources, bytes(8))
        events.append(("unbind", source_arg))
        return 1

    @ctypes.CFUNCTYPE(None, ctypes.c_void_p)
    def refresh_mounts(ai_arg):
        events.append(("refresh", ai_arg))

    put(region + 0xa800, asm(
        f"mov rax, {ctypes.cast(refresh_mounts, ctypes.c_void_p).value}; jmp rax"))
    priority_base = 0xb000
    priority_code, priority_labels = b.assemble(
        pathlib.Path("patch/vehicle-priority-fire.asm").read_text().splitlines(),
        priority_base, b.CURSOR_OFFSET, symbols={
            "vehicle_special_fire_source": special_labels["vehicle_special_fire_source"],
            "vehicle_special_fire_board_resume": 0xa900,
            "vehicle_special_fire_disembark_resume": 0xa940,
            "vehicle_special_fire_refresh_ai": 0xa800,
        })
    put(region + priority_base, priority_code)
    REBALANCE = ctypes.CFUNCTYPE(None, ctypes.c_void_p)(
        region + priority_labels["vehicle_special_fire_rebalance"])

    regular_descriptor = obj(0x110)
    rocket_descriptor = obj(0x110)
    put(rocket_descriptor + 0x108, struct.pack("<I", 6))
    regular_gun = obj(0x48, [(0, gun_vt), (0x40, regular_descriptor)])
    rocket_gun = obj(0x48, [(0, gun_vt), (0x40, rocket_descriptor)])

    def passenger(weapons, native):
        entries = obj(len(weapons) * 8, [(i * 8, gun) for i, gun in enumerate(weapons)])
        human = obj(0x50, [(0x38, entries), (0x40, entries + 8 * len(weapons))])
        ai = obj(0x300, [(0x1f0, obj(0x18, [(0x10, human)])), (0x2f0, native)])
        facets = obj(0x60, [(0x28, ai)])
        return obj(0x10, [(0, ENTITY_VT), (8, facets)])

    regular_passenger = passenger([regular_gun], regular_gun)
    rocket_passenger = passenger([regular_gun, rocket_gun], regular_gun)
    mount_sources = obj(8, [(0, regular_gun)])
    mount_vt = obj(0x158, [(0x148, ctypes.cast(bind_mount, ctypes.c_void_p).value),
                            (0x150, ctypes.cast(unbind_mount, ctypes.c_void_p).value)])
    mounted_gunner = obj(0x78, [(0, mount_vt), (0x68, mount_sources),
                                (0x70, mount_sources + 8)])
    gunner_vector = obj(8, [(0, mounted_gunner)])
    vehicle_ai = obj(0x220, [(0x208, gunner_vector), (0x210, gunner_vector + 8)])
    vehicle_facets = obj(0x60, [(0x28, vehicle_ai)])
    vehicle_entity = obj(0x10, [(0, ENTITY_VT), (8, vehicle_facets)])
    passengers = obj(0x10, [(0, regular_passenger), (8, rocket_passenger)])
    helper = obj(0x120, [(0x20, vehicle_entity), (0x110, passengers),
                         (0x118, passengers + 0x10)])
    REBALANCE(helper)
    check("a late RPG passenger takes a mount occupied by a regular weapon",
          q(mount_sources) == rocket_gun and events == [
              ("unbind", regular_gun), ("bind", rocket_gun), ("refresh", vehicle_ai)],
          str(events))
    events.clear()
    REBALANCE(helper)
    check("a mounted special gun remains bound without further moves",
          q(mount_sources) == rocket_gun and not events, str(events))
    put(mount_sources, bytes(8))
    events.clear()
    REBALANCE(helper)
    check("a special passenger gets a free mount before an earlier regular passenger",
          q(mount_sources) == rocket_gun and events == [
              ("bind", rocket_gun), ("refresh", vehicle_ai)], str(events))
    put(passengers + 8, bytes(8))
    put(mount_sources, bytes(8))
    events.clear()
    REBALANCE(helper)
    check("a regular passenger fills a mount vacated by the special carrier",
          q(mount_sources) == regular_gun and events == [
              ("bind", regular_gun), ("refresh", vehicle_ai)], str(events))
    former_gun = obj(0x48, [(0, other_vt), (0x40, regular_descriptor)])
    put(passengers + 8, struct.pack("<Q", rocket_passenger))
    put(mount_sources, struct.pack("<Q", former_gun))
    events.clear()
    REBALANCE(helper)
    check("a former source is not displaced as a regular passenger gun",
          q(mount_sources) == former_gun and not events, str(events))

    print("\n== the behaviour census answers as the getter did, and counts its callers\n")
    census_code, census_labels = b.assemble(
        pathlib.Path("patch/behaviour-census.asm").read_text().splitlines(),
        b.CENSUS_OFFSET, b.CENSUS_TABLE)
    put(block + b.CENSUS_OFFSET, census_code)
    TABLE = block + b.CENSUS_TABLE
    asker = scratch(0x100)
    put(asker, asm(f"sub rsp, 0x28; mov rax, {block + census_labels['census_behaviour']}; "
                   "call rax; add rsp, 0x28; ret"))
    ASK = ctypes.CFUNCTYPE(ctypes.c_uint32, ctypes.c_void_p)(asker)
    ret_at = asker + asm(f"sub rsp, 0x28; mov rax, {block + census_labels['census_behaviour']}; "
                         "call rax").__len__()
    squad_ai = obj(0x300)
    put(squad_ai + 0x200, struct.pack("<I", 3))
    answers = [ASK(squad_ai) for _ in range(5)]
    check("it answers with the squad's behaviour", answers == [3] * 5, str(answers))
    check("one caller, counted five times", (q(TABLE), q(TABLE + 8), q(TABLE + 16)) == (ret_at, 5, 0),
          f"{q(TABLE):#x} {q(TABLE + 8)} {q(TABLE + 16):#x}")

    print()
    print(f"{failures} failed" if failures else "all cases as expected")
    return 1 if failures else 0

if __name__ == "__main__":
    sys.exit(main())
