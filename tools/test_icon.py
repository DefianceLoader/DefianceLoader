"""Run the icon squad-expansion patch on fabricated entities.

The stubs end in a jump back into game.dll through an imm64 the injector
fills. Here those fixups are pointed at stubs of the test's own making: the
two resume targets at a `ret`, and the row resolver at one that hands back a
planted entity. That exercises squad_of, the Ctrl branch of the single click,
and the whole double-click path without game.dll being involved.
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


def blob(data, size=None):
    addr = scratch(size or len(data))
    ctypes.memmove(addr, data, len(data))
    return addr


def words(values):
    return blob(b"".join(int(v).to_bytes(8, "little") for v in values))


def poke(addr, offset, value, width=8):
    ctypes.memmove(addr + offset, int(value).to_bytes(width, "little"), width)


STUB_FACETS = blob(asm("mov rax, qword ptr [rcx + 0x110]; ret"))


def entity(facets=None):
    vt = [0] * 32
    vt[0xb0 // 8] = STUB_FACETS
    vt[0x98 // 8] = blob(asm("xor eax, eax; ret"))
    obj = blob(bytes(bytearray(0x120)), 0x120)
    poke(obj, 0, words(vt))
    if facets is not None:
        poke(obj, 0x110, facets)
    return obj


def member_of(squad_entity, *, selectable=True, parent=True, holder=True):
    """A member entity whose facet chain leads to `squad_entity`, with any hop
    optionally missing so the fallback can be checked."""
    facets = blob(bytes(bytearray(0x60)), 0x60)
    if selectable:
        own = blob(bytes(bytearray(0x40)), 0x40)
        if parent:
            squad_selectable = blob(bytes(bytearray(0x40)), 0x40)
            if holder:
                held = blob(bytes(bytearray(0x20)), 0x20)
                poke(held, 0x10, squad_entity)
                poke(squad_selectable, 0x10, held)
            poke(own, 0x28, squad_selectable)
        poke(facets, 0x50, own)
    return entity(facets)


# the trace base has to match what tools/icon.py built, or squad_of's
# rip-relative stores land outside the block
import json
BUILT = json.loads(pathlib.Path("out/payload-game.json").read_text())
TRACE = BUILT["trace_offset"]
code, labels = b.assemble(
    pathlib.Path("patch/icon-squad.asm").read_text().splitlines(), 0, TRACE,
    symbols=b.GAME_SYMBOLS)
block = scratch(BUILT["block_bytes"])
ctypes.memmove(block, code, len(code))

poke(block, code.index(struct.pack("<Q", 0xaaaaaaaaaaaaaaad)), blob(asm("xor eax, eax; ret")))

# the fixups: both resume targets just return, the resolver hands back a planted
# entity so the double-click path can be driven
# The single click jumps back with the frame already unwound, so a bare ret
# stands in. The double click jumps back into the middle of its own frame, so
# its stand-in has to carry fn_1f4200's epilogue.
RESUME_SINGLE = blob(asm("ret"))
RESUME_DOUBLE = blob(asm("add rsp, 0x40; pop rdi; ret"))
RESOLVED = scratch(8)
RESOLVER = blob(asm(f"mov rax, {RESOLVED}; mov rax, qword ptr [rax]; ret"))
for n, target in ((1, RESUME_SINGLE), (2, RESOLVER), (3, RESUME_DOUBLE)):
    at = code.find(struct.pack("<Q", 0xaaaaaaaaaaaaaaa0 + n))
    assert at >= 0, f"fixup {n} not found"
    ctypes.memmove(block + at, struct.pack("<Q", target), 8)

SINGLE = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int)(
    block + labels["single_click"])
DOUBLE = ctypes.CFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p)(block + labels["double_click"])
SQUAD_OF = ctypes.CFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p)(block + labels["squad_of"])

failures = 0


def check(label, got, want, names=None):
    global failures
    ok = got == want
    if not ok:
        failures += 1
    show = (lambda v: names.get(v, hex(v) if v else "NULL")) if names else \
        (lambda v: hex(v) if v else "NULL")
    print(f"{'PASS' if ok else 'FAIL'}  {label}")
    if not ok:
        print(f"        wanted {show(want)}, got {show(got)}")


print("== squad_of walks the facet chain\n")
squad = entity()
member = member_of(squad)
names = {squad: "the squad", member: "the member", 0: "NULL"}
check("a full chain gives the squad", SQUAD_OF(member), squad, names)
for missing in ("selectable", "parent", "holder"):
    m = member_of(squad, **{missing: False})
    check(f"a missing {missing} hop falls back to the member", SQUAD_OF(m), m)

# The crash shape from the field: on a squad's facet, +0x28 is not a facet
# pointer but a flag byte with its neighbours behind it, which reads as a huge
# non-canonical value. The null test accepts it, and dereferencing its +0x10
# is what faulted; the range guard must reject it instead.
for label, junk in (("a non-canonical value", 0x4F7FAC0300000000),
                    ("a tiny bogus pointer", 1)):
    facets = blob(bytes(bytearray(0x60)), 0x60)
    own = blob(bytes(bytearray(0x40)), 0x40)
    poke(own, 0x28, junk)
    poke(facets, 0x50, own)
    crashy = entity(facets)
    check(f"+0x28 holding {label} falls back instead of faulting", SQUAD_OF(crashy), crashy,
          {crashy: "the entity"})

check("no entity at all is handed straight back", SQUAD_OF(None) or 0, 0)
bare = entity()
check("an entity with no facets falls back to itself", SQUAD_OF(bare), bare,
      {bare: "the entity"})

print("\n== the single click expands only without Ctrl\n")
# the stub leaves the entity it chose in rdx; a shim captures it
CAPTURE = scratch(8)
capture_stub = blob(asm(f"mov rax, {CAPTURE}; mov qword ptr [rax], rdx; ret"))
at = code.find(struct.pack("<Q", 0xaaaaaaaaaaaaaaa1))
ctypes.memmove(block + at, struct.pack("<Q", capture_stub), 8)

member = member_of(squad)
names = {squad: "the squad", member: "the member", 0: "NULL"}
SINGLE(None, member, 0)
check("a plain click selects the squad",
      int.from_bytes((ctypes.c_char * 8).from_address(CAPTURE).raw, "little"), squad, names)
SINGLE(None, member, 1)
check("Ctrl+click leaves the member",
      int.from_bytes((ctypes.c_char * 8).from_address(CAPTURE).raw, "little"), member, names)

standalone = entity()
SINGLE(None, standalone, 0)
check("an entity with no squad is left alone",
      int.from_bytes((ctypes.c_char * 8).from_address(CAPTURE).raw, "little"), standalone,
      {standalone: "the entity"})

print("\n== the double click always expands\n")
ctypes.memmove(RESOLVED, struct.pack("<Q", member), 8)
check("it expands whatever the resolver found", DOUBLE(None) or 0, squad, names)
ctypes.memmove(RESOLVED, struct.pack("<Q", 0), 8)
check("nothing under the cursor stays nothing", DOUBLE(None) or 0, 0, names)
ctypes.memmove(RESOLVED, struct.pack("<Q", standalone), 8)
check("an entity with no squad is left alone", DOUBLE(None) or 0, standalone,
      {standalone: "the entity"})

print("\n== the world cursor's plain select expands, and Ctrl keeps the one\n")
RESUME_WORLD = blob(asm("ret"))
at = code.find(struct.pack("<Q", 0xaaaaaaaaaaaaaaa4))
assert at >= 0, "the world resume slot was not found"
ctypes.memmove(block + at, struct.pack("<Q", RESUME_WORLD), 8)

# GetAsyncKeyState is resolved by name when the injector runs; stand in for it
# here so Ctrl can be driven from the test
EXPORT_SLOTS = [code.find(struct.pack("<Q", p))
                for p in (0xaaaaaaaaaaaaaaa6, 0xaaaaaaaaaaaaaaa8)]
assert all(slot >= 0 for slot in EXPORT_SLOTS), "a GetAsyncKeyState slot was not found"


def press_ctrl(down):
    value = 0x8000 if down else 0
    fake = blob(asm(f"mov eax, {value:#x}; ret"))
    for slot in EXPORT_SLOTS:
        ctypes.memmove(block + slot, struct.pack("<Q", fake), 8)


# the manager stand-in: its vt+0x60 records the entity it is handed
SEL = scratch(8)
select_capture = blob(asm(f"mov rax, {SEL}; mov qword ptr [rax], rdx; ret"))
manager_vt = blob(bytes(bytearray(0x80)), 0x80)
poke(manager_vt, 0x60, select_capture)
manager = blob(bytes(bytearray(0x20)), 0x20)
poke(manager, 0, manager_vt)

# entered as the hook enters it: the manager vtable in rax, the manager in rcx,
# the entity in rdx
shim = blob(asm(
    f"mov rax, qword ptr [rcx]; mov r11, {block + labels['world_select']}; call r11; ret"))
WORLD = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)(shim)

member = member_of(squad)
press_ctrl(False)
WORLD(manager, member)
check("plain: the squad is selected, not the member",
      int.from_bytes((ctypes.c_char * 8).from_address(SEL).raw, "little"), squad,
      {squad: "the squad", member: "the member"})
standalone = entity()
WORLD(manager, standalone)
check("a squadless entity is passed through unchanged",
      int.from_bytes((ctypes.c_char * 8).from_address(SEL).raw, "little"), standalone,
      {standalone: "the entity"})
press_ctrl(True)
WORLD(manager, member)
check("Ctrl: the one soldier is selected",
      int.from_bytes((ctypes.c_char * 8).from_address(SEL).raw, "little"), member,
      {squad: "the squad", member: "the member"})

print("\n== the world Shift toggle expands too, so it toggles the squad\n")
TOGGLE = scratch(8)
resume_toggle = blob(asm(f"mov rax, {TOGGLE}; mov qword ptr [rax], rdx; ret"))
at = code.find(struct.pack("<Q", 0xaaaaaaaaaaaaaaa5))
assert at >= 0, "the toggle resume slot was not found"
ctypes.memmove(block + at, struct.pack("<Q", resume_toggle), 8)
shim_toggle = blob(asm(
    f"mov rax, qword ptr [rcx]; mov r11, {block + labels['world_toggle']}; call r11; ret"))
SHIFT = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)(shim_toggle)
member = member_of(squad)
press_ctrl(False)
SHIFT(manager, member)
check("Shift: the squad is toggled, not the member",
      int.from_bytes((ctypes.c_char * 8).from_address(TOGGLE).raw, "little"), squad,
      {squad: "the squad", member: "the member"})
standalone = entity()
SHIFT(manager, standalone)
check("a squadless entity toggles unchanged",
      int.from_bytes((ctypes.c_char * 8).from_address(TOGGLE).raw, "little"), standalone,
      {standalone: "the entity"})
press_ctrl(True)
SHIFT(manager, member)
check("Ctrl+Shift: the one soldier is toggled",
      int.from_bytes((ctypes.c_char * 8).from_address(TOGGLE).raw, "little"), member,
      {squad: "the squad", member: "the member"})

print("\n== clicks complete subsets; Ctrl ignores squad containers\n")
WENT = scratch(0x18)
went = [blob(asm(f"mov r11, {WENT}; mov qword ptr [r11], {n}; "
                 "mov qword ptr [r11 + 8], rsi; add rsp, 0x28; pop rdi; pop rsi; ret"))
        for n in (1, 2)]
for placeholder, target in ((0xaaaaaaaaaaaaaaab, went[0]), (0xaaaaaaaaaaaaaaac, went[1])):
    at = code.index(struct.pack("<Q", placeholder))
    poke(block, at, target)
KEY_RSP = scratch(8)
key_slot = code.index(struct.pack("<Q", 0xaaaaaaaaaaaaaaad))
chooser = scratch(0xb0)
shim_rule = blob(asm(f"push rsi; push rdi; sub rsp, 0x28; mov rsi, rcx; mov rdi, rdx; "
                     f"mov r11, {block + labels['ctrl_select']}; jmp r11"))
RULE = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)(shim_rule)
for kind in (0, 0x10, 0x200):
    hit = entity()
    vt = ctypes.c_uint64.from_address(hit).value
    poke(vt, 0x98, blob(asm(f"mov eax, {kind}; and eax, edx; ret")))
    holder = scratch(0x18)
    poke(holder, 0x10, hit)
    facet = scratch(0x20)
    poke(facet, 0x10, holder)
    for down in (False, True):
        want = 1 if down and kind == 0x10 else 2
        poke(block, key_slot, blob(asm(
            f"mov r11, {KEY_RSP}; mov qword ptr [r11], rsp; mov eax, {0x8000 if down else 0}; ret")))
        # The old gate swallowed clicks with zero/one selected squad, including
        # partially marked squads. Every selection count now reaches the builder.
        for count in (0, 1, 2):
            poke(chooser, 0x98, 0x1000)
            poke(chooser, 0xa0, 0x1000 + count*8)
            RULE(chooser, facet)
            check(f"Ctrl={down} kind={kind:x} selection count={count}",
                  ctypes.c_uint64.from_address(WENT).value, want)
            check("chooser survives", ctypes.c_uint64.from_address(WENT+8).value, chooser)
            check("key query alignment", ctypes.c_uint64.from_address(KEY_RSP).value % 16, 8)

poke(block, key_slot, blob(asm("xor eax, eax; ret")))

print("\n== world double-click preserves the stock type reference\n")
# Enter by jump with the original mid-function stack alignment and shadow
# space. The resume target restores this shim's nonvolatile registers.
resume_world_double = blob(asm("add rsp, 0x28; pop r14; pop rdi; ret"))
at = code.find(struct.pack("<Q", 0xaaaaaaaaaaaaaaa9))
assert at >= 0
poke(block, at, resume_world_double)
type_capture = blob(asm(f"mov rax, {SEL}; mov qword ptr [rax], r8; ret"))
type_vt = scratch(0x88)
poke(type_vt, 0x80, type_capture)
type_manager = scratch(0x38)  # empty registry for this argument/ABI check
poke(type_manager, 0, type_vt)
shim_double = blob(asm(
    "push rdi; push r14; sub rsp, 0x28; mov r14, rcx; mov rdi, rdx; "
    f"mov r11, {block + labels['world_double']}; jmp r11"))
WORLD_DOUBLE = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)(shim_double)
for label, clicked, expected in [("soldier", member, member),
                                  ("squad icon", squad, squad),
                                  ("standalone entity", standalone, standalone)]:
    WORLD_DOUBLE(type_manager, clicked)
    check(f"double-click {label}: correct type reference",
          ctypes.c_uint64.from_address(SEL).value, expected)

print("\n== Ctrl never expands icon gestures\n")
icon = entity()
poke(ctypes.c_uint64.from_address(icon).value, 0x98,
     blob(asm("mov eax, 0x10; and eax, edx; ret")))
poke(CAPTURE, 0, 0x1234)
SINGLE(None, icon, 1)
check("Ctrl army squad row never reaches selection", ctypes.c_uint64.from_address(CAPTURE).value, 0x1234)
poke(block, key_slot, blob(asm("mov eax, 0x8000; ret")))
poke(RESOLVED, 0, member)
check("Ctrl army double-click is ignored", DOUBLE(None) or 0, 0)
for clicked in (member, icon):
    poke(SEL, 0, 0x1234)
    WORLD_DOUBLE(type_manager, clicked)
    check("Ctrl world double-click never reaches type selection", ctypes.c_uint64.from_address(SEL).value, 0x1234)
poke(block, key_slot, blob(asm("xor eax, eax; ret")))

print("\n== the trace records every hop\n")
trace = [int.from_bytes((ctypes.c_char * 8).from_address(block + TRACE + i * 8).raw, "little")
         for i in range(8)]
before = trace[7]
check("it has been counting calls", before > 0, True)
member = member_of(squad)
SINGLE(None, member, 0)
trace = [int.from_bytes((ctypes.c_char * 8).from_address(block + TRACE + i * 8).raw, "little")
         for i in range(8)]
check("each call bumps the counter", trace[7], before + 1)
check("the entity handed in is recorded", trace[0], member)
check("and the squad it resolved to", trace[6], squad)

print()
print(f"{failures} failed" if failures else "all cases as expected")
sys.exit(1 if failures else 0)
