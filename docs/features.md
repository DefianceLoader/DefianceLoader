# Features

Each feature is a plugin with its own section in the files under
`DefianceLoader/config/`, created on first launch with every setting described.
Switch one off with `enabled = false` under its ID and restart the game; the log
names each plugin's state and why it is inactive. Features that build on
another (for example movement on selection) switch off with it.

To switch features without restarting, set `live_toggle = true` under
`[loader]` in `core.ini` and restart once. From then on, saving a changed
`enabled`, or any other setting of a feature, takes effect at the main menu,
or as the next mission starts or save loads: the feature is switched, or
loaded again with its new values. Core, the performance tuning, the expanded
ammo menu, squad scrolling, unit inspection, vehicle arrival braking, primary weapon drops, moving grenades
and the movement animation overlay still need a restart. A feature built into
Core that was off at startup comes on only after a restart; a separate plugin
DLL can be switched on.

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
A squad ordered into a vehicle with too few free seats fills the seats left
and the rest stay beside it; in the base game none of them board.
Building-panel and vehicle unload orders use only the occupants or passengers;
their squadmates outside stay put. The first TAB
from a building selects all its occupants; further presses cycle squad focus
and then return to building control. Hold Ctrl with TAB (`squad_tab_modifier`
in `infantry.ini`: `ctrl`, `shift` or `off`) to select one squad's occupants at
a time instead. Clicking a squad icon on the building panel selects only that
squad's soldiers inside. With part of a squad selected, the unit panel's 3D
squad preview shows the unselected soldiers darker; this uses darker copies of
the game's materials, which the squad scrolling companion mod carries.

## Soldier move markers

Plugin: `defiance.cover-markers` (on by default; GOG 2026-09-14,
2026-09-25 and 2026-10-07, and Steam 2026-09-22, 2026-09-25 and 2026-10-07). While you aim a move order,
each selected soldier gets a small arrow at the position the game's formation
gives them, in place of the large squad arrow; dragging to set facing rotates
them. After the order, each arrow marks that soldier's destination and clears
when the soldier arrives or settles in cover. Vehicles keep the game's arrows.
Cover selection can move a soldier away from their formation point, so an
arrow may not match the final cover position exactly. To turn it off, set
`enabled = false` in `[defiance.cover-markers]` in `infantry.ini` and restart
the game. On other builds it logs a warning and stays inactive. Details:
[plugins/cover-markers](../plugins/cover-markers/README.md).

## Per-soldier weapons

Plugin: `defiance.ammunition`.

With part of a squad selected, an ammo-panel toggle applies only to those
soldiers; select the whole squad to change everyone. The panel shows only the
weapons the selected soldiers can use, and the counter shows how many of them
can use each. When their settings differ it shows enabled/selected (such as
`2/3`); clicking enables all of them, clicking again disables them. The reload
bar shows how many of them are ready: full when all are enabled and loaded,
two thirds full with two of three enabled, and lower while one reloads. A
single soldier or a vehicle keeps the game's own reload bar. The
companion UI mod right-aligns the counter so two-digit fractions fit. Disabling
a loaded weapon unloads it, so the soldier falls back to an enabled one. A unit
or soldier with every usable weapon disabled takes no part in an attack order,
so it does not fire the round already chambered. Hold Ctrl and turn the mouse
wheel over a card to enable one more soldier (wheel up) or disable one (wheel
down), in roster order, among the soldiers the card counts; the camera does not
zoom while you do. `step_modifier` in `weapons.ini` picks `ctrl`, `shift`,
`alt` or `none`. With `step_click = true`, Ctrl+left-click on a card enables
one and Ctrl+right-click disables one; with `none`, a right-click alone
disables one and a left-click keeps its usual toggle. With expanded-ammo-menu's
all_selected_squads enabled, steps cover selected squads in selection order,
then each squad's roster order; otherwise one squad (or part of one) must be
selected for step controls. Individual overrides cover the first eight local ammo slots, reset
on save/load, and are meant
for single-player.

## Special weapons from passenger vehicles

Plugin: `defiance.vehicle-special-fire` (off by default; set `enabled = true`
in `[defiance.vehicle-special-fire]` in `weapons.ini`). Let a passenger's automatic fire use
special weapons, including sniper rifles, heavy guns, RPGs, and ATGMs, in
vehicles with passenger firing mounts. Passengers carrying special weapons
get mount priority. Mounted guns acquire targets independently, and specials
can exchange firing positions with ordinary weapons as targets change sides.
Select the vehicle and use Attack to order its passengers against an enemy;
Stop restores automatic targeting. Native range, ammunition, deployment,
reload, and aiming checks still apply. Reboard passengers after enabling the
feature or loading older saved bindings.

## Expanded ammo menu

Plugin: `defiance.expanded-ammo-menu` (experimental). More than nine ammunition
entries in the in-mission menu, by adding columns (`columns`, default 12). With
`all_selected_squads`, the menu combines the ammunition of every selected squad.
Details: [plugins/expanded-ammo-menu](../plugins/expanded-ammo-menu/README.md).

## Squad management scrolling

Plugin: `defiance.squad-management-scroll`. In the army presets and squad
management screens, the weapon, ammunition, perk and upgrade rows of infantry
and vehicles scroll with the mouse wheel or their scrollbars, keeping the
original card sizes. The available-training chooser uses a single horizontal
row with a scrollbar so extra cards cannot cover its training buttons. The upgrade column continues past five with empty cards
(`upgrade_slots`, default 20): scroll one into view and drop an upgrade on it to
install more than five; by default the column stops at the most upgrades the
unit's type can hold at once (`fit_upgrades`). Starting an upgrade drag scrolls to the card it will
land on. It needs the separate `defiance-squad-scroll-ui` download, enabled in
the game's mod menu. Details:
[plugins/squad-management-scroll](../plugins/squad-management-scroll/README.md).

## Weapon pickup

Plugin: `defiance.pickup`. When picking up a weapon, the selected soldiers are
preferred, and repeated pickups rotate between the eligible soldiers.

## Moving infantry actions

Plugins: `defiance.moving-actions`, `defiance.moving-actions-animation`,
`defiance.moving-actions-sync`, `defiance.moving-actions-render-sync`, and
`defiance.moving-grenades` (experimental, off by default; each is enabled in
its own section of `infantry.ini`; supported on GOG 2026-09-14,
2026-09-25 and 2026-10-07, and Steam 2026-09-22, 2026-09-25 and 2026-10-07). The movement plugin preserves
movement speed through grenade throws and weapon changes. Companion plugins
repair missed weapon handoffs and model/attachment mismatches; the animation
plugin overlays locomotion leg poses during standing throws and weapon changes.
Grenade continuation retains a valid interrupted route after positional smoke
throws, while explicit Stop and new orders cancel it. A brief native grenade
preparation pause can remain. The animation overlay is experimental and can
still show transition or gait-cadence artifacts.

The animation plugin uses generated data under
`mods/defiance_moving_actions/assets`. Install the companion data with the
companion UI download and restart the game after changes. See
[building from source](building.md) for the PAK-derived asset workflow.

## Primary weapon drops

Plugin: `defiance.weapon-drops` (experimental, off by default; GOG and
Steam 2026-09 and 2026-10-07 builds only). A wiped infantry squad drops its primary
weapons, and collecting a primary re-equips the whole squad with it. To try
it, set `enabled = true` in `[defiance.weapon-drops]` in `infantry.ini` and
restart the game; on other game builds it stays inactive. The `ammo_policy` and
`death_ammo` settings choose what happens to ammunition. Keep copies of your
saves: with `ammo_policy = retain`, a save needs the plugin to load correctly.
Details: [plugins/weapon-drops](../plugins/weapon-drops/README.md).

## Squad previews

Plugin: `defiance.preview-weapon`. Squad previews, in mission and in squad
management, show the first matching weapon instead of the last. It does not
follow later changes to the weapon a soldier holds.

## Unit inspection

Plugin: `defiance.unit-inspection`. Click a squad you do not own to see its
full details: commander, soldier count, rank and experience, and its weapons
and ammunition in the ammo menu; for vehicles and platforms, their weapons and
ammunition. Separate settings for allied, neutral and enemy units; `ally_weapon_toggles` lets the ammo menu's toggles direct an
ally's weapons. The ammo cards' reload bars show the squad's relation by
colour (yours teal, allied yellow, neutral grey-blue, enemy red; each
configurable), with its companion mod, in the `defiance-squad-scroll-ui`
download, enabled in MODS. On the 2026 game updates. Details:
[plugins/unit-inspection](../plugins/unit-inspection/README.md).

## Ability groups

Plugin: `defiance.ability-groups` (experimental). When the selected squads have
two abilities of one ability button (mines and C4 on F2, for example), the game
empties the button; with this plugin it shows one of them, and pressing it opens
a row of all of them over the order panel, picked with the order keys or the
mouse. Details: [plugins/ability-groups](../plugins/ability-groups/README.md).

## Legion vehicle hacking

Plugin: `defiance.legion-vehicle-hacking` (off by default; set
`enabled = true` in `[defiance.legion-vehicle-hacking]` in `vehicles.ini`).
Allows hacking Legion vehicles while
they are repairing themselves. Other hacking target checks remain in place.
Details: [plugins/legion-vehicle-hacking](../plugins/legion-vehicle-hacking/README.md).

## Vehicle arrival braking

Plugin: `defiance.vehicle-arrival` (experimental, off by default). It shortens
the braking window for wheeled and tracked cars and tanks as they approach a
destination. To try it, set `enabled = true` in
`[defiance.vehicle-arrival]` in `vehicles.ini` and restart the game. The
`braking_window_percent` setting defaults to `50`; values from `50` through
`100` are supported, and `100` leaves the game's normal braking behavior.
In-game stopping accuracy still needs testing. Details:
[plugins/vehicle-arrival](../plugins/vehicle-arrival/README.md).

## Regroup

Plugin: `defiance.regroup` (experimental, off by default). Form a new infantry
squad from the selected soldiers (Ctrl+Alt+R), or restore them (Ctrl+Alt+U).
Single-player only, outside buildings and vehicles. Details and limits:
[plugins/regroup](../plugins/regroup/README.md).

## Performance

Plugin: `defiance.performance`, under `[defiance.performance]` in
`DefianceLoader/config/core.ini`; `enabled = false` there leaves every one of
these to the game, and changes here take effect after a restart. Settings
written under `[loader]` by an earlier version still apply until the loader
moves them, and the log says so.

The game runs its rendering and simulation on one thread and keeps it on
the first CPU, which often also handles the graphics card's interrupts. The
plugin lets that thread use every CPU instead, which gives it more of its time
on CPU-bound scenes. `main_thread_cpus = engine` restores the game's own choice; `spread` (every CPU
but the first core) is experimental and caused long stutters in testing.

It also sorts grass by distance with each distance worked out once per
frame, where the game works each out many times, in about half the time; the
order is the same as the game's. `grass_sort = engine` uses the game's own
sort.

It redraws the shadow map one of its distance bands a frame, in turn,
instead of all of them every frame, and the farthest band, which costs the
most, half as often as the others. On a busy scene with high shadows this took
the frame rate from about 31 to over 50 fps in testing. Each band is drawn with
the view it keeps until its next turn, so shadows stay in place; in a band not
redrawn this frame, a moving unit's shadow can trail it by a few frames.
`shadow_cascades = rotate` redraws every band equally often,
`near` redraws the nearest band every frame and the others in turn, and `all`
is the game's own.
Fitting each band to what it covers, the game also walks every shadow caster
in view and then throws the result away; the plugin skips that walk, which leaves
the shadows the same (`shadow_fit = engine` keeps it).

With shadows rotated, the main thread's biggest cost is preparing the objects
in view each frame. The plugin puts those objects, and the meshes it groups for
drawing, in order with what the order depends on read once per item, and
inverts the matrices of moving objects with the game's own arithmetic without
its many small calls; together that took about 3 ms off each frame in testing
(51 to 58 fps). All give exactly the game's results. `view_sort = engine`,
`mesh_sort = engine` and `matrix_inverse = engine` use the game’s own.

On the six supported game builds, `tree_sway = half` updates
living-tree wind sway every other tree-manager update while leaving the rest of
the tree callback at its usual rate. The next sway update receives the sum of
the skipped and current manager time steps. Trees keep moving, but skipped
oscillator samples can delay changes in sway speed and make motion less smooth.
At one busy campaign spot, the user observed roughly 70 to 80 fps with the
earlier doubled-current-step implementation. A later diagnostic using summed
steps measured about 80 gameplay-view calls/s at the saved spot, and the user
found its tree motion acceptable.
`tree_sway = half_facet` also omits the active tree facet's provider and
transform refresh on each skipped sway tick. Its completion and release path
continues to run. At the saved campaign hotspot, one complete ON window gave
about 83 gameplay-view calls/s versus about 80 before and after, and visible
trees looked normal. That gain is provisional; deferred provider updates may
have effects beyond tree motion. This is an opt-in setting, checked in game
only on Steam 2026-09-25.
`tree_sway = engine` is the default. The plugin finds the tree functions by signature
and checks their entry bytes; a game build where any of them is not found keeps
the engine rate and is not patched. Changing this setting requires a restart.
The other five supported builds resolve the same functions but have not yet
been checked in game.

The rendering optimizations leave gameplay unchanged and are fine in
multiplayer. The optional tree sway setting has been checked in single-player.

## Diagnostics

Plugin: `defiance.diagnostics`. Captures configured selection and behaviour
events for troubleshooting; it adds no player controls. The plugin is enabled
by default, but every probe in the sample configuration is off. Configure
probes in `DefianceLoader/config/diagnostics-probes.json`; edits to a valid
file take effect while the game is running. See [Diagnostics probes](diagnostics.md).

The plugin does not install patch units. The generated diagnostics units remain
available to the standalone injector's patching and `--select-probe` workflow.
