//! Backend-neutral graph analysis helpers.
//!
//! SQLite owns storage and mutation. This module keeps the pure Rust analysis
//! that runs over typed snapshots: queue scoring, doctor checks, search
//! ranking, smells, reports, and vocabulary helpers.

pub mod completeness;
pub mod find;
pub mod integrity;
pub mod intent;
pub mod meta;
pub mod relates_to;
pub mod scoring;
pub mod smells;
pub mod snapshot;
pub mod stats;
pub mod symbol_accountability;
pub mod vocab;

pub use completeness::*;
pub use find::*;
pub use integrity::*;
pub use intent::*;
pub use meta::*;
pub use relates_to::*;
pub use scoring::*;
pub use smells::*;
pub use snapshot::*;
pub use stats::*;
pub use symbol_accountability::*;
pub use vocab::*;
