"""Validate each tracked build's units against its own DLLs.

A variant's units are resolved by `tools/variant.py`, and Core applies them
without relocation. That makes their expectations authoritative: every byte a
unit says it will replace must be exactly what the target DLLs hold, its
`source_sha256` must be the target's hash, and every fixup target must lie in
the module. This re-checks all of that offline (and the reference build's units
against the reference DLLs), so a stale or mis-resolved variant is caught
before it ships. It also assembles each profile afresh from
`patch/` and resolves it again: the tracked variant must be exactly that, so an
edit to the shared source that was not carried into the variants fails here
rather than shipping a stale per-build payload. It skips (without failing) a
profile whose DLLs are not present, like the other tests that gate on
`bin/<store>`; the resolver's own guards are checked without any DLL.

    python tools/test_variant.py
"""
import hashlib, json, pathlib, struct, subprocess, sys
import builds
sys.path.insert(0, "tools")
import capstone
from pe import Image
import stamp
import variant

failures = 0


def check(label, ok):
    global failures
    print(f"{'PASS' if ok else 'FAIL'}  {label}")
    failures += not ok


def expected(image, rva, want, what):
    got = image.read(rva, len(want))
    if got != want:
        return f"{what}: {rva:#x} holds {got.hex()}, expected {want.hex()}"
    return None


def by_sha(sha):
    """The first file under bin/ whose sha256 is `sha`, or None."""
    for path in sorted(pathlib.Path("bin").rglob("*.dll")):
        if hashlib.sha256(path.read_bytes()).hexdigest() == sha:
            return path
    return None


def overlaps(spans):
    """Writes the loader would refuse because they overlap."""
    ordered = sorted(spans)
    return [(a, b) for (a, la), (b, _lb) in zip(ordered, ordered[1:]) if a + la > b]


def check_units(folder, logic_image, game_image):
    """A build's units (tools/units.py): every byte a write replaces, every
    anchor and every call's stock function as its DLL holds them, every fixup
    and edit aimed inside it, and no two writes of any units overlapping."""
    problems, spans = [], []
    for path in sorted(folder.glob("*.json")):
        unit = json.loads(path.read_text(encoding="utf-8"))
        image = logic_image if unit["module"] == "logic.dll" else game_image
        size = image.pe.OPTIONAL_HEADER.SizeOfImage
        label = path.stem
        for w in unit["writes"]:
            before = bytes.fromhex(w["before"])
            problems += filter(None, [expected(image, w["rva"], before, f"{label} {w.get('label', 'edit')}")])
            spans.append((unit["module"], w["rva"], len(before)))
            if w.get("stock") and w["rva"] + 5 + struct.unpack_from("<i", before, 1)[0] != w["stock"]:
                problems.append(f"{label}: the call at {w['rva']:#x} does not reach {w['stock']:#x}")
            if w["kind"] != "edit" and not 0 <= w["entry"] < unit["unit_bytes"]:
                problems.append(f"{label}: the write at {w['rva']:#x} enters outside the unit")
        for a in unit["anchors"]:
            problems += filter(None, [expected(image, a["rva"], bytes.fromhex(a["bytes"]), f"{label} anchor")])
        edits = {w["rva"]: bytes.fromhex(w["after"]) for w in unit["writes"] if w["kind"] == "edit"}
        for f in unit["fixups"]:
            if f["kind"] in ("abs64", "rel32", "edit") and not 0 <= f["target"] < size:
                problems.append(f"{label}: fixup target {f['target']:#x} is outside the image")
            if f["kind"] == "edit":
                after = edits.get(f["rva"], b"")
                reached = (f["rva"] + f["offset"] + 4 + struct.unpack_from("<i", after, f["offset"])[0]
                           if f["offset"] + 4 <= len(after) else None)
                if reached != f["target"]:
                    problems.append(f"{label}: the edit at {f['rva']:#x} does not reach {f['target']:#x}")
    for module in ("logic.dll", "game.dll"):
        for a, b in overlaps([(rva, n) for m, rva, n in spans if m == module]):
            problems.append(f"{module}: unit writes at {a:#x} and {b:#x} overlap")
    return problems


class FakeModule:
    """Just enough of a target module for the resolver's guards."""

    def __init__(self, code=b"", functions=None):
        self.image = code
        self.md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
        self.md.detail = True
        self.functions = type("Functions", (), {"function_of": staticmethod(functions or (lambda rva: None))})()


class FakeTarget(variant.Target):
    def __init__(self, module):
        self.module = module


def refused(action):
    try:
        action()
    except SystemExit:
        return True
    return False


def guards():
    """The resolver's refusals, without any game DLL."""
    ref = FakeModule()
    shape = lambda before, after: variant.check_shape(ref, 0, bytes.fromhex(before), FakeTarget(FakeModule(bytes.fromhex(after))), 0, "a site")
    check("a moved field keeps an instruction's shape (vtable slot 0x380 -> 0x398)",
          not refused(lambda: shape("ff9080030000", "ff9098030000")))
    check("a moved frame size keeps a prologue's shape", not refused(lambda: shape("40534883ec60", "40534883ec20")))
    # The first 2026-09 profile aimed move_posture_2 at a read into edi, in a
    # function whose movement state is in rbx; move_posture needs ebp and rsi.
    check("a different register is refused (movzx ebp -> movzx edi)",
          refused(lambda: shape("0fb6a99e020000", "0fb6b99e020000")))
    check("a disp8 -> disp32 change is refused (the call would not fit)",
          refused(lambda: shape("488b4828", "488b8828010000")))
    # Two sites of one reference function, found in two target functions.
    descriptor = {"sites": [
        {"site_name": "a", "site_start": 0x100, "site_group": 0x100, "site_offset": 0, "site_pattern": "90"},
        {"site_name": "b", "site_start": 0x140, "site_group": 0x100, "site_offset": 0, "site_pattern": "90"}]}
    split = FakeTarget(FakeModule(functions=lambda rva: (rva & ~0xfff, (rva & ~0xfff) + 0x100)))
    check("sites of one function split across two target functions are refused",
          refused(lambda: variant.Sites(descriptor, ref, split, {"a": 0x1100, "b": 0x2140})))
    together = variant.Sites(descriptor, ref, split, {"a": 0x1100, "b": 0x1148})
    check("sites of one function may move apart inside it, and share its group",
          {s["site_group"] for s in together.relocated()} == {0x1000})


def assembled_afresh(name):
    """Assemble the profile's payloads from the current patch/ into out/."""
    for tool in ("tools/payload.py", "tools/icon.py", "tools/units.py"):
        run = subprocess.run([sys.executable, tool, "--layout", name], capture_output=True, text=True)
        if run.returncode:
            return f"{tool} --layout {name} failed:\n{run.stdout}{run.stderr}"
    return None


guards()
reference = builds.reference()
if reference.present:
    print("== reference units")
    for problem in check_units(pathlib.Path("tools/variants/reference/units"),
                               Image(str(reference.logic)), Image(str(reference.game))):
        check(f"reference: {problem}", False)
for profile_path in sorted(pathlib.Path("tools/layouts").glob("*.json")):
    profile = json.loads(profile_path.read_text(encoding="utf-8"))
    name = profile["name"]
    staged = pathlib.Path("tools/variants") / name
    if not (staged / "units").is_dir():
        print(f"skip {name}: no tracked variant")
        continue
    logic_dll, game_dll = by_sha(profile["logic_sha256"]), by_sha(profile["game_sha256"])
    if logic_dll is None or game_dll is None:
        print(f"skip {name}: its DLLs are not under bin/")
        continue
    print(f"== {name}  ({logic_dll}, {game_dll})")
    for problem in check_units(staged / "units", Image(str(logic_dll)), Image(str(game_dll))):
        check(f"{name}: {problem}", False)
    units = {p.stem: json.loads(p.read_text(encoding="utf-8")) for p in (staged / "units").glob("*.json")}
    for module, sha in (("logic.dll", profile["logic_sha256"]), ("game.dll", profile["game_sha256"])):
        check(f"{name}: every {module} unit names it",
              all(u["source_sha256"] == sha for u in units.values() if u["module"] == module))
    if not builds.reference().present:
        print(f"skip {name} sync: the reference DLLs are not under bin/")
        continue
    # Reassembling takes about a minute per profile; skip it when nothing it
    # reads (the tooling, patch/, the layouts, the tracked variant and every
    # DLL involved) changed since it last passed.
    sync_key = stamp.digest(stamp.ASSEMBLY_INPUTS + [f"tools/variants/{name}/*"],
                            extra=[hashlib.sha256(logic_dll.read_bytes()).hexdigest(),
                                   hashlib.sha256(game_dll.read_bytes()).hexdigest()])
    if stamp.fresh(f"variant-{name}", sync_key):
        print(f"skip {name} sync: unchanged since it last passed (DEFIANCE_NO_STAMP=1 forces it)")
        continue
    failed_before = failures
    problem = assembled_afresh(name)
    check(f"{name}: assembles from the current patch/", problem is None)
    if problem:
        print(problem)
        continue
    try:
        units = variant.resolve_units(profile, str(logic_dll), str(game_dll))
    except SystemExit as error:
        check(f"{name}: units resolve afresh ({error})", False)
        continue
    tracked = sorted(p.stem for p in (staged / "units").glob("*.json"))
    check(f"{name}: the tracked units are the current source's", tracked == [u for u, _, _ in units])
    for unit, blob, descriptor in units:
        path = staged / "units" / f"{unit}.json"
        check(f"{name}: tracked unit {unit} is the current source's",
              path.with_suffix(".bin").is_file() and path.with_suffix(".bin").read_bytes() == blob
              and path.is_file() and json.loads(path.read_text(encoding="utf-8")) == descriptor)
    if failures == failed_before:
        stamp.record(f"variant-{name}", sync_key)

print(f"\n{failures} failed" if failures else "\nall variant expectations hold")
sys.exit(1 if failures else 0)
