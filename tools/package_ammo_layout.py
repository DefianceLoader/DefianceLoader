"""Package only the expanded-menu layout update, preserving installed config."""
import argparse
import hashlib
import pathlib
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path,
                        default=ROOT / "out/ammo-slot-layout-fix.zip")
    args = parser.parse_args()
    plugin = ROOT / "plugins/expanded-ammo-menu"
    stem = "defiance_plugin_expanded_ammo_menu"
    dll = plugin / "target/release" / (stem + ".dll")
    files = {
        "DefianceLoader/plugins/" + stem + ".dll": dll,
        "DefianceLoader/plugins/" + stem + ".plugin.json": plugin / (stem + ".plugin.json"),
        "README.md": plugin / "README.md",
    }
    instructions = (
        "Close the game. Extract this ZIP into the game directory, replacing the\n"
        "expanded-ammo-menu DLL and manifest in DefianceLoader/plugins.\n"
        "Keep your existing Core, regroup, and config files. Restart the game.\n"
        "This requires the expanded menu to be enabled in config/weapons.ini.\n"
        "The startup log should include: menu context fix v3\n"
        "The all-selected-squads view defaults on; set all_selected_squads = false\n"
        "under [defiance.expanded-ammo-menu] in DefianceLoader/config/weapons.ini to\n"
        "show only the focused squad. See README for experimental limitations.\n"
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(args.output, "w", zipfile.ZIP_DEFLATED) as z:
        for name, path in files.items():
            z.write(path, name)
        z.writestr("INSTALL.txt", instructions)
    with zipfile.ZipFile(args.output) as z:
        assert z.testzip() is None
        assert set(z.namelist()) == set(files) | {"INSTALL.txt"}
        for name, path in files.items():
            assert z.read(name) == path.read_bytes()
    symbols = args.output.with_name(args.output.stem + "-symbols.zip")
    with zipfile.ZipFile(symbols, "w", zipfile.ZIP_DEFLATED) as z:
        for path in (dll, dll.with_suffix(".pdb")):
            z.write(path, path.name)
    print(args.output)
    print("DLL SHA-256:", hashlib.sha256(dll.read_bytes()).hexdigest())


if __name__ == "__main__":
    main()
