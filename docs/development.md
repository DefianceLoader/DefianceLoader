# Development

How the repository is laid out, and how to build, stage, test and release the
loader. To build a release package only, see [building](building.md).

## Layout

```text
crates/api            the plugin ABI (the contract between loader and plugins)
crates/feature-sdk    helpers for plugins: typed settings, native installs
crates/build-support  Windows version resources for shipped binaries
crates/core           patching machinery shared by the loader and the injector
crates/loader         the proxy DLL that hosts plugins inside the game
plugins/              Core, the gameplay plugins, and three standalone plugin
                      workspaces (regroup, expanded-ammo-menu,
                      squad-management-scroll)
patch/*.asm           the assembly patches Core applies
tools/                analysis, build, packaging and test tooling
tools/variants/       assembled patch payloads per supported game build
injector/             an external injector that patches a running game
examples/services/    cross-plugin service examples
```

Everything builds from a checkout. The game's own DLLs are needed only to
reassemble the payloads after editing `patch/` (copy `logic.dll` and `game.dll`
to `bin/logic.orig.dll` and `bin/game.orig.dll`; they are never committed) and
for the native tests that execute the game's code.

## Setup

Tools are pinned in `mise.toml`; with [mise](https://mise.jdx.dev/) installed:

```sh
mise install
mise run setup       # Python packages into .venv (tools/requirements.txt)
```

Local settings go in the untracked `mise.local.toml`, for example
`DEFIANCE_GAME_DIR` (the game's `bin` folder, for staging) and
`DEFIANCE_PAK_PASSWORD` (to derive the squad-scroll UI mod from the game's
archives).

## Build, stage and package

```sh
mise run loader           # build the loader, Core, the plugins and their manifests
mise run loader-stage     # build, then copy into DEFIANCE_GAME_DIR
mise run loader-unstage   # remove them again (settings and logs stay)
mise run loader-package   # out/defiance-loader.zip, plus release symbols
```

`loader` assembles first: with the reference DLLs present it reassembles when
`patch/` or the tooling changed and refreshes `tools/variants/reference`, which
Core embeds; commit those files with the patch change. Staging and packaging
include the squad-scroll companion UI mod when the game folder is available.

The installed layout:

```text
Game/
  bin/
    trm.exe
    dxgi.dll                   the loader (a proxy for the system dxgi.dll)
    defiance-loader.ini        bootstrap only: root, legacy plugins override
  DefianceLoader/
    plugins/                   plugin DLLs, one .plugin.json manifest each
    config/                    core.ini, infantry.ini, weapons.ini, diagnostics.ini
    logs/defiance-loader.log
```

`bin/defiance-loader.ini` holds only `root` (default `../DefianceLoader`,
relative to `bin`) and the legacy `plugins` override; configuration, logs and
plugin discovery hang off `root`. Settings are startup-only. A disabled
plugin's DLL is not loaded, and the planner blocks anything that depends on it.
Core is required infrastructure, not a toggle.

## Tests

```sh
mise run fmt                 # format every Rust workspace (tests check it)
mise run test-quick          # format check, Rust tests, versions, layouts
mise run loader-infra-test   # loader, API and plugin tests; no game DLLs needed
mise run test                # everything, including native tests (needs bin/)
```

Plugin-specific suites have their own tasks (`mise tasks` lists them), for
example `mise run squad-scroll-test`. CI runs the tests that need no game files
on every code change.

## Versions and releases

`mise run bump` lists component versions; `mise run bump patch loader` or
`mise run bump minor builtins` updates a component's `Cargo.toml`, lockfile and
manifest together, refusing a bump that would break a dependant's declared
range. `mise run bump check` (also run by the tests) fails if any copy
disagrees. Releases are built by the `release` workflow from a `v*` tag; see
[building](building.md).

## Supporting another game build

Per-build signatures and addresses are generated, never hand-edited: the
`tools/*_bindings.py` generators write each plugin's `sites.rs`, and
`mise run variants` assembles and resolves the payloads for the builds named in
`tools/layouts/`. The loader and every plugin check each target against the
exact supported build before writing anything.

## The injector

`injector/` patches a running game from outside instead of loading with it,
which is useful for development: it can probe what a patch resolved and check
whether another build's DLLs would work.

```sh
mise run build
target/release/defiance-pickup-inject --self-test
target/release/defiance-pickup-inject                  # then start the game
target/release/defiance-pickup-inject --select-probe   # who asked for a selection
target/release/defiance-pickup-inject --probe          # what the icon chain resolved
target/release/defiance-pickup-inject --scan-check DIR # would another build's DLLs work?
```

Its settings are in `defiance-pickup-inject.ini` beside it, written with comments
on the first run; command-line options override them for one run.

## Analysis tools

```text
tools/pe.py      loading, addresses, strings, cross-references, disassembly
tools/index.py   the cached cross-reference index
tools/rtti.py    MSVC RTTI: class name -> vtable -> methods
tools/show.py    dump the function containing an address
```

`tools/pe.py` takes function bounds from `.pdata` rather than sweeping `.text`
linearly; a linear sweep stops at the first padding byte and misses most
cross-references.
