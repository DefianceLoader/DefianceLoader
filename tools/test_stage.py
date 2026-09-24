"""Filesystem integration tests for staging; no game DLLs or installation needed."""
import contextlib
import io
import pathlib
import tempfile
import unittest
import zipfile

import package_squad_scroll
import stage
from test_package_squad_scroll import AMMO_INFO, fixture as panel_fixture


class StagingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        # Resolved, so expectations built from it compare equal to resolved
        # results where the temp directory is a short 8.3 path (GitHub's
        # Windows runners use C:\Users\RUNNER~1\...).
        self.root = pathlib.Path(self.temp.name).resolve()
        self.game = self.root / "Game" / "bin"
        self.game.mkdir(parents=True)
        self.source = self.root / "build"
        self.source.mkdir()
        (self.source / stage.PROXY_LIB).write_bytes(b"defiance-loader test proxy")
        (self.source / stage.CRASH_HELPER).write_bytes(b"defiance-loader crash helper")
        self.plugin = "defiance_plugin_test.dll"
        (self.source / self.plugin).write_bytes(b"defiance.test plugin")
        self.manifest = "defiance_plugin_test.plugin.json"
        (self.source / self.manifest).write_text('{"id": "defiance.test"}')
        self.config = self.game / "defiance-loader.ini"

    def staging(self, dry=False):
        return stage.Staging(self.game, self.source, False, dry)

    def test_foreign_crash_helper_is_not_overwritten_or_removed(self):
        helper = self.game / stage.CRASH_HELPER
        helper.write_bytes(b"unrelated executable")
        install = self.staging()
        with self.assertRaises(SystemExit):
            install.install()
        self.assertFalse(install.target.exists())
        install.uninstall()
        self.assertEqual(helper.read_bytes(), b"unrelated executable")

    def test_default_install_uninstall_preserves_foreign_files_and_proxy(self):
        install = self.staging()
        expected = self.root / "Game" / "DefianceLoader" / "plugins"
        self.assertEqual(install.plugin_dir.resolve(), expected)
        install.target.write_bytes(b"foreign proxy")
        install.install()
        self.assertTrue((self.game / stage.CRASH_HELPER).is_file())
        self.assertEqual((expected / self.plugin).read_bytes(), b"defiance.test plugin")
        self.assertTrue((expected / self.manifest).is_file())
        self.assertFalse((self.game / "plugins").exists())
        foreign = expected / "defiance_plugin_foreign.dll"
        foreign.write_bytes(b"foreign plugin")
        foreign_manifest = expected / "vendor.plugin.json"
        foreign_manifest.write_text('{"id": "vendor.thing"}')
        install.uninstall()
        self.assertFalse((self.game / stage.CRASH_HELPER).exists())
        self.assertFalse((expected / self.plugin).exists())
        self.assertFalse((expected / self.manifest).exists())
        self.assertEqual(foreign.read_bytes(), b"foreign plugin")
        self.assertEqual(foreign_manifest.read_text(), '{"id": "vendor.thing"}')
        self.assertEqual(install.target.read_bytes(), b"foreign proxy")

    def test_install_without_a_manifest_still_copies_the_dll(self):
        (self.source / self.manifest).unlink()
        install = self.staging()
        expected = self.root / "Game" / "DefianceLoader" / "plugins"
        install.install()
        self.assertTrue((expected / self.plugin).is_file())
        self.assertFalse((expected / self.manifest).exists())
        install.uninstall()
        self.assertFalse((expected / self.plugin).exists())

    def test_legacy_relative_and_absolute_settings_are_honored(self):
        for value in ["plugins", "../custom plugins", str(self.root / "absolute plugins")]:
            with self.subTest(value=value):
                self.config.write_text(f"PlUgInS = {value} ; comment\n[feature]\nplugins = ignored\n")
                install = self.staging()
                expected = pathlib.Path(value)
                if not expected.is_absolute():
                    expected = self.game / expected
                self.assertEqual(install.plugin_dir.resolve(), expected.resolve())
                install.install()
                self.assertTrue((expected / self.plugin).is_file())
                install.uninstall()
                self.assertFalse((expected / self.plugin).exists())
                self.assertTrue(self.config.is_file())

    def test_root_key_moves_the_default_plugin_directory(self):
        self.config.write_text("root = ../Mods/Defiance ; comment\n")
        install = self.staging()
        expected = self.root / "Game" / "Mods" / "Defiance" / "plugins"
        self.assertEqual(install.plugin_dir.resolve(), expected.resolve())
        install.install()
        self.assertTrue((expected / self.plugin).is_file())
        install.uninstall()
        self.assertFalse((expected / self.plugin).exists())
        self.assertTrue(self.config.is_file())

    def test_legacy_plugins_override_beats_root(self):
        self.config.write_text("root = ../Mods/Defiance\nplugins = ../Legacy/plugins\n")
        install = self.staging()
        expected = self.root / "Game" / "Legacy" / "plugins"
        self.assertEqual(install.plugin_dir.resolve(), expected.resolve())

    def test_install_and_uninstall_leave_config_and_logs_alone(self):
        loader = self.root / "Game" / "DefianceLoader"
        (loader / "config").mkdir(parents=True)
        (loader / "logs").mkdir()
        core = loader / "config" / "core.ini"
        core.write_text("[loader]\nwait = 60\n")
        log = loader / "logs" / "defiance-loader.log"
        log.write_text("old session\n")
        install = self.staging()
        install.install()
        install.uninstall()
        self.assertEqual(core.read_text(), "[loader]\nwait = 60\n")
        self.assertEqual(log.read_text(), "old session\n")

    def test_missing_setting_uses_default_and_last_unsectioned_value_wins(self):
        self.config.write_text("wait = 15\n[feature]\nplugins = ignored\n")
        expected = self.root / "Game" / "DefianceLoader" / "plugins"
        self.assertEqual(self.staging().plugin_dir.resolve(), expected)
        self.config.write_text("plugins = ignored\nplugins = ../chosen # comment\n")
        self.assertEqual(self.staging().plugin_dir.resolve(), self.root / "Game" / "chosen")

    def test_standalone_plugins_are_staged_and_removed(self):
        dll = self.root / "defiance_plugin_regroup.dll"
        dll.write_bytes(b"defiance.regroup standalone")
        manifest = self.root / "defiance_plugin_regroup.plugin.json"
        manifest.write_text('{"id": "defiance.regroup"}')
        install = stage.Staging(self.game, self.source, False, False,
                                extra_plugins=[(dll, manifest)])
        expected = self.root / "Game" / "DefianceLoader" / "plugins"
        install.install()
        self.assertEqual((expected / dll.name).read_bytes(), b"defiance.regroup standalone")
        self.assertTrue((expected / manifest.name).is_file())
        install.uninstall()
        self.assertFalse((expected / dll.name).exists())
        self.assertFalse((expected / manifest.name).exists())

    def test_companion_ui_mod_is_staged_when_the_game_has_paks(self):
        with zipfile.ZipFile(self.root / "Game" / "basis.pak", "w") as archive:
            archive.writestr(package_squad_scroll.RESOURCE, panel_fixture())
            archive.writestr(package_squad_scroll.VEHICLE_RESOURCE, panel_fixture(vehicle=True))
            archive.writestr(package_squad_scroll.AMMO_RESOURCE, AMMO_INFO)
        install = stage.Staging(self.game, self.source, False, False)
        mod = self.root / "Game" / "mods" / stage.MOD_DIR
        install.install()
        self.assertTrue((mod / "mod.json").is_file())
        self.assertTrue((mod / "basis" / package_squad_scroll.RESOURCE).is_file())
        install.uninstall()
        self.assertFalse(mod.exists())

    def test_staging_accepts_the_game_root_or_its_bin(self):
        # Given the root, staging must operate in bin; otherwise `../DefianceLoader`
        # resolves one level above the install and the game keeps its old plugins.
        with zipfile.ZipFile(self.root / "Game" / "basis.pak", "w") as archive:
            archive.writestr(package_squad_scroll.RESOURCE, panel_fixture())
            archive.writestr(package_squad_scroll.VEHICLE_RESOURCE, panel_fixture(vehicle=True))
            archive.writestr(package_squad_scroll.AMMO_RESOURCE, AMMO_INFO)
        self.assertEqual(stage.bin_directory(self.root / "Game"), self.game)
        self.assertEqual(stage.bin_directory(self.game), self.game)
        self.assertEqual(stage.bin_directory(self.root / "Elsewhere"), self.root / "Elsewhere")

    def test_dry_run_install_and_uninstall_do_not_change_files(self):
        install = self.staging(dry=True)
        install.install()
        self.assertFalse(install.target.exists())
        self.assertFalse(install.plugin_dir.exists())
        self.staging().install()
        install.uninstall()
        self.assertTrue(install.target.exists())
        self.assertTrue((install.plugin_dir / self.plugin).exists())


if __name__ == "__main__":
    with contextlib.redirect_stdout(io.StringIO()):
        unittest.main()
