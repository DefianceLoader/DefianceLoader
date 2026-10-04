from __future__ import annotations

import struct
import tempfile
import unittest
import zipfile
from pathlib import Path

import package_moving_actions_animation as package
from compose_infantry_animation import Clip, Track, encode_clip


def _model_fixture() -> bytes:
    nodes = [
        ("Bip01", -1), ("Bip01_Pelvis", 0), ("Bip01_Spine", 1),
        ("Bip01_L_Thigh", 2), ("Bip01_L_Calf", 3), ("Bip01_L_Foot", 4),
        ("Bip01_L_Toe0", 5), ("Bip01_R_Thigh", 2), ("Bip01_R_Calf", 7),
        ("Bip01_R_Foot", 8), ("Bip01_R_Toe0", 9),
    ]
    data = bytearray(struct.pack("<4sI4sI", b"MODL", 7, b"NODS", len(nodes)))
    for name, parent in nodes:
        data.extend(name.encode() + b"\0")
        data.extend(struct.pack("<i16fB", parent, *([0.0] * 16), 0))
        data.extend(b"\0")
    return bytes(data)


def _clip_fixture(duration: float, offset: float) -> bytes:
    names = ["Bip01", "Bip01_Pelvis", "Bip01_Spine",
             "Bip01_L_Thigh", "Bip01_L_Calf", "Bip01_L_Foot", "Bip01_L_Toe0",
             "Bip01_R_Thigh", "Bip01_R_Calf", "Bip01_R_Foot", "Bip01_R_Toe0"]
    tracks = tuple(Track(
        name, name.encode(),
        ((0.0, offset, 0.0, 0.0), (duration, offset + 0.1, 0.0, 0.0)),
        ((0.0, 1.0, 0.0, 0.0, 0.0),
         (duration, 0.9238795, 0.0, 0.0, 0.3826834)),
    ) for name in names)
    return encode_clip(Clip(duration, tracks))


def fixture_resources() -> dict[str, bytes]:
    """Small, valid source members for packaging/stage tests; no game files."""
    return {
        package.SOURCES["throw"]: _clip_fixture(3.5666668, 0.0),
        package.SOURCES["switch"]: _clip_fixture(1.6666667, 0.2),
        package.SOURCES["run"]: _clip_fixture(1.0666667, 0.4),
        package.SOURCES["back"]: _clip_fixture(1.4, 0.8),
        package.SOURCES["model"]: _model_fixture(),
    }


class MovingActionsAnimationPackageTests(unittest.TestCase):
    def test_synthetic_paks_make_private_assets_and_provenance(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            game = Path(temporary)
            mapping = fixture_resources()
            with zipfile.ZipFile(game / "basis.pak", "w") as archive:
                for member, data in mapping.items():
                    archive.writestr(member, data)
            entries, sources = package.mod_entries(game)
            prefix = f"mods/{package.MOD_DIR}/"
            self.assertTrue(all(prefix + "assets/" + name in entries for name in package.ASSET_NAMES))
            self.assertIn(prefix + "sources.json", entries)
            self.assertEqual(sources["recipe"], package.RECIPE)
            self.assertEqual(len(sources["sources"]), 5)
            self.assertNotIn("basis/animations/new/anim/st_throw_grenade_m16.anim",
                             "\n".join(entries))

    def test_missing_source_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            game = Path(temporary)
            with zipfile.ZipFile(game / "basis.pak", "w"):
                pass
            with self.assertRaisesRegex(ValueError, "required resources"):
                package.mod_entries(game)


if __name__ == "__main__":
    unittest.main()
