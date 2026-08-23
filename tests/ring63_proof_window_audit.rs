//! Ring 63 — the `writes_during_proof` audit check.
//!
//! Real SQLite + real journal, no mocks. Contracts defended: a solo-actor
//! asserted fact landing inside a bracketed proof-execution window is flagged;
//! a lane-actor write inside the window passes (parallel drivers are
//! legitimate); a solo write after — even in the same instant as — the closing
//! bracket passes (settlement); a started/ended pair with mismatched pids
//! forms no window; an unclosed window from a dead process is its own
//! `unclosed_proof_window` finding (never a silent blind spot), while a live
//! process's open window is a run in flight and stays quiet.

use loom::identity::ExecutionIdentity;
use loom::model::{Claim, NodeType};
use loom::store::Store;

mod common;
use common::*;

const STARTED: &str = "proof_execution_started";
const ENDED: &str = "proof_execution_ended";

fn bracket(root: &std::path::Path, event: &str, validation: &str, pid: u64) {
    loom::journal::append(
        root,
        &ExecutionIdentity::solo(),
        event,
        validation,
        serde_json::json!({ "purpose": "journey run", "pid": pid }),
    )
    .unwrap();
}

fn append_journey_result(root: &std::path::Path, validation: &str, outcome: &str) -> String {
    loom::journal::append(
        root,
        &ExecutionIdentity::solo(),
        "journey_run",
        validation,
        serde_json::json!({
            "assertions_failed": if outcome == "passed" { 0 } else { 1 },
            "assertions_passed": if outcome == "passed" { 1 } else { 0 },
            "journey_hash": "journey-hash",
            "journey_id": "journey-id",
            "outcome": outcome,
            "profile": "proof",
            "surface_hash": "surface-hash"
        }),
    )
    .unwrap()
    .id
}

fn finding_node(store: &Store, name: &str) -> String {
    store
        .add_node(NodeType::Finding, name, "d", "open", serde_json::json!({}))
        .unwrap()
        .id
}

fn assert_observation(store: &Store, node_id: &str, actor: &str) {
    store
        .assert_fact(
            loom::store::Assertion::new(
                loom::store::Subject::Node(node_id.to_string()),
                Claim::Observation,
                "confirmed",
                actor,
            )
            .cited(loom::evidence::cite(store.root(), "saw it").unwrap()),
        )
        .unwrap();
}

fn pause() {
    // Window boundaries are compared at millisecond precision; keep the
    // journal bracket and the fact stamp from sharing one millisecond.
    std::thread::sleep(std::time::Duration::from_millis(15));
}

#[test]
fn solo_write_inside_a_proof_window_is_flagged() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let node = finding_node(&store, "written mid-proof");

    bracket(tmp.path(), STARTED, "val-1", 42);
    pause();
    assert_observation(&store, &node, "solo");
    pause();
    bracket(tmp.path(), ENDED, "val-1", 42);

    let findings = loom::audit::run(&store).unwrap();
    let hit = findings
        .iter()
        .find(|f| f.kind == "writes_during_proof")
        .expect("solo write inside the window must be flagged");
    assert_eq!(hit.subject.id(), Some(node.as_str()));
    assert!(hit.detail.contains("val-1"), "detail: {}", hit.detail);
    assert!(!hit.remedy.is_empty());
}

#[test]
fn lane_writes_settlement_writes_and_mismatched_pids_pass() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let live_pid = u64::from(std::process::id());

    // A parallel lane driver writing during someone else's proof is the
    // sanctioned multi-driver shape, not fabrication.
    let lane_node = finding_node(&store, "lane write mid-proof");
    bracket(tmp.path(), STARTED, "val-2", live_pid);
    pause();
    assert_observation(&store, &lane_node, "llm:analyzer");
    pause();
    bracket(tmp.path(), ENDED, "val-2", live_pid);

    // Settlement boundary, pinned tight: a solo write in the same instant as
    // (or any time after) the closing bracket is legitimate — the end is
    // exclusive precisely so settlement can share its millisecond.
    let settled_node = finding_node(&store, "settled at the window edge");
    assert_observation(&store, &settled_node, "solo");

    // A started/ended pair with different pids is two half-windows, not one
    // window: the pairing key is (validation, pid). The started half stays
    // open under THIS live process, so it is a run in flight — quiet.
    let mismatch_node = finding_node(&store, "between mismatched brackets");
    bracket(tmp.path(), STARTED, "val-3", live_pid);
    pause();
    assert_observation(&store, &mismatch_node, "solo");
    pause();
    bracket(tmp.path(), ENDED, "val-3", live_pid + 1);

    let findings = loom::audit::run(&store).unwrap();
    let flagged: Vec<_> = findings
        .iter()
        .filter(|f| f.kind == "writes_during_proof" || f.kind == "unclosed_proof_window")
        .map(|f| &f.detail)
        .collect();
    assert!(flagged.is_empty(), "unexpected flags: {flagged:?}");
}

#[test]
fn a_dead_runs_unclosed_window_is_its_own_finding() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    // macOS and Linux cap pids well below this, so it can never be alive.
    bracket(tmp.path(), STARTED, "val-crashed", 99_999_999);
    pause();
    let after_crash = finding_node(&store, "written after a crashed start");
    assert_observation(&store, &after_crash, "solo");

    let findings = loom::audit::run(&store).unwrap();
    // No bounded window exists, so the fact itself cannot be indicted…
    assert!(!findings.iter().any(|f| f.kind == "writes_during_proof"));
    // …but the unauditable window is loudly visible instead of silent.
    let hit = findings
        .iter()
        .find(|f| f.kind == "unclosed_proof_window")
        .expect("a dead run's open window must be flagged");
    assert!(hit.detail.contains("val-crashed"), "detail: {}", hit.detail);
    assert!(hit.detail.contains("99999999"), "detail: {}", hit.detail);
}

#[test]
fn exact_resolved_finding_with_later_passing_rerun_clears_unclosed_window() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let validation = "val-crashed";
    let pid = 99_999_999;
    bracket(tmp.path(), STARTED, validation, pid);
    let start_entry = loom::journal::read(tmp.path())
        .unwrap()
        .last()
        .unwrap()
        .id
        .clone();
    pause();
    let rerun_entry = append_journey_result(tmp.path(), validation, "passed");
    let finding = store
        .add_node(
            NodeType::Finding,
            "crashed proof window",
            "the original window was unauditable",
            "unclosed_proof_window",
            serde_json::json!({
                "kind": "unclosed_proof_window",
                "source": "validation",
                "evidence": format!("journal:{start_entry}"),
                "impact": "requires an exact clean rerun",
                "confidence": 1.0,
                "link": validation
            }),
        )
        .unwrap();
    store
        .record_finding_verdict(
            &finding.id,
            "resolved",
            "the exact proof reran and passed",
            &format!("journal:{rerun_entry}"),
        )
        .unwrap();

    let findings = loom::audit::run(&store).unwrap();
    assert!(
        !findings.iter().any(|f| f.kind == "unclosed_proof_window"),
        "durably resolved exact rerun should clear the crash finding: {findings:?}"
    );
}

#[test]
fn unrelated_or_nonpassing_rerun_cannot_clear_unclosed_window() {
    for (rerun_validation, outcome) in [("other-proof", "passed"), ("val-crashed", "blocked")] {
        let tmp = Tmp::new();
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        let validation = "val-crashed";
        bracket(tmp.path(), STARTED, validation, 99_999_999);
        let start_entry = loom::journal::read(tmp.path())
            .unwrap()
            .last()
            .unwrap()
            .id
            .clone();
        pause();
        let rerun_entry = append_journey_result(tmp.path(), rerun_validation, outcome);
        let finding = store
            .add_node(
                NodeType::Finding,
                "crashed proof window",
                "the original window was unauditable",
                "unclosed_proof_window",
                serde_json::json!({
                    "kind": "unclosed_proof_window",
                    "source": "validation",
                    "evidence": format!("journal:{start_entry}"),
                    "impact": "requires an exact clean rerun",
                    "confidence": 1.0,
                    "link": validation
                }),
            )
            .unwrap();
        store
            .record_finding_verdict(
                &finding.id,
                "resolved",
                "claimed resolved",
                &format!("journal:{rerun_entry}"),
            )
            .unwrap();

        let findings = loom::audit::run(&store).unwrap();
        assert!(
            findings.iter().any(|f| f.kind == "unclosed_proof_window"),
            "{rerun_validation}/{outcome} must not clear the exact crash"
        );
    }
}
