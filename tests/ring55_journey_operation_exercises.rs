//! Ring 55 — compiler-owned operation exercises bridge cross-process Journey
//! proof entries without relaxing surface ownership or S3.

use loom::journey::{
    CliOperation, InterfaceSurfaceDefinition, JourneyOperationExerciseFacet, JourneySpec,
    OperationBinding, OperationExercise, OperationOutput, OutputAssertion, OutputFormat,
    SurfaceManifest, ValueType, JOURNEY_COMPILER_VERSION, JOURNEY_SCHEMA, SURFACE_SCHEMA,
};
use loom::journey_runtime::RuntimeStatus;
use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::proofstrength::StrengthWitness;
use loom::signal;
use loom::store::Store;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

mod common;
use common::Tmp;

fn assertion(id: &str) -> OutputAssertion {
    OutputAssertion {
        id: id.into(),
        pointer: "/ok".into(),
        value_type: Some(ValueType::Boolean),
        equals: Some(json!(true)),
        source: None,
    }
}

fn base_operation() -> CliOperation {
    CliOperation {
        id: "publish-http-operation".into(),
        summary: "Publish through the public CLI".into(),
        argv: vec![
            "python3".into(),
            "-c".into(),
            "import json; print(json.dumps({'ok': True}))".into(),
        ],
        environment: Vec::new(),
        read_only: true,
        timeout_seconds: None,
        arguments: Vec::new(),
        output: OperationOutput {
            format: OutputFormat::Json,
            captures: Vec::new(),
            assertions: vec![assertion("publish-http-operation")],
            redact: Vec::new(),
        },
        exercises: Vec::new(),
    }
}

fn surface_with(exercises: Vec<OperationExercise>) -> InterfaceSurfaceDefinition {
    let mut operation = base_operation();
    operation.exercises = exercises;
    InterfaceSurfaceDefinition {
        id: "publish-cli".into(),
        title: "Publish CLI".into(),
        identity: "publish".into(),
        codefile: "src/cli.rs".into(),
        locator: "run_publish".into(),
        operations: vec![operation],
    }
}

#[test]
fn old_manifests_remain_valid_without_exercises() {
    let surface = surface_with(Vec::new());
    surface.validate().unwrap();
    let wire = serde_json::to_value(&surface).unwrap();
    assert!(wire["operations"][0].get("exercises").is_none());
    let round: InterfaceSurfaceDefinition = serde_json::from_value(wire).unwrap();
    assert!(round.operations[0].exercises.is_empty());
}

#[test]
fn unknown_exercise_fields_fail_strict_deserialization() {
    let err = serde_json::from_value::<OperationExercise>(json!({
        "id": "publish-handler",
        "codefile": "src/handler.rs",
        "locator": "post_blueprint",
        "observed_by": "publish-http-operation",
        "extra": true
    }))
    .expect_err("unknown fields must fail closed");
    assert!(err.to_string().contains("unknown field"), "{err}");
}

#[test]
fn exercise_validation_fails_closed_on_bad_declarations() {
    let tmp = Tmp::new();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/cli.rs"), "pub fn run_publish() {}\n").unwrap();
    std::fs::write(
        tmp.path().join("src/handler.rs"),
        "pub fn post_blueprint() {}\n",
    )
    .unwrap();
    let store = Store::init(tmp.path(), Some("exercise-validate"), false).unwrap();
    for path in ["src/cli.rs", "src/handler.rs"] {
        store
            .add_node(NodeType::CodeFile, path, "", "", json!({}))
            .unwrap();
    }

    let duplicate = surface_with(vec![
        OperationExercise {
            id: "publish-handler".into(),
            codefile: "src/handler.rs".into(),
            locator: "post_blueprint".into(),
            observed_by: "publish-http-operation".into(),
        },
        OperationExercise {
            id: "publish-handler".into(),
            codefile: "src/handler.rs".into(),
            locator: "post_blueprint".into(),
            observed_by: "publish-http-operation".into(),
        },
    ]);
    assert!(duplicate
        .validate()
        .unwrap_err()
        .to_string()
        .contains("duplicate"));

    let missing_assertion = surface_with(vec![OperationExercise {
        id: "publish-handler".into(),
        codefile: "src/handler.rs".into(),
        locator: "post_blueprint".into(),
        observed_by: "other-operation-assertion".into(),
    }]);
    assert!(missing_assertion
        .validate()
        .unwrap_err()
        .to_string()
        .contains("observed_by"));

    let anchor = surface_with(vec![OperationExercise {
        id: "publish-handler".into(),
        codefile: "src/handler.rs".into(),
        locator: "anchor:deadbeef".into(),
        observed_by: "publish-http-operation".into(),
    }]);
    assert!(anchor
        .validate()
        .unwrap_err()
        .to_string()
        .contains("navigation-only"));

    let missing_file = SurfaceManifest {
        schema: SURFACE_SCHEMA.into(),
        journey_id: "publish".into(),
        journey_hash: "hash".into(),
        surface: surface_with(vec![OperationExercise {
            id: "publish-handler".into(),
            codefile: "src/missing.rs".into(),
            locator: "post_blueprint".into(),
            observed_by: "publish-http-operation".into(),
        }]),
        setup: None,
        bindings: vec![loom::journey::SurfaceBinding::Operation(OperationBinding {
            step_id: "publish".into(),
            operation_id: "publish-http-operation".into(),
        })],
    };
    assert!(missing_file
        .validate_exercises_for_store(&store)
        .unwrap_err()
        .to_string()
        .contains("codefile"));

    let unresolved = SurfaceManifest {
        schema: SURFACE_SCHEMA.into(),
        journey_id: "publish".into(),
        journey_hash: "hash".into(),
        surface: surface_with(vec![OperationExercise {
            id: "publish-handler".into(),
            codefile: "src/handler.rs".into(),
            locator: "missing_symbol".into(),
            observed_by: "publish-http-operation".into(),
        }]),
        setup: None,
        bindings: vec![loom::journey::SurfaceBinding::Operation(OperationBinding {
            step_id: "publish".into(),
            operation_id: "publish-http-operation".into(),
        })],
    };
    assert!(unresolved
        .validate_exercises_for_store(&store)
        .unwrap_err()
        .to_string()
        .contains("locator"));
}

struct CrossProcessFixture {
    tmp: Tmp,
    store: Store,
    validation_id: String,
    cli_id: String,
    handler_id: String,
    journey_id: String,
}

fn default_exercise() -> OperationExercise {
    OperationExercise {
        id: "publish-handler".into(),
        codefile: "src/handler.rs".into(),
        locator: "post_blueprint".into(),
        observed_by: "publish-http-operation".into(),
    }
}

fn cross_process_fixture(exercises: Vec<OperationExercise>) -> CrossProcessFixture {
    let tmp = Tmp::new();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::create_dir_all(tmp.path().join("journeys")).unwrap();
    // Public adapter does not call the realizing handler — models a process boundary.
    std::fs::write(
        tmp.path().join("src/cli.rs"),
        "pub fn run_publish() -> &'static str { \"ok\" }\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("src/handler.rs"),
        "pub fn post_blueprint() -> &'static str { \"stored\" }\npub fn other_entry() -> &'static str { \"other\" }\n",
    )
    .unwrap();

    let store = Store::init(tmp.path(), Some("operation-exercises"), false).unwrap();
    let spec: JourneySpec = serde_json::from_value(json!({
        "schema": JOURNEY_SCHEMA,
        "id": "publish.flow",
        "name": "Publish",
        "actor": "operator",
        "goal": "Publish a blueprint",
        "inputs": {},
        "preconditions": [],
        "steps": [{"id":"publish","name":"Publish","action":"publishes","expects":[],"produces":{}}],
        "profiles":{"proof":{"inputs":{},"workspace":{}}}
    }))
    .unwrap();
    let artifact = "journeys/publish.flow.yaml";
    std::fs::write(
        tmp.path().join(artifact),
        serde_norway::to_string(&spec).unwrap(),
    )
    .unwrap();
    let journey_hash = spec.semantic_hash().unwrap();
    let journey = store
        .add_node(
            NodeType::Journey,
            "publish.flow",
            "Publish",
            "authored",
            json!({
                "schema": JOURNEY_SCHEMA,
                "stable_id": "publish.flow",
                "name": "Publish",
                "actor": "operator",
                "goal": "Publish a blueprint",
                "artifact": artifact,
                "semantic_hash": journey_hash,
                "input_ids": [],
                "preconditions": [],
                "step_ids": ["publish"],
                "output_ids": [],
                "profile_ids": ["proof"]
            }),
        )
        .unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "publish blueprint",
            "stores a blueprint through the public publish path",
            "implemented",
            json!({}),
        )
        .unwrap();
    store
        .ratify_intent(&intent.id, "fixture wants publish", "test")
        .unwrap();
    let derives = store
        .ensure_edge(EdgeKind::Derives, &journey.id, &intent.id)
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
            "[\"publish\"]",
            TruthClass::Asserted,
        )
        .unwrap();

    let cli = store
        .add_node(NodeType::CodeFile, "src/cli.rs", "", "", json!({}))
        .unwrap();
    let handler = store
        .add_node(NodeType::CodeFile, "src/handler.rs", "", "", json!({}))
        .unwrap();
    let realizes = store
        .ensure_edge(EdgeKind::Implements, &intent.id, &handler.id)
        .unwrap();
    store
        .set_facet(
            &realizes.id,
            TargetKind::Edge,
            "locator",
            "post_blueprint",
            TruthClass::Asserted,
        )
        .unwrap();

    let mut operation = base_operation();
    operation.exercises = exercises;
    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            "publish-cli",
            "Publish CLI",
            "active",
            json!({
                "schema":"loom.interface-surface/v1",
                "stable_id":"publish-cli",
                "title":"Publish CLI",
                "kind":"cli",
                "identity":"publish",
                "codefile":"src/cli.rs",
                "locator":"run_publish",
                "operations":[operation]
            }),
        )
        .unwrap();
    let surfaces = store
        .ensure_edge(EdgeKind::Surfaces, &journey.id, &surface.id)
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
            "[{\"operation_id\":\"publish-http-operation\",\"step_id\":\"publish\"}]",
            TruthClass::Asserted,
        )
        .unwrap();
    let exposes = store
        .ensure_edge(EdgeKind::Exposes, &surface.id, &cli.id)
        .unwrap();
    store
        .set_facet(
            &exposes.id,
            TargetKind::Edge,
            "locator",
            "run_publish",
            TruthClass::Asserted,
        )
        .unwrap();

    let surface_hash = loom::journey::surface_projection_hash(&store, &journey)
        .unwrap()
        .unwrap();
    let validation = store
        .add_node(
            NodeType::Validation,
            "journey:publish.flow:proof",
            "compiled Journey proof",
            "not_run",
            json!({
                "type":"journey",
                "profile":"proof",
                "journey_hash":journey_hash,
                "surface_hash":surface_hash,
                "compiler_version":JOURNEY_COMPILER_VERSION
            }),
        )
        .unwrap();
    store
        .ensure_edge(EdgeKind::Proves, &validation.id, &journey.id)
        .unwrap();
    store
        .ensure_edge(EdgeKind::Validates, &validation.id, &intent.id)
        .unwrap();
    store
        .ensure_edge(EdgeKind::Calls, &validation.id, &surface.id)
        .unwrap();

    let public = store
        .ensure_edge(EdgeKind::Exercises, &validation.id, &cli.id)
        .unwrap();
    store
        .set_facet(
            &public.id,
            TargetKind::Edge,
            "locator",
            "run_publish",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &public.id,
            TargetKind::Edge,
            "surface_locator",
            "run_publish",
            TruthClass::Asserted,
        )
        .unwrap();

    if !operation.exercises.is_empty() {
        let downstream = store
            .ensure_edge(EdgeKind::Exercises, &validation.id, &handler.id)
            .unwrap();
        let mut locators: Vec<String> = operation
            .exercises
            .iter()
            .map(|exercise| exercise.locator.clone())
            .collect();
        locators.sort();
        locators.dedup();
        store
            .set_facet(
                &downstream.id,
                TargetKind::Edge,
                "locator",
                &locators.join(";"),
                TruthClass::Asserted,
            )
            .unwrap();
        let mut facet: Vec<JourneyOperationExerciseFacet> = operation
            .exercises
            .iter()
            .map(|exercise| JourneyOperationExerciseFacet {
                operation_id: operation.id.clone(),
                exercise_id: exercise.id.clone(),
                observed_by: exercise.observed_by.clone(),
                locator: exercise.locator.clone(),
            })
            .collect();
        facet.sort_by(|left, right| {
            left.operation_id
                .cmp(&right.operation_id)
                .then_with(|| left.exercise_id.cmp(&right.exercise_id))
        });
        store
            .set_facet(
                &downstream.id,
                TargetKind::Edge,
                "journey_operation_exercises",
                &serde_json::to_string(&facet).unwrap(),
                TruthClass::Asserted,
            )
            .unwrap();
    }

    CrossProcessFixture {
        tmp,
        store,
        validation_id: validation.id,
        cli_id: cli.id,
        handler_id: handler.id,
        journey_id: journey.id,
    }
}

fn compiled_surface_proof(
    fixture: &CrossProcessFixture,
) -> (
    loom::journey::JourneySpec,
    loom::journey_runtime::CompiledJourneyProof,
) {
    let journey = fixture
        .store
        .get_node(&fixture.journey_id)
        .unwrap()
        .unwrap();
    let spec = loom::journey::parse(
        &fixture
            .tmp
            .path()
            .join(journey.body["artifact"].as_str().unwrap()),
    )
    .unwrap();
    let surface_hash = fixture
        .store
        .get_node(&fixture.validation_id)
        .unwrap()
        .unwrap()
        .body["surface_hash"]
        .as_str()
        .unwrap()
        .to_string();
    let surfaces = fixture
        .store
        .edges_with(Some(EdgeKind::Surfaces), Some(&fixture.journey_id), None)
        .unwrap();
    let surface = fixture.store.get_node(&surfaces[0].to_id).unwrap().unwrap();
    let operations: Vec<CliOperation> =
        serde_json::from_value(surface.body["operations"].clone()).unwrap();
    let proof = loom::journey_runtime::compile(
        &spec,
        &surface_hash,
        "proof",
        operations,
        &[OperationBinding {
            step_id: "publish".into(),
            operation_id: "publish-http-operation".into(),
        }],
    )
    .unwrap();
    (spec, proof)
}

/// Settle a compiled run without sync: verdicts, status, and an immediate
/// regrade. Used where the test must observe grading itself before any
/// sync/doctor pass can touch the graph.
fn settle_compiled(fixture: &CrossProcessFixture) {
    let (spec, proof) = compiled_surface_proof(fixture);
    let observed = loom::journey_runtime::execute_observed(
        fixture.tmp.path(),
        &spec,
        &proof,
        &BTreeMap::new(),
    );
    assert_eq!(
        observed.report().status,
        RuntimeStatus::Passed,
        "{:#?}",
        observed.report()
    );
    loom::journey::settle_compiled_validation(&fixture.store, &fixture.validation_id, &observed)
        .unwrap();
}

fn settle_with_assertions(fixture: &CrossProcessFixture) {
    settle_compiled(fixture);
    for kind in [
        EdgeKind::Calls,
        EdgeKind::Exercises,
        EdgeKind::Derives,
        EdgeKind::Surfaces,
        EdgeKind::Exposes,
        EdgeKind::Implements,
    ] {
        for edge in fixture.store.edges_with(Some(kind), None, None).unwrap() {
            let _ = fixture.store.record_verdict(
                &edge.id,
                loom::model::InspectionStatus::Passing,
                "fixture",
                "fixture",
                1.0,
                "test",
            );
        }
    }
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
}

fn downstream_edge(fixture: &CrossProcessFixture) -> loom::model::Edge {
    fixture
        .store
        .edges_with(
            Some(EdgeKind::Exercises),
            Some(&fixture.validation_id),
            Some(&fixture.handler_id),
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
}

fn set_exercise_facet(fixture: &CrossProcessFixture, raw: &str) {
    let edge = downstream_edge(fixture);
    fixture
        .store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_operation_exercises",
            raw,
            TruthClass::Asserted,
        )
        .unwrap();
}

fn witness(store: &Store, validation_id: &str) -> StrengthWitness {
    let raw = store
        .get_facet(validation_id, TargetKind::Node, "proof_strength")
        .unwrap()
        .expect("proof_strength facet");
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn compile_shaped_topology_keeps_public_and_downstream_entries() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    let exercises = fixture
        .store
        .edges_with(
            Some(EdgeKind::Exercises),
            Some(&fixture.validation_id),
            None,
        )
        .unwrap();
    assert_eq!(exercises.len(), 2);
    let targets: BTreeMap<_, _> = exercises
        .into_iter()
        .map(|edge| (edge.to_id.clone(), edge))
        .collect();
    assert!(targets.contains_key(&fixture.cli_id));
    assert!(targets.contains_key(&fixture.handler_id));
    let downstream = &targets[&fixture.handler_id];
    let facet: Vec<JourneyOperationExerciseFacet> = serde_json::from_str(
        &fixture
            .store
            .get_facet(
                &downstream.id,
                TargetKind::Edge,
                "journey_operation_exercises",
            )
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(facet.len(), 1);
    assert_eq!(facet[0].exercise_id, "publish-handler");
    assert_eq!(facet[0].observed_by, "publish-http-operation");
}

#[test]
fn multiple_entries_in_one_codefile_aggregate_without_losing_provenance() {
    let fixture = cross_process_fixture(vec![
        default_exercise(),
        OperationExercise {
            id: "other-handler".into(),
            codefile: "src/handler.rs".into(),
            locator: "other_entry".into(),
            observed_by: "publish-http-operation".into(),
        },
    ]);
    let edge = fixture
        .store
        .edges_with(
            Some(EdgeKind::Exercises),
            Some(&fixture.validation_id),
            Some(&fixture.handler_id),
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    // Deterministic aggregation: entries sorted by (operation, exercise),
    // locators semicolon-joined and sorted — exact provenance, nothing lost.
    let facet: Vec<JourneyOperationExerciseFacet> = serde_json::from_str(
        &fixture
            .store
            .get_facet(&edge.id, TargetKind::Edge, "journey_operation_exercises")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(facet.len(), 2);
    assert_eq!(facet[0].exercise_id, "other-handler");
    assert_eq!(facet[0].locator, "other_entry");
    assert_eq!(facet[1].exercise_id, "publish-handler");
    assert_eq!(facet[1].locator, "post_blueprint");
    assert_eq!(
        fixture
            .store
            .get_facet(&edge.id, TargetKind::Edge, "locator")
            .unwrap()
            .as_deref(),
        Some("other_entry;post_blueprint")
    );
    settle_with_assertions(&fixture);
    let w = witness(&fixture.store, &fixture.validation_id);
    assert_eq!(w.grade, "S3");
    let evidence = w.call_evidence.expect("call evidence");
    assert_eq!(evidence.source, "journey_operation_exercise");
    assert_eq!(evidence.file, "src/handler.rs");
    assert_eq!(evidence.entry_symbol.as_deref(), Some("post_blueprint"));
    assert_eq!(evidence.exercise_id.as_deref(), Some("publish-handler"));
    assert_eq!(
        evidence.observed_by.as_deref(),
        Some("publish-http-operation")
    );
}

#[test]
fn recompile_shaped_cleanup_removes_obsolete_downstream_entry() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    assert_eq!(
        fixture
            .store
            .edges_with(
                Some(EdgeKind::Exercises),
                Some(&fixture.validation_id),
                None
            )
            .unwrap()
            .len(),
        2
    );
    let obsolete = fixture
        .store
        .edges_with(
            Some(EdgeKind::Exercises),
            Some(&fixture.validation_id),
            Some(&fixture.handler_id),
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    fixture.store.delete_edge(&obsolete.id).unwrap();
    assert_eq!(
        fixture
            .store
            .edges_with(
                Some(EdgeKind::Exercises),
                Some(&fixture.validation_id),
                None
            )
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn passed_cross_process_journey_reaches_s3_through_exercised_handler() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    settle_with_assertions(&fixture);
    let w = witness(&fixture.store, &fixture.validation_id);
    assert_eq!(w.grade, "S3", "{w:?}");
    let evidence = w.call_evidence.expect("call evidence");
    assert_eq!(evidence.source, "journey_operation_exercise");
    assert_eq!(evidence.entry_symbol.as_deref(), Some("post_blueprint"));
    assert_eq!(evidence.grounded_symbol.as_deref(), Some("post_blueprint"));
    assert!(evidence.s3_eligible);
}

#[test]
fn remains_s2_when_handler_unreached_or_only_declared() {
    // Assertion present but compiled locator disagrees with the accepted surface.
    let missed = cross_process_fixture(vec![default_exercise()]);
    let edge = missed
        .store
        .edges_with(
            Some(EdgeKind::Exercises),
            Some(&missed.validation_id),
            Some(&missed.handler_id),
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    missed
        .store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "locator",
            "other_entry",
            TruthClass::Asserted,
        )
        .unwrap();
    let facet = vec![JourneyOperationExerciseFacet {
        operation_id: "publish-http-operation".into(),
        exercise_id: "publish-handler".into(),
        observed_by: "publish-http-operation".into(),
        locator: "other_entry".into(),
    }];
    missed
        .store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_operation_exercises",
            &serde_json::to_string(&facet).unwrap(),
            TruthClass::Asserted,
        )
        .unwrap();
    settle_compiled(&missed);
    let missed_w = witness(&missed.store, &missed.validation_id);
    assert_eq!(missed_w.grade, "S2", "{missed_w:?}");
    assert!(
        missed_w
            .next
            .contains("does not match the accepted surface")
            || missed_w.next.contains("does not reach"),
        "{}",
        missed_w.next
    );
    let missed_issues = signal::doctor(&missed.store).unwrap();
    assert!(
        missed_issues
            .iter()
            .any(|issue| issue.kind == "broken_journey_proof_chain"),
        "{missed_issues:?}"
    );

    // No exercise declared at all.
    let none = cross_process_fixture(Vec::new());
    settle_with_assertions(&none);
    let none_w = witness(&none.store, &none.validation_id);
    assert_eq!(none_w.grade, "S2", "{none_w:?}");
    assert!(
        none_w.next.contains("no operation exercise"),
        "{}",
        none_w.next
    );
    assert!(!none_w.next.contains("edge exercises"));
}

#[test]
fn exercise_change_invalidates_prior_passing_proof_via_surface_hash() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    settle_with_assertions(&fixture);
    assert_eq!(witness(&fixture.store, &fixture.validation_id).grade, "S3");

    let mut body = fixture
        .store
        .get_node(
            &fixture
                .store
                .edges_with(Some(EdgeKind::Calls), Some(&fixture.validation_id), None)
                .unwrap()[0]
                .to_id,
        )
        .unwrap()
        .unwrap()
        .body;
    body["operations"][0]["exercises"][0]["locator"] = json!("other_entry");
    let surface_id = fixture
        .store
        .edges_with(Some(EdgeKind::Calls), Some(&fixture.validation_id), None)
        .unwrap()[0]
        .to_id
        .clone();
    fixture.store.set_node_body(&surface_id, &body).unwrap();
    let journey = fixture
        .store
        .get_node(&fixture.journey_id)
        .unwrap()
        .unwrap();
    let new_hash = loom::journey::surface_projection_hash(&fixture.store, &journey)
        .unwrap()
        .unwrap();
    let mut validation_body = fixture
        .store
        .get_node(&fixture.validation_id)
        .unwrap()
        .unwrap()
        .body;
    let old_hash = validation_body["surface_hash"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(old_hash, new_hash);
    validation_body["surface_hash"] = json!(old_hash);
    fixture
        .store
        .set_node_body(&fixture.validation_id, &validation_body)
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    // Drifted surface hash makes the compiler-owned proof non-current → S0.
    let drifted = witness(&fixture.store, &fixture.validation_id);
    assert_eq!(drifted.grade, "S0", "{drifted:?}");
}

#[test]
fn manual_topology_mutation_remains_rejected_for_compiler_owned_journeys() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    let graph = fixture.tmp.path().to_path_buf();
    let validation = fixture.validation_id.clone();
    drop(fixture.store);
    let err = loom::commands::run(loom::cli::Cli {
        graph: Some(graph),
        json: true,
        command: Some(loom::cli::Command::Edge {
            cmd: loom::cli::EdgeCmd::Exercises {
                validation,
                codefile: "src/handler.rs".into(),
                locator: Some("post_blueprint".into()),
            },
        }),
    })
    .expect_err("edge exercises must refuse compiler-owned Journey topology");
    assert!(
        format!("{err:#}").contains("compiler-owned Journey"),
        "{err:#}"
    );
}

#[test]
fn doctor_accepts_complete_chain_and_flags_malformed_provenance() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    settle_with_assertions(&fixture);
    let clean = signal::doctor(&fixture.store).unwrap();
    assert!(
        clean
            .iter()
            .all(|issue| issue.kind != "broken_journey_proof_chain"),
        "{clean:?}"
    );

    let edge = fixture
        .store
        .edges_with(
            Some(EdgeKind::Exercises),
            Some(&fixture.validation_id),
            Some(&fixture.handler_id),
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    fixture
        .store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_operation_exercises",
            "{not-json",
            TruthClass::Asserted,
        )
        .unwrap();
    let issues = signal::doctor(&fixture.store).unwrap();
    assert!(
        issues
            .iter()
            .any(|issue| issue.kind == "broken_journey_proof_chain"),
        "{issues:?}"
    );
}

#[test]
fn malformed_exercise_provenance_cannot_earn_s3_even_before_doctor() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    set_exercise_facet(&fixture, "{definitely-not-json");
    settle_compiled(&fixture);
    let w = witness(&fixture.store, &fixture.validation_id);
    assert_eq!(w.grade, "S2", "{w:?}");
    assert!(
        w.next.contains("does not match the accepted surface"),
        "{}",
        w.next
    );
    assert!(!w.next.contains("edge exercises"));
    // The malformed provenance must never degrade into an ordinary public
    // entry claim: no S3-eligible witness, and any visible evidence is the
    // explicit mismatch diagnostic.
    if let Some(evidence) = &w.call_evidence {
        assert_eq!(evidence.source, "journey_provenance_mismatch");
        assert!(!evidence.s3_eligible);
    }
}

#[test]
fn missing_exercise_provenance_cannot_fall_back_to_public_entry() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    fixture
        .store
        .clear_facet(
            &downstream_edge(&fixture).id,
            TargetKind::Edge,
            "journey_operation_exercises",
        )
        .unwrap();
    settle_compiled(&fixture);
    let w = witness(&fixture.store, &fixture.validation_id);
    assert_eq!(w.grade, "S2", "{w:?}");
    assert!(
        w.next.contains("does not match the accepted surface"),
        "{}",
        w.next
    );
    // No validation_grounding credit leaked through the aggregate locator.
    assert!(w.call_evidence.is_none_or(|e| !e.s3_eligible));
    assert!(!w.next.contains("edge exercises"));
}

#[test]
fn forged_exercise_provenance_is_doctor_invalid_and_s3_ineligible() {
    let canonical = r#"[{"operation_id":"publish-http-operation","exercise_id":"publish-handler","observed_by":"publish-http-operation","locator":"post_blueprint"}]"#;
    let cases: Vec<(&str, &str)> = vec![
        (
            "wrong operation",
            r#"[{"operation_id":"ghost-operation","exercise_id":"publish-handler","observed_by":"publish-http-operation","locator":"post_blueprint"}]"#,
        ),
        (
            "wrong exercise id",
            r#"[{"operation_id":"publish-http-operation","exercise_id":"ghost-handler","observed_by":"publish-http-operation","locator":"post_blueprint"}]"#,
        ),
        (
            "wrong assertion",
            r#"[{"operation_id":"publish-http-operation","exercise_id":"publish-handler","observed_by":"ghost-assertion","locator":"post_blueprint"}]"#,
        ),
        (
            "wrong locator",
            r#"[{"operation_id":"publish-http-operation","exercise_id":"publish-handler","observed_by":"publish-http-operation","locator":"other_entry"}]"#,
        ),
        ("empty entries", r#"[]"#),
        (
            "duplicate entries",
            r#"[{"operation_id":"publish-http-operation","exercise_id":"publish-handler","observed_by":"publish-http-operation","locator":"post_blueprint"},{"operation_id":"publish-http-operation","exercise_id":"publish-handler","observed_by":"publish-http-operation","locator":"post_blueprint"}]"#,
        ),
    ];
    for (case, forged) in cases {
        let fixture = cross_process_fixture(vec![default_exercise()]);
        set_exercise_facet(&fixture, forged);
        settle_compiled(&fixture);
        let w = witness(&fixture.store, &fixture.validation_id);
        assert_eq!(w.grade, "S2", "{case}: {w:?}");
        assert!(
            w.next.contains("does not match the accepted surface"),
            "{case}: {}",
            w.next
        );
        assert!(!w.next.contains("edge exercises"), "{case}");
        let issues = signal::doctor(&fixture.store).unwrap();
        assert!(
            issues
                .iter()
                .any(|issue| issue.kind == "broken_journey_proof_chain"),
            "{case}: {issues:?}"
        );
    }
    // Restoring the exact canonical facet must restore S3 — the gate is
    // exact semantic agreement, not one-way corruption.
    let fixture = cross_process_fixture(vec![default_exercise()]);
    set_exercise_facet(&fixture, canonical);
    settle_with_assertions(&fixture);
    assert_eq!(witness(&fixture.store, &fixture.validation_id).grade, "S3");
}

#[test]
fn unknown_exercise_target_codefile_is_doctor_invalid_and_s3_ineligible() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    std::fs::write(
        fixture.tmp.path().join("src/extra.rs"),
        "pub fn extra() {}\n",
    )
    .unwrap();
    let extra = fixture
        .store
        .add_node(NodeType::CodeFile, "src/extra.rs", "", "", json!({}))
        .unwrap();
    let forged = fixture
        .store
        .ensure_edge(EdgeKind::Exercises, &fixture.validation_id, &extra.id)
        .unwrap();
    fixture
        .store
        .set_facet(
            &forged.id,
            TargetKind::Edge,
            "locator",
            "extra",
            TruthClass::Asserted,
        )
        .unwrap();
    fixture
        .store
        .set_facet(
            &forged.id,
            TargetKind::Edge,
            "journey_operation_exercises",
            r#"[{"operation_id":"publish-http-operation","exercise_id":"publish-handler","observed_by":"publish-http-operation","locator":"extra"}]"#,
            TruthClass::Asserted,
        )
        .unwrap();
    settle_compiled(&fixture);
    let w = witness(&fixture.store, &fixture.validation_id);
    assert_eq!(w.grade, "S2", "{w:?}");
    assert!(
        w.next.contains("does not match the accepted surface"),
        "{}",
        w.next
    );
    let issues = signal::doctor(&fixture.store).unwrap();
    assert!(
        issues
            .iter()
            .any(|issue| issue.kind == "broken_journey_proof_chain"),
        "{issues:?}"
    );
}

#[test]
fn forged_aggregate_locator_facet_is_doctor_invalid_and_s3_ineligible() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    let edge = downstream_edge(&fixture);
    fixture
        .store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "locator",
            "forged_entry",
            TruthClass::Asserted,
        )
        .unwrap();
    settle_compiled(&fixture);
    let w = witness(&fixture.store, &fixture.validation_id);
    assert_eq!(w.grade, "S2", "{w:?}");
    assert!(
        w.next.contains("does not match the accepted surface"),
        "{}",
        w.next
    );
    let issues = signal::doctor(&fixture.store).unwrap();
    assert!(
        issues
            .iter()
            .any(|issue| issue.kind == "broken_journey_proof_chain"),
        "{issues:?}"
    );
}

#[test]
fn missing_public_entry_file_fails_closed() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    settle_compiled(&fixture);
    std::fs::remove_file(fixture.tmp.path().join("src/cli.rs")).unwrap();
    loom::proofstrength::recompute(&fixture.store, fixture.tmp.path()).unwrap();
    let w = witness(&fixture.store, &fixture.validation_id);
    assert_ne!(w.grade, "S3", "{w:?}");
    assert!(
        w.grade == "S0" || w.grade == "S2",
        "missing public entry must fail closed, got {w:?}"
    );
    let issues = signal::doctor(&fixture.store).unwrap();
    assert!(
        issues
            .iter()
            .any(|issue| issue.kind == "broken_journey_proof_chain"),
        "{issues:?}"
    );
}

#[test]
fn unbound_operation_exercise_is_doctor_invalid_and_s3_ineligible() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    settle_compiled(&fixture);
    // Add a second operation to the accepted surface that NO step binds. Its
    // exercise must not contribute provenance — a facet naming it is forged.
    let surface_id = fixture
        .store
        .edges_with(Some(EdgeKind::Calls), Some(&fixture.validation_id), None)
        .unwrap()[0]
        .to_id
        .clone();
    let mut body = fixture.store.get_node(&surface_id).unwrap().unwrap().body;
    let mut extra_operation = base_operation();
    extra_operation.id = "unbound-http-operation".into();
    extra_operation.exercises = vec![OperationExercise {
        id: "unbound-handler".into(),
        codefile: "src/handler.rs".into(),
        locator: "post_blueprint".into(),
        observed_by: "publish-http-operation".into(),
    }];
    body["operations"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::to_value(extra_operation).unwrap());
    fixture.store.set_node_body(&surface_id, &body).unwrap();
    let journey = fixture
        .store
        .get_node(&fixture.journey_id)
        .unwrap()
        .unwrap();
    let new_hash = loom::journey::surface_projection_hash(&fixture.store, &journey)
        .unwrap()
        .unwrap();
    let mut validation_body = fixture
        .store
        .get_node(&fixture.validation_id)
        .unwrap()
        .unwrap()
        .body;
    validation_body["surface_hash"] = json!(new_hash);
    fixture
        .store
        .set_node_body(&fixture.validation_id, &validation_body)
        .unwrap();
    // Forge the facet to name the UNBOUND operation's exercise.
    set_exercise_facet(
        &fixture,
        r#"[{"operation_id":"unbound-http-operation","exercise_id":"unbound-handler","observed_by":"publish-http-operation","locator":"post_blueprint"}]"#,
    );
    loom::proofstrength::recompute(&fixture.store, fixture.tmp.path()).unwrap();
    let w = witness(&fixture.store, &fixture.validation_id);
    assert_eq!(w.grade, "S2", "{w:?}");
    assert!(
        w.next.contains("does not match the accepted surface"),
        "{}",
        w.next
    );
    let issues = signal::doctor(&fixture.store).unwrap();
    assert!(
        issues
            .iter()
            .any(|issue| issue.kind == "broken_journey_proof_chain"),
        "{issues:?}"
    );
}

#[test]
fn compiler_v3_validation_is_not_current_and_sync_resets_it() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    let mut body = fixture
        .store
        .get_node(&fixture.validation_id)
        .unwrap()
        .unwrap()
        .body;
    body["compiler_version"] = json!("3");
    fixture
        .store
        .set_node_body(&fixture.validation_id, &body)
        .unwrap();
    loom::proofstrength::recompute(&fixture.store, fixture.tmp.path()).unwrap();
    // A stale compiler version cannot be settled, and grading fails closed.
    let w = witness(&fixture.store, &fixture.validation_id);
    assert_eq!(w.grade, "S0", "{w:?}");
    // Sync resets the validation through the normal compiler-owned mechanism.
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    assert_eq!(
        fixture
            .store
            .get_node(&fixture.validation_id)
            .unwrap()
            .unwrap()
            .status,
        "not_run"
    );
    let journey = fixture
        .store
        .get_node(&fixture.journey_id)
        .unwrap()
        .unwrap();
    let readiness = loom::completeness::journey_readiness(&fixture.store, &journey).unwrap();
    assert!(!readiness.compiled, "v3 validation must not be current");
    assert_eq!(
        loom::proofstrength::of(&fixture.store, &fixture.validation_id).unwrap(),
        loom::proofstrength::Strength::S0
    );
}

#[test]
fn pre_fix_compiler_v4_evidence_is_not_current_under_v5() {
    // Pre-fix compiler-v4 validations have no structured assertion evidence
    // and are not the current compiler. A graph that already earned S3 under
    // a later compiler must lose that standing when rewritten to the pre-fix
    // v4 contract; sync must stale it; it cannot retain proven readiness.
    let fixture = cross_process_fixture(vec![default_exercise()]);
    settle_with_assertions(&fixture);
    assert_eq!(witness(&fixture.store, &fixture.validation_id).grade, "S3");
    let mut body = fixture
        .store
        .get_node(&fixture.validation_id)
        .unwrap()
        .unwrap()
        .body;
    body["compiler_version"] = json!("4");
    fixture
        .store
        .set_node_body(&fixture.validation_id, &body)
        .unwrap();
    loom::proofstrength::recompute(&fixture.store, fixture.tmp.path()).unwrap();
    let w = witness(&fixture.store, &fixture.validation_id);
    assert_eq!(w.grade, "S0", "{w:?}");
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    assert_eq!(
        fixture
            .store
            .get_node(&fixture.validation_id)
            .unwrap()
            .unwrap()
            .status,
        "not_run"
    );
    let journey = fixture
        .store
        .get_node(&fixture.journey_id)
        .unwrap()
        .unwrap();
    let readiness = loom::completeness::journey_readiness(&fixture.store, &journey).unwrap();
    assert!(!readiness.compiled, "v4 validation must not be current");
    assert!(!readiness.proven, "v4 validation must not be proven");
    assert_eq!(
        loom::proofstrength::of(&fixture.store, &fixture.validation_id).unwrap(),
        loom::proofstrength::Strength::S0
    );
}

#[test]
fn compiler_v5_rerun_becomes_current_and_can_earn_s3() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    let mut body = fixture
        .store
        .get_node(&fixture.validation_id)
        .unwrap()
        .unwrap()
        .body;
    body["compiler_version"] = json!("4");
    fixture
        .store
        .set_node_body(&fixture.validation_id, &body)
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    body["compiler_version"] = json!(JOURNEY_COMPILER_VERSION);
    fixture
        .store
        .set_node_body(&fixture.validation_id, &body)
        .unwrap();
    settle_with_assertions(&fixture);
    assert_eq!(witness(&fixture.store, &fixture.validation_id).grade, "S3");
    let journey = fixture
        .store
        .get_node(&fixture.journey_id)
        .unwrap()
        .unwrap();
    let readiness = loom::completeness::journey_readiness(&fixture.store, &journey).unwrap();
    assert!(readiness.compiled);
    assert!(readiness.proven);
}

#[test]
fn large_passed_assertion_report_keeps_structural_provenance_and_earns_s3() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    let surfaces = fixture
        .store
        .edges_with(Some(EdgeKind::Surfaces), Some(&fixture.journey_id), None)
        .unwrap();
    let surface_id = surfaces[0].to_id.clone();
    let mut body = fixture.store.get_node(&surface_id).unwrap().unwrap().body;
    let mut operation: CliOperation =
        serde_json::from_value(body["operations"][0].clone()).unwrap();
    let mut payload = serde_json::Map::new();
    payload.insert("ok".into(), json!(true));
    for i in 0..600 {
        let key = format!("k{i:04}");
        payload.insert(key.clone(), json!(true));
        operation
            .output
            .assertions
            .push(assertion(&format!("bulk-{i:04}")));
        operation.output.assertions.last_mut().unwrap().pointer = format!("/{key}");
    }
    operation.argv = vec![
        "python3".into(),
        "-c".into(),
        format!(
            "print({:?})",
            serde_json::Value::Object(payload).to_string()
        ),
    ];
    body["operations"][0] = serde_json::to_value(&operation).unwrap();
    fixture.store.set_node_body(&surface_id, &body).unwrap();
    let journey = fixture
        .store
        .get_node(&fixture.journey_id)
        .unwrap()
        .unwrap();
    let new_hash = loom::journey::surface_projection_hash(&fixture.store, &journey)
        .unwrap()
        .unwrap();
    let mut validation_body = fixture
        .store
        .get_node(&fixture.validation_id)
        .unwrap()
        .unwrap()
        .body;
    validation_body["surface_hash"] = json!(new_hash);
    fixture
        .store
        .set_node_body(&fixture.validation_id, &validation_body)
        .unwrap();
    settle_with_assertions(&fixture);
    // The human-facing excerpt really was truncated into non-JSON text…
    let validates = fixture
        .store
        .edges_with(
            Some(EdgeKind::Validates),
            Some(&fixture.validation_id),
            None,
        )
        .unwrap();
    let view = fixture
        .store
        .fact(
            &loom::store::Subject::Edge(validates[0].id.clone()),
            loom::model::Claim::Verdict,
        )
        .unwrap()
        .unwrap();
    let mut saw_truncated = false;
    let mut saw_structural = false;
    for row in &view.evidence {
        let loom::evidence::Evidence::Run(run) = &row.payload else {
            continue;
        };
        if run.stdout_excerpt.contains("omitted") {
            saw_truncated = true;
        }
        if run
            .observed_assertions()
            .iter()
            .any(|a| a.group == "publish-http-operation" && a.assertion == "publish-http-operation")
        {
            saw_structural = true;
        }
    }
    assert!(saw_truncated, "excerpt should exceed the 8 KiB budget");
    assert!(saw_structural, "structured assertion evidence must persist");
    // …and grading still earns S3 from the structural evidence.
    let w = witness(&fixture.store, &fixture.validation_id);
    assert_eq!(w.grade, "S3", "{w:?}");
    let evidence = w.call_evidence.expect("call evidence");
    assert_eq!(evidence.source, "journey_operation_exercise");
    assert_eq!(evidence.entry_symbol.as_deref(), Some("post_blueprint"));
}

#[test]
fn surface_guidance_template_is_internally_consistent_and_validates() {
    // Callers replace only repository-specific CodeFile keys and locators.
    // The emitted document is otherwise a complete SurfaceManifest.
    let spec: JourneySpec = serde_json::from_value(json!({
        "schema": JOURNEY_SCHEMA,
        "id": "demo.flow",
        "name": "Demo",
        "actor": "operator",
        "goal": "Exercise the template",
        "inputs": {},
        "preconditions": [],
        "steps": [{
            "id": "authored-step-id",
            "name": "Act",
            "action": "does the thing",
            "expects": [],
            "produces": {}
        }],
        "profiles": {"proof": {"inputs": {}, "workspace": {}}}
    }))
    .unwrap();
    let hash = spec.semantic_hash().unwrap();
    let mut template = loom::journey::surface_contract_template(&spec).unwrap();
    assert!(
        template.get("setup").is_none(),
        "minimal template must not emit a setup block"
    );
    assert_eq!(
        template["surface"]["operations"][0]["id"],
        "authored-step-id-operation"
    );
    assert_eq!(
        template["bindings"][0],
        json!({"step_id":"authored-step-id","operation_id":"authored-step-id-operation"})
    );
    let surface = &template["surface"];
    let observed_by = surface["operations"][0]["exercises"][0]["observed_by"]
        .as_str()
        .unwrap();
    let assertion_ids: Vec<&str> = surface["operations"][0]["output"]["assertions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert!(
        assertion_ids.contains(&observed_by),
        "template exercise observed_by '{observed_by}' must be a declared assertion id"
    );

    let tmp = Tmp::new();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/cli.rs"), "pub fn run_publish() {}\n").unwrap();
    std::fs::write(
        tmp.path().join("src/handler.rs"),
        "pub fn post_blueprint() {}\n",
    )
    .unwrap();
    let store = Store::init(tmp.path(), Some("template-validate"), false).unwrap();
    for path in ["src/cli.rs", "src/handler.rs"] {
        store
            .add_node(NodeType::CodeFile, path, "", "", json!({}))
            .unwrap();
    }
    replace_surface_template_placeholders(&mut template);
    let manifest: SurfaceManifest = serde_json::from_value(template).expect(
        "exact emitted template must decode as a SurfaceManifest after placeholder replacement",
    );
    manifest
        .validate_for(&spec, &hash)
        .expect("exact emitted template must pass full SurfaceManifest validation");
    manifest
        .validate_setup_for_store(&store)
        .expect("exact emitted template must pass store-backed surface validation");
}

#[test]
fn surface_guidance_template_covers_every_authored_step() {
    let spec: JourneySpec = serde_json::from_value(json!({
        "schema": JOURNEY_SCHEMA,
        "id": "demo.flow",
        "name": "Demo",
        "actor": "operator",
        "goal": "Exercise the multi-step template",
        "inputs": {},
        "preconditions": [],
        "steps": [
            {
                "id": "first",
                "name": "First",
                "action": "does the first thing",
                "expects": [],
                "produces": {}
            },
            {
                "id": "second",
                "name": "Second",
                "action": "does the second thing",
                "expects": [],
                "produces": {}
            }
        ],
        "profiles": {"proof": {"inputs": {}, "workspace": {}}}
    }))
    .unwrap();
    let hash = spec.semantic_hash().unwrap();
    let mut template = loom::journey::surface_contract_template(&spec).unwrap();
    assert!(
        template.get("setup").is_none(),
        "minimal template must not emit a setup block"
    );
    assert_eq!(
        template["surface"]["operations"]
            .as_array()
            .map(Vec::len)
            .unwrap(),
        2
    );
    assert_eq!(
        template["surface"]["operations"][0]["id"],
        "first-operation"
    );
    assert_eq!(
        template["surface"]["operations"][1]["id"],
        "second-operation"
    );
    assert_eq!(
        template["bindings"],
        json!([
            {"step_id":"first","operation_id":"first-operation"},
            {"step_id":"second","operation_id":"second-operation"}
        ])
    );

    let tmp = Tmp::new();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/cli.rs"), "pub fn run_publish() {}\n").unwrap();
    std::fs::write(
        tmp.path().join("src/handler.rs"),
        "pub fn post_blueprint() {}\n",
    )
    .unwrap();
    let store = Store::init(tmp.path(), Some("template-multi-step"), false).unwrap();
    for path in ["src/cli.rs", "src/handler.rs"] {
        store
            .add_node(NodeType::CodeFile, path, "", "", json!({}))
            .unwrap();
    }
    replace_surface_template_placeholders(&mut template);
    let manifest: SurfaceManifest = serde_json::from_value(template).expect(
        "multi-step template must decode as a SurfaceManifest after placeholder replacement",
    );
    manifest
        .validate_for(&spec, &hash)
        .expect("multi-step template must pass full SurfaceManifest validation after only CodeFile replacements");
    manifest
        .validate_setup_for_store(&store)
        .expect("multi-step template must pass store-backed surface validation");
}

fn replace_surface_template_placeholders(template: &mut serde_json::Value) {
    template["surface"]["codefile"] = json!("src/cli.rs");
    template["surface"]["locator"] = json!("run_publish");
    let Some(operations) = template["surface"]["operations"].as_array_mut() else {
        return;
    };
    for operation in operations {
        let Some(exercises) = operation["exercises"].as_array_mut() else {
            continue;
        };
        for exercise in exercises {
            exercise["codefile"] = json!("src/handler.rs");
            exercise["locator"] = json!("post_blueprint");
        }
    }
}

#[test]
fn validation_show_json_retains_operation_exercise_provenance() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    settle_with_assertions(&fixture);
    assert_eq!(witness(&fixture.store, &fixture.validation_id).grade, "S3");
    let graph = fixture.tmp.path().to_path_buf();
    let validation_id = fixture.validation_id.clone();
    drop(fixture.store);
    let output = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(&graph)
        .args(["--json", "validation", "show", &validation_id])
        .output()
        .expect("run loom validation show");
    assert!(
        output.status.success(),
        "validation show failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let evidence = &parsed["strength"]["call_evidence"];
    assert_eq!(evidence["source"], "journey_operation_exercise");
    assert_eq!(evidence["file"], "src/handler.rs");
    assert_eq!(evidence["operation_id"], "publish-http-operation");
    assert_eq!(evidence["exercise_id"], "publish-handler");
    assert_eq!(evidence["observed_by"], "publish-http-operation");
    assert_eq!(evidence["entry_symbol"], "post_blueprint");
    assert_eq!(evidence["grounded_symbol"], "post_blueprint");
    assert_eq!(evidence["s3_eligible"], true);
}

#[test]
fn manual_edge_call_verdict_and_remove_mutations_remain_rejected() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    let graph = fixture.tmp.path().to_path_buf();
    let validation = fixture.validation_id.clone();
    let surface = fixture
        .store
        .edges_with(Some(EdgeKind::Calls), Some(&fixture.validation_id), None)
        .unwrap()[0]
        .to_id
        .clone();
    let downstream = downstream_edge(&fixture).id.clone();
    drop(fixture.store);

    let attempts: Vec<loom::cli::Command> = vec![
        loom::cli::Command::Edge {
            cmd: loom::cli::EdgeCmd::Exercises {
                validation: validation.clone(),
                codefile: "src/handler.rs".into(),
                locator: Some("post_blueprint".into()),
            },
        },
        loom::cli::Command::Edge {
            cmd: loom::cli::EdgeCmd::Call {
                validation: validation.clone(),
                surface,
            },
        },
        loom::cli::Command::Edge {
            cmd: loom::cli::EdgeCmd::Remove {
                edge_id: downstream,
                reason: Some("test".into()),
            },
        },
        loom::cli::Command::Validation {
            cmd: loom::cli::ValidationCmd::Verdict {
                key: validation.clone(),
                outcome: "passed".into(),
                evidence: String::new(),
                reason: String::new(),
            },
        },
    ];
    for attempt in attempts {
        let err = loom::commands::run(loom::cli::Cli {
            graph: Some(graph.clone()),
            json: true,
            command: Some(attempt),
        })
        .expect_err("manual mutation of compiler-owned topology must be refused");
        assert!(
            format!("{err:#}").contains("compiler-owned"),
            "expected a compiler-owned refusal, got: {err:#}"
        );
    }
}

#[test]
fn cli_compile_creates_public_and_downstream_and_removes_obsolete() {
    let root = Tmp::new();
    loom_bin(root.path(), &["init", root.path().to_str().unwrap()]);
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/cli.rs"), "pub fn run_publish() {}\n").unwrap();
    std::fs::write(
        root.path().join("src/handler.rs"),
        "pub fn post_blueprint() {}\n",
    )
    .unwrap();
    loom_bin(root.path(), &["codefile", "add", "src/cli.rs"]);
    loom_bin(root.path(), &["codefile", "add", "src/handler.rs"]);

    let authored: JourneySpec = serde_json::from_value(json!({
        "schema": JOURNEY_SCHEMA,
        "id": "publish.flow",
        "name": "Publish",
        "actor": "operator",
        "goal": "Publish a blueprint",
        "inputs": {},
        "preconditions": [],
        "steps": [{"id":"publish","name":"Publish","action":"publishes","expects":[],"produces":{}}],
        "profiles":{"proof":{"inputs":{},"workspace":{}}}
    }))
    .unwrap();
    let artifact = root.path().join("publish.journey.json");
    std::fs::write(&artifact, serde_json::to_vec_pretty(&authored).unwrap()).unwrap();
    loom_bin(root.path(), &["journey", "add", artifact.to_str().unwrap()]);
    let derivation = root.path().join("derive.json");
    std::fs::write(
        &derivation,
        serde_json::to_vec_pretty(&json!({
            "schema": "loom.journey-derivation/v1",
            "journey_id": "publish.flow",
            "journey_hash": authored.semantic_hash().unwrap(),
            "proposal_id": "publish-projection",
            "proposal_rationale": "one technical publish behavior",
            "intents": [{
                "id": "publish-intent",
                "operation": "create",
                "name": "publish stores a blueprint",
                "criterion": "a publish stores a blueprint result",
                "level": "feature",
                "visibility": "internal",
                "rationale": "authored publish requires one observable store",
                "step_ids": ["publish"]
            }],
            "relationships": []
        }))
        .unwrap(),
    )
    .unwrap();
    loom_bin(
        root.path(),
        &[
            "journey",
            "derive-accept",
            "publish.flow",
            "--manifest",
            derivation.to_str().unwrap(),
            "--human-decision",
            "The publish derivation exactly represents the approved behavior.",
        ],
    );
    {
        let store = Store::open(root.path()).unwrap();
        let intent = store
            .list_nodes(Some(NodeType::Intent), usize::MAX)
            .unwrap()
            .into_iter()
            .find(|node| node.name.contains("publish"))
            .unwrap();
        let handler = store
            .list_nodes(Some(NodeType::CodeFile), usize::MAX)
            .unwrap()
            .into_iter()
            .find(|node| node.name == "src/handler.rs")
            .unwrap();
        let realizes = store
            .ensure_edge(EdgeKind::Implements, &intent.id, &handler.id)
            .unwrap();
        store
            .set_facet(
                &realizes.id,
                TargetKind::Edge,
                "locator",
                "post_blueprint",
                TruthClass::Asserted,
            )
            .unwrap();
        store.set_node_status(&intent.id, "implemented").unwrap();
    }

    let surface = root.path().join("surface.json");
    std::fs::write(
        &surface,
        serde_json::to_vec_pretty(&json!({
            "schema": SURFACE_SCHEMA,
            "journey_id": "publish.flow",
            "journey_hash": authored.semantic_hash().unwrap(),
            "surface": {
                "id": "publish-cli",
                "title": "Publish CLI",
                "identity": "publish",
                "codefile": "src/cli.rs",
                "locator": "run_publish",
                "operations": [{
                    "id": "publish-http-operation",
                    "summary": "Publish",
                    "argv": ["python3", "-c", "import json; print(json.dumps({'ok': True}))"],
                    "read_only": true,
                    "output": {
                        "format": "json",
                        "assertions": [{
                            "id": "publish-http-operation",
                            "pointer": "/ok",
                            "type": "boolean",
                            "equals": true
                        }]
                    },
                    "exercises": [{
                        "id": "publish-handler",
                        "codefile": "src/handler.rs",
                        "locator": "post_blueprint",
                        "observed_by": "publish-http-operation"
                    }]
                }]
            },
            "bindings": [{"step_id":"publish","operation_id":"publish-http-operation"}]
        }))
        .unwrap(),
    )
    .unwrap();
    loom_bin(
        root.path(),
        &[
            "journey",
            "surface-accept",
            "publish.flow",
            "--manifest",
            surface.to_str().unwrap(),
        ],
    );
    loom_bin(
        root.path(),
        &[
            "--json",
            "journey",
            "compile",
            "publish.flow",
            "--profile",
            "proof",
        ],
    );

    {
        let store = Store::open(root.path()).unwrap();
        let validation = store
            .list_nodes(Some(NodeType::Validation), usize::MAX)
            .unwrap()
            .into_iter()
            .find(|node| node.name == "journey:publish.flow:proof")
            .unwrap();
        let exercises = store
            .edges_with(Some(EdgeKind::Exercises), Some(&validation.id), None)
            .unwrap();
        assert_eq!(exercises.len(), 2, "{exercises:?}");
        let mut saw_downstream = false;
        for edge in &exercises {
            if let Some(raw) = store
                .get_facet(&edge.id, TargetKind::Edge, "journey_operation_exercises")
                .unwrap()
            {
                let facet: Vec<JourneyOperationExerciseFacet> = serde_json::from_str(&raw).unwrap();
                assert_eq!(facet.len(), 1);
                assert_eq!(facet[0].exercise_id, "publish-handler");
                saw_downstream = true;
            }
        }
        assert!(saw_downstream);
    }

    // Re-accept without the exercise and recompile — obsolete downstream edge gone.
    std::fs::write(
        &surface,
        serde_json::to_vec_pretty(&json!({
            "schema": SURFACE_SCHEMA,
            "journey_id": "publish.flow",
            "journey_hash": authored.semantic_hash().unwrap(),
            "surface": {
                "id": "publish-cli-v2",
                "title": "Publish CLI",
                "identity": "publish-v2",
                "codefile": "src/cli.rs",
                "locator": "run_publish",
                "operations": [{
                    "id": "publish-http-operation",
                    "summary": "Publish",
                    "argv": ["python3", "-c", "import json; print(json.dumps({'ok': True}))"],
                    "read_only": true,
                    "output": {
                        "format": "json",
                        "assertions": [{
                            "id": "publish-http-operation",
                            "pointer": "/ok",
                            "type": "boolean",
                            "equals": true
                        }]
                    }
                }]
            },
            "bindings": [{"step_id":"publish","operation_id":"publish-http-operation"}]
        }))
        .unwrap(),
    )
    .unwrap();
    loom_bin(
        root.path(),
        &[
            "journey",
            "surface-accept",
            "publish.flow",
            "--manifest",
            surface.to_str().unwrap(),
        ],
    );
    loom_bin(
        root.path(),
        &[
            "--json",
            "journey",
            "compile",
            "publish.flow",
            "--profile",
            "proof",
        ],
    );
    {
        let store = Store::open(root.path()).unwrap();
        let validation = store
            .list_nodes(Some(NodeType::Validation), usize::MAX)
            .unwrap()
            .into_iter()
            .find(|node| node.name == "journey:publish.flow:proof")
            .unwrap();
        let exercises = store
            .edges_with(Some(EdgeKind::Exercises), Some(&validation.id), None)
            .unwrap();
        assert_eq!(exercises.len(), 1);
        for edge in exercises {
            assert!(store
                .get_facet(&edge.id, TargetKind::Edge, "journey_operation_exercises")
                .unwrap()
                .is_none());
        }
    }
}

#[test]
fn run_with_node_id_exercise_reference_covers_canonical_path_and_edit_invalidates() {
    let root = Tmp::new();
    loom_bin(root.path(), &["init", root.path().to_str().unwrap()]);
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/cli.rs"), "pub fn run_publish() {}\n").unwrap();
    std::fs::write(
        root.path().join("src/handler.rs"),
        "pub fn post_blueprint() {}\n",
    )
    .unwrap();
    loom_bin(root.path(), &["codefile", "add", "src/cli.rs"]);
    loom_bin(root.path(), &["codefile", "add", "src/handler.rs"]);

    let authored: JourneySpec = serde_json::from_value(json!({
        "schema": JOURNEY_SCHEMA,
        "id": "publish.flow",
        "name": "Publish",
        "actor": "operator",
        "goal": "Publish a blueprint",
        "inputs": {},
        "preconditions": [],
        "steps": [{"id":"publish","name":"Publish","action":"publishes","expects":[],"produces":{}}],
        "profiles":{"proof":{"inputs":{},"workspace":{}}}
    }))
    .unwrap();
    let artifact = root.path().join("publish.journey.json");
    std::fs::write(&artifact, serde_json::to_vec_pretty(&authored).unwrap()).unwrap();
    loom_bin(root.path(), &["journey", "add", artifact.to_str().unwrap()]);
    let derivation = root.path().join("derive.json");
    std::fs::write(
        &derivation,
        serde_json::to_vec_pretty(&json!({
            "schema": "loom.journey-derivation/v1",
            "journey_id": "publish.flow",
            "journey_hash": authored.semantic_hash().unwrap(),
            "proposal_id": "publish-projection",
            "proposal_rationale": "one technical publish behavior",
            "intents": [{
                "id": "publish-intent",
                "operation": "create",
                "name": "publish stores a blueprint",
                "criterion": "a publish stores a blueprint result",
                "level": "feature",
                "visibility": "internal",
                "rationale": "authored publish requires one observable store",
                "step_ids": ["publish"]
            }],
            "relationships": []
        }))
        .unwrap(),
    )
    .unwrap();
    loom_bin(
        root.path(),
        &[
            "journey",
            "derive-accept",
            "publish.flow",
            "--manifest",
            derivation.to_str().unwrap(),
            "--human-decision",
            "The publish derivation exactly represents the approved behavior.",
        ],
    );
    {
        let store = Store::open(root.path()).unwrap();
        let intent = store
            .list_nodes(Some(NodeType::Intent), usize::MAX)
            .unwrap()
            .into_iter()
            .find(|node| node.name.contains("publish"))
            .unwrap();
        let handler = store
            .resolve_node("src/handler.rs", Some(NodeType::CodeFile))
            .unwrap();
        let realizes = store
            .ensure_edge(EdgeKind::Implements, &intent.id, &handler.id)
            .unwrap();
        store
            .set_facet(
                &realizes.id,
                TargetKind::Edge,
                "locator",
                "post_blueprint",
                TruthClass::Asserted,
            )
            .unwrap();
        store.set_node_status(&intent.id, "implemented").unwrap();
    }

    // Reference the exercise CodeFile by its NODE ID — a supported lookup key.
    let handler_id = {
        let store = Store::open(root.path()).unwrap();
        store
            .resolve_node("src/handler.rs", Some(NodeType::CodeFile))
            .unwrap()
            .id
    };
    let surface = root.path().join("surface.json");
    std::fs::write(
        &surface,
        serde_json::to_vec_pretty(&json!({
            "schema": SURFACE_SCHEMA,
            "journey_id": "publish.flow",
            "journey_hash": authored.semantic_hash().unwrap(),
            "surface": {
                "id": "publish-cli",
                "title": "Publish CLI",
                "identity": "publish",
                "codefile": "src/cli.rs",
                "locator": "run_publish",
                "operations": [{
                    "id": "publish-http-operation",
                    "summary": "Publish",
                    "argv": ["python3", "-c", "import json; print(json.dumps({'ok': True}))"],
                    "read_only": true,
                    "output": {
                        "format": "json",
                        "assertions": [{
                            "id": "publish-http-operation",
                            "pointer": "/ok",
                            "type": "boolean",
                            "equals": true
                        }]
                    },
                    "exercises": [{
                        "id": "publish-handler",
                        "codefile": handler_id,
                        "locator": "post_blueprint",
                        "observed_by": "publish-http-operation"
                    }]
                }]
            },
            "bindings": [{"step_id":"publish","operation_id":"publish-http-operation"}]
        }))
        .unwrap(),
    )
    .unwrap();
    loom_bin(
        root.path(),
        &[
            "journey",
            "surface-accept",
            "publish.flow",
            "--manifest",
            surface.to_str().unwrap(),
        ],
    );
    loom_bin(
        root.path(),
        &[
            "--json",
            "journey",
            "compile",
            "publish.flow",
            "--profile",
            "proof",
        ],
    );
    loom_bin(
        root.path(),
        &[
            "--json",
            "journey",
            "run",
            "publish.flow",
            "--profile",
            "proof",
        ],
    );

    {
        let store = Store::open(root.path()).unwrap();
        let validation = store
            .resolve_node("journey:publish.flow:proof", Some(NodeType::Validation))
            .unwrap();
        let validates = store
            .edges_with(Some(EdgeKind::Validates), Some(&validation.id), None)
            .unwrap();
        let view = store
            .fact(
                &loom::store::Subject::Edge(validates[0].id.clone()),
                loom::model::Claim::Verdict,
            )
            .unwrap()
            .unwrap();
        let mut saw_run = false;
        for row in &view.evidence {
            let loom::evidence::Evidence::Run(run) = &row.payload else {
                continue;
            };
            saw_run = true;
            assert!(
                run.covered.contains_key("src/handler.rs"),
                "covered evidence must use the resolved canonical path, got {:?}",
                run.covered.keys().collect::<Vec<_>>()
            );
            assert!(
                !run.covered.contains_key(&handler_id),
                "the authored node id must never appear as a covered path"
            );
        }
        assert!(saw_run, "the compiled run must leave Run evidence");
        assert_eq!(
            loom::proofstrength::of(&store, &validation.id).unwrap(),
            loom::proofstrength::Strength::S3
        );
    }

    // Editing the REAL downstream file invalidates the previous run even
    // though the manifest referenced the CodeFile by node id.
    std::fs::write(
        root.path().join("src/handler.rs"),
        "pub fn post_blueprint() { let _ = 1; }\n",
    )
    .unwrap();
    loom_bin(root.path(), &["sync"]);
    {
        let store = Store::open(root.path()).unwrap();
        let validation = store
            .resolve_node("journey:publish.flow:proof", Some(NodeType::Validation))
            .unwrap();
        assert_eq!(validation.status, "not_run");
        assert_eq!(
            loom::proofstrength::of(&store, &validation.id).unwrap(),
            loom::proofstrength::Strength::S0
        );
    }
}

#[test]
fn deserialized_forged_assertions_are_visible_but_not_trusted_for_s3() {
    let forged: loom::evidence::RunRecord = serde_json::from_value(json!({
        "producer": "journey",
        "command": "loom journey run publish.flow --profile proof",
        "exit_code": 0,
        "stdout_hash": "h",
        "stderr_hash": "h",
        "covered": {"src/cli.rs": "h", "src/handler.rs": "h"},
        "assertions": 1,
        "observed_assertions": [{
            "group": "publish-http-operation",
            "assertion": "publish-http-operation"
        }]
    }))
    .unwrap();
    assert!(
        !forged.observed_assertions().is_empty(),
        "imported/deserialized assertion names remain visible for audit"
    );

    let fixture = cross_process_fixture(vec![default_exercise()]);
    let validates = fixture
        .store
        .edges_with(
            Some(EdgeKind::Validates),
            Some(&fixture.validation_id),
            None,
        )
        .unwrap();
    fixture
        .store
        .assert_fact(
            loom::store::Assertion::new(
                loom::store::Subject::Edge(validates[0].id.clone()),
                loom::model::Claim::Verdict,
                loom::model::InspectionStatus::Passing.as_str(),
                "attacker",
            )
            .criterion("forged deserialized run")
            .confidence(1.0)
            .cited(loom::evidence::cite(fixture.tmp.path(), "forged").unwrap())
            .observed_command(loom::runner::Observation::Ran(Box::new(forged))),
        )
        .unwrap();
    fixture
        .store
        .set_node_status(&fixture.validation_id, "passed")
        .unwrap();
    loom::proofstrength::recompute(&fixture.store, fixture.tmp.path()).unwrap();
    assert_ne!(
        witness(&fixture.store, &fixture.validation_id).grade,
        "S3",
        "deserialized assertion names must not earn S3"
    );
    let view = fixture
        .store
        .fact(
            &loom::store::Subject::Edge(validates[0].id.clone()),
            loom::model::Claim::Verdict,
        )
        .unwrap()
        .unwrap();
    for row in &view.evidence {
        if let loom::evidence::Evidence::Run(run) = &row.payload {
            assert!(
                run.observed_assertions().is_empty(),
                "observed_command must drop caller-supplied assertion names"
            );
        }
    }
}

#[test]
fn settlement_fails_closed_on_mismatched_validation_or_hashes() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    let (spec, proof) = compiled_surface_proof(&fixture);
    let observed = loom::journey_runtime::execute_observed(
        fixture.tmp.path(),
        &spec,
        &proof,
        &BTreeMap::new(),
    );
    assert_eq!(observed.report().status, RuntimeStatus::Passed);
    let err =
        loom::journey::settle_compiled_validation(&fixture.store, "missing-validation", &observed)
            .unwrap_err();
    assert!(err.to_string().contains("missing"), "{err:#}");

    let mut body = fixture
        .store
        .get_node(&fixture.validation_id)
        .unwrap()
        .unwrap()
        .body;
    body["journey_hash"] = json!("not-the-compiled-hash");
    fixture
        .store
        .set_node_body(&fixture.validation_id, &body)
        .unwrap();
    let err = loom::journey::settle_compiled_validation(
        &fixture.store,
        &fixture.validation_id,
        &observed,
    )
    .unwrap_err();
    assert!(err.to_string().contains("does not match"), "{err:#}");

    body["journey_hash"] = json!(observed.report().journey_hash);
    body["compiler_version"] = json!("4");
    fixture
        .store
        .set_node_body(&fixture.validation_id, &body)
        .unwrap();
    let err = loom::journey::settle_compiled_validation(
        &fixture.store,
        &fixture.validation_id,
        &observed,
    )
    .unwrap_err();
    assert!(err.to_string().contains("does not match"), "{err:#}");
}

#[test]
fn caller_authored_compiled_proof_cannot_settle_trusted_assertions() {
    // Supported public route: deserialize a compiled proof, keep identity
    // hashes, change argv, execute_observed, then settle. Settlement must
    // refuse because the proof is not the canonical accepted-surface compile.
    let fixture = cross_process_fixture(vec![default_exercise()]);
    let (spec, proof) = compiled_surface_proof(&fixture);
    let mut tampered: loom::journey_runtime::CompiledJourneyProof =
        serde_json::from_value(serde_json::to_value(&proof).unwrap()).unwrap();
    tampered.steps[0].argv = vec![
        "python3".into(),
        "-c".into(),
        "import json; print(json.dumps({'ok': True, 'forged': True}))".into(),
    ];
    assert_ne!(
        loom::journey_runtime::canonical_bytes(&proof).unwrap(),
        loom::journey_runtime::canonical_bytes(&tampered).unwrap(),
        "tampered argv must change canonical proof bytes"
    );
    assert_eq!(tampered.journey_id, proof.journey_id);
    assert_eq!(tampered.journey_hash, proof.journey_hash);
    assert_eq!(tampered.surface_hash, proof.surface_hash);
    assert_eq!(tampered.compiler_version, proof.compiler_version);
    assert_eq!(tampered.profile, proof.profile);

    let observed = loom::journey_runtime::execute_observed(
        fixture.tmp.path(),
        &spec,
        &tampered,
        &BTreeMap::new(),
    );
    assert_eq!(
        observed.report().status,
        RuntimeStatus::Passed,
        "tampered proof may still execute; settlement is the trust gate: {:#?}",
        observed.report()
    );
    let err = loom::journey::settle_compiled_validation(
        &fixture.store,
        &fixture.validation_id,
        &observed,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("canonical accepted-surface proof"),
        "{err:#}"
    );
    loom::proofstrength::recompute(&fixture.store, fixture.tmp.path()).unwrap();
    assert_ne!(
        witness(&fixture.store, &fixture.validation_id).grade,
        "S3",
        "caller-authored compiled proof must not earn S3"
    );
}

#[test]
fn settlement_covers_canonical_projection_files() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    settle_compiled(&fixture);
    let journey = fixture
        .store
        .get_node(&fixture.journey_id)
        .unwrap()
        .unwrap();
    let expected = loom::journey_exercises::expected_projection(&fixture.store, &journey)
        .unwrap()
        .covered_files();
    let validates = fixture
        .store
        .edges_with(
            Some(EdgeKind::Validates),
            Some(&fixture.validation_id),
            None,
        )
        .unwrap();
    let view = fixture
        .store
        .fact(
            &loom::store::Subject::Edge(validates[0].id.clone()),
            loom::model::Claim::Verdict,
        )
        .unwrap()
        .unwrap();
    let mut covered = None;
    for row in &view.evidence {
        if let loom::evidence::Evidence::Run(run) = &row.payload {
            covered = Some(run.covered.keys().cloned().collect::<Vec<_>>());
        }
    }
    let mut covered = covered.expect("settled run");
    covered.sort();
    assert_eq!(covered, expected);
}

#[test]
fn imported_journey_assertion_names_remain_visible_without_s3() {
    let fixture = cross_process_fixture(vec![default_exercise()]);
    settle_with_assertions(&fixture);
    assert_eq!(witness(&fixture.store, &fixture.validation_id).grade, "S3");
    let export = loom::travel::export_to_file(&fixture.store).unwrap();
    let exported = std::fs::read_to_string(&export).unwrap();
    assert!(
        exported.contains("observed_assertions"),
        "export must preserve assertion names for audit"
    );
    assert!(
        exported.contains("publish-http-operation"),
        "export must name the observed assertion"
    );

    let destination = Tmp::new();
    std::fs::create_dir_all(destination.path().join("src")).unwrap();
    std::fs::write(
        destination.path().join("src/cli.rs"),
        "pub fn run_publish() -> &'static str { \"ok\" }\n",
    )
    .unwrap();
    std::fs::write(
        destination.path().join("src/handler.rs"),
        "pub fn post_blueprint() -> &'static str { \"stored\" }\n",
    )
    .unwrap();
    let mut snapshot = loom::travel::read_export(&export).unwrap().into_snapshot();
    loom::travel::quarantine_imported_execution(&mut snapshot).unwrap();
    let mut imported = Store::init(destination.path(), Some("imported-journey"), false).unwrap();
    imported.restore(&snapshot).unwrap();
    loom::proofstrength::recompute(&imported, destination.path()).unwrap();
    let validation = imported
        .resolve_node("journey:publish.flow:proof", Some(NodeType::Validation))
        .unwrap();
    assert_ne!(
        loom::proofstrength::of(&imported, &validation.id).unwrap(),
        loom::proofstrength::Strength::S3,
        "imported execution must not retain S3"
    );
    let mut saw_audit = false;
    for fact in imported.all_facts().unwrap() {
        let view = imported.fact_by_id(&fact.id).unwrap().unwrap();
        for row in &view.evidence {
            match &row.payload {
                loom::evidence::Evidence::Claim { text }
                    if text.contains("observed_assertions")
                        && text.contains("publish-http-operation") =>
                {
                    saw_audit = true;
                }
                loom::evidence::Evidence::Run(run)
                    if run
                        .observed_assertions()
                        .iter()
                        .any(|a| a.assertion == "publish-http-operation") =>
                {
                    panic!("imported Journey run must not remain executable Run evidence");
                }
                _ => {}
            }
        }
    }
    assert!(
        saw_audit,
        "imported assertion names must remain visible as non-authoritative audit text"
    );
}

#[test]
fn generic_command_validation_cannot_mint_journey_assertion_provenance() {
    let tmp = Tmp::new();
    tmp.write("src/lib.rs", "pub fn f() {}\n");
    let store = Store::init(tmp.path(), Some("generic-proof"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "generic",
            "generic",
            "implemented",
            json!({}),
        )
        .unwrap();
    let validation = store
        .add_node(
            NodeType::Validation,
            "unit",
            "generic command",
            "not_run",
            json!({"type":"test","command":"python3 -c \"print('ok')\""}),
        )
        .unwrap();
    store
        .ensure_edge(EdgeKind::Validates, &validation.id, &intent.id)
        .unwrap();
    loom::commands::observe_validation(&store, &validation).unwrap();
    let edges = store
        .edges_with(Some(EdgeKind::Validates), Some(&validation.id), None)
        .unwrap();
    let view = store
        .fact(
            &loom::store::Subject::Edge(edges[0].id.clone()),
            loom::model::Claim::Verdict,
        )
        .unwrap()
        .unwrap();
    for row in &view.evidence {
        if let loom::evidence::Evidence::Run(run) = &row.payload {
            assert!(
                run.observed_assertions().is_empty(),
                "generic command runs must not carry Journey assertion provenance"
            );
            assert_ne!(run.producer, loom::model::RunProducer::Journey);
        }
    }
}

fn loom_bin(root: &Path, args: &[&str]) {
    let bin = env!("CARGO_BIN_EXE_loom");
    let output = Command::new(bin)
        .current_dir(root)
        .args(args)
        .output()
        .expect("run loom");
    assert!(
        output.status.success(),
        "loom {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
