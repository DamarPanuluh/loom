//! Count / coverage / centrality statistics for `loom status` and `loom report`.

use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

use crate::db::LoomDb;
use crate::types::{Intent, IntentCentrality};

use super::completeness::vertical_completeness;
use super::hierarchy::list_hierarchy_for_intent;
use super::implements::list_implements_for_intent;
use super::intent::{intents_without_validations, list_intents};
use super::meta::get_meta;
use super::relates_to::list_relates_to;
use super::row::{col_map, get, i64_val, str_val};
use super::scoring::{intent_degree, unexplored_pairs_scored};

/// Completeness gaps — "what's missing," not "what's present". Flags ungrounded
/// confirmed intents, intents with no validation, and feature groups that have a
/// happy path but no sad/fallback sibling (path-coverage via the `aspect` tag).
pub fn completeness_gaps(db: &dyn LoomDb) -> Result<Vec<String>> {
    let mut gaps = Vec::new();
    let intents = list_intents(db, None, None)?;
    let aspect_by_id: HashMap<String, String> =
        intents.iter().map(|i| (i.id.clone(), i.aspect.clone())).collect();

    // Confirmed intents not grounded to any code.
    for i in &intents {
        if i.status == "confirmed" && list_implements_for_intent(db, &i.id)?.is_empty() {
            gaps.push(format!(
                "Intent '{}' is confirmed but not grounded to code (no IMPLEMENTS edge).",
                i.name
            ));
        }
    }

    // Intents with no validation (no proof of fulfilment).
    for i in intents_without_validations(db)? {
        gaps.push(format!("Intent '{}' has no validation (no proof it's fulfilled).", i.name));
    }

    // Path coverage: a parent whose children include a happy path but no
    // sad/fallback sibling.
    for parent in &intents {
        let mut child_aspects: std::collections::HashSet<String> = std::collections::HashSet::new();
        for h in list_hierarchy_for_intent(db, &parent.id)? {
            if h.parent_id == parent.id {
                if let Some(a) = aspect_by_id.get(&h.child_id) {
                    if !a.is_empty() {
                        child_aspects.insert(a.clone());
                    }
                }
            }
        }
        if child_aspects.contains("happy") {
            let mut missing = Vec::new();
            if !child_aspects.contains("sad") {
                missing.push("sad");
            }
            if !child_aspects.contains("fallback") {
                missing.push("fallback");
            }
            if !missing.is_empty() {
                gaps.push(format!(
                    "Feature group under '{}' has a happy path but no {} sibling.",
                    parent.name,
                    missing.join("/")
                ));
            }
        }
    }
    Ok(gaps)
}

/// Compact "pulse" of the graph — cheap situational awareness + a recommended
/// next action (the compass). Returned in `--json` and rendered as a one-line
/// footer by the orientation commands.
#[derive(Debug, Clone, Serialize)]
pub struct GraphState {
    pub version: String,
    pub intents: i64,
    pub relates_to_edges: i64,
    pub implements_edges: i64,
    pub total_edges: i64,
    pub unresolved_edges: i64,
    /// Intent pairs with no RELATES_TO edge yet (the remaining discovery backlog).
    pub unexplored_pairs: i64,
    pub codefiles: i64,
    pub validations: i64,
    pub notes: i64,
    /// RFC3339 of last `loom sync`, or "" if never synced.
    pub last_synced: String,
    /// The binding axis: HIERARCHY is a well-formed tree, every implemented leaf
    /// is realized in code, every CodeFile is reached. `complete` requires this.
    pub vertically_complete: bool,
    /// The optional axis: every intent pair has an inspected RELATES_TO edge
    /// (none uninspected, stale, or unexplored). Reported, never gates `complete`.
    pub horizontally_explored: bool,
    /// empty | build | fix | incomplete | ground | validate | quality | discovery | complete
    pub phase: String,
    pub next_action: String,
}

fn count_edges_of_type(db: &dyn LoomDb, etype: &str) -> Result<i64> {
    let r = db.execute(&format!("MATCH ()-[r:{etype}]->() RETURN count(r) AS c"))?;
    Ok(r.rows().first().map(|row| i64_val(&row[0])).unwrap_or(0))
}

/// Status histogram for one edge type (group-by, reliable — no per-property filter).
fn edge_status_counts(db: &dyn LoomDb, etype: &str) -> Result<HashMap<String, i64>> {
    let r = db.execute(&format!(
        "MATCH ()-[r:{etype}]->() RETURN r.inspection_status AS s, count(r) AS c"
    ))?;
    let cols = col_map(&r);
    let mut m = HashMap::new();
    for row in r.rows() {
        let s = str_val(get(row, &cols, "s"));
        if !s.is_empty() {
            m.insert(s, i64_val(get(row, &cols, "c")));
        }
    }
    Ok(m)
}

/// Compute the graph pulse + phase + recommended next action.
pub fn graph_state(db: &dyn LoomDb) -> Result<GraphState> {
    let intents = count_intents(db)?;
    let codefiles = count_codefiles(db)?;
    let validations = count_validations(db)?;
    let notes = db
        .execute("MATCH (n:Note) RETURN count(n) AS c")?
        .rows()
        .first()
        .map(|row| i64_val(&row[0]))
        .unwrap_or(0);

    let by_status = count_all_edges_by_inspection_status(db)?;
    let total_edges: i64 = by_status.values().sum();

    // The discovery/fix loop only actions RELATES_TO, so the phase + the
    // "unresolved" backlog are computed from RELATES_TO specifically. IMPLEMENTS/
    // GOVERNS/HIERARCHY are structural (default passing); VALIDATES completeness
    // is surfaced by `loom report`, not the compass. (Counting all edge types
    // here would tell the user to run `loom next` for work it can't action.)
    let rt_uninspected = list_relates_to(db, Some("uninspected"))?.len() as i64;
    let rt_failing = list_relates_to(db, Some("failing"))?.len() as i64;
    let rt_needs_rev = list_relates_to(db, Some("needs_reverification"))?.len() as i64;

    // VALIDATES has its own loop (`loom validate`): uninspected = not yet run,
    // failing = the proof failed. Both are actionable, so they count toward the
    // backlog and the compass routes to them — otherwise an unrun validation
    // would show as "uninspected: N" in status while the compass said complete.
    let v = edge_status_counts(db, "VALIDATES")?;
    let v_uninspected = *v.get("uninspected").unwrap_or(&0);
    let v_failing = *v.get("failing").unwrap_or(&0);

    // GOVERNS is the green gate: an uninspected gate is an unchecked quality
    // claim; failing is a violation. Both are quality work — surfaced by
    // `loom next --mode quality`, resolved with `loom rule verdict`.
    let g = edge_status_counts(db, "GOVERNS")?;
    let g_uninspected = *g.get("uninspected").unwrap_or(&0);
    let g_failing = *g.get("failing").unwrap_or(&0);

    let unresolved_edges =
        rt_uninspected + rt_failing + rt_needs_rev + v_uninspected + v_failing + g_uninspected + g_failing;

    let relates_to_edges = count_edges_of_type(db, "RELATES_TO")?;
    let implements_edges = count_edges_of_type(db, "IMPLEMENTS")?;

    // Use the SAME computation `loom next` uses for discovery candidates, so the
    // compass can never disagree with what `loom next` actually surfaces (e.g.
    // hierarchy-linked pairs are excluded). Authoritative, not a heuristic.
    let unexplored_pairs = unexplored_pairs_scored(db)?.len() as i64;

    // Lifecycle backlog (prescriptive axis): intents that need building/changing.
    let all_intents = list_intents(db, None, None)?;
    let needs_change = all_intents.iter().filter(|i| i.lifecycle == "needs_change").count() as i64;
    let planned = all_intents.iter().filter(|i| i.lifecycle == "planned").count() as i64;

    // The two completeness axes. Vertical (binding) is the spine; horizontal
    // (optional) is the N×N grid. The compass routes vertical gaps ahead of
    // optional discovery, and only calls the graph "complete" when both hold.
    let vc = vertical_completeness(db)?;
    let horizontally_explored = unexplored_pairs == 0 && rt_uninspected == 0 && rt_needs_rev == 0;

    let (phase, next_action) = if intents == 0 {
        ("empty", "Seed intents: `loom intent add --level system …`  (or run `loom guide`).".to_string())
    } else if needs_change > 0 {
        ("build", format!("{needs_change} intent(s) need changes (known issues/refactor): `loom next --mode build`."))
    } else if rt_failing > 0 || rt_needs_rev > 0 {
        ("fix", "Resolve failures / re-verify stale edges: `loom next --mode fix`.".to_string())
    } else if planned > 0 {
        ("build", format!("{planned} planned intent(s) to build: `loom next --mode build`."))
    } else if !vc.multi_parent.is_empty() || vc.cycle {
        ("incomplete", "HIERARCHY isn't a tree (an intent has >1 parent, or there's a cycle): run `loom doctor`, then fix the edges.".to_string())
    } else if !vc.unrealized_leaves.is_empty() {
        ("ground", format!(
            "{} leaf intent(s) implemented but not grounded — `loom edge implement` them, or decompose with `loom edge hierarchy` (see `loom report`).",
            vc.unrealized_leaves.len()
        ))
    } else if !vc.unreached_codefiles.is_empty() {
        ("ground", format!(
            "{} CodeFile(s) reached by no intent — see which with `loom coverage`, then ground them (`loom edge implement`) or `loom ignore` them.",
            vc.unreached_codefiles.len()
        ))
    } else if v_failing > 0 {
        ("validate", "A validation is failing — `loom next --mode validate` (fix the code, then re-run `loom validate <intent>`).".to_string())
    } else if v_uninspected > 0 {
        ("validate", "Run pending validations: `loom next --mode validate`.".to_string())
    } else if g_failing > 0 {
        ("quality", "A quality gate is failing — `loom next --mode quality`, refactor to meet it, then record `loom rule verdict`.".to_string())
    } else if g_uninspected > 0 {
        ("quality", "Quality gates applied but unchecked — `loom next --mode quality`, inspect, then earn green with `loom rule verdict`.".to_string())
    } else if rt_uninspected > 0 || unexplored_pairs > 0 {
        ("discovery", format!(
            "Vertical spine complete ✓. Optional: close the N×N grid — {unexplored_pairs} unexplored pair(s) left: `loom next`."
        ))
    } else {
        ("complete", "Vertically complete ✓ and horizontally explored ✓ — confirm with `loom coverage` (nothing on disk unmapped) and `loom report`.".to_string())
    };

    let meta = get_meta(db)?;
    Ok(GraphState {
        version: meta.as_ref().map(|m| m.version.clone()).unwrap_or_default(),
        intents,
        relates_to_edges,
        implements_edges,
        total_edges,
        unresolved_edges,
        unexplored_pairs,
        codefiles,
        validations,
        notes,
        last_synced: meta.map(|m| m.last_synced).unwrap_or_default(),
        vertically_complete: vc.complete,
        horizontally_explored,
        phase: phase.to_string(),
        next_action,
    })
}

pub fn count_intents(db: &dyn LoomDb) -> Result<i64> {
    let r = db.execute("MATCH (n:Intent) RETURN count(n) AS c")?;
    Ok(r.rows().first().map(|row| i64_val(&row[0])).unwrap_or(0))
}

pub fn count_codefiles(db: &dyn LoomDb) -> Result<i64> {
    let r = db.execute("MATCH (n:CodeFile) RETURN count(n) AS c")?;
    Ok(r.rows().first().map(|row| i64_val(&row[0])).unwrap_or(0))
}

pub fn count_validations(db: &dyn LoomDb) -> Result<i64> {
    let r = db.execute("MATCH (n:Validation) RETURN count(n) AS c")?;
    Ok(r.rows().first().map(|row| i64_val(&row[0])).unwrap_or(0))
}

/// Count edges by inspection_status across every edge type that *carries* one
/// (RELATES_TO, IMPLEMENTS, GOVERNS, VALIDATES). HIERARCHY is excluded — it's a
/// structural tree edge with no inspection_status (schema v3).
/// Returns a flat map of status → count (summed across those types).
pub fn count_all_edges_by_inspection_status(db: &dyn LoomDb) -> Result<HashMap<String, i64>> {
    let mut map: HashMap<String, i64> = HashMap::new();
    let edge_types = [
        "MATCH ()-[r:RELATES_TO]->() RETURN r.inspection_status AS s, count(r) AS c",
        "MATCH ()-[r:IMPLEMENTS]->() RETURN r.inspection_status AS s, count(r) AS c",
        "MATCH ()-[r:GOVERNS]->()   RETURN r.inspection_status AS s, count(r) AS c",
        "MATCH ()-[r:VALIDATES]->() RETURN r.inspection_status AS s, count(r) AS c",
    ];
    for q in &edge_types {
        let result = db.execute(q)?;
        let cols = col_map(&result);
        for row in result.rows() {
            let s = str_val(get(row, &cols, "s"));
            let c = i64_val(get(row, &cols, "c"));
            if !s.is_empty() {
                *map.entry(s).or_insert(0) += c;
            }
        }
    }
    Ok(map)
}

/// Count validation pass rate: fraction of Validation nodes with last_result = 'passed'.
pub fn validation_pass_rate(db: &dyn LoomDb) -> Result<f64> {
    let total_r = db.execute("MATCH (v:Validation) RETURN count(v) AS c")?;
    let total = total_r.rows().first().map(|r| i64_val(&r[0])).unwrap_or(0);
    if total == 0 {
        return Ok(0.0);
    }
    let passed_r = db.execute(
        "MATCH (v:Validation) WHERE v.last_result = 'passed' RETURN count(v) AS c"
    )?;
    let passed = passed_r.rows().first().map(|r| i64_val(&r[0])).unwrap_or(0);
    Ok(passed as f64 / total as f64)
}

/// Distinct CodeFile paths that at least one intent IMPLEMENTS (i.e. grounded
/// in an intent). Used by `loom coverage`.
pub fn grounded_paths(db: &dyn LoomDb) -> Result<Vec<String>> {
    let r = db.execute("MATCH (i:Intent)-[e:IMPLEMENTS]->(cf:CodeFile) RETURN cf.path AS p")?;
    let cols = col_map(&r);
    let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in r.rows() {
        set.insert(str_val(get(row, &cols, "p")));
    }
    Ok(set.into_iter().collect())
}

/// Files implementing the most intents — a structural "tangle" signal (one file
/// carrying many concerns). Returns (path, intent_count) sorted desc.
pub fn tangled_files(db: &dyn LoomDb, limit: usize) -> Result<Vec<(String, i64)>> {
    let q = "MATCH (i:Intent)-[e:IMPLEMENTS]->(cf:CodeFile) \
             RETURN cf.path AS p, count(e) AS c ORDER BY c DESC";
    let result = db.execute(q)?;
    let cols = col_map(&result);
    let mut rows: Vec<(String, i64)> = result
        .rows()
        .iter()
        .map(|row| (str_val(get(row, &cols, "p")), i64_val(get(row, &cols, "c"))))
        .collect();
    rows.truncate(limit);
    Ok(rows)
}

/// Top intents by degree centrality (RELATES_TO edges).
pub fn top_intents_by_centrality(db: &dyn LoomDb, limit: usize) -> Result<Vec<IntentCentrality>> {
    let intents = list_intents(db, None, None)?;
    let mut with_degree: Vec<(Intent, i64)> = Vec::new();
    for intent in intents {
        let deg = intent_degree(db, &intent.id)?;
        with_degree.push((intent, deg));
    }
    with_degree.sort_by(|a, b| b.1.cmp(&a.1));
    with_degree.truncate(limit);
    Ok(with_degree.into_iter().map(|(intent, degree)| IntentCentrality { intent, degree }).collect())
}
