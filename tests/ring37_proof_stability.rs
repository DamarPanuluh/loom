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

/// **An intervening non-comparable status does not hide a real flip.**
///
/// Characterization of a real hole, now inverted. `comparable()` still admits
/// only passed|failed, but the comparison no longer reads the node's CURRENT
/// status — it reads the last SETTLED outcome, held in its own facet. A ripple
/// reset to `not_run` (or a redefinition, which leaves the realizing code
/// untouched) therefore cannot launder a passed→failed flake by standing
/// between the two runs. Same shape for passed→blocked→failed.
#[test]
fn an_intervening_not_run_does_not_hide_a_passed_to_failed_flip() {
    let tmp = Tmp::new();
    let (store, val) = seeded(&tmp, "sh -c 'test ! -f flip'");
    run_proof(&store, &val);
    assert_eq!(store.get_node(&val).unwrap().unwrap().status, "passed");

    // What sync/ripple does before the next observe — not a settled outcome,
    // and deliberately exempt from stability recording.
    store.set_node_status(&val, "not_run").unwrap();

    std::fs::write(tmp.path().join("flip"), "").unwrap();
    run_proof(&store, &val);

    assert_eq!(store.get_node(&val).unwrap().unwrap().status, "failed");
    assert!(
        unstable(&store, &val).is_some(),
        "a reset standing between two runs must not launder the flip — the \
         comparison is against the last settled outcome, not the current status"
    );
}

/// **A sibling intent's code moving does not clear a shared proof's record.**
///
/// Characterization of a real defect, now inverted by removing the last
/// automatic reset. Its companion below covers the other half — a sibling no
/// longer suppresses FLAGGING a fresh flip either, which it did for as long as
/// the anchors were one unioned digest (finding 5c4bc814).
#[test]
fn a_sibling_intent_file_change_does_not_clear_the_shared_proofs_record() {
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
        unstable(&store, &val.id).is_some(),
        "a sibling intent's file moving must not erase a record already taken"
    );
}

/// **A sibling intent's code moving does not excuse a FRESH flip either.**
///
/// The other half of finding 5c4bc814, and the half that made the signal lie.
/// Anchors were one digest unioned over every validated intent, so it changed
/// when ANY of them changed — and a flip in behavior A was read as "the code
/// moved" because somebody edited behavior B. On a shared ring, where flakes
/// actually live, the detector could be silenced by an unrelated commit.
///
/// Now the digest is kept per intent and a flip is excused only when ALL the
/// code the proof exercises moved. A is untouched here, so it is flagged, and
/// the record names A rather than the proof.
#[test]
fn a_sibling_intent_file_change_does_not_excuse_a_fresh_flip() {
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
    assert!(
        unstable(&store, &val.id).is_none(),
        "precondition: one clean run is not a flip"
    );

    // The flip and the sibling edit land together — the commit that touches B
    // while A's proof starts failing. A's code never moved.
    std::fs::write(tmp.path().join("b.rs"), "pub fn b_renamed() {}\n").unwrap();
    std::fs::write(tmp.path().join("flip"), "").unwrap();
    run_proof(&store, &val.id);

    let record = unstable(&store, &val.id)
        .expect("a flip over untouched behavior A is a flake, whatever happened to B");
    assert!(
        record.contains("behavior a"),
        "the record must name the behavior that held still, got: {record}"
    );
    assert!(
        !record.contains("behavior b"),
        "behavior b's code did move, so it is not what went unsteady, got: {record}"
    );
}

/// **A flip is excused only when EVERY validated behavior's code moved.**
///
/// The boundary on the rule above. Both behaviors move, so nothing held still
/// and there is nothing to explain — the proof is not flagged.
#[test]
fn a_flip_is_excused_when_all_validated_behaviors_moved() {
    let tmp = Tmp::new();
    std::fs::write(tmp.path().join("a.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(tmp.path().join("b.rs"), "pub fn b() {}\n").unwrap();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let mut intents = Vec::new();
    for (name, file, locator) in [
        ("behavior a", "a.rs", "fn a"),
        ("behavior b", "b.rs", "fn b"),
    ] {
        let intent = store
            .add_node(
                NodeType::Intent,
                name,
                "",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        let cf = store
            .add_node(NodeType::CodeFile, file, "", "", serde_json::json!({}))
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
                locator,
                TruthClass::Asserted,
            )
            .unwrap();
        intents.push(intent.id);
    }
    let val = store
        .add_node(
            NodeType::Validation,
            "shared proof",
            "",
            "not_run",
            serde_json::json!({ "type": "test", "command": "sh -c 'test ! -f flip'" }),
        )
        .unwrap();
    for intent in &intents {
        store
            .add_edge(EdgeKind::Validates, &val.id, intent, TruthClass::Asserted)
            .unwrap();
    }

    run_proof(&store, &val.id);
    // Every behavior the proof covers moved, alongside the flip.
    std::fs::write(tmp.path().join("a.rs"), "pub fn a_renamed() {}\n").unwrap();
    std::fs::write(tmp.path().join("b.rs"), "pub fn b_renamed() {}\n").unwrap();
    std::fs::write(tmp.path().join("flip"), "").unwrap();
    run_proof(&store, &val.id);

    assert!(
        unstable(&store, &val.id).is_none(),
        "when all the code moved, a different outcome is the system working"
    );
}

/// **An edit to the realizing code does not wipe a live flake record.**
///
/// Characterization of a real defect, now inverted. "Clear on realizing-file
/// change" was the last automatic reset, and it meant rustfmt or a drive-by
/// whitespace touch erased the evidence while the flake trigger sat untouched
/// and the next run still failed. Nothing clears the flag automatically now;
/// only adjudication does.
#[test]
fn a_realizing_file_edit_does_not_wipe_a_sticky_flake_record() {
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
        unstable(&store, &val).is_some(),
        "an incidental edit must not erase evidence that the proof is \
         non-deterministic — the flake trigger was untouched and it still failed"
    );
}

/// **Every path that settles a proof's status goes through the stability door.**
///
/// Structural, and it reads loom's own source, because this is a claim about
/// WHERE code may live and no runtime assertion can defend it. The first
/// version of stability tracking guarded `validation run` alone — and
/// `journey run` settles its status directly, so the proofs covering every
/// user-visible behavior were never watched.
///
/// The FIRST version of this test had the same shape of hole it exists to
/// catch: it scanned only the files it already knew about, so a new settling
/// path in a new file would have been invisible to it. It then walked every
/// file under src/ but classified non-settling sites by FILE COUNT — and that
/// was defeatable: remove one exempt call and add a settling one in the same
/// file, the count holds, and the new settling path is never checked for
/// `record_proof_stability` because the file is not on the settles list.
///
/// Per-call markers close that. Every `set_node_status` call must either
/// (a) sit in a settling file with `record_proof_stability` above it, or
/// (b) carry a `// loom-stability-exempt: …` marker in the eight lines above.
/// Swapping an exempt call for an unmarked settling one fails the marker
/// check; a new file with an unmarked call fails the same way.
#[test]
fn every_proof_status_write_records_stability_first() {
    // Files that settle a PROOF's outcome. Each call here must be preceded by
    // Store::record_proof_stability.
    let settles_a_proof: &[&str] = &["src/commands/proof_cmd.rs", "src/journey.rs"];

    let mut offenders: Vec<String> = Vec::new();
    walk(std::path::Path::new("src"), settles_a_proof, &mut offenders);
    assert!(
        offenders.is_empty(),
        "unaccounted proof-status writes:\n{}",
        offenders.join("\n")
    );

    fn walk(path: &std::path::Path, settles: &[&str], out: &mut Vec<String>) {
        if path.is_dir() {
            for e in std::fs::read_dir(path).unwrap().flatten() {
                walk(&e.path(), settles, out);
            }
            return;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            return;
        }
        let rel = path.to_string_lossy().replace('\\', "/");
        let src = std::fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            // The definition and doc references are not call sites.
            if !line.contains("set_node_status(") || t.starts_with("//") || t.starts_with("pub fn")
            {
                continue;
            }
            let from = i.saturating_sub(8);
            let window = lines[from..=i].join("\n");
            let settles_here = settles.iter().any(|f| rel.ends_with(f));
            if settles_here {
                if !window.contains("record_proof_stability") {
                    out.push(format!(
                        "  {rel}:{} settles a proof without record_proof_stability above it",
                        i + 1
                    ));
                }
                continue;
            }
            if !window.contains("loom-stability-exempt:") {
                out.push(format!(
                    "  {rel}:{} moves node status without recording stability and without a \
                     `// loom-stability-exempt: <why>` marker — if it settles a proof, call \
                     record_proof_stability first (and list this file as settling); if not, \
                     mark it exempt with a reason",
                    i + 1
                ));
            }
        }
    }
}

/// **Running a proof leaves a grade that reflects the run.**
///
/// `regrade` lived only in `observe_validation`, and the `loom validation run`
/// CLI dispatches around it — so the documented way to run a proof recorded the
/// outcome and left the strength facet at its pre-run value (finding 1b274ada).
/// It is a false-green in the other direction: `sync` grades a reset validation
/// S0, the run passes it, the S0 stands, and `proven` reports unproven intents
/// while every proof is green. Observed on this graph as 26 stale S0 grades that
/// a bare `loom sync` corrected with no proof re-run.
///
/// Asserted through the same seam the CLI uses, so a future dispatch that skips
/// the regrade fails here rather than in a dogfood three commits later.
#[test]
fn running_a_proof_through_the_cli_path_leaves_a_fresh_grade() {
    let tmp = Tmp::new();
    let (store, val) = seeded(&tmp, "true");
    // Grade it S0 by hand, the state `sync` leaves behind on a reset proof.
    store
        .set_facet(
            &val,
            TargetKind::Node,
            "proof_strength",
            &serde_json::json!({ "grade": "S0", "ran_and_passed": false }).to_string(),
            TruthClass::Derived,
        )
        .unwrap();
    let root = tmp.path().to_path_buf();
    drop(store);

    // Through the real dispatch, not the library helper: the bypass being
    // guarded lives in `ValidationCmd::Run`'s early return, so a test that
    // called `observe_validation` directly would pass while the CLI stayed
    // broken — which is exactly how this survived.
    loom::commands::run(loom::cli::Cli {
        graph: Some(root.clone()),
        json: true,
        command: Some(loom::cli::Command::Validation {
            cmd: loom::cli::ValidationCmd::Run {
                key: val.clone(),
                all: false,
            },
        }),
    })
    .expect("the proof runs");

    let store = Store::open(&root).unwrap();
    let graded = store
        .get_facet(&val, TargetKind::Node, "proof_strength")
        .unwrap()
        .expect("the run records a grade");
    let grade: serde_json::Value = serde_json::from_str(&graded).unwrap();
    assert_ne!(
        grade["grade"], "S0",
        "a passing run must lift the grade off S0, not leave the figure from before it: {graded}"
    );
    assert_eq!(
        grade["ran_and_passed"], true,
        "the witness must reflect the run that just happened: {graded}"
    );
}
