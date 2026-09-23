"""Differential test of compiled Rust bindings versus executable assembly.

Requires the parity-test pickup DLL and freshly assembled out/logic.dll.
Runs fabricated objects only; never attaches to a game process.
"""
import ctypes
import pathlib
import random
import sys

import test_chooser as fixture


def main():
    dll = ctypes.CDLL(str(pathlib.Path(sys.argv[1]).resolve()))
    choose = dll.defiance_pickup_test_choose
    choose.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_ubyte,
                       ctypes.POINTER(ctypes.c_uint32)]
    choose.restype = ctypes.c_void_p
    rng = random.Random(0xDEF1A)
    pool = [fixture.member(0, marked=True) for _ in range(12)]
    facets_pool = [fixture.peek(member, 0x110, 8) for member in pool]
    selectables = [fixture.peek(facets, 0x50, 8) for facets in facets_pool]
    squad = fixture.squad(pool, 7, 0, 12)
    array = fixture.peek(squad, 0x1e8, 8)
    script = fixture.peek(squad, 0x240, 8)
    held_values = [0, 1, 7, 8, 0x40, 0xFFFFFFFF]
    counters = [0, 1, 12, 0x7FFFFFFF, 0x80000000, 0xFFFFFFFF]
    allocations = list(fixture.HEAP)
    # Snapshot all objects/vtables, including pointees. Only the cursor may change.
    def memory():
        return [ctypes.string_at(p, 0x1000) for p in allocations]

    for case in range(12000):
        count = rng.randrange(13)
        want = rng.choice([0, 1, 7, 8, 0x40])
        allow = rng.choice([0, 1, 2, 255])
        cursor = rng.choice([0, 1, count, count + 1, 0xFFFFFFFF])
        fixture.poke(squad, 0x1f0, array + count * 8)
        fixture.poke(script, 0x1a9, int(case % 31 == 0), 1)
        fixture.poke(squad, 0x260 + want * 8, rng.choice(counters), 4)
        fixture.poke(squad, 0x264 + want * 8, rng.choice(counters), 4)
        for member, facets, selectable in zip(pool, facets_pool, selectables):
            fixture.poke(member, 0x100, rng.choice(held_values), 4)
            fixture.poke(member, 0x110, 0 if rng.randrange(12) == 0 else facets)
            fixture.poke(facets, 0x50, 0 if rng.randrange(12) == 0 else selectable)
            fixture.poke(selectable, 0x10, rng.randrange(2), 1)
            # Exercise null man; restore it on the next iteration.
            fixture.poke(member, 0x108, 0 if rng.randrange(12) == 0 else member)
        fixture.reset_cursor(cursor)
        before = memory()
        expected = fixture.call_patched(squad, want, allow)
        expected_cursor = fixture.peek(fixture.CURSOR)
        fixture.reset_cursor(cursor)
        assert memory() == before, f"assembly changed objects, case {case}"
        actual_cursor = ctypes.c_uint32(cursor)
        actual = choose(squad, want, allow, ctypes.byref(actual_cursor))
        assert (actual, actual_cursor.value) == (expected, expected_cursor), (
            case, count, want, allow, cursor, actual, expected,
            actual_cursor.value, expected_cursor)
        assert memory() == before, f"Rust changed objects, case {case}"
    print("PASS: 12000 assembly/Rust cases; entity, cursor, and object memory match")


if __name__ == "__main__":
    main()
