//! Ring 34 — ordered work, expressed as readiness rather than as a list.
//!
//! loom's router is residue-driven: it serves the weakest standing claim, not
//! the next item on a plan. Sequenced work still has to be expressible, though,
//! and the answer is a constraint rather than a queue — a step is simply not
//! READY until the step before it is built. That keeps one router and one
//! notion of readiness, instead of a plan that can drift from the graph.
//!
//! `sequence` was declarable for a long time and inert: you could record that
//! one behavior follows another and the router would happily hand you the
//! second one first. These tests pin the gate, and pin the one place it
//! deliberately differs from `requires`.

use loom::lane::Lane;
use loom::model::{EdgeKind, NodeType, TruthClass};
use loom::store::Store;
mod common;
use common::Tmp;

fn intent(store: &Store, name: &str, lifecycle: &str) -> loom::model::Node {
    store
        .add_node(
            NodeType::Intent,
            name,
            "a behavior",
            lifecycle,
            serde_json::json!({}),
        )
        .unwrap()
}

/// The build lane's roster, as (name, reason) pairs.
fn build_roster(store: &Store) -> Vec<(String, String)> {
    loom::workitem::queue_items(store, Lane::Build)
        .unwrap()
        .into_iter()
        .map(|e| (e.target.name, e.reason))
        .collect()
}

/// **A step is not served while the step before it is unbuilt.**
#[test]
fn a_follower_is_blocked_until_its_predecessor_is_built() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let cart = intent(&store, "add to cart", "planned");
    let checkout = intent(&store, "checkout", "planned");
    // checkout follows cart: the FROM side stands on the TO side.
    store
        .add_edge(
            EdgeKind::Sequence,
            &checkout.id,
            &cart.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let roster = build_roster(&store);
    let first = &roster[0];
    assert_eq!(
        first.0, "add to cart",
        "the predecessor is what the lane serves first: {roster:#?}"
    );
    let blocked = roster
        .iter()
        .find(|(n, _)| n == "checkout")
        .expect("checkout is still on the roster, carrying its reason");
    assert!(
        blocked.1.contains("follows 'add to cart'"),
        "the reason must name the relation and the step: {blocked:?}"
    );
    assert!(
        blocked.1.starts_with("blocked:"),
        "and say plainly that it is blocked: {blocked:?}"
    );
}

/// **Building the predecessor releases the follower.**
#[test]
fn implementing_the_predecessor_makes_the_follower_ready() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let cart = intent(&store, "add to cart", "implemented");
    let checkout = intent(&store, "checkout", "planned");
    store
        .add_edge(
            EdgeKind::Sequence,
            &checkout.id,
            &cart.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let roster = build_roster(&store);
    let checkout_row = roster
        .iter()
        .find(|(n, _)| n == "checkout")
        .expect("checkout is on the roster");
    assert!(
        !checkout_row.1.contains("blocked:"),
        "with its predecessor implemented, the follower is ready: {checkout_row:?}"
    );
}

/// **`requires` and `sequence` are distinguishable in the reason.**
///
/// A builder told only "blocked" learns nothing. Naming the relation says
/// whether the thing is a dependency or merely an earlier step.
#[test]
fn the_reason_distinguishes_a_dependency_from_an_earlier_step() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let dep = intent(&store, "the dependency", "planned");
    let prev = intent(&store, "the earlier step", "planned");
    let subject = intent(&store, "the subject", "planned");
    store
        .add_edge(
            EdgeKind::Requires,
            &subject.id,
            &dep.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Sequence,
            &subject.id,
            &prev.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let roster = build_roster(&store);
    let row = roster.iter().find(|(n, _)| n == "the subject").unwrap();
    assert!(
        row.1.contains("requires 'the dependency'"),
        "the dependency is named as a requirement: {row:?}"
    );
    assert!(
        row.1.contains("follows 'the earlier step'"),
        "the predecessor is named as an ordering: {row:?}"
    );
}

/// **Ordering is not incompleteness.**
///
/// This is the one place the gate deliberately parts company with the
/// completeness scorecard. A behavior whose PREDECESSOR is unbuilt is still a
/// complete specification — it simply is not the next thing to build. A
/// behavior whose DEPENDENCY is unbuilt is not complete, and the axis says so.
/// Collapsing the two would make `sequence` manufacture incompleteness and
/// rebuild the wall the `wanted` rung once was.
#[test]
fn a_sequence_predecessor_does_not_open_the_prerequisites_axis() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let prev = intent(&store, "the earlier step", "planned");
    let follower = intent(&store, "the later step", "implemented");
    store
        .add_edge(
            EdgeKind::Sequence,
            &follower.id,
            &prev.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let card = loom::completeness::scorecard(&store, &follower).unwrap();
    let axis = card
        .axes
        .iter()
        .find(|a| a.axis == "prerequisites")
        .expect("the prerequisites axis is scored");
    assert_eq!(
        axis.state, "met",
        "an unbuilt PREDECESSOR is an ordering fact, not a missing prerequisite: {axis:#?}"
    );

    // The same shape as a `requires` edge DOES open it — the contrast is the point.
    let dependent = intent(&store, "the dependent", "implemented");
    store
        .add_edge(
            EdgeKind::Requires,
            &dependent.id,
            &prev.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let card = loom::completeness::scorecard(&store, &dependent).unwrap();
    let axis = card
        .axes
        .iter()
        .find(|a| a.axis == "prerequisites")
        .unwrap();
    assert_eq!(
        axis.state, "open",
        "an unbuilt DEPENDENCY is incompleteness: {axis:#?}"
    );
}

/// **A sequence cycle never stalls the lane.**
///
/// The build lane's existing contract is that it always serves something, even
/// when everything is blocked — a stalled lane is worse than a named blocker.
/// Ordering must not become a way to deadlock it.
#[test]
fn a_sequence_cycle_still_serves_work_with_a_named_blocker() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "step A", "planned");
    let b = intent(&store, "step B", "planned");
    store
        .add_edge(EdgeKind::Sequence, &a.id, &b.id, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Sequence, &b.id, &a.id, TruthClass::Asserted)
        .unwrap();

    let item = loom::workitem::next(&store, Some(Lane::Build))
        .unwrap()
        .expect("the lane serves work even when every candidate is blocked");
    assert!(
        item.reason.contains("blocked:") && item.reason.contains("break the cycle"),
        "and it says why, and how to get out: {}",
        item.reason
    );
}
