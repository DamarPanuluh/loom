//! Compiler-owned runtime for semantic Journeys.
//!
//! The authored Journey remains transport-free. This module compiles an
//! accepted CLI projection into deterministic data, then executes only direct
//! argv arrays. It has no shell or HTTP execution path.

mod artifacts;
mod compile;
mod continuation;
mod execute;
mod observation;
mod process;
mod temporal;
mod types;
mod values;

pub use artifacts::{
    baseline_current, baseline_path, cache_matches, proof_path, write_baseline, write_proof,
};
pub use compile::{canonical_bytes, compile, compile_surface, compile_with_setup};
pub use continuation::{pending_continuation, resume_interactive};
pub(crate) use execute::execute_interactive_with_anchors;
pub use execute::{execute, execute_interactive, execute_observed};
pub use observation::{ExecutableBoundary, JourneyObservation};
// Surfaced by `loom limits`: both bound what a worker reads back from a journey run.
pub(crate) use types::{FAILURE_DIAGNOSTIC_BYTES, STREAM_EXCERPT_BYTES};
pub(crate) use process::resolve_trusted_executable;
pub use types::{
    CompiledHumanDecision, CompiledJourneyProof, CompiledProfileShape, CompiledSetup,
    CompiledSetupOperation, CompiledStep, ExecutionOutcome, FailedAssertion, FailedCheckKind,
    FileTransitionReport, JourneyBaseline, PassedAssertion, PendingContinuation, RuntimeReport,
    RuntimeStatus, SetupReport, StepReport, EXECUTOR_PLATFORM_ENVIRONMENT,
};
pub use values::{parse_overrides, report_observation_json};
