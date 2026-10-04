# Vehicle arrival braking

Experimental plugin for wheeled and tracked ground vehicles. It shortens the
slow final approach to a destination by narrowing the game's requested-speed
ramp. The stock waypoint radius, path following, steering, physical acceleration
and braking statistics remain in use. It affects all ground vehicles using the
standard car/tank chassis, regardless of owner.

The plugin is included in loader builds but disabled by default. Copy its DLL
and matching `.plugin.json` into `DefianceLoader/plugins/` when installing it
separately. In `DefianceLoader/config/vehicles.ini`, set:

```ini
[defiance.vehicle-arrival]
enabled = true
braking_window_percent = 50
```

Restart the game. The startup log should contain
`vehicle arrival installed (<build>): 50% braking window`. Values from 50 to 100
are accepted. At 50 the requested-speed ramp covers half the stock distance;
75 is a gentler change, and 100 keeps stock behavior without installing a hook.
Set `enabled = false` and restart to disable it.

In-game stopping accuracy, reverse movement, queued orders, tight turns and
blocked destinations still need testing. Compare the same straight move before
and after enabling it, then check short moves and queued waypoints for overshoot
or repeated corrections. The native tests verify requested speeds, not physical
arrival times. The feature is single-player only.

Build with:

```powershell
mise exec -- cargo build --release --manifest-path plugins/vehicle-arrival/Cargo.toml
```

Run `mise run vehicle-arrival-test` to
exercise the compiled detour against fabricated objects and each supported
stock DLL. Test-only exports are confined to the `parity-test` build; shipping
builds do not expose them. The installer verifies the DLL identity and complete
arrival method before placing one loader-owned entry hook, and refuses unknown
or modified code.
