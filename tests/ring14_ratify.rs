//! Ring 14 — ratification: wantedness as a falsifiable, human-only fact.
//!
//! Real SQLite, no mocks. The contract under test (docs/rethink-lived-graph.md):
//! anyone may mint an intent, only a human may ratify one (INV-8, fail closed);
//! absent ratification reads as unratified (never presumed); redefinition
//! stales ratification; the `converged` rung, the divergence queue depth, and
//! the served ratify work item all agree because they share one predicate.

use loom::cli::{Cli, Command, IntentCmd};
use loom::lane::Lane;
use loom::model::{NodeType, TargetKind};
use loom::registry::OwnerRole;
use loom::store::{Agent, Store};
mod common;
use common::*;

/// Tests that route through `commands::run` (CLI layer) serialize here so a
/// concurrent test can never observe a half-configured process environment.
static CLI_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn ratification(store: &Store, id: &str) -> Option<String> {
    store
        .get_facet(id, TargetKind::Node, "ratification")
        .unwrap()
}

fn cli_add_intent(root: &std::path::Path, name: &str) {
    loom::commands::run(Cli {
        graph: Some(root.to_path_buf()),
        json: true,
        command: Some(Command::Intent {
            cmd: IntentCmd::Add {
                name: name.into(),
                description: "a falsifiable behavior for the ratify ring".into(),
                level: "feature".into(),
                lifecycle: "planned".into(),
                visibility: None,
                layer: None,
                aspect: None,
                allow_symbol_name: false,
            },
        }),
    })
    .unwrap();
}

// =========================================================================
// 1. INV-8: every llm:* lane is denied ratification; solo (human) may write.
// =========================================================================
#[test]
fn inv8_ratify_rejects_every_llm_lane() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "payment can be captured",
            "a behavior",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    for lane in [
        OwnerRole::Builder,
        OwnerRole::Analyzer,
        OwnerRole::Fixer,
        OwnerRole::Validator,
        OwnerRole::Quality,
    ] {
        store.set_agent(Agent::Lane(lane));
        let err = store
            .ratify_intent(&intent.id, "the human said so", "test fixture")
            .expect_err("an llm lane must never ratify");
        assert!(
            err.to_string().contains("INV-8"),
            "rejection must name INV-8, got: {err}"
        );
    }
    // No lane write leaked through.
    assert_eq!(ratification(&store, &intent.id), None);
    // The human (solo) may ratify — with evidence.
    store.set_agent(Agent::Solo);
    store
        .ratify_intent(
            &intent.id,
            "drive utterance 2026-07-18: checkout must capture payment",
            "test fixture",
        )
        .unwrap();
    assert_eq!(
        ratification(&store, &intent.id).as_deref(),
        Some("ratified")
    );
}

// =========================================================================
// 2. Evidence gate (INV-6 applied to ratification): placeholders rejected.
// =========================================================================
#[test]
fn ratify_requires_substantive_evidence() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "user can log in",
            "a behavior",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    for bad in ["", "  ", "…", "todo", "<reason>"] {
        assert!(
            store
                .ratify_intent(&intent.id, bad, "test fixture")
                .is_err(),
            "placeholder evidence {bad:?} must be rejected"
        );
    }
    assert_eq!(ratification(&store, &intent.id), None);
}

// =========================================================================
// 3. Fail closed: absent ratification facet reads as unratified everywhere
//    (predicate, queue count, ladder) — wantedness is never presumed.
// =========================================================================
#[test]
fn absent_ratification_is_unratified_and_gates_the_wanted_rung() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "orders can be cancelled",
            "a behavior",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();

    // Shared predicate sees it; queue count agrees; ladder rung is unmet.
    assert_eq!(loom::workitem::unratified_intents(&store).unwrap().len(), 1);
    assert_eq!(
        loom::maturity::depths(&store)
            .unwrap()
            .get(Lane::Divergence),
        1
    );
    let ladder = loom::maturity::ladder(&store).unwrap();
    let wanted = ladder.rungs.iter().find(|r| r.name == "converged").unwrap();
    assert_eq!(wanted.state, loom::maturity::RungState::Unmet);
    // The inversion: `converged` sits ABOVE the lanes an LLM can drain, so a
    // planned intent outranks the human question. The human is asked last,
    // about the fewest items — never as the gate that blocks realization.
    assert_eq!(ladder.phase, "build");
    assert_eq!(
        loom::lane::Lane::Divergence.rung(),
        "converged",
        "the human-presence rung is the divergence rung"
    );

    // The served work item targets the same intent, human-gated.
    let item = loom::workitem::next(&store, loom::lane::Lane::parse("ratify"))
        .unwrap()
        .expect("ratify queue must serve the unratified intent");
    assert_eq!(item.target.id, intent.id);
    assert_eq!(item.owner_role, "human");
    assert!(item.prompt_contract.human_gate.is_some());

    // Plain `loom next` (no mode) must NEVER serve human-only work.
    if let Some(w) = loom::workitem::next(&store, None).unwrap() {
        assert_ne!(w.mode, "ratify", "default next must not serve ratify");
    }

    // Ratify → everything closes in lockstep.
    store
        .ratify_intent(&intent.id, "curated dogfood spine", "test fixture")
        .unwrap();
    assert_eq!(loom::workitem::unratified_intents(&store).unwrap().len(), 0);
    assert_eq!(
        loom::maturity::depths(&store)
            .unwrap()
            .get(Lane::Divergence),
        0
    );
    let ladder = loom::maturity::ladder(&store).unwrap();
    let wanted = ladder.rungs.iter().find(|r| r.name == "converged").unwrap();
    assert_eq!(wanted.state, loom::maturity::RungState::Met);
}

// =========================================================================
// 4. Wantedness rots with meaning: redefinition stales ratification.
// =========================================================================
#[test]
fn redefinition_stales_ratification() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "reports can be exported",
            "exports a csv report",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .ratify_intent(&intent.id, "asked for in the roadmap", "test fixture")
        .unwrap();
    assert_eq!(
        ratification(&store, &intent.id).as_deref(),
        Some("ratified")
    );

    store
        .redefine_intent(&intent.id, "exports a signed pdf report")
        .unwrap();
    assert_eq!(
        ratification(&store, &intent.id).as_deref(),
        Some("needs_reconfirmation"),
        "a redefined intent is no longer known-wanted"
    );
    // …and it is back in the ratify queue.
    assert_eq!(
        loom::maturity::depths(&store)
            .unwrap()
            .get(Lane::Divergence),
        1
    );
}

// =========================================================================
// 5. Ratification provenance is an asserted historical fact, not current
// wantedness: it records the human and timestamp, then survives a stale.
// =========================================================================
#[test]
fn ratification_records_human_and_timestamp() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "reports can be shared",
            "a behavior",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .ratify_intent(
            &intent.id,
            "human review accepted this report feature",
            "test fixture",
        )
        .unwrap();

    assert_eq!(
        store
            .get_facet(&intent.id, TargetKind::Node, "ratified_by")
            .unwrap()
            .as_deref(),
        Some("human")
    );
    assert!(
        store
            .get_facet(&intent.id, TargetKind::Node, "ratified_at")
            .unwrap()
            .is_some(),
        "a ratification must record when the human asserted it"
    );
    assert_eq!(
        store
            .get_facet(&intent.id, TargetKind::Node, "ratified_presence")
            .unwrap()
            .as_deref(),
        Some("test fixture"),
        "new ratifications retain their demonstrated-presence descriptor"
    );
}

// =========================================================================
// 5b. The CLI ratification path rejects piped input before it can delegate to
// the store. The store remains directly testable without a TTY.
// =========================================================================
#[test]
fn cli_ratify_rejects_noninteractive_stdin_with_the_inv8_finding() {
    let _guard = CLI_LOCK.lock().unwrap();
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "piped ratification is rejected",
            "a behavior",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    drop(store);

    let err = loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: Some(Command::Intent {
            cmd: IntentCmd::Ratify {
                key: Some(intent.name),
                all: false,
                by_policy: None,
                evidence: Some("an interactive human requested this".into()),
            },
        }),
    })
    .expect_err("cargo test stdin is piped, never an interactive terminal");
    let message = err.to_string();
    assert!(message.contains("INV-8"), "got: {message}");
    assert!(message.contains("62b197cc"), "got: {message}");
}

// =========================================================================
// 5c. A builder may assess code impact, but never silently reconfirm human
// wantedness. Preserved behavior leaves ratification intact; a changed
// criterion returns the intent to the human-only ratify queue.
// =========================================================================
#[test]
fn semantic_impact_preserves_or_routes_human_reconfirmation() {
    let _guard = CLI_LOCK.lock().unwrap();
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store.set_agent(Agent::Solo);
    let intent = store
        .add_node(
            NodeType::Intent,
            "semantic impact is classified before reconfirmation",
            "the LLM classifies whether a code change preserves or changes the intent criterion",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .ratify_intent(&intent.id, "human approved this behavior", "test fixture")
        .unwrap();
    drop(store);

    let prior_agent = std::env::var_os("LOOM_AGENT");
    std::env::set_var("LOOM_AGENT", "llm:builder");

    loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: Some(Command::Intent {
            cmd: IntentCmd::Impact {
                key: intent.name.clone(),
                classification: "preserved".into(),
                evidence: "src/commands/intent.rs: intent impact records an assessment".into(),
            },
        }),
    })
    .unwrap();

    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        store
            .get_facet(&intent.id, TargetKind::Node, "semantic_impact")
            .unwrap()
            .as_deref(),
        Some("preserved")
    );
    assert_eq!(
        ratification(&store, &intent.id).as_deref(),
        Some("ratified")
    );
    drop(store);

    loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: Some(Command::Intent {
            cmd: IntentCmd::Impact {
                key: intent.name.clone(),
                classification: "criterion_changed".into(),
                evidence: "src/commands/intent.rs: criterion changed branch stales ratification"
                    .into(),
            },
        }),
    })
    .unwrap();

    match prior_agent {
        Some(value) => std::env::set_var("LOOM_AGENT", value),
        None => std::env::remove_var("LOOM_AGENT"),
    }

    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        ratification(&store, &intent.id).as_deref(),
        Some("needs_reconfirmation")
    );
    assert_eq!(
        loom::maturity::depths(&store)
            .unwrap()
            .get(Lane::Divergence),
        1
    );
}

#[test]
fn ratification_provenance_survives_redefinition_staleness() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "reports can be shared",
            "shares a csv report",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .ratify_intent(
            &intent.id,
            "human review accepted this report feature",
            "test fixture",
        )
        .unwrap();
    let ratified_at = store
        .get_facet(&intent.id, TargetKind::Node, "ratified_at")
        .unwrap();

    store
        .redefine_intent(&intent.id, "shares a signed pdf report")
        .unwrap();

    assert_eq!(
        ratification(&store, &intent.id).as_deref(),
        Some("needs_reconfirmation")
    );
    assert_eq!(
        store
            .get_facet(&intent.id, TargetKind::Node, "ratified_by")
            .unwrap()
            .as_deref(),
        Some("human")
    );
    assert_eq!(
        store
            .get_facet(&intent.id, TargetKind::Node, "ratified_at")
            .unwrap(),
        ratified_at
    );
}

// =========================================================================
// 6. Provenance at minting (CLI layer): a solo (human) mint is born ratified
//    with origin=human — the minting act is the ratification evidence.
// =========================================================================
#[test]
fn solo_mint_is_born_ratified_with_human_origin() {
    let _guard = CLI_LOCK.lock().unwrap();
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    cli_add_intent(tmp.path(), "topics can be captured through door");

    let store = Store::open(tmp.path()).unwrap();
    let n = &store
        .list_nodes(Some(NodeType::Intent), usize::MAX)
        .unwrap()[0];
    assert_eq!(
        store
            .get_facet(&n.id, TargetKind::Node, "origin")
            .unwrap()
            .as_deref(),
        Some("human")
    );
    assert_eq!(ratification(&store, &n.id).as_deref(), Some("ratified"));
    assert_eq!(
        loom::maturity::depths(&store)
            .unwrap()
            .get(Lane::Divergence),
        0
    );
}
