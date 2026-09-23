"""Resolve a per-build payload descriptor against that build's DLLs.

`payload.py` and `icon.py`, run with `DEFIANCE_LAYOUT=<profile>`, assemble the
shared patch source with the build's own class offsets. The addresses in the
descriptor they write are still the reference build's, because the payload code
is position independent and every address is re-aimed at load time. This tool
finishes the job offline: it finds every site in the target build (its exact
signature, the operand-agnostic fallback when a field inside the window moved,
or the profile's `logic_sites`/`game_sites` override), rewrites every address
the descriptor names, and re-reads the bytes the descriptor expects from the
target. The result is a descriptor that applies directly to that build, selected
by its hash, with no runtime relocation.

    python tools/variant.py tools/layouts/gog-2026-09-14.json \
        bin/gog/logic-updated.dll bin/gog/game-updated.dll

It reads the assembled out/payload[-game]-<name>.{bin,json}, leaves them as
they are (so it can be rerun), and writes the resolved payloads to
tools/variants/<name>/. It refuses, so a build it cannot resolve is never
shipped, if:

- a site is missing or ambiguous;
- sites of one reference function now lie in different target functions (a
  recompiled function may shift its sites apart, but not split them);
- the instructions a write replaces are not the reference's with only
  immediates and displacements changed. The payload relies on their registers:
  `move_posture`, for one, reads the movement state from `rsi` and answers in
  `ebp`, so a site that reads the flag into another register belongs to other
  code, whatever its signature looks like;
- a branch that reached one function now reaches two, or a pose return point's
  stock function was not resolved.
"""
import hashlib, json, pathlib, struct, sys
sys.path.insert(0, "tools")
from pe import Image
import sigs


def operand_agnostic(img, start, end):
    """The window's bytes with every immediate and displacement wildcarded:
    the same opcodes and operands, whatever the field offsets became."""
    pat, mask = bytearray(), bytearray()
    for ins in img.md.disasm(img.read(start, end - start), start):
        pat += ins.bytes
        m = bytearray(b"\x01" * ins.size)
        if ins.imm_size:
            m[ins.imm_offset:ins.imm_offset + ins.imm_size] = b"\x00" * ins.imm_size
        if ins.disp_size:
            m[ins.disp_offset:ins.disp_offset + ins.disp_size] = b"\x00" * ins.disp_size
        mask += m
    return bytes(pat), bytes(mask)


def unhex(text):
    return bytes.fromhex(text)


def masked_instructions(md, code, address):
    """[(bytes with immediates and displacements zeroed, size), ...] for `code`,
    or None if it does not disassemble completely."""
    out, covered = [], 0
    for ins in md.disasm(code, address):
        raw = bytearray(ins.bytes)
        if ins.imm_size:
            raw[ins.imm_offset:ins.imm_offset + ins.imm_size] = bytes(ins.imm_size)
        if ins.disp_size:
            raw[ins.disp_offset:ins.disp_offset + ins.disp_size] = bytes(ins.disp_size)
        out.append((bytes(raw), ins.size))
        covered += ins.size
    return out if covered == len(code) else None


def check_shape(ref, ref_rva, ref_bytes, target, rva, what):
    """The target's bytes a write replaces, refused unless they are the
    reference's instructions with only immediates and displacements changed:
    the same opcodes, operand sizes and registers, whatever the fields became."""
    got = target.read(rva, len(ref_bytes), what)
    want = masked_instructions(ref.md, ref_bytes, ref_rva)
    have = masked_instructions(target.module.md, got, rva)
    if want is None or have is None or want != have:
        raise SystemExit(f"{what} at {rva:#x} holds {got.hex()}, not the reference's "
                         f"{ref_bytes.hex()} with only its fields moved")
    return got


def rel32(from_end, to):
    """The rel32 whose instruction ends at `from_end` reaches `to`."""
    return struct.pack("<i", to - from_end)


class Target:
    """The build a descriptor is being resolved for."""

    def __init__(self, path):
        self.module = sigs.Module(path)
        raw = pathlib.Path(path).read_bytes()
        self.sha256 = hashlib.sha256(raw).hexdigest()
        self.file_bytes = len(raw)
        self.image_bytes = self.module.pe.OPTIONAL_HEADER.SizeOfImage

    def matches(self, pattern, mask):
        return self.module.matches(pattern, mask)

    def image(self):
        return self.module.image

    def read(self, rva, length, what):
        out = self.module.image[rva:rva + length]
        if len(out) != length:
            raise SystemExit(f"{what} at {rva:#x} runs off the module")
        return out


class Sites:
    """Every site's home in the target, and the map that moves an address with
    the site whose window holds it."""

    def __init__(self, descriptor, ref, target, overrides=None):
        overrides = overrides or {}
        self.windows, failed = [], []
        for entry in descriptor["sites"]:
            try:
                if entry["site_name"] in overrides:
                    # re-derived by hand where the refactor left no signature to
                    # follow; the profile names the new address
                    new_site = overrides[entry["site_name"]]
                    new_start = new_site - entry["site_offset"]
                else:
                    new_start, new_site = self.locate(entry, ref, target)
            except SystemExit as error:
                failed.append(str(error))
                continue
            self.windows.append((entry, new_start, new_site))
        if failed:
            raise SystemExit("unresolved sites:\n  " + "\n  ".join(failed))
        # A site's group is the function holding it. A recompiled function may
        # move its sites by different amounts, but never into two functions.
        functions = target.module.functions
        self.groups, members = {}, {}
        for entry, _new_start, new_site in self.windows:
            group = (functions.function_of(new_site) or (new_site,))[0]
            self.groups[entry["site_name"]] = group
            members.setdefault(entry["site_group"], {}).setdefault(group, []).append(entry["site_name"])
        split = {old: found for old, found in members.items() if len(found) > 1}
        if split:
            raise SystemExit("sites of one function now lie in different functions:\n  " + "\n  ".join(
                f"{old:#x}: " + "; ".join(f"{', '.join(names)} in {new:#x}" for new, names in found.items())
                for old, found in split.items()))

    def locate(self, entry, ref, target):
        text = entry["site_pattern"]
        pattern = bytes(int(text[i:i + 2], 16) if text[i:i + 2] != "??" else 0
                        for i in range(0, len(text), 2))
        mask = bytes(0 if text[i:i + 2] == "??" else 1 for i in range(0, len(text), 2))
        found = target.matches(pattern, mask)
        start, offset = entry["site_start"], entry["site_offset"]
        if len(found) == 1:
            return found[0], found[0] + offset
        if len(found) > 1:
            raise SystemExit(f"{entry['site_name']}: its signature matches {len(found)} places")
        # A field inside the window moved: retry with the fields wildcarded.
        found = target.matches(*operand_agnostic(ref, start, start + len(mask)))
        if len(found) != 1:
            raise SystemExit(f"{entry['site_name']}: {len(found)} operand-agnostic matches")
        new_start = found[0]
        ref_ins = list(ref.md.disasm(ref.read(start, len(mask)), start))
        new_ins = list(target.module.md.disasm(target.image()[new_start:new_start + len(mask)], new_start))
        index = next((i for i, ins in enumerate(ref_ins) if ins.address == start + offset), None)
        if index is None or index >= len(new_ins):
            raise SystemExit(f"{entry['site_name']}: cannot align its window in the target")
        return new_start, new_ins[index].address

    def relocated(self):
        """The site entries at the target's addresses. `site_start` moves with
        the window it names and `site_group` becomes the target function holding
        it, so a caller that ties a detour to its site by `site_start == hook_rva`
        still finds it, and the sites of one function still share a group."""
        out = []
        for entry, new_start, _new_site in self.windows:
            moved = dict(entry)
            moved["site_start"] = new_start
            moved["site_group"] = self.groups[entry["site_name"]]
            out.append(moved)
        return out

    def at(self, rva, what):
        # the window's end is included: a resume point may sit just past it
        for entry, new_start, new_site in self.windows:
            start, length = entry["site_start"], len(entry["site_pattern"]) // 2
            if start <= rva <= start + length:
                return rva + (new_start - start)
        raise SystemExit(f"no site covers {rva:#x} ({what})")


def resolve_logic(descriptor, ref, target, overrides=None):
    sites = Sites(descriptor, ref, target, overrides)
    new = dict(descriptor)
    new["source_sha256"] = target.sha256
    new["module_bytes"] = target.file_bytes
    new["image_bytes"] = target.image_bytes
    new["stock_chooser"] = sites.at(descriptor["stock_chooser"], "stock chooser")
    new["call_site"] = sites.at(descriptor["call_site"], "chooser call")
    call_before = b"\xe8" + rel32(new["call_site"] + 5, new["stock_chooser"])
    if target.read(new["call_site"], 5, "chooser call") != call_before:
        raise SystemExit(f"the chooser call at {new['call_site']:#x} does not reach {new['stock_chooser']:#x}")
    new["call_before"] = call_before.hex()
    new["move_call_site"] = sites.at(descriptor["move_call_site"], "move call")
    new["move_displaced"] = check_shape(ref, descriptor["move_call_site"], unhex(descriptor["move_displaced"]),
                                        target, new["move_call_site"], "move site").hex()
    new["anchor_rva"] = sites.at(descriptor["anchor_rva"], "anchor")
    new["anchor"] = target.read(new["anchor_rva"], len(unhex(descriptor["anchor"])), "anchor").hex()

    edits = ("select_is", "select_squad", "select_toggle", "select_type")
    edit_rva = {}
    after = {}
    for name in edits:
        edit_rva[name] = sites.at(descriptor[f"{name}_rva"], name)
        new[f"{name}_rva"] = edit_rva[name]
        new[f"{name}_before"] = check_shape(ref, descriptor[f"{name}_rva"], unhex(descriptor[f"{name}_before"]),
                                            target, edit_rva[name], name).hex()
        after[name] = bytearray(unhex(descriptor[f"{name}_after"]))
    if len(set(edit_rva.values())) != len(edits):
        raise SystemExit(f"the selection edits do not have distinct sites: {edit_rva}")
    edit_fixups = []
    for fix in descriptor["edit_fixups"]:
        moved = sites.at(fix["edit_rva"], "an edit")
        matches = [n for n in edits if edit_rva[n] == moved]
        if len(matches) != 1:
            raise SystemExit(f"the fixup at {fix['edit_rva']:#x} names {len(matches)} known edits")
        name = matches[0]
        field = fix["edit_offset"]
        reach = sites.at(fix["edit_target"], "an edit target")
        after[name][field:field + 4] = rel32(edit_rva[name] + field + 4, reach)
        edit_fixups.append({"edit_rva": edit_rva[name], "edit_offset": field, "edit_target": reach})
    for name in edits:
        new[f"{name}_after"] = bytes(after[name]).hex()
    new["edit_fixups"] = edit_fixups

    new["detours"] = [{"hook_rva": rva, "hook_entry": hook["hook_entry"], "hook_feature": hook["hook_feature"],
                       "hook_displaced": check_shape(ref, hook["hook_rva"], unhex(hook["hook_displaced"]),
                                                     target, rva, "a detour").hex()}
                      for hook, rva in ((h, sites.at(h["hook_rva"], "a detour")) for h in descriptor["detours"])]
    new["trace_fixups"] = [{"trace_fix_offset": f["trace_fix_offset"],
                            "trace_fix_rva": sites.at(f["trace_fix_rva"], "a trace resume")} for f in descriptor["trace_fixups"]]
    new["rel_fixups"] = [{"rel_offset": f["rel_offset"], "rel_feature": f["rel_feature"],
                          "rel_target": sites.at(f["rel_target"], "a branch target")} for f in descriptor["rel_fixups"]]

    new["pose_calls"], stocks = [], {}
    for call in descriptor["pose_calls"]:
        rva = sites.at(call["pose_site"], "a retargeted call")
        before = check_shape(ref, call["pose_site"], unhex(call["pose_before"]), target, rva, "a retargeted call")
        stock = call["pose_stock"]
        if stock:
            reached = rva + 5 + struct.unpack_from("<i", before, 1)[0]
            if stocks.setdefault(stock, reached) != reached:
                raise SystemExit(f"sites that reached {stock:#x} now reach {reached:#x} and {stocks[stock]:#x}")
            stock = reached
        new["pose_calls"].append({"pose_site": rva, "pose_entry": call["pose_entry"], "pose_tail": call["pose_tail"],
                                  "pose_stock": stock, "pose_feature": call["pose_feature"], "pose_before": before.hex()})
    new["delta_fixups"] = []
    for f in descriptor["delta_fixups"]:
        if f["delta_to"] not in stocks:
            raise SystemExit(f"a pose return point reaches {f['delta_to']:#x}, which no retargeted call resolved")
        new["delta_fixups"].append({"delta_offset": f["delta_offset"],
                                    "delta_from": sites.at(f["delta_from"], "a pose return point"),
                                    "delta_to": stocks[f["delta_to"]]})
    new["sites"] = sites.relocated()
    new["verified_sha"] = ""
    return new


def resolve_game(descriptor, ref, target, overrides=None):
    sites = Sites(descriptor, ref, target, overrides)
    new = dict(descriptor)
    new["source_sha256"] = target.sha256
    new["module_bytes"] = target.file_bytes
    new["image_bytes"] = target.image_bytes
    new["anchor_rva"] = sites.at(descriptor["anchor_rva"], "anchor")
    new["anchor"] = target.read(new["anchor_rva"], len(unhex(descriptor["anchor"])), "anchor").hex()
    new["hooks"] = [{"rva": rva, "entry": hook["entry"], "hook_feature": hook["hook_feature"],
                     "displaced": check_shape(ref, hook["rva"], unhex(hook["displaced"]), target, rva, "a hook").hex()}
                    for hook, rva in ((h, sites.at(h["rva"], "a hook")) for h in descriptor["hooks"])]
    new["fixups"] = [{"offset": f["offset"], "what": f.get("what", ""), "fixup_feature": f["fixup_feature"],
                      "target_rva": sites.at(f["target_rva"], "a fixup target")} for f in descriptor["fixups"]]
    new["sites"] = sites.relocated()
    new["verified_sha"] = ""
    return new


def resolve_profile(profile, logic_dll, game_dll):
    """[(which, payload bytes, resolved descriptor), ...] for a profile whose
    payloads `payload.py`/`icon.py` assembled into out/."""
    name = profile["name"]
    out = []
    for which, dll, ref_path, stem, resolver in (
        ("logic", logic_dll, "bin/logic.orig.dll", f"payload-{name}", resolve_logic),
        ("game", game_dll, "bin/game.orig.dll", f"payload-game-{name}", resolve_game),
    ):
        payload_path, descriptor_path = pathlib.Path("out") / f"{stem}.bin", pathlib.Path("out") / f"{stem}.json"
        if not descriptor_path.exists():
            raise SystemExit(f"{descriptor_path} is missing; assemble with DEFIANCE_LAYOUT={name} first")
        descriptor = json.loads(descriptor_path.read_text(encoding="utf-8"))
        overrides = {site: int(address) for site, address in profile.get(f"{which}_sites", {}).items()}
        out.append((which, payload_path.read_bytes(), resolver(descriptor, Image(ref_path), Target(dll), overrides)))
    return out


def main():
    if len(sys.argv) != 4:
        raise SystemExit(__doc__)
    profile = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
    staged = pathlib.Path("tools") / "variants" / profile["name"]
    # Resolve both before writing either, so a refusal leaves the tracked pair
    # as it was. The loader embeds them, so they are tracked: the target DLLs
    # are not in the repository and a fresh checkout cannot rebuild them.
    resolved = resolve_profile(profile, sys.argv[2], sys.argv[3])
    staged.mkdir(parents=True, exist_ok=True)
    for which, payload, descriptor in resolved:
        (staged / f"{which}.bin").write_bytes(payload)
        (staged / f"{which}.json").write_text(json.dumps(descriptor, indent=2) + "\n", encoding="utf-8")
        print(f"{which:<6} resolved for {profile['name']}  sha256 {descriptor['source_sha256']}  -> {staged}")


if __name__ == "__main__":
    main()
