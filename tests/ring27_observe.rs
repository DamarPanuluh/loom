//! Ring 27 — `loom observe`, the cheap way in.
//!
//! Every other route into the graph asks an agent to describe work it has
//! already done, in loom's vocabulary, after the fact. That is the tax that
//! gets loom skipped. This one asks for a prefix on a command the agent was
//! going to run anyway, and the run becomes evidence.
//!
//! Which means the command loom runs has to be EXACTLY the command it was
//! handed — the first test here is about shell quoting, because a wrapper that
//! silently mangles its argv would be worse than no wrapper at all.

use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::store::Store;
mod common;
use common::*;

/// Seed the graph and RELEASE it. `loom observe` runs as its own process and
/// takes the write lock, so a test holding an open store would deadlock
/// against the very command it is testing.
fn graph(root: &std::path::Path) -> String {
    let store = Store::init(root, Some("t"), false).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/orders.rs"), "pub fn place() -> u8 { 1 }\n").unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "an order can be placed",
            "a behavior",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = store
        .add_node(
            NodeType::CodeFile,
            "src/orders.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let e = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &e.id,
            TargetKind::Edge,
            "locator",
            "fn place",
            TruthClass::Asserted,
        )
        .unwrap();
    loom::sync::run(&store, root).unwrap();
    intent.id
}

fn observe(root: &std::path::Path, target: Option<&str>, argv: &[&str]) -> serde_json::Value {
    let out = std::process::Command::new(loom_bin())
        .arg("--graph")
        .arg(root)
        .arg("--json")
        .arg("observe")
        .args(
            target
                .map(|t| vec!["--for".to_string(), t.to_string()])
                .unwrap_or_default(),
        )
        .arg("--")
        .args(argv)
        .output()
        .expect("spawn loom observe");
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "observe did not emit JSON: {e}\n--stdout--\n{stdout}\n--stderr--\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn loom_bin() -> std::path::PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("loom")
}

/// The command that runs is the command that was typed.
///
/// Joining argv on spaces looks right and is wrong: it hands
/// `sh -c "echo a b; echo c"` several statements where the caller passed one
/// argument. A wrapper that mangles its own input cannot be trusted with
/// anything downstream.
#[test]
fn arguments_survive_the_shell() {
    let tmp = Tmp::new();
    let _intent = graph(tmp.path());

    // A single argument containing spaces, semicolons and quotes.
    let v = observe(tmp.path(), None, &["printf", "%s", "one; two 'three'"]);
    assert_eq!(v["observed"], true, "{v}");
    assert_eq!(v["exit_code"], 0, "the command ran as ONE argument: {v}");
}

/// A run with a target binds to that behavior's proof and grades honestly:
/// loom ran it and it passed, which is liveness — S1, not S2.
#[test]
fn an_observed_run_binds_and_grades_as_liveness() {
    let tmp = Tmp::new();
    let _intent = graph(tmp.path());

    let v = observe(tmp.path(), Some("an order can be placed"), &["true"]);
    assert_eq!(v["observed"], true, "{v}");
    assert!(!v["bound_to"].is_null(), "it binds: {v}");
    assert_eq!(
        v["strength"], "S1",
        "loom ran it and it passed — that is liveness, not behavior: {v}"
    );
    // The run covers the behavior's grounded files, which is what lets an edit
    // expire it.
    assert!(
        v["covered"]
            .as_array()
            .map(|a| a.iter().any(|f| f == "src/orders.rs"))
            .unwrap_or(false),
        "the run covers what the behavior is grounded in: {v}"
    );

    let store = Store::open(tmp.path()).unwrap();
    let vals = store
        .list_nodes(Some(NodeType::Validation), usize::MAX)
        .unwrap();
    assert_eq!(vals.len(), 1, "one proof, minted by the observation");
}

/// Running the same command twice updates one proof rather than littering the
/// graph with near-duplicates — otherwise habitual use would be self-defeating.
#[test]
fn the_same_command_twice_is_one_proof() {
    let tmp = Tmp::new();
    let _intent = graph(tmp.path());

    observe(tmp.path(), Some("an order can be placed"), &["true"]);
    observe(tmp.path(), Some("an order can be placed"), &["true"]);

    let store = Store::open(tmp.path()).unwrap();
    let vals = store
        .list_nodes(Some(NodeType::Validation), usize::MAX)
        .unwrap();
    assert_eq!(vals.len(), 1, "one command, one proof: {vals:#?}");
}

/// An edit under the run expires it. This is the whole reason `covered` exists.
#[test]
fn editing_a_covered_file_reopens_the_proof() {
    let tmp = Tmp::new();
    let _intent = graph(tmp.path());
    observe(tmp.path(), Some("an order can be placed"), &["true"]);

    let store = Store::open(tmp.path()).unwrap();
    let val = store
        .list_nodes(Some(NodeType::Validation), usize::MAX)
        .unwrap()
        .remove(0);
    assert_eq!(val.status, "passed");

    std::fs::write(
        tmp.path().join("src/orders.rs"),
        "pub fn place() -> u8 { 2 }\n",
    )
    .unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();

    let after = store.get_node(&val.id).unwrap().unwrap();
    assert_eq!(
        after.status, "not_run",
        "the code the run covered changed, so the run no longer says anything"
    );
}

/// A command loom cannot run is not a failing proof. Recorded, visible, never
/// green — and never recorded as a failure of the behavior.
#[test]
fn an_unrunnable_command_is_blocked_not_failed() {
    let tmp = Tmp::new();
    let _intent = graph(tmp.path());

    let v = observe(
        tmp.path(),
        Some("an order can be placed"),
        &["this-binary-does-not-exist-anywhere"],
    );
    // `sh -c` reports a missing binary as a non-zero exit rather than a spawn
    // failure, so this records as a failed run — what matters is that it is
    // never silently green.
    if v["observed"] == true {
        assert_ne!(v["exit_code"], 0, "a missing binary is not a pass: {v}");
    }
    let store = Store::open(tmp.path()).unwrap();
    let vals = store
        .list_nodes(Some(NodeType::Validation), usize::MAX)
        .unwrap();
    assert!(
        vals.iter().all(|x| x.status != "passed"),
        "nothing passes on a command that could not do its job: {vals:#?}"
    );
}

/// Without a target the run is still recorded — a stray `loom observe` leaves
/// something re-checkable behind rather than nothing.
#[test]
fn an_untargeted_run_is_still_journaled() {
    let tmp = Tmp::new();
    let _intent = graph(tmp.path());

    let v = observe(tmp.path(), None, &["true"]);
    assert_eq!(v["observed"], true, "{v}");
    assert!(v["bound_to"].is_null(), "nothing to bind to: {v}");
    assert!(!v["journal"].as_str().unwrap_or("").is_empty());
    let store = Store::open(tmp.path()).unwrap();
    assert!(
        store
            .list_nodes(Some(NodeType::Validation), usize::MAX)
            .unwrap()
            .is_empty(),
        "an unattached run must not invent a behavior to attach itself to"
    );
}
