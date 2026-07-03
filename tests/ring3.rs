//! Ring 3 invariant tests — judgment plane lane gates (INV-7), asserted
//! residue routing, prompt contracts, and intent-redefinition ripple.
//! INV-1 O(N) hierarchy coverage is not present in rings 1-3 and remains a gap.

use loom::model::{EdgeKind, InspectionStatus, NodeType, TruthClass};
use loom::registry::OwnerRole;
use loom::store::{Agent, Store};
use loom::workitem::{self, Mode};
mod common;
use common::*;

fn intent(store: &Store, name: &str) -> String {
    store
        .add_node(
            NodeType::Intent,
            name,
            "b",
            "planned",
            serde_json::json!({}),
        )
        .unwrap()
        .id
}

// ---- INV-7 : lane gates ----------------------------------------------------

#[test]
fn inv7_wrong_lane_rejected_right_lane_allowed() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "user can log in");
    let b = intent(&store, "user can log out");

    // quality lane cannot write a builder-owned hierarchy edge
    store.set_agent(Agent::Lane(OwnerRole::Quality));
    assert!(store
        .add_edge(EdgeKind::Hierarchy, &a, &b, TruthClass::Asserted)
        .is_err());

    // builder lane can
    store.set_agent(Agent::Lane(OwnerRole::Builder));
    assert!(store
        .add_edge(EdgeKind::Hierarchy, &a, &b, TruthClass::Asserted)
        .is_ok());

    // solo can drive every lane
    let c = intent(&store, "user can reset password");
    store.set_agent(Agent::Solo);
    assert!(store
        .add_edge(EdgeKind::Hierarchy, &a, &c, TruthClass::Asserted)
        .is_ok());
}

#[test]
fn inv7_verdict_lane_enforced() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "intent a");
    let b = intent(&store, "intent b");
    let edge = store
        .add_edge(EdgeKind::Relates, &a, &b, TruthClass::Asserted)
        .unwrap();
    // relates is analyzer-owned; a validator may not verdict it
    store.set_agent(Agent::Lane(OwnerRole::Validator));
    assert!(store
        .record_verdict(
            &edge.id,
            InspectionStatus::Independent,
            "",
            "no link",
            0.9,
            "llm"
        )
        .is_err());
    // analyzer may
    store.set_agent(Agent::Lane(OwnerRole::Analyzer));
    assert!(store
        .record_verdict(
            &edge.id,
            InspectionStatus::Independent,
            "the two behaviors are unrelated",
            "no link",
            0.9,
            "llm"
        )
        .is_ok());
}

#[test]
fn agent_parses_loom_agent_values() {
    assert_eq!(Agent::parse("llm").unwrap(), Agent::Solo);
    assert_eq!(
        Agent::parse("llm:builder").unwrap(),
        Agent::Lane(OwnerRole::Builder)
    );
    assert_eq!(
        Agent::parse("quality").unwrap(),
        Agent::Lane(OwnerRole::Quality)
    );
    // H-4: an unrecognized lane fails closed (was silently Agent::Solo).
    assert!(Agent::parse("llm:qualtiy").is_err());
    assert!(Agent::parse("nonsense").is_err());
}

// ---- loom next serves a work item + prompt contract ------------------------

#[test]
fn next_build_serves_planned_intent_with_contract() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    intent(&store, "payment can be captured");
    let item = workitem::next(&store, Some(Mode::Build))
        .expect("next build ok")
        .expect("build item exists");
    assert_eq!(item.mode, "build");
    assert_eq!(item.owner_role, "builder");
    assert_eq!(item.target.name, "payment can be captured");
    assert!(!item.prompt_contract.allowed_actions.is_empty());
    assert!(!item.prompt_contract.write_back.is_empty());
    // serializable for --json
    let json = serde_json::to_string(&item).unwrap();
    assert!(json.contains("prompt_contract"));
}

#[test]
fn next_build_context_points_to_target_and_codefile_survey() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let target_id = intent(&store, "payment can be captured");

    let item = workitem::next(&store, Some(Mode::Build))
        .expect("next build ok")
        .expect("build item exists");

    assert!(item.context.linked_entities.iter().any(|entity| {
        entity.role == "target" && entity.kind == "intent" && entity.id == target_id
    }));
    assert!(item
        .context
        .suggested_reads
        .iter()
        .any(|read| read.command.starts_with("loom intent show ")));
    assert!(item
        .context
        .suggested_reads
        .iter()
        .any(|read| read.command == "loom codefile list"));
}

#[test]
fn next_items_carry_the_right_truth_axis() {
    use loom::truth::TruthAxis;
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "intent a");
    let b = intent(&store, "intent b");

    // build → implementation truth
    let build = workitem::next(&store, Some(Mode::Build))
        .expect("next build ok")
        .expect("build item exists");
    assert_eq!(build.truth_gap.axis, TruthAxis::Implementation);
    assert!(!build.truth_gap.authoritative_write.is_empty());

    // an uninspected relationship edge → analyze → verdict truth
    store
        .add_edge(EdgeKind::Relates, &a, &b, TruthClass::Asserted)
        .unwrap();
    let analyze = workitem::next(&store, Some(Mode::Analyze))
        .expect("next analyze ok")
        .expect("analyze item exists");
    assert_eq!(analyze.truth_gap.axis, TruthAxis::Verdict);

    // a not_run validation → validate → proof truth
    let v = store
        .add_node(
            NodeType::Validation,
            "proof",
            "",
            "not_run",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_edge(EdgeKind::Validates, &v.id, &a, TruthClass::Asserted)
        .unwrap();
    let validate = workitem::next(&store, Some(Mode::Validate))
        .expect("next validate ok")
        .expect("validate item exists");
    assert_eq!(validate.truth_gap.axis, TruthAxis::Proof);

    // serializable for --json
    let json = serde_json::to_string(&build).unwrap();
    assert!(json.contains("truth_gap"));
    assert!(json.contains("implementation"));
}

#[test]
fn next_edge_context_points_to_endpoints_edge_and_grounded_codefile() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "intent a");
    let b = intent(&store, "intent b");
    let file = store
        .add_node(
            NodeType::CodeFile,
            "src/a.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let grounding = store
        .add_edge(EdgeKind::Implements, &a, &file.id, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &grounding.id,
            InspectionStatus::Passing,
            "grounded",
            "src/a.rs",
            0.95,
            "llm",
        )
        .unwrap();
    let relates = store
        .add_edge(EdgeKind::Relates, &a, &b, TruthClass::Asserted)
        .unwrap();

    let item = workitem::next(&store, Some(Mode::Analyze))
        .expect("next analyze ok")
        .expect("analyze item exists");

    assert!(item.context.linked_entities.iter().any(|entity| {
        entity.role == "target_edge" && entity.kind == "edge" && entity.id == relates.id
    }));
    assert!(item
        .context
        .linked_entities
        .iter()
        .any(|entity| entity.role == "from" && entity.id == a));
    assert!(item
        .context
        .linked_entities
        .iter()
        .any(|entity| entity.role == "to" && entity.id == b));
    assert!(item
        .context
        .linked_entities
        .iter()
        .any(|entity| entity.role == "grounded_codefile" && entity.id == file.id));
    assert!(item
        .context
        .suggested_reads
        .iter()
        .any(|read| read.command == format!("loom edge show {}", relates.id)));
    assert!(item
        .context
        .suggested_reads
        .iter()
        .any(|read| read.command == format!("loom codefile show {}", file.id)));
}

#[test]
fn next_analyze_serves_uninspected_claim() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "intent a");
    let b = intent(&store, "intent b");
    store
        .add_edge(EdgeKind::Relates, &a, &b, TruthClass::Asserted)
        .unwrap();
    // uninspected → analyze queue serves it
    let item = workitem::next(&store, Some(Mode::Analyze))
        .expect("next analyze ok")
        .expect("analyze item exists");
    assert_eq!(item.owner_role, "analyzer");
    assert_eq!(item.target.kind, "edge");

    // build queue empty (no planned intents need build? they are planned) -> build serves them
    let build = workitem::next(&store, Some(Mode::Build)).unwrap();
    assert!(build.is_some(), "planned intents are build work");
}

// ---- intent redefinition ripples one hop -----------------------------------

#[test]
fn redefine_intent_reopens_settled_verdicts() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "intent a");
    let b = intent(&store, "intent b");
    let edge = store
        .add_edge(EdgeKind::Relates, &a, &b, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(&edge.id, InspectionStatus::Passing, "c", "e", 0.9, "llm")
        .unwrap();

    let reopened = store
        .redefine_intent(&a, "intent a, but meaning evolved")
        .unwrap();
    assert_eq!(reopened, 1, "the passing relates edge must re-open");
    assert_eq!(
        store.get_edge(&edge.id).unwrap().unwrap().status,
        InspectionStatus::NeedsReverification
    );
    // old wording preserved as a note
    let notes = store.list_nodes(Some(NodeType::Note), usize::MAX).unwrap();
    assert!(notes
        .iter()
        .any(|n| n.description.contains("previous description")));
}

#[test]
fn retire_intent_marks_deprecated() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "old behavior");
    store.retire_intent(&a, "superseded", None).unwrap();
    assert_eq!(store.get_node(&a).unwrap().unwrap().status, "deprecated");
}

// ---- edge id resolution: 8-char prefixes the CLI prints must be actionable --

#[test]
fn resolve_edge_by_prefix_or_errors() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "intent a");
    let b = intent(&store, "intent b");
    let e = store
        .add_edge(EdgeKind::Relates, &a, &b, TruthClass::Asserted)
        .unwrap();

    // exact id resolves
    assert_eq!(store.resolve_edge(&e.id).unwrap().id, e.id);
    // the 8-char prefix that `find`/`next`/`edge list` print resolves the same
    assert_eq!(store.resolve_edge(&e.id[..8]).unwrap().id, e.id);
    // a prefix that matches no edge errors — never a silent guess
    assert!(store.resolve_edge("zzzzzzzz").is_err());

    // with a second edge present, an all-matching key is ambiguous, not a guess
    let c = intent(&store, "intent c");
    store
        .add_edge(EdgeKind::Relates, &a, &c, TruthClass::Asserted)
        .unwrap();
    assert!(store.resolve_edge("").is_err());
}
