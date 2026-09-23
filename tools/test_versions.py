"""Component versions agree everywhere, and tools/bump.py changes them together.

The checks run against the repository; the bumps run in a scratch copy of the
files that hold versions, so nothing here edits the tree.

    python tools/test_versions.py
"""
import json
import pathlib
import re
import shutil
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import bump

ROOT = bump.ROOT
HOLDERS = ["Cargo.lock", "crates/loader/Cargo.toml", bump.BUILTIN_TABLE, "plugins/*/Cargo.toml",
           "plugins/*/Cargo.lock", "plugins/*/*.plugin.json", "plugins/*/src/lib.rs"]


class VersionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="defiance-versions-")
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        for pattern in HOLDERS:
            for source in ROOT.glob(pattern):
                target = self.root / source.relative_to(ROOT)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, target)

    def version(self, name):
        return next(bump.crate_version(c) for c in bump.components(self.root) if c.name == name)

    def test_the_repository_agrees(self):
        self.assertEqual(bump.problems(ROOT), [])

    def test_a_bump_changes_every_copy_together(self):
        loader, core = self.version("loader"), self.version("core")
        major, minor, patch = bump.parse(loader)
        bump.bump("patch", ["loader"], self.root)
        self.assertEqual(self.version("loader"), f"{major}.{minor}.{patch + 1}")
        lock = (self.root / "Cargo.lock").read_text(encoding="utf-8")
        self.assertIn(f'name = "defiance-loader"\nversion = "{major}.{minor}.{patch + 1}"', lock)

        selection = bump.parse(self.version("selection"))
        bump.bump("minor", ["builtins"], self.root)
        expected = f"{selection[0]}.{selection[1] + 1}.0"
        for name in ("selection", "ammunition", "preview-weapon"):
            component = next(c for c in bump.components(self.root) if c.name == name)
            self.assertEqual(bump.manifest_version(self.root, component), bump.crate_version(component))
        self.assertEqual(self.version("selection"), expected)
        self.assertEqual(self.version("core"), core, "builtins leaves core alone")

        regroup = next(c for c in bump.components(self.root) if c.name == "regroup")
        bump.bump("patch", ["regroup"], self.root)
        self.assertEqual(json.loads(regroup.sidecar.read_text(encoding="utf-8"))["version"],
                         bump.crate_version(regroup))
        self.assertEqual(bump.lock_version(regroup), bump.crate_version(regroup))
        self.assertEqual(bump.problems(self.root), [])

    def test_a_bump_backwards_is_refused(self):
        with self.assertRaises(SystemExit):
            bump.bump("0.0.1", ["loader"], self.root)

    def test_a_bump_out_of_a_dependants_range_writes_nothing(self):
        # regroup declares a dependency on selection; cap it, then exceed the cap.
        sidecar = next(self.root.glob("plugins/regroup/*.plugin.json"))
        data = json.loads(sidecar.read_text(encoding="utf-8"))
        selection = self.version("selection")
        for dependency in data["depends"]:
            if dependency["id"] == "defiance.selection":
                dependency["max"] = selection
        sidecar.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
        before = {p: p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        with self.assertRaises(SystemExit) as refused:
            bump.bump("major", ["selection"], self.root)
        self.assertIn("regroup needs defiance.selection", str(refused.exception))
        self.assertEqual({p: p.read_bytes() for p in self.root.rglob("*") if p.is_file()}, before)

    def test_a_literal_exported_version_is_reported(self):
        lib = self.root / "plugins/attack/src/lib.rs"
        lib.write_text(re.sub(r'concat!\(env!\("CARGO_PKG_VERSION"\), "\\0"\)', 'b"9.9.9\\\\0"',
                              lib.read_text(encoding="utf-8")), encoding="utf-8")
        self.assertTrue(any("literal version" in line for line in bump.problems(self.root)))


if __name__ == "__main__":
    unittest.main()
