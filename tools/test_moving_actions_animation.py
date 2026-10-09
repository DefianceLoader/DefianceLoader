"""Fake-ABI test for the scoped moving-actions animation overlay.

Set MOVING_ACTIONS_ANIMATION_BUILD to any of the supported 2026 builds.
The harness maps the native DLL without resolving imports; callbacks and
animation objects live in throwaway VirtualAlloc memory.
"""
from __future__ import annotations

import ctypes as C
import hashlib
import math
import os
import pathlib
import struct
import sys
from ctypes import wintypes as W

from compose_infantry_animation import quat_slerp, read_clip, sample_linear, sample_slerp
from moving_test_memory import K, KEEP, alloc, p64, i32, f32, check
from test_moving_grenades import put_code, route_fixture


ROOT = pathlib.Path(__file__).resolve().parents[1]
PLUGIN = pathlib.Path(os.environ.get(
    "MOVING_ACTIONS_ANIMATION_DLL",
    str(ROOT / "plugins/moving-actions-animation/target/release/defiance_plugin_moving_actions_animation.dll"),
))
ASSETS = pathlib.Path(os.environ.get(
    "MOVING_ACTIONS_ANIMATION_ASSET_DIR",
    str(ROOT / "plugins/moving-actions-animation/assets"),
))
PROFILES = {
    "gog-2026-09-14": ("eb8674f1d16595a3e9cf6a9ec0062735b1184976495d8d6ade36f7e2574e8aab",
                       (0x2C3900, 0x436260, 0x436460), 0x136880, 0x72B0D8, 0x72B370, 0x2C3DF0),
    "steam-2026-09-22": ("30264904e1d5199b954bafbd7828cf7190930c246d35fa7b94eefa915e8f0c38",
                         (0x2C3990, 0x4362F0, 0x4364F0), 0x136910, 0x72B118, 0x72B3B0, 0x2C3E80),
    "gog-2026-09-25": ("1216d627c7288c7db6940168363be582232ed4d3cb860b8b8c8d7489652eca74",
                       (0x2C3900, 0x4368A0, 0x436AA0), 0x136880, 0x72C0D8, 0x72C370, 0x2C3DF0),
    "steam-2026-09-25": ("adb3ad95926036809b4e554b466bef33d4ac7aa5303e59a9e4a940890bc334b5",
                         (0x2C3990, 0x436930, 0x436B30), 0x136910, 0x72C118, 0x72C3B0, 0x2C3E80),
    "gog-2026-10-07": ("da62ed73b43fafa8af0f25fba0501bff641a0cfd1bfb0d3a0b8777a1b0d99271",
                       (0x2C4240, 0x43A2F0, 0x43A4F0), 0x136880, 0x731090, 0x731320, 0x2C4730),
    "steam-2026-10-07": ("fdb1d3bb7fdd0a47ac1b8a530cf0ee302fa2a96cc7283972504b0aa41ab2336f",
                         (0x2C42D0, 0x43A380, 0x43A580), 0x136910, 0x731128, 0x7313C8, 0x2C47C0),
}
BUILD_NAME = os.environ.get("MOVING_ACTIONS_ANIMATION_BUILD", "steam-2026-09-25")
check(BUILD_NAME in PROFILES, f"unsupported test build: {BUILD_NAME}")
LOGIC_SHA256, SITES, SLERP_RVA, HUMAN_ANIMATION_VT, HUMAN_CHASSIS_VT, UPDATE_THUNK = PROFILES[BUILD_NAME]
store, build_date = BUILD_NAME.split("-", 1)
LOGIC = ROOT / "bin" / store / build_date / "logic.dll"
PREFIXES = (
    bytes.fromhex("488bc4488958105556574881ece00000"),
    bytes.fromhex("48895c240848897c2410488b4108498b"),
    bytes.fromhex("48895c241048896c2418488974242057"),
)
UPDATE_TARGET = SITES[0]
OWNER_GETTER = 0
LOGIC_RANGE = None
EXPECTED_ASSET_HASHES = {
    "moving_throw_back.anim": "e00c3aac5f35a33b5401f41d03b5e423f9383036fb2ce59edd87dd99635440f7",
    "stock_throw.anim": "93b231461ddb18bbd7990609fb505b3bff413ed56bcdc3a8e20ec0d312ecdbcb",
    "stock_switch.anim": "3fab8877a8cee9eb41bd1f39451eefafaba1ed034ea803722f3baf1e408eccbc",
    "moving_throw.anim": "e63eee8558994069b74b103e85a6469169b3d5eb14f098f8f2061cbf58f16540",
    "moving_switch.anim": "88b977a43d09c3460a00a3c1bb883818ce0190bdf1ba8fc7dbed724a16d2005b",
}

K.LoadLibraryExW.argtypes = [W.LPCWSTR, C.c_void_p, W.DWORD]
K.LoadLibraryExW.restype = C.c_void_p
K.FreeLibrary.argtypes = [C.c_void_p]
K.VirtualProtect.argtypes = [C.c_void_p, C.c_size_t, W.DWORD, C.POINTER(W.DWORD)]
K.VirtualProtect.restype = W.BOOL
K.GetCurrentProcess.restype = C.c_void_p
K.FlushInstructionCache.argtypes = [C.c_void_p, C.c_void_p, C.c_size_t]
K.FlushInstructionCache.restype = W.BOOL


def ptr(address, offset=0):
    return C.c_void_p.from_address(address + offset).value or 0


def u8(address, offset=0):
    return C.c_uint8.from_address(address + offset).value


def u32(address, offset=0):
    return C.c_uint32.from_address(address + offset).value


def put_bytes(address, offset, data):
    C.memmove(address + offset, data, len(data))


def put_vec3(address, values):
    for index, value in enumerate(values):
        f32(address, index * 4, value)


def read_values(address, count):
    return tuple(C.c_float.from_address(address + i * 4).value for i in range(count))


def alloc_readwrite(size):
    address = K.VirtualAlloc(None, max(size, 8), 0x3000, 0x04)  # committed PAGE_READWRITE
    if not address:
        raise OSError(C.get_last_error())
    KEEP.append(address)
    C.memset(address, 0, size)
    return address


def check_external_owner_getter(address):
    check(LOGIC_RANGE is not None and not (LOGIC_RANGE[0] <= address < LOGIC_RANGE[1]),
          "test owner getter must be outside the logic.dll image")


class NativeClip:
    """Build the native vector/string layout used by the supported clip reader."""

    def __init__(self, clip):
        self.clip = clip
        self.animation = alloc(0x20)
        self.tracks_size = max(len(clip.tracks) * 0x50, 0x50)
        self.tracks_memory = alloc(self.tracks_size)
        self.data_blocks = [(self.tracks_memory, self.tracks_size)]
        f32(self.animation, 0, clip.duration)
        p64(self.animation, 8, self.tracks_memory)
        p64(self.animation, 0x10, self.tracks_memory + len(clip.tracks) * 0x50)
        p64(self.animation, 0x18, self.tracks_memory + len(clip.tracks) * 0x50)
        self.track_records = {}
        self.vector_descriptors = {}
        for index, track in enumerate(clip.tracks):
            record = self.tracks_memory + index * 0x50
            name = track.name_bytes
            if len(name) <= 15:
                put_bytes(record, 0, name + b"\0")
            else:
                name_storage = alloc(len(name) + 1)
                self.data_blocks.append((name_storage, len(name) + 1))
                put_bytes(name_storage, 0, name + b"\0")
                p64(record, 0, name_storage)
            p64(record, 0x10, len(name))
            p64(record, 0x18, 15 if len(name) <= 15 else len(name))
            pos_mem, pos_begin, pos_end = self._vector(record, 0x20, track.positions, "<4f")
            rot_mem, rot_begin, rot_end = self._vector(record, 0x38, track.rotations, "<5f")
            self.track_records[track.name] = record
            self.vector_descriptors[(track.name, "position")] = record + 0x20
            self.vector_descriptors[(track.name, "rotation")] = record + 0x38
            self.data_blocks.extend(((pos_mem, max(len(track.positions) * 16, 16)),
                                     (rot_mem, max(len(track.rotations) * 20, 20))))
            KEEP.extend((pos_mem, rot_mem))

    @staticmethod
    def _vector(record, offset, keys, fmt):
        codec = struct.Struct(fmt)
        memory = alloc(max(codec.size * len(keys), codec.size))
        for index, key in enumerate(keys):
            C.memmove(memory + index * codec.size, codec.pack(*key), codec.size)
        begin = memory
        end = memory + codec.size * len(keys)
        p64(record, offset, begin)
        p64(record, offset + 8, end)
        p64(record, offset + 16, end)
        return memory, begin, end

    def snapshot(self):
        return tuple(C.string_at(address, size) for address, size in self.data_blocks)


def make_fixture(base, native_clip, action=0x17, *, mode=1, route_state=2, posture=1):
    f = route_fixture(base)
    p64(f["chassis"], 0, base + HUMAN_CHASSIS_VT)
    facet, facet_junction, geometry, binding = alloc(0x100), alloc(0x40), alloc(0x500), alloc(0x60)
    p64(facet, 0, base + HUMAN_ANIMATION_VT)
    p64(facet, 0x10, facet_junction)
    p64(facet_junction, 0x10, f["unit"])
    p64(f["components"], 0x58, facet)
    # The native Unit::getComponents vfunc lives in essence.dll, outside
    # logic.dll. Point at an executable test-owned getter with the same ABI.
    check(OWNER_GETTER, "test owner getter was not initialized")
    check_external_owner_getter(OWNER_GETTER)
    p64(f["unit_vt"], 0xB0, OWNER_GETTER)
    p64(f["unit"], 0x68, f["components"])
    p64(facet, 0xB0, geometry)
    p64(geometry, 0x1C8, binding)
    p64(binding, 0x20, native_clip.animation)
    i32(facet, 0x6C, action)
    i32(facet, 0x74, posture)
    i32(f["chassis"], 0xEC, mode)
    C.c_uint8.from_address(f["records"] + 0x2CC).value = route_state
    f.update(facet=facet, facet_junction=facet_junction, geometry=geometry,
             binding=binding, native_clip=native_clip)
    return f


def install_nodes(fixture, *, unknown_rotation_getter=False):
    """Build the native binding node vector used by the phase post-pose probe."""
    clip = fixture["native_clip"].clip
    nodes_memory = alloc(max(len(clip.tracks) * 0x18, 0x18))
    nodes = []
    target_index = next(i for i, track in enumerate(clip.tracks)
                        if track.name == "Bip01_L_Thigh")
    target_page = 0
    for index, _track in enumerate(clip.tracks):
        if unknown_rotation_getter and index == target_index:
            node_memory = alloc(0x3000)
            node = node_memory + 0x1000 - 0x3DC
            target_page = node_memory + 0x1000
        else:
            node = alloc(0x500)
        node_vtable, getter = alloc(0x100), alloc(0x100)
        code = (bytes.fromhex("488d81e0030000c3") if unknown_rotation_getter and index == target_index
                else bytes.fromhex("488d81dc030000c3"))
        put_code(getter, code)
        p64(node, 0, node_vtable)
        p64(node_vtable, 0x88, getter)
        p64(nodes_memory, index * 0x18, node)
        nodes.append(node)
    p64(fixture["binding"], 0x30, nodes_memory)
    p64(fixture["binding"], 0x38, nodes_memory + len(clip.tracks) * 0x18)
    p64(fixture["binding"], 0x40, nodes_memory + len(clip.tracks) * 0x18)
    fixture.update(nodes=nodes, thigh_track_index=target_index,
                   thigh_node=nodes[target_index], node_vector=nodes_memory)
    if target_page:
        old = W.DWORD()
        check(K.VirtualProtect(target_page, 0x1000, 0x01, C.byref(old)),
              f"cannot guard unknown-getter pose field: {C.get_last_error()}")
        fixture["guarded_pose_page"] = (target_page, old.value)
    return fixture


def main():
    if sys.platform != "win32" or C.sizeof(C.c_void_p) != 8:
        raise SystemExit("moving-actions animation test requires Windows x64")
    check(PLUGIN.is_file(), f"missing release plugin: {PLUGIN}")
    check(LOGIC.is_file(), f"missing supported logic.dll: {LOGIC}")
    check(hashlib.sha256(LOGIC.read_bytes()).hexdigest() == LOGIC_SHA256,
          "Steam 2026-09-25 logic.dll hash changed")
    for name, expected in EXPECTED_ASSET_HASHES.items():
        path = ASSETS / name
        check(path.is_file(), f"missing animation asset: {path}")
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if os.environ.get("MOVING_ACTIONS_ANIMATION_ASSET_DIR"):
            import json
            provenance = json.loads((ASSETS.parent / "sources.json").read_text())
            check(provenance["outputs"][name] == digest,
                  f"animation asset differs from sources.json: {name}")
        else:
            check(digest == expected, f"animation asset hash changed: {name}")
        read_clip(path)

    base = K.LoadLibraryExW(str(LOGIC), None, 1)  # DONT_RESOLVE_DLL_REFERENCES.
    check(base, f"LoadLibraryExW failed: {C.get_last_error()}")
    image = LOGIC.read_bytes()
    pe = int.from_bytes(image[0x3C:0x40], "little")
    image_size = int.from_bytes(image[pe + 24 + 56:pe + 24 + 60], "little")
    global LOGIC_RANGE
    LOGIC_RANGE = (base, base + image_size)
    for rva, prefix in zip(SITES, PREFIXES):
        check(C.string_at(base + rva, len(prefix)) == prefix, f"logic prefix differs at {rva:#x}")
    thunk = C.string_at(base + UPDATE_THUNK, 5)
    check(thunk[0] == 0xE9, "update vfunc thunk is no longer a near JMP")
    rel = struct.unpack("<i", thunk[1:])[0]
    check(UPDATE_THUNK + 5 + rel == UPDATE_TARGET, "update vfunc thunk no longer targets RVA 2C3990")

    NATIVE_SLERP = C.CFUNCTYPE(C.c_void_p, C.POINTER(C.c_float), C.POINTER(C.c_float),
                               C.POINTER(C.c_float), C.c_float)
    @NATIVE_SLERP
    def mapped_slerp(left, output, right, amount):
        q = quat_slerp(tuple(left[i] for i in range(4)), tuple(right[i] for i in range(4)),
                       float(amount), order="wxyz")
        for index, value in enumerate(q):
            output[index] = value
        return C.cast(output, C.c_void_p).value

    slerp_jump = b"\x48\xb8" + struct.pack("<Q", C.cast(mapped_slerp, C.c_void_p).value) + b"\xff\xe0"
    global OWNER_GETTER
    OWNER_GETTER = alloc(0x100)
    getter_code = bytes.fromhex("488b4168c3")  # mov rax,[rcx+0x68]; ret
    put_code(OWNER_GETTER, getter_code)
    process = K.GetCurrentProcess()
    check(K.FlushInstructionCache(process, OWNER_GETTER, len(getter_code)),
          "cannot flush external owner getter")

    LOG = C.CFUNCTYPE(None, C.c_uint32, C.c_char_p)
    MODULE_BASE = C.CFUNCTYPE(C.c_void_p, C.c_char_p)
    MODULE_SIZE = C.CFUNCTYPE(C.c_size_t, C.c_void_p)
    HOOK = C.CFUNCTYPE(C.c_int, C.c_void_p, C.c_void_p, C.POINTER(C.c_void_p))
    UNHOOK = C.CFUNCTYPE(C.c_int, C.c_void_p)
    UPDATE = C.CFUNCTYPE(None, C.c_void_p, C.c_float)
    SAMPLER = C.CFUNCTYPE(C.c_void_p, C.c_void_p, C.c_void_p, C.c_float, C.c_void_p)
    callbacks, logs, hooks, unhooks, hook_detours = [], [], [], [], {}
    original_calls = []
    fixture_by_facet = {}
    fixture_config = {}
    bases = {"base": base, "size": image_size}

    @LOG
    def log(level, text):
        logs.append((level, text.decode(errors="replace")))

    @MODULE_BASE
    def module_base(name):
        return bases["base"] if name == b"logic.dll" else None

    @MODULE_SIZE
    def module_size(module):
        return bases["size"] if module == bases["base"] else 0

    def channel_sample(descriptor, out, time, cursor, stride, components, rotation=False):
        begin = ptr(descriptor, 0)
        end = ptr(descriptor, 8)
        count = (end - begin) // stride if begin and end >= begin else 0
        codec = struct.Struct("<" + ("5f" if rotation else "4f"))
        keys = [codec.unpack_from(C.string_at(begin + n * stride, stride)) for n in range(count)]
        if rotation:
            value = sample_slerp(keys, time, order="wxyz")
        else:
            value = sample_linear(keys, time)
        if value is None:
            value = (0.0,) * components
        for index, component in enumerate(value):
            f32(out, index * 4, component)
        if cursor:
            # Make the test prove the trampoline alone changed this cursor.
            C.c_uint32.from_address(cursor).value = count ^ 0xA55A
        return out

    @UPDATE
    def original_update(facet, dt):
        original_calls.append(("update", facet, float(dt)))
        fixture = fixture_by_facet[facet]
        config = fixture_config.get(facet, {})
        if "action_after" in config:
            i32(facet, 0x6C, config["action_after"])
        callback = config.get("inside")
        if callback:
            callback(fixture)

    @SAMPLER
    def original_position(descriptor, out, time, cursor):
        original_calls.append(("position", descriptor, out, float(time), cursor))
        return channel_sample(descriptor, out, time, cursor, 16, 3)

    @SAMPLER
    def original_rotation(descriptor, out, time, cursor):
        original_calls.append(("rotation", descriptor, out, float(time), cursor))
        return channel_sample(descriptor, out, time, cursor, 20, 4, rotation=True)

    def preserving_native_sampler_callback(callback):
        """Python callbacks clobber volatile regs; synthesize a native post-call state."""
        address = alloc(0x100)
        callback_address = C.cast(callback, C.c_void_p).value
        code = (b"\x48\x83\xec\x38"          # sub rsp, 38h (shadow + aligned saves)
                b"\x0f\x29\x54\x24\x20"    # save incoming xmm2
                b"\x48\xb8" + struct.pack("<Q", callback_address) +
                b"\xff\xd0"                  # call rax
                b"\x48\x89\x44\x24\x30"    # save return pointer
                b"\x48\xb9\x22\x22\x22\x22\x11\x11\x11\x11"
                b"\x48\xba\x44\x44\x44\x44\x33\x33\x33\x33"
                b"\x49\xb8\x66\x66\x66\x66\x55\x55\x55\x55"
                b"\x49\xb9\x88\x88\x88\x88\x77\x77\x77\x77"
                b"\x49\xba\xaa\xaa\xaa\xaa\x99\x99\x99\x99"
                b"\x49\xbb\xcc\xcc\xcc\xcc\xbb\xbb\xbb\xbb"
                b"\x66\x0f\x76\xc0"          # pcmpeqd xmm0,xmm0 => all ones
                b"\x66\x0f\x76\xc9"          # pcmpeqd xmm1,xmm1
                b"\x66\x0f\x76\xdb"          # pcmpeqd xmm3,xmm3
                b"\x66\x0f\x76\xe4"          # pcmpeqd xmm4,xmm4
                b"\x66\x0f\x76\xed"          # pcmpeqd xmm5,xmm5
                b"\x0f\x28\x54\x24\x20"    # preserve native time in xmm2
                b"\x48\x8b\x44\x24\x30"    # restore original return pointer
                b"\x48\x83\xc4\x38\xc3")  # add rsp, 38h; ret
        put_code(address, code)
        check(K.FlushInstructionCache(K.GetCurrentProcess(), address, len(code)),
              "cannot flush native original-sampler shim")
        return address

    original_addresses = (
        C.cast(original_update, C.c_void_p).value,
        preserving_native_sampler_callback(original_position),
        preserving_native_sampler_callback(original_rotation),
    )

    originals = (original_update, original_position, original_rotation)
    fail_at = {"index": None}
    null_trampoline_at = {"index": None}

    @HOOK
    def hook(target, detour, out):
        index = len(hooks)
        hooks.append((target, detour))
        if fail_at["index"] == index:
            return 1
        hook_detours[index] = detour
        if null_trampoline_at["index"] == index:
            return 0
        C.cast(out, C.POINTER(C.c_void_p))[0] = original_addresses[index]
        return 0

    @UNHOOK
    def unhook(target):
        unhooks.append(target)
        return 0

    callbacks.extend((log, module_base, module_size, hook, unhook, *originals, mapped_slerp))
    KEEP.extend(callbacks)

    class Api(C.Structure):
        _fields_ = [("abi_version", C.c_uint32), ("reserved", C.c_uint32)] + [
            (name, C.c_void_p) for name in (
                "log", "module_base", "module_size", "find_pattern", "find_pattern_at",
                "hook", "hook_exact", "hook_call", "unhook", "rtti_method", "vtable_slot",
                "config_get", "patch_bytes")]

    api = Api(5, 0, *[C.cast(fn, C.c_void_p).value if fn else None for fn in
                      (log, module_base, module_size, None, None, hook, None, None,
                       unhook, None, None, None, None)])

    class Plugin(C.Structure):
        _fields_ = [("abi_version", C.c_uint32), ("name", C.c_char_p), ("version", C.c_char_p),
                    ("init", C.c_void_p), ("stop", C.c_void_p)]

    lib = C.CDLL(str(PLUGIN))
    lib.defiance_plugin.restype = C.c_void_p
    plugin = C.cast(lib.defiance_plugin(), C.POINTER(Plugin)).contents
    check(plugin.abi_version == 5, "plugin ABI mismatch")
    check(plugin.name == b"defiance.moving-actions-animation", "plugin name mismatch")
    check(plugin.version.startswith(b"0."), f"unexpected plugin version {plugin.version!r}")
    init = C.CFUNCTYPE(C.c_int, C.POINTER(Api))(plugin.init)
    check(init(None) != 0 and not hooks, "null API must be refused before hook")
    wrong = Api.from_buffer_copy(api)
    wrong.abi_version = 4
    check(init(C.byref(wrong)) != 0 and not hooks, "wrong ABI must be refused before hook")
    reserved = Api.from_buffer_copy(api)
    reserved.reserved = 1
    check(init(C.byref(reserved)) != 0 and not hooks, "reserved ABI field must be zero")

    # All static checks pass, but the exposed live module-size check fails.
    bases["size"] = SITES[-1] + len(PREFIXES[-1]) - 1
    check(init(C.byref(api)) != 0 and not hooks, "truncated module range must be refused")
    bases["size"] = image_size

    # A second-hook failure must roll back the first hook; then retry succeeds.
    fail_at["index"] = 1
    check(init(C.byref(api)) != 0, f"second hook failure was accepted: {logs}")
    check(unhooks == [base + SITES[0]],
          f"failed init removed the refused hook site or failed to roll back the owned update hook: {unhooks=} {hooks=} {logs=}")
    hooks.clear(); hook_detours.clear(); unhooks.clear(); fail_at["index"] = None
    # A hook provider that reports success but omits its trampoline has
    # transferred ownership of the current site; rollback all owned sites.
    null_trampoline_at["index"] = 1
    check(init(C.byref(api)) != 0, "success with a null trampoline must be refused")
    check(unhooks == [base + SITES[1], base + SITES[0]],
          "malformed successful hook did not roll back owned sites in reverse order")
    hooks.clear(); hook_detours.clear(); unhooks.clear(); null_trampoline_at["index"] = None
    check(init(C.byref(api)) == 0, f"valid initialization refused: {logs}")
    check([entry[0] for entry in hooks] == [base + rva for rva in SITES], "expected three ordered hooks")
    update = C.CFUNCTYPE(None, C.c_void_p, C.c_float)(hook_detours[0])
    position = C.CFUNCTYPE(C.c_void_p, C.c_void_p, C.c_void_p, C.c_float, C.c_void_p)(hook_detours[1])
    rotation = C.CFUNCTYPE(C.c_void_p, C.c_void_p, C.c_void_p, C.c_float, C.c_void_p)(hook_detours[2])
    check(any("installed" in text.lower() for _, text in logs), "missing install log")
    put_code(base + SLERP_RVA, slerp_jump)
    check(K.FlushInstructionCache(process, base + SLERP_RVA, len(slerp_jump)),
          "cannot flush test slerp thunk")

    def movabs(opcode, value):
        return opcode + struct.pack("<Q", value)

    def make_unblended_sampler_caller(result_block, pos_desc, pos_out, pos_cursor,
                                      rot_desc, rot_out, rot_cursor, time):
        """Native-shaped caller captures volatile state after each detoured sampler."""
        address = alloc(0x1000)
        code = bytearray(b"\x48\x81\xec\xe8\x00\x00\x00")  # aligned frame/shadow
        code += b"\x48\x89\x7c\x24\x20"    # save caller's nonvolatile RDI
        code += movabs(b"\x48\xbf", result_block)  # RDI = result block, stable across callbacks
        code += movabs(b"\x48\xb9", pos_desc) # rcx = position descriptor
        code += movabs(b"\x48\xba", pos_out)  # rdx = position output
        code += movabs(b"\x49\xb9", pos_cursor)  # r9 = cursor
        code += b"\xb8" + struct.pack("<I", struct.unpack("<I", struct.pack("<f", time))[0])
        code += b"\x66\x0f\x6e\xd0"         # movd xmm2, eax (float time)
        code += movabs(b"\x48\xb8", hook_detours[1]) + b"\xff\xd0"  # call position
        gpr_stack = (0x40, 0x48, 0x50, 0x58, 0x60, 0x68, 0x70)
        gpr_store = (b"\x48\x89\x44\x24", b"\x48\x89\x4c\x24",
                     b"\x48\x89\x54\x24", b"\x4c\x89\x44\x24",
                     b"\x4c\x89\x4c\x24", b"\x4c\x89\x54\x24",
                     b"\x4c\x89\x5c\x24")
        xmm_stack = (0x80, 0x90, 0xA0, 0xB0, 0xC0, 0xD0)
        xmm_store_modrm = (0x84, 0x8C, 0x94, 0x9C, 0xA4, 0xAC)

        def capture_native_state():
            capture = bytearray()
            for opcode, stack_offset in zip(gpr_store, gpr_stack):
                capture += opcode + bytes((stack_offset,))
            for modrm, stack_offset in zip(xmm_store_modrm, xmm_stack):
                capture += b"\x0f\x29" + bytes((modrm, 0x24)) + struct.pack("<I", stack_offset)
            return capture

        def copy_native_state(output_offset):
            copy = bytearray()
            if output_offset:
                copy += b"\x48\x81\xc7" + struct.pack("<I", output_offset)
            for index, stack_offset in enumerate(gpr_stack):
                copy += b"\x48\x8b\x84\x24" + struct.pack("<I", stack_offset)
                copy += b"\x48\x89\x87" + struct.pack("<I", index * 8)
            for index, stack_offset in enumerate(xmm_stack):
                copy += b"\x0f\x28\x84\x24" + struct.pack("<I", stack_offset)
                copy += b"\x0f\x29\x87" + struct.pack("<I", 0x40 + index * 16)
            return copy

        code += capture_native_state()
        code += copy_native_state(0)
        code += movabs(b"\x48\xb9", rot_desc) # rcx = rotation descriptor
        code += movabs(b"\x48\xba", rot_out)  # rdx = rotation output
        code += movabs(b"\x49\xb9", rot_cursor)  # r9 = cursor; XMM2 is NOT reloaded
        code += movabs(b"\x48\xb8", hook_detours[2]) + b"\xff\xd0"  # call rotation
        code += capture_native_state()
        code += copy_native_state(0xA0)
        code += b"\x48\x8b\x7c\x24\x20"  # restore caller's RDI
        code += b"\x48\x81\xc4\xe8\x00\x00\x00\xc3"
        put_code(address, bytes(code))
        check(K.FlushInstructionCache(K.GetCurrentProcess(), address, len(code)),
              "cannot flush native position/rotation caller")
        return C.CFUNCTYPE(None)(address)

    # Construct fixture animations directly from the exact staged stock assets.
    stock = {
        "throw": NativeClip(read_clip(ASSETS / "stock_throw.anim")),
        "switch": NativeClip(read_clip(ASSETS / "stock_switch.anim")),
    }
    moving = {
        "throw": read_clip(ASSETS / "moving_throw.anim"),
        "switch": read_clip(ASSETS / "moving_switch.anim"),
        "throw_back": read_clip(ASSETS / "moving_throw_back.anim"),
    }

    # The real evaluator reuses XMM2 from its position call as the rotation
    # call's time. A normal ctypes call reloads each argument, so this tiny
    # executable caller intentionally does not reload XMM2 between detours.
    caller_fixture = make_fixture(base, stock["throw"])
    fixture_by_facet[caller_fixture["facet"]] = caller_fixture
    f32(caller_fixture["geometry"], 0x1D8, 0.73)
    pos_desc = caller_fixture["native_clip"].vector_descriptors[
        ("Bip01_L_Thigh", "position")]
    rot_desc = caller_fixture["native_clip"].vector_descriptors[
        ("Bip01_L_Thigh", "rotation")]
    pos_out, rot_out = alloc(0x20), alloc(0x20)
    pos_cursor, rot_cursor = alloc(0x10), alloc(0x10)
    caller_result = alloc(0x180)
    caller = make_unblended_sampler_caller(
        caller_result, pos_desc, pos_out, pos_cursor, rot_desc, rot_out, rot_cursor, 0.73)
    fixture_config[caller_fixture["facet"]] = {
        "action_after": 0x17,
        "inside": lambda _fixture: caller(),
    }
    call_start = len(original_calls)
    update(caller_fixture["facet"], C.c_float(1 / 60))
    unblended_calls = original_calls[call_start + 1:]
    check(len(unblended_calls) == 2
          and unblended_calls[0][0] == "position"
          and unblended_calls[1][0] == "rotation",
          f"native-shaped unblended caller did not invoke position then rotation: {unblended_calls}")
    expected_time = float(C.c_float(0.73).value)
    check(unblended_calls[0][1:] == (pos_desc, pos_out, expected_time, pos_cursor)
          and unblended_calls[1][1:] == (rot_desc, rot_out, expected_time, rot_cursor),
          f"position detour clobbered the time reused by native rotation: {unblended_calls}")
    expected_gprs = (pos_out, 0x1111111122222222, 0x3333333344444444,
                     0x5555555566666666, 0x7777777788888888,
                     0x99999999AAAAAAAA, 0xBBBBBBBBCCCCCCCC)
    for offset, expected_rax in ((0, pos_out), (0xA0, rot_out)):
        got_gprs = tuple(C.c_uint64.from_address(caller_result + offset + i * 8).value
                         for i in range(7))
        check(got_gprs == (expected_rax, *expected_gprs[1:]),
              f"sampler augmentation changed native volatile GPRs: {got_gprs}")
        xmm_bytes = C.string_at(caller_result + offset + 0x40, 0x60)
        for i in (0, 1, 3, 4, 5):
            check(xmm_bytes[i * 16:(i + 1) * 16] == b"\xff" * 16,
                  f"sampler augmentation changed native XMM{i} state")
        expected_xmm2 = struct.pack("<f", expected_time) + b"\0" * 12
        check(xmm_bytes[0x20:0x30] == expected_xmm2,
              f"native caller did not retain XMM2 across detours: {xmm_bytes[0x20:0x30]}")
    expected_position = sample_linear(moving["throw"].by_name["Bip01_L_Thigh"].positions, 0.73)
    expected_rotation = sample_slerp(moving["throw"].by_name["Bip01_L_Thigh"].rotations,
                                     0.73, order="wxyz")
    check(max(abs(a - b) for a, b in zip(read_values(pos_out, 3), expected_position)) < 2e-4,
          "native-shaped caller's position sample did not use the preserved phase")
    check(max(abs(a - b) for a, b in zip(read_values(rot_out, 4), expected_rotation)) < 2e-4,
          "native-shaped caller's rotation sample did not use the preserved phase")

    def invoke_sample(fixture, node, channel, time, detoured, *, cursor_sentinel=0x10203040):
        descriptor = fixture["native_clip"].vector_descriptors[(node, channel)]
        out = alloc(0x20)
        cursor = alloc(0x10)
        C.c_uint32.from_address(cursor).value = cursor_sentinel
        fn = position if channel == "position" else rotation
        old_original_count = len([call for call in original_calls if call[0] == channel])
        result = fn(descriptor, out, time, cursor) if detoured else (
            original_position(descriptor, out, time, cursor) if channel == "position" else
            original_rotation(descriptor, out, time, cursor))
        expected_cursor = ((len(fixture["native_clip"].clip.by_name[node].positions)
                            if channel == "position" else
                            len(fixture["native_clip"].clip.by_name[node].rotations)) ^ 0xA55A)
        check(result == out, f"{node} {channel}: sampler return pointer changed")
        check(u32(cursor) == expected_cursor, f"{node} {channel}: original cursor side effect lost")
        calls = [call for call in original_calls if call[0] == channel]
        check(len(calls) == old_original_count + 1,
              f"{node} {channel}: original sampler was not forwarded exactly once")
        check(calls[-1][1:] == (descriptor, out, float(C.c_float(time).value), cursor),
              f"{node} {channel}: trampoline arguments changed")
        return read_values(out, 3 if channel == "position" else 4)

    def sample_inside(fixture, clip_key, action, backward=0.0):
        fixture_by_facet[fixture["facet"]] = fixture
        plan = {"kind": clip_key, "action": action, "samples": [], "time": 0.73}
        fixture_config[fixture["facet"]] = {
            "action_after": action,
            "inside": lambda f: plan["samples"].extend(
                (node, channel, invoke_sample(f, node, channel, plan["time"], True))
                for node, channel in (("Bip01_L_Thigh", "position"),
                                      ("Bip01_L_Thigh", "rotation"),
                                      ("Bip01_L_Calf", "position"),
                                      ("Bip01_L_Calf", "rotation"),
                                      ("Bip01_Spine", "position"),
                                      ("Bip01_Spine", "rotation")))
        }
        chassis_before = C.string_at(fixture["chassis"], 0x300)
        route_before = C.string_at(fixture["records"], 0x2F0)
        animation_before = C.string_at(fixture["native_clip"].animation, 0x20)
        keydata_before = fixture["native_clip"].snapshot()
        binding_before = C.string_at(fixture["binding"], 0x60)
        geometry_binding_before = C.string_at(fixture["geometry"] + 0x1C8, 0x18)
        facet_before = bytearray(C.string_at(fixture["facet"], 0x100))
        update_calls_before = len([c for c in original_calls if c[0] == "update"])
        update(fixture["facet"], C.c_float(1 / 60))
        check(len([c for c in original_calls if c[0] == "update"]) == update_calls_before + 1,
              "original facet update was not forwarded exactly once")
        last_update = next(c for c in reversed(original_calls) if c[0] == "update")
        check(last_update == ("update", fixture["facet"], float(C.c_float(1 / 60).value)),
              "native update arguments changed")
        check(C.string_at(fixture["chassis"], 0x300) == chassis_before,
              "animation update modified chassis movement state")
        check(C.string_at(fixture["records"], 0x2F0) == route_before,
              "animation update modified navigation state")
        check(C.string_at(fixture["native_clip"].animation, 0x20) == animation_before,
              "animation update modified native animation headers")
        check(fixture["native_clip"].snapshot() == keydata_before,
              "animation update modified native track/key data")
        check(C.string_at(fixture["binding"], 0x60) == binding_before and
              C.string_at(fixture["geometry"] + 0x1C8, 0x18) == geometry_binding_before,
              "animation update modified animation binding")
        facet_after = bytearray(C.string_at(fixture["facet"], 0x100))
        facet_before[0x6C:0x70] = facet_after[0x6C:0x70]  # Original callback may advance action.
        check(facet_after == facet_before, "animation update changed facet timers or ownership fields")
        expected_clip = moving[clip_key]
        for node, channel, values in plan["samples"]:
            track = expected_clip.by_name[node]
            expected = (sample_linear(track.positions, plan["time"]) if channel == "position"
                        else sample_slerp(track.rotations, plan["time"], order="wxyz"))
            if clip_key == "throw" and backward > 0 and node != "Bip01_Spine":
                other = moving["throw_back"].by_name[node]
                back = (sample_linear(other.positions, plan["time"]) if channel == "position"
                        else sample_slerp(other.rotations, plan["time"], order="wxyz"))
                expected = (tuple(a + (b - a) * backward for a, b in zip(expected, back))
                            if channel == "position" else
                            quat_slerp(expected, back, backward, order="wxyz"))
            if node == "Bip01_Spine":
                expected = (sample_linear(fixture["native_clip"].clip.by_name[node].positions, plan["time"])
                            if channel == "position" else
                            sample_slerp(fixture["native_clip"].clip.by_name[node].rotations,
                                         plan["time"], order="wxyz"))
            check(expected is not None and max(abs(a - b) for a, b in zip(values, expected)) < 2e-4,
                  f"{clip_key} action {action:#x}: {node} {channel} did not match expected sample")
        return fixture

    # Each supported action is checked with the facet changing from ordinary
    # action 3 to the special action during the original update callback.
    for clip_key, action_code in (("throw", 0x17), ("throw", 0x2B), ("switch", 0x18)):
        fixture = make_fixture(base, stock[clip_key], action=3, mode=1, route_state=2, posture=1)
        sample_inside(fixture, clip_key, action_code)
        # Outside the update's TLS scope the exact stock sampler runs.
        values = invoke_sample(fixture, "Bip01_L_Thigh", "position", 0.73, True)
        stock_values = sample_linear(fixture["native_clip"].clip.by_name["Bip01_L_Thigh"].positions, 0.73)
        check(max(abs(a - b) for a, b in zip(values, stock_values)) < 2e-5,
              "out-of-scope sampler did not return stock animation")

    # Face the target while the route continues in a different direction.
    # Confirm forward/backward/intermediate leg sampling without changing
    # chassis, navigation, original clip, root/spine, timers or callback ABI.
    for facing, travel, backward in (
        ((1, 0), (1, 0), 0.0),
        ((-1, 0), (1, 0), 1.0),
        ((-1, 1), (1, 0), 1.0),
        ((-0.05, 1), (1, 0), 0.05 / math.sqrt(1.0025) / 0.35),
        ((0, 1), (1, 0), 0.0),
        ((0, -1), (1, 0), 0.0),
        ((float("nan"), 0), (1, 0), 0.0),
        ((1, 0), (0, 0), 0.0),
    ):
        fixture = make_fixture(base, stock["throw"], action=3)
        f32(fixture["chassis"], 0x84, facing[0])
        f32(fixture["chassis"], 0x88, facing[1])
        f32(fixture["records"], 0x260, travel[0])
        f32(fixture["records"], 0x268, travel[1])
        sample_inside(fixture, "throw", 0x17, backward)
    # A moving throw crosses a waypoint while body facing stays unchanged.
    # Subsequent actual displacement must win over the stale waypoint vector.
    motion_fixture = make_fixture(base, stock["throw"], action=0x17)
    f32(motion_fixture["chassis"], 0x84, 1)
    f32(motion_fixture["records"], 0x260, -1)
    sample_inside(motion_fixture, "throw", 0x17, 1.0)
    f32(motion_fixture["position"], 0, 0.125)
    sample_inside(motion_fixture, "throw", 0x17, 0.0)
    # On an interpolation frame with no fresh step, hold the last direction.
    sample_inside(motion_fixture, "throw", 0x17, 0.0)
    # Opposite actual motion engages the complete backward gait.
    f32(motion_fixture["position"], 0, -0.125)
    f32(motion_fixture["records"], 0x260, 1)
    sample_inside(motion_fixture, "throw", 0x17, 1.0)
    # Stop must discard motion history before a later movement order.
    i32(motion_fixture["chassis"], 0xEC, 0)
    fixture_by_facet[motion_fixture["facet"]] = motion_fixture
    fixture_config[motion_fixture["facet"]] = {}
    update(motion_fixture["facet"], C.c_float(1 / 60))
    i32(motion_fixture["chassis"], 0xEC, 1)
    sample_inside(motion_fixture, "throw", 0x17, 0.0)
    # A save/load-like position discontinuity must not determine gait.
    f32(motion_fixture["position"], 0, -1000)
    sample_inside(motion_fixture, "throw", 0x17, 0.0)

    # A switch retains its existing forward cycle even when facing differs.
    fixture = make_fixture(base, stock["switch"], action=3)
    f32(fixture["chassis"], 0x84, -1)
    f32(fixture["records"], 0x260, 1)
    sample_inside(fixture, "switch", 0x18)

    # Unsupported posture/action/motion states, lost owner links, and changed
    # clip bindings must all retain the original stock sample.
    def stock_only(fixture, label):
        fixture_by_facet[fixture["facet"]] = fixture
        fixture_config[fixture["facet"]] = {
            "inside": lambda f: check(
                max(abs(a - b) for a, b in zip(
                    invoke_sample(f, "Bip01_L_Thigh", "position", 0.73, True),
                    sample_linear(f["native_clip"].clip.by_name["Bip01_L_Thigh"].positions, 0.73))) < 2e-5,
                f"{label}: ineligible clip was overridden")
        }
        update(fixture["facet"], C.c_float(1 / 60))

    negative = [
        ("stationary", {"mode": 0, "route_state": 0, "posture": 1}),
        ("prone", {"mode": 1, "route_state": 2, "posture": 3}),
        ("kneeling", {"mode": 1, "route_state": 2, "posture": 2}),
        ("disabled", {"mode": 1, "route_state": 2, "posture": 0}),
        ("unowned route record", {"mode": 1, "route_state": 2, "posture": 1, "unowned": True}),
        ("idle route record", {"mode": 1, "route_state": 0, "posture": 1}),
        ("unsupported action", {"mode": 1, "route_state": 2, "posture": 1, "action": 0x44}),
    ]
    for label, values in negative:
        action_code = values.get("action", 0x17)
        fixture = make_fixture(base, stock["throw"], action=action_code,
                               mode=values["mode"], route_state=values["route_state"],
                               posture=values["posture"])
        if values.get("unowned"):
            C.c_uint8.from_address(fixture["records"] + 1).value = 0
        stock_only(fixture, label)

    # VirtualQuery protection is checked at both ownership capture and
    # sampler-time revalidation. The data page contains executable-looking
    # bytes but is PAGE_READWRITE, so it must never be called.
    nonexec_getter = alloc_readwrite(0x100)
    put_bytes(nonexec_getter, 0, bytes.fromhex("488b4168c3"))
    nonexec_entry = make_fixture(base, stock["throw"])
    p64(nonexec_entry["unit_vt"], 0xB0, nonexec_getter)
    stock_only(nonexec_entry, "non-executable getter at update entry")

    guarded_getter = alloc_readwrite(0x100)
    put_bytes(guarded_getter, 0, bytes.fromhex("488b4168c3"))
    old_guard_protection = W.DWORD()
    check(K.VirtualProtect(guarded_getter, 0x100, 0x104, C.byref(old_guard_protection)),
          f"cannot set PAGE_GUARD on test getter: {C.get_last_error()}")
    guarded_entry = make_fixture(base, stock["throw"])
    p64(guarded_entry["unit_vt"], 0xB0, guarded_getter)
    stock_only(guarded_entry, "PAGE_GUARD getter at update entry")
    restored = W.DWORD()
    check(K.VirtualProtect(guarded_getter, 0x100, old_guard_protection.value, C.byref(restored)),
          f"cannot restore test getter page protection: {C.get_last_error()}")

    nonexec_sampler = make_fixture(base, stock["throw"])
    fixture_by_facet[nonexec_sampler["facet"]] = nonexec_sampler
    def invalidate_getter_then_sample(f):
        p64(f["unit_vt"], 0xB0, nonexec_getter)
        value = invoke_sample(f, "Bip01_L_Thigh", "position", 0.73, True)
        expected = sample_linear(f["native_clip"].clip.by_name["Bip01_L_Thigh"].positions, 0.73)
        check(max(abs(a - b) for a, b in zip(value, expected)) < 2e-5,
              "sampler-time non-executable owner getter was called or accepted")
    fixture_config[nonexec_sampler["facet"]] = {"inside": invalidate_getter_then_sample}
    update(nonexec_sampler["facet"], C.c_float(1 / 60))

    wrong_owner = make_fixture(base, stock["throw"])
    other_chassis = alloc(0x300)
    p64(wrong_owner["records"], 0x258, other_chassis)
    stock_only(wrong_owner, "wrong navigation owner")
    mismatch = make_fixture(base, stock["throw"])
    p64(mismatch["components"], 0x58, alloc(0x100))
    stock_only(mismatch, "component/facet mismatch")
    mismatched_binding = make_fixture(base, stock["throw"])
    p64(mismatched_binding["binding"], 0x20, stock["switch"].animation)
    stock_only(mismatched_binding, "wrong bound animation")
    wrong_unit = make_fixture(base, stock["throw"])
    unrelated_components, unrelated_unit, unrelated_vtable = alloc(0x80), alloc(0x100), alloc(0x100)
    @C.CFUNCTYPE(C.c_void_p, C.c_void_p)
    def get_unrelated_components(_unit):
        return unrelated_components
    KEEP.append(get_unrelated_components)
    p64(unrelated_unit, 0, unrelated_vtable)
    p64(unrelated_unit, 0x68, unrelated_components)
    p64(unrelated_vtable, 0xB0, OWNER_GETTER)
    p64(wrong_unit["facet_junction"], 0x10, unrelated_unit)
    stock_only(wrong_unit, "wrong owning unit")

    mutated = NativeClip(read_clip(ASSETS / "stock_throw.anim"))
    mutated_fixture = make_fixture(base, mutated)
    thigh = mutated.track_records["Bip01_L_Thigh"]
    position_begin = ptr(thigh, 0x20)
    f32(position_begin, 4, C.c_float.from_address(position_begin + 4).value + 0.01)
    stock_only(mutated_fixture, "mutated stock clip key")

    # The renderer may still sample a transition's previous binding. It must
    # remain on its own stock clip while the current bound clip is overridden.
    previous_binding_case = make_fixture(base, stock["throw"])
    previous_clip = NativeClip(read_clip(ASSETS / "stock_switch.anim"))
    previous_binding = alloc(0x40)
    p64(previous_binding_case["geometry"], 0x1E0, previous_binding)
    p64(previous_binding, 0x20, previous_clip.animation)
    previous_fixture = dict(previous_binding_case)
    previous_fixture["native_clip"] = previous_clip
    fixture_by_facet[previous_binding_case["facet"]] = previous_binding_case
    previous_results = {}
    def sample_both_bindings(f):
        previous_results["current"] = invoke_sample(f, "Bip01_L_Thigh", "position", 0.73, True)
        previous_results["previous"] = invoke_sample(previous_fixture, "Bip01_L_Thigh", "position", 0.73, True)
    fixture_config[previous_binding_case["facet"]] = {
        "action_after": 0x17,
        "inside": sample_both_bindings,
    }
    update(previous_binding_case["facet"], C.c_float(1 / 60))
    current_expected = sample_linear(moving["throw"].by_name["Bip01_L_Thigh"].positions, 0.73)
    previous_expected = sample_linear(previous_clip.clip.by_name["Bip01_L_Thigh"].positions, 0.73)
    check(max(abs(a - b) for a, b in zip(previous_results["current"], current_expected)) < 2e-4,
          "current animation binding was not composited")
    check(max(abs(a - b) for a, b in zip(previous_results["previous"], previous_expected)) < 2e-5,
          "previous transition binding was composited")

    # Verify nested TLS restoration: unit B update samples B; after it returns,
    # unit A's original callback samples A while A's outer scope is still live.
    outer = make_fixture(base, stock["throw"])
    inner = make_fixture(base, stock["switch"], action=0x18)
    fixture_by_facet[outer["facet"]] = outer
    fixture_by_facet[inner["facet"]] = inner
    nested_results = []
    def inner_callback(f):
        nested_results.append(("inner", invoke_sample(f, "Bip01_L_Thigh", "position", 0.73, True)))
    def outer_callback(f):
        # Sampling another unit's vectors while A owns TLS must not borrow A's
        # active composite.
        other_unit_stock = invoke_sample(inner, "Bip01_L_Thigh", "position", 0.73, True)
        other_unit_expected = sample_linear(inner["native_clip"].clip.by_name["Bip01_L_Thigh"].positions, 0.73)
        check(max(abs(a - b) for a, b in zip(other_unit_stock, other_unit_expected)) < 2e-5,
              "outer TLS scope overrode another unit's animation vectors")
        fixture_config[inner["facet"]] = {"inside": inner_callback}
        update(inner["facet"], C.c_float(1 / 60))
        nested_results.append(("outer", invoke_sample(f, "Bip01_L_Thigh", "position", 0.73, True)))
    fixture_config[outer["facet"]] = {"inside": outer_callback}
    update(outer["facet"], C.c_float(1 / 60))
    check(len(nested_results) == 2, "nested update samples were not both executed")
    for kind, values in nested_results:
        expected_clip = moving["switch" if kind == "inner" else "throw"]
        expected = sample_linear(expected_clip.by_name["Bip01_L_Thigh"].positions, 0.73)
        check(max(abs(a - b) for a, b in zip(values, expected)) < 2e-4,
              f"nested {kind} TLS scope was not restored")

    # The diagnostic samples the post-update local node rotation after the
    # original animation callback. Exercise both a native callback that keeps
    # the composited thigh rotation and one that rewrites it to stock pose.
    def run_post_pose_case(label, rewrite_to_stock=False, unknown_getter=False):
        fixture = install_nodes(
            make_fixture(base, stock["throw"]),
            unknown_rotation_getter=unknown_getter,
        )
        fixture_by_facet[fixture["facet"]] = fixture
        stock_rotation = sample_slerp(
            fixture["native_clip"].clip.by_name["Bip01_L_Thigh"].rotations,
            0.73,
            order="wxyz",
        )

        def apply_pose(f):
            sampled = invoke_sample(f, "Bip01_L_Thigh", "rotation", 0.73, True)
            if unknown_getter:
                # The no-access page makes an accidental read of the unknown
                # NodeImpl field fail the test; source should reject by prefix.
                return
            pose = stock_rotation if rewrite_to_stock else sampled
            for component, value in enumerate(pose):
                f32(f["thigh_node"], 0x3DC + component * 4, value)

        fixture_config[fixture["facet"]] = {"inside": apply_pose}
        snapshots = (
            C.string_at(fixture["chassis"], 0x300),
            C.string_at(fixture["records"], 0x2F0),
            C.string_at(fixture["native_clip"].animation, 0x20),
            fixture["native_clip"].snapshot(),
            C.string_at(fixture["binding"], 0x60),
        )
        update_count = len([call for call in original_calls if call[0] == "update"])
        rotation_count = len([call for call in original_calls if call[0] == "rotation"])
        for _ in range(32):
            update(fixture["facet"], C.c_float(1 / 60))
        check(len([call for call in original_calls if call[0] == "update"]) == update_count + 32,
              f"{label}: original update callback count changed")
        check(len([call for call in original_calls if call[0] == "rotation"]) == rotation_count + 32,
              f"{label}: original rotation callback was not forwarded exactly once per frame")
        check(C.string_at(fixture["chassis"], 0x300) == snapshots[0]
              and C.string_at(fixture["records"], 0x2F0) == snapshots[1]
              and C.string_at(fixture["native_clip"].animation, 0x20) == snapshots[2]
              and fixture["native_clip"].snapshot() == snapshots[3]
              and C.string_at(fixture["binding"], 0x60) == snapshots[4],
              f"{label}: diagnostic changed gameplay or animation state")
        line = next((text for _, text in reversed(logs)
                     if "moving action leg phase:" in text
                     and f"chassis={fixture['chassis']:#x}" in text), None)
        check(line is not None, f"{label}: bounded phase diagnostic was not emitted")
        fields = dict(part.split("=", 1) for part in line.split() if "=" in part)
        check(fields.get("post_pose_samples") == ("0" if unknown_getter else "32"),
              f"{label}: unexpected post-pose getter sample count: {line}")
        mismatch_text = fields.get("latest_pose_mismatch_deg")
        if unknown_getter:
            check(mismatch_text == "unavailable", f"{label}: unknown getter was read/accepted: {line}")
            page, protection = fixture["guarded_pose_page"]
            restored = W.DWORD()
            check(K.VirtualProtect(page, 0x1000, protection, C.byref(restored)),
                  f"cannot restore unknown-getter guard page: {C.get_last_error()}")
        elif rewrite_to_stock:
            check(mismatch_text not in (None, "unavailable") and float(mismatch_text) > 10.0,
                  f"{label}: post-callback stock rewrite was not detected: {line}")
        else:
            check(mismatch_text not in (None, "unavailable") and float(mismatch_text) < 0.1,
                  f"{label}: retained sampled pose was reported as mismatched: {line}")

    run_post_pose_case("retained composite pose")
    run_post_pose_case("post-callback stock rewrite", rewrite_to_stock=True)
    run_post_pose_case("unknown getter prefix", unknown_getter=True)

    # Last check: each sample hook forwarded once with the same pointers/time;
    # original update also saw its exact facet and dt exactly once per call.
    check(len(hooks) == 3 and len(hook_detours) == 3, "hook set changed during runtime tests")
    check(all(call[1] and call[2] for call in original_calls if call[0] in ("position", "rotation")),
          "sampler trampoline received a null pointer")
    print(f"PASS {BUILD_NAME} moving-actions animation: ABI/assets/signatures, external executable getter validation, rollback, "
          "post-update action scope, 17/18/2b samples, standing route gates, native forwarding, "
          "invalidations, nested TLS restoration and post-IK pose diagnostics")


if __name__ == "__main__":
    main()
