# Features

Each feature is a plugin with its own section in the files under
`DefianceLoader/config/`, created on first launch with every setting described.
Switch one off with `enabled = false` under its ID and restart the game; the log
names each plugin's state and why it is inactive. Features that build on
another (for example movement on selection) switch off with it.

To switch features without restarting, set `live_toggle = true` under
`[loader]` in `core.ini` and restart once. From then on, saving a changed
`enabled` takes effect at the main menu, or as the next mission starts or save
loads. Core, the expanded ammo menu, squad scrolling and unit inspection
still need a restart, and a feature that was off at startup comes on only
after a restart.

The gameplay features are single-player only: they change the game's simulation
and send nothing to other players. While any of them is active the game will not
go online; the Multiplayer menu shows an error instead, and the log lists the
plugins to disable. Skirmish and the campaign are unaffected. The loader starts
no gameplay plugin unless this guard is in place. It keeps honest players out of
desynced games; it is not anticheat, and a player who modifies the files can
remove it.

## Individual soldier control

Plugins: `defiance.selection`, `defiance.movement`, `defiance.posture`,
`defiance.attack`, `defiance.garrison`, `defiance.firing`.

| Gesture | Effect |
| --- | --- |
| Click a soldier or squad icon | Select the whole squad |
| Shift-click | Add or remove the whole squad |
| Ctrl-click a soldier | Select only that soldier |
| Ctrl+Shift-click a soldier | Add or remove that soldier, within or across squads |
| Drag a box | Select every squad with a soldier or its icon inside, as in the base game |
| Ctrl-drag a box | Select only the soldiers inside it |
| Double-click | Select every soldier of the same squad type on screen |

Set `marquee = soldiers` under `[defiance.selection]` in `infantry.ini` to make
a plain drag select the soldiers inside, as Ctrl-drag does.

With part of a squad selected, move, posture (stand, crouch, prone), attack,
building-entry and firing-mode (T) orders go only to the selected soldiers;
their squadmates stay put, and squadmates lying prone stay down while the rest
enter a building. Select the whole squad to order everyone.
Building-panel exit orders use only that building's occupants. The first TAB
from a building selects all its occupants; further presses cycle squad focus
and then return to building control. Hold Ctrl with TAB (`squad_tab_modifier`
in `infantry.ini`: `ctrl`, `shift` or `off`) to select one squad's occupants at
a time instead. Clicking a squad icon on the building panel selects only that
squad's soldiers inside. With part of a squad selected, the unit panel's 3D
squad preview shows the unselected soldiers darker; this uses darker copies of
the game's materials, which the squad scrolling companion mod carries.

## Per-soldier weapons

Plugin: `defiance.ammunition`.

With part of a squad selected, an ammo-panel toggle applies only to those
soldiers; select the whole squad to change everyone. The panel shows only the
weapons the selected soldiers can use, and the counter shows how many of them
can use each. When their settings differ it shows enabled/selected (such as
`2/3`); clicking enables all of them, clicking again disables them. The reload
bar shows how many of them are ready: full when all are enabled and loaded,
two thirds full with two of three enabled, and lower while one reloads. The
companion UI mod right-aligns the counter so two-digit fractions fit. Disabling
a loaded weapon unloads it, so the soldier falls back to an enabled one. A unit
or soldier with every usable weapon disabled takes no part in an attack order,
so it does not fire the round already chambered. Individual overrides
cover the first eight ammo slots, reset
on save/load, and are meant for single-player.

## Expanded ammo menu

Plugin: `defiance.expanded-ammo-menu` (experimental). More than nine ammunition
entries in the in-mission menu, by adding columns (`columns`, default 12). With
`all_selected_squads`, the menu combines the ammunition of every selected squad.
Details: [plugins/expanded-ammo-menu](../plugins/expanded-ammo-menu/README.md).

## Squad management scrolling

Plugin: `defiance.squad-management-scroll`. In the army presets and squad
management screens, the weapon, ammunition, perk and upgrade rows of infantry
and vehicles scroll with the mouse wheel or their scrollbars, keeping the
original card sizes. The upgrade column continues past five with empty cards
(`upgrade_slots`, default 20): scroll one into view and drop an upgrade on it to
install more than five. Starting an upgrade drag scrolls to the card it will
land on. It needs the separate `defiance-squad-scroll-ui` download, enabled in
the game's mod menu. Details:
[plugins/squad-management-scroll](../plugins/squad-management-scroll/README.md).

## Weapon pickup

Plugin: `defiance.pickup`. When picking up a weapon, the selected soldiers are
preferred, and repeated pickups rotate between the eligible soldiers.

## Squad previews

Plugin: `defiance.preview-weapon`. Squad previews, in mission and in squad
management, show the first matching weapon instead of the last. It does not
follow later changes to the weapon a soldier holds.

## Unit inspection

Plugin: `defiance.unit-inspection`. Click a squad you do not own to see its
full details: commander, soldier count, rank and experience, and its weapons
and ammunition in the ammo menu. Separate settings for allied, neutral and
enemy squads; `ally_weapon_toggles` lets the ammo menu's toggles direct an
ally's weapons. The ammo cards' reload bars show the squad's relation by
colour (yours teal, allied yellow, neutral grey-blue, enemy red; each
configurable), with its companion mod, in the `defiance-squad-scroll-ui`
download, enabled in MODS. Squads only, on the 2026 game updates. Details:
[plugins/unit-inspection](../plugins/unit-inspection/README.md).

## Regroup

Plugin: `defiance.regroup` (experimental, off by default). Form a new infantry
squad from the selected soldiers (Ctrl+Alt+R), or restore them (Ctrl+Alt+U).
Single-player only, outside buildings and vehicles. Details and limits:
[plugins/regroup](../plugins/regroup/README.md).

## Performance

Core. The game runs its rendering and simulation on one thread and keeps it on
the first CPU, which often also handles the graphics card's interrupts. Core
lets that thread use every CPU instead, which gives it more of its time on
CPU-bound scenes. Under `[loader]` in `DefianceLoader/config/core.ini`,
`main_thread_cpus = engine` restores the game's own choice; `spread` (every CPU
but the first core) is experimental and caused long stutters in testing.

Core also sorts grass by distance with each distance worked out once per
frame, where the game works each out many times, in about half the time; the
order is the same as the game's. `grass_sort = engine` under `[loader]` uses
the game's own sort.

Core redraws the shadow map one of its distance bands a frame, in turn,
instead of all of them every frame, and the farthest band, which costs the
most, half as often as the others. On a busy scene with high shadows this took
the frame rate from about 31 to over 50 fps in testing. Each band is drawn with
the view it keeps until its next turn, so shadows stay in place; in a band not
redrawn this frame, a moving unit's shadow can trail it by a few frames.
Under `[loader]`, `shadow_cascades = rotate` redraws every band equally often,
`near` redraws the nearest band every frame and the others in turn, and `all`
is the game's own.
Fitting each band to what it covers, the game also walks every shadow caster
in view and then throws the result away; Core skips that walk, which leaves
the shadows the same (`shadow_fit = engine` keeps it).

With shadows rotated, the main thread's biggest cost is preparing the objects
in view each frame. Core puts those objects, and the meshes it groups for
drawing, in order with what the order depends on read once per item, and
inverts the matrices of moving objects with the game's own arithmetic without
its many small calls; together that took about 3 ms off each frame in testing
(51 to 58 fps). All give exactly the game's results. `view_sort = engine`,
`mesh_sort = engine` and `matrix_inverse = engine` under `[loader]` use the
game's own.

None of these changes gameplay, and all are fine in multiplayer.

## Diagnostics

Plugin: `defiance.diagnostics`. Records extra information about selection and
squad behaviour for troubleshooting; no player controls. Safe to switch off.
