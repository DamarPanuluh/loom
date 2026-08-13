//! Compiler-owned runtime for semantic Journeys.
//!
//! The authored Journey remains transport-free. This module compiles an
//! accepted CLI projection into deterministic data, then executes only direct
//! argv arrays. It has no shell or HTTP execution path.

use crate::journey::{
    CliOperation, HumanDecisionSource, JourneyInput, JourneyProfile, JourneySpec,
    OperationArgument, OperationBinding, OperationOutput, OutputAssertion, OutputCapture,
    OutputFormat, RuntimeSource, SetupGraph, SurfaceBinding, SurfaceFileAction, SurfaceGitSetup,
    SurfaceSetup, TemporarySetup, ValueType, BASELINE_SCHEMA, COMPILED_PROOF_SCHEMA,
    DEFAULT_JOURNEY_TIMEOUT_SECONDS, JOURNEY_COMPILER_VERSION,
};
use crate::Result;
use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Wall-clock ceiling for a single compiled Journey step.
///
/// This bounds a runaway or wedged child; it is not a performance budget and
/// no invariant depends on its exact value. The release rehearsal's
/// `verify-isolated-dogfood` step is the widest legitimate consumer: it copies
/// the candidate into a detached root and runs `cargo fmt`, `clippy`, `test`,
/// and `build` against a cold `target/`, which alone exceeds five minutes on a
/// normal machine. The previous 300s ceiling cut that honest work off mid-gate
/// and reported it as a Journey failure, so a passing earlier gate perversely
/// made the step likelier to time out by letting it reach the expensive ones.
// 2700s: the release fixpoint step builds and tests TWO detached candidates
// back to back (full cargo build + --all-targets suite + 30-journey dogfood
// each); 900s starved it on a warm laptop once every earlier gate finally
// passed, which is the same perverse pattern this constant was last raised for.
const STREAM_EXCERPT_BYTES: usize = 512 * 1024;
const FAILURE_DIAGNOSTIC_BYTES: usize = 64 * 1024;
const REDACTED: &str = "[REDACTED]";
/// Executor infrastructure inherited independently of authored operation
/// declarations. This list is deliberately small and platform-specific.
#[cfg(not(windows))]
pub const EXECUTOR_PLATFORM_ENVIRONMENT: &[&str] = &["PATH", "TMPDIR", "TEMP", "TMP"];
/// Windows process-launch essentials in addition to the portable executor
/// infrastructure names.
#[cfg(windows)]
pub const EXECUTOR_PLATFORM_ENVIRONMENT: &[&str] = &[
    "PATH",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SYSTEMROOT",
    "WINDIR",
    "PATHEXT",
    "COMSPEC",
];
type ResolvedInputs = (
    BTreeMap<String, Value>,
    Vec<String>,
    BTreeMap<String, String>,
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledJourneyProof {
    pub schema: String,
    pub compiler_version: String,
    pub journey_id: String,
    pub journey_hash: String,
    pub surface_hash: String,
    pub profile: String,
    pub profile_shape: CompiledProfileShape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<CompiledSetup>,
    pub steps: Vec<CompiledStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledSetup {
    pub graph: SetupGraph,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<SurfaceGitSetup>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub before_steps: BTreeMap<String, Vec<SurfaceFileAction>>,
    pub operations: Vec<CompiledSetupOperation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledSetupOperation {
    pub operation_id: String,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<String>,
    pub read_only: bool,
    #[serde(default = "default_compiled_timeout_seconds")]
    pub timeout_seconds: u64,
    pub arguments: Vec<OperationArgument>,
    pub assertions: Vec<OutputAssertion>,
    pub redact: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledProfileShape {
    pub input_ids: Vec<String>,
    pub setup_directories: Vec<String>,
    pub setup_files: Vec<String>,
    pub setup_env: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledStep {
    pub step_id: String,
    pub operation_id: String,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<String>,
    pub read_only: bool,
    /// Machine operations have a resolved timeout; human gates deliberately do not.
    #[serde(default = "default_compiled_optional_timeout_seconds")]
    pub timeout_seconds: Option<u64>,
    pub arguments: Vec<OperationArgument>,
    pub captures: Vec<OutputCapture>,
    pub assertions: Vec<OutputAssertion>,
    pub redact: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_decision: Option<CompiledHumanDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledHumanDecision {
    pub source_operation_id: String,
    pub pointer: String,
}

fn default_compiled_timeout_seconds() -> u64 {
    DEFAULT_JOURNEY_TIMEOUT_SECONDS
}

fn default_compiled_optional_timeout_seconds() -> Option<u64> {
    Some(DEFAULT_JOURNEY_TIMEOUT_SECONDS)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Passed,
    Failed,
    Blocked,
}

impl RuntimeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PassedAssertion {
    pub operation_id: String,
    pub assertion_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeReport {
    pub journey_id: String,
    pub profile: String,
    pub journey_hash: String,
    pub surface_hash: String,
    pub status: RuntimeStatus,
    pub assertions_passed: usize,
    pub assertions_failed: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup: Vec<SetupReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_transitions: Vec<FileTransitionReport>,
    pub steps: Vec<StepReport>,
    pub captures: BTreeMap<String, Value>,
    /// Typed output assertions that held during this observed run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub passed_assertions: Vec<PassedAssertion>,
}

#[derive(Debug)]
pub enum ExecutionOutcome {
    Completed {
        report: RuntimeReport,
        /// Sealed capability minted only by this runtime. External crates can
        /// read it from a completed execution and pass it to settlement, but
        /// cannot construct one from a caller-authored report.
        observation: Box<JourneyObservation>,
        human_decisions: Vec<Value>,
    },
    Pending(crate::journey_gate::PendingHuman),
}

/// Proof that the compiler-owned Journey runtime observed a completed run.
///
/// Private fields: only this module can mint one, and only after executing the
/// compiled proof. Deserialize and struct literals from outside this crate
/// cannot express it.
#[derive(Debug, Clone)]
pub struct JourneyObservation {
    report: RuntimeReport,
    proof: CompiledJourneyProof,
}

impl JourneyObservation {
    /// Presentation of the observed run. Never a settlement input by itself.
    pub fn report(&self) -> &RuntimeReport {
        &self.report
    }

    pub(crate) fn proof(&self) -> &CompiledJourneyProof {
        &self.proof
    }

    /// True only when this observation still matches the compiled proof that
    /// produced it. Settlement refuses to mint trusted assertion provenance
    /// unless this holds.
    pub(crate) fn matches_compiled_proof(&self) -> bool {
        observation_matches_proof(&self.proof, &self.report)
    }

    fn from_executed(proof: &CompiledJourneyProof, mut report: RuntimeReport) -> Self {
        if !observation_matches_proof(proof, &report) {
            report.status = RuntimeStatus::Blocked;
            report.passed_assertions.clear();
            report.assertions_passed = 0;
            report.detail = Some(
                "runtime result did not match the compiled Journey proof it claims to settle"
                    .into(),
            );
        }
        Self {
            report,
            proof: proof.clone(),
        }
    }
}

fn observation_matches_proof(proof: &CompiledJourneyProof, report: &RuntimeReport) -> bool {
    if proof.compiler_version != JOURNEY_COMPILER_VERSION
        || report.journey_id != proof.journey_id
        || report.profile != proof.profile
        || report.journey_hash != proof.journey_hash
        || report.surface_hash != proof.surface_hash
    {
        return false;
    }
    let allowed = compiled_assertion_ids(proof);
    report
        .passed_assertions
        .iter()
        .all(|passed| allowed.contains(&(passed.operation_id.clone(), passed.assertion_id.clone())))
}

fn compiled_assertion_ids(proof: &CompiledJourneyProof) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    if let Some(setup) = &proof.setup {
        for operation in &setup.operations {
            for assertion in &operation.assertions {
                out.insert((operation.operation_id.clone(), assertion.id.clone()));
            }
        }
    }
    for step in &proof.steps {
        for assertion in &step.assertions {
            out.insert((step.operation_id.clone(), assertion.id.clone()));
        }
    }
    out
}

fn complete_outcome(
    proof: &CompiledJourneyProof,
    report: RuntimeReport,
    human_decisions: Vec<Value>,
) -> ExecutionOutcome {
    let observation = JourneyObservation::from_executed(proof, report.clone());
    ExecutionOutcome::Completed {
        report: observation.report.clone(),
        observation: Box::new(observation),
        human_decisions,
    }
}

fn blocked_outcome(proof: &CompiledJourneyProof, detail: impl Into<String>) -> ExecutionOutcome {
    complete_outcome(proof, blocked_runtime_report(proof, detail), Vec::new())
}

impl ExecutionOutcome {
    pub(crate) fn blocked(proof: &CompiledJourneyProof, detail: impl Into<String>) -> Self {
        blocked_outcome(proof, detail)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingContinuation {
    pub binding: crate::journey_gate::GateBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileTransitionReport {
    pub step_id: String,
    pub path: String,
    pub expected_hash: String,
    pub observed_before_hash: String,
    pub observed_after_hash: String,
    pub changed: bool,
    pub applied: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepReport {
    pub step_id: String,
    pub operation_id: String,
    pub argv: Vec<String>,
    pub exit_code: i64,
    pub output: Value,
    pub assertions_passed: usize,
    pub assertions_failed: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupReport {
    pub operation_id: String,
    pub argv: Vec<String>,
    pub exit_code: i64,
    pub output: Value,
    pub assertions_passed: usize,
    pub assertions_failed: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JourneyBaseline {
    pub schema: String,
    pub compiler_version: String,
    pub journey_id: String,
    pub journey_hash: String,
    pub surface_hash: String,
    pub profile: String,
    pub report: RuntimeReport,
}

pub fn compile(
    spec: &JourneySpec,
    surface_hash: &str,
    profile: &str,
    operations: Vec<CliOperation>,
    bindings: &[OperationBinding],
) -> Result<CompiledJourneyProof> {
    compile_with_setup(spec, surface_hash, profile, operations, None, bindings)
}

pub fn compile_with_setup(
    spec: &JourneySpec,
    surface_hash: &str,
    profile: &str,
    operations: Vec<CliOperation>,
    setup: Option<&SurfaceSetup>,
    bindings: &[OperationBinding],
) -> Result<CompiledJourneyProof> {
    let bindings: Vec<SurfaceBinding> = bindings.iter().cloned().map(Into::into).collect();
    compile_surface(spec, surface_hash, profile, operations, setup, &bindings)
}

/// Compile the exact accepted manifest binding union. Direct callers that
/// only need CLI operations keep using [`compile_with_setup`]; the command
/// adapter uses this entry point so intrinsic human gates survive compilation.
pub fn compile_surface(
    spec: &JourneySpec,
    surface_hash: &str,
    profile: &str,
    operations: Vec<CliOperation>,
    setup: Option<&SurfaceSetup>,
    bindings: &[SurfaceBinding],
) -> Result<CompiledJourneyProof> {
    spec.validate()?;
    for operation in &operations {
        if operation.timeout_seconds == Some(0) {
            bail!(
                "operation '{}' timeout_seconds must be positive",
                operation.id
            );
        }
    }
    // Derive capabilities from the typed operations before compiling them.
    // `read_only` is checked by this policy; it never decides confinement by
    // itself.
    crate::candidate_surface_policy::inspect_surface(
        spec,
        &spec.id,
        "compiled-surface",
        &operations,
        setup,
        bindings,
        crate::candidate_surface_policy::PolicyMode::Runtime,
    )?;
    let profile_name = profile;
    let profile = profile_for(spec, profile)?;
    if bindings.len() != spec.steps.len() {
        bail!(
            "compiled Journey requires exactly one primary operation per step ({} steps, {} bindings)",
            spec.steps.len(),
            bindings.len()
        );
    }
    let unique_steps: BTreeSet<&str> = bindings.iter().map(SurfaceBinding::step_id).collect();
    let unique_operations: BTreeSet<&str> = bindings
        .iter()
        .filter_map(SurfaceBinding::operation_id)
        .collect();
    let operation_binding_count = bindings
        .iter()
        .filter(|binding| binding.operation_id().is_some())
        .count();
    if unique_steps.len() != bindings.len() || unique_operations.len() != operation_binding_count {
        bail!("compiled Journey bindings repeat a step or primary operation");
    }
    let by_operation: BTreeMap<&str, &CliOperation> = operations
        .iter()
        .map(|operation| (operation.id.as_str(), operation))
        .collect();
    let by_step: BTreeMap<&str, &SurfaceBinding> = bindings
        .iter()
        .map(|binding| (binding.step_id(), binding))
        .collect();
    let input_by_id: BTreeMap<&str, &JourneyInput> = spec
        .inputs
        .iter()
        .map(|(id, input)| (id.as_str(), input))
        .collect();
    let has_human_decision = bindings
        .iter()
        .any(|binding| binding.human_decision().is_some());
    if has_human_decision && setup.is_none() {
        bail!("compiled human decision bindings require setup.graph=local_snapshot");
    }
    let compiled_setup = setup
        .map(|setup| {
            if setup.operations.is_empty()
                && !setup
                    .before_steps
                    .values()
                    .any(|actions| !actions.is_empty())
                && !has_human_decision
            {
                bail!(
                    "compiled Journey setup must contain an operation or before_steps file action"
                );
            }
            let bound_operations: BTreeSet<&str> =
                bindings.iter().filter_map(SurfaceBinding::operation_id).collect();
            let authored_steps: BTreeSet<&str> =
                spec.steps.iter().map(|step| step.id.as_str()).collect();
            for (step_id, actions) in &setup.before_steps {
                if !authored_steps.contains(step_id.as_str()) {
                    bail!("compiled Journey before_steps references unknown step '{step_id}'");
                }
                if actions.is_empty() {
                    bail!("compiled Journey before_steps.{step_id} has no file action");
                }
                let mut paths = BTreeSet::new();
                for action in actions {
                    action.validate()?;
                    if !paths.insert(action.path.as_str()) {
                        bail!(
                            "compiled Journey before_steps.{step_id} repeats path '{}'",
                            action.path
                        );
                    }
                }
            }
            let mut setup_ids = BTreeSet::new();
            let no_outputs = BTreeMap::new();
            let mut compiled = Vec::with_capacity(setup.operations.len());
            for operation_id in &setup.operations {
                if !setup_ids.insert(operation_id.as_str()) {
                    bail!("compiled Journey setup repeats operation '{operation_id}'");
                }
                if bound_operations.contains(operation_id.as_str()) {
                    bail!(
                        "compiled Journey setup operation '{operation_id}' is also a primary step operation"
                    );
                }
                let operation = by_operation.get(operation_id.as_str()).ok_or_else(|| {
                    anyhow!("compiled Journey setup operation '{operation_id}' is missing")
                })?;
                if operation.read_only {
                    bail!("compiled Journey setup operation '{operation_id}' must be mutable");
                }
                if !operation.output.captures.is_empty() {
                    bail!(
                        "compiled Journey setup operation '{operation_id}' must not capture authored outputs"
                    );
                }
                if operation.output.assertions.is_empty() {
                    bail!(
                        "compiled Journey setup operation '{operation_id}' must assert its fixture"
                    );
                }
                validate_sources(
                    &operation.argv,
                    &operation.arguments,
                    &operation.output.assertions,
                    &input_by_id,
                    &no_outputs,
                )?;
                compiled.push(CompiledSetupOperation {
                    operation_id: operation.id.clone(),
                    argv: operation.argv.clone(),
                    environment: canonical_environment(&operation.environment)?,
                    read_only: operation.read_only,
                    timeout_seconds: operation.timeout_seconds.unwrap_or(profile.timeout_seconds),
                    arguments: operation.arguments.clone(),
                    assertions: operation.output.assertions.clone(),
                    redact: operation.output.redact.clone(),
                });
            }
            Ok(CompiledSetup {
                graph: setup.graph,
                git: setup.git.clone(),
                before_steps: setup.before_steps.clone(),
                operations: compiled,
            })
        })
        .transpose()?;
    let mut available_outputs = BTreeMap::new();
    let mut steps = Vec::with_capacity(spec.steps.len());
    let mut assertion_count = 0usize;

    for semantic_step in &spec.steps {
        if let Some(actions) = setup.and_then(|setup| setup.before_steps.get(&semantic_step.id)) {
            for action in actions {
                validate_temporal_sources(action, &input_by_id, &available_outputs).with_context(
                    || {
                        format!(
                            "compiled before_steps.{} path '{}'",
                            semantic_step.id, action.path
                        )
                    },
                )?;
            }
        }
        let binding = by_step.get(semantic_step.id.as_str()).ok_or_else(|| {
            anyhow!(
                "Journey step '{}' has no primary operation",
                semantic_step.id
            )
        })?;
        match binding {
            SurfaceBinding::Operation(binding) => {
                let operation =
                    by_operation
                        .get(binding.operation_id.as_str())
                        .ok_or_else(|| {
                            anyhow!("surface operation '{}' is missing", binding.operation_id)
                        })?;
                validate_sources(
                    &operation.argv,
                    &operation.arguments,
                    &operation.output.assertions,
                    &input_by_id,
                    &available_outputs,
                )?;
                for capture in &operation.output.captures {
                    available_outputs.insert(
                        format!("steps.{}.outputs.{}", semantic_step.id, capture.id),
                        (capture.value_type, capture.redact),
                    );
                }
                assertion_count += operation.output.assertions.len();
                steps.push(CompiledStep {
                    step_id: semantic_step.id.clone(),
                    operation_id: operation.id.clone(),
                    argv: operation.argv.clone(),
                    environment: canonical_environment(&operation.environment)?,
                    read_only: operation.read_only,
                    timeout_seconds: Some(
                        operation.timeout_seconds.unwrap_or(profile.timeout_seconds),
                    ),
                    arguments: operation.arguments.clone(),
                    captures: operation.output.captures.clone(),
                    assertions: operation.output.assertions.clone(),
                    redact: operation.output.redact.clone(),
                    human_decision: None,
                });
            }
            SurfaceBinding::HumanDecision(binding) => {
                compile_human_decision_step(semantic_step, &binding.human_decision, &steps)?;
                steps.push(CompiledStep {
                    step_id: semantic_step.id.clone(),
                    operation_id: "human-decision".into(),
                    argv: Vec::new(),
                    environment: Vec::new(),
                    read_only: true,
                    timeout_seconds: None,
                    arguments: Vec::new(),
                    captures: Vec::new(),
                    assertions: Vec::new(),
                    redact: Vec::new(),
                    human_decision: Some(CompiledHumanDecision {
                        source_operation_id: binding.human_decision.operation_id.clone(),
                        pointer: binding.human_decision.pointer.clone(),
                    }),
                });
            }
        }
    }
    if assertion_count == 0 {
        bail!(
            "Journey '{}' surface has no typed output assertions; a compiled proof must check content",
            spec.id
        );
    }

    // Defaults are valid runtime sources even when the selected profile does
    // not override them, so the compiled shape records every authored input.
    let mut profile_input_ids: Vec<String> = spec.inputs.keys().cloned().collect();
    profile_input_ids.sort();
    let mut setup_directories = profile.workspace.directories.clone();
    setup_directories.sort();
    setup_directories.dedup();
    let mut setup_files: Vec<String> = profile
        .workspace
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    setup_files.sort();
    let setup_env = profile.workspace.env.keys().cloned().collect();

    Ok(CompiledJourneyProof {
        schema: COMPILED_PROOF_SCHEMA.into(),
        compiler_version: JOURNEY_COMPILER_VERSION.into(),
        journey_id: spec.id.clone(),
        journey_hash: spec.semantic_hash()?,
        surface_hash: surface_hash.into(),
        profile: profile_name.to_string(),
        profile_shape: CompiledProfileShape {
            input_ids: profile_input_ids,
            setup_directories,
            setup_files,
            setup_env,
        },
        setup: compiled_setup,
        steps,
    })
}

fn canonical_environment(environment: &[String]) -> Result<Vec<String>> {
    let mut environment = environment.to_vec();
    environment.sort();
    validate_compiled_environment("compiled operation", &environment)?;
    Ok(environment)
}

fn compile_human_decision_step(
    semantic_step: &crate::journey::JourneyStep,
    source: &HumanDecisionSource,
    prior_steps: &[CompiledStep],
) -> Result<()> {
    source.validate()?;
    if !prior_steps
        .iter()
        .any(|step| step.human_decision.is_none() && step.operation_id == source.operation_id)
    {
        bail!(
            "human decision step '{}' must reference an operation bound to an earlier authored step (found '{}')",
            semantic_step.id,
            source.operation_id
        );
    }
    if !semantic_step.produces.is_empty() {
        bail!(
            "human decision step '{}' cannot declare produced machine outputs",
            semantic_step.id
        );
    }
    Ok(())
}

fn validate_sources(
    argv: &[String],
    arguments: &[OperationArgument],
    assertions: &[OutputAssertion],
    inputs: &BTreeMap<&str, &JourneyInput>,
    prior_outputs: &BTreeMap<String, (ValueType, bool)>,
) -> Result<()> {
    for (index, token) in argv.iter().enumerate() {
        if let Some(source) = crate::journey::argv_token_source(token)? {
            if index == 0 {
                bail!("compiled Journey executable cannot be a runtime argv template");
            }
            validate_scalar_source(source, inputs, prior_outputs, false)
                .with_context(|| format!("argv token #{} source is unavailable", index + 1))?;
        }
    }
    for argument in arguments {
        let default_source = format!("inputs.{}", argument.id);
        let source = argument.source.as_deref().unwrap_or(&default_source);
        validate_source_reference(source, inputs, prior_outputs)
            .with_context(|| format!("argument '{}' source is unavailable", argument.id))?;
        if let RuntimeSource::Input(id) = crate::journey::parse_runtime_source(source)? {
            if inputs.get(id).is_some_and(|input| input.secret) {
                bail!(
                    "argument '{}' reads secret input '{}'; secret inputs are environment-only and must not enter CLI argv",
                    argument.id,
                    id
                );
            }
        }
    }
    for assertion in assertions {
        if let Some(source) = assertion.runtime_source() {
            validate_source_reference(source, inputs, prior_outputs)
                .with_context(|| format!("assertion '{}' source is unavailable", assertion.id))?;
        }
    }
    Ok(())
}

fn validate_temporal_sources(
    action: &SurfaceFileAction,
    inputs: &BTreeMap<&str, &JourneyInput>,
    prior_outputs: &BTreeMap<String, (ValueType, bool)>,
) -> Result<()> {
    action.validate()?;
    let Some(template) = &action.template else {
        return Ok(());
    };
    for source in crate::journey::template_references(template)? {
        validate_scalar_source(source, inputs, prior_outputs, true)?;
    }
    Ok(())
}

fn validate_scalar_source(
    source: &str,
    inputs: &BTreeMap<&str, &JourneyInput>,
    prior_outputs: &BTreeMap<String, (ValueType, bool)>,
    allow_run_id: bool,
) -> Result<()> {
    validate_source_reference(source, inputs, prior_outputs)?;
    match crate::journey::parse_runtime_source(source)? {
        RuntimeSource::RunId if allow_run_id => Ok(()),
        RuntimeSource::RunId => bail!("run.id cannot replace an argv token"),
        RuntimeSource::Input(id) => {
            let input = inputs
                .get(id)
                .expect("source existence validated before scalar policy");
            if input.secret {
                bail!("secret input '{id}' cannot enter argv or file content");
            }
            if !input.value_type.is_scalar() {
                bail!("input '{id}' is not scalar");
            }
            Ok(())
        }
        RuntimeSource::StepOutput { .. } => {
            let (value_type, redact) = prior_outputs
                .get(source)
                .expect("source availability validated before scalar policy");
            if *redact {
                bail!("redacted output '{source}' cannot enter argv or file content");
            }
            if !value_type.is_scalar() {
                bail!("output '{source}' is not scalar");
            }
            Ok(())
        }
    }
}

fn validate_source_reference(
    source: &str,
    inputs: &BTreeMap<&str, &JourneyInput>,
    prior_outputs: &BTreeMap<String, (ValueType, bool)>,
) -> Result<()> {
    match crate::journey::parse_runtime_source(source)? {
        RuntimeSource::RunId => Ok(()),
        RuntimeSource::Input(id) if inputs.contains_key(id) => Ok(()),
        RuntimeSource::Input(id) => bail!("unknown Journey input '{id}'"),
        RuntimeSource::StepOutput { .. } if prior_outputs.contains_key(source) => Ok(()),
        RuntimeSource::StepOutput { .. } => {
            bail!("'{source}' is not available from an earlier step")
        }
    }
}

pub fn canonical_bytes(proof: &CompiledJourneyProof) -> Result<Vec<u8>> {
    proof.validate()?;
    let canonical = canonicalize(serde_json::to_value(proof)?);
    let mut bytes = serde_json::to_vec_pretty(&canonical)?;
    bytes.push(b'\n');
    Ok(bytes)
}

impl CompiledJourneyProof {
    pub fn validate(&self) -> Result<()> {
        if self.schema != COMPILED_PROOF_SCHEMA {
            bail!("unsupported compiled Journey schema '{}'", self.schema);
        }
        if self.compiler_version != JOURNEY_COMPILER_VERSION {
            bail!(
                "compiled Journey compiler version '{}' is not current ('{}')",
                self.compiler_version,
                JOURNEY_COMPILER_VERSION
            );
        }
        crate::journey::validate_stable_id("journey", &self.journey_id)?;
        crate::journey::validate_stable_id("profile", &self.profile)?;
        if self.journey_hash.trim().is_empty() || self.surface_hash.trim().is_empty() {
            bail!("compiled Journey hashes must not be empty");
        }
        if self.steps.is_empty() {
            bail!("compiled Journey must contain at least one step");
        }
        if self.steps.iter().any(|step| {
            step.human_decision.is_some() != step.timeout_seconds.is_none()
                || step.timeout_seconds == Some(0)
        }) {
            bail!(
                "compiled Journey machine timeouts must be positive and human gates must have none"
            );
        }
        validate_compiled_step_shapes(self)?;
        if let Some(setup) = &self.setup {
            if setup
                .operations
                .iter()
                .any(|operation| operation.timeout_seconds == 0)
            {
                bail!("compiled setup operation timeout_seconds must be positive");
            }
            if setup.operations.is_empty()
                && !setup
                    .before_steps
                    .values()
                    .any(|actions| !actions.is_empty())
                && !self.steps.iter().any(|step| step.human_decision.is_some())
            {
                bail!(
                    "compiled Journey setup must contain an operation or before_steps file action"
                );
            }
            if let Some(git) = &setup.git {
                match setup.graph {
                    SetupGraph::LocalSnapshot => git.validate()?,
                }
            }
            let mut ids = BTreeSet::new();
            let step_operations: BTreeSet<&str> = self
                .steps
                .iter()
                .filter(|step| step.human_decision.is_none())
                .map(|step| step.operation_id.as_str())
                .collect();
            let step_ids: BTreeSet<&str> = self
                .steps
                .iter()
                .map(|step| step.step_id.as_str())
                .collect();
            for (step_id, actions) in &setup.before_steps {
                if !step_ids.contains(step_id.as_str()) {
                    bail!("compiled Journey before_steps references unknown step '{step_id}'");
                }
                if actions.is_empty() {
                    bail!("compiled Journey before_steps.{step_id} has no file action");
                }
                let mut paths = BTreeSet::new();
                for action in actions {
                    action.validate()?;
                    if !paths.insert(action.path.as_str()) {
                        bail!(
                            "compiled Journey before_steps.{step_id} repeats path '{}'",
                            action.path
                        );
                    }
                }
            }
            for operation in &setup.operations {
                crate::journey::validate_stable_id("setup operation", &operation.operation_id)?;
                if !ids.insert(operation.operation_id.as_str()) {
                    bail!(
                        "compiled Journey setup repeats operation '{}'",
                        operation.operation_id
                    );
                }
                if operation.read_only {
                    bail!(
                        "compiled Journey setup operation '{}' must be mutable",
                        operation.operation_id
                    );
                }
                if operation.assertions.is_empty() {
                    bail!(
                        "compiled Journey setup operation '{}' has no fixture assertion",
                        operation.operation_id
                    );
                }
                validate_compiled_environment(
                    &format!("compiled setup operation '{}'", operation.operation_id),
                    &operation.environment,
                )?;
                if step_operations.contains(operation.operation_id.as_str()) {
                    bail!(
                        "compiled Journey setup operation '{}' is also a primary step operation",
                        operation.operation_id
                    );
                }
            }
        }
        validate_compiled_runtime_sources(self)?;
        Ok(())
    }
}

fn validate_compiled_step_shapes(proof: &CompiledJourneyProof) -> Result<()> {
    let mut step_ids = BTreeSet::new();
    let mut prior_operations = BTreeSet::new();
    let mut operation_ids = BTreeSet::new();
    let mut semantic_assertions = 0usize;
    for step in &proof.steps {
        crate::journey::validate_stable_id("compiled Journey step", &step.step_id)?;
        if !step_ids.insert(step.step_id.as_str()) {
            bail!("compiled Journey repeats step '{}'", step.step_id);
        }
        match &step.human_decision {
            Some(gate) => {
                if step.operation_id != "human-decision"
                    || !step.argv.is_empty()
                    || !step.environment.is_empty()
                    || !step.arguments.is_empty()
                    || !step.captures.is_empty()
                    || !step.assertions.is_empty()
                    || !step.redact.is_empty()
                    || !step.read_only
                {
                    bail!(
                        "compiled human decision step '{}' must not contain a CLI operation, arguments, captures, assertions, or redactions",
                        step.step_id
                    );
                }
                let source = HumanDecisionSource {
                    operation_id: gate.source_operation_id.clone(),
                    pointer: gate.pointer.clone(),
                };
                source.validate()?;
                if !prior_operations.contains(gate.source_operation_id.as_str()) {
                    bail!(
                        "compiled human decision step '{}' references non-prior operation '{}'",
                        step.step_id,
                        gate.source_operation_id
                    );
                }
            }
            None => {
                crate::journey::validate_stable_id(
                    "compiled Journey operation",
                    &step.operation_id,
                )?;
                if step.argv.is_empty() || step.argv.iter().any(String::is_empty) {
                    bail!(
                        "compiled Journey operation '{}' has empty argv",
                        step.operation_id
                    );
                }
                validate_compiled_environment(
                    &format!("compiled Journey operation '{}'", step.operation_id),
                    &step.environment,
                )?;
                if !operation_ids.insert(step.operation_id.as_str()) {
                    bail!(
                        "compiled Journey repeats primary operation '{}'",
                        step.operation_id
                    );
                }
                semantic_assertions += step.assertions.len();
                prior_operations.insert(step.operation_id.as_str());
            }
        }
    }
    if semantic_assertions == 0 {
        bail!("compiled Journey has no typed output assertion");
    }
    Ok(())
}

fn validate_compiled_environment(label: &str, environment: &[String]) -> Result<()> {
    let mut previous: Option<&str> = None;
    for name in environment {
        crate::journey::validate_process_environment_name(name)
            .with_context(|| format!("{label} has invalid environment declaration"))?;
        if previous.is_some_and(|prior| prior >= name.as_str()) {
            bail!("{label} environment names must be unique and canonically ordered");
        }
        previous = Some(name);
    }
    Ok(())
}

fn validate_compiled_runtime_sources(proof: &CompiledJourneyProof) -> Result<()> {
    let input_ids: BTreeSet<&str> = proof
        .profile_shape
        .input_ids
        .iter()
        .map(String::as_str)
        .collect();
    if let Some(setup) = &proof.setup {
        for operation in &setup.operations {
            for (index, token) in operation.argv.iter().enumerate() {
                if let Some(source) = crate::journey::argv_token_source(token)? {
                    validate_compiled_scalar_source(source, &input_ids, &BTreeMap::new(), false)
                        .with_context(|| {
                            format!(
                                "compiled setup operation '{}' argv token #{}",
                                operation.operation_id,
                                index + 1
                            )
                        })?;
                }
            }
        }
    }
    let mut prior_outputs = BTreeMap::new();
    for step in &proof.steps {
        if let Some(actions) = proof
            .setup
            .as_ref()
            .and_then(|setup| setup.before_steps.get(&step.step_id))
        {
            for action in actions {
                if let Some(template) = &action.template {
                    for source in crate::journey::template_references(template)? {
                        validate_compiled_scalar_source(source, &input_ids, &prior_outputs, true)?;
                    }
                }
            }
        }
        for (index, token) in step.argv.iter().enumerate() {
            if let Some(source) = crate::journey::argv_token_source(token)? {
                validate_compiled_scalar_source(source, &input_ids, &prior_outputs, false)
                    .with_context(|| {
                        format!("compiled step '{}' argv token #{}", step.step_id, index + 1)
                    })?;
            }
        }
        for capture in &step.captures {
            prior_outputs.insert(
                format!("steps.{}.outputs.{}", step.step_id, capture.id),
                (capture.value_type, capture.redact),
            );
        }
    }
    Ok(())
}

fn validate_compiled_scalar_source(
    source: &str,
    input_ids: &BTreeSet<&str>,
    prior_outputs: &BTreeMap<String, (ValueType, bool)>,
    allow_run_id: bool,
) -> Result<()> {
    match crate::journey::parse_runtime_source(source)? {
        RuntimeSource::RunId if allow_run_id => Ok(()),
        RuntimeSource::RunId => bail!("run.id cannot replace an argv token"),
        RuntimeSource::Input(id) if input_ids.contains(id) => Ok(()),
        RuntimeSource::Input(id) => bail!("unknown Journey input '{id}'"),
        RuntimeSource::StepOutput { .. } => {
            let Some((value_type, redact)) = prior_outputs.get(source) else {
                bail!("'{source}' is not available from an earlier step");
            };
            if *redact {
                bail!("redacted output '{source}' cannot enter argv or file content");
            }
            if !value_type.is_scalar() {
                bail!("output '{source}' is not scalar");
            }
            Ok(())
        }
    }
}

pub fn proof_path(root: &Path, journey_id: &str, profile: &str) -> Result<PathBuf> {
    crate::journey::validate_stable_id("journey", journey_id)?;
    crate::journey::validate_stable_id("profile", profile)?;
    Ok(root
        .join(".loom")
        .join("compiled")
        .join("journeys")
        .join(journey_id)
        .join(format!("{profile}.proof.json")))
}

pub fn baseline_path(root: &Path, journey_id: &str, profile: &str) -> Result<PathBuf> {
    Ok(proof_path(root, journey_id, profile)?.with_file_name(format!("{profile}.baseline.json")))
}

pub fn write_proof(root: &Path, proof: &CompiledJourneyProof) -> Result<PathBuf> {
    let path = proof_path(root, &proof.journey_id, &proof.profile)?;
    atomic_write(&path, &canonical_bytes(proof)?)?;
    Ok(path)
}

pub fn cache_matches(root: &Path, proof: &CompiledJourneyProof) -> Result<bool> {
    let path = proof_path(root, &proof.journey_id, &proof.profile)?;
    let Ok(actual) = std::fs::read(&path) else {
        return Ok(false);
    };
    Ok(actual == canonical_bytes(proof)?)
}

pub fn write_baseline(root: &Path, report: &RuntimeReport) -> Result<PathBuf> {
    if report.status != RuntimeStatus::Passed {
        bail!("only a passing Journey observation can be frozen");
    }
    let baseline = JourneyBaseline {
        schema: BASELINE_SCHEMA.into(),
        compiler_version: JOURNEY_COMPILER_VERSION.into(),
        journey_id: report.journey_id.clone(),
        journey_hash: report.journey_hash.clone(),
        surface_hash: report.surface_hash.clone(),
        profile: report.profile.clone(),
        report: report.clone(),
    };
    let path = baseline_path(root, &report.journey_id, &report.profile)?;
    let mut bytes = serde_json::to_vec_pretty(&canonicalize(serde_json::to_value(baseline)?))?;
    bytes.push(b'\n');
    atomic_write(&path, &bytes)?;
    Ok(path)
}

pub fn baseline_current(root: &Path, proof: &CompiledJourneyProof) -> Result<Option<bool>> {
    let path = baseline_path(root, &proof.journey_id, &proof.profile)?;
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(None);
    };
    let Ok(baseline) = serde_json::from_slice::<JourneyBaseline>(&bytes) else {
        return Ok(Some(false));
    };
    Ok(Some(
        baseline.schema == BASELINE_SCHEMA
            && baseline.compiler_version == JOURNEY_COMPILER_VERSION
            && baseline.journey_id == proof.journey_id
            && baseline.profile == proof.profile
            && baseline.journey_hash == proof.journey_hash
            && baseline.surface_hash == proof.surface_hash,
    ))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("artifact path '{}' has no parent", path.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("journey"),
        std::process::id()
    ));
    std::fs::write(&temporary, bytes)
        .with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("installing {}", path.display()))?;
    Ok(())
}

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

/// Execute a compiled Journey and return the sealed observation settlement
/// requires. The only way to obtain [`JourneyObservation`] is to actually run
/// this compiler-owned runtime (or the interactive/resume path).
pub fn execute_observed(
    root: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    overrides: &BTreeMap<String, Value>,
) -> JourneyObservation {
    if proof.steps.iter().any(|step| step.human_decision.is_some()) {
        return JourneyObservation::from_executed(
            proof,
            blocked_runtime_report(
                proof,
                "compiled Journey requires host-mediated execution; use the interactive runtime",
            ),
        );
    }
    match execute_interactive(root, spec, proof, overrides) {
        ExecutionOutcome::Completed { observation, .. } => *observation,
        ExecutionOutcome::Pending(_) => JourneyObservation::from_executed(
            proof,
            blocked_runtime_report(
                proof,
                "compiled Journey unexpectedly reached a human decision",
            ),
        ),
    }
}

/// Execute without ever manufacturing a human answer. A gate returns a
/// structured pending capsule; only a later one-shot resume may continue it.
pub fn execute_interactive(
    root: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    overrides: &BTreeMap<String, Value>,
) -> ExecutionOutcome {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing Journey repository root {}", root.display()));
    let outcome = root.and_then(|root| execute_fresh(&root, spec, proof, overrides));
    match outcome {
        Ok(outcome) => outcome,
        Err(error) => blocked_outcome(proof, error.to_string()),
    }
}

fn blocked_runtime_report(
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
) -> Result<ExecutionOutcome> {
    proof.validate()?;
    if proof.journey_id != spec.id || proof.journey_hash != spec.semantic_hash()? {
        bail!("compiled Journey does not match the current authored source");
    }
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
                &inputs,
                &captures,
                &run_id,
                &mut secrets,
                &label,
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
                            },
                        ),
                        Vec::new(),
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
                        },
                    ),
                    Vec::new(),
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
        human_decisions: Vec::new(),
    };
    run_steps(root, spec, proof, temp, isolated, 0, active)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActiveRun {
    run_id: String,
    inputs: BTreeMap<String, Value>,
    secrets: Vec<String>,
    execution_env: BTreeMap<String, String>,
    resolved_host_env: BTreeMap<String, String>,
    setup_reports: Vec<SetupReport>,
    file_transition_reports: Vec<FileTransitionReport>,
    reports: Vec<StepReport>,
    captures: BTreeMap<String, Value>,
    redacted_captures: BTreeSet<String>,
    assertions_passed: usize,
    #[serde(default)]
    passed_assertions: Vec<PassedAssertion>,
    human_decisions: Vec<Value>,
}

fn run_steps(
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
            &active.inputs,
            &active.captures,
            &active.run_id,
            &mut active.secrets,
            &label,
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
            }
        }
        for capture in &step.captures {
            let Some(value) = output.pointer(&capture.pointer).cloned() else {
                step_failed += 1;
                continue;
            };
            if !capture.value_type.accepts(&value) {
                step_failed += 1;
                continue;
            }
            let capture_key = format!("steps.{}.outputs.{}", step.step_id, capture.id);
            if capture.redact {
                if let Some(secret) = scalar_text(&value) {
                    active.secrets.push(secret);
                }
                active.redacted_captures.insert(capture_key.clone());
            }
            active.captures.insert(capture_key, value);
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
    active: ActiveRun,
) -> ExecutionOutcome {
    let captures = redact_capture_map(active.captures, &active.redacted_captures, &active.secrets);
    complete_outcome(
        proof,
        RuntimeReport {
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
        },
        active.human_decisions,
    )
}

const CONTINUATION_RUNTIME_SCHEMA: &str = "loom.journey-runtime-continuation/v1";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinuationState {
    schema: String,
    live_root: PathBuf,
    spec: JourneySpec,
    proof: CompiledJourneyProof,
    gate_binding: crate::journey_gate::GateBinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_subject: Option<CurrentSubjectAnchor>,
    step_index: usize,
    active: ActiveRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentSubjectAnchor {
    kind: String,
    id: String,
    name: String,
    description: String,
    hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservedHumanPrompt {
    subject: crate::journey_gate::GateSubject,
    question: String,
    recommendation: String,
    options: Vec<crate::journey_gate::HumanOption>,
}

fn normalize_human_prompt(
    observed: &Value,
) -> Result<(
    crate::journey_gate::GateSubject,
    crate::journey_gate::HumanPrompt,
    bool,
    Option<CurrentSubjectAnchor>,
)> {
    if let Ok(prompt) = serde_json::from_value::<ObservedHumanPrompt>(observed.clone()) {
        let prompt = crate::journey_gate::HumanPrompt::new(
            prompt.question,
            prompt.recommendation,
            prompt.options,
        )?;
        return Ok((prompt_subject(observed)?, prompt, false, None));
    }

    // Native Ratify work packets already carry the complete human-facing
    // contract. Normalize that read-only projection without exposing its
    // write-back commands or inferring a decision.
    let target = observed
        .get("target")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("Ratify work item has no target object"))?;
    let kind = required_string(target.get("kind"), "Ratify target kind")?;
    let id = required_string(target.get("id"), "Ratify target id")?;
    let name = required_string(target.get("name"), "Ratify target name")?;
    let reason = required_string(observed.get("reason"), "Ratify reason")?;
    let (criterion, current_subject) = ratify_target_criterion(observed, kind, id, name)?;
    let gate = observed
        .pointer("/prompt_contract/human_gate")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("Ratify work item has no prompt_contract.human_gate object"))?;
    let question = required_string(gate.get("question"), "Ratify gate question")?;
    let recommendation = required_string(gate.get("recommendation"), "Ratify gate recommendation")?;
    required_string(
        gate.get("after_answer"),
        "Ratify gate after_answer guidance",
    )?;
    let options = gate
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Ratify gate options must be an array"))?;
    if options.len() != 3 {
        bail!("Ratify gate must expose exactly ratify, reject, and revise options");
    }
    let expected = ["ratify", "reject", "revise"];
    let mut normalized = Vec::with_capacity(3);
    for (option, expected_id) in options.iter().zip(expected) {
        let option = option
            .as_object()
            .ok_or_else(|| anyhow!("Ratify gate option must be an object"))?;
        let id = required_string(option.get("id"), "Ratify gate option id")?;
        if id != expected_id {
            bail!("Ratify gate option order must be ratify, reject, revise");
        }
        let label = required_string(option.get("label"), "Ratify gate option label")?;
        let description =
            required_string(option.get("description"), "Ratify gate option description")?;
        required_string(option.get("write_back"), "Ratify gate write_back")?;
        normalized.push(crate::journey_gate::HumanOption::new(
            id,
            label,
            description,
            expected_id == "revise",
        ));
    }
    let prompt = crate::journey_gate::HumanPrompt::new(
        question,
        format!(
            "{recommendation}\n\nCurrent criterion: {criterion}\nCurrent drift evidence: {reason}"
        ),
        normalized,
    )?;
    let canonical = serde_json::to_string(&canonicalize(observed.clone()))?;
    Ok((
        crate::journey_gate::GateSubject {
            kind: kind.to_string(),
            id: id.to_string(),
            hash: crate::artifact::fingerprint(&canonical),
        },
        prompt,
        true,
        Some(current_subject),
    ))
}

fn ratify_target_criterion<'a>(
    observed: &'a Value,
    kind: &str,
    id: &str,
    name: &str,
) -> Result<(&'a str, CurrentSubjectAnchor)> {
    let linked = observed
        .pointer("/context/linked_entities")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Ratify work item has no context.linked_entities array"))?;
    let mut targets = linked.iter().filter(|entity| {
        entity.get("role").and_then(Value::as_str) == Some("target")
            && entity.get("kind").and_then(Value::as_str) == Some(kind)
            && entity.get("id").and_then(Value::as_str) == Some(id)
            && entity.get("name").and_then(Value::as_str) == Some(name)
    });
    let target = targets
        .next()
        .ok_or_else(|| anyhow!("Ratify context has no exact linked target criterion"))?;
    if targets.next().is_some() {
        bail!("Ratify context repeats the exact linked target criterion");
    }
    let description = required_string(target.get("description"), "Ratify linked target criterion")?;
    let canonical = serde_json::to_string(&json!({
        "kind": kind,
        "id": id,
        "name": name,
        "description": description,
    }))?;
    Ok((
        description,
        CurrentSubjectAnchor {
            kind: kind.to_string(),
            id: id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            hash: crate::artifact::fingerprint(&canonical),
        },
    ))
}

fn prompt_subject(observed: &Value) -> Result<crate::journey_gate::GateSubject> {
    let prompt: ObservedHumanPrompt = serde_json::from_value(observed.clone())?;
    Ok(prompt.subject)
}

fn required_string<'a>(value: Option<&'a Value>, label: &str) -> Result<&'a str> {
    let value = value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("{label} must be a substantive string"))?;
    if crate::model::is_placeholder(value) {
        bail!("{label} must not be a placeholder");
    }
    Ok(value)
}

fn scrub_ratify_control_fields(observed: &mut Value) {
    let Some(gate) = observed
        .pointer_mut("/prompt_contract/human_gate")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    gate.remove("after_answer");
    if let Some(options) = gate.get_mut("options").and_then(Value::as_array_mut) {
        for option in options {
            if let Some(option) = option.as_object_mut() {
                option.remove("write_back");
            }
        }
    }
}

struct GatePoint<'a> {
    step_index: usize,
    step: &'a CompiledStep,
    gate: &'a CompiledHumanDecision,
}

fn suspend_human_decision(
    root: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    temp: TemporaryRoot,
    point: GatePoint<'_>,
    mut active: ActiveRun,
) -> Result<ExecutionOutcome> {
    if proof.setup.is_none() {
        bail!("human decision continuation requires a local_snapshot workspace");
    }
    if !active.secrets.is_empty() || !active.redacted_captures.is_empty() {
        bail!(
            "human decision continuation cannot suspend a secret-bearing runtime; remove secret inputs or redacted captures from the gated profile"
        );
    }
    if !active.human_decisions.is_empty() {
        bail!(
            "human decision continuation cannot persist an earlier human answer; split sequential decisions into separate Journey runs"
        );
    }
    let source_index = active
        .reports
        .iter()
        .rposition(|report| report.operation_id == point.gate.source_operation_id)
        .ok_or_else(|| {
            anyhow!(
                "human decision step '{}' has no observed prior operation '{}'",
                point.step.step_id,
                point.gate.source_operation_id
            )
        })?;
    let observed = active.reports[source_index]
        .output
        .pointer(&point.gate.pointer)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "human decision step '{}' source pointer '{}' is absent from operation '{}'",
                point.step.step_id,
                point.gate.pointer,
                point.gate.source_operation_id
            )
        })?;
    let (subject, prompt, scrub_ratify_controls, current_subject) =
        normalize_human_prompt(&observed).with_context(|| {
            format!(
                "human decision step '{}' source is not a structured prompt",
                point.step.step_id
            )
        })?;
    if scrub_ratify_controls {
        let selected = active.reports[source_index]
            .output
            .pointer_mut(&point.gate.pointer)
            .expect("gate source pointer was observed above");
        scrub_ratify_control_fields(selected);
    }
    let binding = crate::journey_gate::GateBinding {
        journey_id: proof.journey_id.clone(),
        profile: proof.profile.clone(),
        journey_hash: proof.journey_hash.clone(),
        surface_hash: proof.surface_hash.clone(),
        step_id: point.step.step_id.clone(),
        step_index: point.step_index,
        subject,
        prompt_hash: prompt.digest()?,
    };
    let store = capsule_store(root)?;
    let issued = store.issue(binding.clone(), prompt)?;
    let state = ContinuationState {
        schema: CONTINUATION_RUNTIME_SCHEMA.into(),
        live_root: root
            .canonicalize()
            .with_context(|| format!("canonicalizing live Journey root {}", root.display()))?,
        spec: spec.clone(),
        proof: proof.clone(),
        gate_binding: binding,
        current_subject,
        step_index: point.step_index,
        active,
    };
    let installed = (|| -> Result<()> {
        temp.persist_to(&issued.paths.workspace)?;
        write_new_continuation(&issued.paths.runtime_state, &state)
    })();
    if let Err(error) = installed {
        let _ = std::fs::remove_dir_all(&issued.paths.directory);
        return Err(error);
    }
    Ok(ExecutionOutcome::Pending(issued.pending))
}

pub fn pending_continuation(token: &str) -> Result<PendingContinuation> {
    let store = capsule_store_without_graph()?;
    let state = read_continuation(&pending_runtime_state_path(&store, token)?)?;
    state.validate()?;
    Ok(PendingContinuation {
        binding: state.gate_binding,
    })
}

pub fn resume_interactive(
    root: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    token: &str,
    answer: crate::journey_gate::ResumeAnswer,
    executor: &str,
) -> Result<ExecutionOutcome> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing Journey repository root {}", root.display()))?;
    proof.validate()?;
    let store = capsule_store(&root)?;
    let pending_paths = store.inspect_pending(token)?;
    let pending = read_continuation(&pending_paths.runtime_state)?;
    pending.validate()?;
    validate_current_continuation(&root, &pending_paths.workspace, spec, proof, &pending)?;

    let claimed = store.claim(token, &pending.gate_binding, answer, executor)?;
    let resumed = (|| -> Result<ExecutionOutcome> {
        let claimed_paths = store.inspect_claimed(token)?;
        let mut state = read_continuation(&claimed_paths.runtime_state)?;
        state.validate()?;
        validate_current_continuation(&root, &claimed_paths.workspace, spec, proof, &state)?;
        if claimed.receipt.binding != state.gate_binding {
            bail!("claimed human decision does not match its runtime continuation");
        }
        let step = proof
            .steps
            .get(state.step_index)
            .ok_or_else(|| anyhow!("human decision continuation step index is no longer valid"))?;
        if step.human_decision.is_none() || step.step_id != state.gate_binding.step_id {
            bail!("human decision continuation no longer names the compiled gate step");
        }
        let receipt = serde_json::to_value(&claimed.receipt)?;
        state.active.reports.push(StepReport {
            step_id: step.step_id.clone(),
            operation_id: "human-decision".into(),
            argv: Vec::new(),
            exit_code: 0,
            output: receipt.clone(),
            assertions_passed: 1,
            assertions_failed: 0,
        });
        state.active.assertions_passed += 1;
        state.active.human_decisions.push(receipt);
        let workspace = TemporaryRoot::adopt(claimed.paths.workspace.clone())?;
        run_steps(
            &root,
            spec,
            proof,
            workspace,
            true,
            state.step_index + 1,
            state.active,
        )
    })();
    std::fs::remove_dir_all(&claimed.paths.directory).with_context(|| {
        format!(
            "destroying claimed Journey continuation {}",
            claimed.paths.directory.display()
        )
    })?;
    resumed
}

impl ContinuationState {
    fn validate(&self) -> Result<()> {
        if self.schema != CONTINUATION_RUNTIME_SCHEMA {
            bail!("unsupported Journey runtime continuation schema");
        }
        self.proof.validate()?;
        if self.proof.journey_id != self.spec.id
            || self.proof.journey_hash != self.spec.semantic_hash()?
        {
            bail!("Journey runtime continuation has mismatched authored semantics");
        }
        let step = self
            .proof
            .steps
            .get(self.step_index)
            .ok_or_else(|| anyhow!("Journey runtime continuation step index is invalid"))?;
        if step.human_decision.is_none()
            || step.step_id != self.gate_binding.step_id
            || self.gate_binding.journey_id != self.proof.journey_id
            || self.gate_binding.profile != self.proof.profile
            || self.gate_binding.journey_hash != self.proof.journey_hash
            || self.gate_binding.surface_hash != self.proof.surface_hash
            || self.gate_binding.step_index != self.step_index
        {
            bail!("Journey runtime continuation binding is inconsistent");
        }
        if let Some(subject) = &self.current_subject {
            if subject.kind != self.gate_binding.subject.kind
                || subject.id != self.gate_binding.subject.id
                || subject.hash.len() != 16
                || !subject.hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                bail!("Journey runtime continuation current-subject anchor is inconsistent");
            }
        }
        self.gate_binding.validate()
    }
}

fn validate_current_continuation(
    root: &Path,
    workspace: &Path,
    spec: &JourneySpec,
    proof: &CompiledJourneyProof,
    state: &ContinuationState,
) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing Journey root {}", root.display()))?;
    if root != state.live_root {
        bail!("Journey gate resume token belongs to a different graph root");
    }
    if spec.semantic_hash()? != state.spec.semantic_hash()?
        || canonical_bytes(proof)? != canonical_bytes(&state.proof)?
    {
        bail!("Journey gate resume token is stale for the current compiled projection");
    }
    if let Some(subject) = &state.current_subject {
        validate_current_subject(workspace, subject)?;
    }
    Ok(())
}

fn validate_current_subject(root: &Path, subject: &CurrentSubjectAnchor) -> Result<()> {
    let store = crate::store::Store::open_read(root)?;
    let node = store
        .get_node(&subject.id)?
        .ok_or_else(|| anyhow!("Journey gate current subject '{}' is missing", subject.id))?;
    if node.node_type.as_str() != subject.kind
        || node.name != subject.name
        || node.description != subject.description
    {
        bail!("Journey gate resume token is stale for the current subject");
    }
    let canonical = serde_json::to_string(&json!({
        "kind": node.node_type.as_str(),
        "id": node.id,
        "name": node.name,
        "description": node.description,
    }))?;
    if crate::artifact::fingerprint(&canonical) != subject.hash {
        bail!("Journey gate resume token is stale for the current subject");
    }
    Ok(())
}

fn capsule_store(root: &Path) -> Result<crate::journey_gate::CapsuleStore> {
    let store = capsule_store_without_graph()?;
    let live = root
        .canonicalize()
        .with_context(|| format!("canonicalizing Journey root {}", root.display()))?;
    if store.root() == live || store.root().starts_with(&live) {
        bail!("Journey continuation storage must be outside the live graph root");
    }
    Ok(store)
}

fn capsule_store_without_graph() -> Result<crate::journey_gate::CapsuleStore> {
    crate::journey_gate::CapsuleStore::new(
        std::env::temp_dir().join("loom-journey-runtime-continuations-v1"),
    )
}

fn pending_runtime_state_path(
    store: &crate::journey_gate::CapsuleStore,
    token: &str,
) -> Result<PathBuf> {
    let digest = crate::journey_gate::digest_token(token)?;
    Ok(store
        .root()
        .join("pending")
        .join(digest)
        .join("runtime-state.json"))
}

fn read_continuation(path: &Path) -> Result<ContinuationState> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("opening Journey runtime continuation {}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("Journey runtime continuation is not a confined regular file");
    }
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes).context("decoding Journey runtime continuation")
}

fn write_new_continuation(path: &Path, state: &ContinuationState) -> Result<()> {
    let bytes = serde_json::to_vec(state)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("creating Journey runtime continuation {}", path.display()))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

struct RuntimeProgress {
    setup: Vec<SetupReport>,
    file_transitions: Vec<FileTransitionReport>,
    steps: Vec<StepReport>,
    captures: BTreeMap<String, Value>,
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
    }
}

struct RuntimeTemplateSources<'a> {
    spec: &'a JourneySpec,
    inputs: &'a BTreeMap<String, Value>,
    captures: &'a BTreeMap<String, Value>,
    redacted_captures: &'a BTreeSet<String>,
    run_id: &'a str,
}

struct TemporalOutcome {
    report: FileTransitionReport,
    detail: Option<String>,
}

fn apply_temporal_file_action(
    live_root: &Path,
    snapshot_root: &Path,
    step_id: &str,
    action: &SurfaceFileAction,
    sources: &RuntimeTemplateSources<'_>,
) -> Result<TemporalOutcome> {
    let live_root = live_root
        .canonicalize()
        .with_context(|| format!("canonicalizing live repository {}", live_root.display()))?;
    let snapshot_root = snapshot_root.canonicalize().with_context(|| {
        format!(
            "canonicalizing Journey snapshot {}",
            snapshot_root.display()
        )
    })?;
    if snapshot_root == live_root {
        bail!("temporal file actions refuse the live repository");
    }
    let snapshot = crate::store::Store::open_read(&snapshot_root)?;
    let path = action.resolve_for_store(&snapshot)?;
    drop(snapshot);
    let live_path = live_root.join(&action.path);
    if live_path
        .canonicalize()
        .ok()
        .is_some_and(|live_path| path.canonicalize().ok().as_ref() == Some(&live_path))
    {
        bail!(
            "temporal file action '{}' resolved to the live repository",
            action.path
        );
    }

    let before = std::fs::read_to_string(&path)
        .with_context(|| format!("reading temporal file '{}'", action.path))?;
    let observed_before_hash = crate::artifact::fingerprint(&before);
    if observed_before_hash != action.expected_hash {
        let report = FileTransitionReport {
            step_id: step_id.to_string(),
            path: action.path.clone(),
            expected_hash: action.expected_hash.clone(),
            observed_before_hash: observed_before_hash.clone(),
            observed_after_hash: observed_before_hash.clone(),
            changed: false,
            applied: false,
        };
        return Ok(TemporalOutcome {
            report,
            detail: Some(format!(
                "before_steps.{step_id} path '{}' expected prior hash '{}' but observed '{}'",
                action.path, action.expected_hash, observed_before_hash
            )),
        });
    }

    let replacement = match (&action.content, &action.template) {
        (Some(content), None) => content.clone(),
        (None, Some(template)) => render_temporal_template(template, sources)?,
        _ => {
            action.validate()?;
            unreachable!("SurfaceFileAction::validate accepts exactly one replacement")
        }
    };
    atomic_replace_temporal_file(&path, replacement.as_bytes())?;
    let observed_after = std::fs::read_to_string(&path)
        .with_context(|| format!("reading replaced temporal file '{}'", action.path))?;
    let observed_after_hash = crate::artifact::fingerprint(&observed_after);
    let expected_after_hash = crate::artifact::fingerprint(&replacement);
    if observed_after_hash != expected_after_hash {
        bail!(
            "temporal file action '{}' did not install the exact replacement bytes",
            action.path
        );
    }
    Ok(TemporalOutcome {
        report: FileTransitionReport {
            step_id: step_id.to_string(),
            path: action.path.clone(),
            expected_hash: action.expected_hash.clone(),
            observed_before_hash: observed_before_hash.clone(),
            observed_after_hash: observed_after_hash.clone(),
            changed: observed_before_hash != observed_after_hash,
            applied: true,
        },
        detail: None,
    })
}

fn render_temporal_template(
    template: &str,
    sources: &RuntimeTemplateSources<'_>,
) -> Result<String> {
    crate::journey::template_references(template)?;
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        rendered.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| anyhow!("temporal template has an unterminated reference"))?;
        let source = after[..end].trim();
        match crate::journey::parse_runtime_source(source)? {
            RuntimeSource::Input(id) if sources.spec.inputs.get(id).is_some_and(|v| v.secret) => {
                bail!("secret input '{id}' cannot enter temporal file content")
            }
            RuntimeSource::StepOutput { .. } if sources.redacted_captures.contains(source) => {
                bail!("redacted output '{source}' cannot enter temporal file content")
            }
            _ => {}
        }
        let value = source_value(source, sources.inputs, sources.captures, sources.run_id)
            .ok_or_else(|| anyhow!("temporal template source '{source}' is unavailable"))?;
        let value = runtime_scalar_text(value.as_ref())
            .ok_or_else(|| anyhow!("temporal template source '{source}' is not scalar"))?;
        if value.contains('\0') {
            bail!("temporal template source '{source}' resolved a NUL byte");
        }
        rendered.push_str(&value);
        rest = &after[end + 2..];
    }
    rendered.push_str(rest);
    Ok(rendered)
}

fn atomic_replace_temporal_file(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("temporal file '{}' has no parent", path.display()))?;
    let permissions = std::fs::symlink_metadata(path)?.permissions();
    for sequence in 0..1000_u32 {
        let temporary = parent.join(format!(
            ".{}.loom-temporal-{}-{sequence}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file"),
            std::process::id()
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        let result = (|| -> Result<()> {
            file.write_all(content)?;
            file.sync_all()?;
            std::fs::set_permissions(&temporary, permissions.clone())?;
            drop(file);
            std::fs::rename(&temporary, path)
                .with_context(|| format!("installing temporal file {}", path.display()))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        return result;
    }
    bail!(
        "could not allocate a temporal sibling for '{}'",
        path.display()
    )
}

#[allow(clippy::too_many_arguments)]
fn run_json_operation(
    repository_root: &Path,
    cwd: &Path,
    graph_root: &Path,
    policy: &crate::candidate_surface_policy::SurfacePlan,
    operation_id: &str,
    confinement: crate::candidate_surface_policy::ActualConfinement,
    env: &BTreeMap<String, String>,
    resolved_host_env: &BTreeMap<String, String>,
    declared_environment: &[String],
    base_argv: &[String],
    arguments: &[OperationArgument],
    timeout_seconds: u64,
    inputs: &BTreeMap<String, Value>,
    captures: &BTreeMap<String, Value>,
    run_id: &str,
    secrets: &mut Vec<String>,
    label: &str,
) -> Result<(Vec<String>, i64, Value)> {
    let operation_env = operation_environment(env, resolved_host_env, declared_environment)?;
    let (argv, mut display_argv) =
        resolve_argv(base_argv, arguments, inputs, captures, run_id, secrets)?;
    let authorized = policy.authorize(operation_id, argv, confinement)?;
    if authorized.injects_graph() {
        display_argv.insert(1, graph_root.display().to_string());
        display_argv.insert(1, "--graph".into());
    }
    let observed = run_direct(
        repository_root,
        cwd,
        graph_root,
        &operation_env,
        authorized,
        Duration::from_secs(timeout_seconds),
    )
    .with_context(|| format!("{label} could not start"))?;
    if observed.timed_out {
        bail!("{label} exceeded the execution timeout");
    }
    let exit_code = i64::from(observed.status.code().unwrap_or(-1));
    if !observed.status.success() {
        bail!(
            "{}",
            failed_operation_detail(
                label,
                exit_code,
                &observed.stdout,
                &observed.stderr,
                secrets
            )
        );
    }
    let stdout = std::str::from_utf8(&observed.stdout)
        .with_context(|| format!("{label} stdout is not UTF-8 JSON"))?;
    let output = serde_json::from_str(stdout)
        .with_context(|| format!("{label} stdout is not one JSON value"))?;
    Ok((display_argv, exit_code, output))
}

fn failed_operation_detail(
    label: &str,
    exit_code: i64,
    stdout: &[u8],
    stderr: &[u8],
    secrets: &[String],
) -> String {
    let stdout = match serde_json::from_slice::<Value>(stdout) {
        Ok(mut structured) => {
            redact_json_secrets(&mut structured, secrets);
            serde_json::to_string_pretty(&structured)
                .unwrap_or_else(|_| redact_text(&String::from_utf8_lossy(stdout), secrets))
        }
        Err(_) => redact_text(&String::from_utf8_lossy(stdout), secrets),
    };
    let stderr = redact_text(&String::from_utf8_lossy(stderr), secrets);
    format!(
        "{label} exited {exit_code}\nstdout:\n{}\nstderr:\n{}",
        bounded_runtime_diagnostic(&stdout),
        bounded_runtime_diagnostic(&stderr),
    )
}

fn bounded_runtime_diagnostic(text: &str) -> String {
    if text.len() <= FAILURE_DIAGNOSTIC_BYTES {
        return text.trim().to_string();
    }
    let half = FAILURE_DIAGNOSTIC_BYTES / 2;
    let mut head = half;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - half;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n...[diagnostic output omitted]...\n{}",
        text[..head].trim_end(),
        text[tail..].trim_start()
    )
}

fn operation_environment(
    explicit: &BTreeMap<String, String>,
    resolved_host: &BTreeMap<String, String>,
    declared: &[String],
) -> Result<BTreeMap<String, String>> {
    let mut environment = explicit.clone();
    for name in declared {
        if environment.contains_key(name) {
            continue;
        }
        let value = resolved_host.get(name).ok_or_else(|| {
            anyhow!("declared operation environment variable '{name}' was not preflighted")
        })?;
        environment.insert(name.clone(), value.clone());
    }
    Ok(environment)
}

fn preflight_operation_environment(
    proof: &CompiledJourneyProof,
    explicit: &BTreeMap<String, String>,
    secrets: &mut Vec<String>,
) -> Result<BTreeMap<String, String>> {
    let mut declared = BTreeSet::new();
    if let Some(setup) = &proof.setup {
        for operation in &setup.operations {
            declared.extend(operation.environment.iter().cloned());
        }
    }
    for step in &proof.steps {
        declared.extend(step.environment.iter().cloned());
    }
    let mut resolved = BTreeMap::new();
    for name in declared {
        if explicit.contains_key(&name) {
            continue;
        }
        let value = std::env::var(&name).map_err(|error| match error {
            std::env::VarError::NotPresent => {
                anyhow!("declared operation environment variable '{name}' is missing")
            }
            std::env::VarError::NotUnicode(_) => {
                anyhow!("declared operation environment variable '{name}' is not valid UTF-8")
            }
        })?;
        secrets.push(value.clone());
        resolved.insert(name, value);
    }
    Ok(resolved)
}

fn profile_for<'a>(spec: &'a JourneySpec, id: &str) -> Result<&'a JourneyProfile> {
    spec.profiles
        .get(id)
        .ok_or_else(|| anyhow!("Journey '{}' has no profile '{id}'", spec.id))
}

fn resolve_inputs(
    spec: &JourneySpec,
    profile_id: &str,
    profile: &JourneyProfile,
    overrides: &BTreeMap<String, Value>,
    run_id: &str,
) -> Result<ResolvedInputs> {
    let mut values = BTreeMap::new();
    let mut secrets = Vec::new();
    let mut bound_env = BTreeMap::new();
    for (id, input) in &spec.inputs {
        if let Some(value) = &input.default {
            values.insert(id.clone(), value.clone());
        }
    }

    // Resolve environment bindings immediately, then templates as their input
    // dependencies become available. Cycles and unavailable references fail
    // closed instead of silently interpolating an empty string.
    let mut pending: BTreeSet<String> = profile.inputs.keys().cloned().collect();
    while !pending.is_empty() {
        let mut progressed = false;
        for id in pending.clone() {
            let input = spec
                .inputs
                .get(&id)
                .ok_or_else(|| anyhow!("profile binds unknown input '{id}'"))?;
            let binding = profile.inputs.get(&id).expect("pending key came from map");
            let resolved = if let Some(env) = &binding.env {
                let raw = std::env::var(env).with_context(|| {
                    format!(
                        "required environment variable '{}' for Journey input '{}' is not set",
                        env, id
                    )
                })?;
                bound_env.insert(env.clone(), raw.clone());
                if input.secret {
                    // Register the raw value before parsing so neither type
                    // errors nor later evidence can disclose it.
                    secrets.push(raw.clone());
                    Some(
                        crate::journey::parse_typed_text(&raw, input.value_type).map_err(|_| {
                            anyhow!(
                                "secret Journey input '{}' from environment '{}' has the wrong type",
                                id,
                                env
                            )
                        })?,
                    )
                } else {
                    Some(crate::journey::parse_typed_text(&raw, input.value_type)?)
                }
            } else if let Some(template) = &binding.template {
                render_profile_template(template, input.value_type, &values, run_id)?
            } else {
                None
            };
            if let Some(value) = resolved {
                if !input.value_type.accepts(&value) {
                    bail!(
                        "profile '{}' input '{}' resolved to the wrong type",
                        profile_id,
                        id
                    );
                }
                if input.secret {
                    let rendered = scalar_text(&value).unwrap_or_else(|| value.to_string());
                    secrets.push(rendered);
                }
                values.insert(id.clone(), value);
                pending.remove(&id);
                progressed = true;
            }
        }
        if !progressed {
            bail!(
                "profile '{}' has cyclic or unavailable input template references: {}",
                profile_id,
                pending.into_iter().collect::<Vec<_>>().join(", ")
            );
        }
    }
    for (id, value) in overrides {
        let input = spec
            .inputs
            .get(id)
            .ok_or_else(|| anyhow!("diagnose override names unknown input '{id}'"))?;
        if input.secret {
            bail!(
                "secret Journey input '{}' cannot be supplied as a literal diagnose override; use profiles.proof.inputs.{}.env",
                id,
                id
            );
        }
        if !input.value_type.accepts(value) {
            bail!("diagnose override '{}' has the wrong type", id);
        }
        values.insert(id.clone(), value.clone());
    }
    for (id, input) in &spec.inputs {
        if input.required && !values.contains_key(id) {
            bail!(
                "required Journey input '{}' has no profile/default value",
                id
            );
        }
    }
    Ok((values, secrets, bound_env))
}

fn render_profile_template(
    template: &str,
    value_type: crate::journey::ValueType,
    inputs: &BTreeMap<String, Value>,
    run_id: &str,
) -> Result<Option<Value>> {
    let references = crate::journey::template_references(template)?;
    let exact = references.len() == 1 && template.trim() == format!("{{{{ {} }}}}", references[0]);
    if exact {
        return match references[0] {
            "run.id" => Ok(Some(crate::journey::parse_typed_text(run_id, value_type)?)),
            reference if reference.starts_with("inputs.") => {
                let id = &reference["inputs.".len()..];
                Ok(inputs.get(id).cloned())
            }
            _ => Ok(None),
        };
    }

    let mut rendered = template.to_string();
    for reference in references {
        let replacement = match reference {
            "run.id" => run_id.to_string(),
            reference if reference.starts_with("inputs.") => {
                let id = &reference["inputs.".len()..];
                let Some(value) = inputs.get(id) else {
                    return Ok(None);
                };
                scalar_text(value).unwrap_or_else(|| value.to_string())
            }
            _ => return Ok(None),
        };
        rendered = rendered.replace(&format!("{{{{ {reference} }}}}"), &replacement);
        rendered = rendered.replace(&format!("{{{{{reference}}}}}"), &replacement);
    }
    Ok(Some(crate::journey::parse_typed_text(
        &rendered, value_type,
    )?))
}

fn resolve_argv(
    base_argv: &[String],
    arguments: &[OperationArgument],
    inputs: &BTreeMap<String, Value>,
    captures: &BTreeMap<String, Value>,
    run_id: &str,
    secrets: &mut Vec<String>,
) -> Result<(Vec<String>, Vec<String>)> {
    let mut argv = Vec::with_capacity(base_argv.len() + arguments.len() * 2 + 2);
    let mut display = Vec::with_capacity(argv.capacity());
    for (index, token) in base_argv.iter().enumerate() {
        let resolved = match crate::journey::argv_token_source(token)? {
            Some(source) => {
                if index == 0 {
                    bail!("executable argv token cannot be a runtime template");
                }
                let value = source_value(source, inputs, captures, run_id)
                    .ok_or_else(|| anyhow!("argv token source '{source}' is unavailable"))?;
                let rendered = runtime_scalar_text(value.as_ref())
                    .ok_or_else(|| anyhow!("argv token source '{source}' is not scalar"))?;
                if rendered.contains('\0') {
                    bail!("argv token source '{source}' resolved a NUL byte");
                }
                if secrets
                    .iter()
                    .any(|secret| !secret.is_empty() && secret == &rendered)
                {
                    bail!("argv token source '{source}' resolved protected secret material");
                }
                rendered
            }
            None => token.clone(),
        };
        argv.push(resolved.clone());
        display.push(resolved);
    }
    for argument in arguments {
        let default_source = format!("inputs.{}", argument.id);
        let value = source_value(
            argument.source.as_deref().unwrap_or(&default_source),
            inputs,
            captures,
            run_id,
        );
        let Some(value) = value else {
            if argument.required {
                bail!("required argument '{}' has no value", argument.id);
            }
            continue;
        };
        if !argument.value_type.accepts(value.as_ref()) {
            bail!("argument '{}' source has the wrong type", argument.id);
        }
        let rendered = scalar_text(value.as_ref()).unwrap_or_else(|| value.to_string());
        if let Some(flag) = &argument.flag {
            argv.push(flag.clone());
            display.push(flag.clone());
        }
        argv.push(rendered.clone());
        if argument.redact {
            secrets.push(rendered);
            display.push(REDACTED.into());
        } else {
            display.push(rendered);
        }
    }
    Ok((argv, display))
}

fn runtime_scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn runtime_run_id(journey_id: &str, profile: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{journey_id}.{profile}.{}.{nanos}", std::process::id())
}

fn source_value<'a>(
    source: &str,
    inputs: &'a BTreeMap<String, Value>,
    captures: &'a BTreeMap<String, Value>,
    run_id: &'a str,
) -> Option<std::borrow::Cow<'a, Value>> {
    match crate::journey::parse_runtime_source(source).ok()? {
        RuntimeSource::Input(id) => inputs.get(id).map(std::borrow::Cow::Borrowed),
        RuntimeSource::StepOutput { .. } => captures.get(source).map(std::borrow::Cow::Borrowed),
        RuntimeSource::RunId => Some(std::borrow::Cow::Owned(Value::String(run_id.to_string()))),
    }
}

fn assertion_holds(
    assertion: &OutputAssertion,
    output: &Value,
    inputs: &BTreeMap<String, Value>,
    captures: &BTreeMap<String, Value>,
    run_id: &str,
) -> bool {
    if let Some(expected) = assertion.exists_value() {
        return output.pointer(&assertion.pointer).is_some() == expected;
    }
    let Some(actual) = output.pointer(&assertion.pointer) else {
        return false;
    };
    if assertion
        .value_type
        .is_some_and(|value_type| !value_type.accepts(actual))
    {
        return false;
    }
    if let Some(expected) = &assertion.equals {
        if actual != expected {
            return false;
        }
    }
    if assertion
        .not_equals_value()
        .as_ref()
        .is_some_and(|unexpected| actual == unexpected)
    {
        return false;
    }
    if assertion
        .contains_value()
        .as_ref()
        .is_some_and(|expected| !value_contains(actual, expected))
    {
        return false;
    }
    if let Some(pattern) = assertion.matches_pattern() {
        let Some(actual) = actual.as_str() else {
            return false;
        };
        if !regex::Regex::new(&pattern).is_ok_and(|regex| regex.is_match(actual)) {
            return false;
        }
    }
    if let Some(source) = assertion.runtime_source() {
        if source_value(source, inputs, captures, run_id).as_deref() != Some(actual) {
            return false;
        }
    }
    true
}

fn value_contains(actual: &Value, expected: &Value) -> bool {
    match actual {
        Value::String(actual) => expected
            .as_str()
            .is_some_and(|expected| actual.contains(expected)),
        Value::Array(actual) => actual.iter().any(|value| value == expected),
        Value::Object(actual) => expected.as_object().is_some_and(|expected| {
            expected
                .iter()
                .all(|(key, value)| actual.get(key) == Some(value))
        }),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null => Some("null".into()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

fn redact_capture_map(
    mut captures: BTreeMap<String, Value>,
    redacted: &BTreeSet<String>,
    secrets: &[String],
) -> BTreeMap<String, Value> {
    for id in redacted {
        if let Some(value) = captures.get_mut(id) {
            *value = Value::String(REDACTED.into());
        }
    }
    for value in captures.values_mut() {
        redact_json_secrets(value, secrets);
    }
    captures
}

fn redact_json_secrets(value: &mut Value, secrets: &[String]) {
    match value {
        Value::String(text) => *text = redact_text(text, secrets),
        Value::Array(values) => {
            for value in values {
                redact_json_secrets(value, secrets);
            }
        }
        Value::Object(values) => {
            let original = std::mem::take(values);
            let mut preserved = serde_json::Map::new();
            let mut renamed = Vec::new();
            for (key, mut value) in original {
                redact_json_secrets(&mut value, secrets);
                let redacted_key = redact_text(&key, secrets);
                if redacted_key == key {
                    preserved.insert(key, value);
                } else {
                    renamed.push((key, redacted_key, value));
                }
            }
            // Preserve every unrelated key first, then deterministically
            // allocate collision-safe names for redacted keys. The original
            // secret-bearing key orders equal redactions without being kept.
            renamed.sort_by(|left, right| left.0.cmp(&right.0));
            for (_, base, value) in renamed {
                let mut candidate = base.clone();
                let mut suffix = 2usize;
                while preserved.contains_key(&candidate) {
                    candidate = format!("{base}#{suffix}");
                    suffix += 1;
                }
                preserved.insert(candidate, value);
            }
            *values = preserved;
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn redact_pointer(value: &mut Value, pointer: &str) {
    if pointer.is_empty() {
        *value = Value::String(REDACTED.into());
        return;
    }
    let mut segments: Vec<String> = pointer
        .split('/')
        .skip(1)
        .map(|segment| segment.replace("~1", "/").replace("~0", "~"))
        .collect();
    let Some(last) = segments.pop() else {
        return;
    };
    let mut current = value;
    for segment in segments {
        current = match current {
            Value::Object(map) => match map.get_mut(&segment) {
                Some(value) => value,
                None => return,
            },
            Value::Array(values) => match segment
                .parse::<usize>()
                .ok()
                .and_then(|index| values.get_mut(index))
            {
                Some(value) => value,
                None => return,
            },
            _ => return,
        };
    }
    match current {
        Value::Object(map) => {
            if let Some(value) = map.get_mut(&last) {
                *value = Value::String(REDACTED.into());
            }
        }
        Value::Array(values) => {
            if let Some(value) = last
                .parse::<usize>()
                .ok()
                .and_then(|index| values.get_mut(index))
            {
                *value = Value::String(REDACTED.into());
            }
        }
        _ => {}
    }
}

fn redact_text(text: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(text.to_string(), |text, secret| {
            text.replace(secret, REDACTED)
        })
}

fn materialize_setup(root: &Path, setup: &TemporarySetup) -> Result<()> {
    for directory in &setup.directories {
        std::fs::create_dir_all(root.join(directory))
            .with_context(|| format!("creating temporary setup directory '{directory}'"))?;
    }
    for file in &setup.files {
        let path = root.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &file.content)
            .with_context(|| format!("writing temporary setup file '{}'", file.path))?;
    }
    Ok(())
}

struct TemporaryRoot(PathBuf);

impl TemporaryRoot {
    fn create(root: &Path) -> Result<Self> {
        let parent = root.join(".loom").join("tmp");
        std::fs::create_dir_all(&parent)?;
        for sequence in 0..1000_u32 {
            let path = parent.join(format!("journey-{}-{sequence}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        bail!("could not allocate a unique temporary Journey root")
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn persist_to(mut self, destination: &Path) -> Result<()> {
        if destination.exists() {
            bail!(
                "Journey continuation workspace '{}' already exists",
                destination.display()
            );
        }
        std::fs::rename(&self.0, destination).with_context(|| {
            format!(
                "persisting Journey workspace {} as {}",
                self.0.display(),
                destination.display()
            )
        })?;
        self.0 = PathBuf::new();
        Ok(())
    }

    fn adopt(path: PathBuf) -> Result<Self> {
        let metadata = std::fs::symlink_metadata(&path).with_context(|| {
            format!("opening Journey continuation workspace {}", path.display())
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            bail!("Journey continuation workspace is not a confined directory");
        }
        Ok(Self(path))
    }

    fn create_detached(live_root: &Path) -> Result<Self> {
        Self::create_detached_with_prefix(live_root, "loom-journey-git")
    }

    fn create_gate_detached(live_root: &Path) -> Result<Self> {
        Self::create_detached_with_prefix(live_root, "loom-journey-gate")
    }

    fn create_detached_with_prefix(live_root: &Path, prefix: &str) -> Result<Self> {
        let parent = std::env::temp_dir();
        std::fs::create_dir_all(&parent)?;
        let canonical_parent = parent
            .canonicalize()
            .with_context(|| format!("canonicalizing system temp root {}", parent.display()))?;
        let canonical_live = live_root
            .canonicalize()
            .with_context(|| format!("canonicalizing live graph root {}", live_root.display()))?;
        if canonical_parent == canonical_live || canonical_parent.starts_with(&canonical_live) {
            bail!("system temp root must be outside the live repository");
        }
        for sequence in 0..1000_u32 {
            let path = canonical_parent.join(format!("{prefix}-{}-{sequence}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        bail!("could not allocate a detached temporary Journey root")
    }
}

impl Drop for TemporaryRoot {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

struct DirectObservation {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
}

fn run_direct(
    repository_root: &Path,
    cwd: &Path,
    graph_root: &Path,
    env: &BTreeMap<String, String>,
    invocation: crate::candidate_surface_policy::AuthorizedInvocation,
    timeout: Duration,
) -> std::io::Result<DirectObservation> {
    let argv = invocation.into_graph_argv(graph_root);
    let executable = resolve_executable(repository_root, &argv[0]);
    let mut command = Command::new(executable);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .env("LOOM_NON_INTERACTIVE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Preserve only the canonical executor-infrastructure allowlist needed to
    // resolve/spawn child tools. These names are distinct from authored
    // operation.environment declarations. CI, cloud credentials, tokens, and
    // arbitrary host variables remain absent.
    for &key in EXECUTOR_PLATFORM_ENVIRONMENT {
        if !env.contains_key(key) {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
    }
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().map(read_stream);
    let stderr = child.stderr.take().map(read_stream);
    let started = Instant::now();
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (status, false);
        }
        if started.elapsed() >= timeout {
            kill_process_group(&mut child);
            break (child.wait()?, true);
        }
        thread::sleep(Duration::from_millis(10));
    };
    Ok(DirectObservation {
        status,
        stdout: stdout
            .map(|reader| reader.join().unwrap_or_default())
            .unwrap_or_default(),
        stderr: stderr
            .map(|reader| reader.join().unwrap_or_default())
            .unwrap_or_default(),
        timed_out,
    })
}

fn resolve_executable(root: &Path, executable: &str) -> PathBuf {
    let candidate = Path::new(executable);
    if candidate.is_absolute() {
        return candidate.to_path_buf();
    }
    if executable.contains('/') {
        return root.join(candidate);
    }
    if executable == "loom" {
        if let Ok(current) = std::env::current_exe() {
            if current
                .file_stem()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "loom")
            {
                return current;
            }
        }
    }
    candidate.to_path_buf()
}

fn read_stream(mut stream: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut retained = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let room = STREAM_EXCERPT_BYTES.saturating_sub(retained.len());
                    retained.extend_from_slice(&buffer[..read.min(room)]);
                }
            }
        }
        retained
    })
}

fn kill_process_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        Value::Object(object) => {
            let sorted: BTreeMap<String, Value> = object
                .into_iter()
                .map(|(key, value)| (key, canonicalize(value)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        scalar => scalar,
    }
}

pub fn report_observation_json(report: &RuntimeReport) -> Result<Vec<u8>> {
    // This is structured evidence from checks Loom actually performed. The
    // proof-strength layer should consume RunRecord.assertions directly; it
    // must not depend on a synthetic test-runner sentence hidden in stdout.
    Ok(serde_json::to_vec(&json!({
        "journey": report.journey_id,
        "profile": report.profile,
        "status": report.status,
        "assertions_passed": report.assertions_passed,
        "assertions_failed": report.assertions_failed,
        "passed_assertions": report.passed_assertions,
    }))?)
}

pub fn parse_overrides(raw: &[String]) -> Result<BTreeMap<String, Value>> {
    let mut overrides = BTreeMap::new();
    for item in raw {
        let (key, encoded) = item
            .split_once('=')
            .ok_or_else(|| anyhow!("--input '{item}' must be KEY=JSON"))?;
        crate::journey::validate_stable_id("input", key)?;
        if overrides
            .insert(
                key.to_string(),
                serde_json::from_str(encoded)
                    .with_context(|| format!("parsing --input '{key}' as JSON"))?,
            )
            .is_some()
        {
            bail!("--input '{key}' was supplied more than once");
        }
    }
    Ok(overrides)
}
