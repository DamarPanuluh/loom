//! Backend-neutral graph analysis helpers.
//!
//! SQLite owns storage and mutation. This module keeps the pure Rust analysis
//! that runs over typed snapshots: queue scoring, doctor checks, search
//! ranking, smells, reports, and vocabulary helpers.

pub mod calibrate;
pub mod completeness;
pub mod composition;
pub mod comprehensiveness;
pub mod corpus;
pub mod find;
pub mod graph_algo;
pub mod integrity;
pub mod intent;
pub mod maturity;
pub mod meta;
pub mod relates_to;
pub mod scoring;
pub mod smells;
pub mod snapshot;
pub mod stats;
pub mod symbol_accountability;
pub mod symbol_match;
pub mod vocab;

pub use completeness::*;
pub use composition::*;
pub use corpus::*;
pub use find::*;
pub use integrity::*;
pub use intent::*;
pub use maturity::*;
pub use meta::*;
pub use relates_to::*;
pub use scoring::*;
pub use smells::*;
pub use snapshot::*;
pub use stats::*;
pub use symbol_accountability::*;
pub use symbol_match::*;
pub use vocab::*;
