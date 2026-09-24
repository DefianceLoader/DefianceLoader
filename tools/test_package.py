"""Package contents and opt-in behavior, using fixture DLLs and the real config tool."""
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
import zipfile

import package
import package_squad_scroll
from test_package_squad_scroll import fixture as panel_fixture

ROOT = pathlib.Path(__file__).resolve().parent.parent


def config_tool():
    for candidate in (ROOT / "target/release/defiance-config.exe",
                      ROOT / "target/debug/defiance-config.exe"):
        if candidate.is_file():
            return candidate
    raise SystemExit("defiance-config.exe is missing; run mise run loader first")


class PackageTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = pathlib.Path(temp.name)
        self.source = self.root / "build"
        self.source.mkdir()
        self.proxy = self.source / "defiance_loader.dll"
        self.proxy.write_bytes(b"defiance-loader proxy")
        (self.source / "defiance_plugin_feature_selection.dll").write_bytes(b"defiance.selection")
        self.sidecar = self.source / "defiance_plugin_feature_selection.plugin.json"
        self.sidecar.write_text('{"id": "defiance.selection"}')
        (self.source / "defiance_plugin_pickup.dll").write_bytes(b"obsolete pilot")
        # An obsolete copy in the main build directory must not shadow the
        # current plugin from its independent workspace or produce duplicate ZIP entries.
        (self.source / "defiance_plugin_regroup.dll").write_bytes(b"stale regroup")
        self.regroup = self.root / "defiance_plugin_regroup.dll"
        self.regroup.write_bytes(b"current regroup")
        self.expanded = self.root / package.EXPANDED_AMMO_DLL
        self.expanded.write_bytes(b"current expanded ammo")
        self.scroll = self.root / package.SQUAD_SCROLL_DLL
        self.scroll.write_bytes(b"current squad scroll")
        self.helper = self.source / "defiance-crash-helper.exe"
        self.helper.write_bytes(b"defiance-loader crash helper")
        for binary in [self.proxy, self.helper, self.source / "defiance_plugin_feature_selection.dll",
                       self.regroup, self.expanded, self.scroll]:
            binary.with_name(binary.stem.replace("-", "_") + ".pdb").write_bytes(b"fixture symbols")
        self.out = self.root / "defiance-loader.zip"

    def build(self, game=None):
        environment = os.environ.copy()
        environment.pop("DEFIANCE_GAME_DIR", None)
        command = [sys.executable, str(ROOT / "tools/package.py"),
                   "--source", str(self.source), "--out", str(self.out),
                   "--regroup-dll", str(self.regroup),
                   "--expanded-ammo-dll", str(self.expanded),
                   "--squad-scroll-dll", str(self.scroll)]
        if game is not None:
            command += ["--game", str(game)]
        return subprocess.run(command, capture_output=True, text=True, env=environment)

    def test_package_includes_instructions_and_regroup_disabled_by_default(self):
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        with zipfile.ZipFile(self.out) as archive:
            names = archive.namelist()
            self.assertEqual(len(names), len(set(names)))
            self.assertIn(f"bin/{package.stage.proxy_name()}", names)
            self.assertIn("bin/defiance-crash-helper.exe", names)
            self.assertFalse(any(name.endswith(".pdb") for name in names))
            self.assertIn("DefianceLoader/plugins/defiance_plugin_feature_selection.dll", names)
            self.assertIn("DefianceLoader/plugins/defiance_plugin_feature_selection.plugin.json", names)
            self.assertNotIn("DefianceLoader/plugins/defiance_plugin_pickup.dll", names)
            self.assertFalse(any(name.lower().endswith(".ini") for name in names))
            # One player guide; the developer and plugin docs stay in the repository.
            self.assertEqual([name for name in names if name.lower().endswith(".md")], ["README.md"])
            readme = archive.read("README.md").decode("utf-8")
            self.assertIn("keep existing `.ini`", readme)
            self.assertIn("disabled by default", readme)
            self.assertEqual(archive.read("DefianceLoader/plugins/defiance_plugin_regroup.dll"), b"current regroup")
            self.assertEqual(archive.read("DefianceLoader/plugins/" + package.EXPANDED_AMMO_DLL),
                             b"current expanded ammo")
            self.assertEqual(archive.read("DefianceLoader/plugins/" + package.SQUAD_SCROLL_DLL),
                             b"current squad scroll")
            for name in (package.EXPANDED_AMMO_DLL, package.SQUAD_SCROLL_DLL):
                manifest = json.loads(archive.read("DefianceLoader/plugins/" + pathlib.Path(name).stem + ".plugin.json"))
                self.assertEqual(manifest["dll"], name)
            manifest = json.loads(archive.read("DefianceLoader/plugins/defiance_plugin_regroup.plugin.json"))
            self.assertEqual(next(s["default"] for s in manifest["settings"] if s["key"] == "enabled"), "false")

    def test_companion_ui_mod_is_derived_from_the_game(self):
        game = self.root / "game"
        game.mkdir()
        with zipfile.ZipFile(game / "basis.pak", "w") as archive:
            archive.writestr(package_squad_scroll.RESOURCE, panel_fixture())
            archive.writestr(package_squad_scroll.VEHICLE_RESOURCE, panel_fixture(vehicle=True))
        result = self.build(game=game)
        self.assertEqual(result.returncode, 0, result.stderr)
        with zipfile.ZipFile(self.out) as archive:
            names = archive.namelist()
            self.assertIn("mods/defiance_squad_scroll/mod.json", names)
            self.assertIn("mods/defiance_squad_scroll/sources.json", names)
            self.assertIn("mods/defiance_squad_scroll/basis/" + package_squad_scroll.RESOURCE, names)
            self.assertIn("mods/defiance_squad_scroll/basis/" + package_squad_scroll.VEHICLE_RESOURCE, names)
            self.assertTrue(any(name.startswith("mods/defiance_squad_scroll/basis/textures/") for name in names))

    def test_without_a_game_dir_packages_without_the_companion_mod(self):
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        with zipfile.ZipFile(self.out) as archive:
            self.assertFalse(any(name.startswith("mods/") for name in archive.namelist()))

    def test_real_config_reader_honors_default_and_preserves_upgrade_opt_in(self):
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        game = self.root / "game"
        with zipfile.ZipFile(self.out) as archive:
            archive.extractall(game)
        config = game / "DefianceLoader/config/infantry.ini"
        self.assertFalse(config.exists())
        for contents, expected in [
            (None, "false"),
            ("; existing install without regroup\n[defiance.selection]\nenabled=true\n", "false"),
            ("; user opted in\n[defiance.regroup]\nenabled=true\n", "true"),
        ]:
            with self.subTest(expected=expected, contents=contents):
                if contents is not None:
                    config.parent.mkdir(parents=True, exist_ok=True)
                    config.write_text(contents, encoding="utf-8")
                before = config.read_bytes() if config.exists() else None
                report = subprocess.run([str(config_tool()), "--game", str(game / "bin"), "--report"],
                                        capture_output=True, text=True)
                self.assertEqual(report.returncode, 0, report.stderr)
                self.assertIn(f"defiance.regroup.enabled = {expected}", report.stdout)
                self.assertEqual(config.read_bytes() if config.exists() else None, before)

    def test_missing_required_files_refuse_an_incomplete_package(self):
        for missing in (self.proxy, self.sidecar, self.regroup, self.expanded, self.scroll,
                        self.helper, self.proxy.with_suffix(".pdb")):
            with self.subTest(missing=missing.name):
                original = missing.read_bytes()
                missing.unlink()
                result = self.build()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("missing package input", result.stderr)
                self.assertFalse(self.out.exists())
                missing.write_bytes(original)

    def test_symbol_archive_identifies_exact_binaries(self):
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        with zipfile.ZipFile(self.out.with_name("defiance-loader-symbols.zip")) as archive:
            builds = json.loads(archive.read("builds.json"))
            loader = next(row for row in builds if row["binary"] == self.proxy.name)
            self.assertEqual(loader["sha256"], hashlib.sha256(self.proxy.read_bytes()).hexdigest())
            self.assertEqual(archive.read(loader["pdb"]), b"fixture symbols")
            self.assertTrue(any(row["binary"] == package.SQUAD_SCROLL_DLL for row in builds))


if __name__ == "__main__":
    unittest.main()
