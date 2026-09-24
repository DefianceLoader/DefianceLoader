"""Assemble the chooser for the injector rather than for the file patch.

The only difference from tools/build.py is where the block lives. The file
patch appends a section for it, at an address known while assembling; the
injector allocates a page in the running process, whose address is not known
until then. So this assembles the block at a notional base of zero: every
jump inside it is relative, and the cursor is reached rip-relative at a fixed
offset within the same block, which makes the whole payload position
independent. The injector writes it wherever the allocation lands.
"""
import hashlib, json, os, pathlib, struct, sys
sys.path.insert(0, "tools")
import build as b

BASE = 0                              # the payload is position independent
PAYLOAD = pathlib.Path("out/payload.bin")
DESCRIPTOR = pathlib.Path("out/payload.json")

# Loader/injector-only extension, beyond file-patch unwind storage and before
# the rotation cursor. The legacy file patch deliberately does not install it.
ORDER_OFFSET = 0x2c00
REGION_OFFSET = 0x2500
BUILDING_SELECT = (0x1d7ff0, bytes.fromhex("885130c3cc"), "building_select")
REGION_ICON_GATE = (0x418128, bytes.fromhex("498b06498bce"), "region_icon_gate")
REGION_CALLS = [(site, b"\xe8" + struct.pack("<i", 0x418000 - site - 5),
                 f"region_individual_{i}", 2)
                for i, site in enumerate((0x418e1f, 0x419097, 0x419130, 0x4193f0))]
ORDER_CALLS = [
    (0x43bab9, bytes.fromhex("488b442450"), "attack_members", 8),
    (0x43c6cd, bytes.fromhex("498b7d004885ff"), "garrison_members", 9),
    (0x107906, bytes.fromhex("488b78504885ff"), "building_exit_point", 9),
    (0x107e30, bytes.fromhex("488b58504885db"), "building_exit_target", 9),
    (0x10f806, bytes.fromhex("488b58504885db"), "building_facing_direct", 9),
    (0x319cd4, bytes.fromhex("488b58504885db"), "building_facing_command", 9),
    (0x10aa5f, bytes.fromhex("e8fca5f5ff"), "building_capacity_command", 9),
    (0x10ac2b, bytes.fromhex("e830a4f5ff"), "building_capacity_transfer", 9),
    (0xa6b9d, bytes.fromhex("e8bee4fbff"), "building_capacity_approach", 9),
    (0xaac73, bytes.fromhex("e8e8a3fbff"), "building_capacity_enter", 9),
    (0x65736, bytes.fromhex("4c3bff7544"), "building_reserve_occupant", 9),
    (0x65355, bytes.fromhex("482bc8492bce"), "building_capacity_subtract", 9),
]


def main():
    source = pathlib.Path(b.SRC).read_bytes()
    sha = hashlib.sha256(source).hexdigest()
    if sha != b.EXPECT_SOURCE_SHA:
        raise SystemExit(f"{b.SRC} is not the build this patch was written for ({sha})")
    # A per-build layout emits beside the reference payload, same source code.
    if "--layout" in sys.argv:
        os.environ["DEFIANCE_LAYOUT"] = sys.argv[sys.argv.index("--layout") + 1]
    name, profile = b.active_profile()
    if profile:
        b.load_layout(profile)
    payload_path = PAYLOAD if not name else PAYLOAD.with_name(f"payload-{name}.bin")
    descriptor_path = DESCRIPTOR if not name else DESCRIPTOR.with_name(f"payload-{name}.json")

    region, region_labels = b.assemble(
        pathlib.Path("patch/region-individual.asm").read_text().splitlines(),
        REGION_OFFSET, b.CURSOR_OFFSET)
    code, labels = b.assemble(
        pathlib.Path("patch/pickup.asm").read_text().splitlines(), BASE, b.CURSOR_OFFSET)
    move, _ = b.assemble(
        pathlib.Path("patch/move-filter.asm").read_text().splitlines(),
        BASE + b.MOVE_OFFSET, b.CURSOR_OFFSET)
    trace, trace_labels = b.assemble(
        pathlib.Path("patch/select-trace.asm").read_text().splitlines(),
        BASE + b.TRACE_CODE_OFFSET, b.TRACE_OFFSET)
    setter, setter_labels = b.assemble(
        pathlib.Path("patch/soldier-mark.asm").read_text().splitlines(),
        BASE + b.SETTER_OFFSET, b.CURSOR_OFFSET)
    pose, pose_labels = b.assemble(
        pathlib.Path("patch/pose.asm").read_text().splitlines(),
        BASE + b.POSE_OFFSET, b.TRACE_OFFSET)
    posture, posture_labels = b.assemble(
        pathlib.Path("patch/posture-gate.asm").read_text().splitlines(),
        BASE + b.POSTURE_OFFSET, b.CURSOR_OFFSET)
    prone, prone_labels = b.assemble(
        pathlib.Path("patch/prone-query.asm").read_text().splitlines(),
        BASE + b.PRONE_OFFSET, b.CURSOR_OFFSET)
    census, census_labels = b.assemble(
        pathlib.Path("patch/behaviour-census.asm").read_text().splitlines(),
        BASE + b.CENSUS_OFFSET, b.CENSUS_TABLE)
    firing, firing_labels = b.assemble(
        pathlib.Path("patch/firing-mode.asm").read_text().splitlines(),
        BASE + b.FIRING_OFFSET, b.CURSOR_OFFSET)
    ammo, ammo_labels = b.assemble(
        pathlib.Path("patch/ammo-mode.asm").read_text().splitlines(),
        BASE + b.AMMO_OFFSET, b.TRACE_OFFSET, b.AMMO_SCRATCH)
    orders, order_labels = b.assemble(
        pathlib.Path("patch/order-members.asm").read_text().splitlines(),
        BASE + ORDER_OFFSET, b.CURSOR_OFFSET)
    if ORDER_OFFSET + len(orders) > b.CURSOR_OFFSET:
        raise SystemExit("order filters would reach the rotation cursor")
    if len(code) > b.POSTURE_OFFSET or b.POSTURE_OFFSET + len(posture) > b.MOVE_OFFSET:
        raise SystemExit("the chooser and the posture gates do not fit before the filter")
    if b.MOVE_OFFSET + len(move) > b.TRACE_CODE_OFFSET:
        raise SystemExit("the filter does not fit before the trace")
    if b.TRACE_CODE_OFFSET + len(trace) > b.SETTER_OFFSET:
        raise SystemExit("the trace stubs would reach the setter")
    if b.SETTER_OFFSET + len(setter) > b.POSE_OFFSET:
        raise SystemExit("the setter would reach the pose split")
    if b.POSE_OFFSET + len(pose) > b.PRONE_OFFSET:
        raise SystemExit("the pose split would reach the prone query")
    if b.PRONE_OFFSET + len(prone) > b.TRACE_OFFSET:
        raise SystemExit("the prone query would reach the trace ring")
    if b.TRACE_OFFSET + 0x10 + 32 * 16 > b.CENSUS_TABLE:
        raise SystemExit("the trace ring would reach the census")
    if b.CENSUS_TABLE + 32 * 16 > b.CENSUS_OFFSET or b.CENSUS_OFFSET + len(census) > b.FIRING_OFFSET:
        raise SystemExit("the census does not fit before firing mode")
    if b.FIRING_OFFSET + len(firing) > b.AMMO_OFFSET:
        raise SystemExit("firing mode would reach ammo mode")
    if b.AMMO_OFFSET + len(ammo) > b.CURSOR_OFFSET:
        raise SystemExit("ammo mode would reach the rotation cursor")
    if b.AMMO_OFFSET + len(ammo) > b.AMMO_SCRATCH:
        raise SystemExit("the ammo code would reach its scratch cell")
    if b.AMMO_SCRATCH + 0x70 > b.UNWIND_OFFSET:
        raise SystemExit("the ammo scratch would reach the unwind info")

    # One blob laid out as the file patch lays out its section, plus the trace
    # stubs the file patch does not carry, so the injector writes it in a
    # single go and only has to work out the rel32s.
    payload = bytearray(b.FIRING_OFFSET + len(firing))
    payload[:len(code)] = code
    payload[b.MOVE_OFFSET:b.MOVE_OFFSET + len(move)] = move
    payload[b.TRACE_CODE_OFFSET:b.TRACE_CODE_OFFSET + len(trace)] = trace
    payload[b.SETTER_OFFSET:b.SETTER_OFFSET + len(setter)] = setter
    payload[b.POSE_OFFSET:b.POSE_OFFSET + len(pose)] = pose
    payload[b.POSTURE_OFFSET:b.POSTURE_OFFSET + len(posture)] = posture
    payload[b.PRONE_OFFSET:b.PRONE_OFFSET + len(prone)] = prone
    payload[b.CENSUS_OFFSET:b.CENSUS_OFFSET + len(census)] = census
    payload[b.FIRING_OFFSET:b.FIRING_OFFSET + len(firing)] = firing
    payload.extend(bytes(b.AMMO_OFFSET + len(ammo) - len(payload)))
    payload[b.AMMO_OFFSET:b.AMMO_OFFSET + len(ammo)] = ammo
    payload.extend(bytes(ORDER_OFFSET + len(orders) - len(payload)))
    payload[ORDER_OFFSET:ORDER_OFFSET + len(orders)] = orders
    if b.AMMO_OFFSET + len(ammo) > REGION_OFFSET or REGION_OFFSET + len(region) > b.AMMO_SCRATCH:
        raise SystemExit("region code overlaps ammunition storage")
    payload[REGION_OFFSET:REGION_OFFSET + len(region)] = region
    # Branches out of the block, into logic.dll: assembled here against a block
    # at zero, so the injector re-aims each rel32 from where the block landed.
    # Each carries the feature that owns the region it leaves, or 0 when the
    # region is shared (the pose split is reached from several features), so a
    # selective relocation can leave a disabled feature's branches unresolved.
    rel_fixups = []
    for routine, at, feature in ((code, 0, 1), (posture, b.POSTURE_OFFSET, 4),
                                 (move, b.MOVE_OFFSET, 3), (trace, b.TRACE_CODE_OFFSET, 7),
                                 (setter, b.SETTER_OFFSET, 2), (pose, b.POSE_OFFSET, 0),
                                 (prone, b.PRONE_OFFSET, 4), (census, b.CENSUS_OFFSET, 7),
                                 (firing, b.FIRING_OFFSET, 5), (ammo, b.AMMO_OFFSET, 6),
                                 (region, REGION_OFFSET, 2), (orders, ORDER_OFFSET, 9)):
        rel_fixups += [{"rel_offset": offset, "rel_target": target, "rel_feature": feature}
                       for offset, target in b.module_branches(routine, BASE + at)]


    # The five bytes the call site must hold before patching. What it holds
    # afterwards depends on where the block is allocated, so the injector
    # computes that itself.
    before = b"\xe8" + struct.pack("<i", b.STOCK_CHOOSER - (b.CALL_SITE + 5))
    img = b.Image(b.SRC)
    if img.read(b.CALL_SITE, 5) != before:
        raise SystemExit(f"{b.CALL_SITE:#x} does not hold the expected call")
    if img.read(b.MOVE_CALL_SITE, len(b.MOVE_DISPLACED)) != b.MOVE_DISPLACED:
        raise SystemExit(f"{b.MOVE_CALL_SITE:#x} does not hold the expected instruction")
    # Each trace hook jumps over whole instructions and its stub jumps back past
    # them, which from an allocated block has to go through a resolved imm64.
    trace_hooks, trace_fixups = [], []
    for rva, displaced, label, placeholder in b.TRACE_HOOKS:
        want = bytes.fromhex(displaced)
        if img.read(rva, len(want)) != want:
            raise SystemExit(f"{rva:#x} does not hold the displaced bytes {displaced}")
        if label not in trace_labels:
            raise SystemExit(f"the trace has no {label} stub")
        at = payload.find(struct.pack("<Q", placeholder))
        if at < 0 or payload.find(struct.pack("<Q", placeholder), at + 1) >= 0:
            raise SystemExit(f"the {label} resume placeholder is missing or repeated")
        trace_hooks.append({"hook_rva": rva, "hook_displaced": displaced,
                            "hook_entry": trace_labels[label]})
        trace_fixups.append({"trace_fix_offset": at, "trace_fix_rva": rva + len(want)})
    # The soldier's setSelected is replaced outright: a jmp over the whole
    # stock function, with no way back into it, so no fixup.
    if img.read(b.SETTER_RVA, len(b.SETTER_STOCK)) != b.SETTER_STOCK:
        raise SystemExit(f"{b.SETTER_RVA:#x} is not the stock soldier setSelected")
    detours = trace_hooks + [{"hook_rva": b.SETTER_RVA, "hook_displaced": b.SETTER_STOCK.hex(),
                              "hook_entry": b.SETTER_OFFSET},
                             {"hook_rva": b.IS_SELECTED_RVA,
                              "hook_displaced": b.IS_SELECTED_STOCK.hex(),
                              "hook_entry": setter_labels["is_selected"]}]
    detours.append({"hook_rva": BUILDING_SELECT[0],
                    "hook_displaced": BUILDING_SELECT[1].hex(),
                    "hook_entry": region_labels["building_select"]})
    if img.read(REGION_ICON_GATE[0], len(REGION_ICON_GATE[1])) != REGION_ICON_GATE[1]:
        raise SystemExit("region icon gate does not match the native predicate")
    detours.append({"hook_rva": REGION_ICON_GATE[0],
                    "hook_displaced": REGION_ICON_GATE[1].hex(),
                    "hook_entry": region_labels[REGION_ICON_GATE[2]]})
    # the posture gates, entered by jmps over their functions' prologues and
    # jumping back past them through the re-aimed rel32s
    for rva, displaced, label in b.POSTURE_HOOKS:
        if img.read(rva, len(displaced)) != displaced:
            raise SystemExit(f"{rva:#x} does not start with the expected prologue")
        detours.append({"hook_rva": rva, "hook_displaced": displaced.hex(),
                        "hook_entry": posture_labels[label]})
    # the prone query, likewise, whose stock answer jumps back into fn_110bb0
    rva, displaced, label = b.PRONE_HOOK
    if img.read(rva, len(displaced)) != displaced:
        raise SystemExit(f"{rva:#x} does not start with the expected prologue")
    detours.append({"hook_rva": rva, "hook_displaced": displaced.hex(),
                    "hook_entry": prone_labels[label]})
    # firing mode's jmps, and the behaviour census (the injector's only)
    for rva, displaced, label in b.FIRING_HOOKS:
        if img.read(rva, len(displaced)) != displaced:
            raise SystemExit(f"{rva:#x} is not the expected firing-mode code")
        detours.append({"hook_rva": rva, "hook_displaced": displaced.hex(),
                        "hook_entry": firing_labels[label]})
    # ammo slot use: the setter and getter, replaced outright
    for rva, displaced, label in b.AMMO_HOOKS:
        if img.read(rva, len(displaced)) != displaced:
            raise SystemExit(f"{rva:#x} is not the expected ammo-slot code")
        detours.append({"hook_rva": rva, "hook_displaced": displaced.hex(),
                        "hook_entry": ammo_labels[label]})
    # and the simulation reader's three sites, which jump back into fn_1157dd
    for rva, displaced, label in b.AMMO_READER_HOOKS:
        if img.read(rva, len(displaced)) != displaced:
            raise SystemExit(f"{rva:#x} is not the expected ammo-reader code")
        detours.append({"hook_rva": rva, "hook_displaced": displaced.hex(),
                        "hook_entry": ammo_labels[label]})
    rva, displaced, label = b.CENSUS_HOOK
    if img.read(rva, len(displaced)) != displaced:
        raise SystemExit(f"{rva:#x} is not the expected behaviour getter")
    detours.append({"hook_rva": rva, "hook_displaced": displaced.hex(),
                    "hook_entry": census_labels[label]})
    # The pose sites are retargeted like the chooser's call: what must be
    # there, the entry the new call goes to, and the tail after it (a thunk's
    # ret, written over the first byte of its padding).
    # pose_stock is the function a site's rel32 reaches, for the injector to
    # follow in a build where it has moved; zero where there is none.
    pose_calls = []
    for site, displaced, label, feature in ORDER_CALLS + REGION_CALLS:
        if img.read(site, len(displaced)) != displaced:
            raise SystemExit(f"{site:#x} is not the expected order-distribution code")
        pose_calls.append({"pose_site": site, "pose_before": displaced.hex(),
                           "pose_entry": region_labels["region_individual"] if feature == 2 else order_labels[label],
                           "pose_tail": "90" * (len(displaced) - 5), "pose_stock": 0})
    for site, stock, label, thunk in b.POSE_CALLS:
        pose_before, pose_tail = b.pose_site(site, stock, thunk)
        if img.read(site, len(pose_before)) != pose_before:
            raise SystemExit(f"{site:#x} does not reach {stock:#x} as expected")
        pose_calls.append({"pose_site": site, "pose_before": pose_before.hex(),
                           "pose_entry": pose_labels[label], "pose_tail": pose_tail.hex(),
                           "pose_stock": stock})
    # the movement states' reads of the squad's flag become calls too, with
    # the rest of the displaced read left as nops
    for site in b.MOVE_POSTURE_SITES:
        if img.read(site, len(b.MOVE_POSTURE_READ)) != b.MOVE_POSTURE_READ:
            raise SystemExit(f"{site:#x} is not the expected read of the squad's flag")
        pose_calls.append({"pose_site": site, "pose_before": b.MOVE_POSTURE_READ.hex(),
                           "pose_entry": posture_labels["move_posture"],
                           "pose_tail": "90" * (len(b.MOVE_POSTURE_READ) - 5),
                           "pose_stock": 0})
    # and fn_2caeb0's direct read of the squad's firing mode
    for rva, stock_call, label in b.FIRING_CALLS:
        if img.read(rva, len(stock_call)) != stock_call:
            raise SystemExit(f"{rva:#x} is not the expected call to the squad's getter")
        pose_calls.append({"pose_site": rva, "pose_before": stock_call.hex(),
                           "pose_entry": firing_labels[label],
                           "pose_tail": "90" * (len(stock_call) - 5), "pose_stock": 0})
    # and the ammo gate's call sites, retargeted so the shared answer is
    # filtered through the firing soldier's pin
    for gate_rva, gate_call, gate_label in b.AMMO_GATE_CALLS:
        gate_bytes = bytes.fromhex(gate_call)
        if img.read(gate_rva, len(gate_bytes)) != gate_bytes:
            raise SystemExit(f"{gate_rva:#x} is not the expected call to the ammo gate")
        pose_calls.append({"pose_site": gate_rva, "pose_before": gate_call,
                           "pose_entry": ammo_labels[gate_label],
                           "pose_tail": "90" * (len(gate_bytes) - 5), "pose_stock": 0})
    # The pose split finds the stock function from its caller's return point,
    # lea rax, [return point + stock - return point], a distance fixed when it
    # was assembled. Each such disp32 is listed so that the injector can work
    # it out again in a build where the two have moved apart.
    import capstone
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = True
    pose_code = list(md.disasm(pose, BASE + b.POSE_OFFSET))
    delta_fixups = []
    for site, stock, label, thunk in b.POSE_CALLS:
        back = site + 5
        leas = [ins for ins in pose_code if ins.mnemonic == "lea" and any(
            op.type == capstone.x86.X86_OP_MEM and op.mem.disp == stock - back
            for op in ins.operands)]
        if len(leas) != 1:
            raise SystemExit(f"{label}: {len(leas)} leas of {stock - back:#x} in the pose split")
        offset = leas[0].address + leas[0].disp_offset
        if struct.unpack_from("<i", payload, offset)[0] != stock - back:
            raise SystemExit(f"{label}: the disp32 at +{offset:#x} is not {stock - back:#x}")
        delta_fixups.append({"delta_offset": offset, "delta_from": back, "delta_to": stock})
    # The injector's self test plants whatever the descriptor says, so it cannot
    # notice a wrong expectation; check the chooser call's here, against the DLL.
    if img.read(b.CALL_SITE, 5) != before or before[0] != 0xe8:
        raise SystemExit("call_before is not the stock chooser call")

    # Signatures for every site the injector touches (tools/sigs.py), so it can
    # find them in a build it was not written for. Each covers the address just
    # past its patched bytes, where a stub resumes or a call returns.
    import sigs
    module = sigs.Module(b.SRC)
    sites = [("chooser_call", b.CALL_SITE, [b.CALL_SITE + 5]),
             ("chooser", b.STOCK_CHOOSER, [b.STOCK_CHOOSER + 32]),
             ("move_call", b.MOVE_CALL_SITE, [b.MOVE_CALL_SITE + len(b.MOVE_DISPLACED)]),
             ("setter", b.SETTER_RVA, [b.SETTER_RVA + len(b.SETTER_STOCK)]),
             ("is_selected", b.IS_SELECTED_RVA, [b.IS_SELECTED_RVA + len(b.IS_SELECTED_STOCK)])]
    sites += [(label, rva, [rva + len(bytes.fromhex(d))]) for rva, d, label, _ in b.TRACE_HOOKS]
    # An in-place edit's rel8 targets, and its rel32 ones inside the same
    # function, must move with it, so its signature covers them. Every rel32
    # it carries is also listed for the injector to re-aim, which matters for
    # the ones into another function; their targets lie in some other site.
    edit_fixups = []
    names = ("select_is", "select_squad", "select_toggle", "select_type")
    for name, (rva, edit_before, edit_after) in zip(names, b.SELECTION_EDITS):
        cover = [rva + len(edit_before)]
        for field, target in sigs.branch_targets(edit_after, rva):
            if field is None or img.function_of(target) == img.function_of(rva):
                cover.append(target)
            if field is not None:
                edit_fixups.append({"edit_rva": rva, "edit_offset": field, "edit_target": target})
        sites.append((name, rva, cover))
    # posture, the prone query, firing mode and the census: each detour's
    # signature covers its resume point, each retargeted call its return point
    # (and a thunk's padding byte, which takes the ret)
    sites += [(label, rva, [rva + len(displaced)]) for rva, displaced, label
              in b.POSTURE_HOOKS + [b.PRONE_HOOK] + b.FIRING_HOOKS + b.AMMO_HOOKS
              + [b.CENSUS_HOOK]]
    # the reader hooks' signatures must also cover where each jumps back into
    # fn_1157dd, since the injector re-aims those rel32s from the block
    sites += [
        ("ammo_reader_owner", 0x1158aa, [0x1158b1]),
        ("ammo_reader_check1", 0x115a50, [0x115a56, 0x115a5d]),
        ("ammo_reader_check2", 0x115a89, [0x115a93, 0x115c0f]),
    ]
    sites += [(label, site, [site + len(b.pose_site(site, stock, thunk)[0])])
              for site, stock, label, thunk in b.POSE_CALLS]
    sites += [(f"move_posture_{i + 1}", site, [site + len(b.MOVE_POSTURE_READ)])
              for i, site in enumerate(b.MOVE_POSTURE_SITES)]
    sites += [(label, rva, [rva + len(call)]) for rva, call, label in b.FIRING_CALLS]
    sites += [(f"ammo_gate_{i}", rva, [rva + len(bytes.fromhex(call))])
              for i, (rva, call, _) in enumerate(b.AMMO_GATE_CALLS)]
    sites += [(label, site, [site + len(displaced)])
              for site, displaced, label, _ in ORDER_CALLS + REGION_CALLS]
    sites.append(("building_has_places", 0x65060, [0x6507f]))
    sites.append(("building_reserve_next", 0x6577f, [0x65784]))
    sites.append(("region_native_eligible", 0x418000, [0x418020]))
    sites.append((REGION_ICON_GATE[2], REGION_ICON_GATE[0], [0x41812e, 0x4181d3]))
    sites.append((BUILDING_SELECT[2], BUILDING_SELECT[0], [BUILDING_SELECT[0] + 5]))
    names = [name for name, _, _ in sites]
    if len(set(names)) != len(names):
        raise SystemExit(f"two sites share a name: {names}")
    site_entries = sigs.site_entries(module, sites)

    # Every address the injector moves with a site must lie in one's window,
    # its end included, as the injector's Moves::at requires; a hook added
    # without a signature is caught here rather than refused in a player's game.
    windows = [(e["site_start"], e["site_start"] + len(e["site_pattern"]) // 2)
               for e in site_entries]
    moved = [("the chooser call", b.CALL_SITE), ("the stock chooser", b.STOCK_CHOOSER),
             ("the move call", b.MOVE_CALL_SITE)]
    moved += [("a detour", d["hook_rva"]) for d in detours]
    moved += [("a trace resume point", f["trace_fix_rva"]) for f in trace_fixups]
    moved += [("a branch from the block", f["rel_target"]) for f in rel_fixups]
    moved += [("an in-place edit", rva) for rva, _, _ in b.SELECTION_EDITS]
    moved += [("an edit's branch target", f["edit_target"]) for f in edit_fixups]
    moved += [("a retargeted call", c["pose_site"]) for c in pose_calls]
    moved += [("the pose split's return point", f["delta_from"]) for f in delta_fixups]
    bare = [f"{rva:#x} ({what})" for what, rva in moved
            if not any(start <= rva <= end for start, end in windows)]
    if bare:
        raise SystemExit("no site's signature covers " + ", ".join(bare))

    # The injector's self test plants whatever the descriptor says, so it
    # cannot notice a wrong expectation: check each one against the DLL here.
    expected = [(b.CALL_SITE, before), (b.MOVE_CALL_SITE, b.MOVE_DISPLACED),
                (b.STOCK_CHOOSER, img.read(b.STOCK_CHOOSER, 32))]
    expected += [(d["hook_rva"], bytes.fromhex(d["hook_displaced"])) for d in detours]
    expected += [(rva, edit_before) for rva, edit_before, _ in b.SELECTION_EDITS]
    expected += [(c["pose_site"], bytes.fromhex(c["pose_before"])) for c in pose_calls]
    for rva, want in expected:
        if img.read(rva, len(want)) != want:
            raise SystemExit(f"the descriptor would expect {want.hex()} at {rva:#x}, "
                             f"but {b.SRC} holds {img.read(rva, len(want)).hex()}")
    if before[0] != 0xe8:
        raise SystemExit("call_before is not a call")

    owners = {}
    for feature, hooks in [
        (2, [(b.SETTER_RVA,), (b.IS_SELECTED_RVA,), BUILDING_SELECT, REGION_ICON_GATE]),
        (4, b.POSTURE_HOOKS + [b.PRONE_HOOK]),
        (5, b.FIRING_HOOKS),
        (6, b.AMMO_HOOKS + b.AMMO_READER_HOOKS),
        (7, b.TRACE_HOOKS + [b.CENSUS_HOOK]),
    ]:
        for hook in hooks:
            assert hook[0] not in owners
            owners[hook[0]] = feature
    for hook in detours:
        hook["hook_feature"] = owners[hook["hook_rva"]]
    call_owners = {site: 4 for site, *_ in b.POSE_CALLS}
    call_owners.update({site: 4 for site in b.MOVE_POSTURE_SITES})
    call_owners.update({site: 5 for site, *_ in b.FIRING_CALLS})
    call_owners.update({site: 6 for site, *_ in b.AMMO_GATE_CALLS})
    call_owners.update({site: feature for site, _, _, feature in ORDER_CALLS + REGION_CALLS})
    for call in pose_calls:
        call["pose_feature"] = call_owners[call["pose_site"]]

    for key, code in b.layout_gaps_for(b.LAYOUT):
        print(f"warning: layout entry {key} never applied; nearest source line: {code}",
              file=sys.stderr)
    gaps = b.cross_dll_gaps(b.SOURCES.get(id(b.LAYOUT), []), b.LAYOUT, b.GAME_LAYOUT)
    if gaps:
        raise SystemExit("logic payload operand(s) whose class moved in game.dll; "
                         "make them shared symbols:\n  " + "\n  ".join(
                             f"{name}: {code}   ({key} -> {new:#x})" for name, code, key, new in gaps))

    payload_path.parent.mkdir(exist_ok=True)
    payload_path.write_bytes(bytes(payload))
    descriptor_path.write_text(json.dumps({
        "feature_schema": 1,
        "source_sha256": sha,
        "code_bytes": len(code),
        "block_bytes": b.BLOCK_SIZE,
        "cursor_offset": b.CURSOR_OFFSET,
        "call_site": b.CALL_SITE,
        "call_before": before.hex(),
        # the move filter, at its own offset inside the same block
        "move_offset": b.MOVE_OFFSET,
        "move_bytes": len(move),
        "move_call_site": b.MOVE_CALL_SITE,
        "move_displaced": b.MOVE_DISPLACED.hex(),
        # jmp detours into the block: the diagnostic ones on the selection
        # manager, which share one tagged ring, then the soldier's setSelected
        "trace_offset": b.TRACE_OFFSET,
        "ammo_scratch": b.AMMO_SCRATCH,
        "setter_offset": b.SETTER_OFFSET,
        "detours": detours,
        "trace_fixups": trace_fixups,
        "rel_fixups": rel_fixups,
        "census_offset": b.CENSUS_TABLE,
        # lie down and stand up: calls retargeted into the block
        "pose_offset": b.POSE_OFFSET,
        "pose_calls": pose_calls,
        "delta_fixups": delta_fixups,
        "stock_chooser": b.STOCK_CHOOSER,
        # cheap version anchors the injector can check in memory, so a wrong
        # build is caught even before the file hash is computed
        "module_bytes": len(source),
        # what a loaded module reports is SizeOfImage, not the file length
        "image_bytes": img.pe.OPTIONAL_HEADER.SizeOfImage,
        "anchor_rva": b.STOCK_CHOOSER,
        "anchor": img.read(b.STOCK_CHOOSER, 32).hex(),
        # individual selection: the same getter and manager edits build.py
        # makes, so the injector takes the same build with it
        "select_is_rva": b.SELECTION_EDITS[0][0],
        "select_is_before": b.SELECTION_EDITS[0][1].hex(),
        "select_is_after": b.SELECTION_EDITS[0][2].hex(),
        "select_squad_rva": b.SELECTION_EDITS[1][0],
        "select_squad_before": b.SELECTION_EDITS[1][1].hex(),
        "select_squad_after": b.SELECTION_EDITS[1][2].hex(),
        "select_toggle_rva": b.SELECTION_EDITS[2][0],
        "select_toggle_before": b.SELECTION_EDITS[2][1].hex(),
        "select_toggle_after": b.SELECTION_EDITS[2][2].hex(),
        "select_type_rva": b.SELECTION_EDITS[3][0],
        "select_type_before": b.SELECTION_EDITS[3][1].hex(),
        "select_type_after": b.SELECTION_EDITS[3][2].hex(),
        # where each site is in a build this was not written for: found by its
        # signature at load time, and only when asked to (--scan)
        "sites": site_entries,
        "verified_sha": ",".join(b.VERIFIED_BUILDS),
        "edit_fixups": edit_fixups,
    }, indent=2) + "\n", encoding="utf-8", newline="\n")

    print(f"payload   {len(payload)} bytes -> {payload_path}: chooser {len(code)} at +0, "
          f"filter {len(move)} at +{b.MOVE_OFFSET:#x}, "
          f"manager trace {len(trace)} at +{b.TRACE_CODE_OFFSET:#x}, "
          f"setter {len(setter)} at +{b.SETTER_OFFSET:#x}, "
          f"pose split {len(pose)} at +{b.POSE_OFFSET:#x}, "
          f"posture gates {len(posture)} at +{b.POSTURE_OFFSET:#x}, "
          f"prone query {len(prone)} at +{b.PRONE_OFFSET:#x}, "
          f"firing mode {len(firing)} at +{b.FIRING_OFFSET:#x}, census at +{b.CENSUS_TABLE:#x} "
          f"({len(rel_fixups)} "
          f"branches into the module), "
          f"ammo mode {len(ammo)} at +{b.AMMO_OFFSET:#x}, "
          f"ring at +{b.TRACE_OFFSET:#x}")
    print(f"block     {b.BLOCK_SIZE:#x} bytes, position independent; cursor at "
          f"+{b.CURSOR_OFFSET:#x}")
    print(f"call site {b.CALL_SITE:#x} holds {before.hex()}; the injector "
          f"computes the new target")
    print(f"labels    " + " ".join(f"{k}={v:#x}" for k, v in labels.items()))
    print(f"descriptor {descriptor_path}")


main()
