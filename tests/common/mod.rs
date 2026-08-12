use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique temp dir that removes itself on drop.
pub struct Tmp(PathBuf);

impl Tmp {
    pub fn new() -> Tmp {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("loom-test-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    #[allow(dead_code)]
    pub fn write(&self, rel: &str, content: &str) {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Spawn the compiled Loom binary for integration fixtures.
#[allow(dead_code)]
pub fn loom_command() -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_loom"));
    command.env("LOOM_NON_INTERACTIVE", "1");
    command
}

/// Initialize a graph through the public CLI.
#[allow(dead_code)]
pub fn loom_init(root: &Path, name: Option<&str>) {
    let mut command = loom_command();
    command.arg("--graph").arg(root).arg("init").arg(root);
    if let Some(name) = name {
        command.arg("--name").arg(name);
    }
    let output = command.output().expect("spawn loom init");
    assert!(
        output.status.success(),
        "loom init failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Give an intent a REAL passing proof: register a trivial command and let loom
/// run it.
///
/// Fixtures used to hand-record a `passing` verdict on a `validates` edge. That
/// is precisely the move the evidence spine refuses — a proof is `verified` only
/// when loom watched it happen — so a fixture that fabricates one no longer
/// compiles a green graph. Going through the real command path means the test
/// graph is proven for the same reason a production graph would be.
#[allow(dead_code)]
pub fn prove(root: &Path, intent_name: &str, proof_name: &str) {
    use loom::cli::{Cli, Command, ValidationCmd};
    let call = |cmd: Command| {
        loom::commands::run(Cli {
            graph: Some(root.to_path_buf()),
            json: true,
            command: Some(cmd),
        })
        .unwrap_or_else(|e| panic!("fixture proof step failed: {e}"));
    };
    call(Command::Validation {
        cmd: ValidationCmd::Add {
            name: proof_name.into(),
            r#type: "test".into(),
            command: "true".into(),
            intent: intent_name.into(),
        },
    });
    call(Command::Validation {
        cmd: ValidationCmd::Run {
            key: proof_name.into(),
            all: false,
        },
    });
}

/// Register a CodeFile AND put a real file behind it.
///
/// A registered path with nothing on disk is a fiction: `evidence::stamp`
/// silently skips a `file:line` citation into it, so a verdict "citing" that
/// path anchors nothing. Fixtures did this for years and looked green. With the
/// grounding floor demanding `cited`, they correctly stop.
///
/// Only creates what is missing — a fixture that deliberately wrote content is
/// testing that content, and a helper must never clobber it.
#[allow(dead_code)]
pub fn codefile(store: &loom::store::Store, path: &str) -> loom::model::Node {
    let full = store.root().join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    if !full.exists() {
        let body: String = std::iter::once("pub fn behavior() {}\n".to_string())
            .chain((2..=60).map(|n| format!("// line {n}\n")))
            .collect();
        std::fs::write(&full, body).unwrap();
    }
    store
        .add_node(
            loom::model::NodeType::CodeFile,
            path,
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap()
}

/// Build a journey proof that genuinely EARNS S3, rather than declaring it.
///
/// Every conjunct is real, because the grade is derived from them: a spec on
/// disk asserting something about the output (S2), the intent grounded in a
/// symbol, and a proof file whose own symbol calls it, so the call closure
/// reaches the behavior (S3). Fixtures used to reach the top of the old scale
/// by passing `--proof-level L5`, which is exactly the move this replaces.
#[allow(dead_code)]
pub fn s3_journey_proof(
    store: &loom::store::Store,
    root: &std::path::Path,
    intent_id: &str,
    name: &str,
) -> loom::model::Node {
    s3_journey_proof_with_ratification(store, root, intent_id, name, true)
}

/// Build the same witnessed S3 topology while deliberately preserving an
/// unratified Intent, for divergence tests that exercise the human gate.
#[allow(dead_code)]
pub fn s3_journey_proof_unratified(
    store: &loom::store::Store,
    root: &std::path::Path,
    intent_id: &str,
    name: &str,
) -> loom::model::Node {
    s3_journey_proof_with_ratification(store, root, intent_id, name, false)
}

fn s3_journey_proof_with_ratification(
    store: &loom::store::Store,
    root: &std::path::Path,
    intent_id: &str,
    name: &str,
    ratify: bool,
) -> loom::model::Node {
    use loom::journey::{
        CliOperation, JourneySpec, OperationBinding, OperationOutput, OutputAssertion,
        OutputFormat, ValueType, JOURNEY_COMPILER_VERSION, JOURNEY_SCHEMA,
    };
    use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
    use std::collections::BTreeMap;

    let slug = name.replace(' ', "-");
    let behavior_path = format!("src/{slug}-behavior.rs");
    let cli_path = format!("src/{slug}-cli.rs");
    let artifact = format!("journeys/{slug}.yaml");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("journeys")).unwrap();
    std::fs::write(
        root.join(&behavior_path),
        "pub fn perform_checkout() -> &'static str { \"ok\" }\n",
    )
    .unwrap();
    std::fs::write(
        root.join(&cli_path),
        "pub fn run_checkout() -> &'static str { perform_checkout() }\n",
    )
    .unwrap();

    let spec: JourneySpec = serde_json::from_value(serde_json::json!({
        "schema": JOURNEY_SCHEMA,
        "id": slug,
        "name": name,
        "actor": "shopper",
        "goal": "Complete checkout",
        "inputs": {},
        "preconditions": [],
        "steps": [{"id":"checkout","name":"Checkout","action":"checks out","expects":[],"produces":{}}],
        "profiles":{"proof":{"inputs":{},"workspace":{}}}
    }))
    .unwrap();
    std::fs::write(
        root.join(&artifact),
        serde_norway::to_string(&spec).unwrap(),
    )
    .unwrap();
    let journey_hash = spec.semantic_hash().unwrap();
    let journey = store
        .add_node(
            NodeType::Journey,
            &slug,
            name,
            "authored",
            serde_json::json!({
                "schema": JOURNEY_SCHEMA,
                "stable_id": slug,
                "name": name,
                "actor": "shopper",
                "goal": "Complete checkout",
                "artifact": artifact,
                "semantic_hash": journey_hash,
                "input_ids": [],
                "preconditions": [],
                "step_ids": ["checkout"],
                "output_ids": [],
                "profile_ids": ["proof"]
            }),
        )
        .unwrap();

    let behavior = store
        .add_node(
            NodeType::CodeFile,
            &behavior_path,
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let realizes = store
        .ensure_edge(EdgeKind::Implements, intent_id, &behavior.id)
        .unwrap();
    store
        .set_facet(
            &realizes.id,
            TargetKind::Edge,
            "locator",
            "perform_checkout",
            TruthClass::Asserted,
        )
        .unwrap();
    if ratify && store.ratification(intent_id).unwrap() != "ratified" {
        store
            .ratify_intent(intent_id, "canonical Journey fixture", "test fixture")
            .unwrap();
    }
    let derives = store
        .ensure_edge(EdgeKind::Derives, &journey.id, intent_id)
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
            "[\"checkout\"]",
            TruthClass::Asserted,
        )
        .unwrap();

    let cli = store
        .add_node(NodeType::CodeFile, &cli_path, "", "", serde_json::json!({}))
        .unwrap();
    let cli_grounding = store
        .ensure_edge(EdgeKind::Implements, intent_id, &cli.id)
        .unwrap();
    store
        .set_facet(
            &cli_grounding.id,
            TargetKind::Edge,
            "locator",
            "run_checkout",
            TruthClass::Asserted,
        )
        .unwrap();
    let operation = CliOperation {
        id: "checkout-op".into(),
        summary: "Run checkout".into(),
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
            assertions: vec![OutputAssertion {
                id: "checkout-ok".into(),
                pointer: "/ok".into(),
                value_type: Some(ValueType::Boolean),
                equals: Some(serde_json::json!(true)),
                source: None,
            }],
            redact: Vec::new(),
        },
        exercises: Vec::new(),
    };
    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            &format!("{name} CLI"),
            "canonical Journey fixture CLI",
            "active",
            serde_json::json!({
                "schema":"loom.interface-surface/v1",
                "stable_id":format!("{slug}-cli"),
                "title":format!("{name} CLI"),
                "kind":"cli",
                "identity":slug,
                "codefile":cli_path,
                "locator":"run_checkout",
                "operations":[operation.clone()]
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
            "[{\"operation_id\":\"checkout-op\",\"step_id\":\"checkout\"}]",
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
            "run_checkout",
            TruthClass::Asserted,
        )
        .unwrap();

    let surface_hash = loom::journey::surface_projection_hash(store, &journey)
        .unwrap()
        .unwrap();
    let validation = store
        .add_node(
            NodeType::Validation,
            &format!("journey:{slug}:proof"),
            "compiled Journey proof",
            "not_run",
            serde_json::json!({
                "type":"journey",
                "command":format!("loom journey run {slug} --profile proof"),
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
        .ensure_edge(EdgeKind::Validates, &validation.id, intent_id)
        .unwrap();
    store
        .ensure_edge(EdgeKind::Calls, &validation.id, &surface.id)
        .unwrap();
    let exercises = store
        .ensure_edge(EdgeKind::Exercises, &validation.id, &cli.id)
        .unwrap();
    store
        .set_facet(
            &exercises.id,
            TargetKind::Edge,
            "locator",
            "run_checkout",
            TruthClass::Asserted,
        )
        .unwrap();

    let proof = loom::journey_runtime::compile(
        &spec,
        &surface_hash,
        "proof",
        vec![operation],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let report = loom::journey_runtime::execute(root, &spec, &proof, &BTreeMap::new());
    assert_eq!(report.status, loom::journey_runtime::RuntimeStatus::Passed);
    loom::journey::settle_compiled_validation(
        store,
        &validation.id,
        &report,
        &[behavior_path.clone(), cli_path.clone()],
    )
    .unwrap();
    for (edge, criterion, evidence) in [
        (
            &derives,
            "Journey derives this technical behavior",
            artifact.as_str(),
        ),
        (
            &surfaces,
            "Journey is exposed through this CLI",
            cli_path.as_str(),
        ),
        (
            &exposes,
            "CLI surface is implemented by this file",
            cli_path.as_str(),
        ),
        (
            &realizes,
            "behavior code realizes the intent",
            behavior_path.as_str(),
        ),
        (
            &cli_grounding,
            "CLI code realizes the behavior",
            cli_path.as_str(),
        ),
    ] {
        store
            .record_verdict(
                &edge.id,
                loom::model::InspectionStatus::Passing,
                criterion,
                evidence,
                1.0,
                "test",
            )
            .unwrap();
    }
    for kind in [EdgeKind::Calls, EdgeKind::Exercises] {
        for edge in store
            .edges_with(Some(kind), Some(&validation.id), None)
            .unwrap()
        {
            store
                .record_verdict(
                    &edge.id,
                    loom::model::InspectionStatus::Passing,
                    "compiled proof uses this exact surface",
                    &cli_path,
                    1.0,
                    "test",
                )
                .unwrap();
        }
    }
    loom::sync::run(store, root).unwrap();
    store.get_node(&validation.id).unwrap().unwrap()
}

/// Make an intent's proof reach S3: ground the behavior in a symbol, and give
/// it a verifying file whose own symbol calls that symbol.
///
/// Split out from `s3_journey_proof` because CLI-driven fixtures build the
/// validation through `loom validation add` and only need the call witness.
#[allow(dead_code)]
pub fn earn_call_witness(store: &loom::store::Store, root: &std::path::Path, intent_id: &str) {
    use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/behavior.rs"),
        "pub fn perform_behavior() -> &'static str {\n    \"ok\"\n}\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("tests/behavior_test.rs"),
        "#[test]\npub fn exercises_behavior() {\n    let _ = perform_behavior();\n}\n",
    )
    .unwrap();
    let cf = store
        .add_node(
            NodeType::CodeFile,
            "src/behavior.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let g = store
        .add_edge(
            EdgeKind::Implements,
            intent_id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &g.id,
            TargetKind::Edge,
            "locator",
            "fn perform_behavior",
            TruthClass::Asserted,
        )
        .unwrap();
    let test_cf = store
        .add_node(
            NodeType::CodeFile,
            "tests/behavior_test.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    // Attach this proof entry to each validation already registered for the
    // intent. The old helper attached the file intent-wide, which is precisely
    // the witness leak the production model now rejects.
    for validates in store
        .edges_with(Some(EdgeKind::Validates), None, Some(intent_id))
        .unwrap()
    {
        let exercises = store
            .ensure_edge(EdgeKind::Exercises, &validates.from_id, &test_cf.id)
            .unwrap();
        store
            .set_facet(
                &exercises.id,
                TargetKind::Edge,
                "locator",
                "exercises_behavior",
                TruthClass::Asserted,
            )
            .unwrap();
    }
    loom::sync::run(store, root).unwrap();
}

/// Register a proof that reaches S2 — loom ran it AND it asserts something
/// about the output.
///
/// `prove_intent` with `true` lands at S1: a command exited zero, which says
/// nothing about the behavior. `proven` does not accept that, so any fixture
/// asserting a CLEAN graph needs a proof that actually establishes something.
/// The alternative is fixtures that are green for the reason this project
/// exists to reject.
#[allow(dead_code)]
pub fn prove_s2(store: &loom::store::Store, root: &std::path::Path, intent_id: &str, slug: &str) {
    use loom::model::{EdgeKind, NodeType};
    let _ = root;
    let val = store
        .add_node(
            NodeType::Validation,
            &format!("{slug} proof"),
            "",
            "not_run",
            serde_json::json!({
                "type": "test",
                "command": "printf 'test result: ok. 1 passed; 0 failed\\n'",
            }),
        )
        .unwrap();
    store
        .ensure_edge(EdgeKind::Validates, &val.id, intent_id)
        .unwrap();
    let fresh = store.get_node(&val.id).unwrap().unwrap();
    loom::commands::observe_validation(store, &fresh).unwrap();
}
