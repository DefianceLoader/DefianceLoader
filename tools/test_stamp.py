"""tools/stamp.py's test cache: which inputs a test has, and that only passes
are remembered. Uses a temporary stamp directory; no game DLL needed."""
import os, pathlib, subprocess, sys, tempfile, unittest
from unittest import mock

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
        self.saved = stamp.STAMPS, stamp.PASSES
        self.folder = tempfile.TemporaryDirectory()
        stamp.STAMPS = stamp.PASSES = pathlib.Path(self.folder.name)
        os.environ.pop("DEFIANCE_NO_STAMP", None)

    def tearDown(self):
        stamp.STAMPS, stamp.PASSES = self.saved
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
        (stamp.ROOT / "out").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=stamp.ROOT / "out") as scratch:
            failing = pathlib.Path(scratch) / "failing_test.py"
            failing.write_text("import sys\nsys.exit(3)\n", encoding="utf-8")
            script = failing.relative_to(stamp.ROOT).as_posix()
            self.assertEqual(stamp.run_test(script, []), 3)
            self.assertEqual(list(stamp.STAMPS.glob("test-failing_test*.json")), [])


class VariantRefreshTests(unittest.TestCase):
    TOOLS = ("tools/payload.py", "tools/icon.py", "tools/units.py", "tools/variant.py")

    class Build:
        def __init__(self, root, name, base=None, hashes_match=True):
            self.name = name
            self._base = base
            self._hashes_match = hashes_match
            self.folder = root / "tools" / "variants" / name / "units"
            self.folder.mkdir(parents=True)
            self.logic = root / "bin" / name / "logic.dll"
            self.game = root / "bin" / name / "game.dll"
            self.logic.parent.mkdir(parents=True, exist_ok=True)
            self.logic.write_bytes((name + " logic").encode())
            self.game.write_bytes((name + " game").encode())
            self.layout = root / "tools" / "layouts" / f"{name}.json"
            self.layout.parent.mkdir(parents=True, exist_ok=True)
            self.layout.write_text("{}", encoding="utf-8")

        def lineage(self):
            return ([self._base] if self._base else []) + [self]

        def hashes_match_layout(self):
            return self._hashes_match

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temp.name)
        self.out = self.root / "out"
        self.out.mkdir()
        self.reference = self.Build(self.root, "reference")
        self.variant = self.Build(self.root, "gog-2099-01-01", self.reference)
        # The base's committed units are part of every derived variant's key.
        (self.reference.folder / "core.bin").write_bytes(b"base units")
        self.output = self.variant.folder / "core.bin"
        self.output.write_bytes(b"variant units")
        (self.root / "patch").mkdir()
        (self.root / "patch" / "shared.asm").write_text("patch one", encoding="utf-8")
        self.source = self.root / "patch" / "shared.asm"
        self.saved = {
            "ROOT": stamp.ROOT,
            "STAMPS": stamp.STAMPS,
            "FILE_CACHE": stamp.FILE_CACHE,
            "file_cache": stamp._file_cache,
            "assembly_inputs": stamp.ASSEMBLY_INPUTS,
        }
        stamp.ROOT = self.root
        stamp.STAMPS = self.out / "stamps"
        stamp.FILE_CACHE = stamp.STAMPS / "files.json"
        stamp._file_cache = None
        stamp.ASSEMBLY_INPUTS = ["patch/*.asm"]
        self.relative = mock.patch.object(
            stamp.builds, "relative",
            side_effect=lambda path: pathlib.Path(path).relative_to(stamp.ROOT).as_posix(),
        )
        self.relative.start()
        self.layouts = mock.patch.object(stamp.builds, "with_layouts", return_value=[self.variant])
        self.layouts.start()
        self.calls = []

        def successful_run(args, cwd):
            self.calls.append(args)
            if args[1] == "tools/variant.py":
                self.output.write_bytes(b"rebuilt variant units")
            return type("Result", (), {"returncode": 0})()

        self.run = mock.patch.object(stamp.subprocess, "run", side_effect=successful_run)
        self.run.start()

    def tearDown(self):
        self.run.stop()
        self.layouts.stop()
        self.relative.stop()
        stamp.ROOT = self.saved["ROOT"]
        stamp.STAMPS = self.saved["STAMPS"]
        stamp.FILE_CACHE = self.saved["FILE_CACHE"]
        stamp._file_cache = self.saved["file_cache"]
        stamp.ASSEMBLY_INPUTS = self.saved["assembly_inputs"]
        self.temp.cleanup()

    def _refresh(self, require_all=False):
        return stamp.refresh_variants(require_all=require_all)

    def test_runs_four_tools_then_skips_unchanged_variant(self):
        self.assertEqual(self._refresh(), 0)
        self.assertEqual([call[1] for call in self.calls], list(self.TOOLS))
        self.assertTrue(stamp.fresh("assemble-variant-gog-2099-01-01", stamp.digest(
            stamp.ASSEMBLY_INPUTS + [stamp.builds.relative(p) for b in self.variant.lineage()
                                    for p in (b.logic, b.game)]
            + [f"tools/variants/{b.name}/units/*" for b in self.variant.lineage()]
        )))
        self.assertEqual(self._refresh(), 0)
        self.assertEqual(len(self.calls), 4)

    def test_source_and_output_changes_each_invalidate_the_cache(self):
        self._refresh()
        self.source.write_text("a larger changed shared patch", encoding="utf-8")
        self._refresh()
        self.assertEqual(len(self.calls), 8)
        self.output.write_bytes(b"manually changed output bytes")
        self._refresh()
        self.assertEqual(len(self.calls), 12)

    def test_failed_tool_is_not_stamped(self):
        self.run.stop()
        self.run = mock.patch.object(
            stamp.subprocess, "run",
            side_effect=[type("Result", (), {"returncode": 0})(),
                         type("Result", (), {"returncode": 7})()],
        )
        self.run.start()
        self.assertEqual(self._refresh(), 7)
        self.assertFalse((stamp.STAMPS / "assemble-variant-gog-2099-01-01.json").exists())

    def test_dll_and_base_unit_changes_each_invalidate_the_cache(self):
        self._refresh()
        self.variant.logic.write_bytes(b"changed target DLL bytes")
        self._refresh()
        self.assertEqual(len(self.calls), 8)
        (self.reference.folder / "core.bin").write_bytes(b"changed base units")
        self._refresh()
        self.assertEqual(len(self.calls), 12)

    def test_missing_lineage_falls_back_or_refuses_when_required(self):
        self.reference.logic.unlink()
        with mock.patch("builtins.print") as printed:
            self.assertEqual(self._refresh(), 0)
        self.assertIn("using committed units", printed.call_args.args[0])
        self.assertEqual(self.calls, [])
        with self.assertRaisesRegex(SystemExit, "reference/logic.dll missing"):
            self._refresh(require_all=True)

    def test_hash_mismatch_refuses_without_running_or_stamping(self):
        self.variant._hashes_match = False
        self.assertEqual(self._refresh(), 1)
        self.assertEqual(self.calls, [])
        self.assertFalse((stamp.STAMPS / "assemble-variant-gog-2099-01-01.json").exists())

    def test_empty_outputs_are_not_stamped(self):
        self.output.unlink()
        self.run.stop()
        self.run = mock.patch.object(stamp.subprocess, "run", return_value=type("Result", (), {"returncode": 0})())
        self.run.start()
        self.assertEqual(self._refresh(), 1)
        self.assertFalse((stamp.STAMPS / "assemble-variant-gog-2099-01-01.json").exists())


class SharedCacheTests(unittest.TestCase):
    def test_every_worktree_shares_the_main_checkouts_cache(self):
        common = subprocess.run(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
                                cwd=stamp.ROOT, check=True, capture_output=True, text=True).stdout.strip()
        expected = pathlib.Path(common).parent / "out" / "cache"
        if os.environ.get("DEFIANCE_CACHE_DIR"):
            expected = pathlib.Path(os.environ["DEFIANCE_CACHE_DIR"])
        self.assertEqual(stamp.shared_cache.directory().resolve(), expected.resolve())
        self.assertEqual(stamp.PASSES.parent, stamp.shared_cache.directory())

    def test_writes_replace_the_whole_file(self):
        with tempfile.TemporaryDirectory() as folder:
            target = pathlib.Path(folder) / "nested" / "result.json"
            stamp.shared_cache.write_text(target, "first\n")
            stamp.shared_cache.write_text(target, "second\n")
            self.assertEqual(target.read_bytes(), b"second\n")
            self.assertEqual(sorted(p.name for p in target.parent.iterdir()), ["result.json"])


if __name__ == "__main__":
    unittest.main()
