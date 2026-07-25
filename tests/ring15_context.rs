//! Ring 15 — operator context packets and queryable provenance facets.
//!
//! Real CLI/store coverage for the read-only `loom context` packet and the
//! fail-closed `find --where ratification=unratified` filter.

use loom::model::NodeType;
use loom::store::Store;
use std::path::{Path, PathBuf};
use std::process::Command;
mod common;
use common::Tmp;

fn loom_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_loom"))
}

fn loom_ok(root: &Path, args: &[&str]) -> String {
    let output = Command::new(loom_bin())
        .arg("--graph")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "loom {:?} failed:\nstderr: {}\nstdout: {}",
        args,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn loom_json(root: &Path, args: &[&str]) -> serde_json::Value {
    serde_json::from_str(&loom_ok(root, &[args, &["--json"]].concat())).unwrap()
}

fn init(root: &Path) {
    let output = Command::new(loom_bin())
        .arg("init")
        .arg(root)
        .arg("--name")
        .arg("t")
        .output()
        .unwrap();
    assert!(output.status.success());
}

fn add_intent(root: &Path, name: &str) {
    loom_ok(
        root,
        &[
            "intent",
            "add",
            "--name",
            name,
            "--description",
            "a context packet behavior",
            "--lifecycle",
            "implemented",
        ],
    );
}

fn entities(packet: &serde_json::Value) -> &[serde_json::Value] {
    packet["context"]["linked_entities"]
        .as_array()
        .expect("context packets serialize TraversalContext linked entities")
}

#[test]
fn ratification_find_filter_includes_facetless_intents_only() {
    let tmp = Tmp::new();
    init(tmp.path());
    // Minted with a person present, so it is born ratified and the filter has
    // exactly one unratified intent to find.
    std::env::set_var("LOOM_PRESENCE_PROBE", "human");
    add_intent(tmp.path(), "solo mint is ratified");
    std::env::remove_var("LOOM_PRESENCE_PROBE");
    let store = Store::open(tmp.path()).unwrap();
    let facetless = store
        .add_node(
            NodeType::Intent,
            "legacy intent lacks a ratification facet",
            "a migrated intent",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    drop(store);

    let found = loom_json(tmp.path(), &["find", "--where", "ratification=unratified"]);
    let rows = found.as_array().expect("find JSON is an array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], facetless.id);
}

#[test]
fn file_context_lists_owning_intents_and_grounding_locator() {
    let tmp = Tmp::new();
    tmp.write("src/context_target.rs", "pub fn packet() {}\n");
    init(tmp.path());
    add_intent(tmp.path(), "operators can inspect context");
    loom_ok(tmp.path(), &["codefile", "add", "src/context_target.rs"]);
    loom_ok(
        tmp.path(),
        &[
            "edge",
            "implement",
            "operators can inspect context",
            "src/context_target.rs",
            "--locator",
            "packet",
        ],
    );

    let packet = loom_json(tmp.path(), &["context", "src/context_target.rs"]);
    assert!(entities(&packet).iter().any(|entity| {
        entity["role"] == "owning_intent" && entity["name"] == "operators can inspect context"
    }));
    assert!(entities(&packet)
        .iter()
        .any(|entity| { entity["role"] == "grounding" && entity["locator"] == "packet" }));
}

#[test]
fn intent_context_includes_validation_last_result_and_edge_state() {
    let tmp = Tmp::new();
    init(tmp.path());
    add_intent(tmp.path(), "operators can prove context");
    loom_ok(
        tmp.path(),
        &[
            "validation",
            "add",
            "--name",
            "context proof",
            "--type",
            "test",
            "--command",
            "true",
            "--intent",
            "operators can prove context",
        ],
    );
    // A runnable proof is RUN, never reported. The packet below shows a passing
    // proof because loom watched it pass.
    loom_ok(tmp.path(), &["validation", "run", "context proof"]);

    let packet = loom_json(tmp.path(), &["context", "operators can prove context"]);
    assert!(entities(&packet).iter().any(|entity| {
        entity["role"] == "validation"
            && entity["name"] == "context proof"
            && entity["status"] == "passed"
    }));
    assert!(entities(&packet)
        .iter()
        .any(|entity| { entity["role"] == "proof" && entity["edge_status"] == "passing" }));
}

#[test]
fn unresolved_context_target_has_a_helpful_error() {
    let tmp = Tmp::new();
    init(tmp.path());
    let output = Command::new(loom_bin())
        .arg("--graph")
        .arg(tmp.path())
        .args(["context", "unresolvable obscure target"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("could not resolve"),
        "error must say how resolution failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn context_calls_out_stale_edges() {
    let tmp = Tmp::new();
    tmp.write("src/context_target.rs", "pub fn packet() {}\n");
    init(tmp.path());
    add_intent(tmp.path(), "context flags stale evidence");
    loom_ok(tmp.path(), &["codefile", "add", "src/context_target.rs"]);
    let edge = loom_json(
        tmp.path(),
        &[
            "edge",
            "implement",
            "context flags stale evidence",
            "src/context_target.rs",
        ],
    );
    let edge_id = edge["edge"]["id"].as_str().unwrap().to_string();
    loom_ok(
        tmp.path(),
        &[
            "edge",
            "verdict",
            &edge_id,
            "ground",
            "--criterion",
            "the file implements the behavior",
            "--evidence",
            "src/context_target.rs:1 — packet implements it",
        ],
    );
    let store = Store::open(tmp.path()).unwrap();
    assert!(store.stale_edge(&edge_id, "context test").unwrap());
    drop(store);

    let packet = loom_json(tmp.path(), &["context", "context flags stale evidence"]);
    assert!(
        packet["staleness_flags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|flag| flag.as_str().unwrap_or("").contains("needs_reverification")),
        "stale edge must be stated plainly: {packet}"
    );
}

/// `--where ratification=<state>` must read the fact table.
///
/// v3 moved ratification out of `facet` and `set_facet` refuses the key, so the
/// old facet lookup could only ever match nothing. Every state except the
/// `unratified` special case returned an empty list — silently, on a graph full
/// of ratified intents, which is the worst way for a query to be wrong.
#[test]
fn ratification_filter_reads_the_fact_not_a_facet() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let wanted = store
        .add_node(
            NodeType::Intent,
            "a behavior somebody asked for",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let silent = store
        .add_node(
            NodeType::Intent,
            "a behavior nobody has spoken to",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .ratify_intent(&wanted.id, "the owner asked for it in review", "tty")
        .unwrap();
    assert_eq!(store.ratification(&wanted.id).unwrap(), "ratified");
    drop(store);

    let ratified = loom_json(tmp.path(), &["find", "--where", "ratification=ratified"]);
    let names: Vec<&str> = ratified
        .as_array()
        .expect("find returns an array")
        .iter()
        .filter_map(|n| n["name"].as_str())
        .collect();
    assert!(
        names.contains(&"a behavior somebody asked for"),
        "the ratified intent must be findable: {names:?}"
    );
    assert!(
        !names.contains(&"a behavior nobody has spoken to"),
        "and the unratified one must not be: {names:?}"
    );
    let _ = &silent;
}
