"""Compare the compiled Rust firing controls/Core bindings with machine-code assembly."""
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
    rust = ctypes.CDLL(str(source / "defiance_plugin_feature_firing.dll"))
    rust.defiance_test_firing_api.argtypes = [ctypes.c_void_p]
    assert rust.defiance_test_firing_api(core.defiance_test_game_access()) == 0
    set_rust = rust.defiance_test_firing_set
    set_rust.argtypes = [ctypes.c_void_p, ctypes.c_ubyte]
    set_rust.restype = None
    ui_rust = rust.defiance_test_firing_ui
    ui_rust.argtypes = [ctypes.c_void_p]
    ui_rust.restype = ctypes.c_ubyte
    members = [f.soldier() for _ in range(12)]
    ai = f.squad(members, 0)
    holder = f.q(ai + 0x10)
    entity = f.q(holder + 0x10)
    facets = f.q(entity + 8)
    own = f.q(facets + 0x50)
    vector = f.q(f.q(f.q(facets + 0x28) + 8) + 8)
    begin = f.q(vector)
    blocks = list(f.HEAP)
    def snapshot():
        return [ctypes.string_at(p, 0x1000) for p in blocks]
    def restore(data):
        for p, content in zip(blocks, data):
            f.put(p, content)
    rng = random.Random(0xF1A1)
    for case in range(12000):
        count = rng.randrange(13)
        f.put(vector + 8, struct.pack("<Q", begin + count * 8))
        f.put(ai + 0x10, struct.pack("<Q", 0 if case % 29 == 0 else holder))
        f.put(facets + 0x50, struct.pack("<Q", 0 if case % 31 == 0 else own))
        f.put(ai + 0x228, bytes([rng.randrange(256)]))
        for _, facet in members:
            f.put(facet + 0x18, bytes([rng.randrange(2)]))
            f.put(facet + 0x30, bytes([rng.randrange(2)]))
            f.put(facet + 0x1a, bytes([rng.randrange(256), rng.randrange(256)]))
            f.put(facet + 0x1c, struct.pack("<H", rng.choice([0x7a5f, 0, 0xffff])))
            f.put(facet + 0x28, struct.pack("<Q", rng.choice([own, own, 0])))
        before = snapshot()
        expected_ui = f.UI(ai)
        assert snapshot() == before, (case, "assembly UI modified memory")
        assert ui_rust(ai) == expected_ui, (case, "UI before set")
        assert snapshot() == before, (case, "Rust UI modified memory")
        value = rng.randrange(256)
        f.SET(ai, value)
        expected = snapshot()
        expected_ui = f.UI(ai)
        restore(before)
        set_rust(ai, value)
        assert snapshot() == expected, (case, "setter memory differs", value)
        assert ui_rust(ai) == expected_ui, (case, "UI after set")
        assert snapshot() == expected, (case, "UI after set modified memory")
    print("PASS 12000 firing cases: Rust/Core and assembly decisions and complete fixture memory match")


if __name__ == "__main__":
    main()
