"""Execute stock and patched vehicle destination slowdown on supported DLLs.

Runs a private copy of BaseTechChassisFacet::vfunc_48 on fabricated objects.
Only the direction helpers and max-speed getter are stubbed; the native
distance/radius calculation, branches and output writes execute unchanged.
The compiled plugin detour then runs on the same fixtures. Only native arrival
outputs may change; the waypoint radius and all other object fields must match.
Run on Windows x64: mise run vehicle-arrival-test
"""
import ctypes as C
import hashlib
import pathlib
import struct
import sys
import unittest

import capstone

import builds
from pe import Image
from rtti import Rtti

# Vfunc 48 spans three contiguous .pdata entries. Its fixed instruction
# sequence is verified with only relative call/data displacements normalized.
METHOD_BYTES = 0x14a
METHOD_SHA = "f25608247dcd3554d6c0a064f11b6331d0869b28725313ed05a12fe2c72907fa"


def method(img):
    rtti = Rtti(img)
    cols = next(cols for name, _, cols in rtti.find("BaseTechChassisFacet")
                if name == ".?AVBaseTechChassisFacet@Leonardo@@")
    tables = [vt for col, vts in cols if img.u32(col + 4) == 0 for vt in vts]
    if len(tables) != 1:
        raise ValueError(f"expected one primary chassis vtable, found {tables}")
    rva = img.u64(tables[0] + 0x180) - img.base
    for cls in ("CarChassisFacet", "TankChassisFacet"):
        cols = next(cols for name, _, cols in rtti.find(cls)
                    if name == f".?AV{cls}@Leonardo@@")
        primary = [vt for col, vts in cols if img.u32(col + 4) == 0 for vt in vts]
        if len(primary) != 1 or img.u64(primary[0] + 0x180) != img.base + rva:
            raise ValueError(f"{cls} does not share the vehicle arrival callback")
    code = img.read(rva, METHOD_BYTES)
    normalized = bytearray(code)
    relocations = []
    for ins in img.disasm(rva, rva + METHOD_BYTES):
        if (ins.mnemonic == "call"
                and ins.operands[0].type == capstone.x86.X86_OP_IMM):
            offset, size = ins.imm_offset, ins.imm_size
            target = "direction"
        elif any(op.type == capstone.x86.X86_OP_MEM
                 and op.mem.base == capstone.x86.X86_REG_RIP for op in ins.operands):
            offset, size = ins.disp_offset, ins.disp_size
            target = "one"
            native_constant = ins.address + ins.size + ins.disp
            if img.read(native_constant, 4) != struct.pack("<f", 1.0):
                raise ValueError("the speed-ratio cap is not 1.0")
        else:
            continue
        if size != 4:
            raise ValueError("unexpected relative operand size")
        at = ins.address - rva + offset
        normalized[at:at + size] = bytes(size)
        relocations.append((at, ins.address - rva + ins.size, target))
    if hashlib.sha256(normalized).hexdigest() != METHOD_SHA:
        raise ValueError("vehicle arrival callback differs from the characterized algorithm")
    if [r[2] for r in relocations] != ["direction", "direction", "one"]:
        raise ValueError("unexpected vehicle arrival dependencies")
    return rva, code, relocations


PLUGIN_PATH = pathlib.Path(sys.argv.pop(1)).resolve() if len(sys.argv) > 1 else None


class Plugin:
    def __init__(self, path):
        self.dll = C.CDLL(str(path))
        self.bind = self.dll.vehicle_arrival_test_bind
        self.bind.argtypes = [C.c_void_p, C.c_int64]
        self.bind.restype = C.c_int
        self.run = self.dll.vehicle_arrival_update
        self.run.argtypes = [C.c_size_t, C.c_size_t]
        self.run.restype = None
        self.resolve = self.dll.vehicle_arrival_test_resolve
        self.resolve.argtypes = [C.c_char_p, C.c_size_t, C.c_size_t]
        self.resolve.restype = C.c_int64


class Native:
    def __init__(self, img):
        self.rva, code, relocations = method(img)
        self.kernel = C.WinDLL("kernel32", use_last_error=True)
        self.kernel.VirtualAlloc.argtypes = [C.c_void_p, C.c_size_t, C.c_ulong, C.c_ulong]
        self.kernel.VirtualAlloc.restype = C.c_void_p
        self.kernel.VirtualFree.argtypes = [C.c_void_p, C.c_size_t, C.c_ulong]
        self.kernel.VirtualFree.restype = C.c_int
        page = bytearray(code)
        direction = len(page)
        page += b"\xc3"  # Leave the fabricated normalized direction unchanged.
        one = len(page)
        page += struct.pack("<f", 1.0)
        speed = len(page)
        # Return 10.0f in XMM0 from the fake chassis max-speed vfunc.
        page += bytes.fromhex("b800002041660f6ec0c3")
        for at, end, target in relocations:
            destination = direction if target == "direction" else one
            struct.pack_into("<i", page, at, destination - end)
        self.page = self.kernel.VirtualAlloc(None, len(page), 0x3000, 0x40)
        if not self.page:
            raise C.WinError(C.get_last_error())
        C.memmove(self.page, bytes(page), len(page))
        self.vtable = C.create_string_buffer(0x60)
        C.c_uint64.from_buffer(self.vtable, 0x58).value = self.page + speed
        self.run = C.CFUNCTYPE(None, C.c_void_p, C.c_void_p)(self.page)

    def close(self):
        if not self.kernel.VirtualFree(self.page, 0, 0x8000):
            raise C.WinError(C.get_last_error())

    def sample(self, distance, radius=20.0, mode=1, endpoint=True, count=1, helper=0,
               plugin=None, percent=50, snapshot=False):
        chassis = C.create_string_buffer(0x480)
        record = C.create_string_buffer(0x2f0)
        C.c_uint64.from_buffer(chassis).value = C.addressof(self.vtable)
        C.c_int.from_buffer(chassis, 0x320).value = mode
        for offset, value in ((0x1d0, -17.0), (0x1d4, -19.0),
                              (0x200, 0.6), (0x204, 0.0), (0x208, 0.8),
                              (0x234, radius), (0x260, distance)):
            C.c_float.from_buffer(record, offset).value = value
        C.c_int.from_buffer(record, 0x2c8).value = count
        C.c_ubyte.from_buffer(record, 0x2a8).value = 2 if endpoint else 0
        C.c_ubyte.from_buffer(record, 0x244).value = helper
        if plugin is None:
            self.run(C.addressof(chassis), C.addressof(record))
        else:
            if plugin.bind(self.page, percent) != 0:
                raise ValueError("plugin rejected the fixture configuration")
            plugin.run(C.addressof(chassis), C.addressof(record))
        outputs = tuple(C.c_float.from_buffer(record, offset).value
                        for offset in (0x1d4, 0x1d0, 0x200, 0x204, 0x208))
        # Vtable addresses vary per fixture, so omit that pointer from snapshots.
        return (outputs, bytes(record), bytes(chassis)[8:]) if snapshot else outputs


class VehicleArrivalTests(unittest.TestCase):
    def test_supported_native_arrival_ramps(self):
        for build in builds.supported():
            with self.subTest(build=build.name):
                build.require()
                img = Image(build.logic)
                native = Native(img)
                try:
                    # The ramp is linear and saturates at max speed. Halving
                    # its radius should raise near-end speed while keeping
                    # the endpoint stopped and the cruise-speed cap intact.
                    for radius in (20.0, 10.0):
                        for helper in (0, 1):
                            for distance in (0.0, 0.1, 2.5, 5.0, 10.0, 15.0, 20.0, 40.0):
                                ratio = min(distance / radius, 1.0)
                                actual = native.sample(distance, radius, helper=helper)
                                expected = (ratio, 10 * ratio, 6 * ratio, 0.0, 8 * ratio)
                                for got, want in zip(actual, expected):
                                    self.assertAlmostEqual(got, want, places=5)
                    # Intermediate corners without the endpoint flag keep
                    # cruise speed. A path with no points requests a stop.
                    self.assertEqual(native.sample(1, endpoint=False), (1, 10, 6, 0, 8))
                    self.assertEqual(native.sample(1, count=0), (0, 0, 0, 0, 0))
                    # Other movement modes do not apply the arrival ratio.
                    self.assertAlmostEqual(native.sample(1, mode=0)[2], 0.6)
                    self.assertEqual(native.sample(1, mode=0)[:2], (-19, -17))
                    self.assertEqual(native.sample(1, mode=3)[:2], (-19, 10))
                    print(f"{build.name}: logic+{native.rva:#x}; stock and half-radius ramps pass")
                finally:
                    native.close()

    def test_compiled_detour_only_tightens_final_destination_ramps(self):
        if PLUGIN_PATH is None:
            self.fail("pass the parity-test DLL path, or run mise run vehicle-arrival-test")
        plugin = Plugin(PLUGIN_PATH)
        self.assertNotEqual(plugin.bind(None, 50), 0)
        for percent in (49, 101):
            self.assertNotEqual(plugin.bind(1, percent), 0)
        for build in builds.supported():
            with self.subTest(build=build.name):
                img = Image(build.require().logic)
                # The plugin's resolver and this characterization must agree on
                # the callback, and a change inside its ramp must be refused.
                rva, body, _ = method(img)
                image = bytearray(img.pe.get_memory_mapped_image())
                self.assertEqual(plugin.resolve(bytes(image), len(image), img.base), rva)
                image[rva + 0xe5] ^= 1
                self.assertEqual(plugin.resolve(bytes(image), len(image), img.base), -1)
                native = Native(img)
                try:
                    for percent in (50, 75, 100):
                        for helper in (0, 1):
                            for distance in (0, 0.1, 2.5, 5, 10, 15, 20, 40):
                                args = dict(distance=distance, helper=helper, snapshot=True)
                                stock, stock_record, stock_chassis = native.sample(**args)
                                patched, patched_record, patched_chassis = native.sample(
                                    **args, plugin=plugin, percent=percent)
                                ratio = min(distance / (20 * percent / 100), 1)
                                for got, want in zip(patched, (ratio, 10*ratio, 6*ratio, 0, 8*ratio)):
                                    self.assertAlmostEqual(got, want, places=5)
                                self.assertLessEqual(patched[1], 10)
                                self.assertGreaterEqual(patched[1], stock[1])
                                self.assertEqual(patched_chassis, stock_chassis)
                                ignored = {i for offset in (0x1d0, 0x1d4, 0x200, 0x204, 0x208)
                                           for i in range(offset, offset+4)}
                                self.assertEqual(bytes(v for i,v in enumerate(patched_record) if i not in ignored),
                                                 bytes(v for i,v in enumerate(stock_record) if i not in ignored))
                    # These branches and unusual radii must remain bit-for-bit native.
                    for options in (dict(endpoint=False), dict(count=0), dict(mode=0), dict(mode=2),
                                    dict(mode=3), dict(radius=0), dict(radius=float("nan")),
                                    dict(radius=-20), dict(radius=float("inf"))):
                        for distance in (0, 1):
                            stock = native.sample(distance, snapshot=True, **options)
                            patched = native.sample(distance, plugin=plugin, snapshot=True, **options)
                            self.assertEqual(patched[1:], stock[1:])
                    print(f"{build.name}: compiled detour passes; radius and unrelated fields preserved")
                finally:
                    native.close()


if __name__ == "__main__":
    unittest.main()
