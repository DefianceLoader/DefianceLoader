"""Exercise the built render-sync hook with the real stock record rebind.

Shot emission and World2 attachment callbacks are simulated. The actual
logic.dll model search, attachment callback dispatch, and record swap execute.
Only privately mapped images are changed; no installed file or live game is
touched. Callback shims retain the known World2 method addresses so the plugin
can still check the real stock prefixes during initialization.
"""
import ctypes as C
import pathlib
import struct
import subprocess
import sys
from moving_test_memory import K, KEEP, alloc, p64, i32, f32, check
from test_moving_actions_sync import actor
from moving_actions_render_sync_bindings import profiles

ROOT = pathlib.Path(__file__).resolve().parents[1]
DLL = ROOT/'plugins/moving-actions-render-sync/target/release/defiance_plugin_moving_actions_render_sync.dll'
PROFILE = None
LOGIC = WORLD = None
SHOT = PRIMARY_SHOT = GUNNER_TICK = CLIENT_TICK = 0
ATTACH, DETACH = 0x154D40, 0x1551A0
K.VirtualProtect.argtypes=[C.c_void_p,C.c_size_t,C.c_uint32,C.POINTER(C.c_uint32)]
K.FlushInstructionCache.argtypes=[C.c_void_p,C.c_void_p,C.c_size_t]
SHOT_FN=C.CFUNCTYPE(None,C.c_void_p)
TICK_FN=C.CFUNCTYPE(None,C.c_void_p,C.c_uint8,C.c_uint8,C.c_uint32,C.c_float)


def q(p, off=0): return C.c_uint64.from_address(p+off).value


def image_size(path):
    data=path.read_bytes(); pe=int.from_bytes(data[0x3c:0x40],'little')
    return int.from_bytes(data[pe+24+56:pe+24+60],'little')


def replace(address, callback):
    old=C.c_uint32()
    code=b'\x48\xb8'+struct.pack('<Q',C.cast(callback,C.c_void_p).value)+b'\xff\xe0'
    saved=C.string_at(address,len(code))
    check(K.VirtualProtect(address,len(code),0x40,C.byref(old)), 'VirtualProtect')
    C.memmove(address,code,len(code))
    K.FlushInstructionCache(C.c_void_p(-1),address,len(code))
    check(K.VirtualProtect(address,len(code),old.value,C.byref(C.c_uint32())), 'restore protection')
    return saved


def fixture(logic, world, attached=False, selected=True, action=4, state=2, category=1, known=True, front_correct=True, stale_child=False, client=False):
    g,models,animation,descriptor,old,script=actor(0.0)
    gunner_vt = PROFILE.bindings['gunner_client_vt'] if client else PROFILE.bindings['gunner_vt']
    p64(g,0,logic+gunner_vt); p64(animation,0,logic+PROFILE.bindings['animation_vt'])
    i32(g,0x90,state); i32(animation,0x6c,action)
    gun=q(q(g,0x38),8); other=q(q(g,0x38))
    unit=q(g,0x20)
    getcomp=C.CFUNCTYPE(C.c_void_p,C.c_void_p)(q(q(unit),0xb0))
    comp=getcomp(unit); geometry=q(comp,8)
    ai,junction,owner=alloc(0x220),alloc(0x30),alloc(0x30)
    p64(comp,0x28,ai); p64(ai,0x1f0,junction); p64(junction,0x10,g)
    p64(owner,0x10,unit); p64(gun,0x20,owner)
    i32(gun,0xdc,8); i32(gun,0x140,1)
    if not selected: i32(g,0x94,0)
    C.memmove(descriptor+0x1a8,b'model_x\0',8)
    p64(descriptor,0x1b8,7); p64(descriptor,0x1c0,15)
    i32(descriptor,0xc4,category)
    node,oldnode,hands,holster=alloc(0x400),alloc(0x400),alloc(0x400),alloc(0x400)
    nodevt,parentvt=alloc(0x220),alloc(0x220)
    p64(node,0,nodevt); p64(oldnode,0,nodevt); p64(hands,0,parentvt); p64(holster,0,parentvt)
    p64(nodevt,0x208,world+DETACH if known else 0)
    p64(parentvt,0x1f8,world+ATTACH)
    TRANSFORM=C.CFUNCTYPE(None,C.c_void_p,C.c_void_p)
    @TRANSFORM
    def transform(_node,_matrix): pass
    KEEP.append(transform); p64(nodevt,0x140,C.cast(transform,C.c_void_p).value)
    refs=[alloc(0x20) for _ in range(4)]
    for ref in refs: i32(ref,8,100)
    p64(geometry,0x158,hands); p64(geometry,0x160,refs[0])
    p64(geometry,0x178,holster); p64(geometry,0x180,refs[1])
    if front_correct:
        p64(models,0,descriptor); p64(models,0x20,node); p64(models,0x28,refs[2])
        p64(models,0xb0,old); p64(models,0xd0,oldnode); p64(models,0xd8,refs[3])
    else:
        p64(models,0x20,oldnode); p64(models,0x28,refs[3])
        p64(models,0xd0,node); p64(models,0xd8,refs[2])
    p64(node,0x368,hands if attached else holster)
    p64(oldnode,0x368,holster if attached else hands)
    p64(hands,0x378,node if attached and not stale_child else oldnode)
    p64(holster,0x378,oldnode if attached else node)
    return gun,g,models,descriptor,node,hands,holster


def main(profile):
    global PROFILE, LOGIC, WORLD, SHOT, PRIMARY_SHOT, GUNNER_TICK, CLIENT_TICK
    PROFILE = profile
    LOGIC, WORLD = pathlib.Path(profile.logic), pathlib.Path(profile.world)
    SHOT = profile.bindings['shot']; PRIMARY_SHOT = profile.bindings['primary_shot']
    GUNNER_TICK = profile.bindings['gunner_tick']; CLIENT_TICK = profile.bindings['client_tick']
    logic=K.LoadLibraryExW(str(LOGIC),None,1); world=K.LoadLibraryExW(str(WORLD),None,1)
    check(logic and world,'map images')
    logs,hooks,shots,attachments,ticks=[],[],[],[],[]
    LOG=C.CFUNCTYPE(None,C.c_uint32,C.c_char_p)
    BASE=C.CFUNCTYPE(C.c_void_p,C.c_char_p)
    SIZE=C.CFUNCTYPE(C.c_size_t,C.c_void_p)
    HOOK=C.CFUNCTYPE(C.c_int,C.c_void_p,C.c_void_p,C.POINTER(C.c_void_p))
    behavior={'refuse':False,'size':True}
    @LOG
    def log(level,text): logs.append(f'[{level}] {text.decode()}')
    @BASE
    def base(name): return {b'logic.dll':logic,b'world2.dll':world}.get(name)
    @SIZE
    def size(base):
        return image_size(LOGIC if base==logic else WORLD) if behavior['size'] else 0
    @SHOT_FN
    def original(gun):
        shots.append(gun)
        i32(gun,0xdc,C.c_int32.from_address(gun+0xdc).value-1)
    @TICK_FN
    def original_tick(g,a,b,c,dt): ticks.append((g,a,b,c,dt))
    @HOOK
    def hook(target,detour,out):
        if behavior['refuse']: return 1
        hooks.append((target,detour)); out[0]=C.cast(original if target in (logic+SHOT,logic+PRIMARY_SHOT) else original_tick,C.c_void_p).value
        return 0
    class Api(C.Structure):
        _fields_=[('abi',C.c_uint32),('reserved',C.c_uint32)]+[(n,C.c_void_p) for n in
            ('log','base','size','find','find_at','hook','hook_exact','hook_call','unhook','rtti','vt','config','patch')]
    api=Api(5,0,*[C.cast(fn,C.c_void_p).value if fn else None for fn in
        (log,base,size,None,None,hook,None,None,None,None,None,None,None)])
    class Plugin(C.Structure):
        _fields_=[('abi',C.c_uint32),('name',C.c_char_p),('version',C.c_char_p),('init',C.c_void_p),('stop',C.c_void_p)]
    lib=C.CDLL(str(DLL)); lib.defiance_plugin.restype=C.c_void_p
    plugin=C.cast(lib.defiance_plugin(),C.POINTER(Plugin)).contents
    init=C.CFUNCTYPE(C.c_int,C.POINTER(Api))(plugin.init)
    check(init(None)!=0,'null API')
    bad=Api.from_buffer_copy(api); bad.abi=4
    check(init(C.byref(bad))!=0,'wrong ABI')
    behavior['size']=False; check(init(C.byref(api))!=0 and not hooks,'size refusal')
    behavior.update(size=True,refuse=True); check(init(C.byref(api))!=0 and not hooks,'hook refusal')
    behavior['refuse']=False
    check(init(C.byref(api))==0,f'initialization {logs}')
    check(any(s.startswith('[0] moving weapon render installed: logic.dll sha256=') for s in logs),f'missing info install line {logs}')
    check([p[0] for p in hooks]==[logic+SHOT,logic+PRIMARY_SHOT,logic+GUNNER_TICK,logic+CLIENT_TICK],f'hook target/count for {profile.name}')
    detour=SHOT_FN(hooks[0][1])

    # These two callbacks simulate World2 render-node linking, while the stock
    # logic.dll rebind callback dispatch and record swapping execute unchanged.
    ATTACH_FN=C.CFUNCTYPE(None,C.c_void_p,C.c_void_p)
    @ATTACH_FN
    def attach(parent,shared):
        node=q(shared); p64(node,0x368,parent); p64(parent,0x378,node)
        attachments.append((parent,node))
    @SHOT_FN
    def detach(node):
        parent=q(node,0x368)
        if parent and q(parent,0x378)==node: p64(parent,0x378,0)
        p64(node,0x368,0)
    KEEP.extend((log,base,size,hook,original,original_tick,attach,detach))
    replace(world+ATTACH,attach); replace(world+DETACH,detach)
    for front_correct in (True,False):
        gun,g,models,desc,node,hands,holster=fixture(logic,world,front_correct=front_correct)
        start=len(shots)
        detour(gun)
        check(shots[start:]==[gun] and C.c_int32.from_address(gun+0xdc).value==7,'shot forwarding')
        check(q(models)==desc and q(models,0x20)==node,'stock record swap')
        check(q(node,0x368)==hands and q(hands,0x378)==node,'stale actual binding not repaired')
        count=len(attachments); detour(gun)
        check(len(attachments)==count,'matching binding was reapplied')
        print(f'PASS stale render-node binding repaired with real stock rebind; front_correct={front_correct}',flush=True)
    check(any('moving weapon mismatch:' in s and 'rebound=true repaired=true' in s for s in logs),'repair report')
    gun,g,models,desc,node,hands,holster=fixture(logic,world,attached=True,stale_child=True)
    detour(gun)
    check(q(node,0x368)==hands and q(hands,0x378)==node,'stale reciprocal child not repaired')
    print('PASS matching weapon parent with stale attachment child repaired',flush=True)
    gun,g,models,desc,node,hands,holster=fixture(logic,world,client=True)
    detour(gun)
    check(q(node,0x368)==hands and q(hands,0x378)==node,'client gunner binding not repaired')
    print('PASS client gunner binding path',flush=True)
    for kwargs in ({'attached':True},{'selected':False},{'action':0x18},{'action':0x17},
                   {'action':0x2b},{'action':0x99},{'state':4},{'category':5},{'known':False}):
        gun,g,models,desc,node,hands,holster=fixture(logic,world,**kwargs)
        before=q(node,0x368); count=len(attachments); start=len(shots)
        detour(gun)
        check(q(node,0x368)==before and len(attachments)==count,f'unsafe rebind {kwargs}')
        check(shots[start:]==[gun],f'original suppressed {kwargs}')
    print('PASS selected-gun guard, grenade/change/state/category/type guards, idempotence, original emission forwarding, init guards')
    # A rejected actor must still fire once and explain which guard rejected it.
    for stage in ('animation', 'gunner', 'membership', 'models'):
        gun,g,models,desc,node,hands,holster=fixture(logic,world,attached=True)
        unit=q(g,0x20)
        getcomp=C.CFUNCTYPE(C.c_void_p,C.c_void_p)(q(q(unit),0xb0))
        comp=getcomp(unit)
        reason={'animation':'animation-type','gunner':'gunner-type',
                'membership':'gun-not-in-owner-list','models':'descriptor-not-in-models'}[stage]
        if stage=='animation': p64(q(comp,0x58),0,logic+PROFILE.bindings['animation_vt']+8)
        elif stage=='gunner': p64(g,0,logic+PROFILE.bindings['gunner_vt']+8)
        elif stage=='membership': p64(q(g,0x38),8,q(q(g,0x38)))
        else: p64(models,0,q(models,0xb0))
        start=len(shots); count=len(attachments); logstart=len(logs)
        detour(gun)
        check(shots[start:]==[gun] and len(attachments)==count,'rejected snapshot changed shot/binding')
        check(any(s.startswith('[3] moving weapon render skipped:') and f'reason={reason}' in s for s in logs[logstart:]),f'missing debug skip diagnosis {reason}')
    check(not any('moving weapon render hook reached:' in s for s in logs),'routine hook-arrival log remains')
    check(not any('moving weapon render:' in s for s in logs),'routine successful-shot log remains')
    print('PASS rejected-snapshot anomaly diagnostics; normal firing stays quiet and forwards once')
    for client in (False,True):
        gun,g,models,desc,node,hands,holster=fixture(logic,world,attached=False,action=5,client=client)
        observer=TICK_FN(hooks[3 if client else 2][1])
        start=len(shots); count=len(attachments); tickstart=len(ticks); logstart=len(logs)
        observer(g,17,29,0xAABBCCDD,0.25)
        check(ticks[tickstart:]==[(g,17,29,0xAABBCCDD,0.25)],'tick arguments/count changed')
        check(len(shots)==start and len(attachments)==count,'read-only observer fired or rebound')
        check(any('moving weapon mismatch observed:' in s and 'action=0x5' in s for s in logs[logstart:]),'prone observer missed loaded mismatch')
        check(not any('moving weapon inventory:' in s or 'moving weapon watch:' in s for s in logs[logstart:]),'routine watch or inventory dump remains')
        for _ in range(100): observer(g,17,29,0xAABBCCDD,0.25)
        check(sum('moving weapon mismatch observed:' in s for s in logs[logstart:])==1,'unchanged mismatch was logged repeatedly')
    print('PASS both gunner observer hooks: arguments forwarded once; mismatch logged once without routine watch/inventory output')
    gun,g,models,desc,node,hands,holster=fixture(logic,world,attached=True,action=5)
    unit=q(g,0x20); getcomp=C.CFUNCTYPE(C.c_void_p,C.c_void_p)(q(q(unit),0xb0)); comp=getcomp(unit)
    observer=TICK_FN(hooks[2][1]); quiet_start=len(logs)
    for _ in range(8): observer(g,1,2,3,0.25)
    check(not any('moving weapon mismatch' in s or 'moving weapon watch' in s or 'moving weapon inventory' in s for s in logs[quiet_start:]),'ordinary update emitted diagnostic output')
    print('PASS stable ordinary watch remains silent while update checks continue')
    gun,g,models,desc,node,hands,holster=fixture(logic,world)
    start=len(shots)
    SHOT_FN(hooks[1][1])(gun)
    check(shots[start:]==[gun] and q(node,0x368)==hands,'alternate shot forwarding/guarded rebind')
    print('PASS primary vfunc27 shot handler forwards original once and checks stale binding')
    for front_correct in (False,True):
        gun,g,models,desc,node,hands,holster=fixture(logic,world,action=1,front_correct=front_correct)
        unit=q(g,0x20); getcomp=C.CFUNCTYPE(C.c_void_p,C.c_void_p)(q(q(unit),0xb0)); comp=getcomp(unit); animation=q(comp,0x58)
        i32(animation,0x68,1)
        observer=TICK_FN(hooks[2][1]); shotstart=len(shots); logstart=len(logs)
        before=(q(g,0x88),C.c_int32.from_address(g+0x90).value,C.c_int32.from_address(g+0x94).value,C.c_float.from_address(g+0x9c).value,q(gun,0xc8),C.c_int32.from_address(gun+0xdc).value)
        observer(g,1,2,3,0.25)
        check(q(node,0x368)==holster,'repair occurred before confirming persistence')
        observer(g,1,2,3,1.0)
        check(q(models)==desc and q(node,0x368)==hands and q(hands,0x378)==node,'loaded mismatch not repaired')
        after=(q(g,0x88),C.c_int32.from_address(g+0x90).value,C.c_int32.from_address(g+0x94).value,C.c_float.from_address(g+0x9c).value,q(gun,0xc8),C.c_int32.from_address(gun+0xdc).value)
        check(before==after and len(shots)==shotstart,'loaded repair changed combat state/timer/ammo')
        if not front_correct: check(q(animation,0xb8)==q(desc,0x1c8),'stock animation script not handed off')
        check(any(s.startswith('[3] moving weapon loaded repair:') and 'repaired=true' in s for s in logs[logstart:]),'missing debug loaded repair report')
        count=len(attachments); observer(g,1,2,3,1.0)
        check(len(attachments)==count,'loaded repair repeated after correction')
    print('PASS persistent loaded mismatch: stock model and animation script repair after confirmation; timer/selection/targets/ammo preserved; no shot required')
    for action,requested,state,remaining in [(0x17,0x17,2,0.0),(0x18,0x18,4,0.0),(1,1,4,0.0),(1,3,2,0.0),(1,1,2,0.5),(1,1,2,float('nan'))]:
        gun,g,models,desc,node,hands,holster=fixture(logic,world,action=action,state=state,front_correct=False)
        unit=q(g,0x20); getcomp=C.CFUNCTYPE(C.c_void_p,C.c_void_p)(q(q(unit),0xb0)); animation=q(getcomp(unit),0x58)
        i32(animation,0x68,requested); f32(g,0x9c,remaining)
        observer=TICK_FN(hooks[2][1]); count=len(attachments)
        observer(g,1,2,3,0.25); observer(g,1,2,3,1.0)
        check(len(attachments)==count and q(node,0x368)==holster,'loaded repair ignored transition/timer guard')
    print('PASS loaded repair excludes active grenade/switch/transition actions and unfinished or invalid switch timers')
    print(f'PASS render-sync fixture for {profile.name} ({profile.sha})',flush=True)
    K.FreeLibrary(world); K.FreeLibrary(logic)


if __name__=='__main__':
    available = profiles()
    if len(sys.argv) > 1:
        if len(sys.argv) == 3 and sys.argv[1] == '--build':
            requested = sys.argv[2]
        elif len(sys.argv) == 2:
            requested = sys.argv[1]
        else:
            raise SystemExit('usage: test_moving_actions_render_sync.py [--build <build>]')
        chosen = next((p for p in available if p.name == requested), None)
        check(chosen is not None, f'unknown build {requested}')
        main(chosen)
    else:
        for profile in available:
            subprocess.run([sys.executable, __file__, '--build', profile.name], check=True)
