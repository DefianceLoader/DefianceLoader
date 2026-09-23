# DefianceLoader - quick install

1. Extract the contents of this zip file to your base game directory.
2. `bin/dxgi.dll` should sit beside the game's `bin/trm.exe`.
3. `DefianceLoader` should sit in the base game folder, beside `bin`.
   On upgrades, replace the supplied DLLs/manifests but **keep existing `.ini`
   files** if you want to keep your settings.
4. Launch the game once however you usually launch it. The loader creates
   `bin/defiance-loader.ini` and the grouped INIs in `DefianceLoader/config/`.
   Each generated setting includes its description and default value.
   Close the game before editing settings, then restart to apply them.
5. Adjust any `.ini` file values you want to customize.
6. For squad-management scrolling, also extract the separate
   `defiance-squad-scroll-ui` download (its `mods` folder) into the base game
   folder, then enable the `Defiance squad inventory scrolling` mod in the
   in-game mod menu. Packages built with the game present already include it.

Packages contain no INI files. The loader preserves existing values and comments
and adds missing settings on later launches, including settings for disabled plugins.

General layout:

```text
Game/bin/trm.exe
Game/bin/dxgi.dll
Game/bin/defiance-crash-helper.exe
Game/DefianceLoader/plugins/...
Game/DefianceLoader/config/...
```

**Regroup is included but disabled by default.** After the first launch, edit
`DefianceLoader/config/infantry.ini` and add:

```ini
[defiance.regroup]
enabled = true
```

Restart after any config change. Regroup uses Ctrl+Alt+R; restore uses Ctrl+Alt+U.
Read `DefianceLoader/REGROUP.md` for its limitations and controls. Existing
explicit settings are preserved on upgrade.

**Troubleshooting:** check `DefianceLoader/logs/defiance-loader.log`.
If Windows reports a missing DXGI entry point before a log appears, this loader
build requires exports unavailable on your Windows installation. Remove the
installed `bin/dxgi.dll` to restore normal game startup; do not replace Windows'
system DLLs. The proxy uses the real system DLL as a load-time dependency.
Crash reports are saved under `DefianceLoader/logs/crashes`.

**Uninstall:** remove the `bin/dxgi.dll` installed
from this package and `bin/defiance-crash-helper.exe`. Your config files can stay for later use.
