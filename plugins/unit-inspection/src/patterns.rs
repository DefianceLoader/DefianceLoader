//! The unit info panel's game.dll signatures. tools/plugin-host builds its
//! synthetic game module from these too, so the file stands alone.

/// In the squad setter: the owner from entity vt+0xb0 → `+0x20`, `call
/// [rax+0x80]`, `sete al`, the flag at `+0x28`, then the panel at `+0x18`
/// and its entity at `+0x3f0`.
pub const SQUAD_PATTERN: &str = "ff 90 b0 00 00 00 48 8b 48 20 48 85 c9 74 11 48 8b 01 \
     ff 90 80 00 00 00 84 c0 0f 94 c0 88 47 28 48 8b 47 18 40 b6 01 48 8b 88 f0 03 00 00";
/// Offset in [`SQUAD_PATTERN`] of its `call [rax+0x80]` (six bytes).
pub(crate) const SQUAD_CALL_AT: usize = 0x12;
/// In the ammo menu's refresh: the shown entity in `rsi`, its owner asked
/// `call [rdx+0x80]`, and dropped (`xor esi, esi`) when it answers no.
pub const AMMO_PATTERN: &str = "48 8b 10 48 8b c8 ff 92 b0 00 00 00 48 8b 48 20 48 85 c9 74 0d \
     48 8b 11 ff 92 80 00 00 00 84 c0 75 02 33 f6 48 8b af ?? ?? ?? ?? 48 8b 9f ?? ?? ?? ??";
/// Offset in [`AMMO_PATTERN`] of its `call [rdx+0x80]` (six bytes).
pub(crate) const AMMO_CALL_AT: usize = 0x18;
/// The ammo grid's click handler from its `sub rsp, 0x40` (the expanded ammo
/// menu may hook the bytes before it), through its call to the menu's shown-entity
/// getter (`fn_40c20`).
pub const CLICK_PATTERN: &str = "48 83 ec 40 48 8b f9 45 33 ff 48 8d a9 80 01 00 00 \
     48 8d 85 ?? ?? ?? ?? 48 8b dd 48 3b e8 \
     74 2a 48 39 53 18 74 2d 48 39 53 20 74 1e 48 39 53 30 74 18 48 39 53 38 74 12 48 39 53 28 \
     74 0c 48 81 c3 b8 00 00 00 48 3b d8 75 d6 48 3b d8 0f 84 ?? ?? ?? ?? 48 8b cb e8 ?? ?? ?? ?? \
     84 c0 0f 85 ?? ?? ?? ?? 48 8b f3 48 2b f5 48 c1 fe 03 48 b8 a7 37 bd e9 4d 6f 7a d3 \
     48 0f af f0 8b 6b 08 ff c5 89 6b 08 48 8b cf e8 ?? ?? ?? ??";
/// Offset in [`CLICK_PATTERN`] of its call to the shown-entity getter.
pub(crate) const CLICK_SHOWN_CALL_AT: usize = 0x86;
/// The start of the panel's relation label (`fn_366ad0`): the label at
/// `+0x580` (visible at `+0x5b`, shown by vt+0x48), then `LogicUtilsImpl` at
/// `+0x410` asked vt+0x620, the player's own.
pub const LABEL_PATTERN: &str =
    "48 89 74 24 20 57 48 83 ec 40 48 8b f9 48 8b f2 48 8b 89 80 05 00 00 \
     48 85 c9 74 0e 80 79 5b 00 74 08 48 8b 01 33 d2 ff 50 48 48 8b 8f 10 04 00 00 48 8b d6 \
     48 8b 01 ff 90 20 06 00 00 84 c0 0f 85 ?? ?? ?? ??";
/// The label function's relation calls, at their offsets: ally (`mov r8,
/// [rdx+0x628]`), abandoned, enemy and neutral. They vouch for the relation calls the plugin makes.
pub(crate) const LABEL_SLOTS: [(usize, &[u8]); 4] = [
    (0x18a, &[0x4c, 0x8b, 0x82, 0x28, 0x06, 0, 0]),
    (0x1be, &[0xff, 0x90, 0x40, 0x06, 0, 0]),
    (0x1de, &[0xff, 0x90, 0x30, 0x06, 0, 0]),
    (0x1fe, &[0xff, 0x90, 0x38, 0x06, 0, 0]),
];

/// The start of the ammo menu's `fillSlot(menu, index, record)`: the card at
/// `menu + 0x180 + index * 0xb8`.
pub const FILL_PATTERN: &str = "48 89 5c 24 10 48 89 74 24 18 55 57 41 54 41 56 41 57 \
     48 8d ac 24 60 ff ff ff 48 81 ec a0 01 00 00 49 8b f8 48 8b d9 45 33 e4 44 89 a5 d0 00 00 00 \
     48 69 c2 b8 00 00 00 48 8d b1 80 01 00 00 48 03 f0 41 8b 50 3c 85 d2 0f 84 ?? ?? ?? ?? \
     83 fa 01 74 6c";
