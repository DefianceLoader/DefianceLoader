# Selection implementation

The soldier's `setSelected` and `isSelected` methods are Rust by default.
`native.rs` preserves the assembly rules: a stale/non-pointer parent uses the
soldier's own mark, marking into a deselected squad clears stale sibling marks,
and removing the last enabled sibling mark deselects the squad. An unavailable
roster conservatively keeps the squad selected when unmarking.

Core supplies roster copies and member snapshots through `game-access` v1.
Selection owns its feature-specific mark/parent operations and provides the
public `selection` v1 query used by regroup. Getter calls must remain read-only
and game objects must stay live during each operation.

The remaining manager, region-selection, world-input and order integration
patches remain assembly. Their register/stack-specific calling conventions are
not ordinary function replacements. The exact reference assembly is retained
in `patch/soldier-mark.asm` and the other patch files.

Run `mise run rust-controls-test`. It compares the compiled Rust setter/getter
and actual Core bindings with the full assembly functions on 12,000 randomized
fixtures, checking complete memory and parent-forwarding counts. Actual DLL
tests cover both absolute and near-relay entry branches plus failed-init rollback.
These new methods still need an in-game smoke test.

An assembly fallback can be built for this crate with `--no-default-features`.
Upgrade loader, Core, selection, and dependent plugins together and restart.
