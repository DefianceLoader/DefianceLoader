"""Tests for the game-independent injector metadata and runtime checks."""
import json
import pathlib
import subprocess
import sys
import tempfile
import types
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import check_injector


class FakeBuild:
    def __init__(self, root, name, layout):
        self.name = name
        self.layout = layout
        self.folder = root / "bin" / name
        self.logic = self.folder / "logic.dll"
        self.game = self.folder / "game.dll"


class CheckInjectorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temp.name)
        self.variants = self.root / "variants"
        self.variants.mkdir()
        self.hashes = {"logic.dll": "logic-hash", "game.dll": "game-hash"}
        self.reference = FakeBuild(self.root, "reference", None)
        self.modern = FakeBuild(self.root, "gog-2099-01-01", self.root / "layouts" / "gog-2099-01-01.json")
        self.modern.layout.parent.mkdir()
        self.modern.layout.write_text(json.dumps({"logic_sha256": "new-logic", "game_sha256": "new-game"}), encoding="utf-8")
        self.table = types.SimpleNamespace(
            supported=lambda: [self.reference, self.modern],
            build=lambda name: {self.reference.name: self.reference, self.modern.name: self.modern}[name],
            on_disk=lambda: [],
        )
        self._write_variant("reference", self.hashes)
        self._write_variant(self.modern.name, {"logic.dll": "new-logic", "game.dll": "new-game"})

    def tearDown(self):
        self.temp.cleanup()

    def _write_variant(self, name, hashes):
        units = self.variants / name / "units"
        units.mkdir(parents=True)
        for unit, module, plugin in (("core-logic", "logic.dll", "core"),
                                     ("core-game", "game.dll", "core"),
                                     ("selection-game", "game.dll", "selection")):
            descriptor = {
                "name": unit, "module": module, "plugin": plugin,
                "source_sha256": hashes[module], "unit_bytes": 3,
            }
            (units / f"{unit}.json").write_text(json.dumps(descriptor), encoding="utf-8")
            (units / f"{unit}.bin").write_bytes(b"abc")

    def test_metadata_accepts_reference_and_every_supported_layout(self):
        self.assertEqual(check_injector.check_metadata(self.table, self.variants), [self.modern.name])

    def test_metadata_rejects_missing_supported_variant_folder(self):
        (self.variants / self.modern.name).rename(self.variants / "unexpected-build")
        with self.assertRaisesRegex(ValueError, "folders differ from supported layouts"):
            check_injector.check_metadata(self.table, self.variants)

    def test_metadata_rejects_missing_feature_unit(self):
        (self.variants / self.modern.name / "units" / "selection-game.json").unlink()
        with self.assertRaisesRegex(ValueError, "unit names, modules, or plugins differ"):
            check_injector.check_metadata(self.table, self.variants)

    def test_metadata_rejects_stale_source_hash(self):
        path = self.variants / self.modern.name / "units" / "core-game.json"
        item = json.loads(path.read_text(encoding="utf-8"))
        item["source_sha256"] = "stale"
        path.write_text(json.dumps(item), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "does not match layout game.dll hash"):
            check_injector.check_metadata(self.table, self.variants)

    def test_metadata_rejects_wrong_binary_length(self):
        (self.variants / "reference" / "units" / "core-logic.bin").write_bytes(b"short")
        with self.assertRaisesRegex(ValueError, "descriptor says 3"):
            check_injector.check_metadata(self.table, self.variants)

    def test_metadata_rejects_changed_plugin_coverage(self):
        path = self.variants / self.modern.name / "units" / "selection-game.json"
        item = json.loads(path.read_text(encoding="utf-8"))
        item["plugin"] = "other"
        path.write_text(json.dumps(item), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "modules, or plugins differ"):
            check_injector.check_metadata(self.table, self.variants)

    def test_full_check_discovers_new_supported_build_and_propagates_scan_failure(self):
        self.modern.folder.mkdir(parents=True)
        self.modern.logic.write_bytes(b"logic")
        self.modern.game.write_bytes(b"game")
        self.reference.folder.mkdir(parents=True)
        self.reference.logic.write_bytes(b"logic")
        self.reference.game.write_bytes(b"game")
        exe = self.root / "injector.exe"
        exe.touch()
        calls = []

        def run(args, check):
            calls.append(args)
            if args[1] == "--scan-check" and args[-1] == str(self.modern.folder):
                raise subprocess.CalledProcessError(9, args)

        with mock.patch.object(check_injector.subprocess, "run", side_effect=run):
            with self.assertRaises(subprocess.CalledProcessError):
                check_injector.check_full(exe, self.table, variants_dir=self.variants)
        self.assertEqual(calls[0], [str(exe), "--self-test"])
        self.assertEqual([call[1:] for call in calls[1:]], [
            ["--scan-check", str(self.reference.folder)],
            ["--scan-check", str(self.modern.folder)],
        ])

    def test_self_test_failure_stops_before_scanning(self):
        exe = self.root / "injector.exe"
        exe.touch()
        with mock.patch.object(check_injector.subprocess, "run", side_effect=subprocess.CalledProcessError(5, [str(exe), "--self-test"])) as run:
            with self.assertRaises(subprocess.CalledProcessError):
                check_injector.check_full(exe, self.table, variants_dir=self.variants)
        run.assert_called_once_with([str(exe), "--self-test"], check=True)

    def test_full_check_finds_the_installed_pair_under_bin(self):
        install = self.root / "installed-game"
        (install / "bin").mkdir(parents=True)
        for module in ("logic.dll", "game.dll"):
            (install / "bin" / module).write_bytes(b"DLL")
        exe = self.root / "injector.exe"
        exe.touch()
        with mock.patch.object(check_injector.subprocess, "run") as run:
            check_injector.check_full(exe, self.table, game_dir=install, variants_dir=self.variants)
        self.assertEqual(run.call_args_list, [
            mock.call([str(exe), "--self-test"], check=True),
            mock.call([str(exe), "--scan-check", str(install / "bin")], check=True),
        ])

    def test_full_check_rejects_a_complete_unknown_local_pair(self):
        self.table.on_disk = lambda: [FakeBuild(self.root, "steam-2099-01-01", None)]
        unknown = self.table.on_disk()[0]
        unknown.folder.mkdir(parents=True)
        unknown.logic.write_bytes(b"logic")
        unknown.game.write_bytes(b"game")
        exe = self.root / "injector.exe"
        exe.touch()
        with mock.patch.object(check_injector.subprocess, "run"):
            with self.assertRaisesRegex(ValueError, "no supported profile"):
                check_injector.check_full(exe, self.table, variants_dir=self.variants)


if __name__ == "__main__":
    unittest.main()
