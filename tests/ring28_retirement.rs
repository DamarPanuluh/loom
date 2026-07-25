//! Ring 28 — retirement is total.
//!
//! One invariant instead of three descriptions of where it was broken.
//!
//! Retirement failed three separate times in one session: residue counts kept
//! a retired proof gating the ladder, the validation tally kept reporting it as
//! failed, and the ownership index kept it co-owning its files. Each was found
//! by hitting it, each got its own test, and nothing prevented a fourth — I
//! checked two more call sites and they were safe by accident (they filter on
//! `implemented`, and a retired intent is `deprecated`), not by any rule.
//!
//! The rule, stated once: **a retired behavior contributes no CLAIMS** — no
//! residue, no proof debt, no ownership, no smells, no divergence. A graph
//! carrying a retired intent and its whole apparatus reads like one built
//! without it, on every axis that counts claims.
//!
//! With one deliberate exception, which is not a leak but the point: retiring
//! an intent does not delete its code. The file stays registered and becomes
//! UNOWNED, so `covered` goes red and the compass routes to coverage. That is
//! real work — "the only thing that claimed this is gone; what owns it now, or
//! should it go too?" — and hiding it would let deleted capability leave
//! orphaned code behind looking covered.
//!
//! This catches a fourth site by construction rather than by someone
//! remembering to look.

use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store;
mod common;
use common::*;

/// Build the graph. With `include_doomed`, also build a second behavior with a
/// full apparatus around it — and retire it at the end.
fn build(root: &std::path::Path, include_doomed: bool) -> Store {
    let store = Store::init(root, Some("t"), false).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/kept.rs"), "pub fn kept() -> u8 { 1 }\n").unwrap();

    let keeper = store
        .add_node(
            NodeType::Intent,
            "a behavior that stays",
            "the one that survives",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let kept_cf = store
        .add_node(
            NodeType::CodeFile,
            "src/kept.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let g = store
        .add_edge(
            EdgeKind::Implements,
            &keeper.id,
            &kept_cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &g.id,
            TargetKind::Edge,
            "locator",
            "fn kept",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &g.id,
            InspectionStatus::Passing,
            "the behavior lives here",
            "src/kept.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    loom::commands::prove_intent(&store, &keeper.id, "keeper proof", "true").unwrap();

    if include_doomed {
        // A behavior with everything a real one has: its own file, a grounding
        // with a verdict, a proof that FAILS, a relationship to the survivor,
        // and a shared file so it co-owns something.
        std::fs::write(root.join("src/doomed.rs"), "pub fn doomed() -> u8 { 2 }\n").unwrap();
        let doomed = store
            .add_node(
                NodeType::Intent,
                "a behavior that gets removed",
                "the one deleted on purpose",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        let doomed_cf = store
            .add_node(
                NodeType::CodeFile,
                "src/doomed.rs",
                "",
                "",
                serde_json::json!({}),
            )
            .unwrap();
        let dg = store
            .add_edge(
                EdgeKind::Implements,
                &doomed.id,
                &doomed_cf.id,
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .set_facet(
                &dg.id,
                TargetKind::Edge,
                "locator",
                "fn doomed",
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .record_verdict(
                &dg.id,
                InspectionStatus::Passing,
                "it lives here too",
                "src/doomed.rs:1",
                0.9,
                "llm",
            )
            .unwrap();
        // Co-ownership of the survivor's file, which is what produced the
        // spurious structural smell.
        store
            .add_edge(
                EdgeKind::Implements,
                &doomed.id,
                &kept_cf.id,
                TruthClass::Asserted,
            )
            .unwrap();
        // A relationship into the survivor.
        store
            .add_edge(
                EdgeKind::Relates,
                &doomed.id,
                &keeper.id,
                TruthClass::Asserted,
            )
            .unwrap();
        // A proof that fails — the thing that gated the whole ladder.
        loom::commands::prove_intent(&store, &doomed.id, "doomed proof", "false").unwrap();

        store
            .retire_intent(&doomed.id, "the capability was deleted on purpose", None)
            .unwrap();
    }

    loom::sync::run(&store, root).unwrap();
    store
}

/// Every number loom derives about a graph, in one comparable shape.
fn derived_view(store: &Store) -> Vec<(String, String)> {
    let mut view: Vec<(String, String)> = Vec::new();

    // `coverage` is excluded by design: the orphaned file legitimately makes it
    // red, and that is asserted directly in the companion test below. Every
    // other rung must be untouched.
    let ladder = loom::maturity::ladder(store).unwrap();
    for rung in ladder.rungs.iter().filter(|r| r.name != "covered") {
        view.push((
            format!("rung.{}.state", rung.name),
            format!("{:?}", rung.state),
        ));
        view.push((format!("rung.{}.depth", rung.name), rung.depth.to_string()));
    }

    let depths = loom::maturity::depths(store).unwrap();
    for lane in loom::lane::Lane::LADDER
        .iter()
        .filter(|l| **l != loom::lane::Lane::Coverage)
    {
        view.push((
            format!("queue.{}", lane.as_str()),
            depths.get(*lane).to_string(),
        ));
    }

    let proofs = loom::maturity::validation_summary(store).unwrap();
    view.push(("proofs.registered".into(), proofs.registered.to_string()));
    view.push(("proofs.passed".into(), proofs.passed.to_string()));
    view.push(("proofs.failed".into(), proofs.failed.to_string()));

    // Retirement removes the BEHAVIOR, not the file from the registry: the
    // code is still on disk and still registered. So the registered count
    // legitimately differs, and what must match is how much is OWNED — a
    // retired owner is no owner, and its file becomes visible coverage debt
    // rather than quietly counting as covered.
    // Ownership of everything OTHER than the orphan must be unchanged: the
    // retired behavior must not have been propping up any other file.
    let (_, _, unowned, _) = loom::commands::code_ownership_summary(store).unwrap();
    view.push((
        "files.unowned.excluding_orphan".into(),
        unowned
            .iter()
            .filter(|f| *f != "src/doomed.rs")
            .cloned()
            .collect::<Vec<_>>()
            .join(","),
    ));

    // Smells about the SURVIVOR's file: a retired co-owner must not make the
    // survivor look coupled to something that no longer exists.
    let mut smells: Vec<String> = loom::signal::smells(store)
        .unwrap()
        .into_iter()
        .filter(|s| s.message.contains("src/kept.rs"))
        .map(|s| format!("{}:{}", s.kind, s.message))
        .collect();
    smells.sort();
    view.push(("smells.about_kept".into(), smells.join(" | ")));

    view.push((
        "divergences".into(),
        loom::divergence::blocking_count(store).unwrap().to_string(),
    ));
    view.push((
        "risk.candidates".into(),
        loom::risk::rank(store).unwrap().len().to_string(),
    ));
    view.push((
        "audit.findings".into(),
        loom::audit::run(store).unwrap().len().to_string(),
    ));
    view
}

/// A retired behavior is invisible to every derived view.
///
/// Not "the three views I happened to fix" — every one. If a fourth counter
/// learns to read raw edges without asking whether their intent is still real,
/// this fails.
#[test]
fn a_retired_behavior_reads_like_one_that_never_existed() {
    let without = Tmp::new();
    let with = Tmp::new();
    let baseline = derived_view(&build(without.path(), false));
    let retired = derived_view(&build(with.path(), true));

    let mut differences: Vec<String> = Vec::new();
    for ((key, want), (key2, got)) in baseline.iter().zip(retired.iter()) {
        assert_eq!(key, key2, "the two views are built in the same order");
        if want != got {
            differences.push(format!("  {key}: without={want:?} with-retired={got:?}"));
        }
    }
    assert!(
        differences.is_empty(),
        "a retired behavior changed {} derived value(s) — retirement must be total, \
         and every one of these is a place that counts a deprecated intent's edges:\n{}",
        differences.len(),
        differences.join("\n")
    );
}

/// The counterpart: retirement must not be a way to hide LIVE work either.
///
/// The invariant above would also be satisfied by ignoring the survivor, so
/// this pins the other side — the graph still sees everything that was not
/// retired.
#[test]
fn retiring_one_behavior_leaves_the_others_fully_visible() {
    let tmp = Tmp::new();
    let store = build(tmp.path(), true);

    let survivors: Vec<String> = store
        .list_nodes(Some(NodeType::Intent), usize::MAX)
        .unwrap()
        .into_iter()
        .filter(|n| n.status != "deprecated")
        .map(|n| n.name)
        .collect();
    assert_eq!(survivors, vec!["a behavior that stays"]);

    let summary = loom::maturity::validation_summary(&store).unwrap();
    assert_eq!(
        summary.registered, 1,
        "the survivor's proof is still counted: {summary:?}"
    );
    assert_eq!(summary.passed, 1);

    let (_, owned, unowned, _) = loom::commands::code_ownership_summary(&store).unwrap();
    assert!(owned >= 1, "the survivor still owns its file");
    // And the retired behavior's file is now visible work, not quietly covered.
    assert!(
        unowned.contains(&"src/doomed.rs".to_string()),
        "a file whose only owner was retired becomes coverage debt: {unowned:?}"
    );
}
