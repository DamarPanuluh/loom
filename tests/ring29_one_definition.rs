//! Ring 29 — one definition per quantity.
//!
//! The defect class that bit four times in a single session, each time in a
//! different place, each time fixed only where it was found:
//!
//! - ownership had two definitions (`unowned_codefiles` and an inline copy in
//!   `code_ownership_summary`), so `loom coverage` called a verified test file
//!   unowned while the queue had stopped serving it;
//! - the proof tally had two, so `status` reported "1 failed" for a capability
//!   the gate had already excluded;
//! - lane depth and lane queue had two, so `proven` advertised 14 items its
//!   queue could not hand back;
//! - and `unproven implemented` had two, so teaching the QUEUE that a proof
//!   must reach S2 left the RUNG counting bare passes — 15 against 59.
//!
//! Every pair agreed until one of them changed. That is the shape: a number
//! computed twice is a number that will disagree with itself the moment
//! someone improves one copy, and the disagreement is silent because both
//! answers look plausible.
//!
//! Stated once: **a lane's reported depth is the size of the work it can hand
//! out.** If a counter and a queue are ever derived separately again, this
//! fails — wherever it happens, without anyone remembering to look.

use loom::lane::Lane;
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store;
mod common;
use common::*;

/// A graph with something outstanding in as many lanes at once as possible, so
/// the comparison below is not vacuously true on empty queues.
fn busy_graph(root: &std::path::Path) -> Store {
    let store = Store::init(root, Some("t"), false).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();

    // Grounded + proven, so `covered` and `proven` have real content.
    std::fs::write(root.join("src/kept.rs"), "pub fn kept() -> u8 { 1 }\n").unwrap();
    let proven = store
        .add_node(
            NodeType::Intent,
            "a proven behavior",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = store
        .add_node(NodeType::CodeFile, "src/kept.rs", "", "", serde_json::json!({}))
        .unwrap();
    let g = store
        .add_edge(EdgeKind::Implements, &proven.id, &cf.id, TruthClass::Asserted)
        .unwrap();
    store
        .set_facet(&g.id, TargetKind::Edge, "locator", "fn kept", TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &g.id,
            InspectionStatus::Passing,
            "lives here",
            "src/kept.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    prove_s2(&store, root, &proven.id, "kept");

    // Implemented but unproven — validate work.
    std::fs::write(root.join("src/bare.rs"), "pub fn bare() -> u8 { 2 }\n").unwrap();
    let bare = store
        .add_node(
            NodeType::Intent,
            "an unproven behavior",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let bare_cf = store
        .add_node(NodeType::CodeFile, "src/bare.rs", "", "", serde_json::json!({}))
        .unwrap();
    store
        .add_edge(EdgeKind::Implements, &bare.id, &bare_cf.id, TruthClass::Asserted)
        .unwrap();

    // An unowned file — coverage work.
    std::fs::write(root.join("src/orphan.rs"), "pub fn orphan() {}\n").unwrap();
    store
        .add_node(
            NodeType::CodeFile,
            "src/orphan.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();

    // An uninspected relationship — analyze work.
    store
        .add_edge(EdgeKind::Relates, &proven.id, &bare.id, TruthClass::Asserted)
        .unwrap();

    // A RETIRED behavior with its own governs and validates edges. This is the
    // shape my first fixture missed and the real graph exposed: the depth uses
    // `live_edges_by_status`, which drops claims about retired behaviors, while
    // the rosters read raw edges — so the roster ran longer than the number
    // printed beside it.
    let doomed = store
        .add_node(
            NodeType::Intent,
            "a behavior since removed",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let doomed_rule = store
        .add_node(
            NodeType::QualityRule,
            "a rule on the removed behavior",
            "d",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Governs,
            &doomed_rule.id,
            &doomed.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let doomed_val = store
        .add_node(
            NodeType::Validation,
            "its proof",
            "",
            "not_run",
            serde_json::json!({ "type": "test", "command": "true" }),
        )
        .unwrap();
    store
        .ensure_edge(EdgeKind::Validates, &doomed_val.id, &doomed.id)
        .unwrap();
    store
        .retire_intent(&doomed.id, "deleted on purpose", None)
        .unwrap();

    // An unmeasured quality rule — quality work.
    let rule = store
        .add_node(
            NodeType::QualityRule,
            "a rule",
            "d",
            "",
            serde_json::json!({ "patterns": ["never_matches_anything_xyzzy"] }),
        )
        .unwrap();
    store
        .add_edge(EdgeKind::Governs, &rule.id, &proven.id, TruthClass::Asserted)
        .unwrap();

    loom::sync::run(&store, root).unwrap();
    store
}

/// A lane's depth is the size of the work it can actually hand out.
///
/// Not "roughly agrees" and not "both are non-zero": the same number. A rung
/// that says 14 while its roster holds 2 is advertising work nobody can
/// collect, and a rung that says 2 while its roster holds 14 is hiding work
/// nobody will be shown.
#[test]
fn every_lane_reports_the_depth_it_can_serve() {
    let tmp = Tmp::new();
    let store = busy_graph(tmp.path());

    let ladder = loom::maturity::ladder(&store).unwrap();
    let depths = loom::maturity::depths(&store).unwrap();

    let mut disagreements: Vec<String> = Vec::new();
    for lane in Lane::LADDER {
        if !lane.serves_items() {
            continue; // routes to a whole-graph command, not a per-item roster
        }
        let roster = loom::workitem::queue_items(&store, *lane).unwrap().len();
        let depth = depths.get(*lane);
        if roster != depth {
            let rung = ladder
                .rungs
                .iter()
                .find(|r| r.lane == *lane)
                .map(|r| r.name.as_str())
                .unwrap_or("?");
            disagreements.push(format!(
                "  lane '{}' (rung '{rung}'): depth says {depth}, roster holds {roster}",
                lane.as_str()
            ));
        }
    }

    assert!(
        disagreements.is_empty(),
        "a quantity computed twice has disagreed with itself — the counter and \
         the queue must derive from ONE predicate:\n{}",
        disagreements.join("\n")
    );
}

/// The same rule for the two summaries that describe ownership.
///
/// `code_ownership_summary` used to carry its own copy of the ownership rule
/// while `unowned_codefiles` documented itself as "the single definition of the
/// coverage gap". They agreed by coincidence until the test-file rule landed in
/// one of them.
#[test]
fn ownership_has_one_definition() {
    let tmp = Tmp::new();
    let store = busy_graph(tmp.path());

    let queue = loom::commands::unowned_names(&store).unwrap();
    let (_, _, summary, _) = loom::commands::code_ownership_summary(&store).unwrap();
    assert_eq!(
        queue, summary,
        "the coverage summary and the coverage queue must be the same list"
    );
}
