"""Draft a build's layout profile from its base's (tools/builds.py lineage).

A layout maps the reference build's offsets to one build's, because the patch
source is written against the reference. A new build is closest to its base,
the previous GOG build or the GOG build the same update shipped as, so its
changes are found against the base (tools/offsetmap.py `--base`) and composed
with the base's own table: reference -> base -> new.

Only the operands the payload assembles decide anything. Each one (a
`[reg + disp]` in `patch/`, keyed as tools/build.py's `apply_layout` keys it)
goes to its base offset through the base's table, then through the base-to-new
changes: an unambiguous move is applied. An ambiguous one (the shape moved for
some uses and not others, usually another class sharing it) needs a decision
by hand: tools/offsetmap.py `--base BASE --occurrences KEY` shows where it
moved, and the profile records the offset chosen under `<module>_decided`,
which later drafts apply. The base's other entries are carried over as they
are, and so are its named symbols, which offsetmap cannot see; the payload
tests on the new build are what check them.

    python tools/chainlayout.py gog-2026-09-25            # report
    python tools/chainlayout.py gog-2026-09-25 --write    # write or refresh its profile
    python tools/chainlayout.py steam-2026-09-22 --check  # compare with the tracked one

`--write` keeps the profile's decisions and writes an undecided key as null,
so the file is where the decision is made.

The draft names the base and the new DLLs' hashes and has no site overrides:
tools/variant.py finds a derived build's sites through its base's resolved ones.
"""
import hashlib, json, pathlib, sys
import builds
sys.path.insert(0, "tools")
import offsetmap
from build import SIMPLE_OPERAND

PATCH = builds.ROOT / "patch"


def payload_keys():
    """Every `mnemonic|reg|disp` a payload operand in `patch/` could be rewritten
    by, the stack frames aside."""
    keys = set()
    for path in sorted(PATCH.rglob("*.asm")):
        for raw in path.read_text(encoding="utf-8").splitlines():
            code = raw.split(";")[0].strip()
            if not code:
                continue
            mnemonic = code.split(None, 1)[0]
            for match in SIMPLE_OPERAND.finditer(code):
                reg, sign, disp = match.groups()
                if reg in ("rsp", "rbp") or disp is None:
                    continue
                keys.add(f"{mnemonic}|{reg}|{int(disp, 16) * (-1 if sign == '-' else 1):#x}")
    return keys


def profile_of(b):
    """The build's layout profile, or the reference's empty one."""
    return json.loads(b.layout.read_text(encoding="utf-8")) if b.layout else {}


def compose(which, base, target, keys, decided):
    """(table, moved, ambiguous, step) for `which` ("logic" or "game"): the
    base's table with each payload operand carried through the base-to-target
    changes. `decided` holds the offsets chosen by hand for ambiguous keys;
    an ambiguous key without one is returned in `ambiguous`."""
    table = dict(profile_of(base).get(f"{which}_layout", {}))
    dll = target.logic if which == "logic" else target.game
    step = offsetmap.analyze(which, str(dll), base=None if base == builds.reference() else base.name)
    moved, ambiguous = [], []
    for key in sorted(keys):
        mnemonic, reg, ref = key.split("|")
        ref = int(ref, 16)
        at_base = table.get(key, ref)
        base_key = f"{mnemonic}|{reg}|{at_base:#x}"
        if base_key in step["unambiguous"]:
            new = step["unambiguous"][base_key]
            moved.append((key, at_base, new))
            if new == ref:
                table.pop(key, None)
            else:
                table[key] = new
        elif base_key in step["ambiguous"]:
            new = decided.get(key)
            if new is None:
                ambiguous.append((key, at_base, step["ambiguous"][base_key]))
            elif new == ref:
                table.pop(key, None)
            else:
                table[key] = new
    return table, moved, ambiguous, step


def draft(name):
    target = builds.build(name).require()
    base = target.base
    if base is None:
        raise SystemExit(f"{name} is the reference; it has no base")
    keys = payload_keys()
    profile = {"name": name, "base": base.name}
    problems = []
    base_profile, own = profile_of(base), profile_of(target)
    for which in ("logic", "game"):
        dll = target.logic if which == "logic" else target.game
        decided = {k: v for k, v in own.get(f"{which}_decided", {}).items() if v is not None}
        table, moved, ambiguous, _step = compose(which, base, target, keys, decided)
        profile[f"{which}_layout"] = table
        profile[f"{which}_sha256"] = hashlib.sha256(dll.read_bytes()).hexdigest()
        profile[f"{which}_symbols"] = dict(base_profile.get(f"{which}_symbols", {}))
        profile[f"{which}_sites"] = {}
        profile[f"{which}_decided"] = {**decided, **{key: None for key, _, _ in ambiguous}}
        print(f"{which}: {len(table)} entries; {len(moved)} payload operands moved from {base.name}, "
              f"{len(decided)} decided by hand, {len(ambiguous)} undecided; "
              f"{len(profile[f'{which}_symbols'])} symbols carried unchecked")
        for key, old, new in moved:
            print(f"  moved     {key}: {old:#x} -> {new:#x}")
        for key, old, counts in ambiguous:
            problems.append(f"{which} {key} (at {old:#x} in {base.name}): "
                            + ", ".join(f"{int(v):#x} x{n}" for v, n in counts.items()))
    return profile, problems


def main(argv):
    if len(argv) < 2:
        raise SystemExit(__doc__)
    name = argv[1]
    profile, problems = draft(name)
    if problems:
        print("undecided payload operands (tools/offsetmap.py <which> <dll> --base BASE --occurrences KEY):")
        for line in problems:
            print(f"  {line}")
    if "--check" in argv:
        tracked = json.loads(builds.build(name).layout.read_text(encoding="utf-8"))
        fields = [f"{w}_{part}" for w in ("logic", "game") for part in ("layout", "symbols", "sha256")] + ["base"]
        differ = [f for f in fields if tracked.get(f) != profile.get(f)]
        print("matches the tracked profile" if not differ else f"differs from the tracked profile in: {', '.join(differ)}")
        return 1 if differ or problems else 0
    if "--write" in argv:
        path = builds.LAYOUTS / f"{name}.json"
        path.write_text(json.dumps(profile, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
        print(f"wrote {builds.relative(path)}" + ("; decide its null entries and rerun" if problems else ""))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
