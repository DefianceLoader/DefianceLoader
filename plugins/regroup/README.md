# Experimental ad-hoc squads

Regroup requires the loader's service extension, selection service v1, and Core's ammo-menu service v1.
Use matching Core and regroup DLLs with a current loader and selection plugin.
It refuses initialization if the service is unavailable. Plugin authors can find
the service contract in `docs/plugin-api.md` and working examples under
`examples/services` in the source repository.

Select 1–16 player-owned infantry by default, then press **Ctrl+Alt+R** to form a real
squad from those soldiers. This can combine members of several squads or split
a subset out of one squad. Soldiers left out of the selection keep their source
squad. Selecting an entire single squad is a no-op.

The destination uses the first source squad's species/name/icon and starts with
that species' squad settings. Native member links and combat lists are changed,
so the intended result is one icon and one settings panel. Soldier entities and
gun objects are reused. Loaded magazines remain with their guns. Shared reserve
ammunition and spare capacity are divided according to the selected soldiers
whose guns support each ammunition type; integer remainders stay with the source.
Pools are combined by ammunition identity, and existing individual ammo pins are
remapped to their new slot indices.

Basic regrouping and restore were confirmed working in gameplay. Keeping
empty originals dormant has only been tested offline. Mission scripting, upgrades and save/load still
need live checks. Start from a separate single-player test save. Multiplayer and
replays are unsupported. Infantry must be outside buildings and vehicles. A
selection whose source squad contains an unsupported member is rejected as a
whole; mixed infantry/vehicle selection is also rejected. Ammunition capacity follows the installed menu unless a smaller cap is configured (see below).

## Restore selected soldiers

After regrouping with this build, select up to max_soldiers soldiers and press
**Ctrl+Alt+U**. Their first recorded origins are retained across repeated regroup
operations. Selected soldiers already home stay put; unselected soldiers stay in
their current squads. Restore uses current weapons and remaining ammunition,
so it does not undo expenditure or resurrect casualties.

Empty recorded originals now stay dormant while any tracked soldier survives.
Their selection is disabled and AI updates paused; Ctrl+Alt+U returns soldiers
to the same original entity and reactivates it. This retains the object holding
its metadata instead of recreating defaults. Temporary regroup squads still
receive normal cleanup, and an empty original is released when no tracked
soldiers survive. Individual perk effects while away are not frozen or copied.

If an original was deleted by another route, a squad of its original type and
team is recreated as a fallback. This
recreates the grouping, **not its old campaign ID, custom metadata, training,
experience or settings**. Those remain an open preservation task.

History is kept only in the current live world: it is not written into saves
and cannot restore groups formed before this build was installed. Start from a
save made before regrouping; test R then U without reloading. There is no
mission-end automatic restore. Do not rely on this build to preserve campaign
rosters or perks. A restore spanning several origins runs one group at a time;
if a later group is refused, earlier completed groups remain restored and are
reported in the log. An original squad's unsupported/garrisoned/vehicle members
can prevent joining it until they are dismounted and outside.

## Install

With the game closed, copy these two files into the existing loader's plugin
directory (normally `Game/DefianceLoader/plugins/`):

- `defiance_plugin_regroup.dll`
- `defiance_plugin_regroup.plugin.json`

Update Core along with regroup; keep selection and the other feature DLLs. This add-on requires
the current ABI 5 loader and individual-selection plugin. It supports only the
reference GOG/Steam logic.dll and game.dll builds; other DLL hashes are refused. It adds
ten owned hooks and does not modify game files on disk.

Regroup is disabled by default. To opt in, set `enabled = true` in
`[defiance.regroup]` in `config/infantry.ini` and restart. To disable it, set
`enabled = false` or remove its two plugin files and restart. Preserve existing
configuration files when upgrading; an existing explicit setting still wins.

## Current build

The initialization log identifies this build as `large squad UI roster fix 11`. The
shortcut now runs from the game's input dispatcher after native keyboard state
updates, instead of polling during selection queries. It ignores auto-repeat
and captured UI input. After loading a mission, expect `game input dispatch
hook reached`; pressing Ctrl+Alt+R should log `regroup keyboard event detected`
and then a preflight result. If nothing happens, include these log lines when
reporting it. The constructor now suppresses species weapon templates for the
new regroup destination before ammunition preparation; the log should include
`regroup destination: skipped default weapon templates`. Live gameplay still
needs validation.

## First live check

1. Pick one soldier from each of two ordinary infantry squads. Press Ctrl+Alt+R.
2. Check that the new squad contains only those soldiers, the remaining soldiers
   stay in their old squads, and health, experience, weapons and ammo match.
3. Change the new squad's behavior, posture and firing settings from its single
   panel. Move, attack and check formation. Verify source squads remain independent.
4. Try splitting a subset from one squad, then combining complete squads so the
   empty source squads must disappear.
5. Press Ctrl+Alt+U on selected regrouped soldiers. Check their original
   squads, unselected squadmates, loaded magazines, reserve ammo and ammo pins.
6. Repeat after two successive regroups, and separately after emptying original
   squads. The latter should log that the originals were parked and then reactivated.
   Check their original names, training, experience and settings after restore;
   check for unwanted empty icons while soldiers are away.
7. Save/load and mission-end behavior remain separate unverified checks; loading
   a save discards the restoration history.

The log (`DefianceLoader/logs/defiance-loader.log`) includes the load result,
selection rejection reasons, number of transferred soldiers, and immediate
identity/magazine/ammunition checks. A refusal changes no source members. A
runtime constructor failure is logged as an error; do not treat it as success.

## Build and checks

This crate is a separate workspace so it is not included in the normal package:

```powershell
mise exec -- python tools/regroup_bindings.py
mise exec -- cargo test --manifest-path plugins/regroup/Cargo.toml
mise exec -- cargo build --release --manifest-path plugins/regroup/Cargo.toml
mise exec -- python tools/test_regroup.py -v
```

`regroup_bindings.py` resolves each function in both local reference images,
validates the constructor and binding call targets, and generates exact-build
tables. Runtime initialization checks the DLL hash and every used entry before
installing hooks. Rust tests cover ammunition conservation, pin remapping and
the transfer orchestration using fabricated engine objects. Python tests execute
the original combat-list removal leaf routine. Neither substitutes for live
gameplay or save/load testing.

## Dormant icon fix 6

Preserved empty originals now hide their world hover icon. Restoring soldiers
resumes the native icon update, including its normal visibility rules. The
startup log includes `dormant icon fix 6`. This UI fix needs in-game verification.

## Configurable hotkeys (iteration 7)

In `DefianceLoader/config/infantry.ini`, add these entries to the existing section:

```ini
[defiance.regroup]
enabled = true
regroup_hotkey = Ctrl+Alt+R
restore_hotkey = Ctrl+Alt+U
```

Restart the game after editing. Install both the updated DLL and its manifest.
The example above opts in; an omitted `enabled` defaults to false. Omitted
hotkey settings keep the bindings above. The startup log prints both bindings
when the plugin initializes.

Use one key, optionally joined with `+` to any combination of `Ctrl`, `Alt`,
and `Shift`. Names are case-insensitive and surrounding whitespace is ignored.
Modifier order does not matter. No modifier means a plain key, e.g. `F9`.
Modifiers must match exactly: `Ctrl+R` does not trigger while Alt or Shift is held.
Left/right variants of a modifier are treated alike. Windows-key modifiers,
mouse buttons, raw key codes, and punctuation keys are not supported.

Allowed key names:

- `A` through `Z`; `0` through `9` (top row).
- `F1` through `F24`.
- `Numpad0` through `Numpad9`.
- `Space`, `Tab`, `Enter`, `Escape`, `Backspace`.
- `Insert`, `Delete`, `Home`, `End`, `PageUp`, `PageDown`.
- `Left`, `Right`, `Up`, `Down`.
- Numpad operators: `Multiply`, `Add`, `Subtract`, `Decimal`, `Divide`.

Examples: `Ctrl+Shift+G`, `Alt+F8`, `F9`. Numpad bindings follow the key reported
by Windows, so Num Lock can change which binding matches. Enter covers both
Enter keys. Unsupported names, repeated modifiers, missing keys, or identical
regroup/restore chords refuse plugin initialization with a descriptive error.

The plugin receives input after the game: a binding can also activate an existing
game shortcut. Choose unused combinations. OS-reserved shortcuts may not reach
the game. Held-key repeat is ignored; existing foreground/UI-capture guards apply.

The package includes `regroup-hotkeys.ini.example` as a reference. With the
updated loader, startup inserts missing entries and their key-list comments into
`config/infantry.ini` automatically. Existing bindings are preserved. Older
loaders require merging the example into the existing section manually.

## Configurable limits

In DefianceLoader/config/infantry.ini:

~~~ini
[defiance.regroup]
enabled = true
max_soldiers = 16
max_weapon_types = 0
~~~

max_soldiers accepts 1..64 for configuration compatibility, applies to both
regrouping and restore destinations, and defaults to 16. This release temporarily
caps the effective value at 20 because native squad-command buffers still assume
20 members. Higher configured values log a warning. Values above 16 remain
experimental; the low-level 64-member tests do not establish live game support.

max_weapon_types counts distinct **ammunition records**, not gun models.
0 automatically follows the installed menu: nine with the stock menu, or
columns * 3 after expanded-ammo-menu successfully installs (36 by default).
Explicit values 1..126 impose a smaller cap and are always clamped to installed
capacity. For example, 8 retains the previous cap. Invalid values refuse plugin
initialization before hooks. All settings require a full game restart.

Regroup asks Core's ammo-menu service at operation time, so plugin initialization
order does not matter. It never reads weapons.ini. An absent, disabled or failed
expanded menu leaves Core reporting nine. Logs show effective caps on hotkey use.

Per-soldier ammunition overrides still have only eight bits. If sorting the
combined pool moves a pinned ammo type past slot eight, its individual override
is dropped and that soldier follows the squad's setting for that type.
Whole-squad ammo toggles work beyond eight. Use max_weapon_types = 8 if retaining
all representable individual overrides is more important than expanded squads.

Automated coverage includes boundary/config tests, real Rust transfer code
against native-layout fixtures with 9/10/36/64 soldiers and ammo types, count
conservation, and refusal before detachment. Live formation/UI/save behavior
at increased limits remains unverified.


## Constructor failure fix

This build keeps the requested existing squad species when the native spawn-menu
factory would otherwise replace an unlisted species with a default type. The
change is scoped to regroup/restore creation; ordinary game spawns retain their
original behavior. Species and world checks remain mandatory.

If the member constructor was not claimed, it consumes the temporary name vector
without spawning default soldiers. An unverified factory result blocks further
regroup and restore attempts until a full restart, with result/claim/abort
diagnostics in the log. Failed cleanup is only requested for an empty squad;
populated results are never deleted automatically.

This does not remove duplicates already produced by an older build. Restart
the game and load a save from before the failed attempts (or start the battle
again) before testing. The new init log says "large squad UI roster fix 11".
Live confirmation of the affected squad types is still required.


## Species identity check

Distinct species objects can describe the same squad type. Destination matching
now accepts either the original species object or an exact nonempty species
identifier with the same native species vtable. Different names/classes and
malformed identifiers remain refused. This applies at both template suppression
and transfer validation; world identity must still match.

A failed comparison logs both identifiers and native types. A successful match
between separate objects is logged explicitly. This does not promise to copy
per-instance upgrade/training metadata; the existing preservation limitations
still apply. Retesting the affected live squad types remains necessary.


## Large-squad perk refresh crash fix

The stock squad perk-refresh routine has a 20-pointer stack buffer and copies
the whole roster without checking its size. At 21 members it overwrites saved
register storage; at 22 it overwrites the return address. This was reproduced
with the actual GOG and Steam copy instructions and matches the supplied crash
while expanding a 16-soldier regroup to 23.

Regroup now hooks that refresh routine for the process lifetime. Squads of 20
or fewer and non-squad/member-owned paths keep the original implementation.
Larger squads use the same native prepare/update/member-refresh helpers with
a dynamically allocated roster snapshot (bounded to 64), including refreshes
outside the initial transfer. This fixes the identified overflow; other large
squad gameplay/UI/save behavior still requires testing.

Regression tests cover 20/21/22/23/25/64-member refreshes and the native overflow.
Startup identifies this build as "large squad UI roster fix 11". Restart the game
after replacing the plugin, then reload a pre-crash save before testing again.


## UI roster export and current soldier ceiling

The 23-member crash after a successful, verified transfer was in the game UI.
Its native roster export resized an output vector to 20, copied every member,
then resized again. This could overflow the allocation and zeroed entries 21
onward, causing the UI to dereference a null entity. The replacement uses the
game's vector resize before copying a validated snapshot. Ordinary squads of
20 or fewer keep their original export path.

The audit also found six calls in native squad-command dispatch using another
20-pointer stack buffer. Those command paths have not been converted yet.
**This release therefore caps regroup and restore destinations at 20 soldiers.**
A configured value such as 25 remains readable but logs a warning and uses 20;
the default remains 16. A 23-member selection is refused before creating a
squad or moving soldiers. Ammunition limits are independent and unchanged.

The UI and perk fixes remain installed for oversized squads that already exist,
but they do not make such squads safe to command. Restart and reload a save from
before creating oversized squads (or restart the battle). Do not continue with
an existing squad above 20 members.

Offline coverage reproduces the original UI null entries using both supported
builds' machine code, and tests replacement exports through 64 members with
fresh/reused vectors. That broader test coverage is preparation for future
command conversion, not a claim of live support above 20.
