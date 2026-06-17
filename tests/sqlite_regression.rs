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
        .env_remove("LOOM_DIAGNOSE_MISSING_BASE")
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

fn run_json_failure_as(cwd: &Path, args: &[&str], agent: &str) -> Value {
    let output = Command::new(loom_bin())
        .args(args)
        .current_dir(cwd)
        .env("LOOM_AGENT", agent)
        .env_remove("LOOM_GRAPH")
        .output()
        .unwrap_or_else(|err| panic!("failed to run loom {args:?}: {err}"));

    if output.status.success() {
        panic!(
            "loom {:?} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "loom {:?} emitted invalid JSON after failure: {err}\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn run_text_as(cwd: &Path, args: &[&str], agent: &str) -> String {
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

    String::from_utf8(output.stdout)
        .unwrap_or_else(|err| panic!("loom {args:?} emitted non-UTF8 stdout: {err}"))
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

fn delete_interface_inventory(root: &Path) {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    conn.execute("DELETE FROM calls", [])
        .expect("delete scratch calls");
    conn.execute("DELETE FROM interface_surface", [])
        .expect("delete scratch interface surfaces");
}

fn insert_interface_surface(root: &Path, id: &str, name: &str, method: &str, target: &str) {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    conn.execute(
        "INSERT INTO interface_surface(
            id, name, description, surface_kind, method, target, created_at, updated_at
         ) VALUES(?1, ?2, 'scratch interface gap fixture', 'http_endpoint', ?3, ?4, 'now', 'now')",
        rusqlite::params![id, name, method, target],
    )
    .expect("insert scratch interface surface");
}

fn delete_validates_for_validation(root: &Path, validation_id: &str) {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    conn.execute(
        "DELETE FROM validates WHERE validation_id = ?1",
        rusqlite::params![validation_id],
    )
    .expect("delete scratch validates");
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
fn sqlite_migrate_reports_open_time_schema_contract() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-migrate-schema");

    let migrated = run_json(&graph.root, &["migrate", "--json"]);
    assert_eq!(migrated["status"], "ok");
    assert_eq!(migrated["backend"], "sqlite");
    assert_eq!(migrated["migrated"], false);
    assert_eq!(migrated["version"], "9");
    assert!(
        migrated["message"]
            .as_str()
            .is_some_and(|message| message.contains("created on open")),
        "migrate should teach the current SQLite schema contract: {migrated}"
    );
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
            "--confidence",
            "0.9",
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

#[test]
fn sqlite_populate_backfills_interface_calls_from_existing_sagas() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-populate-interfaces");
    run_json(&graph.root, &["init", ".", "--json"]);

    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "cart creation",
            "--description",
            "customer can create a cart through the HTTP checkout API",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
            "--boundary",
            "inbound",
            "--json",
        ],
        "llm:builder",
    );
    let intent_id = intent["id"].as_str().expect("intent id");
    write_scratch_file(
        &graph.root,
        "journeys/cart.yaml",
        &format!(
            "saga: cart-flow\nsteps:\n  - name: create cart\n    intent: {intent_id}\n    request:\n      method: POST\n      url: /carts\n    expect:\n      status: 201\n"
        ),
    );
    assert_status_ok(&run_json_as(
        &graph.root,
        &["saga", "add", "journeys/cart.yaml", "--json"],
        "llm:builder",
    ));

    let initial = run_json(&graph.root, &["interface", "list", "--json"]);
    assert_eq!(initial["total"], 1);
    delete_interface_inventory(&graph.root);

    let pending = run_json_as(
        &graph.root,
        &["next", "--mode", "populate", "--json"],
        "llm:builder",
    );
    assert_eq!(pending["mode"], "populate");
    assert_eq!(pending["kind"], "interface_from_sagas");
    assert_eq!(pending["missing_surfaces"], 1);

    let applied = run_json_as(
        &graph.root,
        &["populate", "interfaces", "--from-sagas", "--json"],
        "llm:builder",
    );
    assert_eq!(applied["status"], "ok");
    assert_eq!(applied["interface_surfaces_created"], 1);
    assert_eq!(applied["calls_written"], 1);

    let populated = run_json(&graph.root, &["interface", "list", "--json"]);
    assert_eq!(populated["total"], 1);
    assert_eq!(populated["interfaces"][0]["target"], "/carts");
    assert_eq!(populated["interfaces"][0]["calls"], 1);

    let plan = run_json_as(&graph.root, &["populate", "plan", "--json"], "llm:builder");
    assert_eq!(
        plan["populate"]["interface_from_sagas"]["pending"], false,
        "populate should be idempotent after backfill: {plan}"
    );
}

#[test]
fn sqlite_interface_gaps_detect_boundary_intent_without_calls() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-interface-gap-boundary");
    run_json(&graph.root, &["init", ".", "--json"]);

    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "public cart endpoint",
            "--description",
            "customer can call the public cart endpoint through the service boundary",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
            "--boundary",
            "inbound",
            "--json",
        ],
        "llm:builder",
    );
    assert_status_ok(&intent);

    let gaps = run_json(&graph.root, &["interface", "gaps", "--json"]);
    assert_eq!(gaps["interface_gaps"]["boundary_intent_without_calls"], 1);
    assert_eq!(gaps["interface_gaps"]["total"], 1);

    let plan = run_json(&graph.root, &["populate", "plan", "--json"]);
    assert_eq!(
        plan["populate"]["interface_gaps"]["boundary_intent_without_calls"],
        gaps["interface_gaps"]["boundary_intent_without_calls"]
    );
}

#[test]
fn sqlite_status_surfaces_populate_gap_lane() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-status-populate");
    run_json(&graph.root, &["init", ".", "--json"]);

    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "status visible endpoint",
            "--description",
            "operator can see that this boundary endpoint still needs interface population",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
            "--boundary",
            "inbound",
            "--json",
        ],
        "llm:builder",
    );
    assert_status_ok(&intent);

    let status = run_json(&graph.root, &["status", "--json"]);
    assert_eq!(status["other_lanes"]["populate"], 1);
    assert_eq!(status["populate"]["total"], 1);
    assert_eq!(status["populate"]["interface_gaps"], 1);
    assert_eq!(
        status["populate"]["next_command"],
        "loom next --mode populate"
    );

    let human = run_text_as(&graph.root, &["status"], "llm:validator");
    assert!(
        human.contains("populate: 1 gap(s) waiting"),
        "human status should teach the populate gap: {human}"
    );
    assert!(
        human.contains("other open lanes: populate 1"),
        "human status should include populate in other lanes: {human}"
    );
}

#[test]
fn sqlite_saga_diagnose_reports_missing_env_without_stamping() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-saga-diagnose-env");
    write_scratch_file(
        &graph.root,
        "journeys/diagnose-env.yaml",
        r#"
saga: diagnose-env
base: "{{ env.LOOM_DIAGNOSE_MISSING_BASE }}"
steps:
  - name: call target
    intent: saga runner halt-on-failure semantics
    request: { method: GET, url: /health }
    expect: { status: 200 }
"#,
    );

    let diagnosed = run_json_failure_as(
        &graph.root,
        &["saga", "diagnose", "journeys/diagnose-env.yaml", "--json"],
        "llm:validator",
    );
    assert_eq!(diagnosed["status"], "failed");
    assert_eq!(
        diagnosed["diagnosis"]["steps"][0]["root_cause"]["kind"],
        "env_var_missing"
    );
    assert!(diagnosed["diagnosis"]["steps"][0]["root_cause"]["fix"]
        .as_str()
        .unwrap()
        .contains("LOOM_DIAGNOSE_MISSING_BASE=<value> loom saga run diagnose-env"));
}

#[test]
fn sqlite_inbox_add_normalize_mark_and_export() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-inbox-flow");

    let added = run_json(
        &graph.root,
        &[
            "inbox",
            "add",
            "status debt feels scarier than reality",
            "--source",
            "chat",
            "--json",
        ],
    );
    let id = added["item"]["id"].as_str().expect("inbox id").to_string();
    assert_eq!(added["item"]["status"], "new");

    let status = run_json(&graph.root, &["status", "--json"]);
    assert_eq!(status["intake"]["untriaged"], 1);
    assert_eq!(status["completion"]["required_autonomous_debt"]["total"], 0);

    let triage = run_json(&graph.root, &["inbox", "triage", "--take", "5", "--json"]);
    assert_eq!(triage["count"], 1);
    assert_eq!(triage["taken"], 1);
    assert_eq!(triage["queue_total"], 1);
    assert!(triage["normalize_templates"][0]
        .as_str()
        .unwrap()
        .contains(&id));

    let next = run_json(&graph.root, &["next", "--all", "--json"]);
    assert!(next["queues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|queue| queue["queue"] == "inbox" && queue["optional"] == true));

    let normalized = run_json(
        &graph.root,
        &[
            "inbox",
            "normalize",
            &id,
            "--kind",
            "rough_edge",
            "--claim",
            "status should separate required debt from optional enrichment",
            "--route",
            "note",
            "--command",
            "loom note add --kind decision --text \"status taxonomy accepted\"",
            "--json",
        ],
    );
    assert_eq!(normalized["item"]["status"], "triaged");
    assert_eq!(normalized["item"]["route_kind"], "note");

    let proposed = run_json(
        &graph.root,
        &[
            "inbox",
            "add",
            "add saga diagnose so failed HTTP proofs explain root causes",
            "--source",
            "chat",
            "--json",
        ],
    );
    let proposal_id = proposed["item"]["id"]
        .as_str()
        .expect("proposal inbox id")
        .to_string();
    let proposal = run_json(
        &graph.root,
        &[
            "inbox",
            "normalize",
            &proposal_id,
            "--kind",
            "feature_proposal",
            "--claim",
            "saga failures should produce structured diagnosis",
            "--route",
            "intent",
            "--command",
            "loom intent add --name 'saga failure diagnosis' --description 'diagnose failed saga runs' --level feature --lifecycle planned",
            "--json",
        ],
    );
    assert_eq!(proposal["item"]["kind"], "feature_proposal");
    assert_eq!(proposal["item"]["route_kind"], "intent");

    let marked = run_json(
        &graph.root,
        &[
            "inbox",
            "mark",
            &id,
            "--status",
            "routed",
            "--reason",
            "route command reviewed and no graph mutation was needed for this fixture",
            "--json",
        ],
    );
    assert_eq!(marked["item"]["status"], "routed");

    let exported = run_json(&graph.root, &["export", "-", "--json"]);
    let inbox = exported["nodes"]["InboxItem"].as_array().unwrap();
    assert!(inbox.iter().any(|item| item["id"] == id));
}

#[test]
fn sqlite_door_captures_inbox_item_before_routing() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-door-inbox");

    let door = run_json(
        &graph.root,
        &[
            "door",
            "users need a better intake boundary",
            "--limit",
            "3",
            "--json",
        ],
    );
    let id = door["inbox_item"]["id"]
        .as_str()
        .expect("door inbox id")
        .to_string();
    assert_eq!(door["inbox_item"]["status"], "new");
    assert!(door["next_step"].as_str().unwrap().contains(&id));

    let listed = run_json(&graph.root, &["inbox", "list", "--status", "new", "--json"]);
    assert_eq!(listed["count"], 1);
    assert!(listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == id));
}

#[test]
fn sqlite_interface_gaps_detect_surface_without_calls() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-interface-gap-surface");
    run_json(&graph.root, &["init", ".", "--json"]);
    insert_interface_surface(
        &graph.root,
        "surface-without-calls",
        "GET /health",
        "GET",
        "/health",
    );

    let gaps = run_json(&graph.root, &["interface", "gaps", "--json"]);
    assert_eq!(gaps["interface_gaps"]["surface_without_calls"], 1);
    assert_eq!(gaps["interface_gaps"]["total"], 1);
    assert_eq!(
        gaps["interface_gaps"]["examples"][0]["kind"],
        "surface_without_calls"
    );
}

#[test]
fn sqlite_interface_gaps_detect_call_without_validates() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-interface-gap-validates");
    run_json(&graph.root, &["init", ".", "--json"]);

    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "payment capture endpoint",
            "--description",
            "customer payment can be captured through the HTTP checkout boundary",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
            "--boundary",
            "inbound",
            "--json",
        ],
        "llm:builder",
    );
    let intent_id = intent["id"].as_str().expect("intent id");
    write_scratch_file(
        &graph.root,
        "journeys/payment.yaml",
        &format!(
            "saga: payment-flow\nsteps:\n  - name: capture payment\n    intent: {intent_id}\n    request:\n      method: POST\n      url: /payments/capture\n    expect:\n      status: 200\n"
        ),
    );
    let saga = run_json_as(
        &graph.root,
        &["saga", "add", "journeys/payment.yaml", "--json"],
        "llm:builder",
    );
    let validation_id = saga["validation_id"].as_str().expect("validation id");
    delete_validates_for_validation(&graph.root, validation_id);

    let gaps = run_json(&graph.root, &["interface", "gaps", "--json"]);
    assert_eq!(gaps["interface_gaps"]["call_without_validates"], 1);
    assert_eq!(gaps["interface_gaps"]["total"], 1);
    assert_eq!(
        gaps["interface_gaps"]["examples"][0]["kind"],
        "call_without_validates"
    );
}

#[test]
fn sqlite_populate_next_prioritizes_deterministic_backfill_before_interface_gaps() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-populate-priority");
    run_json(&graph.root, &["init", ".", "--json"]);

    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "cart priority endpoint",
            "--description",
            "customer can create a cart through the HTTP priority endpoint",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
            "--boundary",
            "inbound",
            "--json",
        ],
        "llm:builder",
    );
    let intent_id = intent["id"].as_str().expect("intent id");
    write_scratch_file(
        &graph.root,
        "journeys/priority.yaml",
        &format!(
            "saga: priority-flow\nsteps:\n  - name: create priority cart\n    intent: {intent_id}\n    request:\n      method: POST\n      url: /priority-carts\n    expect:\n      status: 201\n"
        ),
    );
    assert_status_ok(&run_json_as(
        &graph.root,
        &["saga", "add", "journeys/priority.yaml", "--json"],
        "llm:builder",
    ));
    insert_interface_surface(
        &graph.root,
        "unbound-extra-surface",
        "GET /unbound",
        "GET",
        "/unbound",
    );
    delete_interface_inventory(&graph.root);
    insert_interface_surface(
        &graph.root,
        "unbound-extra-surface",
        "GET /unbound",
        "GET",
        "/unbound",
    );

    let next = run_json_as(
        &graph.root,
        &["next", "--mode", "populate", "--json"],
        "llm:builder",
    );
    assert_eq!(next["kind"], "interface_from_sagas");
    assert_eq!(next["missing_surfaces"], 1);
}
