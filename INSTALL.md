# DefianceLoader

Gameplay plugins for *Terminator: Dark Fate - Defiance*. Feature guide:
https://github.com/DefianceLoader/DefianceLoader/blob/main/docs/features.md

## Install

1. Extract this zip into the game folder (the one containing `bin`), so that
   `bin/dxgi.dll` sits beside `bin/trm.exe` and `DefianceLoader` sits beside
   `bin`.
2. For squad-management scrolling, also extract the `defiance-squad-scroll-ui`
   download into the game folder and enable **Defiance squad inventory
   scrolling** in the game's mod menu.
3. Start the game as usual.

When updating, overwrite the files but **keep existing `.ini` files** to keep
your settings.

## Settings

The first launch creates `DefianceLoader/config/*.ini`, with every setting
described. Close the game before editing and restart it afterwards. Any
feature can be turned off with `enabled = false` in its section.

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
