"""Execute order hooks through their actual call-site register/stack ABI.

The stock collector is represented by its completed command-local array. The
real payload wrappers run via patched copies of the actual module instructions.
Guard cells and register snapshots catch stack, count and resume-flag errors.
"""
import ctypes, json, pathlib, sys, struct
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
    """Use the real selection getter, including disabled and deselected parents."""
    vt = [0] * 32
    vt[0xb0 // 8] = STUB_FACETS
    obj = blob(bytes(bytearray(0x120)), 0x120)
    poke(obj, 0, words(vt))
    if marked is not None:
        selectable_vt = [0] * 16
        selectable_vt[0x58 // 8] = SELECTED_GETTER
        selectable = scratch(0x40)
        poke(selectable, 0, words(selectable_vt))
        poke(selectable, 0x18, int(marked != "disabled"), 1)
        poke(selectable, 0x30, int(marked is True or isinstance(marked, str)), 1)
        parent = scratch(0x20)
        poke(parent, 0, words([0]*11 + [STUB_SELECTED]))
        poke(parent, 0x10, int(marked != "deselected"), 1)
        poke(selectable, 0x28, parent)
        facets = blob(bytes(bytearray(0x60)), 0x60)
        poke(facets, 0x50, selectable)
        poke(obj, 0x110, facets)
    return obj



descriptor = json.loads(pathlib.Path("out/payload.json").read_text())
payload = blob(pathlib.Path("out/payload.bin").read_bytes())
SELECTED_GETTER = payload + next(h["hook_entry"] for h in descriptor["detours"]
                                if h["hook_rva"] == 0x4491c0)
image = Image()
failures = 0

def check(condition, label):
    global failures
    if not condition:
        failures += 1
        print("FAIL", label)

# Members fit in the same 16-pointer stack array used by the stock dispatcher.
# The order handle and attack continuation sentinel exercise displaced loads.
for feature, count_register, rva in [(8, "r13", 0x43bab9), (9, "rax", 0x43c6cd)]:
    hook = next(c for c in descriptor["pose_calls"] if c["pose_site"] == rva)
    before = bytes.fromhex(hook["pose_before"])
    assert image.read(hook["pose_site"], len(before)) == before
    for handle_value in (0, 0x12345678):
        order_handle = words([handle_value])
        # A near call + descriptor tail, exactly as installed in logic.dll.
        site = scratch(64)
        patch = b"\xe8" + struct.pack("<i", payload + hook["pose_entry"] - site - 5)
        patch += bytes.fromhex(hook["pose_tail"])
        # Capture flags before any flag-changing instruction executes.
        patch += asm("setz byte ptr [rbx + 0x18]; ret")
        ctypes.memmove(site, patch, len(patch))
        trampoline = blob(asm(f"""
            push rbx
            push rbp
            push rsi
            push rdi
            push r12
            push r13
            push r14
            push r15
            sub rsp, 0x160
            mov rbx, r8
            mov r12, rdx
            mov r15, rdx
            lea rbp, [rsp + 0x80]
            mov rsi, rcx
            xor r9d, r9d
        copy_members:
            cmp r9, r15
            jae copied_members
            mov rax, qword ptr [rsi + r9*8]
            mov qword ptr [rbp + r9*8 - 0x10], rax
            inc r9
            jmp copy_members
        copied_members:
            mov qword ptr [rsp + 0x68], 0x76543210
            mov qword ptr [rsp + 0xf0], 0x76543210
            mov qword ptr [rsp + 0x48], 0x12345678
            mov r13, {order_handle}
            mov {count_register}, r12
            mov r10, {site}
            call r10
            mov qword ptr [rbx], rax
            mov qword ptr [rbx + 8], r13
            mov qword ptr [rbx + 0x10], rdi
            mov qword ptr [rbx + 0x110], r12
            mov rax, qword ptr [rsp + 0x68]
            mov qword ptr [rbx + 0x20], rax
            mov rax, qword ptr [rsp + 0xf0]
            mov qword ptr [rbx + 0x28], rax
            xor r9d, r9d
        copy_back:
            cmp r9, r15
            jae finished
            mov rax, qword ptr [rbp + r9*8 - 0x10]
            mov qword ptr [rbx + r9*8 + 0x30], rax
            inc r9
            jmp copy_back
        finished:
            add rsp, 0x160
            pop r15
            pop r14
            pop r13
            pop r12
            pop rdi
            pop rsi
            pop rbp
            pop rbx
            ret
        """))
        run = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p)(trampoline)
        cases = [[], [False]*4, [True]*4, [False,True,False,False],
                 [True,False,True,False], [False,False,False,True],
                 [True], [False], [None,True,False], [None,False],
                 [True,False]*8, [True,"disabled","deselected",False],
                 ["disabled","deselected"]]
        for marks in cases:
            members = [entity(m) for m in marks]
            # Include an actual null member separately from a missing facet.
            for null_member in (False, True):
                current = ([0] + members) if null_member else members
                if len(current) > 16:
                    continue
                source = words(current)
                output = scratch(0x130)
                run(source, len(current), output)
                expected = [m for m, selected in zip(members, marks) if selected is True]
                if not expected:
                    expected = current
                count = peek(output, 8 if feature == 8 else 0)
                label = f"site={rva:x} feature={feature} marks={marks} null={null_member} handle={handle_value}"
                check(count == len(expected), label + " count")
                check([peek(output, 0x30+i*8) for i in range(count)] == expected, label + " members")
                check([peek(source, i*8) for i in range(len(current))] == current, label + " source untouched")
                check(peek(output, 0x20) == 0x76543210 and peek(output, 0x28) == 0x76543210, label + " guards")
                if feature == 8:
                    check(peek(output) == 0x12345678, label + " displaced load")
                else:
                    check(peek(output, 8) == order_handle and peek(output, 0x10) == handle_value, label + " displaced registers")
                    check(peek(output, 0x18, 1) == (handle_value == 0), label + " resume flags")
# Building-panel exit and both facing paths use occupants without selection.
for rva, dest in [(0x107906, "rdi"), (0x107e30, "rbx"),
                  (0x10f806, "rbx"), (0x319cd4, "rbx")]:
    hook = next(c for c in descriptor["pose_calls"] if c["pose_site"] == rva)
    before = bytes.fromhex(hook["pose_before"])
    assert image.read(rva, len(before)) == before
    site = scratch(128)
    patch = b"\xe8" + struct.pack("<i", payload + hook["pose_entry"] - site - 5)
    patch += bytes.fromhex(hook["pose_tail"])
    patch += asm(f"""
        setz byte ptr [r12 + 0x18]
        mov qword ptr [r12], {dest}
        jz done
        mov rax, qword ptr [{dest} + 0x38]
        mov qword ptr [r12 + 8], rax
        mov rax, qword ptr [{dest} + 0x40]
        mov qword ptr [r12 + 0x10], rax
    done:
        ret
    """)
    ctypes.memmove(site, patch, len(patch))
    trampoline = blob(asm(f"""
        push rbx
        push rdi
        push r12
        sub rsp, 0x28
        mov r12, rdx
        mov rax, rcx
        mov r10, {site}
        call r10
        add rsp, 0x28
        pop r12
        pop rdi
        pop rbx
        ret
    """))
    run_exit = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)(trampoline)
    for occupants in ([], [entity(False)], [entity(False), entity(True), entity(False)]):
        # Distinct handles model occupants from multiple squads. The associated
        # squad list deliberately contains different units.
        refs = [words([0, 0, member]) for member in occupants]
        array = words(refs)
        active = scratch(0x1c0)
        poke(active, 0x1a8, array)
        poke(active, 0x1b0, array + len(refs)*8)
        tangible = scratch(0x168)
        poke(tangible, 0x160, active)
        selectable = scratch(0x48)
        outsiders = words([entity(True), entity(False)])
        poke(selectable, 0x38, outsiders)
        poke(selectable, 0x40, outsiders + 16)
        facets = scratch(0x60)
        poke(facets, 0x10, tangible)
        poke(facets, 0x50, selectable)
        for missing in ("none", "active", "tangible", "facets"):
            poke(tangible, 0x160, 0 if missing == "active" else active)
            poke(facets, 0x10, 0 if missing == "tangible" else tangible)
            output = scratch(0x20)
            run_exit(0 if missing == "facets" else facets, output)
            label = f"building command {rva:x} occupants={len(refs)} missing={missing}"
            valid = missing == "none"
            check(peek(output) == (active + 0x170 if valid else 0), label + " view")
            check(peek(output, 0x18, 1) == int(not valid), label + " flags")
            if valid:
                check(peek(output, 8) == array and peek(output, 0x10) == array + len(refs)*8, label + " bounds")
                check([peek(peek(output, 8), i*8) for i in range(len(refs))] == refs, label + " occupants only")
            check(peek(selectable, 0x38) == outsiders and peek(active, 0x1a8) == array, label + " source untouched")
# Run the actual stock hasPlacesForUnits function, with its real reservation
# traversal/deduplication/faction filtering. Only allocation and world lookup
# are fixtures. This catches errors that a Python copy of its formula misses.
native = scratch(0x690000)
ctypes.memmove(native + 0x65060, image.read(0x65060, 0x360), 0x360)

def near_call(site, target):
    return b"\xe8" + struct.pack("<i", target - site - 5)

@ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p)
def append_reservation(vector, end, value):
    begin = peek(vector)
    old = [peek(begin, n) for n in range(0, peek(vector, 8) - begin, 8)]
    array = words(old + [peek(value)])
    for offset, address in [(0, array), (8, array + 8*(len(old)+1)), (16, array + 8*(len(old)+1))]:
        poke(vector, offset, address)

for at, target in [(0x65117, blob(asm("mov rax, rcx; ret"))),
                   (0x652a9, ctypes.cast(append_reservation, ctypes.c_void_p).value),
                   (0x6539d, blob(asm("ret")))]:
    # A local absolute-jump relay keeps callback addresses outside rel32 safe.
    relay = blob(b"\xff\x25\x00\x00\x00\x00" + struct.pack("<Q", target))
    ctypes.memmove(native + at, near_call(native + at, relay), 5)

for fixup in descriptor["rel_fixups"]:
    if fixup.get("rel_feature") == 9:
        address = payload + fixup["rel_offset"]
        ctypes.memmove(address, struct.pack("<i", native + fixup["rel_target"] - address - 4), 4)

def install_order_site(rva):
    hook = next(c for c in descriptor["pose_calls"] if c["pose_site"] == rva)
    assert image.read(rva, len(bytes.fromhex(hook["pose_before"]))) == bytes.fromhex(hook["pose_before"])
    patch = near_call(native + rva, payload + hook["pose_entry"]) + bytes.fromhex(hook["pose_tail"])
    ctypes.memmove(native + rva, patch, len(patch))
    return payload + hook["pose_entry"]

install_order_site(0x65355)
has_places = ctypes.CFUNCTYPE(ctypes.c_uint8, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t)(native + 0x65060)
get_field = blob(asm("mov rax, qword ptr [rcx + 0x110]; ret"))
get_order_owner = blob(asm("mov rax, qword ptr [rcx + 0x18]; ret"))
# Same faction is friendly; different faction is the native excluded value 5.
relation = blob(asm("xor eax, eax; cmp edx, r8d; je done; mov eax, 5; done: ret"))
context_vt = [0]*24
context_vt[0xb0//8] = relation
context = words([words(context_vt)])

def capacity_unit(faction=1):
    unit = entity(False)
    ownership = scratch(0x180)
    poke(ownership, 0x118, faction, 4)
    poke(peek(unit, 0x110), 0x20, ownership)
    return unit

def reservation_list(owners):
    head = scratch(0x18)
    previous = head
    for owner in owners:
        order_vt = [0]*13
        order_vt[0x60//8] = get_order_owner
        order = words([words(order_vt), 0, 0, owner])
        ref = words([0, 10, order])  # non-final reference: no destructor fixture
        node = words([head, previous, ref])
        poke(previous, 0, node)
        previous = node
    poke(previous, 0, head)
    poke(head, 8, previous)
    return head

def capacity_building(capacity, owners, blocked=0):
    building = scratch(0x218)
    entity_vt = [0]*24
    entity_vt[0x78//8] = get_field
    obj = scratch(0x118)
    poke(obj, 0, words(entity_vt))
    poke(obj, 0x110, context)
    poke(building, 0x90, words([0, 10, obj]))
    poke(building, 0xe0, 0x100000)
    poke(building, 0xe8, 0x100000 + capacity*0xb8)
    poke(building, 0x188, reservation_list(owners))
    poke(building, 0x190, len(owners))
    poke(building, 0x208, blocked, 2)
    return building

first = [capacity_unit() for _ in range(5)]
second = [capacity_unit() for _ in range(5)]
building = capacity_building(7, first)
check(not has_places(building, words(second), 5), "stock whole-squad predicate rejects 5 with 2 places")
for rva in (0x10aa5f, 0x10ac2b):
    entry = install_order_site(rva)
    command = ctypes.CFUNCTYPE(ctypes.c_uint8, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t)(entry)
    check(command(building, words(second), 5), f"partial command {rva:x} accepts 2 free places")
    check(not command(capacity_building(7, first + second[:2]), words(second[2:]), 3), "full building rejects outside candidates")
    check(command(capacity_building(7, first + second[:2]), words(second[:2]), 2), "occupants can receive facing orders when full")
    check(not command(building, words([]), 0), "empty command rejected")
    check(not command(building, 0, 5), "missing array rejected")
    check(not command(0, words(second), 5), "missing building rejected")
    check(command(building, words([0, second[0]]), 2), "null candidates skipped")

for rva, state_register in [(0xa6b9d, "r15"), (0xaac73, "r14")]:
    entry = install_order_site(rva)
    trampoline = blob(asm(f"""
        push {state_register}
        push rdi
        push r12
        push r13
        sub rsp, 8
        sub rsp, 0x20
        mov {state_register}, rdx
        mov rdx, r8
        mov r13, r9
        mov rax, {entry}
        call rax
        mov qword ptr [r13], {"rdi" if rva == 0xa6b9d else "r12"}
        add rsp, 0x20
        add rsp, 8
        pop r13
        pop r12
        pop rdi
        pop {state_register}
        ret
    """))
    state_check = ctypes.CFUNCTYPE(ctypes.c_uint8, ctypes.c_void_p, ctypes.c_void_p,
                                  ctypes.c_void_p, ctypes.c_void_p)(trampoline)
    admitted = list(first)
    for index, unit in enumerate(second):
        ai_vt = [0]*11
        ai_vt[0x50//8] = get_field
        ai = scratch(0x118)
        poke(ai, 0, words(ai_vt))
        poke(ai, 0x110, unit)
        state = words([0, 0, words([0, 10, ai])])
        copied = words(first)
        count = scratch(8)
        allowed = bool(state_check(capacity_building(7, admitted), state, copied, count))
        check(allowed == (index < 2), f"individual state {rva:x} admission {index}")
        expected_count = int(allowed) if rva == 0xa6b9d else 1
        check(peek(count) == expected_count and peek(copied) == unit,
              "failure skips squad retreat / restricts individual cleanup to owner")
        check([peek(copied, j*8) for j in range(1, 5)] == first[1:], "copied array tail untouched")
        if allowed:
            admitted.append(unit)
    check(len(admitted) == 7, "five plus five fills exactly seven places")
    check(not state_check(building, words([0, 0, 0]), copied, count), "missing state owner rejected")
    check(peek(count) == 0, "missing owner disables native squad cleanup")

# Use real native traversal to check duplicates, self exclusion, faction rules,
# and unsigned underflow when a building loses usable places.
unit = second[0]
check(has_places(capacity_building(2, [first[0], first[0]]), words([unit]), 1), "duplicate reservation counts once")
check(has_places(capacity_building(1, [unit]), words([unit]), 1), "own reservation not counted twice")
check(has_places(capacity_building(1, [capacity_unit(2)]), words([unit]), 1), "native faction filtering retained")
check(not has_places(capacity_building(2, first), words([unit]), 1), "over-reserved save does not underflow")
check(not has_places(capacity_building(2, [], blocked=3), words([unit]), 1), "blocked places do not underflow")
check(not has_places(capacity_building(7, first, blocked=2), words([unit]), 1), "blocked places count toward capacity")

# Exercise both exits from the actual patched comparison. Parent identity is
# deliberately identical: stock would promote the outside soldier as well.
install_order_site(0x65736)
ctypes.memmove(native + 0x6573b, asm("mov eax, 1; ret"), 6)
ctypes.memmove(native + 0x6577f, asm("xor eax, eax; ret"), 3)
promotion = blob(asm(f"""
    push rbx
    push rsi
    push rdi
    push r15
    sub rsp, 0x28
    mov rbx, rcx
    mov rsi, rdx
    mov rdi, 1234
    mov r15, rdi
    mov rax, {native + 0x65736}
    call rax
    add rsp, 0x28
    pop r15
    pop rdi
    pop rsi
    pop rbx
    ret
"""))
promotes = ctypes.CFUNCTYPE(ctypes.c_uint8, ctypes.c_void_p, ctypes.c_void_p)(promotion)
for owner in [unit, first[0], 0]:
    node = peek(reservation_list([owner]))
    check(bool(promotes(node, unit)) == (owner == unit), "promote only entrant even with shared squad")

print(f"Order hook regressions: {failures} failures")
sys.exit(bool(failures))
