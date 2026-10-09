# Moving actions render-binding repair

This experimental companion repairs a held-weapon model or attachment mismatch.
It finds its `logic.dll` and `world2.dll` sites by byte signature and RTTI
class ([`src/sites.rs`](src/sites.rs)) and checks every hooked entry before it
hooks anything. It checks the selected human gun at
shot callbacks and watches loaded gunner state at a throttled interval. A
persistent loaded mismatch can use the stock model-and-animation handoff after
two matching observations. When the model list already matches, it uses the
stock attachment rebind. It preserves the original firing and update calls.

Routine shots, hook arrivals, watch snapshots and inventory dumps are not
logged. At info the plugin writes only its install line
(`moving weapon render installed`) or an error naming why it refused. A build
whose sites do not each resolve uniquely, or whose hooked entries differ, logs
`moving weapon render: not a supported build (...); no writes made` as a
warning and installs nothing. A loaded repair whose post-check fails is a
warning. The rest is debug
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
and restart. No installed game file is patched on disk.

## Supported builds

The sites test resolves every `logic.dll` site on GOG 2026-09-14 / 2026-09-25 /
2026-10-07 and Steam 2026-09-22 / 2026-09-25 / 2026-10-07, and the `world2.dll` render-node
callbacks on the two `world2.dll` snapshots in `bin/` (GOG 2026-09-25 and
2026-10-07). Unverified:

- the `world2.dll` of GOG 2026-09-14 and Steam 2026-09-22 (no snapshot);
- the December 2025 builds: their `logic.dll` sites resolve, but no
  `world2.dll` from them is in `bin/` and the native harness does not cover
  them.

On an unverified build the plugin either resolves every site and installs, or
logs the warning above and installs nothing.

```powershell
mise exec -- cargo test --manifest-path plugins/moving-actions-render-sync/Cargo.toml
mise exec -- cargo build --release --manifest-path plugins/moving-actions-render-sync/Cargo.toml
mise exec -- cargo clippy --release --manifest-path plugins/moving-actions-render-sync/Cargo.toml -- -D warnings
mise exec -- python tools/test_moving_actions_render_sync.py
```

The native harness maps stock game images privately and exercises the actual
stock model rebind and loaded-switch handoff, with shims for World2 node
callbacks and simulated firing. It verifies repair guards, original-call
forwarding, quiet normal updates, bounded mismatch/skip diagnostics, and repair
reporting. It does not verify rendered appearance in a live game.
