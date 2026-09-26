"""Execute stock/patched marquee manager loops with controlled spatial eligibility.
Native collection, enabled checks and Shift add/remove decisions are retained.
Only allocation, categorization and the world-space predicate use fixture stubs.
"""
import ctypes as C, json, pathlib, struct
import build as b
from payload import REGION_OFFSET
from test_selection import Native

n = Native()
img = b.Image()
def asm(s): return b.assemble(s.splitlines(), 0, 0)[0]
def code(s): return n.code(asm(s))
def q(p): return C.c_uint64.from_address(p).value
def put(p,v): C.c_uint64.from_address(p).value=v
callbacks=[]
def callback(fn, count):
    cb=C.CFUNCTYPE(None,*([C.c_void_p]*count))(fn); callbacks.append(cb)
    return C.cast(cb,C.c_void_p).value
# Native vectors grow into fixture-owned storage and are never freed by C++.
def grow(vec,end,source):
    start=q(vec)
    if not start:
        start=n.data(0x100); put(vec,start); put(vec+8,start); put(vec+16,start+0x100)
    dest=q(vec+8); put(dest,q(source)); put(vec+8,dest+8)
grow_ptr=callback(grow,3)
noop=code('ret')
cat=code('mov rax, [rdx]\nmov [rcx], rax\nmov rax, [rdx+8]\nmov [rcx+8], rax\nret')
eligible=code('movzx eax, byte ptr [rcx+0x20]\nret')
setter=code('mov byte ptr [rcx+0x30], dl\nret')
getter=code('movzx eax, byte ptr [rcx+0x30]\nret')
get_facets=code('mov rax, [rcx+8]\nret')
is_type=code('mov eax, [rcx+0x10]\nand eax, edx\nret')
sv=n.data(0x60,[(0x50,setter),(0x58,getter)])
ev=n.data(0xb8,[(0x98,is_type),(0xb0,get_facets)])

def entity(kind, inside, enabled=True):
    selectable=n.data(0x38,[(0,sv),(0x18,int(enabled))])
    facets=n.data(0x60,[(0x50,selectable)])
    return n.data(0x28,[(0,ev),(8,facets),(0x10,kind),(0x20,int(inside))]), selectable

# Use the actual assembled payload and descriptor fixup for the native predicate.
desc=json.loads(pathlib.Path('out/payload.json').read_text())
payload=n.code(pathlib.Path('out/payload.bin').read_bytes())
fix=next(f for f in desc['rel_fixups'] if f['rel_target']==0x418000)
C.memmove(payload+fix['rel_offset'],struct.pack('<i',eligible-(payload+fix['rel_offset']+4)),4)
hooks={h['pose_site']:h for h in desc['pose_calls'] if h['pose_entry']==REGION_OFFSET}
assert len(hooks)==4

def add(manager,vec):
    for p in range(q(vec),q(vec+8),8):
        obj=q(p); selectable=q(q(obj+8)+0x50)
        C.c_ubyte.from_address(selectable+0x30).value=1
add_ptr=callback(add,2)

def routine(start,end,patched):
    raw=bytearray(img.read(start,end-start))
    for ins in list(img.md.disasm(bytes(raw),start)):
        if ins.mnemonic=='call' and ins.op_str.startswith('0x'):
            target=int(ins.op_str,16)
            target={0x418000:eligible,0x418390:cat,0x5a070:grow_ptr,
                    0x55410:noop,0x680ac0:noop,0x419cf0:add_ptr}[target]
            if patched and ins.address in hooks:
                hook=hooks[ins.address]
                assert img.read(ins.address,5).hex()==hook['pose_before']
                target=payload+hook['pose_entry']
            at=len(raw); raw+=b'\x48\xb8'+struct.pack('<Q',target)+b'\xff\xe0'
            struct.pack_into('<i',raw,ins.address-start+1,start+at-ins.address-5)
    return C.CFUNCTYPE(None,C.c_void_p,C.c_void_p)(n.code(bytes(raw)))

for patched in (False,True):
    select=routine(0x418db0,0x418f31,patched)
    remove=routine(0x419380,0x4194e1,patched)
    shift=routine(0x419020,0x4191e9,patched)
    objects=[entity(0x10,True),entity(0,True),entity(0,False),entity(0,True,False),entity(0,True)]
    arr=n.data(len(objects)*8,[(i*8,o[0]) for i,o in enumerate(objects)])
    vt=n.data(0xa0,[(0x90,C.cast(remove,C.c_void_p).value)])
    manager=n.data(0x38,[(0,vt),(0x28,arr),(0x30,arr+len(objects)*8)])
    region=n.data(0x20)
    def marks(): return [C.c_ubyte.from_address(s+0x30).value for _,s in objects]
    def setmarks(values):
        for (_,s),v in zip(objects,values): C.c_ubyte.from_address(s+0x30).value=v
    expected=[0 if patched else 1,1,0,0,1]
    select(manager,region)
    assert marks()==expected,(patched,'replace',marks())
    setmarks([0,0,1,0,0])
    shift(manager,region)
    assert marks()==[expected[0],1,1,0,1],(patched,'shift add',marks())
    # Everything eligible is selected: Shift must remove only individual hits,
    # retaining both the icon's flag and the outside soldier.
    setmarks([1,1,1,0,1])
    shift(manager,region)
    assert marks()==[1 if patched else 0,0,1,0,0],(patched,'shift remove',marks())
    # Empty individual box containing only the icon changes no individual mark.
    for obj,_ in objects[1:]: C.c_ubyte.from_address(obj+0x20).value=0
    setmarks([0]*5); select(manager,region)
    assert marks()==[0 if patched else 1,0,0,0,0]
    print('PASS', 'patched' if patched else 'stock reproduction', 'marquee replace, Shift add/remove, icon-only, outside/disabled exclusion')
# Exercise the actual assembled icon gate with the native predicate's stack
# frame. The earlier fixture replaced the whole predicate and missed icon hits.
gate = next(h['hook_entry'] for h in desc['detours'] if h['hook_rva'] == 0x418128)
epilogue = "add rsp, 0xa0\npop r14\npop rdi\npop rsi\nret"
stock_icon = code("movzx eax, byte ptr [rdi+0x21]\nor al, byte ptr [rdi+0x20]\n" + epilogue)
position_only = code("movzx eax, byte ptr [rdi+0x20]\n" + epilogue)
# The displaced instructions load the context's vtable before stock handling.
context = n.data(8, [(0, ev)])
native_frame = code(f"push rsi\npush rdi\npush r14\nsub rsp, 0xa0\nmov rdi, rcx\nmov r14, r8\nmov rax, {payload+gate}\njmp rax")
for target, replacement in [(0x418000, native_frame), (0x41812e, stock_icon), (0x4181d3, position_only)]:
    fix = next(f for f in desc['rel_fixups'] if f['rel_target'] == target)
    C.memmove(payload+fix['rel_offset'], struct.pack('<i', replacement-(payload+fix['rel_offset']+4)), 4)
marquee = C.CFUNCTYPE(C.c_ubyte,C.c_void_p,C.c_void_p,C.c_void_p)(payload+REGION_OFFSET)
other_caller = C.CFUNCTYPE(C.c_ubyte,C.c_void_p,C.c_void_p,C.c_void_p)(native_frame)
for kind in [0x20, 0x200, 0x80, 0x10]:
    obj,_ = entity(kind,False)
    C.c_ubyte.from_address(obj+0x21).value=1  # icon inside, body outside
    assert other_caller(obj,0,context)==1, ('non-marquee icon unchanged',kind)
    assert marquee(obj,0,context)==(kind not in (0x20,0x10)), ('icon-only marquee',kind)
    C.c_ubyte.from_address(obj+0x20).value=1
    assert marquee(obj,0,context)==(kind!=0x10), ('body inside',kind)
    C.c_ubyte.from_address(obj+0x20).value=0
    C.c_ubyte.from_address(obj+0x21).value=0
    assert marquee(obj,0,context)==0
print('PASS native-frame icon gate: infantry position only, buildings/vehicles and non-marquee callers unchanged')

# Squads mode (the cell's mode byte 1): a squad counts by its icon, its body or
# any soldier's own position; soldiers never count alone; Ctrl (asked through
# the cell's GetAsyncKeyState with VK_CONTROL) selects as soldiers mode does.
cell = payload + desc['marquee_cell']
ctrl_up = code("xor eax, eax\nret")
ctrl_down = code("xor eax, eax\ncmp ecx, 0x11\njne done\nmov eax, 0xffff8000\ndone:\nret")
def squad(icon, body, members):
    """A squad entity whose AI facet (+0x28) answers its roster, as
    patch/select-squad.asm reaches it: vt+squad_roster, then vt+0x68."""
    obj, _ = entity(0x10, body)
    C.c_ubyte.from_address(obj+0x21).value = icon
    soldiers = [entity(0x20, inside)[0] for inside in members]
    array = n.data(max(8, 8*len(soldiers)), [(i*8, s) for i, s in enumerate(soldiers)])
    vector = n.data(0x10, [(0, array), (8, array+8*len(soldiers))])
    list_vt = n.data(0x70, [(0x68, code(f"mov rax, {vector}\nret"))])
    roster = n.data(0x10, [(0, list_vt)])
    getter = code(f"mov rax, {roster}\nret")
    ai_vt = n.data(b.SYMBOLS["squad_roster"] + 8, [(b.SYMBOLS["squad_roster"], getter)])
    put(q(obj+8)+0x28, n.data(0x10, [(0, ai_vt)]))
    return obj

C.c_ubyte.from_address(cell+8).value = 1
for key, soldiers_mode in ((ctrl_up, False), (ctrl_down, True), (0, False)):
    put(cell, key)
    for label, obj, want, want_soldiers in (
            ("a soldier inside", entity(0x20, True)[0], 0, 1),
            ("a squad by its icon", squad(1, 0, [0, 0]), 1, 0),
            ("a squad by one soldier inside", squad(0, 0, [0, 1, 0]), 1, 0),
            ("a squad with nobody inside", squad(0, 0, [0, 0]), 0, 0),
            ("a squad without members", squad(0, 0, []), 0, 0),
            ("a vehicle inside", entity(0x80, True)[0], 1, 1)):
        got = marquee(obj, 0, context)
        expected = want_soldiers if soldiers_mode else want
        assert got == expected, ('squads mode', 'ctrl' if soldiers_mode else 'no ctrl', label, got)
C.c_ubyte.from_address(cell+8).value = 0
put(cell, ctrl_up)
assert marquee(squad(1, 1, [1]), 0, context) == 0, 'soldiers mode ignores squads'
assert marquee(entity(0x20, True)[0], 0, context) == 1, 'soldiers mode takes the soldier'
print('PASS squads mode: whole squads by icon or any soldier inside, Ctrl for soldiers, soldiers mode unchanged')

# The replace pass's select loop body: a squad hit goes to the manager's select
# (fn_418cb0, which marks its roster), anything else keeps setSelected(1).
entry = next(c['pose_entry'] for c in desc['pose_calls'] if c['pose_site'] == 0x418e90)
picked = n.data(8)
manager_select = code(f"mov rax, {picked}\nmov [rax], rdx\nret")
fix = next(f for f in desc['rel_fixups'] if f['rel_target'] == 0x418cb0)
C.memmove(payload+fix['rel_offset'], struct.pack('<i', manager_select-(payload+fix['rel_offset']+4)), 4)
slot = n.data(8)
kept = n.data(8)
loop = C.CFUNCTYPE(None)(code(f"push rbx\nsub rsp, 0x20\nmov rbx, {slot}\nmov rax, {payload+entry}\ncall rax\n"
                              f"mov rax, {kept}\nmov [rax], rbx\nadd rsp, 0x20\npop rbx\nret"))
for kind, via_manager in ((0x10, True), (0x20, False), (0x80, False)):
    obj, selectable = entity(kind, True)
    put(slot, obj); put(picked, 0)
    C.c_ubyte.from_address(selectable+0x30).value = 0
    loop()
    assert (q(picked) == obj) == via_manager, ('manager select', kind)
    assert C.c_ubyte.from_address(selectable+0x30).value == int(not via_manager), ('own setSelected', kind)
    assert q(kept) == slot, 'rbx survives'
print('PASS replace pass: squads through the manager select, others through their own setSelected')
n.close()
