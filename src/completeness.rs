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

use crate::model::{EdgeKind, Node, NodeType, TargetKind};
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
            if target.status != "implemented" {
                unmet.push(format!("'{}' is {}", target.name, target.status));
            }
        }
    }
    if unmet.is_empty() {
        Ok(axis(
            "prerequisites",
            "met",
            format!("all {} prerequisite(s) implemented", requires.len()),
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

/// The behavior has an observed, passing proof.
fn proof_axis(store: &Store, intent: &Node) -> Result<AxisState> {
    let validates = store.edges_with(Some(EdgeKind::Validates), None, Some(&intent.id))?;
    if validates.is_empty() {
        return Ok(axis("proof", "open", "no validation registered".into()));
    }
    let mut blocked = 0usize;
    for e in &validates {
        if e.status.as_str() == "passing" {
            return Ok(axis("proof", "met", "passing proof on record".into()));
        }
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

/// A user-visible behavior is reachable end to end: a passing journey proof,
/// or at least declared journey coverage.
fn journey_axis(store: &Store, intent: &Node, user_visible: bool) -> Result<AxisState> {
    if !user_visible {
        return Ok(axis(
            "journey",
            "not_applicable",
            "internal intent — journeys prove user-reachable flows".into(),
        ));
    }
    for e in store.edges_with(Some(EdgeKind::Validates), None, Some(&intent.id))? {
        let Some(v) = store.get_node(&e.from_id)? else {
            continue;
        };
        let is_journey = v.body.get("proof_kind").and_then(|x| x.as_str()) == Some("journey");
        let is_l5 = matches!(
            v.body.get("proof_level").and_then(|x| x.as_str()),
            Some("L5") | Some("L6")
        );
        if is_journey && is_l5 && e.status.as_str() == "passing" {
            return Ok(axis("journey", "met", "passing journey proof".into()));
        }
    }
    let covered = !store
        .edges_with(Some(EdgeKind::Covers), None, Some(&intent.id))?
        .is_empty();
    if covered {
        Ok(axis(
            "journey",
            "open",
            "journey coverage declared but no passing journey proof".into(),
        ))
    } else {
        Ok(axis(
            "journey",
            "open",
            "no journey proof or coverage (loom journey add / coverage add)".into(),
        ))
    }
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
