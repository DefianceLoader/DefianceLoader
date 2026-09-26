"""The plugin binding generators reproduce their tracked tables from the local
game DLLs: each walks the supported builds (tools/builds.py), locating each
through its base. A table that comes out different is restored and reported.
Needs every supported build's DLLs."""
import pathlib, subprocess, sys, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import builds

GENERATORS = {
    "tools/regroup_bindings.py": "plugins/regroup/src/bindings.rs",
    "tools/squad_scroll_bindings.py": "plugins/squad-management-scroll/src/sites.rs",
    "tools/ammo_menu_sites.py": "plugins/expanded-ammo-menu/src/sites.rs",
}


class BindingTests(unittest.TestCase):
    def test_generators_reproduce_their_tables(self):
        missing = [b.name for b in builds.supported() if not b.present]
        if missing:
            self.skipTest(f"builds absent: {', '.join(missing)}")
        for script, table in GENERATORS.items():
            with self.subTest(generator=script):
                path = builds.ROOT / table
                before = path.read_bytes()
                run = subprocess.run([sys.executable, script], cwd=builds.ROOT, capture_output=True, text=True)
                after = path.read_bytes()
                if after != before:
                    path.write_bytes(before)
                self.assertEqual(run.returncode, 0, run.stderr[-2000:])
                self.assertTrue(after == before, f"{table} changed; regenerate and review it")


if __name__ == "__main__":
    unittest.main()
