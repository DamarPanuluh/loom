//! Ring 1 invariant tests — the write-boundary integrity contract.
//!
//! Real tests against real SQLite (no mocks). INV-2 (derived rebuildable) is a
//! ring-2 invariant because it requires `sync`; it is verified there.

use loom::cli::{Cli, Command, IntentCmd};
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use loom::travel::Export;
mod common;
use common::*;

fn seed_intent(store: &Store, name: &str) -> String {
    store
        .add_node(
            NodeType::Intent,
            name,
            "a behavior",
            "planned",
            serde_json::json!({}),
        )
        .unwrap()
        .id
}

fn seed_codefile(store: &Store, path: &str) -> String {
    store
        .add_node(NodeType::CodeFile, path, "", "", serde_json::json!({}))
        .unwrap()
        .id
}

// ---- INV-6 : evidence gate -------------------------------------------------

#[test]
fn inv6_passing_requires_criterion_and_evidence() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = seed_intent(&store, "payment can be captured");
    let file = seed_codefile(&store, "src/payment.rs");
    let edge = store
        .add_edge(EdgeKind::Implements, &intent, &file, TruthClass::Asserted)
        .unwrap();

    // empty evidence rejected
    assert!(store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "criterion",
            "",
            0.9,
            "llm"
        )
        .is_err());
    // empty criterion rejected
    assert!(store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "",
            "evidence",
            0.9,
            "llm"
        )
        .is_err());
    // both present accepted
    let ok = store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "c",
            "src/payment.rs:10",
            0.9,
            "llm",
        )
        .unwrap();
    assert_eq!(ok.status, InspectionStatus::Passing);
}

// ---- INV-4 : absence is default; independent needs evidence -----------------

#[test]
fn inv4_independent_requires_evidence() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = seed_intent(&store, "user can log in");
    let b = seed_intent(&store, "user can log out");
    let edge = store
        .add_edge(EdgeKind::Relates, &a, &b, TruthClass::Asserted)
        .unwrap();
    assert!(store
        .record_verdict(&edge.id, InspectionStatus::Independent, "", "", 0.9, "llm")
        .is_err());
    let ok = store
        .record_verdict(
            &edge.id,
            InspectionStatus::Independent,
            "login and logout are independent behaviors",
            "login and logout share no state",
            0.9,
            "llm",
        )
        .unwrap();
    assert_eq!(ok.status, InspectionStatus::Independent);
}

// ---- INV-5 : class-partitioned authorship ----------------------------------

#[test]
fn inv5_verdict_path_rejects_derived_edge() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let finding = store
        .add_derived_node(
            NodeType::Finding,
            "long-file:src/pay.rs",
            "src/pay.rs is long",
            "600+ lines",
            "long_file",
            serde_json::json!({}),
        )
        .unwrap();
    let file = seed_codefile(&store, "src/pay.rs");
    // A legitimately-derived edge (Flags is sync-owned); add_edge no longer
    // creates derived edges (M-12), so derived edges come from add_derived_edge.
    let derived = store
        .add_derived_edge(EdgeKind::Flags, &finding.id, &file)
        .unwrap();
    // verdict path must refuse a derived edge
    assert!(store
        .record_verdict(
            &derived.id,
            InspectionStatus::Passing,
            "c",
            "e",
            0.9,
            "sync"
        )
        .is_err());
    // sync path must accept it
    assert!(store
        .set_derived_status(&derived.id, InspectionStatus::Current)
        .is_ok());
}

#[test]
fn inv5_derived_path_rejects_asserted_edge() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = seed_intent(&store, "payment can be captured");
    let file = seed_codefile(&store, "src/payment.rs");
    let asserted = store
        .add_edge(EdgeKind::Implements, &intent, &file, TruthClass::Asserted)
        .unwrap();
    assert!(store
        .set_derived_status(&asserted.id, InspectionStatus::Current)
        .is_err());
}

// ---- edge-kind registry typing --------------------------------------------

#[test]
fn registry_rejects_wrong_endpoint_types() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = seed_intent(&store, "intent a");
    let b = seed_intent(&store, "intent b");
    // implements requires (Intent -> CodeFile); intent->intent is illegal
    assert!(store
        .add_edge(EdgeKind::Implements, &a, &b, TruthClass::Asserted)
        .is_err());
    // governs requires (QualityRule -> Intent); intent->intent is illegal
    assert!(store
        .add_edge(EdgeKind::Governs, &a, &b, TruthClass::Asserted)
        .is_err());
    // hierarchy intent->intent is legal
    assert!(store
        .add_edge(EdgeKind::Hierarchy, &a, &b, TruthClass::Asserted)
        .is_ok());
}

#[test]
fn registry_rejects_disallowed_truth_class() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = seed_intent(&store, "intent a");
    let b = seed_intent(&store, "intent b");
    // hierarchy is asserted-only; a derived hierarchy edge is illegal
    assert!(store
        .add_edge(EdgeKind::Hierarchy, &a, &b, TruthClass::Derived)
        .is_err());
}

#[test]
fn exposes_is_asserted_only() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            "GET /x",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap()
        .id;
    let f1 = seed_codefile(&store, "src/a.rs");
    // Asserted exposes are declared by judgment; derived-`exposes` extraction is
    // not implemented (H-5/M-10), so the derived path is refused.
    assert!(store
        .add_edge(EdgeKind::Exposes, &surface, &f1, TruthClass::Asserted)
        .is_ok());
    let f2 = seed_codefile(&store, "src/b.rs");
    assert!(store
        .add_edge(EdgeKind::Exposes, &surface, &f2, TruthClass::Derived)
        .is_err());
}

// ---- import round-trip + determinism ---------------------------------------

#[test]
fn export_is_byte_deterministic_and_roundtrips() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("demo"), false).unwrap();
    let intent = seed_intent(&store, "payment can be captured");
    let file = seed_codefile(&store, "src/payment.rs");
    let edge = store
        .add_edge(EdgeKind::Implements, &intent, &file, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "c",
            "src/payment.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    store
        .set_facet(
            &intent,
            TargetKind::Node,
            "visibility",
            "user_visible",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_tag(&intent, TargetKind::Node, "payments")
        .unwrap();

    let json_a = Export::from_snapshot(store.snapshot().unwrap())
        .to_json()
        .unwrap();
    let json_b = Export::from_snapshot(store.snapshot().unwrap())
        .to_json()
        .unwrap();
    assert_eq!(json_a, json_b, "same graph must export byte-identically");

    // round-trip into a fresh store
    let tmp2 = Tmp::new();
    let mut store2 = Store::init(tmp2.path(), None, false).unwrap();
    let export = Export::from_json(&json_a).unwrap();
    store2.restore(&export.into_snapshot()).unwrap();
    let json_c = Export::from_snapshot(store2.snapshot().unwrap())
        .to_json()
        .unwrap();
    assert_eq!(
        json_a, json_c,
        "export -> import -> export must be identical"
    );
}

#[test]
fn import_refuses_nonempty_graph() {
    let tmp = Tmp::new();
    let mut store = Store::init(tmp.path(), Some("demo"), false).unwrap();
    seed_intent(&store, "some behavior");
    let snap = store.snapshot().unwrap();
    // importing into the same (non-empty) store must be refused
    assert!(store.restore(&snap).is_err());
}

#[test]
fn malformed_import_is_rejected_loudly() {
    assert!(Export::from_json("{ not valid json").is_err());
    assert!(Export::from_json(r#"{"format":1}"#).is_err()); // missing required fields
}

// ---- welcome : bare `loom` must orient, never error ------------------------

#[test]
fn welcome_tolerates_a_missing_graph() {
    // A confused human's most likely first move is running loom in a directory
    // that has no graph yet. `welcome` (and bare `loom`, which routes to it)
    // must orient them, not bail with "no loom graph — run loom init".
    let tmp = Tmp::new();
    // No init: the directory has no `.loom` store.
    loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: false,
        command: Some(Command::Welcome),
    })
    .expect("welcome must succeed with no graph");
    // The bare-`loom` path (no subcommand) resolves to the same orientation.
    loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: None,
    })
    .expect("bare loom must succeed with no graph");
}

#[test]
fn welcome_orients_on_a_real_graph() {
    let tmp = Tmp::new();
    loom::commands::run(Cli {
        graph: None,
        json: false,
        command: Some(Command::Init {
            path: Some(tmp.path().to_path_buf()),
            name: Some("t".into()),
            observed: false,
        }),
    })
    .unwrap();
    // With a graph present, welcome reads it and routes without error.
    loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: Some(Command::Welcome),
    })
    .expect("welcome must succeed on a real graph");
}

// ---- INV-ATOM : symbols are locators, not intents (CLI guard) ---------------

#[test]
fn inv_atom_rejects_symbol_named_intent() {
    let tmp = Tmp::new();
    // init via the command path so the whole flow is exercised
    loom::commands::run(Cli {
        graph: None,
        json: false,
        command: Some(Command::Init {
            path: Some(tmp.path().to_path_buf()),
            name: Some("t".into()),
            observed: false,
        }),
    })
    .unwrap();

    // symbol-looking name, no override → rejected
    let err = loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: false,
        command: Some(Command::Intent {
            cmd: IntentCmd::Add {
                name: "capture_payment".into(),
                description: "".into(),
                level: "feature".into(),
                lifecycle: "planned".into(),
                visibility: None,
                layer: None,
                aspect: None,
                allow_symbol_name: false,
            },
        }),
    });
    assert!(err.is_err(), "symbol-named intent must be rejected");

    // override without description → still rejected
    let err2 = loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: false,
        command: Some(Command::Intent {
            cmd: IntentCmd::Add {
                name: "capture_payment".into(),
                description: "".into(),
                level: "feature".into(),
                lifecycle: "planned".into(),
                visibility: None,
                layer: None,
                aspect: None,
                allow_symbol_name: true,
            },
        }),
    });
    assert!(
        err2.is_err(),
        "override without description must be rejected"
    );

    // override with behavioral description → accepted
    loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: false,
        command: Some(Command::Intent {
            cmd: IntentCmd::Add {
                name: "capture_payment".into(),
                description: "payment is captured and inventory reserved before fulfillment".into(),
                level: "feature".into(),
                lifecycle: "planned".into(),
                visibility: None,
                layer: None,
                aspect: None,
                allow_symbol_name: true,
            },
        }),
    })
    .unwrap();

    // behavioral name → accepted with no flag
    loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: false,
        command: Some(Command::Intent {
            cmd: IntentCmd::Add {
                name: "payment can be captured".into(),
                description: "".into(),
                level: "feature".into(),
                lifecycle: "planned".into(),
                visibility: None,
                layer: None,
                aspect: None,
                allow_symbol_name: false,
            },
        }),
    })
    .unwrap();
}

// ---- edge deletion: prune a redundant grounding, leave nothing orphaned -----

#[test]
fn edge_remove_prunes_asserted_edge_and_its_facets() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = seed_intent(&store, "feature a");
    let file = seed_codefile(&store, "src/a.rs");
    let edge = store
        .add_edge(EdgeKind::Implements, &intent, &file, TruthClass::Asserted)
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "locator",
            "a",
            TruthClass::Asserted,
        )
        .unwrap();

    store.delete_edge(&edge.id).unwrap();
    assert!(
        store.get_edge(&edge.id).unwrap().is_none(),
        "edge must be gone"
    );
    assert!(
        store
            .get_facet(&edge.id, TargetKind::Edge, "locator")
            .unwrap()
            .is_none(),
        "the edge's locator facet must not orphan"
    );
}

#[test]
fn edge_remove_refuses_derived_edges() {
    let tmp = Tmp::new();
    {
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        let file = seed_codefile(&store, "src/a.rs");
        let finding = store
            .add_derived_node(
                NodeType::Finding,
                "oversized_file:src/a.rs:",
                "f",
                "d",
                "oversized_file",
                serde_json::json!({}),
            )
            .unwrap();
        store
            .add_derived_edge(EdgeKind::Flags, &finding.id, &file)
            .unwrap();
    }
    let store = Store::open(tmp.path()).unwrap();
    let e = store
        .edges_with(Some(EdgeKind::Flags), None, None)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    drop(store);

    let res = loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: false,
        command: Some(Command::Edge {
            cmd: loom::cli::EdgeCmd::Remove {
                edge_id: e.id.clone(),
                reason: None,
            },
        }),
    });
    assert!(
        res.is_err(),
        "derived edges are rebuilt by sync; remove must refuse them"
    );
    let store = Store::open(tmp.path()).unwrap();
    assert!(
        store.get_edge(&e.id).unwrap().is_some(),
        "a refused remove must not delete the edge"
    );
}

// ---- node deletion: incident edges go, and their edge-scoped facets do NOT orphan

#[test]
fn delete_node_cleans_incident_edge_facets() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = seed_intent(&store, "feature a");
    let file = seed_codefile(&store, "src/a.rs");
    let edge = store
        .add_edge(EdgeKind::Implements, &intent, &file, TruthClass::Asserted)
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "locator",
            "a",
            TruthClass::Asserted,
        )
        .unwrap();

    // delete the codefile → its implements edge must go, AND the edge's locator
    // facet (keyed by edge id, no FK) must not be left orphaned.
    store.delete_node(&file).unwrap();
    assert!(store.get_node(&file).unwrap().is_none(), "node gone");
    assert!(
        store.get_edge(&edge.id).unwrap().is_none(),
        "incident edge gone"
    );
    assert!(
        store
            .get_facet(&edge.id, TargetKind::Edge, "locator")
            .unwrap()
            .is_none(),
        "the cascaded edge's locator facet must not orphan"
    );
}

#[test]
fn delete_node_refuses_derived_nodes() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let finding = store
        .add_derived_node(
            NodeType::Finding,
            "oversized_file:src/a.rs:",
            "f",
            "d",
            "oversized_file",
            serde_json::json!({}),
        )
        .unwrap();
    assert!(
        store.delete_node(&finding.id).is_err(),
        "a derived node is owned by sync; hard delete must refuse it"
    );
}

#[test]
fn set_node_body_roundtrips() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let n = store
        .add_node(
            NodeType::InterfaceSurface,
            "S",
            "",
            "",
            serde_json::json!({ "kind": "http" }),
        )
        .unwrap();
    store
        .set_node_body(
            &n.id,
            &serde_json::json!({ "kind": "sdk_method", "identity": "/x" }),
        )
        .unwrap();
    let got = store
        .get_node(&n.id)
        .expect("get_node ok")
        .expect("node exists");
    assert_eq!(got.body["kind"], "sdk_method");
    assert_eq!(got.body["identity"], "/x");
}
