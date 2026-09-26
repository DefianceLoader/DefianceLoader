"""Validate each tracked per-build variant against its own DLLs.

A variant descriptor is resolved by `tools/variant.py`, and the loader applies
it without relocation. That make the descriptor's expectations authoritative:
every byte it says it will replace must be exactly what the target DLLs hold,
its `source_sha256` must be the target's hash, and every fixup target must lie
in the module. This re-checks all of that offline, so a stale or mis-resolved
variant is caught before it ships. It also assembles each profile afresh from
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


def spans(descriptor):
    """(rva, length) for every write the loader prepares, all features on."""
    out = [(descriptor["call_site"], len(bytes.fromhex(descriptor["call_before"]))),
           (descriptor["move_call_site"], len(bytes.fromhex(descriptor["move_displaced"])))]
    out += [(h["hook_rva"], len(bytes.fromhex(h["hook_displaced"]))) for h in descriptor["detours"]]
    out += [(c["pose_site"], len(bytes.fromhex(c["pose_before"]))) for c in descriptor["pose_calls"]]
    out += [(descriptor[f"{name}_rva"], len(bytes.fromhex(descriptor[f"{name}_before"])))
            for name in ("select_is", "select_squad", "select_toggle", "select_type")]
    return out


def overlaps(spans):
    """The loader refuses feature plans whose enabled spans overlap; all
    features are enabled here, so none may."""
    ordered = sorted(spans)
    return [(a, b) for (a, la), (b, _lb) in zip(ordered, ordered[1:]) if a + la > b]


def check_logic(descriptor, image):
    problems = []
    problems += filter(None, [expected(image, descriptor["anchor_rva"], bytes.fromhex(descriptor["anchor"]), "anchor"),
                              expected(image, descriptor["call_site"], bytes.fromhex(descriptor["call_before"]), "chooser call"),
                              expected(image, descriptor["move_call_site"], bytes.fromhex(descriptor["move_displaced"]), "move call")])
    for hook in descriptor["detours"]:
        problems += filter(None, [expected(image, hook["hook_rva"], bytes.fromhex(hook["hook_displaced"]), hook.get("hook_entry", "detour"))])
    for call in descriptor["pose_calls"]:
        problems += filter(None, [expected(image, call["pose_site"], bytes.fromhex(call["pose_before"]), "retargeted call")])
    for name in ("select_is", "select_squad", "select_toggle", "select_type"):
        problems += filter(None, [expected(image, descriptor[f"{name}_rva"], bytes.fromhex(descriptor[f"{name}_before"]), name)])
    size = image.pe.OPTIONAL_HEADER.SizeOfImage
    for rva, length in spans(descriptor):
        if rva + length > size:
            problems.append(f"a write at {rva:#x} runs past the image")
    for a, b in overlaps(spans(descriptor)):
        problems.append(f"writes at {a:#x} and {b:#x} overlap")
    # Each edit's replacement rel32 must reach the moved target it names.
    names = {descriptor[f"{n}_rva"]: n for n in ("select_is", "select_squad", "select_toggle", "select_type")}
    for fix in descriptor["edit_fixups"]:
        name = names.get(fix["edit_rva"])
        after = bytes.fromhex(descriptor[f"{name}_after"]) if name else b""
        if not after or fix["edit_offset"] + 4 > len(after):
            problems.append(f"the edit at {fix['edit_rva']:#x} has no replacement field")
            continue
        rel = struct.unpack_from("<i", after, fix["edit_offset"])[0]
        reached = fix["edit_rva"] + fix["edit_offset"] + 4 + rel
        if reached != fix["edit_target"]:
            problems.append(f"the edit at {fix['edit_rva']:#x} reaches {reached:#x}, not {fix['edit_target']:#x}")
    return problems


def check_game(descriptor, image):
    problems = []
    problems += filter(None, [expected(image, descriptor["anchor_rva"], bytes.fromhex(descriptor["anchor"]), "anchor")])
    for hook in descriptor["hooks"]:
        problems += filter(None, [expected(image, hook["rva"], bytes.fromhex(hook["displaced"]), "hook")])
    size = image.pe.OPTIONAL_HEADER.SizeOfImage
    for fix in descriptor["fixups"]:
        if not 0 <= fix["target_rva"] < size:
            problems.append(f"fixup target {fix['target_rva']:#x} is outside the image")
    hooks = [(h["rva"], len(bytes.fromhex(h["displaced"]))) for h in descriptor["hooks"]]
    for rva, length in hooks:
        if rva + length > size:
            problems.append(f"a hook at {rva:#x} runs past the image")
    for a, b in overlaps(hooks):
        problems.append(f"hooks at {a:#x} and {b:#x} overlap")
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
    for tool in ("tools/payload.py", "tools/icon.py"):
        run = subprocess.run([sys.executable, tool, "--layout", name], capture_output=True, text=True)
        if run.returncode:
            return f"{tool} --layout {name} failed:\n{run.stdout}{run.stderr}"
    return None


guards()
for profile_path in sorted(pathlib.Path("tools/layouts").glob("*.json")):
    profile = json.loads(profile_path.read_text(encoding="utf-8"))
    name = profile["name"]
    staged = pathlib.Path("tools/variants") / name
    if not (staged / "logic.json").exists():
        print(f"skip {name}: no tracked variant")
        continue
    logic_dll, game_dll = by_sha(profile["logic_sha256"]), by_sha(profile["game_sha256"])
    if logic_dll is None or game_dll is None:
        print(f"skip {name}: its DLLs are not under bin/")
        continue
    print(f"== {name}  ({logic_dll}, {game_dll})")
    logic = json.loads((staged / "logic.json").read_text(encoding="utf-8"))
    game = json.loads((staged / "game.json").read_text(encoding="utf-8"))
    check(f"{name}: logic descriptor names {logic_dll.name}", logic["source_sha256"] == profile["logic_sha256"])
    check(f"{name}: game descriptor names {game_dll.name}", game["source_sha256"] == profile["game_sha256"])
    # The loader ties a native entry to its patch by `site.start == hook.rva`,
    # so those four sites must have moved with their detours.
    for native in ("firing_set", "firing_ui", "setter", "is_selected"):
        site = next((s for s in logic["sites"] if s["site_name"] == native), None)
        hook = None if site is None else next((h for h in logic["detours"] if h["hook_rva"] == site["site_start"]), None)
        check(f"{name}: {native} site matches its detour", site is not None and hook is not None)
    for problem in check_logic(logic, Image(str(logic_dll))):
        check(f"{name}: {problem}", False)
    for problem in check_game(game, Image(str(game_dll))):
        check(f"{name}: {problem}", False)
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
        fresh = variant.resolve_profile(profile, str(logic_dll), str(game_dll))
    except SystemExit as error:
        check(f"{name}: resolves afresh ({error})", False)
        continue
    for which, payload, descriptor in fresh:
        tracked = json.loads((staged / f"{which}.json").read_text(encoding="utf-8"))
        check(f"{name}: tracked {which}.bin is the current source's", (staged / f"{which}.bin").read_bytes() == payload)
        check(f"{name}: tracked {which}.json is the current source's", tracked == descriptor)
    if failures == failed_before:
        stamp.record(f"variant-{name}", sync_key)

print(f"\n{failures} failed" if failures else "\nall variant expectations hold")
sys.exit(1 if failures else 0)
