# Legion vehicle hacking

Allows hacking Legion vehicles while their drones are repairing them, without
an EMP first. The game still applies its other hacking target checks, and a
vehicle that is not repairing still needs an EMP. The plugin finds the code it
changes by its bytes, so it works on the 2025-12-23 and 2026 GOG and Steam
builds; on a build where that code differs it logs a warning and changes
nothing.

Off by default; set `enabled = true` under
`[defiance.legion-vehicle-hacking]` in
`DefianceLoader/config/vehicles.ini`. Restart required. Like the other
gameplay plugins shipped with the loader, it blocks online multiplayer while
active.
