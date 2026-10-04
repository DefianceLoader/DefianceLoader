"""Check committed injector units and exercise the injector's build checks.

This checker never assembles payloads. Its metadata-only mode is suitable for
public CI, where the game's DLLs are intentionally unavailable.
"""
import argparse
import json
import os
import pathlib
import subprocess
import sys

import builds

ROOT = pathlib.Path(__file__).resolve().parents[1]
VARIANTS = ROOT / "tools" / "variants"
DEFAULT_EXE = ROOT / "target" / "release" / "defiance-pickup-inject.exe"


def fail(message):
    raise ValueError(message)


def _read_descriptor(path):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"{path}: cannot read unit descriptor: {error}")


def check_metadata(build_table=builds, variants_dir=VARIANTS):
    """Validate every committed variant against supported build metadata."""
    supported = build_table.supported()
    modern = {b.name for b in supported if b.layout is not None}
    folders = {p.name for p in variants_dir.iterdir() if p.is_dir() and p.name != "reference"}
    if folders != modern:
        fail(f"unit variant folders differ from supported layouts: "
             f"missing={sorted(modern - folders)}, unsupported={sorted(folders - modern)}")

    reference_dir = variants_dir / "reference" / "units"
    if not reference_dir.is_dir():
        fail(f"missing reference unit directory: {reference_dir}")
    reference = _descriptors(reference_dir)
    reference_shape = {name: (item["module"], item["plugin"]) for name, item in reference.items()}
    ref_logic = reference.get("core-logic")
    ref_game = reference.get("core-game")
    if not ref_logic or not ref_game or ref_logic["module"] != "logic.dll" or ref_game["module"] != "game.dll":
        fail("reference units must include core-logic and core-game anchors")
    reference_hashes = {"logic.dll": ref_logic["source_sha256"], "game.dll": ref_game["source_sha256"]}

    for name in sorted(modern):
        directory = variants_dir / name / "units"
        current = _descriptors(directory)
        shape = {unit: (item["module"], item["plugin"]) for unit, item in current.items()}
        if shape != reference_shape:
            fail(f"{name}: unit names, modules, or plugins differ from reference; "
                 f"missing={sorted(reference_shape.keys() - shape.keys())}, "
                 f"extra={sorted(shape.keys() - reference_shape.keys())}, "
                 f"changed={sorted(k for k in shape.keys() & reference_shape.keys() if shape[k] != reference_shape[k])}")
        layout = json.loads(name_path(build_table.build(name).layout).read_text(encoding="utf-8"))
        expected = {"logic.dll": layout["logic_sha256"], "game.dll": layout["game_sha256"]}
        for unit, descriptor in current.items():
            module = descriptor["module"]
            if descriptor["source_sha256"] != expected[module]:
                fail(f"{name}/{unit}: source_sha256 does not match layout {module} hash")

    for variant_name in ["reference", *sorted(modern)]:
        directory = variants_dir / variant_name / "units"
        descriptors = reference if variant_name == "reference" else _descriptors(directory)
        hashes = reference_hashes
        if variant_name != "reference":
            layout = json.loads(name_path(build_table.build(variant_name).layout).read_text(encoding="utf-8"))
            hashes = {"logic.dll": layout["logic_sha256"], "game.dll": layout["game_sha256"]}
        for unit, descriptor in descriptors.items():
            module = descriptor["module"]
            if module not in hashes:
                fail(f"{variant_name}/{unit}: unexpected module {module!r}")
            if descriptor["source_sha256"] != hashes[module]:
                fail(f"{variant_name}/{unit}: source_sha256 disagrees with {module} build hash")
            binary = directory / f"{unit}.bin"
            try:
                actual_size = binary.stat().st_size
            except OSError as error:
                fail(f"{binary}: missing unit bytes: {error}")
            if actual_size != descriptor.get("unit_bytes"):
                fail(f"{binary}: has {actual_size} bytes; descriptor says {descriptor.get('unit_bytes')}")
    return sorted(modern)


def name_path(path):
    return pathlib.Path(path)


def _descriptors(directory):
    if not directory.is_dir():
        fail(f"missing unit directory: {directory}")
    descriptors = {}
    for path in sorted(directory.glob("*.json")):
        item = _read_descriptor(path)
        name = path.stem
        if not isinstance(item, dict):
            fail(f"{path}: unit descriptor must be a JSON object")
        if item.get("name") != name:
            fail(f"{path}: descriptor name {item.get('name')!r} differs from filename")
        if not isinstance(item.get("module"), str) or not isinstance(item.get("plugin"), str):
            fail(f"{path}: descriptor needs string module and plugin fields")
        if not isinstance(item.get("source_sha256"), str):
            fail(f"{path}: descriptor needs a source_sha256 string")
        if not isinstance(item.get("unit_bytes"), int) or item["unit_bytes"] < 0:
            fail(f"{path}: descriptor needs a non-negative unit_bytes integer")
        if name in descriptors:
            fail(f"{directory}: duplicate unit {name}")
        descriptors[name] = item
    return descriptors


def _complete(build):
    return build.logic.is_file() and build.game.is_file()


def _scan(executable, directory):
    print(f"scan-check: {directory}", flush=True)
    subprocess.run([str(executable), "--scan-check", str(directory)], check=True)


def check_full(executable=DEFAULT_EXE, build_table=builds, game_dir=None, variants_dir=VARIANTS):
    """Run injector self-test and scan every locally available supported pair."""
    names = check_metadata(build_table, variants_dir)
    executable = pathlib.Path(executable)
    if not executable.is_file():
        fail(f"injector executable not found: {executable}; build defiance-pickup-inject first")
    subprocess.run([str(executable), "--self-test"], check=True)

    supported = build_table.supported()
    supported_names = {b.name for b in supported}
    for build in supported:
        if _complete(build):
            _scan(executable, build.folder)
        else:
            absent = [str(path) for path in (build.logic, build.game) if not path.is_file()]
            print(f"skip {build.name}: supported DLL copy absent ({', '.join(absent)})")

    unknown = [b for b in build_table.on_disk() if _complete(b) and b.name not in supported_names]
    if unknown:
        fail("complete local DLL builds have no supported profile: " + ", ".join(b.name for b in unknown))

    game_dir = pathlib.Path(game_dir or os.environ.get("DEFIANCE_GAME_DIR", "")) if (game_dir or os.environ.get("DEFIANCE_GAME_DIR")) else None
    if game_dir:
        candidates = [game_dir, game_dir / "bin"]
        install = next((path for path in candidates if (path / "logic.dll").is_file() and (path / "game.dll").is_file()), None)
        if install is None:
            print(f"skip DEFIANCE_GAME_DIR: no logic.dll/game.dll pair in {game_dir} or {game_dir / 'bin'}")
        else:
            _scan(executable, install)
    return names


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metadata-only", action="store_true", help="check committed metadata without an executable")
    parser.add_argument("--exe", type=pathlib.Path, default=DEFAULT_EXE, help="injector executable to exercise")
    args = parser.parse_args(argv)
    try:
        if args.metadata_only:
            names = check_metadata()
            print(f"injector metadata valid for reference and {len(names)} supported layout(s)")
        else:
            names = check_full(args.exe)
            print(f"injector checks passed for reference and {len(names)} supported layout(s)")
        return 0
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"injector check failed: {error}", file=sys.stderr)
        return getattr(error, "returncode", 1) or 1


if __name__ == "__main__":
    raise SystemExit(main())
