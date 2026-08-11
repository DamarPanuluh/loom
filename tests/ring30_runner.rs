//! Ring 30 — the runner is the one thing the evidence spine cannot check.
//!
//! Everything else in loom is guarded by anchoring: a claim must point at
//! something re-checkable, and `assert_fact` refuses what does not. But the
//! anchor for a proof is a `RunRecord`, and a `RunRecord` is produced by the
//! runner. The spine can prove a claim IS anchored; it cannot prove the anchor
//! was produced correctly.
//!
//! That is not theoretical. `loom observe` held the graph's write lock across
//! its own child, so any command that also opened the graph — including a
//! compiled Journey profile settling proof — blocked on its observer and
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

use loom::identity::Agent;
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

fn shell_token(value: &std::path::Path) -> String {
    let value = value.to_string_lossy();
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || "._-/=:@+,".contains(c))
    {
        value.into_owned()
    } else if !value.contains('\'') {
        format!("'{value}'")
    } else {
        assert!(
            !value.contains('"') && !value.chars().any(|c| matches!(c, '$' | '`' | '\\')),
            "test path cannot be represented by the strict whole-token parser: {value}"
        );
        format!("\"{value}\"")
    }
}

fn seeded(root: &std::path::Path) -> String {
    let store = Store::init(root, Some("t"), false).unwrap();
    store.set_agent(Agent::Solo);
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
        // compiled Journey settlement and `loom sync` do.
        let child = format!(
            "{} --graph {} sync",
            shell_token(&loom_bin()),
            shell_token(tmp.path())
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
/// A direct, simple current-loom `sync` command that encounters a held graph
/// lock must be classified as blocked rather than recorded as a code failure.
#[test]
fn a_failure_loom_caused_is_blocked_not_failed() {
    let observer = Tmp::new();
    let _intent = seeded(observer.path());
    let contended = Tmp::new();
    let _other_intent = seeded(contended.path());
    let held = Store::open(contended.path()).unwrap();
    let bin = loom_bin();
    let child_args = [
        bin.to_string_lossy().into_owned(),
        "--graph".into(),
        contended.path().to_string_lossy().into_owned(),
        "sync".into(),
    ];
    let out = std::process::Command::new(&bin)
        .arg("--graph")
        .arg(observer.path())
        .arg("--json")
        .arg("observe")
        .arg("--")
        .args(&child_args)
        .output()
        .expect("spawn observing loom");
    drop(held);

    assert!(
        out.status.success(),
        "a blocked observation is recorded, not a CLI failure: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("blocked observation JSON: {e}: {:?}", out));
    assert_eq!(value["observed"], false, "{value}");
    assert!(
        value["blocked"]
            .as_str()
            .is_some_and(|reason| reason.contains("infrastructure contention")),
        "{value}"
    );
}

#[test]
fn a_command_that_cannot_start_is_blocked_not_failed() {
    let tmp = Tmp::new();
    let missing_cwd = tmp.path().join("missing-working-directory");
    let observation = loom::runner::observe_command(
        &missing_cwd,
        loom::model::RunProducer::Command,
        "true",
        &[],
        0,
        60,
    )
    .expect("a start failure is an observed block, not an I/O error");

    match observation {
        loom::runner::Observation::Blocked { reason } => {
            assert!(reason.contains("could not start"), "{reason}");
        }
        loom::runner::Observation::Ran(run) => {
            panic!("an unstarted command cannot produce a run: {run:?}");
        }
    }
}

#[test]
fn a_timed_out_command_is_blocked_not_failed() {
    let tmp = Tmp::new();
    let observation = loom::runner::observe_command(
        tmp.path(),
        loom::model::RunProducer::Command,
        "sleep 1",
        &[],
        0,
        0,
    )
    .expect("a timeout is an observed block, not an I/O error");

    match observation {
        loom::runner::Observation::Blocked { reason } => {
            assert!(reason.contains("exceeded timeout_secs=0"), "{reason}");
        }
        loom::runner::Observation::Ran(run) => {
            panic!("a timed-out command cannot produce a run: {run:?}");
        }
    }
}

/// Exit 75 is only loom contention when an exact out-of-band attestation
/// accompanies it. Other programs may use the same conventional
/// temporary-failure code, and their result remains an observed run rather than
/// an infrastructure block.
#[test]
fn an_arbitrary_shell_exit_75_is_recorded_as_ran() {
    let tmp = Tmp::new();
    let observation = loom::runner::observe_command(
        tmp.path(),
        loom::model::RunProducer::Command,
        "exit 75",
        &[],
        0,
        60,
    )
    .expect("no io failure");

    match observation {
        loom::runner::Observation::Ran(run) => assert_eq!(run.exit_code, 75),
        loom::runner::Observation::Blocked { reason } => {
            panic!("an arbitrary exit 75 is a completed run, not contention: {reason}")
        }
    }
}

/// Even the exact private frame cannot spoof contention when an arbitrary
/// command discovers the public environment variable name. It receives no
/// capability, so its conventional exit 75 remains a completed run.
#[cfg(unix)]
#[test]
fn an_arbitrary_command_cannot_write_a_spoofed_attestation() {
    let tmp = Tmp::new();
    let command = r#"if [ -n "$LOOM_CONTENTION_FD" ]; then eval "printf 'LOOM-CONTENTION/1\\n' >&$LOOM_CONTENTION_FD"; fi; exit 75"#;
    let observation = loom::runner::observe_command(
        tmp.path(),
        loom::model::RunProducer::Command,
        command,
        &[],
        0,
        60,
    )
    .expect("no io failure");

    match observation {
        loom::runner::Observation::Ran(run) => assert_eq!(run.exit_code, 75),
        loom::runner::Observation::Blocked { reason } => {
            panic!("an arbitrary command must not receive the private FD: {reason}")
        }
    }
}

/// A PATH-spoofed `sh` must not become the intermediary for a directly
/// attestable loom invocation. Besides preventing command substitution, this
/// keeps the private descriptor out of any shell chosen by untrusted PATH.
#[cfg(unix)]
#[test]
fn direct_current_loom_bypasses_a_malicious_path_shell() {
    use std::os::unix::fs::PermissionsExt;

    let observer = Tmp::new();
    let _intent = seeded(observer.path());
    let contended = Tmp::new();
    let contended_root = contended.path().join("contended graph");
    let _other_intent = seeded(&contended_root);
    let held = Store::open(&contended_root).unwrap();

    let helpers = observer.path().join("helpers");
    std::fs::create_dir_all(&helpers).unwrap();
    let leaked = observer.path().join("path-sh-saw-contention-fd");
    let sh = helpers.join("sh");
    std::fs::write(
        &sh,
        format!(
            "#!/bin/sh\nif [ -n \"$LOOM_CONTENTION_FD\" ]; then printf leaked > '{}'; fi\nexec /bin/sh \"$@\"\n",
            leaked.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![helpers];
    paths.extend(std::env::split_paths(&old_path));
    let path = std::env::join_paths(paths).unwrap();
    let bin = loom_bin();
    let spaced_bin = observer.path().join("loom alias with spaces");
    std::os::unix::fs::symlink(&bin, &spaced_bin).unwrap();
    let out = std::process::Command::new(&bin)
        .arg("--graph")
        .arg(observer.path())
        .arg("--json")
        .arg("observe")
        .arg("--")
        .arg(&spaced_bin)
        .arg("--graph")
        .arg(&contended_root)
        .arg("sync")
        .env("PATH", path)
        .output()
        .expect("spawn observing loom");
    drop(held);

    assert!(
        out.status.success(),
        "direct contention should be recorded as blocked: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["observed"], false, "{value}");
    assert!(value["blocked"].is_string(), "{value}");
    assert!(
        !leaked.exists(),
        "no PATH-selected shell may observe the direct mode's private FD"
    );
}

/// Shell-mode commands retain normal shell semantics but have the capability
/// environment explicitly removed.
#[cfg(unix)]
#[test]
fn ordinary_shell_commands_work_without_the_attestation_environment() {
    let tmp = Tmp::new();
    let captured = loom::subprocess::run(
        r#"printf 'left-'; printf '%s' "${LOOM_CONTENTION_FD-unset}"; printf '%s' '-right'"#,
        tmp.path(),
        std::time::Duration::from_secs(5),
    )
    .expect("shell spawn")
    .expect("shell completed");

    assert_eq!(captured.status.code(), Some(0));
    assert_eq!(captured.stdout, b"left-unset-right");
}

/// Public diagnostic marker text is not an attestation, on either output
/// stream. A test may legitimately print loom's graph or harness error text and
/// use exit 75; that completed failure must stay a run.
#[test]
fn public_contention_markers_cannot_spoof_a_block() {
    let tmp = Tmp::new();
    for command in [
        "printf 'loom-lock-contention\\n'; exit 75",
        "printf 'loom-harness-contention\\n' >&2; exit 75",
    ] {
        let observation = loom::runner::observe_command(
            tmp.path(),
            loom::model::RunProducer::Command,
            command,
            &[],
            0,
            60,
        )
        .expect("no io failure");
        match observation {
            loom::runner::Observation::Ran(run) => assert_eq!(run.exit_code, 75),
            loom::runner::Observation::Blocked { reason } => {
                panic!("public marker text must not attest contention: {reason}")
            }
        }
    }
}

/// Shell control syntax disables capability installation even when the command
/// starts with the current executable. The strict parser behavior is exercised
/// directly in `subprocess` unit tests; this integration check confirms such a
/// command still runs normally through the shell.
#[cfg(unix)]
#[test]
fn shell_metacharacters_after_current_exe_disable_attestation() {
    let tmp = Tmp::new();
    let command = format!(
        "{} --graph {} sync; exit 0",
        std::env::current_exe().unwrap().display(),
        tmp.path().display()
    );
    let observation = loom::runner::observe_command(
        tmp.path(),
        loom::model::RunProducer::Command,
        &command,
        &[],
        0,
        60,
    )
    .expect("no io failure");

    match observation {
        loom::runner::Observation::Ran(run) => assert_eq!(run.exit_code, 0),
        loom::runner::Observation::Blocked { reason } => {
            panic!("a frame without final exit 75 is not contention: {reason}")
        }
    }
}

/// A directly attested loom consumes the capability at startup. Its own `sync`
/// may spawn arbitrary helper commands, which must not see the capability. A
/// fake `git` placed first on PATH records any environment leak.
#[cfg(unix)]
#[test]
fn nested_arbitrary_command_cannot_inherit_the_attestation_capability() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = Tmp::new();
    let _intent = seeded(tmp.path());
    let helpers = tmp.path().join("helpers");
    std::fs::create_dir_all(&helpers).unwrap();
    let leaked = tmp.path().join("contention-fd-leaked");
    let git = helpers.join("git");
    std::fs::write(
        &git,
        format!(
            "#!/bin/sh\nif [ -n \"$LOOM_CONTENTION_FD\" ]; then printf leaked > '{}'; fi\nexit 1\n",
            leaked.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![helpers.clone()];
    paths.extend(std::env::split_paths(&old_path));
    let path = std::env::join_paths(paths).unwrap();
    let bin = loom_bin();
    let child_args = [
        bin.to_string_lossy().into_owned(),
        "--graph".into(),
        tmp.path().to_string_lossy().into_owned(),
        "sync".into(),
    ];
    let out = std::process::Command::new(&bin)
        .arg("--graph")
        .arg(tmp.path())
        .arg("observe")
        .arg("--")
        .args(&child_args)
        .env("PATH", path)
        .output()
        .expect("spawn directly attested loom");
    assert!(
        out.status.success(),
        "nested sync observation should complete: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !leaked.exists(),
        "startup must remove LOOM_CONTENTION_FD before nested commands spawn"
    );
}

/// A run records what it covered, so an edit expires it. Without this a proof
/// stays green over code it no longer describes, which is the failure `covered`
/// exists to prevent.
#[test]
fn an_observed_run_records_what_it_covered() {
    let tmp = Tmp::new();
    let intent = seeded(tmp.path());
    let store = Store::open(tmp.path()).unwrap();
    let files = store.files_grounding(&intent).unwrap();
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

/// **A quality rule is measured against the code, not against the test.**
///
/// The expiry set and the measurement set are different questions asked of the
/// same groundings. A proof must expire when EITHER the code or its test moves,
/// so `files_grounding` returns both roles. But a code-quality rule is a claim
/// about the code the behavior lives in — and the shapes those rules forbid are
/// idiomatic in a test. A test SHOULD `.unwrap()`; panicking on an unexpected
/// `None` is how it reports failure.
///
/// Without this split loom scanned `verifies` groundings for rule violations
/// and then refused the resulting `passing` verdict over its own test files,
/// which made the verdict unreachable for every intent proved by a Rust test —
/// 34 unsettleable pairs in loom's own graph.
#[test]
fn a_rule_is_measured_against_realizing_files_not_verifying_ones() {
    let tmp = Tmp::new();
    let intent = seeded(tmp.path());
    let store = Store::open(tmp.path()).unwrap();
    store.set_agent(Agent::Solo);

    // Attach a test file to the same behavior, in the `verifies` role.
    std::fs::create_dir_all(tmp.path().join("tests")).unwrap();
    std::fs::write(
        tmp.path().join("tests/thing_test.rs"),
        "#[test]\nfn t() { assert_eq!(thing(), 1); }\n",
    )
    .unwrap();
    let tf = store
        .add_node(
            NodeType::CodeFile,
            "tests/thing_test.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let ve = store
        .add_edge(EdgeKind::Implements, &intent, &tf.id, TruthClass::Asserted)
        .unwrap();
    store
        .set_grounding_role(&ve.id, loom::model::GroundingRole::Verifies)
        .unwrap();

    let expiry = store.files_grounding(&intent).unwrap();
    let measured = store.files_realizing(&intent).unwrap();

    assert!(
        expiry.contains(&"tests/thing_test.rs".to_string())
            && expiry.contains(&"src/thing.rs".to_string()),
        "the expiry set is BOTH roles — an edit to the test must expire the proof too: {expiry:?}"
    );
    assert_eq!(
        measured,
        vec!["src/thing.rs".to_string()],
        "the measurement set is the realizing code alone, with the verifying test excluded: {measured:?}"
    );
}

/// An untagged grounding is `realizes` by default, so the role split must not
/// silently drop a behavior's code from its own measurement.
#[test]
fn an_untagged_grounding_still_counts_as_realizing() {
    let tmp = Tmp::new();
    let intent = seeded(tmp.path());
    let store = Store::open(tmp.path()).unwrap();

    // `seeded` never writes a role facet on the src/thing.rs grounding.
    let measured = store.files_realizing(&intent).unwrap();
    assert_eq!(
        measured,
        vec!["src/thing.rs".to_string()],
        "a grounding with no role facet defaults to realizes and must stay in the \
         measured set, or the filter would erase the very code being judged: {measured:?}"
    );
}
