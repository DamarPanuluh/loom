//! Ring 6 tests — smells (structural), debt (statistical, never stored),
//! doctor (integrity), and a live saga run against a mock HTTP server.

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

// ---- live saga run ---------------------------------------------------------

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
fn saga_run_stamps_passing_steps() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let cart = intent(&store, "cart can be created", "implemented");
    let pay = intent(&store, "payment can be captured", "implemented");
    // saga validation + validates edges (as `loom saga add` would create)
    let saga = store
        .add_node(
            NodeType::Validation,
            "checkout-flow",
            "",
            "not_run",
            serde_json::json!({"type":"saga"}),
        )
        .unwrap();
    store
        .ensure_edge(EdgeKind::Validates, &saga.id, &cart)
        .unwrap();
    store
        .ensure_edge(EdgeKind::Validates, &saga.id, &pay)
        .unwrap();

    let (base, handle) = mock_server(vec![
        (201, r#"{"id":"c1"}"#.into()),
        (200, r#"{"state":"paid"}"#.into()),
    ]);
    let spec = loom::saga::SagaSpec {
        saga: "checkout-flow".into(),
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
    let outcomes = loom::saga::execute(&store, &spec, true).unwrap();
    handle.join().unwrap();
    assert_eq!(outcomes.len(), 2);
    assert!(
        outcomes.iter().all(|o| o.passed),
        "both steps should pass: {outcomes:?}"
    );

    // both validates edges stamped passing; saga node passed
    for intent_id in [&cart, &pay] {
        let e = store
            .edges_with(Some(EdgeKind::Validates), Some(&saga.id), Some(intent_id))
            .unwrap();
        assert_eq!(e[0].status, InspectionStatus::Passing);
    }
    assert_eq!(store.get_node(&saga.id).unwrap().unwrap().status, "passed");
}

// ---- HTTP contract JSON → saga run ----------------------------------------
//
// These exercise the `loom saga run` contract for an HTTP-contract spec
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
/// assert it exits zero (the saga add/run wiring under test).
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

/// Contract: an HTTP-contract JSON with two routes runs through the saga
/// runner. Route 1 extracts `person_id`; route 2 threads it into the path,
/// a query param, and the JSON body, and asserts `response_fields` existence.
/// The mock conditions route 2's success on the extracted id appearing in all
/// three places — a broken interpolation reddens this.
#[test]
fn http_contract_runs_two_routes_threading_extracted_id() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // intents the routes declare (saga add would link these)
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

    // `saga run` requires a pre-existing Validation node named after the
    // contract's `name`; `saga add` creates it (and the step edges).
    run_cli(tmp.path(), &["saga", "add", spec_path.to_str().unwrap()]);
    let out = run_cli_json(tmp.path(), &["saga", "run", spec_path.to_str().unwrap()]);
    handle.join().unwrap();

    assert_eq!(out["saga"], "sample-service-http");
    assert_eq!(out["total"], 2, "both routes ran: {out}");
    assert_eq!(out["passed"], 2, "both routes passed: {out}");
    let outcomes = out["outcomes"].as_array().expect("outcomes is an array");
    assert_eq!(outcomes.len(), 2);
    assert!(
        outcomes.iter().all(|o| o["passed"] == true),
        "every outcome passed: {outcomes:?}"
    );

    let store = Store::open(tmp.path()).unwrap();
    // both validates edges stamped passing; saga node passed
    let saga = store
        .resolve_node("sample-service-http", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(saga.status, "passed");
    let validates = store
        .edges_with(Some(EdgeKind::Validates), Some(&saga.id), None)
        .unwrap();
    assert_eq!(validates.len(), 2, "saga add linked both route intents");
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

    run_cli(tmp.path(), &["saga", "add", spec_path.to_str().unwrap()]);
    let out = run_cli_json(tmp.path(), &["saga", "run", spec_path.to_str().unwrap()]);
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
    // the failing route stamps its validates edge failing; saga node failed
    let saga = store
        .resolve_node("missing-field-http", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(saga.status, "failed");
    let validates = store
        .edges_with(Some(EdgeKind::Validates), Some(&saga.id), None)
        .unwrap();
    assert_eq!(validates.len(), 1, "the one route's edge was linked");
    assert_eq!(
        validates[0].status,
        InspectionStatus::Failing,
        "the failing route's edge is failing"
    );
}
