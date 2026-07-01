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

    // proofs (ring 5): validations linked to implemented intents.
    let validations = store
        .list_nodes(Some(NodeType::Validation), usize::MAX)?
        .len();

    let open_smells = crate::signal::smells(store)?.len();
    let untriaged = crate::signal::untriaged_findings(store)?.len();
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
    });

    // Compass: lowest unmet rung → phase + next command.
    let (phase, next_command) = compass(
        active.len(),
        planned,
        ungrounded,
        stale,
        uninspected,
        open_smells,
        untriaged,
    );

    Ok(Ladder {
        rungs,
        phase,
        next_command,
    })
}

fn compass(
    active: usize,
    planned: usize,
    ungrounded: usize,
    stale: usize,
    uninspected: usize,
    open_smells: usize,
    untriaged: usize,
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
    if uninspected > 0 {
        return ("analyze".into(), "loom next --mode analyze".into());
    }
    if open_smells > 0 {
        return ("audit".into(), "loom smells".into());
    }
    if untriaged > 0 {
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
    validations: usize,
    stale: usize,
    uninspected: usize,
    open_smells: usize,
    untriaged: usize,
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
        } else if c.validations == 0 {
            RungState::Unmet
        } else {
            RungState::Met
        },
        detail: format!("{} validation(s)", c.validations),
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
        } else if c.open_smells == 0 && c.untriaged == 0 {
            RungState::Met
        } else {
            RungState::Unmet
        },
        detail: if c.active == 0 {
            "no active intents yet".into()
        } else {
            format!(
                "{} open smell(s), {} untriaged finding(s)",
                c.open_smells, c.untriaged
            )
        },
    });
    rungs
}
