"""Load real feature DLLs against stock module copies, never a running game.

Build first. Set CARGO_TARGET_DIR to use an alternate Cargo output directory.
Each scenario runs in a separate process with fresh mapped modules; the
processes run concurrently and each one's output is printed whole.
"""
import concurrent.futures
import os
import builds
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
    unit_inspection = source
    if not (unit_inspection / "defiance_plugin_unit_inspection.dll").is_file():
        unit_inspection = ROOT / "plugins/unit-inspection/target/release"
    reference = builds.reference().require()
    targets = [(reference, reference.logic, reference.game)]
    targets.extend((b, b.logic, b.game) for b in builds.supported()
                   if b.name != builds.REFERENCE and b.present)
    jobs = []
    with tempfile.TemporaryDirectory(prefix="defiance-plugin-tests-") as temporary:
        for index, (build, logic, game) in enumerate(targets):
            name = build.name
            # The host applies resolved unit descriptors against each stock
            # build. Layout outputs contain that build's resolved RVAs; release
            # copies use the reference descriptors because their addresses are
            # unchanged. Each build gets its own repo folder, since its
            # scenarios run alongside other builds'.
            resolved_units = (ROOT / "tools" / "variants" / name / "units" if build.layout
                              else ROOT / "tools/variants/reference/units")
            if not resolved_units.is_dir():
                raise SystemExit(
                    f"resolved units for {name} are missing at {resolved_units}; "
                    f"run mise run variants first"
                )
            repo = pathlib.Path(temporary) / f"{index}-repo"
            host_units = repo / "tools/variants/reference/units"
            host_units.mkdir(parents=True)
            for path in resolved_units.iterdir():
                if path.suffix in (".json", ".bin"):
                    shutil.copy2(path, host_units / path.name)
            scenarios = ("all", "core-only", "without-ammo", "fail-ammo", "without-attack", "without-garrison", "fail-attack", "fail-garrison", "disabled-ammo-corrupt", "enabled-ammo-corrupt", "shared-helper-corrupt", "without-selection", "without-posture", "without-movement", "without-firing", "without-pickup", "without-diagnostics", "without-preview-weapon", "without-vehicle-special-fire", "without-performance", "diagnostics-only", "without-posture-and-ammo", "without-movement-and-ammo", "without-core", "unknown-build")
            if not args.assembly_pickup:
                scenarios += ("rust-fail-pickup", "rust-changed-pickup")
            scenarios += ("fail-firing", "fail-selection")
            if not args.rust_pickup and build.name == targets[0][0].name:
                if not (unit_inspection / "defiance_plugin_unit_inspection.dll").is_file():
                    raise SystemExit("unit-inspection DLL missing; run mise run loader first")
                scenarios += ("unit-inspection-partial-install",)
            # The host reads the modules beside its exe; only unknown-build
            # changes them (it appends to logic.dll), so it gets its own copy
            # and every other scenario of this build shares one folder.
            shared = module_folder(pathlib.Path(temporary) / f"{index}", host, logic, game)
            altered = module_folder(pathlib.Path(temporary) / f"{index}-unknown-build", host, logic, game)
            for scenario in scenarios:
                folder = altered if scenario == "unknown-build" else shared
                plugin_dir = unit_inspection if scenario == "unit-inspection-partial-install" else source
                jobs.append((name, scenario, [str(folder / host.name), str(repo), str(plugin_dir), scenario]))
        # Each scenario is its own process with fresh mapped modules, so they run at once.
        failed = []
        with concurrent.futures.ThreadPoolExecutor(max_workers=os.cpu_count() or 4) as pool:
            results = pool.map(lambda job: subprocess.run(job[2], env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                                          text=True, encoding="utf-8", errors="replace"), jobs)
            for (name, scenario, _), result in zip(jobs, results):
                print(f"== {name}: {scenario}", flush=True)
                print(result.stdout, end="", flush=True)
                if result.returncode:
                    failed.append(f"{name}: {scenario} ({result.returncode})")
    if failed:
        raise SystemExit("failed:\n  " + "\n  ".join(failed))


def module_folder(folder, host, logic, game):
    """A folder holding the host and one build's modules under the names it loads."""
    folder.mkdir()
    shutil.copy2(host, folder / host.name)
    shutil.copy2(logic, folder / "logic.dll")
    shutil.copy2(game, folder / "game.dll")
    return folder


if __name__ == "__main__":
    main()
