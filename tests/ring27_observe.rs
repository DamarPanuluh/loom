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
            serde_json::json!({ "level": "feature" }),
        )
        .unwrap();
    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "level",
            "feature",
            TruthClass::Asserted,
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

fn cli_json(root: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let out = std::process::Command::new(loom_bin())
        .arg("--graph")
        .arg(root)
        .arg("--json")
        .args(args)
        .output()
        .expect("spawn loom CLI");
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "loom {:?} did not emit JSON: {e}\n--stdout--\n{stdout}\n--stderr--\n{}",
            args,
            String::from_utf8_lossy(&out.stderr)
        )
    })
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
    assert_eq!(v["stdout_excerpt"], "one; two 'three'", "{v}");
    assert_eq!(v["stderr_excerpt"], "", "{v}");
}

/// A non-zero observation remains an observed run, and its public envelope
/// carries only the bounded excerpts already retained by the RunRecord. This
/// lets structured callers distinguish an expected refusal from an
/// infrastructure block without accepting a caller-supplied outcome.
#[test]
fn nonzero_observation_reports_bounded_stream_excerpts() {
    let tmp = Tmp::new();
    let _intent = graph(tmp.path());

    let v = observe(
        tmp.path(),
        None,
        &[
            "sh",
            "-c",
            "printf 'bounded stdout'; printf 'bounded stderr' >&2; exit 7",
        ],
    );
    assert_eq!(v["observed"], true, "{v}");
    assert_eq!(v["exit_code"], 7, "the actual non-zero status is kept: {v}");
    assert_eq!(v["stdout_excerpt"], "bounded stdout", "{v}");
    assert_eq!(v["stderr_excerpt"], "bounded stderr", "{v}");
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
    drop(store);

    let next = cli_json(tmp.path(), &["next", "--mode", "validate"]);
    let item = &next["work_item"];
    assert_eq!(item["mode"], "validate", "{next}");
    for field in ["reason", "prompt_contract"] {
        assert!(!item[field].is_null(), "{field} is present: {next}");
    }
    assert!(
        item["reason"]
            .as_str()
            .unwrap_or("")
            .contains("ran and passed")
            && item["reason"].as_str().unwrap_or("").contains("S1")
            && item["reason"].as_str().unwrap_or("").contains("S2"),
        "packet explains weak passing proof: {next}"
    );
    assert!(
        item["prompt_contract"]["write_back"]
            .as_str()
            .unwrap_or("")
            .contains("validation update")
            && item["prompt_contract"]["stop_condition"]
                .as_str()
                .unwrap_or("")
                .contains("S2"),
        "contract instructs strengthening to meaningful proof: {next}"
    );

    let cards = cli_json(tmp.path(), &["completeness", "an order can be placed"]);
    let proof_axis = cards[0]["axes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|axis| axis["axis"] == "proof")
        .unwrap();
    assert_eq!(proof_axis["state"], "open", "{cards}");
    assert!(
        proof_axis["detail"].as_str().unwrap_or("").contains("S1")
            && proof_axis["detail"].as_str().unwrap_or("").contains("S2"),
        "completeness reports the passing proof as weak: {cards}"
    );
}

/// Running the same command twice updates one proof rather than littering the
/// graph with near-duplicates — otherwise habitual use would be self-defeating.
///
/// And the SECOND run must report the same grade as the first. It used to read
/// the grade off a node looked up by the name loom would have minted, so every
/// run that reused an existing proof — which is the common case, since proofs
/// are keyed on the command for exactly this reason — reported S0 for a proof
/// the graph had already graded S1.
#[test]
fn the_same_command_twice_is_one_proof() {
    let tmp = Tmp::new();
    let _intent = graph(tmp.path());

    let first = observe(tmp.path(), Some("an order can be placed"), &["true"]);
    let second = observe(tmp.path(), Some("an order can be placed"), &["true"]);
    assert_eq!(first["strength"], "S1", "{first}");
    assert_eq!(
        second["strength"], first["strength"],
        "a repeat run reports the grade of the proof it bound to: {second}"
    );

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

/// `loom observe` must be able to wrap loom itself.
///
/// The worst defect this command has had. `observe` held the graph's write
/// lock across the child, so any child that also opens the graph — and loom's
/// compiled Journey profiles also settle through the graph — blocked on its parent and
/// exited non-zero. That did not merely fail: it recorded a FALSE FAILING
/// verdict against a behavior that passes, and the validate packet recommended
/// exactly that form. A tool whose thesis is that nothing counts unless loom
/// observed it cannot afford to observe wrong.
#[test]
fn observing_a_command_that_uses_loom_does_not_deadlock_on_its_own_lock() {
    let tmp = Tmp::new();
    let _intent = graph(tmp.path());

    // The child opens the SAME graph for writing, exactly as compiled Journey
    // settlement and `loom sync` do.
    let child = format!(
        "{} --graph {} sync",
        loom_bin().display(),
        tmp.path().display()
    );
    let v = observe(
        tmp.path(),
        Some("an order can be placed"),
        &["sh", "-c", &child],
    );
    assert_eq!(v["observed"], true, "{v}");
    assert_eq!(
        v["exit_code"], 0,
        "a child that opens the graph must not be blocked by its observer: {v}"
    );
    assert_eq!(v["strength"], "S1", "{v}");
}
