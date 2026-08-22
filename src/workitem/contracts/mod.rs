//! Role contracts — the PromptContract text for every work-item lane.
//!
//! Plane: judgment-plane routing (text assembly only). Each contract states
//! the role, mindset, allowed and forbidden actions, required evidence, and
//! the exact prefilled write-back command for one lane — the write-back must
//! target the store's gated paths, so a contract can never instruct a way
//! around INV-4/5/6. No store writes happen here.

mod build;
mod inspect;
mod journey;
mod quality;
mod repair;
mod triage;
mod validate;

use super::q;
use crate::model::{Edge, EdgeKind};

const FINDING_ADD_ACTION: &str = "loom finding add '<claim>' --source code_audit --kind code_audit --file <registered-codefile> --evidence '<file:line — observed fact>' --impact '<why it matters>' --confidence <0.0-1.0>";
const NON_BLOCKING_SMELL_RULE: &str = "silently skipping a material non-blocking smell; either capture it as a finding, reject it with evidence in triage, or leave it unmentioned because it is below capture threshold";

// ---- role contracts (see docs/llm-driver.md) -------------------------------

/// The exact re-record command that closes THIS edge's verdict, prefilled with
/// real endpoint names. Relates claims use the ergonomic name-resolving
/// `edge explore`; every other relationship kind is re-recorded by edge id —
/// `edge explore` would silently target a different (relates) edge.
pub(super) fn verdict_write_back(edge: &Edge, from: &str, to: &str) -> String {
    match edge.kind {
        EdgeKind::Governs => format!(
            "loom rule verdict {} {} <passing|failing|independent> --criterion '…' --evidence '…' --confidence <0.0-1.0>",
            q(from),
            q(to)
        ),
        EdgeKind::Validates => format!(
            "loom validation verdict {} <passed|failed|blocked> --evidence '…'   (blocked: add --reason '…')",
            q(from)
        ),
        EdgeKind::Relates => format!(
            "loom edge explore {} {} <ground|issue|independent> --criterion '…' --evidence '…' --confidence <0.0-1.0>",
            q(from),
            q(to)
        ),
        _ => format!(
            "loom edge verdict {} <ground|issue|independent> --criterion '…' --evidence '…' --confidence <0.0-1.0>",
            edge.id
        ),
    }
}

pub(super) use build::{builder_contract, coverage_contract, missing_codefile_contract};
pub(super) use inspect::{
    adversarial_reviewer_contract, analyzer_contract, exemplar_contract, prove_contract,
    research_contract, reviewer_contract,
};
pub(super) use journey::{derive_contract, elaborator_contract, surface_contract};
pub(super) use quality::{quality_contract, quality_contract_body};
pub(super) use repair::{
    fixer_contract, needed_finding_analyze_contract, needed_finding_fix_contract,
    needed_finding_validate_contract,
};
pub(super) use triage::{
    inbox_triage_contract, ratify_contract, rectify_contract, structural_finding_triage_contract,
    triage_contract,
};
pub(super) use validate::{
    audit_contract, deepen_contract, journey_proof_contract, journey_proof_contract_for_profile,
    unproven_contract, validator_contract,
};
