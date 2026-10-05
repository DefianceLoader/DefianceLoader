# Moving-action animation sampler (experimental)

This plugin keeps a lower-body locomotion pose during standing grenade throws
and weapon changes. It finds its hooked samplers, the animation update, the
quaternion helper and the two facet vtables by signature and RTTI in the loaded
`logic.dll` ([`src/sites.rs`](src/sites.rs)), and checks each entry and the
HumanAnimationFacet layout before hooking. Its sites test resolves every site
uniquely on the four September 2026 snapshots in `bin/` and proves they equal
the addresses the plugin used before. The December 2025 builds have an older
facet layout, so they are refused. A site that does not resolve, or a changed
entry or layout, logs a warning ("not a supported build") and hooks nothing;
if the loader refuses any hook, the plugin removes the hooks it installed
before it. It depends on
`defiance.moving-actions`; both are off by default (set `enabled = true` in
each one's section of `DefianceLoader/config/infantry.ini`) and a full game
restart is required.

The native update and key samplers always run. During the update scope, the
plugin checks action, posture, chassis ownership, movement state, animation
geometry, and stock clip shape. It overlays only thigh, calf, foot, and toe
position/rotation tracks for throw (`0x17`, `0x2b`) and weapon change (`0x18`).
Action timing, root/upper-body tracks, events, timers, and navigation remain
native. The run gait is baked, so cadence does not adapt to speed; start/end
transitions and non-standing actions remain visual limitations.

Stage/package builds derive five private runtime resources from the installed
game's `basis.pak` and `patch_*.pak` files and place them under
`<game>/mods/defiance_moving_actions/assets/`. The companion mod also includes
`sources.json` with source archive/member hashes and output hashes. Its assets
are custom plugin data; the builder never overrides the game's stock action
clips. The plugin reads and validates these files at initialization. The PAK
password is supplied through `DEFIANCE_PAK_PASSWORD` when the installed
archives are encrypted. A manual build can be made with:

```powershell
mise exec -- python tools/package_moving_actions_animation.py --game "C:\Games\Defiance"
```

For fixture/native harnesses, `MOVING_ACTIONS_ANIMATION_ASSET_DIR` can point at
the generated companion `assets` directory.
