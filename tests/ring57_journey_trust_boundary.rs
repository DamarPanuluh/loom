//! Journey trust-boundary regressions: the Store-owned guarded runtime is
//! the only way to mint trusted assertion provenance (S3-eligible Journey
//! evidence).
//!
//! Adversarial cases:
//! 1. The exact canonical proof, deserialized, executed in an
//!    attacker-controlled root containing a fake RELATIVE executable, then
//!    settled against the trusted store — must fail and remain below S3.
//! 2. The same with a PATH shim for a BARE executable name.
//! 3. Store-owned runs against attacker-owned stores where the executable is
//!    caller-selected rather than Store-approved — a PATH shim for a bare
//!    name, a relative executable symlinked outside the repository, and a
//!    self-replacing executable — must refuse and remain below S3.
//! 4. An interactive (human-gated) run paused, a covered CodeFile modified
//!    between execution and settlement — resume must refuse and settle
//!    nothing.
//! 5. The normal trusted path (approved bare tool and confined relative
//!    binary) and the interactive/resume path still pass.

use loom::journey::{
    CliOperation, HumanDecisionBinding, HumanDecisionSource, JourneySpec, OperationBinding,
    OutputAssertion, OutputFormat, SetupGraph, SurfaceBinding, SurfaceSetup, ValueType,
    JOURNEY_COMPILER_VERSION, JOURNEY_SCHEMA,
};
use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Serialize env mutations (PATH shim) against any other test mutating the
/// process environment.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// RAII restoration of the process PATH after a shim. Restores even on panic
/// so a failed assertion can never leak a mutated PATH into sibling tests.
struct PathShim(Option<std::ffi::OsString>);

impl PathShim {
    fn prepend(directory: &Path) -> Self {
        let old_path = std::env::var_os("PATH");
        let mut entries = vec![directory.to_path_buf()];
        if let Some(path) = &old_path {
            entries.extend(std::env::split_paths(path));
        }
        std::env::set_var("PATH", std::env::join_paths(entries).unwrap());
        Self(old_path)
    }
}

impl Drop for PathShim {
    fn drop(&mut self) {
        match &self.0 {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
    }
}

struct Tmp(PathBuf);

impl Tmp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "loom-journey-boundary-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn executable_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

/// A canonical store fixture whose surface operation invokes a RELATIVE
/// executable (`tools/<slug>-bin`) printing `{"ok": true}`.
struct CanonicalFixture {
    _tmp: Tmp,
    root: PathBuf,
    store: Store,
    validation_id: String,
    spec: JourneySpec,
    proof: loom::journey_runtime::CompiledJourneyProof,
    /// The exact canonical proof bytes, as persisted by the compiler.
    canonical_json: serde_json::Value,
}

fn canonical_fixture(executable_argv0: &str, label: &str) -> CanonicalFixture {
    let tmp = Tmp::new(label);
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("tools")).unwrap();
    // The TRUSTED executable: real, prints ok, stamps a sentinel proving it ran.
    let real_sentinel = root.join("real-ran");
    let real_bin = root.join(executable_argv0);
    let real_body = format!(
        "#!/usr/bin/env python3\nimport json\nfrom pathlib import Path\nPath({real_sentinel:?}).write_text('ran')\nprint(json.dumps({{'ok': True}}))\n"
    );
    executable_script(&real_bin, &real_body);

    let slug = "boundary";
    let cli_path = format!("src/{slug}-cli.rs");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join(&cli_path),
        "pub fn boundary_cli() -> &'static str { \"ok\" }\n",
    )
    .unwrap();
    let artifact = format!("journeys/{slug}.yaml");
    std::fs::create_dir_all(root.join("journeys")).unwrap();
    let spec: JourneySpec = serde_json::from_value(json!({
        "schema": JOURNEY_SCHEMA,
        "id": slug,
        "name": "Boundary",
        "actor": "operator",
        "goal": "Cross the trust boundary",
        "inputs": {},
        "preconditions": [],
        "steps": [{"id":"act","name":"Act","action":"acts","expects":[],"produces":{}}],
        "profiles": {"proof": {"inputs": {}, "workspace": {}}}
    }))
    .unwrap();
    std::fs::write(
        root.join(&artifact),
        serde_norway::to_string(&spec).unwrap(),
    )
    .unwrap();
    let journey_hash = spec.semantic_hash().unwrap();

    let store = Store::init(&root, Some("boundary"), false).unwrap();
    let journey = store
        .add_node(
            NodeType::Journey,
            slug,
            "Boundary",
            "authored",
            json!({
                "schema": JOURNEY_SCHEMA,
                "stable_id": slug,
                "artifact": artifact,
                "semantic_hash": journey_hash,
                "step_ids": ["act"],
            }),
        )
        .unwrap();
    let cli = store
        .add_node(NodeType::CodeFile, &cli_path, "", "", json!({}))
        .unwrap();
    let operation = CliOperation {
        id: "act-op".into(),
        summary: "Act".into(),
        argv: vec![executable_argv0.to_string()],
        environment: Vec::new(),
        read_only: true,
        timeout_seconds: None,
        arguments: Vec::new(),
        output: loom::journey::OperationOutput {
            format: OutputFormat::Json,
            captures: Vec::new(),
            assertions: vec![OutputAssertion {
                id: "act-ok".into(),
                pointer: "/ok".into(),
                value_type: Some(ValueType::Boolean),
                equals: Some(json!(true)),
                source: None,
            }],
            redact: Vec::new(),
        },
        exercises: Vec::new(),
    };
    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            "Boundary CLI",
            "fixture",
            "active",
            json!({
                "schema": loom::journey::INTERFACE_SURFACE_SCHEMA,
                "stable_id": "boundary-cli",
                "title": "Boundary CLI",
                "kind": "cli",
                "identity": "boundary",
                "codefile": cli_path,
                "locator": "boundary_cli",
                "operations": [operation],
            }),
        )
        .unwrap();
    let surfaces = store
        .ensure_edge(EdgeKind::Surfaces, &journey.id, &surface.id)
        .unwrap();
    store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "journey_hash",
            &journey_hash,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "operation_bindings",
            "[{\"operation_id\":\"act-op\",\"step_id\":\"act\"}]",
            TruthClass::Asserted,
        )
        .unwrap();
    let exposes = store
        .ensure_edge(EdgeKind::Exposes, &surface.id, &cli.id)
        .unwrap();
    store
        .set_facet(
            &exposes.id,
            TargetKind::Edge,
            "locator",
            "boundary_cli",
            TruthClass::Asserted,
        )
        .unwrap();

    let surface_hash = loom::journey::surface_projection_hash(&store, &journey)
        .unwrap()
        .unwrap();
    let validation = store
        .add_node(
            NodeType::Validation,
            &format!("journey:{slug}:proof"),
            "compiled Journey proof",
            "not_run",
            json!({
                "type": "journey",
                "profile": "proof",
                "journey_hash": journey_hash,
                "surface_hash": surface_hash,
                "compiler_version": JOURNEY_COMPILER_VERSION,
            }),
        )
        .unwrap();
    store
        .ensure_edge(EdgeKind::Proves, &validation.id, &journey.id)
        .unwrap();
    let exercises = store
        .ensure_edge(EdgeKind::Exercises, &validation.id, &cli.id)
        .unwrap();
    store
        .set_facet(
            &exercises.id,
            TargetKind::Edge,
            "locator",
            "boundary_cli",
            TruthClass::Asserted,
        )
        .unwrap();

    // The exact canonical proof: the same derivation settlement re-runs.
    let proof = loom::journey_runtime::compile_surface(
        &spec,
        &surface_hash,
        "proof",
        vec![operation],
        None,
        &[SurfaceBinding::Operation(OperationBinding {
            step_id: "act".into(),
            operation_id: "act-op".into(),
        })],
    )
    .unwrap();
    let canonical_json = serde_json::to_value(&proof).unwrap();

    CanonicalFixture {
        _tmp: tmp,
        root,
        store,
        validation_id: validation.id,
        spec,
        proof,
        canonical_json,
    }
}

impl CanonicalFixture {
    /// The exact canonical proof bytes, round-tripped through JSON
    /// deserialization exactly as an external caller would.
    fn deserialized_canonical_proof(&self) -> loom::journey_runtime::CompiledJourneyProof {
        serde_json::from_value(self.canonical_json.clone())
            .expect("canonical proof must deserialize")
    }

    fn grade(&self) -> String {
        let validation = self.store.get_node(&self.validation_id).unwrap().unwrap();
        let callgraph = loom::callgraph::build(&self.store).unwrap();
        let mut best: Option<loom::proofstrength::StrengthWitness> = None;
        for edge in self
            .store
            .edges_with(Some(EdgeKind::Validates), Some(&self.validation_id), None)
            .unwrap()
        {
            let witness = loom::proofstrength::grade(
                &self.store,
                &self.root,
                &validation,
                &edge.to_id,
                &callgraph,
            )
            .unwrap();
            let stronger = best
                .as_ref()
                .map(|current| {
                    loom::proofstrength::Strength::parse(&witness.grade)
                        > loom::proofstrength::Strength::parse(&current.grade)
                })
                .unwrap_or(true);
            if stronger {
                best = Some(witness);
            }
        }
        best.map(|witness| witness.grade)
            .unwrap_or_else(|| "S0".into())
    }
}

/// The trusted path earns its grade, so the adversarial tests can assert the
/// attack is what stays below S3.
fn settle_trusted(fixture: &CanonicalFixture) {
    let report = loom::journey::run_and_settle_compiled_validation(
        &fixture.store,
        &fixture.validation_id,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(report.status, loom::journey_runtime::RuntimeStatus::Passed);
}

#[test]
fn attacker_root_with_fake_relative_executable_cannot_settle_against_trusted_store() {
    let fixture = canonical_fixture("tools/boundary-bin", "relative");
    let proof = fixture.deserialized_canonical_proof();
    assert_eq!(
        loom::journey_runtime::canonical_bytes(&proof).unwrap(),
        loom::journey_runtime::canonical_bytes(&fixture.proof).unwrap(),
        "the deserialized proof must be byte-exact canonical"
    );

    // Attacker-controlled root: same authored spec, but the relative
    // executable is a fake that prints the same passing output.
    let attacker = Tmp::new("attacker-relative");
    let attacker_root = attacker.path();
    std::fs::create_dir_all(attacker_root.join("tools")).unwrap();
    std::fs::create_dir_all(attacker_root.join("journeys")).unwrap();
    let fake_sentinel = attacker_root.join("fake-ran");
    let fake_body = format!(
        "#!/usr/bin/env python3\nimport json\nfrom pathlib import Path\nPath({fake_sentinel:?}).write_text('ran')\nprint(json.dumps({{'ok': True}}))\n"
    );
    executable_script(&attacker_root.join("tools/boundary-bin"), &fake_body);
    // The exact authored spec, copied into the attacker root.
    std::fs::write(
        attacker_root.join("journeys/boundary.yaml"),
        serde_norway::to_string(&fixture.spec).unwrap(),
    )
    .unwrap();
    let attacker_spec =
        loom::journey::parse(&attacker_root.join("journeys/boundary.yaml")).unwrap();

    let observed = loom::journey_runtime::execute_observed(
        attacker_root,
        &attacker_spec,
        &proof,
        &BTreeMap::new(),
    );
    assert_eq!(
        observed.report().status,
        loom::journey_runtime::RuntimeStatus::Passed,
        "the fake executable must actually run: {:#?}",
        observed.report()
    );
    assert!(
        fake_sentinel.is_file(),
        "the attacker's fake executable must have executed"
    );
    assert!(
        !fixture.root.join("real-ran").is_file(),
        "the trusted executable must not have run"
    );

    // Settlement against the trusted store must refuse — and mint nothing.
    let err = loom::journey::settle_compiled_validation(
        &fixture.store,
        &fixture.validation_id,
        &observed,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("Store-owned guarded runtime"),
        "{err:#}"
    );
    assert_ne!(fixture.grade(), "S3", "attack must remain below S3");
    assert_eq!(
        fixture
            .store
            .get_node(&fixture.validation_id)
            .unwrap()
            .unwrap()
            .status,
        "not_run",
        "refused settlement must not change the validation"
    );
}

#[test]
fn attacker_root_with_path_shim_for_bare_executable_cannot_settle() {
    let fixture = canonical_fixture("boundary-shim-bin", "path-shim");
    let proof = fixture.deserialized_canonical_proof();

    let attacker = Tmp::new("attacker-shim");
    let attacker_root = attacker.path();
    std::fs::create_dir_all(attacker_root.join("shim")).unwrap();
    std::fs::create_dir_all(attacker_root.join("journeys")).unwrap();
    let fake_sentinel = attacker_root.join("fake-ran");
    let fake_body = format!(
        "#!/usr/bin/env python3\nimport json\nfrom pathlib import Path\nPath({fake_sentinel:?}).write_text('ran')\nprint(json.dumps({{'ok': True}}))\n"
    );
    executable_script(&attacker_root.join("shim/boundary-shim-bin"), &fake_body);
    std::fs::write(
        attacker_root.join("journeys/boundary.yaml"),
        serde_norway::to_string(&fixture.spec).unwrap(),
    )
    .unwrap();
    let attacker_spec =
        loom::journey::parse(&attacker_root.join("journeys/boundary.yaml")).unwrap();

    let observed = {
        let _serialize = ENV_LOCK.lock().unwrap();
        let shim_dir = attacker_root.join("shim");
        let old_path = std::env::var_os("PATH");
        let mut entries = vec![shim_dir];
        if let Some(path) = &old_path {
            entries.extend(std::env::split_paths(path));
        }
        std::env::set_var("PATH", std::env::join_paths(entries).unwrap());
        let observed = loom::journey_runtime::execute_observed(
            attacker_root,
            &attacker_spec,
            &proof,
            &BTreeMap::new(),
        );
        match old_path {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
        observed
    };
    assert_eq!(
        observed.report().status,
        loom::journey_runtime::RuntimeStatus::Passed,
        "the PATH shim must actually run: {:#?}",
        observed.report()
    );
    assert!(fake_sentinel.is_file(), "the PATH shim must have executed");
    assert!(
        !fixture.root.join("real-ran").is_file(),
        "the trusted executable must not have run"
    );

    let err = loom::journey::settle_compiled_validation(
        &fixture.store,
        &fixture.validation_id,
        &observed,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("Store-owned guarded runtime"),
        "{err:#}"
    );
    assert_ne!(
        fixture.grade(),
        "S3",
        "PATH-shim attack must remain below S3"
    );
    assert_eq!(
        fixture
            .store
            .get_node(&fixture.validation_id)
            .unwrap()
            .unwrap()
            .status,
        "not_run"
    );
}

#[test]
fn normal_store_owned_run_settles_and_earns_its_grade() {
    let fixture = canonical_fixture("tools/boundary-bin", "happy");
    settle_trusted(&fixture);
    assert_eq!(
        fixture
            .store
            .get_node(&fixture.validation_id)
            .unwrap()
            .unwrap()
            .status,
        "passed"
    );
    assert!(fixture.root.join("real-ran").is_file());
}

/// A refused Store-owned run must settle as a blocked observation (or refuse
/// settlement outright), never execute the attacker's binary, and never earn
/// S3.
fn assert_store_owned_refused(
    report: &loom::journey_runtime::RuntimeReport,
    fixture: &CanonicalFixture,
) {
    assert_eq!(
        report.status,
        loom::journey_runtime::RuntimeStatus::Blocked,
        "{:#?}",
        report
    );
    assert_ne!(fixture.grade(), "S3", "attack must remain below S3");
    assert_ne!(
        fixture
            .store
            .get_node(&fixture.validation_id)
            .unwrap()
            .unwrap()
            .status,
        "passed",
        "a refused attack must not leave a passing validation"
    );
}

#[test]
fn store_owned_path_shim_for_bare_executable_refuses_and_stays_below_s3() {
    let fixture = canonical_fixture("boundary-shim-bin", "store-owned-shim");
    // Attacker prepends a passing shim to PATH. The Store-owned runtime must
    // resolve the bare name through the approved toolchain boundary only, so
    // the shim is never consulted and never runs.
    let attacker = Tmp::new("store-owned-shim-dir");
    let shim_dir = attacker.path().join("shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let fake_sentinel = attacker.path().join("fake-ran");
    let fake_body = format!(
        "#!/usr/bin/env python3\nimport json\nfrom pathlib import Path\nPath({fake_sentinel:?}).write_text('ran')\nprint(json.dumps({{'ok': True}}))\n"
    );
    executable_script(&shim_dir.join("boundary-shim-bin"), &fake_body);

    let report = {
        let _serialize = ENV_LOCK.lock().unwrap();
        let _shim = PathShim::prepend(&shim_dir);
        loom::journey::run_and_settle_compiled_validation(
            &fixture.store,
            &fixture.validation_id,
            &BTreeMap::new(),
        )
        .unwrap()
    };
    assert_store_owned_refused(&report, &fixture);
    let detail = report.detail.as_deref().unwrap_or("");
    assert!(detail.contains("executable boundary"), "{detail}");
    assert!(detail.contains("boundary-shim-bin"), "{detail}");
    assert!(
        !fake_sentinel.is_file(),
        "the PATH shim must never run on the Store-owned path"
    );
    assert_eq!(
        fixture
            .store
            .get_node(&fixture.validation_id)
            .unwrap()
            .unwrap()
            .status,
        "blocked",
        "the refusal must settle as a blocked observation"
    );
}

#[test]
fn store_owned_relative_symlink_escape_refuses_and_stays_below_s3() {
    let fixture = canonical_fixture("tools/boundary-bin", "store-owned-symlink");
    // Replace the confined relative binary with a symlink that escapes the
    // repository root. The Store-owned runtime must canonicalize before the
    // spawn and refuse anything that lands outside the trusted root.
    let outside = Tmp::new("store-owned-symlink-target");
    let evil_sentinel = outside.path().join("evil-ran");
    let evil = outside.path().join("evil.py");
    executable_script(
        &evil,
        &format!(
            "#!/usr/bin/env python3\nimport json\nfrom pathlib import Path\nPath({evil_sentinel:?}).write_text('ran')\nprint(json.dumps({{'ok': True}}))\n"
        ),
    );
    std::fs::remove_file(fixture.root.join("tools/boundary-bin")).unwrap();
    std::os::unix::fs::symlink(&evil, fixture.root.join("tools/boundary-bin")).unwrap();

    let report = loom::journey::run_and_settle_compiled_validation(
        &fixture.store,
        &fixture.validation_id,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_store_owned_refused(&report, &fixture);
    let detail = report.detail.as_deref().unwrap_or("");
    assert!(detail.contains("outside the repository root"), "{detail}");
    assert!(
        !evil_sentinel.is_file(),
        "the symlinked outside executable must never run"
    );
    assert_eq!(
        fixture
            .store
            .get_node(&fixture.validation_id)
            .unwrap()
            .unwrap()
            .status,
        "blocked"
    );
}

#[test]
fn store_owned_self_replacing_executable_refuses_and_stays_below_s3() {
    let fixture = canonical_fixture("tools/boundary-bin", "store-owned-self-replace");
    // The executable prints a passing payload and then overwrites its own file
    // with different bytes before exiting. The Store-owned runtime hashed it
    // before the spawn and must refuse once the same path no longer matches.
    let path = fixture.root.join("tools/boundary-bin");
    executable_script(
        &path,
        "#!/usr/bin/env python3\nimport json\nfrom pathlib import Path\n\
         Path(__file__).write_text(\"#!/usr/bin/env python3\\nprint('replaced')\\n\")\n\
         print(json.dumps({'ok': True}))\n",
    );

    let report = loom::journey::run_and_settle_compiled_validation(
        &fixture.store,
        &fixture.validation_id,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_store_owned_refused(&report, &fixture);
    let detail = report.detail.as_deref().unwrap_or("");
    assert!(
        detail.contains("missing, replaced, or modified while it was running"),
        "{detail}"
    );
    assert_eq!(
        fixture
            .store
            .get_node(&fixture.validation_id)
            .unwrap()
            .unwrap()
            .status,
        "blocked",
        "a self-replacing executable must settle as a blocked observation"
    );
}

/// Rewrite the fixture operation to a bare approved tool (`python3` resolved
/// through the approved toolchain directories) that prints the passing JSON,
/// and keep the compiled validation's surface hash in step.
fn bare_tool_happy_fixture() -> CanonicalFixture {
    let fixture = canonical_fixture("tools/boundary-bin", "approved-bare");
    let journey = fixture
        .store
        .list_nodes(Some(NodeType::Journey), usize::MAX)
        .unwrap()
        .into_iter()
        .find(|node| node.name == fixture.spec.id)
        .expect("fixture journey node");
    let surfaces = fixture
        .store
        .edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)
        .unwrap();
    let surface_id = surfaces[0].to_id.clone();
    let mut body = fixture.store.get_node(&surface_id).unwrap().unwrap().body;
    let mut operation: CliOperation =
        serde_json::from_value(body["operations"][0].clone()).unwrap();
    operation.argv = vec![
        "python3".into(),
        "-c".into(),
        "import json; print(json.dumps({'ok': True}))".into(),
    ];
    body["operations"][0] = serde_json::to_value(&operation).unwrap();
    fixture.store.set_node_body(&surface_id, &body).unwrap();
    let new_hash = loom::journey::surface_projection_hash(&fixture.store, &journey)
        .unwrap()
        .unwrap();
    let mut validation_body = fixture
        .store
        .get_node(&fixture.validation_id)
        .unwrap()
        .unwrap()
        .body;
    validation_body["surface_hash"] = json!(new_hash);
    fixture
        .store
        .set_node_body(&fixture.validation_id, &validation_body)
        .unwrap();
    fixture
}

#[test]
fn store_owned_approved_bare_tool_and_confined_relative_binary_still_work() {
    // Confined relative binary: resolves inside the trusted root and settles.
    let relative = canonical_fixture("tools/boundary-bin", "happy-relative");
    settle_trusted(&relative);
    assert_eq!(
        relative
            .store
            .get_node(&relative.validation_id)
            .unwrap()
            .unwrap()
            .status,
        "passed"
    );
    assert!(relative.root.join("real-ran").is_file());

    // Approved bare tool: resolves through the approved toolchain directories
    // (never the caller PATH) and still runs to a passing settle.
    let bare = bare_tool_happy_fixture();
    settle_trusted(&bare);
    assert_eq!(
        bare.store
            .get_node(&bare.validation_id)
            .unwrap()
            .unwrap()
            .status,
        "passed"
    );
}

/// A store fixture whose Journey pauses at a structured human gate: step one
/// presents the prompt, step two is the host-mediated decision.
struct GateFixture {
    _tmp: Tmp,
    root: PathBuf,
    store: Store,
    validation_id: String,
    covered_file: String,
}

fn gate_fixture() -> GateFixture {
    let tmp = Tmp::new("gate");
    let root = tmp.path().to_path_buf();
    let store = Store::init(&root, Some("gate"), false).unwrap();
    let subject = store
        .add_node(
            NodeType::Intent,
            "gate subject remains wanted",
            "The current criterion still requires it.",
            "planned",
            json!({}),
        )
        .unwrap();
    let subject_id = subject.id;

    let slug = "gate.flow";
    let cli_path = "src/gate_cli.rs";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join(cli_path),
        "pub fn gate_cli() -> &'static str { \"ok\" }\n",
    )
    .unwrap();
    let artifact = "journeys/gate.flow.yaml";
    std::fs::create_dir_all(root.join("journeys")).unwrap();
    let mut spec: JourneySpec = serde_json::from_value(json!({
        "schema": JOURNEY_SCHEMA,
        "id": slug,
        "name": "Ask and record one exact human choice",
        "actor": "operator",
        "goal": "Pause at a structured host-mediated question",
        "inputs": {},
        "preconditions": [],
        "steps": [
            {
                "id": "present-decision",
                "name": "Present decision",
                "action": "present evidence and choices",
                "expects": ["a structured choice is presented"],
                "produces": {}
            },
            {
                "id": "record-human-choice",
                "name": "Record human choice",
                "action": "record the exact mediated answer",
                "expects": ["the human remains authority"],
                "produces": {}
            }
        ],
        "profiles": {"proof": {"inputs": {}, "workspace": {}}}
    }))
    .unwrap();
    let ratify_packet = json!({
        "presented": true,
        "work_item": {
            "target": {
                "kind": "intent",
                "id": subject_id,
                "name": "gate subject remains wanted"
            },
            "reason": "meaning drifted under the ratified criterion",
            "context": {"linked_entities": [{
                "role": "target",
                "kind": "intent",
                "id": subject_id,
                "name": "gate subject remains wanted",
                "description": "The current criterion still requires it."
            }]},
            "prompt_contract": {"human_gate": {
                "question": "Should the subject remain wanted?",
                "recommendation": "Recommend one option; never treat it as the decision.",
                "after_answer": "Run a generated write-back command.",
                "options": [
                    {"id": "ratify", "label": "Keep behavior", "description": "Retain the criterion.", "write_back": "loom intent ratify ..."},
                    {"id": "reject", "label": "Remove behavior", "description": "Reject the criterion.", "write_back": "loom intent reject ..."},
                    {"id": "revise", "label": "Revise criterion", "description": "Supply a corrected criterion.", "write_back": "loom intent revise ..."}
                ]
            }}
        }
    })
    .to_string();
    spec.profiles
        .get_mut("proof")
        .unwrap()
        .workspace
        .files
        .push(
            serde_json::from_value(json!({
                "path": "fixture/ratify-packet.json",
                "content": ratify_packet
            }))
            .unwrap(),
        );
    std::fs::write(root.join(artifact), serde_norway::to_string(&spec).unwrap()).unwrap();
    let journey_hash = spec.semantic_hash().unwrap();

    let journey = store
        .add_node(
            NodeType::Journey,
            slug,
            "Gate",
            "authored",
            json!({
                "schema": JOURNEY_SCHEMA,
                "stable_id": slug,
                "artifact": artifact,
                "semantic_hash": journey_hash,
                "step_ids": ["present-decision", "record-human-choice"],
            }),
        )
        .unwrap();
    let cli = store
        .add_node(NodeType::CodeFile, cli_path, "", "", json!({}))
        .unwrap();
    let present: CliOperation = serde_json::from_value(json!({
        "id": "present-decision-op",
        "summary": "Emit a structured recommendation without choosing",
        "argv": ["python3", "-c", "print(open('fixture/ratify-packet.json').read())"],
        "read_only": true,
        "arguments": [],
        "output": {
            "format": "json",
            "assertions": [{"id": "prompt-presented", "pointer": "/presented", "type": "boolean", "equals": true}]
        }
    }))
    .unwrap();
    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            "Gate CLI",
            "fixture",
            "active",
            json!({
                "schema": loom::journey::INTERFACE_SURFACE_SCHEMA,
                "stable_id": "gate-cli",
                "title": "Gate CLI",
                "kind": "cli",
                "identity": "gate",
                "codefile": cli_path,
                "locator": "gate_cli",
                "operations": [present],
            }),
        )
        .unwrap();
    let surfaces = store
        .ensure_edge(EdgeKind::Surfaces, &journey.id, &surface.id)
        .unwrap();
    store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "journey_hash",
            &journey_hash,
            TruthClass::Asserted,
        )
        .unwrap();
    let bindings = vec![
        SurfaceBinding::Operation(OperationBinding {
            step_id: "present-decision".into(),
            operation_id: "present-decision-op".into(),
        }),
        SurfaceBinding::HumanDecision(HumanDecisionBinding {
            step_id: "record-human-choice".into(),
            human_decision: HumanDecisionSource {
                operation_id: "present-decision-op".into(),
                pointer: "/work_item".into(),
            },
        }),
    ];
    store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "operation_bindings",
            &serde_json::to_string(&serde_json::to_value(&bindings).unwrap()).unwrap(),
            TruthClass::Asserted,
        )
        .unwrap();
    let setup = SurfaceSetup {
        graph: SetupGraph::LocalSnapshot,
        git: None,
        before_steps: BTreeMap::new(),
        operations: Vec::new(),
    };
    store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "setup",
            &serde_json::to_string(&setup).unwrap(),
            TruthClass::Asserted,
        )
        .unwrap();
    let exposes = store
        .ensure_edge(EdgeKind::Exposes, &surface.id, &cli.id)
        .unwrap();
    store
        .set_facet(
            &exposes.id,
            TargetKind::Edge,
            "locator",
            "gate_cli",
            TruthClass::Asserted,
        )
        .unwrap();

    let surface_hash = loom::journey::surface_projection_hash(&store, &journey)
        .unwrap()
        .unwrap();
    let validation = store
        .add_node(
            NodeType::Validation,
            &format!("journey:{slug}:proof"),
            "compiled Journey proof",
            "not_run",
            json!({
                "type": "journey",
                "profile": "proof",
                "journey_hash": journey_hash,
                "surface_hash": surface_hash,
                "compiler_version": JOURNEY_COMPILER_VERSION,
            }),
        )
        .unwrap();
    store
        .ensure_edge(EdgeKind::Proves, &validation.id, &journey.id)
        .unwrap();
    let exercises = store
        .ensure_edge(EdgeKind::Exercises, &validation.id, &cli.id)
        .unwrap();
    store
        .set_facet(
            &exercises.id,
            TargetKind::Edge,
            "locator",
            "gate_cli",
            TruthClass::Asserted,
        )
        .unwrap();

    GateFixture {
        _tmp: tmp,
        root,
        store,
        validation_id: validation.id,
        covered_file: cli_path.to_string(),
    }
}

impl GateFixture {
    fn pending(&self) -> loom::journey_gate::PendingHuman {
        match loom::journey::run_interactive_and_settle_compiled_validation(
            &self.store,
            &self.validation_id,
            &BTreeMap::new(),
        )
        .unwrap()
        {
            loom::journey::InteractiveJourneyRun::Pending(pending) => pending,
            other => panic!("expected a host-mediated pause, got {other:?}"),
        }
    }

    fn answer() -> loom::journey_gate::ResumeAnswer {
        loom::journey_gate::ResumeAnswer {
            choice_id: "ratify".into(),
            human_decision: "Keep behavior because the cited evidence is current".into(),
            free_form: None,
        }
    }
}

#[test]
fn covered_file_modified_between_execution_and_settlement_refuses_resume() {
    let fixture = gate_fixture();
    let pending = fixture.pending();
    assert_eq!(pending.binding.step_id, "record-human-choice");

    // Modify a covered CodeFile between execution (the paused run executed
    // its first step) and settlement (the resume).
    let covered = fixture.root.join(&fixture.covered_file);
    let original = std::fs::read_to_string(&covered).unwrap();
    std::fs::write(&covered, format!("{original}// drift injected\n")).unwrap();

    let err = loom::journey::resume_and_settle_compiled_validation(
        &fixture.store,
        &pending.resume_token,
        GateFixture::answer(),
        "llm:builder",
    )
    .unwrap_err();
    assert!(err.to_string().contains("covered file changed"), "{err:#}");
    assert_eq!(
        fixture
            .store
            .get_node(&fixture.validation_id)
            .unwrap()
            .unwrap()
            .status,
        "not_run",
        "refused resume must settle nothing"
    );

    // The token was not consumed: restoring the file allows the exact resume.
    std::fs::write(&covered, &original).unwrap();
    let report = match loom::journey::resume_and_settle_compiled_validation(
        &fixture.store,
        &pending.resume_token,
        GateFixture::answer(),
        "llm:builder",
    )
    .unwrap()
    {
        loom::journey::InteractiveJourneyRun::Completed(report) => report,
        other => panic!("expected completion after the answer, got {other:?}"),
    };
    assert_eq!(report.status, loom::journey_runtime::RuntimeStatus::Passed);
    assert_eq!(
        fixture
            .store
            .get_node(&fixture.validation_id)
            .unwrap()
            .unwrap()
            .status,
        "passed"
    );
}

#[test]
fn interactive_and_resume_path_settles_under_the_trusted_boundary() {
    let fixture = gate_fixture();
    let pending = fixture.pending();
    assert_eq!(pending.binding.step_id, "record-human-choice");
    let report = match loom::journey::resume_and_settle_compiled_validation(
        &fixture.store,
        &pending.resume_token,
        GateFixture::answer(),
        "llm:builder",
    )
    .unwrap()
    {
        loom::journey::InteractiveJourneyRun::Completed(report) => report,
        other => panic!("expected completion after the answer, got {other:?}"),
    };
    assert_eq!(report.status, loom::journey_runtime::RuntimeStatus::Passed);
    assert_eq!(report.assertions_passed, 2);
    assert_eq!(
        fixture
            .store
            .get_node(&fixture.validation_id)
            .unwrap()
            .unwrap()
            .status,
        "passed",
        "resume must settle the validation"
    );
    // The human decision is journaled once, against the validation.
    let journal = loom::journal::read(&fixture.root).unwrap();
    let decisions: Vec<_> = journal
        .iter()
        .filter(|entry| entry.event == "journey_human_decision")
        .collect();
    assert_eq!(decisions.len(), 1);
}
