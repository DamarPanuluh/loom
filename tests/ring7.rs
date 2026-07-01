//! Ring 7 tests — the dogfood-milestone contract on a controlled graph:
//! coverage, deterministic export-check, clean doctor, zero open smells,
//! meaningful maturity, and a served work item — all end to end.

use loom::model::{EdgeKind, InspectionStatus, NodeType, TruthClass};
use loom::store::Store;
use loom::{maturity, signal, travel, workitem};
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
        let p = std::env::temp_dir().join(format!("loom-ring7-{}-{nanos}-{n}", std::process::id()));
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

/// Build a small, clean, fully-grounded graph (the dogfood shape in miniature).
fn build_clean_graph(tmp: &Tmp) -> Store {
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/auth.rs"),
        "pub fn login() { /* ok */ }\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("src/cart.rs"),
        "pub fn create() { /* ok */ }\n",
    )
    .unwrap();
    let store = Store::init(tmp.path(), Some("demo"), false).unwrap();

    let sys = store
        .add_node(
            NodeType::Intent,
            "demo app works end to end",
            "purpose",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let auth = store
        .add_node(
            NodeType::Intent,
            "user can log in",
            "session on valid creds",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cart = store
        .add_node(
            NodeType::Intent,
            "cart can be created",
            "an empty cart is created",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_edge(EdgeKind::Hierarchy, &sys.id, &auth.id, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Hierarchy, &sys.id, &cart.id, TruthClass::Asserted)
        .unwrap();

    let fa = store
        .add_node(
            NodeType::CodeFile,
            "src/auth.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let fc = store
        .add_node(
            NodeType::CodeFile,
            "src/cart.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    // ground each file to exactly one intent (no overlap, no tangle)
    let ea = store
        .add_edge(EdgeKind::Implements, &auth.id, &fa.id, TruthClass::Asserted)
        .unwrap();
    let ec = store
        .add_edge(EdgeKind::Implements, &cart.id, &fc.id, TruthClass::Asserted)
        .unwrap();
    // inspect the groundings so there is no asserted residue
    store
        .record_verdict(
            &ea.id,
            InspectionStatus::Passing,
            "login implemented",
            "src/auth.rs:1",
            0.95,
            "llm",
        )
        .unwrap();
    store
        .record_verdict(
            &ec.id,
            InspectionStatus::Passing,
            "cart create implemented",
            "src/cart.rs:1",
            0.95,
            "llm",
        )
        .unwrap();
    // inspect the hierarchy edges too
    for e in store
        .edges_with(Some(EdgeKind::Hierarchy), Some(&sys.id), None)
        .unwrap()
    {
        store
            .record_verdict(
                &e.id,
                InspectionStatus::Passing,
                "decomposition holds",
                "hierarchy",
                0.95,
                "llm",
            )
            .unwrap();
    }
    // arm duplicate detection with distinct vocab tags (no collisions)
    for (id, term) in [(&sys.id, "system"), (&auth.id, "auth"), (&cart.id, "cart")] {
        store.add_vocab_term(term, "demo plane").unwrap();
        store
            .set_tag(id, loom::model::TargetKind::Node, term)
            .unwrap();
    }
    loom::sync::run(&store, tmp.path()).unwrap();
    store
}

#[test]
fn dogfood_graph_is_clean_and_exportable() {
    let tmp = Tmp::new();
    let store = build_clean_graph(&tmp);

    // doctor: clean
    assert!(
        signal::doctor(&store).unwrap().is_empty(),
        "doctor must be clean"
    );

    // smells: zero open (no tangle/overlap; small grounded graph)
    let smells = signal::smells(&store).unwrap();
    assert!(
        smells.is_empty(),
        "expected zero open smells, got: {smells:?}"
    );

    // export is deterministic and, once written, fresh
    let path = travel::export_to_file(&store).unwrap();
    assert!(path.exists());
    assert!(
        travel::export_is_fresh(&store).unwrap(),
        "export must be fresh right after writing"
    );
}

#[test]
fn dogfood_maturity_is_meaningful_not_seed() {
    let tmp = Tmp::new();
    let store = build_clean_graph(&tmp);
    let ladder = maturity::ladder(&store).unwrap();
    assert_ne!(
        ladder.phase, "seed",
        "a populated graph must not be in the seed phase"
    );
    // seeded + realized + hardened all met on a clean grounded graph
    let met = |name: &str| {
        ladder
            .rungs
            .iter()
            .find(|r| r.name == name)
            .map(|r| r.state == maturity::RungState::Met)
            .unwrap_or(false)
    };
    assert!(met("seeded"));
    assert!(met("realized"));
    assert!(
        met("hardened"),
        "no asserted residue should leave hardened met"
    );
}

#[test]
fn dogfood_export_check_detects_drift() {
    let tmp = Tmp::new();
    let store = build_clean_graph(&tmp);
    travel::export_to_file(&store).unwrap();
    assert!(travel::export_is_fresh(&store).unwrap());
    // mutate the graph → committed export is now stale
    store
        .add_node(
            NodeType::Intent,
            "new behavior",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    assert!(
        !travel::export_is_fresh(&store).unwrap(),
        "export --check must detect drift"
    );
}

#[test]
fn dogfood_next_serves_work_until_clean() {
    let tmp = Tmp::new();
    let store = build_clean_graph(&tmp);
    // clean grounded graph: no required residue
    assert!(
        workitem::next(&store, None).unwrap().is_none(),
        "clean graph has no required work"
    );
    // introduce a planned intent → build work appears
    store
        .add_node(
            NodeType::Intent,
            "checkout works",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let item = workitem::next(&store, None).unwrap().unwrap();
    assert_eq!(item.mode, "build");
}
