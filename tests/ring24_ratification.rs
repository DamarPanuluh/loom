//! Ring 24 — ratification, inverted.
//!
//! The failure this ring exists to prevent: 39 of 51 ratifications in loom's
//! own graph were fabricated, 30 of them sharing one timestamp minute. The
//! cause was not malice — the one rung requiring human attention blocked the
//! five above it, so a worker facing 51 challenge prompts produced 51 records
//! instead of stopping. Wantedness is now earned from evidence by default, and
//! the human is asked only where evidence and judgment diverge.

use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::ratification::{effective, DeFactoWitness, Ratification};
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

/// The structural guarantee: `de_facto` is derived, so there is no path from
/// caller input to it. A worker cannot declare a behavior wanted by writing the
/// facet loom computes.
#[test]
fn de_facto_cannot_be_asserted() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "some behavior");

    let err = store
        .set_facet(
            &i.clone(),
            TargetKind::Node,
            "de_facto",
            r#"{"demonstrated_in":"src/a.rs","proven_by":"t","used_by":"u","usage_hops":0}"#,
            TruthClass::Asserted,
        )
        .expect_err("asserting a derived key must be refused");
    assert!(
        format!("{err}").contains("derived"),
        "the refusal must say why: {err}"
    );

    // And the reserved ratification keys stay closed — the door that let a
    // ratification exist with no evidence and no journal behind it.
    for key in ["ratification", "ratified_by", "ratified_at"] {
        assert!(
            store
                .set_facet(&i, TargetKind::Node, key, "ratified", TruthClass::Asserted)
                .is_err(),
            "'{key}' must not be writable as a facet"
        );
    }
}

/// Wantedness is earned. All three conjuncts, or it is silence.
#[test]
fn de_facto_needs_all_three_conjuncts() {
    let full = DeFactoWitness {
        demonstrated_in: Some("src/a.rs".into()),
        proven_by: Some("a proof".into()),
        used_by: Some("exercised directly".into()),
        usage_hops: 0,
    };
    assert_eq!(
        effective(Ratification::Unratified, Some(&full)),
        Ratification::DeFacto
    );
    for drop_one in 0..3 {
        let mut w = full.clone();
        match drop_one {
            0 => w.demonstrated_in = None,
            1 => w.proven_by = None,
            _ => w.used_by = None,
        }
        assert_eq!(
            effective(Ratification::Unratified, Some(&w)),
            Ratification::Unratified,
            "two of three conjuncts is silence about the third"
        );
    }
}

/// A rejection is absolute. This is the clause that makes `loom intent reject`
/// safe to use freely: nothing the code does can quietly undo it.
#[test]
fn no_evidence_resurrects_a_rejected_behavior() {
    let full = DeFactoWitness {
        demonstrated_in: Some("src/a.rs".into()),
        proven_by: Some("a proof".into()),
        used_by: Some("exercised directly".into()),
        usage_hops: 0,
    };
    assert_eq!(
        effective(Ratification::Rejected, Some(&full)),
        Ratification::Rejected
    );
}

/// A ratification must point at the moment it happened. Prose alone is the
/// exact shape of the 39 fabricated records.
#[test]
fn a_ratification_needs_more_than_a_sentence() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "a wanted behavior");

    // loom journals the act itself before stamping, so an honest ratification
    // anchors on that entry — and the prose is still checked for substance, or
    // every ratification would self-anchor on the entry loom just wrote.
    store
        .ratify_intent(
            &i,
            "the product owner asked for this in the Q3 review",
            "tty",
        )
        .expect("a substantive utterance ratifies");
    assert_eq!(store.ratification(&i).unwrap(), "ratified");

    let j = intent(&store, "another behavior");
    let err = store
        .ratify_intent(&j, "…", "tty")
        .expect_err("placeholder prose is not evidence");
    assert!(format!("{err}").contains("substantive"), "{err}");
}

/// Rejecting mints removal work rather than deleting: the code still performs
/// the behavior, and that is a fact about the repo, not an opinion.
#[test]
fn rejecting_turns_live_code_into_tracked_work() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "a behavior nobody wants");
    let cf = codefile(&store, "src/unwanted.rs");
    store
        .add_edge(EdgeKind::Implements, &i, &cf.id, TruthClass::Asserted)
        .unwrap();

    store
        .reject_intent(&i, "this duplicates checkout and confuses users", "tty")
        .unwrap();
    assert_eq!(store.ratification(&i).unwrap(), "rejected");

    // The store-level write records the judgment; the CLI handler is what mints
    // the findings, so assert the judgment survives and is absolute here.
    assert_eq!(
        loom::ratification::effective_for(&store, &i).unwrap(),
        Ratification::Rejected
    );
}

/// The presence rule, stated as a test: an agent lane is never a person, and
/// neither is an unset agent in automation.
#[test]
fn an_llm_lane_may_author_everything_and_ratify_nothing() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "a behavior an agent built");
    store.set_agent(loom::store::Agent::Lane(loom::registry::OwnerRole::Builder));

    let err = store
        .ratify_intent(&i, "the agent decided this was wanted", "tty")
        .expect_err("INV-8: an llm lane may not ratify");
    assert!(format!("{err}").contains("INV-8"), "{err}");

    // Nor may it reject — the authority is symmetric.
    assert!(store
        .reject_intent(&i, "the agent decided this was unwanted", "tty")
        .is_err());
}

/// The regression that defines this whole redesign.
///
/// Fifty behaviors nobody has said yes to yet, plus one that is demonstrably
/// happening and that users can see. The old `wanted` rung counted 51 and
/// served 51 challenge prompts. The divergence queue counts the one thing that
/// is actually a question.
#[test]
fn fifty_unratified_intents_are_not_fifty_questions() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    for n in 0..50 {
        store
            .add_node(
                NodeType::Intent,
                &format!("planned behavior {n}"),
                "not built yet",
                "planned",
                serde_json::json!({}),
            )
            .unwrap();
    }

    // One behavior that IS happening, and that users can see.
    let visible = intent(&store, "users can cancel an order");
    store
        .set_facet(
            &visible,
            TargetKind::Node,
            "visibility",
            "user_visible",
            TruthClass::Asserted,
        )
        .unwrap();
    earn_call_witness(&store, tmp.path(), &visible);
    let g = store
        .edges_with(Some(EdgeKind::Implements), Some(&visible), None)
        .unwrap()
        .into_iter()
        .find(|e| store.grounding_role(&e.id).unwrap() == loom::model::GroundingRole::Realizes)
        .expect("a realizing grounding");
    store
        .record_verdict(
            &g.id,
            loom::model::InspectionStatus::Passing,
            "the behavior lives here",
            "src/behavior.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    s3_journey_proof(&store, tmp.path(), &visible, "cancel journey");
    loom::sync::run(&store, tmp.path()).unwrap();

    let all = loom::divergence::all(&store).unwrap();
    let blocking: Vec<_> = all.iter().filter(|d| d.blocking).collect();
    assert!(
        blocking.len() <= 2,
        "the human is asked about what diverges, not about every unbuilt intent: {blocking:#?}"
    );
    assert!(
        loom::workitem::unratified_intents(&store).unwrap().len() >= 50,
        "the intents are still honestly unratified — they just are not questions"
    );
}
