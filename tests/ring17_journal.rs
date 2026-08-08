//! Ring 17 — append-only evidence journal and journal citations.

use loom::{evidence, journal};
mod common;
use common::*;

#[test]
fn journal_appends_and_reads_entries_without_a_mutation_surface() {
    let tmp = Tmp::new();
    let identity = loom::identity::ExecutionIdentity::solo();
    let first = journal::append(
        tmp.path(),
        &identity,
        "validation_verdict",
        "validation-1",
        serde_json::json!({ "outcome": "passed" }),
    )
    .unwrap();
    let second = journal::append(
        tmp.path(),
        &identity,
        "ratification",
        "intent-1",
        serde_json::json!({ "presence": "tty+challenge" }),
    )
    .unwrap();
    let entries = journal::read(tmp.path()).unwrap();
    assert_eq!(entries, vec![first.clone(), second.clone()]);
    assert!(journal::path(tmp.path()).ends_with(".loom/journal/events.jsonl"));
    // The module intentionally offers append/read/exists only: no edit/delete
    // operation can rewrite this audit history through Loom's API.
    assert!(journal::exists(tmp.path(), &first.id).unwrap());
}

#[test]
fn journal_evidence_references_resolve_or_fail_closed() {
    let tmp = Tmp::new();
    let identity = loom::identity::ExecutionIdentity::solo();
    let entry = journal::append(
        tmp.path(),
        &identity,
        "proof_run",
        "validation-1",
        serde_json::json!({}),
    )
    .unwrap();
    assert!(evidence::stamp(tmp.path(), &journal::reference(&entry)).is_ok());
    let err = evidence::stamp(tmp.path(), "journal:not-real").unwrap_err();
    assert!(err
        .to_string()
        .contains("no such append-only journal entry"));
}
