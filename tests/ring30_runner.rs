//! Ring 30 — the runner is the one thing the evidence spine cannot check.
//!
//! Everything else in loom is guarded by anchoring: a claim must point at
//! something re-checkable, and `assert_fact` refuses what does not. But the
//! anchor for a proof is a `RunRecord`, and a `RunRecord` is produced by the
//! runner. The spine can prove a claim IS anchored; it cannot prove the anchor
//! was produced correctly.
//!
//! That is not theoretical. `loom observe` held the graph's write lock across
//! its own child, so any command that also opened the graph — and loom's own
//! journey proofs are all `loom journey run …` — blocked on its observer and
//! exited non-zero. loom recorded a FAILING verdict against a behavior that
//! passes: a false fact, anchored, `verified`, written by the component whose
//! whole purpose is honest observation. Nothing in the spine could have caught
//! it, because the anchor was real.
//!
//! So the runner needs its own invariants, and they are these:
//!
//!   1. No runner holds the graph while running a subprocess.
//!   2. A failure loom's own infrastructure caused is never attributed to the
//!      code under test.

use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::store::Store;
mod common;
use common::*;

fn loom_bin() -> std::path::PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("loom")
}

fn seeded(root: &std::path::Path) -> String {
    let store = Store::init(root, Some("t"), false).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/thing.rs"), "pub fn thing() -> u8 { 1 }\n").unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "a behavior under proof",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = store
        .add_node(
            NodeType::CodeFile,
            "src/thing.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let g = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &g.id,
            TargetKind::Edge,
            "locator",
            "fn thing",
            TruthClass::Asserted,
        )
        .unwrap();
    intent.id
}

/// **No runner holds the graph while running a subprocess.**
///
/// Checked against every entry point that spawns one, not just the one that
/// was broken — the next runner added is the one most likely to repeat it.
#[test]
fn no_runner_holds_the_graph_while_it_observes() {
    for (label, args) in [
        (
            "observe",
            vec!["observe", "--for", "a behavior under proof"],
        ),
        ("validation run", vec!["validation", "run", "the proof"]),
    ] {
        let tmp = Tmp::new();
        let intent = seeded(tmp.path());

        // The proof's command opens the SAME graph for writing, exactly as
        // `loom journey run` and `loom sync` do.
        let child = format!(
            "{} --graph {} sync",
            loom_bin().display(),
            tmp.path().display()
        );
        {
            let store = Store::open(tmp.path()).unwrap();
            let val = store
                .add_node(
                    NodeType::Validation,
                    "the proof",
                    "",
                    "not_run",
                    serde_json::json!({ "type": "test", "command": child.clone() }),
                )
                .unwrap();
            store
                .ensure_edge(EdgeKind::Validates, &val.id, &intent)
                .unwrap();
        }

        let mut cmd = std::process::Command::new(loom_bin());
        cmd.arg("--graph").arg(tmp.path()).args(&args);
        if label == "observe" {
            cmd.arg("--").arg("sh").arg("-c").arg(&child);
        }
        let out = cmd.output().expect("spawn loom");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );

        // Whatever it reports, it must NOT be a failing verdict caused by loom
        // holding its own lock.
        assert!(
            !text.contains("loom-lock-contention"),
            "{label} ran its child while holding the graph: {text}"
        );
        let store = Store::open(tmp.path()).unwrap();
        for v in store
            .list_nodes(Some(NodeType::Validation), usize::MAX)
            .unwrap()
        {
            assert_ne!(
                v.status, "failed",
                "{label} recorded a FAILING verdict for a command that only \
                 needed the graph: {text}"
            );
        }
    }
}

/// **Infrastructure failure is never attributed to the code.**
///
/// If a runner ever does hold the lock again, the outcome must be `blocked` —
/// recorded, visible, never green, and never a claim that the behavior broke.
/// Defence in depth for the failure mode above.
#[test]
fn a_failure_loom_caused_is_blocked_not_failed() {
    let tmp = Tmp::new();
    let _intent = seeded(tmp.path());

    // Hold the graph, then run a command that needs it — the exact shape of the
    // original bug, forced deliberately.
    let held = Store::open(tmp.path()).unwrap();
    let child = format!(
        "{} --graph {} sync",
        loom_bin().display(),
        tmp.path().display()
    );
    let observation = loom::runner::observe_command(
        tmp.path(),
        loom::model::RunProducer::Command,
        &child,
        &[],
        0,
        60,
    )
    .expect("no io failure");
    drop(held);

    match observation {
        loom::runner::Observation::Blocked { reason } => {
            assert!(
                reason.contains("loom held it") || reason.contains("could not be observed"),
                "the reason must name loom's own infrastructure: {reason}"
            );
        }
        loom::runner::Observation::Ran(run) => {
            assert_eq!(
                run.exit_code, 0,
                "a non-zero exit caused by loom's own lock must never be recorded \
                 as a run — that is a false fact about the code"
            );
        }
    }
}

/// A run records what it covered, so an edit expires it. Without this a proof
/// stays green over code it no longer describes, which is the failure `covered`
/// exists to prevent.
#[test]
fn an_observed_run_records_what_it_covered() {
    let tmp = Tmp::new();
    let intent = seeded(tmp.path());
    let store = Store::open(tmp.path()).unwrap();
    let files = loom::runner::files_grounding(&store, &intent).unwrap();
    assert!(
        files.contains(&"src/thing.rs".to_string()),
        "the behavior's grounded files are the run's covered set: {files:?}"
    );

    let observation = loom::runner::observe_command(
        tmp.path(),
        loom::model::RunProducer::Command,
        "true",
        &files,
        0,
        60,
    )
    .expect("no io failure");
    match observation {
        loom::runner::Observation::Ran(run) => {
            assert!(
                run.covered.contains_key("src/thing.rs"),
                "the run must record the hash of what it covered: {:?}",
                run.covered
            );
            assert!(
                !run.covered["src/thing.rs"].is_empty(),
                "and a real fingerprint, or an edit cannot expire it"
            );
        }
        loom::runner::Observation::Blocked { reason } => panic!("`true` should run: {reason}"),
    }
}
