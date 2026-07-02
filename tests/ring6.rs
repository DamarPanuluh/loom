//! Ring 6 tests — smells (structural), debt (statistical, never stored),
//! doctor (integrity), and a live journey run against a mock HTTP server.

use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Tmp {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("loom-ring6-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn write(&self, rel: &str, content: &str) {
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

fn intent(store: &Store, name: &str, lifecycle: &str) -> String {
    store
        .add_node(NodeType::Intent, name, "", lifecycle, serde_json::json!({}))
        .unwrap()
        .id
}
fn codefile(store: &Store, path: &str) -> String {
    store
        .add_node(NodeType::CodeFile, path, "", "", serde_json::json!({}))
        .unwrap()
        .id
}

// ---- smells ----------------------------------------------------------------

#[test]
fn smells_detect_tangle_and_overlap() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let cf = codefile(&store, "src/god.rs");
    for i in 0..3 {
        let id = intent(&store, &format!("behavior {i}"), "implemented");
        store
            .add_edge(EdgeKind::Implements, &id, &cf, TruthClass::Asserted)
            .unwrap();
    }
    let smells = loom::signal::smells(&store).unwrap();
    assert!(smells.iter().any(|s| s.kind == "tangled_file"));

    // a separate 2-owner file with no edge → overlapping_ownership
    let cf2 = codefile(&store, "src/pair.rs");
    let a = intent(&store, "alpha behavior", "implemented");
    let b = intent(&store, "beta behavior", "implemented");
    store
        .add_edge(EdgeKind::Implements, &a, &cf2, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Implements, &b, &cf2, TruthClass::Asserted)
        .unwrap();
    let smells = loom::signal::smells(&store).unwrap();
    assert!(smells.iter().any(|s| s.kind == "overlapping_ownership"));
}

#[test]
fn smells_duplicated_responsibility_via_tags() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store.add_vocab_term("retry", "retry policy").unwrap();
    let a = intent(&store, "retry on http failure", "implemented");
    let b = intent(&store, "retry on queue failure", "implemented");
    let fa = codefile(&store, "src/http.rs");
    let fb = codefile(&store, "src/queue.rs");
    store
        .add_edge(EdgeKind::Implements, &a, &fa, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Implements, &b, &fb, TruthClass::Asserted)
        .unwrap();
    store.set_tag(&a, TargetKind::Node, "retry").unwrap();
    store.set_tag(&b, TargetKind::Node, "retry").unwrap();
    let smells = loom::signal::smells(&store).unwrap();
    assert!(smells.iter().any(|s| s.kind == "duplicated_responsibility"));
}

// ---- journey proof smells --------------------------------------------------

/// helper: an implemented intent marked user_visible.
fn visible_intent(store: &Store, name: &str) -> String {
    let id = intent(store, name, "implemented");
    store
        .set_facet(
            &id,
            TargetKind::Node,
            "visibility",
            "user_visible",
            TruthClass::Asserted,
        )
        .unwrap();
    id
}

#[test]
fn journey_proof_smell_fires_when_user_visible_intent_has_no_validation() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let _ = visible_intent(&store, "checkout completes");
    let smells = loom::signal::smells(&store).unwrap();
    assert!(smells
        .iter()
        .any(|s| s.kind == "missing_journey_proof" && s.message.contains("checkout completes")),);
}

#[test]
fn journey_proof_smell_fires_when_validation_is_too_shallow() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent_id = visible_intent(&store, "checkout completes");
    // a non-journey, non-L5 validation linked via Validates
    let validation = store
        .add_node(
            NodeType::Validation,
            "unit checkout",
            "",
            "passed",
            serde_json::json!({"proof_kind":"unit","proof_level":"L1"}),
        )
        .unwrap();
    let edge = store
        .ensure_edge(EdgeKind::Validates, &validation.id, &intent_id)
        .unwrap();
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "unit test passes",
            "cargo test passed",
            0.9,
            "test",
        )
        .unwrap();
    let smells = loom::signal::smells(&store).unwrap();
    assert!(smells
        .iter()
        .any(|s| s.kind == "proof_too_shallow_for_intent"
            && s.message.contains("checkout completes")),);
}

#[test]
fn journey_proof_smell_silent_when_passing_l5_journey_proof_exists() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent_id = visible_intent(&store, "checkout completes");
    let validation = store
        .add_node(
            NodeType::Validation,
            "checkout journey",
            "",
            "passed",
            serde_json::json!({"proof_kind":"journey","proof_level":"L5"}),
        )
        .unwrap();
    let edge = store
        .ensure_edge(EdgeKind::Validates, &validation.id, &intent_id)
        .unwrap();
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "journey passes end-to-end",
            "journey run passed",
            0.9,
            "test",
        )
        .unwrap();
    let smells = loom::signal::smells(&store).unwrap();
    assert!(
        !smells
            .iter()
            .any(|s| s.kind == "missing_journey_proof" || s.kind == "proof_too_shallow_for_intent"),
        "no journey proof smell should fire: {smells:?}"
    );
}

// Drift gate ties sync staleness to the smell: a passing L5 journey proof
// silences the smell, but once its artifact drifts and sync resets the proof,
// the smell MUST re-fire — a stale artifact cannot keep an intent "proven".
#[test]
fn journey_proof_smell_re_fires_after_artifact_drift_resets_proof() {
    let tmp = Tmp::new();
    tmp.write("contracts/checkout.v1.json", r#"{"routes":[]}"#);
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent_id = visible_intent(&store, "checkout completes");
    let validation = store
        .add_node(
            NodeType::Validation,
            "checkout journey",
            "",
            "not_run",
            serde_json::json!({
                "type": "journey",
                "proof_kind": "journey",
                "proof_level": "L5",
                "artifact": "contracts/checkout.v1.json",
            }),
        )
        .unwrap();
    let edge = store
        .ensure_edge(EdgeKind::Validates, &validation.id, &intent_id)
        .unwrap();
    // Baseline sync + pass → no smell.
    loom::sync::run(&store, tmp.path()).unwrap();
    store.set_node_status(&validation.id, "passed").unwrap();
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "journey passes end-to-end",
            "journey run passed",
            0.9,
            "test",
        )
        .unwrap();
    let silent = loom::signal::smells(&store).unwrap();
    assert!(
        !silent
            .iter()
            .any(|s| s.kind == "missing_journey_proof" || s.kind == "proof_too_shallow_for_intent"),
        "passing L5 journey proof should silence the smell: {silent:?}"
    );

    // Artifact drifts + sync resets the proof → smell re-fires.
    tmp.write(
        "contracts/checkout.v1.json",
        r#"{"routes":[{"path":"/x"}]}"#,
    );
    loom::sync::run(&store, tmp.path()).unwrap();
    let smells = loom::signal::smells(&store).unwrap();
    assert!(
        smells
            .iter()
            .any(|s| s.kind == "proof_too_shallow_for_intent"
                && s.message.contains("checkout completes")),
        "a drifted artifact must re-fire the journey proof smell: {smells:?}"
    );
}
// ---- debt: statistical, never stored (INV-3) -------------------------------

#[test]
fn debt_size_outlier_is_not_stored() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // loc facets: three small, one huge → outlier
    for (p, loc) in [("a.rs", 40), ("b.rs", 50), ("c.rs", 45), ("big.rs", 5000)] {
        let id = codefile(&store, p);
        store
            .set_facet(
                &id,
                TargetKind::Node,
                "loc",
                &loc.to_string(),
                TruthClass::Derived,
            )
            .unwrap();
    }
    let edges_before = store.list_edges(None, usize::MAX).unwrap().len();
    let debt = loom::signal::debt(&store).unwrap();
    assert!(debt
        .iter()
        .any(|d| d.kind == "size_outlier" && d.message.contains("big.rs")));
    // INV-3: computing debt stores no edges
    let edges_after = store.list_edges(None, usize::MAX).unwrap().len();
    assert_eq!(edges_before, edges_after, "debt must never store edges");
}

// ---- doctor ----------------------------------------------------------------

#[test]
fn doctor_clean_on_valid_graph() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "user can log in", "implemented");
    let cf = codefile(&store, "src/auth.rs");
    let e = store
        .add_edge(EdgeKind::Implements, &i, &cf, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &e.id,
            InspectionStatus::Passing,
            "c",
            "src/auth.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    let issues = loom::signal::doctor(&store).unwrap();
    assert!(
        issues.is_empty(),
        "valid graph must pass doctor: {issues:?}"
    );
}

#[test]
fn doctor_flags_hierarchy_cycles() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let parent = intent(&store, "parent behavior", "implemented");
    let child = intent(&store, "child behavior", "implemented");
    store
        .add_edge(EdgeKind::Hierarchy, &parent, &child, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Hierarchy, &child, &parent, TruthClass::Asserted)
        .unwrap();

    let issues = loom::signal::doctor(&store).unwrap();
    assert!(
        issues.iter().any(|issue| issue.kind == "hierarchy_cycle"),
        "cyclic hierarchy must be reported by doctor: {issues:?}"
    );
}

// ---- live journey run ------------------------------------------------------

/// A tiny HTTP/1.1 server that answers `n` requests with the given (status, body).
fn mock_server(responses: Vec<(u16, String)>) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        for (status, body) in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

#[test]
fn journey_run_stamps_passing_steps() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let cart = intent(&store, "cart can be created", "implemented");
    let pay = intent(&store, "payment can be captured", "implemented");
    // journey validation + validates edges (as `loom journey add` would create)
    let journey = store
        .add_node(
            NodeType::Validation,
            "checkout-flow",
            "",
            "not_run",
            serde_json::json!({"type":"journey"}),
        )
        .unwrap();
    store
        .ensure_edge(EdgeKind::Validates, &journey.id, &cart)
        .unwrap();
    store
        .ensure_edge(EdgeKind::Validates, &journey.id, &pay)
        .unwrap();

    let (base, handle) = mock_server(vec![
        (201, r#"{"id":"c1"}"#.into()),
        (200, r#"{"state":"paid"}"#.into()),
    ]);
    let spec = loom::journey::JourneySpec {
        journey: "checkout-flow".into(),
        base,
        steps: vec![
            serde_json::from_value(serde_json::json!({
                "name": "create cart", "intent": "cart can be created",
                "request": { "method": "POST", "url": "/carts" },
                "expect": { "status": 201 },
                "capture": { "cart_id": "$.id" }
            }))
            .unwrap(),
            serde_json::from_value(serde_json::json!({
                "name": "capture payment", "intent": "payment can be captured",
                "request": { "method": "POST", "url": "/carts/{{ cart_id }}/pay" },
                "expect": { "status": 200, "body": { "$.state": "paid" } }
            }))
            .unwrap(),
        ],
    };
    let outcomes = loom::journey::execute(Some(&store), &spec, true).unwrap();
    handle.join().unwrap();
    assert_eq!(outcomes.len(), 2);
    assert!(
        outcomes.iter().all(|o| o.passed),
        "both steps should pass: {outcomes:?}"
    );

    // both validates edges stamped passing; journey node passed
    for intent_id in [&cart, &pay] {
        let e = store
            .edges_with(
                Some(EdgeKind::Validates),
                Some(&journey.id),
                Some(intent_id),
            )
            .unwrap();
        assert_eq!(e[0].status, InspectionStatus::Passing);
    }
    assert_eq!(
        store.get_node(&journey.id).unwrap().unwrap().status,
        "passed"
    );
}

// ---- HTTP contract JSON → journey run -------------------------------------
//
// These exercise the `loom journey run` contract for an HTTP-contract spec
// (routes → normalized steps). The mock server conditions its second response
// on the `person_id` extracted from route 1 actually appearing in the path,
// query, AND body of route 2 — so a broken interpolation cannot pass.

/// A mock HTTP/1.1 server that answers `n` requests, recording each request's
/// request line + body. `handler` receives the raw request text and returns the
/// (status, body) to emit. Lets a test condition a response on what was
/// received — proving interpolation actually happened.
fn mock_server_handling(
    n: usize,
    handler: impl Fn(&str) -> (u16, String) + Send + Sync + 'static,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        for _ in 0..n {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = [0u8; 8192];
            let read = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..read]).to_string();
            let (status, body) = handler(&req);
            let resp = format!(
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

/// Run the compiled `loom` binary against `tmp` graph with the given args;
/// assert it exits zero (the journey add/run wiring under test).
fn run_cli(tmp: &Path, args: &[&str]) {
    let mut cmd = std::process::Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom")));
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {:?}: {e}", args));
    assert!(
        out.status.success(),
        "loom {:?} failed: {:?}\n--stderr--\n{}",
        args,
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run `loom --graph <tmp> <args> --json` and return stdout parsed as JSON,
/// panicking with stdout/stderr on failure so a regression is diagnosed.
fn run_cli_json(tmp: &Path, args: &[&str]) -> serde_json::Value {
    let mut cmd = std::process::Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom")));
    cmd.arg("--graph").arg(tmp).args(args).arg("--json");
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {:?}: {e}", args));
    assert!(
        out.status.success(),
        "loom {:?} failed: {:?}\n--stderr--\n{}",
        args,
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "loom {:?} did not emit JSON (status {:?}):\n--stdout--\n{}\nparse: {e}",
            args, out.status, stdout
        )
    })
}

/// Contract: an HTTP-contract JSON with two routes runs through the journey
/// runner. Route 1 extracts `person_id`; route 2 threads it into the path,
/// a query param, and the JSON body, and asserts `response_fields` existence.
/// The mock conditions route 2's success on the extracted id appearing in all
/// three places — a broken interpolation reddens this.
#[test]
fn http_contract_runs_two_routes_threading_extracted_id() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // intents the routes declare (journey add would link these)
    let _create = intent(&store, "register a person", "implemented");
    let _fetch = intent(&store, "fetch the person record", "implemented");
    drop(store);

    // route 2 succeeds only if the extracted person_id ("p-42") is present in
    // the path, the `event_id` query param, AND the JSON body's `subject`.
    let (base, handle) = mock_server_handling(2, |req| {
        // Match the request line precisely: route 1 is the exact path
        // `/v1/example/persons` (followed by ` ` or `?`), NOT the longer
        // route-2 path `/v1/example/persons/p-42/events`. A bare `contains`
        // would match both and hand route 2 the canned 201.
        let request_line = req.lines().next().unwrap_or("");
        let route1 = request_line.starts_with("POST /v1/example/persons ")
            || request_line.starts_with("POST /v1/example/persons?");
        if route1 {
            return (201, r#"{"person_id":"p-42","name":"ada"}"#.into());
        }
        // route 2: path must carry p-42, query must carry event_id=p-42,
        // body must carry subject=p-42. Missing any → a 404 that fails the run.
        let path_ok = req.contains("/v1/example/persons/p-42/events");
        let query_ok = req.contains("event_id=p-42");
        let body_ok = req.contains(r#""subject":"p-42""#);
        if path_ok && query_ok && body_ok {
            (
                200,
                r#"{"event_id":"e-7","subject":"p-42","occurred_at":"now"}"#.into(),
            )
        } else {
            (404, r#"{"error":"not found"}"#.into())
        }
    });

    let spec_path = tmp.path().join("sample-service-http.contract.json");
    std::fs::write(
        &spec_path,
        serde_json::json!({
            "name": "sample-service-http",
            "base": base,
            "routes": [
                {
                    "method": "POST",
                    "path": "/v1/example/persons",
                    "intent": "register a person",
                    "success_status": 201,
                    "extract": [{ "field": "person_id", "as": "person_id" }],
                    "response_fields": ["person_id", "name"]
                },
                {
                    "method": "POST",
                    "path": "/v1/example/persons/{{ person_id }}/events",
                    "intent": "fetch the person record",
                    "success_status": 200,
                    "query": { "event_id": "{{ person_id }}" },
                    "example_request": { "subject": "{{ person_id }}" },
                    "response_fields": ["event_id", "subject"]
                }
            ]
        })
        .to_string(),
    )
    .unwrap();

    // `journey run` requires a pre-existing Validation node named after the
    // contract's `name`; `journey add` creates it (and the step edges).
    run_cli(tmp.path(), &["journey", "add", spec_path.to_str().unwrap()]);
    let out = run_cli_json(tmp.path(), &["journey", "run", spec_path.to_str().unwrap()]);
    handle.join().unwrap();

    assert_eq!(out["journey"], "sample-service-http");
    assert_eq!(out["total"], 2, "both routes ran: {out}");
    assert_eq!(out["passed"], 2, "both routes passed: {out}");
    let outcomes = out["outcomes"].as_array().expect("outcomes is an array");
    assert_eq!(outcomes.len(), 2);
    assert!(
        outcomes.iter().all(|o| o["passed"] == true),
        "every outcome passed: {outcomes:?}"
    );

    let store = Store::open(tmp.path()).unwrap();
    // both validates edges stamped passing; journey node passed
    let journey = store
        .resolve_node("sample-service-http", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(journey.status, "passed");
    let validates = store
        .edges_with(Some(EdgeKind::Validates), Some(&journey.id), None)
        .unwrap();
    assert_eq!(validates.len(), 2, "journey add linked both route intents");
    for e in &validates {
        assert_eq!(
            e.status,
            InspectionStatus::Passing,
            "each route's validates edge is passing"
        );
    }
}

/// Contract: when a route's `response_fields` declares a field the response
/// omits, the step fails with a detail naming the missing field. This is the
/// existence-check failure path — a regression that silently drops the check
/// (or misnames the field) reddens this.
#[test]
fn http_contract_missing_response_field_fails_with_detail() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let _create = intent(&store, "register a person", "implemented");
    drop(store);

    let (base, handle) = mock_server_handling(1, |_req| {
        // response omits `name` — route declares it in response_fields
        (201, r#"{"person_id":"p-42"}"#.into())
    });

    let spec_path = tmp.path().join("missing-field.contract.json");
    std::fs::write(
        &spec_path,
        serde_json::json!({
            "name": "missing-field-http",
            "base": base,
            "routes": [
                {
                    "method": "POST",
                    "path": "/v1/example/persons",
                    "intent": "register a person",
                    "success_status": 201,
                    "response_fields": ["person_id", "name"]
                }
            ]
        })
        .to_string(),
    )
    .unwrap();

    run_cli(tmp.path(), &["journey", "add", spec_path.to_str().unwrap()]);
    let out = run_cli_json(tmp.path(), &["journey", "run", spec_path.to_str().unwrap()]);
    handle.join().unwrap();

    assert_eq!(out["total"], 1, "one route ran: {out}");
    assert_eq!(out["passed"], 0, "the route failed: {out}");
    let outcomes = out["outcomes"].as_array().expect("outcomes is an array");
    assert_eq!(outcomes.len(), 1);
    assert_eq!(
        outcomes[0]["passed"],
        serde_json::Value::Bool(false),
        "the step is marked failed: {outcomes:?}"
    );
    let detail = outcomes[0]["detail"]
        .as_str()
        .expect("failure carries a detail string");
    assert!(
        detail.contains("$.name"),
        "the detail names the missing field path ($.name): {detail}"
    );

    let store = Store::open(tmp.path()).unwrap();
    // the failing route stamps its validates edge failing; journey node failed
    let journey = store
        .resolve_node("missing-field-http", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(journey.status, "failed");
    let validates = store
        .edges_with(Some(EdgeKind::Validates), Some(&journey.id), None)
        .unwrap();
    assert_eq!(validates.len(), 1, "the one route's edge was linked");
    assert_eq!(
        validates[0].status,
        InspectionStatus::Failing,
        "the failing route's edge is failing"
    );
}

// ---- journey diagnose (graph-free HTTP contract executor) ------------------
//
// These test the `loom journey diagnose <spec>` path: a consumer-facing proof
// that parses JSON or YAML, sends requests, checks status/fields, and threads
// captures — no graph registration, no intent resolution.

/// Run the compiled `loom` binary with arbitrary args (no --graph); returns
/// stdout parsed as JSON. Panics on non-zero exit or non-JSON output.
fn run_loom_json(args: &[&str]) -> serde_json::Value {
    let mut cmd = std::process::Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom")));
    cmd.args(args).arg("--json");
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {:?}: {e}", args));
    assert!(
        out.status.success(),
        "loom {:?} failed: {:?}\n--stdout--\n{}\n--stderr--\n{}",
        args,
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "loom {:?} did not emit JSON:\n--stdout--\n{}\nparse: {e}",
            args, stdout
        )
    })
}

/// Contract: `loom journey diagnose <spec.yaml>` parses a YAML HTTP contract,
/// sends requests against a mock server, checks status + field existence,
/// threads captures via `{{ person_id }}` interpolation, and reports green.
/// No graph, no intent nodes, no `journey add`.
#[test]
fn journey_diagnose_yaml_contract_without_graph() {
    let tmp = Tmp::new();
    let (base, handle) = mock_server_handling(2, |req| {
        // Route 1: POST /persons → 201 { person_id: "p1" }
        // Route 2: POST /persons/p1/events → check person_id was threaded
        if req.contains("POST /v1/persons HTTP") {
            (201, r#"{"person_id":"p1","name":"ada"}"#.into())
        } else if req.contains("/persons/p1/events") {
            (200, r#"{"event_id":"e1","subject":"p1"}"#.into())
        } else {
            (404, r#"{"error":"unknown route"}"#.into())
        }
    });

    let spec_path = tmp.path().join("contract.yaml");
    std::fs::write(
        &spec_path,
        format!(
            r#"name: yaml-journey
base: "{base}"
routes:
  - method: POST
    path: /v1/persons
    intent: register a person
    success_status: 201
    extract:
      - field: person_id
        as: person_id
    response_fields:
      - person_id
      - name
  - method: POST
    path: "/v1/persons/{{{{ person_id }}}}/events"
    intent: emit person event
    success_status: 200
    example_request:
      subject: "{{{{ person_id }}}}"
    response_fields:
      - event_id
      - subject
"#
        ),
    )
    .unwrap();

    let out = run_loom_json(&["journey", "diagnose", spec_path.to_str().unwrap()]);
    handle.join().unwrap();

    assert_eq!(out["journey"], "yaml-journey");
    assert_eq!(out["total"], 2, "both routes ran: {out}");
    assert_eq!(out["passed"], 2, "both routes passed: {out}");
    let outcomes = out["outcomes"].as_array().expect("outcomes is an array");
    assert!(
        outcomes.iter().all(|o| o["passed"] == true),
        "every outcome passed: {outcomes:?}"
    );
}

// ---- journey add soft intent resolution -----------------------------------
//
// `journey add` must not fail when step intents don't resolve to graph nodes.
// It should report unmatched steps and create the Validation node anyway.

#[test]
fn journey_add_tolerates_unresolved_intents() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // Only add ONE intent; leave the other unresolvable.
    let _known = intent(&store, "register a person", "implemented");
    drop(store);

    let spec_path = tmp.path().join("soft.contract.json");
    std::fs::write(
        &spec_path,
        serde_json::json!({
            "name": "soft-resolution",
            "base": "http://127.0.0.1:0",
            "routes": [
                {
                    "method": "POST",
                    "path": "/v1/persons",
                    "intent": "register a person",
                    "success_status": 201
                },
                {
                    "method": "GET",
                    "path": "/v1/unknown",
                    "intent": "Consumer records a verified peer vouch through the four-method seam",
                    "success_status": 200
                }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let out = run_cli_json(tmp.path(), &["journey", "add", spec_path.to_str().unwrap()]);
    assert!(out["added"] == true, "journey add succeeded: {out}");
    assert_eq!(out["linked_steps"], 1, "one intent resolved: {out}");
    let unmatched = out["unmatched_steps"].as_array().unwrap();
    assert_eq!(unmatched.len(), 1, "one step unmatched: {out}");
    assert_eq!(
        unmatched[0]["intent"],
        "Consumer records a verified peer vouch through the four-method seam",
        "the unmatched intent is reported: {unmatched:?}"
    );

    // The Validation node exists and is usable despite the unmatched step.
    let store = Store::open(tmp.path()).unwrap();
    let journey = store
        .resolve_node("soft-resolution", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(journey.status, "not_run");
    let validates = store
        .edges_with(Some(EdgeKind::Validates), Some(&journey.id), None)
        .unwrap();
    assert_eq!(validates.len(), 1, "only the resolved intent is linked");
}

// ---- journey diagnose: --base-url override + clear no-base error -----------

/// Contract: a legacy `saga:` spec whose `base` is unset (no env var, no field)
/// fails fast with an actionable error naming the fix — not a bare "builder error".
#[test]
fn journey_diagnose_reports_clear_error_when_base_unresolved() {
    let tmp = Tmp::new();
    let spec_path = tmp.path().join("no-base.json");
    std::fs::write(
        &spec_path,
        serde_json::json!({
            "saga": "no-base-journey",
            "steps": [
                { "name": "ping", "intent": "ping", "request": { "method": "GET", "url": "/ping" } }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let mut cmd = std::process::Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom")));
    cmd.args(["journey", "diagnose", spec_path.to_str().unwrap()]);
    // Ensure BASE_URL is not inherited from the test environment.
    cmd.env_remove("BASE_URL");
    let out = cmd.output().unwrap();
    assert!(!out.status.success(), "must fail when base cannot resolve");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no usable base URL") && stderr.contains("--base-url"),
        "error names the fix: {stderr}"
    );
}

/// Contract: `--base-url` overrides an unresolved/absent `base` field and lets
/// the journey actually run against a real server.
#[test]
fn journey_diagnose_base_url_flag_overrides_spec() {
    let tmp = Tmp::new();
    let (base, handle) = mock_server_handling(1, |_req| (200, r#"{"ok":true}"#.into()));

    let spec_path = tmp.path().join("override-base.json");
    std::fs::write(
        &spec_path,
        serde_json::json!({
            "journey": "override-base-journey",
            "steps": [
                { "name": "ping", "intent": "ping", "request": { "method": "GET", "url": "/ping" } }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let out = run_loom_json(&[
        "journey",
        "diagnose",
        spec_path.to_str().unwrap(),
        "--base-url",
        &base,
    ]);
    handle.join().unwrap();

    assert_eq!(
        out["passed"], 1,
        "the overridden base reached the server: {out}"
    );
    assert_eq!(out["failed"], 0, "{out}");
}

// ---- expect-side variable interpolation -------------------------------------
//
// A captured value threaded into a subsequent request BODY already worked;
// the bug was that the same `{{ var }}` inside `expect.body` (the assertion
// side) was compared literally instead of interpolated first — so "assert the
// response echoes back what we sent" could never pass without hardcoding.

/// Contract (graph-free `journey diagnose` path): a captured var referenced inside
/// `expect.body` is interpolated before comparison, so an echo-back assertion
/// against the actual captured value passes.
#[test]
fn journey_diagnose_interpolates_captured_vars_in_expect_body() {
    let tmp = Tmp::new();
    let (base, handle) = mock_server_handling(2, |req| {
        if req.starts_with("POST") {
            (201, r#"{"person_id":"p-77"}"#.into())
        } else {
            (200, r#"{"subject_person_id":"p-77"}"#.into())
        }
    });

    let spec_path = tmp.path().join("echo.json");
    std::fs::write(
        &spec_path,
        serde_json::json!({
            "journey": "echo-journey",
            "base": base,
            "steps": [
                {
                    "name": "create",
                    "intent": "create resource",
                    "request": { "method": "POST", "url": "/resources" },
                    "expect": { "status": 201 },
                    "capture": { "person_id": "$.person_id" }
                },
                {
                    "name": "verify-echo",
                    "intent": "verify echo",
                    "request": { "method": "GET", "url": "/resources/{{ person_id }}" },
                    "expect": { "body": { "$.subject_person_id": "{{ person_id }}" } }
                }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let out = run_loom_json(&["journey", "diagnose", spec_path.to_str().unwrap()]);
    handle.join().unwrap();

    assert_eq!(out["total"], 2, "{out}");
    assert_eq!(
        out["passed"], 2,
        "the echo assertion must resolve {{{{ person_id }}}} before comparing: {out}"
    );
}

/// Contract (graph-linked `journey run` path): the same interpolation fix applies
/// to `src/journey.rs::check_response`, exercised via `journey add` + `journey run`.
#[test]
fn journey_run_interpolates_captured_vars_in_expect_body() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let _create = intent(&store, "create resource", "implemented");
    let _verify = intent(&store, "verify echo", "implemented");
    drop(store);

    let (base, handle) = mock_server_handling(2, |req| {
        if req.starts_with("POST") {
            (201, r#"{"person_id":"p-77"}"#.into())
        } else {
            (200, r#"{"subject_person_id":"p-77"}"#.into())
        }
    });

    let spec_path = tmp.path().join("echo-journey.json");
    std::fs::write(
        &spec_path,
        serde_json::json!({
            "journey": "echo-journey",
            "base": base,
            "steps": [
                {
                    "name": "create",
                    "intent": "create resource",
                    "request": { "method": "POST", "url": "/resources" },
                    "expect": { "status": 201 },
                    "capture": { "person_id": "$.person_id" }
                },
                {
                    "name": "verify-echo",
                    "intent": "verify echo",
                    "request": { "method": "GET", "url": "/resources/{{ person_id }}" },
                    "expect": { "body": { "$.subject_person_id": "{{ person_id }}" } }
                }
            ]
        })
        .to_string(),
    )
    .unwrap();

    run_cli(tmp.path(), &["journey", "add", spec_path.to_str().unwrap()]);
    let out = run_cli_json(tmp.path(), &["journey", "run", spec_path.to_str().unwrap()]);
    handle.join().unwrap();

    assert_eq!(out["total"], 2, "{out}");
    assert_eq!(
        out["passed"], 2,
        "the echo assertion must resolve {{{{ person_id }}}} before comparing: {out}"
    );

    let store = Store::open(tmp.path()).unwrap();
    let journey = store
        .resolve_node("echo-journey", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(
        journey.status, "passed",
        "both steps passed, journey is passed"
    );
}

// ---- contract format: single-brace path params + verified-field detail -----
//
// The HTTP contract format uses OpenAPI/REST-style `{person_id}` in path
// templates (not loom's canonical `{{ person_id }}`). A route's `extract`
// captures a value from one route; a later route's path references it via
// the single-brace form. This must thread through exactly like the journey
// format's `{{ var }}`, and a passing step's detail should name which
// response fields were actually verified — not just "status 200 ok".

/// Contract: `loom journey diagnose <contract.json>` normalizes `{person_id}` in
/// a later route's path to the value captured by an earlier route's
/// `extract`, and the passing detail names the verified response fields.
#[test]
fn journey_diagnose_contract_format_substitutes_single_brace_path_params() {
    let tmp = Tmp::new();
    let (base, handle) = mock_server_handling(2, |req| {
        if req.starts_with("POST") {
            (200, r#"{"person_id":"p-1"}"#.into())
        } else {
            // Fails the test (via detail) if the path param wasn't substituted:
            // the literal, URL-encoded "{person_id}" would appear in the path.
            assert!(
                req.contains("GET /v1/grid/standing/p-1?context=research"),
                "path param must be substituted with the captured value, got: {req}"
            );
            (200, r#"{"subject_person_id":"p-1","headline":"ok"}"#.into())
        }
    });

    let spec_path = tmp.path().join("contract-path-param.json");
    std::fs::write(
        &spec_path,
        serde_json::json!({
            "name": "contract-path-param",
            "base": base,
            "routes": [
                {
                    "method": "POST",
                    "path": "/v1/grid/resolve",
                    "success_status": 200,
                    "extract": [{ "field": "person_id", "as": "person_id" }],
                    "response_fields": ["person_id"]
                },
                {
                    "method": "GET",
                    "path": "/v1/grid/standing/{person_id}",
                    "success_status": 200,
                    "query": { "context": "research" },
                    "response_fields": ["subject_person_id", "headline"]
                }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let out = run_loom_json(&["journey", "diagnose", spec_path.to_str().unwrap()]);
    handle.join().unwrap();

    assert_eq!(out["total"], 2, "{out}");
    assert_eq!(out["passed"], 2, "{out}");
    let outcomes = out["outcomes"].as_array().unwrap();
    let first_detail = outcomes[0]["detail"].as_str().unwrap();
    let second_detail = outcomes[1]["detail"].as_str().unwrap();
    assert!(
        first_detail.contains("verified: $.person_id"),
        "success detail names verified fields: {first_detail}"
    );
    assert!(
        second_detail.contains("verified:")
            && second_detail.contains("$.subject_person_id")
            && second_detail.contains("$.headline"),
        "success detail names verified fields: {second_detail}"
    );
}

/// Contract: a journey-format spec with no `expect.exists`/`expect.body` keeps
/// the plain "status N" detail — the verified-fields addition must not
/// clutter a step that asserted nothing about the body.
#[test]
fn journey_diagnose_detail_stays_plain_when_no_body_expectations() {
    let tmp = Tmp::new();
    let (base, handle) = mock_server_handling(1, |_req| (200, r#"{"ok":true}"#.into()));

    let spec_path = tmp.path().join("plain.json");
    std::fs::write(
        &spec_path,
        serde_json::json!({
            "journey": "plain-journey",
            "base": base,
            "steps": [
                { "name": "ping", "intent": "ping", "request": { "method": "GET", "url": "/ping" } }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let out = run_loom_json(&["journey", "diagnose", spec_path.to_str().unwrap()]);
    handle.join().unwrap();

    let detail = out["outcomes"][0]["detail"].as_str().unwrap();
    assert_eq!(
        detail, "status 200 ok",
        "no expectations: plain detail, got: {detail}"
    );
}

// ---- journey invariant update ------------------------------------------------

/// Run `loom --graph <tmp> <args>` and return (status, stdout, stderr) without
/// asserting on exit code — used for error-path assertions where a non-zero
/// exit is the contract under test.
fn run_cli_raw(tmp: &Path, args: &[&str]) -> (std::process::ExitStatus, String, String) {
    let mut cmd = std::process::Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom")));
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {:?}: {e}", args));
    (
        out.status,
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Contract: `journey invariant update --asserts <B>` re-points the invariant's
/// Asserts edge to intent B, preserving the invariant node id and recording a
/// decision note that mentions "re-pointed journey invariant". The old Asserts
/// edge to A is gone.
#[test]
fn journey_invariant_update_repoints_asserts_edge_preserving_node_id() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "intent A", "implemented");
    let b = intent(&store, "intent B", "implemented");
    drop(store);

    run_cli(
        tmp.path(),
        &[
            "journey",
            "invariant",
            "add",
            "--name",
            "inv1",
            &a,
            "--field",
            "f",
            "--assertion",
            "x > 0",
            "--reason",
            "r",
        ],
    );

    // Capture the invariant id as it exists right after add.
    let after_add = run_cli_json(tmp.path(), &["journey", "invariant", "list"]);
    let added = after_add
        .as_array()
        .expect("invariant list --json emits an array")
        .iter()
        .find(|r| r["name"] == "inv1")
        .expect("inv1 present after add");
    let inv_id = added["id"]
        .as_str()
        .expect("invariant row has id")
        .to_string();
    assert_eq!(
        added["asserts"],
        "intent A",
        "after add, invariant asserts intent A, got: {}",
        serde_json::to_string_pretty(&added).unwrap()
    );

    // Re-point to B.
    run_cli(
        tmp.path(),
        &[
            "journey",
            "invariant",
            "update",
            "inv1",
            "--asserts",
            &b,
            "--reason",
            "wrong intent",
        ],
    );

    let after_update = run_cli_json(tmp.path(), &["journey", "invariant", "list"]);
    let updated = after_update
        .as_array()
        .expect("invariant list --json emits an array")
        .iter()
        .find(|r| r["id"] == inv_id)
        .expect("same invariant node id preserved across update");
    assert_eq!(
        updated["id"], inv_id,
        "update preserves the invariant node id (re-point lives on the edge)"
    );
    assert_eq!(
        updated["asserts"],
        "intent B",
        "after update, invariant asserts intent B, got: {}",
        serde_json::to_string_pretty(&updated).unwrap()
    );

    // Exactly one Asserts edge from the invariant now (the old A edge was deleted,
    // not orphaned alongside the new B edge). `list` only surfaces the first
    // Asserts edge, so verify the count at the store level to defend this.
    let store = Store::open(tmp.path()).unwrap();
    let asserts_edges = store
        .edges_with(Some(EdgeKind::Asserts), Some(&inv_id), None)
        .unwrap();
    assert_eq!(
        asserts_edges.len(),
        1,
        "re-point replaces the Asserts edge (1 expected), got: {}",
        asserts_edges.len()
    );
    assert_eq!(
        asserts_edges[0].to_id, b,
        "the single Asserts edge points at intent B"
    );
    drop(store);

    // A decision note was added recording the re-point.
    let notes = run_cli_json(tmp.path(), &["note", "list", &inv_id]);
    let arr = notes.as_array().expect("note list --json emits an array");
    let re_pointed = arr.iter().find(|n| {
        n["text"]
            .as_str()
            .is_some_and(|t| t.contains("re-pointed journey invariant"))
    });
    assert!(
        re_pointed.is_some(),
        "a decision note mentioning 're-pointed journey invariant' must exist on the invariant, got: {}",
        serde_json::to_string_pretty(&notes).unwrap()
    );
    let note = re_pointed.unwrap();
    assert_eq!(
        note["kind"].as_str(),
        Some("decision"),
        "the re-point note is a decision note, got: {}",
        serde_json::to_string_pretty(&note).unwrap()
    );
    assert!(
        note["text"].as_str().unwrap().contains("intent B"),
        "the re-point note names the new target intent B, got: {}",
        serde_json::to_string_pretty(&note).unwrap()
    );
    assert!(
        note["text"].as_str().unwrap().contains("intent A"),
        "the re-point note records the prior target intent A, got: {}",
        serde_json::to_string_pretty(&note).unwrap()
    );
}

/// Contract: re-pointing `--asserts` at the intent the invariant already asserts
/// is idempotent — exactly one Asserts edge remains, no duplicate is created, and
/// no old edge is deleted (there was none to delete).
#[test]
fn journey_invariant_update_repoint_to_current_intent_is_idempotent() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "intent A", "implemented");
    let b = intent(&store, "intent B", "implemented");
    drop(store);

    run_cli(
        tmp.path(),
        &[
            "journey",
            "invariant",
            "add",
            "--name",
            "inv2",
            &a,
            "--field",
            "f",
            "--assertion",
            "x > 0",
            "--reason",
            "r",
        ],
    );
    run_cli(
        tmp.path(),
        &[
            "journey",
            "invariant",
            "update",
            "inv2",
            "--asserts",
            &b,
            "--reason",
            "first repoint",
        ],
    );

    // Snapshot the single Asserts edge after the first real re-point.
    let store = Store::open(tmp.path()).unwrap();
    let inv2 = store
        .resolve_node("inv2", Some(NodeType::JourneyInvariantPoint))
        .unwrap();
    let edges_before = store
        .edges_with(Some(EdgeKind::Asserts), Some(&inv2.id), None)
        .unwrap();
    assert_eq!(
        edges_before.len(),
        1,
        "baseline: one Asserts edge after first repoint"
    );
    drop(store);

    // Now re-point to the SAME intent B again.
    run_cli(
        tmp.path(),
        &[
            "journey",
            "invariant",
            "update",
            "inv2",
            "--asserts",
            &b,
            "--reason",
            "duplicate repoint",
        ],
    );

    // List still shows B exactly once (list takes the first Asserts edge, so a
    // duplicate would be hidden here — hence the store-level count below).
    let list = run_cli_json(tmp.path(), &["journey", "invariant", "list"]);
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "inv2")
        .unwrap();
    assert_eq!(
        row["asserts"],
        "intent B",
        "list still shows intent B once, got: {}",
        serde_json::to_string_pretty(&row).unwrap()
    );

    // Store-level invariant: still exactly one Asserts edge (no duplicate
    // created by re-pointing at the already-asserted intent). `list` only
    // surfaces the first Asserts edge, so a duplicate would be invisible there
    // — the count is what defends the no-duplicate contract.
    let store = Store::open(tmp.path()).unwrap();
    let edges_after = store
        .edges_with(Some(EdgeKind::Asserts), Some(&inv2.id), None)
        .unwrap();
    assert_eq!(
        edges_after.len(),
        1,
        "idempotent re-point leaves exactly one Asserts edge (no duplicate), got: {}",
        edges_after.len()
    );
    assert_eq!(
        edges_after[0].to_id, b,
        "the single edge still points at intent B"
    );
}

/// Contract: an update with ONLY `--reason` (no --field/--assertion/--asserts/
/// --reason-text) is rejected with a non-zero exit and a message naming the
/// missing update fields. The reason is otherwise valid, so this is the
/// "nothing to update" guard, not the empty-reason guard.
#[test]
fn journey_invariant_update_with_only_reason_exits_nonzero() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "intent A", "implemented");
    drop(store);

    run_cli(
        tmp.path(),
        &[
            "journey",
            "invariant",
            "add",
            "--name",
            "inv3",
            &a,
            "--field",
            "f",
            "--assertion",
            "x > 0",
            "--reason",
            "r",
        ],
    );

    let (status, _stdout, stderr) = run_cli_raw(
        tmp.path(),
        &[
            "journey",
            "invariant",
            "update",
            "inv3",
            "--reason",
            "just a reason, no fields",
        ],
    );
    assert!(
        !status.success(),
        "update with only --reason must exit non-zero, got: {status:?}\n--stderr--\n{stderr}"
    );
    assert!(
        stderr.contains("nothing to update"),
        "stderr must mention 'nothing to update' so the operator knows which flags to pass, got: {stderr}"
    );
}

// ---- note targets: edges, node precedence, and the no-match error ----------

/// Contract: a note can be attached to an EDGE (by id or prefix) and scoped
/// `note list` returns exactly that note with `target_id` equal to the full
/// edge id. Adjudications live on claims, and claims live on edges too.
#[test]
fn note_add_attaches_to_edge_and_list_scopes_to_edge() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let _a = intent(&store, "intent alpha", "implemented");
    let _b = intent(&store, "intent beta", "implemented");
    drop(store);

    // Create the relates edge via the surface under test and capture its id.
    let relate = run_cli_json(
        tmp.path(),
        &["edge", "relate", "relates", "intent alpha", "intent beta"],
    );
    let edge_id = relate["edge"]["id"]
        .as_str()
        .expect("edge relate --json emits the edge with its id")
        .to_string();
    assert!(
        !edge_id.is_empty(),
        "relate must produce a non-empty edge id, got: {relate}"
    );
    let prefix = &edge_id[..8];

    // Attach a warning note by the edge id PREFIX (the resolution path that
    // distinguishes edges from nodes must accept the short form too).
    let added = run_cli_json(
        tmp.path(),
        &[
            "note",
            "add",
            prefix,
            "--kind",
            "warning",
            "--text",
            "verdict recorded from wrong lane",
        ],
    );
    assert_eq!(
        added["target"]["id"].as_str(),
        Some(edge_id.as_str()),
        "note add resolves the prefix to the full edge id, got: {}",
        serde_json::to_string_pretty(&added).unwrap()
    );

    // note list scoped to the edge returns exactly that note, with target_id
    // equal to the FULL edge id (not the prefix we passed).
    let notes = run_cli_json(tmp.path(), &["note", "list", &edge_id]);
    let arr = notes.as_array().expect("note list --json emits an array");
    assert_eq!(
        arr.len(),
        1,
        "exactly one note scoped to the edge, got: {}",
        serde_json::to_string_pretty(&notes).unwrap()
    );
    assert_eq!(
        arr[0]["target_id"].as_str(),
        Some(edge_id.as_str()),
        "the scoped note's target_id is the full edge id, got: {}",
        serde_json::to_string_pretty(&arr[0]).unwrap()
    );
    assert_eq!(
        arr[0]["kind"].as_str(),
        Some("warning"),
        "the note kind round-trips as warning, got: {}",
        serde_json::to_string_pretty(&arr[0]).unwrap()
    );
    assert_eq!(
        arr[0]["text"].as_str(),
        Some("verdict recorded from wrong lane"),
        "the note text is preserved verbatim, got: {}",
        serde_json::to_string_pretty(&arr[0]).unwrap()
    );
}

/// Contract: node precedence — when the target string names a node (here an
/// intent), the note lands on the node even though edges exist in the graph.
/// `resolve_note_target` tries nodes first and only falls through to edges on a
/// hard "no node matches", so a name that resolves a node must never be
/// misread as an edge prefix.
#[test]
fn note_add_on_node_name_lands_on_node_not_edge() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "intent alpha", "implemented");
    let _b = intent(&store, "intent beta", "implemented");
    drop(store);

    // Add a relates edge so edges exist in the graph — proving the node path
    // is chosen by precedence, not by absence of edges.
    run_cli(
        tmp.path(),
        &["edge", "relate", "relates", "intent alpha", "intent beta"],
    );

    let added = run_cli_json(
        tmp.path(),
        &["note", "add", "intent alpha", "--text", "node note"],
    );
    assert_eq!(
        added["target"]["id"].as_str(),
        Some(a.as_str()),
        "the note attached to the intent node, not an edge, got: {}",
        serde_json::to_string_pretty(&added).unwrap()
    );

    // Scoped list on the node returns the note; scoped list on the edge must
    // NOT see it — the node note did not leak onto the edge.
    let node_notes = run_cli_json(tmp.path(), &["note", "list", "intent alpha"]);
    let arr = node_notes
        .as_array()
        .expect("note list --json emits an array");
    assert_eq!(
        arr.len(),
        1,
        "one note scoped to the intent node, got: {}",
        serde_json::to_string_pretty(&node_notes).unwrap()
    );
    assert_eq!(
        arr[0]["target_id"].as_str(),
        Some(a.as_str()),
        "the note's target_id is the intent node id, got: {}",
        serde_json::to_string_pretty(&arr[0]).unwrap()
    );
}

/// Contract: a target that matches neither a node nor an edge exits non-zero
/// with a message containing "no node or edge matches" — the single error that
/// tells the operator the target could not be resolved at all.
#[test]
fn note_add_dead_target_exits_nonzero_with_no_match_message() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let _a = intent(&store, "intent alpha", "implemented");
    drop(store);

    // "deadbeef99" cannot be a node name (no node is named that), a node id
    // prefix, or an edge id prefix in this graph.
    let (status, _stdout, stderr) =
        run_cli_raw(tmp.path(), &["note", "add", "deadbeef99", "--text", "x"]);
    assert!(
        !status.success(),
        "note add on an unresolvable target must exit non-zero, got: {status:?}\n--stderr--\n{stderr}"
    );
    assert!(
        stderr.contains("no node or edge matches"),
        "stderr must name the no-match contract so the operator knows the target resolved to nothing, got: {stderr}"
    );
}
