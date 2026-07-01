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
