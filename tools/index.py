"""Build the .text cross-reference index once and cache it."""
import pickle, pathlib, sys, time
sys.path.insert(0, "tools")
from pe import Image

CACHE = pathlib.Path("out/xrefs.pkl")


def load(img=None):
    if CACHE.exists():
        with CACHE.open("rb") as fh:
            return pickle.load(fh)
    img = img or Image()
    t = time.time()
    index = img.xref_index
    CACHE.parent.mkdir(exist_ok=True)
    with CACHE.open("wb") as fh:
        pickle.dump(index, fh, protocol=4)
    print(f"indexed {len(index)} targets in {time.time() - t:.1f}s", file=sys.stderr)
    return index


if __name__ == "__main__":
    index = load()
    print(f"{len(index)} distinct targets")
    for arg in sys.argv[1:]:
        rva = int(arg, 16)
        print(f"{rva:#x}: {[hex(a) for a in index.get(rva, [])]}")
