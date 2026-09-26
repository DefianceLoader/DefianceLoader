"""Byte signatures for the patch sites, so a build the patch was not written
for can still be patched where its code is unchanged.

A game update usually recompiles everything, which moves every function even
when the code we patch is the same. A site's signature is the bytes around it
with every field that encodes a distance to other code wildcarded: rel32 calls
and jumps, and rip-relative displacements. Everything else stays fixed, and
that includes the struct offsets and vtable slots the patch depends on, so a
build whose layout changed at a site does not match it. A signature is whole
instructions, covers every address that must move with the site (a resume
point, a branch target inside the same function), and is grown until it
matches exactly once in the module's code.

    python tools/sigs.py        check every signature against the GOG DLLs it
                                was built from, and find it in each other build
                                kept in bin/<store>/
"""
import re, sys
import builds
import capstone
import pefile
sys.path.insert(0, "tools")
from pe import Image

MAX_BYTES = 512
# Unique today is not unique in the next build: a short pattern is more likely
# to turn up somewhere else once its own site has changed, so none is shorter.
MIN_BYTES = 24


class Module:
    def __init__(self, path):
        self.pe = pefile.PE(path)
        self.image = self.pe.get_memory_mapped_image()   # indexed by rva
        self.functions = Image(path)                       # for .pdata bounds
        self.md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
        self.md.detail = True
        self.code = [(s.VirtualAddress, s.VirtualAddress + s.Misc_VirtualSize)
                     for s in self.pe.sections if s.Characteristics & 0x20000000]

    def masked(self, ins):
        """An instruction's bytes, and a mask with 0 over each field that
        encodes a distance to other code."""
        mask = bytearray(b"\x01" * ins.size)
        if ins.group(capstone.CS_GRP_BRANCH_RELATIVE) and ins.imm_size == 4:
            mask[ins.imm_offset:ins.imm_offset + 4] = b"\x00" * 4
        for op in ins.operands:
            if op.type == capstone.x86.X86_OP_MEM and op.mem.base == capstone.x86.X86_REG_RIP:
                mask[ins.disp_offset:ins.disp_offset + ins.disp_size] = b"\x00" * ins.disp_size
        return bytes(ins.bytes), bytes(mask)

    def matches(self, pattern, mask):
        """Every rva in the module's code where the masked pattern occurs,
        overlapping occurrences included, as the injector's scan finds them."""
        regex = re.compile(b"(?=" + b"".join(re.escape(bytes([b])) if m else b"."
                                             for b, m in zip(pattern, mask)) + b")", re.DOTALL)
        out = []
        for start, end in self.code:
            blob = self.image[start:end]
            out += [start + m.start() for m in regex.finditer(blob)]
        return out

    def bounds(self, rva):
        """The .pdata function around `rva`, or, for a leaf function with no
        entry, a run forward from `rva` long enough for any signature."""
        return self.functions.function_of(rva) or (rva, rva + MAX_BYTES + 64)

    def signature(self, rva, cover):
        """The shortest unique window of whole instructions around `rva` that
        spans every address in `cover` (inclusive at its end, since a resume
        point may be the address just past the window). Returns (start rva,
        pattern bytes, mask)."""
        lo, hi = min([rva] + cover), max([rva] + cover)
        f, first, last = self.bounds(rva), self.bounds(lo), self.bounds(hi)
        start, end = min(f[0], first[0]), max(f[1], last[1])
        insns = list(self.md.disasm(self.image[start:end], start))
        starts = [i.address for i in insns]
        if rva not in starts:
            raise SystemExit(f"{rva:#x} is not an instruction boundary")
        a = max(i for i, ins in enumerate(insns) if ins.address <= lo)
        b = min(i for i, ins in enumerate(insns) if ins.address + ins.size >= hi)
        while True:
            parts = [self.masked(ins) for ins in insns[a:b + 1]]
            pattern = b"".join(p for p, _ in parts)
            mask = b"".join(m for _, m in parts)
            found = self.matches(pattern, mask)
            if found == [insns[a].address] and len(pattern) >= MIN_BYTES:
                return insns[a].address, pattern, mask
            if insns[a].address not in found:
                raise SystemExit(f"the signature for {rva:#x} does not match its own site")
            if len(pattern) > MAX_BYTES:
                raise SystemExit(f"no unique signature for {rva:#x} within {MAX_BYTES} bytes")
            if b + 1 < len(insns):
                b += 1
            elif a > 0:
                a -= 1
            else:
                raise SystemExit(f"{rva:#x}: the whole function is not unique")


def branch_targets(code, rva):
    """Targets of the relative branches in `code` assembled at `rva`, as
    (offset of a rel32 field or None for a rel8, target rva)."""
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = True
    out = []
    for ins in md.disasm(code, rva):
        if ins.group(capstone.CS_GRP_BRANCH_RELATIVE):
            target = ins.operands[0].imm
            field = ins.address - rva + ins.imm_offset if ins.imm_size == 4 else None
            out.append((field, target))
    return out


def encode(pattern, mask):
    """Hex, with ?? for each wildcarded byte."""
    return "".join(f"{b:02x}" if m else "??" for b, m in zip(pattern, mask))


def site_entries(module, sites):
    """sites: (name, rva, cover). Returns descriptor entries, each with the
    window's start, its encoded pattern and the site's offset in it."""
    out = []
    for name, rva, cover in sites:
        start, pattern, mask = module.signature(rva, cover)
        # sites in one function must all move by the same distance; a leaf
        # with no .pdata entry is a group of its own
        group = (module.functions.function_of(rva) or (rva,))[0]
        out.append({"site_name": name, "site_start": start, "site_group": group,
                    "site_pattern": encode(pattern, mask), "site_offset": rva - start})
    return out


def check(module, entries, label):
    """Each signature finds exactly its own site; changing a wildcarded byte
    still finds it, and changing a fixed byte does not."""
    failures = 0
    for e in entries:
        text = e["site_pattern"]
        pattern = bytes(int(text[i:i + 2], 16) if text[i:i + 2] != "??" else 0
                        for i in range(0, len(text), 2))
        mask = bytes(0 if text[i:i + 2] == "??" else 1 for i in range(0, len(text), 2))
        found = module.matches(pattern, mask)
        ok = found == [e["site_start"]]
        wild = mask.count(0)
        print(f"{'PASS' if ok else 'FAIL'}  {label} {e['site_name']:<16} {len(mask):3} bytes, "
              f"{wild:2} wildcarded, at {e['site_start']:#x}")
        failures += not ok
    return failures


def check_other(module, entries, label):
    """In another build: each signature found exactly once, and the sites of one
    function all moved by the same distance, as the injector requires."""
    failures, moved = 0, {}
    for e in entries:
        text = e["site_pattern"]
        pattern = bytes(int(text[i:i + 2], 16) if text[i:i + 2] != "??" else 0
                        for i in range(0, len(text), 2))
        mask = bytes(0 if text[i:i + 2] == "??" else 1 for i in range(0, len(text), 2))
        found = module.matches(pattern, mask)
        ok = len(found) == 1
        if ok:
            delta = found[0] - e["site_start"]
            ok = moved.setdefault(e["site_group"], delta) == delta
        print(f"{'PASS' if ok else 'FAIL'}  {label} {e['site_name']:<16} "
              + (f"moved {delta:+#x}" if len(found) == 1 else f"found {len(found)} times"))
        failures += not ok
    return failures


if __name__ == "__main__":
    import json, pathlib
    failures = 0
    # The reference, then its release's copy in the other store, whose sites the
    # reference's signatures must find directly.
    copies = [builds.build(n) for n in builds.RELEASE_COPIES if n != builds.REFERENCE]
    for descriptor, name, module in (("out/payload.json", "logic.dll", "logic"),
                                     ("out/payload-game.json", "game.dll", "game")):
        entries = json.load(open(descriptor))["sites"]
        failures += check(Module(str(getattr(builds.reference(), module))), entries, f"gog {name}")
        for other in (b for b in copies if b.present):
            failures += check_other(Module(str(getattr(other, module))), entries, f"{other.name} {name}")
    print(f"\n{failures} failed" if failures else "\nall signatures unique at their sites")
    sys.exit(1 if failures else 0)
