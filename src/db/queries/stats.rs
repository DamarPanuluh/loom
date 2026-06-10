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
use super::rule::list_rules;
use super::scoring::{count_unexplored_pairs, intent_degree, normative_coverage};

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

/// One axis of the 360° coverage vector: covered/total along one dimension of
/// understanding. `total == 0` means the axis has no surface yet (e.g. no
/// quality rules seeded) — rendered as "—", never as a vacuous 100%.
#[derive(Debug, Clone, Serialize)]
pub struct CoverageAxis {
    pub covered: i64,
    pub total: i64,
}

impl CoverageAxis {
    pub fn done(&self) -> bool {
        self.total > 0 && self.covered >= self.total
    }
}

/// The 360° coverage vector — every dimension of "do we understand this repo",
/// each as a counted fraction so the driving LLM always sees which vantage
/// point is weakest. Surfaced on the pulse footer (every orientation command)
/// and in `graph_state` JSON; the compass routes to the weakest binding axis.
#[derive(Debug, Clone, Serialize)]
pub struct Coverage360 {
    /// Physical: CodeFiles reached by ≥1 IMPLEMENTS / all registered CodeFiles.
    pub grounded_files: CoverageAxis,
    /// Semantic→physical join: implemented leaf intents with code / implemented leaves.
    pub realized_leaves: CoverageAxis,
    /// Horizontal grid: intent pairs with an inspected RELATES_TO / candidate pairs.
    pub explored_pairs: CoverageAxis,
    /// Normative: rule × intent-with-code pairs measured (verdict here or on an
    /// ancestor — independent counts: "measured, doesn't apply") / full grid.
    pub measured_pairs: CoverageAxis,
    /// Proof: implemented leaf intents with a passed validation / implemented leaves.
    pub proven_leaves: CoverageAxis,
}

/// Compact "pulse" of the graph — cheap situational awareness + a recommended
/// next action (the compass). Returned in `--json` and rendered as a one-line
/// footer by the orientation commands.
#[derive(Debug, Clone, Serialize)]
pub struct GraphState {
    pub version: String,
    /// This graph's identity (uuid + human name) — what other looms reference.
    pub graph_id: String,
    pub graph_name: String,
    /// "owned" | "observed" ("" = owned, pre-identity graph).
    pub custody: String,
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
    /// The 360° coverage vector — every dimension counted, weakest first to fix.
    pub coverage: Coverage360,
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
    let v_failing = *v.get("failing").unwrap_or(&0);
    // `blocked` proofs are recorded decisions ("can't run yet — reason on
    // file"), not pending work: exclude their uninspected VALIDATES edges so
    // the compass doesn't route to work nobody can do. Filtering happens in
    // Rust (node-anchored scan) — the reliable path.
    let blocked_uninspected = {
        let r = db.execute(
            "MATCH (v:Validation)-[e:VALIDATES]->(:Intent) \
             RETURN v.last_result AS lr, e.inspection_status AS s",
        )?;
        let cols = col_map(&r);
        r.rows()
            .iter()
            .filter(|row| {
                str_val(get(row, &cols, "lr")) == "blocked"
                    && str_val(get(row, &cols, "s")) == "uninspected"
            })
            .count() as i64
    };
    let v_uninspected = (*v.get("uninspected").unwrap_or(&0) - blocked_uninspected).max(0);

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
    // Arithmetic count — the full scored O(N²) enumeration lives in discovery
    // (`unexplored_pairs_scored`) where the items are actually consumed.
    let unexplored_pairs = count_unexplored_pairs(db)?;

    // Lifecycle backlog (prescriptive axis): intents that need building/changing.
    let all_intents = list_intents(db, None, None)?;
    let needs_change = all_intents.iter().filter(|i| i.lifecycle == "needs_change").count() as i64;
    let planned = all_intents.iter().filter(|i| i.lifecycle == "planned").count() as i64;

    // The two completeness axes. Vertical (binding) is the spine; horizontal
    // (optional) is the N×N grid. The compass routes vertical gaps ahead of
    // optional discovery, and only calls the graph "complete" when both hold.
    let vc = vertical_completeness(db)?;
    let horizontally_explored = unexplored_pairs == 0 && rt_uninspected == 0 && rt_needs_rev == 0;

    // --- The 360° coverage vector ---------------------------------------
    let nc = normative_coverage(db)?;
    let rules_count = list_rules(db)?.len() as i64;

    let hierarchy = super::hierarchy::list_all_hierarchy(db)?;
    let is_parent: std::collections::HashSet<&str> =
        hierarchy.iter().map(|(p, _)| p.as_str()).collect();
    let with_code = super::implements::intents_with_implements(db)?;
    let implemented_leaves: Vec<&Intent> = all_intents
        .iter()
        .filter(|i| i.lifecycle == "implemented" && !is_parent.contains(i.id.as_str()))
        .collect();
    let realized_leaves = CoverageAxis {
        covered: implemented_leaves.iter().filter(|i| with_code.contains(&i.id)).count() as i64,
        total: implemented_leaves.len() as i64,
    };

    let grounded_files = CoverageAxis {
        covered: grounded_paths(db)?.len() as i64,
        total: codefiles,
    };

    // Explored = inspected RELATES_TO pairs over the candidate grid (C(n,2)
    // minus HIERARCHY-linked pairs — containment isn't a grid cell). Set ops
    // run over EDGES only; the denominator stays arithmetic (no O(N²) walk).
    let pair_key = |a: &str, b: &str| {
        if a < b { (a.to_string(), b.to_string()) } else { (b.to_string(), a.to_string()) }
    };
    let hier_pairs: std::collections::HashSet<(String, String)> =
        hierarchy.iter().map(|(p, c)| pair_key(p, c)).collect();
    let mut inspected_pairs: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    for e in list_relates_to(db, None)? {
        if matches!(e.inspection_status.as_str(), "passing" | "failing" | "independent") {
            let k = pair_key(&e.from_id, &e.to_id);
            if !hier_pairs.contains(&k) {
                inspected_pairs.insert(k);
            }
        }
    }
    let explored_pairs = CoverageAxis {
        covered: inspected_pairs.len() as i64,
        total: (intents * (intents - 1) / 2 - hier_pairs.len() as i64).max(0),
    };

    // Proven = implemented leaves whose proof actually PASSED (blocked/not_run
    // are visible elsewhere; this axis counts earned proof only).
    let proven_ids: std::collections::HashSet<String> = {
        let r = db.execute(
            "MATCH (v:Validation)-[e:VALIDATES]->(i:Intent) \
             RETURN v.last_result AS lr, i.id AS iid",
        )?;
        let cols = col_map(&r);
        r.rows()
            .iter()
            .filter(|row| str_val(get(row, &cols, "lr")) == "passed")
            .map(|row| str_val(get(row, &cols, "iid")))
            .collect()
    };
    let proven_leaves = CoverageAxis {
        covered: implemented_leaves.iter().filter(|i| proven_ids.contains(&i.id)).count() as i64,
        total: implemented_leaves.len() as i64,
    };

    let coverage = Coverage360 {
        grounded_files,
        realized_leaves,
        explored_pairs,
        measured_pairs: CoverageAxis { covered: nc.measured_pairs, total: nc.total_pairs },
        proven_leaves,
    };
    let unmeasured_queue = nc.queue.len();

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
    } else if unmeasured_queue > 0 {
        ("quality", format!(
            "{unmeasured_queue} rule×intent pair(s) never measured — `loom next --mode quality`. One command resolves each: `loom rule verdict` creates the edge with the verdict (a verdict at component altitude covers descendants; independent = measured, doesn't apply)."
        ))
    } else if rules_count == 0 && nc.intents_with_code > 0 {
        ("quality", "The normative plane is EMPTY — no measuring sticks, so 360° coverage can't be earned. `loom detect` recommends packs for this repo; seed with `loom rule seed iso5055` (baseline, applies to any code), then measure at the highest honest altitude.".to_string())
    } else if rt_uninspected > 0 || unexplored_pairs > 0 {
        ("discovery", format!(
            "Vertical spine complete ✓. Optional: close the N×N grid — {unexplored_pairs} unexplored pair(s) left: `loom next`."
        ))
    } else {
        ("complete", "Vertically complete ✓ and horizontally explored ✓ — confirm with `loom coverage` (nothing on disk unmapped) and `loom report`. Then make the green DURABLE: wire `loom export --check` into pre-commit/CI so a code change can't merge with a stale committed graph, and keep running `loom sync` after changes (maintenance mode).".to_string())
    };

    let meta = get_meta(db)?;
    Ok(GraphState {
        version: meta.as_ref().map(|m| m.version.clone()).unwrap_or_default(),
        graph_id: meta.as_ref().map(|m| m.graph_id.clone()).unwrap_or_default(),
        graph_name: meta.as_ref().map(|m| m.graph_name.clone()).unwrap_or_default(),
        custody: meta.as_ref().map(|m| m.custody.clone()).unwrap_or_default(),
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
        coverage,
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
