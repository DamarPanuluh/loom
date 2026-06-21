//! Priority scoring and discovery-candidate selection for `loom next`.

use anyhow::Result;
use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::types::{
    DiscoveryCentrality, DiscoverySignal, Governs, Hypothesis, InspectionStatus, Intent, Note,
    QualityRule, RelatesTo, RelationKind, TargetsEdge, ValidatesEdge, Validation,
};

use super::snapshot::{DiscoverySnapshot, QuerySnapshot};

/// Extract the staling file from a sync-flip transition note ("... -> <status>
/// (sync: <path> changed)") so fix queues can group stale claims by hot file.
pub fn parse_sync_cause(text: &str) -> Option<&str> {
    text.rsplit_once("(sync: ")?
        .1
        .strip_suffix(')')?
        .strip_suffix(" changed")
}

/// Weight of the bridge-centrality term in `loom next` edge scoring. Degree
/// measures local blast radius; betweenness measures whether an intent is a
/// CHOKEPOINT on the paths between regions of the graph — a structural risk a
/// degree count is blind to. The bump per endpoint is `BRIDGE_WEIGHT × (that
/// intent's betweenness / the graph's max betweenness)`, so the single most
/// bridge-like intent contributes the full weight and everyone else scales
/// down. At 3.0 it is on the order of an urgency step (uninspected=2, failing=4)
/// and a few degrees — enough that a low-degree chokepoint edge can overtake a
/// higher-degree clique edge, never so much that betweenness alone dominates.
/// A graph with no bridges (max betweenness 0 — e.g. a clique or a tree of
/// stars) gets no bump and scores exactly as before.
pub const BRIDGE_WEIGHT: f64 = 3.0;

/// Decaying priority bump for the graded sync ripple. `loom sync` FLIPS only
/// the direct one-hop RELATES_TO neighbors of a changed file's intents to
/// `needs_reverification` (the blast radius stays surgical — one hop). But a
/// change two or three hops out is *suggestive*, not yet stale: those edges
/// keep their status and instead receive a decaying priority nudge so the
/// analyzer drifts toward the changed region without the graph lying about what
/// has actually been invalidated. HOP2 (two hops from the change) > HOP3.
pub const RIPPLE_BUMP_HOP2: f64 = 2.0;
pub const RIPPLE_BUMP_HOP3: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryClassFilter {
    SuspectedCoupling,
    ImpactMap,
    All,
}

impl DiscoveryClassFilter {
    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value.unwrap_or("suspected-coupling") {
            "suspected-coupling" | "suspected_coupling" => Ok(Self::SuspectedCoupling),
            "impact-map" | "impact_map" => Ok(Self::ImpactMap),
            "all" => Ok(Self::All),
            other => anyhow::bail!(
                "invalid discovery class '{other}'. Valid values: suspected-coupling, impact-map, all"
            ),
        }
    }

    fn accepts(self, class: &str) -> bool {
        match self {
            Self::SuspectedCoupling => class == "suspected_coupling",
            Self::ImpactMap => class == "impact_map",
            Self::All => true,
        }
    }

    pub fn as_cli_value(self) -> &'static str {
        match self {
            Self::SuspectedCoupling => "suspected-coupling",
            Self::ImpactMap => "impact-map",
            Self::All => "all",
        }
    }
}

/// Per-intent graded-ripple priority bump derived from the CURRENT stale
/// frontier — the set of intents that are an endpoint of a `needs_reverification`
/// RELATES_TO edge (exactly what the one-hop flip produces). Distance is
/// measured from that frontier over the undirected real-RELATES_TO graph:
///
/// - frontier itself (distance 0): the changed intents AND their one-hop
///   neighbors — already flipped/urgent, so **no** bump.
/// - distance 1 (two hops from the change): `RIPPLE_BUMP_HOP2`.
/// - distance 2 (three hops from the change): `RIPPLE_BUMP_HOP3`.
/// - farther / unreached: nothing.
///
/// Deriving from `needs_reverification` rather than from "what this sync
/// touched" keeps the bump COHERENT with scoring and self-correcting: as the
/// one-hop edges get re-inspected and leave `needs_reverification`, the frontier
/// shrinks and the downstream bumps fade on their own. Empty map when nothing is
/// stale (no frontier → no ripple).
pub fn ripple_bump_by_intent(snapshot: &QuerySnapshot) -> HashMap<String, f64> {
    let ids: Vec<&str> = snapshot.intents.iter().map(|i| i.id.as_str()).collect();
    let n = ids.len();
    if n == 0 {
        return HashMap::new();
    }
    let index: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let mut neighbors: Vec<HashSet<usize>> = vec![HashSet::new(); n];
    let mut frontier: HashSet<usize> = HashSet::new();
    for edge in &snapshot.relates {
        if edge.inspection_status == "independent" {
            continue;
        }
        let (Some(&a), Some(&b)) = (
            index.get(edge.from_id.as_str()),
            index.get(edge.to_id.as_str()),
        ) else {
            continue;
        };
        if a == b {
            continue;
        }
        neighbors[a].insert(b);
        neighbors[b].insert(a);
        if edge.inspection_status == "needs_reverification" {
            frontier.insert(a);
            frontier.insert(b);
        }
    }
    if frontier.is_empty() {
        return HashMap::new();
    }
    let adjacency: Vec<Vec<usize>> = neighbors
        .into_iter()
        .map(|s| s.into_iter().collect())
        .collect();
    let sources: Vec<usize> = frontier.into_iter().collect();
    let dist = super::graph_algo::hop_distances(n, &adjacency, &sources);
    let mut out = HashMap::new();
    for (i, &d) in dist.iter().enumerate() {
        let bump = match d {
            1 => RIPPLE_BUMP_HOP2,
            2 => RIPPLE_BUMP_HOP3,
            _ => continue, // 0 (frontier, already stale) or >2 / unreached
        };
        out.insert(ids[i].to_string(), bump);
    }
    out
}

/// A build work item: the intent, its priority, and whether it is a non-leaf
/// whose children are all implemented (a roll-up, not a code-writing task).
pub fn scored_candidates_from_snapshot(
    snapshot: &QuerySnapshot,
    mode: &str,
) -> Vec<(RelatesTo, f64)> {
    let mut candidates: Vec<RelatesTo> = snapshot
        .relates
        .iter()
        .filter(|edge| match mode {
            "fix" => matches!(
                edge.inspection_status.as_str(),
                "failing" | "needs_reverification"
            ),
            _ => edge.inspection_status == "uninspected",
        })
        .cloned()
        .collect();

    let mut seen = std::collections::HashSet::new();
    candidates.retain(|e| seen.insert(e.id.clone()));

    if candidates.is_empty() {
        return Vec::new();
    }

    let active: std::collections::HashSet<&str> =
        snapshot.intents.iter().map(|i| i.id.as_str()).collect();
    candidates.retain(|e| active.contains(e.from_id.as_str()) && active.contains(e.to_id.as_str()));
    if candidates.is_empty() {
        return Vec::new();
    }

    // Bridge centrality, normalized so the most bridge-like intent contributes
    // the full BRIDGE_WEIGHT and others scale down. Computed once over the
    // shared snapshot (cached there); `max_bc == 0` (no bridges) leaves scoring
    // byte-identical to the pure-degree formula.
    let betweenness = snapshot.betweenness();
    let max_bc = betweenness.values().cloned().fold(0.0f64, f64::max);
    let bridge_bonus = |id: &str| -> f64 {
        if max_bc <= 0.0 {
            0.0
        } else {
            BRIDGE_WEIGHT * betweenness.get(id).copied().unwrap_or(0.0) / max_bc
        }
    };

    // Graded sync-ripple bump: edges two/three hops from a stale region rank
    // higher without being flipped. Empty (no cost) when nothing is stale.
    let ripple = ripple_bump_by_intent(snapshot);

    let now = chrono::Utc::now();
    let mut scored: Vec<(RelatesTo, f64)> = Vec::new();
    for edge in candidates {
        let deg_a = *snapshot.degrees.get(&edge.from_id).unwrap_or(&0);
        let deg_b = *snapshot.degrees.get(&edge.to_id).unwrap_or(&0);
        let status: InspectionStatus = edge
            .inspection_status
            .parse()
            .unwrap_or(InspectionStatus::Uninspected);
        let urgency = status.urgency();
        let age_penalty = if edge.last_inspected.is_empty() {
            0.0
        } else if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&edge.last_inspected) {
            let parsed_utc = parsed.with_timezone(&chrono::Utc);
            let days = now.signed_duration_since(parsed_utc).num_days() as f64;
            days * 0.05
        } else {
            0.0
        };
        let bridge = bridge_bonus(&edge.from_id) + bridge_bonus(&edge.to_id);
        let ripple_bump = ripple.get(&edge.from_id).copied().unwrap_or(0.0)
            + ripple.get(&edge.to_id).copied().unwrap_or(0.0);
        let score = deg_a as f64 + deg_b as f64 + urgency - age_penalty + bridge + ripple_bump;
        scored.push((edge, score));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
}
#[derive(Debug, Clone)]
pub struct BuildCandidate {
    pub intent: Intent,
    pub score: f64,
    /// True when this is a planned PARENT whose children are all implemented —
    /// the action is "verify children and mark implemented", not "write code".
    pub rollup: bool,
}

/// Normative-plane coverage: how much of the rule × intent-with-code grid has
/// actually been measured. HIERARCHY-AWARE like the `unmeasured_intents` smell —
pub fn build_candidates_from_snapshot(snapshot: &QuerySnapshot) -> Vec<BuildCandidate> {
    let lifecycle_of: HashMap<&str, &str> = snapshot
        .intents
        .iter()
        .map(|i| (i.id.as_str(), i.lifecycle.as_str()))
        .collect();
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for (p, c) in &snapshot.hierarchy {
        children.entry(p.clone()).or_default().push(c.clone());
    }
    let mut pending: Vec<(&Intent, f64, bool)> = Vec::new();
    for intent in &snapshot.intents {
        // `deferred` (and `implemented`) fall through here: a parked intent is
        // never served by the build queue.
        let urgency = match intent.lifecycle.as_str() {
            "needs_change" => 4.0,
            "planned" => 2.0,
            // Cleanup is tracked work: a to_be_removed intent stays in the build
            // queue (delete the code) WHILE it is still grounded, and drops out
            // once its code is gone (done by absence).
            "to_be_removed" if snapshot.with_code.contains(&intent.id) => 3.0,
            _ => continue,
        };
        let kids = children.get(&intent.id);
        let mut rollup = false;
        if intent.lifecycle == "planned" {
            if let Some(kids) = kids {
                // Only planned/needs_change children are PENDING work. A
                // `deferred` child is consciously parked, so it does NOT hold
                // the parent open — the parent rolls up once its active children
                // are all implemented.
                let pending_child = kids.iter().any(|c| {
                    matches!(
                        lifecycle_of.get(c.as_str()),
                        Some(&"planned") | Some(&"needs_change")
                    )
                });
                if pending_child {
                    continue;
                }
                rollup = true;
            }
        }
        pending.push((intent, urgency, rollup));
    }
    let mut scored: Vec<BuildCandidate> = pending
        .into_iter()
        .map(|(intent, urgency, rollup)| BuildCandidate {
            intent: intent.clone(),
            score: *snapshot.degrees.get(&intent.id).unwrap_or(&0) as f64 + urgency,
            rollup,
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored
}
/// a verdict on a component covers its descendants (measuring at the highest
/// honest altitude is the encouraged strategy, never punished).
pub struct NormativeCoverage {
    /// Non-deprecated intents that have real code (≥1 IMPLEMENTS).
    pub intents_with_code: i64,
    /// rules × intents_with_code — the full measuring grid.
    pub total_pairs: i64,
    /// Pairs considered: a GOVERNS edge of ANY state, directly or on an ancestor.
    pub measured_pairs: i64,
    /// Unmeasured pairs at the HIGHEST altitude only — the actual work queue.
    /// (An unmeasured intent whose ancestor is also unmeasured is omitted: one
    /// verdict up there covers it. Bounded by #rules × #top-level branches.)
    pub queue: Vec<(QualityRule, Intent)>,
}

/// Passing/failing verdicts recorded with confidence below this surface in the
/// review queue — the strategic double-check loop for tiered agents: a
/// low-capability scout records HONEST confidence, and the graph itself routes
/// the uncertain claims to a stronger reviewer. Re-recording with confidence
/// at/above the threshold resolves the item; overturning to `independent`
/// resolves it immediately because independent claims carry their rationale in
/// notes/evidence, not in confidence.
pub const REVIEW_CONFIDENCE: f64 = 0.7;

/// One uncertain passing/failing claim for the reviewer: a recorded verdict
/// (RELATES_TO or GOVERNS) whose confidence is below `REVIEW_CONFIDENCE`,
/// scored by (1 − confidence) × combined centrality — an uncertain claim about
/// a hub outranks an uncertain claim about a leaf pair. Cheap certainty on
/// leaves is deliberately left alone: double-check strategically, not
/// exhaustively.
#[derive(Debug, Clone)]
pub enum ReviewCandidate {
    RelatesTo(RelatesTo),
    Governs(Governs),
}

pub fn review_candidates_from_snapshot(snapshot: &QuerySnapshot) -> Vec<(ReviewCandidate, f64)> {
    let active: std::collections::HashSet<&str> =
        snapshot.intents.iter().map(|i| i.id.as_str()).collect();
    let needs_review = |status: &str, confidence: f64| {
        matches!(status, "passing" | "failing")
            && confidence > 0.0
            && confidence < REVIEW_CONFIDENCE
    };
    let mut scored: Vec<(ReviewCandidate, f64)> = Vec::new();
    for edge in &snapshot.relates {
        if !needs_review(&edge.inspection_status, edge.confidence)
            || !active.contains(edge.from_id.as_str())
            || !active.contains(edge.to_id.as_str())
        {
            continue;
        }
        let deg = (*snapshot.degrees.get(&edge.from_id).unwrap_or(&0)
            + *snapshot.degrees.get(&edge.to_id).unwrap_or(&0)) as f64;
        let score = (1.0 - edge.confidence) * (deg + 1.0);
        scored.push((ReviewCandidate::RelatesTo(edge.clone()), score));
    }
    for edge in &snapshot.governs {
        if !needs_review(&edge.inspection_status, edge.confidence)
            || !active.contains(edge.intent_id.as_str())
        {
            continue;
        }
        let deg = *snapshot.degrees.get(&edge.intent_id).unwrap_or(&0) as f64;
        let score = (1.0 - edge.confidence) * (deg + 1.0);
        scored.push((ReviewCandidate::Governs(edge.clone()), score));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

pub fn normative_coverage_from_snapshot(snapshot: &QuerySnapshot) -> NormativeCoverage {
    let candidates: Vec<&Intent> = snapshot
        .intents
        .iter()
        .filter(|i| i.status != "deprecated" && snapshot.with_code.contains(&i.id))
        .collect();

    let considered: std::collections::HashSet<(&str, &str)> = snapshot
        .governs
        .iter()
        .map(|g| (g.rule_id.as_str(), g.intent_id.as_str()))
        .collect();
    let parent_of: HashMap<&str, &str> = snapshot
        .hierarchy
        .iter()
        .map(|(p, c)| (c.as_str(), p.as_str()))
        .collect();
    let considered_up = |rule_id: &str, intent_id: &str| -> bool {
        let mut cur = Some(intent_id);
        let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
        while let Some(id) = cur {
            if !visited.insert(id) {
                return false;
            }
            if considered.contains(&(rule_id, id)) {
                return true;
            }
            cur = parent_of.get(id).copied();
        }
        false
    };

    let total_pairs = snapshot.rules.len() as i64 * candidates.len() as i64;
    let mut measured_pairs = 0i64;
    let mut queue: Vec<(QualityRule, Intent)> = Vec::new();
    for rule in &snapshot.rules {
        let unmeasured: std::collections::HashSet<&str> = candidates
            .iter()
            .filter(|i| !considered_up(&rule.id, &i.id))
            .map(|i| i.id.as_str())
            .collect();
        measured_pairs += candidates.len() as i64 - unmeasured.len() as i64;
        for intent in &candidates {
            if !unmeasured.contains(intent.id.as_str()) {
                continue;
            }
            let mut cur = parent_of.get(intent.id.as_str()).copied();
            let mut shadowed = false;
            let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
            while let Some(id) = cur {
                if !visited.insert(id) {
                    break;
                }
                if unmeasured.contains(id) {
                    shadowed = true;
                    break;
                }
                cur = parent_of.get(id).copied();
            }
            if !shadowed {
                queue.push((rule.clone(), (*intent).clone()));
            }
        }
    }
    NormativeCoverage {
        intents_with_code: candidates.len() as i64,
        total_pairs,
        measured_pairs,
        queue,
    }
}

/// An intent whose proof needs the validator's attention, with why.
#[derive(Debug, Clone)]
pub struct ValidateCandidate {
    pub intent: Intent,
    pub score: f64,
    /// What is wrong with this intent's proof (failing / never run / missing).
    pub reason: String,
}

pub fn quality_candidates_from_snapshot(snapshot: &QuerySnapshot) -> Vec<(Governs, f64)> {
    let active: std::collections::HashSet<&str> =
        snapshot.intents.iter().map(|i| i.id.as_str()).collect();
    let mut scored: Vec<(Governs, f64)> = Vec::new();
    for edge in &snapshot.governs {
        if !active.contains(edge.intent_id.as_str()) {
            continue;
        }
        let urgency = match edge.inspection_status.as_str() {
            "failing" => 4.0,
            "needs_reverification" => 3.0,
            "uninspected" => 2.0,
            _ => continue,
        };
        let deg = *snapshot.degrees.get(&edge.intent_id).unwrap_or(&0);
        scored.push((edge.clone(), deg as f64 + urgency));
    }
    for (rule, intent) in normative_coverage_from_snapshot(snapshot).queue {
        let deg = *snapshot.degrees.get(&intent.id).unwrap_or(&0);
        scored.push((
            Governs {
                id: String::new(),
                rule_id: rule.id,
                intent_id: intent.id.clone(),
                rule_name: rule.name,
                intent_name: intent.name.clone(),
                inspection_status: "unmeasured".to_string(),
                criterion: String::new(),
                confidence: 0.0,
                evidence: String::new(),
                last_inspected: String::new(),
                inspected_by: String::new(),
                notes: format!("detection: {}", rule.detection_logic),
            },
            deg as f64 + 1.0,
        ));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

pub fn validate_selection_from_snapshot(snapshot: &QuerySnapshot) -> Vec<(Intent, f64, String)> {
    let is_parent: std::collections::HashSet<String> =
        snapshot.hierarchy.iter().map(|(p, _)| p.clone()).collect();

    let mut edges_by_intent: HashMap<&str, Vec<&ValidatesEdge>> = HashMap::new();
    for edge in &snapshot.validates {
        edges_by_intent
            .entry(edge.intent_id.as_str())
            .or_default()
            .push(edge);
    }
    let val_by_id: HashMap<&str, &Validation> = snapshot
        .validations
        .iter()
        .map(|v| (v.id.as_str(), v))
        .collect();

    let mut selected: Vec<(Intent, f64, String)> = Vec::new();
    for intent in &snapshot.intents {
        let edges = edges_by_intent
            .get(intent.id.as_str())
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let (urgency, reason) = if edges.is_empty() {
            if intent.lifecycle == "implemented" && !is_parent.contains(&intent.id) {
                (
                    3.0,
                    "no proof: this implemented leaf intent has no validations".to_string(),
                )
            } else {
                continue;
            }
        } else {
            let validations: Vec<&Validation> = edges
                .iter()
                .filter_map(|e| val_by_id.get(e.validation_id.as_str()).copied())
                .collect();
            if validations.iter().any(|v| v.last_result == "failed")
                || edges.iter().any(|e| e.inspection_status == "failing")
            {
                (4.0, "a linked validation is failing".to_string())
            } else if let Some(v) = validations.iter().find(|v| {
                v.command.trim().is_empty()
                    && (v.last_result == "not_run" || v.last_result.is_empty())
            }) {
                (
                    2.0,
                    format!(
                        "validation '{}' has no command — needs `loom validation update {} --command \"…\"` or a manual `loom validation mark {}`",
                        v.name, v.id, v.id
                    ),
                )
            } else if validations
                .iter()
                .any(|v| v.last_result == "not_run" || v.last_result.is_empty())
            {
                (
                    2.0,
                    "linked validations have not been run (or were invalidated by a code change)"
                        .to_string(),
                )
            } else {
                continue;
            }
        };
        selected.push((intent.clone(), urgency, reason));
    }
    selected
}

/// A user↔intent drift suspect: active intent meaning whose surrounding claims
/// changed since the user last confirmed it.
#[derive(Debug, Clone)]
pub struct AlignCandidate {
    pub intent: crate::types::Intent,
    pub last_confirmed: Option<String>,
    pub churn_since_confirm: usize,
    pub degree: i64,
    pub score: f64,
}

/// Days a quiet meaning may sit unaffirmed before the slow sweep admits it to
/// the align queue without any churn. The grace period is the anti-needy
/// guard: a wording the user dictated yesterday is not a drift suspect. With
/// it, the queue DRAINS — `loom next --mode align` reporting empty is the
/// interview's honest stopping point, not the conversation petering out.
pub const ALIGN_GRACE_DAYS: f64 = 30.0;

pub fn align_candidates_from_snapshot_notes(
    snapshot: &QuerySnapshot,
    notes: &[Note],
) -> Vec<AlignCandidate> {
    let intents = &snapshot.intents;
    let degrees = &snapshot.degrees;
    // All notes once (the snapshot memoises the scan, so the smells + doctor
    // passes in the same `loom next --all` / orientation command reuse it
    // instead of each re-walking the multi-thousand-strong Note label),
    // partitioned by kind in one pass. The Note label is ordered newest-last,
    // so a later `insert` into the freshness maps overwrites with the latest
    // stamp — the same "newest wins" the separate filtered scans relied on — and
    // notes_by_target keeps transition notes in that same created-at order.
    let mut notes_by_target: HashMap<&str, Vec<&Note>> = HashMap::new();
    let mut confirmed_at: HashMap<&str, &str> = HashMap::new();
    let mut redefined_at: HashMap<&str, &str> = HashMap::new();
    for n in notes {
        match n.kind.as_str() {
            "transition" => notes_by_target
                .entry(n.target_id.as_str())
                .or_default()
                .push(n),
            "confirm" => {
                confirmed_at.insert(n.target_id.as_str(), n.created_at.as_str());
            }
            // `loom intent update --description` writes "redefined: …";
            // `--description --reword` writes "reworded: …". BOTH reset the
            // freshness clock (the meaning statement was just deliberately
            // restated), only the former ripples claims. Renames are cosmetic —
            // no stamp, no ripple, no clock reset.
            "decision" if n.text.starts_with("redefined: ") || n.text.starts_with("reworded: ") => {
                redefined_at.insert(n.target_id.as_str(), n.created_at.as_str());
            }
            _ => {}
        }
    }

    // Index every inspectable edge id by intent — one pass over the snapshot
    // instead of 3×N DB round-trips (`edges_for_intent` et al.).
    let mut edge_ids_by_intent: HashMap<&str, HashSet<&str>> = HashMap::new();
    for edge in &snapshot.relates {
        edge_ids_by_intent
            .entry(edge.from_id.as_str())
            .or_default()
            .insert(edge.id.as_str());
        edge_ids_by_intent
            .entry(edge.to_id.as_str())
            .or_default()
            .insert(edge.id.as_str());
    }
    for edge in &snapshot.governs {
        edge_ids_by_intent
            .entry(edge.intent_id.as_str())
            .or_default()
            .insert(edge.id.as_str());
    }
    for edge in &snapshot.implements {
        edge_ids_by_intent
            .entry(edge.intent_id.as_str())
            .or_default()
            .insert(edge.id.as_str());
    }

    // One clock read per scoring pass keeps equal-baseline candidates equal.
    // Reading inside the loop made tied align items depend on iteration order.
    let now = Utc::now();
    let mut candidates = Vec::new();
    for intent in intents {
        // "This is internal, don't ask the user again": a recorded
        // visibility=internal ruling takes the intent OUT of the interview.
        // The ruling is cleared by redefinition (`intent update --description`),
        // so "unless it changes" is exactly when it can come back.
        if intent.visibility == "internal" {
            continue;
        }
        let last_confirmed = confirmed_at.get(intent.id.as_str()).map(|s| s.to_string());
        // Newest of the confirm and redefinition stamps (RFC3339 sorts
        // lexicographically — sync freshness leans on the same property);
        // creation is the fallback when neither event ever happened.
        let redefined = redefined_at.get(intent.id.as_str()).copied();
        let baseline = match (last_confirmed.as_deref(), redefined) {
            (None, None) => intent.created_at.as_str(),
            (a, b) => a
                .into_iter()
                .chain(b)
                .max()
                .unwrap_or(intent.created_at.as_str()),
        };

        let edge_ids = edge_ids_by_intent
            .get(intent.id.as_str())
            .map(|s| s.iter().copied().collect::<HashSet<_>>())
            .unwrap_or_default();

        let mut churn_since_confirm = 0;
        for edge_id in &edge_ids {
            if let Some(notes) = notes_by_target.get(*edge_id) {
                churn_since_confirm += notes
                    .iter()
                    .filter(|note| {
                        // Strict `>`: a redefinition's own ripple flips share its
                        // timestamp and must not count against the new wording.
                        note.created_at.as_str() > baseline && note.text.contains("(sync: ")
                    })
                    .count();
            }
        }

        let degree = *degrees.get(intent.id.as_str()).unwrap_or(&0);
        let age_days = DateTime::parse_from_rfc3339(baseline)
            .ok()
            .map(|at| {
                let age = now.signed_duration_since(at.with_timezone(&Utc));
                age.num_seconds().max(0) as f64 / 86_400.0
            })
            .unwrap_or(0.0);
        if churn_since_confirm == 0 && age_days < ALIGN_GRACE_DAYS {
            // Fresh and quiet — asking would be noise. Skipping is what lets
            // the queue drain to the empty state the interview terminates on.
            continue;
        }
        let score =
            (1.0 + churn_since_confirm as f64) * (1.0 + (degree as f64).ln_1p()) + age_days / 90.0;

        candidates.push(AlignCandidate {
            intent: intent.clone(),
            last_confirmed,
            churn_since_confirm,
            degree,
            score,
        });
    }

    candidates.sort_by(|a, b| {
        let a_score = (a.score * 1_000_000.0).round() as i64;
        let b_score = (b.score * 1_000_000.0).round() as i64;
        b_score
            .cmp(&a_score)
            .then_with(|| a.intent.name.cmp(&b.intent.name))
            .then_with(|| a.intent.id.cmp(&b.intent.id))
    });
    candidates
}

pub fn prove_candidates_from_parts(
    hypotheses: Vec<Hypothesis>,
    targets: Vec<TargetsEdge>,
    degrees: &HashMap<String, i64>,
) -> Vec<(Hypothesis, f64)> {
    if hypotheses.is_empty() {
        return Vec::new();
    }
    let mut targets_by_h: HashMap<String, Vec<TargetsEdge>> = HashMap::new();
    for target in targets {
        targets_by_h
            .entry(target.hypothesis_id.clone())
            .or_default()
            .push(target);
    }

    let mut out: Vec<(Hypothesis, f64)> = Vec::new();
    for hypothesis in hypotheses {
        let targets = targets_by_h.get(hypothesis.id.as_str());
        let due = match hypothesis.status.as_str() {
            "proposed" => true,
            "supported" => targets.is_some_and(|ts| {
                ts.iter()
                    .any(|target| target.inspection_status == "needs_reverification")
            }),
            _ => false,
        };
        if !due {
            continue;
        }
        let reach: i64 = targets
            .map(|ts| {
                ts.iter()
                    .map(|target| degrees.get(&target.intent_id).copied().unwrap_or(0))
                    .sum()
            })
            .unwrap_or(0);
        out.push((hypothesis, 1.0 + reach as f64));
    }
    // Highest blast radius first; oldest breaks ties (nothing rots).
    out.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.created_at.cmp(&b.0.created_at))
    });
    out
}

pub fn validate_candidates_from_snapshot(snapshot: &QuerySnapshot) -> Vec<ValidateCandidate> {
    let selected = validate_selection_from_snapshot(snapshot);
    let mut scored: Vec<ValidateCandidate> = selected
        .into_iter()
        .map(|(intent, urgency, reason)| ValidateCandidate {
            score: *snapshot.degrees.get(&intent.id).unwrap_or(&0) as f64 + urgency,
            intent,
            reason,
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored
}

pub fn count_unexplored_pairs_from(
    active_intents: &[Intent],
    relates: &[RelatesTo],
    hierarchy: &[(String, String)],
) -> i64 {
    let active_ids: std::collections::HashSet<&str> =
        active_intents.iter().map(|i| i.id.as_str()).collect();
    let intent_count = active_intents.len() as i64;
    let mut linked: std::collections::HashSet<(&str, &str)> = std::collections::HashSet::new();
    fn key<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
        if a < b {
            (a, b)
        } else {
            (b, a)
        }
    }
    for e in relates {
        if active_ids.contains(e.from_id.as_str()) && active_ids.contains(e.to_id.as_str()) {
            linked.insert(key(&e.from_id, &e.to_id));
        }
    }
    for (p, c) in hierarchy {
        if active_ids.contains(p.as_str()) && active_ids.contains(c.as_str()) {
            linked.insert(key(p, c));
        }
    }
    (intent_count * (intent_count - 1) / 2 - linked.len() as i64).max(0)
}

/// Intent pairs that have NO RELATES_TO edge between them yet, returned as
/// synthetic "unexplored" candidates. Scored by combined centrality PLUS a
/// suspicion bonus — pairs that share implemented files, read alike, or live
/// in the same domain are the ones most likely to hide a real relationship
/// (or a split-brain), so the analyzer is pointed at them first instead of
/// grinding a flat N×N grid. The why travels in the synthetic edge's `notes`
/// so `loom next` can display it.
///
/// Takes a snapshot the caller already loaded — the `loom next` discovery
/// fall-through holds one, and a snapshot is a point-in-time read view, so
/// reusing it is identical to loading a fresh graph here (one read-only command
/// never loads the same graph twice).
/// The mechanical relationship kinds (the `populate` tier) present between two
/// intents — derived from extraction with no judgment. Reused by the synthetic
/// discovery pair and the `loom populate kinds` backfill so both agree on what
/// the graph mechanically knows about a coupling.
pub fn mechanical_kinds_for_pair(
    discovery: &DiscoverySnapshot,
    a: &Intent,
    b: &Intent,
) -> Vec<RelationKind> {
    let empty_files = std::collections::HashSet::new();
    let empty_tags = Vec::new();
    let fa = discovery.files_of.get(&a.id).unwrap_or(&empty_files);
    let fb = discovery.files_of.get(&b.id).unwrap_or(&empty_files);
    let mut kinds = Vec::new();
    let imports = fa
        .iter()
        .flat_map(|x| fb.iter().map(move |y| (*x, *y)))
        .chain(fb.iter().flat_map(|x| fa.iter().map(move |y| (*x, *y))))
        .any(|p| discovery.import_links.contains(&p));
    if imports {
        kinds.push(RelationKind::Imports);
    }
    if fa.intersection(fb).next().is_some() {
        kinds.push(RelationKind::SharesFile);
    }
    let (tag_weight, _) = super::vocab::shared_tag_weight(
        discovery.tags_by_intent.get(&a.id).unwrap_or(&empty_tags),
        discovery.tags_by_intent.get(&b.id).unwrap_or(&empty_tags),
        &discovery.tag_counts,
    );
    if tag_weight > 0.0 {
        kinds.push(RelationKind::SharesVocab);
    }
    if !a.domain.is_empty() && a.domain == b.domain && a.domain != "unknown" {
        kinds.push(RelationKind::SameDomain);
    }
    kinds
}

pub fn unexplored_pairs_scored_from_snapshot(
    snapshot: &QuerySnapshot,
    class_filter: DiscoveryClassFilter,
) -> Result<Vec<(RelatesTo, f64)>> {
    use super::smells::jaccard;

    let discovery = DiscoverySnapshot::from_query(snapshot)?;
    let linked: std::collections::HashSet<(&str, &str)> = discovery
        .linked
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let base_urgency = InspectionStatus::Uninspected.urgency();
    let empty_files = std::collections::HashSet::new();

    // Score one candidate pair by intent index. Returns None when the pair is
    // already linked, or — after computing its signals — its discovery class is
    // not the one requested. Identical predicates to the old all-pairs body, so
    // the candidate path below is a pure pruning of which (i, j) we evaluate.
    let score_pair = |i: usize, j: usize| -> Option<(RelatesTo, f64)> {
        {
            let a = &snapshot.intents[i];
            let b = &snapshot.intents[j];
            if linked.contains(&(a.id.as_str(), b.id.as_str())) {
                return None;
            }

            let fa = discovery.files_of.get(&a.id).unwrap_or(&empty_files);
            let fb = discovery.files_of.get(&b.id).unwrap_or(&empty_files);
            let shared = fa.intersection(fb).count();
            let sim = jaccard(
                &discovery.tokens_by_intent[&a.id],
                &discovery.tokens_by_intent[&b.id],
            );
            let same_domain = !a.domain.is_empty() && a.domain == b.domain && a.domain != "unknown";
            let empty_tags = Vec::new();
            let (tag_weight, shared_tags) = super::vocab::shared_tag_weight(
                discovery.tags_by_intent.get(&a.id).unwrap_or(&empty_tags),
                discovery.tags_by_intent.get(&b.id).unwrap_or(&empty_tags),
                &discovery.tag_counts,
            );
            let imports = fa
                .iter()
                .flat_map(|x| fb.iter().map(move |y| (*x, *y)))
                .filter(|p| discovery.import_links.contains(p))
                .count();
            let mut why: Vec<String> = Vec::new();
            let mut signals: Vec<DiscoverySignal> = Vec::new();
            if imports > 0 {
                let detail = format!("{imports} import link(s)");
                why.push(format!("their code imports each other ({imports} link(s))"));
                signals.push(DiscoverySignal {
                    kind: "import_link".to_string(),
                    detail,
                    weight: 5.0 * imports as f64,
                });
            }
            if shared > 0 {
                let mut paths: Vec<&str> = fa
                    .intersection(fb)
                    .filter_map(|idx| snapshot.codefiles.get(*idx).map(|cf| cf.path.as_str()))
                    .collect();
                paths.sort_unstable();
                let detail = if paths.len() <= 3 {
                    paths.join(", ")
                } else {
                    format!(
                        "{}, +{} more",
                        paths.iter().take(3).copied().collect::<Vec<_>>().join(", "),
                        paths.len() - 3
                    )
                };
                why.push(format!("share {shared} implemented file(s)"));
                signals.push(DiscoverySignal {
                    kind: "shared_file".to_string(),
                    detail,
                    weight: 3.0 * shared as f64,
                });
            }
            if sim >= 0.25 {
                why.push(format!("descriptions overlap ({sim:.2})"));
                signals.push(DiscoverySignal {
                    kind: "description_overlap".to_string(),
                    detail: format!("{sim:.2}"),
                    weight: 4.0 * sim,
                });
            }
            if tag_weight > 0.0 {
                // The structured `weight` field and the prose must agree — they
                // both report the SAME scored contribution (4.0 × tag_weight), not
                // the raw tag_weight in one place and the scaled value in the
                // other (an AI trusts the --json `weight` field over the prose).
                let weight = 4.0 * tag_weight;
                why.push(format!(
                    "tagged with the same vocabulary ({}, weight {weight:.2})",
                    shared_tags.join(", ")
                ));
                signals.push(DiscoverySignal {
                    kind: "shared_vocab".to_string(),
                    detail: shared_tags.join(", "),
                    weight,
                });
            }
            if same_domain {
                why.push(format!("same domain '{}'", a.domain));
                signals.push(DiscoverySignal {
                    kind: "same_domain".to_string(),
                    detail: a.domain.clone(),
                    weight: 1.0,
                });
            }
            // Tag collisions are graded by rarity (Σ 1/freq), so a collision on
            // a near-unique term outranks the binary same_domain bump — the
            // bounded vocabulary is the same signal domain wanted to be, with a
            // working denominator.
            let suspicion = 5.0 * imports as f64
                + 3.0 * shared as f64
                + 4.0 * sim
                + 4.0 * tag_weight
                + if same_domain { 1.0 } else { 0.0 };

            let degree_a = *snapshot.degrees.get(&a.id).unwrap_or(&0);
            let degree_b = *snapshot.degrees.get(&b.id).unwrap_or(&0);
            let discovery_class = if signals.is_empty() {
                why.push(format!(
                    "ranked by structural centrality only (degree {} + {})",
                    degree_a, degree_b
                ));
                "impact_map"
            } else {
                why.push(format!("structural degree {} + {}", degree_a, degree_b));
                "suspected_coupling"
            };
            if !class_filter.accepts(discovery_class) {
                return None;
            }

            let score = degree_a as f64 + degree_b as f64 + base_urgency + suspicion;
            Some((
                RelatesTo {
                    id: String::new(),
                    from_id: a.id.clone(),
                    to_id: b.id.clone(),
                    from_name: a.name.clone(),
                    to_name: b.name.clone(),
                    inspection_status: "unexplored".to_string(),
                    criterion: String::new(),
                    confidence: 0.0,
                    evidence: String::new(),
                    last_inspected: String::new(),
                    inspected_by: String::new(),
                    priority_score: score,
                    notes: format!("discovery signal: {}", why.join("; ")),
                    kinds: mechanical_kinds_for_pair(&discovery, a, b)
                        .into_iter()
                        .map(|k| k.as_str().to_string())
                        .collect(),
                    stable: false,
                    discovery_class: discovery_class.to_string(),
                    discovery_signals: signals,
                    discovery_centrality: DiscoveryCentrality {
                        a_degree: degree_a,
                        b_degree: degree_b,
                    },
                },
                score,
            ))
        }
    };

    let n = snapshot.intents.len();
    let mut scored: Vec<(RelatesTo, f64)> = Vec::new();
    match class_filter {
        // `impact_map` ranks the SIGNAL-LESS pairs by structural centrality, so it
        // genuinely needs every pair; `all` covers both classes → every pair too.
        // These deliberate "scan everything" requests keep the O(N²) walk.
        DiscoveryClassFilter::ImpactMap | DiscoveryClassFilter::All => {
            for i in 0..n {
                for j in (i + 1)..n {
                    if let Some(pair) = score_pair(i, j) {
                        scored.push(pair);
                    }
                }
            }
        }
        // The DEFAULT. Every `suspected_coupling` pair shares a file, an import
        // link, a tag, a domain, or a description token — so the candidate set
        // built from inverted indices is an EXACT superset, and scoring it is
        // O(candidates) instead of the O(N²) all-pairs scan (75s → sub-second on
        // a few-thousand-intent graph). `score_pair` re-applies the exact same
        // predicates, so a generated pair that doesn't actually qualify is dropped.
        DiscoveryClassFilter::SuspectedCoupling => {
            for (i, j) in suspected_coupling_candidates(snapshot, &discovery) {
                if let Some(pair) = score_pair(i, j) {
                    scored.push(pair);
                }
            }
        }
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored)
}

/// Candidate intent-index pairs for the `suspected_coupling` discovery class —
/// an EXACT superset of the signal-bearing pairs, assembled from inverted
/// indices so the default discovery scan is O(candidates) rather than O(N²).
///
/// Pairs sharing a file, an import link, or a tag are sparse and enumerated in
/// full. The DENSE facets — same-domain and shared-description-token — are
/// bounded by a per-bucket cap: a facet shared by more than `BUCKET_CAP` intents
/// is not discriminating (it only ever yields weakest-signal pairs that never
/// reach the served top of a large queue), so its O(k²) expansion is skipped.
/// The cap never fires below its size, so small graphs keep exact behavior; a
/// pair that also shares a sparse facet is still generated via that facet.
fn suspected_coupling_candidates(
    snapshot: &QuerySnapshot,
    discovery: &DiscoverySnapshot,
) -> Vec<(usize, usize)> {
    const BUCKET_CAP: usize = 64;

    let index: HashMap<&str, usize> = snapshot
        .intents
        .iter()
        .enumerate()
        .map(|(i, it)| (it.id.as_str(), i))
        .collect();
    let mut pairs: HashSet<(usize, usize)> = HashSet::new();

    // File ownership inversion: file index → its sorted, unique owning intents.
    let path_index: HashMap<&str, usize> = snapshot
        .codefiles
        .iter()
        .enumerate()
        .map(|(i, cf)| (cf.path.as_str(), i))
        .collect();
    let mut owners_of_file: HashMap<usize, Vec<usize>> = HashMap::new();
    for (path, intent_ids) in &discovery.intents_on_file {
        let Some(&file_idx) = path_index.get(path.as_str()) else {
            continue;
        };
        let mut members: Vec<usize> = intent_ids
            .iter()
            .filter_map(|id| index.get(id.as_str()).copied())
            .collect();
        members.sort_unstable();
        members.dedup();
        owners_of_file.insert(file_idx, members);
    }
    // File-sharing candidates (sparse, enumerated in full).
    for members in owners_of_file.values() {
        add_intra_bucket_pairs(members, &mut pairs);
    }
    // Import-link candidates: file x imports file y → owners(x) × owners(y).
    for (x, y) in &discovery.import_links {
        let (Some(ox), Some(oy)) = (owners_of_file.get(x), owners_of_file.get(y)) else {
            continue;
        };
        for &a in ox {
            for &b in oy {
                if a != b {
                    pairs.insert(if a < b { (a, b) } else { (b, a) });
                }
            }
        }
    }
    // Shared-tag candidates (sparse).
    add_inverted_bucket_pairs(
        discovery.tags_by_intent.iter().flat_map(|(id, tags)| {
            index
                .get(id.as_str())
                .copied()
                .into_iter()
                .flat_map(move |i| tags.iter().map(move |t| (t.as_str(), i)))
        }),
        usize::MAX,
        &mut pairs,
    );
    // Same-domain candidates (DENSE → capped).
    add_inverted_bucket_pairs(
        snapshot.intents.iter().enumerate().filter_map(|(i, it)| {
            (!it.domain.is_empty() && it.domain != "unknown").then_some((it.domain.as_str(), i))
        }),
        BUCKET_CAP,
        &mut pairs,
    );
    // Shared-description-token candidates (DENSE → capped).
    add_inverted_bucket_pairs(
        discovery.tokens_by_intent.iter().flat_map(|(id, tokens)| {
            index
                .get(id.as_str())
                .copied()
                .into_iter()
                .flat_map(move |i| tokens.iter().map(move |t| (t.as_str(), i)))
        }),
        BUCKET_CAP,
        &mut pairs,
    );

    pairs.into_iter().collect()
}

/// All unordered pairs of `members` (assumed small), inserted canonically.
fn add_intra_bucket_pairs(members: &[usize], pairs: &mut HashSet<(usize, usize)>) {
    for p in 0..members.len() {
        for q in (p + 1)..members.len() {
            let (a, b) = (members[p], members[q]);
            pairs.insert(if a < b { (a, b) } else { (b, a) });
        }
    }
}

/// Group `(key, intent_index)` items by key, then emit every intra-group pair —
/// EXCEPT for a group larger than `cap`, whose non-discriminating O(k²) blowup
/// is skipped. `cap = usize::MAX` keeps the facet uncapped (sparse facets).
fn add_inverted_bucket_pairs<'a, I: Iterator<Item = (&'a str, usize)>>(
    items: I,
    cap: usize,
    pairs: &mut HashSet<(usize, usize)>,
) {
    let mut buckets: HashMap<&str, Vec<usize>> = HashMap::new();
    for (key, idx) in items {
        buckets.entry(key).or_default().push(idx);
    }
    for members in buckets.values_mut() {
        members.sort_unstable();
        members.dedup();
        if members.len() > cap {
            continue;
        }
        add_intra_bucket_pairs(members, pairs);
    }
}

/// Per-lane queue depths for the autonomous work modes, computed in one pass
/// over an already-loaded snapshot. `loom status` uses this for its "other open
/// lanes" footer so the single-pointer compass doesn't hide that other lanes
/// have work; the numbers are the SAME selections `loom next --mode <lane>`
/// serves (coherence by construction). All counts are pure in-memory derivation
/// over the snapshot — no extra DB I/O.
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct LaneDepths {
    pub build: i64,
    pub fix: i64,
    pub validate: i64,
    pub quality: i64,
}

pub fn lane_depths_from_snapshot(snapshot: &QuerySnapshot) -> LaneDepths {
    LaneDepths {
        build: build_candidates_from_snapshot(snapshot).len() as i64,
        fix: scored_candidates_from_snapshot(snapshot, "fix").len() as i64,
        validate: validate_candidates_from_snapshot(snapshot).len() as i64,
        quality: quality_candidates_from_snapshot(snapshot).len() as i64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(id: &str) -> Intent {
        Intent {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            criterion: String::new(),
            abstraction_level: "feature".to_string(),
            domain: "test".to_string(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "implemented".to_string(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: String::new(),
            boundary: String::new(),
            lifecycle: "implemented".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn rule(id: &str) -> QualityRule {
        QualityRule {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            detection_logic: String::new(),
            kind: String::new(),
            severity: "medium".to_string(),
            inspection_effort: "mid".to_string(),
        }
    }

    fn relates(id: &str, status: &str, confidence: f64) -> RelatesTo {
        RelatesTo {
            id: id.to_string(),
            from_id: "a".to_string(),
            to_id: "b".to_string(),
            from_name: "a".to_string(),
            to_name: "b".to_string(),
            inspection_status: status.to_string(),
            criterion: String::new(),
            confidence,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: "llm:analyzer".to_string(),
            priority_score: 0.0,
            notes: String::new(),
            kinds: Vec::new(),
            stable: false,
            discovery_class: String::new(),
            discovery_signals: Vec::new(),
            discovery_centrality: DiscoveryCentrality::default(),
        }
    }

    fn governs(id: &str, status: &str, confidence: f64) -> Governs {
        Governs {
            id: id.to_string(),
            rule_id: "rule".to_string(),
            intent_id: "a".to_string(),
            rule_name: "rule".to_string(),
            intent_name: "a".to_string(),
            inspection_status: status.to_string(),
            criterion: String::new(),
            confidence,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: "llm:quality".to_string(),
            notes: String::new(),
        }
    }

    #[test]
    fn review_queue_ignores_independent_verdict_confidence() {
        let snapshot = QuerySnapshot::from_parts(
            vec![intent("a"), intent("b")],
            Vec::new(),
            vec![
                relates("rel-passing-low", "passing", 0.6),
                relates("rel-failing-low", "failing", 0.6),
                relates("rel-independent-low", "independent", 0.6),
                relates("rel-passing-high", "passing", 0.8),
                relates("rel-passing-zero", "passing", 0.0),
            ],
            vec![
                governs("gov-passing-low", "passing", 0.6),
                governs("gov-failing-low", "failing", 0.6),
                governs("gov-independent-low", "independent", 0.6),
                governs("gov-passing-high", "passing", 0.8),
                governs("gov-passing-zero", "passing", 0.0),
            ],
            vec![rule("rule")],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(Vec::new()),
        );

        let mut relates_ids = Vec::new();
        let mut governs_ids = Vec::new();
        for (candidate, _) in review_candidates_from_snapshot(&snapshot) {
            match candidate {
                ReviewCandidate::RelatesTo(edge) => relates_ids.push(edge.id),
                ReviewCandidate::Governs(edge) => governs_ids.push(edge.id),
            }
        }
        relates_ids.sort();
        governs_ids.sort();

        assert_eq!(relates_ids, ["rel-failing-low", "rel-passing-low"]);
        assert_eq!(governs_ids, ["gov-failing-low", "gov-passing-low"]);
    }
}
