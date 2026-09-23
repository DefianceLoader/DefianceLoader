"""MSVC RTTI recovery: class name -> vtable(s).

Type descriptor holds the mangled name at +16. A Complete Object Locator
stores the descriptor as an image-relative offset. A vtable's slot [-1] holds
the absolute VA of its locator, so the vtable starts 8 bytes after that
pointer.
"""
import re, struct, sys
sys.path.insert(0, "tools")
from pe import Image


class Rtti:
    def __init__(self, img: Image):
        self.img = img

    def descriptors(self, pattern=r"\.\?AV"):
        """{mangled: descriptor_rva}"""
        out = {}
        for name, va, size, raw in self.img.sections:
            if name != ".data" and not name.startswith(".rdata"):
                continue
            blob = self.img.data[raw:raw + size]
            for m in re.finditer(rb"\.\?A[VU][\x21-\x7e]{2,200}?@@", blob):
                text = m.group().decode("ascii")
                out.setdefault(text, va + m.start() - 16)
        return out

    def locators(self, descriptor_rva):
        """COL records whose type-descriptor field points at this descriptor."""
        want = struct.pack("<I", descriptor_rva)
        hits = []
        for name, va, size, raw in self.img.sections:
            if not name.startswith(".rdata") and name != ".data":
                continue
            blob = self.img.data[raw:raw + size]
            start = 0
            while True:
                i = blob.find(want, start)
                if i < 0:
                    break
                start = i + 1
                # field 3 of the locator; the record begins 12 bytes earlier
                col = va + i - 12
                sig = self.img.u32(col)
                if sig in (0, 1):
                    hits.append(col)
        return hits

    def vtables(self, col_rva):
        """Vtables whose [-1] slot stores this locator's VA."""
        want = struct.pack("<Q", self.img.base + col_rva)
        hits = []
        for name, va, size, raw in self.img.sections:
            if not name.startswith(".rdata") and name != ".data":
                continue
            blob = self.img.data[raw:raw + size]
            start = 0
            while True:
                i = blob.find(want, start)
                if i < 0:
                    break
                start = i + 1
                hits.append(va + i + 8)
        return hits

    def find(self, class_name):
        """[(mangled, descriptor, [(col, [vtable, ...]), ...]), ...]"""
        out = []
        for mangled, drva in self.descriptors().items():
            if class_name in mangled:
                cols = [(c, self.vtables(c)) for c in self.locators(drva)]
                out.append((mangled, drva, cols))
        return out

    def methods(self, vtable_rva, limit=64):
        """Function pointers from a vtable, stopping at the first non-code."""
        out = []
        for n in range(limit):
            va = self.img.u64(vtable_rva + n * 8)
            if not va or va < self.img.base:
                break
            rva = va - self.img.base
            if self.img.section_of(rva) != ".text":
                break
            out.append(rva)
        return out


if __name__ == "__main__":
    img = Image()
    r = Rtti(img)
    for needle in sys.argv[1:] or ["AiPickUpOrder", "AiPickUpState", "PickUpWeapon", "WeaponSlotScriptInfo"]:
        print(f"===== {needle}")
        for mangled, drva, cols in r.find(needle):
            print(f"  {mangled}  descriptor {drva:#x}")
            for col, vts in cols:
                names = " ".join(f"{v:#x}" for v in vts) or "(none)"
                print(f"    locator {col:#x}  vtable {names}")
