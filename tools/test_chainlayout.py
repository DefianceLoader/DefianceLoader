"""tools/chainlayout.py: every layout derived from a base other than the
reference re-derives from that base as tracked. Needs the builds' DLLs; a
build that is absent is skipped."""
import pathlib, sys, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import builds, chainlayout, json


def derived():
    """Layouts whose base is another layout (the reference-based first step
    was derived by hand before the chain existed)."""
    return [b for b in builds.with_layouts() if b.base and b.base.layout]


class ChainTests(unittest.TestCase):
    def test_payload_operands_are_found(self):
        keys = chainlayout.payload_keys()
        self.assertTrue(any(k.startswith("call|rax|") for k in keys))
        self.assertFalse(any(k.split("|")[1] in ("rsp", "rbp") for k in keys))

    def test_derived_layouts_re_derive_from_their_base(self):
        for b in derived():
            with self.subTest(build=b.name):
                if not (b.present and b.base.present):
                    self.skipTest(f"{b.name} or its base {b.base.name} is absent")
                profile, problems = chainlayout.draft(b.name)
                self.assertEqual(problems, [])
                tracked = json.loads(b.layout.read_text(encoding="utf-8"))
                for field in ("base", "logic_layout", "game_layout", "logic_symbols", "game_symbols"):
                    self.assertEqual(profile[field], tracked[field], field)

    def test_derived_layouts_need_no_site_overrides(self):
        # tools/variant.py finds a derived build's sites through its base
        for b in derived():
            with self.subTest(build=b.name):
                tracked = json.loads(b.layout.read_text(encoding="utf-8"))
                self.assertEqual(tracked.get("logic_sites", {}), {})
                self.assertEqual(tracked.get("game_sites", {}), {})


if __name__ == "__main__":
    unittest.main()
