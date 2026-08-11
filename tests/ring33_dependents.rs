//! Ring 33 — what stands on a behavior.
//!
//! `loom impact` answers "what could a change here reach?" for CODE, by walking
//! the call graph. Nothing answered it for BEHAVIOR: the intent graph carries
//! `requires`, `scenario_of`, `hierarchy` and `sequence`, and every one of them
//! was write-only as far as reachability went. You could declare that one
//! behavior stands on another and never ask what that implied.
//!
//! The traversal is one recursive SQL query rather than a Rust pointer-chase,
//! because the edges are already indexed rows. These tests pin the properties
//! that make the answer trustworthy: transitivity, shortest-path hop counts, a
//! depth bound that also makes cycles terminate, and the fact that a dependent
//! with no passing proof is reported as such.

use loom::model::{EdgeKind, NodeType, TruthClass};
use loom::store::{Agent, Store};
mod common;
use common::Tmp;

fn intent(store: &Store, name: &str) -> loom::model::Node {
    store
        .add_node(
            NodeType::Intent,
            name,
            "a behavior",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap()
}

fn link(store: &Store, kind: EdgeKind, from: &str, to: &str) {
    store
        .add_edge(kind, from, to, TruthClass::Asserted)
        .unwrap();
}

fn names(found: &[loom::store::Dependent]) -> Vec<&str> {
    found.iter().map(|d| d.intent.name.as_str()).collect()
}

/// **Standing on is transitive, and hop counts are shortest-path.**
///
/// If A requires B and B requires C, then changing C reaches A — two hops away.
/// A reader deciding how far a change travels needs the nearest route, not
/// whichever one the walk happened to find first.
#[test]
fn dependents_are_transitive_and_report_the_shortest_route() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "A the far dependent");
    let b = intent(&store, "B the middle");
    let c = intent(&store, "C the foundation");
    link(&store, EdgeKind::Requires, &a.id, &b.id);
    link(&store, EdgeKind::Requires, &b.id, &c.id);

    let found = store.dependents(&c.id, 5).unwrap();
    assert_eq!(
        names(&found),
        vec!["B the middle", "A the far dependent"],
        "nearest first: B at 1 hop, A at 2"
    );
    assert_eq!(found[0].hops, 1);
    assert_eq!(found[1].hops, 2);

    // Give A a direct route as well; it must now report the SHORTER one.
    link(&store, EdgeKind::Requires, &a.id, &c.id);
    let found = store.dependents(&c.id, 5).unwrap();
    assert_eq!(
        found.iter().filter(|d| d.intent.id == a.id).count(),
        1,
        "multiple reverse routes must collapse to one dependent row"
    );
    let a_row = found
        .iter()
        .find(|d| d.intent.name == "A the far dependent")
        .expect("A still stands on C");
    assert_eq!(
        a_row.hops, 1,
        "with two routes the shortest wins, so distance stays meaningful"
    );
}

/// **All four standing-on edge kinds walk the same direction.**
///
/// `requires`, `scenario_of`, `hierarchy` and `sequence` differ in meaning, but
/// in every one of them the FROM side is the thing that stands on the TO side.
/// That is what lets one traversal serve all four — and it is worth asserting,
/// because getting a direction backwards would silently invert the answer.
#[test]
fn every_standing_on_edge_kind_traverses_uniformly() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let base = intent(&store, "the base behavior");
    for (kind, who) in [
        (EdgeKind::Requires, "the requirer"),
        (EdgeKind::ScenarioOf, "the scenario"),
        (EdgeKind::Hierarchy, "the parent"),
        (EdgeKind::Sequence, "the next step"),
    ] {
        let n = intent(&store, who);
        link(&store, kind, &n.id, &base.id);
    }

    let found = store.dependents(&base.id, 5).unwrap();
    let mut got = names(&found);
    got.sort_unstable();
    assert_eq!(
        got,
        vec![
            "the next step",
            "the parent",
            "the requirer",
            "the scenario"
        ],
        "all four kinds reach the behavior they point at: {found:#?}"
    );
}

/// **A cycle terminates instead of hanging.**
///
/// `requires` cycles are possible — the build lane already has a fallback that
/// names one rather than stalling. The traversal must survive one too, and the
/// depth bound is what guarantees that without a visited-set.
#[test]
fn a_requires_cycle_terminates_under_the_depth_bound() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store.set_agent(Agent::Solo);
    let a = intent(&store, "A");
    let b = intent(&store, "B");
    link(&store, EdgeKind::Requires, &a.id, &b.id);
    link(&store, EdgeKind::Requires, &b.id, &a.id);

    let found = store.dependents(&a.id, 4).unwrap();
    assert_eq!(
        names(&found),
        vec!["B"],
        "the cycle yields B once, not forever"
    );
    assert_eq!(found[0].hops, 1);
}

/// **Depth bounds how far the answer travels.**
#[test]
fn depth_bounds_the_walk() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let far = intent(&store, "three away");
    let mid = intent(&store, "two away");
    let near = intent(&store, "one away");
    let base = intent(&store, "base");
    link(&store, EdgeKind::Requires, &near.id, &base.id);
    link(&store, EdgeKind::Requires, &mid.id, &near.id);
    link(&store, EdgeKind::Requires, &far.id, &mid.id);

    assert_eq!(
        names(&store.dependents(&base.id, 1).unwrap()),
        vec!["one away"]
    );
    assert_eq!(
        names(&store.dependents(&base.id, 2).unwrap()),
        vec!["one away", "two away"]
    );
    assert_eq!(store.dependents(&base.id, 5).unwrap().len(), 3);
}

/// **An unproven dependent is called out.**
///
/// This is the reason to ask the question at all: a behavior standing on the
/// one you are changing, with nothing that would catch it breaking.
#[test]
fn a_dependent_with_no_passing_proof_is_reported_unproven() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let base = intent(&store, "base");
    let bare = intent(&store, "stands on it with nothing proving it");
    let covered = intent(&store, "stands on it and is proven");
    link(&store, EdgeKind::Requires, &bare.id, &base.id);
    link(&store, EdgeKind::Requires, &covered.id, &base.id);

    let proof = store
        .add_node(
            NodeType::Validation,
            "a proof that ran",
            "",
            "passed",
            serde_json::json!({"type":"test","command":"true"}),
        )
        .unwrap();
    link(&store, EdgeKind::Validates, &proof.id, &covered.id);

    let found = store.dependents(&base.id, 5).unwrap();
    let bare_row = found.iter().find(|d| d.intent.id == bare.id).unwrap();
    let covered_row = found.iter().find(|d| d.intent.id == covered.id).unwrap();
    assert!(!bare_row.proven, "no validates edge at all means unproven");
    assert!(
        covered_row.proven,
        "a validation whose status is 'passed' counts: {covered_row:#?}"
    );
}

/// **A behavior nothing points at reaches nobody**, and non-intent nodes never
/// leak into the answer even though the edge table holds plenty of them.
#[test]
fn an_unreferenced_behavior_has_no_dependents() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let lonely = intent(&store, "nothing stands on this");
    std::fs::write(tmp.path().join("f.rs"), "pub fn a() {}\n").unwrap();
    let cf = store
        .add_node(NodeType::CodeFile, "f.rs", "", "", serde_json::json!({}))
        .unwrap();
    link(&store, EdgeKind::Implements, &lonely.id, &cf.id);

    assert!(
        store.dependents(&lonely.id, 5).unwrap().is_empty(),
        "an implements grounding is not a behavior standing on another behavior"
    );
}
