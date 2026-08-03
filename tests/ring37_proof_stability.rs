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

/// Attack: a single flake followed by two matching greens clears the flag.
///
/// The claim says a genuinely flaky proof "keeps re-tripping". That is true for
/// strict alternation, but a mostly-passing flake — the INV-8 shape of "fails
/// one in five" — clears as soon as any two consecutive runs agree. Dogfood
/// that re-runs a flaky suite twice after a one-off fail will report stability.
#[test]
fn a_mostly_passing_flake_clears_after_two_matching_greens() {
    let tmp = Tmp::new();
    let (store, val) = seeded(&tmp, "sh -c 'test ! -f flip'");
    run_proof(&store, &val); // pass
    std::fs::write(tmp.path().join("flip"), "").unwrap();
    run_proof(&store, &val); // fail → flagged
    assert!(unstable(&store, &val).is_some());
    std::fs::remove_file(tmp.path().join("flip")).unwrap();
    run_proof(&store, &val); // pass (still flagged: failed→passed)
    assert!(
        unstable(&store, &val).is_some(),
        "a flip back to pass is still a flip"
    );
    run_proof(&store, &val); // pass again → clears
    assert!(
        unstable(&store, &val).is_none(),
        "two greens after a one-off fail erase the instability record"
    );
}

/// Attack: editing a verifies-role test file changes the digest and hides a flake.
#[test]
fn a_test_file_edit_suppresses_flake_detection() {
    let tmp = Tmp::new();
    std::fs::write(tmp.path().join("s.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(tmp.path().join("t.rs"), "// test\n").unwrap();
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
    let src = store
        .add_node(NodeType::CodeFile, "s.rs", "", "", serde_json::json!({}))
        .unwrap();
    let test = store
        .add_node(NodeType::CodeFile, "t.rs", "", "", serde_json::json!({}))
        .unwrap();
    let g_src = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &src.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &g_src.id,
            TargetKind::Edge,
            "locator",
            "fn a",
            TruthClass::Asserted,
        )
        .unwrap();
    let g_test = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &test.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_grounding_role(&g_test.id, loom::model::GroundingRole::Verifies)
        .unwrap();
    let val = store
        .add_node(
            NodeType::Validation,
            "the proof",
            "",
            "not_run",
            serde_json::json!({ "type": "test", "command": "sh -c 'test ! -f flip'" }),
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

    run_proof(&store, &val.id); // pass
    std::fs::write(tmp.path().join("flip"), "").unwrap();
    // Edit the TEST file (not the realizing source) at the same time as the flake.
    std::fs::write(tmp.path().join("t.rs"), "// test changed\n").unwrap();
    run_proof(&store, &val.id); // fail, but anchors moved because verifies is in the digest

    assert_eq!(store.get_node(&val.id).unwrap().unwrap().status, "failed");
    assert!(
        unstable(&store, &val.id).is_none(),
        "a verifies-role edit moves the digest and suppresses the flake signal"
    );
}

/// Attack: passed → blocked is recorded as instability.
///
/// A blocked outcome is usually infrastructure (lock, missing binary, empty
/// command) — not a flake about the code. Treating it like passed↔failed
/// conflates "the harness could not run" with "the proof disagreed with itself".
#[test]
fn passed_to_blocked_is_flagged_as_instability() {
    let tmp = Tmp::new();
    let (store, val) = seeded(&tmp, "true");
    run_proof(&store, &val);
    assert_eq!(store.get_node(&val).unwrap().unwrap().status, "passed");

    // Untrusted import blocks before execution — infrastructure, not a flake.
    let mut body = store.get_node(&val).unwrap().unwrap().body;
    body["command_trusted"] = serde_json::json!(false);
    store.set_node_body(&val, &body).unwrap();
    run_proof(&store, &val);

    assert_eq!(store.get_node(&val).unwrap().unwrap().status, "blocked");
    let detail = unstable(&store, &val).expect("passed→blocked is flagged today");
    assert!(
        detail.contains("passed") && detail.contains("blocked"),
        "infrastructure block is recorded as instability: {detail}"
    );
}

/// Attack: a validation over TWO intents clears instability when EITHER moves.
#[test]
fn a_sibling_intent_file_change_clears_instability_for_the_shared_proof() {
    let tmp = Tmp::new();
    std::fs::write(tmp.path().join("a.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(tmp.path().join("b.rs"), "pub fn b() {}\n").unwrap();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let ia = store
        .add_node(
            NodeType::Intent,
            "behavior a",
            "a",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let ib = store
        .add_node(
            NodeType::Intent,
            "behavior b",
            "b",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cfa = store
        .add_node(NodeType::CodeFile, "a.rs", "", "", serde_json::json!({}))
        .unwrap();
    let cfb = store
        .add_node(NodeType::CodeFile, "b.rs", "", "", serde_json::json!({}))
        .unwrap();
    let ga = store
        .add_edge(EdgeKind::Implements, &ia.id, &cfa.id, TruthClass::Asserted)
        .unwrap();
    store
        .set_facet(&ga.id, TargetKind::Edge, "locator", "fn a", TruthClass::Asserted)
        .unwrap();
    let gb = store
        .add_edge(EdgeKind::Implements, &ib.id, &cfb.id, TruthClass::Asserted)
        .unwrap();
    store
        .set_facet(&gb.id, TargetKind::Edge, "locator", "fn b", TruthClass::Asserted)
        .unwrap();
    let val = store
        .add_node(
            NodeType::Validation,
            "shared proof",
            "",
            "not_run",
            serde_json::json!({ "type": "test", "command": "sh -c 'test ! -f flip'" }),
        )
        .unwrap();
    store
        .add_edge(EdgeKind::Validates, &val.id, &ia.id, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Validates, &val.id, &ib.id, TruthClass::Asserted)
        .unwrap();

    run_proof(&store, &val.id);
    std::fs::write(tmp.path().join("flip"), "").unwrap();
    run_proof(&store, &val.id);
    assert!(unstable(&store, &val.id).is_some(), "precondition: flagged");

    // Change ONLY sibling B's realizing file — A (and the flake trigger) untouched.
    std::fs::write(tmp.path().join("b.rs"), "pub fn b_renamed() {}\n").unwrap();
    run_proof(&store, &val.id); // still fails (flip present), but digest moved

    assert!(
        unstable(&store, &val.id).is_none(),
        "a change to a sibling intent's file clears instability for the shared validation"
    );
}
