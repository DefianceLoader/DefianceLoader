"""Build an installable loader package from a release build directory.

The package is a zip with

    bin/<proxy>.dll              the proxy DLL, to go beside trm.exe
    DefianceLoader/plugins/      the plugin DLLs and their manifests — the
                                 built-ins, plus the standalone regroup,
                                 expanded-ammo-menu and squad-management-scroll
    mods/defiance_squad_scroll/  the scrolling companion UI mod, when the game
                                 directory is available to derive it

The ZIP also includes README.md: the player guide (INSTALL.md), the only
document shipped; the developer and plugin docs stay in the repository. Regroup
is included disabled by default;
expanded-ammo-menu and squad-management-scroll are enabled by default, and the
latter needs its companion UI mod, which is derived from the installed game's
paks. Extract to a
temporary directory and follow README.md; preserve existing configuration files
when copying an upgrade into the game. The loader creates configuration on first
launch; the archive contains no INI files.

    python tools/package.py --game "C:\\Games\\...\\Terminator Dark Fate - Defiance"
    python tools/package.py --source target/release --out out/defiance-loader.zip
    DEFIANCE_GAME_DIR=".../bin" python tools/package.py     # parent is the root

Without a game directory the squad-management-scroll plugin is still packaged,
but without its companion UI mod; the plugin then logs a warning and leaves the
stock panel in place until the mod is added.

The companion UI mod alone (data files derived from the installed game's UI;
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
import package_squad_scroll  # noqa: E402  (the companion UI overlay builder)

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_SOURCE = ROOT / "target" / "release"
REGROUP_DLL = "defiance_plugin_regroup.dll"
EXPANDED_AMMO_DLL = "defiance_plugin_expanded_ammo_menu.dll"
SQUAD_SCROLL_DLL = "defiance_plugin_squad_management_scroll.dll"
EXCLUDED = {"defiance_plugin_pickup.dll", "defiance_plugin_example.dll", REGROUP_DLL}


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


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--source", default=str(DEFAULT_SOURCE))
    parser.add_argument("--out", default=str(ROOT / "out" / "defiance-loader.zip"))
    parser.add_argument("--regroup-dll", type=pathlib.Path,
                        default=ROOT / "plugins/regroup/target/release" / REGROUP_DLL)
    parser.add_argument("--expanded-ammo-dll", type=pathlib.Path,
                        default=ROOT / "plugins/expanded-ammo-menu/target/release" / EXPANDED_AMMO_DLL)
    parser.add_argument("--squad-scroll-dll", type=pathlib.Path,
                        default=ROOT / "plugins/squad-management-scroll/target/release" / SQUAD_SCROLL_DLL)
    parser.add_argument("--game", default=os.environ.get("DEFIANCE_GAME_DIR"),
                        help="game directory (or its bin) for the squad-scroll companion UI mod")
    parser.add_argument("--companion-only", action="store_true",
                        help="package only the companion UI mod (needs --game); "
                             "default --out out/defiance-squad-scroll-ui.zip")
    args = parser.parse_args(argv)

    if args.companion_only:
        game = game_root(args.game)
        if game is None:
            parser.error("--companion-only needs --game (or DEFIANCE_GAME_DIR): the mod is "
                         "derived from the installed game's UI files")
        try:
            mod = package_squad_scroll.mod_entries(game)[0]
        except ValueError as error:
            parser.error(f"could not build the squad-scroll companion mod: {error}")
        out = pathlib.Path(args.out if "--out" in (argv or []) else
                           ROOT / "out" / "defiance-squad-scroll-ui.zip")
        out.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as package:
            for name, data in mod.items():
                package.writestr(name, data)
        print(f"packaged the companion UI mod ({len(mod)} files) into {out}")
        return 0

    source = pathlib.Path(args.source)
    if not source.is_dir():
        parser.error(f"{source} is not a directory")
    out = pathlib.Path(args.out)
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
    ]
    pairs = [(plugin, plugin.with_suffix(".plugin.json")) for plugin in plugins]
    for dll, manifest, expected in standalone:
        data = json.loads(manifest.read_text(encoding="utf-8"))
        enabled = next(setting["default"] for setting in data["settings"] if setting["key"] == "enabled")
        if expected is not None and enabled != expected:
            parser.error(f"the packaged {data['id']} manifest must default enabled to {expected}")
        if dll.name != data["dll"]:
            parser.error(f"{data['id']} DLL filename does not match its manifest")
        pairs.append((dll, manifest))

    proxy = source / stage.PROXY_LIB
    helper = source / "defiance-crash-helper.exe"
    binaries = [proxy, helper, *(plugin for plugin, _ in pairs)]
    symbols = [binary.with_name(binary.stem.replace("-", "_") + ".pdb") for binary in binaries]

    game = game_root(args.game)
    mod = {}
    if args.game and game is None:
        parser.error(f"{args.game} has no basis.pak; pass the game directory or its bin")
    if game is not None:
        try:
            mod = package_squad_scroll.mod_entries(game)[0]
        except ValueError as error:
            parser.error(f"could not build the squad-scroll companion mod: {error}")

    for required in [*binaries, *symbols, ROOT / "INSTALL.md",
                     *(path for pair in pairs for path in pair)]:
        if not required.is_file():
            parser.error(f"missing package input: {required}; run mise run loader")

    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as package:
        package.write(ROOT / "INSTALL.md", "README.md")
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
    companion = f", companion UI mod ({len(mod)} files)" if mod else " (no companion UI mod)"
    print(f"packaged {len(pairs)} plugin(s){companion} into {out}")
    print(f"matching release symbols: {symbols_out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
