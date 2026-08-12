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

/// Generic edge commands must not rewrite compiler-owned Journey proof
/// topology. Journey compile/run are the only owners of these closure edges.
pub fn require_generic_edge_mutable(store: &Store, edge: &Edge) -> Result<()> {
    if !matches!(
        edge.kind,
        EdgeKind::Proves | EdgeKind::Validates | EdgeKind::Calls | EdgeKind::Exercises
    ) {
        return Ok(());
    }
    let Some(source) = store.get_node(&edge.from_id)? else {
        return Ok(());
    };
    if source.node_type == NodeType::Validation
        && compiler_owned_journey_validation(store, &source)?.is_some()
    {
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

fn canonicalize_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let sorted: std::collections::BTreeMap<String, serde_json::Value> = map
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize_json(value)))
                .collect();
            serde_json::to_value(sorted).expect("a JSON map remains serializable")
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonicalize_json).collect())
        }
        other => other.clone(),
    }
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
    let Some(raw) = store.get_facet(intent_id, TargetKind::Node, "journey_exemption")? else {
        return Ok(false);
    };
    let Some(value) = parse_canonical_json(&raw) else {
        return Ok(false);
    };
    let Ok(exemption) = serde_json::from_value::<JourneyExemption>(value) else {
        return Ok(false);
    };
    Ok(!exemption.kind.trim().is_empty()
        && !exemption.reason.trim().is_empty()
        && !exemption.human_decision_digest.trim().is_empty())
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

/// Compute all seven readiness stages for one authored Journey.
pub fn journey_readiness(store: &Store, journey: &Node) -> Result<JourneyReadiness> {
    let schema_ok = journey.body.get("schema").and_then(|v| v.as_str()) == Some("loom.journey/v1");
    let stable_id = journey
        .body
        .get("stable_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let semantic_hash = journey
        .body
        .get("semantic_hash")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let artifact = journey
        .body
        .get("artifact")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let step_ids = exact_string_array(journey.body.get("step_ids")).unwrap_or_default();
    let authored = journey.node_type == NodeType::Journey
        && schema_ok
        && !stable_id.is_empty()
        && stable_id == journey.name
        && !semantic_hash.is_empty()
        && !artifact.is_empty()
        && !step_ids.is_empty()
        && store.root().join(artifact).is_file();

    let derivations = store.edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)?;
    let mut covered_steps = std::collections::BTreeSet::new();
    let mut derived_intent_ids = Vec::new();
    let mut derive_gaps = Vec::new();
    for edge in &derivations {
        match current_derivation(store, edge, &semantic_hash, &step_ids)? {
            Some(mapped) => {
                covered_steps.extend(mapped);
                if !derived_intent_ids.contains(&edge.to_id) {
                    derived_intent_ids.push(edge.to_id.clone());
                }
            }
            None => derive_gaps.push(format!(
                "derivation {} is stale, hash-mismatched, or has invalid step_ids",
                crate::model::short(&edge.id)
            )),
        }
    }
    for step in &step_ids {
        if !covered_steps.contains(step) {
            derive_gaps.push(format!("step '{step}' is unmapped"));
        }
    }
    derived_intent_ids.sort();
    let derived = authored && derive_gaps.is_empty() && !derived_intent_ids.is_empty();

    let mut derivations_ratified = derived;
    let mut implemented = derived;
    for id in &derived_intent_ids {
        let Some(intent) = store.get_node(id)? else {
            derivations_ratified = false;
            implemented = false;
            continue;
        };
        if store.ratification(id)? != "ratified" {
            derivations_ratified = false;
        }
        if intent.status != "implemented" || store.realizing_groundings(id)?.is_empty() {
            implemented = false;
        }
    }

    let surfaces = store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)?;
    let mut surface_declared = false;
    let mut surfaced = false;
    let mut surface_gaps = Vec::new();
    for edge in &surfaces {
        if !projection_current(edge) || !hash_bound(store, edge, &semantic_hash)? {
            surface_gaps.push(format!(
                "surface {} is stale or bound to an older Journey hash",
                crate::model::short(&edge.id)
            ));
            continue;
        }
        let Some(interface) = store.get_node(&edge.to_id)? else {
            surface_gaps.push("accepted CLI surface target is missing".into());
            continue;
        };
        if !valid_cli_interface(&interface) {
            surface_gaps
                .push("accepted surface is not a valid loom.interface-surface/v1 CLI".into());
            continue;
        }
        let Some(operations) = interface_operations(&interface) else {
            surface_gaps.push("accepted CLI surface has malformed operations".into());
            continue;
        };
        if exact_operation_bindings(store, &edge.id, &step_ids, &operations)?.is_none() {
            surface_gaps.push(format!(
                "surface {} lacks canonical complete operation bindings",
                crate::model::short(&edge.id)
            ));
            continue;
        }
        surface_declared = true;
        if interface_has_code(store, &interface)? {
            surfaced = true;
        } else {
            surface_gaps.push("accepted CLI surface has no exposed live CodeFile".into());
        }
    }
    if !surface_declared {
        surface_gaps.push("no current hash-bound CLI surface".into());
    }

    let current_surface_hash = crate::journey::surface_projection_hash(store, journey)?;
    let mut compiled = false;
    let mut proven = false;
    for edge in store.edges_with(Some(EdgeKind::Proves), None, Some(&journey.id))? {
        if !projection_current(&edge) {
            continue;
        }
        let Some(validation) = store.get_node(&edge.from_id)? else {
            continue;
        };
        let hash_current = surfaced
            && validation.body.get("journey_hash").and_then(|v| v.as_str())
                == Some(semantic_hash.as_str())
            && validation.body.get("surface_hash").and_then(|v| v.as_str())
                == current_surface_hash.as_deref()
            && current_surface_hash.is_some()
            && validation.body.get("profile").and_then(|v| v.as_str()) == Some("proof")
            && validation
                .body
                .get("compiler_version")
                .and_then(|v| v.as_str())
                .is_some_and(|version| !version.is_empty());
        if validation.node_type != NodeType::Validation || !hash_current {
            continue;
        }
        compiled = true;
        if edge.status == InspectionStatus::Passing
            && validation.status == "passed"
            && crate::proofstrength::of(store, &validation.id)?
                >= crate::proofstrength::Strength::END_TO_END
        {
            proven = true;
        }
    }
    let realized = authored
        && derived
        && derivations_ratified
        && implemented
        && surfaced
        && compiled
        && proven;

    Ok(JourneyReadiness {
        journey_id: journey.id.clone(),
        journey_name: journey.name.clone(),
        semantic_hash,
        step_ids,
        authored,
        derived,
        implemented,
        surfaced,
        compiled,
        proven,
        realized,
        derivations_ratified,
        derived_intent_ids,
        derive_gaps,
        surface_gaps,
    })
}

/// Every active authored Journey, stable by authored id.
pub fn all_journey_readiness(store: &Store) -> Result<Vec<JourneyReadiness>> {
    let mut out = Vec::new();
    for journey in store.list_nodes(Some(NodeType::Journey), usize::MAX)? {
        if journey.status == "deprecated" {
            continue;
        }
        out.push(journey_readiness(store, &journey)?);
    }
    out.sort_by(|a, b| a.journey_name.cmp(&b.journey_name));
    Ok(out)
}

/// The single enumerated predicate behind Derive depth, roster, and serving.
pub fn journey_derive_gaps(store: &Store) -> Result<Vec<JourneyDeriveGap>> {
    let readiness = all_journey_readiness(store)?;
    let mut out = Vec::new();
    for journey in &readiness {
        for detail in &journey.derive_gaps {
            let (kind, subject_id, subject_name) = match detail
                .strip_prefix("step '")
                .and_then(|rest| rest.strip_suffix("' is unmapped"))
            {
                Some(step) => ("unmapped_step", step, step),
                None => (
                    "stale_derivation",
                    journey.journey_id.as_str(),
                    journey.journey_name.as_str(),
                ),
            };
            out.push(JourneyDeriveGap {
                journey_id: journey.journey_id.clone(),
                kind: kind.into(),
                subject_id: subject_id.into(),
                subject_name: subject_name.into(),
                detail: detail.clone(),
            });
        }
    }

    // An unrooted Intent is assigned to the first authored Journey only as the
    // packet subject that can accept a manifest. The worker still has to decide
    // whether that Journey honestly derives it; otherwise it authors a better
    // root or records the dedicated human exemption.
    let Some(host) = readiness.iter().find(|journey| journey.authored) else {
        return Ok(out);
    };
    let currently_rooted: std::collections::BTreeSet<&str> = readiness
        .iter()
        .flat_map(|journey| journey.derived_intent_ids.iter().map(String::as_str))
        .collect();
    for intent in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
        if intent.status == "deprecated"
            || currently_rooted.contains(intent.id.as_str())
            || intent_journey_exempt(store, &intent.id)?
        {
            continue;
        }
        out.push(JourneyDeriveGap {
            journey_id: host.journey_id.clone(),
            kind: "unrooted_intent".into(),
            subject_id: intent.id.clone(),
            subject_name: intent.name.clone(),
            detail: format!(
                "intent '{}' has no current Journey derivation and no valid journey_exemption",
                intent.name
            ),
        });
    }
    out.sort_by(|a, b| {
        let rank = |kind: &str| match kind {
            "unmapped_step" => 0,
            "stale_derivation" => 1,
            _ => 2,
        };
        rank(&a.kind)
            .cmp(&rank(&b.kind))
            .then(a.journey_id.cmp(&b.journey_id))
            .then(a.subject_name.cmp(&b.subject_name))
    });
    Ok(out)
}

/// The single enumerated predicate behind Surface depth, roster, and serving.
pub fn journey_surface_gaps(store: &Store) -> Result<Vec<JourneySurfaceGap>> {
    let mut out = Vec::new();
    for journey in all_journey_readiness(store)? {
        if journey.authored
            && journey.derived
            && journey.derivations_ratified
            && journey.implemented
            && !journey.surfaced
        {
            out.push(JourneySurfaceGap {
                journey_id: journey.journey_id,
                detail: journey.surface_gaps.join("; "),
            });
        }
    }
    Ok(out)
}

impl Scorecard {
    pub fn open_axes(&self) -> impl Iterator<Item = &AxisState> {
        self.axes.iter().filter(|a| a.state == "open")
    }
}

/// Compute the scorecard for one intent.
pub fn scorecard(store: &Store, intent: &Node) -> Result<Scorecard> {
    let visibility = store.get_facet(&intent.id, TargetKind::Node, "visibility")?;
    let user_visible = visibility.as_deref() == Some("user_visible");
    let mut axes = Vec::with_capacity(AXES.len());
    axes.push(apply_waiver(
        store,
        intent,
        scenarios_axis(store, intent, user_visible)?,
    )?);
    axes.push(apply_waiver(
        store,
        intent,
        prerequisites_axis(store, intent)?,
    )?);
    axes.push(apply_waiver(store, intent, boundary_axis(store, intent)?)?);
    axes.push(apply_waiver(store, intent, proof_axis(store, intent)?)?);
    axes.push(apply_waiver(
        store,
        intent,
        journey_axis(store, intent, user_visible)?,
    )?);
    // Questions are never waivable: an unanswered question is either answered
    // or withdrawn (inbox disposition), not waived away.
    axes.push(questions_axis(store, intent)?);
    let open = axes.iter().filter(|a| a.state == "open").count();
    Ok(Scorecard {
        intent_id: intent.id.clone(),
        intent_name: intent.name.clone(),
        visibility,
        axes,
        open,
    })
}

/// Scorecards for every active feature-level intent, most-incomplete first.
pub fn all_scorecards(store: &Store) -> Result<Vec<Scorecard>> {
    let mut cards = Vec::new();
    for intent in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
        if intent.status == "deprecated" {
            continue;
        }
        let level = store
            .get_facet(&intent.id, TargetKind::Node, "level")?
            .unwrap_or_default();
        if level != "feature" {
            continue;
        }
        cards.push(scorecard(store, &intent)?);
    }
    cards.sort_by(|a, b| b.open.cmp(&a.open).then(a.intent_name.cmp(&b.intent_name)));
    Ok(cards)
}

/// If the axis is open but waived, the waiver wins (with its reason).
fn apply_waiver(store: &Store, intent: &Node, axis: AxisState) -> Result<AxisState> {
    if axis.state != "open" {
        return Ok(axis);
    }
    let key = format!("waiver:{}", axis.axis);
    match store.get_facet(&intent.id, TargetKind::Node, &key)? {
        Some(reason) if !reason.is_empty() => Ok(AxisState {
            state: "waived".into(),
            waived_reason: Some(reason),
            ..axis
        }),
        _ => Ok(axis),
    }
}

fn axis(axis: &str, state: &str, detail: String) -> AxisState {
    AxisState {
        axis: axis.into(),
        state: state.into(),
        detail,
        waived_reason: None,
    }
}

/// A happy path is surrounded when sad/fallback/edge_case scenarios exist in
/// its family: ScenarioOf edges pointing at it, or hierarchy children, each
/// carrying an `aspect` facet.
fn scenarios_axis(store: &Store, intent: &Node, user_visible: bool) -> Result<AxisState> {
    if !user_visible {
        return Ok(axis(
            "scenarios",
            "not_applicable",
            "internal intent — scenario closure applies to user-visible behavior".into(),
        ));
    }
    // Own aspect: a sad/fallback/edge intent is itself a scenario, not a
    // scenario-needing happy path.
    if let Some(own) = store.get_facet(&intent.id, TargetKind::Node, "aspect")? {
        if own != "happy" {
            return Ok(axis(
                "scenarios",
                "not_applicable",
                format!("this intent IS a {own} scenario"),
            ));
        }
    }
    let mut family = Vec::new();
    for e in store.edges_with(Some(EdgeKind::ScenarioOf), None, Some(&intent.id))? {
        family.push(e.from_id);
    }
    for e in store.edges_with(Some(EdgeKind::Hierarchy), Some(&intent.id), None)? {
        family.push(e.to_id);
    }
    let mut present = std::collections::BTreeSet::new();
    for id in &family {
        if let Some(member) = store.get_node(id)? {
            if member.status == "deprecated" {
                continue;
            }
            if let Some(a) = store.get_facet(&member.id, TargetKind::Node, "aspect")? {
                present.insert(a);
            }
        }
    }
    let missing: Vec<&str> = SCENARIO_ASPECTS
        .iter()
        .filter(|a| !present.contains(**a))
        .copied()
        .collect();
    if missing.is_empty() {
        Ok(axis(
            "scenarios",
            "met",
            "sad, fallback, and edge_case scenarios all present".into(),
        ))
    } else {
        Ok(axis(
            "scenarios",
            "open",
            format!(
                "no {} scenario beside the happy path",
                missing.join(", no ")
            ),
        ))
    }
}

/// Whether an intent can satisfy a `requires` prerequisite.
///
/// Lifecycle alone is not realization: an implemented leaf still belongs to
/// the build queue until it has a current realizing grounding. Hierarchy
/// parents remain exempt from direct grounding because their implementation
/// rolls up through their children, matching the build-lane residue rule.
pub(crate) fn prerequisite_is_realized(store: &Store, intent: &Node) -> Result<bool> {
    if intent.status != "implemented" {
        return Ok(false);
    }
    if !store.realizing_groundings(&intent.id)?.is_empty() {
        return Ok(true);
    }
    Ok(!store
        .edges_with(Some(EdgeKind::Hierarchy), Some(&intent.id), None)?
        .is_empty())
}

/// Every declared prerequisite (requires edge) must be realized.
fn prerequisites_axis(store: &Store, intent: &Node) -> Result<AxisState> {
    let requires = store.edges_with(Some(EdgeKind::Requires), Some(&intent.id), None)?;
    if requires.is_empty() {
        return Ok(axis(
            "prerequisites",
            "met",
            "none declared (elaboration may still propose some)".into(),
        ));
    }
    let mut unmet = Vec::new();
    for e in &requires {
        if let Some(target) = store.get_node(&e.to_id)? {
            if !prerequisite_is_realized(store, &target)? {
                let state = if target.status == "implemented" {
                    "implemented but ungrounded"
                } else {
                    target.status.as_str()
                };
                unmet.push(format!("'{}' is {state}", target.name));
            }
        }
    }
    if unmet.is_empty() {
        Ok(axis(
            "prerequisites",
            "met",
            format!("all {} prerequisite(s) realized", requires.len()),
        ))
    } else {
        Ok(axis("prerequisites", "open", unmet.join("; ")))
    }
}

/// Boundary expectations (auth, validation, errors, …) are the quality rules:
/// measured on this intent or an ancestor (highest honest altitude).
fn boundary_axis(store: &Store, intent: &Node) -> Result<AxisState> {
    let rules_exist = !store.list_nodes(Some(NodeType::QualityRule), 1)?.is_empty();
    if !rules_exist {
        return Ok(axis(
            "boundary",
            "not_applicable",
            "no quality rules seeded (loom rule seed <pack>)".into(),
        ));
    }
    // Walk self + hierarchy ancestors (bounded) collecting governs edges.
    let mut ids = vec![intent.id.clone()];
    let mut cursor = intent.id.clone();
    for _ in 0..5 {
        let parents = store.edges_with(Some(EdgeKind::Hierarchy), None, Some(&cursor))?;
        match parents.into_iter().next() {
            Some(e) => {
                cursor = e.from_id.clone();
                ids.push(e.from_id);
            }
            None => break,
        }
    }
    let mut measured = 0usize;
    let mut failing = 0usize;
    let mut pending = 0usize;
    for id in &ids {
        for e in store.edges_with(Some(EdgeKind::Governs), None, Some(id))? {
            match e.status.as_str() {
                "passing" | "independent" => measured += 1,
                "failing" => failing += 1,
                _ => pending += 1,
            }
        }
    }
    if failing > 0 {
        Ok(axis(
            "boundary",
            "open",
            format!("{failing} failing quality verdict(s) — fix queue"),
        ))
    } else if pending > 0 {
        Ok(axis(
            "boundary",
            "open",
            format!("{pending} unmeasured/stale rule(s) — quality queue"),
        ))
    } else if measured > 0 {
        Ok(axis(
            "boundary",
            "met",
            format!("{measured} rule verdict(s) standing at this altitude"),
        ))
    } else {
        Ok(axis(
            "boundary",
            "open",
            "no rule ever measured here or above — quality queue proposes pairs".into(),
        ))
    }
}

/// The behavior has an observed, passing proof strong enough to establish
/// behavior rather than liveness alone.
fn proof_axis(store: &Store, intent: &Node) -> Result<AxisState> {
    let proof = crate::proofstrength::assess(store, &intent.id)?;
    if !proof.any_registered {
        return Ok(axis("proof", "open", "no validation registered".into()));
    }
    if proof.meaningful_passing {
        let best = proof
            .best_passing_strength
            .unwrap_or(crate::proofstrength::Strength::MEANINGFUL)
            .as_str();
        return Ok(axis(
            "proof",
            "met",
            format!("passing meaningful proof on record ({best})"),
        ));
    }
    if proof.any_passing {
        let best = proof
            .best_passing_strength
            .unwrap_or(crate::proofstrength::Strength::S0)
            .as_str();
        return Ok(axis(
            "proof",
            "open",
            format!(
                "passing proof is {best} and proves liveness only — strengthen it to S2 with an output/content assertion and rerun"
            ),
        ));
    }

    let validates = store.edges_with(Some(EdgeKind::Validates), None, Some(&intent.id))?;
    let mut blocked = 0usize;
    for e in &validates {
        if let Some(v) = store.get_node(&e.from_id)? {
            if v.status == "blocked" {
                blocked += 1;
            }
        }
    }
    if blocked > 0 {
        Ok(axis(
            "proof",
            "open",
            format!("{blocked} proof(s) honestly blocked — unblock or waive"),
        ))
    } else {
        Ok(axis(
            "proof",
            "open",
            "validation(s) registered but none passing".into(),
        ))
    }
}

/// A user-visible behavior is complete on the Journey axis only when a current
/// root Journey derives it and that Journey is fully realized: authored,
/// derived, implemented, surfaced into real target-repository code, compiled,
/// and proven by a passing S3 proof through that surface.
fn journey_axis(store: &Store, intent: &Node, user_visible: bool) -> Result<AxisState> {
    if !user_visible {
        return Ok(axis(
            "journey",
            "not_applicable",
            "internal intent — journeys prove user-reachable flows".into(),
        ));
    }
    let readiness = all_journey_readiness(store)?;
    let mut roots: Vec<&JourneyReadiness> = readiness
        .iter()
        .filter(|journey| journey.derived_intent_ids.contains(&intent.id))
        .collect();
    if roots.iter().any(|journey| journey.realized) {
        return Ok(axis(
            "journey",
            "met",
            "derived from a realized Journey with a passing S3 proof through its surfaced CLI"
                .into(),
        ));
    }
    roots.sort_by(|a, b| a.journey_name.cmp(&b.journey_name));
    let detail = match roots.first() {
        None => "no current Journey derives this behavior — loom next --mode derive".into(),
        Some(journey) if !journey.derived => format!(
            "Journey '{}' still has unmapped or stale derivations",
            journey.journey_name
        ),
        Some(journey) if !journey.derivations_ratified => format!(
            "Journey '{}' has derivations awaiting human ratification",
            journey.journey_name
        ),
        Some(journey) if !journey.implemented => format!(
            "Journey '{}' has derived intents without realizing groundings",
            journey.journey_name
        ),
        Some(journey) if !journey.surfaced => format!(
            "Journey '{}' has no current CLI surface",
            journey.journey_name
        ),
        Some(journey) if !journey.compiled => format!(
            "Journey '{}' surface does not expose a live target-repository CodeFile",
            journey.journey_name
        ),
        Some(journey) if !journey.proven => format!(
            "Journey '{}' is compiled but lacks a passing S3 proof",
            journey.journey_name
        ),
        Some(journey) => format!("Journey '{}' is not yet realized", journey.journey_name),
    };
    Ok(axis("journey", "open", detail))
}

/// Unanswered questions raised for the human: open Question nodes linked to
/// this intent by a live asserted `questions` edge.
fn questions_axis(store: &Store, intent: &Node) -> Result<AxisState> {
    let mut linked = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Questions), None, Some(&intent.id))? {
        if let Some(question) = store.get_node(&edge.from_id)? {
            linked.push(question);
        }
    }
    let open = linked
        .into_iter()
        .filter(|n| n.node_type == NodeType::Question && n.status == "open")
        .count();
    if open == 0 {
        Ok(axis("questions", "met", "no unanswered questions".into()))
    } else {
        Ok(axis(
            "questions",
            "open",
            format!("{open} question(s) awaiting the human"),
        ))
    }
}
