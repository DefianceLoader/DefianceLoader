"""Exercise the release scrolling DLL in disposable native fixtures for both builds.

Real mapped game images validate hash/preflight/vtables. Engine allocation,
rendering and inventory services are stubbed AFTER install, never during
preflight. The production detours and native event dispatch remain executable.
No installed game files are changed. Requires local game binaries.

Each case patches one panel class: `squad` (UnitManagerSquadInfo, with weapon,
ammunition, upgrade and perk rows) or `vehicle` (UnitManagerVehicleInfo, with
weapon, ammunition and upgrade rows). Both run against every supported build.
"""
import ctypes as C
import builds
from ctypes import wintypes as W
import pathlib
import re
import struct
import subprocess
import sys
from test_expanded_ammo_menu import K, alloc, put, q, pq, cb, stub, resolve
from pe import Image
from sigs import Module

ROOT = pathlib.Path(__file__).resolve().parent.parent
DLL = ROOT / 'plugins/squad-management-scroll/target/release/defiance_plugin_squad_management_scroll.dll'


# The builds by the index of the generated tables.
BUILDS = ("gog-2025-12-23", "steam-2025-12-23", "gog-2026-09-14", "steam-2026-09-22", "gog-2026-09-25", "steam-2026-09-25")
PATHS = {i: builds.build(n).game for i, n in enumerate(BUILDS)}

# Per panel kind: the unit member on the panel, the four widget-array bases
# (weapons, ammunition, upgrades, perks; 0 when the panel has no such row), the
# vtable/refresh/preflight names, and whether the squad-only rows apply.
PANELS = {
    'squad': dict(unit=0x288, root=0x120, bases=[0x160, 0x190, 0x1e8, 0x1c0],
                  vtable='panel_vtable', refresh='refresh', perks=True),
    'vehicle': dict(unit=0x278, root=0x128, bases=[0x150, 0x180, 0x1b0, 0],
                    vtable='vehicle_vtable', refresh='vehicle_refresh', perks=False),
    # The training chooser shares the native event dispatcher, but has its own
    # window layout and show hook. Its focused fixture branches before the
    # inventory-panel assertions below.
    'training': dict(unit=0x288, root=0x128, bases=[0x160, 0x190, 0x1e8, 0x1c0],
                     vtable='training_vtable', refresh='refresh', perks=False),
}


def parse_table(build):
    """RVAs and preflight lengths for the generated build entry."""
    table = (ROOT / 'plugins/squad-management-scroll/src/sites.rs').read_text().split('    Build {')[build + 1]
    rvas = {n: int(r, 16) for n, r in re.findall(r'(\w+): (?:Site \{\s*rva: )?0x([a-f0-9]+)', table)}
    before = {}
    for name, body in re.findall(r'(\w+): Site \{\s*rva: 0x[0-9a-f]+,\s*before: &\[(.*?)\]', table, re.S):
        before[name] = body.count('0x')
    return rvas, before


def case(build, mode, kind='squad'):
    panel_kind = PANELS[kind]
    path = ROOT / PATHS[build]
    image = Image(path)
    rvas, before = parse_table(build)
    base = K.LoadLibraryExW(str(path), None, 1)  # imports intentionally unresolved
    assert base, C.get_last_error()
    size = image.pe.OPTIONAL_HEADER.SizeOfImage
    original = C.string_at(base, size)
    addr = lambda name: base + rvas[name]
    owned, messages, detours = {}, [], {}
    calls = 0

    @cb(None, C.c_uint32, C.c_char_p)
    def log(level, message): messages.append(message.decode())
    @cb(C.c_void_p, C.c_char_p)
    def module_base(_): return base
    @cb(C.c_size_t, C.c_void_p)
    def module_size(_): return size
    @cb(C.c_int, C.c_void_p, C.c_void_p, C.c_void_p, C.c_size_t)
    def patch(at, before_bytes, after, n):
        nonlocal calls
        calls += 1
        if mode == f'fail{calls}': return 1
        assert C.string_at(at, n) == C.string_at(before_bytes, n)
        owned[at] = C.string_at(at, n)
        put(at, C.string_at(after, n))
        return 0
    @cb(C.c_int, C.c_void_p)
    def unhook(at):
        put(at, owned.pop(at)); return 0

    @cb(None, C.c_void_p)
    def stock_squad(panel):
        # Stock redraw rebinds the first six; production post-hook must restore
        # the viewport on every refresh, rather than only on wheel input.
        squad = q(panel + 0x288)
        if squad:
            for i in range(6):
                pq(q(panel + 0x160 + i*8) + 0x200, q(squad + 0x270) + i*0x48)
                pq(q(panel + 0x190 + i*8) + 0x1d0, q(squad + 0x210) + i*0x28)
    @cb(None, C.c_void_p)
    def stock_vehicle(panel):
        # The vehicle refresh inlines the same weapon and ammunition binding,
        # over the vehicle's own widget arrays.
        vehicle = q(panel + 0x278)
        if vehicle:
            for i in range(6):
                pq(q(panel + 0x150 + i*8) + 0x200, q(vehicle + 0x270) + i*0x48)
                pq(q(panel + 0x180 + i*8) + 0x1d0, q(vehicle + 0x210) + i*0x28)
    rects, thumbs = {}, []
    @cb(None, C.c_void_p)
    def stock_thumb(widget):
        # The stock layout is horizontal-only; the plugin lays out a slider
        # taller than wide itself and never reaches this for one.
        r = rects[widget]
        assert i32(r+12) - i32(r+4) <= i32(r+8) - i32(r), 'vertical slider reached the stock layout'
        thumbs.append((widget, i32(widget+0x1bc)))
        put(rects[q(widget+0x1a8)], struct.pack('<4i', 0, 0, i32(widget+0x1b0), 4))
    training_fixture = {}
    training_shows = []
    training_layout_calls = []
    chosen = []
    @cb(C.c_void_p, C.c_void_p, C.c_void_p, C.c_void_p)
    def stock_squad_chooser(panel, out, item): chosen.append(('squad', panel, out, item)); return out
    @cb(C.c_void_p, C.c_void_p, C.c_void_p, C.c_void_p)
    def stock_vehicle_chooser(panel, out, item): chosen.append(('vehicle', panel, out, item)); return out
    @cb(None, C.c_void_p, C.c_void_p, C.c_uint8, C.c_int32, C.c_void_p)
    def stock_training_show(window, squad, mode_value, currency, callback):
        training_shows.append((window, squad, mode_value, currency, callback))
        cards = training_fixture['cards'][:training_fixture['count']]
        first, second = training_fixture['rows']
        for card in training_fixture['cards']:
            byte(card+0x5b, 0)
        layout = C.CFUNCTYPE(C.c_int, C.c_void_p, C.c_int32, C.c_void_p)(addr('training_layout'))
        # Stock fills two rows. The chooser's native layout returns the count
        # remaining from the requested index, so the second row starts after
        # the first row's capacity.
        layout(window, 0, first)
        capacity = training_fixture['visible']
        if len(cards) > capacity:
            layout(window, capacity, second)
        for card in cards:
            byte(card+0x5b, 1)
    stocks = {'refresh': stock_squad, 'vehicle_refresh': stock_vehicle, 'thumb': stock_thumb,
              'squad_chooser': stock_squad_chooser, 'vehicle_chooser': stock_vehicle_chooser,
              'training_show': stock_training_show}

    @cb(C.c_int, C.c_void_p, C.c_void_p, C.c_size_t, C.c_void_p)
    def hook(at, detour, n, out):
        nonlocal calls
        calls += 1
        if mode == f'fail{calls}': return 1
        name = next((k for k in stocks if at == addr(k)), None)
        assert name is not None, hex(at)
        assert n == before[name], (name, n, before[name])
        owned[at] = C.string_at(at, n)
        pq(out, stocks[name])  # publication precedes detour, as in the loader
        detours[name] = detour
        stub(at, detour)
        return 0
    # The loader materializes every declared setting (defaults included) and
    # validates it before init, so the harness supplies them all: a perk cap of
    # nine so scrolling past five can be exercised, and per mode the
    # upgrade/vehicle flags off or the upgrade slot cap lowered. `noconfig`
    # withholds one, which the plugin must refuse rather than default.
    settings = {'perk_slots': b'9', 'upgrade_slots': b'5' if mode == 'upslots5' else b'20',
                'upgrades': b'false' if mode == 'noupgrades' else b'true',
                'fit_upgrades': b'false' if mode == 'nofit' else b'true',
                'vehicles': b'false' if mode == 'novehicles' else b'true'}
    if mode == 'noconfig': del settings['upgrade_slots']
    setting_bufs = {k: C.create_string_buffer(v) for k, v in settings.items()}
    @cb(C.c_void_p, C.c_char_p, C.c_char_p)
    def config_get(section, key):
        if section and key and section.decode() == 'defiance.squad-management-scroll':
            buf = setting_bufs.get(key.decode())
            if buf is not None:
                return C.addressof(buf)
        return None
    class Api(C.Structure):
        _fields_ = [('abi', C.c_uint32), ('reserved', C.c_uint32)] + [(n, C.c_void_p) for n in
            ['log','module_base','module_size','find_pattern','find_pattern_at','hook','hook_exact',
             'hook_call','unhook','rtti_method','vtable_slot','config_get','patch_bytes']]
    class Plugin(C.Structure):
        _fields_ = [('abi',C.c_uint32),('name',C.c_char_p),('version',C.c_char_p),('init',C.c_void_p),('stop',C.c_void_p)]
    api = Api(5, 0, log, module_base, module_size, 0, 0, 0, hook, 0, unhook, 0, 0, config_get, patch)
    lib = C.CDLL(str(DLL)); lib.defiance_plugin.restype = C.POINTER(Plugin)
    init = C.CFUNCTYPE(C.c_int, C.POINTER(Api))(lib.defiance_plugin().contents.init)
    if mode == 'code': put(addr('thumb'), bytes([C.c_ubyte.from_address(addr('thumb')).value ^ 1]))
    if mode == 'vtable': put(addr(panel_kind['vtable']) + 8, struct.pack('<Q', addr('dispatch') + 1))
    result = init(C.byref(api))
    if mode not in ('ok', 'noupgrades', 'novehicles', 'upslots5', 'nofit'):
        assert result != 0 and not owned, (mode, result, messages)
        if mode.startswith('fail'): assert C.string_at(base, size) == original
        else: assert calls == 0
        print(f'PASS build={build} {kind} {mode}: refusal/rollback', flush=True)
        return
    # Seven vtable slots plus every declared hook, including the training
    # chooser's post-show hook.
    expected_owned = 7 + len(stocks)
    assert result == 0 and len(owned) == expected_owned, messages

    # Mutable, bounded native object fixtures. These are not real rendering.
    def i32(at, value=None):
        if value is None: return C.c_int32.from_address(at).value
        C.c_int32.from_address(at).value = value
    def byte(at, value): C.c_ubyte.from_address(at).value = value
    panel, root, unit, context, holder, service = [alloc(0x800) for _ in range(6)]
    pq(panel, addr(panel_kind['vtable'])); pq(panel+panel_kind['root'], root); pq(panel+panel_kind['unit'], unit)
    # Read the operand from actual native code and require it to be the one the
    # build's table names, so a stale offset cannot pass on any build.
    access = image.disasm(rvas['weapon']+0x202, rvas['weapon']+0x209)[0]
    service_offset = access.operands[1].mem.disp
    assert service_offset == rvas['context_service'], (hex(service_offset), hex(rvas['context_service']))
    pq(panel+0x118, context); pq(context+service_offset, holder)
    hv = alloc(0x100); pq(holder, hv)
    @cb(C.c_void_p, C.c_void_p)
    def get_service(_): return service
    pq(hv+0x58, get_service)
    # The upgrade display is the script service's vt+8 called with the key
    # static (+8) and the element. The stock refresh initialises that static;
    # the fixture seeds it directly, apart from the training key.
    key_static, upgrade_static, key_seen = alloc(0x20), alloc(0x20), []
    pq(addr('upgrade_key'), upgrade_static)
    # The squad's rank, reached as the stock perk refresh reaches it: holder
    # vt+0x68 -> vt+0x38 -> the build's rank slot, called with the squad's
    # experience (+0x70) and the thresholds at the panel's [+0x2b8]+0x28. It
    # matters only on a panel that locks cards past the rank (+0x210), as this
    # one does until the unlocked case below.
    limit, limit_calls = [7], []
    services, rules, info = alloc(0x40), alloc(0x40), alloc(0x80)
    services_vt, rules_vt = alloc(0x100), alloc(rvas['perk_limit'] + 8)
    pq(services, services_vt); pq(rules, rules_vt); pq(panel+0x2b8, info); pq(unit+0x70, 0x5eed)
    @cb(C.c_void_p, C.c_void_p)
    def get_services(_): return services
    @cb(C.c_void_p, C.c_void_p)
    def get_rules(_): return rules
    @cb(C.c_size_t, C.c_void_p, C.c_void_p, C.c_void_p)
    def training_limit(this, key, where):
        limit_calls.append((this, key, where)); return limit[0]
    pq(hv+0x68, get_services); pq(services_vt+0x38, get_rules); pq(rules_vt+rvas['perk_limit'], training_limit)
    byte(panel+0x210, 1)
    # The training table, walked as the TrainingWindow constructor walks it: the
    # key from the build's getter (a static pointer; +8 is the key), the path
    # from the script service's vt+0x20, the table from holder vt+0x70 ->
    # vt+0x20 (cells, columns at +0x18, rows at +0x20, a header row), and each
    # row's training through the script service's vt+8. `available` of its ten
    # trainings list this squad in their `squads` (+0x28); the rest another.
    def msvc(at, text):
        put(at, text.encode() + b'\0'); pq(at+0x10, len(text)); pq(at+0x18, 15)
    msvc(unit+0x28, 'Fnd_rangers')
    @cb(C.c_void_p, C.c_void_p)
    def training_key(out): pq(out, key_static); return out
    stub(addr('training_key'), training_key)
    columns, rows = 3, 12            # a header, ten trainings, one empty row
    cells = alloc(rows*columns*0x20)
    for row in range(1, 11): msvc(cells + row*columns*0x20, f'training_{row}')
    table_name, table, tables, tables_vt, service_vt = alloc(0x20), alloc(0x28), alloc(0x10), alloc(0x40), alloc(0x40)
    pq(table, cells); pq(table+0x18, columns); pq(table+0x20, rows)
    pq(service, service_vt); pq(tables, tables_vt)
    lists = {}
    for owner in ('Fnd_rangers', 'Res_militia'):
        names = alloc(0x20); msvc(names, owner); lists[owner] = names
    infos = [alloc(0x40) for _ in range(10)]
    available = [9]
    # The upgrade table, walked the same way with the upgrade key. A record fits
    # the vehicle types its nine lists name (+0x190..+0x250, each a vector of
    # pointers to entries named at +0x28), or the squads its one squad list
    # names (+0x268, entries named at the build's squad_fit_name; an empty name
    # fits every squad), and carries its conflict tags at +0x30. Rows 1-3 fit
    # rangers tagged armor, armor and ammo+armor (all sharing armor: one card),
    # row 4 names no unit (fits every squad, no vehicle), row 5 repeats row 1's
    # record; of the next 34 rows the first `untagged` fit rangers with no tag,
    # the rest militia. The last row is empty.
    up_columns, up_rows = 2, 41
    up_cells = alloc(up_rows*up_columns*0x20)
    for row in range(1, up_rows-1): msvc(up_cells + row*up_columns*0x20, f'upgrade_{row}')
    up_table_name, up_table = alloc(0x20), alloc(0x28)
    pq(up_table, up_cells); pq(up_table+0x18, up_columns); pq(up_table+0x20, up_rows)
    squad_name = rvas['squad_fit_name']
    def up_record(owner, fits_at, *tags):
        record, entry, storage = alloc(0x280), alloc(0x880), alloc(8)
        if kind == 'squad': fits_at, named = 0x268, squad_name
        else: named = 0x28
        msvc(entry+named, owner); pq(storage, entry)
        pq(record+fits_at, storage); pq(record+fits_at+8, storage+8)
        strings = alloc(0x20*max(1, len(tags)))
        for i, tag in enumerate(tags): msvc(strings+i*0x20, tag)
        pq(record+0x30, strings); pq(record+0x38, strings+0x20*len(tags))
        return record
    up_fixed = [up_record('Fnd_rangers', 0x190, 'armor'), up_record('Fnd_rangers', 0x250, 'armor'),
                up_record('Fnd_rangers', 0x1d8, 'ammo', 'armor'), up_record('', 0x190)]
    up_fixed.append(up_fixed[0])
    up_other = up_record('Res_militia', 0x190)
    up_plain = [(up_record('Fnd_rangers', 0x220), up_record('Res_militia', 0x220)) for _ in range(34)]
    untagged, fixed_fit, paths_seen = [30], [True], []
    anyone = int(kind == 'squad')  # row 4 fits squads only
    @cb(C.c_void_p, C.c_void_p, C.c_void_p)
    def table_path(_, key):
        key_seen.append(key); paths_seen.append(key)
        return up_table_name if key == upgrade_static + 8 else table_name
    @cb(C.c_void_p, C.c_void_p)
    def get_tables(_): return tables
    @cb(C.c_void_p, C.c_void_p, C.c_void_p)
    def get_table(_, name):
        if name == up_table_name: return up_table
        assert name == table_name; return table
    # The upgrade column uses the same script-service slot (vt+8) with the
    # upgrade key and the element; return the element as its display so the
    # binding can be checked. Training rows keep the perk-row behaviour.
    upgrade_records = alloc(32*0x20)
    up_lo, up_hi = upgrade_records, upgrade_records + 32*0x20
    @cb(C.c_void_p, C.c_void_p, C.c_void_p, C.c_void_p)
    def lookup(_, key, where):
        key_seen.append(key)
        if up_lo <= where < up_hi:
            return where
        if key == upgrade_static + 8:
            row = (where - up_cells) // (up_columns*0x20)
            assert 1 <= row < up_rows-1, 'the header and the empty row are never looked up'
            if row <= 5: return up_fixed[row-1] if fixed_fit[0] else up_other
            return up_plain[row-6][0 if row-6 < untagged[0] else 1]
        row = (where - cells) // (columns*0x20)
        assert 1 <= row <= 10, 'the header and the empty row are never looked up'
        names = lists['Fnd_rangers' if row <= available[0] else 'Res_militia']
        info = infos[row-1]; pq(info+0x28, names); pq(info+0x30, names+0x20)
        return info
    pq(service_vt+0x20, table_path); pq(service_vt+0x8, lookup); pq(hv+0x70, get_tables); pq(tables_vt+0x20, get_table)
    records_w, records_a = alloc(32*0x48), alloc(32*0x28)
    scripts = alloc(32*0x80)
    pq(unit+0x270, records_w); pq(unit+0x278, records_w+12*0x48)
    pq(unit+0x210, records_a); pq(unit+0x218, records_a+9*0x28)
    # The upgrade vector (element strings, stride 0x20) is longer than the five
    # widgets so the column can scroll.
    pq(unit+0x288, upgrade_records); pq(unit+0x290, upgrade_records+8*0x20)
    byte(scripts+0x70, 1)  # a hidden weapon must NOT consume a visible index
    @cb(C.c_void_p, C.c_void_p, C.c_void_p)
    def script(_, record): return scripts + (record-records_w)//0x48*0x80
    @cb(None, C.c_void_p)
    def weapon(widget): pass  # bound record is written by production code
    @cb(None, C.c_void_p, C.c_void_p)
    def ammo(widget, record): pq(widget+0x1d0, record or 0)
    upgrade_bound = {}
    @cb(None, C.c_void_p, C.c_void_p)
    def upgrade(widget, display): upgrade_bound[widget] = display
    perk_bound = {}
    @cb(None, C.c_void_p, C.c_void_p)
    def perk(widget, record):
        # raw: null is a bug. A blank card is bound to an empty MSVC string
        # (size 0, inline capacity 15), recorded with its open/highlight bytes.
        assert record, 'the perk bind dereferences its record'
        if q(record+0x10) == 0 and q(record+0x18) == 15:
            record = ('blank', C.c_ubyte.from_address(widget+0x1d0).value, C.c_ubyte.from_address(widget+0x1d1).value)
        perk_bound[widget] = record
    @cb(None, C.c_void_p, C.c_void_p)
    def listen(widget, listener):
        ls = q(widget+0xb0); end = q(ls+0x10)
        assert end < q(ls+0x18), 'duplicate listener registration exhausted fixture'
        pq(end, listener); pq(ls+0x10, end+8)
    @cb(C.c_void_p, C.c_void_p)
    def rect(widget): return rects[widget]
    @cb(None, C.c_void_p, C.c_ubyte)
    def visible(widget, flag): byte(widget+0x5b, flag)
    quads = {}
    @cb(None, C.c_void_p, C.c_void_p)
    def set_geometry(widget, geometry):
        # widget vt+0x28: {rect; vector<point> quad; flags}; the quad is copied.
        put(rects[widget], C.string_at(geometry, 16))
        begin, end = q(geometry+0x10), q(geometry+0x18)
        quads[widget] = [struct.unpack('<2i', C.string_at(at, 8)) for at in range(begin, end, 8)]
    @cb(None, C.c_void_p, C.c_void_p, C.c_void_p, C.c_void_p)
    def hover(*_): pass
    destroyed = []
    @cb(C.c_void_p, C.c_void_p, C.c_uint32)
    def destroy(widget, flags): destroyed.append(('squad', widget)); return widget
    @cb(C.c_void_p, C.c_void_p, C.c_uint32)
    def training_destroy(widget, flags): destroyed.append(('training', widget)); return widget
    @cb(C.c_void_p, C.c_void_p, C.c_uint32)
    def vehicle_destroy(widget, flags): destroyed.append(('vehicle', widget)); return widget
    for n, callback in [('weapon',weapon),('ammo',ammo),('upgrade',upgrade),('perk',perk),('script',script),('listen',listen),('destroy',destroy),('training_destroy',training_destroy),('vehicle_destroy',vehicle_destroy)]:
        stub(addr(n), callback)
    put(addr(panel_kind['vtable'])+0x90, struct.pack('<Q',hover))
    sv = addr('slider_vtable')
    put(sv+0x40, struct.pack('<Q',rect)); put(sv+0x48, struct.pack('<Q',visible))
    wv = alloc(0x100); pq(wv+0x28,set_geometry); pq(wv+0x40,rect); pq(wv+0x48,visible)
    def widget(name=b'', slider=False, vertical=False):
        w = alloc(0x240); pq(w, sv if slider else wv); pq(w+0x38,root)
        put(w+0x18,name+b'\0'); pq(w+0x28,len(name)); pq(w+0x30,15)
        byte(w+0x5b,1); byte(w+0x5c,1)
        ls, storage = alloc(0x30), alloc(256)
        pq(w+0xb0,ls); pq(ls+8,storage); pq(ls+0x10,storage); pq(ls+0x18,storage+256)
        rects[w] = alloc(48)
        put(rects[w], struct.pack('<4i', 449, 49, 453, 417) if vertical else struct.pack('<4i', 1, 0, 453, 4))
        if slider: pq(w+0x1a8,widget())
        return w
    sliders = [widget(b'df_weapons',True),widget(b'df_ammo',True),
               widget(b'df_upgrades',True,vertical=True)]
    # its screen rect (x, y, w, h), at unit scale, for the pointer mapping
    put(sliders[2]+0x150, struct.pack('<4i', 449, 49, 4, 368))
    if panel_kind['perks']:
        sliders.append(widget(b'df_perks',True))
    children = alloc(len(sliders)*8)
    for i, s in enumerate(sliders): pq(children+i*8, s)
    pq(root+0x40,children); pq(root+0x48,children+len(sliders)*8)
    cards = [[widget() for _ in range(6)] for _ in range(2)]
    for s in range(2):
        for i,w in enumerate(cards[s]): pq(panel+panel_kind['bases'][s]+i*8,w)
    # The upgrade column's five widgets at the panel's upgrade base.
    upgrade_cards = [widget() for _ in range(5)]
    for i, w in enumerate(upgrade_cards): pq(panel+panel_kind['bases'][2]+i*8, w)
    perk_cards = []
    if panel_kind['perks']:
        perk_cards = [widget() for _ in range(5)]
        for i, w in enumerate(perk_cards): pq(panel+panel_kind['bases'][3]+i*8, w)
    perk_records = alloc(32*0x20)
    pq(unit+0x2c8, perk_records); pq(unit+0x2d0, perk_records+7*0x20)
    refresh = C.CFUNCTYPE(None,C.c_void_p)(detours[panel_kind['refresh']])
    event_call = C.CFUNCTYPE(None,C.c_void_p,C.c_void_p,C.c_void_p,C.c_void_p)
    dispatch = event_call(q(addr(panel_kind['vtable'])+8))
    ctrl_dispatch = event_call(q(addr('slider_ctrl_vtable')+8))
    event = alloc(40)
    def wheel(source, delta=-120, ctrl=None):
        i32(event,0x20a); C.c_int16.from_address(event+10).value=delta
        (ctrl_dispatch if ctrl else dispatch)(ctrl or panel,source,0,event)
    def bindings(section): return [q(w+[0x200,0x1d0][section]) for w in cards[section]]
    def upgrade_bindings(): return [upgrade_bound.get(w,0) for w in upgrade_cards]
    def perk_bindings(): return [perk_bound.get(w,0) for w in perk_cards]
    if kind == 'training':
        # Build the native training window independently from the inventory
        # panels above. Stock show binds and lays out every available card in
        # two rows; the production post-show detour must collapse it to one.
        training_window, training_root = alloc(0x300), alloc(0x80)
        training_fixture['window'] = training_window
        training_fixture['count'] = 8
        training_cards = [widget() for _ in range(8)]
        training_fixture['cards'] = training_cards
        training_first, training_second = widget(), widget()
        rects[training_first] = alloc(48); rects[training_second] = alloc(48)
        put(rects[training_first], struct.pack('<4i', 10, 309, 450, 695))
        put(rects[training_second], struct.pack('<4i', 10, 695, 450, 1081))
        training_slider = widget(b'df_trainings', True)
        training_children = alloc(8)
        pq(training_children, training_slider)
        pq(training_root+0x40, training_children); pq(training_root+0x48, training_children+8)
        pq(training_window+0x128, training_root)
        pq(training_window+0x148, training_first); pq(training_window+0x150, training_second)
        training_vector = alloc(len(training_cards)*8)
        for index, card in enumerate(training_cards): pq(training_vector+index*8, card)
        pq(training_window+0x1d8, training_vector)
        pq(training_window+0x1e0, training_vector+len(training_cards)*8)
        pq(training_window+0x1e8, training_vector+len(training_cards)*8)
        put(training_window, struct.pack('<Q', addr('training_vtable')))
        old_root = root
        root = training_root
        # Four 100px cards fit in the 440px first row at a 10px gap.
        training_fixture['visible'] = 4
        training_fixture['rows'] = (training_first, training_second)

        @cb(C.c_int, C.c_void_p, C.c_int32, C.c_void_p)
        def stock_training_layout(window, index, row):
            training_layout_calls.append((window, index, row))
            count = training_fixture['count']
            row_rect = struct.unpack('<4i', C.string_at(rects[row], 16))
            pitch = 110
            for slot in range(index, min(index + training_fixture['visible'], count)):
                card = training_fixture['cards'][slot]
                x = row_rect[0] + (slot - index) * pitch
                put(rects[card], struct.pack('<4i', x, row_rect[1], x+100, row_rect[1]+100))
                byte(card+0x5b, 1)
            return max(0, count-index)
        stub(addr('training_layout'), stock_training_layout)
        show = C.CFUNCTYPE(None, C.c_void_p, C.c_void_p, C.c_uint8, C.c_int32, C.c_void_p)(
            detours['training_show'])
        training_dispatch = event_call(q(addr('training_vtable')+8))
        held = [False]
        @cb(C.c_int16, C.c_int)
        def get_key_state(key): return -128 if held[0] else 0
        plugin_image = Image(DLL)
        plugin_base = int(lib._handle)
        key_state_iat = next(
            plugin_base + imp.address - plugin_image.base
            for module in plugin_image.pe.DIRECTORY_ENTRY_IMPORT
            if module.dll.lower() == b'user32.dll'
            for imp in module.imports
            if imp.name == b'GetKeyState'
        )
        put(key_state_iat, struct.pack('<Q', get_key_state))

        def set_training_count(count):
            training_fixture['count'] = count
            end = training_vector + count*8
            pq(training_window+0x1e0, end); pq(training_window+0x1e8, end)

        def training_visible():
            return [C.c_ubyte.from_address(card+0x5b).value != 0 for card in training_cards]

        def training_wheel(source, delta=-120):
            i32(event, 0x20a); C.c_int16.from_address(event+10).value = delta
            training_dispatch(training_window, source, 0, event)

        show(training_window, unit, 1, 250, 0x1234)
        assert training_shows == [(training_window, unit, 1, 250, 0x1234)]
        assert training_layout_calls[:2] == [
            (training_window, 0, training_first),
            (training_window, 4, training_second),
        ], training_layout_calls
        assert training_visible() == [True]*4 + [False]*4, training_visible()
        assert i32(training_slider+0x1b8) == 4
        assert i32(training_slider+0x1bc) == 0

        # Wheel input on a nested button follows its card's parent chain.
        training_button = widget()
        pq(training_button+0x38, training_cards[0])
        held[0] = True
        training_wheel(training_button)
        held[0] = False
        assert i32(training_slider+0x1bc) == 0
        assert training_visible() == [True]*4 + [False]*4, 'wheel during a held drag must not relayout cards'
        training_wheel(training_button)
        assert i32(training_slider+0x1bc) == 1
        assert training_visible() == [False, True, True, True, True, False, False, False]

        # The slider controller uses the same wheel viewport and direction as
        # a card, including while its stock drag flag is clear.
        training_ctrl = alloc(0x40); pq(training_ctrl+8, training_slider)
        i32(event, 0x20a); C.c_int16.from_address(event+10).value = -120
        ctrl_dispatch(training_ctrl, training_slider, 0, event)
        assert i32(training_slider+0x1bc) == 2
        assert training_visible() == [False, False, True, True, True, True, False, False]

        # A native slider change seeks to the final viewport and lays out the
        # final cards through the same row layout callback.
        i32(training_slider+0x1bc, 4)
        i32(event, 0x481)
        held[0] = True
        training_dispatch(training_window, training_slider, 0, event)
        held[0] = False
        assert training_visible() == [False]*4 + [True]*4, training_visible()

        # The dispatch jump table routes 0x47b to the training selection
        # callback on vtable +b0; 0x479 is a separate +d0 message.
        # unchanged, even when it originates from a nested button on the final
        # visible card.
        forwarded_training = []
        @cb(None, C.c_void_p, C.c_void_p, C.c_void_p, C.c_void_p)
        def training_action(window, source, arg, ev): forwarded_training.append((window, source, arg, ev))
        put(addr('training_vtable')+0xb0, struct.pack('<Q', training_action))
        i32(event, 0x47b)
        training_dispatch(training_window, training_button, 0x55, event)
        assert forwarded_training == [(training_window, training_button, 0x55, event)]

        # Showing the same window again starts at zero, even when stock reuses
        # the same squad and slider objects.
        show(training_window, unit, 1, 250, 0x1234)
        assert i32(training_slider+0x1bc) == 0
        assert training_visible() == [True]*4 + [False]*4

        # A shorter list clamps the viewport; an empty menu hides its slider.
        set_training_count(3)
        show(training_window, unit, 1, 250, 0x1234)
        assert i32(training_slider+0x1b8) == 0
        assert training_visible() == [True, True, True, False, False, False, False, False]
        set_training_count(0)
        show(training_window, unit, 1, 250, 0x1234)
        assert C.c_ubyte.from_address(training_slider+0x5b).value == 0
        assert training_visible() == [False]*8

        # Without the companion slider, stock's two-row result stays visible.
        pq(training_root+0x48, training_children)
        set_training_count(8)
        before_layout_calls = len(training_layout_calls)
        show(training_window, unit, 1, 250, 0x1234)
        assert len(training_layout_calls)-before_layout_calls == 2
        assert training_visible() == [True]*8
        pq(training_root+0x48, training_children+8)

        # The training-window deleting destructor removes its per-window state.
        delete_training = C.CFUNCTYPE(C.c_void_p, C.c_void_p, C.c_uint32)(q(addr('training_vtable')))
        assert delete_training(training_window, 0) == training_window
        assert destroyed[-1] == ('training', training_window)
        root = old_root
        print(f'PASS build={build} training: two-row stock, one-row viewport, wheel, slider seek, click forwarding, reopen, shrink, empty, missing slider, destroy', flush=True)
        return
    refresh(panel)
    if mode == 'noupgrades':
        # The setting off: upgrade widgets stay stock (never rebound) and the
        # upgrade slider is hidden, while the weapon and ammunition rows scroll.
        assert not upgrade_bound
        assert C.c_ubyte.from_address(sliders[2]+0x5b).value == 0
        wheel(cards[0][1],-120); assert i32(sliders[0]+0x1bc)==1
        assert not upgrade_bound
        print(f'PASS build={build} {kind} noupgrades: upgrade column off, other rows unaffected', flush=True)
        return
    if mode == 'novehicles':
        assert kind == 'vehicle'
        # The setting off: the whole vehicle panel is left stock and every
        # companion slider is hidden.
        assert not upgrade_bound and not any(upgrade_bound.values())
        assert all(C.c_ubyte.from_address(w+0x5b).value == 0 for w in sliders)
        print(f'PASS build={build} {kind} novehicles: vehicle panel off, all sliders hidden', flush=True)
        return
    if mode == 'upslots5':
        # Capped at five: exactly the five widgets, no scrollbar, wheel is a no-op.
        assert upgrade_bindings()==[upgrade_records+i*0x20 for i in range(5)]
        assert i32(sliders[2]+0x1b8)==0
        assert C.c_ubyte.from_address(sliders[2]+0x5b).value == 0
        wheel(upgrade_cards[1],-120)
        assert i32(sliders[2]+0x1bc)==0
        print(f'PASS build={build} {kind} upslots5: upgrade column capped at five', flush=True)
        return
    if mode == 'nofit':
        # The setting off: the column is upgrade_slots long however few the
        # type can hold, and the upgrade table is never walked.
        untagged[0] = 0; refresh(panel)
        assert i32(sliders[2]+0x1b8)==15
        assert upgrade_static + 8 not in paths_seen
        assert not [m for m in messages if 'holds' in m]
        print(f'PASS build={build} {kind} nofit: upgrade column upgrade_slots long', flush=True)
        return
    assert bindings(0)==[records_w+i*0x48 for i in range(1,7)]
    assert bindings(1)==[records_a+i*0x28 for i in range(6)]
    # The upgrade column binds each of its five widgets to the element at the
    # viewport offset, through the script service's display resolver.
    assert upgrade_bindings()==[upgrade_records+i*0x20 for i in range(5)]
    # The upgrade column is upgrade_slots (default twenty) long: eight owned
    # upgrades, then blank cards to drop more onto.
    assert [i32(w+0x1b8) for w in sliders[:3]]==[5,3,15]
    wheel(cards[0][1],-60); assert i32(sliders[0]+0x1bc)==0
    wheel(cards[0][1],-60); assert i32(sliders[0]+0x1bc)==1
    assert bindings(0)[-1]==records_w+7*0x48
    wheel(cards[1][0],-240); assert i32(sliders[1]+0x1bc)==2
    # The upgrade column scrolls independently, like the other rows.
    wheel(upgrade_cards[1],-120)
    assert i32(sliders[2]+0x1bc)==1
    assert upgrade_bindings()==[upgrade_records+i*0x20 for i in range(1,6)]
    assert bindings(0)[0]==records_w+2*0x48 and bindings(1)[0]==records_a+2*0x28
    wheel(upgrade_cards[0],120)
    assert i32(sliders[2]+0x1bc)==0
    assert upgrade_bindings()==[upgrade_records+i*0x20 for i in range(5)]
    # Scrolled to the end, the owned tail and the first blanks: a blank binds
    # null, as the stock refresh binds an empty slot.
    wheel(upgrade_cards[0],-120*5)
    assert i32(sliders[2]+0x1bc)==5
    # (ctypes hands a null pointer argument over as None)
    assert [b or 0 for b in upgrade_bindings()]==[upgrade_records+i*0x20 for i in range(5,8)]+[0,0]
    wheel(upgrade_cards[0],120*5)
    assert upgrade_bindings()==[upgrade_records+i*0x20 for i in range(5)]
    # The upgrade slider is vertical: the thumb (a quarter of the 368 px track,
    # five of twenty) runs down the gutter, laid out by the plugin, not stock.
    thumb_of = lambda s: struct.unpack('<4i', C.string_at(rects[q(s+0x1a8)], 16))
    assert thumb_of(sliders[2])==(449,49,453,49+92)
    assert quads[q(sliders[2]+0x1a8)]==[(449,49),(453,49),(453,141),(449,141)]
    wheel(upgrade_cards[0],-120*5)
    assert thumb_of(sliders[2])==(449,49+92,453,49+184)
    wheel(upgrade_cards[0],120*5)
    # Dragging maps the pointer's y: the thumb's centre follows it.
    vctrl=alloc(0x40); pq(vctrl+8,sliders[2])
    def move(y, dragging=True):
        byte(vctrl+0x30,int(dragging)); i32(event,0x200); i32(event+0x18,451); i32(event+0x1c,y)
        ctrl_dispatch(vctrl,sliders[2],0,event)
    move(150)
    assert i32(sliders[2]+0x1bc)==3
    assert upgrade_bindings()==[upgrade_records+i*0x20 for i in range(3,8)]
    assert abs(C.c_float.from_address(sliders[2]+0x1c0).value-2.989)<0.01, 'drag position must not snap early'
    move(10000); assert i32(sliders[2]+0x1bc)==15
    assert [b or 0 for b in upgrade_bindings()]==[0]*5
    move(-10000); assert i32(sliders[2]+0x1bc)==0
    # Without a drag in progress a move is the stock controller's.
    move(10000, dragging=False); assert i32(sliders[2]+0x1bc)==0
    assert upgrade_bindings()==[upgrade_records+i*0x20 for i in range(5)]
    # As an upgrade drag starts, the column scrolls ahead of the stock chooser
    # so the card it should pick is in view. Conflict tags are the record's
    # sorted vector<string> at +0x30; the fixture's display is the element, so
    # the last owned upgrade (7) takes its tags from the unused element 8.
    def tagged(at, *names):
        strings = alloc(0x20*max(1, len(names)))
        for i, name in enumerate(names): msvc(strings+i*0x20, name)
        pq(at+0x30, strings); pq(at+0x38, strings+0x20*len(names))
    armor = alloc(0x20); msvc(armor, 'armor')
    pq(upgrade_records+8*0x20+0x10, armor); pq(upgrade_records+8*0x20+0x18, armor+0x20)
    choose = C.CFUNCTYPE(C.c_void_p, C.c_void_p, C.c_void_p, C.c_void_p)(
        detours['squad_chooser' if kind == 'squad' else 'vehicle_chooser'])
    out = alloc(0x18)
    def drag_start(item):
        chosen.clear(); assert choose(panel, out, item) == out
        assert chosen == [(kind, panel, out, item)], chosen
        return i32(sliders[2]+0x1bc)
    item = alloc(0x40); tagged(item, 'ammo', 'armor')
    # conflicts with the off-screen upgrade 7: scrolled the least to show it
    assert drag_start(item) == 3
    assert upgrade_bindings()==[upgrade_records+i*0x20 for i in range(3,8)]
    # no conflict: the first empty card (8) is brought into view
    wheel(upgrade_cards[0],120*5); tagged(item, 'engine')
    assert drag_start(item) == 4
    assert [b or 0 for b in upgrade_bindings()][-1] == 0
    # already in view: nothing moves
    assert drag_start(item) == 4
    # a duplicate of an installed upgrade scrolls to it (the chooser refuses it)
    assert drag_start(upgrade_records+2*0x20) == 2
    # anything but an upgrade (item type +0x2c) is left to the chooser
    wheel(upgrade_cards[0],-120*5); i32(item+0x2c, 1); tagged(item, 'armor')
    before = i32(sliders[2]+0x1bc); assert drag_start(item) == before
    wheel(upgrade_cards[0],120*20)
    assert upgrade_bindings()==[upgrade_records+i*0x20 for i in range(5)]
    # The column is as long as the unit's type can hold (fit_upgrades): each
    # untagged upgrade that fits it and one armor (the three armor-tagged ones
    # conflict), capped at upgrade_slots. Each type's answer is logged once,
    # and again on a change.
    fitted = lambda: [m for m in messages if 'holds' in m]
    line = lambda holds, fit: f'squad scrolling: Fnd_rangers holds {holds} upgrades at once ({fit} fit it, 40 upgrade table rows)'
    assert fitted()==[line(31 + anyone, 33 + anyone)], fitted()
    assert paths_seen.count(upgrade_static + 8) > 1
    untagged[0] = 9 - anyone; refresh(panel); refresh(panel)
    assert i32(sliders[2]+0x1b8)==5, 'ten cards'
    assert fitted()[1:]==[line(10, 12)], fitted()
    wheel(upgrade_cards[0],-120*20); assert i32(sliders[2]+0x1bc)==5
    assert [b or 0 for b in upgrade_bindings()]==[upgrade_records+i*0x20 for i in range(5,8)]+[0,0]
    wheel(upgrade_cards[0],120*20)
    # a drag start still shows the first empty card, which the ten include
    i32(item+0x2c, 0); tagged(item, 'engine')
    assert drag_start(item) == 4
    wheel(upgrade_cards[0],120*20)
    # Never shorter than what it owns: eight upgrades, no empty card, and a
    # drag start that conflicts with none has nothing to show.
    untagged[0] = 0; refresh(panel)
    assert i32(sliders[2]+0x1b8)==3
    assert drag_start(item) == 0
    # An answer of none, or no answer (the key static unset), keeps upgrade_slots.
    fixed_fit[0] = False; refresh(panel)
    assert i32(sliders[2]+0x1b8)==15
    fixed_fit[0] = True; pq(addr('upgrade_key'), 0); refresh(panel)
    assert i32(sliders[2]+0x1b8)==15
    pq(addr('upgrade_key'), upgrade_static); untagged[0] = 30; refresh(panel)
    assert i32(sliders[2]+0x1b8)==15
    assert upgrade_bindings()==[upgrade_records+i*0x20 for i in range(5)]
    refresh(panel)
    assert bindings(0)[0]==records_w+2*0x48 and bindings(1)[0]==records_a+2*0x28
    if panel_kind['perks']:
        assert perk_bindings()==[perk_records+i*0x20 for i in range(5)]
        # the perk row is perk_slots (nine) cards: seven perks and two blanks
        assert i32(sliders[3]+0x1b8)==4
        # The perk pick row scrolls independently over unit+0x2c8 (stride 0x20);
        # a wheel over the row must not move the weapon or ammunition viewports.
        wheel(perk_cards[1],-60); wheel(perk_cards[1],-60)
        assert i32(sliders[3]+0x1bc)==1
        assert perk_bindings()==[perk_records+i*0x20 for i in range(1,6)]
        assert bindings(0)[0]==records_w+2*0x48 and bindings(1)[0]==records_a+2*0x28
        wheel(perk_cards[0],120)
        assert i32(sliders[3]+0x1bc)==0
        assert perk_bindings()==[perk_records+i*0x20 for i in range(5)]
        # The row is always perk_slots (nine) cards: the squad's perks, then
        # blank cards by the stock refresh's rule. This panel locks cards
        # (+0x210), so a blank below the squad's rank is open and the rest
        # locked. Blanks are bound to an empty string, never null: the bind
        # dereferences its record at [rdx+0x10], and passing null crashed the
        # game at game.dll+0x2d20da.
        limit[0] = 2
        pq(unit+0x2d0, perk_records+2*0x20); perk_bound.clear(); refresh(panel)
        assert perk_bindings()==[perk_records, perk_records+0x20]+[('blank',0,0)]*3, perk_bindings()
        assert i32(sliders[3]+0x1b8)==4, 'nine cards however few perks'
        # Below the rank the blanks are open; the first is highlighted when the
        # squad may pick now (the panel's click picks for that card).
        limit[0] = 7; byte(panel+0x352, 1); perk_bound.clear(); refresh(panel)
        assert perk_bindings()==[perk_records, perk_records+0x20, ('blank',1,1), ('blank',1,0), ('blank',1,0)], perk_bindings()
        assert limit_calls[-1]==(rules, 0x5eed, info+0x28), limit_calls[-1]
        wheel(perk_cards[0],-480); assert i32(sliders[3]+0x1bc)==4
        assert perk_bindings()==[('blank',1,0)]*3+[('blank',0,0)]*2, 'locked from the rank on'
        wheel(perk_cards[0],480)
        # Past five perks the row scrolls to the open card for the next pick.
        limit[0] = 9; pq(unit+0x2d0, perk_records+7*0x20); refresh(panel)
        assert i32(sliders[3]+0x1b8)==4
        wheel(perk_cards[0],-480); assert i32(sliders[3]+0x1bc)==4
        assert perk_bindings()==[perk_records+i*0x20 for i in range(4,7)]+[('blank',1,1),('blank',1,0)], perk_bindings()
        # a card that shows a perk is never left open or highlighted
        assert all(C.c_uint16.from_address(w+0x1d0).value==0 for w in perk_cards[:3])
        byte(panel+0x352, 0); refresh(panel)
        assert perk_bindings()[3:]==[('blank',1,0),('blank',1,0)], 'no highlight when the squad may not pick'
        wheel(perk_cards[0],480); assert i32(sliders[3]+0x1bc)==0
        assert perk_bindings()==[perk_records+i*0x20 for i in range(5)]
        assert all(C.c_uint16.from_address(w+0x1d0).value==0 for w in perk_cards)
        # A panel that does not lock cards (+0x210 clear) draws every blank open,
        # whatever the rank, and needs no rank: seven perks at rank five still get
        # the highlighted open card for the next pick.
        byte(panel+0x210, 0); limit[0] = 5; byte(panel+0x352, 1); calls = len(limit_calls); refresh(panel)
        assert len(limit_calls) == calls, 'the rank is not needed when no card locks'
        wheel(perk_cards[0],-480); assert i32(sliders[3]+0x1bc)==4
        assert perk_bindings()==[perk_records+i*0x20 for i in range(4,7)]+[('blank',1,1),('blank',1,0)], perk_bindings()
        # The row is capped at the trainings the squad's type can take: rangers
        # with eight available and seven owned get one open card, not two.
        wheel(perk_cards[0],480); available[0] = 8; key_seen.clear(); refresh(panel)
        assert key_static + 8 in key_seen and set(key_seen) <= {key_static + 8, upgrade_static + 8}, key_seen
        assert i32(sliders[3]+0x1b8)==3
        wheel(perk_cards[0],-480); assert i32(sliders[3]+0x1bc)==3
        assert perk_bindings()==[perk_records+i*0x20 for i in range(3,7)]+[('blank',1,1)], perk_bindings()
        # fewer available than owned: the perks still all show, with no open card
        available[0] = 6; refresh(panel)
        assert i32(sliders[3]+0x1b8)==2 and i32(sliders[3]+0x1bc)==2
        assert perk_bindings()==[perk_records+i*0x20 for i in range(2,7)], perk_bindings()
        # a row shorter than five leaves the cards past it as the stock refresh drew
        wheel(perk_cards[0],480); available[0] = 4; pq(unit+0x2d0, perk_records+2*0x20); perk_bound.clear(); refresh(panel)
        assert perk_bindings()==[perk_records, perk_records+0x20, ('blank',1,1), ('blank',1,0), 0], perk_bindings()
        available[0] = 9; pq(unit+0x2d0, perk_records+7*0x20); refresh(panel)
        wheel(perk_cards[0],480); byte(panel+0x352, 0); byte(panel+0x210, 1)
        limit[0] = 7; refresh(panel)
        # A perk card the stock refresh hid (a panel mode, or the 2026-09 builds'
        # cards after an "exclusive" perk) leaves the row stock: no rebinding, its
        # slider hidden, and a wheel over it reaches the native panel.
        wheeled = []
        @cb(None,C.c_void_p,C.c_void_p,C.c_void_p,C.c_void_p)
        def native_wheel(p,s,a,e): wheeled.append(s)
        native_wheel_slot = q(addr(panel_kind['vtable'])+0xd8)
        put(addr(panel_kind['vtable'])+0xd8,struct.pack('<Q',native_wheel))
        byte(perk_cards[3]+0x5b,0); perk_bound.clear(); refresh(panel)
        assert perk_bound=={} and C.c_ubyte.from_address(sliders[3]+0x5b).value==0
        wheel(perk_cards[1],-120)
        assert wheeled==[perk_cards[1]] and perk_bound=={} and i32(sliders[3]+0x1bc)==0
        assert bindings(0)[0]==records_w+2*0x48, 'the weapon row keeps its viewport'
        byte(perk_cards[3]+0x5b,1); refresh(panel)
        assert perk_bindings()==[perk_records+i*0x20 for i in range(5)]
        put(addr(panel_kind['vtable'])+0xd8,struct.pack('<Q',native_wheel_slot))
    # A hidden upgrade column leaves the column stock: its slider hidden and a
    # wheel over it reaches the native panel.
    wheeled = []
    @cb(None,C.c_void_p,C.c_void_p,C.c_void_p,C.c_void_p)
    def native_wheel2(p,s,a,e): wheeled.append(s)
    native_slot = q(addr(panel_kind['vtable'])+0xd8)
    put(addr(panel_kind['vtable'])+0xd8,struct.pack('<Q',native_wheel2))
    byte(upgrade_cards[3]+0x5b,0); upgrade_bound.clear(); refresh(panel)
    assert upgrade_bound=={} and C.c_ubyte.from_address(sliders[2]+0x5b).value==0
    wheel(upgrade_cards[1],-120)
    assert wheeled==[upgrade_cards[1]] and upgrade_bound=={}
    byte(upgrade_cards[3]+0x5b,1); refresh(panel)
    assert upgrade_bindings()==[upgrade_records+i*0x20 for i in range(5)]
    put(addr(panel_kind['vtable'])+0xd8,struct.pack('<Q',native_slot))
    # Child hit targets and wheel direction over the slider use the same offset.
    child=widget(); pq(child+0x38,cards[0][0]); wheel(child)
    assert i32(sliders[0]+0x1bc)==2
    ctrl=alloc(0x40); pq(ctrl+8,sliders[0]); wheel(sliders[0],-120,ctrl)
    assert i32(sliders[0]+0x1bc)==3
    # Native slider change notification comes through event 0x481.
    i32(sliders[0]+0x1bc,5); C.c_float.from_address(sliders[0]+0x1c0).value=4.75
    i32(event,0x481); dispatch(panel,sliders[0],0,event)
    assert bindings(0)[-1]==records_w+11*0x48
    assert C.c_float.from_address(sliders[0]+0x1c0).value==4.75, 'drag position must not snap early'
    # Execute the actual game drag-position math and notification dispatcher,
    # rather than only fabricating a slider-change event. Rendering remains a
    # stub; the native code maps screen coordinates and emits 0x481 itself.
    module, reference = Module(path), Module(builds.reference().game)
    position = base + resolve(module,reference,0x2c5f20)
    notify = base + resolve(module,reference,0x2c59c0)
    listener_vt = alloc(16)
    @cb(None,C.c_void_p,C.c_void_p,C.c_void_p,C.c_void_p)
    def notify_list(_,source,arg,ev): dispatch(panel,source,arg,ev)
    pq(listener_vt+8,notify_list); pq(q(sliders[0]+0xb0),listener_vt)
    for x,expected in [(-100,0),(10000,5)]:
        i32(event+0x18,x)
        C.CFUNCTYPE(None,C.c_void_p,C.c_void_p)(position)(sliders[0],event+0x18)
        C.CFUNCTYPE(None,C.c_void_p,C.c_void_p,C.c_void_p)(notify)(ctrl,0,event)
        assert i32(sliders[0]+0x1bc)==expected
        assert bindings(0)[0]==records_w+(expected+1)*0x48
    # Other input still reaches the native panel vtable.
    forwarded=[]
    @cb(None,C.c_void_p,C.c_void_p,C.c_void_p,C.c_void_p)
    def click(p,s,a,e): forwarded.append(s)
    put(addr(panel_kind['vtable'])+0x70,struct.pack('<Q',click)); i32(event,0x201)
    dispatch(panel,cards[0][5],0,event); assert forwarded==[cards[0][5]]
    assert q(forwarded[0]+0x200)==records_w+11*0x48
    pq(unit+0x278,records_w+8*0x48); pq(unit+0x218,records_a+3*0x28); refresh(panel)
    assert [i32(w+0x1bc) for w in sliders[:3]]==[1,0,0]
    assert bindings(1)[3:]==[0,0,0] and C.c_ubyte.from_address(sliders[1]+0x5b).value==0
    # Null/other-type selection clears state, changing units resets, and
    # destruction clears even if the allocator later reuses the same address.
    pq(panel+panel_kind['unit'],0); refresh(panel); pq(panel+panel_kind['unit'],unit); refresh(panel)
    assert i32(sliders[0]+0x1bc)==0
    wheel(cards[0][0]); assert i32(sliders[0]+0x1bc)==1
    delete=C.CFUNCTYPE(C.c_void_p,C.c_void_p,C.c_uint32)(q(addr(panel_kind['vtable'])))
    assert delete(panel,0)==panel
    assert destroyed==[(kind,panel)], destroyed
    refresh(panel); assert i32(sliders[0]+0x1bc)==0
    # Missing companion layout must leave the stock bindings and warn once.
    pq(root+0x48,children); refresh(panel); refresh(panel)
    assert bindings(0)[0]==records_w
    assert len([m for m in messages if 'controls missing' in m])==1
    print(f'PASS build={build} {kind}: production viewport, filtered items, upgrade column, events, listeners, refresh/shrink/reset/destruction/fallback',flush=True)


if __name__ == '__main__':
    if len(sys.argv)>2: case(int(sys.argv[1]),sys.argv[2],sys.argv[3] if len(sys.argv)>3 else 'squad')
    else:
        for build in range(len(BUILDS)):
            if not (ROOT / PATHS[build]).exists():
                continue
            for kind in ['squad','vehicle']:
                modes = ['code','vtable','fail1','fail2','fail3','fail4','fail5','fail6','fail7','fail8','fail9','fail10','noconfig','ok','noupgrades','upslots5','nofit']
                if kind == 'vehicle':
                    modes.append('novehicles')
                for mode in modes:
                    subprocess.run([sys.executable,__file__,str(build),mode,kind],check=True,timeout=90)
            subprocess.run([sys.executable,__file__,str(build),'ok','training'],check=True,timeout=90)
