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

/// Per-bucket cap for the DENSE discovery facets (same-domain and
/// shared-description-token). A facet shared by more than this many intents is
/// not discriminating — its O(k²) expansion would only ever yield
/// weakest-signal pairs that never reach the served top of a large queue — so
/// `suspected_coupling_candidates` skips it. The cap never fires below its size,
/// so small graphs keep exact behavior. `capped_discovery_buckets` reports which
/// buckets this excluded so the suspected-coupling lane can DISCLOSE the
/// elision instead of pruning silently (the exhaustive `--class all` walk
/// ignores the cap and remains the honest escape hatch).
pub const BUCKET_CAP: usize = 64;

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

/// A dense discovery facet bucket that exceeded [`BUCKET_CAP`] and was therefore
/// excluded from `suspected_coupling` candidate generation. Surfaced by
/// [`capped_discovery_buckets`] so the default discovery lane can DISCLOSE the
/// elision (the pairs are still owed and enumerable under `--class all`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct CappedBucket {
    /// Which facet the bucket belongs to: `"domain"` or `"description_token"`.
    pub facet: &'static str,
    /// The shared value (the domain name, or the description token).
    pub key: String,
    /// How many intents share this facet value (> [`BUCKET_CAP`]).
    pub members: usize,
}

/// The exhaustive escape hatch that ignores the dense-bucket cap — the honest
/// recovery path for any pair the suspected-coupling lane deprioritized.
pub const DISCOVERY_ESCAPE_HATCH: &str = "loom next --mode discovery --class all";

/// One-line human disclosure that the suspected-coupling lane excluded `N` dense
/// facet buckets from prioritization. `None` when nothing was capped (small
/// graphs, or any lane other than suspected-coupling), so callers can print it
/// unconditionally without leaking noise into the common case.
pub fn bucket_disclosure_line(capped: &[CappedBucket]) -> Option<String> {
    let top = capped.first()?;
    Some(format!(
        "ⓘ {} dense bucket(s) (domain / description-token facets with >{} members) excluded from suspected-coupling prioritization — largest: {} '{}' ({} intents). Still owed; run `{}` to enumerate them.",
        capped.len(),
        BUCKET_CAP,
        top.facet,
        top.key,
        top.members,
        DISCOVERY_ESCAPE_HATCH,
    ))
}

/// Attach `capped_buckets` + `discovery_escape_hatch` to a JSON object when any
/// bucket was excluded (omitted entirely otherwise, so quiet graphs stay quiet).
pub fn inject_capped_buckets(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    capped: &[CappedBucket],
) {
    if capped.is_empty() {
        return;
    }
    if let Ok(v) = serde_json::to_value(capped) {
        obj.insert("capped_buckets".to_string(), v);
        obj.insert(
            "discovery_escape_hatch".to_string(),
            DISCOVERY_ESCAPE_HATCH.into(),
        );
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
        .filter(|edge| is_relates_candidate(edge, mode))
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

/// Whether a RELATES_TO edge belongs to `mode`'s candidate queue. The single
/// source of truth shared by the scored ranking and the count-only path, so the
/// two can never drift on which edges count.
fn is_relates_candidate(edge: &RelatesTo, mode: &str) -> bool {
    match mode {
        "fix" => matches!(
            edge.inspection_status.as_str(),
            "failing" | "needs_reverification"
        ),
        _ => edge.inspection_status == "uninspected",
    }
}

/// How many edges `mode`'s queue holds — the SAME set `scored_candidates_from_snapshot`
/// returns, counted WITHOUT the Brandes betweenness pass that only affects
/// ranking. `loom status` (turn-zero, the most-run command) needs the depth, not
/// the order, so it must not pay O(V·E) centrality just to print a number.
pub fn relates_candidate_count_from_snapshot(snapshot: &QuerySnapshot, mode: &str) -> usize {
    let active: std::collections::HashSet<&str> =
        snapshot.intents.iter().map(|i| i.id.as_str()).collect();
    let mut seen = std::collections::HashSet::new();
    snapshot
        .relates
        .iter()
        .filter(|edge| is_relates_candidate(edge, mode))
        .filter(|edge| {
            active.contains(edge.from_id.as_str()) && active.contains(edge.to_id.as_str())
        })
        .filter(|edge| seen.insert(edge.id.as_str()))
        .count()
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
/// a verdict on a component covers its descendants ONLY with --covers-descendants
/// (measuring at the highest honest altitude is the encouraged strategy, never punished).
pub struct NormativeCoverage {
    /// Non-deprecated intents that have real code (≥1 IMPLEMENTS).
    pub intents_with_code: i64,
    /// rules × intents_with_code — the full measuring grid.
    pub total_pairs: i64,
    /// Pairs considered: a GOVERNS edge with an inspected status (passing|failing|independent|partial), directly or on an ancestor (ancestor covers descendants only when covers_descendants=true).
    pub measured_pairs: i64,
    /// Unmeasured pairs at the HIGHEST altitude only — the actual work queue.
    /// (An unmeasured intent whose ancestor is also unmeasured is omitted: one
    /// verdict up there covers it. Bounded by #rules × #top-level branches.)
    pub queue: Vec<(QualityRule, Intent)>,
}

/// Passing/failing verdicts below this confidence always enter the review queue.
/// High-risk GOVERNS passing/partial verdicts have an additional finite
/// double-check path below; this constant remains the threshold a re-recorded
/// verdict must meet to leave uncertainty-driven review.
pub const REVIEW_CONFIDENCE: f64 = 0.7;

/// One claim for the reviewer: low-confidence or empty-evidence passing/failing
/// verdicts always queue, and high-risk GOVERNS pass/partial verdicts queue until
/// the edge has been re-recorded after creation. That makes review a finite
/// double-check: the first high-risk green claim asks for review, the
/// re-inspection updates `last_inspected`, and the item deterministically leaves
/// the optional queue.
#[derive(Debug, Clone)]
pub enum ReviewCandidate {
    RelatesTo(RelatesTo),
    Governs(Governs),
}

pub fn review_candidates_from_snapshot(snapshot: &QuerySnapshot) -> Vec<(ReviewCandidate, f64)> {
    let active: std::collections::HashSet<&str> =
        snapshot.intents.iter().map(|i| i.id.as_str()).collect();
    let needs_review = |status: &str, confidence: f64, evidence: &str| {
        if !matches!(status, "passing" | "failing") || confidence == 0.0 {
            return false;
        }
        // Low-confidence verdicts always need a second look.
        if confidence < REVIEW_CONFIDENCE {
            return true;
        }
        // A passing/failing verdict with NO evidence is a laundered claim —
        // the doctor detects the aggregate pattern (near-uniform confidence
        // + few evidence strings), but routing each empty-evidence verdict to
        // the review queue makes the smell individually actionable. An honest
        // inspection always records what it saw.
        evidence.trim().is_empty()
    };
    let review_confirmed_after_creation = |edge: &Governs| {
        !edge.created_at.is_empty()
            && !edge.last_inspected.is_empty()
            && edge.last_inspected > edge.created_at
    };
    let mut scored: Vec<(ReviewCandidate, f64)> = Vec::new();
    for edge in &snapshot.relates {
        if !needs_review(&edge.inspection_status, edge.confidence, &edge.evidence)
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
    // Build a rule severity lookup for GOVERNS review triggers.
    let rule_severity: std::collections::HashMap<&str, &str> = snapshot
        .rules
        .iter()
        .map(|r| (r.id.as_str(), r.severity.as_str()))
        .collect();
    // Build an intent altitude lookup for the high-altitude review trigger.
    let intent_altitude: std::collections::HashMap<&str, &str> = snapshot
        .intents
        .iter()
        .map(|i| (i.id.as_str(), i.abstraction_level.as_str()))
        .collect();
    for edge in &snapshot.governs {
        if !active.contains(edge.intent_id.as_str()) {
            continue;
        }
        // The base needs_review check (low confidence or empty evidence).
        let base_review = needs_review(&edge.inspection_status, edge.confidence, &edge.evidence);
        // Additional GOVERNS-specific triggers route risky green claims to review
        // once. A high-confidence re-record updates last_inspected while preserving
        // created_at, so the same edge does not re-queue forever. `partial`
        // remains open-ended: it is explicitly not fully discharged, so it leaves
        // review only when changed to passing/failing/independent.
        let severity = rule_severity
            .get(edge.rule_id.as_str())
            .copied()
            .unwrap_or("");
        let altitude = intent_altitude
            .get(edge.intent_id.as_str())
            .copied()
            .unwrap_or("");
        let is_high_severity = severity == "error";
        let is_high_altitude = altitude == "system" || altitude == "cross_cutting";
        let is_partial = edge.inspection_status == "partial";
        let high_risk_passing =
            edge.inspection_status == "passing" && (is_high_severity || is_high_altitude);
        let extra_review = edge.confidence > 0.0
            && (is_partial || (high_risk_passing && !review_confirmed_after_creation(edge)));
        if !base_review && !extra_review {
            continue;
        }
        let deg = *snapshot.degrees.get(&edge.intent_id).unwrap_or(&0) as f64;
        // Boost score for the extra triggers so they surface first.
        let boost = if is_high_severity { 2.0 } else { 0.0 }
            + if is_high_altitude { 1.0 } else { 0.0 }
            + if is_partial { 1.0 } else { 0.0 };
        let score = (1.0 - edge.confidence) * (deg + 1.0) + boost;
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
        .filter(|g| {
            matches!(
                g.inspection_status.as_str(),
                "passing" | "failing" | "independent" | "partial"
            )
        })
        .map(|g| (g.rule_id.as_str(), g.intent_id.as_str()))
        .collect();
    let covers_set = covers_descendants_set(&snapshot.governs);
    let parent_of: HashMap<&str, &str> = snapshot
        .hierarchy
        .iter()
        .map(|(p, c)| (c.as_str(), p.as_str()))
        .collect();
    // The shared coverage predicate: direct edge always covers; ancestor edge
    // covers ONLY when covers_descendants=true on that ancestor's pair.
    let considered_up = |rule_id: &str, intent_id: &str| -> bool {
        governs_covers_intent(rule_id, intent_id, &considered, &covers_set, &parent_of)
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

/// A GOVERNS edge "covers" an intent when either:
/// - it's a DIRECT edge on that intent (any inspected status), OR
/// - it's an ANCESTOR edge (parent, grandparent, ...) with `covers_descendants=true`.
///
/// This is the SINGLE coverage predicate shared by `normative_coverage_from_snapshot`,
/// the `unmeasured_intents` smell, and the `unmeasured_intents` alarm in status.
/// Before v12, all three treated ANY ancestor GOVERNS as covering descendants,
/// producing a false green: a component-level verdict with `covers_descendants=false`
/// (the default) would suppress the quality queue for its children. Now only
/// `--covers-descendants` verdicts roll up.
///
/// `considered` must already filter to inspected statuses (passing/failing/
/// independent/partial) — `uninspected` and `needs_reverification` are NOT
/// measurements. The caller is responsible for that filter; this predicate
/// only adds the `covers_descendants` dimension.
pub fn governs_covers_intent(
    rule_id: &str,
    intent_id: &str,
    considered: &std::collections::HashSet<(&str, &str)>,
    covers_set: &std::collections::HashSet<(&str, &str)>,
    parent_of: &HashMap<&str, &str>,
) -> bool {
    let mut cur = Some(intent_id);
    let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
    while let Some(id) = cur {
        if !visited.insert(id) {
            return false;
        }
        if considered.contains(&(rule_id, id)) {
            // Direct edge covers always. Ancestor edge covers only when
            // covers_descendants is set on THAT ancestor's (rule, intent) pair.
            if id == intent_id || covers_set.contains(&(rule_id, id)) {
                return true;
            }
        }
        cur = parent_of.get(id).copied();
    }
    false
}

/// Build the set of (rule_id, intent_id) pairs where `covers_descendants=true`.
pub fn covers_descendants_set(governs: &[Governs]) -> std::collections::HashSet<(&str, &str)> {
    governs
        .iter()
        .filter(|g| g.covers_descendants == "true")
        .map(|g| (g.rule_id.as_str(), g.intent_id.as_str()))
        .collect()
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
                created_at: String::new(),
                covers_descendants: String::new(),
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
            } else if intent.lifecycle == "implemented"
                && !is_parent.contains(&intent.id)
                && !validations
                    .iter()
                    .any(|v| v.discrimination_status == "discriminating")
            {
                // Asserted-only: the proof PASSES but no runner discriminated
                // (asserted >=1 thing). Realized needs an EXECUTED proof, so this
                // leaf is real validate work — the queue must serve it, not skip
                // it as "green". This is the discriminating-proof gap that the
                // ladder's Realized rung counts; surfacing it here converges the
                // cascade with the ladder (no more silent false-green).
                (
                    2.5,
                    "asserted-only — the linked proof passes but does NOT discriminate (no runner asserted >=1 thing). Write a discriminating test whose output a runner reports (cargo `test result: ok. N passed`, pytest/jest `N passed`, mocha `N passing`, node --test `# pass N`, go `--- PASS:`, unittest `Ran N tests` + `OK`), link it, then re-run `loom validate <intent>` — otherwise it cannot count toward Realized.".to_string(),
                )
            } else {
                continue;
            }
        };
        selected.push((intent.clone(), urgency, reason));
    }
    let already_selected: std::collections::HashSet<String> = selected
        .iter()
        .map(|(intent, _, _)| intent.id.clone())
        .collect();
    let by_id: HashMap<&str, &Intent> = snapshot
        .intents
        .iter()
        .map(|intent| (intent.id.as_str(), intent))
        .collect();
    for owed in crate::db::queries::comprehensiveness::journey_ledger_from_snapshot(snapshot).owed {
        if already_selected.contains(&owed.id) {
            continue;
        }
        if let Some(intent) = by_id.get(owed.id.as_str()) {
            selected.push((
                (*intent).clone(),
                2.7,
                "missing journey proof: user-visible leaf has no passing discriminating boundary saga".to_string(),
            ));
        }
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
            // Boundary-crossing SURPRISE: a real structural coupling (imports or
            // a shared file) between two intents the architecture keeps APART —
            // different domains, or different layers. The code couples what the
            // design separates, so this is the most architecturally suspicious
            // kind of undeclared coupling (a leak / misplaced responsibility) and
            // earns the strongest discovery bump so it surfaces FIRST. An
            // UNcoupled cross-domain pair is just unrelated, not surprising — the
            // structural-coupling guard is what makes this signal, not noise.
            let cross_domain = !a.domain.is_empty()
                && !b.domain.is_empty()
                && a.domain != "unknown"
                && b.domain != "unknown"
                && a.domain != b.domain;
            let cross_layer = !a.layer.is_empty() && !b.layer.is_empty() && a.layer != b.layer;
            let boundary_surprise = (imports > 0 || shared > 0) && (cross_domain || cross_layer);
            if boundary_surprise {
                let mut crossings: Vec<String> = Vec::new();
                if cross_domain {
                    crossings.push(format!("domain {} ✗ {}", a.domain, b.domain));
                }
                if cross_layer {
                    crossings.push(format!("layer {} ✗ {}", a.layer, b.layer));
                }
                let detail = crossings.join(", ");
                why.push(format!(
                    "coupling CROSSES an architectural boundary ({detail}) — surprising; inspect first"
                ));
                signals.push(DiscoverySignal {
                    kind: "boundary_crossing".to_string(),
                    detail,
                    weight: 6.0,
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
                + if same_domain { 1.0 } else { 0.0 }
                + if boundary_surprise { 6.0 } else { 0.0 };

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

/// The dense discovery facet buckets that exceeded [`BUCKET_CAP`] and were
/// therefore excluded from `suspected_coupling` candidate generation — the basis
/// for the suspected-coupling lane's elision disclosure. Mirrors EXACTLY the two
/// capped facets in [`suspected_coupling_candidates`] (same-domain and
/// shared-description-token) so the disclosed count matches what was skipped.
/// Cheap: one pass each over intents and the discovery tokens. Returns empty on
/// small graphs (the cap never fires), so it adds no noise in the common case.
/// Sorted by `members` desc (biggest, most-suspect bucket first), then by key.
pub fn capped_discovery_buckets(snapshot: &QuerySnapshot) -> Result<Vec<CappedBucket>> {
    let discovery = DiscoverySnapshot::from_query(snapshot)?;

    let mut out: Vec<CappedBucket> = Vec::new();

    // Same-domain facet: group active intents by domain (skip empty/unknown,
    // exactly as the candidate generator does).
    let mut domain_counts: HashMap<&str, usize> = HashMap::new();
    for it in &snapshot.intents {
        if !it.domain.is_empty() && it.domain != "unknown" {
            *domain_counts.entry(it.domain.as_str()).or_default() += 1;
        }
    }
    for (domain, members) in domain_counts {
        if members > BUCKET_CAP {
            out.push(CappedBucket {
                facet: "domain",
                key: domain.to_string(),
                members,
            });
        }
    }

    // Shared-description-token facet: invert tokens_by_intent into token → count.
    let mut token_counts: HashMap<&str, usize> = HashMap::new();
    for tokens in discovery.tokens_by_intent.values() {
        for t in tokens {
            *token_counts.entry(t.as_str()).or_default() += 1;
        }
    }
    for (token, members) in token_counts {
        if members > BUCKET_CAP {
            out.push(CappedBucket {
                facet: "description_token",
                key: token.to_string(),
                members,
            });
        }
    }

    out.sort_by(|a, b| {
        b.members
            .cmp(&a.members)
            .then_with(|| a.facet.cmp(b.facet))
            .then_with(|| a.key.cmp(&b.key))
    });
    Ok(out)
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
        // Count-only: the depth, not the ranking — skips Brandes betweenness.
        fix: relates_candidate_count_from_snapshot(snapshot, "fix") as i64,
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
            evidence_examples: String::new(),
            signal_expectations: String::new(),
            applies_when: String::new(),
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

    impl RelatesTo {
        fn with_evidence(mut self, evidence: &str) -> Self {
            self.evidence = evidence.to_string();
            self
        }
    }

    fn governs(id: &str, status: &str, confidence: f64) -> Governs {
        Governs {
            covers_descendants: String::new(),
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
            created_at: String::new(),
        }
    }

    impl Governs {
        fn with_evidence(mut self, evidence: &str) -> Self {
            self.evidence = evidence.to_string();
            self
        }

        fn with_timestamps(mut self, created_at: &str, last_inspected: &str) -> Self {
            self.created_at = created_at.to_string();
            self.last_inspected = last_inspected.to_string();
            self
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
                relates("rel-passing-high", "passing", 0.8).with_evidence("inspected coupling"),
                relates("rel-passing-zero", "passing", 0.0),
            ],
            vec![
                governs("gov-passing-low", "passing", 0.6),
                governs("gov-failing-low", "failing", 0.6),
                governs("gov-independent-low", "independent", 0.6),
                governs("gov-passing-high", "passing", 0.8).with_evidence("inspected compliance"),
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

    #[test]
    fn review_queue_routes_empty_evidence_verdicts_even_at_high_confidence() {
        // A verdict with no evidence is a laundered claim — the doctor detects
        // the aggregate pattern, but routing each one to review makes the smell
        // individually actionable. Confidence alone can't certify what was never
        // inspected.
        let snapshot = QuerySnapshot::from_parts(
            vec![intent("a"), intent("b")],
            Vec::new(),
            vec![
                relates("rel-passing-high-no-evidence", "passing", 0.9),
                relates("rel-passing-high-with-evidence", "passing", 0.9)
                    .with_evidence("code shows import coupling at src/a.rs:3"),
            ],
            vec![],
            vec![rule("rule")],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(Vec::new()),
        );

        let mut ids = Vec::new();
        for (candidate, _) in review_candidates_from_snapshot(&snapshot) {
            if let ReviewCandidate::RelatesTo(edge) = candidate {
                ids.push(edge.id);
            }
        }
        assert_eq!(
            ids,
            ["rel-passing-high-no-evidence"],
            "high-confidence verdict WITH evidence stays out of review; \
             empty-evidence verdict enters regardless of confidence"
        );
    }

    #[test]
    fn high_risk_governs_review_closes_after_rerecord() {
        let mut error_rule = rule("rule");
        error_rule.severity = "error".to_string();
        let snapshot = QuerySnapshot::from_parts(
            vec![intent("a")],
            Vec::new(),
            Vec::new(),
            vec![
                governs("gov-error-first-pass", "passing", 0.9)
                    .with_evidence("inspected the error path at src/a.rs:10")
                    .with_timestamps("2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
                governs("gov-error-rerecorded", "passing", 0.9)
                    .with_evidence("re-inspected the error path at src/a.rs:10")
                    .with_timestamps("2026-01-01T00:00:00Z", "2026-01-01T00:01:00Z"),
            ],
            vec![error_rule],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(Vec::new()),
        );

        let mut ids = Vec::new();
        for (candidate, _) in review_candidates_from_snapshot(&snapshot) {
            if let ReviewCandidate::Governs(edge) = candidate {
                ids.push(edge.id);
            }
        }
        ids.sort();

        assert_eq!(
            ids,
            ["gov-error-first-pass"],
            "high-risk passing GOVERNS verdicts queue once, then a re-recorded \
             verdict leaves optional review deterministically"
        );
    }

    #[test]
    fn partial_governs_review_does_not_close_by_rerecording_partial() {
        let snapshot = QuerySnapshot::from_parts(
            vec![intent("a")],
            Vec::new(),
            Vec::new(),
            vec![governs("gov-partial-rerecorded", "partial", 0.9)
                .with_evidence("only part of the subtree has located evidence")
                .with_timestamps("2026-01-01T00:00:00Z", "2026-01-01T00:01:00Z")],
            vec![rule("rule")],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(Vec::new()),
        );

        let mut ids = Vec::new();
        for (candidate, _) in review_candidates_from_snapshot(&snapshot) {
            if let ReviewCandidate::Governs(edge) = candidate {
                ids.push(edge.id);
            }
        }

        assert_eq!(
            ids,
            ["gov-partial-rerecorded"],
            "partial remains reviewable until the verdict is fully discharged"
        );
    }

    // SWEEP #12: `loom status`'s fix-lane depth must count the SAME edges the
    // scored ranking selects — but without paying the Brandes betweenness pass
    // (which only affects order). The count-only path and `scored.len()` must agree.
    #[test]
    fn fix_lane_count_matches_scored_len_without_betweenness() {
        let snapshot = QuerySnapshot::from_parts(
            vec![intent("a"), intent("b")],
            Vec::new(),
            vec![
                relates("rel-failing", "failing", 0.6),
                relates("rel-needs-rev", "needs_reverification", 0.6),
                relates("rel-passing", "passing", 0.6), // not a fix candidate
                relates("rel-uninspected", "uninspected", 0.6), // discovery, not fix
            ],
            Vec::new(),
            vec![rule("rule")],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(Vec::new()),
        );
        assert_eq!(
            relates_candidate_count_from_snapshot(&snapshot, "fix"),
            scored_candidates_from_snapshot(&snapshot, "fix").len(),
            "the count-only status path must select the same set as the scored ranking"
        );
        // And it actually counts the two fix-candidate edges.
        assert_eq!(relates_candidate_count_from_snapshot(&snapshot, "fix"), 2);
    }

    fn note(target: &str, kind: &str, text: &str, created_at: &str) -> Note {
        Note {
            id: format!("{target}-{created_at}"),
            target_id: target.to_string(),
            target_kind: "edge".to_string(),
            kind: kind.to_string(),
            text: text.to_string(),
            author: "test".to_string(),
            audience: String::new(),
            created_at: created_at.to_string(),
            resolution: String::new(),
        }
    }

    fn align_snap(
        intents: Vec<Intent>,
        relates: Vec<RelatesTo>,
        notes: Vec<Note>,
    ) -> Vec<AlignCandidate> {
        let snapshot = QuerySnapshot::from_parts(
            intents,
            Vec::new(),
            relates,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(Vec::new()),
        );
        align_candidates_from_snapshot_notes(&snapshot, &notes)
    }

    #[test]
    fn align_ranks_churned_unconfirmed_intent_first() {
        let mut hot = intent("hot");
        hot.created_at = "2020-01-01T00:00:00Z".into();
        let mut quiet = intent("quiet");
        quiet.created_at = "2020-01-01T00:00:00Z".into();
        let rel = RelatesTo {
            id: "rel-1".into(),
            from_id: "hot".into(),
            to_id: "quiet".into(),
            from_name: "hot".into(),
            to_name: "quiet".into(),
            inspection_status: "passing".into(),
            criterion: String::new(),
            confidence: 0.9,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: "llm:analyzer".into(),
            priority_score: 0.0,
            notes: String::new(),
            kinds: Vec::new(),
            stable: false,
            discovery_class: String::new(),
            discovery_signals: Vec::new(),
            discovery_centrality: DiscoveryCentrality::default(),
        };
        let notes = vec![note(
            "rel-1",
            "transition",
            "stale (sync: src/x.rs changed)",
            "2025-06-01T00:00:00Z",
        )];
        let ranked = align_snap(vec![hot.clone(), quiet], vec![rel], notes);
        assert_eq!(ranked.len(), 2, "both old intents are eligible");
        assert_eq!(
            ranked[0].intent.id, "hot",
            "churn outranks quiet age-only: {ranked:?}"
        );
        assert_eq!(ranked[0].churn_since_confirm, 1);
    }

    #[test]
    fn align_ignores_retired_intents() {
        let mut internal = intent("internal-only");
        internal.visibility = "internal".into();
        internal.created_at = "2020-01-01T00:00:00Z".into();
        let ranked = align_snap(
            vec![internal],
            Vec::new(),
            vec![note(
                "internal-only",
                "transition",
                "stale (sync: anything)",
                "2025-06-01T00:00:00Z",
            )],
        );
        assert!(
            ranked.is_empty(),
            "internal machinery is never interview material"
        );
    }

    #[test]
    fn align_churn_before_confirm_not_counted() {
        let mut fresh = intent("fresh");
        fresh.created_at = "2026-06-01T00:00:00Z".into();
        let rel = RelatesTo {
            id: "rel-2".into(),
            from_id: "fresh".into(),
            to_id: "other".into(),
            from_name: "fresh".into(),
            to_name: "other".into(),
            inspection_status: "passing".into(),
            criterion: String::new(),
            confidence: 0.9,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: "llm:analyzer".into(),
            priority_score: 0.0,
            notes: String::new(),
            kinds: Vec::new(),
            stable: false,
            discovery_class: String::new(),
            discovery_signals: Vec::new(),
            discovery_centrality: DiscoveryCentrality::default(),
        };
        let notes = vec![
            note(
                "fresh",
                "confirm",
                "meaning re-affirmed",
                "2026-06-01T12:00:00Z",
            ),
            note(
                "rel-2",
                "transition",
                "stale (sync: src/x.rs changed)",
                "2026-06-01T11:00:00Z",
            ),
        ];
        let ranked = align_snap(vec![fresh], vec![rel], notes);
        assert!(
            ranked.is_empty(),
            "sync churn at or before confirm baseline must not count: {ranked:?}"
        );
    }
}
