use super::{
    all_journey_readiness, parse_journey_exemption, AxisState, JourneyReadiness, Scorecard, AXES,
    SCENARIO_ASPECTS,
};
use crate::model::{Edge, EdgeKind, Node, NodeType, TargetKind};
use crate::store::Store;
use crate::Result;
use anyhow::bail;
use std::collections::{BTreeMap, BTreeSet};

impl Scorecard {
    pub fn open_axes(&self) -> impl Iterator<Item = &AxisState> {
        self.axes.iter().filter(|a| a.state == "open")
    }
}

/// Compute the scorecard for one intent.
pub fn scorecard(store: &Store, intent: &Node) -> Result<Scorecard> {
    let boundaries = BoundaryIndex::load(store)?;
    let readiness = all_journey_readiness(store)?;
    scorecard_with_boundaries(store, intent, &boundaries, &readiness)
}

fn scorecard_with_boundaries(
    store: &Store,
    intent: &Node,
    boundaries: &BoundaryIndex,
    readiness: &[JourneyReadiness],
) -> Result<Scorecard> {
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
    axes.push(apply_waiver(
        store,
        intent,
        boundary_axis(boundaries, intent)?,
    )?);
    axes.push(apply_waiver(store, intent, proof_axis(store, intent)?)?);
    axes.push(apply_waiver(
        store,
        intent,
        journey_axis(store, intent, user_visible, readiness)?,
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
    let readiness = all_journey_readiness(store)?;
    all_scorecards_with(store, &readiness)
}

/// The same cards over an already-computed readiness snapshot. The journey
/// axis reads readiness per intent; recomputing that whole-graph walk inside
/// every card was finding 6825299d — ~43 user-visible intents × ~5.7s of
/// readiness turned one status call into CPU-minutes.
pub fn all_scorecards_with(
    store: &Store,
    readiness: &[JourneyReadiness],
) -> Result<Vec<Scorecard>> {
    let boundaries = BoundaryIndex::load(store)?;
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
        cards.push(scorecard_with_boundaries(
            store,
            &intent,
            &boundaries,
            readiness,
        )?);
    }
    cards.sort_by(|a, b| b.open.cmp(&a.open).then(a.intent_name.cmp(&b.intent_name)));
    Ok(cards)
}

/// User-visible incomplete feature intents that are still implementation-active.
/// A `blocked` intent stays in the graph (wanted, parked on a recorded external
/// hold) but is not elaborated until it returns to `planned` / `needs_change`.
pub fn elaboration_queue(
    store: &Store,
    readiness: &[JourneyReadiness],
) -> Result<Vec<(Node, Scorecard)>> {
    let mut out = Vec::new();
    for card in all_scorecards_with(store, readiness)? {
        if card.open == 0 || card.visibility.as_deref() != Some("user_visible") {
            continue;
        }
        let Some(intent) = store.get_node(&card.intent_id)? else {
            continue;
        };
        if intent.status == "blocked" {
            continue;
        }
        out.push((intent, card));
    }
    Ok(out)
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
    let present_kinds: Vec<&str> = SCENARIO_ASPECTS
        .iter()
        .copied()
        .filter(|aspect| present.contains(*aspect))
        .collect();
    if present_kinds.is_empty() {
        Ok(axis(
            "scenarios",
            "open",
            "no sad, fallback, or edge_case scenario beside the happy path".into(),
        ))
    } else {
        Ok(axis(
            "scenarios",
            "met",
            format!(
                "{} scenario(s) beside the happy path",
                present_kinds.join(", ")
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

/// In-memory quality topology shared by every scorecard in one projection.
/// Loading the three edge families once avoids turning a status read into
/// hundreds of SQLite round trips as scenario and hierarchy depth grows.
struct BoundaryIndex {
    rules_exist: bool,
    hierarchy_parents: BTreeMap<String, Vec<String>>,
    hierarchy_children: BTreeMap<String, Vec<String>>,
    scenario_parents: BTreeMap<String, Vec<String>>,
    governs: BTreeMap<String, Vec<Edge>>,
}

impl BoundaryIndex {
    fn load(store: &Store) -> Result<Self> {
        let active: BTreeSet<String> = store
            .list_nodes(Some(NodeType::Intent), usize::MAX)?
            .into_iter()
            .filter(|node| node.status != "deprecated")
            .map(|node| node.id)
            .collect();
        let mut hierarchy_parents: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut hierarchy_children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for edge in store.edges_with(Some(EdgeKind::Hierarchy), None, None)? {
            if active.contains(&edge.from_id) && active.contains(&edge.to_id) {
                hierarchy_parents
                    .entry(edge.to_id.clone())
                    .or_default()
                    .push(edge.from_id.clone());
                hierarchy_children
                    .entry(edge.from_id)
                    .or_default()
                    .push(edge.to_id);
            }
        }
        let mut scenario_parents: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for edge in store.edges_with(Some(EdgeKind::ScenarioOf), None, None)? {
            if active.contains(&edge.from_id) && active.contains(&edge.to_id) {
                scenario_parents
                    .entry(edge.from_id)
                    .or_default()
                    .push(edge.to_id);
            }
        }
        let mut governs: BTreeMap<String, Vec<Edge>> = BTreeMap::new();
        for edge in store.edges_with(Some(EdgeKind::Governs), None, None)? {
            if active.contains(&edge.to_id) {
                governs.entry(edge.to_id.clone()).or_default().push(edge);
            }
        }
        for values in hierarchy_parents
            .values_mut()
            .chain(hierarchy_children.values_mut())
            .chain(scenario_parents.values_mut())
        {
            values.sort();
            values.dedup();
        }
        for edges in governs.values_mut() {
            edges.sort_by(|left, right| left.id.cmp(&right.id));
            edges.dedup_by(|left, right| left.id == right.id);
        }
        Ok(Self {
            rules_exist: !store.list_nodes(Some(NodeType::QualityRule), 1)?.is_empty(),
            hierarchy_parents,
            hierarchy_children,
            scenario_parents,
            governs,
        })
    }

    fn hierarchy_ancestor_ids(&self, intent_id: &str) -> BTreeSet<String> {
        let mut ids = BTreeSet::new();
        let mut pending = vec![intent_id.to_string()];
        while let Some(current) = pending.pop() {
            if !ids.insert(current.clone()) {
                continue;
            }
            if let Some(parents) = self.hierarchy_parents.get(&current) {
                pending.extend(parents.iter().cloned());
            }
        }
        ids
    }

    fn hierarchy_leaf_scopes(
        &self,
        intent_id: &str,
        inherited: &BTreeSet<String>,
    ) -> Result<Vec<BTreeSet<String>>> {
        let mut reachable = BTreeSet::new();
        let mut leaves = BTreeSet::new();
        let mut pending = vec![intent_id.to_string()];
        while let Some(current) = pending.pop() {
            if !reachable.insert(current.clone()) {
                continue;
            }
            match self.hierarchy_children.get(&current) {
                Some(children) if !children.is_empty() => {
                    pending.extend(children.iter().cloned());
                }
                _ => {
                    leaves.insert(current);
                }
            }
        }
        if leaves.is_empty() {
            bail!("hierarchy cycle while computing boundary coverage at '{intent_id}'");
        }

        Ok(leaves
            .into_iter()
            .map(|leaf_id| {
                let mut scope = inherited.clone();
                scope.extend(self.hierarchy_ancestor_ids(&leaf_id));
                scope
            })
            .collect())
    }

    fn surface_scopes(&self, intent_id: &str) -> Result<Vec<BTreeSet<String>>> {
        fn visit(
            index: &BoundaryIndex,
            intent_id: &str,
            scenario_path: &mut BTreeSet<String>,
        ) -> Result<Vec<BTreeSet<String>>> {
            if !scenario_path.insert(intent_id.to_string()) {
                bail!("scenario cycle while computing boundary coverage at '{intent_id}'");
            }
            let own_scope = index.hierarchy_ancestor_ids(intent_id);
            let mut scopes = match index.scenario_parents.get(intent_id) {
                Some(parents) if !parents.is_empty() => {
                    let mut inherited = Vec::new();
                    for parent_id in parents {
                        for mut scope in visit(index, parent_id, scenario_path)? {
                            scope.extend(own_scope.iter().cloned());
                            inherited.push(scope);
                        }
                    }
                    inherited
                }
                _ => index.hierarchy_leaf_scopes(intent_id, &own_scope)?,
            };
            scenario_path.remove(intent_id);
            scopes.sort();
            scopes.dedup();
            Ok(scopes)
        }

        visit(self, intent_id, &mut BTreeSet::new())
    }
}

/// Boundary expectations (auth, validation, errors, …) are quality rules.
/// A behavior uses verdicts recorded directly on it, inherited from hierarchy
/// ancestors, or inherited from the happy-path quality surfaces it surrounds.
/// Roll-up intents aggregate their hierarchy leaves because those leaves are
/// the code-bearing surfaces served by the quality queue.
fn boundary_axis(index: &BoundaryIndex, intent: &Node) -> Result<AxisState> {
    if !index.rules_exist {
        return Ok(axis(
            "boundary",
            "not_applicable",
            "no quality rules seeded (loom rule seed <pack>)".into(),
        ));
    }

    let scopes = index.surface_scopes(&intent.id)?;
    let mut seen_edges = BTreeSet::new();
    let mut measured = 0usize;
    let mut failing = 0usize;
    let mut pending = 0usize;
    let mut uncovered = 0usize;
    for ids in &scopes {
        let mut covered = false;
        for id in ids {
            for edge in index.governs.get(id).into_iter().flatten() {
                covered = true;
                if !seen_edges.insert(edge.id.clone()) {
                    continue;
                }
                match edge.status.as_str() {
                    "passing" | "independent" => measured += 1,
                    "failing" => failing += 1,
                    _ => pending += 1,
                }
            }
        }
        if !covered {
            uncovered += 1;
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
    } else if uncovered > 0 {
        Ok(axis(
            "boundary",
            "open",
            format!(
                "{uncovered} code-bearing quality surface(s) have no measured rule — quality queue proposes pairs"
            ),
        ))
    } else if measured > 0 {
        Ok(axis(
            "boundary",
            "met",
            format!("{measured} rule verdict(s) standing across every quality surface"),
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
fn journey_axis(
    store: &Store,
    intent: &Node,
    user_visible: bool,
    readiness: &[JourneyReadiness],
) -> Result<AxisState> {
    if !user_visible {
        return Ok(axis(
            "journey",
            "not_applicable",
            "internal intent — journeys prove user-reachable flows".into(),
        ));
    }
    if let Some(exemption) = parse_journey_exemption(store, &intent.id)? {
        return Ok(axis(
            "journey",
            "not_applicable",
            format!(
                "journey_exemption kind '{}' — {}",
                exemption.kind, exemption.reason
            ),
        ));
    }
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
