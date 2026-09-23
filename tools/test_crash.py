"""Crash only disposable subprocesses, then validate reports and minidump streams."""
import pathlib
import struct
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
BUILD = ROOT / "out/crash-tests/release"


class CrashTests(unittest.TestCase):
    def run_case(self, mode, helper=True):
        temp = tempfile.TemporaryDirectory(prefix="defiance-crash-test-")
        self.addCleanup(temp.cleanup)
        directory = pathlib.Path(temp.name)
        process = subprocess.run([
            str(BUILD / "crash-fixture.exe"), str(directory),
            str(BUILD / ("defiance-crash-helper.exe" if helper else "missing.exe")), mode,
            str(BUILD / "defiance_crash_test_plugin.dll"),
        ], capture_output=True, timeout=25)
        stem = pathlib.Path((directory / "stem.txt").read_text())
        return directory, stem, process

    def dump_exception(self, stem):
        self.assertEqual(stem.with_suffix(".status.txt").read_text(), "dump complete\n")
        data = stem.with_suffix(".dmp").read_bytes()
        self.assertEqual(data[:4], b"MDMP")
        count, directory = struct.unpack_from("<II", data, 8)
        streams = {}
        for index in range(count):
            kind, size, offset = struct.unpack_from("<III", data, directory + 12*index)
            self.assertLessEqual(offset + size, len(data))
            streams[kind] = data[offset:offset+size]
        self.assertIn(3, streams)  # thread list
        self.assertIn(4, streams)  # module list
        exception = streams[6]
        code = struct.unpack_from("<I", exception, 8)[0]
        context_size, context_rva = struct.unpack_from("<II", exception, 160)
        self.assertGreaterEqual(context_size, 256)
        context = data[context_rva:context_rva+context_size]
        return code, context

    def test_native_crash_registers_dump_mapping_and_previous_filter(self):
        directory, stem, process = self.run_case("av")
        self.assertNotEqual(process.returncode, 0)
        report = stem.with_suffix(".crash.txt").read_text()
        self.assertIn("exception=0xc0000005", report)
        self.assertIn("access=1 address=0x0", report)
        self.assertIn("RAX=0x123456789abcdef0", report)
        self.assertIn("previous filter", (directory / "chained.txt").read_text())
        code, context = self.dump_exception(stem)
        self.assertEqual(code, 0xc0000005)
        self.assertEqual(struct.unpack_from("<Q", context, 120)[0], 0x123456789abcdef0)
        details = stem.with_suffix(".details.txt").read_text()
        self.assertIn("fault location:", details)
        self.assertIn("fault mapping: fixture.live", details)
        self.assertNotIn("fault mapping: fixture.removed", details)
        self.assertIn("NOT an unwound call stack", details)

    def test_loader_and_separate_dll_panics(self):
        for mode, message in [("panic", "intentional crash regression panic"), ("plugin-panic", "intentional DLL panic regression")]:
            with self.subTest(mode=mode):
                _, stem, process = self.run_case(mode)
                self.assertNotEqual(process.returncode, 0)
                self.assertIn(message, stem.with_suffix(".crash.txt").read_text())
                code, _ = self.dump_exception(stem)
                self.assertEqual(code, 0xe042444c)

    def test_missing_helper_still_records_native_crash(self):
        directory, stem, process = self.run_case("av", helper=False)
        self.assertNotEqual(process.returncode, 0)
        self.assertIn("RAX=0x123456789abcdef0", stem.with_suffix(".crash.txt").read_text())
        self.assertIn("text report only", stem.with_suffix(".crash.txt").read_text())
        self.assertTrue((directory / "chained.txt").exists())
        self.assertFalse(stem.with_suffix(".dmp").exists())

    def test_owned_fault_survives_replaced_filter_without_claiming_all_faults_are_fatal(self):
        for mode in ("replaced-filter", "handled-owned"):
            with self.subTest(mode=mode):
                directory, stem, process = self.run_case(mode)
                self.assertEqual(process.returncode == 0, mode == "handled-owned")
                self.assertIn("first-chance exception", stem.with_suffix(".crash.txt").read_text())
                self.assertEqual(self.dump_exception(stem)[0], 0xc0000005)
                if mode == "replaced-filter":
                    self.assertTrue((directory / "chained.txt").exists())

    def test_normal_exit_and_handled_exceptions_do_not_report_crashes(self):
        for mode in ("normal", "handled"):
            with self.subTest(mode=mode):
                _, stem, process = self.run_case(mode)
                self.assertEqual(process.returncode, 0, process.stderr)
                self.assertEqual(stem.with_suffix(".crash.txt").read_bytes(), b"")
                self.assertFalse(stem.with_suffix(".dmp").exists())

    def test_removed_range_is_no_longer_observed(self):
        directory, stem, process = self.run_case("removed-range")
        self.assertNotEqual(process.returncode, 0)
        self.assertTrue((directory / "chained.txt").exists())
        self.assertEqual(stem.with_suffix(".crash.txt").read_bytes(), b"")
        self.assertFalse(stem.with_suffix(".dmp").exists())


if __name__ == "__main__":
    unittest.main()
