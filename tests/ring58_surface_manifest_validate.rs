//! Full SurfaceManifest validation coverage: typed outputs, downstream
//! inputs, optional exercises, multi-step Journeys, and human decisions.

use loom::journey::{
    CliOperation, JourneySpec, OperationArgument, OutputFormat, SurfaceManifest, ValueType,
    JOURNEY_SCHEMA,
};
use serde_json::json;

fn spec_with_outputs() -> JourneySpec {
    serde_json::from_value(json!({
        "schema": JOURNEY_SCHEMA,
        "id": "multi.step",
        "name": "Multi step",
        "actor": "operator",
        "goal": "Exercise the manifest validator",
        "inputs": {},
        "preconditions": [],
        "steps": [
            {
                "id": "create",
                "name": "Create",
                "action": "creates the thing",
                "expects": [],
                "produces": {"order-id": {"type": "string", "description": "Created order id"}}
            },
            {
                "id": "verify",
                "name": "Verify",
                "action": "verifies the thing",
                "expects": ["the order exists"],
                "produces": {}
            }
        ],
        "profiles": {"proof": {"inputs": {}, "workspace": {}}}
    }))
    .unwrap()
}

fn op_create() -> CliOperation {
    serde_json::from_value(json!({
        "id": "create-op",
        "summary": "Create",
        "argv": ["python3", "-c", "print('{}')"],
        "read_only": true,
        "arguments": [],
        "output": {
            "format": "json",
            "captures": [{"id": "order-id", "pointer": "/order_id", "type": "string"}],
            "assertions": [{"id": "create-ok", "pointer": "/ok", "type": "boolean", "equals": true}]
        }
    }))
    .unwrap()
}

fn op_verify() -> CliOperation {
    CliOperation {
        id: "verify-op".into(),
        summary: "Verify".into(),
        argv: vec!["python3".into(), "-c".into(), "print('{}')".into()],
        environment: Vec::new(),
        read_only: true,
        timeout_seconds: None,
        arguments: vec![OperationArgument {
            id: "order".into(),
            value_type: ValueType::String,
            required: true,
            flag: Some("--order".into()),
            source: Some("steps.create.outputs.order-id".into()),
            redact: false,
        }],
        output: loom::journey::OperationOutput {
            format: OutputFormat::Json,
            captures: Vec::new(),
            assertions: vec![loom::journey::OutputAssertion {
                id: "verify-ok".into(),
                pointer: "/ok".into(),
                value_type: Some(ValueType::Boolean),
                equals: Some(json!(true)),
                source: None,
            }],
            redact: Vec::new(),
        },
        exercises: Vec::new(),
    }
}

fn base_manifest(spec: &JourneySpec) -> serde_json::Value {
    json!({
        "schema": loom::journey::SURFACE_SCHEMA,
        "journey_id": spec.id,
        "journey_hash": spec.semantic_hash().unwrap(),
        "surface": {
            "id": "multi-cli",
            "title": "Multi CLI",
            "identity": "multi",
            "codefile": "src/multi_cli.rs",
            "locator": "multi_cli",
            "operations": [op_create(), op_verify()]
        },
        "bindings": [
            {"step_id": "create", "operation_id": "create-op"},
            {"step_id": "verify", "operation_id": "verify-op"}
        ]
    })
}

fn validate(manifest: &serde_json::Value, spec: &JourneySpec) -> Result<(), String> {
    let manifest: SurfaceManifest =
        serde_json::from_value(manifest.clone()).map_err(|error| format!("decode: {error}"))?;
    manifest
        .validate_for(spec, &spec.semantic_hash().unwrap())
        .map_err(|error| format!("{error:#}"))
}

#[test]
fn typed_outputs_must_match_authored_produces() {
    let spec = spec_with_outputs();
    let base = base_manifest(&spec);
    validate(&base, &spec).expect("the baseline manifest must validate");

    // A capture for an output the step does not produce is refused.
    let mut undeclared = base.clone();
    undeclared["surface"]["operations"][0]["output"]["captures"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "ghost", "pointer": "/ghost", "type": "string"}));
    let err = validate(&undeclared, &spec).unwrap_err();
    assert!(err.contains("undeclared output"), "{err}");

    // A missing capture for an authored output is refused.
    let mut missing = base.clone();
    missing["surface"]["operations"][0]["output"]["captures"] = json!([]);
    let err = validate(&missing, &spec).unwrap_err();
    assert!(err.contains("does not capture"), "{err}");

    // A type mismatch with the authored output is refused.
    let mut wrong_type = base.clone();
    wrong_type["surface"]["operations"][0]["output"]["captures"][0]["type"] = json!("integer");
    let err = validate(&wrong_type, &spec).unwrap_err();
    assert!(err.contains("type does not match"), "{err}");
}

#[test]
fn downstream_arguments_reference_only_prior_outputs() {
    let spec = spec_with_outputs();
    let base = base_manifest(&spec);
    validate(&base, &spec).expect("a downstream reference to a prior output must validate");

    // A forward reference (a later step's output, or the step's own) refuses.
    let mut forward = base.clone();
    forward["surface"]["operations"][1]["arguments"][0]["source"] =
        json!("steps.verify.outputs.nothing");
    let err = validate(&forward, &spec).unwrap_err();
    assert!(
        err.contains("not an output of a prior Journey step"),
        "{err}"
    );

    // An unknown authored input refuses.
    let mut unknown = base.clone();
    unknown["surface"]["operations"][1]["arguments"][0]["source"] = json!("inputs.missing");
    let err = validate(&unknown, &spec).unwrap_err();
    assert!(err.contains("unknown Journey input"), "{err}");

    // Removing the source (defaults to inputs.<id>) also refuses: the spec
    // declares no such input.
    let mut defaulted = base.clone();
    defaulted["surface"]["operations"][1]["arguments"][0]
        .as_object_mut()
        .unwrap()
        .remove("source");
    let err = validate(&defaulted, &spec).unwrap_err();
    assert!(err.contains("unknown Journey input"), "{err}");
}

#[test]
fn exercises_are_optional_and_structurally_validated_when_present() {
    let spec = spec_with_outputs();
    let base = base_manifest(&spec);
    // No exercises anywhere: the manifest must validate (exercises are
    // downstream-process provenance the authored model cannot infer).
    validate(&base, &spec).expect("a manifest without exercises must validate");

    // A well-formed exercise (observed_by naming a declared assertion in the
    // same operation) is structurally valid.
    let mut with_exercise = base.clone();
    with_exercise["surface"]["operations"][0]["exercises"] = json!([{
        "id": "create-downstream-entry",
        "codefile": "src/handler.rs",
        "locator": "post_create",
        "observed_by": "create-ok"
    }]);
    validate(&with_exercise, &spec).expect("a declared exercise must validate");

    // observed_by must name an assertion of the same operation.
    let mut bad_observed_by = base.clone();
    bad_observed_by["surface"]["operations"][0]["exercises"] = json!([{
        "id": "create-downstream-entry",
        "codefile": "src/handler.rs",
        "locator": "post_create",
        "observed_by": "verify-ok"
    }]);
    let err = validate(&bad_observed_by, &spec).unwrap_err();
    assert!(err.contains("observed_by"), "{err}");
}

#[test]
fn multi_step_journeys_bind_every_step_exactly_once() {
    let spec = spec_with_outputs();
    let base = base_manifest(&spec);
    validate(&base, &spec).expect("both steps bound once must validate");

    let mut unbound = base.clone();
    unbound["bindings"] = json!([{"step_id": "create", "operation_id": "create-op"}]);
    let err = validate(&unbound, &spec).unwrap_err();
    assert!(err.contains("does not bind"), "{err}");

    let mut duplicate = base.clone();
    duplicate["bindings"] = json!([
        {"step_id": "create", "operation_id": "create-op"},
        {"step_id": "verify", "operation_id": "create-op"}
    ]);
    let err = validate(&duplicate, &spec).unwrap_err();
    assert!(err.contains("bound more than once"), "{err}");

    let mut unknown_op = base.clone();
    unknown_op["bindings"] = json!([
        {"step_id": "create", "operation_id": "create-op"},
        {"step_id": "verify", "operation_id": "missing-op"}
    ]);
    let err = validate(&unknown_op, &spec).unwrap_err();
    assert!(err.contains("unknown operation"), "{err}");
}

#[test]
fn human_decision_bindings_require_setup_and_a_prior_operation() {
    let spec = spec_with_outputs();
    let base = base_manifest(&spec);

    // A human-gated step: the verify step becomes the host-mediated decision.
    let human_bindings = json!([
        {"step_id": "create", "operation_id": "create-op"},
        {
            "step_id": "verify",
            "human_decision": {"operation_id": "create-op", "pointer": "/work_item"}
        }
    ]);
    let mut manifest = base.clone();
    manifest["bindings"] = human_bindings.clone();

    // Without a local_snapshot setup, the gate is refused.
    let err = validate(&manifest, &spec).unwrap_err();
    assert!(err.contains("require setup.graph=local_snapshot"), "{err}");

    // With setup, the binding validates.
    manifest["setup"] = json!({
        "graph": "local_snapshot",
        "operations": [],
        "before_steps": {}
    });
    validate(&manifest, &spec).expect("a human decision with local_snapshot setup must validate");

    // The gate must reference an operation bound to an EARLIER step.
    let mut late_gate = manifest.clone();
    late_gate["bindings"] = json!([
        {"step_id": "verify", "operation_id": "verify-op"},
        {
            "step_id": "create",
            "human_decision": {"operation_id": "verify-op", "pointer": "/work_item"}
        }
    ]);
    let err = validate(&late_gate, &spec).unwrap_err();
    assert!(err.contains("earlier authored step"), "{err}");

    // A human-gated step cannot declare produced machine outputs.
    let mut producing_spec = spec_with_outputs();
    producing_spec.steps[1].produces.insert(
        "verdict".into(),
        loom::journey::JourneyOutput {
            value_type: ValueType::String,
            description: String::new(),
        },
    );
    manifest["journey_hash"] = json!(producing_spec.semantic_hash().unwrap());
    let err = validate(&manifest, &producing_spec).unwrap_err();
    assert!(
        err.contains("cannot declare produced machine outputs"),
        "{err}"
    );
}

#[test]
fn setup_operations_are_mutable_unique_and_unbound() {
    let spec = spec_with_outputs();
    let mut manifest = base_manifest(&spec);

    // A read_only setup operation is refused: setup must establish the
    // isolated fixture.
    let mut setup_op: CliOperation = op_create();
    setup_op.id = "fixture-op".into();
    setup_op.read_only = true;
    setup_op.output = loom::journey::OperationOutput {
        format: OutputFormat::Json,
        captures: Vec::new(),
        assertions: setup_op.output.assertions.clone(),
        redact: Vec::new(),
    };
    manifest["setup"] = json!({
        "graph": "local_snapshot",
        "operations": ["fixture-op"],
        "before_steps": {}
    });
    manifest["surface"]["operations"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::to_value(&setup_op).unwrap());
    let err = validate(&manifest, &spec).unwrap_err();
    assert!(err.contains("must be mutable"), "{err}");

    // A setup operation that is also bound to an authored step is refused.
    let mut setup_op = setup_op.clone();
    setup_op.read_only = false;
    manifest["surface"]["operations"] = json!([op_create(), op_verify(), setup_op]);
    manifest["setup"]["operations"] = json!(["create-op"]);
    let err = validate(&manifest, &spec).unwrap_err();
    assert!(err.contains("also bound to an authored step"), "{err}");

    // A setup operation capturing authored step outputs is refused.
    let capturing: CliOperation = serde_json::from_value(json!({
        "id": "fixture-op",
        "summary": "Fixture",
        "argv": ["python3", "-c", "print('{}')"],
        "read_only": false,
        "arguments": [],
        "output": {
            "format": "json",
            "captures": [{"id": "order-id", "pointer": "/order_id", "type": "string"}],
            "assertions": [{"id": "fixture-ok", "pointer": "/ok", "type": "boolean", "equals": true}]
        }
    }))
    .unwrap();
    manifest["surface"]["operations"] = json!([op_create(), op_verify(), capturing]);
    manifest["setup"]["operations"] = json!(["fixture-op"]);
    let err = validate(&manifest, &spec).unwrap_err();
    assert!(
        err.contains("must not capture authored step outputs"),
        "{err}"
    );

    // A valid mutable setup operation with assertions passes.
    let mut valid: CliOperation = serde_json::from_value(json!({
        "id": "fixture-op",
        "summary": "Fixture",
        "argv": ["python3", "-c", "print('{}')"],
        "read_only": false,
        "arguments": [],
        "output": {
            "format": "json",
            "assertions": [{"id": "fixture-ok", "pointer": "/ok", "type": "boolean", "equals": true}]
        }
    }))
    .unwrap();
    valid.output.redact = Vec::new();
    manifest["surface"]["operations"] = json!([op_create(), op_verify(), valid]);
    manifest["setup"]["operations"] = json!(["fixture-op"]);
    validate(&manifest, &spec)
        .expect("a mutable, unbound, asserting setup operation must validate");
}
