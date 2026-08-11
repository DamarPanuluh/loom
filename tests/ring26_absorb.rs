//! Ring 26 — absorb, and the stamp that makes it safe.
//!
//! Absorb exists to collapse friction: an agent that edits code already knows
//! what it did, and making it re-describe that in loom's vocabulary is the tax
//! that gets loom skipped. But a tool that writes the graph FROM the work is
//! only safe if what it observed can be re-checked, because the proposal body
//! is ordinary JSON the agent can rewrite.

use loom::absorb;
use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::store::Store;
mod common;
use common::*;

fn seed(store: &Store, root: &std::path::Path) -> String {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/checkout.rs"),
        "pub fn perform_checkout() {}\n",
    )
    .unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "checkout completes",
            "a behavior",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = store
        .add_node(
            NodeType::CodeFile,
            "src/checkout.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let e = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &e.id,
            TargetKind::Edge,
            "locator",
            "fn perform_checkout",
            TruthClass::Asserted,
        )
        .unwrap();
    loom::sync::run(store, root).unwrap();
    intent.id
}

/// Observing writes nothing. This matters more than it sounds: sync's derivation
/// writes while it observes, so an absorb that reused it would destroy the very
/// change set it needs to see.
#[test]
fn observing_is_a_pure_read() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    seed(&store, tmp.path());

    let render = |s: &Store| {
        let path = loom::travel::export_to_file(s).unwrap();
        std::fs::read_to_string(path).unwrap()
    };
    let before = render(&store);
    let _ = absorb::observe(&store, tmp.path()).unwrap();
    let after = render(&store);
    assert_eq!(before, after, "observe must not mutate the graph");
}

/// A new symbol in a file a behavior already owns is proposed, not invented.
#[test]
fn a_new_symbol_in_an_owned_file_is_proposed() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = seed(&store, tmp.path());

    // The agent does what agents do: writes a function.
    std::fs::write(
        tmp.path().join("src/checkout.rs"),
        "pub fn perform_checkout() {}\npub fn apply_discount() {}\n",
    )
    .unwrap();

    let items = absorb::observe(&store, tmp.path()).unwrap();
    let found = items
        .iter()
        .find(|i| i.evidence.symbol == "apply_discount")
        .expect("the new symbol is observed");
    assert_eq!(found.kind, absorb::Kind::ExtendLocator);
    assert_eq!(found.intent_id.as_deref(), Some(intent.as_str()));
    assert!(
        found.needs.is_empty(),
        "loom can derive this one — it needs nothing from a human"
    );
}

/// A locator naming code that moved is proposed for re-pointing, and it ASKS
/// rather than guessing: loom cannot know whether the behavior moved or was
/// removed, and inventing an answer there is how a graph starts lying.
#[test]
fn a_vanished_symbol_asks_instead_of_guessing() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    seed(&store, tmp.path());

    std::fs::write(
        tmp.path().join("src/checkout.rs"),
        "pub fn something_else_entirely() {}\n",
    )
    .unwrap();

    let items = absorb::observe(&store, tmp.path()).unwrap();
    let repoint = items
        .iter()
        .find(|i| i.kind == absorb::Kind::RepointLocator)
        .expect("the vanished locator is observed");
    assert!(
        !repoint.needs.is_empty(),
        "loom must ask where the behavior went rather than deciding"
    );
}

/// THE property. A hand-edited stamp does not survive the recompute.
#[test]
fn a_rewritten_stamp_does_not_survive_adoption() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    seed(&store, tmp.path());
    std::fs::write(
        tmp.path().join("src/checkout.rs"),
        "pub fn perform_checkout() {}\npub fn apply_discount() {}\n",
    )
    .unwrap();

    let items = absorb::observe(&store, tmp.path()).unwrap();
    let real = items
        .iter()
        .find(|i| i.evidence.symbol == "apply_discount")
        .unwrap()
        .clone();
    assert!(
        absorb::still_holds(tmp.path(), &real),
        "an untouched observation holds"
    );

    // Tamper: claim loom saw something it did not.
    let mut forged = real.clone();
    forged.evidence.fingerprint = "0000000000000000".into();
    assert!(
        !absorb::still_holds(tmp.path(), &forged),
        "a rewritten stamp must not pass the recompute"
    );

    // And the same check catches honest staleness — a batch adopted after the
    // file moved on is acting on an observation that stopped being true.
    std::fs::write(tmp.path().join("src/checkout.rs"), "pub fn gone() {}\n").unwrap();
    assert!(!absorb::still_holds(tmp.path(), &real));
}

/// Absorb is idempotent: observing twice with no edits in between proposes the
/// same thing, so running it habitually costs nothing.
#[test]
fn absorbing_twice_proposes_the_same_thing() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    seed(&store, tmp.path());
    std::fs::write(
        tmp.path().join("src/checkout.rs"),
        "pub fn perform_checkout() {}\npub fn apply_discount() {}\n",
    )
    .unwrap();

    let a = absorb::observe(&store, tmp.path()).unwrap();
    let b = absorb::observe(&store, tmp.path()).unwrap();
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.evidence, y.evidence);
        assert_eq!(x.kind, y.kind);
    }
}

/// The batch lands as an ordinary Proposal, so it inherits adopt/defer/reject
/// rather than inventing a second way to apply changes.
#[test]
fn the_batch_is_an_ordinary_proposal() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    seed(&store, tmp.path());
    std::fs::write(
        tmp.path().join("src/checkout.rs"),
        "pub fn perform_checkout() {}\npub fn apply_discount() {}\n",
    )
    .unwrap();

    let items = absorb::observe(&store, tmp.path()).unwrap();
    let proposal = absorb::record(&store, &items).unwrap();
    assert_eq!(proposal.node_type, NodeType::Proposal);
    assert_eq!(
        proposal.body.get("source").and_then(|v| v.as_str()),
        Some("absorb")
    );
    let stored = proposal
        .body
        .get("items")
        .and_then(|v| v.as_array())
        .expect("items array");
    assert_eq!(stored.len(), items.len());
    assert!(
        stored.iter().all(|i| i.get("absorb_evidence").is_some()),
        "every item carries what loom saw, so adoption can re-check it"
    );
}

/// The JSON adapter returns the exact Proposal state written by absorb. The
/// legacy observation fields remain available, while the persisted view proves
/// that every proposed mutation is still open and none became graph truth.
#[test]
fn absorb_json_exposes_persisted_open_proposal_without_adopting_items() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    seed(&store, tmp.path());
    std::fs::write(
        tmp.path().join("src/checkout.rs"),
        "pub fn perform_checkout() {}\npub fn apply_discount() {}\n",
    )
    .unwrap();

    let intent_ids_before: std::collections::BTreeSet<_> = store
        .list_nodes(Some(NodeType::Intent), usize::MAX)
        .unwrap()
        .into_iter()
        .map(|node| node.id)
        .collect();
    let edge_ids_before: std::collections::BTreeSet<_> = store
        .list_edges(None, usize::MAX)
        .unwrap()
        .into_iter()
        .map(|edge| edge.id)
        .collect();
    let proposal_count_before = store.count_nodes(Some(NodeType::Proposal)).unwrap();
    drop(store);

    let output = loom_command()
        .arg("--graph")
        .arg(tmp.path())
        .args(["absorb", "--json"])
        .output()
        .expect("run loom absorb --json");
    assert!(
        output.status.success(),
        "absorb failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let proposal_id = response["proposal"]
        .as_str()
        .expect("legacy proposal id remains a string");
    let persisted = &response["persisted_proposal"];
    assert_eq!(persisted["id"], proposal_id);
    assert_eq!(persisted["type"], "proposal");
    assert_eq!(persisted["truth_class"], "asserted");
    assert_eq!(persisted["status"], "captured");
    assert_eq!(persisted["body"]["source"], "absorb");

    let observed_items = response["items"]
        .as_array()
        .expect("legacy observation items remain an array");
    let persisted_items = persisted["body"]["items"]
        .as_array()
        .expect("persisted proposal items are returned");
    assert!(!persisted_items.is_empty());
    assert_eq!(persisted_items.len(), observed_items.len());
    for (index, (persisted_item, observed_item)) in
        persisted_items.iter().zip(observed_items).enumerate()
    {
        assert_eq!(persisted_item["number"], index + 1);
        assert_eq!(persisted_item["status"], "open");
        assert_eq!(persisted_item["kind"], observed_item["kind"]);
        assert_eq!(persisted_item["text"], observed_item["text"]);
        assert_eq!(persisted_item["intent_id"], observed_item["intent_id"]);
        assert_eq!(persisted_item["absorb_evidence"], observed_item["evidence"]);
        assert_eq!(persisted_item["needs"], observed_item["needs"]);
        assert!(
            persisted_item.get("spawned").is_none(),
            "an unadopted absorb item must not name authoritative work"
        );
    }
    assert_eq!(
        response["ready"].as_u64().unwrap() + response["needs_you"].as_u64().unwrap(),
        observed_items.len() as u64
    );

    let store = Store::open(tmp.path()).unwrap();
    let intent_ids_after: std::collections::BTreeSet<_> = store
        .list_nodes(Some(NodeType::Intent), usize::MAX)
        .unwrap()
        .into_iter()
        .map(|node| node.id)
        .collect();
    let edge_ids_after: std::collections::BTreeSet<_> = store
        .list_edges(None, usize::MAX)
        .unwrap()
        .into_iter()
        .map(|edge| edge.id)
        .collect();
    assert_eq!(intent_ids_after, intent_ids_before);
    assert_eq!(edge_ids_after, edge_ids_before);
    assert_eq!(
        store.count_nodes(Some(NodeType::Proposal)).unwrap(),
        proposal_count_before + 1
    );
    let stored = store.get_node(proposal_id).unwrap().unwrap();
    assert_eq!(stored.status, "captured");
    assert_eq!(stored.body, persisted["body"]);
}

#[test]
fn absorb_json_returns_null_persisted_proposal_when_nothing_is_missing() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    seed(&store, tmp.path());
    drop(store);

    let output = loom_command()
        .arg("--graph")
        .arg(tmp.path())
        .args(["absorb", "--json"])
        .output()
        .expect("run empty loom absorb --json");
    assert!(
        output.status.success(),
        "absorb failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["proposal"], serde_json::Value::Null);
    assert_eq!(response["persisted_proposal"], serde_json::Value::Null);
    assert_eq!(response["items"], serde_json::json!([]));

    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(store.count_nodes(Some(NodeType::Proposal)).unwrap(), 0);
}

/// A new test symbol whose call closure reaches a behavior's code is a proof
/// waiting to be registered. This rule needed `tests/` in the graph to fire at
/// all — before that, no test file was a call-graph entry point, and the
/// variant sat declared and unreachable.
#[test]
fn a_test_that_exercises_a_behavior_is_proposed_as_its_proof() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = seed(&store, tmp.path());

    // Register the test file and let loom see its current contents FIRST —
    // absorb reports what is NEW since it last looked, which is the same order
    // the real workflow has: the agent writes code, then absorbs.
    std::fs::create_dir_all(tmp.path().join("tests")).unwrap();
    std::fs::write(tmp.path().join("tests/checkout_test.rs"), "// empty\n").unwrap();
    store
        .add_node(
            NodeType::CodeFile,
            "tests/checkout_test.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();

    // Now the agent writes the test.
    std::fs::write(
        tmp.path().join("tests/checkout_test.rs"),
        "pub fn checks_checkout() {\n    let _ = perform_checkout();\n}\n",
    )
    .unwrap();

    let items = absorb::observe(&store, tmp.path()).unwrap();
    let proof = items
        .iter()
        .find(|i| i.kind == absorb::Kind::RegisterProof)
        .expect("the test's reach into the behavior is observed");
    assert_eq!(proof.intent_id.as_deref(), Some(intent.as_str()));
    assert!(
        !proof.needs.is_empty(),
        "loom sees WHAT the test touches; only a person can say it CHECKS it"
    );
}
