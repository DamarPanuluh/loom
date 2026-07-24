//! Ring 21 — the lane table is the ONE routing structure.
//!
//! Before `Lane`, the maturity ladder (`build_rungs`) and the compass were two
//! independent if-chains over overlapping inputs, held in agreement only by
//! comments. They could — and did — disagree: the compass pointed at lanes whose
//! queue served nothing, and three lanes had no rung at all.
//!
//! The contract under test, stated once: **every rung has a lane, every lane has
//! a rung, and a rung is Unmet iff its lane's queue is non-empty.** The compass
//! is a projection of the ladder, never a second decision. These are properties,
//! not examples — a hand-maintained comment cannot enforce them, a proptest can.

use loom::lane::{LadderInputs, Lane, QueueDepths};
use loom::maturity::{build_rungs, compass, ladder, RungState};
use loom::model::{EdgeKind, InspectionStatus, NodeType, TruthClass};
use loom::store::Store;
use loom::workitem;
use proptest::prelude::*;
mod common;
use common::*;

/// Arbitrary ladder inputs. Small ranges keep every rung reachable — the point
/// is to hit every combination of met/unmet, not to model realistic magnitudes.
fn any_inputs() -> impl Strategy<Value = LadderInputs> {
    (
        (
            any::<bool>(),
            0usize..3,
            0usize..3,
            0usize..3,
            0usize..3,
            0usize..3,
        ),
        (
            0usize..3,
            0usize..3,
            0usize..3,
            0usize..3,
            0usize..3,
            0usize..3,
            any::<bool>(),
        ),
    )
        .prop_map(
            |(
                (observed, active, implemented, planned, unowned, failing),
                (stale_rel, stale_gov, stale_val, triage, hypotheses, divergences, export_fresh),
            )| LadderInputs {
                observed,
                active,
                implemented,
                codefiles: unowned,
                planned,
                unowned_codefiles: unowned,
                failing,
                stale_relationships: stale_rel,
                stale_governs: stale_gov,
                stale_validates: stale_val,
                triage_findings: triage,
                proposed_hypotheses: hypotheses,
                divergences,
                export_fresh,
                ..Default::default()
            },
        )
}

proptest! {
    /// The compass phase is ALWAYS the lowest unmet rung's lane. Not "usually",
    /// not "when the comments are kept in step".
    #[test]
    fn compass_is_the_lowest_unmet_rungs_lane(c in any_inputs()) {
        let rungs = build_rungs(&c);
        let (phase, rung, _cmd, axis) = compass(&rungs);
        match rungs.iter().find(|r| r.state == RungState::Unmet) {
            Some(gate) => {
                prop_assert_eq!(&phase, gate.lane.as_str());
                prop_assert_eq!(&rung, &gate.name);
                prop_assert_eq!(axis, Some(gate.lane.axis()));
            }
            None => prop_assert_eq!(&phase, "complete"),
        }
    }

    /// `Unmet ⟺ depth > 0` — the rung predicate and the queue predicate are the
    /// same function, so a rung can never be red with an empty queue (or green
    /// with a full one).
    #[test]
    fn rung_state_and_queue_depth_never_disagree(c in any_inputs()) {
        let depths = QueueDepths::from_inputs(&c);
        for r in build_rungs(&c) {
            prop_assert_eq!(r.depth, depths.get(r.lane));
            match r.state {
                RungState::Unmet => prop_assert!(r.depth > 0),
                RungState::Met => prop_assert_eq!(r.depth, 0),
                // NotApplicable reports absent machinery; Open never completes.
                RungState::NotApplicable | RungState::Open => {}
            }
        }
    }

    /// Blocked propagation: everything above the gate is blocked BY the gate,
    /// nothing at or below it is.
    #[test]
    fn everything_above_the_gate_is_blocked_by_it(c in any_inputs()) {
        let rungs = build_rungs(&c);
        let Some(g) = rungs.iter().position(|r| r.state == RungState::Unmet) else {
            prop_assert!(rungs.iter().all(|r| !r.blocked));
            return Ok(());
        };
        let gate = rungs[g].name.clone();
        for (i, r) in rungs.iter().enumerate() {
            if i <= g {
                prop_assert!(!r.blocked, "{} is at/below the gate", r.name);
            } else {
                prop_assert!(r.blocked, "{} is above the gate", r.name);
                prop_assert_eq!(r.blocked_by.as_deref(), Some(gate.as_str()));
            }
        }
    }

    /// A lane disabled on an observed graph can never be the compass gate —
    /// routing an observed graph at `build` is a pure dead end.
    #[test]
    fn observed_graphs_never_route_at_a_disabled_lane(c in any_inputs()) {
        prop_assume!(c.observed);
        let rungs = build_rungs(&c);
        if let Some(gate) = rungs.iter().find(|r| r.state == RungState::Unmet) {
            prop_assert!(!gate.lane.observed_disabled(), "gated at {}", gate.name);
        }
    }
}

#[test]
fn the_ladder_is_a_total_bijection_with_lanes() {
    let rungs = build_rungs(&LadderInputs::default());
    assert_eq!(rungs.len(), Lane::LADDER.len());
    for (r, lane) in rungs.iter().zip(Lane::LADDER) {
        assert_eq!(r.lane, *lane, "rung order IS Lane::LADDER order");
        assert_eq!(r.name, lane.rung());
    }
}

/// The clause a hand-maintained if-chain kept getting wrong: when the compass
/// names a lane, `loom next --mode <that lane>` must actually hand back work.
/// Each case below is a graph shape whose gate lane previously could — or did —
/// serve nothing.
#[test]
fn every_gate_lane_serves_the_work_it_points_at() {
    /// (expected gate lane, graph builder).
    type Case = (&'static str, fn(&Store));
    let cases: Vec<Case> = vec![
        ("build", |store: &Store| {
            store
                .add_node(
                    NodeType::Intent,
                    "orders can be placed",
                    "a behavior",
                    "planned",
                    serde_json::json!({}),
                )
                .unwrap();
        }),
        // The regression this test exists for: an implemented intent with no
        // proof counts toward `proven`, so the validate lane must serve it.
        ("validate", |store: &Store| {
            let intent = store
                .add_node(
                    NodeType::Intent,
                    "orders can be placed",
                    "a behavior",
                    "implemented",
                    serde_json::json!({}),
                )
                .unwrap();
            let cf = store
                .add_node(
                    NodeType::CodeFile,
                    "src/o.rs",
                    "",
                    "",
                    serde_json::json!({}),
                )
                .unwrap();
            let e = store
                .add_edge(
                    EdgeKind::Implements,
                    &intent.id,
                    &cf.id,
                    TruthClass::Asserted,
                )
                .unwrap();
            store
                .record_verdict(
                    &e.id,
                    InspectionStatus::Passing,
                    "grounded",
                    "src/o.rs:1",
                    0.9,
                    "llm",
                )
                .unwrap();
        }),
        ("fix", |store: &Store| {
            let a = store
                .add_node(
                    NodeType::Intent,
                    "a",
                    "x",
                    "implemented",
                    serde_json::json!({}),
                )
                .unwrap();
            let b = store
                .add_node(
                    NodeType::Intent,
                    "b",
                    "y",
                    "implemented",
                    serde_json::json!({}),
                )
                .unwrap();
            let e = store
                .add_edge(EdgeKind::Relates, &a.id, &b.id, TruthClass::Asserted)
                .unwrap();
            store
                .record_verdict(&e.id, InspectionStatus::Failing, "c", "broken", 0.9, "llm")
                .unwrap();
        }),
    ];

    for (expected, build) in cases {
        let tmp = Tmp::new();
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        build(&store);
        // Ratification is a separate axis; settle it so these cases exercise the
        // lane under test rather than the divergence queue.
        for n in workitem::unratified_intents(&store).unwrap() {
            store
                .ratify_intent(&n.id, "test fixture: wanted", "test fixture")
                .unwrap();
        }

        let l = ladder(&store).unwrap();
        assert_eq!(l.phase, expected, "gate lane for the {expected} case");
        let lane = Lane::parse(&l.phase).unwrap();
        assert!(
            workitem::next(&store, Some(lane)).unwrap().is_some(),
            "the compass points at `{}` but that lane serves nothing — the exact \
             disagreement the lane table exists to make impossible",
            l.phase
        );
    }
}

/// A fresh intent's packet must propose WHERE to look, not hand back a listing
/// command. Found by pointing loom at a repository it had never seen: on
/// anything larger than a toy, "survey the registered codefiles" is the moment a
/// sidekick is least useful.
#[test]
fn a_fresh_intent_packet_proposes_candidate_files() {
    let tmp = Tmp::new();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    // One file whose symbols echo the intent, one that has nothing to do with it.
    std::fs::write(
        tmp.path().join("src/ruang.rs"),
        "pub fn create_communal_ruang() {}\npub fn authorize_ruang_read() {}\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join("src/telemetry.rs"), "pub fn flush() {}\n").unwrap();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    for path in ["src/ruang.rs", "src/telemetry.rs"] {
        store
            .add_node(NodeType::CodeFile, path, "", "", serde_json::json!({}))
            .unwrap();
    }
    loom::sync::run(&store, tmp.path()).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "a channel can be opened in a ruang",
            "opening a channel makes it visible to its ruang members",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .ratify_intent(&intent.id, "test fixture: wanted", "test fixture")
        .unwrap();

    let item = workitem::next(&store, Some(Lane::Build))
        .unwrap()
        .expect("a planned intent is build work");
    let reads: Vec<&str> = item
        .context
        .read_set
        .iter()
        .map(|r| r.path.as_str())
        .collect();
    assert!(
        reads.contains(&"src/ruang.rs"),
        "the file whose SYMBOLS echo the intent must be proposed: {reads:?}"
    );
    assert!(
        !reads.contains(&"src/telemetry.rs"),
        "an unrelated file must not be proposed: {reads:?}"
    );
    // The proposal names what matched and stays honest about its own weight:
    // a place to look, never a grounding.
    let why = &item
        .context
        .read_set
        .iter()
        .find(|r| r.path == "src/ruang.rs")
        .unwrap()
        .why;
    assert!(why.contains("ruang"), "names the matched symbols: {why}");
    assert!(
        why.contains("confirm"),
        "a candidate is a hint, not a verdict: {why}"
    );
}
