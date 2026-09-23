"""Execute stock/patched marquee manager loops with controlled spatial eligibility.
Native collection, enabled checks and Shift add/remove decisions are retained.
Only allocation, categorization and the world-space predicate use fixture stubs.
"""
import ctypes as C, json, pathlib, struct
import build as b
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
hooks={h['pose_site']:h for h in desc['pose_calls'] if h['pose_entry']==0x2500}
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
marquee = C.CFUNCTYPE(C.c_ubyte,C.c_void_p,C.c_void_p,C.c_void_p)(payload+0x2500)
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
n.close()
