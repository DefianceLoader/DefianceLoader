"""End-to-end tests for the explicit configuration migration tool.

Build first (`mise run loader`). Needs no game DLLs and never touches a real
installation: every case uses a fresh temporary game directory.
"""
import os
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent


def tool():
    target = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    for candidate in (target / "release" / "defiance-config.exe",
                      target / "debug" / "defiance-config.exe"):
        if candidate.is_file():
            return candidate
    raise SystemExit("defiance-config.exe is missing; run mise run loader first")


class ConfigToolTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.bootstrap = self.bin / "defiance-loader.ini"
        self.plugins = self.root / "DefianceLoader" / "plugins"
        self.plugins.mkdir(parents=True)
        (self.plugins / "defiance_plugin_core.dll").write_bytes(b"defiance.core plugin")

    def run_tool(self, *args):
        return subprocess.run([str(tool()), "--game", str(self.bin), *args],
                              capture_output=True, text=True)

    def test_preview_reports_the_mismatch_and_writes_nothing(self):
        self.bootstrap.write_text("wait = 15\nallow_unknown_build = yes\nplugins = ../old/plugins\n")
        result = self.run_tool()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("set `plugins =", result.stdout)
        self.assertIn("wait = 15", result.stdout)
        self.assertIn("preview only", result.stdout)
        self.assertFalse((self.root / "DefianceLoader" / "config" / "core.ini").exists())

    def test_apply_is_idempotent_and_preserves_explicit_values(self):
        self.bootstrap.write_text("wait = 15\nallow_unknown_build = yes\n")
        core = self.root / "DefianceLoader" / "config" / "core.ini"
        core.parent.mkdir(parents=True)
        core.write_text("[loader]\nwait = 5\n; keep me\n")
        applied = self.run_tool("--apply")
        self.assertEqual(applied.returncode, 0, applied.stderr)
        text = core.read_text()
        self.assertIn("wait = 5", text)
        self.assertIn("; keep me", text)
        self.assertIn("allow_unknown_build = yes", text)
        self.assertTrue((core.parent / "defiance-config.ini").is_file())

        again = self.run_tool("--apply")
        self.assertEqual(again.returncode, 0, again.stderr)
        self.assertIn("up to date", again.stdout)
        self.assertEqual(core.read_text(), text)

    def test_a_newer_schema_is_refused(self):
        meta = self.root / "DefianceLoader" / "config"
        meta.mkdir(parents=True)
        (meta / "defiance-config.ini").write_text("[config]\nschema_version = 99\n")
        result = self.run_tool()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("newer", result.stderr)

    def test_a_legacy_plugins_override_mismatch_is_reported(self):
        # The override points at a missing directory while plugins live at the
        # new default; the tool names the exact replacement, never guessing.
        self.bootstrap.write_text("plugins = ../old/plugins\n")
        result = self.run_tool()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("no DLLs", result.stdout)
        self.assertIn("set `plugins =", result.stdout)
        # With a DLL in the override directory there is nothing to report.
        override = self.root / "old" / "plugins"
        override.mkdir(parents=True)
        (override / "defiance_plugin_core.dll").write_bytes(b"defiance.core")
        quiet = self.run_tool()
        self.assertEqual(quiet.returncode, 0, quiet.stderr)
        self.assertNotIn("no DLLs", quiet.stdout)

    def test_report_names_paths_provenance_and_reasons_without_writing(self):
        config = self.root / "DefianceLoader" / "config"
        config.mkdir(parents=True)
        (config / "infantry.ini").write_text("[defiance.selection]\nenabled = false\n")
        (config / "core.ini").write_text("[loader]\nwait = 5\n")
        self.bootstrap.write_text("root = ../DefianceLoader\n")
        result = self.run_tool("--report")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("defiance.selection.enabled = false", result.stdout)
        self.assertIn("infantry.ini", result.stdout)
        self.assertIn("loader.wait = 5", result.stdout)
        # The report is read-only: it writes no defaults and no metadata.
        self.assertFalse((config / "weapons.ini").exists())
        self.assertFalse((config / "defiance-config.ini").exists())

    def test_packaged_defaults_are_written_once_and_never_overwritten(self):
        config = self.root / "DefianceLoader" / "config"
        first = subprocess.run([str(tool()), "--defaults", str(config)],
                               capture_output=True, text=True)
        self.assertEqual(first.returncode, 0, first.stderr)
        core = config / "core.ini"
        self.assertTrue(core.is_file())
        self.assertTrue((config / "infantry.ini").is_file())
        core.write_text("; mine\n[loader]\nwait = 3\n")
        second = subprocess.run([str(tool()), "--defaults", str(config)],
                                capture_output=True, text=True)
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertIn("kept", second.stdout)
        self.assertEqual(core.read_text(), "; mine\n[loader]\nwait = 3\n")

    def test_migration_refuses_unreadable_or_malformed_inputs_without_writes(self):
        config = self.root / "DefianceLoader" / "config"
        config.mkdir(parents=True)
        core = config / "core.ini"
        meta = config / "defiance-config.ini"
        for damaged in (self.bootstrap, core):
            for data in (b"\xff\xfe[\x00l\x00", b"[broken\nwait=7\n"):
                with self.subTest(path=damaged.name, data=data):
                    self.bootstrap.write_text("wait=7\n")
                    core.write_text("; preserve me\n[loader]\n")
                    damaged.write_bytes(data)
                    before = {p: p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
                    for args in ((), ("--apply",)):
                        result = self.run_tool(*args)
                        self.assertNotEqual(result.returncode, 0, result.stdout)
                        self.assertIn("no files changed", result.stderr)
                        after = {p: p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
                        self.assertEqual(after, before)
                    self.assertFalse(meta.exists())

    def test_migration_refuses_a_config_path_that_is_a_directory(self):
        self.bootstrap.write_text("wait=7\n")
        core = self.root / "DefianceLoader" / "config" / "core.ini"
        core.mkdir(parents=True)
        (core / "keep.txt").write_text("keep")
        result = self.run_tool("--apply")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no files changed", result.stderr)
        self.assertEqual((core / "keep.txt").read_text(), "keep")
        self.assertFalse((core.parent / "defiance-config.ini").exists())


if __name__ == "__main__":
    unittest.main()
