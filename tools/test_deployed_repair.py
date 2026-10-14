"""Execute deployed-repair native windows from supported game.dll and logic.dll files.

Fixtures copy native state-query, position-selection, and repair windows and
redirect their hooks through the compiled plugin. Native branches and the
existing menu operations execute. No installed game files are modified.
"""
import ctypes as C
import pathlib
import struct
import sys
import unittest

import builds
from pe import Image

WINDOW = bytes.fromhex(
    "498b4500ba1a000000498bcdff507084c0753d"
    "498b4500ba19000000498bcdff507084c0752a"
    "498b4500ba1c000000498bcdff507084c07517"
    "498b4500ba18000000498bcdff507084c00f842affffff"
    "81e7ba65f7ffe91fffffff"
)
MENU_RVAS = (0x23f9e4, 0x2442d4, 0x240d04, 0x245624, 0x240d04, 0x245624,
             0x24dfc4, 0x2528b4)
POSITION_RVAS = (0xe833e, 0xe83ce, 0xef5ce, 0xef65e,
                 0xef5ce, 0xef65e, 0xef5ce, 0xef65e)
PLUGIN_PATH = pathlib.Path(sys.argv.pop(1)).resolve() if len(sys.argv) > 1 else None


def helicopter_query_slot(build):
    return 0x2e0 if build.name.endswith("2025-12-23") else 0x2f0


def position_vector_offset(build):
    return 0x970 if build.name.endswith("2025-12-23") else 0x988


class Plugin:
    def __init__(self, path):
        self.dll = C.CDLL(str(path))
        self.resolve = self.dll.deployed_repair_test_resolve
        self.resolve.argtypes = [C.c_void_p, C.c_size_t, C.c_size_t,
                                 C.c_void_p, C.c_size_t, C.c_size_t, C.c_void_p]
        self.resolve.restype = C.c_int
        self.detour = self.dll.deployed_repair_test_menu_detour
        self.detour.argtypes = [C.c_size_t]
        self.detour.restype = C.c_size_t
        self.helicopter_detour = self.dll.deployed_repair_test_helicopter_detour
        self.helicopter_detour.argtypes = [C.c_size_t, C.c_size_t]
        self.helicopter_detour.restype = C.c_size_t
        self.position_detour = self.dll.deployed_repair_test_position_detour
        self.position_detour.argtypes = [C.c_size_t, C.c_size_t]
        self.position_detour.restype = C.c_size_t

    def sites(self, game, logic):
        out = (C.c_size_t * 3)()
        game_bytes = game.pe.get_memory_mapped_image()
        logic_bytes = logic.pe.get_memory_mapped_image()
        rc = self.resolve(game_bytes, len(game_bytes), game.base,
                          logic_bytes, len(logic_bytes), logic.base, out)
        if rc:
            raise ValueError("the compiled plugin refused a reference build")
        return tuple(out)


class Native:
    """Copy real instructions, with the same R13/EDI and stack contract."""
    def __init__(self, image, plugin=None):
        offset = image.data.find(WINDOW)
        if offset < 0 or image.data.find(WINDOW, offset + 1) >= 0:
            raise ValueError("expected one complete deployment restriction window")
        self.mask = image.file_to_rva(offset) + 0x50
        code = image.read(self.mask - 0x50, len(WINDOW))
        self.calls = []
        callback_type = C.CFUNCTYPE(C.c_bool, C.c_size_t, C.c_uint32)

        def state(ai, requested):
            self.calls.append(requested)
            return requested == C.c_uint32.from_address(ai + 8).value

        self.state = callback_type(state)
        self.table = C.create_string_buffer(0x78)
        C.c_size_t.from_buffer(self.table, 0x70).value = C.cast(self.state, C.c_void_p).value
        self.ai = C.create_string_buffer(16)
        C.c_size_t.from_buffer(self.ai).value = C.addressof(self.table)
        self.kernel = C.WinDLL("kernel32", use_last_error=True)
        self.kernel.VirtualAlloc.argtypes = [C.c_void_p, C.c_size_t, C.c_ulong, C.c_ulong]
        self.kernel.VirtualAlloc.restype = C.c_void_p
        self.kernel.VirtualFree.argtypes = [C.c_void_p, C.c_size_t, C.c_ulong]
        self.kernel.VirtualFree.restype = C.c_int
        # Native backward branches land 0x86 bytes before the copied window.
        window = 0x86
        entry = 0x20
        page = bytearray(b"\xcc" * (window + len(code)))
        page[:11] = bytes.fromhex("89f8488d6424285f415dc3")
        setup = bytes.fromhex("4155574883ec284989cd89d7")
        page[entry:entry + len(setup)] = setup
        jump = entry + len(setup)
        page[jump:jump + 5] = b"\xe9" + struct.pack("<i", window - jump - 5)
        page[window:window + len(code)] = code
        relay = len(page)
        page += bytes(14)
        self.page = self.kernel.VirtualAlloc(None, len(page), 0x3000, 0x40)
        if not self.page:
            raise C.WinError(C.get_last_error())
        if plugin:
            hook = plugin.detour(self.page + window + 0x50 + 6)
            page[relay:relay + 14] = bytes.fromhex("ff2500000000") + struct.pack("<Q", hook)
            patch = window + 0x50
            page[patch:patch + 6] = b"\xe9" + struct.pack("<i", relay - patch - 5) + b"\x90"
        C.memmove(self.page, bytes(page), len(page))
        self.run = C.CFUNCTYPE(C.c_uint32, C.c_size_t, C.c_uint32)(self.page + entry)

    def sample(self, state, permissions):
        self.calls.clear()
        C.c_uint32.from_buffer(self.ai, 8).value = state
        return self.run(C.addressof(self.ai), permissions)

    def close(self):
        if not self.kernel.VirtualFree(self.page, 0, 0x8000):
            raise C.WinError(C.get_last_error())


class HelicopterNative:
    """Execute the stock repair window with its native helicopter query detoured."""
    def __init__(self, image, site, query_slot, plugin=None):
        self.site = site
        self.query_slot = query_slot
        self.code_start = site - 0x5f
        self.code = image.read(self.code_start, 0x65 + 0x88)
        query_preimage = b"\xff\x90" + query_slot.to_bytes(4, "little")
        if image.read(site, len(query_preimage)) != query_preimage:
            raise ValueError("unexpected helicopter menu preimage")
        self.calls = []
        self.callbacks = []

        def method(restype, argtypes, callback):
            function = C.WINFUNCTYPE(restype, *argtypes)(callback)
            self.callbacks.append(function)
            return C.cast(function, C.c_void_p).value

        # The first entity getter supplies the damage interface in its +0x18 field.
        self.damage = C.create_string_buffer(0x100)
        self.damage_vtable = C.create_string_buffer(0x100)
        C.c_size_t.from_buffer(self.damage).value = C.addressof(self.damage_vtable)
        C.c_size_t.from_buffer(self.damage, 0x78).value = 1000
        C.c_size_t.from_buffer(self.damage, 0x80).value = 8000
        self.spares = C.create_string_buffer(8)
        self.damage_entity = C.create_string_buffer(0x40)
        C.c_size_t.from_buffer(self.damage_entity, 0x18).value = C.addressof(self.damage)
        self.entity_vtable = C.create_string_buffer(0x200)
        self.entity = C.create_string_buffer(0x80)
        C.c_size_t.from_buffer(self.entity).value = C.addressof(self.entity_vtable)
        self.menu = C.create_string_buffer(0xe30)
        self.utility = C.create_string_buffer(0x20)
        self.utility_vtable = C.create_string_buffer(0x300)
        C.c_size_t.from_buffer(self.utility).value = C.addressof(self.utility_vtable)
        C.c_size_t.from_buffer(self.menu, 0xe10).value = C.addressof(self.utility)
        self.frame = C.create_string_buffer(0x80)
        C.c_size_t.from_buffer(self.frame, 0x67).value = C.addressof(self.menu)
        self.crew_vtable = C.create_string_buffer(0x200)
        self.crew = C.create_string_buffer(0x20)
        C.c_size_t.from_buffer(self.crew).value = C.addressof(self.crew_vtable)
        self.ai_vtable = C.create_string_buffer(0x80)
        self.ai = C.create_string_buffer(0x80)
        C.c_size_t.from_buffer(self.ai).value = C.addressof(self.ai_vtable)
        self.candidate = C.create_string_buffer(0x8b0)
        C.c_size_t.from_buffer(self.entity_vtable, 0xb0).value = method(
            C.c_void_p, [C.c_void_p], lambda _this: C.addressof(self.damage_entity))
        C.c_size_t.from_buffer(self.entity_vtable, 0x50).value = method(
            C.c_void_p, [C.c_void_p], lambda _this: C.addressof(self.candidate))
        C.c_size_t.from_buffer(self.damage_vtable, 0x80).value = method(
            C.c_uint32, [C.c_void_p], lambda _this: self.maximum)
        C.c_size_t.from_buffer(self.damage_vtable, 0x78).value = method(
            C.c_uint32, [C.c_void_p], lambda _this: self.current)
        C.c_size_t.from_buffer(self.damage_vtable, 0xd0).value = method(
            C.c_void_p, [C.c_void_p], lambda _this: C.addressof(self.spares)
            if self.resources else 0)
        is_helicopter = method(C.c_bool, [C.c_void_p, C.c_void_p],
                               lambda _this, _entity: self.helicopter)
        C.c_size_t.from_buffer(self.utility_vtable, query_slot).value = is_helicopter
        C.c_size_t.from_buffer(self.crew_vtable, 0x1f8).value = method(
            C.c_bool, [C.c_void_p], lambda _this: self.crew_eligible)
        C.c_size_t.from_buffer(self.ai_vtable, 0x68).value = method(
            C.c_bool, [C.c_void_p, C.c_uint32], self.top_state)

        self.kernel = C.WinDLL("kernel32", use_last_error=True)
        self.kernel.VirtualAlloc.argtypes = [C.c_void_p, C.c_size_t, C.c_ulong, C.c_ulong]
        self.kernel.VirtualAlloc.restype = C.c_void_p
        self.kernel.VirtualFree.argtypes = [C.c_void_p, C.c_size_t, C.c_ulong]
        self.kernel.VirtualFree.restype = C.c_int
        window = 0x100
        entry = 0x20
        dynamiccast = window + len(self.code) + 0x40
        relay = dynamiccast + 0x10
        page = bytearray(b"\xcc" * (relay + 14))
        # Preserve the Windows nonvolatile registers used by the copied game code.
        setup = bytes.fromhex("415456555741555341564883ec304989cc4889d64c89c54d89cd31ff")
        page[entry:entry + len(setup)] = setup
        jump = entry + len(setup)
        page[jump:jump + 5] = b"\xe9" + struct.pack("<i", window - jump - 5)
        page[window:window + len(self.code)] = self.code
        # End the copied native block by returning its command mask to the fixture.
        end = window + len(self.code)
        epilogue = bytes.fromhex("89f84883c430415e5b415d5f5d5e415cc3")
        page[end:end + len(epilogue)] = epilogue
        # The dynamic cast is unrelated to this regression; force its null result,
        # which selects the native crew helper path without leaving the fixture.
        page[dynamiccast:dynamiccast + 4] = bytes.fromhex("31c0c3cc")
        call = 0x65 + 0x2d
        if self.code[call] != 0xe8:
            raise ValueError("unexpected dynamic-cast call in repair window")
        instruction = window + call
        displacement = dynamiccast - instruction - 5
        page[instruction + 1:instruction + 5] = struct.pack("<i", displacement)
        self.page = self.kernel.VirtualAlloc(None, len(page), 0x3000, 0x40)
        if not self.page:
            raise C.WinError(C.get_last_error())
        if plugin:
            hook = plugin.helicopter_detour(self.page + window + 0x65, query_slot)
            page[relay:relay + 14] = bytes.fromhex("ff2500000000") + struct.pack("<Q", hook)
            patch = window + 0x5f
            page[patch:patch + 6] = b"\xe9" + struct.pack("<i", relay - patch - 5) + b"\x90"
        C.memmove(self.page, bytes(page), len(page))
        self.run = C.WINFUNCTYPE(C.c_uint32, C.c_size_t, C.c_size_t,
                                 C.c_size_t, C.c_size_t)(self.page + entry)

    def top_state(self, _ai, requested):
        self.calls.append(requested)
        return requested == 0x13 and self.state == 0x13

    def sample(self, *, helicopter, state=0x13, current=1000, maximum=8000,
               resources=True, crew=True):
        self.helicopter = helicopter
        self.state = state
        self.current = current
        self.maximum = maximum
        self.resources = resources
        self.crew_eligible = crew
        self.calls.clear()
        return self.run(C.addressof(self.entity), C.addressof(self.crew),
                        C.addressof(self.frame), C.addressof(self.ai))

    def close(self):
        if not self.kernel.VirtualFree(self.page, 0, 0x8000):
            raise C.WinError(C.get_last_error())


class PositionNative:
    """Execute the copied native point-selection prefix through its bounds branch."""
    def __init__(self, image, site, offset, plugin):
        self.site = site
        self.offset = offset
        self.code = bytearray(image.read(site, 0xc0))
        before = (b"\x48\x8b\x8e" + struct.pack("<I", offset)
                  + b"\x48\x8b\x86" + struct.pack("<I", offset + 8))
        if bytes(self.code[:14]) != before:
            raise ValueError("unexpected Wheels point-vector preimage")

        rip_disp = struct.unpack_from("<i", self.code, 0x27)[0]
        constant_rva = site + 0x2b + rip_disp
        self.xmm7_value = struct.unpack("<f", image.read(constant_rva, 4))[0]
        self.kernel = C.WinDLL("kernel32", use_last_error=True)
        self.kernel.VirtualAlloc.argtypes = [C.c_void_p, C.c_size_t, C.c_ulong, C.c_ulong]
        self.kernel.VirtualAlloc.restype = C.c_void_p
        self.kernel.VirtualFree.argtypes = [C.c_void_p, C.c_size_t, C.c_ulong]
        self.kernel.VirtualFree.restype = C.c_int

        window = 0x100
        valid_target = window + 0xc0 + 0x20
        bounds_target = valid_target + 0x10
        finish = bounds_target + 0x10
        constant = finish + 0x60
        relay = constant + 0x10
        page = bytearray(b"\xcc" * (relay + 14))
        page[window:window + len(self.code)] = self.code
        # The copied movss uses a local copy of the build's actual SSE constant.
        struct.pack_into("<i", page, window + 0x27,
                         constant - (window + 0x2b))
        page[constant:constant + 4] = image.read(constant_rva, 4)

        def jump(at, target):
            page[at:at + 5] = b"\xe9" + struct.pack("<i", target - at - 5)

        # Stop each real native path immediately after point loads or at its
        # out-of-range target, while recording which branch the native compare chose.
        jump(window + 0x6d, valid_target)
        jump(window + 0xc0, bounds_target)
        page[valid_target:valid_target + 6] = b"\xb8\x01\x00\x00\x00\xe9"
        page[valid_target + 6:valid_target + 10] = struct.pack(
            "<i", finish - (valid_target + 10))
        page[bounds_target:bounds_target + 3] = b"\x31\xc0\xe9"
        page[bounds_target + 3:bounds_target + 7] = struct.pack(
            "<i", finish - (bounds_target + 7))

        # The wrapper sets RSI/R13, reserves native stack locals, and initializes
        # XMM7/XMM8 to sentinels before entering the copied selection code.
        entry = 0x20
        setup = bytes.fromhex(
            "535556415541564881ec00010000"
            "f30f7f7c2420f3440f7f442430"
            "4889ce4989d54d89c6488dac2400010000"
            "660fefff66450f76c0"
        )
        page[entry:entry + len(setup)] = setup
        jump(entry + len(setup), window)

        # Save branch status, selected coordinates, and initialized SIMD values,
        # then restore the caller's nonvolatile registers and XMM state.
        finish_code = bytes.fromhex(
            "418906"
            "f3410f7f7e08f3450f7f4618"
            "0f1045b7410f114628"
            "f30f6f7c2420f3440f6f442430"
            "4881c400010000415e415d5e5d5bc3"
        )
        page[finish:finish + len(finish_code)] = finish_code

        self.page = self.kernel.VirtualAlloc(None, len(page), 0x3000, 0x40)
        if not self.page:
            raise C.WinError(C.get_last_error())
        hook = plugin.position_detour(self.page + window + 14, offset)
        page[relay:relay + 14] = bytes.fromhex("ff2500000000") + struct.pack("<Q", hook)
        page[window:window + 14] = (
            b"\xe9" + struct.pack("<i", relay - window - 5) + b"\x90" * 9)
        C.memmove(self.page, bytes(page), len(page))
        self.run = C.WINFUNCTYPE(C.c_uint32, C.c_size_t, C.c_int32,
                                 C.c_void_p)(self.page + entry)

    def descriptor(self, points):
        records = C.create_string_buffer(max(0x180, len(points) * 0x180))
        for index, point in enumerate(points):
            for axis, value in enumerate(point):
                C.c_float.from_buffer(records, index * 0x180 + 0xa4 + axis * 4).value = value
        descriptor = C.create_string_buffer(self.offset + 16)
        begin = C.addressof(records)
        C.c_size_t.from_buffer(descriptor, self.offset).value = begin
        C.c_size_t.from_buffer(descriptor, self.offset + 8).value = begin + len(points) * 0x180
        return descriptor, records

    def sample(self, descriptor, index):
        output = C.create_string_buffer(56)
        result = self.run(descriptor or 0, index, C.addressof(output))
        status = C.c_uint32.from_buffer(output).value
        if status != result:
            raise ValueError("native position fixture returned a mismatched branch result")
        coordinates = struct.unpack_from("<4f", output, 40)
        xmm7 = struct.unpack_from("<4f", output, 8)
        xmm8 = struct.unpack_from("<4f", output, 24)
        return status, coordinates[:3], xmm7, xmm8

    def close(self):
        if not self.kernel.VirtualFree(self.page, 0, 0x8000):
            raise C.WinError(C.get_last_error())


class DeployedRepairTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if PLUGIN_PATH is None:
            raise ValueError("pass the parity-test plugin DLL")
        cls.plugin = Plugin(PLUGIN_PATH)

    def test_compiled_detour_changes_only_existing_repair_permission_while_deployed(self):
        for build, expected_rva in zip(builds.supported(), MENU_RVAS):
            with self.subTest(build=build.name):
                build.require()
                image = Image(build.game)
                logic = Image(build.logic)
                menu, order, stop = self.plugin.sites(image, logic)
                self.assertEqual(menu, expected_rva)
                self.assertTrue(order and stop)
                stock = Native(image)
                patched = Native(image, self.plugin)
                try:
                    self.assertEqual(stock.mask, menu)
                    for state in (0, 0x18, 0x19, 0x1a, 0x1c, 0x2e):
                        for permissions in (0, 0x8000, 0xffffffff, 0x7fff7fff):
                            before = stock.sample(state, permissions)
                            after = patched.sample(state, permissions)
                            expected = before
                            if state == 0x1a:
                                expected |= permissions & 0x8000
                            self.assertEqual(after, expected,
                                             (build.name, state, permissions))
                            self.assertEqual((after ^ before) & ~0x8000, 0)
                finally:
                    patched.close()
                    stock.close()

    def test_compiled_resolver_refuses_changed_menu_preimage(self):
        for build, expected_rva in zip(builds.supported(), MENU_RVAS):
            with self.subTest(build=build.name):
                build.require()
                game = Image(build.game)
                logic = Image(build.logic)
                mapped = bytearray(game.pe.get_memory_mapped_image())
                mapped[expected_rva + 2] ^= 1
                output = (C.c_size_t * 3)(111, 222, 333)
                logic_bytes = logic.pe.get_memory_mapped_image()
                result = self.plugin.resolve(
                    bytes(mapped), len(mapped), game.base,
                    logic_bytes, len(logic_bytes), logic.base, output)
                self.assertNotEqual(result, 0)
                self.assertEqual(tuple(output), (111, 222, 333))

    def test_helicopter_detour_gates_the_native_crew_repair_window(self):
        native_builds = {"gog-2025-12-23", "gog-2026-10-07", "steam-2026-10-07"}
        found = set()
        for build, menu_rva in zip(builds.supported(), MENU_RVAS):
            if build.name not in native_builds:
                continue
            found.add(build.name)
            with self.subTest(build=build.name):
                build.require()
                image = Image(build.game)
                query_slot = helicopter_query_slot(build)
                site = menu_rva - 0x306
                stock = HelicopterNative(image, site, query_slot)
                patched = HelicopterNative(image, site, query_slot, self.plugin)
                try:
                    # A helicopter remains blocked by stock code while landed.
                    self.assertEqual(stock.sample(helicopter=True, state=0x13) & 0x8000,
                                     0)

                    # The deployed, landed state reaches the native crew path.
                    self.assertEqual(patched.sample(helicopter=True, state=0x13) & 0x8000,
                                     0x8000)
                    self.assertEqual(patched.calls, [0x13])

                    # Ground behavior stays equivalent for every AI state.
                    for state in (0, 0x12, 0x13, 0x14, 0x15, 0x2e):
                        with self.subTest(state=state):
                            self.assertEqual(
                                stock.sample(helicopter=False, state=state),
                                patched.sample(helicopter=False, state=state))
                            self.assertEqual(patched.calls, [])

                    # Airborne, landing, takeoff, and other states remain blocked.
                    for state in (0, 0x12, 0x14, 0x15, 0x2e):
                        with self.subTest(helicopter_state=state):
                            self.assertEqual(
                                stock.sample(helicopter=True, state=state) & 0x8000,
                                patched.sample(helicopter=True, state=state) & 0x8000)
                            self.assertEqual(patched.calls, [0x13])

                    # Native prerequisites still reject every ineligible repair.
                    for case in (
                        {"current": 8000},
                        {"resources": False},
                        {"crew": False},
                    ):
                        with self.subTest(ineligible=case):
                            stock_result = stock.sample(
                                helicopter=True, state=0x13, **case) & 0x8000
                            patched_result = patched.sample(
                                helicopter=True, state=0x13, **case) & 0x8000
                            self.assertEqual(stock_result, 0)
                            self.assertEqual(patched_result, stock_result)
                finally:
                    patched.close()
                    stock.close()
        self.assertEqual(found, native_builds)

    def test_position_detour_handles_null_empty_and_indexed_vectors(self):
        native_builds = {"gog-2025-12-23", "gog-2026-10-07", "steam-2026-10-07"}
        found = set()
        for build, site in zip(builds.supported(), POSITION_RVAS):
            if build.name not in native_builds:
                continue
            found.add(build.name)
            with self.subTest(build=build.name):
                build.require()
                image = Image(build.logic)
                fixture = PositionNative(image, site, position_vector_offset(build), self.plugin)
                try:
                    def assert_simd_initialized(result):
                        _status, _coordinates, xmm7, xmm8 = result
                        self.assertAlmostEqual(xmm7[0], fixture.xmm7_value, places=6)
                        self.assertEqual(xmm7[1:], (0.0, 0.0, 0.0))
                        self.assertEqual(xmm8, (0.0, 0.0, 0.0, 0.0))

                    # Null descriptors safely choose the native bounding-box path
                    # for crew indices, including a negative signed index.
                    for index in (0, 1, 2, -1):
                        with self.subTest(null_index=index):
                            result = fixture.sample(None, index)
                            self.assertEqual(result[0], 0)
                            assert_simd_initialized(result)

                    # A valid, empty vector follows the same native bounds path.
                    empty, empty_records = fixture.descriptor([])
                    for index in (0, 1):
                        with self.subTest(empty_index=index):
                            result = fixture.sample(C.addressof(empty), index)
                            self.assertEqual(result[0], 0)
                            assert_simd_initialized(result)

                    points = ((1.25, -2.5, 3.75),
                              (10.0, 20.5, -30.25),
                              (-100.0, 0.125, 42.0))
                    descriptor, records = fixture.descriptor(points)
                    for index, expected in enumerate(points):
                        with self.subTest(point_index=index):
                            result = fixture.sample(C.addressof(descriptor), index)
                            self.assertEqual(result[0], 1)
                            for actual, coordinate in zip(result[1], expected):
                                self.assertAlmostEqual(actual, coordinate, places=6)
                            assert_simd_initialized(result)

                    # The vector end and signed index still bound native selection.
                    for index in (3, -1):
                        with self.subTest(out_of_bounds_index=index):
                            result = fixture.sample(C.addressof(descriptor), index)
                            self.assertEqual(result[0], 0)
                            assert_simd_initialized(result)
                finally:
                    fixture.close()
        self.assertEqual(found, native_builds)

    def test_compiled_resolver_refuses_changed_position_vector_preimage(self):
        for build, site in zip(builds.supported(), POSITION_RVAS):
            with self.subTest(build=build.name):
                build.require()
                game = Image(build.game)
                logic = Image(build.logic)
                offset = position_vector_offset(build)
                before = (b"\x48\x8b\x8e" + struct.pack("<I", offset)
                          + b"\x48\x8b\x86" + struct.pack("<I", offset + 8))
                mapped = bytearray(logic.pe.get_memory_mapped_image())
                self.assertEqual(bytes(mapped[site:site + len(before)]), before)
                game_bytes = game.pe.get_memory_mapped_image()
                for changed_offset in (0, 7):
                    with self.subTest(call_byte=changed_offset):
                        changed = bytearray(mapped)
                        changed[site + changed_offset] ^= 1
                        output = (C.c_size_t * 3)(111, 222, 333)
                        result = self.plugin.resolve(
                            game_bytes, len(game_bytes), game.base,
                            bytes(changed), len(changed), logic.base, output)
                        self.assertNotEqual(result, 0)
                        self.assertEqual(tuple(output), (111, 222, 333))

    def test_compiled_resolver_refuses_changed_helicopter_preimage(self):
        for build, menu_rva in zip(builds.supported(), MENU_RVAS):
            with self.subTest(build=build.name):
                build.require()
                game = Image(build.game)
                logic = Image(build.logic)
                query_slot = helicopter_query_slot(build)
                heli_site = menu_rva - 0x306
                mapped = bytearray(game.pe.get_memory_mapped_image())
                self.assertEqual(bytes(mapped[heli_site:heli_site + 6]),
                                 b"\xff\x90" + query_slot.to_bytes(4, "little"))
                logic_bytes = logic.pe.get_memory_mapped_image()
                for offset in (0, 2):
                    with self.subTest(call_byte=offset):
                        changed = bytearray(mapped)
                        changed[heli_site + offset] ^= 1
                        output = (C.c_size_t * 3)(111, 222, 333)
                        result = self.plugin.resolve(
                            bytes(changed), len(changed), game.base,
                            logic_bytes, len(logic_bytes), logic.base, output)
                        self.assertNotEqual(result, 0)
                        self.assertEqual(tuple(output), (111, 222, 333))


if __name__ == "__main__":
    unittest.main()
