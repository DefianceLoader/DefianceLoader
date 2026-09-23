# defiance_re

Plugin loader, gameplay patches and reverse-engineering tools for
*Terminator: Dark Fate - Defiance*. The loader is the player-facing install;
the external injector remains available for development and diagnostics.

**Installing a release?** Follow the [quick installation guide](INSTALL.md),
also included as `README.md` in the release ZIP. Regroup is included as an
experimental add-on and is disabled by default.

**Writing a plugin?** See the [author guide](docs/plugin-authoring.md),
[public API reference](docs/plugin-api.md), and
[runnable service examples](examples/services/README.md).

    bin/logic.orig.dll      a copy of the GOG logic.dll; the game tree stays read-only
    bin/game.orig.dll       a copy of the GOG game.dll, likewise (neither is tracked)
    bin/<store>/            other builds' logic.dll and game.dll, checked by the tests
    out/logic.dll           the built, patched DLL
    patch/pickup.asm        the replacement weapon-pickup chooser
    patch/move-filter.asm   narrows a move order to the marked soldiers
    patch/icon-squad.asm    world/army-list squad expansion and complete double-click results
    patch/preview-weapon.asm  squad previews keep the first matching weapon
    patch/building-control.asm  TAB control of each squad's building occupants
    patch/select-trace.asm  diagnostic: who drives a selection, read with --select-probe
    patch/select-squad.asm  make manager squad selection mark every member, not only the leader
    patch/soldier-mark.asm  the soldier's setSelected: marks him, keeps his squad's selection in step
    patch/pose.asm          lie down and stand up for the marked soldiers only
    patch/posture-gate.asm  per-soldier posture pins the squad cannot override
    patch/prone-query.asm   what V offers, answered for the picked soldiers
    patch/firing-mode.asm   firing mode (T) for the picked soldiers
    patch/ammo-mode.asm     weapon/ammo toggles for picked soldiers
    patch/behaviour-census.asm  diagnostic: who reads a squad's behaviour (G), read with --select-probe
    patch/pickup-*.asm      earlier rules, kept for reference

    injector/              the external injector, for patching without touching the game
    tools/                  the analysis and build tooling

Setup, once:

    mise exec python@3.12 "--" python -m venv .venv
    ./.venv/Scripts/python.exe -m pip install -r tools/requirements.txt

## Install and build the plugin loader

The injector patches a running game from outside. The loader does the same work
from inside: a proxy DLL the game loads itself, hosting plugins. For players it
is the easier install — no separate program to start, and the store's shortcut
keeps working. The injector stays the developer's tool, whose probes and
`--scan-check` read a patch the loader does not.

    python tools/proxy.py --scan "C:\Games\...\bin"  # pick the proxy DLL, write its forwarders
    mise run loader                                  # assemble and build the plugins
    python tools/stage.py --game "C:\Games\...\bin"  # copy them into the game
    python tools/stage.py --game "..." --uninstall   # take them back out

`mise run loader`, `mise run loader-stage` and `mise run loader-unstage` wrap
those. `mise run loader-package` writes an installable zip (proxy, plugins,
manifests and quick installation instructions; no INI files) to
`out/defiance-loader.zip`. The loader creates commented INIs on first launch;
generated field help includes its default value. Existing settings are preserved.
`loader` also builds the separate regroup, expanded-ammo-menu and
squad-management-scroll workspaces. `loader-package` includes all three in the
ZIP: regroup disabled by default; expanded-ammo-menu and
squad-management-scroll enabled by default, the latter with its companion UI mod under
`mods/defiance_squad_scroll/`. That mod is derived from the installed game's
paks, so pass `--game` (or set `DEFIANCE_GAME_DIR`); without it the plugin ships
alone and leaves the stock panel in place. Their READMEs become
`DefianceLoader/REGROUP.md`, `EXPANDED-AMMO.md` and `SQUAD-SCROLL.md`.

Before a release, bump the versions of what changed: `mise run bump` lists the
components, and `mise run bump patch loader` or `mise run bump minor builtins`
updates a component's `Cargo.toml`, lockfile and manifest (the built-in table
or its `.plugin.json`) together, refusing a bump that would break a
dependant's declared range. Each binary's version info and each plugin's
exported version come from its `Cargo.toml`. `mise run bump check` (also run by
the test tasks) fails if any copy disagrees.
`loader-stage` now stages the same set the package contains: the built-ins plus
the three standalone plugins, and, when the game directory holds the PAKs, the
squad-scroll companion UI mod under `mods/defiance_squad_scroll`;
`loader-unstage` removes them again. The loader is the
player-facing default; the injector is the developer tool. Ten
gameplay/diagnostic plugins share the assembly payloads,
with explicit dependencies and per-plugin patch ownership. The plugin
API is in [docs/plugin-api.md](docs/plugin-api.md) and plugin authoring in
[docs/plugin-authoring.md](docs/plugin-authoring.md); the SDK example is in
plugins/example/README.md.

The new optional attack and garrison plugins narrow explicit attack and
building-entry orders to marked soldiers. Building-panel exit orders use only
that building's occupants. Attack and entry have been live-tested; the exit fix
passes native tests and awaits gameplay verification.

The first TAB from a building selects all its occupants; further TAB presses
cycle squad focus while retaining that selection, then return to building
control. Outside squadmates remain unselected.

The standalone `defiance.preview-weapon` plugin (enabled by default, needs only
core, independent of selection) makes both squad previews show the first
matching weapon instead of the last; it does not track
later held-weapon changes.

The default installed layout is:

```text
Game/
  bin/
    trm.exe
    dxgi.dll
    defiance-loader.ini        # bootstrap only: root, optional legacy plugins
  DefianceLoader/
    plugins/
      defiance_plugin_core.dll
      defiance_plugin_feature_*.dll
      <dll-stem>.plugin.json   # one sidecar manifest per DLL
    config/
      core.ini                 # [loader] wait/build policy, [logging] level
      infantry.ini             # selection, movement, posture, attack, garrison
      weapons.ini              # firing, ammunition, pickup
      diagnostics.ini
    logs/
      defiance-loader.log
```

`--game` and `DEFIANCE_GAME_DIR` still refer to `Game/bin`. `bin/defiance-loader.ini`
keeps only bootstrap settings: `root` (default `../DefianceLoader`, resolved
against `bin`) and the legacy unsectioned `plugins` override, which is also
resolved against `bin` exactly as before. Config, logs and plugin discovery all
hang off `root`. `tools/stage.py` resolves the same paths.

Each gameplay feature can be switched off in its group file, under its stable ID:

```ini
[defiance.movement]
enabled = false
```

Every setting is startup-only: edit, save, restart. A disabled feature's DLL is
not loaded, and the planner blocks anything that depends on it. The loader log
names each plugin's state and the reason it is inactive. Core is required
infrastructure, not a toggle. An existing `plugins = plugins` setting is kept as
an override, even though it used to be the generated default; to migrate, first
uninstall with its current setting, then stage again with the new default. Move
any third-party plugins separately. Uninstall leaves configuration, logs and
unrelated files in place.

Loader infrastructure can be developed and tested without the game DLLs:

    mise run loader-infra-test

This runs the core, API, loader, build-support and standalone plugin tests,
including pickup. It and `mise run test` first check that every Rust workspace is
formatted with default rustfmt; `mise run fmt` formats them. The [Rust pickup implementation](plugins/pickup/README.md) can be
built and verified with `mise run pickup-rust`; Rust is now the release default.
Firing controls and soldier selection methods also use Rust; run
`mise run rust-controls-test` for their assembly comparisons. See
[Core's public functions](docs/plugin-api.md) for shared game access.
`mise run loader` and `mise run loader-test` assemble fresh
payloads first and require local copies of the game DLLs. `loader-stage`
depends on that same build. Infrastructure tests also run in Windows CI.

## Known-good rollback point

`selection-v1` (commit `0470a61`) is the last build where the original ask all
works together: individual selection (plain, Shift, Ctrl, Ctrl+Shift), per-soldier
movement and the weapon pickup chooser, verified in game. Work after it (the
pose split and per-soldier posture) is experimental. To go back:

    git checkout selection-v1          # or: git switch -c from-v1 selection-v1
    ./.venv/Scripts/python.exe tools/build.py
    ./.venv/Scripts/python.exe tools/payload.py
    ./.venv/Scripts/python.exe tools/icon.py
    cargo build --release

Then:

    ./.venv/Scripts/python.exe tools/build.py          # assemble and patch
    ./.venv/Scripts/python.exe tools/verify.py         # what changed, and how it reads back
    ./.venv/Scripts/python.exe tools/test_chooser.py   # run both choosers on fake squads
    ./.venv/Scripts/python.exe tools/test_move.py      # run the move filter on fake vectors
    ./.venv/Scripts/python.exe tools/test_icon.py      # run the icon expansion on fake entities
    ./.venv/Scripts/python.exe tools/test_preview.py   # run the preview weapon guard on fake frames
    ./.venv/Scripts/python.exe tools/test_selection.py # real manager regression, facets and filter
    ./.venv/Scripts/python.exe tools/test_trace.py     # run the manager trace stubs natively
    ./.venv/Scripts/python.exe tools/test_pose.py      # run the pose split through its real entries
    ./.venv/Scripts/python.exe tools/test_posture.py   # run the posture gates on pinned soldiers
    ./.venv/Scripts/python.exe tools/test_firing.py    # run firing mode and the census on fake squads
    ./.venv/Scripts/python.exe tools/test_ammo.py      # run native ammo/menu/weapon-selection regressions
    pwsh -File tools/install.ps1                       # back up and install
    pwsh -File tools/install.ps1 -Revert               # restore the shipped DLL

Or patch a running game instead, leaving the install untouched:

    ./.venv/Scripts/python.exe tools/payload.py        # the logic.dll payload
    ./.venv/Scripts/python.exe tools/icon.py           # the game.dll payload
    cargo build --release
    ./target/release/defiance-pickup-inject --self-test
    ./target/release/defiance-pickup-inject            # then start the game
    ./target/release/defiance-pickup-inject --select-probe   # who asked for a selection
    ./target/release/defiance-pickup-inject --no-game  # leave out the selection-mode hooks
    ./target/release/defiance-pickup-inject --probe          # what the icon chain resolved
    ./target/release/defiance-pickup-inject --scan-check DIR # would another build's DLLs work?

Players need none of this: they double-click the exe and then start the game.
Its settings (which builds to patch, the game.dll half, the wait, whether the
window stays open) are in `defiance-pickup-inject.ini` beside it, written with
comments on the first run; command-line options override them for one run.


The tools, in the order they are useful:

    tools/pe.py             loading, addresses, strings, xrefs, disassembly
    tools/index.py          the cached cross-reference index (built from .pdata bounds)
    tools/rtti.py           MSVC RTTI: class name -> vtable -> methods
    tools/show.py           dump the function containing an address
    tools/ctx.py            one-line summary of a function: callers, callees, strings

`tools/pe.py` takes function bounds from `.pdata` rather than sweeping `.text`
linearly; a single sweep stops at the first padding byte and finds about a
thirteenth of the cross-references.

## Per-soldier weapon toggles

The current ammo patch applies an ammunition-panel toggle to the marked
soldiers when only part of a squad is selected. Select the whole squad to
change that slot for everyone. Changing one slot preserves the individual
settings of other slots. The panel considers only selected soldiers who can
use each weapon, including their individual overrides. Disabling a squad's
sole launcher therefore shows it disabled when the whole squad is selected.

The upper-right counter shows the number of selected soldiers who can use
that weapon, counting each soldier once. If their settings disagree, it shows
enabled/selected, such as `2/3`, and the ammunition count is highlighted in
amber. Clicking mixed enables all selected users; clicking again disables
them. Uniform states show the selected-user count, such as `3`, and restore
the native ammunition color. The original localized weapon tooltip is kept.
The fraction and selection-aware count pass native tests; their layout still
needs a live visual check.

The ammunition menu now hides slots that none of the selected squad members
can use. A grenadier sees his rifle and launcher; selecting him together with
a sniper shows the union of their weapons. Disabled weapons and weapons with
no ammunition stay visible so they can be re-enabled. Whole-squad selection
uses all selectable members. Non-squad panels keep their original behavior.
This visibility filter passes native tests and awaits a live-game check.

Disabling a loaded weapon returns its reservation through the native unload
method, allowing the weapon chooser to fall back to an enabled weapon. The
ammunition menu displays the effective flag and uses it for the next click.
Native regressions reproduce the old stuck-toggle and stuck-launcher bugs,
then check redraw, re-enable, rifle fallback, and reservation accounting.
Live-tested successfully: individual toggles, checkbox updates, rifle fallback,
and re-enabling work. The overrides cover slots 0-7, reset on save/load, and
are intended for single-player. Use the rebuilt injector for
the panel change, since the file patch only changes logic.dll.

Crash reports, dump analysis, and release symbols: [crash-reporting.md](docs/crash-reporting.md).

## Licence

The loader, the gameplay plugins, the injector and the tooling are licensed
under the [Mozilla Public License 2.0](LICENSE): you may use them in any
project, including closed-source ones, but changes to these files must be
published under the same licence.

The pieces a plugin builds against are licensed under your choice of the
[MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE) licence, so plugins can use
any licence: the plugin ABI (`crates/api`), the feature SDK
(`crates/feature-sdk`), the build support (`crates/build-support`), the example
plugin (`plugins/example`) and the service examples (`examples/`). Each crate's
`Cargo.toml` states its licence.

Terminator: Dark Fate - Defiance and its files belong to their owners. This
repository contains none of the game's files, only short byte signatures and
addresses used to recognise supported builds; the licences above cover only
this project's own code.
