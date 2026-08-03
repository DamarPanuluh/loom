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

/// **Agreement does not clear it; adjudication does.**
///
/// I wrote this the other way round first — a matching pair cleared the flag —
/// and the review was right that it undercut the motivating case. Clearing now
/// goes through the same adjudication lifecycle every other smell uses, rather
/// than a second, weaker rule competing with it.
#[test]
fn agreement_alone_does_not_clear_the_instability() {
    let tmp = Tmp::new();
    let (store, val) = seeded(&tmp, "sh -c 'test ! -f flip'");
    run_proof(&store, &val);
    std::fs::write(tmp.path().join("flip"), "").unwrap();
    run_proof(&store, &val);
    assert!(unstable(&store, &val).is_some(), "precondition: flagged");

    run_proof(&store, &val); // fails again — consistent with the run before it
    assert!(
        unstable(&store, &val).is_some(),
        "agreement is not evidence the proof became deterministic; only a person \
         adjudicating it clears the record"
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

/// **A mostly-passing flake stays flagged through the greens that follow.**
///
/// This began as a characterization test of a real defect: the flag used to
/// clear on any two consecutive agreeing runs, so the INV-8 shape — failing
/// roughly one run in five — would be wiped by the greens that arrive
/// constantly, before anyone saw it. The flag is now sticky and only the
/// adjudication lifecycle clears it, so the assertion is inverted.
#[test]
fn a_mostly_passing_flake_stays_flagged_through_later_greens() {
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
    run_proof(&store, &val); // pass again — must NOT clear
    assert!(
        unstable(&store, &val).is_some(),
        "a flake that mostly passes is still a flake — greens must not erase the record"
    );
}

/// **Editing the TEST must not hide a flake in the code under test.**
///
/// Characterization of a real defect, now inverted: the digest was built from
/// `files_grounding` (both roles), so touching a verifies-role test reset it and
/// suppressed detection — including of a flake that very edit might have
/// introduced. It is built from `files_realizing` now. Expiry and flake
/// discrimination use the same files for opposite purposes.
#[test]
fn a_test_file_edit_does_not_suppress_flake_detection() {
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
        unstable(&store, &val.id).is_some(),
        "editing the test must not reset the digest — the code under test did not move, \
         and the flake may be the very thing that edit introduced"
    );
}

/// **passed → blocked is the harness failing, not the proof wavering.**
///
/// Characterization of a real defect, now inverted. `blocked` means loom could
/// not observe at all — a timeout, a missing binary, its own lock. Recording
/// that as instability conflates "could not run" with "disagreed with itself",
/// and dilutes the signal until nobody reads it.
#[test]
fn passed_to_blocked_is_not_flagged_as_instability() {
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
    assert!(
        unstable(&store, &val).is_none(),
        "loom could not observe at all — that is the harness failing, not the proof \
         disagreeing with itself"
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
        .set_facet(
            &ga.id,
            TargetKind::Edge,
            "locator",
            "fn a",
            TruthClass::Asserted,
        )
        .unwrap();
    let gb = store
        .add_edge(EdgeKind::Implements, &ib.id, &cfb.id, TruthClass::Asserted)
        .unwrap();
    store
        .set_facet(
            &gb.id,
            TargetKind::Edge,
            "locator",
            "fn b",
            TruthClass::Asserted,
        )
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

/// Attack: sticky + "clear on realizing-file change" lets an edit hide a flake.
///
/// After a flip is flagged, editing the realizing file is the ONLY automatic
/// reset. A genuinely flaky proof whose outcome still depends on outside state
/// loses its record the moment anyone (or rustfmt, or a drive-by edit) touches
/// the realizing surface — even when the flake trigger is untouched and the
/// next run still fails.
#[test]
fn a_realizing_file_edit_clears_a_sticky_flake_record() {
    let tmp = Tmp::new();
    let (store, val) = seeded(&tmp, "sh -c 'test ! -f flip'");
    run_proof(&store, &val); // pass
    std::fs::write(tmp.path().join("flip"), "").unwrap();
    run_proof(&store, &val); // fail → sticky flag
    assert!(unstable(&store, &val).is_some(), "precondition: flagged");

    // Touch the realizing file only (whitespace). Flake trigger still present.
    std::fs::write(tmp.path().join("s.rs"), "pub fn a() {}\n// touch\n").unwrap();
    run_proof(&store, &val); // still fails

    assert_eq!(store.get_node(&val).unwrap().unwrap().status, "failed");
    assert!(
        unstable(&store, &val).is_none(),
        "editing the realizing file cleared the sticky flake record while the \
         proof was still failing for a reason outside that file"
    );
}
