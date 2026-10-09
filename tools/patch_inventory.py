"""Every plugin's writes into logic.dll and game.dll, per build, and whether
they can all install together whatever order the plugins start in.

The loader refuses a write that overlaps another owner's, but only when the
second plugin installs, in the order it happens to start. This finds such
conflicts before any game runs: it starts the real plugin DLLs through the
loader's own startup (discovery, configuration, plan, `init`) against each
build's stock DLLs, in a test host (tools/plugin-host/src/bin/patch-inventory.rs),
and compares three kinds of run:

- solo: one plugin with only its dependencies, so it finds its sites in
  bytes no other feature has changed;
- forward: every plugin, in the loader's order (dependencies first, ties by ID);
- reverse: every plugin, ties by descending ID.

It fails when two plugins' solo writes overlap, or when a plugin installs
differently (another state or other spans) in either full run than alone,
unless ORDERED allows it and the loader keeps that order. Built-in units,
standalone plugins and Core's own hooks are all covered, as each plugin
really installs them with the default settings and SETTINGS.

    python tools/patch_inventory.py check    # against tools/fixtures/patch-inventory/
    python tools/patch_inventory.py record   # rewrite those

Needs `mise run loader` built and the builds' DLLs in bin/; an absent build
is skipped. The fixtures record each plugin's state and spans per build, so a
feature that stops installing on a build, or moves, shows up as a change.
"""
import concurrent.futures, json, os, pathlib, shutil, subprocess, sys, tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import builds, stage

ROOT = builds.ROOT
FIXTURES = ROOT / "tools/fixtures/patch-inventory"
CORE = "defiance.core"
# The modules copied from a build's folder when it has them: logic.dll and
# game.dll always; the renderer and engine modules, which only Core patches,
# only where they are kept for the build. Without them Core's hooks there are
# neither run nor compared with the fixture.
MODULES = ("logic.dll", "game.dll", "world2.dll", "galileo.dll")


def module_files(build):
    """Return the module payloads present and absent for this build."""
    present = tuple(module for module in MODULES if (build.folder / module).is_file())
    absent = tuple(module for module in MODULES if module not in present)
    return present, absent

# Settings for the runs, beyond each plugin's defaults, so that every write a
# player can turn on is inventoried.
SETTINGS = {
    "defiance.legion-vehicle-hacking": {"enabled": "true"},
    "defiance.moving-actions": {"enabled": "true"},
    "defiance.moving-actions-animation": {"enabled": "true"},
    "defiance.moving-actions-render-sync": {"enabled": "true"},
    "defiance.moving-actions-sync": {"enabled": "true"},
    "defiance.moving-grenades": {"enabled": "true"},
    "defiance.regroup": {"enabled": "true"},
    "defiance.vehicle-special-fire": {"enabled": "true"},
    "defiance.vehicle-arrival": {"enabled": "true", "braking_window_percent": "50"},
    "defiance.weapon-drops": {"enabled": "true"},
}

# (earlier, later): `later` depends on bytes `earlier` checks before it
# installs, so it must start after it, and a run that starts them the other
# way round may install either differently. The loader keeps this order only
# through the IDs' alphabetical order; the forward run proves it still does.
# Empty: a plugin checks bytes it only reads through the loader's `original`
# service, which undoes other plugins' writes, rather than rely on an order.
ORDERED = {}


def plugins():
    """{id: (dll, manifest)} for the built plugins with a manifest."""
    target = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "release"
    pairs = [(dll, dll.with_suffix(".plugin.json")) for dll in sorted(target.glob("defiance_plugin_*.dll"))
             if dll.with_suffix(".plugin.json").is_file()]
    pairs += stage.standalone_pairs()
    out = {json.loads(m.read_text(encoding="utf-8"))["id"]: (dll, m) for dll, m in pairs}
    missing = {CORE, "defiance.unit-inspection", "defiance.expanded-ammo-menu"} - out.keys()
    if missing:
        raise SystemExit(f"{', '.join(sorted(missing))} not built; run mise run loader")
    return out


def closure(found, plugin):
    """`plugin` and everything it depends on, transitively, with Core, which
    the loader requires of every plugin that is not multiplayer-safe."""
    out, todo = set(), [plugin, CORE]
    while todo:
        id = todo.pop()
        if id in out or id not in found:
            continue
        out.add(id)
        manifest = json.loads(found[id][1].read_text(encoding="utf-8"))
        todo += [d["id"] for d in manifest.get("depends", [])]
    return out


def run(build, found, ids, reverse=False, reload=False):
    """Start `ids` against `build`'s stock DLLs: {"states", "order", "spans"}."""
    host = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "release/patch-inventory.exe"
    with tempfile.TemporaryDirectory(prefix="defiance-inventory-") as temporary:
        root = pathlib.Path(temporary).resolve()
        exe = root / "bin" / host.name
        exe.parent.mkdir()
        shutil.copy2(host, exe)
        present, _ = module_files(build)
        for module in present:
            shutil.copy2(build.folder / module, root / "bin" / module)
        folder = root / "DefianceLoader/plugins"
        folder.mkdir(parents=True)
        for id in ids:
            dll, manifest = found[id]
            shutil.copy2(dll, folder / dll.name)
            shutil.copy2(manifest, folder / manifest.name)
        # The loader reads a plugin's settings from its group's file.
        config = root / "DefianceLoader/config"
        config.mkdir()
        for id, values in SETTINGS.items():
            if id in ids:
                group = json.loads(found[id][1].read_text(encoding="utf-8"))["group"]
                with open(config / f"{group}.ini", "a", encoding="utf-8") as file:
                    file.write(f"[{id}]\n" + "".join(f"{k} = {v}\n" for k, v in values.items()))
        arguments = (["reverse"] if reverse else []) + (["reload-vehicle"] if reload else [])
        result = subprocess.run([str(exe)] + arguments,
                                capture_output=True, text=True)
        lines = [line for line in result.stdout.splitlines() if line.startswith("{")]
        if result.returncode != 0 or not lines:
            log = root / "DefianceLoader/logs/defiance-loader.log"
            raise SystemExit(f"{build.name}: the test host failed ({result.returncode}):\n{result.stdout}"
                             f"{result.stderr}{log.read_text(errors='replace') if log.is_file() else ''}")
        out = json.loads(lines[-1])
        log = root / "DefianceLoader/logs/defiance-loader.log"
        if log.is_file():
            out["errors"] = [line for line in log.read_text(errors="replace").splitlines()
                             if "[error]" in line]
        out["states"] = {id: state.split("(")[0] for id, state in out["states"].items()}
        return out


def own(result, plugin):
    """`plugin`'s spans in `result`, as comparable tuples."""
    return sorted((s["module"], s["rva"], s["length"], s["kind"]) for s in result["spans"]
                  if s["plugin"] == plugin)


def inventory(build, found):
    """The solo record of every plugin on `build`, and what is wrong with it."""
    ids = sorted(found)
    with concurrent.futures.ThreadPoolExecutor(max_workers=os.cpu_count()) as pool:
        solos = dict(zip(ids, pool.map(lambda id: run(build, found, closure(found, id)), ids)))
        forward, reverse = pool.map(lambda r: run(build, found, ids, r), [False, True])
    record, problems = compare(ids, solos, forward, reverse)
    return {"build": build.name, "plugins": record}, problems


def compare(ids, solos, forward, reverse, ordered=ORDERED):
    """Each plugin's solo record ({id: {"state", "spans"}}) from `solos` (its
    solo run by ID), and the problems: overlaps between solo spans, and
    plugins the full runs install differently."""
    record = {id: {"state": solos[id]["states"].get(id), "spans": [list(s) for s in own(solos[id], id)]}
              for id in ids}
    problems = [f"{id} writes outside the mapped modules at {s[1]:#x}" for id in ids
                for s in record[id]["spans"] if s[0] == "other"]
    # Overlaps between the spans each plugin writes on its own.
    spans = sorted((tuple(s), id) for id in ids for s in record[id]["spans"])
    for i, ((module, rva, length, kind), id) in enumerate(spans):
        for (other_module, other_rva, other_length, other_kind), other in spans[i + 1:]:
            if other_module != module or other_rva >= rva + length:
                break
            if other != id:
                problems.append(f"{module}.dll {rva:#x}..{rva + length:#x} ({id}, {kind}) overlaps "
                                f"{other_rva:#x}..{other_rva + other_length:#x} ({other}, {other_kind})")
    # Each plugin installs as it does alone, in either order, unless ORDERED
    # allows the difference and the loader keeps that order.
    for name, full in (("forward", forward), ("reverse", reverse)):
        position = {id: i for i, id in enumerate(full["order"])}
        for (earlier, later), reason in ordered.items():
            if name == "forward" and earlier in position and later in position \
                    and position[earlier] > position[later]:
                problems.append(f"the loader starts {later} before {earlier}, but {reason}")
        allowed = {id for pair in ordered for id in pair
                   if all(p in position for p in pair) and position[pair[0]] > position[pair[1]]}
        for id in ids:
            alone = (record[id]["state"], record[id]["spans"])
            together = (full["states"].get(id), [list(s) for s in own(full, id)])
            if together != alone and id not in allowed:
                problems.append(f"{id} installs differently with every plugin ({name} order): "
                                f"{together[0]} with {len(together[1])} spans, alone {alone[0]} "
                                f"with {len(alone[1])}")
    return record, problems


def render(record):
    lines = []
    for id, entry in record["plugins"].items():
        spans = ",\n    ".join(json.dumps(s) for s in entry["spans"])
        lines.append(f'  {json.dumps(id)}: {{"state": {json.dumps(entry["state"])}, "spans": [' +
                     (f"\n    {spans}\n  ]}}" if spans else "]}"))
    return f'{{\n "build": {json.dumps(record["build"])},\n "plugins": {{\n' + ",\n".join(
        " " + line for line in lines) + "\n }\n}\n"


def differences(recorded, record, absent=()):
    """How `record` differs from `recorded`, leaving out the spans recorded in
    `absent` modules, which this run could not write."""
    out = []
    for id in sorted(recorded["plugins"].keys() | record["plugins"].keys()):
        a, b = recorded["plugins"].get(id), record["plugins"].get(id)
        if a is None or b is None:
            out.append(f"{id}: {'not recorded' if a is None else 'no longer built'}")
        elif a["state"] != b["state"]:
            out.append(f"{id}: {a['state']} became {b['state']}")
        else:
            gone = [s for s in a["spans"] if s not in b["spans"] and s[0] not in absent]
            new = [s for s in b["spans"] if s not in a["spans"]]
            out += [f"{id}: no longer writes {s}" for s in gone] + [f"{id}: now writes {s}" for s in new]
    return out


def main(argv):
    if argv[1:] not in (["record"], ["check"]):
        raise SystemExit(__doc__)
    found = plugins()
    failed = []
    for build in builds.supported():
        if not build.present:
            print(f"{build.name}: absent, skipped")
            continue
        record, problems = inventory(build, found)
        path = FIXTURES / f"{build.name}.json"
        if argv[1] == "record":
            FIXTURES.mkdir(parents=True, exist_ok=True)
            path.write_text(render(record), encoding="utf-8", newline="\n")
        elif not path.is_file():
            problems.append(f"no {builds.relative(path)}; run tools/patch_inventory.py record")
        else:
            _, absent = module_files(build)
            recorded = json.loads(path.read_text(encoding="utf-8"))
            problems += differences(recorded, record, [m.removesuffix(".dll") for m in absent])
            if any(s[0] + ".dll" in absent for entry in recorded["plugins"].values()
                   for s in entry["spans"]):
                print(f"{build.name}: no {', '.join(absent)} in {builds.relative(build.folder)}; "
                      "the writes recorded there are not checked")
        active = sum(entry["state"] == "Active" for entry in record["plugins"].values())
        spans = sum(len(entry["spans"]) for entry in record["plugins"].values())
        if problems:
            failed.append(build.name)
            print(f"{build.name}:\n  " + "\n  ".join(problems))
        else:
            print(f"{build.name}: {active} of {len(found)} plugins install alone, {spans} spans, "
                  "no overlaps, the same in either order")
    if failed:
        raise SystemExit(f"patch inventory problems on {', '.join(failed)}")


if __name__ == "__main__":
    main(sys.argv)
