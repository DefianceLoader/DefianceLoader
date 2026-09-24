"""Run expanded-menu production DLL and patched stock UI code in disposable processes.
No installed game files are written. Each build/capacity gets a fresh process.
"""
import ctypes as C
import hashlib, json, pathlib, struct, subprocess, sys
from ctypes import wintypes as W
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parent))
from pe import Image
from sigs import Module
from rtti import Rtti
from ammo_menu_sites import offsets_for
ROOT=pathlib.Path(__file__).resolve().parent.parent
DLL=ROOT/"plugins/expanded-ammo-menu/target/release/defiance_plugin_expanded_ammo_menu.dll"
K=C.WinDLL("kernel32",use_last_error=True)
K.LoadLibraryExW.argtypes=[W.LPCWSTR,C.c_void_p,W.DWORD]; K.LoadLibraryExW.restype=C.c_void_p
K.VirtualAlloc.argtypes=[C.c_void_p,C.c_size_t,W.DWORD,W.DWORD]; K.VirtualAlloc.restype=C.c_void_p
K.VirtualProtect.argtypes=[C.c_void_p,C.c_size_t,W.DWORD,C.POINTER(W.DWORD)]
K.FlushInstructionCache.argtypes=[C.c_void_p,C.c_void_p,C.c_size_t]
KEEP=[]
def alloc(n):
    p=K.VirtualAlloc(None,n,0x3000,0x40); assert p
    return p
def put(p,b):
    old=W.DWORD(); assert K.VirtualProtect(p,len(b),0x40,C.byref(old))
    C.memmove(p,b,len(b))
    assert K.VirtualProtect(p,len(b),old,C.byref(W.DWORD()))
    K.FlushInstructionCache(C.c_void_p(-1),p,len(b))
def q(p): return C.c_uint64.from_address(p).value
def pq(p,v): C.c_uint64.from_address(p).value=v
def cb(restype,*args):
    def wrap(fn):
        fun=C.CFUNCTYPE(restype,*args)(fn); KEEP.append(fun); return C.cast(fun,C.c_void_p).value
    return wrap
def stub(at,callback):
    put(at,b"\x48\xb8"+struct.pack("<Q",callback)+b"\xff\xe0")
def resolve(module,source,at):
    start,pattern,mask=source.signature(at,[])
    matches=module.matches(pattern,mask)
    assert len(matches)==1,(hex(at),matches)
    return matches[0]+at-start

PATHS={0:'bin/game.orig.dll',1:'bin/steam/game.dll',2:'bin/gog/game-updated.dll',3:'bin/steam/game-updated.dll'}
LOGIC={0:'bin/logic.orig.dll',1:'bin/steam/logic.dll',2:'bin/gog/logic-updated.dll',3:'bin/steam/logic-updated.dll'}
# The AmmunitionMenu constructor slice (reference game.dll+0x3df21..0x3df62),
# no relative operands, so the same bytes locate it in any build.
CONSTRUCTOR=("488b9620010000488b820801000048898618010000488b8a18010000"
             "48898e28010000488b820001000048898630010000488b01ff90d0000000"
             "48898638010000")
# The same slice on a build whose owner object shifted a field: the reads
# [rdx+0x100]/[rdx+0x108]/[rdx+0x118] became [rdx+0x120]/[rdx+0x128]/[rdx+0x138]
# (both Steam builds), so the reference bytes do not locate it.
CONSTRUCTOR_SHIFTED=("488b9620010000488b822801000048898618010000488b8a38010000"
                     "48898e28010000488b822001000048898630010000488b01ff90d0000000"
                     "48898638010000")


def case(build_index,columns,combined=False):
    path=ROOT/PATHS[build_index]
    module=Module(path); source=Module(ROOT/"bin/game.orig.dll")
    LOFF=offsets_for(hashlib.sha256(path.read_bytes()).hexdigest())
    base=K.LoadLibraryExW(str(path),None,1) # DONT_RESOLVE_DLL_REFERENCES
    assert base,C.get_last_error()
    rows=json.loads((ROOT/"out/ammo-menu-sites.json").read_text())[build_index]["sites"]
    mapped_size=module.pe.OPTIONAL_HEADER.SizeOfImage
    original=C.string_at(base,mapped_size)
    owned={}; calls=0; fail_at=0
    setting=C.create_string_buffer(str(columns).encode(),32); messages=[]
    combined_setting=C.create_string_buffer(b"true" if combined else b"false")
    @cb(None,C.c_uint32,C.c_char_p)
    def log(level,message): messages.append(message.decode())
    @cb(C.c_void_p,C.c_char_p)
    def module_base(name): return base
    @cb(C.c_size_t,C.c_void_p)
    def module_size(_): return mapped_size
    @cb(C.c_void_p,C.c_char_p,C.c_char_p)
    def config(section,key): return C.addressof(combined_setting if key==b"all_selected_squads" else setting)
    @cb(C.c_int,C.c_void_p,C.c_void_p,C.c_void_p,C.c_size_t)
    def patch(at,before,after,n):
        nonlocal calls
        calls+=1
        if fail_at and calls==fail_at:return 1
        old=C.string_at(at,n)
        if old!=C.string_at(before,n) or at in owned:return 1
        owned[at]=old; put(at,C.string_at(after,n)); return 0
    @cb(C.c_int,C.c_void_p)
    def unhook(at):
        if at not in owned:return 1
        put(at,owned.pop(at)); return 0
    @cb(C.c_int,C.c_void_p,C.c_void_p,C.c_size_t,C.c_void_p)
    def hook_exact(at,detour,n,out):
        nonlocal calls
        calls += 1
        if fail_at and calls == fail_at: return 1
        old = C.string_at(at,n)
        trampoline = alloc(n+12)
        put(trampoline,old+bytes.fromhex("48b8")+struct.pack("<Q",at+n)+bytes.fromhex("ffe0"))
        owned[at] = old
        put(at,bytes.fromhex("48b8")+struct.pack("<Q",detour)+bytes.fromhex("ffe0")+bytes.fromhex("90")*(n-12))
        pq(out,trampoline)
        return 0
    class Api(C.Structure):
        _fields_=[("abi",C.c_uint32),("reserved",C.c_uint32)]+[(name,C.c_void_p) for name in
          ["log","module_base","module_size","find_pattern","find_pattern_at","hook","hook_exact","hook_call","unhook","rtti_method","vtable_slot","config_get","patch_bytes"]]
    api=Api(5,0,log,module_base,module_size,0,0,0,hook_exact,0,unhook,0,0,config,patch)
    class Plugin(C.Structure):
        _fields_=[("abi",C.c_uint32),("name",C.c_char_p),("version",C.c_char_p),("init",C.c_void_p),("stop",C.c_void_p)]
    lib=C.CDLL(str(DLL)); lib.defiance_plugin.restype=C.POINTER(Plugin)
    init=C.CFUNCTYPE(C.c_int,C.POINTER(Api))(lib.defiance_plugin().contents.init)
    # Core service fixture: capacity must not change on any failed installation.
    published=0; refuse_publication=False
    @cb(C.c_uint32)
    def capacity(): return published or 9
    @cb(C.c_int,C.c_uint32)
    def publish(value):
        nonlocal published
        if refuse_publication or published: return 2
        assert value==columns*3
        published=value
        return 0
    table=(C.c_void_p*2)(capacity,publish)
    @cb(C.c_ubyte,C.c_void_p)
    def is_selected(facet): return C.c_ubyte.from_address(facet+0x18).value
    selection_table=(C.c_void_p*1)(is_selected)
    @cb(C.c_void_p,C.c_char_p,C.c_char_p,C.c_uint32,C.c_size_t)
    def query(provider,name,version,size):
        if provider==b"defiance.selection":
            assert (name,version,size)==(b"selection",1,8)
            return C.addressof(selection_table)
        assert (provider,name,version,size)==(b"defiance.core",b"ammo-menu",1,16)
        return C.addressof(table)
    class Services(C.Structure):
        _fields_=[("version",C.c_uint32),("size",C.c_uint32),("register",C.c_void_p),("query",C.c_void_p)]
    services=Services(1,24,0,query)
    # A missing Core service must refuse before any patch.
    assert init(C.byref(api))!=0 and calls==0
    lib.defiance_plugin_services.argtypes=[C.POINTER(Services)]
    assert lib.defiance_plugin_services(C.byref(services))==0
    # A corrupt last site must refuse before the first write.
    last=rows[-1]; address=base+last["rva"]; saved=C.string_at(address,1)
    put(address,bytes([saved[0]^1])); assert init(C.byref(api))!=0 and calls==0
    put(address,saved)
    # Failure at several install positions restores every earlier owned span.
    for fail in [1,2,3 if columns==3 else len(rows)//2, 4 if columns==3 else len(rows)+1]:
        fail_at=fail; calls=0
        assert init(C.byref(api))!=0
        assert not owned and published==0
        assert C.string_at(base,mapped_size)==original
    fail_at=0; calls=0; refuse_publication=True
    assert init(C.byref(api))!=0
    assert not owned and published==0
    assert C.string_at(base,mapped_size)==original
    refuse_publication=False
    assert init(C.byref(api))==0,messages
    assert published==columns*3
    count=columns*3; delta=(count-9)*0xb8; size=0x838+delta
    menu=alloc(size+64); put(menu+size,b"G"*64)
    menu_vt=alloc(0x100); pq(menu,menu_vt)
    noop=alloc(16); put(noop,b"\x31\xc0\xc3"); pq(menu_vt+0x38,noop)
    def address(at):return base+resolve(module,source,at)
    hover=address(0x3e830); click=address(0x3f8a0); hide=address(0x3f130)
    blocked=address(0x3b9f0); selected=address(0x40a80)
    # Execute stock hide loop across EVERY slot, including the added tail.
    widget_vt=alloc(0x60); visibility=[]
    @cb(None,C.c_void_p,C.c_ubyte)
    def visible(widget,value):
        visibility.append(widget); C.c_ubyte.from_address(widget+0x5b).value=value
    pq(widget_vt+0x48,visible)
    widgets=[]
    for i in range(count):
        widget=alloc(0x80); pq(widget,widget_vt); C.c_ubyte.from_address(widget+0x5b).value=1
        pq(menu+0x180+i*0xb8+0x48,widget); widgets.append(widget)
    C.CFUNCTYPE(None,C.c_void_p)(hide)(menu)
    assert visibility==widgets
    # Hover and clicks must search all slots and compute the original index.
    put(blocked,b"\x48\x8b\xc1\xc3")
    for index in [0,8,9,count-1]:
        if index>=count:continue
        slot=menu+0x180+index*0xb8; widget=0x100000+index*0x100
        pq(slot+0x18,widget)
        got=C.CFUNCTYPE(C.c_void_p,C.c_void_p,C.c_void_p)(hover)(menu,widget)
        assert got==slot,(index,hex(got or 0),hex(slot))
    put(blocked,b"\x31\xc0\xc3")
    entity=alloc(0x20); entity_vt=alloc(0xc0); facets=alloc(0x60); ai=alloc(0x40); ai_vt=alloc(0x400)
    pq(entity,entity_vt); pq(facets+0x28,ai); pq(ai,ai_vt)
    getter=alloc(16); put(getter,b"\x48\xb8"+struct.pack("<Q",facets)+b"\xc3"); pq(entity_vt+0xb0,getter)
    put(selected,b"\x48\xb8"+struct.pack("<Q",entity)+b"\xc3")
    clicked=[]
    @cb(None,C.c_void_p,C.c_size_t,C.c_uint32)
    def change(_,index,value): clicked.append((index,value))
    pq(ai_vt+LOFF['ai_set'],change)
    for index in [0,8,9,count-1]:
        if index>=count:continue
        slot=menu+0x180+index*0xb8; C.c_uint32.from_address(slot+8).value=0
        C.CFUNCTYPE(None,C.c_void_p,C.c_void_p)(click)(menu,0x100000+index*0x100)
        assert clicked[-1]==(index,1),clicked
    assert C.string_at(menu+size,64)==b"G"*64
    for i,w in enumerate(widgets): pq(menu+0x180+i*0xb8+0x18,w)
    # Execute the real redraw loop against a changing ammo vector. The tree
    # footer follows the enlarged inline array; an old footer offset would
    # dereference a slot instead and this fixture would fail.
    ammo=alloc(0x20); ammo_vt=alloc(0x60); vector=alloc(24)
    records=alloc((count+5)*0x48); pq(ammo,ammo_vt)
    return_ammo=alloc(16); put(return_ammo,b"\x48\xb8"+struct.pack("<Q",ammo)+b"\xc3"); pq(ai_vt+LOFF['pool_get'],return_ammo)
    return_vector=alloc(16); put(return_vector,b"\x48\xb8"+struct.pack("<Q",vector)+b"\xc3"); pq(ammo_vt+0x48,return_vector)
    pq(ai_vt+LOFF['gunner_count'],noop)
    sentinel=alloc(0x30); pq(sentinel+8,sentinel); C.c_ubyte.from_address(sentinel+0x19).value=1
    pq(menu+0x7f8+delta,sentinel)
    fills=[]; fill_records=[]; hides=[]; shown=None; companions=[]
    @cb(None,C.c_void_p,C.c_size_t,C.c_void_p)
    def fill(_,index,record):
        fills.append((index,record))
        fill_records.append(C.string_at(record,0x48))
        if shown is not None:
            C.c_ubyte.from_address(widgets[index]+0x5b).value = index in shown
    @cb(None,C.c_void_p,C.c_size_t)
    def hide_slot(_,index):
        hides.append(index)
        if shown is not None:
            C.c_ubyte.from_address(widgets[index]+0x5b).value = 0
    stub(address(0x3f1d0),fill); stub(address(0x3f800),hide_slot); put(address(0x3e890),b"\xc3")
    redraw=C.CFUNCTYPE(None,C.c_void_p,C.c_void_p)(address(0x3ed10))
    for length in [0,1,9,10,count,count+5,2]:
        pq(vector,records); pq(vector+8,records+length*0x48)
        fills.clear(); hides.clear(); redraw(menu,entity)
        assert fills==[(i,records+i*0x48) for i in range(min(length,count))],(length,fills)
        assert hides==list(range(min(length,count),count)),(length,hides)
        assert C.string_at(menu+size,64)==b"G"*64

    # Stock column-right positioning is already a three-row grid. Exercise
    # that exact machine code, observing the widget's virtual position setter.
    parent=alloc(0x180); parent_vt=alloc(0x80); pq(parent,parent_vt)
    parent_rect=alloc(16); put(parent_rect,struct.pack("<4i",647,0,727,1062))
    widget=alloc(0x180); vt=alloc(0x80); pq(widget,vt); pq(widget+0x38,parent)
    widget_rect=alloc(16); put(widget_rect,struct.pack("<4i",0,0,88,68))
    rp=alloc(16); put(rp,b"\x48\xb8"+struct.pack("<Q",parent_rect)+b"\xc3"); pq(parent_vt+0x40,rp)
    rw=alloc(16); put(rw,b"\x48\xb8"+struct.pack("<Q",widget_rect)+b"\xc3"); pq(vt+0x40,rw)
    positions=[]
    @cb(None,C.c_void_p,C.c_void_p)
    def position(_,point): positions.append(struct.unpack("<2i",C.string_at(point,8)))
    pq(vt+0x30,position); pq(vt+0x68,noop)
    put(address(0x2d2bf0),b"\x48\x8b\xc2\xc3")
    layout=C.CFUNCTYPE(None,C.c_void_p,C.c_void_p)(address(0x3b590))
    slot=alloc(0xb8)
    for index in range(count):
        C.c_int32.from_address(slot+0xc).value=index
        layout(slot,widget)
        assert positions[-1]==(647+(index//3)*90,1062-(index%3+1)*70),(index,positions[-1])

    # Sparse restored squads must have dense display positions while retaining
    # their original physical slots, record pointers, hover and click indices.
    positions_by_widget={}
    @cb(C.c_void_p,C.c_void_p)
    def actual_rect(widget): return widget+0x100
    @cb(None,C.c_void_p,C.c_void_p)
    def moved(widget,point):
        dx,dy=struct.unpack("<2i",C.string_at(point,8))
        x,y,r,b=struct.unpack("<4i",C.string_at(widget+0x100,16))
        put(widget+0x100,struct.pack("<4i",x+dx,y+dy,r+dx,b+dy))
        positions_by_widget[widget]=(x+dx,y+dy)
    pq(vt+0x30,moved); pq(vt+0x40,actual_rect)
    for index in range(count):
        slot=menu+0x180+index*0xb8
        pair=[]
        x,y=647+(index//3)*90,1062-(index%3+1)*70
        for child in range(2):
            w=alloc(0x180); pq(w,vt)
            put(w+0x100,struct.pack("<4i",x+child*5,y+child*7,x+88,y+68))
            pair.append(w)
        pq(pair[0]+0x38,parent); pq(pair[1]+0x38,pair[0])
        children=alloc(8); pq(children,pair[1])
        pq(pair[0]+0x40,children); pq(pair[0]+0x48,children+8)
        widgets[index]=pair[0]; companions.append(pair[1])
        entries=alloc(16); pq(entries,pair[0]); pq(entries+8,pair[1])
        pq(slot+0x80,entries); pq(slot+0x88,entries+16)
        pq(slot+0x18,pair[0])
        C.c_int32.from_address(slot+0xc).value=index
    for visible_indices in [[],[count-1],[0,2,4,count-1],list(range(count)),[1,3],[]]:
        shown=set(visible_indices)
        pq(vector,records); pq(vector+8,records+count*0x48)
        for repeat in range(25):
            positions_by_widget.clear()
            redraw(menu,entity)
            if repeat:
                assert not positions_by_widget, "unchanged frames must not move cards again"
            for display,index in enumerate(sorted(shown)):
                expected=(647+(display//3)*90,1062-(display%3+1)*70)
                assert struct.unpack("<2i",C.string_at(widgets[index]+0x100,8))==expected
                assert struct.unpack("<2i",C.string_at(companions[index]+0x100,8))==(expected[0]+5,expected[1]+7)
                slot=menu+0x180+index*0xb8
                assert C.c_int32.from_address(slot+0xc).value==display
                C.c_uint32.from_address(slot+8).value=0
                C.CFUNCTYPE(None,C.c_void_p,C.c_void_p)(click)(menu,widgets[index])
                assert clicked[-1]==(index,1),(index,clicked[-1])
        assert C.string_at(menu+size,64)==b"G"*64

    if combined:
        shown=None
        pq(vt+0x48,visible)
        stub(address(0x3bba0),noop)
        selected_entities=[]
        world=alloc(0x20); wvt=alloc(0x710); pq(world,wvt)
        context=alloc(0x220); cvt=alloc(0xe0); pq(context,cvt)
        player=alloc(8); manager=alloc(0x70); registry=alloc(16)
        pq(manager+0x40,registry); pq(manager+0x48,registry+16)
        # Use the actual supported LogicHybridServer player getter, not a
        # callback that assumes the same object layout as production code.
        logic=Image(ROOT/LOGIC[build_index])
        matches=Rtti(logic).find("LogicHybridServer@")
        assert len(matches)==1
        server_vtables=[v for _,vs in matches[0][2] for v in vs]
        assert len(server_vtables)==1
        server_vt=server_vtables[0]
        getter=logic.u64(server_vt+0x40)-logic.base
        getter_code=logic.read(getter,8)
        assert getter_code==bytes.fromhex("488b8118020000c3")
        player_of=alloc(16); put(player_of,getter_code)
        pq(context+0x218,player)
        pq(cvt+0xd0,noop) # native constructor's optional recording service
        wrong_calls=[]
        @cb(C.c_void_p,C.c_void_p)
        def not_a_player_getter(value):
            wrong_calls.append(value)
            return 0
        pq(wvt+0x40,not_a_player_getter)
        @cb(C.c_void_p,C.c_void_p,C.c_void_p)
        def manager_of(w,p): assert (w,p)==(world,player); return manager
        pq(cvt+0x40,player_of); pq(wvt+LOFF['world_player'],manager_of)
        # Execute the real AmmunitionMenu constructor's service-field setup.
        # This is deliberately not pq(menu+128/130, ...): that old fixture
        # mirrored the production bug and masked a mission-load crash.
        owner=alloc(0x160)
        owner_shift=0x20 if build_index in (1,3) else 0
        pq(owner+0x100+owner_shift,world)
        pq(owner+0x118+owner_shift,context)
        pq(menu+0x120,owner)
        # The constructor moved between builds but its bytes (no relative
        # operands) are the same, so find it by content rather than by rva.
        ctor=bytes.fromhex(CONSTRUCTOR)
        at=module.image.find(ctor)
        if at<0: at=module.image.find(bytes.fromhex(CONSTRUCTOR_SHIFTED))
        # Steam shifted the owner fields, so its constructor bytes differ;
        # there it is at the reference rva, as before.
        if at<0: at=0x3df21
        setup=module.image[at:at+len(ctor)]
        instructions=list(module.md.disasm(setup,at))
        assert instructions[-1].address+instructions[-1].size==at+len(ctor)
        assert all(not any(op.type==3 and ins.reg_name(op.mem.base)=="rip"
                           for op in ins.operands) for ins in instructions)
        # Preserve RSI and reserve the Win64 shadow area around the slice.
        code=bytes.fromhex("564883ec204889ce")+setup+bytes.fromhex("4883c4205ec3")
        initialize=alloc(len(code)); put(initialize,code)
        C.CFUNCTYPE(None,C.c_void_p)(initialize)(menu)
        assert (q(menu+0x128),q(menu+0x130))==(context,world)
        ai_records={}; actions=[]
        @cb(C.c_ubyte,C.c_void_p,C.c_uint32)
        def infantry(_,flag): return flag==0x10
        @cb(C.c_size_t,C.c_void_p)
        def owned(_): return 1
        @cb(C.c_void_p,C.c_void_p)
        def entity_facets(e): return q(e+8)
        @cb(C.c_void_p,C.c_void_p)
        def ai_pool(a): return q(a+8)
        @cb(C.c_void_p,C.c_void_p)
        def pool_vector(p): return q(p+8)
        @cb(C.c_uint32,C.c_void_p,C.c_size_t)
        def disabled(a,i): return C.c_uint32.from_address(ai_records[a]+i*0x48+0x3c).value
        @cb(None,C.c_void_p,C.c_size_t,C.c_uint32)
        def toggle(a,i,value):
            actions.append((a,i,value))
            C.c_uint32.from_address(ai_records[a]+i*0x48+0x3c).value=value
        @cb(C.c_void_p,C.c_void_p)
        def plus8(p): return q(p+8)
        @cb(C.c_size_t,C.c_void_p)
        def one(_): return 1
        @cb(C.c_size_t,C.c_void_p)
        def two(_): return 2
        @cb(C.c_void_p,C.c_void_p,C.c_size_t)
        def indexed_gun(p,i): return q(q(p+8)+8*i)
        @cb(C.c_uint8,C.c_void_p,C.c_void_p)
        def compatible(p,ammo): return q(p+8)==ammo
        @cb(C.c_float,C.c_void_p)
        def reload_progress(p): return C.c_float.from_address(p+16).value
        labels={}; progress_updates=[]
        @cb(None,C.c_void_p,C.c_void_p)
        def label_text(w,s): labels[w]=C.string_at(q(s),q(s+16)).decode()
        @cb(None,C.c_void_p)
        def update_progress(w): progress_updates.append(w)
        stub(address(0x2cb730),label_text)
        stub(address(0x2c3380),update_progress)
        for i in range(count):
            slot=menu+0x180+i*0xb8
            for off in [0x30,0x40,0x48,0x50]:
                w=alloc(0x200); pq(w,vt); pq(slot+off,w)
        member_selections=[]
        ammo100_guns=[]
        for n,ids in enumerate([[100,200],[300,100]]):
            e=alloc(0x20); ev=alloc(0xc0); f=alloc(0x60); pq(e,ev); pq(e+8,f)
            pq(ev+0xb0,entity_facets); pq(ev+0x98,infantry)
            sf=alloc(0x40); C.c_ubyte.from_address(sf+0x18).value=1; pq(f+0x50,sf)
            team=alloc(8); tv=alloc(0x90); pq(team,tv); pq(tv+0x80,owned); pq(f+0x20,team)
            a=alloc(0x20); av=alloc(0x400); pq(a,av); pq(f+0x28,a)
            p=alloc(0x20); pv=alloc(0x60); v=alloc(24); rs=alloc(0x90)
            pq(a+8,p); pq(p,pv); pq(p+8,v); pq(v,rs); pq(v+8,rs+0x90)
            pq(av+LOFF['pool_get'],ai_pool); pq(av+0x3e8,disabled); pq(av+LOFF['ai_set'],toggle); pq(pv+0x48,pool_vector)
            # A real member roster, gunner and gun vtables exercise recipient
            # filtering and reload reads, instead of baking totals into pools.
            roster=alloc(16); rv=alloc(0x70); header=alloc(16); entries=alloc(8)
            pq(roster,rv); pq(roster+8,header); pq(rv+0x68,plus8)
            pq(header,entries); pq(header+8,entries+8)
            pq(a+16,roster)
            @cb(C.c_void_p,C.c_void_p)
            def roster_of(p): return q(p+16)
            pq(av+LOFF['roster'],roster_of)
            soldier=alloc(16); sv=alloc(0xc0); facets2=alloc(0x60)
            pq(soldier,sv); pq(soldier+8,facets2); pq(sv+0xb0,entity_facets); pq(entries,soldier)
            selection=alloc(0x40); selection_vt=alloc(0x60)
            pq(selection,selection_vt); pq(selection_vt+0x58,one)
            C.c_ubyte.from_address(selection+0x18).value=1; pq(selection+0x28,sf)
            pq(facets2+0x50,selection)
            member_selections.append(selection)
            soldier_ai=alloc(16); sav=alloc(0x140); gunner=alloc(16); gv=alloc(0x100); gun_entries=alloc(16)
            pq(facets2+0x28,soldier_ai); pq(soldier_ai,sav); pq(soldier_ai+8,gunner)
            # A live but individually unselected squadmate must not inflate
            # the count or contribute progress for a partial selection.
            if n==0:
                second=alloc(16); second_facets=alloc(0x60); second_selection=alloc(0x40)
                second_vt=alloc(0x60); pq(second_vt+0x58,noop)
                pq(second,sv); pq(second+8,second_facets); pq(second_facets+0x28,soldier_ai)
                pq(second_facets+0x50,second_selection); pq(second_selection,second_vt)
                pq(second_selection+0x28,sf); C.c_ubyte.from_address(second_selection+0x18).value=1
                entries2=alloc(16); pq(entries2,soldier); pq(entries2+8,second)
                pq(header,entries2); pq(header+8,entries2+16)
            pq(sav+LOFF['gunner_count'],one)
            @cb(C.c_void_p,C.c_void_p,C.c_size_t)
            def gunner_of(p,i): return q(p+8)
            pq(sav+LOFF['gunner_get'],gunner_of); pq(gunner,gv); pq(gunner+8,gun_entries)
            pq(gv+0xf0,two); pq(gv+0xf8,indexed_gun)
            for i,identity in enumerate(ids):
                gun=alloc(24); gun_vt=alloc(0x160); pq(gun,gun_vt); pq(gun+8,identity)
                C.c_float.from_address(gun+16).value=0.25+n*0.5
                pq(gun_vt+0x148,compatible); pq(gun_vt+0x158,plus8); pq(gun_vt+0xc8,reload_progress)
                pq(gun_entries+i*8,gun)
                if identity==100: ammo100_guns.append(gun)
                pq(rs+i*0x48,identity)
                for off,value in [(0x28,100),(0x2c,40+n),(0x34,1),(0x3c,n)]:
                    C.c_uint32.from_address(rs+i*0x48+off).value=value
            ai_records[a]=rs; selected_entities.append((e,a))
            pq(registry+n*8,e)
        fill_records.clear()
        redraw(menu,entity)
        assert not wrong_calls, "world utility was called as a player context"
        union=fill_records[-3:]
        assert [struct.unpack_from("<Q",r)[0] for r in union]==[100,200,300]
        assert struct.unpack_from("<I",union[0],0x2c)[0]==81
        assert struct.unpack_from("<I",union[0],0x28)[0]==200
        first=menu+0x180
        assert labels[q(first+0x40)]=="1/2"
        assert C.c_uint32.from_address(q(first+0x30)+0x1a0).value==0xffffc04d
        assert abs(C.c_float.from_address(q(first+0x48)+0x1a8).value-0.25)<0.001
        assert abs(C.c_float.from_address(q(first+0x50)+0x1a8).value-0.405)<0.001
        C.CFUNCTYPE(None,C.c_void_p,C.c_void_p)(click)(menu,widgets[0])
        assert actions==[(selected_entities[0][1],0,0),(selected_entities[1][1],1,0)],actions
        redraw(menu,entity); actions.clear()
        assert labels[q(first+0x40)]=="2"
        assert abs(C.c_float.from_address(q(first+0x48)+0x1a8).value-0.5)<0.001
        # Ready guns report 1.0 in the real getter; exclude them, while zero
        # remains visible for a reload that has just started.
        C.c_float.from_address(ammo100_guns[0]+16).value=1.0
        redraw(menu,entity)
        assert abs(C.c_float.from_address(q(first+0x48)+0x1a8).value-0.75)<0.001
        C.c_float.from_address(ammo100_guns[1]+16).value=1.0
        redraw(menu,entity)
        assert C.c_ubyte.from_address(q(first+0x48)+0x5b).value==0
        C.c_float.from_address(ammo100_guns[0]+16).value=0.0
        redraw(menu,entity)
        assert C.c_ubyte.from_address(q(first+0x48)+0x5b).value==1
        assert C.c_float.from_address(q(first+0x48)+0x1a8).value==0.0
        C.c_float.from_address(ammo100_guns[0]+16).value=0.25
        C.c_float.from_address(ammo100_guns[1]+16).value=0.75
        C.CFUNCTYPE(None,C.c_void_p,C.c_void_p)(click)(menu,widgets[0])
        assert actions==[(selected_entities[0][1],0,1),(selected_entities[1][1],1,1)],actions
        redraw(menu,entity)
        assert C.c_ubyte.from_address(q(first+0x48)+0x5b).value==0
        # An individually pinned user can still enable the type when the
        # shared pool is disabled. Its state must contribute to the count.
        sf=member_selections[0]
        C.c_ubyte.from_address(sf+0x19).value=0xa5
        C.c_ubyte.from_address(sf+0x1e).value=1
        C.c_ubyte.from_address(sf+0x1f).value=0
        redraw(menu,entity)
        assert labels[q(first+0x40)]=="1/2"
        C.c_ubyte.from_address(sf+0x19).value=0
        actions.clear(); pq(manager+0x48,registry+8)
        C.CFUNCTYPE(None,C.c_void_p,C.c_void_p)(click)(menu,widgets[0])
        assert not actions, "changed selection must refuse stale card clicks"
        pq(manager+0x48,registry+16)
        # Transient loading/teardown: missing context, world or player must
        # retain the original focused redraw and reject stale union clicks.
        for at in [menu+0x128,menu+0x130,context+0x218]:
            redraw(menu,entity)
            saved=q(at); pq(at,0); actions.clear()
            C.CFUNCTYPE(None,C.c_void_p,C.c_void_p)(click)(menu,widgets[0])
            assert not actions, "missing context must refuse cached union clicks"
            fill_records.clear(); redraw(menu,entity)
            assert len(fill_records)==count, "missing context must use focused rendering"
            pq(at,saved)
        assert not wrong_calls

    # The array cleanup helper must pass the expanded count, not nine.
    destroyed=[]
    @cb(None,C.c_void_p,C.c_size_t,C.c_size_t,C.c_void_p)
    def destroy(at,stride,n,destructor):destroyed.append((at,stride,n))
    stub(address(0x4994b8),destroy)
    C.CFUNCTYPE(None,C.c_void_p)(address(0x3e4d0))(menu+0x180)
    assert destroyed==[(menu+0x180,0xb8,count)]
    print(f"PASS build={build_index} columns={columns} combined={combined}: actual DLL preflight/rollback, stock redraw/layout/hide/hover/click/cleanup, tail canary",flush=True)

if __name__=="__main__":
    if len(sys.argv)>1:
        case(int(sys.argv[1]),int(sys.argv[2]),len(sys.argv)>3)
    else:
        for build in [0,1,2,3]:
            if not (ROOT/PATHS[build]).exists():
                continue
            for columns in [3,4,12,42]:
                for combined in [False,True]:
                    args=[sys.executable,__file__,str(build),str(columns)]
                    if combined: args.append("combined")
                    subprocess.run(args,check=True,timeout=240)
