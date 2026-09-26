"""Assemble the patches into a copy of logic.dll.

  1. one section is appended, `.patch`, code/execute/read/write, holding the
     pickup chooser at its start, the move filter at MOVE_OFFSET and the
     rotation cursor at CURSOR_OFFSET. The file grows by that page;
  2. the pickup call site is retargeted to the chooser, leaving the stock one
     in place for the three query callers, and the move distribution gets a
     detour over one displaced instruction;
  3. a RUNTIME_FUNCTION and UNWIND_INFO per routine go into .pdata's own
     padding, so the new frames unwind rather than being mistaken for leaves;
  4. the selection getter and manager paths are edited in place, the setter
     becomes a jmp to its replacement, and the pose sites call the pose split.
"""
import hashlib, json, os, pathlib, re, struct, sys
import builds
import keystone
import pefile
sys.path.insert(0, "tools")
from pe import Image

SRC = str(builds.reference().logic)
DST = "out/logic.dll"
MANIFEST = "out/manifest.json"

# Every address and structure offset below was read out of this exact build.
# A game update replaces logic.dll, and nothing about a new one can be assumed,
# so the source is refused rather than patched blind.
EXPECT_SOURCE_SHA = "17ef48350153306e210e14a24b0ad398c56fb99d3d46c88f246df260ade85780"
# Other builds the signatures have been checked against (tools/sigs.py and
# --scan-check), which the injector then relocates to without --scan. Steam:
# the same source built 14 minutes after the GOG one, 23 Dec 2025.
VERIFIED_BUILDS = {
    "d320f848508c45c9f04df235204b5fbc9ffbb1b7f869e4c5a58d80bb2ecc10ed": "Steam",
}

# The chooser gets a section of its own rather than .text's alignment padding
# (free, but capped at 417 bytes): a call rel32 reaches anywhere within 2GB,
# and the cursor is addressed rip-relative at a fixed offset inside the same
# block, so the block is self-contained and could sit anywhere. The padding is
# left untouched.
BLOCK_SIZE = 0x3000
STATE_CHARS = 0xE0000020              # code, execute, read, write
CURSOR_OFFSET = 0x2f00                # clear of any plausible code size
CALL_SITE = 0x43b7d8                  # call fn_43e0f0 in the pickup order path
STOCK_CHOOSER = 0x43e0f0
PDATA_DIR = 3

STATE_NAME = b".patch\x00\x00"

CHOOSER_PUSHES = ["rbx", "rbp", "rsi", "rdi", "r12", "r13", "r14", "r15"]
CHOOSER_FRAME = 0x38

# The move filter: fn_439f20 collects a squad's members into a command-local
# vector and then allocates formation destinations and one order per member, so
# a move always moves the whole squad however few soldiers are selected. The
# filter runs at the boundary after that collection and before the empty check,
# and narrows the vector to the marked members when the marks discriminate.
MOVE_OFFSET = 0x400                   # inside the block, clear of the chooser
MOVE_CALL_SITE = 0x43a032
MOVE_DISPLACED = bytes.fromhex("488b7c2430")   # mov rdi, qword ptr [rsp+0x30]
# Diagnostic: detours on the selection manager's select, toggle, deselect and
# clear-all, which record who called and with what into one tagged ring, so a
# gesture's calls can be read back in order. Injector only.
TRACE_CODE_OFFSET = 0x600
TRACE_OFFSET = 0x1000                 # index, then 32 sixteen-byte entries
TRACE_HOOKS = [
    # rva, the displaced bytes, the stub's label, its resume placeholder
    (0x418cb0, "40534881ecc0000000", "select_trace", 0xbbbbbbbbbbbbbbb1),
    (0x418f40, "48895c2408", "toggle_trace", 0xbbbbbbbbbbbbbbb2),
    (0x419350, "4883ec28488b02", "deselect_trace", 0xbbbbbbbbbbbbbbb3),
    (0x4194f0, "48895c2408", "clear_trace", 0xbbbbbbbbbbbbbbb4),
]

MOVE_PUSHES = ["rbx", "rbp", "rsi", "rdi", "r12", "r13"]
MOVE_FRAME = 0x38

# Individual selection: remember which soldier was clicked, without giving up
# the squad selection that the panel and the orders are built on.
#
# SquadUnitSelectableFacet forwards both ways through +0x28, the squad's
# selectable facet, and carries a branch for the case where +0x28 is absent
# that uses the soldier's own selected flag at +0x30. Taking only that branch
# selects the soldier but leaves the squad unselected, and the squad selection
# is what tells the client to show the squad panel and where to send orders —
# a lone soldier then has no UI and cannot be commanded. Buildings do use the
# own-flag branch, but they have no squad; a soldier still carries one.
#
# So setSelected writes the flag and then selects the squad, keeping the panel
# and the orders working, and isSelected reports the soldier's flag so the
# clicked soldier is the one marked inside the squad. Unmarking one of several
# marked members must not select-away the squad they still belong to, but
# unmarking the last one has to, or the squad stays selected with nobody
# marked and a move order moves all of it. That needs the squad's members, so
# setSelected is patch/soldier-mark.asm, in the block, reached by a jmp laid
# over the stock function (SETTER_RVA); it also clears stale marks when a
# soldier is marked into a squad that is not selected.
# isSelected must still require its parent squad to be selected: a direct
# squad deselect must not leave a soldier visibly selected. Preserve that
# guard, then take the existing enabled-and-own-flag branch as well.
SETTER_OFFSET = 0x800                 # inside the block, after the trace stubs
SETTER_RVA = 0x4491a0
SETTER_STOCK = bytes.fromhex("488bc1488b49284885c97407488b0148ff6050885030c3cccccccc")
SETTER_PUSHES = ["rbx", "rsi", "rdi", "r12", "r13", "r14"]
SETTER_FRAME = 0x28
# The getter is replaced too: the selected-unit UI can hold a soldier facet
# whose squad object is gone, and the stock code calls through the stale
# parent's zero vtable. The replacement answers from the soldier's own bytes
# in that case.
IS_SELECTED_RVA = 0x4491c0
IS_SELECTED_STOCK = bytes.fromhex("40534883ec20")

# Lie down and stand up. The stock lie-down (fn_1054c0) and stand-up
# (fn_105620) are reached from net::ChangePoseCommand's handler, fn_31ee20, and
# from the AiUtilsImpl thunks single-player's panel calls directly. All four
# sites go to patch/pose.asm, which orders only the marked soldiers when the
# marks pick out part of a squad. It records into the trace ring, which in the
# file patch is simply unused space in the block.
POSE_OFFSET = 0xa00                   # inside the block, after the setter
POSE_PUSHES = ["rbx", "rbp", "rsi", "rdi", "r12", "r13", "r14", "r15"]
POSE_FRAME = 0x28
POSE_CALLS = [
    # the site, the stock function it reaches, the entry it is sent to, and
    # whether it is a thunk's tail jmp rather than the handler's call
    (0x31eeb9, 0x1054c0, "pose_down", False),
    (0x31eec0, 0x105620, "pose_up", False),
    # AiUtilsImpl vt+0x70 and vt+0x78, which single-player's panel calls
    (0x10f713, 0x1054c0, "pose_down_direct", True),
    (0x10f723, 0x105620, "pose_up_direct", True),
]


# Per-soldier posture: gates on the two functions that change a soldier's
# posture, fn_2aff10 (lay down) and fn_2b0200 (stand up), which refuse a change
# that contradicts the soldier's pin (patch/posture-gate.asm). Each is entered
# by a jmp over its first two instructions and jumps back past them.
POSTURE_OFFSET = 0x200                # inside the block, between chooser and filter
POSTURE_HOOKS = [
    # rva, the displaced push rbx / sub rsp, imm8, the gate
    (0x2aff10, bytes.fromhex("40534883ec70"), "lie_down_gate"),
    (0x2b0200, bytes.fromhex("40534883ec60"), "stand_up_gate"),
]
# The two movement states' read of the squad's flag, movzx ebp, byte [rcx+0x29e],
# becomes a call to move_posture and two nops, so a walk follows the soldier's
# own pin: crawl when pinned prone.
MOVE_POSTURE_READ = bytes.fromhex("0fb6a99e020000")
MOVE_POSTURE_SITES = [0xcffc6, 0xd0626]
MOVE_POSTURE_PUSHES = ["rax", "rcx", "rdx", "r8", "r9", "r10", "r11"]
# The squad's stand-up loop (fn_43d5a0) asks each member's posture whether to
# stand him, cmp qword [rbx+0x28], 0; the compare becomes a call to
# squad_stand_gate, which skips a soldier pinned prone.
SQUAD_STAND_READ = bytes.fromhex("48837b2800")
SQUAD_STAND_SITES = [0x43d639]
# "Is it prone?" (AiUtilsImpl vt+0x2c0, fn_110bb0), which the squad panel asks
# to choose between lie down and stand up, answered for the picked soldiers.
PRONE_OFFSET = 0xd00                  # inside the block, after the pose split
PRONE_HOOK = (0x110bb0, bytes.fromhex("40534883ec20"), "prone_query")

# Firing mode (T) per soldier: patch/firing-mode.asm. The squad's setter and
# getter (SquadAiFacet vt+0x388, vt+0x380) and the soldier's getter
# (HumanAiFacet vt+0x380) start with jmps to it, the soldier's getter's tail
# call to the squad's becomes a jmp to squad_flag, and fn_2caeb0's direct call
# to the squad's getter becomes a call to firing_call.
FIRING_OFFSET = 0x1600                # inside the block, after the census
FIRING_PUSHES = ["rbx", "rbp", "rsi", "rdi", "r12", "r13", "r14", "r15"]
FIRING_HOOKS = [
    # rva, the bytes displaced, the entry
    (0x436ff0, bytes.fromhex("889128020000c3"), "firing_set"),
    (0x436fe0, bytes.fromhex("0fb68128020000c3"), "firing_ui"),
    (0x2ae8f0, bytes.fromhex("48895c2408"), "firing_soldier"),
    (0x2ae9ac, bytes.fromhex("48ffa080030000"), "squad_flag"),
]
FIRING_CALLS = [
    # rva, the call there now, the entry it is sent to
    (0x2caf8b, bytes.fromhex("ff9080030000"), "firing_call"),
]
# Diagnostic, injector only: who reads a squad's behaviour (G), counted by a
# replacement of its getter (patch/behaviour-census.asm) into a table.
CENSUS_TABLE = 0x1300                 # 32 entries of caller and count
CENSUS_OFFSET = 0x1500
CENSUS_HOOK = (0x436f30, bytes.fromhex("8b8100020000c3"), "census_behaviour")

# Ammo data is shared by the squad. Per-soldier pins override its disabled
# flag at the UI getter, roster gate, loaded-round queries and resupply reads.
# Factory hooks reset pins on new/load, including recycled allocations.
AMMO_OFFSET = 0x1a00                  # inside the block, after firing mode
AMMO_PUSHES = ["rbx", "rbp", "rsi", "rdi", "r12", "r13", "r14", "r15"]
AMMO_FRAME = 0x48                     # ammo_set, whose frame carries the trace
AMMO_GET_FRAME = 0x38                 # ammo_get, which does not
AMMO_HOOKS = [
    # rva, the bytes displaced, the entry
    (0x11e460, bytes.fromhex("4889542410"), "ammo_set"),
    (0x11e740, bytes.fromhex("40534883ec20"), "ammo_get"),
    (0x287160, bytes.fromhex("80b9e200000000"), "gun_ready"),
    (0x287190, bytes.fromhex("80b9e000000000"), "gun_can_fire"),
    (0x2871b0, bytes.fromhex("83b9dc00000000"), "gun_empty"),
    (0x2871c0, bytes.fromhex("8b81dc000000"), "gun_loaded"),
    (0x25c971, bytes.fromhex("48c7472800000000"), "ammo_init_new"),
    (0x25cabb, bytes.fromhex("48c7472800000000"), "ammo_init_load"),
]
# Hook the comparisons themselves, including the pre-scan loop backedge.
# Capture the member facet in this invocation's unused stack slot before RSI
# becomes shared ammo data; every displaced instruction and resume is covered.
AMMO_READER_HOOKS = [
    (0x115a50, bytes.fromhex("837910007507"), "ammo_reader_check1"),
    (0x115a89, bytes.fromhex("837a3c010f847c010000"), "ammo_reader_check2"),
    (0x1158aa, bytes.fromhex("488b70284885f6"), "ammo_reader_owner"),
]
# Diagnostic counters only; no game pointer from this cell is dereferenced.
AMMO_SCRATCH = 0x2700
# The identified call+test sites for the ammo gate, retargeted to a shim that puts the gun
# in rdi and then filters the shared answer through the firing soldier's pin.
# rva, the displaced call + test, the entry. The gun is rdi, rbx or r13.
AMMO_GATE_CALLS = [
    (0x28ac93, "ff506085c0", "ammo_gate_body"),
    (0x28a72c, "ff506085c0", "gate_from_r13"),
    (0x28a46d, "ff506085c0", "gate_from_rbx"),
    (0x28a30a, "ff506085c0", "gate_from_rbx"),
    (0x28c5dd, "ff506085c0", "gate_from_rbx"),
    (0x28e525, "ff506085c0", "gate_from_rbx"),
    (0x28b2bd, "ff506085c0", "ammo_gate_body"),
    (0x28a867, "ff506085c0", "gate_from_r15"),
    (0x28a8c6, "ff506085c0", "gate_from_r15"),
]
# .pdata's own padding holds only the RUNTIME_FUNCTIONs; their UNWIND_INFO
# goes here, in the block, because the padding is nearly full.
UNWIND_OFFSET = 0x2800


def module_branches(code, base):
    """The rel32 jmps and calls in `code`, assembled at `base` in the injector
    payload's frame (the block at zero), whose targets lie outside the block:
    references into logic.dll. The file patch assembles at the real rva, so
    there they are right as they stand; the injector re-aims each one from
    where the block landed. Returns (offset of the rel32 field, target rva)."""
    import capstone
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    out = []
    for ins in md.disasm(code, base):
        if ins.bytes[0] in (0xe8, 0xe9) and ins.size == 5:
            target = int(ins.op_str, 16)
            if not 0 <= target < BLOCK_SIZE:
                out.append((ins.address + 1, target))
    return out


def pose_site(site, stock, thunk):
    """What a pose site holds before patching, and the tail written after its
    new call. A thunk's jmp is followed by int3 padding; it becomes a call and
    a ret in the padding's first byte."""
    before = (b"\xe9" if thunk else b"\xe8") + struct.pack("<i", stock - (site + 5))
    return (before + b"\xcc", b"\xc3") if thunk else (before, b"")


_SELECT_IS_EDIT = [
    # Include the parent branch in the expected span so the injector refuses
    # the older unconditional jump instead of leaving that bypass in place.
    (0x4491d0,
     bytes.fromhex("7410488b01ff505884c0741a807b1800eb0a"),
     bytes.fromhex("7410488b01ff505884c0741a807b1800eb00")),
]


# A simple [base +/- disp] operand, and an indexed [base + index*scale +/- disp].
SIMPLE_OPERAND = re.compile(r"\[\s*([a-z][a-z0-9]*)\s*(?:([+-])\s*(0x[0-9a-fA-F]+))?\s*\]")
INDEXED_OPERAND = re.compile(
    r"\[\s*([a-z][a-z0-9]*)\s*\+\s*([a-z][a-z0-9]*)(?:\s*\*\s*\d+)?\s*([+-])\s*(0x[0-9a-fA-F]+)\s*\]")


def apply_layout(text, layout, applied=None):
    """Rewrite each memory operand's displacement through `layout`, a per-build
    table keyed (mnemonic, base register, reference displacement) -> the
    target's. The patch source is shared between builds; only the layout
    changes, so one class field or vtable slot moving is a table entry, not a
    copied patch. Stack frames (rsp/rbp) never move.

    tables key on the base register alone, so an indexed operand a table entry
    claims is refused rather than rewritten: the index may make it a different
    class's field. `applied`, when given, collects the keys that were used."""
    if not layout:
        return text
    mnemonic = text.split(None, 1)[0] if text.split() else ""
    key_of = lambda reg, value: f"{mnemonic}|{reg}|{value:#x}"

    def guard(match):
        base, _index, sign, disp = match.groups()
        value = int(disp, 16) * (-1 if sign == "-" else 1)
        key = key_of(base, value)
        if base not in ("rsp", "rbp") and key in layout:
            raise SystemExit(
                f"layout entry {key} matches an indexed operand {match.group(0)!r}; "
                "the table keys ignore the index register, so resolve it by hand")
        return match.group(0)

    def swap(match):
        reg, sign, disp = match.group(1), match.group(2), match.group(3)
        if reg in ("rsp", "rbp") or disp is None:
            return match.group(0)
        value = int(disp, 16) * (-1 if sign == "-" else 1)
        key = key_of(reg, value)
        target = layout.get(key)
        if target is None:
            return match.group(0)
        if applied is not None:
            applied.add(key)
        return f"[{reg} + {target:#x}]"

    text = INDEXED_OPERAND.sub(guard, text)
    return SIMPLE_OPERAND.sub(swap, text)


# The active build's layout tables. Empty means the build the patch was written
# for: every reference offset stands. `load_layout` replaces them for a build
# whose classes gained members (tools/offsetmap.py derives the entries).
LAYOUT = {}
GAME_LAYOUT = {}
# Named constants the patch source names but that differ between builds and are
# not class offsets: a displaced prologue's frame size, for instance. Same
# shared source; the profile supplies the value.
# `squad_roster` is the squad AI facet's roster getter. It is a logic.dll class
# slot, but the game.dll payload reaches the same object, so both tables carry
# it: the per-module offset maps cannot see a cross-DLL use. `ai_can_attack`
# is every AI facet's "can attack this target kind" (kind, enabled ammo only);
# `ai_attack_order` the AI test the attack command applies to each recipient,
# `ai_attack_ready` the one the order buttons apply after `ai_can_attack`.
# The game layout maps `call [rax+0x368]` to another class's slot, so the
# latter must be a symbol: symbols are substituted after the layout.
REFERENCE_SYMBOLS = {"lie_down_frame": 0x70, "stand_up_frame": 0x60,
                     "gunner_count": 0x130, "gunner_get": 0x120,
                     "ammo_pool_get": 0x1b8, "squad_roster": 0x3b8,
                     "ai_can_attack": 0x390}
SYMBOLS = dict(REFERENCE_SYMBOLS)
REFERENCE_GAME_SYMBOLS = {"gunner_count": 0x130, "gunner_get": 0x120, "squad_roster": 0x3b8,
                          "ai_can_attack": 0x390, "ai_attack_order": 0x368,
                          "ai_attack_ready": 0x370, "ammo_pool_get": 0x1b8}
GAME_SYMBOLS = dict(REFERENCE_GAME_SYMBOLS)
# Which layout keys an assembly actually rewrote, and the sources it was run
# over, keyed by the table's identity, so a per-build run can report entries
# that never applied.
APPLIED = {}
SOURCES = {}


def layout_gaps_for(layout):
    """`layout_gaps` over every source assembled with `layout` so far."""
    return layout_gaps(SOURCES.get(id(layout), []), layout)


def cross_dll_gaps(sources, own, other):
    """Operands whose shape moved in the *other* module's layout. A class
    defined in one module but reached from the other has its slot in that
    module's map only; the active layout does not rewrite it, so it silently
    keeps the reference offset. Such a slot must be a shared symbol rather than
    a per-module map entry. Returns (source name, line, key, new offset)."""
    gaps = []
    for name, lines in sources:
        for raw in lines:
            code = raw.split(";")[0].strip()
            if not code:
                continue
            mnemonic = code.split(None, 1)[0]
            for match in SIMPLE_OPERAND.finditer(code):
                reg, sign, disp = match.group(1), match.group(2), match.group(3)
                if reg in ("rsp", "rbp") or disp is None:
                    continue
                value = int(disp, 16) * (-1 if sign == "-" else 1)
                key = f"{mnemonic}|{reg}|{value:#x}"
                if key in other and key not in own:
                    gaps.append((name, code, key, other[key]))
    return gaps


def layout_gaps(sources, layout):
    """Layout entries that never applied, paired with any source operand that
    shares their reference displacement but a different mnemonic or base: a
    mis-keyed entry would otherwise silently leave that read at the reference
    offset. `sources` is [(name, lines), ...]; only a real near-miss is
    reported, since unused global entries are expected."""
    operands = []
    for _name, lines in sources:
        for raw in lines:
            code = raw.split(";")[0].strip()
            if not code:
                continue
            mnemonic = code.split(None, 1)[0]
            for pattern in (SIMPLE_OPERAND, INDEXED_OPERAND):
                for match in pattern.finditer(code):
                    groups = match.groups()
                    reg, sign, disp = (groups if pattern is SIMPLE_OPERAND
                                       else (groups[0], groups[2], groups[3]))
                    if disp is None:
                        continue
                    operands.append((mnemonic, reg, int(disp, 16) * (-1 if sign == "-" else 1), code))
    gaps = []
    for key in layout:
        mnemonic, reg, old = key.split("|")
        old = int(old, 16)
        near = next((code for m, r, value, code in operands
                     if value == old and (m != mnemonic or r != reg)), None)
        if near is not None and key not in APPLIED.get(id(layout), ()):
            gaps.append((key, near))
    return gaps


LAYOUT_DIR = pathlib.Path(__file__).resolve().parent / "layouts"


def active_profile():
    """The per-build layout the tooling should emit for: `DEFIANCE_LAYOUT`
    names a `tools/layouts/*.json` profile (or a path). None means the build
    the patch was written for. Returns (name, profile)."""
    spec = os.environ.get("DEFIANCE_LAYOUT")
    if not spec:
        return None, None
    path = pathlib.Path(spec)
    if not path.suffix:
        path = LAYOUT_DIR / f"{spec}.json"
    return path.stem, json.loads(path.read_text(encoding="utf-8"))


def load_layout(profile):
    """Adopt a build profile's layout tables (tools/layouts/*.json). The
    selection edits are re-assembled, because their replacement bytes embed
    class offsets too."""
    global LAYOUT, GAME_LAYOUT, SYMBOLS, GAME_SYMBOLS
    # Stack frames never move in the payload; drop their dead entries so the
    # layout audit is not all noise.
    class_offsets = lambda table: {k: v for k, v in table.items()
                                   if k.split("|")[1] not in ("rsp", "rbp")}
    LAYOUT = class_offsets(profile.get("logic_layout", {}))
    GAME_LAYOUT = class_offsets(profile.get("game_layout", {}))
    SYMBOLS = {**REFERENCE_SYMBOLS, **profile.get("logic_symbols", {})}
    GAME_SYMBOLS = {**REFERENCE_GAME_SYMBOLS, **profile.get("game_symbols", {})}
    APPLIED.clear()
    SOURCES.clear()
    refresh_selection_edits()
    return LAYOUT, GAME_LAYOUT


def assemble(lines, base, cursor_rva, scratch_rva=None, census_rva=None, layout=None, symbols=None):
    """Two-pass assembly with labels, iterated until the sizes settle.

    A line may name {cursor}, which becomes the rip-relative displacement of
    the cursor dword, {scratch}, a second cell in the block, or {census}, a
    diagnostic caller table. That displacement depends on the length of the
    very instruction holding it, so it is measured with a same-width
    placeholder first and the result is checked.

    Every memory operand is put through `layout` (the active build's table
    when none is given), so the shared source reads the build's own offsets.
    """
    ks = keystone.Ks(keystone.KS_ARCH_X86, keystone.KS_MODE_64)
    if layout is None:
        layout = LAYOUT
    if symbols is None:
        symbols = SYMBOLS
    applied = APPLIED.setdefault(id(layout), set())
    SOURCES.setdefault(id(layout), []).append((base, lines))
    body = []
    for raw in lines:
        line = raw.split(";")[0].strip()
        if not line:
            continue
        m = re.fullmatch(r"([A-Za-z_][A-Za-z0-9_]*):", line)
        body.append(("label", m.group(1)) if m else ("insn", line))

    def encode(text, addr, labels):
        resolved = apply_layout(text, layout, applied)
        for name, value in symbols.items():
            resolved = re.sub(rf"\{{{re.escape(name)}\}}", hex(value), resolved)
        for name, value in labels.items():
            resolved = re.sub(rf"\b{name}\b", hex(value), resolved)
        for token, target in (("{cursor}", cursor_rva), ("{scratch}", scratch_rva),
                              ("{census}", census_rva)):
            if target is None or token not in resolved:
                continue
            probe, _ = ks.asm(resolved.replace(token, "0x7ffffff0").encode(), addr)
            disp = target - (addr + len(probe))
            out, _ = ks.asm(resolved.replace(token, hex(disp)).encode(), addr)
            if len(out) != len(probe):
                raise SystemExit(f"{text!r} changed width once the displacement was known")
            return bytes(out)
        out, _ = ks.asm(resolved.encode(), addr)
        if out is None:
            raise SystemExit(f"cannot assemble {text!r} at {addr:#x}")
        return bytes(out)

    # every label is seeded so that a forward reference assembles on pass one;
    # the sizes then settle over the following passes
    labels = {name: base for kind, name in body if kind == "label"}
    sizes = {}
    for _ in range(8):
        addr, changed = base, False
        for i, (kind, text) in enumerate(body):
            if kind == "label":
                if labels.get(text) != addr:
                    labels[text], changed = addr, True
                continue
            try:
                n = len(encode(text, addr, labels))
            except keystone.KsError as err:
                raise SystemExit(f"cannot assemble {text!r} at {addr:#x}: {err}")
            if sizes.get(i) != n:
                sizes[i], changed = n, True
            addr += n
        if not changed:
            break
    out, addr = bytearray(), base
    for kind, text in body:
        if kind == "label":
            continue
        encoded = encode(text, addr, labels)
        out += encoded
        addr += len(encoded)
    return bytes(out), labels


# Select a squad's complete member vector, not just its first member. The
# replacement fits the original branch and uses its existing frame/unwind.
SQUAD_SELECT_RVA = 0x418cda
SQUAD_SELECT_BEFORE = bytes.fromhex(
    "488b03ba10000000488bcbff909800000084c00f84b4000000488b03488bcb"
    "ff90b0000000488b48284885c90f849b000000488b01ff90b80300004885c0"
    "0f8489000000488b10488bc8ff5268488b104c8b40084c2bc249c1f8034d"
    "85c0746d4e8d04c500000000488d4c2420e8dea32600488b4c2420488b01eb3a")


def selection_edits():
    """The four in-place selection edits, assembled with the active layout.
    select-squad's replacement carries the squad facet's vtable slot, so it is
    rebuilt whenever the layout changes."""
    edits = list(_SELECT_IS_EDIT)
    squad_code, _ = assemble(
        pathlib.Path("patch/select-squad.asm").read_text().splitlines(), SQUAD_SELECT_RVA, 0)
    if len(squad_code) > len(SQUAD_SELECT_BEFORE):
        raise RuntimeError("squad selection code exceeds the original branch")
    edits.append((SQUAD_SELECT_RVA, SQUAD_SELECT_BEFORE,
                  squad_code.ljust(len(SQUAD_SELECT_BEFORE), b"\x90")))

    # Stock toggle-add and same-type selection write the squad flag directly.
    # Keep their eligibility/type filters, but select accepted entities through
    # the manager so every member receives its mark. select (418cb0) does not
    # use RCX; the toggle bucket consumer has no manager pointer available.
    for rva, expected, instructions in [
        (0x419ca3, "488b0b488b01ff90b0000000488b48504885c97408488b01b201ff5050",
         ["mov rdx, qword ptr [rbx]", "xor ecx, ecx", "call 0x418cb0"]),
        (0x419309, "488b06b201488bceff5050",
         ["mov rdx, r14", "mov rcx, r13", "call 0x418cb0"]),
    ]:
        before = bytes.fromhex(expected)
        after, _ = assemble(instructions, rva, 0)
        if len(after) > len(before):
            raise RuntimeError(f"selection edit at {rva:#x} exceeds its span")
        edits.append((rva, before, after.ljust(len(before), b"\x90")))
    return edits


SELECTION_EDITS = selection_edits()


def refresh_selection_edits():
    global SELECTION_EDITS
    SELECTION_EDITS = selection_edits()
    return SELECTION_EDITS

REGISTER = {"rax": 0, "rcx": 1, "rdx": 2, "rbx": 3, "rbp": 5, "rsi": 6, "rdi": 7,
            "r8": 8, "r9": 9, "r10": 10, "r11": 11,
            "r12": 12, "r13": 13, "r14": 14, "r15": 15}


def unwind_info(pushes, frame_bytes):
    """UNWIND_INFO for a prologue that pushes `pushes` in order and then
    subtracts `frame_bytes`. Built from the prologue's own layout rather than
    written out by hand, so a second routine cannot silently inherit the
    first one's frame. Codes run in descending prolog offset."""
    codes, offset = [], 0
    for name in pushes:
        offset += 1 if REGISTER[name] < 8 else 2      # r8 and up need a REX byte
        codes.append((offset, (REGISTER[name] << 4) | 0))       # UWOP_PUSH_NONVOL
    offset += 4                                        # sub rsp, imm8
    codes.append((offset, ((frame_bytes // 8 - 1) << 4) | 2))   # UWOP_ALLOC_SMALL
    codes.reverse()
    blob = bytes([1, offset, len(codes), 0])
    for at, op in codes:
        blob += bytes([at, op])
    return blob


def main():
    src = pathlib.Path(SRC).read_bytes()
    sha = hashlib.sha256(src).hexdigest()
    if sha != EXPECT_SOURCE_SHA:
        raise SystemExit(
            f"{SRC} is sha256 {sha}, not the build this patch was written for\n"
            f"  expected {EXPECT_SOURCE_SHA}\n"
            "The addresses and structure offsets have to be re-derived before\n"
            "this can be rebuilt; tools/show.py and tools/rtti.py find them.")
    img = Image(SRC)
    data = bytearray(src)
    pe = img.pe

    # where the new section will live
    last = max(pe.sections, key=lambda s: s.VirtualAddress)
    align = pe.OPTIONAL_HEADER.SectionAlignment
    state_rva = (last.VirtualAddress + last.Misc_VirtualSize + align - 1) & ~(align - 1)

    code_rva = state_rva
    cursor_rva = state_rva + CURSOR_OFFSET
    code, labels = assemble(pathlib.Path("patch/pickup.asm").read_text().splitlines(),
                            code_rva, cursor_rva)
    if len(code) > CURSOR_OFFSET:
        raise SystemExit(f"{len(code)} bytes of code would reach the cursor at "
                         f"{CURSOR_OFFSET:#x}")

    # the move filter shares the block, after the chooser and before the cursor
    move_rva = state_rva + MOVE_OFFSET
    move, move_labels = assemble(
        pathlib.Path("patch/move-filter.asm").read_text().splitlines(), move_rva, cursor_rva)
    if len(code) > MOVE_OFFSET or MOVE_OFFSET + len(move) > CURSOR_OFFSET:
        raise SystemExit(f"{len(code)} and {len(move)} bytes do not both fit before "
                         f"the cursor at {CURSOR_OFFSET:#x}")

    # the soldier's setSelected, after the filter; the injector keeps its trace
    # stubs between the two, which this file patch does not carry
    setter_rva = state_rva + SETTER_OFFSET
    setter, setter_labels = assemble(
        pathlib.Path("patch/soldier-mark.asm").read_text().splitlines(), setter_rva, cursor_rva)
    if MOVE_OFFSET + len(move) > SETTER_OFFSET or SETTER_OFFSET + len(setter) > CURSOR_OFFSET:
        raise SystemExit(f"the setter's {len(setter)} bytes do not fit between the filter "
                         f"and the cursor")

    # the posture gates, between the chooser and the filter
    posture_rva = state_rva + POSTURE_OFFSET
    posture, posture_labels = assemble(
        pathlib.Path("patch/posture-gate.asm").read_text().splitlines(), posture_rva, cursor_rva)
    if len(code) > POSTURE_OFFSET or POSTURE_OFFSET + len(posture) > MOVE_OFFSET:
        raise SystemExit(f"the chooser's {len(code)} and the gates' {len(posture)} bytes "
                         f"do not fit before the filter")

    # firing mode, after the (injector's) census
    firing_rva = state_rva + FIRING_OFFSET
    firing, firing_labels = assemble(
        pathlib.Path("patch/firing-mode.asm").read_text().splitlines(), firing_rva, cursor_rva)
    if FIRING_OFFSET + len(firing) > CURSOR_OFFSET:
        raise SystemExit(f"firing mode's {len(firing)} bytes would reach the cursor")

    # ammo slot use, after firing mode; it records into the manager trace ring
    ammo_rva = state_rva + AMMO_OFFSET
    ammo, ammo_labels = assemble(
        pathlib.Path("patch/ammo-mode.asm").read_text().splitlines(), ammo_rva,
        state_rva + TRACE_OFFSET, state_rva + AMMO_SCRATCH)
    if FIRING_OFFSET + len(firing) > AMMO_OFFSET or AMMO_OFFSET + len(ammo) > AMMO_SCRATCH:
        raise SystemExit(f"ammo mode's {len(ammo)} bytes do not fit after firing mode")

    # the prone query, after the pose split
    prone_rva = state_rva + PRONE_OFFSET
    prone, prone_labels = assemble(
        pathlib.Path("patch/prone-query.asm").read_text().splitlines(), prone_rva, cursor_rva)

    # the pose split, after the setter and before the ring it records into
    pose_rva = state_rva + POSE_OFFSET
    pose, pose_labels = assemble(
        pathlib.Path("patch/pose.asm").read_text().splitlines(), pose_rva,
        state_rva + TRACE_OFFSET)
    if SETTER_OFFSET + len(setter) > POSE_OFFSET or POSE_OFFSET + len(pose) > PRONE_OFFSET:
        raise SystemExit(f"the pose split's {len(pose)} bytes do not fit between the "
                         f"setter and the prone query")
    if PRONE_OFFSET + len(prone) > TRACE_OFFSET:
        raise SystemExit(f"the prone query's {len(prone)} bytes would reach the ring")

    # 1. the four routines, in the block appended below
    block = bytearray(BLOCK_SIZE)
    block[:len(code)] = code
    block[MOVE_OFFSET:MOVE_OFFSET + len(move)] = move
    block[SETTER_OFFSET:SETTER_OFFSET + len(setter)] = setter
    block[POSE_OFFSET:POSE_OFFSET + len(pose)] = pose
    block[POSTURE_OFFSET:POSTURE_OFFSET + len(posture)] = posture
    block[PRONE_OFFSET:PRONE_OFFSET + len(prone)] = prone
    block[FIRING_OFFSET:FIRING_OFFSET + len(firing)] = firing
    block[AMMO_OFFSET:AMMO_OFFSET + len(ammo)] = ammo

    # 2. the call sites: the chooser is retargeted, the move filter is a detour
    #    over one displaced instruction that it performs on the way out
    call_off = img.rva_to_file(CALL_SITE)
    want = b"\xe8" + struct.pack("<i", STOCK_CHOOSER - (CALL_SITE + 5))
    if bytes(data[call_off:call_off + 5]) != want:
        raise SystemExit(f"{CALL_SITE:#x} is not the expected call to the stock chooser")
    data[call_off:call_off + 5] = b"\xe8" + struct.pack("<i", code_rva - (CALL_SITE + 5))

    move_off = img.rva_to_file(MOVE_CALL_SITE)
    if bytes(data[move_off:move_off + len(MOVE_DISPLACED)]) != MOVE_DISPLACED:
        raise SystemExit(f"{MOVE_CALL_SITE:#x} does not hold the expected instruction")
    data[move_off:move_off + 5] = b"\xe8" + struct.pack(
        "<i", move_rva - (MOVE_CALL_SITE + 5))

    # the stock setter becomes a jmp to its replacement, the rest of it nops
    set_off = img.rva_to_file(SETTER_RVA)
    if bytes(data[set_off:set_off + len(SETTER_STOCK)]) != SETTER_STOCK:
        raise SystemExit(f"{SETTER_RVA:#x} is not the stock soldier setSelected")
    data[set_off:set_off + len(SETTER_STOCK)] = (b"\xe9" + struct.pack(
        "<i", setter_rva - (SETTER_RVA + 5))).ljust(len(SETTER_STOCK), b"\x90")

    # the getter next to it, jmp over its prologue into the guarded copy
    is_off = img.rva_to_file(IS_SELECTED_RVA)
    if bytes(data[is_off:is_off + len(IS_SELECTED_STOCK)]) != IS_SELECTED_STOCK:
        raise SystemExit(f"{IS_SELECTED_RVA:#x} is not the stock isSelected prologue")
    data[is_off:is_off + len(IS_SELECTED_STOCK)] = (b"\xe9" + struct.pack(
        "<i", setter_labels["is_selected"] - (IS_SELECTED_RVA + 5))
        ).ljust(len(IS_SELECTED_STOCK), b"\x90")

    # the two posture functions start with a jmp to their gates
    for rva, displaced, label in POSTURE_HOOKS:
        off = img.rva_to_file(rva)
        if bytes(data[off:off + len(displaced)]) != displaced:
            raise SystemExit(f"{rva:#x} does not start with the expected prologue")
        data[off:off + len(displaced)] = (b"\xe9" + struct.pack(
            "<i", posture_labels[label] - (rva + 5))).ljust(len(displaced), b"\x90")

    # the movement states' reads call move_posture instead
    for site in MOVE_POSTURE_SITES:
        off = img.rva_to_file(site)
        if bytes(data[off:off + len(MOVE_POSTURE_READ)]) != MOVE_POSTURE_READ:
            raise SystemExit(f"{site:#x} is not the expected read of the squad's flag")
        data[off:off + len(MOVE_POSTURE_READ)] = (b"\xe8" + struct.pack(
            "<i", posture_labels["move_posture"] - (site + 5))).ljust(len(MOVE_POSTURE_READ), b"\x90")

    # and the squad's stand-up loop asks squad_stand_gate
    for site in SQUAD_STAND_SITES:
        off = img.rva_to_file(site)
        if bytes(data[off:off + len(SQUAD_STAND_READ)]) != SQUAD_STAND_READ:
            raise SystemExit(f"{site:#x} is not the expected compare in the squad's stand-up")
        data[off:off + len(SQUAD_STAND_READ)] = b"\xe8" + struct.pack(
            "<i", posture_labels["squad_stand_gate"] - (site + 5))

    # firing mode: jmps over the getters and setter, a call over the direct read
    jmp, call, nop = bytes.fromhex("e9"), bytes.fromhex("e8"), bytes.fromhex("90")
    for rva, displaced, label in FIRING_HOOKS:
        off = img.rva_to_file(rva)
        if bytes(data[off:off + len(displaced)]) != displaced:
            raise SystemExit(f"{rva:#x} is not the expected firing-mode code")
        data[off:off + len(displaced)] = (jmp + struct.pack(
            "<i", firing_labels[label] - (rva + 5))).ljust(len(displaced), nop)
    for rva, before, label in FIRING_CALLS:
        off = img.rva_to_file(rva)
        if bytes(data[off:off + len(before)]) != before:
            raise SystemExit(f"{rva:#x} is not the expected call to the squad's getter")
        data[off:off + len(before)] = (call + struct.pack(
            "<i", firing_labels[label] - (rva + 5))).ljust(len(before), nop)

    # ammo slot use: the setter and getter become jmps to their replacements
    for rva, displaced, label in AMMO_HOOKS + AMMO_READER_HOOKS:
        off = img.rva_to_file(rva)
        if bytes(data[off:off + len(displaced)]) != displaced:
            raise SystemExit(f"{rva:#x} is not the expected ammo-slot code")
        data[off:off + len(displaced)] = (jmp + struct.pack(
            "<i", ammo_labels[label] - (rva + 5))).ljust(len(displaced), nop)

    for site, before_hex, label in AMMO_GATE_CALLS:
        before = bytes.fromhex(before_hex)
        off = img.rva_to_file(site)
        if bytes(data[off:off + len(before)]) != before:
            raise SystemExit(f"{site:#x} is not the expected ammo gate")
        data[off:off + len(before)] = b"\xe8" + struct.pack(
            "<i", ammo_labels[label] - (site + 5)) + b"\x90" * (len(before) - 5)

    # the prone query starts with a jmp to its replacement
    rva, displaced, label = PRONE_HOOK
    off = img.rva_to_file(rva)
    if bytes(data[off:off + len(displaced)]) != displaced:
        raise SystemExit(f"{rva:#x} does not start with the expected prologue")
    data[off:off + len(displaced)] = (b"\xe9" + struct.pack(
        "<i", prone_labels[label] - (rva + 5))).ljust(len(displaced), b"\x90")

    # the handler's calls and the AiUtils thunks go to the pose split's entries
    for site, stock, label, thunk in POSE_CALLS:
        off = img.rva_to_file(site)
        before, tail = pose_site(site, stock, thunk)
        if bytes(data[off:off + len(before)]) != before:
            raise SystemExit(f"{site:#x} does not reach {stock:#x} as expected")
        data[off:off + len(before)] = b"\xe8" + struct.pack(
            "<i", pose_labels[label] - (site + 5)) + tail

    # 3. unwind data in .pdata's padding, one record per routine
    d = pe.OPTIONAL_HEADER.DATA_DIRECTORY[PDATA_DIR]
    pdata = next(s for s in pe.sections if s.Name.rstrip(b"\x00") == b".pdata")
    routines = [(code_rva, len(code), unwind_info(CHOOSER_PUSHES, CHOOSER_FRAME)),
                (move_rva, len(move), unwind_info(MOVE_PUSHES, MOVE_FRAME)),
                (setter_rva, setter_labels["is_selected"] - setter_rva,
                 unwind_info(SETTER_PUSHES, SETTER_FRAME)),
                (setter_labels["is_selected"], setter_labels["is_end"] - setter_labels["is_selected"],
                 unwind_info(["rbx"], 0x20)),
                # the body only: the entries and the recorder after it use no stack
                (pose_rva, pose_labels["pose_down"] - pose_rva,
                 unwind_info(POSE_PUSHES, POSE_FRAME)),
                # each gate up to its displaced prologue, and the pin lookup
                (posture_labels["stand_up_gate"],
                 posture_labels["stand_up_resume"] - posture_labels["stand_up_gate"],
                 unwind_info(["rbx"], 0x20)),
                (posture_labels["lie_down_gate"],
                 posture_labels["lie_down_resume"] - posture_labels["lie_down_gate"],
                 unwind_info(["rbx"], 0x20)),
                (posture_labels["pin_of"], posture_labels["pin_end"] - posture_labels["pin_of"],
                 unwind_info([], 0x28)),
                (posture_labels["move_posture"],
                 posture_labels["move_posture_end"] - posture_labels["move_posture"],
                 unwind_info(MOVE_POSTURE_PUSHES, 0x20)),
                (posture_labels["squad_stand_gate"],
                 posture_labels["squad_stand_end"] - posture_labels["squad_stand_gate"],
                 unwind_info(MOVE_POSTURE_PUSHES, 0x20)),
                # the query's body, and its callable copy of the stock prologue
                (prone_rva, prone_labels["stock_prone"] - prone_rva,
                 unwind_info(POSE_PUSHES, POSE_FRAME)),
                (prone_labels["stock_prone"], prone_rva + len(prone) - prone_labels["stock_prone"],
                 unwind_info(["rbx"], 0x20)),
                # firing mode: the setter and the panel's getter, their two
                # helpers, the soldier's getter up to its stock resume, the
                # direct read's replacement and the pin lookup
                (firing_labels["firing_set"], firing_labels["firing_ui"] - firing_labels["firing_set"],
                 unwind_info(FIRING_PUSHES, 0x28)),
                (firing_labels["firing_ui"], firing_labels["list_members"] - firing_labels["firing_ui"],
                 unwind_info(FIRING_PUSHES, 0x28)),
                (firing_labels["list_members"], firing_labels["list_end"] - firing_labels["list_members"],
                 unwind_info([], 0x28)),
                (firing_labels["next_soldier"], firing_labels["next_end"] - firing_labels["next_soldier"],
                 unwind_info([], 0x28)),
                (firing_labels["firing_soldier"],
                 firing_labels["soldier_stock"] - firing_labels["firing_soldier"],
                 unwind_info(["rbx"], 0x20)),
                (firing_labels["firing_call"], firing_labels["call_end"] - firing_labels["firing_call"],
                 unwind_info(["rbx"], 0x20)),
                (firing_labels["fire_pin"], firing_labels["fire_end"] - firing_labels["fire_pin"],
                 unwind_info([], 0x28)),
                # ammo mode: both entries and their four helpers
                (ammo_labels["ammo_set"], ammo_labels["ammo_get"] - ammo_labels["ammo_set"],
                 unwind_info(AMMO_PUSHES, AMMO_FRAME)),
                (ammo_labels["ammo_get"], ammo_labels["list_members"] - ammo_labels["ammo_get"],
                 unwind_info(AMMO_PUSHES, AMMO_GET_FRAME)),
                (ammo_labels["list_members"], ammo_labels["list_end"] - ammo_labels["list_members"],
                 unwind_info([], 0x28)),
                (ammo_labels["next_soldier"], ammo_labels["next_end"] - ammo_labels["next_soldier"],
                 unwind_info([], 0x28)),
                (ammo_labels["write_one"], ammo_labels["write_end"] - ammo_labels["write_one"],
                 unwind_info(["rbx", "rbp"], 0x28)),
                (ammo_labels["get_one"], ammo_labels["get_end"] - ammo_labels["get_one"],
                 unwind_info(["rbx"], 0x20))]
    for label, end, pushes, frame in [
        ("gate_from_rbx", "gate_from_rbx_end", ["rdi"], 0x20),
        ("gate_from_r13", "gate_from_r13_end", ["rdi"], 0x20),
        ("gate_from_r15", "gate_from_r15_end", ["rdi"], 0x20),
        ("ammo_unload_disabled", "ammo_unload_disabled_end", ["rbx", "rbp", "rsi", "rdi", "r12", "r13", "r14"], 0x20),
        ("ammo_gate_body", "gate_end", ["rbx", "rsi", "rdi"], 0x30),
        ("gun_pin", "gun_pin_end", ["rbx", "rsi", "rdi"], 0x20),
        ("gun_ready", "gun_ready_end", ["rbx"], 0x20),
        ("gun_can_fire", "gun_can_fire_end", ["rbx"], 0x20),
        ("gun_empty", "gun_empty_end", ["rbx"], 0x20),
        ("gun_loaded", "gun_loaded_end", ["rbx"], 0x20),
    ]:
        routines.append((ammo_labels[label], ammo_labels[end] - ammo_labels[label],
                         unwind_info(pushes, frame)))
    routines.sort(key=lambda row: row[0])
    entry_rva = d.VirtualAddress + d.Size
    entry_end = entry_rva + 12 * len(routines)
    if entry_end > pdata.VirtualAddress + pdata.SizeOfRawData:
        raise SystemExit(".pdata has no room for the unwind entries")
    # The RUNTIME_FUNCTIONs must stay contiguous in the exception directory,
    # whose padding is nearly full; the UNWIND_INFO they point at goes in the
    # appended block, which has room and is mapped read/execute.
    ui_rva = state_rva + UNWIND_OFFSET
    uis = b"".join(ui for _, _, ui in routines)
    if UNWIND_OFFSET + len(uis) > CURSOR_OFFSET:
        raise SystemExit("the unwind info does not fit before the cursor")
    block[UNWIND_OFFSET:UNWIND_OFFSET + len(uis)] = uis
    at_entry, at_ui = entry_rva, ui_rva
    for rva, length, ui in routines:
        struct.pack_into("<III", data, img.rva_to_file(at_entry), rva, rva + length, at_ui)
        at_entry += 12
        at_ui += len(ui)
    struct.pack_into("<I", data, d.get_file_offset() + 4, d.Size + 12 * len(routines))
    struct.pack_into("<I", data, pdata.get_file_offset() + 8,
                     max(pdata.Misc_VirtualSize, entry_end - pdata.VirtualAddress))

    # 4. the state section
    table = (pe.DOS_HEADER.e_lfanew + 4 + pe.FILE_HEADER.sizeof()
             + pe.FILE_HEADER.SizeOfOptionalHeader)
    count = pe.FILE_HEADER.NumberOfSections
    entry = table + count * 40
    first_raw = min(s.PointerToRawData for s in pe.sections if s.PointerToRawData)
    if entry + 40 > first_raw:
        raise SystemExit("no room in the header for another section")
    file_align = pe.OPTIONAL_HEADER.FileAlignment
    raw_at = (len(data) + file_align - 1) & ~(file_align - 1)
    data.extend(bytes(raw_at - len(data)))
    data.extend(block)
    data[entry:entry + 40] = STATE_NAME + struct.pack(
        "<IIIIIIHHI", BLOCK_SIZE, state_rva, BLOCK_SIZE, raw_at, 0, 0, 0, 0, STATE_CHARS)
    struct.pack_into("<H", data,
                     pe.FILE_HEADER.get_field_absolute_offset("NumberOfSections"), count + 1)
    struct.pack_into("<I", data,
                     pe.OPTIONAL_HEADER.get_field_absolute_offset("SizeOfImage"),
                     state_rva + BLOCK_SIZE)
    struct.pack_into("<I", data,
                     pe.OPTIONAL_HEADER.get_field_absolute_offset("SizeOfCode"),
                     pe.OPTIONAL_HEADER.SizeOfCode + BLOCK_SIZE)

    # 5. selection setter and getter edits; existing unwind behavior is kept.
    for rva, want, patched_bytes in SELECTION_EDITS:
        off = img.rva_to_file(rva)
        if bytes(data[off:off + len(want)]) != want:
            raise SystemExit(f"{rva:#x} is not the expected selection branch")
        data[off:off + len(want)] = patched_bytes

    out = pathlib.Path(DST)
    out.parent.mkdir(exist_ok=True)
    out.write_bytes(bytes(data))

    # the header checksum, on the finished file
    patched = pefile.PE(str(out))
    blob = bytearray(out.read_bytes())
    struct.pack_into("<I", blob,
                     patched.OPTIONAL_HEADER.get_field_absolute_offset("CheckSum"),
                     patched.generate_checksum())
    out.write_bytes(bytes(blob))

    out_sha = hashlib.sha256(out.read_bytes()).hexdigest()
    pathlib.Path(MANIFEST).write_text(json.dumps({
        "patch": "pickup.asm",
        "source_sha256": sha,
        "output_sha256": out_sha,
        "code_rva": code_rva,
        "code_bytes": len(code),
        "call_site": CALL_SITE,
        "cursor_rva": cursor_rva,
        "move_rva": move_rva,
        "move_bytes": len(move),
        "move_call_site": MOVE_CALL_SITE,
        "move_displaced": MOVE_DISPLACED.hex(),
        "setter_rva": setter_rva,
        "setter_bytes": len(setter),
        "setter_site": SETTER_RVA,
        "pose_rva": pose_rva,
        "pose_bytes": len(pose),
        "pose_sites": [site for site, _, _, _ in POSE_CALLS],
    }, indent=2) + "\n", encoding="utf-8")

    print(f"source   {SRC}  sha256 {sha}")
    print(f"chooser  {len(code)} bytes at rva {code_rva:#x} "
          f"({CURSOR_OFFSET - len(code)} bytes before the cursor)")
    print(f"call     {CALL_SITE:#x}  {STOCK_CHOOSER:#x} -> {code_rva:#x}")
    print(f"filter   {len(move)} bytes at rva {move_rva:#x}; detour over "
          f"{MOVE_CALL_SITE:#x} ({MOVE_DISPLACED.hex()})")
    print(f"setter   {len(setter)} bytes at rva {setter_rva:#x}; jmp over {SETTER_RVA:#x}")
    print(f"pose     {len(pose)} bytes at rva {pose_rva:#x}; calls at "
          + " ".join(f"{site:#x}" for site, _, _, _ in POSE_CALLS))
    print(f"unwind   {len(routines)} RUNTIME_FUNCTIONs from {entry_rva:#x}, UNWIND_INFO "
          f"from {ui_rva:#x}, directory {d.Size:#x} -> {d.Size + 12 * len(routines):#x}")
    print(f"state    section .patch at rva {state_rva:#x}, {BLOCK_SIZE:#x} bytes of "
          f"code and data at file {raw_at:#x}; cursor at {cursor_rva:#x}")
    print(f"select   individual selection at "
          + " ".join(f"{rva:#x}" for rva, _, _ in SELECTION_EDITS))
    print("labels   " + " ".join(f"{k}={v:#x}" for k, v in labels.items()))
    print("filter   " + " ".join(f"{k}={v:#x}" for k, v in move_labels.items()))
    print(f"output   {DST}  sha256 {out_sha}")
    print(f"manifest {MANIFEST}")


if __name__ == "__main__":
    main()
