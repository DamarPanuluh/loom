//! Regression tests for the trust gaps reported by a real coverage-lane user.

use loom::lane::Lane;
use loom::maturity;
use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use loom::workitem;

mod common;
use common::*;

fn loom_json(root: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_loom"));
    command
        .env(loom::identity::AGENT_ENV, "solo")
        .env_remove(loom::identity::PROFILE_ENV)
        .arg("--graph")
        .arg(root)
        .args(args)
        .arg("--json");
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("spawn loom {args:?}: {error}"));
    assert!(
        output.status.success(),
        "loom {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "loom {args:?} emitted invalid JSON: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

#[test]
fn coverage_packet_surfaces_precedent_and_cannot_mint_an_intent() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("coverage-trust"), false).unwrap();
    codefile(&store, "src/qualification/generated.rs");
    codefile(&store, "src/qualification/engine.rs");
    store
        .set_meta(
            "ignores",
            &serde_json::to_string(&serde_json::json!([{
                "glob": "src/qualification/generated.rs",
                "reason": "generated adapter — outside behavioral ownership"
            }]))
            .unwrap(),
        )
        .unwrap();

    let packet = workitem::next(&store, Some(Lane::Coverage))
        .unwrap()
        .expect("the remaining unowned file should be served");
    assert_eq!(packet.target.name, "src/qualification/engine.rs");
    assert!(packet
        .prompt_contract
        .allowed_actions
        .contains(&"loom ignore list".to_string()));
    assert!(packet
        .prompt_contract
        .forbidden_actions
        .iter()
        .any(|action| action == "creating a new intent in the coverage lane"));
    assert!(!packet
        .prompt_contract
        .write_back
        .contains("loom intent add"));
    let precedents = packet.prompt_contract.examples.unwrap();
    assert_eq!(
        precedents["existing_ignore_precedents"][0]["reason"],
        "generated adapter — outside behavioral ownership"
    );
    assert_eq!(
        precedents["neighboring_file_dispositions"][0]["disposition"],
        "excluded"
    );
    drop(store);

    let coverage = loom_json(tmp.path(), &["coverage"]);
    assert_eq!(coverage["codefiles"]["registered"], 2);
    assert_eq!(coverage["codefiles"]["in_scope"], 1);
    assert_eq!(coverage["codefiles"]["excluded"], 1);
    assert_eq!(
        coverage["codefiles"]["exclusions_by_reason"]
            ["generated adapter — outside behavioral ownership"],
        1
    );

    let shown = loom_json(
        tmp.path(),
        &["codefile", "show", "src/qualification/generated.rs"],
    );
    assert_eq!(shown["ignored"], true);
    assert_eq!(
        shown["ignore_rules"][0]["glob"],
        "src/qualification/generated.rs"
    );
}

#[test]
fn edge_list_filters_and_keeps_ownership_fields_in_json() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("edge-trust"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "qualification surface",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let other = store
        .add_node(
            NodeType::Intent,
            "unrelated behavior",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let surface = codefile(&store, "src/qualification.rs");
    let unrelated = codefile(&store, "src/unrelated.rs");
    let edge = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &surface.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "role",
            "consumes",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "locator",
            "route:/qualification",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Implements,
            &other.id,
            &unrelated.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let edge_id = edge.id.clone();
    drop(store);

    let listed = loom_json(
        tmp.path(),
        &["edge", "list", "--intent", "qualification surface"],
    );
    assert_eq!(listed["pagination"]["total"], 1);
    assert_eq!(listed["items"][0]["from"]["name"], "qualification surface");
    assert_eq!(listed["items"][0]["to"]["name"], "src/qualification.rs");
    assert_eq!(listed["items"][0]["role"], "consumes");
    assert_eq!(listed["items"][0]["locator"], "route:/qualification");

    let by_file = loom_json(
        tmp.path(),
        &["edge", "list", "--codefile", "src/qualification.rs"],
    );
    assert_eq!(by_file["pagination"]["total"], 1);
    assert_eq!(by_file["items"][0]["id"], edge_id);
}

#[test]
fn doctor_issues_are_a_global_integrity_gate() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("integrity-trust"), false).unwrap();
    let parent = store
        .add_node(
            NodeType::Intent,
            "parent",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let child = store
        .add_node(
            NodeType::Intent,
            "child",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Hierarchy,
            &parent.id,
            &child.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Hierarchy,
            &child.id,
            &parent.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let ladder = maturity::ladder(&store).unwrap();
    assert_eq!(ladder.phase, "audit");
    assert!(ladder
        .rungs
        .iter()
        .filter(|rung| rung.lane != Lane::Audit)
        .all(|rung| { rung.blocked && rung.blocked_by.as_deref() == Some("graph integrity") }));
    drop(store);

    let status = loom_json(tmp.path(), &["status"]);
    assert_eq!(status["integrity"]["valid"], false);
    assert!(!status["integrity"]["doctor_issues"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(status["compass"]["phase"], "audit");
}
