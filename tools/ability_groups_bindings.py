"""Reproduce the ability-groups binding table from every supported build's DLLs.

Run from the repository root. The anchors are GOG 2026-09-25 `game.dll`
addresses; each is found in every supported build by a
signature taken there, and the order-panel helpers through the mine button's
click, which calls them. Each function's code is checked for the offsets
`plugins/ability-groups/src/native.rs` uses as constants, and the game menu's
submenu fields are read from the code that clears them.
"""
import hashlib
import builds
from pathlib import Path
from sigs import Module
from rustfmt import rustfmt

ROOT = Path(__file__).resolve().parent.parent
ANCHOR_BUILD = "gog-2026-09-25"
# Hooked, then called: each is (bar, ...) or a GUI object, as native.rs says.
HOOKS = dict(update=0x1cec0, reset=0x1cdf0, key=0x1fd40, bar_destroy=0x1acd0,
             order_key=0x23d900, click=0x2b0d30)
# Called only.
CALLS = dict(hide=0x1d0f0, move_widget=0x2d3ab0, label=0x241c70, close_submenus=0x1cd80)
# The mine button's click lambda: opening its submenu, it calls these three on
# the game menu in this order, then stores 1 at the order panel's hidden flag.
MINE_CLICK = 0x1f8c0
MINE_CALLS = ("clear_order", "order_reset", "hide_orders")
# The game menu's order key registration, per order button binding (`r15` is
# binding + 0x20, where it writes the `slot<cell>` name it registers): these
# instructions store the button at binding + 0x40 and pass the binding itself
# to order_key.
ORDER_BINDING = {0x23eacf: "mov qword ptr [r15 + 0x20], rax",
                 0x23ec21: "lea r12, [r15 - 0x20]"}
# Code each function must contain, for the offsets native.rs reads.
SHAPES = {
    "update": ["+ 0x144]", "+ 0x88]", "+ 0x38]", "+ 0x20]", "+ 0x10]", "+ 0x48]", "+ 0x5b]",
               "call qword ptr [rax + 0x40]"],
    "reset": ["+ 0x78]", "+ 0x80]", "+ 0x88]", "+ 0xe8"],
    "key": ["+ 0x88]", "+ 0x10]", "+ 0x26c]", "+ 0x68]", "call qword ptr [rax + 0x108]",
            "call qword ptr [rax + 0xf0]"],
    "order_key": ["+ 0x40]", "+ 0x189]"],
    "hide": ["call qword ptr [rax + 0x18]"],
    # The mine menu (bar + 0x78) closed through its vtable, the airstrike menu
    # (bar + 0x80) directly.
    "close_submenus": ["+ 0x78]", "+ 0x80]", "call qword ptr [rax + 0x18]"],
}
MIN_DISPLACED = 14


def code(img, rva, limit=0x800):
    """The function at `rva` as text, read on to its int3 padding (.pdata can
    split a function into chunks)."""
    lines = []
    for ins in img.disasm(rva, rva + limit):
        if ins.mnemonic == "int3":
            break
        lines.append(f"{ins.mnemonic} {ins.op_str}")
    return lines


def site(img, rva, hooked):
    """The whole instructions from `rva` covering MIN_DISPLACED bytes, which
    install compares and a hook displaces: a hook's may not branch or address
    relative to rip."""
    span = bytearray()
    for ins in img.disasm(rva, rva + 64):
        movable = "rip" not in ins.op_str and not ins.mnemonic.startswith(("j", "call", "loop"))
        assert movable or not hooked, (hex(rva), ins.mnemonic, ins.op_str)
        span.extend(ins.bytes)
        if len(span) >= MIN_DISPLACED:
            return bytes(span)
    raise ValueError(f"{rva:#x}: no displaced span")


def locate(source, target, anchors):
    found = {}
    for name, rva in anchors.items():
        start, pattern, mask = source.signature(rva, [])
        hits = target.matches(pattern, mask)
        if len(hits) != 1:
            raise ValueError(f"{name}: expected one match, got {[hex(h) for h in hits]}")
        found[name] = hits[0] + rva - start
    return found


def table(source, target):
    img = target.functions
    found = locate(source, target, {**HOOKS, **CALLS, "mine_click": MINE_CLICK})
    binding = locate(source, target, {text: rva for rva, text in ORDER_BINDING.items()})
    for text, rva in binding.items():
        ins = next(iter(img.disasm(rva, rva + 16)))
        assert f"{ins.mnemonic} {ins.op_str}" == text, (hex(rva), text)
    for name, needles in SHAPES.items():
        lines = code(img, found[name])
        for needle in needles:
            assert any(needle in line for line in lines), (name, needle)
    lines = code(img, found.pop("mine_click"))
    assert "mov rcx, qword ptr [rbx + 0x50]" in lines, "the game menu at bar + 0x50"
    calls = [int(line.split()[1], 16) for line in lines if line.startswith("call 0x")]
    found.update(zip(MINE_CALLS, calls[:3]))
    # The order reset clears the panel's hidden flag and the submenu state last.
    reset = code(img, found["order_reset"])
    hidden = next(l for l in reset if l.startswith("mov byte ptr [rbx + ") and l.endswith("], 0")
                  and "0x89c" not in l)
    state = next(l for l in reset if l.startswith("mov dword ptr [rbx + ") and l.endswith("], 0"))
    offset = lambda line: int(line.split("+ ")[1].split("]")[0], 16)
    return found, offset(hidden), offset(state)


def generate():
    source = Module(builds.build(ANCHOR_BUILD).require().game)
    out = ["// Generated by tools/ability_groups_bindings.py; do not hand edit.",
           "use super::{Build, Site};", "pub(super) const BUILDS: &[Build] = &["]
    for build in sorted(builds.supported(), key=lambda b: b.name):
        path = build.require().game
        target = Module(path)
        found, hidden, state = table(source, target)
        out.append("    Build {")
        out.append(f'        name: "{build.name}",')
        out.append(f'        sha: "{hashlib.sha256(path.read_bytes()).hexdigest()}",')
        for name in (*HOOKS, *CALLS, *MINE_CALLS):
            data = ", ".join(f"0x{b:02x}" for b in site(target.functions, found[name], name in HOOKS))
            out.append(f"        {name}: Site {{ rva: 0x{found[name]:x}, before: &[{data}] }},")
        out.append(f"        orders_hidden: 0x{hidden:x},")
        out.append(f"        submenu: 0x{state:x},")
        out.append("    },")
    out.append("];\n")
    return "\n".join(out)


if __name__ == "__main__":
    path = ROOT / "plugins/ability-groups/src/sites.rs"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(generate(), encoding="utf-8")
    rustfmt(path)
    print(path)
