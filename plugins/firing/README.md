# Firing mode implementation

The setter and UI aggregate are Rust by default. `model.rs` holds the decision
rules; `native.rs` uses Core's `game-access` service, without game-layout offsets
of its own. The manifest declares direct Core and selection dependencies.

Core maps the named `firing_set` and `firing_ui` entries through its verified
build descriptor. The feature installs ordinary entry hooks through the loader;
all remaining firing sites retain the assembly implementation. A failed second
hook rolls the first one back with the rest of the feature's owned patches.

The soldier getter and its in-function call adapter remain assembly because
they resume game code or receive input in nonstandard registers. Shared pin
storage stays compatible with those readers and the other features.

Run `mise run rust-controls-test` for the native reference tests, 12,000
compiled Rust/Core versus assembly comparisons, and actual DLL integration.
Comparisons cover return values and every byte of the fabricated object memory,
including stale markers, disabled/foreign members, absent rosters, and byte
overflow. The new controls still need in-game smoke testing after installation.

Build an assembly control fallback with `--no-default-features` for this crate.
The normal package contains Rust controls. Core and firing must be upgraded
together. All configuration/backend changes require a game restart.
