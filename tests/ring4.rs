//! Ring 4 tests — maturity ladder + compass routing.

use loom::maturity::{ladder, RungState};
use loom::model::{EdgeKind, InspectionStatus, NodeType, TruthClass};
use loom::store::Store;
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
        let p = std::env::temp_dir().join(format!("loom-ring4-{}-{nanos}-{n}", std::process::id()));
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

#[test]
fn empty_graph_compass_routes_to_seed() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let l = ladder(&store).unwrap();
    assert_eq!(l.phase, "seed");
    assert_eq!(l.rungs[0].state, RungState::Unmet); // seeded unmet on empty graph
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
    let l = ladder(&store).unwrap();
    assert_eq!(l.phase, "complete");
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

    // baseline: nothing left → complete
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
    assert_ne!(judged.phase, "triage");
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
