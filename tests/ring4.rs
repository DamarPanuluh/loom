//! Ring 4 tests — maturity ladder + compass routing.

use loom::maturity::{ladder, RungState};
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use loom::travel;
use loom::workitem::{self, Mode};
mod common;
use common::*;

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
    let l = ladder(&store).unwrap();
    assert_eq!(l.phase, "build");
    // seeded met, realized unmet
    assert_eq!(l.rungs[0].state, RungState::Met);
    assert_eq!(l.rungs[1].state, RungState::Unmet);
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
    let l = ladder(&store).unwrap();
    assert_eq!(
        l.phase, "build",
        "ungrounded implemented intent routes to build"
    );
    assert_eq!(l.next_command, "loom next --mode build");

    // The build queue is non-empty and serves exactly that intent — the compass
    // and the queue partition agree.
    let counts = workitem::queue_counts(&store).unwrap();
    assert_eq!(
        counts.build, 1,
        "the ungrounded intent is counted in the build queue"
    );
    let item = workitem::next(&store, Some(Mode::Build))
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
        workitem::queue_counts(&store).unwrap().build,
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
        workitem::queue_counts(&store).unwrap().build,
        0,
        "observed graph disables the build lane"
    );

    // Flip back to owned: the lane returns.
    assert!(!store.set_observed(false).unwrap());
    assert!(
        !store.identity().unwrap().observed,
        "mode set back to owned"
    );
    assert_eq!(workitem::queue_counts(&store).unwrap().build, 1);
}

#[test]
fn observed_graph_compass_never_routes_to_a_disabled_lane() {
    // Regression: on an observed graph the build/coverage/fix lanes are disabled
    // (`queue_counts` forces them to 0). An ungrounded implemented intent leaves
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

    let counts = workitem::queue_counts(&store).unwrap();
    assert_eq!(
        counts.build, 0,
        "build lane is disabled on an observed graph"
    );
    assert_eq!(
        counts.coverage, 0,
        "coverage lane is disabled on an observed graph"
    );

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
    let l = ladder(&store).unwrap();

    // The gate is the lowest Unmet rung: `realized` at index 1.
    let gate = l.rungs.iter().position(|r| r.state == RungState::Unmet);
    assert_eq!(gate, Some(1), "realized is the lowest unmet rung");

    // Gate and everything below it are never blocked.
    assert!(!l.rungs[0].blocked, "seeded (below gate) is not blocked");
    assert!(!l.rungs[1].blocked, "the gate rung itself is not blocked");
    assert_eq!(l.rungs[1].blocked_by, None);

    // Every rung above the gate is blocked by it — including `excellent`, which
    // is independently Met but must not read as satisfied above unmet `realized`.
    for r in &l.rungs[2..] {
        assert!(
            r.blocked,
            "{} sits above the gate and must be blocked",
            r.name
        );
        assert_eq!(r.blocked_by.as_deref(), Some("realized"));
    }
    let excellent = l.rungs.iter().find(|r| r.name == "excellent").unwrap();
    assert_eq!(
        excellent.state,
        RungState::Met,
        "excellent's own truth is unchanged"
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
    // hardened unmet because of the failing edge
    let hardened = l.rungs.iter().find(|r| r.name == "hardened").unwrap();
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
    let v = store
        .add_node(
            NodeType::Validation,
            "proof",
            "",
            "passed",
            serde_json::json!({}),
        )
        .unwrap();
    let ve = store
        .add_edge(EdgeKind::Validates, &v.id, &a.id, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &ve.id,
            InspectionStatus::Passing,
            "proof",
            "cargo test proof",
            1.0,
            "llm",
        )
        .unwrap();
    let before_export = ladder(&store).unwrap();
    assert_eq!(before_export.phase, "export");
    let exported = before_export
        .rungs
        .iter()
        .find(|r| r.name == "exported")
        .unwrap();
    assert_eq!(exported.state, RungState::Unmet);
    travel::export_to_file(&store).unwrap();
    let after_export = ladder(&store).unwrap();
    assert_eq!(after_export.phase, "complete");
    assert_eq!(
        after_export
            .rungs
            .iter()
            .find(|r| r.name == "exported")
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
    let proof = store
        .add_node(
            NodeType::Validation,
            "proof",
            "",
            "passed",
            serde_json::json!({}),
        )
        .unwrap();
    let proof_edge = store
        .add_edge(
            EdgeKind::Validates,
            &proof.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &proof_edge.id,
            InspectionStatus::Passing,
            "proof",
            "cargo test proof",
            1.0,
            "llm",
        )
        .unwrap();

    let l = ladder(&store).unwrap();
    assert_eq!(l.phase, "coverage");
    assert_eq!(l.next_command, "loom coverage");
    let realized = l.rungs.iter().find(|r| r.name == "realized").unwrap();
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
    let proof = store
        .add_node(
            NodeType::Validation,
            "proof",
            "",
            "passed",
            serde_json::json!({}),
        )
        .unwrap();
    let proof_edge = store
        .add_edge(
            EdgeKind::Validates,
            &proof.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &proof_edge.id,
            InspectionStatus::Passing,
            "proof",
            "cargo test proof",
            1.0,
            "llm",
        )
        .unwrap();

    // Pre-ignore: the unowned file is a real coverage gap — it heads the queue
    // and blocks the realized rung. (Sanity, so the test cannot pass on a
    // silently-empty graph.)
    let before = workitem::next(&store, Some(Mode::Coverage)).unwrap();
    let before = before.expect("unowned codefile must surface a coverage work item");
    assert_eq!(before.mode, "coverage");
    assert_eq!(before.target.name, "src/vendored.rs");
    let realized_before = ladder(&store)
        .unwrap()
        .rungs
        .into_iter()
        .find(|r| r.name == "realized")
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
        workitem::next(&store, Some(Mode::Coverage))
            .unwrap()
            .is_none(),
        "an ignored file must not surface as coverage work"
    );
    let realized_after = ladder(&store)
        .unwrap()
        .rungs
        .into_iter()
        .find(|r| r.name == "realized")
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
        let proof = store
            .add_node(
                NodeType::Validation,
                name,
                "",
                "passed",
                serde_json::json!({}),
            )
            .unwrap();
        let proof_edge = store
            .add_edge(
                EdgeKind::Validates,
                &proof.id,
                &intent.id,
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .record_verdict(
                &proof_edge.id,
                InspectionStatus::Passing,
                "proof",
                "cargo test proof",
                1.0,
                "llm",
            )
            .unwrap();
    }

    let l = ladder(&store).unwrap();
    assert_eq!(l.phase, "audit");
    assert_eq!(l.next_command, "loom doctor");
    let hardened = l.rungs.iter().find(|r| r.name == "hardened").unwrap();
    assert_eq!(hardened.state, RungState::Unmet);
    assert!(hardened.detail.contains("1 doctor issue(s)"));
    // audit-by-doctor is graph-integrity, so it maps to the verdict axis, not signal.
    assert_eq!(l.truth_axis, Some(loom::truth::TruthAxis::Verdict));
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
    let proof = store
        .add_node(
            NodeType::Validation,
            "proof a",
            "",
            "passed",
            serde_json::json!({}),
        )
        .unwrap();
    let proof_edge = store
        .add_edge(EdgeKind::Validates, &proof.id, &a.id, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &proof_edge.id,
            InspectionStatus::Passing,
            "proof",
            "cargo test proof_a",
            1.0,
            "llm",
        )
        .unwrap();

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

    let unit = store
        .add_node(
            NodeType::Validation,
            "unit proof",
            "",
            "passed",
            serde_json::json!({
                "type": "test",
                "command": "cargo test unit",
                "proof_kind": "unit",
                "proof_level": "L2",
            }),
        )
        .unwrap();
    let unit_edge = store
        .add_edge(
            EdgeKind::Validates,
            &unit.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &unit_edge.id,
            InspectionStatus::Passing,
            "unit proof passed",
            "unit exit 0",
            0.9,
            "test",
        )
        .unwrap();

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
                "command": "cargo test journey",
                "proof_kind": "journey",
                "proof_level": "L5",
            }),
        )
        .unwrap();
    let journey_edge = store
        .add_edge(
            EdgeKind::Validates,
            &journey.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &journey_edge.id,
            InspectionStatus::Passing,
            "journey proof passed",
            "journey exit 0",
            0.9,
            "test",
        )
        .unwrap();

    let after = ladder(&store).unwrap();
    let proven_after = after.rungs.iter().find(|r| r.name == "proven").unwrap();
    assert_eq!(proven_after.state, RungState::Met);
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
    let v = store
        .add_node(
            NodeType::Validation,
            "proof",
            "",
            "passed",
            serde_json::json!({}),
        )
        .unwrap();
    let ve = store
        .add_edge(EdgeKind::Validates, &v.id, &i.id, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &ve.id,
            InspectionStatus::Passing,
            "proof",
            "cargo test proof",
            1.0,
            "llm",
        )
        .unwrap();

    // baseline: graph is clean but not complete until the travel export is fresh.
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
    let excellent = l.rungs.iter().find(|r| r.name == "excellent").unwrap();
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
    let judged_excellent = judged.rungs.iter().find(|r| r.name == "excellent").unwrap();
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
    let excellent = l.rungs.iter().find(|r| r.name == "excellent").unwrap();
    assert_eq!(excellent.state, RungState::NotApplicable);
    assert!(!excellent.detail.contains("untriaged"));
    assert_eq!(l.phase, "seed");
}
