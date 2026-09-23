"""Package the experimental regroup DLL without replacing core or configuration."""
import argparse
import hashlib
import json
import pathlib
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dll", type=pathlib.Path,
                        default=ROOT / "plugins/regroup/target/release/defiance_plugin_regroup.dll")
    parser.add_argument("--output", type=pathlib.Path, default=ROOT / "out/defiance-regroup-prototype.zip")
    args = parser.parse_args()
    manifest_path = ROOT / "plugins/regroup/defiance_plugin_regroup.plugin.json"
    manifest = json.loads(manifest_path.read_text())
    if args.dll.name != manifest["dll"] or not args.dll.is_file():
        raise SystemExit(f"build the release DLL first: {args.dll}")
    import pefile
    pe = pefile.PE(str(args.dll))
    if b"defiance_plugin" not in {e.name for e in pe.DIRECTORY_ENTRY_EXPORT.symbols}:
        raise SystemExit("DLL is missing the plugin export")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    # Generate a merge example from the same defaults/help as the manifest.
    # Never package a replacement infantry.ini: it contains other plugins too.
    import textwrap
    example = args.output.parent / "regroup-hotkeys.ini.example"
    lines = ["; Merge these entries into the existing section in config/infantry.ini.",
             "; This example is not loaded automatically. Preserve your other settings.",
             "[defiance.regroup]"]
    for setting in manifest["settings"]:
        lines.extend("; " + line for line in textwrap.wrap(setting["description"], width=88))
        lines.append(f'{setting["key"]} = {setting["default"]}')
    example.write_text("\n".join(lines) + "\n", encoding="utf-8")
    files = [(example, example.name), (args.dll, manifest["dll"]), (manifest_path, manifest_path.name),
             (ROOT / "plugins/regroup/README.md", "README.md")]
    with zipfile.ZipFile(args.output, "w", zipfile.ZIP_DEFLATED) as archive:
        for source, name in files:
            archive.write(source, name)
    with zipfile.ZipFile(args.output) as archive:
        assert archive.testzip() is None
        assert sorted(archive.namelist()) == sorted(name for _, name in files)
        for source, name in files:
            assert archive.read(name) == source.read_bytes()
    print(f"{args.output}: {len(files)} files")
    print(f"DLL SHA-256: {hashlib.sha256(args.dll.read_bytes()).hexdigest()}")


if __name__ == "__main__":
    main()
