"""Check that every vtable call in patch/*.asm reaches its class's method in
every supported build.

The game and logic layouts (tools/layouts/*.json) rewrite a payload's
`call [reg+disp]` by operand shape, not by class, so one table entry moves a
slot for every class the patches call through that shape. A slot two classes
share can move for one and not the other. So each vtable call names its class
in a `vt:` tag that opens its comment:

    call qword ptr [rax + 0x58]     ; vt:logic/SelectableFacet@Leonardo selected?

The tag is `vt:<dll>/<Class@Namespace>`, the class as its RTTI names it
without `.?AV` and `@@`, in logic, game, essence or storage (the DLL that
holds the class, which need not be the one whose payload assembles the call).
A call that may reach several classes names each, separated by `|`; each is
checked, and each may reach its own override.

For each tagged logic or game class this resolves the class's vtable by RTTI
in the reference and in each build, aligns the build's methods to the
reference's by body (instructions with immediates and displacements masked),
and requires the displacement the build's layout and symbols assemble to name
the method aligned with the reference slot. A slot whose body changed in a run
of equally many changed methods aligns by position; one that does not align
at all must have an unchanged body where the payload calls. essence.dll and
storage.dll are not in bin/, so a call into either must assemble to its
reference displacement.

    python tools/test_vtable_slots.py
"""
import collections, difflib, hashlib, json, re, sys
import builds
import chainlayout
import units
from pe import Image
from rtti import Rtti
import build as b

CALL = re.compile(r"\s*(call|jmp)\s+(?:qword ptr\s+)?\[\s*(r[a-z0-9]+)\s*\+\s*([^\]]+?)\s*\]", re.I)
TAG = re.compile(r"vt:(\S+)")
DLLS = ("logic", "game", "essence", "storage")
FIXED = ("essence", "storage")
REFERENCE_SYMBOLS = {"logic": b.REFERENCE_SYMBOLS, "game": b.REFERENCE_GAME_SYMBOLS}

failures = 0


def check(label, ok, detail=""):
    global failures
    if not ok:
        failures += 1
    print(f"{'ok  ' if ok else 'FAIL'}  {label}{f' ({detail})' if detail and not ok else ''}")


def payload_files():
    """{patch path: the module whose payload assembles it}, includes expanded
    under the file that includes them."""
    owner = {}
    for modules in units.UNITS.values():
        for module, (_, _, routines) in modules.items():
            for sources, _, _ in routines:
                for path in sources:
                    owner[builds.ROOT / path] = module
    for name in chainlayout.EXTRA_LOGIC_PATCHES:
        owner[chainlayout.PATCH / name] = "logic"
    return owner


def calls():
    """[(where, payload module, code, [(dll, class)])] for every vtable call
    the payloads assemble."""
    owner = payload_files()
    found, seen = [], set()
    for path in sorted(builds.ROOT.glob("patch/*.asm")):
        for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            code, _, comment = raw.partition(";")
            if not CALL.fullmatch(code.rstrip()):
                continue
            where = f"{path.relative_to(builds.ROOT).as_posix()}:{number}"
            module = owner.get(path) or next((m for p, m in owner.items() if included(p, path)), None)
            if module is None:
                check(f"{where}: assembled by some payload", False, "no unit lists it")
                continue
            tag = TAG.match(comment.strip())
            classes = [tuple(part.split("/", 1)) for part in tag.group(1).split("|")] if tag else []
            if not classes or any(len(c) != 2 or c[0] not in DLLS for c in classes):
                check(f"{where}: tagged vt:<{'|'.join(DLLS)}>/<Class>", False, raw.strip())
                continue
            found.append((where, module, code.strip(), classes))
            seen.add(path)
    return found


def included(parent, path):
    return any(re.fullmatch(rf'\s*%include\s+"{re.escape(path.name)}"\s*', line.split(";")[0])
               for line in parent.read_text(encoding="utf-8").splitlines())


def displacement(code, module, layout, symbols):
    text = b.apply_layout(code, layout)
    for name, value in symbols.items():
        text = text.replace(f"{{{name}}}", hex(value))
    disp = CALL.fullmatch(text).group(3)
    if not re.fullmatch(r"0x[0-9a-fA-F]+|\d+", disp):
        raise SystemExit(f"{code!r}: unresolved displacement {disp!r} for the {module} payload")
    return int(disp, 0)


class Module:
    """One DLL of one build: its classes' methods and masked bodies."""

    def __init__(self, path):
        self.img = Image(str(path))
        self.rtti = Rtti(self.img)
        self.tables, self.bodies = {}, {}

    def methods(self, name):
        if name not in self.tables:
            self.tables[name] = self._methods(name)
        return self.tables[name]

    def _methods(self, name):
        mangled = f".?AV{name}@@".encode()
        rvas = [va + i - 16 for section, va, size, raw in self.img.sections
                if section == ".data" or section.startswith(".rdata")
                for i in [self.img.data[raw:raw + size].find(mangled)] if i >= 0]
        if len(rvas) != 1:
            return None
        tables = [vt for col in self.rtti.locators(rvas[0]) if self.img.u32(col + 4) == 0
                  for vt in self.rtti.vtables(col)]
        return self.rtti.methods(tables[0], limit=400) if len(tables) == 1 else None

    def body(self, rva):
        """A hash of the function at rva with immediates and displacements
        zeroed, so a function moved or relinked still matches itself."""
        if rva not in self.bodies:
            span = self.img.function_of(rva)
            end = span[1] if span and span[0] == rva else rva + 32
            out = bytearray()
            for ins in self.img.md.disasm(self.img.read(rva, min(end - rva, 4096)), rva):
                raw = bytearray(ins.bytes)
                for offset, size in ((ins.imm_offset, ins.imm_size), (ins.disp_offset, ins.disp_size)):
                    if size:
                        raw[offset:offset + size] = bytes(size)
                out += raw
            self.bodies[rva] = hashlib.sha1(out).hexdigest()
        return self.bodies[rva]


def alignment(ref, new, name):
    """{reference slot: build slot} for one class."""
    a = [ref.body(r) for r in ref.methods(name)]
    c = [new.body(r) for r in new.methods(name)]
    out = {}
    for op, i1, i2, j1, j2 in difflib.SequenceMatcher(None, a, c, autojunk=False).get_opcodes():
        if op == "equal" or (op == "replace" and i2 - i1 == j2 - j1):
            out.update((i1 + k, j1 + k) for k in range(i2 - i1))
    return out


def main():
    found = calls()
    print(f"{len(found)} tagged vtable calls\n")
    reference = builds.reference()
    if not reference.present:
        print(f"skip  {reference.name}: DLLs not in bin/")
        return
    ref = {dll: Module(getattr(reference, dll)) for dll in ("logic", "game")}

    # The tags must name real classes with a method at the call's slot. A call
    # naming several may reach a different override in each.
    ref_slot = {}
    for where, module, code, classes in found:
        disp = displacement(code, module, {}, REFERENCE_SYMBOLS[module])
        for dll, name in classes:
            if dll in FIXED:
                continue
            methods = ref[dll].methods(name)
            if methods is None or disp // 8 >= len(methods) or disp % 8:
                check(f"{where}: {dll}/{name} has a method at +{disp:#x}", False,
                      "no such class" if methods is None else f"{len(methods)} slots")
                continue
        ref_slot[where] = disp

    results = collections.Counter()
    for build in builds.supported():
        if build.name == reference.name:
            continue
        if not build.present:
            print(f"skip  {build.name}: DLLs not in bin/")
            continue
        profile = json.loads(build.layout.read_text(encoding="utf-8")) if build.layout else {}
        layouts = {"logic": b.class_offsets(profile.get("logic_layout", {})),
                   "game": b.class_offsets(profile.get("game_layout", {}))}
        symbols = {"logic": {**b.REFERENCE_SYMBOLS, **profile.get("logic_symbols", {})},
                   "game": {**b.REFERENCE_GAME_SYMBOLS, **profile.get("game_symbols", {})}}
        new = {dll: Module(getattr(build, dll)) for dll in ("logic", "game")}
        aligned = {}
        bad = 0
        for where, module, code, classes in found:
            if where not in ref_slot:
                continue
            want = ref_slot[where]
            got = displacement(code, module, layouts[module], symbols[module])
            for dll, name in classes:
                if dll in FIXED:
                    ok, detail = got == want, f"assembles +{got:#x}, {dll} slots never move"
                elif new[dll].methods(name) is None:
                    ok, detail = False, f"{dll}/{name} not resolved by RTTI"
                else:
                    if (dll, name) not in aligned:
                        aligned[dll, name] = alignment(ref[dll], new[dll], name)
                    slot = aligned[dll, name].get(want // 8)
                    methods = new[dll].methods(name)
                    if slot is not None:
                        ok = got == slot * 8
                        detail = f"assembles +{got:#x}, the reference method is at +{slot * 8:#x}"
                    else:
                        body = ref[dll].body(ref[dll].methods(name)[want // 8])
                        ok = got // 8 < len(methods) and new[dll].body(methods[got // 8]) == body
                        detail = f"+{want:#x} does not align and +{got:#x} has another body"
                if not ok:
                    bad += 1
                    check(f"{build.name}: {where} reaches {dll}/{name}'s method", False, detail)
                results[ok] += 1
        check(f"{build.name}: every vtable call reaches its class's method", bad == 0, f"{bad} miss")
    print(f"\n{results[True]} call-class-build checks passed")


main()
print()
print(f"{failures} failed" if failures else "all cases as expected")
sys.exit(1 if failures else 0)
