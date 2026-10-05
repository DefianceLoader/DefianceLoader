# Moving actions test plugin

Experimental soldier movement during grenade throws and weapon changes.
In-game testing confirms movement continues during weapon changes. Earlier
tests reported pauses, possibly because the running game had not picked up the
build. The temporary moving-actions-probe companion also recorded displacement
throughout weapon changes. Full-body action clips still produce sliding, and
the user reported occasional held-weapon/firing-weapon mismatches. The
`moving-actions-render-sync` v0.1.3 companion now repairs the captured persistent
M2010/MP5 mismatch, and the user has confirmed that fix in-game. Both manual and
automatic grenade throws can still stop soldiers at preparation; issuing a new
move order during the throw lets movement continue. The original animation
speed patch therefore does not cover every grenade preparation path.
The optional `moving-grenade-probe` captures stop/turn caller history before
actions 0x17 and 0x2b to locate that preparation pause. It is read-only and does
not change grenade movement behavior.
The chassis speed helper returns zero speed for these actions. This
plugin changes three action-table entries to use the engine's existing
posture-dependent speed calculation. Throw/release timing, weapon-switch delay,
ammunition use, and the other action-table entries retain their engine behavior.

The plugin finds the movement-speed getter by byte signature
([`src/sites.rs`](src/sites.rs)) and checks the whole getter, its jump table
and its action table before writing through the loader's patch API. A build
where the getter does not resolve to exactly one place, or where any of those
bytes differ, logs `moving actions: not a supported build (...); no writes
made` as a warning and patches nothing. The sites test and the native test
cover all six GOG and Steam snapshots in `bin/`.

## Install for testing

Close the game. Copy `defiance_plugin_moving_actions.dll` and
`defiance_plugin_moving_actions.plugin.json` into the existing loader's plugins
directory (normally `DefianceLoader/plugins`), then start the game. This package
requires an installed DefianceLoader with ABI 5 support.

The experiment is off by default. To enable it, set `enabled = true` in
`[defiance.moving-actions]` in `DefianceLoader/config/infantry.ini` and
restart. To disable it again, set
`enabled = false` in `[defiance.moving-actions]` in
`DefianceLoader/config/infantry.ini`, then restart; removing both plugin files
while the game is closed also disables it. The patch is process-local; the
game's DLL files are not edited. The loader restores its owned bytes on unload
or failed initialization. The manifest retains the loader's single-player guard.

The startup log at `DefianceLoader/logs/defiance-loader.log` must include:

```text
moving actions installed: grenade throws and weapon changes retain movement; 3 table bytes patched
```

## In-game checks

1. Give an infantry squad a long move order and trigger a weapon change using
   its ammunition panel, with another usable weapon available. Check that the
   changing soldier advances and switches weapons after the usual delay.
2. While moving, order a smoke/EMP throw or watch an automatic combat grenade
   throw. Check movement, projectile release position, ammunition consumption,
   target location, and whether the original move order continues.
3. Repeat while running and in crouched/prone postures. Check walking/running
   speed before and after the action, animation appearance, and squad formation.
4. Compare with the plugin disabled and restart between comparisons.

The optional `moving-actions-sync` companion checks completed weapon changes
for a missed model handoff. It repairs a verified stock timer edge case using
the existing engine handoff code. This companion requires in-game testing to
establish whether its timer-edge fix is exercised. The later
`moving-actions-render-sync` companion checks loaded model bindings independently
of firing and includes the repair confirmed in-game. Keep this successful
repair enabled while investigating grenade preparation.

The speed patch does not add animation blending. A stationary throw/change
animation may slide while the soldier advances. Running multipliers are applied
by the engine only during its locomotion action, so actions can temporarily use
the normal posture speed. Grenade preparation can independently pause the move
order; the user reports that it resumes afterward.

## Animation investigation

The full assets are available at `C:\TDFD_Unpacked_new_lol`. The standing throw
clip at `basis/animations/new/anim/st_throw_grenade_m16.anim` has 29 tracks and a
3.566667-second duration. `st_change_weapon.anim` also has 29 tracks and a
1.666667-second duration. Both contain pelvis and both leg chains, including
thighs, calves, feet, and toes. The comparison `st_run_m16.anim` has 23 tracks
and a 1.066-second duration, including upper and lower body.

The files parse exactly to EOF using the ANIM v1 structure verified against
world2 Animation serialization routines (0xa7f80 / 0xa8180 / 0xa7910): a 16-byte
header (magic, version, duration, track count), then null-terminated track names
with counted translation (16-byte) and rotation (20-byte) key records.

World2's generic Animator has five layers (constructor 0xaf880, play 0xaf990,
update 0xaee10). AnimationClip::getTransform (0xac750) leaves a node untouched
only when that clip lacks the node's track. These full-body action clips would
therefore overwrite running legs on a later layer. An experiment needs sparse
upper-body variants plus a verified route to an independent layer. The current
HumanAnimationFacet path (2c3e80 / 2c3990 / 4337b0) has not yet been connected to
that layer API. No animation assets or playback methods have been changed.

## Build and native checks

```powershell
mise exec -- cargo test --manifest-path plugins/moving-actions/Cargo.toml
mise exec -- python tools/moving_actions_bindings.py
mise exec -- cargo build --release --manifest-path plugins/moving-actions/Cargo.toml
mise exec -- python tools/test_moving_actions.py
```

The native test uses mapped stock DLL copies and fabricated chassis state. It
does not modify an installed game or claim to validate in-game animation.
