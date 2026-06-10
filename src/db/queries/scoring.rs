//! Priority scoring and discovery-candidate selection for `loom next`.

use anyhow::{Context, Result};
use std::collections::HashMap;

use crate::db::LoomDb;
use crate::types::{Governs, InspectionStatus, Intent, QualityRule, RelatesTo, ValidatesEdge, Validation};

use super::governs::list_all_governs;
use super::hierarchy::list_all_hierarchy;
use super::intent::list_intents;
use super::relates_to::list_relates_to;
use super::row::{col_map, get, i64_val, str_val};
use super::rule::list_rules;
use super::validates::list_all_validates;
use super::validation::list_validations;

/// Compute RELATES_TO degree for EVERY intent in TWO queries (out + in),
/// merging the counts in Rust. Use this instead of calling `intent_degree`
/// in a loop — N intents cost 2 queries total instead of 2×N.
pub fn all_intent_degrees(db: &dyn LoomDb) -> Result<HashMap<String, i64>> {
    let mut degrees: HashMap<String, i64> = HashMap::new();

    // Out-degree: for each intent that is the source of a RELATES_TO edge.
    let out_r = db.execute(
        "MATCH (a:Intent)-[r:RELATES_TO]->(:Intent) RETURN a.id AS id, count(r) AS c",
    )?;
    let out_cols = col_map(&out_r);
    for row in out_r.rows() {
        let id = str_val(get(row, &out_cols, "id"));
        let c  = i64_val(get(row, &out_cols, "c"));
        if !id.is_empty() {
            *degrees.entry(id).or_insert(0) += c;
        }
    }

    // In-degree: for each intent that is the target of a RELATES_TO edge.
    let in_r = db.execute(
        "MATCH (:Intent)-[r:RELATES_TO]->(b:Intent) RETURN b.id AS id, count(r) AS c",
    )?;
    let in_cols = col_map(&in_r);
    for row in in_r.rows() {
        let id = str_val(get(row, &in_cols, "id"));
        let c  = i64_val(get(row, &in_cols, "c"));
        if !id.is_empty() {
            *degrees.entry(id).or_insert(0) += c;
        }
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
    let intents = list_intents(db, None, None)?;
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
    let degrees = all_intent_degrees(db)?;

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

pub fn normative_coverage(db: &dyn LoomDb) -> Result<NormativeCoverage> {
    let intents = list_intents(db, None, None)?;
    let rules = list_rules(db)?;
    let governs = list_all_governs(db)?;
    let hierarchy = list_all_hierarchy(db)?;

    let with_code: std::collections::HashSet<String> =
        super::implements::intents_with_implements(db)?;
    let candidates: Vec<&Intent> = intents
        .iter()
        .filter(|i| i.status != "deprecated" && with_code.contains(&i.id))
        .collect();

    let considered: std::collections::HashSet<(String, String)> = governs
        .iter()
        .map(|g| (g.rule_id.clone(), g.intent_id.clone()))
        .collect();
    let parent_of: HashMap<&str, &str> =
        hierarchy.iter().map(|(p, c)| (c.as_str(), p.as_str())).collect();
    // Considered directly OR via any ancestor's verdict on the same rule.
    let considered_up = |rule_id: &str, intent_id: &str| -> bool {
        let mut cur = Some(intent_id);
        let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
        while let Some(id) = cur {
            if !visited.insert(id) {
                return false;
            }
            if considered.contains(&(rule_id.to_string(), id.to_string())) {
                return true;
            }
            cur = parent_of.get(id).copied();
        }
        false
    };

    let total_pairs = rules.len() as i64 * candidates.len() as i64;
    let mut measured_pairs = 0i64;
    let mut queue: Vec<(QualityRule, Intent)> = Vec::new();
    for r in &rules {
        let unmeasured: std::collections::HashSet<&str> = candidates
            .iter()
            .filter(|i| !considered_up(&r.id, &i.id))
            .map(|i| i.id.as_str())
            .collect();
        measured_pairs += candidates.len() as i64 - unmeasured.len() as i64;
        // Queue only the TOPS of unmeasured subtrees: if any ancestor is also
        // unmeasured-with-code, a verdict there covers this one — skip it.
        for i in &candidates {
            if !unmeasured.contains(i.id.as_str()) {
                continue;
            }
            let mut cur = parent_of.get(i.id.as_str()).copied();
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
                queue.push(((*r).clone(), (*i).clone()));
            }
        }
    }
    Ok(NormativeCoverage {
        intents_with_code: candidates.len() as i64,
        total_pairs,
        measured_pairs,
        queue,
    })
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
    // Bulk-load all degrees once; both the GOVERNS loop and the normative-coverage
    // queue need degrees, so this replaces up to 2×(governs + queue) queries.
    let degrees = all_intent_degrees(db)?;

    let mut scored: Vec<(Governs, f64)> = Vec::new();
    for g in list_all_governs(db)? {
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
    let intents = list_intents(db, None, None)?;
    let is_parent: std::collections::HashSet<String> =
        list_all_hierarchy(db)?.into_iter().map(|(p, _)| p).collect();

    // Bulk load: all VALIDATES edges + all Validation nodes — avoid N per-intent queries.
    let all_edges = list_all_validates(db)?;
    let all_validations = list_validations(db)?;

    // Index: intent_id → Vec<ValidatesEdge>
    let mut edges_by_intent: HashMap<&str, Vec<&ValidatesEdge>> = HashMap::new();
    for e in &all_edges {
        edges_by_intent.entry(e.intent_id.as_str()).or_default().push(e);
    }
    // Index: validation_id → Validation
    let val_by_id: HashMap<&str, &Validation> =
        all_validations.iter().map(|v| (v.id.as_str(), v)).collect();

    let mut selected: Vec<(Intent, f64, String)> = Vec::new();
    for i in intents {
        let edges = edges_by_intent.get(i.id.as_str()).map(|v| v.as_slice()).unwrap_or(&[]);
        let (urgency, reason) = if edges.is_empty() {
            if i.lifecycle == "implemented" && !is_parent.contains(&i.id) {
                (3.0, "no proof: this implemented leaf intent has no validations".to_string())
            } else {
                continue;
            }
        } else {
            // Resolve validation objects from the pre-loaded map.
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
                // All proofs green — or `blocked`, which is deliberately NOT
                // queue work: it's a recorded "can't run yet" with a reason
                // (visible in `loom validation list` / `loom report`), and
                // surfacing it here would nag about work nobody can do.
                continue;
            }
        };
        selected.push((i, urgency, reason));
    }
    Ok(selected)
}

/// Intents with weak or absent proof, scored by centrality + urgency — the
/// worklist for `loom next --mode validate`. Selection logic lives in
/// `validate_selection` (shared with the compass).
pub fn validate_candidates(db: &dyn LoomDb) -> Result<Vec<ValidateCandidate>> {
    let selected = validate_selection(db)?;
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    // One bulk degree query instead of 2×N per-intent queries.
    let degrees = all_intent_degrees(db)?;
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

/// Count of intent pairs with no RELATES_TO edge (and no HIERARCHY link) —
/// arithmetic, not enumerative: C(n,2) minus linked unordered pairs. This is
/// what `graph_state` needs on every pulse; building the full scored O(N²)
/// list just to .len() it was an iso5055-perf-no-redundant-work violation
/// found by loom measuring itself.
pub fn count_unexplored_pairs(db: &dyn LoomDb) -> Result<i64> {
    let n = list_intents(db, None, None)?.len() as i64;
    let mut linked: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let key = |a: &str, b: &str| {
        if a < b { (a.to_string(), b.to_string()) } else { (b.to_string(), a.to_string()) }
    };
    for e in list_relates_to(db, None)? {
        linked.insert(key(&e.from_id, &e.to_id));
    }
    for (p, c) in list_all_hierarchy(db)? {
        linked.insert(key(&p, &c));
    }
    Ok((n * (n - 1) / 2 - linked.len() as i64).max(0))
}

/// Intent pairs that have NO RELATES_TO edge between them yet, returned as
/// synthetic "unexplored" candidates. Scored by combined centrality PLUS a
/// suspicion bonus — pairs that share implemented files, read alike, or live
/// in the same domain are the ones most likely to hide a real relationship
/// (or a split-brain), so the analyzer is pointed at them first instead of
/// grinding a flat N×N grid. The why travels in the synthetic edge's `notes`
/// so `loom next` can display it.
pub fn unexplored_pairs_scored(db: &dyn LoomDb) -> Result<Vec<(RelatesTo, f64)>> {
    use super::smells::{jaccard, tokens};

    let intents = list_intents(db, None, None)?;
    let edges = list_relates_to(db, None)?;

    // Suspicion inputs, computed once per intent.
    let mut files_of: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
    for im in super::implements::list_all_implements(db)? {
        files_of.entry(im.intent_id).or_default().insert(im.codefile_path);
    }
    let toks: HashMap<&str, std::collections::HashSet<String>> = intents
        .iter()
        .map(|i| (i.id.as_str(), tokens(&format!("{} {}", i.name, i.description))))
        .collect();
    // file → file static import links (extracted by `loom sync`): the physical
    // evidence that two intents' code actually touches.
    let mut import_links: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    for cf in super::codefile::list_codefiles(db)? {
        let imports: Vec<String> = serde_json::from_str(&cf.imports)
            .with_context(|| format!("Malformed imports JSON for CodeFile '{}'", cf.path))?;
        for t in imports {
            import_links.insert((cf.path.clone(), t.clone()));
            import_links.insert((t, cf.path.clone()));
        }
    }

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

    // Bulk-load all degrees once — N intents costs 2 queries, not 2×N.
    let degrees = all_intent_degrees(db)?;

    let base_urgency = InspectionStatus::Uninspected.urgency();
    let empty_files = std::collections::HashSet::new();
    let mut scored: Vec<(RelatesTo, f64)> = Vec::new();
    for i in 0..intents.len() {
        for j in (i + 1)..intents.len() {
            let a = &intents[i];
            let b = &intents[j];
            if linked.contains(&(a.id.clone(), b.id.clone())) {
                continue;
            }

            // Suspicion bonus + human-readable why.
            let fa = files_of.get(&a.id).unwrap_or(&empty_files);
            let fb = files_of.get(&b.id).unwrap_or(&empty_files);
            let shared = fa.intersection(fb).count();
            let sim = jaccard(&toks[a.id.as_str()], &toks[b.id.as_str()]);
            let same_domain = !a.domain.is_empty() && a.domain == b.domain && a.domain != "unknown";
            let imports = fa
                .iter()
                .flat_map(|x| fb.iter().map(move |y| (x.clone(), y.clone())))
                .filter(|p| import_links.contains(p))
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

            let score = *degrees.get(&a.id).unwrap_or(&0) as f64
                + *degrees.get(&b.id).unwrap_or(&0) as f64
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
