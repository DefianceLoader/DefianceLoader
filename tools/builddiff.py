"""Compare another build of a DLL with its base: the one the patch was written
for, or with `--base NAME` any supported build (tools/builds.py), usually the
new build's neighbour.

The signatures (tools/sigs.py) vouch for the patch sites; this vouches for the
rest. Every .pdata function of the reference build is matched in the other by
its bytes with distance fields wildcarded. A function that does not match, but
has a same-size counterpart at one of the common shifts, differs only in data
references (jump tables and image-relative tables that moved); anything else
changed. A changed object layout shows up as offsets changing across many
functions, so a handful of changes, none holding a site, means the payload's
assumptions carry over.

    python tools/builddiff.py logic bin/steam/2025-12-23/logic.dll
    python tools/builddiff.py game  bin/steam/2025-12-23/game.dll
    python tools/builddiff.py game  bin/gog/2026-09-25/game.dll --base gog-2026-09-14
"""
import bisect, collections, hashlib, json, sys
import builds
sys.path.insert(0, "tools")
import sigs

REFERENCE = {"logic": (str(builds.reference().logic), "out/payload.json"),
             "game": (str(builds.reference().game), "out/payload-game.json")}


def base_paths(which, name=None):
    """(DLL, descriptor) of the build compared against: `name`, or the reference.
    Another base's sites are its resolved variant's (tools/variants/)."""
    base = builds.build(name) if name else builds.reference()
    if base == builds.reference():
        return REFERENCE[which]
    dll = base.require().logic if which == "logic" else base.require().game
    return str(dll), str(builds.ROOT / "tools" / "variants" / base.name / f"{which}.json")


def base_option(argv):
    """`argv` without its `--base NAME`, and NAME (None without one)."""
    if "--base" not in argv:
        return list(argv), None
    i = argv.index("--base")
    if i + 1 >= len(argv):
        raise SystemExit("--base needs a build name")
    return argv[:i] + argv[i + 2:], argv[i + 1]


def functions(module):
    d = module.pe.OPTIONAL_HEADER.DATA_DIRECTORY[3]
    t = module.image[d.VirtualAddress:d.VirtualAddress + d.Size]
    return [(int.from_bytes(t[i:i + 4], "little"), int.from_bytes(t[i + 4:i + 8], "little"))
            for i in range(0, len(t), 12)]


def normalized(module, start, end):
    out = bytearray()
    for ins in module.md.disasm(module.image[start:end], start):
        b, m = module.masked(ins)
        out += bytes(x if k else 0 for x, k in zip(b, m))
    return hashlib.blake2b(out, digest_size=16).digest()


def shape(module, start, end):
    """The instruction sequence, mnemonics and sizes only: equal for two copies
    of a function that differ only in the data they reference."""
    return [(i.mnemonic, i.size) for i in module.md.disasm(module.image[start:end], start)]


def main(which, other_path, base=None):
    reference, descriptor = base_paths(which, base)
    ref, other = sigs.Module(reference), sigs.Module(other_path)
    rf, of = functions(ref), functions(other)
    where = collections.defaultdict(list)
    for s, e in of:
        where[normalized(other, s, e)].append(s)
    shifts, unmatched = collections.Counter(), []
    for s, e in rf:
        found = where.get(normalized(ref, s, e))
        if found:
            shifts[min(found, key=lambda x: abs(x - s)) - s] += 1
        else:
            unmatched.append((s, e))
    common = [d for d, _ in shifts.most_common(8)]
    starts, ends = [s for s, _ in of], dict(of)
    data_only, changed = 0, []
    for s, e in unmatched:
        same, mine = False, None
        for d in common:
            i = bisect.bisect_left(starts, s + d)
            for j in (i - 1, i):
                if 0 <= j < len(starts) and abs(starts[j] - (s + d)) < 0x40 and \
                        ends[starts[j]] - starts[j] == e - s:
                    mine = mine or shape(ref, s, e)
                    same |= shape(other, starts[j], ends[starts[j]]) == mine
        if same:
            data_only += 1
        else:
            changed.append((s, e))
    try:
        sites = json.load(open(descriptor))["sites"]
    except OSError:
        raise SystemExit(f"{descriptor} is missing: the base has no resolved sites to check")
    touched = [x["site_name"] for x in sites
               for s, e in changed if s <= x["site_start"] + x["site_offset"] < e]
    print(f"{which}.dll: {len(rf)} functions, {len(rf) - len(unmatched)} identical, "
          f"{data_only} differ only in data references, {len(changed)} changed")
    print("  common shifts:", ", ".join(f"{d:+#x} x{n}" for d, n in shifts.most_common(6)))
    print("  changed functions holding a patch site:", ", ".join(touched) or "none")
    return 1 if touched else 0


if __name__ == "__main__":
    args, base = base_option(sys.argv[1:])
    sys.exit(main(args[0], args[1], base))
