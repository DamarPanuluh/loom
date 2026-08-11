//! Ring 53 — detached, structured, rehearsal-only release verification.

use clap::Parser;
use loom::cli::{Cli, Command, ReleaseCmd, ReleasePhaseArg};
use loom::model::{Node, NodeType, TruthClass};
use loom::release::{
    CommandObservation, OuterJourneyAttestation, ReleaseExecutor, ReleaseRehearsalReport,
    ReleaseStatus, OUTER_COMPILER_VERSION_ENV, OUTER_CONTEXT_CAPSULE_ENV, OUTER_JOURNEY_HASH_ENV,
    OUTER_JOURNEY_ID_ENV, OUTER_JOURNEY_PROFILE_ENV, OUTER_JOURNEY_RUN_ID_ENV,
    OUTER_PROOF_HASH_ENV, OUTER_SURFACE_HASH_ENV, RELEASE_REHEARSAL_SCHEMA,
};
use loom::travel::{Export, FORMAT};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Output};
use std::sync::Mutex;

mod common;
use common::Tmp;

static RELEASE_ENV: Mutex<()> = Mutex::new(());
const RELEASE_INVENTORY_MANIFEST_HASH: &str = "1e7f61e0ec084423";
const RELEASE_INVENTORY_ENTRY_COUNT: usize = 261;
const RELEASE_INVENTORY_FILE_COUNT: usize = 257;
const RELEASE_INVENTORY_TOMBSTONE_COUNT: usize = 4;

#[test]
fn release_cli_has_only_three_typed_rehearsal_phases_and_no_skip() {
    for (name, expected) in [
        ("isolated-dogfood", ReleasePhaseArg::IsolatedDogfood),
        ("fresh-fixpoint", ReleasePhaseArg::FreshFixpoint),
        ("gated-preparation", ReleasePhaseArg::GatedPreparation),
    ] {
        let parsed =
            Cli::try_parse_from(["loom", "release", "rehearse", "--phase", name, "--json"])
                .unwrap();
        match parsed.command.unwrap() {
            Command::Release {
                cmd: ReleaseCmd::Rehearse { phase },
            } => assert_eq!(phase, expected),
            other => panic!("unexpected command: {other:?}"),
        }
    }
    assert!(Cli::try_parse_from(["loom", "release", "rehearse", "--json"]).is_err());
    assert!(Cli::try_parse_from([
        "loom",
        "release",
        "rehearse",
        "--phase",
        "isolated-dogfood",
        "--skip",
        "another-journey",
        "--json",
    ])
    .is_err());
    match Cli::try_parse_from([
        "loom",
        "release",
        "snapshot",
        "--destination",
        "/tmp/candidate",
        "--json",
    ])
    .unwrap()
    .command
    .unwrap()
    {
        Command::Release {
            cmd: ReleaseCmd::Snapshot { destination },
        } => assert_eq!(destination, Path::new("/tmp/candidate")),
        other => panic!("unexpected command: {other:?}"),
    }
    match Cli::try_parse_from([
        "loom",
        "release",
        "authorize-derivations",
        "--manifest-dir",
        "/tmp/reviewed",
        "--human-decision",
        "I approve the exact reviewed batch.",
        "--json",
    ])
    .unwrap()
    .command
    .unwrap()
    {
        Command::Release {
            cmd:
                ReleaseCmd::AuthorizeDerivations {
                    manifest_dir,
                    human_decision,
                },
        } => {
            assert_eq!(manifest_dir, Path::new("/tmp/reviewed"));
            assert_eq!(human_decision, "I approve the exact reviewed batch.");
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn rehearsal_without_runtime_owned_outer_context_blocks_without_mutation() {
    let root = fixture_root("missing-context");
    let before = snapshot(root.path());
    let output = release_command(root.path(), "isolated-dogfood", false);
    assert!(!output.status.success());
    let report = report(&output);
    assert_eq!(report.schema, RELEASE_REHEARSAL_SCHEMA);
    assert_eq!(report.status, ReleaseStatus::Blocked);
    assert!(report
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("reserved outer Journey id"));
    assert!(!report.graph.outer_profile.excluded_from_nested_execution);
    assert_safe_effects(&report, false);
    assert_eq!(snapshot(root.path()), before);
}

#[test]
fn legacy_export_is_refused_before_any_candidate_command_or_live_mutation() {
    let fixture = RuntimeFixture::new("legacy-export");
    fixture.root.write(
        "loom.graph.json",
        r#"{"format":4,"schema_version":11,"graph_id":"old","name":"legacy","observed":false,"nodes":[],"edges":[],"facets":[],"tags":[]}"#,
    );
    let before = snapshot(fixture.root.path());
    let _environment = fixture.activate();
    let mut executor = FakeExecutor::passing(&fixture);
    let report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut executor,
    )
    .unwrap();
    assert_eq!(report.status, ReleaseStatus::Blocked);
    let detail = report.detail.as_deref().unwrap_or_default();
    assert!(
        detail.contains("refuses legacy or malformed exports"),
        "{detail}"
    );
    assert!(detail.contains("schema v11"), "{detail}");
    assert!(!report.graph.legacy_imported);
    assert!(!report.graph.legacy_migrated);
    assert!(
        executor.calls.is_empty(),
        "legacy import failed after execution"
    );
    assert_safe_effects(&report, true);
    assert_eq!(snapshot(fixture.root.path()), before);
}

#[test]
fn imported_v12_argv_stays_quarantined_without_candidate_owned_manifest() {
    let fixture = RuntimeFixture::new("quarantine");
    let sentinel = fixture.root.path().join("argv-executed");
    let surface = Node {
        id: "surface-id".into(),
        node_type: NodeType::InterfaceSurface,
        name: "imported-release-cli".into(),
        description: "foreign executable contract".into(),
        status: "declared".into(),
        truth_class: TruthClass::Asserted,
        body: serde_json::json!({
            "schema": loom::journey::INTERFACE_SURFACE_SCHEMA,
            "stable_id": "imported-release-cli",
            "title": "Imported release CLI",
            "kind": "cli",
            "identity": "foreign release",
            "codefile": "src/foreign.rs",
            "locator": "foreign_release",
            "operations": [{
                "id": "execute-imported-argv",
                "summary": "must remain quarantined",
                "argv": ["python3", "-c", format!("open({:?},'w').write('bad')", sentinel)],
                "read_only": false,
                "arguments": [],
                "output": {"format":"json","assertions":[]}
            }]
        }),
        created_at: String::new(),
        updated_at: String::new(),
    };
    let export = Export {
        format: FORMAT,
        schema_version: loom::SCHEMA_VERSION,
        graph_id: "portable-v12".into(),
        name: "portable-v12".into(),
        observed: false,
        nodes: vec![
            surface,
            Node {
                id: "foreign-validation".into(),
                node_type: NodeType::Validation,
                name: "foreign validation".into(),
                description: "must remain quarantined".into(),
                status: "passed".into(),
                truth_class: TruthClass::Asserted,
                body: json!({
                    "type":"test",
                    "command":format!("python3 -c 'open({:?}, \"w\").write(\"bad\")'", sentinel)
                }),
                created_at: String::new(),
                updated_at: String::new(),
            },
        ],
        edges: vec![],
        facts: vec![],
        evidence: vec![],
        facets: vec![],
        tags: vec![],
        config: Default::default(),
        journal: vec![],
    };
    fixture
        .root
        .write("loom.graph.json", &export.to_json().unwrap());
    let before = snapshot(fixture.root.path());
    let _environment = fixture.activate();
    let mut executor = FakeExecutor::passing(&fixture);
    let report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut executor,
    )
    .unwrap();
    assert_eq!(report.status, ReleaseStatus::Blocked);
    let detail = report.detail.as_deref().unwrap_or_default();
    assert!(
        detail.contains("remain quarantined")
            || detail.contains("does not reauthorize an imported quarantined surface"),
        "{detail}"
    );
    assert!(
        !sentinel.exists(),
        "imported argv crossed the trust boundary"
    );
    assert!(executor.calls.is_empty(), "foreign argv reached executor");
    assert_safe_effects(&report, true);
    assert_eq!(snapshot(fixture.root.path()), before);
}

#[test]
fn release_surface_manifest_binds_three_distinct_structured_attestations() {
    let spec = loom::journey::parse(Path::new("journeys/release-workflow.yaml")).unwrap();
    assert_eq!(spec.semantic_hash().unwrap(), "8cd6742023f60b62");
    let manifest: loom::journey::SurfaceManifest =
        serde_json::from_value(release_surface_manifest(&spec.semantic_hash().unwrap())).unwrap();
    manifest
        .validate_for(&spec, &spec.semantic_hash().unwrap())
        .unwrap();
    assert_eq!(manifest.surface.codefile, "src/commands/release_cmd.rs");
    assert_eq!(manifest.surface.locator, "rehearse_cmd");
    let ids: Vec<&str> = manifest
        .surface
        .operations
        .iter()
        .map(|operation| operation.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "verify-isolated-dogfood",
            "verify-fresh-fixpoint",
            "prepare-gated-local-release"
        ]
    );
    assert!(manifest.surface.operations.iter().all(|operation| {
        operation.read_only
            && operation.argv.first().map(String::as_str) == Some("loom")
            && operation.argv.last().map(String::as_str) == Some("--json")
            && operation.environment == ["CARGO_HOME", "RUSTUP_HOME"]
    }));
    let persisted = loom::journey::SurfaceManifest::parse_json(Path::new(
        "journeys/surfaces/release-workflow.surface.json",
    ))
    .unwrap();
    persisted
        .validate_for(&spec, &spec.semantic_hash().unwrap())
        .unwrap();
    for operation in &persisted.surface.operations {
        assert_eq!(
            operation.environment,
            ["CARGO_HOME", "RUSTUP_HOME"],
            "{} declares only the toolchain cache homes it consumes",
            operation.id
        );
        let pointers: std::collections::BTreeSet<&str> = operation
            .output
            .assertions
            .iter()
            .map(|assertion| assertion.pointer.as_str())
            .collect();
        for required in [
            "/source_inventory/path",
            "/source_inventory/schema",
            "/source_inventory/manifest_hash",
            "/source_inventory/provenance",
            "/source_inventory/git_verification",
            "/source_inventory/entry_count",
            "/source_inventory/file_count",
            "/source_inventory/tombstone_count",
            "/source_inventory/inventory_hash",
            "/source_inventory/git_influenced_plan",
            "/source_inventory/materialized_matches",
            "/source_inventory/missing",
            "/source_inventory/unexpected",
            "/source_inventory/secret",
            "/source_inventory/symlink",
            "/source_inventory/non_regular",
            "/source_inventory/reserved",
            "/graph/outer_profile/journey_hash",
            "/graph/outer_profile/surface_hash",
            "/graph/outer_profile/compiler_version",
            "/graph/outer_profile/proof_hash",
            "/graph/outer_profile/context_binding_limit",
            "/execution_ledger",
            "/dependency_cache/unchanged",
            "/effects/top_level_install_argv_attempted",
            "/effects/top_level_commit_argv_attempted",
            "/effects/top_level_push_argv_attempted",
        ] {
            assert!(
                pointers.contains(required),
                "{} misses {required}",
                operation.id
            );
        }
        let expected_ledger = match operation.id.as_str() {
            "verify-isolated-dogfood" => {
                vec![("/execution_ledger/0/source", json!("candidate_file_plan"))]
            }
            "verify-fresh-fixpoint" | "prepare-gated-local-release" => vec![
                (
                    "/execution_ledger/0/source",
                    json!("empty_workspace_probe:nonempty"),
                ),
                ("/execution_ledger/0/outcome", json!("rejected")),
                (
                    "/execution_ledger/1/source",
                    json!("empty_workspace_probe:preinitialized"),
                ),
                ("/execution_ledger/1/outcome", json!("rejected")),
                ("/execution_ledger/2/source", json!("candidate_file_plan")),
            ],
            other => panic!("unexpected release operation {other}"),
        };
        for (pointer, expected) in expected_ledger {
            let assertion = operation
                .output
                .assertions
                .iter()
                .find(|assertion| assertion.pointer == pointer)
                .unwrap_or_else(|| panic!("{} misses {pointer}", operation.id));
            assert_eq!(
                assertion.equals,
                Some(expected),
                "{} {pointer}",
                operation.id
            );
        }
    }
}

#[test]
fn compass_fixture_targets_a_portable_derivation_identity_not_a_graph_local_uuid() {
    let manifest = loom::journey::SurfaceManifest::parse_json(Path::new(
        "journeys/surfaces/compass-projection.surface.json",
    ))
    .unwrap();
    let setup = manifest.setup.as_ref().unwrap();
    let operation = manifest
        .surface
        .operations
        .iter()
        .find(|operation| operation.id == setup.operations[0])
        .unwrap();
    assert_eq!(
        operation.argv[3],
        "rung, lane, and queue state remain one invariant"
    );
    let assertion = &operation.output.assertions[0];
    assert_eq!(assertion.pointer, "/intent/name");
    assert_eq!(
        assertion.equals,
        Some(json!("rung, lane, and queue state remain one invariant"))
    );
    assert!(!operation
        .argv
        .iter()
        .any(|part| { part.len() == 32 && part.bytes().all(|byte| byte.is_ascii_hexdigit()) }));
}

#[test]
fn divergence_queue_fixture_verifies_then_drifts_a_named_ratified_intent() {
    let manifest = loom::journey::SurfaceManifest::parse_json(Path::new(
        "journeys/surfaces/divergence-queue.surface.json",
    ))
    .unwrap();
    let setup = manifest.setup.as_ref().unwrap();
    assert!(matches!(
        setup.graph,
        loom::journey::SetupGraph::LocalSnapshot
    ));
    assert_eq!(
        setup.operations,
        vec!["verify-fixture-ratification", "create-meaning-drift"]
    );
    let fixture_name = "dispatch the authoritative proof runner";

    let verify = manifest
        .surface
        .operations
        .iter()
        .find(|operation| operation.id == "verify-fixture-ratification")
        .unwrap();
    assert_eq!(verify.argv[4], fixture_name);
    assert!(verify
        .output
        .assertions
        .iter()
        .any(|assertion| assertion.pointer == "/name"
            && assertion.equals == Some(json!(fixture_name))));
    assert!(verify
        .output
        .assertions
        .iter()
        .any(|assertion| assertion.pointer == "/ratification"
            && assertion.equals == Some(json!("ratified"))));

    let drift = manifest
        .surface
        .operations
        .iter()
        .find(|operation| operation.id == "create-meaning-drift")
        .unwrap();
    assert_eq!(drift.argv[4], fixture_name);
    assert!(drift
        .output
        .assertions
        .iter()
        .any(|assertion| assertion.pointer == "/intent/description"
            && assertion.equals
                == Some(json!("fixture: criterion changed after human ratification"))));
    assert!(drift
        .output
        .assertions
        .iter()
        .any(|assertion| assertion.pointer == "/reword" && assertion.equals == Some(json!(false))));

    let step = manifest
        .surface
        .operations
        .iter()
        .find(|operation| operation.id == "inspect-human-decision-work")
        .unwrap();
    let step_json = serde_json::to_value(step).unwrap();
    assert!(step_json["output"]["assertions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|assertion| assertion["pointer"] == "/work_item/reason"
            && assertion["contains"]
                == json!("redefined after ratification — the words changed under the yes")));

    // The fixture must travel by stable Intent NAME. A 32-hex token in argv or
    // an equal assertion is staging-graph residue: the fresh candidate mints
    // different UUIDs, so any such identity silently matches nothing.
    let is_graph_local_uuid =
        |part: &str| part.len() == 32 && part.bytes().all(|byte| byte.is_ascii_hexdigit());
    for operation in &manifest.surface.operations {
        assert!(
            !operation.argv.iter().any(|part| is_graph_local_uuid(part)),
            "operation '{}' addresses a graph-local UUID instead of a stable name",
            operation.id
        );
        for assertion in &operation.output.assertions {
            if let Some(equals) = &assertion.equals {
                if let Some(text) = equals.as_str() {
                    assert!(
                        !is_graph_local_uuid(text),
                        "assertion '{}' compares a graph-local UUID",
                        assertion.id
                    );
                }
            }
            assert!(
                !assertion.pointer.split('/').any(is_graph_local_uuid),
                "assertion '{}' indexes a graph-local UUID",
                assertion.id
            );
        }
    }
}

#[test]
fn fake_executor_reauthorizes_only_the_candidate_manifest_and_records_every_gate() {
    let fixture = RuntimeFixture::new("fake-success");
    let before = snapshot(fixture.root.path());
    let _environment = fixture.activate();
    let mut executor = FakeExecutor::passing(&fixture);
    let report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut executor,
    )
    .unwrap();
    assert_eq!(report.status, ReleaseStatus::Passed, "{report:#?}");
    assert_eq!(report.graph.imported_surfaces_quarantined, 1);
    assert_eq!(report.graph.manifests_reauthorized.len(), 1);
    assert_eq!(
        report.graph.manifests_reauthorized[0].surface_id,
        "loom-release-rehearsal"
    );
    assert_eq!(
        report
            .execution_ledger
            .iter()
            .filter(|entry| entry.policy == "outer_profile_compile_only")
            .count(),
        3
    );
    assert!(report.execution_ledger.iter().all(|entry| {
        !entry.attempted
            || (!entry.argv.iter().any(|arg| arg == "install")
                && !entry.argv.iter().any(|arg| arg == "commit")
                && !entry.argv.iter().any(|arg| arg == "push"))
    }));
    assert!(executor
        .calls
        .iter()
        .any(|call| call.windows(2).any(|pair| pair == ["journey", "run"])));
    let derive = executor
        .calls
        .iter()
        .position(|call| has_sequence(call, &["journey", "derive-accept"]))
        .expect("approved derivation was not replayed");
    let run = executor
        .calls
        .iter()
        .position(|call| has_sequence(call, &["journey", "run"]))
        .expect("candidate Journey did not run");
    assert!(
        derive < run,
        "derivation authority was replayed after execution"
    );
    assert!(executor.calls[derive]
        .windows(2)
        .any(|pair| pair == ["--human-decision", "Ring 53 fixture approval"]));
    assert_safe_effects(&report, true);
    assert_eq!(snapshot(fixture.root.path()), before);
}

#[test]
fn derivation_candidate_permit_is_one_shot_and_replay_fails_before_candidate_writes() {
    let fixture = RuntimeFixture::new("derivation-permit-replay");
    let before = snapshot(fixture.root.path());
    let _environment = fixture.activate();
    let mut first = FakeExecutor::passing(&fixture);
    let first_report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut first,
    )
    .unwrap();
    assert_eq!(
        first_report.status,
        ReleaseStatus::Passed,
        "{first_report:#?}"
    );

    let mut replay = FakeExecutor::passing(&fixture);
    let replay_report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut replay,
    )
    .unwrap();
    assert_eq!(replay_report.status, ReleaseStatus::Blocked);
    assert!(replay_report
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("already been consumed"));
    assert!(
        replay.calls.is_empty(),
        "replay reached candidate execution"
    );
    assert_eq!(snapshot(fixture.root.path()), before);
}

#[test]
fn changed_bound_derivation_manifest_fails_before_candidate_writes() {
    let fixture = RuntimeFixture::new("changed-bound-derivation");
    let mut capsule: Value =
        serde_json::from_slice(&std::fs::read(&fixture.capsule_path).unwrap()).unwrap();
    capsule["derivation_authority"]["derivations"][0]["manifest"]["proposal_rationale"] =
        json!("changed after the human approval");
    std::fs::write(
        &fixture.capsule_path,
        serde_json::to_vec_pretty(&capsule).unwrap(),
    )
    .unwrap();
    let _environment = fixture.activate();
    let mut executor = FakeExecutor::passing(&fixture);
    let report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut executor,
    )
    .unwrap();
    assert_eq!(report.status, ReleaseStatus::Blocked);
    assert!(report
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("missing, stale, or malformed"));
    assert!(executor.calls.is_empty());
}

#[test]
fn missing_declared_host_environment_leaves_parent_authority_pending() {
    let fixture = RuntimeFixture::new("missing-env-before-authority-claim");
    let spec =
        loom::journey::parse(&fixture.root.path().join("journeys/release-workflow.yaml")).unwrap();
    let store = loom::store::Store::open(fixture.root.path()).unwrap();
    let journey = store
        .resolve_node("release-workflow", Some(NodeType::Journey))
        .unwrap();
    let surface_hash = loom::journey::surface_projection_hash(&store, &journey)
        .unwrap()
        .unwrap();
    drop(store);
    let manifest = loom::journey::SurfaceManifest::parse_json(
        &fixture
            .root
            .path()
            .join("journeys/surfaces/release-workflow.surface.json"),
    )
    .unwrap();
    let proof = loom::journey_runtime::compile_surface(
        &spec,
        &surface_hash,
        "proof",
        manifest.surface.operations,
        manifest.setup.as_ref(),
        &manifest.bindings,
    )
    .unwrap();

    let authority_store = Tmp::new();
    let _lock = RELEASE_ENV
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous_store = std::env::var_os(loom::release::DERIVATION_AUTHORITY_STORE_ENV);
    let previous_token = std::env::var_os(loom::release::DERIVATION_AUTHORITY_TOKEN_ENV);
    let previous_cargo = std::env::var_os("CARGO_HOME");
    std::env::set_var(
        loom::release::DERIVATION_AUTHORITY_STORE_ENV,
        authority_store.path(),
    );
    let grant = loom::release::authorize_derivations(
        fixture.root.path(),
        fixture._review_root.path(),
        "Ring 53 missing-environment preflight approval".into(),
        "llm:builder",
    )
    .unwrap();
    std::env::set_var(loom::release::DERIVATION_AUTHORITY_TOKEN_ENV, &grant.token);
    std::env::remove_var("CARGO_HOME");

    let report =
        loom::journey_runtime::execute(fixture.root.path(), &spec, &proof, &BTreeMap::new());

    match previous_store {
        Some(value) => std::env::set_var(loom::release::DERIVATION_AUTHORITY_STORE_ENV, value),
        None => std::env::remove_var(loom::release::DERIVATION_AUTHORITY_STORE_ENV),
    }
    match previous_token {
        Some(value) => std::env::set_var(loom::release::DERIVATION_AUTHORITY_TOKEN_ENV, value),
        None => std::env::remove_var(loom::release::DERIVATION_AUTHORITY_TOKEN_ENV),
    }
    match previous_cargo {
        Some(value) => std::env::set_var("CARGO_HOME", value),
        None => std::env::remove_var("CARGO_HOME"),
    }

    assert_eq!(
        report.status,
        loom::journey_runtime::RuntimeStatus::Blocked,
        "{report:#?}"
    );
    assert!(report
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("declared operation environment variable 'CARGO_HOME' is missing"));
    assert!(authority_store
        .path()
        .join("pending")
        .join(&grant.token)
        .is_file());
    assert!(!authority_store
        .path()
        .join("claimed")
        .join(&grant.token)
        .exists());
}

#[test]
fn failed_code_gate_retains_bounded_redacted_stdout_and_stderr() {
    let fixture = RuntimeFixture::new("code-gate-diagnostics");
    let _environment = fixture.activate();
    let secret = std::fs::canonicalize(fixture.cargo_cache.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut executor = FakeExecutor::passing(&fixture);
    executor.code_gate_failure = Some(CommandObservation {
        success: false,
        exit_code: 101,
        stdout: format!(
            "thread 'release_gate_panics_in_stdout' panicked at src/lib.rs:7\n\
             secret-json-key: {secret}\n{}",
            "x".repeat(160 * 1024)
        )
        .into_bytes(),
        stderr: b"error: test failed, to rerun pass `--lib`\n".to_vec(),
    });
    let report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut executor,
    )
    .unwrap();
    assert_eq!(report.status, ReleaseStatus::Blocked, "{report:#?}");
    let detail = report.detail.unwrap();
    assert!(detail.contains("stdout:"), "{detail}");
    assert!(detail.contains("release_gate_panics_in_stdout"), "{detail}");
    assert!(detail.contains("stderr:"), "{detail}");
    assert!(detail.contains("test failed"), "{detail}");
    assert!(detail.contains("[REDACTED]"), "{detail}");
    assert!(detail.contains("diagnostic output omitted"), "{detail}");
    assert!(!detail.contains(&secret), "{detail}");
    assert!(detail.len() < 140 * 1024, "diagnostic was not bounded");
}

#[test]
fn fake_executor_blocks_pending_failed_malformed_and_residue_evidence() {
    let seed = RuntimeFixture::new("fake-blocking-seed");
    let passed = FakeExecutor::passing(&seed).journey_report;
    drop(seed);
    let mut blocked = passed.clone();
    blocked["status"] = json!("blocked");
    let mut failed = passed.clone();
    failed["status"] = json!("failed");
    failed["assertions_failed"] = json!(1);
    for (index, journey_report) in [
        json!({"pending_human":true}),
        blocked,
        failed,
        json!("malformed"),
    ]
    .into_iter()
    .enumerate()
    {
        let fixture = RuntimeFixture::new(&format!("fake-blocking-{index}"));
        let _environment = fixture.activate();
        let mut executor = FakeExecutor::passing(&fixture);
        executor.journey_report = journey_report;
        let report = loom::release::rehearse_with_executor(
            fixture.root.path(),
            loom::release::ReleasePhase::IsolatedDogfood,
            &mut executor,
        )
        .unwrap();
        assert_eq!(report.status, ReleaseStatus::Blocked, "{report:#?}");
        assert!(
            report
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("Journey run output")
                || report
                    .detail
                    .as_deref()
                    .unwrap_or_default()
                    .contains("complete passing report"),
            "{:?}",
            report.detail
        );
    }

    {
        let fixture = RuntimeFixture::new("fake-blocking-coverage");
        let _environment = fixture.activate();
        let mut coverage = FakeExecutor::passing(&fixture);
        coverage.coverage["intents"]["planned_or_needs_change"] = json!(1);
        let report = loom::release::rehearse_with_executor(
            fixture.root.path(),
            loom::release::ReleasePhase::IsolatedDogfood,
            &mut coverage,
        )
        .unwrap();
        assert_eq!(report.status, ReleaseStatus::Blocked);
        assert!(report
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("coverage is blocking"));
    }

    {
        let fixture = RuntimeFixture::new("fake-blocking-drift");
        let _environment = fixture.activate();
        let mut malformed_drift = FakeExecutor::passing(&fixture);
        malformed_drift.drift = json!({"journeys":{},"stale":0});
        let report = loom::release::rehearse_with_executor(
            fixture.root.path(),
            loom::release::ReleasePhase::IsolatedDogfood,
            &mut malformed_drift,
        )
        .unwrap();
        assert_eq!(report.status, ReleaseStatus::Blocked);
        assert!(report
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("journeys is not an array"));
    }
}

#[test]
fn predictable_forged_or_stale_outer_context_never_authorizes_rehearsal() {
    let root = fixture_root("predictable-context");
    let output = release_command(root.path(), "isolated-dogfood", true);
    assert!(!output.status.success());
    assert!(report(&output)
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("semantic hash context"));

    let fixture = RuntimeFixture::new("stale-context");
    let _environment = fixture.activate();
    std::env::set_var(OUTER_SURFACE_HASH_ENV, "stale-surface");
    let mut executor = FakeExecutor::passing(&fixture);
    let stale_capsule = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut executor,
    )
    .unwrap();
    assert_eq!(stale_capsule.status, ReleaseStatus::Blocked);
    assert!(stale_capsule
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("does not match reserved runtime context"));
    std::env::set_var(OUTER_SURFACE_HASH_ENV, &fixture.outer.surface_hash);

    let store = loom::store::Store::open(fixture.root.path()).unwrap();
    let surface = store
        .resolve_node("loom-release-rehearsal", Some(NodeType::InterfaceSurface))
        .unwrap();
    let mut changed = surface.body.clone();
    changed["identity"] = json!("changed after compilation");
    store.set_node_body(&surface.id, &changed).unwrap();
    drop(store);
    let mut executor = FakeExecutor::passing(&fixture);
    let stale_graph = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut executor,
    )
    .unwrap();
    assert_eq!(stale_graph.status, ReleaseStatus::Blocked);
    assert!(stale_graph
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("surface hash is stale"));
}

#[test]
fn journey_coverage_and_drift_evidence_fail_closed() {
    let passed = json!({
        "journey_id":"candidate-check",
        "profile":"proof",
        "journey_hash":"journey-hash",
        "surface_hash":"surface-hash",
        "status":"passed",
        "assertions_passed":1,
        "assertions_failed":0,
        "steps":[{
            "step_id":"check",
            "operation_id":"check-op",
            "argv":["loom","status","--json"],
            "exit_code":0,
            "output":{"ok":true},
            "assertions_passed":1,
            "assertions_failed":0
        }],
        "captures":{}
    });
    loom::release::require_passed_journey_report(
        &serde_json::to_vec(&passed).unwrap(),
        "candidate-check",
        "proof",
        1,
    )
    .unwrap();
    let mut blocked_report = passed.clone();
    blocked_report["status"] = json!("blocked");
    let mut failed_report = passed.clone();
    failed_report["status"] = json!("failed");
    failed_report["assertions_failed"] = json!(1);
    failed_report["detail"] = json!(format!("[REDACTED] {}", "x".repeat(96 * 1024)));
    failed_report["steps"][0]["exit_code"] = json!(7);
    failed_report["steps"][0]["assertions_failed"] = json!(1);
    failed_report["steps"][0]["argv"] = json!(["never-expose-argv-secret"]);
    failed_report["steps"][0]["output"] = json!({"secret":"never-expose-output-secret"});
    let diagnostic = loom::release::require_passed_journey_report(
        &serde_json::to_vec(&failed_report).unwrap(),
        "candidate-check",
        "proof",
        1,
    )
    .unwrap_err()
    .to_string();
    assert!(diagnostic.contains("\"status\":\"failed\""), "{diagnostic}");
    assert!(
        diagnostic.contains("\"assertions_failed\":1"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("\"step_count\":1"), "{diagnostic}");
    assert!(diagnostic.contains("\"step_id\":\"check\""), "{diagnostic}");
    assert!(diagnostic.contains("\"exit_code\":7"), "{diagnostic}");
    assert!(diagnostic.contains("[REDACTED]"), "{diagnostic}");
    assert!(
        diagnostic.contains("diagnostic output omitted"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains("never-expose-argv-secret"));
    assert!(!diagnostic.contains("never-expose-output-secret"));
    assert!(diagnostic.len() < 70 * 1024);
    let mut incomplete_report = passed.clone();
    incomplete_report["steps"] = json!([]);
    for invalid in [
        json!({"pending_human":true}),
        blocked_report,
        failed_report,
        incomplete_report,
    ] {
        assert!(loom::release::require_passed_journey_report(
            &serde_json::to_vec(&invalid).unwrap(),
            "candidate-check",
            "proof",
            1,
        )
        .is_err());
    }
    assert!(loom::release::require_passed_journey_report(
        b"not-json",
        "candidate-check",
        "proof",
        1,
    )
    .is_err());

    let clean_coverage = json!({
        "intents":{"planned_or_needs_change":0},
        "grounding":{"ungrounded":0},
        "codefiles":{"registered":1,"owned":1,"observed":1,"unowned":0}
    });
    loom::release::require_clean_coverage(&serde_json::to_vec(&clean_coverage).unwrap()).unwrap();
    let mut planned = clean_coverage.clone();
    planned["intents"] = json!({"planned_or_needs_change":1});
    let mut ungrounded = clean_coverage.clone();
    ungrounded["grounding"] = json!({"ungrounded":1});
    let mut unowned = clean_coverage.clone();
    unowned["codefiles"] = json!({"registered":2,"owned":1,"observed":0,"unowned":1});
    for blocked in [planned, ungrounded, unowned, json!({"intents":{}})] {
        assert!(
            loom::release::require_clean_coverage(&serde_json::to_vec(&blocked).unwrap()).is_err()
        );
    }

    loom::release::require_clean_drift(br#"{"journeys":[{"current":true}],"stale":0}"#).unwrap();
    for blocked in [
        br#"[]"#.as_slice(),
        br#"{"journeys":{},"stale":0}"#.as_slice(),
        br#"{"journeys":[{"current":false}],"stale":1}"#.as_slice(),
        br#"{"journeys":[{}],"stale":0}"#.as_slice(),
    ] {
        assert!(loom::release::require_clean_drift(blocked).is_err());
    }
}

#[test]
fn exact_outer_recursion_is_suppressed_but_variants_and_escape_argv_are_rejected() {
    let spec = loom::journey::parse(Path::new("journeys/release-workflow.yaml")).unwrap();
    let manifest: loom::journey::SurfaceManifest =
        serde_json::from_value(release_surface_manifest(&spec.semantic_hash().unwrap())).unwrap();
    let outer = OuterJourneyAttestation {
        journey_id: "release-workflow".into(),
        profile: "proof".into(),
        run_id: "release-workflow.proof.test".into(),
        journey_hash: spec.semantic_hash().unwrap(),
        surface_hash: "surface".into(),
        compiler_version: loom::journey::JOURNEY_COMPILER_VERSION.into(),
        proof_hash: "proof".into(),
        excluded_from_nested_execution: true,
        exclusion_reason: "exact outer".into(),
        context_binding_limit: "same-user boundary".into(),
    };
    let mut ledger = Vec::new();
    loom::release::inspect_candidate_manifest_operations(&spec, &manifest, &outer, &mut ledger)
        .unwrap();
    assert_eq!(ledger.len(), 3);
    assert!(ledger
        .iter()
        .all(|entry| entry.outcome == "suppressed_exact_outer" && !entry.attempted));

    for mut invalid in [manifest.clone(), manifest.clone(), manifest.clone()] {
        if invalid.surface.operations[0].id == "verify-isolated-dogfood" {
            invalid.surface.operations[0]
                .argv
                .insert(5, "--skip".into());
        }
        assert!(loom::release::inspect_candidate_manifest_operations(
            &spec,
            &invalid,
            &outer,
            &mut Vec::new(),
        )
        .is_err());
    }
    let mut shell = manifest;
    shell.surface.operations[0].argv = vec!["sh".into(), "-c".into(), "git push".into()];
    let mut ledger = Vec::new();
    assert!(loom::release::inspect_candidate_manifest_operations(
        &spec,
        &shell,
        &outer,
        &mut ledger
    )
    .is_err());
    assert_eq!(ledger[0].outcome, "rejected_candidate_surface_policy");
}

#[test]
fn candidate_surface_policy_rejects_aliases_overrides_control_templates_and_unconfined_effects() {
    let spec = loom::journey::parse(Path::new("journeys/release-workflow.yaml")).unwrap();
    let manifest: loom::journey::SurfaceManifest =
        serde_json::from_value(release_surface_manifest(&spec.semantic_hash().unwrap())).unwrap();
    let outer = OuterJourneyAttestation {
        journey_id: "release-workflow".into(),
        profile: "proof".into(),
        run_id: "release-workflow.proof.policy".into(),
        journey_hash: spec.semantic_hash().unwrap(),
        surface_hash: "surface".into(),
        compiler_version: loom::journey::JOURNEY_COMPILER_VERSION.into(),
        proof_hash: "proof".into(),
        excluded_from_nested_execution: true,
        exclusion_reason: "exact outer".into(),
        context_binding_limit: "same-user boundary".into(),
    };

    let mut cases = Vec::new();
    let mut alias = manifest.clone();
    alias.surface.operations[0].argv[0] = "./loom".into();
    cases.push(alias);
    let mut graph_override = manifest.clone();
    graph_override.surface.operations[0]
        .argv
        .splice(1..1, ["--graph".into(), "/tmp/escape".into()]);
    cases.push(graph_override);
    let mut root_override = manifest.clone();
    root_override.surface.operations[0]
        .argv
        .splice(1..1, ["--root".into(), "../escape".into()]);
    cases.push(root_override);
    let mut control_template = manifest.clone();
    control_template.surface.operations[0].argv =
        vec!["loom".into(), "${{ inputs.topic }}".into(), "--json".into()];
    cases.push(control_template);
    let mut reserved_env = manifest.clone();
    reserved_env.surface.operations[0]
        .environment
        .push("LOOM_AGENT".into());
    cases.push(reserved_env);
    for name in [
        "PATH",
        "GIT_CONFIG_COUNT",
        "GIT_TRACE",
        "LD_PRELOAD",
        "HTTPS_PROXY",
        "OTHER_HOST_VALUE",
    ] {
        let mut ambient_env = manifest.clone();
        ambient_env.surface.operations[0].environment = vec![name.into()];
        cases.push(ambient_env);
    }
    let mut escaped_codefile = manifest.clone();
    escaped_codefile.surface.codefile = "../src/release.rs".into();
    cases.push(escaped_codefile);
    let mut mislabeled_mutation = manifest.clone();
    mislabeled_mutation.surface.operations[0].argv =
        vec!["loom".into(), "sync".into(), "--json".into()];
    mislabeled_mutation.surface.operations[0].read_only = true;
    cases.push(mislabeled_mutation);
    let mut unreachable = manifest.clone();
    let mut extra = unreachable.surface.operations[0].clone();
    extra.id = "unreachable-operation".into();
    extra.argv = vec!["loom".into(), "status".into(), "--json".into()];
    unreachable.surface.operations.push(extra);
    cases.push(unreachable);

    for invalid in cases {
        assert!(
            loom::release::inspect_candidate_manifest_operations(
                &spec,
                &invalid,
                &outer,
                &mut Vec::new(),
            )
            .is_err(),
            "invalid candidate operation was accepted: {invalid:#?}"
        );
    }

    let mut workspace_env_spec = spec.clone();
    workspace_env_spec
        .profiles
        .get_mut("proof")
        .unwrap()
        .workspace
        .env
        .insert("PATH".into(), "must-not-leak".into());
    let error = loom::candidate_surface_policy::inspect_manifest(
        &workspace_env_spec,
        &manifest,
        loom::candidate_surface_policy::PolicyMode::DetachedReleaseInspection {
            outer_journey_id: "release-workflow",
            outer_surface_id: "loom-release-rehearsal",
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("PATH"));
    assert!(!error.to_string().contains("must-not-leak"));

    let proof_spec = loom::journey::parse(Path::new("journeys/proof-stability.yaml")).unwrap();
    let mut unsafe_payload = loom::journey::SurfaceManifest::parse_json(Path::new(
        "journeys/surfaces/proof-stability.surface.json",
    ))
    .unwrap();
    let registration = unsafe_payload
        .surface
        .operations
        .iter_mut()
        .find(|operation| operation.id == "register-repeatable-proof")
        .unwrap();
    let command_index = registration
        .argv
        .iter()
        .position(|token| token == "--command")
        .unwrap()
        + 1;
    registration.argv[command_index] = "sh -c 'git push'".into();
    assert!(loom::candidate_surface_policy::inspect_manifest(
        &proof_spec,
        &unsafe_payload,
        loom::candidate_surface_policy::PolicyMode::DetachedReleaseInspection {
            outer_journey_id: "release-workflow",
            outer_surface_id: "loom-release-rehearsal",
        },
    )
    .is_err());

    let mut true_payload = loom::journey::SurfaceManifest::parse_json(Path::new(
        "journeys/surfaces/proof-stability.surface.json",
    ))
    .unwrap();
    let registration = true_payload
        .surface
        .operations
        .iter_mut()
        .find(|operation| operation.id == "register-repeatable-proof")
        .unwrap();
    let command_index = registration
        .argv
        .iter()
        .position(|token| token == "--command")
        .unwrap()
        + 1;
    registration.argv[command_index] = "true".into();
    let error = loom::candidate_surface_policy::inspect_manifest(
        &proof_spec,
        &true_payload,
        loom::candidate_surface_policy::PolicyMode::DetachedReleaseInspection {
            outer_journey_id: "release-workflow",
            outer_surface_id: "loom-release-rehearsal",
        },
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("executed Validation payload must use exact bare argv0 'loom'"));
}

#[test]
fn absorb_is_always_a_confined_mutation_despite_authored_read_only() {
    let spec = loom::journey::parse(Path::new("journeys/release-workflow.yaml")).unwrap();
    let mut manifest: loom::journey::SurfaceManifest =
        serde_json::from_value(release_surface_manifest(&spec.semantic_hash().unwrap())).unwrap();
    let operation = &mut manifest.surface.operations[0];
    operation.argv = vec!["loom".into(), "absorb".into(), "--json".into()];
    operation.environment.clear();
    operation.read_only = false;
    let operation_id = operation.id.clone();
    let argv = operation.argv.clone();

    let plan = loom::candidate_surface_policy::inspect_manifest(
        &spec,
        &manifest,
        loom::candidate_surface_policy::PolicyMode::DetachedReleaseInspection {
            outer_journey_id: "release-workflow",
            outer_surface_id: "loom-release-rehearsal",
        },
    )
    .unwrap();
    assert_eq!(
        plan.inspections()
            .iter()
            .find(|inspection| inspection.operation_id == operation_id)
            .unwrap()
            .capability,
        loom::candidate_surface_policy::DerivedCapability::ConfinedMutation
    );
    assert!(plan
        .authorize(
            &operation_id,
            argv,
            loom::candidate_surface_policy::ActualConfinement::LiveReadOnly,
        )
        .is_err());

    manifest.surface.operations[0].read_only = true;
    assert!(loom::candidate_surface_policy::inspect_manifest(
        &spec,
        &manifest,
        loom::candidate_surface_policy::PolicyMode::DetachedReleaseInspection {
            outer_journey_id: "release-workflow",
            outer_surface_id: "loom-release-rehearsal",
        },
    )
    .is_err());
}

#[test]
fn fresh_fixpoint_runs_both_workspace_rejection_probes() {
    let fixture = RuntimeFixture::new("probe-success");
    let _environment = fixture.activate();
    let mut executor = FakeExecutor::passing(&fixture);
    let report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::FreshFixpoint,
        &mut executor,
    )
    .unwrap();
    assert_eq!(report.status, ReleaseStatus::Passed, "{report:#?}");
    assert_eq!(report.workspace.nonempty_probe, "rejected");
    assert_eq!(report.workspace.preinitialized_probe, "rejected");
    assert_probe_ledger_prefix(&report);
}

#[test]
fn gated_preparation_runs_both_exact_workspace_probes_and_unrelated_failures_block() {
    let fixture = RuntimeFixture::new("gated-probes");
    let _environment = fixture.activate();
    let mut executor = FakeExecutor::passing(&fixture);
    let report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::GatedPreparation,
        &mut executor,
    )
    .unwrap();
    assert_eq!(report.status, ReleaseStatus::Passed, "{report:#?}");
    assert_eq!(report.workspace.nonempty_probe, "rejected");
    assert_eq!(report.workspace.preinitialized_probe, "rejected");
    assert_probe_ledger_prefix(&report);
    assert!(
        loom::release::validate_workspace_probe_failure("nonempty", "permission denied").is_err()
    );
    assert!(loom::release::validate_workspace_probe_failure(
        "preinitialized",
        "unrelated source failure"
    )
    .is_err());
}

#[test]
fn production_dependency_cache_checks_locked_host_target_offline_without_cache_drift() {
    let lock = RELEASE_ENV
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap();
    let previous = std::env::var_os(loom::release::RELEASE_CARGO_HOME_ENV);
    std::env::remove_var(loom::release::RELEASE_CARGO_HOME_ENV);
    // The temp root is process-wide and this suite runs its tests in parallel
    // threads, so it is read here and never written: mutating it would relocate
    // every concurrently building fixture in the same process.
    let temp_is_nested = temp_root_is_nested_in(&root);

    let outcome = loom::release::dependency_cache_smoke(&root);

    match previous {
        Some(value) => std::env::set_var(loom::release::RELEASE_CARGO_HOME_ENV, value),
        None => std::env::remove_var(loom::release::RELEASE_CARGO_HOME_ENV),
    }
    drop(lock);

    // The adapter refuses to allocate its scratch workspace inside the checkout
    // it is verifying, so a verification run can never write into the tree whose
    // bytes it attests. When this suite itself runs INSIDE a detached release
    // candidate, the ambient temp root IS that candidate's confined sandbox,
    // nested under the root passed here — so refusing is the correct outcome and
    // the assertion follows the environment rather than relaxing the guard.
    if temp_is_nested {
        let error = outcome
            .expect_err("a temp root inside the checkout under test must be refused")
            .to_string();
        assert!(
            error.contains("temp root must be outside the caller repository"),
            "{error}"
        );
        return;
    }

    let attestation = outcome.unwrap();
    assert_eq!(
        attestation.strategy,
        "existing_cargo_home_read_only_verified"
    );
    assert!(attestation.offline);
    assert!(attestation.unchanged);
    assert_eq!(attestation.before_hash, attestation.after_hash);
}

/// Whether the process-wide temp root currently resolves inside `root`.
fn temp_root_is_nested_in(root: &Path) -> bool {
    std::env::temp_dir()
        .canonicalize()
        .map(|temp| temp.starts_with(root))
        .unwrap_or(false)
}

#[test]
fn git_aware_candidate_plan_refuses_secret_paths_before_execution() {
    let fixture = RuntimeFixture::new("secret-path");
    fixture
        .root
        .write("config/.env.production", "TOKEN=never-copy\n");
    assert!(ProcessCommand::new("git")
        .args(["add", "-f", "config/.env.production"])
        .current_dir(fixture.root.path())
        .status()
        .unwrap()
        .success());
    let _environment = fixture.activate();
    let mut executor = FakeExecutor::passing(&fixture);
    let report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut executor,
    )
    .unwrap();
    assert_eq!(report.status, ReleaseStatus::Blocked);
    let detail = report.detail.as_deref().unwrap_or_default();
    assert!(detail.contains("secret-bearing"), "{detail}");
    assert!(executor.calls.is_empty());
}

#[test]
fn source_inventory_has_git_and_gitless_hash_parity() {
    let mut fixture = RuntimeFixture::new("inventory-parity");
    let _environment = fixture.activate();
    let mut git_executor = FakeExecutor::passing(&fixture);
    let git_report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut git_executor,
    )
    .unwrap();
    assert_eq!(git_report.status, ReleaseStatus::Passed, "{git_report:#?}");
    let backup = Tmp::new();
    std::fs::rename(fixture.root.path().join(".git"), backup.path().join("git")).unwrap();
    drop(_environment);
    fixture.refresh_outer_authority("inventory-parity-gitless");
    let _environment = fixture.activate();
    let mut snapshot_executor = FakeExecutor::passing(&fixture);
    let snapshot_report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut snapshot_executor,
    )
    .unwrap();
    assert_eq!(
        snapshot_report.status,
        ReleaseStatus::Passed,
        "{snapshot_report:#?}"
    );
    assert_eq!(git_report.candidate_hash, snapshot_report.candidate_hash);
    let git_inventory = git_report.source_inventory.unwrap();
    let snapshot_inventory = snapshot_report.source_inventory.unwrap();
    assert_eq!(
        git_inventory.inventory_hash,
        snapshot_inventory.inventory_hash
    );
    assert_eq!(
        git_inventory.manifest_hash,
        snapshot_inventory.manifest_hash
    );
    assert_eq!(git_inventory.git_verification, "verified");
    assert_eq!(snapshot_inventory.git_verification, "not_applicable");
    assert!(!git_inventory.git_influenced_plan);
    assert!(!snapshot_inventory.git_influenced_plan);
}

#[test]
fn source_inventory_manifest_binds_exact_tombstones_and_counts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(
        loom::release::source_inventory_manifest_hash(root).unwrap(),
        RELEASE_INVENTORY_MANIFEST_HASH
    );
    let inventory: Value =
        serde_json::from_slice(&std::fs::read(root.join("release/inventory.json")).unwrap())
            .unwrap();
    let files = inventory["files"].as_array().unwrap();
    assert_eq!(files.len(), RELEASE_INVENTORY_ENTRY_COUNT);
    assert_eq!(
        files
            .iter()
            .filter(|entry| entry["mode"] != "absent")
            .count(),
        RELEASE_INVENTORY_FILE_COUNT
    );
    let tombstones: Vec<_> = files
        .iter()
        .filter(|entry| entry["mode"] == "absent")
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        tombstones,
        [
            "src/commands/journey/coverage.rs",
            "src/commands/journey/invariants.rs",
            "src/commands/journey/prompt.rs",
            "tests/ring41_diagnose_parity.rs",
        ]
    );
}

#[test]
fn all_source_surface_manifests_parse_and_bind_authored_journeys() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut manifests: Vec<PathBuf> = std::fs::read_dir(root.join("journeys/surfaces"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".surface.json"))
        })
        .collect();
    manifests.sort();
    assert_eq!(manifests.len(), 30);

    let mut operation_count = 0;
    let mut declared_read_only = 0;
    let mut declared_mutable = 0;
    for path in manifests {
        let manifest = loom::journey::SurfaceManifest::parse_json(&path).unwrap();
        let journey_path = root
            .join("journeys")
            .join(format!("{}.yaml", manifest.journey_id));
        let journey = loom::journey::parse(&journey_path).unwrap();
        let journey_hash = journey.semantic_hash().unwrap();
        manifest
            .validate_for(&journey, &journey_hash)
            .unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
        let plan = loom::candidate_surface_policy::inspect_manifest(
            &journey,
            &manifest,
            loom::candidate_surface_policy::PolicyMode::DetachedReleaseInspection {
                outer_journey_id: "release-workflow",
                outer_surface_id: "loom-release-rehearsal",
            },
        )
        .unwrap_or_else(|error| panic!("{} policy: {error:#}", path.display()));
        assert_eq!(plan.inspections().len(), manifest.surface.operations.len());
        operation_count += manifest.surface.operations.len();
        declared_read_only += manifest
            .surface
            .operations
            .iter()
            .filter(|operation| operation.read_only)
            .count();
        declared_mutable += manifest
            .surface
            .operations
            .iter()
            .filter(|operation| !operation.read_only)
            .count();
    }
    assert_eq!(operation_count, 86);
    assert_eq!(declared_read_only, 34);
    assert_eq!(declared_mutable, 52);
}

#[test]
fn divergence_queue_builds_local_meaning_drift_from_stable_identity() {
    const TARGET: &str = "dispatch the authoritative proof runner";
    const DRIFT: &str = "fixture: criterion changed after human ratification";
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = root.join("journeys/surfaces/divergence-queue.surface.json");
    let source = std::fs::read_to_string(&path).unwrap();
    let manifest = loom::journey::SurfaceManifest::parse_json(&path).unwrap();
    let setup = manifest.setup.as_ref().expect("confined setup");
    assert_eq!(setup.graph, loom::journey::SetupGraph::LocalSnapshot);
    assert_eq!(
        setup.operations,
        ["verify-fixture-ratification", "create-meaning-drift"]
    );

    // Staging-graph residue the redesigned fixture must never carry: the old
    // fixture's ambient-residue Intent UUID, the new fixture Intent's staging
    // UUID, and the old fixture's drift wording.
    for stale in [
        "428f2b0e0a8d55edc61caad0099f2ad1",
        "5345985b41a16ffdade0d603bced3643",
        "classify only evidence-judgment conflicts for human decision",
        "The queue sends every unratified behavior to a human, including missing approval and agent-resolvable friction.",
    ] {
        assert!(
            !source.contains(stale),
            "the divergence-queue fixture must not borrow another graph's identity: {stale}"
        );
    }

    let verify = manifest
        .surface
        .operations
        .iter()
        .find(|operation| operation.id == "verify-fixture-ratification")
        .unwrap();
    assert_eq!(verify.argv[4], TARGET);
    assert!(!verify.read_only, "setup operations must be mutable");
    let verify_json = serde_json::to_value(verify).unwrap();
    let verify_assertions = verify_json["output"]["assertions"].as_array().unwrap();
    let verify_assertion = |id: &str| {
        verify_assertions
            .iter()
            .find(|assertion| assertion["id"] == id)
            .unwrap_or_else(|| panic!("missing divergence verify assertion {id}"))
    };
    assert_eq!(verify_assertion("setup-target-name")["pointer"], "/name");
    assert_eq!(verify_assertion("setup-target-name")["equals"], TARGET);
    assert_eq!(
        verify_assertion("setup-target-ratified")["equals"],
        "ratified"
    );

    let fixture = manifest
        .surface
        .operations
        .iter()
        .find(|operation| operation.id == "create-meaning-drift")
        .unwrap();
    assert_eq!(fixture.argv[4], TARGET);
    assert!(fixture.argv.iter().any(|token| token == DRIFT));
    assert!(!fixture.read_only);
    let fixture_json = serde_json::to_value(fixture).unwrap();
    let fixture_assertions = fixture_json["output"]["assertions"].as_array().unwrap();
    let fixture_assertion = |id: &str| {
        fixture_assertions
            .iter()
            .find(|assertion| assertion["id"] == id)
            .unwrap_or_else(|| panic!("missing divergence setup assertion {id}"))
    };
    assert_eq!(
        fixture_assertion("setup-drift-target-name")["equals"],
        TARGET
    );
    assert_eq!(
        fixture_assertion("setup-changed-description")["equals"],
        DRIFT
    );
    assert_eq!(fixture_assertion("setup-is-redefinition")["equals"], false);

    let inspect = manifest
        .surface
        .operations
        .iter()
        .find(|operation| operation.id == "inspect-human-decision-work")
        .unwrap();
    let serialized = serde_json::to_value(inspect).unwrap();
    let assertions = serialized["output"]["assertions"].as_array().unwrap();
    let assertion = |id: &str| {
        assertions
            .iter()
            .find(|assertion| assertion["id"] == id)
            .unwrap_or_else(|| panic!("missing divergence assertion {id}"))
    };
    assert_eq!(assertion("served-target-kind")["equals"], "intent");
    assert_eq!(
        assertion("served-item-is-meaning-drift")["contains"],
        "redefined after ratification — the words changed under the yes"
    );
    assert_eq!(assertion("exactly-three-options")["exists"], false);

    let journey = loom::journey::parse(&root.join("journeys/divergence-queue.yaml")).unwrap();
    let semantic_hash = journey.semantic_hash().unwrap();
    manifest.validate_for(&journey, &semantic_hash).unwrap();
    loom::candidate_surface_policy::inspect_manifest(
        &journey,
        &manifest,
        loom::candidate_surface_policy::PolicyMode::DetachedReleaseInspection {
            outer_journey_id: "release-workflow",
            outer_surface_id: "loom-release-rehearsal",
        },
    )
    .unwrap();
}

#[test]
fn self_audit_binds_to_current_local_exact_set_authorization() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = root.join("journeys/surfaces/self-audit.surface.json");
    let source = std::fs::read_to_string(&path).unwrap();

    for stale in [
        "Approve all 27 manifests",
        "b3668c1dc71cb45a3",
        "30c9ea0547a0aedc573b08278af17ce5",
        "e44d232852824672a8611785158d0361",
    ] {
        assert!(
            !source.contains(stale),
            "self-audit must not bind to historical authorization material: {stale}"
        );
    }

    let document: Value = serde_json::from_str(&source).unwrap();
    let operation = document["surface"]["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|operation| operation["id"] == "audit-authorized-batch")
        .unwrap();
    let assertions = operation["output"]["assertions"].as_array().unwrap();
    let assertion = |id: &str| {
        assertions
            .iter()
            .find(|assertion| assertion["id"] == id)
            .unwrap_or_else(|| panic!("missing self-audit assertion {id}"))
    };

    let envelope = assertion("authorized-current-ratification-envelope")["matches"]
        .as_str()
        .unwrap();
    for current_field in [
        r#""event":\s*"batch_authorization""#,
        r#""origin":\s*"local""#,
        r#""authority":\s*"human""#,
        r#""claim":\s*"ratification""#,
        r#""operation":\s*"ratify""#,
        r#""decision_mode":\s*"batch""#,
        "journey-derive-accept:2",
    ] {
        assert!(envelope.contains(current_field), "missing {current_field}");
    }
    assert!(
        envelope.contains(r#""subjects":\s*\[\s*"[0-9a-f]{32}"\s*,\s*"[0-9a-f]{32}"\s*\]"#),
        "the authorization must cover exactly two deterministic subjects"
    );

    let manifest = loom::journey::SurfaceManifest::parse_json(&path).unwrap();
    let journey = loom::journey::parse(&root.join("journeys/self-audit.yaml")).unwrap();
    let semantic_hash = journey.semantic_hash().unwrap();
    manifest.validate_for(&journey, &semantic_hash).unwrap();
    loom::candidate_surface_policy::inspect_manifest(
        &journey,
        &manifest,
        loom::candidate_surface_policy::PolicyMode::DetachedReleaseInspection {
            outer_journey_id: "release-workflow",
            outer_surface_id: "loom-release-rehearsal",
        },
    )
    .unwrap();
}

#[test]
fn source_inventory_preserves_staged_deletions_and_rename_tombstones() {
    let mut fixture = RuntimeFixture::new("staged-tombstones");
    fixture.root.write("src/removed.rs", "tracked deletion\n");
    assert!(ProcessCommand::new("git")
        .args(["add", "-A"])
        .current_dir(fixture.root.path())
        .status()
        .unwrap()
        .success());
    assert!(ProcessCommand::new("git")
        .args([
            "-c",
            "user.name=Loom Test",
            "-c",
            "user.email=loom@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture base",
        ])
        .current_dir(fixture.root.path())
        .status()
        .unwrap()
        .success());

    std::fs::remove_file(fixture.root.path().join("src/removed.rs")).unwrap();
    assert!(ProcessCommand::new("git")
        .args(["add", "-u", "--", "src/removed.rs"])
        .current_dir(fixture.root.path())
        .status()
        .unwrap()
        .success());
    assert!(ProcessCommand::new("git")
        .args(["mv", "src/lib.rs", "src/lib_renamed.rs"])
        .current_dir(fixture.root.path())
        .status()
        .unwrap()
        .success());

    let inventory_path = fixture.root.path().join("release/inventory.json");
    let mut inventory: Value =
        serde_json::from_slice(&std::fs::read(&inventory_path).unwrap()).unwrap();
    let entries = inventory["files"].as_array_mut().unwrap();
    entries
        .iter_mut()
        .find(|entry| entry["path"] == "src/lib.rs")
        .unwrap()["mode"] = json!("absent");
    entries.push(json!({"path":"src/lib_renamed.rs","mode":"regular"}));
    entries.sort_by(|left, right| {
        left["path"]
            .as_str()
            .unwrap()
            .cmp(right["path"].as_str().unwrap())
    });
    std::fs::write(
        &inventory_path,
        serde_json::to_vec_pretty(&inventory).unwrap(),
    )
    .unwrap();
    assert!(ProcessCommand::new("git")
        .args(["add", "--", "release/inventory.json"])
        .current_dir(fixture.root.path())
        .status()
        .unwrap()
        .success());

    let _environment = fixture.activate();
    let mut git_executor = FakeExecutor::passing(&fixture);
    let git_report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut git_executor,
    )
    .unwrap();
    assert_eq!(git_report.status, ReleaseStatus::Passed, "{git_report:#?}");
    let git_hash = git_report.candidate_hash.clone();
    let git_inventory = git_report.source_inventory.unwrap();
    assert_eq!(git_inventory.entry_count, 11);
    assert_eq!(git_inventory.file_count, 9);
    assert_eq!(git_inventory.tombstone_count, 2);

    let backup = Tmp::new();
    std::fs::rename(fixture.root.path().join(".git"), backup.path().join("git")).unwrap();
    drop(_environment);
    fixture.refresh_outer_authority("staged-tombstones-gitless");
    let _environment = fixture.activate();
    let mut gitless_executor = FakeExecutor::passing(&fixture);
    let gitless_report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut gitless_executor,
    )
    .unwrap();
    assert_eq!(gitless_report.status, ReleaseStatus::Passed);
    assert_eq!(gitless_report.candidate_hash, git_hash);
}

#[test]
fn source_inventory_rejects_arbitrary_git_tombstone_and_present_gitless_tombstone() {
    let git_fixture = RuntimeFixture::new("arbitrary-tombstone");
    let inventory_path = git_fixture.root.path().join("release/inventory.json");
    let mut inventory: Value =
        serde_json::from_slice(&std::fs::read(&inventory_path).unwrap()).unwrap();
    let entries = inventory["files"].as_array_mut().unwrap();
    entries.push(json!({"path":"src/never-tracked.rs","mode":"absent"}));
    entries.sort_by(|left, right| {
        left["path"]
            .as_str()
            .unwrap()
            .cmp(right["path"].as_str().unwrap())
    });
    std::fs::write(
        &inventory_path,
        serde_json::to_vec_pretty(&inventory).unwrap(),
    )
    .unwrap();
    let environment = git_fixture.activate();
    let mut executor = FakeExecutor::passing(&git_fixture);
    let report = loom::release::rehearse_with_executor(
        git_fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut executor,
    )
    .unwrap();
    assert_eq!(report.status, ReleaseStatus::Blocked);
    let detail = report.detail.unwrap();
    assert!(detail.contains("extra: [src/never-tracked.rs]"), "{detail}");
    drop(environment);

    let gitless_fixture = RuntimeFixture::new("present-tombstone");
    let backup = Tmp::new();
    std::fs::rename(
        gitless_fixture.root.path().join(".git"),
        backup.path().join("git"),
    )
    .unwrap();
    gitless_fixture
        .root
        .write("src/removed.rs", "resurrected deletion\n");
    let _environment = gitless_fixture.activate();
    let mut executor = FakeExecutor::passing(&gitless_fixture);
    let report = loom::release::rehearse_with_executor(
        gitless_fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut executor,
    )
    .unwrap();
    assert_eq!(report.status, ReleaseStatus::Blocked);
    assert!(report
        .detail
        .unwrap()
        .contains("tombstone 'src/removed.rs' is present"));
}

#[test]
fn source_inventory_rejects_skip_worktree_index_state() {
    let fixture = RuntimeFixture::new("skip-worktree");
    assert!(ProcessCommand::new("git")
        .args(["update-index", "--skip-worktree", "src/lib.rs"])
        .current_dir(fixture.root.path())
        .status()
        .unwrap()
        .success());
    let _environment = fixture.activate();
    let mut executor = FakeExecutor::passing(&fixture);
    let report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut executor,
    )
    .unwrap();
    assert_eq!(report.status, ReleaseStatus::Blocked);
    let detail = report.detail.unwrap();
    assert!(
        detail.contains(
            "refuses sparse, assume-unchanged, or exceptional Git index path 'src/lib.rs'"
        ),
        "{detail}"
    );
    assert!(executor.calls.is_empty());
}

#[test]
fn snapshot_scripts_require_one_existing_target_adapter_before_any_build() {
    for relative in ["scripts/dogfood.sh", "scripts/check-fixpoint.sh"] {
        let script = std::fs::read_to_string(relative).unwrap();
        assert_eq!(script.matches("release snapshot --destination").count(), 1);
        assert!(script.contains("$ROOT/target/debug/loom"));
        assert!(script.contains("[ ! -L \"$SNAPSHOTTER\" ]"));
        assert!(script.contains("snapshot_adapter_provenance=existing_target_binary"));
        assert!(script.contains("shasum -a 256"));
        assert!(script.contains("env -i PATH=\"$SNAPSHOT_PATH\""));
        assert!(script.contains(RELEASE_INVENTORY_MANIFEST_HASH));
        assert!(!script.contains("CARGO_TARGET_DIR"));
        assert!(!script.contains("tar "));
        let snapshot = script.find("release snapshot --destination").unwrap();
        if let Some(build) = script.find("cargo build") {
            assert!(snapshot < build, "{relative} builds before snapshotting");
        }
    }

    let fixture = Tmp::new();
    fixture.write("scripts/dogfood.sh", include_str!("../scripts/dogfood.sh"));
    let output = ProcessCommand::new("bash")
        .args(["scripts/dogfood.sh", "--check"])
        .current_dir(fixture.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("trusted snapshot adapter is missing"));
    assert!(!fixture.path().join("target").exists());
}

#[test]
fn gitless_inventory_blocks_missing_and_undeclared_leafs() {
    for (label, mutate, expected) in [
        ("missing", "missing", "missing"),
        ("undeclared", "undeclared", "undeclared"),
    ] {
        let fixture = RuntimeFixture::new(label);
        let backup = Tmp::new();
        std::fs::rename(fixture.root.path().join(".git"), backup.path().join("git")).unwrap();
        match mutate {
            "missing" => std::fs::remove_file(fixture.root.path().join("src/lib.rs")).unwrap(),
            "undeclared" => fixture.root.write(
                "src/.travel.rs.pending-snap",
                "accidental snapshot debris\n",
            ),
            _ => unreachable!(),
        }
        let _environment = fixture.activate();
        let mut executor = FakeExecutor::passing(&fixture);
        let report = loom::release::rehearse_with_executor(
            fixture.root.path(),
            loom::release::ReleasePhase::IsolatedDogfood,
            &mut executor,
        )
        .unwrap();
        assert_eq!(report.status, ReleaseStatus::Blocked);
        assert!(report.detail.unwrap().contains(expected));
        assert!(executor.calls.is_empty());
    }
}

#[test]
fn git_inventory_mismatch_blocks_without_selecting_the_plan() {
    let fixture = RuntimeFixture::new("git-mismatch");
    fixture
        .root
        .write("ring49-rustup-value/toolchain", "not declared\n");
    let _environment = fixture.activate();
    let mut executor = FakeExecutor::passing(&fixture);
    let report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut executor,
    )
    .unwrap();
    assert_eq!(report.status, ReleaseStatus::Blocked);
    assert!(report
        .detail
        .unwrap()
        .contains("stale against tracked/nonignored Git files"));
    assert!(executor.calls.is_empty());
}

#[test]
fn ignored_pending_snapshot_debris_still_blocks_explicit_inventory() {
    let fixture = RuntimeFixture::new("ignored-pending-snapshot");
    fixture.root.write(
        ".gitignore",
        ".loom/\ntarget/\n.release-sandbox/\n*.pending-snap\n",
    );
    fixture
        .root
        .write("src/.travel.rs.pending-snap", "ignored but unsafe\n");
    let _environment = fixture.activate();
    let mut executor = FakeExecutor::passing(&fixture);
    let report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut executor,
    )
    .unwrap();
    assert_eq!(report.status, ReleaseStatus::Blocked);
    let detail = report.detail.unwrap();
    assert!(detail.contains("src/.travel.rs.pending-snap"), "{detail}");
    assert!(executor.calls.is_empty());
}

#[test]
fn typed_snapshot_failure_leaves_the_empty_destination_untouched() {
    let fixture = RuntimeFixture::new("atomic-snapshot-failure");
    fixture.root.write("unexpected.txt", "not declared\n");
    let destination = Tmp::new();
    let error = loom::release::snapshot(fixture.root.path(), destination.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("stale against tracked/nonignored Git files"));
    assert!(std::fs::read_dir(destination.path())
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn typed_snapshot_rejects_inside_nonempty_and_preinitialized_destinations() {
    let fixture = RuntimeFixture::new("snapshot-destinations");
    let inside = fixture.root.path().join("inside");
    std::fs::create_dir(&inside).unwrap();
    assert!(loom::release::snapshot(fixture.root.path(), &inside).is_err());

    let nonempty = Tmp::new();
    nonempty.write("occupied", "x");
    assert!(loom::release::snapshot(fixture.root.path(), nonempty.path()).is_err());

    let initialized = Tmp::new();
    initialized.write(".loom/graph.sqlite", "x");
    assert!(loom::release::snapshot(fixture.root.path(), initialized.path()).is_err());
}

#[cfg(unix)]
#[test]
fn typed_snapshot_normalizes_declared_regular_and_executable_modes() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = RuntimeFixture::new("snapshot-modes");
    let git_backup = Tmp::new();
    std::fs::rename(
        fixture.root.path().join(".git"),
        git_backup.path().join("git"),
    )
    .unwrap();

    let regular = fixture.root.path().join(".gitignore");
    std::fs::set_permissions(&regular, std::fs::Permissions::from_mode(0o600)).unwrap();
    let executable = fixture.root.path().join("src/lib.rs");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();

    let inventory_path = fixture.root.path().join("release/inventory.json");
    let mut inventory: Value =
        serde_json::from_slice(&std::fs::read(&inventory_path).unwrap()).unwrap();
    let entry = inventory["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["path"] == "src/lib.rs")
        .unwrap();
    entry["mode"] = json!("executable");
    std::fs::write(
        &inventory_path,
        serde_json::to_vec_pretty(&inventory).unwrap(),
    )
    .unwrap();

    let destination = Tmp::new();
    let report = loom::release::snapshot(fixture.root.path(), destination.path()).unwrap();
    assert_eq!(report.status, "passed");
    assert_eq!(
        report.candidate_hash,
        report.source_inventory.inventory_hash
    );
    assert_eq!(
        std::fs::metadata(destination.path().join(".gitignore"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    assert_eq!(
        std::fs::metadata(destination.path().join("src/lib.rs"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}

#[cfg(unix)]
#[test]
fn gitless_inventory_refuses_symlink_fifo_and_nested_reserved_paths() {
    use std::os::unix::fs::symlink;

    for kind in ["symlink", "fifo", "reserved"] {
        let fixture = RuntimeFixture::new(kind);
        let backup = Tmp::new();
        std::fs::rename(fixture.root.path().join(".git"), backup.path().join("git")).unwrap();
        match kind {
            "symlink" => symlink("lib.rs", fixture.root.path().join("src/link.rs")).unwrap(),
            "fifo" => assert!(ProcessCommand::new("mkfifo")
                .arg(fixture.root.path().join("src/pipe"))
                .status()
                .unwrap()
                .success()),
            "reserved" => fixture.root.write("src/target/payload", "reserved\n"),
            _ => unreachable!(),
        }
        let _environment = fixture.activate();
        let mut executor = FakeExecutor::passing(&fixture);
        let report = loom::release::rehearse_with_executor(
            fixture.root.path(),
            loom::release::ReleasePhase::IsolatedDogfood,
            &mut executor,
        )
        .unwrap();
        assert_eq!(report.status, ReleaseStatus::Blocked, "{kind}: {report:#?}");
        assert!(executor.calls.is_empty());
    }
}

#[cfg(unix)]
#[test]
fn source_inventory_refuses_symlinked_reserved_root_and_destination() {
    use std::os::unix::fs::symlink;

    let fixture = RuntimeFixture::new("symlinked-reserved-root");
    let graph_backup = Tmp::new();
    std::fs::rename(
        fixture.root.path().join(".loom"),
        graph_backup.path().join("loom"),
    )
    .unwrap();
    symlink(
        graph_backup.path().join("loom"),
        fixture.root.path().join(".loom"),
    )
    .unwrap();
    let destination = Tmp::new();
    assert!(
        loom::release::snapshot(fixture.root.path(), destination.path())
            .unwrap_err()
            .to_string()
            .contains("symlinked reserved root")
    );

    let source = RuntimeFixture::new("symlinked-destination");
    let target = Tmp::new();
    let link = target.path().with_extension("link");
    symlink(target.path(), &link).unwrap();
    assert!(loom::release::snapshot(source.root.path(), &link).is_err());
    std::fs::remove_file(link).unwrap();
}

#[test]
fn gitless_snapshot_plan_succeeds_before_manifest_attestation_blocks() {
    let fixture = RuntimeFixture::new("gitless-manifest-block");
    let backup = Tmp::new();
    std::fs::rename(fixture.root.path().join(".git"), backup.path().join("git")).unwrap();
    let inventory_path = fixture.root.path().join("release/inventory.json");
    let mut inventory: Value =
        serde_json::from_slice(&std::fs::read(&inventory_path).unwrap()).unwrap();
    inventory["files"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| entry["path"] != "journeys/surfaces/release-workflow.surface.json");
    std::fs::write(
        &inventory_path,
        serde_json::to_vec_pretty(&inventory).unwrap(),
    )
    .unwrap();
    std::fs::remove_file(
        fixture
            .root
            .path()
            .join("journeys/surfaces/release-workflow.surface.json"),
    )
    .unwrap();
    let _environment = fixture.activate();
    let mut executor = FakeExecutor::passing(&fixture);
    let report = loom::release::rehearse_with_executor(
        fixture.root.path(),
        loom::release::ReleasePhase::IsolatedDogfood,
        &mut executor,
    )
    .unwrap();
    assert_eq!(report.status, ReleaseStatus::Blocked);
    assert!(report
        .detail
        .unwrap()
        .contains("canonical manifests are missing"));
    assert!(report
        .execution_ledger
        .iter()
        .any(|entry| { entry.source == "candidate_file_plan" && entry.outcome == "passed" }));
    assert!(executor.calls.is_empty());
}

fn release_surface_manifest(journey_hash: &str) -> Value {
    let common = |phase: &str| {
        vec![
            json!({"id":format!("{phase}-schema"),"pointer":"/schema","type":"string","equals":RELEASE_REHEARSAL_SCHEMA}),
            json!({"id":format!("{phase}-phase"),"pointer":"/phase","type":"string","equals":phase}),
            json!({"id":format!("{phase}-passed"),"pointer":"/status","type":"string","equals":"passed"}),
            json!({"id":format!("{phase}-inventory-path"),"pointer":"/source_inventory/path","type":"string","equals":"release/inventory.json"}),
            json!({"id":format!("{phase}-inventory-schema"),"pointer":"/source_inventory/schema","type":"string","equals":"loom.release-source-inventory-attestation/v1"}),
            json!({"id":format!("{phase}-inventory-hash"),"pointer":"/source_inventory/manifest_hash","type":"string","equals":RELEASE_INVENTORY_MANIFEST_HASH}),
            json!({"id":format!("{phase}-inventory-content-hash"),"pointer":"/source_inventory/inventory_hash","type":"string","matches":"^[0-9a-f]{16}$"}),
            json!({"id":format!("{phase}-inventory-provenance"),"pointer":"/source_inventory/provenance","type":"string","matches":"^source_controlled_manifest_(git_verified|non_git)$"}),
            json!({"id":format!("{phase}-inventory-git-verification"),"pointer":"/source_inventory/git_verification","type":"string","matches":"^(verified|not_applicable)$"}),
            json!({"id":format!("{phase}-inventory-not-git-selected"),"pointer":"/source_inventory/git_influenced_plan","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-inventory-materialized"),"pointer":"/source_inventory/materialized_matches","type":"boolean","equals":true}),
            json!({"id":format!("{phase}-inventory-entries"),"pointer":"/source_inventory/entry_count","type":"integer","equals":RELEASE_INVENTORY_ENTRY_COUNT}),
            json!({"id":format!("{phase}-inventory-files"),"pointer":"/source_inventory/file_count","type":"integer","equals":RELEASE_INVENTORY_FILE_COUNT}),
            json!({"id":format!("{phase}-inventory-tombstones"),"pointer":"/source_inventory/tombstone_count","type":"integer","equals":RELEASE_INVENTORY_TOMBSTONE_COUNT}),
            json!({"id":format!("{phase}-inventory-missing"),"pointer":"/source_inventory/missing","type":"integer","equals":0}),
            json!({"id":format!("{phase}-inventory-unexpected"),"pointer":"/source_inventory/unexpected","type":"integer","equals":0}),
            json!({"id":format!("{phase}-inventory-secret"),"pointer":"/source_inventory/secret","type":"integer","equals":0}),
            json!({"id":format!("{phase}-inventory-symlink"),"pointer":"/source_inventory/symlink","type":"integer","equals":0}),
            json!({"id":format!("{phase}-inventory-non-regular"),"pointer":"/source_inventory/non_regular","type":"integer","equals":0}),
            json!({"id":format!("{phase}-inventory-reserved"),"pointer":"/source_inventory/reserved","type":"integer","equals":0}),
            json!({"id":format!("{phase}-candidate-hash"),"pointer":"/candidate_hash","type":"string","matches":"^[0-9a-f]{16}$"}),
            json!({"id":format!("{phase}-result-hash"),"pointer":"/result_hash","type":"string","matches":"^[0-9a-f]{16}$"}),
            json!({"id":format!("{phase}-detached"),"pointer":"/workspace/detached","type":"boolean","equals":true}),
            json!({"id":format!("{phase}-empty"),"pointer":"/workspace/initially_empty","type":"boolean","equals":true}),
            json!({"id":format!("{phase}-excludes"),"pointer":"/workspace/source_excludes","type":"json","equals":[".git",".loom","target"]}),
            json!({"id":format!("{phase}-schema-v12"),"pointer":"/graph/schema_version","type":"integer","equals":12}),
            json!({"id":format!("{phase}-no-legacy-import"),"pointer":"/graph/legacy_imported","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-no-legacy-migration"),"pointer":"/graph/legacy_migrated","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-authority-fails-closed"),"pointer":"/graph/authority_fail_closed","type":"boolean","equals":true}),
            json!({"id":format!("{phase}-authority-not-fabricated"),"pointer":"/graph/authority_fabricated","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-exact-outer-journey"),"pointer":"/graph/outer_profile/journey_id","type":"string","equals":"release-workflow"}),
            json!({"id":format!("{phase}-exact-outer-profile"),"pointer":"/graph/outer_profile/profile","type":"string","equals":"proof"}),
            json!({"id":format!("{phase}-journey-hash"),"pointer":"/graph/outer_profile/journey_hash","type":"string","equals":journey_hash}),
            json!({"id":format!("{phase}-surface-hash"),"pointer":"/graph/outer_profile/surface_hash","type":"string","matches":"^[0-9a-f]{16}$"}),
            json!({"id":format!("{phase}-compiler-version"),"pointer":"/graph/outer_profile/compiler_version","type":"string","equals":loom::journey::JOURNEY_COMPILER_VERSION}),
            json!({"id":format!("{phase}-proof-hash"),"pointer":"/graph/outer_profile/proof_hash","type":"string","matches":"^[0-9a-f]{16}$"}),
            json!({"id":format!("{phase}-context-binding-limit"),"pointer":"/graph/outer_profile/context_binding_limit","type":"string","equals":"same-user filesystem/process isolation is not a cryptographic authority boundary"}),
            json!({"id":format!("{phase}-one-self-exclusion"),"pointer":"/graph/outer_profile/excluded_from_nested_execution","type":"boolean","equals":true}),
            json!({"id":format!("{phase}-ledger"),"pointer":"/execution_ledger","type":"json"}),
            json!({"id":format!("{phase}-cache-strategy"),"pointer":"/dependency_cache/strategy","type":"string","equals":"existing_cargo_home_read_only_verified"}),
            json!({"id":format!("{phase}-cache-unchanged"),"pointer":"/dependency_cache/unchanged","type":"boolean","equals":true}),
            json!({"id":format!("{phase}-cache-offline"),"pointer":"/dependency_cache/offline","type":"boolean","equals":true}),
            json!({"id":format!("{phase}-source-unchanged"),"pointer":"/effects/live_source_changed","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-graph-unchanged"),"pointer":"/effects/live_graph_changed","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-target-unchanged"),"pointer":"/effects/live_target_changed","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-git-unchanged"),"pointer":"/effects/live_git_changed","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-git-head-unchanged"),"pointer":"/effects/live_git_head_changed","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-git-index-unchanged"),"pointer":"/effects/live_git_index_changed","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-git-remotes-unchanged"),"pointer":"/effects/live_git_remotes_changed","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-installed-binary-unchanged"),"pointer":"/effects/installed_binary_changed","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-no-release-files"),"pointer":"/effects/release_paths_changed","type":"json","equals":[]}),
            json!({"id":format!("{phase}-argv-attempt-scope"),"pointer":"/effects/argv_attempt_scope","type":"string","equals":"direct top-level argv ledger only; descendant process containment is not claimed"}),
            json!({"id":format!("{phase}-no-top-level-install"),"pointer":"/effects/top_level_install_argv_attempted","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-no-top-level-commit"),"pointer":"/effects/top_level_commit_argv_attempted","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-no-top-level-push"),"pointer":"/effects/top_level_push_argv_attempted","type":"boolean","equals":false}),
            json!({"id":format!("{phase}-push-human-gated"),"pointer":"/policy/push_requires_explicit_human_decision","type":"boolean","equals":true}),
            json!({"id":format!("{phase}-no-bitwise-claim"),"pointer":"/policy/bitwise_reproducibility_claimed","type":"boolean","equals":false}),
        ]
    };
    let mut isolated = common("isolated_dogfood");
    isolated.extend([
        json!({"id":"isolated-ledger-candidate-plan","pointer":"/execution_ledger/0/source","type":"string","equals":"candidate_file_plan"}),
        json!({"id":"isolated-timeline","pointer":"/timeline","type":"json","equals":[{"id":"isolated_dogfood","outcome":"passed"}]})
    ]);

    let mut fixpoint = common("fresh_fixpoint");
    fixpoint.extend([
        json!({"id":"fixpoint-nonempty-rejected","pointer":"/workspace/nonempty_probe","type":"string","equals":"rejected"}),
        json!({"id":"fixpoint-preinitialized-rejected","pointer":"/workspace/preinitialized_probe","type":"string","equals":"rejected"}),
        json!({"id":"fixpoint-ledger-nonempty-source","pointer":"/execution_ledger/0/source","type":"string","equals":"empty_workspace_probe:nonempty"}),
        json!({"id":"fixpoint-ledger-nonempty-outcome","pointer":"/execution_ledger/0/outcome","type":"string","equals":"rejected"}),
        json!({"id":"fixpoint-ledger-preinitialized-source","pointer":"/execution_ledger/1/source","type":"string","equals":"empty_workspace_probe:preinitialized"}),
        json!({"id":"fixpoint-ledger-preinitialized-outcome","pointer":"/execution_ledger/1/outcome","type":"string","equals":"rejected"}),
        json!({"id":"fixpoint-ledger-candidate-plan","pointer":"/execution_ledger/2/source","type":"string","equals":"candidate_file_plan"}),
        json!({"id":"fixpoint-performed","pointer":"/fixpoint/performed","type":"boolean","equals":true}),
        json!({"id":"fixpoint-candidate-equal","pointer":"/fixpoint/candidate_hash_equal","type":"boolean","equals":true}),
        json!({"id":"fixpoint-result-equal","pointer":"/fixpoint/result_hash_equal","type":"boolean","equals":true}),
        json!({"id":"fixpoint-timeline","pointer":"/timeline","type":"json","equals":[{"id":"fresh_fixpoint","outcome":"passed"}]}),
    ]);

    let mut gated = common("gated_preparation");
    gated.extend([
        json!({"id":"gated-nonempty-rejected","pointer":"/workspace/nonempty_probe","type":"string","equals":"rejected"}),
        json!({"id":"gated-preinitialized-rejected","pointer":"/workspace/preinitialized_probe","type":"string","equals":"rejected"}),
        json!({"id":"gated-ledger-nonempty-source","pointer":"/execution_ledger/0/source","type":"string","equals":"empty_workspace_probe:nonempty"}),
        json!({"id":"gated-ledger-nonempty-outcome","pointer":"/execution_ledger/0/outcome","type":"string","equals":"rejected"}),
        json!({"id":"gated-ledger-preinitialized-source","pointer":"/execution_ledger/1/source","type":"string","equals":"empty_workspace_probe:preinitialized"}),
        json!({"id":"gated-ledger-preinitialized-outcome","pointer":"/execution_ledger/1/outcome","type":"string","equals":"rejected"}),
        json!({"id":"gated-ledger-candidate-plan","pointer":"/execution_ledger/2/source","type":"string","equals":"candidate_file_plan"}),
        json!({"id":"gated-fixpoint-performed","pointer":"/fixpoint/performed","type":"boolean","equals":true}),
        json!({"id":"gated-candidate-equal","pointer":"/fixpoint/candidate_hash_equal","type":"boolean","equals":true}),
        json!({"id":"gated-result-equal","pointer":"/fixpoint/result_hash_equal","type":"boolean","equals":true}),
        json!({"id":"gated-timeline","pointer":"/timeline","type":"json","equals":[
            {"id":"isolated_dogfood","outcome":"passed"},
            {"id":"fresh_fixpoint","outcome":"passed"},
            {"id":"mutation","outcome":"skipped_rehearsal"}
        ]}),
    ]);

    json!({
        "schema":"loom.journey.surface/v1",
        "journey_id":"release-workflow",
        "journey_hash":journey_hash,
        "surface":{
            "id":"loom-release-rehearsal",
            "title":"Detached Loom release rehearsal",
            "identity":"loom release rehearse",
            "codefile":"src/commands/release_cmd.rs",
            "locator":"rehearse_cmd",
            "operations":[
                {
                    "id":"verify-isolated-dogfood",
                    "summary":"Verify the exact candidate in one detached fresh-v12 graph",
                    "argv":["loom","release","rehearse","--phase","isolated-dogfood","--json"],
                    "environment":["CARGO_HOME","RUSTUP_HOME"],
                    "read_only":true,
                    "arguments":[],
                    "output":{"format":"json","assertions":isolated}
                },
                {
                    "id":"verify-fresh-fixpoint",
                    "summary":"Repeat the candidate verification from independent empty workspaces",
                    "argv":["loom","release","rehearse","--phase","fresh-fixpoint","--json"],
                    "environment":["CARGO_HOME","RUSTUP_HOME"],
                    "read_only":true,
                    "arguments":[],
                    "output":{"format":"json","assertions":fixpoint}
                },
                {
                    "id":"prepare-gated-local-release",
                    "summary":"Run both gates and stop before release, install, commit, or push mutation",
                    "argv":["loom","release","rehearse","--phase","gated-preparation","--json"],
                    "environment":["CARGO_HOME","RUSTUP_HOME"],
                    "read_only":true,
                    "arguments":[],
                    "output":{"format":"json","assertions":gated}
                }
            ]
        },
        "bindings":[
            {"step_id":"verify-isolated-dogfood","operation_id":"verify-isolated-dogfood"},
            {"step_id":"verify-fresh-fixpoint","operation_id":"verify-fresh-fixpoint"},
            {"step_id":"prepare-gated-local-release","operation_id":"prepare-gated-local-release"}
        ]
    })
}

struct RuntimeFixture {
    root: Tmp,
    _review_root: Tmp,
    _capsule_root: Tmp,
    cargo_cache: Tmp,
    capsule_path: PathBuf,
    outer: OuterJourneyAttestation,
}

impl RuntimeFixture {
    fn new(label: &str) -> Self {
        let root = Tmp::new();
        root.write(".gitignore", ".loom/\ntarget/\n.release-sandbox/\n");
        root.write(
            "Cargo.toml",
            &format!(
                "[package]\nname = \"ring53-{label}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"
            ),
        );
        root.write(
            "release/inventory.json",
            &serde_json::to_string_pretty(&json!({
                "schema":"loom.release-inventory/v2",
                "files":[
                    {"path":".gitignore","mode":"regular"},
                    {"path":"Cargo.toml","mode":"regular"},
                    {"path":"journeys/candidate-check.yaml","mode":"regular"},
                    {"path":"journeys/release-workflow.yaml","mode":"regular"},
                    {"path":"journeys/surfaces/release-workflow.surface.json","mode":"regular"},
                    {"path":"loom.graph.json","mode":"regular"},
                    {"path":"release/inventory.json","mode":"regular"},
                    {"path":"src/commands/release_cmd.rs","mode":"regular"},
                    {"path":"src/lib.rs","mode":"regular"},
                    {"path":"src/removed.rs","mode":"absent"}
                ],
                "reserved_components":[".git",".loom",".qoder",".reasonix",".release-sandbox","review-manifests","target"],
                "secret_name_patterns":[".env",".env.*","*.key","*.pem",".netrc",".npmrc",".pypirc","credentials","credentials.json","id_ed25519","id_rsa","secrets.json"]
            })).unwrap(),
        );
        root.write("src/lib.rs", "pub fn candidate() -> bool { true }\n");
        root.write(
            "src/commands/release_cmd.rs",
            include_str!("../src/commands/release_cmd.rs"),
        );
        root.write(
            "journeys/release-workflow.yaml",
            include_str!("../journeys/release-workflow.yaml"),
        );
        root.write(
            "journeys/candidate-check.yaml",
            r#"schema: loom.journey/v1
id: candidate-check
name: Candidate check
actor: release verifier
goal: Observe one candidate check
inputs: {}
preconditions: []
steps:
- id: check
  name: Check
  action: Check the candidate.
  expects:
  - The structured check passes.
  produces: {}
profiles:
  proof:
    inputs: {}
    workspace: {}
"#,
        );
        root.write(
            "journeys/surfaces/release-workflow.surface.json",
            include_str!("../journeys/surfaces/release-workflow.surface.json"),
        );
        assert!(ProcessCommand::new("git")
            .args(["init", "--quiet"])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success());
        common::loom_init(root.path(), Some("ring53-release"));
        builder(
            root.path(),
            &["codefile", "add", "src/commands/release_cmd.rs", "--json"],
        );
        let journey_path = root.path().join("journeys/release-workflow.yaml");
        builder(
            root.path(),
            &["journey", "add", journey_path.to_str().unwrap(), "--json"],
        );
        let manifest_path = root
            .path()
            .join("journeys/surfaces/release-workflow.surface.json");
        builder(
            root.path(),
            &[
                "journey",
                "surface-accept",
                "release-workflow",
                "--manifest",
                manifest_path.to_str().unwrap(),
                "--json",
            ],
        );
        let review_root = Tmp::new();
        review_root.write(
            "release-workflow.json",
            &serde_json::to_string_pretty(&json!({
                "schema": loom::journey::DERIVATION_SCHEMA,
                "journey_id": "release-workflow",
                "journey_hash": "8cd6742023f60b62",
                "proposal_id": "ring53-release-workflow-derivation",
                "proposal_rationale": "Bind the fixture release workflow to one technical intent.",
                "intents": [{
                    "id": "release-workflow-technical-intent",
                    "operation": "create",
                    "name": "Rehearse release gates without mutating the caller",
                    "criterion": "Detached release rehearsal runs every current Journey with exact human authority and leaves caller state unchanged.",
                    "level": "feature",
                    "visibility": "internal",
                    "rationale": "The fixture needs one complete current derivation projection.",
                    "step_ids": [
                        "verify-isolated-dogfood",
                        "verify-fresh-fixpoint",
                        "prepare-gated-local-release"
                    ]
                }],
                "relationships": [],
                "unresolved_question": null
            }))
            .unwrap(),
        );
        let derivation_path = review_root.path().join("release-workflow.json");
        builder(
            root.path(),
            &[
                "journey",
                "derive-accept",
                "release-workflow",
                "--manifest",
                derivation_path.to_str().unwrap(),
                "--human-decision",
                "Ring 53 fixture approval",
                "--json",
            ],
        );
        let store = loom::store::Store::open(root.path()).unwrap();
        loom::travel::export_to_file(&store).unwrap();
        let journey = store
            .resolve_node("release-workflow", Some(NodeType::Journey))
            .unwrap();
        let surface_hash = loom::journey::surface_projection_hash(&store, &journey)
            .unwrap()
            .unwrap();
        drop(store);

        let spec =
            loom::journey::parse(&root.path().join("journeys/release-workflow.yaml")).unwrap();
        let manifest = loom::journey::SurfaceManifest::parse_json(
            &root
                .path()
                .join("journeys/surfaces/release-workflow.surface.json"),
        )
        .unwrap();
        let proof = loom::journey_runtime::compile_surface(
            &spec,
            &surface_hash,
            "proof",
            manifest.surface.operations.clone(),
            manifest.setup.as_ref(),
            &manifest.bindings,
        )
        .unwrap();
        let capsule_root = Tmp::new();
        let cargo_cache = Tmp::new();
        for relative in ["registry/cache", "registry/index", "registry/src"] {
            std::fs::create_dir_all(cargo_cache.path().join(relative)).unwrap();
        }
        let run_id = format!("release-workflow.proof.ring53-{label}");
        let authority_store = Tmp::new();
        let environment_lock = RELEASE_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_store = std::env::var_os(loom::release::DERIVATION_AUTHORITY_STORE_ENV);
        let previous_token = std::env::var_os(loom::release::DERIVATION_AUTHORITY_TOKEN_ENV);
        std::env::set_var(
            loom::release::DERIVATION_AUTHORITY_STORE_ENV,
            authority_store.path(),
        );
        let grant = loom::release::authorize_derivations(
            root.path(),
            review_root.path(),
            "Ring 53 fixture approval".into(),
            "llm:builder",
        )
        .unwrap();
        std::env::set_var(loom::release::DERIVATION_AUTHORITY_TOKEN_ENV, &grant.token);
        let capsule_result = loom::release::write_outer_context_capsule(
            root.path(),
            capsule_root.path(),
            &spec,
            &proof,
            &run_id,
        );
        match previous_store {
            Some(value) => std::env::set_var(loom::release::DERIVATION_AUTHORITY_STORE_ENV, value),
            None => std::env::remove_var(loom::release::DERIVATION_AUTHORITY_STORE_ENV),
        }
        match previous_token {
            Some(value) => std::env::set_var(loom::release::DERIVATION_AUTHORITY_TOKEN_ENV, value),
            None => std::env::remove_var(loom::release::DERIVATION_AUTHORITY_TOKEN_ENV),
        }
        drop(environment_lock);
        let (capsule_path, capsule) = capsule_result.unwrap();
        root.write("src/removed.rs", "tracked deletion\n");
        assert!(ProcessCommand::new("git")
            .args(["add", "-A"])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success());
        std::fs::remove_file(root.path().join("src/removed.rs")).unwrap();
        Self {
            root,
            _review_root: review_root,
            _capsule_root: capsule_root,
            cargo_cache,
            capsule_path,
            outer: OuterJourneyAttestation {
                journey_id: capsule.journey_id,
                profile: capsule.profile,
                run_id: capsule.run_id,
                journey_hash: capsule.journey_hash,
                surface_hash: capsule.surface_hash,
                compiler_version: capsule.compiler_version,
                proof_hash: capsule.proof_hash,
                excluded_from_nested_execution: true,
                exclusion_reason: "exact outer fixture".into(),
                context_binding_limit: "same-user fixture boundary".into(),
            },
        }
    }

    fn activate(&self) -> ReleaseEnvironment {
        ReleaseEnvironment::install(&self.outer, &self.capsule_path, self.cargo_cache.path())
    }

    fn refresh_outer_authority(&mut self, label: &str) {
        let store = loom::store::Store::open(self.root.path()).unwrap();
        let journey = store
            .resolve_node("release-workflow", Some(NodeType::Journey))
            .unwrap();
        let surface_hash = loom::journey::surface_projection_hash(&store, &journey)
            .unwrap()
            .unwrap();
        drop(store);
        let spec =
            loom::journey::parse(&self.root.path().join("journeys/release-workflow.yaml")).unwrap();
        let manifest = loom::journey::SurfaceManifest::parse_json(
            &self
                .root
                .path()
                .join("journeys/surfaces/release-workflow.surface.json"),
        )
        .unwrap();
        let proof = loom::journey_runtime::compile_surface(
            &spec,
            &surface_hash,
            "proof",
            manifest.surface.operations.clone(),
            manifest.setup.as_ref(),
            &manifest.bindings,
        )
        .unwrap();
        let capsule_root = Tmp::new();
        let authority_store = Tmp::new();
        let run_id = format!("release-workflow.proof.ring53-{label}");
        let environment_lock = RELEASE_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_store = std::env::var_os(loom::release::DERIVATION_AUTHORITY_STORE_ENV);
        let previous_token = std::env::var_os(loom::release::DERIVATION_AUTHORITY_TOKEN_ENV);
        std::env::set_var(
            loom::release::DERIVATION_AUTHORITY_STORE_ENV,
            authority_store.path(),
        );
        let grant = loom::release::authorize_derivations(
            self.root.path(),
            self._review_root.path(),
            "Ring 53 fixture approval".into(),
            "llm:builder",
        )
        .unwrap();
        std::env::set_var(loom::release::DERIVATION_AUTHORITY_TOKEN_ENV, &grant.token);
        let (capsule_path, capsule) = loom::release::write_outer_context_capsule(
            self.root.path(),
            capsule_root.path(),
            &spec,
            &proof,
            &run_id,
        )
        .unwrap();
        match previous_store {
            Some(value) => std::env::set_var(loom::release::DERIVATION_AUTHORITY_STORE_ENV, value),
            None => std::env::remove_var(loom::release::DERIVATION_AUTHORITY_STORE_ENV),
        }
        match previous_token {
            Some(value) => std::env::set_var(loom::release::DERIVATION_AUTHORITY_TOKEN_ENV, value),
            None => std::env::remove_var(loom::release::DERIVATION_AUTHORITY_TOKEN_ENV),
        }
        drop(environment_lock);
        self._capsule_root = capsule_root;
        self.capsule_path = capsule_path;
        self.outer = OuterJourneyAttestation {
            journey_id: capsule.journey_id,
            profile: capsule.profile,
            run_id: capsule.run_id,
            journey_hash: capsule.journey_hash,
            surface_hash: capsule.surface_hash,
            compiler_version: capsule.compiler_version,
            proof_hash: capsule.proof_hash,
            excluded_from_nested_execution: true,
            exclusion_reason: "exact outer fixture".into(),
            context_binding_limit: "same-user fixture boundary".into(),
        };
    }
}

struct ReleaseEnvironment {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl ReleaseEnvironment {
    fn install(outer: &OuterJourneyAttestation, capsule: &Path, cargo_cache: &Path) -> Self {
        let lock = RELEASE_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let values = [
            (OUTER_JOURNEY_ID_ENV, outer.journey_id.as_str()),
            (OUTER_JOURNEY_PROFILE_ENV, outer.profile.as_str()),
            (OUTER_JOURNEY_RUN_ID_ENV, outer.run_id.as_str()),
            (OUTER_JOURNEY_HASH_ENV, outer.journey_hash.as_str()),
            (OUTER_SURFACE_HASH_ENV, outer.surface_hash.as_str()),
            (OUTER_COMPILER_VERSION_ENV, outer.compiler_version.as_str()),
            (OUTER_PROOF_HASH_ENV, outer.proof_hash.as_str()),
        ];
        let mut previous = Vec::new();
        for (key, value) in values {
            previous.push((key, std::env::var_os(key)));
            std::env::set_var(key, value);
        }
        previous.push((
            OUTER_CONTEXT_CAPSULE_ENV,
            std::env::var_os(OUTER_CONTEXT_CAPSULE_ENV),
        ));
        std::env::set_var(OUTER_CONTEXT_CAPSULE_ENV, capsule);
        previous.push((
            loom::release::RELEASE_CARGO_HOME_ENV,
            std::env::var_os(loom::release::RELEASE_CARGO_HOME_ENV),
        ));
        std::env::set_var(loom::release::RELEASE_CARGO_HOME_ENV, cargo_cache);
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for ReleaseEnvironment {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

struct FakeExecutor {
    outer: OuterJourneyAttestation,
    journey_report: Value,
    coverage: Value,
    drift: Value,
    calls: Vec<Vec<String>>,
    code_gate_failure: Option<CommandObservation>,
}

impl FakeExecutor {
    fn passing(fixture: &RuntimeFixture) -> Self {
        Self {
            outer: fixture.outer.clone(),
            journey_report: json!({
                "journey_id":"candidate-check",
                "profile":"proof",
                "journey_hash":"candidate-check-hash",
                "surface_hash":"candidate-surface-hash",
                "status":"passed",
                "assertions_passed":1,
                "assertions_failed":0,
                "steps":[{
                    "step_id":"check",
                    "operation_id":"check-op",
                    "argv":["loom","status","--json"],
                    "exit_code":0,
                    "output":{"ok":true},
                    "assertions_passed":1,
                    "assertions_failed":0
                }],
                "captures":{}
            }),
            coverage: json!({
                "intents":{"planned_or_needs_change":0},
                "grounding":{"ungrounded":0},
                "codefiles":{"registered":1,"owned":1,"observed":1,"unowned":0}
            }),
            drift: json!({"journeys":[{"current":true}],"stale":0}),
            calls: Vec::new(),
            code_gate_failure: None,
        }
    }
}

impl ReleaseExecutor for FakeExecutor {
    fn execute(
        &mut self,
        cwd: &Path,
        executable: &Path,
        argv: &[String],
        environment: &BTreeMap<String, String>,
    ) -> loom::Result<CommandObservation> {
        assert!(environment.get("HOME").is_some());
        assert!(environment.get("CARGO_HOME").is_some());
        assert_eq!(
            environment.get("GIT_TERMINAL_PROMPT").map(String::as_str),
            Some("0")
        );
        let temp = Path::new(environment.get("TMPDIR").expect("sandbox TMPDIR"));
        assert!(
            !temp.starts_with(cwd),
            "release child temp root must be external to its candidate cwd"
        );
        for reserved in [
            OUTER_JOURNEY_ID_ENV,
            OUTER_JOURNEY_PROFILE_ENV,
            OUTER_JOURNEY_RUN_ID_ENV,
            OUTER_JOURNEY_HASH_ENV,
            OUTER_SURFACE_HASH_ENV,
            OUTER_COMPILER_VERSION_ENV,
            OUTER_PROOF_HASH_ENV,
            OUTER_CONTEXT_CAPSULE_ENV,
            loom::release::DERIVATION_AUTHORITY_TOKEN_ENV,
            loom::release::DERIVATION_AUTHORITY_STORE_ENV,
        ] {
            assert!(
                environment.get(reserved).is_none(),
                "release child inherited runtime-only authority {reserved}"
            );
        }
        if has_sequence(argv, &["journey", "derive-accept"])
            || has_sequence(argv, &["intent", "ratify", "--all"])
        {
            assert_eq!(
                environment.get("LOOM_AGENT").map(String::as_str),
                Some("llm:builder")
            );
            assert_eq!(
                environment.get("LOOM_AGENT_PROFILE").map(String::as_str),
                Some("release-rehearsal")
            );
        } else {
            assert!(environment.get("LOOM_AGENT").is_none());
            assert!(environment.get("LOOM_AGENT_PROFILE").is_none());
        }
        self.calls.push(argv.to_vec());
        let output = if executable.file_name().and_then(|name| name.to_str()) == Some("cargo") {
            if let Some(failure) = self.code_gate_failure.take() {
                return Ok(failure);
            }
            Value::Null
        } else if has_sequence(argv, &["ignore", "list"]) {
            json!([])
        } else if has_sequence(argv, &["journey", "compile", "release-workflow"]) {
            json!({
                "compiled":true,
                "journey_id":self.outer.journey_id,
                "profile":self.outer.profile,
                "journey_hash":self.outer.journey_hash,
                "surface_hash":self.outer.surface_hash,
                "compiler_version":self.outer.compiler_version
            })
        } else if has_sequence(argv, &["journey", "run", "candidate-check"]) {
            self.journey_report.clone()
        } else if argv.iter().any(|arg| arg == "coverage") {
            self.coverage.clone()
        } else if has_sequence(argv, &["journey", "drift"]) {
            self.drift.clone()
        } else {
            json!({})
        };
        Ok(CommandObservation {
            success: true,
            exit_code: 0,
            stdout: if output.is_null() {
                Vec::new()
            } else {
                serde_json::to_vec(&output).unwrap()
            },
            stderr: Vec::new(),
        })
    }
}

fn has_sequence(argv: &[String], expected: &[&str]) -> bool {
    argv.windows(expected.len()).any(|window| {
        window
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
    })
}

fn builder(root: &Path, args: &[&str]) -> Value {
    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_loom"))
        .env("LOOM_AGENT", "llm:builder")
        .env("LOOM_NON_INTERACTIVE", "1")
        .arg("--graph")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "loom {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn fixture_root(label: &str) -> Tmp {
    let root = Tmp::new();
    root.write(
        "Cargo.toml",
        &format!("[package]\nname = \"ring53-{label}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    );
    root.write("src/lib.rs", "pub fn candidate() -> bool { true }\n");
    root.write(".loom/sentinel", "live graph bytes\n");
    root.write("target/sentinel", "live build bytes\n");
    root.write(".git/HEAD", "ref: refs/heads/main\n");
    root
}

fn release_command(root: &Path, phase: &str, with_context: bool) -> Output {
    let mut command = ProcessCommand::new(env!("CARGO_BIN_EXE_loom"));
    command
        .arg("--graph")
        .arg(root)
        .args(["release", "rehearse", "--phase", phase, "--json"])
        .env_remove("LOOM_AGENT")
        .env_remove("LOOM_AGENT_PROFILE")
        .env_remove(OUTER_JOURNEY_ID_ENV)
        .env_remove(OUTER_JOURNEY_PROFILE_ENV)
        .env_remove(OUTER_JOURNEY_RUN_ID_ENV)
        .env_remove(OUTER_JOURNEY_HASH_ENV)
        .env_remove(OUTER_SURFACE_HASH_ENV)
        .env_remove(OUTER_COMPILER_VERSION_ENV)
        .env_remove(OUTER_PROOF_HASH_ENV)
        .env_remove(OUTER_CONTEXT_CAPSULE_ENV)
        .env("LOOM_NON_INTERACTIVE", "1");
    if with_context {
        command
            .env(OUTER_JOURNEY_ID_ENV, "release-workflow")
            .env(OUTER_JOURNEY_PROFILE_ENV, "proof")
            .env(OUTER_JOURNEY_RUN_ID_ENV, "release-workflow.proof.53.1");
    }
    command.output().expect("spawn release rehearsal")
}

fn report(output: &Output) -> ReleaseRehearsalReport {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "release stdout was not one report ({error})\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_safe_effects(report: &ReleaseRehearsalReport, require_cache: bool) {
    assert!(!report.effects.live_source_changed);
    assert!(!report.effects.live_graph_changed);
    assert!(!report.effects.live_target_changed);
    assert!(!report.effects.live_git_changed);
    assert!(!report.effects.live_git_head_changed);
    assert!(!report.effects.live_git_index_changed);
    assert!(!report.effects.live_git_remotes_changed);
    assert!(!report.effects.installed_binary_changed);
    assert!(report.effects.release_paths_changed.is_empty());
    assert_eq!(
        report.effects.argv_attempt_scope,
        "direct top-level argv ledger only; descendant process containment is not claimed"
    );
    assert!(!report.effects.top_level_install_argv_attempted);
    assert!(!report.effects.top_level_commit_argv_attempted);
    assert!(!report.effects.top_level_push_argv_attempted);
    if require_cache {
        let inventory = report
            .source_inventory
            .as_ref()
            .expect("full-gate report includes source inventory provenance");
        assert_eq!(inventory.path, "release/inventory.json");
        assert!(inventory.file_count > 0);
        assert_eq!(
            inventory.entry_count,
            inventory.file_count + inventory.tombstone_count
        );
        assert_eq!(inventory.tombstone_count, 1);
        assert_eq!(inventory.manifest_hash.len(), 16);
        assert_eq!(inventory.inventory_hash.len(), 16);
        if report.status == ReleaseStatus::Passed {
            assert_eq!(
                report.candidate_hash.as_deref(),
                Some(inventory.inventory_hash.as_str())
            );
        }
        assert!(!inventory.git_influenced_plan);
        assert!(inventory.materialized_matches);
        assert_eq!(
            [
                inventory.missing,
                inventory.unexpected,
                inventory.secret,
                inventory.symlink,
                inventory.non_regular,
                inventory.reserved,
            ],
            [0; 6]
        );
        let cache = report
            .dependency_cache
            .as_ref()
            .expect("full-gate report includes dependency cache provenance");
        assert_eq!(cache.strategy, "existing_cargo_home_read_only_verified");
        assert!(cache.offline);
        assert!(cache.unchanged);
    } else {
        assert!(report.source_inventory.is_none());
        assert!(
            report.dependency_cache.is_none(),
            "early context rejection must not initialize a dependency cache"
        );
    }
    assert!(report.policy.push_requires_explicit_human_decision);
    assert!(!report.policy.bitwise_reproducibility_claimed);
}

fn assert_probe_ledger_prefix(report: &ReleaseRehearsalReport) {
    assert!(report.execution_ledger.len() >= 3);
    assert_eq!(
        report.execution_ledger[0].source,
        "empty_workspace_probe:nonempty"
    );
    assert_eq!(report.execution_ledger[0].outcome, "rejected");
    assert!(report.execution_ledger[0].attempted);
    assert_eq!(
        report.execution_ledger[1].source,
        "empty_workspace_probe:preinitialized"
    );
    assert_eq!(report.execution_ledger[1].outcome, "rejected");
    assert!(report.execution_ledger[1].attempted);
    assert_eq!(report.execution_ledger[2].source, "candidate_file_plan");
}

fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, path: &Path, rows: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(path)
            .unwrap()
            .collect::<std::io::Result<_>>()
            .unwrap();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if path.strip_prefix(root).unwrap() == Path::new(".loom/lock") {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, rows);
            } else {
                rows.push((
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    std::fs::read(path).unwrap(),
                ));
            }
        }
    }
    let mut rows = Vec::new();
    walk(root, root, &mut rows);
    rows
}
