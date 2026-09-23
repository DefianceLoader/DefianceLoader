"""Reproduce the two supported squad-panel binding tables from local game DLLs.

Run from the repository root. No game code is copied into the repository beyond
short preflight signatures. Calls to shared helpers are resolved through callers.
"""
import hashlib
import sys
from pathlib import Path
import capstone
from sigs import Module
from rtti import Rtti
from rustfmt import rustfmt

ROOT = Path(__file__).resolve().parent.parent
FUNCTIONS = dict(refresh=0x29e6c0, dispatch=0x2c2da0, destroy=0x29c4e0,
                 ammo=0x3cb00, weapon=0x3674b0, script=0x709e0,
                 listen=0x2d3270, thumb=0x2c6180, slider_dispatch=0x2c52d0,
                 # The training_slot_* (perk) bind, called by the perk refresh
                 # fn_29dc30 which the full refresh runs; (widget, record).
                 perk=0x2cff30,
                 # The upgrade_slot_* column bind; both panels pass
                 # (widget, display) where the display is resolved from the
                 # element through the script service and a static key below.
                 upgrade=0x365740)
CALLERS = dict(script=0x3674b0, listen=0x29b040)
# The army presets window's drag-start helper: for an item being dragged it asks
# the squad panel ([window+0x2c0]) and the vehicle panel ([window+0x2c8]) which
# of their cards may take it. The two choosers are found through it, since the
# squad chooser's own prologue differs between the 2025 and 2026 builds.
DRAG_START = 0x67580


def first_call(img, at, span=0x40):
    """The target of the first direct call in the function at `at`."""
    for ins in img.disasm(at, at + span):
        if ins.mnemonic == 'call' and ins.operands and ins.op_str.startswith('0x'):
            return int(ins.op_str, 16)
    raise ValueError(f'no direct call at {at:#x}')


def upgrade_key(img, refresh):
    """The upgrade column's key static, as the stock loop reads it: inside the
    vehicle refresh's upgrade loop it loads a pointer from a .data static (a
    lazily-initialised key, guarded by a nearby static-init flag), adds 8, and
    passes it to the script service's display resolver before the upgrade bind.
    The element display is `service->vt+8(service, key+8, element)`; native.rs
    reads this static (initialised by the stock refresh) to rebind scrolled
    cards. Found by the `mov reg, [rip+D]; add reg, 8` pair that feeds the
    indirect display call ahead of a direct upgrade-bind call."""
    start, end = img.function_of(refresh)
    insns = list(img.md.disasm(img.read(start, end - start), start))
    for i, ins in enumerate(insns):
        if ins.mnemonic != 'call' or not ins.op_str.startswith('qword ptr [rbp'):
            continue
        for j in range(i - 1, max(0, i - 8), -1):
            if insns[j].mnemonic == 'add' and insns[j].op_str.endswith(', 8'):
                reg = insns[j].op_str.split(',')[0]
                for k in range(j - 1, max(0, j - 4), -1):
                    mov = insns[k]
                    if (mov.mnemonic != 'mov' or len(mov.operands) != 2
                            or not mov.op_str.startswith(reg + ', qword ptr [rip')):
                        continue
                    mem = mov.operands[1]
                    if mem.mem.base == capstone.x86.X86_REG_RIP:
                        target = mov.address + mov.size + mem.mem.disp
                        if img.section_of(target) == '.data':
                            return target
    raise ValueError(f'no upgrade key static in the refresh at {refresh:#x}')


def choosers(source, target):
    """The squad and vehicle panels' drop-target choosers, as the drag-start
    helper calls them: a direct call right after `mov rcx, [reg + 0x2c0]`
    (squad) or `[reg + 0x2c8]` (vehicle). Each walks its five upgrade cards and
    picks the first visible empty one, or one bound to an upgrade the item
    conflicts with."""
    start, pattern, mask = source.signature(DRAG_START, [])
    hits = target.matches(pattern, mask)
    if len(hits) != 1:
        raise ValueError(f'drag start: expected a unique function, got {hits}')
    helper = hits[0] + DRAG_START - start
    insns = target.functions.disasm(*target.functions.function_of(helper))
    found = {}
    for previous, ins in zip(insns, insns[1:]):
        if ins.mnemonic != 'call' or not ins.op_str.startswith('0x'):
            continue
        for offset, name in ((0x2c0, 'squad_chooser'), (0x2c8, 'vehicle_chooser')):
            if (previous.mnemonic == 'mov' and previous.op_str.startswith('rcx, qword ptr [')
                    and previous.op_str.endswith(f' + {offset:#x}]')):
                assert name not in found, (name, hex(ins.address))
                found[name] = int(ins.op_str, 16)
    assert set(found) == {'squad_chooser', 'vehicle_chooser'}, found
    return found


def table(source, target):
    result = {}
    for name, rva in FUNCTIONS.items():
        if name == 'refresh':
            # The frame size in its prologue changed, so no signature follows
            # it; it is the callee of the panel's vtable slot 7 wrapper, found
            # after the vtables below.
            continue
        at = rva
        if name in CALLERS:
            at = next(i.address for i in source.functions.disasm(
                *source.functions.function_of(CALLERS[name]))
                if i.mnemonic == 'call' and i.op_str == hex(rva))
        start, pattern, mask = source.signature(at, [])
        hits = target.matches(pattern, mask)
        if len(hits) != 1:
            raise ValueError(f'{name}: expected unique caller/function, got {hits}')
        mapped = hits[0] + at - start
        if name in CALLERS:
            ins = target.functions.disasm(mapped, mapped + 5)[0]
            assert ins.mnemonic == 'call'
            mapped = ins.operands[0].imm
        result[name] = mapped
    for name, cls in [('panel_vtable', 'UnitManagerSquadInfo'),
                      ('vehicle_vtable', 'UnitManagerVehicleInfo'),
                      ('slider_vtable', 'GuiSliderWidget'),
                      ('slider_ctrl_vtable', 'GuiSliderCtrl')]:
        candidates = [vt for _, _, cols in Rtti(target.functions).find(cls)
                      for col, vts in cols if target.functions.u32(col + 4) == 0
                      for vt in vts]
        assert len(candidates) == 1, (cls, candidates)
        result[name] = candidates[0]
    img = target.functions
    assert img.u64(result['panel_vtable']) - img.base == result['destroy']
    assert img.u64(result['panel_vtable'] + 8) - img.base == result['dispatch']
    assert img.u64(result['slider_ctrl_vtable'] + 8) - img.base == result['slider_dispatch']
    # The vehicle panel shares the base panel's dispatch (its vtable slot 1) but
    # has its own destroy and refresh; both derive from the vehicle vtable.
    assert img.u64(result['vehicle_vtable'] + 8) - img.base == result['dispatch']
    result['vehicle_destroy'] = img.u64(result['vehicle_vtable']) - img.base
    wrapper = img.u64(result['vehicle_vtable'] + 0x38) - img.base
    result['vehicle_refresh'] = first_call(img, wrapper, span=0x60)
    result['upgrade_key'] = upgrade_key(img, result['vehicle_refresh'])
    # Slot 7 is a wrapper that calls the panel's refresh. Follow it, because
    # that function's prologue frame moved and its signature no longer matches.
    wrapper = img.u64(result['panel_vtable'] + 0x38) - img.base
    result['refresh'] = first_call(img, wrapper)
    # Steam's GUI context added 0x20 bytes before its service holder. Derive
    # this operand from the weapon redraw, not from its common prologue.
    access = img.disasm(result['weapon'] + 0x202, result['weapon'] + 0x209)[0]
    assert access.mnemonic == 'mov' and access.op_str in (
        'rcx, qword ptr [rax + 0x118]', 'rcx, qword ptr [rax + 0x138]')
    result['context_service'] = access.operands[1].mem.disp
    result['perk_limit'] = perk_limit(img, result)
    result['training_key'] = training_key(img, result)
    result.update(choosers(source, target))
    return result


def training_key(img, result):
    """The getter of the training table's key, as the perk bind calls it first:
    it stores a static pointer in its out-parameter, and pointer + 8 is the key
    the script service resolves training names with. native.rs counts a squad's
    trainings as the TrainingWindow constructor builds its list, so that walk is
    checked here: the table's path from script-service vt+0x20, the table from
    holder vt+0x70 -> vt+0x20 (cells at +0, columns at +0x18, rows at +0x20),
    each row's training by script-service vt+8, all with this same key."""
    key = first_call(img, result['perk'], span=0x60)
    window = next(vt for m, _d, cols in Rtti(img).find('.?AVTrainingWindow@Leonardo@@')
                  if m == '.?AVTrainingWindow@Leonardo@@' for col, vts in cols for vt in vts)
    ctor = max((img.function_of(x) for x in img.xrefs(window) if img.function_of(x)),
               key=lambda f: f[1] - f[0])
    code = '\n'.join(f'{i.mnemonic} {i.op_str}' for i in img.disasm(*ctor))
    assert code.count(f'call {key:#x}') >= 2, 'the constructor resolves with the same key'
    for shape in ('call qword ptr [rax + 0x58]', 'call qword ptr [rax + 0x70]',
                  'mov rdi, qword ptr [rax + 0x20]\nmov rcx, qword ptr [rax + 0x18]'):
        assert shape in code, shape
    return key


def perk_limit(img, result):
    """The vtable slot of the squad's rank, as the stock perk refresh calls it:
    [[panel+0x118]+context_service] -> vt+0x68 -> vt+0x38 -> vt+slot, with the
    squad's experience (+0x70) and the rank thresholds at [panel+0x2b8]+0x28. The
    perk refresh is the one function calling the perk bind; its fixed panel
    offsets (+0x210 whether cards past the rank are locked, +0x352 whether a perk
    may be picked now) are
    checked here because native.rs uses them as constants. The full refresh
    runs it, so it is found among that function's direct callees."""
    def callees(function):
        return {i.operands[0].imm for i in img.disasm(*function)
                if i.mnemonic == 'call' and i.op_str.startswith('0x')}
    callers = {img.function_of(rva) for rva in callees(img.function_of(result['refresh']))
               if img.function_of(rva) and result['perk'] in callees(img.function_of(rva))}
    assert len(callers) == 1, [hex(f[0]) for f in callers]
    start, end = callers.pop()
    code = [f'{i.mnemonic} {i.op_str}' for i in img.disasm(start, end)]
    chain = [c for c in code[:30] if c.startswith(('mov rcx, qword ptr [rax + ', 'call qword ptr [r'))]
    assert chain[0] == f"mov rcx, qword ptr [rax + {result['context_service']:#x}]", chain
    assert chain[1:3] == ['call qword ptr [rax + 0x68]', 'call qword ptr [rdx + 0x38]'], chain
    slot = next(c for c in code[:30] if c.startswith('mov r9, qword ptr ['))
    assert 'cmp byte ptr [r14 + 0x210], 0' in code
    assert any(c.startswith('cmp byte ptr [r14 + 0x352], ') for c in code)
    return int(slot.split('+ ')[1].rstrip(']'), 16)


def generate():
    source = Module(ROOT / 'bin/game.orig.dll')
    out = ['// Generated by tools/squad_scroll_bindings.py; do not hand edit.',
           'use super::{Build, Site};', 'pub(super) const BUILDS: &[Build] = &[']
    # Extra target DLLs name more builds to emit beside the reference and
    # Steam, each located by the reference's signatures and RTTI.
    extra = [Path(p) for p in sys.argv[1:]]
    for path in [ROOT / 'bin/game.orig.dll', ROOT / 'bin/steam/game.dll'] + extra:
        target = source if path.name == 'game.orig.dll' else Module(path)
        entries = table(source, target)
        out.append('    Build {')
        out.append(f'        sha: "{hashlib.sha256(path.read_bytes()).hexdigest()}",')
        for name, rva in entries.items():
            if name.endswith('vtable') or name in ('context_service', 'perk_limit', 'upgrade_key'):
                out.append(f'        {name}: 0x{rva:x},')
            else:
                # Complete instructions, including the 21-byte refresh prologue.
                span = bytearray()
                for ins in target.functions.disasm(rva, rva + 64):
                    span.extend(ins.bytes)
                    if len(span) >= 21:
                        break
                data = ', '.join(f'0x{b:02x}' for b in span)
                out.append(f'        {name}: Site {{ rva: 0x{rva:x}, before: &[{data}] }},')
        out.append('    },')
    out.append('];\n')
    return '\n'.join(out)


if __name__ == '__main__':
    path = ROOT / 'plugins/squad-management-scroll/src/sites.rs'
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(generate(), encoding='utf-8')
    rustfmt(path)
    print(path)
