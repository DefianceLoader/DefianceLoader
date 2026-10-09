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
import package_unit_inspection
import package_moving_actions_animation
from test_package_moving_actions_animation import fixture_resources
from test_package_squad_scroll import AMMO_INFO, fixture as panel_fixture
from test_package_unit_inspection import reload_bar

ROOT = pathlib.Path(__file__).resolve().parent.parent
TRAINING_INFO = '\r\n'.join([
    'name\ttype\tregion\tlink\ttip\tproperty\tvalue',
    'title_text\ttext\t230,254,1691,290',
    'close_button\ttext_button\t226,250,442,294',
    'items_first_line\twidget\t1,309,1921,695',
    'items_second_line\twidget\t1,695,1921,1081',
    '']).encode()


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
        self.injector = self.source / "defiance-pickup-inject.exe"
        self.injector.write_bytes(b"defiance-pickup-inject executable")
        # The same loader DLL starts the host under a neutral filename or is
        # copied to the generated system-DLL proxy name for proxy startup.
        self.loader = self.proxy
        (self.source / "defiance_plugin_feature_selection.dll").write_bytes(b"defiance.selection")
        self.sidecar = self.source / "defiance_plugin_feature_selection.plugin.json"
        self.sidecar.write_bytes(
            (package.ROOT / "plugins/selection" / self.sidecar.name).read_bytes())
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
        self.inspection = self.root / package.UNIT_INSPECTION_DLL
        self.inspection.write_bytes(b"current unit inspection")
        self.ability_groups = self.root / package.ABILITY_GROUPS_DLL
        self.ability_groups.write_bytes(b"current ability groups")
        self.legion_hacking = self.root / package.LEGION_VEHICLE_HACKING_DLL
        self.legion_hacking.write_bytes(b"current Legion vehicle hacking")
        self.vehicle_arrival = self.root / package.VEHICLE_ARRIVAL_DLL
        self.vehicle_arrival.write_bytes(b"current vehicle arrival")
        self.cover_markers = self.root / package.COVER_MARKERS_DLL
        self.cover_markers.write_bytes(b"current cover markers")
        self.weapon_drops = self.root / package.WEAPON_DROPS_DLL
        self.weapon_drops.write_bytes(b"current weapon drops")
        self.moving = {}
        for folder, stem in package.stage.MOVING_ACTIONS:
            path = self.root / (stem + ".dll")
            path.write_bytes(("current " + stem).encode())
            path.with_suffix(".pdb").write_bytes(b"fixture moving symbols")
            self.moving[pathlib.PurePosixPath(folder).name] = path
            # A stale copy in the common output directory cannot shadow a
            # standalone build or create duplicate archive entries.
            (self.source / path.name).write_bytes(b"stale moving plugin")
        self.helper = self.source / "defiance-crash-helper.exe"
        self.helper.write_bytes(b"defiance-loader crash helper")
        for binary in [self.proxy, self.injector, self.loader, self.helper,
                       self.source / "defiance_plugin_feature_selection.dll",
                       self.regroup, self.expanded, self.scroll, self.inspection,
                       self.ability_groups, self.legion_hacking, self.cover_markers,
                       self.vehicle_arrival, self.weapon_drops]:
            binary.with_name(binary.stem.replace("-", "_") + ".pdb").write_bytes(b"fixture symbols")
        self.out = self.root / "defiance-loader.zip"

    def build(self, game=None, startup="proxy"):
        environment = os.environ.copy()
        environment.pop("DEFIANCE_GAME_DIR", None)
        command = [sys.executable, str(ROOT / "tools/package.py"),
                   "--source", str(self.source), "--out", str(self.out),
                   "--regroup-dll", str(self.regroup),
                   "--expanded-ammo-dll", str(self.expanded),
                   "--squad-scroll-dll", str(self.scroll),
                   "--unit-inspection-dll", str(self.inspection),
                   "--ability-groups-dll", str(self.ability_groups),
                   "--legion-vehicle-hacking-dll", str(self.legion_hacking),
                   "--cover-markers-dll", str(self.cover_markers),
                   "--vehicle-arrival-dll", str(self.vehicle_arrival),
                   "--weapon-drops-dll", str(self.weapon_drops)]
        for name, path in self.moving.items():
            command += ["--" + name + "-dll", str(path)]
        if startup != "proxy":
            command += ["--startup", startup]
        if game is not None:
            command += ["--game", str(game)]
        return subprocess.run(command, capture_output=True, text=True, env=environment)

    def test_package_includes_instructions_and_regroup_disabled_by_default(self):
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        with zipfile.ZipFile(self.out) as archive:
            names = archive.namelist()
            self.assertEqual(len(names), len(set(names)))
            for path in self.moving.values():
                member = "DefianceLoader/plugins/" + path.name
                self.assertIn(member, names)
                self.assertEqual(archive.read(member), path.read_bytes())
                manifest = path.name.replace(".dll", ".plugin.json")
                self.assertIn("DefianceLoader/plugins/" + manifest, names)
            self.assertIn(f"bin/{package.stage.proxy_name()}", names)
            self.assertIn("bin/defiance-crash-helper.exe", names)
            self.assertFalse(any(name.endswith(".pdb") for name in names))
            self.assertIn("DefianceLoader/plugins/defiance_plugin_feature_selection.dll", names)
            self.assertIn("DefianceLoader/plugins/defiance_plugin_feature_selection.plugin.json", names)
            self.assertIn("DefianceLoader/plugins/" + package.LEGION_VEHICLE_HACKING_DLL, names)
            self.assertIn("DefianceLoader/plugins/" + package.ABILITY_GROUPS_DLL, names)
            self.assertIn("DefianceLoader/plugins/" + package.VEHICLE_ARRIVAL_DLL, names)
            self.assertIn("DefianceLoader/plugins/" + package.COVER_MARKERS_DLL, names)
            self.assertIn("DefianceLoader/plugins/" + package.WEAPON_DROPS_DLL, names)
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
            self.assertEqual(archive.read("DefianceLoader/plugins/" + package.LEGION_VEHICLE_HACKING_DLL),
                             b"current Legion vehicle hacking")
            self.assertEqual(archive.read("DefianceLoader/plugins/" + package.COVER_MARKERS_DLL),
                             b"current cover markers")
            self.assertEqual(archive.read("DefianceLoader/plugins/" + package.ABILITY_GROUPS_DLL),
                             b"current ability groups")
            self.assertEqual(archive.read("DefianceLoader/plugins/" + package.VEHICLE_ARRIVAL_DLL),
                             b"current vehicle arrival")
            self.assertEqual(archive.read("DefianceLoader/plugins/" + package.WEAPON_DROPS_DLL),
                             b"current weapon drops")
            for name in (package.EXPANDED_AMMO_DLL, package.SQUAD_SCROLL_DLL):
                manifest = json.loads(archive.read("DefianceLoader/plugins/" + pathlib.Path(name).stem + ".plugin.json"))
                self.assertEqual(manifest["dll"], name)
            manifest = json.loads(archive.read("DefianceLoader/plugins/defiance_plugin_regroup.plugin.json"))
            self.assertEqual(next(s["default"] for s in manifest["settings"] if s["key"] == "enabled"), "false")
            manifest = json.loads(archive.read("DefianceLoader/plugins/defiance_plugin_vehicle_arrival.plugin.json"))
            self.assertEqual(next(s["default"] for s in manifest["settings"] if s["key"] == "enabled"), "false")
            manifest = json.loads(archive.read("DefianceLoader/plugins/defiance_plugin_cover_markers.plugin.json"))
            self.assertEqual(next(s["default"] for s in manifest["settings"] if s["key"] == "enabled"), "true")
            manifest = json.loads(archive.read("DefianceLoader/plugins/defiance_plugin_weapon_drops.plugin.json"))
            self.assertEqual(next(s["default"] for s in manifest["settings"] if s["key"] == "enabled"), "false")
            names_off = [package.LEGION_VEHICLE_HACKING_DLL, *(path.name for path in self.moving.values())]
            for name in names_off:
                manifest = json.loads(archive.read("DefianceLoader/plugins/" + name.replace(".dll", ".plugin.json")))
                enabled = next(s["default"] for s in manifest["settings"] if s["key"] == "enabled")
                self.assertEqual(enabled, "false", name)

    def test_exe_startup_package_contains_injector_loader_and_defaults_without_proxy(self):
        result = self.build(startup="exe")
        self.assertEqual(result.returncode, 0, result.stderr)
        with zipfile.ZipFile(self.out) as archive:
            names = archive.namelist()
            self.assertIn("bin/defiance-pickup-inject.exe", names)
            self.assertIn("bin/defiance_loader.dll", names)
            self.assertIn("bin/defiance-pickup-inject.ini", names)
            self.assertNotIn(f"bin/{package.stage.proxy_name()}", names)
            self.assertEqual(archive.read("bin/defiance-pickup-inject.exe"), self.injector.read_bytes())
            self.assertEqual(archive.read("bin/defiance_loader.dll"), self.loader.read_bytes())
            settings = archive.read("bin/defiance-pickup-inject.ini").decode("utf-8")
            self.assertIn("builds = known", settings)
            self.assertIn("game = true", settings)
            self.assertIn("wait = 180", settings)
            self.assertIn("pause = auto", settings)
            self.assertIn("mode = loader", settings)
            self.assertIn("loader_dll = defiance_loader.dll", settings)
            self.assertIn("DefianceLoader/plugins/" + package.EXPANDED_AMMO_DLL, names)
            self.assertIn("DefianceLoader/plugins/" + package.EXPANDED_AMMO_DLL.removesuffix(".dll") + ".plugin.json", names)
            self.assertIn("Start `bin/defiance-pickup-inject.exe`", archive.read("README.md").decode("utf-8"))
            self.assertNotIn("bin/dxgi.dll", archive.read("README.md").decode("utf-8"))
            self.assertFalse(any(name.lower().endswith(".ini") for name in names
                                 if name != "bin/defiance-pickup-inject.ini"))

    def test_companion_ui_mod_is_derived_from_the_game(self):
        game = self.root / "game"
        game.mkdir()
        with zipfile.ZipFile(game / "basis.pak", "w") as archive:
            archive.writestr(package_squad_scroll.RESOURCE, panel_fixture())
            archive.writestr(package_squad_scroll.VEHICLE_RESOURCE, panel_fixture(vehicle=True))
            archive.writestr(package_squad_scroll.AMMO_RESOURCE, AMMO_INFO)
            archive.writestr(package_squad_scroll.TRAINING_RESOURCE, TRAINING_INFO)
            archive.writestr(package_unit_inspection.RESOURCE, reload_bar())
            for name, data in fixture_resources().items():
                archive.writestr(name, data)
        result = self.build(game=game)
        self.assertEqual(result.returncode, 0, result.stderr)
        with zipfile.ZipFile(self.out) as archive:
            names = archive.namelist()
            self.assertIn("mods/defiance_squad_scroll/mod.json", names)
            self.assertIn("mods/defiance_squad_scroll/sources.json", names)
            self.assertIn("mods/defiance_squad_scroll/basis/" + package_squad_scroll.RESOURCE, names)
            self.assertIn("mods/defiance_squad_scroll/basis/" + package_squad_scroll.VEHICLE_RESOURCE, names)
            self.assertIn("mods/defiance_squad_scroll/basis/" + package_squad_scroll.TRAINING_RESOURCE, names)
            sources = json.loads(archive.read("mods/defiance_squad_scroll/sources.json"))
            self.assertEqual(
                sources[package_squad_scroll.TRAINING_RESOURCE]["basis"]["layout_revision"], 5)
            self.assertTrue(any(name.startswith("mods/defiance_squad_scroll/basis/textures/") for name in names))
            self.assertIn("mods/defiance_unit_inspection/basis/" + package_unit_inspection.RESOURCE, names)
            animation_root = "mods/" + package_moving_actions_animation.MOD_DIR + "/"
            self.assertIn(animation_root + "mod.json", names)
            self.assertIn(animation_root + "sources.json", names)
            self.assertIn(animation_root + "assets/moving_throw.anim", names)
            self.assertIn(animation_root + "assets/moving_throw_back.anim", names)

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
        # The package ships no game; the config tool needs the executable there.
        (game / "bin/trm.exe").write_bytes(b"")
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
                self.assertIn("defiance.vehicle-arrival.enabled = false", report.stdout)
                self.assertIn("defiance.cover-markers.enabled = true", report.stdout)
                self.assertIn("defiance.weapon-drops.enabled = false", report.stdout)
                self.assertIn("defiance.legion-vehicle-hacking.enabled = false", report.stdout)
                self.assertIn("defiance.moving-actions.enabled = false", report.stdout)
                self.assertEqual(config.read_bytes() if config.exists() else None, before)

    def test_private_test_plugins_are_never_packaged(self):
        plugin = self.source / "defiance_plugin_testing_god_mode.dll"
        plugin.write_bytes(b"test aid")
        plugin.with_suffix(".pdb").write_bytes(b"symbols")
        plugin.with_suffix(".plugin.json").write_text('{"id": "defiance.testing-god-mode"}')
        result = self.build()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("private test plugin", result.stderr)
        self.assertFalse(self.out.exists())

    def test_feature_plugins_ship_their_committed_manifest(self):
        plugin = self.source / "defiance_plugin_feature_vehicle_special_fire.dll"
        plugin.write_bytes(b"defiance.vehicle-special-fire")
        plugin.with_suffix(".pdb").write_bytes(b"symbols")
        manifest = plugin.with_suffix(".plugin.json")
        committed = json.loads(
            (package.ROOT / "plugins/vehicle-special-fire" / manifest.name).read_text(encoding="utf-8"))
        # The committed manifest is what keeps this plugin off for new players.
        enabled = next(s for s in committed["settings"] if s["key"] == "enabled")
        self.assertEqual(enabled["default"], "false")
        for default, ok in (("true", False), ("false", True)):
            with self.subTest(default=default):
                staged = json.loads(json.dumps(committed))
                next(s for s in staged["settings"] if s["key"] == "enabled")["default"] = default
                manifest.write_text(json.dumps(staged))
                result = self.build()
                self.assertEqual(result.returncode == 0, ok, result.stderr)
                if not ok:
                    self.assertIn("differs from", result.stderr)

    def test_missing_required_files_refuse_an_incomplete_package(self):
        for missing in (self.proxy, self.sidecar, self.regroup, self.expanded, self.scroll,
                        self.cover_markers, self.vehicle_arrival, self.weapon_drops,
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


class ReleaseWorkflowTests(unittest.TestCase):
    def test_release_builds_every_packaged_standalone_plugin(self):
        # package.py refuses a missing plugin, so a workspace the release
        # workflow does not build fails the tagged release.
        workflow = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        for folder, _ in package.stage.STANDALONE:
            self.assertIn(f"cargo build --release --manifest-path {folder}/Cargo.toml", workflow)


if __name__ == "__main__":
    unittest.main()
