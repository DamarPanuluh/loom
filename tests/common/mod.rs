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
            proof_kind: None,
            journey_id: None,
            repo_native_kind: None,
            artifact: None,
        },
    });
    call(Command::Validation {
        cmd: ValidationCmd::Run {
            key: proof_name.into(),
            all: false,
        },
    });
}

/// Put an existing validation into a genuinely-passing state: point it at a
/// trivial command and let loom run it.
///
/// For fixtures whose subject is drift detection or coverage, not proof
/// semantics. The command is irrelevant to what they assert; what matters is
/// that the passing state was EARNED by a run loom observed, because a
/// hand-written passing verdict is no longer a state the graph can hold.
#[allow(dead_code)]
pub fn observe_passing(store: &loom::store::Store, val_name: &str) {
    use loom::model::NodeType;
    let val = store
        .resolve_node(val_name, Some(NodeType::Validation))
        .unwrap_or_else(|e| panic!("resolve validation {val_name}: {e}"));
    let mut body = val.body.clone();
    body["command"] = serde_json::json!("echo proof-ok");
    // A journey proof's GRADE is derived from its spec, so a fixture claiming a
    // real journey proof has to have one. Written here rather than in every
    // caller: the alternative is fixtures that pass loom's runner and then
    // report S1, which is not what any of them mean.
    let is_journey = body.get("proof_kind").and_then(|k| k.as_str()) == Some("journey")
        || body.get("type").and_then(|k| k.as_str()) == Some("journey");
    if is_journey && body.get("journey").is_none() && body.get("journey_id").is_none() {
        let slug = val_name.replace(' ', "-");
        std::fs::create_dir_all(store.root().join("journeys")).unwrap();
        std::fs::write(
            store.root().join(format!("journeys/{slug}.yaml")),
            format!(
                concat!(
                    "journey: {}\n",
                    "steps:\n",
                    "  - name: run it\n",
                    "    intent: {}\n",
                    "    run: echo proof-ok\n",
                    "    expect:\n",
                    "      stdout_contains: [\"proof-ok\"]\n",
                ),
                slug, val_name
            ),
        )
        .unwrap();
        body["journey"] = serde_json::json!(slug);
    }
    store.set_node_body(&val.id, &body).unwrap();
    let val = store.get_node(&val.id).unwrap().unwrap();
    loom::commands::observe_validation(store, &val)
        .unwrap_or_else(|e| panic!("observe {val_name}: {e}"));
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
    use loom::model::{EdgeKind, TargetKind, TruthClass};

    // The behavior lives in a symbol...
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/checkout.rs"),
        "pub fn perform_checkout() -> &'static str {\n    \"ok\"\n}\n",
    )
    .unwrap();
    let cf = store
        .add_node(
            loom::model::NodeType::CodeFile,
            "src/checkout.rs",
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
            "fn perform_checkout",
            TruthClass::Asserted,
        )
        .unwrap();

    // ...and the proof's own file calls it.
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("tests/checkout_test.rs"),
        "pub fn exercises_checkout() {\n    let _ = perform_checkout();\n}\n",
    )
    .unwrap();
    let test_cf = store
        .add_node(
            loom::model::NodeType::CodeFile,
            "tests/checkout_test.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();

    // The spec asserts something about the OUTPUT, not just the exit code.
    std::fs::create_dir_all(root.join("journeys")).unwrap();
    std::fs::write(
        root.join(format!("journeys/{name}.yaml")),
        format!(
            "journey: {name}\nsteps:\n  - name: run it\n    intent: checkout\n    \
             run: echo checkout-ok\n    expect:\n      stdout_contains: [\"checkout-ok\"]\n"
        ),
    )
    .unwrap();

    // The test file attaches to the BEHAVIOR with the `verifies` role — that is
    // the proof's reach, and what the call witness reads.
    let v_edge = store
        .add_edge(
            EdgeKind::Implements,
            intent_id,
            &test_cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &v_edge.id,
            TargetKind::Edge,
            "role",
            "verifies",
            TruthClass::Asserted,
        )
        .unwrap();

    let validation = store
        .add_node(
            loom::model::NodeType::Validation,
            name,
            "",
            "not_run",
            serde_json::json!({
                "proof_kind": "journey",
                "type": "test",
                "command": "echo checkout-ok",
                "journey": name,
            }),
        )
        .unwrap();
    store
        .ensure_edge(EdgeKind::Validates, &validation.id, intent_id)
        .unwrap();
    let fresh = store.get_node(&validation.id).unwrap().unwrap();
    loom::commands::observe_validation(store, &fresh).unwrap();
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
        "pub fn exercises_behavior() {\n    let _ = perform_behavior();\n}\n",
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
    let v = store
        .add_edge(
            EdgeKind::Implements,
            intent_id,
            &test_cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &v.id,
            TargetKind::Edge,
            "role",
            "verifies",
            TruthClass::Asserted,
        )
        .unwrap();
    loom::sync::run(store, root).unwrap();
}
