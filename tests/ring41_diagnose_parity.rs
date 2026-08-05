//! Ring 41 — dry-run parity: diagnose predicts run.
//!
//! Diagnose and the recorded run execute the SAME step semantics — one code
//! path, persistence the only flag. A diagnose success must predict a run
//! success; a diagnose failure must mean the run would fail. The historical
//! violation: an HTTP step with no declared status passed `run` on any 2xx
//! but failed `diagnose` unless it was exactly 200, so dry-run success (and
//! failure) predicted nothing.

use std::io::{Read, Write};
use std::net::TcpListener;
mod common;
use common::*;

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
            let _n = stream.read(&mut buf).expect("mock read");
            let resp = format!(
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(resp.as_bytes()).expect("mock write");
        }
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

/// The regression: a step with NO declared status and a 201 response.
/// Run semantics accept any 2xx; diagnose must agree.
#[test]
fn diagnose_accepts_any_2xx_when_no_status_is_declared() {
    let tmp = Tmp::new();
    let (base, handle) = mock_server(vec![(201, r#"{"id":"c1"}"#.into())]);
    let spec_path = tmp.path().join("create.yaml");
    std::fs::write(
        &spec_path,
        format!(
            r#"journey: create-resource
base: "{base}"
steps:
  - name: create it
    intent: a resource can be created
    request:
      method: POST
      url: /resources
"#
        ),
    )
    .unwrap();

    let out = std::process::Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom")))
        .args(["journey", "diagnose", spec_path.to_str().unwrap(), "--json"])
        .output()
        .expect("spawn loom journey diagnose");
    handle.join().unwrap();
    assert!(
        out.status.success(),
        "diagnose must accept 201 when no status is declared — run does: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(stdout["passed"], 1, "{stdout}");
    assert_eq!(stdout["failed"], 0, "{stdout}");
}

/// The other direction of parity: a step that fails diagnose fails run —
/// same check, same answer.
#[test]
fn diagnose_and_run_agree_on_a_status_mismatch() {
    let tmp = Tmp::new();
    let (base, handle) = mock_server(vec![(500, r#"{"error":"boom"}"#.into())]);
    let spec_path = tmp.path().join("fail.yaml");
    std::fs::write(
        &spec_path,
        format!(
            r#"journey: failing-journey
base: "{base}"
steps:
  - name: read it
    intent: a resource can be read
    request:
      method: GET
      url: /resources/1
"#
        ),
    )
    .unwrap();

    let out = std::process::Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom")))
        .args(["journey", "diagnose", spec_path.to_str().unwrap(), "--json"])
        .output()
        .expect("spawn loom journey diagnose");
    handle.join().unwrap();
    assert!(
        !out.status.success(),
        "a 500 against the 2xx default must fail diagnose exactly as it fails run"
    );
    let stdout: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(stdout["failed"], 1, "{stdout}");
    let detail = stdout["outcomes"][0]["detail"].as_str().unwrap();
    assert!(
        detail.contains("2xx"),
        "the failure names the unified default: {detail}"
    );
}

/// An unusable base fails fast in run too — previously only diagnose bailed
/// early while run surfaced an opaque HTTP client error.
#[test]
fn run_fails_fast_on_an_unusable_base_like_diagnose() {
    let tmp = Tmp::new();
    let store = loom::store::Store::init(tmp.path(), Some("t"), false).unwrap();
    // A registered journey so `run` gets past the registration check.
    let intent = store
        .add_node(
            loom::model::NodeType::Intent,
            "a resource can be created",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let journey = store
        .add_node(
            loom::model::NodeType::Validation,
            "no-base",
            "",
            "not_run",
            serde_json::json!({"type":"journey","journey_id":"no-base"}),
        )
        .unwrap();
    store
        .ensure_edge(loom::model::EdgeKind::Validates, &journey.id, &intent.id)
        .unwrap();
    drop(store);

    let spec_path = tmp.path().join("nobase.yaml");
    std::fs::write(
        &spec_path,
        r#"journey: no-base
base: ""
steps:
  - name: create it
    intent: a resource can be created
    request:
      method: POST
      url: /resources
"#,
    )
    .unwrap();

    let out = std::process::Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom")))
        .arg("--graph")
        .arg(tmp.path())
        .args(["journey", "run", spec_path.to_str().unwrap()])
        .output()
        .expect("spawn loom journey run");
    assert!(!out.status.success(), "an unusable base must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no usable base URL"),
        "run names the cause like diagnose does: {stderr}"
    );
}
