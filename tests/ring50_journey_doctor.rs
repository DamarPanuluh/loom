//! Journey-root integrity diagnostics are fail-closed without conflating
//! ordinary readiness gaps with corrupt persisted topology.

use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::signal;
use loom::store::Store;

mod common;
use common::{s3_journey_proof, s3_journey_proof_unratified, Tmp};

fn intent(store: &Store, name: &str) -> loom::model::Node {
    store
        .add_node(
            NodeType::Intent,
            name,
            "a falsifiable user-visible behavior",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap()
}

const HUMAN_BINDINGS: &str = r#"[{"operation_id":"present-op","step_id":"present"},{"human_decision":{"operation_id":"present-op","pointer":"/work_item"},"step_id":"record"}]"#;

struct HumanSurfaceTopology {
    journey: loom::model::Node,
    intent: loom::model::Node,
    surface: loom::model::Node,
    surfaces: loom::model::Edge,
    codefile: loom::model::Node,
}

fn human_surface_topology(store: &Store, root: &std::path::Path) -> HumanSurfaceTopology {
    let spec: loom::journey::JourneySpec = serde_json::from_value(serde_json::json!({
        "schema": loom::journey::JOURNEY_SCHEMA,
        "id": "human-gate",
        "name": "Ask for a human decision",
        "actor": "operator",
        "goal": "Present and record one exact human decision",
        "inputs": {},
        "preconditions": [],
        "steps": [
            {
                "id": "present",
                "name": "Present",
                "action": "present evidence and choices",
                "expects": [],
                "produces": {}
            },
            {
                "id": "record",
                "name": "Record",
                "action": "record the exact human answer",
                "expects": [],
                "produces": {}
            }
        ],
        "profiles": {"proof":{"inputs":{},"workspace":{}}}
    }))
    .unwrap();
    std::fs::create_dir_all(root.join("journeys")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let artifact = "journeys/human-gate.yaml";
    std::fs::write(root.join(artifact), serde_norway::to_string(&spec).unwrap()).unwrap();
    std::fs::write(root.join("src/human-gate.rs"), "pub fn present() {}\n").unwrap();
    let journey_hash = spec.semantic_hash().unwrap();
    let journey = store
        .add_node(
            NodeType::Journey,
            "human-gate",
            "Present and record one exact human decision",
            "authored",
            serde_json::json!({
                "schema": loom::journey::JOURNEY_SCHEMA,
                "stable_id": "human-gate",
                "name": "Ask for a human decision",
                "actor": "operator",
                "goal": "Present and record one exact human decision",
                "artifact": artifact,
                "semantic_hash": journey_hash,
                "input_ids": [],
                "preconditions": [],
                "step_ids": ["present", "record"],
                "output_ids": [],
                "profile_ids": ["proof"]
            }),
        )
        .unwrap();
    let intent = intent(store, "human gate behavior");
    store
        .ratify_intent(&intent.id, "the fixture human wants this", "test fixture")
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
            r#"["present","record"]"#,
            TruthClass::Asserted,
        )
        .unwrap();
    let codefile = store
        .add_node(
            NodeType::CodeFile,
            "src/human-gate.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            "human-gate-cli",
            "Host-mediated human gate",
            "declared",
            serde_json::json!({
                "schema": loom::journey::INTERFACE_SURFACE_SCHEMA,
                "stable_id": "human-gate-cli",
                "title": "Host-mediated human gate",
                "kind": "cli",
                "identity": "loom next --mode ratify",
                "codefile": "src/human-gate.rs",
                "locator": "present",
                "operations": [{
                    "id": "present-op",
                    "summary": "Present a structured decision packet",
                    "argv": ["loom", "next", "--mode", "ratify", "--json"],
                    "read_only": false,
                    "arguments": [],
                    "output": {
                        "format": "json",
                        "assertions": [{
                            "id": "work-item",
                            "pointer": "/work_item",
                            "type": "json"
                        }]
                    }
                }]
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
            HUMAN_BINDINGS,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "setup",
            r#"{"graph":"local_snapshot","operations":[]}"#,
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
            "present",
            TruthClass::Asserted,
        )
        .unwrap();
    HumanSurfaceTopology {
        journey,
        intent,
        surface,
        surfaces,
        codefile,
    }
}

#[test]
fn doctor_accepts_a_current_compiled_journey_and_detects_chain_corruption() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("journey-doctor"), false).unwrap();
    let intent = intent(&store, "doctor behavior");
    s3_journey_proof(&store, tmp.path(), &intent.id, "doctor-proof");

    let initial = signal::doctor(&store).unwrap();
    assert!(
        initial.is_empty(),
        "canonical fixture must be clean: {initial:?}"
    );

    let exercises = store
        .edges_with(Some(EdgeKind::Exercises), None, None)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    store
        .set_facet(
            &exercises.id,
            TargetKind::Edge,
            "locator",
            "",
            TruthClass::Asserted,
        )
        .unwrap();
    let issues = signal::doctor(&store).unwrap();
    assert!(issues
        .iter()
        .any(|issue| issue.kind == "broken_journey_proof_chain"));
}

#[test]
fn doctor_detects_unratified_derivations_artifact_drift_and_retired_metadata() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("journey-doctor"), false).unwrap();
    let intent = intent(&store, "unratified doctor behavior");
    let validation =
        s3_journey_proof_unratified(&store, tmp.path(), &intent.id, "unratified-doctor-proof");
    store
        .add_node(
            NodeType::Validation,
            "retired journey validation",
            "legacy representation",
            "not_run",
            serde_json::json!({"proof_kind":"journey", "journey_id":"old"}),
        )
        .unwrap();

    let journey = store
        .edges_with(Some(EdgeKind::Proves), Some(&validation.id), None)
        .unwrap()
        .into_iter()
        .next()
        .and_then(|edge| store.get_node(&edge.to_id).unwrap())
        .unwrap();
    let artifact = journey.body["artifact"].as_str().unwrap();
    std::fs::write(tmp.path().join(artifact), "schema: broken\n").unwrap();

    let issues = signal::doctor(&store).unwrap();
    for expected in [
        "unratified_journey_derivation",
        "invalid_journey_artifact",
        "retired_journey_metadata",
    ] {
        assert!(
            issues.iter().any(|issue| issue.kind == expected),
            "missing {expected}: {issues:?}"
        );
    }
}

#[test]
fn doctor_allows_journey_id_in_an_adopted_derivation_proposal() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("journey-doctor"), false).unwrap();
    store
        .add_node(
            NodeType::Proposal,
            "accepted Journey derivation",
            "the exact technical bundle accepted by the human",
            "adopted",
            serde_json::json!({
                "schema": "loom.journey-derivation/v1",
                "journey_id": "checkout",
                "journey_hash": "0123456789abcdef",
                "manifest_hash": "fedcba9876543210"
            }),
        )
        .unwrap();

    assert!(
        signal::doctor(&store)
            .unwrap()
            .into_iter()
            .all(|issue| issue.kind != "retired_journey_metadata"),
        "a v1 adopted Proposal is not the retired Validation-as-Journey shape"
    );
}

#[test]
fn doctor_accepts_canonical_human_binding_and_rejects_corrupt_gate_shapes() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("human-gate-doctor"), false).unwrap();
    let topology = human_surface_topology(&store, tmp.path());
    let clean = signal::doctor(&store).unwrap();
    assert!(
        clean.is_empty(),
        "canonical human gate must be clean: {clean:?}"
    );

    let invalid = [
        (
            "missing binding",
            r#"[{"operation_id":"present-op","step_id":"present"}]"#,
        ),
        (
            "duplicate step",
            r#"[{"operation_id":"present-op","step_id":"present"},{"human_decision":{"operation_id":"present-op","pointer":"/work_item"},"step_id":"present"}]"#,
        ),
        (
            "unknown step",
            r#"[{"operation_id":"present-op","step_id":"present"},{"human_decision":{"operation_id":"present-op","pointer":"/work_item"},"step_id":"unknown"}]"#,
        ),
        (
            "unknown operation binding",
            r#"[{"operation_id":"missing-op","step_id":"present"},{"human_decision":{"operation_id":"missing-op","pointer":"/work_item"},"step_id":"record"}]"#,
        ),
        (
            "duplicate operation binding",
            r#"[{"operation_id":"present-op","step_id":"present"},{"operation_id":"present-op","step_id":"record"}]"#,
        ),
        (
            "unknown human source",
            r#"[{"operation_id":"present-op","step_id":"present"},{"human_decision":{"operation_id":"missing-op","pointer":"/work_item"},"step_id":"record"}]"#,
        ),
        (
            "forward human source",
            r#"[{"human_decision":{"operation_id":"present-op","pointer":"/work_item"},"step_id":"present"},{"operation_id":"present-op","step_id":"record"}]"#,
        ),
        (
            "invalid pointer",
            r#"[{"operation_id":"present-op","step_id":"present"},{"human_decision":{"operation_id":"present-op","pointer":"work_item"},"step_id":"record"}]"#,
        ),
        (
            "mixed machine and human shape",
            r#"[{"operation_id":"present-op","step_id":"present"},{"human_decision":{"operation_id":"present-op","pointer":"/work_item"},"operation_id":"present-op","step_id":"record"}]"#,
        ),
    ];
    for (case, bindings) in invalid {
        store
            .set_facet(
                &topology.surfaces.id,
                TargetKind::Edge,
                "operation_bindings",
                bindings,
                TruthClass::Asserted,
            )
            .unwrap();
        let issues = signal::doctor(&store).unwrap();
        assert!(
            issues
                .iter()
                .any(|issue| issue.kind == "bad_journey_surface_binding"),
            "{case} must fail closed: {issues:?}"
        );
    }

    store
        .set_facet(
            &topology.surfaces.id,
            TargetKind::Edge,
            "operation_bindings",
            HUMAN_BINDINGS,
            TruthClass::Asserted,
        )
        .unwrap();
    let restored = signal::doctor(&store).unwrap();
    assert!(
        restored.is_empty(),
        "restored canonical gate must be clean: {restored:?}"
    );
}

#[test]
fn intrinsic_human_binding_is_not_a_calls_or_exercises_witness() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("human-gate-proof-chain"), false).unwrap();
    let topology = human_surface_topology(&store, tmp.path());
    let surface_hash = loom::journey::surface_projection_hash(&store, &topology.journey)
        .unwrap()
        .unwrap();
    let validation = store
        .add_node(
            NodeType::Validation,
            "journey:human-gate:proof",
            "compiled human gate proof",
            "not_run",
            serde_json::json!({
                "type": "journey",
                "profile": "proof",
                "journey_hash": topology.journey.body["semantic_hash"],
                "surface_hash": surface_hash,
                "compiler_version": loom::journey::JOURNEY_COMPILER_VERSION
            }),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Proves,
            &validation.id,
            &topology.journey.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Validates,
            &validation.id,
            &topology.intent.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let missing_witness = signal::doctor(&store).unwrap();
    assert!(missing_witness
        .iter()
        .any(|issue| issue.kind == "broken_journey_proof_chain"));

    store
        .add_edge(
            EdgeKind::Calls,
            &validation.id,
            &topology.surface.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let exercises = store
        .add_edge(
            EdgeKind::Exercises,
            &validation.id,
            &topology.codefile.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &exercises.id,
            TargetKind::Edge,
            "locator",
            "present",
            TruthClass::Asserted,
        )
        .unwrap();
    let complete = signal::doctor(&store).unwrap();
    assert!(
        complete.is_empty(),
        "explicit chain must be clean: {complete:?}"
    );
}

#[test]
fn doctor_checks_only_graph_referenced_source_anchors_and_fails_on_duplicates() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("anchor-doctor"), false).unwrap();
    std::fs::write(
        tmp.path().join("entry.rs"),
        "// loom:anchor doctor.entry\npub fn entry() {}\n",
    )
    .unwrap();
    let file = store
        .add_node(
            NodeType::CodeFile,
            "entry.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let behavior = intent(&store, "doctor anchor resolves");
    let edge = store
        .add_edge(
            EdgeKind::Implements,
            &behavior.id,
            &file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "locator",
            "anchor:doctor.entry",
            TruthClass::Asserted,
        )
        .unwrap();

    let valid = signal::doctor(&store).unwrap();
    assert!(
        valid
            .iter()
            .all(|issue| issue.kind != "invalid_source_anchor"),
        "valid referenced anchor must be clean: {valid:?}"
    );

    std::fs::write(
        tmp.path().join("copy.rs"),
        "// loom:anchor doctor.entry\npub fn copy() {}\n",
    )
    .unwrap();
    store
        .add_node(NodeType::CodeFile, "copy.rs", "", "", serde_json::json!({}))
        .unwrap();
    let issues = signal::doctor(&store).unwrap();
    let issue = issues
        .iter()
        .find(|issue| issue.kind == "invalid_source_anchor")
        .expect("duplicate referenced anchor must be diagnosed");
    assert!(issue.message.contains("duplicated"), "{issue:?}");
}
