//! Count / coverage / centrality statistics for `loom status` and `loom report`.

use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

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
    /// How firmly to take `next_action`: "directive" = a failure or binding
    /// gap the agent should just act on, "recommended" = discretionary work it
    /// may reasonably sequence against the other open lanes. Drives the
    /// `→ Next:` vs `→ Recommended:` verb so the single pointer signals its own
    /// confidence instead of always reading as a command.
    pub next_kind: String,
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
    let candidate_pair_total = intents * (intents - 1) / 2 - hier_pairs.len() as i64;
    let explored_pairs = CoverageAxis {
        covered: inspected_pairs.len() as i64,
        total: candidate_pair_total,
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

    // Each arm declares its `next_kind`: "directive" when the phase is a failure
    // or a binding vertical gap the agent should just act on; "recommended" when
    // the work is discretionary improvement it may sequence against other open
    // lanes (the verb the `→ Next:` / `→ Recommended:` line picks from).
    let (phase, next_kind, next_action) = if intents == 0 {
        ("seed", "directive", "Empty graph — capture the user's head first: `loom guide --mode seed` teaches the interview; land answers with `loom intent add --level system …`.".to_string())
    } else if needs_change > 0 {
        ("build", "directive", format!("{needs_change} intent(s) need changes (known issues/refactor): `loom next --mode build`."))
    } else if rt_failing > 0 {
        (
            "fix",
            "directive",
            format!("{rt_failing} relationship(s) FAILING — `loom next --mode fix` (resolve violations at root cause)."),
        )
    } else if planned > 0 {
        (
            "build",
            "recommended",
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
            "recommended",
            format!("{rt_needs_rev} stale edge(s) to re-verify (optional grid upkeep after a code change) — `loom next --mode fix`."),
        )
    } else if !vc.multi_parent.is_empty() || vc.cycle {
        ("incomplete", "directive", "HIERARCHY isn't a tree (an intent has >1 parent, or there's a cycle): run `loom doctor`, then fix the edges.".to_string())
    } else if !vc.unrealized_leaves.is_empty() {
        ("ground", "directive", format!(
            "{} leaf intent(s) implemented but not grounded — `loom edge implement` them, or decompose with `loom edge hierarchy` (see `loom report`).",
            vc.unrealized_leaves.len()
        ))
    } else if !vc.unreached_codefiles.is_empty() {
        ("ground", "directive", format!(
            "{} CodeFile(s) reached by no intent — see which with `loom coverage`, then ground them (`loom edge implement`) or `loom ignore` them.",
            vc.unreached_codefiles.len()
        ))
    } else if v_failing_in_backlog {
        ("validate", "directive", "A validation is failing — `loom next --mode validate` (fix the code, then re-run `loom validate <intent>`).".to_string())
    } else if !validate_backlog.is_empty() {
        (
            "validate",
            "recommended",
            if v_no_proof > 0 {
                format!("{} intent(s) need proof (missing or unrun validations): `loom next --mode validate`.", validate_backlog.len())
            } else {
                "Run pending validations: `loom next --mode validate`.".to_string()
            },
        )
    } else if g_failing > 0 {
        ("quality", "directive", "A quality gate is failing — `loom next --mode quality`, refactor to meet it, then record `loom rule verdict`.".to_string())
    } else if g_needs_rev > 0 {
        ("quality", "recommended", "Quality green went stale (the code under a passing verdict changed) — `loom next --mode quality`, re-inspect, re-earn with `loom rule verdict`.".to_string())
    } else if g_uninspected > 0 {
        ("quality", "recommended", "Quality gates applied but unchecked — `loom next --mode quality`, inspect, then earn green with `loom rule verdict`.".to_string())
    } else if unmeasured_queue > 0 {
        ("quality", "recommended", format!(
            "{unmeasured_queue} rule×intent pair(s) never measured — `loom next --mode quality`. One command resolves each: `loom rule verdict` creates the edge with the verdict (a verdict at component altitude covers descendants; independent = measured, doesn't apply)."
        ))
    } else if rules_count == 0 && nc.intents_with_code > 0 {
        ("quality", "recommended", "The normative plane is EMPTY — no measuring sticks, so 360° coverage can't be earned. `loom detect` recommends packs for this repo; seed with `loom rule seed iso5055` (baseline, applies to any code), then measure at the highest honest altitude.".to_string())
    } else if rt_uninspected > 0 || unexplored_pairs > 0 {
        ("discovery", "recommended", format!(
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
            ("audit", "recommended", format!(
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
            ("complete", "recommended", msg)
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
        next_kind: next_kind.to_string(),
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GateReasonCount {
    pub reason: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BlockedValidationSummary {
    /// Current blocked validation nodes. This is the actionable work count.
    pub validations: i64,
    /// Uninspected VALIDATES edges affected by those blocked validation nodes.
    /// Diagnostic only: one validation can prove many intents.
    pub affected_proof_edges: i64,
    /// Blocked validation nodes grouped by the best read-side reason we can infer
    /// from their recorded blocker text and validation metadata.
    pub by_reason: Vec<GateReasonCount>,
}

pub const GATE_REASON_MANUAL_ACCEPTANCE: &str = "manual_acceptance";
pub const GATE_REASON_MISSING_ARTIFACT: &str = "missing_artifact";
pub const GATE_REASON_MISSING_SECRET: &str = "missing_secret";
pub const GATE_REASON_MISSING_LOCAL_ENV: &str = "missing_local_env";
pub const GATE_REASON_EXTERNAL_SERVICE_REQUIRED: &str = "external_service_required";
pub const GATE_REASON_STALE_BLOCKER_NEEDS_AUDIT: &str = "stale_blocker_needs_audit";

impl BlockedValidationSummary {
    pub fn autonomous_validation_count(&self) -> i64 {
        self.by_reason
            .iter()
            .filter(|item| blocked_gate_reason_is_autonomous(&item.reason))
            .map(|item| item.count)
            .sum()
    }

    pub fn human_validation_count(&self) -> i64 {
        self.validations - self.autonomous_validation_count()
    }

    pub fn human_gate_reasons(&self) -> Vec<GateReasonCount> {
        self.by_reason
            .iter()
            .filter(|item| !blocked_gate_reason_is_autonomous(&item.reason))
            .cloned()
            .collect()
    }
}

pub fn blocked_gate_reason_is_autonomous(reason: &str) -> bool {
    matches!(
        reason,
        GATE_REASON_MISSING_ARTIFACT | GATE_REASON_STALE_BLOCKER_NEEDS_AUDIT
    )
}

pub fn uninspected_outside_queues_from_snapshot(
    snapshot: &QuerySnapshot,
) -> UninspectedOutsideQueues {
    let current_blocked = current_blocked_validation_ids(snapshot);
    let lifecycle_by_intent: HashMap<&str, &str> = snapshot
        .intents
        .iter()
        .map(|i| (i.id.as_str(), i.lifecycle.as_str()))
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
                && current_blocked.contains(e.validation_id.as_str())
                && lifecycle_by_intent.get(e.intent_id.as_str()).copied() != Some("deferred")
        })
        .count() as i64;
    UninspectedOutsideQueues {
        implements,
        blocked_validations,
    }
}

pub fn blocked_validation_summary_from_snapshot(
    snapshot: &QuerySnapshot,
) -> BlockedValidationSummary {
    let current_blocked = current_blocked_validation_ids(snapshot);
    if current_blocked.is_empty() {
        return BlockedValidationSummary {
            validations: 0,
            affected_proof_edges: 0,
            by_reason: Vec::new(),
        };
    }
    let lifecycle_by_intent: HashMap<&str, &str> = snapshot
        .intents
        .iter()
        .map(|i| (i.id.as_str(), i.lifecycle.as_str()))
        .collect();
    let mut notes_by_validation: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut affected_proof_edges = 0;
    for edge in &snapshot.validates {
        let validation_id = edge.validation_id.as_str();
        if edge.inspection_status == "uninspected"
            && current_blocked.contains(validation_id)
            && lifecycle_by_intent.get(edge.intent_id.as_str()).copied() != Some("deferred")
        {
            affected_proof_edges += 1;
            notes_by_validation
                .entry(validation_id)
                .or_default()
                .push(edge.notes.as_str());
        }
    }

    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for validation in &snapshot.validations {
        if !current_blocked.contains(validation.id.as_str()) {
            continue;
        }
        let edge_notes = notes_by_validation
            .get(validation.id.as_str())
            .map(|notes| notes.join("\n"))
            .unwrap_or_default();
        let reason = classify_blocked_gate_reason(validation, &edge_notes);
        *counts.entry(reason.to_string()).or_insert(0) += 1;
    }
    let by_reason = counts
        .into_iter()
        .map(|(reason, count)| GateReasonCount { reason, count })
        .collect();
    BlockedValidationSummary {
        validations: current_blocked.len() as i64,
        affected_proof_edges,
        by_reason,
    }
}

fn classify_blocked_gate_reason(validation: &Validation, edge_notes: &str) -> &'static str {
    let haystack = format!(
        "{}\n{}\n{}\n{}\n{}",
        validation.name,
        validation.description,
        validation.validation_type,
        validation.command,
        edge_notes
    )
    .to_ascii_lowercase();
    if validation.validation_type == "manual_check"
        || contains_any(
            &haystack,
            &["manual", "acceptance", "human", "visual", "sign-off"],
        )
    {
        return GATE_REASON_MANUAL_ACCEPTANCE;
    }
    if contains_any(
        &haystack,
        &[
            "missing saga spec",
            "missing spec",
            "missing file",
            "missing artifact",
            "no such file",
            "not found",
        ],
    ) {
        return GATE_REASON_MISSING_ARTIFACT;
    }
    if contains_any(
        &haystack,
        &[
            "secret",
            "credential",
            "token",
            "password",
            "private key",
            "api key",
        ],
    ) {
        return GATE_REASON_MISSING_SECRET;
    }
    if contains_any(
        &haystack,
        &[
            "missing env",
            "env value",
            "environment variable",
            ".env",
            "local env",
        ],
    ) {
        return GATE_REASON_MISSING_LOCAL_ENV;
    }
    if contains_any(
        &haystack,
        &[
            "external",
            "live target",
            "service",
            "staging",
            "base_url",
            "target_url",
            "server",
            "docker",
            "compose",
        ],
    ) {
        return GATE_REASON_EXTERNAL_SERVICE_REQUIRED;
    }
    if contains_any(
        &haystack,
        &["stale", "misclassified", "reclassify", "audit blocker"],
    ) {
        return GATE_REASON_STALE_BLOCKER_NEEDS_AUDIT;
    }
    GATE_REASON_EXTERNAL_SERVICE_REQUIRED
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
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
    let raw_uninspected = *by_status.get("uninspected").unwrap_or(&0);
    let uninspected =
        (raw_uninspected - noncurrent_uninspected_validation_edges_from_snapshot(snapshot)).max(0);
    let passing = *by_status.get("passing").unwrap_or(&0);
    let failing = *by_status.get("failing").unwrap_or(&0);
    let independent = *by_status.get("independent").unwrap_or(&0);
    let needs_reverification = *by_status.get("needs_reverification").unwrap_or(&0);

    let pass_rate = validation_pass_rate_from_snapshot(snapshot);
    let (blocked_validations, validation_pass_rate_runnable) =
        blocked_count_and_runnable_rate_from_snapshot(snapshot);
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

fn noncurrent_uninspected_validation_edges_from_snapshot(snapshot: &QuerySnapshot) -> i64 {
    let current_blocked = current_blocked_validation_ids(snapshot);
    let validation_result: HashMap<&str, &str> = snapshot
        .validations
        .iter()
        .map(|v| (v.id.as_str(), v.last_result.as_str()))
        .collect();
    snapshot
        .validates
        .iter()
        .filter(|edge| {
            edge.inspection_status == "uninspected"
                && validation_result.get(edge.validation_id.as_str()).copied() == Some("blocked")
                && !current_blocked.contains(edge.validation_id.as_str())
        })
        .count() as i64
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

    // Path coverage (structural twin of the happy_path_only smell): a parent
    // whose children include a family's TRIGGER aspect but not its required
    // siblings. Shares super::smells::ASPECT_FAMILIES so the report and the
    // gating smell never disagree on which states a parent owes (behavioral
    // happy→sad/fallback, UI populated→empty/error). This is the structural
    // check (aspect present?), not the realized+grounded+proven gating bar.
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
        for (trigger, required) in super::smells::ASPECT_FAMILIES {
            if !child_aspects.contains(*trigger) {
                continue;
            }
            let missing: Vec<&str> = required
                .iter()
                .copied()
                .filter(|a| !child_aspects.contains(*a))
                .collect();
            if !missing.is_empty() {
                gaps.push(format!(
                    "Group under '{}' has a '{trigger}' aspect but no {} sibling.",
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

/// Current blocked proof count plus runnable pass rate. A blocked validation
/// attached only to deferred intents is future work, not a human-gated item for
/// the current lane; it stays recorded on the validation node but is suppressed
/// from status/closeout debt until the target intent becomes current again.
///
/// The runnable rate is still passed / (total - all blocked): the health of
/// proofs that CAN run, so blocked sagas and deferred acceptance checks do not
/// make the headline rate read as failures.
pub fn blocked_count_and_runnable_rate_from_snapshot(snapshot: &QuerySnapshot) -> (i64, f64) {
    let blocked = current_blocked_validation_ids(snapshot).len();
    let all_blocked = snapshot
        .validations
        .iter()
        .filter(|v| v.last_result == "blocked")
        .count();
    let validations = &snapshot.validations;
    let passed = validations
        .iter()
        .filter(|v| v.last_result == "passed")
        .count();
    let runnable = validations.len() - all_blocked;
    let rate = if runnable > 0 {
        passed as f64 / runnable as f64
    } else {
        0.0
    };
    (blocked as i64, rate)
}

pub fn current_blocked_validation_ids(snapshot: &QuerySnapshot) -> std::collections::HashSet<&str> {
    let blocked_ids: std::collections::HashSet<&str> = snapshot
        .validations
        .iter()
        .filter(|v| v.last_result == "blocked")
        .map(|v| v.id.as_str())
        .collect();
    if blocked_ids.is_empty() {
        return std::collections::HashSet::new();
    }

    let lifecycle_by_intent: HashMap<&str, &str> = snapshot
        .intents
        .iter()
        .map(|i| (i.id.as_str(), i.lifecycle.as_str()))
        .collect();
    let mut linked_blocked = std::collections::HashSet::new();
    let mut current = std::collections::HashSet::new();
    for edge in &snapshot.validates {
        let validation_id = edge.validation_id.as_str();
        if !blocked_ids.contains(validation_id) {
            continue;
        }
        linked_blocked.insert(validation_id);
        if lifecycle_by_intent.get(edge.intent_id.as_str()).copied() != Some("deferred") {
            current.insert(validation_id);
        }
    }

    // A blocked validation with no VALIDATES edge is malformed enough to be
    // current operator debt: there is no deferred target proving it is parked.
    for validation_id in blocked_ids {
        if !linked_blocked.contains(validation_id) {
            current.insert(validation_id);
        }
    }
    current
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
    use crate::types::{CodeFile, Implements, ValidatesEdge, Validation};

    fn val(id: &str, result: &str) -> Validation {
        Validation {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            validation_type: "manual_check".into(),
            command: String::new(),
            last_run: String::new(),
            last_result: result.into(),
        }
    }

    fn validates(validation_id: &str, intent_id: &str, status: &str) -> ValidatesEdge {
        ValidatesEdge {
            id: format!("val:{validation_id}:{intent_id}"),
            validation_id: validation_id.to_string(),
            intent_id: intent_id.to_string(),
            validation_name: validation_id.to_string(),
            intent_name: intent_id.to_string(),
            created_at: String::new(),
            inspection_status: status.to_string(),
            notes: String::new(),
        }
    }

    fn validation_snapshot(
        intents: Vec<Intent>,
        validations: Vec<Validation>,
        validates: Vec<ValidatesEdge>,
    ) -> QuerySnapshot {
        QuerySnapshot::from_parts(
            intents,
            vec![],
            vec![],
            vec![],
            vec![],
            validates,
            validations,
            vec![],
            vec![],
            None,
        )
    }

    #[test]
    fn runnable_rate_excludes_blocked_from_the_denominator() {
        // 2 passed, 1 blocked: all-up 2/3, but blocked is environmental — the
        // runnable rate is 2/2 = 100%, and the blocked count is surfaced.
        let snapshot = validation_snapshot(
            vec![intent("a", "implemented")],
            vec![
                val("p1", "passed"),
                val("p2", "passed"),
                val("b", "blocked"),
            ],
            vec![validates("b", "a", "uninspected")],
        );
        let (blocked, runnable) = blocked_count_and_runnable_rate_from_snapshot(&snapshot);
        assert_eq!(blocked, 1);
        assert!((runnable - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn runnable_rate_equals_all_up_when_nothing_blocked() {
        let snapshot =
            validation_snapshot(vec![], vec![val("p", "passed"), val("f", "failed")], vec![]);
        let (blocked, runnable) = blocked_count_and_runnable_rate_from_snapshot(&snapshot);
        assert_eq!(blocked, 0);
        assert!((runnable - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn all_blocked_is_zero_runnable_not_a_divide_by_zero() {
        let snapshot = validation_snapshot(
            vec![intent("a", "implemented"), intent("b", "implemented")],
            vec![val("b1", "blocked"), val("b2", "blocked")],
            vec![
                validates("b1", "a", "uninspected"),
                validates("b2", "b", "uninspected"),
            ],
        );
        let (blocked, runnable) = blocked_count_and_runnable_rate_from_snapshot(&snapshot);
        assert_eq!(blocked, 2);
        assert_eq!(runnable, 0.0);
    }

    #[test]
    fn deferred_blocked_validation_is_not_current_human_gate() {
        let snapshot = validation_snapshot(
            vec![intent("done", "implemented"), intent("future", "deferred")],
            vec![val("passed", "passed"), val("future-proof", "blocked")],
            vec![validates("future-proof", "future", "uninspected")],
        );
        let (blocked, runnable) = blocked_count_and_runnable_rate_from_snapshot(&snapshot);
        let outside = uninspected_outside_queues_from_snapshot(&snapshot);
        let report = status_report_from_snapshot(&snapshot);
        assert_eq!(blocked, 0);
        assert_eq!(outside.blocked_validations, 0);
        assert_eq!(report.uninspected_edges, 0);
        assert!((runnable - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn mixed_deferred_and_current_blocked_validation_stays_current() {
        let snapshot = validation_snapshot(
            vec![
                intent("current", "implemented"),
                intent("future", "deferred"),
            ],
            vec![val("mixed", "blocked")],
            vec![
                validates("mixed", "future", "uninspected"),
                validates("mixed", "current", "uninspected"),
            ],
        );
        let (blocked, runnable) = blocked_count_and_runnable_rate_from_snapshot(&snapshot);
        let outside = uninspected_outside_queues_from_snapshot(&snapshot);
        let report = status_report_from_snapshot(&snapshot);
        assert_eq!(blocked, 1);
        assert_eq!(outside.blocked_validations, 1);
        assert_eq!(report.uninspected_edges, 2);
        assert_eq!(runnable, 0.0);
    }

    #[test]
    fn blocked_validation_summary_leads_with_validation_objects_not_edges() {
        let snapshot = validation_snapshot(
            vec![
                intent("a", "implemented"),
                intent("b", "implemented"),
                intent("future", "deferred"),
            ],
            vec![val("blocked-saga", "blocked")],
            vec![
                validates("blocked-saga", "a", "uninspected"),
                validates("blocked-saga", "b", "uninspected"),
                validates("blocked-saga", "future", "uninspected"),
            ],
        );
        let summary = blocked_validation_summary_from_snapshot(&snapshot);
        assert_eq!(summary.validations, 1);
        assert_eq!(summary.affected_proof_edges, 2);
        assert_eq!(
            summary.by_reason,
            vec![GateReasonCount {
                reason: "manual_acceptance".to_string(),
                count: 1,
            }]
        );
    }

    use super::super::scoring::{
        build_candidates_from_snapshot, quality_candidates_from_snapshot, ripple_bump_by_intent,
        scored_candidates_from_snapshot, unexplored_pairs_scored_from_snapshot,
        validate_candidates_from_snapshot, DiscoveryClassFilter, RIPPLE_BUMP_HOP2,
        RIPPLE_BUMP_HOP3,
    };

    fn intent(id: &str, lifecycle: &str) -> Intent {
        Intent {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            criterion: String::new(),
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
            kinds: Vec::new(),
            stable: false,
            discovery_class: String::new(),
            discovery_signals: Vec::new(),
            discovery_centrality: Default::default(),
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

    fn codefile(path: &str, imports: Vec<&str>) -> CodeFile {
        CodeFile {
            id: format!("cf:{path}"),
            path: path.to_string(),
            language: "rust".to_string(),
            last_modified: String::new(),
            imports: imports.into_iter().map(str::to_string).collect(),
            symbols: Vec::new(),
            symbol_facts: Vec::new(),
            content_hash: String::new(),
        }
    }

    fn implements(intent_id: &str, path: &str) -> Implements {
        Implements {
            id: format!("imp:{intent_id}:{path}"),
            intent_id: intent_id.to_string(),
            codefile_id: format!("cf:{path}"),
            intent_name: intent_id.to_string(),
            codefile_path: path.to_string(),
            inspection_status: "passing".to_string(),
            criterion: String::new(),
            confidence: 1.0,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            locator: String::new(),
            notes: String::new(),
            created_at: String::new(),
        }
    }

    fn snap_with_code(
        intents: Vec<Intent>,
        relates: Vec<RelatesTo>,
        implements: Vec<Implements>,
        codefiles: Vec<CodeFile>,
    ) -> QuerySnapshot {
        QuerySnapshot::from_parts(
            intents,
            vec![],
            relates,
            vec![],
            vec![],
            vec![],
            vec![],
            implements,
            codefiles,
            None,
        )
    }

    fn gs_of(snapshot: &QuerySnapshot) -> GraphState {
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
    }

    #[test]
    fn unexplored_shared_file_pair_is_suspected_coupling() {
        let snapshot = snap_with_code(
            vec![intent("a", "implemented"), intent("b", "implemented")],
            vec![],
            vec![
                implements("a", "src/shared.rs"),
                implements("b", "src/shared.rs"),
            ],
            vec![codefile("src/shared.rs", vec![])],
        );

        let scored = unexplored_pairs_scored_from_snapshot(
            &snapshot,
            DiscoveryClassFilter::SuspectedCoupling,
        )
        .unwrap();

        assert_eq!(scored.len(), 1);
        let edge = &scored[0].0;
        assert_eq!(edge.discovery_class, "suspected_coupling");
        assert!(edge
            .discovery_signals
            .iter()
            .any(|s| s.kind == "shared_file" && s.detail == "src/shared.rs"));
    }

    #[test]
    fn unexplored_import_pair_is_suspected_coupling() {
        let snapshot = snap_with_code(
            vec![intent("a", "implemented"), intent("b", "implemented")],
            vec![],
            vec![implements("a", "src/a.rs"), implements("b", "src/b.rs")],
            vec![
                codefile("src/a.rs", vec!["src/b.rs"]),
                codefile("src/b.rs", vec![]),
            ],
        );

        let scored = unexplored_pairs_scored_from_snapshot(
            &snapshot,
            DiscoveryClassFilter::SuspectedCoupling,
        )
        .unwrap();

        assert_eq!(scored.len(), 1);
        let edge = &scored[0].0;
        assert_eq!(edge.discovery_class, "suspected_coupling");
        assert!(edge
            .discovery_signals
            .iter()
            .any(|s| s.kind == "import_link"));
    }

    #[test]
    fn unexplored_same_domain_pair_is_suspected_coupling() {
        let mut a = intent("a", "implemented");
        a.domain = "db".to_string();
        let mut b = intent("b", "implemented");
        b.domain = "db".to_string();
        let snapshot = snap(vec![a, b], vec![]);

        let scored = unexplored_pairs_scored_from_snapshot(
            &snapshot,
            DiscoveryClassFilter::SuspectedCoupling,
        )
        .unwrap();

        assert_eq!(scored.len(), 1);
        let edge = &scored[0].0;
        assert_eq!(edge.discovery_class, "suspected_coupling");
        assert!(edge
            .discovery_signals
            .iter()
            .any(|s| s.kind == "same_domain" && s.detail == "db"));
    }

    #[test]
    fn centrality_only_pairs_route_to_impact_map_not_default_discovery() {
        let snapshot = snap(
            vec![intent("a", "implemented"), intent("b", "implemented")],
            vec![],
        );

        let default = unexplored_pairs_scored_from_snapshot(
            &snapshot,
            DiscoveryClassFilter::SuspectedCoupling,
        )
        .unwrap();
        assert!(default.is_empty());

        let impact =
            unexplored_pairs_scored_from_snapshot(&snapshot, DiscoveryClassFilter::ImpactMap)
                .unwrap();
        assert_eq!(impact.len(), 1);
        let edge = &impact[0].0;
        assert_eq!(edge.discovery_class, "impact_map");
        assert!(edge.discovery_signals.is_empty());
        assert!(edge.notes.contains("structural centrality only"));

        let all =
            unexplored_pairs_scored_from_snapshot(&snapshot, DiscoveryClassFilter::All).unwrap();
        assert_eq!(all.len(), 1);
    }

    fn phase_of(snapshot: &QuerySnapshot) -> String {
        gs_of(snapshot).phase
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

    #[test]
    fn compass_marks_directive_vs_recommended() {
        // A failing edge is a violation — the agent should just act: directive.
        let failing = snap(
            vec![intent("a", "implemented"), intent("b", "implemented")],
            vec![rel("a", "b", "failing")],
        );
        assert_eq!(gs_of(&failing).next_kind, "directive");

        // Building a planned intent is discretionary construction the agent may
        // sequence against other lanes: recommended (the "your call" verb).
        let planned = snap(vec![intent("p", "planned")], vec![]);
        let gs = gs_of(&planned);
        assert_eq!(gs.phase, "build");
        assert_eq!(gs.next_kind, "recommended");

        // Stale-only re-verification is optional grid upkeep: recommended.
        let stale_only = snap(
            vec![intent("a", "implemented"), intent("b", "implemented")],
            vec![rel("a", "b", "needs_reverification")],
        );
        assert_eq!(gs_of(&stale_only).next_kind, "recommended");
    }

    #[test]
    fn betweenness_lets_a_low_degree_chokepoint_outrank_a_high_degree_clique() {
        // A 5-clique {c1..c5} (every pair adjacent → betweenness 0) plus a
        // bridge c1—m—z hanging off it. `m` is a low-degree chokepoint: every
        // path from `z` into the clique routes through it, so it has the highest
        // betweenness in the graph; `c2..c5` are high-degree but bridge nothing.
        let mut intents = Vec::new();
        for id in ["c1", "c2", "c3", "c4", "c5", "m", "z"] {
            intents.push(intent(id, "implemented"));
        }
        let clique = ["c1", "c2", "c3", "c4", "c5"];
        let mut relates = Vec::new();
        for i in 0..clique.len() {
            for j in (i + 1)..clique.len() {
                relates.push(rel(clique[i], clique[j], "uninspected"));
            }
        }
        relates.push(rel("c1", "m", "uninspected")); // bridge into the clique
        relates.push(rel("m", "z", "uninspected")); // the pendant beyond the bridge
        let snapshot = snap(intents, relates);

        // The bridge edge c1—m has a SMALLER degree sum than the clique edge
        // c2—c3 (7 vs 8), so on degree alone it would rank lower.
        let deg = &snapshot.degrees;
        let bridge_degree = deg["c1"] + deg["m"];
        let clique_degree = deg["c2"] + deg["c3"];
        assert!(
            bridge_degree < clique_degree,
            "by degree alone the bridge edge loses: {bridge_degree} vs {clique_degree}"
        );

        // But scoring adds bridge centrality, so the chokepoint edge wins.
        let scored = scored_candidates_from_snapshot(&snapshot, "discovery");
        let pos = |id: &str| scored.iter().position(|(e, _)| e.id == id).unwrap();
        let bridge_pos = pos("rt:c1:m");
        let clique_pos = pos("rt:c2:c3");
        assert!(
            bridge_pos < clique_pos,
            "betweenness must rank the low-degree chokepoint edge above the high-degree clique edge: \
             c1—m at {bridge_pos}, c2—c3 at {clique_pos}"
        );
        let score_of = |id: &str| scored.iter().find(|(e, _)| e.id == id).unwrap().1;
        assert!(score_of("rt:c1:m") > score_of("rt:c2:c3"));
    }

    #[test]
    fn ripple_bump_decays_with_distance_from_the_stale_frontier() {
        // Chain a—b—c—d—e with the a—b edge stale (needs_reverification). The
        // frontier is {a,b}; c is one hop from it (two from the change), d two
        // hops, e beyond the graded radius.
        let intents = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|id| intent(id, "implemented"))
            .collect();
        let relates = vec![
            rel("a", "b", "needs_reverification"),
            rel("b", "c", "uninspected"),
            rel("c", "d", "uninspected"),
            rel("d", "e", "uninspected"),
        ];
        let snapshot = snap(intents, relates);
        let bump = ripple_bump_by_intent(&snapshot);

        assert_eq!(
            bump.get("c"),
            Some(&RIPPLE_BUMP_HOP2),
            "two hops from change"
        );
        assert_eq!(
            bump.get("d"),
            Some(&RIPPLE_BUMP_HOP3),
            "three hops from change"
        );
        assert!(
            !bump.contains_key("e"),
            "beyond the graded radius — no bump"
        );
        // The frontier itself is already flipped/urgent and gets NO bump.
        assert!(!bump.contains_key("a") && !bump.contains_key("b"));
    }

    #[test]
    fn ripple_bump_empty_without_stale_edges() {
        // Nothing needs_reverification → no frontier → no ripple, scoring
        // identical to a freshly-synced graph.
        let intents = ["a", "b", "c"]
            .iter()
            .map(|id| intent(id, "implemented"))
            .collect();
        let relates = vec![rel("a", "b", "uninspected"), rel("b", "c", "passing")];
        let snapshot = snap(intents, relates);
        assert!(ripple_bump_by_intent(&snapshot).is_empty());
    }

    #[test]
    fn ripple_elevates_an_edge_near_a_stale_region() {
        // Two disjoint triangles (each a clique → zero betweenness, every node
        // degree 2). A pendant `x` is wired to `a` with a stale edge, putting
        // triangle T1={a,b,c} next to the stale frontier and T2={d,e,f} far from
        // it. The b—c and e—f edges have identical degree and (zero) betweenness,
        // so any ranking difference is the graded ripple alone.
        let intents = ["a", "b", "c", "d", "e", "f", "x"]
            .iter()
            .map(|id| intent(id, "implemented"))
            .collect();
        let relates = vec![
            rel("a", "b", "uninspected"),
            rel("a", "c", "uninspected"),
            rel("b", "c", "uninspected"),
            rel("d", "e", "uninspected"),
            rel("d", "f", "uninspected"),
            rel("e", "f", "uninspected"),
            rel("a", "x", "needs_reverification"), // the stale frontier sits on T1
        ];
        let snapshot = snap(intents, relates);

        let deg = &snapshot.degrees;
        assert_eq!(
            deg["b"] + deg["c"],
            deg["e"] + deg["f"],
            "equal degree sums"
        );

        let scored = scored_candidates_from_snapshot(&snapshot, "discovery");
        let score_of = |id: &str| scored.iter().find(|(e, _)| e.id == id).unwrap().1;
        assert!(
            score_of("rt:b:c") > score_of("rt:e:f"),
            "the edge near the stale region must rank above the equally-shaped far edge"
        );
    }

    #[test]
    fn no_bridges_leaves_scoring_on_pure_degree() {
        // A clique has zero betweenness everywhere → the bridge term vanishes
        // and edges order by degree+urgency exactly as before the feature.
        let intents = vec![
            intent("a", "implemented"),
            intent("b", "implemented"),
            intent("c", "implemented"),
        ];
        let relates = vec![
            rel("a", "b", "uninspected"),
            rel("a", "c", "uninspected"),
            rel("b", "c", "uninspected"),
        ];
        let snapshot = snap(intents, relates);
        assert!(
            snapshot.betweenness().values().all(|&b| b == 0.0),
            "a triangle has no bridges"
        );
        // All three edges have identical degree sums (2+2) and status → equal
        // scores, no betweenness perturbation.
        let scored = scored_candidates_from_snapshot(&snapshot, "discovery");
        let first = scored[0].1;
        assert!(scored.iter().all(|(_, s)| (*s - first).abs() < 1e-9));
    }

    #[test]
    fn deferred_intents_are_excluded_from_the_build_queue() {
        let s = snap(
            vec![intent("p", "planned"), intent("d", "deferred")],
            vec![],
        );
        let build = build_candidates_from_snapshot(&s);
        let ids: Vec<&str> = build.iter().map(|b| b.intent.id.as_str()).collect();
        assert!(ids.contains(&"p"), "planned work is queued");
        assert!(
            !ids.contains(&"d"),
            "a deferred (parked) intent never enters the build queue"
        );
    }

    #[test]
    fn a_deferred_child_does_not_block_parent_rollup() {
        // A planned parent whose children are implemented OR deferred rolls up —
        // the parked child is not pending work.
        let intents = vec![
            intent("parent", "planned"),
            intent("done", "implemented"),
            intent("parked", "deferred"),
        ];
        let hierarchy = vec![
            ("parent".to_string(), "done".to_string()),
            ("parent".to_string(), "parked".to_string()),
        ];
        let s = QuerySnapshot::from_parts(
            intents,
            hierarchy,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            None,
        );
        let build = build_candidates_from_snapshot(&s);
        let parent = build
            .iter()
            .find(|b| b.intent.id == "parent")
            .expect("the parent surfaces as a roll-up candidate");
        assert!(parent.rollup, "a deferred child must not block the roll-up");
        assert!(
            !build.iter().any(|b| b.intent.id == "parked"),
            "the deferred child itself is not queued"
        );
    }
}
