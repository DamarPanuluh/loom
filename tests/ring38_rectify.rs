//! Rectify lane — LLM prep that clears needless ratify friction.
//!
//! Contract: duplicates and un-escalated discoveries route to `rectify`;
//! zombies / meaning-drift / escalated discoveries stay on human `ratify`.
//! INV-8 still denies ratification from an llm:rectify agent.

use loom::lane::Lane;
use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::store::{Agent, Store};

mod common;
use common::{earn_call_witness, Tmp};

fn seed_duplicate_pair(store: &Store) -> (String, String) {
    // Shared realizing file+symbol + empty tags (jaccard 1.0) → DuplicateIntent.
    let a = store
        .add_node(
            NodeType::Intent,
            "alpha behavior for rectify ring",
            "falsifiable alpha",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let b = store
        .add_node(
            NodeType::Intent,
            "beta behavior for rectify ring",
            "falsifiable beta that collides",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = store
        .add_node(
            NodeType::CodeFile,
            "src/rectify_ring.rs",
            "",
            "present",
            serde_json::json!({}),
        )
        .unwrap();
    for intent in [&a.id, &b.id] {
        let edge = store
            .add_edge(EdgeKind::Implements, intent, &cf.id, TruthClass::Asserted)
            .unwrap();
        store
            .set_facet(
                &edge.id,
                TargetKind::Edge,
                "locator",
                "shared",
                TruthClass::Asserted,
            )
            .unwrap();
    }
    (a.id, b.id)
}

#[test]
fn duplicate_routes_to_rectify_not_ratify() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("rectify"), false).unwrap();
    seed_duplicate_pair(&store);

    assert_eq!(
        loom::divergence::rectifiable_count(&store).unwrap(),
        1,
        "one duplicate pair yields one rectifiable divergence"
    );
    assert_eq!(
        loom::divergence::human_blocking_count(&store).unwrap(),
        0,
        "duplicates are not human ratify work"
    );

    let rectify = loom::workitem::next(&store, Some(Lane::Rectify))
        .unwrap()
        .expect("rectify serves the duplicate");
    assert_eq!(rectify.mode, "rectify");
    assert_eq!(rectify.owner_role, "rectify");
    assert!(
        rectify
            .prompt_contract
            .forbidden_actions
            .iter()
            .any(|a| a.contains("ratify")),
        "contract must forbid ratification"
    );
    assert!(
        rectify
            .prompt_contract
            .allowed_actions
            .iter()
            .any(|a| a.contains("scenario-of") || a.contains("visibility") || a.contains("retire")),
        "contract must allow structural prep"
    );

    let ratify = loom::workitem::next(&store, Some(Lane::Divergence)).unwrap();
    assert!(
        ratify.is_none(),
        "human ratify stays empty while only duplicates remain"
    );
}

#[test]
fn human_queue_requires_recorded_evidence_judgment_conflict() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("ratify-conflict"), false).unwrap();
    let missing_approval = store
        .add_node(
            NodeType::Intent,
            "users can download an activity report",
            "an activity report can be downloaded as CSV",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .set_facet(
            &missing_approval.id,
            TargetKind::Node,
            "visibility",
            "user_visible",
            TruthClass::Asserted,
        )
        .unwrap();

    assert!(
        loom::workitem::unratified_intents(&store)
            .unwrap()
            .iter()
            .any(|intent| intent.id == missing_approval.id),
        "missing approval must remain explicit in the wantedness projection"
    );
    assert_eq!(
        loom::divergence::human_blocking_count(&store).unwrap(),
        0,
        "missing approval alone is not an evidence/judgment conflict"
    );
    assert!(
        loom::workitem::next(&store, Some(Lane::Divergence))
            .unwrap()
            .is_none(),
        "a bare unratified intent must not interrupt the human queue"
    );

    let drifted = store
        .add_node(
            NodeType::Intent,
            "users export reports",
            "users export CSV reports",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .ratify_intent(
            &drifted.id,
            "the Q1 product review requested CSV exports",
            "keep CSV exports",
        )
        .unwrap();
    store
        .redefine_intent(&drifted.id, "users export PDF reports")
        .unwrap();

    assert_eq!(
        store.ratification(&drifted.id).unwrap(),
        "needs_reconfirmation",
        "changed meaning must stale the prior wantedness judgment"
    );
    assert_eq!(
        loom::divergence::human_blocking_count(&store).unwrap(),
        1,
        "recorded wantedness against changed meaning is a concrete conflict"
    );
    let ratify = loom::workitem::next(&store, Some(Lane::Divergence))
        .unwrap()
        .expect("the concrete evidence/judgment conflict must enter Ratify");
    assert_eq!(ratify.mode, "ratify");
    assert_eq!(ratify.target.id, drifted.id);
    assert_ne!(ratify.target.id, missing_approval.id);
    assert!(
        ratify.reason.contains("meaning drifted")
            && ratify.reason.contains("redefined after ratification"),
        "Ratify must explain the concrete conflict: {}",
        ratify.reason
    );
}

#[test]
fn plain_next_does_not_skip_rectify_the_way_it_skips_ratify() {
    // Contract: rectify is autonomous prep; ratify is human-gated. The plain
    // `loom next` walk skips `requires_human_decision` lanes only — so it can
    // serve rectify and must never serve ratify.
    assert!(!Lane::Rectify.requires_human_decision());
    assert!(Lane::Divergence.requires_human_decision());

    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("rectify-plain"), false).unwrap();
    seed_duplicate_pair(&store);

    let mut served = Vec::new();
    for &l in Lane::LADDER {
        if l.requires_human_decision() || !l.serves_items() {
            continue;
        }
        if let Some(w) = loom::workitem::next(&store, Some(l)).unwrap() {
            served.push(w.mode.clone());
        }
    }
    assert!(
        served.iter().any(|m| m == "rectify"),
        "plain walk reaches rectify: {served:?}"
    );
    assert!(
        !served.iter().any(|m| m == "ratify"),
        "plain walk must not serve human ratify: {served:?}"
    );
}

#[test]
fn escalate_moves_discovery_from_rectify_to_ratify() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("rectify-esc"), false).unwrap();

    let intent = store
        .add_node(
            NodeType::Intent,
            "users can cancel an order via rectify escalate",
            "a product surface nobody has ratified",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "visibility",
            "user_visible",
            TruthClass::Asserted,
        )
        .unwrap();
    earn_call_witness(&store, tmp.path(), &intent.id);
    let g = store
        .edges_with(Some(EdgeKind::Implements), Some(&intent.id), None)
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
    common::s3_journey_proof_unratified(&store, tmp.path(), &intent.id, "cancel-via-rectify");
    loom::sync::run(&store, tmp.path()).unwrap();

    let witness = loom::ratification::witness(&store, &intent.id)
        .unwrap()
        .expect("sync records the complete technical witness");
    assert!(
        witness.holds(),
        "grounding, verdict, proof strength, and recorded usage should remain inspectable"
    );
    assert_eq!(
        store.ratification(&intent.id).unwrap(),
        "unratified",
        "technical evidence must not write a human Ratification fact"
    );
    assert_eq!(
        loom::ratification::effective_for(&store, &intent.id).unwrap(),
        loom::ratification::Ratification::Unratified,
        "a complete technical witness must not imply human wantedness"
    );
    assert!(
        loom::workitem::unratified_intents(&store)
            .unwrap()
            .iter()
            .any(|candidate| candidate.id == intent.id),
        "missing human authority stays explicit after technical evidence accumulates"
    );

    let discovered = loom::divergence::all(&store)
        .unwrap()
        .into_iter()
        .find(|d| {
            d.intent_id == intent.id
                && d.kind == loom::divergence::Kind::DiscoveredBehavior
                && d.blocking
        })
        .expect("user-visible witnessed behavior is a blocking discovery");

    assert!(
        loom::divergence::is_rectifiable(&store, &discovered).unwrap(),
        "un-escalated discovery belongs to rectify"
    );
    assert_eq!(loom::divergence::rectifiable_count(&store).unwrap(), 1);
    assert_eq!(loom::divergence::human_blocking_count(&store).unwrap(), 0);

    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            loom::divergence::RECTIFY_FACET,
            loom::divergence::RECTIFY_ESCALATED,
            TruthClass::Asserted,
        )
        .unwrap();

    assert_eq!(
        loom::divergence::rectifiable_count(&store).unwrap(),
        0,
        "escalated discovery leaves rectify"
    );
    assert_eq!(
        loom::divergence::human_blocking_count(&store).unwrap(),
        1,
        "escalated discovery enters human ratify"
    );
    let ratify = loom::workitem::next(&store, Some(Lane::Divergence))
        .unwrap()
        .expect("ratify serves escalated discovery");
    assert_eq!(ratify.mode, "ratify");
    assert_eq!(ratify.target.id, intent.id);
}

#[test]
fn llm_rectify_lane_cannot_ratify() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("rectify-inv8"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "must not be auto ratified",
            "behavior under inv8",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store.set_agent(Agent::Lane(loom::registry::OwnerRole::Rectify));
    let err = store
        .ratify_intent(&intent.id, "llm decided yes", "forged")
        .expect_err("rectify lane must not ratify");
    let msg = format!("{err}");
    assert!(msg.contains("INV-8"), "expected INV-8 denial, got: {msg}");
}

#[test]
fn ladder_puts_rectified_before_converged() {
    let names: Vec<_> = Lane::LADDER.iter().map(|l| l.as_str()).collect();
    let rectify = names
        .iter()
        .position(|n| *n == "rectify")
        .expect("rectify lane");
    let ratify = names
        .iter()
        .position(|n| *n == "ratify")
        .expect("ratify lane");
    assert!(
        rectify < ratify,
        "rectify must climb before human ratify: {names:?}"
    );
    assert_eq!(Lane::Rectify.rung(), "rectified");
    assert_eq!(Lane::parse("rectify"), Some(Lane::Rectify));
    assert!(!Lane::Rectify.requires_human_decision());
    assert!(Lane::Divergence.requires_human_decision());
}
