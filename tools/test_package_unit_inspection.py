"""The unit-inspection reload bar mod and the BC7 decoder it reads with. The
real-texture check needs DEFIANCE_GAME_DIR and the archive password; it is
skipped without them."""
import os
import pathlib
import re
import struct
import tempfile
import unittest
import zipfile

import bc7
import package_squad_scroll
import package_unit_inspection as mod

ROOT = pathlib.Path(__file__).resolve().parent.parent


def mode6(first, second, indices, parity=(1, 1)):
    """One BC7 mode 6 block: RGBA endpoints of 7 bits, then the parity bits
    and sixteen 4-bit indices (the anchor 3 bits)."""
    value, at = 1 << 6, 7
    for channel in range(4):
        for end in (first, second):
            value |= end[channel] << at
            at += 7
    for bit in parity:
        value |= bit << at
        at += 1
    for i, index in enumerate(indices):
        value |= index << at
        at += 3 if i == 0 else 4
    assert at == 128
    return value.to_bytes(16, "little")


def bc7_dds(width, height, blocks):
    """A DX10-header DDS holding BC7 blocks, as the game ships them."""
    fourcc = struct.unpack("<I", b"DX10")[0]
    header = [124, 0x81007, height, width, len(blocks) * 16, 0, 1] + [0] * 11
    header += [32, 0x4, fourcc, 0, 0, 0, 0, 0]
    header += [0x1000, 0, 0, 0, 0]
    return (b"DDS " + struct.pack("<31I", *header) + struct.pack("<5I", 98, 3, 0, 1, 0)
            + b"".join(blocks))


def reload_bar(width=88, height=4):
    """A stock-shaped reload bar: one flat teal-to-cyan step per block column,
    alpha 0x66 (7-bit 0x33 with parity 0)."""
    blocks = []
    for by in range(height // 4):
        for bx in range(width // 4):
            colour = (0x1b + bx, 0x32 + bx * 2, 0x39 + bx * 2, 0x33)
            blocks.append(mode6(colour, colour, [0] * 16, parity=(0, 0)))
    return bc7_dds(width, height, blocks)


class Bc7Tests(unittest.TestCase):
    def test_mode6_interpolates_between_its_endpoints(self):
        indices = [0] + [15] * 15
        width, height, rows = bc7.decode(bc7_dds(4, 4, [mode6((0, 0, 0, 0), (127, 127, 127, 127), indices)]))
        self.assertEqual((width, height), (4, 4))
        self.assertEqual(rows[0][0], (1, 1, 1, 1))
        self.assertEqual(rows[3][3], (255, 255, 255, 255))

    def test_uncompressed_round_trip(self):
        rows = [[(1, 2, 3, 4), (5, 6, 7, 8)]]
        self.assertEqual(bc7.decode(bc7.encode(rows))[2], rows)

    def test_other_modes_are_refused(self):
        with self.assertRaises(ValueError):
            bc7.decode(bc7_dds(4, 4, [b"\x01" + b"\0" * 15]))


class ModTests(unittest.TestCase):
    def test_the_grey_bar_keeps_the_gradient_and_alpha(self):
        width, height, rows = bc7.decode(mod.greyscale(reload_bar()))
        self.assertEqual((width, height), mod.SIZE)
        self.assertTrue(all(p[0] == p[1] == p[2] and p[3] == 0x66 for row in rows for p in row))
        brightness = [p[0] for p in rows[0]]
        self.assertEqual(brightness, sorted(brightness))
        self.assertEqual(brightness[-1], 255)

    def test_an_unexpected_texture_is_refused(self):
        with self.assertRaisesRegex(ValueError, "not the stock"):
            mod.greyscale(reload_bar(width=64))

    def test_the_plugin_recognises_the_grey_size(self):
        source = (ROOT / "plugins/unit-inspection/src/lib.rs").read_text(encoding="utf-8")
        match = re.search(r"const GREY_BAR: \(u32, u32\) = \((\d+), (\d+)\);", source)
        self.assertIsNotNone(match)
        self.assertEqual(tuple(map(int, match.groups())), mod.SIZE)
        self.assertNotEqual(mod.SIZE, mod.STOCK_SIZE)

    def test_the_mod_tree(self):
        with tempfile.TemporaryDirectory() as temp:
            game = pathlib.Path(temp).resolve()
            with zipfile.ZipFile(game / "basis.pak", "w") as archive:
                archive.writestr(mod.RESOURCE, reload_bar())
            entries, _ = mod.mod_entries(game)
        prefix = f"mods/{mod.MOD_DIR}/"
        self.assertIn(prefix + "mod.json", entries)
        self.assertIn(prefix + "basis/" + mod.RESOURCE, entries)
        self.assertTrue(all(name.startswith(prefix) for name in entries))

    @unittest.skipUnless(os.environ.get("DEFIANCE_GAME_DIR") and os.environ.get(package_squad_scroll.PASSWORD_ENV),
                         "needs the installed game and its archive password")
    def test_the_installed_games_bar(self):
        game = pathlib.Path(os.environ["DEFIANCE_GAME_DIR"])
        game = game if (game / "basis.pak").is_file() else game.parent
        entries, _ = mod.mod_entries(game)
        data = entries[f"mods/{mod.MOD_DIR}/basis/{mod.RESOURCE}"]
        width, height, rows = bc7.decode(data)
        self.assertEqual((width, height), mod.SIZE)
        brightness = [p[0] for p in rows[0]]
        self.assertEqual(brightness, sorted(brightness))


if __name__ == "__main__":
    unittest.main()
