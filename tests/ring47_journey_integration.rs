//! Ring 47 — semantic Journey compiler seams agree end to end.

use loom::model::{EdgeKind, InspectionStatus, Node, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use rusqlite::Connection;
mod common;
use common::Tmp;

struct CompiledFixture {
    tmp: Tmp,
    store: Store,
    journey: Node,
    validation: Node,
    proves_id: String,
    validates_id: String,
    calls_id: String,
    exercises_id: String,
    surface_exercises_id: String,
}

fn compiled_fixture() -> CompiledFixture {
    let tmp = Tmp::new();
    tmp.write(
        "journeys/flow.yaml",
        "schema: loom.journey/v1\nid: flow\nname: Flow\nactor: user\ngoal: Complete the flow\ninputs: {}\npreconditions: []\nsteps:\n  - id: act\n    name: Act\n    action: complete the flow\n    expects: []\n    produces: {}\nprofiles:\n  proof:\n    inputs: {}\n    workspace: {}\n",
    );
    tmp.write(
        "src/behavior.rs",
        "pub fn perform_behavior() -> &'static str { \"ok\" }\n",
    );
    tmp.write("src/flow_cli.rs", "pub fn flow_cli() {}\n");
    tmp.write(
        "tests/compiled_proof.rs",
        "#[test]\nfn compiled_proof() { let _ = perform_behavior(); }\n",
    );
    tmp.write("proof-observation.txt", "compiled proof observation\n");

    let spec: loom::journey::JourneySpec = serde_norway::from_str(
        &std::fs::read_to_string(tmp.path().join("journeys/flow.yaml")).unwrap(),
    )
    .unwrap();
    let journey_hash = spec.semantic_hash().unwrap();

    let store = Store::init(tmp.path(), Some("compiled Journey"), false).unwrap();
    let journey = store
        .add_node(
            NodeType::Journey,
            "flow",
            "a user completes the flow",
            "authored",
            serde_json::json!({
                "schema":"loom.journey/v1",
                "stable_id":"flow",
                "name":"Flow",
                "actor":"user",
                "goal":"a user completes the flow",
                "artifact":"journeys/flow.yaml",
                "semantic_hash": journey_hash,
                "input_ids":[],
                "preconditions":[],
                "step_ids":["act"],
                "output_ids":[],
                "profile_ids":["proof"]
            }),
        )
        .unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "the flow works",
            "the technical behavior is observable",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let behavior = store
        .add_node(
            NodeType::CodeFile,
            "src/behavior.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let realizes = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &behavior.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &realizes.id,
            TargetKind::Edge,
            "locator",
            "perform_behavior",
            TruthClass::Asserted,
        )
        .unwrap();
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
            "[\"act\"]",
            TruthClass::Asserted,
        )
        .unwrap();

    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            "flow-cli",
            "the target repository CLI",
            "active",
            serde_json::json!({
                "schema":"loom.interface-surface/v1",
                "stable_id":"flow-cli",
                "title":"Flow CLI",
                "kind":"cli",
                "identity":"flow",
                "codefile":"src/flow_cli.rs",
                "locator":"flow_cli",
                "operations":[{"id":"act-op","summary":"act","argv":["python3","-c","import json; print(json.dumps({'ok': True}))"],"arguments":[],"output":{"format":"json","assertions":[{"id":"act-ok","pointer":"/ok","type":"boolean","equals":true}]},"exercises":[{"id":"proof-entry","codefile":"tests/compiled_proof.rs","locator":"compiled_proof","observed_by":"act-ok"}]}]
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
            "[{\"operation_id\":\"act-op\",\"step_id\":\"act\"}]",
            TruthClass::Asserted,
        )
        .unwrap();
    let surface_code = store
        .add_node(
            NodeType::CodeFile,
            "src/flow_cli.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let exposes = store
        .add_edge(
            EdgeKind::Exposes,
            &surface.id,
            &surface_code.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &exposes.id,
            TargetKind::Edge,
            "locator",
            "flow_cli",
            TruthClass::Asserted,
        )
        .unwrap();
    let proof_code = store
        .add_node(
            NodeType::CodeFile,
            "tests/compiled_proof.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();

    let surface_hash = loom::journey::surface_projection_hash(&store, &journey)
        .unwrap()
        .unwrap();
    let validation = store
        .add_node(
            NodeType::Validation,
            "journey:flow:proof",
            "compiler-owned proof profile",
            "not_run",
            serde_json::json!({
                "type":"journey",
                "command":"cargo test --test compiled_proof",
                "profile":"proof",
                "journey_hash": journey_hash,
                "surface_hash":surface_hash,
                "compiler_version": loom::journey::JOURNEY_COMPILER_VERSION
            }),
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
    let validates = store
        .add_edge(
            EdgeKind::Validates,
            &validation.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let calls = store
        .add_edge(
            EdgeKind::Calls,
            &validation.id,
            &surface.id,
            TruthClass::Asserted,
        )
        .unwrap();
    // Compiler-owned Exercises topology: one public surface edge and one
    // downstream exercise edge with exact provenance facets.
    let surface_exercises = store
        .add_edge(
            EdgeKind::Exercises,
            &validation.id,
            &surface_code.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &surface_exercises.id,
            TargetKind::Edge,
            "locator",
            "flow_cli",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &surface_exercises.id,
            TargetKind::Edge,
            "surface_locator",
            "flow_cli",
            TruthClass::Asserted,
        )
        .unwrap();
    let exercises = store
        .add_edge(
            EdgeKind::Exercises,
            &validation.id,
            &proof_code.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &exercises.id,
            TargetKind::Edge,
            "locator",
            "compiled_proof",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &exercises.id,
            TargetKind::Edge,
            "journey_operation_exercises",
            &serde_json::to_string(&vec![loom::journey::JourneyOperationExerciseFacet {
                operation_id: "act-op".into(),
                exercise_id: "proof-entry".into(),
                observed_by: "act-ok".into(),
                locator: "compiled_proof".into(),
            }])
            .unwrap(),
            TruthClass::Asserted,
        )
        .unwrap();

    // Settle through the real compiler-owned execution path: compile the
    // accepted surface, run it, and mint structured assertion evidence.
    let operations: Vec<loom::journey::CliOperation> =
        serde_json::from_value(surface.body["operations"].clone()).unwrap();
    let bindings = [loom::journey::OperationBinding {
        step_id: "act".into(),
        operation_id: "act-op".into(),
    }];
    let proof =
        loom::journey_runtime::compile(&spec, &surface_hash, "proof", operations, &bindings)
            .unwrap();
    let observed =
        loom::journey_runtime::execute_observed(tmp.path(), &spec, &proof, &Default::default());
    assert_eq!(
        observed.report().status,
        loom::journey_runtime::RuntimeStatus::Passed,
        "{:#?}",
        observed.report()
    );
    loom::journey::settle_compiled_validation(&store, &validation.id, &observed).unwrap();
    // The closure edges carry inspected verdicts, exactly as sync leaves them
    // on a live compiled Journey. `stale_edge` only re-opens inspected edges,
    // so the drift tests below can assert the whole closure invalidates.
    for (edge, cited) in [
        (&calls, "src/flow_cli.rs:1"),
        (&exercises, "src/flow_cli.rs:1"),
        (&surface_exercises, "src/behavior.rs:1"),
    ] {
        store
            .record_verdict(
                &edge.id,
                InspectionStatus::Passing,
                "compiled proof topology was exercised",
                cited,
                1.0,
                "ring47",
            )
            .unwrap();
    }

    CompiledFixture {
        tmp,
        store,
        journey,
        validation,
        proves_id: proves.id,
        validates_id: validates.id,
        calls_id: calls.id,
        exercises_id: exercises.id,
        surface_exercises_id: surface_exercises.id,
    }
}

#[test]
fn only_the_hash_bound_compiler_closure_earns_journey_s3() {
    let fixture = compiled_fixture();
    let witness: loom::proofstrength::StrengthWitness = serde_json::from_str(
        &fixture
            .store
            .get_facet(&fixture.validation.id, TargetKind::Node, "proof_strength")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(witness.observed_assertions.as_deref(), Some("1"));
    assert_eq!(
        loom::proofstrength::of(&fixture.store, &fixture.validation.id).unwrap(),
        loom::proofstrength::Strength::S3
    );

    // Removing the compiler-owned provenance facet must demote to S2 — a
    // bare Exercises edge cannot earn reach for a compiler-v5 Journey proof.
    fixture
        .store
        .clear_facet(
            &fixture.exercises_id,
            TargetKind::Edge,
            "journey_operation_exercises",
        )
        .unwrap();
    loom::proofstrength::recompute(&fixture.store, fixture.tmp.path()).unwrap();
    assert_eq!(
        loom::proofstrength::of(&fixture.store, &fixture.validation.id).unwrap(),
        loom::proofstrength::Strength::S2,
        "structured assertions earn S2, but a bare Exercises file cannot earn reach"
    );
    fixture
        .store
        .set_facet(
            &fixture.exercises_id,
            TargetKind::Edge,
            "journey_operation_exercises",
            &serde_json::to_string(&vec![loom::journey::JourneyOperationExerciseFacet {
                operation_id: "act-op".into(),
                exercise_id: "proof-entry".into(),
                observed_by: "act-ok".into(),
                locator: "compiled_proof".into(),
            }])
            .unwrap(),
            TruthClass::Asserted,
        )
        .unwrap();
    loom::proofstrength::recompute(&fixture.store, fixture.tmp.path()).unwrap();
    assert_eq!(
        loom::proofstrength::of(&fixture.store, &fixture.validation.id).unwrap(),
        loom::proofstrength::Strength::S3,
        "Calls plus compiler-owned exercise provenance reach earns S3"
    );

    let raw = fixture
        .store
        .add_node(
            NodeType::Validation,
            "raw authored Journey",
            "must not be treated as compiler output",
            "passed",
            serde_json::json!({
                "type":"journey",
                "artifact":"journeys/flow.yaml",
                "command":"cargo test --test compiled_proof"
            }),
        )
        .unwrap();
    fixture
        .store
        .add_edge(
            EdgeKind::Validates,
            &raw.id,
            &fixture
                .store
                .edges_with(
                    Some(EdgeKind::Validates),
                    Some(&fixture.validation.id),
                    None,
                )
                .unwrap()[0]
                .to_id,
            TruthClass::Asserted,
        )
        .unwrap();
    loom::proofstrength::recompute(&fixture.store, fixture.tmp.path()).unwrap();
    assert_eq!(
        loom::proofstrength::of(&fixture.store, &raw.id).unwrap(),
        loom::proofstrength::Strength::S0
    );
}

#[test]
fn compiler_input_drift_stales_the_proof_closure_once() {
    let fixture = compiled_fixture();
    let surface = fixture
        .store
        .edges_with(Some(EdgeKind::Surfaces), Some(&fixture.journey.id), None)
        .unwrap()[0]
        .to_id
        .clone();
    let mut body = fixture.store.get_node(&surface).unwrap().unwrap().body;
    body["title"] = serde_json::json!("Changed Flow CLI");
    fixture.store.set_node_body(&surface, &body).unwrap();

    let first = loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    assert_eq!(
        fixture
            .store
            .get_node(&fixture.validation.id)
            .unwrap()
            .unwrap()
            .status,
        "not_run"
    );
    for edge_id in [&fixture.proves_id, &fixture.validates_id, &fixture.calls_id] {
        assert_eq!(
            fixture.store.get_edge(edge_id).unwrap().unwrap().status,
            InspectionStatus::NeedsReverification
        );
    }
    assert!(first.validations_reset >= 1);
    let second = loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    assert_eq!(second.validations_reset, 0);
}

#[test]
fn backing_code_drift_closes_the_interface_and_proof_ripple_once() {
    let fixture = compiled_fixture();
    let realizes = fixture
        .store
        .edges_with(Some(EdgeKind::Implements), None, None)
        .unwrap()[0]
        .id
        .clone();
    let exposes = fixture
        .store
        .edges_with(Some(EdgeKind::Exposes), None, None)
        .unwrap()[0]
        .id
        .clone();
    let exercises = fixture.exercises_id.clone();
    fixture
        .store
        .record_verdict(
            &realizes,
            InspectionStatus::Passing,
            "perform_behavior realizes the independently anchored behavior",
            "src/behavior.rs:1",
            1.0,
            "ring47",
        )
        .unwrap();
    fixture
        .store
        .record_verdict(
            &exposes,
            InspectionStatus::Passing,
            "flow_cli realizes the declared interface surface",
            "src/flow_cli.rs:1",
            1.0,
            "ring47",
        )
        .unwrap();

    fixture.tmp.write(
        "src/flow_cli.rs",
        "pub fn flow_cli() { let backing_contract_changed = true; assert!(backing_contract_changed); }\n",
    );
    let first = loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();

    assert_eq!(first.files_changed, 1);
    assert_eq!(first.surfaces_affected, 1);
    assert_eq!(first.contracts_reset, 1);
    assert_eq!(first.validations_reset, 1);
    assert_eq!(first.edges_staled, 5);
    assert_eq!(
        fixture.store.get_edge(&exposes).unwrap().unwrap().status,
        InspectionStatus::NeedsReverification,
        "the changed backing file invalidates its interface grounding"
    );
    for edge_id in [
        &fixture.proves_id,
        &fixture.validates_id,
        &fixture.calls_id,
        &exercises,
    ] {
        assert_eq!(
            fixture.store.get_edge(edge_id).unwrap().unwrap().status,
            InspectionStatus::NeedsReverification,
            "the affected compiled proof/interface closure is invalidated"
        );
    }
    assert_eq!(
        fixture
            .store
            .get_edge(&fixture.surface_exercises_id)
            .unwrap()
            .unwrap()
            .status,
        InspectionStatus::Passing,
        "the public surface Exercises edge anchors on unchanged behavior code and is spared"
    );
    assert_eq!(
        fixture.store.get_edge(&realizes).unwrap().unwrap().status,
        InspectionStatus::Passing,
        "the independently anchored behavior grounding remains settled"
    );
    assert_eq!(
        fixture
            .store
            .get_node(&fixture.validation.id)
            .unwrap()
            .unwrap()
            .status,
        "not_run"
    );

    let second = loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    assert_eq!(second.files_changed, 0);
    assert_eq!(second.edges_staled, 0);
    assert_eq!(second.surfaces_affected, 0);
    assert_eq!(second.contracts_reset, 0);
    assert_eq!(second.validations_reset, 0);
}

#[test]
fn maturity_uses_the_shared_journey_gap_predicates() {
    let fixture = compiled_fixture();
    let inputs = loom::lane::LadderInputs::gather(&fixture.store).unwrap();
    assert_eq!(
        inputs.authored_journeys,
        loom::completeness::all_journey_readiness(&fixture.store)
            .unwrap()
            .into_iter()
            .filter(|journey| journey.authored)
            .count()
    );
    assert_eq!(
        inputs.derive_gaps,
        loom::completeness::journey_derive_gaps(&fixture.store)
            .unwrap()
            .len()
    );
    assert_eq!(
        inputs.surface_gaps,
        loom::completeness::journey_surface_gaps(&fixture.store)
            .unwrap()
            .len()
    );
}

#[test]
fn export_has_no_retired_baseline_sidecar() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("travel"), false).unwrap();
    let path = loom::travel::export_to_file(&store).unwrap();
    let exported = std::fs::read_to_string(path).unwrap();
    assert!(!exported.contains("baselines"));
}

#[test]
fn drive_freeze_registers_semantics_without_commands_or_baselines() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("drive"), false).unwrap();
    store
        .append_journal(
            "drive_exchange",
            "semantic-drive",
            serde_json::json!({
                "utterance":"show the current project status",
                "intent":"project status is visible",
                "command":"printf executable-secret",
                "exit":0,
                "stdout":"executable-secret"
            }),
        )
        .unwrap();
    drop(store);

    loom::commands::run(loom::cli::Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: Some(loom::cli::Command::Drive {
            cmd: Some(loom::cli::DriveCmd::Freeze {
                name: "semantic-drive".into(),
            }),
        }),
    })
    .unwrap();
    let artifact = tmp.path().join("journeys/semantic-drive.yaml");
    let text = std::fs::read_to_string(&artifact).unwrap();
    assert!(!text.contains("printf"));
    assert!(!text.contains("executable-secret"));
    let parsed = loom::journey::parse(&artifact).unwrap();
    assert_eq!(parsed.id, "semantic-drive");
    assert_eq!(parsed.steps[0].action, "show the current project status");
    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        store
            .list_nodes(Some(NodeType::Journey), usize::MAX)
            .unwrap()
            .len(),
        1
    );
    assert!(!tmp.path().join(".loom/baselines").exists());
}

#[test]
fn cli_init_uses_global_graph_for_a_fresh_root_and_refuses_old_bytes_unchanged() {
    let fresh = Tmp::new();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .arg("--graph")
        .arg(fresh.path())
        .args(["init", "--name", "fresh-v12"])
        .env_remove("LOOM_AGENT")
        .env_remove("LOOM_AGENT_PROFILE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fresh --graph init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let store = Store::open(fresh.path()).unwrap();
    assert_eq!(store.identity().unwrap().name, "fresh-v12");
    drop(store);

    let old = Tmp::new();
    drop(Store::init(old.path(), Some("old-v11"), false).unwrap());
    let database = old.path().join(loom::LOOM_DIR).join(loom::GRAPH_DB);
    let connection = Connection::open(&database).unwrap();
    connection.pragma_update(None, "user_version", 11).unwrap();
    connection
        .execute("UPDATE meta SET value='11' WHERE key='schema_version'", [])
        .unwrap();
    drop(connection);
    let before = std::fs::read(&database).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .arg("--graph")
        .arg(old.path())
        .args(["init", "--name", "must-not-write"])
        .env_remove("LOOM_AGENT")
        .env_remove("LOOM_AGENT_PROFILE")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("graph is v11"), "{error}");
    assert!(error.contains("graph is untouched"), "{error}");
    assert_eq!(
        std::fs::read(&database).unwrap(),
        before,
        "CLI refusal must not mutate old graph bytes"
    );
}

#[cfg(unix)]
#[test]
fn imported_surface_cannot_execute_until_local_surface_accept() {
    use std::os::unix::fs::PermissionsExt;

    let source = Tmp::new();
    let destination = Tmp::new();
    Store::init(source.path(), Some("surface source"), false).unwrap();
    let artifact = source.path().join("journeys/flow.json");
    std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
    let spec: loom::journey::JourneySpec = serde_json::from_value(serde_json::json!({
        "schema": loom::journey::JOURNEY_SCHEMA,
        "id": "flow",
        "name": "Flow",
        "actor": "operator",
        "goal": "complete the flow",
        "inputs": {},
        "preconditions": [],
        "steps": [{
            "id": "act",
            "name": "Act",
            "action": "complete the flow",
            "expects": ["the flow completes"],
            "produces": {}
        }],
        "profiles": {
            "proof": {
                "inputs": {},
                "workspace": {"directories": [], "files": [], "env": {}}
            }
        }
    }))
    .unwrap();
    spec.validate().unwrap();
    std::fs::write(&artifact, serde_json::to_vec_pretty(&spec).unwrap()).unwrap();
    let journey_hash = spec.semantic_hash().unwrap();

    source.write("src/flow_cli.rs", "pub fn flow_cli() {}\n");
    let sentinel = source.path().join("imported-argv-executed");
    let runner = source.path().join("danger.py");
    let quoted_sentinel = serde_json::to_string(sentinel.to_str().unwrap()).unwrap();
    std::fs::write(
        &runner,
        format!(
            "#!/usr/bin/env python3\nimport json\nfrom pathlib import Path\nPath({quoted_sentinel}).write_text('executed')\nprint(json.dumps({{'ok': True}}))\n"
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&runner).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&runner, permissions).unwrap();

    loom::commands::run(loom::cli::Cli {
        graph: Some(source.path().to_path_buf()),
        json: true,
        command: Some(loom::cli::Command::Journey {
            cmd: loom::cli::JourneyCmd::Add {
                spec: artifact.clone(),
            },
        }),
    })
    .unwrap();
    let source_store = Store::open(source.path()).unwrap();
    let journey = source_store
        .resolve_node("flow", Some(NodeType::Journey))
        .unwrap();
    let intent = source_store
        .add_node(
            NodeType::Intent,
            "flow executes",
            "the flow's implementation executes",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let derives = source_store
        .add_edge(
            EdgeKind::Derives,
            &journey.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    source_store
        .set_facet(
            &derives.id,
            TargetKind::Edge,
            "journey_hash",
            &journey_hash,
            TruthClass::Asserted,
        )
        .unwrap();
    source_store
        .set_facet(
            &derives.id,
            TargetKind::Edge,
            "step_ids",
            "[\"act\"]",
            TruthClass::Asserted,
        )
        .unwrap();
    let codefile = source_store
        .add_node(
            NodeType::CodeFile,
            "src/flow_cli.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let grounding = source_store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &codefile.id,
            TruthClass::Asserted,
        )
        .unwrap();
    source_store
        .set_facet(
            &grounding.id,
            TargetKind::Edge,
            "locator",
            "flow_cli",
            TruthClass::Asserted,
        )
        .unwrap();

    let surface_definition: loom::journey::InterfaceSurfaceDefinition =
        serde_json::from_value(serde_json::json!({
            "id": "flow-cli",
            "title": "Flow CLI",
            "identity": "flow",
            "codefile": "src/flow_cli.rs",
            "locator": "flow_cli",
            "operations": [{
                "id": "act-op",
                "summary": "Run the flow",
                "argv": [runner.to_str().unwrap()],
                "read_only": false,
                "arguments": [],
                "output": {
                    "format": "json",
                    "assertions": [{
                        "id": "flow-ok",
                        "pointer": "/ok",
                        "type": "boolean",
                        "equals": true
                    }]
                }
            }]
        }))
        .unwrap();
    surface_definition.validate().unwrap();
    let surface = source_store
        .add_node(
            NodeType::InterfaceSurface,
            "flow-cli",
            "Flow CLI",
            "declared",
            surface_definition.node_body().unwrap(),
        )
        .unwrap();
    let exposes = source_store
        .add_edge(
            EdgeKind::Exposes,
            &surface.id,
            &codefile.id,
            TruthClass::Asserted,
        )
        .unwrap();
    source_store
        .set_facet(
            &exposes.id,
            TargetKind::Edge,
            "locator",
            "flow_cli",
            TruthClass::Asserted,
        )
        .unwrap();
    let surfaces = source_store
        .add_edge(
            EdgeKind::Surfaces,
            &journey.id,
            &surface.id,
            TruthClass::Asserted,
        )
        .unwrap();
    source_store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "journey_hash",
            &journey_hash,
            TruthClass::Asserted,
        )
        .unwrap();
    source_store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "operation_bindings",
            "[{\"operation_id\":\"act-op\",\"step_id\":\"act\"}]",
            TruthClass::Asserted,
        )
        .unwrap();
    let export = loom::travel::export_to_file(&source_store).unwrap();
    drop(source_store);

    destination.write(
        "journeys/flow.json",
        &std::fs::read_to_string(&artifact).unwrap(),
    );
    destination.write("src/flow_cli.rs", "pub fn flow_cli() {}\n");
    let mut imported = loom::travel::read_export(&export).unwrap().into_snapshot();
    assert_eq!(
        loom::travel::quarantine_imported_execution(&mut imported).unwrap(),
        1
    );
    let mut destination_store =
        Store::init(destination.path(), Some("surface destination"), false).unwrap();
    destination_store.restore(&imported).unwrap();
    let imported_intent = destination_store
        .resolve_node("flow executes", Some(NodeType::Intent))
        .unwrap();
    destination_store
        .ratify_intent(&imported_intent.id, "local test authorization", "ring47")
        .unwrap();
    let imported_surface = destination_store
        .resolve_node("flow-cli", Some(NodeType::InterfaceSurface))
        .unwrap();
    assert_eq!(imported_surface.status, "quarantined");
    drop(destination_store);

    for command in [
        loom::cli::JourneyCmd::Compile {
            journey: "flow".into(),
            profile: "proof".into(),
        },
        loom::cli::JourneyCmd::Run {
            journey: "flow".into(),
            profile: "proof".into(),
        },
    ] {
        let error = loom::commands::run(loom::cli::Cli {
            graph: Some(destination.path().to_path_buf()),
            json: true,
            command: Some(loom::cli::Command::Journey { cmd: command }),
        })
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("imported") && error.contains("quarantined"),
            "{error}"
        );
        assert!(
            !error.contains(runner.to_str().unwrap()),
            "quarantine error leaked imported argv: {error}"
        );
        assert!(
            !sentinel.exists(),
            "quarantined argv executed before rejection"
        );
    }

    let manifest = destination.path().join("surface.json");
    std::fs::write(
        &manifest,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": loom::journey::SURFACE_SCHEMA,
            "journey_id": "flow",
            "journey_hash": journey_hash,
            "surface": surface_definition,
            "bindings": [{"step_id":"act", "operation_id":"act-op"}]
        }))
        .unwrap(),
    )
    .unwrap();
    loom::commands::run(loom::cli::Cli {
        graph: Some(destination.path().to_path_buf()),
        json: true,
        command: Some(loom::cli::Command::Journey {
            cmd: loom::cli::JourneyCmd::SurfaceAccept {
                journey: "flow".into(),
                manifest,
            },
        }),
    })
    .unwrap();
    loom::commands::run(loom::cli::Cli {
        graph: Some(destination.path().to_path_buf()),
        json: true,
        command: Some(loom::cli::Command::Journey {
            cmd: loom::cli::JourneyCmd::Compile {
                journey: "flow".into(),
                profile: "proof".into(),
            },
        }),
    })
    .unwrap();
    loom::commands::run(loom::cli::Cli {
        graph: Some(destination.path().to_path_buf()),
        json: true,
        command: Some(loom::cli::Command::Journey {
            cmd: loom::cli::JourneyCmd::Run {
                journey: "flow".into(),
                profile: "proof".into(),
            },
        }),
    })
    .unwrap();
    assert!(
        sentinel.exists(),
        "locally authorized surface did not execute"
    );
}
