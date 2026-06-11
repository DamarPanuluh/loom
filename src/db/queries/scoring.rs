//! Priority scoring and discovery-candidate selection for `loom next`.

use anyhow::Result;
use std::collections::HashMap;

use crate::db::LoomDb;
use crate::types::{Governs, InspectionStatus, Intent, QualityRule, RelatesTo, ValidatesEdge, Validation};

use super::governs::list_all_governs;
use super::hierarchy::list_all_hierarchy;
use super::intent::list_active_intents;
use super::relates_to::list_relates_to;
use super::row::{col_map, get, str_val};
use super::snapshot::{DiscoverySnapshot, QuerySnapshot};
/// Compute RELATES_TO degree (centrality) for EVERY intent in ONE edge scan
/// plus one node scan, merged in Rust. Two deliberate exclusions:
/// - `independent` edges: a VERIFIED ABSENCE of relationship gives closure to
///   the grid but contributes NOTHING to blast radius — counting it made
///   well-scouted intents look like hubs.
/// - edges touching `deprecated` intents: retired design is invisible to
///   computation (the retirement contract).
/// Status is read as a RETURNED column and filtered in Rust — matching an
/// edge by its own property in a WHERE is the known-unreliable grafeo path.
pub fn all_intent_degrees(db: &dyn LoomDb) -> Result<HashMap<String, i64>> {
    let active: std::collections::HashSet<String> = super::intent::list_active_intents(db)?
        .into_iter()
        .map(|i| i.id)
        .collect();
    let mut degrees: HashMap<String, i64> = HashMap::new();
    let r = db.execute(
        "MATCH (a:Intent)-[r:RELATES_TO]->(b:Intent) \
         RETURN a.id AS f, b.id AS t, r.inspection_status AS s",
    )?;
    let cols = col_map(&r);
    for row in r.rows() {
        let s = str_val(get(row, &cols, "s"));
        if s == "independent" {
            continue;
        }
        let f = str_val(get(row, &cols, "f"));
        let t = str_val(get(row, &cols, "t"));
        if !active.contains(&f) || !active.contains(&t) {
            continue;
        }
        *degrees.entry(f).or_insert(0) += 1;
        *degrees.entry(t).or_insert(0) += 1;
    }
    Ok(degrees)
}

/// Compute priority scores for all candidate RELATES_TO edges and return sorted.
/// Formula: degree(a) + degree(b) + urgency(status) - age_penalty(last_inspected)
/// mode: "discovery" (uninspected) | "fix" (failing + needs_reverification)
///
/// `degrees` is an optional pre-built degree map (from `all_intent_degrees`); pass
/// `None` and it is built here. Callers that already have the map (e.g. `run_all`)
/// should pass it in to avoid a redundant pair of queries.
pub fn scored_candidates(
    db: &dyn LoomDb,
    mode: &str,
) -> Result<Vec<(RelatesTo, f64)>> {
    scored_candidates_with_degrees(db, mode, None)
}

pub fn scored_candidates_with_degrees(
    db: &dyn LoomDb,
    mode: &str,
    prebuilt_degrees: Option<&HashMap<String, i64>>,
) -> Result<Vec<(RelatesTo, f64)>> {
    let mut candidates = match mode {
        "fix" => {
            let mut v = list_relates_to(db, Some("failing"))?;
            v.extend(list_relates_to(db, Some("needs_reverification"))?);
            v
        }
        _ => list_relates_to(db, Some("uninspected"))?,
    };

    // Remove duplicates (in case of overlap)
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|e| seen.insert(e.id.clone()));

    // Retired endpoints take their edges out of every queue (the retirement
    // contract: invisible to computation, visible to history).
    if !candidates.is_empty() {
        let active: std::collections::HashSet<String> =
            list_active_intents(db)?.into_iter().map(|i| i.id).collect();
        candidates.retain(|e| active.contains(&e.from_id) && active.contains(&e.to_id));
    }

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Build degrees once for all candidates rather than 2 queries per unique endpoint.
    let owned;
    let degrees: &HashMap<String, i64> = if let Some(d) = prebuilt_degrees {
        d
    } else {
        owned = all_intent_degrees(db)?;
        &owned
    };

    let mut scored: Vec<(RelatesTo, f64)> = Vec::new();
    let now = chrono::Utc::now();

    for edge in candidates {
        let deg_a = *degrees.get(&edge.from_id).unwrap_or(&0);
        let deg_b = *degrees.get(&edge.to_id).unwrap_or(&0);

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

        let score = deg_a as f64 + deg_b as f64 + urgency - age_penalty;
        scored.push((edge, score));
    }

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored)
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
            "fix" => matches!(edge.inspection_status.as_str(), "failing" | "needs_reverification"),
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
        let score = deg_a as f64 + deg_b as f64 + urgency - age_penalty;
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

/// Intents that need building or changing (lifecycle `planned` | `needs_change`),
/// scored by centrality + urgency — the worklist for `loom next --mode build`.
/// `needs_change` (a known issue / refactor) outranks `planned` (greenfield).
///
/// Altitude rule: a *planned parent* is never "built" directly — its children
/// are. While any child is still planned/needs_change the parent is deferred
/// (the children are in the queue); once all children are implemented the
/// parent surfaces as a roll-up. `needs_change` intents always surface
/// (component-level refactors are legitimate work at any altitude).
pub fn build_candidates(db: &dyn LoomDb) -> Result<Vec<BuildCandidate>> {
    build_candidates_with_degrees(db, None)
}

pub fn build_candidates_with_degrees(
    db: &dyn LoomDb,
    prebuilt_degrees: Option<&HashMap<String, i64>>,
) -> Result<Vec<BuildCandidate>> {
    let intents = list_active_intents(db)?;
    let lifecycle_of: HashMap<&str, &str> = intents
        .iter()
        .map(|i| (i.id.as_str(), i.lifecycle.as_str()))
        .collect();
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for (p, c) in list_all_hierarchy(db)? {
        children.entry(p).or_default().push(c);
    }

    // Collect the build candidates BEFORE degree lookup so we only bulk-load
    // degrees when there is actually work to score.
    let mut pending: Vec<(&Intent, f64, bool)> = Vec::new();
    for i in &intents {
        let urgency = match i.lifecycle.as_str() {
            "needs_change" => 4.0,
            "planned" => 2.0,
            _ => continue,
        };
        let kids = children.get(&i.id);
        let mut rollup = false;
        if i.lifecycle == "planned" {
            if let Some(kids) = kids {
                let p = kids.iter().any(|c| {
                    matches!(lifecycle_of.get(c.as_str()), Some(&"planned") | Some(&"needs_change"))
                });
                if p {
                    continue;
                }
                rollup = true;
            }
        }
        pending.push((i, urgency, rollup));
    }

    if pending.is_empty() {
        return Ok(Vec::new());
    }

    // One bulk degree query instead of 2×N per-intent queries.
    let owned;
    let degrees = if let Some(d) = prebuilt_degrees {
        d
    } else {
        owned = all_intent_degrees(db)?;
        &owned
    };

    let mut scored: Vec<BuildCandidate> = pending
        .into_iter()
        .map(|(i, urgency, rollup)| {
            let deg = *degrees.get(&i.id).unwrap_or(&0) as f64;
            BuildCandidate { intent: i.clone(), score: deg + urgency, rollup }
        })
        .collect();
    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored)
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
        let urgency = match intent.lifecycle.as_str() {
            "needs_change" => 4.0,
            "planned" => 2.0,
            _ => continue,
        };
        let kids = children.get(&intent.id);
        let mut rollup = false;
        if intent.lifecycle == "planned" {
            if let Some(kids) = kids {
                let pending_child = kids.iter().any(|c| {
                    matches!(lifecycle_of.get(c.as_str()), Some(&"planned") | Some(&"needs_change"))
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
    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
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

/// Verdicts recorded with confidence below this surface in the review queue —
/// the strategic double-check loop for tiered agents: a low-capability scout
/// records HONEST confidence, and the graph itself routes the uncertain claims
/// to a stronger reviewer. Re-recording with confidence at/above the threshold
/// (or overturning the verdict) resolves the item.
pub const REVIEW_CONFIDENCE: f64 = 0.7;

/// One uncertain claim for the reviewer: a recorded verdict (RELATES_TO or
/// GOVERNS) whose confidence is below `REVIEW_CONFIDENCE`, scored by
/// (1 − confidence) × combined centrality — an uncertain claim about a hub
/// outranks an uncertain claim about a leaf pair. Cheap certainty on leaves is
/// deliberately left alone: double-check strategically, not exhaustively.
#[derive(Debug, Clone)]
pub enum ReviewCandidate {
    RelatesTo(RelatesTo),
    Governs(Governs),
}

pub fn review_candidates(db: &dyn LoomDb) -> Result<Vec<(ReviewCandidate, f64)>> {
    review_candidates_with_degrees(db, None)
}

pub fn review_candidates_with_degrees(
    db: &dyn LoomDb,
    prebuilt_degrees: Option<&HashMap<String, i64>>,
) -> Result<Vec<(ReviewCandidate, f64)>> {
    let active: std::collections::HashSet<String> =
        list_active_intents(db)?.into_iter().map(|i| i.id).collect();
    let owned;
    let degrees = if let Some(d) = prebuilt_degrees {
        d
    } else {
        owned = all_intent_degrees(db)?;
        &owned
    };
    let needs_review = |status: &str, confidence: f64| {
        matches!(status, "passing" | "failing" | "independent")
            && confidence > 0.0
            && confidence < REVIEW_CONFIDENCE
    };

    let mut scored: Vec<(ReviewCandidate, f64)> = Vec::new();
    for e in list_relates_to(db, None)? {
        if !needs_review(&e.inspection_status, e.confidence)
            || !active.contains(&e.from_id)
            || !active.contains(&e.to_id)
        {
            continue;
        }
        let deg = (*degrees.get(&e.from_id).unwrap_or(&0)
            + *degrees.get(&e.to_id).unwrap_or(&0)) as f64;
        let score = (1.0 - e.confidence) * (deg + 1.0);
        scored.push((ReviewCandidate::RelatesTo(e), score));
    }
    for g in list_all_governs(db)? {
        if !needs_review(&g.inspection_status, g.confidence) || !active.contains(&g.intent_id) {
            continue;
        }
        let deg = *degrees.get(&g.intent_id).unwrap_or(&0) as f64;
        let score = (1.0 - g.confidence) * (deg + 1.0);
        scored.push((ReviewCandidate::Governs(g), score));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored)
}

pub fn normative_coverage(db: &dyn LoomDb) -> Result<NormativeCoverage> {
    let snapshot = QuerySnapshot::load(db)?;
    Ok(normative_coverage_from_snapshot(&snapshot))
}

pub fn review_candidates_from_snapshot(snapshot: &QuerySnapshot) -> Vec<(ReviewCandidate, f64)> {
    let active: std::collections::HashSet<&str> =
        snapshot.intents.iter().map(|i| i.id.as_str()).collect();
    let needs_review = |status: &str, confidence: f64| {
        matches!(status, "passing" | "failing" | "independent")
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

/// GOVERNS edges needing the quality agent's attention — uninspected (applied
/// but compliance never earned), failing (violation open), or stale — PLUS
/// synthetic `unmeasured` items: rule × intent-with-code pairs no one ever
/// considered (no GOVERNS edge here or on any ancestor). The worklist for
/// `loom next --mode quality`, scored by intent centrality + urgency so
/// high-blast-radius violations surface first; unmeasured pairs rank below
/// every real edge (urgency 1.0) and resolve in ONE command — `loom rule
/// verdict` creates the edge with the verdict (independent = measured, doesn't
/// apply; a verdict at component altitude covers descendants).
pub fn quality_candidates(db: &dyn LoomDb) -> Result<Vec<(Governs, f64)>> {
    quality_candidates_with_degrees(db, None)
}

pub fn quality_candidates_with_degrees(
    db: &dyn LoomDb,
    prebuilt_degrees: Option<&HashMap<String, i64>>,
) -> Result<Vec<(Governs, f64)>> {
    // Bulk-load all degrees once; both the GOVERNS loop and the normative-coverage
    // queue need degrees, so this replaces up to 2×(governs + queue) queries.
    let owned;
    let degrees = if let Some(d) = prebuilt_degrees {
        d
    } else {
        owned = all_intent_degrees(db)?;
        &owned
    };
    let active: std::collections::HashSet<String> =
        list_active_intents(db)?.into_iter().map(|i| i.id).collect();

    let mut scored: Vec<(Governs, f64)> = Vec::new();
    for g in list_all_governs(db)? {
        // Rules over retired intents are history, not open quality work.
        if !active.contains(&g.intent_id) {
            continue;
        }
        let urgency = match g.inspection_status.as_str() {
            "failing" => 4.0,
            "needs_reverification" => 3.0,
            "uninspected" => 2.0,
            _ => continue,
        };
        let deg = *degrees.get(&g.intent_id).unwrap_or(&0);
        scored.push((g, deg as f64 + urgency));
    }
    for (rule, intent) in normative_coverage(db)?.queue {
        let deg = *degrees.get(&intent.id).unwrap_or(&0);
        scored.push((
            Governs {
                id: String::new(), // no edge yet — `loom rule verdict` creates it
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
    Ok(scored)
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

/// The validator's SELECTION — which intents need attention, with urgency and
/// why, before any centrality scoring:
/// - a linked validation failed (urgency 4)
/// - an implemented LEAF intent has no VALIDATES edge at all (urgency 3 —
///   "intents without validations are risky"; non-leaves are proven by their
///   children, planned intents have nothing to prove yet)
/// - linked validations exist but were never run / were invalidated by sync
///   (urgency 2)
///
/// COHERENCE BY CONSTRUCTION: this one function is consumed verbatim by BOTH
/// the queue (`validate_candidates` adds degree scoring) and the compass
/// (`graph_state` routes phase=validate on its emptiness) — the two can never
/// disagree the way edge-state counts and last_result-based selection once
/// did (phase=validate with an empty validator queue).
pub fn validate_selection(db: &dyn LoomDb) -> Result<Vec<(Intent, f64, String)>> {
    let snapshot = QuerySnapshot::load(db)?;
    Ok(validate_selection_from_snapshot(&snapshot))
}

pub fn validate_selection_from_snapshot(snapshot: &QuerySnapshot) -> Vec<(Intent, f64, String)> {
    let is_parent: std::collections::HashSet<String> =
        snapshot.hierarchy.iter().map(|(p, _)| p.clone()).collect();

    let mut edges_by_intent: HashMap<&str, Vec<&ValidatesEdge>> = HashMap::new();
    for edge in &snapshot.validates {
        edges_by_intent.entry(edge.intent_id.as_str()).or_default().push(edge);
    }
    let val_by_id: HashMap<&str, &Validation> =
        snapshot.validations.iter().map(|v| (v.id.as_str(), v)).collect();

    let mut selected: Vec<(Intent, f64, String)> = Vec::new();
    for intent in &snapshot.intents {
        let edges = edges_by_intent
            .get(intent.id.as_str())
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let (urgency, reason) = if edges.is_empty() {
            if intent.lifecycle == "implemented" && !is_parent.contains(&intent.id) {
                (3.0, "no proof: this implemented leaf intent has no validations".to_string())
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
            } else if validations
                .iter()
                .any(|v| v.last_result == "not_run" || v.last_result.is_empty())
            {
                (2.0, "linked validations have not been run (or were invalidated by a code change)".to_string())
            } else {
                continue;
            }
        };
        selected.push((intent.clone(), urgency, reason));
    }
    selected
}

/// Intents with weak or absent proof, scored by centrality + urgency — the
/// worklist for `loom next --mode validate`. Selection logic lives in
/// `validate_selection` (shared with the compass).
pub fn validate_candidates(db: &dyn LoomDb) -> Result<Vec<ValidateCandidate>> {
    validate_candidates_with_degrees(db, None)
}

pub fn validate_candidates_with_degrees(
    db: &dyn LoomDb,
    prebuilt_degrees: Option<&HashMap<String, i64>>,
) -> Result<Vec<ValidateCandidate>> {
    let selected = validate_selection(db)?;
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    // One bulk degree query instead of 2×N per-intent queries.
    let owned;
    let degrees = if let Some(d) = prebuilt_degrees {
        d
    } else {
        owned = all_intent_degrees(db)?;
        &owned
    };
    let mut scored: Vec<ValidateCandidate> = selected
        .into_iter()
        .map(|(intent, urgency, reason)| {
            let deg = *degrees.get(&intent.id).unwrap_or(&0) as f64;
            ValidateCandidate { score: deg + urgency, intent, reason }
        })
        .collect();
    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored)
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
    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

/// Count of intent pairs with no RELATES_TO edge (and no HIERARCHY link) —
/// arithmetic, not enumerative: C(n,2) minus linked unordered pairs. This is
/// what `graph_state` needs on every pulse; building the full scored O(N²)
/// list just to .len() it was an iso5055-perf-no-redundant-work violation
/// found by loom measuring itself.
#[cfg(test)]
pub fn count_unexplored_pairs(db: &dyn LoomDb) -> Result<i64> {
    let intents = list_active_intents(db)?;
    let relates = list_relates_to(db, None)?;
    let hierarchy = list_all_hierarchy(db)?;
    Ok(count_unexplored_pairs_from(intents.len() as i64, &relates, &hierarchy))
}

pub fn count_unexplored_pairs_from(
    intent_count: i64,
    relates: &[RelatesTo],
    hierarchy: &[(String, String)],
) -> i64 {
    let mut linked: std::collections::HashSet<(&str, &str)> = std::collections::HashSet::new();
    fn key<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
        if a < b { (a, b) } else { (b, a) }
    }
    for e in relates {
        linked.insert(key(&e.from_id, &e.to_id));
    }
    for (p, c) in hierarchy {
        linked.insert(key(p, c));
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
pub fn unexplored_pairs_scored(db: &dyn LoomDb) -> Result<Vec<(RelatesTo, f64)>> {
    use super::smells::jaccard;

    let snapshot = QuerySnapshot::load(db)?;
    let discovery = DiscoverySnapshot::from_query(&snapshot)?;
    let base_urgency = InspectionStatus::Uninspected.urgency();
    let empty_files = std::collections::HashSet::new();
    let mut scored: Vec<(RelatesTo, f64)> = Vec::new();

    for i in 0..snapshot.intents.len() {
        for j in (i + 1)..snapshot.intents.len() {
            let a = &snapshot.intents[i];
            let b = &snapshot.intents[j];
            if discovery.linked.contains(&(a.id.clone(), b.id.clone())) {
                continue;
            }

            let fa = discovery.files_of.get(&a.id).unwrap_or(&empty_files);
            let fb = discovery.files_of.get(&b.id).unwrap_or(&empty_files);
            let shared = fa.intersection(fb).count();
            let sim = jaccard(&discovery.tokens_by_intent[&a.id], &discovery.tokens_by_intent[&b.id]);
            let same_domain = !a.domain.is_empty() && a.domain == b.domain && a.domain != "unknown";
            let imports = fa
                .iter()
                .flat_map(|x| fb.iter().map(move |y| (*x, *y)))
                .filter(|p| discovery.import_links.contains(p))
                .count();
            let mut why: Vec<String> = Vec::new();
            if imports > 0 {
                why.push(format!("their code imports each other ({imports} link(s))"));
            }
            if shared > 0 {
                why.push(format!("share {shared} implemented file(s)"));
            }
            if sim >= 0.25 {
                why.push(format!("descriptions overlap ({sim:.2})"));
            }
            if same_domain {
                why.push(format!("same domain '{}'", a.domain));
            }
            let suspicion = 5.0 * imports as f64
                + 3.0 * shared as f64
                + 4.0 * sim
                + if same_domain { 1.0 } else { 0.0 };

            let score = *snapshot.degrees.get(&a.id).unwrap_or(&0) as f64
                + *snapshot.degrees.get(&b.id).unwrap_or(&0) as f64
                + base_urgency
                + suspicion;
            scored.push((
                RelatesTo {
                    id:                String::new(),
                    from_id:           a.id.clone(),
                    to_id:             b.id.clone(),
                    from_name:         a.name.clone(),
                    to_name:           b.name.clone(),
                    inspection_status: "unexplored".to_string(),
                    criterion:         String::new(),
                    confidence:        0.0,
                    evidence:          String::new(),
                    last_inspected:    String::new(),
                    inspected_by:      String::new(),
                    priority_score:    score,
                    notes:             if why.is_empty() {
                        String::new()
                    } else {
                        format!("suspicion: {}", why.join("; "))
                    },
                },
                score,
            ));
        }
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored)
}
