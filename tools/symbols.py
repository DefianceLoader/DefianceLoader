"""Per-build symbol tables: native addresses found once, then only verified.

`tools/symbols/<build>.json` records, for one build's logic.dll, the RVAs each
bindings generator consumes, in a section named after that generator:

    {"logic_sha256": "...", "weapon-drops": {"spawn": "0x5634c0", ...}}

A generator reads its section and checks the bytes at each address; it does not
search the image. Searching (`locate`) runs only when a build has no section
yet, through the generator's `--discover` option, and the result is committed
for review like any generated table. The reference build's section is written
by hand from the investigation that found the functions.
"""

import hashlib
import json
import pathlib
import re

import capstone

ROOT = pathlib.Path(__file__).resolve().parents[1]
SYMBOLS = ROOT / "tools" / "symbols"

X86 = capstone.x86_const


def path(build):
    return SYMBOLS / f"{build.name}.json"


def logic_sha256(build):
    return hashlib.sha256(build.logic.read_bytes()).hexdigest()


def _load(build):
    file = path(build)
    return json.loads(file.read_text(encoding="utf-8")) if file.is_file() else {}


def section(build, consumer):
    """`consumer`'s {name: rva} for `build`, or None when it has none.

    Raises when the table names another logic.dll than the one on disk."""
    table = _load(build)
    if consumer not in table:
        return None
    if build.logic.is_file() and table["logic_sha256"] != logic_sha256(build):
        raise ValueError(f"{path(build).name} names another logic.dll than bin/ holds")
    return {name: int(rva, 16) for name, rva in table[consumer].items()}


def builds_with(consumer, candidates):
    """The builds among `candidates` whose table has a `consumer` section."""
    return [build for build in candidates if consumer in _load(build)]


def record(build, consumer, rvas):
    """Write `consumer`'s section of `build`'s table, keeping other sections."""
    table = _load(build)
    table["logic_sha256"] = logic_sha256(build)
    table[consumer] = {name: f"0x{rva:x}" for name, rva in rvas.items()}
    ordered = {"logic_sha256": table.pop("logic_sha256")}
    ordered.update(sorted(table.items()))
    SYMBOLS.mkdir(parents=True, exist_ok=True)
    path(build).write_text(json.dumps(ordered, indent=1) + "\n", encoding="utf-8", newline="\n")


def _rip_relative(ins):
    return ins.disp_offset and any(
        op.type == X86.X86_OP_MEM and op.mem.base == X86.X86_REG_RIP for op in ins.operands)


def normalized(image, start, end):
    """The instructions in start..end with every address operand blanked.

    Near call and jump targets and RIP-relative displacements move between
    builds; immediates, struct offsets, registers and the instruction stream do
    not when the function is unchanged. Equal results mean the same code."""
    out = []
    for ins in image.disasm(start, end):
        if ins.mnemonic in ("call", "jmp") or ins.mnemonic.startswith(("j", "loop")):
            if ins.operands and ins.operands[0].type == X86.X86_OP_IMM:
                out.append(ins.mnemonic)
                continue
        data = bytearray(ins.bytes)
        if _rip_relative(ins):
            data[ins.disp_offset:ins.disp_offset + 4] = bytes(4)
        out.append(bytes(data))
    return out


def _prologue(image, rva, length=48):
    """A regex over `length` bytes at `rva`, address operands as wildcards."""
    code = image.read(rva, length)
    keep = [True] * len(code)
    for ins in image.disasm(rva, rva + len(code)):
        at = ins.address - rva
        if ins.size == 5 and ins.bytes[0] in (0xE8, 0xE9):
            blank = range(at + 1, at + 5)
        elif _rip_relative(ins):
            blank = range(at + ins.disp_offset, at + ins.disp_offset + 4)
        else:
            continue
        for i in blank:
            if i < len(keep):
                keep[i] = False
    return re.compile(b"".join(re.escape(code[i:i + 1]) if k else b"." for i, k in enumerate(keep)), re.S)


def candidates(source, rva, length, target):
    """Every .text RVA in `target` whose code matches `source` `rva` (`length`
    bytes) once address operands are blanked."""
    want = normalized(source, rva, rva + length)
    found = []
    for match in _prologue(source, rva).finditer(target.data):
        at = target.file_to_rva(match.start())
        if at is not None and target.section_of(at) == ".text":
            if normalized(target, at, at + length) == want:
                found.append(at)
    return found


def _operands(ins):
    """The addresses an instruction names: its near branch target and its
    RIP-relative operand."""
    if ins.mnemonic in ("call", "jmp") or ins.mnemonic.startswith(("j", "loop")):
        if ins.operands and ins.operands[0].type == X86.X86_OP_IMM:
            yield ins.operands[0].imm
            return
    if _rip_relative(ins):
        for op in ins.operands:
            if op.type == X86.X86_OP_MEM and op.mem.base == X86.X86_REG_RIP:
                yield ins.address + ins.size + op.mem.disp


def correspondence(source, rva, target, at, length):
    """{source address: target address} for each address operand of two
    copies of one function, paired instruction by instruction."""
    pairs = {}
    for a, b in zip(source.disasm(rva, rva + length), target.disasm(at, at + length)):
        for x, y in zip(_operands(a), _operands(b)):
            pairs[x] = y
    return pairs


def locate_all(source, functions, target):
    """{name: target RVA} for `functions` ({name: (source rva, length)}).

    A function with one identical copy is located directly. One with several
    is located through a located function that names it: the copies are walked
    in step and the operand they name is the answer. Raises LookupError listing
    each function left unresolved."""
    found, pending = {}, {}
    for name, (rva, length) in functions.items():
        hits = candidates(source, rva, length, target)
        if len(hits) == 1:
            found[name] = hits[0]
        else:
            pending[name] = hits
    mapped, walked = {}, set()
    while pending:
        for name in found.keys() - walked:
            rva, length = functions[name]
            mapped.update(correspondence(source, rva, target, found[name], length))
            walked.add(name)
        progress = False
        for name, hits in list(pending.items()):
            at = mapped.get(functions[name][0])
            if at is not None and at in hits:
                found[name] = at
                del pending[name]
                progress = True
        if not progress:
            break
    if pending:
        raise LookupError("; ".join(
            f"{name}: {len(hits)} identical copies" if hits else f"{name}: no identical copy"
            for name, hits in pending.items()))
    return found, mapped
