"""Assemble patch/icon-squad.asm for the injector.

This is the only patch that targets game.dll, and it never becomes a file
patch: the block is allocated in the running process and the hooks are
written into the loaded image.

The payload is position independent except for references back into the
module, which cannot be baked as rel32 from an allocated block. Those are
`mov r11, imm64` slots carrying a placeholder; the descriptor records where
each one sits and which rva it wants, and the injector writes base + rva.
"""
import hashlib, json, os, pathlib, struct, sys
import builds
sys.path.insert(0, "tools")
import build as b
from pe import Image

# The GOG game.dll the patch was written against, beside its logic.dll
# rather than read from the install, which may hold another store's build.
GAME = str(builds.reference().game)
PAYLOAD = pathlib.Path("out/payload-game.bin")
DESCRIPTOR = pathlib.Path("out/payload-game.json")

BLOCK_SIZE = 0x2000
BUILDING_TAB_OFFSET = 0x1000  # keep existing code and diagnostic storage stable
# engine-owned reference, world/context/player identities, the narrowed squad
# (+20, compared only), the squad TAB modifier's virtual key (+28, which Core
# writes from the selection feature's setting) and the preview's shown entity
# and selection signature (+30, +38)
BUILDING_STATE_OFFSET = 0x1fc0
TAB_MODIFIER_OFFSET = BUILDING_STATE_OFFSET + 0x28
SUBSET_STATE_OFFSET = BUILDING_STATE_OFFSET + 0x30
PREVIEW_OFFSET = 0x1a00  # between the building TAB code and the engine-owned state
SUBSET_OFFSET = 0x1b00   # the preview's selection marks, after the weapon guard
TRACE_OFFSET = 0xf00   # squad_of records each hop here for --probe (0x60 bytes)
EXPECT_SOURCE_SHA = "f0184b9fe358172c83261419c8ba3d822a0aa6b06ed3cddb2f7aa3ebb9653db4"
# Other builds the signatures have been checked against (tools/sigs.py and
# --scan-check), which the injector then relocates to without --scan. Steam:
# the same source built 15 minutes after the GOG one, 23 Dec 2025.
VERIFIED_BUILDS = {
    "dc10419f417aed4ecff348b7c96b3c7c574a9a2541f76c5fbc35eb92dbc716d6": "Steam",
}

# Each hook displaces a whole number of bytes; a five-byte jmp fits and the
# rest are nops. Each span includes every instruction redone by its stub.
HOOKS = [
    # rva, the bytes that must be there, the label to jump to
    (0x1f4370, "48895c24084889742410", "single_click"),
    (0x1f4200, "4889742410574883ec40", "double_click"),
    # the three-byte call plus the mov after it, both redone in the stub
    (0x3325dc, "ff5060488b5c2430", "world_select"),
    # Shift+click's toggle, a five-byte mov redone in the stub
    (0x3325ab, "488b5c2430", "world_toggle"),
    (0x348fa4, "498b064c8bc7488d542420498bceff9080000000", "world_double"),
    # Replace the already-selected shortcut; Ctrl rejects squad-container hits.
    (0x352de6, "488b86a0000000", "ctrl_select"),
    (0x3f06b, "4c8bc6488bd5488bcfe857010000", "ammo_panel"),
    # The attack command's per-recipient test: also require enabled ammunition.
    (0x3274d2, "488b01ff9068030000", "attack_recipient"),
    # The order buttons' attack test: any selected unit, not the last one.
    (0x23f781, "498b4500498bcd", "attack_button"),
    (0x34c2e6, "ff9040050000", "building_tab_collect"),
    (0x34c386, "e865a5ceff", "building_tab_apply"),
    (0x31bfa0, "ff9040050000", "building_tab_refresh"),
    # The building panel's squad icons: select only the members inside.
    (0x364b97, "488bc8488bd84c8b00", "building_icon_select"),
    # The squad preview's per-soldier weapon: keep the first match instead of
    # the last one the provider's scan leaves behind.
    (0x216d8c, "85c075513883a9000000754d", "preview_primary"),
    (0x216dd8, "4c894d8f4d8be9eb04", "preview_secondary"),
    # The out-of-mission squad-management panel builds its own descriptors and
    # overwrote every one with the last weapon; keep the first.
    (0x29acf0, "488b82b800000048894128", "preview_squad"),
    # The squad preview's unselected soldiers, and its rebuild when the
    # squad's selection changes.
    (0x216c94, "c64587000f57c0", "subset_mark"),
    (0x36449c, "498b064885c07409", "subset_refresh"),
]

# placeholder -> the rva the injector should resolve into that slot, and the
# feature that owns the slot (2 selection, 6 ammunition) so a selective
# relocation can leave a disabled feature's slot unresolved.
FIXUPS = [
    (0xaaaaaaaaaaaaaaa1, 0x1f437a, "resume fn_1f4370 + 0xa", 2),
    (0xaaaaaaaaaaaaaaa2, 0x1f42d0, "fn_1f42d0, the row resolver", 2),
    (0xaaaaaaaaaaaaaaa3, 0x1f4215, "resume fn_1f4200 + 0x15", 2),
    (0xaaaaaaaaaaaaaaa4, 0x3325e4, "resume SmartCursorCmdSelect select + 8", 2),
    (0xaaaaaaaaaaaaaaa5, 0x3325b0, "resume SmartCursorCmdSelect toggle + 0xb", 2),
    (0xaaaaaaaaaaaaaaa9, 0x348fb8, "resume world same-type selection", 2),
    (0xaaaaaaaaaaaaaaab, 0x352e76, "ignore Ctrl squad-icon hits", 2),
    (0xaaaaaaaaaaaaaaac, 0x352e3f, "the cursor's select-command builder", 2),
    (0xaaaaaaaaaaaaaaae, 0x3f079, "resume ammunition menu fill", 6),
    (0xaaaaaaaaaaaaaaaf, 0x3f1d0, "AmmunitionMenu::fillSlot", 6),
    (0xaaaaaaaaaaaaaab0, 0x3f800, "AmmunitionMenu::hideSlot", 6),
    (0xaaaaaaaaaaaaaab1, 0x2cb730, "text label setter", 6),
    (0xaaaaaaaaaaaaaab7, 0x2c3380, "progress bar refresh", 6),
    (0xaaaaaaaaaaaaaab8, 0x3274db, "resume the attack recipient test", 6),
    (0xaaaaaaaaaaaaaab9, 0x23f7df, "resume the order buttons after the attack test", 6),
    (0xaaaaaaaaaaaaaab2, 0x34c2ec, "resume building TAB collection", 2),
    (0xaaaaaaaaaaaaaab3, 0x34c38b, "resume building TAB selection", 2),
    (0xaaaaaaaaaaaaaab4, 0x40e80, "UI raw-entity vector insertion", 2),
    (0xaaaaaaaaaaaaaab5, 0x368f0, "UI entity focus reference assignment", 2),
    (0xaaaaaaaaaaaaaab6, 0x31bfa6, "resume building cycle validation", 2),
    (0xaaaaaaaaaaaaaaba, 0x364bb4, "resume the building panel icon click", 2),
    (0xaaaaaaaaaaaaaac1, 0x216de5, "resume the preview gun scan", 10),
    (0xaaaaaaaaaaaaaac2, 0x216d98, "resume the preview gun filter", 10),
    (0xaaaaaaaaaaaaaac3, 0x29acff, "resume the squad preview gun scan", 10),
    (0xaaaaaaaaaaaaaac4, 0x216c9b, "resume the preview soldier's descriptor", 2),
    (0xaaaaaaaaaaaaaac5, 0x3644b2, "resume the panel's shown-entity test", 2),
    (0xaaaaaaaaaaaaaac6, 0x364563, "the panel's preview rebuild", 2),
]

# placeholder -> a Win32 export the injector resolves by name, since the
# injected code has to call it and no module base plus rva can name it
EXPORTS = [
    (0xaaaaaaaaaaaaaaa6, "user32.dll", "GetAsyncKeyState"),   # world_select
    (0xaaaaaaaaaaaaaaa8, "user32.dll", "GetAsyncKeyState"),   # world_toggle
    (0xaaaaaaaaaaaaaaad, "user32.dll", "GetAsyncKeyState"),   # ctrl_select
    (0xaaaaaaaaaaaaaabb, "user32.dll", "GetAsyncKeyState"),   # squad TAB modifier
]

# read by the injector before it writes anything, as a version check on a part
# of the module the patch does not touch
ANCHOR_RVA = 0x1f42d0
# Named functions with their own signatures: fixup targets, and the lobby
# connection (Leonardo::Network::connectToLobbyServer), which Core hooks in Rust
# to keep a game with gameplay plugins offline (plugins/core/src/multiplayer.rs).
CALLEE_SITES = {0x3f1d0: "ammo_fill_slot", 0x3f800: "ammo_hide_slot", 0x40e80: "focus_append",
                0x368f0: "focus_assign",
                0x2cb730: "ammo_label_text", 0x2c3380: "ammo_progress_refresh",
                0x1b30a0: "lobby_connect",
                # TacticalMapGameState's constructor and destructor: Core counts
                # loaded missions for hot reload (plugins/core/src/session.rs)
                0x3477d0: "tactical_state_ctor", 0x347ba0: "tactical_state_dtor"}


def main():
    source = pathlib.Path(GAME).read_bytes()
    sha = hashlib.sha256(source).hexdigest()
    if sha != EXPECT_SOURCE_SHA:
        raise SystemExit(f"{GAME} is sha256 {sha}, not the build this patch was "
                         f"written for ({EXPECT_SOURCE_SHA})")
    if "--layout" in sys.argv:
        os.environ["DEFIANCE_LAYOUT"] = sys.argv[sys.argv.index("--layout") + 1]
    name, profile = b.active_profile()
    if profile:
        b.load_layout(profile)
    payload_path = PAYLOAD if not name else PAYLOAD.with_name(f"payload-game-{name}.bin")
    descriptor_path = DESCRIPTOR if not name else DESCRIPTOR.with_name(f"payload-game-{name}.json")
    img = Image(GAME)

    code, labels = b.assemble(
        pathlib.Path("patch/icon-squad.asm").read_text().splitlines(), 0, TRACE_OFFSET,
        layout=b.GAME_LAYOUT, symbols=b.GAME_SYMBOLS)
    if len(code) > TRACE_OFFSET:
        raise SystemExit(f"{len(code)} bytes would overlap the trace at {TRACE_OFFSET:#x}")
    tab, tab_labels = b.assemble(pathlib.Path("patch/building-control.asm").read_text().splitlines(),
                                 BUILDING_TAB_OFFSET, TRACE_OFFSET, BUILDING_STATE_OFFSET,
                                 layout=b.GAME_LAYOUT, symbols=b.GAME_SYMBOLS)
    if BUILDING_TAB_OFFSET + len(tab) > BUILDING_STATE_OFFSET:
        raise SystemExit("building TAB code exceeds game payload storage")
    code += bytes(BUILDING_TAB_OFFSET - len(code)) + tab
    labels.update(tab_labels)

    # The preview weapon guard shares the block, after the building TAB code.
    preview, preview_labels = b.assemble(
        pathlib.Path("patch/preview-weapon.asm").read_text().splitlines(), PREVIEW_OFFSET, TRACE_OFFSET,
        layout=b.GAME_LAYOUT, symbols=b.GAME_SYMBOLS)
    if len(code) > PREVIEW_OFFSET or PREVIEW_OFFSET + len(preview) > BUILDING_STATE_OFFSET:
        raise SystemExit("the preview weapon code does not fit before the engine state")
    code += bytes(PREVIEW_OFFSET - len(code)) + preview
    labels.update(preview_labels)
    subset, subset_labels = b.assemble(
        pathlib.Path("patch/preview-subset.asm").read_text().splitlines(), SUBSET_OFFSET, TRACE_OFFSET,
        SUBSET_STATE_OFFSET, layout=b.GAME_LAYOUT, symbols=b.GAME_SYMBOLS)
    if len(code) > SUBSET_OFFSET or SUBSET_OFFSET + len(subset) > BUILDING_STATE_OFFSET:
        raise SystemExit("the preview subset code does not fit before the engine state")
    code += bytes(SUBSET_OFFSET - len(code)) + subset
    labels.update(subset_labels)

    hooks = []
    for rva, displaced, label in HOOKS:
        want = bytes.fromhex(displaced)
        actual = img.read(rva, len(want))
        if actual != want:
            raise SystemExit(f"{rva:#x} holds {actual.hex()}, not {displaced}")
        if label not in labels:
            raise SystemExit(f"the payload has no {label} label")
        feature = (6 if label in ("ammo_panel", "attack_recipient", "attack_button")
                   else 10 if label.startswith("preview_") else 2)
        hooks.append({"rva": rva, "displaced": displaced, "entry": labels[label], "hook_feature": feature})

    fixups = []
    for placeholder, rva, what, feature in FIXUPS:
        at = code.find(struct.pack("<Q", placeholder))
        if at < 0:
            raise SystemExit(f"the payload has no placeholder for {what}")
        if code.find(struct.pack("<Q", placeholder), at + 1) >= 0:
            raise SystemExit(f"the placeholder for {what} appears more than once")
        fixups.append({"offset": at, "target_rva": rva, "what": what, "fixup_feature": feature})

    exports = []
    for placeholder, dll, name in EXPORTS:
        at = code.find(struct.pack("<Q", placeholder))
        if at < 0:
            raise SystemExit(f"the payload has no placeholder for {dll}!{name}")
        if code.find(struct.pack("<Q", placeholder), at + 1) >= 0:
            raise SystemExit(f"the placeholder for {dll}!{name} appears more than once")
        exports.append({"export_offset": at, "dll": dll, "name": name})

    # Signatures for every site the injector touches (tools/sigs.py). Each
    # fixup target goes with the nearest hook, whose signature then covers it
    # (resume points can sit in the next .pdata chunk). The row resolver and
    # fillSlot have their own sites because they are independent callees.
    import sigs
    module = sigs.Module(GAME)
    cover = {rva: [rva + len(bytes.fromhex(displaced))] for rva, displaced, _ in HOOKS}
    for _, target, what, _ in FIXUPS:
        if target == ANCHOR_RVA or target in CALLEE_SITES:
            continue
        nearest = min(cover, key=lambda rva: abs(target - rva))
        if abs(target - nearest) > 0x100:
            raise SystemExit(f"no hook near the fixup target {target:#x} ({what})")
        cover[nearest].append(target)
    sites = [(label, rva, cover[rva]) for rva, _, label in HOOKS]
    sites.append(("anchor", ANCHOR_RVA, [ANCHOR_RVA + 32]))
    sites.extend((name, rva, [rva + 32]) for rva, name in CALLEE_SITES.items())
    site_entries = sigs.site_entries(module, sites)

    for key, src in b.layout_gaps_for(b.GAME_LAYOUT):
        print(f"warning: layout entry {key} never applied; nearest source line: {src}",
              file=sys.stderr)
    gaps = b.cross_dll_gaps(b.SOURCES.get(id(b.GAME_LAYOUT), []), b.GAME_LAYOUT, b.LAYOUT)
    if gaps:
        raise SystemExit("game payload operand(s) whose class moved in logic.dll; "
                         "make them shared symbols:\n  " + "\n  ".join(
                             f"{name}: {code}   ({key} -> {new:#x})" for name, code, key, new in gaps))

    payload_path.parent.mkdir(exist_ok=True)
    payload_path.write_bytes(code)
    descriptor_path.write_text(json.dumps({
        "feature_schema": 1,
        "module": "game.dll",
        "source_sha256": sha,
        "module_bytes": len(source),
        "image_bytes": img.pe.OPTIONAL_HEADER.SizeOfImage,
        "code_bytes": len(code),
        "block_bytes": BLOCK_SIZE,
        "trace_offset": TRACE_OFFSET,
        "tab_modifier_offset": TAB_MODIFIER_OFFSET,
        "anchor_rva": ANCHOR_RVA,
        "anchor": img.read(ANCHOR_RVA, 32).hex(),
        "hooks": hooks,
        "fixups": fixups,
        "exports": exports,
        # where each site is in a build this was not written for (--scan)
        "sites": site_entries,
        "verified_sha": ",".join(VERIFIED_BUILDS),
    }, indent=2) + "\n", encoding="utf-8", newline="\n")

    print(f"payload   {len(code)} bytes -> {payload_path}, trace at +{TRACE_OFFSET:#x}")
    print(f"module    {GAME.rsplit('/', 1)[-1]}  sha256 {sha}")
    print(f"          {len(source)} bytes on disk, image {img.pe.OPTIONAL_HEADER.SizeOfImage:#x}")
    for hook in hooks:
        print(f"hook      {hook['rva']:#x} ({hook['displaced']}) -> block +{hook['entry']:#x}")
    for fix in fixups:
        print(f"fixup     +{fix['offset']:#x} <- base + {fix['target_rva']:#x}  ({fix['what']})")
    for exp in exports:
        print(f"export    +{exp['export_offset']:#x} <- {exp['dll']}!{exp['name']}")
    print(f"labels    " + " ".join(f"{k}={v:#x}" for k, v in labels.items()))
    print(f"descriptor {descriptor_path}")


if __name__ == "__main__":
    main()
