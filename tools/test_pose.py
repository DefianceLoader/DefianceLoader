"""Run the pose split natively, entered through its real entry points.

Each entry finds the stock lie-down or stand-up from its return address, at a
fixed distance from the handler's call. So the test calls each entry from a
trampoline placed so that its return address sits at exactly that distance
above a stand-in, which records the entity it is handed. That exercises the
arithmetic as well as the rule: marks decide only while they discriminate.
Fabricated squads and soldiers stand in for the engine's, with the facet
layout the stub reads; the ring is checked for the call and every soldier.
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


def blob(data):
    addr = scratch(len(data))
    put(addr, data)
    return addr


def obj(size, fields):
    addr = scratch(size)
    for offset, value in fields:
        put(addr + offset, struct.pack("<Q", value))
    return addr


# One region holds everything, so the thunks' rel32 calls reach the block:
# stand-ins for the stock functions low down, the block near the top, and each
# way in placed so that its return point lands the right distance above its
# stand-in.
region = scratch(0x240000)
code, labels = b.assemble(pathlib.Path("patch/pose.asm").read_text().splitlines(),
                          b.POSE_OFFSET, b.TRACE_OFFSET)
block = region + 0x230000
put(block + b.POSE_OFFSET, code)
RING = block + b.TRACE_OFFSET

LOG = {"down": scratch(0x100), "up": scratch(0x100)}
stock = {}
for name, at in (("down", region + 0x1000), ("up", region + 0x2000)):
    put(at, asm(f"mov r11, {LOG[name]}; mov rax, qword ptr [r11]; "
                "mov qword ptr [r11 + rax * 8 + 8], rcx; inc qword ptr [r11]; ret"))
    stock[name] = at

ARG, CAPTURE = scratch(8), scratch(0x50)
SENTINELS = [0x1010101010101010 * (n + 1) for n in range(8)]
REGS = ["rbx", "rbp", "rsi", "rdi", "r12", "r13", "r14", "r15"]


def trampoline(target, arg):
    """Sentinels in every nonvolatile, the entity in `arg`, a real call to
    `target`, then the registers as they came back. Returns the code and the
    offset of its return point."""
    text = "; ".join(
        [f"push {r}" for r in REGS] + ["sub rsp, 0x28"]
        + [f"mov {r}, {v:#x}" for r, v in zip(REGS, SENTINELS)]
        + [f"mov {arg}, {ARG}", f"mov {arg}, qword ptr [{arg}]",
           f"mov r11, {target}", "call r11", f"mov r11, {CAPTURE}"]
        + [f"mov qword ptr [r11 + {i * 8}], {r}" for i, r in enumerate(REGS)]
        + ["add rsp, 0x28"] + [f"pop {r}" for r in reversed(REGS)] + ["ret"])
    body = asm(text)
    return body, body.index(bytes.fromhex("41ffd3")) + 3      # just past call r11


RUN, TAG = {}, {}
# The handler's calls: the trampoline is the handler, so its return point is
# what has to land at the distance above the stand-in.
for name, entry, ret_rva, stock_rva, bit in (("down", "pose_down", 0x31eebe, 0x1054c0, 60),
                                             ("up", "pose_up", 0x31eec5, 0x105620, 61)):
    body, ret_at = trampoline(block + labels[entry], "rcx")
    ret = stock[name] - (stock_rva - ret_rva)
    put(ret - ret_at, body)
    RUN[name] = ctypes.CFUNCTYPE(None)(ret - ret_at)
    TAG[name] = ret | 1 << bit
# The AiUtils thunks exactly as patched, mov rcx, rdx; call entry; ret, with
# their return point 8 past their start. The trampoline is the panel: it calls
# the thunk with the entity in rdx, and it is the caller the ring names.
for name, entry, ret_rva, stock_rva, bit in (
        ("down direct", "pose_down_direct", 0x10f718, 0x1054c0, 60),
        ("up direct", "pose_up_direct", 0x10f728, 0x105620, 61)):
    thunk = stock[name.split()[0]] - (stock_rva - ret_rva) - 8
    put(thunk, bytes.fromhex("488bca") + bytes.fromhex("e8")
        + struct.pack("<i", block + labels[entry] - (thunk + 8)) + bytes.fromhex("c3"))
    body, ret_at = trampoline(thunk, "rdx")
    at = blob(body)
    RUN[name] = ctypes.CFUNCTYPE(None)(at)
    TAG[name] = at + ret_at | 1 << bit

# fabricated entities: vt+0x98 tests the kind mask at +0x20, vt+0xb0 returns
# the facets at +0x8; facets +0x28 lead to the member vector, +0x50 the facet
KIND = blob(asm("test dword ptr [rcx + 0x20], edx; setnz al; ret"))
FACETS = blob(asm("mov rax, qword ptr [rcx + 8]; ret"))
FIELD8 = blob(asm("mov rax, qword ptr [rcx + 8]; ret"))
GETTER = blob(asm("movzx eax, byte ptr [rcx + 0x30]; and al, byte ptr [rcx + 0x18]; ret"))
ENTITY_VT = obj(0xb8, [(0x98, KIND), (0xb0, FACETS)])
AI_VT = obj(0x3c0, [(0x3b8, FIELD8)])
ROSTER_VT = obj(0x70, [(0x68, FIELD8)])
FACET_VT = obj(0x60, [(0x58, GETTER)])


def soldier(marked=False, enabled=True):
    facet = obj(0x40, [(0, FACET_VT), (0x18, int(enabled)), (0x30, int(marked))])
    facets = obj(0x60, [(0x50, facet)])
    return obj(0x28, [(0, ENTITY_VT), (8, facets), (0x20, 0x20)])


def facet_of(entity):
    return q(q(entity + 8) + 0x50)


def squad(members, listable=True):
    """A squad with its own selectable facet, which each member's facet names
    at +0x28, as a soldier's does."""
    pointers = obj(max(8, 8 * len(members)), [(i * 8, m) for i, m in enumerate(members)])
    vector = obj(0x10, [(0, pointers), (8, pointers + 8 * len(members))])
    roster = obj(0x10, [(0, ROSTER_VT), (8, vector)])
    ai = obj(0x10, [(0, AI_VT), (8, roster)])
    own = obj(0x40, [(0, FACET_VT), (0x18, 1), (0x30, 1)])
    for m in members:
        put(facet_of(m) + 0x28, struct.pack("<Q", own))
    facets = obj(0x60, [(0x28, ai if listable else 0), (0x50, own)])
    return obj(0x28, [(0, ENTITY_VT), (8, facets), (0x20, 0x10)])


def pin(entity):
    """His pin as (value, marker)."""
    f = facet_of(entity)
    return (ctypes.c_ubyte.from_address(f + 0x31).value,
            ctypes.c_uint16.from_address(f + 0x32).value)


failures = 0


def check(label, ok, detail=""):
    global failures
    if not ok:
        failures += 1
    print(f"{'PASS' if ok else 'FAIL'}  {label}{'' if ok else '  ' + detail}")


def run(which, entity):
    log = LOG[which.split()[0]]
    put(ARG, struct.pack("<Q", entity))
    put(log, struct.pack("<Q", 0))
    start = q(RING)
    RUN[which]()
    got = [q(log + 8 + i * 8) for i in range(q(log))]
    ring = [(q(RING + 0x10 + (i % 32) * 16), q(RING + 0x18 + (i % 32) * 16))
            for i in range(start, q(RING))]
    return got, ring


def names(values, table):
    return [table.get(v, hex(v)) for v in values]


print("== the rule\n")
men = [soldier(), soldier(marked=True), soldier()]
sq = squad(men)
table = {sq: "squad", 0: "NULL", **{m: f"soldier {i}" for i, m in enumerate(men)}}
got, ring = run("down", sq)
check("one of three marked: only he lies down", got == [men[1]], str(names(got, table)))
check("the ring has the call and the soldier",
      [s for _, s in ring] == [sq, men[1]], str(names([s for _, s in ring], table)))
check("both carry the handler's return point and bit 60",
      all(t == TAG["down"] for t, _ in ring), str([hex(t) for t, _ in ring]))

men2 = [soldier(marked=True), soldier(), soldier(marked=True)]
sq2 = squad(men2)
table.update({sq2: "squad 2", **{m: f"soldier 2.{i}" for i, m in enumerate(men2)}})
got, _ = run("down", sq2)
check("two of three: both, in squad order", got == [men2[0], men2[2]], str(names(got, table)))

for label, members in (("everybody marked", [soldier(marked=True) for _ in range(3)]),
                       ("nobody marked", [soldier() for _ in range(3)]),
                       ("the only unmarked one is not selectable",
                        [soldier(marked=True), soldier(enabled=False), soldier(marked=True)])):
    s = squad(members)
    got, _ = run("down", s)
    check(f"{label}: the squad as before", got == [s], str(names(got, {s: "squad"})))

print("\n== pins: set on the soldiers ordered alone, cleared by a whole-squad order\n")
check("the one lying down is pinned prone", pin(men[1]) == (3, 0x7a5e), str(pin(men[1])))
check("the others are not pinned", pin(men[0])[1] != 0x7a5e and pin(men[2])[1] != 0x7a5e)
for m in men:
    put(facet_of(m) + 0x30, bytes([1]))                  # now everybody is marked
got, _ = run("down", sq)
check("everybody marked: the squad is ordered", got == [sq], str(names(got, table)))
check("and his pin is gone", pin(men[1])[1] != 0x7a5e, str(pin(men[1])))
junk = soldier(marked=True)
put(facet_of(junk) + 0x31, bytes([3, 0x11, 0x22]))       # heap leftovers, no marker
junk_squad = squad([junk, soldier(marked=True)])
run("down", junk_squad)
check("bytes without the marker are left alone", pin(junk) == (3, 0x2211), str(pin(junk)))
for m in men:
    put(facet_of(m) + 0x30, bytes([0]))
put(facet_of(men[1]) + 0x30, bytes([1]))                 # back to one marked

print("\n== stand up takes the same path to its own stock function\n")
men3 = [soldier(marked=True), soldier()]
sq3 = squad(men3)
got, ring = run("up", sq3)
check("one of two marked: only he stands", got == [men3[0]], str(names(got, {sq3: "squad"})))
check("and he is pinned standing", pin(men3[0]) == (1, 0x7a5e), str(pin(men3[0])))
check("recorded with bit 61", all(t == TAG["up"] for t, _ in ring))

print("\n== what the stock function handles itself passes straight through\n")
lone = soldier(marked=True)
got, _ = run("down", lone)
check("a lone soldier sent by the panel", got == [lone])
got, _ = run("down", 0)
check("no entity", got == [0])
unlisted = squad([soldier(marked=True), soldier()], listable=False)
got, _ = run("down", unlisted)
check("a squad whose members cannot be listed", got == [unlisted])

print("\n== the panel's direct path, through the patched AiUtils thunks\n")
got, ring = run("down direct", sq)
check("one of three marked: only he lies down", got == [men[1]], str(names(got, table)))
check("the ring names the thunk's caller, with bit 60",
      [t for t, _ in ring] == [TAG["down direct"]] * 2, str([hex(t) for t, _ in ring]))
everyone = squad([soldier(marked=True) for _ in range(2)])
got, _ = run("down direct", everyone)
check("everybody marked: the squad as before", got == [everyone])
got, ring = run("up direct", sq3)
check("stand up: only the marked one", got == [men3[0]])
check("recorded with bit 61", all(t == TAG["up direct"] for t, _ in ring))

print("\n== the caller's registers survive both ways in\n")
for way in ("down", "down direct"):
    run(way, sq)
    check(f"{way}: rbx, rbp, rsi, rdi and r12-r15",
          [q(CAPTURE + i * 8) for i in range(8)] == SENTINELS,
          str([hex(q(CAPTURE + i * 8)) for i in range(8)]))

print("\n== what V offers: the prone query answers for the picked soldiers\n")
# The query sits where payload.py puts it, in the same block. Its stock answer
# runs the original function, which here is a stand-in finishing that
# function's frame and answering with a squad flag the case sets.
prone_code, prone_labels = b.assemble(
    pathlib.Path("patch/prone-query.asm").read_text().splitlines(),
    b.PRONE_OFFSET, b.CURSOR_OFFSET)
put(block + b.PRONE_OFFSET, prone_code)
FLAG = scratch(8)
stock_answer = blob(asm(f"mov r11, {FLAG}; movzx eax, byte ptr [r11]; "
                        "add rsp, 0x20; pop rbx; ret"))
for offset, target in b.module_branches(prone_code, b.PRONE_OFFSET):
    assert target == 0x110bb6, hex(target)
    put(block + offset, struct.pack("<i", stock_answer - (block + offset + 4)))
ANSWER = scratch(8)
asker = blob(asm(f"push rbx; sub rsp, 0x20; xor ecx, ecx; mov rdx, {ARG}; "
                 f"mov rdx, qword ptr [rdx]; mov rbx, {block + prone_labels['prone_query']}; "
                 f"call rbx; mov rbx, {ANSWER}; mov byte ptr [rbx], al; add rsp, 0x20; "
                 "pop rbx; ret"))
ASK = ctypes.CFUNCTYPE(None)(asker)


def offered(entity, squad_prone):
    put(FLAG, bytes([squad_prone]))
    put(ARG, struct.pack("<Q", entity))
    ASK()
    return ctypes.c_ubyte.from_address(ANSWER).value


def pinned_squad(marks, pins):
    members = [soldier(marked=m) for m in marks]
    for m, value in zip(members, pins):
        if value is not None:
            put(facet_of(m) + 0x31, bytes([value]) + struct.pack("<H", 0x7a5e))
    return squad(members)


for label, marks, pins, squad_prone, want in (
        ("one picked, pinned prone, squad standing: prone", [0, 1, 0], [None, 3, None], 0, 1),
        ("one picked, unpinned, squad standing: upright", [0, 1, 0], [None] * 3, 0, 0),
        ("one picked, unpinned, squad prone: prone", [0, 1, 0], [None] * 3, 1, 1),
        ("one picked, pinned standing, squad prone: upright", [0, 1, 0], [None, 1, None], 1, 0),
        ("two picked, one of them upright: upright", [1, 1, 0], [3, None, None], 0, 0),
        ("everybody picked, all laid down one by one: prone", [1, 1], [3, 3], 0, 1),
        ("everybody picked, one laid down alone: upright", [1, 1], [3, None], 0, 0),
        ("everybody picked, unpinned, squad standing: upright", [1, 1], [None, None], 0, 0),
        ("everybody picked, unpinned, squad prone: prone", [1, 1], [None, None], 1, 1),
        ("nobody picked, all laid down one by one: prone", [0, 0], [3, 3], 0, 1)):
    got = offered(pinned_squad(marks, pins), squad_prone)
    check(label, got == want, f"answered {got}")
lone = soldier(marked=True)
put(facet_of(lone) + 0x31, bytes([3]) + struct.pack("<H", 0x7a5e))
check("a lone soldier asked about: the stock answer", offered(lone, 0) == 0)
check("no entity: the stock answer", offered(0, 1) == 1)

print()
print(f"{failures} failed" if failures else "all cases as expected")
sys.exit(1 if failures else 0)
