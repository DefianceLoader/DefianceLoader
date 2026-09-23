"""Skip work whose inputs have not changed since it last succeeded.

A stamp in out/stamps/ records a digest of a job's inputs and of the outputs it
produced. A job reruns when any input changed, any output is missing or was
changed since, or DEFIANCE_NO_STAMP is set. Leaving the outputs untouched when
nothing changed matters beyond the time saved: cargo rebuilds everything that
embeds out/payload* whenever those files are rewritten, even byte-identical.

    python tools/stamp.py assemble      the file patch and both payloads

`digest` and the stamp helpers are also used by tools/test_variant.py.
"""
import hashlib
import json
import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
STAMPS = ROOT / "out" / "stamps"
# Everything the assembly tooling reads: its own modules (the three scripts
# and what they import, including variant.py for tools/test_variant.py), the
# patch sources, the per-build layouts and the reference DLLs. Keep the module
# list in step with their imports; a tool outside it can change freely.
ASSEMBLY_INPUTS = ["tools/build.py", "tools/payload.py", "tools/icon.py", "tools/pe.py",
                   "tools/sigs.py", "tools/variant.py", "tools/stamp.py",
                   "patch/**/*.asm", "tools/layouts/*.json",
                   "bin/logic.orig.dll", "bin/game.orig.dll"]
JOBS = {
    "assemble": {
        "inputs": ASSEMBLY_INPUTS,
        "outputs": ["out/logic.dll", "out/manifest.json", "out/payload.bin", "out/payload.json",
                    "out/payload-game.bin", "out/payload-game.json"],
        "commands": [["python", "tools/build.py"], ["python", "tools/payload.py"],
                     ["python", "tools/icon.py"]],
    },
}


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
        hasher.update(hashlib.sha256(path.read_bytes()).digest())
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


def run(name):
    job = JOBS[name]
    key = digest(job["inputs"])
    if fresh(name, key):
        print(f"{name}: inputs unchanged since the last run; skipped (DEFIANCE_NO_STAMP=1 forces it)")
        return 0
    for command in job["commands"]:
        result = subprocess.run([sys.executable if command[0] == "python" else command[0], *command[1:]], cwd=ROOT)
        if result.returncode:
            return result.returncode
    record(name, key, job["outputs"])
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1] not in JOBS:
        raise SystemExit(__doc__)
    sys.exit(run(sys.argv[1]))
