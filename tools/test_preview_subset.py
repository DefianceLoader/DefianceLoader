"""Run the squad preview's selection marks and dimming on fabricated objects.

patch/preview-subset.asm (game.dll): subset_mark writes a soldier's dead byte
from his selectable facet, and subset_refresh decides between the stock test,
nothing, and a rebuild when the shown squad's selection changes.
patch/preview-dim.asm (logic.dll): dim_pose, dim_mode and dim_part read the
byte in the preview builder. Each entry is reached from a small wrapper that
sets the registers the site holds, and each way back into the module is a stub
that records what the site would see and returns to the test.
"""
import ctypes, pathlib, struct, sys
from ctypes import wintypes
import keystone
sys.path.insert(0, "tools")
import build as b

ks = keystone.Ks(keystone.KS_ARCH_X86, keystone.KS_MODE_64)
asm = lambda text, at=0: bytes(ks.asm(text.encode(), at)[0])

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
kernel32.VirtualAlloc.restype = ctypes.c_void_p
kernel32.VirtualAlloc.argtypes = [ctypes.c_void_p, ctypes.c_size_t, wintypes.DWORD, wintypes.DWORD]


def scratch(size):
    addr = kernel32.VirtualAlloc(None, max(size, 8), 0x3000, 0x40)
    if not addr:
        raise OSError("VirtualAlloc failed")
    return addr


def blob(data):
    addr = scratch(len(data))
    ctypes.memmove(addr, data, len(data))
    return addr


def poke(addr, offset, value, width=8):
    ctypes.memmove(addr + offset, int(value).to_bytes(width, "little"), width)


def peek(addr, offset=0, width=8):
    return int.from_bytes(ctypes.string_at(addr + offset, width), "little")


failures = 0


def check(label, got, want):
    global failures
    ok = got == want
    if not ok:
        failures += 1
    print(f"{'PASS' if ok else 'FAIL'}  {label}" + ("" if ok else f" (wanted {want!r}, got {got!r})"))


# ---------------------------------------------------------------- game.dll

GAME_STATE = 0x400                     # the cell, after the code
game_code, game_labels = b.assemble(
    pathlib.Path("patch/preview-subset.asm").read_text().splitlines(), 0, 0, GAME_STATE,
    layout={}, symbols=b.REFERENCE_GAME_SYMBOLS)
assert len(game_code) <= GAME_STATE
game = scratch(0x1000)
ctypes.memmove(game, game_code, len(game_code))
observed = scratch(0x100)


def game_fixup(placeholder, destination):
    at = game_code.index(struct.pack("<Q", placeholder))
    ctypes.memmove(game + at, struct.pack("<Q", destination), 8)


# vtable slots: a method that answers `kind == edx`, constants, and a facet's
# selected byte at +0x19
def is_kind(kind):
    return blob(asm(f"xor eax, eax; cmp edx, {kind}; sete al; ret"))


def returns(value):
    return blob(asm(f"mov rax, {value}; ret"))


SELECTED = blob(asm("movzx eax, byte ptr [rcx + 0x19]; ret"))


def vtable(slots):
    table = scratch(0x400)
    for slot, target in slots.items():
        poke(table, slot, target)
    return table


def soldier(selected, selectable=True):
    facet = scratch(0x40)
    poke(facet, 0, vtable({0x58: SELECTED}))
    poke(facet, 0x18, int(selectable), 1)
    poke(facet, 0x19, int(selected), 1)
    parts = scratch(0x60)
    poke(parts, 0x50, facet)
    entity = scratch(0x20)
    poke(entity, 0, vtable({0x98: is_kind(0x20), 0xb0: returns(parts)}))
    return entity, facet


def squad(members):
    vector = scratch(0x10 + 8 * len(members))
    for i, member in enumerate(members):
        poke(vector, 0x10 + 8 * i, member)
    poke(vector, 0, vector + 0x10)
    poke(vector, 8, vector + 0x10 + 8 * len(members))
    roster = scratch(0x10)
    poke(roster, 0, vtable({0x68: returns(vector)}))
    ai = scratch(0x10)
    poke(ai, 0, vtable({b.REFERENCE_GAME_SYMBOLS["squad_roster"]: returns(roster)}))
    parts = scratch(0x40)
    poke(parts, 0x28, ai)
    entity = scratch(0x20)
    poke(entity, 0, vtable({0x98: is_kind(0x10), 0xb0: returns(parts)}))
    return entity


print("== subset_mark writes the dead byte from the soldier's facet\n")
# The site: rbp the provider's frame, rcx the soldier, rsp aligned. The resume
# stub records rcx and xmm0 (the displaced xorps) and returns.
game_fixup(0xaaaaaaaaaaaaaac4, blob(asm(
    f"mov r10, {observed}; mov [r10], rcx; movq rax, xmm0; mov [r10 + 8], rax; pop rbp; ret")))
MARK = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)(blob(asm(
    f"push rbp; mov rbp, rdx; movq xmm0, rdx; mov r11, {game + game_labels['subset_mark']}; jmp r11")))


def mark(entity):
    frame = scratch(0x200) + 0x100
    poke(frame, -0x79, 0x55, 1)
    MARK(entity, frame)
    check("  the soldier survives in rcx", peek(observed), entity)
    check("  xmm0 is cleared", peek(observed, 8), 0)
    return peek(frame, -0x79, 1)


check("a selected soldier stays 0", mark(soldier(True)[0]), 0)
check("an unselected soldier is 2", mark(soldier(False)[0]), 2)
check("an unselectable soldier stays 0", mark(soldier(False, selectable=False)[0]), 0)
not_soldier = scratch(0x20)
poke(not_soldier, 0, vtable({0x98: is_kind(0x10)}))
check("a non-soldier stays 0", mark(not_soldier), 0)

print("\n== subset_refresh rebuilds when the shown squad's selection changes\n")
# The site: rbx the panel, rdi the entity to show, r14 the shown reference.
# Resuming at the stock je records ZF (1 skips, 0 rebuilds as stock); the
# rebuild entry answers 2. Both record rbx, rdi and r14.
SAVE = f"mov r10, {observed}; mov [r10], rbx; mov [r10 + 8], rdi; mov [r10 + 0x10], r14;"
game_fixup(0xaaaaaaaaaaaaaac5, blob(asm(f"sete al; movzx eax, al; {SAVE} pop r14; pop rdi; pop rbx; ret")))
game_fixup(0xaaaaaaaaaaaaaac6, blob(asm(f"mov eax, 2; {SAVE} pop r14; pop rdi; pop rbx; ret")))
REFRESH = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p)(blob(asm(
    "push rbx; push rdi; push r14; mov rbx, rcx; mov rdi, rdx; mov r14, r8;"
    f"mov r11, {game + game_labels['subset_refresh']}; jmp r11")))
widget = scratch(0x80)
panel = scratch(0x200)
poke(panel, 0x118, widget)


def refresh(shown, entity):
    holder = scratch(0x20)
    poke(holder, 0x10, shown)
    reference = scratch(0x10)
    poke(reference, 0, holder if shown else 0)
    poke(widget, 0x38, 0, 1)
    result = REFRESH(panel, entity, reference)
    check("  rbx, rdi and r14 survive", (peek(observed), peek(observed, 8), peek(observed, 0x10)),
          (panel, entity, reference))
    return result, peek(widget, 0x38, 1)


alice, alice_facet = soldier(True)
bob, bob_facet = soldier(False)
team = squad([alice, bob])
other = squad([soldier(True)[0]])
check("a new entity takes the stock path", refresh(other, team), (0, 0))
check("its signature is recorded", (peek(game, GAME_STATE), peek(game, GAME_STATE + 8)), (team, 0b110))
check("the same selection rebuilds nothing", refresh(team, team), (1, 0))
poke(bob_facet, 0x19, 1, 1)
check("a changed selection rebuilds", refresh(team, team), (2, 1))
check("and then settles", refresh(team, team), (1, 0))
poke(alice_facet, 0x18, 0, 1)
check("a member who stops being selectable rebuilds", refresh(team, team), (2, 1))
check("nothing shown rebuilds nothing", refresh(0, 0), (1, 0))
lone = soldier(False)[0]
check("a single soldier takes the stock path", refresh(team, lone), (0, 0))
check("a single soldier never rebuilds", refresh(lone, lone), (1, 0))

# ---------------------------------------------------------------- logic.dll

LOGIC_CELL = 0x400
logic_code, logic_labels = b.assemble(
    pathlib.Path("patch/preview-dim.asm").read_text().splitlines(), 0, 0, LOGIC_CELL, layout={})
logic = scratch(0x1000)
ctypes.memmove(logic, logic_code, len(logic_code))
# Stubs live in the same allocation, so the module branches can reach them.
stub_at = [0x600]


def stub(text):
    data = asm(text)
    at = logic + stub_at[0]
    ctypes.memmove(at, data, len(data))
    stub_at[0] += (len(data) + 15) & ~15
    return at


def logic_branches(targets):
    """Re-aim each rel32 into logic.dll at the stub for its target."""
    for offset, target in b.module_branches(logic_code, 0):
        ctypes.memmove(logic + offset, struct.pack("<i", targets[target] - (logic + offset + 4)), 4)


LOG = scratch(0x100)
POSE_BACK = stub(f"mov r10, {LOG}; mov [r10], r14; pop r14; pop r12; ret")
MODE_PARTS = stub("mov eax, 1; pop rbp; ret")
MODE_SKIP = stub("mov eax, 0; pop rbp; ret")
PART_CALL = stub(f"mov r10, {LOG}; mov [r10], rcx; mov [r10 + 8], rax; mov [r10 + 0x10], rdx;"
                 "mov eax, 1; pop rbx; pop r15; pop rsi; ret")
PART_NEXT = stub("mov eax, 2; pop rbx; pop r15; pop rsi; ret")
logic_branches({0x20349f: POSE_BACK, 0x2037fe: MODE_PARTS, 0x203897: MODE_SKIP,
                0x20383a: PART_CALL, 0x20383d: PART_NEXT})

print("\n== dim_pose reads 2 as alive\n")
POSE = ctypes.CFUNCTYPE(None, ctypes.c_void_p)(stub(
    f"push r12; push r14; mov r12, rcx; mov r11, {logic + logic_labels['dim_pose']}; jmp r11"))
for byte, want in ((0, 0), (1, 1), (2, 0)):
    descriptor = scratch(0x38)
    poke(descriptor, 0x20, byte, 1)
    POSE(descriptor)
    check(f"dead byte {byte} poses as {want}", peek(LOG), want)

print("\n== dim_mode sets unselected soldiers of a mixed squad apart\n")
MODE = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p)(stub(
    f"push rbp; mov rbp, rcx; mov r11, {logic + logic_labels['dim_mode']}; jmp r11"))


def mode(own, squad_bytes, callback=1):
    poke(logic, LOGIC_CELL, callback)
    poke(logic, LOGIC_CELL + 8, 0x77, 1)
    descriptors = scratch(0x38 * max(len(squad_bytes), 1))
    for i, byte in enumerate(squad_bytes):
        poke(descriptors, 0x38 * i + 0x20, byte, 1)
    descriptor = scratch(0x38)
    poke(descriptor, 0x20, own, 1)
    frame = scratch(0x200) + 0x100
    poke(frame, -0x78, descriptor)
    poke(frame, 0x90, descriptors)
    poke(frame, 0x98, descriptors + 0x38 * len(squad_bytes))
    return MODE(frame), peek(logic, LOGIC_CELL + 8, 1)


check("alive and selected: no silhouette", mode(0, [0, 2]), (0, 0))
check("dead: the stock silhouette", mode(1, [0, 1]), (1, 0))
check("unselected in a mixed squad: dimmed", mode(2, [2, 0, 1]), (1, 2))
check("unselected with nobody selected: as stock", mode(2, [2, 2, 1]), (0, 0))
check("without Core's callback: as stock", mode(2, [2, 0], callback=0), (0, 0))

print("\n== dim_part swaps the override for the callback's material\n")
CALLS = scratch(0x40)
CALLBACK = stub(f"mov r10, {CALLS}; mov [r10], rcx; mov [r10 + 8], rdx; mov rax, rsp; and eax, 0xf;"
                "mov [r10 + 0x10], rax; mov rax, [rcx + 8]; ret")
PART = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)(stub(
    "push rsi; push r15; push rbx; mov rsi, rcx; mov r15, rdx;"
    f"mov r11, {logic + logic_labels['dim_part']}; jmp r11"))
part_vtable = scratch(0x100)
context = scratch(0x40)


def part(dimmed, shared):
    poke(logic, LOGIC_CELL, CALLBACK)
    poke(logic, LOGIC_CELL + 8, 2 if dimmed else 0, 1)
    renderer = scratch(0x60)
    poke(renderer, 0, part_vtable)
    poke(renderer, 8, shared)
    cursor = scratch(0x10)
    poke(cursor, 0, renderer)
    poke(CALLS, 0, 0)
    result = PART(cursor, context)
    return renderer, result


renderer, result = part(False, 0x1234)
check("stock: the setter is called", result, 1)
check("stock: with the context's material", (peek(LOG), peek(LOG, 8), peek(LOG, 0x10)),
      (renderer, part_vtable, context + 0x20))
check("stock: the callback is not asked", peek(CALLS), 0)
renderer, result = part(True, 0x1234)
check("dimmed: the setter is called", result, 1)
check("dimmed: with the callback's material", (peek(LOG), peek(LOG, 8), peek(LOG, 0x10)),
      (renderer, part_vtable, 0x1234))
check("dimmed: the callback gets the part and the context", (peek(CALLS), peek(CALLS, 8)),
      (renderer, context))
check("dimmed: the callback's stack is aligned", peek(CALLS, 0x10), 8)
renderer, result = part(True, 0)
check("no dimmed material: the part is skipped", result, 2)

print()
print(f"{failures} failed" if failures else "all cases as expected")
sys.exit(1 if failures else 0)
