"""Host-level ABI tests for the paired vehicle braking tutorial DLLs.

The DLLs are inputs: this test never builds either example. Pass both release
DLLs explicitly so the caller can stamp the check against those artifacts.
"""
import ctypes as C
import json
import pathlib
import re
import struct
import sys
import unittest

RUST_DLL = CPP_DLL = RUST_SOURCE = CPP_SOURCE = RUST_MANIFEST = CPP_MANIFEST = None


class Api(C.Structure):
    _fields_ = [("abi_version", C.c_uint32), ("reserved", C.c_uint32)] + [
        (name, C.c_void_p) for name in (
            "log", "module_base", "module_size", "find_pattern", "find_pattern_at",
            "hook", "hook_exact", "hook_call", "unhook", "rtti_method",
            "vtable_slot", "config_get", "patch_bytes")]


class Plugin(C.Structure):
    _fields_ = [("abi_version", C.c_uint32), ("name", C.c_char_p),
                ("version", C.c_char_p), ("init", C.c_void_p), ("stop", C.c_void_p)]


class ContractEntry(C.Structure):
    _fields_ = [("module", C.c_char_p), ("rva", C.c_size_t), ("kind", C.c_uint32),
                ("before", C.POINTER(C.c_uint8)), ("before_len", C.c_size_t),
                ("after", C.POINTER(C.c_uint8)), ("after_len", C.c_size_t)]


class Contract(C.Structure):
    _fields_ = [("version", C.c_uint32), ("size", C.c_uint32),
                ("entries", C.POINTER(ContractEntry)), ("count", C.c_size_t)]


class Harness:
    """Invoke only the documented exports, with loader-like ABI callbacks."""
    def __init__(self, path, ident, source, image=None):
        self.dll = C.CDLL(str(path))
        self.entry = self.dll.defiance_plugin
        self.entry.argtypes, self.entry.restype = [], C.POINTER(Plugin)
        self.contract_fn = self.dll.defiance_patch_contract_v1
        self.contract_fn.argtypes, self.contract_fn.restype = [C.POINTER(Api)], C.POINTER(Contract)
        self.ident = ident.encode()
        self.setting = None
        self.image = image if image is not None else C.create_string_buffer(4096)
        self.patterns = []
        self.logs = []
        self.scan_mode = "found"
        self.hooks = []
        self.fail_hook = False
        self.delay_trampoline = False
        self.original_slots = []
        self.original_calls = 0
        self.stock_values = (0.25, 2.5, 1.5, 0.0, 2.0)
        self.original = C.CFUNCTYPE(None, C.c_void_p, C.c_void_p)(self._original)
        self.callbacks = []
        src = source.read_text(encoding="utf-8")
        start = src.find("ARRIVAL_PATTERN")
        end = src.find(";", start)
        literal = src[start:end + 1] if start >= 0 and end >= 0 else ""
        chunks = re.findall(r'["\']([0-9a-fA-F?]+)["\']', literal)
        if not chunks:
            raise ValueError(f"ARRIVAL_PATTERN not found in {source}")
        self.pattern = "".join(chunks)
        self.pattern_regex = re.compile(b"(?=(" + b"".join(
            b"." if self.pattern[i:i + 2] == "??" else re.escape(bytes((int(self.pattern[i:i + 2], 16),)))
            for i in range(0, len(self.pattern), 2)) + b"))", re.DOTALL)
        self._seed_pattern(128)
        self.api = self._api()

    def _seed_pattern(self, offset):
        if C.sizeof(self.image) != 4096:
            return
        values = [0x90 if self.pattern[i:i + 2] == "??" else int(self.pattern[i:i + 2], 16)
                  for i in range(0, len(self.pattern), 2)]
        C.memmove(C.addressof(self.image) + offset, bytes(values), len(values))
        if self.scan_mode == "duplicate":
            C.memmove(C.addressof(self.image) + offset + 512, bytes(values), len(values))

    def _keep(self, typ, fn):
        cb = typ(fn)
        self.callbacks.append(cb)
        return C.cast(cb, C.c_void_p).value

    def _original(self, chassis, record):
        self.original_calls += 1
        for offset, value in zip((0x1d4, 0x1d0, 0x200, 0x204, 0x208), self.stock_values):
            C.c_float.from_address(record + offset).value = value

    def invoke_detour(self, mode=1, values=(0.25, 2.5, 1.5, 0.0, 2.0)):
        chassis, record = C.create_string_buffer(0x480), C.create_string_buffer(0x2f0)
        C.c_uint32.from_buffer(chassis, 0x320).value = mode
        C.memset(record, 0x5a, C.sizeof(record))
        self.stock_values = values
        before = self.original_calls
        detour = C.CFUNCTYPE(None, C.c_void_p, C.c_void_p)(self.hooks[-1][1])
        detour(C.addressof(chassis), C.addressof(record))
        self.assert_original_once(before)
        outputs = tuple(C.c_float.from_buffer(record, offset).value
                        for offset in (0x1d4, 0x1d0, 0x200, 0x204, 0x208))
        return outputs, bytes(record), bytes(chassis)

    def assert_original_once(self, before):
        if self.original_calls != before + 1:
            raise AssertionError(f"original callback ran {self.original_calls - before} times")

    def original_snapshot(self, mode, values):
        chassis, record = C.create_string_buffer(0x480), C.create_string_buffer(0x2f0)
        C.c_uint32.from_buffer(chassis, 0x320).value = mode
        C.memset(record, 0x5a, C.sizeof(record))
        self.stock_values = values
        before = self.original_calls
        self.original(C.addressof(chassis), C.addressof(record))
        self.assert_original_once(before)
        return bytes(record), bytes(chassis)

    def _api(self):
        def log(level, message): self.logs.append(message.decode())
        def module_base(name): return C.addressof(self.image)
        def module_size(base): return C.sizeof(self.image)
        def find(base, size, pattern):
            raw = pattern.decode()
            self.patterns.append(raw)
            if self.scan_mode == "missing": return None
            if self.scan_mode == "force-site": return C.addressof(self.image) + 128
            # Match the signature against bytes in the synthetic module, with
            # ?? interpreted as a wildcard. A doubled signature is ambiguous.
            data = bytes(self.image)
            hits = [m.start() for m in self.pattern_regex.finditer(data)]
            return C.addressof(self.image) + hits[0] if len(hits) == 1 else None
        def hook(target, detour, count, out):
            self.hooks.append((target, detour, count))
            if self.fail_hook: return -1
            if self.delay_trampoline:
                self.original_slots.append(out)
            else:
                C.cast(out, C.POINTER(C.c_void_p))[0] = C.cast(self.original, C.c_void_p).value
            return 0
        def config(ident, key):
            if ident != self.ident or key != b"braking_window_percent" or self.setting is None:
                return None
            self._setting_buffer = C.create_string_buffer(str(self.setting).encode() + b"\0")
            return C.addressof(self._setting_buffer)
        funcs = [
            (C.CFUNCTYPE(None, C.c_uint32, C.c_char_p), log),
            (C.CFUNCTYPE(C.c_void_p, C.c_char_p), module_base),
            (C.CFUNCTYPE(C.c_size_t, C.c_void_p), module_size),
            (C.CFUNCTYPE(C.c_void_p, C.c_void_p, C.c_size_t, C.c_char_p), find),
            (C.CFUNCTYPE(C.c_void_p, C.c_void_p, C.c_size_t, C.c_char_p, C.c_size_t), lambda *_: None),
            (C.CFUNCTYPE(C.c_int, C.c_void_p, C.c_void_p, C.POINTER(C.c_void_p)), lambda *_: -1),
            (C.CFUNCTYPE(C.c_int, C.c_void_p, C.c_void_p, C.c_size_t, C.POINTER(C.c_void_p)), hook),
            (C.CFUNCTYPE(C.c_int, C.c_void_p, C.c_void_p, C.POINTER(C.c_void_p)), lambda *_: -1),
            (C.CFUNCTYPE(C.c_int, C.c_void_p), lambda *_: 0),
            (C.CFUNCTYPE(C.c_void_p, C.c_char_p, C.c_char_p), lambda *_: None),
            (C.CFUNCTYPE(C.c_void_p, C.c_char_p, C.c_size_t), lambda *_: None),
            (C.CFUNCTYPE(C.c_void_p, C.c_char_p, C.c_char_p), config),
            (C.CFUNCTYPE(C.c_int, C.c_void_p, C.c_void_p, C.c_void_p, C.c_size_t), lambda *_: -1),
        ]
        addresses = [self._keep(t, f) for t, f in funcs]
        return Api(5, 0, *addresses)

    def plugin(self):
        p = self.entry().contents
        return p, C.CFUNCTYPE(C.c_int, C.POINTER(Api))(p.init)


class BrakingExamples(unittest.TestCase):
    def assert_only_outputs_changed(self, host, mode, values, result):
        _, record, chassis = result
        baseline_record, baseline_chassis = host.original_snapshot(mode, values)
        changed = {i for offset in (0x1d0, 0x1d4, 0x200, 0x204, 0x208)
                   for i in range(offset, offset + 4)}
        self.assertEqual(bytes(v for i, v in enumerate(record) if i not in changed),
                         bytes(v for i, v in enumerate(baseline_record) if i not in changed))
        self.assertEqual(chassis, baseline_chassis)

    def test_pair(self):
        paths = [RUST_DLL, CPP_DLL]
        if any(p is None or not p.is_file() for p in paths):
            self.fail("pass --rust-dll and --cpp-dll release DLL paths")
        files = (RUST_SOURCE, CPP_SOURCE, RUST_MANIFEST, CPP_MANIFEST)
        self.assertTrue(all(p and p.is_file() for p in files), "pass all four source and manifest files")
        manifests = [json.loads(p.read_text(encoding="utf-8")) for p in (RUST_MANIFEST, CPP_MANIFEST)]
        expected_ids = ("example.vehicle-braking.rust", "example.vehicle-braking.cpp")
        source_manifests = (RUST_MANIFEST, CPP_MANIFEST)
        for index, (manifest, ident, dll) in enumerate(zip(manifests, expected_ids, paths)):
            adjacent = dll.with_suffix(".plugin.json")
            self.assertTrue(adjacent.is_file(), f"missing DLL-adjacent manifest: {adjacent}")
            self.assertEqual(json.loads(adjacent.read_text(encoding="utf-8")), manifest,
                             f"built manifest differs from {source_manifests[index]}")
            self.assertEqual(manifest["id"], ident)
            self.assertEqual(manifest["dll"], dll.name)
            self.assertEqual((manifest["version"], manifest["abi"]), ("0.1.0", 5))
            self.assertEqual(manifest["group"], "examples")
            self.assertFalse(manifest["hot_reload"])
            self.assertFalse(manifest["multiplayer_safe"])
            self.assertTrue(manifest["patch_contract"])
            self.assertEqual(manifest["depends"], [])
            peer = expected_ids[1 - index]
            self.assertEqual(set(manifest["conflicts"]), {peer, "defiance.vehicle-arrival"})
            settings = {setting["key"]: setting for setting in manifest["settings"]}
            self.assertEqual((settings["enabled"]["type"], settings["enabled"]["default"]),
                             ("bool", "true"))
            self.assertEqual(settings["braking_window_percent"]["type"], "int")
            self.assertEqual(settings["braking_window_percent"]["default"], "50")
            self.assertIn("restart", settings["braking_window_percent"]["description"].lower())
        hs = [Harness(paths[0], "example.vehicle-braking.rust", RUST_SOURCE),
              Harness(paths[1], "example.vehicle-braking.cpp", CPP_SOURCE)]
        plugins = [h.plugin() for h in hs]
        for h, (p, init) in zip(hs, plugins):
            self.assertEqual(p.abi_version, 5)
            self.assertEqual(p.name, h.ident)
            self.assertEqual(p.version, b"0.1.0")
            h.setting = "100"
            self.assertEqual(init(C.byref(h.api)), 0)
            self.assertEqual(h.patterns, [])
            self.assertEqual(h.hooks, [])
            self.assertIn("vehicle braking: stock braking; no hook installed", h.logs)
        # Contract calls are public API and must be safe after successful init.
        contracts = [h.contract_fn(C.byref(h.api)).contents for h in hs]
        for c in contracts:
            self.assertEqual((c.version, c.size, c.count), (1, C.sizeof(Contract), 0))
            self.assertFalse(bool(c.entries))

        # Exercise the active path from the same public init callback. Each
        # scanner matches the source's full signature against actual fixture bytes.
        active_contracts = []
        paired_outputs = []
        for h, (p, init) in zip(hs, plugins):
            h.delay_trampoline = True
            for percent in ("50", "75"):
                h.setting = percent
                self.assertEqual(init(C.byref(h.api)), 0)
                self.assertTrue(h.patterns)
                self.assertTrue(all(p == h.pattern for p in h.patterns))
                self.assertEqual(h.hooks[-1][2], 10)
                self.assertTrue(h.original_slots)
                # The plugin must keep the output slot alive after init returns;
                # real host planning may publish the trampoline later.
                C.cast(h.original_slots[-1], C.POINTER(C.c_void_p))[0] = C.cast(h.original, C.c_void_p).value
                contract = h.contract_fn(C.byref(h.api)).contents
                self.assertEqual(contract.count, 1)
                self.assertEqual((contract.version, contract.size), (1, C.sizeof(Contract)))
                self.assertEqual(contract.entries[0].before_len, 10)
                entry = contract.entries[0]
                self.assertEqual(entry.module.lower(), b"logic.dll")
                self.assertEqual((entry.rva, entry.kind, entry.after_len), (128, 1, 0))
                self.assertFalse(bool(entry.after))
                expected = bytes(0x90 if h.pattern[i:i + 2] == "??" else int(h.pattern[i:i + 2], 16)
                                 for i in range(0, 20, 2))
                self.assertEqual(bytes(entry.before[:10]), expected)
                active_contracts.append(bytes(entry.before[:10]))
                self.assertIn(f"vehicle braking: installed ({percent}% braking window)", h.logs)
                # The captured detour must call the delayed-published stock
                # trampoline once, tighten only a fractional destination ratio,
                # and retain every other record/chassis byte.
                before_calls = h.original_calls
                values = (0.25, 2.5, 1.5, 0.0, 2.0)
                detoured = h.invoke_detour(values=values)
                outputs, record, chassis = detoured
                expected_ratio = min(0.25 * 100 / int(percent), 1.0)
                expected = (expected_ratio, 10 * expected_ratio, 6 * expected_ratio,
                            0.0, 8 * expected_ratio)
                for got, want in zip(outputs, expected):
                    self.assertAlmostEqual(got, want, places=6)
                self.assertEqual(h.original_calls, before_calls + 1)
                self.assert_only_outputs_changed(h, 1, values, detoured)
                paired_outputs.append((outputs, record, chassis))
                near_cap = (0.95, 9.5, 5.7, 0.0, 7.6)
                capped = h.invoke_detour(values=near_cap)
                for got, want in zip(capped[0], (1.0, 10.0, 6.0, 0.0, 8.0)):
                    self.assertAlmostEqual(got, want, places=5)
                self.assert_only_outputs_changed(h, 1, near_cap, capped)
                for mode in (0, 2, 3):
                    unchanged = h.invoke_detour(mode=mode, values=values)
                    self.assertEqual(unchanged[0], values)
                    self.assert_only_outputs_changed(h, mode, values, unchanged)
                for ratio in (0.0, 1.0, -0.2, float("nan"), float("inf")):
                    invalid_ratio = (ratio, 2.5, 1.5, 0.0, 2.0)
                    unchanged, raw, chassis = h.invoke_detour(values=invalid_ratio)
                    actual = b"".join(raw[offset:offset + 4]
                                      for offset in (0x1d4, 0x1d0, 0x200, 0x204, 0x208))
                    expected = b"".join(struct.pack("<f", value) for value in invalid_ratio)
                    self.assertEqual(actual, expected)
                    self.assert_only_outputs_changed(h, 1, invalid_ratio,
                                                    (unchanged, raw, chassis))
                for bad_values in (
                    (0.25, -1.0, 1.5, 0.0, 2.0),
                    (0.25, 2.5, float("nan"), 0.0, 2.0),
                    (0.25, 2.5, 1.5, float("inf"), 2.0),
                    (0.25, float("inf"), 1.5, 0.0, 2.0),
                    (0.5, 3.4e38, 1.5, 0.0, 2.0),
                    (0.5, 5.0, 3.4e38, 0.0, 2.0),
                    (0.5, 5.0, 1.5, 0.0, 3.4e38),
                ):
                    unchanged = h.invoke_detour(values=bad_values)
                    self.assert_only_outputs_changed(h, 1, bad_values, unchanged)
                    for got, want, offset in zip(unchanged[0], bad_values,
                                                 (0x1d4, 0x1d0, 0x200, 0x204, 0x208)):
                        raw = unchanged[1]
                        self.assertEqual(raw[offset:offset + 4], struct.pack("<f", want))
        for rust_index, cpp_index in ((0, 2), (1, 3)):
            rust_pair, cpp_pair = paired_outputs[rust_index], paired_outputs[cpp_index]
            self.assertEqual(rust_pair[0], cpp_pair[0])
            self.assertEqual(rust_pair[1], cpp_pair[1])
            self.assertEqual(rust_pair[2], cpp_pair[2])
        self.assertEqual(active_contracts[0], active_contracts[2])
        self.assertEqual(active_contracts[1], active_contracts[3])

        # Null API, ABI mismatch and nonzero reserved are rejected before any
        # callback is called. A separate instance avoids static one-time state.
        for h, (p, _) in zip(hs, plugins):
            init = C.CFUNCTYPE(C.c_int, C.POINTER(Api))(p.init)
            self.assertNotEqual(init(None), 0)
            self.assertFalse(bool(h.contract_fn(None)))
            bad = Api.from_buffer_copy(h.api)
            bad.abi_version = 4
            self.assertNotEqual(init(C.byref(bad)), 0)
            self.assertFalse(bool(h.contract_fn(C.byref(bad))))
            bad = Api.from_buffer_copy(h.api)
            bad.reserved = 1
            self.assertNotEqual(init(C.byref(bad)), 0)
            self.assertFalse(bool(h.contract_fn(C.byref(bad))))

        # Required integer settings accept only their canonical decimal form.
        for h, (p, init) in zip(hs, plugins):
            for setting in (None, "", "49", "101", "+50", " 50", "50 ", "50.0", "abc"):
                h2 = Harness(paths[hs.index(h)], h.ident.decode(),
                             RUST_SOURCE if h is hs[0] else CPP_SOURCE)
                h2.setting = setting
                p2, init2 = h2.plugin()
                self.assertNotEqual(init2(C.byref(h2.api)), 0, (h.ident, setting))
                self.assertFalse(bool(h2.contract_fn(C.byref(h2.api))))
                self.assertEqual(h2.hooks, [], (h.ident, setting))
                self.assertTrue(any("braking_window_percent" in message for message in h2.logs),
                                (h.ident, setting, h2.logs))
            for mode in ("missing", "duplicate"):
                h2 = Harness(paths[hs.index(h)], h.ident.decode(),
                             RUST_SOURCE if h is hs[0] else CPP_SOURCE)
                h2.setting = "75"
                h2.scan_mode = mode
                h2._seed_pattern(128)
                p2, init2 = h2.plugin()
                self.assertNotEqual(init2(C.byref(h2.api)), 0, (h.ident, mode))
                self.assertFalse(bool(h2.contract_fn(C.byref(h2.api))))
                self.assertEqual(h2.hooks, [], (h.ident, mode))
                self.assertTrue(any("signature" in message.lower() or "arrival" in message.lower()
                                    for message in h2.logs), (h.ident, mode, h2.logs))
                self.assertTrue(any("hook" in message.lower() for message in h2.logs),
                                (h.ident, mode, h2.logs))
            h2 = Harness(paths[hs.index(h)], h.ident.decode(),
                         RUST_SOURCE if h is hs[0] else CPP_SOURCE)
            h2.setting = "75"
            body_offset = next(i // 2 for i in range(20, len(h2.pattern), 2)
                               if h2.pattern[i:i + 2] != "??")
            changed = C.c_ubyte.from_address(C.addressof(h2.image) + 128 + body_offset)
            changed.value ^= 0xff
            _, init2 = h2.plugin()
            self.assertNotEqual(init2(C.byref(h2.api)), 0, h.ident)
            self.assertFalse(bool(h2.contract_fn(C.byref(h2.api))))
            self.assertEqual(h2.hooks, [])
            self.assertTrue(any("signature" in message.lower() or "arrival" in message.lower()
                                for message in h2.logs), (h.ident, h2.logs))
            # Force the scanner to return the known site after corrupting its
            # first instruction, so the independent exact-entry guard is tested.
            h2 = Harness(paths[hs.index(h)], h.ident.decode(),
                         RUST_SOURCE if h is hs[0] else CPP_SOURCE)
            h2.setting = "75"
            h2.scan_mode = "force-site"
            C.c_ubyte.from_address(C.addressof(h2.image) + 128).value ^= 0xff
            _, init2 = h2.plugin()
            self.assertNotEqual(init2(C.byref(h2.api)), 0, h.ident)
            self.assertFalse(bool(h2.contract_fn(C.byref(h2.api))))
            self.assertEqual(h2.hooks, [])
            self.assertTrue(any("entry" in message.lower() for message in h2.logs),
                            (h.ident, h2.logs))
            self.assertTrue(any("hook" in message.lower() for message in h2.logs),
                            (h.ident, h2.logs))
            h2 = Harness(paths[hs.index(h)], h.ident.decode(),
                         RUST_SOURCE if h is hs[0] else CPP_SOURCE)
            h2.setting = "75"
            h2.fail_hook = True
            _, init2 = h2.plugin()
            self.assertNotEqual(init2(C.byref(h2.api)), 0, h.ident)
            self.assertEqual(h2.hooks[0][2], 10)
            self.assertTrue(any("hook refused" in message.lower() for message in h2.logs),
                            (h.ident, h2.logs))
            refused_contract = h2.contract_fn(C.byref(h2.api)).contents
            self.assertEqual(refused_contract.count, 1)


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--rust-dll", type=pathlib.Path, required=True)
    parser.add_argument("--cpp-dll", type=pathlib.Path, required=True)
    parser.add_argument("--rust-source", type=pathlib.Path, required=True)
    parser.add_argument("--cpp-source", type=pathlib.Path, required=True)
    parser.add_argument("--rust-manifest", type=pathlib.Path, required=True)
    parser.add_argument("--cpp-manifest", type=pathlib.Path, required=True)
    args, remaining = parser.parse_known_args()
    RUST_DLL, CPP_DLL = args.rust_dll, args.cpp_dll
    RUST_SOURCE, CPP_SOURCE = args.rust_source.resolve(), args.cpp_source.resolve()
    RUST_MANIFEST, CPP_MANIFEST = args.rust_manifest.resolve(), args.cpp_manifest.resolve()
    RUST_DLL, CPP_DLL = RUST_DLL.resolve(), CPP_DLL.resolve()
    unittest.main(argv=[sys.argv[0], *remaining])
