//! Ring 44 — the v12 journey-root graph and travel hard cut.

mod common;

use common::Tmp;
use loom::cli::{Cli, Command};
use loom::model::{EdgeKind, InspectionStatus, NodeType, TruthClass};
use loom::registry::{self, OwnerRole};
use loom::store::Store;
use loom::travel::{Export, FORMAT};
use rusqlite::Connection;
use serde_json::json;
use std::str::FromStr;

fn add_node(store: &Store, node_type: NodeType, name: &str) -> loom::model::Node {
    store.add_node(node_type, name, "", "", json!({})).unwrap()
}

fn loom_json(root: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .arg("--graph")
        .arg(root)
        .args(args)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "loom {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn journey_vocabulary_and_topology_are_exact() {
    assert_eq!(loom::SCHEMA_VERSION, 13);
    assert_eq!(FORMAT, 4);
    assert_eq!(NodeType::from_str("journey").unwrap(), NodeType::Journey);
    assert!(NodeType::from_str("journey_coverage").is_err());
    assert!(NodeType::from_str("journey_invariant_point").is_err());
    assert!(EdgeKind::from_str("covers").is_err());
    assert!(EdgeKind::from_str("asserts").is_err());

    let cases = [
        (
            EdgeKind::Derives,
            NodeType::Journey,
            &[NodeType::Intent][..],
            OwnerRole::Builder,
        ),
        (
            EdgeKind::Surfaces,
            NodeType::Journey,
            &[NodeType::InterfaceSurface][..],
            OwnerRole::Builder,
        ),
        (
            EdgeKind::Proves,
            NodeType::Validation,
            &[NodeType::Journey][..],
            OwnerRole::Validator,
        ),
    ];
    for (kind, from, to, owner) in cases {
        let spec = registry::spec(kind);
        assert_eq!(spec.from, from);
        assert_eq!(spec.to, to);
        assert_eq!(spec.truth_classes, &[TruthClass::Asserted]);
        assert_eq!(spec.owner, owner);
    }
}

#[test]
fn writes_and_imports_accept_only_the_v12_endpoints() {
    let source = Tmp::new();
    let store = Store::init(source.path(), Some("v12 source"), false).unwrap();
    let journey = add_node(&store, NodeType::Journey, "operator completes checkout");
    let intent = add_node(&store, NodeType::Intent, "checkout can complete");
    let surface = add_node(&store, NodeType::InterfaceSurface, "checkout CLI");
    let validation = add_node(&store, NodeType::Validation, "checkout journey proof");
    let question = add_node(&store, NodeType::Question, "Should checkout permit retry?");
    let codefile = add_node(&store, NodeType::CodeFile, "src/checkout.rs");

    for edge in [
        store
            .add_edge(
                EdgeKind::Derives,
                &journey.id,
                &intent.id,
                TruthClass::Asserted,
            )
            .unwrap(),
        store
            .add_edge(
                EdgeKind::Surfaces,
                &journey.id,
                &surface.id,
                TruthClass::Asserted,
            )
            .unwrap(),
        store
            .add_edge(
                EdgeKind::Proves,
                &validation.id,
                &journey.id,
                TruthClass::Asserted,
            )
            .unwrap(),
        store
            .add_edge(
                EdgeKind::Questions,
                &question.id,
                &journey.id,
                TruthClass::Asserted,
            )
            .unwrap(),
        store
            .add_edge(
                EdgeKind::Questions,
                &question.id,
                &intent.id,
                TruthClass::Asserted,
            )
            .unwrap(),
    ] {
        assert_eq!(edge.status, InspectionStatus::Uninspected);
    }

    let error = store
        .add_edge(
            EdgeKind::Questions,
            &question.id,
            &codefile.id,
            TruthClass::Asserted,
        )
        .unwrap_err();
    assert!(error.to_string().contains("journey|intent"), "{error}");

    let snapshot = store.snapshot().unwrap();
    drop(store);
    let destination = Tmp::new();
    let mut restored = Store::init(destination.path(), Some("destination"), false).unwrap();
    restored.restore(&snapshot).unwrap();
    assert_eq!(restored.list_edges(None, usize::MAX).unwrap().len(), 5);
}

#[test]
fn persisted_v1_through_v11_graphs_are_refused_without_stamp_mutation() {
    for (user_version_stamp, meta_version_stamp, reported_version) in
        [(1u32, 1u32, 1u32), (11, 11, 11), (0, 11, 11)]
    {
        let tmp = Tmp::new();
        drop(Store::init(tmp.path(), Some("old"), false).unwrap());
        let db = tmp.path().join(loom::LOOM_DIR).join(loom::GRAPH_DB);
        let conn = Connection::open(&db).unwrap();
        conn.pragma_update(None, "user_version", user_version_stamp)
            .unwrap();
        conn.execute(
            "UPDATE meta SET value=?1 WHERE key='schema_version'",
            [meta_version_stamp.to_string()],
        )
        .unwrap();
        drop(conn);

        for error in [
            Store::open_read(tmp.path()).err().unwrap(),
            Store::open(tmp.path()).err().unwrap(),
            Store::init(tmp.path(), None, false).err().unwrap(),
        ] {
            let message = error.to_string();
            assert!(
                message.contains(&format!("graph is v{reported_version}")),
                "{message}"
            );
            assert!(message.contains("journey paradigm"), "{message}");
            assert!(message.contains("re-init and rebuild"), "{message}");
        }

        let conn = Connection::open(&db).unwrap();
        let user_version: u32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let meta_version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(user_version, user_version_stamp);
        assert_eq!(meta_version, meta_version_stamp.to_string());
    }
}

#[test]
fn old_export_format_or_schema_is_rejected_before_destination_creation() {
    for (name, envelope) in [
        (
            "format3.json",
            r#"{"format":3,"schema_version":12,"graph_id":"g","name":"old","observed":false,"nodes":[],"edges":[],"facets":[],"tags":[]}"#,
        ),
        (
            "schema11.json",
            r#"{"format":4,"schema_version":11,"graph_id":"g","name":"old","observed":false,"nodes":[],"edges":[],"facets":[],"tags":[]}"#,
        ),
    ] {
        let tmp = Tmp::new();
        tmp.write(name, envelope);
        let error = loom::commands::run(Cli {
            graph: Some(tmp.path().to_path_buf()),
            json: false,
            command: Some(Command::Import {
                file: tmp.path().join(name),
                repair_orphans: false,
            }),
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("unsupported"),
            "unexpected refusal: {error}"
        );
        assert!(!tmp
            .path()
            .join(loom::LOOM_DIR)
            .join(loom::GRAPH_DB)
            .exists());
    }
}

#[test]
fn format_four_exports_and_status_expose_schema_and_journeys() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("current"), false).unwrap();
    add_node(&store, NodeType::Journey, "operator completes checkout");
    let export = Export::from_snapshot(store.snapshot().unwrap());
    assert_eq!(export.format, 4);
    assert_eq!(export.schema_version, 13);
    assert_eq!(export.into_snapshot().identity.schema_version, 13);
    drop(store);

    let status = loom_json(tmp.path(), &["status"]);
    assert_eq!(status["graph"]["schema_version"], 13);
    assert_eq!(status["counts"]["journeys"], 1);

    let schema = loom_json(tmp.path(), &["schema"]);
    assert!(schema["node_types"]
        .as_array()
        .unwrap()
        .iter()
        .any(|kind| kind == "journey"));
    let questions = schema["edge_kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|edge| edge["kind"] == "questions")
        .unwrap();
    assert_eq!(questions["to"], json!(["journey", "intent"]));
}
