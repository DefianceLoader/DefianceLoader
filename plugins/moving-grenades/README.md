# Moving grenades

Version 0.1.21 supports GOG 2026-09-14 / 2026-09-25 and Steam 2026-09-22 /
2026-09-25 logic.dll snapshots. It depends on defiance.moving-actions.
Fully restart the game after installation. Preserve previous DLL/manifest
pairs as inactive backups; keep all other plugins.

The user confirmed v0.1.18's smoke continuation worked without observed bugs.
This port preserves that behavior, combat grenade route retention, automatic
attack-move grenade selection, explicit Stop cancellation, and movement-facing
adjustments. The native code still controls grenade eligibility, aiming,
timers, projectile release, and target cleanup. Smoke movement resumes only
after the throw is committed; a brief native preparation pause remains.

When an attack interrupts movement, the plugin retains the exact route and
owned movement-order references. A confirmed throw queue resumes a still-valid
route. For positional abilities such as smoke, it forwards the native final
Stop, waits for Stop initialization and the enclosing AI update to finish,
then verifies target cleanup and route/owner identity. It submits a fresh
native ordinary Move or attack-move order using the retained destination or
target, so the finished interrupted order cannot suppress movement. New
commands, explicit Stop, expiry, or identity changes cancel continuation and
release owned references.

## Supported builds

The plugin finds its sixteen hook sites, helper functions, caller return
addresses and RTTI vtables by signature and RTTI in the loaded `logic.dll`
([`src/sites.rs`](src/sites.rs)), and checks each hook entry and the
navigation bounds before hooking. Its sites test resolves every site uniquely
on the four September 2026 snapshots in `bin/` and proves they equal the
addresses the plugin used before. The December 2025 builds lack the point-turn
steering call the plugin relies on, so they are refused. A site that does not
resolve, or a changed entry, logs a warning ("not a supported build") and
patches nothing. If the loader refuses any hook, the plugin removes every hook
it installed before it and reports the refusal.

Build and test from the repository root:

```powershell
mise exec -- cargo test --manifest-path plugins/moving-grenades/Cargo.toml
mise exec -- python tools/moving_grenades_bindings.py
mise exec -- cargo clippy --manifest-path plugins/moving-grenades/Cargo.toml --all-targets -- -D warnings
mise exec -- cargo build --release --manifest-path plugins/moving-grenades/Cargo.toml
mise exec -- python tools/test_moving_grenades.py --build gog-2026-09-25
```

Use a Python environment with pefile and capstone. The native harness accepts
each supported build name and exercises sequencing, route and target guards,
fresh order identity, reference accounting, and the unsupported-build
warning. Add --reject-manager-trace or --reject-completion to check that a
refused hook removes every earlier hook.
The real-loader integration additionally installs the complete companion
suite and exercises native navigation bounds/cancellation and inactive order
submission. These checks use privately mapped images and callback fixtures;
they do not execute the entire live game controller. Live gameplay on the
other three binaries and EMP behavior remain to be confirmed.
