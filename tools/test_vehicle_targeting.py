"""Start passenger target hooks through the real loader on every local build.

Run after building the release loader, patch-inventory host, Core and feature.
The game modules are mapped without executing their entry points.
"""
import unittest
import hashlib
import json
import os

import builds
import patch_inventory as inventory
import stage
from pe import Image
from rtti import Rtti

PLUGIN = "defiance.vehicle-special-fire"
# The plugin ships disabled; these runs turn it on.
inventory.SETTINGS.setdefault(PLUGIN, {"enabled": "true"})
# The two Gunner-walking AI helpers the plugin hooks, by their function
# prologues (the plugin's own signatures are shorter prefixes of these).
HELPERS = {
    "candidate_query": bytes.fromhex(
        "48 89 5c 24 08 48 89 74 24 18 55 57 41 54 41 56 41 57 "
        "48 8b ec 48 83 ec 30 45 0f b6 e0 4c 8b f9 "
        "48 8b 02 48 8b ca ff 90 b0 00 00 00 48 8b 70 28 48 85 f6"
    ),
    "capable": bytes.fromhex(
        "48 89 5c 24 08 48 89 6c 24 10 48 89 74 24 18 57 48 83 ec 20 "
        "48 8b 01 48 8b ea ff 90 b0 00 00 00 48 8b 70 28 48 85 f6"
    ),
}


def locate(rtti, class_name, slot):
    """The one method in `slot` of `class_name`'s vtables."""
    tables = {table for name, _, cols in rtti.find(class_name)
              if name == class_name for _, entries in cols for table in entries}
    addresses = {methods[slot] for table in tables
                 if len(methods := rtti.methods(table, slot + 1)) > slot}
    if len(addresses) != 1:
        raise AssertionError(f"{class_name} vf{slot}: expected one method, got {addresses}")
    return addresses.pop()


def locate_helper(image, field):
    """The one function start that begins with `field`'s prologue."""
    prefix = HELPERS[field]
    matches = [start for start, _ in image.functions
               if image.read(start, len(prefix)) == prefix]
    if len(matches) != 1:
        raise AssertionError(f"{field}: expected one function with the prologue, got {matches}")
    return matches[0]


class PassengerTargetStartup(unittest.TestCase):
    def test_installed_moving_grenades_start_and_reload(self):
        game = os.environ.get("DEFIANCE_GAME_DIR")
        if not game:
            self.skipTest("no local game installation")
        directory = stage.plugin_directory(stage.bin_directory(game))
        installed = {}
        for manifest in directory.glob("*.plugin.json"):
            metadata = json.loads(manifest.read_text(encoding="utf-8"))
            dll = directory / metadata["dll"]
            if dll.is_file():
                installed[metadata["id"]] = (dll, manifest)
        grenades = "defiance.moving-grenades"
        actions = "defiance.moving-actions"
        if grenades not in installed or actions not in installed:
            self.skipTest("moving-grenades and moving-actions are not installed")
        build = next((b for b in builds.supported()
                      if b.name == "steam-2026-09-25" and b.present), None)
        if build is None:
            self.skipTest("Steam 2026-09-25 reference DLLs unavailable")
        found = inventory.plugins()
        for plugin in (grenades, actions):
            found[plugin] = installed[plugin]
        ids = inventory.closure(found, PLUGIN) | inventory.closure(found, grenades)
        dll, manifest = installed[grenades]
        metadata = json.loads(manifest.read_text(encoding="utf-8"))
        print(f"Installed moving-grenades {metadata['version']}: "
              f"{hashlib.sha256(dll.read_bytes()).hexdigest()}")
        for reverse in (False, True):
            with self.subTest(reverse=reverse):
                startup = inventory.run(build, found, sorted(ids), reverse)
                for plugin in (PLUGIN, grenades, actions):
                    self.assertEqual(startup["states"].get(plugin), "Active",
                                     "\n".join(startup["errors"]))
                reloaded = inventory.run(build, found, sorted(ids), reverse, reload=True)
                for plugin in (PLUGIN, grenades, actions):
                    self.assertEqual(reloaded["states"].get(plugin), "Active",
                                     "\n".join(reloaded["errors"]))
                    self.assertEqual(inventory.own(reloaded, plugin), inventory.own(startup, plugin))
                self.assertFalse(startup["errors"], "\n".join(startup["errors"]))
                self.assertFalse(reloaded["errors"], "\n".join(reloaded["errors"]))

    def test_native_tick_preserves_both_float_timing_arguments(self):
        tested = 0
        for build in builds.supported():
            if not build.present:
                continue
            with self.subTest(build=build.name):
                tested += 1
                image = Image(build.require().logic)
                tick = locate(Rtti(image), ".?AVGunner@Leonardo@@", 5)
                code = list(image.md.disasm(image.read(tick, 0x300), tick))
                ops = [(ins.mnemonic, ins.op_str) for ins in code]
                # Five pushes make rbp+0x50 the fifth incoming stack argument.
                frame = ops.index(("mov", "rbp, rsp"))
                self.assertEqual(sum(op == "push" for op, _ in ops[:frame]), 5)
                self.assertIn(("movaps", "xmm6, xmm3"), ops)
                self.assertIn(("movss", "xmm7, dword ptr [rbp + 0x50]"), ops)
                dispatches = [i for i, ins in enumerate(code)
                              if ins.mnemonic == "call"
                              and ins.op_str == "qword ptr [rax + 0x50]"]
                self.assertGreaterEqual(len(dispatches), 2)
                for i in dispatches[:2]:
                    args = ops[max(0, i - 6):i]
                    self.assertIn(("movaps", "xmm3, xmm6"), args)
                    self.assertIn(("movss", "dword ptr [rsp + 0x20], xmm7"), args)
                print(f"{build.name}: Gunner tick passes float current time and dt")
        self.assertGreater(tested, 0, "no local game DLLs to test")

    def test_supported_builds_and_start_orders(self):
        found = inventory.plugins()
        self.assertIn(PLUGIN, found, "build the vehicle special-fire plugin first")
        tested = 0
        for build in builds.supported():
            if not build.present:
                continue
            with self.subTest(build=build.name):
                tested += 1
                solo = inventory.run(build, found, inventory.closure(found, PLUGIN))
                self.assertEqual(solo["states"].get(PLUGIN), "Active", "\n".join(solo["errors"]))
                spans = inventory.own(solo, PLUGIN)
                image = Image(build.require().logic)
                self.assertEqual(len(spans), 17)
                self.assertEqual(sum(module == "game" for module, *_ in spans), 1)
                for field in HELPERS:
                    rva = locate_helper(image, field)
                    with self.subTest(build=build.name, hook=field):
                        matches = [span for span in spans if span[0] == "logic" and span[1] == rva]
                        self.assertEqual(len(matches), 1,
                                         f"{field} must be claimed exactly once at logic.dll+{rva:#x}")
                        self.assertEqual(matches[0][3], "function", f"{field} must use an entry detour")
                        self.assertGreaterEqual(matches[0][2], 5,
                                                f"{field} must cover a complete entry detour")
                for reverse in (False, True):
                    full = inventory.run(build, found, sorted(found), reverse)
                    self.assertEqual(full["states"].get(PLUGIN), "Active", "\n".join(full["errors"]))
                    self.assertEqual(inventory.own(full, PLUGIN), spans)
                for label, ids, expected_spans in (
                    ("alone", inventory.closure(found, PLUGIN), spans),
                    ("with all plugins", sorted(found), inventory.own(full, PLUGIN)),
                ):
                    with self.subTest(build=build.name, reload=label):
                        reloaded = inventory.run(build, found, ids, reload=True)
                        self.assertEqual(reloaded["states"].get(PLUGIN), "Active",
                                         "\n".join(reloaded["errors"]))
                        self.assertEqual(inventory.own(reloaded, PLUGIN), expected_spans)
                print(f"{build.name}: passenger hooks active, same 17 spans in both orders")
        self.assertGreater(tested, 0, "no local game DLLs to test")


if __name__ == "__main__":
    unittest.main()
