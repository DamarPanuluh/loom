//! Subcommand argument shapes for every `loom` command family.
//!
//! Plane: surface — argument shape only. These enums declare flag names,
//! defaults, and help text; every handler lives in `crate::commands`. Nothing
//! here opens a store, resolves a graph, or contains logic beyond clap parsing.

mod capture;
mod codefile;
mod diagnostics;
mod domain;
mod edge;
mod intent;
mod journey;
mod ops;
mod pattern;
mod proof;
mod proposal;
mod release;

pub use capture::*;
pub use codefile::*;
pub use diagnostics::*;
pub use domain::*;
pub use edge::*;
pub use intent::*;
pub use journey::*;
pub use ops::*;
pub use pattern::*;
pub use proof::*;
pub use proposal::*;
pub use release::*;
