//! Priority scoring and discovery-candidate selection for `loom next`.

use anyhow::Result;
use std::collections::HashMap;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::{Governs, InspectionStatus, Intent, RelatesTo};

use super::governs::list_all_governs;
use super::hierarchy::list_all_hierarchy;
use super::intent::list_intents;
use super::relates_to::list_relates_to;
use super::row::{col_map, get, i64_val, str_val};
use super::validates::{list_validates_for_intent, validations_for_intent};

/// Degree of an intent = total RELATES_TO edges touching it (in + out).
pub fn intent_degree(db: &dyn LoomDb, intent_id: &str) -> Result<i64> {
    let out_q = format!(
        "MATCH (n:Intent {{id: '{}'}})-[r:RELATES_TO]->() RETURN count(r) AS c",
        esc(intent_id)
    );
    let in_q = format!(
        "MATCH ()-[r:RELATES_TO]->(n:Intent {{id: '{}'}}) RETURN count(r) AS c",
        esc(intent_id)
    );
    let out_res = db.execute(&out_q)?;
    let in_res  = db.execute(&in_q)?;
    let out_deg = out_res.rows().first().map(|r| i64_val(&r[0])).unwrap_or(0);
    let in_deg  = in_res.rows().first().map(|r| i64_val(&r[0])).unwrap_or(0);
    Ok(out_deg + in_deg)
}

/// Compute priority scores for all candidate RELATES_TO edges and return sorted.
/// Formula: degree(a) + degree(b) + urgency(status) - age_penalty(last_inspected)
/// mode: "discovery" (uninspected) | "fix" (failing + needs_reverification)
pub fn scored_candidates(
    db: &dyn LoomDb,
    mode: &str,
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

    let mut scored: Vec<(RelatesTo, f64)> = Vec::new();
    let now = chrono::Utc::now();
    let mut degree_cache: HashMap<String, i64> = HashMap::new();

    for edge in candidates {
        let deg_a = if let Some(&d) = degree_cache.get(&edge.from_id) {
            d
        } else {
            let d = intent_degree(db, &edge.from_id)?;
            degree_cache.insert(edge.from_id.clone(), d);
            d
        };
        let deg_b = if let Some(&d) = degree_cache.get(&edge.to_id) {
            d
        } else {
            let d = intent_degree(db, &edge.to_id)?;
            degree_cache.insert(edge.to_id.clone(), d);
            d
        };

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

/// Intents that need building or changing (lifecycle `planned` | `needs_change`),
/// scored by centrality + urgency — the worklist for `loom next --mode build`.
/// `needs_change` (a known issue / refactor) outranks `planned` (greenfield).
pub fn build_candidates(db: &dyn LoomDb) -> Result<Vec<(Intent, f64)>> {
    let mut scored: Vec<(Intent, f64)> = Vec::new();
    for i in list_intents(db, None, None)? {
        let urgency = match i.lifecycle.as_str() {
            "needs_change" => 4.0,
            "planned" => 2.0,
            _ => continue,
        };
        let score = intent_degree(db, &i.id)? as f64 + urgency;
        scored.push((i, score));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored)
}

/// GOVERNS edges needing the quality agent's attention — uninspected (applied
/// but compliance never earned), failing (violation open), or stale. The
/// worklist for `loom next --mode quality`, scored by intent centrality +
/// urgency so high-blast-radius violations surface first.
pub fn quality_candidates(db: &dyn LoomDb) -> Result<Vec<(Governs, f64)>> {
    let mut degree_cache: HashMap<String, i64> = HashMap::new();
    let mut scored: Vec<(Governs, f64)> = Vec::new();
    for g in list_all_governs(db)? {
        let urgency = match g.inspection_status.as_str() {
            "failing" => 4.0,
            "needs_reverification" => 3.0,
            "uninspected" => 2.0,
            _ => continue,
        };
        let deg = if let Some(&d) = degree_cache.get(&g.intent_id) {
            d
        } else {
            let d = intent_degree(db, &g.intent_id)?;
            degree_cache.insert(g.intent_id.clone(), d);
            d
        };
        scored.push((g, deg as f64 + urgency));
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

/// Intents with weak or absent proof — the worklist for
/// `loom next --mode validate`:
/// - a linked validation failed (urgency 4)
/// - an implemented LEAF intent has no VALIDATES edge at all (urgency 3 —
///   "intents without validations are risky"; non-leaves are proven by their
///   children, planned intents have nothing to prove yet)
/// - linked validations exist but were never run / were invalidated by sync
///   (urgency 2)
pub fn validate_candidates(db: &dyn LoomDb) -> Result<Vec<ValidateCandidate>> {
    let intents = list_intents(db, None, None)?;
    let is_parent: std::collections::HashSet<String> =
        list_all_hierarchy(db)?.into_iter().map(|(p, _)| p).collect();

    let mut scored: Vec<ValidateCandidate> = Vec::new();
    for i in intents {
        let edges = list_validates_for_intent(db, &i.id)?;
        let (urgency, reason) = if edges.is_empty() {
            if i.lifecycle == "implemented" && !is_parent.contains(&i.id) {
                (3.0, "no proof: this implemented leaf intent has no validations".to_string())
            } else {
                continue;
            }
        } else {
            let validations = validations_for_intent(db, &i.id)?;
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
                continue; // all proofs green
            }
        };
        let score = intent_degree(db, &i.id)? as f64 + urgency;
        scored.push(ValidateCandidate { intent: i, score, reason });
    }
    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored)
}

/// Intent pairs that have NO RELATES_TO edge between them yet, returned as
/// synthetic "unexplored" candidates scored by combined centrality. This lets
/// `loom next --mode discovery` keep driving exploration of the N×N intent grid
/// once every materialised edge has been inspected — the agent never has to
/// decide manually which pair to look at next.
pub fn unexplored_pairs_scored(db: &dyn LoomDb) -> Result<Vec<(RelatesTo, f64)>> {
    let intents = list_intents(db, None, None)?;
    let edges = list_relates_to(db, None)?;

    // Mark every ordered direction that already has an edge, so an existing
    // a→b edge also suppresses the b→a pair (RELATES_TO is conceptually
    // symmetric for discovery purposes).
    let mut linked: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    for e in &edges {
        linked.insert((e.from_id.clone(), e.to_id.clone()));
        linked.insert((e.to_id.clone(), e.from_id.clone()));
    }
    // Pairs already connected by HIERARCHY are related by containment — don't
    // surface them as "unexplored" RELATES_TO (that's redundant noise).
    let hier = db.execute("MATCH (a:Intent)-[e:HIERARCHY]->(b:Intent) RETURN a.id AS p, b.id AS c")?;
    let hcols = col_map(&hier);
    for row in hier.rows() {
        let p = str_val(get(row, &hcols, "p"));
        let c = str_val(get(row, &hcols, "c"));
        linked.insert((p.clone(), c.clone()));
        linked.insert((c, p));
    }

    let mut degree_cache: HashMap<String, i64> = HashMap::new();
    let mut degree_of = |db: &dyn LoomDb, id: &str| -> Result<i64> {
        if let Some(&d) = degree_cache.get(id) {
            return Ok(d);
        }
        let d = intent_degree(db, id)?;
        degree_cache.insert(id.to_string(), d);
        Ok(d)
    };

    let base_urgency = InspectionStatus::Uninspected.urgency();
    let mut scored: Vec<(RelatesTo, f64)> = Vec::new();
    for i in 0..intents.len() {
        for j in (i + 1)..intents.len() {
            let a = &intents[i];
            let b = &intents[j];
            if linked.contains(&(a.id.clone(), b.id.clone())) {
                continue;
            }
            let score = degree_of(db, &a.id)? as f64 + degree_of(db, &b.id)? as f64 + base_urgency;
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
                    notes:             String::new(),
                },
                score,
            ));
        }
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored)
}
