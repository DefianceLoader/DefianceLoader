"""Per-build layout substitution: operand shapes, symbol tables and the audit
that reports a layout entry which never applied."""
import sys
sys.path.insert(0, "tools")
import build as b

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

print(f"\n{failures} failed" if failures else "\nall layout checks passed")
sys.exit(1 if failures else 0)
