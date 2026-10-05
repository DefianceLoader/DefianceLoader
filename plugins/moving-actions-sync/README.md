# Moving actions weapon-sync test

Companion to `defiance.moving-actions`. The held weapon can occasionally
disagree with the firing weapon.

The stock weapon-change helper updates the held model only while its remaining
timer is positive and below 1.5 seconds. A long frame can consume the timer
before that branch runs, completing the switch with the old model still held.
This edge case was reproduced by executing the stock helper in a native test.
It has not yet been established as the cause of the reported live mismatch.

After a completed switch, this plugin checks the selected descriptor against
the front weapon model. If they differ and the selected model exists in the
unit's model list, it calls the stock handoff branch with zero elapsed time and
a temporary timer inside the handoff window. It restores the completed timer
and keeps the original return value. The existing engine code updates both
the model order and the weapon animation script. The gunner state machine and
firing tick are not advanced again.

The plugin finds the weapon step and the action helper by byte signature
([`src/sites.rs`](src/sites.rs)) and checks both entries before it hooks. A
build where either does not resolve to exactly one place, or whose entry bytes
differ, logs `moving weapon sync: not a supported build (...); no writes made`
as a warning and installs nothing. An ABI mismatch or a refused hook is an
error.

## Supported builds

The sites test resolves both sites on every build in `bin/`. The native
harness covers GOG 2026-09-14 / 2026-09-25 and Steam 2026-09-22 / 2026-09-25.
The December 2025 builds resolve but are unverified: the harness does not
cover them and their native contracts are not established.

## Install and test

Fully exit the game. Add these two files to `DefianceLoader/plugins` without
replacing any existing plugin:

- `defiance_plugin_moving_actions_sync.dll`
- `defiance_plugin_moving_actions_sync.plugin.json`

Requires ABI 5 DefianceLoader and the existing `defiance.moving-actions`
plugin. This companion is off by default; set `enabled = true` under
`[defiance.moving-actions-sync]` in `DefianceLoader/config/infantry.ini`.
Restart the game;
avoid relying on hot reload for this comparison. The loader log reports
`moving weapon sync installed: ...`.

Switch repeatedly between visually distinct weapons during a long move order,
then repeat while stationary. Check that the held weapon agrees with the
weapon firing after each switch. The first 150 completed-switch observations
are logged as `moving weapon sync:` with selected/displayed descriptors and
`repaired=true` when the companion successfully performed a missed handoff.
If the mismatch persists, that log will help distinguish a missed switch
handoff from a separate weapon-selection or firing issue.

The companion `defiance.moving-actions-animation` supplies the locomotion leg
overlay during throws and weapon switches.

To disable this companion, set `enabled = false` under
`[defiance.moving-actions-sync]` in `DefianceLoader/config/infantry.ini`, then
restart, or remove only this companion's two files while the game is closed.
The original movement plugin and all other plugins remain installed.

## Build and verification

```powershell
mise exec -- cargo test --manifest-path plugins/moving-actions-sync/Cargo.toml
mise exec -- cargo build --release --manifest-path plugins/moving-actions-sync/Cargo.toml
mise exec -- cargo clippy --release --manifest-path plugins/moving-actions-sync/Cargo.toml -- -D warnings
mise exec -- python tools/test_moving_actions_sync.py
```

The Windows x64 native test maps the stock DLL without resolving its imports
and executes the real switching helper, model-record reorder, and animation
script handoff using fabricated actor data with empty attachment vectors. It
reproduces the stale-model hitch, checks the built plugin repairs it, verifies
normal switches and grenade gates, and checks that an unresolved build
installs nothing and that a refused hook fails initialization.
It does not touch the installed game or validate rendered animation in-game.
