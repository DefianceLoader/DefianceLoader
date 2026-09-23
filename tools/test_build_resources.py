"""Check release resource failures with a disposable Cargo build-script fixture."""
import os
import pathlib
import subprocess
import tempfile
import unittest

import pefile

ROOT = pathlib.Path(__file__).resolve().parent.parent


class ResourceBuildTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix="defiance-resources-")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.root = pathlib.Path(cls.temporary.name)
        support = (ROOT / "crates/build-support").as_posix()
        (cls.root / "Cargo.toml").write_text(
            '[package]\nname = "resource-fixture"\nversion = "0.1.0"\nedition = "2021"\n'
            f'[build-dependencies]\ndefiance-build-support = {{ path = "{support}" }}\n'
            '[workspace]\n', encoding="utf-8")
        (cls.root / "src").mkdir()
        (cls.root / "src/main.rs").write_text("fn main() {}", encoding="utf-8")
        (cls.root / "build.rs").write_text(
            'fn main() { defiance_build_support::windows_resources("exe", "Resource fixture"); }',
            encoding="utf-8")
        # A native fake compiler allows independent nonzero-exit and missing-output cases.
        source = cls.root / "fake-rc.rs"
        source.write_text('fn main() { std::process::exit(std::env::var("TEST_RC_EXIT").unwrap().parse().unwrap()); }',
                          encoding="utf-8")
        cls.compiler = cls.root / "fake-rc.exe"
        subprocess.run(["rustc", str(source), "-o", str(cls.compiler)], check=True)

    def build(self, release, compiler, code="0"):
        env = dict(os.environ, DEFIANCE_RC=str(compiler), TEST_RC_EXIT=code,
                   CARGO_TARGET_DIR=str(self.root / self._testMethodName))
        args = ["cargo", "build", "--manifest-path", str(self.root / "Cargo.toml")]
        if release:
            args.append("--release")
        return subprocess.run(args, env=env, capture_output=True, text=True, timeout=120)

    def test_release_refuses_missing_compiler(self):
        result = self.build(True, self.root / "missing-rc.exe")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Windows release metadata is required", result.stderr)
        self.assertIn("could not run", result.stderr)

    def test_release_refuses_compiler_failure(self):
        result = self.build(True, self.compiler, "7")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Windows release metadata is required", result.stderr)
        self.assertIn("failed to generate resources", result.stderr)

    def test_release_refuses_success_without_output(self):
        result = self.build(True, self.compiler)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Windows release metadata is required", result.stderr)

    def test_development_can_build_without_compiler(self):
        result = self.build(False, self.root / "missing-rc.exe")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("development build has no resources", result.stderr)

    def test_each_output_names_itself(self):
        # A DLL and a tool from one crate, as the loader builds its proxy and
        # crash helper: each must carry its own name, not the crate's, so the
        # helper never claims to be the DLL. Needs the real resource compiler.
        crate = self.root / "outputs"
        (crate / "src/bin").mkdir(parents=True)
        (crate / "Cargo.toml").write_text(
            '[package]\nname = "output-fixture"\nversion = "1.2.3"\nedition = "2021"\n'
            '[lib]\ncrate-type = ["cdylib"]\n'
            f'[build-dependencies]\ndefiance-build-support = {{ path = "{(ROOT / "crates/build-support").as_posix()}" }}\n'
            '[workspace]\n', encoding="utf-8")
        (crate / "src/lib.rs").write_text("", encoding="utf-8")
        (crate / "src/bin/output-tool.rs").write_text("fn main() {}", encoding="utf-8")
        (crate / "build.rs").write_text(
            'use defiance_build_support::Artifact;\n'
            'fn main() { defiance_build_support::windows_resources_for(&['
            '(Artifact::Library, "Fixture library"), (Artifact::Bin("output-tool"), "Fixture tool")]); }',
            encoding="utf-8")
        env = {k: v for k, v in os.environ.items() if k != "DEFIANCE_RC"}
        env["CARGO_TARGET_DIR"] = str(self.root / "outputs-target")
        result = subprocess.run(["cargo", "build", "--release", "--manifest-path", str(crate / "Cargo.toml")],
                                env=env, capture_output=True, text=True, timeout=300)
        if result.returncode and "rc.exe not found" in result.stderr:
            self.skipTest("the Windows SDK resource compiler is not installed")
        self.assertEqual(result.returncode, 0, result.stderr)
        release = self.root / "outputs-target/release"
        library, tool = strings(release / "output_fixture.dll"), strings(release / "output-tool.exe")
        self.assertEqual((library["OriginalFilename"], library["InternalName"], library["FileDescription"]),
                         ("output_fixture.dll", "output-fixture", "Fixture library"))
        self.assertEqual((tool["OriginalFilename"], tool["InternalName"], tool["FileDescription"]),
                         ("output-tool.exe", "output-tool", "Fixture tool"))
        self.assertEqual(library["FileVersion"], "1.2.3")
        self.assertEqual(tool["FileVersion"], "1.2.3")


def strings(path):
    """A binary's VERSIONINFO string table, as a dict."""
    image = pefile.PE(str(path))
    table = {}
    for info in getattr(image, "FileInfo", []):
        for entry in info:
            for block in getattr(entry, "StringTable", []):
                table.update({k.decode(): v.decode() for k, v in block.entries.items()})
    image.close()
    return table


if __name__ == "__main__":
    unittest.main()
