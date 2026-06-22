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
#[derive(Debug, Clone, Serialize, Default)]
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
#[derive(Debug, Clone, Serialize, Default)]
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
    /// Proof quality ceiling — executed proof: implemented leaves proven by a
    /// passed RUNNABLE validation (validation_type test/assertion/benchmark/saga
    /// with a non-empty command). `proven_leaves.covered` ==
    /// `proven_executed_leaves.covered` + `proven_asserted_leaves.covered`.
    pub proven_executed_leaves: CoverageAxis,
    /// Proof quality floor — asserted proof: implemented leaves proven ONLY by
    /// hand-marked acceptance (manual_check, or any validation with an empty
    /// command). `loom validation mark --result passed` stamps a pass without
    /// executing, and the graph records it identically to a run-pass (it sets
    /// last_run too), so the honest discriminator is the validation's SHAPE, not
    /// last_run. These leaves are PROVEN but not EXECUTED-PROVEN.
    pub proven_asserted_leaves: CoverageAxis,
}

/// Compact "pulse" of the graph — cheap situational awareness + a recommended
/// next action (the compass). Returned in `--json` and rendered as a one-line
/// footer by the orientation commands.
#[derive(Debug, Clone, Serialize, Default)]
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
    /// The horizontal axis: every intent pair has an inspected RELATES_TO edge
    /// (none uninspected, stale, or unexplored). NOT required for the vertical
    /// spine milestone, but REQUIRED for `phase=complete` / `fully_proven` —
    /// the cascade gates `complete` on `unexplored_pairs == 0 && rt_uninspected == 0`.
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
    mut disk_integrity_issues: impl FnMut(&QuerySnapshot) -> Result<usize>,
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

    // The AUTHORITATIVE count: every active intent pair with no RELATES_TO edge
    // (hierarchy pairs excluded — containment is structural). This is the full
    // grid still owed for `phase=complete`. NOTE: `loom next --mode discovery`
    // (default `suspected-coupling`) serves only the high-signal SUBSET, so the
    // count can exceed what the default discovery queue surfaces — `loom edge
    // unexplored` (or `loom next --mode discovery --class all`) retrieves them ALL.
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
    let with_current_code = &snapshot.with_current_code;
    let implemented_leaves: Vec<&Intent> = all_intents
        .iter()
        .filter(|i| i.lifecycle == "implemented" && !is_parent.contains(i.id.as_str()))
        .collect();
    let realized_leaves = CoverageAxis {
        covered: implemented_leaves
            .iter()
            .filter(|i| with_current_code.contains(&i.id))
            .count() as i64,
        total: implemented_leaves.len() as i64,
    };

    let active_ids: std::collections::HashSet<&str> =
        all_intents.iter().map(|i| i.id.as_str()).collect();
    let grounded: std::collections::HashSet<&str> = snapshot
        .implements
        .iter()
        .filter(|edge| {
            active_ids.contains(edge.intent_id.as_str())
                && edge.inspection_status != "needs_reverification"
                && edge.inspection_status != "failing"
        })
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
    // are visible elsewhere; this axis counts earned proof only). Split into
    // EXECUTED vs ASSERTED. The honest discriminator is `last_executed_run`:
    // the executor (`loom validate` / `loom saga run`) stamps it ONLY when it
    // actually ran the command; a hand-mark (`loom validation mark --result
    // passed`) sets `last_run` but never `last_executed_run`. So a
    // command-bearing validation marked passed by hand has `last_run` set but
    // `last_executed_run` empty — it reads ASSERTED, not EXECUTED. This closes
    // the declared-not-executed laundering hole (the single biggest remaining
    // honesty gap): 'proven (exec N)' could once be bought by typing a command
    // + marking it passed; now exec means the command RAN (machine-verified),
    // observed not declared. `last_result=passed` already implies sync hasn't
    // invalidated it (sync flips a stale proof to not_run on code change), so
    // `last_executed_run` non-empty + passed = ran AND still current.
    let validation_by_id: std::collections::HashMap<&str, &Validation> = snapshot
        .validations
        .iter()
        .map(|v| (v.id.as_str(), v))
        .collect();
    let mut proven_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut executed_intent_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for edge in snapshot.validates.iter().filter(|edge| {
        validation_result.get(edge.validation_id.as_str()).copied() == Some("passed")
    }) {
        proven_ids.insert(edge.intent_id.as_str());
        if let Some(v) = validation_by_id.get(edge.validation_id.as_str()) {
            // EXECUTED requires the command actually RAN (last_executed_run
            // stamped by the executor) — not merely that a command string
            // exists. command non-empty + type != manual_check are kept as
            // guards (an empty-command or manual_check proof is never executed
            // even if somehow stamped), but last_executed_run is the load-bearing
            // discriminator.
            // G2: EXECUTED additionally requires the executor to have OBSERVED
            // the runner ASSERT (discrimination_status == "discriminating"). An
            // exit-0 run that asserted nothing is `ran_inert` and falls to the
            // ASSERTED tier — exit-0 alone can no longer mint EXECUTED.
            if !v.command.is_empty()
                && v.validation_type != "manual_check"
                && !v.last_executed_run.is_empty()
                && v.discrimination_status == "discriminating"
            {
                executed_intent_ids.insert(edge.intent_id.as_str());
            }
        }
    }
    let proven_leaves = CoverageAxis {
        covered: implemented_leaves
            .iter()
            .filter(|i| proven_ids.contains(i.id.as_str()))
            .count() as i64,
        total: implemented_leaves.len() as i64,
    };
    let proven_executed_leaves = CoverageAxis {
        covered: implemented_leaves
            .iter()
            .filter(|i| executed_intent_ids.contains(i.id.as_str()))
            .count() as i64,
        total: implemented_leaves.len() as i64,
    };
    // Asserted-only = proven but with NO executed-type pass (proven entirely by
    // hand-marked acceptance). Invariant: proven == executed + asserted-only.
    let proven_asserted_leaves = CoverageAxis {
        covered: implemented_leaves
            .iter()
            .filter(|i| {
                proven_ids.contains(i.id.as_str()) && !executed_intent_ids.contains(i.id.as_str())
            })
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
        proven_executed_leaves,
        proven_asserted_leaves,
    };
    let unmeasured_queue = nc.queue.len();

    // Each arm declares its `next_kind`: "directive" when the phase is a failure
    // or a binding vertical gap the agent should just act on; "recommended" when
    // the work is discretionary improvement it may sequence against other open
    // lanes (the verb the `→ Next:` / `→ Recommended:` line picks from).
    let (phase, next_kind, next_action) = if intents == 0 {
        ("seed", "directive", "Empty graph — SEED the full surface, not a sketch: `loom seed --inbox` ingests every doc + source file into the inbox to triage (empty repo → a vision prompt). Then `loom inbox triage` decomposes each into intents. `loom guide --mode seed` teaches the loop.".to_string())
    } else if needs_change > 0 {
        ("build", "directive", format!("{needs_change} intent(s) need changes (known issues/refactor): `loom next --mode build`."))
    } else if !vc.unremoved_leaves.is_empty() {
        ("build", "directive", format!("{} intent(s) marked for removal still have code — delete it (cleanup is done by absence): `loom next --mode build`.", vc.unremoved_leaves.len()))
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
    } else {
        // The audit gate — the binding structural gate, checked BEFORE the
        // horizontal tail. Open smells (godfiles, oversized functions,
        // undeclared coupling) are structural problems that gate phase=complete
        // and rank ABOVE stale-grid re-verification and discovery in the cascade
        // ORDER. But stale RELATES_TO and unexplored pairs are ALSO required for
        // phase=complete (the horizontal grid is part of full-green) — they are
        // simply checked after audit, not optional. The
        // false-green hole this closes: stale edges (recommended) used to
        // route to `fix` before the audit gate was reached, deferring open
        // findings indefinitely behind "audit: deferred while phase=fix
        // keeps another lane active."
        //
        // The FIRST sub-check is map-vs-territory: files on disk the graph
        // doesn't account for (unmapped real files, drifted content, phantom
        // registrations). This is the false-green hole — green used to TRUST
        // the declared graph and only TELL you to "confirm with `loom coverage`",
        // so a file with no intent laundered into a clean compass. Now it GATES:
        // the map must match the territory. Computed lazily (only graphs that
        // cleared every other gate pay for the on-disk walk + content-hash
        // pass), and the caller supplies the count so the pure-graph layer
        // stays disk-free.
        let disk_issues = disk_integrity_issues(snapshot)?;
        if disk_issues > 0 {
            ("audit", "directive", format!(
                "{disk_issues} file(s) on disk the graph doesn't account for (unmapped, drifted, or missing) — the map must match the territory before green: `loom coverage` to see them, `loom sync` to re-hash drifted files, `loom codefile add` + `loom edge implement` to map, or `loom ignore add <glob> --reason …` to exclude."
            ))
        } else {
            // Open smells are unadjudicated suspicions; green means every one
            // was ANSWERED (structurally fixed, or refuted via its remedy — an
            // `independent` verdict / vocab merge / decision note counts
            // exactly as much as a fix). Computed lazily: only graphs that
            // cleared every other gate pay for the O(N²) scan.
            let open_findings = open_findings(snapshot)?;
            if open_findings > 0 {
                ("audit", "recommended", format!(
                    "{open_findings} open finding(s) — `loom smells`: resolve each via its remedy, ONE at a time after reading its code. A decision note must give a finding-specific reason (the decomposition considered + why it's wrong HERE), not a reused template — loom rejects vacuous/rubber-stamped rulings. Green requires 0 open findings."
                ))
            } else if rt_needs_rev > 0 {
                // Stale RELATES_TO is the OPTIONAL horizontal grid —
                // re-verification ranks BELOW the audit gate (binding for
                // green) but above discovery (pure exploration). `rt_failing`
                // (a genuine violation) is handled above and stays urgent.
                // Both branches route to the same `loom next --mode fix` queue,
                // which serves failing|needs_reverification; here
                // rt_failing == 0, so it serves the stale items.
                (
                    "fix",
                    "recommended",
                    format!("{rt_needs_rev} stale edge(s) to re-verify (optional grid upkeep after a code change) — `loom next --mode fix`."),
                )
            } else if rt_uninspected > 0 || unexplored_pairs > 0 {
                ("discovery", "recommended", format!(
                    "Vertical spine complete ✓ — but full-green (`phase=complete`/`fully_proven`) REQUIRES the N×N grid explored: {unexplored_pairs} unexplored pair(s) + {rt_uninspected} uninspected edge(s) left. `loom edge unexplored` lists them (with pre-filled commands); `loom next --mode discovery` serves the high-signal ones. Verdict `independent` at the highest honest altitude — a component verdict covers its descendants."
                ))
            } else {
                // The pre-decision plane never gates green (a proposal is not
                // state of the world — see Hypothesis), but it must not rot
                // invisibly either: pending proofs are disclosed at the one
                // message every agent reads. Computed lazily with the same
                // justification as the smells scan above.
                let proposed = proposed_hypotheses()?;
                let mut msg = "Vertically complete ✓, horizontally explored ✓, disk reconciled ✓ (nothing on disk unmapped/drifted/missing), 0 open findings ✓ — confirm the roll-up with `loom report`. Then make the green DURABLE: run `loom export` and commit the graph with the code, re-run it after every graph change (`loom export --check` verifies; CI wiring is optional extra hardening), and keep running `loom sync` after code changes (maintenance mode).".to_string();
                if proposed > 0 {
                    msg.push_str(&format!(
                        " Pre-decision plane: {proposed} proposed hypothesis(es) await proof — optional, never gates green: `loom next --mode prove`."
                    ));
                }
                ("complete", "recommended", msg)
            }
        }
    };

    // Note-hygiene nudge: when the log is heavy enough to drag the read path,
    // teach the lever. The cap auto-bounds via sync, so this fires mainly for
    // graphs with the cap OFF or not yet swept — not normal capped operation
    // (the threshold sits well above a healthy capped graph). Cheap: reuses the
    // `notes` count already computed; no per-note materialization.
    //
    // loom-dx #3: the cap>0 branch used to point at `loom note prune
    // --transitions` as the remedy, but that command compacts ONLY low-signal
    // transition churn — confirm/decision/justification notes are real memory
    // it leaves alone. A heavy log of legitimate memory returns "Nothing to
    // prune", so the old advisory implied a remedy that wasn't there and nagged
    // forever. Now it teaches the distinction: prune helps only if transition
    // churn is the bulk; otherwise the log is legitimately heavy (not a bug).
    const NOTE_HEAVY: i64 = 5000;
    let note_hygiene = if notes > NOTE_HEAVY {
        let cap = context.transition_cap;
        if cap == 0 {
            format!("{notes} notes — the transition log is UNCAPPED and slows every command. `loom note prune --set-cap 20` bounds it (sync then holds it there).")
        } else {
            format!(
                "{notes} notes on the read path. `loom note prune --transitions` compacts ONLY low-signal transition churn — confirm/decision/justification notes are real memory it leaves alone, so \"Nothing to prune\" means the bulk is legitimate (not a bug). The cap ({cap}/target) bounds future churn via sync; run `loom sync` (or `loom note prune --transitions`) only if transition churn is the bulk, else the log is just legitimately heavy."
            )
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

/// The `fully_proven` terminal badge — "production ready" in the only sense loom
/// can WITNESS. It is a STRONGER read LAYERED OVER the existing `phase=complete`,
/// NOT a rival state: one definition of "done" stays the cascade; this is its
/// proof-quality CEILING. It holds iff phase is complete AND every strengthening
/// gate below holds, and returns the UNMET gates as `reasons` so an unmet badge
/// is loud + falsifiable, never a vacuous green. Pure over the snapshot + the
/// already-computed open smells (re-derivable from the committed graph) — no
/// agent-controlled denominator, and (post-G2) no exit-0 mint.
///
/// It deliberately does NOT claim deploy-fitness (security / perf / ops) — loom
/// can't witness that; the name asserts proofs RAN, RESOLVE, are LOCAL, and the
/// denominators are non-trivial.
pub fn fully_proven_from_state(
    gs: &GraphState,
    snapshot: &QuerySnapshot,
    open_smells: &[crate::db::queries::smells::Smell],
    entrypoint: &CoverageAxis,
    inbox_untriaged: usize,
) -> (bool, Vec<String>) {
    let mut reasons: Vec<String> = Vec::new();

    // G_INBOX — the decomposition ledger: every file `loom seed --inbox` enumerated
    // must be PROCESSED into intents. Un-triaged items are pieces of the system not
    // yet decomposed — you can't be production-ready over an un-processed surface.
    // Vacuous (0) for graphs that never used the inbox-seed flow, so no false block.
    if inbox_untriaged > 0 {
        reasons.push(format!(
            "{inbox_untriaged} inbox item(s) un-triaged — repo pieces not yet decomposed (`loom inbox triage`)"
        ));
    }

    // G0 — base: one "done" definition; the badge is its ceiling, not a rival.
    if gs.phase != "complete" {
        reasons.push(format!(
            "phase is '{}', not 'complete' — drive the open lane (`loom next`)",
            gs.phase
        ));
    }
    // G1 — EXECUTED floor with an honest denominator: every CURRENT-realized leaf
    // is proven by an EXECUTED (discriminating) proof, not merely asserted. The
    // denominator is realized_leaves, so a leaf cannot drop out by being unproven.
    let exec = &gs.coverage.proven_executed_leaves;
    let realized = &gs.coverage.realized_leaves;
    if realized.covered == 0 {
        reasons.push("no realized leaves to prove".to_string());
    } else if exec.covered < realized.covered {
        reasons.push(format!(
            "{} of {} realized leaves are not EXECUTED-proven (asserted-only or unproven) — write and run a discriminating proof",
            realized.covered - exec.covered,
            realized.covered
        ));
    }
    // G4 — proof-locality: no OPEN nonlocal_proof finding (a proof that does not
    // exercise its grounded code). Advisory for phase=complete; BINDING here.
    let nonlocal = open_smells
        .iter()
        .filter(|s| s.kind == "nonlocal_proof")
        .count();
    if nonlocal > 0 {
        reasons.push(format!(
            "{nonlocal} proof(s) do not exercise their grounded code (nonlocal_proof) — see `loom smells`"
        ));
    }
    // G6 — zero unverified autonomous-inference debt: no record still bearing the
    // `llm:auto` provenance tier (drafted by the autonomous mode, never verified).
    // Always 0 until that mode ships; future-proofs the badge against laundering.
    let auto = snapshot
        .relates
        .iter()
        .filter(|e| e.inspected_by == "llm:auto")
        .count()
        + snapshot
            .governs
            .iter()
            .filter(|e| e.inspected_by == "llm:auto")
            .count();
    if auto > 0 {
        reasons.push(format!(
            "{auto} unverified autonomous inference(s) remain — re-verify or discard before production"
        ));
    }
    // G7 — ENTRYPOINT COMPREHENSIVENESS (mechanical, FORCED): every externally
    // public symbol must be grounded / accepted / adjudicated. Anchored to the
    // real surface (symbol_accountability's `required`), so a thin graph can't
    // under-claim coverage. (Boundary comprehensiveness is the other mechanical
    // dimension; it needs the raw external-import surface, which the snapshot
    // doesn't persist, so it lives in `loom complete`'s disk scan, not here.)
    if entrypoint.covered < entrypoint.total {
        reasons.push(format!(
            "{} public symbol(s) are unowned (no intent/grounding) — `loom complete` / `loom coverage`",
            entrypoint.total - entrypoint.covered
        ));
    }

    (reasons.is_empty(), reasons)
}

#[cfg(test)]
mod tests;
