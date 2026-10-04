# Moving actions render-binding repair

This experimental companion repairs a held-weapon model or attachment mismatch
for GOG 2026-09-14 / 2026-09-25 and Steam 2026-09-22 / 2026-09-25 game images.
It binds each exact DLL hash independently. It checks the selected human gun at
shot callbacks and watches loaded gunner state at a throttled interval. A
persistent loaded mismatch can use the stock model-and-animation handoff after
two matching observations. When the model list already matches, it uses the
stock attachment rebind. It preserves the original firing and update calls.

Routine shots, hook arrivals, watch snapshots and inventory dumps are not
logged. At info the plugin writes only its install line (with the matched
`logic.dll` hash) or an error naming why it refused. A loaded repair whose
post-check fails is a warning. The rest is debug
([log levels](../../docs/development.md#log-levels)):

- `moving weapon mismatch:` — a firing event observed an actual selected/model/
  attachment mismatch, including whether the guarded repair fixed it.
- `moving weapon mismatch observed:` — a loaded mismatch changed or was first
  confirmed during observation, with the current repair-readiness state.
- `moving weapon loaded repair:` — the selected model/animation or attachment
  repair was attempted and succeeded.
- `moving weapon render skipped:` and `moving weapon watch skipped:` — bounded
  snapshot/type/layout anomalies that prevent safe inspection.

Mismatch reports are globally capped per initialization. Stable ordinary
updates remain observed without emitting snapshots. Loaded-mismatch repair
continues checking an unchanged mismatch once per simulation second so a
temporary transition or timer cannot permanently suppress a later safe repair.

## Install and verification

Install this companion's DLL and `.plugin.json` beside the loader plugins. It
requires `defiance.moving-actions` and is off by default; enable it with
`enabled = true` (or disable it with `enabled = false`) in
`[defiance.moving-actions-render-sync]` in `DefianceLoader/config/infantry.ini`
and restart. Supported logic.dll hashes are recorded in the binding profiles
resolved by `tools/moving_actions_render_sync_bindings.py`. The exact World2
SHA256 remains `c39827bec79c0c4e1358259b5a2b3a6762e9270c6f5e1f25ce32d3f95b6a15c2`.
December 2025 and unrecognized native builds are refused before hooks.
No installed game file is patched on disk.

```powershell
mise exec -- cargo build --release --manifest-path plugins/moving-actions-render-sync/Cargo.toml
mise exec -- cargo clippy --release --manifest-path plugins/moving-actions-render-sync/Cargo.toml -- -D warnings
mise exec -- python tools/test_moving_actions_render_sync.py
```

The native harness maps stock game images privately and exercises the actual
stock model rebind and loaded-switch handoff, with shims for World2 node
callbacks and simulated firing. It verifies repair guards, original-call
forwarding, quiet normal updates, bounded mismatch/skip diagnostics, and repair
reporting. It does not verify rendered appearance in a live game.
