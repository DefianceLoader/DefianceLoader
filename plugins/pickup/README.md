# Pickup chooser: Rust implementation

The normal loader package uses Rust after differential and user-confirmed live testing. The
`rust-chooser` Cargo feature moves the pickup decision into `src/model.rs`,
with the game pointers and virtual calls isolated in `src/native.rs`.
There is no handwritten assembly in this Rust chooser. Other features keep
their current implementations.

From the repository root:

```powershell
mise run pickup-rust
```

This assembles the reference, runs Rust unit tests, builds separate candidate
DLLs, compares the compiled Rust chooser with executable assembly on 12,000
fabricated squads, and runs the actual DLLs against available stock game
module copies. It checks returned entity pointers, rotation state, unchanged
object memory, other features' patch bytes, installation failures, and rollback.
The test-only DLL in `out/pickup-parity` exposes an explicit cursor for comparison;
it is not the candidate to install.

## Try it in the game

Close the game. Back up these two files in `DefianceLoader/plugins`:

- `defiance_plugin_core.dll`
- `defiance_plugin_feature_pickup.dll`

Replace both with their counterparts from `out/pickup-rust/release`. Keep
the installed plugin manifests and configuration files. This requires the
current loader installation; it is not a standalone package. The standard
release build and ZIP are not overwritten by the trial command.

On startup, the loader log should include
`pickup: Rust chooser installed`. Test repeated pickups with
two matching weapon holders, one marked soldier, the whole squad marked,
empty slots, and a squad that cannot pick up weapons. Confirm that querying
pickup targets does not advance the rotation. To revert, close the game and
restore both backed-up DLLs. Changing implementations requires a restart.

## Assurance and limits

Core still chooses and verifies the supported game's exact order-path call
site. The loader owns the hook and any near relay. The three query callers
remain stock, and unknown builds or changed bytes are refused. This adds a
private Core service without changing the public plugin ABI.

The decision matches the assembly's signed slot comparisons and its special
handling of empty members. The process-wide cursor advances only on swaps,
including when the chosen member's entity pointer is null.

Synthetic tests do not prove the reverse-engineered offsets, pointer lifetimes,
or game-thread assumptions. The Rust binding takes a snapshot of member state,
so it assumes the virtual getters are read-only and stable during a call;
their number and order differ from assembly's repeated passes. Rust cannot
make invalid game pointers safe. Null objects and negative slot IDs are
rejected defensively; valid slot IDs must index the game's actual counter array.
The initial chooser has been tested in-game. Future binding changes still need live testing.

To build the assembly fallback DLL for comparison, run:

```powershell
mise exec -- cargo build --release -p defiance-plugin-pickup --no-default-features --target-dir out/pickup-assembly
```
