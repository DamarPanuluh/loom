//! Ring 44 — a hang is never an acceptable failure mode.
//!
//! Every lock acquisition has a bounded wait, and the timeout error names
//! the holder: the recorded pid, command, access mode, and start time of
//! the process in whose way the contender stands. Operators used to work
//! around silent deadlocks with sleeps; now the refusal itself carries the
//! diagnosis (and exit code 75, so a parent that spawned loom can tell an
//! infrastructure block from a real failure).

use loom::store::Store;
mod common;
use common::*;

fn loom_raw(tmp: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom")))
        .arg("--graph")
        .arg(tmp)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {args:?}: {e}"))
}

/// A writer holding the graph: a read command waits within the budget and
/// then refuses — naming the recorded holder's pid, mode, and command.
#[test]
fn contention_is_bounded_and_names_the_recorded_holder() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    // Exclusive (write) open held across the subprocess call.
    let store = Store::open(tmp.path()).unwrap();

    let started = std::time::Instant::now();
    let out = loom_raw(tmp.path(), &["status", "--json"]);
    let elapsed = started.elapsed();

    assert_eq!(
        out.status.code(),
        Some(75),
        "contention maps to EX_TEMPFAIL, not a generic failure: {out:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(15),
        "the wait is bounded — never a hang: {elapsed:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("loom-lock-contention"),
        "marked as infrastructure, not a verdict: {stderr}"
    );
    assert!(
        stderr.contains("read_lock_wait_ms=10000"),
        "the limit names itself with its threshold: {stderr}"
    );
    assert!(
        stderr.contains(&std::process::id().to_string()),
        "the holder's pid is named: {stderr}"
    );
    assert!(
        stderr.contains("write access, since"),
        "the holder's mode and start time are named: {stderr}"
    );

    drop(store);
    let ok = loom_raw(tmp.path(), &["status", "--json"]);
    assert!(
        ok.status.success(),
        "the lock releases with the holder: {}",
        String::from_utf8_lossy(&ok.stderr)
    );
}

/// A normal write that outlives the fail-fast writer budget is not a failure
/// for a read-only observer. The shared open waits for the bounded read grace
/// period and returns after the writer exits. `status` uses this exact path;
/// testing the store boundary avoids charging its unrelated debug-build graph
/// projection time to the lock-wait assertion.
#[test]
fn a_read_open_waits_for_a_brief_writer_instead_of_exiting_tempfail() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(2_500));
        drop(store);
    });

    let started = std::time::Instant::now();
    let reader = Store::open_read(tmp.path());
    let elapsed = started.elapsed();
    release.join().unwrap();
    let error = reader.as_ref().err().map(|e| format!("{e:#}"));

    assert!(
        reader.is_ok(),
        "a brief writer must not make a read-only command fail: {error:?}"
    );
    assert!(
        elapsed >= std::time::Duration::from_millis(2_000),
        "status did not actually overlap the held writer: {elapsed:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the read should proceed as soon as the writer exits: {elapsed:?}"
    );
}

/// Readers coexist (shared lock), but a writer contending with a reader
/// also gets a bounded, holder-named refusal.
#[test]
fn a_writer_contending_with_a_reader_is_named_and_bounded() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let reader = Store::open_read(tmp.path()).unwrap();

    // Another reader proceeds immediately — shared locks do not contend.
    let ok = loom_raw(tmp.path(), &["status", "--json"]);
    assert!(
        ok.status.success(),
        "readers coexist: {}",
        String::from_utf8_lossy(&ok.stderr)
    );

    // A writer waits within budget, then refuses with the reader's record.
    let started = std::time::Instant::now();
    let out = loom_raw(tmp.path(), &["codefile", "add", "src/blocked.rs"]);
    let elapsed = started.elapsed();
    assert_eq!(out.status.code(), Some(75), "bounded refusal: {out:?}");
    assert!(elapsed < std::time::Duration::from_secs(15), "never a hang");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("loom-lock-contention"), "marked: {stderr}");
    assert!(
        stderr.contains("lock_wait_ms=2000"),
        "write contention keeps the fail-fast limit: {stderr}"
    );
    assert!(
        stderr.contains(&std::process::id().to_string()),
        "the reading holder's pid is named: {stderr}"
    );
    assert!(
        stderr.contains("read access"),
        "the holder's mode is named: {stderr}"
    );
    drop(reader);
}
