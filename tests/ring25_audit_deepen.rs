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
            .record_finding_verdict(
                &f.id,
                "justified",
                "looked at it",
                &format!("src/f{n}.rs:1"),
            )
            .unwrap();
    }

    let found = loom::audit::run(&store).unwrap();
    assert!(
        found.iter().any(|f| f.kind == "judgment_burst"),
        "judgments too fast to have been made one at a time must be reported: {found:#?}"
    );
    let depths = loom::maturity::depths(&store).unwrap();
    let roster = loom::workitem::queue_items(&store, loom::lane::Lane::Audit).unwrap();
    assert_eq!(depths.get(loom::lane::Lane::Audit), found.len());
    assert_eq!(roster.len(), found.len());
    let packet = loom::workitem::next(&store, Some(loom::lane::Lane::Audit))
        .unwrap()
        .unwrap();
    assert_eq!(packet.target.kind, "graph");
}

#[test]
fn a_human_can_accept_exact_history_without_authorizing_or_rewriting_it() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    for n in 0..loom::audit::BURST_THRESHOLD + 2 {
        let f = store
            .add_node(
                NodeType::Finding,
                &format!("historical finding {n}"),
                "flagged",
                "code_audit",
                serde_json::json!({ "kind": "code_audit" }),
            )
            .unwrap();
        let cf = codefile(&store, &format!("src/historical{n}.rs"));
        store.add_derived_edge(EdgeKind::Flags, &f.id, &cf.id).ok();
        store
            .record_finding_verdict(
                &f.id,
                "justified",
                "historical bulk judgment",
                &format!("src/historical{n}.rs:1"),
            )
            .unwrap();
    }

    let facts_before = store
        .all_facts()
        .unwrap()
        .into_iter()
        .filter(|fact| fact.claim == Claim::Adjudication)
        .collect::<Vec<_>>();
    let first = facts_before.first().unwrap();
    let minute: String = first.asserted_at.chars().take(16).collect();
    let bucket = loom::audit::JudgmentBurstBucket::for_key(
        &store,
        &first.asserted_by,
        &minute,
        loom::batch_auth::BatchClaim::Adjudication,
    )
    .unwrap()
    .unwrap();
    let decision = loom::ratification::HumanDecision::mediated(
        "Approved as a disclosed historical incident, not an authorization",
    )
    .unwrap();
    let incident = loom::audit::AuditIncident::accept(
        &bucket,
        "the burst lacked contemporaneous batch authorization and remains disclosed",
        decision,
    )
    .unwrap();

    // Imported history is visible but cannot satisfy this graph's local human
    // disposition requirement.
    loom::journal::restore_entries(
        tmp.path(),
        &[loom::journal::Entry {
            id: "imported-incident".into(),
            ts: loom::journal::now_iso(),
            actor: "llm:analyzer".into(),
            profile: Some("loom-auditor".into()),
            event: loom::audit::INCIDENT_EVENT.into(),
            target_id: incident.incident_digest.clone(),
            payload: serde_json::to_value(&incident).unwrap(),
            origin: loom::journal::Origin::Local,
        }],
    )
    .unwrap();
    assert!(loom::audit::run(&store)
        .unwrap()
        .iter()
        .any(|finding| finding.kind == "judgment_burst"));

    let entry = store
        .append_journal(
            loom::audit::INCIDENT_EVENT,
            &incident.incident_digest,
            serde_json::to_value(&incident).unwrap(),
        )
        .unwrap();
    assert!(loom::audit::run(&store)
        .unwrap()
        .iter()
        .all(|finding| finding.kind != "judgment_burst"));
    assert_eq!(
        loom::batch_auth::covering_envelope(
            &store,
            &bucket.subjects,
            bucket.claim,
            &bucket.actor,
            &bucket.minute,
            &bucket.batch_ids,
            bucket.latest_assertion_millis,
        )
        .unwrap(),
        None,
        "accepted history must never become batch authorization"
    );
    let facts_after = store
        .all_facts()
        .unwrap()
        .into_iter()
        .filter(|fact| fact.claim == Claim::Adjudication)
        .collect::<Vec<_>>();
    assert_eq!(
        facts_before, facts_after,
        "acceptance must not rewrite facts"
    );
    assert!(facts_after.iter().all(|fact| {
        fact.decision_mode == loom::model::DecisionMode::Individual && fact.batch_id.is_empty()
    }));
    assert_eq!(entry.target_id, incident.incident_digest);
    let disclosures = loom::audit::incident_entries(&store).unwrap();
    assert_eq!(
        disclosures.len(),
        2,
        "local and imported history stay visible"
    );
    assert_eq!(
        disclosures
            .iter()
            .filter(|(entry, _)| entry.origin == loom::journal::Origin::Local)
            .count(),
        1
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
            || store.get_edge(&e.id).unwrap().is_some_and(|edge| {
                edge.status == loom::model::InspectionStatus::NeedsReverification
            }),
        "either the claim re-opened or the audit names it: {found:#?}"
    );
}

#[test]
fn a_reopened_stale_edge_is_not_duplicated_as_audit_work() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "a behavior whose implementation can move");
    let cf = codefile(&store, "src/moving.rs");
    let e = store
        .add_edge(EdgeKind::Implements, &i, &cf.id, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &e.id,
            loom::model::InspectionStatus::Passing,
            "the behavior resolves here",
            "src/moving.rs:1",
            0.9,
            "llm",
        )
        .unwrap();

    std::fs::remove_file(tmp.path().join("src/moving.rs")).unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        store.get_edge(&e.id).unwrap().unwrap().status,
        loom::model::InspectionStatus::NeedsReverification
    );
    assert!(loom::audit::run(&store)
        .unwrap()
        .iter()
        .all(|finding| finding.subject.id() != Some(e.id.as_str())));
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
    store
        .ratify_intent(&i, "asked for in review", "tty")
        .unwrap();
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
        .set_facet(
            &e.id,
            TargetKind::Edge,
            "role",
            "verifies",
            TruthClass::Asserted,
        )
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
        &store,
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
            &store,
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

/// A packet about an already-settled target still converts when later
/// qualifying work lands. Earliest-fact tracking permanently failed those.
#[test]
fn efficacy_credits_reverification_after_an_earlier_fact() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "a behavior revisited");
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
        &store,
        &[loom::packet::Served {
            id: "pkt-revisit".into(),
            kind: "analyze".into(),
            target: i.clone(),
        }],
    )
    .unwrap();

    // Before re-work: already-settled, not converted.
    let before = loom::audit::efficacy(&store).unwrap();
    assert_eq!(before.served, 1);
    assert_eq!(before.converted, 0);

    // Later qualifying write on the same subject — must count.
    std::thread::sleep(std::time::Duration::from_millis(2));
    store
        .record_verdict(
            &edge.id,
            loom::model::InspectionStatus::Passing,
            "still lives here after revisit",
            "src/thing.rs:1",
            0.95,
            "llm",
        )
        .unwrap();

    let after = loom::audit::efficacy(&store).unwrap();
    assert_eq!(after.served, 1);
    assert_eq!(
        after.converted, 1,
        "re-verification after the packet must convert: {after:?}"
    );
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
        .set_facet(
            &e.id,
            TargetKind::Edge,
            "role",
            "verifies",
            TruthClass::Asserted,
        )
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

/// **A parked verdict is not a burst.**
///
/// A Finding is a DERIVED node with a deterministic id: `sync` wipes and
/// rebuilds it every run, and the adjudication on that id deliberately outlives
/// it so the verdict re-attaches when the finding recurs
/// (`wipe_structural_findings` deletes derived finding NODES and never touches
/// the fact table). A verdict whose subject is not currently derived is
/// therefore parked, not lost — and this finding's remedy, "re-open them and
/// judge them individually", cannot be carried out on a subject that is absent.
///
/// Counting them produced a burst no action could close: 44 parked verdicts
/// held loom's own `sound` rung open with no move available.
#[test]
fn a_burst_whose_subjects_are_not_currently_derived_is_not_reported() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    for n in 0..loom::audit::BURST_THRESHOLD + 5 {
        let cf = codefile(&store, &format!("src/f{n}.rs"));
        let f = store
            .add_derived_node(
                NodeType::Finding,
                &format!("burst-parked-{n}"),
                &format!("finding {n}"),
                "flagged",
                "code_audit",
                serde_json::json!({ "kind": "code_audit", "file": format!("src/f{n}.rs") }),
            )
            .unwrap();
        store.add_derived_edge(EdgeKind::Flags, &f.id, &cf.id).ok();
        store
            .record_finding_verdict(
                &f.id,
                "justified",
                "looked at it",
                &format!("src/f{n}.rs:1"),
            )
            .unwrap();
    }
    assert!(
        loom::audit::run(&store)
            .unwrap()
            .iter()
            .any(|f| f.kind == "judgment_burst"),
        "precondition: while the subjects are derived, the burst IS reported"
    );

    // Exactly what a sync rebuild does when the flagged condition stops holding.
    store.wipe_structural_findings().unwrap();

    let found = loom::audit::run(&store).unwrap();
    assert!(
        !found.iter().any(|f| f.kind == "judgment_burst"),
        "a burst with nothing left to re-open must not be reported: {found:#?}"
    );

    // Nothing was erased to achieve that — the verdicts are still in the graph,
    // waiting to re-attach if the finding recurs.
    let parked = store
        .all_facts()
        .unwrap()
        .into_iter()
        .filter(|f| f.claim == loom::model::Claim::Adjudication)
        .count();
    assert_eq!(
        parked,
        loom::audit::BURST_THRESHOLD + 5,
        "the adjudications are preserved, not deleted — they are parked"
    );
}

/// A seal appended after the final fact is retrospective, even inside the same
/// minute, and therefore cannot close the burst.
#[test]
fn a_retrospective_batch_authorization_does_not_close_the_burst() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    let mut subjects = Vec::new();
    for n in 0..loom::audit::BURST_THRESHOLD + 2 {
        let f = store
            .add_node(
                NodeType::Finding,
                &format!("batch finding {n}"),
                "flagged",
                "code_audit",
                serde_json::json!({ "kind": "code_audit" }),
            )
            .unwrap();
        let cf = codefile(&store, &format!("src/b{n}.rs"));
        store.add_derived_edge(EdgeKind::Flags, &f.id, &cf.id).ok();
        store
            .record_finding_verdict(
                &f.id,
                "rejected",
                "environment-contaminated unstable_proof",
                &format!("src/b{n}.rs:1"),
            )
            .unwrap();
        subjects.push(f.id);
    }
    assert!(
        loom::audit::run(&store)
            .unwrap()
            .iter()
            .any(|f| f.kind == "judgment_burst"),
        "precondition: unexplained burst is reported"
    );

    let digest = loom::batch_auth::subject_digest(&subjects);
    let pre = store
        .append_journal(
            "batch_apply",
            &digest,
            serde_json::json!({
                "operation": "adjudicate",
                "subjects": subjects,
                "routing_class": "env_contaminated_unstable_proof",
            }),
        )
        .unwrap();
    let facts = store.all_facts().unwrap();
    let minute: String = facts
        .iter()
        .find(|f| f.claim == Claim::Adjudication)
        .unwrap()
        .asserted_at
        .chars()
        .take(16)
        .collect();
    let envelope = loom::batch_auth::BatchAuthorization::seal(
        loom::batch_auth::BatchClaim::Adjudication,
        "verdict",
        subjects.clone(),
        "llm",
        "llm",
        "environment-contaminated unstable_proof findings falsified by clean reruns",
        vec![format!("journal:{}", pre.id)],
    )
    .unwrap()
    .with_routing_class("env_contaminated_unstable_proof")
    .with_time_bounds(format!("{minute}:00.000Z"), format!("{minute}:59.999Z"));
    let entry = loom::batch_auth::append_envelope(&store, &envelope).unwrap();
    store
        .stamp_batch_ids(&subjects, Claim::Adjudication, &entry.id)
        .unwrap();

    let found = loom::audit::run(&store).unwrap();
    assert!(
        found.iter().any(|f| f.kind == "judgment_burst"),
        "an envelope appended after the facts must not retrospectively close the burst: {found:#?}"
    );
    let stamped = store
        .all_facts()
        .unwrap()
        .into_iter()
        .filter(|f| f.claim == Claim::Adjudication)
        .collect::<Vec<_>>();
    assert!(
        stamped
            .iter()
            .all(|f| f.decision_mode == loom::batch_auth::DecisionMode::Batch
                && f.batch_id == entry.id),
        "stamping retains batch provenance even though retrospective authorization is not trusted"
    );
    // asserted_at must not have been rewritten.
    assert!(
        stamped.iter().all(|f| f.asserted_at.starts_with(&minute)),
        "stamping must not rewrite asserted_at"
    );
}

#[test]
fn prose_only_batch_evidence_is_refused() {
    let mut envelope = loom::batch_auth::BatchAuthorization::seal(
        loom::batch_auth::BatchClaim::Adjudication,
        "verdict",
        vec!["a".to_string(), "b".to_string()],
        "llm",
        "llm",
        "shared predicate",
        vec!["journal:missing-id".to_string()],
    )
    .unwrap();
    // Force prose-only after seal for validate_cover.
    envelope.evidence = vec!["acknowledgment written later".to_string()];
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let envelope_ts = loom::journal::now_iso();
    let subjects = ["a".to_string(), "b".to_string()];
    let reject = loom::batch_auth::validate_cover(
        &store,
        &envelope,
        loom::batch_auth::CoverContext {
            envelope_ts: &envelope_ts,
            envelope_origin: loom::journal::Origin::Local,
            subjects: &subjects,
            claim: loom::batch_auth::BatchClaim::Adjudication,
            burst_minute: "2026-08-04T10:31",
            latest_assertion_millis: i64::MAX,
        },
    )
    .expect_err("prose acknowledgment is not contemporaneous proof");
    assert_eq!(reject, loom::batch_auth::EnvelopeReject::ProseOnlyEvidence);
}

/// Multi-subject ratify seals a batch envelope so the writes do not open a burst.
#[test]
fn ratify_all_seals_a_batch_and_avoids_a_burst() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let mut ids = Vec::new();
    for n in 0..loom::audit::BURST_THRESHOLD + 1 {
        ids.push(intent(&store, &format!("wanted behavior {n}")));
    }
    let decision = loom::ratification::HumanDecision::mediated(
        "ratify the enumerated snapshot — portfolio decision",
    )
    .unwrap();
    let digest = loom::batch_auth::subject_digest(&ids);
    let pre = store
        .append_journal(
            "batch_intent",
            &digest,
            serde_json::json!({
                "operation": "ratify",
                "subjects": ids,
                "human_decision": decision,
            }),
        )
        .unwrap();
    let now = loom::journal::now_iso();
    let envelope = loom::batch_auth::BatchAuthorization::seal(
        loom::batch_auth::BatchClaim::Ratification,
        "ratify",
        ids.clone(),
        "human",
        "solo",
        "portfolio ratification of the enumerated snapshot",
        vec![format!("journal:{}", pre.id)],
    )
    .unwrap()
    .with_time_bounds(&now, &now)
    .with_human_decision(decision.clone());
    let entry = loom::batch_auth::append_envelope(&store, &envelope).unwrap();
    for id in &ids {
        store
            .ratify_intent_from_human_batch(
                id,
                "portfolio ratification of the enumerated snapshot",
                &decision,
                &entry.id,
            )
            .unwrap();
    }
    let found = loom::audit::run(&store).unwrap();
    assert!(
        !found.iter().any(|f| f.kind == "judgment_burst"),
        "authorized batch ratification must not open a judgment_burst: {found:#?}"
    );
    assert!(
        store
            .all_facts()
            .unwrap()
            .into_iter()
            .filter(|f| f.claim == Claim::Ratification)
            .all(|f| f.decision_mode == loom::batch_auth::DecisionMode::Batch),
        "ratification facts retain decision_mode=batch"
    );
}

/// Re-judging burst subjects is NOT a remedy: the fact row for (subject, claim)
/// is an UPSERT keyed on the subject+claim, so a changed re-judgment rewrites
/// `asserted_at` to the current minute — the burst simply relocates (and is
/// re-detected) instead of closing. Identical re-assertions no-op. Either way
/// the audit is never made clean by re-judging.
#[test]
fn re_judging_does_not_close_the_burst() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let mut subjects = Vec::new();
    for n in 0..loom::audit::BURST_THRESHOLD + 2 {
        let cf = store
            .add_node(
                NodeType::CodeFile,
                &format!("src/rerj{n}.rs"),
                "",
                "registered",
                serde_json::json!({}),
            )
            .unwrap();
        let f = store
            .add_derived_node(
                NodeType::Finding,
                &format!("re-judge finding {n}"),
                &format!("finding {n}"),
                "flagged",
                "code_audit",
                serde_json::json!({ "kind": "code_audit", "file": format!("src/rerj{n}.rs") }),
            )
            .unwrap();
        store.add_derived_edge(EdgeKind::Flags, &f.id, &cf.id).ok();
        store
            .record_finding_verdict(
                &f.id,
                "justified",
                "bulk reason",
                &format!("src/rerj{n}.rs:1"),
            )
            .unwrap();
        subjects.push(f.id);
    }
    let found = loom::audit::run(&store).unwrap();
    assert!(
        found.iter().any(|f| f.kind == "judgment_burst"),
        "precondition: unexplained burst is reported"
    );

    // Identical re-assertions are a byte-identical no-op — the burst persists
    // unchanged, same minute.
    for id in &subjects {
        store
            .record_finding_verdict(id, "justified", "bulk reason", "src/rerj0.rs:1")
            .unwrap();
    }
    let found = loom::audit::run(&store).unwrap();
    assert!(
        found.iter().any(|f| f.kind == "judgment_burst"),
        "identical re-assertion must not close the burst: {found:#?}"
    );

    // Changed re-judgments rewrite asserted_at to now. Done in one pass the
    // burst is re-detected at the current minute — audit still not clean.
    for id in &subjects {
        store
            .record_finding_verdict(
                id,
                "justified",
                "a fresh individual reason",
                "src/rerj0.rs:1",
            )
            .unwrap();
    }
    let found = loom::audit::run(&store).unwrap();
    assert!(
        found.iter().any(|f| f.kind == "judgment_burst"),
        "changed re-judgments must relocate, not close, the burst: {found:#?}"
    );
}

/// Option B: a burst CAN be closed retrospectively when a HUMAN vouches and
/// the seal cites a trusted human-gated `batch_intent` record that PREDATES
/// the burst's final fact and binds the exact subject digest — the append-only
/// journal timestamp proves the Q&A happened before the judgments. Machine
/// records (batch_apply), forged event names without a HumanDecision, wrong
/// digests, and the burst actor's own later seal all still fail closed.
#[test]
fn a_human_seal_over_a_pre_burst_record_closes_the_burst() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    let mut subjects = Vec::new();
    for n in 0..loom::audit::BURST_THRESHOLD + 2 {
        let i = intent(&store, &format!("wanted behavior {n}"));
        subjects.push(i);
    }
    // The Q&A record lands FIRST, before any burst fact exists — the ordering
    // proof. It must BE the trusted human authorization act: event
    // `batch_intent` (as `loom intent ratify --all` writes), target_id = the
    // subject digest of the exact burst, payload carrying a HumanDecision.
    let digest = loom::batch_auth::subject_digest(&subjects);
    let decision = loom::ratification::HumanDecision::mediated(
        "the human reviewed the enumerated snapshot and stands behind it",
    )
    .unwrap();
    let record = store
        .append_journal(
            "batch_intent",
            &digest,
            serde_json::json!({
                "operation": "ratify",
                "subjects": subjects,
                "human_decision": decision,
                "evidence": "portfolio review of the enumerated snapshot",
            }),
        )
        .unwrap();
    // NOW the burst facts land, after the human record exists. Ratifications
    // are human-gated, so route them through the direct-ratify path.
    for id in &subjects {
        store
            .ratify_intent(id, "portfolio review of the enumerated snapshot", "tty")
            .unwrap();
    }
    assert!(
        loom::audit::run(&store)
            .unwrap()
            .iter()
            .any(|f| f.kind == "judgment_burst"),
        "precondition: unexplained burst is reported"
    );

    // Retrospective seal: stamped now (after the facts), authority HUMAN,
    // citing the pre-burst batch_intent record.
    let facts = store.all_facts().unwrap();
    let minute: String = facts
        .iter()
        .find(|f| f.claim == Claim::Ratification)
        .unwrap()
        .asserted_at
        .chars()
        .take(16)
        .collect();
    let envelope = loom::batch_auth::BatchAuthorization::seal(
        loom::batch_auth::BatchClaim::Ratification,
        "ratify",
        subjects.clone(),
        "human",
        "solo",
        "the human reviewed the Q&A record and vouches for this snapshot",
        vec![format!("journal:{}", record.id)],
    )
    .unwrap()
    .with_human_decision(
        loom::ratification::HumanDecision::mediated(
            "the human vouches for the enumerated snapshot",
        )
        .unwrap(),
    )
    .with_time_bounds(format!("{minute}:00.000Z"), format!("{minute}:59.999Z"));
    let entry = loom::batch_auth::append_envelope(&store, &envelope).unwrap();
    store
        .stamp_batch_ids(&subjects, Claim::Ratification, &entry.id)
        .unwrap();

    let found = loom::audit::run(&store).unwrap();
    assert!(
        !found.iter().any(|f| f.kind == "judgment_burst"),
        "a human seal over a pre-burst bound human authorization must close the burst: {found:#?}"
    );
}
