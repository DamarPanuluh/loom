//! Ring 8 tests — durable finding triage.

use loom::maturity::ladder;
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use loom::workitem::{self, Mode};
mod common;
use common::*;

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

#[test]
fn graph_state_splits_findings_into_open_and_resolved() {
    // Fresh untriaged finding is open; adjudicating it as justified flips it
    // to resolved. The invariant open + resolved == total holds throughout,
    // and a finding that is both `needed` and stale counts once in open.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let finding = derived_finding(&store);

    // (1) Untriaged: open, nothing resolved.
    let pulse = workitem::graph_state(&store).unwrap();
    assert_eq!(pulse.findings, 1);
    assert_eq!(pulse.open_findings, 1);
    assert_eq!(pulse.resolved_findings, 0);
    assert_eq!(
        pulse.open_findings + pulse.resolved_findings,
        pulse.findings
    );

    // (2) Justified verdict resolves it: open decrements, resolved increments.
    store
        .record_finding_verdict(&finding.id, "justified", "cohesive")
        .unwrap();
    let pulse = workitem::graph_state(&store).unwrap();
    assert_eq!(pulse.findings, 1);
    assert_eq!(pulse.open_findings, 0);
    assert_eq!(pulse.resolved_findings, 1);
    assert_eq!(
        pulse.open_findings + pulse.resolved_findings,
        pulse.findings
    );

    // (3) A `needed` finding whose codefile hash later diverges is both needed
    // and stale. Naive untriaged+stale+needed addition would count it twice
    // (as 2), but the contract counts the finding once in open and zero in
    // resolved, preserving open + resolved == total.
    let tmp2 = Tmp::new();
    let store = Store::init(tmp2.path(), Some("t"), false).unwrap();
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
    // Stamp `needed` while the hash is h1, so the verdict records hash=h1.
    store
        .record_finding_verdict(&finding.id, "needed", "split it")
        .unwrap();
    let current = workitem::graph_state(&store).unwrap();
    assert_eq!(current.findings, 1);
    assert_eq!(current.open_findings, 1);
    assert_eq!(current.resolved_findings, 0);
    assert_eq!(
        current.open_findings + current.resolved_findings,
        current.findings
    );

    // Diverge the codefile hash: the finding is now needed AND stale.
    store
        .set_facet(
            &codefile.id,
            TargetKind::Node,
            "content_hash",
            "h2",
            TruthClass::Derived,
        )
        .unwrap();
    let stale = workitem::graph_state(&store).unwrap();
    assert_eq!(stale.findings, 1);
    assert_eq!(stale.needed, 1);
    assert_eq!(stale.stale_findings, 1);
    // Counted once, not twice — the regression this defends against.
    assert_eq!(stale.open_findings, 1);
    assert_eq!(stale.resolved_findings, 0);
    assert_eq!(
        stale.open_findings + stale.resolved_findings,
        stale.findings
    );
}
