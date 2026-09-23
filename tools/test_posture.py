"""Run the posture gates natively, entered as their functions would be.

Each gate stands in for the first two instructions of fn_2aff10 (lay down) or
fn_2b0200 (stand up) and then jumps back into the function. Here that jump is
re-aimed, exactly as the injector re-aims it, at a stand-in for the rest of
the function: it records the soldier it was given and unwinds the frame the
gate built for it. So a refusal is a gate that returns before the stand-in
runs, and a pass is the stand-in running with the caller's rcx and the
function's own frame. Soldiers are fabricated with the facet layout the gate
reads: AI object +0x10 -> holder +0x10 -> entity, vt+0xb0 -> facets, +0x50 ->
the selectable facet, whose +0x31 is the pin and +0x32 its marker.
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


def scratch(size):
    addr = kernel32.VirtualAlloc(None, max(size, 8), 0x3000, 0x40)
    if not addr:
        raise OSError("VirtualAlloc failed")
    return addr


def put(addr, data):
    ctypes.memmove(addr, data, len(data))


def q(addr):
    return ctypes.c_uint64.from_address(addr).value


def obj(size, fields):
    addr = scratch(size)
    for offset, value in fields:
        put(addr + offset, struct.pack("<Q", value))
    return addr


# the gates, where payload.py puts them, and the stand-ins in the same region
region = scratch(0x10000)
block = region
code, labels = b.assemble(pathlib.Path("patch/posture-gate.asm").read_text().splitlines(),
                          b.POSTURE_OFFSET, b.CURSOR_OFFSET)
put(block + b.POSTURE_OFFSET, code)

LOG = {"up": region + 0x8000, "down": region + 0x8100}
FRAME = {"up": 0x60, "down": 0x70}
RESUME = {0x2b0206: "up", 0x2aff16: "down"}
stand_in = {}
for name, at in (("up", region + 0x9000), ("down", region + 0x9100)):
    put(at, asm(f"mov r11, {LOG[name]}; mov rax, qword ptr [r11]; "
                f"mov qword ptr [r11 + rax * 8 + 8], rcx; inc qword ptr [r11]; "
                f"mov qword ptr [r11 + 0x80], rbx; add rsp, {FRAME[name]:#x}; pop rbx; ret"))
    stand_in[name] = at
# re-aim each branch into the module, as the injector does
aimed = 0
for offset, target in b.module_branches(code, b.POSTURE_OFFSET):
    field = block + offset
    put(field, struct.pack("<i", stand_in[RESUME[target]] - (field + 4)))
    aimed += 1

ARG, CAPTURE = scratch(8), scratch(0x50)
SENTINELS = [0x2020202020202020 * (n + 1) & 0xffffffffffffffff for n in range(8)]
REGS = ["rbx", "rbp", "rsi", "rdi", "r12", "r13", "r14", "r15"]


def trampoline(entry):
    text = "; ".join(
        [f"push {r}" for r in REGS] + ["sub rsp, 0x28"]
        + [f"mov {r}, {v:#x}" for r, v in zip(REGS, SENTINELS)]
        + [f"mov rcx, {ARG}", "mov rcx, qword ptr [rcx]",
           f"mov r11, {block + labels[entry]}", "call r11", f"mov r11, {CAPTURE}"]
        + [f"mov qword ptr [r11 + {i * 8}], {r}" for i, r in enumerate(REGS)]
        + ["add rsp, 0x28"] + [f"pop {r}" for r in reversed(REGS)] + ["ret"])
    at = scratch(0x200)
    put(at, asm(text))
    return ctypes.CFUNCTYPE(None)(at)


RUN = {"up": trampoline("stand_up_gate"), "down": trampoline("lie_down_gate")}
GET_FACETS = scratch(0x10)
put(GET_FACETS, asm("mov rax, qword ptr [rcx + 8]; ret"))
ENTITY_VT = obj(0xb8, [(0xb0, GET_FACETS)])


def soldier(pin=None, marker=0x7a5e, links=True, facet=True):
    """The soldier's AI object, as the two functions are handed it."""
    selectable = obj(0x38, [])
    if pin is not None:
        put(selectable + 0x31, bytes([pin]) + struct.pack("<H", marker))
    facets = obj(0x60, [(0x50, selectable if facet else 0)])
    entity = obj(0x28, [(0, ENTITY_VT), (8, facets)])
    holder = obj(0x18, [(0x10, entity)])
    return obj(0x20, [(0x10, holder if links else 0)])


failures = 0


def check(label, ok, detail=""):
    global failures
    if not ok:
        failures += 1
    print(f"{'PASS' if ok else 'FAIL'}  {label}{'' if ok else '  ' + detail}")


def attempt(which, ai):
    put(ARG, struct.pack("<Q", ai))
    put(LOG[which], struct.pack("<Q", 0))
    RUN[which]()
    return [q(LOG[which] + 8 + i * 8) for i in range(q(LOG[which]))]


print(f"== {aimed} branches into the module re-aimed at the stand-ins\n")
check("both gates jump back into their functions", aimed == 2)

print("\n== the gates\n")
for label, ai, up, down in (
        ("no pin: both pass", soldier(), True, True),
        ("pinned prone: standing up is refused", soldier(pin=3), False, True),
        ("pinned standing: lying down is refused", soldier(pin=1), True, False),
        ("a pin value without its marker is no pin", soldier(pin=3, marker=0x2211), True, True),
        ("no holder: no pin", soldier(links=False), True, True),
        ("no selectable facet: no pin", soldier(facet=False), True, True)):
    got_up, got_down = attempt("up", ai), attempt("down", ai)
    check(label, (got_up == [ai]) == up and (got_down == [ai]) == down,
          f"up {got_up}, down {got_down}")

print("\n== what the function continues with\n")
ai = soldier()
attempt("up", ai)
check("the stand-in saw the caller's rcx", q(LOG["up"] + 8) == ai)
check("rbx is the caller's when the function resumes, and its frame unwinds",
      q(LOG["up"] + 0x80) == SENTINELS[0], hex(q(LOG["up"] + 0x80)))
check("the caller's registers survive a pass",
      [q(CAPTURE + i * 8) for i in range(8)] == SENTINELS)
attempt("up", soldier(pin=3))
check("and a refusal", [q(CAPTURE + i * 8) for i in range(8)] == SENTINELS)

print("\n== the movement states read the soldier's own posture\n")
# Called as the two sites call it, over movzx ebp, byte [rcx+0x29e]: rcx his
# squad's AI, rsi the movement state, whose +0x10 holds an object answering
# vt+0x50 with his entity. Only ebp may change.
GET_ENTITY = scratch(0x10)
put(GET_ENTITY, asm("mov rax, qword ptr [rcx + 8]; ret"))
OWNER_VT = obj(0x58, [(0x50, GET_ENTITY)])
SQUAD_AI, STATE = scratch(0x300), scratch(0x20)
VOLATILE = ["rax", "rcx", "rdx", "r8", "r9", "r10", "r11"]
MARKS = [0x3030303030303030 + n for n in range(7)]
site = scratch(0x200)
put(site, asm("; ".join(
    ["push rbx", "push rbp", "push rsi", "sub rsp, 0x20",
     f"mov rsi, {STATE}", "mov ebp, 0x55"]
    + [f"mov {r}, {v:#x}" for r, v in zip(VOLATILE, MARKS)]
    + [f"mov rcx, {SQUAD_AI}", f"mov rbx, {block + labels['move_posture']}", "call rbx",
       f"mov rbx, {CAPTURE}", "mov qword ptr [rbx + 0x40], rbp"]
    + [f"mov qword ptr [rbx + {i * 8}], {r}" for i, r in enumerate(VOLATILE)]
    + ["add rsp, 0x20", "pop rsi", "pop rbp", "pop rbx", "ret"])))
SITE = ctypes.CFUNCTYPE(None)(site)


def moving(squad_prone, pin=None, marker=0x7a5e, linked=True):
    selectable = obj(0x38, [])
    if pin is not None:
        put(selectable + 0x31, bytes([pin]) + struct.pack("<H", marker))
    facets = obj(0x60, [(0x50, selectable)])
    entity = obj(0x28, [(0, ENTITY_VT), (8, facets)])
    owner = obj(0x18, [(0, OWNER_VT), (8, entity)])
    holder = obj(0x18, [(0x10, owner)])
    put(STATE + 0x10, struct.pack("<Q", holder if linked else 0))
    put(SQUAD_AI + 0x29e, bytes([squad_prone]))
    SITE()
    return q(CAPTURE + 0x40) & 0xffffffff


for label, args, want in (
        ("unpinned in a prone squad: crawl", (1,), 1),
        ("unpinned in a standing squad: walk", (0,), 0),
        ("pinned prone in a standing squad: crawl", (0, 3), 1),
        ("pinned standing in a prone squad: walk", (1, 1), 0),
        ("a pin without its marker: the squad's", (0, 3, 0x2211), 0),
        ("no soldier to be found: the squad's", (1, None, 0x7a5e, False), 1)):
    got = moving(*args)
    check(label, got == want, f"ebp {got}")
check("only ebp changes", [q(CAPTURE + i * 8) for i in range(7)] == MARKS[:1] + [SQUAD_AI] + MARKS[2:],
      str([hex(q(CAPTURE + i * 8)) for i in range(7)]))

print()
print(f"{failures} failed" if failures else "all cases as expected")
sys.exit(1 if failures else 0)
