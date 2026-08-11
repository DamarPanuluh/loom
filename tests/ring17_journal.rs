//! Ring 17 — append-only evidence journal and journal citations.

use clap::Parser;
use loom::cli::{Cli, Command, SurfaceCmd};
use loom::model::NodeType;
use loom::store::Store;
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

#[test]
fn surface_remove_requires_a_reason() {
    assert!(Cli::try_parse_from(["loom", "surface", "remove", "api"]).is_err());
    let parsed = Cli::try_parse_from([
        "loom",
        "surface",
        "remove",
        "api",
        "--reason",
        "the endpoint was retired",
    ])
    .unwrap();
    match parsed.command.unwrap() {
        Command::Surface {
            cmd: SurfaceCmd::Remove { key, reason },
        } => {
            assert_eq!(key, "api");
            assert_eq!(reason, "the endpoint was retired");
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn surface_remove_rejects_blank_before_mutation_and_journals_substantive_reason() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("surface removal"));
    let add = loom_command()
        .args(["--graph"])
        .arg(tmp.path())
        .args(["surface", "add", "--name", "api"])
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );

    let blank = loom_command()
        .args(["--graph"])
        .arg(tmp.path())
        .args(["surface", "remove", "api", "--reason", "   "])
        .output()
        .unwrap();
    assert!(!blank.status.success());
    assert!(String::from_utf8_lossy(&blank.stderr)
        .contains("surface remove needs substantive --reason"));
    let store = Store::open(tmp.path()).unwrap();
    assert!(store
        .resolve_node("api", Some(NodeType::InterfaceSurface))
        .is_ok());
    drop(store);
    assert!(journal::read(tmp.path()).unwrap().is_empty());

    let placeholder = loom_command()
        .args(["--graph"])
        .arg(tmp.path())
        .args(["surface", "remove", "api", "--reason", "todo"])
        .output()
        .unwrap();
    assert!(!placeholder.status.success());
    assert!(Store::open(tmp.path())
        .unwrap()
        .resolve_node("api", Some(NodeType::InterfaceSurface))
        .is_ok());

    let reason = "the endpoint was retired";
    let removed = loom_command()
        .args(["--graph"])
        .arg(tmp.path())
        .args(["surface", "remove", "api", "--reason", reason, "--json"])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(Store::open(tmp.path())
        .unwrap()
        .resolve_node("api", Some(NodeType::InterfaceSurface))
        .is_err());
    let entries = journal::read(tmp.path()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].event, "node_removed");
    assert_eq!(entries[0].payload["kind"], "interface_surface");
    assert_eq!(entries[0].payload["name"], "api");
    assert_eq!(entries[0].payload["reason"], reason);
}
