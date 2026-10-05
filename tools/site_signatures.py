"""Byte signatures that find one site in every build a plugin supports.

A plugin's `sites.rs` resolves each hooked or called address by signature at
load time instead of looking it up by DLL hash (see
`plugins/cover-markers/src/sites.rs`). This writes those signatures from the
addresses already known for each build: for each site, the shortest window of
whole instructions, masked as `tools/sigs.py` masks them (rel32 branches and
RIP displacements wildcarded, everything else fixed), that matches exactly once
in every build's image and lands on that build's address. Where the builds'
code differs, the builds are split into groups that each get a signature, and
the alternatives together must still find exactly one place in every build.

    import site_signatures
    site_signatures.find("logic", {"gog-2026-09-14": 0x12fd30, ...})
        -> [("4883ec28e8????????...", 0), ...]     # (text, offset of the site)

    python tools/site_signatures.py logic gog-2026-09-14=0x12fd30 ...
"""
import functools, re, sys

import capstone

import builds
from pe import Image

MAX_BYTES = 256
# A pattern shorter than this matches somewhere else once its own site changes.
MIN_BYTES = 24
# How many instructions before the site a window may start, for a site whose
# own bytes are not unique.
MAX_BEFORE = 24


@functools.cache
def module(build, dll):
    path = builds.BIN / build.split("-", 1)[0] / build.split("-", 1)[1] / f"{dll}.dll"
    img = Image(str(path))
    return img, img.pe.get_memory_mapped_image()


_md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
_md.detail = True


def masked(ins):
    mask = bytearray(b"\x01" * ins.size)
    if ins.group(capstone.CS_GRP_BRANCH_RELATIVE) and ins.imm_size == 4:
        mask[ins.imm_offset:ins.imm_offset + 4] = b"\x00" * 4
    for op in ins.operands:
        if op.type == capstone.x86.X86_OP_MEM and op.mem.base == capstone.x86.X86_REG_RIP:
            mask[ins.disp_offset:ins.disp_offset + ins.disp_size] = b"\x00" * ins.disp_size
    return bytes(ins.bytes), bytes(mask)


def encode(parts):
    return "".join(f"{b:02x}" if m else "??"
                   for pattern, mask in parts for b, m in zip(pattern, mask))


def instructions(build, dll, rva):
    """(before, after): the instructions of the site's function before `rva`
    (nearest first) and from `rva` on."""
    img, image = module(build, dll)
    start, end = img.function_of(rva) or (rva, rva + MAX_BYTES * 2)
    end = max(end, rva + MAX_BYTES * 2)
    insns = list(_md.disasm(image[start:end], start))
    at = [i.address for i in insns]
    if rva not in at:
        raise SystemExit(f"{build} {dll} {rva:#x} is not an instruction boundary")
    k = at.index(rva)
    return [masked(i) for i in reversed(insns[:k])], [masked(i) for i in insns[k:]]


def regex(text):
    body = b"".join(b"." if text[i:i + 2] == "??" else re.escape(bytes([int(text[i:i + 2], 16)]))
                    for i in range(0, len(text), 2))
    return re.compile(body, re.DOTALL)


def hits(build, dll, text):
    """Every start of `text` in the build's whole image, as the Rust scan finds
    them (overlapping included)."""
    _, image = module(build, dll)
    r, out, pos = regex(text), [], 0
    while (m := r.search(image, pos)) is not None:
        out.append(m.start())
        pos = m.start() + 1
    return out


def window(build, dll, rva, before, after):
    pre, post = instructions(build, dll, rva)
    if before > len(pre) or after > len(post):
        return None
    parts = list(reversed(pre[:before])) + post[:after]
    at = sum(len(p) for p, _ in reversed(pre[:before]))
    return encode(parts), at


def resolves(dll, sites, signatures):
    """Whether the signatures together find exactly each build's site."""
    for build, rva in sites.items():
        found = {h + at for text, at in signatures for h in hits(build, dll, text)}
        if found != {rva}:
            return False
    return True


def group(dll, sites):
    """One signature for these builds, or None: the shortest common window
    unique at each build's site."""
    for before in range(0, MAX_BEFORE + 1):
        for after in range(1, 200):
            texts = {b: window(b, dll, rva, before, after) for b, rva in sites.items()}
            if any(t is None for t in texts.values()):
                break
            if len(set(texts.values())) != 1:
                break
            text, at = next(iter(texts.values()))
            if len(text) // 2 > MAX_BYTES:
                break
            if len(text) // 2 < MIN_BYTES:
                continue
            if all(hits(b, dll, text) == [rva - at] for b, rva in sites.items()):
                return text, at
    return None


def find(dll, sites):
    """Signatures that together resolve `sites` ({build: rva}) in every build:
    one if the builds share the code, else one per group of builds that do."""
    classes = {}
    for b, rva in sites.items():
        classes.setdefault(head(dll, b, rva), {})[b] = rva
    out = []
    for same in sorted(classes.values(), key=len, reverse=True):
        found = group(dll, same)
        if found:
            out.append(found)
            continue
        for b, rva in same.items():
            found = group(dll, {b: rva})
            if found is None:
                raise SystemExit(f"{dll} {b} {rva:#x}: no unique signature")
            out.append(found)
    out = list(dict.fromkeys(out))
    if not resolves(dll, sites, out):
        raise SystemExit(f"{dll}: the signatures for {sites} find more than their sites")
    return out


def head(dll, build, rva):
    """The site's first 24 masked bytes, which builds sharing a signature share."""
    _, post = instructions(build, dll, rva)
    return encode(post[:16])[:2 * MIN_BYTES]


def rust(name, signatures):
    """The `sig(...)` expression for a site's signatures."""
    items = ", ".join(f'("{t}", {at:#x})' for t, at in signatures)
    return f'sig("{name}", &[{items}])'


if __name__ == "__main__":
    dll, *pairs = sys.argv[1:]
    sites = {p.split("=")[0]: int(p.split("=")[1], 0) for p in pairs}
    for text, at in find(dll, sites):
        print(f"{text}  at {at:#x}")
