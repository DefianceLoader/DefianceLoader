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
plugins/              Core, gameplay plugins, and standalone plugin workspaces
patch/*.asm           the built-in plugins' assembly, as patch units
tools/                analysis, build, packaging and test tooling
tools/variants/       assembled patch units per supported game build
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
link date when that is unknown. The same build's `world2.dll` and
`galileo.dll`, which only Core patches, can go beside them, so that the tests
cover Core's hooks there too.

## Setup

Tools are pinned in `mise.toml`; with [mise](https://mise.jdx.dev/) installed:

```sh
mise install
mise run setup       # Python packages into .venv (tools/requirements.txt)
```

Local settings go in the untracked `mise.local.toml`, for example
`DEFIANCE_GAME_DIR` (the game's `bin` folder, for staging),
`DEFIANCE_EXTRA_GAME_DIRS` (more installs, separated by `;`, that
`mise run loader-stage` and `plugins-stage` also stage into) and
`DEFIANCE_PAK_PASSWORD` (to derive companion UI and animation assets from the
game's archives).

## Build, stage and package

```sh
mise run loader           # build the loader, Core, the plugins and their manifests
mise run loader-stage     # build, then copy into DEFIANCE_GAME_DIR
mise run loader-unstage   # remove them again (settings and logs stay)
mise run loader-package   # out/defiance-loader.zip, plus release symbols
```

`loader` assembles first: with the reference DLLs present it reassembles when
`patch/` or the tooling changed and refreshes `tools/variants/reference`, whose
units the built-in plugins embed; commit those files with the patch change, and
run `mise run variants` for the other builds' units. Staging and packaging
include companion UI and animation data when the game folder is available. The
five moving-actions plugins support the six current 2026 builds.
Grenade, weapon-sync and render-sync fixtures accept `--build <profile>`;
the animation fixture uses `MOVING_ACTIONS_ANIMATION_BUILD` and
`MOVING_ACTIONS_ANIMATION_ASSET_DIR` (the generated companion's `assets` directory).

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
  mods/defiance_moving_actions/ generated animation assets (when included)
```

`bin/defiance-loader.ini` holds only `root` (default `../DefianceLoader`,
relative to `bin`) and the legacy `plugins` override; configuration, logs and
plugin discovery hang off `root`. Settings are startup-only. A disabled
plugin's DLL is not loaded, and the planner blocks anything that depends on it.
Core is required infrastructure, not a toggle.

### Log levels

Every loader and plugin message uses one of four levels. Pick by who needs the
line and how often it can occur:

| Level | Contains | Volume |
| --- | --- | --- |
| `error` | A plugin or feature did not install or stopped working: an unsupported build, a refused hook, a setting that disables a feature, a failed restore. State what is lost and why. | One line per failure. |
| `warn` | Something unexpected that was handled and that a player may notice: a fallback taken, a repair whose post-check failed, a rejected config edit that keeps the previous value, a capacity limit reached. | Rate-limited or capped. |
| `info` | Lifecycle a bug report needs: what installed and for which build (by hash), what settings resolved to, and what a user-requested action armed or finished. | A few lines per plugin per session or per user action; never per frame or per game event. |
| `debug` | Investigation detail: per-event records, probe hits and stacks, census reports, skipped or anomalous snapshots, routine repairs, payload-entry lines. | Bounded by caps or hit budgets, since debug sessions run for hours. |

`level = info` (the default) must give a short, readable log for a session
without problems. Messages do not repeat the plugin version (the loader's
plugin line carries it) or describe release history. Call stacks for code
sites use the `[trace] sites` setting in `core.ini` rather than a log level.

### Filter plugin logs

Set the filters in `DefianceLoader/config/core.ini`, then restart the game:

```ini
[logging]
level = info
include_plugins = defiance.diagnostics, testing.passenger-shot-distance
exclude_plugins =
```

`include_plugins` is a comma-separated list of exact plugin IDs; empty includes
all plugins. `exclude_plugins` omits the listed IDs and takes precedence over
inclusion. Matching ignores case and surrounding spaces. The global `level`
still applies. Plugin lines carry their ID, for example
`[info] [defiance.diagnostics] diagnostics: …`, including worker-thread output.
Use the ID from the plugin manifest or loader plan; for a legacy plugin without
a manifest this is its DLL filename stem.

The filters control `defiance-loader.log` and debugger output. Loader startup,
failure, and reload messages use the global level; crash-report session notes
retain plugin messages independently of the output filters. Filtering changes
output only: active diagnostic probes still capture and decode data.

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
`mise run bump minor features` updates a component's `Cargo.toml`, lockfile and
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

`injector/` can start the complete plugin host without a system-DLL proxy, or
install assembled patches directly for development and diagnostics.

```sh
mise run build
mise run injector-check                             # focused refresh, tests and DLL checks
target/release/defiance-pickup-inject --loader         # full installed plugins; then start the game
target/release/defiance-pickup-inject --patches        # assembled patches with embedded defaults
target/release/defiance-pickup-inject --select-probe   # who asked for a selection
target/release/defiance-pickup-inject --probe          # what the icon chain resolved
target/release/defiance-pickup-inject --scan-check DIR # would another build's DLLs work?
```

The standalone injector continues to use the generated diagnostics assembly
units for patching and `--select-probe`. The loader plugin uses the loader-owned
`trace-capture` service instead; it does not install those units. For live,
reloadable loader probes, see
[Diagnostics probes](diagnostics.md).

Its settings are in `defiance-pickup-inject.ini` beside it, written with comments
on the first run; command-line options override them for one run. New files
default to `mode = loader`. Existing files receive missing launcher keys with
`mode = patches`, preserving their behavior and edits; switch that value to
`loader` or pass `--loader` for the full host.

To package the full plugin loader without a system-DLL proxy, build with
`mise run loader-exe-package` (or `mise run loader`, then
`mise exec -- python tools/package.py --startup exe`).
The archive places `defiance-pickup-inject.exe`, `defiance_loader.dll`, and the
injector INI in `bin`. Its INI selects `mode = loader`; start the injector, then
start the game. Plugin features continue to use their normal files under
`DefianceLoader/plugins` and `DefianceLoader/config`.
The host performs the same configuration validation, missing-setting upgrades,
plugin planning and runtime callbacks as proxy startup. The EXE waits for the
host's startup result, detects an existing host, and reports failures with the
normal log location. `wait`, `pause`, `mode` and `loader_dll` are launcher
settings; gameplay enablement and options belong in the shared plugin INIs.
When switching from a proxy install, remove its system-named loader DLL before
starting a fresh game; retain `DefianceLoader` and its existing configuration.

In direct-patch mode, supported 2026 builds select assembled units automatically
from the pair of `logic.dll` and `game.dll` hashes. Mixed builds are refused,
including with `--scan`. Both modules' anchors and every patch span are checked
before hooks are written; repeat runs verify the installed payloads and hooks.
`--no-game` applies only the logic units, while still checking the DLL pair.
`--scan-check` checks these units against DLLs on disk, and `--self-test`
exercises first and repeat installation for every embedded layout.

`build`, `loader` and `watch` refresh the reference payloads and each supported
layout whose DLL lineage is available, then verify the built injector.
Assembly caches track sources, layouts, DLLs and resolved units; unchanged
units keep their modification times so Cargo avoids needless rebuilds.
Missing local DLLs use committed payloads and are reported as skipped checks.
The checker scans every available supported pair from `tools/builds.py` and
the installation configured by `DEFIANCE_GAME_DIR`. A complete archived pair
without a supported profile fails the check.

The injector embeds every assembled unit automatically. Adding a layout in
`tools/layouts` therefore needs matching committed units, but no injector
feature list or dated test command. Metadata and Rust coverage tests check
those units, and CI and release builds run installation self-tests without
needing game DLLs. Local `test` also checks the available DLL copies;
`test-quick` checks committed metadata and Rust coverage. Regenerate and commit
changed units with source changes; use `mise run variants` to require every
supported layout's DLL lineage rather than allowing committed fallback.

Direct-patch mode uses assembled routines, without the loader's Rust callbacks
or separate plugins. Preview material dimming requires the loader; the modern
selection probe reports raw subjects and callers without interpreting their
object layouts. Run against a fresh game with competing loader patches
disabled: foreign or partially installed hooks are refused.

## Hot reload

For plugin development: set `hot_reload = true` under `[loader]` in
`DefianceLoader/config/core.ini` and restart the game once. Afterwards, rebuild
and copy the plugins while the game runs:

```bat
mise run plugins-stage
```

Each changed plugin is unloaded and loaded again, with the plugins that depend
on it, at the main menu or as a mission starts or a save loads; during a mission
it waits. The log says what reloaded and why anything did not. A plugin whose
manifest sets `"hot_reload": false` loads only at startup (Core and the plugins
that change the game while it starts, such as the expanded ammo menu and squad
scrolling), and a plugin whose settings schema changed needs a restart.

A managed plugin that fails startup initialization can recover after its
corrected DLL is copied in. The loader waits for the replacement to settle,
then retries it at the same menu/mission/save boundary and logs recovery with
the current plugin counts. Disabled plugins and plugins that allow only
startup loading still require their normal enablement or restart path.

At the same points, a plugin copied into the plugins directory is loaded (its
settings are added to its config file with their defaults), and a plugin whose
files are deleted is unloaded, with any plugin that needs it. The feature
plugins shipped with the loader load this way too.

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
are named by their crash range (`selection-logic unit+0x…`). Leave
`sites` empty for normal play. A plugin under development can arm sites itself
through the loader's `trace` service ([plugin-api.md](plugin-api.md)). For
structured fields, stack capture, and editable probe definitions, use the
diagnostics plugin's [trace-capture probes](diagnostics.md); these also share
the four hardware breakpoint slots.

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
