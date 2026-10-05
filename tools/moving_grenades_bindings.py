"""Map the moving-grenades sites in each build for the host test.

The plugin finds its sites itself (plugins/moving-grenades/src/sites.rs);
tools/test_moving_grenades.py reads the rows this writes. Each reference RVA
below is a steam-2026-09-25 address the harness names.

RTTI identifies vtables and their method slots. Other functions are located by
masked instructions; masks cover only code-relative/RIP-relative addresses.
Object field displacements and constant arguments remain part of the match.
Interior call sites are matched inside the resolved containing function.
"""
import json
from pathlib import Path
import re

import builds
from pe import Image
from rtti import Rtti
from code_pattern import pattern

ROOT = Path(__file__).resolve().parents[1]
REFERENCE = "steam-2026-09-25"
SITES = (0x2cabb0, 0x2cc9e0, 0x2ccaf0, 0x2c28f0, 0xd9470,
         0x9e960, 0x2d8350, 0x299330, 0x2996e0, 0x2ccbe0,
         0x2d9640, 0x2cc990, 0x333239, 0x2bafa0, 0x103250, 0x2bc400)
CHECK_LENGTHS = dict(zip(SITES, (16, 16, 16, 16, 18, 20, 16, 17,
                                16, 17, 16, 17, 16, 16, 17, 20)))
CHECK_LENGTHS.update({
    0x114fb0: 16, 0x8a0c0: 16, 0x240c0: 16, 0x762f0: 4,
    0x123f0: 16, 0x2cb7a0: 16, 0x2d9744: 8, 0x116d80: 5,
    0x2d9dc0: 15, 0x24070: 9, 0x2d912e: 12,
    0x2c92b7: 5, 0x2ca0f7: 5, 0x46a740: 16, 0x333220: 25,
    0x2d3880: 16, 0x2d913a: 16, 0x2c92bc: 16, 0x2ca0fc: 16,
})
KEYS = (0x123f0, 0x24070, 0x240c0, 0x762f0, 0x8a0c0, 0x9e960, 0xd9470, 0x103250, 0x114fb0,
        0x116d80, 0x299330, 0x2996e0, 0x2bafa0, 0x2bc400, 0x2c28f0, 0x2c92b7, 0x2c92bc,
        0x2ca0f7, 0x2ca0fc, 0x2cabb0, 0x2cb7a0, 0x2cc990, 0x2cc9e0, 0x2ccaf0, 0x2ccbe0,
        0x2d3880, 0x2d8350, 0x2d912e, 0x2d913a, 0x2d9640, 0x2d9744, 0x2d9dc0, 0x333220,
        0x333239, 0x46a740, 0x705d50, 0x705f08, 0x70c068, 0x70e5d8, 0x70f430, 0x72c118,
        0x72c3b0, 0x72cd00, 0x72ced0)
EXTRA_CLASSES = ("AiHumanMoveState", "AiAttackWithMoveState", "HumanAiFacet", "Gun")
# Real native helpers used only by the ABI regression fixtures.
TEST_RVAS = (0x24670, 0x76320, 0x116dd0, 0x2d9750, 0x2d8b50, 0x2d8b6c, 0x2d9138, 0x2d9145,
             0x2d385c)


class Mapper:
    def __init__(self, reference, image):
        self.reference, self.image = reference, image
        self.mapping = {}
        self.methods = {}
        self.audit = {}
        self.rtti = Rtti(image)
        self.ref_rtti = Rtti(reference)
        text = next(s for s in image.sections if s[0] == ".text")
        self.text_rva = text[1]
        self.blob = image.read(text[1], text[2])

    def table(self, rva):
        if rva in self.mapping:
            return self.mapping[rva]
        col = self.reference.u64(rva - 8) - self.reference.base
        descriptor = self.reference.u32(col + 12)
        name = self.reference.cstring(descriptor + 16)
        offset = self.reference.u32(col + 4)
        target_descriptor = self.rtti.descriptors().get(name)
        if target_descriptor is None:
            raise ValueError(f"{self.image.path}: missing RTTI {name}")
        targets = [vt for c in self.rtti.locators(target_descriptor)
                   if self.image.u32(c + 4) == offset for vt in self.rtti.vtables(c)]
        if len(targets) != 1:
            raise ValueError(f"{self.image.path}: {name} offset {offset}: {targets}")
        result = targets[0]
        self.mapping[rva] = result
        old_methods = self.ref_rtti.methods(rva, 160)
        new_methods = self.rtti.methods(result, 160)
        if len(old_methods) != len(new_methods):
            raise ValueError(f"{self.image.path}: changed virtual slots for {name}")
        for old, new in zip(old_methods, new_methods):
            self.methods.setdefault(old, set()).add(new)
        self.audit[hex(rva)] = {"kind": "rtti", "class": name, "slots": len(new_methods)}
        return result

    def extra_tables(self):
        for fragment in EXTRA_CLASSES:
            for name, descriptor, locators in self.ref_rtti.find(fragment):
                if name != f".?AV{fragment}@Leonardo@@":
                    continue
                for col, tables in locators:
                    if self.reference.u32(col + 4) == 0:
                        for table in tables:
                            self.table(table)

    def function(self, start):
        if start in self.mapping:
            return self.mapping[start]
        methods = self.methods.get(start, set())
        if len(methods) == 1:
            result = next(iter(methods))
            self.mapping[start] = result
            self.audit[hex(start)] = {"kind": "virtual-method"}
            return result
        bounds = self.reference.function_of(start)
        leaf = bounds is None
        if leaf:
            ret = next((i for i in self.reference.disasm(start, start + 0x400)
                        if i.mnemonic == "ret"), None)
            if ret is None:
                raise ValueError(f"{hex(start)}: unbounded code has no leaf return")
            bounds = (start, ret.address + ret.size)
        elif bounds[0] != start:
            raise ValueError(f"{hex(start)}: not a bounded function start")
        # Prefer a complete function match. Large functions also use a 256-byte
        # anchor if recompilation changed their tail; record that distinction.
        sizes = sorted({bounds[1] - start, min(bounds[1] - start, 256)}, reverse=True)
        for size in sizes:
            matches = [m.start() + self.text_rva for m in
                       re.finditer(pattern(self.reference, start, size), self.blob, re.DOTALL)]
            if not leaf:
                matches = [m for m in matches if self.image.function_of(m)
                           and self.image.function_of(m)[0] == m]
            if len(matches) == 1:
                self.mapping[start] = matches[0]
                self.audit[hex(start)] = {"kind": "masked-function", "matched_bytes": size,
                                         "function_bytes": bounds[1] - start}
                return matches[0]
        raise ValueError(f"{self.image.path}: no unique function {start:#x}")

    def code(self, rva):
        if rva in self.mapping:
            return self.mapping[rva]
        if self.reference.section_of(rva) == ".rdata":
            return self.table(rva)
        bounds = self.reference.function_of(rva)
        if not bounds and 0x333220 <= rva <= 0x3332cd:
            parent = self.function(0x333220)
            result = parent + rva - 0x333220
            self.mapping[rva] = result
            self.audit[hex(rva)] = {"kind": "leaf-interior", "parent": "0x333220"}
            return result
        if not bounds:
            return self.function(rva)
        if rva == bounds[0]:
            return self.function(rva)
        parent = self.function(bounds[0])
        end = self.image.function_of(parent)[1]
        instructions = self.reference.disasm(bounds[0], bounds[1])
        # A return PC may coincide with the next instruction, or follow a CALL.
        index = next((i for i, ins in enumerate(instructions) if ins.address == rva), None)
        if index is None:
            raise ValueError(f"{rva:#x}: not an instruction boundary")
        lo = max(0, index - 5)
        begin = instructions[lo].address
        hi = min(len(instructions), index + 8)
        finish = instructions[hi - 1].address + instructions[hi - 1].size
        blob = self.image.read(parent, end - parent)
        hits = list(re.finditer(pattern(self.reference, begin, finish - begin), blob, re.DOTALL))
        if len(hits) != 1:
            raise ValueError(f"{self.image.path}: interior {rva:#x} has {len(hits)} matches")
        result = parent + hits[0].start() + rva - begin
        self.mapping[rva] = result
        self.audit[hex(rva)] = {"kind": "interior", "parent": hex(bounds[0]),
                                "matched_bytes": finish - begin}
        return result


def generate():
    reference = Image(builds.build(REFERENCE).require().logic)
    keys = sorted(set(KEYS) | set(CHECK_LENGTHS))
    rows = []
    for build in sorted((b for b in builds.supported() if b.date.startswith("2026-")), key=lambda b: b.name):
        image = Image(build.require().logic)
        mapper = Mapper(reference, image)
        for rva in keys:
            if reference.section_of(rva) == ".rdata":
                mapper.table(rva)
        mapper.extra_tables()
        for rva in keys:
            mapper.code(rva)
        for rva in TEST_RVAS:
            if rva == 0x2d385c:
                # Disposable fixture code, chosen to return at the real aim PC.
                mapper.mapping[rva] = mapper.mapping[0x2d3880] - (0x2d3880 - rva)
            elif rva == 0x2d9750:
                # Disposable candidate wrapper follows the verified call site.
                mapper.mapping[rva] = mapper.mapping[0x2d9744] + (rva - 0x2d9744)
            else:
                mapper.code(rva)
        imports = [(dll.dll, i.name, i.ordinal) for dll in reference.pe.DIRECTORY_ENTRY_IMPORT
                   for i in dll.imports if i.address - reference.base == 0x6d67e8]
        if len(imports) != 1:
            raise ValueError("reference auxiliary-release import is ambiguous")
        target_imports = [i.address - image.base for dll in image.pe.DIRECTORY_ENTRY_IMPORT
                          for i in dll.imports if (dll.dll, i.name, i.ordinal) == imports[0]]
        if len(target_imports) != 1:
            raise ValueError(f"{build.name}: auxiliary-release import is ambiguous")
        mapper.mapping[0x6d67e8] = target_imports[0]
        for call, ret in ((0x2c92b7, 0x2c92bc), (0x2ca0f7, 0x2ca0fc)):
            instruction = image.disasm(mapper.mapping[call], count=1)[0]
            if instruction.mnemonic != "call" or instruction.operands[0].imm != mapper.mapping[0x2ccbe0]:
                raise ValueError(f"{build.name}: movement-facing call changed")
            if mapper.mapping[ret] != instruction.address + instruction.size:
                raise ValueError(f"{build.name}: movement-facing return PC changed")
        row = {"name": build.name,
               "mapping": {hex(k): mapper.mapping[k] for k in keys},
               "test_mapping": {hex(k): mapper.mapping[k] for k in (*TEST_RVAS, 0x6d67e8)},
               "sites": [mapper.mapping[r] for r in SITES],
               "checks": [[r, image.read(mapper.mapping[r], n).hex()]
                          for r, n in CHECK_LENGTHS.items()], "audit": mapper.audit}
        rows.append(row)
        print(f'{build.name}: resolved {len(keys)} native addresses', flush=True)
    output = ROOT / "out/moving-grenades-sites.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(rows, indent=2) + "\n", encoding="utf-8")
    return rows


if __name__ == "__main__":
    generate()
