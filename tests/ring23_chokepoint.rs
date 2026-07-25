//! Ring 23 — there is exactly ONE door.
//!
//! The evidence spine is only as good as the number of ways around it. Before
//! this, `record_verdict` was gated and `set_facet` / `set_node_status` /
//! `record_finding_verdict` / `restore_inner` were not — so anything that did
//! not want to be gated simply used a different primitive. `restore_inner`
//! wrote raw SQL past every check, which is why `loom doctor` needed a
//! `vacuous_verdict` audit to catch afterwards what the boundary should have
//! refused outright.
//!
//! These tests are structural. The first reads loom's own source: an invariant
//! about WHERE code may live cannot be defended by a runtime assertion, only by
//! looking. The rest close the specific doors by name.

use loom::model::{Claim, EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::{Store, Subject};
mod common;
use common::*;

/// Every SQL statement that can move asserted truth, and the one file allowed
/// to contain it.
const CHOKEPOINT: &str = "src/store/facts.rs";
const GUARDED_SQL: &[&str] = &[
    "INSERT INTO fact",
    "INSERT INTO evidence",
    "UPDATE fact ",
    "UPDATE evidence ",
    "UPDATE edge SET status",
];

fn source_files() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("src is readable").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(std::env::current_dir().unwrap())
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                out.push((rel, std::fs::read_to_string(&path).expect("readable")));
            }
        }
    }
    let mut out = Vec::new();
    walk(std::path::Path::new("src"), &mut out);
    assert!(out.len() > 30, "the source scan must actually find loom");
    out
}

#[test]
fn only_the_chokepoint_may_write_asserted_truth() {
    let mut violations: Vec<String> = Vec::new();
    for (path, body) in source_files() {
        if path.ends_with("facts.rs") {
            continue;
        }
        for sql in GUARDED_SQL {
            if body.contains(sql) {
                // The migration is allowed to reset state wholesale — it runs
                // once, before any fact exists, and is reviewed as schema.
                if path.ends_with("store/mod.rs") && *sql == "UPDATE edge SET status" {
                    continue;
                }
                violations.push(format!("{path} contains `{sql}`"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "asserted truth may only be written in {CHOKEPOINT} — found another door:\n  {}",
        violations.join("\n  ")
    );
}

#[test]
fn reserved_facet_keys_cannot_be_written_as_facets() {
    // These keys BECAME facts. Leaving them writable as facets would leave the
    // authority checks on ratification and adjudication one `set_facet` call
    // away from irrelevant — which is exactly how a ratification could once be
    // created with no evidence, no presence proof, and no journal entry.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "orders can be placed",
            "a behavior",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    for key in [
        "ratification",
        "ratified_by",
        "ratified_at",
        "ratified_presence",
        "adjudication",
    ] {
        let err = store
            .set_facet(
                &intent.id,
                TargetKind::Node,
                key,
                "ratified",
                TruthClass::Asserted,
            )
            .expect_err("a reserved key must not be writable as a facet");
        assert!(
            err.to_string().contains("write boundary"),
            "the refusal must point at the boundary: {err}"
        );
    }
}

#[test]
fn a_verdict_and_its_edge_can_never_disagree() {
    // The edge's verdict fields are a VIEW over the fact. There is no column to
    // write, so "the edge says passing but the fact says otherwise" is not a
    // state the schema can represent.
    let tmp = Tmp::new();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/o.rs"), "pub fn place() {}\n").unwrap();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "orders can be placed",
            "a behavior",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "src/o.rs");
    let e = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &e.id,
            InspectionStatus::Passing,
            "place() creates the order",
            "src/o.rs:1",
            0.9,
            "llm",
        )
        .unwrap();

    let edge = store.get_edge(&e.id).unwrap().unwrap();
    let fact = store
        .fact(&Subject::Edge(e.id.clone()), Claim::Verdict)
        .unwrap()
        .expect("the verdict is a fact");
    assert_eq!(edge.status.as_str(), fact.fact.state);
    assert_eq!(edge.criterion, fact.fact.criterion);
    assert_eq!(edge.confidence, fact.fact.confidence);
    assert_eq!(edge.inspected_by, fact.fact.asserted_by);

    // The cited span is real evidence, so the verdict is anchored — not merely
    // asserted with a fluent sentence.
    assert!(
        fact.fact.verification.counts(),
        "a span citation anchors the verdict: {:?}",
        fact.fact.verification
    );
}

#[test]
fn an_import_cannot_smuggle_in_a_verified_fact() {
    // An export can CLAIM anything. Strength is recomputed against the local
    // working tree, so a fact whose anchors do not resolve here lands weak — the
    // hole that made `loom import` a door straight past the write boundary.
    let source = Tmp::new();
    std::fs::create_dir_all(source.path().join("src")).unwrap();
    std::fs::write(source.path().join("src/o.rs"), "pub fn place() {}\n").unwrap();
    let store = Store::init(source.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "orders can be placed",
            "a behavior",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "src/o.rs");
    let e = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &e.id,
            InspectionStatus::Passing,
            "place() creates the order",
            "src/o.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    let mut snap = store.snapshot().unwrap();
    // Forge the strongest possible claim.
    for f in &mut snap.facts {
        f.verification = loom::model::Verification::Verified;
    }
    drop(store);

    // Restore into a tree where the cited file does not exist.
    let elsewhere = Tmp::new();
    let mut restored = Store::init(elsewhere.path(), Some("t"), false).unwrap();
    restored.restore(&snap).unwrap();

    let fact = restored
        .fact(&Subject::Edge(e.id.clone()), Claim::Verdict)
        .unwrap()
        .expect("the fact imported");
    assert!(
        !fact.fact.verification.counts(),
        "a citation into a file this tree does not have cannot count: {:?}",
        fact.fact.verification
    );
}
