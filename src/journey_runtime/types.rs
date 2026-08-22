use crate::journey::{
    OperationArgument, OutputAssertion, OutputCapture, SetupGraph, SurfaceFileAction,
    SurfaceGitSetup, DEFAULT_JOURNEY_TIMEOUT_SECONDS,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::observation::JourneyObservation;

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
pub(crate) const STREAM_EXCERPT_BYTES: usize = 512 * 1024;
pub(crate) const FAILURE_DIAGNOSTIC_BYTES: usize = 64 * 1024;
pub(crate) const REDACTED: &str = "[REDACTED]";
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
pub(crate) type ResolvedInputs = (
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
    /// Expected exit code; process liveness, not an assertion. Zero is the
    /// default rule. Older proofs omit the field and deserialize it as zero.
    #[serde(default, skip_serializing_if = "exit_code_is_zero")]
    pub expected_exit: u32,
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
    /// Expected exit code; process liveness, not an assertion. Zero is the
    /// default rule. Older proofs omit the field and deserialize it as zero.
    /// Human gates never execute, so they always carry zero.
    #[serde(default, skip_serializing_if = "exit_code_is_zero")]
    pub expected_exit: u32,
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

fn exit_code_is_zero(code: &u32) -> bool {
    *code == 0
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

/// One typed check that did NOT hold, named. The counterpart of
/// [`PassedAssertion`], and the same argument applies as for a sync ripple:
/// `assertions_failed: 1` is not actionable. A caller reading a failed run —
/// an operator, a release gate reporting why a candidate Journey refused — has
/// to know WHICH check moved before it can decide anything, and re-running the
/// Journey to find out is the expensive way to learn a name the runtime
/// already had.
///
/// `kind` distinguishes a typed output assertion from a capture that could not
/// be taken, because both increment the same counter and they are repaired in
/// different places.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailedAssertion {
    pub operation_id: String,
    /// The authored assertion id, or the capture id for a capture failure.
    pub assertion_id: String,
    pub pointer: String,
    pub kind: FailedCheckKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailedCheckKind {
    /// A typed `output.assertions` entry that did not hold.
    Assertion,
    /// A declared capture whose pointer resolved to nothing.
    CaptureMissing,
    /// A declared capture whose value did not match its declared type.
    CaptureType,
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
    /// The typed checks that did not hold, named. Empty — and omitted from the
    /// serialization — for any passing run, so a passing report is byte-identical
    /// to one produced before this field existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_assertions: Vec<FailedAssertion>,
}

#[derive(Debug)]
pub enum ExecutionOutcome {
    Completed {
        report: RuntimeReport,
        /// Sealed capability minted only by this runtime. External crates can
        /// read it from a completed execution and pass it to settlement, but
        /// cannot construct one from a caller-authored report — and unless it
        /// was minted through the Store-owned guarded entrypoint (never
        /// through these public execution APIs), settlement refuses it.
        observation: Box<JourneyObservation>,
        human_decisions: Vec<Value>,
    },
    Pending(crate::journey_gate::PendingHuman),
}

pub(crate) fn compiled_assertion_ids(proof: &CompiledJourneyProof) -> BTreeSet<(String, String)> {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingContinuation {
    pub binding: crate::journey_gate::GateBinding,
    /// Canonical repository root the paused run executed in. The resume
    /// entrypoint refuses a token presented at any other root.
    pub live_root: PathBuf,
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
