//! The consumer plane: saga specs and their pure-Rust execution engine.
//!
//! A saga is an EXTERNAL-CONSUMER proof — an ordered chain of endpoint
//! invocations that consumes the system the way a real consumer will, with
//! values captured from one response threading into the next request. It is
//! the runtime complement to the read-evidence everywhere else in the graph:
//! RELATES_TO edges are normally grounded by *reading* code; a passing saga
//! stamps the edges along its intent path with *execution* evidence, and a
//! failing step lands as a failing edge naming exactly which boundary broke.
//!
//! Deliberately small. The engine is a saga executor, not a general HTTP
//! testing tool: sequential steps, `{{ var }}` interpolation, JSONPath
//! captures, asserts on status/headers/body. Anything beyond that belongs in
//! an ordinary command-based Validation. Pure Rust by design (reqwest on
//! rustls + serde_json_path) — no libcurl, loom stays one static binary.

pub mod runner;
pub mod spec;

pub use runner::{run_saga, SagaRunReport};
