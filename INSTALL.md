# DefianceLoader

Gameplay plugins for *Terminator: Dark Fate - Defiance*. Feature guide:
https://github.com/DefianceLoader/DefianceLoader/blob/main/docs/features.md

## Install

1. Extract this zip into the game folder (the one containing `bin`), so that
   `bin/dxgi.dll` sits beside `bin/trm.exe` and `DefianceLoader` sits beside
   `bin`.
2. For squad-management scrolling, also extract the `defiance-squad-scroll-ui`
   download into the game folder and enable **Defiance squad inventory
   scrolling** in the game's mod menu. The same download holds **Defiance unit
   inspection colours**; enable it too for relation-coloured reload bars on
   the ammo cards.
   The companion download also includes generated animation assets for the
   experimental moving-actions animation plugin, under
   `mods/defiance_moving_actions`.
3. Start the game as usual.

When updating, overwrite the files but **keep existing `.ini` files** to keep
your settings.

## Settings

The first launch creates `DefianceLoader/config/*.ini`, with every setting
described. Close the game before editing and restart it afterwards. Any
feature can be turned off with `enabled = false` in its section.

These experimental features are included but off by default. To try one, set
`enabled = true` in its section and restart the game:

| Feature | File | Section |
| --- | --- | --- |
| Moving infantry actions | `infantry.ini` | `[defiance.moving-actions]` |
| Special weapons from passenger vehicles | `weapons.ini` | `[defiance.vehicle-special-fire]` |
| Legion vehicle hacking | `vehicles.ini` | `[defiance.legion-vehicle-hacking]` |
| Vehicle arrival braking | `vehicles.ini` | `[defiance.vehicle-arrival]` |
| Weapon drops | `infantry.ini` | `[defiance.weapon-drops]` |

Moving actions runs on four supported GOG and Steam September 2026 builds. Its
companions (`defiance.moving-actions-animation`, `defiance.moving-actions-sync`,
`defiance.moving-actions-render-sync` and `defiance.moving-grenades`, all in
`infantry.ini`) are off by default too; turn on the ones you want alongside it.
The animation companion needs the generated `mods/defiance_moving_actions`
data. See [features and controls](docs/features.md) for details.

**Regroup is included but disabled by default.** It is experimental and
single-player only. To try it, add to `DefianceLoader/config/infantry.ini`:

```ini
[defiance.regroup]
enabled = true
```

Then Ctrl+Alt+R forms a squad from the selected soldiers and Ctrl+Alt+U
restores them to their original squads, within the same session only (it cannot
restore after loading a save). Keep a save from before you tried it.

## Troubleshooting

- Check `DefianceLoader/logs/defiance-loader.log`. It lists each feature and,
  if one is inactive, why. On a game version it does not recognise, the loader
  changes nothing.
- If the game will not start, remove `bin/dxgi.dll` to play without the loader,
  and report the problem with the log.
- Crash reports are saved in `DefianceLoader/logs/crashes`. When reporting a
  crash, send that crash's files and `defiance-loader.log`, what you were
  doing, and your game store and version. The `.dmp` file can contain personal
  data from memory, so share it privately.

## Uninstall

Delete `bin/dxgi.dll`, `bin/defiance-crash-helper.exe` and
`bin/defiance-loader.ini`. The `DefianceLoader` folder can stay (it holds your
settings) or be deleted.
