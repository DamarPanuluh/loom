//! Count / coverage / centrality statistics for `loom status` and `loom report`.

use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

use crate::types::{Governs, Intent, IntentCentrality, RelatesTo, StatusReport, Validation};

use super::completeness::vertical_completeness_from_snapshot;
use super::meta::GraphMeta;
use super::scoring::{
    count_unexplored_pairs_from, normative_coverage_from_snapshot, validate_selection_from_snapshot,
};
use super::snapshot::QuerySnapshot;

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
    /// A note-hygiene nudge surfaced when the note log is heavy enough to drag
    /// the read path — empty otherwise. loom's output IS the driving LLM's next
    /// prompt, so the lever (`loom note prune --transitions` / the sync cap)
    /// teaches itself here instead of relying on the agent to know it exists.
    pub note_hygiene: String,
}

#[derive(Debug, Clone)]
pub struct GraphStateContext {
    pub meta: Option<GraphMeta>,
    pub notes: i64,
    pub transition_cap: usize,
}

pub fn graph_state_from_snapshot_parts(
    snapshot: &QuerySnapshot,
    context: GraphStateContext,
    mut open_findings: impl FnMut(&QuerySnapshot) -> Result<usize>,
    mut proposed_hypotheses: impl FnMut() -> Result<usize>,
) -> Result<GraphState> {
    // Active intents only: retired (deprecated) design is invisible to every
    // computed number here — counts, pair denominators, coverage axes.
    let all_intents = &snapshot.intents;
    let intents = all_intents.len() as i64;
    let codefiles = snapshot.codefiles.len() as i64;
    let validations = snapshot.validations.len() as i64;
    let notes = context.notes;

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
    let active_ids: std::collections::HashSet<&str> =
        all_intents.iter().map(|i| i.id.as_str()).collect();
    let mut rt_uninspected = 0;
    let mut rt_failing = 0;
    let mut rt_needs_rev = 0;
    for e in all_relates {
        if !active_ids.contains(e.from_id.as_str()) || !active_ids.contains(e.to_id.as_str()) {
            continue;
        }
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
    let validate_backlog = validate_selection_from_snapshot(snapshot);
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
    let nc = normative_coverage_from_snapshot(snapshot);
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
    } else if rt_failing > 0 {
        (
            "fix",
            format!("{rt_failing} relationship(s) FAILING — `loom next --mode fix` (resolve violations at root cause)."),
        )
    } else if planned > 0 {
        (
            "build",
            format!("{planned} planned intent(s) to build: `loom next --mode build`."),
        )
    } else if rt_needs_rev > 0 {
        // Stale RELATES_TO is the OPTIONAL horizontal grid — re-verifying it
        // ranks BELOW building `planned` intents (the binding vertical spine).
        // `rt_failing` (a genuine violation) is handled above and stays urgent.
        // Both branches route to the same `loom next --mode fix` queue, which
        // serves failing|needs_reverification; here rt_failing == 0, so it
        // serves the stale items.
        (
            "fix",
            format!("{rt_needs_rev} stale edge(s) to re-verify (optional grid upkeep after a code change) — `loom next --mode fix`."),
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
        let open_findings = open_findings(snapshot)?;
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
            let proposed = proposed_hypotheses()?;
            let mut msg = "Vertically complete ✓, horizontally explored ✓, 0 open findings ✓ — confirm with `loom coverage` (nothing on disk unmapped) and `loom report`. Then make the green DURABLE: run `loom export` and commit the graph with the code, re-run it after every graph change (`loom export --check` verifies; CI wiring is optional extra hardening), and keep running `loom sync` after code changes (maintenance mode).".to_string();
            if proposed > 0 {
                msg.push_str(&format!(
                    " Pre-decision plane: {proposed} proposed hypothesis(es) await proof — optional, never gates green: `loom next --mode prove`."
                ));
            }
            ("complete", msg)
        }
    };

    // Note-hygiene nudge: when the log is heavy enough to drag the read path,
    // teach the lever. The cap auto-bounds via sync, so this fires mainly for
    // graphs with the cap OFF or not yet swept — not normal capped operation
    // (the threshold sits well above a healthy capped graph). Cheap: reuses the
    // `notes` count already computed; no per-note materialization.
    const NOTE_HEAVY: i64 = 5000;
    let note_hygiene = if notes > NOTE_HEAVY {
        let cap = context.transition_cap;
        if cap == 0 {
            format!("{notes} notes — the transition log is UNCAPPED and slows every command. `loom note prune --set-cap 20` bounds it (sync then holds it there).")
        } else {
            format!("{notes} notes are weighing on the read path. `loom sync` compacts transition churn toward the cap ({cap}/target); `loom note prune --transitions` does it now.")
        }
    } else {
        String::new()
    };

    let meta = context.meta;
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
        note_hygiene,
    })
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

pub fn status_report_from_snapshot(snapshot: &QuerySnapshot) -> StatusReport {
    let total_intents = snapshot.intents.len() as i64;
    let total_codefiles = snapshot.codefiles.len() as i64;
    let total_validations = snapshot.validations.len() as i64;

    let by_status = edge_status_counts_from_snapshot(snapshot);
    let total_edges = by_status.values().sum::<i64>();
    let uninspected = *by_status.get("uninspected").unwrap_or(&0);
    let passing = *by_status.get("passing").unwrap_or(&0);
    let failing = *by_status.get("failing").unwrap_or(&0);
    let independent = *by_status.get("independent").unwrap_or(&0);
    let needs_reverification = *by_status.get("needs_reverification").unwrap_or(&0);

    let pass_rate = validation_pass_rate_from_snapshot(snapshot);
    let (blocked_validations, validation_pass_rate_runnable) =
        blocked_count_and_runnable_rate(&snapshot.validations);
    let no_val_count = intents_without_validations_count_from_snapshot(snapshot);

    StatusReport {
        total_intents,
        total_codefiles,
        total_validations,
        total_edges,
        uninspected_edges: uninspected,
        passing_edges: passing,
        failing_edges: failing,
        independent_edges: independent,
        needs_reverification,
        intents_without_validations: no_val_count,
        validation_pass_rate: pass_rate,
        blocked_validations,
        validation_pass_rate_runnable,
        open_issues: failing,
    }
}

pub fn intents_without_validations_from_snapshot(snapshot: &QuerySnapshot) -> Vec<Intent> {
    let validated: std::collections::HashSet<&str> = snapshot
        .validates
        .iter()
        .map(|e| e.intent_id.as_str())
        .collect();
    let parents: std::collections::HashSet<&str> = snapshot
        .hierarchy
        .iter()
        .map(|(parent, _child)| parent.as_str())
        .collect();
    snapshot
        .intents
        .iter()
        .filter(|i| i.lifecycle == "implemented")
        .filter(|i| !parents.contains(i.id.as_str()))
        .filter(|i| !validated.contains(i.id.as_str()))
        .cloned()
        .collect()
}

pub fn completeness_gaps_from_snapshot(snapshot: &QuerySnapshot) -> Vec<String> {
    let mut gaps = Vec::new();
    let intents = &snapshot.intents;
    let aspect_by_id: HashMap<String, String> = intents
        .iter()
        .map(|i| (i.id.clone(), i.aspect.clone()))
        .collect();

    // Confirmed intents not grounded to any code.
    for intent in intents {
        if intent.status == "confirmed" && !snapshot.with_code.contains(&intent.id) {
            gaps.push(format!(
                "Intent '{}' is confirmed but not grounded to code (no IMPLEMENTS edge).",
                intent.name
            ));
        }
    }

    // Intents with no validation (no proof of fulfilment).
    for intent in intents_without_validations_from_snapshot(snapshot) {
        gaps.push(format!(
            "Intent '{}' has no validation (no proof it's fulfilled).",
            intent.name
        ));
    }

    let mut children_by_parent: HashMap<&str, Vec<&str>> = HashMap::new();
    for (parent, child) in &snapshot.hierarchy {
        children_by_parent
            .entry(parent.as_str())
            .or_default()
            .push(child.as_str());
    }

    // Path coverage: a parent whose children include a happy path but no
    // sad/fallback sibling.
    for parent in intents {
        let mut child_aspects: std::collections::HashSet<String> = std::collections::HashSet::new();
        for child_id in children_by_parent
            .get(parent.id.as_str())
            .into_iter()
            .flatten()
        {
            if let Some(aspect) = aspect_by_id.get(*child_id) {
                if !aspect.is_empty() {
                    child_aspects.insert(aspect.clone());
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
    gaps
}

pub fn top_intents_by_centrality_from_snapshot(
    snapshot: &QuerySnapshot,
    limit: usize,
) -> Vec<IntentCentrality> {
    let mut with_degree: Vec<(Intent, i64)> = snapshot
        .intents
        .iter()
        .cloned()
        .map(|intent| {
            let degree = *snapshot.degrees.get(&intent.id).unwrap_or(&0);
            (intent, degree)
        })
        .collect();
    with_degree.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.id.cmp(&b.0.id)));
    with_degree.truncate(limit);
    with_degree
        .into_iter()
        .map(|(intent, degree)| IntentCentrality { intent, degree })
        .collect()
}

pub fn failing_governs_from_snapshot(snapshot: &QuerySnapshot) -> Vec<Governs> {
    let mut governs: Vec<Governs> = snapshot
        .governs
        .iter()
        .filter(|edge| edge.inspection_status == "failing")
        .cloned()
        .collect();
    governs.sort_by(|a, b| b.last_inspected.cmp(&a.last_inspected));
    governs
}

pub fn recent_passing_from_snapshot(snapshot: &QuerySnapshot, limit: usize) -> Vec<RelatesTo> {
    let mut edges: Vec<RelatesTo> = snapshot
        .relates
        .iter()
        .filter(|edge| edge.inspection_status == "passing")
        .cloned()
        .collect();
    edges.sort_by(|a, b| b.last_inspected.cmp(&a.last_inspected));
    edges.truncate(limit);
    edges
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

/// (blocked count, runnable pass rate) for a set of validations. The runnable
/// rate is passed / (total − blocked): the health of proofs that CAN run, so a
/// wall of environmentally-blocked sagas (live target down) doesn't make the
/// headline rate read as failures. Falls back to the all-up rate when nothing
/// is blocked; 0.0 when nothing is runnable.
pub fn blocked_count_and_runnable_rate(validations: &[Validation]) -> (i64, f64) {
    let blocked = validations
        .iter()
        .filter(|v| v.last_result == "blocked")
        .count();
    let passed = validations
        .iter()
        .filter(|v| v.last_result == "passed")
        .count();
    let runnable = validations.len() - blocked;
    let rate = if runnable > 0 {
        passed as f64 / runnable as f64
    } else {
        0.0
    };
    (blocked as i64, rate)
}

pub fn intents_without_validations_count_from_snapshot(snapshot: &QuerySnapshot) -> i64 {
    let validated: std::collections::HashSet<&str> = snapshot
        .validates
        .iter()
        .map(|e| e.intent_id.as_str())
        .collect();
    // Parents inherit proof from their leaves — mirror of
    // `intents_without_validations` (intent.rs): active, implemented LEAVES only,
    // so this status count never disagrees with `loom report`/the validate queue.
    let parents: std::collections::HashSet<&str> = snapshot
        .hierarchy
        .iter()
        .map(|(parent, _child)| parent.as_str())
        .collect();
    snapshot
        .intents
        .iter()
        .filter(|i| i.lifecycle == "implemented")
        .filter(|i| !parents.contains(i.id.as_str()))
        .filter(|i| !validated.contains(i.id.as_str()))
        .count() as i64
}

pub fn tangled_files_from_snapshot(snapshot: &QuerySnapshot, limit: usize) -> Vec<(String, i64)> {
    let mut counts: HashMap<String, i64> = HashMap::new();
    for edge in &snapshot.implements {
        *counts.entry(edge.codefile_path.clone()).or_insert(0) += 1;
    }
    let mut rows: Vec<_> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rows.truncate(limit);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(result: &str) -> Validation {
        Validation {
            id: result.into(),
            name: result.into(),
            description: String::new(),
            validation_type: "manual_check".into(),
            command: String::new(),
            last_run: String::new(),
            last_result: result.into(),
        }
    }

    #[test]
    fn runnable_rate_excludes_blocked_from_the_denominator() {
        // 2 passed, 1 blocked: all-up 2/3, but blocked is environmental — the
        // runnable rate is 2/2 = 100%, and the blocked count is surfaced.
        let vs = vec![val("passed"), val("passed"), val("blocked")];
        let (blocked, runnable) = blocked_count_and_runnable_rate(&vs);
        assert_eq!(blocked, 1);
        assert!((runnable - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn runnable_rate_equals_all_up_when_nothing_blocked() {
        let vs = vec![val("passed"), val("failed")];
        let (blocked, runnable) = blocked_count_and_runnable_rate(&vs);
        assert_eq!(blocked, 0);
        assert!((runnable - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn all_blocked_is_zero_runnable_not_a_divide_by_zero() {
        let vs = vec![val("blocked"), val("blocked")];
        let (blocked, runnable) = blocked_count_and_runnable_rate(&vs);
        assert_eq!(blocked, 2);
        assert_eq!(runnable, 0.0);
    }

    use super::super::scoring::{
        build_candidates_from_snapshot, quality_candidates_from_snapshot,
        scored_candidates_from_snapshot, validate_candidates_from_snapshot,
    };

    fn intent(id: &str, lifecycle: &str) -> Intent {
        Intent {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            abstraction_level: "feature".to_string(),
            domain: String::new(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "confirmed".to_string(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: String::new(),
            boundary: String::new(),
            lifecycle: lifecycle.to_string(),
            created_at: "t0".to_string(),
            updated_at: "t0".to_string(),
        }
    }

    fn rel(from: &str, to: &str, status: &str) -> RelatesTo {
        RelatesTo {
            id: format!("rt:{from}:{to}"),
            from_id: from.to_string(),
            to_id: to.to_string(),
            from_name: from.to_string(),
            to_name: to.to_string(),
            inspection_status: status.to_string(),
            criterion: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            priority_score: 0.0,
            notes: String::new(),
        }
    }

    fn snap(intents: Vec<Intent>, relates: Vec<RelatesTo>) -> QuerySnapshot {
        QuerySnapshot::from_parts(
            intents,
            vec![],
            relates,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            None,
        )
    }

    fn phase_of(snapshot: &QuerySnapshot) -> String {
        graph_state_from_snapshot_parts(
            snapshot,
            GraphStateContext {
                meta: None,
                notes: 0,
                transition_cap: 0,
            },
            |_| Ok(0),
            || Ok(0),
        )
        .unwrap()
        .phase
    }

    /// Every phase the compass can route to MUST correspond to a non-empty
    /// `loom next --mode <phase>` queue (the coherence-by-construction invariant
    /// CLAUDE.md states but had no test for). Routing to a phase whose queue is
    /// empty would send an agent to a `loom next` that answers "nothing to do".
    fn queue_nonempty_for_phase(phase: &str, snapshot: &QuerySnapshot) -> bool {
        match phase {
            "fix" => !scored_candidates_from_snapshot(snapshot, "fix").is_empty(),
            "build" => !build_candidates_from_snapshot(snapshot).is_empty(),
            "validate" => !validate_candidates_from_snapshot(snapshot).is_empty(),
            "quality" => !quality_candidates_from_snapshot(snapshot).is_empty(),
            "discovery" => snapshot
                .relates
                .iter()
                .any(|e| e.inspection_status == "uninspected"),
            // seed/ground/incomplete/audit/complete are not `loom next --mode`
            // lanes — they route to other commands and have no queue to check.
            _ => true,
        }
    }

    #[test]
    fn compass_phase_always_has_a_nonempty_queue() {
        // A FAILING relationship is a genuine violation: it routes to `fix`
        // and stays ABOVE build even when a planned intent is waiting.
        let failing = snap(
            vec![
                intent("a", "implemented"),
                intent("b", "implemented"),
                intent("p", "planned"),
            ],
            vec![rel("a", "b", "failing")],
        );
        assert_eq!(phase_of(&failing), "fix", "a failing edge outranks build");
        assert!(queue_nonempty_for_phase("fix", &failing));

        // Stale-only (needs_reverification, no failing, no planned) still routes
        // to `fix` — and the fix queue serves the stale items.
        let stale_only = snap(
            vec![intent("a", "implemented"), intent("b", "implemented")],
            vec![rel("a", "b", "needs_reverification")],
        );
        assert_eq!(phase_of(&stale_only), "fix");
        assert!(queue_nonempty_for_phase("fix", &stale_only));

        // The reorder under test: stale RELATES_TO (optional horizontal grid)
        // must NOT bury a `planned` build item (binding vertical spine). With a
        // stale edge AND a planned intent and NO failing edge, the compass picks
        // `build`, not `fix`.
        let stale_plus_planned = snap(
            vec![
                intent("a", "implemented"),
                intent("b", "implemented"),
                intent("p", "planned"),
            ],
            vec![rel("a", "b", "needs_reverification")],
        );
        assert_eq!(
            phase_of(&stale_plus_planned),
            "build",
            "planned build outranks optional stale re-verification"
        );
        assert!(queue_nonempty_for_phase("build", &stale_plus_planned));
    }
}
