# Primary weapon drops

An experimental, single-player plugin for the GOG and Steam 2026-09 builds. The
final infantry casualty emits one native ground pickup per compatible occupied
standard primary slot. Partial casualties do not emit primary pickups. Native
death bookkeeping and special-weapon drops still run. Collecting a supported
primary replaces that slot across the receiving squad's live human members and
leaves one old primary on the ground. Held specials and equipment outside that
declared slot remain available. The replacement registers ammo types and squad
resupply capacity, then imports the ground pickup's rounds **once per squad**,
up to capacity. Excess rounds remain available in a ground pickup. Native
removal releases outgoing capacity/carriers before registering replacements.
Ammo for other equipment stays in the shared pool. Identical primaries use the
same exchange as different primaries, including empty pickups. With transfer
enabled, the collected copy supplies its own ammo and the outgoing copy keeps
its removed ammo shares. Incoming rounds above capacity remain in a separate
ground pickup. Unsupported or ambiguous squad inventories remain on the ground
and produce bounded debug messages in the loader log.

After native cache rebuild, the plugin reconstructs the squad's slot override
from saved member inventories before refresh or reinforcement equips members.
Shared unit-type metadata stays unchanged. Conflicting member inventories fail
closed, and a holder with no member evidence keeps its scripted defaults.
Live checks confirm carried models, repeated exchanges, correct separate ammo
counts for identical primaries, weapon and ammo persistence through save/reload,
and mission completion. The remaining live check is squad destruction while a
secondary weapon is also equipped: verify both pickups and their ammo totals.
The per-soldier prototype is preserved at commit `ab71a88`.

`ammo_policy` controls outgoing primary ammo during swaps:

| Value | Outgoing ammo |
| --- | --- |
| `discard` (default) | Discard the removed primary's shares; drop an empty old weapon. |
| `transfer` | Put those shares on the old weapon's pickup. |
| `retain` | Keep unused rounds with the squad; restore them up to capacity when a compatible primary returns. |

`death_ammo = discard` (default) leaves wipe drops empty.
`death_ammo = transfer` distributes available **unreserved shared rounds** across
compatible primary drops once. Loaded magazine reservations are excluded;
this is a conservative transfer rather than complete magazine recovery.
Unused retained ammo is discarded when
the squad is destroyed. Swap and death policies are independent.

Retain mode stores unused rounds outside live native ammo rows and serializes
them as tagged stock records. The plugin removes those records during load.
Saves containing retained reserves require the plugin for correct interpretation;
use copied saves and disposable missions for validation.
Restoration of saved unused reserves runs only in retain mode. Switching to
discard or transfer preserves existing unused reserves until retain is re-enabled.

The prototype is disabled by default and built separately:

```powershell
mise run weapon-drops-build
mise run weapon-drops-test
mise run weapon-drops-package
```

For a manual test, copy `plugins/weapon-drops/target/release/defiance_plugin_weapon_drops.dll`
and `plugins/weapon-drops/defiance_plugin_weapon_drops.plugin.json` into the
loader's plugin directory (normally `DefianceLoader/plugins/`). In
`DefianceLoader/config/infantry.ini`, set:

```ini
[defiance.weapon-drops]
enabled = true
ammo_policy = transfer
death_ammo = transfer
```

Restart the game. With debug logging enabled, the startup log contains
`experimental squad primary drops and swaps installed`, followed by the selected
ammo policies. On a build the plugin does not support, it logs a warning naming
the site that did not resolve and installs nothing. Settings require a restart.

The separate package includes `mods/defiance_weapon_drops/`, a companion overlay
derived from the installed localization archives. Install that folder alongside
the plugin, then enable **Defiance primary weapon pickup labels** in the game's
**MODS** menu and give it priority over conflicting overlays. Restart the game.
Enabling the loader plugin alone does not mount this companion mod.
It supplies missing primary-pickup names, descriptions
and pickup hints using existing localized inventory text; stock locale rows and
archives remain intact. Packaging requires `DEFIANCE_GAME_DIR` and the local
archive password, like the other companion-mod generators. Generated game
resources remain in the ignored package output, outside the repository.

The plugin refuses unknown DLLs and modified native code. It skips unsupported
gunner layouts, ambiguous primary inventories, grenades, empty meshes and
context overrides whose default pickup model would show another gun. Deaths
outside the guarded human damage callback are not covered.

In a disposable mission, kill one ordinary member and verify no primary drop;
then kill the remaining squad and verify one pickup for each occupied primary
slot. Friendly and enemy infantry follow the same callback. Collect with a
compatible squad carrying a different primary and verify every live member
changes that slot, can select/fire it, and leaves one old pickup.
Verify that its ammo types appear in the squad ammo list, resupply it,
and verify firing consumes the replenished ammo. Exchange an identical primary
with a different ammo count and verify the squad receives that count while the
old ammo remains with the outgoing copy. Check an empty incoming copy and a
full receiving pool, with no Gun or carrier accumulation. Check incompatible
pickups, repeated swaps, a held special, ammo totals, repeated
corpse damage, explosive deaths, scripted removal, reinforcement and save/load.
Existing missions can be used, although inventories duplicated by an earlier
experimental build will be refused. The primary-plus-secondary death check
remains pending; the confirmed live results are listed above.

`weapon-drops-test` exercises the compiled death shim, replacement and special
preservation on fabricated engine objects, and resolves the plugin's sites on
every local logic.dll. It does not launch or modify the game.
