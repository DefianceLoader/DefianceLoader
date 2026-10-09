# Soldier move markers

Shows a small arrow for each selected soldier, including partial squad
selections, while aiming a right-click move order in either stock drag-placement
mode. Dragging to change facing rotates the preview. The larger stock squad
arrow is hidden for infantry;
vehicle arrows remain. After release, the soldier arrows show the individual
move destinations issued by the game, and the large vehicle arrows show vehicle
destinations. Each soldier arrow clears when its soldier settles near it or
stays at a cover position after moving; vehicle arrows clear as their vehicles
arrive. Soldier arrows use the
terrain height at each destination so they remain visible on uneven ground
and when the click hits an object above the ground.

Giving a new order to only part of a squad, or to one of several squads,
replaces the arrows for the newly ordered soldiers. Other soldiers' pending
arrows keep their original facing and stay visible during the new drag and
until they arrive. The game does
not give the destination callback a soldier identity, so a wide cover offset
can occasionally make the ownership estimate imperfect.

The preview uses the game's formation offsets. Cover selection can move a
soldier away from that formation point, so an arrow may differ from the final
cover position. An arrow without a usable soldier track can linger; an order
with no readable tracks times out after three minutes.
Garrisoning a building or boarding a vehicle can also leave the earlier move
arrow visible until another move order; these order paths are under study.

This plugin supports the GOG 2026-09-14, 2026-09-25 and 2026-10-07 builds
and the Steam 2026-09-22, 2026-09-25 and 2026-10-07 builds. It finds its code by signature, so it does
not depend on exact DLL hashes. It is on by default. To disable it, set
`enabled=false` under `[defiance.cover-markers]` in
`DefianceLoader/config/infantry.ini`, then restart the game. On a build where
any of its code cannot be found, it logs a warning and installs nothing.
