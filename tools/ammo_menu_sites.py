"""Audit every inline AmmunitionMenu capacity/layout operand on supported builds.
Reads local game DLLs; never modifies them. Generated Rust contains only patch
instructions, not extracted functions/assets. Capacity is a startup-only setting.
Every supported build (tools/builds.py) is located through its base: each
operand's signature is taken where it is in the base, the neighbour that
differs least.

    python tools/ammo_menu_sites.py
"""
import hashlib, json, pathlib, sys
import builds
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from pe import Image
from sigs import Module
from rustfmt import rustfmt

ROOT = pathlib.Path(__file__).resolve().parent.parent
COUNT = {0x3debc, 0x3e4e0, 0x3e678, 0x3f0ba, 0x3f14b}
SIZE = {0x3e4a9, 0x34d2c6, 0x4b6af9}
LAYOUT = {0x3e0b5, 0x3e135, 0x3e1af}
START, END = 0x3ddd0, 0x40be0

# The class offsets the plugin's Rust code reads, which move between builds
# (tools/offsetmap.py, confirmed against the mirrored stock functions). A build
# not listed uses the reference values.
REFERENCE_OFFSETS = dict(roster=0x3b8, gunner_count=0x130, gunner_get=0x120,
                         pool_get=0x1b8, world_player=0x700, ai_set=0x3e0)
OFFSETS = {
    "bc2af42369f9f6fe70e206ae4846f8f0e46ac01cca9a325c8159ee04a9b5e405":
        dict(roster=0x3d0, gunner_count=0x140, gunner_get=0x130, pool_get=0x1c8,
             world_player=0x708, ai_set=0x3f8),
    "d926a213731d73bac8ccc56b50e2b4292fe8613c7a9bb7c8b9fc9132c122ed25":
        dict(roster=0x3d0, gunner_count=0x140, gunner_get=0x130, pool_get=0x1c8,
             world_player=0x708, ai_set=0x3f8),
}


def offsets_for(sha):
    return OFFSETS.get(sha, REFERENCE_OFFSETS)

def catalog(image):
    result = []
    instructions = list(image.md.disasm(image.read(START, END-START), START))
    assert instructions[-1].address + instructions[-1].size == END, "incomplete class audit"
    for at in SIZE:
        instructions.append(next(image.md.disasm(image.read(at, 15), at)))
    seen = set()
    for ins in instructions:
        if ins.address in seen:
            continue
        seen.add(ins.address)
        kind = None
        if ins.address in COUNT:
            kind = "Count"
            field, width = (ins.disp_offset, ins.disp_size) if ins.mnemonic == "lea" else (ins.imm_offset, ins.imm_size)
        elif ins.address in SIZE:
            kind, field, width = "Shift", ins.imm_offset, ins.imm_size
        elif ins.address in LAYOUT:
            kind, field, width = "Zero", ins.imm_offset, ins.imm_size
        elif ins.disp_size and 0x7f8 <= ins.disp <= 0x830:
            kind, field, width = "Shift", ins.disp_offset, ins.disp_size
        elif ins.disp_size and ins.disp == 0x678:
            kind, field, width = "Extent", ins.disp_offset, ins.disp_size
        elif ins.imm_size and any(op.type == 2 and 0x7f8 <= op.imm <= 0x830 for op in ins.operands):
            kind, field, width = "Shift", ins.imm_offset, ins.imm_size
        elif ins.address == 0x3e3e6:
            kind, field, width = "Extent", ins.imm_offset, ins.imm_size
        if kind:
            assert width in (1,4)
            result.append(dict(rva=ins.address, before=bytes(ins.bytes).hex(), field=field, width=width, kind=kind, asm=ins.mnemonic+" "+ins.op_str))
    assert COUNT | SIZE | LAYOUT <= {s["rva"] for s in result}
    return sorted(result, key=lambda s:s["rva"])

# The plugin's other anchors, as reference rvas with the bytes each checks.
NAMED = [("redraw", 0x3ed10, 20), ("layout", 0x2d1920, 16)]
COMBINED = [(0x3f8a0,15),(0x3f1d0,16),(0x3f800,15),(0x3b540,15),(0x3b9f0,15),(0x3bba0,15),(0x2cb730,16),(0x2c3380,16)]


def locate(base, at, module):
    """Where the point at `at` in `base` is in `module`: its signature there,
    or, when compiler differences after it make that one ambiguous, a unique
    window growing to its left."""
    start, pattern, mask = base.signature(at, [])
    matches = module.matches(pattern, mask)
    if len(matches) != 1:
        lo, hi = base.bounds(at)
        insns = list(base.md.disasm(base.image[lo:hi],lo))
        ix = next(i for i,ins in enumerate(insns) if ins.address == at)
        for left in range(1,min(ix,16)+1):
            selected = insns[ix-left:ix+1]
            parts = [base.masked(ins) for ins in selected]
            pattern = b"".join(p for p,m in parts)
            mask = b"".join(m for p,m in parts)
            start = selected[0].address
            matches = module.matches(pattern,mask)
            if len(matches)==1 and base.matches(pattern,mask)==[start]:
                break
        else:
            raise AssertionError((at, matches))
    return matches[0] + at - start


def main():
    source = builds.reference().game
    image = Image(source)
    assert hashlib.sha256(source.read_bytes()).hexdigest() == "f0184b9fe358172c83261419c8ba3d822a0aa6b06ed3cddb2f7aa3ebb9653db4"
    sites = catalog(image)
    points = [s["rva"] for s in sites] + [at for _, at, _ in NAMED] + [at for at, _ in COMBINED]
    located = {}  # build name -> (module, {reference rva: rva})
    entries = []
    for build in sorted(builds.supported(), key=lambda b: len(b.lineage())):
        path = build.require().game
        module = Module(path)
        if build.base is None:
            where = {at: at for at in points}
        else:
            base, base_where = located[build.base.name]
            where = {at: locate(base, base_where[at], module) for at in points}
        located[build.name] = module, where
        resolved = []
        for site in sites:
            rva = where[site["rva"]]
            before = bytes.fromhex(site["before"])
            assert module.image[rva:rva+len(before)] == before, (path,site)
            resolved.append(dict(site,rva=rva))
        extra = {}
        for name, at, length in NAMED:
            rva = where[at]
            extra[name] = rva
            extra[name + "_before"] = module.image[rva:rva + length].hex()
        extra["combined"] = [(where[at], module.image[where[at]:where[at]+length].hex()) for at, length in COMBINED]
        entries.append(dict(sha=hashlib.sha256(path.read_bytes()).hexdigest(),sites=resolved,**extra))
    lines = ["// Generated by tools/ammo_menu_sites.py; do not hand-edit.\nuse super::{Build, Site, Kind, Offsets};\npub static BUILDS: &[Build] = &[\n"]
    for build in entries:
        lines.append('Build { sha: "'+build["sha"]+'", sites: &[\n')
        for s in build["sites"]:
            bytes_ = ",".join("0x"+s["before"][i:i+2] for i in range(0,len(s["before"]),2))
            lines.append(f'    Site {{ rva: 0x{s["rva"]:x}, before: &[{bytes_}], field: {s["field"]}, width: {s["width"]}, kind: Kind::{s["kind"]} }}, // {s["asm"]}\n')
        extra = ", ".join(f'{name}: 0x{build[name]:x}, {name}_before: &{list(bytes.fromhex(build[name + "_before"]))}' for name in ("redraw", "layout"))
        helpers = ",".join(f"(0x{rva:x}, &{list(bytes.fromhex(raw))})" for rva,raw in build["combined"])
        off = offsets_for(build["sha"])
        tail = ", ".join(f"{key}: 0x{off[key]:x}" for key in
                         ("roster", "gunner_count", "gunner_get", "pool_get", "world_player", "ai_set"))
        lines.append(f"], {extra}, combined: &[{helpers}], offsets: Offsets {{ {tail} }}}},\n")
    lines.append("];\n")
    (ROOT/"plugins/expanded-ammo-menu/src/sites.rs").write_text("".join(lines))
    rustfmt(ROOT/"plugins/expanded-ammo-menu/src/sites.rs")
    (ROOT/"out/ammo-menu-sites.json").write_text(json.dumps(entries,indent=2))
    print(f"Verified {len(sites)} operands per build, GOG and Steam; fixed storage, trailing fields, constructor, destructor, EH cleanup, redraw, hit testing and three-row layout")

if __name__ == "__main__":
    main()
