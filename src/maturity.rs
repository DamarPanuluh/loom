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
        // A proof of a RETIRED behavior is not a proof this graph owes. The
        // edge counts already exclude it; counting the node keeps reporting
        // "1 failed" after the behavior was deliberately removed, which sends
        // an operator looking for a repair that does not exist.
        let retired = store
            .edges_with(Some(EdgeKind::Validates), Some(&v.id), None)?
            .into_iter()
            .filter_map(|e| store.get_node(&e.to_id).ok().flatten())
            .any(|n| n.status == "deprecated");
        if retired {
            continue;
        }
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
        let journey_readiness = crate::completeness::all_journey_readiness(store)?;
        let authored_journeys = journey_readiness
            .iter()
            .filter(|journey| journey.authored)
            .count();
        let derive_gaps = crate::completeness::journey_derive_gaps(store)?.len();
        let surface_gaps = crate::completeness::journey_surface_gaps(store)?.len();

        // Edge residue, split exactly the way the lanes serve it.
        let failing_edges =
            store.live_edges_by_status(TruthClass::Asserted, &[InspectionStatus::Failing])?;
        let failing_exemplars = failing_edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Exemplar)
            .count();
        let failing = failing_edges.len() - failing_exemplars;
        let open_research = store
            .list_nodes(Some(NodeType::TaskRecord), usize::MAX)?
            .iter()
            .filter(|node| crate::research::is_open_research(node))
            .count();
        let stale = store.live_edges_by_status(
            TruthClass::Asserted,
            &[InspectionStatus::NeedsReverification],
        )?;
        let uninspected =
            store.live_edges_by_status(TruthClass::Asserted, &[InspectionStatus::Uninspected])?;
        let split = |edges: &[crate::model::Edge]| -> Result<(usize, usize, usize)> {
            let governs = edges.iter().filter(|e| e.kind == EdgeKind::Governs).count();
            let validates = edges
                .iter()
                .filter(|e| e.kind == EdgeKind::Validates)
                .count();
            // `depends_on` is a federation ripple link and `exercises` is
            // validation-specific evidence provenance; neither is a claim the
            // analyze lane verifies. Compiler-owned Journey proof topology is
            // not one either: `journey compile/run` owns it and the validate
            // lane serves it. Counting any of them as relationships inflated
            // the rung above the queue depth — the exact drift the shared
            // predicate (`crate::workitem::analyze_serves`) exists to prevent.
            let mut relationships = 0usize;
            for edge in edges {
                if crate::workitem::analyze_serves(store, edge)? {
                    relationships += 1;
                }
            }
            Ok((relationships, governs, validates))
        };
        let (stale_relationships, stale_governs, stale_validates) = split(&stale)?;
        let (uninspected_relationships, uninspected_governs, uninspected_validates) =
            split(&uninspected)?;
        let validation_work_units = crate::workitem::validation_work_units(store)?.len();

        // Proofs: registered validations are not proof until they pass.
        let validations = validation_summary(store)?;
        // Counted by the SAME function the queue serves from. This was an
        // inline second copy of the predicate, so when the queue learned that a
        // passing proof must also reach S2, the rung went on counting bare
        // passes — the rung reading 15 while the queue held 59. Two definitions
        // of one quantity, agreeing only until one of them changed.
        let unproven_implemented = crate::workitem::unproven_implemented_intents(store)?.len();

        let smells = crate::signal::smells(store)?;
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

        // One actionable list feeds both this status projection and the audit
        // queue. Split it only for the human-readable rung detail.
        let audit_backlog = crate::audit::backlog(store)?;
        let doctor_issues = audit_backlog
            .iter()
            .filter(|finding| finding.kind == "doctor_issue")
            .count();
        let backlog_smells = audit_backlog
            .iter()
            .filter(|finding| finding.kind == "smell")
            .count();
        let audit_findings = audit_backlog.len() - doctor_issues - backlog_smells;

        Ok(LadderInputs {
            observed: identity.observed,
            authored_journeys,
            active: active.len(),
            implemented: implemented.len(),
            codefiles: store
                .list_nodes(Some(NodeType::CodeFile), usize::MAX)?
                .len(),
            planned,
            // Single source of truth with the build lane: the same predicate the
            // build queue serves, so a non-zero rung always has a work item.
            ungrounded: crate::workitem::ungrounded_implemented_intents(store)?.len(),
            unowned_codefiles: crate::coverage::unowned_codefiles(store)?.len(),
            failing,
            derive_gaps,
            surface_gaps,
            failing_exemplars,
            open_research,
            stale_relationships,
            uninspected_relationships,
            stale_governs,
            uninspected_governs,
            stale_validates,
            uninspected_validates,
            validation_work_units,
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
            proposed_hypotheses: store
                .nodes_by_status(NodeType::Hypothesis, &["proposed"])?
                .len(),
            open_elaborations: crate::completeness::all_scorecards(store)?
                .iter()
                .filter(|c| c.open > 0 && c.visibility.as_deref() == Some("user_visible"))
                .count(),
            rectifiable_divergences: crate::divergence::rectifiable_count(store)?,
            divergences: crate::divergence::human_blocking_count(store)?,
            audit_findings,
            risk_candidates: crate::risk::rank(store)?.len(),
            doctor_issues,
            open_smells: backlog_smells,
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
    ladder_from_inputs(store, &inputs)
}

/// The ladder AND queue depths from ONE gather. Callers that render both (the
/// `loom status` and `loom_status` surfaces) use this so the expensive gather —
/// callgraph build, export render, detector suite, audit pass — runs once, not
/// twice.
pub fn ladder_and_depths(store: &Store) -> Result<(Ladder, QueueDepths)> {
    let inputs = LadderInputs::gather(store)?;
    let depths = QueueDepths::from_inputs(&inputs);
    let ladder = ladder_from_inputs(store, &inputs)?;
    Ok((ladder, depths))
}

/// Turn one gathered snapshot into a Ladder. Kept separate from the gather so a
/// caller holding `LadderInputs` need not gather a second time.
fn ladder_from_inputs(store: &Store, inputs: &LadderInputs) -> Result<Ladder> {
    let rungs = build_rungs(inputs);
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
            } else if lane == Lane::Deepen {
                // Never met, never unmet. Risk work re-ranks as the graph
                // changes rather than draining, so "done" is not one of its
                // states — and because `Open` never blocks, it cannot hold
                // anything up either.
                RungState::Open
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

    // Integrity is a precondition, not a late quality signal. A malformed
    // graph cannot honestly advance implementation or proof work merely
    // because `sound` sits later in the display order. Keep the audit rung
    // actionable and block every other rung until doctor is clean.
    if c.doctor_issues > 0 {
        for rung in &mut rungs {
            if rung.lane == Lane::Audit {
                rung.blocked = false;
                rung.blocked_by = None;
            } else {
                rung.blocked = true;
                rung.blocked_by = Some("graph integrity".into());
            }
        }
        return rungs;
    }

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
/// `deepen` is the permanent fallthrough: it is `Open` whenever a codefile
/// exists, so a graph that has met every floor is pointed at the weakest thing
/// it still stands on rather than told to re-read itself. There is no
/// `complete`.
pub fn compass(rungs: &[Rung]) -> (String, String, String, Option<crate::truth::TruthAxis>) {
    match rungs
        .iter()
        .find(|r| !r.blocked && matches!(r.state, RungState::Unmet | RungState::Open))
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
