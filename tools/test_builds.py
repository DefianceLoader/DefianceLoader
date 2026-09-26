"""tools/builds.py: build names, where their DLLs go, and what the repository
expects of them. Runs without any game DLL present."""
import pathlib, sys, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import builds


class BuildTests(unittest.TestCase):
    def test_a_name_is_its_store_and_date_and_folder(self):
        b = builds.build("steam-2026-09-22")
        self.assertEqual((b.store, b.date), ("steam", "2026-09-22"))
        self.assertEqual(b.logic, builds.BIN / "steam" / "2026-09-22" / "logic.dll")
        self.assertEqual(b.game, builds.BIN / "steam" / "2026-09-22" / "game.dll")
        self.assertEqual(builds.relative(b.game), "bin/steam/2026-09-22/game.dll")

    def test_names_that_are_not_builds_are_refused(self):
        for name in ("gog", "epic-2026-09-14", "gog-26-09-14", "gog_2026-09-14"):
            with self.subTest(name=name), self.assertRaises(ValueError):
                builds.build(name)

    def test_every_layout_is_a_build_name(self):
        for path in builds.LAYOUTS.glob("*.json"):
            with self.subTest(layout=path.name):
                self.assertEqual(builds.build(path.stem).layout, path)

    def test_the_reference_release_comes_first_and_is_supported(self):
        supported = [b.name for b in builds.supported()]
        self.assertEqual(supported[0], builds.REFERENCE)
        self.assertEqual(tuple(supported[:2]), builds.RELEASE_COPIES)
        self.assertIn(builds.reference(), builds.supported())

    def test_present_builds_match_their_layouts(self):
        for b in builds.with_layouts():
            if b.present:
                with self.subTest(build=b.name):
                    self.assertTrue(b.hashes_match_layout())

    def test_each_build_derives_from_its_neighbour(self):
        base = lambda name: builds.build(name).base
        self.assertIsNone(base(builds.REFERENCE))
        self.assertEqual(base("steam-2025-12-23"), builds.reference())
        self.assertEqual(base("gog-2026-09-14").name, "gog-2025-12-23")
        # Steam from the GOG build the same update shipped as, not the older Steam
        self.assertEqual(base("steam-2026-09-22").name, "gog-2026-09-14")
        self.assertEqual([b.name for b in builds.build("steam-2026-09-22").lineage()],
                         ["gog-2025-12-23", "gog-2026-09-14", "steam-2026-09-22"])

    def test_a_new_build_is_proposed_its_neighbour(self):
        # the next GOG update follows the latest GOG build; its Steam twin follows it
        latest_gog = max((b for b in builds.supported() if b.store == "gog"), key=lambda b: b.date)
        self.assertEqual(builds.proposed_base("gog-2099-01-01"), latest_gog)
        self.assertEqual(builds.proposed_base("steam-2099-01-01"), latest_gog)
        self.assertEqual(builds.proposed_base("gog-2025-12-24").name, "gog-2025-12-23")

    def test_every_layout_names_a_supported_base(self):
        supported = builds.supported()
        for b in builds.with_layouts():
            with self.subTest(build=b.name):
                self.assertIn(b.base, supported)
                self.assertLess(b.base.date, b.date) if b.store == "gog" else self.assertLessEqual(b.base.date, b.date)

    def test_a_missing_build_says_which_files(self):
        b = builds.build("gog-1999-01-01")
        self.assertFalse(b.present)
        with self.assertRaises(SystemExit) as refused:
            b.require()
        self.assertIn("bin/gog/1999-01-01/logic.dll", str(refused.exception))


if __name__ == "__main__":
    unittest.main()
