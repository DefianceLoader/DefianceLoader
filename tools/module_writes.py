"""Every byte the built-in features write into logic.dll and game.dll, per build.

The parity reference for moving gameplay out of Core into its plugins:
whatever layout the payload code takes, each feature must keep writing the same
module bytes. A write is recorded as the plugin that owns it, where it is, what
it replaces and what it becomes. A branch into the payload is recorded by the
routine it reaches (its labels), not by an address, so a routine may move within
its block, or into a block of its own, without changing the record.

    python tools/module_writes.py check    # the units against tools/fixtures/module-writes/
    python tools/module_writes.py record   # rewrite those from the units

Both read the tracked units in tools/variants/<build>/units/ (tools/units.py),
so they need no game DLLs. The reference was recorded from the monolithic
payloads before the units existed; a branch's entry lists every label its
routine had there.
"""
import json, pathlib, sys

VARIANTS = pathlib.Path("tools/variants")
FIXTURES = pathlib.Path("tools/fixtures/module-writes")

def builds():
    """The builds with resolved payloads, the reference first."""
    return ["reference"] + sorted(p.name for p in VARIANTS.iterdir()
                                  if p.is_dir() and p.name != "reference")


def unit_writes(build):
    """The writes the units of `build` make, in the same form."""
    out, shas = [], {}
    for path in sorted((VARIANTS / build / "units").glob("*.json")):
        unit = json.loads(path.read_text(encoding="utf-8"))
        module = unit["module"].removesuffix(".dll")
        shas[module] = unit["source_sha256"]
        for w in unit["writes"]:
            write = {"module": module, "plugin": unit["plugin"], "rva": w["rva"], "before": w["before"],
                     "kind": w["kind"]}
            write.update({"after": w["after"]} if w["kind"] == "edit"
                         else {"entry": [w["label"]], "tail": w["tail"]})
            out.append(write)
    if not out:
        raise SystemExit(f"{build} has no units; run tools/units.py (and mise run variants)")
    out.sort(key=lambda w: (w["module"], w["rva"]))
    return {"build": build, "logic_sha256": shas["logic"], "game_sha256": shas["game"], "writes": out}


def differences(reference, record):
    """What `record` writes differently from `reference`: a branch matches when
    its routine is one of the names the reference gives its entry."""
    problems = [f"{key}: {reference[key]} is not {record[key]}"
                for key in ("logic_sha256", "game_sha256") if reference[key] != record[key]]
    want = {(w["module"], w["rva"]): w for w in reference["writes"]}
    have = {(w["module"], w["rva"]): w for w in record["writes"]}
    for key in sorted(want.keys() | have.keys()):
        a, b = want.get(key), have.get(key)
        where = f"{key[0]} {key[1]:#x}"
        if a is None or b is None:
            problems.append(f"{where}: {'only written now' if a is None else 'no longer written'}")
            continue
        plain = lambda w: {k: v for k, v in w.items() if k != "entry"}
        if plain(a) != plain(b) or not set(b.get("entry", [])) <= set(a.get("entry", [])):
            problems.append(f"{where}: {a} became {b}")
    return problems


def render(record):
    lines = [json.dumps(w, separators=(", ", ": ")) for w in record["writes"]]
    head = {k: v for k, v in record.items() if k != "writes"}
    body = json.dumps(head, indent=1)[:-2]
    return body + ',\n "writes": [\n  ' + ",\n  ".join(lines) + "\n ]\n}\n"


def main(argv):
    if argv[1:] not in (["record"], ["check"]):
        raise SystemExit(__doc__)
    failed = []
    for build in builds():
        path = FIXTURES / f"{build}.json"
        if argv[1] == "check":
            problems = differences(json.loads(path.read_text(encoding="utf-8")), unit_writes(build))
            if problems:
                failed.append(build)
                print(f"{build}:\n  " + "\n  ".join(problems))
            else:
                print(f"{build}: the units make the recorded writes")
            continue
        record = unit_writes(build)
        FIXTURES.mkdir(parents=True, exist_ok=True)
        path.write_text(render(record), encoding="utf-8", newline="\n")
        print(f"{build}: {len(record['writes'])} writes -> {path}")
    if failed:
        raise SystemExit(f"module writes differ from the reference for {', '.join(failed)}")


if __name__ == "__main__":
    main(sys.argv)
