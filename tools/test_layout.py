"""Per-build layout substitution: operand shapes, symbol tables and the audit
that reports a layout entry which never applied."""
import sys
sys.path.insert(0, "tools")
import build as b
import payload
import units

failures = 0


def check(label, ok):
    global failures
    print(f"{'PASS' if ok else 'FAIL'}  {label}")
    failures += not ok


def refused(call):
    try:
        call()
    except SystemExit:
        return True
    return False


# A simple displacement is rewritten through the table.
layout = {"mov|rbx|0x10": 0x20}
check("a plain field is remapped", b.apply_layout("mov rax, [rbx + 0x10]", layout) == "mov rax, [rbx + 0x20]")
check("an unmapped field is untouched", b.apply_layout("mov rax, [rbx + 0x18]", layout) == "mov rax, [rbx + 0x18]")
check("a call's slot is remapped", b.apply_layout("call qword ptr [rcx + 0x10]", {"call|rcx|0x10": 0x18})
      == "call qword ptr [rcx + 0x18]")

# A negative displacement keys on its signed value.
negative = {"mov|rbx|-0x10": 0x0}
check("a negative displacement is remapped", b.apply_layout("mov rax, [rbx - 0x10]", negative) == "mov rax, [rbx + 0x0]")

# Stack frames never move.
check("a stack frame is left alone", b.apply_layout("mov [rsp + 0x10], rax", {"mov|rsp|0x10": 0x20})
      == "mov [rsp + 0x10], rax")

# An indexed operand a table entry claims is refused, not silently rewritten:
# the key ignores the index, so it may be another class's field.
check("an indexed operand a key claims is refused",
      refused(lambda: b.apply_layout("call qword ptr [rax + rcx*4 + 0x10]", {"call|rax|0x10": 0x20})))
check("an indexed operand with no key is left alone",
      b.apply_layout("call qword ptr [rax + rcx*4 + 0x10]", {"call|rax|0x18": 0x20})
      == "call qword ptr [rax + rcx*4 + 0x10]")

# A named constant from the active symbol table is substituted.
code, _ = b.assemble(["mov rax, [rcx + {thing}]"], 0, 0, layout={}, symbols={"thing": 0x38})
check("a symbol is substituted", code == bytes.fromhex("488b4138"))

# The audit reports an entry whose reference displacement appears on a
# different mnemonic or base, and stays quiet about one that applied.
audit = {"call|rbx|0x98": 0xa0, "call|rax|0x98": 0xa0}
b.assemble(["call qword ptr [rbx + 0x98]"], 0, 0, layout=audit)
gaps = dict(b.layout_gaps_for(audit))
check("an applied entry is not reported", "call|rbx|0x98" not in gaps)
check("a near-miss is reported", "call|rax|0x98" in gaps)

# A class defined in one module but reached from the other has its moved slot
# only in that module's map; the active layout will not rewrite it, so it must
# be reported for a shared symbol.
other = {"call|rax|0x3b8": 0x3d0}
check("a cross-DLL moved slot is reported",
      [g[2] for g in b.cross_dll_gaps([("game", ["call qword ptr [rax + 0x3b8]"])], {}, other)]
      == ["call|rax|0x3b8"])
check("a cross-DLL slot already using the new offset is not",
      b.cross_dll_gaps([("game", ["call qword ptr [rax + 0x3d0]"])], {}, other) == [])
check("a slot in the active module is not reported",
      b.cross_dll_gaps([("game", ["call qword ptr [rax + 0x3b8]"])], other, other) == [])


class SyntheticImage:
    def __init__(self, code):
        self.data = code
        self.sections = [(".text", 0x2000, len(code), 0)]

    def read(self, rva, size):
        offset = rva - 0x2000
        return self.data[offset:offset + size] if 0 <= offset < len(self.data) else b""

    def function_of(self, rva):
        return (0x2000, 0x2400) if 0x2000 <= rva < 0x2400 else None


def code_with_passenger_calls(valid, decoys=()):
    code = bytearray(b"\x90" * 0x1000)
    for offset in (*valid, *decoys):
        hook = offset + 11
        original = 0x700 if offset in valid else 0x900
        code[offset:offset + 3] = bytes.fromhex("488bc8")
        code[offset + 3:offset + 8] = bytes.fromhex("e800000000")
        code[offset + 8:hook] = bytes.fromhex("488bcb")
        code[hook:hook + 5] = b"\xe8" + (original - hook - 5).to_bytes(4, "little", signed=True)
        code[hook + 5:hook + 11] = bytes.fromhex("4c8be04885c0")
    code[0x700:0x703] = bytes.fromhex("48895c")
    return bytes(code)


call_image = SyntheticImage(code_with_passenger_calls((7,), (0x300,)))
call_offset = 7 + 11
call_before = b"\xe8" + (0x700 - call_offset - 5).to_bytes(4, "little", signed=True)
check("the passenger resolver finds the source-gun call and ignores decoys",
      payload.vehicle_special_fire_source_call(call_image) ==
      (0x2000 + call_offset, call_before, 0x2700))
no_call_image = SyntheticImage(bytes(0x80))
check("the passenger resolver rejects a missing source-gun call",
      refused(lambda: payload.vehicle_special_fire_source_call(no_call_image)))
ambiguous_image = SyntheticImage(code_with_passenger_calls((7, 0x300)))
check("the passenger resolver rejects ambiguous source-gun calls",
      refused(lambda: payload.vehicle_special_fire_source_call(ambiguous_image)))

def code_with_disembark_calls(offsets, original=0x2700):
    code = bytearray(b"\x90" * 0x1000)
    entry = bytes.fromhex("4885d20f84000000005541564883ec28488be94c8bf2")
    code[0x20:0x20 + len(entry)] = entry
    arguments = bytes.fromhex("33d2488bcb4c897c2420e800000000488bcb")
    after = bytes.fromhex(
        "488bd84885c0741e488b4d20488b11ff92b0000000488b48284885c97408488bd3e8")
    for offset in offsets:
        code[offset - len(arguments):offset] = arguments
        code[offset:offset + 5] = b"\xe8" + (original - 0x2000 - offset - 5).to_bytes(
            4, "little", signed=True)
        code[offset + 5:offset + 5 + len(after)] = after
    return bytes(code)


disembark_image = SyntheticImage(code_with_disembark_calls((0xb0, 0x500)))
disembark_before = b"\xe8" + (0x2700 - 0x20b0 - 5).to_bytes(4, "little", signed=True)
check("the disembark selector resolver stays between its native entry and tail",
      payload.vehicle_special_fire_disembark_call(disembark_image, 0x23d0, 0x2700)
      == (0x20b0, disembark_before))
check("the disembark selector resolver rejects a missing selector",
      refused(lambda: payload.vehicle_special_fire_disembark_call(
          SyntheticImage(code_with_disembark_calls(())), 0x23d0, 0x2700)))
check("the disembark selector resolver rejects ambiguous selectors",
      refused(lambda: payload.vehicle_special_fire_disembark_call(
          SyntheticImage(code_with_disembark_calls((0xb0, 0x1b0))), 0x23d0, 0x2700)))
check("the disembark selector resolver rejects a different native chooser",
      refused(lambda: payload.vehicle_special_fire_disembark_call(disembark_image, 0x23d0, 0x2900)))

timer_pattern = payload.SPECIAL_DYNAMIC_RESET + bytes.fromhex("488b435848394350")
timer_code = bytearray(b"\x90" * 0x800)
timer_code[0x70:0x70 + len(timer_pattern)] = timer_pattern
timer_code[0x600:0x600 + len(timer_pattern)] = timer_pattern
check("the mount timer resolver stays within the guarded gunner function",
      payload.vehicle_special_fire_dynamic_site(SyntheticImage(timer_code), 0x2300)
      == (0x2070, payload.SPECIAL_DYNAMIC_RESET, 0x207a))
check("the mount timer resolver rejects a missing reset",
      refused(lambda: payload.vehicle_special_fire_dynamic_site(no_call_image, 0x2300)))
timer_code[0x90:0x90 + len(timer_pattern)] = timer_pattern
check("the mount timer resolver rejects ambiguous resets in the gunner",
      refused(lambda: payload.vehicle_special_fire_dynamic_site(SyntheticImage(timer_code), 0x2300)))
check("the mount timer resolver rejects a rebind site without a function",
      refused(lambda: payload.vehicle_special_fire_dynamic_site(SyntheticImage(timer_code), 0x2800)))

def code_with_client_events(offsets):
    code = bytearray(b"\x90" * 0x800)
    before = bytes.fromhex("418b4744c1e80c4533e4a8017451")
    tail = bytes.fromhex("4c8924f8418b4744c1e802a801")
    for offset in offsets:
        code[offset - len(before):offset] = before
        code[offset:offset + len(payload.SPECIAL_DYNAMIC_CLIENT)] = payload.SPECIAL_DYNAMIC_CLIENT
        code[offset + 0x4d:offset + 0x4d + len(tail)] = tail
    return code


client_code = code_with_client_events((0x70, 0x600))
check("the client mount resolver derives its validated event continuation",
      payload.vehicle_special_fire_client_site(SyntheticImage(client_code))
      == (0x2070, bytes.fromhex("498b5f60488bc3"), 0x2077, 0x20c1))
check("the client mount resolver rejects a missing event",
      refused(lambda: payload.vehicle_special_fire_client_site(no_call_image)))
check("the client mount resolver rejects ambiguous events",
      refused(lambda: payload.vehicle_special_fire_client_site(
          SyntheticImage(code_with_client_events((0x70, 0x170))))))
client_code[0x70 + 0x4d] = 0x90
check("the client mount resolver rejects an inconsistent native event tail",
      refused(lambda: payload.vehicle_special_fire_client_site(SyntheticImage(client_code))))

special_fire_descriptor = {"pose_calls": [
    {"pose_feature": 10, "pose_site": 0x1000, "pose_before": "e800000000"},
    {"pose_feature": 11, "pose_site": 0x47b50b, "pose_before": "e8800ce4ff"},
    {"pose_feature": 11, "pose_site": 0x47bef0, "pose_before": "e89b02e4ff"},
], "detours": [{"hook_feature": 11, "hook_rva": 0x2a81c5,
               "hook_displaced": "85f60f88c2000000"},
               {"hook_feature": 11, "hook_rva": 0x47b81b,
                "hook_displaced": "488b5c2478"},
               {"hook_feature": 11, "hook_rva": 0x47c22b,
                "hook_displaced": "488b742448"},
               {"hook_feature": 11, "hook_rva": 0x2a80a2,
                "hook_displaced": "c783bc0000000000803f"},
               {"hook_feature": 11, "hook_rva": 0x1ccdd0,
                "hook_displaced": "498b5f60488bc3"}],
 "vehicle_special_fire_helpers": {"refresh_ai": 0xcf650},
 "vehicle_special_fire_client_skip": 0x1cce21}
special_fire_symbols = units.vehicle_special_fire_symbol_overrides(special_fire_descriptor, 11)
check("the special-fire unit resolves the native chooser and transport continuations",
      special_fire_symbols == {"vehicle_special_fire_original": 0x2bc190,
                               "vehicle_special_fire_guard_resume": 0x2a81cd,
                               "vehicle_special_fire_guard_skip": 0x2a828f,
                               "vehicle_special_fire_source": units.UNIT_BASE,
                               "vehicle_special_fire_dynamic_resume": 0x2a80ac,
                               "vehicle_special_fire_dynamic_client_resume": 0x1ccdd7,
                               "vehicle_special_fire_dynamic_client_skip": 0x1cce21,
                               "vehicle_special_fire_board_resume": 0x47b820,
                               "vehicle_special_fire_disembark_resume": 0x47c230,
                               "vehicle_special_fire_refresh_ai": 0xcf650})
special_fire_spec = units.UNITS["vehicle-special-fire"][units.LOGIC]
special_fire_blob, _, _, special_fire_code = units.assemble_unit(
    "vehicle-special-fire", units.LOGIC, special_fire_spec, special_fire_symbols)
special_fire_fixups = units.code_fixups(
    "vehicle-special-fire-logic", special_fire_blob, special_fire_code)
check("the assembled special-fire unit carries its native call and tail fixups",
      {fixup["target"] for fixup in special_fire_fixups if fixup["kind"] == "rel32"}
      == set(special_fire_symbols.values()) - {units.UNIT_BASE})
check("the special-fire unit rejects an unexpected call signature",
      refused(lambda: units.vehicle_special_fire_symbol_overrides(
          {**special_fire_descriptor,
           "pose_calls": [{"pose_feature": 11, "pose_site": 0x1000,
                           "pose_before": "0f845f010000"},
                          special_fire_descriptor["pose_calls"][2]]}, 11)))
check("the special-fire unit rejects selectors with different native targets",
      refused(lambda: units.vehicle_special_fire_symbol_overrides(
          {**special_fire_descriptor,
           "pose_calls": [special_fire_descriptor["pose_calls"][1],
                          {"pose_feature": 11, "pose_site": 0x47bef0,
                           "pose_before": "e800000000"}]}, 11)))
check("the special-fire unit requires both boarding and disembark selectors",
      refused(lambda: units.vehicle_special_fire_symbol_overrides(
          {**special_fire_descriptor,
           "pose_calls": [special_fire_descriptor["pose_calls"][1]]}, 11)))

print(f"\n{failures} failed" if failures else "\nall layout checks passed")
sys.exit(1 if failures else 0)
