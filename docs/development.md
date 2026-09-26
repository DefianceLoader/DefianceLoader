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
reassemble the payloads after editing `patch/` and for the native tests that
execute the game's code; they are never committed. Each build's `logic.dll`
and `game.dll` go in `bin/<store>/<date>/`: the GOG release the patches were
written against in `bin/gog/2025-12-23/`, later ones beside it (for example
`bin/gog/2026-09-14/`, `bin/steam/2026-09-22/`). The date is the one
`tools/layouts/` names the build by, the store's release date, or the DLLs'
link date when that is unknown.

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
include the companion UI mods when the game folder is available.

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

`mise run test` skips a native test that already passed on the same inputs:
its script and the tool modules it uses, the built DLLs, payloads and game DLLs
it loads, and the Python package versions (`tools/stamp.py test`). A skipped
test says so. `DEFIANCE_NO_STAMP=1` runs everything; do that before a release.

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
`tools/layouts/`. Each build is derived from its neighbour, not from the
build the patches were written for: a GOG build from the previous GOG build, a
Steam build from the GOG build the same update shipped as. `tools/builds.py`
names each build's base, `tools/chainlayout.py` drafts a new build's layout
from its base's, and the variant resolver finds its sites through the base's.
The loader and every plugin check each target against the exact supported build
before writing anything.

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

## Hot reload

For plugin development: set `hot_reload = true` under `[loader]` in
`DefianceLoader/config/core.ini` and restart the game once. Afterwards, rebuild
and copy the plugins while the game runs:

```bat
mise run plugins-stage
```

Each changed plugin is unloaded and loaded again, with the plugins that depend
on it, at the main menu or as a mission starts or a save loads; during a mission
it waits. The log says what reloaded and why anything did not. Core, the
expanded ammo menu and squad scrolling load only at startup, and a plugin whose
settings changed needs a restart.

At the same points, a plugin copied into the plugins directory is loaded (its
settings are added to its config file with their defaults), and a plugin whose
files are deleted is unloaded, with any plugin that needs it. A built-in plugin
added this way still needs a restart.

## Tracing callers in game

To find what calls a function in the running game, name it in
`DefianceLoader/config/core.ini` and restart:

```ini
[trace]
sites = logic+0x42a940, game+0x366fa7
hits = 20
```

Add `when = mission` to start tracing only once the first mission loads, so
the main menu's scene does not use up the hits.

Each hit logs its call stack (module+offset frames) and the first four
argument registers to `defiance-loader.log`. It uses hardware breakpoints, so
it changes no code and works on addresses other patches already own; at most
four sites, each released once it has logged its hits. Frames prefixed `?`
were recovered by scanning the stack and can be stale; frames in patch payloads
are named by their crash range (`core.logic assembly payload+0x…`). Leave
`sites` empty for normal play. A plugin under development can arm sites itself
through the loader's `trace` service ([plugin-api.md](plugin-api.md)).

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
