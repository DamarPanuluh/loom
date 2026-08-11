//! Ring 19 — drive sessions are terminal-only and can freeze journaled chains.

use loom::cli::{Cli, Command, DriveCmd};
use loom::store::Store;
mod common;
use common::*;

#[test]
fn drive_rejects_noninteractive_stdin() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let err = loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: Some(Command::Drive { cmd: None }),
    })
    .expect_err("test stdin is non-interactive");
    assert!(err.to_string().contains("non-interactive ratification"));
}

#[test]
fn drive_freeze_registers_a_semantic_journaled_chain() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .append_journal(
            "drive_exchange",
            "demo",
            serde_json::json!({
                "utterance": "show status",
                "intent": "status can be shown",
                "command": "printf driven",
            }),
        )
        .unwrap();
    drop(store);
    loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: Some(Command::Drive {
            cmd: Some(DriveCmd::Freeze {
                name: "demo".into(),
            }),
        }),
    })
    .unwrap();
    let yaml = std::fs::read_to_string(tmp.path().join("journeys/demo.yaml")).unwrap();
    assert!(yaml.contains("show status"));
    assert!(!yaml.contains("printf driven"));
    assert!(!tmp.path().join(".loom/baselines").exists());
    let store = Store::open(tmp.path()).unwrap();
    assert!(store
        .resolve_node("demo", Some(loom::model::NodeType::Journey))
        .is_ok());
}

#[test]
fn drive_freeze_keeps_failed_execution_evidence_out_of_semantics() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .append_journal(
            "drive_exchange",
            "failed-drive",
            serde_json::json!({
                "utterance": "run a failing check",
                "intent": "check succeeds",
                "command": "false",
            }),
        )
        .unwrap();
    drop(store);

    loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: Some(Command::Drive {
            cmd: Some(DriveCmd::Freeze {
                name: "failed-drive".into(),
            }),
        }),
    })
    .unwrap();

    let artifact = tmp.path().join("journeys/failed-drive.yaml");
    let yaml = std::fs::read_to_string(&artifact).unwrap();
    assert!(yaml.contains("run a failing check"));
    assert!(!yaml.contains("command"));
    assert!(!yaml.contains("false"));
    assert!(!tmp.path().join(".loom/baselines").exists());
    let parsed = loom::journey::parse(&artifact).unwrap();
    assert_eq!(parsed.id, "failed-drive");
}
