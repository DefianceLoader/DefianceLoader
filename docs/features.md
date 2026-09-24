# Features

Each feature is a plugin with its own section in the files under
`DefianceLoader/config/`, created on first launch with every setting described.
Switch one off with `enabled = false` under its ID and restart the game; the log
names each plugin's state and why it is inactive. Features that build on
another (for example movement on selection) switch off with it.

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
| Drag a box | Select the individual soldiers inside it |
| Double-click | Select every soldier of the same squad type on screen |

With part of a squad selected, move, posture (stand, crouch, prone), attack,
building-entry and firing-mode (T) orders go only to the selected soldiers;
their squadmates stay put. Select the whole squad to order everyone.
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

## Regroup

Plugin: `defiance.regroup` (experimental, off by default). Form a new infantry
squad from the selected soldiers (Ctrl+Alt+R), or restore them (Ctrl+Alt+U).
Single-player only, outside buildings and vehicles. Details and limits:
[plugins/regroup](../plugins/regroup/README.md).

## Diagnostics

Plugin: `defiance.diagnostics`. Records extra information about selection and
squad behaviour for troubleshooting; no player controls. Safe to switch off.
