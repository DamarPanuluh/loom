//! Completeness — the Definition of Complete for a behavioral intent.
//!
//! Plane: derived projection, computed on read, never stored. A human hands
//! loom a core idea; the surroundings (failure scenarios, prerequisites,
//! boundary rules, proofs, journeys, open questions) are what humans forget.
//! This module makes "complete" enumerable: a fixed set of axes, each
//! computable from the graph as met / open / waived / not_applicable.
//!
//! An axis is never closed by silence: it is closed by an artifact (a scenario
//! intent, a requires edge, a verdict, a proof) or by a recorded waiver
//! (`waiver:<axis>` facet — see `loom intent waive`), which re-opens when the
//! intent's meaning changes. The `elaborate` queue drains open axes
//! cognitively; questions route to the human through the inbox.

use crate::model::{Edge, EdgeKind, InspectionStatus, Node, NodeType, TargetKind};
use crate::store::Store;
use crate::Result;
use anyhow::bail;
use serde::Serialize;

/// The completeness axes, in narrative order: what can happen around the
/// behavior, what it needs, what governs it, how it is proven, how a consumer
/// reaches it, and what is still unanswered.
pub const AXES: &[&str] = &[
    "scenarios",
    "prerequisites",
    "boundary",
    "proof",
    "journey",
    "questions",
];

/// Resolve the Journey/profile that structurally owns a compiler-created
/// Validation. The Proves edge is the ownership fact; the mutable body type is
/// only checked for consistency and never grants or removes compiler ownership.
pub fn compiler_owned_journey_validation(
    store: &Store,
    validation: &Node,
) -> Result<Option<(Node, String)>> {
    let proves = store.edges_with(Some(EdgeKind::Proves), Some(&validation.id), None)?;
    let mut owners = Vec::new();
    for edge in proves {
        if let Some(target) = store.get_node(&edge.to_id)? {
            if target.node_type == NodeType::Journey {
                owners.push(target);
            }
        }
    }
    if owners.is_empty() {
        return Ok(None);
    }
    if owners.len() != 1 {
        bail!(
            "compiler-owned Validation '{}' must prove exactly one Journey",
            validation.name
        );
    }
    if validation.body.get("type").and_then(|value| value.as_str()) != Some("journey") {
        bail!(
            "compiler-owned Validation '{}' has a mutated proof type; recompile its Journey",
            validation.name
        );
    }
    let profile = validation
        .body
        .get("profile")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "compiler-owned Validation '{}' has no profile",
                validation.name
            )
        })?;
    crate::journey::validate_stable_id("Journey profile", profile)?;
    let owner = owners.pop().expect("one owner was checked");
    let profile_declared = owner
        .body
        .get("profile_ids")
        .and_then(|value| value.as_array())
        .is_some_and(|profiles| profiles.iter().any(|value| value.as_str() == Some(profile)));
    if !profile_declared {
        bail!(
            "compiler-owned Validation '{}' names profile '{}' outside Journey '{}'",
            validation.name,
            profile,
            owner.name
        );
    }
    Ok(Some((owner, profile.to_string())))
}

/// The Journey/profile that owns this edge when it belongs to a compiler-owned
/// proof closure (`Proves`/`Validates`/`Calls`/`Exercises` out of a Validation
/// that `journey compile` created), or None for an ordinary edge.
///
/// ONE predicate serves both halves of the compiler-ownership cut: the write
/// guard below refuses generic mutation, and the queues refuse to SERVE the
/// same edges through a generic lane. Splitting them let Analyze hand out
/// packets whose only write-back the CLI rejects.
pub fn compiler_owned_proof_edge(store: &Store, edge: &Edge) -> Result<Option<(Node, String)>> {
    if !matches!(
        edge.kind,
        EdgeKind::Proves | EdgeKind::Validates | EdgeKind::Calls | EdgeKind::Exercises
    ) {
        return Ok(None);
    }
    let Some(source) = store.get_node(&edge.from_id)? else {
        return Ok(None);
    };
    if source.node_type != NodeType::Validation {
        return Ok(None);
    }
    compiler_owned_journey_validation(store, &source)
}

/// Generic edge commands must not rewrite compiler-owned Journey proof
/// topology. Journey compile/run are the only owners of these closure edges.
pub fn require_generic_edge_mutable(store: &Store, edge: &Edge) -> Result<()> {
    if compiler_owned_proof_edge(store, edge)?.is_some() {
        bail!(
            "compiler-owned Journey proof topology cannot be changed generically; use loom journey compile/run"
        );
    }
    Ok(())
}

/// The scenario aspects a happy path should be surrounded by.
const SCENARIO_ASPECTS: &[&str] = &["sad", "fallback", "edge_case"];

/// Validate an axis label (shared with `loom intent waive`).
pub fn check_axis(axis: &str) -> Result<()> {
    if !AXES.contains(&axis) {
        bail!(
            "unknown completeness axis '{axis}' (use {})",
            AXES.join("|")
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct AxisState {
    pub axis: String,
    /// met | open | waived | not_applicable
    pub state: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waived_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scorecard {
    pub intent_id: String,
    pub intent_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    pub axes: Vec<AxisState>,
    /// Number of axes in state `open`.
    pub open: usize,
}

/// Read-time Journey maturity. None of these booleans is stored: each is a
/// projection over the authored semantic hash, its current projections, the
/// target repository, and the proof Loom actually observed.
#[derive(Debug, Clone, Serialize)]
pub struct JourneyReadiness {
    pub journey_id: String,
    pub journey_name: String,
    pub semantic_hash: String,
    pub step_ids: Vec<String>,
    pub authored: bool,
    pub derived: bool,
    pub implemented: bool,
    pub surfaced: bool,
    pub compiled: bool,
    pub proven: bool,
    pub realized: bool,
    pub derivations_ratified: bool,
    pub derived_intent_ids: Vec<String>,
    pub derive_gaps: Vec<String>,
    pub surface_gaps: Vec<String>,
}

/// One independently closable item in the derive queue. Several gaps may name
/// the same Journey; the full roster enumerates them so queue depth remains
/// exactly the number the lane advertises.
#[derive(Debug, Clone, Serialize)]
pub struct JourneyDeriveGap {
    pub journey_id: String,
    pub kind: String,
    pub subject_id: String,
    pub subject_name: String,
    pub detail: String,
}

/// One Journey eligible for surface work. Surface work is deliberately held
/// until every current derivation is ratified and realizing-grounded.
#[derive(Debug, Clone, Serialize)]
pub struct JourneySurfaceGap {
    pub journey_id: String,
    pub detail: String,
}

#[derive(Debug, serde::Deserialize)]
struct JourneyExemption {
    kind: String,
    reason: String,
    human_decision_digest: String,
}

/// A Journey projection edge is current when it is freshly accepted or has a
/// passing inspection. `independent` means the relationship was inspected and
/// found not to hold, so it cannot satisfy readiness.
fn projection_current(edge: &Edge) -> bool {
    matches!(
        edge.status,
        InspectionStatus::Uninspected | InspectionStatus::Passing
    )
}

/// Canonical JSON key ordering. Takes `&Value` for its one caller; the rule
/// itself lives in `crate::canonical`.
fn canonicalize_json(value: &serde_json::Value) -> serde_json::Value {
    crate::canonical::canonicalize(value.clone())
}

fn parse_canonical_json(raw: &str) -> Option<serde_json::Value> {
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    let canonical = canonicalize_json(&parsed);
    (serde_json::to_string(&canonical).ok()?.as_str() == raw).then_some(canonical)
}

/// Only the dedicated, human-authorized Journey exemption exempts an Intent
/// from ancestry. A malformed, whitespace-padded, or semantically empty JSON
/// value is treated as no exemption at all.
pub fn intent_journey_exempt(store: &Store, intent_id: &str) -> Result<bool> {
    Ok(parse_journey_exemption(store, intent_id)?.is_some())
}

fn parse_journey_exemption(store: &Store, intent_id: &str) -> Result<Option<JourneyExemption>> {
    let Some(raw) = store.get_facet(intent_id, TargetKind::Node, "journey_exemption")? else {
        return Ok(None);
    };
    let Some(value) = parse_canonical_json(&raw) else {
        return Ok(None);
    };
    let Ok(exemption) = serde_json::from_value::<JourneyExemption>(value) else {
        return Ok(None);
    };
    if exemption.kind.trim().is_empty()
        || exemption.reason.trim().is_empty()
        || exemption.human_decision_digest.trim().is_empty()
    {
        return Ok(None);
    }
    Ok(Some(exemption))
}

fn exact_string_array(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    let values = value?.as_array()?;
    let mut out = Vec::with_capacity(values.len());
    let mut seen = std::collections::BTreeSet::new();
    for value in values {
        let item = value.as_str()?.trim();
        if item.is_empty() || !seen.insert(item.to_string()) {
            return None;
        }
        out.push(item.to_string());
    }
    Some(out)
}

fn exact_facet_array(store: &Store, edge_id: &str, key: &str) -> Result<Option<Vec<String>>> {
    let Some(raw) = store.get_facet(edge_id, TargetKind::Edge, key)? else {
        return Ok(None);
    };
    let Some(value) = parse_canonical_json(&raw) else {
        return Ok(None);
    };
    Ok(exact_string_array(Some(&value)))
}

fn hash_bound(store: &Store, edge: &Edge, semantic_hash: &str) -> Result<bool> {
    Ok(store
        .get_facet(&edge.id, TargetKind::Edge, "journey_hash")?
        .as_deref()
        == Some(semantic_hash))
}

/// Parse the canonical binding union shared by acceptance, readiness, and
/// doctor. Machine operations are unique executable witnesses. An intrinsic
/// human decision instead points back to a machine operation bound by an
/// earlier authored step; it completes its own step without becoming another
/// CLI witness.
pub(crate) fn exact_surface_bindings(
    raw: &str,
    step_ids: &[&str],
    operations: &[crate::journey::CliOperation],
) -> Option<Vec<crate::journey::SurfaceBinding>> {
    let value = parse_canonical_json(raw)?;
    let bindings = serde_json::from_value::<Vec<crate::journey::SurfaceBinding>>(value).ok()?;
    if bindings.len() != step_ids.len() {
        return None;
    }

    let mut operation_ids = std::collections::BTreeSet::new();
    for operation in operations {
        if operation.id.trim().is_empty() || !operation_ids.insert(operation.id.as_str()) {
            return None;
        }
    }

    let mut seen_steps = std::collections::BTreeSet::new();
    let mut bound_operations = std::collections::BTreeSet::new();
    let mut prior_operations = std::collections::BTreeSet::new();
    for (binding, expected_step) in bindings.iter().zip(step_ids) {
        let step_id = binding.step_id();
        if step_id != *expected_step || step_id.trim().is_empty() || !seen_steps.insert(step_id) {
            return None;
        }
        match binding {
            crate::journey::SurfaceBinding::Operation(binding) => {
                let operation_id = binding.operation_id.as_str();
                if operation_id.trim().is_empty()
                    || !operation_ids.contains(operation_id)
                    || !bound_operations.insert(operation_id)
                {
                    return None;
                }
                prior_operations.insert(operation_id);
            }
            crate::journey::SurfaceBinding::HumanDecision(binding) => {
                let source = &binding.human_decision;
                if source.validate().is_err()
                    || !operation_ids.contains(source.operation_id.as_str())
                    || !prior_operations.contains(source.operation_id.as_str())
                {
                    return None;
                }
            }
        }
    }
    Some(bindings)
}

fn exact_operation_bindings(
    store: &Store,
    edge_id: &str,
    step_ids: &[String],
    operations: &[crate::journey::CliOperation],
) -> Result<Option<Vec<crate::journey::SurfaceBinding>>> {
    let Some(raw) = store.get_facet(edge_id, TargetKind::Edge, "operation_bindings")? else {
        return Ok(None);
    };
    let expected_order: Vec<&str> = step_ids.iter().map(String::as_str).collect();
    Ok(exact_surface_bindings(&raw, &expected_order, operations))
}

fn current_derivation(
    store: &Store,
    edge: &Edge,
    semantic_hash: &str,
    authored_steps: &[String],
) -> Result<Option<Vec<String>>> {
    if !projection_current(edge) || !hash_bound(store, edge, semantic_hash)? {
        return Ok(None);
    }
    let Some(mapped) = exact_facet_array(store, &edge.id, "step_ids")? else {
        return Ok(None);
    };
    let authored: std::collections::BTreeSet<&str> =
        authored_steps.iter().map(String::as_str).collect();
    if mapped.iter().any(|step| !authored.contains(step.as_str())) {
        return Ok(None);
    }
    // The edge facet's order is canonical Journey order, never caller order.
    let expected: Vec<String> = authored_steps
        .iter()
        .filter(|step| mapped.contains(step))
        .cloned()
        .collect();
    Ok((mapped == expected).then_some(mapped))
}

fn valid_cli_interface(interface: &Node) -> bool {
    interface.node_type == NodeType::InterfaceSurface
        && interface.status != "quarantined"
        && interface.body.get("schema").and_then(|v| v.as_str())
            == Some("loom.interface-surface/v1")
        && interface.body.get("kind").and_then(|v| v.as_str()) == Some("cli")
        && interface
            .body
            .get("operations")
            .and_then(|v| v.as_array())
            .is_some_and(|operations| !operations.is_empty())
}

fn interface_operations(interface: &Node) -> Option<Vec<crate::journey::CliOperation>> {
    serde_json::from_value(interface.body.get("operations")?.clone()).ok()
}

fn interface_has_code(store: &Store, interface: &Node) -> Result<bool> {
    if !valid_cli_interface(interface) {
        return Ok(false);
    }
    for exposes in store.edges_with(Some(EdgeKind::Exposes), Some(&interface.id), None)? {
        if !projection_current(&exposes) {
            continue;
        }
        let Some(codefile) = store.get_node(&exposes.to_id)? else {
            continue;
        };
        if codefile.node_type == NodeType::CodeFile && store.root().join(&codefile.name).is_file() {
            return Ok(true);
        }
    }
    Ok(false)
}

mod readiness;
mod scorecard;

pub use readiness::{
    all_journey_readiness, journey_derive_gaps, journey_derive_gaps_with, journey_readiness,
    journey_surface_gaps, journey_surface_gaps_with,
};
pub(crate) use scorecard::prerequisite_is_realized;
pub use scorecard::{all_scorecards, all_scorecards_with, elaboration_queue, scorecard};
