//! Ring 9 tests — review queue, queue disjointness, unmeasured-pair fallback,
//! self-contained packets, truth-gap criteria, packs, and door/inbox/note CLI.
//!
//! Each test names the externally observable contract it defends. Failure
//! messages are prefixed with the numbered contract so a red run points at the
//! violated behavior.

use loom::lane::Lane;
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use loom::workitem;
use std::path::Path;

mod common;
use common::*;

// ---- shared builders --------------------------------------------------------

/// A root intent marked implemented (eligible for unmeasured-pair fallback).
fn implemented_intent(store: &Store, name: &str) -> loom::model::Node {
    store
        .add_node(
            NodeType::Intent,
            name,
            "one falsifiable behavior",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap()
}

/// A passing governs edge recorded at the given confidence.
fn passing_governs(
    store: &Store,
    rule: &loom::model::Node,
    intent: &loom::model::Node,
    conf: f64,
) -> loom::model::Edge {
    let e = store
        .ensure_edge(EdgeKind::Governs, &rule.id, &intent.id)
        .unwrap();
    store
        .record_verdict(
            &e.id,
            InspectionStatus::Passing,
            "measured",
            "src/x.rs:1",
            conf,
            "llm",
        )
        .unwrap();
    e
}

// ===========================================================================
// 0. QUEUE ROSTER (loom next --mode <m> --all)
// ===========================================================================

#[test]
fn queue_items_returns_full_depth_not_just_the_top() {
    // Contract: `queue_items` lists EVERY item a queue would serve, in priority
    // order, so `loom next --mode <m> --all` shows real depth (not one item like
    // the singular `next`). Entry 0 is exactly what `loom next --mode <m>` serves.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // Three ungrounded implemented intents — all legitimate build work.
    for name in ["alpha behavior", "bravo behavior", "charlie behavior"] {
        implemented_intent(&store, name);
    }

    let roster = workitem::queue_items(&store, Lane::Build).unwrap();
    assert_eq!(
        roster.len(),
        3,
        "the roster lists every build item, not just the top"
    );
    // Matches the queue count reported by `loom status`.
    assert_eq!(
        roster.len(),
        loom::maturity::depths(&store).unwrap().get(Lane::Build),
        "roster depth equals the queue count status reports"
    );
    // Entry 0 is the same target the singular lane serves — the roster is faithful.
    let top = workitem::next(&store, Some(Lane::Build)).unwrap().unwrap();
    assert_eq!(roster[0].target.name, top.target.name);
    // Sorted by name (all same lifecycle rank), so the depth view is stable.
    assert_eq!(roster[0].target.name, "alpha behavior");
    assert_eq!(roster[2].target.name, "charlie behavior");
}

#[test]
fn queue_items_empty_for_a_disabled_lane_on_an_observed_graph() {
    // The build lane is off on an observed graph, so its roster is empty too —
    // `--mode build --all` never lists work the lane would refuse to serve.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), true).unwrap(); // observed
    implemented_intent(&store, "some behavior"); // ungrounded → would be build work
    assert!(
        workitem::queue_items(&store, Lane::Build)
            .unwrap()
            .is_empty(),
        "observed graph: the build roster is empty (lane disabled)"
    );
}

// ===========================================================================
// 1. REVIEW QUEUE
// ===========================================================================

#[test]
fn review_queue_serves_low_confidence_passing_verdict() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    loom::packs::seed(&store, "iso5055").unwrap();
    let rule = store
        .resolve_node(
            "iso5055-sec-no-hardcoded-secrets",
            Some(NodeType::QualityRule),
        )
        .unwrap();
    let intent = implemented_intent(&store, "config loads secrets from env");
    passing_governs(&store, &rule, &intent, 0.4);

    let item = workitem::next(&store, Some(Lane::Review)).unwrap().expect(
        "REVIEW QUEUE: a passing verdict with confidence in (0,0.7) must be served by review mode",
    );

    // Mode string parse works.
    assert_eq!(
        Lane::parse("review"),
        Some(Lane::Review),
        "REVIEW QUEUE: Lane::parse(\"review\") must yield Lane::Review"
    );
    assert_eq!(
        item.mode, "review",
        "REVIEW QUEUE: item.mode must be \"review\""
    );

    // owner_role equals the edge kind's registry owner (governs -> quality).
    assert_eq!(
        item.owner_role,
        loom::registry::spec(EdgeKind::Governs).owner.as_str(),
        "REVIEW QUEUE: review owner_role must equal the edge kind's registry owner (governs -> quality)"
    );

    // reason names the confidence.
    assert!(
        item.reason.contains("0.40") || item.reason.contains("0.4"),
        "REVIEW QUEUE: reason must name the recorded confidence, got: {}",
        item.reason
    );
    assert!(
        item.reason.contains("review") || item.reason.contains("re-inspect"),
        "REVIEW QUEUE: reason must mention review/re-inspection, got: {}",
        item.reason
    );

    // write_back prefilled with BOTH endpoint names single-quoted, no placeholders.
    let wb = &item.prompt_contract.write_back;
    assert!(
        wb.contains(&format!("'{}'", rule.name)),
        "REVIEW QUEUE: write_back must contain the rule endpoint single-quoted, got: {wb}"
    );
    assert!(
        wb.contains(&format!("'{}'", intent.name)),
        "REVIEW QUEUE: write_back must contain the intent endpoint single-quoted, got: {wb}"
    );
    assert!(
        !wb.contains("<a>") && !wb.contains("<b>"),
        "REVIEW QUEUE: write_back must not contain <a>/<b> placeholders, got: {wb}"
    );
}

#[test]
fn review_queue_serves_low_confidence_independent_verdict() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    loom::packs::seed(&store, "iso5055").unwrap();
    let rule = store
        .resolve_node(
            "iso5055-sec-no-hardcoded-secrets",
            Some(NodeType::QualityRule),
        )
        .unwrap();
    let intent = implemented_intent(&store, "secrets from env");
    let e = store
        .ensure_edge(EdgeKind::Governs, &rule.id, &intent.id)
        .unwrap();
    store
        .record_verdict(
            &e.id,
            InspectionStatus::Independent,
            "rule surface absent",
            "inspected: no such surface in the grounded code",
            0.5,
            "llm",
        )
        .unwrap();

    let item = workitem::next(&store, Some(Lane::Review)).unwrap().expect(
        "REVIEW QUEUE: an independent verdict with confidence in (0,0.7) must be served by review",
    );
    assert_eq!(
        item.mode, "review",
        "REVIEW QUEUE: independent low-conf item must be review mode"
    );
}

#[test]
fn review_queue_excludes_high_confidence_and_failing_verdicts() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    loom::packs::seed(&store, "iso5055").unwrap();
    let rule = store
        .resolve_node(
            "iso5055-sec-no-hardcoded-secrets",
            Some(NodeType::QualityRule),
        )
        .unwrap();
    let intent = implemented_intent(&store, "secrets from env");

    // A confidence >= 0.7 passing verdict — must NOT be served by review.
    passing_governs(&store, &rule, &intent, 0.8);
    assert!(
        workitem::next(&store, Some(Lane::Review))
            .unwrap()
            .is_none(),
        "REVIEW QUEUE: a confidence >= 0.7 verdict must not be served by review"
    );

    // A failing verdict — must NOT be served by review (it routes to fix).
    let rule2 = store
        .resolve_node(
            "iso5055-rel-no-unchecked-failure",
            Some(NodeType::QualityRule),
        )
        .unwrap();
    let intent2 = implemented_intent(&store, "error handling");
    let e = store
        .ensure_edge(EdgeKind::Governs, &rule2.id, &intent2.id)
        .unwrap();
    store
        .record_verdict(
            &e.id,
            InspectionStatus::Failing,
            "unhandled failure",
            "src/x.rs:10 — unwrap",
            0.3,
            "llm",
        )
        .unwrap();
    assert!(
        workitem::next(&store, Some(Lane::Review))
            .unwrap()
            .is_none(),
        "REVIEW QUEUE: a failing verdict must not be served by review (it routes to fix)"
    );
}
#[test]
fn review_queue_serves_low_confidence_relates_as_analyzer() {
    // A low-confidence verdict on a Relates edge must route to review with
    // owner_role == the relates registry owner (analyzer), and the write-back
    // must prefill BOTH endpoint names single-quoted (relates uses `edge explore`).
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let from = implemented_intent(&store, "alpha intent");
    let to = implemented_intent(&store, "beta intent");
    let e = store
        .add_edge(EdgeKind::Relates, &from.id, &to.id, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &e.id,
            InspectionStatus::Independent,
            "non-overlap confirmed",
            "inspected: alpha and beta share no behavior",
            0.45,
            "llm",
        )
        .unwrap();

    let item = workitem::next(&store, Some(Lane::Review))
        .unwrap()
        .expect("REVIEW QUEUE: a low-confidence relates verdict must be served by review");
    assert_eq!(
        item.mode, "review",
        "REVIEW QUEUE: relates item must be review mode"
    );
    assert_eq!(
        item.owner_role,
        loom::registry::spec(EdgeKind::Relates).owner.as_str(),
        "REVIEW QUEUE: relates review owner_role must equal the registry owner (analyzer)"
    );
    assert_eq!(
        item.owner_role, "analyzer",
        "REVIEW QUEUE: relates review owner_role must be \"analyzer\""
    );
    let wb = &item.prompt_contract.write_back;
    assert!(
        wb.contains("edge explore"),
        "REVIEW QUEUE: relates write_back must use `edge explore`, got: {wb}"
    );
    assert!(
        wb.contains(&format!("'{}'", from.name)),
        "REVIEW QUEUE: relates write_back must contain the from endpoint single-quoted, got: {wb}"
    );
    assert!(
        wb.contains(&format!("'{}'", to.name)),
        "REVIEW QUEUE: relates write_back must contain the to endpoint single-quoted, got: {wb}"
    );
}

#[test]
fn record_verdict_rejects_placeholder_text_without_partial_write() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let from = implemented_intent(&store, "placeholder gate source intent");
    let to = implemented_intent(&store, "placeholder gate target intent");
    let e = store
        .add_edge(EdgeKind::Relates, &from.id, &to.id, TruthClass::Asserted)
        .unwrap();

    assert!(
        store
            .record_verdict(
                &e.id,
                InspectionStatus::Passing,
                "source behavior reaches target behavior",
                "…",
                0.9,
                "llm",
            )
            .is_err(),
        "HONESTY GATE: placeholder evidence must be rejected"
    );
    let unchanged = store.resolve_edge(&e.id).unwrap();
    assert_eq!(
        unchanged.status,
        InspectionStatus::Uninspected,
        "HONESTY GATE: rejected placeholder evidence must not change edge status"
    );
    assert_eq!(
        unchanged.criterion, "",
        "HONESTY GATE: rejected placeholder evidence must not persist criterion"
    );
    assert_eq!(
        store.verdict_prose(&unchanged.id).unwrap(),
        "",
        "HONESTY GATE: rejected placeholder evidence must not persist evidence"
    );

    assert!(
        store
            .record_verdict(
                &e.id,
                InspectionStatus::Passing,
                "<reason>",
                "inspected the intent graph and found the asserted relation grounded",
                0.9,
                "llm",
            )
            .is_err(),
        "HONESTY GATE: placeholder criterion must be rejected"
    );
    let still_unchanged = store.resolve_edge(&e.id).unwrap();
    assert_eq!(
        still_unchanged.status,
        InspectionStatus::Uninspected,
        "HONESTY GATE: rejected placeholder criterion must not change edge status"
    );
    assert_eq!(
        still_unchanged.criterion, "",
        "HONESTY GATE: rejected placeholder criterion must not persist criterion"
    );
    assert_eq!(
        store.verdict_prose(&still_unchanged.id).unwrap(),
        "",
        "HONESTY GATE: rejected placeholder criterion must not persist evidence"
    );

    let accepted = store
        .record_verdict(
            &e.id,
            InspectionStatus::Passing,
            "source behavior reaches target behavior",
            "inspected command output linked the two intents before truncation …",
            0.9,
            "llm",
        )
        .expect("HONESTY GATE: substantive evidence ending in ellipsis must be accepted");
    assert_eq!(
        accepted.status,
        InspectionStatus::Passing,
        "HONESTY GATE: accepted substantive verdict must be persisted as passing"
    );
    assert_eq!(
        store.verdict_prose(&accepted.id).unwrap(),
        "inspected command output linked the two intents before truncation …",
        "HONESTY GATE: substantive trailing ellipsis must be preserved"
    );
}

#[test]
fn review_queue_low_confidence_count_matches_eligible_edges() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    loom::packs::seed(&store, "iso5055").unwrap();
    let rule = store
        .resolve_node(
            "iso5055-sec-no-hardcoded-secrets",
            Some(NodeType::QualityRule),
        )
        .unwrap();
    let intent = implemented_intent(&store, "secrets from env");

    // No eligible verdicts yet.
    assert_eq!(
        workitem::graph_state(&store).unwrap().low_confidence,
        0,
        "REVIEW QUEUE: low_confidence must be 0 with no eligible verdicts"
    );

    passing_governs(&store, &rule, &intent, 0.3);
    assert_eq!(
        workitem::graph_state(&store).unwrap().low_confidence,
        1,
        "REVIEW QUEUE: low_confidence must count exactly the eligible (0,0.7) passing/independent edges"
    );

    // A second eligible verdict at a different confidence.
    let rule2 = store
        .resolve_node(
            "iso5055-rel-no-unchecked-failure",
            Some(NodeType::QualityRule),
        )
        .unwrap();
    let intent2 = implemented_intent(&store, "error handling");
    passing_governs(&store, &rule2, &intent2, 0.6);
    assert_eq!(
        workitem::graph_state(&store).unwrap().low_confidence,
        2,
        "REVIEW QUEUE: low_confidence must count 2 eligible edges"
    );

    // A >= 0.7 verdict does NOT increment the count.
    let rule3 = store
        .resolve_node("iso5055-sec-no-injection", Some(NodeType::QualityRule))
        .unwrap();
    let intent3 = implemented_intent(&store, "sql safety");
    passing_governs(&store, &rule3, &intent3, 0.9);
    assert_eq!(
        workitem::graph_state(&store).unwrap().low_confidence,
        2,
        "REVIEW QUEUE: a >= 0.7 verdict must not be counted in low_confidence"
    );
}

// ===========================================================================
// 2. QUEUE DISJOINTNESS (fix / quality / validate partition)
// ===========================================================================

/// A hand-authored validation with one passing `validates` edge and one
/// `exercises` edge naming the code entry its proof drives.
fn generic_validation_with_exercises(
    store: &Store,
) -> (loom::model::Node, loom::model::Edge, loom::model::Edge) {
    let target = implemented_intent(store, "the CLI answers with the record");
    let file = codefile(store, "src/behavior.rs");
    let validation = store
        .add_node(
            NodeType::Validation,
            "behavior-end-to-end",
            "drives the CLI and asserts the record",
            "passed",
            serde_json::json!({"type":"test", "command":"cargo test behavior"}),
        )
        .unwrap();
    // The validates edge is left for the validate lane: a passing verdict on
    // it needs a Loom-observed run, which this fixture deliberately has not.
    let validates = store
        .add_edge(
            EdgeKind::Validates,
            &validation.id,
            &target.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let exercises = store
        .add_edge(
            EdgeKind::Exercises,
            &validation.id,
            &file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    (validation, validates, exercises)
}

/// Provenance that was inspected and then invalidated by a drifted locator.
/// Sync can only stale a settled verdict, so the inspection comes first.
fn inspect_then_stale(store: &Store, edge: &loom::model::Edge) {
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "the proof drives this entry point",
            "src/behavior.rs:1",
            0.95,
            "validator",
        )
        .unwrap();
    assert!(
        store
            .stale_edge(&edge.id, "locator 'behavior' no longer resolves")
            .unwrap(),
        "the fixture must reach the state sync produces"
    );
}

#[test]
fn stale_exercises_provenance_becomes_analyze_work_that_edge_verdict_closes() {
    // `exercises` is provenance, not a claim — so an uninspected one is not
    // queued, and proof strength counts it as current. Once sync invalidates
    // it (a drifted locator), it stops counting and NOTHING could settle it:
    // `validation run` writes verdicts on `validates` alone. That silently
    // dropped a proof from S3 to S2 with no lane to drain. Analyze owns it
    // from the moment it goes stale, and the door it names must open.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let (_validation, _validates, exercises) = generic_validation_with_exercises(&store);

    assert!(
        workitem::queue_items(&store, Lane::Analyze)
            .unwrap()
            .iter()
            .all(|entry| entry.target.id != exercises.id),
        "intact provenance is not a claim awaiting inspection"
    );

    inspect_then_stale(&store, &exercises);
    let roster = workitem::queue_items(&store, Lane::Analyze).unwrap();
    assert!(
        roster.iter().any(|entry| entry.target.id == exercises.id),
        "invalidated provenance must be queued somewhere: {roster:#?}"
    );
    assert_eq!(
        Lane::Analyze.depth(&loom::lane::LadderInputs::gather(&store).unwrap()),
        roster.len(),
        "the relationships rung must count exactly what the analyze queue serves"
    );

    let packet = workitem::next(&store, Some(Lane::Analyze))
        .unwrap()
        .expect("the stale exercises edge is servable analyze work");
    assert_eq!(packet.target.id, exercises.id);
    assert_eq!(
        packet.owner_role, "validator",
        "the registry owner gates the write, so the packet must run as it"
    );
    let write_back = packet.prompt_contract.write_back.clone();
    assert!(
        write_back.contains(&format!("loom edge verdict {}", exercises.id)),
        "a validation run settles `validates` only — the packet must name the verdict: {write_back}"
    );
    drop(store);

    // The named door actually opens.
    loom::commands::run(loom::cli::Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: Some(loom::cli::Command::Edge {
            cmd: loom::cli::EdgeCmd::Verdict {
                edge_id: exercises.id.clone(),
                verdict: "ground".into(),
                criterion: "the proof still drives this entry point".into(),
                evidence: "src/behavior.rs:1".into(),
                confidence: 0.95,
            },
        }),
    })
    .expect("edge verdict must close the packet it was told to close");

    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        store.get_edge(&exercises.id).unwrap().unwrap().status,
        InspectionStatus::Passing
    );
    assert!(
        workitem::queue_items(&store, Lane::Analyze)
            .unwrap()
            .iter()
            .all(|entry| entry.target.id != exercises.id),
        "the closed packet must leave the queue"
    );
}

#[test]
fn validator_owned_non_validates_edges_name_the_verdict_not_the_run() {
    // The validator contract belongs to `validates`: re-run the proof. Every
    // other validator-owned kind is an inspection claim, and a validation run
    // records nothing on it — naming the run was a door that never opened,
    // and it passed the closure invariant only because the validation's name
    // appears in the command.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let (validation, validates, exercises) = generic_validation_with_exercises(&store);
    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            "behavior-cli",
            "the CLI under proof",
            "active",
            serde_json::json!({
                "schema": "loom.interface-surface/v1",
                "stable_id": "behavior-cli",
                "title": "Behavior CLI",
                "kind": "cli",
                "identity": "behavior",
                "operations": [{"id":"show","summary":"show","argv":["behavior","show"],"arguments":[],"output":{"format":"json"}}],
            }),
        )
        .unwrap();
    let calls = store
        .add_edge(
            EdgeKind::Calls,
            &validation.id,
            &surface.id,
            TruthClass::Asserted,
        )
        .unwrap();
    inspect_then_stale(&store, &exercises);

    // Drain the lane: every packet must name a verdict on its own target, and
    // both validator-owned claims must appear.
    let mut seen = std::collections::BTreeSet::new();
    while let Some(packet) = workitem::next(&store, Some(Lane::Analyze)).unwrap() {
        if packet.target.kind != "edge" {
            break;
        }
        if [&calls.id, &exercises.id].contains(&&packet.target.id) {
            assert_eq!(packet.owner_role, "validator");
            assert!(
                packet
                    .prompt_contract
                    .write_back
                    .contains(&format!("loom edge verdict {}", packet.target.id)),
                "a validator-owned inspection claim must name the verdict on its own edge: {}",
                packet.prompt_contract.write_back
            );
            seen.insert(packet.target.id.clone());
        }
        store
            .record_verdict(
                &packet.target.id,
                InspectionStatus::Passing,
                "inspected for this test",
                "src/behavior.rs:1",
                0.95,
                "validator",
            )
            .unwrap();
    }
    assert_eq!(
        seen,
        [calls.id.clone(), exercises.id.clone()]
            .into_iter()
            .collect(),
        "both the calls and the stale exercises claim must be served"
    );

    // `validates` keeps the run — that IS the door that settles it.
    let _ = validates;
    let packet = workitem::next(&store, Some(Lane::Validate))
        .unwrap()
        .expect("a stale validates edge is validate work");
    assert!(
        packet
            .prompt_contract
            .write_back
            .contains("loom validation"),
        "re-running the proof is what settles a validates edge: {}",
        packet.prompt_contract.write_back
    );
}

#[test]
fn queue_modes_never_serve_the_same_edge() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    loom::packs::seed(&store, "iso5055").unwrap();

    // (a) one failing governs edge.
    let rule_fail = store
        .resolve_node(
            "iso5055-sec-no-hardcoded-secrets",
            Some(NodeType::QualityRule),
        )
        .unwrap();
    let intent_fail = implemented_intent(&store, "failing-secrets");
    let gov_fail = store
        .ensure_edge(EdgeKind::Governs, &rule_fail.id, &intent_fail.id)
        .unwrap();
    store
        .record_verdict(
            &gov_fail.id,
            InspectionStatus::Failing,
            "hardcoded secret",
            "src/a.rs:1",
            0.9,
            "llm",
        )
        .unwrap();

    // (b) one needs_reverification governs edge.
    let rule_stale_gov = store
        .resolve_node("iso5055-sec-no-injection", Some(NodeType::QualityRule))
        .unwrap();
    let intent_stale_gov = implemented_intent(&store, "stale-gov-intent");
    let gov_stale = store
        .ensure_edge(EdgeKind::Governs, &rule_stale_gov.id, &intent_stale_gov.id)
        .unwrap();
    store
        .record_verdict(
            &gov_stale.id,
            InspectionStatus::Passing,
            "ok",
            "src/b.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    store
        .stale_edge(&gov_stale.id, "content hash of src/b.rs changed")
        .unwrap();

    // (c) one needs_reverification relates edge.
    let intent_a = implemented_intent(&store, "relates-from");
    let intent_b = implemented_intent(&store, "relates-to");
    let rel_stale = store
        .add_edge(
            EdgeKind::Relates,
            &intent_a.id,
            &intent_b.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &rel_stale.id,
            InspectionStatus::Passing,
            "ground",
            "src/c.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    store
        .stale_edge(&rel_stale.id, "content hash of src/c.rs changed")
        .unwrap();

    // (d) one uninspected validates edge.
    let val = store
        .add_node(
            NodeType::Validation,
            "proof-x",
            "",
            "not_run",
            serde_json::json!({ "command": "true" }),
        )
        .unwrap();
    let intent_val = implemented_intent(&store, "validated-intent");
    let val_edge = store
        .add_edge(
            EdgeKind::Validates,
            &val.id,
            &intent_val.id,
            TruthClass::Asserted,
        )
        .unwrap();

    // fix serves the failing governs edge (role fixer).
    let fix_item = workitem::next(&store, Some(Lane::Fix))
        .unwrap()
        .expect("QUEUE DISJOINTNESS: fix must serve the failing governs edge");
    assert_eq!(
        fix_item.mode, "fix",
        "QUEUE DISJOINTNESS: fix item mode must be fix"
    );
    assert_eq!(
        fix_item.owner_role, "fixer",
        "QUEUE DISJOINTNESS: failing edge must be served by role fixer"
    );
    assert_eq!(
        fix_item.target.id, gov_fail.id,
        "QUEUE DISJOINTNESS: fix must target the failing governs edge"
    );

    // Settle the failing edge so the fix queue drains. Fix is strictly the
    // failing-verdict repair lane: it must NEVER serve a packet whose
    // write-back is a verdict, so stale claims are not fix work.
    store
        .record_verdict(
            &gov_fail.id,
            InspectionStatus::Passing,
            "fixed",
            "src/a.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    assert!(
        workitem::next(&store, Some(Lane::Fix)).unwrap().is_none(),
        "QUEUE DISJOINTNESS: fix must drain once no verdict is failing — stale remeasurement is analyze work"
    );

    // analyze serves ONLY the stale relates, never the stale governs.
    let a_item = workitem::next(&store, Some(Lane::Analyze))
        .unwrap()
        .expect("QUEUE DISJOINTNESS: analyze must serve the stale non-governs/validates edge");
    assert_eq!(
        a_item.mode, "analyze",
        "QUEUE DISJOINTNESS: stale remeasurement item mode must be analyze"
    );
    assert_eq!(
        a_item.owner_role, "analyzer",
        "QUEUE DISJOINTNESS: stale claims are verdict work, served by role analyzer"
    );
    assert_eq!(
        a_item.target.id, rel_stale.id,
        "QUEUE DISJOINTNESS: analyze must serve the stale relates edge, not the stale governs"
    );
    assert_ne!(
        a_item.target.id, gov_stale.id,
        "QUEUE DISJOINTNESS: analyze must never serve the stale governs edge"
    );

    // quality serves the stale governs edge.
    let q_item = workitem::next(&store, Some(Lane::Quality))
        .unwrap()
        .expect("QUEUE DISJOINTNESS: quality must serve the stale governs edge");
    assert_eq!(
        q_item.mode, "quality",
        "QUEUE DISJOINTNESS: quality item mode must be quality"
    );
    assert_eq!(
        q_item.target.id, gov_stale.id,
        "QUEUE DISJOINTNESS: quality must target the stale governs edge"
    );

    // validate serves the uninspected validates edge.
    let v_item = workitem::next(&store, Some(Lane::Validate))
        .unwrap()
        .expect("QUEUE DISJOINTNESS: validate must serve the uninspected validates edge");
    assert_eq!(
        v_item.mode, "validate",
        "QUEUE DISJOINTNESS: validate item mode must be validate"
    );
    assert_eq!(
        v_item.target.id, val_edge.id,
        "QUEUE DISJOINTNESS: validate must target the uninspected validates edge"
    );
    assert_eq!(
        v_item.owner_role, "validator",
        "QUEUE DISJOINTNESS: validate must be served by role validator"
    );

    // Disjointness: no edge id served by two queues at once.
    let mut served = vec![
        a_item.target.id.clone(),
        q_item.target.id.clone(),
        v_item.target.id.clone(),
    ];
    let before = served.len();
    served.sort();
    served.dedup();
    assert_eq!(
        served.len(),
        before,
        "QUEUE DISJOINTNESS: an edge id must never be served by two different queue modes at once"
    );
}

// ===========================================================================
// 3. UNMEASURED PAIR FALLBACK
// ===========================================================================

#[test]
fn unmeasured_pair_fallback_serves_leaf_intent_only() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    loom::packs::seed(&store, "iso5055").unwrap();

    // One implemented roll-up intent, NO governs edges yet.
    let root = implemented_intent(&store, "root behavior");

    // A child leaf intent (hierarchy edge under the root) — this is the code-bearing
    // quality surface and therefore MUST be paired instead of the roll-up.
    let child = store
        .add_node(
            NodeType::Intent,
            "child behavior",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Hierarchy,
            &root.id,
            &child.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let item = workitem::next(&store, Some(Lane::Quality)).unwrap().expect(
        "UNMEASURED PAIR: a seeded pack with an unmeasured leaf intent must serve a quality item",
    );

    assert_eq!(
        item.target.kind, "rule_intent_pair",
        "UNMEASURED PAIR: target.kind must be \"rule_intent_pair\", got {}",
        item.target.kind
    );
    // The served intent must be the LEAF, not the roll-up root.
    assert_eq!(
        item.target.id, child.id,
        "UNMEASURED PAIR: only leaf intents are paired, never roll-up roots — served id {} but leaf is {}",
        item.target.id, child.id
    );
    assert_eq!(
        item.target.to.as_deref(),
        Some(child.name.as_str()),
        "UNMEASURED PAIR: target.to must be the leaf intent name"
    );

    // prompt_contract carries evidence_template AND examples (Some).
    assert!(
        item.prompt_contract.evidence_template.is_some(),
        "UNMEASURED PAIR: evidence_template must be Some for a seeded rule"
    );
    assert!(
        item.prompt_contract.examples.is_some(),
        "UNMEASURED PAIR: examples must be Some for a seeded rule"
    );

    // write_back contains `loom rule verdict '` with real rule and intent names.
    let wb = &item.prompt_contract.write_back;
    assert!(
        wb.contains("loom rule verdict '"),
        "UNMEASURED PAIR: write_back must contain `loom rule verdict '`, got: {wb}"
    );
    let rule_name = item
        .target
        .from
        .as_deref()
        .expect("UNMEASURED PAIR: target.from must be set");
    assert!(
        wb.contains(rule_name),
        "UNMEASURED PAIR: write_back must contain the real rule name '{}', got: {wb}",
        rule_name
    );
    assert!(
        wb.contains(&child.name),
        "UNMEASURED PAIR: write_back must contain the real intent name '{}', got: {wb}",
        child.name
    );
}

// ===========================================================================
// 4. SELF-CONTAINED PACKETS
// ===========================================================================

#[test]
fn build_packet_includes_intent_description_in_linked_entities() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "build me",
            "the intent's behavioral description",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();

    let item = workitem::next(&store, Some(Lane::Build))
        .unwrap()
        .expect("SELF-CONTAINED PACKETS: a planned intent must be served by build mode");

    let target_entity = item
        .context
        .linked_entities
        .iter()
        .find(|e| e.role == "target")
        .expect("SELF-CONTAINED PACKETS: build packet must carry a target linked entity");
    assert_eq!(
        target_entity.kind,
        NodeType::Intent.as_str(),
        "SELF-CONTAINED PACKETS: target entity kind must be intent"
    );
    assert_eq!(
        target_entity.id, intent.id,
        "SELF-CONTAINED PACKETS: build packet must target the intent we created"
    );
    assert_eq!(
        target_entity.description.as_deref(),
        Some("the intent's behavioral description"),
        "SELF-CONTAINED PACKETS: target-role linked entity must include the intent's description"
    );
}

#[test]
fn edge_packet_read_set_carries_codefile_path_and_locator() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    let intent = implemented_intent(&store, "grounded intent");
    let codefile = codefile(&store, "src/x.rs");
    // Ground the intent with a locator facet on the implements edge.
    let imp = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &codefile.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &imp.id,
            TargetKind::Edge,
            "locator",
            "fn do_thing",
            TruthClass::Asserted,
        )
        .unwrap();

    // Record a passing verdict so it is eligible to be staled.
    store
        .record_verdict(
            &imp.id,
            InspectionStatus::Passing,
            "grounded",
            "src/x.rs:1",
            0.9,
            "llm",
        )
        .unwrap();

    // Stale it to produce a fix/analyze edge item with the stale_cause facet.
    store
        .stale_edge(&imp.id, "content hash of src/x.rs changed")
        .unwrap();

    let item = workitem::next(&store, Some(Lane::Analyze))
        .unwrap()
        .expect("SELF-CONTAINED PACKETS: a stale implements edge must be served by analyze");

    // context.read_set contains {path == codefile name, locator == facet value}.
    let read = item
        .context
        .read_set
        .iter()
        .find(|r| r.path == "src/x.rs")
        .expect("SELF-CONTAINED PACKETS: read_set must contain the grounded codefile path");
    assert_eq!(
        read.locator.as_deref(),
        Some("fn do_thing"),
        "SELF-CONTAINED PACKETS: read_set locator must equal the facet value, got {:?}",
        read.locator
    );

    // stale_causes surfaces the stale_cause facet.
    assert!(
        item.stale_causes
            .iter()
            .any(|c| c.contains("content hash of src/x.rs changed")),
        "SELF-CONTAINED PACKETS: stale_causes must surface the stale_cause facet, got {:?}",
        item.stale_causes
    );
}

// ===========================================================================
// 5. TRUTH GAP CRITERIA
// ===========================================================================

#[test]
fn truth_gap_criteria_non_empty_and_intent_mentions_reuse() {
    for axis in loom::truth::TRUTH_AXES {
        let gap = axis.gap();
        assert!(
            !gap.correct_when.is_empty(),
            "TRUTH GAP CRITERIA: {} axis gap().correct_when must be non-empty",
            axis.as_str()
        );
    }

    // The Intent axis's correct_when mentions reuse/overlap sizing.
    let intent_gap = loom::truth::TruthAxis::Intent.gap();
    assert!(
        intent_gap.correct_when.contains("recur under multiple parents"),
        "TRUTH GAP CRITERIA: Intent axis correct_when must mention reuse/overlap sizing (assert stable substring), got: {}",
        intent_gap.correct_when
    );
}

#[test]
fn work_item_serialization_includes_truth_gap_correct_when() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let _intent = store
        .add_node(
            NodeType::Intent,
            "serialize me",
            "desc",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();

    let item = workitem::next(&store, Some(Lane::Build))
        .unwrap()
        .expect("TRUTH GAP CRITERIA: build must serve a planned intent");

    let json = serde_json::to_value(&item).expect("TRUTH GAP CRITERIA: WorkItem must serialize");
    let correct_when = json
        .get("truth_gap")
        .and_then(|g| g.get("correct_when"))
        .and_then(|v| v.as_str())
        .expect("TRUTH GAP CRITERIA: serialized WorkItem must include truth_gap.correct_when");
    assert!(
        !correct_when.is_empty(),
        "TRUTH GAP CRITERIA: truth_gap.correct_when in serialized WorkItem must be non-empty"
    );
}

// ===========================================================================
// 6. PACKS — seed, idempotence, enriched body fields
// ===========================================================================

#[test]
fn every_pack_seeds_idempotently_with_enriched_bodies() {
    for &pack_name in loom::packs::PACKS {
        let tmp = Tmp::new();
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();

        let n1 = loom::packs::seed(&store, pack_name)
            .unwrap_or_else(|e| panic!("PACKS: seed('{pack_name}') failed: {e}"));
        let after1 = store
            .list_nodes(Some(NodeType::QualityRule), usize::MAX)
            .unwrap()
            .len();
        assert_eq!(
            n1, after1,
            "PACKS: seed('{pack_name}') return must equal the seeded rule count"
        );

        // Second seed adds 0 new nodes (idempotent).
        loom::packs::seed(&store, pack_name)
            .unwrap_or_else(|e| panic!("PACKS: re-seed('{pack_name}') failed: {e}"));
        let after2 = store
            .list_nodes(Some(NodeType::QualityRule), usize::MAX)
            .unwrap()
            .len();
        assert_eq!(
            after1, after2,
            "PACKS: re-seeding '{pack_name}' must add 0 new nodes (idempotent)"
        );

        // Every seeded rule body has non-empty required fields + valid confidences.
        for rule in loom::packs::pack(pack_name) {
            let node = store
                .resolve_node(rule.name, Some(NodeType::QualityRule))
                .unwrap_or_else(|e| {
                    panic!(
                        "PACKS: rule '{}' of pack '{}' not found after seed: {e}",
                        rule.name, pack_name
                    )
                });
            let body = &node.body;

            let guide = body
                .get("inspection_guide")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("PACKS: rule '{}' missing inspection_guide", rule.name));
            assert!(
                !guide.is_empty(),
                "PACKS: rule '{}' inspection_guide must be non-empty",
                rule.name
            );

            let hints = body
                .get("detection_hints")
                .and_then(|v| v.as_array())
                .unwrap_or_else(|| {
                    panic!("PACKS: rule '{}' missing detection_hints array", rule.name)
                });
            assert!(
                !hints.is_empty(),
                "PACKS: rule '{}' detection_hints must be non-empty",
                rule.name
            );

            let et = body
                .get("evidence_template")
                .unwrap_or_else(|| panic!("PACKS: rule '{}' missing evidence_template", rule.name));
            assert!(
                et.get("passing")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.is_empty()),
                "PACKS: rule '{}' evidence_template.passing must be non-empty",
                rule.name
            );
            assert!(
                et.get("failing")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.is_empty()),
                "PACKS: rule '{}' evidence_template.failing must be non-empty",
                rule.name
            );

            for side in ["passing_example", "failing_example"] {
                let ex = body
                    .get(side)
                    .unwrap_or_else(|| panic!("PACKS: rule '{}' missing {side}", rule.name));
                assert!(
                    ex.get("criterion")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| !s.is_empty()),
                    "PACKS: rule '{}' {side}.criterion must be non-empty",
                    rule.name
                );
                assert!(
                    ex.get("evidence")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| !s.is_empty()),
                    "PACKS: rule '{}' {side}.evidence must be non-empty",
                    rule.name
                );
                let conf = ex
                    .get("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or_else(|| {
                        panic!(
                            "PACKS: rule '{}' {side}.confidence missing or not a number",
                            rule.name
                        )
                    });
                assert!(
                    conf > 0.0 && conf <= 1.0,
                    "PACKS: rule '{}' {side}.confidence must be in (0,1], got {conf}",
                    rule.name
                );
            }
        }
    }
}

// ===========================================================================
// 7. DOOR + INBOX + NOTE CLI (compiled binary)
// ===========================================================================
//
// These drive the compiled `loom` binary end-to-end, mirroring the ring5 CLI
// test pattern (std::process::Command + CARGO_BIN_EXE_loom).

fn loom_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom"))
}

fn loom_init(tmp: &Path, name: Option<&str>) {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("init").arg(tmp);
    if let Some(n) = name {
        cmd.args(["--name", n]);
    }
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom init: {e}"));
    assert!(
        out.status.success(),
        "loom init {:?} failed: {:?}\n{}",
        tmp,
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run `loom --graph <tmp> <args> --json` and assert success, returning stdout
/// parsed as JSON.
fn loom_json(tmp: &Path, args: &[&str]) -> serde_json::Value {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args).arg("--json");
    let out = cmd.output().unwrap_or_else(|e| panic!("spawn loom: {e}"));
    assert!(
        out.status.success(),
        "loom {:?} --json failed: {:?}\n--stderr--\n{}",
        args,
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "loom {:?} --json did not emit JSON:\n--stdout--\n{}\nparse error: {e}",
            args, stdout
        )
    })
}

/// Run `loom --graph <tmp> <args>` (no --json) and assert success, returning
/// the captured stdout string.
fn loom_run_ok(tmp: &Path, args: &[&str]) -> String {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {:?}: {e}", args));
    assert!(
        out.status.success(),
        "loom {:?} failed: {:?}\n--stderr--\n{}",
        args,
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn door_landing_menu_and_inbox_mark_contract() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));

    // `loom door "x" --json` output contains landing_menu with new_journey +
    // dismiss landings and a next_step naming `loom inbox mark`.
    let door = loom_json(tmp.path(), &["door", "x"]);
    let capability = door
        .get("capability")
        .and_then(|v| v.as_str())
        .expect("DOOR+INBOX CLI: door --json must explain partial-idea elaboration");
    assert!(
        capability.contains("does not need to be a complete specification")
            && capability.contains("one plain-language product question at a time"),
        "DOOR+INBOX CLI: capability must tell a non-coder what Loom and the LLM can do, got: {capability}"
    );
    let menu = door
        .get("landing_menu")
        .and_then(|v| v.as_array())
        .expect("DOOR+INBOX CLI: door --json must emit a landing_menu array");
    let landings: Vec<&str> = menu
        .iter()
        .filter_map(|m| m.get("landing").and_then(|v| v.as_str()))
        .collect();
    assert!(
        landings.contains(&"new_journey"),
        "DOOR+INBOX CLI: landing_menu must contain a 'new_journey' landing, got {:?}",
        landings
    );
    assert!(
        landings.contains(&"dismiss"),
        "DOOR+INBOX CLI: landing_menu must contain a 'dismiss' landing, got {:?}",
        landings
    );
    assert!(
        landings.contains(&"hypothesis"),
        "DOOR+INBOX CLI: landing_menu must contain a 'hypothesis' landing, got {:?}",
        landings
    );
    let new_journey = menu
        .iter()
        .find(|m| m.get("landing").and_then(|v| v.as_str()) == Some("new_journey"))
        .expect("DOOR+INBOX CLI: new_journey landing exists");
    assert!(
        new_journey
            .get("after")
            .and_then(|v| v.as_str())
            .is_some_and(|after| after.contains("loom next --mode derive")
                && after.contains("human-authorized")),
        "DOOR+INBOX CLI: new-Journey landing must route the LLM through human-authorized derivation"
    );
    let text_door = loom_run_ok(tmp.path(), &["door", "another partial idea"]);
    assert!(
        text_door.contains("does not need to be a complete specification")
            && text_door.contains("then: loom next --mode derive"),
        "DOOR+INBOX CLI: human-readable intake must expose the same capability and follow-up"
    );
    let next_step = door
        .get("next_step")
        .and_then(|v| v.as_str())
        .expect("DOOR+INBOX CLI: door --json must emit a next_step string");
    assert!(
        next_step.contains("loom inbox mark"),
        "DOOR+INBOX CLI: next_step must name `loom inbox mark`, got: {}",
        next_step
    );

    // Extract the captured inbox item short id for the mark commands.
    let id = door
        .get("captured")
        .and_then(|c| c.get("id"))
        .and_then(|v| v.as_str())
        .expect("DOOR+INBOX CLI: door --json must emit captured.id")
        .to_string();
    let short = &id[..8.min(id.len())];

    // `loom inbox mark <id> bogus` exits non-zero and names the allowed dispositions.
    // This bails before emitting JSON, so we use raw process + stderr.
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph")
        .arg(tmp.path())
        .args(["inbox", "mark", short, "bogus"]);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom inbox mark bogus: {e}"));
    assert!(
        !out.status.success(),
        "DOOR+INBOX CLI: `loom inbox mark <id> bogus` must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("routed") && stderr.contains("rejected") && stderr.contains("deferred") && stderr.contains("duplicate"),
        "DOOR+INBOX CLI: bogus disposition error must name the allowed dispositions, got stderr: {}",
        stderr
    );

    // A routed item must name where it actually landed: `--reason` carries a
    // '<destination-type>:<stable-node-id>' reference, and the destination node
    // has to exist with the matching type. Prose alone can claim an idea was
    // handled without anything in the graph ever receiving it.
    let destination = loom_json(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "the behavior this idea became",
            "--description",
            "where the captured idea landed",
            "--level",
            "feature",
            "--visibility",
            "user_visible",
            "--aspect",
            "happy",
        ],
    );
    let destination_id = destination
        .get("intent")
        .and_then(|i| i.get("id"))
        .or_else(|| destination.get("id"))
        .and_then(|v| v.as_str())
        .expect("DOOR+INBOX CLI: intent add --json must emit the created id")
        .to_string();

    // An unstructured reason is refused: it cannot be resolved to a destination.
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph")
        .arg(tmp.path())
        .args(["inbox", "mark", short, "routed", "--reason", "r"]);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom inbox mark routed: {e}"));
    assert!(
        !out.status.success(),
        "DOOR+INBOX CLI: a routed item needs a destination reference, not prose"
    );

    // `loom inbox mark <id> routed --reason existing_intent:<id>` succeeds.
    loom_run_ok(
        tmp.path(),
        &[
            "inbox",
            "mark",
            short,
            "routed",
            "--reason",
            &format!("existing_intent:{destination_id}"),
        ],
    );

    // `loom inbox list --status new` filters: the routed item must not appear.
    let new_list = loom_json(tmp.path(), &["inbox", "list", "--status", "new"]);
    let new_arr = new_list["items"]
        .as_array()
        .expect("DOOR+INBOX CLI: inbox list --status new --json must emit items");
    assert!(
        new_arr
            .iter()
            .all(|n| n.get("id").and_then(|v| v.as_str()) != Some(&id)),
        "DOOR+INBOX CLI: `inbox list --status new` must filter out the routed item"
    );

    // `loom inbox show <id>` prints the full text.
    let show = loom_json(tmp.path(), &["inbox", "show", short]);
    let text = show
        .get("text")
        .and_then(|v| v.as_str())
        .expect("DOOR+INBOX CLI: inbox show --json must emit a text field");
    assert_eq!(
        text, "x",
        "DOOR+INBOX CLI: inbox show must print the full captured text, got: {}",
        text
    );
}

#[test]
fn door_weak_existing_intent_match_follows_new_journey_and_is_labelled_weak() {
    // Contract: a weak lexical match (score < 4) must NOT displace new_journey.
    // Strong matches (score >= 4) precede new_journey; weak ones follow spike and
    // carry confidence="weak" with a why that nudges toward new_journey.
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));

    // Seed an existing intent whose description shares exactly one query term
    // with the utterance and whose name shares none. Scoring is
    // score(name)*2 + score(description); a single description hit yields 1,
    // which is below the strong threshold of 4 -> a weak match.
    loom_run_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "unrelated thing",
            "--description",
            "giraffe",
            "--level",
            "feature",
            "--visibility",
            "user_visible",
            "--aspect",
            "happy",
        ],
    );

    let door = loom_json(tmp.path(), &["door", "giraffe feeding schedule"]);
    let menu = door
        .get("landing_menu")
        .and_then(|v| v.as_array())
        .expect("DOOR WEAK MATCH: door --json must emit a landing_menu array");
    let landings: Vec<&str> = menu
        .iter()
        .filter_map(|m| m.get("landing").and_then(|v| v.as_str()))
        .collect();

    // No strong match in this fixture, so new_journey must be first.
    assert_eq!(
        landings.first().copied(),
        Some("new_journey"),
        "DOOR WEAK MATCH: with no strong match, landing_menu[0] must be new_journey, got {:?}",
        landings
    );

    // The weak existing_intent entry must follow spike (i.e. after the
    // new_journey/hypothesis/spike block) and be labelled weak.
    let spike_idx = landings
        .iter()
        .position(|&l| l == "spike")
        .expect("DOOR WEAK MATCH: landing_menu must contain a spike landing");
    let weak_idx = landings
        .iter()
        .position(|&l| l == "existing_intent")
        .expect("DOOR WEAK MATCH: landing_menu must contain an existing_intent landing");
    assert!(
        weak_idx > spike_idx,
        "DOOR WEAK MATCH: weak existing_intent must follow spike, got landings {:?}",
        landings
    );

    let weak_entry = &menu[weak_idx];
    assert_eq!(
        weak_entry.get("confidence").and_then(|v| v.as_str()),
        Some("weak"),
        "DOOR WEAK MATCH: weak existing_intent entry must have confidence == \"weak\", got: {}",
        serde_json::to_string_pretty(weak_entry).unwrap()
    );
    let why = weak_entry
        .get("why")
        .and_then(|v| v.as_str())
        .expect("DOOR WEAK MATCH: weak existing_intent entry must have a why string");
    assert!(
        why.contains("weak lexical overlap") && why.contains("prefer new_journey"),
        "DOOR WEAK MATCH: weak why must mention weak lexical overlap and prefer new_journey, got: {}",
        why
    );
}

#[test]
fn door_weak_generic_name_term_does_not_become_a_strong_match() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    loom_run_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "fix failing checks",
            "--description",
            "routes broken proof results to repair work",
            "--level",
            "feature",
            "--visibility",
            "user_visible",
            "--aspect",
            "happy",
        ],
    );

    let door = loom_json(
        tmp.path(),
        &[
            "door",
            "fix Loom dogfood so it exercises the real repository",
        ],
    );
    let menu = door["landing_menu"]
        .as_array()
        .expect("door emits a landing menu");
    assert_eq!(menu[0]["landing"], "new_journey");
    let existing = menu
        .iter()
        .find(|entry| entry["landing"] == "existing_intent")
        .expect("the lexical match remains visible as a weak option");
    assert_eq!(existing["confidence"], "weak");
}

#[test]
fn note_add_then_list_round_trips() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));

    // Create a target intent to attach the note to.
    loom_run_ok(
        tmp.path(),
        &["intent", "add", "--name", "noteable", "--description", "d"],
    );

    // `loom note add <target> --text t` succeeds.
    loom_run_ok(
        tmp.path(),
        &["note", "add", "noteable", "--text", "a durable decision"],
    );

    // `loom note list <target>` round-trips: the note text appears.
    let list = loom_json(tmp.path(), &["note", "list", "noteable"]);
    let arr = list["items"]
        .as_array()
        .expect("DOOR+INBOX CLI: note list --json must emit items");
    let found = arr
        .iter()
        .any(|n| n.get("text").and_then(|v| v.as_str()) == Some("a durable decision"));
    assert!(
        found,
        "DOOR+INBOX CLI: note list <target> must round-trip the added note text, got: {}",
        serde_json::to_string_pretty(&list).unwrap()
    );
}

#[test]
fn inbox_add_source_code_audit_is_rejected_with_finding_add_hint() {
    // Contract: evidence-backed sources (code_audit|wiki|validation|llm) are
    // rejected by `inbox add` with a message pointing to `loom finding add`.
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));

    for source in ["code_audit", "wiki", "validation", "llm"] {
        let out = std::process::Command::new(loom_bin())
            .arg("--graph")
            .arg(tmp.path())
            .args(["inbox", "add", "an observation", "--source", source])
            .output()
            .unwrap_or_else(|e| panic!("spawn loom inbox add --source {source}: {e}"));
        assert!(
            !out.status.success(),
            "INBOX SOURCE GATE: `inbox add --source {source}` must exit non-zero"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("loom finding add"),
            "INBOX SOURCE GATE: rejection for source '{source}' must mention `loom finding add`, got: {stderr}"
        );
    }
}

#[test]
fn inbox_add_source_question_is_rejected_with_question_add_hint() {
    // Contract: `inbox add --source question` is rejected with a message pointing
    // to `loom question add`, not to `loom finding add`.
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));

    let out = std::process::Command::new(loom_bin())
        .arg("--graph")
        .arg(tmp.path())
        .args([
            "inbox",
            "add",
            "should we support SSO?",
            "--source",
            "question",
        ])
        .output()
        .unwrap_or_else(|e| panic!("spawn loom inbox add --source question: {e}"));
    assert!(
        !out.status.success(),
        "INBOX SOURCE GATE: `inbox add --source question` must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("loom question add"),
        "INBOX SOURCE GATE: rejection for source 'question' must mention `loom question add`, got: {stderr}"
    );
    assert!(
        !stderr.contains("loom finding add"),
        "INBOX SOURCE GATE: rejection for 'question' must NOT redirect to finding add, got: {stderr}"
    );
}

#[test]
fn guide_json_intake_uses_hardcut_entry_points() {
    // Contract: `loom guide --json` intake object has separate entry points for
    // human input, evidence-backed observations, and product questions. Findings
    // and questions no longer route through `loom door`.
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));

    let guide = loom_json(tmp.path(), &["guide"]);
    let intake = guide
        .get("intake")
        .expect("GUIDE JSON: must emit an 'intake' object");

    // Human/external input still goes through door.
    let human = intake
        .get("human_or_external_input")
        .and_then(|v| v.as_str())
        .expect("GUIDE JSON: intake must have 'human_or_external_input'");
    assert!(
        human.contains("loom door"),
        "GUIDE JSON: human_or_external_input must reference loom door, got: {human}"
    );

    // Evidence-backed observations go to finding add, not door.
    let obs = intake
        .get("evidence_backed_observation")
        .and_then(|v| v.as_str())
        .expect("GUIDE JSON: intake must have 'evidence_backed_observation'");
    assert!(
        obs.contains("loom finding add"),
        "GUIDE JSON: evidence_backed_observation must reference loom finding add, got: {obs}"
    );
    assert!(
        !obs.contains("loom door"),
        "GUIDE JSON: evidence_backed_observation must NOT reference loom door, got: {obs}"
    );

    // Product questions go to question add, not door.
    let q = intake
        .get("product_question")
        .and_then(|v| v.as_str())
        .expect("GUIDE JSON: intake must have 'product_question'");
    assert!(
        q.contains("loom question add"),
        "GUIDE JSON: product_question must reference loom question add, got: {q}"
    );
    assert!(
        !q.contains("loom door"),
        "GUIDE JSON: product_question must NOT reference loom door, got: {q}"
    );

    let roles = guide["roles"]
        .as_array()
        .expect("GUIDE JSON: roles must be an array");
    assert!(!roles.iter().any(|role| role == "monitor"));

    let cold_start = &guide["brownfield_cold_start"];
    let commands = cold_start["commands"]
        .as_array()
        .expect("GUIDE JSON: brownfield cold start must contain ordered commands");
    for required in [
        "loom --version",
        "loom init",
        "loom codefile add '<glob>'",
        "loom sync --json",
        "loom bootstrap suggest",
    ] {
        assert!(commands.iter().any(|command| command == required));
    }
    assert!(cold_start["human_authority"]
        .as_str()
        .unwrap()
        .contains("Do not compose, infer, or paraphrase"));
    assert!(cold_start["rebuild_distinction"]
        .as_str()
        .unwrap()
        .contains("does not migrate or reconstruct a pre-v12 graph"));
    assert!(cold_start["proof_interface"]
        .as_str()
        .unwrap()
        .contains("stable production-owned black-box"));

    let resume = &guide["pending_human_resume"];
    assert!(resume["command_template"]
        .as_str()
        .unwrap()
        .contains("--human-decision"));
    assert!(resume["command_template"]
        .as_str()
        .unwrap()
        .contains("--free-form"));
    assert!(resume["stop_instruction"]
        .as_str()
        .unwrap()
        .contains("Never compose, infer, paraphrase, or choose"));

    let closeout = guide["closeout"]
        .as_array()
        .expect("GUIDE JSON: closeout must be an array");
    for required in [
        "loom sync --json",
        "loom doctor --json",
        "loom audit --json",
        "loom journey drift --json",
        "loom status --json",
        "loom export --json",
        "loom export --check",
    ] {
        assert!(closeout.iter().any(|command| command == required));
    }
}
