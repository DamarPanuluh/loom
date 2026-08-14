//! Ring 51 — semantic checkpoint recommendations are exact and read-only.

use clap::Parser;
use loom::checkpoint::{self, CheckpointStatus};
use loom::cli::{CheckpointCmd, Cli, CodefileCmd, Command};
use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::store::Store;

mod common;
use common::Tmp;

struct Fixture {
    // Drop the open store before its temporary root.
    store: Store,
    intent: loom::model::Node,
    tmp: Tmp,
}

fn git(root: &std::path::Path, args: &[&str]) {
    let _ = git_output(root, args);
}

fn git_output(root: &std::path::Path, args: &[&str]) -> Vec<u8> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {} failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn ready_fixture() -> Fixture {
    let tmp = Tmp::new();
    git(tmp.path(), &["init", "-q"]);
    git(tmp.path(), &["config", "user.name", "Loom Test"]);
    git(
        tmp.path(),
        &["config", "user.email", "loom@example.invalid"],
    );
    tmp.write(".gitignore", ".loom/\n");
    tmp.write(
        "src/feature.rs",
        "pub fn checkpoint_behavior() -> &'static str { \"v1\" }\n",
    );

    let store = Store::init(tmp.path(), Some("checkpoint-test"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "checkpoint behavior",
            "the repository exposes one checkpoint behavior",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .ratify_intent(&intent.id, "accepted test behavior", "test fixture")
        .unwrap();
    let file = store
        .add_node(
            NodeType::CodeFile,
            "src/feature.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let grounding = store
        .ensure_edge(EdgeKind::Implements, &intent.id, &file.id)
        .unwrap();
    store
        .set_facet(
            &grounding.id,
            TargetKind::Edge,
            "locator",
            "checkpoint_behavior",
            TruthClass::Asserted,
        )
        .unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();
    loom::commands::prove_intent(&store, &intent.id, "checkpoint proof", "true").unwrap();
    loom::travel::export_to_file(&store).unwrap();
    git(tmp.path(), &["add", "."]);
    git(tmp.path(), &["commit", "-qm", "baseline"]);

    tmp.write(
        "src/feature.rs",
        "pub fn checkpoint_behavior() -> &'static str { \"v2\" }\n",
    );
    loom::sync::run(&store, tmp.path()).unwrap();
    let validation = store
        .resolve_node("checkpoint proof", Some(NodeType::Validation))
        .unwrap();
    loom::commands::observe_validation(&store, &validation).unwrap();
    loom::travel::export_to_file(&store).unwrap();

    Fixture { store, intent, tmp }
}

#[test]
fn checkpoint_and_anchor_cli_surfaces_are_strict() {
    let parsed = Cli::try_parse_from([
        "loom",
        "checkpoint",
        "recommend",
        "--intent",
        "one",
        "--intent",
        "two",
    ])
    .unwrap();
    match parsed.command.unwrap() {
        Command::Checkpoint {
            cmd: CheckpointCmd::Recommend { intents },
        } => assert_eq!(intents, ["one", "two"]),
        other => panic!("unexpected command: {other:?}"),
    }
    assert!(Cli::try_parse_from(["loom", "checkpoint", "recommend"]).is_err());

    let parsed = Cli::try_parse_from([
        "loom",
        "codefile",
        "anchor",
        "src/lib.rs",
        "--at-line",
        "42",
    ])
    .unwrap();
    match parsed.command.unwrap() {
        Command::Codefile {
            cmd:
                CodefileCmd::Anchor {
                    path,
                    at_line,
                    at_symbol,
                },
        } => {
            assert_eq!(path, "src/lib.rs");
            assert_eq!(at_line, Some(42));
            assert_eq!(at_symbol, None);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn ready_recommendation_is_exact_deterministic_and_read_only() {
    let fixture = ready_fixture();
    let before = fixture.store.snapshot().unwrap();
    let head_before = git_output(fixture.tmp.path(), &["rev-parse", "HEAD"]);
    let commit_count_before = git_output(fixture.tmp.path(), &["rev-list", "--count", "HEAD"]);
    let status_before = git_output(
        fixture.tmp.path(),
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    );
    let remotes_before = git_output(fixture.tmp.path(), &["remote", "-v"]);
    let index_before = std::fs::read(fixture.tmp.path().join(".git/index")).unwrap();
    let config_before = std::fs::read(fixture.tmp.path().join(".git/config")).unwrap();
    let reflog_before = std::fs::read(fixture.tmp.path().join(".git/logs/HEAD")).unwrap();
    let keys = vec![fixture.intent.id.clone()];

    let first = checkpoint::recommend(&fixture.store, &keys).unwrap();
    let second = checkpoint::recommend(&fixture.store, &keys).unwrap();
    assert_eq!(
        first.status,
        CheckpointStatus::Ready,
        "{:#?}",
        first.blockers
    );
    assert_eq!(
        first
            .included_paths
            .iter()
            .map(|path| path.path.as_str())
            .collect::<Vec<_>>(),
        ["loom.graph.json", "src/feature.rs"]
    );
    assert!(first.excluded_paths.is_empty());
    assert_eq!(
        first.scope.intent_ids.as_slice(),
        std::slice::from_ref(&fixture.intent.id)
    );
    assert_eq!(first.scope.intent_names, ["checkpoint behavior"]);
    assert_eq!(
        first.included_paths[0].reason,
        "portable graph projection for the selected scope"
    );
    assert_eq!(
        first.included_paths[1].intent_ids.as_slice(),
        std::slice::from_ref(&fixture.intent.id)
    );
    assert!(first.included_paths[1]
        .reason
        .contains("grounded to selected Intent"));
    assert!(first.checks.iter().all(|check| check.status == "passed"));
    assert!(first.checks.iter().any(|check| check.id == "loom_export"
        && check
            .evidence
            .iter()
            .any(|line| line == "loom.graph.json matches the live graph")));
    assert_eq!(
        first.suggested_message.as_deref(),
        Some("feat: checkpoint behavior")
    );
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&second).unwrap()
    );
    assert_eq!(fixture.store.snapshot().unwrap(), before);
    let local = &first.driver_policy.local_commit;
    assert_eq!(local.authority, "acting_llm");
    assert!(local.may_commit_or_defer);
    assert!(local.stage_only_included_paths);
    assert_eq!(local.forbidden_command, "git add -A");
    assert!(local.defer_on_ambiguity);
    assert_eq!(local.publication, "local_only");
    let push = &first.driver_policy.push;
    assert!(!push.allowed_without_human_decision);
    assert_eq!(
        push.required_binding,
        ["repository", "remote", "branch", "commit"]
    );
    assert!(push.drift_requires_new_decision);
    assert_eq!(push.silence_or_refusal, "keep_local");

    assert_eq!(
        git_output(fixture.tmp.path(), &["rev-parse", "HEAD"]),
        head_before
    );
    assert_eq!(
        git_output(fixture.tmp.path(), &["rev-list", "--count", "HEAD"]),
        commit_count_before,
        "recommendation created a commit"
    );
    assert_eq!(
        git_output(
            fixture.tmp.path(),
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"]
        ),
        status_before
    );
    assert_eq!(
        git_output(fixture.tmp.path(), &["remote", "-v"]),
        remotes_before,
        "recommendation changed or contacted a remote"
    );
    assert_eq!(
        std::fs::read(fixture.tmp.path().join(".git/index")).unwrap(),
        index_before,
        "recommendation staged paths"
    );
    assert_eq!(
        std::fs::read(fixture.tmp.path().join(".git/config")).unwrap(),
        config_before
    );
    assert_eq!(
        std::fs::read(fixture.tmp.path().join(".git/logs/HEAD")).unwrap(),
        reflog_before,
        "recommendation wrote local history"
    );
}

#[test]
fn unrelated_dirty_paths_are_excluded_but_staged_overlap_blocks() {
    let fixture = ready_fixture();
    fixture.tmp.write("notes/user.txt", "unrelated user work\n");
    let keys = vec![fixture.intent.id.clone()];
    let report = checkpoint::recommend(&fixture.store, &keys).unwrap();
    assert_eq!(
        report.status,
        CheckpointStatus::Ready,
        "{:#?}",
        report.blockers
    );
    assert!(report
        .excluded_paths
        .iter()
        .any(|path| path.path == "notes/user.txt"));

    git(fixture.tmp.path(), &["add", "notes/user.txt"]);
    let blocked = checkpoint::recommend(&fixture.store, &keys).unwrap();
    assert_eq!(blocked.status, CheckpointStatus::Blocked);
    assert!(blocked
        .blockers
        .iter()
        .any(|blocker| blocker.kind == "excluded_path_staged"));
}

#[test]
fn preexisting_staged_scope_is_user_owned_and_blocks() {
    let fixture = ready_fixture();
    git(fixture.tmp.path(), &["add", "src/feature.rs"]);
    let report =
        checkpoint::recommend(&fixture.store, std::slice::from_ref(&fixture.intent.id)).unwrap();
    assert_eq!(report.status, CheckpointStatus::Blocked);
    assert!(report.blockers.iter().any(|blocker| {
        blocker.kind == "excluded_path_staged" && blocker.paths == ["src/feature.rs"]
    }));
    assert!(report.excluded_paths.iter().any(|path| {
        path.path == "src/feature.rs" && path.reason.contains("staging ownership is ambiguous")
    }));
}

#[test]
fn repository_drift_blocks_without_rewriting_the_graph() {
    let fixture = ready_fixture();
    let before = fixture.store.snapshot().unwrap();
    fixture.tmp.write(
        "src/feature.rs",
        "pub fn checkpoint_behavior() -> &'static str { \"v3\" }\n",
    );
    let report =
        checkpoint::recommend(&fixture.store, std::slice::from_ref(&fixture.intent.id)).unwrap();
    assert_eq!(report.status, CheckpointStatus::Blocked);
    assert!(report
        .blockers
        .iter()
        .any(|blocker| blocker.kind == "sync_stale"));
    assert_eq!(fixture.store.snapshot().unwrap(), before);
}

#[test]
fn sync_preview_detects_missing_and_new_glob_matches_without_registering_them() {
    let fixture = ready_fixture();
    fixture
        .store
        .set_meta("codefile_globs", r#"["src/**/*.rs"]"#)
        .unwrap();
    fixture.tmp.write("src/new.rs", "pub fn new_file() {}\n");
    std::fs::remove_file(fixture.tmp.path().join("src/feature.rs")).unwrap();
    let before = fixture.store.snapshot().unwrap();

    let preview = loom::sync::preview(&fixture.store, fixture.tmp.path()).unwrap();
    assert!(!preview.fresh);
    assert_eq!(preview.missing_files, ["src/feature.rs"]);
    assert_eq!(preview.unregistered_glob_matches, ["src/new.rs"]);
    assert_eq!(fixture.store.snapshot().unwrap(), before);
}
