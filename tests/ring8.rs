//! Ring 8 tests — durable finding triage.

use loom::maturity::ladder;
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store;
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
        let p = std::env::temp_dir().join(format!("loom-ring8-{}-{nanos}-{n}", std::process::id()));
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

fn derived_finding(store: &Store) -> loom::model::Node {
    store
        .add_derived_node(
            NodeType::Finding,
            "oversized_file:src/x.rs:",
            "src/x.rs is oversized",
            "1200 lines",
            "oversized_file",
            serde_json::json!({ "kind": "oversized_file", "symbol": "" }),
        )
        .unwrap()
}

fn mature_graph_with_codefile(store: &Store) -> loom::model::Node {
    let intent = store
        .add_node(
            NodeType::Intent,
            "behavior holds",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let codefile = store
        .add_node(
            NodeType::CodeFile,
            "src/x.rs",
            "",
            "active",
            serde_json::json!({}),
        )
        .unwrap();
    let edge = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &codefile.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "grounded",
            "src/x.rs",
            0.9,
            "llm",
        )
        .unwrap();
    let validation = store
        .add_node(
            NodeType::Validation,
            "proof",
            "",
            "passed",
            serde_json::json!({}),
        )
        .unwrap();
    let ve = store
        .add_edge(
            EdgeKind::Validates,
            &validation.id,
            &intent.id,
            TruthClass::Asserted,
        )
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
    codefile
}

#[test]
fn finding_adjudication_survives_derived_graph_wipe() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let finding = derived_finding(&store);
    store
        .record_finding_verdict(&finding.id, "justified", "cohesive")
        .unwrap();

    store.wipe_derived_graph().unwrap();
    let rebuilt = derived_finding(&store);
    assert_eq!(rebuilt.id, finding.id);

    let raw = store
        .get_facet(&finding.id, TargetKind::Node, "adjudication")
        .unwrap()
        .expect("asserted adjudication facet survives derived wipe");
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["verdict"], "justified");
    assert_eq!(value["reason"], "cohesive");
}

#[test]
fn finding_adjudication_goes_stale_when_codefile_hash_changes() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let codefile = mature_graph_with_codefile(&store);
    store
        .set_facet(
            &codefile.id,
            TargetKind::Node,
            "content_hash",
            "h1",
            TruthClass::Derived,
        )
        .unwrap();
    let finding = derived_finding(&store);
    store
        .add_derived_edge(EdgeKind::Flags, &finding.id, &codefile.id)
        .unwrap();
    store
        .record_finding_verdict(&finding.id, "justified", "cohesive")
        .unwrap();

    let fresh = loom::signal::findings_view(&store).unwrap();
    assert_eq!(fresh.len(), 1);
    assert!(!fresh[0].stale);
    assert!(loom::signal::untriaged_findings(&store).unwrap().is_empty());

    store
        .set_facet(
            &codefile.id,
            TargetKind::Node,
            "content_hash",
            "h2",
            TruthClass::Derived,
        )
        .unwrap();
    let stale = loom::signal::findings_view(&store).unwrap();
    assert!(stale[0].stale);
    assert_eq!(loom::signal::untriaged_findings(&store).unwrap().len(), 0);
    assert_eq!(loom::signal::stale_findings(&store).unwrap().len(), 1);
    assert_eq!(loom::signal::triage_findings(&store).unwrap().len(), 1);
}

#[test]
fn triage_mode_serves_findings_until_verdict_is_recorded() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    mature_graph_with_codefile(&store);
    let finding = derived_finding(&store);

    let item = workitem::next(&store, Some(Mode::Triage))
        .unwrap()
        .expect("untriaged finding is served");
    assert_eq!(item.mode, "triage");
    assert_eq!(item.target.id, finding.id);
    // The contract is copy-paste runnable: the concrete short id, not a `<id>`
    // placeholder, so a text-mode agent never needs a separate `finding list`.
    let short = &finding.id[..8];
    assert!(item.prompt_contract.write_back.contains(short));
    assert!(!item.prompt_contract.write_back.contains("<id>"));
    assert!(item
        .prompt_contract
        .allowed_actions
        .iter()
        .all(|a| a.contains(short)));
    assert_eq!(ladder(&store).unwrap().phase, "triage");

    store
        .record_finding_verdict(&finding.id, "justified", "cohesive")
        .unwrap();
    assert!(workitem::next(&store, Some(Mode::Triage))
        .unwrap()
        .is_none());
    let judged = ladder(&store).unwrap();
    assert_ne!(judged.phase, "triage");
}

#[test]
fn triage_item_surfaces_owning_intents_as_cohesion_evidence() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let codefile = mature_graph_with_codefile(&store);
    let finding = derived_finding(&store);
    store
        .add_derived_edge(EdgeKind::Flags, &finding.id, &codefile.id)
        .unwrap();

    let item = workitem::next(&store, Some(Mode::Triage))
        .unwrap()
        .expect("untriaged finding is served");
    // The judgment input comes from the graph, not grep: the flagged file's
    // owning intent is named in the work item so cohesion is judged at a glance.
    assert!(item.reason.contains("owns 1 intent(s)"));
    assert!(item.reason.contains("behavior holds"));
}

#[test]
fn graph_state_counts_needed_findings() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let finding = derived_finding(&store);
    assert_eq!(workitem::graph_state(&store).unwrap().needed, 0);
    store
        .record_finding_verdict(&finding.id, "needed", "split it")
        .unwrap();
    let pulse = workitem::graph_state(&store).unwrap();
    assert_eq!(pulse.needed, 1);
    // A `needed` verdict is a judgment, so it leaves raw untriaged.
    assert_eq!(pulse.untriaged, 0);
    assert_eq!(pulse.stale_findings, 0);
}
