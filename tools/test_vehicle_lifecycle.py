"""Execute passenger binding cleanup without restoring ghost ammunition."""
import ctypes
import pathlib
import struct
import unittest

import build as b
from test_firing import asm, obj, put, q, region


def ammo(gun):
    return ctypes.c_uint32.from_address(gun + 0xdc).value


def set_ammo(gun, value):
    put(gun + 0xdc, struct.pack("<I", value))


class LifecycleTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        code, labels = b.assemble(
            pathlib.Path("patch/vehicle-priority-fire.asm").read_text().splitlines(),
            0xb000, b.CURSOR_OFFSET,
            symbols={"vehicle_special_fire_source": 0x9000,
                     "vehicle_special_fire_board_resume": 0xa900,
                     "vehicle_special_fire_disembark_resume": 0xa940,
                     "vehicle_special_fire_refresh_ai": 0xa800})
        put(region + 0xb000, code)
        cls.cleanup = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)(
            region + labels["vehicle_special_fire_cleanup"])
        put(region + 0xa700, asm("mov rax, qword ptr [rcx + 8]; ret"))
        cls.entity_vt = obj(0xb8, [(0xb0, region + 0xa700)])

    def setUp(self):
        self.events = []

        @ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)
        def unbind(dummy, source):
            self.events.append(("unbind", dummy, source))
            if source:
                set_ammo(source, ammo(dummy))
            set_ammo(dummy, 0)

        @ctypes.CFUNCTYPE(None, ctypes.c_void_p)
        def refresh(car_ai):
            self.events.append(("refresh", car_ai))

        self.callbacks = unbind, refresh
        self.vt = obj(0x68, [(0x60, ctypes.cast(unbind, ctypes.c_void_p).value)])
        put(region + 0xa800, asm(
            f"mov rax, {ctypes.cast(refresh, ctypes.c_void_p).value}; jmp rax"))
        self.primary, self.special = self.gun(0), self.gun(6)
        self.roster([self.primary, self.special])

    def gun(self, kind, rounds=0):
        descriptor = obj(0x110)
        put(descriptor + 0x108, struct.pack("<I", kind))
        gun = obj(0x150, [(0, self.vt), (0x40, descriptor)])
        set_ammo(gun, rounds)
        return gun

    def roster(self, guns):
        vector = obj(max(8, 8 * len(guns)), [(i * 8, gun) for i, gun in enumerate(guns)])
        human = obj(0x50, [(0x38, vector), (0x40, vector + 8 * len(guns))])
        ai = obj(0x200, [(0x1f0, obj(0x18, [(0x10, human)]))])
        facet = obj(0x30, [(0x28, ai)])
        passenger = obj(0x10, [(0, self.entity_vt), (8, facet)])
        passengers = obj(8, [(0, passenger)])
        self.helper = obj(0x120, [(0x110, passengers), (0x118, passengers + 8)])

    def vehicle(self, groups):
        self.sources, self.dummies, gunners = [], [], []
        for bindings in groups:
            sources = obj(len(bindings) * 8, [(i * 8, source) for i, (source, _) in enumerate(bindings)])
            dummies = [self.gun(6, rounds) for _, rounds in bindings]
            mounts = obj(len(bindings) * 8, [(i * 8, dummy) for i, dummy in enumerate(dummies)])
            gunners.append(obj(0x78, [(0x20, mounts), (0x28, mounts + len(bindings) * 8),
                                      (0x68, sources), (0x70, sources + len(bindings) * 8)]))
            self.sources.append(sources)
            self.dummies.append(dummies)
        vector = obj(8 * len(gunners), [(i * 8, gunner) for i, gunner in enumerate(gunners)])
        self.car_ai = obj(0x220, [(0x208, vector), (0x210, vector + 8 * len(gunners))])
        return gunners

    def test_ghost_does_not_overwrite_ammunition_used_on_foot(self):
        ghost = self.gun(6, 2)
        self.vehicle([[(ghost, 9)]])
        self.cleanup(self.helper, self.car_ai)
        self.assertEqual(q(self.sources[0]), 0)
        self.assertEqual(ammo(ghost), 2)
        self.assertEqual(self.events, [("unbind", self.dummies[0][0], None),
                                      ("refresh", self.car_ai)])

    def test_former_source_is_compared_without_dereferencing_it(self):
        self.vehicle([[(0xdeadbeef, 9)]])
        self.cleanup(self.helper, self.car_ai)
        self.assertEqual(q(self.sources[0]), 0)

    def test_duplicate_keeps_first_copy_without_merging_ammunition(self):
        self.vehicle([[(self.special, 0), (self.special, 9)]])
        self.cleanup(self.helper, self.car_ai)
        self.assertEqual((q(self.sources[0]), q(self.sources[0] + 8)), (self.special, 0))
        self.assertEqual(ammo(self.dummies[0][0]), 0)
        self.assertEqual(ammo(self.special), 0)
        self.assertEqual(self.events[0], ("unbind", self.dummies[0][1], None))

    def test_duplicates_across_gunners_are_removed(self):
        self.vehicle([[(self.special, 3)], [(self.special, 8)]])
        self.cleanup(self.helper, self.car_ai)
        self.assertEqual((q(self.sources[0]), q(self.sources[1])), (self.special, 0))
        self.assertEqual(ammo(self.dummies[0][0]), 3)

    def test_primary_stays_valid_when_passenger_also_has_a_special(self):
        self.vehicle([[(self.primary, 23)]])
        self.cleanup(self.helper, self.car_ai)
        self.assertEqual(q(self.sources[0]), self.primary)
        self.assertEqual(ammo(self.dummies[0][0]), 23)
        self.assertEqual(self.events, [])

    def test_empty_roster_discards_all_bindings_and_refreshes_once(self):
        self.vehicle([[(self.primary, 23), (self.special, 3)]])
        put(self.helper + 0x118, struct.pack("<Q", q(self.helper + 0x110)))
        self.cleanup(self.helper, self.car_ai)
        self.assertEqual((q(self.sources[0]), q(self.sources[0] + 8)), (0, 0))
        self.assertEqual(sum(event[0] == "refresh" for event in self.events), 1)

    def test_inconsistent_mount_vectors_are_left_for_native_handling(self):
        gunners = self.vehicle([[(self.special, 3)]])
        put(gunners[0] + 0x28, struct.pack("<Q", q(gunners[0] + 0x20)))
        self.cleanup(self.helper, self.car_ai)
        self.assertEqual(q(self.sources[0]), self.special)
        self.assertEqual(self.events, [])

    def test_extra_native_mount_vector_is_not_misinterpreted(self):
        gunners = self.vehicle([[(self.special, 3)]])
        put(gunners[0] + 0x58, struct.pack("<Q", 8))
        self.cleanup(self.helper, self.car_ai)
        self.assertEqual(q(self.sources[0]), self.special)
        self.assertEqual(self.events, [])

    def test_malformed_earlier_gunner_does_not_displace_valid_binding(self):
        gunners = self.vehicle([[(self.special, 8)], [(self.special, 3)]])
        put(gunners[0] + 0x28, struct.pack("<Q", q(gunners[0] + 0x20)))
        self.cleanup(self.helper, self.car_ai)
        self.assertEqual((q(self.sources[0]), q(self.sources[1])),
                         (self.special, self.special))
        self.assertEqual(self.events, [])

    def test_oversized_earlier_source_vector_is_not_a_canonical_binding(self):
        gunners = self.vehicle([[(self.special, 8)], [(self.special, 3)]])
        put(gunners[0] + 0x70, struct.pack("<Q", q(gunners[0] + 0x68) + 520))
        self.cleanup(self.helper, self.car_ai)
        self.assertEqual(q(self.sources[1]), self.special)
        self.assertEqual(self.events, [])


if __name__ == "__main__":
    unittest.main()
