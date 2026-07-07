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

#[test]
fn build_queue_serves_prerequisites_before_dependents() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // Names chosen so the DEPENDENT sorts FIRST by name — proving readiness,
    // not alphabetical rank, decides what the build lane serves.
    let dependent = intent(&store, "aaa dependent behavior");
    let prereq = intent(&store, "zzz base prerequisite");
    store
        .add_edge(
            EdgeKind::Requires,
            &dependent,
            &prereq,
            TruthClass::Asserted,
        )
        .unwrap();

    // The prerequisite is served first, even though the dependent sorts earlier.
    let item = workitem::next(&store, Some(Mode::Build))
        .unwrap()
        .expect("a build item");
    assert_eq!(
        item.target.id, prereq,
        "build lane serves the prerequisite before the intent that requires it"
    );

    // Once the prerequisite is implemented, the dependent becomes ready.
    store.set_node_status(&prereq, "implemented").unwrap();
    let item = workitem::next(&store, Some(Mode::Build))
        .unwrap()
        .expect("a build item");
    assert_eq!(
        item.target.id, dependent,
        "the dependent is served once its prerequisite is implemented"
    );
}

#[test]
fn build_queue_does_not_stall_on_a_requires_cycle() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "alpha ring");
    let b = intent(&store, "beta ring");
    store
        .add_edge(EdgeKind::Requires, &a, &b, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Requires, &b, &a, TruthClass::Asserted)
        .unwrap();
    // Neither is ready (each requires the other), but the lane must not stall:
    // it serves the top-ranked candidate carrying a blocked reason.
    let item = workitem::next(&store, Some(Mode::Build))
        .unwrap()
        .expect("a build item even under a requires cycle");
    assert!(
        item.reason.starts_with("blocked: requires"),
        "a fully-blocked candidate is served with a blocked reason, got: {}",
        item.reason
    );
}

// ---- resolve_node: exact-name collision and fragment ambiguity list ids -----

#[test]
fn resolve_node_ambiguous_exact_name_lists_candidate_ids() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // Two intents sharing the same name — add_node enforces no uniqueness.
    let a = intent(&store, "shared behavior name");
    let b = intent(&store, "shared behavior name");
    let err = store
        .resolve_node("shared behavior name", Some(NodeType::Intent))
        .unwrap_err();
    let msg = err.to_string();
    // Both nodes' 8-char short ids must appear so the operator can address one.
    assert!(
        msg.contains(&a[..8]),
        "error must list first collision's short id; got: {msg}"
    );
    assert!(
        msg.contains(&b[..8]),
        "error must list second collision's short id; got: {msg}"
    );
}

#[test]
fn resolve_node_ambiguous_fragment_lists_candidate_ids() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // Several intents sharing a fragment substring; none is named exactly "xfrag".
    let ids: Vec<String> = ["xfrag alpha", "xfrag beta", "xfrag gamma"]
        .iter()
        .map(|n| intent(&store, n))
        .collect();
    // "xfrag" LIKE-matches all three but is no one's exact name; 'x' is not a
    // hex digit so the short-id prefix path is skipped cleanly.
    let err = store
        .resolve_node("xfrag", Some(NodeType::Intent))
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        ids.iter().any(|id| msg.contains(&id[..8])),
        "error must list at least one candidate's short id; got: {msg}"
    );
}

// ---- node pagination: windows, counts, list_nodes == list_nodes_page(0) ----

#[test]
fn list_nodes_page_windows_and_counts() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // Five intents whose names sort alphabetically in a known order.
    let names = ["alpha", "beta", "delta", "epsilon", "gamma"];
    for n in &names {
        intent(&store, n);
    }
    const N: usize = 5;

    // count_nodes returns the correct total.
    assert_eq!(store.count_nodes(Some(NodeType::Intent)).unwrap(), N);

    // Page 0: first 2 rows (by name ascending).
    let pg0 = store.list_nodes_page(Some(NodeType::Intent), 2, 0).unwrap();
    assert_eq!(pg0.len(), 2, "page 0 must have 2 rows");

    // Page 1: next 2 rows (offset 2).
    let pg1 = store.list_nodes_page(Some(NodeType::Intent), 2, 2).unwrap();
    assert_eq!(pg1.len(), 2, "page 1 must have 2 rows");

    // Pages are disjoint.
    let pg0_ids: std::collections::HashSet<&str> = pg0.iter().map(|n| n.id.as_str()).collect();
    let pg1_ids: std::collections::HashSet<&str> = pg1.iter().map(|n| n.id.as_str()).collect();
    assert!(
        pg0_ids.is_disjoint(&pg1_ids),
        "page windows must not overlap"
    );

    // Each page is name-ordered internally.
    let pg0_names: Vec<&str> = pg0.iter().map(|n| n.name.as_str()).collect();
    let mut pg0_sorted = pg0_names.clone();
    pg0_sorted.sort_unstable();
    assert_eq!(pg0_names, pg0_sorted, "page 0 must be name-ordered");

    let pg1_names: Vec<&str> = pg1.iter().map(|n| n.name.as_str()).collect();
    let mut pg1_sorted = pg1_names.clone();
    pg1_sorted.sort_unstable();
    assert_eq!(pg1_names, pg1_sorted, "page 1 must be name-ordered");

    // Pages are globally ordered: last of pg0 < first of pg1.
    assert!(
        pg0.last().unwrap().name < pg1.first().unwrap().name,
        "page boundary must maintain global name order"
    );

    // usize::MAX limit returns all rows.
    let all = store
        .list_nodes_page(Some(NodeType::Intent), usize::MAX, 0)
        .unwrap();
    assert_eq!(all.len(), N, "usize::MAX limit must return all rows");

    // Offset past the total returns empty.
    let past = store
        .list_nodes_page(Some(NodeType::Intent), 2, N + 1)
        .unwrap();
    assert!(past.is_empty(), "offset past end must return empty");

    // list_nodes(type, limit) == list_nodes_page(type, limit, 0): same ids in same order.
    let via_list = store.list_nodes(Some(NodeType::Intent), 3).unwrap();
    let via_page = store.list_nodes_page(Some(NodeType::Intent), 3, 0).unwrap();
    let list_ids: Vec<&str> = via_list.iter().map(|n| n.id.as_str()).collect();
    let page_ids: Vec<&str> = via_page.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(
        list_ids, page_ids,
        "list_nodes must be identical to list_nodes_page with offset 0"
    );
}

// ---- edge pagination: windows, counts, list_edges == list_edges_page(0) ----

fn codefile_node(store: &Store, path: &str) -> String {
    store
        .add_node(NodeType::CodeFile, path, "", "", serde_json::json!({}))
        .unwrap()
        .id
}

#[test]
fn list_edges_page_windows_and_counts() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let ia = intent(&store, "edge-pag intent a");
    let ib = intent(&store, "edge-pag intent b");
    let ic = intent(&store, "edge-pag intent c");
    let f1 = codefile_node(&store, "src/pag_a.rs");
    let f2 = codefile_node(&store, "src/pag_b.rs");

    // 5 edges total: 3 implements + 2 relates.
    store
        .add_edge(EdgeKind::Implements, &ia, &f1, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Implements, &ib, &f1, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Implements, &ic, &f2, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Relates, &ia, &ib, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Relates, &ib, &ic, TruthClass::Asserted)
        .unwrap();

    // count_edges(None) covers all kinds.
    let total = store.count_edges(None).unwrap();
    assert_eq!(total, 5, "expected 5 edges total");

    // Ground truth: all edges ordered by id (the store's own ordering guarantee).
    let all = store.list_edges_page(None, usize::MAX, 0).unwrap();
    assert_eq!(all.len(), 5);

    // Page 0 matches the first 2 of the ground-truth id-ordered set.
    let pg0 = store.list_edges_page(None, 2, 0).unwrap();
    assert_eq!(pg0.len(), 2, "page 0 must have 2 rows");
    assert_eq!(
        pg0.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        all[..2].iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        "page 0 must match first 2 of id-ordered ground truth"
    );

    // Page 1 matches the next 2 — disjoint from page 0 by construction.
    let pg1 = store.list_edges_page(None, 2, 2).unwrap();
    assert_eq!(pg1.len(), 2, "page 1 must have 2 rows");
    assert_eq!(
        pg1.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        all[2..4].iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        "page 1 must match next 2 of id-ordered ground truth"
    );

    // Offset past the total returns empty.
    let past = store.list_edges_page(None, 2, total + 1).unwrap();
    assert!(past.is_empty(), "offset past total must return empty");

    // list_edges(None, limit) == list_edges_page(None, limit, 0): same ids in same order.
    let via_list = store.list_edges(None, 3).unwrap();
    let via_page = store.list_edges_page(None, 3, 0).unwrap();
    assert_eq!(
        via_list.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        via_page.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        "list_edges must be identical to list_edges_page with offset 0"
    );
}
