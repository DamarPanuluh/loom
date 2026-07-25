//! Ring 7 tests — the dogfood-milestone contract on a controlled graph:
//! coverage, deterministic export-check, clean doctor, zero open smells,
//! meaningful maturity, and a served work item — all end to end.

use loom::model::{EdgeKind, InspectionStatus, NodeType, TruthClass};
use loom::store::Store;
use loom::{maturity, signal, travel, workitem};
mod common;
use common::*;

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

    let fa = codefile(&store, "src/auth.rs");
    let fc = codefile(&store, "src/cart.rs");
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
    // Each implemented LEAF needs a passing proof, or the validate lane has
    // real work and the graph is not clean. (`sys` is a hierarchy parent —
    // proven through its children.)
    for (intent, name) in [(&auth, "login proof"), (&cart, "cart proof")] {
        loom::commands::prove_intent(&store, &intent.id, name, "true").unwrap();
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
    assert!(met("grounded"));
    assert!(met("covered"));
    assert!(
        met("inspected") && met("measured"),
        "no asserted residue should leave the verdict rungs met"
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

/// Every command a packet offers must resolve.
///
/// The fix packet told workers to run `loom intent show <name>` where the name
/// came from the wrong endpoint: `validates` runs validation→intent, so `from`
/// is the validation, and the suggested command could never resolve. A packet
/// that hands out commands which fail is worse than one that hands out none —
/// it costs the reader a round trip to discover the tool was wrong, not them.
#[test]
fn fix_packet_names_the_intent_not_the_other_endpoint() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            loom::model::NodeType::Intent,
            "a behavior",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "src/thing.rs");
    store
        .add_edge(
            loom::model::EdgeKind::Implements,
            &intent.id,
            &cf.id,
            loom::model::TruthClass::Asserted,
        )
        .unwrap();
    // A failing PROOF: validates runs validation → intent.
    loom::commands::prove_intent(&store, &intent.id, "the proof", "false").unwrap();

    let item = loom::workitem::next(&store, Some(loom::lane::Lane::Fix))
        .unwrap()
        .expect("a failing proof routes to fix");
    let suggested: Vec<&String> = item
        .prompt_contract
        .allowed_actions
        .iter()
        .filter(|a| a.starts_with("loom intent "))
        .collect();
    assert!(!suggested.is_empty(), "the packet offers intent commands");
    for action in suggested {
        // The TARGET is the first quoted argument; prose after it may mention
        // anything (the retire line warns about inconvenient proofs).
        let target = action
            .split('\'')
            .nth(1)
            .unwrap_or_else(|| panic!("an intent command quotes its target: {action}"));
        assert_eq!(
            target, "a behavior",
            "an intent command must name the INTENT endpoint, not the validation: {action}"
        );
    }
}

/// Retiring a behavior clears the claims about it.
///
/// The fix packet sanctions `loom intent retire` when code was deliberately
/// removed — so following that advice must actually move the ladder. It did
/// not: `live_edges_by_status` treated "live" as "not superseded", so a
/// retired intent's failing proof kept gating every rung above `repaired`, and
/// the operator who did exactly what the tool asked saw nothing happen.
#[test]
fn retiring_a_behavior_stops_its_claims_counting_as_debt() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            loom::model::NodeType::Intent,
            "a behavior that will be removed",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "src/gone.rs");
    store
        .add_edge(
            loom::model::EdgeKind::Implements,
            &intent.id,
            &cf.id,
            loom::model::TruthClass::Asserted,
        )
        .unwrap();
    loom::commands::prove_intent(&store, &intent.id, "its proof", "false").unwrap();

    let before = loom::maturity::ladder(&store).unwrap();
    let repaired = |l: &loom::maturity::Ladder| {
        l.rungs
            .iter()
            .find(|r| r.name == "repaired")
            .unwrap()
            .state
    };
    assert_eq!(
        repaired(&before),
        loom::maturity::RungState::Unmet,
        "a failing proof gates fix"
    );

    store
        .retire_intent(&intent.id, "the capability was deleted on purpose", None)
        .unwrap();

    let after = loom::maturity::ladder(&store).unwrap();
    // Met or NotApplicable — either says "this no longer blocks". What must not
    // survive the retirement is Unmet.
    assert_ne!(
        repaired(&after),
        loom::maturity::RungState::Unmet,
        "a claim about a retired behavior is history, not debt"
    );
}
