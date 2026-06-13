//! Count / coverage / centrality statistics for `loom status` and `loom report`.

use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

use crate::db::LoomDb;
use crate::types::{Intent, IntentCentrality};

use super::completeness::vertical_completeness_from_snapshot;
use super::hierarchy::list_all_hierarchy;
use super::hypothesis::list_hypotheses;
use super::implements::intents_with_implements;
use super::intent::{intents_without_validations, list_active_intents};
use super::meta::get_meta;
use super::row::{col_map, get, i64_val, str_val};
use super::scoring::{
    all_intent_degrees, count_unexplored_pairs_from, normative_coverage_from_snapshot,
    validate_selection_from_snapshot,
};
use super::smells::compute_smells_from;
use super::snapshot::QuerySnapshot;
/// Completeness gaps — "what's missing," not "what's present". Flags ungrounded
/// confirmed intents, intents with no validation, and feature groups that have a
/// happy path but no sad/fallback sibling (path-coverage via the `aspect` tag).
pub fn completeness_gaps(db: &dyn LoomDb) -> Result<Vec<String>> {
    let mut gaps = Vec::new();
    let intents = list_active_intents(db)?;
    let with_code = intents_with_implements(db)?;
    let hierarchy = list_all_hierarchy(db)?;
    let aspect_by_id: HashMap<String, String> = intents
        .iter()
        .map(|i| (i.id.clone(), i.aspect.clone()))
        .collect();
    // Confirmed intents not grounded to any code.
    for i in &intents {
        if i.status == "confirmed" && !with_code.contains(&i.id) {
            gaps.push(format!(
                "Intent '{}' is confirmed but not grounded to code (no IMPLEMENTS edge).",
                i.name
            ));
        }
    }

    // Intents with no validation (no proof of fulfilment).
    for i in intents_without_validations(db)? {
        gaps.push(format!(
            "Intent '{}' has no validation (no proof it's fulfilled).",
            i.name
        ));
    }

    let mut children_by_parent: HashMap<&str, Vec<&str>> = HashMap::new();
    for (parent, child) in &hierarchy {
        children_by_parent
            .entry(parent.as_str())
            .or_default()
            .push(child.as_str());
    }

    // Path coverage: a parent whose children include a happy path but no
    // sad/fallback sibling.
    for parent in &intents {
        let mut child_aspects: std::collections::HashSet<String> = std::collections::HashSet::new();
        for child_id in children_by_parent
            .get(parent.id.as_str())
            .into_iter()
            .flatten()
        {
            if let Some(a) = aspect_by_id.get(*child_id) {
                if !a.is_empty() {
                    child_aspects.insert(a.clone());
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
    /// seed | build | fix | incomplete | ground | validate | quality |
    /// discovery | audit | complete
    pub phase: String,
    pub next_action: String,
    /// The 360° coverage vector — every dimension counted, weakest first to fix.
    pub coverage: Coverage360,
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
    let snapshot = QuerySnapshot::load(db)?;
    graph_state_from_snapshot(db, &snapshot)
}

pub fn graph_state_from_snapshot(db: &dyn LoomDb, snapshot: &QuerySnapshot) -> Result<GraphState> {
    // Active intents only: retired (deprecated) design is invisible to every
    // computed number here — counts, pair denominators, coverage axes.
    let all_intents = &snapshot.intents;
    let intents = all_intents.len() as i64;
    let codefiles = snapshot.codefiles.len() as i64;
    let validations = snapshot.validations.len() as i64;
    let notes = db
        .execute("MATCH (n:Note) RETURN count(n) AS c")?
        .rows()
        .first()
        .map(|row| i64_val(&row[0]))
        .unwrap_or(0);

    let mut by_status: HashMap<&str, i64> = HashMap::new();
    for s in snapshot
        .relates
        .iter()
        .map(|e| e.inspection_status.as_str())
        .chain(
            snapshot
                .implements
                .iter()
                .map(|e| e.inspection_status.as_str()),
        )
        .chain(
            snapshot
                .governs
                .iter()
                .map(|e| e.inspection_status.as_str()),
        )
        .chain(
            snapshot
                .validates
                .iter()
                .map(|e| e.inspection_status.as_str()),
        )
    {
        if !s.is_empty() {
            *by_status.entry(s).or_insert(0) += 1;
        }
    }
    let total_edges: i64 = by_status.values().sum();

    // The discovery/fix loop only actions RELATES_TO, so the phase + the
    // "unresolved" backlog are computed from RELATES_TO specifically. IMPLEMENTS/
    // GOVERNS/HIERARCHY are structural (default passing); VALIDATES completeness
    // is surfaced by `loom report`, not the compass. (Counting all edge types
    // here would tell the user to run `loom next` for work it can't action.)
    let all_relates = &snapshot.relates;
    let mut rt_uninspected = 0;
    let mut rt_failing = 0;
    let mut rt_needs_rev = 0;
    for e in all_relates {
        match e.inspection_status.as_str() {
            "uninspected" => rt_uninspected += 1,
            "failing" => rt_failing += 1,
            "needs_reverification" => rt_needs_rev += 1,
            _ => {}
        }
    }

    // VALIDATES has its own loop (`loom validate`). The compass routes on the
    // validator queue's OWN selection (`validate_selection` — shared verbatim
    // with `loom next --mode validate`), never on raw edge-state counts: the
    // two once disagreed (a multi-intent validation's passed run left sibling
    // edges uninspected → phase=validate with an empty queue). Edge counts
    // below feed only the `unresolved` tally.
    let validate_backlog = validate_selection_from_snapshot(&snapshot);
    let v_failing_in_backlog = validate_backlog.iter().any(|(_, u, _)| *u >= 4.0);
    let v_no_proof = validate_backlog
        .iter()
        .filter(|(_, u, _)| *u >= 3.0 && *u < 4.0)
        .count();

    let validation_result: HashMap<&str, &str> = snapshot
        .validations
        .iter()
        .map(|v| (v.id.as_str(), v.last_result.as_str()))
        .collect();
    let mut v_uninspected_raw = 0;
    let mut v_failing = 0;
    let mut blocked_uninspected = 0;
    for e in &snapshot.validates {
        match e.inspection_status.as_str() {
            "uninspected" => {
                v_uninspected_raw += 1;
                if validation_result.get(e.validation_id.as_str()).copied() == Some("blocked") {
                    blocked_uninspected += 1;
                }
            }
            "failing" => v_failing += 1,
            _ => {}
        }
    }
    let v_uninspected = (v_uninspected_raw - blocked_uninspected).max(0);

    // GOVERNS is the green gate: an uninspected gate is an unchecked quality
    // claim; failing is a violation; needs_reverification is green that must
    // be re-earned after a code change. ALL THREE are quality work — exactly
    // what `loom next --mode quality` serves (stale GOVERNS once drove the
    // queue but not the compass or the unresolved tally — a coherence bug).
    let mut g_uninspected = 0;
    let mut g_failing = 0;
    let mut g_needs_rev = 0;
    for e in &snapshot.governs {
        match e.inspection_status.as_str() {
            "uninspected" => g_uninspected += 1,
            "failing" => g_failing += 1,
            "needs_reverification" => g_needs_rev += 1,
            _ => {}
        }
    }

    let unresolved_edges = rt_uninspected
        + rt_failing
        + rt_needs_rev
        + v_uninspected
        + v_failing
        + g_uninspected
        + g_failing
        + g_needs_rev;

    let relates_to_edges = snapshot.relates.len() as i64;
    let implements_edges = snapshot.implements.len() as i64;

    // Use the SAME computation `loom next` uses for discovery candidates, so the
    // compass can never disagree with what `loom next` actually surfaces (e.g.
    // hierarchy-linked pairs are excluded). Authoritative, not a heuristic.
    // Arithmetic count — the full scored O(N²) enumeration lives in discovery
    // (`unexplored_pairs_scored`) where the items are actually consumed.
    let hierarchy = &snapshot.hierarchy;
    let unexplored_pairs = count_unexplored_pairs_from(all_intents, all_relates, hierarchy);

    // Lifecycle backlog (prescriptive axis): intents that need building/changing.
    let needs_change = all_intents
        .iter()
        .filter(|i| i.lifecycle == "needs_change")
        .count() as i64;
    let planned = all_intents
        .iter()
        .filter(|i| i.lifecycle == "planned")
        .count() as i64;

    // The two completeness axes. Vertical (binding) is the spine; horizontal
    // (optional) is the N×N grid. The compass routes vertical gaps ahead of
    // optional discovery, and only calls the graph "complete" when both hold.
    let vc = vertical_completeness_from_snapshot(snapshot);
    let horizontally_explored = unexplored_pairs == 0 && rt_uninspected == 0 && rt_needs_rev == 0;

    // --- The 360° coverage vector ---------------------------------------
    let nc = normative_coverage_from_snapshot(&snapshot);
    let rules_count = snapshot.rules.len() as i64;

    let is_parent: std::collections::HashSet<&str> =
        hierarchy.iter().map(|(p, _)| p.as_str()).collect();
    let with_code = &snapshot.with_code;
    let implemented_leaves: Vec<&Intent> = all_intents
        .iter()
        .filter(|i| i.lifecycle == "implemented" && !is_parent.contains(i.id.as_str()))
        .collect();
    let realized_leaves = CoverageAxis {
        covered: implemented_leaves
            .iter()
            .filter(|i| with_code.contains(&i.id))
            .count() as i64,
        total: implemented_leaves.len() as i64,
    };

    let active_ids: std::collections::HashSet<&str> =
        all_intents.iter().map(|i| i.id.as_str()).collect();
    let grounded: std::collections::HashSet<&str> = snapshot
        .implements
        .iter()
        .filter(|edge| active_ids.contains(edge.intent_id.as_str()))
        .map(|edge| edge.codefile_path.as_str())
        .collect();
    let grounded_files = CoverageAxis {
        covered: grounded.len() as i64,
        total: codefiles,
    };

    // Explored = inspected RELATES_TO pairs over the candidate grid (C(n,2)
    // minus HIERARCHY-linked pairs — containment isn't a grid cell). Set ops
    // run over EDGES only; the denominator stays arithmetic (no O(N²) walk).
    // EVERYTHING here is filtered to ACTIVE intents — the same universe
    // `count_unexplored_pairs_from` uses — so the identity
    //   total == covered + pending(uninspected/stale pairs) + unexplored
    // holds exactly. (A hierarchy edge touching a retired intent once leaked
    // into the denominator, making status report covered/total/unexplored
    // numbers that didn't add up.)
    fn pair_key<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
        if a < b {
            (a, b)
        } else {
            (b, a)
        }
    }
    let active_ids: std::collections::HashSet<&str> =
        all_intents.iter().map(|i| i.id.as_str()).collect();
    let hier_pairs: std::collections::HashSet<(&str, &str)> = hierarchy
        .iter()
        .filter(|(p, c)| active_ids.contains(p.as_str()) && active_ids.contains(c.as_str()))
        .map(|(p, c)| pair_key(p, c))
        .collect();
    let mut inspected_pairs: std::collections::HashSet<(&str, &str)> =
        std::collections::HashSet::new();
    for e in all_relates {
        if matches!(
            e.inspection_status.as_str(),
            "passing" | "failing" | "independent"
        ) && active_ids.contains(e.from_id.as_str())
            && active_ids.contains(e.to_id.as_str())
        {
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
    let proven_ids: std::collections::HashSet<&str> = snapshot
        .validates
        .iter()
        .filter(|edge| {
            validation_result.get(edge.validation_id.as_str()).copied() == Some("passed")
        })
        .map(|edge| edge.intent_id.as_str())
        .collect();
    let proven_leaves = CoverageAxis {
        covered: implemented_leaves
            .iter()
            .filter(|i| proven_ids.contains(i.id.as_str()))
            .count() as i64,
        total: implemented_leaves.len() as i64,
    };

    let coverage = Coverage360 {
        grounded_files,
        realized_leaves,
        explored_pairs,
        measured_pairs: CoverageAxis {
            covered: nc.measured_pairs,
            total: nc.total_pairs,
        },
        proven_leaves,
    };
    let unmeasured_queue = nc.queue.len();

    let (phase, next_action) = if intents == 0 {
        ("seed", "Empty graph — capture the user's head first: `loom guide --mode seed` teaches the interview; land answers with `loom intent add --level system …`.".to_string())
    } else if needs_change > 0 {
        ("build", format!("{needs_change} intent(s) need changes (known issues/refactor): `loom next --mode build`."))
    } else if rt_failing > 0 || rt_needs_rev > 0 {
        (
            "fix",
            "Resolve failures / re-verify stale edges: `loom next --mode fix`.".to_string(),
        )
    } else if planned > 0 {
        (
            "build",
            format!("{planned} planned intent(s) to build: `loom next --mode build`."),
        )
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
    } else if v_failing_in_backlog {
        ("validate", "A validation is failing — `loom next --mode validate` (fix the code, then re-run `loom validate <intent>`).".to_string())
    } else if !validate_backlog.is_empty() {
        (
            "validate",
            if v_no_proof > 0 {
                format!("{} intent(s) need proof (missing or unrun validations): `loom next --mode validate`.", validate_backlog.len())
            } else {
                "Run pending validations: `loom next --mode validate`.".to_string()
            },
        )
    } else if g_failing > 0 {
        ("quality", "A quality gate is failing — `loom next --mode quality`, refactor to meet it, then record `loom rule verdict`.".to_string())
    } else if g_needs_rev > 0 {
        ("quality", "Quality green went stale (the code under a passing verdict changed) — `loom next --mode quality`, re-inspect, re-earn with `loom rule verdict`.".to_string())
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
        // The audit gate — the last gate before green. Open smells are
        // unadjudicated suspicions; green means every one was ANSWERED
        // (structurally fixed, or refuted via its remedy — an `independent`
        // verdict / vocab merge / decision note counts exactly as much as a
        // fix). Computed lazily: only graphs that cleared every other gate
        // pay for the O(N²) scan, and at this point every pair is linked, so
        // the pairwise detectors short-circuit.
        let open_findings = compute_smells_from(db, &snapshot)?.open.len();
        if open_findings > 0 {
            ("audit", format!(
                "{open_findings} open finding(s) — `loom smells`: resolve or refute each via its remedy (an `independent` verdict or decision note is as valuable as a fix). Green requires 0 open findings."
            ))
        } else {
            // The pre-decision plane never gates green (a proposal is not
            // state of the world — see Hypothesis), but it must not rot
            // invisibly either: pending proofs are disclosed at the one
            // message every agent reads. Computed lazily with the same
            // justification as the smells scan above.
            let proposed = list_hypotheses(db, Some("proposed"))?.len();
            let mut msg = "Vertically complete ✓, horizontally explored ✓, 0 open findings ✓ — confirm with `loom coverage` (nothing on disk unmapped) and `loom report`. Then make the green DURABLE: run `loom export` and commit the graph with the code, re-run it after every graph change (`loom export --check` verifies; CI wiring is optional extra hardening), and keep running `loom sync` after code changes (maintenance mode).".to_string();
            if proposed > 0 {
                msg.push_str(&format!(
                    " Pre-decision plane: {proposed} proposed hypothesis(es) await proof — optional, never gates green: `loom next --mode prove`."
                ));
            }
            ("complete", msg)
        }
    };

    let meta = get_meta(db)?;
    Ok(GraphState {
        version: meta.as_ref().map(|m| m.version.clone()).unwrap_or_default(),
        graph_id: meta
            .as_ref()
            .map(|m| m.graph_id.clone())
            .unwrap_or_default(),
        graph_name: meta
            .as_ref()
            .map(|m| m.graph_name.clone())
            .unwrap_or_default(),
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

/// Uninspected VALIDATES edges whose validation is `blocked` — a recorded
/// "can't run yet", not pending work. Excluded from `unresolved_edges` and
/// reported by `loom status` as outside-the-queues so the raw histogram and
/// the queue tally can be reconciled at a glance. Filtering happens in Rust
/// (node-anchored scan) — the reliable path.
pub fn blocked_uninspected_validates(db: &dyn LoomDb) -> Result<i64> {
    let r = db.execute(
        "MATCH (v:Validation)-[e:VALIDATES]->(:Intent) \
         RETURN v.last_result AS lr, e.inspection_status AS s",
    )?;
    let cols = col_map(&r);
    Ok(r.rows()
        .iter()
        .filter(|row| {
            str_val(get(row, &cols, "lr")) == "blocked"
                && str_val(get(row, &cols, "s")) == "uninspected"
        })
        .count() as i64)
}

/// Uninspected edges NO work queue serves — the explanation for
/// `uninspected_edges > unresolved_edges` in `loom status`: structural
/// IMPLEMENTS assertions (grounding claims, not verdicts) and VALIDATES
/// edges on blocked validations (recorded "can't run yet").
#[derive(Debug, Clone, Serialize)]
pub struct UninspectedOutsideQueues {
    pub implements: i64,
    pub blocked_validations: i64,
}

pub fn uninspected_outside_queues(db: &dyn LoomDb) -> Result<UninspectedOutsideQueues> {
    Ok(uninspected_outside_queues_from_snapshot(
        &QuerySnapshot::load(db)?,
    ))
}

pub fn uninspected_outside_queues_from_snapshot(
    snapshot: &QuerySnapshot,
) -> UninspectedOutsideQueues {
    let validation_result: HashMap<&str, &str> = snapshot
        .validations
        .iter()
        .map(|v| (v.id.as_str(), v.last_result.as_str()))
        .collect();
    let implements = snapshot
        .implements
        .iter()
        .filter(|e| e.inspection_status == "uninspected")
        .count() as i64;
    let blocked_validations = snapshot
        .validates
        .iter()
        .filter(|e| {
            e.inspection_status == "uninspected"
                && validation_result.get(e.validation_id.as_str()) == Some(&"blocked")
        })
        .count() as i64;
    UninspectedOutsideQueues {
        implements,
        blocked_validations,
    }
}

/// Edge inspection_status histogram from an already-loaded snapshot — the hot
/// path for `loom status` / closeout views that would otherwise issue four
/// separate edge-count queries.
pub fn edge_status_counts_from_snapshot(snapshot: &QuerySnapshot) -> HashMap<String, i64> {
    let mut map: HashMap<String, i64> = HashMap::new();
    for s in snapshot
        .relates
        .iter()
        .map(|e| e.inspection_status.as_str())
        .chain(
            snapshot
                .implements
                .iter()
                .map(|e| e.inspection_status.as_str()),
        )
        .chain(
            snapshot
                .governs
                .iter()
                .map(|e| e.inspection_status.as_str()),
        )
        .chain(
            snapshot
                .validates
                .iter()
                .map(|e| e.inspection_status.as_str()),
        )
    {
        if !s.is_empty() {
            *map.entry(s.to_string()).or_insert(0) += 1;
        }
    }
    map
}

pub fn validation_pass_rate_from_snapshot(snapshot: &QuerySnapshot) -> f64 {
    let total = snapshot.validations.len();
    if total == 0 {
        return 0.0;
    }
    let passed = snapshot
        .validations
        .iter()
        .filter(|v| v.last_result == "passed")
        .count();
    passed as f64 / total as f64
}

pub fn intents_without_validations_count_from_snapshot(snapshot: &QuerySnapshot) -> i64 {
    let validated: std::collections::HashSet<&str> = snapshot
        .validates
        .iter()
        .map(|e| e.intent_id.as_str())
        .collect();
    snapshot
        .intents
        .iter()
        .filter(|i| !validated.contains(i.id.as_str()))
        .count() as i64
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
    let passed_r =
        db.execute("MATCH (v:Validation) WHERE v.last_result = 'passed' RETURN count(v) AS c")?;
    let passed = passed_r.rows().first().map(|r| i64_val(&r[0])).unwrap_or(0);
    Ok(passed as f64 / total as f64)
}

/// Distinct CodeFile paths that at least one intent IMPLEMENTS (i.e. grounded
/// in an intent). Used by `loom coverage`.
pub fn grounded_paths(db: &dyn LoomDb) -> Result<Vec<String>> {
    // Grounded means grounded by LIVE design: a file whose only owner was
    // retired is no longer explained (status filtered in Rust).
    let r = db.execute(
        "MATCH (i:Intent)-[e:IMPLEMENTS]->(cf:CodeFile) RETURN cf.path AS p, i.status AS s",
    )?;
    let cols = col_map(&r);
    let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in r.rows() {
        if str_val(get(row, &cols, "s")) != "deprecated" {
            set.insert(str_val(get(row, &cols, "p")));
        }
    }
    Ok(set.into_iter().collect())
}

/// Files implementing the most intents — a structural "tangle" signal (one file
/// carrying many concerns). Returns (path, intent_count) sorted desc.
pub fn tangled_files(db: &dyn LoomDb, limit: usize) -> Result<Vec<(String, i64)>> {
    let q = "MATCH (i:Intent)-[e:IMPLEMENTS]->(cf:CodeFile) \
             RETURN cf.path AS p, count(e) AS c";
    let result = db.execute(q)?;
    let cols = col_map(&result);
    let mut rows: Vec<(String, i64)> = result
        .rows()
        .iter()
        .map(|row| (str_val(get(row, &cols, "p")), i64_val(get(row, &cols, "c"))))
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rows.truncate(limit);
    Ok(rows)
}

/// Top intents by degree centrality (RELATES_TO edges).
pub fn top_intents_by_centrality(db: &dyn LoomDb, limit: usize) -> Result<Vec<IntentCentrality>> {
    let intents = list_active_intents(db)?;
    // Bulk-load all degrees in 2 queries instead of 2×N queries.
    let degrees = all_intent_degrees(db)?;
    let mut with_degree: Vec<(Intent, i64)> = intents
        .into_iter()
        .map(|i| {
            let deg = *degrees.get(&i.id).unwrap_or(&0);
            (i, deg)
        })
        .collect();
    with_degree.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.id.cmp(&b.0.id)));
    with_degree.truncate(limit);
    Ok(with_degree
        .into_iter()
        .map(|(intent, degree)| IntentCentrality { intent, degree })
        .collect())
}
