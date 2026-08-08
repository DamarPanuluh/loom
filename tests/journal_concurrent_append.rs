//! Concurrent journal appenders must not interleave JSONL records.
//!
//! Shared graph readers (`loom next`, etc.) append through separate processes
//! without an exclusive graph lock. Without a journal-specific lock, writers'
//! `to_writer` + newline syscalls can interleave into malformed lines.

use loom::journal;
use loom::store::Store;
use std::process::Command;
use std::sync::{Arc, Barrier};

mod common;
use common::Tmp;

/// Child entry for the multiprocess stress. Returns immediately unless
/// `LOOM_JOURNAL_APPEND_ONCE` points at a graph root.
#[test]
fn journal_append_once_child() {
    let Ok(root) = std::env::var("LOOM_JOURNAL_APPEND_ONCE") else {
        return;
    };
    let tag = std::env::var("LOOM_JOURNAL_APPEND_TAG").unwrap_or_else(|_| "0".into());
    journal::append(
        std::path::Path::new(&root),
        &loom::identity::ExecutionIdentity::solo(),
        "stress",
        &format!("target-{tag}"),
        serde_json::json!({ "tag": tag, "pad": "x".repeat(256) }),
    )
    .expect("append");
}

#[test]
fn concurrent_process_appends_do_not_corrupt_the_journal() {
    let tmp = Tmp::new();
    let _store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let root = tmp.path().to_path_buf();
    let n = 64usize;
    let test_exe = std::env::current_exe().expect("current_exe");

    let mut children = Vec::with_capacity(n);
    for i in 0..n {
        let child = Command::new(&test_exe)
            .env("LOOM_JOURNAL_APPEND_ONCE", root.as_os_str())
            .env("LOOM_JOURNAL_APPEND_TAG", i.to_string())
            .env("RUST_TEST_THREADS", "1")
            .args([
                "--exact",
                "journal_append_once_child",
                "--nocapture",
                "--quiet",
            ])
            .spawn()
            .expect("spawn child");
        children.push(child);
    }
    for (i, mut child) in children.into_iter().enumerate() {
        let status = child.wait().expect("wait child");
        assert!(status.success(), "child {i} failed: {status}");
    }

    let (entries, corrupt) = journal::read_counting(&root).unwrap();
    assert_eq!(
        corrupt,
        0,
        "concurrent process appends must not leave malformed JSONL (got {corrupt}; entries={})",
        entries.len()
    );
    assert_eq!(entries.len(), n, "every append must land as one record");
}

#[test]
fn concurrent_thread_appends_do_not_corrupt_the_journal() {
    let tmp = Tmp::new();
    let _store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let root = tmp.path().to_path_buf();
    let n = 64usize;
    let barrier = Arc::new(Barrier::new(n));
    let mut handles = Vec::with_capacity(n);
    for i in 0..n {
        let root = root.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            journal::append(
                &root,
                &loom::identity::ExecutionIdentity::solo(),
                "stress",
                &format!("target-{i}"),
                serde_json::json!({ "i": i, "pad": "x".repeat(512) }),
            )
            .expect("append")
        }));
    }
    for h in handles {
        h.join().expect("thread");
    }

    let (entries, corrupt) = journal::read_counting(&root).unwrap();
    assert_eq!(
        corrupt, 0,
        "concurrent appends must not leave malformed JSONL lines (got {corrupt})"
    );
    assert_eq!(
        entries.len(),
        n,
        "every append must land as one well-formed record"
    );
}
