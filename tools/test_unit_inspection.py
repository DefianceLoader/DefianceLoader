"""plugins/unit-inspection's patterns against every supported build's game.dll:
each site once in the 2026 builds, with the relation queries where the plugin
checks them; the relation label's function absent from the 2025 builds, so
the plugin changes nothing there. Needs the builds' DLLs; an absent one is
skipped. Whether its writes overlap another plugin's, and the order it must
start in after the expanded ammo menu, are tools/patch_inventory.py's."""
import hashlib, pathlib, re, sys, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import builds, pefile

SOURCE = builds.ROOT / "plugins/unit-inspection/src/lib.rs"
SUPPORTED_2026 = ("gog-2026-09-14", "steam-2026-09-22", "gog-2026-09-25", "steam-2026-09-25")


def constants():
    """{NAME: pattern text} for the `pub const NAME: &str = "..." \\ "...";`s."""
    text = SOURCE.read_text(encoding="utf-8")
    out = {}
    for name, body in re.findall(r'pub const (\w+_PATTERN): &str =\s*((?:"[^"]*"\s*)+);', text):
        out[name] = " ".join(re.findall(r'"([^"]*)"', body)).replace("\\", " ")
    return out


def expanded_ammo_menu_writes():
    """{game.dll sha: [(start, length), ...]} of what plugins/expanded-ammo-menu
    may write, from its generated tables. Of its combined-view functions it
    hooks only the first (the click handler); the others it checks and calls."""
    text = (builds.ROOT / "plugins/expanded-ammo-menu/src/sites.rs").read_text(encoding="utf-8")
    out = {}
    for block in text.split("Build {")[1:]:
        sha = re.search(r'sha: "([0-9a-f]{64})"', block).group(1)
        spans = [(int(rva, 16) + int(field), int(width)) for rva, field, width in re.findall(
            r"rva: (0x[0-9a-f]+),\s*before: &\[[^\]]*\],\s*field: (\d+),\s*width: (\d+)", block)]
        for key in ("redraw", "layout"):
            rva = int(re.search(rf"\b{key}: (0x[0-9a-f]+)", block).group(1), 16)
            before = re.search(rf"\b{key}_before: &\[([^\]]*)\]", block).group(1)
            spans.append((rva, len(before.split(",")) - before.strip().endswith(",")))
        combined = block[block.index("combined: &["):]
        rva, before = re.findall(r"\(\s*(0x[0-9a-f]+),\s*&\[([^\]]*)\]", combined)[0]
        spans.append((int(rva, 16), len([b for b in before.split(",") if b.strip()])))
        out[sha] = spans
    return out


def regex(pattern):
    return re.compile(b"".join(b"." if p == "??" else re.escape(bytes([int(p, 16)])) for p in pattern.split()), re.S)


def image(build):
    return pefile.PE(str(build.game), fast_load=True).get_memory_mapped_image()


class PatternTests(unittest.TestCase):
    def test_every_site_once_in_the_2026_builds(self):
        patterns = constants()
        self.assertEqual(sorted(patterns), ["AMMO_PATTERN", "CLICK_PATTERN", "FILL_PATTERN", "LABEL_PATTERN", "SQUAD_PATTERN"])
        for name in SUPPORTED_2026:
            build = builds.build(name)
            if not build.present:
                self.skipTest(f"{name} is absent")
            data = image(build)
            for key, pattern in patterns.items():
                with self.subTest(build=name, pattern=key):
                    self.assertEqual(len(regex(pattern).findall(data)), 1)
            # the relation queries the plugin asks, where it checks them
            label = regex(patterns["LABEL_PATTERN"]).search(data).start()
            for at, expected in [(0x18a, "4c8b822806"), (0x1be, "ff904006"), (0x1de, "ff903006"), (0x1fe, "ff903806")]:
                with self.subTest(build=name, at=hex(at)):
                    self.assertEqual(data[label + at:label + at + len(expected) // 2].hex(), expected)

    def test_the_patterns_survive_the_expanded_ammo_menu(self):
        # It rewrites operand fields (Site: rva + field, width) and takes over
        # function starts (redraw, layout, the combined entries: rva, bytes);
        # every byte it may change, flipped here, must be outside the
        # patterns or a wildcard in them.
        patterns = constants()
        tables = expanded_ammo_menu_writes()
        for name in SUPPORTED_2026:
            build = builds.build(name)
            if not build.present:
                self.skipTest(f"{name} is absent")
            data = bytearray(image(build))
            sha = hashlib.sha256(build.game.read_bytes()).hexdigest()
            self.assertIn(sha, tables, f"the expanded ammo menu has no table for {name}")
            for start, length in tables[sha]:
                for at in range(start, start + length):
                    data[at] ^= 0xff
            for key, pattern in patterns.items():
                with self.subTest(build=name, pattern=key):
                    self.assertEqual(len(regex(pattern).findall(bytes(data))), 1)

    def test_the_2025_builds_are_left_alone(self):
        pattern = constants()["LABEL_PATTERN"]
        for name in builds.RELEASE_COPIES:
            build = builds.build(name)
            if not build.present:
                continue
            with self.subTest(build=name):
                self.assertIsNone(regex(pattern).search(image(build)))


if __name__ == "__main__":
    unittest.main()
