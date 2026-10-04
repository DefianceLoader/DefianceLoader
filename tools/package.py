"""Build an installable loader package from a release build directory.

The package is a zip with

    bin/<proxy>.dll              the proxy DLL, to go beside trm.exe
    DefianceLoader/plugins/      the plugin DLLs and their manifests — the
                                 built-ins, plus the standalone regroup,
                                 expanded-ammo-menu, squad-management-scroll,
                                 unit-inspection, ability-groups,
                                 legion-vehicle-hacking, cover-markers,
                                 vehicle-arrival, weapon-drops and moving actions
    mods/defiance_squad_scroll/  the scrolling companion UI mod, and
    mods/defiance_unit_inspection/  the unit-inspection reload bar mod, when
                                 the game directory is available to derive them
    mods/defiance_moving_actions/  the generated action-animation sampler data

The ZIP also includes README.md: the player guide (INSTALL.md), the only
document shipped; the developer and plugin docs stay in the repository. Regroup,
moving-actions and its companions, legion-vehicle-hacking, vehicle-arrival,
weapon-drops and vehicle-special-fire are included disabled by default;
expanded-ammo-menu, squad-management-scroll and cover-markers are enabled by
default, and squad-management-scroll needs its companion UI mod, which is derived from the installed game's
paks. Extract to a
temporary directory and follow README.md; preserve existing configuration files
when copying an upgrade into the game. The loader creates configuration on first
launch; the archive contains no INI files.

    python tools/package.py --game "C:\\Games\\...\\Terminator Dark Fate - Defiance"
    python tools/package.py --source target/release --out out/defiance-loader.zip
    python tools/package.py --startup exe --out out/defiance-loader-exe.zip
    DEFIANCE_GAME_DIR=".../bin" python tools/package.py     # parent is the root

Without a game directory the squad-management-scroll plugin is still packaged,
but without its companion UI mod; the plugin then logs a warning and leaves the
stock panel in place until the mod is added. Unit inspection likewise leaves
the reload bars uncoloured without its mod. The moving-actions animation plugin
refuses to install its hooks until its generated companion data is present.

The companion mods alone (data files derived from installed UI and animations;
no code), for releases whose loader package is built without the game:

    python tools/package.py --companion-only --game "C:\\Games\\...\\Defiance"
"""
import argparse
import hashlib
import json
import os
import pathlib
import sys
import zipfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import stage  # noqa: E402  (the proxy name and plugin globs live there)

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_SOURCE = ROOT / "target" / "release"
REGROUP_DLL = "defiance_plugin_regroup.dll"
EXPANDED_AMMO_DLL = "defiance_plugin_expanded_ammo_menu.dll"
SQUAD_SCROLL_DLL = "defiance_plugin_squad_management_scroll.dll"
UNIT_INSPECTION_DLL = "defiance_plugin_unit_inspection.dll"
ABILITY_GROUPS_DLL = "defiance_plugin_ability_groups.dll"
LEGION_VEHICLE_HACKING_DLL = "defiance_plugin_legion_vehicle_hacking.dll"
VEHICLE_ARRIVAL_DLL = "defiance_plugin_vehicle_arrival.dll"
COVER_MARKERS_DLL = "defiance_plugin_cover_markers.dll"
WEAPON_DROPS_DLL = "defiance_plugin_weapon_drops.dll"
EXCLUDED = {"defiance_plugin_pickup.dll", "defiance_plugin_example.dll", REGROUP_DLL,
            *(stem + ".dll" for _, stem in stage.MOVING_ACTIONS)}
# A plugin whose DLL name or ID contains one of these is a private test aid
# (cheats, test-only probes) and is never packaged.
PRIVATE_WORDS = ("testing", "god_mode", "god-mode", "godmode", "cheat")
# Built-in manifests whose `enabled` default the package enforces; it must
# agree with `OFF_BY_DEFAULT` in crates/loader/src/config/builtin.rs.
BUILTIN_DEFAULTS = {"defiance.vehicle-special-fire": "false"}


def game_root(path):
    """Accept the game directory or its `bin`, and require the PAK root."""
    if path is None:
        return None
    path = pathlib.Path(path)
    if (path / "basis.pak").is_file():
        return path
    if path.name.lower() == "bin" and (path.parent / "basis.pak").is_file():
        return path.parent
    return None


def companion_mods(game):
    """Every companion mod's entries, merged; ValueError names the failing mod."""
    entries = {}
    for directory, _, build in stage.COMPANION_MODS:
        try:
            entries.update(build(game)[0])
        except ValueError as error:
            raise ValueError(f"{directory}: {error}") from error
    return entries


def exe_startup_readme(readme):
    """Keep the player guide accurate for the injector-launched package."""
    replacements = (
        ("1. Extract this zip into the game folder (the one containing `bin`), so that\n"
         "   `bin/dxgi.dll` sits beside `bin/trm.exe` and `DefianceLoader` sits beside\n"
         "   `bin`.",
         "1. Extract this zip into the game folder (the one containing `bin`). It\n"
         "   places `defiance-pickup-inject.exe`, `defiance_loader.dll`, and the\n"
         "   injector settings file in `bin`, with `DefianceLoader` beside `bin`."),
        ("3. Start the game as usual.",
         "3. Start `bin/defiance-pickup-inject.exe`, then start the game as usual.\n"
         "   The injector loads the full plugin loader from `bin/defiance_loader.dll`."),
        ("- If the game will not start, remove `bin/dxgi.dll` to play without the loader,\n"
         "  and report the problem with the log.",
         "- To play without the loader, start the game normally instead of through\n"
         "  `bin/defiance-pickup-inject.exe`."),
        ("## Install\n",
         "## Install\n\n"
         "When switching from a proxy install, remove its system-named loader DLL\n"
         "from `bin` first; retain `DefianceLoader` and its configuration. If you\n"
         "keep an older injector INI, set `mode = loader` in it.\n"),
        ("Delete `bin/dxgi.dll`, `bin/defiance-crash-helper.exe` and\n"
         "`bin/defiance-loader.ini`.",
         "Delete `bin/defiance-pickup-inject.exe`, `bin/defiance-pickup-inject.ini`,\n"
         "`bin/defiance_loader.dll`, and `bin/defiance-crash-helper.exe`.")
    )
    for old, new in replacements:
        if old not in readme:
            raise ValueError("INSTALL.md no longer has the expected proxy startup instructions")
        readme = readme.replace(old, new, 1)
    return readme


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--source", default=str(DEFAULT_SOURCE))
    parser.add_argument("--out", default=None)
    parser.add_argument("--startup", choices=("proxy", "exe"), default="proxy",
                        help="package the default proxy DLL or the injector EXE loader fallback")
    parser.add_argument("--regroup-dll", type=pathlib.Path,
                        default=ROOT / "plugins/regroup/target/release" / REGROUP_DLL)
    parser.add_argument("--expanded-ammo-dll", type=pathlib.Path,
                        default=ROOT / "plugins/expanded-ammo-menu/target/release" / EXPANDED_AMMO_DLL)
    parser.add_argument("--squad-scroll-dll", type=pathlib.Path,
                        default=ROOT / "plugins/squad-management-scroll/target/release" / SQUAD_SCROLL_DLL)
    parser.add_argument("--unit-inspection-dll", type=pathlib.Path,
                        default=ROOT / "plugins/unit-inspection/target/release" / UNIT_INSPECTION_DLL)
    parser.add_argument("--ability-groups-dll", type=pathlib.Path,
                        default=ROOT / "plugins/ability-groups/target/release" / ABILITY_GROUPS_DLL)
    parser.add_argument("--legion-vehicle-hacking-dll", type=pathlib.Path,
                        default=ROOT / "plugins/legion-vehicle-hacking/target/release" / LEGION_VEHICLE_HACKING_DLL)
    parser.add_argument("--vehicle-arrival-dll", type=pathlib.Path,
                        default=ROOT / "plugins/vehicle-arrival/target/release" / VEHICLE_ARRIVAL_DLL)
    parser.add_argument("--cover-markers-dll", type=pathlib.Path,
                        default=ROOT / "plugins/cover-markers/target/release" / COVER_MARKERS_DLL)
    parser.add_argument("--weapon-drops-dll", type=pathlib.Path,
                        default=ROOT / "plugins/weapon-drops/target/release" / WEAPON_DROPS_DLL)
    for folder, stem in stage.MOVING_ACTIONS:
        parser.add_argument("--" + pathlib.PurePosixPath(folder).name + "-dll", type=pathlib.Path,
                            default=ROOT / folder / "target/release" / (stem + ".dll"))
    parser.add_argument("--game", default=os.environ.get("DEFIANCE_GAME_DIR"),
                        help="game directory (or its bin) for generated companion mods")
    parser.add_argument("--companion-only", action="store_true",
                        help="package only generated companion mods (needs --game); "
                             "default --out out/defiance-squad-scroll-ui.zip")
    args = parser.parse_args(argv)

    if args.companion_only:
        game = game_root(args.game)
        if game is None:
            parser.error("--companion-only needs --game (or DEFIANCE_GAME_DIR): the mod is "
                         "derived from the installed game's UI files")
        try:
            mod = companion_mods(game)
        except ValueError as error:
            parser.error(f"could not build companion mods: {error}")
        out = pathlib.Path(args.out or ROOT / "out" / "defiance-squad-scroll-ui.zip")
        out.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as package:
            for name, data in mod.items():
                package.writestr(name, data)
        print(f"packaged companion mods ({len(mod)} files) into {out}")
        return 0

    source = pathlib.Path(args.source)
    if not source.is_dir():
        parser.error(f"{source} is not a directory")
    out = pathlib.Path(args.out or ROOT / "out" / (
        "defiance-loader-exe.zip" if args.startup == "exe" else "defiance-loader.zip"))
    out.parent.mkdir(parents=True, exist_ok=True)

    plugins = sorted(
        path for path in source.glob(stage.PLUGIN_GLOB)
        if path.name.lower() not in {name.lower() for name in EXCLUDED}
    )
    standalone = [
        # dll, manifest, the enabled default it must carry (None = any)
        (pathlib.Path(args.regroup_dll),
         ROOT / "plugins/regroup/defiance_plugin_regroup.plugin.json", "false"),
        (pathlib.Path(args.expanded_ammo_dll),
         ROOT / "plugins/expanded-ammo-menu/defiance_plugin_expanded_ammo_menu.plugin.json", "true"),
        (pathlib.Path(args.squad_scroll_dll),
         ROOT / "plugins/squad-management-scroll/defiance_plugin_squad_management_scroll.plugin.json", None),
        (pathlib.Path(args.unit_inspection_dll),
         ROOT / "plugins/unit-inspection/defiance_plugin_unit_inspection.plugin.json", "true"),
        (pathlib.Path(args.ability_groups_dll),
         ROOT / "plugins/ability-groups/defiance_plugin_ability_groups.plugin.json", "true"),
        (pathlib.Path(args.legion_vehicle_hacking_dll),
         ROOT / "plugins/legion-vehicle-hacking/defiance_plugin_legion_vehicle_hacking.plugin.json", "false"),
        (pathlib.Path(args.vehicle_arrival_dll),
         ROOT / "plugins/vehicle-arrival/defiance_plugin_vehicle_arrival.plugin.json", "false"),
        (pathlib.Path(args.cover_markers_dll),
         ROOT / "plugins/cover-markers/defiance_plugin_cover_markers.plugin.json", "true"),
        (pathlib.Path(args.weapon_drops_dll),
         ROOT / "plugins/weapon-drops/defiance_plugin_weapon_drops.plugin.json", "false"),
    ]
    pairs = [(plugin, plugin.with_suffix(".plugin.json")) for plugin in plugins]
    standalone.extend((getattr(args, pathlib.PurePosixPath(folder).name.replace("-", "_") + "_dll"),
                       ROOT / folder / (stem + ".plugin.json"), "false")
                      for folder, stem in stage.MOVING_ACTIONS)
    for dll, manifest, expected in standalone:
        data = json.loads(manifest.read_text(encoding="utf-8"))
        enabled = next(setting["default"] for setting in data["settings"] if setting["key"] == "enabled")
        if expected is not None and enabled != expected:
            parser.error(f"the packaged {data['id']} manifest must default enabled to {expected}")
        if dll.name != data["dll"]:
            parser.error(f"{data['id']} DLL filename does not match its manifest")
        pairs.append((dll, manifest))

    proxy = source / stage.PROXY_LIB
    injector = source / "defiance-pickup-inject.exe"
    loader = source / "defiance_loader.dll"
    helper = source / "defiance-crash-helper.exe"
    startup_binaries = ([injector, loader] if args.startup == "exe" else [proxy])
    binaries = [*startup_binaries, helper, *(plugin for plugin, _ in pairs)]
    symbols = [binary.with_name(binary.stem.replace("-", "_") + ".pdb") for binary in binaries]

    game = game_root(args.game)
    mod = {}
    if args.game and game is None:
        parser.error(f"{args.game} has no basis.pak; pass the game directory or its bin")
    if game is not None:
        try:
            mod = companion_mods(game)
        except ValueError as error:
            parser.error(f"could not build companion mods: {error}")

    for required in [*binaries, *symbols, ROOT / "INSTALL.md",
                     *(path for pair in pairs for path in pair)]:
        if not required.is_file():
            parser.error(f"missing package input: {required}; run mise run loader")
    for dll, manifest in pairs:
        data = json.loads(manifest.read_text(encoding="utf-8"))
        if any(word in name.lower() for name in (dll.name, data["id"]) for word in PRIVATE_WORDS):
            parser.error(f"{dll.name} is a private test plugin and is never packaged")
        expected = BUILTIN_DEFAULTS.get(data["id"])
        enabled = next((setting["default"] for setting in data.get("settings", [])
                        if setting["key"] == "enabled"), None)
        if expected is not None and enabled != expected:
            parser.error(f"the packaged {data['id']} manifest must default enabled to {expected}")

    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as package:
        readme = (ROOT / "INSTALL.md").read_text(encoding="utf-8")
        if args.startup == "exe":
            readme = exe_startup_readme(readme)
        package.writestr("README.md", readme)
        if args.startup == "exe":
            package.write(injector, f"bin/{injector.name}")
            package.write(loader, f"bin/{loader.name}")
            package.writestr("bin/defiance-pickup-inject.ini",
                             "; Injector settings. Command-line options override these values.\n"
                             "builds = known\n"
                             "game = true\n"
                             "wait = 180\n"
                             "pause = auto\n"
                             "mode = loader\n"
                             "loader_dll = defiance_loader.dll\n")
        else:
            package.write(proxy, f"bin/{stage.proxy_name()}")
        package.write(helper, f"bin/{helper.name}")
        for plugin, sidecar in pairs:
            package.write(plugin, f"DefianceLoader/plugins/{plugin.name}")
            package.write(sidecar, f"DefianceLoader/plugins/{sidecar.name}")
        for name, data in mod.items():
            package.writestr(name, data)
    # Keep symbols out of the player install, but archive them with exact binary
    # hashes so later builds cannot silently replace the evidence for a release.
    symbols_out = out.with_name(out.stem + "-symbols.zip")
    manifest = [{"binary": binary.name, "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                 "pdb": symbol.name, "pdb_sha256": hashlib.sha256(symbol.read_bytes()).hexdigest()}
                for binary, symbol in zip(binaries, symbols)]
    with zipfile.ZipFile(symbols_out, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("builds.json", json.dumps(manifest, indent=2) + "\n")
        for symbol in symbols:
            archive.write(symbol, symbol.name)
    companion = f", companion mods ({len(mod)} files)" if mod else " (no companion mods)"
    print(f"packaged {len(pairs)} plugin(s) with {args.startup} startup{companion} into {out}")
    print(f"matching release symbols: {symbols_out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
