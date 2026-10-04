"""Build the moving-actions animation companion data from installed PAKs.

Only derived custom assets are emitted under the mod's private ``assets``
directory. Native action clips remain stock; the plugin samples the generated
composites alongside the native clip rather than replacing engine resources.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tools"))
import compose_infantry_animation as compose  # noqa: E402
import package_squad_scroll as pak  # noqa: E402

MOD_DIR = "defiance_moving_actions"
MOD_NAME = "Defiance moving actions animation assets"
RECIPE = "moving-actions-animation-v1"
ASSET_NAMES = ("stock_throw.anim", "stock_switch.anim", "moving_throw.anim",
               "moving_switch.anim", "moving_throw_back.anim")
SOURCES = {
    "throw": "animations/new/anim/st_throw_grenade_m16.anim",
    "switch": "animations/new/anim/st_change_weapon.anim",
    "run": "animations/new/anim/st_run_m16.anim",
    "back": "animations/new/anim/st_strafe_back_m16.anim",
    "model": "animations/new/skin/usa_soldier_1.model",
}
BACK_PERIOD = 1.0666667222976685


def latest_resources(game: Path) -> tuple[dict[str, bytes], dict[str, dict[str, str]]]:
    """Read latest matching members from basis then patch PAKs, with provenance."""
    archives = [game / "basis.pak"] + sorted(game.glob("patch_*.pak"))
    if not archives[0].is_file():
        raise ValueError("basis.pak not found in the game directory")
    wanted = {value.lower(): key for key, value in SOURCES.items()}
    latest: dict[str, bytes] = {}
    provenance: dict[str, dict[str, str]] = {}
    for archive_path in archives:
        with zipfile.ZipFile(archive_path) as archive:
            matches: dict[str, list[str]] = {key: [] for key in wanted.values()}
            for member in archive.namelist():
                normalized = member.replace("\\", "/").lower().lstrip("/")
                for resource, label in wanted.items():
                    if normalized == resource or normalized.endswith("/" + resource):
                        matches[label].append(member)
            for label, names in matches.items():
                if len(names) > 1:
                    raise ValueError(f"ambiguous {SOURCES[label]} in {archive_path.name}")
                if names:
                    data = pak.read(archive, names[0], archive_path)
                    latest[label] = data
                    provenance[label] = {
                        "archive": archive_path.name,
                        "member": names[0].replace("\\", "/"),
                        "sha256": hashlib.sha256(data).hexdigest(),
                    }
    missing = sorted(set(SOURCES) - set(latest))
    if missing:
        raise ValueError("required resources not found in installed PAKs: " + ", ".join(missing))
    return latest, provenance


def build_assets(resources: dict[str, bytes]) -> tuple[dict[str, bytes], dict[str, object]]:
    """Apply the tracked hierarchy-aware transforms to raw PAK members."""
    required = set(SOURCES)
    if set(resources) != required:
        raise ValueError(f"expected source labels {sorted(required)}")
    with tempfile.TemporaryDirectory(prefix="moving-actions-assets-") as temporary:
        temp = Path(temporary)
        paths = {name: temp / name for name in required}
        for name, data in resources.items():
            paths[name].write_bytes(data)
        action_throw = compose.read_clip(paths["throw"])
        action_switch = compose.read_clip(paths["switch"])
        run = compose.read_clip(paths["run"])
        back = compose.read_clip(paths["back"])
        parents = compose._parent_map(paths["model"])
        back_retimed = compose.retime_clip(back, BACK_PERIOD)
        clips = {
            "stock_throw.anim": action_throw,
            "stock_switch.anim": action_switch,
            "moving_throw.anim": compose.compose_locomotion_legs(action_throw, run, parents),
            "moving_switch.anim": compose.compose_locomotion_legs(action_switch, run, parents),
            "moving_throw_back.anim": compose.compose_locomotion_legs(
                action_throw, back_retimed, parents, preserve_static_descendants=True),
        }
        outputs = {name: compose.encode_clip(clip) for name, clip in clips.items()}
    report: dict[str, object] = {
        "recipe": RECIPE,
        "back_retime": {
            "source_period_seconds": back.duration,
            "target_period_seconds": BACK_PERIOD,
            "phase_offset_seconds": 0.0,
            "policy": "uniformly scale native key timestamps; no asserted contact alignment",
            "static_descendants": "preserve action channels when both action and gait channels are constant",
        },
        "outputs": {name: hashlib.sha256(data).hexdigest() for name, data in outputs.items()},
    }
    return outputs, report


def mod_entries(game: Path) -> tuple[dict[str, bytes], dict[str, object]]:
    """Return game-relative companion files and non-sensitive provenance."""
    sources, source_report = latest_resources(Path(game))
    assets, build_report = build_assets(sources)
    prefix = f"mods/{MOD_DIR}/"
    entries = {
        prefix + "mod.json": json.dumps({
            "name": MOD_NAME,
            "description": "Derived lower-body action composites for the Defiance Loader moving-actions animation plugin. Native action clips remain unchanged.",
            "icon": "basis/mod_icon.dds",
        }, indent=2).encode(),
        prefix + "basis/mod_icon.dds": pak.dds(160, 90, (82, 112, 130, 255)),
    }
    for name, data in assets.items():
        entries[prefix + "assets/" + name] = data
    provenance = {"sources": source_report, **build_report}
    entries[prefix + "sources.json"] = json.dumps(provenance, indent=2).encode()
    return entries, provenance


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--game", type=Path, default=os.environ.get("DEFIANCE_GAME_DIR"))
    parser.add_argument("--out", type=Path, default=ROOT / "out/defiance-moving-actions-animation.zip")
    args = parser.parse_args(argv)
    if args.game is None:
        parser.error("--game is required unless DEFIANCE_GAME_DIR is set")
    entries, provenance = mod_entries(args.game)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(args.out, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, data in entries.items():
            archive.writestr(name, data)
    print(f"{args.out}\nrecipe={provenance['recipe']} sources={len(provenance['sources'])} assets={len(provenance['outputs'])}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
