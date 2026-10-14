# Deployed repair

Allows a deployed vehicle, turret or platform, or a landed helicopter, to use
its existing crew repair action while retaining its deployed or landed position.
Repair still requires an eligible crew, damage and the game's normal repair
resources. The crew exits, repairs
and returns using the native repair order and timing.

Included with the loader, disabled by default. To install separately, copy
`defiance_plugin_deployed_repair.dll` and its matching `.plugin.json` into
`DefianceLoader/plugins/`. In `DefianceLoader/config/vehicles.ini`, set:

```ini
[defiance.deployed-repair]
enabled = true
```

Restart the game. Check the startup log for `deployed repair installed`.
Disable it by setting `enabled = false` and restarting. The plugin is
single-player only and requires a restart for configuration changes.

The menu keeps only the Repair permission that a fully deployed unit already
qualifies for. Other restricted actions and deployment transitions retain their
stock restrictions. For helicopters, the menu permits the native repair checks
only while the current AI state is landed; it still requires damage, spare parts
and eligible crew. During that unit's native self-repair startup, the plugin
suppresses only the automatic undeploy order issued when its crew exits.
A deployed unit's recharge state normally discards the repair task; the plugin
stacks that one task above it instead, leaving other tasks to the stock code.
A landed helicopter's state discards it the same way and gets the same
treatment, except while it is still landing or taking off.
Manual crew removal, other units and other threads retain native behavior.
Units without wheeled-vehicle repair points use the native geometry-bounds
placement for their repair crew.

The installer validates all seven sites before requesting any loader-owned
hooks. Signatures and RTTI identify the menu mask and helicopter veto,
self-repair startup, crew-exit stop call, crew-position selection, and the
recharge and landed states' task handlers across
the eight local reference builds. Unsupported code logs a warning and receives
no writes. The patch contract declares all seven sites, and the loader owns
rollback on installation failure.

Build and test:

```powershell
mise exec -- cargo build --release --manifest-path plugins/deployed-repair/Cargo.toml
mise run deployed-repair-test
```

Native tests execute the stock deployment restriction and compiled mask detour
on fabricated objects for all eight reference builds. The helicopter tests run
the native repair eligibility and crew-position selection on the reference
build and newest build per store. Null and empty repair-point vectors select
the native bounds fallback; valid points retain their original selection.
Rust tests cover site mutations and scoped crew-exit behavior.
Test exports exist only in the
`parity-test` build.

Repair on a deployed vehicle and a landed Black Hawk has been confirmed in game.
Still to check: repair on a deployed turret and platform, the unit resuming
recharge afterwards,
cancellation, manual crew unloading, queued orders and save/load during repair.
Report the `deployed repair installed` line and any plugin errors from
`DefianceLoader/logs/defiance-loader.log` with the observed behavior.
