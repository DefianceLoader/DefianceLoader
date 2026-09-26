"""Run the stock and patched choosers on fabricated squads and compare.

Neither function touches anything outside the objects it is handed and
neither is position dependent, so both can be copied into scratch memory and
called directly. Members and guns are fakes whose virtual methods are
three-instruction stubs reading fields the test plants.

The patched chooser keeps its rotation cursor at a fixed distance from its
own code, so the code is placed at the start of one large allocation and the
cursor lands inside it; the test reads and writes that dword directly.
"""
import ctypes, json, pathlib, sys
import builds
from ctypes import wintypes
import keystone
sys.path.insert(0, "tools")
from pe import Image
import build as cfg

ks = keystone.Ks(keystone.KS_ARCH_X86, keystone.KS_MODE_64)
asm = lambda text: bytes(ks.asm(text.encode())[0])

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
kernel32.VirtualAlloc.restype = ctypes.c_void_p
kernel32.VirtualAlloc.argtypes = [ctypes.c_void_p, ctypes.c_size_t, wintypes.DWORD, wintypes.DWORD]
MEM_COMMIT_RESERVE, PAGE_RWX = 0x3000, 0x40

HEAP = []


def scratch(size):
    # an empty members array still needs a valid address to point at
    addr = kernel32.VirtualAlloc(None, max(size, 8), MEM_COMMIT_RESERVE, PAGE_RWX)
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


# --- the stubs the fake vtables point at -----------------------------------
STUB_SLOT_TYPE = blob(asm("mov eax, dword ptr [rcx + 0x100]; ret"))   # member -> held slot type
STUB_MAN = blob(asm("mov rax, qword ptr [rcx + 0x108]; ret"))         # member -> the man
STUB_ITEM = blob(asm("mov rax, qword ptr [rcx + 0x10]; ret"))         # gun -> its item info
STUB_FACETS = blob(asm("mov rax, qword ptr [rcx + 0x110]; ret"))      # entity -> its facets
STUB_SELECTED = blob(asm("movzx eax, byte ptr [rcx + 0x10]; ret"))    # selectable -> is it marked

MEMBER_SIZE, SQUAD_SIZE = 0x120, 0x300


def poke(addr, offset, value, width=8):
    ctypes.memmove(addr + offset, int(value).to_bytes(width, "little"), width)


def peek(addr, offset=0, width=4):
    buf = (ctypes.c_char * width).from_address(addr + offset)
    return int.from_bytes(buf.raw, "little")


def member(slot_type, gun_obj=0, marked=None):
    """A squad member. `marked` is what his selectable facet reports: None
    gives him no facet at all, which is how a member with nothing selectable
    behaves."""
    vt = [0] * 64
    vt[0xc8 // 8] = STUB_MAN
    vt[0xb0 // 8] = STUB_FACETS
    vt[0x180 // 8] = STUB_SLOT_TYPE
    obj = blob(bytes(bytearray(MEMBER_SIZE)), MEMBER_SIZE)
    poke(obj, 0, words(vt))
    poke(obj, 0x28, gun_obj)
    poke(obj, 0x100, slot_type, 4)
    poke(obj, 0x108, obj)          # a member stands in for its own man
    if marked is not None:
        selectable_vt = [0] * 32
        selectable_vt[0x58 // 8] = STUB_SELECTED
        selectable = blob(bytes(bytearray(0x20)), 0x20)
        poke(selectable, 0, words(selectable_vt))
        poke(selectable, 0x10, 1 if marked else 0, 1)
        facets = blob(bytes(bytearray(0x60)), 0x60)
        poke(facets, 0x50, selectable)
        poke(obj, 0x110, facets)
    return obj


def gun(item_info):
    vt = [0] * 64
    vt[0x160 // 8] = STUB_ITEM
    obj = blob(bytes(bytearray(0x40)), 0x40)
    poke(obj, 0, words(vt))
    poke(obj, 0x10, item_info)
    return obj


def squad(members, slot_type, used, maximum, no_pickup=False):
    info = blob(bytes(bytearray(0x200)), 0x200)
    ctypes.memmove(info + 0x1a9, bytes([1 if no_pickup else 0]), 1)
    array = words(members)
    obj = blob(bytes(bytearray(SQUAD_SIZE)), SQUAD_SIZE)
    poke(obj, 0x1e8, array)
    poke(obj, 0x1f0, array + 8 * len(members))
    poke(obj, 0x240, info)
    poke(obj, 0x260 + slot_type * 8, used, 4)
    poke(obj, 0x264 + slot_type * 8, maximum, 4)
    return obj


# --- the two functions under test -------------------------------------------
PROTO = ctypes.CFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int)

stock_img = Image(str(builds.reference().logic))
call_stock = PROTO(blob(stock_img.read(0x43e0f0, 0x43e23b - 0x43e0f0)))

# the patched chooser, with room for the cursor its code reaches past itself
patched_img = Image("out/logic.dll")
# Everything about the block comes from the build rather than from constants:
# the chooser has already moved once, from .text's padding to a section of its
# own, and a hardcoded length silently copied half of it and crashed the test.
built = json.loads(pathlib.Path("out/manifest.json").read_text())
CURSOR_DELTA = built["cursor_rva"] - built["code_rva"]
region = scratch(CURSOR_DELTA + 0x1000)
code = patched_img.read(built["code_rva"], built["code_bytes"])
ctypes.memmove(region, code, len(code))
call_patched = PROTO(region)
CURSOR = region + CURSOR_DELTA

TYPE = 7
failures = 0


def check(label, got, want, shown=None):
    global failures
    ok = got == want
    if not ok:
        failures += 1
    show = shown or (lambda v: hex(v) if v else "NULL")
    print(f"{'PASS' if ok else 'FAIL'}  {label}")
    if not ok:
        print(f"        wanted {show(want)}, got {show(got)}")


def namer(members):
    names = {m: f"member{i}" for i, m in enumerate(members)}
    return lambda v: names.get(v, "NULL" if not v else hex(v))


def reset_cursor(value=0):
    poke(CURSOR, 0, value, 4)


def main():
    global failures
    failures = 0
    print("== the reported fault: two members, both slots of the wanted type\n")
    m0, m1 = member(TYPE, gun(0xA)), member(TYPE, gun(0xB))
    sq = squad([m0, m1], TYPE, used=2, maximum=2)
    show = namer([m0, m1])
    stock = [call_stock(sq, TYPE, 1) or 0 for _ in range(4)]
    print(f"      stock, four clicks:   {' '.join(show(v) for v in stock)}")
    check("the stock chooser always answers the same member",
          stock, [m0] * 4, lambda v: " ".join(show(x) for x in v))

    reset_cursor()
    got = [call_patched(sq, TYPE, 1) or 0 for _ in range(4)]
    print(f"      patched, four clicks: {' '.join(show(v) for v in got)}")
    check("the patched chooser alternates, so clicking again sends the other one",
          got, [m0, m1, m0, m1], lambda v: " ".join(show(x) for x in v))

    print("\n== the rest of the behaviour is unchanged\n")

    m0, m1, m2 = member(TYPE, gun(0xA)), member(3, gun(0xB)), member(TYPE, gun(0xC))
    sq = squad([m0, m1, m2], TYPE, used=2, maximum=2)
    show = namer([m0, m1, m2])
    reset_cursor()
    got = [call_patched(sq, TYPE, 1) or 0 for _ in range(4)]
    print(f"      three members, the middle one a different slot: {' '.join(show(v) for v in got)}")
    check("only the matching members take turns", got, [m0, m2, m0, m2],
          lambda v: " ".join(show(x) for x in v))

    m0, m1 = member(TYPE, gun(0xA)), member(0)
    sq = squad([m0, m1], TYPE, used=1, maximum=2)
    show = namer([m0, m1])
    reset_cursor(1)
    before = peek(CURSOR)
    check("a free slot still goes to the member holding nothing",
          call_patched(sq, TYPE, 1) or 0, m1, show)
    check("and that path leaves the cursor alone", peek(CURSOR), before)

    m0, m1 = member(TYPE, gun(0xA)), member(TYPE, gun(0xB))
    sq = squad([m0, m1], TYPE, used=2, maximum=2, no_pickup=True)
    reset_cursor()
    check("noPickupGun answers nobody", call_patched(sq, TYPE, 1) or 0, 0)

    sq = squad([m0, m1], TYPE, used=2, maximum=2)
    reset_cursor()
    check("no free slot and swapping not allowed answers nobody",
          call_patched(sq, TYPE, 0) or 0, 0)

    only = member(TYPE, gun(0xA))
    sq = squad([only], TYPE, used=1, maximum=1)
    reset_cursor()
    got = [call_patched(sq, TYPE, 1) or 0 for _ in range(3)]
    check("a single matching member is chosen every time", got, [only] * 3,
          lambda v: " ".join(namer([only])(x) for x in v))

    m0, m1 = member(3, gun(0xA)), member(4, gun(0xB))
    sq = squad([m0, m1], TYPE, used=1, maximum=1)
    reset_cursor()
    before = peek(CURSOR)
    check("a squad with no slot of this type answers nobody",
          call_patched(sq, TYPE, 1) or 0, 0)
    check("and the cursor does not move", peek(CURSOR), before)

    sq = squad([], TYPE, used=0, maximum=2)
    reset_cursor()
    check("an empty squad answers nobody", call_patched(sq, TYPE, 1) or 0, 0)

    print("\n== a marked soldier takes it, when the weapon can go to him\n")

    m0, m1 = member(TYPE, gun(0xA), marked=False), member(TYPE, gun(0xB), marked=True)
    sq = squad([m0, m1], TYPE, used=2, maximum=2)
    show = namer([m0, m1])
    reset_cursor()
    got = [call_patched(sq, TYPE, 1) or 0 for _ in range(3)]
    print(f"      the second is marked, three clicks: {' '.join(show(v) for v in got)}")
    check("the mark wins every time, instead of taking turns", got, [m1] * 3,
          lambda v: " ".join(show(x) for x in v))
    check("and the cursor never moves, so unmarking resumes where it was",
          peek(CURSOR), 0)

    m0, m1 = member(TYPE, gun(0xA)), member(3, gun(0xB), marked=True)
    sq = squad([m0, m1], TYPE, used=2, maximum=2)
    show = namer([m0, m1])
    reset_cursor()
    check("a marked soldier whose slot is the wrong type is passed over",
          call_patched(sq, TYPE, 1) or 0, m0, show)

    m0, m1 = member(TYPE, gun(0xA)), member(0, marked=True)
    sq = squad([m0, m1], TYPE, used=1, maximum=2)
    show = namer([m0, m1])
    reset_cursor()
    check("a marked soldier carrying nothing takes a free slot",
          call_patched(sq, TYPE, 1) or 0, m1, show)

    m0, m1 = member(TYPE, gun(0xA)), member(0, marked=True)
    sq = squad([m0, m1], TYPE, used=2, maximum=2)
    show = namer([m0, m1])
    reset_cursor()
    check("but not when the squad has no free slot of that type",
          call_patched(sq, TYPE, 1) or 0, m0, show)

    m0, m1 = member(TYPE, gun(0xA)), member(TYPE, gun(0xB), marked=True)
    sq = squad([m0, m1], TYPE, used=2, maximum=2)
    reset_cursor()
    check("a mark does not override the no-swapping rule",
          call_patched(sq, TYPE, 0) or 0, 0)

    m0, m1 = member(TYPE, gun(0xA), marked=True), member(TYPE, gun(0xB), marked=True)
    sq = squad([m0, m1], TYPE, used=2, maximum=2)
    show = namer([m0, m1])
    reset_cursor()
    got = [call_patched(sq, TYPE, 1) or 0 for _ in range(4)]
    print(f"      every candidate marked, four clicks:  {' '.join(show(v) for v in got)}")
    check("marking everyone is no preference, so the rotation takes over",
          got, [m0, m1, m0, m1], lambda v: " ".join(show(x) for x in v))

    m0, m1, m2 = (member(TYPE, gun(0xA), marked=True), member(TYPE, gun(0xB), marked=True),
                  member(TYPE, gun(0xC)))
    sq = squad([m0, m1, m2], TYPE, used=3, maximum=3)
    show = namer([m0, m1, m2])
    reset_cursor()
    check("two of three marked still discriminates, and the first marked one goes",
          call_patched(sq, TYPE, 1) or 0, m0, show)

    m0, m1 = member(3, gun(0xA), marked=True), member(TYPE, gun(0xB), marked=True)
    sq = squad([m0, m1], TYPE, used=2, maximum=2)
    show = namer([m0, m1])
    reset_cursor()
    check("a mark on a member the weapon cannot reach is not a candidate at all",
          call_patched(sq, TYPE, 1) or 0, m1, show)

    m0, m1 = member(TYPE, gun(0xA)), member(TYPE, gun(0xB))
    sq = squad([m0, m1], TYPE, used=2, maximum=2)
    show = namer([m0, m1])
    reset_cursor()
    got = [call_patched(sq, TYPE, 1) or 0 for _ in range(2)]
    check("members with no selectable facet still take turns", got, [m0, m1],
          lambda v: " ".join(show(x) for x in v))

    print()
    print(f"{failures} failed" if failures else "all cases as expected")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
