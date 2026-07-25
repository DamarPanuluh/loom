//! loom v2 — a falsifiable graph of what a codebase should do, where it lives,
//! and how it is proven.
//!
//! Plane: crate root — module wiring and shared vocabulary (`Result`, schema
//! and path constants) only; each module below states its own plane.
//!
//! Architecture (rings, see docs/build-plan.md):
//! - `model`    — graph vocabulary: nodes, edges, facets, canonical enums.
//! - `registry` — the edge-kind type system (endpoint + truth-class rules).
//! - `store`    — SQLite persistence behind a focused interface.
//! - `travel`   — deterministic export / two-phase import.
//! - `commands` — CLI command handlers.
//! - `cli`      — clap command surface.

pub mod absorb;
pub mod anchor;
pub mod artifact;
pub mod audit;
pub mod callgraph;
pub mod cli;
pub mod commands;
pub mod completeness;
pub mod deriver;
pub mod divergence;
pub mod evidence;
pub mod extract;
pub mod federation;
pub mod fsglob;
pub mod grammar;
pub mod journal;
pub mod journey;
pub mod lane;
pub mod maturity;
pub mod mcp;
pub mod model;
pub mod packet;
pub mod packs;
pub mod policy;
pub mod prescan;
pub mod proof;
pub mod proofstrength;
pub mod ratification;
pub mod registry;
pub mod risk;
pub mod runner;
pub mod scan;
pub mod seed;
pub mod signal;
pub mod store;
pub mod sync;
pub mod thresholds;
pub mod travel;
pub mod truth;
pub mod workitem;

/// Crate-wide result type. Per the repo `rs-result-type` rule, the error type is
/// a defaulted parameter so call sites stay short while precise errors remain
/// expressible.
pub type Result<T, E = anyhow::Error> = std::result::Result<T, E>;

/// The on-disk schema version stamped into every graph. Bumped when the SQLite
/// schema changes in a way that requires migration.
pub const SCHEMA_VERSION: u32 = 3;

/// Directory holding the local graph store, relative to a project root.
pub const LOOM_DIR: &str = ".loom";

/// The SQLite graph store filename within `LOOM_DIR`.
pub const GRAPH_DB: &str = "graph.sqlite";

/// The committed, portable export filename at the project root.
pub const GRAPH_EXPORT: &str = "loom.graph.json";
