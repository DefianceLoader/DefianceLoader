# Unit inspection

Shows the full unit info panel in missions for squads you do not own. Click an
allied, neutral or enemy squad and the panel shows its commander, soldier
count, rank and experience, and the ammo menu lists its weapons and ammunition
with their counts, as for your own squads. The game's reduced panel shows only
the squad's name, its relation and a weapon-type icon.

Settings, under `[defiance.unit-inspection]` in `DefianceLoader/config/infantry.ini`:

- `show_allied`, `show_neutral`, `show_enemy` (all on): whose details show.
  Abandoned squads count as neutral.
- `ally_weapon_toggles` (off): let the ammo menu's weapon toggles work on
  allied squads, to tell an ally which weapons to use; the ally's AI keeps
  them. Enemy and neutral squads' toggles never act.
- `own_colour` (`teal`), `allied_colour` (`yellow`), `neutral_colour`
  (`grey-blue`), `enemy_colour` (`red`): the reload bar colours on the ammo
  cards. Each is `teal` (the game's), `green`, `yellow`, `grey-blue`, `red`,
  or `#RRGGBB`.

With a squad's details shown, the relation label (ENEMY, NEUTRAL, ALLIED) is
hidden, since the commander's name takes its place.

The ammo cards' reload bars take the shown squad's colour: yours teal as
before, allied yellow, neutral grey-blue, enemy red. This needs the companion
mod "Defiance unit inspection colours" (`mods/defiance_unit_inspection`,
installed with the loader package) enabled in the game's MODS menu: it makes
the bars greyscale so they can take a colour. Without it the bars keep the
game's teal; with the mod but not the plugin they stay grey.

Limits: squads only; vehicles and buildings keep the game's panel. Supported on
the 2026 game updates (GOG and Steam); on older builds the plugin logs that it
does not apply and changes nothing. It blocks online multiplayer while active,
like the other gameplay plugins.
