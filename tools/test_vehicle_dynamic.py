"""Execute dynamic passenger swaps with live-ammo and aiming stand-ins."""
import ctypes
import pathlib
import struct
import sys
import unittest

sys.path.insert(0, "tools")
import build as b
from test_firing import asm, obj, put, q, region


def u32(at):
    return ctypes.c_uint32.from_address(at).value


def set32(at, value):
    put(at, struct.pack("<I", value))


class DynamicMountTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        code, cls.labels = b.assemble(
            pathlib.Path("patch/vehicle-dynamic-fire.asm").read_text().splitlines(),
            0xb000, b.CURSOR_OFFSET,
            symbols={"vehicle_special_fire_dynamic_resume": 0xc000,
                     "vehicle_special_fire_guard_skip": 0xc040,
                     "vehicle_special_fire_dynamic_client_resume": 0xc200,
                     "vehicle_special_fire_dynamic_client_skip": 0xc240})
        put(region + 0xb000, code)
        cls.run_swap = ctypes.CFUNCTYPE(None, ctypes.c_void_p)(
            region + cls.labels["vehicle_special_fire_dynamic_rebalance"])
        cls.run_client = ctypes.CFUNCTYPE(ctypes.c_ubyte, ctypes.c_void_p, ctypes.c_uint64)(
            region + cls.labels["vehicle_special_fire_dynamic_client_swap"])
        put(region + 0xc000, asm(
            "add dword ptr [rbx + 0x15c], 100; "
            "mov eax, dword ptr [rbx + 0xbc]; add rsp, 0x20; pop rbx; ret"))
        put(region + 0xc040, asm(
            "mov eax, dword ptr [rbx + 0xbc]; add rsp, 0x20; pop rbx; ret"))
        thunk = asm(
            "push rbx; sub rsp, 0x20; mov rbx, rcx; "
            f"mov rax, {region + cls.labels['vehicle_special_fire_dynamic_tick']}; jmp rax")
        put(region + 0xc100, thunk)
        cls.run_tick = ctypes.CFUNCTYPE(ctypes.c_uint32, ctypes.c_void_p)(region + 0xc100)
        client_epilogue = "add rsp, 0x20; pop r15; pop rsi; pop rbx; ret"
        put(region + 0xc200, asm(f"xor eax, eax; {client_epilogue}"))
        put(region + 0xc240, asm(f"mov eax, 1; {client_epilogue}"))
        put(region + 0xc300, asm(
            "push rbx; push rsi; push r15; sub rsp, 0x20; mov rsi, rcx; mov r15, rdx; "
            f"mov rax, {region + cls.labels['vehicle_special_fire_dynamic_client']}; jmp rax"))
        cls.run_client_hook = ctypes.CFUNCTYPE(ctypes.c_ubyte, ctypes.c_void_p, ctypes.c_void_p)(
            region + 0xc300)

    def setUp(self):
        self.events = []
        self.target_refreshes = []
        self.allowed = {0: {0, 1}, 6: {1}}
        self.target = obj(0x20, [(8, 2), (0x10, obj(8)), (0x18, 1)])

        @ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)
        def set_target(dummy, target_copy):
            target = q(target_copy)
            self.assertEqual(target, q(dummy + 0xc8))
            self.target_refreshes.append(dummy)
            set32(dummy + 0x84, u32(q(dummy + 0x40) + 0x108))
            set32(dummy + 0x140, 1)
            put(dummy + 0x145, bytes(1))
            if target:
                set32(target + 8, u32(target + 8) - 1)
                if q(target + 0x10):
                    set32(target + 0x18, u32(target + 0x18) - 1)

        @ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)
        def unbind(dummy, source):
            self.events.append(("unbind", dummy, source))
            set32(source + 0xdc, u32(dummy + 0xdc))
            set32(dummy + 0xdc, 0)
            put(dummy + 0x40, bytes(8))

        @ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)
        def bind(dummy, source):
            self.events.append(("bind", dummy, source))
            put(dummy + 0x40, struct.pack("<Q", q(source + 0x40)))
            set32(dummy + 0xdc, u32(source + 0xdc))
            set32(source + 0xdc, 0)

        @ctypes.CFUNCTYPE(ctypes.c_ubyte, ctypes.c_void_p)
        def can_aim(dummy):
            desc = q(dummy + 0x40)
            return int(bool(desc and u32(dummy + 0xdc) and
                            ctypes.c_ubyte.from_address(dummy + 0xe2).value and
                            u32(dummy + 0x84) == u32(desc + 0x108) and
                            u32(dummy + 0x18) in self.allowed.get(u32(desc + 0x108), set())))

        @ctypes.CFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p)
        def aim_info(dummy):
            return dummy + 0x84

        self.callbacks = [unbind, bind, can_aim, aim_info, set_target]
        self.vt = obj(0x1c8, [(0x30, ctypes.cast(set_target, ctypes.c_void_p).value),
                              (0x60, ctypes.cast(unbind, ctypes.c_void_p).value),
                              (0x58, ctypes.cast(bind, ctypes.c_void_p).value),
                              (0x70, ctypes.cast(aim_info, ctypes.c_void_p).value),
                              (0x1c0, ctypes.cast(can_aim, ctypes.c_void_p).value)])

        def source(kind):
            desc = obj(0x110)
            set32(desc + 0x108, kind)
            return obj(0xe0, [(0, self.vt), (0x40, desc)])

        self.special = source(6)
        self.usual = source(0)
        self.left = obj(0x150, [(0, self.vt), (0x40, q(self.special + 0x40)),
                                (0xc8, self.target)])
        self.right = obj(0x150, [(0, self.vt), (0x18, 1),
                                 (0x40, q(self.usual + 0x40)), (0xc8, self.target)])
        for dummy, seed in ((self.left, 17), (self.right, 31)):
            put(dummy + 0xd4, bytes(range(seed, seed + 44)))
            put(dummy + 0xe2, b"\x01")
            put(dummy + 0x12d, bytes(range(seed, seed + 3)))
            put(dummy + 0x145, bytes([seed]))
            set32(dummy + 0x140, seed)
            for offset in (0x20, 0x50, 0x58, 0x110):
                handle = obj(0x20, [(8, 2), (0x18, 1)])
                put(dummy + offset, struct.pack("<Q", handle))
            put(dummy + 0x118, struct.pack("<Q", seed))
        set32(self.left + 0xdc, 7)
        set32(self.right + 0xdc, 23)
        set32(self.left + 0x84, 6)
        self.mounts = obj(16, [(0, self.left), (8, self.right)])
        self.sources = obj(16, [(0, self.special), (8, self.usual)])
        self.gunner = obj(0x160, [(0x20, self.mounts), (0x28, self.mounts + 16),
                                  (0x68, self.sources), (0x70, self.sources + 16),
                                  (0x98, self.target)])

    def test_swaps_and_preserves_each_weapons_ammunition(self):
        self.run_swap(self.gunner)
        self.assertEqual((q(self.sources), q(self.sources + 8)), (self.usual, self.special))
        self.assertEqual((u32(self.left + 0xdc), u32(self.right + 0xdc)), (23, 7))
        self.assertEqual((u32(self.special + 0xdc), u32(self.usual + 0xdc)), (0, 0))
        self.assertEqual(self.events, [])
        self.assertEqual(self.target_refreshes, [self.left, self.right])
        self.assertEqual((u32(self.left + 0x84), u32(self.right + 0x84)), (0, 6))
        self.assertEqual((u32(self.target + 8), u32(self.target + 0x18)), (2, 1))
        self.assertEqual(u32(self.gunner + 0x13c) & 0x1000, 0x1000)
        self.assertEqual((u32(self.gunner + 0x158), u32(self.gunner + 0x15c)), (1, 0))
        self.events.clear()
        self.run_swap(self.gunner)
        self.assertEqual(self.events, [])

    def test_new_target_direction_moves_special_back(self):
        self.run_swap(self.gunner)
        self.allowed[6] = {0}
        self.events.clear()
        self.run_swap(self.gunner)
        self.assertEqual((q(self.sources), q(self.sources + 8)), (self.special, self.usual))
        self.assertEqual((u32(self.left + 0xdc), u32(self.right + 0xdc)), (7, 23))

    def test_failed_destination_restores_both_bindings_and_ammunition(self):
        self.allowed[6] = set()
        self.run_swap(self.gunner)
        self.assertEqual((q(self.sources), q(self.sources + 8)), (self.special, self.usual))
        self.assertEqual((u32(self.left + 0xdc), u32(self.right + 0xdc)), (7, 23))
        self.assertEqual(self.events, [])
        self.assertEqual(u32(self.gunner + 0x13c), 0)
        self.assertEqual((u32(self.left + 0x140), u32(self.right + 0x140)), (17, 31))
        self.assertEqual((u32(self.left + 0x84), u32(self.right + 0x84)), (6, 0))
        self.assertEqual(self.target_refreshes, [self.left, self.right] * 2)
        self.assertEqual((u32(self.target + 8), u32(self.target + 0x18)), (2, 1))

    def test_only_one_exchange_is_attempted_per_update(self):
        self.allowed[6] = {2}
        self.allowed[0] = {0, 1, 2}
        usual = obj(0xe0, [(0, self.vt), (0x40, q(self.usual + 0x40))])
        third = obj(0x150, [(0, self.vt), (0x18, 2), (0x40, q(usual + 0x40)),
                            (0xc8, self.target)])
        put(third + 0xe2, b"\x01")
        set32(third + 0xdc, 11)
        mounts = obj(24, [(0, self.left), (8, self.right), (16, third)])
        sources = obj(24, [(0, self.special), (8, self.usual), (16, usual)])
        for offset, value in ((0x20, mounts), (0x28, mounts + 24),
                              (0x68, sources), (0x70, sources + 24)):
            put(self.gunner + offset, struct.pack("<Q", value))
        self.run_swap(self.gunner)
        self.assertEqual((q(sources), q(sources + 8), q(sources + 16)),
                         (self.special, self.usual, usual))
        self.assertEqual(u32(third + 0xdc), 11)

    def test_live_reload_state_and_owning_handles_follow_each_weapon(self):
        def snapshot(dummy):
            return [ctypes.string_at(dummy + offset, size) for offset, size in
                    ((0x20, 8), (0x40, 8), (0x50, 8), (0x58, 8), (0xd4, 44),
                     (0x110, 16), (0x12d, 3))]
        before = snapshot(self.left), snapshot(self.right)
        handles = [q(dummy + offset) for dummy in (self.left, self.right)
                   for offset in (0x20, 0x50, 0x58, 0x110)]
        self.run_swap(self.gunner)
        self.assertEqual((snapshot(self.left), snapshot(self.right)), before[::-1])
        self.assertTrue(all(q(handle + 8) == 2 and q(handle + 0x18) == 1 for handle in handles))
        self.assertEqual((q(self.left + 0xc8), q(self.right + 0xc8)), (self.target, self.target))
        self.assertEqual((u32(self.left + 0x18), u32(self.right + 0x18)), (0, 1))
        self.assertEqual((ctypes.c_ubyte.from_address(self.left + 0x145).value,
                          ctypes.c_ubyte.from_address(self.right + 0x145).value), (0, 0))

    def test_productive_special_is_not_moved(self):
        self.allowed[6] = {0, 1}
        self.run_swap(self.gunner)
        self.assertEqual(self.events, [])

    def test_another_special_is_not_displaced(self):
        set32(q(self.usual + 0x40) + 0x108, 6)
        self.run_swap(self.gunner)
        self.assertEqual(self.events, [])

    def test_empty_or_former_sources_are_not_dereferenced(self):
        for slot in (self.sources, self.sources + 8):
            saved = q(slot)
            for invalid in (0, obj(8, [(0, obj(8))])):
                put(slot, struct.pack("<Q", invalid))
                self.run_swap(self.gunner)
                self.assertEqual(self.events, [])
            put(slot, struct.pack("<Q", saved))

    def test_mismatched_targets_and_descriptors_are_not_exchanged(self):
        for field in (0xc8, 0x40):
            saved = q(self.right + field)
            put(self.right + field, struct.pack("<Q", obj(0x110)))
            self.run_swap(self.gunner)
            self.assertEqual(self.events, [])
            put(self.right + field, struct.pack("<Q", saved))

    def test_unloaded_special_does_not_disturb_reload(self):
        set32(self.left + 0xdc, 0)
        self.run_swap(self.gunner)
        self.assertEqual(self.events, [])
        self.assertEqual(q(self.sources), self.special)

    def test_disabled_special_is_not_moved(self):
        put(self.left + 0xe2, b"\x00")
        self.run_swap(self.gunner)
        self.assertEqual(q(self.sources), self.special)

    def test_client_event_swaps_occupied_slots_without_losing_a_source(self):
        client = obj(0x70, [(0x18, self.mounts), (0x20, self.mounts + 16),
                            (0x60, self.sources), (0x68, self.sources + 16)])
        self.assertEqual(self.run_client(client, 1), 1)
        self.assertEqual((q(self.sources), q(self.sources + 8)), (self.usual, self.special))
        self.assertEqual((u32(self.left + 0xdc), u32(self.right + 0xdc)), (23, 7))
        self.assertEqual(self.run_client(client, 1 << 32), 1)
        self.assertEqual((q(self.sources), q(self.sources + 8)), (self.special, self.usual))
        self.assertEqual((u32(self.left + 0xdc), u32(self.right + 0xdc)), (7, 23))

    def test_client_keeps_native_empty_destination_move(self):
        client = obj(0x70, [(0x18, self.mounts), (0x20, self.mounts + 16),
                            (0x60, self.sources), (0x68, self.sources + 16)])
        put(self.sources + 8, bytes(8))
        self.assertEqual(self.run_client(client, 1), 0)
        self.assertEqual(q(self.sources), self.special)

    def test_client_hook_selects_swap_and_stock_continuations(self):
        client = obj(0x70, [(0x18, self.mounts), (0x20, self.mounts + 16),
                            (0x60, self.sources), (0x68, self.sources + 16)])
        packet = obj(0x68, [(0x60, 1)])
        self.assertEqual(self.run_client_hook(client, packet), 1)
        self.assertEqual(q(self.sources + 8), self.special)
        put(packet + 0x60, struct.pack("<Q", 1 << 32))
        put(self.sources, bytes(8))
        self.assertEqual(self.run_client_hook(client, packet), 0)

    def test_client_ignores_invalid_indices_and_former_sources(self):
        client = obj(0x70, [(0x18, self.mounts), (0x20, self.mounts + 16),
                            (0x60, self.sources), (0x68, self.sources + 16)])
        for indices in (2, (2 << 32) | 1, 0xffffffff):
            self.assertEqual(self.run_client(client, indices), 1)
            self.assertEqual(q(self.sources), self.special)
        put(self.sources, struct.pack("<Q", obj(8, [(0, obj(8))])))
        self.assertEqual(self.run_client(client, 1), 1)
        self.assertEqual(q(self.sources + 8), self.usual)

    def test_mismatched_mount_and_source_vectors_are_rejected(self):
        put(self.gunner + 0x70, struct.pack("<Q", self.sources + 8))
        self.run_swap(self.gunner)
        self.assertEqual(self.events, [])

    def test_hook_replays_timer_reset_and_returns_to_native_tick(self):
        self.assertEqual(self.run_tick(self.gunner), 0x3f800000)
        self.assertEqual(u32(self.gunner + 0xbc), 0x3f800000)
        self.assertEqual(q(self.sources + 8), self.special)
        self.assertEqual(u32(self.gunner + 0x15c), 0)

    def test_failed_swap_continues_the_native_move_loop(self):
        self.allowed[6] = set()
        self.assertEqual(self.run_tick(self.gunner), 0x3f800000)
        self.assertEqual(u32(self.gunner + 0x15c), 100)
        self.assertEqual(q(self.sources), self.special)


if __name__ == "__main__":
    unittest.main()
