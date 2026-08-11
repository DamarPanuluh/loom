//! Ring 46 — Journey is the routing root.
//!
//! The ladder, singular packet, roster, readiness projection, and Intent
//! completeness axis all read the same hash-bound Journey facts.

use loom::completeness;
use loom::lane::{LadderInputs, Lane};
use loom::model::{EdgeKind, InspectionStatus, Node, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use loom::workitem;
mod common;
use common::*;

fn authored_journey(store: &Store, root: &std::path::Path, id: &str) -> Node {
    let artifact = format!("journeys/{id}.yaml");
    std::fs::create_dir_all(root.join("journeys")).unwrap();
    std::fs::write(
        root.join(&artifact),
        format!("schema: loom.journey/v1\nid: {id}\nname: Complete the flow\nactor: user\ngoal: A user completes the flow\ninputs: {{}}\npreconditions: []\nsteps:\n  - id: choose\n    name: Choose\n    action: Choose an option\n    expects: []\n    produces: {{}}\n  - id: confirm\n    name: Confirm\n    action: Confirm the option\n    expects:\n      - The flow is completed\n    produces:\n      receipt:\n        type: string\nprofiles:\n  proof:\n    inputs: {{}}\n    workspace: {{}}\n"),
    )
    .unwrap();
    store
        .add_node(
            NodeType::Journey,
            id,
            "a user completes the flow",
            "authored",
            serde_json::json!({
                "schema": "loom.journey/v1",
                "stable_id": id,
                "name": "Complete the flow",
                "actor": "user",
                "goal": "a user completes the flow",
                "artifact": artifact,
                "semantic_hash": format!("hash-{id}"),
                "input_ids": [],
                "preconditions": [],
                "step_ids": ["choose", "confirm"],
                "output_ids": ["receipt"],
                "profile_ids": ["proof"],
            }),
        )
        .unwrap()
}

fn intent(store: &Store, name: &str, lifecycle: &str) -> Node {
    store
        .add_node(
            NodeType::Intent,
            name,
            "the technical behavior is falsifiable",
            lifecycle,
            serde_json::json!({}),
        )
        .unwrap()
}

fn derive(store: &Store, journey: &Node, intent: &Node, step_ids: &[&str]) -> loom::model::Edge {
    let edge = store
        .add_edge(
            EdgeKind::Derives,
            &journey.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_hash",
            journey.body["semantic_hash"].as_str().unwrap(),
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "step_ids",
            &serde_json::to_string(step_ids).unwrap(),
            TruthClass::Asserted,
        )
        .unwrap();
    edge
}

fn ratify_and_ground(store: &Store, intent: &Node, path: &str) -> Node {
    store
        .ratify_intent(&intent.id, "the fixture human wants this", "test fixture")
        .unwrap();
    let code = codefile(store, path);
    store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &code.id,
            TruthClass::Asserted,
        )
        .unwrap();
    code
}

fn add_surface(store: &Store, journey: &Node) -> (Node, loom::model::Edge) {
    let interface = store
        .add_node(
            NodeType::InterfaceSurface,
            "flow-cli",
            "the real target-repository CLI",
            "active",
            serde_json::json!({
                "schema": "loom.interface-surface/v1",
                "stable_id": "flow-cli",
                "title": "Flow CLI",
                "kind": "cli",
                "identity": "flow",
                "operations": [
                    {"id":"choose-op", "summary":"choose", "argv":["flow","choose"], "arguments":[], "output":{"format":"json"}},
                    {"id":"confirm-op", "summary":"confirm", "argv":["flow","confirm"], "arguments":[], "output":{"format":"json"}}
                ],
            }),
        )
        .unwrap();
    let edge = store
        .add_edge(
            EdgeKind::Surfaces,
            &journey.id,
            &interface.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_hash",
            journey.body["semantic_hash"].as_str().unwrap(),
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "operation_bindings",
            r#"[{"operation_id":"choose-op","step_id":"choose"},{"operation_id":"confirm-op","step_id":"confirm"}]"#,
            TruthClass::Asserted,
        )
        .unwrap();
    (interface, edge)
}

fn earn_s3(store: &Store, journey: &Node) {
    let surface_hash = loom::journey::surface_projection_hash(store, journey)
        .unwrap()
        .expect("surface projection exists");
    let validation = store
        .add_node(
            NodeType::Validation,
            "journey:flow:proof",
            "compiled Journey proof",
            "passed",
            serde_json::json!({
                "type":"journey",
                "profile":"proof",
                "journey_hash": journey.body["semantic_hash"].as_str().unwrap(),
                "surface_hash": surface_hash,
                "compiler_version": "test-v1",
            }),
        )
        .unwrap();
    store
        .set_facet(
            &validation.id,
            TargetKind::Node,
            "proof_strength",
            &serde_json::to_string(&loom::proofstrength::StrengthWitness {
                grade: "S3".into(),
                ran_and_passed: true,
                content_assertions: 1,
                call_witness: Some("flow_cli::main -> behavior".into()),
                next: "raise the boundary".into(),
                ..Default::default()
            })
            .unwrap(),
            TruthClass::Derived,
        )
        .unwrap();
    let proves = store
        .add_edge(
            EdgeKind::Proves,
            &validation.id,
            &journey.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &proves.id,
            InspectionStatus::Passing,
            "compiled proof reaches the surfaced CLI",
            "src/flow_cli.rs:1",
            0.99,
            "validator",
        )
        .unwrap();
}

#[test]
fn ladder_prefix_and_observed_disable_are_journey_rooted() {
    assert_eq!(
        &Lane::LADDER[..7],
        &[
            Lane::Seed,
            Lane::Fix,
            Lane::Derive,
            Lane::Build,
            Lane::Surface,
            Lane::Coverage,
            Lane::Validate,
        ]
    );
    assert_eq!(Lane::Derive.rung(), "derived");
    assert_eq!(Lane::Surface.rung(), "surfaced");
    assert_eq!(Lane::parse("derive"), Some(Lane::Derive));
    assert_eq!(Lane::parse("surface"), Some(Lane::Surface));
    assert_eq!(Lane::Derive.axis(), loom::truth::TruthAxis::Intent);
    assert_eq!(Lane::Surface.axis(), loom::truth::TruthAxis::Implementation);

    let owned = LadderInputs {
        authored_journeys: 1,
        active: 2,
        derive_gaps: 3,
        surface_gaps: 2,
        ..Default::default()
    };
    assert_eq!(Lane::Seed.depth(&owned), 0);
    assert_eq!(Lane::Derive.depth(&owned), 3);
    assert_eq!(Lane::Surface.depth(&owned), 2);
    let observed = LadderInputs {
        observed: true,
        ..owned
    };
    assert_eq!(Lane::Derive.depth(&observed), 0);
    assert_eq!(Lane::Surface.depth(&observed), 0);

    let no_root = LadderInputs {
        active: 8,
        ..Default::default()
    };
    assert_eq!(Lane::Seed.depth(&no_root), 1);
}

#[test]
fn derive_queue_covers_unmapped_stale_and_unrooted_non_exempt_work() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let journey = authored_journey(&store, tmp.path(), "flow");
    let mapped = intent(&store, "mapped behavior", "planned");
    let stale = derive(&store, &journey, &mapped, &["choose"]);
    store
        .set_facet(
            &stale.id,
            TargetKind::Edge,
            "journey_hash",
            "old-hash",
            TruthClass::Asserted,
        )
        .unwrap();
    let unrooted = intent(&store, "unrooted behavior", "planned");
    let exempt = intent(&store, "deliberately rootless utility", "implemented");
    store
        .set_facet(
            &exempt.id,
            TargetKind::Node,
            "journey_exemption",
            r#"{"human_decision_digest":"sha256:abc","kind":"infrastructure","reason":"not user-reachable"}"#,
            TruthClass::Asserted,
        )
        .unwrap();

    let gaps = completeness::journey_derive_gaps(&store).unwrap();
    assert!(gaps.iter().any(|gap| gap.kind == "unmapped_step"));
    assert!(gaps.iter().any(|gap| gap.kind == "stale_derivation"));
    assert!(gaps
        .iter()
        .any(|gap| gap.kind == "unrooted_intent" && gap.subject_id == unrooted.id));
    assert!(!gaps.iter().any(|gap| gap.subject_id == exempt.id));

    let roster = workitem::queue_items(&store, Lane::Derive).unwrap();
    assert_eq!(roster.len(), gaps.len());
    let item = workitem::next(&store, Some(Lane::Derive))
        .unwrap()
        .expect("derive gap is servable");
    assert_eq!(item.target.id, journey.id);
    assert_eq!(item.owner_role, "builder");
    assert!(item
        .prompt_contract
        .write_back
        .contains("loom journey derive-accept"));
    assert!(item.prompt_contract.write_back.contains(&journey.id));
}

#[test]
fn malformed_or_noncanonical_exemption_never_hides_unrooted_work() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    authored_journey(&store, tmp.path(), "flow");
    let subject = intent(&store, "utility", "implemented");
    for invalid in [
        r#"{"kind":"infra","reason":"because"}"#,
        r#"{ "human_decision_digest":"x","kind":"infra","reason":"because" }"#,
        r#"{"human_decision_digest":"","kind":"infra","reason":"because"}"#,
    ] {
        store
            .set_facet(
                &subject.id,
                TargetKind::Node,
                "journey_exemption",
                invalid,
                TruthClass::Asserted,
            )
            .unwrap();
        assert!(!completeness::intent_journey_exempt(&store, &subject.id).unwrap());
    }
}

#[test]
fn surface_waits_for_current_ratified_realizing_derivations() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let journey = authored_journey(&store, tmp.path(), "flow");
    let derived = intent(&store, "flow is executed", "planned");
    derive(&store, &journey, &derived, &["choose", "confirm"]);

    assert!(completeness::journey_surface_gaps(&store)
        .unwrap()
        .is_empty());
    store
        .ratify_intent(&derived.id, "the fixture human wants this", "test fixture")
        .unwrap();
    assert!(completeness::journey_surface_gaps(&store)
        .unwrap()
        .is_empty());
    store.set_node_status(&derived.id, "implemented").unwrap();
    assert!(completeness::journey_surface_gaps(&store)
        .unwrap()
        .is_empty());
    let code = codefile(&store, "src/behavior.rs");
    store
        .add_edge(
            EdgeKind::Implements,
            &derived.id,
            &code.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let gaps = completeness::journey_surface_gaps(&store).unwrap();
    assert_eq!(gaps.len(), 1);
    let item = workitem::next(&store, Some(Lane::Surface))
        .unwrap()
        .expect("eligible Journey is surface work");
    assert_eq!(item.target.id, journey.id);
    assert!(item
        .prompt_contract
        .write_back
        .contains("loom journey surface-accept"));

    let (interface, _) = add_surface(&store, &journey);
    let surface_code = codefile(&store, "src/flow_cli.rs");
    store
        .add_edge(
            EdgeKind::Exposes,
            &interface.id,
            &surface_code.id,
            TruthClass::Asserted,
        )
        .unwrap();
    assert!(completeness::journey_surface_gaps(&store)
        .unwrap()
        .is_empty());
}

#[test]
fn readiness_and_validate_progress_to_realized_s3() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let journey = authored_journey(&store, tmp.path(), "flow");
    let derived = intent(&store, "flow is executed", "implemented");
    derive(&store, &journey, &derived, &["choose", "confirm"]);
    ratify_and_ground(&store, &derived, "src/behavior.rs");

    let r = completeness::journey_readiness(&store, &journey).unwrap();
    assert!(r.authored && r.derived && r.implemented);
    assert!(!r.surfaced && !r.compiled && !r.proven && !r.realized);

    let (interface, _) = add_surface(&store, &journey);
    let r = completeness::journey_readiness(&store, &journey).unwrap();
    assert!(!r.surfaced);
    assert!(!r.compiled);
    let surface_code = codefile(&store, "src/flow_cli.rs");
    store
        .add_edge(
            EdgeKind::Exposes,
            &interface.id,
            &surface_code.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let r = completeness::journey_readiness(&store, &journey).unwrap();
    assert!(r.surfaced && !r.compiled && !r.proven && !r.realized);

    let validate = workitem::next(&store, Some(Lane::Validate))
        .unwrap()
        .expect("compiled unproven Journey is validation work");
    assert_eq!(validate.target.id, journey.id);
    assert!(validate
        .prompt_contract
        .write_back
        .contains("loom journey compile"));
    assert!(validate
        .prompt_contract
        .write_back
        .contains("loom journey run"));

    earn_s3(&store, &journey);
    let r = completeness::journey_readiness(&store, &journey).unwrap();
    assert!(r.proven && r.realized);
}

#[test]
fn intrinsic_human_binding_completes_readiness_without_becoming_a_cli_witness() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let journey = authored_journey(&store, tmp.path(), "flow");
    let derived = intent(&store, "flow is executed", "implemented");
    derive(&store, &journey, &derived, &["choose", "confirm"]);
    ratify_and_ground(&store, &derived, "src/behavior.rs");

    let (interface, surfaces) = add_surface(&store, &journey);
    store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "operation_bindings",
            r#"[{"operation_id":"choose-op","step_id":"choose"},{"human_decision":{"operation_id":"choose-op","pointer":"/work_item"},"step_id":"confirm"}]"#,
            TruthClass::Asserted,
        )
        .unwrap();
    let surface_code = codefile(&store, "src/flow_cli.rs");
    store
        .add_edge(
            EdgeKind::Exposes,
            &interface.id,
            &surface_code.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let readiness = completeness::journey_readiness(&store, &journey).unwrap();
    assert!(readiness.surfaced, "{:?}", readiness.surface_gaps);
    assert!(readiness.surface_gaps.is_empty());
    assert!(completeness::journey_surface_gaps(&store)
        .unwrap()
        .is_empty());
    assert!(store
        .edges_with(Some(EdgeKind::Calls), None, None)
        .unwrap()
        .is_empty());
    assert!(store
        .edges_with(Some(EdgeKind::Exercises), None, None)
        .unwrap()
        .is_empty());

    store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "operation_bindings",
            r#"[{"operation_id":"choose-op","step_id":"choose"},{"human_decision":{"operation_id":"missing-op","pointer":"/work_item"},"step_id":"confirm"}]"#,
            TruthClass::Asserted,
        )
        .unwrap();
    let rejected = completeness::journey_readiness(&store, &journey).unwrap();
    assert!(!rejected.surfaced);
    assert!(rejected
        .surface_gaps
        .iter()
        .any(|gap| gap.contains("canonical complete operation bindings")));
}

#[test]
fn intent_keeps_six_axes_and_journey_requires_realized_root() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let journey = authored_journey(&store, tmp.path(), "flow");
    let derived = intent(&store, "flow is executed", "implemented");
    store
        .set_facet(
            &derived.id,
            TargetKind::Node,
            "visibility",
            "user_visible",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &derived.id,
            TargetKind::Node,
            "level",
            "feature",
            TruthClass::Asserted,
        )
        .unwrap();
    derive(&store, &journey, &derived, &["choose", "confirm"]);
    ratify_and_ground(&store, &derived, "src/behavior.rs");

    let before = completeness::scorecard(&store, &derived).unwrap();
    assert_eq!(before.axes.len(), 6);
    assert_eq!(
        before
            .axes
            .iter()
            .find(|axis| axis.axis == "journey")
            .unwrap()
            .state,
        "open"
    );

    let (interface, _) = add_surface(&store, &journey);
    let surface_code = codefile(&store, "src/flow_cli.rs");
    store
        .add_edge(
            EdgeKind::Exposes,
            &interface.id,
            &surface_code.id,
            TruthClass::Asserted,
        )
        .unwrap();
    earn_s3(&store, &journey);

    let after = completeness::scorecard(&store, &derived).unwrap();
    assert_eq!(after.axes.len(), 6);
    assert_eq!(
        after
            .axes
            .iter()
            .find(|axis| axis.axis == "journey")
            .unwrap()
            .state,
        "met"
    );
}

#[test]
fn observed_graph_serves_neither_derive_nor_surface() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), true).unwrap();
    authored_journey(&store, tmp.path(), "flow");
    intent(&store, "unrooted", "planned");
    assert!(workitem::queue_items(&store, Lane::Derive)
        .unwrap()
        .is_empty());
    assert!(workitem::queue_items(&store, Lane::Surface)
        .unwrap()
        .is_empty());
    assert!(workitem::next(&store, Some(Lane::Derive))
        .unwrap()
        .is_none());
    assert!(workitem::next(&store, Some(Lane::Surface))
        .unwrap()
        .is_none());
}
