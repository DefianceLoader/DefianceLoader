"""Offline native checks for the ad-hoc squad investigation.

Runs the unmodified SquadAiFacet gunner-removal routine against fabricated
objects. This verifies a building block, not live transfers, spawning or saves.
No running game is opened or modified. Windows x64 only.
"""
import ctypes as C
import hashlib
import pathlib
import unittest

import build as b
from test_selection import Native
from pe import Image


class GunnerRemovalTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        source = pathlib.Path(b.SRC).read_bytes()
        if hashlib.sha256(source).hexdigest() != b.EXPECT_SOURCE_SHA:
            raise RuntimeError("unsupported source DLL")
        cls.native = Native()
        image = b.Image(b.SRC)
        # A complete leaf function: no external calls or RIP-relative data.
        cls.remove = C.CFUNCTYPE(None, C.c_void_p, C.c_void_p)(
            cls.native.code(image.read(0x43dc70, 0x4c)))

    @classmethod
    def tearDownClass(cls):
        cls.native.close()

    def fixture(self, count=3):
        n = self.native
        # Opaque HumanGunner pointers, deliberately distinct from entity IDs.
        gunners = [n.data(0x20, [(8, i + 10)]) for i in range(count)]
        storage = n.data(
            max(8, count * 8), [(i * 8, p) for i, p in enumerate(gunners)])
        ai = n.data(0x300, [(0x1e8, storage), (0x1f0, storage + count * 8),
                             (0x1f8, storage + count * 8), (0x298, 0x12345678)])
        return ai, storage, gunners

    @staticmethod
    def qword(address):
        return C.c_uint64.from_address(address).value

    def roster(self, ai):
        begin, end = self.qword(ai + 0x1e8), self.qword(ai + 0x1f0)
        return [self.qword(p) for p in range(begin, end, 8)]

    def test_middle_removal_moves_last_pointer_without_touching_gunners(self):
        ai, storage, gunners = self.fixture()
        before = [C.string_at(p, 0x20) for p in gunners]
        self.remove(ai, gunners[1])
        self.assertEqual(self.roster(ai), [gunners[0], gunners[2]])
        self.assertEqual(self.qword(ai + 0x1e8), storage)
        self.assertEqual(self.qword(ai + 0x1f8), storage + 24)
        self.assertEqual(self.qword(ai + 0x298), 0x12345678)
        self.assertEqual([C.string_at(p, 0x20) for p in gunners], before)

    def test_last_pointer_removal(self):
        ai, _, gunners = self.fixture()
        self.remove(ai, gunners[-1])
        self.assertEqual(self.roster(ai), gunners[:-1])

    def test_only_pointer_removal_leaves_empty_vector(self):
        ai, storage, gunners = self.fixture(1)
        self.remove(ai, gunners[0])
        self.assertEqual(self.roster(ai), [])
        self.assertEqual(self.qword(ai + 0x1f0), storage)

    def test_missing_pointer_is_noop(self):
        ai, storage, gunners = self.fixture()
        before = C.string_at(ai, 0x300), C.string_at(storage, 24)
        self.remove(ai, self.native.data(0x20))
        self.assertEqual(self.roster(ai), gunners)
        self.assertEqual((C.string_at(ai, 0x300), C.string_at(storage, 24)), before)

    def test_empty_roster_is_noop(self):
        ai, _, _ = self.fixture(0)
        before = C.string_at(ai, 0x300)
        self.remove(ai, self.native.data(0x20))
        self.assertEqual(C.string_at(ai, 0x300), before)


class KeyboardStateTests(unittest.TestCase):
    def test_both_builds_update_the_modifier_offsets_used_by_regroup(self):
        reference = Image("bin/game.orig.dll")
        # Actual native bitset-to-modifier-byte code. Preserve RBX around this
        # extracted straight-line fragment; it has no calls or relative data.
        fragment = reference.read(0x2da319, 0x2da34f - 0x2da319)
        for path in ["bin/gog/game.dll", "bin/steam/game.dll", "bin/gog/game-updated.dll",
                     "bin/steam/game-updated.dll"]:
            with self.subTest(build=path):
                image = Image(path)
                self.assertEqual(image.data.count(fragment), 1)
                native = Native()
                try:
                    update = C.CFUNCTYPE(None, C.c_void_p)(
                        native.code(b"\x53\x48\x89\xcb" + fragment + b"\x5b\xc3"))
                    obj = native.data(0x890)
                    for ctrl, alt in [(False, False), (True, False),
                                      (True, True), (False, True), (False, False)]:
                        C.c_uint32.from_address(obj + 0x38).value = (
                            (int(ctrl) << 17) | (int(alt) << 18))
                        update(obj)
                        self.assertEqual(C.c_ubyte.from_address(obj + 0x880).value, ctrl)
                        self.assertEqual(C.c_ubyte.from_address(obj + 0x881).value, alt)
                finally:
                    native.close()


class PerkRosterCopyTests(unittest.TestCase):
    def test_native_copy_overwrites_twenty_slot_frame_at_member_twenty_two(self):
        # Execute the actual roster-length and memcpy-argument instructions,
        # but in an oversized test frame. The game's saved return-address slot
        # becomes an observable canary rather than a real return address.
        crt = C.CDLL("msvcrt")
        memcpy = C.cast(crt.memcpy, C.c_void_p).value
        for path, start in [("bin/gog/logic.dll", 0x33151d),
                            ("bin/steam/logic.dll", 0x3315ad),
                            ("bin/gog/logic-updated.dll", 0x3401bd),
                            ("bin/steam/logic-updated.dll", 0x34024d)]:
            with self.subTest(build=path):
                im = Image(path)
                length = im.read(start, 14)
                setup = im.read(start + 0x13, 13)
                self.assertEqual([i.mnemonic for i in im.md.disasm(length, start)],
                                 ["mov", "mov", "sub", "sar"])
                self.assertEqual([i.mnemonic for i in im.md.disasm(setup, start + 0x13)],
                                 ["lea", "lea"])
                n = Native()
                try:
                    prefix, _ = b.assemble([
                        "push rdi", "sub rsp, 0x300", "mov rax, rcx",
                        "mov r10, 0x12345678", "mov qword ptr [rsp+0xc8], r10",
                    ], 0, 0)
                    suffix, _ = b.assemble([
                        f"mov rax, {memcpy}", "call rax",
                        "mov rax, qword ptr [rsp+0xc8]",
                        "add rsp, 0x300", "pop rdi", "ret",
                    ], 0, 0)
                    copy = C.CFUNCTYPE(C.c_uint64, C.c_void_p)(
                        n.code(prefix + length + setup + suffix))
                    for count in [16, 20, 21, 22, 23, 25, 64]:
                        data = n.data(count * 8, [(i*8, 0x1000+i) for i in range(count)])
                        vector = n.data(24, [(0, data), (8, data+count*8)])
                        self.assertEqual(copy(vector), 0x1015 if count >= 22 else 0x12345678)
                finally:
                    n.close()



class UiRosterExportTests(unittest.TestCase):
    def test_stock_export_zeroes_members_after_twenty_in_both_builds(self):
        # Execute the actual export and resize instructions. Provide spare
        # physical capacity so the bad copy can be measured without corrupting
        # the test process. The vector's logical size is still only 20 at copy.
        import struct
        crt = C.CDLL("msvcrt")
        memset = C.cast(crt.memset, C.c_void_p).value
        for path, export_at, resize_at in [
            ("bin/gog/logic.dll", 0x110240, 0xcf4a0),
            ("bin/steam/logic.dll", 0x1102d0, 0xcf530),
            ("bin/gog/logic-updated.dll", 0x118360, 0xd7540),
            ("bin/steam/logic-updated.dll", 0x1183f0, 0xd75d0),
        ]:
            with self.subTest(build=path):
                im = Image(path)
                n = Native()
                try:
                    copy_sizes = []
                    @C.CFUNCTYPE(C.c_size_t, C.c_void_p, C.c_void_p)
                    def copy_roster(vector, dst):
                        begin = C.c_size_t.from_address(vector).value
                        end = C.c_size_t.from_address(vector + 8).value
                        count = (end - begin) // 8
                        copy_sizes.append((count, (C.c_size_t.from_address(out + 8).value - dst) // 8))
                        C.memmove(dst, begin, count * 8)
                        return count

                    # Keep both functions and their absolute target thunks in
                    # one block. All external relative branches are relocated.
                    raw = bytearray(im.read(export_at, 0x3d))
                    raw.extend(b"\xcc" * (0x100 - len(raw)))
                    raw.extend(im.read(resize_at, 0x8f))
                    raw.extend(b"\xcc" * (0x200 - len(raw)))
                    for target in [C.cast(copy_roster, C.c_void_p).value, memset]:
                        raw.extend(b"\x48\xb8" + struct.pack("<Q", target) + b"\xff\xe0")
                    for offset, opcode, destination in [
                        (0x18, 0xe8, 0x100), (0x23, 0xe8, 0x200),
                        (0x38, 0xe9, 0x100), (0x172, 0xe8, 0x20c),
                    ]:
                        self.assertEqual(raw[offset], opcode)
                        struct.pack_into("<i", raw, offset + 1, destination - offset - 5)
                    # Reallocation is unreachable with our 64-entry capacity;
                    # replace its external tail jump with an observable trap.
                    self.assertEqual(raw[0x155], 0xe9)
                    raw[0x155:0x15a] = b"\xcc" * 5
                    export = C.CFUNCTYPE(None, C.c_void_p, C.c_void_p, C.c_void_p)(n.code(bytes(raw)))
                    for count in [0, 16, 20, 21, 22, 23, 25, 64]:
                        values = [0x1000 + i for i in range(count)]
                        src = n.data(max(8, count * 8), [(i*8, v) for i, v in enumerate(values)])
                        roster = n.data(24, [(0, src), (8, src + count*8)])
                        data = n.data(65*8, [(64*8, 0xdeadbeef)])
                        out = n.data(24, [(0, data), (8, data), (16, data + 64*8)])
                        export(0, roster, out)
                        actual = [C.c_size_t.from_address(data + i*8).value for i in range(count)]
                        self.assertEqual(actual, values[:20] + [0]*max(0, count-20))
                        self.assertEqual(C.c_size_t.from_address(out+8).value, data+count*8)
                        self.assertEqual(C.c_size_t.from_address(data+64*8).value, 0xdeadbeef)
                        self.assertEqual(copy_sizes[-1], (count, 20))
                finally:
                    n.close()

if __name__ == "__main__":
    unittest.main()
