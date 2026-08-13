//! Ring 45 — strict semantic Journey roots and accepted projections.

use loom::model::{EdgeKind, NodeType, TargetKind};
use loom::store::Store;
use serde_json::{json, Value};

mod common;
use common::*;

fn semantic_journey(id: &str, name: &str) -> Value {
    json!({
        "schema": "loom.journey/v1",
        "id": id,
        "name": name,
        "actor": "shopper",
        "goal": "Complete a semantic consumer flow",
        "inputs": {
            "sku": {"type":"string", "description":"Catalog item SKU"},
            "quantity": {"type":"integer", "description":"Requested quantity", "default":1}
        },
        "preconditions": ["the item exists in the catalog"],
        "steps": [
            {
                "id":"add-item",
                "name":"Add item",
                "action":"adds the item to the cart",
                "expects":[],
                "produces":{"cart-id":{"type":"string","description":"Created cart id"}}
            },
            {
                "id":"submit-order",
                "name":"Submit order",
                "action":"submits the order",
                "expects":["the order is accepted"],
                "produces":{"order-id":{"type":"string","description":"Accepted order id"}}
            }
        ],
        "profiles": {
            "proof": {
                "inputs":{
                    "sku":{"template":"sku-1"},
                    "quantity":{"template":"2"}
                },
                "workspace":{
                    "directories":["fixtures"],
                    "files":[{"path":"fixtures/catalog.json", "content":"{\"sku\":\"sku-1\"}"}],
                    "env":{"APP_MODE":"test"}
                }
            }
        }
    })
}

fn write_journey(tmp: &Tmp, id: &str, title: &str) -> std::path::PathBuf {
    let path = tmp.path().join(format!("{id}.journey.json"));
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&semantic_journey(id, title)).unwrap(),
    )
    .unwrap();
    path
}

fn derivation_manifest(hash: Value) -> Value {
    json!({
        "schema":"loom.journey-derivation/v1",
        "journey_id":"checkout.happy",
        "journey_hash":hash,
        "proposal_id":"checkout-technical-projection",
        "proposal_rationale":"These two technical behaviors are the minimal independently falsifiable implementation projection.",
        "intents":[
            {
                "id":"cart-accepts-item",
                "operation":"create",
                "name":"cart accepts a catalog item",
                "criterion":"Adding an available SKU records the requested quantity in the cart",
                "level":"feature",
                "visibility":"internal",
                "rationale":"Cart mutation is independently observable after the add-item Journey step.",
                "step_ids":["add-item"]
            },
            {
                "id":"order-is-submitted",
                "operation":"create",
                "name":"an order is submitted from the cart",
                "criterion":"Submitting a valid cart creates one accepted order",
                "level":"feature",
                "visibility":"internal",
                "rationale":"Order creation is independently falsifiable after the submit-order Journey step.",
                "step_ids":["submit-order"]
            }
        ],
        "relationships":[{
            "id":"submission-requires-cart",
            "kind":"requires",
            "from":"order-is-submitted",
            "to":"cart-accepts-item",
            "rationale":"An order can only be submitted from a previously populated cart."
        }],
        "unresolved_question":null
    })
}

fn builder_command(tmp: &Tmp, args: &[&str]) -> std::process::Output {
    let mut command = loom_command();
    command
        .env("LOOM_AGENT", "llm:builder")
        .arg("--graph")
        .arg(tmp.path())
        .args(args);
    command.output().expect("spawn loom")
}

fn builder_json(tmp: &Tmp, args: &[&str]) -> Value {
    let output = builder_command(tmp, args);
    assert!(
        output.status.success(),
        "loom {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "loom {args:?} did not emit JSON: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn lint_surface(journey_id: &str, journey_hash: Value, assertion: Value) -> Value {
    json!({
        "schema":"loom.journey.surface/v1", "journey_id":journey_id,
        "journey_hash":journey_hash,
        "surface":{
            "id":format!("{journey_id}-cli"), "title":"Lint CLI", "identity":journey_id,
            "codefile":"src/shop_cli.rs", "locator":"run_shop_cli",
            "operations":[
                {"id":"cart-add", "summary":"Add", "argv":["shop","add"], "arguments":[],
                 "output":{"format":"json", "captures":[{"id":"cart-id","pointer":"/cart_id","type":"string"}], "assertions":[assertion]}},
                {"id":"order-submit", "summary":"Submit", "argv":["shop","submit"], "arguments":[],
                 "output":{"format":"json", "captures":[{"id":"order-id","pointer":"/order_id","type":"string"}]}}
            ]
        },
        "bindings":[
            {"step_id":"add-item", "operation_id":"cart-add"},
            {"step_id":"submit-order", "operation_id":"order-submit"}
        ]
    })
}

#[test]
fn journey_lint_command_contract_and_surface_accept_policy_are_end_to_end() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("journey-lint"));
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/shop_cli.rs"),
        "pub fn run_shop_cli() {}\n",
    )
    .unwrap();
    builder_json(&tmp, &["codefile", "add", "src/shop_cli.rs", "--json"]);
    std::fs::create_dir_all(tmp.path().join("surfaces")).unwrap();

    let mut manifests = Vec::new();
    for (id, assertion) in [
        (
            "alpha.flow",
            json!({"id":"position", "pointer":"/rows/0/name", "equals":"a"}),
        ),
        (
            "beta.flow",
            json!({"id":"count", "pointer":"/entry_count", "equals":2}),
        ),
    ] {
        let path = write_journey(&tmp, id, id);
        builder_json(&tmp, &["journey", "add", path.to_str().unwrap(), "--json"]);
        let packet = builder_json(&tmp, &["journey", "surface", id, "--json"]);
        let manifest = lint_surface(id, packet["semantic_hash"].clone(), assertion);
        let path = tmp
            .path()
            .join("surfaces")
            .join(format!("{id}.surface.json"));
        std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
        manifests.push((id, path, manifest));
    }

    let report = builder_json(&tmp, &["journey", "lint", "--json"]);
    assert_eq!(
        report,
        json!({
            "schema":"loom.journey-lint/v1", "status":"passed", "scanned":2,
            "blocking":0, "advisory":2,
            "findings":[
                {"rule":"exact-census-pin", "severity":"advisory", "journey_id":"beta.flow",
                 "manifest_path":"surfaces/beta.flow.surface.json", "operation":"cart-add",
                 "assertion":"count", "message":"assert an invariant or bounded relationship instead of an exact whole-graph count or total"},
                {"rule":"positional-census-pointer", "severity":"advisory", "journey_id":"alpha.flow",
                 "manifest_path":"surfaces/alpha.flow.surface.json", "operation":"cart-add",
                 "assertion":"position", "message":"select census data by stable identity instead of a numeric JSON-pointer position"}
            ]
        })
    );
    let targeted = builder_json(&tmp, &["journey", "lint", "alpha.flow", "--json"]);
    assert_eq!(targeted["scanned"], 1);
    assert_eq!(targeted["advisory"], 1);

    let advisory_accept = builder_json(
        &tmp,
        &[
            "journey",
            "surface-accept",
            manifests[0].0,
            "--manifest",
            manifests[0].1.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(advisory_accept["surface_created"], true);
    let before = Store::open(tmp.path())
        .unwrap()
        .list_nodes(Some(NodeType::InterfaceSurface), usize::MAX)
        .unwrap()
        .len();

    let mut blocked = manifests[1].2.clone();
    blocked["surface"]["operations"][0]["argv"] =
        json!(["shop", "0123456789abcdef0123456789abcdef"]);
    let blocked_path = tmp.path().join("blocked.surface.json");
    std::fs::write(&blocked_path, serde_json::to_vec_pretty(&blocked).unwrap()).unwrap();
    let output = builder_command(
        &tmp,
        &[
            "journey",
            "surface-accept",
            "beta.flow",
            "--manifest",
            blocked_path.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("surface lint blocked acceptance"));
    let after = Store::open(tmp.path())
        .unwrap()
        .list_nodes(Some(NodeType::InterfaceSurface), usize::MAX)
        .unwrap()
        .len();
    assert_eq!(
        after, before,
        "blocked acceptance must not mutate the graph"
    );

    let missing = write_journey(&tmp, "missing.flow", "Missing");
    builder_json(
        &tmp,
        &["journey", "add", missing.to_str().unwrap(), "--json"],
    );
    let output = builder_command(&tmp, &["journey", "lint", "missing.flow", "--json"]);
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("missing.flow") && error.contains("surfaces/missing.flow.surface.json"),
        "{error}"
    );
}

#[test]
fn every_dogfood_journey_uses_the_strict_profile_map_and_parses() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("journeys");
    for entry in std::fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("yaml") {
            continue;
        }
        let spec = loom::journey::parse(&path)
            .unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
        assert!(
            spec.profiles.contains_key("proof"),
            "{} has no proof profile",
            path.display()
        );
    }
}

#[test]
fn strict_schema_rejects_unknown_and_legacy_transport_fields() {
    let tmp = Tmp::new();
    for (name, field, value) in [
        ("unknown", "mystery", json!(true)),
        ("run", "run", json!("app checkout")),
        ("request", "request", json!({"url":"/orders"})),
        ("intent", "intent", json!("orders are accepted")),
        ("routes", "routes", json!([])),
        ("base", "base", json!("https://example.test")),
    ] {
        let mut document = semantic_journey("checkout.happy", "Checkout");
        document[field] = value;
        let path = tmp.path().join(format!("{name}.json"));
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        let error = loom::journey::parse(&path).unwrap_err().to_string();
        assert!(
            error.contains("unknown field") || error.contains("parsing"),
            "legacy field {field} was not rejected clearly: {error}"
        );
    }

    let mut nested = semantic_journey("checkout.happy", "Checkout");
    nested["steps"][0]["http"] = json!({"method":"POST"});
    let path = tmp.path().join("nested.json");
    std::fs::write(&path, serde_json::to_vec(&nested).unwrap()).unwrap();
    assert!(loom::journey::parse(&path).is_err());
}

#[test]
fn canonical_hash_is_transport_and_format_independent_but_step_order_sensitive() {
    let tmp = Tmp::new();
    let json_path = tmp.path().join("journey.json");
    let yaml_path = tmp.path().join("journey.yaml");
    let document = semantic_journey("checkout.happy", "Checkout");
    std::fs::write(&json_path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    std::fs::write(&yaml_path, serde_norway::to_string(&document).unwrap()).unwrap();
    let json_spec = loom::journey::parse(&json_path).unwrap();
    let yaml_spec = loom::journey::parse(&yaml_path).unwrap();
    assert_eq!(
        json_spec.semantic_hash().unwrap(),
        yaml_spec.semantic_hash().unwrap()
    );

    let mut reordered = json_spec.clone();
    reordered.name = "Checkout renamed".into();
    reordered
        .profiles
        .get_mut("proof")
        .unwrap()
        .workspace
        .directories
        .reverse();
    assert_eq!(
        json_spec.semantic_hash().unwrap(),
        reordered.semantic_hash().unwrap()
    );
    reordered.steps.reverse();
    assert_ne!(
        json_spec.semantic_hash().unwrap(),
        reordered.semantic_hash().unwrap()
    );
}

#[test]
fn profile_timeout_defaults_positive_and_is_semantic_hash_neutral() {
    let document = semantic_journey("checkout.timeout", "Checkout timeout");
    let base: loom::journey::JourneySpec = serde_json::from_value(document).unwrap();
    assert_eq!(base.profiles["proof"].timeout_seconds, 2700);

    let mut changed = base.clone();
    changed.profiles.get_mut("proof").unwrap().timeout_seconds = 19;
    assert_eq!(
        base.semantic_hash().unwrap(),
        changed.semantic_hash().unwrap()
    );
    assert_ne!(
        base.canonical_value().unwrap(),
        changed.canonical_value().unwrap()
    );

    changed.profiles.get_mut("proof").unwrap().timeout_seconds = 0;
    assert!(changed
        .validate()
        .unwrap_err()
        .to_string()
        .contains("positive"));
}

#[test]
fn semantic_validation_rejects_bad_references_types_and_temporary_paths() {
    let base: loom::journey::JourneySpec =
        serde_json::from_value(semantic_journey("checkout.happy", "Checkout")).unwrap();

    let mut forward_reference = base.clone();
    forward_reference.steps[0].action =
        "use {{ steps.submit-order.outputs.order-id }} before it exists".into();
    assert!(forward_reference
        .validate()
        .unwrap_err()
        .to_string()
        .contains("prior Journey step"));

    let mut bad_type = base.clone();
    bad_type
        .profiles
        .get_mut("proof")
        .unwrap()
        .inputs
        .get_mut("quantity")
        .unwrap()
        .template = Some("two".into());
    assert!(bad_type
        .validate()
        .unwrap_err()
        .to_string()
        .contains("wrong type"));

    let mut escape = base.clone();
    escape.profiles.get_mut("proof").unwrap().workspace.files[0].path = "../outside".into();
    assert!(escape
        .validate()
        .unwrap_err()
        .to_string()
        .contains("escapes"));

    let mut invalid_output = base;
    invalid_output.steps[1].produces.insert(
        "Invalid Output".into(),
        loom::journey::JourneyOutput {
            value_type: loom::journey::ValueType::String,
            description: String::new(),
        },
    );
    assert!(invalid_output.validate().is_err());
}

#[test]
fn proof_profile_secrets_are_env_only_and_never_literal_or_template_values() {
    let mut document = semantic_journey("checkout.secure", "Secure checkout");
    document["inputs"]["token"] = json!({
        "type":"string",
        "description":"Checkout authorization token",
        "secret":true
    });
    document["profiles"]["proof"]["inputs"]["token"] = json!({"env":"CHECKOUT_TOKEN"});
    let valid: loom::journey::JourneySpec = serde_json::from_value(document.clone()).unwrap();
    valid.validate().unwrap();

    let mut defaulted = document.clone();
    defaulted["inputs"]["token"]["default"] = json!("literal-secret");
    assert!(
        serde_json::from_value::<loom::journey::JourneySpec>(defaulted)
            .unwrap()
            .validate()
            .unwrap_err()
            .to_string()
            .contains("must not declare a default")
    );

    let mut templated = document.clone();
    templated["profiles"]["proof"]["inputs"]["token"] = json!({"template":"literal-secret"});
    assert!(
        serde_json::from_value::<loom::journey::JourneySpec>(templated)
            .unwrap()
            .validate()
            .unwrap_err()
            .to_string()
            .contains("must bind via")
    );

    let mut raw_literal = document.clone();
    raw_literal["profiles"]["proof"]["inputs"]["token"] = json!("literal-secret");
    assert!(serde_json::from_value::<loom::journey::JourneySpec>(raw_literal).is_err());

    let mut authored_env_value = document;
    authored_env_value["profiles"]["proof"]["workspace"]["env"]["CHECKOUT_TOKEN"] =
        json!("literal-secret");
    assert!(
        serde_json::from_value::<loom::journey::JourneySpec>(authored_env_value)
            .unwrap()
            .validate()
            .unwrap_err()
            .to_string()
            .contains("must come from the process")
    );
}

#[test]
fn proof_profile_is_required_and_runtime_sources_are_scoped_and_ordered() {
    let mut no_proof = semantic_journey("checkout.happy", "Checkout");
    no_proof["profiles"] = json!({"demo":{"inputs":{},"workspace":{}}});
    assert!(
        serde_json::from_value::<loom::journey::JourneySpec>(no_proof)
            .unwrap()
            .validate()
            .unwrap_err()
            .to_string()
            .contains("profiles.proof")
    );

    let spec: loom::journey::JourneySpec =
        serde_json::from_value(semantic_journey("checkout.happy", "Checkout")).unwrap();
    let hash = spec.semantic_hash().unwrap();
    let manifest_value = json!({
        "schema":"loom.journey.surface/v1",
        "journey_id":"checkout.happy",
        "journey_hash":hash,
        "surface":{
            "id":"shop-cli",
            "title":"Shop CLI",
            "identity":"shop",
            "codefile":"src/shop_cli.rs",
            "locator":"run_shop_cli",
            "operations":[
                {
                    "id":"cart-add",
                    "summary":"Add item",
                    "argv":["shop","cart","add"],
                    "arguments":[{"id":"sku","type":"string","source":"inputs.sku"}],
                    "output":{
                        "format":"json",
                        "captures":[{"id":"cart-id","pointer":"/cart_id","type":"string"}],
                        "assertions":[{"id":"added","pointer":"/added","type":"boolean"}]
                    }
                },
                {
                    "id":"order-submit",
                    "summary":"Submit order",
                    "argv":["shop","order","submit"],
                    "arguments":[
                        {"id":"cart-id","type":"string","source":"steps.add-item.outputs.cart-id"},
                        {"id":"run-id","type":"string","source":"run.id"}
                    ],
                    "output":{
                        "format":"json",
                        "captures":[{"id":"order-id","pointer":"/order_id","type":"string"}],
                        "assertions":[{"id":"accepted","pointer":"/sku","source":"inputs.sku"}]
                    }
                }
            ]
        },
        "bindings":[
            {"step_id":"add-item","operation_id":"cart-add"},
            {"step_id":"submit-order","operation_id":"order-submit"}
        ]
    });
    let valid: loom::journey::SurfaceManifest =
        serde_json::from_value(manifest_value.clone()).unwrap();
    valid.validate_for(&spec, &hash).unwrap();

    let mut secret_spec = spec.clone();
    secret_spec.inputs.get_mut("sku").unwrap().secret = true;
    assert!(valid
        .validate_for(&secret_spec, &hash)
        .unwrap_err()
        .to_string()
        .contains("environment-only"));

    let mut legacy = manifest_value.clone();
    legacy["surface"]["operations"][0]["arguments"][0]["source"] = json!("input:sku");
    assert!(
        serde_json::from_value::<loom::journey::SurfaceManifest>(legacy)
            .unwrap()
            .validate_for(&spec, &hash)
            .is_err()
    );

    let mut forward = manifest_value;
    forward["surface"]["operations"][0]["arguments"][0]["source"] =
        json!("steps.submit-order.outputs.order-id");
    let forward_error = serde_json::from_value::<loom::journey::SurfaceManifest>(forward)
        .unwrap()
        .validate_for(&spec, &hash)
        .unwrap_err();
    assert!(format!("{forward_error:#}").contains("prior Journey step"));
}

#[test]
fn operation_environment_declarations_are_strict_unique_and_canonical() {
    let operation = json!({
        "id":"inspect-toolchain",
        "summary":"Inspect the declared toolchain homes",
        "argv":["tool","inspect","--json"],
        "environment":["RUSTUP_HOME","CARGO_HOME"],
        "read_only":true,
        "output":{
            "format":"json",
            "assertions":[{"id":"ready","pointer":"/ready","equals":true}]
        }
    });
    let surface: loom::journey::InterfaceSurfaceDefinition = serde_json::from_value(json!({
        "id":"toolchain-cli",
        "title":"Toolchain CLI",
        "identity":"toolchain inspection",
        "codefile":"src/tool.rs",
        "locator":"inspect",
        "operations":[operation]
    }))
    .unwrap();
    surface.validate().unwrap();
    let canonical = surface.canonical_operations().unwrap();
    assert_eq!(
        canonical[0]["environment"],
        json!(["CARGO_HOME", "RUSTUP_HOME"])
    );
    let mut reordered = surface.clone();
    reordered.operations[0].environment.reverse();
    assert_eq!(surface.node_body().unwrap(), reordered.node_body().unwrap());
    let mut without_environment = surface.clone();
    without_environment.operations[0].environment.clear();
    assert_ne!(
        surface.node_body().unwrap(),
        without_environment.node_body().unwrap(),
        "environment declarations participate in the canonical surface projection"
    );

    for environment in [
        json!([""]),
        json!(["1INVALID"]),
        json!(["INVALID-NAME"]),
        json!(["CARGO_HOME", "CARGO_HOME"]),
    ] {
        let mut invalid = serde_json::to_value(&surface).unwrap();
        invalid["operations"][0]["environment"] = environment;
        let invalid: loom::journey::InterfaceSurfaceDefinition =
            serde_json::from_value(invalid).unwrap();
        assert!(invalid.validate().is_err());
    }

    let mut unknown = serde_json::to_value(&surface).unwrap();
    unknown["operations"][0]["environment_values"] = json!({"CARGO_HOME":"secret"});
    assert!(serde_json::from_value::<loom::journey::InterfaceSurfaceDefinition>(unknown).is_err());

    let serialized = serde_json::to_string(&surface).unwrap();
    assert!(serialized.contains("CARGO_HOME"));
    assert!(!serialized.contains("/private/toolchain"));
}

#[test]
fn output_assertion_operator_schema_is_strict_and_round_trips() {
    let cases = [
        json!({"id":"ne","pointer":"/value","not_equals":false}),
        json!({"id":"exists","pointer":"/value","exists":true}),
        json!({"id":"contains","pointer":"/value","contains":{"lane":"build"}}),
        json!({"id":"matches","pointer":"/value","matches":"^build$"}),
    ];
    for case in cases {
        let assertion: loom::journey::OutputAssertion =
            serde_json::from_value(case.clone()).unwrap();
        assert_eq!(serde_json::to_value(assertion).unwrap(), case);
    }

    assert!(
        serde_json::from_value::<loom::journey::OutputAssertion>(json!({
            "id":"ambiguous",
            "pointer":"/value",
            "equals":"build",
            "not_equals":"surface"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<loom::journey::OutputAssertion>(json!({
            "id":"unknown",
            "pointer":"/value",
            "starts_with":"build"
        }))
        .is_err()
    );

    let invalid_surface = |assertion: Value| {
        serde_json::from_value::<loom::journey::InterfaceSurfaceDefinition>(json!({
            "id":"status-cli",
            "title":"Status CLI",
            "identity":"loom status",
            "codefile":"src/commands/status_cmd.rs",
            "locator":"status",
            "operations":[{
                "id":"status",
                "summary":"Read status",
                "argv":["loom","status","--json"],
                "read_only":true,
                "output":{"format":"json","assertions":[assertion]}
            }]
        }))
        .unwrap()
        .validate()
        .unwrap_err()
        .to_string()
    };
    assert!(invalid_surface(json!({
        "id":"exists-with-type",
        "pointer":"/value",
        "exists":true,
        "type":"string"
    }))
    .contains("exists operator"));
    assert!(invalid_surface(json!({
        "id":"bad-regex",
        "pointer":"/value",
        "matches":"["
    }))
    .contains("invalid matches regex"));
    assert!(invalid_surface(json!({
        "id":"bad-contains-type",
        "pointer":"/value",
        "type":"integer",
        "contains":1
    }))
    .contains("contains operator"));
}

#[test]
fn journey_add_creates_only_the_root_node_and_is_semantically_idempotent() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("semantic"));
    let path = write_journey(&tmp, "checkout.happy", "Checkout succeeds");
    let path_text = path.to_str().unwrap();

    let first = builder_json(&tmp, &["journey", "add", path_text, "--json"]);
    assert_eq!(first["added"], true, "{first}");
    let second = builder_json(&tmp, &["journey", "add", path_text, "--json"]);
    assert_eq!(second["added"], false, "{second}");
    assert_eq!(second["changed"], false, "{second}");

    let store = Store::open(tmp.path()).unwrap();
    let journeys = store
        .list_nodes(Some(NodeType::Journey), usize::MAX)
        .unwrap();
    assert_eq!(journeys.len(), 1);
    assert_eq!(journeys[0].name, "checkout.happy");
    assert_eq!(journeys[0].body["schema"], "loom.journey/v1");
    assert_eq!(
        store
            .list_nodes(Some(NodeType::Validation), usize::MAX)
            .unwrap()
            .len(),
        0,
        "journey add must not mint a proof Validation"
    );
    assert!(store.snapshot().unwrap().edges.is_empty());
}

#[test]
fn journey_updates_preserve_display_and_reorder_truth_then_invalidate_changed_steps_only() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("selective-drift"));
    let path = write_journey(&tmp, "checkout.happy", "Checkout succeeds");
    builder_json(&tmp, &["journey", "add", path.to_str().unwrap(), "--json"]);

    let (first_edge, second_edge, surface_edge) = {
        let store = Store::open(tmp.path()).unwrap();
        let journey = store
            .resolve_node("checkout.happy", Some(NodeType::Journey))
            .unwrap();
        let first = store
            .add_node(
                NodeType::Intent,
                "cart accepts an item",
                "the cart records the selected item",
                "planned",
                json!({}),
            )
            .unwrap();
        let second = store
            .add_node(
                NodeType::Intent,
                "order is submitted",
                "the order is accepted",
                "planned",
                json!({}),
            )
            .unwrap();
        let first_edge = store
            .add_edge(
                EdgeKind::Derives,
                &journey.id,
                &first.id,
                loom::model::TruthClass::Asserted,
            )
            .unwrap();
        let second_edge = store
            .add_edge(
                EdgeKind::Derives,
                &journey.id,
                &second.id,
                loom::model::TruthClass::Asserted,
            )
            .unwrap();
        for (edge, steps) in [
            (&first_edge, ["add-item"]),
            (&second_edge, ["submit-order"]),
        ] {
            store
                .set_facet(
                    &edge.id,
                    TargetKind::Edge,
                    "step_ids",
                    &serde_json::to_string(&steps).unwrap(),
                    loom::model::TruthClass::Asserted,
                )
                .unwrap();
        }
        let surface = store
            .add_node(
                NodeType::InterfaceSurface,
                "shop-cli",
                "test surface",
                "declared",
                json!({}),
            )
            .unwrap();
        let surface_edge = store
            .add_edge(
                EdgeKind::Surfaces,
                &journey.id,
                &surface.id,
                loom::model::TruthClass::Asserted,
            )
            .unwrap();
        store
            .set_facet(
                &surface_edge.id,
                TargetKind::Edge,
                "operation_bindings",
                r#"[{"step_id":"add-item","operation_id":"cart-add"},{"step_id":"submit-order","operation_id":"order-submit"}]"#,
                loom::model::TruthClass::Asserted,
            )
            .unwrap();
        (first_edge.id, second_edge.id, surface_edge.id)
    };

    let mut document = semantic_journey("checkout.happy", "Checkout renamed");
    std::fs::write(&path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let renamed = builder_json(&tmp, &["journey", "add", path.to_str().unwrap(), "--json"]);
    assert_eq!(renamed["changed"], false, "{renamed}");

    document["steps"].as_array_mut().unwrap().reverse();
    std::fs::write(&path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let reordered = builder_json(&tmp, &["journey", "add", path.to_str().unwrap(), "--json"]);
    assert_eq!(reordered["changed"], true, "{reordered}");
    assert_eq!(reordered["invalidated_projections"], 0, "{reordered}");
    {
        let store = Store::open(tmp.path()).unwrap();
        assert!(store.get_edge(&first_edge).unwrap().is_some());
        assert!(store.get_edge(&second_edge).unwrap().is_some());
        assert!(store.get_edge(&surface_edge).unwrap().is_some());
        assert_eq!(
            store
                .get_facet(&surface_edge, TargetKind::Edge, "operation_bindings")
                .unwrap()
                .unwrap(),
            r#"[{"step_id":"submit-order","operation_id":"order-submit"},{"step_id":"add-item","operation_id":"cart-add"}]"#
        );
    }

    document["steps"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|step| step["id"] == "submit-order")
        .unwrap()["action"] = json!("submits the order with reviewed payment details");
    std::fs::write(&path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let changed = builder_json(&tmp, &["journey", "add", path.to_str().unwrap(), "--json"]);
    assert_eq!(changed["invalidated_projections"], 2, "{changed}");
    let store = Store::open(tmp.path()).unwrap();
    assert!(store.get_edge(&first_edge).unwrap().is_some());
    assert!(store.get_edge(&second_edge).unwrap().is_none());
    assert!(store.get_edge(&surface_edge).unwrap().is_none());
}

#[test]
fn derive_packet_is_read_only_and_accept_is_human_gated_and_atomic() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("derive"));
    let path = write_journey(&tmp, "checkout.happy", "Checkout succeeds");
    builder_json(&tmp, &["journey", "add", path.to_str().unwrap(), "--json"]);
    let store = Store::open(tmp.path()).unwrap();
    let before = store.snapshot().unwrap();
    let facts_before = store.all_facts().unwrap().len();
    drop(store);
    let journal_before = loom::journal::read(tmp.path()).unwrap().len();
    let packet = builder_json(&tmp, &["journey", "derive", "checkout.happy", "--json"]);
    assert_eq!(packet["mode"], "derive");
    assert!(
        packet.get("candidate_state").is_none(),
        "the existing derive response must not change unless --candidate-json is present"
    );
    assert_eq!(
        packet["uncovered_step_ids"],
        json!(["add-item", "submit-order"])
    );
    assert_eq!(
        packet["accept_command"],
        "loom journey derive-accept checkout.happy --manifest <manifest.json> --human-decision <exact-answer>"
    );
    assert_eq!(
        packet["human_gate"]["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| option["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["accept", "revise", "defer"]
    );
    assert!(packet["human_gate"]["recommendation"]
        .as_str()
        .unwrap()
        .contains("Recommend acceptance only"));
    assert!(packet["human_gate"]["after_answer"]
        .as_str()
        .unwrap()
        .contains("Missing human authority is a pause"));
    assert!(packet["next_action"]
        .as_str()
        .unwrap()
        .contains("wait for the human's exact answer"));
    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        store.snapshot().unwrap(),
        before,
        "derivation packet inspection must not mutate the graph"
    );
    assert!(
        store
            .list_nodes(Some(NodeType::Intent), usize::MAX)
            .unwrap()
            .is_empty(),
        "an unapproved packet creates no technical Intent"
    );
    let journey = store
        .resolve_node("checkout.happy", Some(NodeType::Journey))
        .unwrap();
    assert!(
        loom::workitem::queue_items(&store, loom::lane::Lane::Build)
            .unwrap()
            .is_empty(),
        "technical work in an unapproved derivation packet must not enter the Build queue"
    );
    let readiness = loom::completeness::journey_readiness(&store, &journey).unwrap();
    assert!(
        !readiness.derived,
        "an unapproved derivation packet must not satisfy Journey derivation readiness"
    );
    assert!(
        !readiness.derivations_ratified,
        "an unapproved derivation packet must not satisfy ratification readiness"
    );
    assert!(
        !readiness.implemented,
        "an unapproved derivation packet must not satisfy implementation readiness"
    );
    assert!(
        readiness.derived_intent_ids.is_empty(),
        "an unapproved derivation packet must contribute no readiness Intent targets"
    );
    assert!(
        store
            .edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)
            .unwrap()
            .is_empty(),
        "an unapproved packet creates no current Derives projection"
    );
    assert!(
        store
            .list_nodes(Some(NodeType::Proposal), usize::MAX)
            .unwrap()
            .iter()
            .all(|proposal| proposal.status != "adopted"),
        "an unapproved packet creates no adopted Proposal"
    );
    assert!(
        store
            .all_facts()
            .unwrap()
            .iter()
            .all(|fact| fact.claim != loom::model::Claim::Ratification),
        "an unapproved packet asserts no ratification"
    );
    assert_eq!(store.all_facts().unwrap().len(), facts_before);
    drop(store);
    let journal = loom::journal::read(tmp.path()).unwrap();
    assert_eq!(journal.len(), journal_before);
    assert!(
        journal
            .iter()
            .all(|entry| entry.event != "journey_derivation_accept"),
        "an unapproved packet records no asserted approval"
    );

    let manifest_document = derivation_manifest(packet["semantic_hash"].clone());
    let manifest = tmp.path().join("derivation.json");
    std::fs::write(
        &manifest,
        serde_json::to_vec_pretty(&manifest_document).unwrap(),
    )
    .unwrap();

    let store = Store::open(tmp.path()).unwrap();
    let export_path = loom::travel::export_to_file(&store).unwrap();
    let candidate_before = store.snapshot().unwrap();
    let candidate_facts_before = store.all_facts().unwrap();
    drop(store);
    let candidate_journal_before = loom::journal::read(tmp.path()).unwrap();
    let candidate_export_before = std::fs::read(&export_path).unwrap();

    let candidate = builder_json(
        &tmp,
        &[
            "journey",
            "derive",
            "checkout.happy",
            "--candidate-json",
            manifest.to_str().unwrap(),
            "--json",
        ],
    );
    let candidate_state = &candidate["candidate_state"];
    assert!(
        candidate_state["canonical_manifest_hash"]
            .as_str()
            .is_some_and(|hash| !hash.is_empty()),
        "{candidate_state}"
    );
    for field in [
        "matching_adopted_proposals",
        "candidate_intent_matches",
        "derives_edges",
        "ratification_facts",
        "build_queue_entries",
        "readiness_derived_candidate_ids",
    ] {
        assert_eq!(
            candidate_state[field],
            json!([]),
            "unapproved candidate state field '{field}' must carry no authority: {candidate_state}"
        );
    }

    let inline = serde_json::to_string(&manifest_document).unwrap();
    let inline_candidate = builder_json(
        &tmp,
        &[
            "journey",
            "derive",
            "checkout.happy",
            "--candidate-json",
            &inline,
            "--json",
        ],
    );
    assert_eq!(
        inline_candidate["candidate_state"], *candidate_state,
        "inline JSON and a manifest path must project the same canonical candidate"
    );

    let mut unresolved_document = manifest_document.clone();
    unresolved_document["unresolved_question"] = json!({
        "id": "payment-ownership",
        "text": "Which subsystem owns payment settlement?"
    });
    let unresolved = serde_json::to_string(&unresolved_document).unwrap();
    let unresolved_candidate = builder_json(
        &tmp,
        &[
            "journey",
            "derive",
            "checkout.happy",
            "--candidate-json",
            &unresolved,
            "--json",
        ],
    );
    assert_eq!(
        unresolved_candidate["candidate_state"]["matching_adopted_proposals"],
        json!([]),
        "an unresolved candidate is inspectable but cannot acquire adopted authority"
    );

    let mut unknown_field_document = manifest_document.clone();
    unknown_field_document["invented_authority"] = json!(true);
    let unknown_field = serde_json::to_string(&unknown_field_document).unwrap();
    let rejected_unknown = builder_command(
        &tmp,
        &[
            "journey",
            "derive",
            "checkout.happy",
            "--candidate-json",
            &unknown_field,
            "--json",
        ],
    );
    assert!(!rejected_unknown.status.success());
    assert!(
        String::from_utf8_lossy(&rejected_unknown.stderr).contains("unknown field"),
        "{}",
        String::from_utf8_lossy(&rejected_unknown.stderr)
    );

    let mut stale_document = manifest_document.clone();
    stale_document["journey_hash"] = json!("stale-journey-hash");
    let stale = serde_json::to_string(&stale_document).unwrap();
    let rejected_stale = builder_command(
        &tmp,
        &[
            "journey",
            "derive",
            "checkout.happy",
            "--candidate-json",
            &stale,
            "--json",
        ],
    );
    assert!(!rejected_stale.status.success());
    assert!(
        String::from_utf8_lossy(&rejected_stale.stderr).contains("hash mismatch"),
        "{}",
        String::from_utf8_lossy(&rejected_stale.stderr)
    );

    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        store.snapshot().unwrap(),
        candidate_before,
        "candidate inspection and rejected candidates must leave the full graph unchanged"
    );
    assert_eq!(store.all_facts().unwrap(), candidate_facts_before);
    drop(store);
    assert_eq!(
        loom::journal::read(tmp.path()).unwrap(),
        candidate_journal_before,
        "candidate inspection must not append authority or audit events"
    );
    assert_eq!(
        std::fs::read(&export_path).unwrap(),
        candidate_export_before,
        "candidate inspection must not refresh or rewrite the committed export"
    );

    let store = Store::open(tmp.path()).unwrap();
    let denied_before = store.snapshot().unwrap();
    let denied_facts_before = store.all_facts().unwrap().len();
    drop(store);
    let denied_journal_before = loom::journal::read(tmp.path()).unwrap().len();
    let denied = builder_command(
        &tmp,
        &[
            "journey",
            "derive-accept",
            "checkout.happy",
            "--manifest",
            manifest.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(
        !denied.status.success(),
        "acceptance without human decision passed"
    );
    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        store.snapshot().unwrap(),
        denied_before,
        "denied acceptance must leave the complete graph unchanged"
    );
    assert_eq!(store.all_facts().unwrap().len(), denied_facts_before);
    assert_eq!(
        store
            .list_nodes(Some(NodeType::Intent), usize::MAX)
            .unwrap()
            .len(),
        0
    );
    drop(store);
    assert_eq!(
        loom::journal::read(tmp.path()).unwrap().len(),
        denied_journal_before,
        "denied acceptance must not record approval provenance"
    );

    let accepted = builder_json(
        &tmp,
        &[
            "journey",
            "derive-accept",
            "checkout.happy",
            "--manifest",
            manifest.to_str().unwrap(),
            "--human-decision",
            "I approve these exact technical intents for this Journey.",
            "--json",
        ],
    );
    assert_eq!(accepted["accepted"], true, "{accepted}");
    assert_eq!(accepted["created"], 2, "{accepted}");
    assert_eq!(accepted["relationships_created"], 1, "{accepted}");
    assert_eq!(accepted["proposal"]["status"], "adopted", "{accepted}");
    assert_eq!(
        accepted["manifest_hash"], candidate_state["canonical_manifest_hash"],
        "inspection and acceptance must use the same canonical manifest hash"
    );

    let store = Store::open(tmp.path()).unwrap();
    let journey = store
        .resolve_node("checkout.happy", Some(NodeType::Journey))
        .unwrap();
    let edges = store
        .edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)
        .unwrap();
    assert_eq!(edges.len(), 2);
    for edge in &edges {
        assert_eq!(
            store
                .get_facet(&edge.id, TargetKind::Edge, "journey_hash")
                .unwrap(),
            Some(packet["semantic_hash"].as_str().unwrap().to_string())
        );
        assert!(store
            .get_facet(&edge.id, TargetKind::Edge, "rationale")
            .unwrap()
            .is_some());
        assert_eq!(
            store
                .get_facet(&edge.id, TargetKind::Edge, "proposal_id")
                .unwrap()
                .as_deref(),
            accepted["proposal"]["id"].as_str()
        );
        assert_eq!(
            store
                .get_facet(&edge.id, TargetKind::Edge, "manifest_hash")
                .unwrap()
                .as_deref(),
            accepted["manifest_hash"].as_str()
        );
        assert_eq!(store.ratification(&edge.to_id).unwrap(), "ratified");
    }
    let relationship = store
        .list_edges(Some(EdgeKind::Requires), usize::MAX)
        .unwrap();
    assert_eq!(relationship.len(), 1);
    assert!(store
        .get_facet(
            &relationship[0].id,
            TargetKind::Edge,
            "journey_derivation_bindings"
        )
        .unwrap()
        .unwrap()
        .contains(accepted["proposal"]["id"].as_str().unwrap()));
    drop(store);

    let accepted_store = Store::open(tmp.path()).unwrap();
    let accepted_before_candidate = accepted_store.snapshot().unwrap();
    let accepted_facts_before_candidate = accepted_store.all_facts().unwrap();
    drop(accepted_store);
    let accepted_journal_before_candidate = loom::journal::read(tmp.path()).unwrap();
    let accepted_export_before_candidate = std::fs::read(&export_path).unwrap();
    let accepted_candidate = builder_json(
        &tmp,
        &[
            "journey",
            "derive",
            "checkout.happy",
            "--candidate-json",
            manifest.to_str().unwrap(),
            "--json",
        ],
    );
    let accepted_state = &accepted_candidate["candidate_state"];
    assert_eq!(
        accepted_state["matching_adopted_proposals"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        accepted_state["candidate_intent_matches"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(accepted_state["derives_edges"].as_array().unwrap().len(), 2);
    assert_eq!(
        accepted_state["ratification_facts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        accepted_state["build_queue_entries"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        accepted_state["readiness_derived_candidate_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(accepted_state["ratification_facts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|fact| fact["state"] == "ratified"));
    let mut accepted_ids: Vec<_> = accepted["intents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|intent| intent["id"].as_str().unwrap().to_string())
        .collect();
    let mut projected_ids: Vec<_> = accepted_state["candidate_intent_matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["intent"]["id"].as_str().unwrap().to_string())
        .collect();
    accepted_ids.sort();
    projected_ids.sort();
    assert_eq!(projected_ids, accepted_ids);
    let plain_after_accept = builder_json(&tmp, &["journey", "derive", "checkout.happy", "--json"]);
    assert!(plain_after_accept.get("candidate_state").is_none());
    let accepted_store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        accepted_store.snapshot().unwrap(),
        accepted_before_candidate
    );
    assert_eq!(
        accepted_store.all_facts().unwrap(),
        accepted_facts_before_candidate
    );
    drop(accepted_store);
    assert_eq!(
        loom::journal::read(tmp.path()).unwrap(),
        accepted_journal_before_candidate
    );
    assert_eq!(
        std::fs::read(&export_path).unwrap(),
        accepted_export_before_candidate
    );

    let before_repeat = Store::open(tmp.path()).unwrap().snapshot().unwrap();
    let journal_before = loom::journal::read(tmp.path()).unwrap().len();
    let repeated = builder_json(
        &tmp,
        &[
            "journey",
            "derive-accept",
            "checkout.happy",
            "--manifest",
            manifest.to_str().unwrap(),
            "--human-decision",
            "I approve these exact technical intents for this Journey.",
            "--json",
        ],
    );
    assert_eq!(repeated["idempotent"], true, "{repeated}");
    assert_eq!(repeated["proposal"]["id"], accepted["proposal"]["id"]);
    assert_eq!(
        loom::journal::read(tmp.path()).unwrap().len(),
        journal_before
    );
    assert_eq!(
        Store::open(tmp.path()).unwrap().snapshot().unwrap(),
        before_repeat
    );
}

#[test]
fn one_journey_step_may_project_to_two_distinct_intents() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("overlapping-derivation"));
    let mut authored = semantic_journey("checkout.single", "Single-step checkout");
    authored["steps"].as_array_mut().unwrap().truncate(1);
    let path = tmp.path().join("checkout.single.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&authored).unwrap()).unwrap();
    builder_json(&tmp, &["journey", "add", path.to_str().unwrap(), "--json"]);
    let packet = builder_json(&tmp, &["journey", "derive", "checkout.single", "--json"]);
    let manifest = tmp.path().join("overlapping.json");
    std::fs::write(
        &manifest,
        serde_json::to_vec_pretty(&json!({
            "schema":"loom.journey-derivation/v1",
            "journey_id":"checkout.single",
            "journey_hash":packet["semantic_hash"],
            "proposal_id":"single-step-two-intents",
            "proposal_rationale":"The one user action crosses two independently falsifiable technical boundaries.",
            "intents":[
                {
                    "id":"validate-cart-input",
                    "operation":"create",
                    "name":"cart input is validated",
                    "criterion":"An unavailable SKU is rejected before cart mutation",
                    "level":"feature",
                    "visibility":"internal",
                    "rationale":"Input validation is independently falsifiable at the shared authored step.",
                    "step_ids":["add-item"]
                },
                {
                    "id":"persist-cart-item",
                    "operation":"create",
                    "name":"a valid cart item is persisted",
                    "criterion":"A valid SKU and quantity are recorded in the cart",
                    "level":"feature",
                    "visibility":"internal",
                    "rationale":"Persistence is a distinct observable boundary reached by the same authored step.",
                    "step_ids":["add-item"]
                }
            ],
            "relationships":[],
            "unresolved_question":null
        }))
        .unwrap(),
    )
    .unwrap();
    let accepted = builder_json(
        &tmp,
        &[
            "journey",
            "derive-accept",
            "checkout.single",
            "--manifest",
            manifest.to_str().unwrap(),
            "--human-decision",
            "I approve both distinct technical behaviors for this one Journey step.",
            "--json",
        ],
    );
    assert_eq!(accepted["created"], 2, "{accepted}");
    let store = Store::open(tmp.path()).unwrap();
    let journey = store
        .resolve_node("checkout.single", Some(NodeType::Journey))
        .unwrap();
    assert_eq!(
        store
            .edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn one_new_derived_intent_gets_a_prior_singleton_batch_envelope() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("singleton-derivation-batch"));
    let mut authored = semantic_journey("checkout.singleton", "Singleton checkout");
    authored["steps"].as_array_mut().unwrap().truncate(1);
    let path = tmp.path().join("checkout.singleton.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&authored).unwrap()).unwrap();
    builder_json(&tmp, &["journey", "add", path.to_str().unwrap(), "--json"]);
    let packet = builder_json(&tmp, &["journey", "derive", "checkout.singleton", "--json"]);
    let manifest = tmp.path().join("singleton-derivation.json");
    std::fs::write(
        &manifest,
        serde_json::to_vec_pretty(&json!({
            "schema":"loom.journey-derivation/v1",
            "journey_id":"checkout.singleton",
            "journey_hash":packet["semantic_hash"],
            "proposal_id":"singleton-technical-projection",
            "proposal_rationale":"The authored step has one independently falsifiable technical behavior.",
            "intents":[{
                "id":"persist-cart-item",
                "operation":"create",
                "name":"a valid cart item is persisted",
                "criterion":"A valid SKU and quantity are recorded in the cart",
                "level":"feature",
                "visibility":"internal",
                "rationale":"Persistence is the one observable boundary projected from this authored step.",
                "step_ids":["add-item"]
            }],
            "relationships":[],
            "unresolved_question":null
        }))
        .unwrap(),
    )
    .unwrap();
    let accepted = builder_json(
        &tmp,
        &[
            "journey",
            "derive-accept",
            "checkout.singleton",
            "--manifest",
            manifest.to_str().unwrap(),
            "--human-decision",
            "I approve this exact singleton technical projection.",
            "--json",
        ],
    );
    assert_eq!(accepted["created"], 1, "{accepted}");

    let store = Store::open(tmp.path()).unwrap();
    let facts = store
        .all_facts()
        .unwrap()
        .into_iter()
        .filter(|fact| fact.claim == loom::model::Claim::Ratification)
        .collect::<Vec<_>>();
    let [fact] = facts.as_slice() else {
        panic!("expected one singleton ratification fact, got {facts:#?}");
    };
    assert_eq!(fact.decision_mode, loom::model::DecisionMode::Batch);
    assert!(!fact.batch_id.is_empty());

    let entries = loom::journal::read(tmp.path()).unwrap();
    let envelope_entry = entries
        .iter()
        .find(|entry| entry.id == fact.batch_id)
        .expect("ratification fact names its prior batch envelope");
    let envelope = loom::batch_auth::parse_entry(envelope_entry)
        .unwrap()
        .expect("batch_id resolves to a batch authorization envelope");
    assert_eq!(envelope.claim, loom::batch_auth::BatchClaim::Ratification);
    assert_eq!(envelope.subjects, vec![fact.subject_id.clone()]);
    assert!(envelope.covers_subjects(std::slice::from_ref(&fact.subject_id)));
    assert!(envelope.human_decision.is_some());
    let envelope_millis = loom::journal::stamp_millis(&envelope_entry.ts)
        .expect("batch envelope has a valid timestamp");
    let fact_millis = loom::journal::stamp_millis(&fact.asserted_at)
        .expect("ratification fact has a valid timestamp");
    assert!(
        envelope_millis <= fact_millis,
        "the singleton envelope must exist no later than its covered fact"
    );
}

#[test]
fn idempotent_derive_accept_reseals_when_local_envelope_was_voided() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("reseal-derivation-envelope"));
    let mut authored = semantic_journey("checkout.singleton", "Singleton checkout");
    authored["steps"].as_array_mut().unwrap().truncate(1);
    let path = tmp.path().join("checkout.singleton.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&authored).unwrap()).unwrap();
    builder_json(&tmp, &["journey", "add", path.to_str().unwrap(), "--json"]);
    let packet = builder_json(&tmp, &["journey", "derive", "checkout.singleton", "--json"]);
    let manifest = tmp.path().join("singleton-derivation.json");
    std::fs::write(
        &manifest,
        serde_json::to_vec_pretty(&json!({
            "schema":"loom.journey-derivation/v1",
            "journey_id":"checkout.singleton",
            "journey_hash":packet["semantic_hash"],
            "proposal_id":"singleton-technical-projection",
            "proposal_rationale":"The authored step has one independently falsifiable technical behavior.",
            "intents":[{
                "id":"persist-cart-item",
                "operation":"create",
                "name":"a valid cart item is persisted",
                "criterion":"A valid SKU and quantity are recorded in the cart",
                "level":"feature",
                "visibility":"internal",
                "rationale":"Persistence is the one observable boundary projected from this authored step.",
                "step_ids":["add-item"]
            }],
            "relationships":[],
            "unresolved_question":null
        }))
        .unwrap(),
    )
    .unwrap();
    let decision = "I approve this exact singleton technical projection.";
    let accepted = builder_json(
        &tmp,
        &[
            "journey",
            "derive-accept",
            "checkout.singleton",
            "--manifest",
            manifest.to_str().unwrap(),
            "--human-decision",
            decision,
            "--json",
        ],
    );
    assert_eq!(accepted["created"], 1, "{accepted}");
    let local_before = loom::batch_auth::load_envelopes(tmp.path())
        .unwrap()
        .into_iter()
        .filter(|(_, envelope)| envelope.command_id == "journey-derive-accept:1")
        .count();
    assert_eq!(local_before, 1);

    // Import keeps ratification facts but drops local envelope standing.
    // Replay must reseal, not no-op, without a new product decision.
    let journal_path = loom::journal::path(tmp.path());
    let rewritten = loom::journal::read(tmp.path())
        .unwrap()
        .into_iter()
        .map(|mut entry| {
            if entry.event == loom::batch_auth::EVENT {
                entry.origin = loom::journal::Origin::Imported;
            }
            serde_json::to_string(&entry).unwrap()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&journal_path, rewritten).unwrap();
    assert!(loom::batch_auth::load_envelopes(tmp.path())
        .unwrap()
        .is_empty());

    let repeated = builder_json(
        &tmp,
        &[
            "journey",
            "derive-accept",
            "checkout.singleton",
            "--manifest",
            manifest.to_str().unwrap(),
            "--human-decision",
            decision,
            "--json",
        ],
    );
    assert_eq!(repeated["idempotent"], true, "{repeated}");
    assert_eq!(repeated["created"], 0, "{repeated}");
    let local_after: Vec<_> = loom::batch_auth::load_envelopes(tmp.path())
        .unwrap()
        .into_iter()
        .map(|(_, envelope)| envelope)
        .filter(|envelope| envelope.command_id == "journey-derive-accept:1")
        .collect();
    assert_eq!(local_after.len(), 1, "{local_after:#?}");
    assert_eq!(
        local_after[0].claim,
        loom::batch_auth::BatchClaim::Ratification
    );
    assert_eq!(local_after[0].operation, "ratify");
}

#[test]
fn derivation_preflight_rolls_back_and_relationship_ownership_preserves_independent_edges() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("derivation-preflight"));
    let path = write_journey(&tmp, "checkout.happy", "Checkout succeeds");
    builder_json(&tmp, &["journey", "add", path.to_str().unwrap(), "--json"]);
    let packet = builder_json(&tmp, &["journey", "derive", "checkout.happy", "--json"]);
    assert_eq!(
        packet["manifest_contract"]["schema"],
        "loom.journey-derivation/v1"
    );
    assert_eq!(
        packet["manifest_contract"]["unresolved_question"],
        Value::Null
    );

    let (cart, order, unrelated, independent_edge) = {
        let store = Store::open(tmp.path()).unwrap();
        let make_intent = |name: &str, criterion: &str| {
            let node = store
                .add_node(NodeType::Intent, name, criterion, "planned", json!({}))
                .unwrap();
            store
                .set_facet(
                    &node.id,
                    TargetKind::Node,
                    "level",
                    "feature",
                    loom::model::TruthClass::Asserted,
                )
                .unwrap();
            store
                .set_facet(
                    &node.id,
                    TargetKind::Node,
                    "visibility",
                    "internal",
                    loom::model::TruthClass::Asserted,
                )
                .unwrap();
            node
        };
        let cart = make_intent("existing cart mutation", "the cart records one item");
        let order = make_intent("existing order submission", "one order is recorded");
        let unrelated = make_intent("unrelated behavior", "an unrelated condition holds");
        let edge = store
            .add_edge(
                EdgeKind::Requires,
                &order.id,
                &cart.id,
                loom::model::TruthClass::Asserted,
            )
            .unwrap();
        (cart, order, unrelated, edge)
    };

    let reuse_manifest = |hash: Value, proposal_id: &str| {
        let mut manifest = derivation_manifest(hash);
        manifest["proposal_id"] = json!(proposal_id);
        for (index, intent) in manifest["intents"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            intent["operation"] = json!("reuse");
            intent["intent_id"] = json!(if index == 0 { &cart.id } else { &order.id });
            intent.as_object_mut().unwrap().remove("name");
            intent.as_object_mut().unwrap().remove("criterion");
        }
        manifest
    };
    let base = reuse_manifest(packet["semantic_hash"].clone(), "checkout-reuse-projection");
    let manifest_path = tmp.path().join("reuse-derivation.json");
    let decision = "I approve this exact reuse projection and its declared relationship.";
    let assert_rejected_without_writes = |document: Value| {
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&document).unwrap(),
        )
        .unwrap();
        let before = Store::open(tmp.path()).unwrap().snapshot().unwrap();
        let journal_before = loom::journal::read(tmp.path()).unwrap().len();
        let output = builder_command(
            &tmp,
            &[
                "journey",
                "derive-accept",
                "checkout.happy",
                "--manifest",
                manifest_path.to_str().unwrap(),
                "--human-decision",
                decision,
                "--json",
            ],
        );
        assert!(
            !output.status.success(),
            "invalid manifest unexpectedly passed"
        );
        assert_eq!(Store::open(tmp.path()).unwrap().snapshot().unwrap(), before);
        assert_eq!(
            loom::journal::read(tmp.path()).unwrap().len(),
            journal_before
        );
    };

    let mut unresolved = base.clone();
    unresolved["unresolved_question"] =
        json!({"id":"choose-boundary","text":"Which subsystem owns submission?"});
    assert_rejected_without_writes(unresolved);

    let mut stale = base.clone();
    stale["journey_hash"] = json!("stale");
    assert_rejected_without_writes(stale);

    let mut missing_rationale = base.clone();
    missing_rationale["intents"][0]["rationale"] = json!("");
    assert_rejected_without_writes(missing_rationale);

    let mut duplicate_step = base.clone();
    duplicate_step["intents"][0]["step_ids"] = json!(["add-item", "add-item"]);
    assert_rejected_without_writes(duplicate_step);

    let mut duplicate_target = base.clone();
    duplicate_target["intents"][1]["intent_id"] = json!(cart.id);
    assert_rejected_without_writes(duplicate_target);

    let mut illegal_target = base.clone();
    illegal_target["intents"][1]["intent_id"] = json!(independent_edge.id);
    assert_rejected_without_writes(illegal_target);

    let mut unknown = base.clone();
    unknown["relationships"][0]["to"] = json!("missing-entry");
    assert_rejected_without_writes(unknown);

    let mut self_link = base.clone();
    self_link["relationships"][0]["to"] = json!("order-is-submitted");
    assert_rejected_without_writes(self_link);

    let mut duplicate_relation = base.clone();
    duplicate_relation["relationships"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id":"submission-requires-cart-again",
            "kind":"requires",
            "from":"order-is-submitted",
            "to":"cart-accepts-item",
            "rationale":"Duplicate relationship must be rejected."
        }));
    assert_rejected_without_writes(duplicate_relation);

    let mut cycle = base.clone();
    cycle["relationships"].as_array_mut().unwrap().push(json!({
        "id":"cart-requires-submission",
        "kind":"requires",
        "from":"cart-accepts-item",
        "to":"order-is-submitted",
        "rationale":"This inverse dependency would create a cycle."
    }));
    assert_rejected_without_writes(cycle);

    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&base).unwrap()).unwrap();
    let first = builder_json(
        &tmp,
        &[
            "journey",
            "derive-accept",
            "checkout.happy",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--human-decision",
            decision,
            "--json",
        ],
    );
    assert_eq!(first["created"], 0, "{first}");
    assert_eq!(first["relationships_created"], 0, "{first}");
    assert!(Store::open(tmp.path())
        .unwrap()
        .get_edge(&independent_edge.id)
        .unwrap()
        .is_some());

    let second_path = write_journey(&tmp, "checkout.returning", "Returning checkout");
    builder_json(
        &tmp,
        &["journey", "add", second_path.to_str().unwrap(), "--json"],
    );
    let second_packet = builder_json(&tmp, &["journey", "derive", "checkout.returning", "--json"]);
    let mut second_manifest = reuse_manifest(
        second_packet["semantic_hash"].clone(),
        "returning-reuse-projection",
    );
    second_manifest["journey_id"] = json!("checkout.returning");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&second_manifest).unwrap(),
    )
    .unwrap();
    let second = builder_json(
        &tmp,
        &[
            "journey",
            "derive-accept",
            "checkout.returning",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--human-decision",
            decision,
            "--json",
        ],
    );

    let mut replacement = base;
    replacement["proposal_id"] = json!("checkout-reuse-projection-v2");
    replacement["proposal_rationale"] = json!(
        "The reused intents remain exact; no relationship is required by the revised projection."
    );
    replacement["relationships"] = json!([]);
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&replacement).unwrap(),
    )
    .unwrap();
    builder_json(
        &tmp,
        &[
            "journey",
            "derive-accept",
            "checkout.happy",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--human-decision",
            decision,
            "--json",
        ],
    );
    let store = Store::open(tmp.path()).unwrap();
    assert!(store.get_edge(&independent_edge.id).unwrap().is_some());
    let bindings = store
        .get_facet(
            &independent_edge.id,
            TargetKind::Edge,
            "journey_derivation_bindings",
        )
        .unwrap()
        .unwrap();
    assert!(!bindings.contains(first["proposal"]["id"].as_str().unwrap()));
    assert!(bindings.contains(second["proposal"]["id"].as_str().unwrap()));
    assert_eq!(
        store
            .resolve_node(&unrelated.id, Some(NodeType::Intent))
            .unwrap()
            .id,
        unrelated.id
    );
}

#[test]
fn surface_accept_creates_json_operations_and_reuses_one_interface_surface() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("surface"));
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/shop_cli.rs"),
        "pub fn run_shop_cli() {}\n",
    )
    .unwrap();
    builder_json(&tmp, &["codefile", "add", "src/shop_cli.rs", "--json"]);
    let first_path = write_journey(&tmp, "checkout.happy", "Checkout succeeds");
    builder_json(
        &tmp,
        &["journey", "add", first_path.to_str().unwrap(), "--json"],
    );
    let first_packet = builder_json(&tmp, &["journey", "surface", "checkout.happy", "--json"]);
    assert_eq!(
        first_packet["accept_command"],
        "loom journey surface-accept checkout.happy --manifest <manifest.json>"
    );
    assert!(
        first_packet["manifest_contract"].get("setup").is_none()
            || first_packet["manifest_contract"]["setup"].is_null(),
        "minimal surface template must not emit a setup block: {}",
        first_packet["manifest_contract"]
    );

    let surface_definition = json!({
        "id":"shop-cli",
        "title":"Shopper CLI",
        "identity":"shop checkout",
        "codefile":"src/shop_cli.rs",
        "locator":"run_shop_cli",
        "operations":[
            {
                "id":"prepare-cart",
                "summary":"Prepare the isolated cart fixture",
                "argv":["shop", "fixture", "prepare"],
                "read_only":false,
                "arguments":[],
                "output":{
                    "format":"json",
                    "assertions":[{"id":"fixture-ready","pointer":"/ready","type":"boolean","equals":true}]
                }
            },
            {
                "id":"cart-add",
                "summary":"Add one item to the cart",
                "argv":["shop", "cart", "add"],
                "arguments":[{"id":"sku", "type":"string", "required":true, "flag":"--sku", "source":"inputs.sku"}],
                "output":{"format":"json","captures":[{"id":"cart-id","pointer":"/cart_id","type":"string"}]}
            },
            {
                "id":"order-submit",
                "summary":"Submit the current cart",
                "argv":["shop", "order", "submit"],
                "arguments":[],
                "output":{"format":"json","captures":[{"id":"order-id","pointer":"/order_id","type":"string"}]}
            }
        ]
    });
    let bindings = json!([
        {"step_id":"add-item", "operation_id":"cart-add"},
        {"step_id":"submit-order", "operation_id":"order-submit"}
    ]);
    let source_hash = loom::artifact::fingerprint("pub fn run_shop_cli() {}\n");
    let manifest = tmp.path().join("surface.json");
    std::fs::write(
        &manifest,
        serde_json::to_vec_pretty(&json!({
            "schema":"loom.journey.surface/v1",
            "journey_id":"checkout.happy",
            "journey_hash":first_packet["semantic_hash"],
            "surface":surface_definition,
            "setup":{
                "graph":"local_snapshot",
                "git":{
                    "mode":"isolated_snapshot",
                    "dirty_paths":["src/shop_cli.rs"]
                },
                "before_steps":{
                    "submit-order":[{
                        "path":"src/shop_cli.rs",
                        "expected_hash":source_hash,
                        "template":"// {{ inputs.sku }}\npub fn run_shop_cli() {}\n"
                    }]
                },
                "operations":["prepare-cart"]
            },
            "bindings":bindings
        }))
        .unwrap(),
    )
    .unwrap();
    let accepted = builder_json(
        &tmp,
        &[
            "journey",
            "surface-accept",
            "checkout.happy",
            "--manifest",
            manifest.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(accepted["surface_created"], true, "{accepted}");
    assert_eq!(accepted["setup"]["graph"], "local_snapshot");
    assert_eq!(accepted["setup"]["git"]["mode"], "isolated_snapshot");
    assert_eq!(
        accepted["setup"]["git"]["dirty_paths"],
        json!(["src/shop_cli.rs"])
    );
    assert_eq!(accepted["setup"]["operations"], json!(["prepare-cart"]));
    assert_eq!(
        accepted["setup"]["before_steps"]["submit-order"][0]["path"],
        "src/shop_cli.rs"
    );

    // A second Journey with the same semantic step ids can reuse the exact
    // same InterfaceSurface definition; only its Surfaces edge is new.
    let second_path = write_journey(&tmp, "checkout.returning", "Returning checkout");
    builder_json(
        &tmp,
        &["journey", "add", second_path.to_str().unwrap(), "--json"],
    );
    let second_packet = builder_json(
        &tmp,
        &["journey", "surface", "checkout.returning", "--json"],
    );
    let second_manifest = tmp.path().join("surface-second.json");
    let mut second: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    second["journey_id"] = json!("checkout.returning");
    second["journey_hash"] = second_packet["semantic_hash"].clone();
    std::fs::write(
        &second_manifest,
        serde_json::to_vec_pretty(&second).unwrap(),
    )
    .unwrap();
    let reused = builder_json(
        &tmp,
        &[
            "journey",
            "surface-accept",
            "checkout.returning",
            "--manifest",
            second_manifest.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(reused["surface_created"], false, "{reused}");

    let store = Store::open(tmp.path()).unwrap();
    let surfaces = store
        .list_nodes(Some(NodeType::InterfaceSurface), usize::MAX)
        .unwrap();
    assert_eq!(surfaces.len(), 1);
    assert_eq!(surfaces[0].body["kind"], "cli");
    assert_eq!(
        surfaces[0].body["operations"][0]["output"]["format"],
        "json"
    );
    let edges = store
        .edges_with(Some(EdgeKind::Surfaces), None, Some(&surfaces[0].id))
        .unwrap();
    assert_eq!(edges.len(), 2, "both Journeys reuse one InterfaceSurface");
    let setup: Value = serde_json::from_str(
        &store
            .get_facet(&edges[0].id, TargetKind::Edge, "setup")
            .unwrap()
            .expect("accepted setup facet"),
    )
    .unwrap();
    assert_eq!(setup["graph"], "local_snapshot");
    assert_eq!(setup["git"]["mode"], "isolated_snapshot");
    assert_eq!(setup["git"]["dirty_paths"], json!(["src/shop_cli.rs"]));
    assert_eq!(setup["operations"], json!(["prepare-cart"]));
    assert_eq!(
        setup["before_steps"]["submit-order"][0]["expected_hash"],
        source_hash
    );
    let journey = store
        .resolve_node("checkout.happy", Some(NodeType::Journey))
        .unwrap();
    let first_hash = loom::journey::surface_projection_hash(&store, &journey)
        .unwrap()
        .expect("accepted projection has a hash");
    let second_hash = loom::journey::surface_projection_hash(&store, &journey)
        .unwrap()
        .expect("accepted projection remains hashable");
    assert_eq!(first_hash, second_hash, "projection hash is deterministic");
    drop(store);

    let without_setup = tmp.path().join("surface-without-setup.json");
    let mut unprepared: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    unprepared.as_object_mut().unwrap().remove("setup");
    std::fs::write(
        &without_setup,
        serde_json::to_vec_pretty(&unprepared).unwrap(),
    )
    .unwrap();
    builder_json(
        &tmp,
        &[
            "journey",
            "surface-accept",
            "checkout.happy",
            "--manifest",
            without_setup.to_str().unwrap(),
            "--json",
        ],
    );
    let store = Store::open(tmp.path()).unwrap();
    let journey = store
        .resolve_node("checkout.happy", Some(NodeType::Journey))
        .unwrap();
    let unprepared_hash = loom::journey::surface_projection_hash(&store, &journey)
        .unwrap()
        .unwrap();
    assert_ne!(
        first_hash, unprepared_hash,
        "setup participates in surface hash"
    );
    drop(store);
    builder_json(
        &tmp,
        &[
            "journey",
            "surface-accept",
            "checkout.happy",
            "--manifest",
            manifest.to_str().unwrap(),
            "--json",
        ],
    );
    let store = Store::open(tmp.path()).unwrap();
    let journey = store
        .resolve_node("checkout.happy", Some(NodeType::Journey))
        .unwrap();
    assert_eq!(
        loom::journey::surface_projection_hash(&store, &journey)
            .unwrap()
            .unwrap(),
        first_hash,
        "identical setup restores the deterministic projection hash"
    );
    drop(store);

    let accepted_manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    let mut rejected = Vec::new();
    for (label, dirty_paths) in [
        ("empty", json!([])),
        ("duplicate", json!(["src/shop_cli.rs", "src/shop_cli.rs"])),
        ("traversal", json!(["../src/shop_cli.rs"])),
        ("reserved", json!([".loom/graph.sqlite"])),
        ("unregistered", json!(["src/not_registered.rs"])),
    ] {
        let mut value = accepted_manifest.clone();
        value["setup"]["git"]["dirty_paths"] = dirty_paths;
        rejected.push((label, value));
    }
    let mut missing_graph = accepted_manifest.clone();
    missing_graph["setup"]
        .as_object_mut()
        .unwrap()
        .remove("graph");
    rejected.push(("missing-graph", missing_graph));
    let mut unknown_git_field = accepted_manifest;
    unknown_git_field["setup"]["git"]["repository"] = json!("live");
    rejected.push(("unknown-git-field", unknown_git_field));

    let accepted_manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    for (label, mutate) in [
        ("before-traversal", "../src/shop_cli.rs"),
        ("before-reserved", ".git/config"),
        ("before-unregistered", "src/not_registered.rs"),
    ] {
        let mut value = accepted_manifest.clone();
        value["setup"]["before_steps"]["submit-order"][0]["path"] = json!(mutate);
        rejected.push((label, value));
    }
    let mut stale_hash = accepted_manifest.clone();
    stale_hash["setup"]["before_steps"]["submit-order"][0]["expected_hash"] = json!("stale");
    rejected.push(("before-bad-hash", stale_hash));
    let mut both_replacements = accepted_manifest.clone();
    both_replacements["setup"]["before_steps"]["submit-order"][0]["content"] = json!("replacement");
    rejected.push(("before-two-replacements", both_replacements));
    let mut no_replacement = accepted_manifest.clone();
    no_replacement["setup"]["before_steps"]["submit-order"][0]
        .as_object_mut()
        .unwrap()
        .remove("template");
    rejected.push(("before-no-replacement", no_replacement));
    let mut unknown_step = accepted_manifest.clone();
    let actions = unknown_step["setup"]["before_steps"]
        .as_object_mut()
        .unwrap()
        .remove("submit-order")
        .unwrap();
    unknown_step["setup"]["before_steps"]["future-step"] = actions;
    rejected.push(("before-unknown-step", unknown_step));
    let mut unknown_source = accepted_manifest.clone();
    unknown_source["setup"]["before_steps"]["submit-order"][0]["template"] =
        json!("{{ inputs.unknown }}");
    rejected.push(("before-unknown-source", unknown_source));
    let mut unknown_action_field = accepted_manifest;
    unknown_action_field["setup"]["before_steps"]["submit-order"][0]["mode"] = json!("live");
    rejected.push(("before-unknown-field", unknown_action_field));

    for (label, value) in rejected {
        let rejected_path = tmp.path().join(format!("surface-{label}.json"));
        std::fs::write(&rejected_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        let before = Store::open(tmp.path()).unwrap().snapshot().unwrap();
        let output = builder_command(
            &tmp,
            &[
                "journey",
                "surface-accept",
                "checkout.happy",
                "--manifest",
                rejected_path.to_str().unwrap(),
                "--json",
            ],
        );
        assert!(
            !output.status.success(),
            "invalid isolated Git setup '{label}' was accepted"
        );
        assert_eq!(
            Store::open(tmp.path()).unwrap().snapshot().unwrap(),
            before,
            "rejected isolated Git setup '{label}' mutated graph truth"
        );
    }
}

#[test]
fn surface_accept_persists_a_strict_nonauthoritative_human_gate_binding() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("human-gate-surface"));
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/human_gate_cli.rs"),
        "pub fn run_gate_cli() {}\n",
    )
    .unwrap();
    builder_json(
        &tmp,
        &["codefile", "add", "src/human_gate_cli.rs", "--json"],
    );
    let authored = tmp.path().join("journeys/human-gate.json");
    std::fs::create_dir_all(authored.parent().unwrap()).unwrap();
    std::fs::write(
        &authored,
        serde_json::to_vec_pretty(&json!({
            "schema":"loom.journey/v1",
            "id":"human-gate",
            "name":"Ask then record an exact human choice",
            "actor":"operator",
            "goal":"Keep recommendation separate from authority",
            "inputs":{},
            "preconditions":[],
            "steps":[
                {
                    "id":"present-decision",
                    "name":"Present decision",
                    "action":"present evidence, recommendation, and choices",
                    "expects":["the prompt remains a recommendation"],
                    "produces":{}
                },
                {
                    "id":"record-human-choice",
                    "name":"Record choice",
                    "action":"record the exact mediated answer",
                    "expects":["the human remains authority"],
                    "produces":{}
                }
            ],
            "profiles":{"proof":{"inputs":{},"workspace":{}}}
        }))
        .unwrap(),
    )
    .unwrap();
    builder_json(
        &tmp,
        &["journey", "add", authored.to_str().unwrap(), "--json"],
    );
    let packet = builder_json(&tmp, &["journey", "surface", "human-gate", "--json"]);
    let manifest_value = json!({
        "schema":"loom.journey.surface/v1",
        "journey_id":"human-gate",
        "journey_hash":packet["semantic_hash"],
        "surface":{
            "id":"human-gate-cli",
            "title":"Human gate CLI",
            "identity":"structured human gate presentation",
            "codefile":"src/human_gate_cli.rs",
            "locator":"run_gate_cli",
            "operations":[{
                "id":"present-decision-op",
                "summary":"Project a structured recommendation and choices",
                "argv":["loom","status","--json"],
                "read_only":true,
                "arguments":[],
                "output":{
                    "format":"json",
                    "assertions":[{"id":"gate-present","pointer":"/human_gate","exists":true}]
                }
            }]
        },
        "setup":{"graph":"local_snapshot","operations":[]},
        "bindings":[
            {"step_id":"present-decision","operation_id":"present-decision-op"},
            {
                "step_id":"record-human-choice",
                "human_decision":{"operation_id":"present-decision-op","pointer":"/human_gate"}
            }
        ]
    });
    let manifest = tmp.path().join("human-gate.surface.json");
    std::fs::write(
        &manifest,
        serde_json::to_vec_pretty(&manifest_value).unwrap(),
    )
    .unwrap();
    let accepted = builder_json(
        &tmp,
        &[
            "journey",
            "surface-accept",
            "human-gate",
            "--manifest",
            manifest.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(accepted["setup"]["graph"], "local_snapshot");
    assert_eq!(accepted["setup"]["operations"], json!([]));

    let store = Store::open(tmp.path()).unwrap();
    let journey = store
        .resolve_node("human-gate", Some(NodeType::Journey))
        .unwrap();
    let edge = store
        .edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)
        .unwrap()
        .pop()
        .unwrap();
    let persisted: Value = serde_json::from_str(
        &store
            .get_facet(&edge.id, TargetKind::Edge, "operation_bindings")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        persisted[1]["human_decision"],
        json!({"operation_id":"present-decision-op","pointer":"/human_gate"})
    );
    assert!(store
        .list_nodes(Some(NodeType::Validation), usize::MAX)
        .unwrap()
        .is_empty());
    let projection_hash = loom::journey::surface_projection_hash(&store, &journey)
        .unwrap()
        .unwrap();
    drop(store);

    builder_json(
        &tmp,
        &[
            "journey",
            "surface-accept",
            "human-gate",
            "--manifest",
            manifest.to_str().unwrap(),
            "--json",
        ],
    );
    let store = Store::open(tmp.path()).unwrap();
    let journey = store
        .resolve_node("human-gate", Some(NodeType::Journey))
        .unwrap();
    assert_eq!(
        loom::journey::surface_projection_hash(&store, &journey)
            .unwrap()
            .unwrap(),
        projection_hash
    );
    assert_eq!(
        store
            .edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)
            .unwrap()
            .len(),
        1
    );
    drop(store);

    let mut rejected = Vec::new();
    let mut no_snapshot = manifest_value.clone();
    no_snapshot.as_object_mut().unwrap().remove("setup");
    rejected.push(("no-local-snapshot", no_snapshot));
    let mut both_variants = manifest_value.clone();
    both_variants["bindings"][1]["operation_id"] = json!("present-decision-op");
    rejected.push(("both-binding-variants", both_variants));
    let mut non_prior = manifest_value.clone();
    non_prior["bindings"][1]["human_decision"]["operation_id"] = json!("future-operation");
    rejected.push(("non-prior-operation", non_prior));
    let mut malformed_pointer = manifest_value.clone();
    malformed_pointer["bindings"][1]["human_decision"]["pointer"] = json!("/~x");
    rejected.push(("malformed-pointer", malformed_pointer));
    let mut default_answer = manifest_value.clone();
    default_answer["bindings"][1]["human_decision"]["default"] = json!("keep");
    rejected.push(("default-answer", default_answer));
    let mut gate_argv = manifest_value;
    gate_argv["bindings"][1]["human_decision"]["argv"] = json!(["loom", "intent", "ratify"]);
    rejected.push(("gate-argv", gate_argv));

    for (label, value) in rejected {
        let path = tmp.path().join(format!("human-gate-{label}.json"));
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        let before = Store::open(tmp.path()).unwrap().snapshot().unwrap();
        let output = builder_command(
            &tmp,
            &[
                "journey",
                "surface-accept",
                "human-gate",
                "--manifest",
                path.to_str().unwrap(),
                "--json",
            ],
        );
        assert!(
            !output.status.success(),
            "invalid gate '{label}' was accepted"
        );
        assert_eq!(
            Store::open(tmp.path()).unwrap().snapshot().unwrap(),
            before,
            "rejected gate '{label}' mutated graph truth"
        );
    }
}

#[test]
fn surface_accept_preserves_a_strict_navigation_only_anchor_locator() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("anchor-surface"));
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/shop_cli.rs"),
        "// loom:anchor cli.shop.run\npub fn renamed_shop_entry() {}\n",
    )
    .unwrap();
    builder_json(&tmp, &["codefile", "add", "src/shop_cli.rs", "--json"]);
    let journey = write_journey(&tmp, "checkout.happy", "Checkout succeeds");
    builder_json(
        &tmp,
        &["journey", "add", journey.to_str().unwrap(), "--json"],
    );
    let packet = builder_json(&tmp, &["journey", "surface", "checkout.happy", "--json"]);
    let manifest = tmp.path().join("anchor-surface.json");
    std::fs::write(
        &manifest,
        serde_json::to_vec_pretty(&json!({
            "schema":"loom.journey.surface/v1",
            "journey_id":"checkout.happy",
            "journey_hash":packet["semantic_hash"],
            "surface":{
                "id":"anchored-shop-cli",
                "title":"Anchored Shop CLI",
                "identity":"shop checkout",
                "codefile":"src/shop_cli.rs",
                "locator":"anchor:cli.shop.run",
                "operations":[
                    {"id":"cart-add","summary":"Add item","argv":["shop","cart","add"],"output":{"format":"json","captures":[{"id":"cart-id","pointer":"/cart_id","type":"string"}]}},
                    {"id":"order-submit","summary":"Submit order","argv":["shop","order","submit"],"output":{"format":"json","captures":[{"id":"order-id","pointer":"/order_id","type":"string"}]}}
                ]
            },
            "bindings":[
                {"step_id":"add-item","operation_id":"cart-add"},
                {"step_id":"submit-order","operation_id":"order-submit"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    builder_json(
        &tmp,
        &[
            "journey",
            "surface-accept",
            "checkout.happy",
            "--manifest",
            manifest.to_str().unwrap(),
            "--json",
        ],
    );

    let store = Store::open(tmp.path()).unwrap();
    let surface = store
        .resolve_node("anchored-shop-cli", Some(NodeType::InterfaceSurface))
        .unwrap();
    let exposes = store
        .edges_with(Some(EdgeKind::Exposes), Some(&surface.id), None)
        .unwrap();
    assert_eq!(exposes.len(), 1);
    assert_eq!(
        store
            .get_facet(&exposes[0].id, TargetKind::Edge, "locator")
            .unwrap()
            .as_deref(),
        Some("anchor:cli.shop.run")
    );
    assert!(
        store
            .list_nodes(Some(NodeType::Validation), usize::MAX)
            .unwrap()
            .is_empty(),
        "accepting a navigation anchor must not create proof"
    );
}

#[test]
fn duplicate_anchor_surface_rejection_rolls_back_every_projection_write() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("anchor-surface-rollback"));
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    for file in ["src/shop_cli.rs", "src/shop_cli_copy.rs"] {
        std::fs::write(
            tmp.path().join(file),
            "// loom:anchor cli.shop.run\npub fn run_shop_cli() {}\n",
        )
        .unwrap();
        builder_json(&tmp, &["codefile", "add", file, "--json"]);
    }
    let journey = write_journey(&tmp, "checkout.happy", "Checkout succeeds");
    builder_json(
        &tmp,
        &["journey", "add", journey.to_str().unwrap(), "--json"],
    );
    let packet = builder_json(&tmp, &["journey", "surface", "checkout.happy", "--json"]);
    let manifest = tmp.path().join("duplicate-anchor-surface.json");
    std::fs::write(
        &manifest,
        serde_json::to_vec_pretty(&json!({
            "schema":"loom.journey.surface/v1",
            "journey_id":"checkout.happy",
            "journey_hash":packet["semantic_hash"],
            "surface":{
                "id":"ambiguous-shop-cli",
                "title":"Ambiguous Shop CLI",
                "identity":"shop checkout",
                "codefile":"src/shop_cli.rs",
                "locator":"anchor:cli.shop.run",
                "operations":[
                    {"id":"cart-add","summary":"Add item","argv":["shop","cart","add"],"output":{"format":"json","captures":[{"id":"cart-id","pointer":"/cart_id","type":"string"}]}},
                    {"id":"order-submit","summary":"Submit order","argv":["shop","order","submit"],"output":{"format":"json","captures":[{"id":"order-id","pointer":"/order_id","type":"string"}]}}
                ]
            },
            "bindings":[
                {"step_id":"add-item","operation_id":"cart-add"},
                {"step_id":"submit-order","operation_id":"order-submit"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let output = builder_command(
        &tmp,
        &[
            "journey",
            "surface-accept",
            "checkout.happy",
            "--manifest",
            manifest.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("duplicated"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let store = Store::open(tmp.path()).unwrap();
    assert!(store
        .list_nodes(Some(NodeType::InterfaceSurface), usize::MAX)
        .unwrap()
        .is_empty());
    assert!(store
        .edges_with(Some(EdgeKind::Surfaces), None, None)
        .unwrap()
        .is_empty());
    assert!(store
        .edges_with(Some(EdgeKind::Exposes), None, None)
        .unwrap()
        .is_empty());
}

#[test]
fn stale_or_conflicting_surface_manifest_leaves_no_partial_projection() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("surface-rollback"));
    let path = write_journey(&tmp, "checkout.happy", "Checkout succeeds");
    builder_json(&tmp, &["journey", "add", path.to_str().unwrap(), "--json"]);
    let manifest = tmp.path().join("bad-surface.json");
    std::fs::write(
        &manifest,
        serde_json::to_vec_pretty(&json!({
            "schema":"loom.journey.surface/v1",
            "journey_id":"checkout.happy",
            "journey_hash":"stale",
            "surface":{
                "id":"shop-cli",
                "title":"Shop CLI",
                "identity":"shop checkout",
                "codefile":"src/shop_cli.rs",
                "locator":"run_shop_cli",
                "operations":[{
                    "id":"unsafe",
                    "summary":"An unsafe shell operation",
                    "argv":["sh", "-c", "shop checkout"],
                    "output":{"format":"json"}
                }]
            },
            "bindings":[
                {"step_id":"add-item", "operation_id":"unsafe"},
                {"step_id":"submit-order", "operation_id":"unsafe"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let output = builder_command(
        &tmp,
        &[
            "journey",
            "surface-accept",
            "checkout.happy",
            "--manifest",
            manifest.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(!output.status.success());
    let store = Store::open(tmp.path()).unwrap();
    assert!(store
        .list_nodes(Some(NodeType::InterfaceSurface), usize::MAX)
        .unwrap()
        .is_empty());
    assert!(store
        .edges_with(Some(EdgeKind::Surfaces), None, None)
        .unwrap()
        .is_empty());
}
