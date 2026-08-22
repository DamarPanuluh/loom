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
pub mod batch_auth;
pub mod callgraph;
pub mod candidate_surface_policy;
pub mod checkpoint;
pub mod cli;
pub mod commands;
pub mod completeness;
pub mod coverage;
pub mod deriver;
pub mod divergence;
pub mod evidence;
pub mod extract;
pub mod federation;
pub mod fsglob;
pub mod grammar;
pub mod harness;
pub mod identity;
pub mod journal;
pub mod journey;
pub mod journey_exercises;
pub mod journey_gate;
pub mod journey_runtime;
pub mod lane;
pub mod limits;
pub mod locator;
pub mod maturity;
pub mod mcp;
pub mod model;
pub mod packet;
pub mod packs;
pub mod pattern;
pub mod policy;
pub mod prescan;
pub mod proof;
pub mod proofstrength;
pub mod ratification;
pub mod registry;
pub mod release;
pub mod research;
pub mod review;
pub mod risk;
pub mod rolelease;
pub mod runner;
pub mod scan;
pub mod seed;
pub mod signal;
pub mod statistics;
pub mod store;
pub mod subprocess;
pub mod sync;
pub mod text;
pub mod thresholds;
pub mod travel;
pub mod truth;
pub mod workitem;

/// Crate-wide result type. Per the repo `rs-result-type` rule, the error type is
/// a defaulted parameter so call sites stay short while precise errors remain
/// expressible.
pub type Result<T, E = anyhow::Error> = std::result::Result<T, E>;

/// The on-disk schema version stamped into every graph. Bumped when the storage
/// shape or graph vocabulary changes incompatibly.
pub const SCHEMA_VERSION: u32 = 13;

/// Cargo package version of this binary. Distinct from [`SCHEMA_VERSION`]: two
/// builds can share this string and still disagree on schema.
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Meta key: crate version of the last write-open. Cannot see same-crate schema
/// forks by itself; pair with [`WRITER_SCHEMA_KEY`].
pub const WRITER_VERSION_KEY: &str = "loom_writer_version";

/// Meta key: [`SCHEMA_VERSION`] of the last write-open.
pub const WRITER_SCHEMA_KEY: &str = "loom_writer_schema";

/// Schema versions below this predate the journey-root cut and cannot be
/// migrated. Versions in `[JOURNEY_SCHEMA_CUT, SCHEMA_VERSION)` may migrate
/// (with consent when the crate version did not increase).
pub const JOURNEY_SCHEMA_CUT: u32 = 12;

/// Directory holding the local graph store, relative to a project root.
pub const LOOM_DIR: &str = ".loom";

/// The SQLite graph store filename within `LOOM_DIR`.
pub const GRAPH_DB: &str = "graph.sqlite";

/// The committed, portable export filename at the project root.
pub const GRAPH_EXPORT: &str = "loom.graph.json";

/// Process exit code for infrastructure contention (graph or proof-harness
/// lock), distinct from a failed proof or command error.
pub const LOCK_CONTENTION_EXIT_CODE: i32 = 75;
