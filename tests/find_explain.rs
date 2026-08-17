//! Find --tag/--where and explain neighborhood brief.

use std::path::Path;
use std::process::Command;

mod common;
use common::*;

fn loom_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom"))
}

fn loom_init(tmp: &Path) {
    let out = Command::new(loom_bin())
        .arg("init")
        .arg(tmp)
        .arg("--name")
        .arg("t")
        .output()
        .unwrap();
    assert!(out.status.success());
}

fn loom_ok(tmp: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "loom {:?} failed: {}\n{}",
        args,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn find_tag_and_where_and_explain() {
    let tmp = Tmp::new();
    loom_init(tmp.path());

    loom_ok(
        tmp.path(),
        &["vocab", "add", "auth", "--why", "authentication concern"],
    );
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "users can sign in",
            "--description",
            "auth succeeds",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
            "--level",
            "feature",
        ],
    );
    loom_ok(
        tmp.path(),
        &["intent", "tag", "add", "users can sign in", "auth"],
    );
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "internal hash ripple",
            "--description",
            "sync stales",
            "--lifecycle",
            "implemented",
            "--visibility",
            "internal",
        ],
    );

    let out = loom_ok(
        tmp.path(),
        &[
            "find",
            "--tag",
            "auth",
            "--where",
            "visibility=user_visible",
            "--json",
        ],
    );
    let rows: serde_json::Value = serde_json::from_str(&out).unwrap();
    let arr = rows.as_array().unwrap();
    assert_eq!(
        arr.len(),
        1,
        "expected only the tagged user_visible intent: {out}"
    );
    assert_eq!(arr[0]["name"], "users can sign in");

    let brief = loom_ok(tmp.path(), &["explain", "users can sign in", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&brief).unwrap();
    assert_eq!(v["intent"]["name"], "users can sign in");
    assert_eq!(v["intent"]["visibility"], "user_visible");
    assert!(v["completeness"]["axes"].is_array());
}

#[test]
fn find_exact_uses_the_grounding_aware_projection() {
    let tmp = Tmp::new();
    loom_init(tmp.path());

    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "users can sign in",
            "--description",
            "auth succeeds",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
        ],
    );
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/auth.rs"), "fn sign_in() {}\n").unwrap();
    loom_ok(tmp.path(), &["codefile", "add", "src/auth.rs"]);
    let grounding = loom_ok(
        tmp.path(),
        &[
            "edge",
            "implement",
            "users can sign in",
            "src/auth.rs",
            "--locator",
            "sign_in:12-30",
            "--json",
        ],
    );
    let grounding: serde_json::Value = serde_json::from_str(&grounding).unwrap();
    let edge_id = grounding["edge"]["id"]
        .as_str()
        .expect("edge implement returns the grounding id");
    let evidence_sentinel = "reviewed sign-in grounding implementation";
    loom_ok(
        tmp.path(),
        &[
            "edge",
            "verdict",
            edge_id,
            "ground",
            "--criterion",
            "the sign_in locator resolves the behavior's realizing implementation",
            "--evidence",
            evidence_sentinel,
        ],
    );

    let out = loom_ok(
        tmp.path(),
        &["find", "users can sign in", "--exact", "--json"],
    );
    let rows: serde_json::Value = serde_json::from_str(&out).unwrap();
    let hit = &rows.as_array().unwrap()[0];
    assert_eq!(hit["exact"], true, "exact compatibility field: {out}");
    assert_eq!(hit["groundings"][0]["path"], "src/auth.rs");
    assert_eq!(hit["groundings"][0]["locator"], "sign_in:12-30");
    assert_eq!(hit["groundings"][0]["role"], "realizes");
    assert_eq!(hit["groundings"][0]["status"], "passing");
    assert!(
        hit["groundings"][0]["evidence"]
            .as_str()
            .is_some_and(|evidence| evidence.contains(evidence_sentinel)),
        "exact result carries the grounding's verdict evidence: {out}"
    );
}

#[test]
fn find_exact_refuses_ambiguous_behavior_identity() {
    let tmp = Tmp::new();
    loom_init(tmp.path());

    let store = loom::store::Store::open(tmp.path()).unwrap();
    let first = store
        .add_node(
            loom::model::NodeType::Intent,
            "shared exact behavior",
            "first behavior",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let second = store
        .add_node(
            loom::model::NodeType::Intent,
            "shared exact behavior",
            "second behavior",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    drop(store);

    let output = Command::new(loom_bin())
        .arg("--graph")
        .arg(tmp.path())
        .args(["find", "shared exact behavior", "--exact", "--json"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "ambiguous exact identity must be refused"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!("--json refusal must be one error envelope, not result rows: {error}\n{stdout}")
    });
    assert_eq!(envelope["status"], "error", "{stdout}");
    assert!(
        envelope
            .as_object()
            .is_some_and(|object| !object.contains_key("exact")),
        "ambiguity must not emit a successful exact-match row: {stdout}"
    );
    let detail = envelope["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("ambiguous exact match for 'shared exact behavior'"),
        "JSON detail must explain the ambiguity: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ambiguous exact match for 'shared exact behavior'"),
        "error must explain the ambiguity: {stderr}"
    );
    for candidate in [&first, &second] {
        assert!(
            stderr.contains(&candidate.id[..8]),
            "error must list candidate short id {}: {stderr}",
            &candidate.id[..8]
        );
    }
}

#[test]
fn find_exact_prefers_intent_over_same_named_codefile() {
    let tmp = Tmp::new();
    loom_init(tmp.path());

    let store = loom::store::Store::open(tmp.path()).unwrap();
    let intent = store
        .add_node(
            loom::model::NodeType::Intent,
            "shared exact name",
            "the behavior",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_node(
            loom::model::NodeType::CodeFile,
            "shared exact name",
            "a file that happens to share the behavior name",
            "active",
            serde_json::json!({}),
        )
        .unwrap();
    drop(store);

    let output = Command::new(loom_bin())
        .arg("--graph")
        .arg(tmp.path())
        .args(["find", "shared exact name", "--exact", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Intent+CodeFile same name is not ambiguous behavior identity: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let rows = rows.as_array().expect("find --json is an array");
    assert_eq!(rows.len(), 1, "exact search should return the Intent only");
    assert_eq!(rows[0]["id"], intent.id);
    assert_eq!(rows[0]["kind"], "intent");
}
