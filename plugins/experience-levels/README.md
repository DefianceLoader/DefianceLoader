# Experience levels

Experimental plugin that lets a squad have more than the stock six experience
levels (`level_0` to `level_5`). Each row of `scripts/species/exp_levels.txt`
may carry extra `level_6`, `level_7`, … columns; a row without them keeps its
six levels, so the count is per row and so per squad type.

The game reads every per-level field (experience thresholds, health, accuracy,
speeds and the rest) through six cell parsers. The plugin keeps each row's
stock six cells where the game checks for exactly six, records the extra cells
beside them, and has each parser append one value per extra level. A row ends
at its first missing or empty extra cell; stat lookups past its end use its
last value. Ammunition damage profiles past the stock levels use the last
stock level's profile.

For a modding example, row references, extra rank assets and current limits,
see [Experience levels in the feature guide](../../docs/features.md#experience-levels).

The plugin is disabled by default. In `DefianceLoader/config/gameplay.ini`,
set:

```ini
[defiance.experience-levels]
enabled = true
max_levels = 32
```

`max_levels` counts `level_0` and accepts 6 to 64. A row's levels stop at the
first missing or empty `level_N` column, or at `max_levels`. Restart the game.
The startup log should contain
`experience levels installed (logic+<rva>): up to 32 levels per row; meter laid out after Core squad refreshes`, and the
first row with extra levels read logs
`experience levels: first row with extra levels read`.

The squad info panel's experience meter gets one divider per level boundary,
made and spaced for each squad, so no UI script changes are needed. Core owns
the infantry refresh hook and invokes the original before notifying
subscribers; this plugin subscribes once and lays out the meter for infantry
panels only. The plugin requires Core 0.8.0 or later.

New levels need their own rank icons and rank names: `exp_level_6` and later
in the game's textures and in `strres.xml`. Without them the icon and name are
blank. The feature is single-player only.

Build with:

```powershell
mise exec "--" cargo build --release --manifest-path plugins/experience-levels/Cargo.toml
```

The plugin finds the row reader, the cell lookup and the six parsers by
whole-function signatures and hooks them only when all of them match; on any
other code it logs a warning and installs nothing. `cargo test` checks that the
sites resolve in every 2026 build in `bin/` and runs a parser stand-in over
fabricated rows.
