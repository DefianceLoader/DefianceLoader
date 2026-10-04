"""Generate the experimental primary weapon drop bindings for every build whose
symbol table (tools/symbols.py) has a `weapon-drops` section.

    python tools/weapon_drop_bindings.py               # verify and generate
    python tools/weapon_drop_bindings.py --discover B  # find build B's addresses

Steam 2026-09-25 is the reference: its section was written from the native
investigation. `--discover` finds each function in another build as the one
copy whose code matches the reference's with address operands blanked, places
the call sites at the same offsets inside them, and records the result.
"""

import argparse
import hashlib
import pathlib
import struct
import sys
import capstone

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

import builds
import symbols
from pe import Image
from rustfmt import rustfmt


CONSUMER = "weapon-drops"
REFERENCE = "steam-2026-09-25"

DEATH_AFTER = bytes((0xC6, 0x47, 0x18, 0x00))
COLLECT_BEFORE = bytes.fromhex("48 89 5c 24 08")
REBUILD_BEFORE = bytes.fromhex("48 89 4c 24 08")
RESERVE_SITES = {
    "reserve_writer": bytes.fromhex("48 89 5c 24 10"),
    "reserve_load": bytes.fromhex("48 89 54 24 10"),
    "reserve_destroy": bytes.fromhex("48 89 5c 24 08"),
}
# The ammo share getter is a leaf without an unwind record: its byte length.
LEAF_LENGTHS = {"ammo share getter": 0x7A}
# Call sites, each inside a function at the same offset in every build: the
# site's symbol, its function, and the function the call reaches.
CALL_SITES = (
    ("death_call", "damage callback", "original death"),
    ("collect_drop", "collect", "drop special"),
    ("collect_add", "collect", "pickup add"),
)
# Human vtable slots 0 and 1, checked through `human_vtable`.
HUMAN_METHODS = ("human vfunc0", "human vfunc1")
FUNCTIONS = (
    "damage callback",
    "manager_get",
    "spawn",
    "slot_context",
    "slot_type",
    "item_override",
    "canonical",
    "holder_get",
    "original death",
    "collect",
    "squad cache rebuild",
    "squad member equipment",
    "squad member refresh",
    "squad reinforcement",
    "drop special",
    "pickup add",
    "detach",
    "ammo_dispose",
    "primary_add",
    "ammo_mode",
    "ammo_remove",
    "ammo share getter",
    "import_rounds",
    "ammo capacity registration",
    "ammo_set_record",
    "weak_bind",
    "reserve_writer",
    "reserve_load",
    "reserve_destroy",
    "string_copy",
    "string_destroy",
    "ammo carrier registration",
    "gun constructor",
    "carried geometry builder",
    "visual_sync",
    "visual_switch",
    "visual swap",
    "visual move",
)
HELPERS = (
    "manager_get", "spawn", "slot_context", "slot_type", "item_override", "canonical",
    "holder_get", "detach", "ammo_dispose", "primary_add", "ammo_mode", "ammo_remove",
    "import_rounds", "ammo_set_record", "weak_bind", "string_copy", "string_destroy",
    "visual_switch", "visual_sync",
)

OUT = ROOT / "plugins" / "weapon-drops" / "src" / "sites.rs"


def label(build):
    store = {"gog": "GOG", "steam": "Steam"}[build.store]
    return f"{store} {build.date}"


def call_target(rva, code):
    """Return the destination of a five-byte near call, rejecting other code."""
    if len(code) != 5 or code[0] != 0xE8:
        raise ValueError(f"call site at {rva:#x} is not a five-byte near call")
    return rva + len(code) + struct.unpack("<i", code[1:])[0]


def relocation_ranges(image):
    """Yield base-relocation byte ranges in the mapped image."""
    directory = image.pe.OPTIONAL_HEADER.DATA_DIRECTORY[5]
    if not directory.VirtualAddress or not directory.Size:
        return
    widths = {1: 2, 2: 2, 3: 4, 4: 2, 10: 8}
    for block in image.pe.DIRECTORY_ENTRY_BASERELOC:
        for entry in block.entries:
            if entry.type == 0:  # IMAGE_REL_BASED_ABSOLUTE padding
                continue
            width = widths.get(entry.type)
            if width is None:
                raise ValueError(f"unsupported base relocation type {entry.type}")
            yield entry.rva, entry.rva + width


def function_span(image, name, rva):
    span = image.function_of(rva)
    if span is None:
        length = LEAF_LENGTHS.get(name)
        if length is None:
            raise ValueError(f"{name} at {rva:#x} has no .pdata function entry")
        end = rva + length
        code = image.read(rva, length)
        instructions = list(image.md.disasm(code, rva))
        if (
            not instructions
            or instructions[-1].address + instructions[-1].size != end
            or instructions[-1].mnemonic != "ret"
            or any(ins.mnemonic == "call" for ins in instructions)
        ):
            raise ValueError(f"{name} leaf extent differs")
        for ins in instructions:
            if ins.mnemonic.startswith("j") and (
                not ins.operands
                or ins.operands[0].type != capstone.x86_const.X86_OP_IMM
                or not rva <= ins.operands[0].imm < end
            ):
                raise ValueError(f"{name} leaf branches outside its verified extent")
        return rva, end, hashlib.sha256(code).hexdigest()
    start, end = span
    if start != rva:
        raise ValueError(f"{name} at {rva:#x} is not a .pdata function start ({start:#x})")
    code = image.read(start, end - start)
    if len(code) != end - start or not code:
        raise ValueError(f"{name} function span at {rva:#x} is incomplete")
    return start, end, hashlib.sha256(code).hexdigest()


def semantic_function_spans(image, name, rva):
    """Find all .pdata spans reachable through a helper's direct control flow.

    MSVC can split one native method into adjacent .pdata entries. Walking
    branches and fallthroughs ensures a short entry does not leave its body
    unguarded. Calls remain separate functions and are guarded by their own
    binding when needed.
    """
    root_start, root_end, _ = function_span(image, name, rva)
    if name in LEAF_LENGTHS:
        return [function_span(image, name, rva)]
    pending = [rva]
    seen = set()
    spans = {(root_start, root_end)}
    while pending:
        at = pending.pop()
        if at in seen or image.section_of(at) != ".text":
            continue
        seen.add(at)
        instructions = list(image.md.disasm(image.read(at, 15), at, count=1))
        if not instructions:
            raise ValueError(f"cannot decode reachable {name} code at {at:#x}")
        ins = instructions[0]
        # MSVC pads a noreturn-call epilogue with a trap outside .pdata.
        # A trap has no successor and is not an unguarded function body.
        if ins.mnemonic.lower() == "int3":
            continue
        span = image.function_of(ins.address)
        if span is None:
            raise ValueError(f"reachable {name} code at {at:#x} has no .pdata span")
        spans.add(span)
        mnemonic = ins.mnemonic.lower()
        next_at = ins.address + ins.size
        if mnemonic.startswith("ret") or mnemonic in ("ud2", "int3"):
            continue

        if mnemonic == "jmp":
            if ins.operands and ins.operands[0].type == capstone.x86_const.X86_OP_IMM:
                pending.append(ins.operands[0].imm)
            continue

        if mnemonic.startswith("j") or mnemonic.startswith("loop"):
            if ins.operands and ins.operands[0].type == capstone.x86_const.X86_OP_IMM:
                pending.append(ins.operands[0].imm)
            pending.append(next_at)
            continue

        pending.append(next_at)

        if len(seen) > 10000:
            raise ValueError(f"reachable {name} code exceeds the scan limit")

    return [function_span(image, name, start) for start, _end in sorted(spans)]


def validate_relocations(image, guards):
    relocations = list(relocation_ranges(image))
    for name, start, end, _sha in guards:
        for reloc_start, reloc_end in relocations:
            if start < reloc_end and reloc_start < end:
                raise ValueError(
                    f"{name} guard {start:#x}..{end:#x} contains a base relocation "
                    f"at {reloc_start:#x}..{reloc_end:#x}"
                )


def vtable_methods(image, vtable):
    return tuple((image.u64(vtable + index * 8) or 0) - image.base for index in range(len(HUMAN_METHODS)))


def verify(build, image, rva):
    """Check `build`'s recorded addresses against its image; return its guards."""
    for site, function, target in CALL_SITES:
        start, end, _sha = function_span(image, function, rva[function])
        if not start <= rva[site] < end:
            raise ValueError(f"{site} is outside {function}")
        if call_target(rva[site], image.read(rva[site], 5)) != rva[target]:
            raise ValueError(f"{site} does not call {target}")
    if image.read(rva["death_call"] + 5, len(DEATH_AFTER)) != DEATH_AFTER:
        raise ValueError("death callback instruction after the original call changed")
    if image.read(rva["collect"], 5) != COLLECT_BEFORE:
        raise ValueError("collection entry is not the verified relocation-free instruction")
    if image.read(rva["squad cache rebuild"], 5) != REBUILD_BEFORE:
        raise ValueError("squad rebuild entry is not the verified relocation-free instruction")
    for name, before in RESERVE_SITES.items():
        if image.read(rva[name], len(before)) != before:
            raise ValueError(f"{name} entry is not the verified relocation-free instruction")
    methods = tuple(rva[name] for name in HUMAN_METHODS)
    if vtable_methods(image, rva["human_vtable"]) != methods:
        raise ValueError(f"human vtable does not start with {', '.join(HUMAN_METHODS)}")

    guards = []
    for name in FUNCTIONS:
        for start, end, sha in semantic_function_spans(image, name, rva[name]):
            guards.append((name, start, end, sha))
    validate_relocations(image, guards)
    return guards


def length(image, name, rva):
    start, end, _sha = function_span(image, name, rva)
    return end - start


def discover(name):
    """Record build `name`'s addresses, found from the reference's."""
    reference = builds.build(REFERENCE).require()
    source = Image(reference.logic)
    known = symbols.section(reference, CONSUMER)
    target_build = builds.build(name).require()
    target = Image(target_build.logic)
    functions = {
        function: (known[function], length(source, function, known[function]))
        for function in (*FUNCTIONS, *HUMAN_METHODS[1:])
    }
    try:
        found, mapped = symbols.locate_all(source, functions, target)
    except LookupError as error:
        raise SystemExit(f"{name}: not ported, the reference code differs\n  "
                         + str(error).replace("; ", "\n  "))
    for site, function, _target in CALL_SITES:
        found[site] = found[function] + known[site] - known[function]
    # Slot 0 is named only by the vtable: a candidate counts when it sits
    # beside the located slot 1, and exactly one such vtable must exist.
    first = HUMAN_METHODS[0]
    hits = []
    for candidate in symbols.candidates(
            source, known[first], length(source, first, known[first]), target):
        pointers = struct.pack("<QQ", target.base + candidate, target.base + found[HUMAN_METHODS[1]])
        start = target.data.find(pointers)
        while start != -1:
            hits.append((target.file_to_rva(start), candidate))
            start = target.data.find(pointers, start + 1)
    if len(hits) != 1:
        raise SystemExit(f"{name}: {len(hits)} human vtables found")
    hits[0], found[first] = hits[0]
    if mapped.get(known["human_vtable"], hits[0]) != hits[0]:
        raise SystemExit(f"{name}: the code names another human vtable")
    found["human_vtable"] = hits[0]
    verify(target_build, target, found)
    symbols.record(target_build, CONSUMER, {key: found[key] for key in sorted(found)})
    print(f"Recorded {label(target_build)} weapon-drop addresses")


def rust_bytes(data):
    return "&[" + ", ".join(f"0x{byte:02x}" for byte in data) + "]"


def generate():
    lines = [
        "// Generated by tools/weapon_drop_bindings.py; do not hand-edit.\n",
        "use super::{Build, Guard};\n",
        "pub(super) const BUILDS: &[Build] = &[\n",
    ]
    for build in symbols.builds_with(CONSUMER, builds.supported()):
        build.require()
        image = Image(build.logic)
        rva = symbols.section(build, CONSUMER)
        guards = verify(build, image, rva)
        digest = hashlib.sha256(image.data).hexdigest()
        lines += [
            "    Build {\n",
            f'        name: "{label(build)}",\n',
            f'        sha: "{digest}",\n',
        ]
        for field, before, name in (
            ("death_call", "death_before", "death_call"),
            ("collect", "collect_before", "collect"),
            ("collect_drop", "collect_drop_before", "collect_drop"),
            ("collect_add", "collect_add_before", "collect_add"),
            ("rebuild", "rebuild_before", "squad cache rebuild"),
        ):
            lines.append(f"        {field}: 0x{rva[name]:x},\n")
            lines.append(f"        {before}: {rust_bytes(image.read(rva[name], 5))},\n")
        lines.append(f"        human_vtable: 0x{rva['human_vtable']:x},\n")
        for field, before in RESERVE_SITES.items():
            lines.append(f"        {field}: 0x{rva[field]:x},\n")
            lines.append(f"        {field}_before: {rust_bytes(before)},\n")
        for field in HELPERS:
            lines.append(f"        {field}: 0x{rva[field]:x},\n")
        lines.append("        guards: &[\n")
        for _name, start, end, sha in guards:
            lines.append(
                f'            Guard {{ rva: 0x{start:x}, bytes: 0x{end - start:x}, sha: "{sha}" }},\n'
            )
        lines += ["        ],\n", "    },\n"]
        print(f"Verified {label(build)} logic.dll bindings")
    lines.append("];\n")
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text("".join(lines), encoding="utf-8", newline="\n")
    rustfmt(OUT)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--discover", metavar="BUILD", help="find and record BUILD's addresses")
    args = parser.parse_args()
    if args.discover:
        discover(args.discover)
    generate()
