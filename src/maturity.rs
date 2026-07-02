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
    /// The truth axis this phase is about (which form of truth is stale/missing).
    /// `None` only for the terminal `complete` phase.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truth_axis: Option<crate::truth::TruthAxis>,
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

fn unowned_registered_codefiles(store: &Store) -> Result<usize> {
    // Single source of truth (ignore-aware) shared with the coverage diagnostic
    // and the coverage work queue.
    Ok(crate::commands::unowned_codefiles(store)?.len())
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
    let mut unproven_implemented = 0usize;
    for n in &implemented {
        if parents.contains(&n.id) {
            continue; // roll-up parent — proven through child leaves
        }
        let proofs = store.edges_with(Some(EdgeKind::Validates), None, Some(&n.id))?;
        if !proofs.iter().any(|e| e.status == InspectionStatus::Passing) {
            unproven_implemented += 1;
        }
    }

    let smells = crate::signal::smells(store)?;
    let open_smells = smells.len();
    let open_journey_proof_smells = smells
        .iter()
        .filter(|s| s.kind == "missing_journey_proof" || s.kind == "proof_too_shallow_for_intent")
        .count();
    let untriaged = crate::signal::untriaged_findings(store)?.len();
    let stale_findings = crate::signal::stale_findings(store)?.len();
    let unowned_codefiles = unowned_registered_codefiles(store)?;
    let doctor_issues = crate::signal::doctor(store)?.len();
    let export_fresh = crate::travel::export_is_fresh(store)?;
    let rungs = build_rungs(&RungInputs {
        active: active.len(),
        planned,
        ungrounded,
        unowned_codefiles,
        implemented: implemented.len(),
        validations,
        unproven_implemented,
        stale,
        uninspected,
        doctor_issues,
        open_smells,
        open_journey_proof_smells,
        untriaged,
        stale_findings,
        export_fresh,
    });

    // Compass: lowest unmet rung → phase + next command. Routing follows the
    // EXACT queue partition (`queue_counts`) so the compass never points a
    // lane at work that `loom next --mode <m>` would not serve.
    let queues = crate::workitem::queue_counts(store)?;
    let (phase, next_command) = compass(
        active.len(),
        planned,
        ungrounded,
        unowned_codefiles,
        implemented.len(),
        unproven_implemented,
        &validations,
        &queues,
        doctor_issues,
        open_smells,
        open_journey_proof_smells,
        untriaged,
        stale_findings,
        export_fresh,
    );

    let truth_axis = crate::truth::axis_for_phase(&phase, &next_command);
    Ok(Ladder {
        rungs,
        phase,
        next_command,
        truth_axis,
    })
}

#[allow(clippy::too_many_arguments)]
fn compass(
    active: usize,
    planned: usize,
    ungrounded: usize,
    unowned_codefiles: usize,
    implemented: usize,
    unproven_implemented: usize,
    validations: &ValidationSummary,
    queues: &crate::workitem::QueueCounts,
    doctor_issues: usize,
    open_smells: usize,
    open_journey_proof_smells: usize,
    untriaged: usize,
    stale_findings: usize,
    export_fresh: bool,
) -> (String, String) {
    if active == 0 {
        return (
            "seed".into(),
            "loom door \"<what should this codebase do>\" or loom intent add".into(),
        );
    }
    if queues.fix > 0 {
        return ("fix".into(), "loom next --mode fix".into());
    }
    if planned > 0 || ungrounded > 0 {
        return ("build".into(), "loom next --mode build".into());
    }
    if unowned_codefiles > 0 {
        return ("coverage".into(), "loom coverage".into());
    }
    if queues.validate > 0
        || (implemented > 0
            && (validations.registered == 0
                || validations.passed < validations.registered
                || unproven_implemented > 0
                || open_journey_proof_smells > 0))
    {
        return ("validate".into(), "loom next --mode validate".into());
    }
    if queues.quality > 0 {
        return ("quality".into(), "loom next --mode quality".into());
    }
    if queues.analyze > 0 {
        return ("analyze".into(), "loom next --mode analyze".into());
    }
    if doctor_issues > 0 {
        return ("audit".into(), "loom doctor".into());
    }
    if open_smells > 0 {
        return ("audit".into(), "loom smells".into());
    }
    if untriaged > 0 || stale_findings > 0 {
        return ("triage".into(), "loom next --mode triage".into());
    }
    if !export_fresh {
        return ("export".into(), "loom export && loom export --check".into());
    }
    ("complete".into(), "loom status".into())
}

/// The scalar counts the rung ladder is computed from.
struct RungInputs {
    active: usize,
    planned: usize,
    ungrounded: usize,
    unowned_codefiles: usize,
    implemented: usize,
    validations: ValidationSummary,
    unproven_implemented: usize,
    stale: usize,
    uninspected: usize,
    doctor_issues: usize,
    open_smells: usize,
    open_journey_proof_smells: usize,
    untriaged: usize,
    stale_findings: usize,
    export_fresh: bool,
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
    // Realized: nothing planned, every implemented leaf grounded, every
    // registered CodeFile owned by at least one intent (or removed/ignored).
    let realized = c.active > 0 && c.planned == 0 && c.ungrounded == 0 && c.unowned_codefiles == 0;
    rungs.push(Rung {
        name: "realized".into(),
        state: if c.active == 0 {
            RungState::NotApplicable
        } else if realized {
            RungState::Met
        } else {
            RungState::Unmet
        },
        detail: format!(
            "{} unrealized, {} ungrounded, {} unowned codefile(s)",
            c.planned, c.ungrounded, c.unowned_codefiles
        ),
    });
    rungs.push(Rung {
        name: "proven".into(),
        state: if c.implemented == 0 {
            RungState::NotApplicable
        } else if c.validations.registered > 0
            && c.validations.passed == c.validations.registered
            && c.unproven_implemented == 0
            && c.open_journey_proof_smells == 0
        {
            RungState::Met
        } else {
            RungState::Unmet
        },
        detail: format!(
            "{} registered: {} passed, {} failed, {} blocked, {} not_run, {} unproven implemented intent(s){}{}",
            c.validations.registered,
            c.validations.passed,
            c.validations.failed,
            c.validations.blocked,
            c.validations.not_run,
            c.unproven_implemented,
            if c.validations.other > 0 {
                format!(", {} other", c.validations.other)
            } else {
                String::new()
            },
            if c.open_journey_proof_smells > 0 {
                format!(", {} journey proof gap(s)", c.open_journey_proof_smells)
            } else {
                String::new()
            }
        ),
    });
    rungs.push(Rung {
        name: "hardened".into(),
        state: if c.active == 0 {
            RungState::NotApplicable
        } else if c.stale == 0 && c.uninspected == 0 && c.doctor_issues == 0 {
            RungState::Met
        } else {
            RungState::Unmet
        },
        detail: format!(
            "{} stale/failing, {} uninspected, {} doctor issue(s)",
            c.stale, c.uninspected, c.doctor_issues
        ),
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
    rungs.push(Rung {
        name: "exported".into(),
        state: if c.active == 0 {
            RungState::NotApplicable
        } else if c.export_fresh {
            RungState::Met
        } else {
            RungState::Unmet
        },
        detail: if c.active == 0 {
            "no active intents yet".into()
        } else if c.export_fresh {
            "loom.graph.json fresh".into()
        } else {
            "loom.graph.json missing or stale".into()
        },
    });
    rungs
}
