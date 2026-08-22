use crate::journey::{
    CliOperation, JourneySpec, OperationOutput, OutputFormat, SetupGraph,
    DEFAULT_JOURNEY_TIMEOUT_SECONDS,
};
use crate::Result;
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::continuation::{suspend_human_decision, GatePoint};
use super::observation::{blocked_outcome, complete_outcome, ExecutionAnchors, JourneyObservation};
use super::process::{
    capture_execution_anchors, preflight_operation_environment, run_json_operation, TemporaryRoot,
};
use super::temporal::{apply_temporal_file_action, RuntimeTemplateSources};
use super::types::{
    CompiledJourneyProof, ExecutionOutcome, FailedAssertion, FailedCheckKind, FileTransitionReport,
    PassedAssertion, RuntimeReport, RuntimeStatus, SetupReport, StepReport,
};
use super::values::{
    assertion_holds, materialize_setup, profile_for, redact_capture_map, redact_json_secrets,
    redact_pointer, resolve_inputs, runtime_run_id, scalar_text,
};

pub fn execute(
    root: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    overrides: &BTreeMap<String, Value>,
) -> RuntimeReport {
    execute_observed(root, spec, proof, overrides)
        .report()
        .clone()
}

/// Execute a compiled Journey and return an ordinary, UNTRUSTED observation.
///
/// This is a public low-level execution API: the returned observation is a
/// presentation of what ran and is refused by settlement for trusted assertion
/// provenance. Only the Store-owned guarded entrypoint
/// ([`crate::journey::run_and_settle_compiled_validation`] and its
/// interactive/resume siblings) mints observations settlement accepts.
pub fn execute_observed(
    root: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    overrides: &BTreeMap<String, Value>,
) -> JourneyObservation {
    execute_observed_with_anchors(root, spec, proof, overrides, None)
}

/// The Store-owned runtime's execution primitive: execute against `root` and
/// bind the observation to execution-time anchors over `covered_files`.
pub(crate) fn execute_observed_with_anchors(
    root: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    overrides: &BTreeMap<String, Value>,
    covered_files: Option<&[String]>,
) -> JourneyObservation {
    if proof.steps.iter().any(|step| step.human_decision.is_some()) {
        return JourneyObservation::from_executed(
            proof,
            blocked_runtime_report(
                proof,
                "compiled Journey requires host-mediated execution; use the interactive runtime",
            ),
            None,
        );
    }
    match execute_interactive_with_anchors(root, spec, proof, overrides, covered_files) {
        ExecutionOutcome::Completed { observation, .. } => *observation,
        ExecutionOutcome::Pending(_) => JourneyObservation::from_executed(
            proof,
            blocked_runtime_report(
                proof,
                "compiled Journey unexpectedly reached a human decision",
            ),
            None,
        ),
    }
}

/// Execute without ever manufacturing a human answer. A gate returns a
/// structured pending capsule; only a later one-shot resume may continue it.
///
/// Public low-level API: observations minted here are ordinary untrusted
/// reports; settlement refuses them for trusted assertion provenance.
pub fn execute_interactive(
    root: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    overrides: &BTreeMap<String, Value>,
) -> ExecutionOutcome {
    execute_interactive_with_anchors(root, spec, proof, overrides, None)
}

/// The Store-owned interactive execution primitive: binds the observation to
/// execution-time anchors (captured before the first step, persisted through
/// any human-gate continuation, rechecked at resume and after the last step).
pub(crate) fn execute_interactive_with_anchors(
    root: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    overrides: &BTreeMap<String, Value>,
    covered_files: Option<&[String]>,
) -> ExecutionOutcome {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing Journey repository root {}", root.display()));
    let outcome = root.and_then(|root| execute_fresh(&root, spec, proof, overrides, covered_files));
    match outcome {
        Ok(outcome) => outcome,
        Err(error) => blocked_outcome(proof, error.to_string()),
    }
}

pub(crate) fn blocked_runtime_report(
    proof: &CompiledJourneyProof,
    detail: impl Into<String>,
) -> RuntimeReport {
    RuntimeReport {
        journey_id: proof.journey_id.clone(),
        profile: proof.profile.clone(),
        journey_hash: proof.journey_hash.clone(),
        surface_hash: proof.surface_hash.clone(),
        status: RuntimeStatus::Blocked,
        assertions_passed: 0,
        assertions_failed: 0,
        detail: Some(detail.into()),
        setup: Vec::new(),
        file_transitions: Vec::new(),
        steps: Vec::new(),
        captures: BTreeMap::new(),
        passed_assertions: Vec::new(),
        failed_assertions: Vec::new(),
    }
}

fn runtime_surface_plan(
    proof: &CompiledJourneyProof,
) -> Result<crate::candidate_surface_policy::SurfacePlan> {
    let mut operations = Vec::new();
    if let Some(setup) = &proof.setup {
        operations.extend(setup.operations.iter().map(|operation| CliOperation {
            id: operation.operation_id.clone(),
            summary: "compiler-owned setup operation".into(),
            argv: operation.argv.clone(),
            environment: operation.environment.clone(),
            read_only: operation.read_only,
            timeout_seconds: Some(operation.timeout_seconds),
            expected_exit: operation.expected_exit,
            arguments: operation.arguments.clone(),
            output: OperationOutput {
                format: OutputFormat::Json,
                captures: Vec::new(),
                assertions: operation.assertions.clone(),
                redact: operation.redact.clone(),
            },
            exercises: Vec::new(),
        }));
    }
    operations.extend(
        proof
            .steps
            .iter()
            .filter(|step| step.human_decision.is_none())
            .map(|step| CliOperation {
                id: step.operation_id.clone(),
                summary: "compiler-owned Journey step".into(),
                argv: step.argv.clone(),
                environment: step.environment.clone(),
                read_only: step.read_only,
                timeout_seconds: step.timeout_seconds,
                expected_exit: step.expected_exit,
                arguments: step.arguments.clone(),
                output: OperationOutput {
                    format: OutputFormat::Json,
                    captures: step.captures.clone(),
                    assertions: step.assertions.clone(),
                    redact: step.redact.clone(),
                },
                exercises: Vec::new(),
            }),
    );
    crate::candidate_surface_policy::inspect_compiled_operations(
        &proof.journey_id,
        &operations,
        proof.setup.is_some(),
    )
}

fn runtime_confinement(
    proof: &CompiledJourneyProof,
    isolated: bool,
) -> crate::candidate_surface_policy::ActualConfinement {
    if proof.setup.is_some() {
        crate::candidate_surface_policy::ActualConfinement::LocalSnapshot
    } else if isolated {
        crate::candidate_surface_policy::ActualConfinement::FreshIsolated
    } else {
        crate::candidate_surface_policy::ActualConfinement::LiveReadOnly
    }
}

fn execute_fresh(
    root: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    overrides: &BTreeMap<String, Value>,
    covered_files: Option<&[String]>,
) -> Result<ExecutionOutcome> {
    proof.validate()?;
    if proof.journey_id != spec.id || proof.journey_hash != spec.semantic_hash()? {
        bail!("compiled Journey does not match the current authored source");
    }
    // Capture the execution-time anchors immediately before anything may
    // execute: the covered hashes in force now, the canonical execution root,
    // and (as operations spawn) the resolved executable boundary. The caller
    // holds the harness guard across this whole call; settlement persists
    // exactly these hashes and never resamples them.
    let mut anchors = match covered_files {
        Some(files) => Some(capture_execution_anchors(root, files)?),
        None => None,
    };
    match execute_fresh_with_anchors(root, spec, proof, overrides, &mut anchors) {
        Ok(outcome) => Ok(outcome),
        Err(error) => Ok(complete_outcome(
            proof,
            blocked_runtime_report(proof, format!("{error:#}")),
            Vec::new(),
            anchors,
        )),
    }
}

fn execute_fresh_with_anchors(
    root: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    overrides: &BTreeMap<String, Value>,
    anchors: &mut Option<ExecutionAnchors>,
) -> Result<ExecutionOutcome> {
    if let Some(setup) = &proof.setup {
        // Validate the live trust boundary before clone_local_snapshot can
        // dereference an alias into an apparently regular cloned file.
        let live = crate::store::Store::open_read(root)?;
        for actions in setup.before_steps.values() {
            for action in actions {
                action.resolve_for_store(&live)?;
            }
        }
        if let Some(git) = &setup.git {
            git.validate_for_store(&live)?;
        }
    }
    let policy = runtime_surface_plan(proof)?;
    let profile = profile_for(spec, &proof.profile)?;
    let run_id = runtime_run_id(&spec.id, &proof.profile);
    let (inputs, mut secrets, bound_env) =
        resolve_inputs(spec, &proof.profile, profile, overrides, &run_id)?;
    let has_human_decision = proof.steps.iter().any(|step| step.human_decision.is_some());
    let git_isolated = proof
        .setup
        .as_ref()
        .and_then(|setup| setup.git.as_ref())
        .is_some();
    let release_rehearsal = spec.id == "release-workflow" && proof.profile == "proof";
    let temp = if release_rehearsal || has_human_decision {
        TemporaryRoot::create_gate_detached(root)?
    } else if git_isolated {
        TemporaryRoot::create_detached(root)?
    } else {
        TemporaryRoot::create(root)?
    };
    let isolated = proof.setup.is_some() || policy.requires_isolation();
    if let Some(setup) = &proof.setup {
        match setup.graph {
            SetupGraph::LocalSnapshot => {
                let live = crate::store::Store::open_read(root)?;
                live.clone_local_snapshot(temp.path())?;
            }
        }
    } else if isolated {
        // Mutable Loom operations never receive the live repository graph.
        // They run in a fresh Loom-owned graph inside the temporary workspace.
        crate::store::Store::init(temp.path(), Some("Journey proof workspace"), false)?;
    }
    materialize_setup(temp.path(), &profile.workspace)?;
    let captures = BTreeMap::new();
    let redacted_captures = BTreeSet::new();
    secrets.extend(profile.workspace.env.values().cloned());
    let mut execution_env = profile.workspace.env.clone();
    execution_env.extend(bound_env);
    for name in execution_env.keys() {
        if crate::candidate_surface_policy::reserved_runtime_environment(name) {
            bail!("Journey profile declares reserved runtime environment name '{name}'");
        }
    }
    // Resolve every host-supplied operation environment value before the
    // exact release authority is claimed below. Values supplied explicitly by
    // the profile or an input binding retain precedence and are not reread
    // from the host. The resolved map stays separate so a variable declared by
    // one operation is never leaked to another operation.
    let resolved_host_env = preflight_operation_environment(proof, &execution_env, &mut secrets)?;
    if release_rehearsal {
        // Runtime-owned release context is inserted only for the exact outer
        // release-workflow/proof run. Ordinary Journeys must neither consume
        // its one-shot authority nor inherit recursion-suppression context.
        // Authored workspace/input env cannot impersonate these names because
        // the reserved-name check above runs before this insertion.
        execution_env.insert(crate::release::OUTER_JOURNEY_ID_ENV.into(), spec.id.clone());
        execution_env.insert(
            crate::release::OUTER_JOURNEY_PROFILE_ENV.into(),
            proof.profile.clone(),
        );
        execution_env.insert(
            crate::release::OUTER_JOURNEY_RUN_ID_ENV.into(),
            run_id.clone(),
        );
        let (context_capsule_path, context_capsule) =
            crate::release::write_outer_context_capsule(root, temp.path(), spec, proof, &run_id)?;
        execution_env.insert(
            crate::release::OUTER_JOURNEY_HASH_ENV.into(),
            context_capsule.journey_hash,
        );
        execution_env.insert(
            crate::release::OUTER_SURFACE_HASH_ENV.into(),
            context_capsule.surface_hash,
        );
        execution_env.insert(
            crate::release::OUTER_COMPILER_VERSION_ENV.into(),
            context_capsule.compiler_version,
        );
        execution_env.insert(
            crate::release::OUTER_PROOF_HASH_ENV.into(),
            context_capsule.proof_hash,
        );
        execution_env.insert(
            crate::release::OUTER_CONTEXT_CAPSULE_ENV.into(),
            context_capsule_path.to_string_lossy().into_owned(),
        );
    }
    let graph_root = if isolated { temp.path() } else { root };
    let mut setup_reports = Vec::new();
    let mut failed_assertions: Vec<FailedAssertion> = Vec::new();
    let file_transition_reports = Vec::new();
    let reports = Vec::new();
    let assertions_passed = 0usize;
    let git_fixture_paths = if let Some(setup) = &proof.setup {
        let snapshot = crate::store::Store::open_read(temp.path())?;
        for actions in setup.before_steps.values() {
            for action in actions {
                action.resolve_for_store(&snapshot)?;
            }
        }
        if let Some(git) = &setup.git {
            git.validate_for_store(&snapshot)?;
        }
        drop(snapshot);
        setup
            .git
            .as_ref()
            .map(|git| {
                match git.mode {
                    crate::journey::SurfaceGitMode::IsolatedSnapshot => {
                        crate::checkpoint::materialize_isolated_git_snapshot(
                            root,
                            temp.path(),
                            &git.dirty_paths,
                        )?;
                    }
                }
                Ok::<Vec<String>, anyhow::Error>(git.dirty_paths.clone())
            })
            .transpose()?
    } else {
        None
    };

    if let Some(setup) = &proof.setup {
        for operation in &setup.operations {
            let label = format!("setup operation '{}'", operation.operation_id);
            let boundary = anchors
                .as_mut()
                .map(|anchors| &mut anchors.executed_boundary);
            let (display_argv, exit_code, mut output) = match run_json_operation(
                root,
                temp.path(),
                graph_root,
                &policy,
                &operation.operation_id,
                runtime_confinement(proof, isolated),
                &execution_env,
                &resolved_host_env,
                &operation.environment,
                &operation.argv,
                &operation.arguments,
                operation.timeout_seconds,
                operation.expected_exit,
                &inputs,
                &captures,
                &run_id,
                &mut secrets,
                &label,
                boundary,
            ) {
                Ok(observed) => observed,
                Err(error) => {
                    return Ok(complete_outcome(
                        proof,
                        report_with(
                            proof,
                            RuntimeStatus::Blocked,
                            (assertions_passed, 0),
                            error.to_string(),
                            RuntimeProgress {
                                setup: setup_reports,
                                file_transitions: file_transition_reports,
                                steps: reports,
                                captures: redact_capture_map(
                                    captures,
                                    &redacted_captures,
                                    &secrets,
                                ),
                                failed_assertions,
                            },
                        ),
                        Vec::new(),
                        std::mem::take(anchors),
                    ))
                }
            };
            let mut setup_passed = 0usize;
            let mut setup_failed = 0usize;
            for assertion in &operation.assertions {
                if assertion_holds(assertion, &output, &inputs, &captures, &run_id) {
                    setup_passed += 1;
                } else {
                    setup_failed += 1;
                    failed_assertions.push(FailedAssertion {
                        operation_id: operation.operation_id.clone(),
                        assertion_id: assertion.id.clone(),
                        pointer: assertion.pointer.clone(),
                        kind: FailedCheckKind::Assertion,
                    });
                }
            }
            for pointer in &operation.redact {
                redact_pointer(&mut output, pointer);
            }
            redact_json_secrets(&mut output, &secrets);
            setup_reports.push(SetupReport {
                operation_id: operation.operation_id.clone(),
                argv: display_argv,
                exit_code,
                output,
                assertions_passed: setup_passed,
                assertions_failed: setup_failed,
            });
            if setup_failed > 0 {
                return Ok(complete_outcome(
                    proof,
                    report_with(
                        proof,
                        RuntimeStatus::Blocked,
                        (assertions_passed, 0),
                        format!("{label} failed {setup_failed} fixture check(s)"),
                        RuntimeProgress {
                            setup: setup_reports,
                            file_transitions: file_transition_reports,
                            steps: reports,
                            captures: redact_capture_map(captures, &redacted_captures, &secrets),
                            failed_assertions,
                        },
                    ),
                    Vec::new(),
                    std::mem::take(anchors),
                ));
            }
        }
    }
    if let Some(dirty_paths) = &git_fixture_paths {
        crate::checkpoint::verify_isolated_git_snapshot(temp.path(), dirty_paths)?;
    }

    let active = ActiveRun {
        run_id,
        inputs,
        secrets,
        execution_env,
        resolved_host_env,
        setup_reports,
        file_transition_reports,
        reports,
        captures,
        redacted_captures,
        assertions_passed,
        passed_assertions: Vec::new(),
        failed_assertions,
        human_decisions: Vec::new(),
        anchors: std::mem::take(anchors),
    };
    run_steps(root, spec, proof, temp, isolated, 0, active)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActiveRun {
    pub(crate) run_id: String,
    pub(crate) inputs: BTreeMap<String, Value>,
    pub(crate) secrets: Vec<String>,
    pub(crate) execution_env: BTreeMap<String, String>,
    pub(crate) resolved_host_env: BTreeMap<String, String>,
    pub(crate) setup_reports: Vec<SetupReport>,
    pub(crate) file_transition_reports: Vec<FileTransitionReport>,
    pub(crate) reports: Vec<StepReport>,
    pub(crate) captures: BTreeMap<String, Value>,
    pub(crate) redacted_captures: BTreeSet<String>,
    pub(crate) assertions_passed: usize,
    #[serde(default)]
    pub(crate) passed_assertions: Vec<PassedAssertion>,
    #[serde(default)]
    pub(crate) failed_assertions: Vec<FailedAssertion>,
    pub(crate) human_decisions: Vec<Value>,
    /// Execution-time anchors, present only on Store-owned guarded runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) anchors: Option<ExecutionAnchors>,
}

pub(crate) fn run_steps(
    root: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    temp: TemporaryRoot,
    isolated: bool,
    start_index: usize,
    mut active: ActiveRun,
) -> Result<ExecutionOutcome> {
    let policy = runtime_surface_plan(proof)?;
    let graph_root = if isolated { temp.path() } else { root };
    for (step_index, step) in proof.steps.iter().enumerate().skip(start_index) {
        let label = format!("step '{}'", step.step_id);
        if let Some(actions) = proof
            .setup
            .as_ref()
            .and_then(|setup| setup.before_steps.get(&step.step_id))
        {
            for action in actions {
                let sources = RuntimeTemplateSources {
                    spec,
                    inputs: &active.inputs,
                    captures: &active.captures,
                    redacted_captures: &active.redacted_captures,
                    run_id: &active.run_id,
                };
                let outcome =
                    apply_temporal_file_action(root, graph_root, &step.step_id, action, &sources)?;
                let blocked = outcome.detail.clone();
                active.file_transition_reports.push(outcome.report);
                if let Some(detail) = blocked {
                    return Ok(completed_outcome(
                        proof,
                        RuntimeStatus::Blocked,
                        0,
                        Some(detail),
                        active,
                    ));
                }
            }
        }
        if let Some(gate) = &step.human_decision {
            return suspend_human_decision(
                root,
                spec,
                proof,
                temp,
                GatePoint {
                    step_index,
                    step,
                    gate,
                },
                active,
            );
        }
        let boundary = active
            .anchors
            .as_mut()
            .map(|anchors| &mut anchors.executed_boundary);
        let (display_argv, exit_code, mut output) = match run_json_operation(
            root,
            temp.path(),
            graph_root,
            &policy,
            &step.operation_id,
            runtime_confinement(proof, isolated),
            &active.execution_env,
            &active.resolved_host_env,
            &step.environment,
            &step.argv,
            &step.arguments,
            step.timeout_seconds
                .unwrap_or(DEFAULT_JOURNEY_TIMEOUT_SECONDS),
            step.expected_exit,
            &active.inputs,
            &active.captures,
            &active.run_id,
            &mut active.secrets,
            &label,
            boundary,
        ) {
            Ok(observed) => observed,
            Err(error) => {
                return Ok(completed_outcome(
                    proof,
                    RuntimeStatus::Blocked,
                    0,
                    Some(error.to_string()),
                    active,
                ))
            }
        };

        let mut step_passed = 0usize;
        let mut step_failed = 0usize;
        for assertion in &step.assertions {
            if assertion_holds(
                assertion,
                &output,
                &active.inputs,
                &active.captures,
                &active.run_id,
            ) {
                step_passed += 1;
                active.passed_assertions.push(PassedAssertion {
                    operation_id: step.operation_id.clone(),
                    assertion_id: assertion.id.clone(),
                });
            } else {
                step_failed += 1;
                active.failed_assertions.push(FailedAssertion {
                    operation_id: step.operation_id.clone(),
                    assertion_id: assertion.id.clone(),
                    pointer: assertion.pointer.clone(),
                    kind: FailedCheckKind::Assertion,
                });
            }
        }
        for capture in &step.captures {
            let crate::journey::Resolved::Unique(value) =
                crate::journey::resolve_pointer(&output, &capture.pointer)
            else {
                step_failed += 1;
                active.failed_assertions.push(FailedAssertion {
                    operation_id: step.operation_id.clone(),
                    assertion_id: capture.id.clone(),
                    pointer: capture.pointer.clone(),
                    kind: FailedCheckKind::CaptureMissing,
                });
                continue;
            };
            if !capture.value_type.accepts(value) {
                step_failed += 1;
                active.failed_assertions.push(FailedAssertion {
                    operation_id: step.operation_id.clone(),
                    assertion_id: capture.id.clone(),
                    pointer: capture.pointer.clone(),
                    kind: FailedCheckKind::CaptureType,
                });
                continue;
            }
            let capture_key = format!("steps.{}.outputs.{}", step.step_id, capture.id);
            if capture.redact {
                if let Some(secret) = scalar_text(value) {
                    active.secrets.push(secret);
                }
                active.redacted_captures.insert(capture_key.clone());
            }
            active.captures.insert(capture_key, value.clone());
        }
        for pointer in &step.redact {
            redact_pointer(&mut output, pointer);
        }
        for capture in &step.captures {
            if capture.redact {
                redact_pointer(&mut output, &capture.pointer);
            }
        }
        redact_json_secrets(&mut output, &active.secrets);
        active.reports.push(StepReport {
            step_id: step.step_id.clone(),
            operation_id: step.operation_id.clone(),
            argv: display_argv,
            exit_code,
            output,
            assertions_passed: step_passed,
            assertions_failed: step_failed,
        });
        active.assertions_passed += step_passed;
        if step_failed > 0 {
            return Ok(completed_outcome(
                proof,
                RuntimeStatus::Failed,
                step_failed,
                Some(format!(
                    "step '{}' failed {step_failed} typed check(s)",
                    step.step_id
                )),
                active,
            ));
        }
    }

    Ok(completed_outcome(
        proof,
        RuntimeStatus::Passed,
        0,
        None,
        active,
    ))
}

fn completed_outcome(
    proof: &CompiledJourneyProof,
    status: RuntimeStatus,
    assertions_failed: usize,
    detail: Option<String>,
    mut active: ActiveRun,
) -> ExecutionOutcome {
    // Immediate post-execution recheck, under the same guard the caller holds:
    // the covered files must still match the pre-execution hashes, or the
    // run's execution-time evidence is invalidated and the report is blocked.
    let (status, detail) = match &active.anchors {
        Some(anchors) if !anchors.covered_still_match(&anchors.execution_root) => (
            RuntimeStatus::Blocked,
            Some(
                "a covered file changed during Journey execution; execution-time evidence \
                 was invalidated"
                    .into(),
            ),
        ),
        _ => (status, detail),
    };
    let anchors = active.anchors.take();
    let captures = redact_capture_map(active.captures, &active.redacted_captures, &active.secrets);
    let mut report = RuntimeReport {
        journey_id: proof.journey_id.clone(),
        profile: proof.profile.clone(),
        journey_hash: proof.journey_hash.clone(),
        surface_hash: proof.surface_hash.clone(),
        status,
        assertions_passed: active.assertions_passed,
        assertions_failed,
        detail,
        setup: active.setup_reports,
        file_transitions: active.file_transition_reports,
        steps: active.reports,
        captures,
        passed_assertions: active.passed_assertions,
        failed_assertions: active.failed_assertions,
    };
    if report.status == RuntimeStatus::Blocked {
        // A blocked-by-recheck run observed nothing trustworthy.
        report.passed_assertions.clear();
        report.assertions_passed = 0;
    }
    complete_outcome(proof, report, active.human_decisions, anchors)
}

struct RuntimeProgress {
    setup: Vec<SetupReport>,
    file_transitions: Vec<FileTransitionReport>,
    steps: Vec<StepReport>,
    captures: BTreeMap<String, Value>,
    failed_assertions: Vec<FailedAssertion>,
}

fn report_with(
    proof: &CompiledJourneyProof,
    status: RuntimeStatus,
    assertions: (usize, usize),
    detail: String,
    progress: RuntimeProgress,
) -> RuntimeReport {
    RuntimeReport {
        journey_id: proof.journey_id.clone(),
        profile: proof.profile.clone(),
        journey_hash: proof.journey_hash.clone(),
        surface_hash: proof.surface_hash.clone(),
        status,
        assertions_passed: assertions.0,
        assertions_failed: assertions.1,
        detail: Some(detail),
        setup: progress.setup,
        file_transitions: progress.file_transitions,
        steps: progress.steps,
        captures: progress.captures,
        passed_assertions: Vec::new(),
        failed_assertions: progress.failed_assertions,
    }
}
