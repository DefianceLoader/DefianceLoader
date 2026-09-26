"""Run the preview-weapon guard on fabricated stack frames.

Continuations return distinct markers and capture the fallback register.
Native loop fragments below exercise the actual filters, descriptor traversal,
and fallback stores from both supported builds without launching the game.
"""
import ctypes, pathlib, struct, sys
import builds
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


def poke(addr, offset, value, width=8):
    ctypes.memmove(addr + offset, int(value).to_bytes(width, "little"), width)


code, labels = b.assemble(
    pathlib.Path("patch/preview-weapon.asm").read_text().splitlines(), 0, 0)
block = scratch(len(code))
ctypes.memmove(block, code, len(code))

# Distinct module-return stubs restore the nonvolatile registers the cave uses
# (rbp is the frame it writes through, rbx and r13 the provider's locals) and
# then return, so each entry is callable from ctypes on its own.
observed = scratch(8)
for marker, placeholder in enumerate((0xaaaaaaaaaaaaaac1, 0xaaaaaaaaaaaaaac2, 0xaaaaaaaaaaaaaac3), 1):
    RET = blob(asm(f"mov r10, {observed}; mov [r10], r13; mov eax, {marker}; add rsp, 0x20; pop r13; pop rbx; pop rbp; ret"))
    at = code.find(struct.pack("<Q", placeholder))
    assert at >= 0, f"the fixup {placeholder:#x} was not found"
    ctypes.memmove(block + at, struct.pack("<Q", RET), 8)

PROLOGUE = "push rbp; push rbx; push r13; sub rsp, 0x20;"
SECONDARY = blob(asm(f"{PROLOGUE} mov r13, r8; mov rbp, rcx; mov r9, rdx; "
                     f"mov r11, {block + labels['preview_secondary']}; jmp r11"))
PRIMARY = blob(asm(f"{PROLOGUE} mov eax, r9d; mov rbp, rcx; mov r9, rdx; mov rbx, r8; "
                   f"mov r11, {block + labels['preview_primary']}; jmp r11"))
SQUAD = blob(asm(f"{PROLOGUE} mov r11, {block + labels['preview_squad']}; jmp r11"))
CALL_SECONDARY = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p)(SECONDARY)
CALL_PRIMARY = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int)(PRIMARY)
CALL_SQUAD = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)(SQUAD)

failures = 0


def check(label, got, want):
    global failures
    ok = got == want
    if not ok:
        failures += 1
    print(f"{'PASS' if ok else 'FAIL'}  {label}" + ("" if ok else f" (wanted {want:#x}, got {got:#x})"))


def frame():
    base = scratch(0x200)
    return base + 0x100


def slot(rbp, offset):
    return ctypes.c_uint64.from_address(rbp + offset).value


print("== the secondary weapon keeps the first match\n")
rbp = frame()
check("secondary resumes scan", CALL_SECONDARY(rbp, 0xAAAA, 0), 1)
check("first squad fallback", slot(observed, 0), 0xAAAA)
check("the first gun is stored", slot(rbp, -0x71), 0xAAAA)
CALL_SECONDARY(rbp, 0xBBBB, 0xAAAA)
check("later weapon preserves fallback", slot(observed, 0), 0xAAAA)
check("a later gun does not overwrite it", slot(rbp, -0x71), 0xAAAA)
CALL_SECONDARY(frame(), 0xCCCC, 0xAAAA)
check("later soldier preserves squad fallback", slot(observed, 0), 0xAAAA)

print("\n== the primary weapon keeps the first match\n")
holder = scratch(0x100)
poke(holder, 0xa9, 0)
rbp = frame()
check("primary match resumes scan", CALL_PRIMARY(rbp, 0xCCCC, holder, 1), 1)
check("the first matching gun is stored", slot(rbp, -0x69), 0xCCCC)
CALL_PRIMARY(rbp, 0xDDDD, holder, 1)
check("a later matching gun does not overwrite it", slot(rbp, -0x69), 0xCCCC)

print("\n== the squad-management descriptor keeps the first weapon\n")
weapon = scratch(0x200)
desc = scratch(0x40)
poke(weapon, 0xb8, 0xAAAA)
check("squad resumes descriptor loop", CALL_SQUAD(desc, weapon), 3)
check("the first squad weapon is stored", slot(desc, 0x28), 0xAAAA)
poke(weapon, 0xb8, 0xBBBB)
CALL_SQUAD(desc, weapon)
check("a later squad weapon does not overwrite it", slot(desc, 0x28), 0xAAAA)
fresh = scratch(0x40)
CALL_SQUAD(fresh, weapon)
check("a fresh descriptor still takes its first weapon", slot(fresh, 0x28), 0xBBBB)

print("\n== the primary entry reproduces the filter it displaced\n")
rbp = frame()
check("lookup zero with flag clear continues secondary filter", CALL_PRIMARY(rbp, 0xEEEE, holder, 0), 2)
check("lookup zero with the flag clear stores nothing", slot(rbp, -0x69), 0)
blocked = scratch(0x100)
poke(blocked, 0xa9, 1)
check("lookup zero with flag set skips to scan", CALL_PRIMARY(rbp, 0xFFFF, blocked, 0), 1)
check("lookup zero with the flag set stores nothing", slot(rbp, -0x69), 0)
CALL_PRIMARY(rbp, 0x1234, holder, 1)
check("a later real match still stores on a fresh frame", slot(rbp, -0x69), 0x1234)

def redirect(placeholder, destination):
    at = code.index(struct.pack("<Q", placeholder))
    ctypes.memmove(block + at, struct.pack("<Q", destination), 8)

def jump(at, target, size=5):
    data = b'\xe9' + struct.pack('<i', target-at-5) + b'\x90'*(size-5)
    ctypes.memmove(at, data, len(data))

def native_loops(path, mission_delta, squad_delta):
    from pe import Image
    image = Image(path)
    mission = scratch(0x200)
    origin = 0x216d8c + mission_delta
    raw = image.read(origin, 0x66)
    # These are the production displaced spans, not a second implementation.
    assert raw[:12] == bytes.fromhex('85c075513883a9000000754d')
    assert raw[0x4c:0x55] == bytes.fromhex('4c894d8f4d8be9eb04')
    assert raw[-13:] == bytes.fromhex('4883c608493bf60f854effffff')
    ctypes.memmove(mission, raw, len(raw))
    jump(mission, block+labels['preview_primary'], 12)
    jump(mission+0x4c, block+labels['preview_secondary'], 9)
    redirect(0xaaaaaaaaaaaaaac1, mission+0x59)
    redirect(0xaaaaaaaaaaaaaac2, mission+0x0c)
    # Native ADD/CMP/JNE advances the gun list; only lookup before the filter
    # is replaced by controlled candidates. Fall-through exits the real loop.
    epilogue = asm('mov [rbp+0x10], r13; pop r14; pop r13; pop rsi; pop rbx; pop rbp; ret')
    ctypes.memmove(mission+len(raw), epilogue, len(epilogue))
    dispatch = blob(asm(f'mov rbx, [rsi]; mov eax, [rbx]; mov r9, [rbx+8]; mov r11, {mission}; jmp r11'))
    # Rewrite just the native loop-back displacement for this fixture's lookup.
    ctypes.memmove(mission+len(raw)-4, struct.pack('<i',dispatch-(mission+len(raw))),4)
    init = image.read(0x216c98+mission_delta, 8)
    assert init == bytes.fromhex('0f57c0f30f7f458f')
    entry = blob(asm('push rbp; push rbx; push rsi; push r13; push r14; mov rbp, rcx; mov rsi, rdx; lea r14, [rdx+r8*8]; mov r13, r9')
                 + init + asm(f'mov r11, {dispatch}; jmp r11'))
    call = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_size_t)(entry)
    def candidate(weapon, primary=0, blocked=False, filtered=False):
        row = scratch(0x220)
        poke(row,0,primary,4); poke(row,8,weapon); poke(row,0xa9,int(blocked),1)
        if filtered:
            item=scratch(0x120); poke(item,0x11c,0x400,4)
            items=scratch(16); poke(items,0,item)
            poke(row,0x210,items); poke(row,0x218,items+16)
        return row
    def scan(rows, fallback=0):
        pointers=blob(struct.pack('<'+'Q'*len(rows),*rows))
        frame_=frame()
        poke(frame_,-0x71,0xdead); poke(frame_,-0x69,0xbeef)
        call(frame_,pointers,len(rows),fallback)
        return slot(frame_,-0x71),slot(frame_,-0x69),slot(frame_,0x10)
    got=scan([candidate(0x999,blocked=True),candidate(0x888,filtered=True),
              candidate(0xaaa),candidate(0xbbb),candidate(0xccc,1),candidate(0xddd,1)])
    assert got==(0xaaa,0xccc,0xaaa),got
    second=scan([candidate(0xeee),candidate(0xfff)],got[2])
    assert second==(0xeee,0,0xaaa),second
    assert scan([candidate(0x999,blocked=True)],second[2])==(0,0,0xaaa)
    # Execute the real downstream fallback descriptor stores.
    stores=image.read(0x216f7e+mission_delta,12)
    assert stores==bytes.fromhex('c64220014c896a2848897230')
    fallback_entry=blob(asm('push r13; push rsi; mov r13, rdx; mov rdx, rcx; xor esi, esi')
                        +stores+asm('pop rsi; pop r13; ret'))
    dead=scratch(0x38)
    ctypes.CFUNCTYPE(None,ctypes.c_void_p,ctypes.c_size_t)(fallback_entry)(dead,second[2])
    assert slot(dead,0x28)==0xaaa and slot(dead,0x30)==0
    assert ctypes.c_ubyte.from_address(dead+0x20).value==1

    squad=scratch(0x100)
    raw=image.read(0x29acf0+squad_delta,20)
    assert raw==bytes.fromhex('488b82b8000000488941284883c138493bc875ec')
    ctypes.memmove(squad,raw,len(raw))
    jump(squad,block+labels['preview_squad'],11)
    redirect(0xaaaaaaaaaaaaaac3,squad+15)
    ctypes.memmove(squad+20,b'\xc3',1)
    call_squad=ctypes.CFUNCTYPE(None,ctypes.c_void_p,ctypes.c_void_p,ctypes.c_void_p)(squad)
    descriptors=scratch(3*0x38+8); weapon=scratch(0xc0)
    poke(descriptors,3*0x38,0xcafebabe)
    for weapon_id in [0x111,0x222]:
        poke(weapon,0xb8,weapon_id)
        call_squad(descriptors,weapon,descriptors+3*0x38)
    assert [slot(descriptors+i*0x38,0x28) for i in range(3)]==[0x111]*3
    assert slot(descriptors,3*0x38)==0xcafebabe
    print(f'PASS  {path}: native filters, per-soldier reset, scan, fallback and descriptor loop')

native_loops(str(builds.reference().game),0,0)
native_loops(str(builds.build("steam-2025-12-23").game),0x48f0,0x5390)

print()
print(f"{failures} failed" if failures else "all cases as expected")
sys.exit(1 if failures else 0)
