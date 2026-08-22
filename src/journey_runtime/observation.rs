use crate::journey::JOURNEY_COMPILER_VERSION;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::execute::blocked_runtime_report;
use super::types::{
    compiled_assertion_ids, CompiledJourneyProof, ExecutionOutcome, RuntimeReport, RuntimeStatus,
};

/// Where an observation stands relative to the trusted-settlement boundary.
///
/// The public execution APIs ([`execute`], [`execute_observed`],
/// [`execute_interactive`], [`resume_interactive`]) mint [`Untrusted`]
/// observations: ordinary reports, presentable and diagnosable, that
/// settlement refuses. Only the Store-owned guarded runtime entrypoint
/// ([`crate::journey::run_and_settle_compiled_validation`] and its
/// interactive/resume siblings) marks an observation [`Trusted`] — after
/// re-deriving the canonical proof, projection, executable boundary, and
/// execution-time covered hashes from the same store, under the same guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ObservationTrust {
    #[default]
    Untrusted,
    Trusted,
}

/// One executed operation's executable boundary: what actually ran.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutableBoundary {
    pub operation_id: String,
    /// argv[0] exactly as compiled into the proof (a literal path or a
    /// runtime template). Settling binds this to the compiled operation.
    pub declared: String,
    /// argv[0] exactly as resolved for the spawn.
    pub argv0: String,
    /// Absolute canonical path of the resolved executable, when one exists;
    /// otherwise the raw resolution.
    pub resolved: String,
    /// Content fingerprint of the resolved executable at execution time.
    /// Empty when the file could not be read.
    pub hash: String,
}

/// Execution-time anchors captured by the compiler-owned runtime: the covered
/// hashes in force immediately before execution, the canonical execution root,
/// and the resolved executable boundary for every spawned operation. Persisted
/// with the observation and (for interactive runs) inside the continuation
/// state; settlement never resamples evidence from these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExecutionAnchors {
    pub covered_hashes: BTreeMap<String, String>,
    pub execution_root: PathBuf,
    pub executed_boundary: Vec<ExecutableBoundary>,
}

impl ExecutionAnchors {
    /// The pre-execution covered hashes still match the files on disk under
    /// `root`. Used for the immediate post-execution recheck and the
    /// resume-time recheck, both under the harness guard.
    pub(crate) fn covered_still_match(&self, root: &Path) -> bool {
        self.covered_hashes.iter().all(|(file, hash)| {
            std::fs::read_to_string(root.join(file))
                .map(|content| &crate::artifact::fingerprint(&content) == hash)
                .unwrap_or(false)
        })
    }
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
    anchors: Option<ExecutionAnchors>,
    trust: ObservationTrust,
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

    pub(crate) fn anchors(&self) -> Option<&ExecutionAnchors> {
        self.anchors.as_ref()
    }

    /// Only the Store-owned guarded settlement entrypoint may flip this.
    pub(crate) fn is_trusted(&self) -> bool {
        self.trust == ObservationTrust::Trusted
    }

    pub(crate) fn mark_trusted(&mut self) {
        self.trust = ObservationTrust::Trusted;
    }

    pub(crate) fn from_executed(
        proof: &CompiledJourneyProof,
        mut report: RuntimeReport,
        anchors: Option<ExecutionAnchors>,
    ) -> Self {
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
            anchors,
            trust: ObservationTrust::Untrusted,
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

pub(crate) fn complete_outcome(
    proof: &CompiledJourneyProof,
    report: RuntimeReport,
    human_decisions: Vec<Value>,
    anchors: Option<ExecutionAnchors>,
) -> ExecutionOutcome {
    let observation = JourneyObservation::from_executed(proof, report.clone(), anchors);
    ExecutionOutcome::Completed {
        report: observation.report.clone(),
        observation: Box::new(observation),
        human_decisions,
    }
}

pub(crate) fn blocked_outcome(
    proof: &CompiledJourneyProof,
    detail: impl Into<String>,
) -> ExecutionOutcome {
    complete_outcome(
        proof,
        blocked_runtime_report(proof, detail),
        Vec::new(),
        None,
    )
}
