"""Dump a function, or the function containing an address, with annotations."""
import sys
sys.path.insert(0, "tools")
from pe import Image
import index as idx


def main():
    img = Image()
    xr = idx.load(img)
    for arg in sys.argv[1:]:
        rva = int(arg, 16)
        f = img.function_of(rva)
        if not f:
            print(f"{rva:#x} is not inside a .pdata function")
            continue
        start, end = f
        callers = xr.get(start, [])
        print(f"===== fn_{start:x} .. {end:x}  ({end - start} bytes)"
              f"  callers: {' '.join(hex(c) for c in callers[:12])}"
              f"{' …' if len(callers) > 12 else ''}")
        print(img.text(start, end))
        print()


main()
