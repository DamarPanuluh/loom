//! Proof command family — quality rules, validations, and their verdicts.
//!
//! Plane: CLI surface over the judgment plane. Every settled state written
//! here flows through `Store::record_verdict`, so the evidence gates
//! (INV-4/5/6) and the role gate (INV-7) apply unchanged — this module shapes
//! arguments, resolves names to nodes/edges, and renders output; it must never
//! offer a path around the store's write boundary.

pub(crate) use super::*;

mod observe;
mod rule;
mod support;
mod validate;
mod validation;

pub(crate) use observe::observe_run;
pub(crate) use rule::rule;
pub(crate) use support::{
    mark_validation, regrade, validation_targets, warn_if_command_already_proves_another,
    ProofCommandCollision,
};
pub use support::{observe_validation, prove_intent};
pub(crate) use validate::{observe_cmd, validate_cmd};
pub(crate) use validation::validation;
