//! Ring 55 — compiler-owned operation exercises bridge cross-process Journey
//! proof entries without relaxing surface ownership or S3.

use loom::journey::{
    CliOperation, InterfaceSurfaceDefinition, JourneyOperationExerciseFacet, JourneySpec,
    OperationBinding, OperationExercise, OperationOutput, OutputAssertion, OutputFormat,
    SurfaceManifest, ValueType, JOURNEY_COMPILER_VERSION, JOURNEY_SCHEMA, SURFACE_SCHEMA,
};
use loom::journey_runtime::{PassedAssertion, RuntimeReport, RuntimeStatus};
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

fn cross_process_fixture(with_exercise: bool) -> CrossProcessFixture {
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
    if with_exercise {
        operation.exercises = vec![OperationExercise {
            id: "publish-handler".into(),
            codefile: "src/handler.rs".into(),
            locator: "post_blueprint".into(),
            observed_by: "publish-http-operation".into(),
        }];
    }
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

    if with_exercise {
        let downstream = store
            .ensure_edge(EdgeKind::Exercises, &validation.id, &handler.id)
            .unwrap();
        store
            .set_facet(
                &downstream.id,
                TargetKind::Edge,
                "locator",
                "post_blueprint",
                TruthClass::Asserted,
            )
            .unwrap();
        let facet = vec![JourneyOperationExerciseFacet {
            operation_id: "publish-http-operation".into(),
            exercise_id: "publish-handler".into(),
            observed_by: "publish-http-operation".into(),
            locator: "post_blueprint".into(),
        }];
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

fn settle_with_assertions(fixture: &CrossProcessFixture, passed: Vec<PassedAssertion>) {
    let report = RuntimeReport {
        journey_id: "publish.flow".into(),
        profile: "proof".into(),
        journey_hash: fixture
            .store
            .get_node(&fixture.journey_id)
            .unwrap()
            .unwrap()
            .body["semantic_hash"]
            .as_str()
            .unwrap()
            .into(),
        surface_hash: fixture
            .store
            .get_node(&fixture.validation_id)
            .unwrap()
            .unwrap()
            .body["surface_hash"]
            .as_str()
            .unwrap()
            .into(),
        status: RuntimeStatus::Passed,
        assertions_passed: passed.len().max(1),
        assertions_failed: 0,
        detail: None,
        setup: Vec::new(),
        file_transitions: Vec::new(),
        steps: Vec::new(),
        captures: BTreeMap::new(),
        passed_assertions: passed,
    };
    loom::journey::settle_compiled_validation(
        &fixture.store,
        &fixture.validation_id,
        &report,
        &["src/cli.rs".into(), "src/handler.rs".into()],
    )
    .unwrap();
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

fn witness(store: &Store, validation_id: &str) -> StrengthWitness {
    let raw = store
        .get_facet(validation_id, TargetKind::Node, "proof_strength")
        .unwrap()
        .expect("proof_strength facet");
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn compile_shaped_topology_keeps_public_and_downstream_entries() {
    let fixture = cross_process_fixture(true);
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
    let fixture = cross_process_fixture(true);
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
    let facet = vec![
        JourneyOperationExerciseFacet {
            operation_id: "publish-http-operation".into(),
            exercise_id: "publish-handler".into(),
            observed_by: "publish-http-operation".into(),
            locator: "post_blueprint".into(),
        },
        JourneyOperationExerciseFacet {
            operation_id: "publish-http-operation".into(),
            exercise_id: "other-handler".into(),
            observed_by: "publish-http-operation".into(),
            locator: "other_entry".into(),
        },
    ];
    fixture
        .store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "locator",
            "other_entry;post_blueprint",
            TruthClass::Asserted,
        )
        .unwrap();
    fixture
        .store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_operation_exercises",
            &serde_json::to_string(&facet).unwrap(),
            TruthClass::Asserted,
        )
        .unwrap();
    settle_with_assertions(
        &fixture,
        vec![PassedAssertion {
            operation_id: "publish-http-operation".into(),
            assertion_id: "publish-http-operation".into(),
        }],
    );
    let w = witness(&fixture.store, &fixture.validation_id);
    assert_eq!(w.grade, "S3");
    let evidence = w.call_evidence.expect("call evidence");
    assert_eq!(evidence.source, "journey_operation_exercise");
    assert_eq!(evidence.file, "src/handler.rs");
    assert!(evidence.operation_id.is_some());
    assert!(evidence.exercise_id.is_some());
    assert_eq!(
        evidence.observed_by.as_deref(),
        Some("publish-http-operation")
    );
}

#[test]
fn recompile_shaped_cleanup_removes_obsolete_downstream_entry() {
    let fixture = cross_process_fixture(true);
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
    let fixture = cross_process_fixture(true);
    settle_with_assertions(
        &fixture,
        vec![PassedAssertion {
            operation_id: "publish-http-operation".into(),
            assertion_id: "publish-http-operation".into(),
        }],
    );
    let w = witness(&fixture.store, &fixture.validation_id);
    assert_eq!(w.grade, "S3", "{w:?}");
    let evidence = w.call_evidence.expect("call evidence");
    assert_eq!(evidence.source, "journey_operation_exercise");
    assert_eq!(evidence.entry_symbol.as_deref(), Some("post_blueprint"));
    assert_eq!(evidence.grounded_symbol.as_deref(), Some("post_blueprint"));
    assert!(evidence.s3_eligible);
}

#[test]
fn remains_s2_when_assertion_missing_handler_unreached_or_only_declared() {
    // Only declaration / no observed assertion ids → public adapter cannot reach handler.
    let declared = cross_process_fixture(true);
    settle_with_assertions(&declared, Vec::new());
    let declared_w = witness(&declared.store, &declared.validation_id);
    assert_eq!(declared_w.grade, "S2", "{declared_w:?}");
    assert!(
        declared_w.next.contains("observed_by assertion")
            || declared_w.next.contains("operation exercise"),
        "{}",
        declared_w.next
    );
    assert!(!declared_w.next.contains("edge exercises"));

    // Assertion present but entry does not reach realizing symbol.
    let missed = cross_process_fixture(true);
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
    settle_with_assertions(
        &missed,
        vec![PassedAssertion {
            operation_id: "publish-http-operation".into(),
            assertion_id: "publish-http-operation".into(),
        }],
    );
    let missed_w = witness(&missed.store, &missed.validation_id);
    assert_eq!(missed_w.grade, "S2", "{missed_w:?}");
    assert!(
        missed_w.next.contains("does not reach") || missed_w.next.contains("realizing"),
        "{}",
        missed_w.next
    );

    // No exercise declared at all.
    let none = cross_process_fixture(false);
    settle_with_assertions(
        &none,
        vec![PassedAssertion {
            operation_id: "publish-http-operation".into(),
            assertion_id: "publish-http-operation".into(),
        }],
    );
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
    let fixture = cross_process_fixture(true);
    settle_with_assertions(
        &fixture,
        vec![PassedAssertion {
            operation_id: "publish-http-operation".into(),
            assertion_id: "publish-http-operation".into(),
        }],
    );
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
    let fixture = cross_process_fixture(true);
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
    let fixture = cross_process_fixture(true);
    settle_with_assertions(
        &fixture,
        vec![PassedAssertion {
            operation_id: "publish-http-operation".into(),
            assertion_id: "publish-http-operation".into(),
        }],
    );
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
