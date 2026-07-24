//! Ring 4 tests — maturity ladder + compass routing.

use loom::lane::Lane;
use loom::maturity::{ladder, RungState};
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use loom::travel;
use loom::workitem;
mod common;
use common::*;

/// Test fixture: seeded intents are wanted by construction — ratify them all
/// so ladder/compass tests exercise the gate under test, not the ratify gate.
fn ratify_all(store: &Store) {
    for n in workitem::unratified_intents(store).unwrap() {
        store
            .ratify_intent(
                &n.id,
                "test fixture: seeded intent is wanted",
                "test fixture",
            )
            .unwrap();
    }
}

#[test]
fn empty_graph_compass_routes_to_seed() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let l = ladder(&store).unwrap();
    assert_eq!(l.phase, "seed");
    assert_eq!(l.rungs[0].state, RungState::Unmet); // seeded unmet on empty graph
}

#[test]
fn derived_floor_balance_is_a_measured_fact() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // Empty graph: no facts, so the floor is a defined 0.0 (never NaN).
    let empty = ladder(&store).unwrap().derived_floor;
    assert_eq!(empty.derived, 0);
    assert_eq!(empty.asserted, 0);
    assert_eq!(empty.ratio, 0.0);

    // An asserted intent is asserted weight; still no derived facts.
    store
        .add_node(
            NodeType::Intent,
            "payment can be captured",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let floor = ladder(&store).unwrap().derived_floor;
    assert!(floor.asserted >= 1, "the intent counts as an asserted fact");
    assert_eq!(floor.derived, 0);
    assert_eq!(floor.ratio, 0.0);

    // A derived finding node lifts the programmatic floor above zero.
    store
        .add_derived_node(
            NodeType::Finding,
            "df:1",
            "df:1",
            "oversized",
            "oversized_file",
            serde_json::json!({ "kind": "oversized_file", "symbol": "" }),
        )
        .unwrap();
    let lifted = ladder(&store).unwrap().derived_floor;
    assert!(lifted.derived >= 1);
    assert!(lifted.ratio > 0.0 && lifted.ratio < 1.0);
    assert_eq!(
        lifted.derived + lifted.asserted,
        // ratio is the derived share of all counted facts
        {
            let total = lifted.derived + lifted.asserted;
            let by_ratio = (lifted.ratio * total as f64).round() as usize;
            assert_eq!(by_ratio, lifted.derived);
            total
        }
    );
}

#[test]
fn planned_intent_routes_to_build() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .add_node(
            NodeType::Intent,
            "payment can be captured",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    ratify_all(&store);
    let l = ladder(&store).unwrap();
    assert_eq!(l.phase, "build");
    // seeded met, wanted met (fixture-ratified), realized unmet
    assert_eq!(l.rungs[0].state, RungState::Met);
    assert_eq!(l.rungs[1].state, RungState::Met);
    assert_eq!(l.rungs[2].state, RungState::Unmet);
}

#[test]
fn implemented_but_ungrounded_intent_routes_to_a_nonempty_build_queue() {
    // Regression: an `implemented` intent with no realizing grounding leaves the
    // `realized` rung Unmet, so the compass routes to `build`. The build queue
    // MUST serve it — otherwise the compass points `loom next --mode build` at an
    // empty queue (a dead end). The compass's stated invariant is that routing
    // follows the exact queue partition; this defends it for the ungrounded case.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .add_node(
            NodeType::Intent,
            "the widget renders on load",
            "a user sees the widget appear on first paint",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();

    // Compass routes to build (realized rung Unmet: 1 ungrounded).
    ratify_all(&store);
    let l = ladder(&store).unwrap();
    assert_eq!(
        l.phase, "build",
        "ungrounded implemented intent routes to build"
    );
    assert_eq!(l.next_command, "loom next --mode build");

    // The build queue is non-empty and serves exactly that intent — the compass
    // and the queue partition agree.
    let counts = loom::maturity::depths(&store).unwrap();
    assert_eq!(
        counts.get(Lane::Build),
        1,
        "the ungrounded intent is counted in the build queue"
    );
    let item = workitem::next(&store, Some(Lane::Build))
        .unwrap()
        .expect("build queue must serve the ungrounded implemented intent, not return None");
    assert_eq!(item.mode, "build");
    assert_eq!(item.target.name, "the widget renders on load");
    assert!(
        item.reason.contains("ungrounded"),
        "the reason steers to grounding, got: {}",
        item.reason
    );

    // Default `loom next` (no mode) reaches the same work — compass and default
    // routing never disagree.
    let default_item = workitem::next(&store, None)
        .unwrap()
        .expect("default next serves it");
    assert_eq!(default_item.mode, "build");
}

#[test]
fn graph_mode_is_settable_after_init_and_takes_effect() {
    // Regression (Bug 3): `observed` is a graph MODE, reachable after init via
    // `loom mode` (Store::set_observed), not an orphaned flag. Setting it to
    // observed disables the build lane; setting it back to owned re-enables it.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap(); // owned
    assert!(!store.identity().unwrap().observed, "starts owned");

    // An ungrounded implemented intent: build work while owned.
    store
        .add_node(
            NodeType::Intent,
            "the thing works",
            "a user does the thing and it works",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    assert_eq!(
        loom::maturity::depths(&store).unwrap().get(Lane::Build),
        1,
        "owned graph serves the ungrounded intent as build work"
    );

    // Flip to observed: identity reflects it and the build lane goes dark.
    assert!(store.set_observed(true).unwrap());
    assert!(
        store.identity().unwrap().observed,
        "mode set to observed persists"
    );
    assert_eq!(
        loom::maturity::depths(&store).unwrap().get(Lane::Build),
        0,
        "observed graph disables the build lane"
    );

    // Flip back to owned: the lane returns.
    assert!(!store.set_observed(false).unwrap());
    assert!(
        !store.identity().unwrap().observed,
        "mode set back to owned"
    );
    assert_eq!(loom::maturity::depths(&store).unwrap().get(Lane::Build), 1);
}

#[test]
fn analyze_packet_carries_the_lane_that_owns_the_write() {
    // Regression (2026-07-19 drain): `--mode analyze` served an implements-edge
    // re-verdict with owner_role=analyzer, but the registry owns implements
    // writes as builder — INV-7 rejected the whole batch. The packet (and the
    // --all roster row) must name the registry owner, and that lane's write
    // must actually succeed.
    use loom::registry::OwnerRole;
    use loom::store::Agent;
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "the widget renders",
            "a user sees the widget",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = store
        .add_node(
            NodeType::CodeFile,
            "src/widget.rs",
            "",
            "active",
            serde_json::json!({}),
        )
        .unwrap();
    let edge = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    ratify_all(&store);

    let item = workitem::next(&store, Some(Lane::Analyze))
        .unwrap()
        .expect("uninspected implements claim is analyze work");
    assert_eq!(item.target.id, edge.id);
    assert_eq!(
        item.owner_role, "builder",
        "packet must name the registry owner of the write, not the mode's default lane"
    );
    let roster = workitem::queue_items(&store, Lane::Analyze).unwrap();
    assert_eq!(roster[0].owner_role.as_deref(), Some("builder"));

    // The promise must be real: the named lane's write is accepted.
    store.set_agent(Agent::Lane(OwnerRole::Builder));
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "grounded",
            "src/widget.rs",
            0.9,
            "llm",
        )
        .unwrap();
    store.set_agent(Agent::Solo);
}

#[test]
fn observed_graph_compass_never_routes_to_a_disabled_lane() {
    // Regression: on an observed graph the build/coverage/fix lanes are disabled
    // (`Lane::depth` forces them to 0). An ungrounded implemented intent leaves
    // the `realized` rung Unmet, but the compass must NOT route to `build` — that
    // lane returns nothing here, a pure dead end. It routes to `validate` instead
    // (which IS enabled on observed graphs), matching what the queues can serve.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), true).unwrap(); // observed = true
    store
        .add_node(
            NodeType::Intent,
            "the upstream emits an event we map",
            "an external service fires an event this graph observes",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();

    let counts = loom::maturity::depths(&store).unwrap();
    assert_eq!(
        counts.get(Lane::Build),
        0,
        "build lane is disabled on an observed graph"
    );
    assert_eq!(
        counts.get(Lane::Coverage),
        0,
        "coverage lane is disabled on an observed graph"
    );

    ratify_all(&store);
    let l = ladder(&store).unwrap();
    assert_ne!(
        l.phase, "build",
        "compass must not route to the disabled build lane"
    );
    assert_ne!(
        l.phase, "coverage",
        "compass must not route to the disabled coverage lane"
    );
    assert_ne!(
        l.phase, "fix",
        "compass must not route to the disabled fix lane"
    );
    // The compass never points at a `loom next --mode <m>` that is force-disabled:
    // whatever phase it picks, that mode's queue count is > 0 or it is a
    // non-lane phase (validate's proven-rung signal, audit, export, complete).
    assert_eq!(
        l.phase, "validate",
        "an unproven observed intent routes to validate (enabled on observed graphs)"
    );
}

#[test]
fn rungs_above_the_lowest_unmet_rung_are_marked_blocked() {
    // A single planned intent: `seeded` Met, `realized` Unmet (the gate). Higher
    // rungs may be independently Met (e.g. `excellent` — no findings/smells yet),
    // but the display must not present them as satisfied above an unmet lower
    // rung: they are blocked by the gate. `state` itself is untouched.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .add_node(
            NodeType::Intent,
            "payment can be captured",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    ratify_all(&store);
    let l = ladder(&store).unwrap();

    // The gate is the lowest Unmet rung: `grounded` at index 2 (after seeded
    // and repaired).
    let gate = l.rungs.iter().position(|r| r.state == RungState::Unmet);
    assert_eq!(gate, Some(2), "grounded is the lowest unmet rung");

    // Gate and everything below it are never blocked.
    assert!(!l.rungs[0].blocked, "seeded (below gate) is not blocked");
    assert!(!l.rungs[1].blocked, "repaired (below gate) is not blocked");
    assert!(!l.rungs[2].blocked, "the gate rung itself is not blocked");
    assert_eq!(l.rungs[2].blocked_by, None);

    // Every rung above the gate is blocked by it — including a rung that is
    // independently Met but must not read as satisfied above an unmet lower rung.
    for r in &l.rungs[3..] {
        assert!(
            r.blocked,
            "{} sits above the gate and must be blocked",
            r.name
        );
        assert_eq!(r.blocked_by.as_deref(), Some("grounded"));
    }
    let excellent = l.rungs.iter().find(|r| r.name == "triaged").unwrap();
    assert_eq!(
        excellent.state,
        RungState::Met,
        "the triaged rung's own truth is unchanged"
    );
    assert!(
        excellent.blocked,
        "but it is displayed as blocked by realized"
    );
}

#[test]
fn stale_edge_routes_to_fix() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = store
        .add_node(
            NodeType::Intent,
            "intent a",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let b = store
        .add_node(
            NodeType::Intent,
            "intent b",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = store
        .add_node(
            NodeType::CodeFile,
            "src/a.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    // ground both so realized is satisfiable, then create a failing edge
    store
        .add_edge(EdgeKind::Implements, &a.id, &cf.id, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Implements, &b.id, &cf.id, TruthClass::Asserted)
        .unwrap();
    let e = store
        .add_edge(EdgeKind::Relates, &a.id, &b.id, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(&e.id, InspectionStatus::Failing, "c", "broken", 0.9, "llm")
        .unwrap();
    let l = ladder(&store).unwrap();
    assert_eq!(l.phase, "fix");
    // repaired unmet because of the failing edge
    let hardened = l.rungs.iter().find(|r| r.name == "repaired").unwrap();
    assert_eq!(hardened.state, RungState::Unmet);
}

#[test]
fn fully_grounded_no_residue_routes_complete() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = store
        .add_node(
            NodeType::Intent,
            "intent a",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = store
        .add_node(
            NodeType::CodeFile,
            "src/a.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let e = store
        .add_edge(EdgeKind::Implements, &a.id, &cf.id, TruthClass::Asserted)
        .unwrap();
    // inspect the implements edge so there is no uninspected residue
    store
        .record_verdict(
            &e.id,
            InspectionStatus::Passing,
            "c",
            "src/a.rs:1",
            0.95,
            "llm",
        )
        .unwrap();
    // A real proof: loom runs it and records what it observed. A
    // hand-written passing verdict is refused now — it is the same
    // shortcut that made this graph report proofs nobody ran.
    loom::commands::prove_intent(&store, &a.id, "proof", "true").unwrap();
    ratify_all(&store);
    let before_export = ladder(&store).unwrap();
    assert_eq!(before_export.phase, "export");
    let exported = before_export
        .rungs
        .iter()
        .find(|r| r.name == "published")
        .unwrap();
    assert_eq!(exported.state, RungState::Unmet);
    travel::export_to_file(&store).unwrap();
    let after_export = ladder(&store).unwrap();
    assert_eq!(after_export.phase, "complete");
    assert_eq!(
        after_export
            .rungs
            .iter()
            .find(|r| r.name == "published")
            .unwrap()
            .state,
        RungState::Met
    );
}

#[test]
fn registered_unowned_codefile_routes_to_coverage() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "intent a",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let owned = store
        .add_node(
            NodeType::CodeFile,
            "src/owned.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_node(
            NodeType::CodeFile,
            "src/unowned.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let impl_edge = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &owned.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &impl_edge.id,
            InspectionStatus::Passing,
            "c",
            "src/owned.rs:1",
            0.95,
            "llm",
        )
        .unwrap();
    loom::commands::prove_intent(&store, &intent.id, "proof", "true").unwrap();
    ratify_all(&store);
    let l = ladder(&store).unwrap();
    assert_eq!(l.phase, "coverage");
    assert_eq!(l.next_command, "loom coverage");
    let realized = l.rungs.iter().find(|r| r.name == "covered").unwrap();
    assert_eq!(realized.state, RungState::Unmet);
    assert!(realized.detail.contains("1 unowned codefile(s)"));
    assert_eq!(l.truth_axis, Some(loom::truth::TruthAxis::Implementation));
}

#[test]
fn ignored_unowned_codefile_excluded_from_coverage_gate_and_queue() {
    // Same graph shape as `registered_unowned_codefile_routes_to_coverage`: an
    // implemented intent, one owned+grounded codefile, one UNOWNED codefile, and
    // a passing proof — so the only remaining spine gap is the unowned file.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "intent a",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let owned = store
        .add_node(
            NodeType::CodeFile,
            "src/owned.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_node(
            NodeType::CodeFile,
            "src/vendored.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let impl_edge = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &owned.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &impl_edge.id,
            InspectionStatus::Passing,
            "c",
            "src/owned.rs:1",
            0.95,
            "llm",
        )
        .unwrap();
    loom::commands::prove_intent(&store, &intent.id, "proof", "true").unwrap();
    // Pre-ignore: the unowned file is a real coverage gap — it heads the queue
    // and blocks the realized rung. (Sanity, so the test cannot pass on a
    // silently-empty graph.)
    let before = workitem::next(&store, Some(Lane::Coverage)).unwrap();
    let before = before.expect("unowned codefile must surface a coverage work item");
    assert_eq!(before.mode, "coverage");
    assert_eq!(before.target.name, "src/vendored.rs");
    let realized_before = ladder(&store)
        .unwrap()
        .rungs
        .into_iter()
        .find(|r| r.name == "covered")
        .unwrap();
    assert_eq!(realized_before.state, RungState::Unmet);
    assert!(realized_before.detail.contains("1 unowned codefile(s)"));

    // Record a coverage exclusion for the vendored file (the shape written by
    // `loom ignore add`).
    store
        .set_meta(
            "ignores",
            &serde_json::to_string(&serde_json::json!([{
                "glob": "src/vendored.rs",
                "reason": "vendored upstream — outside the tracked surface",
            }]))
            .unwrap(),
        )
        .unwrap();

    // Post-ignore: the deliberately-excluded file is no longer a gap — the
    // queue drains and the realized rung is unblocked by ownership.
    assert!(
        workitem::next(&store, Some(Lane::Coverage))
            .unwrap()
            .is_none(),
        "an ignored file must not surface as coverage work"
    );
    let realized_after = ladder(&store)
        .unwrap()
        .rungs
        .into_iter()
        .find(|r| r.name == "covered")
        .unwrap();
    assert!(
        realized_after.detail.contains("0 unowned codefile(s)"),
        "ignored file must drop from the unowned count: {}",
        realized_after.detail
    );
    assert_eq!(
        realized_after.state,
        RungState::Met,
        "with the only gap ignored, the realized rung is met"
    );
}

#[test]
fn doctor_issue_routes_to_audit_after_earlier_gates_pass() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let parent = store
        .add_node(
            NodeType::Intent,
            "parent",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let child = store
        .add_node(
            NodeType::Intent,
            "child",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let parent_file = store
        .add_node(
            NodeType::CodeFile,
            "src/parent.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let child_file = store
        .add_node(
            NodeType::CodeFile,
            "src/child.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    for (intent, file, locator) in [
        (&parent, &parent_file, "src/parent.rs:1"),
        (&child, &child_file, "src/child.rs:1"),
    ] {
        let edge = store
            .add_edge(
                EdgeKind::Implements,
                &intent.id,
                &file.id,
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .record_verdict(
                &edge.id,
                InspectionStatus::Passing,
                "c",
                locator,
                0.95,
                "llm",
            )
            .unwrap();
    }
    let down = store
        .add_edge(
            EdgeKind::Hierarchy,
            &parent.id,
            &child.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let up = store
        .add_edge(
            EdgeKind::Hierarchy,
            &child.id,
            &parent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    for edge in [down, up] {
        store
            .record_verdict(
                &edge.id,
                InspectionStatus::Passing,
                "hierarchy",
                "cycle fixture",
                0.95,
                "llm",
            )
            .unwrap();
    }
    for (intent, name) in [(&parent, "proof parent"), (&child, "proof child")] {
        loom::commands::prove_intent(&store, &intent.id, name, "true").unwrap();
    }

    ratify_all(&store);
    let l = ladder(&store).unwrap();
    assert_eq!(l.phase, "audit");
    assert_eq!(l.next_command, "loom doctor");
    let hardened = l.rungs.iter().find(|r| r.name == "sound").unwrap();
    assert_eq!(hardened.state, RungState::Unmet);
    assert!(hardened.detail.contains("1 doctor issue(s)"));
    // The audit lane adjudicates flagged observations — one axis per lane.
    assert_eq!(l.truth_axis, Some(loom::truth::TruthAxis::Signal));
}

#[test]
fn proven_rung_requires_each_implemented_leaf_to_have_passing_validation() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = store
        .add_node(
            NodeType::Intent,
            "intent a",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let b = store
        .add_node(
            NodeType::Intent,
            "intent b",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let file_a = store
        .add_node(
            NodeType::CodeFile,
            "src/a.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let file_b = store
        .add_node(
            NodeType::CodeFile,
            "src/b.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    for (intent, file, locator) in [(&a, &file_a, "src/a.rs:1"), (&b, &file_b, "src/b.rs:1")] {
        let edge = store
            .add_edge(
                EdgeKind::Implements,
                &intent.id,
                &file.id,
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .record_verdict(
                &edge.id,
                InspectionStatus::Passing,
                "c",
                locator,
                0.95,
                "llm",
            )
            .unwrap();
    }
    // A real proof: loom runs it and records what it observed. A
    // hand-written passing verdict is refused now — it is the same
    // shortcut that made this graph report proofs nobody ran.
    loom::commands::prove_intent(&store, &a.id, "proof a", "true").unwrap();
    ratify_all(&store);
    let l = ladder(&store).unwrap();
    assert_eq!(l.phase, "validate");
    assert_eq!(l.next_command, "loom next --mode validate");
    let proven = l.rungs.iter().find(|r| r.name == "proven").unwrap();
    assert_eq!(proven.state, RungState::Unmet);
    assert!(proven.detail.contains("1 unproven implemented intent(s)"));
}

#[test]
fn proven_rung_requires_journey_proof_for_user_visible_intents() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "checkout completes",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "visibility",
            "user_visible",
            TruthClass::Asserted,
        )
        .unwrap();
    let file = store
        .add_node(
            NodeType::CodeFile,
            "src/checkout.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let impl_edge = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &impl_edge.id,
            InspectionStatus::Passing,
            "checkout is grounded",
            "src/checkout.rs",
            0.9,
            "test",
        )
        .unwrap();

    // A runnable unit proof, actually run: the point of this test is that a
    // passing UNIT proof does not satisfy the journey axis, and that only holds
    // if the unit proof is genuinely passing.
    let unit = store
        .add_node(
            NodeType::Validation,
            "unit proof",
            "",
            "not_run",
            serde_json::json!({
                "type": "test",
                "command": "true",
                "proof_kind": "unit",
                "proof_level": "L2",
            }),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Validates,
            &unit.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    loom::commands::observe_validation(&store, &unit).unwrap();

    ratify_all(&store);
    let before = ladder(&store).unwrap();
    let proven_before = before.rungs.iter().find(|r| r.name == "proven").unwrap();
    assert_eq!(before.phase, "validate");
    assert_eq!(proven_before.state, RungState::Unmet);
    assert!(
        proven_before.detail.contains("1 journey proof gap"),
        "detail should name the journey-proof gap: {}",
        proven_before.detail
    );

    let journey = store
        .add_node(
            NodeType::Validation,
            "journey proof",
            "",
            "passed",
            serde_json::json!({
                "type": "test",
                "command": "true",
                "proof_kind": "journey",
                "proof_level": "L5",
            }),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Validates,
            &journey.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    loom::commands::observe_validation(&store, &journey).unwrap();

    ratify_all(&store);
    let after = ladder(&store).unwrap();
    let proven_after = after.rungs.iter().find(|r| r.name == "proven").unwrap();
    assert_eq!(proven_after.state, RungState::Met);
}

#[test]
fn proven_rung_honors_journey_axis_waiver() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "cli surface holds",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "visibility",
            "user_visible",
            TruthClass::Asserted,
        )
        .unwrap();
    let file = store
        .add_node(
            NodeType::CodeFile,
            "src/cli.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let impl_edge = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &impl_edge.id,
            InspectionStatus::Passing,
            "grounded",
            "src/cli.rs",
            0.9,
            "test",
        )
        .unwrap();
    let unit = store
        .add_node(
            NodeType::Validation,
            "unit proof",
            "",
            "passed",
            serde_json::json!({
                "type": "test",
                "command": "true",
                "proof_kind": "unit",
                "proof_level": "L2",
            }),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Validates,
            &unit.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    loom::commands::observe_validation(&store, &unit).unwrap();

    let before = ladder(&store).unwrap();
    assert_eq!(
        before
            .rungs
            .iter()
            .find(|r| r.name == "proven")
            .unwrap()
            .state,
        RungState::Unmet
    );

    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "waiver:journey",
            "CLI surface; HTTP journey runner does not apply",
            TruthClass::Asserted,
        )
        .unwrap();

    let after = ladder(&store).unwrap();
    let proven = after.rungs.iter().find(|r| r.name == "proven").unwrap();
    assert_eq!(proven.state, RungState::Met);
    assert!(
        !proven.detail.contains("journey proof gap"),
        "waived journey must not count as a proven gap: {}",
        proven.detail
    );
}

// ---- compass: findings route through durable triage --------------------------

#[test]
fn findings_route_to_triage_until_judged() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // one implemented intent, grounded + inspected → graph-maturity residue clean
    let i = store
        .add_node(
            NodeType::Intent,
            "behavior holds",
            "b",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = store
        .add_node(
            NodeType::CodeFile,
            "src/x.rs",
            "",
            "active",
            serde_json::json!({}),
        )
        .unwrap();
    let e = store
        .add_edge(EdgeKind::Implements, &i.id, &cf.id, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &e.id,
            InspectionStatus::Passing,
            "grounded",
            "x",
            0.9,
            "llm",
        )
        .unwrap();
    // A real proof: loom runs it and records what it observed. A
    // hand-written passing verdict is refused now — it is the same
    // shortcut that made this graph report proofs nobody ran.
    loom::commands::prove_intent(&store, &i.id, "proof", "true").unwrap();
    // baseline: graph is clean but not complete until the travel export is fresh.
    ratify_all(&store);
    assert_eq!(ladder(&store).unwrap().phase, "export");
    travel::export_to_file(&store).unwrap();
    assert_eq!(ladder(&store).unwrap().phase, "complete");

    // a single unadjudicated derived finding: graph maturity is affected until
    // the finding is judged, then the durable verdict removes it from triage.
    store
        .add_derived_node(
            NodeType::Finding,
            "oversized_file:src/x.rs:",
            "src/x.rs is oversized",
            "1200 loc",
            "oversized_file",
            serde_json::json!({ "kind": "oversized_file", "symbol": "" }),
        )
        .unwrap();
    let l = ladder(&store).unwrap();
    let excellent = l.rungs.iter().find(|r| r.name == "triaged").unwrap();
    assert_eq!(excellent.state, RungState::Unmet);
    assert_eq!(l.phase, "triage");
    assert_eq!(l.next_command, "loom next --mode triage");

    let f = store
        .list_nodes(Some(NodeType::Finding), usize::MAX)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    store
        .record_finding_verdict(&f.id, "justified", "cohesive")
        .unwrap();
    let judged = ladder(&store).unwrap();
    let judged_excellent = judged.rungs.iter().find(|r| r.name == "triaged").unwrap();
    assert_eq!(judged.phase, "export");
    assert_eq!(judged.next_command, "loom export && loom export --check");
    assert_eq!(judged_excellent.state, RungState::Met);
}

#[test]
fn excellent_rung_when_not_applicable_hides_untriaged_count() {
    // A registered codefile produces a finding, but with no active intents the
    // excellent rung is NotApplicable — it must not advertise an untriaged count
    // that looks actionable while the compass routes elsewhere (seed).
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .add_derived_node(
            NodeType::Finding,
            "oversized_file:src/x.rs:",
            "src/x.rs is oversized",
            "1200 loc",
            "oversized_file",
            serde_json::json!({ "kind": "oversized_file", "symbol": "" }),
        )
        .unwrap();
    let l = ladder(&store).unwrap();
    let excellent = l.rungs.iter().find(|r| r.name == "triaged").unwrap();
    assert_eq!(excellent.state, RungState::NotApplicable);
    assert!(!excellent.detail.contains("untriaged"));
    assert_eq!(l.phase, "seed");
}

#[test]
fn hardened_rung_blocks_on_unmeasured_quality_pairs() {
    // A fully grounded + proven graph with seeded quality rules must report
    // hardened = Unmet until every rule × leaf implemented intent pair has a
    // governs verdict. The detail string must mention unmeasured quality pairs.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    // One implemented root intent, grounded and proven.
    let intent = store
        .add_node(
            NodeType::Intent,
            "user can pay",
            "payment works",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = store
        .add_node(
            NodeType::CodeFile,
            "src/pay.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let imp = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(&imp.id, InspectionStatus::Passing, "c", "e", 0.9, "llm")
        .unwrap();
    loom::commands::prove_intent(&store, &intent.id, "pay test", "true").unwrap();

    // Before seeding: hardened should be Met (no stale, no uninspected, no doctor, no pairs).
    ratify_all(&store);
    let l = ladder(&store).unwrap();
    let hardened = l.rungs.iter().find(|r| r.name == "measured").unwrap();
    assert_eq!(
        hardened.state,
        RungState::Met,
        "hardened is Met before any rules are seeded"
    );

    // Seed a pack — creates quality rules but no governs edges yet.
    let n = loom::packs::seed(&store, "iso5055").unwrap();
    assert!(n > 0, "iso5055 pack seeds at least one rule");

    // Now hardened must be Unmet: unmeasured pairs block it.
    ratify_all(&store);
    let l = ladder(&store).unwrap();
    let hardened = l.rungs.iter().find(|r| r.name == "measured").unwrap();
    assert_eq!(
        hardened.state,
        RungState::Unmet,
        "hardened is Unmet when seeded rules have unmeasured pairs"
    );
    assert!(
        hardened.detail.contains("never-measured rule pair"),
        "hardened detail mentions unmeasured quality pairs: {}",
        hardened.detail
    );

    // Compass should route to quality (assuming earlier rungs are met).
    assert_eq!(l.phase, "quality", "compass routes to quality lane");

    // Measure every rule against the intent → all pairs satisfied.
    let rules = store
        .list_nodes(Some(NodeType::QualityRule), usize::MAX)
        .unwrap();
    for rule in &rules {
        let ge = store
            .add_edge(
                EdgeKind::Governs,
                &rule.id,
                &intent.id,
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .record_verdict(
                &ge.id,
                InspectionStatus::Passing,
                "criterion",
                "evidence",
                0.9,
                "llm",
            )
            .unwrap();
    }

    // Now hardened should be Met.
    ratify_all(&store);
    let l = ladder(&store).unwrap();
    let hardened = l.rungs.iter().find(|r| r.name == "measured").unwrap();
    assert_eq!(
        hardened.state,
        RungState::Met,
        "hardened is Met after all quality pairs are measured"
    );
}
