"""Check the built DLL: what changed, and does the new code read back right."""
import pathlib, sys
import builds
sys.path.insert(0, "tools")
from pe import Image

orig = builds.reference().logic.read_bytes()
new = pathlib.Path("out/logic.dll").read_bytes()
print(f"sizes {len(orig)} -> {len(new)}"
      f"{'  (unchanged)' if len(orig) == len(new) else '  CHANGED'}")

runs, i = [], 0
while i < min(len(orig), len(new)):
    if orig[i] != new[i]:
        j = i
        while j < len(new) and orig[j] != new[j]:
            j += 1
        runs.append((i, j - i))
        i = j
    else:
        i += 1
img = Image("out/logic.dll")
print(f"{len(runs)} changed byte ranges:")
for off, n in runs:
    rva = img.file_to_rva(off)
    print(f"   file {off:#08x}  rva {rva if rva is None else hex(rva)}  {n} bytes  "
          f"{img.section_of(rva) if rva else 'header'}")

print("\nthe new chooser as it will execute:")
f = img.function_of(0x6c0460)
print(f"  .pdata entry: {hex(f[0])}..{hex(f[1])}" if f else "  NOT in .pdata")
print(img.text(0x6c0460, f[1] if f else 0x6c0570))
print("\nthe retargeted call site:")
print(img.text(0x43b7cf, 0x43b7e0))
