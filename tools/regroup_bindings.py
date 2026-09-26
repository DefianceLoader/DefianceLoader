"""Generate exact-build bindings for the experimental regroup add-on.

Reads every supported build's DLLs (tools/builds.py). Each build is located
through its base, the neighbour it was derived from: masked instruction
matching ports a code location from where it is in the base, which differs
least. SHA-256 of the complete DLL gates their runtime use.

    python tools/regroup_bindings.py
"""
import hashlib
import builds
import pathlib
import re

from capstone.x86 import X86_OP_IMM, X86_OP_MEM, X86_REG_RIP
from pe import Image
from rtti import Rtti
from rustfmt import rustfmt

FUNCTIONS = {
    "spawn": 0x5138f0, "add": 0x446140,
    "remove": 0x4463d0, "remove_gunner": 0x43dc70, "cleanup": 0x43d8c0,
    "reserve": 0x55c20, "bind": 0x9dcb0, "free_strings": 0x5f4e0,
    "append": 0x5a070, "members": 0x444de0, "wire": 0x4476a0,
    "holder_init": 0x4439a0,
    "holder_ctor": 0x442600, "templates": 0x443460,
    "string_assign": 0x24690,
    "weak_bind": 0x244d0,
    "squad_update": 0x43d3c0,
    "perk_refresh": 0x331420, "perk_prepare": 0x3311c0,
    "perk_update": 0x332e80, "perk_member": 0x3315b0,
    "export_roster": 0x110240, "resize_roster": 0xcf4a0,
}
CLASSES = {"squad_ai": ".?AVSquadAiFacet@Leonardo@@",
           "human_ai": ".?AVHumanAiFacet@Leonardo@@",
           "gun": ".?AVGun@Leonardo@@"}


def pattern(image, rva, size=256):
    raw = bytearray(image.read(rva, size))
    mask = bytearray(b"\1" * len(raw))
    for ins in image.md.disasm(raw, rva):
        relative = ins.group(1) or ins.group(2)  # jump/call
        for op in ins.operands:
            if op.type == X86_OP_MEM and op.mem.base == X86_REG_RIP:
                start = ins.address - rva + ins.disp_offset
                mask[start:start + ins.disp_size] = b"\0" * ins.disp_size
            elif op.type == X86_OP_IMM and relative:
                start = ins.address - rva + ins.imm_offset
                mask[start:start + ins.imm_size] = b"\0" * ins.imm_size
    return b"".join(re.escape(bytes([v])) if m else b"." for v, m in zip(raw, mask))


def call_site(image, parent, callee):
    """The address of the direct call to `callee` inside `parent`. Found by
    content, so a recompiled parent that changed shape still works."""
    f = image.function_of(parent)
    end = f[1] if f else parent + 0x800
    for ins in image.md.disasm(image.read(parent, end - parent), parent):
        if (ins.mnemonic == "call" and ins.operands
                and ins.operands[0].type == X86_OP_IMM and ins.operands[0].imm == callee):
            return ins.address
    return None


def bindings(path, base, located, overrides=None):
    """The bindings of the logic.dll at `path`, each found from `located`, where
    it is in `base` (the base build's image and bindings)."""
    overrides = overrides or {}
    image = Image(str(path))
    text = next(s for s in image.sections if s[0] == ".text")
    blob = image.read(text[1], text[2])
    mapped = {}
    for key in FUNCTIONS:
        if key in overrides:
            mapped[key] = overrides[key]
            continue
        rva = located[key]
        hits = list(re.finditer(pattern(base, rva, 64 if key == "holder_ctor" else 256), blob, re.DOTALL))
        if len(hits) != 1:
            raise RuntimeError(f"{path}: {key} has {len(hits)} candidates")
        mapped[key] = hits[0].start() + text[1]
    # Call sites are resolved within their validated containing functions,
    # then checked to reach the independently resolved target.
    for key, parent, _delta, callee in [
        ("create_call", "holder_init", 0, "members"),
        ("wire_call", "add", 0, "wire"),
        ("templates_call", "holder_ctor", 0, "templates"),
        ("spawn_fallback_call", "spawn", 0, "string_assign"),
    ]:
        site = call_site(image, mapped[parent], mapped[callee])
        if site is None:
            raise RuntimeError(f"{path}: {key} (call to {callee}) is not in {parent}")
        mapped[key] = site
    for key, name in CLASSES.items():
        candidates = [vt for n, _, cols in Rtti(image).find(name) if n == name
                      for _, vts in cols for vt in vts]
        if len(candidates) != 1:
            raise RuntimeError(f"{path}: ambiguous {name}: {candidates}")
        mapped[key] = candidates[0]
    return image, mapped


def operand_agnostic(image, rva, size):
    """The bytes at `rva`, with every immediate and displacement wildcarded: a
    function whose field offsets moved still matches."""
    raw = bytearray(image.read(rva, size))
    mask = bytearray(b"\1" * len(raw))
    for ins in image.md.disasm(raw, rva):
        if ins.imm_size:
            mask[ins.address - rva + ins.imm_offset:ins.address - rva + ins.imm_offset + ins.imm_size] = b"\0" * ins.imm_size
        if ins.disp_size:
            mask[ins.address - rva + ins.disp_offset:ins.address - rva + ins.disp_offset + ins.disp_size] = b"\0" * ins.disp_size
    return b"".join(re.escape(bytes([v])) if m else b"." for v, m in zip(raw, mask))


# Functions whose signatures no longer follow from the base, re-derived by RTTI
# vtable slots, callers, or function pairing. Keyed by the module's sha. The
# 2026-09-14 build is the first after the reference, where these were
# refactored; its successors find them through it.
LOGIC_OVERRIDES = {
    "eb8674f1d16595a3e9cf6a9ec0062735b1184976495d8d6ade36f7e2574e8aab":
        {"add": 0x459470, "remove": 0x4597f0, "wire": 0x45a500, "squad_update": 0x4507a0,
         "perk_refresh": 0x3400c0, "export_roster": 0x118360, "resize_roster": 0xd7540},
}

# The game.dll input bindings: input dispatch, key-state update, stock keyboard
# shortcuts and world selection-manager resolution, as reference rvas. The stock
# shortcuts reference a build-specific UI service offset beyond the prologue
# (not used by the detour), so only 32 bytes of it are matched.
GAME_FUNCTIONS = [0x2db9a0, 0x2da230, 0x2da980, 0x332510, 0x1d0290]
GAME_SHORT = 0x2da980


def in_lineage_order():
    """Every supported build, each after its base."""
    return sorted(builds.supported(), key=lambda b: len(b.lineage()))


def main():
    rows = ["// Generated by tools/regroup_bindings.py; do not edit.",
            "pub struct Build { pub sha: &'static str, pub rvas: &'static [usize],",
            "pub checks: &'static [(usize, &'static [u8])] }", "pub static BUILDS: &[Build] = &["]
    keys = list(FUNCTIONS) + ["create_call", "wire_call", "templates_call", "spawn_fallback_call"] + list(CLASSES)
    found = {}  # build name -> (image, bindings)
    for build in in_lineage_order():
        path = build.require().logic
        sha = hashlib.sha256(path.read_bytes()).hexdigest()
        if build.base is None:
            base, located = Image(str(path)), dict(FUNCTIONS)
        else:
            base, located = found[build.base.name]
        image, mapped = bindings(path, base, located, LOGIC_OVERRIDES.get(sha))
        found[build.name] = image, mapped
        checks = []
        for key in FUNCTIONS:
            at = mapped[key]
            checks.append(f"(0x{at:x}, &{list(image.read(at, 19 if key == "perk_refresh" else 16))})")
        for key in ["create_call", "wire_call", "templates_call", "spawn_fallback_call"]:
            at = mapped[key]
            checks.append(f"(0x{at:x}, &{list(image.read(at, 5))})")
        rows.append(f'Build {{ sha: "{sha}", rvas: &[' + ",".join(f"0x{mapped[k]:x}" for k in keys)
                    + "], checks: &[" + ",".join(checks) + "] },")
        print(f"{path}: {len(mapped)} bindings and {len(checks)} runtime checks")
    rows.append("];")
    for n, key in enumerate(keys):
        if key not in ("members", "wire", "holder_init", "holder_ctor", "templates", "string_assign"):
            rows.append(f"pub const {key.upper()}: usize = {n};")
    rows.append("pub static GAME_BUILDS: &[Build] = &[")
    game_found = {}  # build name -> (image, rvas)
    for build in in_lineage_order():
        path = build.require().game
        image = Image(str(path))
        if build.base is None:
            base, located = image, list(GAME_FUNCTIONS)
        else:
            base, located = game_found[build.base.name]
        section = next(s for s in image.sections if s[0] == ".text")
        blob = image.read(section[1], section[2])
        rvas = []
        checks = []
        for reference_rva, rva in zip(GAME_FUNCTIONS, located):
            size = 32 if reference_rva == GAME_SHORT else 96
            f = base.function_of(rva)
            if f:
                size = min(size, f[1] - rva)
            hits = list(re.finditer(pattern(base, rva, size), blob, re.DOTALL))
            if not hits:
                hits = list(re.finditer(operand_agnostic(base, rva, size), blob, re.DOTALL))
            if len(hits) != 1:
                raise RuntimeError(f"{path}: input binding {rva:x} has {len(hits)} candidates")
            at = hits[0].start() + section[1]
            rvas.append(at)
            checks.append(f"(0x{at:x}, &{list(image.read(at, 16))})")
        game_found[build.name] = image, rvas
        sha = hashlib.sha256(path.read_bytes()).hexdigest()
        rows.append(f'Build {{ sha: "{sha}", rvas: &[' + ",".join(f"0x{at:x}" for at in rvas)
                    + "], checks: &[" + ",".join(checks) + "] },")
        print(f"{path}: input dispatch at {rvas[0]:#x}, {len(checks)} runtime checks")
    rows.append("];")
    target = pathlib.Path("plugins/regroup/src/bindings.rs")
    target.write_text("\n".join(rows) + "\n", encoding="utf-8")
    rustfmt(target)


if __name__ == "__main__":
    main()

