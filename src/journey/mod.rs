//! Strict semantic Journey artifacts and their accepted projections.
//!
//! A Journey is authored without transport or implementation detail. It says
//! who does what, in which order, and what must then be true. Technical intents
//! and reusable CLI surfaces are separate, hash-bound projections accepted by
//! dedicated commands.

mod compile;
mod derivation;
mod lint;
mod sources;
mod spec;
mod surface_manifest;
mod surface_ops;
mod surface_setup;

pub const SURFACE_SCHEMA: &str = "loom.journey.surface/v1";
pub const INTERFACE_SURFACE_SCHEMA: &str = "loom.interface-surface/v1";
pub const COMPILED_PROOF_SCHEMA: &str = "loom.journey.proof/v1";
pub const BASELINE_SCHEMA: &str = "loom.journey.baseline/v1";
pub const JOURNEY_COMPILER_VERSION: &str = "6";

pub use compile::{
    resettle_uninspected_compiler_topology, resume_and_settle_compiled_validation,
    run_and_settle_compiled_validation, run_interactive_and_settle_compiled_validation,
    settle_compiled_validation, surface_projection_hash, InteractiveJourneyRun,
};
pub use derivation::{
    surface_contract_template, DerivationManifest, DerivationQuestion, DerivedIntent,
    DerivedIntentOperation, DerivedRelationship, DerivedRelationshipKind, DERIVATION_SCHEMA,
};
pub use lint::{
    JourneyLintFinding, JourneyLintReport, JourneyLintSeverity, JOURNEY_LINT_REPORT_SCHEMA,
};
pub(crate) use sources::{
    argv_token_source, parse_runtime_source, resolve_pointer, Resolved, RuntimeSource,
};
pub use spec::{
    parse, proof_profiles, validate_stable_id, JourneyInput, JourneyOutput, JourneyProfile,
    JourneySpec, JourneyStep, ProfileInputBinding, TemporaryFile, TemporarySetup, ValueType,
    DEFAULT_JOURNEY_TIMEOUT_SECONDS, JOURNEY_SCHEMA,
};
pub(crate) use spec::{parse_typed_text, template_references, validate_process_environment_name};
pub use surface_manifest::SurfaceManifest;
pub use surface_ops::{
    CliOperation, HumanDecisionBinding, HumanDecisionSource, InterfaceSurfaceDefinition,
    JourneyOperationExerciseFacet, OperationArgument, OperationBinding, OperationExercise,
    OperationOutput, OutputAssertion, OutputCapture, OutputFormat, SurfaceBinding,
};
pub use surface_setup::{
    SetupGraph, SurfaceFileAction, SurfaceGitMode, SurfaceGitSetup, SurfaceSetup,
};
