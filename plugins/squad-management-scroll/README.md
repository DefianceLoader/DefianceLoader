# Squad and vehicle inventory scrolling

Adds independent scrolling to the **out-of-mission infantry squad and vehicle
panels**. The original three-by-two weapon grid, six-card ammo row, vertical
upgrade column and five-card perk pick row keep their positions and sizes.
Four-pixel tracks occupy existing gaps above the weapons, below the ammunition,
under the perks, and in the band below the upgrade column. Scroll over a card or
the column with the mouse wheel, or drag the corresponding scrollbar. Each step
advances one item in the game's existing order. Controls disappear when the row
fits (six weapons or ammunition, five upgrades or perks). Changing squads or
vehicles resets every position; inventory changes clamp them to the remaining
items.

The infantry panel has four scrollable rows: weapons, ammunition, the upgrade
column (`upgrade_slot_*`), and the perk pick row. The vehicle panel has three:
weapons, ammunition, and the upgrade column. Both panels are supported per
build; the weapon and ammunition bindings are shared, and the upgrade column
binds through the script service's display resolver, exactly as the stock
refresh does. The upgrade list is a dynamic vector on the unit, so scrolling
never drops upgrades the game can hold. No fixed widget arrays are enlarged.

The upgrade scrollbar is a vertical track in the four-pixel gutter right of
the column. The game's slider lays out and drags only along x, so the plugin
hooks its thumb layout and maps the drag along y for a slider taller than wide;
every other slider keeps the stock behaviour.

Three settings join `perk_slots`: `upgrade_slots` (integer, 5-30, default 20)
is the upgrade column's length: the unit's upgrades, then empty cards up to that
count, five at a time. Scroll an empty card into view and drop an upgrade on it
to install more than five; the game still rejects or replaces a conflicting
upgrade. As an upgrade drag starts, the column scrolls to the card the drop will
land on: the installed upgrade it conflicts with (a shared `slot_type`), which
the drop replaces, or the one it duplicates, which the game refuses; otherwise
the first empty card. Five reproduces stock. `upgrades` and `vehicles` (bool,
default true) switch the upgrade columns and the vehicle panel on/off; turning
one off leaves the stock row and hides its scrollbar. All settings live under
`[defiance.squad-management-scroll]` in `DefianceLoader/config/weapons.ini`.

The `perk_slots` setting (integer, 5-20, default 5) bounds how many perk cards
the pick row exposes: five reproduces stock, a larger value scrolls through the
rest. The row is as long as the number of trainings the squad's type can take
(from the game's training table), up to `perk_slots`, from the start: the
squad's perks, then open (+) cards (locked ones where the game locks cards above
a squad's rank), the first highlighted when the squad can pick a perk now. A
squad owning five or more perks can still pick its next one. It grants no perks and does not change the
game's own perk or rank limits, which are data-driven. When the game hides the
perk cards (in the 2026-09 builds, every card after a perk tagged `exclusive`),
the row is left unscrolled; an `exclusive` perk past the fifth entry is not
detected.

The plugin rebinds the original widgets to real inventory records. It does not
increase fixed native widget arrays or change inventory/gameplay limits. The
in-mission ammunition menu is a separate feature.

## Install

Requires the current Defiance Loader (ABI 5). Close the game first.

1. Extract the package. Copy `DefianceLoader` and `mods` into the game root,
   beside its `bin` folder. Preserve your existing INI files.
2. The loader plugin is enabled by default. It creates its configuration on
   first launch. Existing settings are preserved: if you previously disabled
   it, set `enabled = true` under `[defiance.squad-management-scroll]` in
   `DefianceLoader/config/weapons.ini`.
3. Enable **Defiance squad inventory scrolling** in the game's MODS menu and
   restart as requested. Give it precedence over other mods that replace
   `scripts/ui/InfantryInfoPanel.txt` or `scripts/ui/VehicleInfoPanel.txt`.

No configuration is generated during the build. Default: enabled. If the
companion controls are missing or have the wrong native type, the plugin leaves
the stock display in place and writes a warning to the loader log.

The package derives its layout from the newest installed patch containing the
shared infantry and vehicle panels and uses it in every emitted overlay. It does
not merge other mods' panel changes. Do not distribute those extracted game UI
definitions as original project assets. Regenerate the add-on after game updates.
Unsupported game DLL hashes and modified hook sites are refused before writes.
Disable the plugin and UI mod together to uninstall; restart before removing
their files. Hot unloading is not supported.

The companion UI remains a mod-manager installation so its assets participate
in the game's normal mod priority and enable/disable controls. This makes
conflicts manageable; it does not merge two replacements of the same panel.
Removing that installation step would require another asset-loading mechanism
whose precedence and ability to preserve other UI mods would need validation.

### Replacement package after the September 21 crash

The initial package incorrectly restored the old `basis.pak` panel in its base
overlay. That definition lacks five upgrade widgets which the current game
dereferences during stock refresh. The corrected package uses the latest panel
in both overlays and requires all five widgets when packaging. Replace the
existing `mods/defiance_squad_scroll` files with the corrected ZIP while the
game is closed; preserve configuration. The plugin DLL is unchanged. Simply
disabling the plugin while leaving the old UI mod enabled does not fix this
asset mismatch.

## Build and validate

From the repository root, using `mise exec --`:

```
cargo test --manifest-path plugins/squad-management-scroll/Cargo.toml
cargo build --release --manifest-path plugins/squad-management-scroll/Cargo.toml
python tools/test_squad_scroll.py
python tools/test_package_squad_scroll.py
python tools/package_squad_scroll.py --game "C:\Games\GOG Galaxy\Terminator Dark Fate - Defiance"
```

The package is `out/defiance-squad-scroll.zip`. It adds this feature to an
existing loader installation; it does not replace the loader or other plugins.
Binding tables can be reproduced with `python tools/squad_scroll_bindings.py`
using the local GOG and Steam DLL copies. No installed files are edited by these
commands.

## Validation

Version 1.0.0 is non-experimental and enabled by default following the user's
successful in-game test with the seven-weapon squad on September 21, 2026.
The earlier UI-package crash is fixed. Automated viewport/native-fixture and
packaging tests remain part of validation.

For future changes, check a squad and a vehicle with at least seven weapons and
seven ammunition types, and with more than five upgrades, in both scroll
directions, dragging to each end, quantities/tooltips/actions on the last item,
changing squads/vehicles, shrinking inventory, reopening the panel, and
different UI scales. Confirm the weapon and ammunition scrollbars appear only
when their row overflows (seven entries), that the upgrade column is
`upgrade_slots` long with its vertical scrollbar in the gutter beside it
(dragging, track clicks and the wheel), that dropping an upgrade on an empty card
past five installs it, and that starting an upgrade drag scrolls to the card it
will land on (a conflicting or duplicate upgrade, else the first empty card). For the perk row, raise `perk_slots` above five and check a
squad with more than five perks (a rank-table edit), including wheel and slider,
the perk slider's placement under the row at different UI scales, and that
picking a perk still applies it. Check six-or-fewer items and confirm the other
panels remain unchanged. Do not treat fixture coverage as rendering or full game integration
coverage.
