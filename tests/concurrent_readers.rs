//! Concurrent readers — read-only diagnostics must share the lock.
//!
//! A held shared reader must not force `loom doctor` / `loom coverage` into
//! exclusive-lock contention (exit 75). Those commands are pure diagnostics
//! and must use `open_read`, matching Loom's shared-reader facility.

use loom::cli::{Cli, Command};
use loom::store::{Store, LOCK_CONTENTION_MARKER};
use loom::Result;
mod common;
use common::Tmp;

fn run(root: &std::path::Path, command: Command) -> Result<()> {
    loom::commands::run(Cli {
        graph: Some(root.to_path_buf()),
        json: true,
        command: Some(command),
    })
}

#[test]
fn doctor_and_coverage_run_under_a_held_shared_reader() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    drop(store);

    // Hold a shared reader for the whole test — the slow status/next shape.
    let reader = Store::open_read(tmp.path()).expect("shared reader opens");
    assert!(reader.list_nodes(None, 1).is_ok());

    // Baseline: exclusive open must contend while the shared lock is held.
    let exclusive = Store::open(tmp.path());
    let exclusive_err = exclusive.err().map(|e| format!("{e}")).unwrap();
    assert!(
        exclusive_err.contains(LOCK_CONTENTION_MARKER),
        "exclusive open must contend under a held shared reader: {exclusive_err}"
    );

    // A second shared reader proceeds (the facility doctor/coverage use).
    let reader2 = Store::open_read(tmp.path());
    assert!(reader2.is_ok(), "shared readers must proceed together");
    drop(reader2);

    let doctor = run(tmp.path(), Command::Doctor);
    assert!(
        doctor.is_ok(),
        "doctor is read-only and must not take the write lock under a shared reader: {doctor:?}"
    );

    let coverage = run(tmp.path(), Command::Coverage);
    assert!(
        coverage.is_ok(),
        "coverage is read-only and must not take the write lock under a shared reader: {coverage:?}"
    );

    drop(reader);
}
