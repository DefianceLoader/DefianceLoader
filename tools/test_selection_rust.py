"""Compare compiled Rust soldier methods against the full assembly functions."""
import ctypes
import pathlib
import random
import struct
import sys
import test_firing as f


def main():
    source = pathlib.Path(sys.argv[1]).resolve()
    core = ctypes.CDLL(str(source / "defiance_plugin_core.dll"))
    core.defiance_test_game_access.restype = ctypes.c_void_p
    rust = ctypes.CDLL(str(source / "defiance_plugin_feature_selection.dll"))
    rust.defiance_test_selection_api.argtypes = [ctypes.c_void_p]
    assert rust.defiance_test_selection_api(core.defiance_test_game_access()) == 0
    set_rust = rust.defiance_test_selection_set
    set_rust.argtypes = [ctypes.c_void_p, ctypes.c_ubyte]
    set_rust.restype = None
    get_rust = rust.defiance_test_selection_get
    get_rust.argtypes = [ctypes.c_void_p]
    get_rust.restype = ctypes.c_ubyte
    code, labels = f.b.assemble(pathlib.Path("patch/soldier-mark.asm").read_text().splitlines(), 0, 0)
    entry = f.obj(len(code))
    f.put(entry, code)
    set_reference = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_ubyte)(entry)
    get_reference = ctypes.CFUNCTYPE(ctypes.c_ubyte, ctypes.c_void_p)(entry + labels["is_selected"])
    parent_set = f.obj(32)
    f.put(parent_set, bytes.fromhex("885130ff4134c3"))
    parent_get = f.obj(32)
    f.put(parent_get, bytes.fromhex("0fb64130c3"))
    parent_vt = f.obj(0x60, [(0x50, parent_set), (0x58, parent_get)])
    child_vt = f.obj(0x60, [(0x50, entry), (0x58, entry + labels["is_selected"])])
    members = [f.soldier() for _ in range(12)]
    ai = f.squad(members, 0)
    holder = f.q(ai + 0x10)
    entity = f.q(holder + 0x10)
    facets = f.q(entity + 8)
    own = f.q(facets + 0x50)
    vector = f.q(f.q(f.q(facets + 0x28) + 8) + 8)
    begin = f.q(vector)
    dead = f.obj(0x40)
    foreign = f.obj(0x40, [(0, parent_vt)])
    for _, facet in members:
        f.put(facet, struct.pack("<Q", child_vt))
    f.put(own, struct.pack("<Q", parent_vt))
    blocks = list(f.HEAP)
    def snapshot():
        return [ctypes.string_at(p, 0x1000) for p in blocks]
    def restore(data):
        for p, content in zip(blocks, data):
            f.put(p, content)
    rng = random.Random(0x5E1EC7)
    for case in range(12000):
        count = rng.randrange(13)
        f.put(vector + 8, struct.pack("<Q", begin + count * 8))
        f.put(own + 0x10, struct.pack("<Q", 0 if case % 17 == 0 else holder))
        f.put(own + 0x30, bytes([rng.choice([0, 1, 255])]))
        f.put(foreign + 0x30, bytes([rng.choice([0, 1, 255])]))
        for _, facet in members:
            f.put(facet + 0x18, bytes([rng.choice([0, 1, 2, 255])]))
            f.put(facet + 0x30, bytes([rng.choice([0, 1, 255])]))
            f.put(facet + 0x28, struct.pack("<Q", rng.choice([own, own, own, 0, 1, dead, foreign, 0x800000000000])))
        before = snapshot()
        for _, facet in members:
            assert get_rust(facet) == get_reference(facet), (case, "getter before set")
        assert snapshot() == before, (case, "getter modified memory")
        target = rng.choice(members)[1]
        value = rng.choice([0, 0, 1, 2, 255])
        set_reference(target, value)
        expected = snapshot()
        answers = [get_reference(facet) for _, facet in members]
        restore(before)
        set_rust(target, value)
        assert snapshot() == expected, (case, "setter memory mismatch", value)
        assert [get_rust(facet) for _, facet in members] == answers, (case, "getter after set")
        assert snapshot() == expected, (case, "getter after set modified memory")
    print("PASS 12000 selection cases: getter/setter, forwarding counts, stale marks and full object memory match")


if __name__ == "__main__":
    main()
