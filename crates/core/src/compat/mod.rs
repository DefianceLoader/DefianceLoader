//! Injector compatibility: the descriptor protocol the direct injector reads
//! (`tools/payload.py` and `tools/icon.py` write it) and the policy around it.
//!
//! [`descriptor`] parses the feature-specific fields (selection, movement,
//! ammo, posture, pickup), [`relocate`] moves them to a build by signature,
//! [`apply`] writes them and holds the self tests, and [`install`] is the
//! "try as built, fall back to signatures" sequence. Only `injector/` uses this
//! module. The loader and the plugins use the shared primitives beside it
//! ([`crate::apply::Process`], [`mod@crate::scan`], [`crate::decode`],
//! [`crate::pe`], [`crate::install::needs_relocation`]) and never this schema.

pub mod apply;
pub mod descriptor;
pub mod install;
pub mod relocate;

pub use descriptor::{GamePatch, Patch};
