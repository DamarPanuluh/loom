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
fn drive_freeze_compiles_a_synthetic_journaled_chain() {
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
    assert!(yaml.contains("printf driven"));
    assert!(tmp.path().join(".loom/baselines/demo.json").exists());
}

#[test]
fn drive_freeze_rejects_a_failed_chain_without_baseline_or_freeze_event() {
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

    let err = loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: Some(Command::Drive {
            cmd: Some(DriveCmd::Freeze {
                name: "failed-drive".into(),
            }),
        }),
    })
    .unwrap_err()
    .to_string();

    assert!(err.contains("step 'drive-1' failed"));
    assert!(!tmp
        .path()
        .join(".loom/baselines/failed-drive.json")
        .exists());
    assert!(loom::journal::read(tmp.path())
        .unwrap()
        .iter()
        .all(|entry| entry.event != "drive_freeze"));
}
