"""Execute building focus adapters and native TAB/panel membership code.

Allocation, selection collection, entity reference retrieval and UI drawing are
fixtures; native reference assignment/search execute with real refcount behavior. The adapters, native TAB/panel membership/manager clear, production
soldier setters/getters and ammunition UI query execute as x64 machine code.
No running game is required.
"""
import ctypes as c
import pathlib
import struct
import sys

import keystone

sys.path.insert(0, "tools")
import build as b
from pe import Image

k32 = c.WinDLL("kernel32", use_last_error=True)
k32.VirtualAlloc.restype = c.c_void_p
k32.VirtualAlloc.argtypes = [c.c_void_p, c.c_size_t, c.c_uint32, c.c_uint32]
ks = keystone.Ks(keystone.KS_ARCH_X86, keystone.KS_MODE_64)
asm = lambda s: bytes(ks.asm(s)[0])
keep = []


def alloc(n):
    p = k32.VirtualAlloc(None, max(n, 8), 0x3000, 0x40)
    assert p, c.get_last_error()
    keep.append(p)
    return p


def put(p, data):
    c.memmove(p, data, len(data))


def blob(data):
    p = alloc(len(data))
    put(p, data)
    return p


def q(p, off=0):
    return c.c_uint64.from_address(p + off).value


def setq(p, off, value):
    put(p + off, struct.pack("<Q", value))


def vector(values, capacity=None):
    v = alloc(24)
    storage = alloc(max(len(values), capacity or 0) * 8)
    for i, value in enumerate(values):
        setq(storage, i * 8, value)
    for off, value in ((0, storage), (8, storage + len(values) * 8),
                       (16, storage + max(len(values), capacity or 0) * 8)):
        setq(v, off, value)
    return v


def values(v):
    return [q(p) for p in range(q(v), q(v, 8), 8)]



facets_fn = blob(asm("mov rax,[rcx+0x110]; ret"))
type_fn = blob(asm("mov eax,[rcx+0x118]; and eax,edx; setnz al; ret"))
plain_vt = alloc(0x60)
setq(plain_vt, 0x50, blob(asm("mov [rcx+0x30],dl; ret")))
setq(plain_vt, 0x58, blob(asm("movzx eax,byte ptr [rcx+0x30]; ret")))
mark_code, mark_labels = b.assemble(pathlib.Path('patch/soldier-mark.asm').read_text().splitlines(), 0, 0)
mark_block = blob(mark_code)
member_vt = alloc(0x60)
setq(member_vt, 0x50, mark_block)
setq(member_vt, 0x58, mark_block + mark_labels['is_selected'])
entities = []
ref_vt = alloc(0x18)
setq(ref_vt, 8, blob(asm('inc dword ptr [rcx+0x18]; ret')))
setq(ref_vt, 0x10, blob(asm('inc dword ptr [rcx+0x1c]; ret')))


def holder(e):
    h = alloc(32)
    setq(h, 0, ref_vt)
    setq(h, 8, 1)
    setq(h, 0x10, e)
    return h


def entity(kind, enabled=True):
    e, vt, facets, selectable = alloc(0x120), alloc(0xc0), alloc(0x60), alloc(0x40)
    setq(e, 0, vt)
    setq(vt, 0x98, type_fn)
    setq(vt, 0xb0, facets_fn)
    setq(e, 0x110, facets)
    setq(e, 0x118, kind)
    setq(facets, 0x50, selectable)
    setq(selectable, 0, member_vt if kind == 0x20 else plain_vt)
    setq(selectable, 0x10, holder(e))
    setq(e, 0x100, q(selectable, 0x10))
    put(selectable + 0x18, bytes([enabled]))
    entities.append(e)
    return e


def facet(e):
    return q(q(e, 0x110), 0x50)


def marked(e):
    return c.c_ubyte.from_address(facet(e) + 0x30).value


def set_mark(e, value):
    f = facet(e)
    c.CFUNCTYPE(None, c.c_void_p, c.c_ubyte)(q(q(f), 0x50))(f, value)


def make_squad():
    squad = entity(0x10)
    men = [entity(0x20) for _ in range(5)]
    roster = vector(men)
    ai, vt, group, group_vt = alloc(24), alloc(0x3c0), alloc(24), alloc(0x70)
    setq(ai, 0, vt)
    setq(ai, 8, group)
    setq(vt, 0x3b8, blob(asm('mov rax,[rcx+8]; ret')))
    setq(group, 0, group_vt)
    setq(group, 8, roster)
    setq(group_vt, 0x68, blob(asm('mov rax,[rcx+8]; ret')))
    setq(q(squad, 0x110), 0x28, ai)
    for man in men:
        setq(facet(man), 0x28, facet(squad))
    return squad, men


building, second_building = entity(0x200), entity(0x200)
active_by_building = {}
for e in (building, second_building):
    tangible, active = alloc(0x168), alloc(0x210)
    setq(q(e, 0x110), 0x10, tangible)
    setq(tangible, 0x160, active)
    active_by_building[e] = active
squad_a, men_a = make_squad()
squad_b, men_b = make_squad()
disabled = entity(0x20, False)
setq(facet(disabled), 0x28, facet(squad_a))


def occupants(items, e=building):
    v = vector([holder(man) if man else 0 for man in items])
    active = active_by_building[e]
    setq(active, 0x1a8, q(v))
    setq(active, 0x1b0, q(v, 8))


logic = Image('bin/logic.orig.dll')
manager, manager_vt = alloc(0x58), alloc(0xa0)
setq(manager, 0, manager_vt)
setq(manager_vt, 0x98, blob(logic.read(0x4194f0, 0x51)))
registry = vector(entities)
setq(manager, 0x28, q(registry))
setq(manager, 0x30, q(registry, 8))
clear = c.CFUNCTYPE(None, c.c_void_p)(q(manager_vt, 0x98))


def select(*items):
    clear(manager)
    for e in items:
        set_mark(e, 1)


def selection():
    # Native manager's UI registry excludes squad members, but includes squads.
    return [e for e in entities if (q(e, 0x118) != 0x20 or not q(facet(e), 0x28)) and marked(e)]


spare, growths = 0, 0
CALL = c.CFUNCTYPE(c.c_uint64, c.c_void_p, c.c_void_p, c.c_void_p)


@CALL
def collect(world, game_context_arg, out):
    collection_arguments.append(game_context_arg)
    v = vector(selection(), spare)
    put(out, c.string_at(v, 24))
    return 0x12345678


@CALL
def grow(v, end, item):
    global growths
    assert end == q(v, 8)
    replacement = vector(values(v) + [q(item)], len(values(v)) + 4)
    put(v, c.string_at(replacement, 24))
    growths += 1
    return q(v, 8) - 8


context, vtable = alloc(8), alloc(0x708)  # world facade, not the game context
game_context, context_vt, player = alloc(0x220), alloc(0x48), alloc(0x100)
setq(game_context, 0, context_vt)
setq(game_context, 0x218, player)
# Exact native GameContext::getPlayer bytes, also verified in the crash image.
get_player = bytes.fromhex('488b8118020000c3')
assert get_player in logic.data
setq(context_vt, 0x40, blob(get_player))
lookup_bad_args = alloc(8)
lookup_calls = alloc(8)
collection_arguments = []
setq(context, 0, vtable)
setq(vtable, 0x540, c.cast(collect, c.c_void_p).value)
setq(vtable, 0x548, blob(asm('xor eax,eax; ret')))
setq(vtable, 0x700, blob(asm(f'mov r10,{lookup_calls}; inc qword ptr [r10]; mov r10,{player}; cmp rdx,r10; jne bad; mov rax,{manager}; ret; bad: mov r10,{lookup_bad_args}; inc qword ptr [r10]; xor eax,eax; ret')))
game = alloc(0x500000)
image = Image('bin/game.orig.dll')
for start, end in ((0x34c240, 0x34c3dd), (0x31bf60, 0x31c04b)):
    put(game + start, image.read(start, end-start))
source = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'patch/building-control.asm')
STATE_OFFSET = 0x1fe0
code, labels = b.assemble(source.read_text(encoding='utf-8-sig').splitlines(), 0, 0x800, STATE_OFFSET)
assert len(code) < STATE_OFFSET
block = alloc(0x2000)
put(block, code)
state = block + STATE_OFFSET
for placeholder, target in ((0xaaaaaaaaaaaaaab2, game + 0x34c2ec),
                            (0xaaaaaaaaaaaaaab3, game + 0x34c38b),
                            (0xaaaaaaaaaaaaaab4, c.cast(grow, c.c_void_p).value),
                            (0xaaaaaaaaaaaaaab5, game + 0x368f0),
                            (0xaaaaaaaaaaaaaab6, game + 0x31bfa6)):
    setq(block, code.index(struct.pack('<Q', placeholder)), target)


for site, label, trampoline, size in ((0x34c2e6, 'building_tab_collect', 0x1000, 6),
                                       (0x34c386, 'building_tab_apply', 0x1020, 5),
                                       (0x31bfa0, 'building_tab_refresh', 0x1040, 6)):
    put(game + trampoline, asm(f'mov r11,{block + labels[label]}; jmp r11'))
    put(game + site, b'\xe9' + struct.pack('<i', trampoline-site-5) + b'\x90' * (size-5))
# Real unpatched panel membership. Stop before drawing/reference bookkeeping.
epilogue = 'add rsp,0xf0; pop r14; pop rdi; pop rsi; pop rbx; pop rbp; ret'
put(game + 0x31c04b, asm('xor eax,eax; ' + epilogue))
put(game + 0x31c28c, asm('mov eax,1; ' + epilogue))
# Execute real native entity-reference ownership and TAB search; only the
# imported entity->reference getter is fabricated, with the native temp retain.
put(game + 0x351a10, image.read(0x351a10, 0x128))
put(game + 0x368f0, image.read(0x368f0, 0xd2))
setq(game, 0x4cc870, blob(asm('mov rax,[rcx+0x100]; inc dword ptr [rax+8]; mov [rdx],rax; mov rax,rdx; ret')))
put(game + 0x34c4e0, asm('ret'))
put(game + 0x499474, asm('ret'))
ui, parent, event, focus, panel = alloc(0x370), alloc(0x268), alloc(16), holder(building), alloc(0x158)
setq(ui, 0x128, context)
setq(ui, 0x130, game_context)
setq(ui, 0x138, parent)
setq(ui, 0x2d8, focus)
put(parent + 0x25f, b'\x01')
setq(event, 8, 9)
setq(panel, 0x128, context)
setq(panel, 0x120, game_context)
setq(panel, 0x150, ui + 0x2d8)
assign_focus = c.CFUNCTYPE(None, c.c_void_p, c.c_void_p)(game + 0x368f0)
def focused():
    ref = q(ui, 0x2d8)
    return q(ref, 0x10) if ref else 0

def set_focus(e):
    assign_focus(ui + 0x2d8, e)

tab = c.CFUNCTYPE(None, c.c_void_p, c.c_void_p, c.c_void_p, c.c_void_p)(game + 0x34c240)
refresh = c.CFUNCTYPE(c.c_int, c.c_void_p)(game + 0x31bf60)
shim = blob(asm(f'sub rsp,0x28; mov rax,[rcx]; mov r9d,1; mov r11,{block + labels["building_focus_candidates"]}; call r11; add rsp,0x28; ret'))
candidate_fn = CALL(shim)
checks = 0


def check(label, actual, expected):
    global checks
    assert actual == expected, (label, actual, expected)
    checks += 1
    print('PASS', label)


def candidates():
    v = alloc(24)
    result = candidate_fn(context, game_context, v)
    check('native return preserved', result, 0x12345678)
    return values(v)


occupants([men_a[0], men_b[0], men_a[1], men_b[1], 0, disabled, squad_a])
occupants([men_a[3]], second_building)
select(building)
check('building begins with building-only commands', selection(), [building])
check('one candidate per squad, despite interleaved occupants', candidates(), [building, squad_a, squad_b])
check('native vector growth exercised', growths > 0, True)
spare = 16
old_growths = growths
check('spare capacity same list', candidates(), [building, squad_a, squad_b])
check('no growth with spare capacity', growths, old_growths)
for target, expected_men in ((squad_a, men_a[:2] + men_b[:2]), (squad_b, men_a[:2] + men_b[:2]), (building, []), (squad_a, men_a[:2] + men_b[:2])):
    previous_calls = q(lookup_calls)
    changes_selection = target == building or not q(state)
    tab(ui, 0, 0, event)
    check('native TAB cycles distinct squads and wraps to building', focused(), target)
    check('whole occupant collection remains selected through squad focus', selection(), [building] if target == building else [squad_a, squad_b])
    check('all occupants marked, never outside/other-building members', [e for e in men_a + men_b if marked(e)], expected_men)
    check('panel refresh retains native squad focus', refresh(panel), 1)
    check('building reference retained only during occupant mode', bool(q(state)), target != building)
    check('intermediate TAB never resolves or clears the manager', q(lookup_calls) - previous_calls, int(changes_selection))
    check('native building reference count balances focus plus anchor', q(q(building, 0x100), 8), 1 + int(bool(q(state))) + int(q(ui, 0x2d8) == q(building, 0x100)))
# Test the actual existing ammunition UI query using these same marks.
ui_code, ui_labels = b.assemble(pathlib.Path('patch/icon-squad.asm').read_text().splitlines(), 0, 0x800)
ui_block = blob(ui_code)
one = blob(asm('mov eax,1; ret'))
get_child = blob(asm('mov rax,[rcx+8]; ret'))
for man in men_a + men_b:
    ai, ai_vt, gunner, gunner_vt, gun, gun_vt = alloc(16), alloc(0x138), alloc(16), alloc(0x100), alloc(8), alloc(0x150)
    for obj, vt in ((ai, ai_vt), (gunner, gunner_vt), (gun, gun_vt)):
        setq(obj, 0, vt)
    setq(ai, 8, gunner)
    setq(gunner, 8, gun)
    setq(ai_vt, 0x130, one)
    setq(ai_vt, 0x120, get_child)
    setq(gunner_vt, 0xf0, one)
    setq(gunner_vt, 0xf8, get_child)
    setq(gun_vt, 0x148, one)
    setq(q(man, 0x110), 0x28, ai)
record = alloc(0x48)
setq(record, 0, alloc(8))
put(record + 0x34, struct.pack('<I', 5))
count_stub = blob(asm(f'sub rsp,0x28; mov r11,{ui_block + ui_labels["ammo_ui_state"]}; call r11; mov eax,r9d; add rsp,0x28; ret'))
count_users = CALL(count_stub)
select(building)
check('reproduce old UI-only focus: no member marks counts all five', count_users(squad_a, record, 0), 5)
set_focus(building)
tab(ui, 0, 0, event)
check('weapon users are two occupants, not full squad of five', count_users(squad_a, record, 0), 2)
tab(ui, 0, 0, event)
check('next squad weapon users are its two occupants', count_users(squad_b, record, 0), 2)
# A manual outside selection must cancel building cycling without cached state.
select(men_a[0], men_a[4])
check('outside marked squadmate cancels building cycle', candidates(), [squad_a])
select(men_a[0], men_a[3])
check('selection spanning buildings is ordinary TAB', candidates(), [squad_a])
select(squad_a)
check('squad flag without member marks is ordinary TAB', candidates(), [squad_a])
select(building, squad_a)
check('mixed building/squad selection remains ordinary', candidates(), [building, squad_a])
set_focus(building)
tab(ui, 0, 0, event)
check('ordinary mixed TAB does not alter command selection', selection(), [building, squad_a])
select()
check('empty selection remains empty', candidates(), [])
check('empty selection releases anchor', q(state), 0)
# Leaving the building keeps the selected soldiers under normal squad control.
select(men_a[0], men_a[1])
occupants(men_b[:2])
check('departed occupants are not retained as a building cycle', candidates(), [squad_a])
select(building)
set_focus(building)
tab(ui, 0, 0, event)
check('fresh roster skips departed squad', selection(), [squad_b])
occupants([])
select(building)
set_focus(building)
tab(ui, 0, 0, event)
check('empty building stays building-only', selection(), [building])
for begin, end in ((0, 0), (0x1000, 0xff8), (0x1000, 0x1001)):
    active = active_by_building[building]
    setq(active, 0x1a8, begin)
    setq(active, 0x1b0, end)
    check('empty/reversed/misaligned vector leaves safe selection', candidates(), [building])
standalone = entity(0x20)
registry = vector(entities)
setq(manager, 0x28, q(registry))
setq(manager, 0x30, q(registry, 8))
occupants([standalone])
select(building)
set_focus(building)
tab(ui, 0, 0, event)
check('standalone inhabitant has individual control', selection(), [standalone])
tab(ui, 0, 0, event)
check('standalone inhabitant cycles back to building', selection(), [building])

# Lifecycle tests exercise the refresh hook, not only the next TAB key.
def start_cycle():
    occupants(men_a[:2] + men_b[:2])
    select(building)
    set_focus(building)
    tab(ui, 0, 0, event)
    check('cycle starts with an owned building reference', q(q(state), 0x10), building)


start_cycle()
old_begin, old_end = q(manager, 0x28), q(manager, 0x30)
setq(manager, 0x28, 0)
setq(manager, 0x30, 0)
tab(ui, 0, 0, event)
check('next squad needs no rediscovery through manager registry', focused(), squad_b)
check('all occupants still selected without registry lookup', selection(), [squad_a, squad_b])
setq(manager, 0x28, old_begin)
setq(manager, 0x30, old_end)
for kind in ('outside', 'deselect-member', 'other-building', 'empty'):
    start_cycle()
    if kind == 'outside':
        set_mark(men_a[4], 1)
    elif kind == 'deselect-member':
        set_mark(men_a[1], 0)
    elif kind == 'other-building':
        select(second_building)
    else:
        select()
    before = [marked(e) for e in entities]
    refresh(panel)
    check(kind + ' selection cancels cycle on refresh', q(state), 0)
    check(kind + ' cancellation preserves player selection', [marked(e) for e in entities], before)
    check(kind + ' cancellation releases only the retained reference', q(q(building, 0x100), 8), 1)
for kind in ('departed', 'arrived'):
    start_cycle()
    occupants(([men_a[0]] + men_b[:2]) if kind == 'departed' else men_a[:3] + men_b[:2])
    before = [marked(e) for e in entities]
    refresh(panel)
    check(kind + ' occupant cancels the stale collection', q(state), 0)
    check(kind + ' occupant does not silently rewrite selection', [marked(e) for e in entities], before)
start_cycle()
dead_ref = q(state)
setq(dead_ref, 0x10, 0)  # engine invalidates the entity but retains the holder
refresh(panel)
check('destroyed building invalidation releases anchor safely', q(state), 0)
check('invalidated holder has no leaked cycle reference', q(dead_ref, 8), 1)
setq(dead_ref, 0x10, building)
start_cycle()
other_world = alloc(8)
setq(other_world, 0, vtable)
setq(panel, 0x128, other_world)
refresh(panel)
check('world replacement clears cycle', q(state), 0)
setq(panel, 0x128, context)
start_cycle()
other_context = alloc(0x220)
setq(other_context, 0, context_vt)
setq(other_context, 0x218, player)
setq(panel, 0x120, other_context)
refresh(panel)
check('game-context replacement clears cycle', q(state), 0)
setq(panel, 0x120, game_context)
start_cycle()
setq(game_context, 0x218, alloc(0x100))
refresh(panel)
check('player replacement in same context clears cycle', q(state), 0)
setq(game_context, 0x218, player)
check('collection receives game context, not player', all(arg in (game_context, other_context) for arg in collection_arguments), True)
check('manager always receives player, never game context', q(lookup_bad_args), 0)
lookup = c.CFUNCTYPE(c.c_void_p, c.c_void_p, c.c_void_p)(block + labels['focus_manager'])
check('null game context does not call manager', lookup(context, None), None)
setq(game_context, 0x218, 0)
check('null player does not call manager', lookup(context, game_context), None)
select(building)
set_focus(building)
tab(ui, 0, 0, event)
check('unavailable player retains building selection', selection(), [building])
check('unavailable player retains building focus', focused(), building)
check('failed lookup never forwards a null/wrong player', q(lookup_bad_args), 0)
print(f'{checks} building TAB checks passed')
