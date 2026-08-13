//! Ring 18 — compiled Journey profile baselines.

use loom::journey::{
    CliOperation, JourneySpec, OperationBinding, OperationOutput, OutputAssertion, OutputFormat,
    ValueType, JOURNEY_SCHEMA,
};
use loom::journey_runtime::{self, CompiledJourneyProof, RuntimeStatus};
use serde_json::json;
use std::collections::BTreeMap;

mod common;
use common::*;

fn spec() -> JourneySpec {
    serde_json::from_value(json!({
        "schema": JOURNEY_SCHEMA,
        "id": "baseline.demo",
        "name": "Baseline demo",
        "actor": "operator",
        "goal": "Print a stable result",
        "inputs": {},
        "preconditions": [],
        "steps": [{"id":"print","name":"Print","action":"prints the result","expects":[],"produces":{}}],
        "profiles":{"proof":{"inputs":{},"workspace":{}}}
    }))
    .unwrap()
}

fn operation(value: &str) -> CliOperation {
    CliOperation {
        id: "print-op".into(),
        summary: "Print JSON".into(),
        argv: vec![
            "python3".into(),
            "-c".into(),
            format!("import json; print(json.dumps({{'value':'{value}'}}))"),
        ],
        environment: Vec::new(),
        read_only: true,
        timeout_seconds: None,
        expected_exit: 0,
        arguments: Vec::new(),
        output: OperationOutput {
            format: OutputFormat::Json,
            captures: Vec::new(),
            assertions: vec![OutputAssertion {
                id: "value-is-stable".into(),
                pointer: "/value".into(),
                value_type: Some(ValueType::String),
                equals: Some(json!(value)),
                source: None,
            }],
            redact: Vec::new(),
        },
        exercises: Vec::new(),
    }
}

fn compiled(value: &str, surface_hash: &str) -> CompiledJourneyProof {
    journey_runtime::compile(
        &spec(),
        surface_hash,
        "proof",
        vec![operation(value)],
        &[OperationBinding {
            step_id: "print".into(),
            operation_id: "print-op".into(),
        }],
    )
    .unwrap()
}

#[test]
fn freeze_then_identical_replay_has_a_current_baseline() {
    let tmp = Tmp::new();
    let proof = compiled("stable", "surface-v1");
    let first = journey_runtime::execute(tmp.path(), &spec(), &proof, &BTreeMap::new());
    assert_eq!(first.status, RuntimeStatus::Passed, "{first:#?}");
    journey_runtime::write_baseline(tmp.path(), &first).unwrap();

    let replay = journey_runtime::execute(tmp.path(), &spec(), &proof, &BTreeMap::new());
    assert_eq!(replay, first);
    assert_eq!(
        journey_runtime::baseline_current(tmp.path(), &proof).unwrap(),
        Some(true)
    );
}

#[test]
fn compiled_contract_drift_invalidates_the_baseline() {
    let tmp = Tmp::new();
    let original = compiled("before", "surface-v1");
    let report = journey_runtime::execute(tmp.path(), &spec(), &original, &BTreeMap::new());
    journey_runtime::write_baseline(tmp.path(), &report).unwrap();

    let drifted = compiled("after", "surface-v2");
    assert_eq!(
        journey_runtime::baseline_current(tmp.path(), &drifted).unwrap(),
        Some(false),
        "a baseline is bound to the exact semantic Journey and surface projection"
    );
}

#[test]
fn only_a_passing_compiled_observation_can_be_frozen() {
    let tmp = Tmp::new();
    let mut proof = compiled("expected", "surface-v1");
    proof.steps[0].argv = vec![
        "python3".into(),
        "-c".into(),
        "import json; print(json.dumps({'value':'wrong'}))".into(),
    ];
    let failed = journey_runtime::execute(tmp.path(), &spec(), &proof, &BTreeMap::new());
    assert_eq!(failed.status, RuntimeStatus::Failed, "{failed:#?}");
    let error = journey_runtime::write_baseline(tmp.path(), &failed).unwrap_err();
    assert!(error.to_string().contains("only a passing"), "{error}");
    assert!(
        !journey_runtime::baseline_path(tmp.path(), "baseline.demo", "proof")
            .unwrap()
            .exists()
    );
}

#[test]
fn failed_refreeze_preserves_the_prior_baseline_bytes() {
    let tmp = Tmp::new();
    let proof = compiled("stable", "surface-v1");
    let passed = journey_runtime::execute(tmp.path(), &spec(), &proof, &BTreeMap::new());
    let path = journey_runtime::write_baseline(tmp.path(), &passed).unwrap();
    let before = std::fs::read(&path).unwrap();

    let mut failing = proof;
    failing.steps[0].argv = vec!["definitely-not-a-real-ring18-command".into()];
    let blocked = journey_runtime::execute(tmp.path(), &spec(), &failing, &BTreeMap::new());
    assert_eq!(blocked.status, RuntimeStatus::Blocked);
    assert!(journey_runtime::write_baseline(tmp.path(), &blocked).is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn an_uncompiled_or_empty_profile_cannot_be_frozen() {
    let mut empty = spec();
    empty.steps.clear();
    let error =
        journey_runtime::compile(&empty, "surface-v1", "proof", Vec::new(), &[]).unwrap_err();
    assert!(
        error.to_string().contains("at least one semantic step"),
        "{error}"
    );
}
