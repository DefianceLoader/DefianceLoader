"""Check the weapon-drop plugin's generated image guards for every build.

Pass the parity-test DLL as the first argument. This test maps local PE images
as the loader does and calls only the plugin's validation export; it does not
execute code from a game image.
"""
import ctypes as C
import hashlib
import pathlib
import re
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

import builds
import symbols
from pe import Image
import weapon_drop_bindings as bindings


PLUGIN_PATH = pathlib.Path(sys.argv.pop(1)).resolve() if len(sys.argv) > 1 else None
SITES_PATH = ROOT / "plugins" / "weapon-drops" / "src" / "sites.rs"
SHA_PATTERN = re.compile(r'\bsha:\s*"([0-9a-f]{64})"')
GUARD_PATTERN = re.compile(
    r"Guard\s*\{\s*rva:\s*(0x[0-9a-fA-F]+),\s*"
    r"bytes:\s*(0x[0-9a-fA-F]+),\s*sha:\s*\"[0-9a-f]+\",?\s*\}"
)


def generated_builds():
    """[(logic.dll sha256, [(rva, size)])] in the order the runtime indexes."""
    text = SITES_PATH.read_text(encoding="utf-8")
    result = []
    for block in text.split("Build {")[1:]:
        result.append((
            SHA_PATTERN.search(block).group(1),
            [(int(rva, 16), int(size, 16)) for rva, size in GUARD_PATTERN.findall(block)],
        ))
    return result


def mapped_image(image):
    """Return the PE image with sections placed at their RVAs and zero-fill."""
    pe = image.pe
    size = pe.OPTIONAL_HEADER.SizeOfImage
    result = bytearray(size)
    header_size = min(pe.OPTIONAL_HEADER.SizeOfHeaders, len(image.data), size)
    result[:header_size] = image.data[:header_size]
    for section in pe.sections:
        start = section.VirtualAddress
        if start >= size or not section.SizeOfRawData:
            continue
        end = min(size, start + section.SizeOfRawData)
        raw_start = section.PointerToRawData
        raw_end = raw_start + (end - start)
        result[start:end] = image.data[raw_start:raw_end]
    return result


class Plugin:
    def __init__(self, path):
        self.dll = C.CDLL(str(path))
        self.validate = self.dll.weapon_drops_test_validate
        self.validate.argtypes = [C.c_void_p, C.c_size_t, C.c_size_t]
        self.validate.restype = C.c_int

    def check(self, image, build=0, size=None):
        buffer = (C.c_ubyte * len(image)).from_buffer(image)
        return self.validate(C.addressof(buffer), len(image) if size is None else size, build)


class WeaponDropValidationTests(unittest.TestCase):
    def setUp(self):
        if PLUGIN_PATH is None:
            self.fail("pass the weapon-drops parity-test DLL path")
        if not PLUGIN_PATH.is_file():
            self.fail(f"parity-test DLL is missing: {PLUGIN_PATH.name}")
        self.plugin = Plugin(PLUGIN_PATH)
        self.generated = generated_builds()

    def supported(self):
        """Yield (index, build, symbols, mapped image) for each local supported build."""
        bound = symbols.builds_with(bindings.CONSUMER, builds.supported())
        self.assertEqual(len(bound), len(self.generated), "sites.rs is stale; rerun the generator")
        local = 0
        for index, build in enumerate(bound):
            if not build.logic.is_file():
                continue
            image = Image(build.logic)
            self.assertEqual(hashlib.sha256(image.data).hexdigest(), self.generated[index][0],
                             f"{build.name}: sites.rs names another logic.dll")
            local += 1
            yield index, build, symbols.section(build, bindings.CONSUMER), mapped_image(image)
        if not local:
            self.skipTest("no supported logic.dll reference is local")

    def test_stock_images_and_every_generated_guard(self):
        for index, build, rva, stock in self.supported():
            guards = self.generated[index][1]
            self.assertTrue(guards, f"{build.name}: generated sites.rs has no function guards")
            self.assertTrue(
                {start for start, _size in guards}.issuperset(rva[name] for name in bindings.FUNCTIONS),
                f"{build.name}: generated sites.rs omits a weapon_drop_bindings.FUNCTIONS entry",
            )
            self.assertEqual(self.plugin.check(stock, index), 0, f"{build.name}: stock image rejected")
            for start, size in guards:
                with self.subTest(build=build.name, guard=f"{start:#x}+{size:#x}"):
                    self.assertGreater(size, 0)
                    self.assertLessEqual(start + size, len(stock))
                    stock[start + size // 2] ^= 1
                    try:
                        self.assertNotEqual(self.plugin.check(stock, index), 0)
                    finally:
                        stock[start + size // 2] ^= 1

    def test_hook_sites_and_invalid_images_are_rejected(self):
        for index, build, rva, stock in self.supported():
            for site, _function, target in bindings.CALL_SITES:
                self.assertEqual(
                    bindings.call_target(rva[site], bytes(stock[rva[site]:rva[site] + 5])), rva[target])
            sites = ("death_call", "collect", "collect_drop", "collect_add", "squad cache rebuild",
                     *bindings.RESERVE_SITES)
            for site in sites:
                with self.subTest(build=build.name, site=site):
                    stock[rva[site]] ^= 1
                    try:
                        self.assertNotEqual(self.plugin.check(stock, index), 0,
                                            "mutated hook site was accepted")
                    finally:
                        stock[rva[site]] ^= 1
            for site, before in bindings.RESERVE_SITES.items():
                self.assertEqual(bytes(stock[rva[site]:rva[site] + len(before)]), before)

            self.assertNotEqual(self.plugin.validate(None, len(stock), index), 0,
                                "null image was accepted")
            last_guard_end = max(start + size for start, size in self.generated[index][1])
            self.assertNotEqual(self.plugin.check(stock, index, size=last_guard_end - 1), 0,
                                "truncated image was accepted")
            self.assertNotEqual(self.plugin.check(stock, len(self.generated)), 0,
                                "invalid build index was accepted")

    def test_each_image_matches_only_its_own_binding(self):
        local = [build for build in builds.on_disk() if build.logic.is_file()]
        if not local:
            self.skipTest("no local logic.dll reference builds")
        shas = [sha for sha, _guards in self.generated]
        for build in local:
            image = Image(build.logic)
            digest = hashlib.sha256(image.data).hexdigest()
            mapped = mapped_image(image)
            for index in range(len(self.generated)):
                with self.subTest(build=build.name, index=index):
                    accepted = self.plugin.check(mapped, index) == 0
                    self.assertEqual(accepted, digest == shas[index],
                                     f"{build.name} against build index {index}")

    def test_generator_call_target_rejects_non_call_opcode(self):
        for _index, _build, rva, stock in self.supported():
            call = bytes(stock[rva["death_call"]:rva["death_call"] + 5])
            with self.assertRaises(ValueError):
                bindings.call_target(rva["death_call"], b"\x90" + call[1:])


if __name__ == "__main__":
    unittest.main()
