"""Run patch/ammo-mode.asm natively on fabricated squads and soldiers.

The setter and getter are entered directly, as their sites call them: rcx the
AI facet, rdx the slot index, and the setter's r8d the value. Fabricated
objects have the layout the routines read: AI facet +0x10 -> holder +0x10 ->
entity; entity vt+0xb0 -> facets, facets +0x28 the AI facet and +0x50 the
selectable facet; a soldier's facet +0x18 enabled, +0x28 his squad's facet,
+0x30 his mark (vt+0x58 answers enabled and marked); the AI facet's vt+0x1b8
-> ammo data, whose vt+0x48 -> a slot vector of 0x48-byte records with the
enabled dword at +0x3c. A soldier AI's vt+0x3b8 answers zero, a squad's the
member roster, which is what tells the two apart.
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


def dword(addr):
    return ctypes.c_uint32.from_address(addr).value


def byte(addr):
    return ctypes.c_ubyte.from_address(addr).value


def obj(size, fields=()):
    addr = scratch(size)
    for offset, value in fields:
        put(addr + offset, struct.pack("<Q", value))
    return addr


region = scratch(0x400000)
block = region
code, labels = b.assemble(pathlib.Path("patch/ammo-mode.asm").read_text().splitlines(),
                          b.AMMO_OFFSET, b.TRACE_OFFSET, b.AMMO_SCRATCH)
put(block + b.AMMO_OFFSET, code)

# stubs standing in for virtual methods
RET8 = obj(0x80)
put(RET8, asm("mov qword ptr [rsp + 8], rcx; mov qword ptr [rsp + 0x10], rdx; "
                 "mov qword ptr [rsp + 0x18], r8; mov qword ptr [rsp + 0x20], r9; "
                 "movaps xmmword ptr [rsp + 8], xmm0; mov rax, qword ptr [rcx + 8]; xor ecx, ecx; ret"))
RET20 = obj(0x10)
put(RET20, asm("mov rax, qword ptr [rcx + 0x20]; ret"))
RET48 = obj(0x10)
put(RET48, asm("mov rax, qword ptr [rcx + 0x48]; ret"))
NONE = obj(0x10)
put(NONE, asm("xor eax, eax; ret"))
GETTER = obj(0x10)
put(GETTER, asm("movzx eax, byte ptr [rcx + 0x30]; and al, byte ptr [rcx + 0x18]; ret"))
IS_SOLDIER = obj(0x20)
put(IS_SOLDIER, asm("cmp edx, 0x20; sete al; ret"))
ENT_VT = obj(0xc0, [(0x98, IS_SOLDIER), (0xb0, RET8)])
FACET_VT = obj(0x60, [(0x58, GETTER)])
# the AI facet: vt+0x1b8 -> [this+8] the ammo data, vt+0x3b8 -> [this+0x20]
AI_VT = obj(0x400, [(0x1b8, RET8), (0x3b8, RET20)])
SOLDIER_VT = obj(0x400, [(0x130, NONE), (0x1b8, RET8), (0x3b8, NONE)])
# the ammo data object: vt+0x48 -> [this+8] the slot vector
DATA_VT = obj(0x60, [(0x48, RET8)])
# the member roster: vt+0x68 -> [this+8] the member vector
ROSTER_VT = obj(0x70, [(0x68, RET8)])


def ammo(values):
    """A slot vector of len(values) records, +0x3c the value. Returns its base."""
    base = scratch(0x48 * max(1, len(values)))
    for i, v in enumerate(values):
        put(base + i * 0x48 + 0x3c, struct.pack("<I", v))
    vec = obj(0x10, [(0, base), (8, base + 0x48 * len(values))])
    return obj(0x40, [(0, DATA_VT), (8, vec), (0x20, base), (0x28, base + 0x48 * len(values))]), base


def soldier(ai_vt, marked=False, enabled=True, values=(0,)):
    data, base = ammo(list(values))
    facet = obj(0x40, [(0, FACET_VT)])
    put(facet + 0x18, bytes([int(enabled)]))
    put(facet + 0x30, bytes([int(marked)]))
    ai = obj(0x200, [(0, ai_vt), (8, data)])
    facets = obj(0x60, [(0x28, ai), (0x50, facet)])
    entity = obj(0xc0, [(0, ENT_VT), (8, facets), (0xb8, facet)])
    return {"ai": ai, "data": data, "base": base, "facet": facet, "entity": entity}


def squad(members):
    """Wires the members to one squad facet and returns the squad's AI facet."""
    pointers = scratch(8 * max(1, len(members)))
    for i, m in enumerate(members):
        put(pointers + i * 8, struct.pack("<Q", m["entity"]))
    vector = obj(0x10, [(0, pointers), (8, pointers + 8 * len(members))])
    roster = obj(0x10, [(0, ROSTER_VT), (8, vector)])
    data, base = ammo([0] * 4)
    own = obj(0x40, [(0, FACET_VT)])
    for m in members:
        put(m["facet"] + 0x28, struct.pack("<Q", own))
        put(m["ai"] + 8, struct.pack("<Q", data))
        m["data"], m["base"] = data, base
        soldier_of(m["ai"], m["entity"])
    ai = obj(0x40, [(0, AI_VT), (8, data), (0x20, roster)])
    facets = obj(0x60, [(0x28, ai), (0x50, own)])
    entity = obj(0x28, [(0, ENT_VT), (8, facets)])
    holder = obj(0x18, [(0x10, entity)])
    put(ai + 0x10, struct.pack("<Q", holder))
    return {"ai": ai, "data": data, "base": base, "roster": roster}


def soldier_of(ai, entity):
    """A lone soldier's AI facet, wired so list_members finds no squad."""
    holder = obj(0x18, [(0x10, entity)])
    put(ai + 0x10, struct.pack("<Q", holder))
    return ai


SET = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_uint64, ctypes.c_uint32)(
    block + labels["ammo_set"])
GET = ctypes.CFUNCTYPE(ctypes.c_uint32, ctypes.c_void_p, ctypes.c_uint64)(
    block + labels["ammo_get"])

failures = 0


def check(label, ok, detail=""):
    global failures
    if not ok:
        failures += 1
    print(f"{'PASS' if ok else 'FAIL'}  {label}{'' if ok else '  ' + detail}")


def slot(data, base, index=0):
    return dword(base + index * 0x48 + 0x3c)


print("== a lone soldier is written and read through his own slots\n")
alone = soldier(SOLDIER_VT)
soldier_of(alone["ai"], alone["entity"])
SET(alone["ai"], 0, 0)
check("the slot takes the value", slot(alone["data"], alone["base"]) == 0,
      str(slot(alone["data"], alone["base"])))
SET(alone["ai"], 0, 1)
check("and back", slot(alone["data"], alone["base"]) == 1)
check("the getter reads it", GET(alone["ai"], 0) == 1, str(GET(alone["ai"], 0)))
check("a slot past the end reads 1", GET(alone["ai"], 4) == 1, str(GET(alone["ai"], 4)))

print("\n== one of three marked: only he is pinned, the shared vector is not touched\n")
men = [soldier(SOLDIER_VT), soldier(SOLDIER_VT, marked=True), soldier(SOLDIER_VT)]
sq = squad(men)
SET(sq["ai"], 0, 1)
check("the marked soldier carries a pin",
      byte(men[1]["facet"] + 0x19) == 0xA5
      and byte(men[1]["facet"] + 0x1e) & 1 and byte(men[1]["facet"] + 0x1f) & 1,
      f"marker {byte(men[1]['facet'] + 0x19):#x} set {byte(men[1]['facet'] + 0x1e):#x} "
      f"value {byte(men[1]['facet'] + 0x1f):#x}")
check("an unmarked soldier has none", byte(men[0]["facet"] + 0x19) != 0xA5)
check("the shared vector is left alone", slot(sq["data"], sq["base"]) == 0,
      str(slot(sq["data"], sq["base"])))
check("the getter answers with the pin", GET(sq["ai"], 0) == 1)
SET(sq["ai"], 0, 0)
check("a second toggle pins the other value", GET(sq["ai"], 0) == 0
      and not (byte(men[1]["facet"] + 0x1f) & 1))

print("\n== nobody, or everybody, marked: the pins clear and the squad is written\n")
for how, marks in (("nobody marked", (False, False, False)),
                   ("everybody marked", (True, True, True))):
    men = [soldier(SOLDIER_VT, marked=m) for m in marks]
    sq = squad(men)
    for m in men:                       # a stale pin the squad change must drop
        put(m["facet"] + 0x19, bytes([0xA5]))
        put(m["facet"] + 0x1e, bytes([1]))
        put(m["facet"] + 0x1f, bytes([1]))
    SET(sq["ai"], 0, 0)
    check(f"{how}: the shared vector changes", slot(sq["data"], sq["base"]) == 0,
          str(slot(sq["data"], sq["base"])))
    check(f"{how}: every pin is cleared", all(
        not (byte(m["facet"] + 0x1e) & 1) for m in men))
    check(f"{how}: the getter is the shared value", GET(sq["ai"], 0) == 0)

print("\n== a slot number is carried into the pin\n")
men = [soldier(SOLDIER_VT), soldier(SOLDIER_VT, marked=True, values=(7, 7, 7, 7))]
sq = squad(men)
SET(sq["ai"], 2, 0)
facet = men[1]["facet"]
check("slot 2 is pinned, the other slots are not",
      byte(facet + 0x1e) == 1 << 2 and byte(facet + 0x1f) == 0,
      f"set {byte(facet + 0x1e):#x} value {byte(facet + 0x1f):#x}")
check("the shared slots are untouched",
      [slot(men[1]["data"], men[1]["base"], i) for i in range(4)] == [0, 0, 0, 0],
      str([slot(men[1]["data"], men[1]["base"], i) for i in range(4)]))
check("the getter reads the pin", GET(sq["ai"], 2) == 0)

print("\n== the reader helper substitutes a pin for the shared value\n")
tramp = scratch(0x40)
put(tramp, asm("push rsi; sub rsp, 0x20; mov r10, r8; mov eax, ecx; "
               f"mov r11, {block + labels['ammo_effective']:#x}; call r11; add rsp, 0x20; pop rsi; ret"))
EFFECTIVE = ctypes.CFUNCTYPE(ctypes.c_uint32, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p)(tramp)
pin = men[1]["facet"]
original_facet = ctypes.string_at(pin, 0x38)
check("the pinned slot answers the pin", EFFECTIVE(7, 2, men[1]["facet"]) == 0)
check("another slot keeps the shared value", EFFECTIVE(7, 1, men[1]["facet"]) == 7)
check("an unpinned facet keeps the shared value", EFFECTIVE(7, 2, men[0]["facet"]) == 7)
check("a missing selectable keeps the shared value", EFFECTIVE(7, 2, None) == 7)
check("the reader never writes into the facet", ctypes.string_at(pin, 0x38) == original_facet)

print("\n== the trace ring records what the setter was handed\n")
probe = soldier(SOLDIER_VT)
soldier_of(probe["ai"], probe["entity"])
before = q(block + b.TRACE_OFFSET)
SET(probe["ai"], 3, 5)
after = q(block + b.TRACE_OFFSET)
written = after - before
entry = (after - written) & 31
tag = lambda i: q(block + b.TRACE_OFFSET + 0x10 + i * 16)
sub = lambda i: q(block + b.TRACE_OFFSET + 0x18 + i * 16)
kinds = [((tag((entry + i) & 31) >> 56) & 0xff) for i in range(written)]
check("the entry and its slot/value are recorded",
      kinds[:2] == [0x40, 0x41] and sub(entry) == probe["ai"],
      f"kinds {[hex(k) for k in kinds]}")
check("the entry facet's data object follows",
      kinds[2] == 0x48 and sub((entry + 2) & 31) == probe["data"],
      f"kind {kinds[2] if len(kinds) > 2 else -1:#x}")
check("the single branch, then the facet written and its data",
      kinds[3:] == [0x45, 0x46, 0x47]
      and sub((entry + 4) & 31) == probe["ai"]
      and sub((entry + 5) & 31) == probe["data"],
      f"kinds {[hex(k) for k in kinds[3:]]}")

# a marked soldier instead takes the pin branch, and is traced as such
men = [soldier(SOLDIER_VT), soldier(SOLDIER_VT, marked=True)]
sq = squad(men)
before = q(block + b.TRACE_OFFSET)
SET(sq["ai"], 3, 5)
after = q(block + b.TRACE_OFFSET)
written = after - before
entry = (after - written) & 31
kinds = [((tag((entry + i) & 31) >> 56) & 0xff) for i in range(written)]
check("the marked branch records the pin",
      kinds == [0x40, 0x41, 0x48, 0x42, 0x44, 0x49]
      and sub((entry + 5) & 31) == men[1]["facet"],
      f"kinds {[hex(k) for k in kinds]}")

print("\n== independent slots and mixed selections\n")
men = [soldier(SOLDIER_VT, marked=True), soldier(SOLDIER_VT)]
sq = squad(men)
SET(sq["ai"], 1, 1)
SET(sq["ai"], 2, 1)
put(men[1]["facet"] + 0x30, b"\x01")
check("mixed whole-squad display stays enabled", GET(sq["ai"], 1) == 0)
SET(sq["ai"], 2, 0)
check("changing squad slot 2 preserves soldier slot 1", byte(men[0]["facet"] + 0x1e) == 2)
put(men[1]["facet"] + 0x30, b"\x00")
check("selecting him again shows his disabled slot", GET(sq["ai"], 1) == 1)
SET(sq["ai"], 1, 0)
check("the selected soldier can re-enable", GET(sq["ai"], 1) == 0)
put(men[1]["facet"] + 0x30, b"\x01")
SET(sq["ai"], 1, 1)
put(men[1]["facet"] + 0x30, b"\x00")
SET(sq["ai"], 1, 0)
check("one soldier can enable a squad-disabled slot", GET(sq["ai"], 1) == 0 and slot(sq["data"], sq["base"], 1) == 1)
put(men[0]["facet"] + 0x30, b"\x00")
put(men[1]["facet"] + 0x30, b"\x01")
check("the other soldier still reports disabled", GET(sq["ai"], 1) == 1)

print("\n== native roster gate and gun shims\n")
from pe import Image
stock = Image()
from capstone.x86 import X86_OP_MEM, X86_REG_RSP
resupply_bounds = stock.function_of(0x115a50)
stack_overlap = [i for i in stock.disasm(*resupply_bounds) for operand in i.operands
                 if operand.type == X86_OP_MEM and operand.mem.base == X86_REG_RSP
                 and operand.mem.disp < 0x30 and operand.mem.disp + operand.size > 0x28]
check("stock resupply frame leaves the owner-save slot unused", not stack_overlap)
check("stock replaces member AI with ammo data before the reader hooks",
      stock.read(0x115a11, 3) == bytes.fromhex("488bf0"))
STOCK_GATE = scratch(0x100)
put(STOCK_GATE, stock.read(0x117230, 0x80))
# A wrapper spills all four home slots, as any real Win64 method may do.
GATE_SPILL = scratch(0x100)
put(GATE_SPILL, asm("mov [rsp+8], rcx; mov [rsp+0x10], rdx; mov [rsp+0x18], r8; "
                    f"mov [rsp+0x20], r9; mov rax, {STOCK_GATE}; jmp rax"))
FIRE_DATA_VT = obj(0xc0, [(0x48, RET8), (0x60, GATE_SPILL)])

def gun_case(marked=True):
    men = [soldier(SOLDIER_VT, marked=marked), soldier(SOLDIER_VT)]
    sq = squad(men)
    put(sq["data"], struct.pack("<Q", FIRE_DATA_VT))
    weapons = [obj(0x130, [(0x11c, 0x20)]) for _ in range(4)]
    for i, weapon in enumerate(weapons):
        rec = sq["base"] + i * 0x48
        put(rec, struct.pack("<Q", weapon))
        put(rec + 0x2c, struct.pack("<II", 12, 2))
    guns = [obj(0x150, [(0x18, obj(0x18, [(0x10, m["entity"])])),
                       (0x50, weapons[1]), (0x58, obj(0x18, [(0x10, sq["data"])])),
                       (0xdc, 2)]) for m in men]
    return men, sq, weapons, guns

# Enter each hook with exactly the register convention of its call site.
def gate_callable(label, gun_reg):
    t = scratch(0x100)
    put(t, asm("push rbx; push rdi; push r13; push r15; sub rsp, 0x28; "
               f"mov {gun_reg}, rcx; mov rcx, [rcx+0x58]; mov rcx, [rcx+0x10]; "
               f"mov rax, {block + labels[label]}; call rax; "
               "setz dl; movzx edx, dl; shl rdx, 32; or rax, rdx; "
               "add rsp, 0x28; pop r15; pop r13; pop rdi; pop rbx; ret"))
    return ctypes.CFUNCTYPE(ctypes.c_uint64, ctypes.c_void_p, ctypes.c_void_p)(t)

men, sq, weapons, guns = gun_case()
for label, reg in (("ammo_gate_body", "rdi"), ("gate_from_rbx", "rbx"), ("gate_from_r13", "r13"), ("gate_from_r15", "r15")):
    gate = gate_callable(label, reg)
    SET(sq["ai"], 1, 1)
    before = ctypes.string_at(sq["base"], 4 * 0x48)
    check(f"{label}: selected gun disabled, ZF set", gate(guns[0], weapons[1]) == 1 << 32)
    check(f"{label}: other soldier retains stock ammo", gate(guns[1], weapons[1]) == 10)
    check(f"{label}: other weapon remains enabled", gate(guns[0], weapons[2]) == 10)
    check(f"{label}: queries do not mutate ammo or reserve",
          ctypes.string_at(sq["base"], 4 * 0x48) == before and dword(guns[0] + 0xdc) == 2)
    SET(sq["ai"], 1, 0)
    put(sq["base"] + 0x48 + 0x3c, struct.pack("<I", 1))
    check(f"{label}: pin enables despite squad disable", gate(guns[0], weapons[1]) == 10)
    check(f"{label}: unpinned gun respects squad disable", gate(guns[1], weapons[1]) == 1 << 32)
    put(sq["data"] + 0x38, b"\x01")
    put(weapons[1] + 0x11c, struct.pack("<I", 1))
    check(f"{label}: enabling does not bypass special-ammo restriction", gate(guns[0], weapons[1]) == 1 << 32)
    put(sq["data"] + 0x38, b"\x00")
    put(weapons[1] + 0x11c, struct.pack("<I", 0x20))
    put(sq["base"] + 0x48 + 0x3c, bytes(4))

# Non-soldier owners must ignore even a coincidental marker in their padding.
vehicle_vt = obj(0xc0, [(0x98, NONE), (0xb0, RET8)])
SET(sq["ai"], 1, 1)
put(men[0]["entity"], struct.pack("<Q", vehicle_vt))
check("vehicle owner ignores soldier padding pins", gate(guns[0], weapons[1]) == 10)
put(men[0]["entity"], struct.pack("<Q", ENT_VT))

print("\n== loaded-round queries consult the pin without losing the round\n")
put(guns[0] + 0xe2, b"\x01")
for label, off, on in (("gun_ready", 0, 1), ("gun_can_fire", 0, 1), ("gun_empty", 1, 0), ("gun_loaded", 0, 2)):
    call = ctypes.CFUNCTYPE(ctypes.c_uint32, ctypes.c_void_p)(block + labels[label])
    SET(sq["ai"], 1, 1)
    check(f"{label}: a reserved round is blocked", call(guns[0]) & 0xff == off)
    SET(sq["ai"], 1, 0)
    check(f"{label}: re-enable restores the original answer", call(guns[0]) & 0xff == on)
    check(f"{label}: reservation remains intact", dword(guns[0] + 0xdc) == 2)

print("\n== the actual resupply loop consumes the substituted flag on later slots\n")
# Copy the native pre-scan loop, patch its real hook, and replace its two exits.
# Slot 0 is full. Only slot 1 needs ammo, exposing the old first-iteration-only hook.
put(region + 0x115a49, stock.read(0x115a49, 0x25))
site, before, _ = b.AMMO_READER_HOOKS[0]
put(region + site, b"\xe9" + struct.pack("<i", labels["ammo_reader_check1"] - site - 5) + b"\x90")
put(region + 0x115a6b, asm("xor r12b, r12b"))
put(region + 0x115a6e, asm("movzx eax, r12b; xor eax, 1; ret"))
pre = scratch(0x100)
# Enter through the actual owner hook, then execute the stock virtual call
# that changes RSI from member AI to shared ammo data. Neither reader may
# interpret that shared data as a member AI, even when its bytes resemble one.
poison_entity = obj(0xc0, [(0xb8, 1)])
put(sq["data"] + 0x10, struct.pack("<Q", obj(0x18, [(0x10, poison_entity)])))
owner_site, owner_before, owner_label = b.AMMO_READER_HOOKS[2]
put(region + owner_site, b"\xe9" + struct.pack("<i", labels[owner_label] - owner_site - 5) + b"\x90" * (len(owner_before)-5))
put(region + 0x1158b1, asm("push rdx; mov rcx, rsi; mov rax, [rsi]; sub rsp, 0x20; "
                         "call [rax+0x1b8]; add rsp, 0x20; pop rdx; mov rsi, rax; "
                         "mov r8, rdx; lea rcx, [rdx+0x2c]; xor edx, edx; mov r9d, 2; mov r12d, 1; "
                         f"mov rax, {region + 0x115a49}; jmp rax"))
put(pre, asm("push rsi; push r12; sub rsp, 0x38; mov rax, [rcx+0x10]; mov rax, [rax+0x10]; mov rax, [rax+8]; "
             f"mov r11, {region + owner_site}; call r11; add rsp, 0x38; pop r12; pop rsi; ret"))
PRESCAN = ctypes.CFUNCTYPE(ctypes.c_uint32, ctypes.c_void_p, ctypes.c_void_p)(pre)
for i in range(2):
    put(sq["base"] + i*0x48 + 0x28, struct.pack("<II", 12, 12 if i == 0 else 5))
SET(sq["ai"], 1, 1)
check("disabled second slot is skipped by the real loop", PRESCAN(men[0]["ai"], sq["base"]) == 0)
check("other soldier's second slot remains eligible", PRESCAN(men[1]["ai"], sq["base"]) == 1)
SET(sq["ai"], 1, 0)
check("re-enabled second slot is eligible again", PRESCAN(men[0]["ai"], sq["base"]) == 1)
facets = q(men[0]["entity"] + 8)
put(facets + 0x50, bytes(8))
check("missing member selectable uses stock resupply", PRESCAN(men[0]["ai"], sq["base"]) == 1)
put(facets + 0x50, struct.pack("<Q", men[0]["facet"]))


print("\n== resupply selection loop and invalid indices\n")
put(region + 0x115a93, asm("mov eax, 1; ret"))
put(region + 0x115c0f, asm("xor eax, eax; ret"))
select = scratch(0x100)
put(region + 0x1158b1, asm("mov rsi, [rsi+8]; mov ebp, r8d; "
                         f"mov rax, {block + labels['ammo_reader_check2']}; jmp rax"))
put(select, asm("push rbp; push rsi; sub rsp, 0x38; mov rax, [rcx+0x10]; mov rax, [rax+0x10]; mov rax, [rax+8]; "
                f"mov r11, {region + owner_site}; call r11; "
                "add rsp, 0x38; pop rsi; pop rbp; ret"))
SELECT_SLOT = ctypes.CFUNCTYPE(ctypes.c_uint32, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32)(select)
SET(sq["ai"], 1, 1)
check("disabled slot skips the second loop", SELECT_SLOT(men[0]["ai"], sq["base"] + 0x48, 1) == 0)
check("unmarked soldier still resupplies", SELECT_SLOT(men[1]["ai"], sq["base"] + 0x48, 1) == 1)
SET(sq["ai"], 1, 0)
check("re-enabled slot enters the second loop", SELECT_SLOT(men[0]["ai"], sq["base"] + 0x48, 1) == 1)
alone = soldier(SOLDIER_VT)
soldier_of(alone["ai"], alone["entity"])
before = ctypes.string_at(alone["base"], 0x80)
for invalid in (1, 8, 0xffffffff, 0xffffffffffffffff):
    SET(alone["ai"], invalid, 1)
check("invalid writes leave the allocation untouched", ctypes.string_at(alone["base"], 0x80) == before)

print("\n== both factory paths reset recycled ammo pins\n")
for label, resume in (("ammo_init_new", 0x25c979), ("ammo_init_load", 0x25cac3)):
    put(region + resume, asm("ret"))
    facet = obj(0x40)
    put(facet, bytes([0xa5]) * 0x38)
    t = scratch(0x80)
    put(t, asm("push rdi; sub rsp, 0x20; mov rdi, rcx; "
               f"mov rax, {block + labels[label]}; call rax; add rsp, 0x20; pop rdi; ret"))
    ctypes.CFUNCTYPE(None, ctypes.c_void_p)(t)(facet)
    check(f"{label}: no old pins survive", byte(facet + 0x19) == 0 and byte(facet + 0x1e) == 0 and byte(facet + 0x1f) == 0)
    check(f"{label}: native parent initialization is replayed", q(facet + 0x28) == 0)
    check(f"{label}: unrelated fields unchanged", byte(facet + 0x18) == 0xa5 and byte(facet + 0x30) == 0xa5)

# Original HumanAiFacet/HumanGunner accessors and Gun release/arm/choose.
for start, end in ((0x9f060, 0x9f078), (0x2aa4a0, 0x2aa4c1),
                   (0x296160, 0x29616d), (0x2cb6d0, 0x2cb6ec),
                   (0x1be5e0, 0x1be600), (0x28a440, 0x28a561),
                   (0x28abf0, 0x28ad3a), (0x117680, 0x1176d2),
                   (0x117790, 0x1177e5), (0x28a900, 0x28a96f)):
    put(region + start, stock.read(start, end - start))
# Chooser's owner classification: the fixture is a normal human.
put(region + 0x10caa0, asm("xor eax, eax; ret"))
for site, displaced, label in b.AMMO_GATE_CALLS:
    if site in (0x28a46d, 0x28ac93):
        put(region + site, b"\xe8" + struct.pack("<i", labels[label] - site - 5))
put(FIRE_DATA_VT + 0x88, struct.pack("<Q", region + 0x117680))
put(FIRE_DATA_VT + 0x90, struct.pack("<Q", region + 0x117790))
gun_vt = obj(0x150, [(0x148, region + 0x28a900)] + [(0x80, block + labels["gun_ready"]),
                     (0xf0, region + 0x28a4d0), (0xf8, region + 0x28a510)])
gunner_vt = obj(0x100, [(0xd0, region + 0x296160), (0xe0, region + 0x2cb6d0),
                        (0xf0, region + 0x1be5e0), (0xf8, region + 0x1be5f0)])

def wire_guns(men, guns, weapon_sets):
    ai_vt = obj(0x400)
    put(ai_vt, ctypes.string_at(SOLDIER_VT, 0x400))
    put(ai_vt + 0x120, struct.pack("<Q", region + 0x9f060))
    put(ai_vt + 0x130, struct.pack("<Q", region + 0x2aa4a0))
    for m, gun, allowed in zip(men, guns, weapon_sets):
        candidates = obj(max(0x10, len(allowed) * 0x10), [(i*0x10, w) for i, w in enumerate(allowed)])
        weapon_list = obj(0x240, [(0x210, candidates), (0x218, candidates + len(allowed)*0x10)])
        put(gun, struct.pack("<Q", gun_vt))
        put(gun + 0x40, struct.pack("<Q", weapon_list))
        gun_vector = obj(8, [(0, gun)])
        gunner = obj(0x48, [(0, gunner_vt), (0x38, gun_vector), (0x40, gun_vector + 8)])
        put(m["ai"], struct.pack("<Q", ai_vt))
        put(m["ai"] + 0x1f0, struct.pack("<Q", obj(0x18, [(0x10, gunner)])))

print("\n== original ammunition-menu draw and click consumers\n")
# Execute the original click handler, fillSlot's flag/icon branch, and the
# actual hook site. Only rendering and entity lookup are replaced by sinks.
game = Image("bin/game.orig.dll")
game_region = scratch(0x600000)
ui_code, ui_labels = b.assemble(pathlib.Path("patch/icon-squad.asm").read_text().splitlines(), 0, 0x800)
ui_block = scratch(0x1000)
put(ui_block, ui_code)
for placeholder, target in ((0xaaaaaaaaaaaaaaae, 0x3f079), (0xaaaaaaaaaaaaaaaf, 0x3f1d0), (0xaaaaaaaaaaaaaab0, 0x3f800), (0xaaaaaaaaaaaaaab1, 0x2cb730)):
    put(ui_block + ui_code.index(struct.pack("<Q", placeholder)), struct.pack("<Q", game_region + target))
put(game_region + 0x3f06b, game.read(0x3f06b, 14))
put(game_region + 0x3f079, asm("add rsp, 0x20; pop r12; pop rdi; pop rbp; pop rsi; pop rbx; ret"))
put(game_region + 0x3f1d0, game.read(0x3f1d0, 0xec))
# Preserve the native text-color branch before the test epilogue, so checks
# exercise its normal/empty color restoration rather than a renderer mock.
put(game_region + 0x3f2bc, b"\xe9" + struct.pack("<i", 0x3f50a - 0x3f2bc - 5))
put(game_region + 0x3f50a, game.read(0x3f50a, 0xee))
put(game_region + 0x526c58, game.read(0x526c58, 4))
put(game_region + 0x3f5f8, asm("mov rbx, [rsp+0x1d8]; mov rsi, [rsp+0x1e0]; "
                             "add rsp, 0x1a0; pop r15; pop r14; pop r12; pop rdi; pop rbp; ret"))
# fillSlot passes the icon pointer and flag to the slot renderer, which caches
# flag at +8 (native 3b7ff). Capture both; no graphics/string subsystem needed.
put(game_region + 0x3b750, asm("mov [rcx+8], edx; mov [rcx+0x10], r8; "
                             "mov rax, [rcx+0x18]; mov byte ptr [rax+0x5b], 1; "
                             "mov rdx, [rax+0x80]; mov dword ptr [rdx], 0x6f6d6d41; mov byte ptr [rdx+4], 0; "
                             "mov qword ptr [rax+0x90], 4; mov rax, [rcx+0x40]; test rax, rax; jz rendered; "
                             "mov rdx, [rax+0x1a8]; mov word ptr [rdx], 0x32; mov qword ptr [rax+0x1b8], 1; rendered: ret"))
put(game_region + 0x3f800, game.read(0x3f800, 0x93))
# Execute the actual text-label setter. Its standard-library assign/memcmp
# dependencies use preallocated string storage in the UI fixture.
put(game_region + 0x2cb730, game.read(0x2cb730, 0x90))
put(game_region + 0x33740, asm("push rdi; push rsi; mov rax, rcx; mov [rcx+0x10], r8; "
                             "mov rdi, [rcx]; mov rsi, rdx; mov rcx, r8; rep movsb; "
                             "mov byte ptr [rdi], 0; pop rsi; pop rdi; ret"))
memcmp = ctypes.cast(ctypes.CDLL("msvcrt").memcmp, ctypes.c_void_p).value
put(game_region + 0x49a49c, asm(f"mov rax, {memcmp}; jmp rax"))

put(game_region + 0x3f8a0, game.read(0x3f8a0, 0x1c5))
put(game_region + 0x3b9f0, asm("xor eax, eax; ret"))
put(game_region + 0x40a80, asm("mov rax, [rcx+8]; ret"))
ui_update = scratch(0x100)
put(ui_update, asm("push rbx; push rsi; push rbp; push rdi; push r12; sub rsp, 0x20; "
                   "mov rdi, rcx; mov r12, [rcx+8]; mov rsi, [rcx+0x10]; mov rbp, [rcx+0x28]; "
                   f"mov r11, {game_region + 0x3f06b}; jmp r11"))
put(AI_VT + 0x3e8, struct.pack("<Q", block + labels["ammo_get"]))
put(AI_VT + 0x3e0, struct.pack("<Q", block + labels["ammo_set"]))
men, sq, weapons, guns = gun_case()
wire_guns(men, guns, [weapons[:2], [weapons[0], weapons[2]]])
squad_entity = q(q(sq["ai"] + 0x10) + 0x10)
menu = obj(0x900, [(0, obj(0x40, [(0x38, ui_update)])), (8, squad_entity),
                   (0x10, sq["base"] + 0x48), (0x28, 1)])
ui_slot = menu + 0x180 + 0xb8
visible = scratch(0x30)
put(visible, asm("mov [rcx+0x5b], dl; ret"))
control_vt = obj(0x50, [(0x48, visible)])
def ui_control():
    return obj(0xc0, [(0, control_vt), (0x80, obj(128)), (0x98, 127)])
def ui_label():
    return obj(0x200, [(0x1a8, obj(128)), (0x1c0, 127)])
def ui_text(label):
    return ctypes.string_at(q(label + 0x1a8), q(label + 0x1b8))
control = ui_control()
controls = obj(8, [(0, control)])
put(ui_slot + 0x18, struct.pack("<Q", control))
put(ui_slot + 0x80, struct.pack("<QQ", controls, controls + 8))
DRAW = ctypes.CFUNCTYPE(None, ctypes.c_void_p)(ui_update)
CLICK = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)(game_region + 0x3f8a0)
DRAW(menu)
CLICK(menu, control)
check("baseline: original redraw incorrectly resets a disabled soldier's checkbox", GET(sq["ai"], 1) == 1 and dword(ui_slot + 8) == 0)
CLICK(menu, control)
check("baseline: original next click disables again", GET(sq["ai"], 1) == 1)
put(game_region + 0x3f06b, asm(f"mov r11, {ui_block + ui_labels['ammo_panel']}; jmp r11") + b"\x90")
DRAW(menu)
check("actual fillSlot receives disabled flag and selects disabled icon", dword(ui_slot + 8) == 1 and q(ui_slot + 0x10) == weapons[1] + 0x168)
CLICK(menu, control)
check("actual click re-enables and redraw caches enabled state", GET(sq["ai"], 1) == 0 and dword(ui_slot + 8) == 0)
check("actual fillSlot selects enabled icon", q(ui_slot + 0x10) == weapons[1] + 0x148)
CLICK(menu, control)
check("next click disables again", GET(sq["ai"], 1) == 1 and dword(ui_slot + 8) == 1)
check("menu never writes the shared record", slot(sq["data"], sq["base"], 1) == 0)
put(men[0]["facet"] + 0x30, bytes(1))
put(men[1]["facet"] + 0x30, b"\x01")
DRAW(menu)
check("redraw hides launcher when selecting the sniper", byte(control + 0x5b) == 0)
put(men[0]["facet"] + 0x30, b"\x01")
put(men[1]["facet"] + 0x30, bytes(1))
DRAW(menu)
check("returning to the grenadier restores his disabled checkbox", dword(ui_slot + 8) == 1 and byte(control + 0x5b) == 1)

print("\n== weapon-user state aggregation and original squad clicks\n")
STATE = ctypes.CFUNCTYPE(ctypes.c_uint32, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint64)(ui_block + ui_labels["ammo_ui_state"])
def state(index):
    return STATE(squad_entity, sq["base"] + index*0x48, index)
put(men[1]["facet"] + 0x30, b"\x01")
DRAW(menu)
check("squad shows sole launcher user disabled despite shared enabled default", state(1) == 2 and dword(ui_slot + 8) == 1 and slot(sq["data"], sq["base"], 1) == 0)
CLICK(menu, control)
check("original squad click re-enables the sole launcher user", state(1) == 1 and dword(ui_slot + 8) == 0 and not (byte(men[0]["facet"] + 0x1e) & 2))
put(men[1]["facet"] + 0x30, bytes(1))
SET(sq["ai"], 1, 1)
SET(sq["ai"], 0, 1)
put(men[1]["facet"] + 0x30, b"\x01")
check("two rifle users with opposite pins/defaults aggregate as mixed", state(0) == 3)
put(menu + 0x10, struct.pack("<Q", sq["base"]))
put(menu + 0x28, bytes(8))
rifle_slot = menu + 0x180
rifle_control = ui_control()
rifle_label = ui_label()
rifle_quantity = ui_label()
put(rifle_slot + 0x30, struct.pack("<Q", rifle_quantity))
put(rifle_slot + 0x40, struct.pack("<Q", rifle_label))
put(rifle_slot + 0x98, struct.pack("<ffff", 0.75, 0.75, 0.75, 1.0))
put(rifle_slot + 0xa8, struct.pack("<ffff", 1.0, 0.2, 0.2, 1.0))
rifle_controls = obj(8, [(0, rifle_control)])
put(rifle_slot + 0x18, struct.pack("<Q", rifle_control))
put(rifle_slot + 0x80, struct.pack("<QQ", rifle_controls, rifle_controls + 8))
DRAW(menu)
check("mixed keeps weapon artwork and shows enabled/selected", ui_text(rifle_label) == b"1/2" and q(rifle_slot + 0x10) == weapons[0] + 0x148)
check("mixed preserves the native tooltip resource key", ctypes.string_at(q(rifle_control + 0x80), q(rifle_control + 0x90)) == b"Ammo")
check("mixed ammo quantity is amber and marked dirty", dword(rifle_quantity + 0x1a0) == 0xffffc04d and byte(rifle_quantity + 0x188) == 1)
check("native text setter marks the mixed label dirty", byte(rifle_label + 0x188) == 1)
CLICK(menu, rifle_control)
check("clicking mixed enables all users and clears their slot overrides", state(0) == 1 and dword(rifle_slot + 8) == 0 and all(not (byte(m["facet"] + 0x1e) & 1) for m in men))
check("uniform redraw restores ordinary count and tooltip", ui_text(rifle_label) == b"2" and ctypes.string_at(q(rifle_control + 0x80), q(rifle_control + 0x90)) == b"Ammo")
check("native redraw restores the ordinary quantity color", dword(rifle_quantity + 0x1a0) == 0xffbfbfbf)
put(men[1]["facet"] + 0x30, bytes(1))
SET(sq["ai"], 0, 1)
put(men[1]["facet"] + 0x30, b"\x01")
put(sq["base"] + 0x2c, bytes(4))
DRAW(menu)
check("empty mixed slot still shows amber", dword(rifle_quantity + 0x1a0) == 0xffffc04d)
CLICK(menu, rifle_control)
check("native redraw restores empty-ammo color after mixed resolves", dword(rifle_quantity + 0x1a0) == 0xffff3333)
put(sq["base"] + 0x2c, struct.pack("<I", 12))
DRAW(menu)
check("whole rifle toggle preserves the individual launcher override", byte(men[0]["facet"] + 0x1e) & 2 and state(1) == 2)
CLICK(menu, rifle_control)
check("next squad click disables every rifle user", state(0) == 2 and dword(rifle_slot + 8) == 1)
CLICK(menu, rifle_control)
check("uniform off click enables every rifle user", state(0) == 1 and dword(rifle_slot + 8) == 0)
# A personal on pin can override the shared off default for the sole user.
SET(sq["ai"], 1, 1)
put(men[1]["facet"] + 0x30, bytes(1))
SET(sq["ai"], 1, 0)
put(men[1]["facet"] + 0x30, b"\x01")
check("sole user's explicit on overrides squad off for display", state(1) == 1 and slot(sq["data"], sq["base"], 1) == 1)
check("unsupported weapons do not acquire an aggregate state", state(3) == 0)
put(men[0]["facet"] + 0x30, bytes(1))
put(men[1]["facet"] + 0x30, bytes(1))
check("no marks uses the same actual-user aggregation", state(1) == 1)
check("non-squad state retains native shared flag", STATE(men[0]["entity"], sq["base"] + 0x48, 1) == 2)
SET(sq["ai"], 1, 0)
put(men[0]["facet"] + 0x30, b"\x01")
SET(sq["ai"], 1, 1)
put(menu + 0x10, struct.pack("<Q", sq["base"] + 0x48))
put(menu + 0x28, struct.pack("<Q", 1))
DRAW(menu)

print("\n== selection-specific weapon slot visibility\n")
USABLE = ctypes.CFUNCTYPE(ctypes.c_uint32, ctypes.c_void_p, ctypes.c_void_p)(ui_block + ui_labels["ammo_ui_usable"])
def shown():
    return [bool(USABLE(squad_entity, w)) for w in weapons]
check("grenadier sees rifle and launcher, not sniper or unrelated ammo", shown() == [True, True, False, False])
# A disabled alternative remains present even when a different weapon is active.
put(guns[0] + 0x50, struct.pack("<Q", weapons[0]))
put(sq["base"] + 0x48 + 0x2c, bytes(4))
check("disabled empty launcher stays visible while rifle is current", USABLE(squad_entity, weapons[1]) == 1 and GET(sq["ai"], 1) == 1)
put(men[0]["facet"] + 0x30, bytes(1))
put(men[1]["facet"] + 0x30, b"\x01")
check("sniper sees rifle and sniper, not launcher", shown() == [True, False, True, False])
put(men[0]["facet"] + 0x30, b"\x01")
check("whole selection shows union of both loadouts", shown() == [True, True, True, False])
put(men[0]["facet"] + 0x30, bytes(1))
put(men[1]["facet"] + 0x30, bytes(1))
check("no individual marks follows whole-squad recipients", shown() == [True, True, True, False])
put(men[0]["facet"] + 0x30, b"\x01")
put(men[0]["facet"] + 0x18, bytes(1))
check("unselectable member does not contribute equipment", shown() == [True, False, True, False])
put(men[0]["facet"] + 0x18, b"\x01")
parent = q(men[0]["facet"] + 0x28)
put(men[0]["facet"] + 0x28, bytes(8))
check("stale member of a different squad is ignored", shown() == [True, False,True, False])
put(men[0]["facet"] + 0x28, struct.pack("<Q", parent))
check("non-squad panels retain native visibility", USABLE(men[0]["entity"], weapons[2]) == 1)
# Keep native row indices: the visible sniper row still toggles record 2.
put(men[0]["facet"] + 0x30, bytes(1))
put(men[1]["facet"] + 0x30, b"\x01")
put(menu + 0x10, struct.pack("<Q", sq["base"] + 2*0x48))
put(menu + 0x28, struct.pack("<Q", 2))
sniper_slot = menu + 0x180 + 2*0xb8
sniper_control = ui_control()
sniper_controls = obj(8, [(0, sniper_control)])
put(sniper_slot + 0x18, struct.pack("<Q", sniper_control))
put(sniper_slot + 0x80, struct.pack("<QQ", sniper_controls, sniper_controls + 8))
DRAW(menu)
CLICK(menu, sniper_control)
check("visible sniper row toggles its original slot index", GET(sq["ai"], 2) == 1 and dword(sniper_slot + 8) == 1 and byte(sniper_control + 0x5b) == 1)
check("filter does not alter shared flags", [slot(sq["data"], sq["base"], i) for i in range(4)] == [0]*4)
# Include later guns; compatibility must not stop at the first gun/current type.
gunner = q(q(men[1]["ai"] + 0x1f0) + 0x10)
extra = obj(0x150)
put(extra, ctypes.string_at(guns[0], 0x150))
put(extra + 0x18, struct.pack("<Q", q(guns[1] + 0x18)))
gun_vector = obj(0x18, [(0, 0), (8, guns[1]), (0x10, extra)])
put(gunner + 0x38, struct.pack("<QQ", gun_vector, gun_vector + 0x18))
check("all equipped guns are considered, including after a null entry", shown() == [True, True, True, False])
# Two selected specialists within a larger squad must exclude the third man's
# unique weapon; this differs from the all-marked whole-squad case above.
trio = [soldier(SOLDIER_VT, marked=m) for m in (True, True, False)]
trio_sq = squad(trio)
trio_entity = q(q(trio_sq["ai"] + 0x10) + 0x10)
trio_guns = [obj(0x150, [(0x18, obj(0x18, [(0x10, m["entity"])]))]) for m in trio]
wire_guns(trio, trio_guns, [weapons[:2], [weapons[0], weapons[2]], [weapons[3]]])
check("multi-member subset excludes an unselected specialist's unique weapon",
      [bool(USABLE(trio_entity, w)) for w in weapons] == [True, True, True, False])
put(trio[2]["facet"] + 0x30, b"\x01")
check("adding the third specialist reveals his weapon",
      [bool(USABLE(trio_entity, w)) for w in weapons] == [True]*4)

# Mixed clicks on a subset must preserve an unselected owner's personal off.
wire_guns(trio, trio_guns, [weapons[:2], [weapons[0], weapons[2]], [weapons[0], weapons[3]]])
put(trio_sq["base"], struct.pack("<Q", weapons[0]))
for m in trio:
    put(m["facet"] + 0x30, bytes(1))
put(trio[0]["facet"] + 0x30, b"\x01")
SET(trio_sq["ai"], 0, 1)
put(trio[0]["facet"] + 0x30, bytes(1))
put(trio[2]["facet"] + 0x30, b"\x01")
SET(trio_sq["ai"], 0, 1)
put(trio[2]["facet"] + 0x30, bytes(1))
put(trio[0]["facet"] + 0x30, b"\x01")
put(trio[1]["facet"] + 0x30, b"\x01")
put(menu + 8, struct.pack("<Q", trio_entity))
put(menu + 0x10, struct.pack("<Q", trio_sq["base"]))
put(menu + 0x28, bytes(8))
DRAW(menu)
check("two-member mixed subset displays enabled/selected", ui_text(rifle_label) == b"1/2" and STATE(trio_entity, trio_sq["base"], 0) == 3)
CLICK(menu, rifle_control)
check("mixed subset click enables only selected users", STATE(trio_entity, trio_sq["base"], 0) == 1 and byte(trio[2]["facet"] + 0x1f) & 1 and slot(trio_sq["data"], trio_sq["base"]) == 0)
check("uniform subset label reports two selected users", ui_text(rifle_label) == b"2")
put(trio[2]["facet"] + 0x30, b"\x01")
DRAW(menu)
check("mixed whole squad shows two enabled of three users", ui_text(rifle_label) == b"2/3")
# Count soldiers, not compatible guns, and continue past the first disagreement.
gunner = q(q(trio[1]["ai"] + 0x1f0) + 0x10)
second_gun = obj(0x150)
put(second_gun, ctypes.string_at(trio_guns[1], 0x150))
extra_guns = obj(16, [(0, trio_guns[1]), (8, second_gun)])
put(gunner + 0x38, struct.pack("<QQ", extra_guns, extra_guns + 16))
DRAW(menu)
check("two compatible guns on one soldier still count him once", ui_text(rifle_label) == b"2/3")
CLICK(menu, rifle_control)
check("uniform whole squad shows three selected users", ui_text(rifle_label) == b"3")
put(trio[1]["facet"] + 0x30, bytes(1))
put(trio[2]["facet"] + 0x30, bytes(1))
DRAW(menu)
check("single selected user is explicitly shown as one", ui_text(rifle_label) == b"1")
check("selection count never mutates shared weapon count", dword(trio_sq["base"] + 0x34) == 0)
put(trio_sq["base"] + 0x34, struct.pack("<I", 5))
put(menu + 8, struct.pack("<Q", trio[0]["entity"]))
DRAW(menu)
check("non-squad panel retains its native count", ui_text(rifle_label) == b"5")

# The decimal helper must handle more than one digit without buffer overwrite.
number_buf = obj(32)
number_call = scratch(0x100)
put(number_call, asm("sub rsp, 0x28; mov eax, ecx; "
                     f"mov r10, {number_buf}; mov r11, {ui_block + ui_labels['ammo_ui_number']}; call r11; "
                     "mov byte ptr [r10], 0; add rsp, 0x28; ret"))
NUMBER = ctypes.CFUNCTYPE(None, ctypes.c_uint32)(number_call)
for value in (0, 1, 12, 100, 65535, 0xffffffff):
    put(number_buf, bytes([0xa5])*32)
    NUMBER(value)
    check(f"decimal count {value} fits its buffer", ctypes.string_at(number_buf) == str(value).encode() and byte(number_buf + 11) == 0xa5)


print("\n== native gun enumeration, release, weapon choice, and reserve\n")
men, sq, weapons, guns = gun_case()
# Rifle is the first compatible candidate; launcher is preferred later.
put(weapons[0] + 0x30, struct.pack("<Q", 1))
put(weapons[1] + 0x30, struct.pack("<Q", 1))
candidates = obj(0x20, [(0, weapons[0]), (0x10, weapons[1])])
weapon_list = obj(0x240, [(0x210, candidates), (0x218, candidates + 0x20)])
for gun in guns:
    put(gun, struct.pack("<Q", gun_vt))
    put(gun + 0x40, struct.pack("<Q", weapon_list))
# Two loaded guns reserve two rounds each from the shared launcher record.
put(sq["base"] + 0x48 + 0x30, struct.pack("<I", 4))
put(sq["base"] + 0x30, bytes(4))
CHOOSE = ctypes.CFUNCTYPE(ctypes.c_uint32, ctypes.c_void_p, ctypes.c_uint32)(region + 0x28abf0)
SET(sq["ai"], 1, 1)   # no enumerator yet: reproduces previous loaded-query patch
CHOOSE(guns[0], 1)
check("baseline: gate alone still lets native chooser retain loaded launcher", q(guns[0] + 0x50) == weapons[1])
SET(sq["ai"], 1, 0)
# Use native accessors with their actual layouts, not a test-only gun map.
wire_guns(men, guns, [weapons[:2], weapons[:2]])
SET(sq["ai"], 1, 1)
check("toggle unloads selected gun through native release", dword(guns[0] + 0xdc) == 0)
check("unmarked soldier retains his loaded launcher", dword(guns[1] + 0xdc) == 2)
check("release returns reservation without consuming ammunition", dword(sq["base"] + 0x48 + 0x30) == 2 and dword(sq["base"] + 0x48 + 0x2c) == 12)
CHOOSE(guns[0], 1)
check("native chooser falls back to rifle and arms it", q(guns[0] + 0x50) == weapons[0] and dword(guns[0] + 0xdc) == 1)
check("native reserve accounts for rifle round", dword(sq["base"] + 0x30) == 1)
SET(sq["ai"], 1, 0)
CHOOSE(guns[0], 1)
check("re-enabling lets native chooser select and arm launcher again", q(guns[0] + 0x50) == weapons[1] and dword(guns[0] + 0xdc) == 1)
check("switch releases rifle and reserves launcher without losing ammo", dword(sq["base"] + 0x30) == 0 and dword(sq["base"] + 0x48 + 0x30) == 3 and dword(sq["base"] + 0x48 + 0x2c) == 12)
SET(sq["ai"], 1, 1)
CHOOSE(guns[0], 1)
check("second disable switches back to rifle", q(guns[0] + 0x50) == weapons[0] and dword(guns[0] + 0xdc) == 1)
SET(sq["ai"], 0, 1)
CHOOSE(guns[0], 1)
check("disabling both leaves no loaded round", dword(guns[0] + 0xdc) == 0)
SET(sq["ai"], 1, 0)
CHOOSE(guns[0], 1)
check("enabling launcher recovers from both disabled", q(guns[0] + 0x50) == weapons[1] and dword(guns[0] + 0xdc) == 1)


print()
print(f"{failures} failed" if failures else "all cases as expected")
sys.exit(1 if failures else 0)
