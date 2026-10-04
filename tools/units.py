"""Assemble each built-in feature's payload as units: one per (plugin, module).

A unit is a code blob and a descriptor. The blob is assembled on its own, not
at a fixed offset in a shared block, so the plugin that owns it can have it
allocated anywhere within rel32 reach of its module. Its private cells sit at
its start; a cell several units write (the logic manager's trace ring) is
imported by name and Core provides it. The descriptor lists:

- `writes`: what the unit changes in its module: a `jmp` or `call` into the
  blob (`entry`, `label`), with `tail` after the branch, or an in-place `edit`.
  A `call` whose rel32 reaches a stock function names it (`stock`), so a moved
  build keeps reaching that build's copy.
- `fixups`: slots in the blob resolved where it lands: `abs64` (module base
  + `target`), `rel32` (a branch into the module), `delta` (a disp32 holding
  `to - from`), `export` (a Win32 function's address), `cell` (a disp32 to an
  imported cell, whose instruction ends at `end`), and `edit` (a rel32 inside
  an edit's `after` bytes, re-aimed when the edit moves).
- `cells`, `natives` (functions a plugin may replace outright in Rust), the
  `sites` whose signatures cover every address above, and `anchors` (bytes
  checked before anything is written; Core's own units).

The writes are the ones `tools/payload.py` and `tools/icon.py` already checked
against the DLL, read from their descriptors in out/ and filtered by owning
feature, so run those first; `tools/module_writes.py check` compares the result
with the recorded reference.

    python tools/units.py                  # out/units/
    python tools/units.py --layout NAME    # out/units-NAME/, for tools/variant.py
"""
import json, os, pathlib, struct, sys
sys.path.insert(0, "tools")
import capstone
import build as b
import icon

# The blob is assembled at a notional base far from any module rva, so a
# branch that leaves it is told apart from one inside it; imported cells get
# their own notional address, found again in the assembled code.
UNIT_BASE = 0x60000000
IMPORT_BASE = 0x50000000
IMPORTS = {"trace_ring": IMPORT_BASE}
# The logic manager's trace ring: an index, then 32 sixteen-byte entries.
TRACE_RING_BYTES = 0x10 + 32 * 16

LOGIC, GAME = "logic", "game"

# plugin -> module -> (the feature ID the monolithic descriptors tag its writes
# with, private cells [(name, bytes)], routines [(sources, {token: cell},
# entry name or None)]). A token's cell is a private cell, "name+offset" inside
# one, or "@name" for an imported cell.
UNITS = {
    "pickup": {LOGIC: (1, [("cursor", 0x10)], [
        (["patch/pickup.asm"], {"cursor": "cursor"}, "chooser")])},
    "selection": {
        LOGIC: (2, [("marquee", 0x10), ("preview_dim", 0x10)], [
            (["patch/soldier-mark.asm"], {}, "set_selected"),
            (["patch/region-individual.asm"], {"scratch": "marquee"}, None),
            (["patch/preview-dim.asm"], {"scratch": "preview_dim"}, None)]),
        GAME: (2, [("trace", 0x60), ("state", 0x40)], [
            (["patch/icon-squad.asm"], {"cursor": "trace"}, None),
            (["patch/building-control.asm"], {"cursor": "trace", "scratch": "state"}, None),
            (["patch/preview-subset.asm"], {"cursor": "trace", "scratch": "state+0x30"}, None)]),
    },
    "movement": {LOGIC: (3, [], [(["patch/move-filter.asm"], {}, "move_filter")])},
    "posture": {LOGIC: (4, [], [
        (["patch/posture-gate.asm"], {}, None),
        (["patch/prone-query.asm"], {}, None),
        (["patch/pose.asm"], {"cursor": "@trace_ring"}, None)])},
    "firing": {LOGIC: (5, [], [(["patch/firing-mode.asm"], {}, None)])},
    "ammunition": {
        LOGIC: (6, [("scratch", 0x70)], [
            (["patch/ammo-mode.asm"], {"cursor": "@trace_ring", "scratch": "scratch"}, None)]),
        GAME: (6, [("step", 0x38)], [(["patch/ammo-panel.asm"], {"scratch": "step"}, None)]),
    },
    "diagnostics": {LOGIC: (7, [("census", 0x200)], [
        (["patch/select-trace.asm"], {"cursor": "@trace_ring"}, None),
        (["patch/behaviour-census.asm"], {"cursor": "census"}, None)])},
    "attack": {LOGIC: (8, [], [(["patch/order-attack.asm", "patch/order-selected.asm"], {}, None)])},
    "garrison": {LOGIC: (9, [], [(["patch/order-garrison.asm"], {}, None)])},
    "preview-weapon": {GAME: (10, [], [(["patch/preview-weapon.asm"], {}, None)])},
    "vehicle-special-fire": {LOGIC: (11, [], [
        (["patch/vehicle-special-fire.asm", "patch/vehicle-priority-fire.asm",
          "patch/vehicle-dynamic-fire.asm"], {}, None)])},
}

# The selection state cell holds the squad TAB modifier's key at +0x28
# (icon.TAB_MODIFIER_OFFSET within icon.BUILDING_STATE_OFFSET's state).
NAMED_CELLS = {("selection", GAME): [("tab_modifier", "state", icon.TAB_MODIFIER_OFFSET - icon.BUILDING_STATE_OFFSET)]}

# Ordinary functions a plugin may replace with Rust, by the site of the jmp
# over their start.
NATIVES = {("selection", LOGIC): [("setter", b.SETTER_RVA), ("is_selected", b.IS_SELECTED_RVA)],
           ("firing", LOGIC): [(label, rva) for rva, _, label in b.FIRING_HOOKS
                               if label in ("firing_set", "firing_ui")]}

md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
md.detail = True


def align(value, to=16):
    return (value + to - 1) // to * to


def cell_address(spec, cells):
    if spec.startswith("@"):
        return IMPORTS[spec[1:]]
    name, _, extra = spec.partition("+")
    return UNIT_BASE + cells[name][0] + (int(extra, 16) if extra else 0)


def assemble_unit(plugin, module, spec, symbol_overrides=None):
    """(blob, labels by unit offset, cells {name: (offset, bytes)}, code start)."""
    feature, cell_list, routines = spec
    cells, at = {}, 0
    for name, size in cell_list:
        cells[name] = (at, size)
        at = align(at + size)
    code_start = at
    layout = b.LAYOUT if module == LOGIC else b.GAME_LAYOUT
    symbols = dict(b.SYMBOLS if module == LOGIC else b.GAME_SYMBOLS)
    symbols.update(symbol_overrides or {})
    blob, labels, shared = bytearray(code_start), {}, set()
    for sources, tokens, entry in routines:
        lines = ([f"{entry}:"] if entry else []) + b.source(*sources)
        address = {t: cell_address(c, cells) for t, c in tokens.items()}
        base = UNIT_BASE + align(len(blob))
        blob.extend(bytes(base - UNIT_BASE - len(blob)))
        code, found = b.assemble(lines, base, address.get("cursor"), address.get("scratch"),
                                 address.get("census"), layout=layout, symbols=symbols)
        # Labels are local to their routine, as in the monolithic block: a
        # name two routines define is left out of the map, and so can name
        # no entry.
        for label, where in found.items():
            if label in labels or label in shared:
                labels.pop(label, None)
                shared.add(label)
            else:
                labels[label] = where - UNIT_BASE
        blob.extend(code)
    return bytes(blob), labels, cells, code_start


def code_fixups(name, blob, code_start):
    """The rel32 branches that leave the blob and the disp32s to imported cells."""
    fixups, end = [], UNIT_BASE + len(blob)
    decoded = list(md.disasm(blob[code_start:], UNIT_BASE + code_start))
    if sum(ins.size for ins in decoded) != len(blob) - code_start:
        raise SystemExit(f"{name}: the code does not disassemble to its end")
    for ins in decoded:
        if ins.group(capstone.CS_GRP_JUMP) or ins.group(capstone.CS_GRP_CALL):
            op = ins.operands[0] if ins.operands else None
            if op is not None and op.type == capstone.x86.X86_OP_IMM and not UNIT_BASE <= op.imm < end:
                if ins.bytes[0] not in (0xe8, 0xe9) or ins.size != 5:
                    raise SystemExit(f"{name}: {ins.mnemonic} {ins.op_str} at "
                                     f"+{ins.address - UNIT_BASE:#x} leaves the unit but is not a rel32 jmp/call")
                fixups.append({"kind": "rel32", "offset": ins.address - UNIT_BASE + 1, "target": op.imm})
        for op in ins.operands:
            if op.type == capstone.x86.X86_OP_MEM and op.mem.base == capstone.x86.X86_REG_RIP:
                target = ins.address + ins.size + op.mem.disp
                if UNIT_BASE <= target < end:
                    continue
                cell = next((c for c, at in IMPORTS.items() if at == target), None)
                if cell is None:
                    raise SystemExit(f"{name}: {ins.mnemonic} {ins.op_str} at "
                                     f"+{ins.address - UNIT_BASE:#x} reaches {target:#x}, outside the unit")
                fixups.append({"kind": "cell", "offset": ins.address - UNIT_BASE + ins.disp_offset,
                               "end": ins.address - UNIT_BASE + ins.size, "cell": cell})
    return fixups


def placeholder_fixups(blob, placeholders):
    """[(fixup, placeholder)] for each placeholder imm64 the blob holds."""
    out = []
    for placeholder, fixup in placeholders:
        raw = struct.pack("<Q", placeholder)
        at = blob.find(raw)
        if at < 0:
            continue
        if blob.find(raw, at + 1) >= 0:
            raise SystemExit(f"the placeholder {placeholder:#x} appears more than once")
        out.append(({**fixup, "offset": at}, placeholder))
    return out


def descriptor_writes(descriptor, module, names):
    """(feature, write) for every write of a monolithic descriptor, each branch
    naming the block labels at its entry."""
    def branch(kind, rva, before, entry, tail, stock=0):
        return {"kind": kind, "rva": rva, "before": before, "labels": names[entry], "tail": tail,
                **({"stock": stock} if kind == "call" else {})}

    def nops(before):
        return "90" * (len(bytes.fromhex(before)) - 5)
    out = []
    if module == LOGIC:
        d = descriptor
        out.append((1, branch("call", d["call_site"], d["call_before"], 0, "", d["stock_chooser"])))
        out.append((3, branch("call", d["move_call_site"], d["move_displaced"], d["move_offset"],
                              nops(d["move_displaced"]))))
        out += [(c["pose_feature"], branch("call", c["pose_site"], c["pose_before"], c["pose_entry"],
                                           c["pose_tail"], c["pose_stock"])) for c in d["pose_calls"]]
        out += [(h["hook_feature"], branch("jmp", h["hook_rva"], h["hook_displaced"], h["hook_entry"],
                                           nops(h["hook_displaced"]))) for h in d["detours"]]
        for name in ("select_is", "select_squad", "select_toggle", "select_type"):
            if d[f"{name}_before"]:
                out.append((2, {"kind": "edit", "rva": d[f"{name}_rva"], "before": d[f"{name}_before"],
                                "after": d[f"{name}_after"]}))
    else:
        out += [(h["hook_feature"], branch("jmp", h["rva"], h["displaced"], h["entry"],
                                           nops(h["displaced"]))) for h in descriptor["hooks"]]
    return out


def covering(sites, addresses):
    """The site entries whose window, its end included, holds any address."""
    def covers(entry, address):
        start = entry["site_start"]
        return start <= address <= start + len(entry["site_pattern"]) // 2
    out = [entry for entry in sites if any(covers(entry, a) for a in addresses)]
    bare = [a for a in addresses if not any(covers(entry, a) for entry in sites)]
    if bare:
        raise SystemExit("no site's signature covers " + ", ".join(f"{a:#x}" for a in sorted(set(bare))))
    return out


def vehicle_special_fire_symbol_overrides(descriptor, feature):
    """Resolve the passenger chooser, AI helpers, and hook continuations."""
    calls = [call for call in descriptor["pose_calls"] if call["pose_feature"] == feature]
    if len(calls) != 2 or len({call["pose_site"] for call in calls}) != 2:
        raise SystemExit(f"vehicle-special-fire-logic: expected two passenger selector calls, found {len(calls)}")
    targets = set()
    for call in calls:
        before = bytes.fromhex(call["pose_before"])
        if len(before) != 5 or before[0] != 0xe8:
            raise SystemExit("vehicle-special-fire-logic: the passenger selector is not a rel32 call")
        targets.add(call["pose_site"] + 5 + int.from_bytes(before[1:5], "little", signed=True))
    if len(targets) != 1:
        raise SystemExit("vehicle-special-fire-logic: passenger selectors have different native targets")
    guards = [hook for hook in descriptor["detours"] if hook["hook_feature"] == feature
              and bytes.fromhex(hook["hook_displaced"]).startswith(bytes.fromhex("85f60f88"))]
    if len(guards) != 1:
        raise SystemExit(f"vehicle-special-fire-logic: expected one rebind guard, found {len(guards)}")
    guard = guards[0]
    displaced = bytes.fromhex(guard["hook_displaced"])
    if len(displaced) != 8 or displaced[:4] != bytes.fromhex("85f60f88"):
        raise SystemExit("vehicle-special-fire-logic: the rebind guard is not the expected branch")
    resume = guard["hook_rva"] + len(displaced)
    tails = {}
    for name, prefix in (("board", "488b5c2478"), ("disembark", "488b742448")):
        matches = [hook for hook in descriptor["detours"] if hook["hook_feature"] == feature
                   and hook["hook_displaced"] == prefix]
        if len(matches) != 1:
            raise SystemExit(f"vehicle-special-fire-logic: expected one {name} tail")
        tails[f"vehicle_special_fire_{name}_resume"] = matches[0]["hook_rva"] + 5
    helpers = descriptor["vehicle_special_fire_helpers"]
    if set(helpers) != {"refresh_ai"}:
        raise SystemExit("vehicle-special-fire-logic: incomplete rebalance helpers")
    dynamic = [hook for hook in descriptor["detours"] if hook["hook_feature"] == feature
               and hook["hook_displaced"] == "c783bc0000000000803f"]
    if len(dynamic) != 1:
        raise SystemExit(f"vehicle-special-fire-logic: expected one mount update timer, found {len(dynamic)}")
    clients = [hook for hook in descriptor["detours"] if hook["hook_feature"] == feature
               and hook["hook_displaced"] == "498b5f60488bc3"]
    if len(clients) != 1:
        raise SystemExit(f"vehicle-special-fire-logic: expected one client mount event, found {len(clients)}")
    client_resume = clients[0]["hook_rva"] + 7
    client_skip = descriptor["vehicle_special_fire_client_skip"]
    if client_skip <= client_resume:
        raise SystemExit("vehicle-special-fire-logic: client mount event skip precedes its continuation")
    return {
        "vehicle_special_fire_original": targets.pop(),
        "vehicle_special_fire_guard_resume": resume,
        "vehicle_special_fire_guard_skip": resume + int.from_bytes(displaced[4:8], "little", signed=True),
        "vehicle_special_fire_source": UNIT_BASE,
        "vehicle_special_fire_dynamic_resume": dynamic[0]["hook_rva"] + 10,
        "vehicle_special_fire_dynamic_client_resume": client_resume,
        "vehicle_special_fire_dynamic_client_skip": client_skip,
        **tails,
        **{f"vehicle_special_fire_{name}": rva for name, rva in helpers.items()},
    }


def build_unit(plugin, module, spec, descriptor, names, placeholders, claimed):
    name = f"{plugin}-{module}"
    feature = spec[0]
    symbol_overrides = {}
    if plugin == "vehicle-special-fire" and module == LOGIC:
        symbol_overrides = vehicle_special_fire_symbol_overrides(descriptor, feature)
    blob, labels, cells, code_start = assemble_unit(plugin, module, spec, symbol_overrides)
    fixups = code_fixups(name, blob, code_start)
    for fixup, placeholder in placeholder_fixups(blob, placeholders):
        if placeholder in claimed:
            raise SystemExit(f"{name}: the placeholder {placeholder:#x} is also in {claimed[placeholder]}")
        claimed[placeholder] = name
        fixups.append(fixup)
    writes = []
    for owner, write in descriptor_writes(descriptor, module, names):
        if owner != feature:
            continue
        if write["kind"] != "edit":
            found = {labels[label] for label in write.pop("labels") if label in labels}
            if len(found) != 1:
                raise SystemExit(f"{name}: the write at {write['rva']:#x} reaches {len(found)} unit offsets")
            write["entry"] = found.pop()
            write["label"] = next(label for label, at in sorted(labels.items()) if at == write["entry"])
        writes.append(write)
    edits = {w["rva"] for w in writes if w["kind"] == "edit"}
    fixups += [{"kind": "edit", "rva": f["edit_rva"], "offset": f["edit_offset"], "target": f["edit_target"]}
               for f in descriptor.get("edit_fixups", []) if f["edit_rva"] in edits]
    # the pose split finds each stock function from its caller's return point,
    # lea rax, [return point + stock - return point]: one disp32 per call site
    code = list(md.disasm(blob[code_start:], UNIT_BASE + code_start))
    for write in writes:
        if write["kind"] != "call" or not write.get("stock") or write["label"] == "chooser":
            continue
        back, stock = write["rva"] + 5, write["stock"]
        leas = [ins for ins in code if ins.mnemonic == "lea" and any(
            op.type == capstone.x86.X86_OP_MEM and op.mem.disp == stock - back for op in ins.operands)]
        if len(leas) != 1:
            raise SystemExit(f"{name}: {len(leas)} leas of {stock - back:#x} for the call at {write['rva']:#x}")
        fixups.append({"kind": "delta", "offset": leas[0].address - UNIT_BASE + leas[0].disp_offset,
                       "from": back, "to": stock})
    required = [w["rva"] for w in writes]
    required += [f["target"] for f in fixups if f["kind"] in ("abs64", "rel32", "edit")]
    required += [f["from"] for f in fixups if f["kind"] == "delta"]
    natives = [{"name": n, "rva": rva} for n, rva in NATIVES.get((plugin, module), [])]
    if any(not any(w["rva"] == n["rva"] and w["kind"] == "jmp" for w in writes) for n in natives):
        raise SystemExit(f"{name}: a native entry has no jmp write")
    named = [{"name": n, "offset": cells[c][0] + extra, "bytes": 8}
             for n, c, extra in NAMED_CELLS.get((plugin, module), [])]
    return name, blob, {
        "unit_schema": 1,
        "name": name,
        "plugin": plugin,
        "module": f"{module}.dll",
        "source_sha256": descriptor["source_sha256"],
        "unit_bytes": len(blob),
        "code_offset": code_start,
        "cells": [{"name": n, "offset": at, "bytes": size} for n, (at, size) in cells.items()] + named,
        "writes": sorted(writes, key=lambda w: w["rva"]),
        "fixups": fixups,
        "natives": natives,
        "anchors": [],
        "sites": covering(descriptor["sites"], required),
        "verified_sha": descriptor["verified_sha"],
        "labels": dict(sorted(labels.items(), key=lambda item: (item[1], item[0]))),
    }


def core_units(logic, game):
    """Core's own units: no code, only the build anchors every other unit
    relies on, and the sites of the functions Core hooks itself."""
    def core(module, descriptor, site_names):
        anchor = {"rva": descriptor["anchor_rva"], "bytes": descriptor["anchor"]}
        sites = covering(descriptor["sites"], [anchor["rva"]])
        sites += [e for e in descriptor["sites"] if e["site_name"] in site_names and e not in sites]
        return f"core-{module}", b"", {
            "unit_schema": 1, "name": f"core-{module}", "plugin": "core",
            "module": f"{module}.dll", "source_sha256": descriptor["source_sha256"],
            "unit_bytes": 0, "code_offset": 0, "cells": [], "writes": [], "fixups": [], "natives": [],
            "anchors": [anchor], "sites": sites, "verified_sha": descriptor["verified_sha"], "labels": {}}
    return [core(LOGIC, logic, []),
            core(GAME, game, ["lobby_connect", "tactical_state_ctor", "tactical_state_dtor"])]


def main():
    name = None
    if "--layout" in sys.argv:
        name = sys.argv[sys.argv.index("--layout") + 1]
        os.environ["DEFIANCE_LAYOUT"] = name
    active, profile = b.active_profile()
    if profile:
        b.load_layout(profile)
    suffix = f"-{active}" if active else ""
    out = pathlib.Path("out") / f"units{suffix}"
    read = lambda stem: json.loads((pathlib.Path("out") / stem).read_text(encoding="utf-8"))
    logic, game = read(f"payload{suffix}.json"), read(f"payload-game{suffix}.json")
    names = {LOGIC: {int(k): v for k, v in read(f"payload{suffix}.labels.json").items()},
             GAME: {int(k): v for k, v in read(f"payload-game{suffix}.labels.json").items()}}
    placeholders = {
        LOGIC: [(placeholder, {"kind": "abs64", "target": rva + len(bytes.fromhex(displaced))})
                for rva, displaced, _, placeholder in b.TRACE_HOOKS],
        GAME: [(placeholder, {"kind": "abs64", "target": rva}) for placeholder, rva, _, _ in icon.FIXUPS]
        + [(placeholder, {"kind": "export", "dll": dll, "name": fn}) for placeholder, dll, fn in icon.EXPORTS],
    }
    units, claimed = core_units(logic, game), {}
    for plugin, modules in UNITS.items():
        for module, spec in modules.items():
            descriptor = logic if module == LOGIC else game
            units.append(build_unit(plugin, module, spec, descriptor, names[module],
                                    placeholders[module], claimed))
    missing = [f"{p:#x}" for module in placeholders.values() for p, _ in module if p not in claimed]
    if missing:
        raise SystemExit(f"placeholders no unit holds: {', '.join(missing)}")
    # every monolithic write belongs to exactly one unit
    for module, descriptor in ((LOGIC, logic), (GAME, game)):
        total = len(descriptor_writes(descriptor, module, names[module]))
        held = sum(len(u[2]["writes"]) for u in units if u[2]["module"] == f"{module}.dll")
        if held != total:
            raise SystemExit(f"{module}.dll: units hold {held} of the {total} writes")
    out.mkdir(parents=True, exist_ok=True)
    for stale in out.glob("*"):
        stale.unlink()
    for unit, blob, descriptor in units:
        (out / f"{unit}.bin").write_bytes(blob)
        (out / f"{unit}.json").write_text(json.dumps(descriptor, indent=1) + "\n",
                                          encoding="utf-8", newline="\n")
        print(f"{unit:<22} {len(blob):5} bytes, {len(descriptor['writes']):2} writes, "
              f"{len(descriptor['fixups']):3} fixups, {len(descriptor['sites']):2} sites")
    print(f"units -> {out}")


if __name__ == "__main__":
    main()
