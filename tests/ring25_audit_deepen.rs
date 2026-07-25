//! Ring 25 — the self-audit, and the queue that never empties.
//!
//! On this repo the audit reports clean, and that is NOT vindication: the v2→v3
//! migration erased every asserted verdict, because not one of them was
//! anchored. So the detector has to be proven against a planted signature
//! rather than against our own (now empty) history — otherwise "clean" would
//! only mean "the check does not work".

use loom::model::{Claim, EdgeKind, NodeType, TargetKind, TruthClass};
use loom::store::Store;
mod common;
use common::*;

fn intent(store: &Store, name: &str) -> String {
    store
        .add_node(
            NodeType::Intent,
            name,
            "a behavior",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap()
        .id
}

/// The exact shape of the incident: a `ratified` state with no journal entry
/// behind it. loom writes the entry before stamping, so on a graph this version
/// produced the invariant holds — a violation means the record arrived some
/// other way, and that is worth knowing.
#[test]
fn an_unjournaled_ratification_is_caught() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "a behavior someone claimed was wanted");

    // Honest ratification first: journal entry, then fact. Audit stays quiet.
    store
        .ratify_intent(&i, "the product owner asked for this", "tty")
        .unwrap();
    assert!(
        loom::audit::run(&store).unwrap().is_empty(),
        "a properly journaled ratification is not a finding"
    );

    // Now plant the signature by removing the journal file the fact cites —
    // the same end state as a facet written past the boundary.
    let journal = loom::journal::path(tmp.path());
    std::fs::write(&journal, "").unwrap();

    let found = loom::audit::run(&store).unwrap();
    assert!(
        found.iter().any(|f| f.kind == "unjournaled_ratification"),
        "a ratification with no act behind it must be caught: {found:#?}"
    );
    assert!(
        found.iter().all(|f| !f.remedy.is_empty()),
        "every finding names its own remedy — an audit that only accuses is a scoreboard"
    );
}

/// Thirty judgments in one minute. Nobody reads and decides thirty behaviors in
/// sixty seconds; the cluster at 2026-07-18T19:20 was the tell.
#[test]
fn a_judgment_burst_is_caught() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    for n in 0..loom::audit::BURST_THRESHOLD + 5 {
        let f = store
            .add_node(
                NodeType::Finding,
                &format!("finding {n}"),
                "flagged",
                "code_audit",
                serde_json::json!({ "kind": "code_audit" }),
            )
            .unwrap();
        let cf = codefile(&store, &format!("src/f{n}.rs"));
        store.add_derived_edge(EdgeKind::Flags, &f.id, &cf.id).ok();
        store
            .record_finding_verdict(&f.id, "justified", "looked at it", &format!("src/f{n}.rs:1"))
            .unwrap();
    }

    let found = loom::audit::run(&store).unwrap();
    assert!(
        found.iter().any(|f| f.kind == "judgment_burst"),
        "judgments too fast to have been made one at a time must be reported: {found:#?}"
    );
}

/// A settled claim standing on nothing re-checkable. The spine refuses these at
/// write time, so a hit means the fact arrived by import or carry-forward.
#[test]
fn a_settled_claim_with_no_surviving_anchor_is_caught() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "a grounded behavior");
    let cf = codefile(&store, "src/thing.rs");
    let e = store
        .add_edge(EdgeKind::Implements, &i, &cf.id, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &e.id,
            loom::model::InspectionStatus::Passing,
            "lives here",
            "src/thing.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    assert!(loom::audit::run(&store).unwrap().is_empty());

    // Delete the file the claim points at, then re-verify: every anchor falls.
    std::fs::remove_file(tmp.path().join("src/thing.rs")).unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();

    let found = loom::audit::run(&store).unwrap();
    assert!(
        found.iter().any(|f| f.kind == "unanchored_claim")
            || store.edge_verification(&e.id).unwrap()
                != loom::model::Verification::Expired,
        "either the claim re-opened or the audit names it: {found:#?}"
    );
}

/// `deepen` is `Open`: never met, never unmet, and it cannot block anything.
#[test]
fn the_top_rung_never_completes_and_never_blocks() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "a behavior");
    let cf = codefile(&store, "src/a.rs");
    store
        .add_edge(EdgeKind::Implements, &i, &cf.id, TruthClass::Asserted)
        .unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();

    let ladder = loom::maturity::ladder(&store).unwrap();
    let deepening = ladder
        .rungs
        .iter()
        .find(|r| r.name == "deepening")
        .expect("deepen is on the ladder");
    // Never met, never unmet — so it can never be the reason something else is
    // held up, and "done" is not one of its states.
    assert_eq!(deepening.state, loom::maturity::RungState::Open);
    assert!(ladder
        .rungs
        .iter()
        .all(|r| r.blocked_by.as_deref() != Some("deepening")));
    // It IS post-floor work, so it shows as blocked while any floor is unmet —
    // deepening a graph that has not met its floors is polishing a draft.
    let gate = ladder
        .rungs
        .iter()
        .find(|r| r.state == loom::maturity::RungState::Unmet);
    assert_eq!(deepening.blocked, gate.is_some());
    // And it is LAST: everything else gates before the standing invitation.
    assert_eq!(
        ladder.rungs.last().map(|r| r.name.as_str()),
        Some("deepening")
    );
}

/// The ranking exists to be argued with: every candidate reports its inputs and
/// names one move.
#[test]
fn every_risk_candidate_explains_itself() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "users can cancel an order");
    earn_call_witness(&store, tmp.path(), &i);
    let g = store
        .edges_with(Some(EdgeKind::Implements), Some(&i), None)
        .unwrap()
        .into_iter()
        .find(|e| store.grounding_role(&e.id).unwrap() == loom::model::GroundingRole::Realizes)
        .unwrap();
    store
        .record_verdict(
            &g.id,
            loom::model::InspectionStatus::Passing,
            "lives here",
            "src/behavior.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();

    for c in loom::risk::rank(&store).unwrap() {
        assert!(c.score > 0.0, "a zero-score candidate is not a candidate");
        assert!(!c.why.is_empty(), "the move explains itself");
        assert!(
            c.blast_radius >= 0.0 && c.blast_radius <= 1.0,
            "blast radius is a fraction: {}",
            c.blast_radius
        );
    }
}

/// The fact vocabulary and the audit agree about what a ratification is.
#[test]
fn ratification_claims_are_what_the_audit_reads() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "a behavior");
    store.ratify_intent(&i, "asked for in review", "tty").unwrap();
    let fact = store
        .fact(&loom::store::Subject::Node(i.clone()), Claim::Ratification)
        .unwrap()
        .expect("the ratification is a fact, not a facet");
    assert_eq!(fact.fact.state, "ratified");
    // And the facet door stays shut.
    assert!(store
        .set_facet(
            &i,
            TargetKind::Node,
            "ratification",
            "ratified",
            TruthClass::Asserted
        )
        .is_err());
}

/// A test file is owned when something VERIFIES through it, not when something
/// realizes it. Behaviour is not implemented in `tests/`, so demanding a
/// realizing owner would mean the evidence backbone could only be registered by
/// permanently reddening coverage — which is exactly why 22.8k lines of it
/// stayed outside this graph while coverage reported 67 of 67 owned.
#[test]
fn a_test_file_is_owned_by_what_it_verifies() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "a behavior");

    std::fs::create_dir_all(tmp.path().join("tests")).unwrap();
    std::fs::write(
        tmp.path().join("tests/behavior_test.rs"),
        "pub fn exercises() {}\n",
    )
    .unwrap();
    let cf = store
        .add_node(
            NodeType::CodeFile,
            "tests/behavior_test.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();

    // Registered and unattached: honestly unowned, and the coverage queue says so.
    assert!(
        loom::commands::unowned_names(&store)
            .unwrap()
            .contains(&"tests/behavior_test.rs".to_string()),
        "a test file protecting nothing is real debt"
    );

    // Attached with the VERIFIES role: owned, without pretending the behavior
    // lives there.
    let e = store
        .add_edge(EdgeKind::Implements, &i, &cf.id, TruthClass::Asserted)
        .unwrap();
    store
        .set_facet(&e.id, TargetKind::Edge, "role", "verifies", TruthClass::Asserted)
        .unwrap();
    assert!(
        !loom::commands::unowned_names(&store)
            .unwrap()
            .contains(&"tests/behavior_test.rs".to_string()),
        "what a test verifies IS its ownership"
    );
}

/// Efficacy is derived from the record on both sides, never self-reported.
///
/// The obvious design asks the writer to cite the packet it used — a claim
/// about its own usefulness, made by the party with an interest in it, which is
/// the same shape as an agent reporting that its proof passed.
#[test]
fn efficacy_counts_only_work_that_came_after_the_packet() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    // Nothing served: unmeasured, and explicitly not zero.
    let e = loom::audit::efficacy(&store).unwrap();
    assert_eq!(e.served, 0);
    assert_eq!(e.ratio, 0.0);

    // Work established BEFORE any packet was served cannot be credited to one.
    let i = intent(&store, "a behavior");
    let cf = codefile(&store, "src/thing.rs");
    let edge = store
        .add_edge(EdgeKind::Implements, &i, &cf.id, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &edge.id,
            loom::model::InspectionStatus::Passing,
            "lives here",
            "src/thing.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    loom::packet::serve(
        tmp.path(),
        &[loom::packet::Served {
            id: "pkt-test-1".into(),
            kind: "context".into(),
            target: i.clone(),
        }],
    )
    .unwrap();

    let e = loom::audit::efficacy(&store).unwrap();
    assert_eq!(e.served, 1);
    assert_eq!(
        e.converted, 0,
        "work that already existed is not work the packet enabled"
    );
}

/// The ratio discriminates. A measure that reports the same number whatever
/// happened is not a measure — and this one did exactly that until the two
/// planes were put on one clock.
#[test]
fn the_efficacy_ratio_distinguishes_helped_from_ignored() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let helped = intent(&store, "a behavior work followed");
    let ignored = intent(&store, "a behavior nobody acted on");

    for target in [&helped, &ignored] {
        loom::packet::serve(
            tmp.path(),
            &[loom::packet::Served {
                id: format!("pkt-{target}"),
                kind: "context".into(),
                target: target.clone(),
            }],
        )
        .unwrap();
    }

    // Work lands on ONE of them, after the packets were served.
    let cf = codefile(&store, "src/acted.rs");
    let edge = store
        .add_edge(EdgeKind::Implements, &helped, &cf.id, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &edge.id,
            loom::model::InspectionStatus::Passing,
            "lives here",
            "src/acted.rs:1",
            0.9,
            "llm",
        )
        .unwrap();

    let e = loom::audit::efficacy(&store).unwrap();
    assert_eq!(e.served, 2);
    assert_eq!(
        e.converted, 1,
        "one packet was followed by work and one was not: {e:?}"
    );
    assert!((e.ratio - 0.5).abs() < f64::EPSILON, "{e:?}");
}

/// One definition of the coverage gap, not two that agree by coincidence.
///
/// `code_ownership_summary` carried its own copy of the ownership rule. When
/// the test-file rule landed in `unowned_codefiles` only, `loom coverage` and
/// the `covered` rung disagreed about the same files — the summary said a
/// verified test file was unowned while the queue had already stopped serving
/// it.
#[test]
fn coverage_and_the_rung_count_the_same_files() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "a behavior");

    codefile(&store, "src/thing.rs");
    std::fs::create_dir_all(tmp.path().join("tests")).unwrap();
    std::fs::write(tmp.path().join("tests/thing_test.rs"), "pub fn t() {}\n").unwrap();
    let tf = store
        .add_node(
            NodeType::CodeFile,
            "tests/thing_test.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let e = store
        .add_edge(EdgeKind::Implements, &i, &tf.id, TruthClass::Asserted)
        .unwrap();
    store
        .set_facet(&e.id, TargetKind::Edge, "role", "verifies", TruthClass::Asserted)
        .unwrap();

    let queue = loom::commands::unowned_names(&store).unwrap();
    let (_, _, summary, _) = loom::commands::code_ownership_summary(&store).unwrap();
    assert_eq!(
        queue, summary,
        "the summary and the queue must be the same list, not two derivations of it"
    );
    assert!(
        !queue.contains(&"tests/thing_test.rs".to_string()),
        "a test file with a verifies grounding is owned in BOTH: {queue:?}"
    );
}
