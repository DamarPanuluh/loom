//! Ring 51 — Loom-issued source anchors are strict navigation identities.
//!
//! A marker is not graph truth or proof. Once a graph locator references it,
//! Loom resolves exactly one occurrence attached to one smallest supported
//! entry and fails closed on every ambiguity.

use loom::model::{EdgeKind, NodeType, TruthClass};
use loom::store::Store;
mod common;
use common::Tmp;

fn register(store: &Store, path: &str, content: &str) -> loom::model::Node {
    let full = store.root().join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(full, content).unwrap();
    store
        .add_node(NodeType::CodeFile, path, "", "", serde_json::json!({}))
        .unwrap()
}

#[test]
fn codefile_anchor_issues_exact_marker_without_mutating_source_or_graph() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("source anchors"), false).unwrap();
    register(&store, "src/cli/cart.rs", "pub fn pay() -> bool { true }\n");
    let source_before = std::fs::read_to_string(tmp.path().join("src/cli/cart.rs")).unwrap();
    let nodes_before = store.list_nodes(None, usize::MAX).unwrap().len();
    let edges_before = store.edges_with(None, None, None).unwrap().len();
    let journal_before = loom::journal::read(tmp.path()).unwrap().len();
    drop(store);

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .args(["--graph"])
        .arg(tmp.path())
        .args([
            "codefile",
            "anchor",
            "src/cli/cart.rs",
            "--at-line",
            "1",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "issuance failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["anchor_id"], "cli.cart.pay");
    assert_eq!(json["locator"], "anchor:cli.cart.pay");
    assert_eq!(json["marker"], "// loom:anchor cli.cart.pay");
    assert_eq!(json["attached_entry"]["name"], "pay");

    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("src/cli/cart.rs")).unwrap(),
        source_before,
        "issuance must not insert the marker"
    );
    assert_eq!(
        store.list_nodes(None, usize::MAX).unwrap().len(),
        nodes_before
    );
    assert_eq!(
        store.edges_with(None, None, None).unwrap().len(),
        edges_before
    );
    assert_eq!(
        loom::journal::read(tmp.path()).unwrap().len(),
        journal_before
    );
    assert!(
        !tmp.path().join(loom::GRAPH_EXPORT).exists(),
        "read-only issuance must not create an export"
    );
}

#[test]
fn issuance_is_idempotent_and_collision_safe() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("source anchors"), false).unwrap();
    let first = register(
        &store,
        "src/cli/cart.rs",
        "// loom:anchor cli.cart.pay\npub fn pay() {}\n",
    );
    let second = register(&store, "src/cli.cart.rs", "pub fn pay() {}\n");

    let existing = loom::locator::issue_anchor(&store, &first, 2).unwrap();
    assert_eq!(existing.id, "cli.cart.pay");
    assert_eq!(existing.marker, "// loom:anchor cli.cart.pay");

    let collision = loom::locator::issue_anchor(&store, &second, 1).unwrap();
    assert!(collision.id.starts_with("cli.cart.pay."), "{collision:?}");
    assert_ne!(collision.id, existing.id);
    assert_eq!(
        loom::locator::issue_anchor(&store, &second, 1).unwrap().id,
        collision.id,
        "the collision suffix must be deterministic"
    );
}

#[test]
fn hash_comment_configs_work_but_commentless_json_fails_closed() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("source anchors"), false).unwrap();
    let toml = register(&store, "config/settings.toml", "port = 9000\n");
    let issued = loom::locator::issue_anchor(&store, &toml, 1).unwrap();
    assert_eq!(issued.marker, "# loom:anchor config.settings.port");
    assert_eq!(issued.entry_kind, "config_entry");
    assert_eq!(issued.entry_name, "port");

    let json = register(&store, "config/settings.json", "{\"port\":9000}\n");
    let error = loom::locator::issue_anchor(&store, &json, 1)
        .unwrap_err()
        .to_string();
    assert!(error.contains("unsupported"), "{error}");
}

#[test]
fn rename_keeps_navigation_identity_and_impact_uses_the_current_symbol() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("source anchors"), false).unwrap();
    let file = register(
        &store,
        "src/pay.rs",
        "// loom:anchor checkout.pay\npub fn renamed_payment() -> bool { true }\n",
    );
    let intent = store
        .add_node(
            NodeType::Intent,
            "payment completes",
            "payment returns a result",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let grounding = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &grounding.id,
            loom::model::TargetKind::Edge,
            "locator",
            "anchor:checkout.pay",
            TruthClass::Asserted,
        )
        .unwrap();

    let resolved = loom::locator::resolve_anchor(&store, "anchor:checkout.pay").unwrap();
    assert_eq!(resolved.entry_name, "renamed_payment");
    assert_eq!(resolved.callable_symbol.as_deref(), Some("renamed_payment"));
    assert_eq!(
        loom::locator::realizing_navigation_symbols(&store, &intent.id).unwrap(),
        ["renamed_payment"]
    );
    assert!(
        loom::locator::realizing_symbols(&store, &intent.id)
            .unwrap()
            .is_empty(),
        "proof-facing symbols must exclude anchors"
    );
    drop(store);

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .args(["--graph"])
        .arg(tmp.path())
        .args(["impact", "anchor:checkout.pay", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "impact failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["anchor"]["entry"]["name"], "renamed_payment");
    assert_eq!(json["anchor"]["codefile"], "src/pay.rs");
    assert!(json["intents_at_risk"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| { item["intent"] == "payment completes" }));
}

#[test]
fn missing_duplicate_detached_and_wrong_file_anchors_fail_closed() {
    let missing_tmp = Tmp::new();
    let missing_store = Store::init(missing_tmp.path(), Some("missing"), false).unwrap();
    let missing_file = register(&missing_store, "src/missing.rs", "pub fn live() {}\n");
    let missing =
        loom::locator::validate_for_codefile(&missing_store, &missing_file, "anchor:missing.entry")
            .unwrap_err()
            .to_string();
    assert!(missing.contains("missing"), "{missing}");

    let duplicate_tmp = Tmp::new();
    let duplicate_store = Store::init(duplicate_tmp.path(), Some("duplicate"), false).unwrap();
    let first = register(
        &duplicate_store,
        "src/a.rs",
        "// loom:anchor duplicate.entry\npub fn a() {}\n",
    );
    register(
        &duplicate_store,
        "src/b.rs",
        "// loom:anchor duplicate.entry\npub fn b() {}\n",
    );
    let duplicate =
        loom::locator::validate_for_codefile(&duplicate_store, &first, "anchor:duplicate.entry")
            .unwrap_err()
            .to_string();
    assert!(duplicate.contains("duplicated"), "{duplicate}");

    let detached_tmp = Tmp::new();
    let detached_store = Store::init(detached_tmp.path(), Some("detached"), false).unwrap();
    let detached_file = register(
        &detached_store,
        "src/detached.rs",
        "// loom:anchor detached.entry\n\npub fn live() {}\n",
    );
    let detached = loom::locator::validate_for_codefile(
        &detached_store,
        &detached_file,
        "anchor:detached.entry",
    )
    .unwrap_err()
    .to_string();
    assert!(detached.contains("detached"), "{detached}");

    let wrong_tmp = Tmp::new();
    let wrong_store = Store::init(wrong_tmp.path(), Some("wrong"), false).unwrap();
    register(
        &wrong_store,
        "src/right.rs",
        "// loom:anchor wrong.file\npub fn live() {}\n",
    );
    let wrong_file = register(&wrong_store, "src/wrong.rs", "pub fn other() {}\n");
    let wrong =
        loom::locator::validate_for_codefile(&wrong_store, &wrong_file, "anchor:wrong.file")
            .unwrap_err()
            .to_string();
    assert!(wrong.contains("not target CodeFile"), "{wrong}");
}

#[test]
fn anchors_never_become_locator_run_evidence() {
    let tmp = Tmp::new();
    std::fs::write(
        tmp.path().join("entry.rs"),
        "// loom:anchor proof.entry\npub fn entry() {}\n",
    )
    .unwrap();
    assert!(loom::locator::symbols("anchor:proof.entry").is_empty());
    assert!(
        loom::runner::resolve_locator(tmp.path(), "entry.rs", Some("anchor:proof.entry")).is_none()
    );
    assert!(
        loom::runner::locator_probe(tmp.path(), "entry.rs", Some("anchor:proof.entry")).is_none()
    );
    assert!(loom::runner::seam_probe(tmp.path(), "entry.rs", Some("anchor:proof.entry")).is_none());
}
