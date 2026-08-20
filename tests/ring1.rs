//! Ring 1 invariant tests — the write-boundary integrity contract.
//!
//! Real tests against real SQLite (no mocks). INV-2 (derived rebuildable) is a
//! ring-2 invariant because it requires `sync`; it is verified there.

use loom::cli::{Cli, Command, IntentCmd};
use loom::journal;
use loom::model::{
    Claim, EdgeKind, Facet, InspectionStatus, NodeType, Tag, TargetKind, TruthClass,
};
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
    codefile(store, path).id
}

// ---- INV-6 : evidence gate -------------------------------------------------

#[test]
fn inv6_passing_requires_criterion_and_evidence() {
    let tmp = Tmp::new();
    // The evidence below cites this file; a citation into a file that does not
    // exist has never been evidence, it only looked like it.
    tmp.write(
        "src/payment.rs",
        "pub fn capture() {}\n// line 2\n// 3\n// 4\n// 5\n// 6\n// 7\n// 8\n// 9\n// 10\n",
    );
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
    tmp.write("src/payment.rs", "pub fn capture() {}\n");
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

    // Round-trip into a fresh store — carrying the CODE too. Verification
    // strength is recomputed against the importing tree by design: a span
    // citation into a file that tree does not have is not evidence there, so an
    // import without the code round-trips weaker on purpose. Federation reads
    // that delta as signal; a round-trip test has to supply the code to compare
    // like with like.
    let tmp2 = Tmp::new();
    tmp2.write("src/payment.rs", "pub fn capture() {}\n");
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

#[test]
fn journal_collision_fails_before_graph_restore_and_corrected_import_can_retry() {
    let source = Tmp::new();
    let source_store = Store::init(source.path(), Some("source"), false).unwrap();
    let imported_id = seed_intent(&source_store, "imported behavior");
    let mut export = Export::from_snapshot(source_store.snapshot().unwrap());
    let colliding = journal::Entry {
        id: "colliding-export-journal-id".into(),
        ts: "1000".into(),
        actor: "exporter".into(),
        profile: None,
        event: "ratification".into(),
        target_id: imported_id.clone(),
        payload: serde_json::json!({"decision": "accepted"}),
        origin: journal::Origin::Local,
    };
    export.journal.push(colliding.clone());
    let export_file = source.path().join("collision-export.json");
    std::fs::write(&export_file, export.to_json().unwrap()).unwrap();

    let destination = Tmp::new();
    journal::append(
        destination.path(),
        &loom::identity::ExecutionIdentity::solo(),
        "local authority",
        "local-target",
        serde_json::json!({}),
    )
    .unwrap();
    // Preserve the locally-minted row's authority but force the exported id so
    // import preflight encounters the collision before Store::init/restore.
    let mut local = journal::read(destination.path()).unwrap().remove(0);
    local.id.clone_from(&colliding.id);
    let journal_file = journal::path(destination.path());
    std::fs::write(
        &journal_file,
        format!("{}\n", serde_json::to_string(&local).unwrap()),
    )
    .unwrap();

    let failed = loom::commands::run(Cli {
        graph: Some(destination.path().to_path_buf()),
        json: false,
        command: Some(Command::Import {
            file: export_file.clone(),
            repair_orphans: false,
        }),
    })
    .unwrap_err();
    assert!(failed.to_string().contains("existing provenance is local"));
    assert!(
        !destination.path().join(".loom/graph.sqlite").exists(),
        "journal validation must fail before Store::init creates graph state"
    );

    export.journal.clear();
    std::fs::write(&export_file, export.to_json().unwrap()).unwrap();
    loom::commands::run(Cli {
        graph: Some(destination.path().to_path_buf()),
        json: false,
        command: Some(Command::Import {
            file: export_file,
            repair_orphans: false,
        }),
    })
    .expect("corrected import can retry after preflight rejection");
    let restored = Store::open(destination.path()).unwrap();
    assert!(restored.get_node(&imported_id).unwrap().is_some());
    assert_eq!(restored.list_nodes(None, 100).unwrap().len(), 1);
}

// ---- import soft-ref + repair -------------------------------------------

#[test]
fn restore_preserves_soft_ref_adjudication_by_default() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("demo"), false).unwrap();
    seed_intent(&store, "some behavior");

    let mut snap = store.snapshot().unwrap();
    // Push an asserted adjudication facet whose target_id is absent from the
    // snapshot's nodes. This is a soft ref: the Finding re-materializes on the
    // next sync. Strict restore must accept it without error.
    snap.facets.push(Facet {
        target_id: "d0006b0dbff045baf".into(),
        target_kind: TargetKind::Node,
        key: "adjudication".into(),
        value: r#"{"verdict":"justified","reason":"x","hash":"h","at":"2026-01-01T00:00:00Z"}"#
            .into(),
        truth_class: TruthClass::Asserted,
    });

    let tmp2 = Tmp::new();
    let mut store2 = Store::init(tmp2.path(), None, false).unwrap();
    store2.restore(&snap).unwrap(); // must not error

    // The soft-ref facet must be present in the imported store.
    let val = store2
        .get_facet("d0006b0dbff045baf", TargetKind::Node, "adjudication")
        .unwrap();
    assert!(
        val.is_some(),
        "soft-ref adjudication facet must survive strict restore"
    );

    // export -> import -> export: soft ref must survive byte-identical.
    let json_a = Export::from_snapshot(store2.snapshot().unwrap())
        .to_json()
        .unwrap();
    let tmp3 = Tmp::new();
    let mut store3 = Store::init(tmp3.path(), None, false).unwrap();
    store3
        .restore(&Export::from_json(&json_a).unwrap().into_snapshot())
        .unwrap();
    // Direct state check: soft ref must be present in store3's graph, not just
    // reflected in bytes. This rules out export silently omitting it from both
    // sides, which would yield equal-but-empty bytes and mask the bug.
    assert!(
        store3
            .get_facet("d0006b0dbff045baf", TargetKind::Node, "adjudication")
            .unwrap()
            .is_some(),
        "soft-ref adjudication facet must be present in store3 after second restore",
    );
    let json_b = Export::from_snapshot(store3.snapshot().unwrap())
        .to_json()
        .unwrap();
    assert_eq!(
        json_a, json_b,
        "soft ref must survive export -> import -> export unchanged",
    );
}

#[test]
fn restore_rejects_genuine_orphan_facet() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("demo"), false).unwrap();
    seed_intent(&store, "some behavior");

    let mut snap = store.snapshot().unwrap();
    // A non-adjudication facet on an absent target is a true orphan.
    snap.facets.push(Facet {
        target_id: "d0006b0dbff045baf".into(),
        target_kind: TargetKind::Node,
        key: "visibility".into(),
        value: "internal".into(),
        truth_class: TruthClass::Asserted,
    });

    let tmp2 = Tmp::new();
    let mut store2 = Store::init(tmp2.path(), None, false).unwrap();
    let err = store2.restore(&snap).unwrap_err();
    assert!(
        err.to_string().contains("repair-orphans"),
        "error must mention --repair-orphans; got: {err}",
    );
}

#[test]
fn restore_rejects_orphan_tag() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("demo"), false).unwrap();
    seed_intent(&store, "some behavior");

    let mut snap = store.snapshot().unwrap();
    // A tag whose target_id is absent from the snapshot's nodes/edges is an orphan.
    snap.tags.push(Tag {
        target_id: "beadfeedbeadfeed".into(),
        target_kind: TargetKind::Node,
        term: "orphan".into(),
    });

    let tmp2 = Tmp::new();
    let mut store2 = Store::init(tmp2.path(), None, false).unwrap();
    let err = store2.restore(&snap).unwrap_err();
    assert!(
        err.to_string().contains("repair-orphans"),
        "error must mention --repair-orphans; got: {err}",
    );
}

#[test]
fn restore_repairing_drops_orphans_keeps_soft_refs() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("demo"), false).unwrap();
    seed_intent(&store, "some behavior");

    let mut snap = store.snapshot().unwrap();
    // Soft-ref adjudication — must be kept, counted in preserved_soft_refs.
    snap.facets.push(Facet {
        target_id: "d0006b0dbff045baf".into(),
        target_kind: TargetKind::Node,
        key: "adjudication".into(),
        value: r#"{"verdict":"justified","reason":"x","hash":"h","at":"2026-01-01T00:00:00Z"}"#
            .into(),
        truth_class: TruthClass::Asserted,
    });
    // Genuine orphan facet — must be dropped and reported.
    snap.facets.push(Facet {
        target_id: "d0006b0dbff045baf".into(),
        target_kind: TargetKind::Node,
        key: "visibility".into(),
        value: "internal".into(),
        truth_class: TruthClass::Asserted,
    });
    // Orphan tag — must be dropped and reported.
    snap.tags.push(Tag {
        target_id: "beadfeedbeadfeed".into(),
        target_kind: TargetKind::Node,
        term: "orphan".into(),
    });

    let tmp2 = Tmp::new();
    let mut store2 = Store::init(tmp2.path(), None, false).unwrap();
    let report = store2.restore_repairing(&snap).unwrap();

    assert_eq!(
        report.preserved_soft_refs, 1,
        "one soft-ref adjudication must be preserved"
    );
    assert_eq!(
        report.dropped_facets.len(),
        1,
        "one genuine orphan facet must be dropped"
    );
    assert_eq!(
        report.dropped_facets[0],
        (
            "node".to_string(),
            "d0006b0dbff045baf".to_string(),
            "visibility".to_string(),
        ),
        "dropped_facets entry must carry (target_kind, target_id, key)",
    );
    assert_eq!(
        report.dropped_tags.len(),
        1,
        "one orphan tag must be dropped"
    );
    assert_eq!(
        report.dropped_tags[0],
        (
            "node".to_string(),
            "beadfeedbeadfeed".to_string(),
            "orphan".to_string(),
        ),
        "dropped_tags entry must carry (target_kind, target_id, term)",
    );

    // Verify the repaired graph: soft ref present, orphan facet/tag absent.
    let repaired = store2.snapshot().unwrap();
    assert!(
        repaired
            .facets
            .iter()
            .any(|f| f.target_id == "d0006b0dbff045baf" && f.key == "adjudication"),
        "soft-ref adjudication must be in the repaired store",
    );
    assert!(
        !repaired
            .facets
            .iter()
            .any(|f| f.target_id == "d0006b0dbff045baf" && f.key == "visibility"),
        "orphan facet must NOT be in the repaired store",
    );
    assert!(
        !repaired
            .tags
            .iter()
            .any(|t| t.target_id == "beadfeedbeadfeed" && t.term == "orphan"),
        "orphan tag must NOT be in the repaired store",
    );
}

#[test]
fn restore_repairing_still_refuses_nonempty_graph() {
    let tmp = Tmp::new();
    let mut store = Store::init(tmp.path(), Some("demo"), false).unwrap();
    seed_intent(&store, "some behavior");
    let snap = store.snapshot().unwrap();
    // The non-empty guard must not be bypassed by the repair path.
    assert!(store.restore_repairing(&snap).is_err());
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
    store
        .record_verdict(
            &edge.id,
            loom::model::InspectionStatus::Passing,
            "this edge owns the implementation",
            "src/a.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    let fact_id = store
        .fact(&loom::store::Subject::Edge(edge.id.clone()), Claim::Verdict)
        .unwrap()
        .unwrap()
        .fact
        .id;
    assert!(!store.evidence_for(&fact_id).unwrap().is_empty());

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
    assert!(
        store
            .fact(&loom::store::Subject::Edge(edge.id), Claim::Verdict)
            .unwrap()
            .is_none(),
        "the deleted edge's fact and cascading evidence must be gone"
    );
    assert!(store.evidence_for(&fact_id).unwrap().is_empty());
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
        .ratify_intent(&intent, "approved for deletion test", "tty")
        .unwrap();
    let node_fact = store
        .fact(
            &loom::store::Subject::Node(intent.clone()),
            Claim::Ratification,
        )
        .unwrap()
        .unwrap()
        .fact
        .id;
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

    store.delete_node(&intent).unwrap();
    assert!(
        store
            .fact(&loom::store::Subject::Node(intent), Claim::Ratification,)
            .unwrap()
            .is_none(),
        "a hard-deleted node's fact must not survive"
    );
    assert!(store.evidence_for(&node_fact).unwrap().is_empty());
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

/// A graph from the future is refused with a sentence about what to do, on
/// BOTH the read and the write path.
///
/// The migrator's own message — "migration number that is too high" — is
/// accurate about its internals and useless to the person holding an old
/// binary. Worse, the read path used to tell them to run a write command,
/// which is an instruction that cannot succeed: migrations only move forward.
#[test]
fn a_graph_from_the_future_says_upgrade_rather_than_migrate() {
    let tmp = Tmp::new();
    {
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        drop(store);
    }
    // Unknown writer + future schema: the honest instruction is to upgrade.
    // Wipe the writer breadcrumb so this case stays distinct from a same-crate
    // schema fork (see `a_same_crate_schema_fork_does_not_say_upgrade`).
    let conn = rusqlite::Connection::open(tmp.path().join(".loom/graph.sqlite")).unwrap();
    conn.pragma_update(None, "user_version", loom::SCHEMA_VERSION + 10)
        .unwrap();
    conn.execute(
        "DELETE FROM meta WHERE key IN (?1, ?2)",
        rusqlite::params![loom::WRITER_VERSION_KEY, loom::WRITER_SCHEMA_KEY],
    )
    .unwrap();
    drop(conn);

    for open in [
        Store::open as fn(&std::path::Path) -> loom::Result<Store>,
        Store::open_read as fn(&std::path::Path) -> loom::Result<Store>,
    ] {
        let msg = match open(tmp.path()) {
            Ok(_) => panic!("a future graph must be refused"),
            Err(e) => format!("{e}"),
        };
        assert!(
            msg.contains("newer loom") && msg.contains("upgrade"),
            "the refusal must name the fix: {msg}"
        );
        assert!(
            !msg.contains("too high"),
            "never surface the migrator's internal phrasing: {msg}"
        );
        assert!(
            msg.contains("no downgrade"),
            "the refusal must say the graph cannot be rolled back: {msg}"
        );
    }
}

#[test]
fn a_same_crate_schema_fork_does_not_say_upgrade() {
    let tmp = Tmp::new();
    {
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        drop(store);
    }
    let conn = rusqlite::Connection::open(tmp.path().join(".loom/graph.sqlite")).unwrap();
    conn.pragma_update(None, "user_version", loom::SCHEMA_VERSION + 1)
        .unwrap();
    conn.execute(
        "INSERT INTO meta(key,value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=?2",
        rusqlite::params![
            loom::WRITER_SCHEMA_KEY,
            (loom::SCHEMA_VERSION + 1).to_string()
        ],
    )
    .unwrap();
    drop(conn);

    for open in [
        Store::open as fn(&std::path::Path) -> loom::Result<Store>,
        Store::open_read as fn(&std::path::Path) -> loom::Result<Store>,
    ] {
        let msg = match open(tmp.path()) {
            Ok(_) => panic!("a same-crate schema fork must be refused"),
            Err(e) => format!("{e}"),
        };
        assert!(
            msg.contains("same-version") && msg.contains("will not help"),
            "the refusal must name the fork, not an upgrade: {msg}"
        );
        assert!(
            !msg.contains("upgrade this binary"),
            "reinstalling the same crate cannot open this graph: {msg}"
        );
    }
}
