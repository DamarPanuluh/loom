//! Maturity ladder + compass.
//!
//! Plane: pure computation over the store. The ladder is a vector of rungs, not
//! a scalar; the lowest unmet rung is the routing focus (the compass). Rungs
//! that depend on later-ring data (proofs, debt) report `NotApplicable` until
//! that data exists, so the ladder never lies by counting absent machinery as
//! failure.

use crate::model::{EdgeKind, InspectionStatus, NodeType, TruthClass};
use crate::store::Store;
use crate::Result;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RungState {
    Met,
    Unmet,
    NotApplicable,
}

#[derive(Debug, Clone, Serialize)]
pub struct Rung {
    pub name: String,
    pub state: RungState,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Ladder {
    pub rungs: Vec<Rung>,
    /// The compass phase: the lowest unmet rung's lane.
    pub phase: String,
    /// The single suggested next command.
    pub next_command: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct ValidationSummary {
    pub registered: usize,
    pub passed: usize,
    pub failed: usize,
    pub blocked: usize,
    pub not_run: usize,
    pub other: usize,
}

pub fn validation_summary(store: &Store) -> Result<ValidationSummary> {
    let mut summary = ValidationSummary::default();
    for v in store.list_nodes(Some(NodeType::Validation), usize::MAX)? {
        summary.registered += 1;
        match v.status.as_str() {
            "passed" => summary.passed += 1,
            "failed" => summary.failed += 1,
            "blocked" => summary.blocked += 1,
            "not_run" => summary.not_run += 1,
            _ => summary.other += 1,
        }
    }
    Ok(summary)
}

/// Compute the maturity ladder and compass for the current graph.
pub fn ladder(store: &Store) -> Result<Ladder> {
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?;
    let active: Vec<_> = intents
        .iter()
        .filter(|n| n.status != "deprecated")
        .collect();
    let planned = active
        .iter()
        .filter(|n| n.status == "planned" || n.status == "needs_change")
        .count();
    let implemented: Vec<_> = active
        .iter()
        .filter(|n| n.status == "implemented")
        .collect();

    // grounding: only LEAF intents need code. A hierarchy parent is realized
    // through its children, so it is exempt from the grounding requirement.
    let parents: std::collections::HashSet<String> = store
        .list_edges(Some(EdgeKind::Hierarchy), usize::MAX)?
        .into_iter()
        .map(|e| e.from_id)
        .collect();
    let mut ungrounded = 0usize;
    for n in &implemented {
        if parents.contains(&n.id) {
            continue; // roll-up parent — realized via children
        }
        let impls = store.edges_with(Some(EdgeKind::Implements), Some(&n.id), None)?;
        if impls.is_empty() {
            ungrounded += 1;
        }
    }

    let stale = store
        .edges_by_status(
            TruthClass::Asserted,
            &[
                InspectionStatus::NeedsReverification,
                InspectionStatus::Failing,
            ],
        )?
        .len();
    let uninspected = store
        .edges_by_status(TruthClass::Asserted, &[InspectionStatus::Uninspected])?
        .len();

    // proofs (ring 5): registered validations are not proof until they pass.
    let validations = validation_summary(store)?;

    let open_smells = crate::signal::smells(store)?.len();
    let untriaged = crate::signal::untriaged_findings(store)?.len();
    let stale_findings = crate::signal::stale_findings(store)?.len();
    let rungs = build_rungs(&RungInputs {
        active: active.len(),
        planned,
        ungrounded,
        implemented: implemented.len(),
        validations,
        stale,
        uninspected,
        open_smells,
        untriaged,
        stale_findings,
    });

    // Compass: lowest unmet rung → phase + next command.
    let (phase, next_command) = compass(
        active.len(),
        planned,
        ungrounded,
        implemented.len(),
        &validations,
        stale,
        uninspected,
        open_smells,
        untriaged,
        stale_findings,
    );

    Ok(Ladder {
        rungs,
        phase,
        next_command,
    })
}

#[allow(clippy::too_many_arguments)]
fn compass(
    active: usize,
    planned: usize,
    ungrounded: usize,
    implemented: usize,
    validations: &ValidationSummary,
    stale: usize,
    uninspected: usize,
    open_smells: usize,
    untriaged: usize,
    stale_findings: usize,
) -> (String, String) {
    if active == 0 {
        return (
            "seed".into(),
            "loom door \"<what should this codebase do>\" or loom intent add".into(),
        );
    }
    if stale > 0 {
        return ("fix".into(), "loom next --mode fix".into());
    }
    if planned > 0 || ungrounded > 0 {
        return ("build".into(), "loom next --mode build".into());
    }
    if implemented > 0
        && (validations.registered == 0 || validations.passed < validations.registered)
    {
        return ("validate".into(), "loom next --mode validate".into());
    }
    if uninspected > 0 {
        return ("analyze".into(), "loom next --mode analyze".into());
    }
    if open_smells > 0 {
        return ("audit".into(), "loom smells".into());
    }
    if untriaged > 0 || stale_findings > 0 {
        return ("triage".into(), "loom next --mode triage".into());
    }
    (
        "complete".into(),
        "loom export && loom export --check".into(),
    )
}

/// The scalar counts the rung ladder is computed from.
struct RungInputs {
    active: usize,
    planned: usize,
    ungrounded: usize,
    implemented: usize,
    validations: ValidationSummary,
    stale: usize,
    uninspected: usize,
    open_smells: usize,
    untriaged: usize,
    stale_findings: usize,
}

/// Build the five maturity rungs from the gathered counts.
fn build_rungs(c: &RungInputs) -> Vec<Rung> {
    let mut rungs = Vec::new();
    rungs.push(Rung {
        name: "seeded".into(),
        state: if c.active == 0 {
            RungState::Unmet
        } else {
            RungState::Met
        },
        detail: format!("{} active intent(s)", c.active),
    });
    // Realized: nothing planned, every implemented leaf grounded.
    let realized = c.active > 0 && c.planned == 0 && c.ungrounded == 0;
    rungs.push(Rung {
        name: "realized".into(),
        state: if c.active == 0 {
            RungState::NotApplicable
        } else if realized {
            RungState::Met
        } else {
            RungState::Unmet
        },
        detail: format!("{} unrealized, {} ungrounded", c.planned, c.ungrounded),
    });
    rungs.push(Rung {
        name: "proven".into(),
        state: if c.implemented == 0 {
            RungState::NotApplicable
        } else if c.validations.registered > 0 && c.validations.passed == c.validations.registered {
            RungState::Met
        } else {
            RungState::Unmet
        },
        detail: format!(
            "{} registered: {} passed, {} failed, {} blocked, {} not_run{}",
            c.validations.registered,
            c.validations.passed,
            c.validations.failed,
            c.validations.blocked,
            c.validations.not_run,
            if c.validations.other > 0 {
                format!(", {} other", c.validations.other)
            } else {
                String::new()
            }
        ),
    });
    rungs.push(Rung {
        name: "hardened".into(),
        state: if c.active == 0 {
            RungState::NotApplicable
        } else if c.stale == 0 && c.uninspected == 0 {
            RungState::Met
        } else {
            RungState::Unmet
        },
        detail: format!("{} stale/failing, {} uninspected", c.stale, c.uninspected),
    });
    rungs.push(Rung {
        name: "excellent".into(),
        state: if c.active == 0 {
            RungState::NotApplicable
        } else if c.open_smells == 0 && c.untriaged == 0 && c.stale_findings == 0 {
            RungState::Met
        } else {
            RungState::Unmet
        },
        detail: if c.active == 0 {
            "no active intents yet".into()
        } else {
            format!(
                "{} open smell(s), {} untriaged finding(s), {} stale finding(s)",
                c.open_smells, c.untriaged, c.stale_findings
            )
        },
    });
    rungs
}
