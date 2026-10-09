//! The patching machinery that does not need a target process, shared by the
//! injector and the loader.
//!
//! The masked signature scan (`scan`, `pattern`), process memory and module
//! writes (`apply`), the relocation policy (`install`), instruction decoding
//! (`decode`), the PE reader (`pe`), RTTI lookup and the unit format live here
//! once, for both. The injector's descriptor protocol and its install sequence
//! sit apart in [`compat`], which the loader and plugins do not use.

pub mod apply;
pub mod compat;
pub mod decode;
pub mod install;
pub mod json;
pub mod pattern;
pub mod pe;
pub mod report;
pub mod rtti;
pub mod scan;
pub mod sha256;
pub mod sites;
pub mod unit;
pub mod unit_apply;
#[cfg(test)]
mod unit_apply_tests;

pub use apply::{module_image, Target};
pub use install::{needs_relocation, Scan};
pub use pattern::parse as parse_pattern;
pub use scan::{scan, Moves, Site};
