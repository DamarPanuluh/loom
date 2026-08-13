//! Ring 43 — S3 belongs to the validation that earned it.
//!
//! An intent may have several proofs and one broad verifying surface. That
//! surface is useful migration context, but it cannot lend its call path to a
//! sibling validation. The grade follows the validation's own journey command
//! or explicit `exercises` edge.

use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::proofstrength::{StrengthWitness, STRENGTH_WITNESS_MODEL};
use loom::store::Store;
mod common;
use common::Tmp;

struct Fixture {
    tmp: Tmp,
    store: Store,
    intent_id: String,
    test_file_id: String,
}

fn fixture() -> Fixture {
    let tmp = Tmp::new();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::create_dir_all(tmp.path().join("tests")).unwrap();
    std::fs::create_dir_all(tmp.path().join("journeys")).unwrap();
    std::fs::write(
        tmp.path().join("src/behavior.rs"),
        "pub fn perform_behavior() -> &'static str { \"ok\" }\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("tests/behavior_test.rs"),
        "#[test]\nfn exercises_behavior() { let _ = perform_behavior(); }\n#[test]\nfn another_entry() { let _ = perform_behavior(); }\n",
    )
    .unwrap();

    let store = Store::init(tmp.path(), Some("validation-specific strength"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "the behavior works",
            "observable behavior",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let implementation = store
        .add_node(
            NodeType::CodeFile,
            "src/behavior.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let realizing = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &implementation.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &realizing.id,
            TargetKind::Edge,
            "locator",
            "fn perform_behavior",
            TruthClass::Asserted,
        )
        .unwrap();
    let test_file = store
        .add_node(
            NodeType::CodeFile,
            "tests/behavior_test.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let verifies = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &test_file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_grounding_role(&verifies.id, loom::model::GroundingRole::Verifies)
        .unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();

    Fixture {
        tmp,
        store,
        intent_id: intent.id,
        test_file_id: test_file.id,
    }
}

fn add_journey_validation(
    fixture: &Fixture,
    name: &str,
    _artifact: &str,
    step_command: &str,
) -> String {
    let validation = fixture
        .store
        .add_node(
            NodeType::Validation,
            name,
            "",
            "not_run",
            serde_json::json!({
                "type": "test",
                "command": step_command,
            }),
        )
        .unwrap();
    fixture
        .store
        .ensure_edge(EdgeKind::Validates, &validation.id, &fixture.intent_id)
        .unwrap();
    mark_validation_passing(fixture, &validation.id, step_command);
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    validation.id
}

fn mark_validation_passing(fixture: &Fixture, validation_id: &str, command: &str) {
    // Record a passing run for the SAME command the body declares. We do not
    // re-exec cargo in the temp fixture (no package layout); the grade path
    // only needs ran_and_passed + command↔entry binding. Production still
    // gets runs only from the real runner.
    let run = loom::runner::record(
        fixture.tmp.path(),
        loom::model::RunProducer::Command,
        command,
        &[],
        1,
        0,
        b"proof-ok\n",
        b"",
        1,
    );
    for e in fixture
        .store
        .edges_with(Some(EdgeKind::Validates), Some(validation_id), None)
        .unwrap()
    {
        let mut run = run.clone();
        // Cover the realizing/verifying files so reverify holds in-fixture.
        run.covered = std::collections::BTreeMap::from([
            (
                "src/behavior.rs".into(),
                loom::artifact::fingerprint(
                    &std::fs::read_to_string(fixture.tmp.path().join("src/behavior.rs")).unwrap(),
                ),
            ),
            (
                "tests/behavior_test.rs".into(),
                loom::artifact::fingerprint(
                    &std::fs::read_to_string(fixture.tmp.path().join("tests/behavior_test.rs"))
                        .unwrap(),
                ),
            ),
        ]);
        fixture
            .store
            .assert_fact(
                loom::store::Assertion::new(
                    loom::store::Subject::Edge(e.id.clone()),
                    loom::model::Claim::Verdict,
                    loom::model::InspectionStatus::Passing.as_str(),
                    "test",
                )
                .criterion("proof")
                .confidence(1.0)
                .cited(loom::evidence::cite(fixture.tmp.path(), "proof-ok").unwrap())
                .observed_command(loom::runner::Observation::Ran(Box::new(run))),
            )
            .unwrap();
    }
    fixture
        .store
        .record_proof_stability(validation_id, "passed")
        .unwrap();
    fixture
        .store
        .set_node_status(validation_id, "passed")
        .unwrap();
}

fn witness(store: &Store, validation_id: &str) -> StrengthWitness {
    serde_json::from_str(
        &store
            .get_facet(validation_id, TargetKind::Node, "proof_strength")
            .unwrap()
            .expect("proof strength facet"),
    )
    .unwrap()
}

#[test]
fn uncalled_helper_in_the_test_file_cannot_earn_a_witness() {
    let fixture = fixture();
    // A dead helper that reaches the grounded code, called by nothing: it is
    // NOT harness-executed, so it must never surface as a derived entry.
    let dead_file = fixture.tmp.path().join("tests/dead_helper.rs");
    std::fs::write(
        &dead_file,
        "fn dead_helper() { let _ = perform_behavior(); }\n",
    )
    .unwrap();
    fixture
        .store
        .add_node(
            NodeType::CodeFile,
            "tests/dead_helper.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    assert_eq!(
        graph.file_test_symbols("tests/dead_helper.rs").len(),
        0,
        "a file whose symbols nothing executes has no test entry points"
    );
    let entries = loom::proofstrength::command_entries(
        "cargo test --test dead_helper",
        &graph,
        "journey_command",
    );
    assert!(
        entries.is_empty(),
        "an uncalled helper must fail closed, not look like an executed entry"
    );
}

#[test]
fn uncalled_helper_inside_cfg_test_module_is_not_a_test_entry() {
    let fixture = fixture();
    // The common unit-test layout: a `#[cfg(test)] mod tests` wrapping both a
    // `#[test]` function and an uncalled helper. Only the harness-attributed
    // function is executed by `cargo test`; the dead helper must not be
    // treated as a test entry just because it sits inside the test module.
    let mod_file = fixture.tmp.path().join("tests/cfg_test_mod.rs");
    std::fs::write(
        &mod_file,
        "#[cfg(test)]\nmod tests {\n    use super::*;\n    fn dead_helper() { let _ = perform_behavior(); }\n    #[test]\n    fn exercises_behavior() { let _ = perform_behavior(); }\n}\n",
    )
    .unwrap();
    fixture
        .store
        .add_node(
            NodeType::CodeFile,
            "tests/cfg_test_mod.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let tests = graph.file_test_symbols("tests/cfg_test_mod.rs");
    assert_eq!(
        tests,
        ["exercises_behavior"]
            .into_iter()
            .map(String::from)
            .collect(),
        "only the #[test]-attributed function is harness-executed"
    );
    let entries = loom::proofstrength::command_entries(
        "cargo test --test cfg_test_mod",
        &graph,
        "journey_command",
    );
    assert!(
        entries
            .iter()
            .all(|entry| entry.entry_symbol.as_deref() == Some("exercises_behavior")),
        "only the executed test may be a derived entry: {entries:?}"
    );
}

#[test]
fn validation_that_runs_the_verifier_earns_s3_but_echo_sibling_stays_s2() {
    let fixture = fixture();
    let genuine = add_journey_validation(
        &fixture,
        "genuine proof",
        "journeys/genuine.yaml",
        "cargo test --test behavior_test",
    );
    let echo = add_journey_validation(
        &fixture,
        "echo proof",
        "journeys/echo.yaml",
        "echo proof-ok",
    );
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();

    let genuine_witness = witness(&fixture.store, &genuine);
    assert_eq!(genuine_witness.grade, "S3");
    assert_eq!(
        genuine_witness.call_witness.as_deref(),
        Some("perform_behavior")
    );
    let evidence = genuine_witness
        .call_evidence
        .expect("exact evidence source");
    assert_eq!(evidence.source, "validation_command");
    assert_eq!(evidence.file, "tests/behavior_test.rs");
    assert!(matches!(
        evidence.entry_symbol.as_deref(),
        Some("exercises_behavior" | "another_entry")
    ));
    assert!(evidence.s3_eligible);

    let echo_witness = witness(&fixture.store, &echo);
    assert_eq!(echo_witness.grade, "S2");
    assert_eq!(echo_witness.call_witness, None);
    let fallback = echo_witness.call_evidence.expect("visible fallback");
    assert_eq!(fallback.source, "intent_wide_fallback");
    assert_eq!(fallback.file, "tests/behavior_test.rs");
    assert!(!fallback.s3_eligible);
    assert!(echo_witness
        .next
        .contains("nothing this proof runs reaches the symbol"));
}

#[test]
fn exact_typed_handler_grounding_earns_a_zero_hop_s3_witness() {
    let fixture = fixture();
    let handler_path = "src/commands/capture_cmd.rs";
    std::fs::create_dir_all(fixture.tmp.path().join("src/commands")).unwrap();
    std::fs::write(fixture.tmp.path().join(handler_path), "pub fn door() {}\n").unwrap();
    let handler_file = fixture
        .store
        .add_node(
            NodeType::CodeFile,
            handler_path,
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let realizing = fixture
        .store
        .add_edge(
            EdgeKind::Implements,
            &fixture.intent_id,
            &handler_file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    fixture
        .store
        .set_facet(
            &realizing.id,
            TargetKind::Edge,
            "locator",
            "fn door",
            TruthClass::Asserted,
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();

    let validation = add_journey_validation(
        &fixture,
        "door handler proof",
        "journeys/door-handler.yaml",
        "./loom door 'ship the exact handler' --json",
    );
    let w = witness(&fixture.store, &validation);
    assert_eq!(
        w.grade, "S3",
        "the exact typed entry is the realizing handler itself: {w:?}"
    );
    assert_eq!(w.call_witness.as_deref(), Some("door"));
    let evidence = w.call_evidence.expect("zero-hop call evidence");
    assert_eq!(evidence.source, "validation_command");
    assert_eq!(evidence.file, handler_path);
    assert_eq!(evidence.entry_symbol.as_deref(), Some("door"));
    assert!(evidence.s3_eligible);
}

#[test]
fn zero_hop_symbol_name_in_another_file_does_not_earn_s3() {
    let fixture = fixture();
    let grounded_path = "src/commands/capture_cmd.rs";
    let lookalike_path = "tests/lookalike_handler.rs";
    std::fs::create_dir_all(fixture.tmp.path().join("src/commands")).unwrap();
    std::fs::write(fixture.tmp.path().join(grounded_path), "pub fn door() {}\n").unwrap();
    std::fs::write(
        fixture.tmp.path().join(lookalike_path),
        "pub fn door() {}\n",
    )
    .unwrap();
    let grounded_file = fixture
        .store
        .add_node(
            NodeType::CodeFile,
            grounded_path,
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let lookalike_file = fixture
        .store
        .add_node(
            NodeType::CodeFile,
            lookalike_path,
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let realizing = fixture
        .store
        .add_edge(
            EdgeKind::Implements,
            &fixture.intent_id,
            &grounded_file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    fixture
        .store
        .set_facet(
            &realizing.id,
            TargetKind::Edge,
            "locator",
            "fn door",
            TruthClass::Asserted,
        )
        .unwrap();

    let validation = add_journey_validation(
        &fixture,
        "lookalike handler proof",
        "journeys/lookalike-handler.yaml",
        "echo proof-ok",
    );
    let exercises = fixture
        .store
        .add_edge(
            EdgeKind::Exercises,
            &validation,
            &lookalike_file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    fixture
        .store
        .set_facet(
            &exercises.id,
            TargetKind::Edge,
            "locator",
            "fn door",
            TruthClass::Asserted,
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();

    let w = witness(&fixture.store, &validation);
    assert_eq!(
        w.grade, "S2",
        "symbol-name equality across files is not a zero-hop path: {w:?}"
    );
    assert_eq!(w.call_witness, None);
}

#[test]
fn explicit_validation_evidence_is_specific_and_stales_when_edited() {
    let fixture = fixture();
    let validation_id = add_journey_validation(
        &fixture,
        "explicit proof",
        "journeys/explicit.yaml",
        "echo proof-ok",
    );
    let exercises = fixture
        .store
        .add_edge(
            EdgeKind::Exercises,
            &validation_id,
            &fixture.test_file_id,
            TruthClass::Asserted,
        )
        .unwrap();
    fixture
        .store
        .set_facet(
            &exercises.id,
            TargetKind::Edge,
            "locator",
            "exercises_behavior",
            TruthClass::Asserted,
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let before = witness(&fixture.store, &validation_id);
    // Locator-bound exercises is the product's validation-specific entry:
    // with a call witness to the realizing symbol it earns S3 (module docs).
    // Bare-file exercises remain ineligible (see bare_exercises test).
    assert_eq!(before.grade, "S3");
    assert_eq!(
        before.call_evidence.as_ref().map(|e| e.source.as_str()),
        Some("validation_grounding")
    );
    assert!(before
        .call_evidence
        .as_ref()
        .map(|e| e.s3_eligible)
        .unwrap_or(false));

    std::fs::write(
        fixture.tmp.path().join("tests/behavior_test.rs"),
        "#[test]\nfn exercises_behavior() { let _ = perform_behavior(); let changed = true; assert!(changed); }\n",
    )
    .unwrap();
    let report = loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    assert!(report.validations_reset >= 1);
    assert_eq!(
        fixture
            .store
            .get_node(&validation_id)
            .unwrap()
            .unwrap()
            .status,
        "not_run"
    );
    assert_eq!(witness(&fixture.store, &validation_id).grade, "S0");
}

#[test]
fn cargo_options_do_not_become_test_filters_and_compound_commands_fail_closed() {
    let fixture = fixture();
    let mapped = add_journey_validation(
        &fixture,
        "package-selected proof",
        "journeys/package-selected.yaml",
        "cargo test --test behavior_test --color=never",
    );
    let mapped_equals = add_journey_validation(
        &fixture,
        "equals-selected proof",
        "journeys/equals-selected.yaml",
        "cargo test --color=never --test=behavior_test",
    );
    let no_run = add_journey_validation(
        &fixture,
        "compile-only proof",
        "journeys/compile-only.yaml",
        "cargo test --no-run --test behavior_test",
    );
    let listed = add_journey_validation(
        &fixture,
        "list-only proof",
        "journeys/list-only.yaml",
        "cargo test --test behavior_test -- --list",
    );
    for (name, artifact, command) in [
        (
            "target-selected proof",
            "journeys/target-selected.yaml",
            "cargo test --target wasm32-unknown-unknown --test behavior_test",
        ),
        (
            "package-selected proof",
            "journeys/package-selected-closed.yaml",
            "cargo test --package loom --test behavior_test",
        ),
        (
            "manifest-selected proof",
            "journeys/manifest-selected.yaml",
            "cargo test --manifest-path Cargo.toml --test behavior_test",
        ),
        ("doc proof", "journeys/doc.yaml", "cargo test --doc"),
        (
            "scoped cargo run proof",
            "journeys/scoped-cargo-run.yaml",
            "cargo run --package loom --bin loom",
        ),
        (
            "skip proof",
            "journeys/skip.yaml",
            "cargo test --test behavior_test -- --skip exercises_behavior",
        ),
    ] {
        let id = add_journey_validation(&fixture, name, artifact, command);
        loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
        assert_eq!(witness(&fixture.store, &id).grade, "S2", "{command}");
    }
    let compound = add_journey_validation(
        &fixture,
        "short-circuit proof",
        "journeys/short-circuit.yaml",
        "printf proof-ok || cargo test --test behavior_test",
    );
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    assert_eq!(witness(&fixture.store, &mapped).grade, "S3");
    assert_eq!(witness(&fixture.store, &mapped_equals).grade, "S3");
    assert_eq!(witness(&fixture.store, &no_run).grade, "S2");
    assert_eq!(witness(&fixture.store, &listed).grade, "S2");
    assert_eq!(witness(&fixture.store, &compound).grade, "S2");
}

#[test]
fn loom_cli_subcommands_are_argument_sensitive_and_flag_only_invocations_fail_closed() {
    let fixture = fixture();
    // Register a second codefile holding a real "handler" symbol so the
    // loom-command mapping has something to resolve.
    fixture
        .store
        .add_node(
            NodeType::CodeFile,
            "src/commands/status_cmd.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    std::fs::create_dir_all(fixture.tmp.path().join("src/commands")).unwrap();
    std::fs::write(
        fixture.tmp.path().join("src/commands/status_cmd.rs"),
        "pub(crate) fn sync_cmd() {}\n",
    )
    .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let file = graph
        .files()
        .find(|file| file.ends_with("status_cmd.rs"))
        .expect("sync_cmd file indexed");

    let mapped =
        loom::proofstrength::command_entries("./loom sync --json", &graph, "journey_command");
    assert!(
        mapped.iter().any(|entry| {
            entry.file == file && entry.entry_symbol.as_deref() == Some("sync_cmd")
        }),
        "loom sync must map to the sync_cmd handler symbol"
    );
    assert!(
        mapped
            .iter()
            .all(|entry| entry.entry_symbol.as_deref() != Some("main")),
        "loom sync must never map to main"
    );

    for flag_only in [
        "loom --help",
        "loom",
        "loom --version",
        "loom -h",
        "loom -V",
        "./loom --help sync",
        "./loom --version sync",
    ] {
        let entries = loom::proofstrength::command_entries(flag_only, &graph, "journey_command");
        assert!(
            entries.is_empty(),
            "flag-only or help invocations must fail closed: {flag_only}"
        );
    }
    let unknown = loom::proofstrength::command_entries(
        "./loom definitely-not-a-command",
        &graph,
        "journey_command",
    );
    assert!(unknown.is_empty(), "unknown subcommands must fail closed");

    // Dispatcher-level subcommands map to a shared `dispatch` symbol that is
    // not unique evidence of which handler runs; they fail closed and rely on
    // explicit exercises evidence or validation_command instead.
    for dispatcher in [
        "./loom edge",
        "loom edge list",
        "./loom intent",
        "./loom journey",
    ] {
        let entries = loom::proofstrength::command_entries(dispatcher, &graph, "journey_command");
        assert!(
            entries.is_empty(),
            "dispatcher-level subcommands must fail closed: {dispatcher}"
        );
    }

    // Value-bearing global options must be consumed, never mistaken for the
    // subcommand: `loom --graph <path> sync status` executes `status`.
    let with_global = loom::proofstrength::command_entries(
        "./loom --graph some/path status --json",
        &graph,
        "journey_command",
    );
    assert!(
        with_global
            .iter()
            .all(|entry| entry.entry_symbol.as_deref() != Some("sync_cmd")),
        "a global option value must never be taken as the subcommand"
    );
}

#[test]
fn script_paths_without_an_obvious_entry_symbol_fail_closed() {
    let fixture = fixture();
    std::fs::create_dir_all(fixture.tmp.path().join("tools")).unwrap();
    std::fs::write(
        fixture.tmp.path().join("tools/aux.py"),
        "def helper():\n    pass\n",
    )
    .unwrap();
    fixture
        .store
        .add_node(
            NodeType::CodeFile,
            "tools/aux.py",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let entries =
        loom::proofstrength::command_entries("python3 tools/aux.py", &graph, "journey_command");
    assert!(
        entries.is_empty(),
        "a script with no main/run/handler symbol must not earn entry evidence"
    );

    // Substring look-alikes must not manufacture an entry: `dry_run_check`
    // contains `run`, but it is not the script's entry point.
    std::fs::write(
        fixture.tmp.path().join("tools/lookalike.py"),
        "def dry_run_check():\n    pass\n",
    )
    .unwrap();
    fixture
        .store
        .add_node(
            NodeType::CodeFile,
            "tools/lookalike.py",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let lookalike = loom::proofstrength::command_entries(
        "python3 tools/lookalike.py",
        &graph,
        "journey_command",
    );
    assert!(
        lookalike.is_empty(),
        "a symbol merely containing 'run' must not be treated as the entry"
    );

    // A bare filename must not credit an ambiguous match: two registered
    // files named aux.py means `python3 aux.py` cannot know which one runs.
    // Register globs so sync rescans and extracts the new files.
    fixture
        .store
        .set_meta("codefile_globs", "[\"src/**\", \"tests/**\", \"other/**\"]")
        .unwrap();
    std::fs::write(
        fixture.tmp.path().join("src/aux.py"),
        "def main():\n    pass\n",
    )
    .unwrap();
    fixture
        .store
        .add_node(
            NodeType::CodeFile,
            "src/aux.py",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let ambiguous =
        loom::proofstrength::command_entries("python3 aux.py", &graph, "journey_command");
    assert!(
        ambiguous.is_empty(),
        "a bare name matching several files must fail closed"
    );
    // An explicit repo-relative path resolving to exactly one file may still
    // map when that file has an exact entry symbol AND the entry is genuinely
    // invoked — a bare definition that nothing calls must fail closed (it
    // never runs, so it cannot be the executed surface).
    std::fs::write(
        fixture.tmp.path().join("src/run.py"),
        "def main():\n    pass\n",
    )
    .unwrap();
    fixture
        .store
        .add_node(
            NodeType::CodeFile,
            "src/run.py",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let uncalled =
        loom::proofstrength::command_entries("python3 src/run.py", &graph, "journey_command");
    assert!(
        uncalled.is_empty(),
        "a main that nothing invokes must not map to an executed entry"
    );
    // Once the entry is actually called (the script's `__main__` guard calls
    // it), the explicit path maps to the exact invoked symbol.
    std::fs::write(
        fixture.tmp.path().join("src/run.py"),
        "def main():\n    pass\n\nif __name__ == '__main__':\n    main()\n",
    )
    .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let explicit =
        loom::proofstrength::command_entries("python3 src/run.py", &graph, "journey_command");
    assert_eq!(
        explicit
            .iter()
            .map(|entry| entry.entry_symbol.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("main")],
        "an unambiguous explicit path with an invoked exact main maps to it"
    );
    // A call from a dead function is not execution: `dead() -> main()` with no
    // top-level invocation means the script never runs `main`, so the entry
    // must fail closed.
    std::fs::write(
        fixture.tmp.path().join("src/run.py"),
        "def main():\n    pass\n\ndef dead():\n    main()\n",
    )
    .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let dead_call =
        loom::proofstrength::command_entries("python3 src/run.py", &graph, "journey_command");
    assert!(
        dead_call.is_empty(),
        "an entry invoked only by a dead function is not an executed surface"
    );
}

#[test]
fn binary_main_requires_exact_symbol_not_substring_lookalike() {
    let fixture = fixture();
    std::fs::create_dir_all(fixture.tmp.path().join("src/bin")).unwrap();
    std::fs::write(
        fixture.tmp.path().join("src/bin/svc.rs"),
        "fn main_helper() {}\n",
    )
    .unwrap();
    fixture
        .store
        .add_node(
            NodeType::CodeFile,
            "src/bin/svc.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let entries =
        loom::proofstrength::command_entries("cargo run --bin svc", &graph, "journey_command");
    assert!(
        entries.is_empty(),
        "a binary whose only symbol is main_helper must not earn a main entry"
    );
}

#[test]
fn every_loom_subcommand_handler_maps_to_a_real_unique_cli_symbol() {
    // Hermetic fixture: copy the repository's Rust sources into a fresh temp
    // root, register them with sync, and build the call graph there — never
    // from the checkout's mutable local `.loom` database.
    let tmp = Tmp::new();
    let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut pending = vec![src_root.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let rel = path
                .strip_prefix(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let dest = tmp.path().join(&rel);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::copy(&path, &dest).unwrap();
        }
    }
    let store = Store::init(tmp.path(), Some("loom"), false).unwrap();
    // sync extracts only registered codefiles; register the copied sources.
    let mut pending = vec![src_root.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let rel = path
                .strip_prefix(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
                .unwrap()
                .to_string_lossy()
                .into_owned();
            store
                .add_node(NodeType::CodeFile, &rel, "", "", serde_json::json!({}))
                .unwrap();
        }
    }
    loom::sync::run(&store, tmp.path()).unwrap();
    let graph = loom::callgraph::build(&store).unwrap();
    // Every mapped argv must land on the exact file+symbol selected by the
    // typed CLI and commands::run. This includes parameterized and nested
    // journey steps; a shared symbol name in some other module is not enough.
    for (command, file, handler) in [
        ("./loom sync", "src/commands/status_cmd.rs", "sync_cmd"),
        ("./loom status", "src/commands/status_cmd.rs", "status"),
        ("./loom next", "src/commands/status_cmd.rs", "next_cmd"),
        (
            "./loom next --mode ratify --json",
            "src/commands/status_cmd.rs",
            "next_cmd",
        ),
        (
            "./loom next --mode ratify --all",
            "src/commands/status_cmd.rs",
            "queue_list",
        ),
        (
            "./loom next --all --full --json",
            "src/commands/status_cmd.rs",
            "next_all",
        ),
        ("./loom welcome", "src/commands/orient_cmd.rs", "welcome"),
        ("./loom guide", "src/commands/orient_cmd.rs", "guide"),
        (
            "./loom coverage",
            "src/commands/diagnostics_cmd.rs",
            "coverage_cmd",
        ),
        (
            "./loom impact door --depth 2 --json",
            "src/commands/diagnostics_cmd.rs",
            "impact_cmd",
        ),
        (
            "./loom explain behavior",
            "src/commands/discover_cmd.rs",
            "explain_cmd",
        ),
        (
            "./loom find 'falsifiable graph' --limit 5 --exact --json",
            "src/commands/discover_cmd.rs",
            "find_cmd",
        ),
        (
            "./loom audit",
            "src/commands/diagnostics_cmd.rs",
            "audit_cmd",
        ),
        (
            "./loom deepen --limit 3",
            "src/commands/diagnostics_cmd.rs",
            "deepen_cmd",
        ),
        ("./loom export", "src/commands/status_cmd.rs", "export"),
        (
            "./loom whoami",
            "src/commands/diagnostics_cmd.rs",
            "whoami_cmd",
        ),
        (
            "./loom smells",
            "src/commands/diagnostics_cmd.rs",
            "smells_cmd",
        ),
        (
            "./loom doctor",
            "src/commands/diagnostics_cmd.rs",
            "doctor_cmd",
        ),
        (
            "./loom observe -- true",
            "src/commands/proof_cmd.rs",
            "observe_cmd",
        ),
        (
            "./loom decide chosen --instead-of rejected --because reason",
            "src/commands/capture_cmd.rs",
            "decide_cmd",
        ),
        (
            "./loom door 'ship a faster flow' --json",
            "src/commands/capture_cmd.rs",
            "door",
        ),
        (
            "./loom codefile list --limit 5 --offset 2 --json",
            "src/commands/codefile_cmd.rs",
            "dispatch",
        ),
        (
            "./loom inbox mark abc routed --reason checked",
            "src/commands/capture_cmd.rs",
            "inbox",
        ),
        (
            "./loom inbox remove abc",
            "src/commands/capture_cmd.rs",
            "inbox",
        ),
    ] {
        assert!(
            graph.file_defines(file, handler),
            "mapped destination {file}:{handler} must exist"
        );
        let entries = loom::proofstrength::command_entries(command, &graph, "journey_command");
        assert!(
            entries.iter().any(|entry| {
                entry.entry_symbol.as_deref() == Some(handler) && entry.file == file
            }),
            "{command} must map to real handler {file}:{handler}; got {entries:?}"
        );
    }
    // Unsupported families and every invalid typed/semantic invocation fail
    // closed. In particular, Clap-valid `next --full` is rejected by
    // commands::run and therefore cannot earn entry credit here.
    for closed in [
        "./loom edge",
        "./loom debt",
        "./loom intent",
        "./loom journey",
        "./loom prove",
        "./loom validation",
        "./loom finding",
        "./loom question",
        "./loom inbox",
        "./loom note",
        "./loom task",
        "./loom rule",
        "./loom scan",
        "./loom ignore",
        "./loom apply",
        "./loom door",
        "./loom observe",
        "./loom decide",
        "./loom next --mode definitely-not-a-mode",
        "./loom next --full",
        "./loom next --all --full",
        "./loom impact",
        "./loom impact door extra",
        "./loom deepen --limit nope",
        "./loom codefile list --limit nope",
        "./loom codefile list extra",
        "./loom inbox mark abc",
        "./loom inbox remove",
        "./loom find query extra",
        "./loom status --help",
        "./loom sync extra",
        "cargo run --bin svc -- --help",
    ] {
        assert!(
            loom::proofstrength::command_entries(closed, &graph, "journey_command").is_empty(),
            "{closed} must fail closed"
        );
    }
}

#[test]
fn legacy_intent_wide_grade_is_demoted_and_journaled_once() {
    let fixture = fixture();
    let validation_id = add_journey_validation(
        &fixture,
        "legacy echo proof",
        "journeys/legacy.yaml",
        "echo proof-ok",
    );
    let legacy = serde_json::json!({
        "grade": "S3",
        "ran_and_passed": true,
        "content_assertions": 1,
        "call_witness": "perform_behavior",
        "baseline_clean": false,
        "boundary": null,
        "next": ""
    });
    fixture
        .store
        .set_facet(
            &validation_id,
            TargetKind::Node,
            "proof_strength",
            &legacy.to_string(),
            TruthClass::Derived,
        )
        .unwrap();

    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let migrated = witness(&fixture.store, &validation_id);
    assert_eq!(migrated.witness_model, STRENGTH_WITNESS_MODEL);
    assert_eq!(migrated.grade, "S2");
    let changes: Vec<_> = loom::journal::read(fixture.tmp.path())
        .unwrap()
        .into_iter()
        .filter(|entry| entry.event == "proof_strength_changed" && entry.target_id == validation_id)
        .collect();
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0].payload["reason"],
        "witness_model_change: intent-wide → validation-specific"
    );

    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let repeated = loom::journal::read(fixture.tmp.path())
        .unwrap()
        .into_iter()
        .filter(|entry| entry.event == "proof_strength_changed" && entry.target_id == validation_id)
        .count();
    assert_eq!(
        repeated, 1,
        "an unchanged sync must not repeat migration history"
    );
}

#[test]
fn bare_exercises_file_without_locator_cannot_earn_s3() {
    let fixture = fixture();
    let validation_id = add_journey_validation(
        &fixture,
        "bare exercises proof",
        "journeys/bare-exercises.yaml",
        "echo proof-ok",
    );
    fixture
        .store
        .add_edge(
            EdgeKind::Exercises,
            &validation_id,
            &fixture.test_file_id,
            TruthClass::Asserted,
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let w = witness(&fixture.store, &validation_id);
    assert_eq!(
        w.grade, "S2",
        "locator-free exercises is provenance, not an entry: {w:?}"
    );
    assert_eq!(w.call_witness, None);
    if let Some(evidence) = &w.call_evidence {
        assert!(
            !evidence.s3_eligible,
            "bare file claim must not be S3-eligible"
        );
    }
}

#[test]
fn anchor_bound_exercises_is_navigation_only_and_cannot_earn_s3() {
    let fixture = fixture();
    std::fs::write(
        fixture.tmp.path().join("tests/behavior_test.rs"),
        "#[test]\n// loom:anchor proof.behavior-entry\nfn exercises_behavior() { let _ = perform_behavior(); }\n#[test]\nfn another_entry() { let _ = perform_behavior(); }\n",
    )
    .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let validation_id = add_journey_validation(
        &fixture,
        "anchor exercises proof",
        "journeys/anchor-exercises.yaml",
        "echo proof-ok",
    );
    let edge = fixture
        .store
        .add_edge(
            EdgeKind::Exercises,
            &validation_id,
            &fixture.test_file_id,
            TruthClass::Asserted,
        )
        .unwrap();
    fixture
        .store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "locator",
            "anchor:proof.behavior-entry",
            TruthClass::Asserted,
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();

    let witness = witness(&fixture.store, &validation_id);
    assert_eq!(
        witness.grade, "S2",
        "anchor must not manufacture S3: {witness:?}"
    );
    assert_eq!(witness.call_witness, None);
    let evidence = witness
        .call_evidence
        .expect("visible navigation provenance");
    assert_eq!(evidence.source, "anchor_navigation");
    assert_eq!(evidence.entry_symbol, None);
    assert!(!evidence.s3_eligible);
}

#[test]
fn quoted_cargo_filter_is_one_token_and_unsupported_shell_fails_closed() {
    let fixture = fixture();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    // Quoted multi-word filter must stay one argv token; whitespace-split would
    // invent a filter that never ran.
    let quoted = loom::proofstrength::command_entries(
        "cargo test --test behavior_test -- \"foo bar\"",
        &graph,
        "journey_command",
    );
    assert!(
        quoted.is_empty(),
        "quoted filter must not split into bare tokens that match symbols: {quoted:?}"
    );
    for bad in [
        "cargo test --test behavior_test > /tmp/out",
        "cargo test --test $(echo behavior_test)",
        "FOO=`id` cargo test --test behavior_test",
        "cargo test --test \"unterminated",
    ] {
        let entries = loom::proofstrength::command_entries(bad, &graph, "journey_command");
        assert!(
            entries.is_empty(),
            "unsupported shell syntax must fail closed: {bad} -> {entries:?}"
        );
    }
}

#[test]
fn loom_handler_mapping_requires_a_unique_defining_file() {
    let fixture = fixture();
    // Fixture graph has no real loom CLI handlers, so loom sync must fail closed
    // rather than inventing an entry from a non-unique or missing definition.
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let entries =
        loom::proofstrength::command_entries("./loom sync --json", &graph, "journey_command");
    assert!(
        entries.is_empty(),
        "fixture has no unique sync_cmd definition, so loom sync must fail closed: {entries:?}"
    );
}

#[test]
fn path_loom_binary_is_not_mapped_to_project_handlers() {
    let fixture = fixture();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    for cmd in [
        "./fixtures/loom sync",
        "/tmp/loom status",
        "bin/loom sync --json",
    ] {
        let entries = loom::proofstrength::command_entries(cmd, &graph, "journey_command");
        assert!(
            entries.is_empty(),
            "path-qualified loom binary must not map to project handlers: {cmd} -> {entries:?}"
        );
    }
}

#[test]
fn double_quoted_non_escape_backslash_is_preserved() {
    let fixture = fixture();
    // A filter that keeps the backslash must not collapse into a live symbol.
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let entries = loom::proofstrength::command_entries(
        "cargo test --test behavior_test -- \"exercises\\_behavior\"",
        &graph,
        "journey_command",
    );
    assert!(
        entries.is_empty(),
        "preserved backslash filter must not match a live symbol: {entries:?}"
    );
}

#[test]
fn cargo_filter_matches_harness_tests_not_helpers() {
    let fixture = fixture();
    // behavior_test defines exercises_behavior (#[test]) which calls perform_behavior.
    // A filter that only names a non-test helper must not invent an entry.
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let helper_only = loom::proofstrength::command_entries(
        "cargo test --test behavior_test -- perform_behavior",
        &graph,
        "journey_command",
    );
    assert!(
        helper_only.is_empty(),
        "helper-name filter must not select a harness entry: {helper_only:?}"
    );
}

#[test]
fn unknown_loom_flags_fail_closed() {
    let fixture = fixture();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    for cmd in ["loom --bogus sync", "loom sync --bogus", "loom --graph"] {
        let entries = loom::proofstrength::command_entries(cmd, &graph, "journey_command");
        assert!(
            entries.is_empty(),
            "unknown/incomplete loom argv must fail closed: {cmd} -> {entries:?}"
        );
    }
}

#[test]
fn empty_quoted_and_invalid_env_assignment_fail_closed() {
    let fixture = fixture();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    for cmd in [
        "cargo test --test \"\"",
        "1=x cargo test --test behavior_test",
    ] {
        let entries = loom::proofstrength::command_entries(cmd, &graph, "journey_command");
        assert!(
            entries.is_empty(),
            "empty argv / invalid env prefix must fail closed: {cmd} -> {entries:?}"
        );
    }
}

#[test]
fn unrecognized_cargo_options_fail_closed() {
    let fixture = fixture();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    for cmd in [
        "cargo test --test behavior_test --timings",
        "cargo test --test behavior_test --offline",
        "cargo test --test behavior_test --frozen",
    ] {
        let entries = loom::proofstrength::command_entries(cmd, &graph, "journey_command");
        assert!(
            entries.is_empty(),
            "unmodeled cargo option must fail closed: {cmd} -> {entries:?}"
        );
    }
}

#[test]
fn subcommand_local_loom_flags_fail_closed() {
    let fixture = fixture();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    for cmd in [
        "loom sync --name anything",
        "loom status --force",
        "loom sync --force",
    ] {
        let entries = loom::proofstrength::command_entries(cmd, &graph, "journey_command");
        assert!(
            entries.is_empty(),
            "subcommand-local flags must not map via global allowlist: {cmd} -> {entries:?}"
        );
    }
}

#[test]
fn malformed_cargo_option_values_fail_closed() {
    let fixture = fixture();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    for cmd in [
        "cargo test --test behavior_test --color",
        "cargo test --test behavior_test --color=garbage",
        "cargo run --bin svc --bogus",
        "cargo run --bin",
    ] {
        let entries = loom::proofstrength::command_entries(cmd, &graph, "journey_command");
        assert!(
            entries.is_empty(),
            "malformed/unknown cargo option must fail closed: {cmd} -> {entries:?}"
        );
    }
}

#[test]
fn loom_boolean_equals_and_non_cli_globals_fail_closed() {
    let fixture = fixture();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    for cmd in [
        "loom --json=true status",
        "loom status --quiet",
        "loom --color never status",
        "loom --config x status",
    ] {
        let entries = loom::proofstrength::command_entries(cmd, &graph, "journey_command");
        assert!(
            entries.is_empty(),
            "non-Cli globals / boolean=value must fail closed: {cmd} -> {entries:?}"
        );
    }
}

#[test]
fn extra_cargo_positionals_and_invalid_jobs_fail_closed() {
    let fixture = fixture();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    for cmd in [
        "cargo test --test behavior_test exercises_behavior extra",
        "cargo test --test behavior_test -j garbage",
    ] {
        let entries = loom::proofstrength::command_entries(cmd, &graph, "journey_command");
        assert!(
            entries.is_empty(),
            "extra positional / invalid jobs must fail closed: {cmd} -> {entries:?}"
        );
    }
}

#[test]
fn loom_extra_positionals_fail_closed() {
    let fixture = fixture();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    for cmd in ["./loom sync extra", "./loom validation"] {
        let entries = loom::proofstrength::command_entries(cmd, &graph, "journey_command");
        assert!(
            entries.is_empty(),
            "extra/incomplete loom argv must fail closed: {cmd} -> {entries:?}"
        );
    }
}

#[test]
fn ignored_and_cfg_gated_tests_are_not_harness_entries() {
    let fixture = fixture();
    std::fs::write(
        fixture.tmp.path().join("tests/gated.rs"),
        r#"
#[test]
#[ignore]
fn ignored_test() { let _ = crate_under_test::perform_behavior(); }

#[cfg(feature = "never")]
#[test]
fn cfg_gated() { let _ = crate_under_test::perform_behavior(); }

#[test]
fn live_test() { let _ = crate_under_test::perform_behavior(); }
"#,
    )
    .unwrap();
    fixture
        .store
        .add_node(
            NodeType::CodeFile,
            "tests/gated.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let tests = graph.file_test_symbols("tests/gated.rs");
    assert_eq!(
        tests,
        ["live_test"].into_iter().map(String::from).collect(),
        "ignored/cfg-gated tests must not be harness entries: {tests:?}"
    );
}

#[test]
fn bare_script_basename_and_bare_binary_fail_closed() {
    let fixture = fixture();
    std::fs::create_dir_all(fixture.tmp.path().join("tools")).unwrap();
    std::fs::write(
        fixture.tmp.path().join("tools/check.py"),
        "def main():
    return 0

if __name__ == '__main__':
    main()
",
    )
    .unwrap();
    fixture
        .store
        .add_node(
            NodeType::CodeFile,
            "tools/check.py",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    assert!(
        loom::proofstrength::command_entries("python3 check.py", &graph, "journey_command")
            .is_empty(),
        "bare basename must fail closed"
    );
    assert!(
        loom::proofstrength::command_entries("svc", &graph, "journey_command").is_empty(),
        "bare binary must fail closed"
    );
    // Explicit repo-relative path still maps when file-scope main exists.
    let explicit =
        loom::proofstrength::command_entries("python3 tools/check.py", &graph, "journey_command");
    assert!(
        explicit
            .iter()
            .any(|e| e.entry_symbol.as_deref() == Some("main")),
        "repo-relative script with file-scope main must map: {explicit:?}"
    );
}

#[test]
fn unscoped_cargo_test_without_target_fails_closed() {
    let fixture = fixture();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let entries = loom::proofstrength::command_entries("cargo test", &graph, "journey_command");
    assert!(
        entries.is_empty(),
        "unscoped cargo test must not invent all tests/* entries: {entries:?}"
    );
}

#[test]
fn traversal_and_nested_test_paths_fail_closed() {
    let fixture = fixture();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    for cmd in [
        "target/../../tmp/loom sync",
        "cargo test --test nested/proof",
        "cargo test --test ../escape",
    ] {
        let entries = loom::proofstrength::command_entries(cmd, &graph, "journey_command");
        assert!(
            entries.is_empty(),
            "traversal/nested target must fail closed: {cmd} -> {entries:?}"
        );
    }
}

#[test]
fn unknown_test_macro_and_cfg_attr_ignore_are_not_harness_entries() {
    let fixture = fixture();
    std::fs::write(
        fixture.tmp.path().join("tests/macros.rs"),
        r#"
#[noop::test]
fn fake_macro() { let _ = crate_under_test::perform_behavior(); }

#[cfg_attr(test, ignore)]
#[test]
fn cfg_attr_ignored() { let _ = crate_under_test::perform_behavior(); }

#[tokio::test]
async fn real_tokio() { let _ = crate_under_test::perform_behavior(); }

#[test]
fn live() { let _ = crate_under_test::perform_behavior(); }
"#,
    )
    .unwrap();
    fixture
        .store
        .add_node(
            NodeType::CodeFile,
            "tests/macros.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let tests = graph.file_test_symbols("tests/macros.rs");
    assert_eq!(
        tests,
        ["live", "real_tokio"]
            .into_iter()
            .map(String::from)
            .collect(),
        "unknown macros and cfg_attr ignore must not be harness entries: {tests:?}"
    );
}

#[test]
fn cargo_run_requires_unique_binary_candidate() {
    let fixture = fixture();
    std::fs::create_dir_all(fixture.tmp.path().join("src/bin")).unwrap();
    std::fs::create_dir_all(fixture.tmp.path().join("pkg/src/bin")).unwrap();
    std::fs::write(fixture.tmp.path().join("src/bin/svc.rs"), "fn main() {}\n").unwrap();
    std::fs::write(
        fixture.tmp.path().join("pkg/src/bin/svc.rs"),
        "fn main() {}\n",
    )
    .unwrap();
    for path in ["src/bin/svc.rs", "pkg/src/bin/svc.rs"] {
        fixture
            .store
            .add_node(NodeType::CodeFile, path, "", "", serde_json::json!({}))
            .unwrap();
    }
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let entries =
        loom::proofstrength::command_entries("cargo run --bin svc", &graph, "journey_command");
    assert!(
        entries.is_empty(),
        "ambiguous binary candidates must fail closed: {entries:?}"
    );
}

#[test]
fn cargo_run_bin_does_not_rewrite_hyphens_into_underscores() {
    let fixture = fixture();
    std::fs::create_dir_all(fixture.tmp.path().join("src/bin")).unwrap();
    // File uses underscores; command uses hyphens. Must not match.
    std::fs::write(
        fixture.tmp.path().join("src/bin/svc_api.rs"),
        "fn main() {}\n",
    )
    .unwrap();
    fixture
        .store
        .add_node(
            NodeType::CodeFile,
            "src/bin/svc_api.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    loom::sync::run(&fixture.store, fixture.tmp.path()).unwrap();
    let graph = loom::callgraph::build(&fixture.store).unwrap();
    let entries =
        loom::proofstrength::command_entries("cargo run --bin svc-api", &graph, "journey_command");
    assert!(
        entries.is_empty(),
        "hyphenated cargo bin must not match underscored source: {entries:?}"
    );
}
