//! The patching machinery that does not need a target process, shared by the
//! injector and the loader.
//!
//! It began as `injector/src/relocate.rs` and `injector/src/main.rs` mixed
//! together. Now the descriptor types and parser (`descriptor`), the masked
//! signature scan (`scan`, `pattern`), the signature relocation (`relocate`),
//! the writes into a module (`apply`), the policy that picks between them
//! (`install`) and the PE reader (`pe`) all live here once. The injector is a
//! front-end that discovers the process and passes the payloads it embeds; the
//! loader's core plugin does the same with the game's own modules.

pub mod apply;
pub mod decode;
pub mod descriptor;
pub mod features;
pub mod install;
pub mod pattern;
pub mod pe;
pub mod relocate;
pub mod report;
pub mod rtti;
pub mod scan;
pub mod sha256;

pub use apply::{apply, apply_game, module_image, Target};
pub use descriptor::{GamePatch, Patch};
pub use install::{
    game_for_build, install_game, install_logic, logic_for_build, needs_relocation, relocate_game,
    relocate_logic, Applied, Scan,
};
pub use pattern::parse as parse_pattern;
pub use scan::{scan, Moves, Site};
