"""Stage the loader and its plugins into the game directory, or take them out.

The loader is a proxy DLL: it is placed beside `trm.exe` under the name of a
system DLL the game imports (`dxgi.dll` by default). Plugin discovery follows
the same rules as the runtime: `root/plugins`, where `root` comes from the
bootstrap `root` key (default `../DefianceLoader`, resolved against bin), or the
legacy unsectioned `plugins` key when present, resolved against bin exactly as
before. Relative paths are resolved against bin, not the working directory.

It also stages the standalone regroup, expanded-ammo-menu,
squad-management-scroll and unit-inspection plugins from their own workspaces,
and, when the game directory holds the PAKs, derives and writes the companion
UI mods (`COMPANION_MODS`: `mods/defiance_squad_scroll`,
`mods/defiance_unit_inspection`). Uninstall removes them, again only when they
are recognised as ours.

It never writes, moves or deletes configuration or logs.

It never touches a file it did not write: the proxy is recognised by a string
the loader carries, and an existing foreign DLL of the same name is moved aside
to `<name>.defiance-backup` first, and put back on uninstall.

    python tools/stage.py --game "C:\\Games\\...\\bin"
    python tools/stage.py --game "..." --dry-run
    python tools/stage.py --game "..." --uninstall

`--game` may be left out if DEFIANCE_GAME_DIR is set.
"""
import argparse
import hashlib
import json
import os
import pathlib
import re
import shutil
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import package_squad_scroll  # noqa: E402  (the companion UI overlay builder)
import package_unit_inspection  # noqa: E402  (the reload bar mod builder)

ROOT = pathlib.Path(__file__).resolve().parent.parent
GENERATED = ROOT / "crates" / "loader" / "src" / "proxy_generated.rs"
DEFAULT_SOURCE = ROOT / "target" / "release"
PROXY_LIB = "defiance_loader.dll"
CRASH_HELPER = "defiance-crash-helper.exe"
PLUGIN_GLOB = "defiance_plugin_*.dll"
BACKUP_SUFFIX = ".defiance-backup"
# Strings every file this project builds carries; they tell a file of ours from
# a foreign one of the same name. The proxy carries the config/log prefix, a
# plugin its own `defiance.<name>` string.
MARKERS = (b"defiance-loader", b"defiance.")
# The companion mods the plugins need: (directory under the game's `mods`, the
# name its mod.json carries, builder). The name is checked before a tree is
# overwritten or removed.
MOD_DIR = "defiance_squad_scroll"
MOD_NAME = "Defiance squad inventory scrolling"
COMPANION_MODS = [
    (MOD_DIR, MOD_NAME, package_squad_scroll.mod_entries),
    (package_unit_inspection.MOD_DIR, package_unit_inspection.MOD_NAME,
     package_unit_inspection.mod_entries),
]
# Standalone plugins built from their own workspaces, staged beside the loader
# plugins so staging matches the loader package.
STANDALONE = [
    ("plugins/regroup", "defiance_plugin_regroup"),
    ("plugins/expanded-ammo-menu", "defiance_plugin_expanded_ammo_menu"),
    ("plugins/squad-management-scroll", "defiance_plugin_squad_management_scroll"),
    ("plugins/unit-inspection", "defiance_plugin_unit_inspection"),
]


def game_root(path):
    """Accept the game's bin or its root; return the directory holding the paks."""
    path = pathlib.Path(path)
    if (path / "basis.pak").is_file():
        return path
    if (path.parent / "basis.pak").is_file():
        return path.parent
    return None


def bin_directory(path):
    """The game's `bin`, accepting either it or the game root.

    The bootstrap INI and the `../DefianceLoader` default are resolved against
    bin, so staging must operate there: given the root (it holds `basis.pak` and
    a `bin`), return that `bin`, or the add-on tree and proxy land one level up.
    """
    path = pathlib.Path(path)
    if (path / "basis.pak").is_file() and (path / "bin").is_dir():
        return path / "bin"
    return path


def is_our_mod(mod_dir, name=MOD_NAME):
    """Whether a companion mod tree is the one this project wrote."""
    try:
        data = json.loads((mod_dir / "mod.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    return data.get("name") == name


def standalone_pairs():
    """The built standalone plugin (DLL, committed manifest) pairs, when present."""
    pairs = []
    for workspace, stem in STANDALONE:
        dll = ROOT / workspace / "target/release" / (stem + ".dll")
        manifest = ROOT / workspace / (stem + ".plugin.json")
        if dll.is_file() and manifest.is_file():
            pairs.append((dll, manifest))
    return pairs


def proxy_name():
    """The system DLL the loader stands in for, from the generated forwarders."""
    match = re.search(r'pub const REAL: &str\s*=\s*"([^"]+)"', GENERATED.read_text(encoding="utf-8"))
    if match is None:
        raise SystemExit(f"no REAL in {GENERATED}; run tools/proxy.py first")
    return match.group(1)


def digest(path):
    hasher = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def looks_ours(path):
    """Whether a file was written by this project, by the marker strings in it."""
    try:
        with open(path, "rb") as handle:
            data = handle.read()
    except OSError:
        return False
    return any(marker in data for marker in MARKERS)


def bootstrap_value(game, wanted):
    """The last unsectioned value of `wanted` in bin/defiance-loader.ini, or None.

    Only the bootstrap keys are read; a sectioned key of the same name (for
    example a plugin's) is ignored, matching the runtime's rules.
    """
    config = game / "defiance-loader.ini"
    try:
        text = config.read_text(encoding="utf-8")
    except FileNotFoundError:
        return None
    section = ""
    value = None
    for raw in text.splitlines():
        line = re.split(r"[;#]", raw, maxsplit=1)[0].strip()
        if not line:
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1].strip().lower()
            continue
        if section or "=" not in line:
            continue
        key, candidate = line.split("=", 1)
        if key.strip().lower() == wanted:
            value = candidate.strip()
    return value


def resolve_against(base, value):
    """Resolve a bootstrap path against bin, never the working directory."""
    path = pathlib.Path(value)
    return path if path.is_absolute() else base / path


def loader_root(game):
    """The add-on tree, matching the runtime's `root` default."""
    return resolve_against(game, bootstrap_value(game, "root") or "../DefianceLoader")


def plugin_directory(game):
    """The plugin discovery directory: the legacy override, or root/plugins."""
    override = bootstrap_value(game, "plugins")
    if override is not None:
        return resolve_against(game, override)
    return loader_root(game) / "plugins"


class Staging:
    def __init__(self, game, source, force, dry, extra_plugins=(), companion=True,
                 expect_standalone=False):
        self.game = game
        self.source = source
        self.force = force
        self.dry = dry
        self.loader = source / PROXY_LIB
        staged = sorted(p for p in source.glob(PLUGIN_GLOB)
                        if p.name.lower() not in {"defiance_plugin_pickup.dll", "defiance_plugin_example.dll"})
        # The sidecar manifest beside each staged DLL, if the build produced one,
        # or the standalone plugin's committed manifest beside its workspace.
        manifests = {plugin: source / (plugin.stem + ".plugin.json") for plugin in staged}
        self.extra_plugins = list(extra_plugins)
        self.expect_standalone = expect_standalone
        for dll, manifest in self.extra_plugins:
            if dll.name.lower() not in {plugin.name.lower() for plugin in staged}:
                staged.append(dll)
                manifests[dll] = manifest
        self.plugins = staged
        self.manifests = [manifests[plugin] for plugin in staged if manifests[plugin].is_file()]
        self.real = proxy_name()
        self.target = game / self.real
        self.backup = game / (self.real + BACKUP_SUFFIX)
        self.plugin_dir = plugin_directory(game)
        root = game_root(game)
        # One {dir, name, entries, reason} per companion mod.
        self.mods = []
        for directory, name, build in COMPANION_MODS:
            mod = dict(dir=(root or game.parent) / "mods" / directory, prefix=f"mods/{directory}/",
                       name=name, entries={}, reason="")
            if companion:
                if root is None:
                    mod["reason"] = "the game directory with basis.pak was not found beside bin"
                else:
                    try:
                        mod["entries"] = build(root)[0]
                    except ValueError as error:
                        mod["reason"] = str(error)
            self.mods.append(mod)

    def act(self, verb, path, extra=""):
        print(f"  {'(dry-run) ' if self.dry else ''}{verb} {path}{extra}")

    def copy(self, source, destination):
        self.act("copy", source, f" -> {destination}")
        if not self.dry:
            destination.parent.mkdir(parents=True, exist_ok=True)
            # Write beside it and swap in, so a hot-reload watcher never reads a
            # half-written DLL.
            partial = destination.with_name(destination.name + ".partial")
            shutil.copy2(source, partial)
            os.replace(partial, destination)

    def install_plugins(self):
        """Copy only the plugin DLLs and their manifests: the hot-reload
        workflow while the game runs, when the proxy DLL itself is locked."""
        print(f"installing {len(self.plugins)} plugin(s) into {self.plugin_dir}")
        for plugin in self.plugins:
            self.copy(plugin, self.plugin_dir / plugin.name)
        for manifest in self.manifests:
            self.copy(manifest, self.plugin_dir / manifest.name)
        print("done. With [loader] hot_reload = true, a running game reloads the "
              "changed plugins at the menu or when a mission starts or a save loads.")

    def install(self):
        print(f"installing the loader as {self.real} and {len(self.plugins)} plugin(s) into {self.plugin_dir}")
        if not self.loader.is_file():
            raise SystemExit(f"{self.loader} is missing; build it first (cargo build --release)")
        helper = self.source / CRASH_HELPER
        helper_target = self.game / CRASH_HELPER
        if helper.is_file() and helper_target.exists() and not looks_ours(helper_target) and not self.force:
            raise SystemExit(f"{helper_target} is not ours; move it aside before installing")
        if self.target.exists() and not looks_ours(self.target):
            if self.backup.exists() and not self.force:
                raise SystemExit(
                    f"{self.target} is not ours and {self.backup} already exists; "
                    "move one aside by hand, or pass --force to overwrite"
                )
            self.act("back up", self.target, f" -> {self.backup}")
            if not self.dry:
                os.replace(self.target, self.backup)
        self.copy(self.loader, self.target)
        if helper.is_file():
            self.copy(helper, helper_target)
        else:
            print("  warning: crash helper missing; native crash reports will be text only")
        if not self.plugins:
            print("  warning: no plugin DLLs were built; the loader will do nothing")
        if self.expect_standalone:
            present = {plugin.stem for plugin, _ in self.extra_plugins}
            missing = [stem for _, stem in STANDALONE if stem not in present]
            if missing:
                print(f"  warning: standalone plugin(s) not built: {', '.join(missing)}; run mise run loader")
        for plugin in self.plugins:
            self.copy(plugin, self.plugin_dir / plugin.name)
        for manifest in self.manifests:
            self.copy(manifest, self.plugin_dir / manifest.name)
        self.write_mod()
        print(f"done. Start the game; the log is {self.game / 'defiance-loader.log'}")

    def write_mod(self):
        """Write the companion UI mods whose sources are available."""
        for mod in self.mods:
            if not mod["entries"]:
                if mod["reason"]:
                    print(f"  warning: companion mod {mod['dir'].name} not installed: {mod['reason']}")
                continue
            if mod["dir"].exists() and not is_our_mod(mod["dir"], mod["name"]) and not self.force:
                raise SystemExit(f"{mod['dir']} is not ours; move it aside or pass --force")
        for mod in self.mods:
            prefix = mod["prefix"]
            for name, data in mod["entries"].items():
                relative = name[len(prefix):] if name.startswith(prefix) else pathlib.PurePosixPath(name).name
                self.act("write", mod["dir"] / relative)
                if not self.dry:
                    destination = mod["dir"] / relative
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    destination.write_bytes(data)

    def remove_mod(self):
        """Remove the companion mod trees this project wrote."""
        for mod in self.mods:
            if not mod["dir"].exists() or not is_our_mod(mod["dir"], mod["name"]):
                continue
            self.act("remove", mod["dir"], " (companion UI mod)")
            if not self.dry:
                shutil.rmtree(mod["dir"])

    def uninstall(self):
        print(f"removing the loader and its plugins from {self.game}")
        helper = self.game / CRASH_HELPER
        if helper.exists() and (looks_ours(helper) or self.force):
            self.act("remove", helper)
            if not self.dry:
                helper.unlink()
        if self.target.exists():
            if looks_ours(self.target) or self.force:
                self.act("remove", self.target)
                if not self.dry:
                    self.target.unlink()
                if self.backup.exists():
                    self.act("restore", self.backup, f" -> {self.real}")
                    if not self.dry:
                        os.replace(self.backup, self.target)
            else:
                print(f"  kept {self.target}: it is not ours (use --force to remove anyway)")
        elif self.backup.exists():
            self.act("restore", self.backup, f" -> {self.real}")
            if not self.dry:
                os.replace(self.backup, self.target)
        for plugin in sorted(self.plugin_dir.glob(PLUGIN_GLOB)):
            if looks_ours(plugin) or self.force:
                self.act("remove", plugin)
                if not self.dry:
                    plugin.unlink()
        # Remove our manifests too, but leave an unrelated third-party one alone.
        staged = {plugin.name.lower() for plugin in self.plugins}
        for manifest in sorted(self.plugin_dir.glob("*.plugin.json")):
            stem = manifest.name[: -len(".plugin.json")]
            owns = (stem + ".dll").lower() in staged
            if owns or looks_ours(manifest) or self.force:
                self.act("remove", manifest)
                if not self.dry:
                    manifest.unlink()
        self.remove_mod()
        if self.plugin_dir.is_dir() and not any(self.plugin_dir.iterdir()):
            self.act("remove empty directory", self.plugin_dir)
            if not self.dry:
                self.plugin_dir.rmdir()
        print(f"done. {self.game / 'defiance-loader.log'} and the .ini are left in place")


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--game", default=os.environ.get("DEFIANCE_GAME_DIR"),
                        help="the game's bin directory (or set DEFIANCE_GAME_DIR)")
    parser.add_argument("--source", default=str(DEFAULT_SOURCE),
                        help=f"where the built files are (default {DEFAULT_SOURCE})")
    parser.add_argument("--uninstall", action="store_true", help="remove instead of install")
    parser.add_argument("--force", action="store_true",
                        help="overwrite or remove a file that is not recognised as ours")
    parser.add_argument("--dry-run", action="store_true", help="print the plan and change nothing")
    parser.add_argument("--plugins-only", action="store_true",
                        help="copy only the plugin DLLs and manifests (while the game runs, for hot reload)")
    args = parser.parse_args(argv)

    if not args.game:
        parser.error("give --game DIR or set DEFIANCE_GAME_DIR")
    game = bin_directory(args.game)
    if not game.is_dir():
        parser.error(f"{game} is not a directory")
    staging = Staging(game, pathlib.Path(args.source), args.force, args.dry_run,
                      extra_plugins=standalone_pairs(), expect_standalone=True,
                      companion=not args.plugins_only)
    if args.uninstall:
        staging.uninstall()
    elif args.plugins_only:
        staging.install_plugins()
    else:
        staging.install()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
