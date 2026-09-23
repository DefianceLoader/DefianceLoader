"""Load real feature DLLs against stock module copies, never a running game.

Build first. Set CARGO_TARGET_DIR to use an alternate Cargo output directory.
Each scenario runs in a separate process with fresh mapped modules.
"""
import os
import pathlib
import shutil
import subprocess
import tempfile
import argparse

ROOT = pathlib.Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust-pickup", action="store_true", help="test out/pickup-rust release DLLs")
    parser.add_argument("--assembly-pickup", action="store_true", help="expect a pickup DLL built with --no-default-features")
    args = parser.parse_args()
    target = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if args.rust_pickup:
        target = ROOT / "out/pickup-rust"
    environment = os.environ.copy()
    if not args.assembly_pickup:
        environment["DEFIANCE_TEST_RUST_PICKUP"] = "1"
    else:
        environment.pop("DEFIANCE_TEST_RUST_PICKUP", None)
    source = (target / "release").resolve()
    host = source / "defiance-plugin-host-test.exe"
    if not host.is_file():
        raise SystemExit(f"{host} missing; run mise run build first")
    builds = [("original", ROOT / "bin/logic.orig.dll", ROOT / "bin/game.orig.dll")]
    builds += [(store, ROOT / "bin" / store / "logic.dll", ROOT / "bin" / store / "game.dll")
               for store in ("gog", "steam") if (ROOT / "bin" / store / "logic.dll").is_file()]
    with tempfile.TemporaryDirectory(prefix="defiance-plugin-tests-") as temporary:
        folder = pathlib.Path(temporary)
        shutil.copy2(host, folder / host.name)
        for name, logic, game in builds:
            scenarios = ("all", "core-only", "without-ammo", "fail-ammo", "without-attack", "without-garrison", "fail-attack", "fail-garrison", "disabled-ammo-corrupt", "enabled-ammo-corrupt", "shared-helper-corrupt", "without-selection", "without-posture", "without-movement", "without-firing", "without-pickup", "without-diagnostics", "without-preview-weapon", "diagnostics-only", "without-posture-and-ammo", "without-movement-and-ammo", "without-core", "unknown-build")
            if not args.assembly_pickup:
                scenarios += ("rust-fail-pickup", "rust-changed-pickup", "rust-disabled-pickup")
            scenarios += ("fail-firing", "fail-selection")
            for scenario in scenarios:
                # unknown-build deliberately changes the on-disk module hash.
                shutil.copy2(logic, folder / "logic.dll")
                shutil.copy2(game, folder / "game.dll")
                print(f"== {name}: {scenario}", flush=True)
                subprocess.run([str(folder / host.name), str(ROOT), str(source), scenario], check=True, env=environment)


if __name__ == "__main__":
    main()
