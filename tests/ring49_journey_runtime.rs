//! Ring 49 — compiled semantic Journey runtime.

use loom::journey::{
    CliOperation, HumanDecisionBinding, HumanDecisionSource, InterfaceSurfaceDefinition,
    JourneySpec, OperationBinding, OperationOutput, OutputAssertion, OutputCapture, OutputFormat,
    SetupGraph, SurfaceBinding, SurfaceFileAction, SurfaceGitMode, SurfaceGitSetup,
    SurfaceManifest, SurfaceSetup, ValueType, JOURNEY_SCHEMA, SURFACE_SCHEMA,
};
use loom::journey_gate::ResumeAnswer;
use loom::journey_runtime::{self, ExecutionOutcome, RuntimeStatus};
use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "loom-ring49-{label}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn relative_path_from(base: &Path, target: &Path) -> PathBuf {
    let base = base.canonicalize().unwrap();
    let target = target.canonicalize().unwrap();
    let base: Vec<_> = base
        .components()
        .map(|component| component.as_os_str().to_os_string())
        .collect();
    let target: Vec<_> = target
        .components()
        .map(|component| component.as_os_str().to_os_string())
        .collect();
    let shared = base
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = PathBuf::new();
    for _ in shared..base.len() {
        relative.push("..");
    }
    for component in &target[shared..] {
        relative.push(component);
    }
    relative
}

#[cfg(unix)]
fn build_environment_probe(root: &Path) -> PathBuf {
    let source = root.join("environment_probe.c");
    let binary = root.join("environment_probe");
    std::fs::write(
        &source,
        r#"#include <stdio.h>
#include <stdlib.h>
#include <string.h>
extern char **environ;
static int compare(const void *left, const void *right) {
    return strcmp(*(const char * const *)left, *(const char * const *)right);
}
int main(void) {
    size_t count = 0;
    while (environ[count] != NULL) count++;
    qsort(environ, count, sizeof(char *), compare);
    fputs("{\"ok\":true,\"keys\":[", stdout);
    for (size_t index = 0; index < count; index++) {
        const char *separator = strchr(environ[index], '=');
        if (index > 0) putchar(',');
        printf("\"%.*s\"", (int)(separator - environ[index]), environ[index]);
    }
    fputs("]}\n", stdout);
    return 0;
}
"#,
    )
    .unwrap();
    let status = Command::new("cc")
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .status()
        .unwrap();
    assert!(status.success(), "environment probe must compile");
    binary
}

fn spec() -> JourneySpec {
    serde_json::from_value(json!({
        "schema": JOURNEY_SCHEMA,
        "id": "checkout.happy",
        "name": "Checkout succeeds",
        "actor": "shopper",
        "goal": "Complete checkout",
        "inputs": {
          "secret": {
            "type": "string",
            "description": "A sensitive checkout token",
            "secret": true
          }
        },
        "preconditions": [],
        "steps": [{
            "id": "checkout",
            "name": "Checkout",
            "action": "checks out",
            "expects": ["the checkout is recorded"],
            "produces": {"checkout-token":{"type":"string","description":"Redacted token"}}
        }],
        "profiles": {
            "proof": {
                "inputs": {"secret": {"env":"LOOM_RING49_SECRET"}},
                "workspace": {
                    "files": [{"path": "fixture/value.txt", "content": "ready"}]
                }
            }
        }
    }))
    .unwrap()
}

fn operation() -> CliOperation {
    CliOperation {
        id: "checkout-op".into(),
        summary: "Execute checkout".into(),
        argv: vec![
            "python3".into(),
            "-c".into(),
            concat!(
                "import json,os; ",
                "ready=open('fixture/value.txt').read()=='ready'; ",
                "print(json.dumps({'ok':ready,'secret':os.environ['LOOM_RING49_SECRET'],'undeclared':os.environ.get('LOOM_RING49_UNDECLARED')}))"
            )
            .into(),
        ],
        environment: Vec::new(),
        read_only: false,
        timeout_seconds: None,
        arguments: vec![],
        output: OperationOutput {
            format: OutputFormat::Json,
            captures: vec![OutputCapture {
                id: "checkout-token".into(),
                pointer: "/secret".into(),
                value_type: ValueType::String,
                redact: true,
            }],
            assertions: vec![OutputAssertion {
                id: "fixture-ready".into(),
                pointer: "/ok".into(),
                value_type: Some(ValueType::Boolean),
                equals: Some(json!(true)),
                source: None,
            }],
            redact: vec!["/secret".into()],
        },
        exercises: Vec::new(),
    }
}

fn setup_operation(id: &str) -> CliOperation {
    CliOperation {
        id: id.into(),
        summary: "Prepare an isolated fixture".into(),
        argv: vec![
            "python3".into(),
            "-c".into(),
            "import json; print(json.dumps({'ready': True}))".into(),
        ],
        environment: Vec::new(),
        read_only: false,
        timeout_seconds: None,
        arguments: vec![],
        output: OperationOutput {
            format: OutputFormat::Json,
            captures: vec![],
            assertions: vec![OutputAssertion {
                id: "fixture-ready".into(),
                pointer: "/ready".into(),
                value_type: Some(ValueType::Boolean),
                equals: Some(json!(true)),
                source: None,
            }],
            redact: vec![],
        },
        exercises: Vec::new(),
    }
}

fn compiled() -> journey_runtime::CompiledJourneyProof {
    journey_runtime::compile(
        &spec(),
        "surface-hash",
        "proof",
        vec![operation()],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap()
}

#[test]
fn compiled_timeout_resolves_profile_default_and_operation_override_into_proof() {
    let authored = spec();
    let default_proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![operation()],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    assert_eq!(default_proof.steps[0].timeout_seconds, Some(2700));

    let semantic_hash = authored.semantic_hash().unwrap();
    let default_bytes = journey_runtime::canonical_bytes(&default_proof).unwrap();
    let mut overridden = operation();
    overridden.timeout_seconds = Some(17);
    let override_proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![overridden],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    assert_eq!(override_proof.steps[0].timeout_seconds, Some(17));
    assert_eq!(authored.semantic_hash().unwrap(), semantic_hash);
    assert_ne!(
        journey_runtime::canonical_bytes(&override_proof).unwrap(),
        default_bytes
    );

    let mut zero = operation();
    zero.timeout_seconds = Some(0);
    let error = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![zero],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap_err();
    assert!(error.to_string().contains("positive"));
}

#[test]
fn compilation_is_deterministic_and_contains_no_profile_secret() {
    let first = compiled();
    let second = compiled();
    let first_bytes = journey_runtime::canonical_bytes(&first).unwrap();
    let second_bytes = journey_runtime::canonical_bytes(&second).unwrap();
    assert_eq!(first_bytes, second_bytes);
    assert!(!String::from_utf8(first_bytes)
        .unwrap()
        .contains("do-not-persist"));
}

#[test]
fn declared_environment_is_canonical_and_cache_significant_without_values() {
    let authored = spec();
    let bindings = [OperationBinding {
        step_id: "checkout".into(),
        operation_id: "checkout-op".into(),
    }];
    let mut first_operation = operation();
    first_operation.environment = vec!["RUSTUP_HOME".into(), "CARGO_HOME".into()];
    let mut reordered_operation = first_operation.clone();
    reordered_operation.environment.reverse();
    let first = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![first_operation],
        &bindings,
    )
    .unwrap();
    let reordered = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![reordered_operation],
        &bindings,
    )
    .unwrap();
    let without = compiled();
    assert_eq!(first.steps[0].environment, ["CARGO_HOME", "RUSTUP_HOME"]);
    assert_eq!(
        journey_runtime::canonical_bytes(&first).unwrap(),
        journey_runtime::canonical_bytes(&reordered).unwrap()
    );
    assert_ne!(
        journey_runtime::canonical_bytes(&first).unwrap(),
        journey_runtime::canonical_bytes(&without).unwrap()
    );

    let artifact = String::from_utf8(journey_runtime::canonical_bytes(&first).unwrap()).unwrap();
    assert!(artifact.contains("CARGO_HOME"));
    assert!(artifact.contains("RUSTUP_HOME"));
    assert!(!artifact.contains("ring49-cargo-value"));
    assert!(!artifact.contains("ring49-rustup-value"));

    let root = TempRoot::new("environment-cache");
    journey_runtime::write_proof(root.path(), &without).unwrap();
    assert!(!journey_runtime::cache_matches(root.path(), &first).unwrap());
}

#[test]
fn executor_inherits_only_declared_host_environment_with_profile_precedence() {
    let host_homes = TempRoot::new("declared-environment-host-homes");
    let cargo_host = host_homes.path().join("cargo-home");
    let rustup_host = host_homes.path().join("rustup-home");
    let cargo_profile = host_homes.path().join("profile-cargo-home");
    for path in [&cargo_host, &rustup_host, &cargo_profile] {
        std::fs::create_dir_all(path).unwrap();
    }
    let previous_cargo = std::env::var_os("CARGO_HOME");
    let previous_rustup = std::env::var_os("RUSTUP_HOME");
    let previous_undeclared = std::env::var_os("LOOM_RING49_UNDECLARED");
    std::env::set_var("CARGO_HOME", &cargo_host);
    std::env::set_var("RUSTUP_HOME", &rustup_host);
    std::env::set_var("LOOM_RING49_UNDECLARED", "ring49-secret-sentinel");
    std::env::set_var("LOOM_RING49_SECRET", "do-not-persist");

    let root = TempRoot::new("declared-environment");
    let mut authored = spec();
    authored.steps[0].produces.clear();
    let workspace_env = &mut authored.profiles.get_mut("proof").unwrap().workspace.env;
    workspace_env.insert(
        "CARGO_HOME".into(),
        cargo_profile.to_string_lossy().into_owned(),
    );
    workspace_env.insert(
        "EXPECTED_CARGO_HOME".into(),
        cargo_profile.to_string_lossy().into_owned(),
    );
    workspace_env.insert(
        "EXPECTED_RUSTUP_HOME".into(),
        rustup_host.to_string_lossy().into_owned(),
    );
    let mut declared = operation();
    declared.output.captures.clear();
    declared.environment = vec!["CARGO_HOME".into(), "RUSTUP_HOME".into()];
    declared.argv = vec![
        "python3".into(),
        "-c".into(),
        concat!(
            "import json,os; ",
            "cargo=os.environ.get('CARGO_HOME'); rustup=os.environ.get('RUSTUP_HOME'); ",
            "print(json.dumps({'ok':cargo==os.environ['EXPECTED_CARGO_HOME'] and rustup==os.environ['EXPECTED_RUSTUP_HOME'],",
            "'cargo':cargo,'rustup':rustup,'undeclared':os.environ.get('LOOM_RING49_UNDECLARED')}))"
        )
        .into(),
    ];
    let proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![declared],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());

    match previous_cargo {
        Some(value) => std::env::set_var("CARGO_HOME", value),
        None => std::env::remove_var("CARGO_HOME"),
    }
    match previous_rustup {
        Some(value) => std::env::set_var("RUSTUP_HOME", value),
        None => std::env::remove_var("RUSTUP_HOME"),
    }
    match previous_undeclared {
        Some(value) => std::env::set_var("LOOM_RING49_UNDECLARED", value),
        None => std::env::remove_var("LOOM_RING49_UNDECLARED"),
    }

    assert_eq!(report.status, RuntimeStatus::Passed, "{report:#?}");
    assert_eq!(report.steps[0].output["cargo"], "[REDACTED]");
    assert_eq!(report.steps[0].output["rustup"], "[REDACTED]");
    assert_eq!(report.steps[0].output["undeclared"], Value::Null);
    let serialized = serde_json::to_string(&report).unwrap();
    assert!(!serialized.contains(&cargo_profile.to_string_lossy().into_owned()));
    assert!(!serialized.contains(&rustup_host.to_string_lossy().into_owned()));
    assert!(!serialized.contains("ring49-secret-sentinel"));
}

#[test]
fn missing_declared_environment_blocks_with_name_only() {
    let missing = format!("LOOM_RING49_MISSING_{}", std::process::id());
    std::env::remove_var(&missing);
    std::env::set_var("LOOM_RING49_SECRET", "do-not-persist");
    let root = TempRoot::new("missing-declared-environment");
    let mut authored = spec();
    authored.steps[0].produces.clear();
    let mut declared = operation();
    declared.output.captures.clear();
    declared.environment = vec![missing.clone()];
    let proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![declared],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
    assert_eq!(report.status, RuntimeStatus::Blocked, "{report:#?}");
    let detail = report.detail.unwrap();
    assert!(detail.contains(&missing), "{detail}");
    assert!(!detail.contains("do-not-persist"), "{detail}");
    assert!(report.steps.is_empty());
}

#[test]
fn declared_environment_values_are_redacted_from_child_errors() {
    let name = format!("LOOM_RING49_ERROR_SECRET_{}", std::process::id());
    let value = "ring49-error-value-must-not-leak";
    std::env::set_var(&name, value);
    std::env::set_var("LOOM_RING49_SECRET", "do-not-persist");
    let root = TempRoot::new("declared-environment-error");
    let mut authored = spec();
    authored.steps[0].produces.clear();
    let mut declared = operation();
    declared.output.captures.clear();
    declared.environment = vec![name.clone()];
    declared.argv = vec![
        "python3".into(),
        "-c".into(),
        format!("import os,sys; sys.stderr.write(os.environ[{name:?}]); sys.exit(7)"),
    ];
    let proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![declared],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
    std::env::remove_var(&name);
    assert_eq!(report.status, RuntimeStatus::Blocked, "{report:#?}");
    let detail = report.detail.unwrap();
    assert!(detail.contains("[REDACTED]"), "{detail}");
    assert!(!detail.contains(value), "{detail}");
}

#[test]
fn failed_child_retains_bounded_stdout_panic_and_stderr_summary() {
    let name = format!("LOOM_RING49_PANIC_SECRET_{}", std::process::id());
    let value = "ring49-panic-key-must-not-leak";
    std::env::set_var(&name, value);
    std::env::set_var("LOOM_RING49_SECRET", "do-not-persist");
    let root = TempRoot::new("failed-child-diagnostics");
    let mut authored = spec();
    authored.steps[0].produces.clear();
    let mut declared = operation();
    declared.output.captures.clear();
    declared.environment = vec![name.clone()];
    declared.argv = vec![
        "python3".into(),
        "-c".into(),
        format!(
            "import os,sys; secret=os.environ[{name:?}]; \
             sys.stdout.write(\"thread 'checkout_panics_in_stdout' panicked at checkout.py:7\\n\" + secret + \": panic\\n\" + \"x\"*(160*1024)); \
             sys.stderr.write(\"test result: FAILED. 0 passed; 1 failed\\n\"); sys.exit(7)"
        ),
    ];
    let proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![declared],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
    std::env::remove_var(&name);
    assert_eq!(report.status, RuntimeStatus::Blocked, "{report:#?}");
    assert_eq!(report.assertions_failed, 0);
    assert!(report.steps.is_empty());
    let detail = report.detail.unwrap();
    assert!(detail.contains("stdout:"), "{detail}");
    assert!(detail.contains("checkout_panics_in_stdout"), "{detail}");
    assert!(detail.contains("stderr:"), "{detail}");
    assert!(detail.contains("test result: FAILED"), "{detail}");
    assert!(detail.contains("diagnostic output omitted"), "{detail}");
    assert!(detail.contains("[REDACTED]"), "{detail}");
    assert!(!detail.contains(value), "{detail}");
    assert!(detail.len() < 140 * 1024, "diagnostic was not bounded");
}

#[test]
fn failed_child_preserves_structured_release_blocked_stdout_redacted() {
    let name = format!("LOOM_RING49_STRUCTURED_SECRET_{}", std::process::id());
    let value = "ring49-structured-key-must-not-leak";
    std::env::set_var(&name, value);
    std::env::set_var("LOOM_RING49_SECRET", "do-not-persist");
    let root = TempRoot::new("structured-release-blocked");
    let mut authored = spec();
    authored.steps[0].produces.clear();
    let mut declared = operation();
    declared.output.captures.clear();
    declared.environment = vec![name.clone()];
    declared.argv = vec![
        "python3".into(),
        "-c".into(),
        format!(
            "import json,os,sys; secret=os.environ[{name:?}]; \
             json.dump({{'schema':'loom.release-rehearsal/v1','status':'blocked','detail':'structured code gate failed','diagnostic':{{secret:'hidden'}}}},sys.stdout); \
             sys.stderr.write('release rehearsal blocked\\n'); sys.exit(9)"
        ),
    ];
    let proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![declared],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
    std::env::remove_var(&name);
    assert_eq!(report.status, RuntimeStatus::Blocked, "{report:#?}");
    assert_eq!(report.assertions_failed, 0);
    assert!(report.steps.is_empty());
    let detail = report.detail.unwrap();
    assert!(detail.contains("stdout:"), "{detail}");
    assert!(detail.contains("loom.release-rehearsal/v1"), "{detail}");
    assert!(detail.contains("structured code gate failed"), "{detail}");
    assert!(detail.contains("stderr:"), "{detail}");
    assert!(detail.contains("release rehearsal blocked"), "{detail}");
    assert!(detail.contains("[REDACTED]"), "{detail}");
    assert!(!detail.contains(value), "{detail}");
}

#[test]
fn declared_environment_values_are_redacted_from_nested_json_keys_collision_safely() {
    let name = format!("LOOM_RING49_KEY_SECRET_{}", std::process::id());
    let value = "/private/ring49/secret-path";
    std::env::set_var(&name, value);
    std::env::set_var("LOOM_RING49_SECRET", "do-not-persist");
    let root = TempRoot::new("declared-environment-key");
    let mut authored = spec();
    authored.steps[0].produces.clear();
    let mut declared = operation();
    declared.output.captures.clear();
    declared.environment = vec![name.clone()];
    declared.argv = vec![
        "python3".into(),
        "-c".into(),
        format!(
            concat!(
                "import json,os; secret=os.environ[{:?}]; ",
                "print(json.dumps({{'ok':True,'objects':{{'[REDACTED]':'unrelated',secret:'secret-key-value'}},",
                "'nested':{{'prefix-'+secret+'-suffix':{{secret:'nested-secret-key'}}}}}}))"
            ),
            name
        ),
    ];
    let proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![declared],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
    std::env::remove_var(&name);
    assert_eq!(report.status, RuntimeStatus::Passed, "{report:#?}");
    assert_eq!(report.steps[0].output["objects"]["[REDACTED]"], "unrelated");
    assert_eq!(
        report.steps[0].output["objects"]["[REDACTED]#2"],
        "secret-key-value"
    );
    assert_eq!(
        report.steps[0].output["nested"]["prefix-[REDACTED]-suffix"]["[REDACTED]"],
        "nested-secret-key"
    );
    let serialized_report = serde_json::to_string(&report).unwrap();
    assert!(!serialized_report.contains(value));
    let evidence = journey_runtime::JourneyBaseline {
        schema: loom::journey::BASELINE_SCHEMA.into(),
        compiler_version: loom::journey::JOURNEY_COMPILER_VERSION.into(),
        journey_id: authored.id.clone(),
        journey_hash: authored.semantic_hash().unwrap(),
        surface_hash: proof.surface_hash.clone(),
        profile: "proof".into(),
        report,
    };
    assert!(!serde_json::to_string(&evidence).unwrap().contains(value));
}

#[cfg(unix)]
#[test]
fn non_unicode_declared_environment_blocks_without_raw_bytes() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let name = format!("LOOM_RING49_NON_UNICODE_{}", std::process::id());
    let previous = std::env::var_os(&name);
    std::env::set_var(
        &name,
        OsString::from_vec(b"ring49-non-unicode-\xff-value".to_vec()),
    );
    std::env::set_var("LOOM_RING49_SECRET", "do-not-persist");
    let root = TempRoot::new("non-unicode-declared-environment");
    let mut authored = spec();
    authored.steps[0].produces.clear();
    let mut declared = operation();
    declared.output.captures.clear();
    declared.environment = vec![name.clone()];
    let proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![declared],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
    match previous {
        Some(value) => std::env::set_var(&name, value),
        None => std::env::remove_var(&name),
    }
    assert_eq!(report.status, RuntimeStatus::Blocked, "{report:#?}");
    let serialized = serde_json::to_string(&report).unwrap();
    assert!(serialized.contains(&name));
    assert!(serialized.contains("not valid UTF-8"));
    assert!(!serialized.contains("ring49-non-unicode"));
    assert!(report.steps.is_empty());
}

#[cfg(unix)]
#[test]
fn child_environment_is_exactly_explicit_plus_executor_infrastructure() {
    let declared_name = format!("LOOM_RING49_DECLARED_{}", std::process::id());
    let ambient_name = format!("LOOM_RING49_AMBIENT_{}", std::process::id());
    std::env::set_var(&declared_name, "declared-value");
    std::env::set_var(&ambient_name, "ambient-value-must-not-leak");
    std::env::set_var("LOOM_RING49_SECRET", "do-not-persist");
    let root = TempRoot::new("exact-child-environment");
    let probe = build_environment_probe(root.path());
    let mut authored = spec();
    authored.steps[0].produces.clear();
    authored
        .profiles
        .get_mut("proof")
        .unwrap()
        .workspace
        .env
        .insert("LOOM_RING49_PROFILE".into(), "profile-value".into());
    let mut declared = operation();
    declared.output.captures.clear();
    declared.environment = vec![declared_name.clone()];
    declared.argv = vec![probe.to_string_lossy().into_owned()];
    let proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![declared],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
    std::env::remove_var(&declared_name);
    std::env::remove_var(&ambient_name);
    assert_eq!(report.status, RuntimeStatus::Passed, "{report:#?}");

    let mut expected = vec![
        "LOOM_NON_INTERACTIVE".to_string(),
        "LOOM_RING49_PROFILE".to_string(),
        "LOOM_RING49_SECRET".to_string(),
        declared_name,
    ];
    expected.extend(
        journey_runtime::EXECUTOR_PLATFORM_ENVIRONMENT
            .iter()
            .filter(|name| std::env::var_os(name).is_some())
            .map(|name| (*name).to_string()),
    );
    expected.sort();
    expected.dedup();
    assert_eq!(report.steps[0].output["keys"], json!(expected));
    assert!(!serde_json::to_string(&report)
        .unwrap()
        .contains(&ambient_name));
}

#[test]
fn executor_platform_environment_allowlist_is_fixed_and_narrow() {
    #[cfg(not(windows))]
    assert_eq!(
        journey_runtime::EXECUTOR_PLATFORM_ENVIRONMENT,
        ["PATH", "TMPDIR", "TEMP", "TMP"]
    );
    #[cfg(windows)]
    assert_eq!(
        journey_runtime::EXECUTOR_PLATFORM_ENVIRONMENT,
        [
            "PATH",
            "TMPDIR",
            "TEMP",
            "TMP",
            "SYSTEMROOT",
            "WINDIR",
            "PATHEXT",
            "COMSPEC"
        ]
    );
}

#[test]
fn cache_integrity_detects_missing_and_tampered_artifacts() {
    let root = TempRoot::new("cache");
    let proof = compiled();
    assert!(!journey_runtime::cache_matches(root.path(), &proof).unwrap());
    let path = journey_runtime::write_proof(root.path(), &proof).unwrap();
    assert_eq!(
        path,
        root.path()
            .join(".loom/compiled/journeys/checkout.happy/proof.proof.json")
    );
    assert!(journey_runtime::cache_matches(root.path(), &proof).unwrap());
    std::fs::write(&path, b"{\"tampered\":true}\n").unwrap();
    assert!(!journey_runtime::cache_matches(root.path(), &proof).unwrap());
}

#[test]
fn executor_uses_temp_setup_direct_argv_json_checks_and_redaction() {
    std::env::set_var("LOOM_RING49_SECRET", "do-not-persist");
    std::env::set_var("LOOM_RING49_UNDECLARED", "must-not-leak");
    let root = TempRoot::new("execute");
    let proof = compiled();
    let report = journey_runtime::execute(root.path(), &spec(), &proof, &BTreeMap::new());
    assert_eq!(report.status, RuntimeStatus::Passed, "{report:#?}");
    assert_eq!(report.assertions_passed, 1);
    assert_eq!(report.assertions_failed, 0);
    assert_eq!(report.steps.len(), 1);
    assert!(!report.steps[0]
        .argv
        .iter()
        .any(|part| part == "do-not-persist"));
    assert_eq!(report.steps[0].output["secret"], "[REDACTED]");
    assert_eq!(report.steps[0].output["undeclared"], Value::Null);
    assert_eq!(
        report.captures["steps.checkout.outputs.checkout-token"],
        "[REDACTED]"
    );
    assert!(!root
        .path()
        .join(".loom/tmp")
        .read_dir()
        .unwrap()
        .any(|_| true));
}

#[test]
fn release_only_outer_context_is_absent_from_ordinary_journeys() {
    std::env::set_var("LOOM_RING49_SECRET", "do-not-persist");
    let root = TempRoot::new("reserved-outer-context");
    let mut authored = spec();
    authored.steps[0].produces.clear();
    let mut operation = operation();
    operation.read_only = true;
    operation.output.captures.clear();
    operation.output.redact.clear();
    operation.output.assertions = vec![OutputAssertion {
        id: "release-context-is-absent".into(),
        pointer: "/outer_present".into(),
        value_type: Some(ValueType::Boolean),
        equals: Some(json!(false)),
        source: None,
    }];
    let reserved = [
        loom::release::OUTER_CONTEXT_CAPSULE_ENV,
        loom::release::OUTER_JOURNEY_ID_ENV,
        loom::release::OUTER_JOURNEY_PROFILE_ENV,
        loom::release::OUTER_JOURNEY_RUN_ID_ENV,
        loom::release::OUTER_JOURNEY_HASH_ENV,
        loom::release::OUTER_SURFACE_HASH_ENV,
        loom::release::OUTER_COMPILER_VERSION_ENV,
        loom::release::OUTER_PROOF_HASH_ENV,
    ];
    operation.argv = vec![
        "python3".into(),
        "-c".into(),
        format!(
            "import json,os; reserved={reserved:?}; print(json.dumps({{'outer_present':any(name in os.environ for name in reserved)}}))",
        ),
    ];
    let proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![operation],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let relative_root = relative_path_from(&std::env::current_dir().unwrap(), root.path());
    assert!(!relative_root.is_absolute());
    let report = journey_runtime::execute(&relative_root, &authored, &proof, &BTreeMap::new());
    assert_eq!(report.status, RuntimeStatus::Passed, "{report:#?}");
    assert_eq!(report.steps[0].output["outer_present"], false);
}

#[test]
fn invalid_json_and_missing_executable_are_blocked() {
    std::env::set_var("LOOM_RING49_SECRET", "do-not-persist");
    let root = TempRoot::new("outcomes");
    let mut invalid = operation();
    invalid.argv = vec!["python3".into(), "-c".into(), "print('not-json')".into()];
    invalid.arguments.clear();
    let invalid_proof = journey_runtime::compile(
        &spec(),
        "surface-hash",
        "proof",
        vec![invalid],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let blocked = journey_runtime::execute(root.path(), &spec(), &invalid_proof, &BTreeMap::new());
    assert_eq!(blocked.status, RuntimeStatus::Blocked);

    let mut missing = operation();
    missing.argv = vec!["definitely-not-a-real-ring49-command".into()];
    let missing_proof = journey_runtime::compile(
        &spec(),
        "surface-hash",
        "proof",
        vec![missing],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let blocked = journey_runtime::execute(root.path(), &spec(), &missing_proof, &BTreeMap::new());
    assert_eq!(blocked.status, RuntimeStatus::Blocked);
}

#[test]
fn secret_environment_parse_errors_are_blocked_without_disclosure() {
    let raw_secret = "definitely-not-an-integer-secret";
    std::env::set_var("LOOM_RING49_SECRET", raw_secret);
    let root = TempRoot::new("secret-parse");
    let mut typed = spec();
    typed.inputs.get_mut("secret").unwrap().value_type = ValueType::Integer;
    let proof = journey_runtime::compile(
        &typed,
        "surface-hash",
        "proof",
        vec![operation()],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let report = journey_runtime::execute(root.path(), &typed, &proof, &BTreeMap::new());
    assert_eq!(report.status, RuntimeStatus::Blocked);
    let detail = report.detail.unwrap_or_default();
    assert!(!detail.contains(raw_secret), "secret leaked in: {detail}");
    assert!(detail.contains("wrong type"), "{detail}");
}

#[test]
fn mutable_loom_operation_is_confined_to_a_temporary_graph() {
    std::env::set_var("LOOM_RING49_SECRET", "do-not-persist");
    let root = TempRoot::new("mutable-graph");
    let live = Store::init(root.path(), Some("live graph"), false).unwrap();
    let mut mutable = operation();
    mutable.argv = vec![
        env!("CARGO_BIN_EXE_loom").into(),
        "intent".into(),
        "add".into(),
        "--name".into(),
        "temporary proof mutation".into(),
        "--description".into(),
        "created only inside the proof workspace".into(),
        "--json".into(),
    ];
    mutable.output.captures.clear();
    mutable.output.redact.clear();
    mutable.output.assertions = vec![OutputAssertion {
        id: "intent-created".into(),
        pointer: "/intent/name".into(),
        value_type: Some(ValueType::String),
        equals: Some(json!("temporary proof mutation")),
        source: None,
    }];
    let mut authored = spec();
    authored.steps[0].produces.clear();
    let proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![mutable],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
    assert_eq!(report.status, RuntimeStatus::Passed, "{report:#?}");
    assert!(live
        .list_nodes(Some(NodeType::Intent), usize::MAX)
        .unwrap()
        .is_empty());
}

#[test]
fn policy_derives_loom_mutation_and_rechecks_cached_read_only_tampering() {
    let mut authored = spec();
    authored.steps[0].produces.clear();
    let mut mutation = operation();
    mutation.argv = vec!["loom".into(), "sync".into(), "--json".into()];
    mutation.output.captures.clear();
    mutation.output.redact.clear();
    mutation.read_only = true;
    let bindings = [OperationBinding {
        step_id: "checkout".into(),
        operation_id: "checkout-op".into(),
    }];
    let error = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![mutation.clone()],
        &bindings,
    )
    .unwrap_err();
    assert!(error.to_string().contains("marked read_only"));

    mutation.read_only = false;
    let mut proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![mutation],
        &bindings,
    )
    .unwrap();
    proof.steps[0].read_only = true;
    let root = TempRoot::new("policy-cache-tamper");
    Store::init(root.path(), Some("live graph"), false).unwrap();
    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
    assert_eq!(report.status, RuntimeStatus::Blocked);
    assert!(report
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("marked read_only")));
}

#[test]
fn reserved_authority_environment_blocks_before_any_child_operation() {
    // LOOM_RING49_SECRET is process-wide state shared by every test in this
    // binary that binds the `secret` input. Tests run in parallel threads, so
    // this test must only ever set it to the shared sentinel and must never
    // unset it: removing it strands whichever sibling test is mid-execute with
    // "required environment variable 'LOOM_RING49_SECRET' ... is not set".
    // The value is irrelevant here because the reserved-name check blocks
    // before any child operation reads it.
    std::env::set_var("LOOM_RING49_SECRET", "do-not-persist");
    let mut authored = spec();
    authored
        .profiles
        .get_mut("proof")
        .unwrap()
        .workspace
        .env
        .insert("LOOM_AGENT".into(), "solo".into());
    let proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![operation()],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let root = TempRoot::new("reserved-authority-env");
    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
    assert_eq!(report.status, RuntimeStatus::Blocked);
    assert!(report
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("reserved runtime environment name 'LOOM_AGENT'")));
}

#[test]
fn typed_assertion_operators_evaluate_strings_arrays_objects_and_presence() {
    let root = TempRoot::new("assertion-operators");
    let mut authored = spec();
    authored.steps[0].produces.clear();
    let operation: CliOperation = serde_json::from_value(json!({
        "id": "checkout-op",
        "summary": "Emit structured assertion values",
        "argv": [
            "python3",
            "-c",
            "import json; print(json.dumps({'text':'compass-build','items':[1,{'id':'x'}],'object':{'lane':'build','rung':'grounded'},'count':2}))"
        ],
        "read_only": true,
        "arguments": [],
        "output": {
            "format": "json",
            "assertions": [
                {"id":"not-equal","pointer":"/count","not_equals":3},
                {"id":"present","pointer":"/text","exists":true},
                {"id":"absent","pointer":"/missing","exists":false},
                {"id":"string-contains","pointer":"/text","type":"string","contains":"build"},
                {"id":"array-contains","pointer":"/items","contains":{"id":"x"}},
                {"id":"object-contains","pointer":"/object","contains":{"lane":"build"}},
                {"id":"regex-matches","pointer":"/text","type":"string","matches":"^compass-[a-z]+$"},
                {"id":"type-only","pointer":"/count","type":"integer"}
            ]
        }
    }))
    .unwrap();
    let proof = journey_runtime::compile(
        &authored,
        "surface-hash",
        "proof",
        vec![operation],
        &[OperationBinding {
            step_id: "checkout".into(),
            operation_id: "checkout-op".into(),
        }],
    )
    .unwrap();
    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
    assert_eq!(report.status, RuntimeStatus::Passed, "{report:#?}");
    assert_eq!(report.assertions_passed, 8);
    assert_eq!(report.assertions_failed, 0);
}

#[test]
fn surface_rejects_one_primary_operation_bound_to_multiple_steps() {
    let mut two_steps = spec();
    two_steps.steps.push(
        serde_json::from_value(json!({
            "id": "confirm",
            "name": "Confirm",
            "action": "confirms the result",
            "expects": [],
            "produces": {}
        }))
        .unwrap(),
    );
    let hash = two_steps.semantic_hash().unwrap();
    let manifest = SurfaceManifest {
        schema: SURFACE_SCHEMA.into(),
        journey_id: two_steps.id.clone(),
        journey_hash: hash.clone(),
        surface: InterfaceSurfaceDefinition {
            id: "checkout-cli".into(),
            title: "Checkout CLI".into(),
            identity: "checkout".into(),
            codefile: "runner.py".into(),
            locator: "main".into(),
            operations: vec![operation()],
        },
        setup: None,
        bindings: vec![
            SurfaceBinding::Operation(OperationBinding {
                step_id: "checkout".into(),
                operation_id: "checkout-op".into(),
            }),
            SurfaceBinding::Operation(OperationBinding {
                step_id: "confirm".into(),
                operation_id: "checkout-op".into(),
            }),
        ],
    };
    assert!(manifest
        .validate_for(&two_steps, &hash)
        .unwrap_err()
        .to_string()
        .contains("bound more than once"));
}

#[test]
fn surface_setup_is_ordered_strict_disjoint_and_cache_significant() {
    let mut authored = spec();
    authored.steps[0].produces.clear();
    let hash = authored.semantic_hash().unwrap();
    let bindings = vec![OperationBinding {
        step_id: "checkout".into(),
        operation_id: "checkout-op".into(),
    }];
    let setup = SurfaceSetup {
        graph: SetupGraph::LocalSnapshot,
        git: None,
        before_steps: BTreeMap::new(),
        operations: vec!["prepare-one".into(), "prepare-two".into()],
    };
    let operations = vec![
        setup_operation("prepare-one"),
        setup_operation("prepare-two"),
        {
            let mut operation = operation();
            operation.output.captures.clear();
            operation
        },
    ];
    let manifest = SurfaceManifest {
        schema: SURFACE_SCHEMA.into(),
        journey_id: authored.id.clone(),
        journey_hash: hash.clone(),
        surface: InterfaceSurfaceDefinition {
            id: "checkout-cli".into(),
            title: "Checkout CLI".into(),
            identity: "checkout".into(),
            codefile: "runner.py".into(),
            locator: "main".into(),
            operations: operations.clone(),
        },
        setup: Some(setup.clone()),
        bindings: bindings.clone().into_iter().map(Into::into).collect(),
    };
    manifest.validate_for(&authored, &hash).unwrap();

    let first = journey_runtime::compile_with_setup(
        &authored,
        "surface-hash",
        "proof",
        operations.clone(),
        Some(&setup),
        &bindings,
    )
    .unwrap();
    let second = journey_runtime::compile_with_setup(
        &authored,
        "surface-hash",
        "proof",
        operations.clone(),
        Some(&setup),
        &bindings,
    )
    .unwrap();
    assert_eq!(
        journey_runtime::canonical_bytes(&first).unwrap(),
        journey_runtime::canonical_bytes(&second).unwrap()
    );
    let mut reversed = setup.clone();
    reversed.operations.reverse();
    let reordered = journey_runtime::compile_with_setup(
        &authored,
        "surface-hash",
        "proof",
        operations,
        Some(&reversed),
        &bindings,
    )
    .unwrap();
    assert_ne!(
        journey_runtime::canonical_bytes(&first).unwrap(),
        journey_runtime::canonical_bytes(&reordered).unwrap(),
        "ordered setup must participate in the compiled cache key"
    );
    let mut with_git = setup.clone();
    with_git.git = Some(SurfaceGitSetup {
        mode: SurfaceGitMode::IsolatedSnapshot,
        dirty_paths: vec!["runner.py".into()],
    });
    let git_fixture = journey_runtime::compile_with_setup(
        &authored,
        "surface-hash",
        "proof",
        vec![
            setup_operation("prepare-one"),
            setup_operation("prepare-two"),
            {
                let mut operation = operation();
                operation.output.captures.clear();
                operation
            },
        ],
        Some(&with_git),
        &bindings,
    )
    .unwrap();
    assert_ne!(
        journey_runtime::canonical_bytes(&first).unwrap(),
        journey_runtime::canonical_bytes(&git_fixture).unwrap(),
        "isolated Git setup must participate in the compiled cache key"
    );

    let mut invalid = manifest.clone();
    invalid.setup.as_mut().unwrap().operations = vec!["prepare-one".into(), "prepare-one".into()];
    assert!(invalid
        .validate_for(&authored, &hash)
        .unwrap_err()
        .to_string()
        .contains("repeats operation"));
    invalid.setup.as_mut().unwrap().operations = vec!["missing".into()];
    assert!(invalid
        .validate_for(&authored, &hash)
        .unwrap_err()
        .to_string()
        .contains("unknown operation"));
    invalid.setup.as_mut().unwrap().operations = vec!["checkout-op".into()];
    assert!(invalid
        .validate_for(&authored, &hash)
        .unwrap_err()
        .to_string()
        .contains("also bound"));

    let mut unknown = serde_json::to_value(manifest).unwrap();
    unknown["setup"]["clone_policy"] = json!("unsafe");
    assert!(serde_json::from_value::<SurfaceManifest>(unknown).is_err());

    for dirty_paths in [
        json!([]),
        json!(["runner.py", "runner.py"]),
        json!(["../runner.py"]),
        json!(["/runner.py"]),
        json!(["src\\runner.py"]),
        json!([".loom/graph.sqlite"]),
    ] {
        let mut invalid = serde_json::to_value(&git_fixture.setup).unwrap();
        invalid["git"]["dirty_paths"] = dirty_paths;
        let invalid: journey_runtime::CompiledSetup = serde_json::from_value(invalid).unwrap();
        let mut proof = git_fixture.clone();
        proof.setup = Some(invalid);
        assert!(proof.validate().is_err());
    }

    let mut invalid_mode = serde_json::to_value(&git_fixture.setup).unwrap();
    invalid_mode["git"]["mode"] = json!("working_tree");
    assert!(
        serde_json::from_value::<Option<journey_runtime::CompiledSetup>>(invalid_mode).is_err()
    );
    let missing_graph = json!({
        "git":{"mode":"isolated_snapshot","dirty_paths":["runner.py"]},
        "operations":["prepare-one"]
    });
    assert!(serde_json::from_value::<SurfaceSetup>(missing_graph).is_err());
}

#[test]
fn interface_surface_has_no_shell_or_http_execution_form() {
    let mut surface = InterfaceSurfaceDefinition {
        id: "checkout-cli".into(),
        title: "Checkout CLI".into(),
        identity: "checkout".into(),
        codefile: "runner.py".into(),
        locator: "main".into(),
        operations: vec![operation()],
    };
    surface.operations[0].argv = vec!["sh".into(), "-c".into(), "true".into()];
    assert!(surface
        .validate()
        .unwrap_err()
        .to_string()
        .contains("shell"));
    surface.operations[0].argv = vec!["curl".into(), "https://example.com".into()];
    assert!(surface.validate().unwrap_err().to_string().contains("HTTP"));
}

#[test]
fn local_snapshot_setup_precedes_status_without_strength_or_live_mutation() {
    let root = TempRoot::new("local-snapshot-setup");
    let rooted_intent = rooted_compass_fixture(&root);
    let before_status = loom_command(root.path(), &["status", "--json"]);
    assert_eq!(before_status["compass"]["phase"], "surface");

    let database = root.path().join(".loom/graph.sqlite");
    let journal = root.path().join(".loom/journal/events.jsonl");
    let database_before = std::fs::read(&database).unwrap();
    let journal_before = std::fs::read(&journal).unwrap();

    let clone = TempRoot::new("trusted-local-clone");
    let source = Store::open_read(root.path()).unwrap();
    let source_snapshot = source.snapshot().unwrap();
    source.clone_local_snapshot(clone.path()).unwrap();
    drop(source);
    let cloned = Store::open_read(clone.path()).unwrap();
    assert_eq!(cloned.snapshot().unwrap(), source_snapshot);
    assert_eq!(
        loom::journal::read(clone.path()).unwrap(),
        loom::journal::read(root.path()).unwrap(),
        "a trusted local clone must preserve local journal authority verbatim"
    );
    assert!(clone.path().join("compass-fixture.journey.json").is_file());
    assert!(clone.path().join("src/compass_fixture.rs").is_file());
    drop(cloned);

    let authored: JourneySpec = serde_json::from_value(json!({
        "schema": JOURNEY_SCHEMA,
        "id": "compass.runtime",
        "name": "Compass runtime",
        "actor": "operator",
        "goal": "Inspect the lowest unmet rung",
        "inputs": {},
        "preconditions": ["A current rooted behavior is unrealized"],
        "steps": [{
            "id":"inspect-work-direction",
            "name":"Inspect work direction",
            "action":"inspects status",
            "expects":["the compass points to build and grounded"],
            "produces":{}
        }],
        "profiles":{"proof":{"inputs":{},"workspace":{}}}
    }))
    .unwrap();
    let mut authored = authored;
    authored
        .profiles
        .get_mut("proof")
        .unwrap()
        .workspace
        .env
        .insert(
            "PATH".into(),
            Path::new(env!("CARGO_BIN_EXE_loom"))
                .parent()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
    let setup_operation: CliOperation = serde_json::from_value(json!({
        "id":"make-rooted-intent-unrealized",
        "summary":"Make one rooted Intent planned in the isolated graph",
        "argv":[
            "loom",
            "intent",
            "update",
            rooted_intent,
            "--lifecycle",
            "planned",
            "--reason",
            "Journey proof fixture changes only the isolated local snapshot",
            "--json"
        ],
        "read_only":false,
        "arguments":[],
        "output":{
            "format":"json",
            "assertions":[{
                "id":"fixture-is-planned",
                "pointer":"/intent/status",
                "type":"string",
                "equals":"planned"
            }]
        }
    }))
    .unwrap();
    let status_operation: CliOperation = serde_json::from_value(json!({
        "id":"inspect-status",
        "summary":"Inspect the current compass",
        "argv":["loom","status","--json"],
        "read_only":true,
        "arguments":[],
        "output":{
            "format":"json",
            "assertions":[
                {"id":"phase-is-build","pointer":"/compass/phase","type":"string","equals":"build"},
                {"id":"rung-is-grounded","pointer":"/compass/rung","type":"string","equals":"grounded"}
            ]
        }
    }))
    .unwrap();
    let setup = SurfaceSetup {
        graph: SetupGraph::LocalSnapshot,
        git: None,
        before_steps: BTreeMap::new(),
        operations: vec!["make-rooted-intent-unrealized".into()],
    };
    let bindings = [OperationBinding {
        step_id: "inspect-work-direction".into(),
        operation_id: "inspect-status".into(),
    }];
    let proof = journey_runtime::compile_with_setup(
        &authored,
        "surface-with-setup",
        "proof",
        vec![setup_operation.clone(), status_operation.clone()],
        Some(&setup),
        &bindings,
    )
    .unwrap();
    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
    assert_eq!(report.status, RuntimeStatus::Passed, "{report:#?}");
    assert_eq!(report.setup.len(), 1);
    assert_eq!(report.setup[0].assertions_passed, 1);
    assert_eq!(report.setup[0].argv[1], "--graph");
    assert!(
        Path::new(&report.setup[0].argv[2]).is_absolute(),
        "setup graph injection was not canonical: {:?}",
        report.setup[0].argv
    );
    assert_eq!(report.steps.len(), 1);
    assert_eq!(report.steps[0].argv[1], "--graph");
    assert!(
        Path::new(&report.steps[0].argv[2]).is_absolute(),
        "step graph injection was not canonical: {:?}",
        report.steps[0].argv
    );
    assert_eq!(report.steps[0].output["compass"]["phase"], "build");
    assert_eq!(report.steps[0].output["compass"]["rung"], "grounded");
    assert_eq!(
        report.assertions_passed, 2,
        "setup checks must not contribute semantic proof strength"
    );
    assert_eq!(std::fs::read(&database).unwrap(), database_before);
    assert_eq!(std::fs::read(&journal).unwrap(), journal_before);
    let live = Store::open(root.path()).unwrap();
    assert_eq!(
        live.resolve_node(&rooted_intent, Some(NodeType::Intent))
            .unwrap()
            .status,
        "implemented"
    );
    drop(live);

    let mut failing_setup = setup_operation;
    failing_setup.output.assertions[0].equals = Some(json!("implemented"));
    let failing = journey_runtime::compile_with_setup(
        &authored,
        "surface-with-failing-setup",
        "proof",
        vec![failing_setup, status_operation],
        Some(&setup),
        &bindings,
    )
    .unwrap();
    let blocked = journey_runtime::execute(root.path(), &authored, &failing, &BTreeMap::new());
    assert_eq!(blocked.status, RuntimeStatus::Blocked, "{blocked:#?}");
    assert_eq!(blocked.setup.len(), 1);
    assert!(
        blocked.steps.is_empty(),
        "semantic step ran after setup failed"
    );
    assert_eq!(blocked.assertions_passed, 0);
    assert_eq!(std::fs::read(&database).unwrap(), database_before);
    assert_eq!(std::fs::read(&journal).unwrap(), journal_before);
}

#[test]
fn source_divergence_surface_creates_one_confined_stable_name_conflict() {
    const TARGET: &str = "dispatch the authoritative proof runner";
    const DRIFT: &str = "fixture: criterion changed after human ratification";
    let root = TempRoot::new("source-divergence-surface");
    let store = Store::init(root.path(), Some("divergence surface"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            TARGET,
            "A proof run invokes the registered authoritative runner and captures its actual outcome.",
            "planned",
            json!({}),
        )
        .unwrap();
    store
        .ratify_intent(
            &intent.id,
            "the proof dispatch policy was reviewed before this isolated fixture",
            "keep the authoritative runner dispatch behavior",
        )
        .unwrap();
    drop(store);

    let database = root.path().join(".loom/graph.sqlite");
    let journal = root.path().join(".loom/journal/events.jsonl");
    let database_before = std::fs::read(&database).unwrap();
    let journal_before = std::fs::read(&journal).unwrap();

    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut journey =
        loom::journey::parse(&repository.join("journeys/divergence-queue.yaml")).unwrap();
    journey
        .profiles
        .get_mut("proof")
        .unwrap()
        .workspace
        .env
        .insert(
            "PATH".into(),
            Path::new(env!("CARGO_BIN_EXE_loom"))
                .parent()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
    let manifest = SurfaceManifest::parse_json(
        &repository.join("journeys/surfaces/divergence-queue.surface.json"),
    )
    .unwrap();
    let proof = journey_runtime::compile_surface(
        &journey,
        "source-divergence-surface",
        "proof",
        manifest.surface.operations,
        manifest.setup.as_ref(),
        &manifest.bindings,
    )
    .unwrap();
    let report = journey_runtime::execute(root.path(), &journey, &proof, &BTreeMap::new());

    assert_eq!(report.status, RuntimeStatus::Passed, "{report:#?}");
    assert_eq!(report.setup.len(), 2);
    assert_eq!(report.setup[0].output["name"], TARGET);
    assert_eq!(report.setup[0].output["ratification"], "ratified");
    assert_eq!(report.setup[1].output["intent"]["name"], TARGET);
    assert_eq!(report.setup[1].output["intent"]["description"], DRIFT);
    assert_eq!(report.steps.len(), 1);
    assert_eq!(
        report.steps[0].output["work_item"]["target"]["kind"],
        "intent"
    );
    assert!(report.steps[0].output["work_item"]["reason"]
        .as_str()
        .unwrap()
        .contains("redefined after ratification — the words changed under the yes"));
    assert_eq!(
        report.steps[0].output["work_item"]["prompt_contract"]["human_gate"]["options"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(std::fs::read(&database).unwrap(), database_before);
    assert_eq!(std::fs::read(&journal).unwrap(), journal_before);
}

#[test]
fn temporal_files_and_in_place_argv_templates_are_confined_ordered_and_hash_bound() {
    let root = TempRoot::new("temporal-files");
    loom_command(root.path(), &["init", root.path().to_str().unwrap()]);
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    let live_file = root.path().join("src/temporal_fixture.rs");
    let base = "pub fn base() {}\n";
    let locator = "pub fn locator() {}\n";
    let reopened = "// loom:anchor source.anchor\npub fn reopened() {}\n";
    std::fs::write(&live_file, base).unwrap();
    loom_command(
        root.path(),
        &["codefile", "add", "src/temporal_fixture.rs", "--json"],
    );
    let database = root.path().join(".loom/graph.sqlite");
    let journal = root.path().join(".loom/journal/events.jsonl");
    let live_before = std::fs::read(&live_file).unwrap();
    let database_before = std::fs::read(&database).unwrap();
    let journal_before = std::fs::read(&journal).ok();

    let injected_topic = "door topic; touch must-not-exist";
    let authored: JourneySpec = serde_json::from_value(json!({
        "schema":JOURNEY_SCHEMA,
        "id":"temporal.files",
        "name":"Temporal file fixture",
        "actor":"operator",
        "goal":"Observe exact repository transitions without touching live bytes",
        "inputs":{
            "topic":{"type":"string","description":"Door topic key","default":injected_topic}
        },
        "preconditions":[],
        "steps":[
            {
                "id":"locator",
                "name":"Locator",
                "action":"inspect locator bytes",
                "expects":[],
                "produces":{"anchor-marker":{"type":"string","description":"Issued source anchor marker"}}
            },
            {
                "id":"reopens",
                "name":"Reopens",
                "action":"inspect changed anchored bytes",
                "expects":[],
                "produces":{}
            },
            {
                "id":"sync",
                "name":"Sync",
                "action":"inspect an unchanged replacement",
                "expects":[],
                "produces":{}
            }
        ],
        "profiles":{"proof":{"inputs":{},"workspace":{}}}
    }))
    .unwrap();
    let inspect_locator: CliOperation = serde_json::from_value(json!({
        "id":"inspect-locator",
        "summary":"Read locator bytes with a dynamic key before a literal argument",
        "argv":[
            "python3","-c",
            "import json,pathlib,sys; print(json.dumps({'topic':sys.argv[1],'literal':sys.argv[2],'content':pathlib.Path('src/temporal_fixture.rs').read_text(),'marker':'source.anchor'}))",
            "${{ inputs.topic }}","literal-tail"
        ],
        "read_only":true,
        "arguments":[],
        "output":{
            "format":"json",
            "captures":[{"id":"anchor-marker","pointer":"/marker","type":"string"}],
            "assertions":[
                {"id":"topic-is-one-token","pointer":"/topic","type":"string","equals":injected_topic},
                {"id":"literal-follows-topic","pointer":"/literal","type":"string","equals":"literal-tail"}
            ]
        }
    }))
    .unwrap();
    let inspect_reopens: CliOperation = serde_json::from_value(json!({
        "id":"inspect-reopens",
        "summary":"Read bytes containing the prior captured source anchor marker",
        "argv":["python3","-c","import json,pathlib; print(json.dumps({'content':pathlib.Path('src/temporal_fixture.rs').read_text()}))"],
        "read_only":true,
        "arguments":[],
        "output":{"format":"json","assertions":[{"id":"anchor-was-interpolated","pointer":"/content","type":"string","equals":reopened}]}
    }))
    .unwrap();
    let inspect_sync: CliOperation = serde_json::from_value(json!({
        "id":"inspect-sync",
        "summary":"Read bytes after an idempotent replacement",
        "argv":["python3","-c","import json,pathlib; print(json.dumps({'content':pathlib.Path('src/temporal_fixture.rs').read_text()}))"],
        "read_only":true,
        "arguments":[],
        "output":{"format":"json","assertions":[{"id":"unchanged-content-stands","pointer":"/content","type":"string","equals":reopened}]}
    }))
    .unwrap();
    let operations = vec![
        inspect_locator.clone(),
        inspect_reopens.clone(),
        inspect_sync.clone(),
    ];
    let bindings = vec![
        OperationBinding {
            step_id: "locator".into(),
            operation_id: "inspect-locator".into(),
        },
        OperationBinding {
            step_id: "reopens".into(),
            operation_id: "inspect-reopens".into(),
        },
        OperationBinding {
            step_id: "sync".into(),
            operation_id: "inspect-sync".into(),
        },
    ];
    let setup = SurfaceSetup {
        graph: SetupGraph::LocalSnapshot,
        git: None,
        before_steps: BTreeMap::from([
            (
                "locator".into(),
                vec![SurfaceFileAction {
                    path: "src/temporal_fixture.rs".into(),
                    expected_hash: loom::artifact::fingerprint(base),
                    content: Some(locator.into()),
                    template: None,
                }],
            ),
            (
                "reopens".into(),
                vec![SurfaceFileAction {
                    path: "src/temporal_fixture.rs".into(),
                    expected_hash: loom::artifact::fingerprint(locator),
                    content: None,
                    template: Some(
                        "// loom:anchor {{ steps.locator.outputs.anchor-marker }}\npub fn reopened() {}\n"
                            .into(),
                    ),
                }],
            ),
            (
                "sync".into(),
                vec![SurfaceFileAction {
                    path: "src/temporal_fixture.rs".into(),
                    expected_hash: loom::artifact::fingerprint(reopened),
                    content: Some(reopened.into()),
                    template: None,
                }],
            ),
        ]),
        operations: vec![],
    };
    let proof = journey_runtime::compile_with_setup(
        &authored,
        "temporal-surface",
        "proof",
        operations.clone(),
        Some(&setup),
        &bindings,
    )
    .unwrap();
    let identical = journey_runtime::compile_with_setup(
        &authored,
        "temporal-surface",
        "proof",
        operations.clone(),
        Some(&setup),
        &bindings,
    )
    .unwrap();
    assert_eq!(
        journey_runtime::canonical_bytes(&proof).unwrap(),
        journey_runtime::canonical_bytes(&identical).unwrap(),
        "temporal setup compiles deterministically"
    );
    let mut changed_setup = setup.clone();
    changed_setup.before_steps.get_mut("sync").unwrap()[0].content =
        Some(format!("{reopened}// changed\n"));
    let changed = journey_runtime::compile_with_setup(
        &authored,
        "temporal-surface",
        "proof",
        operations.clone(),
        Some(&changed_setup),
        &bindings,
    )
    .unwrap();
    assert_ne!(
        journey_runtime::canonical_bytes(&proof).unwrap(),
        journey_runtime::canonical_bytes(&changed).unwrap(),
        "before_steps participates in the compiled cache key"
    );

    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
    assert_eq!(report.status, RuntimeStatus::Passed, "{report:#?}");
    assert_eq!(report.assertions_passed, 4);
    assert_eq!(report.file_transitions.len(), 3);
    assert!(report.file_transitions.iter().all(|entry| entry.applied));
    assert_eq!(
        report
            .file_transitions
            .iter()
            .map(|entry| entry.changed)
            .collect::<Vec<_>>(),
        vec![true, true, false]
    );
    assert_eq!(report.steps[0].argv[3], injected_topic);
    assert_eq!(report.steps[0].argv[4], "literal-tail");
    assert!(!root.path().join("must-not-exist").exists());
    assert_eq!(std::fs::read(&live_file).unwrap(), live_before);
    assert_eq!(std::fs::read(&database).unwrap(), database_before);
    assert_eq!(std::fs::read(&journal).ok(), journal_before);

    let mut stale = proof.clone();
    stale
        .setup
        .as_mut()
        .unwrap()
        .before_steps
        .get_mut("locator")
        .unwrap()[0]
        .expected_hash = "0000000000000000".into();
    let blocked = journey_runtime::execute(root.path(), &authored, &stale, &BTreeMap::new());
    assert_eq!(blocked.status, RuntimeStatus::Blocked, "{blocked:#?}");
    assert!(
        blocked.steps.is_empty(),
        "the semantic step ran after a stale hash"
    );
    assert_eq!(blocked.file_transitions.len(), 1);
    assert!(!blocked.file_transitions[0].applied);
    assert_eq!(
        blocked.file_transitions[0].observed_before_hash,
        blocked.file_transitions[0].observed_after_hash
    );
    assert_eq!(std::fs::read(&live_file).unwrap(), live_before);

    let mut nul = BTreeMap::new();
    nul.insert("topic".into(), json!("bad\u{0}topic"));
    let blocked = journey_runtime::execute(root.path(), &authored, &proof, &nul);
    assert_eq!(blocked.status, RuntimeStatus::Blocked);
    assert!(blocked.detail.unwrap().contains("NUL"));
    assert!(blocked.steps.is_empty());

    let mut mixed = operations.clone();
    mixed[0].argv[3] = "prefix-${{ inputs.topic }}".into();
    assert!(journey_runtime::compile_with_setup(
        &authored,
        "mixed-token",
        "proof",
        mixed,
        Some(&setup),
        &bindings,
    )
    .unwrap_err()
    .to_string()
    .contains("exactly one"));
    let mut future = operations;
    future[0].argv[3] = "${{ steps.locator.outputs.anchor-marker }}".into();
    let future_error = journey_runtime::compile_with_setup(
        &authored,
        "future-token",
        "proof",
        future,
        Some(&setup),
        &bindings,
    )
    .unwrap_err();
    assert!(format!("{future_error:#}").contains("earlier step"));

    let mut unknown = inspect_locator.clone();
    unknown.argv[3] = "${{ inputs.unknown }}".into();
    let unknown_error = journey_runtime::compile_with_setup(
        &authored,
        "unknown-token",
        "proof",
        vec![unknown, inspect_reopens.clone(), inspect_sync.clone()],
        Some(&setup),
        &bindings,
    )
    .unwrap_err();
    assert!(format!("{unknown_error:#}").contains("unknown Journey input"));

    let mut protected = authored.clone();
    protected.inputs.insert(
        "protected".into(),
        serde_json::from_value(json!({
            "type":"string",
            "description":"must remain out of argv and file content",
            "secret":true
        }))
        .unwrap(),
    );
    protected.profiles.get_mut("proof").unwrap().inputs.insert(
        "protected".into(),
        serde_json::from_value(json!({"env":"LOOM_RING49_PROTECTED"})).unwrap(),
    );
    let mut secret_argv = inspect_locator.clone();
    secret_argv.argv[3] = "${{ inputs.protected }}".into();
    let secret_error = journey_runtime::compile_with_setup(
        &protected,
        "secret-token",
        "proof",
        vec![secret_argv, inspect_reopens.clone(), inspect_sync.clone()],
        Some(&setup),
        &bindings,
    )
    .unwrap_err();
    assert!(format!("{secret_error:#}").contains("secret input"));
    let mut secret_setup = setup.clone();
    secret_setup.before_steps.get_mut("locator").unwrap()[0].content = None;
    secret_setup.before_steps.get_mut("locator").unwrap()[0].template =
        Some("{{ inputs.protected }}".into());
    let secret_file_error = journey_runtime::compile_with_setup(
        &protected,
        "secret-file-template",
        "proof",
        vec![
            inspect_locator.clone(),
            inspect_reopens.clone(),
            inspect_sync.clone(),
        ],
        Some(&secret_setup),
        &bindings,
    )
    .unwrap_err();
    assert!(format!("{secret_file_error:#}").contains("secret input"));

    let mut structured = authored.clone();
    structured.inputs.insert(
        "structured".into(),
        serde_json::from_value(json!({
            "type":"json",
            "description":"structured input cannot become one argv token",
            "default":{"key":"value"}
        }))
        .unwrap(),
    );
    let mut non_scalar = inspect_locator.clone();
    non_scalar.argv[3] = "${{ inputs.structured }}".into();
    let non_scalar_error = journey_runtime::compile_with_setup(
        &structured,
        "non-scalar-token",
        "proof",
        vec![non_scalar, inspect_reopens.clone(), inspect_sync.clone()],
        Some(&setup),
        &bindings,
    )
    .unwrap_err();
    assert!(format!("{non_scalar_error:#}").contains("not scalar"));

    let mut redacted_operations = vec![inspect_locator, inspect_reopens, inspect_sync];
    redacted_operations[0].output.captures[0].redact = true;
    redacted_operations[1]
        .argv
        .push("${{ steps.locator.outputs.anchor-marker }}".into());
    let redacted_error = journey_runtime::compile_with_setup(
        &authored,
        "redacted-token",
        "proof",
        redacted_operations,
        Some(&setup),
        &bindings,
    )
    .unwrap_err();
    assert!(format!("{redacted_error:#}").contains("redacted output"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = root.path().join("outside-temporal-target.rs");
        std::fs::write(&outside, "outside bytes stay unchanged\n").unwrap();
        let outside_before = std::fs::read(&outside).unwrap();
        std::fs::remove_file(&live_file).unwrap();
        symlink(&outside, &live_file).unwrap();
        let blocked = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
        assert_eq!(blocked.status, RuntimeStatus::Blocked);
        let detail = blocked.detail.unwrap();
        assert!(detail.contains("symlink"), "{detail}");
        assert_eq!(std::fs::read(&outside).unwrap(), outside_before);
    }
}

#[test]
fn human_decision_gate_pauses_without_authority_then_resumes_one_shot_in_same_snapshot() {
    let root = TempRoot::new("human-gate-runtime");
    loom_command(root.path(), &["init", root.path().to_str().unwrap()]);
    let store = Store::open(root.path()).unwrap();
    let subject = store
        .add_node(
            NodeType::Intent,
            "signed export remains wanted",
            "The current criterion still requires a signed export.",
            "planned",
            json!({}),
        )
        .unwrap();
    let subject_id = subject.id.clone();
    drop(store);
    let database = root.path().join(".loom/graph.sqlite");
    let journal = root.path().join(".loom/journal/events.jsonl");
    let database_before = std::fs::read(&database).unwrap();
    let journal_before = std::fs::read(&journal).ok();

    let mut authored: JourneySpec = serde_json::from_value(json!({
        "schema":JOURNEY_SCHEMA,
        "id":"human-gate-runtime",
        "name":"Ask and record one exact human choice",
        "actor":"operator",
        "goal":"Pause at a structured host-mediated question",
        "inputs":{},
        "preconditions":[],
        "steps":[
            {
                "id":"present-decision",
                "name":"Present decision",
                "action":"present evidence and choices",
                "expects":["a structured choice is presented"],
                "produces":{}
            },
            {
                "id":"record-human-choice",
                "name":"Record human choice",
                "action":"record the exact mediated answer",
                "expects":["the human remains authority"],
                "produces":{}
            }
        ],
        "profiles":{"proof":{"inputs":{},"workspace":{}}}
    }))
    .unwrap();
    let ratify_packet = json!({
        "presented":true,
        "work_item":{
            "target":{
                "kind":"intent",
                "id":subject_id.clone(),
                "name":"signed export remains wanted"
            },
            "reason":"meaning drifted: 'signed export remains wanted' — redefined after ratification — the words changed under the yes",
            "context":{"linked_entities":[{
                "role":"target",
                "kind":"intent",
                "id":subject_id.clone(),
                "name":"signed export remains wanted",
                "description":"The current criterion still requires a signed export."
            }]},
            "prompt_contract":{"human_gate":{
                "question":"Should the signed export remain a wanted behavior?",
                "recommendation":"Recommend one option from the current implementation and proof evidence; never treat it as the decision.",
                "after_answer":"Run a generated write-back command.",
                "options":[
                    {"id":"ratify","label":"Keep behavior","description":"Retain the current criterion.","write_back":"loom intent ratify ..."},
                    {"id":"reject","label":"Remove behavior","description":"Reject the current criterion.","write_back":"loom intent reject ..."},
                    {"id":"revise","label":"Revise criterion","description":"Supply a corrected criterion.","write_back":"loom intent revise ..."}
                ]
            }}
        }
    })
    .to_string();
    authored
        .profiles
        .get_mut("proof")
        .unwrap()
        .workspace
        .files
        .push(
            serde_json::from_value(json!({
                "path":"fixture/ratify-packet.json",
                "content":ratify_packet
            }))
            .unwrap(),
        );
    let present: CliOperation = serde_json::from_value(json!({
        "id":"present-decision-op",
        "summary":"Emit a structured recommendation without choosing",
        "argv":["python3","-c","print(open('fixture/ratify-packet.json').read())"],
        "read_only":true,
        "arguments":[],
        "output":{
            "format":"json",
            "assertions":[{"id":"prompt-presented","pointer":"/presented","type":"boolean","equals":true}]
        }
    }))
    .unwrap();
    let setup = SurfaceSetup {
        graph: SetupGraph::LocalSnapshot,
        git: None,
        before_steps: BTreeMap::new(),
        operations: Vec::new(),
    };
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
    let proof = journey_runtime::compile_surface(
        &authored,
        "0123456789abcdef",
        "proof",
        vec![present],
        Some(&setup),
        &bindings,
    )
    .unwrap();

    let pending = match journey_runtime::execute_interactive(
        root.path(),
        &authored,
        &proof,
        &BTreeMap::new(),
    ) {
        ExecutionOutcome::Pending(pending) => pending,
        other => panic!("expected a host-mediated pause, got {other:#?}"),
    };
    assert_eq!(pending.binding.step_id, "record-human-choice");
    assert_eq!(pending.binding.subject.kind, "intent");
    assert_eq!(pending.binding.subject.id, subject_id);
    assert_eq!(pending.binding.subject.hash.len(), 16);
    assert_eq!(pending.options.len(), 3);
    assert!(pending.options[2].free_form);
    assert!(pending.recommendation.contains("Current drift evidence"));
    assert!(!pending.human_terminal_required);
    let pending_json = serde_json::to_string(&pending).unwrap();
    assert!(!pending_json.contains("write_back"));
    assert!(!pending_json.contains("after_answer"));
    assert_eq!(std::fs::read(&database).unwrap(), database_before);
    assert_eq!(std::fs::read(&journal).ok(), journal_before);
    assert!(
        !journey_runtime::proof_path(root.path(), "human-gate-runtime", "proof")
            .unwrap()
            .exists()
    );

    // The repository root remains the token's binding, but the decision's
    // subject is the snapshot this run actually presented and executed in.
    let live = Store::open(root.path()).unwrap();
    live.update_node(
        &subject_id,
        None,
        Some("The live subject changed after the isolated run paused."),
        None,
    )
    .unwrap();
    drop(live);
    let live_database_after_pause = std::fs::read(&database).unwrap();

    let invalid = journey_runtime::resume_interactive(
        root.path(),
        &authored,
        &proof,
        &pending.resume_token,
        ResumeAnswer {
            choice_id: "ratify".into(),
            human_decision: "<answer>".into(),
            free_form: None,
        },
        "llm:builder",
    )
    .unwrap_err();
    assert!(
        format!("{invalid:#}").contains("placeholder"),
        "{invalid:#}"
    );

    let mut stale = proof.clone();
    stale.surface_hash = "fedcba9876543210".into();
    let stale_error = journey_runtime::resume_interactive(
        root.path(),
        &authored,
        &stale,
        &pending.resume_token,
        ResumeAnswer {
            choice_id: "ratify".into(),
            human_decision: "Keep behavior because the cited evidence is current".into(),
            free_form: None,
        },
        "llm:builder",
    )
    .unwrap_err();
    assert!(format!("{stale_error:#}").contains("stale"));

    let completed = journey_runtime::resume_interactive(
        root.path(),
        &authored,
        &proof,
        &pending.resume_token,
        ResumeAnswer {
            choice_id: "ratify".into(),
            human_decision: "Keep behavior because the cited evidence is current".into(),
            free_form: None,
        },
        "llm:builder",
    )
    .unwrap();
    let (report, decisions) = match completed {
        ExecutionOutcome::Completed {
            report,
            human_decisions,
            ..
        } => (report, human_decisions),
        other => panic!("expected completion after the answer, got {other:#?}"),
    };
    assert_eq!(report.status, RuntimeStatus::Passed, "{report:#?}");
    assert_eq!(report.assertions_passed, 2);
    assert_eq!(report.steps.len(), 2);
    assert_eq!(report.steps[1].operation_id, "human-decision");
    assert_eq!(report.steps[1].output["authority"], "human");
    assert_eq!(report.steps[1].output["executor"], "llm:builder");
    let report_json = serde_json::to_string(&report).unwrap();
    assert!(!report_json.contains("write_back"));
    assert!(!report_json.contains("after_answer"));
    assert_eq!(decisions.len(), 1);
    assert_eq!(std::fs::read(&database).unwrap(), live_database_after_pause);
    assert_eq!(std::fs::read(&journal).ok(), journal_before);

    let replay = journey_runtime::resume_interactive(
        root.path(),
        &authored,
        &proof,
        &pending.resume_token,
        ResumeAnswer {
            choice_id: "ratify".into(),
            human_decision: "Keep behavior because the cited evidence is current".into(),
            free_form: None,
        },
        "llm:builder",
    )
    .unwrap_err();
    assert!(
        format!("{replay:#}").contains("opening Journey gate continuation"),
        "{replay:#}"
    );
}

#[cfg(unix)]
#[test]
fn isolated_git_setup_is_one_commit_exactly_dirty_and_never_touches_live_git() {
    use std::os::unix::fs::PermissionsExt;

    let root = TempRoot::new("isolated-git-setup");
    loom_command(root.path(), &["init", root.path().to_str().unwrap()]);
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    let codefile = root.path().join("src/checkpoint_fixture.rs");
    std::fs::write(&codefile, "pub fn checkpoint_fixture() {}\n").unwrap();
    loom_command(
        root.path(),
        &["codefile", "add", "src/checkpoint_fixture.rs", "--json"],
    );

    git_capture(root.path(), &["init", "--quiet"]);
    git_capture(root.path(), &["config", "user.name", "Loom Test"]);
    git_capture(
        root.path(),
        &["config", "user.email", "loom@example.invalid"],
    );
    std::fs::write(root.path().join(".gitignore"), ".loom/\n").unwrap();
    git_capture(
        root.path(),
        &["add", ".gitignore", "src/checkpoint_fixture.rs"],
    );
    git_capture(root.path(), &["commit", "--quiet", "-m", "live baseline"]);
    git_capture(
        root.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/never-contact.git",
        ],
    );

    let head_before = git_capture(root.path(), &["rev-parse", "HEAD"]);
    let commits_before = git_capture(root.path(), &["rev-list", "--count", "HEAD"]);
    let status_before = git_capture(
        root.path(),
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    );
    let remotes_before = git_capture(root.path(), &["remote", "-v"]);
    let index_before = std::fs::read(root.path().join(".git/index")).unwrap();
    let config_before = std::fs::read(root.path().join(".git/config")).unwrap();
    let reflog_before = std::fs::read(root.path().join(".git/logs/HEAD")).unwrap();
    let source_before = std::fs::read(&codefile).unwrap();
    let source_mode_before = std::fs::metadata(&codefile).unwrap().permissions().mode();

    let authored: JourneySpec = serde_json::from_value(json!({
        "schema":JOURNEY_SCHEMA,
        "id":"checkpoint.git-fixture",
        "name":"Inspect isolated Git fixture",
        "actor":"driver",
        "goal":"Receive exact local checkpoint evidence without touching live Git",
        "inputs":{},
        "preconditions":[],
        "steps":[{
            "id":"inspect-fixture",
            "name":"Inspect fixture",
            "action":"inspects isolated Git state",
            "expects":["one baseline commit and one exact unstaged path exist"],
            "produces":{}
        }],
        "profiles":{"proof":{"inputs":{},"workspace":{
            "files":[{"path":"secrets/token.txt","content":"must-never-be-tracked"}]
        }}}
    }))
    .unwrap();
    let prepare: CliOperation = serde_json::from_value(json!({
        "id":"prepare-graph",
        "summary":"Confirm setup runs inside the isolated Git fixture",
        "argv":["python3","-c",concat!(
            "import json,pathlib,subprocess; ",
            "pathlib.Path('fixture.ready').write_text('ready'); ",
            "top=pathlib.Path(subprocess.check_output(['git','rev-parse','--show-toplevel'],text=True).strip()).resolve(); ",
            "print(json.dumps({'ready': True, 'isolated_git': top==pathlib.Path.cwd().resolve()}))"
        )],
        "read_only":false,
        "arguments":[],
        "output":{"format":"json","assertions":[
            {"id":"clone-ready","pointer":"/ready","type":"boolean","equals":true},
            {"id":"no-live-git-discovery","pointer":"/isolated_git","type":"boolean","equals":true}
        ]}
    }))
    .unwrap();
    let inspect: CliOperation = serde_json::from_value(json!({
        "id":"inspect-isolated-git",
        "summary":"Project the isolated repository state",
        "argv":["python3","-c",concat!(
            "import json,pathlib,subprocess; ",
            "g=lambda *a: subprocess.check_output(['git',*a],text=True); ",
            "print(json.dumps({",
            "'commit_count':int(g('rev-list','--count','HEAD').strip()),",
            "'remotes':g('remote').splitlines(),",
            "'status':g('status','--porcelain=v1','--untracked-files=all').rstrip('\\n'),",
            "'tracked':g('ls-files').splitlines(),",
            "'secret_exists':pathlib.Path('secrets/token.txt').is_file()",
            "}))"
        )],
        "read_only":true,
        "arguments":[],
        "output":{"format":"json","assertions":[
            {"id":"one-commit","pointer":"/commit_count","type":"integer","equals":1},
            {"id":"no-remotes","pointer":"/remotes","type":"json","equals":[]},
            {"id":"exact-dirty-path","pointer":"/status","type":"string","equals":" M src/checkpoint_fixture.rs"},
            {"id":"exact-tracked-scope","pointer":"/tracked","type":"json","equals":["src/checkpoint_fixture.rs"]},
            {"id":"secret-remains-fixture-only","pointer":"/secret_exists","type":"boolean","equals":true}
        ]}
    }))
    .unwrap();
    let setup = SurfaceSetup {
        graph: SetupGraph::LocalSnapshot,
        git: Some(SurfaceGitSetup {
            mode: SurfaceGitMode::IsolatedSnapshot,
            dirty_paths: vec!["src/checkpoint_fixture.rs".into()],
        }),
        before_steps: BTreeMap::new(),
        operations: vec!["prepare-graph".into()],
    };
    let proof = journey_runtime::compile_with_setup(
        &authored,
        "isolated-git-surface",
        "proof",
        vec![prepare, inspect],
        Some(&setup),
        &[OperationBinding {
            step_id: "inspect-fixture".into(),
            operation_id: "inspect-isolated-git".into(),
        }],
    )
    .unwrap();
    let temp_before = std::fs::read_dir(root.path().join(".loom/tmp"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    let detached_before = detached_git_temp_count();
    let report = journey_runtime::execute(root.path(), &authored, &proof, &BTreeMap::new());
    assert_eq!(report.status, RuntimeStatus::Passed, "{report:#?}");
    assert_eq!(report.assertions_passed, 5);
    assert_eq!(report.steps[0].output["commit_count"], 1);
    assert_eq!(
        report.steps[0].output["tracked"],
        json!(["src/checkpoint_fixture.rs"])
    );
    assert_eq!(
        std::fs::read_dir(root.path().join(".loom/tmp"))
            .map(|entries| entries.count())
            .unwrap_or(0),
        temp_before,
        "the isolated repository survived runtime teardown"
    );
    assert_eq!(
        detached_git_temp_count(),
        detached_before,
        "the detached isolated repository survived runtime teardown"
    );

    assert_eq!(
        git_capture(root.path(), &["rev-parse", "HEAD"]),
        head_before
    );
    assert_eq!(
        git_capture(root.path(), &["rev-list", "--count", "HEAD"]),
        commits_before
    );
    assert_eq!(
        git_capture(
            root.path(),
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"]
        ),
        status_before
    );
    assert_eq!(git_capture(root.path(), &["remote", "-v"]), remotes_before);
    assert_eq!(
        std::fs::read(root.path().join(".git/index")).unwrap(),
        index_before
    );
    assert_eq!(
        std::fs::read(root.path().join(".git/config")).unwrap(),
        config_before
    );
    assert_eq!(
        std::fs::read(root.path().join(".git/logs/HEAD")).unwrap(),
        reflog_before
    );
    assert_eq!(std::fs::read(&codefile).unwrap(), source_before);
    assert_eq!(
        std::fs::metadata(&codefile).unwrap().permissions().mode(),
        source_mode_before
    );
}

fn rooted_compass_fixture(root: &TempRoot) -> String {
    loom_command(root.path(), &["init", root.path().to_str().unwrap()]);
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(
        root.path().join("src/compass_fixture.rs"),
        "pub fn compass_fixture() {}\n",
    )
    .unwrap();
    loom_command(
        root.path(),
        &["codefile", "add", "src/compass_fixture.rs", "--json"],
    );

    let authored: JourneySpec = serde_json::from_value(json!({
        "schema": JOURNEY_SCHEMA,
        "id": "compass.fixture",
        "name": "Compass fixture",
        "actor": "operator",
        "goal": "Provide one rooted behavior",
        "inputs": {},
        "preconditions": [],
        "steps": [{
            "id":"root-behavior",
            "name":"Root behavior",
            "action":"does one rooted thing",
            "expects":[],
            "produces":{}
        }],
        "profiles":{"proof":{"inputs":{},"workspace":{}}}
    }))
    .unwrap();
    let artifact = root.path().join("compass-fixture.journey.json");
    std::fs::write(&artifact, serde_json::to_vec_pretty(&authored).unwrap()).unwrap();
    loom_command(
        root.path(),
        &["journey", "add", artifact.to_str().unwrap(), "--json"],
    );
    let derivation = root.path().join("compass-fixture.derive.json");
    std::fs::write(
        &derivation,
        serde_json::to_vec_pretty(&json!({
            "schema":"loom.journey-derivation/v1",
            "journey_id":"compass.fixture",
            "journey_hash":authored.semantic_hash().unwrap(),
            "proposal_id":"compass-fixture-proposal",
            "proposal_rationale":"The fixture Journey needs one rooted technical behavior for isolated runtime setup",
            "intents":[{
                "id":"rooted-compass-intent",
                "operation":"create",
                "name":"a rooted compass fixture is realized",
                "criterion":"the fixture behavior has a current realizing grounding before isolated setup",
                "level":"feature",
                "visibility":"internal",
                "rationale":"A rooted implemented Intent keeps seed and derive below the build rung",
                "step_ids":["root-behavior"]
            }],
            "relationships":[]
        }))
        .unwrap(),
    )
    .unwrap();
    loom_command(
        root.path(),
        &[
            "journey",
            "derive-accept",
            "compass.fixture",
            "--manifest",
            derivation.to_str().unwrap(),
            "--human-decision",
            "The isolated runtime fixture exactly represents the approved rooted behavior.",
            "--json",
        ],
    );
    let store = Store::open(root.path()).unwrap();
    let intent = store
        .resolve_node(
            "a rooted compass fixture is realized",
            Some(NodeType::Intent),
        )
        .unwrap();
    let codefile = store
        .resolve_node("src/compass_fixture.rs", Some(NodeType::CodeFile))
        .unwrap();
    store.set_node_status(&intent.id, "implemented").unwrap();
    let grounding = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &codefile.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &grounding.id,
            TargetKind::Edge,
            "locator",
            "compass_fixture",
            TruthClass::Asserted,
        )
        .unwrap();
    intent.id
}

#[cfg(unix)]
#[test]
fn cli_compile_reconciles_strict_topology_and_run_settles_observation() {
    use std::os::unix::fs::PermissionsExt;

    let root = TempRoot::new("cli");
    loom_command(root.path(), &["init", root.path().to_str().unwrap()]);

    let runner = root.path().join("runner.py");
    std::fs::write(
        &runner,
        "#!/usr/bin/env python3\nimport json\ndef main():\n    print(json.dumps({'ok': True}))\nif __name__ == '__main__':\n    main()\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&runner).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&runner, permissions).unwrap();
    loom_command(root.path(), &["codefile", "add", "runner.py"]);

    let artifact = root.path().join("checkout.journey.json");
    let authored = serde_json::from_value::<JourneySpec>(json!({
        "schema": JOURNEY_SCHEMA,
        "id": "checkout.happy",
        "name": "Checkout succeeds",
        "actor": "shopper",
        "goal": "Complete checkout",
        "inputs": {},
        "preconditions": [],
        "steps": [{"id":"checkout","name":"Checkout","action":"checks out","expects":[],"produces":{}}],
        "profiles":{"proof":{"inputs":{},"workspace":{}}}
    }))
    .unwrap();
    std::fs::write(&artifact, serde_json::to_vec_pretty(&authored).unwrap()).unwrap();
    loom_command(root.path(), &["journey", "add", artifact.to_str().unwrap()]);
    loom_command(
        root.path(),
        &["--json", "journey", "show", "checkout.happy"],
    );
    loom_command(root.path(), &["--json", "journey", "map"]);

    let derivation = root.path().join("derive.json");
    std::fs::write(
        &derivation,
        serde_json::to_vec_pretty(&json!({
            "schema": "loom.journey-derivation/v1",
            "journey_id": "checkout.happy",
            "journey_hash": authored.semantic_hash().unwrap(),
            "proposal_id": "checkout-technical-projection",
            "proposal_rationale": "Checkout needs one falsifiable technical behavior projection",
            "intents": [{
                "id": "checkout-intent",
                "operation": "create",
                "name": "checkout records a result",
                "criterion": "a checkout emits a successful recorded result",
                "level": "feature",
                "visibility": "internal",
                "rationale": "The authored checkout step requires one observable recorded result",
                "step_ids": ["checkout"]
            }],
            "relationships": []
        }))
        .unwrap(),
    )
    .unwrap();
    let other_artifact = root.path().join("other.journey.json");
    std::fs::write(
        &other_artifact,
        serde_json::to_vec_pretty(&json!({
            "schema": JOURNEY_SCHEMA,
            "id": "other.flow",
            "name": "Another flow",
            "actor": "operator",
            "goal": "Do another thing",
            "inputs": {},
            "preconditions": [],
            "steps": [{"id":"other-step","name":"Other step","action":"does another thing","expects":[],"produces":{}}],
            "profiles":{"proof":{"inputs":{},"workspace":{}}}
        }))
        .unwrap(),
    )
    .unwrap();
    loom_command(
        root.path(),
        &["journey", "add", other_artifact.to_str().unwrap()],
    );
    loom_command_failure(
        root.path(),
        &[
            "journey",
            "derive-accept",
            "other.flow",
            "--manifest",
            derivation.to_str().unwrap(),
            "--human-decision",
            "The checkout derivation exactly represents the approved behavior.",
        ],
    );
    loom_command(
        root.path(),
        &[
            "journey",
            "derive-accept",
            "checkout.happy",
            "--manifest",
            derivation.to_str().unwrap(),
            "--human-decision",
            "The checkout derivation exactly represents the approved behavior.",
        ],
    );

    {
        let store = Store::open(root.path()).unwrap();
        let intent = store
            .list_nodes(Some(NodeType::Intent), usize::MAX)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let codefile = store
            .resolve_node("runner.py", Some(NodeType::CodeFile))
            .unwrap();
        store.set_node_status(&intent.id, "implemented").unwrap();
        let grounding = store
            .add_edge(
                EdgeKind::Implements,
                &intent.id,
                &codefile.id,
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .set_facet(
                &grounding.id,
                TargetKind::Edge,
                "locator",
                "main",
                TruthClass::Asserted,
            )
            .unwrap();
    }

    let surface = root.path().join("surface.json");
    std::fs::write(
        &surface,
        serde_json::to_vec_pretty(&json!({
            "schema": "loom.journey.surface/v1",
            "journey_id": "checkout.happy",
            "journey_hash": authored.semantic_hash().unwrap(),
            "surface": {
                "id": "checkout-cli",
                "title": "Checkout CLI",
                "identity": "runner.py",
                "codefile": "runner.py",
                "locator": "main",
                "operations": [{
                    "id": "checkout-op",
                    "summary": "Run checkout",
                    "argv": [runner.to_str().unwrap()],
                    "output": {
                        "format": "json",
                        "assertions": [{
                            "id": "checkout-ok",
                            "pointer": "/ok",
                            "type": "boolean",
                            "equals": true
                        }]
                    }
                }]
            },
            "bindings": [{"step_id":"checkout","operation_id":"checkout-op"}]
        }))
        .unwrap(),
    )
    .unwrap();
    loom_command(
        root.path(),
        &[
            "journey",
            "surface-accept",
            "checkout.happy",
            "--manifest",
            surface.to_str().unwrap(),
        ],
    );
    {
        let store = Store::open(root.path()).unwrap();
        let interface = store
            .list_nodes(Some(NodeType::InterfaceSurface), usize::MAX)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let exposes = store
            .edges_with(Some(EdgeKind::Exposes), Some(&interface.id), None)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        store
            .set_facet(
                &exposes.id,
                TargetKind::Edge,
                "locator",
                "main",
                TruthClass::Asserted,
            )
            .unwrap();
    }

    loom_command(
        root.path(),
        &[
            "--json",
            "journey",
            "compile",
            "checkout.happy",
            "--profile",
            "proof",
        ],
    );
    {
        let store = Store::open(root.path()).unwrap();
        let validations = store
            .list_nodes(Some(NodeType::Validation), usize::MAX)
            .unwrap();
        assert_eq!(validations.len(), 1);
        let validation = &validations[0];
        assert_eq!(validation.name, "journey:checkout.happy:proof");
        assert_eq!(validation.body["type"], "journey");
        assert_eq!(validation.body["profile"], "proof");
        assert_eq!(
            store
                .edges_with(Some(EdgeKind::Proves), Some(&validation.id), None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .edges_with(Some(EdgeKind::Validates), Some(&validation.id), None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .edges_with(Some(EdgeKind::Calls), Some(&validation.id), None)
                .unwrap()
                .len(),
            1
        );
        let exercises = store
            .edges_with(Some(EdgeKind::Exercises), Some(&validation.id), None)
            .unwrap();
        assert_eq!(exercises.len(), 1);
        assert_eq!(
            store
                .get_facet(&exercises[0].id, TargetKind::Edge, "locator")
                .unwrap()
                .as_deref(),
            Some("main")
        );
    }

    loom_command(
        root.path(),
        &[
            "--json",
            "journey",
            "diagnose",
            "checkout.happy",
            "--profile",
            "proof",
        ],
    );
    {
        let store = Store::open(root.path()).unwrap();
        let validation = store
            .resolve_node("journey:checkout.happy:proof", Some(NodeType::Validation))
            .unwrap();
        assert_eq!(validation.status, "not_run");
    }
    loom_command(
        root.path(),
        &[
            "--json",
            "journey",
            "freeze",
            "checkout.happy",
            "--profile",
            "proof",
        ],
    );
    assert!(root
        .path()
        .join(".loom/compiled/journeys/checkout.happy/proof.baseline.json")
        .is_file());
    let drift = loom_command(
        root.path(),
        &["--json", "journey", "drift", "checkout.happy"],
    );
    assert_eq!(drift["stale"], 0);

    let proof_artifact = root
        .path()
        .join(".loom/compiled/journeys/checkout.happy/proof.proof.json");
    std::fs::write(&proof_artifact, b"{\"tampered\":true}\n").unwrap();
    loom_command(
        root.path(),
        &[
            "--json",
            "journey",
            "run",
            "checkout.happy",
            "--profile",
            "proof",
        ],
    );
    assert!(!std::fs::read_to_string(&proof_artifact)
        .unwrap()
        .contains("tampered"));
    let store = Store::open(root.path()).unwrap();
    let validation = store
        .list_nodes(Some(NodeType::Validation), usize::MAX)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(validation.status, "passed");
    for kind in [EdgeKind::Validates, EdgeKind::Proves] {
        let edge = store
            .edges_with(Some(kind), Some(&validation.id), None)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(
            store.edge_verification(&edge.id).unwrap().as_str(),
            "verified"
        );
    }
}

fn loom_command(root: &Path, args: &[&str]) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_loom"))
        .arg("--graph")
        .arg(root)
        .args(args)
        .env_remove("LOOM_AGENT")
        .env_remove("LOOM_AGENT_PROFILE")
        .env("LOOM_NON_INTERACTIVE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "loom {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| json!({"stdout": String::from_utf8_lossy(&output.stdout)}))
}

fn git_capture(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
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

fn detached_git_temp_count() -> usize {
    let prefix = format!("loom-journey-git-{}-", std::process::id());
    std::fs::read_dir(std::env::temp_dir())
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .count()
}

fn loom_command_failure(root: &Path, args: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_loom"))
        .arg("--graph")
        .arg(root)
        .args(args)
        .env_remove("LOOM_AGENT")
        .env_remove("LOOM_AGENT_PROFILE")
        .env("LOOM_NON_INTERACTIVE", "1")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "loom {:?} unexpectedly succeeded\nstdout:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout)
    );
}
