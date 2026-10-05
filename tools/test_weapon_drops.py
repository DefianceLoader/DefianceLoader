"""Check that the weapon-drop plugin resolves its sites on every local build.

Pass the parity-test DLL as the first argument. This test maps local PE images
as the loader does and calls only the plugin's resolve export; it does not
execute code from a game image. The September 2026 builds must resolve and the
December 2025 builds, which the plugin does not support, must be refused.
"""
import ctypes as C
import pathlib
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

import builds
from pe import Image


PLUGIN_PATH = pathlib.Path(sys.argv.pop(1)).resolve() if len(sys.argv) > 1 else None


def supported(build):
    return build.date >= "2026-09"


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
        self.resolve = self.dll.weapon_drops_test_resolve
        self.resolve.argtypes = [C.c_void_p, C.c_size_t, C.c_size_t]
        self.resolve.restype = C.c_int

    def check(self, image, base, size=None):
        buffer = (C.c_ubyte * len(image)).from_buffer(image)
        return self.resolve(C.addressof(buffer), len(image) if size is None else size, base)


class WeaponDropResolveTests(unittest.TestCase):
    def setUp(self):
        if PLUGIN_PATH is None:
            self.fail("pass the weapon-drops parity-test DLL path")
        if not PLUGIN_PATH.is_file():
            self.fail(f"parity-test DLL is missing: {PLUGIN_PATH.name}")
        self.plugin = Plugin(PLUGIN_PATH)

    def local(self):
        """Yield (build, image base, mapped image) for each local logic.dll."""
        found = [build for build in builds.on_disk() if build.logic.is_file()]
        if not found:
            self.skipTest("no local logic.dll reference builds")
        for build in found:
            image = Image(build.logic)
            yield build, image.pe.OPTIONAL_HEADER.ImageBase, mapped_image(image)

    def test_supported_builds_resolve_and_others_are_refused(self):
        for build, base, image in self.local():
            with self.subTest(build=build.name):
                self.assertEqual(self.plugin.check(image, base) == 0, supported(build))

    def test_null_and_truncated_images_are_refused(self):
        for build, base, image in self.local():
            if not supported(build):
                continue
            self.assertNotEqual(self.plugin.resolve(None, len(image), base), 0)
            self.assertNotEqual(self.plugin.check(image, base, size=len(image) // 2), 0)
            self.assertNotEqual(self.plugin.check(image, base, size=0), 0)


if __name__ == "__main__":
    unittest.main()
