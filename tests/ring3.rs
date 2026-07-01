//! Ring 3 invariant tests — judgment plane: lane gates (INV-7), the asserted
//! residue router + prompt contract, and intent-redefinition ripple.

use loom::model::{EdgeKind, InspectionStatus, NodeType, TruthClass};
use loom::registry::OwnerRole;
use loom::store::{Agent, Store};
use loom::workitem::{self, Mode};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Tmp {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("loom-ring3-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

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
            "",
            "no link",
            0.9,
            "llm"
        )
        .is_ok());
}

#[test]
fn agent_parses_loom_agent_values() {
    assert_eq!(Agent::parse("llm"), Agent::Solo);
    assert_eq!(Agent::parse("llm:builder"), Agent::Lane(OwnerRole::Builder));
    assert_eq!(Agent::parse("quality"), Agent::Lane(OwnerRole::Quality));
    assert_eq!(Agent::parse("nonsense"), Agent::Solo);
}

// ---- loom next serves a work item + prompt contract ------------------------

#[test]
fn next_build_serves_planned_intent_with_contract() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    intent(&store, "payment can be captured");
    let item = workitem::next(&store, Some(Mode::Build)).unwrap().unwrap();
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
fn next_analyze_serves_uninspected_then_fix_after_stale() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "intent a");
    let b = intent(&store, "intent b");
    store
        .add_edge(EdgeKind::Relates, &a, &b, TruthClass::Asserted)
        .unwrap();
    // uninspected → analyze queue serves it
    let item = workitem::next(&store, Some(Mode::Analyze))
        .unwrap()
        .unwrap();
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
