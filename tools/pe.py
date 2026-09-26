"""Shared loading and lookup for logic.dll analysis.

x64 PE, so .pdata gives exact function bounds; that is the backbone here.
Addresses are image-relative (RVA) everywhere unless a name says 'file'.
"""
import bisect, functools, re, struct
import builds
import capstone, pefile

DLL = str(builds.reference().logic)


class Image:
    def __init__(self, path=DLL):
        self.pe = pefile.PE(path, fast_load=False)
        self.path = path
        self.base = self.pe.OPTIONAL_HEADER.ImageBase
        self.data = self.pe.__data__[:]
        self.sections = [
            (s.Name.rstrip(b"\x00").decode(), s.VirtualAddress,
             max(s.Misc_VirtualSize, s.SizeOfRawData), s.PointerToRawData)
            for s in self.pe.sections
        ]
        self.md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
        self.md.detail = True
        self._functions = None

    # --- address conversion ---------------------------------------------
    def rva_to_file(self, rva):
        for _, va, size, raw in self.sections:
            if va <= rva < va + size:
                return raw + (rva - va)
        return None

    def file_to_rva(self, off):
        for _, va, size, raw in self.sections:
            if raw and raw <= off < raw + size:
                return va + (off - raw)
        return None

    def section_of(self, rva):
        for name, va, size, _ in self.sections:
            if va <= rva < va + size:
                return name
        return None

    def read(self, rva, n):
        off = self.rva_to_file(rva)
        if off is None:
            return b""
        return self.data[off:off + n]

    def u32(self, rva):
        b = self.read(rva, 4)
        return struct.unpack("<I", b)[0] if len(b) == 4 else None

    def u64(self, rva):
        b = self.read(rva, 8)
        return struct.unpack("<Q", b)[0] if len(b) == 8 else None

    # --- functions from the exception directory --------------------------
    @property
    def functions(self):
        """Sorted [(start_rva, end_rva)] from .pdata RUNTIME_FUNCTIONs."""
        if self._functions is None:
            out = []
            d = self.pe.OPTIONAL_HEADER.DATA_DIRECTORY[3]  # EXCEPTION
            off = self.rva_to_file(d.VirtualAddress)
            for i in range(d.Size // 12):
                s, e, _u = struct.unpack_from("<III", self.data, off + i * 12)
                if s and e > s:
                    out.append((s, e))
            out.sort()
            self._functions = out
        return self._functions

    @functools.cached_property
    def _starts(self):
        return [s for s, _ in self.functions]

    def function_of(self, rva):
        i = bisect.bisect_right(self._starts, rva) - 1
        if i < 0:
            return None
        s, e = self.functions[i]
        return (s, e) if s <= rva < e else None

    # --- strings ---------------------------------------------------------
    @functools.cached_property
    def strings(self):
        """{text: [rva, ...]} for printable C strings of 4+ bytes."""
        found = {}
        for name, va, size, raw in self.sections:
            if not raw:
                continue
            blob = self.data[raw:raw + size]
            for m in re.finditer(rb"[\x20-\x7e]{4,}\x00", blob):
                text = m.group()[:-1].decode("ascii")
                found.setdefault(text, []).append(va + m.start())
        return found

    def string_rvas(self, text, exact=True):
        if exact:
            return self.strings.get(text, [])
        return [r for t, rs in self.strings.items() if text in t for r in rs]

    # --- cross references -------------------------------------------------
    @functools.cached_property
    def _code_ranges(self):
        return [(va, va + size, raw) for n, va, size, raw in self.sections
                if n in (".text",)]

    @functools.cached_property
    def xref_index(self):
        """{target_rva: [instruction_rva, ...]} for rip-relative operands and
        direct call/jmp. Disassembly follows .pdata function bounds: a single
        linear sweep of .text stops at the first padding byte."""
        index = {}
        for start, end in self.functions:
            off = self.rva_to_file(start)
            if off is None:
                continue
            code = self.data[off:off + (end - start)]
            for ins in self.md.disasm(code, start):
                for op in ins.operands:
                    if op.type == capstone.x86.X86_OP_MEM and op.mem.base == capstone.x86.X86_REG_RIP:
                        index.setdefault(ins.address + ins.size + op.mem.disp, []).append(ins.address)
                    elif op.type == capstone.x86.X86_OP_IMM and ins.mnemonic in ("call", "jmp"):
                        index.setdefault(op.imm, []).append(ins.address)
        return index

    def xrefs(self, rva):
        return self.xref_index.get(rva, [])

    # --- disassembly ------------------------------------------------------
    def disasm(self, rva, end=None, count=None):
        if end is None:
            f = self.function_of(rva)
            end = f[1] if f else rva + 0x200
        code = self.read(rva, end - rva)
        out = []
        for ins in self.md.disasm(code, rva):
            out.append(ins)
            if count and len(out) >= count:
                break
        return out

    def text(self, rva, end=None, count=None, annotate=True):
        lines = []
        for ins in self.disasm(rva, end, count):
            line = f"  {ins.address:#08x}  {ins.mnemonic:<9} {ins.op_str}"
            if annotate:
                note = self.annotate(ins)
                if note:
                    line = f"{line:<58} ; {note}"
            lines.append(line)
        return "\n".join(lines)

    def annotate(self, ins):
        notes = []
        for op in ins.operands:
            if op.type == capstone.x86.X86_OP_MEM and op.mem.base == capstone.x86.X86_REG_RIP:
                target = ins.address + ins.size + op.mem.disp
                s = self.cstring(target)
                if s:
                    notes.append(f'"{s[:60]}"')
                elif self.section_of(target):
                    notes.append(f"{self.section_of(target)}:{target:#x}")
            elif op.type == capstone.x86.X86_OP_IMM and ins.mnemonic in ("call", "jmp"):
                f = self.function_of(op.imm)
                if f and f[0] == op.imm:
                    notes.append(f"fn_{op.imm:x}")
        return " ".join(notes)

    def cstring(self, rva, limit=200):
        off = self.rva_to_file(rva)
        if off is None:
            return None
        end = self.data.find(b"\x00", off, off + limit)
        if end < 0:
            return None
        raw = self.data[off:end]
        if len(raw) < 4 or any(c < 0x20 or c > 0x7E for c in raw):
            return None
        return raw.decode("ascii")


if __name__ == "__main__":
    img = Image()
    print("machine  ", hex(img.pe.FILE_HEADER.Machine))
    print("imagebase", hex(img.base))
    print("sections ", [(n, hex(v), hex(s)) for n, v, s, _ in img.sections])
    print("functions", len(img.functions))
    dbg = getattr(img.pe, "DIRECTORY_ENTRY_DEBUG", [])
    for d in dbg:
        pdb = getattr(d.entry, "PdbFileName", b"")
        if pdb:
            print("pdb      ", pdb.rstrip(b"\x00").decode(errors="replace"))
    print("exports  ", len(getattr(getattr(img.pe, "DIRECTORY_ENTRY_EXPORT", None), "symbols", []) or []))
