"""Run the move filter on fabricated member vectors.

The filter is a detour, not a function: it reads the vector out of its
caller's frame at [rsp+0x30] and [rsp+0x38], writes the new end back, leaves
the new end in rcx for the comparison that follows the detour, and performs
the displaced `mov rdi, [rsp+0x30]` on the way out. So the test enters it the
way the engine does, through a trampoline that lays out that exact frame and
reports all three results.
"""
import ctypes, json, pathlib, sys
from ctypes import wintypes
import keystone
sys.path.insert(0, "tools")
from pe import Image

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


def blob(data, size=None):
    addr = scratch(size or len(data))
    ctypes.memmove(addr, data, len(data))
    return addr


def words(values):
    return blob(b"".join(int(v).to_bytes(8, "little") for v in values))


def poke(addr, offset, value, width=8):
    ctypes.memmove(addr + offset, int(value).to_bytes(width, "little"), width)


def peek(addr, offset=0, width=8):
    return int.from_bytes((ctypes.c_char * width).from_address(addr + offset).raw, "little")


STUB_FACETS = blob(asm("mov rax, qword ptr [rcx + 0x110]; ret"))
STUB_SELECTED = blob(asm("movzx eax, byte ptr [rcx + 0x10]; ret"))


def entity(marked=None):
    """An entity with a selectable facet reporting `marked`, or none at all."""
    vt = [0] * 32
    vt[0xb0 // 8] = STUB_FACETS
    obj = blob(bytes(bytearray(0x120)), 0x120)
    poke(obj, 0, words(vt))
    if marked is not None:
        selectable_vt = [0] * 16
        selectable_vt[0x58 // 8] = STUB_SELECTED
        selectable = blob(bytes(bytearray(0x20)), 0x20)
        poke(selectable, 0, words(selectable_vt))
        poke(selectable, 0x10, 1 if marked else 0, 1)
        facets = blob(bytes(bytearray(0x60)), 0x60)
        poke(facets, 0x50, selectable)
        poke(obj, 0x110, facets)
    return obj


built = json.loads(pathlib.Path("out/manifest.json").read_text())
img = Image("out/logic.dll")
filter_code = blob(img.read(built["move_rva"], built["move_bytes"]))

# The order the filter asks "is this a run?" (vt+0x70 with flag 0x200); it
# arrives in rsi, as it does in fn_439f20. A walk unless a case says otherwise.
STUB_FLAG = blob(asm("test dword ptr [rcx + 8], edx; setnz al; ret"))
ORDER_VT = words([0] * 14 + [STUB_FLAG] + [0])


def order(run=False):
    o = blob(bytes(bytearray(0x10)), 0x10)
    poke(o, 0, ORDER_VT)
    poke(o, 8, 0x200 if run else 0, 4)
    return o


ORDER = scratch(8)
poke(ORDER, 0, order())
# The squad's AI, which fn_439f20 keeps in r13; its +0x29e is the prone flag.
SQUAD_AI = scratch(0x300)

# The trampoline stands in for fn_439f20 at the detour boundary: the vector in
# the frame, its end also in rcx, the order in rsi, the squad's AI in r13,
# then the call.
TRAMPOLINE = blob(asm(f"""
    push rbx
    push rdi
    push rsi
    push r13
    sub rsp, 0x68
    mov rsi, {ORDER}
    mov rsi, qword ptr [rsi]
    mov r13, {SQUAD_AI}
    mov rbx, r9
    mov qword ptr [rsp + 0x30], rcx
    mov qword ptr [rsp + 0x38], rdx
    mov qword ptr [rsp + 0x40], r8
    mov rcx, rdx
    mov rax, {filter_code}
    call rax
    mov qword ptr [rbx], rcx
    mov qword ptr [rbx + 8], rdi
    mov rax, qword ptr [rsp + 0x38]
    mov qword ptr [rbx + 0x10], rax
    mov rax, qword ptr [rsp + 0x40]
    mov qword ptr [rbx + 0x18], rax
    add rsp, 0x68
    pop r13
    pop rsi
    pop rdi
    pop rbx
    ret
"""))
PROTO = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p,
                         ctypes.c_void_p, ctypes.c_void_p)
run_filter = PROTO(TRAMPOLINE)

failures = 0


def case(label, marks, expect):
    """`marks` is one entry per member: True, False or None for no facet.
    `expect` lists the indices that should survive, or None for untouched."""
    global failures
    members = [entity(m) for m in marks]
    vector = words(members) if members else scratch(8)
    begin = vector
    end = begin + 8 * len(members)
    capacity = end + 0x40                 # deliberately past the end
    out = scratch(0x20)
    run_filter(begin, end, capacity, out)

    new_end, rdi, stored_end, stored_capacity = (peek(out, i * 8) for i in range(4))
    kept = [(peek(begin, i * 8)) for i in range((new_end - begin) // 8)]
    want = members if expect is None else [members[i] for i in expect]

    problems = []
    if kept != want:
        names = {m: f"member{i}" for i, m in enumerate(members)}
        problems.append(f"kept {[names.get(k, hex(k)) for k in kept]}, wanted "
                        f"{[names.get(w) for w in want]}")
    if stored_end != new_end:
        problems.append(f"the stored end {stored_end:#x} disagrees with rcx {new_end:#x}")
    if rdi != begin:
        problems.append(f"rdi is {rdi:#x}, not the vector's begin {begin:#x}")
    if stored_capacity != capacity:
        problems.append("the capacity was changed, which would break the cleanup")
    if problems:
        failures += 1
    print(f"{'PASS' if not problems else 'FAIL'}  {label}")
    for problem in problems:
        print(f"        {problem}")


print("== the marks decide only while they discriminate\n")
case("nobody marked leaves the whole squad moving", [False, False, False], None)
case("everybody marked is no preference either", [True, True, True], None)
case("one of three marked moves only him", [False, True, False], [1])
case("two of three marked moves those two", [True, False, True], [0, 2])
case("the first of two marked moves only him", [True, False], [0])

print("\n== shapes that must not misbehave\n")
case("a single member, unmarked, is untouched", [False], None)
case("a single member, marked, is no preference", [True], None)
case("an empty vector is untouched", [], None)
case("members with no selectable facet count as unmarked",
     [None, True, None], [1])
case("a squad where only facet-less members exist is untouched",
     [None, None], None)

print("\n== posture pins: a run clears them on the soldiers that move, a walk keeps them\n")


def facet(member):
    return peek(peek(member, 0x110), 0x50)


def pinned(member):
    f = facet(member)
    return peek(f, 0x31, 1), peek(f, 0x32, 2)


def pin_case(label, run, marks, pins, expect_pins, squad_prone=False):
    """`pins` and `expect_pins` give each member's (pin, marker) before and after."""
    global failures
    members = [entity(m) for m in marks]
    for m, (value, marker) in zip(members, pins):
        poke(facet(m), 0x31, value, 1)
        poke(facet(m), 0x32, marker, 2)
    poke(ORDER, 0, order(run))
    poke(SQUAD_AI, 0x29e, int(squad_prone), 1)
    begin = words(members)
    end = begin + 8 * len(members)
    run_filter(begin, end, end + 0x40, scratch(0x20))
    poke(ORDER, 0, order())
    got = [pinned(m) for m in members]
    ok = got == expect_pins
    if not ok:
        failures += 1
    print(f"{'PASS' if ok else 'FAIL'}  {label}")
    if not ok:
        print(f"        got {got}, wanted {expect_pins}")


PRONE, NONE = (3, 0x7a5e), (0, 0)
pin_case("a walk keeps the pin of the soldier who moves", False,
         [True, False, False], [PRONE, PRONE, NONE], [PRONE, PRONE, NONE])
pin_case("a run clears it, and only on the soldier who moves", True,
         [True, False, False], [PRONE, PRONE, NONE], [NONE, PRONE, NONE])
pin_case("a whole-squad run clears every soldier's pin", True,
         [False, False], [PRONE, PRONE], [NONE, NONE])
pin_case("bytes without the marker are left alone", True,
         [False, False], [(3, 0x2211), NONE], [(3, 0x2211), NONE])

print("\n== one soldier running out of a prone squad leaves the rest down\n")
STAND = (1, 0x7a5e)
pin_case("the soldiers left behind are pinned prone", True,
         [True, False, False], [NONE, NONE, NONE], [NONE, PRONE, PRONE], squad_prone=True)
pin_case("and the runner's own prone pin goes", True,
         [True, False], [PRONE, NONE], [NONE, PRONE], squad_prone=True)
pin_case("a soldier left behind keeps a pin of his own", True,
         [True, False], [NONE, STAND], [NONE, STAND], squad_prone=True)
pin_case("from a standing squad, nobody is pinned", True,
         [True, False], [NONE, NONE], [NONE, NONE], squad_prone=False)
pin_case("a walk from a prone squad pins nobody", False,
         [True, False], [NONE, NONE], [NONE, NONE], squad_prone=True)
pin_case("the whole squad running pins nobody", True,
         [False, False], [NONE, NONE], [NONE, NONE], squad_prone=True)

print()
print(f"{failures} failed" if failures else "all cases as expected")
sys.exit(1 if failures else 0)
