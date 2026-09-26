# DefianceLoader

A plugin loader and gameplay plugins for *Terminator: Dark Fate - Defiance*.
Drop it into the game folder and it loads with the game: no separate program to
start, and your usual launcher or shortcut keeps working.

## Features

- **Individual soldier control:** select single soldiers within a squad, and
  give only them move, posture, attack, building-entry and firing-mode orders.
- **Per-soldier weapons:** enable or disable weapons and ammunition for the
  selected soldiers from the ammo panel.
- **Expanded ammo menu:** more than nine ammunition entries in the in-mission
  menu.
- **Squad management scrolling:** scroll weapon, ammunition, perk and upgrade
  rows, and install more than five upgrades per unit, in the army presets and
  squad management screens.
- **Smarter weapon pickup:** the selected soldiers pick up weapons first, and
  repeated pickups rotate between eligible soldiers.
- **Squad previews** show the first matching weapon, and **TAB** from a building
  selects its occupants.
- **Unit inspection:** click an allied, neutral or enemy squad to see its full
  details, weapons and ammunition included; the ammo cards' reload bars show
  its relation by colour.
- **Performance:** faster shadows, object sorting and matrix work on the main
  thread, with exactly the game's results (about 31 to 58 fps on a busy scene
  in testing).
- **Regroup** (experimental, off by default): form new squads from selected
  soldiers.

Every feature can be switched off on its own. See [features](docs/features.md)
for controls and details.

**Single-player only.** The gameplay features change the game's simulation and
send nothing to other players. While any of them is active, the game will not
connect to multiplayer; switch them off to play online. This protects honest
players from desynced games. It is not anticheat.

## Install

Download the latest [release](https://github.com/DefianceLoader/DefianceLoader/releases)
and follow the [installation guide](INSTALL.md). The loader checks the game
version at startup and changes nothing on a version it does not recognise; the
log in `DefianceLoader/logs/defiance-loader.log` says why.

## Documentation

- [Features and controls](docs/features.md)
- [Building from source](docs/building.md) and [development](docs/development.md)
- [Writing a plugin](docs/plugin-authoring.md), the [plugin API](docs/plugin-api.md)
  and [service examples](examples/services/README.md)
- [Crash reports](docs/crash-reporting.md)

## Licence

The loader, plugins and tools are under the [Mozilla Public License 2.0](LICENSE).
The parts plugins build against (`crates/api`, `crates/feature-sdk`,
`crates/build-support`, `plugins/example`, `examples/`) are under your choice of
[MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), so plugins can use any
licence. Each crate's `Cargo.toml` states its licence.

*Terminator: Dark Fate - Defiance* belongs to its owners. This repository
contains none of the game's files, only short byte signatures and addresses used
to recognise supported versions.
