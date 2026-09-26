# Building from source

The release package — the loader (`bin/dxgi.dll`), its crash helper, the
built-in and standalone plugins, and the player guide — builds from this
repository alone. No copy of the game is needed: the patch payloads Core embeds
are committed under `tools/variants/`, and the per-build signatures are
committed in the sources.

Official releases are built this way by the `release` workflow
(`.github/workflows/release.yml`) on GitHub's Windows runners, and every file it
produces carries a signed build-provenance attestation. To confirm a downloaded
file came from that workflow and this repository:

```sh
gh attestation verify defiance-loader-vX.Y.Z.zip -R DefianceLoader/DefianceLoader
```

`SHA256SUMS.txt` on each release lists the archives and every DLL and EXE inside
them.

## Requirements

- Windows 10 or 11, x64.
- [Rust](https://rustup.rs/) stable with the `x86_64-pc-windows-msvc` toolchain
  (rustup's default on Windows).
- Visual Studio 2022 Build Tools with the "Desktop development with C++"
  workload, which provides the MSVC linker and the Windows SDK (`rc.exe`
  embeds version information in release binaries).
- Python 3.12, only for packaging:
  `python -m pip install -r tools/requirements.txt`.

## Build and package

From the repository root, in a command prompt or PowerShell:

```bat
cargo build --release
cargo build --release --manifest-path plugins/regroup/Cargo.toml
cargo build --release --manifest-path plugins/expanded-ammo-menu/Cargo.toml
cargo build --release --manifest-path plugins/squad-management-scroll/Cargo.toml
cargo build --release --manifest-path plugins/unit-inspection/Cargo.toml
target\release\manifest-gen.exe --out target\release
python tools/package.py --out out/defiance-loader.zip
python tools/checksums.py out/defiance-loader.zip --out out/SHA256SUMS.txt
```

`out/defiance-loader.zip` is the player package and
`out/defiance-loader-symbols.zip` holds the matching debug symbols (PDBs) with
the hash of each binary they belong to. With [mise](https://mise.jdx.dev/)
installed, `mise run loader` performs the build steps.

Builds on another machine or toolchain version are not guaranteed to be
byte-identical to the release; compare behaviour and the source tag, or use
the attestation above to check the released files themselves.

## The companion UI mods

The squad-management-scroll plugin needs a small UI mod that adds scrollbars to
the game's unit panels; the same mod right-aligns the in-mission ammo card's
user count so two-digit fractions fit, and carries darker copies of the game's
standard materials for the squad preview's unselected soldiers. Unit
inspection's mod holds a greyscale copy of the ammo card's reload bar texture,
which the plugin colours (`tools/package_unit_inspection.py`). Both are plain
text and DDS textures, derived from the game's own UI and material definitions,
so they can only be built on a machine with the game installed, and they are
distributed as a separate download (`defiance-squad-scroll-ui.zip`). The game's
archives are encrypted; set `DEFIANCE_PAK_PASSWORD` to their password first.

```bat
python tools/package.py --companion-only --game "C:\Games\...\Terminator Dark Fate - Defiance"
```

## What the loader does

The game loads `bin/dxgi.dll` in place of the system DLL; it forwards every
DirectX call to the real `dxgi.dll` in the Windows system folder and loads the
plugins from `DefianceLoader/plugins`. The plugins change the game's behaviour
by patching its code in memory, inside the game's own process
(`VirtualAllocEx`/`WriteProcessMemory` on that process, with other threads
briefly suspended while a patch is written), after checking each target against
the exact supported game build. The loader and plugins contain no networking
code, and they write only inside the game folder: the loader's folder
(`DefianceLoader/`: settings, logs and crash reports, or wherever
`bin/defiance-loader.ini` points it), that `.ini` itself, and, while the
diagnostics trace runs, `bin/defiance-pickup-inject.block`. Security software may
flag this pattern — a proxy DLL that patches the host process — as it is
shared by game mod loaders and malware alike.
