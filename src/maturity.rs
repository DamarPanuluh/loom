//! Maturity ladder + compass.
//!
//! Plane: one gather over the store, then pure projection through
//! [`crate::lane::Lane`]. The ladder is a vector of rungs, not a scalar; the
//! lowest unmet rung is the routing focus (the compass).
//!
//! Contract: the compass is a PROJECTION of the ladder, not a second decision.
//! `Unmet ⟺ Lane::depth > 0`, so the compass can never point at a lane whose
//! queue `loom next --mode <m>` would find empty. Rungs whose machinery does not
//! exist yet report `NotApplicable`, so the ladder never counts absent machinery
//! as failure; `NotApplicable` is transparent to the gate.

use crate::lane::{LadderInputs, Lane, QueueDepths};
use crate::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use crate::store::Store;
use crate::Result;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RungState {
    Met,
    Unmet,
    NotApplicable,
    /// Never completes, by design. Reserved for the post-floor `deepen` rung:
    /// risk work re-ranks rather than draining, so "done" is not one of its
    /// states.
    Open,
}

#[derive(Debug, Clone, Serialize)]
pub struct Rung {
    pub name: String,
    /// The lane that serves this rung. Every rung has exactly one.
    pub lane: Lane,
    pub state: RungState,
    /// The queue depth that produced `state` — `Unmet` iff this is non-zero.
    pub depth: usize,
    pub detail: String,
    /// Derived: this rung sits above the lowest Unmet rung, so it is unreachable
    /// until that gate is met. Presentation-only — `state` still reports this
    /// rung's own per-concern truth.
    #[serde(default)]
    pub blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Ladder {
    pub rungs: Vec<Rung>,
    /// The compass phase: the lowest unmet rung's lane.
    pub phase: String,
    /// The lowest unmet rung's name.
    pub rung: String,
    /// The single suggested next command.
    pub next_command: String,
    /// The truth axis this phase is about (which form of truth is stale/missing).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truth_axis: Option<crate::truth::TruthAxis>,
    /// The derived-floor balance: how much of the graph is machine-maintained
    /// (derived) versus judgment the queue must carry (asserted).
    pub derived_floor: DerivedFloor,
}

/// The ratio of derived to asserted facts, surfaced so a thin programmatic
/// floor stays a visible measured number instead of silently growing the
/// judgment queue. Facts are counted across nodes, edges, and facets.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct DerivedFloor {
    pub derived: usize,
    pub asserted: usize,
    /// `derived / (derived + asserted)`; `0.0` for an empty graph.
    pub ratio: f64,
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

impl LadderInputs {
    /// The ONE gather. Every rung, the compass, and every queue depth read from
    /// this single snapshot, so two predicates can never drift apart.
    pub fn gather(store: &Store) -> Result<LadderInputs> {
        let identity = store.identity()?;
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

        // Grounding: only LEAF intents need code. A hierarchy parent is realized
        // through its children, so it is exempt from the grounding requirement.
        let parents: std::collections::HashSet<String> = store
            .list_edges(Some(EdgeKind::Hierarchy), usize::MAX)?
            .into_iter()
            .map(|e| e.from_id)
            .collect();

        // Edge residue, split exactly the way the lanes serve it.
        let failing = store
            .live_edges_by_status(TruthClass::Asserted, &[InspectionStatus::Failing])?
            .len();
        let stale = store.live_edges_by_status(
            TruthClass::Asserted,
            &[InspectionStatus::NeedsReverification],
        )?;
        let uninspected =
            store.live_edges_by_status(TruthClass::Asserted, &[InspectionStatus::Uninspected])?;
        let split = |edges: &[crate::model::Edge]| -> (usize, usize, usize) {
            let governs = edges.iter().filter(|e| e.kind == EdgeKind::Governs).count();
            let validates = edges
                .iter()
                .filter(|e| e.kind == EdgeKind::Validates)
                .count();
            (edges.len() - governs - validates, governs, validates)
        };
        let (stale_relationships, stale_governs, stale_validates) = split(&stale);
        let (uninspected_relationships, uninspected_governs, uninspected_validates) =
            split(&uninspected);

        // Proofs: registered validations are not proof until they pass.
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
        // A resolving adjudication is an accepted exception and no longer counts
        // as open — honoring the close-out contract "open smells fixed *or
        // adjudicated*". `needed`/`blocked`/untriaged smells still count.
        let mut open_smells = 0usize;
        for s in &smells {
            if !crate::signal::smell_has_resolving_adjudication(store, &s.identity)? {
                open_smells += 1;
            }
        }
        // Journey proof gaps block `proven` unless the intent's journey axis is
        // deliberately waived. (Both the waiver and this suppression are slated
        // for deletion once proof strength is derived rather than asserted.)
        let mut open_journey_proof_smells = 0usize;
        for s in &smells {
            if s.kind != "missing_journey_proof" && s.kind != "proof_too_shallow_for_intent" {
                continue;
            }
            let intent_id = s.identity.rsplit_once(':').map(|(_, id)| id).unwrap_or("");
            if intent_id.is_empty() {
                open_journey_proof_smells += 1;
                continue;
            }
            let waived = store
                .get_facet(intent_id, TargetKind::Node, "waiver:journey")?
                .map(|r| !r.is_empty())
                .unwrap_or(false);
            if !waived {
                open_journey_proof_smells += 1;
            }
        }

        let floor = crate::policy::load(store)?.review_confidence_floor;
        let low_confidence = store
            .live_edges_by_status(
                TruthClass::Asserted,
                &[InspectionStatus::Passing, InspectionStatus::Independent],
            )?
            .into_iter()
            .filter(|e| e.confidence > 0.0 && e.confidence < floor)
            .count();

        Ok(LadderInputs {
            observed: identity.observed,
            active: active.len(),
            implemented: implemented.len(),
            codefiles: store
                .list_nodes(Some(NodeType::CodeFile), usize::MAX)?
                .len(),
            planned,
            // Single source of truth with the build lane: the same predicate the
            // build queue serves, so a non-zero rung always has a work item.
            ungrounded: crate::workitem::ungrounded_implemented_intents(store)?.len(),
            unowned_codefiles: crate::commands::unowned_codefiles(store)?.len(),
            failing,
            stale_relationships,
            uninspected_relationships,
            stale_governs,
            uninspected_governs,
            stale_validates,
            uninspected_validates,
            unmeasured_quality_pairs: crate::workitem::unmeasured_quality_pairs(store)?.len(),
            validations,
            unproven_implemented,
            open_journey_proof_smells,
            low_confidence,
            triage_findings: crate::signal::triage_findings(store)?.len(),
            inbox_new: store
                .list_nodes(Some(NodeType::InboxItem), usize::MAX)?
                .into_iter()
                .filter(|n| n.status == "new")
                .count(),
            untriaged_findings: crate::signal::untriaged_findings(store)?.len(),
            stale_findings: crate::signal::stale_findings(store)?.len(),
            proposed_hypotheses: store
                .nodes_by_status(NodeType::Hypothesis, &["proposed"])?
                .len(),
            open_elaborations: crate::completeness::all_scorecards(store)?
                .iter()
                .filter(|c| c.open > 0 && c.visibility.as_deref() == Some("user_visible"))
                .count(),
            divergences: crate::workitem::unratified_intents(store)?.len(),
            doctor_issues: crate::signal::doctor(store)?.len(),
            open_smells,
            export_fresh: crate::travel::export_is_fresh(store)?,
        })
    }
}

/// Per-lane backlog depths. The status surface reads this, so it can never
/// disagree with what `loom next --mode <m>` would serve.
pub fn depths(store: &Store) -> Result<QueueDepths> {
    Ok(QueueDepths::from_inputs(&LadderInputs::gather(store)?))
}

/// Compute the maturity ladder and compass for the current graph.
pub fn ladder(store: &Store) -> Result<Ladder> {
    let inputs = LadderInputs::gather(store)?;
    let rungs = build_rungs(&inputs);
    let (phase, rung, next_command, truth_axis) = compass(&rungs);

    let (derived_facts, asserted_facts) = store.truth_class_census()?;
    let total = derived_facts + asserted_facts;
    let derived_floor = DerivedFloor {
        derived: derived_facts,
        asserted: asserted_facts,
        ratio: if total == 0 {
            0.0
        } else {
            derived_facts as f64 / total as f64
        },
    };
    Ok(Ladder {
        rungs,
        phase,
        rung,
        next_command,
        truth_axis,
        derived_floor,
    })
}

/// One rung per lane, in `Lane::LADDER` order, each `Unmet` iff its queue is
/// non-empty.
pub fn build_rungs(c: &LadderInputs) -> Vec<Rung> {
    let mut rungs: Vec<Rung> = Lane::LADDER
        .iter()
        .map(|&lane| {
            let depth = lane.depth(c);
            let state = if lane.not_applicable(c) {
                RungState::NotApplicable
            } else if depth == 0 {
                RungState::Met
            } else {
                RungState::Unmet
            };
            Rung {
                name: lane.rung().to_string(),
                lane,
                state,
                depth,
                detail: lane.detail(c),
                blocked: false,
                blocked_by: None,
            }
        })
        .collect();

    // Gate = the lowest Unmet rung; NotApplicable is transparent (its machinery
    // doesn't exist yet, so it can't be a prerequisite). Every rung above the
    // gate is unreachable until the gate is met — mark it blocked so the display
    // never shows a higher rung as satisfied above an unmet lower rung.
    if let Some(g) = rungs.iter().position(|r| r.state == RungState::Unmet) {
        let gate = rungs[g].name.clone();
        for r in rungs.iter_mut().skip(g + 1) {
            r.blocked = true;
            r.blocked_by = Some(gate.clone());
        }
    }
    rungs
}

/// The compass is not a decision — it is a projection of the ladder. Returns
/// `(phase, rung, next_command, truth_axis)`.
///
/// TODO(deepen): when the `deepen` lane joins `Lane::LADDER` it becomes the
/// permanent fallthrough and this terminal `complete` arm goes away — a tool
/// whose thesis is "every command's output is the prompt for the next decision"
/// should not end by telling you to re-read itself.
pub fn compass(rungs: &[Rung]) -> (String, String, String, Option<crate::truth::TruthAxis>) {
    match rungs
        .iter()
        .find(|r| matches!(r.state, RungState::Unmet | RungState::Open))
    {
        Some(gate) => (
            gate.lane.as_str().to_string(),
            gate.name.clone(),
            gate.lane.next_command(),
            Some(gate.lane.axis()),
        ),
        None => (
            "complete".into(),
            "complete".into(),
            "loom status".into(),
            None,
        ),
    }
}
