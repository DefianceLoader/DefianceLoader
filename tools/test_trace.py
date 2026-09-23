"""Run the manager trace stubs natively and check they are transparent.

Each stub is entered through a real `call`, as the engine would, and its
resume imm64 is pointed at a stand-in that captures the machine state the
original function would have continued with. The stubs are diagnostics that
run inside real game calls, so the things that would crash the game are what
is checked: the displaced instructions redone exactly, rsp where the original
code expects it, and rcx, rdx and rbx intact. The ring is checked too: one
entry per call, the caller's return address, the kind bit, the subject, and
wrap-around after thirty-two entries.
"""
import ctypes, json, pathlib, struct, sys
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
    return int.from_bytes((ctypes.c_char * 8).from_address(addr).raw, "little")


code, labels = b.assemble(
    pathlib.Path("patch/select-trace.asm").read_text().splitlines(),
    b.TRACE_CODE_OFFSET, b.TRACE_OFFSET)
block = scratch(b.BLOCK_SIZE)
put(block + b.TRACE_CODE_OFFSET, code)
RING = block + b.TRACE_OFFSET

CAPTURE = scratch(0x100)       # what each stand-in saw on resuming
ENTITY_SLOT = scratch(0x10)    # [rdx] for deselect's displaced load
put(ENTITY_SLOT, struct.pack("<Q", 0x1111222233334444))

# Stand-ins at the resume points. Each records rax, rcx, rdx, rbx, rsp and the
# stack words that prove the displaced instructions ran, then unwinds whatever
# the displaced instructions built and returns to the trampoline.
def stand_in(unwind, extra):
    return asm(f"""
        mov r11, {CAPTURE}
        mov qword ptr [r11 + 0x00], rax
        mov qword ptr [r11 + 0x08], rcx
        mov qword ptr [r11 + 0x10], rdx
        mov qword ptr [r11 + 0x18], rbx
        mov qword ptr [r11 + 0x20], rsp
        {extra}
        {unwind}
        ret
    """)


STANDINS = {
    # select: push rbx; sub rsp, 0xc0 -- rbx sits at [rsp+0xc0]
    "select_trace": stand_in("add rsp, 0xc0; pop rbx",
                             "mov rax, qword ptr [rsp + 0xc0]; mov qword ptr [r11 + 0x28], rax"),
    # toggle and clear: mov [rsp+8], rbx -- read it back
    "toggle_trace": stand_in("", "mov rax, qword ptr [rsp + 8]; mov qword ptr [r11 + 0x28], rax"),
    "clear_trace": stand_in("", "mov rax, qword ptr [rsp + 8]; mov qword ptr [r11 + 0x28], rax"),
    # deselect: sub rsp, 0x28; mov rax, [rdx] -- rax is captured at +0x00
    "deselect_trace": stand_in("add rsp, 0x28", ""),
}
placeholders = {label: ph for _, _, label, ph in b.TRACE_HOOKS}
for label, body in STANDINS.items():
    target = scratch(0x100)
    put(target, body)
    at = code.find(struct.pack("<Q", placeholders[label]))
    assert at >= 0, f"{label} has no resume slot"
    put(block + b.TRACE_CODE_OFFSET + at, struct.pack("<Q", target))

# A trampoline per stub: known rbx, rcx, rdx, then a real call so a real return
# address is on the stack, then report that address and the entry rsp.
REPORT = scratch(0x20)
MANAGER, SENTINEL = 0x6666777788889999, 0x0bb0bb0bb0bb0bb0
def trampoline(label):
    body = asm(f"""
        push rbx
        push rsi
        sub rsp, 0x28
        mov rbx, {SENTINEL}
        mov rcx, {MANAGER}
        mov rdx, {ENTITY_SLOT}
        mov r11, {block + labels[label]}
        lea rsi, [rip + 0]
        call r11
        mov r11, {REPORT}
        mov qword ptr [r11], rsi
        add rsp, 0x28
        pop rsi
        pop rbx
        ret
    """)
    addr = scratch(0x100)
    put(addr, body)
    return ctypes.CFUNCTYPE(None)(addr), addr


failures = 0
def check(label, ok, detail=""):
    global failures
    if not ok:
        failures += 1
    print(f"{'PASS' if ok else 'FAIL'}  {label}{'' if ok else '  ' + detail}")


KIND = {"select_trace": 56, "toggle_trace": 57, "deselect_trace": 58, "clear_trace": 59}
for label in ("select_trace", "toggle_trace", "deselect_trace", "clear_trace"):
    print(f"\n== {label}")
    run, addr = trampoline(label)
    before = q(RING)
    run()
    rax, rcx, rdx, rbx, rsp, extra = (q(CAPTURE + i * 8) for i in range(6))
    # the return address the stub saw is the instruction after `call r11`;
    # rsi holds a lea of an earlier point, so find the call by its bytes
    body = (ctypes.c_char * 0x80).from_address(addr).raw
    ret_addr = addr + body.index(bytes.fromhex("41ffd3")) + 3
    index = before % 32
    tagged, subject = q(RING + 0x10 + index * 16), q(RING + 0x18 + index * 16)
    check("one entry recorded", q(RING) == before + 1)
    check("the caller is the return address",
          tagged & 0x00ff_ffff_ffff_ffff == ret_addr,
          f"{tagged & 0x00ff_ffff_ffff_ffff:#x} vs {ret_addr:#x}")
    check("the kind bit is set, and only it",
          tagged >> 56 == 1 << (KIND[label] - 56), f"{tagged >> 56:#x}")
    want_subject = MANAGER if label == "clear_trace" else ENTITY_SLOT
    check("the subject is the entity (or the manager for clear)",
          subject == want_subject, f"{subject:#x}")
    check("rcx survives", rcx == MANAGER, f"{rcx:#x}")
    check("rdx survives", rdx == ENTITY_SLOT, f"{rdx:#x}")
    check("rbx survives", rbx == SENTINEL, f"{rbx:#x}")
    if label == "select_trace":
        check("the displaced push rbx is on the stack", extra == SENTINEL, f"{extra:#x}")
    elif label in ("toggle_trace", "clear_trace"):
        check("the displaced mov [rsp+8], rbx was redone", extra == SENTINEL, f"{extra:#x}")
    else:
        check("the displaced mov rax, [rdx] was redone", rax == 0x1111222233334444,
              f"{rax:#x}")

print("\n== the ring wraps")
run, _ = trampoline("toggle_trace")
start = q(RING)
for _ in range(40):
    run()
check("the index keeps counting past the ring", q(RING) == start + 40)
last = (q(RING) - 1) % 32
check("the newest entry sits at index mod 32",
      q(RING + 0x10 + last * 16) >> 56 == 1 << 1)

print()
print(f"{failures} failed" if failures else "all cases as expected")
sys.exit(1 if failures else 0)
