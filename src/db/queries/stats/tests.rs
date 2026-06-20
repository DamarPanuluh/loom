use super::*;
use crate::types::{CodeFile, Governs, Implements, QualityRule, ValidatesEdge, Validation};

fn val(id: &str, result: &str) -> Validation {
    Validation {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        validation_type: "manual_check".into(),
        command: String::new(),
        last_run: String::new(),
        last_result: result.into(),
        last_executed_run: String::new(),
    }
}

/// A command-bearing, runnable validation (type=test, command set) — the
/// shape that USED to read as EXECUTED on the proven axis by static shape
/// alone. `last_executed_run` is the new discriminator: empty = hand-marked
/// (ASSERTED), non-empty = the executor ran it (EXECUTED).
fn cmd_val(id: &str, result: &str, last_executed_run: &str) -> Validation {
    Validation {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        validation_type: "test".into(),
        command: "cargo test".into(),
        last_run: "2026-06-19T00:00:00Z".into(),
        last_result: result.into(),
        last_executed_run: last_executed_run.into(),
    }
}

/// honesty-next / execute-runnable-validations (the headliner): the proven
/// axis splits EXECUTED (the executor RAN the command — last_executed_run
/// non-empty) from ASSERTED (hand-marked — last_executed_run empty). A
/// command-bearing validation marked passed by HAND must read ASSERTED, not
/// EXECUTED — closing the declared-not-executed laundering hole (you can no
/// longer buy 'proven (exec N)' by typing a command + marking it passed).
#[test]
fn proven_executed_requires_last_executed_run_not_just_a_command() {
    let leaf = intent("leaf", "implemented");
    let imp = implements("leaf", "src/x.rs");

    // Case 1: command-bearing validation, passed, but last_executed_run EMPTY
    // — a hand-mark (`loom validation mark --result passed`) on a proof that
    // never ran. Proven? yes (it passed). Executed? NO — asserted only.
    let snap_hand = QuerySnapshot::from_parts(
        vec![leaf.clone()],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![validates("vh", "leaf", "passing")],
        vec![cmd_val("vh", "passed", "")],
        vec![imp.clone()],
        vec![],
        None,
    );
    let gs_hand = gs_of(&snap_hand);
    assert_eq!(
        gs_hand.coverage.proven_leaves.covered, 1,
        "a passed validation proves the leaf"
    );
    assert_eq!(
        gs_hand.coverage.proven_executed_leaves.covered, 0,
        "a hand-marked command-bearing proof is ASSERTED, not EXECUTED — last_executed_run empty"
    );
    assert_eq!(
        gs_hand.coverage.proven_asserted_leaves.covered, 1,
        "the hand-mark lands in the asserted-only bucket"
    );

    // Case 2: same shape, but the EXECUTOR ran it (last_executed_run stamped).
    // Now it reads EXECUTED — machine-verified, not declared.
    let snap_exec = QuerySnapshot::from_parts(
        vec![leaf],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![validates("ve", "leaf", "passing")],
        vec![cmd_val("ve", "passed", "2026-06-19T00:00:00Z")],
        vec![imp],
        vec![],
        None,
    );
    let gs_exec = gs_of(&snap_exec);
    assert_eq!(
        gs_exec.coverage.proven_executed_leaves.covered, 1,
        "an executor-stamped last_executed_run is EXECUTED (the command ran)"
    );
    assert_eq!(
        gs_exec.coverage.proven_asserted_leaves.covered, 0,
        "an executed proof is not in the asserted-only bucket"
    );
    // Invariant from the field doc: proven == executed + asserted-only.
    assert_eq!(
        gs_exec.coverage.proven_leaves.covered,
        gs_exec.coverage.proven_executed_leaves.covered
            + gs_exec.coverage.proven_asserted_leaves.covered,
        "proven decomposes into executed + asserted-only"
    );
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

use crate::db::queries::scoring::{
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
        |_| Ok(0),
    )
    .unwrap()
}

#[test]
fn note_hygiene_heavy_log_reframes_prune_as_conditional_not_a_false_remedy() {
    // loom-dx #3: a heavy note log must NOT imply `loom note prune
    // --transitions` will fix it — that command compacts ONLY transition
    // churn, so a log of legitimate memory (confirm/decision/justification)
    // returns "Nothing to prune". The old advisory pointed at a remedy that
    // wasn't there and nagged forever; the reframe teaches the distinction.
    let snapshot = snap(vec![], vec![]);
    let gs = graph_state_from_snapshot_parts(
        &snapshot,
        GraphStateContext {
            meta: None,
            notes: 6000,
            transition_cap: 20,
        },
        |_| Ok(0),
        || Ok(0),
        |_| Ok(0),
    )
    .unwrap();
    let h = &gs.note_hygiene;
    assert!(h.contains("6000 notes"), "names the count: {h}");
    assert!(
        h.contains("ONLY low-signal transition churn"),
        "prune is qualified to transition churn only (not a blanket remedy): {h}"
    );
    assert!(
        h.contains("Nothing to prune"),
        "teaches that Nothing-to-prune on legitimate memory is expected, not a bug: {h}"
    );
    assert!(
        h.contains("legitimately heavy"),
        "names the legitimate-memory case so the driver stops chasing a missing remedy: {h}"
    );
}

#[test]
fn note_hygiene_uncapped_log_keeps_set_cap_remedy() {
    // cap==0 is always actionable (bound the transition log), so that branch
    // keeps its direct remedy — the reframe only retrains the cap>0 branch.
    let snapshot = snap(vec![], vec![]);
    let gs = graph_state_from_snapshot_parts(
        &snapshot,
        GraphStateContext {
            meta: None,
            notes: 6000,
            transition_cap: 0,
        },
        |_| Ok(0),
        || Ok(0),
        |_| Ok(0),
    )
    .unwrap();
    let h = &gs.note_hygiene;
    assert!(
        h.contains("UNCAPPED") && h.contains("--set-cap"),
        "uncapped keeps the set-cap remedy: {h}"
    );
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

/// `gs_of` with a controllable disk-integrity count — the audit-gate else
/// branch (the only place the disk check runs) takes a 5th closure.
fn gs_of_with_disk(snapshot: &QuerySnapshot, disk_issues: usize) -> GraphState {
    graph_state_from_snapshot_parts(
        snapshot,
        GraphStateContext {
            meta: None,
            notes: 0,
            transition_cap: 0,
        },
        |_| Ok(0),
        || Ok(0),
        move |_| Ok(disk_issues),
    )
    .unwrap()
}

/// A snapshot that clears EVERY gate up to the audit-gate else branch — the
/// only place the disk check lives. A system root (not grounded, not a leaf)
/// with one grounded implemented leaf, one rule measured at the root (the
/// GOVERNS verdict covers the leaf descendant), and a passed validation on
/// the leaf (clears the validate backlog). No RELATES_TO, so the grid is
/// fully explored. With disk reconciled this reads `complete`.
fn complete_reaching_snapshot() -> QuerySnapshot {
    let mut sys = intent("sys", "implemented");
    sys.abstraction_level = "system".to_string();
    let feat = intent("feat", "implemented");
    QuerySnapshot::from_parts(
        vec![sys, feat],
        vec![("sys".to_string(), "feat".to_string())],
        vec![],
        vec![Governs {
            id: "gov:r1:sys".to_string(),
            rule_id: "r1".to_string(),
            intent_id: "sys".to_string(),
            rule_name: "r1".to_string(),
            intent_name: "sys".to_string(),
            inspection_status: "passing".to_string(),
            criterion: String::new(),
            confidence: 1.0,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            notes: String::new(),
        }],
        vec![QualityRule {
            id: "r1".to_string(),
            name: "r1".to_string(),
            description: String::new(),
            detection_logic: String::new(),
            severity: "low".to_string(),
            kind: String::new(),
            inspection_effort: String::new(),
        }],
        vec![validates("v1", "feat", "passing")],
        vec![val("v1", "passed")],
        vec![implements("feat", "src/a.rs")],
        vec![codefile("src/a.rs", vec![])],
        None,
    )
}

/// A snapshot that clears EVERY binding gate (grounded, realized, validated,
/// measured) and has ONLY a stale RELATES_TO edge. Used to verify that stale
/// edges route to `fix` from within the audit gate — after all binding gates
/// clear — not from above them (where they used to bury grounding gaps,
/// validation debt, and the audit gate itself).
fn stale_clearing_snapshot() -> QuerySnapshot {
    let mut sys = intent("sys", "implemented");
    sys.abstraction_level = "system".to_string();
    let feat = intent("feat", "implemented");
    QuerySnapshot::from_parts(
        vec![sys, feat],
        vec![("sys".to_string(), "feat".to_string())],
        vec![rel("sys", "feat", "needs_reverification")],
        vec![Governs {
            id: "gov:r1:sys".to_string(),
            rule_id: "r1".to_string(),
            intent_id: "sys".to_string(),
            rule_name: "r1".to_string(),
            intent_name: "sys".to_string(),
            inspection_status: "passing".to_string(),
            criterion: String::new(),
            confidence: 1.0,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            notes: String::new(),
        }],
        vec![QualityRule {
            id: "r1".to_string(),
            name: "r1".to_string(),
            description: String::new(),
            detection_logic: String::new(),
            severity: "low".to_string(),
            kind: String::new(),
            inspection_effort: String::new(),
        }],
        vec![validates("v1", "feat", "passing")],
        vec![val("v1", "passed")],
        vec![implements("feat", "src/a.rs")],
        vec![codefile("src/a.rs", vec![])],
        None,
    )
}

// FALSE-GREEN [map-vs-territory-reconcile-on-read]: a graph that clears
// every other gate must NOT read `complete` while the disk has files the
// graph doesn't account for. Green used to trust the declared graph and
// only TELL you to "confirm with `loom coverage`" — an unmapped real file
// laundered into a clean compass. Now the disk gate blocks green (audit,
// directive) until the map matches the territory.
#[test]
fn map_vs_territory_blocks_green_when_disk_unaccounted() {
    let snap = complete_reaching_snapshot();
    // Sanity: with the map matching the territory, the graph IS complete.
    let green = gs_of_with_disk(&snap, 0);
    assert_eq!(
        green.phase, "complete",
        "with disk reconciled the phase should be complete: {green:?}"
    );
    // The false-green hole: unmapped/drifted/missing files drop it to audit.
    let red = gs_of_with_disk(&snap, 3);
    assert_eq!(
        red.phase, "audit",
        "disk-unaccounted files must block green (audit, not complete): {red:?}"
    );
    assert_eq!(
        red.next_kind, "directive",
        "map≠territory is a directive the agent should just act on, not a suggestion: {red:?}"
    );
    assert!(
        red.next_action.contains("map must match the territory"),
        "the action should name the gate: {}",
        red.next_action
    );
}

// FALSE-GREEN [compass-must-not-overstate]: the leaf spine can be sound
// (vc.complete — every implemented LEAF realized + every CodeFile reached)
// while BROADER completeness gaps remain (a confirmed NON-leaf intent not
// grounded, a missing validation, a path-coverage hole). report's headline ✓
// is scope-labeled to LEAF and reconciled with these gaps — a bare
// "✓ every leaf realized" directly above "N Completeness Gaps" reads as
// "completeness done". This locks the invariant the qualified headline
// relies on: vc.complete does NOT imply gaps.is_empty().
#[test]
fn leaf_spine_sound_does_not_imply_no_completeness_gaps() {
    use crate::db::queries::completeness::vertical_completeness_from_snapshot;
    let mut sys = intent("sys", "implemented");
    sys.abstraction_level = "system".to_string();
    let feat = intent("feat", "implemented");
    // sys is a confirmed NON-leaf (has a child) and NOT grounded → a broader
    // gap. feat is a grounded leaf → the leaf spine is sound.
    let snapshot = QuerySnapshot::from_parts(
        vec![sys, feat],
        vec![("sys".to_string(), "feat".to_string())],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![implements("feat", "src/a.rs")],
        vec![codefile("src/a.rs", vec![])],
        None,
    );
    let vc = vertical_completeness_from_snapshot(&snapshot);
    assert!(vc.complete, "leaf spine is sound: {vc:?}");
    let gaps = completeness_gaps_from_snapshot(&snapshot);
    assert!(
        gaps.iter()
            .any(|g| g.contains("'sys'") && g.contains("not grounded")),
        "a confirmed non-leaf intent not grounded is a broader gap the leaf-spine ✓ must not bury: {gaps:?}"
    );
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
    // to `fix` — but from WITHIN the audit gate, after all binding gates
    // clear. A bare `snap` (ungrounded leaves) would hit `ground` first,
    // which is the correct precedence: grounding gaps are binding, stale
    // edges are optional. Use a snapshot that clears every binding gate so
    // stale edges are the only issue.
    let stale_only = stale_clearing_snapshot();
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
    // Uses a clearing snapshot so stale edges are the only issue (a bare
    // `snap` has ungrounded leaves → `ground` directive fires first).
    let stale_only = stale_clearing_snapshot();
    assert_eq!(gs_of(&stale_only).next_kind, "recommended");
}

// FALSE-GREEN [audit-gate-not-deferred-by-stale-edges]: the audit gate (open
// smells — godfiles, oversized functions, undeclared coupling) is a binding
// gate for phase=complete. Stale RELATES_TO is optional grid upkeep. The
// audit gate MUST rank above stale edges: a graph with BOTH open findings
// AND stale edges routes to `audit`, not `fix`. The old ordering put
// `rt_needs_rev` above the audit gate, deferring open findings indefinitely
// behind "audit: deferred while phase=fix keeps another lane active" — a
// false-green where 84 open smell findings (including 3 godfiles) hid
// behind 285 stale edges.
#[test]
fn audit_gate_outranks_stale_edges() {
    let snapshot = stale_clearing_snapshot();
    // With 0 open findings: stale edges route to `fix` (recommended).
    assert_eq!(
        phase_of(&snapshot),
        "fix",
        "stale edges with a clean audit gate route to fix"
    );
    // With open findings: the audit gate intercepts — phase is `audit`,
    // not `fix`. The stale edges are still visible in `other open lanes`
    // but don't bury the structural findings.
    let gs = graph_state_from_snapshot_parts(
        &snapshot,
        GraphStateContext {
            meta: None,
            notes: 0,
            transition_cap: 0,
        },
        |_| Ok(84), // open findings exist
        || Ok(0),
        |_| Ok(0), // disk reconciled
    )
    .unwrap();
    assert_eq!(
        gs.phase, "audit",
        "open findings must route to audit even when stale edges exist: {gs:?}"
    );
    assert_eq!(
        gs.next_kind, "recommended",
        "open findings (not disk issues) are recommended, not directive: {gs:?}"
    );
    assert!(
        gs.next_action.contains("open finding"),
        "the action should name the audit findings: {}",
        gs.next_action
    );
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
fn bridge_ranking_inverts_when_direct_cross_cluster_edge_removes_chokepoint() {
    fn dense_bridge_snapshot(with_direct_cross_edge: bool) -> QuerySnapshot {
        let mut intents = Vec::new();
        for id in [
            "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8", "bridge", "b0", "b1",
            "b2", "b3", "b4", "b5", "b6", "b7",
        ] {
            intents.push(intent(id, "implemented"));
        }

        let mut relates = Vec::new();
        let cluster_a = ["a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8"];
        let cluster_b = ["b0", "b1", "b2", "b3", "b4", "b5", "b6", "b7"];
        for cluster in [cluster_a.as_slice(), cluster_b.as_slice()] {
            for i in 0..cluster.len() {
                for j in (i + 1)..cluster.len() {
                    relates.push(rel(cluster[i], cluster[j], "uninspected"));
                }
            }
        }
        relates.push(rel("a0", "bridge", "uninspected"));
        relates.push(rel("bridge", "b0", "uninspected"));
        if with_direct_cross_edge {
            relates.push(rel("a0", "b0", "uninspected"));
        }

        snap(intents, relates)
    }

    let bridged = dense_bridge_snapshot(false);
    let bridged_scores = scored_candidates_from_snapshot(&bridged, "discovery");
    let score_of = |scored: &[(RelatesTo, f64)], id: &str| {
        scored
            .iter()
            .find(|(edge, _)| edge.id == id)
            .map(|(_, score)| *score)
            .unwrap()
    };
    let position_of = |scored: &[(RelatesTo, f64)], id: &str| {
        scored.iter().position(|(edge, _)| edge.id == id).unwrap()
    };

    assert_eq!(
        bridged.degrees["bridge"], 2,
        "the connector intent is low-degree: one edge into each dense cluster"
    );
    assert!(
        bridged.degrees["a0"] + bridged.degrees["bridge"]
            < bridged.degrees["a1"] + bridged.degrees["a2"],
        "without bridge centrality, the a0—bridge edge loses to the high-degree clique edge"
    );
    assert!(
        bridged.betweenness().get("bridge").copied().unwrap_or(0.0) > 0.0,
        "bridge routes the only path between the two clusters"
    );
    assert_eq!(
        bridged.betweenness().get("a1").copied().unwrap_or(0.0),
        0.0,
        "a1 is high-degree inside the clique, but not a bridge"
    );
    assert!(
        position_of(&bridged_scores, "rt:a0:bridge") < position_of(&bridged_scores, "rt:a1:a2"),
        "betweenness must lift the low-degree bridge-incident edge above the higher-degree clique edge"
    );
    assert!(
        score_of(&bridged_scores, "rt:a0:bridge") > score_of(&bridged_scores, "rt:a1:a2"),
        "ranking inversion must be score-driven, not insertion-order noise"
    );

    let bypassed = dense_bridge_snapshot(true);
    let bypassed_scores = scored_candidates_from_snapshot(&bypassed, "discovery");
    assert_eq!(
        bypassed.betweenness().get("bridge").copied().unwrap_or(0.0),
        0.0,
        "the direct a0—b0 edge removes bridge's chokepoint role"
    );
    assert!(
        score_of(&bypassed_scores, "rt:a0:bridge") < score_of(&bypassed_scores, "rt:a1:a2"),
        "once bridge is no longer on cross-cluster shortest paths, the former bridge edge drops below the high-degree clique edge"
    );
    assert!(
        position_of(&bypassed_scores, "rt:a0:bridge") > position_of(&bypassed_scores, "rt:a1:a2"),
        "the scorer's sorted priority order must invert after the bridge role is removed"
    );
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
