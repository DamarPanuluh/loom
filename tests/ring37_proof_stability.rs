//! Ring 37 — a proof that changes its mind about unchanged code.
//!
//! loom re-runs every registered proof on every dogfood and, until now, forgot
//! that it had: a validation carried one outcome and the next run overwrote it.
//! So a proof passing four runs and failing the fifth was recorded as whichever
//! run loom happened to observe.
//!
//! That is not hypothetical. The INV-8 proofs — defending the human-presence
//! ratification boundary, the most consequential gate in the graph — were flaky
//! for want of a lock: tests mutating process-global LOOM_AGENT made them pass
//! or fail on thread scheduling, and loom recorded whichever it saw.
//!
//! The anchor set is what separates a flake from an honest change. If a covered
//! file moved, a different outcome is the system working. If nothing moved, the
//! proof depends on something other than the code.

use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::store::Store;
mod common;
use common::Tmp;

/// A validation over one grounded behavior, run through loom's own path.
fn seeded(tmp: &Tmp, command: &str) -> (Store, String) {
    std::fs::write(tmp.path().join("s.rs"), "pub fn a() {}\n").unwrap();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "a behavior",
            "does something",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = store
        .add_node(NodeType::CodeFile, "s.rs", "", "", serde_json::json!({}))
        .unwrap();
    let g = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &g.id,
            TargetKind::Edge,
            "locator",
            "fn a",
            TruthClass::Asserted,
        )
        .unwrap();
    let val = store
        .add_node(
            NodeType::Validation,
            "the proof",
            "",
            "not_run",
            serde_json::json!({ "type": "test", "command": command }),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Validates,
            &val.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    (store, val.id)
}

fn run_proof(store: &Store, val_id: &str) {
    let val = store.get_node(val_id).unwrap().unwrap();
    loom::commands::observe_validation(store, &val).unwrap();
}

fn unstable(store: &Store, val_id: &str) -> Option<String> {
    store
        .get_facet(val_id, TargetKind::Node, "proof_unstable")
        .unwrap()
}

/// **A flip over unchanged code is recorded as instability.**
#[test]
fn an_outcome_that_flips_over_unchanged_code_is_flagged() {
    let tmp = Tmp::new();
    // Passes or fails depending on a file OUTSIDE the proof's anchor set, so
    // the covered hashes are identical across both runs — exactly the shape of
    // a test that depends on shared state rather than on the code it proves.
    let (store, val) = seeded(&tmp, "sh -c 'test ! -f flip'");
    run_proof(&store, &val);
    assert_eq!(
        store.get_node(&val).unwrap().unwrap().status,
        "passed",
        "precondition: the first run passes"
    );
    assert!(unstable(&store, &val).is_none(), "one run cannot disagree");

    std::fs::write(tmp.path().join("flip"), "").unwrap();
    run_proof(&store, &val);

    assert_eq!(store.get_node(&val).unwrap().unwrap().status, "failed");
    let detail = unstable(&store, &val).expect("the flip is recorded");
    assert!(
        detail.contains("passed") && detail.contains("failed"),
        "the record names both outcomes so a reader knows what changed: {detail}"
    );
}

/// **A consistent pair clears it.**
///
/// One flip is evidence of instability; runs that agree afterwards are evidence
/// it settled. A genuinely flaky proof keeps re-tripping this, which is the
/// signal wanted — loom reports what it observed rather than deciding how many
/// runs constitute proof.
#[test]
fn a_matching_pair_over_unchanged_code_clears_it() {
    let tmp = Tmp::new();
    let (store, val) = seeded(&tmp, "sh -c 'test ! -f flip'");
    run_proof(&store, &val);
    std::fs::write(tmp.path().join("flip"), "").unwrap();
    run_proof(&store, &val);
    assert!(unstable(&store, &val).is_some(), "precondition: flagged");

    run_proof(&store, &val); // fails again — consistent with the run before it
    assert!(
        unstable(&store, &val).is_none(),
        "two runs agreeing over the same anchors is a settled proof"
    );
}

/// **A different outcome after the CODE moved is not instability.**
///
/// This is the whole discrimination. A proof that starts failing because the
/// behavior broke is the system working, and flagging it would drown the real
/// signal in every honest regression.
#[test]
fn a_flip_after_the_covered_code_changed_is_not_flagged() {
    let tmp = Tmp::new();
    let (store, val) = seeded(&tmp, "sh -c 'grep -q \"pub fn a\" s.rs'");
    run_proof(&store, &val);
    assert_eq!(store.get_node(&val).unwrap().unwrap().status, "passed");

    // The anchored file itself changes, and the proof legitimately flips.
    std::fs::write(tmp.path().join("s.rs"), "pub fn renamed() {}\n").unwrap();
    run_proof(&store, &val);

    assert_eq!(store.get_node(&val).unwrap().unwrap().status, "failed");
    assert!(
        unstable(&store, &val).is_none(),
        "the code moved, so a different outcome is honest, not flaky"
    );
}

/// **The first observation records its anchors.**
///
/// A run that stores nothing leaves the next with nothing to compare against,
/// so every second run would read as "the code moved" and no flip could ever be
/// seen. Found by building it the other way round first.
#[test]
fn the_first_run_records_its_anchor_set() {
    let tmp = Tmp::new();
    let (store, val) = seeded(&tmp, "true");
    run_proof(&store, &val);
    let anchors = store
        .get_facet(&val, TargetKind::Node, "proof_anchors")
        .unwrap()
        .expect("the first run records what it ran over");
    assert!(
        anchors.contains("s.rs"),
        "the anchor digest names the covered file: {anchors}"
    );
}
