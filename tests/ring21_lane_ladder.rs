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
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use loom::workitem;
use proptest::prelude::*;
mod common;
use common::*;

fn satisfy_journey_root_prerequisites(store: &Store, surfaced: bool) {
    let artifact = "journeys/lane-fixture.yaml";
    let artifact_body = "schema: loom.journey/v1\nid: lane-fixture\nname: Exercise the selected lane\nactor: operator\ngoal: Exercise the selected lane\ninputs: {}\npreconditions: []\nsteps:\n  - id: work\n    name: Work\n    action: Perform the behavior under test\n    expects: []\n    produces: {}\nprofiles:\n  proof:\n    inputs: {}\n    workspace: {}\n";
    std::fs::create_dir_all(store.root().join("journeys")).unwrap();
    std::fs::write(store.root().join(artifact), artifact_body).unwrap();
    let journey_hash = serde_norway::from_str::<loom::journey::JourneySpec>(artifact_body)
        .unwrap()
        .semantic_hash()
        .unwrap();
    let journey = store
        .add_node(
            NodeType::Journey,
            "lane-fixture",
            "exercise the selected lane",
            "authored",
            serde_json::json!({
                "schema": "loom.journey/v1",
                "stable_id": "lane-fixture",
                "name": "Exercise the selected lane",
                "actor": "operator",
                "goal": "exercise the selected lane",
                "artifact": artifact,
                "semantic_hash": journey_hash,
                "input_ids": [],
                "preconditions": [],
                "step_ids": ["work"],
                "output_ids": [],
                "profile_ids": ["proof"],
            }),
        )
        .unwrap();
    for intent in store
        .list_nodes(Some(NodeType::Intent), usize::MAX)
        .unwrap()
    {
        let derives = store
            .add_edge(
                EdgeKind::Derives,
                &journey.id,
                &intent.id,
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .set_facet(
                &derives.id,
                TargetKind::Edge,
                "journey_hash",
                &journey_hash,
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .set_facet(
                &derives.id,
                TargetKind::Edge,
                "step_ids",
                r#"["work"]"#,
                TruthClass::Asserted,
            )
            .unwrap();
    }
    if !surfaced {
        return;
    }
    let codefile = store
        .list_nodes(Some(NodeType::CodeFile), usize::MAX)
        .unwrap()
        .into_iter()
        .next()
        .expect("validate fixture has a grounded CodeFile");
    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            "lane-cli",
            "the target repository CLI",
            "active",
            serde_json::json!({
                "schema": "loom.interface-surface/v1",
                "stable_id": "lane-cli",
                "title": "Lane CLI",
                "kind": "cli",
                "identity": "lane",
                "codefile": "src/o.rs",
                "locator": "fn place",
                "operations": [{
                    "id": "work-op",
                    "summary": "perform the work",
                    "argv": ["lane", "work"],
                    "arguments": [],
                    "output": {"format": "json"}
                }],
            }),
        )
        .unwrap();
    let surfaces = store
        .add_edge(
            EdgeKind::Surfaces,
            &journey.id,
            &surface.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "journey_hash",
            &journey_hash,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "operation_bindings",
            r#"[{"operation_id":"work-op","step_id":"work"}]"#,
            TruthClass::Asserted,
        )
        .unwrap();
    let exposes = store
        .add_edge(
            EdgeKind::Exposes,
            &surface.id,
            &codefile.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &exposes.id,
            TargetKind::Edge,
            "locator",
            "fn place",
            TruthClass::Asserted,
        )
        .unwrap();
}

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
        // The gate is the lowest rung that is Unmet OR Open — `deepen` is
        // permanently Open, so it is the fallthrough for a graph with code in
        // it. "complete" survives only for a graph with nothing to deepen at
        // all, where every rung is NotApplicable.
        match rungs
            .iter()
            .find(|r| matches!(r.state, RungState::Unmet | RungState::Open))
        {
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
                // Absent machinery implies an empty queue. A lane holding work
                // has machinery by definition, so it reads Unmet — this arm
                // used to be unchecked, and a non-empty queue could hide
                // behind NotApplicable.
                RungState::NotApplicable => prop_assert_eq!(r.depth, 0),
                // Deepen re-ranks rather than draining; it never completes.
                RungState::Open => {}
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
            std::fs::create_dir_all(store.root().join("src")).unwrap();
            std::fs::write(store.root().join("src/o.rs"), "pub fn place() {}\n").unwrap();
            let intent = store
                .add_node(
                    NodeType::Intent,
                    "orders can be placed",
                    "a behavior",
                    "implemented",
                    serde_json::json!({}),
                )
                .unwrap();
            let cf = codefile(store, "src/o.rs");
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
        satisfy_journey_root_prerequisites(&store, expected == "validate");
        // Ratification is a separate axis; settle it so these cases exercise the
        // lane under test rather than the divergence queue.
        for n in workitem::unratified_intents(&store).unwrap() {
            store
                .ratify_intent(&n.id, "test fixture: wanted", "test fixture")
                .unwrap();
        }

        let doctor = loom::signal::doctor(&store).unwrap();
        assert!(doctor.is_empty(), "invalid {expected} fixture: {doctor:?}");
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

#[test]
fn ordinary_next_uses_lane_priority_not_insertion_order() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let build_residue = store
        .add_node(
            NodeType::Intent,
            "inserted first build residue",
            "a behavior still waiting to be built",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let source = store
        .add_node(
            NodeType::Intent,
            "fix source",
            "a behavior with a broken relationship",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let target = store
        .add_node(
            NodeType::Intent,
            "fix target",
            "the related behavior",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let fix_residue = store
        .add_edge(
            EdgeKind::Relates,
            &source.id,
            &target.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &fix_residue.id,
            InspectionStatus::Failing,
            "the asserted relationship must hold",
            "the relationship is broken",
            0.9,
            "llm",
        )
        .unwrap();

    let selected = workitem::next(&store, None)
        .unwrap()
        .expect("ordinary next selects the highest-priority autonomous residue");
    assert_eq!(selected.mode, Lane::Fix.as_str());
    assert_eq!(
        selected.target.id, fix_residue.id,
        "later-inserted Fix residue must outrank earlier Build residue by Lane::LADDER"
    );
    assert_ne!(selected.target.id, build_residue.id);
}

#[test]
fn requires_readiness_waits_for_realization_and_preserves_residue_order() {
    fn prerequisite_state(store: &Store, intent: &loom::model::Node) -> String {
        loom::completeness::scorecard(store, intent)
            .unwrap()
            .axes
            .into_iter()
            .find(|axis| axis.axis == "prerequisites")
            .unwrap()
            .state
    }

    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let higher_priority = store
        .add_node(
            NodeType::Intent,
            "repair existing asserted behavior",
            "higher-priority residue",
            "needs_change",
            serde_json::json!({}),
        )
        .unwrap();
    let dependent = store
        .add_node(
            NodeType::Intent,
            "aaa dependent behavior",
            "must wait for its prerequisite",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let prerequisite = store
        .add_node(
            NodeType::Intent,
            "zzz prerequisite behavior",
            "the behavior the dependent stands on",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Requires,
            &dependent.id,
            &prerequisite.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let first = workitem::next(&store, Some(Lane::Build))
        .unwrap()
        .expect("higher-priority residue is build work");
    assert_eq!(
        first.target.id, higher_priority.id,
        "requires readiness must not bypass ordinary higher-priority residue"
    );
    store
        .set_node_status(&higher_priority.id, "implemented")
        .unwrap();
    let higher_file = codefile(&store, "src/higher_priority.rs");
    store
        .add_edge(
            EdgeKind::Implements,
            &higher_priority.id,
            &higher_file.id,
            TruthClass::Asserted,
        )
        .unwrap();

    assert_eq!(prerequisite_state(&store, &dependent), "open");
    let planned = workitem::next(&store, Some(Lane::Build))
        .unwrap()
        .expect("the planned prerequisite is build work");
    assert_eq!(
        planned.target.id, prerequisite.id,
        "the dependent is excluded while its prerequisite is planned"
    );

    store
        .set_node_status(&prerequisite.id, "implemented")
        .unwrap();
    assert_eq!(
        prerequisite_state(&store, &dependent),
        "open",
        "implemented lifecycle without grounding is not realization"
    );
    let ungrounded = workitem::next(&store, Some(Lane::Build))
        .unwrap()
        .expect("the ungrounded prerequisite remains build work");
    assert_eq!(ungrounded.target.id, prerequisite.id);
    assert!(
        ungrounded.reason.contains("implemented but ungrounded"),
        "the packet explains why lifecycle alone is insufficient: {}",
        ungrounded.reason
    );

    let prerequisite_file = codefile(&store, "src/prerequisite.rs");
    store
        .add_edge(
            EdgeKind::Implements,
            &prerequisite.id,
            &prerequisite_file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    assert_eq!(prerequisite_state(&store, &dependent), "met");
    let ready = workitem::next(&store, Some(Lane::Build))
        .unwrap()
        .expect("the dependent becomes eligible after prerequisite realization");
    assert_eq!(ready.target.id, dependent.id);

    let rollup = store
        .add_node(
            NodeType::Intent,
            "implemented hierarchy roll-up",
            "realized through its child",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let child = store
        .add_node(
            NodeType::Intent,
            "grounded hierarchy child",
            "the roll-up implementation",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let child_file = codefile(&store, "src/hierarchy_child.rs");
    store
        .add_edge(
            EdgeKind::Implements,
            &child.id,
            &child_file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Hierarchy,
            &rollup.id,
            &child.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let rollup_dependent = store
        .add_node(
            NodeType::Intent,
            "behavior requiring the roll-up",
            "depends on the hierarchy parent",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Requires,
            &rollup_dependent.id,
            &rollup.id,
            TruthClass::Asserted,
        )
        .unwrap();
    assert_eq!(
        prerequisite_state(&store, &rollup_dependent),
        "met",
        "hierarchy roll-ups remain exempt from direct grounding"
    );
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
        codefile(&store, path);
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

/// The invariant, checked against a graph rather than generated inputs.
///
/// The proptest covers `depth > 0 ⟺ Unmet`, but depth is computed from the same
/// counters the rung reads — so it cannot catch a lane whose QUEUE has no
/// branch for one of the things its depth counts. That is what happened: the
/// `proven` rung read 14 while `loom next --mode validate` answered "no work",
/// because nothing served a journey-proof gap. The reverse drift is just as
/// invalid: a lane must not serve hidden work while its rung says Met.
#[test]
fn every_per_item_gating_rung_is_unmet_iff_it_serves_something() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    // A user-visible behavior, grounded and unit-proven — which leaves exactly
    // one thing outstanding: it has no journey proof.
    let intent = store
        .add_node(
            NodeType::Intent,
            "users can see this happen",
            "a behavior a user can see",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .set_facet(
            &intent.id,
            loom::model::TargetKind::Node,
            "visibility",
            "user_visible",
            TruthClass::Asserted,
        )
        .unwrap();
    let cf = common::codefile(&store, "src/seen.rs");
    let g = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &g.id,
            loom::model::InspectionStatus::Passing,
            "lives here",
            "src/seen.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    loom::commands::prove_intent(&store, &intent.id, "unit proof", "true").unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();

    let ladder = loom::maturity::ladder(&store).unwrap();
    for rung in &ladder.rungs {
        match rung.lane {
            // These lanes close through whole-graph commands, not packets.
            Lane::Seed | Lane::Export => {
                assert!(!rung.lane.serves_items());
                continue;
            }
            // Deepening is deliberately non-draining and Open, never a gating
            // Unmet rung even when an optional improvement packet exists.
            Lane::Deepen => {
                assert_eq!(rung.state, RungState::Open);
                continue;
            }
            _ => {}
        }

        let serves = loom::workitem::next(&store, Some(rung.lane))
            .unwrap()
            .is_some();
        assert_eq!(
            rung.state == RungState::Unmet,
            serves,
            "rung '{}' is {:?} at depth {} while lane '{}' {} work — rung and queue must agree in both directions",
            rung.name,
            rung.state,
            rung.depth,
            rung.lane.as_str(),
            if serves { "serves" } else { "serves no" }
        );
    }
}

/// The compass and the autonomous walk answer DIFFERENT questions — "what is
/// the lowest rung blocking maturity?" versus "what can a driver do alone right
/// now?" — and the router deliberately never serves `seed`, `export`, or
/// `ratify` on its own. Both answers are correct, but nothing in the output
/// said they were different questions, so a reader saw `status` name one phase
/// and `next` hand over another and concluded loom was of two minds.
///
/// A cold graph is the sharpest case: it gates at `seeded`, which the walk
/// never serves, while coverage work sits above that gate and is served. The
/// packet must name the gate. When the two agree, the notice must be ABSENT —
/// a notice on every packet is noise, not signal.
#[test]
fn a_packet_above_the_compass_gate_names_the_gate_and_stays_silent_otherwise() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    tmp.write("src/a.rs", "pub fn a() {}\n");
    store
        .add_node(
            NodeType::CodeFile,
            "src/a.rs",
            "a registered file nothing owns yet",
            "active",
            serde_json::json!({}),
        )
        .unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();

    let ladder = ladder(&store).unwrap();
    assert_eq!(
        ladder.phase, "seed",
        "a graph with no authored Journey gates at seed"
    );
    let served = workitem::next(&store, None)
        .unwrap()
        .expect("coverage work is served");
    assert_ne!(
        served.mode, ladder.phase,
        "fixture precondition: the walk must serve something other than the gate"
    );

    let gate = workitem::off_gate(&ladder, Some(&served))
        .expect("a packet that is not the gate must name the gate");
    assert_eq!(gate.lane, "seed");
    assert_eq!(gate.rung, "seeded");
    // The gate closes through a whole-graph command, not `loom next --mode
    // seed` — which is not even a legal mode, so naming the command matters
    // more than naming the lane.
    assert_eq!(gate.next_command, "loom journey add <spec>");
    assert!(
        gate.note.contains("above the gate") && gate.note.contains("not a work packet"),
        "the note must say why this packet is not the gate: {}",
        gate.note
    );

    // Agreement is silent: a packet whose mode IS the gate names nothing.
    let mut agreeing = served.clone();
    agreeing.mode = ladder.phase.clone();
    assert!(
        workitem::off_gate(&ladder, Some(&agreeing)).is_none(),
        "the notice must be absent when the served packet IS the compass gate"
    );
}

/// `NotApplicable` means "this lane's machinery does not exist yet", so it must
/// never be worn by a lane holding real queued work. Checking it BEFORE depth
/// let a non-empty queue read as absent machinery, and because `NotApplicable`
/// is deliberately transparent to the gate, the hidden rung could not become
/// the compass gate either — on a graph with no active intents, a broken
/// doctor was reported as `complete`.
#[test]
fn absent_machinery_never_masks_a_non_empty_queue() {
    // Registered code, no intents yet — the brownfield cold start — plus a
    // broken graph. Every non-audit rung is blocked by integrity, and audit
    // itself used to read NotApplicable because `active == 0`.
    let c = LadderInputs {
        active: 0,
        codefiles: 1,
        unowned_codefiles: 1,
        doctor_issues: 1,
        authored_journeys: 1,
        ..Default::default()
    };
    let rungs = build_rungs(&c);
    for rung in &rungs {
        if rung.state == RungState::NotApplicable {
            assert_eq!(
                rung.depth, 0,
                "rung '{}' claims absent machinery while holding {} queued item(s)",
                rung.name, rung.depth
            );
        }
    }
    let (phase, _, _, _) = compass(&rungs);
    assert_eq!(
        phase, "audit",
        "a graph with integrity issues must route to audit, never report itself finished"
    );
}

#[test]
fn blocked_lifecycle_leaves_the_build_queue_with_an_honest_write() {
    // A planned intent whose implementation is gated on a current external
    // prerequisite is still wanted — it is not deprecated — but it is not
    // build work. The packet must name --lifecycle blocked so a driver can
    // park it; once parked, build serves nothing.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "rakan-launch",
            "pre-external-launch surface, gated on the first external tenant, not on code",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();

    let served = workitem::next(&store, Some(Lane::Build))
        .unwrap()
        .expect("a planned intent is build work");
    assert_eq!(served.target.id, intent.id);
    assert!(
        served
            .prompt_contract
            .allowed_actions
            .iter()
            .any(|a| a.contains("--lifecycle blocked")),
        "build packet must allow parking on an external hold: {:?}",
        served.prompt_contract.allowed_actions
    );
    assert!(
        served
            .prompt_contract
            .write_back
            .contains("--lifecycle blocked"),
        "write_back must name blocked as a close: {}",
        served.prompt_contract.write_back
    );

    store
        .update_node(&intent.id, None, None, Some("blocked"))
        .unwrap();
    assert!(
        workitem::next(&store, Some(Lane::Build)).unwrap().is_none(),
        "a blocked intent must leave the build queue"
    );
    let depths = loom::maturity::depths(&store).unwrap();
    assert_eq!(
        depths.get(Lane::Build),
        0,
        "compass build depth must match the empty queue"
    );
}
