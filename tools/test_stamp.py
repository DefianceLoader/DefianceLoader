"""tools/stamp.py's test cache: which inputs a test has, and that only passes
are remembered. Uses a temporary stamp directory; no game DLL needed."""
import os, pathlib, subprocess, sys, tempfile, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import stamp


class ImportTests(unittest.TestCase):
    def test_import_forms(self):
        source = "import os\nimport builds\nfrom pe import Image\nimport a, b as c  # x\nimport x.y\n    import late\n"
        self.assertEqual(sorted(stamp.imported_names(source)), ["a", "b", "builds", "late", "os", "pe", "x"])

    def test_a_test_reaches_the_modules_it_uses(self):
        names = [p.name for p in stamp.imported_tools(stamp.ROOT / "tools" / "test_pickup_rust.py")]
        for module in ("test_pickup_rust.py", "test_chooser.py", "build.py", "pe.py", "builds.py"):
            self.assertIn(module, names)

    def test_argument_paths_and_artifacts_are_inputs(self):
        patterns, extra = stamp.test_inputs("tools/test_orders.py", ["tools/layouts", "--flag"])
        self.assertIn("tools/test_orders.py", patterns)
        self.assertIn("tools/layouts/**/*", patterns)
        self.assertIn("bin/**/*.dll", patterns)
        self.assertIn("--flag", extra)
        self.assertTrue(any(e.startswith("DEFIANCE_GAME_DIR=") for e in extra))


class CacheTests(unittest.TestCase):
    def setUp(self):
        self.saved = stamp.STAMPS
        self.folder = tempfile.TemporaryDirectory()
        stamp.STAMPS = pathlib.Path(self.folder.name)
        os.environ.pop("DEFIANCE_NO_STAMP", None)

    def tearDown(self):
        stamp.STAMPS = self.saved
        self.folder.cleanup()

    def test_recent_passes_are_remembered(self):
        for n in range(stamp.TEST_KEYS_KEPT + 2):
            stamp.record_pass("t", f"key{n}")
        self.assertTrue(stamp.passed_before("t", f"key{stamp.TEST_KEYS_KEPT + 1}"))
        self.assertTrue(stamp.passed_before("t", "key2"))
        self.assertFalse(stamp.passed_before("t", "key0"))
        self.assertFalse(stamp.passed_before("other", "key2"))

    def test_the_override_runs_everything(self):
        stamp.record_pass("t", "k")
        os.environ["DEFIANCE_NO_STAMP"] = "1"
        try:
            self.assertFalse(stamp.passed_before("t", "k"))
        finally:
            del os.environ["DEFIANCE_NO_STAMP"]

    def test_a_failing_test_is_not_remembered(self):
        with tempfile.TemporaryDirectory(dir=stamp.ROOT / "out") as scratch:
            failing = pathlib.Path(scratch) / "failing_test.py"
            failing.write_text("import sys\nsys.exit(3)\n", encoding="utf-8")
            script = failing.relative_to(stamp.ROOT).as_posix()
            self.assertEqual(stamp.run_test(script, []), 3)
            self.assertEqual(list(stamp.STAMPS.glob("test-failing_test*.json")), [])


if __name__ == "__main__":
    unittest.main()
