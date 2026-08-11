//! Ring 36 — one command cannot prove many behaviors, and one verifier cannot
//! strengthen every sibling proof.
//!
//! If a single command is registered as the proof of seven behaviors, it is at
//! most exercising one of them. The others inherit its green from whatever it
//! really tests, and each stays "proven" for exactly as long as that unrelated
//! suite keeps passing.
//!
//! The same ownership rule now applies inside one intent's strength ladder. A
//! genuine verifying test may earn S3 for the validation that runs or explicitly
//! exercises it; a sibling journey that only runs `echo` remains S2. The old
//! intent-wide call witness let every sibling borrow that one verifier's reach,
//! reproducing the shared-command failure shape after registration.
//!
//! This is not hypothetical. An intent claiming "a locator that cannot resolve
//! falls back to file-scope reopening" carried TWO passing validations, both
//! running `cargo test --test ring6 -q`, while thirteen groundings with
//! unresolvable locators sat green underneath it — the behavior did not exist.
//! The shared-command smell reports that structural risk, while validation-
//! specific S3 now prevents an unrelated verifier elsewhere on the intent from
//! making those proofs look stronger than their own execution.
//!
//! Reported, never gated. A ring genuinely covering several behaviors is a
//! legitimate shape, so this earns a verdict with a reason rather than a wall;
//! strength likewise remains a report, now sourced to the validation that earned
//! it.

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

/// **In-process path reaches the warning (call witness).**
///
/// The CLI-spawn test above proves the user-visible stderr contract, but a
/// subprocess spawn does not appear in loom's call graph — so proof strength
/// could not witness `warn_if_command_already_proves_another`. Calling the
/// typed handler the binary uses closes that gap without weakening the CLI
/// assertion.
#[test]
fn in_process_add_of_a_shared_command_reaches_the_warning() {
    use loom::cli::{Cli, Command, IntentCmd, ValidationCmd};

    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = behavior(&store, "the first behavior");
    proved_by(&store, &a, "first proof", "cargo test --test ring6 -q");
    drop(store);

    loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: false,
        command: Some(Command::Intent {
            cmd: IntentCmd::Add {
                name: "the second behavior".into(),
                description: "does another thing".into(),
                level: "feature".into(),
                lifecycle: "implemented".into(),
                visibility: None,
                layer: None,
                aspect: None,
                allow_symbol_name: false,
            },
        }),
    })
    .expect("fixture intent");

    loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: false,
        command: Some(Command::Validation {
            cmd: ValidationCmd::Add {
                name: "second proof".into(),
                r#type: "test".into(),
                command: "cargo test --test ring6 -q".into(),
                intent: "the second behavior".into(),
            },
        }),
    })
    .expect("shared command is a warning, not a refusal");

    let store = Store::open(tmp.path()).unwrap();
    let v = store
        .resolve_node("second proof", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(
        v.body.get("command").and_then(|c| c.as_str()),
        Some("cargo test --test ring6 -q")
    );
}

/// Attack: `validation update` must skip the validation being edited.
#[test]
fn updating_a_validation_command_skips_self_and_still_warns_on_others() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = behavior(&store, "the first behavior");
    let b = behavior(&store, "the second behavior");
    proved_by(&store, &a, "first proof", "cargo test --test ring6 -q");
    proved_by(&store, &b, "second proof", "true");
    drop(store);

    // Updating second proof TO the shared command should warn about first.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .args([
            "--graph",
            tmp.path().to_str().unwrap(),
            "--json",
            "validation",
            "update",
            "second proof",
            "--command",
            "cargo test --test ring6 -q",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "update must succeed (warn, not refuse)"
    );
    let payload: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(payload["collision"]["detected"], true);
    assert_eq!(
        payload["collision"]["prior_behaviors"],
        serde_json::json!([{"id": a, "name": "the first behavior"}])
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already the proof of"),
        "update path must warn: {stderr}"
    );
    assert!(
        stderr.contains("the first behavior"),
        "and name the other behavior: {stderr}"
    );

    // Updating first proof to the SAME command it already has must NOT warn
    // about itself (skip_validation).
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .args([
            "--graph",
            tmp.path().to_str().unwrap(),
            "validation",
            "update",
            "first proof",
            "--command",
            "cargo test --test ring6 -q",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    // It will still warn about second proof (now also on that command) — that is
    // correct. It must not list 'the first behavior' as a collision with itself.
    assert!(
        !stderr.contains("'the first behavior'"),
        "skip-self must not name the intent this validation already proves: {stderr}"
    );
}

/// Attack: JSON clients receive deterministic collision facts while the same
/// conversational warning remains on stderr for a human at the terminal.
#[test]
fn duplicate_command_json_is_structured_while_warning_stays_on_stderr() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let zeta = behavior(&store, "zeta prior behavior");
    let alpha = behavior(&store, "alpha prior behavior");
    proved_by(&store, &zeta, "zeta proof", "cargo test --test ring6 -q");
    proved_by(
        &store,
        &alpha,
        "alpha proof one",
        "cargo test --test ring6 -q",
    );
    proved_by(
        &store,
        &alpha,
        "alpha proof two",
        "cargo test --test ring6 -q",
    );
    drop(store);

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .args([
            "--graph",
            tmp.path().to_str().unwrap(),
            "--json",
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
    assert!(out.status.success());

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .args([
            "--graph",
            tmp.path().to_str().unwrap(),
            "--json",
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
    assert!(out.status.success());
    let payload: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(payload["validation"]["name"], "second proof");
    assert_eq!(payload["edge"]["kind"], "validates");
    assert_eq!(
        payload["collision"],
        serde_json::json!({
            "detected": true,
            "command": "cargo test --test ring6 -q",
            "prior_behavior_count": 2,
            "prior_behaviors": [
                {"id": alpha, "name": "alpha prior behavior"},
                {"id": zeta, "name": "zeta prior behavior"}
            ]
        }),
        "collision rows must be unique and sorted by behavior name/id"
    );
    assert!(
        stderr.contains("already the proof of"),
        "the conversational warning must remain on stderr: {stderr}"
    );
    assert!(stderr.contains("alpha prior behavior"), "{stderr}");
    assert!(stderr.contains("zeta prior behavior"), "{stderr}");

    let store = Store::open(tmp.path()).unwrap();
    let registered = store
        .resolve_node("second proof", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(registered.status, "not_run");
    assert_eq!(
        store
            .edges_with(Some(EdgeKind::Validates), Some(&registered.id), None)
            .unwrap()
            .len(),
        1,
        "the nonblocking diagnostic must preserve atomic registration"
    );
}

#[test]
fn unique_and_blank_commands_report_explicit_empty_collision_objects() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let unique_target = behavior(&store, "unique target");
    let blank_target = behavior(&store, "blank target");
    drop(store);

    let register = |name: &str, command: &str, intent: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
            .args([
                "--graph",
                tmp.path().to_str().unwrap(),
                "--json",
                "validation",
                "add",
                "--name",
                name,
                "--type",
                "test",
                "--command",
                command,
                "--intent",
                intent,
            ])
            .output()
            .unwrap()
    };

    for (name, command, intent) in [
        (
            "unique proof",
            "cargo test --test ring36 unique-sentinel",
            unique_target.as_str(),
        ),
        ("blank proof", "", blank_target.as_str()),
    ] {
        let out = register(name, command, intent);
        assert!(
            out.status.success(),
            "registration failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !String::from_utf8_lossy(&out.stderr).contains("already the proof of"),
            "non-collisions must stay conversationally silent"
        );
        let payload: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            payload["collision"],
            serde_json::json!({
                "detected": false,
                "command": command,
                "prior_behavior_count": 0,
                "prior_behaviors": []
            })
        );
    }
}

/// **The collision lookup asks SQLite, and asks it exactly.**
///
/// The warning behind these tests runs on every `validation add`/`update`, and
/// used to find its collisions by listing EVERY validation in the graph with an
/// unbounded limit, deserializing each body, then running an edge query per node
/// — to discard all but the handful that matched (finding e8735f90). The
/// question is now a single indexed statement that returns only real collisions.
///
/// Asserted through the store rather than through timing: a benchmark would pin
/// the machine, not the behavior. What matters is that the lookup is exact —
/// matching commands only, never a prefix or a near-miss, and never the
/// validation being edited.
#[test]
fn the_command_collision_lookup_returns_only_exact_matches() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    let mk = |name: &str, command: &str| {
        store
            .add_node(
                NodeType::Validation,
                name,
                "",
                "not_run",
                serde_json::json!({ "type": "test", "command": command }),
            )
            .unwrap()
            .id
    };
    let one = mk("one", "cargo test --test ring6 -q");
    let two = mk("two", "cargo test --test ring6 -q");
    // A prefix of the target, and a superstring of it: both must be excluded, or
    // the warning would accuse commands that share a suite but not a test.
    let _prefix = mk("prefix", "cargo test --test ring6");
    let _longer = mk("longer", "cargo test --test ring6 -q --nocapture");
    // A validation with no command key at all must not explode the extract.
    store
        .add_node(
            NodeType::Validation,
            "bodyless",
            "",
            "not_run",
            serde_json::json!({ "type": "test" }),
        )
        .unwrap();

    let hits = store
        .validations_with_command("cargo test --test ring6 -q", None)
        .unwrap();
    assert_eq!(
        hits.len(),
        2,
        "exactly the two validations registering that command, got {hits:?}"
    );
    assert!(hits.contains(&one) && hits.contains(&two));

    // Skipping self is how `update` avoids warning about the row it is editing.
    let others = store
        .validations_with_command("cargo test --test ring6 -q", Some(&one))
        .unwrap();
    assert_eq!(
        others,
        vec![two.clone()],
        "self is excluded, the sibling is not"
    );

    assert!(
        store
            .validations_with_command("cargo test --test nothing", None)
            .unwrap()
            .is_empty(),
        "a command nobody registers collides with nobody"
    );
}
