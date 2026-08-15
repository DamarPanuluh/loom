//! Ring 61 — two silent-failure repairs the 2026-08-15 triage confirmed.
//!
//! Both defects were green before: doctor reported a clean graph it could not
//! actually check, and the shared-proof detector dropped a collision it was
//! built to find. Each test fails on the pre-repair code.

mod common;
use common::*;
use loom::model::{EdgeKind, NodeType, TruthClass};
use loom::store::Store;

#[test]
fn an_unreadable_upstream_registry_is_reported_instead_of_read_as_clean() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    assert!(
        loom::signal::doctor(&store).unwrap().is_empty(),
        "a fresh graph must be clean, so the issue below is the corruption's"
    );

    // Corrupt the linked-upstream registry: present, but not the JSON the
    // reader expects. read_upstream_entries returns Err for exactly this.
    store.set_meta("upstream_graphs", "{not json").unwrap();
    assert!(
        loom::federation::read_upstream_entries(&store).is_err(),
        "fixture must actually break the read"
    );

    let issues = loom::signal::doctor(&store).unwrap();
    let hit = issues
        .iter()
        .find(|i| i.kind == "unreadable_upstream_registry")
        .unwrap_or_else(|| {
            panic!("doctor must not report a graph clean when it could not check it: {issues:#?}")
        });
    assert!(
        hit.message
            .contains("orphaned upstream intents cannot be checked"),
        "the issue must say which check was skipped: {}",
        hit.message
    );
}

#[test]
fn one_proof_validating_two_behaviors_still_raises_a_shared_command_smell() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let mk = |name: &str| {
        store
            .add_node(
                NodeType::Intent,
                name,
                "",
                "implemented",
                serde_json::json!({ "level": "feature" }),
            )
            .unwrap()
    };
    let first = mk("first behavior");
    let second = mk("second behavior");

    // ONE validation, wired to BOTH behaviors — the case a single-valued map
    // collapses to whichever Validates edge happened to land last.
    let proof = store
        .add_node(
            NodeType::Validation,
            "one suite for two claims",
            "",
            "passed",
            serde_json::json!({ "type": "test", "command": "cargo test --test ring_shared" }),
        )
        .unwrap();
    for target in [&first, &second] {
        store
            .add_edge(
                EdgeKind::Validates,
                &proof.id,
                &target.id,
                TruthClass::Asserted,
            )
            .unwrap();
    }

    let smells = loom::signal::smells(&store).unwrap();
    let shared: Vec<_> = smells
        .iter()
        .filter(|s| s.kind == "shared_proof_command")
        .collect();
    assert_eq!(
        shared.len(),
        1,
        "one command proving two behaviors is a collision, whichever edge was written last: {smells:#?}"
    );
    assert!(
        shared[0].message.contains("2 behaviors"),
        "the smell must count both behaviors: {}",
        shared[0].message
    );
}
