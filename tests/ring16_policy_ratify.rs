//! Ring 16 — policy-delegated ratification remains human-authorized while its
//! individual intent evidence is honestly machine-attributed.

use loom::cli::{Cli, Command, IntentCmd};
use loom::model::{NodeType, TargetKind, TruthClass};
use loom::policy::{self, RatificationPolicies, RatificationPolicy};
use loom::registry::OwnerRole;
use loom::store::{Agent, Store};
mod common;
use common::*;

static CLI_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn intent(store: &Store, name: &str, origin: &str, level: &str) -> loom::model::Node {
    let intent = store
        .add_node(
            NodeType::Intent,
            name,
            "a policy-scoped behavior",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "origin",
            origin,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "level",
            level,
            TruthClass::Asserted,
        )
        .unwrap();
    intent
}

fn policy() -> RatificationPolicy {
    RatificationPolicy {
        name: "llm-refactor".into(),
        enabled: true,
        origins: vec!["llm".into()],
        levels: vec!["component".into()],
        lifecycles: vec!["planned".into()],
        human_authored_at: "2026-07-19T12:00:00Z".into(),
    }
}

#[test]
fn policy_ratifies_only_matching_intents_with_machine_attribution() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let matching = intent(&store, "matching refactor", "llm", "component");
    let non_matching = intent(&store, "human feature", "human", "feature");

    let applied = policy::apply_ratification_policy(&store, &policy(), "tty+challenge").unwrap();
    assert_eq!(
        applied.iter().map(|n| &n.id).collect::<Vec<_>>(),
        vec![&matching.id]
    );
    assert_eq!(
        store
            .get_facet(&matching.id, TargetKind::Node, "ratification")
            .unwrap()
            .as_deref(),
        Some("ratified")
    );
    assert_eq!(
        store
            .get_facet(&matching.id, TargetKind::Node, "ratified_by")
            .unwrap()
            .as_deref(),
        Some("policy:llm-refactor")
    );
    let notes = store.notes_for(&matching.id).unwrap();
    assert!(notes[0]
        .description
        .contains("by policy 'llm-refactor' (human-authored 2026-07-19)"));
    assert_eq!(
        store
            .get_facet(&non_matching.id, TargetKind::Node, "ratification")
            .unwrap(),
        None,
        "the policy must leave non-matching intents untouched"
    );
}

#[test]
fn policy_application_rejects_an_llm_lane() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    intent(&store, "matching refactor", "llm", "component");
    store.set_agent(Agent::Lane(OwnerRole::Builder));

    let err = policy::apply_ratification_policy(&store, &policy(), "tty+challenge")
        .expect_err("a policy delegates scope but never LLM authority");
    assert!(err.to_string().contains("INV-8"), "got: {err}");
}

#[test]
fn cli_policy_application_rejects_piped_stdin() {
    let _guard = CLI_LOCK.lock().unwrap();
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    intent(&store, "matching refactor", "llm", "component");
    policy::save_ratification_policies(
        &store,
        &RatificationPolicies {
            policies: vec![policy()],
        },
    )
    .unwrap();
    drop(store);

    let err = loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: Some(Command::Intent {
            cmd: IntentCmd::Ratify {
                key: None,
                all: false,
                by_policy: Some("llm-refactor".into()),
                evidence: None,
            },
        }),
    })
    .expect_err("cargo test stdin is non-interactive");
    let message = err.to_string();
    assert!(message.contains("INV-8"), "got: {message}");
    assert!(message.contains("62b197cc"), "got: {message}");
}
