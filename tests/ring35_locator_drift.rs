//! Ring 35 — a locator that names nothing is a broken anchor.
//!
//! A locator is the promise "this behavior lives at this symbol". Nothing was
//! checking it. `recheck` expires a LOCATOR run when its symbol stops
//! resolving, but a verdict recorded with a cited SPAN carries no such run — so
//! a grounding could name a deleted symbol and stay `passing` indefinitely.
//!
//! That is not hypothetical: this repository carried 13 live claims at
//! confidence 0.95 pointing at symbols that no longer existed — four of them
//! left behind by a hard-cut that deleted the code — while `doctor` reported
//! clean and the ladder showed `grounded` met. A claim that points at nothing
//! is exactly what the evidence spine exists to refuse.

use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::store::Store;
mod common;
use common::Tmp;

/// Ground an intent in a file at `locator`, settle it with a cited span, and
/// return the edge id. The span is the point: it satisfies the anchor floor
/// without anyone ever probing the locator.
fn grounded(store: &Store, tmp: &Tmp, file: &str, body: &str, locator: &str) -> String {
    std::fs::write(tmp.path().join(file), body).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            &format!("a behavior grounded at {locator}"),
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = store
        .add_node(NodeType::CodeFile, file, "", "", serde_json::json!({}))
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
            locator,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &e.id,
            loom::model::InspectionStatus::Passing,
            "the behavior lives here",
            &format!("{file}:1"),
            0.95,
            "llm",
        )
        .unwrap();
    e.id
}

fn status(store: &Store, edge: &str) -> String {
    store
        .get_edge(edge)
        .unwrap()
        .unwrap()
        .status
        .as_str()
        .to_string()
}

/// **A grounding whose locator resolves to no symbol re-opens on sync.**
#[test]
fn a_locator_that_names_nothing_reopens() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let edge = grounded(&store, &tmp, "gone.rs", "pub fn present() {}\n", "vanished");
    assert_eq!(status(&store, &edge), "passing", "settled before sync");

    loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        status(&store, &edge),
        "needs_reverification",
        "a locator naming no symbol in the file is a broken anchor"
    );
}

/// **A locator that does resolve is left alone.**
///
/// The ripple must not re-open every grounding on every sync — the whole point
/// of symbol-scoped staleness is that untouched claims stay settled.
#[test]
fn a_resolving_locator_stays_settled() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let edge = grounded(&store, &tmp, "here.rs", "pub fn present() {}\n", "present");

    loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        status(&store, &edge),
        "passing",
        "the symbol is there, so the claim is untouched"
    );
}

/// **A module-scope locator is a whole-file claim, not a symbol.**
///
/// The convention in this graph is that a locator opening with `module` names
/// the file's subject rather than a callable — 39 groundings use it. Requiring
/// those to resolve would reject every one of them to catch the broken few, so
/// they are deliberately exempt.
#[test]
fn a_module_scope_locator_is_exempt() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let edge = grounded(
        &store,
        &tmp,
        "whole.rs",
        "pub fn a() {}\n",
        "module the thing this file is about",
    );

    loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        status(&store, &edge),
        "passing",
        "a whole-file scope names no symbol on purpose and must not be judged as one"
    );
}

/// **A locator naming several symbols still points at real code.**
///
/// Ambiguity is a different complaint from absence. `resolve_locator` reports
/// the cardinality separately and the ripple only refuses zero, because two
/// functions sharing a name are both really there — and the anchor honestly
/// covers both.
#[test]
fn an_ambiguous_locator_is_not_treated_as_missing() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let edge = grounded(
        &store,
        &tmp,
        "twice.rs",
        "mod a { pub fn helper() {} }\nmod b { pub fn helper() {} }\n",
        "helper",
    );

    loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        status(&store, &edge),
        "passing",
        "two matches is ambiguous, not absent"
    );
}

/// **A vanished FILE is not this ripple's complaint.**
///
/// File deletion has its own handling; judging a symbol inside a file that is
/// not there would double-report it under the wrong cause.
#[test]
fn a_missing_file_is_left_to_its_own_ripple() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let edge = grounded(
        &store,
        &tmp,
        "doomed.rs",
        "pub fn present() {}\n",
        "present",
    );
    std::fs::remove_file(tmp.path().join("doomed.rs")).unwrap();

    loom::sync::run(&store, tmp.path()).unwrap();
    let cause = store
        .get_facet(&edge, TargetKind::Edge, "stale_cause")
        .unwrap()
        .unwrap_or_default();
    assert!(
        !cause.contains("names no symbol"),
        "a deleted file must not be reported as a naming problem: {cause}"
    );
}

/// **Only a `realizes` grounding promises a symbol.**
///
/// A `consumes` locator names a SEAM — an interface string the consumer calls,
/// which is not a definition and will never resolve as one; `recheck` re-runs
/// those through their own Seam arm. Judging them here would stale every seam
/// grounding on every sync. ring11 caught exactly that when this ripple was
/// first written unscoped, which is why the scoping is asserted here too.
#[test]
fn a_consumes_seam_locator_is_not_judged_as_a_symbol() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    std::fs::write(
        tmp.path().join("page.svelte"),
        "<script>loadProfile();</script>\n",
    )
    .unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "the page loads a profile",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = store
        .add_node(
            NodeType::CodeFile,
            "page.svelte",
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
    // A seam, not a definition: nothing in the file DEFINES loadProfile.
    store
        .set_facet(
            &e.id,
            TargetKind::Edge,
            "locator",
            "loadProfile",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_grounding_role(&e.id, loom::model::GroundingRole::Consumes)
        .unwrap();
    store
        .record_verdict(
            &e.id,
            loom::model::InspectionStatus::Passing,
            "the page consumes this seam",
            "page.svelte:1",
            0.95,
            "llm",
        )
        .unwrap();

    loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        status(&store, &e.id),
        "passing",
        "a consumes seam names an interface, not a definition, and must not be judged as a missing symbol"
    );
}

/// **Write-time probe (finding c1fb2418).** `edge implement` used to accept any
/// locator string — including prose-with-line-numbers and names the file never
/// contained — while `pattern exemplar add` already refused via
/// `unique_locator_probe`. The write path now shares the sync backstop's rule:
/// a realizing locator must match at least one live symbol; `module …` stays
/// exempt as whole-file scope.
#[test]
fn edge_implement_refuses_an_unresolvable_locator() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    std::fs::write(tmp.path().join("src.rs"), "pub fn alpha() {}\n").unwrap();
    store
        .add_node(
            NodeType::Intent,
            "alpha behavior",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_node(NodeType::CodeFile, "src.rs", "", "", serde_json::json!({}))
        .unwrap();

    // Library-level: the probe helper is what the CLI and apply both call.
    assert!(
        !loom::runner::grounding_locator_resolves(
            tmp.path(),
            "src.rs",
            "alpha:12 (stale before uninspected)"
        ),
        "prose + rotted line number must not resolve"
    );
    assert!(
        !loom::runner::grounding_locator_resolves(tmp.path(), "src.rs", "fn never_existed"),
        "a name the file never contained must not resolve"
    );
    assert!(
        loom::runner::grounding_locator_resolves(tmp.path(), "src.rs", "fn alpha"),
        "a live symbol must resolve"
    );
    assert!(
        loom::runner::grounding_locator_resolves(tmp.path(), "src.rs", "module src"),
        "module-scope whole-file locators stay exempt"
    );
}

#[test]
fn edge_implement_cli_refuses_and_module_scope_is_accepted() {
    let tmp = Tmp::new();
    // Use the CLI path — that is what agents actually call.
    let bin = env!("CARGO_BIN_EXE_loom");
    let run = |args: &[&str]| {
        std::process::Command::new(bin)
            .args(["--graph"])
            .arg(tmp.path())
            .args(args)
            .output()
            .unwrap()
    };
    let init = std::process::Command::new(bin)
        .args(["init"])
        .arg(tmp.path())
        .args(["--name", "t"])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "init: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    std::fs::write(tmp.path().join("src.rs"), "pub fn alpha() {}\n").unwrap();
    let intent = run(&[
        "intent",
        "add",
        "--name",
        "alpha behavior",
        "--description",
        "alpha does something",
        "--lifecycle",
        "implemented",
    ]);
    assert!(
        intent.status.success(),
        "intent add: {}",
        String::from_utf8_lossy(&intent.stderr)
    );
    let cf = run(&["codefile", "add", "src.rs"]);
    assert!(
        cf.status.success(),
        "codefile add: {}",
        String::from_utf8_lossy(&cf.stderr)
    );

    let bad = run(&[
        "edge",
        "implement",
        "alpha behavior",
        "src.rs",
        "--locator",
        "fn never_existed",
    ]);
    assert!(
        !bad.status.success(),
        "unresolvable locator must be refused: {}",
        String::from_utf8_lossy(&bad.stderr)
    );
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("must resolve"),
        "refusal names the probe: {}",
        String::from_utf8_lossy(&bad.stderr)
    );

    let module_ok = run(&[
        "edge",
        "implement",
        "alpha behavior",
        "src.rs",
        "--locator",
        "module src",
    ]);
    assert!(
        module_ok.status.success(),
        "module-scope must stay accepted: {}",
        String::from_utf8_lossy(&module_ok.stderr)
    );
}
