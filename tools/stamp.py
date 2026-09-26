"""Skip work whose inputs have not changed since it last succeeded.

A stamp in out/stamps/ records a digest of a job's inputs and of the outputs it
produced. A job reruns when any input changed, any output is missing or was
changed since, or DEFIANCE_NO_STAMP is set. Leaving the outputs untouched when
nothing changed matters beyond the time saved: cargo rebuilds everything that
embeds out/payload* whenever those files are rewritten, even byte-identical.

    python tools/stamp.py assemble      the file patch and both payloads
    python tools/stamp.py test tools/test_orders.py [args...]
                                        a test, skipped if it passed on these inputs

After assembling, the reference build's payloads (out/payload[-game].{bin,json})
are copied into the tracked tools/variants/reference/, which is what Core, the
injector and the test host embed: a checkout builds without the game's DLLs,
and a patch change shows up as a change to those files. Without the reference
DLLs (`builds.reference()`, bin/gog/2025-12-23/) assembling is skipped and the
committed payloads are used as they are.

A test's inputs ([`test_inputs`]) are the script and the tools/ modules it
imports (followed through their imports), the arguments that name files or
folders, everything the native tests load that the build or the game provides
(`TEST_ARTIFACTS`: the assembled and committed payloads, the release DLLs and
executables, the game DLLs under bin/), the Python and test packages' versions,
and DEFIANCE_GAME_DIR. A test that reads something outside those must not be
run through here: it would pass from the cache after that input changed. Only
passes are recorded, and a skipped test says so.

`digest` and the stamp helpers are also used by tools/test_variant.py.
"""
import hashlib
import builds
import json
import os
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
STAMPS = ROOT / "out" / "stamps"
# Everything the assembly tooling reads: its own modules (the three scripts
# and what they import, including variant.py for tools/test_variant.py), the
# patch sources, the per-build layouts and the reference DLLs. Keep the module
# list in step with their imports; a tool outside it can change freely.
ASSEMBLY_INPUTS = ["tools/build.py", "tools/payload.py", "tools/icon.py", "tools/pe.py",
                   "tools/sigs.py", "tools/variant.py", "tools/stamp.py", "tools/builds.py",
                   "patch/**/*.asm", "tools/layouts/*.json",
                   builds.relative(builds.reference().logic), builds.relative(builds.reference().game)]
JOBS = {
    "assemble": {
        "inputs": ASSEMBLY_INPUTS,
        "outputs": ["out/logic.dll", "out/manifest.json", "out/payload.bin", "out/payload.json",
                    "out/payload-game.bin", "out/payload-game.json"],
        "commands": [["python", "tools/build.py"], ["python", "tools/payload.py"],
                     ["python", "tools/icon.py"]],
    },
}


# What the cached tests load besides their own code: the assembled and
# committed payloads, the workspace's and the standalone plugins' release
# builds, the patch sources and layouts, and every game DLL in bin/.
TEST_ARTIFACTS = ["out/logic.dll", "out/manifest.json", "out/payload*.bin", "out/payload*.json",
                  "tools/variants/**/*.bin", "tools/variants/**/*.json",
                  "target/release/*.dll", "target/release/*.exe",
                  "plugins/*/target/release/*.dll", "out/pickup-rust/**/*.dll",
                  "patch/**/*.asm", "tools/layouts/*.json", "bin/**/*.dll"]
# Python packages the tests use; a new version reruns them.
TEST_PACKAGES = ["keystone-engine", "capstone", "pefile"]

# File digests by (size, modification time), so the large DLLs are hashed once
# until they change, not once per test.
FILE_CACHE = STAMPS / "files.json"
_file_cache = None


def file_digest(path):
    global _file_cache
    if _file_cache is None:
        try:
            _file_cache = json.loads(FILE_CACHE.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            _file_cache = {}
    stat = path.stat()
    key = path.relative_to(ROOT).as_posix()
    mark = f"{stat.st_size}:{stat.st_mtime_ns}"
    entry = _file_cache.get(key)
    if entry and entry[0] == mark:
        return entry[1]
    value = hashlib.sha256(path.read_bytes()).hexdigest()
    _file_cache[key] = [mark, value]
    return value


def save_file_cache():
    if _file_cache is not None:
        STAMPS.mkdir(parents=True, exist_ok=True)
        FILE_CACHE.write_text(json.dumps(_file_cache, sort_keys=True) + "\n", encoding="utf-8")


def files(patterns):
    found = set()
    for pattern in patterns:
        found.update(p for p in ROOT.glob(pattern) if p.is_file() and "__pycache__" not in p.parts)
    return sorted(found)


def digest(patterns, extra=()):
    """One hash over the paths and contents of every matching file (plus any
    extra strings), so an added, removed or edited input changes it."""
    hasher = hashlib.sha256()
    for path in files(patterns):
        hasher.update(path.relative_to(ROOT).as_posix().encode() + b"\0")
        hasher.update(bytes.fromhex(file_digest(path)))
    for text in extra:
        hasher.update(text.encode() + b"\0")
    return hasher.hexdigest()


def fresh(name, key):
    """Whether the stamp `name` holds `key` and its recorded outputs are intact."""
    if os.environ.get("DEFIANCE_NO_STAMP"):
        return False
    path = STAMPS / f"{name}.json"
    try:
        stamp = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    if stamp.get("key") != key:
        return False
    return all((ROOT / out).is_file() and hashlib.sha256((ROOT / out).read_bytes()).hexdigest() == sha
               for out, sha in stamp.get("outputs", {}).items())


def record(name, key, outputs=()):
    STAMPS.mkdir(parents=True, exist_ok=True)
    stamp = {"key": key, "outputs": {path.relative_to(ROOT).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
                                     for path in files(outputs)}}
    (STAMPS / f"{name}.json").write_text(json.dumps(stamp, indent=2) + "\n", encoding="utf-8")


# The reference payloads as assembled, and their tracked copies.
REFERENCE = ROOT / "tools" / "variants" / "reference"
REFERENCE_COPIES = {"out/payload.bin": "logic.bin", "out/payload.json": "logic.json",
                    "out/payload-game.bin": "game.bin", "out/payload-game.json": "game.json"}
REFERENCE_DLLS = [builds.relative(builds.reference().logic), builds.relative(builds.reference().game)]


def sync_reference():
    """Copy the assembled reference payloads into tools/variants/reference,
    rewriting only what changed, so cargo does not rebuild for identical bytes."""
    REFERENCE.mkdir(parents=True, exist_ok=True)
    for source, name in REFERENCE_COPIES.items():
        data = (ROOT / source).read_bytes()
        target = REFERENCE / name
        if not target.is_file() or target.read_bytes() != data:
            target.write_bytes(data)
            print(f"updated {target.relative_to(ROOT).as_posix()}")


FROM_IMPORT = re.compile(r"^[ \t]*from[ \t]+(\w+)[ \t]+import\b", re.M)
PLAIN_IMPORT = re.compile(r"^[ \t]*import[ \t]+([^\n#]+)", re.M)


def imported_names(source):
    """The top-level module names `source` imports (`import a, b as c`, `from d import e`)."""
    names = FROM_IMPORT.findall(source)
    for group in PLAIN_IMPORT.findall(source):
        names += [part.strip().split()[0].split(".")[0] for part in group.split(",") if part.strip()]
    return names


MENTIONED_TOOL = re.compile(r"tools[/\\\\](\w+)\.py")


def imported_tools(script):
    """`script` and every tools/ module it imports or names (a script it runs as
    `tools/x.py`), followed through theirs."""
    seen, todo = set(), [script]
    while todo:
        path = todo.pop()
        if path in seen or not path.is_file():
            continue
        seen.add(path)
        source = path.read_text(encoding="utf-8", errors="replace")
        for name in imported_names(source) + MENTIONED_TOOL.findall(source):
            module = ROOT / "tools" / f"{name}.py"
            if module.is_file():
                todo.append(module)
    return sorted(seen)


def test_inputs(script, args):
    """The patterns and strings whose digest decides whether a test reruns."""
    patterns = [p.relative_to(ROOT).as_posix() for p in imported_tools(ROOT / script)]
    for arg in args:
        path = ROOT / arg
        if path.is_file():
            patterns.append(pathlib.Path(arg).as_posix())
        elif path.is_dir():
            patterns.append(pathlib.Path(arg).as_posix() + "/**/*")
    from importlib import metadata
    versions = []
    for package in TEST_PACKAGES:
        try:
            versions.append(f"{package}=={metadata.version(package)}")
        except metadata.PackageNotFoundError:
            versions.append(f"{package} absent")
    extra = [script, *args, sys.version, *versions,
             *(f"{var}={os.environ.get(var, '')}" for var in ("DEFIANCE_GAME_DIR", "CARGO_TARGET_DIR"))]
    return patterns + TEST_ARTIFACTS, extra


# A test remembers the inputs of its last few passes, so going back to an
# earlier state (another branch) finds it again.
TEST_KEYS_KEPT = 8


def passed_before(name, key):
    if os.environ.get("DEFIANCE_NO_STAMP"):
        return False
    try:
        return key in json.loads((STAMPS / f"{name}.json").read_text(encoding="utf-8")).get("passes", [])
    except (OSError, ValueError):
        return False


def record_pass(name, key):
    path = STAMPS / f"{name}.json"
    try:
        passes = json.loads(path.read_text(encoding="utf-8")).get("passes", [])
    except (OSError, ValueError):
        passes = []
    passes = [key] + [k for k in passes if k != key][:TEST_KEYS_KEPT - 1]
    STAMPS.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"passes": passes}, indent=2) + "\n", encoding="utf-8")


def run_test(script, args):
    patterns, extra = test_inputs(script, args)
    key = digest(patterns, extra)
    save_file_cache()
    name = "test-" + pathlib.Path(script).stem + ("-" + hashlib.sha256("\0".join(args).encode()).hexdigest()[:8] if args else "")
    if passed_before(name, key):
        print(f"{pathlib.Path(script).name}: passed on these inputs before; skipped (DEFIANCE_NO_STAMP=1 runs it)")
        return 0
    result = subprocess.run([sys.executable, script, *args], cwd=ROOT)
    if result.returncode == 0:
        record_pass(name, key)
    return result.returncode


def run(name):
    job = JOBS[name]
    if name == "assemble" and not all((ROOT / dll).is_file() for dll in REFERENCE_DLLS):
        missing = [dll for dll in REFERENCE_DLLS if not (ROOT / dll).is_file()]
        if all((REFERENCE / n).is_file() for n in REFERENCE_COPIES.values()):
            print(f"assemble: {', '.join(missing)} not present; using the committed payloads in "
                  "tools/variants/reference")
            return 0
        print(f"assemble: {', '.join(missing)} not present and no committed payloads", file=sys.stderr)
        return 1
    key = digest(job["inputs"])
    if fresh(name, key):
        print(f"{name}: inputs unchanged since the last run; skipped (DEFIANCE_NO_STAMP=1 forces it)")
    else:
        for command in job["commands"]:
            result = subprocess.run([sys.executable if command[0] == "python" else command[0], *command[1:]], cwd=ROOT)
            if result.returncode:
                return result.returncode
        record(name, key, job["outputs"])
    if name == "assemble":
        sync_reference()
    return 0


if __name__ == "__main__":
    if len(sys.argv) >= 3 and sys.argv[1] == "test":
        sys.exit(run_test(sys.argv[2], sys.argv[3:]))
    if len(sys.argv) != 2 or sys.argv[1] not in JOBS:
        raise SystemExit(__doc__)
    code = run(sys.argv[1])
    save_file_cache()
    sys.exit(code)
