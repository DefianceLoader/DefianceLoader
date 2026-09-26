# Expanded ammunition menu (experimental)

An independent, opt-in plugin that keeps **three rows** and adds columns to the
right. Default: 12 columns / 36 ammo slots, replacing the game's nine-slot limit.
This is a bounded expansion, not an unlimited or scrolling menu. Configure 3–42
columns (9–126 slots); records beyond the configured capacity remain hidden.
Large column counts can extend off-screen, depending on resolution, UI scale,
and other UI mods. The stock widget spacing/visuals are retained. Visible cards pack together in
three rows, filling upward then adding columns to the right. Hidden ammo entries
do not leave holes after regroup/restore or when switching squads.

## Install and enable

Use a current DefianceLoader release with ammunition and selection plugins enabled.
This ZIP includes matching Core, regroup and expanded-menu DLLs and sidecars.
Close the game completely. Copy the DLLs and sidecars into
`Game/DefianceLoader/plugins/`. Keep your existing configuration files.

The sidecar defaults to **enabled**. The generated
`DefianceLoader/config/weapons.ini` carries this section:

```ini
[defiance.expanded-ammo-menu]
enabled = true
columns = 12
```

Restart after changing the column count. Never inject this into an already
running battle, hot reload it, or unload its patches while menus exist: it
changes the game's in-memory menu object layout. To disable, exit the game,
set `enabled = false`, and restart.

The known GOG and Steam December 2025 `game.dll` builds are supported. Unknown or
modified on-disk DLLs and mismatching live patch bytes are refused. The normal
loader log records the chosen capacity or refusal.

## Behavior and limits

Ammo indexes stay unchanged: clicking slot ten still targets ammo record ten.
Only display positions are compacted; original indexes are preserved. The
plugin does not change ammunition
simulation, resupply, or quantities. Whole-squad controls work through the
existing ammunition plugin.

**Individual-soldier overrides still support only the first eight slots.**
With only part of a squad selected, toggling later slots cannot set a per-soldier
override and may have no effect. Select the whole squad to change those slots.
Expanding per-soldier pin storage is a separate change, not included here.

This has native automated coverage but has **not been live-tested in a battle**.
Check a squad with more than nine types: displayed counts/icons, tooltips,
clicks on slots 10+, switching to a smaller squad, resupply, battle reload, and
normal exit. Check your resolution/UI scaling for overlap or clipping. Use the
loader's crash reports if anything fails.

## Build and test

From the repository root:

```powershell
mise run loader
mise exec -- cargo test --manifest-path plugins/expanded-ammo-menu/Cargo.toml
mise exec -- cargo build --release --manifest-path plugins/expanded-ammo-menu/Cargo.toml
mise exec -- python tools/package_expanded_ammo_menu.py
```

The menu remains a standalone workspace; mise run loader builds matching Core,
regroup and manifests for the package. Output: `out/expanded-ammo-regroup.zip` and a separate symbols ZIP.

To re-audit the sites and run the native game-code tests, provide the repository's
local `bin/gog/2025-12-23/game.dll` and `bin/steam/2025-12-23/game.dll` and the Python dependencies
from `mise run setup`, then run:

```powershell
mise exec -- python tools/ammo_menu_sites.py
mise exec -- python tools/test_expanded_ammo_menu.py
```

No test starts the game or changes an installed game file. The tests load game
code into disposable processes and use fabricated UI objects.

## Regroup integration

See plugins/regroup/README.md. Set max_weapon_types = 0 in the
[defiance.regroup] section of DefianceLoader/config/infantry.ini to use the
installed menu capacity automatically. max_soldiers defaults to 16 and accepts
1..64; larger squads remain experimental. Both optional plugins default disabled.

Core owns the process-wide installed capacity. This plugin requires Core's
ammo-menu service v1 and publishes its capacity only after all patches succeed.
Old Core DLLs are refused before patching. Regroup reads the service when used,
so there is no cross-config lookup or initialization-order dependency.


## Compact-layout update

The standalone ammo-slot-layout-fix.zip contains only this plugin's DLL,
manifest and these instructions. With the game closed, extract it into the game
directory so the two plugin files land under DefianceLoader/plugins. Keep your
existing Core, regroup and configuration files. The startup log now includes
"compact visible slots". A full restart is required.

The compaction runs after the native menu refresh, moving each card root by a relative offset. Native movement propagates that
offset to its children, including hidden progress bars. Unchanged frames do not
move cards again. Ammo records, selection
indexes and quantities stay unchanged. Native regression cases cover empty,
sparse, full and shrinking layouts, plus click indices after moving cards.


The first compact-layout release incorrectly called constructor positioning on
every widget on every frame. Native movement is additive, so this caused drift
off-screen and damaged child offsets. The corrected build moves only the root,
by the difference between its old and new grid positions. Regression tests now
use additive movement, real child lists and repeated frames.


## All selected infantry squads

This is enabled by default: the generated
DefianceLoader/config/weapons.ini has all_selected_squads = true under
[defiance.expanded-ammo-menu]. Set it to false and restart to keep the
focused-squad view.

With two or more selected infantry squads, the menu shows the union of their
ammunition types, including squads that are not focused. Matching types share
one card with summed capacity, rounds and carrier counts. Types no longer used
by any selected squad do not consume a visible card. One selected squad uses
the ordinary focused view.

Cards show the selected users of each type. When only some allow it, the card
shows the single-squad enabled/total count (for example, 2/5). Clicking a mixed or fully disabled type enables it for all selected
users; clicking a fully enabled type disables it. Each squad's own ammo
index is looked up by type identity. Partial-squad selections retain the
ammunition plugin's selected-soldier rules (including its eight-slot pin limit).
A selection change invalidates old card clicks before any toggle is applied.
Selected vehicles join the view: each is one user of its ammunition types,
enabled or disabled as a whole (vehicles have no per-soldier pins), and a
click toggles it with the squads. A selected building without guns of its
own does not contribute.

This view is experimental and intended for single-player use. It does not emit
the native per-unit replay/recording commands; replay/network synchronization is
not supported. The ammo-level bar shows summed rounds divided by summed
capacity. The reload bar shows the ready share of the selected users of that
type, as the single-squad panel does: each enabled user counts 1 when ready or
his native reload progress while reloading, a disabled user 0, divided by the
users. It is full when all are enabled and ready, and is a group indicator, not
a prediction of when every soldier will finish reloading.

The display limit is columns * 3: 36 types by default, up to 126. Extra types
are not displayed. This is independent of regroup's temporary 20-soldier
destination ceiling; selecting several squads does not merge their soldiers.

## Mission-load crash correction (0.2.1)

The 0.2.0 all-selected-squads lookup reversed two AmmunitionMenu fields:
+0x128 is the LogicHybridServer context, while +0x130 is the LogicUtils world
facade. The first render called the facade's unrelated +0x40 method as a
player getter, causing the supplied mission-load crash. Version 0.2.1 uses the
correct fields for rendering and click validation. Missing context, world,
player or manager falls back to the ordinary focused display.

Regression coverage executes the supported game's constructor service-field
initialization and LogicHybridServer's native player getter. It detects calling
the world utility as a player context and rejects the previous DLL. The tests
require bin/gog/2025-12-23/logic.dll and bin/steam/2025-12-23/logic.dll as well as the game DLLs.
Tests also cover missing context/world/player during render and between render
and click. Game-code fixtures still do not replace a live mission test.

The incremental ammo-menu-selected-squads-crash-fix.zip updates only the
expanded-menu DLL and manifest. Keep your other plugins and configuration.
The startup log includes "menu context fix v3".
