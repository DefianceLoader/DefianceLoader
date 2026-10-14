"""Compare both example detours with the characterized native arrival callback."""
import argparse
import ctypes as C
import pathlib
import sys
import unittest

import builds
from pe import Image
from test_vehicle_arrival import Native, method
from test_vehicle_braking_examples import Harness

RUST_DLL = CPP_DLL = RUST_SOURCE = CPP_SOURCE = RUST_MANIFEST = CPP_MANIFEST = None


class Detour:
    def __init__(self, address):
        self.call = C.CFUNCTYPE(None, C.c_void_p, C.c_void_p)(address)

    def bind(self, page, percent):
        return 0

    def run(self, chassis, record):
        self.call(chassis, record)


def sample_dll(path, ident, source, percent, image=None):
    host = Harness(path, ident, source)
    if image is not None:
        host.image = (C.create_string_buffer(image, len(image))
                      if isinstance(image, (bytes, bytearray)) else image)
    host.setting = str(percent)
    host.delay_trampoline = True
    plugin, init = host.plugin()
    return host, plugin, init


class VehicleBrakingNativeTests(unittest.TestCase):
    def setUp(self):
        for path in (RUST_DLL, CPP_DLL, RUST_SOURCE, CPP_SOURCE, RUST_MANIFEST, CPP_MANIFEST):
            self.assertIsNotNone(path, "pass the six explicit DLL/source/manifest paths")
            self.assertTrue(path.is_file(), f"missing explicit test input: {path}")

    def test_signature_and_contract_resolve_on_every_supported_build(self):
        for build in builds.supported():
            with self.subTest(build=build.name):
                img = Image(build.require().logic)
                rva, code, _ = method(img)
                image = C.create_string_buffer(bytes(img.pe.get_memory_mapped_image()))
                for path, ident, source in (
                    (RUST_DLL, "example.vehicle-braking.rust", RUST_SOURCE),
                    (CPP_DLL, "example.vehicle-braking.cpp", CPP_SOURCE),
                ):
                    host, plugin, init = sample_dll(path, ident, source, 75, image)
                    contract = host.contract_fn(C.byref(host.api)).contents
                    self.assertEqual((contract.version, contract.size, contract.count),
                                     (1, C.sizeof(type(contract)), 1))
                    entry = contract.entries[0]
                    self.assertEqual(entry.rva, rva)
                    self.assertEqual(bytes(entry.before[:entry.before_len]), code[:10])
                    self.assertEqual(init(C.byref(host.api)), 0)
                    self.assertEqual(host.hooks[0][0], C.addressof(host.image) + rva)
                    self.assertEqual(host.hooks[0][2], 10)

    def test_both_detours_match_native_fixture(self):
        candidates = {build.name: build for build in builds.supported()}
        selected = {builds.reference().name}
        newest = {}
        for build in candidates.values():
            newest[build.store] = max(newest.get(build.store, build), build,
                                      key=lambda item: item.date)
        selected.update(build.name for build in newest.values())
        for name in sorted(selected):
            build = candidates[name]
            img = Image(build.require().logic)
            native = Native(img)
            try:
                for path, ident, source in (
                    (RUST_DLL, "example.vehicle-braking.rust", RUST_SOURCE),
                    (CPP_DLL, "example.vehicle-braking.cpp", CPP_SOURCE),
                ):
                    for percent in (50, 75):
                        host, plugin, init = sample_dll(path, ident, source, percent)
                        self.assertEqual(init(C.byref(host.api)), 0)
                        # Simulate the loader publishing its trampoline after init.
                        slot = host.original_slots[-1]
                        original_calls = [0]
                        @C.CFUNCTYPE(None, C.c_void_p, C.c_void_p)
                        def trampoline(chassis, record):
                            original_calls[0] += 1
                            native.run(chassis, record)
                        C.cast(slot, C.POINTER(C.c_void_p))[0] = C.cast(trampoline, C.c_void_p).value
                        detour = Detour(host.hooks[-1][1])
                        for helper in (0, 1):
                            for distance in (0.1, 2.5, 5, 10, 15, 20, 40):
                                stock = native.sample(distance, helper=helper, snapshot=True)
                                before_calls = original_calls[0]
                                updated, record, chassis = native.sample(
                                    distance, helper=helper, snapshot=True, plugin=detour,
                                    percent=percent)
                                ratio = min(stock[0][0] * 100 / percent, 1.0)
                                expected = (ratio, 10 * ratio, 6 * ratio, 0.0, 8 * ratio)
                                for got, want in zip(updated, expected):
                                    self.assertAlmostEqual(got, want, places=5)
                                self.assertEqual(chassis, native.sample(
                                    distance, helper=helper, snapshot=True)[2])
                                ignored = {i for offset in (0x1d0, 0x1d4, 0x200, 0x204, 0x208)
                                           for i in range(offset, offset + 4)}
                                baseline = native.sample(distance, helper=helper, snapshot=True)[1]
                                self.assertEqual(bytes(v for i, v in enumerate(record) if i not in ignored),
                                                 bytes(v for i, v in enumerate(baseline) if i not in ignored))
                                self.assertEqual(original_calls[0], before_calls + 1)
                                self.assertLessEqual(updated[1], 10)
                        for options in (
                            {"mode": 0}, {"mode": 2}, {"mode": 3},
                            {"endpoint": False}, {"count": 0}, {"radius": 0},
                            {"radius": float("nan")}, {"radius": -20},
                            {"radius": float("inf")},
                        ):
                            stock = native.sample(1, snapshot=True, **options)
                            before_calls = original_calls[0]
                            patched = native.sample(1, snapshot=True, plugin=detour,
                                                    percent=percent, **options)
                            self.assertEqual(patched[1:], stock[1:], options)
                            self.assertEqual(original_calls[0], before_calls + 1)
            finally:
                native.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--rust-dll", type=pathlib.Path, required=True)
    parser.add_argument("--cpp-dll", type=pathlib.Path, required=True)
    parser.add_argument("--rust-source", type=pathlib.Path, required=True)
    parser.add_argument("--cpp-source", type=pathlib.Path, required=True)
    parser.add_argument("--rust-manifest", type=pathlib.Path, required=True)
    parser.add_argument("--cpp-manifest", type=pathlib.Path, required=True)
    args, remaining = parser.parse_known_args()
    RUST_DLL, CPP_DLL = args.rust_dll, args.cpp_dll
    RUST_SOURCE, CPP_SOURCE = args.rust_source.resolve(), args.cpp_source.resolve()
    RUST_MANIFEST, CPP_MANIFEST = args.rust_manifest.resolve(), args.cpp_manifest.resolve()
    RUST_DLL, CPP_DLL = RUST_DLL.resolve(), CPP_DLL.resolve()
    unittest.main(argv=[sys.argv[0], *remaining])
