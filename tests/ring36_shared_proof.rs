//! Ring 36 — one command cannot prove many behaviors.
//!
//! If a single command is registered as the proof of seven behaviors, it is at
//! most exercising one of them. The others inherit its green from whatever it
//! really tests, and each stays "proven" for exactly as long as that unrelated
//! suite keeps passing.
//!
//! This is not hypothetical. An intent claiming "a locator that cannot resolve
//! falls back to file-scope reopening" carried TWO passing validations, both
//! running `cargo test --test ring6 -q`, while thirteen groundings with
//! unresolvable locators sat green underneath it — the behavior did not exist.
//! Nothing caught it: `proof_too_shallow_for_intent` gates user-visible intents
//! only, and the strength machinery already grades these S2 for want of a call
//! witness without any rung consuming that below user_visible.
//!
//! Reported, never gated. A ring genuinely covering several behaviors is a
//! legitimate shape, so this earns a verdict with a reason rather than a wall.

use loom::model::{EdgeKind, NodeType, TruthClass};
use loom::store::Store;
mod common;
use common::Tmp;

fn behavior(store: &Store, name: &str) -> String {
    store
        .add_node(
            NodeType::Intent,
            name,
            "a behavior",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap()
        .id
}

/// Register `command` as the proof of `intent`.
fn proved_by(store: &Store, intent: &str, proof_name: &str, command: &str) {
    let v = store
        .add_node(
            NodeType::Validation,
            proof_name,
            "",
            "passed",
            serde_json::json!({ "type": "test", "command": command }),
        )
        .unwrap();
    store
        .add_edge(EdgeKind::Validates, &v.id, intent, TruthClass::Asserted)
        .unwrap();
}

fn smells_of(store: &Store, kind: &str) -> Vec<loom::signal::Smell> {
    loom::signal::smells(store)
        .unwrap()
        .into_iter()
        .filter(|s| s.kind == kind)
        .collect()
}

/// **Two behaviors leaning on one command is reported, and counted.**
#[test]
fn one_command_proving_two_behaviors_is_reported() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = behavior(&store, "the first behavior");
    let b = behavior(&store, "the second behavior");
    proved_by(&store, &a, "first proof", "cargo test --test ring6 -q");
    proved_by(&store, &b, "second proof", "cargo test --test ring6 -q");

    let found = smells_of(&store, "shared_proof_command");
    assert_eq!(found.len(), 1, "one command, one smell: {found:#?}");
    assert!(
        found[0].message.contains("2 behaviors"),
        "the count is what makes it actionable: {}",
        found[0].message
    );
    assert!(
        found[0].message.contains("cargo test --test ring6 -q"),
        "and the command is named so the reader can go look: {}",
        found[0].message
    );
    assert!(
        !found[0].remedy.is_empty(),
        "an audit that only accuses is a scoreboard"
    );
}

/// **A command proving exactly one behavior is silent.**
///
/// This is the shape being asked for — narrowing a proof to its behavior must
/// not itself be reported.
#[test]
fn a_command_proving_one_behavior_is_silent() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = behavior(&store, "the first behavior");
    let b = behavior(&store, "the second behavior");
    proved_by(
        &store,
        &a,
        "first proof",
        "cargo test --test ring6 first_case",
    );
    proved_by(
        &store,
        &b,
        "second proof",
        "cargo test --test ring6 second_case",
    );

    assert!(
        smells_of(&store, "shared_proof_command").is_empty(),
        "each behavior has its own command; nothing to report"
    );
}

/// **Two proofs of the SAME behavior sharing a command is not the complaint.**
///
/// Belt and braces on one claim is redundancy, not a borrowed green — the
/// smell counts distinct behaviors, not validations.
#[test]
fn two_proofs_of_one_behavior_sharing_a_command_are_silent() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = behavior(&store, "the only behavior");
    proved_by(&store, &a, "one proof", "cargo test --test ring6 -q");
    proved_by(&store, &a, "another proof", "cargo test --test ring6 -q");

    assert!(
        smells_of(&store, "shared_proof_command").is_empty(),
        "one behavior, however many proofs, is not a borrowed green"
    );
}

/// **Identity keys on the command, not the behaviors leaning on it.**
///
/// The set of behaviors sharing a suite grows as work lands. Keying identity on
/// that set would re-open the adjudication every time a sibling appeared, so a
/// judgment made once would never survive.
#[test]
fn identity_survives_a_new_behavior_joining_the_same_command() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = behavior(&store, "the first behavior");
    let b = behavior(&store, "the second behavior");
    proved_by(&store, &a, "first proof", "cargo test --test ring6 -q");
    proved_by(&store, &b, "second proof", "cargo test --test ring6 -q");
    let before = smells_of(&store, "shared_proof_command")[0]
        .identity
        .clone();

    let c = behavior(&store, "a third behavior joins later");
    proved_by(&store, &c, "third proof", "cargo test --test ring6 -q");
    let after = smells_of(&store, "shared_proof_command");

    assert_eq!(
        after[0].identity, before,
        "a durable adjudication must survive the group growing"
    );
    assert!(
        after[0].message.contains("3 behaviors"),
        "while the message still reports the current count: {}",
        after[0].message
    );
}

/// **A validation nobody wired to a behavior is not counted.**
///
/// An unlinked proof proves nothing about any intent, so it cannot be lending
/// its green to one.
#[test]
fn an_unlinked_validation_is_not_counted() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = behavior(&store, "the only behavior");
    proved_by(&store, &a, "linked proof", "cargo test --test ring6 -q");
    store
        .add_node(
            NodeType::Validation,
            "orphan proof",
            "",
            "passed",
            serde_json::json!({ "type": "test", "command": "cargo test --test ring6 -q" }),
        )
        .unwrap();

    assert!(
        smells_of(&store, "shared_proof_command").is_empty(),
        "an unlinked validation lends its green to nobody"
    );
}

/// **The duplicate command is called out at WRITE time, not only later.**
///
/// A warning, never a refusal: a ring genuinely covering several behaviors is a
/// legitimate shape — fifteen of this repo's shared commands are exactly that —
/// so refusing would break honest work to catch dishonest work. But saying it
/// when the proof is registered is the only moment it is cheap; afterwards it
/// costs a smell, a triage verdict, and someone re-deriving why.
#[test]
fn registering_a_command_that_already_proves_another_behavior_still_succeeds() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = behavior(&store, "the first behavior");
    proved_by(&store, &a, "first proof", "cargo test --test ring6 -q");
    drop(store);

    // The second registration goes through the real CLI so the warning path is
    // the one a user actually hits.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .args([
            "--graph",
            tmp.path().to_str().unwrap(),
            "intent",
            "add",
            "--name",
            "the second behavior",
            "--description",
            "does another thing",
            "--lifecycle",
            "implemented",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "fixture intent added");

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .args([
            "--graph",
            tmp.path().to_str().unwrap(),
            "validation",
            "add",
            "--name",
            "second proof",
            "--type",
            "test",
            "--command",
            "cargo test --test ring6 -q",
            "--intent",
            "the second behavior",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "a shared command is a warning, not a refusal — honest rings must still register"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already the proof of"),
        "and the warning names the collision: {stderr}"
    );
    assert!(
        stderr.contains("the first behavior"),
        "including which behavior it collides with: {stderr}"
    );
}
