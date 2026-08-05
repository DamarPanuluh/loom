//! Ring 43 — the graph survives refactors of its own subjects.
//!
//! A file rename/split used to orphan every edge pointing at the old file
//! node: `codefile remove` hard-deleted the groundings (and their verdict
//! history) or the operator kept a ghost registration that warned forever.
//! Now the move is one recorded graph operation: `loom edge retarget`
//! re-points an edge IN PLACE — id, locator, and verdict facts kept — and
//! `loom codefile remove --successor` cascades every live edge the same way
//! before the node goes. What still holds is decided by sync's normal
//! reverification: content that moved intact re-anchors and keeps its
//! verdict; content that changed is staled honestly. And removal without a
//! successor REFUSES, naming every blocker with its retarget command.

use loom::model::{EdgeKind, NodeType};
use loom::store::Store;
mod common;
use common::*;

fn loom_json(tmp: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let out = std::process::Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom")))
        .arg("--graph")
        .arg(tmp)
        .args(args)
        .arg("--json")
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {args:?}: {e}"));
    assert!(
        out.status.success(),
        "loom {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .unwrap_or_else(|e| panic!("no json: {e}"))
}

fn loom_fail(tmp: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom")))
        .arg("--graph")
        .arg(tmp)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {args:?}: {e}"));
    assert!(
        !out.status.success(),
        "loom {args:?} unexpectedly succeeded:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stderr).to_string()
}

const SUBJECT: &str = "pub fn subject() -> u8 {\n    42\n}\n";

/// An intent grounded in src/old.rs with a PASSING verdict on the edge.
/// Returns (edge_id8, old_file_node_id, intent_id). The store is dropped
/// before any CLI subprocess runs: the graph lock is single-holder, so an
/// open in-process store would contend with the spawned `loom`.
fn grounded_verdict(tmp: &Tmp) -> (String, String, String) {
    tmp.write("src/old.rs", SUBJECT);
    let (intent_id, old_id) = {
        let store = Store::open(tmp.path()).unwrap();
        let intent = store
            .add_node(
                NodeType::Intent,
                "the moved behavior",
                "d",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        let old = store
            .add_node(
                NodeType::CodeFile,
                "src/old.rs",
                "",
                "",
                serde_json::json!({}),
            )
            .unwrap();
        (intent.id, old.id)
    };
    let created = loom_json(
        tmp.path(),
        &[
            "edge",
            "implement",
            &intent_id,
            "src/old.rs",
            "--locator",
            "fn subject",
        ],
    );
    let edge_id = created["edge"]["id"].as_str().expect("edge id").to_string();
    loom_json(tmp.path(), &["sync"]);
    loom_json(
        tmp.path(),
        &[
            "edge",
            "verdict",
            &edge_id[..8],
            "ground",
            "--criterion",
            "subject returns the constant",
            "--evidence",
            "src/old.rs:@subject",
        ],
    );
    (edge_id[..8].to_string(), old_id, intent_id)
}

fn edge_show(tmp: &std::path::Path, id8: &str) -> serde_json::Value {
    loom_json(tmp, &["edge", "show", id8])
}

/// The composition P10 exists for: retarget the edge, remove the old file,
/// sync — the verdict SURVIVES because the content moved intact (P1's
/// re-anchoring does the rest; the move itself never forces a re-verdict).
#[test]
fn a_rename_keeps_its_verdict_through_retarget_and_sync() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let (id8, old_id, _intent) = grounded_verdict(&tmp);

    // The rename: same content, new path, successor registered.
    tmp.write("src/new.rs", SUBJECT);
    loom_json(tmp.path(), &["codefile", "add", "src/new.rs"]);

    let out = loom_json(
        tmp.path(),
        &[
            "edge",
            "retarget",
            &id8,
            "--to",
            "src/new.rs",
            "--reason",
            "renamed old.rs to new.rs",
        ],
    );
    assert!(!out["edge"]["to_id"].as_str().unwrap().is_empty());

    // The old node goes; the edge no longer blocks it.
    std::fs::remove_file(tmp.path().join("src/old.rs")).unwrap();
    loom_json(tmp.path(), &["codefile", "remove", "src/old.rs"]);
    let store = Store::open(tmp.path()).unwrap();
    assert!(
        store.get_node(&old_id).unwrap().is_none(),
        "old codefile node removed"
    );
    let new_id = store
        .resolve_node("src/new.rs", Some(NodeType::CodeFile))
        .unwrap()
        .id;
    drop(store);

    loom_json(tmp.path(), &["sync"]);
    let e = edge_show(tmp.path(), &id8);
    assert_eq!(
        e["to_id"].as_str().unwrap(),
        new_id,
        "edge re-pointed at the successor: {e}"
    );
    assert_eq!(
        e["status"].as_str().unwrap(),
        "passing",
        "content moved intact → verdict kept, no ceremonial re-verdict: {e}"
    );
}

/// `codefile remove --successor` expresses a split as ONE recorded
/// operation: every live edge retargeted in place, then the node removed.
#[test]
fn remove_with_successor_cascades_the_edges_in_place() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let (id8, old_id, _intent) = grounded_verdict(&tmp);

    tmp.write("src/new.rs", SUBJECT);
    loom_json(tmp.path(), &["codefile", "add", "src/new.rs"]);
    std::fs::remove_file(tmp.path().join("src/old.rs")).unwrap();

    let out = loom_json(
        tmp.path(),
        &[
            "codefile",
            "remove",
            "src/old.rs",
            "--successor",
            "src/new.rs",
        ],
    );
    assert!(out["removed"].as_bool().unwrap());
    assert_eq!(
        out["retargeted_edges"].as_array().unwrap(),
        &vec![serde_json::json!(id8)],
        "the one grounding edge cascaded: {out}"
    );

    let store = Store::open(tmp.path()).unwrap();
    assert!(store.get_node(&old_id).unwrap().is_none());
    let new_id = store
        .resolve_node("src/new.rs", Some(NodeType::CodeFile))
        .unwrap()
        .id;
    drop(store);

    loom_json(tmp.path(), &["sync"]);
    let e = edge_show(tmp.path(), &id8);
    assert_eq!(e["to_id"].as_str().unwrap(), new_id);
    assert_eq!(
        e["status"].as_str().unwrap(),
        "passing",
        "verdict history survived the cascade: {e}"
    );
}

/// Without --successor, removal REFUSES and names every blocker with its
/// retarget command — no silent orphaning, no ghost registration.
#[test]
fn remove_without_successor_refuses_and_lists_blockers() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let (id8, old_id, _intent) = grounded_verdict(&tmp);

    let err = loom_fail(tmp.path(), &["codefile", "remove", "src/old.rs"]);
    assert!(
        err.contains("cannot remove codefile 'src/old.rs'"),
        "names the file: {err}"
    );
    assert!(err.contains(&id8), "names the blocking edge: {err}");
    assert!(
        err.contains(&format!("loom edge retarget {id8} --to")),
        "names the remedy command: {err}"
    );
    assert!(err.contains("--successor"), "names the cascade flag: {err}");

    // Refusal, not partial damage: the node and its edge both survive.
    let store = Store::open(tmp.path()).unwrap();
    assert!(store.get_node(&old_id).unwrap().is_some());
    let e = store.resolve_edge(&id8).unwrap();
    assert_eq!(e.to_id, old_id);
}

/// A retarget that would silently duplicate a live bridge is refused and
/// the existing edge is named.
#[test]
fn retarget_refuses_to_duplicate_an_existing_bridge() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    tmp.write("src/a.rs", SUBJECT);
    tmp.write("src/b.rs", "pub fn other() {}\n");
    let intent_id = {
        let store = Store::open(tmp.path()).unwrap();
        let intent = store
            .add_node(
                NodeType::Intent,
                "dup",
                "d",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        codefile(&store, "src/a.rs");
        codefile(&store, "src/b.rs");
        intent.id
    };
    let first = loom_json(tmp.path(), &["edge", "implement", &intent_id, "src/a.rs"]);
    let second = loom_json(tmp.path(), &["edge", "implement", &intent_id, "src/b.rs"]);
    let first8 = first["edge"]["id"].as_str().unwrap()[..8].to_string();
    let second8 = second["edge"]["id"].as_str().unwrap()[..8].to_string();

    let err = loom_fail(
        tmp.path(),
        &[
            "edge",
            "retarget",
            &second8,
            "--to",
            "src/a.rs",
            "--reason",
            "consolidating",
        ],
    );
    assert!(err.contains("already targets"), "says why: {err}");
    assert!(
        err.contains(&first8),
        "names the edge it would duplicate: {err}"
    );
}

/// Endpoint typing survives retarget: an implements edge cannot be
/// re-pointed at another intent.
#[test]
fn retarget_refuses_a_target_of_the_wrong_type() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    tmp.write("src/a.rs", SUBJECT);
    let (intent_id, other_id) = {
        let store = Store::open(tmp.path()).unwrap();
        let intent = store
            .add_node(
                NodeType::Intent,
                "typed",
                "d",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        let other = store
            .add_node(
                NodeType::Intent,
                "not a file",
                "d",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        codefile(&store, "src/a.rs");
        (intent.id, other.id)
    };
    let created = loom_json(tmp.path(), &["edge", "implement", &intent_id, "src/a.rs"]);
    let id8 = created["edge"]["id"].as_str().unwrap()[..8].to_string();

    let err = loom_fail(
        tmp.path(),
        &[
            "edge",
            "retarget",
            &id8,
            "--to",
            &other_id,
            "--reason",
            "wrong target",
        ],
    );
    assert!(
        err.contains("requires to-node type 'codefile'"),
        "the registry's endpoint rule is enforced: {err}"
    );
}

/// Derived edges are sync-owned: retargeting one is refused at the store
/// gate, not patched around.
#[test]
fn retarget_refuses_a_derived_edge() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    tmp.write("src/a.rs", SUBJECT);
    tmp.write("src/b.rs", SUBJECT);
    let cf_a = codefile(&store, "src/a.rs");
    let cf_b = codefile(&store, "src/b.rs");
    let finding = store
        .add_derived_node(
            NodeType::Finding,
            "finding:test-derived",
            "a derived finding",
            "d",
            "open",
            serde_json::json!({}),
        )
        .unwrap();
    let de = store
        .add_derived_edge(EdgeKind::Flags, &finding.id, &cf_a.id)
        .unwrap();
    let err = store
        .retarget_edge(&de.id, &cf_b.id, "move it")
        .unwrap_err();
    assert!(
        err.to_string().contains("derived"),
        "derived edges are sync-owned: {err}"
    );
}

/// An ungrounded file still removes exactly as before — the refusal only
/// guards live claims.
#[test]
fn remove_without_edges_still_works() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let cf = codefile(&store, "src/lonely.rs");
    drop(store);

    let out = loom_json(tmp.path(), &["codefile", "remove", "src/lonely.rs"]);
    assert!(out["removed"].as_bool().unwrap());
    let store = Store::open(tmp.path()).unwrap();
    assert!(store.get_node(&cf.id).unwrap().is_none());
}

/// A reason is the audit of why the subject moved — a retarget without one
/// is refused.
#[test]
fn retarget_requires_a_reason() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    tmp.write("src/a.rs", SUBJECT);
    tmp.write("src/b.rs", SUBJECT);
    let intent = store
        .add_node(
            NodeType::Intent,
            "reasoned",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf_a = codefile(&store, "src/a.rs");
    let cf_b = codefile(&store, "src/b.rs");
    let e = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf_a.id,
            loom::model::TruthClass::Asserted,
        )
        .unwrap();
    let err = store.retarget_edge(&e.id, &cf_b.id, "   ").unwrap_err();
    assert!(
        err.to_string().contains("--reason"),
        "a blank reason is no audit: {err}"
    );
}
