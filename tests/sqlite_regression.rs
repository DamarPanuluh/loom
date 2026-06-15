use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

static SQLITE_TEST_LOCK: Mutex<()> = Mutex::new(());

fn sqlite_test_lock() -> MutexGuard<'static, ()> {
    SQLITE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

struct ScratchGraph {
    root: PathBuf,
}

impl ScratchGraph {
    fn new(prefix: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let root = env::temp_dir().join(format!("loom-{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).expect("create scratch graph directory");

        let export = Path::new(env!("CARGO_MANIFEST_DIR")).join("loom.graph.json");
        fs::copy(export, root.join("loom.graph.json")).expect("copy committed loom export");

        Self { root }
    }
}

impl Drop for ScratchGraph {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn loom_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_loom"))
}

fn run_json(cwd: &Path, args: &[&str]) -> Value {
    run_json_as(cwd, args, "llm:validator")
}

fn run_json_as(cwd: &Path, args: &[&str], agent: &str) -> Value {
    let output = Command::new(loom_bin())
        .args(args)
        .current_dir(cwd)
        .env("LOOM_AGENT", agent)
        .env_remove("LOOM_GRAPH")
        .output()
        .unwrap_or_else(|err| panic!("failed to run loom {args:?}: {err}"));

    if !output.status.success() {
        panic!(
            "loom {:?} failed with {}\nstdout:\n{}\nstderr:\n{}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "loom {:?} emitted invalid JSON: {err}\nstdout:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn setup_imported_graph(prefix: &str) -> ScratchGraph {
    let graph = ScratchGraph::new(prefix);
    run_json(&graph.root, &["init", ".", "--json"]);
    run_json(&graph.root, &["import", "loom.graph.json", "--json"]);
    graph
}

fn write_scratch_file(root: &Path, path: &str, contents: &str) {
    let file = root.join(path);
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).expect("create scratch file parent");
    }
    fs::write(file, contents).expect("write scratch file");
}

fn assert_status_ok(value: &Value) {
    assert!(
        value.is_object(),
        "command returned non-object JSON: {value}"
    );
}

#[test]
fn sqlite_imported_export_read_surface() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-read-surface");

    let status = run_json(&graph.root, &["status", "--json"]);
    assert!(status["total_intents"].as_i64().unwrap_or_default() > 0);

    let doctor = run_json(&graph.root, &["doctor", "--json"]);
    assert_eq!(
        doctor["healthy"], true,
        "doctor should be healthy: {doctor}"
    );

    for args in [
        vec!["report", "--json"],
        vec!["next", "--all", "--json"],
        vec!["find", "sqlite", "--limit", "10", "--json"],
        vec!["door", "sqlite storage", "--limit", "10", "--json"],
        vec!["coverage", "--json"],
        vec!["smells", "--limit", "10", "--json"],
        vec!["export", "-", "--json"],
    ] {
        let value = run_json(&graph.root, &args);
        assert!(value.is_object(), "loom {args:?} returned non-object JSON");
    }
}

#[test]
fn sqlite_audit_summary_surfaces_stay_bounded() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-audit-summary");

    let smells = run_json(&graph.root, &["smells", "--summary", "--json"]);
    assert_eq!(smells["summary"], true);
    assert!(
        smells.get("open_by_kind").is_some(),
        "summary keeps smell counts by kind: {smells}"
    );
    assert!(
        smells.get("smells").is_none(),
        "summary must not dump full smell evidence bodies: {smells}"
    );
    assert!(
        smells.get("adjudicated").is_none(),
        "summary must not dump adjudication bodies: {smells}"
    );

    let coverage = run_json(&graph.root, &["coverage", "--summary", "--json"]);
    assert_eq!(coverage["summary"], true);
    assert!(
        coverage.get("symbol_accountability").is_some(),
        "summary keeps actionable coverage counts: {coverage}"
    );
    assert!(
        coverage.get("raw_actionable_symbol_gaps").is_none(),
        "summary must not dump raw symbol-gap archives: {coverage}"
    );
    assert!(
        coverage.get("adjudicated_symbol_gaps").is_none(),
        "summary must not dump adjudicated symbol-gap archives: {coverage}"
    );
}

#[test]
fn sqlite_primary_mutation_surface_on_fresh_graph() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-mutation-surface");
    run_json(&graph.root, &["init", ".", "--json"]);

    let parent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "checkout flow",
            "--description",
            "customer can submit a cart and receive an order confirmation",
            "--level",
            "feature",
            "--lifecycle",
            "planned",
            "--json",
        ],
        "llm:builder",
    );
    let child = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "checkout validation",
            "--description",
            "checkout rejects invalid carts before creating an order",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--json",
        ],
        "llm:builder",
    );
    let parent_id = parent["id"].as_str().expect("parent id");
    let child_id = child["id"].as_str().expect("child id");

    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "edge",
            "hierarchy",
            parent_id,
            child_id,
            "--notes",
            "checkout validation is a child behavior of checkout flow",
            "--json",
        ],
        "llm:builder",
    ));

    write_scratch_file(
        &graph.root,
        "src/checkout.rs",
        "pub fn validate_checkout() -> bool {\n    true\n}\n",
    );
    assert_status_ok(&run_json_as(
        &graph.root,
        &["codefile", "add", "src/checkout.rs", "--json"],
        "llm:builder",
    ));
    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "edge",
            "implement",
            child_id,
            "src/checkout.rs",
            "--locator",
            "fn validate_checkout",
            "--notes",
            "scratch implementation for SQLite regression coverage",
            "--json",
        ],
        "llm:builder",
    ));

    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "edge",
            "explore",
            parent_id,
            child_id,
            "ground",
            "--criterion",
            "checkout validation directly constrains checkout flow behavior",
            "--evidence",
            "the validation child blocks invalid carts before order creation",
            "--confidence",
            "0.9",
            "--json",
        ],
        "llm:analyzer",
    ));

    assert_status_ok(&run_json(
        &graph.root,
        &[
            "validation",
            "add",
            "--name",
            "checkout validation smoke",
            "--type",
            "test",
            "--command",
            "true",
            "--intent",
            child_id,
            "--json",
        ],
    ));
    assert_status_ok(&run_json(
        &graph.root,
        &[
            "validation",
            "mark",
            "checkout validation smoke",
            "--result",
            "passed",
            "--evidence",
            "scratch command returns success",
            "--json",
        ],
    ));

    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "rule",
            "add",
            "--name",
            "checkout validation rule",
            "--description",
            "checkout behavior has a validation before it is considered complete",
            "--severity",
            "warning",
            "--json",
        ],
        "llm:quality",
    ));
    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "rule",
            "apply",
            "checkout validation rule",
            child_id,
            "--criterion",
            "child checkout behavior has an attached passing validation",
            "--json",
        ],
        "llm:quality",
    ));
    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "rule",
            "verdict",
            "checkout validation rule",
            child_id,
            "--status",
            "passing",
            "--criterion",
            "child checkout behavior has an attached passing validation",
            "--evidence",
            "checkout validation smoke is marked passed",
            "--json",
        ],
        "llm:quality",
    ));

    let hypothesis = run_json_as(
        &graph.root,
        &[
            "hypothesis",
            "add",
            "--name",
            "checkout validation split",
            "--claim",
            "checkout validation may need its own module as rules grow",
            "--proposal",
            "split validation helpers from the checkout flow orchestration",
            "--predicted-outcome",
            "validation helpers become independently testable",
            "--target",
            child_id,
            "--json",
        ],
        "llm:builder",
    );
    assert_status_ok(&hypothesis);
    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "hypothesis",
            "prove",
            "checkout validation split",
            "--verdict",
            "refuted",
            "--evidence",
            "scratch fixture is intentionally small and does not justify a split",
            "--json",
        ],
        "llm:analyzer",
    ));

    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "ignore",
            "add",
            "target/sqlite-regression/**",
            "--reason",
            "scratch regression output",
            "--json",
        ],
        "llm:builder",
    ));
    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "delegate",
            "add",
            "target/delegated/**",
            "--to",
            "target/delegated/loom.graph.json",
            "--json",
        ],
        "llm:builder",
    ));
    assert_status_ok(&run_json_as(
        &graph.root,
        &["delegate", "remove", "target/delegated/**", "--json"],
        "llm:builder",
    ));

    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "persona",
            "add",
            "--name",
            "checkout-operator",
            "--description",
            "operator verifying checkout behavior in a scratch regression graph",
            "--json",
        ],
        "llm:builder",
    ));
    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "persona",
            "serve",
            "checkout-operator",
            child_id,
            "ground",
            "--criterion",
            "operator needs the validation behavior to trust checkout changes",
            "--evidence",
            "the scratch validation smoke is the operator's regression proof",
            "--confidence",
            "0.9",
            "--json",
        ],
        "llm:analyzer",
    ));

    let status = run_json(&graph.root, &["status", "--json"]);
    assert!(status["total_intents"].as_i64().unwrap_or_default() >= 2);

    let export = run_json(&graph.root, &["export", "-", "--json"]);
    assert!(export["nodes"]["Intent"].as_array().unwrap().len() >= 2);
}
