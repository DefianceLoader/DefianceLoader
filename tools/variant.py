"""Resolve a build's patch units against that build's DLLs.

`tools/units.py --layout <profile>` assembles the shared patch source with the
build's own class offsets. The addresses in the units it writes are still the
reference build's, because unit code is position independent and every
address is re-aimed where it lands. This tool finishes the job offline: it
finds every site in the target build, rewrites every address each unit names,
and re-reads the bytes each write expects from the target.

A site is found, in order: by the profile's `logic_sites`/`game_sites`
override; through the build's base (tools/builds.py), whose resolved variant
says where the site is there, by that window's bytes (distance fields
wildcarded) or, when a field inside it moved, operand-agnostically; and by the
reference's own signature, likewise. Neighbouring builds differ least, so the
base usually finds a site the reference's signature no longer does. The result
is a set of units that apply directly to that build, selected by its hashes,
with no runtime relocation.

    python tools/variant.py tools/layouts/gog-2026-09-14.json \
        bin/gog/2026-09-14/logic.dll bin/gog/2026-09-14/game.dll

It reads the assembled out/units-<name>/, leaves it as it is (so it can be
rerun), and writes the resolved units to tools/variants/<name>/units/. It
refuses, so a build it cannot resolve is never shipped, if:

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
import collections, hashlib, json, pathlib, struct, sys
import builds
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

    def __init__(self, descriptor, ref, target, overrides=None, via=None):
        """`via` is the base build's (sigs.Module, resolved descriptor), when
        the base is not the reference."""
        overrides = overrides or {}
        base_sites = {e["site_name"]: e for e in via[1]["sites"]} if via else {}
        self.windows, failed = [], []
        self.how = collections.Counter()
        for entry in descriptor["sites"]:
            try:
                if entry["site_name"] in overrides:
                    # re-derived by hand where no signature follows the site;
                    # the profile names the new address
                    new_site = overrides[entry["site_name"]]
                    new_start = new_site - entry["site_offset"]
                    self.how["override"] += 1
                else:
                    found = None
                    if entry["site_name"] in base_sites:
                        found = self.locate_via(entry, base_sites[entry["site_name"]], via[0], target)
                    if found:
                        self.how["through the base"] += 1
                    else:
                        found = self.locate(entry, ref, target)
                        self.how["by the reference's signature"] += 1
                    new_start, new_site = found
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

    @staticmethod
    def locate_via(entry, base_entry, base, target):
        """(start, site) of the site through the base's resolved window, or
        None: the base's bytes there, whole instructions over the reference
        window's length, found once in the target exactly (distance fields
        wildcarded) or with every field wildcarded."""
        start, length = base_entry["site_start"], len(entry["site_pattern"]) // 2
        insns, end = [], start
        for ins in base.md.disasm(base.image[start:start + length + 16], start):
            if end >= start + length:
                break
            insns.append(ins)
            end = ins.address + ins.size
        if end < start + length:
            return None
        parts = [base.masked(ins) for ins in insns]
        exact = (b"".join(p for p, _ in parts), b"".join(m for _, m in parts))
        loose = bytearray(exact[1])
        at = 0
        for ins in insns:
            if ins.imm_size:
                loose[at + ins.imm_offset:at + ins.imm_offset + ins.imm_size] = bytes(ins.imm_size)
            if ins.disp_size:
                loose[at + ins.disp_offset:at + ins.disp_offset + ins.disp_size] = bytes(ins.disp_size)
            at += ins.size
        for pattern, mask in (exact, (exact[0], bytes(loose))):
            found = target.matches(pattern, mask)
            if len(found) > 1:
                # Identical twins (two copies of one function): the k-th of n
                # in the base is the k-th of n in the target.
                twins = base.matches(pattern, mask)
                if len(twins) == len(found) and start in twins:
                    found = [found[twins.index(start)]]
            if len(found) == 1:
                return found[0], found[0] + entry["site_offset"]
        return None

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


def resolve_unit(unit, ref, target, overrides=None, via=None):
    """A unit (tools/units.py) for the target build: every address it names
    moved with its site, every byte it replaces re-read from the target."""
    sites = Sites(unit, ref, target, overrides, via)
    new = dict(unit)
    new["source_sha256"] = target.sha256
    writes, stocks = [], {}
    for write in unit["writes"]:
        rva = sites.at(write["rva"], f"{unit['name']} write")
        before = check_shape(ref, write["rva"], unhex(write["before"]), target, rva, f"{unit['name']} write")
        moved = {**write, "rva": rva, "before": before.hex()}
        if write.get("stock"):
            reached = rva + 5 + struct.unpack_from("<i", before, 1)[0]
            if stocks.setdefault(write["stock"], reached) != reached:
                raise SystemExit(f"sites that reached {write['stock']:#x} now reach {reached:#x} "
                                 f"and {stocks[write['stock']]:#x}")
            moved["stock"] = reached
        writes.append(moved)
    by_rva = {w["rva"]: w for w in writes}
    fixups = []
    for fix in unit["fixups"]:
        kind = fix["kind"]
        if kind in ("abs64", "rel32"):
            fix = {**fix, "target": sites.at(fix["target"], f"a {kind} target")}
        elif kind == "delta":
            if fix["to"] not in stocks:
                raise SystemExit(f"a return point reaches {fix['to']:#x}, which no retargeted call resolved")
            fix = {**fix, "from": sites.at(fix["from"], "a return point"), "to": stocks[fix["to"]]}
        elif kind == "edit":
            rva, reach = sites.at(fix["rva"], "an edit"), sites.at(fix["target"], "an edit target")
            edit = by_rva.get(rva)
            if edit is None or edit["kind"] != "edit":
                raise SystemExit(f"the edit fixup at {fix['rva']:#x} names no edit")
            after = bytearray(unhex(edit["after"]))
            after[fix["offset"]:fix["offset"] + 4] = rel32(rva + fix["offset"] + 4, reach)
            edit["after"] = bytes(after).hex()
            fix = {**fix, "rva": rva, "target": reach}
        fixups.append(fix)
    new["writes"] = sorted(writes, key=lambda w: w["rva"])
    new["fixups"] = fixups
    new["natives"] = [{**n, "rva": sites.at(n["rva"], "a native entry")} for n in unit["natives"]]
    new["anchors"] = [{"rva": rva, "bytes": target.read(rva, len(unhex(a["bytes"])), "anchor").hex()}
                      for a, rva in ((a, sites.at(a["rva"], "anchor")) for a in unit["anchors"])]
    new["sites"] = sites.relocated()
    new["verified_sha"] = ""
    return new, sites.how


def resolve_units(profile, logic_dll, game_dll):
    """[(unit name, blob, resolved descriptor), ...] for the units
    `tools/units.py --layout` assembled into out/units-<name>/."""
    name = profile["name"]
    folder = pathlib.Path("out") / f"units-{name}"
    if not folder.is_dir():
        raise SystemExit(f"{folder} is missing; run tools/units.py --layout {name} first")
    base = builds.build(name).base
    via_folder = (None if base is None or base == builds.reference()
                  else pathlib.Path("tools") / "variants" / base.name / "units")
    targets = {"logic.dll": (Target(logic_dll), Image(str(builds.reference().logic))),
               "game.dll": (Target(game_dll), Image(str(builds.reference().game)))}
    modules = {}
    out = []
    for path in sorted(folder.glob("*.json")):
        unit = json.loads(path.read_text(encoding="utf-8"))
        target, ref = targets[unit["module"]]
        which = unit["module"].removesuffix(".dll")
        overrides = {site: int(address) for site, address in profile.get(f"{which}_sites", {}).items()}
        via = None
        if via_folder is not None:
            resolved = via_folder / path.name
            if not resolved.exists():
                raise SystemExit(f"{name}'s base {base.name} has no resolved {path.stem}; resolve it first")
            dll = base.require().logic if which == "logic" else base.require().game
            via = modules.setdefault(which, sigs.Module(str(dll))), json.loads(resolved.read_text(encoding="utf-8"))
        try:
            resolved_unit, how = resolve_unit(unit, ref, target, overrides, via)
        except SystemExit as error:
            raise SystemExit(f"{path.stem}: {error}")
        print(f"  {path.stem:<22} sites: " + ", ".join(f"{n} {h}" for h, n in how.items()))
        out.append((path.stem, path.with_suffix(".bin").read_bytes(), resolved_unit))
    return out


def main():
    if len(sys.argv) != 4:
        raise SystemExit(__doc__)
    profile = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
    staged = pathlib.Path("tools") / "variants" / profile["name"]
    # Resolve both before writing either, so a refusal leaves the tracked pair
    # as it was. The loader embeds them, so they are tracked: the target DLLs
    # are not in the repository and a fresh checkout cannot rebuild them.
    units = resolve_units(profile, sys.argv[2], sys.argv[3])
    folder = staged / "units"
    folder.mkdir(parents=True, exist_ok=True)
    for stale in folder.glob("*"):
        stale.unlink()
    for name, blob, descriptor in units:
        (folder / f"{name}.bin").write_bytes(blob)
        (folder / f"{name}.json").write_text(json.dumps(descriptor, indent=1) + "\n", encoding="utf-8", newline="\n")
    print(f"units  resolved for {profile['name']}: {len(units)} -> {folder}")


if __name__ == "__main__":
    main()
